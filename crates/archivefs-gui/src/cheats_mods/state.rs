use crate::*;


/// What `try_resolve_dolphin_provider_from_local_sources` found, kept
/// distinct from `dolphin_provider` itself because `NotLoaded` alone can no
/// longer distinguish "hasn't looked yet" from "looked locally and found
/// nothing" now that a fruitless local lookup does not fall through to an
/// automatic network request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum DolphinLocalLookupState {
    #[default]
    NotAttempted,
    NoCatalogueInstalled,
    NotInCatalogue,
    RegionMismatch,
    NoUsableCodes {
        warnings: Vec<String>,
    },
}


pub(crate) struct CheatWorkflowState {
    /// Exact-byte identity - the same `ArchiveRecord.mount_plan.archive
    /// .path` identity the rest of the app uses, never a filename.
    pub(crate) archive_path: PathBuf,
    pub(crate) display_name: String,
    pub(crate) normalized_name: String,
    pub(crate) platform: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) source_root: PathBuf,
    pub(crate) size_bytes: Option<u64>,
    /// The emulator adapter is explicit and independent from archive
    /// selection. PCSX2 is offered only for a canonical PS2 archive.
    pub(crate) adapter: CheatEmulatorAdapter,
    pub(crate) identity_request: Option<GameIdentityRequest>,
    pub(crate) identity: CheatStepResource<(GameIdentityRequest, GameIdentityReport)>,
    pub(crate) preview_request: Option<CheatPreviewRequestKey>,
    pub(crate) preview: CheatStepResource<CheatPreviewResponse>,
    pub(crate) transaction: CheatTransactionState,
    pub(crate) transaction_notice: Option<String>,
    /// The explicitly selected profile. Preselected only when exactly
    /// one eligible profile exists (the CLI's own auto-selection rule);
    /// with several eligible profiles the user must choose - never
    /// silently picked.
    pub(crate) selected_profile_id: Option<String>,
    pub(crate) selected_pcsx2_profile_id: Option<String>,
    /// The PCSX2 profile identity bound to this archive's inventory.
    pub(crate) pcsx2_inventory_profile_id: Option<String>,
    pub(crate) pcsx2_inventory: CheatStepResource<Pcsx2PnachInventory>,
    pub(crate) pcsx2_activation: CheatActivationReadiness,
    pub(crate) pcsx2_activation_receiver: Option<Receiver<Result<Option<bool>, String>>>,
    pub(crate) pcsx2_gamehacking: CheatStepResource<Pcsx2GameHackingState>,
    /// Dolphin-family GameHacking.org coverage. Wii is adapted into this
    /// existing state only after its own identity and safety policy runs.
    pub(crate) gamecube_gamehacking: CheatStepResource<GameCubeGameHackingState>,
    pub(crate) gamecube_gamehacking_request: Option<DolphinGameHackingRequestKey>,
    pub(crate) gamecube_gamehacking_cancellation: Option<Arc<AtomicBool>>,
    pub(crate) gamecube_gamehacking_generation: u64,
    /// True exactly when the current `gamecube_gamehacking` `Failed` state
    /// was classified as `GameHackingErrorKind::CloudflareBlocked` (see
    /// `GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE`) rather than an ordinary
    /// failure - drives the dedicated neutral/blocked banner and hides
    /// Retry, since retrying immediately cannot help and core-side cooldown
    /// gating already prevents hammering a blocked origin.
    pub(crate) gamecube_gamehacking_blocked: bool,
    /// The open browser-assisted import flow for the selected GameHacking
    /// candidate. It is reset when the selected game changes.
    pub(crate) browser_import: Option<BrowserImportState>,
    /// Local reason the import panel could not be opened.
    pub(crate) browser_import_open_error: Option<(String, String)>,
    /// BSFree Archive GameCube coverage in Cheats & Mods: an optional local
    /// SQLite source, matched by platform + title to the selected archive's
    /// verified Dolphin Game ID. Installable only for the proven hex-pair
    /// formats; every other BSFree record stays browseable.
    pub(crate) bsfree_gamecube: CheatStepResource<BsFreeGameCubeGuiState>,
    pub(crate) bsfree_gamecube_cancellation: Option<Arc<AtomicBool>>,
    pub(crate) bsfree_gamecube_generation: u64,
    /// BSFree Wii coverage: the same optional local SQLite source, matched by
    /// platform + title to the archive's verified Dolphin Wii Game ID. Only
    /// the verified hex-pair subset is installable.
    pub(crate) bsfree_wii: CheatStepResource<BsFreeWiiGuiState>,
    pub(crate) bsfree_wii_cancellation: Option<Arc<AtomicBool>>,
    pub(crate) bsfree_wii_generation: u64,
    pub(crate) selected_dolphin_profile_id: Option<String>,
    /// An optional additional Dolphin configuration directory to scan,
    /// typed by the user - covers portable/AppImage installs, which have
    /// no fixed native or Flatpak path EmuWiz can discover on its own.
    /// Never auto-populated; rescanning without it drops nothing already
    /// found under the standard native/Flatpak locations.
    pub(crate) dolphin_explicit_root: String,
    /// The Dolphin profile identity bound to this archive's inventory.
    pub(crate) dolphin_inventory_profile_id: Option<String>,
    pub(crate) dolphin_inventory: CheatStepResource<DolphinGameIniInventory>,
    pub(crate) dolphin_activation: CheatActivationReadiness,
    pub(crate) dolphin_activation_receiver: Option<Receiver<Result<Option<bool>, String>>>,
    /// External discovery is bound only to exact archive/game/revision identity.
    /// Dolphin paths enter only when the adapter builds the selection below.
    pub(crate) dolphin_provider_request: Option<DolphinProviderRequestKey>,
    pub(crate) dolphin_provider: CheatStepResource<GeckoProviderFetchResult>,
    pub(crate) dolphin_provider_selection: Option<DolphinProviderSelectionState>,
    pub(crate) dolphin_destination_error: Option<String>,
    /// What the network-free local lookup (catalogue, then cached
    /// single-game result) found when it found nothing usable to show in
    /// `dolphin_provider` - lets the beginner view distinguish "still
    /// looking" (`NotAttempted`, `dolphin_provider` stays `NotLoaded`
    /// briefly) from "looked locally and there is genuinely nothing here"
    /// (any other variant, `dolphin_provider` stays `NotLoaded`
    /// indefinitely since no automatic network request follows).
    pub(crate) dolphin_local_lookup: DolphinLocalLookupState,
    /// The most recent automatic-selection outcome for the Dolphin
    /// profile - drives the beginner view's "using X automatically"
    /// confirmation, the "choose one of N profiles" chooser, or the
    /// setup-needed state. Recomputed whenever `dolphin_profiles`
    /// changes; never recomputed merely because the page re-renders.
    pub(crate) dolphin_profile_selection: Option<EmulatorProfileSelection>,
    /// The profile currently highlighted in the profile chooser dialog,
    /// pending the user's explicit "Use selected profile" confirmation.
    pub(crate) dolphin_profile_choice: Option<String>,
    /// Whether the beginner page's "Details" disclosure is expanded.
    /// Owned here (not egui's own collapsing-header memory) so it is
    /// trivially set from a test fixture and so it can be reset
    /// deliberately whenever the workflow state it discloses changes.
    /// Collapsed (`false`) by default.
    pub(crate) dolphin_details_open: bool,
    /// Whether the beginner install confirmation's "Show exact changes"
    /// disclosure is expanded. Resets to `false` whenever a fresh
    /// confirmation begins (see `start_beginner_install_dolphin`).
    pub(crate) dolphin_show_exact_changes: bool,
    pub(crate) selected_xenia_profile_id: Option<String>,
    /// An explicit Xenia Canary directory typed by the user - the only
    /// way EmuWiz ever learns of a Xenia install, since it has no
    /// single standard location.
    pub(crate) xenia_explicit_root: String,
    pub(crate) xenia_provider_request: Option<XeniaProviderRequestKey>,
    pub(crate) xenia_provider: CheatStepResource<XeniaProviderFetchResult>,
    /// Which of the provider's returned candidate documents the user
    /// picked - Xenia's own dataset legitimately has multiple files per
    /// Title ID (different Title Update/module-hash variants).
    pub(crate) xenia_selected_candidate_index: Option<usize>,
    pub(crate) xenia_selection: Option<XeniaSelectionState>,
    pub(crate) xenia_destination_error: Option<String>,
    /// The most recent automatic-selection outcome for the Xenia Canary
    /// profile - same role as `dolphin_profile_selection`.
    pub(crate) xenia_profile_selection: Option<EmulatorProfileSelection>,
    /// The profile currently highlighted in the profile chooser dialog,
    /// pending the user's explicit "Use selected profile" confirmation.
    pub(crate) xenia_profile_choice: Option<String>,
    /// Xenia's counterpart to `dolphin_details_open`.
    pub(crate) xenia_details_open: bool,
    /// Xenia's counterpart to `dolphin_show_exact_changes`.
    pub(crate) xenia_show_exact_changes: bool,
    /// Independent source mode. Changing it never changes the archive,
    /// profile, destination, or any fetched result retained by another mode.
    pub(crate) source_mode: CheatSourceMode,
    /// The profile identity bound to the current read-only installed-library
    /// inspection. A profile change invalidates only this observation.
    pub(crate) existing_library_profile_id: Option<String>,
    pub(crate) existing_library: CheatStepResource<RetroArchCheatLibraryInspection>,
    /// Step 2: the trusted-source cache listing, loaded in the
    /// background when the workflow opens.
    pub(crate) source_list: CheatStepResource<CheatSourceList>,
    /// Step 2: the most recent catalogue retrieval (network fetch or
    /// offline cached-snapshot reuse). A result whose source no longer
    /// matches `selected_source_id` at receive time is discarded.
    pub(crate) source_fetch: CheatStepResource<CheatSourceFetchResult>,
    /// The explicitly selected trusted source - preselected only when
    /// exactly one enabled trusted source exists (same single-candidate
    /// rule as profiles).
    pub(crate) selected_source_id: Option<String>,
    /// Whether "Fetch / Update catalogue" bypasses the fresh-cache
    /// short-circuit (the CLI's `--force-refresh`). Never applies to
    /// offline reuse.
    pub(crate) fetch_force_refresh: bool,
    /// Stage 4: the ranked candidate cheat files for this exact archive,
    /// bound to the key that produced them.
    pub(crate) candidates: CheatStepResource<CheatCandidateStage>,
    pub(crate) candidates_request: Option<CheatPreviewRequestKey>,
    /// Stage 4 search box, used only when the list is capped.
    pub(crate) candidate_query: String,
    /// Stages 5 and 6: the chosen candidate, its parsed cheats, and the
    /// user's per-cheat choices. Cleared whenever the candidate list is.
    pub(crate) candidate_selection: Option<CheatCandidateSelection>,
    /// A candidate the user chose that could not be opened - kept so the
    /// failure stays on screen instead of silently reverting the choice.
    pub(crate) candidate_load_error: Option<String>,
}


/// One completed candidate match, bound to the exact context that produced
/// it so a stale result can never be shown against a different archive,
/// profile, or catalogue snapshot.
pub(crate) struct CheatCandidateStage {
    pub(crate) key: CheatPreviewRequestKey,
    pub(crate) catalogue_root: PathBuf,
    pub(crate) list: CheatCandidateList,
}


/// The chosen candidate and everything derived from it.
pub(crate) struct CheatCandidateSelection {
    pub(crate) candidate: CheatCandidate,
    pub(crate) loaded: LoadedCandidate,
    pub(crate) selection: CheatSelection,
}



/// What one generated-install preview produced, alongside the shared
/// report. Retained so review, confirmation, and the result view can all
/// name the exact destination and staged bytes the user approved.
#[derive(Clone)]
pub(crate) struct GeneratedCheatInstall {
    pub(crate) staging_root: PathBuf,
    pub(crate) destination: ResolvedCheatDestination,
    pub(crate) staged: StagedCheatFile,
    pub(crate) candidate_display_name: String,
}



#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DolphinProviderRequestKey {
    pub(crate) archive_path: PathBuf,
    pub(crate) game_id: String,
    pub(crate) revision: u16,
}



#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DolphinGameHackingRequestKey {
    pub(crate) archive_path: PathBuf,
    pub(crate) platform: String,
    pub(crate) game_id: String,
    pub(crate) revision: Option<u16>,
    pub(crate) generation: u64,
}



#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WiiGameHackingFetchMode {
    CacheOnly,
    ExplicitNetworkAllowed,
}



/// Adapter-owned state derived from inert provider results plus the selected
/// Dolphin destination. Provider retrieval never receives either of these paths.
#[derive(Clone)]
pub(crate) struct DolphinProviderSelectionState {
    pub(crate) destination: LoadedDolphinDestination,
    pub(crate) selection: DolphinProviderCodeSelection,
}



#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XeniaProviderRequestKey {
    pub(crate) archive_path: PathBuf,
    pub(crate) title_id: String,
}



/// Adapter-owned state derived from the chosen candidate document plus
/// the real destination file it would install to.
#[derive(Clone)]
pub(crate) struct XeniaSelectionState {
    pub(crate) candidate: XeniaCandidate,
    pub(crate) destination: LoadedXeniaDestination,
    pub(crate) selection: XeniaPatchSelection,
}



/// The Dolphin equivalent of `GeneratedCheatInstall`: the same matched file,
/// re-written with only `[Gecko_Enabled]` replaced.
#[derive(Clone)]
pub(crate) struct GeneratedDolphinInstall {
    pub(crate) staging_root: PathBuf,
    pub(crate) provider: GeckoProviderFetchResult,
    pub(crate) destination: PathBuf,
    pub(crate) staged: StagedDolphinIni,
}



/// The GameHacking.org GameCube counterpart to `GeneratedDolphinInstall`:
/// the staged, surgically edited GameSettings file for a selection of
/// externally sourced `ActionReplay`/`Gecko` cheats, rather than the
/// bundled Gecko catalogue.
#[derive(Clone)]
pub(crate) struct GeneratedGameCubeGameHackingInstall {
    pub(crate) staging_root: PathBuf,
    pub(crate) staged: StagedGameCubeIni,
    pub(crate) profile: DolphinProfile,
}


/// The BSFree Archive GameCube equivalent of
/// `GeneratedGameCubeGameHackingInstall`, additionally carrying the two-pass
/// duplicate/conflict analysis the BSFree provider computed before staging, so
/// the review/result UI can list already-installed, skipped, conflict, and
/// unsupported cheats honestly.
pub(crate) struct GeneratedBsFreeGameCubeInstall {
    pub(crate) staging_root: PathBuf,
    pub(crate) staged: StagedGameCubeIni,
    pub(crate) profile: DolphinProfile,
    pub(crate) findings: Vec<BsFreeDedupFinding>,
    pub(crate) skipped_duplicates: Vec<String>,
    pub(crate) skipped_unselectable: Vec<String>,
}



/// The BSFree Wii equivalent of [`GeneratedBsFreeGameCubeInstall`]: the staged
/// Dolphin GameSettings INI (produced by the shared Wii adapter) plus the
/// dedup/conflict findings. The staged artifact type is the same, because both
/// platforms write the same Dolphin `GameSettings` structure.
#[derive(Clone)]
pub(crate) struct GeneratedBsFreeWiiInstall {
    pub(crate) staging_root: PathBuf,
    pub(crate) staged: StagedGameCubeIni,
    pub(crate) profile: DolphinProfile,
    pub(crate) findings: Vec<BsFreeWiiDedupFinding>,
    pub(crate) skipped_duplicates: Vec<String>,
    pub(crate) skipped_unselectable: Vec<String>,
}



/// The Xenia equivalent of `GeneratedDolphinInstall`: the exact chosen
/// candidate document, staged as a real merged `.patch.toml`.
#[derive(Clone)]
pub(crate) struct GeneratedXeniaInstall {
    pub(crate) staging_root: PathBuf,
    pub(crate) candidate: XeniaCandidate,
    pub(crate) destination: PathBuf,
    pub(crate) staged: StagedXeniaPatchFile,
}


pub(crate) struct Pcsx2GameHackingState {
    pub(crate) status: GameHackingMatchStatus,
    pub(crate) detail: String,
    pub(crate) game: Option<GameHackingGame>,
    pub(crate) match_candidates: Vec<GameHackingMatchCandidate>,
    pub(crate) candidates: Vec<Pcsx2CheatCandidate>,
    pub(crate) selection: Pcsx2CheatSelection,
    pub(crate) cached_fallback: bool,
}


/// GameCube-only GameHacking.org coverage: matched title, named cheats,
/// and - unlike the initial preview-only milestone - a selection of
/// exactly which `ActionReplay`/`Gecko` cheats to install into the real
/// Dolphin GameSettings file. `RawUnknown`/`Unsupported` cheats can never
/// be selected (see `GameCubeCheatSelection::from_cheats`).
pub(crate) struct GameCubeGameHackingState {
    pub(crate) status: GameHackingGameCubeMatchStatus,
    pub(crate) detail: String,
    pub(crate) game: Option<GameHackingGameCubeGame>,
    pub(crate) match_candidates: Vec<GameHackingGameCubeMatchCandidate>,
    pub(crate) cheats: Vec<GameHackingGameCubeCheat>,
    pub(crate) selection: GameCubeCheatSelection,
    pub(crate) cached_fallback: bool,
}


/// State for the explicitly user-mediated GameHacking.org browser import.
/// No content is fetched or applied by this state; the core importer owns
/// validation, cache writes, and provenance.
pub(crate) struct BrowserImportState {
    pub(crate) plan: BrowserImportPlan,
    pub(crate) identity: BrowserImportLocalIdentity,
    pub(crate) candidate_title: String,
    pub(crate) kind: Option<BrowserImportKind>,
    pub(crate) pasted: String,
    pub(crate) paste_open: bool,
    pub(crate) notice: Option<String>,
    pub(crate) failure: Option<(String, String)>,
    pub(crate) outcome: Option<BrowserImportOutcome>,
}


impl BrowserImportState {
    pub(crate) fn new(
        plan: BrowserImportPlan,
        identity: BrowserImportLocalIdentity,
        candidate_title: String,
    ) -> Self {
        Self {
            plan,
            identity,
            candidate_title,
            kind: None,
            pasted: String::new(),
            paste_open: false,
            notice: None,
            failure: None,
            outcome: None,
        }
    }

    pub(crate) fn clear_result(&mut self) {
        self.notice = None;
        self.failure = None;
        self.outcome = None;
    }
}


/// BSFree GameCube coverage inside Cheats & Mods: the matched BSFree game,
/// its classified cheats, and the user's explicit per-cheat selection. Only
/// the installable formats (`GeckoEquivalent`, `ActionReplayNative`) can be
/// selected; everything else stays browseable but unselectable. Identity is
/// the selected archive's verified Dolphin Game ID; the BSFree game itself is
/// matched by platform + title and always requires review before Apply.
pub(crate) struct BsFreeGameCubeGuiState {
    pub(crate) status: BsFreeGameCubeSearchStatus,
    pub(crate) detail: String,
    pub(crate) candidates: Vec<BsFreeGameCubeMatch>,
    pub(crate) game: Option<BsFreeGameCubeMatch>,
    pub(crate) cheats: Vec<BsFreeGameCubeCheat>,
    pub(crate) selection: BsFreeGameCubeCheatSelection,
    /// Destination-based duplicate/conflict analysis computed once per fetch
    /// against the real Dolphin GameSettings file, so the list can show
    /// "Already installed"/"Conflict" honestly. Refreshed on every re-search.
    pub(crate) analysis: Vec<BsFreeDedupFinding>,
    /// Editable search title, pre-filled with the archive's title. A different
    /// search re-runs the bounded local BSFree query.
    pub(crate) search_title: String,
}


/// BSFree Wii coverage inside Cheats & Mods, mirroring the GameCube state.
/// Only the verified hex-pair subset is selectable; every other BSFree Wii
/// record (encrypted, unverified device, malformed) stays browse-only.
pub(crate) struct BsFreeWiiGuiState {
    pub(crate) status: BsFreeWiiSearchStatus,
    pub(crate) detail: String,
    pub(crate) candidates: Vec<BsFreeWiiMatch>,
    pub(crate) game: Option<BsFreeWiiMatch>,
    pub(crate) cheats: Vec<BsFreeWiiCheat>,
    pub(crate) selection: BsFreeWiiCheatSelection,
    /// Destination-based duplicate/conflict analysis, computed once per fetch
    /// against the real Dolphin GameSettings file.
    pub(crate) analysis: Vec<BsFreeWiiDedupFinding>,
    pub(crate) search_title: String,
}



#[derive(Clone)]
pub(crate) struct GeneratedPcsx2Install {
    pub(crate) staging_root: PathBuf,
    /// Present only when a legacy CRC-only file was found with
    /// EmuWiz-managed cheats that needed consolidating into the
    /// serial+CRC file this PCSX2 build actually reads. Applied as its own
    /// chained operation, with its own journal and independent Undo (via
    /// the generic History & Logs rollback flow), immediately after the
    /// primary install succeeds - PCSX2's shared preview pipeline treats
    /// two verified-exact entries for one identity in the same report as
    /// an unresolvable ambiguity, so this can never be folded into the
    /// primary plan.
    pub(crate) legacy_migration_report: Option<SharedPreviewReport>,
}



#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GameIdentityRequest {
    pub(crate) archive_path: PathBuf,
    pub(crate) platform: Option<String>,
    pub(crate) adapter: CheatEmulatorAdapter,
}



#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheatPreviewRequestKey {
    pub(crate) archive_path: PathBuf,
    pub(crate) platform: Option<String>,
    pub(crate) adapter: CheatEmulatorAdapter,
    pub(crate) profile_id: Option<String>,
    pub(crate) source_mode: CheatSourceMode,
    pub(crate) source_id: Option<String>,
    pub(crate) snapshot_id: Option<String>,
}



#[derive(Debug)]
pub(crate) enum CheatPreviewOutcome {
    Ready(SharedPreviewReport),
    Failed(CheatPreviewFailure),
}



#[derive(Debug)]
pub(crate) enum CheatPreviewFailure {
    Shared(SharedPreviewError),
    Materialization(RetroArchMaterializationError),
    /// Generating, staging, or previewing a selected-cheat install failed.
    InstallPlan(CheatInstallPlanError),
    /// Generating, staging, or previewing a Dolphin Gecko install failed.
    DolphinInstallPlan(DolphinInstallPlanError),
    /// Generating, staging, or previewing a Xenia patch install failed.
    XeniaInstallPlan(XeniaInstallPlanError),
    /// Generating, staging, or previewing a PCSX2 PNACH install failed.
    Pcsx2InstallPlan(Pcsx2InstallPlanError),
    /// Generating, staging, or previewing a GameHacking.org GameCube
    /// cheat install or removal failed.
    GameCubeGameHackingInstallPlan(GameCubeInstallPlanError),
    /// Generating, staging, or previewing a BSFree Archive GameCube cheat
    /// install failed.
    BsFreeGameCubeInstallPlan(BsFreeGameCubeError),
}


impl std::fmt::Display for CheatPreviewFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shared(error) => error.fmt(formatter),
            Self::Materialization(error) => error.fmt(formatter),
            Self::InstallPlan(error) => error.fmt(formatter),
            Self::DolphinInstallPlan(error) => error.fmt(formatter),
            Self::XeniaInstallPlan(error) => error.fmt(formatter),
            Self::Pcsx2InstallPlan(error) => error.fmt(formatter),
            Self::GameCubeGameHackingInstallPlan(error) => error.fmt(formatter),
            Self::BsFreeGameCubeInstallPlan(error) => error.fmt(formatter),
        }
    }
}


pub(crate) struct CheatPreviewResponse {
    pub(crate) key: CheatPreviewRequestKey,
    pub(crate) outcome: CheatPreviewOutcome,
    pub(crate) materialized: Option<RetroArchMaterializedPreview>,
    /// Present only for the generated-file install path: the staged bytes
    /// and the destination they were previewed against.
    pub(crate) generated: Option<GeneratedCheatInstall>,
    /// Present only for the Dolphin Gecko install path.
    pub(crate) dolphin_generated: Option<GeneratedDolphinInstall>,
    /// Present only for the Xenia patch install path.
    pub(crate) xenia_generated: Option<GeneratedXeniaInstall>,
    /// Present only for the GameHacking.org PCSX2 install path.
    pub(crate) pcsx2_generated: Option<GeneratedPcsx2Install>,
    /// Present only for the GameHacking.org GameCube install/removal path.
    pub(crate) gamecube_gamehacking_generated: Option<GeneratedGameCubeGameHackingInstall>,
    /// Present only for the BSFree Archive GameCube install path. A staged
    /// Dolphin GameSettings INI plus the provider's dedup/conflict findings;
    /// kept distinct so the two sources never cross-talk in the shared
    /// review/apply/result UI.
    pub(crate) bsfree_gamecube_generated: Option<GeneratedBsFreeGameCubeInstall>,
    /// Present only for the BSFree Archive Wii install path - the same shape
    /// as the GameCube one, routed through the shared Wii/Dolphin adapter.
    pub(crate) bsfree_wii_generated: Option<GeneratedBsFreeWiiInstall>,
}


pub(crate) enum CheatPreviewWork {
    Shared(SharedPreviewRequest),
    RetroArch(RetroArchMaterializationRequest),
}


pub(crate) enum CheatTransactionState {
    Idle,
    Review {
        key: CheatPreviewRequestKey,
        plan: SharedTransactionPlan,
        replacement_approved: bool,
    },
    Applying {
        key: CheatPreviewRequestKey,
        receiver: Receiver<Result<SharedApplyResult, String>>,
    },
    Result {
        key: CheatPreviewRequestKey,
        result: SharedApplyResult,
    },
}



#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheatEmulatorAdapter {
    RetroArch,
    Pcsx2,
    Dolphin,
    Xenia,
    Unsupported,
}



#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheatActivationReadiness {
    Enabled,
    Disabled,
    Unknown,
}


impl CheatActivationReadiness {
    pub(crate) const fn from_bool(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Enabled,
            Some(false) => Self::Disabled,
            None => Self::Unknown,
        }
    }
}


impl CheatEmulatorAdapter {
    pub(crate) const fn display_name(self) -> Option<&'static str> {
        match self {
            Self::RetroArch => Some("RetroArch"),
            Self::Pcsx2 => Some("PCSX2"),
            Self::Dolphin => Some("Dolphin"),
            Self::Xenia => Some("Xenia"),
            Self::Unsupported => None,
        }
    }
}



#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheatSourceMode {
    ExistingRetroArchLibrary,
    ArchiveFsTrustedCatalogue,
}


impl CheatSourceMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ExistingRetroArchLibrary => "Existing RetroArch library",
            Self::ArchiveFsTrustedCatalogue => "EmuWiz cached catalogue",
        }
    }
}



#[derive(Default)]
pub(crate) struct CheatArchivePickerState {
    pub(crate) search: String,
    pub(crate) platform_filter: Option<String>,
    pub(crate) source_filter: Option<PathBuf>,
    pub(crate) candidate: Option<PathBuf>,
}


impl CheatArchivePickerState {
    pub(crate) fn for_current(current: Option<&Path>, platform_filter: Option<String>) -> Self {
        Self {
            candidate: current.map(Path::to_path_buf),
            platform_filter,
            ..Self::default()
        }
    }
}


/// A background-loaded cheat-workflow resource. Stale-result protection
/// is ownership, the app's existing pattern: starting a new load
/// replaces this state wholesale, dropping the previous receiver, so a
/// superseded worker's `send` fails and its result can never apply.
/// Closing the workflow - or the selected archive changing - drops the
/// whole `CheatWorkflowState` the same way.
pub(crate) enum CheatStepResource<T> {
    NotLoaded,
    Loading {
        receiver: Receiver<Result<T, String>>,
    },
    Ready(T),
    Failed(String),
}


pub(crate) enum SharedHistoryState {
    NotLoaded,
    Loading {
        receiver: Receiver<Result<SharedHistoryReport, String>>,
    },
    Ready(SharedHistoryReport),
    Failed(String),
}


pub(crate) enum SharedRollbackState {
    Idle,
    Previewing {
        receiver: Receiver<Result<(SharedRollbackPreview, PathBuf, PathBuf), String>>,
    },
    Review {
        preview: SharedRollbackPreview,
        history_root: PathBuf,
        backup_root: PathBuf,
    },
    Applying {
        receiver: Receiver<Result<SharedRollbackResult, String>>,
    },
    Result(SharedRollbackResult),
    Failed(String),
}


pub(crate) enum HistoryPageAction {
    PreviewRollback {
        journal_path: PathBuf,
        destination_root: PathBuf,
    },
    ConfirmRollback,
    CancelRollback,
    Refresh,
}


/// The Details "Game ID" row's three-way state - distinct from
/// `BeginnerCheatStatus::IdentityUnavailable` only in that it does not
/// require `dolphin_profile_selection` to be resolved first, matching what
/// the row itself actually depends on.
pub(crate) enum DolphinIdentityRowState<'a> {
    Verified(&'a str),
    Pending,
    Unavailable,
}



/// The beginner page's plain-English status - one of the exact statuses
/// the milestone specifies. Every adapter maps its own technical state
/// into this same small vocabulary so a first-time user never has to
/// learn provider/profile/identity terminology just to tell whether
/// EmuWiz found anything yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BeginnerCheatStatus {
    FindingCompatibleCheats,
    CheatsFound {
        compatible_count: usize,
    },
    NoCompatibleCheatsFound,
    /// Dolphin's upstream GameSettings dataset has no file at all for this
    /// exact game ID (an exact-lookup HTTP 404). Distinct from
    /// `NoCompatibleCheatsFound` (a file exists, but nothing in it is safe
    /// to offer): here there is nothing to offer because upstream simply
    /// never published anything for this game - a normal outcome, not an
    /// error, so it renders with neutral/info styling rather than the
    /// warning/blocked tones used elsewhere in this enum.
    NoUpstreamCheatsAvailable,
    EmulatorSetupNeeded,
    /// Multiple valid profiles were found; the profile chooser (rendered
    /// separately) is asking the user to pick one.
    ChooseEmulatorProfile,
    CouldNotCheckForCheats {
        detail: String,
    },
    UsingSavedResultsWhileOffline,
    /// Identity inspection reached a final result, but it never produced a
    /// `Verified` exact game ID (malformed image, or a recognised format
    /// EmuWiz cannot yet decode without extracting the full image) -
    /// a terminal state, never re-attempted automatically, so the page
    /// never spins on "Finding compatible cheats" forever.
    IdentityUnavailable {
        detail: String,
    },
}


impl BeginnerCheatStatus {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::FindingCompatibleCheats => "Finding compatible cheats".to_string(),
            Self::CheatsFound { compatible_count } => {
                let noun = if *compatible_count == 1 {
                    "compatible enhancement"
                } else {
                    "compatible enhancements"
                };
                format!("{compatible_count} {noun} found")
            }
            Self::NoCompatibleCheatsFound => "No compatible cheats found".to_string(),
            Self::NoUpstreamCheatsAvailable => {
                "No upstream Dolphin cheats are available for this game.".to_string()
            }
            Self::EmulatorSetupNeeded => "Emulator setup needed".to_string(),
            Self::ChooseEmulatorProfile => "Choose an emulator profile".to_string(),
            Self::CouldNotCheckForCheats { .. } => "Could not check for cheats".to_string(),
            Self::UsingSavedResultsWhileOffline => "Using saved results while offline".to_string(),
            Self::IdentityUnavailable { .. } => "Exact Game ID unavailable".to_string(),
        }
    }

    pub(crate) fn tone(&self) -> widgets::StatusTone {
        match self {
            Self::FindingCompatibleCheats
            | Self::EmulatorSetupNeeded
            | Self::ChooseEmulatorProfile => widgets::StatusTone::Pending,
            Self::CheatsFound { .. } => widgets::StatusTone::Success,
            Self::NoCompatibleCheatsFound | Self::UsingSavedResultsWhileOffline => {
                widgets::StatusTone::Warning
            }
            Self::NoUpstreamCheatsAvailable => widgets::StatusTone::Info,
            Self::CouldNotCheckForCheats { .. } | Self::IdentityUnavailable { .. } => {
                widgets::StatusTone::Blocked
            }
        }
    }
}



/// Every distinct platform actually present with a non-zero count, sorted
/// alphabetically, plus a separate `Unknown` count - the shared "All /
/// <platform> (count) / Unknown" data behind the platform strip on
/// Library and Mount. Derived purely from live per-archive platform
/// strings (never a fixed list), so a canonical platform the registry
/// recognises but that has zero archives never clutters the strip, and a
/// platform with real archives is never hidden just because EmuWiz has
/// no cheat adapter for it. `None` (no assigned platform) counts as
/// Unknown, matching `persisted_archive_has_unknown_platform`.
pub(crate) struct DetectedPlatformCounts {
    pub(crate) named: Vec<(String, usize)>,
    pub(crate) unknown: usize,
}



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheatCandidatePrerequisite {
    ProfileCheatDirectoryUnresolved,
    CatalogueNotRetrieved,
    CatalogueLocalPathUnavailable,
}


impl CheatCandidatePrerequisite {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::ProfileCheatDirectoryUnresolved => {
                "Select an eligible RetroArch profile with a resolved cheat directory (Stage 1) before matching."
            }
            Self::CatalogueNotRetrieved => {
                "Retrieve or reuse the trusted catalogue snapshot in the Trusted catalogue details section before matching."
            }
            Self::CatalogueLocalPathUnavailable => {
                "The trusted catalogue's local path cannot be represented exactly; re-fetch it before matching."
            }
        }
    }
}

