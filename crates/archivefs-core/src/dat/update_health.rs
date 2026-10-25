//! Read-only health and update-all planning recovered from 35cf7cca.
//! Reuses provider outcomes and installed identities; never opens a catalogue
//! database, downloads a snapshot, or applies an update. Evidence is session
//! scoped and bound to the installed SHA-256, not to a display name.

use crate::dat::updates::{ManagedDatUpdateFailureKind, ManagedDatUpdateOutcome};

pub const STALE_AFTER_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Current,
    UpdateAvailable,
    Stale,
    Unknown,
    CheckFailed,
}

impl Health {
    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::UpdateAvailable => "Update available",
            Self::Stale => "Stale",
            Self::Unknown => "Unknown",
            Self::CheckFailed => "Check failed",
        }
    }
}

/// Last attempt and last success are separate: failure never destroys evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckHistory {
    pub last_attempt: Option<Check>,
    pub last_success: Option<Check>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub installed_sha256: String,
    pub checked_at: u64,
    pub available_version: Option<String>,
    pub result: CheckResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    Current,
    UpdateAvailable { available_version: String },
    Unknown,
    Failed { detail: String },
}

impl CheckHistory {
    pub fn record(&mut self, check: Check) {
        if matches!(
            check.result,
            CheckResult::Current | CheckResult::UpdateAvailable { .. }
        ) {
            self.last_success = Some(check.clone());
        }
        self.last_attempt = Some(check);
    }
}

/// The provider owns comparison rules. In particular a Git commit is not a
/// numeric version, and changed Redump content is not necessarily a downgrade.
pub fn check_from_managed_outcome(
    outcome: &ManagedDatUpdateOutcome,
    installed_sha256: String,
    checked_at: u64,
) -> Check {
    let result = match outcome {
        ManagedDatUpdateOutcome::UpToDate { upstream_revision }
            if upstream_revision
                .as_deref()
                .is_some_and(|revision| !revision.trim().is_empty()) =>
        {
            CheckResult::Current
        }
        ManagedDatUpdateOutcome::UpToDate { .. } => CheckResult::Unknown,
        ManagedDatUpdateOutcome::UpdateAvailable { upstream_revision } => {
            if upstream_revision.trim().is_empty() {
                CheckResult::Unknown
            } else {
                CheckResult::UpdateAvailable {
                    available_version: upstream_revision.clone(),
                }
            }
        }
        ManagedDatUpdateOutcome::Offline => CheckResult::Failed {
            detail: "Provider is offline".into(),
        },
        ManagedDatUpdateOutcome::RateLimited {
            retry_after_seconds,
        } => CheckResult::Failed {
            detail: format!(
                "Provider rate limited the check; retry after {} seconds",
                retry_after_seconds.unwrap_or(60)
            ),
        },
        ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::UnsupportedAcquisition,
            ..
        }
        | ManagedDatUpdateOutcome::Disabled => CheckResult::Unknown,
        ManagedDatUpdateOutcome::Failed { detail, .. } => CheckResult::Failed {
            detail: detail.clone(),
        },
        // Installation is not a comparison.
        ManagedDatUpdateOutcome::Updated { .. } => CheckResult::Unknown,
    };
    let available_version = match outcome {
        ManagedDatUpdateOutcome::UpToDate { upstream_revision } => upstream_revision.clone(),
        ManagedDatUpdateOutcome::UpdateAvailable { upstream_revision } => {
            Some(upstream_revision.clone())
        }
        _ => None,
    }
    .filter(|version| !version.trim().is_empty());
    Check {
        installed_sha256,
        checked_at,
        available_version,
        result,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub health: Health,
    pub last_checked: Option<u64>,
    pub available_version: Option<String>,
    pub detail: String,
}

pub fn assess_health(
    installed_sha256: Option<&str>,
    history: &CheckHistory,
    now: u64,
) -> Assessment {
    let mut assessment = Assessment {
        health: Health::Unknown,
        last_checked: history.last_attempt.as_ref().map(|check| check.checked_at),
        available_version: None,
        detail: "No applicable provider comparison. Installation and validation do not prove currentness.".into(),
    };
    let Some(hash) = installed_sha256.filter(|hash| valid_sha256(hash)) else {
        return assessment;
    };
    let Some(check) = history
        .last_attempt
        .as_ref()
        .filter(|check| check.installed_sha256 == hash)
    else {
        return assessment;
    };
    match &check.result {
        CheckResult::Failed { detail } => {
            assessment.health = Health::CheckFailed;
            assessment.detail = detail.clone();
            // Keep known candidate information without claiming the failed
            // service comparison succeeded.
            if let Some(Check {
                installed_sha256,
                result: CheckResult::UpdateAvailable { available_version },
                ..
            }) = &history.last_success
                && installed_sha256 == hash
            {
                assessment.available_version = Some(available_version.clone());
            }
        }
        CheckResult::Current | CheckResult::UpdateAvailable { .. } => {
            assessment.available_version = check.available_version.clone();
            if check.checked_at > now {
                assessment.detail =
                    "Provider check timestamp is in the future; check again.".into();
                return assessment;
            }
            if let CheckResult::UpdateAvailable { available_version } = &check.result {
                assessment.available_version = Some(available_version.clone());
            }
            assessment.health = if now - check.checked_at > STALE_AFTER_SECONDS {
                Health::Stale
            } else if matches!(check.result, CheckResult::Current) {
                Health::Current
            } else {
                Health::UpdateAvailable
            };
            assessment.detail = if assessment.health == Health::Stale {
                "Provider evidence is older than seven days. Check again before planning.".into()
            } else {
                "Compared by the existing managed DAT provider against this installed SHA-256."
                    .into()
            };
        }
        CheckResult::Unknown => {}
    }
    assessment
}

pub fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanDisposition {
    Eligible,
    Current,
    CheckFirst,
    Unsupported,
    Conflict,
}

/// A plan is descriptive only; it confers no replacement authority.
pub fn plan_update_all(
    assessment: &Assessment,
    supported: bool,
    conflict: bool,
) -> PlanDisposition {
    if conflict {
        PlanDisposition::Conflict
    } else if !supported {
        PlanDisposition::Unsupported
    } else {
        match assessment.health {
            Health::UpdateAvailable => PlanDisposition::Eligible,
            Health::Current => PlanDisposition::Current,
            _ => PlanDisposition::CheckFirst,
        }
    }
}

#[cfg(test)]
mod tests;
