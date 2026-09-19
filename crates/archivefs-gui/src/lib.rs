//! The EmuWiz GUI.
//!
//! This library owns the whole GUI: `ArchiveFsApp`, every page and
//! controller module, and the native startup in [`run`]. The three shipped
//! executables - `emuwiz`, `emuwiz-gui` and `archivefs-gui` - are thin
//! launchers in `src/bin/` that do nothing but call it, so the module tree
//! and its tests compile once instead of once per binary name.
//!
//! This file is the coordination boundary the repository's GUI root
//! architecture policy describes (see AGENTS.md): module declarations,
//! bootstrap, native options and the eframe launch. Feature logic belongs
//! in a focused module.

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

use app::{ActionFeedback, AppOperationRequest, ArchiveFsApp, CleanupFeedback};
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
    BsFreeDedupFindingKind, BsFreeDownloadOptions, BsFreeGameCubeCheat,
    BsFreeGameCubeCheatSelection, BsFreeGameCubeCodeFormat, BsFreeGameCubeError,
    BsFreeGameCubeErrorKind, BsFreeGameCubeInstallPreviewRequest, BsFreeGameCubeMatch,
    BsFreeGameCubeSearchOutcome, BsFreeGameCubeSearchStatus, BsFreeGameSearchRequest, BsFreePaths,
    BsFreeSystem, BsFreeWiiCheat, BsFreeWiiCheatSelection, BsFreeWiiCodeFormat,
    BsFreeWiiDedupFinding, BsFreeWiiError, BsFreeWiiErrorKind, BsFreeWiiInstallPreviewRequest,
    BsFreeWiiMatch, BsFreeWiiSearchOutcome, BsFreeWiiSearchStatus, CheatCandidate,
    CheatCandidateArchive, CheatCandidateClassification, CheatCandidateList, CheatCandidateOptions,
    CheatCatalogueStatus, CheatDestinationRequest, CheatInstallPlanError,
    CheatInstallPreviewRequest, CheatJourneyGameIdentity, CheatJourneyIdentityEvidence,
    CheatJourneyIdentityEvidenceKind, CheatJourneyIdentityState, CheatProviderSourceState,
    CheatSelection, CheatSourceCancellation, CheatSourceError, CheatSourceExclusionKind,
    CheatSourceFetchOptions, CheatSourceFetchResult, CheatSourceFetchStatus, CheatSourceFreshness,
    CheatSourceList, CheatSourceListEntry, CheatSourceProgress, CheatSourceProgressPhase,
    CheatSourceProgressReporter, DesktopBrowserLauncher, DeviceFormatCompatibility,
    DolphinCandidate, DolphinCatalogue, DolphinCatalogueError, DolphinCatalogueErrorKind,
    DolphinCatalogueFetchOptions, DolphinCatalogueFetchResult, DolphinCatalogueLoad,
    DolphinDedupFinding, DolphinGameIniInventory, DolphinGeckoLookupResult,
    DolphinInstallPlanError, DolphinInstallPreviewRequest, DolphinInstallationType,
    DolphinMatchState, DolphinProfile, DolphinProfileDiscovery, DolphinProfileDiscoveryRoots,
    DolphinProfileScope, DolphinProviderCodeSelection, DolphinSettingsDirectoryState,
    EmulatorProfileCandidate, EmulatorProfileSelectReason, EmulatorProfileSelection,
    FlycastProfileDiscovery, FlycastProfileDiscoveryRoots, GAMEHACKING_BROWSER_IMPORT_BLOCKED_BODY,
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
mod archive_context;
use archive_context::ArchiveContext;
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
use ui::components::ArrowDirection;
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
    ArchiveRow, LibraryRowFilters, LoadedData, MergedDisplayRowsCache, MergedDisplayRowsKey,
    RowOrigin, build_display_rows, cached_display_rows, matching_row_indices,
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
mod identity_providers_page;
mod dat_identity_panel;
mod selected_game_readiness;
mod screenscraper_page;
mod screenscraper_enrichment_page;
mod screenscraper_batch_enrichment_page;
use dat_identity_panel::*;
mod source_controller;
#[allow(unused_imports)]
use source_controller::{SourcesAddDialogState, SourcesRemoveDialogState, SourcesRoleDialogState};
pub(crate) mod bulk_confirmation;
use bulk_confirmation::show_bulk_action_typed_count_gate;
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
pub(crate) mod pcsx2_texture_mod_page;
pub(crate) mod exact_duplicate_review_page;
#[allow(dead_code)]
pub(crate) mod feature_discovery;
pub(crate) mod game_metadata;
pub(crate) mod game_presentation;
use game_presentation::{
    UNKNOWN_PLATFORM_EXPLANATION, platform_provenance_lines, unknown_platform_aggregate_headline,
    unknown_platform_banner_visible,
};
#[allow(dead_code)]
pub(crate) mod gamer_artwork;
pub(crate) mod home_page;
mod simple_mode;
use home_page::home_library_snapshot;
pub(crate) mod identity_sources_page;
#[allow(dead_code)]
pub(crate) mod launch_readiness_page;
pub(crate) mod library_view_history_page;
pub(crate) mod local_mod_package_page;
pub(crate) mod rpcs3_ordinary_mod_page;
pub(crate) mod cemu_graphic_pack_page;
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
use retroarch_core_setup::{
    load_retroarch_core_directory_override, retroarch_core_directory_override_path,
};
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
mod source_state;
use source_state::{source_platform_state, source_platform_value_label};
mod sources_page;
pub(crate) mod status_wording;
use status_wording::{
    format_database_upgrade_success, format_scan_activity, format_scan_completion,
};
#[allow(dead_code)]
pub(crate) mod tape_analysis_page;
#[allow(dead_code)]
mod ui;
mod view_mode;
use view_mode::{GuiMode, load_gui_mode, save_gui_mode};

use crate::romm_config::{
    ConfigDialogRequest, build_mappings_view, show_config_dialog, token_field_state, validate_draft,
};
use crate::romm_source::{
    RommCardRequest, RommOperation, RommOperationOutcome, RommProgress, RommProgressEvent,
    VerifyRommSummary,
};
use activity_history::{
    ACTIVITY_EXPANDED_BY_DEFAULT, ALL_ACTIVITY_ACTIONS, ALL_ACTIVITY_OUTCOMES, ActivityAction,
    ActivityOutcome, ActivityPanelAction, HistoryEntry, HistoryLogFilters, OperationHistory,
    activity_outcome_tone, show_activity_panel, visible_history_entries,
};
use administration_pages::*;
use sources_page::*;

use archivefs_core::{
    ArchiveFsError, ArchiveHealthInput, ArchiveMountSession, ArchivePresence, ArchiveRecord,
    ArchiveUnmountSession, BulkPlatformAssignmentResult, CatalogueDuplicateArchive,
    CatalogueDuplicateGroup, CatalogueDuplicateReport, Config, Database, DatabaseHealthReport,
    DoctorReport, DoctorStatus, FrontendProfileKind, HealthCategory, HealthIssue, InspectorEntry,
    InspectorEntryClassification, InspectorEntryKind, InspectorReport, LazyUnmountCleanupResult,
    LibraryViewConfig, LibraryViewPlan, LibraryViewPlanEntry, MANUAL_PLATFORM_SOURCE,
    MissingArchiveRemovalResult, MountOneOutcome, MountState, PersistedArchive, PlatformAlias,
    PlatformAssignmentChange, PlatformProvenanceDetails, RecentScanAdditions, RecoveryAction,
    RecoveryOffer, RemoveSourceFolderOutcome, ScanPersistSummary, SetSourceFolderEnabledOutcome,
    SourceAvailability, SourceFolderConfig, SourceFolderView, SourceHealthIssue, SourceRole,
    UnmountOneOutcome,
    add_source_folder_default, assign_source_platform_default, build_source_folder_views,
    canonical_platform_names, catalogue_filename_duplicates, check_archive_index_freshness,
    check_database_health, classify_archive_health, cleanup_selected_mount_tree,
    default_config_path, default_database_path, default_index_path, diagnose_database,
    format_unix_timestamp_utc, inspect_archive, is_inspectable, is_known_disc_companion,
    latest_schema_version, lazy_unmount_one_archive_path_with_progress,
    list_source_folder_views_default, load_library_view_configs_default,
    load_read_only_snapshot_default, load_source_folder_configs_from, mount_one_archive_path,
    pending_schema_migration_versions, persisted_archive_has_unknown_platform,
    plan_stale_mount_directories, read_archive_index, remount_one_archive_path,
    remove_source_folder_default, scan_all_enabled_sources_default, scan_and_persist,
    scan_source_folder_default, set_source_folder_enabled_default, set_source_role_default,
    source_health_issues,
    unmount_one_archive_path, upgrade_library_database, validate_library_view_destination,
    validate_new_source_folder,
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

/// Run the EmuWiz GUI: the entire behaviour of the shipped executables.
///
/// Handles `--version`/`-V` and `--clipboard-check` before opening a
/// window, then launches eframe with the app id and icon. Returns
/// `eframe::Result` so a launcher's `main` can forward it unchanged and
/// keep the existing exit-code behaviour.
pub fn run() -> eframe::Result<()> {
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

/// `archivefs-gui --clipboard-check` - see [`run`]'s doc comment. Prints
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
    ArchiveInspectorState, ArchivePreparationState, show_archive_inspector_panel,
};
use catalogue_bsfree_ui_state::{
    BsFreeGuiState, BsFreeManagerState, BsFreeOperation, BsFreeOperationResult,
    CatalogueBsFreeUiState, RunningBsFreeOperation,
};
use database_load::{
    CachedLibrarySnapshot, DatabaseGeneration, DatabaseState, start_database_load,
};
use doctor_repair_state::DoctorRepairState;
use emulator_readiness_state::EmulatorReadinessState;
use health_duplicate_ui_state::{
    DuplicateGroupIdentity, DuplicateReviewFilters, DuplicateSortField, HealthDashboardFilters,
    HealthDuplicateUiState, HealthIssueFilter, HealthSortField,
};
use live_library_controller::{
    LiveLibraryPoll, LoadState, RefreshGeneration, poll_load, start_load,
};
use mount_operation_controller::{
    ArchiveAction, LAZY_UNMOUNT_WARNING, OperationRequest, REMOUNT_GUIDANCE, RunningOperation,
    record_cleanup_started_activity,
};
use mount_ui_state::MountUiState;
use selected_evidence_ui_state::SelectedEvidenceUiState;
use sources_ui_state::SourcesUiState;
use artwork_media_state::ArtworkMediaState;

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

// Re-exports the test modules reach through `use super::*;`. They are not
// part of the running application, so they are gated rather than left as
// unconditional imports that the library build reports as unused.
#[cfg(test)]
use activity_history::{HISTORY_LIMIT, activity_summary_entry};
#[cfg(test)]
use archive_inspector_controller::{
    ArchiveInspectorStatus, DEFAULT_INSPECTOR_PATH_COLUMN_WIDTH, INSPECTOR_DETAILS_COLUMN_WIDTH,
    InspectorSortField, show_inspector_row, visible_inspector_entry_indices,
};
#[cfg(test)]
use archivefs_core::{
    ArchiveStats, ArchiveStatus, CatalogueStats, CompletedScanSummary, ConfigIdentity,
    DatabaseHealth, FrontendProfile, LibraryViewApplyReport, LibraryViewLayoutTemplate,
    LibraryViewPlanAction, SetupDiagnosticStatus, SetupDiagnostics,
};
#[cfg(test)]
use database_load::{
    DatabaseLoadError, DatabaseMessage, DatabaseOutcome, classify_unhealthy_database,
    load_database_snapshot_at,
};
#[cfg(test)]
use mount_operation_controller::{
    CleanupOutcome, LAZY_UNMOUNT_SUCCESS, NORMAL_UNMOUNT_FAILURE_SUMMARY,
    NORMAL_UNMOUNT_RECOVERY_GUIDANCE, OperationFailure, OperationProgress, OperationSuccess,
    record_cleanup_finished_activity, run_unmount_with_cleanup,
};

#[cfg(test)]
mod tests;

// --- RetroArch core-directory override persistence (increment 2) ---------
//
// Storage + plumbing only: these cover the on-disk round-trip for the
// GUI-only `retroarch_core_directory_override.txt` file. The discovery
// behaviour it feeds is covered in `archivefs-core`
// (`retroarch_cheat_setup` + `emulator_environment::retroarch`).
