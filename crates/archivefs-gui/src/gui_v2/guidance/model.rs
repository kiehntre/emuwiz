//! The canonical, typed Mr Wiz guidance model. Pure data: no egui, no I/O.
//!
//! Mr Wiz explains product truth; it never determines it. Everything here is a
//! description of facts the *caller already holds* (a page snapshot, a readiness
//! result, a job registry), plus the vocabulary the deterministic selector and
//! the authored catalogue share. Nothing in this module queries the database, the
//! filesystem, the network, an emulator or a provider.
//!
//! Terminology follows `docs/design/MR_WIZ_GUIDANCE_AND_AI_ASSISTANT_V1.md`:
//! the six message categories are the existing ones; the 29 subject areas are
//! *topics*, not extra severities; the levels are Quick, Explain and Technical
//! (with Minimal as the shortest equivalent of Quick for experienced users).

// Phase 1 is backend-only: GUI wiring is deliberately deferred, so most of this
// vocabulary is exercised by the selector and the tests rather than by a page.
#![allow(dead_code)]

/// The six message categories (unchanged from the pre-engine implementation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GuidanceCategory {
    Tip,
    Explain,
    WhyBlocked,
    Success,
    Warning,
    EmptyState,
}

impl GuidanceCategory {
    /// Tie-break order within one priority band and scope: a refusal reason is
    /// never hidden by a milder message of the same weight.
    pub(crate) fn precedence(self) -> u8 {
        match self {
            Self::WhyBlocked => 6,
            Self::Warning => 5,
            Self::EmptyState => 4,
            Self::Explain => 3,
            Self::Tip => 2,
            Self::Success => 1,
        }
    }
}

/// How much detail a message carries. One axis of verbosity, shortest first.
///
/// `Minimal` is the experienced-user equivalent of `Quick`; `Explain` and
/// `Technical` are the Level 2 and Level 3 disclosures. A script may omit any
/// level but `Quick`; asking for an omitted level resolves to `Quick` and the
/// selection says so rather than inventing text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum GuidanceLevel {
    Minimal,
    Quick,
    Explain,
    Technical,
}

/// The 29 subject areas of the design's coverage table plus the Activity
/// supplement. A topic says what a script is *about*; severity stays in
/// [`GuidanceCategory`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum GuidanceTopic {
    FirstRun,
    Home,
    Sources,
    Scanning,
    IdentityDat,
    Problems,
    Mame,
    ArtworkMetadata,
    EmulatorSetup,
    BiosFirmware,
    LaunchReadiness,
    FailedLaunch,
    Cheats,
    ModsPatches,
    Conversion,
    MultiDisc,
    PlayingLibrary,
    Duplicates,
    HistoryUndo,
    Romm,
    OfflineMode,
    EmptyLibrary,
    UnknownGame,
    ConflictingEvidence,
    UnsupportedFormat,
    ReadOnlySafety,
    SuccessStates,
    Warnings,
    RecoveryFromFailure,
    /// The design's "Supplement: Activity" row.
    Activity,
}

impl GuidanceTopic {
    /// The 29 numbered rows of the design's coverage table, in order.
    pub(crate) const DESIGN: [Self; 29] = [
        Self::FirstRun,
        Self::Home,
        Self::Sources,
        Self::Scanning,
        Self::IdentityDat,
        Self::Problems,
        Self::Mame,
        Self::ArtworkMetadata,
        Self::EmulatorSetup,
        Self::BiosFirmware,
        Self::LaunchReadiness,
        Self::FailedLaunch,
        Self::Cheats,
        Self::ModsPatches,
        Self::Conversion,
        Self::MultiDisc,
        Self::PlayingLibrary,
        Self::Duplicates,
        Self::HistoryUndo,
        Self::Romm,
        Self::OfflineMode,
        Self::EmptyLibrary,
        Self::UnknownGame,
        Self::ConflictingEvidence,
        Self::UnsupportedFormat,
        Self::ReadOnlySafety,
        Self::SuccessStates,
        Self::Warnings,
        Self::RecoveryFromFailure,
    ];
}

/// What a message is about, for tie-breaking: a message about the operation in
/// front of the user is more specific than one about the whole collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum GuidanceScope {
    Collection,
    Source,
    Game,
    Operation,
}

/// The semantic mascot states (design section 9). There is no angry state, and
/// no state is a promise of correctness: it is a supplementary cue, never the only
/// signal. No artwork is selected here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MascotState {
    /// Quiet presence, no urgent claim.
    Neutral,
    /// Tip, explanation, empty state, or a routine blocker with a next step.
    Helpful,
    /// Evidence is loading or uncertain and needs review.
    Thinking,
    /// A meaningful preservation risk or conflicting evidence affecting a decision.
    Warning,
    /// A current failure that needs recovery. Calm, not exaggerated.
    Concerned,
    /// A confirmed result.
    Success,
}

/// The page or task a message may appear on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GuidancePage {
    Home,
    Sources,
    Games,
    Launch,
    ProblemsRepair,
    Organisation,
    CheatsMods,
    Museum,
    TapeInspector,
    ArchiveInspector,
    DatManagement,
    BiosFirmware,
    EmulatorSetup,
    Setup,
    CheckGames,
    Activity,
    History,
    Saves,
    Converter,
    Artwork,
    Romm,
    Advanced,
    Settings,
    /// Added with the design scripts (29); no GUI route maps to it yet.
    Duplicates,
}

/// A next step Mr Wiz can *offer*. Typed identifiers only: the engine never
/// executes or navigates. A later GUI adapter decides whether and how each is
/// rendered, and re-validates the selection and evidence revision on click.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GuidanceAction {
    ChooseGameFolders,
    BrowseGames,
    AddGameFolder,
    ReviewGameFolder,
    ReviewGameFolders,
    ViewScanProgress,
    ReviewUnavailableFolders,
    ManageDats,
    ReviewGameEvidence,
    ReviewPossibleMatch,
    CompareIdentityEvidence,
    ReviewFirstFinding,
    ReviewProblems,
    ReviewInMame,
    ReviewParentDependency,
    ReviewReferenceEvidence,
    ReviewReferenceLimitation,
    ReviewArtworkSources,
    RefreshArtwork,
    ViewArtworkDetails,
    ViewArtworkSources,
    ReviewInstallations,
    ReviewBiosFolder,
    ChooseBiosFolder,
    ChooseEmulator,
    ReviewLaunch,
    ReviewLaunchDetails,
    ChooseGame,
    BackToGameDetails,
    ReviewExpectedBase,
    PreviewConversion,
    ReviewConversionWarning,
    ReviewDiscSet,
    ReviewPlayingLibraryPlan,
    ReviewDuplicateGroup,
    ReviewUndo,
    ReviewRecoveryDetails,
    ReviewRommSetup,
    ReviewSourceStatus,
    ReviewSupportedOptions,
    ReviewDestination,
    ChooseOutputFolder,
    ViewRepairInHistory,
    VerifyResult,
    ReviewResult,
    PreviewAgain,
    ViewActivity,
}

impl GuidanceAction {
    /// The authored button label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ChooseGameFolders => "Choose game folders",
            Self::BrowseGames => "Browse games",
            Self::AddGameFolder => "Add game folder",
            Self::ReviewGameFolder => "Review game folder",
            Self::ReviewGameFolders => "Review game folders",
            Self::ViewScanProgress => "View scan progress",
            Self::ReviewUnavailableFolders => "Review unavailable folders",
            Self::ManageDats => "Manage DATs",
            Self::ReviewGameEvidence => "Review game evidence",
            Self::ReviewPossibleMatch => "Review possible match",
            Self::CompareIdentityEvidence => "Compare identity evidence",
            Self::ReviewFirstFinding => "Review first finding",
            Self::ReviewProblems => "Review problems",
            Self::ReviewInMame => "Review in MAME",
            Self::ReviewParentDependency => "Review parent dependency",
            Self::ReviewReferenceEvidence => "Review reference evidence",
            Self::ReviewReferenceLimitation => "Review reference limitation",
            Self::ReviewArtworkSources => "Review artwork sources",
            Self::RefreshArtwork => "Refresh artwork",
            Self::ViewArtworkDetails => "View artwork details",
            Self::ViewArtworkSources => "View artwork sources",
            Self::ReviewInstallations => "Review installations",
            Self::ReviewBiosFolder => "Review BIOS folder",
            Self::ChooseBiosFolder => "Choose BIOS folder",
            Self::ChooseEmulator => "Choose emulator",
            Self::ReviewLaunch => "Review launch",
            Self::ReviewLaunchDetails => "Review launch details",
            Self::ChooseGame => "Choose a game",
            Self::BackToGameDetails => "Back to Game Details",
            Self::ReviewExpectedBase => "Review expected base",
            Self::PreviewConversion => "Preview conversion",
            Self::ReviewConversionWarning => "Review conversion warning",
            Self::ReviewDiscSet => "Review disc set",
            Self::ReviewPlayingLibraryPlan => "Review Playing Library plan",
            Self::ReviewDuplicateGroup => "Review duplicate group",
            Self::ReviewUndo => "Review undo",
            Self::ReviewRecoveryDetails => "Review recovery details",
            Self::ReviewRommSetup => "Review RomM setup",
            Self::ReviewSourceStatus => "Review source status",
            Self::ReviewSupportedOptions => "Review supported options",
            Self::ReviewDestination => "Review destination",
            Self::ChooseOutputFolder => "Choose output folder",
            Self::ViewRepairInHistory => "View repair in History",
            Self::VerifyResult => "Verify result",
            Self::ReviewResult => "Review result",
            Self::PreviewAgain => "Preview again",
            Self::ViewActivity => "View Activity",
        }
    }

    /// The page this action navigates to, when it is *pure navigation* to one
    /// existing page. Used to drop an offer that would take the person to where
    /// they already are (the design's "suppress this duplicate action when already
    /// there"). Controls that live on a page, such as adding a folder, are not
    /// navigation and are never dropped.
    pub(crate) fn lands_on(self) -> Option<GuidancePage> {
        match self {
            Self::ViewActivity | Self::ViewScanProgress => Some(GuidancePage::Activity),
            Self::BrowseGames | Self::ChooseGame => Some(GuidancePage::Games),
            Self::ManageDats => Some(GuidancePage::DatManagement),
            _ => None,
        }
    }
}

// --- Typed facts ---------------------------------------------------------------------

/// A value a message template may use. Never a path or a secret by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Param {
    Count(u64),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParamKind {
    Count,
    Text,
}

/// Named template values supplied by facts.
pub(crate) type Params = Vec<(&'static str, Param)>;

/// What kind of fact this is, without its payload. Selectors speak in kinds, so
/// the catalogue can be analysed (reachability, duplicate selectors) without
/// running anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum FactKind {
    // Projection of the pre-engine optional evidence fields. These keep the
    // existing page guidance working unchanged until a page-owned adapter
    // supplies the design facts below instead.
    LegacyLibraryEmpty,
    LegacyLibraryHasGames,
    LegacySourceUnavailable,
    LegacySourceUnavailableWithScan,
    LegacySourceLastScan,
    LegacyLaunchIdentityUnverified,
    LegacyLaunchIdentityVerified,
    LegacyProblemBlocker,
    LegacyProblemsChecking,
    LegacyProblemsClear,
    LegacyProblemsFindings,
    LegacyTapeStructure,
    LegacyDatIdentity,
    LegacyJobsIdle,
    LegacyJobsRunning,
    // Facts justified by the 42 designed scripts.
    FreshEnvironment,
    HomeFirstUseTipEligible,
    SourcesNoneConfigured,
    SourceUnavailable,
    ScanRunning,
    ScanPartialFailure,
    DatNoneAvailable,
    IdentityUnknown,
    OperationNeedsStrongerIdentity,
    IdentityCandidateOnly,
    IdentityConflicting,
    ProblemsActionable,
    NoFirstFindingTarget,
    MameMissingMembers,
    MameParentDependencyMissing,
    MameBadDumpReference,
    MameNoDumpReference,
    ArtworkCoverMissing,
    ArtworkRefreshAvailable,
    IdentityPreventsArtworkMatch,
    ArtworkCachedUsable,
    ArtworkCacheStale,
    ArtworkSourceUnavailable,
    ArtworkAlternatives,
    EmulatorMultipleInstallations,
    FirmwareMissing,
    BiosFolderChooserAvailable,
    LaunchNoCompatibleEmulator,
    LaunchReady,
    LaunchWarningsUnacknowledged,
    LaunchFailed,
    NoGameSelected,
    CheatsNoneKnown,
    PatchBaseMismatch,
    PatchRevisionMismatchKnown,
    ConversionPreviewAvailable,
    ConversionPreservationUnknown,
    MultiDiscRequiredMissing,
    PlayingLibraryPlan,
    DuplicatesExact,
    UndoAvailable,
    UndoRefused,
    RommSnapshotUnavailable,
    OptionalSourceUnavailable,
    LibraryLoadedEmpty,
    FormatOperationUnsupported,
    NoCapabilityView,
    DestinationReadOnly,
    DestinationChooserAvailable,
    RepairCompletedVerified,
    RepairCompletedVerificationPending,
    VerificationUnavailable,
    SourceChangedSincePreview,
    QueuedWork,
}

use ParamKind::{Count, Text};

impl FactKind {
    /// The template parameters a fact of this kind supplies.
    pub(crate) fn params(self) -> &'static [(&'static str, ParamKind)] {
        match self {
            Self::LegacySourceUnavailableWithScan | Self::LegacySourceLastScan => &[("scan", Text)],
            Self::LegacyProblemBlocker => &[("blocker", Text)],
            Self::LegacyProblemsFindings => &[("count", Count), ("attention_clause", Text)],
            Self::LegacyTapeStructure => &[("format", Text), ("blocks", Count)],
            Self::LegacyDatIdentity => &[("name", Text)],
            Self::LegacyJobsRunning => &[("count", Count)],
            Self::ScanPartialFailure => &[("folder_count", Count)],
            Self::IdentityCandidateOnly => &[("title", Text)],
            Self::ProblemsActionable => &[("finding_count", Count)],
            Self::MameMissingMembers => &[("missing_count", Count)],
            Self::MameParentDependencyMissing => &[("parent", Text)],
            Self::ArtworkAlternatives => &[("provider", Text), ("alternative_count", Count)],
            Self::EmulatorMultipleInstallations => &[
                ("emulator", Text),
                ("count", Count),
                ("count_word", Text),
                ("count_lower", Text),
            ],
            Self::LaunchFailed => &[("plain_failure_reason", Text)],
            Self::ConversionPreviewAvailable => &[("source_format", Text), ("target_format", Text)],
            Self::PlayingLibraryPlan => &[("set_count", Count), ("link_count", Count)],
            Self::UndoRefused => &[("plain_undo_reason", Text)],
            Self::RommSnapshotUnavailable => &[("plain_source_reason", Text)],
            Self::FormatOperationUnsupported => &[("format", Text), ("operation", Text)],
            Self::QueuedWork => &[("queued_count", Count), ("running_count", Count)],
            _ => &[],
        }
    }

    /// A representative fact, for catalogue analysis only.
    pub(crate) fn sample(self) -> GuidanceFact {
        use GuidanceFact as F;
        let text = || "sample".to_string();
        match self {
            Self::LegacyLibraryEmpty => F::LegacyLibraryEmpty,
            Self::LegacyLibraryHasGames => F::LegacyLibraryHasGames,
            Self::LegacySourceUnavailable => F::LegacySourceUnavailable,
            Self::LegacySourceUnavailableWithScan => {
                F::LegacySourceUnavailableWithScan { scan: text() }
            }
            Self::LegacySourceLastScan => F::LegacySourceLastScan { scan: text() },
            Self::LegacyLaunchIdentityUnverified => F::LegacyLaunchIdentityUnverified,
            Self::LegacyLaunchIdentityVerified => F::LegacyLaunchIdentityVerified,
            Self::LegacyProblemBlocker => F::LegacyProblemBlocker { blocker: text() },
            Self::LegacyProblemsChecking => F::LegacyProblemsChecking,
            Self::LegacyProblemsClear => F::LegacyProblemsClear,
            Self::LegacyProblemsFindings => F::LegacyProblemsFindings {
                count: 2,
                attention: Some(1),
            },
            Self::LegacyTapeStructure => F::LegacyTapeStructure {
                format: text(),
                blocks: 2,
            },
            Self::LegacyDatIdentity => F::LegacyDatIdentity { name: text() },
            Self::LegacyJobsIdle => F::LegacyJobsIdle,
            Self::LegacyJobsRunning => F::LegacyJobsRunning { count: 2 },
            Self::FreshEnvironment => F::FreshEnvironment,
            Self::HomeFirstUseTipEligible => F::HomeFirstUseTipEligible,
            Self::SourcesNoneConfigured => F::SourcesNoneConfigured,
            Self::SourceUnavailable => F::SourceUnavailable,
            Self::ScanRunning => F::ScanRunning,
            Self::ScanPartialFailure => F::ScanPartialFailure { folder_count: 2 },
            Self::DatNoneAvailable => F::DatNoneAvailable,
            Self::IdentityUnknown => F::IdentityUnknown,
            Self::OperationNeedsStrongerIdentity => F::OperationNeedsStrongerIdentity,
            Self::IdentityCandidateOnly => F::IdentityCandidateOnly { title: text() },
            Self::IdentityConflicting => F::IdentityConflicting,
            Self::ProblemsActionable => F::ProblemsActionable { finding_count: 2 },
            Self::NoFirstFindingTarget => F::NoFirstFindingTarget,
            Self::MameMissingMembers => F::MameMissingMembers { missing_count: 2 },
            Self::MameParentDependencyMissing => F::MameParentDependencyMissing { parent: text() },
            Self::MameBadDumpReference => F::MameBadDumpReference,
            Self::MameNoDumpReference => F::MameNoDumpReference,
            Self::ArtworkCoverMissing => F::ArtworkCoverMissing,
            Self::ArtworkRefreshAvailable => F::ArtworkRefreshAvailable,
            Self::IdentityPreventsArtworkMatch => F::IdentityPreventsArtworkMatch,
            Self::ArtworkCachedUsable => F::ArtworkCachedUsable,
            Self::ArtworkCacheStale => F::ArtworkCacheStale,
            Self::ArtworkSourceUnavailable => F::ArtworkSourceUnavailable,
            Self::ArtworkAlternatives => F::ArtworkAlternatives {
                provider: text(),
                alternative_count: 2,
            },
            Self::EmulatorMultipleInstallations => F::EmulatorMultipleInstallations {
                emulator: "Dolphin".into(),
                count: 2,
            },
            Self::FirmwareMissing => F::FirmwareMissing,
            Self::BiosFolderChooserAvailable => F::BiosFolderChooserAvailable,
            Self::LaunchNoCompatibleEmulator => F::LaunchNoCompatibleEmulator,
            Self::LaunchReady => F::LaunchReady,
            Self::LaunchWarningsUnacknowledged => F::LaunchWarningsUnacknowledged,
            Self::LaunchFailed => F::LaunchFailed {
                plain_failure_reason: text(),
                operation_id: 1,
            },
            Self::NoGameSelected => F::NoGameSelected,
            Self::CheatsNoneKnown => F::CheatsNoneKnown,
            Self::PatchBaseMismatch => F::PatchBaseMismatch,
            Self::PatchRevisionMismatchKnown => F::PatchRevisionMismatchKnown,
            Self::ConversionPreviewAvailable => F::ConversionPreviewAvailable {
                source_format: text(),
                target_format: text(),
            },
            Self::ConversionPreservationUnknown => F::ConversionPreservationUnknown,
            Self::MultiDiscRequiredMissing => F::MultiDiscRequiredMissing,
            Self::PlayingLibraryPlan => F::PlayingLibraryPlan {
                set_count: 2,
                link_count: 2,
            },
            Self::DuplicatesExact => F::DuplicatesExact,
            Self::UndoAvailable => F::UndoAvailable,
            Self::UndoRefused => F::UndoRefused {
                plain_undo_reason: text(),
            },
            Self::RommSnapshotUnavailable => F::RommSnapshotUnavailable {
                plain_source_reason: text(),
            },
            Self::OptionalSourceUnavailable => F::OptionalSourceUnavailable,
            Self::LibraryLoadedEmpty => F::LibraryLoadedEmpty,
            Self::FormatOperationUnsupported => F::FormatOperationUnsupported {
                format: text(),
                operation: text(),
            },
            Self::NoCapabilityView => F::NoCapabilityView,
            Self::DestinationReadOnly => F::DestinationReadOnly,
            Self::DestinationChooserAvailable => F::DestinationChooserAvailable,
            Self::RepairCompletedVerified => F::RepairCompletedVerified { operation_id: 1 },
            Self::RepairCompletedVerificationPending => {
                F::RepairCompletedVerificationPending { operation_id: 1 }
            }
            Self::VerificationUnavailable => F::VerificationUnavailable,
            Self::SourceChangedSincePreview => F::SourceChangedSincePreview,
            Self::QueuedWork => F::QueuedWork {
                queued_count: 2,
                running_count: 1,
            },
        }
    }
}

/// One fact the caller already holds. Unknown, loading and failed are *absence*:
/// a caller that has not finished a check supplies no fact, and the engine says
/// nothing rather than treating "not loaded" as "nothing found".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GuidanceFact {
    // Pre-engine projection (see `FactKind`).
    LegacyLibraryEmpty,
    LegacyLibraryHasGames,
    LegacySourceUnavailable,
    LegacySourceUnavailableWithScan {
        scan: String,
    },
    LegacySourceLastScan {
        scan: String,
    },
    LegacyLaunchIdentityUnverified,
    LegacyLaunchIdentityVerified,
    LegacyProblemBlocker {
        blocker: String,
    },
    LegacyProblemsChecking,
    LegacyProblemsClear,
    LegacyProblemsFindings {
        count: u64,
        attention: Option<u64>,
    },
    LegacyTapeStructure {
        format: String,
        blocks: u64,
    },
    LegacyDatIdentity {
        name: String,
    },
    LegacyJobsIdle,
    LegacyJobsRunning {
        count: u64,
    },
    // Design facts.
    FreshEnvironment,
    HomeFirstUseTipEligible,
    SourcesNoneConfigured,
    SourceUnavailable,
    ScanRunning,
    ScanPartialFailure {
        folder_count: u64,
    },
    DatNoneAvailable,
    IdentityUnknown,
    OperationNeedsStrongerIdentity,
    IdentityCandidateOnly {
        title: String,
    },
    IdentityConflicting,
    ProblemsActionable {
        finding_count: u64,
    },
    NoFirstFindingTarget,
    MameMissingMembers {
        missing_count: u64,
    },
    MameParentDependencyMissing {
        parent: String,
    },
    MameBadDumpReference,
    MameNoDumpReference,
    ArtworkCoverMissing,
    ArtworkRefreshAvailable,
    IdentityPreventsArtworkMatch,
    ArtworkCachedUsable,
    ArtworkCacheStale,
    ArtworkSourceUnavailable,
    ArtworkAlternatives {
        provider: String,
        alternative_count: u64,
    },
    EmulatorMultipleInstallations {
        emulator: String,
        count: u64,
    },
    FirmwareMissing,
    BiosFolderChooserAvailable,
    LaunchNoCompatibleEmulator,
    LaunchReady,
    LaunchWarningsUnacknowledged,
    LaunchFailed {
        plain_failure_reason: String,
        operation_id: u64,
    },
    NoGameSelected,
    CheatsNoneKnown,
    PatchBaseMismatch,
    PatchRevisionMismatchKnown,
    ConversionPreviewAvailable {
        source_format: String,
        target_format: String,
    },
    ConversionPreservationUnknown,
    MultiDiscRequiredMissing,
    PlayingLibraryPlan {
        set_count: u64,
        link_count: u64,
    },
    DuplicatesExact,
    UndoAvailable,
    UndoRefused {
        plain_undo_reason: String,
    },
    RommSnapshotUnavailable {
        plain_source_reason: String,
    },
    OptionalSourceUnavailable,
    LibraryLoadedEmpty,
    FormatOperationUnsupported {
        format: String,
        operation: String,
    },
    NoCapabilityView,
    DestinationReadOnly,
    DestinationChooserAvailable,
    RepairCompletedVerified {
        operation_id: u64,
    },
    RepairCompletedVerificationPending {
        operation_id: u64,
    },
    VerificationUnavailable,
    SourceChangedSincePreview,
    QueuedWork {
        queued_count: u64,
        running_count: u64,
    },
}

/// Longest text value a template will interpolate; longer values are cut.
pub(crate) const MAX_PARAM_CHARS: usize = 120;

fn number_word(count: u64) -> String {
    const WORDS: [&str; 11] = [
        "Zero", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
    ];
    WORDS
        .get(count as usize)
        .map_or_else(|| count.to_string(), |word| (*word).to_string())
}

impl GuidanceFact {
    pub(crate) fn kind(&self) -> FactKind {
        use FactKind as K;
        use GuidanceFact as F;
        match self {
            F::LegacyLibraryEmpty => K::LegacyLibraryEmpty,
            F::LegacyLibraryHasGames => K::LegacyLibraryHasGames,
            F::LegacySourceUnavailable => K::LegacySourceUnavailable,
            F::LegacySourceUnavailableWithScan { .. } => K::LegacySourceUnavailableWithScan,
            F::LegacySourceLastScan { .. } => K::LegacySourceLastScan,
            F::LegacyLaunchIdentityUnverified => K::LegacyLaunchIdentityUnverified,
            F::LegacyLaunchIdentityVerified => K::LegacyLaunchIdentityVerified,
            F::LegacyProblemBlocker { .. } => K::LegacyProblemBlocker,
            F::LegacyProblemsChecking => K::LegacyProblemsChecking,
            F::LegacyProblemsClear => K::LegacyProblemsClear,
            F::LegacyProblemsFindings { .. } => K::LegacyProblemsFindings,
            F::LegacyTapeStructure { .. } => K::LegacyTapeStructure,
            F::LegacyDatIdentity { .. } => K::LegacyDatIdentity,
            F::LegacyJobsIdle => K::LegacyJobsIdle,
            F::LegacyJobsRunning { .. } => K::LegacyJobsRunning,
            F::FreshEnvironment => K::FreshEnvironment,
            F::HomeFirstUseTipEligible => K::HomeFirstUseTipEligible,
            F::SourcesNoneConfigured => K::SourcesNoneConfigured,
            F::SourceUnavailable => K::SourceUnavailable,
            F::ScanRunning => K::ScanRunning,
            F::ScanPartialFailure { .. } => K::ScanPartialFailure,
            F::DatNoneAvailable => K::DatNoneAvailable,
            F::IdentityUnknown => K::IdentityUnknown,
            F::OperationNeedsStrongerIdentity => K::OperationNeedsStrongerIdentity,
            F::IdentityCandidateOnly { .. } => K::IdentityCandidateOnly,
            F::IdentityConflicting => K::IdentityConflicting,
            F::ProblemsActionable { .. } => K::ProblemsActionable,
            F::NoFirstFindingTarget => K::NoFirstFindingTarget,
            F::MameMissingMembers { .. } => K::MameMissingMembers,
            F::MameParentDependencyMissing { .. } => K::MameParentDependencyMissing,
            F::MameBadDumpReference => K::MameBadDumpReference,
            F::MameNoDumpReference => K::MameNoDumpReference,
            F::ArtworkCoverMissing => K::ArtworkCoverMissing,
            F::ArtworkRefreshAvailable => K::ArtworkRefreshAvailable,
            F::IdentityPreventsArtworkMatch => K::IdentityPreventsArtworkMatch,
            F::ArtworkCachedUsable => K::ArtworkCachedUsable,
            F::ArtworkCacheStale => K::ArtworkCacheStale,
            F::ArtworkSourceUnavailable => K::ArtworkSourceUnavailable,
            F::ArtworkAlternatives { .. } => K::ArtworkAlternatives,
            F::EmulatorMultipleInstallations { .. } => K::EmulatorMultipleInstallations,
            F::FirmwareMissing => K::FirmwareMissing,
            F::BiosFolderChooserAvailable => K::BiosFolderChooserAvailable,
            F::LaunchNoCompatibleEmulator => K::LaunchNoCompatibleEmulator,
            F::LaunchReady => K::LaunchReady,
            F::LaunchWarningsUnacknowledged => K::LaunchWarningsUnacknowledged,
            F::LaunchFailed { .. } => K::LaunchFailed,
            F::NoGameSelected => K::NoGameSelected,
            F::CheatsNoneKnown => K::CheatsNoneKnown,
            F::PatchBaseMismatch => K::PatchBaseMismatch,
            F::PatchRevisionMismatchKnown => K::PatchRevisionMismatchKnown,
            F::ConversionPreviewAvailable { .. } => K::ConversionPreviewAvailable,
            F::ConversionPreservationUnknown => K::ConversionPreservationUnknown,
            F::MultiDiscRequiredMissing => K::MultiDiscRequiredMissing,
            F::PlayingLibraryPlan { .. } => K::PlayingLibraryPlan,
            F::DuplicatesExact => K::DuplicatesExact,
            F::UndoAvailable => K::UndoAvailable,
            F::UndoRefused { .. } => K::UndoRefused,
            F::RommSnapshotUnavailable { .. } => K::RommSnapshotUnavailable,
            F::OptionalSourceUnavailable => K::OptionalSourceUnavailable,
            F::LibraryLoadedEmpty => K::LibraryLoadedEmpty,
            F::FormatOperationUnsupported { .. } => K::FormatOperationUnsupported,
            F::NoCapabilityView => K::NoCapabilityView,
            F::DestinationReadOnly => K::DestinationReadOnly,
            F::DestinationChooserAvailable => K::DestinationChooserAvailable,
            F::RepairCompletedVerified { .. } => K::RepairCompletedVerified,
            F::RepairCompletedVerificationPending { .. } => K::RepairCompletedVerificationPending,
            F::VerificationUnavailable => K::VerificationUnavailable,
            F::SourceChangedSincePreview => K::SourceChangedSincePreview,
            F::QueuedWork { .. } => K::QueuedWork,
        }
    }

    /// Whether the fact carries what its kind promises. A fact that says "N
    /// folders could not be read" with N = 0, or names a title that is empty, is
    /// not evidence: it is ignored, so the engine never builds a message around a
    /// specific it was not given.
    pub(crate) fn is_valid(&self) -> bool {
        use GuidanceFact as F;
        let text = |value: &str| !value.trim().is_empty();
        match self {
            F::LegacySourceUnavailableWithScan { scan } | F::LegacySourceLastScan { scan } => {
                text(scan)
            }
            F::LegacyProblemBlocker { blocker } => text(blocker),
            F::LegacyProblemsFindings { count, .. } => *count > 0,
            F::LegacyTapeStructure { format, .. } => text(format),
            F::LegacyDatIdentity { name } => text(name),
            F::LegacyJobsRunning { count } => *count > 0,
            F::ScanPartialFailure { folder_count } => *folder_count > 0,
            F::IdentityCandidateOnly { title } => text(title),
            F::ProblemsActionable { finding_count } => *finding_count > 0,
            F::MameMissingMembers { missing_count } => *missing_count > 0,
            F::MameParentDependencyMissing { parent } => text(parent),
            F::ArtworkAlternatives {
                provider,
                alternative_count,
            } => text(provider) && *alternative_count > 0,
            F::EmulatorMultipleInstallations { emulator, count } => text(emulator) && *count >= 2,
            F::LaunchFailed {
                plain_failure_reason,
                ..
            } => text(plain_failure_reason),
            F::ConversionPreviewAvailable {
                source_format,
                target_format,
            } => text(source_format) && text(target_format),
            F::PlayingLibraryPlan { set_count, .. } => *set_count > 0,
            F::UndoRefused { plain_undo_reason } => text(plain_undo_reason),
            F::RommSnapshotUnavailable {
                plain_source_reason,
            } => text(plain_source_reason),
            F::FormatOperationUnsupported { format, operation } => text(format) && text(operation),
            F::QueuedWork { queued_count, .. } => *queued_count > 0,
            _ => true,
        }
    }

    /// The template values this fact supplies, text bounded in length.
    pub(crate) fn params(&self) -> Params {
        use GuidanceFact as F;
        let bounded = |value: &str| {
            let value = value.trim();
            if value.chars().count() <= MAX_PARAM_CHARS {
                Param::Text(value.to_string())
            } else {
                let cut: String = value.chars().take(MAX_PARAM_CHARS).collect();
                Param::Text(format!("{cut}…"))
            }
        };
        match self {
            F::LegacySourceUnavailableWithScan { scan } | F::LegacySourceLastScan { scan } => {
                vec![("scan", bounded(scan))]
            }
            F::LegacyProblemBlocker { blocker } => vec![("blocker", bounded(blocker))],
            F::LegacyProblemsFindings { count, attention } => vec![
                ("count", Param::Count(*count)),
                (
                    "attention_clause",
                    Param::Text(match attention {
                        Some(attention) if *attention > 0 => {
                            format!(", {attention} needing attention first")
                        }
                        _ => String::new(),
                    }),
                ),
            ],
            F::LegacyTapeStructure { format, blocks } => {
                vec![
                    ("format", bounded(format)),
                    ("blocks", Param::Count(*blocks)),
                ]
            }
            F::LegacyDatIdentity { name } => vec![("name", bounded(name))],
            F::LegacyJobsRunning { count } => vec![("count", Param::Count(*count))],
            F::ScanPartialFailure { folder_count } => {
                vec![("folder_count", Param::Count(*folder_count))]
            }
            F::IdentityCandidateOnly { title } => vec![("title", bounded(title))],
            F::ProblemsActionable { finding_count } => {
                vec![("finding_count", Param::Count(*finding_count))]
            }
            F::MameMissingMembers { missing_count } => {
                vec![("missing_count", Param::Count(*missing_count))]
            }
            F::MameParentDependencyMissing { parent } => vec![("parent", bounded(parent))],
            F::ArtworkAlternatives {
                provider,
                alternative_count,
            } => vec![
                ("provider", bounded(provider)),
                ("alternative_count", Param::Count(*alternative_count)),
            ],
            F::EmulatorMultipleInstallations { emulator, count } => vec![
                ("emulator", bounded(emulator)),
                ("count", Param::Count(*count)),
                ("count_word", Param::Text(number_word(*count))),
                (
                    "count_lower",
                    Param::Text(number_word(*count).to_lowercase()),
                ),
            ],
            F::LaunchFailed {
                plain_failure_reason,
                ..
            } => vec![("plain_failure_reason", bounded(plain_failure_reason))],
            F::ConversionPreviewAvailable {
                source_format,
                target_format,
            } => vec![
                ("source_format", bounded(source_format)),
                ("target_format", bounded(target_format)),
            ],
            F::PlayingLibraryPlan {
                set_count,
                link_count,
            } => vec![
                ("set_count", Param::Count(*set_count)),
                ("link_count", Param::Count(*link_count)),
            ],
            F::UndoRefused { plain_undo_reason } => {
                vec![("plain_undo_reason", bounded(plain_undo_reason))]
            }
            F::RommSnapshotUnavailable {
                plain_source_reason,
            } => vec![("plain_source_reason", bounded(plain_source_reason))],
            F::FormatOperationUnsupported { format, operation } => {
                vec![
                    ("format", bounded(format)),
                    ("operation", bounded(operation)),
                ]
            }
            F::QueuedWork {
                queued_count,
                running_count,
            } => vec![
                ("queued_count", Param::Count(*queued_count)),
                ("running_count", Param::Count(*running_count)),
            ],
            _ => Vec::new(),
        }
    }

    /// Identifies the *semantic event* this fact represents, for repeat
    /// suppression. It includes what makes the situation materially different (a
    /// title, a reason, an operation) and excludes ordinary counts and progress,
    /// which must not create a new event.
    pub(crate) fn semantic_key(&self) -> String {
        use GuidanceFact as F;
        let kind = format!("{:?}", self.kind());
        match self {
            F::LegacyProblemBlocker { blocker } => format!("{kind}:{blocker}"),
            F::LegacyDatIdentity { name } => format!("{kind}:{name}"),
            F::IdentityCandidateOnly { title } => format!("{kind}:{title}"),
            F::MameParentDependencyMissing { parent } => format!("{kind}:{parent}"),
            F::ArtworkAlternatives { provider, .. } => format!("{kind}:{provider}"),
            F::EmulatorMultipleInstallations { emulator, .. } => format!("{kind}:{emulator}"),
            F::LaunchFailed {
                plain_failure_reason,
                operation_id,
            } => format!("{kind}:{operation_id}:{plain_failure_reason}"),
            F::ConversionPreviewAvailable {
                source_format,
                target_format,
            } => format!("{kind}:{source_format}:{target_format}"),
            F::UndoRefused { plain_undo_reason } => format!("{kind}:{plain_undo_reason}"),
            F::RommSnapshotUnavailable {
                plain_source_reason,
            } => format!("{kind}:{plain_source_reason}"),
            F::FormatOperationUnsupported { format, operation } => {
                format!("{kind}:{format}:{operation}")
            }
            F::RepairCompletedVerified { operation_id }
            | F::RepairCompletedVerificationPending { operation_id } => {
                format!("{kind}:{operation_id}")
            }
            _ => kind,
        }
    }
}

// --- Evidence and context -------------------------------------------------------------

/// Evidence the page already holds.
///
/// The optional fields are the pre-engine shape and keep working: `None` means
/// unknown or not yet loaded and produces no guidance, never "nothing found".
/// New callers supply typed [`GuidanceFact`]s in `facts`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GuidanceEvidence {
    pub(crate) has_games: Option<bool>,
    pub(crate) source_available: Option<bool>,
    pub(crate) source_last_scan: Option<String>,
    pub(crate) launch_identity_verified: Option<bool>,
    pub(crate) blocker: Option<String>,
    pub(crate) tape_format: Option<String>,
    pub(crate) tape_blocks: Option<usize>,
    pub(crate) dat_name: Option<String>,
    pub(crate) operation_succeeded: Option<bool>,
    /// Current findings in the Problems inbox: `None` while still checking.
    pub(crate) problems_actionable: Option<usize>,
    pub(crate) problems_needing_attention: Option<usize>,
    pub(crate) jobs_running: Option<usize>,
    /// Typed facts from a page-owned adapter.
    pub(crate) facts: Vec<GuidanceFact>,
}

impl GuidanceEvidence {
    /// Every valid fact, in a fixed order: the projection of the optional fields
    /// first, then the explicit facts. Invalid facts are dropped here, so nothing
    /// downstream can build a message around a payload the caller did not give.
    pub(crate) fn facts(&self) -> Vec<GuidanceFact> {
        use GuidanceFact as F;
        let mut facts = Vec::new();
        match self.has_games {
            Some(false) => facts.push(F::LegacyLibraryEmpty),
            Some(true) => facts.push(F::LegacyLibraryHasGames),
            None => {}
        }
        if self.source_available == Some(false) {
            facts.push(match &self.source_last_scan {
                Some(scan) => F::LegacySourceUnavailableWithScan { scan: scan.clone() },
                None => F::LegacySourceUnavailable,
            });
        } else if let Some(scan) = &self.source_last_scan {
            facts.push(F::LegacySourceLastScan { scan: scan.clone() });
        }
        match self.launch_identity_verified {
            Some(false) => facts.push(F::LegacyLaunchIdentityUnverified),
            Some(true) => facts.push(F::LegacyLaunchIdentityVerified),
            None => {}
        }
        if let Some(blocker) = &self.blocker {
            facts.push(F::LegacyProblemBlocker {
                blocker: blocker.clone(),
            });
        } else {
            match (self.problems_actionable, self.problems_needing_attention) {
                (None, _) => facts.push(F::LegacyProblemsChecking),
                (Some(0), _) => facts.push(F::LegacyProblemsClear),
                (Some(count), attention) => facts.push(F::LegacyProblemsFindings {
                    count: count as u64,
                    attention: attention.map(|attention| attention as u64),
                }),
            }
        }
        if let (Some(format), Some(blocks)) = (&self.tape_format, self.tape_blocks) {
            facts.push(F::LegacyTapeStructure {
                format: format.clone(),
                blocks: blocks as u64,
            });
        }
        if let Some(name) = &self.dat_name {
            facts.push(F::LegacyDatIdentity { name: name.clone() });
        }
        match self.jobs_running {
            Some(count) if count > 0 => facts.push(F::LegacyJobsRunning {
                count: count as u64,
            }),
            _ => facts.push(F::LegacyJobsIdle),
        }
        facts.extend(self.facts.iter().cloned());
        facts.retain(GuidanceFact::is_valid);
        facts
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GuidanceContext {
    pub(crate) page: GuidancePage,
    pub(crate) evidence: GuidanceEvidence,
}

impl GuidanceContext {
    pub(crate) fn new(page: GuidancePage) -> Self {
        Self {
            page,
            evidence: GuidanceEvidence::default(),
        }
    }
}
