//! GUI Maintenance Batch 2: main.rs's test suite, relocated out of the
//! production file and split by feature/domain.
//!
//! # Why this split is approximate, not a strict domain boundary
//!
//! The original `#[cfg(test)] mod tests { ... }` in main.rs interleaved
//! tests for unrelated features throughout its ~30,000 lines - a platform
//! shelf test could sit next to a Doctor test, next to a mount test. This
//! split was done at *safe item boundaries* within that existing order
//! (never splitting a function/struct/comment mid-body), grouping
//! contiguous runs of items into files named after their predominant theme.
//! It shrinks main.rs and avoids one more giant single file, but a given
//! domain file may still contain a handful of tests for other features that
//! happened to sit nearby in the original source. Every test was copied
//! byte-for-byte from its original location - nothing was rewritten,
//! renamed, deleted, or had its assertions changed.
//!
//! Two exceptions ARE strict single-topic modules, because they were
//! already self-contained in the original file:
//! - [`selected`] - `gather_selected_evidence_with_registry(_at)` wiring
//!   tests (GUI Batch A), was already its own nested `mod` in the original.
//! - [`transactions`] - `romm_dispatch_tests`, was already a separate
//!   top-level `#[cfg(test)] mod` alongside the main one in the original.
//!
//! # What lives directly in this file
//!
//! The shared imports and test-only helpers/fixtures that main.rs's own
//! top-level scope carried (only ever used by `#[cfg(test)]` code, never by
//! production code) - relocated here unchanged so every domain file below
//! can reach them the same way the original `mod tests { use super::*; ... }`
//! did, via its own `use super::*;`.
//!
//! # Known remaining gap
//!
//! A handful of bare top-level `#[test] fn` items remain in main.rs itself,
//! interleaved with real production functions near the end of the file
//! (after `mod romm_dispatch_tests`'s original location) - e.g. the
//! artwork-picker tests around `picker_output_text_contains`. Moving those
//! safely requires separating individual functions out of a region that
//! alternates between test and production code line-by-line, which this
//! batch treats as a distinct, higher-risk seam and leaves for a follow-up
//! pass rather than risk misplacing a production function.

#[cfg(test)]
use archivefs_core::game_identity::inspect_game_identity;

#[cfg(test)]
use archivefs_core::patch_manager::{
    EmulatorDestinationDirectories, EmulatorInstallationType, EmulatorProfileConfidence,
    ResolvedEmulatorProfile,
};

#[cfg(test)]
use archivefs_core::patch_manager::{
    DOLPHIN_CATALOGUE_REPOSITORY, DOLPHIN_CATALOGUE_SCHEMA_VERSION, DolphinCatalogueGame,
    DolphinCatalogueMetadata, GeckoProviderEntry, GeckoProviderResult, GeckoRegion,
    GeckoRevisionApplicability, Pcsx2PatchDirectory, Pcsx2ProfileBlocker, Pcsx2ProfileBlockerKind,
    XeniaInstallationType, XeniaProfileScope, XeniaProviderDocument, XeniaProviderResult,
};

use super::*;
use archivefs_core::emulator_environment::EncodedPath;
use archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport;
use archivefs_core::patch_manager::{
    CHEAT_SOURCE_RESULT_SCHEMA_VERSION, CheatSourceManifest, RETROARCH_CHEAT_SETUP_SCHEMA_VERSION,
    RetroArchCheatSetupProfile, RetroArchCheatSetupProfileBlocker, RetroArchCheatSetupProfileState,
    SharedApplyContext, SharedApplyEntry, SharedApplyJournal, SharedApplyOutcome, SharedPlanEntry,
    SharedTransactionPath, SharedTransactionStage, trusted_retroarch_cheat_sources,
};
use archivefs_core::{
    Archive, ArchiveHealth, ArchiveMetadata, DoctorCheck, LibraryViewPlanCounts, MountPlan,
    SetupDiagnostic, SourceScanStatus, classify_entry,
};
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

/// Relocated from an inline `#[cfg(test)] fn from_config` method that
/// originally lived inside `impl GuiConfigSnapshot` in main.rs, alongside
/// its real (non-test) methods. Split into its own `impl` block here rather
/// than left inline, since Rust allows more than one inherent `impl` for a
/// type across files in the same crate, and `GuiConfigSnapshot`'s private
/// fields remain visible here (this module is a descendant of the crate
/// root that defines them, the same visibility rule that already let the
/// original `mod tests { use super::*; ... }` reach them). Behavior is
/// unchanged from the original method.
#[cfg(test)]
impl GuiConfigSnapshot {
    fn from_config(config: Config) -> Self {
        Self {
            current: Some(config),
            last_error: None,
            load_attempts: 0,
            loader: hermetic_gui_config,
        }
    }
}

#[cfg(test)]
fn hermetic_gui_config() -> Result<Config, String> {
    Ok(Config {
        source_folders: vec![PathBuf::from("/library")],
        mount_root: PathBuf::from("/mount"),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    })
}

/// Bundled artwork that is still fully opaque, pending visual cleanup.
///
/// The house style is transparent RGBA: artwork is drawn over a panel
/// background, so an opaque square renders as a visible tile and does not adapt
/// between light and dark themes. Every image here was imported for its
/// *identity* - it puts the right logo on the right platform - and its
/// background has deliberately not been machine-stripped, because automatic
/// background removal damages artwork more often than it helps.
///
/// This list is the review queue, and
/// `bundled_registry_is_complete_unique_and_decodable_without_filesystem_paths`
/// keeps it honest in both directions: an entry that gains transparency must be
/// removed from it, and a new opaque asset cannot be bundled without being
/// added. It is expected to shrink to nothing.
#[cfg(test)]
const OPAQUE_ARTWORK_PENDING_VISUAL_REVIEW: &[&str] = &[
    "3do",
    "acornelectron",
    "amigacd32",
    "amstradcpc",
    "appleii",
    "arcade",
    "atari2600",
    "atari5200",
    "atari7800",
    "atarijaguar",
    "atarilynx",
    "atarist",
    "bbcmicro",
    "colecovision",
    "commodore64",
    "gameboyadvance",
    "gameboycolor",
    "gamegear",
    "neogeo",
    "neogeopocket",
    "nes",
    "nintendo3ds",
    "philipscdi",
    "playstationvita",
    "psp",
    "scummvm",
    "sega32x",
    "sharpx68000",
    "turbografx16",
    "vic20",
    "virtualboy",
    "wonderswancolor",
    "zxspectrum",
];

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlatformPngWarning {
    LegacySourceDimensions { width: u32, height: u32 },
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct PlatformPngInspection {
    width: u32,
    height: u32,
    color_type: image::ColorType,
    has_transparent_pixel: bool,
    warnings: Vec<PlatformPngWarning>,
}

/// Validation used by the artwork audit tests. A legacy square image remains
/// usable and produces a warning; malformed, animated, zero-sized, non-square
/// or absurdly large images are refused. Production decoding independently
/// retains its tighter 1024x1024 allocation ceiling.
#[cfg(test)]
fn inspect_platform_png(bytes: &[u8]) -> Result<PlatformPngInspection, &'static str> {
    use image::ImageDecoder as _;

    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("invalid PNG signature");
    }
    let mut offset = 8usize;
    while offset < bytes.len() {
        let length_bytes = bytes.get(offset..offset + 4).ok_or("truncated PNG chunk")?;
        let length = u32::from_be_bytes(
            length_bytes
                .try_into()
                .map_err(|_| "truncated PNG chunk length")?,
        ) as usize;
        let kind = bytes
            .get(offset + 4..offset + 8)
            .ok_or("truncated PNG chunk type")?;
        if kind == b"acTL" {
            return Err("animated PNG artwork is unsupported");
        }
        offset = offset
            .checked_add(12)
            .and_then(|value| value.checked_add(length))
            .ok_or("PNG chunk offset overflow")?;
        if offset > bytes.len() {
            return Err("PNG chunk extends beyond the file");
        }
        if kind == b"IEND" {
            break;
        }
    }

    let decoder = image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|_| "malformed PNG")?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err("zero-sized PNG");
    }
    if width > 8192 || height > 8192 {
        return Err("absurd PNG dimensions");
    }
    if width != height {
        return Err("platform artwork must be square");
    }
    let color_type = decoder.color_type();
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| "malformed PNG pixels")?
        .into_rgba8();
    let has_transparent_pixel = rgba.pixels().any(|pixel| pixel.0[3] < u8::MAX);
    let warnings = (width != 1024 || height != 1024)
        .then_some(PlatformPngWarning::LegacySourceDimensions { width, height })
        .into_iter()
        .collect();
    Ok(PlatformPngInspection {
        width,
        height,
        color_type,
        has_transparent_pixel,
        warnings,
    })
}

/// Layout queries used to express the shelf's visual invariants. Test-only:
/// nothing in the rendering path needs them, and the invariants they check are
/// what the regression test asserts.
#[cfg(test)]
impl ShelfGeometry {
    /// The rightmost card that is actually visible inside the strip.
    fn last_visible_card(&self) -> Option<egui::Rect> {
        self.cards
            .iter()
            .filter(|card| card.left() < self.strip.right() && card.right() > self.strip.left())
            .copied()
            .reduce(|left, right| {
                if right.right() > left.right() {
                    right
                } else {
                    left
                }
            })
    }

    fn chevron_height(&self) -> f32 {
        self.previous
            .into_iter()
            .chain(self.next)
            .map(|rect| rect.height())
            .fold(0.0_f32, f32::max)
    }
}

mod cheats_mods_workflows;
mod database_and_catalogue;
mod doctor_and_repair;
mod emulator_profiles_and_setup;
mod health_and_platform_actions;
mod library_views_and_sources;
mod mounts_and_history;
mod platform_shelf_and_library_shell;
mod selected;
mod selected_context_and_bsfree;
mod selected_page_mount_removal;
mod transactions;

// -- Hoisted shared test fixtures ------------------------------------
//
// The functions/types below were originally defined once at the top
// level of the single inline `mod tests { ... }` in main.rs and used by
// tests that landed in more than one of the domain files above after
// the split. Moved here (their only home now) so every domain file
// reaches them the same way, via `use super::*;` - unchanged bodies,
// just relocated.

/// A deterministic, in-memory stand-in for `NativeClipboard` - the
/// injectable clipboard every context-menu test uses instead of the
/// real OS clipboard, so assertions never depend on (or pollute) the
/// machine actually running the tests. Can independently simulate
/// every case `ClipboardTextStatus` distinguishes: usable text,
/// confirmed-empty, and a broken backend (`unavailable`) - plus a
/// failing write (`set_error`), to exercise "backend failure produces
/// a diagnostic and no mutation".
#[derive(Default)]
struct InMemoryClipboard {
    text: Option<String>,
    unavailable: Option<String>,
    set_error: Option<String>,
    set_calls: Vec<String>,
}

impl InMemoryClipboard {
    fn with_text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            unavailable: Some(reason.into()),
            ..Self::default()
        }
    }

    fn failing_to_write(reason: impl Into<String>) -> Self {
        Self {
            set_error: Some(reason.into()),
            ..Self::default()
        }
    }

    /// Test-only: the text from the most recent *successful*
    /// `set_text` call - i.e. what Copy/Cut actually wrote. Distinct
    /// from `get_text_status`, which is what a following Paste would
    /// read back (the same value, once written - real clipboards
    /// read back what was last written too).
    fn copied_text(&self) -> Option<String> {
        self.set_calls.last().cloned()
    }
}

fn row(search_text: &str) -> ArchiveRow {
    ArchiveRow {
        path: PathBuf::new(),
        archive_path: String::new(),
        mount_path: String::new(),
        platform: String::new(),
        state: String::new(),
        search_text: search_text.to_lowercase(),
        origin: RowOrigin::Live,
        unknown_platform: false,
        source_path: None,
    }
}

fn record(archive_path: &str, mount_state: MountState) -> ArchiveRecord {
    let archive = Archive::from_path(archive_path).unwrap();
    ArchiveRecord::new(
        MountPlan::new(archive, PathBuf::from("/mnt/archivefs/Test")),
        mount_state,
        ArchiveMetadata {
            title: None,
            platform: None,
            region: None,
            languages: None,
            version: None,
            disc: None,
            publisher: None,
            developer: None,
            release_year: None,
            genre: None,
            notes: None,
            source: None,
            synopsis: None,
            players: None,
            rating: None,
        },
        ArchiveHealth::Pending,
    )
}

fn loose_mega_drive_record(path: &str) -> ArchiveRecord {
    let archive = Archive::from_path_in_root(path, "/roms").unwrap();
    ArchiveRecord::new(
        MountPlan::new(archive, PathBuf::from("/mnt/archivefs/Alien_3")),
        MountState::NotMountable,
        ArchiveMetadata {
            title: None,
            platform: Some("MegaDrive".to_string()),
            region: None,
            languages: None,
            version: None,
            disc: None,
            publisher: None,
            developer: None,
            release_year: None,
            genre: None,
            notes: None,
            source: None,
            synopsis: None,
            players: None,
            rating: None,
        },
        ArchiveHealth::Unsupported,
    )
}

fn cheat_profile(profile_id: &str, eligible: bool) -> RetroArchCheatSetupProfile {
    RetroArchCheatSetupProfile {
        profile_id: profile_id.to_string(),
        installation_type: ProfileKind::Native,
        scope: ProfileScope::User,
        state: if eligible {
            RetroArchCheatSetupProfileState::Eligible
        } else {
            RetroArchCheatSetupProfileState::Ineligible
        },
        eligible,
        executable_evidence: Vec::new(),
        configuration_path: EncodedPath::from_path(Path::new("/isolated/retroarch.cfg")),
        cheat_destination_root: eligible
            .then(|| EncodedPath::from_path(Path::new("/isolated/cheats"))),
        blockers: if eligible {
            Vec::new()
        } else {
            vec![RetroArchCheatSetupProfileBlocker {
                code: "cheats_destination_unresolved".to_string(),
                detail: "no resolved cheats destination".to_string(),
            }]
        },
        diagnostics: Vec::new(),
    }
}

fn cheat_discovery(profiles: Vec<RetroArchCheatSetupProfile>) -> RetroArchCheatSetupDiscovery {
    RetroArchCheatSetupDiscovery {
        schema_version: RETROARCH_CHEAT_SETUP_SCHEMA_VERSION,
        profiles,
        diagnostics: Vec::new(),
        environment: RetroArchEnvironmentReport {
            format_version: 1,
            profiles: Vec::new(),
            diagnostics: Vec::new(),
        },
    }
}

/// A fetch-result fixture built on a *real* trusted definition
/// (`CheatSourceArchiveType` is not exported, so the definition
/// cannot be constructed from scratch) with only its ID overridden.
/// No network and no real cache paths are involved.
fn cheat_fetch_result_for(
    source_id: &str,
    status: CheatSourceFetchStatus,
) -> CheatSourceFetchResult {
    let mut source = trusted_retroarch_cheat_sources()
        .into_iter()
        .next()
        .expect("a built-in trusted source exists");
    source.source_id = source_id.to_string();
    CheatSourceFetchResult {
        schema_version: CHEAT_SOURCE_RESULT_SCHEMA_VERSION,
        status,
        source,
        local_catalogue_path: EncodedPath::from_path(Path::new("/isolated/catalogue.json")),
        immutable_snapshot_path: EncodedPath::from_path(Path::new("/isolated/snapshot")),
        manifest: cheat_manifest(source_id),
        freshness: CheatSourceFreshness::Fresh,
        from_cache: status != CheatSourceFetchStatus::Fetched,
        stale: false,
        warnings: Vec::new(),
    }
}

fn cheat_source_list_fixture() -> (String, String, CheatSourceList) {
    let source = trusted_retroarch_cheat_sources()
        .into_iter()
        .next()
        .expect("a built-in trusted source exists");
    let source_id = source.source_id.clone();
    (
        source_id,
        source.display_name.clone(),
        CheatSourceList {
            schema_version: CHEAT_SOURCE_RESULT_SCHEMA_VERSION,
            entries: vec![CheatSourceListEntry {
                source,
                trust_status: "built_in_reviewed".to_string(),
                freshness: CheatSourceFreshness::Fresh,
                current_cached_version: Some("fixture".to_string()),
                fetched_at_unix_seconds: Some(0),
                archive_sha256: Some("fixture-digest".to_string()),
                catalogue_file_count: Some(4),
                indexed_file_count: Some(4),
                excluded_file_count: Some(0),
                exclusion_examples: Vec::new(),
                setup_usable: true,
                status: archivefs_core::patch_manager::CheatCatalogueStatus::Ready,
                total_bytes: Some(0),
                last_error: None,
                last_error_at_unix_seconds: None,
                warnings: Vec::new(),
            }],
        },
    )
}

/// Builds a workflow that has already reached stage 6: a candidate
/// list, a chosen candidate, and a cheat selection, so the tests below
/// can assert what survives a context change and what does not.
fn workflow_at_cheat_selection_stage(app: &mut ArchiveFsApp) {
    let document = archivefs_core::patch_manager::parse_cht_text(
        "cheats = 2\ncheat0_desc = \"A\"\ncheat0_code = \"AA\"\n\
             cheat1_desc = \"B\"\ncheat1_code = \"BB\"\n",
    )
    .expect("fixture parses");
    let candidate = CheatCandidate {
        catalogue_relative_path: "NES/a.cht".to_string(),
        display_name: "a".to_string(),
        platform: Some("NES".to_string()),
        region: None,
        revision: None,
        classification: CheatCandidateClassification::Strong,
        confidence_score: 700,
        evidence: Vec::new(),
        cheat_count: 2,
        source_file_hash: None,
        auto_selectable: false,
        manually_selectable: true,
    };
    let mut selection = CheatSelection::from_document(&document);
    assert!(selection.set_selected(0, true));
    let key = cheat_preview_key(app.cheat_workflow.as_ref().expect("workflow"));
    let workflow = app.cheat_workflow.as_mut().expect("workflow");
    workflow.candidates_request = Some(key.clone());
    workflow.candidates = CheatStepResource::Ready(CheatCandidateStage {
        key,
        catalogue_root: PathBuf::from("/catalogue"),
        list: CheatCandidateList {
            candidates: vec![candidate.clone()],
            total_matched: 1,
            truncated: false,
            query: None,
            records_scanned: 1,
            scan_limit_reached: false,
        },
    });
    workflow.candidate_selection = Some(CheatCandidateSelection {
        candidate,
        loaded: LoadedCandidate {
            absolute_path: PathBuf::from("/catalogue/NES/a.cht"),
            digest: "a".repeat(64),
            document,
        },
        selection,
    });
}

fn run_settle_frames(
    ctx: &egui::Context,
    app: &mut ArchiveFsApp,
    frame: &mut eframe::Frame,
    base_input: &egui::RawInput,
    count: usize,
) {
    for _ in 0..count {
        let _ = ctx.run(base_input.clone(), |ctx| app.update(ctx, frame));
    }
}

fn app_with_cheats_mods_context() -> ArchiveFsApp {
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/a.zip", MountState::Pending));
    }
    app.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    app.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    app.retroarch_profiles =
        RetroArchProfilesState::Ready(cheat_discovery(vec![cheat_profile("native-user", true)]));
    app.cheat_workflow = Some(CheatWorkflowState {
        archive_path: PathBuf::from("/roms/a.zip"),
        display_name: "a".to_string(),
        normalized_name: "a".to_string(),
        platform: None,
        region: None,
        source_root: PathBuf::from("/roms"),
        size_bytes: None,
        adapter: CheatEmulatorAdapter::RetroArch,
        identity_request: None,
        identity: CheatStepResource::NotLoaded,
        preview_request: None,
        preview: CheatStepResource::NotLoaded,
        transaction: CheatTransactionState::Idle,
        transaction_notice: None,
        selected_profile_id: Some("native-user".to_string()),
        selected_pcsx2_profile_id: None,
        pcsx2_inventory_profile_id: None,
        pcsx2_inventory: CheatStepResource::NotLoaded,
        pcsx2_gamehacking: CheatStepResource::NotLoaded,
        gamecube_gamehacking: CheatStepResource::NotLoaded,
        gamecube_gamehacking_request: None,
        gamecube_gamehacking_cancellation: None,
        gamecube_gamehacking_generation: 0,
        gamecube_gamehacking_blocked: false,
        bsfree_gamecube: CheatStepResource::NotLoaded,
        bsfree_gamecube_cancellation: None,
        bsfree_gamecube_generation: 0,
        bsfree_wii: CheatStepResource::NotLoaded,
        bsfree_wii_cancellation: None,
        bsfree_wii_generation: 0,
        selected_dolphin_profile_id: None,
        dolphin_explicit_root: String::new(),
        dolphin_inventory_profile_id: None,
        dolphin_inventory: CheatStepResource::NotLoaded,
        dolphin_provider_request: None,
        dolphin_provider: CheatStepResource::NotLoaded,
        dolphin_provider_selection: None,
        dolphin_destination_error: None,
        dolphin_local_lookup: DolphinLocalLookupState::NotAttempted,
        dolphin_profile_selection: None,
        dolphin_profile_choice: None,
        dolphin_details_open: false,
        dolphin_show_exact_changes: false,
        selected_xenia_profile_id: None,
        xenia_explicit_root: String::new(),
        xenia_provider_request: None,
        xenia_provider: CheatStepResource::NotLoaded,
        xenia_selected_candidate_index: None,
        xenia_selection: None,
        xenia_destination_error: None,
        xenia_profile_selection: None,
        xenia_profile_choice: None,
        xenia_details_open: false,
        xenia_show_exact_changes: false,
        source_mode: CheatSourceMode::ArchiveFsTrustedCatalogue,
        existing_library_profile_id: None,
        existing_library: CheatStepResource::NotLoaded,
        source_list: CheatStepResource::NotLoaded,
        source_fetch: CheatStepResource::NotLoaded,
        selected_source_id: Some("source-a".to_string()),
        fetch_force_refresh: false,
        candidates: CheatStepResource::NotLoaded,
        candidates_request: None,
        candidate_query: String::new(),
        candidate_selection: None,
        candidate_load_error: None,
    });
    app.view = MainView::CheatsMods;
    app.tools_overlay = ToolsOverlay::None;
    app
}

fn pcsx2_profile_fixture() -> Pcsx2Profile {
    Pcsx2Profile {
        profile_id: "pcsx2-native-test".to_string(),
        installation_type: Pcsx2InstallationType::Native,
        scope: Pcsx2ProfileScope::User,
        configuration_path: PathBuf::from("/isolated/PCSX2"),
        provenance: "test fixture",
        eligible: true,
        blockers: Vec::new(),
        patch_directories: vec![
            Pcsx2PatchDirectory {
                path: PathBuf::from("/isolated/PCSX2/cheats"),
                category: Pcsx2PatchCategory::Cheats,
                state: Pcsx2PatchDirectoryState::Available,
                warning: None,
                identity: None,
            },
            Pcsx2PatchDirectory {
                path: PathBuf::from("/isolated/PCSX2/cheats_ws"),
                category: Pcsx2PatchCategory::WidescreenPatches,
                state: Pcsx2PatchDirectoryState::Missing,
                warning: None,
                identity: None,
            },
        ],
        configuration_identity: None,
        executable_candidates: Vec::new(),
    }
}

fn empty_pcsx2_inventory() -> Pcsx2PnachInventory {
    Pcsx2PnachInventory {
        profile_id: "pcsx2-native-test".to_string(),
        files: Vec::new(),
        warnings: Vec::new(),
        directories_traversed: 2,
        entries_visited: 0,
        bytes_inspected: 0,
        complete: true,
    }
}

fn dolphin_profile_fixture() -> DolphinProfile {
    DolphinProfile {
        profile_id: "dolphin-native-test".to_string(),
        installation_type: DolphinInstallationType::Native,
        scope: DolphinProfileScope::User,
        configuration_path: PathBuf::from("/isolated/dolphin-emu"),
        provenance: "test fixture".to_string(),
        eligible: true,
        blockers: Vec::new(),
        game_settings_path: PathBuf::from("/isolated/dolphin-emu/GameSettings"),
        game_settings_state: DolphinSettingsDirectoryState::Available,
        game_settings_warning: None,
        configuration_identity: None,
        game_settings_identity: None,
        resolved: ResolvedEmulatorProfile {
            emulator_executable: None,
            installation_type: EmulatorInstallationType::NativeSystem,
            configuration_root: PathBuf::from("/isolated/dolphin-emu"),
            data_user_root: PathBuf::from("/isolated/dolphin-emu"),
            active_explicit_profile: None,
            destinations: EmulatorDestinationDirectories {
                game_settings: Some(PathBuf::from("/isolated/dolphin-emu/GameSettings")),
                ..EmulatorDestinationDirectories::default()
            },
            discovery_evidence: vec!["test fixture".to_string()],
            confidence: EmulatorProfileConfidence::KnownPath,
            priority: 100,
            writable: true,
        },
    }
}

fn dolphin_workflow_with_matched_identity(
    directory: &std::path::Path,
    game_id: &str,
) -> ArchiveFsApp {
    let mut app = app_with_cheats_mods_context();
    let profile = DolphinProfile {
        configuration_path: directory.to_path_buf(),
        game_settings_path: directory.join("GameSettings"),
        game_settings_state: DolphinSettingsDirectoryState::Missing,
        ..dolphin_profile_fixture()
    };
    app.dolphin_profiles = DolphinProfilesState::Ready(DolphinProfileDiscovery {
        profiles: vec![profile],
        warnings: Vec::new(),
        complete: true,
    });
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("GameCube".to_string());
    workflow.adapter = CheatEmulatorAdapter::Dolphin;
    workflow.selected_dolphin_profile_id = Some("dolphin-native-test".to_string());
    workflow.identity_request = Some(GameIdentityRequest {
        archive_path: workflow.archive_path.clone(),
        platform: workflow.platform.clone(),
        adapter: CheatEmulatorAdapter::Dolphin,
    });
    let report = GameIdentityReport {
        archive_path: workflow.archive_path.clone(),
        platform: archivefs_core::game_identity::IdentityPlatform::GameCube,
        format: IdentityImageFormat::Iso,
        evidence: vec![
            archivefs_core::game_identity::IdentityEvidence {
                kind: IdentityKind::DolphinGameId,
                status: IdentityStatus::Verified,
                value: Some(game_id.to_string()),
                confidence: archivefs_core::game_identity::IdentityConfidence::ExactBytes,
                provenance: archivefs_core::game_identity::IdentityProvenance {
                    archive_path: workflow.archive_path.clone(),
                    member_path: None,
                    member_index: None,
                    method: "test fixture disc header read".to_string(),
                },
                diagnostic: "test fixture".to_string(),
            },
            archivefs_core::game_identity::IdentityEvidence {
                kind: IdentityKind::DolphinRevision,
                status: IdentityStatus::Verified,
                value: Some("0".to_string()),
                confidence: archivefs_core::game_identity::IdentityConfidence::ExactBytes,
                provenance: archivefs_core::game_identity::IdentityProvenance {
                    archive_path: workflow.archive_path.clone(),
                    member_path: None,
                    member_index: None,
                    method: "test fixture disc header read".to_string(),
                },
                diagnostic: "test fixture".to_string(),
            },
        ],
        warnings: Vec::new(),
        bytes_read: 512,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    workflow.identity =
        CheatStepResource::Ready((workflow.identity_request.clone().unwrap(), report));
    app
}

fn wii_workflow_with_matched_identity(directory: &std::path::Path, game_id: &str) -> ArchiveFsApp {
    let mut app = dolphin_workflow_with_matched_identity(directory, game_id);
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("Wii".to_string());
    workflow.identity_request.as_mut().unwrap().platform = workflow.platform.clone();
    let CheatStepResource::Ready((request, report)) = &mut workflow.identity else {
        unreachable!()
    };
    request.platform = workflow.platform.clone();
    report.platform = archivefs_core::game_identity::IdentityPlatform::Wii;
    for evidence in &mut report.evidence {
        if evidence.kind == IdentityKind::DolphinRevision {
            evidence.status = IdentityStatus::Candidate;
            evidence.confidence =
                archivefs_core::game_identity::IdentityConfidence::StructuredMetadata;
            evidence.diagnostic = "outer Wii header revision is non-authoritative".to_string();
        }
    }
    app
}

fn gafe01_not_available_fetch() -> GeckoProviderFetchResult {
    GeckoProviderFetchResult {
            result: GeckoProviderResult {
                provider_id: "dolphin_upstream_gamesettings".to_string(),
                provider_display_name: "Dolphin upstream GameSettings".to_string(),
                source_identity: "https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Data/Sys/GameSettings/GAFE01.ini".to_string(),
                retrieved_at_unix_seconds: 1,
                game_id: "GAFE01".to_string(),
                title: None,
                region: GeckoRegion::Usa,
                revision: 0,
                entries: Vec::new(),
                warnings: Vec::new(),
                attribution: "Gecko definitions from Dolphin upstream.".to_string(),
                license: "GPL-2.0-or-later".to_string(),
            },
            status: GeckoProviderFetchStatus::NotAvailable,
            refresh_error: None,
        }
}

fn install_provider_fixture(app: &mut ArchiveFsApp, configuration_path: &Path) {
    let fetch = gafe01_provider_fetch();
    let destination = load_dolphin_destination(configuration_path, "GAFE01").unwrap();
    let selection = DolphinProviderCodeSelection::from_provider(&fetch.result, &destination);
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.dolphin_provider_request = Some(DolphinProviderRequestKey {
        archive_path: workflow.archive_path.clone(),
        game_id: "GAFE01".to_string(),
        revision: 0,
    });
    workflow.dolphin_provider = CheatStepResource::Ready(fetch);
    workflow.dolphin_provider_selection = Some(DolphinProviderSelectionState {
        destination,
        selection,
    });
}

/// Everything the beginner Dolphin view needs to show the compatible
/// checklist and offer "Install selected" - the exact "select the
/// game, compatible cheats appear automatically" state the milestone
/// describes with its Animal Crossing/16:9 Widescreen example.
fn dolphin_workflow_ready_for_beginner_install(temp: &Path) -> ArchiveFsApp {
    let mut app = dolphin_workflow_with_matched_identity(temp, "GAFE01");
    install_provider_fixture(&mut app, temp);
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: "dolphin-native-test".to_string(),
        reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::OnlyValidProfile,
    });
    app
}

fn render_dolphin_workflow(app: &mut ArchiveFsApp) -> egui::FullOutput {
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let workflow = app.cheat_workflow.as_mut().unwrap();
            let _ = show_dolphin_workflow(ui, workflow, &app.dolphin_profiles, &mut clipboard);
        });
    })
}

/// A minimal but fully valid successful `SharedApplyResult` fixture -
/// only the fields the beginner Result view reads
/// (`journal.status`, `journal_path`) need real content; everything
/// else just needs to be a well-typed, self-consistent value.
fn successful_shared_apply_result() -> SharedApplyResult {
    let archive_path = SharedTransactionPath::from_path(Path::new("/roms/a.zip"));
    let destination_root = SharedTransactionPath::from_path(Path::new("/dolphin/GameSettings"));
    let source_path = SharedTransactionPath::from_path(Path::new("/staging/GAFE01.ini"));
    let destination_relative_path = SharedTransactionPath::from_path(Path::new("GAFE01.ini"));
    let context = SharedApplyContext {
        adapter: PreviewAdapter::Dolphin,
        selected_archive: archive_path.clone(),
        verified_game_identity: "GAFE01".to_string(),
        profile_id: "dolphin-native-test".to_string(),
        source_mode: "EmuWiz trusted catalogue".to_string(),
    };
    let plan_entry = SharedPlanEntry {
        adapter: PreviewAdapter::Dolphin,
        selected_archive: archive_path,
        verified_game_identity: "GAFE01".to_string(),
        source_path: source_path.clone(),
        source_digest: "a".repeat(64),
        destination_root: destination_root.clone(),
        destination_relative_path,
        destination_pre_state: PreviewDestinationState::Missing,
        destination_pre_digest: None,
        proposed_action: archivefs_core::patch_manager::PreviewProposedAction::Install,
        backup_required: false,
        parent_creation_approved: true,
        content_verification: None,
    };
    let entry = SharedApplyEntry {
        plan_entry,
        destination_existed_before_apply: Some(false),
        destination_parent_existed_before_apply: Some(true),
        observed_source_digest: Some("a".repeat(64)),
        observed_destination_digest: None,
        backup_path: None,
        backup_digest: None,
        temporary_path: None,
        final_destination_digest: Some("a".repeat(64)),
        created_directories: Vec::new(),
        replacement_approved: false,
        verification_succeeded: true,
        outcome: SharedApplyOutcome::InstalledNew,
        stages: vec![SharedTransactionStage::Success],
        warnings: Vec::new(),
        failures: Vec::new(),
    };
    let journal = SharedApplyJournal {
        schema_version: 1,
        operation_id: "op-beginner-test".to_string(),
        plan_id: "plan-beginner-test".to_string(),
        timestamp_unix_seconds: 0,
        context,
        approved_source_root: source_path,
        destination_root,
        created_root_directories: Vec::new(),
        dry_run: false,
        entries: vec![entry],
        status: SharedApplyStatus::Success,
        rollback_operation_id: None,
    };
    SharedApplyResult {
        journal,
        journal_path: Some(PathBuf::from("/history/op-beginner-test.json")),
        journal_failure: None,
    }
}

fn default_config_identity() -> ConfigIdentity {
    ConfigIdentity {
        config_path: Some(PathBuf::from("/config/archivefs.toml")),
        content_digest: Some([1; 32]),
    }
}

// ---------------------------------------------------------------------
// Human-smoke regressions: Gamer View platform filtering
//
// Confirmed on a real 13,891-archive library: after ticking a state
// filter in Advanced View (or opening the Health dashboard's "Review
// missing", which sets `missing` directly) and returning to Gamer View,
// every platform card still showed its full count while the list said
// "No games match the selected platform." for all of them. Gamer View
// shows no such checkbox, so there was no way back except a restart -
// `LibraryRowFilters` is not persisted, which is exactly why restarting
// appeared to "fix" it.
// ---------------------------------------------------------------------

/// A pointer click at one position, as one frame of input.
fn click_at(screen: egui::Rect, position: egui::Pos2) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(screen),
        events: vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ],
        ..Default::default()
    }
}

fn gamer_app_with_platforms(platforms: &[(&str, usize)]) -> ArchiveFsApp {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    let mut records = Vec::new();
    let mut index = 0usize;
    for (platform, count) in platforms {
        for _ in 0..*count {
            let mut row = record(&format!("/roms/g{index:04}.zip"), MountState::Pending);
            row.metadata.platform = Some((*platform).to_string());
            row.metadata.title = Some(format!("Title{index:04}"));
            records.push(row);
            index += 1;
        }
    }
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));
    app
}

fn gamer_screen() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0))
}

/// Runs `frames` settling frames and returns the last output.
fn run_gamer_frames(
    app: &mut ArchiveFsApp,
    ctx: &egui::Context,
    input: egui::RawInput,
    frames: usize,
) -> egui::FullOutput {
    let mut frame = eframe::Frame::_new_kittest();
    let mut output = None;
    for _ in 0..frames {
        output = Some(ctx.run(input.clone(), |ctx| app.update(ctx, &mut frame)));
    }
    output.expect("at least one frame")
}

fn gamer_shelf_geometry(ctx: &egui::Context) -> ShelfGeometry {
    ctx.data(|data| data.get_temp::<PlatformShelfState>(platform_shelf_state_id()))
        .map(|state| state.geometry)
        .unwrap_or_default()
}

pub(super) fn app_for_operation_tests() -> ArchiveFsApp {
    ArchiveFsApp {
        state: LoadState::Ready(Box::new(empty_loaded_data("/mount"))),
        database_state: DatabaseState::NotCreated {
            database_path: PathBuf::from("/config/library.sqlite3"),
        },
        database_generation: DatabaseGeneration::INITIAL,
        pending_source_scan_summary: None,
        sources_last_scan: None,
        // Left unloaded: these tests never open the Cheat Sources page,
        // and loading it here would read the real per-user preferences.
        cheat_sources_page: None,
        rom_organisation_page: None,
        repair_review_page: None,
        repair_history_page: None,
        exact_duplicate_review_page: None,
        library_view_history_page: None,
        quick_rename_mode: false,
        cheat_sources_ui: cheat_sources_page::CheatSourcesPageUi::default(),
        dat_sources_page: None,
        dat_sources_ui: dat_sources_page::DatSourcesPageUi::default(),
        doctor_scan: DoctorScanState::NotRun,
        doctor_scan_generation: RefreshGeneration::INITIAL,
        doctor_selected_finding: None,
        doctor_repair_review: None,
        doctor_repair_result: None,
        doctor_repair_finished_at_unix_seconds: None,
        library_filters: LibraryRowFilters::default(),
        library_platform_query: String::new(),
        filter: String::new(),
        filtered_rows: None,
        archive_context: ArchiveContext::default(),
        operation: None,
        mount_all: None,
        unmount_all: None,
        confirm_mount_all: None,
        focus_mount_all_cancel: false,
        mount_all_result: None,
        mount_queue: Vec::new(),
        mount_search: String::new(),
        confirm_mount_queue: false,
        active_mounts_confirm_unmount: None,
        history_filters: HistoryLogFilters::default(),
        shared_history: SharedHistoryState::NotLoaded,
        shared_history_operation: None,
        shared_rollback: SharedRollbackState::Idle,
        retroarch_profiles: RetroArchProfilesState::NotScanned,
        pcsx2_profiles: Pcsx2ProfilesState::NotScanned,
        dolphin_profiles: DolphinProfilesState::NotScanned,
        dolphin_local_profiles: DolphinLocalProfilesState::NotScanned,
        pcsx2_launch_profiles: Pcsx2LaunchProfilesState::NotScanned,
        flycast_profiles: FlycastProfilesState::NotScanned,
        pcsx2_firmware_evidence: Pcsx2FirmwareEvidenceState::NotLoaded,
        xenia_profiles: XeniaProfilesState::NotScanned,
        remembered_emulator_profiles: Vec::new(),
        cheat_workflow: None,
        dolphin_texture_mod: dolphin_texture_mod_page::DolphinTextureModPageState::default(),
        launch_retroarch: launch_readiness_page::RetroArchLaunchState::default(),
        launch_dolphin: launch_readiness_page::DolphinLaunchState::default(),
        launch_pcsx2: launch_readiness_page::Pcsx2LaunchState::default(),
        cheat_archive_picker: None,
        confirm_cheat_archive_change: None,
        confirm_unmount_all: None,
        focus_unmount_all_cancel: false,
        confirm_unmount_selected: None,
        focus_unmount_selected_cancel: false,
        unmount_all_result: None,
        feedback: None,
        confirm_unmount: None,
        confirm_lazy_unmount: None,
        confirm_lazy_unmount_final: None,
        focus_lazy_cancel: false,
        focus_final_lazy_cancel: false,
        lazy_unmount_offers: HashSet::new(),
        remount_offers: HashSet::new(),
        history: OperationHistory::default(),
        cleanup_after_unmount: false,
        diagnostics: DiagnosticsState::Ready {
            generation: RefreshGeneration::INITIAL,
            report: setup_report(true, true),
        },
        config_previously_confirmed: true,
        setup_action: None,
        refresh_error: None,
        snapshot_stale: false,
        refresh_generation: RefreshGeneration::INITIAL,
        snapshot_generation: Some(RefreshGeneration::INITIAL),
        platform_action: None,
        platform_choice: None,
        platform_custom_text: String::new(),
        alias_action: None,
        missing_removal: None,
        confirm_remove_missing: None,
        new_alias_text: String::new(),
        new_alias_platform_choice: None,
        bulk_platform_action: None,
        bulk_platform_choice: None,
        sort_field: None,
        sort_ascending: true,
        library_scroll_offset: 0.0,
        duplicate_filters: DuplicateReviewFilters::initial(),
        duplicate_sort_field: DuplicateSortField::Title,
        duplicate_sort_ascending: true,
        selected_duplicate_group: None,
        selected_duplicate_archive: None,
        health_filters: HealthDashboardFilters::default(),
        health_sort_field: HealthSortField::default(),
        health_sort_ascending: true,
        selected_health_issue: None,
        diagnostics_refresh_generation: RefreshGeneration::INITIAL,
        health_report_cache: None,
        clipboard: NativeClipboard::new(),
        view: MainView::default(),
        library_tab: LibraryTab::default(),
        problems_repair_tab: ProblemsRepairTab::default(),
        sources_tab: SourcesTab::default(),
        tools_overlay: ToolsOverlay::default(),
        show_activity: ACTIVITY_EXPANDED_BY_DEFAULT,
        show_about: false,
        show_skipped_files: false,
        skipped_files_filter: None,
        select_all_visible_requested: false,
        source_action: None,
        bsfree_manager: BsFreeManagerState::NotLoaded,
        bsfree_operation: None,
        bsfree_ui: BsFreeGuiState::default(),
        gui_config: GuiConfigSnapshot::from_config(Config {
            source_folders: vec![PathBuf::from("/library")],
            mount_root: PathBuf::from("/mount"),
            ratarmount_bin: "ratarmount".to_string(),
            master_rom_root: None,
        }),
        romm_snapshot: None,
        romm_operation: None,
        romm_generation: 0,
        selected_evidence: selected_evidence_page::SelectedEvidenceState::Idle,
        selected_evidence_generation: 0,
        no_intro_source_cache: Arc::new(Mutex::new(
            selected_evidence_no_intro::NoIntroSourceCache::new(),
        )),
        identity_sources: identity_sources_page::IdentitySourcesState::Idle,
        identity_sources_generation: 0,
        plan_preview: plan_preview_page::PlanPreviewState::Idle,
        plan_preview_generation: 0,
        rpcs3_status: rpcs3_page::Rpcs3State::Idle,
        rpcs3_status_generation: 0,
        pcsx2_status: pcsx2_page::Pcsx2StatusState::Idle,
        pcsx2_status_generation: 0,
        pcsx2_status_archive_path: None,
        romm_ui: RommCardState::default(),
        romm_config_draft: None,
        romm_preview: None,
        romm_browse: None,
        romm_stale_progress: None,
        romm_game: crate::romm_game::GamePanelState::default(),
        romm_hash_progress: None,
        catalogue_manager: CatalogueManagerState::NotLoaded,
        catalogue_review: None,
        catalogue_retrieval: None,
        catalogue_generation: 0,
        catalogue_last_result: None,
        dolphin_catalogue_manager: DolphinCatalogueManagerState::NotLoaded,
        dolphin_catalogue_review: None,
        dolphin_catalogue_retrieval: None,
        dolphin_catalogue_generation: 0,
        dolphin_catalogue_last_result: None,
        dolphin_catalogue_remove_confirm: false,
        dolphin_catalogue_update_available: None,
        dolphin_catalogue_update_check: None,
        sources_add_dialog: None,
        gamer_view_pending_first_scan: None,
        sources_remove_dialog: None,
        // Deliberately `Vec::new()`, never `load_library_view_configs_default()`,
        // in this test-only constructor - every other field here is a
        // hermetic literal default too (e.g. `database_state:
        // DatabaseState::NotCreated`), never a real disk/env read, so
        // tests stay isolated from whatever the real environment's
        // `~/.config/archivefs/library_views.json` happens to contain.
        library_views: Vec::new(),
        library_view_action: None,
        library_view_last_plan: None,
        library_view_form_dialog: None,
        library_view_remove_dialog: None,
        library_view_focus_archive: None,
        library_view_plan_filter: LibraryViewPlanFilter::default(),
        library_source_filter: None,
        library_column_widths: LibraryColumnWidths::default(),
        archive_inspector: None,
        archive_inspector_generation: RefreshGeneration::INITIAL,
        // Hermetic literal default, matching every other field in
        // this test-only constructor - never `load_gui_mode()`.
        ui_mode: GuiMode::default(),
        gamer_view_screen: GamerViewScreen::default(),
        mount_all_typed_count: String::new(),
        unmount_all_typed_count: String::new(),
        missing_removal_typed_count: String::new(),
        confirm_mount_selected: None,
        focus_mount_selected_cancel: false,
        mount_selected_typed_count: String::new(),
        confirm_bulk_platform_action: None,
        focus_bulk_platform_cancel: false,
        bulk_platform_action_typed_count: String::new(),
        // Hermetic literal default, matching every other field in
        // this test-only constructor - never a real disk read.
        custom_platform_artwork_directory: None,
        platform_artwork_cache: PlatformArtworkCache::default(),
        platform_artwork_manager: PlatformArtworkManagerState::default(),

        gamer_covers: crate::gamer_artwork::GamerCoverCache::default(),
        // No worker in tests: nothing here may open the real catalogue or
        // touch the network. Covers are driven through `absorb` instead.
        gamer_cover_worker: None,
        // Never true in tests: starting the worker would open the real
        // per-user identity cache and could reach a configured RomM
        // instance. Tests drive `gamer_covers` directly instead.
        gamer_cover_worker_allowed: false,
        gamer_cover_library: None,
        selected_game_metadata: None,
        game_metadata_worker: None,
        // Never true in tests, for the same reason as `gamer_cover_worker_allowed`.
        game_metadata_worker_allowed: false,
    }
}

fn setup_report(ready_for_scanning: bool, ready_for_actions: bool) -> SetupDiagnostics {
    SetupDiagnostics {
        config_path: Some(PathBuf::from("/config/archivefs.toml")),
        config_path_error: None,
        config_missing: false,
        mount_root: Some(PathBuf::from("/mount")),
        can_create_mount_root: false,
        ready_for_scanning,
        ready_for_actions,
        config_identity: default_config_identity(),
        checks: Vec::new(),
    }
}

fn empty_loaded_data(mount_root: &str) -> LoadedData {
    LoadedData {
        mount_root: PathBuf::from(mount_root),
        records: Vec::new(),
        rows: Vec::new(),
        stats: ArchiveStats {
            total_archives: 0,
            mounted_count: 0,
            pending_count: 0,
            platform_counts: Vec::new(),
            extension_counts: Vec::new(),
            largest_archive: None,
            smallest_archive: None,
            total_size_bytes: 0,
        },
        doctor: DoctorReport {
            config_path: PathBuf::from("/config/archivefs.toml"),
            checks: Vec::new(),
            archives_found: 0,
            archives_with_platform: 0,
            archives_unknown_platform: 0,
            unknown_platform_examples: Vec::new(),
            platform_counts: Vec::new(),
            pending_archives: 0,
            mounted_archives: 0,
        },
        config_identity: default_config_identity(),
    }
}

fn row_with_fields(
    path: &str,
    platform: &str,
    state: &str,
    archive_path: &str,
    mount_path: &str,
) -> ArchiveRow {
    ArchiveRow {
        path: PathBuf::from(path),
        archive_path: archive_path.to_string(),
        mount_path: mount_path.to_string(),
        platform: platform.to_string(),
        state: state.to_string(),
        search_text: format!("{archive_path}\n{mount_path}\n{platform}\n{state}").to_lowercase(),
        origin: RowOrigin::Live,
        unknown_platform: false,
        source_path: None,
    }
}

/// Like `empty_loaded_data`, but with `rows` populated directly -
/// `records` stays empty and `cached` stays `None` at every call
/// site that uses this, so `build_display_rows` always passes these
/// rows straight through unchanged (see its `cached.is_none()`
/// short-circuit), making `data.rows` and `merged_rows` identical for
/// these tests.
fn loaded_data_with_rows(mount_root: &str, rows: Vec<ArchiveRow>) -> LoadedData {
    LoadedData {
        rows,
        ..empty_loaded_data(mount_root)
    }
}

fn history_entry(outcome: ActivityOutcome, message: impl Into<String>) -> HistoryEntry {
    HistoryEntry::new(ActivityAction::Mount, None, outcome, message)
}

fn mount_all_item(name: &str, target: &str) -> MountAllItem {
    MountAllItem {
        archive_path: PathBuf::from(format!("/roms/{name}.zip")),
        mount_path: PathBuf::from(format!("/mount/{target}")),
        display_name: name.to_string(),
    }
}

fn unmount_all_item(name: &str) -> UnmountAllItem {
    UnmountAllItem {
        archive_path: PathBuf::from(format!("/roms/{name}.zip")),
        mount_path: PathBuf::from(format!("/mount/{name}")),
        display_name: name.to_string(),
    }
}

// -----------------------------------------------------------------
// Stage 4: persistent library database GUI integration - helpers.
// -----------------------------------------------------------------

/// A unique per-test temporary directory, following the exact
/// pattern `archivefs-core/src/database.rs`'s own test module uses
/// (no `tempfile` dependency in this workspace) - see requirement 8:
/// every stage 4 test that touches real paths uses one of these, and
/// none of them ever touch the real `HOME`/config/database path.
fn database_test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "archivefs-gui-database-test-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_archive_file(dir: &Path, relative_path: &str, content: &[u8]) -> PathBuf {
    let full_path = dir.join(relative_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full_path, content).unwrap();
    full_path
}

fn config_for(source_dir: &Path, mount_dir: &Path) -> Config {
    Config {
        source_folders: vec![source_dir.to_path_buf()],
        mount_root: mount_dir.to_path_buf(),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    }
}

fn record_at(path: PathBuf, mount_state: MountState) -> ArchiveRecord {
    let archive = Archive::from_path(&path).unwrap();
    ArchiveRecord::new(
        MountPlan::new(archive, PathBuf::from("/mnt/archivefs/Test")),
        mount_state,
        ArchiveMetadata {
            title: None,
            platform: None,
            region: None,
            languages: None,
            version: None,
            disc: None,
            publisher: None,
            developer: None,
            release_year: None,
            genre: None,
            notes: None,
            source: None,
            synopsis: None,
            players: None,
            rating: None,
        },
        ArchiveHealth::Pending,
    )
}

fn row_for(record: &ArchiveRecord) -> ArchiveRow {
    let status = ArchiveStatus {
        archive_path: record.mount_plan.archive.path.clone(),
        mount_path: record.mount_plan.mount_path.clone(),
        state: record.mount_state,
    };
    ArchiveRow::new(record, &status)
}

fn persisted_archive(path: PathBuf, missing: bool) -> PersistedArchive {
    PersistedArchive {
        id: 1,
        source_folder_id: 1,
        relative_path: PathBuf::from(path.file_name().unwrap()),
        absolute_path: path,
        archive_kind: "zip".to_string(),
        display_name: "Test Archive".to_string(),
        normalized_name: "test archive".to_string(),
        size_bytes: Some(1024),
        modified_time_unix_seconds: Some(0),
        platform: None,
        platform_source: None,
        last_known_health: "Pending".to_string(),
        last_seen_at: "2026-01-01T00:00:00Z".to_string(),
        last_verified_missing_at: missing.then(|| "2026-01-01T00:00:00Z".to_string()),
        identity_report: None,
    }
}

fn persisted_archive_with_platform(
    path: PathBuf,
    id: i64,
    platform: &str,
    source: &str,
) -> PersistedArchive {
    PersistedArchive {
        platform: Some(platform.to_string()),
        platform_source: Some(source.to_string()),
        id,
        ..persisted_archive(path, false)
    }
}

fn cached_snapshot(archives: Vec<PersistedArchive>) -> CachedLibrarySnapshot {
    let platform_details = archives
        .iter()
        .map(|archive| {
            (
                archive.id,
                PlatformProvenanceDetails {
                    platform: archive.platform.clone(),
                    source: archive.platform_source.clone(),
                    matched_component: None,
                    automatic_fallback: None,
                },
            )
        })
        .collect();
    let duplicate_report = catalogue_filename_duplicates(&archives);
    CachedLibrarySnapshot {
        database_path: PathBuf::from("/config/library.sqlite3"),
        schema_version: latest_schema_version(),
        archives,
        platform_details,
        stats: empty_catalogue_stats(),
        last_completed_scan: None,
        recently_found: None,
        platform_aliases: Vec::new(),
        duplicate_report,
        source_views: Vec::new(),
    }
}

fn source_view_fixture(id: i64, path: &str, enabled: bool) -> SourceFolderView {
    SourceFolderView {
        path: PathBuf::from(path),
        enabled,
        created_at: None,
        id: Some(id),
        availability: if enabled {
            SourceAvailability::Available
        } else {
            SourceAvailability::Disabled
        },
        last_scan_status: Some(SourceScanStatus::Success),
        last_scan_error: None,
        last_scan_at: None,
        last_successful_scan_at: None,
        last_archive_count: None,
        assigned_platform: None,
        unknown_archive_count: 0,
    }
}

// -------------------------------------------------------------------
// Skipped-files drill-down (`show_skipped_files_window`).
// -------------------------------------------------------------------

fn skipped_files_summary(
    skipped_files: Vec<archivefs_core::SkippedFile>,
    skipped_unsupported_extension: i64,
    skipped_ambiguous_platform: i64,
) -> ScanPersistSummary {
    ScanPersistSummary {
        scan_run_id: 1,
        counts: archivefs_core::ScanRunCounts {
            skipped_unsupported_extension,
            skipped_ambiguous_platform,
            ..Default::default()
        },
        folder_errors: Vec::new(),
        platform_assignment_warnings: Vec::new(),
        skipped_files,
        ingestion_stats: Default::default(),
        ingestion_skip_reasons: Default::default(),
        ingestion_platform_counts: Default::default(),
        ingestion_skipped: Vec::new(),
        ingestion_recognised_sample: Vec::new(),
    }
}

/// `render_row` must paint the *same* row - same `id_source` - in
/// both frames for egui to track this correctly, exactly as
/// `show_loaded_data` does across real consecutive UI frames).
/// Returns frame 2's `Response` plus whatever `ui.input(|i|
/// i.modifiers.ctrl)` reads during that same frame - proving the
/// *real* egui event path (not just the pure `apply_row_click`
/// helper in isolation) delivers a working click with an accurate
/// modifier reading, which is the actual bug this test guards
/// against regressing.
fn run_frame(
    ctx: &egui::Context,
    raw_input: egui::RawInput,
    render_row: &impl Fn(&mut egui::Ui) -> egui::Response,
) -> (egui::Response, bool) {
    let mut response = None;
    let mut ctrl_held = false;
    let _ = ctx.run(raw_input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            response = Some(render_row(ui));
            ctrl_held = ui.input(|i| i.modifiers.ctrl);
        });
    });
    (response.unwrap(), ctrl_held)
}

/// Simulates a real click gesture on the row `render_row` paints, at
/// `pos`, with `modifiers` held throughout. `render_row` must paint
/// the *same* row - same `id_source` - every time it is called, so
/// egui recognizes it as the same persistent widget across frames.
///
/// egui's hit-testing for a given frame's pointer events is computed
/// from the widget rects *registered in the previous frame* (this
/// frame's widgets have not been laid out yet when input is
/// processed) - see `egui::interaction::interact`. So registering a
/// click on a widget that has never been rendered before takes three
/// frames, not one: frame 1 merely registers the row's rect; frame 2
/// (now hit-testable) carries the press event, setting egui's
/// internal "potential click" on this row; frame 3 (hit-testable
/// again) carries the release event, which is where
/// `Response::clicked()` actually becomes true. This mirrors real
/// user input closely enough to exercise the genuine event path this
/// test suite is guarding (see the three-separate-`ctx.run` structure
/// below), rather than only calling `apply_row_click` directly with a
/// hand-built `bool`.
fn simulate_row_click(
    ctx: &egui::Context,
    pos: egui::Pos2,
    modifiers: egui::Modifiers,
    render_row: impl Fn(&mut egui::Ui) -> egui::Response,
) -> (egui::Response, bool) {
    let moved_only = egui::RawInput {
        modifiers,
        events: vec![egui::Event::PointerMoved(pos)],
        ..Default::default()
    };
    run_frame(ctx, moved_only, &render_row);

    let press = egui::RawInput {
        modifiers,
        events: vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers,
        }],
        ..Default::default()
    };
    run_frame(ctx, press, &render_row);

    let release = egui::RawInput {
        modifiers,
        events: vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers,
        }],
        ..Default::default()
    };
    run_frame(ctx, release, &render_row)
}

fn simulate_row_secondary_click(
    ctx: &egui::Context,
    pos: egui::Pos2,
    render_row: impl Fn(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    let moved_only = egui::RawInput {
        events: vec![egui::Event::PointerMoved(pos)],
        ..Default::default()
    };
    run_frame(ctx, moved_only, &render_row);

    let press = egui::RawInput {
        events: vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }],
        ..Default::default()
    };
    run_frame(ctx, press, &render_row);

    let release = egui::RawInput {
        events: vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
        ..Default::default()
    };
    run_frame(ctx, release, &render_row).0
}

fn test_row_cells() -> [&'static str; 4] {
    ["Xbox", "Pending", "/roms/a.zip", "/mnt/Xbox/a"]
}

// A bounded, realistic `screen_rect` is required here, not just for
// fidelity: with `RawInput::default()` (no `screen_rect`), egui falls
// back to a very large default canvas. Both `ScrollArea`s in
// `show_loaded_data` use `auto_shrink([false, false])`, i.e. "always
// claim all remaining space" - against a near-infinite canvas that
// remaining space is effectively constant, so the scroll areas
// silently absorb whatever height content placed above them adds and
// the panel's total `min_rect().height()` comes out identical either
// way. Bounding the panel to a realistic window size makes the inner
// vertical `ScrollArea`'s `.max(row_height)` floor (see
// `show_loaded_data`) bite, so it can no longer fully compensate -
// only then does content above it actually show up in the total
// height, which is what makes a height comparison a meaningful "did
// the real layout render it" check instead of a tautology.
fn bounded_test_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1000.0, 250.0),
        )),
        ..Default::default()
    }
}

/// A single key-press event, bundled with the same bounded
/// `screen_rect` `bounded_test_input` uses - see its comment for why
/// an unbounded default canvas would make a height-based assertion
/// meaningless. Keyboard shortcuts do not depend on hit-testing
/// (unlike pointer clicks), so - unlike `simulate_row_click` - a
/// single frame carrying the event is enough; there is no
/// previous-frame-rect requirement to work around.
fn key_press_input(key: egui::Key, modifiers: egui::Modifiers) -> egui::RawInput {
    egui::RawInput {
        modifiers,
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..bounded_test_input()
    }
}

// -----------------------------------------------------------------
// v0.4.3-alpha: Health and Recovery Dashboard.
// -----------------------------------------------------------------

fn health_test_record(
    path: &str,
    mount_state: MountState,
    health: ArchiveHealth,
    platform: Option<&str>,
) -> ArchiveRecord {
    let mut record = record_at(PathBuf::from(path), mount_state);
    record.health = health;
    record.metadata.platform = platform.map(str::to_string);
    record
}

/// Like `loaded_data_with_rows`, but populates `records` (and derives
/// `rows`/`stats.mounted_count`/`stats.pending_count` from them) since
/// `build_health_issues` reads the live snapshot's `records`, not its
/// display-only `rows`.
fn loaded_data_with_records(mount_root: &str, records: Vec<ArchiveRecord>) -> LoadedData {
    let rows = records.iter().map(row_for).collect();
    let mounted_count = records
        .iter()
        .filter(|record| record.mount_state == MountState::Mounted)
        .count();
    let pending_count = records
        .iter()
        .filter(|record| record.mount_state != MountState::Mounted)
        .count();
    let mut data = empty_loaded_data(mount_root);
    data.stats.total_archives = records.len();
    data.stats.mounted_count = mounted_count;
    data.stats.pending_count = pending_count;
    data.records = records;
    data.rows = rows;
    data
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

/// Finds the screen-space center of the first `Shape::Text` whose
/// laid-out string *exactly matches* `needle` (unlike
/// `rendered_text_contains`'s substring search, an exact match is
/// required here so e.g. "Cancel" never matches a longer label that
/// happens to contain it) - used to click a real button inside an
/// already-rendered `egui::Window`, where the button's position
/// cannot be predicted by rendering an identical standalone widget
/// (unlike a fresh panel's first widget, a window's content depends
/// on everything painted before it and the window's own computed
/// size/anchor).
fn find_exact_text_center(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn find_in_shape(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text_shape) => (text_shape.galley.text() == needle)
                .then(|| text_shape.pos + text_shape.galley.size() / 2.0),
            egui::Shape::Vec(nested) => nested.iter().find_map(|s| find_in_shape(s, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|clipped| find_in_shape(&clipped.shape, needle))
}

/// Counts every `Shape::Text` whose laid-out string *exactly matches*
/// `needle` - the exact-match counterpart of `rendered_text_contains`,
/// used where "present" isn't precise enough (e.g. confirming a
/// heading renders exactly once, not zero or two times).
/// Like `find_exact_text_center`, but also returns the clip rect the
/// shape was painted with - the boundary egui actually enforces at
/// paint time. If the shape's own position falls outside that clip
/// rect, the text is not actually visible on screen even though it
/// was laid out and painted (a scroll area clips its content to its
/// own viewport rect; a bottom panel overlapping that viewport would
/// clip content the same way scrolling too little would).
fn find_exact_text_position_and_clip(
    output: &egui::FullOutput,
    needle: &str,
) -> Option<(egui::Pos2, egui::Rect)> {
    fn find_in_shape(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text_shape) => (text_shape.galley.text() == needle)
                .then(|| text_shape.pos + text_shape.galley.size() / 2.0),
            egui::Shape::Vec(nested) => nested.iter().find_map(|s| find_in_shape(s, needle)),
            _ => None,
        }
    }
    output.shapes.iter().find_map(|clipped| {
        find_in_shape(&clipped.shape, needle).map(|pos| (pos, clipped.clip_rect))
    })
}

fn count_exact_text_occurrences(output: &egui::FullOutput, needle: &str) -> usize {
    fn count_in_shape(shape: &egui::Shape, needle: &str) -> usize {
        match shape {
            egui::Shape::Text(text_shape) => usize::from(text_shape.galley.text() == needle),
            egui::Shape::Vec(nested) => nested.iter().map(|s| count_in_shape(s, needle)).sum(),
            _ => 0,
        }
    }
    output
        .shapes
        .iter()
        .map(|clipped| count_in_shape(&clipped.shape, needle))
        .sum()
}

/// Shared fixture: three configured sources spanning every listed
/// availability state, so a single render exercises the whole Sources
/// page in one pass.
fn three_source_views() -> Vec<SourceFolderView> {
    vec![
        SourceFolderView {
            path: PathBuf::from("/home/davedap/Archives"),
            enabled: true,
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            id: Some(1),
            availability: SourceAvailability::Available,
            last_scan_status: Some(SourceScanStatus::Success),
            last_scan_error: None,
            last_scan_at: Some("2026-07-01T00:00:00Z".to_string()),
            last_successful_scan_at: Some("2026-07-01T00:00:00Z".to_string()),
            last_archive_count: Some(1242),
            assigned_platform: None,
            unknown_archive_count: 0,
        },
        SourceFolderView {
            path: PathBuf::from("/mnt/usbdrive/retro"),
            enabled: true,
            created_at: None,
            id: Some(2),
            availability: SourceAvailability::Unavailable,
            last_scan_status: Some(SourceScanStatus::Failed),
            last_scan_error: Some("No such file or directory (os error 2)".to_string()),
            last_scan_at: Some("2026-07-02T00:00:00Z".to_string()),
            last_successful_scan_at: Some("2026-06-01T00:00:00Z".to_string()),
            last_archive_count: Some(87),
            assigned_platform: None,
            unknown_archive_count: 0,
        },
        SourceFolderView {
            path: PathBuf::from("/mnt/nvme2/collections"),
            enabled: false,
            created_at: None,
            id: Some(3),
            availability: SourceAvailability::Disabled,
            last_scan_status: None,
            last_scan_error: None,
            last_scan_at: None,
            last_successful_scan_at: None,
            last_archive_count: None,
            assigned_platform: None,
            unknown_archive_count: 0,
        },
    ]
}

fn sample_library_view(id: &str, name: &str, destination: &str) -> LibraryViewConfig {
    LibraryViewConfig {
        id: id.to_string(),
        name: name.to_string(),
        destination_root: PathBuf::from(destination),
        enabled: true,
        source_folders: Vec::new(),
        platforms: Vec::new(),
        layout_template: LibraryViewLayoutTemplate::PlatformFilename,
        profile: FrontendProfile::default(),
    }
}

fn row_menu_context_for<'a>(records: &'a [ArchiveRecord]) -> RowMenuContext<'a> {
    RowMenuContext {
        records,
        cached: None,
        busy: false,
        block_reason: None,
        platform_busy: false,
        retroarch_profiles: &RetroArchProfilesState::NotScanned,
        library_views_configured: false,
        library_view_last_plan: None,
    }
}

// -- Hoisted shared test fixtures (round 2) ---------------------------

struct RealLoadedDataHarness {
    filter: String,
    filtered_rows: Option<Vec<usize>>,
    archive_context: ArchiveContext,
    library_filters: LibraryRowFilters,
    library_platform_query: String,
    sort_field: Option<SortField>,
    sort_ascending: bool,
    library_scroll_offset: f32,
    library_column_widths: LibraryColumnWidths,
    /// Persistent across `.render()` calls (unlike every other piece of
    /// per-frame throwaway state below) so tests can drive the
    /// "Unmount selected" confirmation dialog across multiple real
    /// frames: open it on one render, then Cancel/Confirm on the next.
    confirm_unmount_selected: Option<UnmountSelectedConfirmation>,
    focus_unmount_selected_cancel: bool,
    cleanup_after_unmount: bool,
    history: OperationHistory,
    requested_action: Option<AppOperationRequest>,
    /// The last `.render()` call's full output, for
    /// `rendered_text_contains` checks - `.render()` itself keeps
    /// returning just the panel height (unchanged, every existing
    /// caller relies on that), so this is purely additive.
    last_output: Option<egui::FullOutput>,
}

impl RealLoadedDataHarness {
    fn new() -> Self {
        Self {
            filter: String::new(),
            filtered_rows: None,
            archive_context: ArchiveContext::default(),
            library_filters: LibraryRowFilters::default(),
            library_platform_query: String::new(),
            sort_field: None,
            sort_ascending: true,
            library_scroll_offset: 0.0,
            library_column_widths: LibraryColumnWidths::default(),
            confirm_unmount_selected: None,
            focus_unmount_selected_cancel: false,
            cleanup_after_unmount: false,
            history: OperationHistory::default(),
            requested_action: None,
            last_output: None,
        }
    }

    /// Renders one frame with `input`, returning the whole panel's
    /// rendered content height - a real, observable side effect of
    /// everything `show_loaded_data` actually painted this frame
    /// (mount-all/doctor/search/filters/table/bulk bar/details panel
    /// all included), never a pixel-position assertion. Also records
    /// whatever `show_loaded_data` returned this frame into
    /// `self.requested_action`, so a test can assert on it directly
    /// after the call.
    fn render(&mut self, ctx: &egui::Context, data: &LoadedData, input: egui::RawInput) -> f32 {
        let mut confirm_unmount = None;
        let mut confirm_lazy_unmount = None;
        let mut confirm_lazy_unmount_final = None;
        let mut confirm_mount_all = None;
        let mut focus_mount_all_cancel = false;
        let mut mount_all_typed_count = String::new();
        let mut confirm_unmount_all = None;
        let mut focus_unmount_all_cancel = false;
        let mut unmount_all_typed_count = String::new();
        let mut confirm_mount_selected = None;
        let mut focus_mount_selected_cancel = false;
        let mut mount_selected_typed_count = String::new();
        let mut confirm_bulk_platform_action = None;
        let mut focus_bulk_platform_cancel = false;
        let mut bulk_platform_action_typed_count = String::new();
        let mut focus_lazy_cancel = false;
        let mut focus_final_lazy_cancel = false;
        let lazy_unmount_offers = HashSet::new();
        let remount_offers = HashSet::new();
        let mut platform_choice = None;
        let mut platform_custom_text = String::new();
        let mut bulk_platform_choice = None;
        let mut confirm_remove_missing = None;
        let mut missing_removal_typed_count = String::new();
        let mut clipboard = InMemoryClipboard::default();
        let mut select_all_visible_requested = false;
        let mut library_source_filter = None;

        let mut panel_height = 0.0;
        self.requested_action = None;
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                self.requested_action = show_loaded_data(
                    ui,
                    data,
                    LoadedViewState {
                        filter: &mut self.filter,
                        filtered_rows: &mut self.filtered_rows,
                        selected_archive: &mut self.archive_context.focused,
                        operation: None,
                        busy: false,
                        block_reason: None,
                        action_readiness_debug_lines: &[],
                        feedback: None,
                        confirm_unmount: &mut confirm_unmount,
                        confirm_lazy_unmount: &mut confirm_lazy_unmount,
                        confirm_lazy_unmount_final: &mut confirm_lazy_unmount_final,
                        confirm_mount_all: &mut confirm_mount_all,
                        focus_mount_all_cancel: &mut focus_mount_all_cancel,
                        mount_all_typed_count: &mut mount_all_typed_count,
                        confirm_unmount_all: &mut confirm_unmount_all,
                        focus_unmount_all_cancel: &mut focus_unmount_all_cancel,
                        unmount_all_typed_count: &mut unmount_all_typed_count,
                        confirm_unmount_selected: &mut self.confirm_unmount_selected,
                        focus_unmount_selected_cancel: &mut self.focus_unmount_selected_cancel,
                        confirm_mount_selected: &mut confirm_mount_selected,
                        focus_mount_selected_cancel: &mut focus_mount_selected_cancel,
                        mount_selected_typed_count: &mut mount_selected_typed_count,
                        confirm_bulk_platform_action: &mut confirm_bulk_platform_action,
                        focus_bulk_platform_cancel: &mut focus_bulk_platform_cancel,
                        bulk_platform_action_typed_count: &mut bulk_platform_action_typed_count,
                        focus_lazy_cancel: &mut focus_lazy_cancel,
                        focus_final_lazy_cancel: &mut focus_final_lazy_cancel,
                        lazy_unmount_offers: &lazy_unmount_offers,
                        remount_offers: &remount_offers,
                        cleanup_after_unmount: &mut self.cleanup_after_unmount,
                        mount_all_result: None,
                        unmount_all_result: None,
                        history: &mut self.history,
                        cached: None,
                        library_filters: &mut self.library_filters,
                        platform_choice: &mut platform_choice,
                        platform_custom_text: &mut platform_custom_text,
                        platform_busy: false,
                        retroarch_profiles: &RetroArchProfilesState::NotScanned,
                        selected_archives: &mut self.archive_context.selected,
                        bulk_platform_choice: &mut bulk_platform_choice,
                        bulk_platform_busy: false,
                        missing_removal_available: false,
                        missing_removal_busy: false,
                        confirm_remove_missing: &mut confirm_remove_missing,
                        missing_removal_typed_count: &mut missing_removal_typed_count,
                        sort_field: &mut self.sort_field,
                        sort_ascending: &mut self.sort_ascending,
                        library_scroll_offset: &mut self.library_scroll_offset,
                        clipboard: &mut clipboard,
                        select_all_visible_requested: &mut select_all_visible_requested,
                        library_source_filter: &mut library_source_filter,
                        library_column_widths: &mut self.library_column_widths,
                        library_views_configured: false,
                        library_view_last_plan: None,
                        recent_scan: None,
                        recent_view: false,
                        library_platform_query: &mut self.library_platform_query,
                    },
                );
                panel_height = ui.min_rect().height();
            });
        });
        self.last_output = Some(output);
        panel_height
    }
}

/// A struct, not a tuple, purely to keep `show_selected_archive`'s
/// many small pieces of caller-owned state readable at the two call
/// sites below by field name instead of position.
struct EmptySelectedArchiveViewStateParts {
    confirm_unmount: Option<PathBuf>,
    confirm_lazy_unmount: Option<PathBuf>,
    focus_lazy_cancel: bool,
    lazy_unmount_offers: HashSet<PathBuf>,
    remount_offers: HashSet<PathBuf>,
    cleanup_after_unmount: bool,
    platform_choice: Option<String>,
    platform_custom_text: String,
    clipboard: InMemoryClipboard,
}

fn empty_selected_archive_view_state_parts() -> EmptySelectedArchiveViewStateParts {
    EmptySelectedArchiveViewStateParts {
        confirm_unmount: None,
        confirm_lazy_unmount: None,
        focus_lazy_cancel: false,
        lazy_unmount_offers: HashSet::new(),
        remount_offers: HashSet::new(),
        cleanup_after_unmount: false,
        platform_choice: None,
        platform_custom_text: String::new(),
        clipboard: InMemoryClipboard::default(),
    }
}

// -- Hoisted shared test fixtures (round 3, transitive deps) ----------

fn duplicate_catalogue_for_gui() -> Vec<PersistedArchive> {
    let mut first = persisted_archive_with_platform(
        PathBuf::from("/roms/a/Sonic the Hedgehog.zip"),
        1,
        "Mega Drive",
        "heuristic-path-detector",
    );
    first.display_name = "Sonic the Hedgehog".to_string();
    let mut second = persisted_archive_with_platform(
        PathBuf::from("/backup/Sonic the Hedgehog.7z"),
        2,
        "Mega Drive",
        "heuristic-path-detector",
    );
    second.display_name = "Sonic the Hedgehog".to_string();
    second.size_bytes = Some(2048);
    second.last_verified_missing_at = Some("2026-02-01T00:00:00Z".to_string());
    let mut third = persisted_archive_with_platform(
        PathBuf::from("/roms/a/Another Game.zip"),
        3,
        "SNES",
        "heuristic-path-detector",
    );
    third.display_name = "Another Game".to_string();
    let mut fourth = persisted_archive_with_platform(
        PathBuf::from("/backup/Another_Game.7z"),
        4,
        "SNES",
        "heuristic-path-detector",
    );
    fourth.display_name = "Another Game".to_string();
    vec![first, second, third, fourth]
}

fn cheat_manifest(source_id: &str) -> CheatSourceManifest {
    CheatSourceManifest {
        format_version: CHEAT_SOURCE_RESULT_SCHEMA_VERSION,
        source_id: source_id.to_string(),
        source_url: "https://example.invalid/catalogue.zip".to_string(),
        canonical_repository_url: "https://github.com/libretro/libretro-database".to_string(),
        resolved_revision: "1".repeat(40),
        pinned_version: None,
        fetched_at_unix_seconds: 0,
        downloaded_bytes: 0,
        extracted_bytes: 0,
        archive_entry_count: 0,
        archive_sha256: "test-digest".to_string(),
        response_content_type: None,
        response_etag: None,
        response_last_modified: None,
        catalogue_file_count: 0,
        indexed_file_count: 0,
        valid_cheat_count: 0,
        malformed_cheat_count: 0,
        skipped_entry_count: 0,
        excluded_unsupported_count: 0,
        excluded_path_encoding_count: 0,
        exclusion_examples: vec![],
        discovered_platforms: Vec::new(),
        validation_complete: true,
        warnings: Vec::new(),
        catalogue_relative_path: "catalogue.json".to_string(),
        cache_relative_path: "cache".to_string(),
        files: Vec::new(),
    }
}

fn gafe01_provider_fetch() -> GeckoProviderFetchResult {
    GeckoProviderFetchResult {
            result: GeckoProviderResult {
                provider_id: "dolphin_upstream_gamesettings".to_string(),
                provider_display_name: "Dolphin upstream GameSettings".to_string(),
                source_identity: "https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Data/Sys/GameSettings/GAFE01.ini".to_string(),
                retrieved_at_unix_seconds: 1,
                game_id: "GAFE01".to_string(),
                title: Some("Animal Crossing".to_string()),
                region: GeckoRegion::Usa,
                revision: 0,
                entries: vec![GeckoProviderEntry {
                    provider_entry_id: "gafe01-widescreen".to_string(),
                    name: "16:9 Widescreen".to_string(),
                    code_lines: vec![
                        "040037A0 3C608000".to_string(),
                        "040037A4 C38337AC".to_string(),
                    ],
                    notes: Vec::new(),
                    region: GeckoRegion::Usa,
                    revision_applicability: GeckoRevisionApplicability::Uncertain,
                    parse_warnings: vec!["Revision applicability is not declared.".to_string()],
                    safe_to_offer: true,
                }],
                warnings: Vec::new(),
                attribution: "Gecko definitions from Dolphin upstream.".to_string(),
                license: "GPL-2.0-or-later".to_string(),
            },
            status: GeckoProviderFetchStatus::FreshCache,
            refresh_error: None,
        }
}

fn empty_catalogue_stats() -> CatalogueStats {
    CatalogueStats {
        total_archives: 0,
        present_archives: 0,
        missing_archives: 0,
        archives_with_platform: 0,
        archives_unknown_platform: 0,
    }
}
