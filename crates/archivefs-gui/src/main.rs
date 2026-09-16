// egui 0.34 keeps the 0.32 panel/context entry points as compatibility
// shims. Retaining them in this security-only dependency update avoids a
// broad layout rewrite; the dedicated GUI migration can remove this once
// its changed panel semantics are reviewed independently.
#![allow(deprecated)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::ArchiveFsApp;
use archivefs_core::dat::library_identity_summary::LibraryDatIdentitySummary;
use archivefs_core::diagnostics::environment::{
    FreeSpacePolicy, StorageAssessment, assess_storage, mount_table, storage_resources,
};
use archivefs_core::diagnostics::managed::{ManagedEntryScan, scan_managed_entries};
use archivefs_core::diagnostics::profiles::{
    DiscoveredProfiles, LinuxEmulatorInstallationEvidence, PpssppReadinessAssessment,
    ProfileAssessmentReport, Rpcs3ReadinessAssessment, XemuReadinessAssessment,
    XeniaReadinessAssessment, assess_emulator_profiles, assess_ppsspp_readiness,
    assess_rpcs3_readiness, assess_xemu_readiness, assess_xenia_readiness,
    discover_linux_emulator_installations, discover_managed_appimage_installations,
    managed_appimage_executable_for, managed_scan_targets, profile_destination_directories,
};
use archivefs_core::diagnostics::repair::{
    DoctorRepairAction, DoctorRepairContext, DoctorRepairOutcome, DoctorRepairRejection,
    DoctorRepairRequest, DoctorRepairStatus, DoctorRepairVerification, execute_doctor_repair,
};
use archivefs_core::diagnostics::{
    CoverageStatus, DoctorCategory, DoctorScan, DoctorScanInputs, DoctorSeverity, Finding,
    Gathered, MountRootSafety, assess_mount_root_safety, run_doctor_scan,
};
use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::emulator_environment::retroarch::{
    DiscoveryEnvironment, ProfileKind, ProfileScope,
};
use archivefs_core::game_identity::{
    GameIdentityReport, IdentityImageFormat, IdentityKind, IdentityStatus,
    inspect_catalogued_game_identity,
};
use archivefs_core::patch_manager::{
    BrowserImportErrorKind, BrowserImportKind, BrowserImportLocalIdentity, BrowserImportOutcome,
    BrowserImportPlan, BrowserImportPlatform, BrowserImportRequest, BrowserImportSource,
    BrowserImportTextOrigin, BsFreeCatalogue, BsFreeCheat, BsFreeDedupFinding,
    BsFreeDedupFindingKind, BsFreeDownloadOptions, BsFreeGame, BsFreeGameCubeCheat,
    BsFreeGameCubeCheatSelection, BsFreeGameCubeCodeFormat, BsFreeGameCubeError,
    BsFreeGameCubeErrorKind, BsFreeGameCubeInstallPreviewRequest, BsFreeGameCubeMatch,
    BsFreeGameCubeSearchOutcome, BsFreeGameCubeSearchStatus, BsFreeGameSearchRequest,
    BsFreeGameSearchResult, BsFreePaths, BsFreeSourceStatus, BsFreeSystem, BsFreeWiiCheat,
    BsFreeWiiCheatSelection, BsFreeWiiCodeFormat, BsFreeWiiDedupFinding, BsFreeWiiError,
    BsFreeWiiErrorKind, BsFreeWiiInstallPreviewRequest, BsFreeWiiMatch, BsFreeWiiSearchOutcome,
    BsFreeWiiSearchStatus, CheatCandidate, CheatCandidateArchive, CheatCandidateClassification,
    CheatCandidateList, CheatCandidateOptions, CheatCatalogueStatus, CheatDestinationRequest,
    CheatInstallPlanError, CheatInstallPreviewRequest, CheatJourneyGameIdentity,
    CheatJourneyIdentityEvidence, CheatJourneyIdentityEvidenceKind, CheatJourneyIdentityState,
    CheatProviderSourceState, CheatSelection, CheatSourceCancellation, CheatSourceError,
    CheatSourceExclusionKind, CheatSourceFetchOptions, CheatSourceFetchResult,
    CheatSourceFetchStatus, CheatSourceFreshness, CheatSourceList, CheatSourceListEntry,
    CheatSourceProgress, CheatSourceProgressPhase, CheatSourceProgressReporter,
    DesktopBrowserLauncher, DeviceFormatCompatibility, DolphinCandidate, DolphinCatalogue,
    DolphinCatalogueError, DolphinCatalogueErrorKind, DolphinCatalogueFetchOptions,
    DolphinCatalogueFetchResult, DolphinCatalogueLoad, DolphinDedupFinding,
    DolphinGameIniInventory, DolphinGeckoLookupResult, DolphinInstallPlanError,
    DolphinInstallPreviewRequest, DolphinInstallationType, DolphinMatchState, DolphinProfile,
    DolphinProfileDiscovery, DolphinProfileDiscoveryRoots, DolphinProfileScope,
    DolphinProviderCodeSelection, DolphinSettingsDirectoryState, EmulatorProfileCandidate,
    EmulatorProfileSelectReason, EmulatorProfileSelection, FlycastProfileDiscovery,
    FlycastProfileDiscoveryRoots, GAMEHACKING_BROWSER_IMPORT_BLOCKED_BODY,
    GAMEHACKING_BROWSER_IMPORT_BLOCKED_TITLE, GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE,
    GameCubeCheatSelection, GameCubeCodeFormat, GameCubeGameHackingInstallPreviewRequest,
    GameCubeGameIdentity, GameCubeInstallPlanError, GameCubeInstallPlanErrorKind,
    GameHackingErrorKind, GameHackingFetchOptions, GameHackingGame, GameHackingGameCubeCheat,
    GameHackingGameCubeFetchOptions, GameHackingGameCubeGame, GameHackingGameCubeMatchCandidate,
    GameHackingGameCubeMatchStatus, GameHackingGameCubeMatchStrength, GameHackingGameCubeProvider,
    GameHackingMatchCandidate, GameHackingMatchStatus, GameHackingProvider, GameHackingWiiCheat,
    GameHackingWiiGame, GameHackingWiiMatch, GameHackingWiiMatchCandidate,
    GameHackingWiiMatchStatus, GameHackingWiiMatchStrength, GameHackingWiiProvider,
    GeckoProviderFetchOptions, GeckoProviderFetchResult, GeckoProviderFetchStatus,
    GeckoProviderQuery, HttpsCheatSourceTransport, ImportSourceKind, ImportTrustState,
    LoadedCandidate, LoadedDolphinDestination, LoadedXeniaDestination, LocalSafetyScanningState,
    PageRequest, Pcsx2CheatCandidate, Pcsx2CheatSelection, Pcsx2GameIdentity,
    Pcsx2InstallPlanError, Pcsx2InstallPreviewRequest, Pcsx2InstallationType, Pcsx2MatchState,
    Pcsx2PatchCategory, Pcsx2PatchDirectoryState, Pcsx2PnachInventory, Pcsx2Profile,
    Pcsx2ProfileDiscovery, Pcsx2ProfileDiscoveryRoots, Pcsx2ProfileScope, PreviewAdapter,
    PreviewDestinationState, PreviewEligibility, PreviewIdentity, PreviewIdentityKind,
    PreviewIdentityState, PreviewMatchStrength, PreviewProposedAction, PreviewSourceItem,
    PreviewState, ProviderGameMatchConfidence, ReadOnlyCheatCatalogue, RememberedEmulatorProfile,
    ResolvedCheatDestination, RetroArchCheatLibraryInspection, RetroArchCheatLibraryState,
    RetroArchCheatSetupDiscovery, RetroArchLocalCheatMatchState, RetroArchMaterializationError,
    RetroArchMaterializationErrorKind, RetroArchMaterializationRequest,
    RetroArchMaterializedPreview, SharedAdapterWriteSupport, SharedApplyConfirmation,
    SharedApplyOptions, SharedApplyResult, SharedApplyStatus, SharedHistoryReport,
    SharedPreviewError, SharedPreviewReport, SharedPreviewRequest, SharedRollbackConfirmation,
    SharedRollbackOptions, SharedRollbackPreview, SharedRollbackResult, SharedTransactionPlan,
    StagedCheatFile, StagedDolphinIni, StagedGameCubeCheat, StagedGameCubeIni,
    StagedXeniaPatchFile, UNKNOWN_CODE_POLICY, WiiCodeFormat, WiiGameIdentity, XeniaCandidate,
    XeniaCandidateCompatibility, XeniaCandidateOutcome, XeniaInstallPlanError,
    XeniaInstallPreviewRequest, XeniaPatchSelection, XeniaProfile, XeniaProfileDiscovery,
    XeniaProfileDiscoveryRoots, adapter_write_support, bsfree_gamecube_load_confirmed,
    bsfree_gamecube_search, bsfree_wii_load_confirmed, bsfree_wii_search,
    build_bsfree_gamecube_install_preview, build_bsfree_wii_install_preview,
    build_cheat_candidates, build_cheat_install_preview, build_dolphin_install_preview,
    build_gamecube_gamehacking_install_preview, build_pcsx2_cheat_candidates,
    build_pcsx2_install_preview, build_pcsx2_legacy_migration_preview, build_shared_preview,
    build_shared_transaction_plan, build_wii_gamehacking_install_preview, build_xenia_candidates,
    build_xenia_install_preview, check_dolphin_catalogue_update_with_transport,
    classify_bsfree_gamecube_cheat, default_bsfree_source_root, default_cheat_source_cache_root,
    default_dolphin_catalogue_cache_root, default_gecko_provider_cache_root,
    default_shared_backup_root, default_shared_history_root, discover_dolphin_profiles,
    discover_pcsx2_profiles, discover_retroarch_cheat_setup_profiles_with_core_directory_override,
    discover_shared_apply_history, discover_xenia_profiles, download_bsfree_database,
    execute_shared_apply, execute_shared_rollback, fetch_dolphin_catalogue_with_transport,
    fetch_dolphin_upstream_gecko, fetch_retroarch_cheat_source, fetch_xenia_provider_patches,
    generate_shared_operation_id, import_gamehacking_browser_content, import_local_bsfree_database,
    inspect_bsfree_source, inspect_dolphin_profile_with_activation,
    inspect_pcsx2_profile_with_activation, inspect_retroarch_cheat_library_for_game,
    list_retroarch_cheat_sources, load_candidate_document, load_cheat_catalogue_snapshot,
    load_dolphin_catalogue, load_dolphin_catalogue_update_state, load_dolphin_destination,
    load_remembered_emulator_profiles_default, load_xenia_destination, match_dolphin_inventory,
    match_pcsx2_inventory, match_strength_for_candidate, materialize_retroarch_shared_preview,
    open_gamehacking_url_in_browser, parse_dolphin_ini, plan_gamehacking_browser_import,
    preview_shared_rollback, rebuild_dolphin_catalogue_index_with_transport, region_for_game_id,
    remembered_profile_for, remove_dolphin_catalogue, remove_local_bsfree_source,
    require_dolphin_managed_gamehacking_verification, resolve_cheat_destination,
    resolve_dolphin_gecko_lookup, select_dolphin_profile, select_emulator_profile,
    selected_pcsx2_managed_cheats, set_bsfree_enabled, stage_bsfree_gamecube_install,
    stage_bsfree_wii_install, stage_dolphin_provider_ini, stage_gamecube_gamehacking_install,
    stage_gamecube_gamehacking_removal, stage_generated_cheat_file, stage_pcsx2_pnach,
    stage_xenia_patch_file, validate_installed_bsfree_source,
};
use archivefs_core::patch_manager::{
    XENIA_UPSTREAM_ATTRIBUTION, XENIA_UPSTREAM_LICENSE, XENIA_UPSTREAM_REPOSITORY,
    XeniaPatchesDirectoryState, XeniaProviderFetchOptions, XeniaProviderFetchResult,
    XeniaProviderFetchStatus,
};
use collection_discovery_page::*;
use platform_artwork_manager::{PlatformArtworkManager, PlatformArtworkManagerAction};
mod activity_history;
mod administration_pages;
mod app;
mod app_frame;
mod app_overlays;
mod app_pages;
mod app_polling;
mod app_reactions;
mod app_shell;
mod archive_inspector_controller;
mod artwork_media_state;
mod catalogue_bsfree_ui_state;
mod cheats_mods;
mod cheats_mods_preview;
#[allow(dead_code)]
mod es_de_media_state;
#[allow(dead_code)]
mod launchbox_local_state;
mod onboarding;
#[allow(dead_code)]
mod platform_artwork_manager;
use cheats_mods::*;
use cheats_mods_preview::*;
#[allow(dead_code)]
mod bios_projection_page;
mod cheat_reconciliation_review;
mod cheatbase_page;
mod clipboard;
use clipboard::{
    ClipboardBackend, ClipboardTextStatus, NativeClipboard, clipboard_environment_summary,
    clipboard_status_label,
};
use ui::text_edit::show_text_edit_with_context_menu;
mod emulator_download_page;
mod emulator_readiness_state;
mod emulator_setup;
use emulator_setup::*;
mod emulator_inventory_page;
mod emulator_setup_overrides;
mod emulator_setup_page;
mod gamer_platform_shelf;
mod health_duplicate_ui_state;
#[allow(dead_code)]
mod onframe_install_session;
#[allow(dead_code)]
mod onframe_install_state;
mod user_cheat_import_page;
use gamer_platform_shelf::*;
mod gamer_view;
use gamer_view::*;
mod emulator_setup_focus;
use emulator_setup_focus::*;
mod library_rows;
use library_rows::{
    ArchiveRow, LibraryRowFilters, LoadedData, RowOrigin, build_display_rows, matching_row_indices,
};
mod library_view;
use library_view::*;
mod library_view_controller;
#[allow(unused_imports)]
use library_view_controller::{
    LibraryViewAction, LibraryViewActionOutcome, LibraryViewFormDialogState, LibraryViewPlanFilter,
    LibraryViewRemoveDialogState, RunningLibraryViewAction, library_view_action_log_category,
    library_view_action_started_message, library_view_action_success_message,
    library_view_apply_summary_message, library_view_current_skip_count, library_view_dialog_size,
    library_view_form_profile, library_view_selections_side_by_side, library_view_submit_blocker,
    load_library_views, run_library_view_action, start_library_view_worker,
};
mod library_ui_state;
use library_ui_state::LibraryUiState;
mod database_load;
mod doctor_repair_state;
mod sources_ui_state;
mod live_library_controller;
mod setup_controller;
#[allow(unused_imports)]
use setup_controller::{
    DiagnosticsMessage, DiagnosticsState, DiagnosticsUiAction, RunningSetupAction, SetupAction,
    action_readiness_debug_lines, archive_action_block_reason, diagnostics_can_continue,
    diagnostics_state_can_continue, latest_generation_actions_safe, missing_config_is_first_run,
    show_setup_diagnostics, snapshot_identity, start_diagnostics, start_setup_action_worker,
    starter_config_available,
};
mod navigation;
#[allow(unused_imports)]
use navigation::{
    ADVANCED_NAV_GROUPS, GAMER_MENU_ADD_FOLDER_LABEL, GAMER_MENU_ADVANCED_LABEL, GAMER_MENU_LABEL,
    GAMER_MENU_SCAN_LABEL, GAMER_MENU_SETUP_LABEL, LibraryTab, MainView, NavClick, NavEntry,
    NavGroup, PRIMARY_NAVIGATION_DESTINATIONS, ProblemsRepairTab, SourcesTab, TOOLS_MENU_WORKFLOWS,
    ToolsOverlay, library_tab_for_main_view, library_tab_label, main_view_content_width,
    main_view_for_home_card, main_view_for_library_tab, main_view_for_problems_repair_tab,
    main_view_for_sources_tab, main_view_title, main_view_uses_page_scroll, nav_overlay,
    nav_quick_rename, nav_romm, nav_view, navigation_destination_enabled,
    navigation_destination_selected, problems_repair_tab_for_main_view, show_library_shell_header,
    show_primary_navigation, sources_tab_for_main_view, sources_tab_label,
};
mod selected_game_panel;
use selected_game_panel::*;
mod dat_identity_panel;
mod selected_game_readiness;
use dat_identity_panel::*;
mod source_controller;
#[allow(unused_imports)]
use source_controller::{SourcesAddDialogState, SourcesRemoveDialogState};
pub mod bulk_confirmation;
pub(crate) mod cheat_sources_page;
mod collection_discovery_page;
pub(crate) mod dat_catalogue_picker;
pub(crate) mod dat_coverage_panel;
#[allow(dead_code)]
pub(crate) mod dat_sources_page;
mod doctor_repair;
pub(crate) mod media_sets_page;
use doctor_repair::*;
pub(crate) mod doctor_page;
// Existing tests (`tests/doctor_and_repair.rs`, and this file's own unit
// tests) call `doctor_page`'s items unqualified via their own `use
// super::*;` - written before this extraction, when they lived directly in
// `main.rs`. Re-exporting here keeps every one of those call sites correct
// without rewriting them, and costs nothing extra: `doctor_page` itself
// still calls its own items unqualified regardless of this re-export.
// `#[allow(unused_imports)]`: production `main.rs` code always qualifies as
// `doctor_page::...`, so the glob only pays off in the `#[cfg(test)]` test
// tree, which clippy's `unused_imports` pass does not credit here.
#[allow(unused_imports)]
use doctor_page::*;
pub(crate) mod mount_batch;
use mount_batch::*;
mod dat_authority_dashboard;
pub(crate) mod dolphin_texture_mod_page;
pub(crate) mod exact_duplicate_review_page;
#[allow(dead_code)]
pub(crate) mod feature_discovery;
pub(crate) mod game_metadata;
pub mod game_presentation;
#[allow(dead_code)]
pub(crate) mod gamer_artwork;
pub(crate) mod home_page;
pub(crate) mod identity_sources_page;
#[allow(dead_code)]
pub(crate) mod launch_readiness_page;
pub(crate) mod library_view_history_page;
pub(crate) mod local_mod_package_page;
mod mount_operation_controller;
mod mount_operations;
mod mount_ui_state;
#[allow(dead_code)]
pub(crate) mod museum_page;
pub(crate) mod needs_attention;
pub(crate) mod optical_conversion_page;
pub(crate) mod pcsx2_page;
pub(crate) mod plan_preview_page;
mod platform_source_actions;
pub(crate) mod ready_to_play_page;
pub(crate) mod storage_health_page;
use platform_source_actions::*;
pub(crate) mod playing_library_page;
pub(crate) mod problems_repair_page;
pub(crate) mod publisher_profile_page;
pub(crate) mod repair_history_page;
#[allow(dead_code)]
pub(crate) mod repair_review_page;
pub(crate) mod retroarch_core_setup;
pub(crate) mod rom_organisation_page;
mod romm;
use romm::*;
mod romm_operation_controller;
use romm_operation_controller::{load_romm_snapshot, run_romm_operation};
mod romm_ui_state;
use romm_ui_state::RommUiState;
pub(crate) mod romm_browse;
pub(crate) mod romm_config;
pub(crate) mod romm_game;
#[allow(dead_code)]
pub(crate) mod romm_source;
pub(crate) mod rpcs3_page;
pub(crate) mod selected_evidence_no_intro;
#[allow(dead_code)]
pub(crate) mod selected_evidence_page;
mod selected_evidence_pipeline;
mod selected_evidence_ui_state;
use selected_evidence_pipeline::*;
pub mod selection_guard;
mod source_state;
mod sources_page;
pub mod status_wording;
#[allow(dead_code)]
pub(crate) mod tape_analysis_page;
#[allow(dead_code)]
mod ui;
pub mod view_mode;

use crate::romm_config::{
    ConfigDialogRequest, build_mappings_view, show_config_dialog, token_field_state, validate_draft,
};
use crate::romm_source::{
    RommCardRequest, RommOperation, RommOperationOutcome, RommProgress, RommProgressEvent,
    VerifyRommSummary,
};
use activity_history::{
    ACTIVITY_EXPANDED_BY_DEFAULT, ALL_ACTIVITY_ACTIONS, ALL_ACTIVITY_OUTCOMES, ActivityAction,
    ActivityOutcome, ActivityPanelAction, HISTORY_LIMIT, HistoryEntry, HistoryLogFilters,
    OperationHistory, activity_outcome_tone, activity_summary_entry, show_activity_panel,
    visible_history_entries,
};
use administration_pages::*;
use sources_page::*;

use archivefs_core::{
    ArchiveFsError, ArchiveHealthInput, ArchiveMountSession, ArchivePresence, ArchiveRecord,
    ArchiveSnapshot, ArchiveStats, ArchiveStatus, ArchiveUnmountSession,
    BulkPlatformAssignmentResult, CUSTOM_FOLDER_ALIAS_SOURCE, CatalogueDuplicateArchive,
    CatalogueDuplicateGroup, CatalogueDuplicateReport, CatalogueStats, CompletedScanSummary,
    Config, ConfigIdentity, DAT_ROMM_AGREEMENT_SOURCE, Database, DatabaseHealth,
    DatabaseHealthReport, DatabaseUpgradeReport, DoctorReport, DoctorStatus,
    FrontendPlatformMapping, FrontendProfile, FrontendProfileKind, FrontendProfilePolicy,
    HealthCategory, HealthIssue, InspectorEntry, InspectorEntryClassification, InspectorEntryKind,
    InspectorReport, LazyUnmountCleanupResult, LibraryViewApplyReport, LibraryViewConfig,
    LibraryViewLayoutTemplate, LibraryViewPlan, LibraryViewPlanAction, LibraryViewPlanEntry,
    MANUAL_PLATFORM_SOURCE, MissingArchiveRemovalResult, MountOneOutcome, MountState,
    PersistedArchive, PlatformAlias, PlatformAssignmentChange, PlatformProvenanceDetails,
    ROMM_PLATFORM_SOURCE, RecentScanAdditions, RecoveryAction, RecoveryOffer,
    RemoveSourceFolderOutcome, ScanPersistSummary, SetSourceFolderEnabledOutcome,
    SetupDiagnosticStatus, SetupDiagnostics, SourceAvailability, SourceFolderConfig,
    SourceFolderView, SourceHealthIssue, UnmountOneOutcome, VERIFIED_DAT_PLATFORM_SOURCE,
    add_library_view_default, add_source_folder_default, apply_library_view_default,
    assign_source_platform_default, build_source_folder_views, canonical_platform_names,
    catalogue_filename_duplicates, check_archive_index_freshness, check_database_health,
    classify_archive_health, cleanup_selected_mount_tree, create_configured_mount_root_default,
    create_starter_config_default, default_config_path, default_database_path, default_index_path,
    diagnose_database, edit_library_view_default, format_unix_timestamp_utc, inspect_archive,
    is_inspectable, is_known_disc_companion, latest_schema_version,
    lazy_unmount_one_archive_path_with_progress, list_source_folder_views_default,
    load_library_view_configs_default, load_read_only_snapshot_default,
    load_source_folder_configs_from, mount_one_archive_path, pending_schema_migration_versions,
    persisted_archive_has_unknown_platform, plan_stale_mount_directories,
    preview_library_view_default, read_archive_index, remount_one_archive_path,
    remove_library_view_default, remove_source_folder_default, repair_library_view_default,
    run_setup_diagnostics_default, scan_all_enabled_sources_default, scan_and_persist,
    scan_source_folder_default, set_library_view_enabled_default, set_mount_root_default,
    set_source_folder_enabled_default, source_health_issues, unmount_one_archive_path,
    upgrade_library_database, validate_library_view_destination, validate_new_source_folder,
};
use eframe::egui;
use ui::components::{
    archive_kind_name, detail_row, detail_row_with_copy, format_size, optional_detail_row,
    summary_value,
};
use ui::platform_artwork::{
    GameRowArtworkPaint, PlatformArtworkCache, PlatformArtworkPaint, PlatformAssetCategory,
    bundled_platform_artwork, paint_game_row_artwork, paint_platform_artwork_at,
    platform_asset_category, platform_asset_id,
};
use ui::{components as widgets, layout as ui_layout, theme};
const HEALTH_METRIC_MIN_WIDTH: f32 = 148.0;
const HEALTH_METRIC_HEIGHT: f32 = 58.0;
fn responsive_card_columns(
    available_width: f32,
    minimum_card_width: f32,
    spacing: f32,
    item_count: usize,
) -> usize {
    if item_count == 0 {
        return 0;
    }
    (((available_width + spacing) / (minimum_card_width + spacing)).floor() as usize)
        .clamp(1, item_count)
}

fn gui_version_line() -> String {
    format!("emuwiz {}", env!("CARGO_PKG_VERSION"))
}

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{}: {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

/// Structured diagnostics for install/apply paths (selected cheat counts,
/// resolved profiles, target paths, write/journal outcomes) go through the
/// `log` facade rather than only the in-app history, so they are visible
/// even when a failure never reaches a rendered banner. Level defaults to
/// `info`; set `EMUWIZ_LOG` (e.g. `debug`) to see more. The legacy
/// `ARCHIVEFS_LOG` is honoured during the compatibility period, with
/// `EMUWIZ_LOG` winning if both are set.
fn init_logging() {
    let _ = log::set_logger(&LOGGER);
    let emuwiz = std::env::var("EMUWIZ_LOG").ok();
    let legacy = std::env::var("ARCHIVEFS_LOG").ok();
    log::set_max_level(resolve_log_level(emuwiz, legacy));
}

/// The log level selected by the environment. `EMUWIZ_LOG` wins over the
/// legacy `ARCHIVEFS_LOG`; an invalid value falls back to `info`.
fn resolve_log_level(emuwiz: Option<String>, legacy: Option<String>) -> log::LevelFilter {
    emuwiz
        .or(legacy)
        .and_then(|value| value.parse::<log::LevelFilter>().ok())
        .unwrap_or(log::LevelFilter::Info)
}

const LINUX_APP_ID: &str = "io.github.kiehntre.emuwiz";
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/branding/emuwiz-logo-256.png");

fn app_icon() -> Option<egui::IconData> {
    match eframe::icon_data::from_png_bytes(APP_ICON_PNG) {
        Ok(icon) => Some(icon),
        Err(error) => {
            log::warn!("could not decode the embedded EmuWiz application icon: {error}");
            None
        }
    }
}

fn main() -> eframe::Result<()> {
    init_logging();
    let arguments = std::env::args().collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("{}", gui_version_line());
        return Ok(());
    }
    if arguments.iter().any(|arg| arg == "--clipboard-check") {
        run_clipboard_check();
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_app_id(LINUX_APP_ID);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "EmuWiz",
        options,
        Box::new(|creation_context| {
            Ok(Box::new(ArchiveFsApp::new(
                creation_context.egui_ctx.clone(),
            )))
        }),
    )
}

/// `archivefs-gui --clipboard-check` - see `main`'s doc comment. Prints
/// exactly three lines to stdout and nothing else clipboard-related; the
/// environment summary and any error detail also already went to stderr
/// via `NativeClipboard::new`, so this only needs to report the outcome.
fn run_clipboard_check() {
    let mut clipboard = NativeClipboard::new();
    println!(
        "clipboard backend initialised: {}",
        if clipboard.inner.is_some() {
            "yes"
        } else {
            "no"
        }
    );
    match clipboard.get_text_status() {
        ClipboardTextStatus::Ready(_) => {
            println!("text available: yes");
            println!("read error: none");
        }
        ClipboardTextStatus::Empty => {
            println!("text available: no");
            println!("read error: none");
        }
        ClipboardTextStatus::Unavailable(reason) => {
            println!("text available: no");
            println!("read error: {reason}");
        }
    }
}

/// Renders the ">25 items" typed-count input when required (decisions
/// 1-3, docs/GUI_NAVIGATION_RESET_DESIGN.md §9) and returns whether the
/// confirm button should be enabled - the one shared gate every bulk
/// confirmation dialog in the app calls, so the threshold and comparison
/// can never drift between them.
fn show_bulk_action_typed_count_gate(
    ui: &mut egui::Ui,
    count: usize,
    typed: &mut String,
    otherwise_available: bool,
) -> bool {
    if bulk_action_requires_typed_count(count) {
        ui.label(format!(
            "This affects more than {BULK_ACTION_TYPED_CONFIRMATION_THRESHOLD} items. Type the \
             exact count ({count}) to confirm."
        ));
        ui.add(
            egui::TextEdit::singleline(typed)
                .desired_width(80.0)
                .hint_text(count.to_string()),
        );
    }
    bulk_action_confirm_enabled(count, typed, otherwise_available)
}

/// Decisions 1-3 (docs/GUI_NAVIGATION_RESET_DESIGN.md §9): every bulk
/// action shows a preview and exact item count; 1-25 items use a normal
/// confirmation; more than this threshold requires typing the exact
/// count. One shared threshold and one shared comparison function - every
/// bulk-action confirmation dialog in the app (Mount All, Unmount All,
/// Mount Queue, Mount Selected, bulk platform assignment, missing-entry
/// removal) calls these two functions rather than each re-implementing
/// its own gate, so the rule cannot drift between call sites.
const BULK_ACTION_TYPED_CONFIRMATION_THRESHOLD: usize = 25;

fn bulk_action_requires_typed_count(count: usize) -> bool {
    count > BULK_ACTION_TYPED_CONFIRMATION_THRESHOLD
}

/// Exact match only - no leading/trailing whitespace tolerance beyond a
/// plain `trim`, no partial/prefix match, no sign, no thousands
/// separator. A count that hasn't been typed, or was typed wrong, must
/// never satisfy this.
fn bulk_action_typed_count_matches(typed: &str, count: usize) -> bool {
    let trimmed = typed.trim();
    !trimmed.is_empty() && trimmed == count.to_string()
}

/// Whether a bulk-action confirmation's primary button should be enabled:
/// the ordinary busy/eligibility gate, *and*, only once the count exceeds
/// the threshold, an exact typed match.
fn bulk_action_confirm_enabled(count: usize, typed: &str, otherwise_available: bool) -> bool {
    otherwise_available
        && (!bulk_action_requires_typed_count(count)
            || bulk_action_typed_count_matches(typed, count))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RefreshGeneration(u64);

impl RefreshGeneration {
    const INITIAL: Self = Self(0);

    fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// GUI Batch A closeout: wires the persisted DAT source registry
/// (`archivefs_core::dat::sources::DatSourceRegistry`) into
/// [`selected_evidence_page::gather_selected_evidence`]'s No-Intro lookup,
/// so the Selected page's evidence panel can use whatever No-Intro DAT the
/// user has already registered - not just a hardcoded `None`.
///
/// Runs entirely off the UI thread (called from inside the same
/// `thread::spawn` `start_selected_evidence_load` already uses). Order of
/// operations:
///
/// 1. Read the file once and run the real structural detectors to learn the
///    candidate platform - cheap header parsing, not hashing.
/// 2. Load the registry (the same on-disk file `DatSourcesPageState::load`
///    reads; this is not a second persistent registry, just another read of
///    the same one) and resolve it through `no_intro_source_cache`, which
///    only re-parses a DAT file when the registry's relevant, platform-
///    scoped fingerprint has actually changed since the last resolve - see
///    `selected_evidence_no_intro::NoIntroSourceCache`.
/// 3. Hand the resolved source (if exactly one) to the existing, unchanged
///    `gather_selected_evidence`, which does the real hashing and lookup.
///    Ambiguity is never resolved to a first pick: when more than one
///    enabled, platform-relevant source qualifies, the report's
///    `no_intro` field is patched to `NoIntroLookupResult::Ambiguous`
///    instead, naming every competing source.
#[allow(dead_code)]
fn gather_selected_evidence_with_registry(
    path: &Path,
    no_intro_source_cache: &Mutex<selected_evidence_no_intro::NoIntroSourceCache>,
) -> Result<selected_evidence_page::SelectedEvidenceReport, String> {
    gather_selected_evidence_with_registry_and_platform(path, None, no_intro_source_cache)
}

fn gather_selected_evidence_with_registry_and_platform(
    path: &Path,
    platform_hint: Option<&str>,
    no_intro_source_cache: &Mutex<selected_evidence_no_intro::NoIntroSourceCache>,
) -> Result<selected_evidence_page::SelectedEvidenceReport, String> {
    let dat_sources_config_path = archivefs_core::dat::sources::default_dat_sources_config_path();
    gather_selected_evidence_with_registry_at_and_platform(
        path,
        platform_hint,
        no_intro_source_cache,
        dat_sources_config_path.as_deref().ok(),
    )
}

/// [`gather_selected_evidence_with_registry`] with the DAT sources config
/// path injected, so a test never reads or depends on the real home
/// directory's registry file. `None` (no resolvable path, e.g. `HOME`
/// unset) behaves exactly like an empty registry - `NotImported` - the same
/// honest fallback `DatSourcesPageState` itself uses when the path cannot be
/// resolved.
#[allow(dead_code)]
fn gather_selected_evidence_with_registry_at(
    path: &Path,
    no_intro_source_cache: &Mutex<selected_evidence_no_intro::NoIntroSourceCache>,
    dat_sources_config_path: Option<&std::path::Path>,
) -> Result<selected_evidence_page::SelectedEvidenceReport, String> {
    gather_selected_evidence_with_registry_at_and_platform(
        path,
        None,
        no_intro_source_cache,
        dat_sources_config_path,
    )
}

fn gather_selected_evidence_with_registry_at_and_platform(
    path: &Path,
    platform_hint: Option<&str>,
    no_intro_source_cache: &Mutex<selected_evidence_no_intro::NoIntroSourceCache>,
    dat_sources_config_path: Option<&std::path::Path>,
) -> Result<selected_evidence_page::SelectedEvidenceReport, String> {
    let platform = std::fs::read(path)
        .ok()
        .map(|bytes| selected_evidence_page::gather_structural_evidence(path, &bytes))
        .and_then(|structural_facts| {
            archivefs_core::platform_evidence_fusion::fuse_platform_evidence(structural_facts)
                .resolved_platform
        })
        .or(platform_hint);

    let registry = dat_sources_config_path
        .and_then(|config_path| {
            archivefs_core::dat::sources::load_dat_sources_config_from(config_path).ok()
        })
        .map(|config| archivefs_core::dat::sources::DatSourceRegistry::from_config(&config).0)
        .unwrap_or_default();

    let no_intro_state = no_intro_source_cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .resolve(&registry, platform)
        .clone();

    let ambiguity_note =
        selected_evidence_no_intro::no_intro_source_note(&no_intro_state).filter(|_| {
            matches!(
                no_intro_state,
                selected_evidence_no_intro::NoIntroSourceState::Ambiguous(_)
            )
        });
    let resolved_source = match &no_intro_state {
        selected_evidence_no_intro::NoIntroSourceState::Selected(imported) => {
            Some(imported.as_ref())
        }
        _ => None,
    };

    let mut result = selected_evidence_page::gather_selected_evidence_with_platform(
        path,
        platform_hint,
        resolved_source,
    );
    if let (Ok(report), Some(note)) = (&mut result, ambiguity_note) {
        report.no_intro = selected_evidence_page::NoIntroLookupResult::Ambiguous { note };
    }
    result
}

// ---------------------------------------------------------------------
// Persistent library database (stage 4): a read-only, background-loaded
// cache of archivefs_core::Database that speeds up startup and browsing.
// It is deliberately a *separate* state machine from LoadState/
// DiagnosticsState above, polled the same way (its own generation
// counter, its own channel, the same stale-message double-check) - see
// docs/DATABASE_DESIGN.md section 5 and
// docs/adr/0001-persistent-library-database.md: this cache is never
// consulted to authorize a mount or unmount. Only `latest_generation_actions_safe`
// (backed by a live snapshot and live diagnostics, both unchanged by this
// stage) gates archive actions - see `build_display_rows` and
// `show_selected_archive` below for how a cache-only row is guaranteed to
// never carry a live ArchiveRecord into the action-granting code path.
// ---------------------------------------------------------------------

use archive_inspector_controller::{
    ArchiveInspectorState, ArchiveInspectorStatus, ArchivePreparationState,
    DEFAULT_INSPECTOR_PATH_COLUMN_WIDTH, INSPECTOR_DETAILS_COLUMN_WIDTH, InspectorSortField,
    show_archive_inspector_panel, show_inspector_row, visible_inspector_entry_indices,
};
use catalogue_bsfree_ui_state::CatalogueBsFreeUiState;
use database_load::{
    CachedLibrarySnapshot, DatabaseGeneration, DatabaseLoadError, DatabaseLoadResult,
    DatabaseMessage, DatabaseOutcome, DatabaseState, classify_unhealthy_database,
    load_database_snapshot, load_database_snapshot_at, load_snapshot_from, start_database_load,
};
use doctor_repair_state::DoctorRepairState;
use emulator_readiness_state::EmulatorReadinessState;
use health_duplicate_ui_state::{
    DuplicateGroupIdentity, DuplicateReviewFilters, DuplicateSortField, HealthDashboardFilters,
    HealthDuplicateUiState, HealthIssueFilter, HealthSortField,
};
use live_library_controller::{
    LiveLibraryPoll, LoadMessage, LoadResult, LoadState, poll_load, start_load,
};
use mount_operation_controller::{
    ArchiveAction, CleanupOutcome, LAZY_CLEANUP_FAILURE, LAZY_CLEANUP_SUCCESS,
    LAZY_UNMOUNT_SUCCESS, LAZY_UNMOUNT_WARNING, NORMAL_UNMOUNT_FAILURE_SUMMARY,
    NORMAL_UNMOUNT_RECOVERY_GUIDANCE, OperationFailure, OperationProgress, OperationRequest,
    OperationResult, OperationSuccess, REMOUNT_GUIDANCE, RunningOperation,
    cleanup_completed_message, perform_archive_action, record_cleanup_finished_activity,
    record_cleanup_started_activity, run_unmount_with_cleanup,
};
use mount_ui_state::MountUiState;
use selected_evidence_ui_state::SelectedEvidenceUiState;
use sources_ui_state::SourcesUiState;
use artwork_media_state::ArtworkMediaState;

#[derive(Debug)]
enum BsFreeManagerState {
    NotLoaded,
    Ready(Box<BsFreeSourceStatus>),
    Failed(String),
}

#[derive(Clone, Debug)]
enum BsFreeOperation {
    LoadStatus,
    Download,
    Import(PathBuf),
    Validate,
    SetEnabled(bool),
    Remove,
    LoadSystems,
    Search(BsFreeGameSearchRequest),
    LoadGame { upstream_uid: i64, offset: u32 },
}

#[derive(Debug)]
enum BsFreeOperationResult {
    Status(Box<BsFreeSourceStatus>),
    Removed,
    Search(BsFreeGameSearchResult),
    Systems(archivefs_core::patch_manager::ProviderPage<BsFreeSystem>),
    Game(
        BsFreeGame,
        archivefs_core::patch_manager::ProviderPage<BsFreeCheat>,
    ),
}

struct RunningBsFreeOperation {
    operation: BsFreeOperation,
    receiver: Receiver<Result<BsFreeOperationResult, String>>,
}

#[derive(Debug, Default)]
struct BsFreeGuiState {
    import_path: String,
    download_confirm: bool,
    remove_confirm: bool,
    search_context: Option<PathBuf>,
    search_title: String,
    search_platform: String,
    search_system_id: Option<i64>,
    platforms: Option<Result<Vec<BsFreeSystem>, String>>,
    platform_query: String,
    search_result: Option<Result<BsFreeGameSearchResult, String>>,
    selected_game: Option<BsFreeGame>,
    cheats: Option<Result<archivefs_core::patch_manager::ProviderPage<BsFreeCheat>, String>>,
}

struct RunningMissingRemoval {
    requested_paths: usize,
    receiver: Receiver<Result<MissingArchiveRemovalResult, String>>,
}

///
/// Projects the already-loaded catalogue into the small collection summary
/// Museum's platform grid consumes. This is deliberately an in-memory
/// projection: it never scans source folders, opens media, or asks RomM for
/// fresh data while rendering.
///
/// RomM per-platform media coverage is not enriched here - that requires the
/// RomM/Home-Intelligence snapshot's `media_coverage`/`platform_media_coverage`
/// fields, which are a separate, not-yet-reconciled batch; every
/// `romm_media_coverage` this function produces is `None` until that lands.
fn home_library_snapshot(snapshot: &CachedLibrarySnapshot) -> home_page::HomeLibrarySnapshot {
    let mut by_platform: HashMap<String, (usize, usize, usize)> = HashMap::new();
    for archive in &snapshot.archives {
        let Some(platform) = archive.platform.as_deref() else {
            continue;
        };
        let entry = by_platform.entry(platform.to_string()).or_default();
        entry.0 += 1;
        entry.1 += 1;
        entry.2 += usize::from(archive.last_verified_missing_at.is_some());
    }
    let mut platforms = by_platform
        .into_iter()
        .map(
            |(name, (total, identified, missing))| home_page::HomePlatformSummary {
                name,
                total,
                identified,
                missing,
                romm_media_coverage: None,
            },
        )
        .collect::<Vec<_>>();
    platforms.sort_by(|left, right| left.name.cmp(&right.name));

    home_page::HomeLibrarySnapshot {
        total: snapshot.stats.total_archives.max(0) as usize,
        present: snapshot.stats.present_archives.max(0) as usize,
        identified: snapshot.stats.archives_with_platform.max(0) as usize,
        unresolved: snapshot.stats.archives_unknown_platform.max(0) as usize,
        missing: snapshot.stats.missing_archives.max(0) as usize,
        duplicate_groups: snapshot.duplicate_report.groups.len(),
        platforms,
        romm_media_coverage: None,
    }
}

/// Whether the RetroArch cheat-database status should be (re)loaded for the
/// currently active view - lazily, at most once per `NotLoaded` state, on
/// both Sources (its original home) and Cheats & Mods (its new shortcut -
/// see `show_retroarch_catalogue_manager`'s call site there), so opening
/// either page shows current status without a manual refresh.
fn catalogue_status_load_needed(view: MainView, catalogue_manager: &CatalogueManagerState) -> bool {
    matches!(view, MainView::Sources | MainView::CheatsMods)
        && matches!(catalogue_manager, CatalogueManagerState::NotLoaded)
}

/// Maps a RomM `ProviderState` to the three-bucket readiness Home shows,
/// without pulling `archivefs_core::identity_source` into `home_page`
/// itself. `NeverImported`/`Disabled`/`Importing`/`Stale`/`Error` are all
/// "configured, but not currently serving" - distinct both from "never set
/// up" and from "ready" - matching `ProviderState`'s own doc comments on
/// why each of those is not conflated with an error or with not-configured.
fn romm_readiness_label(
    state: &archivefs_core::identity_source::status::ProviderState,
) -> home_page::RommReadinessLabel {
    use archivefs_core::identity_source::status::ProviderState;
    use home_page::RommReadinessLabel;
    match state {
        ProviderState::NotConfigured => RommReadinessLabel::NotConfigured("Not configured"),
        ProviderState::Disabled => RommReadinessLabel::Unavailable("Disabled"),
        ProviderState::NeverImported => {
            RommReadinessLabel::Unavailable("Enabled, nothing imported yet")
        }
        ProviderState::Importing => RommReadinessLabel::Unavailable("Importing"),
        ProviderState::Ready => RommReadinessLabel::Ready("Ready"),
        ProviderState::ReadyOffline => RommReadinessLabel::Ready("Ready (offline)"),
        ProviderState::Stale { .. } => RommReadinessLabel::Unavailable("Stale"),
        ProviderState::Error { .. } => RommReadinessLabel::Unavailable("Error"),
    }
}

/// Authoritative archive context shared by every primary workflow.
///
/// Invariants:
/// - `focused` is the Library/Selected detail identity, never a row index.
/// - `selected` is the exact multi-selection used for highlighting and bulk
///   actions. A single selection always equals `focused`.
/// - the active Cheats & Mods archive is derived from `focused`; it is not
///   stored a second time. Adapter state may be cached for this identity,
///   but may never choose a different archive.
/// - queue membership and mounted records are independent and must never
///   clear or replace this context.
#[derive(Default)]
struct ArchiveContext {
    focused: Option<PathBuf>,
    selected: HashSet<PathBuf>,
}

impl ArchiveContext {
    fn select_only(&mut self, path: PathBuf) {
        self.selected.clear();
        self.selected.insert(path.clone());
        self.focused = Some(path);
    }

    fn clear_selection(&mut self) {
        self.focused = None;
        self.selected.clear();
    }

    fn prune(&mut self, rows: &[ArchiveRow]) {
        self.selected
            .retain(|path| rows.iter().any(|row| &row.path == path));
        if self
            .focused
            .as_ref()
            .is_some_and(|focused| !rows.iter().any(|row| &row.path == focused))
        {
            self.focused = None;
        }
    }

    fn active_cheats(&self) -> Option<&Path> {
        self.focused.as_deref()
    }
}

/// The one GUI-owned EmuWiz configuration snapshot.
///
/// Rendering is deliberately unable to load this from disk. A failed deliberate
/// reload keeps the last usable value, while retaining an actionable error for the
/// configuration UI.
#[derive(Clone, Debug)]
struct GuiConfigSnapshot {
    current: Option<Config>,
    last_error: Option<String>,
    load_attempts: u64,
    loader: fn() -> Result<Config, String>,
}

impl GuiConfigSnapshot {
    fn load_with(loader: fn() -> Result<Config, String>) -> Self {
        match loader() {
            Ok(config) => Self {
                current: Some(config),
                last_error: None,
                load_attempts: 1,
                loader,
            },
            Err(error) => Self {
                current: None,
                last_error: Some(error),
                load_attempts: 1,
                loader,
            },
        }
    }

    fn load_default() -> Self {
        Self::load_with(load_default_gui_config)
    }

    fn reload_with(&mut self, loader: fn() -> Result<Config, String>) -> Result<(), String> {
        self.load_attempts = self.load_attempts.wrapping_add(1);
        match loader() {
            Ok(config) => {
                self.current = Some(config);
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                Err(error)
            }
        }
    }

    fn reload_default(&mut self) -> Result<(), String> {
        self.reload_with(self.loader)
    }

    fn source_roots(&self) -> Result<&[PathBuf], String> {
        self.current
            .as_ref()
            .map(|config| config.source_folders.as_slice())
            .ok_or_else(|| {
                self.last_error
                    .clone()
                    .unwrap_or_else(|| "EmuWiz configuration has not been loaded yet.".to_string())
            })
    }
}

fn load_default_gui_config() -> Result<Config, String> {
    Config::load_default().map_err(|error| error.to_string())
}

#[derive(Debug)]
enum AppOperationRequest {
    Archive(OperationRequest),
    MountAll(Vec<MountAllItem>),
    UnmountAll {
        items: Vec<UnmountAllItem>,
        cleanup_after_unmount: bool,
    },
    PlatformAssignment {
        archive_path: PathBuf,
        action: PlatformAction,
    },
    BulkPlatformAssignment {
        archive_paths: Vec<PathBuf>,
        kind: BulkPlatformActionKind,
    },
    RemoveMissing(Vec<PathBuf>),
    /// The moved-library "fix it here" card's navigation-only actions. None
    /// of these change catalogue rows, source configuration, or the
    /// filesystem themselves: they route the user to the existing explicit
    /// flow (`SourcesTab::Libraries` folder editor, `SourceAction::ScanAll`,
    /// missing-only review mode). The card's own "Clean up confirmed missing
    /// entries" button does not use this enum - it opens the existing
    /// `confirm_remove_missing` dialog directly, so the typed-count gate and
    /// explicit confirmation are unchanged.
    UpdateGameFolder,
    FullRescan,
    ReviewMissingGames,
    InspectArchive(PathBuf),
    /// Navigates to the Library Views page with this archive as the
    /// "Show in Library View preview" focus - see
    /// `ArchiveFsApp::library_view_focus_archive`'s doc comment. Never
    /// starts a Preview itself: it only helps the user find what a
    /// *previously run* preview already said about this archive, since
    /// jumping pages must never bypass Preview's own safety gate.
    ShowInLibraryViews(PathBuf),
    /// Opens the first-class Cheats & Mods workspace for this exact
    /// archive - see `ArchiveFsApp::open_cheats_mods_workspace`.
    OpenCheatsMods(PathBuf),
    /// Navigates to the existing Verify Games (DAT Sources) page - the
    /// selected-game DAT check section's own "Verify Games" call to
    /// action. Never starts an audit itself; it only opens the real,
    /// existing workflow that does.
    OpenDatSources,
}

struct ActionFeedback {
    succeeded: bool,
    message: String,
    cleanup: Option<CleanupFeedback>,
    warning: Option<String>,
    more_information: Option<String>,
}

struct CleanupFeedback {
    succeeded: bool,
    message: String,
}

pub(crate) fn open_folder_in_file_manager(folder: &Path) -> archivefs_core::Result<()> {
    let (program, argument) = if cfg!(target_os = "windows") {
        ("explorer", folder.as_os_str())
    } else if cfg!(target_os = "macos") {
        ("open", folder.as_os_str())
    } else {
        ("xdg-open", folder.as_os_str())
    };
    let status = Command::new(program)
        .arg(argument)
        .status()
        .map_err(ArchiveFsError::from)?;
    if !status.success() {
        return Err(ArchiveFsError::ExternalCommand {
            program: program.to_string(),
            status: status.code(),
            stderr: format!("could not open {}", folder.display()),
        });
    }
    Ok(())
}

fn apply_missing_removal(
    archive_paths: &[PathBuf],
) -> archivefs_core::Result<MissingArchiveRemovalResult> {
    let database_path = default_database_path()?;
    apply_missing_removal_at(&database_path, archive_paths)
}

fn apply_missing_removal_at(
    database_path: &Path,
    archive_paths: &[PathBuf],
) -> archivefs_core::Result<MissingArchiveRemovalResult> {
    if !database_path.exists() {
        return Err(ArchiveFsError::Database(format!(
            "library database does not exist at {}",
            database_path.display()
        )));
    }
    let mut database = Database::open_or_create(database_path)?;
    let mut ids = Vec::with_capacity(archive_paths.len());
    for path in archive_paths {
        let archive_id = database
            .find_archive_id_by_absolute_path(path)?
            .ok_or_else(|| {
                ArchiveFsError::Database(format!(
                    "no archive found with exact stored path {}; nothing was removed",
                    path.display()
                ))
            })?;
        ids.push(archive_id);
    }
    database.remove_missing_archives(&ids)
}

/// The one honest explanation shown everywhere platform detection came up
/// empty. `detect_platform_with_details` (archivefs-core) only ever
/// returns `Some` detection or `None` - it does not currently distinguish
/// *why* it found nothing (unsupported extension vs. ambiguous folder vs.
/// archive contents never inspected vs. no alias match), so the GUI must
/// not invent a specific-sounding reason it cannot back up. This is that
/// one generic, still-useful explanation, kept in one place so a future
/// core change that adds a real per-entry reason only has to update the
/// call sites below, not invent new copy. See docs/GUI_SIMPLIFICATION.md
/// for the core API shape that would unlock per-entry reasons.
const UNKNOWN_PLATFORM_EXPLANATION: &str = "EmuWiz checks the filename, title, and folder \
    path against known platform names and folder aliases. When none of those match, the \
    platform is left Unknown rather than guessed. Assign a platform manually below, or add a \
    folder alias in Sources so future scans recognize it automatically.";

/// Aggregate-form headline for the Unknown-platform explanation banner
/// shown on the Library page - see `UNKNOWN_PLATFORM_EXPLANATION`.
fn unknown_platform_aggregate_headline(count: usize) -> String {
    let noun = if count == 1 { "entry" } else { "entries" };
    format!("{count} {noun} with unknown platform")
}

/// Gates the Library page's aggregate Unknown-platform banner: only worth
/// showing once the user has actually asked to see Unknown-platform rows
/// (the filter checkbox), and only when there is at least one such row to
/// explain.
fn unknown_platform_banner_visible(filters: &LibraryRowFilters, unknown_count: usize) -> bool {
    filters.unknown_platform && unknown_count > 0
}

fn platform_source_label(source: Option<&str>) -> &'static str {
    match source {
        Some(MANUAL_PLATFORM_SOURCE) => "Manual assignment",
        Some(VERIFIED_DAT_PLATFORM_SOURCE) => "Verified by DAT",
        Some(ROMM_PLATFORM_SOURCE) => "Detected from RomM",
        Some(DAT_ROMM_AGREEMENT_SOURCE) => "Verified by DAT and RomM",
        Some(CUSTOM_FOLDER_ALIAS_SOURCE) => "Custom folder alias",
        Some("source_assignment") => "Source assignment",
        Some("header_identity") => "Format/header identity",
        Some("folder_alias") => "Built-in folder alias",
        Some("heuristic-path-detector") => "Filename/path heuristic",
        Some(_) => "Automatic detection",
        None => "Unknown",
    }
}

/// The confidence a stored platform source implies, using the same four-level
/// scale as [`archivefs_core::platform::DetectionConfidence`].
///
/// An explicit assignment and a format/header identity are decisive; a folder
/// alias or a filename heuristic is good evidence that could still be wrong;
/// no platform at all is Unknown. Kept as a mapping from the stored source
/// rather than re-running detection, so what a person sees is the confidence of
/// the assignment that is actually recorded.
fn platform_confidence_label(details: &PlatformProvenanceDetails) -> &'static str {
    use archivefs_core::platform::DetectionConfidence;
    if details.platform.is_none() {
        return DetectionConfidence::Unknown.label();
    }
    match details.source.as_deref() {
        Some(MANUAL_PLATFORM_SOURCE)
        | Some("header_identity")
        | Some(VERIFIED_DAT_PLATFORM_SOURCE)
        | Some(DAT_ROMM_AGREEMENT_SOURCE) => DetectionConfidence::Confirmed.label(),
        Some(ROMM_PLATFORM_SOURCE) => "High",
        Some(_) => DetectionConfidence::Probable.label(),
        None => DetectionConfidence::Unknown.label(),
    }
}

fn platform_provenance_lines(details: &PlatformProvenanceDetails) -> Vec<(&'static str, String)> {
    // The canonical display name, with the stored identifier alongside it when
    // the two differ - a person reads "Sega Mega Drive / Genesis" while the
    // library stores "MegaDrive", and both matter.
    let platform_line = match details.platform.as_deref() {
        Some(stored) => {
            let display = archivefs_core::platform::display_name_for(stored);
            if display == stored {
                stored.to_string()
            } else {
                format!("{display} ({stored})")
            }
        }
        None => "Unknown".to_string(),
    };
    let mut lines = vec![
        ("Platform", platform_line),
        ("Confidence", platform_confidence_label(details).to_string()),
        (
            "Source",
            platform_source_label(details.source.as_deref()).to_string(),
        ),
        (
            "Assignment",
            if details.source.as_deref() == Some(MANUAL_PLATFORM_SOURCE) {
                "Manually assigned".to_string()
            } else if details.platform.is_some() {
                "Automatically detected".to_string()
            } else {
                "Not assigned".to_string()
            },
        ),
    ];
    if details.platform.is_none() {
        lines.push((
            "Reason",
            "No explicit override, header identity, source assignment, folder alias, or filename evidence matched."
                .to_string(),
        ));
    }

    match (
        details.source.as_deref(),
        details.matched_component.as_ref(),
    ) {
        (Some(CUSTOM_FOLDER_ALIAS_SOURCE), Some(matched)) => {
            lines.push(("Matched alias", matched.clone()));
        }
        (Some("folder_alias"), Some(matched)) => {
            lines.push(("Matched folder", matched.clone()));
        }
        _ => {}
    }

    if details.source.as_deref() == Some(MANUAL_PLATFORM_SOURCE) {
        let fallback = details.automatic_fallback.as_ref();
        lines.push((
            "Automatic fallback",
            fallback
                .map(|fallback| fallback.platform.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
        ));
        if let Some(fallback) = fallback {
            lines.push((
                "Fallback source",
                platform_source_label(Some(&fallback.source)).to_string(),
            ));
            match (
                fallback.source.as_str(),
                fallback.matched_component.as_ref(),
            ) {
                (CUSTOM_FOLDER_ALIAS_SOURCE, Some(matched)) => {
                    lines.push(("Fallback matched alias", matched.clone()));
                }
                ("folder_alias", Some(matched)) => {
                    lines.push(("Fallback matched folder", matched.clone()));
                }
                _ => {}
            }
        }
    }

    lines
}

fn format_scan_completion(summary: &ScanPersistSummary) -> String {
    format!(
        "Scan completed\nSeen: {}\nAdded: {}\nUpdated: {} (including {} restored)\nNewly missing: {}\nUnchanged: {}\nSkipped unsupported: {}\nSkipped ambiguous: {}\nErrors: {}",
        summary.counts.archives_seen,
        summary.counts.archives_added,
        summary.counts.archives_updated,
        summary.counts.archives_restored,
        summary.counts.archives_missing,
        summary.counts.archives_unchanged,
        summary.counts.skipped_unsupported_extension,
        summary.counts.skipped_ambiguous_platform,
        summary.counts.errors_count,
    )
}

fn format_scan_activity(summary: &ScanPersistSummary) -> String {
    format!(
        "Scan completed: seen {}, added {}, updated {} (including {} restored), newly missing {}, unchanged {}, skipped {}, errors {}.",
        summary.counts.archives_seen,
        summary.counts.archives_added,
        summary.counts.archives_updated,
        summary.counts.archives_restored,
        summary.counts.archives_missing,
        summary.counts.archives_unchanged,
        summary.counts.skipped_unsupported_extension + summary.counts.skipped_ambiguous_platform,
        summary.counts.errors_count,
    )
}

fn format_database_upgrade_success(
    report: &DatabaseUpgradeReport,
    summary: &ScanPersistSummary,
) -> String {
    let migration_chain = report
        .applied_versions
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(" → ");
    format!(
        "Library database upgraded safely from schema {} to schema {} using migrations {}. The \
         original database is recoverable from {}. {}",
        report.from_version,
        report.to_version,
        migration_chain,
        report.backup_path.display(),
        format_scan_activity(summary)
    )
}

/// A source's actual platform state, derived purely from the archives the
/// snapshot already has catalogued for it (`PersistedArchive::platform`,
/// matched by `PersistedArchive::source_folder_id == SourceFolderView::id`),
/// never a new query, rescan, persisted field, or schema change. This is
/// ground truth from the same data the Library page already shows, not a
/// guess: if every catalogued archive under a source agrees on one
/// platform, that is the source's platform.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SourcePlatformState {
    /// This source has no catalogued archives yet (never scanned, or
    /// scanned and found nothing).
    NotYetKnown,
    /// Archives exist, but none resolved a platform.
    Unknown,
    /// Every catalogued archive agrees on this one platform.
    Single(String),
    /// Every catalogued archive resolved a platform, but not to the same
    /// one - `usize` is the number of distinct platforms found.
    Mixed(usize),
    /// Some archives resolved a platform and some did not.
    Partial { known: i64, unknown: i64 },
}

/// Computes [`SourcePlatformState`] for one source from the snapshot's full
/// archive list - an `O(archives)` scan per source, over data already
/// loaded in memory (never a filesystem rescan; see the type's own doc
/// comment for why no new persistence is needed).
fn source_platform_state(
    view: &SourceFolderView,
    archives: &[PersistedArchive],
) -> SourcePlatformState {
    let Some(source_id) = view.id else {
        return SourcePlatformState::NotYetKnown;
    };
    let mut resolved: Vec<&str> = Vec::new();
    let mut unresolved: i64 = 0;
    for archive in archives {
        if archive.source_folder_id != source_id {
            continue;
        }
        match archive.platform.as_deref() {
            Some(platform) => resolved.push(platform),
            None => unresolved += 1,
        }
    }
    if resolved.is_empty() && unresolved == 0 {
        return SourcePlatformState::NotYetKnown;
    }
    if resolved.is_empty() {
        return SourcePlatformState::Unknown;
    }
    if unresolved > 0 {
        return SourcePlatformState::Partial {
            known: resolved.len() as i64,
            unknown: unresolved,
        };
    }
    let mut distinct = resolved;
    distinct.sort_unstable();
    distinct.dedup();
    match distinct.as_slice() {
        [single] => SourcePlatformState::Single((*single).to_string()),
        many => SourcePlatformState::Mixed(many.len()),
    }
}

/// Simple, human-facing wording for [`SourcePlatformState`] - deliberately
/// avoids "unclassified", "heuristic", and "detected automatically"; a real
/// platform name is shown whenever the catalogued archives actually agree
/// on one. Deliberately just the *value*, with no "Platform:" prefix of
/// its own - the caller (the source card's facts grid) already supplies
/// that as the row's own label column, so prefixing it here as well would
/// render as the literal duplicate "Platform: Platform: X".
fn source_platform_value_label(state: &SourcePlatformState) -> String {
    match state {
        SourcePlatformState::NotYetKnown => "not yet known".to_string(),
        SourcePlatformState::Unknown => "Unknown".to_string(),
        SourcePlatformState::Single(platform) => platform.clone(),
        SourcePlatformState::Mixed(count) => format!("Mixed ({count} platforms)"),
        SourcePlatformState::Partial { known, unknown } => {
            format!("Partial ({known} known, {unknown} unknown)")
        }
    }
}

/// What the Settings page asks `update` to do - each maps onto an
/// existing proven workflow (`SetupAction::OpenConfigFolder`, the
/// diagnostics refresh, the Diagnostics overlay, the background
/// profile-discovery scan), never new machinery.
enum SettingsPageAction {
    OpenConfigFolder,
    ValidateConfiguration,
    OpenDiagnostics,
    RescanRetroArchProfiles,
    PlatformArtwork(PlatformArtworkManagerAction),
    /// "Run first-time setup again": re-enters the onboarding overlay
    /// (`onboarding::restart_onboarding`) without touching source folders,
    /// DAT registrations, or any other configured state.
    RunFirstTimeSetupAgain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrowDirection {
    Up,
    Down,
}

#[derive(Clone, Copy)]
struct SummaryMetric<'a> {
    label: &'a str,
    value: usize,
    tone: widgets::StatusTone,
}

fn show_health_metric_card(ui: &mut egui::Ui, width: f32, metric: SummaryMetric<'_>) {
    let color = metric.tone.color(ui);
    let content_width = (width - 20.0).max(64.0);
    let fill = if metric.value == 0 {
        theme::card_fill(ui)
    } else {
        color.gamma_multiply(0.10)
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(theme::border(ui))
        .corner_radius(7)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(content_width, HEALTH_METRIC_HEIGHT - 10.0));
            ui.set_max_width(content_width);
            ui.vertical_centered(|ui| {
                let value = egui::RichText::new(metric.value.to_string()).strong();
                ui.label(if metric.value == 0 {
                    value.color(ui.visuals().weak_text_color())
                } else {
                    value.color(color)
                });
                ui.add_sized(
                    [content_width, ui.text_style_height(&egui::TextStyle::Small)],
                    egui::Label::new(egui::RichText::new(metric.label).small()).truncate(),
                )
                .on_hover_text(metric.label);
            });
        });
}

fn show_health_metric_cards(ui: &mut egui::Ui, metrics: &[SummaryMetric<'_>]) {
    let spacing = ui.spacing().item_spacing.x;
    let columns = responsive_card_columns(
        ui.available_width(),
        HEALTH_METRIC_MIN_WIDTH,
        spacing,
        metrics.len(),
    );
    if columns == 0 {
        return;
    }
    for row in metrics.chunks(columns) {
        let width = ((ui.available_width() - spacing * (columns.saturating_sub(1) as f32))
            / columns as f32)
            .max(1.0);
        ui.horizontal(|ui| {
            for metric in row {
                show_health_metric_card(ui, width, *metric);
            }
        });
    }
}

// =====================================================================
// GUI Navigation Reset: Gamer View / Advanced View
//
// Implements docs/GUI_NAVIGATION_RESET_DESIGN.md. Mode is a view-layer
// switch only: no change to archivefs-core, no change to any existing
// `MainView` variant, render function, or adapter behaviour. Gamer View
// reuses the same `ArchiveRecord`/`ArchiveRow`/`OperationRequest`/
// `AppOperationRequest`/`CheatWorkflowState` types and the same
// `start_operation`/`open_cheats_mods_workspace`/
// `start_cheat_install_rollback` methods every existing page already
// dispatches through - it never re-implements mount, cheat-install, or
// rollback logic.
// =====================================================================

/// Decision 5 (docs/GUI_NAVIGATION_RESET_DESIGN.md §9): exactly these two
/// modes, no alternate labels. `GamerView` is the unconditional default
/// for a fresh profile/first launch (decision matches §1.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GuiMode {
    #[default]
    GamerView,
    AdvancedView,
}

/// A dedicated on-disk preference file, following the same precedent the
/// design (§1.1) points to: `~/.config/archivefs/emulator_profiles.toml`
/// is its own small file rather than a new `Config`/`config.toml` field,
/// specifically to avoid coupling unrelated persistence together. Mode is
/// a GUI-layer-only concept (never read by `archivefs-core` or the CLI),
/// so it lives in the GUI crate rather than in core.
fn gui_mode_config_path() -> Option<PathBuf> {
    archivefs_core::app_dirs::config_path("gui_mode.txt").ok()
}

/// The GUI-only file that persists an explicit RetroArch core-directory
/// override, as one plain path line. A sibling of `gui_mode.txt` under the
/// EmuWiz config directory - not part of `config.toml`, so no parser or
/// schema change, and an install that has never set one simply has no
/// file. Reading is fully injectable (`_at`) for tests.
fn retroarch_core_directory_override_path() -> Option<PathBuf> {
    archivefs_core::app_dirs::config_path("retroarch_core_directory_override.txt").ok()
}

/// Reads the override from an explicit file path. A missing/unreadable
/// file, or one that is empty or only whitespace, is `None` (automatic
/// discovery). The stored path is taken verbatim - it is the user's
/// explicit choice, never canonicalised here.
fn load_retroarch_core_directory_override_at(path: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(path).ok()?;
    let trimmed = contents.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// The default-location counterparts, used by the running app.
fn load_retroarch_core_directory_override() -> Option<PathBuf> {
    retroarch_core_directory_override_path()
        .as_deref()
        .and_then(load_retroarch_core_directory_override_at)
}

fn parse_gui_mode(contents: &str) -> GuiMode {
    match contents.trim() {
        "advanced" => GuiMode::AdvancedView,
        _ => GuiMode::GamerView,
    }
}

fn gui_mode_file_contents(mode: GuiMode) -> &'static str {
    match mode {
        GuiMode::GamerView => "gamer",
        GuiMode::AdvancedView => "advanced",
    }
}

/// A missing or unreadable file means "nothing chosen yet" - falls back
/// to the unconditional default (`GamerView`), never an error.
fn load_gui_mode() -> GuiMode {
    gui_mode_config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|contents| parse_gui_mode(&contents))
        .unwrap_or_default()
}

/// Best-effort: a failure to persist the chosen mode (e.g. a read-only
/// home directory) never blocks the mode switch itself from taking
/// effect for the rest of the session - it just won't survive a restart.
fn save_gui_mode(mode: GuiMode) {
    let Some(path) = gui_mode_config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, gui_mode_file_contents(mode));
}

#[cfg(test)]
mod tests;

#[test]
fn benign_loose_rom_doctor_findings_use_a_friendly_summary() {
    use archivefs_core::diagnostics::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};
    let mut finding = Finding {
        id: "mounts.not_required".to_string(),
        category: DoctorCategory::Mounts,
        subsystem: DoctorSubsystem::ArchiveHealth,
        severity: DoctorSeverity::Info,
        title: "No mount required".to_string(),
        explanation: "This loose ROM is used directly.".to_string(),
        why_it_matters: None,
        next_step: None,
        evidence: Vec::new(),
        affected: None,
        recovery: None,
        repair: None,
        measurements: std::collections::BTreeMap::new(),
    };
    // 840 loose-ROM findings collapse into one friendly, exact heading.
    assert_eq!(
        repeated_doctor_group_heading(&finding, 840),
        "840 loose ROMs are healthy"
    );
    assert_eq!(
        repeated_doctor_group_explanation(&finding),
        Some("These games can be used directly. Nothing needs fixing.")
    );
    // A different kind keeps its precise heading (technical detail preserved).
    finding.id = "mounts.historical_failure".to_string();
    assert!(repeated_doctor_group_heading(&finding, 12).contains("Historical mount failures"));
}

// --- RetroArch core-directory override persistence (increment 2) ---------
//
// Storage + plumbing only: these cover the on-disk round-trip for the
// GUI-only `retroarch_core_directory_override.txt` file. The discovery
// behaviour it feeds is covered in `archivefs-core`
// (`retroarch_cheat_setup` + `emulator_environment::retroarch`).

#[test]
fn a_missing_core_directory_override_file_loads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retroarch_core_directory_override.txt");
    assert!(!path.exists());
    assert_eq!(load_retroarch_core_directory_override_at(&path), None);
}

#[test]
fn an_empty_or_whitespace_core_directory_override_file_loads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retroarch_core_directory_override.txt");
    std::fs::write(&path, "   \n\t").unwrap();
    assert_eq!(load_retroarch_core_directory_override_at(&path), None);
}

#[test]
fn a_core_directory_override_round_trips_through_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("nested/retroarch_core_directory_override.txt");
    let chosen = PathBuf::from("/opt/libretro/cores");
    save_retroarch_core_directory_override_at(&path, Some(chosen.as_path()));
    assert!(path.exists(), "save must create the file (and any parent)");
    assert_eq!(
        load_retroarch_core_directory_override_at(&path),
        Some(chosen)
    );
}

#[test]
fn clearing_a_core_directory_override_removes_the_file_and_next_load_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retroarch_core_directory_override.txt");
    save_retroarch_core_directory_override_at(
        &path,
        Some(PathBuf::from("/opt/libretro/cores").as_path()),
    );
    assert!(path.exists());
    save_retroarch_core_directory_override_at(&path, None);
    assert!(!path.exists(), "clearing must remove the file");
    assert_eq!(load_retroarch_core_directory_override_at(&path), None);
    // Clearing an already-absent file is a harmless no-op.
    save_retroarch_core_directory_override_at(&path, None);
    assert!(!path.exists());
}

#[test]
fn a_persisted_core_directory_override_survives_a_second_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retroarch_core_directory_override.txt");
    save_retroarch_core_directory_override_at(&path, Some(Path::new("/first/cores")));
    save_retroarch_core_directory_override_at(&path, Some(Path::new("/second/cores")));
    assert_eq!(
        load_retroarch_core_directory_override_at(&path),
        Some(PathBuf::from("/second/cores"))
    );
}
