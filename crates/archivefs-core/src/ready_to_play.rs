//! Pure Ready-to-Play projection over evidence gathered by existing workflows.
//!
//! This module deliberately performs no discovery, probing, hashing, I/O, or
//! launch authorization. Callers pass the current launch plan and any other
//! already-gathered evidence; the result is a cheap, deterministic view.

use serde::Serialize;

use crate::attention::{
    AttentionCategory, AttentionDestination, AttentionItem, AttentionSeverity, AttentionSnapshot,
};
use crate::diagnostics::DoctorSeverity;
use crate::launch::planning::{CanonicalIdentityStatus, LaunchPlan};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReadyToPlayState {
    Ready,
    ReadyWithWarnings,
    NeedsAttention,
    Blocked,
    Unsupported,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessReasonFamily {
    Identity,
    Content,
    MediaTopology,
    Firmware,
    Emulator,
    Configuration,
    Dependency,
    Arcade,
    DatCompatibility,
    ModOrPatch,
    Controller,
    LaunchPlan,
    Unsupported,
    UnknownEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fixability {
    InformationOnly,
    UserCanFix,
    EmuwizCanGuide,
    EmuwizCanRepairSafely,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessEvidenceState {
    Gathered,
    NotGathered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityEvidenceState {
    Resolved,
    Conflicting,
    Unknown,
    FilenameOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaEvidenceState {
    Complete,
    Incomplete,
    Unknown,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorEvidenceState {
    Ready,
    Unsupported,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModEvidenceState {
    None,
    Compatible,
    Conflicting,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadinessReason {
    pub family: ReadinessReasonFamily,
    pub severity: DoctorSeverity,
    pub summary: String,
    pub technical_detail: String,
    pub provenance: String,
    pub fixability: Fixability,
    /// The original launch evidence remains available to typed consumers.
    #[serde(skip_serializing)]
    pub original_blocker: Option<LaunchBlockerKind>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadyToPlayResult {
    pub item_identity: String,
    pub platform_id: Option<String>,
    pub state: ReadyToPlayState,
    pub reasons: Vec<ReadinessReason>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyToPlayEvidence<'a> {
    pub item_identity: String,
    pub launch_plan: Option<&'a LaunchPlan>,
    pub launch_plan_state: ReadinessEvidenceState,
    pub identity: IdentityEvidenceState,
    pub emulator: EmulatorEvidenceState,
    pub firmware: ReadinessEvidenceState,
    pub media: MediaEvidenceState,
    pub mod_state: ModEvidenceState,
    pub arcade_dependency_blocked: bool,
    pub save_state_version_sensitive: bool,
}

impl<'a> ReadyToPlayEvidence<'a> {
    pub fn from_launch_plan(item_identity: impl Into<String>, plan: &'a LaunchPlan) -> Self {
        let identity = if plan.platform_id.is_some() && plan.game_key.is_some() {
            IdentityEvidenceState::Resolved
        } else {
            IdentityEvidenceState::Unknown
        };
        Self {
            item_identity: item_identity.into(),
            launch_plan: Some(plan),
            launch_plan_state: ReadinessEvidenceState::Gathered,
            identity,
            emulator: EmulatorEvidenceState::Ready,
            firmware: ReadinessEvidenceState::Gathered,
            media: MediaEvidenceState::NotApplicable,
            mod_state: ModEvidenceState::None,
            arcade_dependency_blocked: false,
            save_state_version_sensitive: false,
        }
    }
}

fn reason(
    family: ReadinessReasonFamily,
    severity: DoctorSeverity,
    summary: impl Into<String>,
    detail: impl Into<String>,
    provenance: impl Into<String>,
    fixability: Fixability,
    original_blocker: Option<LaunchBlockerKind>,
) -> ReadinessReason {
    ReadinessReason {
        family,
        severity,
        summary: summary.into(),
        technical_detail: detail.into(),
        provenance: provenance.into(),
        fixability,
        original_blocker,
    }
}

fn blocker_reason(blocker: &LaunchBlocker) -> ReadinessReason {
    let kind_text = format!("{:?}", blocker.kind);
    let family = match blocker.kind {
        LaunchBlockerKind::IdentityUnresolved | LaunchBlockerKind::IdentityConflict => {
            ReadinessReasonFamily::Identity
        }
        LaunchBlockerKind::ContentNotResolved => ReadinessReasonFamily::Content,
        LaunchBlockerKind::MediaTopologyBlocked
        | LaunchBlockerKind::MediaTopologyReviewRequired => ReadinessReasonFamily::MediaTopology,
        LaunchBlockerKind::RequiredFirmwareMissing => ReadinessReasonFamily::Firmware,
        LaunchBlockerKind::NoInstallationCandidate
        | LaunchBlockerKind::ProfileIneligible
        | LaunchBlockerKind::CoreMissing
        | LaunchBlockerKind::RetroArchProfileMissing
        | LaunchBlockerKind::RetroArchExecutableMissing
        | LaunchBlockerKind::DolphinCandidateRequired
        | LaunchBlockerKind::DolphinPlatformMismatch => ReadinessReasonFamily::Emulator,
        LaunchBlockerKind::MameDependencyBlocked | LaunchBlockerKind::FbneoDependencyBlocked => {
            ReadinessReasonFamily::Dependency
        }
        _ if kind_text.contains("Unsupported") => ReadinessReasonFamily::Unsupported,
        _ => ReadinessReasonFamily::LaunchPlan,
    };
    let fixability = match family {
        ReadinessReasonFamily::Unsupported => Fixability::Unsupported,
        ReadinessReasonFamily::Identity | ReadinessReasonFamily::Firmware => Fixability::UserCanFix,
        ReadinessReasonFamily::MediaTopology | ReadinessReasonFamily::Dependency => {
            Fixability::EmuwizCanGuide
        }
        _ => Fixability::InformationOnly,
    };
    reason(
        family,
        DoctorSeverity::Error,
        format!("Launch requires attention: {:?}", blocker.kind),
        blocker.detail.clone(),
        "LaunchPlan / LaunchCandidate",
        fixability,
        Some(blocker.kind),
    )
}

fn warning_reason(detail: &str) -> ReadinessReason {
    reason(
        ReadinessReasonFamily::LaunchPlan,
        DoctorSeverity::Warning,
        "The game is launchable with a warning",
        detail,
        "LaunchPlan / LaunchCandidate",
        Fixability::InformationOnly,
        None,
    )
}

fn sort_reasons(reasons: &mut [ReadinessReason]) {
    reasons.sort_by_key(|reason| {
        (
            reason.severity.rank(),
            reason.family as u8,
            reason.summary.clone(),
            reason.technical_detail.clone(),
        )
    });
}

/// Aggregate current evidence using the documented precedence:
/// unsupported, proven blocked, fixable attention, unknown evidence, warning,
/// then ready. Unknown never masks a proven blocker.
pub fn project_ready_to_play(evidence: &ReadyToPlayEvidence<'_>) -> ReadyToPlayResult {
    let mut reasons = Vec::new();
    let plan = evidence.launch_plan;

    if evidence.identity == IdentityEvidenceState::FilenameOnly {
        reasons.push(reason(
            ReadinessReasonFamily::Identity,
            DoctorSeverity::Error,
            "Launch identity is not confirmed",
            "Filename-only evidence is not promoted to launch identity.",
            "identity evidence",
            Fixability::UserCanFix,
            None,
        ));
    }
    if evidence.identity == IdentityEvidenceState::Conflicting {
        reasons.push(reason(
            ReadinessReasonFamily::Identity,
            DoctorSeverity::Error,
            "Identity evidence conflicts",
            "Existing identity evidence did not resolve to one launch identity.",
            "identity evidence",
            Fixability::UserCanFix,
            None,
        ));
    }
    if evidence.identity == IdentityEvidenceState::Unknown {
        reasons.push(reason(
            ReadinessReasonFamily::UnknownEvidence,
            DoctorSeverity::Info,
            "Launch identity is not currently established",
            "Required identity evidence has not been gathered or resolved.",
            "identity evidence",
            Fixability::InformationOnly,
            None,
        ));
    }
    if evidence.media == MediaEvidenceState::Incomplete {
        reasons.push(reason(
            ReadinessReasonFamily::MediaTopology,
            DoctorSeverity::Error,
            "Required media is incomplete",
            "The existing media topology reports a required member is missing.",
            "media_set / launch topology",
            Fixability::EmuwizCanGuide,
            None,
        ));
    }
    if evidence.emulator == EmulatorEvidenceState::Unsupported {
        reasons.push(reason(
            ReadinessReasonFamily::Unsupported,
            DoctorSeverity::Error,
            "This launch adapter is unsupported",
            "No supported launch adapter was established for this item.",
            "emulator/profile evidence",
            Fixability::Unsupported,
            None,
        ));
    }
    if evidence.arcade_dependency_blocked {
        reasons.push(reason(
            ReadinessReasonFamily::Dependency,
            DoctorSeverity::Error,
            "Arcade dependencies are incomplete",
            "Existing arcade dependency evidence blocks this launch.",
            "arcade election / DAT dependency evidence",
            Fixability::EmuwizCanGuide,
            None,
        ));
    }
    if let Some(plan) = plan {
        for candidate in &plan.candidates {
            reasons.extend(candidate.blockers.iter().map(blocker_reason));
            reasons.extend(
                candidate
                    .warnings
                    .iter()
                    .map(|warning| warning_reason(&format!("{:?}", warning.kind))),
            );
        }
        if plan.candidates.is_empty() {
            reasons.push(reason(
                ReadinessReasonFamily::LaunchPlan,
                DoctorSeverity::Error,
                "No valid launch plan exists",
                "The existing launch planner produced no candidate.",
                "LaunchPlan",
                Fixability::EmuwizCanGuide,
                None,
            ));
        }
    } else if evidence.launch_plan_state == ReadinessEvidenceState::NotGathered {
        reasons.push(reason(
            ReadinessReasonFamily::UnknownEvidence,
            DoctorSeverity::Warning,
            "Launch readiness has not been gathered",
            "A current LaunchPlan is not available; this is unknown, not missing.",
            "launch planning coverage",
            Fixability::InformationOnly,
            None,
        ));
    }
    if evidence.emulator == EmulatorEvidenceState::Unknown
        || evidence.firmware == ReadinessEvidenceState::NotGathered
        || evidence.media == MediaEvidenceState::Unknown
        || evidence.mod_state == ModEvidenceState::Unknown
    {
        reasons.push(reason(
            ReadinessReasonFamily::UnknownEvidence,
            DoctorSeverity::Info,
            "Some readiness evidence is not available",
            "Required evidence has not been gathered; it is not treated as missing.",
            "diagnostic coverage",
            Fixability::InformationOnly,
            None,
        ));
    }

    if evidence.save_state_version_sensitive {
        reasons.push(reason(
            ReadinessReasonFamily::Configuration,
            DoctorSeverity::Info,
            "Save-state resume may depend on emulator version",
            "This informational condition does not lower game launch readiness.",
            "emulator save-state evidence",
            Fixability::InformationOnly,
            None,
        ));
    }
    sort_reasons(&mut reasons);

    let has_unsupported = reasons
        .iter()
        .any(|r| r.family == ReadinessReasonFamily::Unsupported);
    let has_blocker = reasons.iter().any(|r| r.severity.is_blocking());
    let only_firmware_blockers = has_blocker
        && reasons
            .iter()
            .filter(|r| r.severity.is_blocking())
            .all(|r| r.family == ReadinessReasonFamily::Firmware);
    let has_unknown = reasons
        .iter()
        .any(|r| r.family == ReadinessReasonFamily::UnknownEvidence);
    let has_warning = reasons
        .iter()
        .any(|r| r.severity == DoctorSeverity::Warning);
    let state = if has_unsupported {
        ReadyToPlayState::Unsupported
    } else if only_firmware_blockers {
        ReadyToPlayState::NeedsAttention
    } else if has_blocker {
        ReadyToPlayState::Blocked
    } else if evidence.launch_plan_state == ReadinessEvidenceState::NotGathered || has_unknown {
        ReadyToPlayState::Unknown
    } else if has_warning || reasons.iter().any(|r| r.severity == DoctorSeverity::Info) {
        ReadyToPlayState::ReadyWithWarnings
    } else if plan
        .map(|p| {
            p.candidates
                .iter()
                .any(|c| c.readiness == LaunchReadiness::Ready)
        })
        .unwrap_or(false)
    {
        ReadyToPlayState::Ready
    } else {
        ReadyToPlayState::Blocked
    };
    ReadyToPlayResult {
        item_identity: evidence.item_identity.clone(),
        platform_id: plan.and_then(|p| p.platform_id.clone()),
        state,
        reasons,
    }
}

/// Project a result into the existing bounded attention model.
pub fn readiness_attention(result: &ReadyToPlayResult) -> AttentionSnapshot {
    let mut snapshot = AttentionSnapshot::default();
    if matches!(result.state, ReadyToPlayState::Ready) {
        return snapshot;
    }
    let severity = match result.state {
        ReadyToPlayState::Blocked | ReadyToPlayState::Unsupported => AttentionSeverity::Blocking,
        ReadyToPlayState::NeedsAttention => AttentionSeverity::ActionNeeded,
        ReadyToPlayState::ReadyWithWarnings => AttentionSeverity::Warning,
        ReadyToPlayState::Unknown => AttentionSeverity::Info,
        ReadyToPlayState::Ready => unreachable!(),
    };
    let mut item = AttentionItem::new(
        format!("ready-to-play:{}", result.item_identity),
        AttentionCategory::Launch,
        severity,
        format!("Ready-to-Play: {:?}", result.state),
        AttentionDestination::LaunchReadiness,
    );
    item.affected = Some(result.item_identity.clone());
    item.platform = result.platform_id.clone();
    item.summary = result
        .reasons
        .first()
        .map(|reason| reason.summary.clone())
        .unwrap_or_else(|| "Review current launch evidence".into());
    item.provenance = "Ready-to-Play projection over current launch evidence".into();
    snapshot.insert(item);
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::{LaunchCandidate, LaunchContentRef, LaunchTarget};
    use crate::launch::readiness::{FirmwareReadiness, LaunchWarning};

    fn plan(readiness: LaunchReadiness, blocker: Option<LaunchBlocker>) -> LaunchPlan {
        LaunchPlan {
            platform_id: Some("ps1".into()),
            game_key: Some("SLUS-00001".into()),
            candidates: vec![LaunchCandidate {
                target: LaunchTarget::Standalone {
                    adapter_id: "duckstation",
                    profile_id: "default".into(),
                    profile_path: None,
                },
                content: LaunchContentRef {
                    kind: None,
                    container: None,
                    resolved_path: Some("/fixture/game.bin".into()),
                    requires_mount: false,
                    provenance: "fixture".into(),
                },
                firmware: FirmwareReadiness::NotRequired,
                blockers: blocker.into_iter().collect(),
                warnings: Vec::<LaunchWarning>::new(),
                readiness,
                preference: crate::launch::planning::CandidatePreference::SoleEligible,
            }],
            summary: Default::default(),
            media_topology: None,
        }
    }

    #[test]
    fn ready_and_warning_states_are_pure_projections() {
        let plan = plan(LaunchReadiness::Ready, None);
        let evidence = ReadyToPlayEvidence::from_launch_plan("game", &plan);
        assert_eq!(
            project_ready_to_play(&evidence).state,
            ReadyToPlayState::Ready
        );
        let mut warning = evidence;
        warning.emulator = EmulatorEvidenceState::Unknown;
        assert_eq!(
            project_ready_to_play(&warning).state,
            ReadyToPlayState::ReadyWithWarnings
        );
    }

    #[test]
    fn unknown_does_not_become_missing_or_mask_a_blocker() {
        let plan = plan(
            LaunchReadiness::Blocked,
            Some(LaunchBlocker::new(
                LaunchBlockerKind::RequiredFirmwareMissing,
                "fixture BIOS missing",
            )),
        );
        let mut evidence = ReadyToPlayEvidence::from_launch_plan("game", &plan);
        evidence.firmware = ReadinessEvidenceState::NotGathered;
        let result = project_ready_to_play(&evidence);
        assert_eq!(result.state, ReadyToPlayState::Blocked);
        assert!(
            result
                .reasons
                .iter()
                .any(|r| r.family == ReadinessReasonFamily::Firmware)
        );
        let missing = result
            .reasons
            .iter()
            .find(|r| r.family == ReadinessReasonFamily::Firmware)
            .unwrap();
        assert_eq!(missing.severity, DoctorSeverity::Error);
    }

    #[test]
    fn not_gathered_is_unknown_and_filename_identity_is_not_confirmed() {
        let evidence = ReadyToPlayEvidence {
            item_identity: "game".into(),
            launch_plan: None,
            launch_plan_state: ReadinessEvidenceState::NotGathered,
            identity: IdentityEvidenceState::FilenameOnly,
            emulator: EmulatorEvidenceState::Unknown,
            firmware: ReadinessEvidenceState::NotGathered,
            media: MediaEvidenceState::Unknown,
            mod_state: ModEvidenceState::Unknown,
            arcade_dependency_blocked: false,
            save_state_version_sensitive: false,
        };
        let result = project_ready_to_play(&evidence);
        assert_eq!(result.state, ReadyToPlayState::Blocked);
        assert!(
            result
                .reasons
                .iter()
                .any(|r| r.family == ReadinessReasonFamily::Identity)
        );
    }
}
