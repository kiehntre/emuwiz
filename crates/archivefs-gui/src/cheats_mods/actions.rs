use crate::*;

pub(crate) enum CheatArchivePickerAction {
    Cancel,
    Select(PathBuf),
}


/// What the cheat workflow panel asks `update` to do.
#[derive(Clone)]
pub(crate) enum CheatWorkflowAction {
    ChooseArchive,
    OpenLibrary,
    RescanProfiles,
    RescanPcsx2Profiles,
    InspectPcsx2Profile,
    FetchPcsx2GameHacking {
        force_refresh: bool,
    },
    ConfirmPcsx2GameHackingMatch {
        game_id: u64,
    },
    TogglePcsx2CheatSelected {
        id: String,
        selected: bool,
    },
    InstallSelectedPcsx2,
    RescanDolphinProfiles,
    InspectDolphinProfile,
    InspectExistingLibrary,
    RefreshSources,
    ManageCatalogue,
    UseCachedSnapshot,
    ReviewApply,
    ConfirmApply,
    CancelApply,
    OpenApplyHistory,
    /// Stage 4: build (or rebuild) the ranked candidate list.
    MatchCandidates,
    /// Stage 4: choose one candidate by its catalogue-relative path.
    SelectCandidate(String),
    /// Stage 5: go back to the candidate list without losing it.
    ClearCandidateChoice,
    /// Stage 6 toggles. `enabled` distinguishes "included in the installed
    /// file" from "active as soon as RetroArch loads it".
    ToggleCheatSelected {
        index: u32,
        selected: bool,
    },
    ToggleCheatEnabled {
        index: u32,
        enabled: bool,
    },
    SelectAllCheats,
    ClearAllCheats,
    /// Stage 7: generate the file and preview installing it.
    BuildInstallPreview,
    /// Stage 9: restore whatever the install replaced.
    RollbackInstall,
    /// Retrieve exact-ID Gecko definitions from the one configured external
    /// provider. Refresh bypasses only a fresh cache, subject to rate limiting.
    FetchDolphinProvider {
        force_refresh: bool,
    },
    /// Dolphin Stage 4 toggles.
    ToggleDolphinCodeSelected {
        index: usize,
        selected: bool,
    },
    SelectAllDolphinCodes,
    ClearAllDolphinCodes,
    /// Dolphin Stage 5: stage the edited file and preview installing it.
    BuildDolphinInstallPreview,
    RescanXeniaProfiles,
    /// Retrieve patches for the verified Title ID from the Xenia Canary
    /// game-patches upstream provider. Refresh bypasses only a fresh
    /// cache, subject to rate limiting.
    FetchXeniaProvider {
        force_refresh: bool,
    },
    /// Choose which returned candidate document to work with - Xenia's
    /// own dataset legitimately has multiple files per Title ID.
    SelectXeniaCandidate(usize),
    ClearXeniaCandidateChoice,
    /// The explicit expert override required before a partially verified
    /// (module-hash-unverified) candidate can ever be staged.
    AcknowledgeXeniaPartialVerification(bool),
    ToggleXeniaPatchSelected {
        index: usize,
        selected: bool,
    },
    SelectAllXeniaPatches,
    ClearAllXeniaPatches,
    BuildXeniaInstallPreview,
    /// The beginner profile chooser selects and remembers one candidate in
    /// the same click. Choosing a profile is not a destructive operation.
    ChooseDolphinProfile(String),
    ChooseXeniaProfile(String),
    /// One click: builds the install preview and moves straight to the
    /// review stage, so the beginner "Install selected" button never
    /// requires a separate technical Preview step first.
    InstallSelectedDolphin,
    InstallSelectedXenia,
    ToggleDolphinShowExactChanges(bool),
    ToggleXeniaShowExactChanges(bool),
    ToggleDolphinDetailsOpen(bool),
    ToggleXeniaDetailsOpen(bool),
    /// GameCube-only GameHacking.org coverage: matches against the cached
    /// catalogue and, once matched, downloads only that one game's cheats.
    FetchGameCubeGameHacking {
        force_refresh: bool,
    },
    ConfirmGameCubeGameHackingMatch {
        game_id: u64,
    },
    ToggleGameCubeGameHackingCheatSelected {
        index: usize,
        selected: bool,
    },
    InstallSelectedGameCubeGameHacking,
    RemoveSelectedGameCubeGameHacking,
    OpenBrowserImport(BrowserImportPlatform),
    CloseBrowserImport,
    OpenGameHackingPageInBrowser,
    CopyGameHackingPageUrl,
    ImportBrowserSavedFile,
    ToggleBrowserImportPaste(bool),
    ImportBrowserPastedText,
    ImportBrowserClipboard,
    ChooseBrowserImportKind(Option<BrowserImportKind>),
    /// BSFree Archive GameCube coverage: search the optional local SQLite
    /// database for the selected archive's game (bounded, read-only).
    FetchBsFreeGameCube {
        search_title: String,
    },
    /// Confirm one of several BSFree GameCube search candidates and load its
    /// classified cheats.
    ConfirmBsFreeGameCubeMatch {
        upstream_uid: i64,
    },
    ToggleBsFreeGameCubeCheatSelected {
        index: usize,
        selected: bool,
    },
    SelectAllBsFreeGameCubeCheats,
    ClearAllBsFreeGameCubeCheats,
    InstallSelectedBsFreeGameCube,
    /// BSFree Archive Wii coverage: search the optional local SQLite database
    /// for the selected archive's Wii game (bounded, read-only).
    FetchBsFreeWii {
        search_title: String,
    },
    /// Confirm one of several BSFree Wii search candidates and load its
    /// classified cheats.
    ConfirmBsFreeWiiMatch {
        upstream_uid: i64,
    },
    ToggleBsFreeWiiCheatSelected {
        index: usize,
        selected: bool,
    },
    SelectAllBsFreeWiiCheats,
    ClearAllBsFreeWiiCheats,
    InstallSelectedBsFreeWii,
}

pub(crate) const MODS_UNAVAILABLE_BODY: &str = "This workspace is reserved for future verified emulator-specific adapters, including patches, texture packs, widescreen fixes, and frame-rate patches. No mod workflow is available yet.";
pub(crate) const LOCAL_INSPECTION_PRIVACY_COPY: &str = "Trusted catalogue archives are validated locally on this device for unsafe paths, special entries, resource-limit violations, and unexpected structure. Scan results, filenames, file contents, hashes, and metadata are not sent to the EmuWiz developers or any third party. General local or community-source inspection is planned and is not active yet.";
pub(crate) const IMPORT_CONSENT_COPY: &str = "Only import cheats or mods from sources you trust. EmuWiz performs local structural and format checks where an implemented adapter provides them, but it is not an antivirus scanner.";
pub(crate) const ETHICAL_USE_COPY: &str = "EmuWiz is intended for preservation, accessibility, personal customization, and legitimate interoperability. It must not be used to bypass copy protection, licensing systems, access controls, or other technical protections. Game developers, artists, musicians, writers, testers, and publishers invest substantial effort in creating games; supporting legitimate releases helps future games, updates, and preservation efforts.";
pub(crate) const USER_RESPONSIBILITY_COPY: &str = "You are responsible for ensuring that you have the right to use, modify, import, and distribute cheats, patches, mods, textures, or related files. EmuWiz does not verify ownership or licensing.";
pub(crate) const SCANNING_DISABLED_WARNING: &str =
    "Turning this off does not make unsafe files safe. It only stops EmuWiz checking them.";

