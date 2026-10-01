//! Typed evidence adapters: existing EmuWiz state in, [`GuidanceFact`]s out.
//!
//! Pure functions of values the caller already holds. Nothing here reads the
//! database, the filesystem or the network, renders anything, or decides anything:
//! the canonical subsystem's answer is carried across, never re-derived.
//!
//! **Authority rule.** Mr Wiz is non-authoritative. Every adapter maps a canonical
//! value to a fact only when the fact says no more than the value does.
//! Unknown, candidate, needs-review, unsupported, partial and "not loaded" never
//! become verified, ready or success; they yield a weaker fact or no fact. `None`
//! in any snapshot means "unknown / not loaded" and yields nothing, so the engine
//! stays silent instead of reading absence as "nothing found".
//!
//! Phase 2A builds this layer only. No page calls it yet.

#![allow(dead_code)]

use archivefs_core::launch::{CanonicalIdentityStatus, FirmwareReadiness};

use super::model::GuidanceFact as F;
use super::model::GuidanceFact;
use crate::gamer_artwork::{CoverAnswer, NoCover};
use crate::gui_v2::launch_readiness_summary::{
    FirmwareSummary, GameReadinessSummary, ReadinessFreshness, ReadinessPresentationState,
};

/// Library and scan state. Canonical source: the page's loaded library and the
/// scan job's own progress; counts are supplied only once a load has finished.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LibraryGuidanceEvidence {
    pub(crate) sources_configured: Option<bool>,
    /// Games in a *completed* library load; `None` while loading or failed.
    pub(crate) games_loaded: Option<u64>,
    pub(crate) scan_running: Option<bool>,
    /// Folders a finished scan could not read.
    pub(crate) scan_unreadable_folders: Option<u64>,
    /// DATs available to the identity check; `None` until known.
    pub(crate) dats_available: Option<u64>,
}

impl LibraryGuidanceEvidence {
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        let mut facts = Vec::new();
        if self.sources_configured == Some(false) {
            facts.push(F::SourcesNoneConfigured);
        }
        if self.scan_running == Some(true) {
            facts.push(F::ScanRunning);
        }
        if let Some(folder_count) = self.scan_unreadable_folders.filter(|n| *n > 0) {
            facts.push(F::ScanPartialFailure { folder_count });
        }
        if self.games_loaded == Some(0) {
            facts.push(F::LibraryLoadedEmpty);
        }
        if self.dats_available == Some(0) {
            facts.push(F::DatNoneAvailable);
        }
        facts
    }
}

/// Source health. Canonical source: the source availability the page already
/// projects. Catalogue-health review state is deliberately not read here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceAvailability {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SourceGuidanceEvidence {
    pub(crate) availability: Option<SourceAvailability>,
    /// An optional provider (not the user's games) could not be reached.
    pub(crate) optional_provider_unreachable: Option<bool>,
    /// The source differs from what a pending preview was built against.
    pub(crate) changed_since_preview: Option<bool>,
}

impl SourceGuidanceEvidence {
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        let mut facts = Vec::new();
        if self.availability == Some(SourceAvailability::Unavailable) {
            facts.push(F::SourceUnavailable);
        }
        if self.optional_provider_unreachable == Some(true) {
            facts.push(F::OptionalSourceUnavailable);
        }
        if self.changed_since_preview == Some(true) {
            facts.push(F::SourceChangedSincePreview);
        }
        facts
    }
}

/// The selected game and its identity. Canonical source: the launch planner's
/// [`CanonicalIdentityStatus`]. A resolved identity yields no design fact: the
/// design catalogue has no "verified" script, and Mr Wiz does not announce it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GameGuidanceEvidence {
    /// `Some(false)` when the page has loaded and nothing is selected.
    pub(crate) game_selected: Option<bool>,
    pub(crate) identity: Option<CanonicalIdentityStatus>,
    /// A title the identity layer proposed but did not confirm.
    pub(crate) candidate_title: Option<String>,
    /// The operation in hand refuses anything short of a verified identity.
    pub(crate) operation_needs_verified_identity: Option<bool>,
}

impl GameGuidanceEvidence {
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        let mut facts = Vec::new();
        if self.game_selected == Some(false) {
            facts.push(F::NoGameSelected);
        }
        match &self.identity {
            Some(CanonicalIdentityStatus::Conflicting) => facts.push(F::IdentityConflicting),
            Some(CanonicalIdentityStatus::Unknown) => {
                facts.push(F::IdentityUnknown);
                // A candidate is only ever a candidate beside an unknown identity.
                if let Some(title) = &self.candidate_title {
                    facts.push(F::IdentityCandidateOnly {
                        title: title.clone(),
                    });
                }
                if self.operation_needs_verified_identity == Some(true) {
                    facts.push(F::OperationNeedsStrongerIdentity);
                }
            }
            Some(CanonicalIdentityStatus::Resolved(_)) | None => {}
        }
        facts
    }
}

/// An emulator installation count, as the environment report states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EmulatorInstallations {
    pub(crate) emulator: String,
    pub(crate) count: u64,
}

/// Launch readiness. Canonical source: [`GameReadinessSummary`], the existing
/// projection of the launch plan, plus core [`FirmwareReadiness`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LaunchGuidanceEvidence {
    pub(crate) readiness: Option<GameReadinessSummary>,
    pub(crate) firmware: Option<FirmwareReadiness>,
    pub(crate) installations: Option<EmulatorInstallations>,
    /// A BIOS-folder chooser exists on this page.
    pub(crate) bios_folder_chooser_available: Option<bool>,
}

impl LaunchGuidanceEvidence {
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        let mut facts = Vec::new();
        if let Some(summary) = &self.readiness {
            let current = summary.freshness == ReadinessFreshness::Current;
            match summary.status {
                // Ready only when current and carrying no warning to acknowledge.
                ReadinessPresentationState::Ready if current && summary.warnings.is_empty() => {
                    facts.push(F::LaunchReady);
                }
                ReadinessPresentationState::NeedsEmulator if current => {
                    facts.push(F::LaunchNoCompatibleEmulator);
                }
                ReadinessPresentationState::NeedsFirmware if current => {
                    facts.push(F::FirmwareMissing);
                }
                // Checking, stale, warnings, identity review and the other blocked
                // states are not mapped: the summary does not say enough.
                _ => {}
            }
            if current && summary.firmware == FirmwareSummary::Missing {
                facts.push(F::FirmwareMissing);
            }
        }
        if self.firmware == Some(FirmwareReadiness::Missing) {
            facts.push(F::FirmwareMissing);
        }
        if let Some(found) = &self.installations {
            facts.push(F::EmulatorMultipleInstallations {
                emulator: found.emulator.clone(),
                count: found.count,
            });
        }
        if self.bios_folder_chooser_available == Some(true) {
            facts.push(F::BiosFolderChooserAvailable);
        }
        dedup(facts)
    }
}

/// Cover artwork. Canonical source: the artwork resolver's [`CoverAnswer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoverEvidence {
    Usable,
    NoneRecorded,
    SourceUnavailable,
}

impl CoverEvidence {
    /// `None` for answers that say nothing safe (failed, public-only, no RomM
    /// identity): those are not reinterpreted.
    pub(crate) fn from_answer(answer: &CoverAnswer) -> Option<Self> {
        match answer {
            CoverAnswer::Ready(_) | CoverAnswer::Unchanged { .. } => Some(Self::Usable),
            CoverAnswer::None(NoCover::NoArtwork) => Some(Self::NoneRecorded),
            CoverAnswer::None(NoCover::Unavailable) => Some(Self::SourceUnavailable),
            CoverAnswer::None(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ArtworkGuidanceEvidence {
    pub(crate) cover: Option<CoverEvidence>,
}

impl ArtworkGuidanceEvidence {
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        match self.cover {
            Some(CoverEvidence::Usable) => vec![F::ArtworkCachedUsable],
            Some(CoverEvidence::NoneRecorded) => vec![F::ArtworkCoverMissing],
            Some(CoverEvidence::SourceUnavailable) => vec![F::ArtworkSourceUnavailable],
            None => Vec::new(),
        }
    }
}

/// Cheats. Canonical source: the cheat catalogue count the page loaded. How a
/// cheat applies to a game stays with the applicability model; it is not copied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CheatGuidanceEvidence {
    /// Known cheats for the selected game, once the lookup finished.
    pub(crate) known_cheats: Option<u64>,
}

impl CheatGuidanceEvidence {
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        match self.known_cheats {
            Some(0) => vec![F::CheatsNoneKnown],
            _ => Vec::new(),
        }
    }
}

/// How a repair was verified, as the repair result itself states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepairVerification {
    Verified,
    Pending,
    Unavailable,
}

/// A finished operation, with its identity so a new one is a new event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OperationOutcome {
    LaunchFailed {
        operation_id: u64,
        plain_failure_reason: String,
    },
    RepairCompleted {
        operation_id: u64,
        verification: RepairVerification,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OperationGuidanceEvidence {
    pub(crate) outcome: Option<OperationOutcome>,
    pub(crate) queued: Option<u64>,
    pub(crate) running: Option<u64>,
    pub(crate) destination_read_only: Option<bool>,
    /// `(format, operation)` the canonical capability answer refused.
    pub(crate) unsupported: Option<(String, String)>,
}

impl OperationGuidanceEvidence {
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        let mut facts = Vec::new();
        match &self.outcome {
            Some(OperationOutcome::LaunchFailed {
                operation_id,
                plain_failure_reason,
            }) => facts.push(F::LaunchFailed {
                plain_failure_reason: plain_failure_reason.clone(),
                operation_id: *operation_id,
            }),
            Some(OperationOutcome::RepairCompleted {
                operation_id,
                verification,
            }) => {
                let operation_id = *operation_id;
                match verification {
                    RepairVerification::Verified => {
                        facts.push(F::RepairCompletedVerified { operation_id });
                    }
                    RepairVerification::Pending => {
                        facts.push(F::RepairCompletedVerificationPending { operation_id });
                    }
                    RepairVerification::Unavailable => {
                        facts.push(F::RepairCompletedVerificationPending { operation_id });
                        facts.push(F::VerificationUnavailable);
                    }
                }
            }
            None => {}
        }
        if let Some(queued_count) = self.queued.filter(|n| *n > 0) {
            facts.push(F::QueuedWork {
                queued_count,
                running_count: self.running.unwrap_or(0),
            });
        }
        if self.destination_read_only == Some(true) {
            facts.push(F::DestinationReadOnly);
        }
        if let Some((format, operation)) = &self.unsupported {
            facts.push(F::FormatOperationUnsupported {
                format: format.clone(),
                operation: operation.clone(),
            });
        }
        facts
    }
}

/// Everything a page can hand over; each part is optional state of its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PageGuidanceEvidence {
    pub(crate) library: LibraryGuidanceEvidence,
    pub(crate) source: SourceGuidanceEvidence,
    pub(crate) game: GameGuidanceEvidence,
    pub(crate) launch: LaunchGuidanceEvidence,
    pub(crate) artwork: ArtworkGuidanceEvidence,
    pub(crate) cheats: CheatGuidanceEvidence,
    pub(crate) operation: OperationGuidanceEvidence,
}

impl PageGuidanceEvidence {
    /// Valid, de-duplicated facts in a fixed order. The selector ignores order, so
    /// this order carries no meaning; it only makes the output reproducible.
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        let mut facts = Vec::new();
        facts.extend(self.library.facts());
        facts.extend(self.source.facts());
        facts.extend(self.game.facts());
        facts.extend(self.launch.facts());
        facts.extend(self.artwork.facts());
        facts.extend(self.cheats.facts());
        facts.extend(self.operation.facts());
        facts.retain(GuidanceFact::is_valid);
        dedup(facts)
    }
}

/// One fact per kind: the selector takes the first of each anyway.
fn dedup(facts: Vec<GuidanceFact>) -> Vec<GuidanceFact> {
    let mut seen = Vec::new();
    facts
        .into_iter()
        .filter(|fact| {
            let kind = fact.kind();
            let new = !seen.contains(&kind);
            seen.push(kind);
            new
        })
        .collect()
}
