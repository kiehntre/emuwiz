//! Catalogue-side persistence of already-verified identity facts.
//!
//! # A cache, never the trust anchor
//!
//! Everything here is a *projection* of facts
//! [`crate::game_identity::GameIdentityReport`] already established as
//! [`IdentityStatus::Verified`]. It exists so Library, Doctor and other
//! read-only consumers can explain identity / launch readiness without
//! re-inspecting the content on every access.
//!
//! It is **never** consulted for launch or cheat/mod authorization. Those
//! paths keep re-verifying from a fresh `GameIdentityReport` exactly as they
//! do today (see [`crate::launch`]'s command planners, which take
//! `Option<&str>` verified values as arguments and never read a database).
//!
//! # What is persisted
//!
//! Only genuine, opaque per-game identity facts - the same set
//! [`crate::launch::evidence_bridge`] already treats as identity-conferring
//! ([`is_persistable_verified_identity_kind`]), plus the three Dolphin
//! qualifiers (`DolphinRevision` / `DolphinDiscNumber` / `DolphinRegion`)
//! that only ever accompany a verified `DolphinGameId`. Format/title/
//! platform metadata that merely happens to also carry `Verified` status is
//! excluded, [`IdentityConfidence::FilenameOnly`] is refused, and a report
//! that carries two different `Verified` values for one kind (a conflicting
//! report) persists neither - no evidence is promoted while being stored.
//!
//! # File-identity freshness
//!
//! Each fact is stored with the archive file's `(device, inode, size,
//! mtime)` snapshot - the same discipline
//! [`crate::launch::process_spawn::CapturedFileIdentity`] uses to notice a
//! file swapped at the same path. A later read compares that snapshot with
//! the file's current identity to derive [`IdentityFactFreshness`]. A stale
//! fact stays visible for explanation; it must never authorize a launch.
//!
//! The snapshot is metadata, so it is only trusted where metadata can be:
//! the stored mtime has whole-second resolution, so a file edited in the same
//! second as its stored mtime looks unchanged. A fact is therefore `Current`
//! only if the file object is identifiable (`device`/`inode`) on both sides and
//! its stored mtime precedes the recorded observation time by at least
//! [`RACY_MTIME_MARGIN_SECONDS`]. Anything else that matches is `Unknown`,
//! which every consumer treats as not-current.

use serde::{Deserialize, Serialize};

use crate::game_identity::{GameIdentityReport, IdentityConfidence, IdentityKind, IdentityStatus};
use crate::launch::evidence_bridge::is_identity_conferring;
use crate::launch::process_spawn::CapturedFileIdentity;

/// Whether a verified `(kind, value)` pair is one this cache stores.
///
/// This is [`crate::launch::evidence_bridge`]'s own identity-conferring set
/// (so the two never drift), widened by the three Dolphin qualifier kinds,
/// which are only ever emitted `Verified` alongside a verified
/// [`IdentityKind::DolphinGameId`] and are useful to persist next to it.
pub fn is_persistable_verified_identity_kind(kind: IdentityKind) -> bool {
    is_identity_conferring(kind)
        || matches!(
            kind,
            IdentityKind::DolphinRevision
                | IdentityKind::DolphinDiscNumber
                | IdentityKind::DolphinRegion
        )
}

/// One verified fact extracted from a report, ready to persist. Value and
/// confidence are carried exactly as the report produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistableVerifiedFact {
    pub kind: IdentityKind,
    pub value: String,
    pub confidence: IdentityConfidence,
    pub method: Option<String>,
    pub member_path: Option<Vec<u8>>,
}

/// Extracts every genuinely verified, persistable identity fact from
/// `report`.
///
/// A kind is skipped when: its status is not [`IdentityStatus::Verified`];
/// it is not [`is_persistable_verified_identity_kind`]; its confidence is
/// [`IdentityConfidence::FilenameOnly`]; it has no value; or the report
/// carries more than one `Verified` value for it that disagree (a
/// conflicting report - neither value is promoted).
pub fn persistable_verified_facts(report: &GameIdentityReport) -> Vec<PersistableVerifiedFact> {
    let mut out: Vec<PersistableVerifiedFact> = Vec::new();
    let mut conflicted: Vec<IdentityKind> = Vec::new();

    for evidence in &report.evidence {
        if evidence.status != IdentityStatus::Verified
            || !is_persistable_verified_identity_kind(evidence.kind)
            || evidence.confidence == IdentityConfidence::FilenameOnly
        {
            continue;
        }
        let Some(value) = evidence.value.as_deref() else {
            continue;
        };
        if conflicted.contains(&evidence.kind) {
            continue;
        }
        match out.iter_mut().find(|fact| fact.kind == evidence.kind) {
            Some(existing) if existing.value == value => {}
            Some(_) => {
                // A second, disagreeing verified value for the same kind:
                // this report is internally conflicting for this kind.
                // Drop what we had and never store a "winner".
                out.retain(|fact| fact.kind != evidence.kind);
                conflicted.push(evidence.kind);
            }
            None => out.push(PersistableVerifiedFact {
                kind: evidence.kind,
                value: value.to_string(),
                confidence: evidence.confidence,
                method: {
                    let method = evidence.provenance.method.trim();
                    (!method.is_empty()).then(|| method.to_string())
                },
                member_path: evidence.provenance.member_path.clone(),
            }),
        }
    }
    out
}

/// A verified identity fact as it was persisted for one catalogued archive.
/// A cache row - see the module docs on why it never authorizes anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedIdentityFact {
    pub archive_id: i64,
    pub kind: IdentityKind,
    /// The exact verified value, byte-for-byte as the report produced it.
    pub value: String,
    /// The confidence the report recorded for this evidence. Never
    /// [`IdentityConfidence::FilenameOnly`] for a persisted fact.
    pub confidence: IdentityConfidence,
    /// The evidence provenance's `method` string, when it carried one.
    pub method: Option<String>,
    /// The archive member the fact came from (raw bytes - paths are not
    /// guaranteed UTF-8), or `None` for the outer file.
    pub member_path: Option<Vec<u8>>,
    /// When the inspection that produced this fact was persisted.
    pub observed_at: String,
    /// `(device, inode, size, mtime)` snapshot of the archive file at
    /// inspection time.
    pub file_device: u64,
    pub file_inode: u64,
    pub file_size_bytes: u64,
    pub file_modified_unix_seconds: Option<i64>,
}

/// How many whole seconds a stored mtime must precede the fact's observation
/// time before an unchanged mtime counts as evidence of an unchanged file. The
/// margin covers whole-second mtime resolution plus the gap between capturing
/// the file identity and recording the observation.
pub const RACY_MTIME_MARGIN_SECONDS: i64 = 2;

/// Parses the strict `YYYY-MM-DDTHH:MM:SSZ` UTC form that
/// `format_unix_timestamp_utc` writes to `observed_at`. Anything else is
/// `None`: an unreadable observation time can never prove currency.
fn parse_utc_timestamp(value: &str) -> Option<i64> {
    let b = value.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = value.get(range)?;
        digits
            .bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day < 1 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    // Days from the civil date (proleptic Gregorian), epoch 1970-01-01.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Whether a persisted fact still describes the same bytes/object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityFactFreshness {
    /// The stored file-identity snapshot equals the file's current identity.
    Current,
    /// The file changed at the same path since the fact was stored - the
    /// fact describes a previous state and must not authorize a launch.
    Stale,
    /// Not enough comparable evidence to tell current from stale (the
    /// current file identity is unavailable, or a snapshot field could not
    /// be compared).
    Unknown,
}

fn system_time_unix_seconds(time: std::time::SystemTime) -> Option<i64> {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(delta) => i64::try_from(delta.as_secs()).ok(),
        Err(err) => i64::try_from(err.duration().as_secs())
            .ok()
            .map(|value| -value),
    }
}

impl PersistedIdentityFact {
    /// The stored snapshot as a [`CapturedFileIdentity`], for callers that
    /// want to compare it themselves.
    pub fn snapshot(&self) -> CapturedFileIdentity {
        CapturedFileIdentity {
            device: self.file_device,
            inode: self.file_inode,
            size: self.file_size_bytes,
            modified: self.file_modified_unix_seconds.and_then(|seconds| {
                u64::try_from(seconds)
                    .ok()
                    .map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s))
            }),
        }
    }

    /// Derives [`IdentityFactFreshness`] against the file's current identity.
    ///
    /// Positive evidence of change always wins:
    /// - `current` is `None` -> `Unknown`.
    /// - `(device, inode)` known on both sides and different -> `Stale`
    ///   (a different file now sits at that path).
    /// - `size` differs -> `Stale`.
    /// - `mtime` known on both sides and different -> `Stale`.
    ///
    /// `Current` needs positive evidence of sameness, not merely the absence
    /// of change: `(device, inode)` known on both sides and equal, `mtime`
    /// comparable and equal, and the stored `mtime` at least
    /// [`RACY_MTIME_MARGIN_SECONDS`] older than the (parseable) observation
    /// time, so a same-second rewrite cannot hide behind a whole-second
    /// timestamp. Any other agreement is `Unknown`.
    pub fn freshness(&self, current: Option<&CapturedFileIdentity>) -> IdentityFactFreshness {
        let Some(current) = current else {
            return IdentityFactFreshness::Unknown;
        };
        let stored_object_known = self.file_device != 0 || self.file_inode != 0;
        let current_object_known = current.device != 0 || current.inode != 0;
        if stored_object_known
            && current_object_known
            && (self.file_device, self.file_inode) != (current.device, current.inode)
        {
            return IdentityFactFreshness::Stale;
        }
        if self.file_size_bytes != current.size {
            return IdentityFactFreshness::Stale;
        }
        match (
            self.file_modified_unix_seconds,
            current.modified.and_then(system_time_unix_seconds),
        ) {
            (Some(stored), Some(now)) if stored != now => IdentityFactFreshness::Stale,
            (Some(stored), Some(_))
                if stored_object_known
                    && current_object_known
                    && self.mtime_precedes_observation(stored) =>
            {
                IdentityFactFreshness::Current
            }
            _ => IdentityFactFreshness::Unknown,
        }
    }

    /// Whether `stored_mtime` is old enough, relative to when this fact was
    /// observed, that an unchanged whole-second mtime means an unchanged file.
    fn mtime_precedes_observation(&self, stored_mtime: i64) -> bool {
        parse_utc_timestamp(&self.observed_at).is_some_and(|observed| {
            stored_mtime
                .checked_add(RACY_MTIME_MARGIN_SECONDS)
                .is_some_and(|earliest| earliest <= observed)
        })
    }
}

// --- (de)serialisation of enum keys for the database layer ---------------

/// The stable machine name a [`IdentityKind`] is stored under.
pub fn identity_kind_to_db(kind: IdentityKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{kind:?}"))
}

/// Parses a stored machine name back to an [`IdentityKind`].
pub fn identity_kind_from_db(value: &str) -> Option<IdentityKind> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

/// The stable machine name a [`IdentityConfidence`] is stored under.
pub fn identity_confidence_to_db(confidence: IdentityConfidence) -> String {
    serde_json::to_value(confidence)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{confidence:?}"))
}

/// Parses a stored machine name back to an [`IdentityConfidence`].
pub fn identity_confidence_from_db(value: &str) -> Option<IdentityConfidence> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

#[cfg(test)]
mod tests;
