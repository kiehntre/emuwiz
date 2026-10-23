//! Whether the saved evidence a launch would rest on is current.
//!
//! A scan whose inspection of a file did not complete (a read failed part-way,
//! the file changed while open) keeps the row's detector-owned platform and
//! saved identity report as history but marks the row `RetryAvailable`. That
//! mark is not a finding about the file: the file may be perfectly fine and
//! the next completed inspection clears it. Until then the saved report and
//! platform were produced by a pass that did not finish, so they may not
//! authorise a launch on their own.
//!
//! This module is the one place that decision lives. It changes no stored
//! data and adds no launch path: a stale report simply resolves to
//! [`CanonicalIdentityStatus::Unknown`], which every planner and command
//! builder already refuses, and the reason is named separately so a person is
//! told "rescan" instead of "this game is unidentified".
use super::VerifiedIdentityFact;
use super::evidence_bridge::canonical_identity_from_game_report;
use super::planning::CanonicalIdentityStatus;
use crate::ArchiveHealth;
use crate::game_identity::GameIdentityReport;

/// What a person is told. Never describes the file as damaged.
pub const INSPECTION_INCOMPLETE_REASON: &str = "The last scan could not finish reading this \
game's file, so what EmuWiz saved about it may be out of date. Rescan its source to try again; \
nothing about the game or its file was changed.";

/// Prefix of the content-reference provenance for an incomplete inspection,
/// which the shared content blocker repeats instead of its generic wording.
pub(crate) const INSPECTION_INCOMPLETE_PROVENANCE: &str = "inspection incomplete: ";

/// Whether the saved evidence for one catalogued file is current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceFreshness {
    /// The last inspection completed (or no inspection problem is recorded).
    Current,
    /// The last inspection did not complete; a rescan can restore it.
    InspectionIncomplete,
}

impl EvidenceFreshness {
    pub fn from_health(health: ArchiveHealth) -> Self {
        if health == ArchiveHealth::RetryAvailable {
            Self::InspectionIncomplete
        } else {
            Self::Current
        }
    }

    /// From a persisted `archives.last_known_health` value.
    pub fn from_stored_health(stored: &str) -> Self {
        if stored == ArchiveHealth::RetryAvailable.to_string() {
            Self::InspectionIncomplete
        } else {
            Self::Current
        }
    }

    pub fn is_incomplete(self) -> bool {
        self == Self::InspectionIncomplete
    }
}

/// [`canonical_identity_from_game_report`] for a *saved* report: the report is
/// consulted only while the inspection behind it is current. The report itself
/// is untouched; this only declines to rely on it.
pub fn canonical_identity_for_saved_report(
    report: &GameIdentityReport,
    freshness: EvidenceFreshness,
) -> (CanonicalIdentityStatus, Vec<VerifiedIdentityFact>) {
    if freshness.is_incomplete() {
        return (CanonicalIdentityStatus::Unknown, Vec::new());
    }
    canonical_identity_from_game_report(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_retry_mark_is_incomplete_and_the_stored_spelling_round_trips() {
        for health in [
            ArchiveHealth::Pending,
            ArchiveHealth::Mounted,
            ArchiveHealth::Failed,
            ArchiveHealth::MissingParts,
            ArchiveHealth::Corrupt,
            ArchiveHealth::Unsupported,
            ArchiveHealth::PermissionDenied,
        ] {
            assert_eq!(
                EvidenceFreshness::from_health(health),
                EvidenceFreshness::Current
            );
            assert_eq!(
                EvidenceFreshness::from_stored_health(&health.to_string()),
                EvidenceFreshness::Current
            );
        }
        assert!(EvidenceFreshness::from_health(ArchiveHealth::RetryAvailable).is_incomplete());
        assert!(EvidenceFreshness::from_stored_health("RetryAvailable").is_incomplete());
        assert_eq!(
            EvidenceFreshness::from_stored_health("anything else"),
            EvidenceFreshness::Current
        );
    }

    #[test]
    fn the_reason_blames_the_scan_not_the_file() {
        assert!(INSPECTION_INCOMPLETE_REASON.contains("Rescan"));
        assert!(
            !INSPECTION_INCOMPLETE_REASON
                .to_lowercase()
                .contains("corrupt")
        );
    }
}
