//! Phase 2A coverage report: which catalogue scripts can receive real evidence
//! today, and which are blocked on product state. A test-only wiring checklist.
//!
//! Print with
//! `cargo test -p archivefs-gui --lib print_evidence_coverage -- --ignored --nocapture`.

use std::collections::BTreeSet;
use std::fmt;

use super::model::FactKind;
use super::script::GuidanceScript;

/// How far a fact is from real evidence. Ordered best to worst, so a script's
/// status is the worst status among the facts it requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Coverage {
    /// An existing typed value maps directly.
    AvailableNow,
    /// The state exists; the caller supplies a bool, count or enum.
    SimpleProjection,
    /// No state exists yet. Nothing may be invented.
    NeedsNewProductState,
    /// A subsystem exists but offers no safe public projection (or is out of lane).
    Deferred,
}

pub(super) struct FactCoverage {
    pub(super) kind: FactKind,
    pub(super) coverage: Coverage,
    pub(super) source: &'static str,
    /// The snapshot that emits the fact, or empty when no adapter exists yet.
    pub(super) adapter: &'static str,
}

pub(super) const ALL_KINDS: &[FactKind] = &[
    FactKind::LegacyLibraryEmpty,
    FactKind::LegacyLibraryHasGames,
    FactKind::LegacySourceUnavailable,
    FactKind::LegacySourceUnavailableWithScan,
    FactKind::LegacySourceLastScan,
    FactKind::LegacyLaunchIdentityUnverified,
    FactKind::LegacyLaunchIdentityVerified,
    FactKind::LegacyProblemBlocker,
    FactKind::LegacyProblemsChecking,
    FactKind::LegacyProblemsClear,
    FactKind::LegacyProblemsFindings,
    FactKind::LegacyTapeStructure,
    FactKind::LegacyDatIdentity,
    FactKind::LegacyJobsIdle,
    FactKind::LegacyJobsRunning,
    FactKind::FreshEnvironment,
    FactKind::HomeFirstUseTipEligible,
    FactKind::SourcesNoneConfigured,
    FactKind::SourceUnavailable,
    FactKind::ScanRunning,
    FactKind::ScanPartialFailure,
    FactKind::DatNoneAvailable,
    FactKind::IdentityUnknown,
    FactKind::OperationNeedsStrongerIdentity,
    FactKind::IdentityCandidateOnly,
    FactKind::IdentityConflicting,
    FactKind::ProblemsActionable,
    FactKind::NoFirstFindingTarget,
    FactKind::MameMissingMembers,
    FactKind::MameParentDependencyMissing,
    FactKind::MameBadDumpReference,
    FactKind::MameNoDumpReference,
    FactKind::ArtworkCoverMissing,
    FactKind::ArtworkRefreshAvailable,
    FactKind::IdentityPreventsArtworkMatch,
    FactKind::ArtworkCachedUsable,
    FactKind::ArtworkCacheStale,
    FactKind::ArtworkSourceUnavailable,
    FactKind::ArtworkAlternatives,
    FactKind::EmulatorMultipleInstallations,
    FactKind::FirmwareMissing,
    FactKind::BiosFolderChooserAvailable,
    FactKind::LaunchNoCompatibleEmulator,
    FactKind::LaunchReady,
    FactKind::LaunchWarningsUnacknowledged,
    FactKind::LaunchFailed,
    FactKind::NoGameSelected,
    FactKind::CheatsNoneKnown,
    FactKind::PatchBaseMismatch,
    FactKind::PatchRevisionMismatchKnown,
    FactKind::ConversionPreviewAvailable,
    FactKind::ConversionPreservationUnknown,
    FactKind::MultiDiscRequiredMissing,
    FactKind::PlayingLibraryPlan,
    FactKind::DuplicatesExact,
    FactKind::UndoAvailable,
    FactKind::UndoRefused,
    FactKind::RommSnapshotUnavailable,
    FactKind::OptionalSourceUnavailable,
    FactKind::LibraryLoadedEmpty,
    FactKind::FormatOperationUnsupported,
    FactKind::NoCapabilityView,
    FactKind::DestinationReadOnly,
    FactKind::DestinationChooserAvailable,
    FactKind::RepairCompletedVerified,
    FactKind::RepairCompletedVerificationPending,
    FactKind::VerificationUnavailable,
    FactKind::SourceChangedSincePreview,
    FactKind::QueuedWork,
];

/// Exhaustive: adding a `FactKind` without classifying it does not compile.
pub(super) fn coverage_of(kind: FactKind) -> FactCoverage {
    use Coverage::*;
    let (coverage, source, adapter) = match kind {
        FactKind::LegacyLibraryEmpty
        | FactKind::LegacyLibraryHasGames
        | FactKind::LegacySourceUnavailable
        | FactKind::LegacySourceUnavailableWithScan
        | FactKind::LegacySourceLastScan
        | FactKind::LegacyLaunchIdentityUnverified
        | FactKind::LegacyLaunchIdentityVerified
        | FactKind::LegacyProblemBlocker
        | FactKind::LegacyProblemsChecking
        | FactKind::LegacyProblemsClear
        | FactKind::LegacyProblemsFindings
        | FactKind::LegacyTapeStructure
        | FactKind::LegacyDatIdentity
        | FactKind::LegacyJobsIdle
        | FactKind::LegacyJobsRunning => (
            AvailableNow,
            "Page-supplied legacy evidence (GuidanceEvidence optional fields)",
            "GuidanceEvidence::facts",
        ),
        FactKind::FreshEnvironment => (
            NeedsNewProductState,
            "no first-run/fresh-environment state is public",
            "",
        ),
        FactKind::HomeFirstUseTipEligible => (
            NeedsNewProductState,
            "needs tip-exposure/preference state (Phase 2 preference migration)",
            "",
        ),
        FactKind::SourcesNoneConfigured => (
            SimpleProjection,
            "configured source list the page holds",
            "LibraryGuidanceEvidence",
        ),
        FactKind::SourceUnavailable => (
            SimpleProjection,
            "source availability the page already projects",
            "SourceGuidanceEvidence",
        ),
        FactKind::ScanRunning => (
            SimpleProjection,
            "scan job state",
            "LibraryGuidanceEvidence",
        ),
        FactKind::ScanPartialFailure => (
            SimpleProjection,
            "finished scan result (unreadable folder count)",
            "LibraryGuidanceEvidence",
        ),
        FactKind::DatNoneAvailable => (
            SimpleProjection,
            "DAT inventory count",
            "LibraryGuidanceEvidence",
        ),
        FactKind::IdentityUnknown => (
            AvailableNow,
            "launch::CanonicalIdentityStatus::Unknown",
            "GameGuidanceEvidence",
        ),
        FactKind::OperationNeedsStrongerIdentity => (
            SimpleProjection,
            "operation capability flag supplied by the caller",
            "GameGuidanceEvidence",
        ),
        FactKind::IdentityCandidateOnly => (
            SimpleProjection,
            "identity layer candidate title; only beside Unknown",
            "GameGuidanceEvidence",
        ),
        FactKind::IdentityConflicting => (
            AvailableNow,
            "launch::CanonicalIdentityStatus::Conflicting",
            "GameGuidanceEvidence",
        ),
        FactKind::ProblemsActionable => (
            SimpleProjection,
            "Problems inbox count; typed fact not yet adapted",
            "",
        ),
        FactKind::NoFirstFindingTarget => (
            Deferred,
            "no public projection of a first-finding target",
            "",
        ),
        FactKind::MameMissingMembers => (
            Deferred,
            "MAME collection health lane is out of scope; no safe public projection used",
            "",
        ),
        FactKind::MameParentDependencyMissing => (
            Deferred,
            "MAME collection health lane is out of scope; no safe public projection used",
            "",
        ),
        FactKind::MameBadDumpReference => (
            Deferred,
            "MAME collection health lane is out of scope; no safe public projection used",
            "",
        ),
        FactKind::MameNoDumpReference => (
            Deferred,
            "MAME collection health lane is out of scope; no safe public projection used",
            "",
        ),
        FactKind::ArtworkCoverMissing => (
            AvailableNow,
            "gamer_artwork::CoverAnswer::None(NoArtwork)",
            "ArtworkGuidanceEvidence",
        ),
        FactKind::ArtworkRefreshAvailable => (
            Deferred,
            "refresh capability is page-owned; not projected",
            "",
        ),
        FactKind::IdentityPreventsArtworkMatch => (
            Deferred,
            "NoCover::NoRommIdentity is RomM identity, not canonical identity; not reinterpreted",
            "",
        ),
        FactKind::ArtworkCachedUsable => (
            AvailableNow,
            "gamer_artwork::CoverAnswer::Ready/Unchanged",
            "ArtworkGuidanceEvidence",
        ),
        FactKind::ArtworkCacheStale => (
            NeedsNewProductState,
            "resolver exposes no cache-staleness state",
            "",
        ),
        FactKind::ArtworkSourceUnavailable => (
            AvailableNow,
            "gamer_artwork::CoverAnswer::None(Unavailable)",
            "ArtworkGuidanceEvidence",
        ),
        FactKind::ArtworkAlternatives => (
            NeedsNewProductState,
            "no alternative-artwork inventory is exposed",
            "",
        ),
        FactKind::EmulatorMultipleInstallations => (
            SimpleProjection,
            "emulator environment report install count",
            "LaunchGuidanceEvidence",
        ),
        FactKind::FirmwareMissing => (
            AvailableNow,
            "launch::FirmwareReadiness::Missing / readiness summary",
            "LaunchGuidanceEvidence",
        ),
        FactKind::BiosFolderChooserAvailable => (
            SimpleProjection,
            "page-owned capability flag",
            "LaunchGuidanceEvidence",
        ),
        FactKind::LaunchNoCompatibleEmulator => (
            AvailableNow,
            "launch_readiness_summary NeedsEmulator (current)",
            "LaunchGuidanceEvidence",
        ),
        FactKind::LaunchReady => (
            AvailableNow,
            "launch_readiness_summary Ready (current, no warnings)",
            "LaunchGuidanceEvidence",
        ),
        FactKind::LaunchWarningsUnacknowledged => (
            NeedsNewProductState,
            "no warning-acknowledgement state exists",
            "",
        ),
        FactKind::LaunchFailed => (
            SimpleProjection,
            "launch outcome with operation id and plain reason",
            "OperationGuidanceEvidence",
        ),
        FactKind::NoGameSelected => (
            SimpleProjection,
            "page selection state",
            "GameGuidanceEvidence",
        ),
        FactKind::CheatsNoneKnown => (
            SimpleProjection,
            "cheat lookup count",
            "CheatGuidanceEvidence",
        ),
        FactKind::PatchBaseMismatch => (
            Deferred,
            "patch backend lane; no public projection used",
            "",
        ),
        FactKind::PatchRevisionMismatchKnown => (
            Deferred,
            "patch backend lane; no public projection used",
            "",
        ),
        FactKind::ConversionPreviewAvailable => (
            SimpleProjection,
            "converter page preview state; not yet adapted",
            "",
        ),
        FactKind::ConversionPreservationUnknown => (
            NeedsNewProductState,
            "no conversion preservation verdict exists",
            "",
        ),
        FactKind::MultiDiscRequiredMissing => (
            Deferred,
            "multi-disc topology not projected publicly for guidance",
            "",
        ),
        FactKind::PlayingLibraryPlan => (
            SimpleProjection,
            "Playing Library plan counts; not yet adapted",
            "",
        ),
        FactKind::DuplicatesExact => (
            SimpleProjection,
            "exact-duplicate review result; not yet adapted",
            "",
        ),
        FactKind::UndoAvailable => (SimpleProjection, "History undo state; not yet adapted", ""),
        FactKind::UndoRefused => (
            SimpleProjection,
            "History undo refusal reason; not yet adapted",
            "",
        ),
        FactKind::RommSnapshotUnavailable => {
            (SimpleProjection, "RomM snapshot state; not yet adapted", "")
        }
        FactKind::OptionalSourceUnavailable => (
            SimpleProjection,
            "optional provider reachability",
            "SourceGuidanceEvidence",
        ),
        FactKind::LibraryLoadedEmpty => (
            SimpleProjection,
            "completed library load count",
            "LibraryGuidanceEvidence",
        ),
        FactKind::FormatOperationUnsupported => (
            SimpleProjection,
            "canonical capability refusal (format, operation)",
            "OperationGuidanceEvidence",
        ),
        FactKind::NoCapabilityView => (NeedsNewProductState, "no capability-view state exists", ""),
        FactKind::DestinationReadOnly => (
            SimpleProjection,
            "destination writability probe result supplied by caller",
            "OperationGuidanceEvidence",
        ),
        FactKind::DestinationChooserAvailable => (
            SimpleProjection,
            "page-owned capability flag; not yet adapted",
            "",
        ),
        FactKind::RepairCompletedVerified => (
            SimpleProjection,
            "repair result with its own verification state",
            "OperationGuidanceEvidence",
        ),
        FactKind::RepairCompletedVerificationPending => (
            SimpleProjection,
            "repair result with its own verification state",
            "OperationGuidanceEvidence",
        ),
        FactKind::VerificationUnavailable => (
            SimpleProjection,
            "repair result verification unavailable",
            "OperationGuidanceEvidence",
        ),
        FactKind::SourceChangedSincePreview => (
            SimpleProjection,
            "preview-vs-source comparison supplied by caller",
            "SourceGuidanceEvidence",
        ),
        FactKind::QueuedWork => (
            SimpleProjection,
            "job queue counts",
            "OperationGuidanceEvidence",
        ),
    };
    FactCoverage {
        kind,
        coverage,
        source,
        adapter,
    }
}

#[derive(Debug)]
pub(super) struct ScriptCoverage {
    pub(super) id: &'static str,
    pub(super) coverage: Coverage,
    /// Required facts no adapter emits yet.
    pub(super) unadapted: Vec<FactKind>,
}

#[derive(Debug)]
pub(super) struct EvidenceReport {
    pub(super) scripts: Vec<ScriptCoverage>,
    pub(super) facts_without_consumer: Vec<FactKind>,
}

impl EvidenceReport {
    pub(super) fn count(&self, coverage: Coverage) -> usize {
        self.scripts
            .iter()
            .filter(|s| s.coverage == coverage)
            .count()
    }

    /// Scripts every required fact of which has an adapter: real evidence today.
    pub(super) fn receivable_now(&self) -> Vec<&'static str> {
        self.scripts
            .iter()
            .filter(|s| s.unadapted.is_empty())
            .map(|s| s.id)
            .collect()
    }

    pub(super) fn without_adapter(&self) -> Vec<&'static str> {
        self.scripts
            .iter()
            .filter(|s| !s.unadapted.is_empty())
            .map(|s| s.id)
            .collect()
    }
}

pub(super) fn report(scripts: &'static [GuidanceScript]) -> EvidenceReport {
    let mut consumed = BTreeSet::new();
    let mut rows = Vec::new();
    for script in scripts {
        let mut coverage = Coverage::AvailableNow;
        let mut unadapted = Vec::new();
        for kind in script.requires {
            consumed.insert(*kind);
            let row = coverage_of(*kind);
            coverage = coverage.max(row.coverage);
            if row.adapter.is_empty() {
                unadapted.push(*kind);
            }
        }
        // A script with no required fact (legacy page-level guidance) needs none.
        rows.push(ScriptCoverage {
            id: script.id,
            coverage,
            unadapted,
        });
    }
    EvidenceReport {
        scripts: rows,
        facts_without_consumer: ALL_KINDS
            .iter()
            .copied()
            .filter(|kind| !consumed.contains(kind))
            .collect(),
    }
}

impl fmt::Display for EvidenceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Mr Wiz evidence coverage ({} scripts)",
            self.scripts.len()
        )?;
        for c in [
            Coverage::AvailableNow,
            Coverage::SimpleProjection,
            Coverage::NeedsNewProductState,
            Coverage::Deferred,
        ] {
            writeln!(f, "  {c:?}: {}", self.count(c))?;
        }
        writeln!(f, "  receivable now: {}", self.receivable_now().len())?;
        writeln!(
            f,
            "  facts with no script consumer: {:?}",
            self.facts_without_consumer
        )?;
        writeln!(f, "  script | status | unadapted facts")?;
        for s in &self.scripts {
            writeln!(f, "    {} | {:?} | {:?}", s.id, s.coverage, s.unadapted)?;
        }
        writeln!(f, "  fact | status | source | adapter")?;
        for kind in ALL_KINDS {
            let r = coverage_of(*kind);
            writeln!(
                f,
                "    {:?} | {:?} | {} | {}",
                r.kind, r.coverage, r.source, r.adapter
            )?;
        }
        Ok(())
    }
}
