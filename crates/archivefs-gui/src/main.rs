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
mod activity_history;
mod administration_pages;
mod archive_inspector_controller;
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
mod live_library_controller;
mod setup_controller;
#[allow(unused_imports)]
use setup_controller::{
    DiagnosticsMessage, DiagnosticsState, DiagnosticsUiAction, RunningSetupAction, SetupAction,
    action_readiness_debug_lines, archive_action_block_reason, diagnostics_can_continue,
    diagnostics_state_can_continue, latest_generation_actions_safe, snapshot_identity,
    start_diagnostics, start_setup_action_worker, starter_config_available,
};
mod navigation;
#[allow(unused_imports)]
use navigation::{
    ADVANCED_NAV_GROUPS, GAMER_MENU_ADD_FOLDER_LABEL, GAMER_MENU_ADVANCED_LABEL, GAMER_MENU_LABEL,
    GAMER_MENU_SCAN_LABEL, GAMER_MENU_SETUP_LABEL, NavClick, NavEntry, NavGroup,
    PRIMARY_NAVIGATION_DESTINATIONS, TOOLS_MENU_WORKFLOWS, library_tab_for_main_view,
    library_tab_label, main_view_content_width, main_view_for_home_card, main_view_for_library_tab,
    main_view_for_problems_repair_tab, main_view_for_sources_tab, main_view_title,
    main_view_uses_page_scroll, nav_overlay, nav_quick_rename, nav_romm, nav_view,
    navigation_destination_enabled, navigation_destination_selected,
    problems_repair_tab_for_main_view, show_primary_navigation, sources_tab_for_main_view,
    sources_tab_label,
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
    ALL_ACTIVITY_ACTIONS, ALL_ACTIVITY_OUTCOMES, ActivityAction, ActivityOutcome, HISTORY_LIMIT,
    HistoryEntry, HistoryLogFilters, OperationHistory, visible_history_entries,
};
use administration_pages::*;
use sources_page::*;

use crate::platform_artwork_manager::PlatformArtworkManager;
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
    bundled_platform_artwork, canonical_platform_asset_id, custom_platform_artwork_path,
    paint_game_row_artwork, paint_platform_artwork_at, platform_asset_category, platform_asset_id,
};
use ui::{components as widgets, layout as ui_layout, theme};
// Brings `String`'s char-index-safe insert/delete/slice methods into
// scope - see `show_text_edit_with_context_menu` and its helpers, the
// only place these are used. The same trait egui's own `TextEdit`
// editing uses internally, so this can never disagree with it about
// UTF-8/char-boundary handling.
use eframe::egui::TextBuffer;
const COLUMN_WIDTHS: [f32; 4] = [120.0, 120.0, 440.0, 520.0];
const COLUMN_HEADERS: [&str; 4] = ["Platform", "State", "Archive path", "Mount path"];
const MIN_RESIZABLE_COLUMN_WIDTH: f32 = 160.0;
/// An upper bound purely to stop a single wild drag gesture from producing
/// an absurd column width - not a meaningful design constraint otherwise;
/// horizontal scrolling (see `show_loaded_data`'s outer
/// `egui::ScrollArea::horizontal`) is what actually accommodates a wide
/// column, not this cap.
const MAX_RESIZABLE_COLUMN_WIDTH: f32 = 2400.0;
/// How much of a resizable column's own trailing edge is reserved for its
/// drag handle (see `show_header_row`) - deliberately taken out of the
/// column's own width rather than added on top, so a header button and its
/// handle never occupy overlapping screen space (and therefore never
/// compete for the same click/drag).
const COLUMN_RESIZE_HANDLE_WIDTH: f32 = 8.0;
/// The Library table's two user-resizable column widths - Platform and
/// State are not part of this (see `COLUMN_WIDTHS`'s doc comment); they
/// stay fixed. Lives on `ArchiveFsApp` for the app's whole session, so a
/// resize survives navigating away from and back to the Library page
/// exactly like every other Library-page display preference already does.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LibraryColumnWidths {
    archive_path: f32,
    mount_path: f32,
}

impl Default for LibraryColumnWidths {
    fn default() -> Self {
        Self {
            archive_path: COLUMN_WIDTHS[2],
            mount_path: COLUMN_WIDTHS[3],
        }
    }
}

impl LibraryColumnWidths {
    /// The full four-column widths array in the same `[Platform, State,
    /// Archive path, Mount path]` order every rendering function already
    /// expects - the one place Platform/State's fixed widths and the two
    /// resizable ones are combined.
    fn as_array(&self) -> [f32; 4] {
        [
            COLUMN_WIDTHS[0],
            COLUMN_WIDTHS[1],
            self.archive_path,
            self.mount_path,
        ]
    }
}

fn responsive_library_column_widths(available_width: f32, spacing: f32) -> LibraryColumnWidths {
    let fixed = COLUMN_WIDTHS[0] + COLUMN_WIDTHS[1] + spacing * 3.0;
    let path_space = (available_width - fixed).max(520.0);
    LibraryColumnWidths {
        archive_path: (path_space * 0.46).max(240.0),
        mount_path: (path_space * 0.54).max(280.0),
    }
}

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

const SEARCH_FILTER_TEXT_EDIT_ID: &str = "archivefs_library_search_filter";
const ACTIVITY_EXPANDED_BY_DEFAULT: bool = false;
/// Matches the collapsed activity panel's real content: one row of
/// buttons/badges plus its frame margin. Only used as the very first
/// frame's guess for the "activity_collapsed" panel id - actual content
/// height takes over immediately after and is what gets persisted.
const ACTIVITY_PANEL_COLLAPSED_DEFAULT_HEIGHT: f32 = 44.0;
/// Matches the expanded activity panel's real content: the button row,
/// separator, and the history list's own `max_height(220.0)` scroll area.
/// Only used as the very first frame's guess for the "activity_expanded"
/// panel id, for the same reason as the collapsed default above.
const ACTIVITY_PANEL_EXPANDED_DEFAULT_HEIGHT: f32 = 220.0;
const NORMAL_UNMOUNT_FAILURE_SUMMARY: &str = "EmuWiz could not unmount this archive normally.\n\nA program may still be using files from this mount, or this may indicate that the mount is not responding correctly.";
const NORMAL_UNMOUNT_RECOVERY_GUIDANCE: &str = "Before using Lazy Unmount:\n\n1. Close any emulator, file manager, terminal, media player, or other application that may be using this mount.\n2. Wait a few seconds.\n3. Try Normal Unmount again.\n\nUse Lazy Unmount only when the mount will not release normally.";
const LAZY_UNMOUNT_WARNING: &str = "Lazy Unmount removes the mount from the visible filesystem immediately, even if a program still has files open.\n\nThis can interrupt applications using the mount and may cause unsaved work or incomplete file operations to be lost.\n\nClose applications using this mount before continuing.\n\nUse this only when Normal Unmount repeatedly fails.";
const LAZY_UNMOUNT_SUCCESS: &str = "Lazy unmount completed.\n\nThe mount is no longer visible. Some applications may still hold references to files that were open before the unmount. Close and reopen those applications before remounting.";
const LAZY_CLEANUP_SUCCESS: &str = "Empty mount directories were cleaned safely.";
const LAZY_CLEANUP_FAILURE: &str = "The mount was detached successfully, but EmuWiz could not remove one or more empty directories. No non-empty directory was removed.";
const REMOUNT_GUIDANCE: &str = "Make sure applications that used the previous mount have been closed. Remounting while an application still holds the old mount may cause confusing or stale file access.";

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

struct LoadedData {
    mount_root: PathBuf,
    records: Vec<ArchiveRecord>,
    rows: Vec<ArchiveRow>,
    stats: ArchiveStats,
    doctor: DoctorReport,
    config_identity: ConfigIdentity,
}

impl LoadedData {
    fn from_snapshot(snapshot: ArchiveSnapshot) -> Self {
        let rows = snapshot
            .records
            .iter()
            .zip(&snapshot.statuses)
            .map(|(record, status)| ArchiveRow::new(record, status))
            .collect();

        Self {
            mount_root: snapshot.mount_root,
            records: snapshot.records,
            rows,
            stats: snapshot.stats,
            doctor: snapshot.doctor,
            config_identity: snapshot.config_identity,
        }
    }
}

/// Where a displayed row's data came from - see requirement 4. Only `Live`
/// rows carry a path that `selected_record`/`selected_record_index` can
/// ever match against `LoadedData.records`, since those come from the
/// cache's `PersistedArchive.absolute_path`, never from a live
/// `ArchiveRecord` - this is what guarantees a cache-only selection can
/// never resolve to a live record and so can never expose an action
/// button (see `show_selected_archive`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowOrigin {
    /// Backed by the latest coherent live snapshot. Actions are available,
    /// subject to `latest_generation_actions_safe`.
    Live,
    /// Known to the persisted catalogue, not (yet) confirmed by the live
    /// snapshot, and not marked missing by the last scan.
    CachedAwaitingValidation,
    /// Known to the persisted catalogue and marked missing
    /// (`last_verified_missing_at` set) as of the last completed scan.
    CachedMissing,
    /// Known to the persisted catalogue, not marked missing by the last
    /// scan, but its path is not reachable right now (a cheap existence
    /// check at merge time, for display only - never used to authorize
    /// mount/unmount).
    CachedUnavailable,
}

impl RowOrigin {
    fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::CachedAwaitingValidation => "Cached: awaiting validation",
            Self::CachedMissing => "Cached: missing",
            Self::CachedUnavailable => "Cached: source unavailable",
        }
    }

    /// Gamer View's own wording for the same states `label()` describes
    /// precisely for Advanced View - "Cached" is internal snapshot/database
    /// vocabulary (docs/GUI_NAVIGATION_RESET_DESIGN.md §2.6's banned-word
    /// spirit, even though "Cached" itself isn't verbatim on that list).
    /// `Live` never reaches this - the one call site only shows a line at
    /// all when `origin != Live`.
    fn gamer_view_label(self) -> &'static str {
        match self {
            Self::Live => "",
            Self::CachedAwaitingValidation => "Checking this game...",
            Self::CachedMissing => "We couldn't find this game's file just now.",
            Self::CachedUnavailable => "We can't reach this game's location right now.",
        }
    }
}

#[derive(Clone)]
struct ArchiveRow {
    /// Exact-byte identity used for selection and reconciliation - never
    /// rendered directly, and never compared via `.display()` (see
    /// requirement 5). For a live row this is
    /// `ArchiveRecord.mount_plan.archive.path`; for a cache-only row it is
    /// `PersistedArchive.absolute_path` - the same pairing the database's
    /// own `(source_folder_id, relative_path)` uniqueness constraint
    /// already encodes.
    path: PathBuf,
    archive_path: String,
    mount_path: String,
    platform: String,
    state: String,
    search_text: String,
    origin: RowOrigin,
    unknown_platform: bool,
    source_path: Option<PathBuf>,
}

impl ArchiveRow {
    fn new(record: &ArchiveRecord, status: &ArchiveStatus) -> Self {
        let archive_path = status.archive_path.display().to_string();
        let mount_path = status.mount_path.display().to_string();
        let raw_platform = record
            .metadata
            .platform
            .as_deref()
            .or(record.identity.platform.as_deref());
        let unknown_platform = raw_platform.is_none();
        let platform = raw_platform.unwrap_or("Unknown").to_string();
        let state = status.state.to_string();
        let search_text =
            format!("{archive_path}\n{mount_path}\n{platform}\n{state}").to_lowercase();

        Self {
            path: record.mount_plan.archive.path.clone(),
            archive_path,
            mount_path,
            platform,
            state,
            search_text,
            origin: RowOrigin::Live,
            unknown_platform,
            source_path: None,
        }
    }

    /// Synthesizes a display-only row for a cache-only archive: one the
    /// persisted catalogue knows about but the latest live snapshot does
    /// not confirm. `path_exists` is a cheap, display-only existence
    /// check (never a substitute for live validation) that distinguishes
    /// "unreachable right now" from "awaiting the next live refresh".
    fn from_cached(persisted: &PersistedArchive, path_exists: bool) -> Self {
        let archive_path = persisted.absolute_path.display().to_string();
        let unknown_platform = persisted_archive_has_unknown_platform(persisted);
        let platform = persisted
            .platform
            .as_deref()
            .unwrap_or("Unknown")
            .to_string();
        let origin = if persisted.last_verified_missing_at.is_some() {
            RowOrigin::CachedMissing
        } else if !path_exists {
            RowOrigin::CachedUnavailable
        } else {
            RowOrigin::CachedAwaitingValidation
        };
        let state = origin.label().to_string();
        let mount_path = String::new();
        let search_text =
            format!("{archive_path}\n{mount_path}\n{platform}\n{state}").to_lowercase();

        Self {
            path: persisted.absolute_path.clone(),
            archive_path,
            mount_path,
            platform,
            state,
            search_text,
            origin,
            unknown_platform,
            source_path: None,
        }
    }

    /// Overrides this row's platform-derived fields (`platform`,
    /// `unknown_platform`, and the platform portion of `search_text`)
    /// with the library database's effective (manual-aware) platform for
    /// this archive.
    ///
    /// A live row built from `ArchiveRecord` alone only ever sees the
    /// live scan's own automatic detection
    /// (`record.metadata.platform`/`record.identity.platform`), which
    /// disagrees with the persisted effective platform exactly when a
    /// manual assignment is active and automatic detection found
    /// nothing. Without this override, such a row would be wrongly
    /// classified (and counted/filtered) as unknown. Only ever applied
    /// when the database already has a persisted row for this exact path
    /// (see `build_display_rows`); a live row with no persisted
    /// counterpart yet keeps its live-only classification, the only
    /// signal available for it.
    fn with_persisted_platform(mut self, persisted: &PersistedArchive) -> Self {
        self.unknown_platform = persisted_archive_has_unknown_platform(persisted);
        self.platform = persisted
            .platform
            .as_deref()
            .unwrap_or("Unknown")
            .to_string();
        self.search_text = format!(
            "{}\n{}\n{}\n{}",
            self.archive_path, self.mount_path, self.platform, self.state
        )
        .to_lowercase();
        self
    }
    fn with_source_path(mut self, source_path: Option<PathBuf>) -> Self {
        self.source_path = source_path;
        self
    }

    fn matches(&self, normalized_filter: &str) -> bool {
        self.search_text.contains(normalized_filter)
    }

    fn row_text_color(&self, visuals: &egui::Visuals) -> Option<egui::Color32> {
        match self.origin {
            RowOrigin::Live => None,
            RowOrigin::CachedAwaitingValidation => Some(egui::Color32::from_rgb(150, 150, 150)),
            RowOrigin::CachedMissing => Some(visuals.error_fg_color),
            RowOrigin::CachedUnavailable => Some(egui::Color32::from_rgb(210, 140, 40)),
        }
    }
}

/// Merges live rows with cache-only rows for display - see requirement 4
/// and 5. Live rows always win: a cached archive whose exact path already
/// appears among `records` is represented only by its live row, never
/// duplicated - but with its platform/unknown-platform classification
/// overridden from the persisted effective value when the database
/// already has an entry for it (see `ArchiveRow::with_persisted_platform`
/// and requirement 6). Recomputed fresh whenever the underlying live or
/// cached data changes (see `ArchiveFsApp::recompute_filtered_rows`), not
/// on every frame, so it stays cheap without risking a stale merge.
fn build_display_rows(
    records: &[ArchiveRecord],
    live_rows: &[ArchiveRow],
    cached: Option<&CachedLibrarySnapshot>,
) -> Vec<ArchiveRow> {
    let persisted_by_path: HashMap<&Path, &PersistedArchive> = cached
        .map(|cached| {
            cached
                .archives
                .iter()
                .map(|persisted| (persisted.absolute_path.as_path(), persisted))
                .collect()
        })
        .unwrap_or_default();

    // Resolves one row's owning source: by exact database id when a
    // persisted counterpart names one (the reliable case - see
    // `PersistedArchive::source_folder_id`), otherwise by the longest
    // configured source path that is a prefix of the archive's absolute
    // path (the only signal available for a brand new live-only row never
    // yet persisted). Longest-first so a source nested inside another
    // configured source's path never wrongly claims ownership of the
    // outer source's own direct children.
    let source_path_by_id: HashMap<i64, &Path> = cached
        .map(|cached| {
            cached
                .source_views
                .iter()
                .filter_map(|view| view.id.map(|id| (id, view.path.as_path())))
                .collect()
        })
        .unwrap_or_default();
    let mut source_paths_longest_first: Vec<&Path> = cached
        .map(|cached| cached.source_views.iter().map(|view| view.path.as_path()))
        .into_iter()
        .flatten()
        .collect();
    source_paths_longest_first.sort_by_key(|path| std::cmp::Reverse(path.as_os_str().len()));
    let resolve_source_path = |row_path: &Path, persisted: Option<&PersistedArchive>| {
        if let Some(path) = persisted
            .and_then(|persisted| source_path_by_id.get(&persisted.source_folder_id).copied())
        {
            return Some(path.to_path_buf());
        }
        source_paths_longest_first
            .iter()
            .find(|source_path| row_path.starts_with(source_path))
            .map(|path| path.to_path_buf())
    };

    let mut merged: Vec<ArchiveRow> = live_rows
        .iter()
        .cloned()
        .map(|row| {
            let persisted = persisted_by_path.get(row.path.as_path()).copied();
            let source_path = resolve_source_path(&row.path, persisted);
            let row = match persisted {
                Some(persisted) => row.with_persisted_platform(persisted),
                None => row,
            };
            row.with_source_path(source_path)
        })
        .collect();

    if let Some(cached) = cached {
        let live_paths: HashSet<&Path> = records
            .iter()
            .map(|record| record.mount_plan.archive.path.as_path())
            .collect();
        for persisted in &cached.archives {
            if live_paths.contains(persisted.absolute_path.as_path()) {
                continue;
            }
            let path_exists = persisted.absolute_path.exists();
            let source_path = resolve_source_path(&persisted.absolute_path, Some(persisted));
            merged.push(
                ArchiveRow::from_cached(persisted, path_exists).with_source_path(source_path),
            );
        }
    }

    merged
}

/// Optional search filters over the merged row list (requirement 6). Two
/// independent groups - state and platform - each AND'd together; within
/// a group, an unchecked filter set imposes no restriction (defaults to
/// "show everything") and multiple checked filters within the same group
/// are OR'd, so checking both `present` and `missing` shows both rather
/// than nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LibraryRowFilters {
    present: bool,
    missing: bool,
    awaiting_validation: bool,
    known_platform: bool,
    unknown_platform: bool,
    /// Session-wide platform-first selection shared with Mount and the
    /// Cheats & Mods chooser. `Some("Unknown")` is diagnostic, not empty.
    platform: Option<String>,
}

impl LibraryRowFilters {
    fn is_active(&self) -> bool {
        self.present
            || self.missing
            || self.awaiting_validation
            || self.known_platform
            || self.unknown_platform
            || self.platform.is_some()
    }

    fn matches(&self, row: &ArchiveRow) -> bool {
        let state_group_active = self.present || self.missing || self.awaiting_validation;
        let state_match = !state_group_active || {
            let is_present = matches!(row.origin, RowOrigin::Live);
            let is_missing = matches!(row.origin, RowOrigin::CachedMissing);
            let is_awaiting = matches!(
                row.origin,
                RowOrigin::CachedAwaitingValidation | RowOrigin::CachedUnavailable
            );
            (self.present && is_present)
                || (self.missing && is_missing)
                || (self.awaiting_validation && is_awaiting)
        };

        let platform_group_active = self.known_platform || self.unknown_platform;
        let platform_match = !platform_group_active
            || (self.known_platform && !row.unknown_platform)
            || (self.unknown_platform && row.unknown_platform);

        let selected_platform_match = self.platform.as_deref().is_none_or(|wanted| {
            if wanted == "Unknown" {
                row.unknown_platform
            } else {
                !row.unknown_platform && row.platform == wanted
            }
        });

        state_match && platform_match && selected_platform_match
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
    DEFAULT_INSPECTOR_PATH_COLUMN_WIDTH, InspectorSortField,
};
use catalogue_bsfree_ui_state::CatalogueBsFreeUiState;
use database_load::{
    CachedLibrarySnapshot, DatabaseGeneration, DatabaseLoadError, DatabaseLoadResult,
    DatabaseMessage, DatabaseOutcome, DatabaseState, classify_unhealthy_database,
    load_database_snapshot, load_database_snapshot_at, load_snapshot_from, start_database_load,
};
use doctor_repair_state::DoctorRepairState;
use emulator_readiness_state::EmulatorReadinessState;
use health_duplicate_ui_state::HealthDuplicateUiState;
use live_library_controller::{
    LiveLibraryPoll, LoadMessage, LoadResult, LoadState, poll_load, start_load,
};
use mount_operation_controller::{
    ArchiveAction, CleanupOutcome, OperationFailure, OperationProgress, OperationRequest,
    OperationResult, OperationSuccess, RunningOperation, cleanup_completed_message,
    perform_archive_action, record_cleanup_finished_activity, record_cleanup_started_activity,
    run_unmount_with_cleanup,
};
use mount_ui_state::MountUiState;
use selected_evidence_ui_state::SelectedEvidenceUiState;

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DuplicateReviewFilters {
    search: String,
    platform: Option<String>,
    include_missing: bool,
    more_than_two: bool,
}

impl DuplicateReviewFilters {
    fn initial() -> Self {
        Self {
            include_missing: true,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DuplicateSortField {
    #[default]
    Title,
    Platform,
    Entries,
    KnownSize,
}

impl std::fmt::Display for DuplicateSortField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Title => "Title",
            Self::Platform => "Platform",
            Self::Entries => "Number of entries",
            Self::KnownSize => "Total known size",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct DuplicateGroupIdentity {
    normalized_title: String,
    platform: String,
}

impl From<&CatalogueDuplicateGroup> for DuplicateGroupIdentity {
    fn from(group: &CatalogueDuplicateGroup) -> Self {
        Self {
            normalized_title: group.normalized_title.clone(),
            platform: group.platform.clone(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HealthIssueFilter {
    #[default]
    All,
    Missing,
    MountFailures,
    Retryable,
    Terminal,
    Historical,
    NoMountRequired,
    NeedsContext,
    AwaitingValidation,
    CachedOnly,
    RecoveryAvailable,
    UnknownPlatform,
}

impl HealthIssueFilter {
    const ALL: [Self; 12] = [
        Self::All,
        Self::Missing,
        Self::MountFailures,
        Self::Retryable,
        Self::Terminal,
        Self::Historical,
        Self::NoMountRequired,
        Self::NeedsContext,
        Self::AwaitingValidation,
        Self::CachedOnly,
        Self::RecoveryAvailable,
        Self::UnknownPlatform,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All issues",
            Self::Missing => "Missing",
            Self::MountFailures => "Mount failures",
            Self::Retryable => "Retryable",
            Self::Terminal => "Terminal",
            Self::Historical => "Historical mount failures",
            Self::NoMountRequired => "No mount required",
            Self::NeedsContext => "Needs context",
            Self::AwaitingValidation => "Awaiting validation",
            Self::CachedOnly => "Cached-only",
            Self::RecoveryAvailable => "Recovery available",
            Self::UnknownPlatform => "Unknown platform",
        }
    }

    fn matches(self, category: HealthCategory) -> bool {
        match self {
            Self::All => true,
            Self::Missing => category == HealthCategory::Missing,
            Self::MountFailures => matches!(
                category,
                HealthCategory::TerminalFailure | HealthCategory::RetryableFailure
            ),
            Self::Retryable => category == HealthCategory::RetryableFailure,
            Self::Terminal => category == HealthCategory::TerminalFailure,
            Self::Historical => category == HealthCategory::HistoricalMountFailure,
            Self::NoMountRequired => category == HealthCategory::MountNotRequired,
            Self::NeedsContext => category == HealthCategory::MountFailureEvidenceInsufficient,
            Self::AwaitingValidation => category == HealthCategory::AwaitingValidation,
            Self::CachedOnly => category == HealthCategory::CachedOnly,
            Self::RecoveryAvailable => category == HealthCategory::RecoveryAvailable,
            Self::UnknownPlatform => category == HealthCategory::UnknownPlatform,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HealthDashboardFilters {
    search: String,
    platform: Option<String>,
    category: HealthIssueFilter,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HealthSortField {
    #[default]
    Severity,
    Path,
    Platform,
    State,
    Reason,
}

impl std::fmt::Display for HealthSortField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Severity => "Severity",
            Self::Path => "Archive path",
            Self::Platform => "Platform",
            Self::State => "State",
            Self::Reason => "Reason",
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HealthReportCacheKey {
    live_data_ptr: Option<usize>,
    database_snapshot_ptr: Option<usize>,
    diagnostics_generation: RefreshGeneration,
}

/// The Health Dashboard's cached report - see `cached_health_issues`.
/// Recovery offers are compared by content (`HashSet::eq`), not identity:
/// `lazy_unmount_offers`/`remount_offers` are mutated in place across
/// several scattered call sites (individual and batch mount/unmount/
/// remount/lazy-unmount completion), so no single pointer or generation
/// bump could reliably cover all of them without being easy to miss one.
/// Both sets are always small (bounded by archives with an active
/// recovery offer this session), so cloning and comparing them each frame
/// is negligible next to the cost `build_health_issues` would pay to
/// actually rebuild.
struct HealthReportCache {
    key: HealthReportCacheKey,
    lazy_unmount_offers: HashSet<PathBuf>,
    remount_offers: HashSet<PathBuf>,
    issues: Vec<HealthIssue>,
}
/// Every top-level destination the app can show. `Health`, `Duplicates`,
/// and `LibraryViews` are **compatibility dispatch keys**, not separate
/// sidebar destinations any more (see `LibraryTab`): they exist purely so
/// `self.view` (still the single source of truth for what actually
/// renders) can name which Library tab is active without a second,
/// parallel field. Each maps 1:1 to a `LibraryTab` via
/// `library_tab_for_main_view`/`main_view_for_library_tab`.
///
/// Kept as real enum variants (Library IA migration Phase 3 decision,
/// evidence in docs/GUI_SIMPLIFICATION.md's "Library IA migration -
/// Phase 3" section) rather than removed and replaced with `LibraryTab`
/// alone: production code still keys the shell's content dispatch off
/// `self.view` matching them (`library_tab_for_main_view`,
/// `main_view_title`, `main_view_content_width`,
/// `main_view_uses_page_scroll` all still need an exhaustive `MainView`
/// match), and 50+ existing tests across three milestones construct or
/// compare against these three variants directly. No persisted,
/// external, or CLI state depends on them - the only reasons to keep
/// them are internal (production dispatch + test surface), not
/// backward-compatibility with anything outside this process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum MainView {
    #[default]
    Home,
    NeedsAttention,
    Library,
    /// Read-only view over already-gathered Ready-to-Play projections.
    ReadyToPlay,
    RecentlyFound,
    Health,
    Duplicates,
    Sources,
    /// Collection Discovery's content, dispatched as the "Discovery" tab of
    /// the consolidated Sources destination - see `sources_tab_for_main_view`.
    /// Was previously `ToolsOverlay::CollectionDiscovery`, a completely
    /// separate rendering mechanism reached only from its own now-removed
    /// sidebar row; folding it into `MainView` lets it share Sources' tab
    /// chrome like `DatSources`/`CheatSources` already do. The underlying
    /// renderer (`collection_discovery_page::show_collection_discovery_panel`)
    /// is unchanged.
    SourcesDiscovery,
    LibraryViews,
    Mount,
    Selected,
    CheatsMods,
    /// The registered cheat sources: which are consulted, in what order, and
    /// for which platforms. Its own destination rather than a section of
    /// Cheats & Mods, because it is configuration that outlives any one
    /// archive being worked on.
    CheatSources,
    /// Canonical organisation: planning and (only after explicit approval)
    /// applying moves of identified games into a configured master ROM root.
    CanonicalOrganisation,
    /// Read-only frontend publisher projections over the existing Playing
    /// Library / 1G1R plan. This destination has no execution path.
    PublisherProfiles,
    /// Evidence-backed filename cleanup for one chosen library folder. This
    /// is a task-oriented entry point over the existing DAT audit, rename
    /// plan, review, and journalled apply flow; DAT Sources remains the
    /// advanced catalogue-management page.
    IdentifyRename,
    /// Repair Review: preview-only review of a saved whole-library repair
    /// plan. Loads a `LibraryRepairPlan` produced by the CLI's
    /// `repair scan --plan-out` contract and shows its proposals. Nothing is
    /// applied from this page.
    RepairReview,
    /// Repair History: recent rename transactions journaled through the
    /// Repair Center (and any other flow sharing the same journal
    /// directory), with reverify status and safe undo when the core proves
    /// a transaction is reversible.
    //
    // Still routed and rendered (every `MainView` match handles it) and
    // exercised by the navigation tests, but since the 0.8.1 consolidation
    // it is reached as a tab within Problems & Repair rather than assigned
    // as a top-level `view`, so production code no longer constructs it
    // directly.
    #[allow(dead_code)]
    RepairHistory,
    /// Duplicate Finder: a DAT-independent duplicate/equivalent-content scan
    /// (`archivefs_core::repair::exact_duplicate`, plus the N64 and optical
    /// equivalent scanners) with evidence-backed canonical-copy selection and
    /// multi-file (CUE/GDI/M3U) protection, quarantined through the same
    /// transaction/journal/rollback engine every other repair flow already
    /// uses.
    ///
    /// Since 0.8.1's "core workflows directly discoverable" pass this is a
    /// first-class destination with its own sidebar and top-menu entry
    /// ("Duplicate Finder") - it is no longer routed through
    /// `ProblemsRepairTab::Repair` (`problems_repair_tab_for_main_view` no
    /// longer maps it), so arriving here never shows Repair Review / Repair
    /// History framing. Deliberately a separate destination from
    /// `MainView::Duplicates` (a read-only Library-tab duplicate viewer over
    /// a different, DAT-relative notion of "duplicate") - the two are
    /// unrelated and never share state.
    ExactDuplicateReview,
    /// Disc Conversion: verified CUE/BIN -> CHD conversion
    /// (`optical_conversion_page` over `archivefs_core::repair`'s
    /// `build_chd_conversion_plan` / `execute_chd_conversion` /
    /// `rollback_chd_conversion`). A first-class destination with its own
    /// sidebar and top-menu entry - the user never has to conceptually enter
    /// "Repair" to convert a disc image. Reuses the exact same
    /// `OpticalConversionPageState` and backend the Repair tab used before.
    DiscConversion,
    /// Read-only catalogue-backed storage usage and future conversion analysis.
    StorageHealth,
    /// Read-only presentation of bounded tape analysis for the selected game.
    TapeInspector,
    /// Emulator Setup: the read-only emulator readiness / profile check.
    /// Renders `doctor_page::show_doctor_page` over the shared
    /// `ArchiveFsApp::doctor_scan` - the same engine and state the Problems &
    /// Repair -> Diagnostics tab uses (no second scan, no divergent state) -
    /// but presented as a dedicated, clearly-named destination so emulator
    /// setup is discoverable without going through "Problems & Repair". The
    /// Doctor scan's "Emulators" and "Emulator profiles" categories carry the
    /// per-emulator rows.
    EmulatorSetup,
    /// Read-only inventory of installed emulator versions and channels.
    EmulatorInventory,
    /// Read-only master BIOS inventory and emulator projection planner.
    BiosProjection,
    /// Curated, read-only collection view backed by the loaded catalogue and
    /// existing evidence/artwork state.
    Museum,
    /// Library View History: a read-only view of the durable, append-only
    /// Library View apply/remove history
    /// (`archivefs_core::library_view_history`), re-read from disk on every
    /// visit/refresh. Deliberately distinct from `HistoryLogs`, which shows
    /// the in-memory `OperationHistory` recent-activity log that does not
    /// survive a restart - this page never touches that log.
    LibraryViewHistory,
    /// The registered DAT catalogues: which local DAT files and folders
    /// EmuWiz can check a library against. Its own destination for the
    /// same reason Cheat Sources is: it is configuration that outlives any
    /// one archive being worked on.
    DatSources,
    /// Read-only presentation of the existing media-set topology and swap
    /// plan. This page derives only from the loaded catalogue snapshot.
    MediaSets,
    ActiveMounts,
    /// The consolidated "Problems & Repair" destination: one sidebar entry
    /// over Overview/Diagnostics/Repair tabs - see `problems_repair_page`'s
    /// module doc. `Doctor`/`RepairReview`/`RepairHistory` below remain the
    /// actual rendering destinations each tab lands on (their own engines
    /// are untouched); `Problems` itself renders only the Overview tab and
    /// the shared tab chrome. `problems_repair_tab_for_main_view` is the
    /// `LibraryTab`-style projection tying all four together.
    Problems,
    Doctor,
    HistoryLogs,
    Settings,
    About,
}

/// The five lenses onto Library data, now visibly unified as tabs of one
/// Library page (see docs/GUI_SIMPLIFICATION.md's "Library IA migration"
/// section) even though each still has its own `MainView` variant and
/// render function underneath, retained for compatibility - see
/// `ArchiveFsApp::update`'s central-panel dispatch, where all five are
/// rendered from one block instead of five separate ones. `Archives`
/// corresponds to `MainView::Library` (the archive table); `Views`
/// corresponds to `MainView::LibraryViews` (saved library views) - named
/// differently from its `MainView` variant because "Library Views" would
/// read twice as "Library" now that it is a tab labelled "Library".
///
/// # Synchronization rule
///
/// `ArchiveFsApp::view` (`MainView`) remains the single source of truth
/// for which underlying render function actually runs - unchanged.
/// `ArchiveFsApp::library_tab` (`LibraryTab`) is a *derived* projection of
/// it: once per frame, before anything renders,
/// `library_tab_for_main_view(self.view)` is consulted, and if `self.view`
/// is one of the five Library-related destinations, `self.library_tab` is
/// set to match. If `self.view` is anything else (Mount, Settings, ...),
/// `self.library_tab` is left untouched, so it keeps remembering the last
/// Library tab visited. The unified Library shell then reads
/// `self.library_tab` to decide which tab's content to render.
///
/// This makes every existing way of navigating to a Library destination -
/// the sidebar's single "Library" button, or any of the ~11 scattered
/// `self.view = MainView::X` assignments elsewhere in the app - a correct
/// "legacy route" into the right `LibraryTab` automatically, with no call
/// site needing to know `LibraryTab` exists. The only sanctioned way to
/// write `library_tab` going the other direction (choosing a tab and
/// having `view` follow) is `ArchiveFsApp::navigate_to_library_tab`,
/// which the shell's `tab_row` calls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum LibraryTab {
    #[default]
    Archives,
    Health,
    Duplicates,
    Views,
    RecentlyFound,
}

/// Compresses `DoctorScanState` into the `home_page::SetupCheckSummary` the
/// Home "Set up emulators" card shows. The card's action opens Problems &
/// Repair -> Diagnostics, which renders this exact `doctor_scan` state, so
/// badge and page can never disagree. A pass is reported only for a
/// completed, clean run that actually performed at least one check - a
/// never-run, in-flight, or zero-checks state is never a pass.
fn setup_check_summary(state: &DoctorScanState) -> home_page::SetupCheckSummary {
    use home_page::SetupCheckSummary;
    match state.displayed() {
        None => {
            if state.is_running() {
                SetupCheckSummary::Running
            } else {
                SetupCheckSummary::NeverRun
            }
        }
        Some(outcome) => {
            let scan = &outcome.scan;
            if scan.checked_subsystems().is_empty() {
                SetupCheckSummary::NoChecksRun
            } else if scan.is_healthy() {
                SetupCheckSummary::Healthy
            } else {
                let blocking = scan.blocking_count();
                if blocking > 0 {
                    SetupCheckSummary::NeedsAttention(blocking)
                } else {
                    SetupCheckSummary::Warnings(scan.findings.len())
                }
            }
        }
    }
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

/// The three tabs of the consolidated "Problems & Repair" destination -
/// see `MainView::Problems`'s doc comment and `problems_repair_page`'s
/// module doc. Mirrors `LibraryTab` exactly: `ArchiveFsApp::view` remains
/// the single source of truth for which underlying render function runs;
/// `ArchiveFsApp::problems_repair_tab` is a *derived* projection of it via
/// `problems_repair_tab_for_main_view`, reconciled once per frame
/// (`reconcile_problems_repair_tab`) exactly like `reconcile_library_tab`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum ProblemsRepairTab {
    #[default]
    Overview,
    Diagnostics,
    Repair,
}

/// The four tabs of the consolidated "Sources" destination - see
/// `MainView::Sources`'s sibling variants below and `sources_page`'s module
/// doc. Mirrors `LibraryTab`/`ProblemsRepairTab` exactly: `ArchiveFsApp::view`
/// remains the single source of truth; `ArchiveFsApp::sources_tab` is a
/// *derived* projection of it via `sources_tab_for_main_view`, reconciled
/// once per frame (`reconcile_sources_tab`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum SourcesTab {
    #[default]
    Libraries,
    Dats,
    Cheats,
    Discovery,
}

/// The unified Library shell's chrome: the shared "Library" heading and
/// the five-tab selector, rendered identically regardless of which tab is
/// selected. Content dispatch (`match self.library_tab { ... }`) stays in
/// `ArchiveFsApp::update`'s central-panel closure, since each arm needs
/// direct `&mut self` field access the existing per-page renderers
/// already require (`self.health_duplicate_ui.health_filters`, `self.health_duplicate_ui.duplicate_filters`,
/// `self.library_views`, ...) - bundling all of that into this function's
/// parameters would mean exactly the giant parameter-heavy universal
/// renderer this milestone was asked to avoid. Broken out on its own so
/// the chrome itself - which tabs render, in which order, with which
/// labels, and that a click returns the right `LibraryTab` - is directly
/// testable without going through a full `eframe::App::update` call.
fn show_library_shell_header(ui: &mut egui::Ui, current_tab: LibraryTab) -> Option<LibraryTab> {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::GAMES,
        "My Games",
        "Browse and manage your game library.",
    );
    let tab_options: [(LibraryTab, &str); 5] = [
        (
            LibraryTab::Archives,
            library_tab_label(LibraryTab::Archives),
        ),
        (LibraryTab::Health, library_tab_label(LibraryTab::Health)),
        (
            LibraryTab::Duplicates,
            library_tab_label(LibraryTab::Duplicates),
        ),
        (LibraryTab::Views, library_tab_label(LibraryTab::Views)),
        (
            LibraryTab::RecentlyFound,
            library_tab_label(LibraryTab::RecentlyFound),
        ),
    ];
    let clicked = widgets::tab_row(ui, &tab_options, current_tab);
    ui.add_space(8.0);
    clicked
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ToolsOverlay {
    #[default]
    None,
    Diagnostics,
    PlatformAliases,
    DatabaseStatus,
    DoctorChecks,
    ArchiveInspector,
    /// First-run onboarding (`onboarding.rs`): a thin step tracker that
    /// takes over the central panel exactly like `Diagnostics` does, but
    /// dispatches its body per-step to the real Sources/DAT Sources/
    /// Emulator Setup page methods rather than one fixed renderer.
    Onboarding,
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

struct ArchiveFsApp {
    state: LoadState,
    library_ui: LibraryUiState,
    /// The sole owner of primary archive identity across Library, Selected,
    /// Mount, and Cheats & Mods. Mount state remains derived from live
    /// records and is intentionally not stored here.
    archive_context: ArchiveContext,
    mount_ui: MountUiState,
    /// The History & Logs page's filter/sort state.
    history_filters: HistoryLogFilters,
    shared_history: SharedHistoryState,
    shared_history_operation: Option<String>,
    shared_rollback: SharedRollbackState,
    doctor_repair: DoctorRepairState,
    emulator_readiness: EmulatorReadinessState,
    /// The Tape Inspector library browser's persisted search/platform/format
    /// filter state, retained across page re-renders.
    tape_inspector_filter: tape_analysis_page::LibraryTapeFilterState,
    /// The full-page Cheats & Mods workspace's current archive and
    /// trusted-catalogue state. It survives ordinary page navigation so
    /// returning to the same exact archive does not discard a completed
    /// cache inspection or retrieval.
    cheat_workflow: Option<CheatWorkflowState>,
    /// Independent read-only review state for user-supplied .cht/.pnach files.
    user_cheat_import_page: user_cheat_import_page::UserCheatImportPageState,
    /// The Dolphin texture-mod panel's own state - deliberately separate
    /// from `cheat_workflow` (a texture mod is not a cheat) and keyed by
    /// `{ archive_path, profile_id, verified_game_id }` internally, so it
    /// never reuses state describing a different game or profile. See
    /// `dolphin_texture_mod_page`'s own module doc comment.
    dolphin_texture_mod: dolphin_texture_mod_page::DolphinTextureModPageState,
    local_mod_package: local_mod_package_page::LocalModPackagePageState,
    /// The Launch Readiness panel's "Launch RetroArch" tracker - see
    /// `launch_readiness_page`'s own module doc comment. Deliberately not
    /// reset on archive/page navigation: a running or just-exited process
    /// must still be reaped/shown correctly even after the user selects a
    /// different game.
    launch_retroarch: launch_readiness_page::RetroArchLaunchState,
    /// The Launch Readiness panel's "Launch Dolphin" tracker - the same
    /// reasoning as `launch_retroarch` above applies unchanged.
    launch_dolphin: launch_readiness_page::DolphinLaunchState,
    /// The Launch Readiness panel's "Launch PCSX2" tracker - the same
    /// reasoning as `launch_retroarch` above applies unchanged.
    launch_pcsx2: launch_readiness_page::Pcsx2LaunchState,
    launch_standalone: launch_readiness_page::StandaloneLaunchState,
    launch_amiga_whdload: launch_readiness_page::AmigaWHDLoadLaunchState,
    /// A tentative archive choice is isolated here until the picker is
    /// applied. It never mutates Library focus or multi-selection.
    cheat_archive_picker: Option<CheatArchivePickerState>,
    /// A different archive requires confirmation when fetched catalogue
    /// state would otherwise be discarded.
    confirm_cheat_archive_change: Option<PathBuf>,
    feedback: Option<ActionFeedback>,
    history: OperationHistory,

    /// Set once this session has seen a diagnostics report where the
    /// config file was confirmed present and readable. Lets the Setup
    /// screen tell a genuine first run apart from a config that
    /// disappeared after previously being found - the latter may mean
    /// something was deleted, unmounted, or is otherwise a real problem,
    /// and must never be presented with the same reassuring "you have not
    /// configured this yet" framing as a fresh install.

    /// First-run onboarding: loaded once at startup from
    /// `onboarding_state.txt` (see `onboarding.rs`), advanced only through
    /// `onboarding::*` helper methods, and persisted back on every
    /// transition. Never duplicates source/DAT/emulator state - it only
    /// tracks which step of the guided tour the user is on.

    /// One-shot: whether the auto-open check (first genuine run only, see
    /// `maybe_auto_open_onboarding`) has already run this session.

    /// Doctor Stage 1A: the current read-only diagnostic scan. Entirely
    /// separate from `self.state`/`self.refresh`, so running Doctor never
    /// reloads the application.

    /// The Cheat Sources page, loaded lazily the first time it is opened so
    /// that starting the GUI never reads the preferences file for a page the
    /// user has not visited.
    cheat_sources_page: Option<cheat_sources_page::CheatSourcesPageState>,
    /// Read-only review of core-produced duplicate/conflict reports.
    cheat_reconciliation_review: cheat_reconciliation_review::CheatReconciliationReviewState,
    /// The browse-only CheatBase panel embedded in Cheats & Mods. Its setup,
    /// search, and inspection work is explicit and independent of emulator
    /// cheat installation workflows.
    cheatbase_page: cheatbase_page::CheatBasePageState,
    /// The approval-bound managed-emulator download section shown inside
    /// Emulator Setup. Downloads only run after an explicit confirmation
    /// click; installing an emulator never implies it is launch-ready.
    emulator_download_page: emulator_download_page::EmulatorDownloadPageState,
    rom_organisation_page: Option<rom_organisation_page::RomOrganisationPageState>,
    publisher_profile_page: Option<publisher_profile_page::PublisherProfilePageState>,
    /// The Repair Review page, loaded lazily on first visit. Preview-only;
    /// it never applies anything.
    repair_review_page: Option<repair_review_page::RepairReviewPageState>,
    /// The Repair History page, loaded lazily on first visit: recent rename
    /// transactions journaled through the Repair Center, re-read from disk
    /// on every refresh.
    repair_history_page: Option<repair_history_page::RepairHistoryPageState>,
    /// The Exact Duplicate Review page, loaded lazily on first visit:
    /// starts with no source folder chosen and no scan run, exactly like
    /// `RepairReviewPageState::default()` starts with no plan loaded.
    exact_duplicate_review_page: Option<exact_duplicate_review_page::ExactDuplicateReviewPageState>,
    optical_conversion_page: Option<optical_conversion_page::OpticalConversionPageState>,
    storage_health_page: storage_health_page::StorageHealthPageState,
    /// The Library View History page, loaded lazily on first visit:
    /// durable Library View apply/remove records, re-read from disk on
    /// every refresh. Distinct from `history` (`OperationHistory`) below,
    /// which is in-memory only.
    library_view_history_page: Option<library_view_history_page::LibraryViewHistoryPageState>,
    /// Unsubmitted Cheat Sources text and disclosure state. Held here rather
    /// than in the page state because none of it is policy - see
    /// `CheatSourcesPageUi`.
    cheat_sources_ui: cheat_sources_page::CheatSourcesPageUi,
    /// The DAT Sources page, loaded lazily on first visit for the same reason
    /// Cheat Sources is: starting the GUI should not read a registry file for
    /// a page nobody has opened.
    dat_sources_page: Option<dat_sources_page::DatSourcesPageState>,
    media_sets_page: media_sets_page::MediaSetsPageState,
    quick_rename_mode: bool,
    /// Unsubmitted DAT Sources text and disclosure state. Held here rather
    /// than in the page state because none of it is policy.
    dat_sources_ui: dat_sources_page::DatSourcesPageUi,
    /// The finding whose evidence panel is open, by stable finding id.

    /// The repair awaiting confirmation, if any.

    /// The most recent repair result, kept on screen next to the finding it
    /// was for.

    /// When the last repair finished, alongside (never replacing) the scan's
    /// own timestamp.
    setup_action: Option<RunningSetupAction>,
    refresh_error: Option<String>,
    snapshot_stale: bool,
    refresh_generation: RefreshGeneration,
    snapshot_generation: Option<RefreshGeneration>,
    database_state: DatabaseState,
    database_generation: DatabaseGeneration,
    needs_attention: needs_attention::AttentionWorkspace,
    dat_authority: dat_authority_dashboard::DashboardState,
    /// A `ScanPersistSummary` from a just-completed Sources-page scan
    /// (`SourceActionOutcome::Scanned`), waiting to be carried into the
    /// `DatabaseState::Ready.last_scan_summary` produced by the plain
    /// snapshot reload that `poll_source_action` always triggers afterward
    /// (`DatabaseOutcome::Loaded`, which otherwise has no scan summary of
    /// its own). Consumed (taken) by the very next `poll_database_load`
    /// completion regardless of its outcome, so a summary can never attach
    /// to an unrelated, later reload.
    pending_source_scan_summary: Option<ScanPersistSummary>,
    /// The Sources page's persistent echo of its most recent scan result
    /// (see [`SourcesLastScan`]) - unlike `pending_source_scan_summary`
    /// above, this is never consumed/cleared by a reload; it stays visible
    /// on the Sources page until superseded by a newer Sources-page scan.
    sources_last_scan: Option<SourcesLastScan>,
    health_duplicate_ui: HealthDuplicateUiState,
    /// The real OS clipboard backing every text field's context menu -
    /// see `NativeClipboard`'s doc comment for why this is kept for the
    /// app's whole lifetime rather than opened per click.
    clipboard: NativeClipboard,
    /// Which of the four primary destinations is currently showing - see
    /// `MainView`'s doc comment. Never reset except by an explicit
    /// navigation click; every page's own state (filters/sort/selection)
    /// lives in its own fields below, independent of this one.
    view: MainView,
    /// The last Library-area tab the user was on - see `LibraryTab`'s doc
    /// comment for the synchronization rule with `view`. Drives which tab
    /// the unified Library shell shows.
    library_tab: LibraryTab,
    /// The last "Problems & Repair" tab the user was on - see
    /// `ProblemsRepairTab`'s doc comment for the synchronization rule with
    /// `view`, identical to `library_tab`'s.
    problems_repair_tab: ProblemsRepairTab,
    /// The last "Sources" tab the user was on - see `SourcesTab`'s doc
    /// comment for the synchronization rule with `view`, identical to
    /// `library_tab`'s.
    sources_tab: SourcesTab,
    /// Which "Tools" screen (if any) is showing in front of `view` - see
    /// `ToolsOverlay`'s doc comment.
    tools_overlay: ToolsOverlay,
    show_activity: bool,
    /// Whether the Help "About EmuWiz" window is open.
    show_about: bool,
    /// Whether the "Skipped files" drill-down window (opened from the
    /// Database Status overlay's "Skipped N" detail) is open. Read-only:
    /// opening it never re-scans, re-classifies, or mutates anything - it
    /// only displays `ScanPersistSummary::skipped_files` from the most
    /// recently completed scan this session already produced.
    show_skipped_files: bool,
    /// The active reason filter for the skipped-files window. `None` is
    /// "All reasons".
    skipped_files_filter: Option<archivefs_core::SkipReason>,
    /// A one-shot signal from the Library menu's "Select all visible" item.
    /// `show_loaded_data` consumes and clears it the same way it already
    /// consumes a Ctrl+A keypress or the inline button, calling the exact
    /// same `select_all_visible` helper (see its own call site). Needed
    /// because the menu bar renders before `show_loaded_data` computes
    /// this frame's `visible_indices`, so the request cannot be applied
    /// directly from the menu's own click handler.
    select_all_visible_requested: bool,
    /// The Sources page's currently running background action, if any -
    /// mirrors `alias_action` exactly, including the "one writer at a
    /// time" convention `source_action_available` enforces.
    source_action: Option<RunningSourceAction>,
    /// A folder picked for the temporary preparation root but not yet
    /// applied. Picking or cancelling never writes config.toml.
    mount_root_draft: Option<PathBuf>,
    /// The visible outcome of the most recent "Apply folder" for the
    /// temporary preparation root, rendered in the Sources -> Libraries
    /// mount-root card. Set from the background `SetupAction::SetMountRoot`
    /// result; cleared when a new apply starts.
    mount_root_feedback: Option<sources_page::MountRootFeedback>,
    catalogue_bsfree_ui: CatalogueBsFreeUiState,
    /// Loaded once for GUI use. RomM rendering and cached browsing borrow this
    /// snapshot instead of reading `config.toml` on every frame.
    gui_config: GuiConfigSnapshot,
    /// The last authoritative RomM snapshot. `None` until the first status load,
    /// so the card shows "reading" rather than a screenful of zeroes.
    romm_ui: RommUiState,
    selected_evidence_ui: SelectedEvidenceUiState,
    /// The "Add Folder" dialog's open/closed state and its own fields -
    /// see `SourcesAddDialogState`.
    sources_add_dialog: Option<SourcesAddDialogState>,
    /// Set when Gamer View's first-run "Add games" action dispatches a
    /// `SourceAction::Add` for this exact path - so the resulting
    /// `SourceActionOutcome::Added` knows to immediately chain a
    /// `SourceAction::ScanOne` for the same folder (one seamless "pick a
    /// folder, see your games" flow, reusing the existing Sources
    /// add/scan machinery unchanged rather than duplicating it) instead of
    /// leaving a newly-added, never-scanned source silently empty. Cleared
    /// once the chained scan is started, so a normal Advanced View Sources
    /// page "Add" never chains an unwanted scan.
    gamer_view_pending_first_scan: Option<PathBuf>,
    /// Set when a scan requested from Gamer View finishes with existing
    /// skipped/ambiguous/failed detail that the user can review in Sources ->
    /// Discovery. This is presentation state only; the scan itself is still
    /// the shared SourceAction::ScanAll/ScanOne path.
    gamer_view_scan_review_available: bool,
    /// Distinguishes a Gamer View scan from a scan started elsewhere while
    /// the shared source worker is running, so its completion can use the
    /// beginner-facing summary without changing scan semantics.
    gamer_view_scan_pending_review: bool,
    /// The Remove-source confirmation dialog's open/closed state - see
    /// `SourcesRemoveDialogState`.
    sources_remove_dialog: Option<SourcesRemoveDialogState>,
    /// Every configured Library View - loaded at startup and refreshed
    /// after every add/edit/enable/disable/remove action completes (see
    /// `reload_library_views`). Independent of `database_state`'s cached
    /// catalogue snapshot: views are a small flat config file, not a
    /// derived database read, so they are never stale behind a scan.
    library_views: Vec<LibraryViewConfig>,
    /// The Library Views page's currently running background action, if
    /// any - mirrors `source_action` exactly, including the "one writer at
    /// a time" convention.
    library_view_action: Option<RunningLibraryViewAction>,
    /// The most recently computed Preview for a view, if any - both what
    /// the Library Views page shows in its plan table and what "Apply"/
    /// "Repair" act on for that view, and what the Library page's "Show in
    /// Library View preview" hook (see `RowContextMenuAction`) reads to
    /// decide whether "Copy planned view path" is available for a given
    /// archive. Cleared whenever a different view is previewed, or after
    /// Apply/Repair/Remove changes the view it belongs to (its plan may no
    /// longer be accurate).
    library_view_last_plan: Option<(LibraryViewConfig, LibraryViewPlan)>,
    /// The Add View / Edit View dialog's open/closed state and its own
    /// fields - see `LibraryViewFormDialogState`. `editing_id` distinguishes
    /// the two (`None` = Add, `Some(id)` = Edit), following the same
    /// "one `Option` field is the dialog's open/closed flag" convention as
    /// every other dialog in this app.
    library_view_form_dialog: Option<LibraryViewFormDialogState>,
    /// The Remove-view confirmation dialog's open/closed state - see
    /// `LibraryViewRemoveDialogState`.
    library_view_remove_dialog: Option<LibraryViewRemoveDialogState>,
    /// The Library page's "Show in Library View preview" hook - the exact
    /// archive path the Library Views page shows a read-only status banner
    /// for once navigated there (see `library_view_planned_entry_for`).
    /// Unlike a one-shot flag, this deliberately persists across frames -
    /// clearing it the instant it is shown would make the banner disappear
    /// before the user could read it, since every frame re-renders. It is
    /// only ever replaced by a newer "Show in Library View preview" click.
    library_view_focus_archive: Option<PathBuf>,
    /// The Library Views page's Preview details filter - see
    /// `LibraryViewPlanFilter`.
    library_view_plan_filter: LibraryViewPlanFilter,
    /// The Archive Inspector overlay's state for whichever archive it was
    /// last opened for, if any - `None` means it has never been opened
    /// this session. Independent of `tools_overlay`: closing the overlay
    /// (setting `tools_overlay` back to `None`) deliberately leaves this
    /// as-is, so reopening it shows the same archive's already-loaded
    /// report instead of re-inspecting from scratch.
    archive_inspector: Option<ArchiveInspectorState>,
    archive_inspector_generation: RefreshGeneration,
    archive_preparation: ArchivePreparationState,
    archive_preparation_generation: RefreshGeneration,
    /// docs/GUI_NAVIGATION_RESET_DESIGN.md's mode switch - a view-layer
    /// concept only, persisted independently of every other field (see
    /// `load_gui_mode`/`save_gui_mode`).
    ui_mode: GuiMode,
    /// Which of Gamer View's two screens is showing - never persisted.
    gamer_view_screen: GamerViewScreen,
    /// The typed count for Mount All's >25-item confirmation gate - see
    /// `bulk_action_confirm_enabled`. Cleared whenever the dialog closes.
    missing_removal_typed_count: String,
    /// Row-context-menu "Mount selected" (audit finding: this previously
    /// dispatched with no confirmation at all, unlike "Unmount selected").
    /// The exact paths are re-derived fresh from the live snapshot at
    /// confirm time, never trusted from when the dialog opened.
    /// Bulk platform assignment/clear (audit finding: this previously
    /// dispatched instantly with no confirmation at all, from both the
    /// selection action bar and the row context menu).
    confirm_bulk_platform_action: Option<(Vec<PathBuf>, BulkPlatformActionKind)>,
    focus_bulk_platform_cancel: bool,
    bulk_platform_action_typed_count: String,
    /// EmuWiz-owned, upgrade-stable custom artwork directory. `None` is
    /// possible only when the operating-system data root cannot be resolved.
    custom_platform_artwork_directory: Option<PathBuf>,
    /// Decoded local artwork and failed-decode fingerprints for this
    /// session. It is invalidated by directory or file-metadata changes.
    platform_artwork_cache: PlatformArtworkCache,
    platform_artwork_manager: PlatformArtworkManagerState,
    platform_artwork: PlatformArtworkManager,
    /// RomM cover artwork for the Gamer View game list: what has been asked
    /// for, what has been answered, and which library generation those
    /// answers belong to. Holds no thread of its own - see `gamer_cover_worker`.
    gamer_covers: crate::gamer_artwork::GamerCoverCache,
    /// Selected-Details RomM screenshots, sharing the cover worker and
    /// ArtworkCache security path while remaining separate from cover slots.
    gamer_screenshots: crate::gamer_artwork::GamerScreenshotCache,
    /// The thread that resolves those covers, started on the first frame that
    /// actually draws the list so a session that never opens Gamer View never
    /// Museum's own navigation state (grid vs. one platform's detail view) -
    /// see `museum_page`'s own module doc.
    museum_page: museum_page::MuseumPageState,
    museum_hero: museum_page::MuseumHeroState,
    /// opens the catalogue. `None` until then.
    gamer_cover_worker: Option<crate::gamer_artwork::CoverWorker>,
    /// Whether a cover worker may be started at all. Always true in the running
    /// application.
    ///
    /// Tests set it false. Starting the worker opens the real per-user identity
    /// cache under `$HOME` and, for a developer who has RomM configured, can
    /// reach their instance - neither of which a `cargo test` run may do. It
    /// also made cover tests racy: the worker answered the very rows the test
    /// was driving by hand, so a reply could overwrite the slot under test
    /// between one frame and the next.
    gamer_cover_worker_allowed: bool,
    /// The `config_identity` the cover cache's answers were resolved against.
    /// A change means the same path may now be a different archive, so every
    /// answer is discarded - see `GamerCoverCache::library_changed`.
    gamer_cover_library: Option<ConfigIdentity>,
    /// Enrichment (synopsis/genre/players/rating/release year) for the
    /// currently selected/featured Gamer View game, if any was found. Holds
    /// at most one game's worth of data - see
    /// `crate::game_metadata::GameMetadataWorker`.
    selected_game_metadata: Option<(PathBuf, crate::game_metadata::GameMetadataResult)>,
    /// The thread that resolves enrichment lookups, started lazily like
    /// `gamer_cover_worker`. `None` until Gamer View first needs it.
    game_metadata_worker: Option<crate::game_metadata::GameMetadataWorker>,
    /// Mirrors `gamer_cover_worker_allowed`: tests set this false so a
    /// `cargo test` run never opens the real per-user identity cache.
    game_metadata_worker_allowed: bool,
    /// The Gamer View browsing rail's A-Z jump strip index - see
    /// [`crate::gamer_view::AlphaJumpIndex`]. Persisted here (like
    /// `gamer_covers`) because it caches a sort/bucket rebuild across
    /// frames, rebuilding only when the visible result set changes.
    gamer_alpha_jump: crate::gamer_view::AlphaJumpIndex,
    es_de_media: crate::es_de_media_state::EsDeMediaState,
    launchbox_local_media: crate::launchbox_local_state::LaunchBoxLocalMediaState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ArtworkManagerFilter {
    #[default]
    All,
    Missing,
    Custom,
    FallbackOnly,
}

enum PlatformArtworkTaskResult {
    Status(Result<archivefs_core::platform_artwork::PlatformArtworkStatus, String>),
    Mutation(Result<String, String>),
    BulkPreview(Result<archivefs_core::platform_artwork::BulkArtworkPreview, String>),
}

#[derive(Default)]
struct PlatformArtworkManagerState {
    search: String,
    filter: ArtworkManagerFilter,
    status: Option<archivefs_core::platform_artwork::PlatformArtworkStatus>,
    bulk_preview: Option<archivefs_core::platform_artwork::BulkArtworkPreview>,
    replace_existing: bool,
    pending_import: Option<(String, PathBuf)>,
    pending_remove: Option<String>,
    message: Option<(bool, String)>,
    task: Option<mpsc::Receiver<PlatformArtworkTaskResult>>,
    /// An in-flight native file dialog, run on a background thread so the egui
    /// frame is never blocked while it is open.
    pending_pick: Option<FilePickRequest>,
}

/// A native image-picker file dialog running on a background thread.
struct FilePickRequest {
    platform_id: String,
    /// Whether this platform already has a custom image (replacement flow).
    custom: bool,
    receiver: mpsc::Receiver<Option<PathBuf>>,
}

/// The outcome of draining one file-picker channel, as a tiny pure state
/// machine so the caller (and tests) reason about it without a real dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FilePickDrain {
    /// The picker is still running - leave `pending_pick` intact.
    Pending,
    /// The user pressed Cancel (`Ok(None)`): clear the picker, change nothing,
    /// show no error.
    Cancelled,
    /// The user chose a file: clear the picker and continue the import.
    Picked(PathBuf),
    /// The picker thread ended without sending a result: clear the picker and
    /// surface a friendly error so the buttons become available again.
    Disconnected,
}

/// Reads one state from the picker channel. Pure and deterministic: no file
/// dialog is required.
fn drain_file_pick(receiver: &mpsc::Receiver<Option<PathBuf>>) -> FilePickDrain {
    match receiver.try_recv() {
        Ok(Some(path)) => FilePickDrain::Picked(path),
        Ok(None) => FilePickDrain::Cancelled,
        Err(mpsc::TryRecvError::Empty) => FilePickDrain::Pending,
        Err(mpsc::TryRecvError::Disconnected) => FilePickDrain::Disconnected,
    }
}

/// The plain-language error shown when the picker thread exits without a
/// result (for example the dialog failed to open).
const FILE_PICKER_DISCONNECTED_MESSAGE: &str =
    "The image picker closed unexpectedly. Please try again.";

impl ArchiveFsApp {
    fn is_busy(&self) -> bool {
        self.mount_ui.operation.is_some()
            || self.mount_ui.mount_all.is_some()
            || self.mount_ui.unmount_all.is_some()
            || self.setup_action.is_some()
    }

    fn new(context: egui::Context) -> Self {
        theme::apply(&context);
        let gui_config = GuiConfigSnapshot::load_default();
        let generation = RefreshGeneration::INITIAL;
        let database_generation = DatabaseGeneration::INITIAL;
        let mut history = OperationHistory::default();
        history.record(HistoryEntry::new(
            ActivityAction::Refresh,
            None,
            ActivityOutcome::Started,
            "Loading your library.",
        ));
        Self {
            state: start_load(context.clone(), generation, None),
            database_state: start_database_load(context.clone(), database_generation, None, false),
            database_generation,
            needs_attention: needs_attention::AttentionWorkspace::default(),
            dat_authority: dat_authority_dashboard::DashboardState::default(),
            pending_source_scan_summary: None,
            sources_last_scan: None,
            cheat_sources_page: None,
            cheat_reconciliation_review:
                cheat_reconciliation_review::CheatReconciliationReviewState::default(),
            cheatbase_page: cheatbase_page::CheatBasePageState::default(),
            emulator_download_page: emulator_download_page::EmulatorDownloadPageState::default(),
            rom_organisation_page: None,
            publisher_profile_page: None,
            repair_review_page: None,
            repair_history_page: None,
            exact_duplicate_review_page: None,
            optical_conversion_page: None,
            library_view_history_page: None,
            cheat_sources_ui: cheat_sources_page::CheatSourcesPageUi::default(),
            dat_sources_page: None,
            media_sets_page: media_sets_page::MediaSetsPageState::default(),
            quick_rename_mode: false,
            dat_sources_ui: dat_sources_page::DatSourcesPageUi::default(),
            archive_context: ArchiveContext::default(),
            library_ui: LibraryUiState::default(),
            mount_ui: MountUiState::default(),
            history_filters: HistoryLogFilters::default(),
            shared_history: SharedHistoryState::NotLoaded,
            shared_history_operation: None,
            shared_rollback: SharedRollbackState::Idle,
            doctor_repair: DoctorRepairState::new(context.clone(), generation),
            emulator_readiness: EmulatorReadinessState::new(),
            storage_health_page: storage_health_page::StorageHealthPageState::default(),
            tape_inspector_filter: tape_analysis_page::LibraryTapeFilterState::default(),
            cheat_workflow: None,
            user_cheat_import_page: user_cheat_import_page::UserCheatImportPageState::default(),
            dolphin_texture_mod: dolphin_texture_mod_page::DolphinTextureModPageState::default(),
            local_mod_package: local_mod_package_page::LocalModPackagePageState::default(),
            launch_retroarch: launch_readiness_page::RetroArchLaunchState::default(),
            launch_dolphin: launch_readiness_page::DolphinLaunchState::default(),
            launch_pcsx2: launch_readiness_page::Pcsx2LaunchState::default(),
            launch_standalone: launch_readiness_page::StandaloneLaunchState::default(),
            launch_amiga_whdload: launch_readiness_page::AmigaWHDLoadLaunchState::default(),
            cheat_archive_picker: None,
            confirm_cheat_archive_change: None,
            feedback: None,
            history,
            setup_action: None,
            refresh_error: None,
            snapshot_stale: false,
            refresh_generation: generation,
            snapshot_generation: None,
            health_duplicate_ui: HealthDuplicateUiState::default(),
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
            mount_root_draft: None,
            mount_root_feedback: None,
            catalogue_bsfree_ui: CatalogueBsFreeUiState::default(),
            gui_config,
            romm_ui: RommUiState::default(),
            selected_evidence_ui: SelectedEvidenceUiState::default(),
            sources_add_dialog: None,
            gamer_view_pending_first_scan: None,
            gamer_view_scan_review_available: false,
            gamer_view_scan_pending_review: false,
            sources_remove_dialog: None,
            library_views: load_library_view_configs_default().unwrap_or_default(),
            library_view_action: None,
            library_view_last_plan: None,
            library_view_form_dialog: None,
            library_view_remove_dialog: None,
            library_view_focus_archive: None,
            library_view_plan_filter: LibraryViewPlanFilter::default(),
            archive_inspector: None,
            archive_inspector_generation: RefreshGeneration::INITIAL,
            archive_preparation: ArchivePreparationState::default(),
            archive_preparation_generation: RefreshGeneration::INITIAL,
            ui_mode: load_gui_mode(),
            gamer_view_screen: GamerViewScreen::default(),
            missing_removal_typed_count: String::new(),
            confirm_bulk_platform_action: None,
            focus_bulk_platform_cancel: false,
            bulk_platform_action_typed_count: String::new(),
            custom_platform_artwork_directory:
                archivefs_core::platform_artwork::default_platform_artwork_root().ok(),
            platform_artwork_cache: PlatformArtworkCache::default(),
            platform_artwork_manager: PlatformArtworkManagerState::default(),
            platform_artwork: PlatformArtworkManager::new(
                archivefs_core::platform_artwork::default_platform_artwork_root().ok(),
                open_folder_in_file_manager,
            ),
            museum_page: museum_page::MuseumPageState::default(),
            museum_hero: museum_page::MuseumHeroState::default(),

            gamer_covers: crate::gamer_artwork::GamerCoverCache::default(),
            gamer_screenshots: crate::gamer_artwork::GamerScreenshotCache::default(),
            gamer_cover_worker: None,
            gamer_cover_worker_allowed: true,
            gamer_cover_library: None,
            selected_game_metadata: None,
            game_metadata_worker: None,
            game_metadata_worker_allowed: true,
            gamer_alpha_jump: crate::gamer_view::AlphaJumpIndex::default(),
            es_de_media: crate::es_de_media_state::EsDeMediaState::default(),
            launchbox_local_media: crate::launchbox_local_state::LaunchBoxLocalMediaState::default(
            ),
        }
    }

    /// The compatibility wrapper for navigating *by tab* - the write
    /// direction LibraryTab's synchronization rule reserves exclusively
    /// for this method (every other call site still navigates by setting
    /// `view` directly, exactly as before). Sets both fields together so
    /// they can never briefly disagree; `tools_overlay` is cleared to
    /// match every other navigation call site's behaviour (see the
    /// sidebar-click handler in `update`). Called by the unified Library
    /// shell's tab_row.
    fn navigate_to_library_tab(&mut self, tab: LibraryTab) {
        self.view = main_view_for_library_tab(tab);
        self.library_tab = tab;
        self.tools_overlay = ToolsOverlay::None;
    }

    fn navigate_to_missing_catalogue_review(&mut self) {
        self.navigate_to_library_tab(LibraryTab::Archives);
        administration_pages::set_missing_review_mode(&mut self.library_ui.library_filters, true);
        self.archive_context.clear_selection();
    }

    /// `ProblemsRepairTab`'s exact counterpart to `navigate_to_library_tab` -
    /// same synchronization rule, same reason for existing (called by the
    /// consolidated page's own tab row).
    fn navigate_to_problems_repair_tab(&mut self, tab: ProblemsRepairTab) {
        self.view = main_view_for_problems_repair_tab(tab);
        self.problems_repair_tab = tab;
        self.tools_overlay = ToolsOverlay::None;
    }

    /// `SourcesTab`'s exact counterpart to `navigate_to_library_tab`.
    fn navigate_to_sources_tab(&mut self, tab: SourcesTab) {
        self.view = main_view_for_sources_tab(tab);
        self.sources_tab = tab;
        self.tools_overlay = ToolsOverlay::None;
    }

    fn navigate_to_home_card(&mut self, card: home_page::HomeCard) {
        match card {
            home_page::HomeCard::RomM => {
                // The RomM provider integration is the RomM source card on
                // Sources -> Libraries. Route there so the card's
                // `romm_snapshot` readiness badge and its destination
                // describe the same subsystem. (The whole-collection Playing
                // Library planner remains reachable honestly via Library
                // Organisation -> "Build Playing Library".) Sidebar and
                // top-menu "RomM" converge on this exact call.
                self.navigate_to_sources_tab(SourcesTab::Libraries);
            }
            home_page::HomeCard::ConvertDiscs => {
                // First-class Disc Conversion destination. The dispatch
                // branch lazily creates `optical_conversion_page`, so no
                // pre-seeding is needed; done here too so the state exists
                // the instant the card is clicked.
                self.navigate_to_main_view(MainView::DiscConversion);
                self.optical_conversion_page.get_or_insert_with(
                    optical_conversion_page::OpticalConversionPageState::default,
                );
            }
            home_page::HomeCard::DuplicateReview => {
                // First-class Duplicate Finder destination - renders
                // standalone, without Repair Review / Repair History.
                self.navigate_to_main_view(MainView::ExactDuplicateReview);
            }
            _ => self.navigate_to_main_view(main_view_for_home_card(card)),
        }
    }

    /// The one sanctioned way to change `self.view` in response to a user
    /// action (a sidebar click, a Home card, a menu item) - shared so
    /// every navigation source applies the same special cases
    /// (`CheatsMods` clears any open tools overlay; `Library` restores
    /// whichever tab was last selected rather than resetting to Archives)
    /// instead of each call site re-deriving them, and so the *last*
    /// navigation call in a frame always wins over anything set earlier
    /// that frame.
    fn navigate_to_main_view(&mut self, target: MainView) {
        if target == MainView::CheatsMods {
            self.view = MainView::CheatsMods;
            self.tools_overlay = ToolsOverlay::None;
        } else if target == MainView::Library {
            self.navigate_to_library_tab(self.library_tab);
        } else if target == MainView::Problems {
            // Restores whichever "Problems & Repair" tab was last selected,
            // exactly like `Library` above - clicking the one sidebar entry
            // a second time should not reset Diagnostics/Repair progress
            // back to Overview.
            self.navigate_to_problems_repair_tab(self.problems_repair_tab);
        } else if target == MainView::Sources {
            // Restores whichever Sources tab was last selected, exactly
            // like `Library`/`Problems` above.
            self.navigate_to_sources_tab(self.sources_tab);
        } else {
            self.view = target;
            self.tools_overlay = ToolsOverlay::None;
        }
    }

    /// Gamer View's gear menu has no sidebar behind it, so "Advanced View"
    /// is the only realistic path a genuine first-run user has to reach
    /// the task list. It must always land on Home, never on whatever
    /// `self.view` happened to hold from an earlier Advanced View visit.
    ///
    /// Deliberately does not persist `ui_mode` itself - the caller does
    /// that (see the gear menu's "Advanced View" handler) - so this method
    /// stays a pure state transition, callable from a test without writing
    /// to the real per-user GUI-mode file.
    fn switch_to_advanced_view_at_home(&mut self) {
        self.ui_mode = GuiMode::AdvancedView;
        self.view = MainView::Home;
        self.tools_overlay = ToolsOverlay::None;
    }

    /// LibraryTab's synchronization rule, applied once. Called at the top
    /// of every frame in `update`, before anything else runs - see
    /// LibraryTab's doc comment for the rule itself. Broken out as its own
    /// method (rather than inlined in `update`) purely so it can be
    /// exercised directly by tests without going through a full
    /// `eframe::App::update` call, which nothing else in this codebase
    /// does either.
    fn reconcile_library_tab(&mut self) {
        if let Some(tab) = library_tab_for_main_view(self.view) {
            self.library_tab = tab;
        }
    }

    /// `ProblemsRepairTab`'s exact counterpart to `reconcile_library_tab`,
    /// called alongside it every frame.
    fn reconcile_problems_repair_tab(&mut self) {
        if let Some(tab) = problems_repair_tab_for_main_view(self.view) {
            self.problems_repair_tab = tab;
        }
    }

    /// `SourcesTab`'s exact counterpart to `reconcile_library_tab`, called
    /// alongside it every frame.
    fn reconcile_sources_tab(&mut self) {
        if let Some(tab) = sources_tab_for_main_view(self.view) {
            self.sources_tab = tab;
        }
    }

    fn refresh(&mut self, context: &egui::Context) {
        self.refresh_generation = self.refresh_generation.next();
        let generation = self.refresh_generation;
        self.history.record(HistoryEntry::new(
            ActivityAction::Refresh,
            None,
            ActivityOutcome::Started,
            "Refreshing your library.",
        ));
        let previous = match std::mem::replace(
            &mut self.state,
            LoadState::Error("refresh starting".to_string()),
        ) {
            LoadState::Ready(data) => Some(data),
            LoadState::Loading { previous, .. } => previous,
            LoadState::Error(_) => None,
        };
        self.refresh_diagnostics(context);
        self.state = start_load(context.clone(), generation, previous);
    }
    fn poll_load(&mut self, _context: &egui::Context) {
        let Some(result) = poll_load(
            &mut self.state,
            self.refresh_generation,
            self.database_state.snapshot(),
        ) else {
            return;
        };
        match result {
            LiveLibraryPoll::Completed { merged_rows } => {
                self.library_ui.filtered_rows =
                    matching_row_indices(&merged_rows, &self.library_ui.filter);
                self.prune_selection(&merged_rows);
                self.refresh_error = None;
                self.snapshot_stale = false;
                self.snapshot_generation = Some(self.refresh_generation);
                self.history.record(HistoryEntry::new(
                    ActivityAction::Refresh,
                    None,
                    ActivityOutcome::Completed,
                    "Your library was refreshed.",
                ));
            }
            LiveLibraryPoll::Failed {
                error,
                has_previous,
            } => {
                self.snapshot_stale = has_previous;
                self.refresh_error = Some(error.clone());
                self.history.record(HistoryEntry::new(
                    ActivityAction::Refresh,
                    None,
                    ActivityOutcome::Failed,
                    error,
                ));
                self.tools_overlay = ToolsOverlay::Diagnostics;
            }
        }
    }

    /// Starts a background database load. `run_scan_first =
    /// true` is "Scan library" (runs `scan_and_persist` before reloading);
    /// `false` is "Refresh database status" / "Retry database load" (a
    /// read-only reload). Never blocks the UI thread - mirrors
    /// `refresh`/`start_load` exactly.
    fn start_database_action(&mut self, context: egui::Context, run_scan_first: bool) {
        if self.library_ui.missing_removal.is_some() || self.database_state.is_loading() {
            return;
        }
        self.database_generation = self.database_generation.next();
        let generation = self.database_generation;
        let previous = match std::mem::replace(
            &mut self.database_state,
            DatabaseState::Error {
                message: "database action starting".to_string(),
                previous: None,
            },
        ) {
            DatabaseState::Ready { snapshot, .. } => Some(snapshot),
            DatabaseState::Outdated { previous, .. } | DatabaseState::Error { previous, .. } => {
                previous
            }
            DatabaseState::Loading { .. } => unreachable!("loading state was rejected above"),
            DatabaseState::NotCreated { .. } => None,
        };
        if run_scan_first {
            self.history.record(HistoryEntry::new(
                ActivityAction::LibraryDatabase,
                None,
                ActivityOutcome::Started,
                "Scanning configured source folders into the library database.",
            ));
        }
        self.database_state = start_database_load(context, generation, previous, run_scan_first);
    }

    fn poll_database_load(&mut self, _context: &egui::Context) {
        let message = match &self.database_state {
            DatabaseState::Loading {
                generation,
                receiver,
                ..
            } => match receiver.try_recv() {
                Ok(message) => Some(message),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    *generation,
                    Err(DatabaseLoadError::Failed {
                        message: "background database loader stopped unexpectedly".to_string(),
                    }),
                )),
            },
            DatabaseState::NotCreated { .. }
            | DatabaseState::Ready { .. }
            | DatabaseState::Outdated { .. }
            | DatabaseState::Error { .. } => None,
        };

        let Some((generation, result)) = message else {
            return;
        };
        // Two independent staleness checks, mirroring poll_load exactly:
        // (1) is this even the current database generation, and (2) does
        // the state we are about to replace still agree it is Loading at
        // that same generation (it could have been replaced by a newer
        // start_database_action call between the channel send and this
        // poll). Either mismatch means this message is from a previous
        // generation and must be ignored, never merged into current state.
        if generation != self.database_generation {
            return;
        }
        let (previous, worker) = match std::mem::replace(
            &mut self.database_state,
            DatabaseState::Error {
                message: "database load result pending".to_string(),
                previous: None,
            },
        ) {
            DatabaseState::Loading {
                generation: state_generation,
                previous,
                worker,
                ..
            } if state_generation == generation => (previous, worker),
            other => {
                self.database_state = other;
                return;
            }
        };
        if let Some(worker) = worker {
            let _ = worker.join();
        }

        // Consumed unconditionally, whatever `result` turns out to be below:
        // a pending Sources-page scan summary is only ever valid for the
        // very next reload completion, never a later one (requirement:
        // never invent a state transition - if this reload doesn't land in
        // `Ready`, there is no `last_scan_summary` to attach it to, so it
        // is simply dropped rather than held over).
        let pending_source_scan_summary = self.pending_source_scan_summary.take();
        self.database_state = match result {
            Ok(DatabaseOutcome::Loaded(snapshot)) => DatabaseState::Ready {
                snapshot: Box::new(snapshot),
                last_scan_summary: pending_source_scan_summary,
            },
            Ok(DatabaseOutcome::Scanned {
                snapshot,
                scan_summary,
                upgrade,
            }) => {
                let activity = match &upgrade {
                    Some(report) => format_database_upgrade_success(report, &scan_summary),
                    None => format_scan_activity(&scan_summary),
                };
                self.history.record(HistoryEntry::new(
                    ActivityAction::LibraryDatabase,
                    None,
                    ActivityOutcome::Completed,
                    activity.clone(),
                ));
                if upgrade.is_some() {
                    self.feedback = Some(ActionFeedback {
                        succeeded: true,
                        message: activity,
                        cleanup: None,
                        warning: None,
                        more_information: None,
                    });
                }
                DatabaseState::Ready {
                    snapshot: Box::new(snapshot),
                    last_scan_summary: Some(scan_summary),
                }
            }
            Err(DatabaseLoadError::NotCreated { database_path }) => {
                DatabaseState::NotCreated { database_path }
            }
            Err(DatabaseLoadError::Outdated { health }) => {
                DatabaseState::Outdated { health, previous }
            }
            Err(DatabaseLoadError::Failed { message }) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::LibraryDatabase,
                    None,
                    ActivityOutcome::Failed,
                    message.clone(),
                ));
                DatabaseState::Error { message, previous }
            }
        };

        let duplicate_report = self
            .database_state
            .snapshot()
            .map(|snapshot| snapshot.duplicate_report.clone());
        prune_duplicate_review_selection(
            &mut self.health_duplicate_ui.selected_duplicate_group,
            &mut self.health_duplicate_ui.selected_duplicate_archive,
            duplicate_report.as_ref(),
        );

        // The merged row set may have just changed (a cache reload/scan
        // just settled) - recompute the cached filtered-index list against
        // it now rather than leaving it stale until the next live refresh
        // or filter-text edit. Only meaningful once a live snapshot
        // exists; the cache-only preview shown before that filters itself
        // fresh each frame instead (see `show_loaded_data`'s Loading
        // branch).
        if let LoadState::Ready(data) = &self.state {
            let merged =
                build_display_rows(&data.records, &data.rows, self.database_state.snapshot());
            self.library_ui.filtered_rows = matching_row_indices(&merged, &self.library_ui.filter);
            self.prune_selection(&merged);
        }
    }

    /// Removes any exact-identity entry from `selected_archives` - and
    /// clears `selected_archive` if it was the one that vanished - that
    /// no longer names any row in `merged_rows`, the just-recomputed
    /// live+cache catalogue (requirement 7: "remove selections that no
    /// longer exist in the loaded catalogue"). Called from both
    /// `poll_load` and `poll_database_load`, right where each already
    /// recomputes `filtered_rows` against the same merged list, so
    /// selection state is never one step behind what is actually on
    /// screen. Compares exact `PathBuf` identity only, never a lossy
    /// display string, and never touches row indices - there are none to
    /// go stale here in the first place.
    fn prune_selection(&mut self, merged_rows: &[ArchiveRow]) {
        self.archive_context.prune(merged_rows);
    }

    fn refresh_diagnostics(&mut self, context: &egui::Context) {
        self.health_duplicate_ui.diagnostics_refresh_generation = self
            .health_duplicate_ui
            .diagnostics_refresh_generation
            .next();
        self.history.record(HistoryEntry::new(
            ActivityAction::Diagnostics,
            None,
            ActivityOutcome::Started,
            "Refreshing setup diagnostics.",
        ));
        self.doctor_repair.diagnostics =
            start_diagnostics(context.clone(), self.refresh_generation);
    }

    /// The Health Dashboard's report, rebuilt only when the underlying
    /// live snapshot, database snapshot, diagnostics refresh, or recovery
    /// offers have actually changed since the last call - see
    /// `HealthReportCacheKey`'s doc comment for why pointer identity
    /// rather than a raw generation comparison. Never called unless the
    /// dashboard is actually open (see its one call site), so this adds
    /// no cost to the ordinary library view.
    fn cached_health_issues(&mut self) -> &[HealthIssue] {
        let key = HealthReportCacheKey {
            live_data_ptr: match &self.state {
                LoadState::Ready(data) => Some(std::ptr::from_ref(data.as_ref()) as usize),
                LoadState::Loading { .. } | LoadState::Error(_) => None,
            },
            database_snapshot_ptr: self
                .database_state
                .snapshot()
                .map(|snapshot| std::ptr::from_ref(snapshot) as usize),
            diagnostics_generation: self.health_duplicate_ui.diagnostics_refresh_generation,
        };

        let cache_is_fresh = self
            .health_duplicate_ui
            .health_report_cache
            .as_ref()
            .is_some_and(|cache| {
                cache.key == key
                    && cache.lazy_unmount_offers == self.mount_ui.lazy_unmount_offers
                    && cache.remount_offers == self.mount_ui.remount_offers
            });

        if !cache_is_fresh {
            let live_records = match &self.state {
                LoadState::Ready(data) => Some(&data.records),
                LoadState::Loading { .. } | LoadState::Error(_) => None,
            };
            let issues = match (live_records, self.database_state.snapshot()) {
                (Some(records), Some(snapshot)) => build_health_issues(
                    records,
                    snapshot,
                    &self.mount_ui.lazy_unmount_offers,
                    &self.mount_ui.remount_offers,
                ),
                _ => Vec::new(),
            };
            self.health_duplicate_ui.health_report_cache = Some(HealthReportCache {
                key,
                lazy_unmount_offers: self.mount_ui.lazy_unmount_offers.clone(),
                remount_offers: self.mount_ui.remount_offers.clone(),
                issues,
            });
        }

        &self
            .health_duplicate_ui
            .health_report_cache
            .as_ref()
            .unwrap()
            .issues
    }

    fn poll_diagnostics(&mut self) {
        enum PollResult {
            Completed(DiagnosticsMessage),
            Disconnected(RefreshGeneration),
        }

        let result = match &self.doctor_repair.diagnostics {
            DiagnosticsState::Loading {
                generation,
                receiver,
            } => match receiver.try_recv() {
                Ok(message) => Some(PollResult::Completed(message)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(PollResult::Disconnected(*generation)),
            },
            DiagnosticsState::Ready { .. } | DiagnosticsState::Error { .. } => None,
        };
        match result {
            Some(PollResult::Completed((generation, report)))
                if generation == self.refresh_generation =>
            {
                self.history.record(HistoryEntry::new(
                    ActivityAction::Diagnostics,
                    None,
                    ActivityOutcome::Completed,
                    if report.ready_for_actions {
                        "Diagnostics completed: EmuWiz is ready."
                    } else {
                        "Diagnostics completed: setup needs attention."
                    },
                ));
                if !report.config_missing && report.config_path_error.is_none() {
                    self.doctor_repair.config_previously_confirmed = true;
                }
                self.doctor_repair.diagnostics = DiagnosticsState::Ready { generation, report };
                self.maybe_auto_open_onboarding();
            }
            Some(PollResult::Disconnected(generation)) if generation == self.refresh_generation => {
                let message = "The diagnostics worker stopped unexpectedly. Run diagnostics again."
                    .to_string();
                self.history.record(HistoryEntry::new(
                    ActivityAction::Diagnostics,
                    None,
                    ActivityOutcome::Failed,
                    message.clone(),
                ));
                self.doctor_repair.diagnostics = DiagnosticsState::Error {
                    generation,
                    message,
                };
                self.tools_overlay = ToolsOverlay::Diagnostics;
            }
            Some(PollResult::Completed(_)) | Some(PollResult::Disconnected(_)) | None => {}
        }
    }

    fn start_setup_action(&mut self, context: egui::Context, action: SetupAction) {
        if self.is_busy()
            || (matches!(&action, SetupAction::SetMountRoot(_)) && self.source_action.is_some())
        {
            return;
        }
        if matches!(&action, SetupAction::SetMountRoot(_)) {
            // The previous outcome is no longer current the moment a new
            // apply begins.
            self.mount_root_feedback = None;
        }
        let started_message = match &action {
            SetupAction::CreateStarterConfig => "Creating starter config.",
            SetupAction::CreateMountRoot => "Creating configured mount root.",
            SetupAction::OpenConfigFolder => "Opening config folder.",
            SetupAction::SetMountRoot(_) => "Updating temporary preparation folder.",
        };
        self.history.record(HistoryEntry::new(
            ActivityAction::Setup,
            None,
            ActivityOutcome::Started,
            started_message,
        ));
        self.setup_action = Some(start_setup_action_worker(context, action));
    }

    // --- Doctor Stage 1A ------------------------------------------------

    /// Executes the reviewed repair. The only path that mutates.
    ///
    /// Every safety gate lives in `execute_doctor_repair`, which re-resolves
    /// the finding against live state; nothing here trusts the review's
    /// captured path. Afterwards only the affected finding's own check is
    /// re-run (inside the executor), never the whole scan, and exactly one
    /// History entry is recorded whatever the result.
    /// Draws the Cheat Sources page and applies whatever it asked for.
    ///
    /// The state is loaded on first visit rather than at startup: opening the
    /// GUI should not read a preferences file for a page nobody has looked
    /// at. A path that cannot even be resolved (no `HOME`) is reported in
    /// place instead of failing the whole page, since every other part of the
    /// GUI still works without it.
    /// Draws the Canonical Organisation page, loading its state lazily.
    fn show_rom_organisation_page(&mut self, ui: &mut egui::Ui) {
        let page = self
            .rom_organisation_page
            .get_or_insert_with(rom_organisation_page::RomOrganisationPageState::load);
        rom_organisation_page::show_rom_organisation_page(ui, page);
    }

    /// Draws the read-only publisher projection page. The source is only the
    /// already-built Playing Library plan from Library Organisation; this
    /// page never scans or re-elects games itself.
    fn show_publisher_profile_page(&mut self, ui: &mut egui::Ui) {
        let source_plan = self
            .rom_organisation_page
            .as_ref()
            .and_then(|page| page.playing_library.plan().cloned());
        let page = self
            .publisher_profile_page
            .get_or_insert_with(publisher_profile_page::PublisherProfilePageState::default);
        page.set_source_plan(source_plan);
        if let Some(action) = publisher_profile_page::show_publisher_profile_page(ui, page) {
            match action {
                publisher_profile_page::PublisherProfilePageAction::OpenLibraryOrganisation => {
                    self.navigate_to_main_view(MainView::CanonicalOrganisation)
                }
            }
        }
    }

    fn show_optical_conversion_page(&mut self, ui: &mut egui::Ui) {
        let selected_context = self.archive_context.focused.as_ref().map(|path| {
            let platform = match &self.selected_evidence_ui.selected_evidence {
                selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                    if report.path == *path =>
                {
                    report.identity.platform.map(str::to_string)
                }
                _ => None,
            };
            optical_conversion_page::SelectedConversionContext {
                path: path.clone(),
                platform,
            }
        });
        let page = self
            .optical_conversion_page
            .get_or_insert_with(optical_conversion_page::OpticalConversionPageState::default);
        page.set_selected_context(selected_context);
        optical_conversion_page::show_optical_conversion_page(ui, page);
    }

    fn show_library_view_history_page(&mut self, ui: &mut egui::Ui) {
        let page = self
            .library_view_history_page
            .get_or_insert_with(library_view_history_page::LibraryViewHistoryPageState::load);
        library_view_history_page::show_library_view_history_page(ui, page);
    }

    /// The consolidated "Sources" destination - one sidebar entry over
    /// Libraries/DATs/Cheats/Discovery tabs (`SourcesTab`). Renders the
    /// shared heading and tab row, then dispatches to whichever tab
    /// `self.sources_tab` currently names. Each arm calls exactly the same
    /// rendering this destination used before consolidation
    /// (`show_sources_libraries_tab`, `self.show_dat_sources_page`,
    /// `self.show_cheat_sources_page`, `show_sources_discovery_tab`) -
    /// nothing here re-implements source management, DAT handling, cheat
    /// provisioning, or collection discovery.
    fn show_sources_page(&mut self, context: &egui::Context, ui: &mut egui::Ui, tab: SourcesTab) {
        self.es_de_media.start(context.clone());
        if self.es_de_media.poll() {
            self.gamer_covers.identity_refreshed();
            self.gamer_screenshots.identity_refreshed();
            if let Some(worker) = self.gamer_cover_worker.as_ref() {
                worker.update_esde(self.es_de_media.snapshot().cloned());
            }
        }
        self.launchbox_local_media.start(context.clone());
        if self.launchbox_local_media.poll() {
            self.gamer_covers.identity_refreshed();
            self.gamer_screenshots.identity_refreshed();
            if let Some(worker) = self.gamer_cover_worker.as_ref() {
                worker.update_launchbox(self.launchbox_local_media.snapshot().cloned());
            }
        }
        if let Some(clicked) = sources_page::show_sources_tabs(ui, tab) {
            self.navigate_to_sources_tab(clicked);
        }
        match tab {
            SourcesTab::Libraries => self.show_sources_libraries_tab(context, ui),
            SourcesTab::Dats => self.show_dat_sources_page(ui),
            SourcesTab::Cheats => self.show_cheat_sources_page(context, ui),
            SourcesTab::Discovery => {
                if let Some(action) = sources_page::show_sources_discovery_tab(
                    ui,
                    &self.database_state,
                    &self.es_de_media,
                    &self.launchbox_local_media,
                ) {
                    match action {
                        sources_page::LocalProviderRefreshAction::EsDe => {
                            self.es_de_media.refresh(context.clone());
                        }
                        sources_page::LocalProviderRefreshAction::LaunchBoxLocal => {
                            self.launchbox_local_media.refresh(context.clone());
                        }
                    }
                }
            }
        }
    }

    /// The "Libraries" tab: source-folder configuration, the RetroArch
    /// cheat-database catalogue manager, BSFree provisioning, and RomM -
    /// exactly the content `MainView::Sources` rendered before
    /// consolidation, unchanged apart from the outer page header now being
    /// `show_sources_page`'s shared one.
    fn show_sources_libraries_tab(&mut self, context: &egui::Context, ui: &mut egui::Ui) {
        let catalogue_snapshot = self.database_state.snapshot();
        let source_state = source_state::merge_configured_sources(
            self.gui_config.source_roots().ok().unwrap_or_default(),
            catalogue_snapshot.map(|snapshot| snapshot.source_views.as_slice()),
        );
        let sources = source_state.sources.as_slice();
        let archives = catalogue_snapshot
            .map(|snapshot| snapshot.archives.as_slice())
            .unwrap_or(&[]);
        let mount_root = match &self.state {
            LoadState::Ready(data) => Some(data.mount_root.as_path()),
            LoadState::Loading { .. } | LoadState::Error(_) => None,
        };

        let sources_action = sources_page::sources_content_column(ui, |ui| {
            show_sources_overview(
                ui,
                sources,
                source_state.catalogue_available,
                &self.catalogue_bsfree_ui.catalogue_manager,
                self.catalogue_bsfree_ui.catalogue_retrieval.as_ref(),
            );
            ui.add_space(theme::SECTION_GAP);

            if let Some(last_scan) = &self.sources_last_scan
                && show_sources_last_scan_banner(ui, last_scan)
            {
                self.show_skipped_files = true;
                self.skipped_files_filter = None;
            }
            ui.add_space(theme::SECTION_GAP);

            show_sources_page_with_mount_root(
                ui,
                sources,
                archives,
                mount_root,
                source_state.catalogue_available,
                self.source_action.is_some(),
                &mut self.mount_root_draft,
                self.setup_action.is_some()
                    || self.source_action.is_some()
                    || self.database_state.is_loading(),
                self.mount_root_feedback.as_ref(),
                &mut self.sources_add_dialog,
                &mut self.sources_remove_dialog,
                &mut self.clipboard,
            )
        });
        if let Some(sources_action) = sources_action {
            match sources_action {
                SourcesPageAction::AddFolder(path) => {
                    self.start_source_action(context.clone(), SourceAction::Add(path));
                }
                SourcesPageAction::ApplyMountRoot(path) => {
                    self.start_setup_action(context.clone(), SetupAction::SetMountRoot(path));
                }
                SourcesPageAction::ScanOne(path) => {
                    self.start_source_action(context.clone(), SourceAction::ScanOne(path));
                }
                SourcesPageAction::ScanAll => {
                    self.start_source_action(context.clone(), SourceAction::ScanAll);
                }
                SourcesPageAction::RefreshStatus => {
                    self.start_database_action(context.clone(), false);
                }
                SourcesPageAction::AssignPlatform { path, platform } => {
                    self.start_source_action(
                        context.clone(),
                        SourceAction::AssignPlatform { path, platform },
                    );
                }
                SourcesPageAction::SetEnabled { path, enabled } => {
                    self.start_source_action(
                        context.clone(),
                        SourceAction::SetEnabled { path, enabled },
                    );
                }
                SourcesPageAction::ConfirmRemove {
                    path,
                    keep_catalogue,
                } => {
                    self.start_source_action(
                        context.clone(),
                        SourceAction::Remove {
                            path,
                            keep_catalogue,
                        },
                    );
                }
                SourcesPageAction::ViewScanDetails => {
                    self.navigate_to_sources_tab(SourcesTab::Discovery);
                }
                SourcesPageAction::ViewInLibrary(path) => {
                    self.navigate_to_library_tab(LibraryTab::Archives);
                    self.library_ui.library_source_filter = Some(Some(path));
                }
            }
        }

        ui.add_space(theme::SECTION_GAP);
        // Large, infrequently-touched configuration blocks - collapsed by
        // default so the Sources page opens on the source-folder list rather
        // than a wall of stacked cards. State persists for the session.
        let catalogue_action = widgets::collapsible_section(
            ui,
            "sources_catalogue_manager",
            "Cheat database & RetroArch catalogue",
            false,
            |ui| {
                ui.label(
                    "Download, update, or verify the trusted cheat database EmuWiz uses for \
                     cheat setup.",
                );
                show_retroarch_catalogue_manager(
                    ui,
                    &self.catalogue_bsfree_ui.catalogue_manager,
                    self.catalogue_bsfree_ui.catalogue_review.as_ref(),
                    self.catalogue_bsfree_ui.catalogue_retrieval.as_ref(),
                    self.catalogue_bsfree_ui.catalogue_last_result.as_ref(),
                    &mut self.clipboard,
                )
            },
        )
        .flatten();
        if let Some(catalogue_action) = catalogue_action {
            self.handle_catalogue_manager_action(context, catalogue_action);
        }

        ui.add_space(theme::SECTION_GAP);
        let bsfree_operation =
            widgets::collapsible_section(ui, "sources_bsfree", "BSFree source", false, |ui| {
                show_bsfree_source_card(
                    ui,
                    &self.catalogue_bsfree_ui.bsfree_manager,
                    self.catalogue_bsfree_ui.bsfree_operation.is_some(),
                    &mut self.catalogue_bsfree_ui.bsfree_ui,
                    &mut self.clipboard,
                )
            })
            .flatten();
        if let Some(operation) = bsfree_operation {
            self.start_bsfree_operation(context.clone(), operation);
        }

        ui.add_space(theme::SECTION_GAP);
        let romm_view = romm_source::build_card_view(
            self.romm_ui.snapshot.as_deref(),
            self.romm_ui
                .operation
                .as_ref()
                .map(|running| &running.operation),
            self.romm_ui
                .operation
                .as_ref()
                .is_some_and(|running| running.cancellation_requested),
        );
        let romm_progress = self
            .romm_ui
            .operation
            .as_ref()
            .and_then(|running| running.progress.as_ref())
            .cloned();
        let linkage_paths = self
            .database_state
            .snapshot()
            .map(|snapshot| {
                snapshot
                    .archives
                    .iter()
                    .map(|archive| archive.absolute_path.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(request) = romm_source::show_romm_source_card(
            ui,
            &romm_view,
            &mut self.romm_ui.card,
            romm_progress.as_ref(),
        ) {
            match request {
                RommCardRequest::Start(operation) => {
                    // Declines when something is already running, which
                    // is what makes a double click harmless.
                    self.start_romm_operation(context.clone(), operation);
                }
                RommCardRequest::Cancel => self.cancel_romm_operation(),
                RommCardRequest::OpenConfigure => self.open_romm_configuration(),
                RommCardRequest::OpenBrowse(view) => self.open_romm_browse(view),
                RommCardRequest::CheckLinks => {
                    self.start_romm_operation(
                        context.clone(),
                        RommOperation::CheckLinks {
                            local_paths: linkage_paths,
                        },
                    );
                }
                RommCardRequest::ReviewMappings => {
                    self.start_romm_operation(context.clone(), RommOperation::PlanMappings);
                }
                RommCardRequest::ApplyMappings => self.open_romm_mapping_plan(),
            }
        }

        if let Some(request) = self.show_romm_configuration_window(context) {
            self.handle_romm_config_request(context, request);
        }

        // Drawn as a window rather than appended here - see
        // `show_romm_browse_window`. Appending it below the source
        // card put it past the bottom of the viewport, so clicking
        // "Browse records" looked like it did nothing at all.
        if let Some(request) = self.show_romm_browse_window(context) {
            self.handle_romm_browse_request(context, request);
        }

        ui.add_space(theme::SECTION_GAP);
        show_sources_recent_activity(ui, &self.history);
    }

    /// Draws the DAT Sources page and applies whatever it asked for.
    ///
    /// Loaded on first visit rather than at startup, for the same reason Cheat
    /// Sources is. A path that cannot be resolved (no `HOME`) is reported in
    /// place instead of failing the whole page.
    ///
    /// The library folders offered as audit targets, and the trusted roots the
    /// hashing policy uses, both come from the same `Config` the rest of the
    /// build reads. A missing or unreadable config is not fatal here: it means
    /// no folders are offered and no symlink may be followed, which are the
    /// safe answers rather than an error the user cannot act on from this page.
    fn show_dat_sources_page(&mut self, ui: &mut egui::Ui) {
        self.show_dat_sources_page_mode(ui, false);
    }

    /// Shares loading, background-job polling, action dispatch, and history
    /// recording between the advanced catalogue page and the task-oriented
    /// Identify & Rename workflow. The two views deliberately use the same
    /// state and core actions; neither gets a private rename implementation.
    fn show_identify_rename_page(&mut self, ui: &mut egui::Ui) {
        self.show_dat_sources_page_mode(ui, true);
    }

    fn show_dat_sources_page_mode(&mut self, ui: &mut egui::Ui, identify_rename: bool) {
        if !identify_rename {
            self.dat_authority
                .show(ui, database_state_path(&self.database_state));
        }
        if self.dat_sources_page.is_none() {
            let path = match archivefs_core::dat::sources::default_dat_sources_config_path() {
                Ok(path) => path,
                Err(error) => {
                    widgets::banner(
                        ui,
                        "Registry location unknown",
                        &format!(
                            "{error}. DAT sources cannot be read or saved without a home \
                             directory."
                        ),
                        widgets::StatusTone::Blocked,
                    );
                    return;
                }
            };
            let config = Config::load_default().ok();
            let library_folders = config
                .as_ref()
                .map(|config| config.source_folders.clone())
                .unwrap_or_default();
            let trusted = config
                .as_ref()
                .map(archivefs_core::safe_read::TrustedRoots::from_config)
                .unwrap_or_else(archivefs_core::safe_read::TrustedRoots::none);
            self.dat_sources_page = Some(
                dat_sources_page::DatSourcesPageState::load(path, library_folders, trusted)
                    .with_database_path(database_state_path(&self.database_state)),
            );
        }

        let Some(page) = self.dat_sources_page.as_mut() else {
            return;
        };
        // Drained before the view is built, so the view stays a pure function
        // of state. A running job repaints continuously; an idle page does not.
        let dat_changed = page.poll();
        if dat_changed {
            self.dat_authority.invalidate();
            self.needs_attention.invalidate();
        }
        if dat_changed || page.is_busy() {
            ui.ctx().request_repaint();
        }
        let view = page.view_with_romm_summary(self.romm_ui.verify_summary);
        let action = if identify_rename {
            if self.quick_rename_mode {
                dat_sources_page::show_quick_rename_page(ui, &view, &mut self.dat_sources_ui)
            } else {
                dat_sources_page::show_identify_rename_page(ui, &view, &mut self.dat_sources_ui)
            }
        } else {
            dat_sources_page::show_dat_sources_page(ui, &view, &mut self.dat_sources_ui)
        };
        if let Some(action) = action {
            let open_dat_sources = matches!(
                action,
                dat_sources_page::DatSourcesPageAction::OpenDatSources
            );
            let open_advanced = matches!(
                action,
                dat_sources_page::DatSourcesPageAction::OpenAdvancedIdentifyRename
            );
            self.dat_authority.invalidate();
            self.needs_attention.invalidate();
            if matches!(action, dat_sources_page::DatSourcesPageAction::Revert) {
                self.dat_sources_ui.clear();
            }
            page.apply(action);
            if open_dat_sources {
                self.quick_rename_mode = false;
                self.view = MainView::DatSources;
            } else if open_advanced {
                self.quick_rename_mode = false;
            }
        }
        // Surface apply/rollback outcomes into History & Logs, without private
        // paths (the journal keeps those, never the general log).
        for record in page.drain_history_records() {
            let entry = match record.action {
                dat_sources_page::RenameHistoryAction::Apply => HistoryEntry::new(
                    ActivityAction::DatRenameApply,
                    None,
                    ActivityOutcome::Completed,
                    format!("{}: {}", record.transaction_id, record.message),
                ),
                dat_sources_page::RenameHistoryAction::Rollback => HistoryEntry::new(
                    ActivityAction::DatRenameRollback,
                    None,
                    ActivityOutcome::Completed,
                    format!("{}: {}", record.transaction_id, record.message),
                ),
            };
            self.history.record(entry);
        }
        let reload_enriched_catalogue = page.take_identity_enrichment_completed();
        if reload_enriched_catalogue {
            self.start_database_action(ui.ctx().clone(), false);
        }
    }

    fn poll_setup_action(&mut self, context: &egui::Context) {
        let result = self.setup_action.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.action.clone(), result))
        });
        if let Some((action, result)) = result {
            self.setup_action = None;
            match result {
                Ok(message) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Setup,
                        None,
                        ActivityOutcome::Completed,
                        message.clone(),
                    ));
                    let reload_warning = if matches!(&action, SetupAction::SetMountRoot(_)) {
                        self.gui_config.reload_default().err().map(|error| {
                            format!(
                                "The folder changed successfully, but EmuWiz could not reload its configuration: {error}."
                            )
                        })
                    } else {
                        None
                    };
                    self.feedback = Some(ActionFeedback {
                        succeeded: true,
                        message: message.clone(),
                        cleanup: None,
                        warning: reload_warning.clone(),
                        more_information: None,
                    });
                    if matches!(&action, SetupAction::SetMountRoot(_)) {
                        self.mount_root_feedback = Some(sources_page::MountRootFeedback {
                            succeeded: true,
                            summary: message,
                            detail: None,
                            warning: reload_warning,
                        });
                        self.mount_root_draft = None;
                        self.refresh(context);
                    } else if action != SetupAction::OpenConfigFolder {
                        self.refresh_diagnostics(context);
                    }
                }
                Err(message) => {
                    let (feedback_message, more_information) = if matches!(
                        &action,
                        SetupAction::SetMountRoot(_)
                    ) {
                        (
                                "Could not change the temporary game preparation folder. Choose an existing writable folder and try again."
                                    .to_string(),
                                Some(message.clone()),
                            )
                    } else {
                        (message.clone(), None)
                    };
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Setup,
                        None,
                        ActivityOutcome::Failed,
                        message.clone(),
                    ));
                    if matches!(&action, SetupAction::SetMountRoot(_)) {
                        self.mount_root_feedback = Some(sources_page::MountRootFeedback {
                            succeeded: false,
                            summary: feedback_message.clone(),
                            detail: Some(message.clone()),
                            warning: None,
                        });
                    }
                    self.feedback = Some(ActionFeedback {
                        succeeded: false,
                        message: feedback_message,
                        cleanup: None,
                        warning: None,
                        more_information,
                    });
                }
            }
        }
    }

    fn start_catalogue_status_load(&mut self, context: egui::Context) {
        if matches!(
            self.catalogue_bsfree_ui.catalogue_manager,
            CatalogueManagerState::Loading(_)
        ) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.catalogue_bsfree_ui.catalogue_manager = CatalogueManagerState::Loading(receiver);
        thread::spawn(move || {
            let result = default_cheat_source_cache_root()
                .and_then(|root| list_retroarch_cheat_sources(&root));
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn start_catalogue_retrieval(&mut self, context: egui::Context) {
        if self.catalogue_bsfree_ui.catalogue_retrieval.is_some() {
            return;
        }
        let Some(review) = self.catalogue_bsfree_ui.catalogue_review.take() else {
            return;
        };
        self.catalogue_bsfree_ui.catalogue_generation = self
            .catalogue_bsfree_ui
            .catalogue_generation
            .wrapping_add(1);
        let generation = self.catalogue_bsfree_ui.catalogue_generation;
        let source_id = review.source_id;
        let force_refresh = review.kind == CatalogueRetrievalKind::Update;
        let cancellation = CheatSourceCancellation::default();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        let (progress_sender, progress_receiver) = mpsc::channel();
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            None,
            ActivityOutcome::Started,
            format!(
                "RetroArch catalogue {review_kind} started for '{source_id}'.",
                review_kind = if force_refresh { "update" } else { "download" }
            ),
        ));
        self.catalogue_bsfree_ui.catalogue_retrieval = Some(RunningCatalogueRetrieval {
            generation,
            source_id: source_id.clone(),
            cancellation,
            receiver,
            progress_receiver,
            progress: None,
            cancellation_requested: false,
        });
        let progress_context = context.clone();
        let progress = CheatSourceProgressReporter::new(move |event| {
            let _ = progress_sender.send(event);
            progress_context.request_repaint();
        });
        thread::spawn(move || {
            let result = default_cheat_source_cache_root().and_then(|cache_root| {
                fetch_retroarch_cheat_source(
                    &source_id,
                    &CheatSourceFetchOptions {
                        cache_root,
                        force_refresh,
                        offline: false,
                        expected_sha256: None,
                        max_download_bytes: None,
                        cancellation: Some(worker_cancellation),
                        progress: Some(progress),
                    },
                    &HttpsCheatSourceTransport::new(),
                )
            });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// The single dispatch point for `CatalogueManagerAction` - shared by
    /// every page that renders `show_retroarch_catalogue_manager` (Sources,
    /// and now Cheats & Mods) so the Review-then-Confirm two-step and the
    /// "no automatic network access" guarantee it enforces cannot drift
    /// between call sites.
    fn handle_catalogue_manager_action(
        &mut self,
        context: &egui::Context,
        action: CatalogueManagerAction,
    ) {
        match action {
            CatalogueManagerAction::Refresh => {
                self.start_catalogue_status_load(context.clone());
            }
            CatalogueManagerAction::Review { source_id, kind } => {
                self.catalogue_bsfree_ui.catalogue_review =
                    Some(CatalogueReview { source_id, kind });
            }
            CatalogueManagerAction::Confirm => {
                self.start_catalogue_retrieval(context.clone());
            }
            CatalogueManagerAction::CancelReview => {
                self.catalogue_bsfree_ui.catalogue_review = None;
            }
            CatalogueManagerAction::CancelRunning => {
                if let Some(running) = self.catalogue_bsfree_ui.catalogue_retrieval.as_mut() {
                    running.cancellation.cancel();
                    running.cancellation_requested = true;
                }
            }
        }
    }

    fn poll_catalogue_manager(&mut self, context: &egui::Context) {
        if let CatalogueManagerState::Loading(receiver) =
            &self.catalogue_bsfree_ui.catalogue_manager
        {
            match receiver.try_recv() {
                Ok(Ok(list)) => {
                    self.catalogue_bsfree_ui.catalogue_manager = CatalogueManagerState::Ready(list)
                }
                Ok(Err(error)) => {
                    self.catalogue_bsfree_ui.catalogue_manager =
                        CatalogueManagerState::Failed(error)
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.catalogue_bsfree_ui.catalogue_manager =
                        CatalogueManagerState::Failed(CheatSourceError {
                            schema_version:
                                archivefs_core::patch_manager::CHEAT_SOURCE_RESULT_SCHEMA_VERSION,
                            stage: archivefs_core::patch_manager::CheatSourceErrorStage::Cache,
                            code: "status_worker_stopped".to_string(),
                            message: "catalogue status worker stopped unexpectedly".to_string(),
                            retry_after_seconds: None,
                        });
                }
            }
        }
        if let Some(running) = self.catalogue_bsfree_ui.catalogue_retrieval.as_mut() {
            for progress in running.progress_receiver.try_iter() {
                running.progress = Some(progress);
            }
        }
        let result = self
            .catalogue_bsfree_ui
            .catalogue_retrieval
            .as_ref()
            .and_then(|running| {
                running
                    .receiver
                    .try_recv()
                    .ok()
                    .map(|result| (running.generation, running.source_id.clone(), result))
            });
        let Some((generation, source_id, result)) = result else {
            return;
        };
        self.catalogue_bsfree_ui.catalogue_retrieval = None;
        if generation != self.catalogue_bsfree_ui.catalogue_generation {
            return;
        }
        match &result {
            Ok(fetch) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatSourceRetrieval,
                    None,
                    ActivityOutcome::Completed,
                    format!(
                        "RetroArch catalogue '{}': revision resolved to {}.",
                        source_id, fetch.manifest.resolved_revision
                    ),
                ));
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatSourceRetrieval,
                    None,
                    ActivityOutcome::Completed,
                    format!(
                        "RetroArch catalogue '{}': download and verification completed ({} bytes, {} manifest files).",
                        source_id,
                        fetch.manifest.downloaded_bytes,
                        fetch.manifest.files.len()
                    ),
                ));
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatSourceRetrieval,
                    None,
                    ActivityOutcome::Completed,
                    format!(
                        "RetroArch catalogue '{}' activated at revision {} ({} files verified; {} indexed, {} excluded).",
                        source_id,
                        fetch.manifest.resolved_revision,
                        fetch.manifest.files.len(),
                        fetch.manifest.indexed_file_count,
                        fetch.manifest.malformed_cheat_count
                            + fetch.manifest.excluded_unsupported_count
                            + fetch.manifest.excluded_path_encoding_count
                    ),
                ));
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.source_fetch = CheatStepResource::Ready(fetch.clone());
                    workflow.source_list = CheatStepResource::NotLoaded;
                    clear_cheat_candidate_state(workflow);
                }
            }
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatSourceRetrieval,
                    None,
                    if error.code == "cancelled" {
                        ActivityOutcome::Skipped
                    } else {
                        ActivityOutcome::Failed
                    },
                    format!(
                        "RetroArch catalogue '{source_id}': {error}. Existing snapshot retained."
                    ),
                ));
            }
        }
        self.catalogue_bsfree_ui.catalogue_last_result = Some(result);
        self.catalogue_bsfree_ui.catalogue_manager = CatalogueManagerState::NotLoaded;
        self.start_catalogue_status_load(context.clone());
    }

    /// Whether a new Library Views action may start - the same "one
    /// writer at a time" convention `source_action_available` uses
    /// (Preview/Apply reads the same database a scan writes to).
    fn library_view_action_available(&self) -> bool {
        self.library_view_action.is_none()
            && self.source_action.is_none()
            && self.library_ui.alias_action.is_none()
            && self.library_ui.platform_action.is_none()
            && self.library_ui.bulk_platform_action.is_none()
            && self.library_ui.missing_removal.is_none()
            && !self.database_state.is_loading()
    }

    /// Re-reads the configured Library Views list from disk - a cheap,
    /// synchronous, read-only file read (the same kind
    /// `source_action_log_category`'s doc comment already calls out for
    /// config reads of this size), never a background thread. A read
    /// failure is treated as "no views configured" here rather than
    /// surfaced as an error, matching `load_library_view_configs_default`'s
    /// own "missing file is not an error" contract - the only way this can
    /// actually fail once the file exists is a corrupt/foreign-format
    /// file, which the next successful Add/Edit save overwrites anyway.
    fn reload_library_views(&mut self) {
        self.library_views = load_library_views();
    }

    fn start_library_view_action(&mut self, context: egui::Context, action: LibraryViewAction) {
        if !self.library_view_action_available() {
            return;
        }
        self.history.record(HistoryEntry::new(
            library_view_action_log_category(&action),
            None,
            ActivityOutcome::Started,
            library_view_action_started_message(&action),
        ));
        self.library_view_action = Some(start_library_view_worker(action, context));
    }

    /// Mirrors `poll_source_action`: refreshes `library_views` from disk on
    /// every successful outcome (Add/Edit/SetEnabled/Remove all mutate the
    /// configured list; Preview/Apply/Repair don't, but reloading anyway is
    /// harmless and keeps this one code path for all six), and updates
    /// `library_view_last_plan` for the two outcomes that produce or
    /// invalidate a plan.
    fn poll_library_view_action(&mut self, context: &egui::Context) {
        let result = self.library_view_action.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.action.clone(), result))
        });
        let Some((action, result)) = result else {
            return;
        };
        self.library_view_action = None;
        let log_category = library_view_action_log_category(&action);
        match result {
            Ok(outcome) => {
                let message = library_view_action_success_message(&outcome);
                self.history.record(HistoryEntry::new(
                    log_category,
                    None,
                    ActivityOutcome::Completed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                match &outcome {
                    LibraryViewActionOutcome::Added(_) => {
                        self.library_view_form_dialog = None;
                    }
                    LibraryViewActionOutcome::Edited(view) => {
                        self.library_view_form_dialog = None;
                        // The edited view's own filters may have changed -
                        // any previously computed plan for it no longer
                        // reflects the current configuration truthfully.
                        if self
                            .library_view_last_plan
                            .as_ref()
                            .is_some_and(|(previous, _)| previous.id == view.id)
                        {
                            self.library_view_last_plan = None;
                        }
                    }
                    LibraryViewActionOutcome::Previewed { view, plan } => {
                        self.library_view_last_plan = Some((view.clone(), plan.clone()));
                    }
                    LibraryViewActionOutcome::Applied { view, .. }
                    | LibraryViewActionOutcome::Repaired { view, .. } => {
                        // Apply/Repair change what is actually on disk -
                        // the previous plan's Create/Repair/Remove counts
                        // no longer describe reality, even though the
                        // view itself is unchanged.
                        if self
                            .library_view_last_plan
                            .as_ref()
                            .is_some_and(|(previous, _)| previous.id == view.id)
                        {
                            self.library_view_last_plan = None;
                        }
                    }
                    LibraryViewActionOutcome::Removed { view, .. } => {
                        self.library_view_remove_dialog = None;
                        if self
                            .library_view_last_plan
                            .as_ref()
                            .is_some_and(|(previous, _)| previous.id == view.id)
                        {
                            self.library_view_last_plan = None;
                        }
                    }
                    LibraryViewActionOutcome::SetEnabled(_) => {}
                }
                self.reload_library_views();
            }
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    log_category,
                    None,
                    ActivityOutcome::Failed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                // Deliberately does not clear `library_view_form_dialog`/
                // `library_view_remove_dialog` on failure - exactly like
                // `poll_source_action`, a failed Add/Edit/Remove should
                // leave the dialog open with its input intact.
            }
        }
        context.request_repaint();
    }

    fn missing_removal_action_available(&self) -> bool {
        self.library_ui.missing_removal.is_none()
            && self.library_ui.platform_action.is_none()
            && self.library_ui.bulk_platform_action.is_none()
            && self.library_ui.alias_action.is_none()
            && self.source_action.is_none()
            && self.library_view_action.is_none()
            && matches!(self.database_state, DatabaseState::Ready { .. })
    }

    fn missing_removal_unavailable_reason(&self) -> Option<String> {
        match &self.database_state {
            DatabaseState::Loading { .. } => Some(
                "Catalogue is still loading. Removal will be available when loading completes."
                    .to_string(),
            ),
            DatabaseState::Outdated { .. } => Some(
                "Catalogue data needs to be refreshed before stale entries can be removed."
                    .to_string(),
            ),
            DatabaseState::Error { message, .. } => Some(message.clone()),
            DatabaseState::NotCreated { .. } => Some(
                "Catalogue is not available yet. Create or load the catalogue before removing stale entries."
                    .to_string(),
            ),
            DatabaseState::Ready { .. } => {
                if self.library_ui.missing_removal.is_some()
                    || self.library_ui.platform_action.is_some()
                    || self.library_ui.bulk_platform_action.is_some()
                    || self.library_ui.alias_action.is_some()
                    || self.source_action.is_some()
                    || self.library_view_action.is_some()
                {
                    Some("Another catalogue operation is currently running.".to_string())
                } else {
                    None
                }
            }
        }
    }

    fn start_missing_removal(&mut self, context: egui::Context, archive_paths: Vec<PathBuf>) {
        if !self.missing_removal_action_available() || archive_paths.is_empty() {
            return;
        }
        let requested_paths = archive_paths.len();
        let (sender, receiver) = mpsc::channel();
        self.library_ui.missing_removal = Some(RunningMissingRemoval {
            requested_paths,
            receiver,
        });
        thread::spawn(move || {
            let result = apply_missing_removal(&archive_paths).map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn poll_missing_removal(&mut self, context: &egui::Context) {
        let result = self
            .library_ui
            .missing_removal
            .as_ref()
            .and_then(|running| {
                running
                    .receiver
                    .try_recv()
                    .ok()
                    .map(|result| (running.requested_paths, result))
            });
        let Some((requested_paths, result)) = result else {
            return;
        };
        self.library_ui.missing_removal = None;
        match result {
            Ok(result) => {
                let message = format!(
                    "Removed {} missing catalogue entr{}. No archive files were deleted.",
                    result.removed,
                    if result.removed == 1 { "y" } else { "ies" }
                );
                self.history.record(HistoryEntry::new(
                    ActivityAction::CatalogueCleanup,
                    None,
                    ActivityOutcome::Completed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                self.start_database_action(context.clone(), false);
            }
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CatalogueCleanup,
                    None,
                    ActivityOutcome::Failed,
                    format!(
                        "Could not remove {requested_paths} selected missing catalogue entr{}: {message}",
                        if requested_paths == 1 { "y" } else { "ies" }
                    ),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
            }
        }
    }

    /// Applies a `MountPageAction` returned by the Mount or Selected
    /// page. Queue execution re-derives eligibility from the live
    /// snapshot at the moment of the click (queue order, `Pending`
    /// only), then hands the items to the proven `start_mount_all`
    /// batch engine - never a stale item list captured at render time.
    fn handle_mount_page_action(
        &mut self,
        context: &egui::Context,
        action: Option<MountPageAction>,
    ) {
        match action {
            Some(MountPageAction::MountQueue) => {
                let items = match &self.state {
                    LoadState::Ready(data) => {
                        let eligible =
                            queued_pending_paths(&self.mount_ui.mount_queue, &data.records);
                        mount_all_items_for_paths(&data.records, &eligible)
                    }
                    _ => Vec::new(),
                };
                if !items.is_empty() {
                    self.start_mount_all(context.clone(), items);
                }
            }
            Some(MountPageAction::Refresh) => self.refresh(context),
            Some(MountPageAction::GoToMount) => {
                self.view = MainView::Mount;
                self.tools_overlay = ToolsOverlay::None;
            }
            Some(MountPageAction::OpenCheatsMods(archive_path)) => {
                self.archive_context.select_only(archive_path.clone());
                self.open_cheats_mods_workspace(context, archive_path);
            }
            Some(MountPageAction::ScanRetroArchProfiles) => {
                self.start_retroarch_profile_scan(context.clone());
            }
            None => {}
        }
    }

    /// GUI Batch C: starts (or refreshes) the read-only "Plan Preview" load
    /// for the currently-ready selected-evidence report - see
    /// `plan_preview_page`'s own module doc. Explicit only (a button
    /// press); never called automatically. A no-op when the evidence
    /// report is not `Ready` (nothing to plan for yet).
    fn start_plan_preview_load(&mut self, context: egui::Context) {
        let selected_evidence_page::SelectedEvidenceState::Ready { report, .. } =
            &self.selected_evidence_ui.selected_evidence
        else {
            return;
        };
        let source_path = report.path.clone();
        let identity = report.identity_result.clone();
        let identity_presentation = report.identity.clone();
        let physical_hash = report.hashes.as_ref().map(|hashes| hashes.sha1.clone());

        self.selected_evidence_ui.plan_preview_generation += 1;
        let generation = self.selected_evidence_ui.plan_preview_generation;
        let (sender, receiver) = mpsc::channel();
        self.selected_evidence_ui.plan_preview = plan_preview_page::PlanPreviewState::Loading {
            generation,
            receiver,
        };
        thread::spawn(move || {
            let master_root = Config::load_default()
                .ok()
                .and_then(|config| config.master_rom_root);
            let outcome = plan_preview_page::gather_plan_preview(
                &source_path,
                &identity,
                &identity_presentation,
                physical_hash.as_deref(),
                master_root.as_deref(),
            );
            let _ = sender.send((generation, outcome));
            context.request_repaint();
        });
    }

    /// GUI Batch C: applies the pure
    /// [`plan_preview_page::PlanPreviewAction`] the panel returned this
    /// frame - the only thing it can ever ask for, and read-only.
    fn handle_plan_preview_action(
        &mut self,
        context: &egui::Context,
        action: Option<plan_preview_page::PlanPreviewAction>,
    ) {
        if let Some(plan_preview_page::PlanPreviewAction::Load) = action {
            self.start_plan_preview_load(context.clone());
        }
    }

    /// GUI Batch C: drains a completed plan-preview load, discarding
    /// anything whose generation is no longer current - the same
    /// stale-result guard `poll_identity_sources` already uses.
    fn poll_plan_preview(&mut self) {
        if let plan_preview_page::PlanPreviewState::Loading {
            generation,
            receiver,
        } = &self.selected_evidence_ui.plan_preview
            && let Ok((message_generation, outcome)) = receiver.try_recv()
            && message_generation == *generation
        {
            self.selected_evidence_ui.plan_preview = plan_preview_page::PlanPreviewState::Ready {
                generation: message_generation,
                outcome,
            };
        }
    }

    /// Looks up the remembered profile id for an adapter key (`"dolphin"`
    /// or `"xenia"`), if any.
    fn remembered_profile_id(&self, adapter: &str) -> Option<String> {
        remembered_profile_for(
            &self.emulator_readiness.remembered_emulator_profiles,
            adapter,
        )
        .filter(|profile| adapter != "dolphin" || !is_dolphin_standard_fallback_root(&profile.root))
        .map(|profile| profile.profile_id.clone())
    }

    /// Looks up the remembered profile's root directory for an adapter
    /// key - used to seed the explicit-root text field so a remembered
    /// portable/explicit profile is rediscovered without the user typing
    /// it again every session.
    fn remembered_profile_root(&self, adapter: &str) -> Option<PathBuf> {
        remembered_profile_for(
            &self.emulator_readiness.remembered_emulator_profiles,
            adapter,
        )
        .filter(|profile| adapter != "dolphin" || !is_dolphin_standard_fallback_root(&profile.root))
        .map(|profile| profile.root.clone())
    }

    /// Persists `profile_id`/`root` as the remembered profile for
    /// `adapter`, updating the in-memory cache immediately so the rest of
    /// the session sees it without a reload. The write is a small local
    /// file (atomic rename) - failures are non-fatal and only recorded in
    /// the Activity Log, never surfaced as a blocking error, since the
    /// session-level selection already succeeded regardless of whether it
    /// could be remembered for next time.
    fn persist_remembered_profile(&mut self, adapter: &str, profile_id: &str, root: &Path) {
        let already_current = remembered_profile_for(
            &self.emulator_readiness.remembered_emulator_profiles,
            adapter,
        )
        .is_some_and(|profile| profile.profile_id == profile_id && profile.root == root);
        if already_current {
            return;
        }
        // The real on-disk write is skipped under `cargo test`: it would
        // otherwise write to the developer's actual
        // `~/.config/archivefs/emulator_profiles.toml` every time a test
        // drives profile discovery to a resolved selection, exactly the
        // kind of real-filesystem side effect the rest of this test suite
        // never has (config-mutating core functions are only ever called
        // from the real app entry point, never from GUI unit tests). The
        // in-memory cache is still updated unconditionally, so selection
        // and chooser behavior remain fully testable.
        #[cfg(not(test))]
        let write_result = archivefs_core::patch_manager::remember_emulator_profile_default(
            adapter, profile_id, root,
        );
        #[cfg(test)]
        let write_result: Result<(), ArchiveFsError> = Ok(());
        match write_result {
            Ok(()) => {
                self.emulator_readiness
                    .remembered_emulator_profiles
                    .retain(|profile| profile.adapter != adapter);
                self.emulator_readiness.remembered_emulator_profiles.push(
                    RememberedEmulatorProfile {
                        adapter: adapter.to_string(),
                        profile_id: profile_id.to_string(),
                        root: root.to_path_buf(),
                    },
                );
            }
            Err(error) => {
                let action = if adapter == "xenia" {
                    ActivityAction::XeniaProfileScan
                } else {
                    ActivityAction::DolphinProfileScan
                };
                self.history.record(HistoryEntry::new(
                    action,
                    None,
                    ActivityOutcome::Failed,
                    format!("Could not remember the chosen {adapter} profile: {error}"),
                ));
            }
        }
    }
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

impl Drop for ArchiveFsApp {
    fn drop(&mut self) {
        // Database and source actions include the GUI's catalogue scan paths.
        // Retaining and joining these handles prevents a normal window close
        // from abandoning SQLite in the middle of a write transaction. This
        // cannot run for SIGKILL, power loss, or process abort; SQLite's
        // rollback journal remains the safety boundary for those cases.
        if let DatabaseState::Loading { worker, .. } = &mut self.database_state
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
        if let Some(mut action) = self.source_action.take()
            && let Some(worker) = action.worker.take()
        {
            let _ = worker.join();
        }
    }
}

impl ArchiveFsApp {
    fn start_platform_artwork_task(
        &mut self,
        context: egui::Context,
        action: PlatformArtworkManagerAction,
    ) {
        if self.platform_artwork_manager.task.is_some() {
            return;
        }
        let Some(root) = self.custom_platform_artwork_directory.clone() else {
            self.platform_artwork_manager.message = Some((
                false,
                "EmuWiz could not resolve its local data directory.".to_owned(),
            ));
            return;
        };
        if matches!(action, PlatformArtworkManagerAction::OpenFolder) {
            if let Err(error) = std::fs::create_dir_all(&root)
                .map_err(ArchiveFsError::from)
                .and_then(|()| open_folder_in_file_manager(&root))
            {
                self.platform_artwork_manager.message = Some((false, error.to_string()));
            }
            return;
        }
        let preview = self.platform_artwork_manager.bulk_preview.clone();
        let replace = self.platform_artwork_manager.replace_existing;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            use archivefs_core::platform_artwork as artwork;
            let result = match action {
                PlatformArtworkManagerAction::Rescan => PlatformArtworkTaskResult::Status(
                    artwork::inspect_platform_artwork(&root).map_err(|error| error.to_string()),
                ),
                PlatformArtworkManagerAction::Import {
                    platform_id,
                    source,
                } => PlatformArtworkTaskResult::Mutation(
                    artwork::import_platform_artwork(&root, &platform_id, &source, replace)
                        .map(|result| {
                            format!(
                                "Imported {} as {}{}.",
                                platform_id,
                                result.destination.display(),
                                result
                                    .warnings
                                    .first()
                                    .map(|warning| format!(" Warning: {warning}"))
                                    .unwrap_or_default()
                            )
                        })
                        .map_err(|error| error.to_string()),
                ),
                PlatformArtworkManagerAction::PreviewFolder(source) => {
                    PlatformArtworkTaskResult::BulkPreview(
                        artwork::preview_import_folder(&root, &source)
                            .map_err(|error| error.to_string()),
                    )
                }
                PlatformArtworkManagerAction::ApplyFolder => PlatformArtworkTaskResult::Mutation(
                    preview
                        .ok_or_else(|| "Run folder preview before importing.".to_owned())
                        .and_then(|preview| {
                            artwork::apply_import_folder(&root, &preview, replace)
                                .map_err(|error| error.to_string())
                        })
                        .map(|result| {
                            format!(
                                "Imported {} image(s); {} item(s) remained for review.",
                                result.imported.len(),
                                result.skipped.len()
                            )
                        }),
                ),
                PlatformArtworkManagerAction::Remove(platform_id) => {
                    PlatformArtworkTaskResult::Mutation(
                        artwork::remove_custom_platform_artwork(&root, &platform_id, true)
                            .map(|removed| {
                                if removed {
                                    format!("Restored the default artwork for {platform_id}.")
                                } else {
                                    format!("No custom artwork existed for {platform_id}.")
                                }
                            })
                            .map_err(|error| error.to_string()),
                    )
                }
                PlatformArtworkManagerAction::OpenFolder => unreachable!(),
            };
            let _ = sender.send(result);
            context.request_repaint();
        });
        self.platform_artwork_manager.task = Some(receiver);
    }

    fn poll_platform_artwork_task(&mut self, context: &egui::Context) {
        let Some(receiver) = &self.platform_artwork_manager.task else {
            return;
        };
        let Ok(result) = receiver.try_recv() else {
            return;
        };
        self.platform_artwork_manager.task = None;
        match result {
            PlatformArtworkTaskResult::Status(result) => match result {
                Ok(status) => {
                    self.platform_artwork_manager.status = Some(status);
                }
                Err(error) => self.platform_artwork_manager.message = Some((false, error)),
            },
            PlatformArtworkTaskResult::BulkPreview(result) => match result {
                Ok(preview) => {
                    self.platform_artwork_manager.bulk_preview = Some(preview);
                    self.platform_artwork_manager.message = Some((
                        true,
                        "Folder preview complete; nothing was written.".to_owned(),
                    ));
                }
                Err(error) => self.platform_artwork_manager.message = Some((false, error)),
            },
            PlatformArtworkTaskResult::Mutation(result) => {
                self.platform_artwork_cache.clear();
                self.platform_artwork_manager.message = Some(match result {
                    Ok(message) => (true, message),
                    Err(error) => (false, error),
                });
                self.start_platform_artwork_task(
                    context.clone(),
                    PlatformArtworkManagerAction::Rescan,
                );
            }
        }
    }

    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.reconcile_library_tab();
        self.reconcile_problems_repair_tab();
        self.reconcile_sources_tab();
        self.reconcile_selected_evidence_selection();
        self.reconcile_archive_preparation();
        self.poll_needs_attention(context);
        self.poll_platform_artwork_task(context);
        self.poll_shared_history();
        // Gamer View's "Undo last change" (docs/GUI_NAVIGATION_RESET_DESIGN.md
        // mandatory risk #2) drives this exact same `shared_rollback` state
        // machine while `self.view` stays `Library` (Gamer View never sets
        // `MainView::HistoryLogs`) - so this Advanced-View-only cleanup rule
        // must not fire while in Gamer View, or a rollback preview/review
        // started from Gamer View would be reset to `Idle` before the user
        // ever sees it. Advanced View's own behaviour (reset on leaving
        // History & Logs) is completely unchanged.
        if self.view != MainView::HistoryLogs
            && self.ui_mode != GuiMode::GamerView
            && matches!(
                self.shared_rollback,
                SharedRollbackState::Previewing { .. } | SharedRollbackState::Review { .. }
            )
        {
            self.shared_rollback = SharedRollbackState::Idle;
        }
        self.poll_shared_rollback();
        if self.view == MainView::HistoryLogs
            && matches!(self.shared_history, SharedHistoryState::NotLoaded)
        {
            self.refresh_shared_history(context.clone());
        }
        self.poll_load(context);
        self.poll_database_load(context);
        if self.dat_authority.tick(
            self.database_generation.0,
            database_state_path(&self.database_state),
            matches!(self.view, MainView::DatSources | MainView::NeedsAttention)
                && !self.database_state.is_loading(),
            context,
        ) {
            self.invalidate_needs_attention();
        }
        self.poll_diagnostics();
        self.poll_setup_action(context);
        self.poll_doctor_scan();
        self.poll_rpcs3_status();
        self.poll_pcsx2_status();
        self.poll_platform_action(context);
        self.poll_bulk_platform_action(context);
        self.poll_alias_action(context);
        self.poll_source_action(context);
        self.poll_bsfree_operation(context);
        self.poll_romm_operation(context);
        self.poll_catalogue_manager(context);
        self.poll_dolphin_catalogue_manager(context);
        self.poll_library_view_action(context);
        self.poll_archive_inspection();
        self.poll_archive_preparation(context);
        self.poll_selected_evidence();
        self.poll_identity_sources();
        self.poll_plan_preview();
        self.poll_missing_removal(context);
        self.poll_operation(context);
        self.poll_mount_all(context);
        self.poll_unmount_all(context);
        self.poll_retroarch_profiles();
        self.poll_pcsx2_profiles();
        self.poll_dolphin_profiles();
        self.poll_dolphin_local_profiles();
        self.poll_pcsx2_launch_profiles();
        self.poll_flycast_profiles();
        self.poll_pcsx2_firmware_evidence();
        self.poll_cheat_workflow(context);
        self.cheatbase_page.poll(context);
        self.emulator_download_page.poll(context);
        if let Some(_installed_id) = self.emulator_download_page.take_completed_install() {
            // A managed AppImage was just installed: re-run the read-only
            // discovery / Doctor / readiness so Emulator Setup and Play
            // availability reflect it. Discovery is authoritative - the
            // download page never asserts launch readiness itself.
            self.start_doctor_scan(context.clone());
        }
        if matches!(
            self.view,
            MainView::Sources | MainView::CheatsMods | MainView::CheatSources
        ) && matches!(
            self.catalogue_bsfree_ui.bsfree_manager,
            BsFreeManagerState::NotLoaded
        ) && self.catalogue_bsfree_ui.bsfree_operation.is_none()
        {
            self.start_bsfree_operation(context.clone(), BsFreeOperation::LoadStatus);
        }
        // Only once the Sources page is actually open, and only local reads - so
        // starting EmuWiz still makes no network request of any kind.
        if self.view == MainView::Sources
            && self.romm_ui.snapshot.is_none()
            && self.romm_ui.operation.is_none()
        {
            self.start_romm_status_load(context.clone());
        }
        if self.view == MainView::CheatsMods
            && self.cheat_workflow.as_ref().is_some_and(|workflow| {
                workflow.adapter == CheatEmulatorAdapter::Dolphin
                    && workflow.selected_dolphin_profile_id.is_some()
                    && matches!(workflow.dolphin_inventory, CheatStepResource::NotLoaded)
            })
            && matches!(
                self.emulator_readiness.dolphin_profiles,
                DolphinProfilesState::Ready(_)
            )
        {
            self.start_dolphin_inventory(context.clone());
        }
        if catalogue_status_load_needed(self.view, &self.catalogue_bsfree_ui.catalogue_manager) {
            self.start_catalogue_status_load(context.clone());
        }
        if dolphin_catalogue_status_load_needed(
            self.view,
            &self.catalogue_bsfree_ui.dolphin_catalogue_manager,
        ) {
            self.start_dolphin_catalogue_status_load(context.clone());
        }
        // The one quiet, automatic "Check for updates" per session: only
        // once a catalogue is confirmed installed, and only once ever
        // (`dolphin_catalogue_update_available` starts `None` and this is
        // the only place that can set it besides an explicit click).
        if self.view == MainView::CheatsMods
            && self
                .catalogue_bsfree_ui
                .dolphin_catalogue_update_available
                .is_none()
            && self
                .catalogue_bsfree_ui
                .dolphin_catalogue_update_check
                .is_none()
            && matches!(
                &self.catalogue_bsfree_ui.dolphin_catalogue_manager,
                DolphinCatalogueManagerState::Ready(snapshot) if snapshot.catalogue.is_some()
            )
        {
            self.start_dolphin_catalogue_update_check(context.clone());
        }
        if self.view == MainView::CheatsMods
            && self.cheat_workflow.as_ref().is_some_and(|workflow| {
                workflow.identity_request.is_none()
                    && matches!(workflow.identity, CheatStepResource::NotLoaded)
            })
        {
            self.start_game_identity_inspection(context.clone());
        }
        if self.view == MainView::CheatsMods
            && self.cheat_workflow.as_ref().is_some_and(|workflow| {
                workflow.preview_request.is_none()
                    && matches!(workflow.preview, CheatStepResource::NotLoaded)
            })
        {
            self.start_cheat_preview(context.clone());
        }
        // Matching is deliberately manual only - triggered by the "Find
        // matching cheat files" button (`start_cheat_candidate_match`),
        // never auto-started here. An earlier version auto-triggered this
        // every frame whenever `candidates` was `NotLoaded`; when a
        // prerequisite silently failed, that auto-trigger raced the
        // button's own click on the same silent failure, forever, with
        // neither ever producing a visible result. Matching now has
        // exactly one entry point, and every call to it produces a visible
        // state (see `start_cheat_candidate_match`'s doc comment).
        let loading = matches!(self.state, LoadState::Loading { .. });
        let diagnostics_loading = matches!(
            self.doctor_repair.diagnostics,
            DiagnosticsState::Loading { .. }
        );
        let busy = self.is_busy();
        let actions_safe = latest_generation_actions_safe(
            self.refresh_generation,
            self.snapshot_generation,
            self.snapshot_stale,
            snapshot_identity(&self.state),
            &self.doctor_repair.diagnostics,
        );
        let archive_actions_blocked = busy || !actions_safe;
        let archive_action_block_reason = archive_action_block_reason(
            busy,
            self.refresh_generation,
            self.snapshot_generation,
            self.snapshot_stale,
            snapshot_identity(&self.state),
            &self.doctor_repair.diagnostics,
        );
        let action_readiness_debug_lines = action_readiness_debug_lines(
            busy,
            self.refresh_generation,
            self.snapshot_generation,
            self.snapshot_stale,
            snapshot_identity(&self.state),
            &self.doctor_repair.diagnostics,
        );
        let missing_removal_available = self.missing_removal_action_available();
        if loading || diagnostics_loading || busy {
            context.request_repaint_after(std::time::Duration::from_millis(100));
        }

        let has_database = self.database_state.snapshot().is_some();

        let mut navigation_request = None;
        if self.ui_mode == GuiMode::AdvancedView {
            egui::TopBottomPanel::top("menu_bar").show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(main_view_title(self.view)).strong());
                    ui.separator();
                    ui.menu_button("File", |ui| {
                        if ui.button("Quit").clicked() {
                            context.send_viewport_cmd(egui::ViewportCommand::Close);
                            ui.close();
                        }
                    });
                    ui.menu_button("Library", |ui| {
                        if ui
                            .add_enabled(
                                !loading && !busy,
                                egui::Button::new("Scan library"),
                            )
                            .on_hover_text("Scan your configured source folders for new and changed files.")
                            .clicked()
                        {
                            self.start_database_action(context.clone(), true);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new("Refresh database status"),
                            )
                            .on_hover_text(
                                "Re-read the catalogue database status without rescanning your folders.",
                            )
                            .clicked()
                        {
                            self.start_database_action(context.clone(), false);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button("Select all visible")
                            .on_hover_text("Select every archive currently shown in the Library.")
                            .clicked()
                        {
                            self.select_all_visible_requested = true;
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                !self.archive_context.selected.is_empty(),
                                egui::Button::new("Clear selection"),
                            )
                            .on_hover_text("Deselect every selected archive.")
                            .clicked()
                        {
                            self.archive_context.clear_selection();
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                !loading && !busy,
                                egui::Button::new("Refresh"),
                            )
                            .on_hover_text(
                                "Refresh EmuWiz's current view of your files without running a full scan.",
                            )
                            .clicked()
                        {
                            self.refresh(context);
                            ui.close();
                        }
                    });
                    ui.menu_button("Sources", |ui| {
                        if ui.button("Open Sources page").clicked() {
                            self.view = MainView::Sources;
                            self.tools_overlay = ToolsOverlay::None;
                            ui.close();
                        }
                        if ui
                            .button("RomM")
                            .on_hover_text(
                                "Connect EmuWiz to your RomM server and browse its records \
                                 (Sources -> Libraries).",
                            )
                            .clicked()
                        {
                            self.navigate_to_sources_tab(SourcesTab::Libraries);
                            ui.close();
                        }
                    });
                    ui.menu_button("Tools", |ui| {
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new("Diagnostics"),
                            )
                            .on_hover_text(
                                "Check configuration, mount root and source-folder health.",
                            )
                            .clicked()
                        {
                            self.tools_overlay = ToolsOverlay::Diagnostics;
                            self.refresh_diagnostics(context);
                            ui.close();
                        }
                        if ui
                            .button("Doctor checks")
                            .on_hover_text("Run the read-only Doctor scan of this EmuWiz installation.")
                            .clicked()
                        {
                            self.tools_overlay = ToolsOverlay::DoctorChecks;
                            ui.close();
                        }
                        if ui
                            .button("Platform Aliases")
                            .on_hover_text("Review the folder and filename aliases EmuWiz uses to recognise platforms.")
                            .clicked()
                        {
                            self.tools_overlay = ToolsOverlay::PlatformAliases;
                            ui.close();
                        }
                        if ui
                            .button("Database Status")
                            .on_hover_text("Inspect the catalogue database and its health.")
                            .clicked()
                        {
                            self.tools_overlay = ToolsOverlay::DatabaseStatus;
                            ui.close();
                        }
                        // Collection Discovery moved to the Sources group of
                        // the grouped sidebar (docs/GUI_NAVIGATION_RESET_
                        // DESIGN.md §3.2, Phase 2) - it lives naturally next
                        // to Sources/DAT Sources now rather than in this
                        // generic Tools menu; see `ADVANCED_NAV_GROUPS`.
                        ui.separator();
                        // Major workflows, also on the sidebar and Home -
                        // exposed here so they never depend on returning to
                        // Home to be found. Rendered from `TOOLS_MENU_WORKFLOWS`
                        // so the label and destination a test asserts are the
                        // exact ones this menu uses; every route converges on
                        // the same `MainView` (see
                        // `major_workflows_are_reachable_from_home_sidebar_and_top_menu`).
                        for (label, hover, target) in TOOLS_MENU_WORKFLOWS {
                            if ui.button(label).on_hover_text(hover).clicked() {
                                self.navigate_to_main_view(target);
                                if target == MainView::DiscConversion {
                                    self.optical_conversion_page.get_or_insert_with(
                                        optical_conversion_page::OpticalConversionPageState::default,
                                    );
                                }
                                ui.close();
                            }
                        }
                        ui.separator();
                        let activity_label = if self.show_activity {
                            "Hide Activity"
                        } else {
                            "Show Activity"
                        };
                        if ui
                            .button(activity_label)
                            .on_hover_text("Show or hide the recent-activity panel.")
                            .clicked()
                        {
                            self.show_activity = !self.show_activity;
                            ui.close();
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("About EmuWiz").clicked() {
                            self.show_about = true;
                            ui.close();
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if loading || busy {
                            ui.spinner();
                        }
                        // Decision 7 (docs/GUI_NAVIGATION_RESET_DESIGN.md §9):
                        // a clear, always-visible way back - never gear-hidden,
                        // never buried.
                        if ui
                            .button("Return to Gamer View")
                            .on_hover_text("Switch back to the simple, one-screen view.")
                            .clicked()
                        {
                            self.ui_mode = GuiMode::GamerView;
                            self.view = MainView::Library;
                            self.tools_overlay = ToolsOverlay::None;
                            save_gui_mode(self.ui_mode);
                        }
                    });
                });
            });

            egui::SidePanel::left("app_navigation")
                .resizable(false)
                .exact_width(218.0)
                .show(context, |ui| {
                    ui.add_space(14.0);
                    navigation_request =
                        show_primary_navigation(ui, self.view, self.tools_overlay, has_database);
                });
        } else {
            // Decision 6 (docs/GUI_NAVIGATION_RESET_DESIGN.md §9): reached
            // through a small gear menu, not a permanent top-level control.
            egui::TopBottomPanel::top("gamer_top_bar").show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if loading || busy {
                            ui.spinner();
                        }
                        ui.menu_button(GAMER_MENU_LABEL, |ui| {
                            let source_actions_available = !busy && self.source_action_available();
                            if ui
                                .add_enabled(
                                    source_actions_available,
                                    egui::Button::new(GAMER_MENU_ADD_FOLDER_LABEL),
                                )
                                .on_hover_text(
                                    "Choose another folder for EmuWiz to scan for games.",
                                )
                                .clicked()
                            {
                                if let Some(folder) = rfd::FileDialog::new()
                                    .set_title("Choose another games folder")
                                    .pick_folder()
                                {
                                    self.gamer_view_scan_review_available = false;
                                    self.start_source_action(
                                        context.clone(),
                                        SourceAction::Add(folder),
                                    );
                                }
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    source_actions_available,
                                    egui::Button::new(GAMER_MENU_SCAN_LABEL),
                                )
                                .on_hover_text(
                                    "Look through all enabled game folders for new and changed games.",
                                )
                                .clicked()
                            {
                                self.gamer_view_scan_review_available = false;
                                self.gamer_view_scan_pending_review = true;
                                self.start_source_action(context.clone(), SourceAction::ScanAll);
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .button(GAMER_MENU_SETUP_LABEL)
                                .on_hover_text("Check emulator setup and launch readiness.")
                                .clicked()
                            {
                                self.ui_mode = GuiMode::AdvancedView;
                                save_gui_mode(self.ui_mode);
                                self.navigate_to_main_view(MainView::EmulatorSetup);
                                ui.close();
                            }
                            if ui.button(GAMER_MENU_ADVANCED_LABEL).clicked() {
                                self.switch_to_advanced_view_at_home();
                                save_gui_mode(self.ui_mode);
                                ui.close();
                            }
                        });
                    });
                });
            });
        }
        match navigation_request {
            Some(NavClick::View(view)) => {
                self.navigate_to_main_view(view);
                // Disc Conversion's dispatch lazily creates its page state,
                // but seed it here too so the page is populated the instant
                // the sidebar entry is clicked - the same thing Home's card
                // and the top menu do.
                if view == MainView::DiscConversion {
                    self.optical_conversion_page.get_or_insert_with(
                        optical_conversion_page::OpticalConversionPageState::default,
                    );
                }
            }
            Some(NavClick::QuickRename) => {
                self.quick_rename_mode = true;
                self.navigate_to_main_view(MainView::IdentifyRename);
            }
            Some(NavClick::Overlay(overlay)) => self.tools_overlay = overlay,
            Some(NavClick::Romm) => self.navigate_to_sources_tab(SourcesTab::Libraries),
            None => {}
        }

        if self.ui_mode == GuiMode::AdvancedView
            && let Some(ActivityPanelAction::ShowRelatedArchive(path)) = show_activity_panel(
                context,
                &mut self.history,
                &mut self.show_activity,
                &mut self.clipboard,
            )
        {
            self.navigate_to_library_tab(LibraryTab::Archives);
            self.archive_context.select_only(path);
        }

        if self.show_about {
            let mount_root = match &self.state {
                LoadState::Ready(data) => Some(data.mount_root.as_path()),
                _ => None,
            };
            show_about_window(
                context,
                &mut self.show_about,
                &self.database_state,
                &self.doctor_repair.diagnostics,
                mount_root,
                &mut self.clipboard,
            );
        }

        if self.show_skipped_files {
            let summary = match &self.database_state {
                DatabaseState::Ready {
                    last_scan_summary: Some(summary),
                    ..
                } => Some(summary),
                _ => None,
            };
            show_skipped_files_window(
                context,
                &mut self.show_skipped_files,
                summary,
                &mut self.skipped_files_filter,
            );
        }

        let mut retry = false;
        let mut requested_action = None;
        let mut diagnostics_action = None;
        let mut health_dashboard_action = None;
        let mut stop_mount_all = false;
        let mut stop_unmount_all = false;
        egui::CentralPanel::default().show(context, |ui| {
            let width = if self.tools_overlay == ToolsOverlay::None {
                main_view_content_width(self.view)
            } else if self.tools_overlay == ToolsOverlay::ArchiveInspector {
                ui_layout::ContentWidth::Wide
            } else {
                ui_layout::ContentWidth::Normal
            };
            let page_scroll = main_view_uses_page_scroll(self.view)
                || (self.ui_mode == GuiMode::GamerView && self.view == MainView::Library);
            ui_layout::page(ui, width, page_scroll, self.view, |ui| {
                self.reconcile_cheats_mods_context(context);

                if self.tools_overlay != ToolsOverlay::None {
                    match self.tools_overlay {
                        ToolsOverlay::Diagnostics => {
                            diagnostics_action = show_setup_diagnostics(
                                ui,
                                &self.doctor_repair.diagnostics,
                                self.setup_action.is_some(),
                                self.feedback.as_ref(),
                                self.refresh_error.as_deref(),
                                self.snapshot_stale && matches!(self.state, LoadState::Ready(_)),
                                self.doctor_repair.config_previously_confirmed,
                            );
                        }
                        ToolsOverlay::PlatformAliases => {
                            if widgets::show_tools_overlay_header(ui, "Platform Aliases") {
                                self.tools_overlay = ToolsOverlay::None;
                            }
                            let cached_aliases = self
                                .database_state
                                .snapshot()
                                .map(|snapshot| snapshot.platform_aliases.as_slice())
                                .unwrap_or(&[]);
                            if let Some(action) = show_platform_aliases_panel(
                                ui,
                                cached_aliases,
                                &mut self.library_ui.new_alias_text,
                                &mut self.library_ui.new_alias_platform_choice,
                                self.library_ui.alias_action.is_some(),
                                &mut self.clipboard,
                            ) {
                                self.start_alias_action(context.clone(), action);
                            }
                        }
                        ToolsOverlay::DatabaseStatus => {
                            if widgets::show_tools_overlay_header(ui, "Database Status") {
                                self.tools_overlay = ToolsOverlay::None;
                            }
                            if let Some(action) = show_database_panel(ui, &self.database_state) {
                                match action {
                                    DatabasePanelAction::ScanLibrary
                                    | DatabasePanelAction::ScanAndUpgradeLibrary => {
                                        self.start_database_action(context.clone(), true);
                                    }
                                    DatabasePanelAction::ViewRecentlyFound => {
                                        self.navigate_to_library_tab(LibraryTab::RecentlyFound);
                                    }
                                    DatabasePanelAction::RefreshStatus
                                    | DatabasePanelAction::RetryLoad => {
                                        self.start_database_action(context.clone(), false);
                                    }
                                    DatabasePanelAction::ViewSkippedFiles => {
                                        self.show_skipped_files = true;
                                        self.skipped_files_filter = None;
                                    }
                                }
                            }
                        }
                        // The original `DoctorReport` checks, unchanged. The
                        // Doctor *page* now shows the shared read-only
                        // findings instead (see `show_doctor_page`), so this
                        // overlay keeps the older per-check view - and the
                        // summary/report text that belongs with it -
                        // reachable rather than dropping either.
                        ToolsOverlay::DoctorChecks => {
                            if widgets::show_tools_overlay_header(ui, "Doctor Checks") {
                                self.tools_overlay = ToolsOverlay::None;
                            }
                            let doctor = match &self.state {
                                LoadState::Ready(data) => Some(&data.doctor),
                                _ => None,
                            };
                            if let Some(report) = doctor {
                                ui.label(doctor_summary_text(report));
                                if widgets::action_button(
                                    ui,
                                    "Copy summary",
                                    widgets::ActionStyle::Secondary,
                                    true,
                                )
                                .clicked()
                                {
                                    let _ = self.clipboard.set_text(doctor_report_text(report));
                                }
                                ui.add_space(8.0);
                            }
                            show_doctor_checks_panel(ui, doctor);
                        }
                        ToolsOverlay::ArchiveInspector => {
                            if let Some(inspector) = self.archive_inspector.as_mut()
                                && show_archive_inspector_panel(ui, inspector, &mut self.clipboard)
                            {
                                self.tools_overlay = ToolsOverlay::None;
                            }
                        }
                        ToolsOverlay::Onboarding => {
                            self.show_onboarding_overlay(ui, context);
                        }
                        ToolsOverlay::None => unreachable!(),
                    }
                    return;
                }

                // docs/GUI_NAVIGATION_RESET_DESIGN.md: Gamer View is one
                // screen (`self.view` stays at its default, `Library`,
                // the whole time it's active) plus the existing,
                // unmodified Cheats & Mods page when opened from the
                // selected-game action panel below. Every other
                // `MainView` destination is Advanced-View-only and
                // unreachable while `ui_mode` is `GamerView`, since
                // nothing in this mode's UI ever sets `self.view` to one.
                if self.ui_mode == GuiMode::GamerView && self.view != MainView::CheatsMods {
                    self.es_de_media.start(ui.ctx().clone());
                    if self.es_de_media.poll() {
                        self.gamer_covers.identity_refreshed();
                        self.gamer_screenshots.identity_refreshed();
                        if let Some(worker) = self.gamer_cover_worker.as_ref() {
                            worker.update_esde(self.es_de_media.snapshot().cloned());
                        }
                    }
                    if let Some(path) = self.archive_context.focused.clone() {
                        let evidence_is_stale = match &self.selected_evidence_ui.selected_evidence {
                            selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                                report.path != path
                            }
                            selected_evidence_page::SelectedEvidenceState::Loading {
                                path: loading_path, ..
                            } => loading_path != &path,
                            selected_evidence_page::SelectedEvidenceState::Idle => true,
                            selected_evidence_page::SelectedEvidenceState::Error {
                                path: error_path, ..
                            } => error_path != &path,
                        };
                        if evidence_is_stale {
                            self.start_selected_evidence_load(context.clone(), path);
                        }
                    }
                    self.maybe_start_selected_evidence_enrichment(context);
                    if matches!(self.emulator_readiness.retroarch_profiles, RetroArchProfilesState::NotScanned) {
                        self.start_retroarch_profile_scan(context.clone());
                    }
                    let data = match &self.state {
                        LoadState::Ready(data) => Some(data.as_ref()),
                        LoadState::Loading { previous, .. } => previous.as_deref(),
                        LoadState::Error(_) => None,
                    };
                    // A reloaded library may map the same path to a different
                    // archive, and a re-imported RomM catalogue changes what any
                    // path resolves to, so both discard every answer rather than
                    // risk drawing one game's cover beside another. A search or a
                    // platform change does neither: it narrows which records are
                    // visible without changing what any of them is, so covers
                    // already loaded stay loaded and are not fetched twice.
                    let library = data.map(|data| data.config_identity.clone());
                    if self.gamer_cover_library != library {
                        self.gamer_cover_library = library;
                        self.gamer_covers.library_changed();
                        self.gamer_screenshots.library_changed();
                    }
                    // Answers first, so a cover that arrived since the last frame
                    // is drawn in this one. Anything from a superseded generation
                    // is dropped inside `absorb`.
                    if let Some(worker) = self.gamer_cover_worker.as_ref() {
                        for update in worker.drain_delivery() {
                            self.gamer_covers.absorb_delivery(&update);
                            self.gamer_screenshots.absorb_delivery(&update);
                        }
                        for reply in worker.drain() {
                            if !self.gamer_covers.absorb(ui.ctx(), reply.clone()) {
                                self.gamer_screenshots.absorb(ui.ctx(), reply);
                            }
                        }
                    }
                    let mut cover_requests: Vec<crate::gamer_artwork::CoverJob> = Vec::new();
                    let mut screenshot_requests: Vec<crate::gamer_artwork::CoverJob> = Vec::new();
                    // Enrichment (synopsis/genre/players/rating/release year):
                    // answers first, same as covers above, then a request only
                    // when the focused game actually changed - never once per
                    // frame, and never for a row that merely scrolled into view.
                    if let Some(worker) = self.game_metadata_worker.as_mut() {
                        for reply in worker.poll() {
                            self.selected_game_metadata = Some((reply.local_path, reply.result));
                        }
                    }
                    let focused_archive = self.archive_context.focused.clone();
                    let metadata_is_stale = self
                        .selected_game_metadata
                        .as_ref()
                        .map(|(path, _)| path)
                        != focused_archive.as_ref();
                    if metadata_is_stale
                        && let Some(path) = focused_archive.as_ref()
                        && self.game_metadata_worker_allowed
                    {
                        let worker = self.game_metadata_worker.get_or_insert_with(|| {
                            crate::game_metadata::GameMetadataWorker::start(ui.ctx().clone())
                        });
                        worker.request(path);
                    }
                    let game_metadata = self
                        .selected_game_metadata
                        .as_ref()
                        .filter(|(path, _)| Some(path) == focused_archive.as_ref())
                        .map(|(_, result)| result);
                    let gamer_launch_input = self.build_launch_readiness_input(data);
                    let gamer_play_action =
                        launch_readiness_page::gamer_play_action(&gamer_launch_input);
                    let gamer_identity_status = match &self.selected_evidence_ui.selected_evidence {
                        selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                            Some(gamer_identity_status_from_verdict(report.identity.status))
                        }
                        selected_evidence_page::SelectedEvidenceState::Loading { .. }
                        | selected_evidence_page::SelectedEvidenceState::Idle => {
                            Some(GamerIdentityStatus::StillChecking)
                        }
                        selected_evidence_page::SelectedEvidenceState::Error { .. } => None,
                    };
                    let (prepared_member, member_choices_owned, preparation_message_owned) =
                        focused_archive
                            .as_deref()
                            .map(|path| self.archive_preparation_view(path))
                            .unwrap_or((false, None, None));
                    let member_choices = member_choices_owned.as_deref();
                    let preparation_message = preparation_message_owned.as_deref();
                    let gamer_action = show_gamer_view(
                        ui,
                        data,
                        GamerViewViewState {
                            filter: &mut self.library_ui.filter,
                            library_filters: &mut self.library_ui.library_filters,
                            archive_context: &mut self.archive_context,
                            screen: &mut self.gamer_view_screen,
                            busy: archive_actions_blocked,
                            block_reason: archive_action_block_reason,
                            cleanup_after_unmount: self.mount_ui.cleanup_after_unmount,
                            cheat_workflow: self.cheat_workflow.as_ref(),
                            feedback: self.feedback.as_ref(),
                            scan_review_available: self.gamer_view_scan_review_available,
                            artwork_directory: self.custom_platform_artwork_directory.as_deref(),
                            artwork_cache: &mut self.platform_artwork_cache,
                            covers: &mut self.gamer_covers,
                            screenshots: &mut self.gamer_screenshots,
                            cover_requests: &mut cover_requests,
                            screenshot_requests: &mut screenshot_requests,
                            game_metadata,
                            identity_status: gamer_identity_status,
                            prepared_member,
                            member_choices,
                            preparation_message,
                            play_action: &gamer_play_action,
                            retroarch_launch_state: &mut self.launch_retroarch,
                            dolphin_launch_state: &mut self.launch_dolphin,
                            pcsx2_launch_state: &mut self.launch_pcsx2,
                            standalone_launch_state: &mut self.launch_standalone,
                            alpha_jump: &mut self.gamer_alpha_jump,
                        },
                    );
                    // Started only once the list has actually asked for something,
                    // so a session that never opens Gamer View never opens the
                    // catalogue, and an empty or unfiltered-to-nothing list starts
                    // no thread at all.
                    if (!cover_requests.is_empty() || !screenshot_requests.is_empty())
                        && self.gamer_cover_worker_allowed
                    {
                        let worker = self.gamer_cover_worker.get_or_insert_with(|| {
                            crate::gamer_artwork::CoverWorker::start(
                                ui.ctx().clone(),
                                self.gui_config.source_roots().ok().map(<[PathBuf]>::to_vec),
                                self.es_de_media.snapshot().cloned(),
                                self.launchbox_local_media.snapshot().cloned(),
                            )
                        });
                        let generation = self.gamer_covers.generation();
                        for job in cover_requests {
                            worker.request(generation, job);
                        }
                        for job in screenshot_requests {
                            worker.request(generation, job);
                        }
                    }
                    match gamer_action {
                        Some(GamerViewAction::Prepare(archive_path)) => {
                            self.start_archive_preparation(context.clone(), archive_path);
                        }
                        Some(GamerViewAction::SelectArchiveMember(archive_path, member_name)) => {
                            self.select_archive_member(
                                context,
                                archive_path,
                                member_name,
                            );
                        }
                        Some(GamerViewAction::Play(request)) => {
                            request.start(
                                &mut self.launch_retroarch,
                                &mut self.launch_dolphin,
                                &mut self.launch_pcsx2,
                                &mut self.launch_standalone,
                                &mut self.launch_amiga_whdload,
                            );
                        }
                        Some(GamerViewAction::Operation(request)) => {
                            if matches!(
                                request.action,
                                ArchiveAction::Unmount | ArchiveAction::LazyUnmount
                            ) {
                                self.archive_preparation_generation =
                                    self.archive_preparation_generation.next();
                                self.archive_preparation = ArchivePreparationState::Idle;
                            }
                            requested_action = Some(AppOperationRequest::Archive(request));
                        }
                        Some(GamerViewAction::OpenCheatsMods(archive_path)) => {
                            requested_action = Some(AppOperationRequest::OpenCheatsMods(archive_path));
                        }
                        Some(GamerViewAction::CopyLocation(folder)) => {
                            match self.clipboard.set_text(folder) {
                                Ok(()) => {
                                    self.feedback = Some(ActionFeedback {
                                        succeeded: true,
                                        message: "Copied the game's folder location to the clipboard.".to_string(),
                                        cleanup: None,
                                        warning: None,
                                        more_information: None,
                                    });
                                }
                                Err(error) => {
                                    self.feedback = Some(ActionFeedback {
                                        succeeded: false,
                                        message: format!("Could not copy to the clipboard: {error}"),
                                        cleanup: None,
                                        warning: None,
                                        more_information: None,
                                    });
                                }
                            }
                        }
                        Some(GamerViewAction::Undo) => {
                            self.start_cheat_install_rollback(context.clone());
                        }
                        Some(GamerViewAction::AddGamesFolder(folder)) => {
                            self.gamer_view_pending_first_scan = Some(folder.clone());
                            self.start_source_action(context.clone(), SourceAction::Add(folder));
                        }
                        Some(GamerViewAction::ReviewScan) => {
                            self.gamer_view_scan_review_available = false;
                            self.ui_mode = GuiMode::AdvancedView;
                            save_gui_mode(self.ui_mode);
                            self.navigate_to_sources_tab(SourcesTab::Discovery);
                        }
                        Some(GamerViewAction::ScanForNewGames) => {
                            self.start_source_action(context.clone(), SourceAction::ScanAll);
                        }
                        Some(GamerViewAction::ReviewIdentity(archive_path)) => {
                            self.review_identity(archive_path);
                        }
                        Some(GamerViewAction::OpenLaunchChoices(archive_path)) => {
                            self.review_identity(archive_path);
                        }
                        Some(GamerViewAction::CheckEmulators(archive_path)) => {
                            self.archive_context.select_only(archive_path);
                            self.ui_mode = GuiMode::AdvancedView;
                            save_gui_mode(self.ui_mode);
                            self.start_doctor_scan(context.clone());
                            self.navigate_to_main_view(MainView::EmulatorSetup);
                        }
                        Some(GamerViewAction::OpenEmulatorSetup(archive_path, focus)) => {
                            self.open_emulator_setup_for(archive_path, focus);
                        }
                        // A match guard here (clippy's suggestion) would make
                        // this otherwise-exhaustive `GamerViewAction` match
                        // non-exhaustive and force a redundant fallback arm.
                        #[allow(clippy::collapsible_match)]
                        Some(GamerViewAction::RefreshGameInformation) => {
                            if self.game_metadata_worker_allowed {
                                let worker = self.game_metadata_worker.get_or_insert_with(|| {
                                    crate::game_metadata::GameMetadataWorker::start(
                                        context.clone(),
                                    )
                                });
                                worker.reload();
                                // Cleared so the panel shows its "not yet resolved"
                                // state until the reload (queued strictly before
                                // this re-request on the same worker channel)
                                // finishes and answers it - never stale data
                                // presented as freshly refreshed.
                                self.selected_game_metadata = None;
                                if let Some(path) = &self.archive_context.focused {
                                    worker.request(path);
                                }
                            }
                        }
                        None => {}
                    }
                    return;
                }

                if let Some(error) = &self.refresh_error {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("Refresh failed; showing the last known snapshot: {error}"),
                    );
                    ui.separator();
                }
                if let Some(batch) = self.mount_ui.mount_all.as_ref() {
                    stop_mount_all = show_mount_all_progress(ui, &batch.progress);
                    ui.separator();
                }
                if let Some(batch) = self.mount_ui.unmount_all.as_ref() {
                    stop_unmount_all = show_unmount_all_progress(ui, &batch.progress);
                    ui.separator();
                }

                if self.view == MainView::Museum {
                    let library = self.database_state.snapshot().map(home_library_snapshot);
                    let selected_game = self.museum_selected_game();
                    let mut artwork = self.platform_artwork.render_assets();
                    if let Some(worker) = self.gamer_cover_worker.as_ref() {
                        for update in worker.drain_delivery() {
                            self.gamer_covers.absorb_delivery(&update);
                            self.gamer_screenshots.absorb_delivery(&update);
                        }
                        for reply in worker.drain() {
                            if !self.gamer_covers.absorb(ui.ctx(), reply.clone()) {
                                self.gamer_screenshots.absorb(ui.ctx(), reply);
                            }
                        }
                    }
                    let mut screenshot_requests = Vec::new();
                    let action = museum_page::show_with_selected_game_and_artwork_with_hero(
                        ui,
                        &mut self.museum_hero,
                        &mut self.museum_page,
                        library.as_ref(),
                        selected_game.as_ref(),
                        Some(&self.gamer_covers),
                        Some(&mut self.gamer_screenshots),
                        Some(&mut artwork),
                        &mut screenshot_requests,
                    );
                    if !screenshot_requests.is_empty() && self.gamer_cover_worker_allowed {
                        let worker = self.gamer_cover_worker.get_or_insert_with(|| {
                            crate::gamer_artwork::CoverWorker::start(
                                ui.ctx().clone(),
                                self.gui_config.source_roots().ok().map(<[PathBuf]>::to_vec),
                                self.es_de_media.snapshot().cloned(),
                                self.launchbox_local_media.snapshot().cloned(),
                            )
                        });
                        let generation = self.gamer_covers.generation();
                        for job in screenshot_requests {
                            worker.request(generation, job);
                        }
                    }
                    match action {
                        Some(museum_page::MuseumAction::BrowseLibraryForPlatform(_)) => {
                            self.navigate_to_library_tab(LibraryTab::Archives);
                        }
                        Some(museum_page::MuseumAction::OpenEmulatorSetup) => {
                            self.navigate_to_main_view(MainView::EmulatorSetup);
                        }
                        Some(museum_page::MuseumAction::OpenSelectedEvidence) => {
                            self.navigate_to_main_view(MainView::Selected);
                        }
                        Some(museum_page::MuseumAction::OpenCheats(path)) => {
                            self.open_cheats_mods_workspace(context, path);
                        }
                        Some(museum_page::MuseumAction::OpenRomm) => {
                            self.navigate_to_sources_tab(SourcesTab::Libraries);
                        }
                        Some(museum_page::MuseumAction::OpenDiscConversion) => {
                            self.navigate_to_main_view(MainView::DiscConversion);
                        }
                        None => {}
                    }
                    return;
                }

                if self.view == MainView::Home {
                    let source_folder_count = self
                        .gui_config
                        .source_roots()
                        .map(|roots| roots.len())
                        .unwrap_or(0);
                    let has_database = self.database_state.snapshot().is_some();
                    // `config_missing` (banner only) still comes from the
                    // background setup diagnostics; the "Set up emulators"
                    // card's readiness comes from `doctor_scan` - the same
                    // state its "Open Doctor" action lands on.
                    let config_missing = match &self.doctor_repair.diagnostics {
                        DiagnosticsState::Ready { report, .. } => report.config_missing,
                        DiagnosticsState::Loading { .. } | DiagnosticsState::Error { .. } => false,
                    };
                    let setup_check = setup_check_summary(&self.doctor_repair.doctor_scan);
                    let first_run = missing_config_is_first_run(self.doctor_repair.config_previously_confirmed);
                    // Never triggers the load these pages themselves start
                    // on first visit - `None` here means "not visited yet
                    // this session", not "not configured".
                    let cheat_sources_enabled_count = self
                        .cheat_sources_page
                        .as_ref()
                        .map(|page| page.enabled_source_count());
                    let dat_sources_registered_count = self
                        .dat_sources_page
                        .as_ref()
                        .map(|page| page.registered_source_count());
                    let romm_state_label = self
                        .romm_ui.snapshot
                        .as_ref()
                        .map(|snapshot| romm_readiness_label(&snapshot.status.state));

                    let home_inputs = home_page::HomeInputs {
                        source_folder_count,
                        has_database,
                        setup_check,
                        config_missing,
                        first_run,
                        cheat_sources_enabled_count,
                        dat_sources_registered_count,
                        romm_state_label,
                    };
                    let home_view = home_page::build_home_view(&home_inputs);
                    if let Some(card) = home_page::show_home_page(ui, &home_view) {
                        self.quick_rename_mode = card == home_page::HomeCard::QuickRename;
                        self.navigate_to_home_card(card);
                    }
                }

                if self.view == MainView::NeedsAttention {
                    if let Some(destination) = needs_attention::show_needs_attention_page(
                        ui,
                        &mut self.needs_attention,
                    ) {
                        self.navigate_attention(destination);
                    }
                    return;
                }

                if let Some(tab) = sources_tab_for_main_view(self.view) {
                    self.show_sources_page(context, ui, tab);
                    return;
                }

                if self.view == MainView::CanonicalOrganisation {
                    self.show_rom_organisation_page(ui);
                    return;
                }

                if self.view == MainView::PublisherProfiles {
                    self.show_publisher_profile_page(ui);
                    return;
                }

                if self.view == MainView::IdentifyRename {
                    self.show_identify_rename_page(ui);
                    return;
                }

                if self.view == MainView::LibraryViewHistory {
                    self.show_library_view_history_page(ui);
                    return;
                }

                if self.view == MainView::MediaSets {
                    if let Some(snapshot) = self.database_state.snapshot() {
                        self.media_sets_page
                            .refresh(&snapshot.archives, self.database_generation.0);
                    }
                    media_sets_page::show_media_sets_page(ui, &mut self.media_sets_page);
                    return;
                }

                if self.view == MainView::CheatsMods {
                    // Manual QA finding: opening Cheats & Mods from
                    // Gamer View's selected-game panel had no obvious way
                    // back - only this page's own "Open Library" button,
                    // deep in its content, which isn't a substitute for a
                    // clear, always-visible "back to games" affordance in
                    // the mode whose entire premise is "no navigation
                    // puzzle." Advanced View is unaffected: it still
                    // reaches this page only through the sidebar and
                    // already has its own established navigation.
                    if self.ui_mode == GuiMode::GamerView
                        && ui.button("\u{2190} Back to games").clicked()
                    {
                        self.view = MainView::Library;
                    }
                    let play_target = if self.cheat_workflow.is_some() {
                        let live_for_play_target = match &self.state {
                            LoadState::Ready(data) => Some(data.as_ref()),
                            LoadState::Loading { previous, .. } => previous.as_deref(),
                            LoadState::Error(_) => None,
                        };
                        match launch_readiness_page::gamer_play_action(
                            &self.build_launch_readiness_input(live_for_play_target),
                        ) {
                            launch_readiness_page::GamerPlayAction::Launch(request) => {
                                Some(request.adapter_name())
                            }
                            launch_readiness_page::GamerPlayAction::BlockedTyped(_) => None,
                        }
                    } else {
                        None
                    };
                    let live = match &self.state {
                        LoadState::Ready(data) => Some(data.as_ref()),
                        LoadState::Loading { previous, .. } => previous.as_deref(),
                        LoadState::Error(_) => None,
                    };
                    let retroarch_route = self.cheat_workflow.as_ref().is_some_and(|workflow| {
                        workflow.adapter == CheatEmulatorAdapter::RetroArch
                    });
                    let dolphin_route = self
                        .cheat_workflow
                        .as_ref()
                        .is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Dolphin);
                    let now_unix_seconds = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |duration| duration.as_secs());
                    let bsfree_context = self.cheat_workflow.as_ref().map(|workflow| {
                        (
                            workflow.archive_path.clone(),
                            workflow.display_name.clone(),
                            workflow.platform.clone().unwrap_or_default(),
                        )
                    });
                    let cheatbase_seed = self.cheat_workflow.as_ref().map(|workflow| {
                        cheatbase_page::CheatBaseGameSeed {
                            title: workflow.display_name.clone(),
                            platform: workflow.platform.clone(),
                            region: workflow.region.clone(),
                        }
                    });
                    let user_cheat_library = live
                        .map(|data| {
                            data.records
                                .iter()
                                .map(|record| archivefs_core::patch_manager::UserCheatLibraryGame {
                                    game_id: record.mount_plan.archive.path.display().to_string(),
                                    title: record
                                        .metadata
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| record.identity.display_name.clone()),
                                    platform: record
                                        .metadata
                                        .platform
                                        .clone()
                                        .or_else(|| record.identity.platform.clone()),
                                    region: record
                                        .metadata
                                        .region
                                        .clone()
                                        .or_else(|| record.identity.region.clone()),
                                    serial: None,
                                    title_id: None,
                                    crc: None,
                                    content_hash: record.identity.content_hash.clone(),
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let user_cheat_selected = self.cheat_workflow.as_ref().map(|workflow| {
                        (
                            workflow.archive_path.display().to_string(),
                            workflow.display_name.clone(),
                        )
                    });
                    let local_cheat_install_context = self
                        .cheat_workflow
                        .as_ref()
                        .filter(|workflow| workflow.adapter == CheatEmulatorAdapter::RetroArch)
                        .map(|workflow| {
                            local_cheat_install_context(workflow, &self.emulator_readiness.retroarch_profiles)
                        });
                    let local_pcsx2_install_context = self
                        .cheat_workflow
                        .as_ref()
                        .filter(|workflow| workflow.adapter == CheatEmulatorAdapter::Pcsx2)
                        .and_then(|workflow| {
                            local_pcsx2_install_context(workflow, &self.emulator_readiness.pcsx2_profiles)
                        });
                    let local_dolphin_install_context = self
                        .cheat_workflow
                        .as_ref()
                        .filter(|workflow| workflow.adapter == CheatEmulatorAdapter::Dolphin)
                        .and_then(|workflow| {
                            local_dolphin_install_context(workflow, &self.emulator_readiness.dolphin_profiles)
                        });
                    let local_xenia_install_context = self
                        .cheat_workflow
                        .as_ref()
                        .filter(|workflow| workflow.adapter == CheatEmulatorAdapter::Xenia)
                        .map(|workflow| local_xenia_install_context(workflow, &self.emulator_readiness.xenia_profiles));
                    let (action, catalogue_action, dolphin_catalogue_action, bsfree_action, cheatbase_action) = ui_layout::page(
                        ui,
                        ui_layout::ContentWidth::Wide,
                        true,
                        "cheats_mods_workspace_scroll",
                        |ui| {
                            self.user_cheat_import_page.show(
                                ui,
                                &ui.ctx().clone(),
                                &user_cheat_library,
                                user_cheat_selected
                                    .as_ref()
                                    .map(|(id, title)| (id.as_str(), title.as_str())),
                                local_cheat_install_context.as_ref(),
                                local_pcsx2_install_context.as_ref(),
                                local_dolphin_install_context.as_ref(),
                                local_xenia_install_context.as_ref(),
                            );
                            ui.add_space(theme::SECTION_GAP);
                            let cheatbase_action = cheatbase_page::show_cheatbase_page(
                                ui,
                                &mut self.cheatbase_page,
                                cheatbase_seed,
                            );
                            ui.add_space(theme::SECTION_GAP);
                            let dolphin_catalogue_action = dolphin_route.then(|| {
                                let action = show_dolphin_catalogue_manager(
                                    ui,
                                    &self.catalogue_bsfree_ui.dolphin_catalogue_manager,
                                    self.catalogue_bsfree_ui.dolphin_catalogue_retrieval.as_ref(),
                                    self.catalogue_bsfree_ui.dolphin_catalogue_last_result.as_ref(),
                                    DolphinCatalogueCardContext {
                                        review: self.catalogue_bsfree_ui.dolphin_catalogue_review,
                                        update_available: self.catalogue_bsfree_ui.dolphin_catalogue_update_available,
                                        remove_confirm: self.catalogue_bsfree_ui.dolphin_catalogue_remove_confirm,
                                        now_unix_seconds,
                                    },
                                    &mut self.clipboard,
                                );
                                ui.add_space(theme::SECTION_GAP);
                                action
                            }).flatten();
                            // Drained before the workspace renders, so a
                            // running install/undo is reflected in this
                            // same frame's render, not one frame late.
                            if self.dolphin_texture_mod.poll() || self.dolphin_texture_mod.is_busy()
                            {
                                ui.ctx().request_repaint();
                            }
                            if self.local_mod_package.poll() || self.local_mod_package.is_busy() {
                                ui.ctx().request_repaint();
                            }
                            if let Some(workflow) = self.cheat_workflow.as_ref() {
                                show_cheat_play_target_warning(
                                    ui,
                                    workflow.adapter,
                                    play_target,
                                );
                                ui.add_space(theme::SECTION_GAP / 2.0);
                            }
                            let action = show_cheats_mods_page(
                                ui,
                                self.cheat_workflow.as_mut(),
                                &self.emulator_readiness.retroarch_profiles,
                                &self.emulator_readiness.pcsx2_profiles,
                                &self.emulator_readiness.dolphin_profiles,
                                &self.emulator_readiness.xenia_profiles,
                                live,
                                self.database_state.snapshot(),
                                &self.history,
                                busy || self.catalogue_bsfree_ui.catalogue_retrieval.is_some(),
                                &mut self.clipboard,
                                &mut self.dolphin_texture_mod,
                                &mut self.local_mod_package,
                            );
                            ui.add_space(theme::SECTION_GAP);
                            self.cheat_reconciliation_review.show(ui);
                            ui.add_space(theme::SECTION_GAP);
                            let bsfree_action = show_bsfree_game_browser(
                                ui,
                                &self.catalogue_bsfree_ui.bsfree_manager,
                                self.catalogue_bsfree_ui.bsfree_operation.is_some(),
                                &mut self.catalogue_bsfree_ui.bsfree_ui,
                                bsfree_context.as_ref(),
                            );
                            let catalogue_action = retroarch_route.then(|| {
                                ui.add_space(theme::SECTION_GAP);
                                widgets::section_header(
                                    ui,
                                    "Database and sources",
                                    Some(
                                        "Download, update, or verify the trusted RetroArch cheat database without leaving this page.",
                                    ),
                                );
                                show_retroarch_catalogue_manager(
                                    ui,
                                    &self.catalogue_bsfree_ui.catalogue_manager,
                                    self.catalogue_bsfree_ui.catalogue_review.as_ref(),
                                    self.catalogue_bsfree_ui.catalogue_retrieval.as_ref(),
                                    self.catalogue_bsfree_ui.catalogue_last_result.as_ref(),
                                    &mut self.clipboard,
                                )
                            }).flatten();
                            (action, catalogue_action, dolphin_catalogue_action, bsfree_action, cheatbase_action)
                        });
                    let picker_rows = live
                        .map(|data| {
                            build_display_rows(
                                &data.records,
                                &data.rows,
                                self.database_state.snapshot(),
                            )
                        })
                        .unwrap_or_default();
                    if let Some(catalogue_action) = catalogue_action {
                        self.handle_catalogue_manager_action(context, catalogue_action);
                    }
                    if let Some(dolphin_catalogue_action) = dolphin_catalogue_action {
                        self.handle_dolphin_catalogue_manager_action(context, dolphin_catalogue_action);
                    }
                    if let Some(bsfree_action) = bsfree_action {
                        self.start_bsfree_operation(context.clone(), bsfree_action);
                    }
                    if let Some(cheatbase_action) = cheatbase_action {
                        self.cheatbase_page.handle(cheatbase_action, context.clone());
                    }
                    match action {
                        Some(CheatWorkflowAction::ChooseArchive) => {
                            self.open_cheat_archive_picker();
                        }
                        Some(CheatWorkflowAction::OpenLibrary) => {
                            self.navigate_to_library_tab(LibraryTab::Archives);
                        }
                        Some(CheatWorkflowAction::RescanProfiles) => {
                            self.start_retroarch_profile_scan(context.clone());
                        }
                        Some(CheatWorkflowAction::RescanPcsx2Profiles) => {
                            self.start_pcsx2_profile_scan(context.clone());
                        }
                        Some(CheatWorkflowAction::InspectPcsx2Profile) => {
                            self.start_pcsx2_inventory(context.clone());
                        }
                        Some(CheatWorkflowAction::FetchPcsx2GameHacking { force_refresh }) => {
                            self.start_pcsx2_gamehacking_fetch(context.clone(), force_refresh);
                        }
                        Some(CheatWorkflowAction::ConfirmPcsx2GameHackingMatch { game_id }) => {
                            self.confirm_pcsx2_gamehacking_match(context.clone(), game_id);
                        }
                        Some(CheatWorkflowAction::TogglePcsx2CheatSelected { id, selected }) => {
                            self.update_pcsx2_cheat_selection(&id, selected);
                        }
                        Some(CheatWorkflowAction::InstallSelectedPcsx2) => {
                            self.start_pcsx2_install_preview();
                        }
                        Some(CheatWorkflowAction::FetchGameCubeGameHacking { force_refresh }) => {
                            self.start_gamecube_gamehacking_fetch(context.clone(), force_refresh);
                        }
                        Some(CheatWorkflowAction::ConfirmGameCubeGameHackingMatch { game_id }) => {
                            self.confirm_gamecube_gamehacking_match(context.clone(), game_id);
                        }
                        Some(CheatWorkflowAction::ToggleGameCubeGameHackingCheatSelected {
                            index,
                            selected,
                        }) => {
                            self.update_gamecube_gamehacking_cheat_selection(index, selected);
                        }
                        Some(CheatWorkflowAction::InstallSelectedGameCubeGameHacking) => {
                            self.start_gamecube_gamehacking_install_preview();
                        }
                        Some(CheatWorkflowAction::RemoveSelectedGameCubeGameHacking) => {
                            self.start_gamecube_gamehacking_removal_preview();
                        }
                        Some(CheatWorkflowAction::OpenBrowserImport(platform)) => {
                            self.open_browser_import(platform);
                        }
                        Some(CheatWorkflowAction::CloseBrowserImport) => {
                            self.close_browser_import();
                        }
                        Some(CheatWorkflowAction::OpenGameHackingPageInBrowser) => {
                            self.open_gamehacking_page_in_browser();
                        }
                        Some(CheatWorkflowAction::CopyGameHackingPageUrl) => {
                            self.copy_gamehacking_page_url();
                        }
                        Some(CheatWorkflowAction::ImportBrowserSavedFile) => {
                            self.import_browser_saved_file(context.clone());
                        }
                        Some(CheatWorkflowAction::ToggleBrowserImportPaste(open)) => {
                            if let Some(state) = self
                                .cheat_workflow
                                .as_mut()
                                .and_then(|workflow| workflow.browser_import.as_mut())
                            {
                                state.paste_open = open;
                            }
                        }
                        Some(CheatWorkflowAction::ImportBrowserPastedText) => {
                            self.import_browser_pasted_text(context.clone());
                        }
                        Some(CheatWorkflowAction::ImportBrowserClipboard) => {
                            self.import_browser_clipboard(context.clone());
                        }
                        Some(CheatWorkflowAction::ChooseBrowserImportKind(kind)) => {
                            if let Some(state) = self
                                .cheat_workflow
                                .as_mut()
                                .and_then(|workflow| workflow.browser_import.as_mut())
                            {
                                state.kind = kind;
                            }
                        }
                        Some(CheatWorkflowAction::FetchBsFreeGameCube { search_title }) => {
                            self.start_bsfree_gamecube_search(context.clone(), search_title);
                        }
                        Some(CheatWorkflowAction::ConfirmBsFreeGameCubeMatch { upstream_uid }) => {
                            self.start_bsfree_gamecube_confirm(context.clone(), upstream_uid);
                        }
                        Some(CheatWorkflowAction::ToggleBsFreeGameCubeCheatSelected {
                            index,
                            selected,
                        }) => {
                            self.update_bsfree_gamecube_cheat_selection(index, selected);
                        }
                        Some(CheatWorkflowAction::SelectAllBsFreeGameCubeCheats) => {
                            self.update_bsfree_gamecube_cheat_selection_all(true);
                        }
                        Some(CheatWorkflowAction::ClearAllBsFreeGameCubeCheats) => {
                            self.update_bsfree_gamecube_cheat_selection_all(false);
                        }
                        Some(CheatWorkflowAction::InstallSelectedBsFreeGameCube) => {
                            self.start_bsfree_gamecube_install_preview();
                        }
                        Some(CheatWorkflowAction::FetchBsFreeWii { search_title }) => {
                            self.start_bsfree_wii_search(context.clone(), search_title);
                        }
                        Some(CheatWorkflowAction::ConfirmBsFreeWiiMatch { upstream_uid }) => {
                            self.start_bsfree_wii_confirm(context.clone(), upstream_uid);
                        }
                        Some(CheatWorkflowAction::ToggleBsFreeWiiCheatSelected {
                            index,
                            selected,
                        }) => {
                            self.update_bsfree_wii_cheat_selection(index, selected);
                        }
                        Some(CheatWorkflowAction::SelectAllBsFreeWiiCheats) => {
                            self.update_bsfree_wii_cheat_selection_all(true);
                        }
                        Some(CheatWorkflowAction::ClearAllBsFreeWiiCheats) => {
                            self.update_bsfree_wii_cheat_selection_all(false);
                        }
                        Some(CheatWorkflowAction::InstallSelectedBsFreeWii) => {
                            self.start_bsfree_wii_install_preview();
                        }
                        Some(CheatWorkflowAction::RescanDolphinProfiles) => {
                            self.start_dolphin_profile_scan(context.clone());
                        }
                        Some(CheatWorkflowAction::InspectDolphinProfile) => {
                            self.start_dolphin_inventory(context.clone());
                        }
                        Some(CheatWorkflowAction::InspectExistingLibrary) => {
                            self.start_existing_retroarch_library_inspection(context.clone());
                        }
                        Some(CheatWorkflowAction::RefreshSources) => {
                            self.start_cheat_source_list(context.clone());
                        }
                        Some(CheatWorkflowAction::ManageCatalogue) => {
                            self.view = MainView::Sources;
                            self.start_catalogue_status_load(context.clone());
                        }
                        Some(CheatWorkflowAction::UseCachedSnapshot) => {
                            self.start_cheat_source_fetch(context.clone(), true);
                        }
                        Some(CheatWorkflowAction::ReviewApply) => {
                            self.review_cheat_apply();
                        }
                        Some(CheatWorkflowAction::ConfirmApply) => {
                            self.start_cheat_apply(context.clone());
                        }
                        Some(CheatWorkflowAction::CancelApply) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.transaction = CheatTransactionState::Idle;
                                workflow.transaction_notice = Some(
                                    "Installation cancelled before apply; no live emulator file was changed."
                                        .to_string(),
                                );
                            }
                            self.history.record(HistoryEntry::new(
                                ActivityAction::CheatInstall,
                                self.cheat_workflow
                                    .as_ref()
                                    .map(|workflow| workflow.archive_path.clone()),
                                ActivityOutcome::Cancelled,
                                "Install cancelled before the write phase; nothing was changed.",
                            ));
                        }
                        Some(CheatWorkflowAction::MatchCandidates) => {
                            self.start_cheat_candidate_match(context.clone());
                        }
                        Some(CheatWorkflowAction::SelectCandidate(relative_path)) => {
                            self.apply_cheat_candidate_choice(&relative_path);
                        }
                        Some(CheatWorkflowAction::ClearCandidateChoice) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.candidate_selection = None;
                                workflow.candidate_load_error = None;
                                workflow.preview = CheatStepResource::NotLoaded;
                                workflow.preview_request = None;
                                workflow.transaction = CheatTransactionState::Idle;
                            }
                        }
                        Some(CheatWorkflowAction::ToggleCheatSelected { index, selected }) => {
                            self.update_cheat_selection(|selection| {
                                selection.set_selected(index, selected);
                            });
                        }
                        Some(CheatWorkflowAction::ToggleCheatEnabled { index, enabled }) => {
                            self.update_cheat_selection(|selection| {
                                selection.set_enabled(index, enabled);
                            });
                        }
                        Some(CheatWorkflowAction::SelectAllCheats) => {
                            self.update_cheat_selection(CheatSelection::select_all);
                        }
                        Some(CheatWorkflowAction::ClearAllCheats) => {
                            self.update_cheat_selection(CheatSelection::clear_all);
                        }
                        Some(CheatWorkflowAction::BuildInstallPreview) => {
                            self.start_generated_cheat_preview(context.clone());
                        }
                        Some(CheatWorkflowAction::RollbackInstall) => {
                            self.start_cheat_install_rollback(context.clone());
                        }
                        Some(CheatWorkflowAction::FetchDolphinProvider { force_refresh }) => {
                            self.start_dolphin_provider_fetch(context.clone(), force_refresh);
                        }
                        Some(CheatWorkflowAction::RescanXeniaProfiles) => {
                            self.start_xenia_profile_scan();
                        }
                        Some(CheatWorkflowAction::FetchXeniaProvider { force_refresh }) => {
                            self.start_xenia_provider_fetch(context.clone(), force_refresh);
                        }
                        Some(CheatWorkflowAction::SelectXeniaCandidate(index)) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.xenia_selected_candidate_index = Some(index);
                                workflow.xenia_selection = None;
                                workflow.xenia_destination_error = None;
                                workflow.preview = CheatStepResource::NotLoaded;
                                workflow.preview_request = None;
                                workflow.transaction = CheatTransactionState::Idle;
                            }
                        }
                        Some(CheatWorkflowAction::ClearXeniaCandidateChoice) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.xenia_selected_candidate_index = None;
                                workflow.xenia_selection = None;
                                workflow.xenia_destination_error = None;
                                workflow.preview = CheatStepResource::NotLoaded;
                                workflow.preview_request = None;
                                workflow.transaction = CheatTransactionState::Idle;
                            }
                        }
                        Some(CheatWorkflowAction::AcknowledgeXeniaPartialVerification(
                            acknowledged,
                        )) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut()
                                && let Some(state) = workflow.xenia_selection.as_mut()
                            {
                                state.selection.partial_verification_acknowledged = acknowledged;
                            }
                        }
                        Some(CheatWorkflowAction::ToggleXeniaPatchSelected {
                            index,
                            selected,
                        }) => {
                            self.update_xenia_patch_selection(|selection| {
                                selection.set_selected(index, selected);
                            });
                        }
                        Some(CheatWorkflowAction::SelectAllXeniaPatches) => {
                            self.update_xenia_patch_selection(XeniaPatchSelection::select_all);
                        }
                        Some(CheatWorkflowAction::ClearAllXeniaPatches) => {
                            self.update_xenia_patch_selection(XeniaPatchSelection::clear_all);
                        }
                        Some(CheatWorkflowAction::BuildXeniaInstallPreview) => {
                            self.start_xenia_install_preview();
                        }
                        Some(CheatWorkflowAction::ToggleDolphinCodeSelected {
                            index,
                            selected,
                        }) => {
                            self.update_dolphin_code_selection(|selection| {
                                selection.set_selected(index, selected);
                            });
                        }
                        Some(CheatWorkflowAction::SelectAllDolphinCodes) => {
                            self.update_dolphin_code_selection(
                                DolphinProviderCodeSelection::select_all,
                            );
                        }
                        Some(CheatWorkflowAction::ClearAllDolphinCodes) => {
                            self.update_dolphin_code_selection(
                                DolphinProviderCodeSelection::clear_all,
                            );
                        }
                        Some(CheatWorkflowAction::BuildDolphinInstallPreview) => {
                            self.start_dolphin_install_preview();
                        }
                        Some(CheatWorkflowAction::ChooseDolphinProfile(profile_id)) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.dolphin_profile_choice = Some(profile_id);
                            }
                            self.confirm_dolphin_profile_choice();
                        }
                        Some(CheatWorkflowAction::ChooseXeniaProfile(profile_id)) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.xenia_profile_choice = Some(profile_id);
                            }
                            self.confirm_xenia_profile_choice();
                        }
                        Some(CheatWorkflowAction::InstallSelectedDolphin) => {
                            self.start_beginner_install_dolphin();
                        }
                        Some(CheatWorkflowAction::InstallSelectedXenia) => {
                            self.start_beginner_install_xenia();
                        }
                        Some(CheatWorkflowAction::ToggleDolphinShowExactChanges(show)) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.dolphin_show_exact_changes = show;
                            }
                        }
                        Some(CheatWorkflowAction::ToggleXeniaShowExactChanges(show)) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.xenia_show_exact_changes = show;
                            }
                        }
                        Some(CheatWorkflowAction::ToggleDolphinDetailsOpen(open)) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.dolphin_details_open = open;
                            }
                        }
                        Some(CheatWorkflowAction::ToggleXeniaDetailsOpen(open)) => {
                            if let Some(workflow) = self.cheat_workflow.as_mut() {
                                workflow.xenia_details_open = open;
                            }
                        }
                        Some(CheatWorkflowAction::OpenApplyHistory) => {
                            self.shared_history_operation = self
                                .cheat_workflow
                                .as_ref()
                                .and_then(|workflow| match &workflow.transaction {
                                    CheatTransactionState::Result { result, .. } => {
                                        Some(result.journal.operation_id.clone())
                                    }
                                    _ => None,
                                });
                            self.shared_history = SharedHistoryState::NotLoaded;
                            self.view = MainView::HistoryLogs;
                        }
                        None => {}
                    }
                    let picker_action = self.cheat_archive_picker.as_mut().and_then(|picker| {
                        show_cheat_archive_picker(
                            context,
                            picker,
                            &picker_rows,
                            &mut self.library_ui.library_filters.platform,
                            &mut self.clipboard,
                        )
                    });
                    match picker_action {
                        Some(CheatArchivePickerAction::Cancel) => {
                            self.cheat_archive_picker = None;
                        }
                        Some(CheatArchivePickerAction::Select(path)) => {
                            let requires_confirmation = cheat_archive_change_requires_confirmation(
                                self.cheat_workflow.as_ref(),
                                &path,
                            );
                            self.cheat_archive_picker = None;
                            if requires_confirmation {
                                self.confirm_cheat_archive_change = Some(path);
                            } else {
                                self.apply_cheat_archive_choice(context, path);
                            }
                        }
                        None => {}
                    }
                    if let Some(path) = self.confirm_cheat_archive_change.clone() {
                        let candidate_still_exists = picker_rows
                            .iter()
                            .any(|row| row.origin == RowOrigin::Live && row.path == path);
                        egui::Window::new("Change Cheats & Mods archive?")
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                            .show(context, |ui| {
                                ui.label("The current archive has catalogue retrieval state. Changing archive discards that page state; it does not alter either archive or RetroArch.");
                                ui.label(path.display().to_string());
                                ui.horizontal(|ui| {
                                    if ui.button("Cancel").clicked() {
                                        self.confirm_cheat_archive_change = None;
                                    }
                                    if ui
                                        .add_enabled(
                                            candidate_still_exists,
                                            egui::Button::new("Change archive"),
                                        )
                                        .clicked()
                                    {
                                        self.apply_cheat_archive_choice(context, path.clone());
                                    }
                                });
                            });
                    }
                    return;
                }

                if self.view == MainView::Mount {
                    let live = match &self.state {
                        LoadState::Ready(data) => Some(data.as_ref()),
                        _ => None,
                    };
                    let action = show_mount_page(
                        ui,
                        live,
                        self.mount_ui.mount_all_result.as_ref(),
                        MountPageViewState {
                            queue: &mut self.mount_ui.mount_queue,
                            search: &mut self.mount_ui.mount_search,
                            platform: &mut self.library_ui.library_filters.platform,
                            confirm: &mut self.mount_ui.confirm_mount_queue,
                            busy: archive_actions_blocked,
                            block_reason: archive_action_block_reason,
                        },
                    );
                    self.handle_mount_page_action(context, action);
                    return;
                }

                if self.view == MainView::Selected {
                    let action = self.show_game_details(
                        context,
                        ui,
                        archive_actions_blocked,
                        archive_action_block_reason,
                    );
                    self.handle_mount_page_action(context, action);
                    return;
                }

                if self.view == MainView::ActiveMounts {
                    let live_records = match &self.state {
                        LoadState::Ready(data) => Some(data.records.as_slice()),
                        _ => None,
                    };
                    let action = show_active_mounts_page(
                        ui,
                        live_records,
                        &mut self.mount_ui.active_mounts_confirm_unmount,
                        &mut self.mount_ui.cleanup_after_unmount,
                        self.feedback.as_ref(),
                        archive_actions_blocked,
                    );
                    match action {
                        Some(ActiveMountsPageAction::Unmount(archive_path)) => {
                            requested_action =
                                Some(AppOperationRequest::Archive(OperationRequest {
                                    action: ArchiveAction::Unmount,
                                    archive_path,
                                    cleanup_after_unmount: self.mount_ui.cleanup_after_unmount,
                                }));
                        }
                        Some(ActiveMountsPageAction::OpenInLibrary(path)) => {
                            self.navigate_to_library_tab(LibraryTab::Archives);
                            self.archive_context.select_only(path);
                        }
                        Some(ActiveMountsPageAction::Refresh) => self.refresh(context),
                        None => {}
                    }
                    ui.add_space(theme::SECTION_GAP);
                    show_active_mounts_recent_activity(ui, &self.history);
                    return;
                }

                // First-class workflow destinations - rendered standalone,
                // never wrapped in Problems & Repair chrome (see 0.8.1's
                // "core workflows directly discoverable" pass). Home, the
                // sidebar and the top menu all route straight here.
                if self.view == MainView::ExactDuplicateReview {
                    widgets::page_header_with_icon(
                        ui,
                        crate::ui::icons::CHECK,
                        "Duplicate Finder",
                        "Find identical or equivalent copies in your library, keep one, and move \
                         the rest into a recoverable quarantine. Nothing is permanently deleted.",
                    );
                    ui.add_space(theme::SECTION_GAP);
                    self.show_exact_duplicate_review_page(ui);
                    return;
                }

                if self.view == MainView::DiscConversion {
                    self.show_optical_conversion_page(ui);
                    return;
                }

                if self.view == MainView::StorageHealth {
                    if let Some(snapshot) = self.database_state.snapshot() {
                        self.storage_health_page.show(ui, &snapshot.archives);
                    } else {
                        ui.heading("Storage Health");
                        ui.label("Storage analysis is not available until the library catalogue is loaded.");
                    }
                    return;
                }

                if self.view == MainView::TapeInspector {
                    let selected_path = self.archive_context.focused.clone();
                    if let Some(path) = selected_path
                        .as_ref()
                        .filter(|path| tape_analysis_page::is_tape_path(path))
                    {
                        let evidence_is_stale = match &self.selected_evidence_ui.selected_evidence {
                            selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                                report.path != *path
                            }
                            selected_evidence_page::SelectedEvidenceState::Loading {
                                path: loading_path, ..
                            } => loading_path != path,
                            selected_evidence_page::SelectedEvidenceState::Idle => true,
                            selected_evidence_page::SelectedEvidenceState::Error {
                                path: error_path, ..
                            } => error_path != path,
                        };
                        if evidence_is_stale {
                            self.start_selected_evidence_load(context.clone(), path.clone());
                        }
                    }
                    let (analysis, analysis_error) = match &self.selected_evidence_ui.selected_evidence {
                        selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                            if Some(report.path.as_path()) == selected_path.as_deref() =>
                        {
                            (report.tape_analysis.as_ref(), None)
                        }
                        selected_evidence_page::SelectedEvidenceState::Error {
                            path: error_path,
                            message,
                            ..
                        } if Some(error_path.as_path()) == selected_path.as_deref() => {
                            (None, Some(message.as_str()))
                        }
                        _ => (None, None),
                    };
                    let live_records = match &self.state {
                        LoadState::Ready(data) => Some(data.records.as_slice()),
                        _ => None,
                    };
                    let tape_action = tape_analysis_page::show_page_with_error(
                        ui,
                        selected_path
                            .as_deref()
                            .filter(|path| tape_analysis_page::is_tape_path(path)),
                        analysis,
                        analysis_error,
                        live_records,
                        &mut self.tape_inspector_filter,
                    );
                    if let Some(
                        tape_analysis_page::TapeInspectorAction::ChooseFile(path)
                        | tape_analysis_page::TapeInspectorAction::SelectLibraryTape(path),
                    ) = tape_action
                    {
                        self.archive_context.select_only(path);
                    }
                    return;
                }

                if self.view == MainView::EmulatorSetup {
                    self.show_emulator_setup_page(ui, context);
                    return;
                }

                if self.view == MainView::ReadyToPlay {
                    let selected_identity = self
                        .archive_context
                        .focused
                        .as_ref()
                        .map(|path| path.display().to_string());
                    let live = match &self.state {
                        LoadState::Ready(data) => Some(data.as_ref()),
                        _ => None,
                    };
                    let results = selected_identity
                        .map(|identity| {
                            let input = self.build_launch_readiness_input(live);
                            launch_readiness_page::ready_to_play_result(&input, identity)
                        })
                        .into_iter()
                        .collect();
                    self.emulator_readiness.ready_to_play_page.set_results(results);
                    self.emulator_readiness.ready_to_play_page.show(ui);
                    return;
                }

                if self.view == MainView::EmulatorInventory {
                    let emulator_running = self.launch_dolphin.is_active()
                        || self.launch_pcsx2.is_active()
                        || self.launch_standalone.is_active();
                    self.emulator_readiness.emulator_inventory_page.show(ui, emulator_running);
                    return;
                }

                if self.view == MainView::BiosProjection {
                    self.emulator_readiness.bios_projection_page.show(ui);
                    return;
                }

                if problems_repair_tab_for_main_view(self.view).is_some() {
                    self.show_problems_repair_page(ui, context);
                    return;
                }

                if self.view == MainView::HistoryLogs {
                    let history_action = show_history_logs_page(
                        ui,
                        &self.shared_history,
                        &mut self.shared_rollback,
                        self.shared_history_operation.as_deref(),
                        &mut self.history,
                        &mut self.history_filters,
                        &mut self.clipboard,
                        &mut self.doctor_repair.database_restore_plan,
                        &mut self.doctor_repair.database_restore_confirmation,
                        &mut self.doctor_repair.database_restore_feedback,
                        self.database_state.is_loading() || busy,
                    );
                    match history_action {
                        Some(HistoryPageAction::PreviewRollback {
                            journal_path,
                            destination_root,
                        }) => self.start_shared_rollback_preview(
                            context.clone(),
                            journal_path,
                            destination_root,
                        ),
                        Some(HistoryPageAction::ConfirmRollback) => {
                            self.start_shared_rollback(context.clone());
                        }
                        Some(HistoryPageAction::CancelRollback) => {
                            self.shared_rollback = SharedRollbackState::Idle;
                        }
                        Some(HistoryPageAction::Refresh) => {
                            self.shared_history = SharedHistoryState::NotLoaded;
                        }
                        Some(HistoryPageAction::ReviewDatabaseRestore { backup_path }) => {
                            match default_database_path()
                                .and_then(|live| archivefs_core::prepare_database_restore(live, backup_path))
                            {
                                Ok(plan) => {
                                    self.doctor_repair.database_restore_plan = Some(plan);
                                    self.doctor_repair.database_restore_confirmation.clear();
                                    self.doctor_repair.database_restore_feedback = None;
                                }
                                Err(error) => self.doctor_repair.database_restore_feedback = Some(error.to_string()),
                            }
                        }
                        Some(HistoryPageAction::ExecuteDatabaseRestore) => {
                            if self.database_state.is_loading() || self.is_busy() {
                                self.doctor_repair.database_restore_feedback = Some("Database is busy loading or scanning; restore remains blocked until it is idle.".into());
                            } else if let Some(plan) = self.doctor_repair.database_restore_plan.clone() {
                                match archivefs_core::restore_database(&plan, &self.doctor_repair.database_restore_confirmation) {
                                    Ok(result) => {
                                        self.doctor_repair.database_restore_feedback = Some(format!("{} Emergency backup: {}", result.receipt.message, result.emergency_backup_path.display()));
                                        self.doctor_repair.database_restore_plan = None;
                                        self.doctor_repair.database_restore_confirmation.clear();
                                        self.database_generation = self.database_generation.next();
                                        let generation = self.database_generation;
                                        let previous = self.database_state.snapshot().cloned().map(Box::new);
                                        self.database_state = start_database_load(context.clone(), generation, previous, false);
                                        self.shared_history = SharedHistoryState::NotLoaded;
                                    }
                                    Err(error) => self.doctor_repair.database_restore_feedback = Some(error.to_string()),
                                }
                            }
                        }
                        None => {}
                    }
                    return;
                }

                if self.view == MainView::Settings {
                    if self.platform_artwork_manager.status.is_none()
                        && self.platform_artwork_manager.task.is_none()
                    {
                        self.start_platform_artwork_task(
                            context.clone(),
                            PlatformArtworkManagerAction::Rescan,
                        );
                    }
                    let mount_root = match &self.state {
                        LoadState::Ready(data) => Some(data.mount_root.as_path()),
                        _ => None,
                    };
                    let action = show_settings_page(
                        ui,
                        &self.database_state,
                        &self.doctor_repair.diagnostics,
                        &self.emulator_readiness.retroarch_profiles,
                        mount_root,
                        busy,
                        &mut self.clipboard,
                        self.custom_platform_artwork_directory.as_deref(),
                        &mut self.platform_artwork_cache,
                        &mut self.platform_artwork_manager,
                    );
                    match action {
                        Some(SettingsPageAction::OpenConfigFolder) => {
                            self.start_setup_action(context.clone(), SetupAction::OpenConfigFolder);
                        }
                        Some(SettingsPageAction::ValidateConfiguration) => {
                            self.refresh_diagnostics(context);
                        }
                        Some(SettingsPageAction::OpenDiagnostics) => {
                            self.tools_overlay = ToolsOverlay::Diagnostics;
                            self.refresh_diagnostics(context);
                        }
                        Some(SettingsPageAction::RescanRetroArchProfiles) => {
                            self.start_retroarch_profile_scan(context.clone());
                        }
                        Some(SettingsPageAction::RunFirstTimeSetupAgain) => {
                            self.restart_onboarding();
                        }
                        Some(SettingsPageAction::PlatformArtwork(action)) => {
                            self.start_platform_artwork_task(context.clone(), action);
                        }
                        None => {}
                    }
                    return;
                }

                if self.view == MainView::About {
                    let mount_root = match &self.state {
                        LoadState::Ready(data) => Some(data.mount_root.as_path()),
                        _ => None,
                    };
                    show_about_contents(
                        ui,
                        &self.database_state,
                        &self.doctor_repair.diagnostics,
                        mount_root,
                        &mut self.clipboard,
                    );
                    return;
                }

                // The unified Library shell: one heading and one tab
                // selector shared by all five Library-related
                // destinations, dispatching to each tab's existing,
                // otherwise-unmodified content. `library_tab_for_main_view`
                // covers the Library-related MainView variants (see its doc
                // comment), so this replaces what
                // used to be separate `if self.view == MainView::X` blocks.
                //
                // The Archives arm deliberately does *not* `return`:
                // falling through to the existing `match &self.state`
                // block below is exactly how MainView::Library already
                // reached it before this shell existed. The other three
                // arms `return` after rendering, exactly as their own
                // standalone `if` blocks used to.
                if library_tab_for_main_view(self.view).is_some() {
                    if let Some(clicked_tab) = show_library_shell_header(ui, self.library_tab) {
                        self.navigate_to_library_tab(clicked_tab);
                    }

                    match self.library_tab {
                        LibraryTab::Archives => {}
                        LibraryTab::Health => {
                            // `cached_health_issues` needs `&mut self` (it
                            // may rebuild and store the cache); `.to_vec()`
                            // copies the small already-built
                            // `Vec<HealthIssue>` out and ends that mutable
                            // borrow immediately, so the immutable borrows
                            // of `self.database_state`/`self.state` just
                            // below (for the much larger
                            // `LoadedData`/`CachedLibrarySnapshot`, passed
                            // by reference rather than cloned) never
                            // conflict with it.
                            let issues = self.cached_health_issues().to_vec();
                            if let Some(snapshot) = self.database_state.snapshot() {
                                let live_data = match &self.state {
                                    LoadState::Ready(data) => Some(data.as_ref()),
                                    _ => None,
                                };
                                health_dashboard_action = show_health_dashboard_panel(
                                    ui,
                                    live_data,
                                    snapshot,
                                    &issues,
                                    HealthDashboardViewState {
                                        filters: &mut self.health_duplicate_ui.health_filters,
                                        sort_field: &mut self.health_duplicate_ui.health_sort_field,
                                        sort_ascending: &mut self.health_duplicate_ui.health_sort_ascending,
                                        selected_issue: &mut self.health_duplicate_ui.selected_health_issue,
                                        busy: archive_actions_blocked,
                                        clipboard: &mut self.clipboard,
                                    },
                                );
                            } else {
                                ui.label("Scan the library to see the health dashboard.");
                            }
                            return;
                        }
                        LibraryTab::Duplicates => {
                            if let Some(snapshot) = self.database_state.snapshot() {
                                match show_duplicate_review_panel(
                                    ui,
                                    &snapshot.duplicate_report,
                                    DuplicateReviewViewState {
                                        filters: &mut self.health_duplicate_ui.duplicate_filters,
                                        sort_field: &mut self.health_duplicate_ui.duplicate_sort_field,
                                        sort_ascending: &mut self.health_duplicate_ui.duplicate_sort_ascending,
                                        selected_group: &mut self.health_duplicate_ui.selected_duplicate_group,
                                        selected_archive: &mut self.health_duplicate_ui.selected_duplicate_archive,
                                        clipboard: &mut self.clipboard,
                                    },
                                ) {
                                    Some(DuplicateReviewAction::Close) => {
                                        self.navigate_to_library_tab(LibraryTab::Archives);
                                    }
                                    Some(DuplicateReviewAction::ViewInLibrary(path)) => {
                                        self.navigate_to_library_tab(LibraryTab::Archives);
                                        self.archive_context.select_only(path);
                                    }
                                    Some(DuplicateReviewAction::Inspect(path)) => {
                                        self.start_archive_inspection(context.clone(), path);
                                    }
                                    None => {}
                                }
                            } else {
                                ui.label("Scan the library to review duplicates.");
                            }
                            return;
                        }
                        LibraryTab::Views => {
                            let all_source_folders = self
                                .database_state
                                .snapshot()
                                .map(|snapshot| snapshot.source_views.as_slice())
                                .unwrap_or(&[]);
                            let library_view_action = show_library_views_page(
                                ui,
                                &self.library_views,
                                all_source_folders,
                                self.library_view_action.is_some(),
                                self.library_view_last_plan.as_ref(),
                                self.library_view_focus_archive.as_deref(),
                                &mut self.library_view_plan_filter,
                                &mut self.library_view_form_dialog,
                                &mut self.library_view_remove_dialog,
                                &mut self.clipboard,
                            );
                            if let Some(library_view_action) = library_view_action {
                                self.start_library_view_action(
                                    context.clone(),
                                    library_view_action,
                                );
                            }
                            return;
                        }
                        LibraryTab::RecentlyFound => {}
                    }
                }

                match &self.state {
                    LoadState::Loading { .. } => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(80.0);
                            ui.spinner();
                            ui.heading("Loading EmuWiz data...");
                            ui.label("Scanning runs in the background.");
                        });
                        // Requirement 1: show cached library rows before the
                        // live snapshot finishes loading, clearly labelled as
                        // last-known state. This is a read-only preview -
                        // built with the same ArchiveRow/show_archive_rows
                        // machinery as the live table, but with no selection
                        // or action wiring at all, so it cannot expose a mount
                        // or unmount button even in principle.
                        if let Some(snapshot) = self.database_state.snapshot() {
                            ui.separator();
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                "Showing last-known catalogue state while the live snapshot loads.",
                            );
                            let preview_rows: Vec<ArchiveRow> = snapshot
                                .archives
                                .iter()
                                .map(|persisted| {
                                    let path_exists = persisted.absolute_path.exists();
                                    ArchiveRow::from_cached(persisted, path_exists)
                                })
                                .collect();
                            let row_height = fixed_row_height(
                                ui.text_style_height(&egui::TextStyle::Body),
                                ui.spacing().interact_size.y,
                            );
                            let horizontal_spacing = ui.spacing().item_spacing.x;
                            let preview_widths = self.library_ui.library_column_widths.as_array();
                            egui::ScrollArea::horizontal()
                                .id_salt("cache_preview_horizontal")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_min_width(table_width(
                                        horizontal_spacing,
                                        &preview_widths,
                                    ));
                                    // This cache-only loading preview has no sort
                                    // state of its own to wire up (it disappears
                                    // the moment the live snapshot loads) - the
                                    // headers render inertly, unsorted. Still
                                    // resizable (shares `self.library_ui.library_column_widths`
                                    // with the real table) so a resize made here
                                    // is not lost once the live snapshot loads.
                                    let _ = show_header_row(
                                        ui,
                                        &COLUMN_HEADERS,
                                        &COLUMN_SORT_FIELDS,
                                        row_height,
                                        None,
                                        true,
                                        &mut self.library_ui.library_column_widths,
                                    );
                                    ui.separator();
                                    let body_height = ui.available_height().max(row_height);
                                    egui::ScrollArea::vertical()
                                        .id_salt("cache_preview_vertical")
                                        .max_height(body_height)
                                        .auto_shrink([false, false])
                                        .show_rows(
                                            ui,
                                            row_height,
                                            preview_rows.len(),
                                            |ui, row_range| {
                                                // Discard the result: this preview never sets
                                                // selected_archive, so it can never drive
                                                // show_selected_archive's action buttons, and
                                                // `records: &[]` means its context menu (if
                                                // right-clicked) offers nothing live either.
                                                let _ = show_archive_rows(
                                                    ui,
                                                    &preview_rows,
                                                    None,
                                                    row_range,
                                                    row_height,
                                                    None,
                                                    &mut HashSet::new(),
                                                    &mut None,
                                                    &preview_widths,
                                                    &RowMenuContext {
                                                        records: &[],
                                                        cached: None,
                                                        busy: true,
                                                        block_reason: None,
                                                        platform_busy: false,
                                                        retroarch_profiles: &self
                                                            .emulator_readiness
                                                            .retroarch_profiles,
                                                        library_views_configured: false,
                                                        library_view_last_plan: None,
                                                    },
                                                );
                                            },
                                        );
                                });
                        }
                    }
                    LoadState::Error(error) => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(80.0);
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                "Could not load EmuWiz",
                            );
                            ui.label(error);
                            ui.add_space(8.0);
                            retry = ui.button("Retry").clicked();
                        });
                    }
                    LoadState::Ready(data) => {
                        let missing_removal_unavailable_reason =
                            self.missing_removal_unavailable_reason();
                        requested_action = show_loaded_data(
                            ui,
                            data,
                            LoadedViewState {
                                filter: &mut self.library_ui.filter,
                                filtered_rows: &mut self.library_ui.filtered_rows,
                                selected_archive: &mut self.archive_context.focused,
                                operation: self.mount_ui.operation.as_ref(),
                                busy: archive_actions_blocked,
                                block_reason: archive_action_block_reason,
                                action_readiness_debug_lines: &action_readiness_debug_lines,
                                feedback: self.feedback.as_ref(),
                                confirm_unmount: &mut self.mount_ui.confirm_unmount,
                                confirm_lazy_unmount: &mut self.mount_ui.confirm_lazy_unmount,
                                confirm_lazy_unmount_final: &mut self.mount_ui.confirm_lazy_unmount_final,
                                confirm_mount_all: &mut self.mount_ui.confirm_mount_all,
                                focus_mount_all_cancel: &mut self.mount_ui.focus_mount_all_cancel,
                                mount_all_typed_count: &mut self.mount_ui.mount_all_typed_count,
                                confirm_unmount_all: &mut self.mount_ui.confirm_unmount_all,
                                focus_unmount_all_cancel: &mut self.mount_ui.focus_unmount_all_cancel,
                                unmount_all_typed_count: &mut self.mount_ui.unmount_all_typed_count,
                                confirm_unmount_selected: &mut self.mount_ui.confirm_unmount_selected,
                                focus_unmount_selected_cancel: &mut self
                                    .mount_ui
                                    .focus_unmount_selected_cancel,
                                confirm_mount_selected: &mut self.mount_ui.confirm_mount_selected,
                                focus_mount_selected_cancel: &mut self.mount_ui.focus_mount_selected_cancel,
                                mount_selected_typed_count: &mut self.mount_ui.mount_selected_typed_count,
                                confirm_bulk_platform_action: &mut self
                                    .confirm_bulk_platform_action,
                                focus_bulk_platform_cancel: &mut self.focus_bulk_platform_cancel,
                                bulk_platform_action_typed_count: &mut self
                                    .bulk_platform_action_typed_count,
                                focus_lazy_cancel: &mut self.mount_ui.focus_lazy_cancel,
                                focus_final_lazy_cancel: &mut self.mount_ui.focus_final_lazy_cancel,
                                lazy_unmount_offers: &self.mount_ui.lazy_unmount_offers,
                                remount_offers: &self.mount_ui.remount_offers,
                                cleanup_after_unmount: &mut self.mount_ui.cleanup_after_unmount,
                                mount_all_result: self.mount_ui.mount_all_result.as_ref(),
                                unmount_all_result: self.mount_ui.unmount_all_result.as_ref(),
                                history: &mut self.history,
                                cached: self.database_state.snapshot(),
                                library_filters: &mut self.library_ui.library_filters,
                                platform_choice: &mut self.library_ui.platform_choice,
                                platform_custom_text: &mut self.library_ui.platform_custom_text,
                                platform_busy: self.library_ui.platform_action.is_some(),
                                retroarch_profiles: &self.emulator_readiness.retroarch_profiles,
                                selected_evidence: &self.selected_evidence_ui.selected_evidence,
                                selected_archives: &mut self.archive_context.selected,
                                bulk_platform_choice: &mut self.library_ui.bulk_platform_choice,
                                bulk_platform_busy: self.library_ui.bulk_platform_action.is_some(),
                                missing_removal_available,
                                missing_removal_unavailable_reason,
                                missing_removal_busy: self.library_ui.missing_removal.is_some(),
                                confirm_remove_missing: &mut self.library_ui.confirm_remove_missing,
                                missing_removal_typed_count: &mut self.missing_removal_typed_count,
                                sort_field: &mut self.library_ui.sort_field,
                                sort_ascending: &mut self.library_ui.sort_ascending,
                                library_scroll_offset: &mut self.library_ui.library_scroll_offset,
                                clipboard: &mut self.clipboard,
                                select_all_visible_requested: &mut self
                                    .select_all_visible_requested,
                                library_source_filter: &mut self.library_ui.library_source_filter,
                                library_column_widths: &mut self.library_ui.library_column_widths,
                                library_views_configured: !self.library_views.is_empty(),
                                library_view_last_plan: self.library_view_last_plan.as_ref(),
                                recent_scan: if self.library_tab == LibraryTab::RecentlyFound {
                                    self.database_state
                                        .snapshot()
                                        .and_then(|snapshot| snapshot.recently_found.as_ref())
                                } else {
                                    None
                                },
                                recent_view: self.library_tab == LibraryTab::RecentlyFound,
                                library_platform_query: &mut self.library_ui.library_platform_query,
                            },
                        );
                        if self.library_tab == LibraryTab::Archives
                            && self.archive_context.focused.is_some()
                        {
                            ui.add_space(crate::ui::theme::SECTION_GAP);
                            let game_details_action = self.show_game_details(
                                context,
                                ui,
                                archive_actions_blocked,
                                archive_action_block_reason,
                            );
                            self.handle_mount_page_action(context, game_details_action);
                        }
                    }
                }
            });
        });
        if stop_mount_all {
            self.request_mount_all_stop();
        }
        if let Some(action) = diagnostics_action {
            match action {
                DiagnosticsUiAction::Refresh => self.refresh_diagnostics(context),
                DiagnosticsUiAction::Continue => {
                    self.tools_overlay = ToolsOverlay::None;
                    self.refresh(context);
                }
                DiagnosticsUiAction::ViewLastSnapshot => {
                    self.tools_overlay = ToolsOverlay::None;
                }
                DiagnosticsUiAction::CreateStarterConfig => {
                    self.start_setup_action(context.clone(), SetupAction::CreateStarterConfig)
                }
                DiagnosticsUiAction::CreateMountRoot => {
                    self.start_setup_action(context.clone(), SetupAction::CreateMountRoot)
                }
                DiagnosticsUiAction::OpenConfigFolder => {
                    self.start_setup_action(context.clone(), SetupAction::OpenConfigFolder)
                }
                DiagnosticsUiAction::CopyConfigPath => {
                    if let DiagnosticsState::Ready { report, .. } = &self.doctor_repair.diagnostics
                        && let Some(path) = &report.config_path
                    {
                        let path = path.display().to_string();
                        let _ = self.clipboard.set_text(path.clone());
                        self.history.record(HistoryEntry::new(
                            ActivityAction::Setup,
                            None,
                            ActivityOutcome::Completed,
                            format!("Copied config path: {path}"),
                        ));
                    }
                }
            }
        }
        if let Some(action) = health_dashboard_action {
            match action {
                HealthDashboardAction::BackToLibrary => {
                    self.navigate_to_library_tab(LibraryTab::Archives);
                }
                HealthDashboardAction::Archive(request) => {
                    requested_action = Some(AppOperationRequest::Archive(request));
                }
                HealthDashboardAction::RefreshDiagnostics => {
                    self.refresh_diagnostics(context);
                }
                HealthDashboardAction::OpenMissingReview => {
                    self.navigate_to_missing_catalogue_review();
                }
                HealthDashboardAction::OpenDuplicateReview => {
                    self.navigate_to_library_tab(LibraryTab::Duplicates);
                }
                HealthDashboardAction::ViewInLibrary(path) => {
                    self.navigate_to_library_tab(LibraryTab::Archives);
                    self.archive_context.select_only(path);
                }
                HealthDashboardAction::Inspect(path) => {
                    requested_action = Some(AppOperationRequest::InspectArchive(path));
                }
                HealthDashboardAction::FilterByCategory(filter) => {
                    self.health_duplicate_ui.health_filters.category = filter;
                }
            }
        }
        if stop_unmount_all {
            self.request_unmount_all_stop();
        }
        if retry {
            self.refresh(context);
        }
        if let Some(request) = requested_action {
            match request {
                AppOperationRequest::Archive(request) => {
                    self.start_operation(
                        context.clone(),
                        request.action,
                        request.archive_path,
                        request.cleanup_after_unmount,
                    );
                }
                AppOperationRequest::MountAll(items) => {
                    self.start_mount_all(context.clone(), items);
                }
                AppOperationRequest::UnmountAll {
                    items,
                    cleanup_after_unmount,
                } => {
                    self.start_unmount_all(context.clone(), items, cleanup_after_unmount);
                }
                AppOperationRequest::PlatformAssignment {
                    archive_path,
                    action,
                } => {
                    self.start_platform_action(context.clone(), archive_path, action);
                }
                AppOperationRequest::BulkPlatformAssignment {
                    archive_paths,
                    kind,
                } => {
                    self.start_bulk_platform_action(context.clone(), archive_paths, kind);
                }
                AppOperationRequest::RemoveMissing(archive_paths) => {
                    self.start_missing_removal(context.clone(), archive_paths);
                }
                AppOperationRequest::UpdateGameFolder => {
                    self.navigate_to_sources_tab(SourcesTab::Libraries);
                }
                AppOperationRequest::FullRescan => {
                    self.start_source_action(context.clone(), SourceAction::ScanAll);
                }
                AppOperationRequest::ReviewMissingGames => {
                    self.navigate_to_missing_catalogue_review();
                }
                AppOperationRequest::InspectArchive(archive_path) => {
                    self.start_archive_inspection(context.clone(), archive_path);
                }
                AppOperationRequest::ShowInLibraryViews(archive_path) => {
                    self.navigate_to_library_tab(LibraryTab::Views);
                    self.library_view_focus_archive = Some(archive_path);
                }
                AppOperationRequest::OpenCheatsMods(archive_path) => {
                    self.archive_context.select_only(archive_path.clone());
                    self.open_cheats_mods_workspace(context, archive_path);
                }
                AppOperationRequest::OpenDatSources => {
                    self.quick_rename_mode = false;
                    self.view = MainView::DatSources;
                }
            }
        }
    }
}

impl eframe::App for ArchiveFsApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.update(ui.ctx(), frame);
    }
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

/// Whether a currently-missing config should be framed as an ordinary
/// first run rather than a possible problem: only when this session has
/// never once seen the config file present and readable. Kept as its own
/// pure predicate (mirroring `library_table_message`/
/// `gamer_empty_list_guidance`) so the distinction is directly testable
/// without an `egui::Ui`.
fn missing_config_is_first_run(config_previously_confirmed: bool) -> bool {
    !config_previously_confirmed
}

fn show_setup_diagnostics(
    ui: &mut egui::Ui,
    state: &DiagnosticsState,
    action_running: bool,
    feedback: Option<&ActionFeedback>,
    refresh_error: Option<&str>,
    has_last_snapshot: bool,
    config_previously_confirmed: bool,
) -> Option<DiagnosticsUiAction> {
    let mut action = None;
    ui.heading("Setup / Diagnostics");
    ui.label("Check configuration, folders, and required system tools before using EmuWiz.");
    ui.add_space(8.0);
    if let Some(error) = refresh_error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Archive refresh failed: {error}"),
        );
        ui.label("Diagnostics are being refreshed from the current configuration.");
        if has_last_snapshot
            && ui
                .add_enabled(
                    !action_running,
                    egui::Button::new("View Last Known Snapshot"),
                )
                .clicked()
        {
            return Some(DiagnosticsUiAction::ViewLastSnapshot);
        }
        ui.add_space(8.0);
    }
    if let DiagnosticsState::Error { message, .. } = state {
        ui.colored_label(ui.visuals().error_fg_color, message);
        ui.label("Select Refresh Diagnostics to try again.");
        if ui
            .add_enabled(!action_running, egui::Button::new("Refresh Diagnostics"))
            .clicked()
        {
            return Some(DiagnosticsUiAction::Refresh);
        }
        return None;
    }
    let DiagnosticsState::Ready { report, .. } = state else {
        ui.spinner();
        ui.label("Running diagnostics in the background...");
        ui.add_enabled(false, egui::Button::new("Continue to EmuWiz"));
        return None;
    };
    if report.config_missing && report.config_path_error.is_none() {
        if missing_config_is_first_run(config_previously_confirmed) {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong("Welcome to EmuWiz");
                ui.label(
                    "EmuWiz is not configured yet - that is expected on a fresh install, not \
                     an error. Select Create Starter Config below to begin, then add a source \
                     folder on the Sources page.",
                );
                ui.label(
                    "DAT Sources and Cheat Sources live on their own pages and start empty; \
                     both are optional. RomM is optional too. Audits are always read-only: \
                     EmuWiz will not rename, move or delete any ROM without a later, \
                     explicit, reviewed action.",
                );
            });
        } else {
            // Not a first run: this session already saw the config file
            // present and readable, and it is now gone. That can be an
            // intentional removal, but it can just as easily mean a real
            // problem (deleted by accident, an unmounted drive, a bug) -
            // so this is never dressed up as an ordinary welcome.
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.colored_label(theme::WARNING, "Configuration file is no longer found");
                ui.label(
                    "EmuWiz found your configuration earlier in this session, and it is no \
                     longer present. If you did not remove it intentionally, check whether it \
                     was deleted, moved, or is on a drive that is no longer mounted, before \
                     creating a new one below.",
                );
            });
        }
        ui.add_space(8.0);
    }
    ui.horizontal_wrapped(|ui| {
        ui.strong("Config path:");
        match &report.config_path {
            Some(path) => {
                ui.monospace(path.display().to_string());
                if ui
                    .add_enabled(!action_running, egui::Button::new("Copy Config Path"))
                    .clicked()
                {
                    action = Some(DiagnosticsUiAction::CopyConfigPath);
                }
                if ui
                    .add_enabled(!action_running, egui::Button::new("Open Config Folder"))
                    .clicked()
                {
                    action = Some(DiagnosticsUiAction::OpenConfigFolder);
                }
            }
            None => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    report
                        .config_path_error
                        .as_deref()
                        .unwrap_or("Config path could not be resolved."),
                );
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        if starter_config_available(report)
            && ui
                .add_enabled(!action_running, egui::Button::new("Create Starter Config"))
                .clicked()
        {
            action = Some(DiagnosticsUiAction::CreateStarterConfig);
        }
        if report.can_create_mount_root
            && ui
                .add_enabled(!action_running, egui::Button::new("Create Mount Root"))
                .clicked()
        {
            action = Some(DiagnosticsUiAction::CreateMountRoot);
        }
        if ui
            .add_enabled(!action_running, egui::Button::new("Refresh Diagnostics"))
            .clicked()
        {
            action = Some(DiagnosticsUiAction::Refresh);
        }
        if ui
            .add_enabled(
                !action_running && diagnostics_state_can_continue(state),
                egui::Button::new("Continue to EmuWiz"),
            )
            .clicked()
        {
            action = Some(DiagnosticsUiAction::Continue);
        }
        if action_running {
            ui.spinner();
            ui.label("Setup action running...");
        }
    });
    if let Some(feedback) = feedback {
        ui.colored_label(
            if feedback.succeeded {
                egui::Color32::from_rgb(70, 170, 90)
            } else {
                ui.visuals().error_fg_color
            },
            &feedback.message,
        );
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("setup_diagnostics_checks")
        .show(ui, |ui| {
            for check in &report.checks {
                let (state_label, color) = match check.status {
                    SetupDiagnosticStatus::Ready => ("Ready", egui::Color32::from_rgb(70, 170, 90)),
                    // Neither a pass nor a problem: the check did not run.
                    SetupDiagnosticStatus::NotChecked => ("Not checked", theme::muted(ui)),
                    // Expected on a fresh install - informational, not red.
                    SetupDiagnosticStatus::NotConfigured => ("Not configured", theme::muted(ui)),
                    SetupDiagnosticStatus::Warning => {
                        ("Warning", egui::Color32::from_rgb(220, 170, 40))
                    }
                    SetupDiagnosticStatus::Error => ("Error", ui.visuals().error_fg_color),
                };
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(color, state_label);
                        ui.strong(&check.name);
                    });
                    ui.label(&check.detail);
                    if check.status != SetupDiagnosticStatus::Ready {
                        ui.label(format!("Why it matters: {}", check.why_it_matters));
                        ui.label(format!("Next step: {}", check.next_step));
                    }
                });
                ui.add_space(4.0);
            }
        });
    action
}
enum ActivityPanelAction {
    ShowRelatedArchive(PathBuf),
}

fn activity_outcome_tone(outcome: ActivityOutcome) -> widgets::StatusTone {
    match outcome {
        ActivityOutcome::Completed => widgets::StatusTone::Success,
        ActivityOutcome::Failed | ActivityOutcome::Rejected => widgets::StatusTone::Blocked,
        ActivityOutcome::OfflineUsable => widgets::StatusTone::Info,
        ActivityOutcome::Started | ActivityOutcome::Retried | ActivityOutcome::Confirmed => {
            widgets::StatusTone::Active
        }
        ActivityOutcome::Offered | ActivityOutcome::Skipped | ActivityOutcome::Cancelled => {
            widgets::StatusTone::Pending
        }
    }
}

fn activity_summary_entry(history: &OperationHistory) -> Option<&HistoryEntry> {
    history
        .entries()
        .find(|entry| {
            matches!(
                entry.outcome,
                ActivityOutcome::Failed | ActivityOutcome::Rejected
            )
        })
        .or_else(|| history.entries().next())
}

fn show_activity_panel(
    context: &egui::Context,
    history: &mut OperationHistory,
    expanded: &mut bool,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<ActivityPanelAction> {
    let mut action = None;
    // Root cause of the bottom-clipping bug: `TopBottomPanel::bottom` picks
    // this frame's panel height by loading `PanelState` persisted under
    // its *own id* from the previous frame (egui's `panel.rs`), and only
    // falls back to a fresh default the very first time that id is ever
    // shown. Collapsed and expanded here render wildly different content
    // heights (one status row vs. a history list up to ~220px tall plus a
    // button row), but previously both used the *same* id ("activity") -
    // so the frame right after toggling from collapsed to expanded loaded
    // the collapsed height, squeezed the expanded content into it (that
    // content's own clip rect is the panel rect: see egui's `panel.rs`,
    // "If we overflow, don't do so visibly"), and only corrected itself
    // one frame later. A user's screenshot taken in that window - or
    // rendered while the app is between reactive repaints - shows exactly
    // "one line of content" jammed near the screen edge. Giving each
    // visual state its own id keeps their persisted heights from ever
    // contaminating each other, so there is no longer a wrong state to
    // render even transiently.
    let (panel_id, default_height) = if *expanded {
        ("activity_expanded", ACTIVITY_PANEL_EXPANDED_DEFAULT_HEIGHT)
    } else {
        (
            "activity_collapsed",
            ACTIVITY_PANEL_COLLAPSED_DEFAULT_HEIGHT,
        )
    };
    let maximum_height = if *expanded {
        (context.input(|input| input.screen_rect().height()) * 0.28)
            .clamp(120.0, ACTIVITY_PANEL_EXPANDED_DEFAULT_HEIGHT)
    } else {
        ACTIVITY_PANEL_COLLAPSED_DEFAULT_HEIGHT
    };
    egui::TopBottomPanel::bottom(panel_id)
        .resizable(*expanded)
        .default_height(default_height)
        .height_range(ACTIVITY_PANEL_COLLAPSED_DEFAULT_HEIGHT..=maximum_height)
        .show(context, |ui| {
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    if *expanded {
                        "Hide activity"
                    } else {
                        "Show activity"
                    },
                    widgets::ActionStyle::Quiet,
                    true,
                )
                .clicked()
                {
                    *expanded = !*expanded;
                }
                widgets::status_badge(
                    ui,
                    format!("{} events", history.len()),
                    widgets::StatusTone::Info,
                );
                if !*expanded && let Some(entry) = activity_summary_entry(history) {
                    widgets::status_badge(
                        ui,
                        entry.outcome.to_string(),
                        activity_outcome_tone(entry.outcome),
                    );
                    ui.add(egui::Label::new(&entry.message).truncate())
                        .on_hover_text(&entry.message);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if *expanded
                        && widgets::action_button(
                            ui,
                            "Clear activity history",
                            widgets::ActionStyle::Destructive,
                            history.entries().next().is_some(),
                        )
                        .clicked()
                    {
                        history.clear();
                    }
                });
            });
            if !*expanded {
                return;
            }
            ui.separator();

            if history.entries().next().is_none() {
                ui.weak("No recent activity.");
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("activity_history")
                .max_height(220.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Collected as owned data *before* the loop, rather
                    // than iterating `history.entries()` directly, so a
                    // menu item can freely call `history.clear()`/
                    // `history.remove()` without fighting the borrow
                    // checker over a `history` still being iterated.
                    let rows: Vec<(
                        usize,
                        ActivityAction,
                        ActivityOutcome,
                        String,
                        Option<PathBuf>,
                    )> = history
                        .entries()
                        .enumerate()
                        .map(|(index, entry)| {
                            (
                                index,
                                entry.action,
                                entry.outcome,
                                entry.message.clone(),
                                entry.archive_path.clone(),
                            )
                        })
                        .collect();
                    let mut remove_index = None;
                    for (index, activity, outcome, text, archive_path) in &rows {
                        let response = widgets::card(ui, |ui| {
                            widgets::activity_row_header(
                                ui,
                                outcome.to_string(),
                                activity_outcome_tone(*outcome),
                                activity.to_string(),
                                None,
                                |_ui| {},
                            );
                            ui.add(
                                egui::Label::new(text)
                                    .selectable(true)
                                    .wrap()
                                    .sense(egui::Sense::click()),
                            )
                        });
                        ui.add_space(6.0);
                        response.context_menu(|ui| {
                            if ui.button("Copy message").clicked() {
                                let _ = clipboard.set_text(text.clone());
                                ui.close();
                            }
                            if let Some(archive_path) = archive_path {
                                if ui.button("Copy related path").clicked() {
                                    let _ = clipboard.set_text(archive_path.display().to_string());
                                    ui.close();
                                }
                                if ui.button("Show related archive").clicked() {
                                    action = Some(ActivityPanelAction::ShowRelatedArchive(
                                        archive_path.clone(),
                                    ));
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if ui.button("Remove this entry").clicked() {
                                remove_index = Some(*index);
                                ui.close();
                            }
                            if ui.button("Clear activity history").clicked() {
                                history.clear();
                                ui.close();
                            }
                        });
                    }
                    // Deferred to after the loop: removing mid-iteration
                    // would shift every later index out from under `rows`.
                    if let Some(index) = remove_index {
                        history.remove(index);
                    }
                });
        });
    action
}

fn doctor_summary_text(report: &DoctorReport) -> String {
    let passed = report
        .checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Pass)
        .count();
    let warnings = report
        .checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Warn)
        .count();
    let failed = report
        .checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Fail)
        .count();
    format!(
        "{} checks: {passed} passed, {warnings} warnings, {failed} failed · \
         {} archives ({} mounted, {} pending, {} unknown platform)",
        report.checks.len(),
        report.archives_found,
        report.mounted_archives,
        report.pending_archives,
        report.archives_unknown_platform
    )
}

/// The full-report text behind the Doctor page's "Copy Report" - the
/// design's copy-full-details affordance. Plain text, one check per
/// line, so it pastes cleanly into an issue or support request. The
/// design's per-check "Suggested fix" is deliberately absent:
/// `DoctorCheck` carries no suggestion field today, and inventing fixes
/// in the GUI would be untruthful (see the capability matrix's Doctor
/// "Suggested action" row).
fn doctor_report_text(report: &DoctorReport) -> String {
    let mut lines = vec![
        format!("EmuWiz doctor report"),
        format!("Config: {}", report.config_path.display()),
        doctor_summary_text(report),
        String::new(),
    ];
    for check in &report.checks {
        lines.push(format!(
            "[{}] {} — {}",
            check.status, check.name, check.detail
        ));
    }
    if !report.platform_counts.is_empty() {
        lines.push(String::new());
        lines.push("Platform counts:".to_string());
        for (platform, count) in &report.platform_counts {
            lines.push(format!("  {platform}: {count}"));
        }
    }
    lines.join("\n")
}

fn show_doctor_checks_panel(ui: &mut egui::Ui, doctor: Option<&DoctorReport>) {
    let Some(doctor) = doctor else {
        widgets::empty_state(
            ui,
            "Health checks unavailable",
            "Scan the library before running the full EmuWiz health report.",
            None,
        );
        return;
    };
    for status in [DoctorStatus::Fail, DoctorStatus::Warn] {
        for check in doctor.checks.iter().filter(|check| check.status == status) {
            widgets::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(
                        ui,
                        check.status.to_string(),
                        if status == DoctorStatus::Fail {
                            widgets::StatusTone::Blocked
                        } else {
                            widgets::StatusTone::Warning
                        },
                    );
                    ui.label(egui::RichText::new(&check.name).strong());
                });
                ui.add(egui::Label::new(&check.detail).wrap());
            });
            ui.add_space(6.0);
        }
    }
    let passed = doctor
        .checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Pass)
        .count();
    egui::CollapsingHeader::new(format!("Passed checks ({passed})"))
        .default_open(false)
        .show(ui, |ui| {
            for check in doctor
                .checks
                .iter()
                .filter(|check| check.status == DoctorStatus::Pass)
            {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(ui, "Pass", widgets::StatusTone::Success);
                    ui.strong(&check.name);
                    ui.label(egui::RichText::new(&check.detail).color(theme::muted(ui)));
                });
            }
        });
}

const INSPECTOR_DETAILS_COLUMN_WIDTH: f32 = 300.0;

/// Whether one entry matches the Archive Inspector's current search text
/// (case-insensitive substring against its exact stored name) and
/// classification filter - pure, so it is directly testable without
/// rendering anything, mirroring `health_issue_matches`'s existing
/// convention in this file.
fn inspector_entry_matches(
    entry: &InspectorEntry,
    search_lower: &str,
    classification_filter: Option<InspectorEntryClassification>,
) -> bool {
    if let Some(filter) = classification_filter
        && entry.classification != filter
    {
        return false;
    }
    search_lower.is_empty() || entry.name.to_lowercase().contains(search_lower)
}

/// Filters and sorts the already-inspected entry list without ever
/// mutating it - `entries` is only ever read here, exactly like
/// `visible_health_issue_indices` reads its own `issues` slice.
fn visible_inspector_entry_indices(
    entries: &[InspectorEntry],
    search: &str,
    classification_filter: Option<InspectorEntryClassification>,
    sort_field: InspectorSortField,
    sort_ascending: bool,
) -> Vec<usize> {
    let search_lower = search.trim().to_lowercase();
    let mut indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            inspector_entry_matches(entry, &search_lower, classification_filter).then_some(index)
        })
        .collect();
    indices.sort_by(|&left, &right| {
        let (left_entry, right_entry) = (&entries[left], &entries[right]);
        let ordering = match sort_field {
            InspectorSortField::Path => left_entry.name.cmp(&right_entry.name),
            InspectorSortField::Size => left_entry
                .uncompressed_size
                .cmp(&right_entry.uncompressed_size),
            InspectorSortField::Classification => {
                left_entry.classification.cmp(&right_entry.classification)
            }
        }
        .then_with(|| left_entry.name.cmp(&right_entry.name));
        if sort_ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    indices
}
fn inspector_entry_details_text(entry: &InspectorEntry) -> String {
    match entry.kind {
        InspectorEntryKind::Directory => "Directory".to_string(),
        InspectorEntryKind::File => format!(
            "{} \u{2014} {} \u{2014} compressed {} \u{2014} {}",
            entry.classification.label(),
            format_size(Some(entry.uncompressed_size)),
            format_size(entry.compressed_size),
            entry.compression_method.as_deref().unwrap_or("Unknown"),
        ),
    }
}

/// Renders one Archive Inspector entry row - the same technique
/// `show_data_row` uses for the Library table (a single `Sense::click()`
/// region with `Painter`-painted cell text, never a child widget inside
/// that region), generalised to two columns via the same now-slice-based
/// `cell_index_at`/`hovered_cell_full_text` helpers the Library table
/// itself uses. Selection here is single (`selected: bool`, no
/// multi-select/Ctrl-click) - "Selecting one entry shows its complete
/// details" never needed the Library table's fuller multi-select model.
fn show_inspector_row(
    ui: &mut egui::Ui,
    entry: &InspectorEntry,
    row_height: f32,
    selected: bool,
    widths: &[f32],
) -> egui::Response {
    let spacing = ui.spacing().item_spacing.x;
    let width = widths.iter().sum::<f32>() + spacing * (widths.len().saturating_sub(1) as f32);
    let (_, rect) = ui.allocate_space(egui::vec2(width, row_height));
    let row_id = egui::Id::new("inspector_row").with(&entry.name);
    let mut response = ui.interact(rect, row_id, egui::Sense::click());

    let visuals = ui.visuals();
    if selected {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.selection.bg_fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill);
    }

    let details_text = inspector_entry_details_text(entry);
    let cells: [&str; 2] = [entry.name.as_str(), details_text.as_str()];
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().text_color();
    let mut x = rect.left();
    for (text, column_width) in cells.iter().zip(widths.iter().copied()) {
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(x, rect.top()),
            egui::vec2(column_width, row_height),
        );
        ui.painter().with_clip_rect(cell_rect).text(
            egui::pos2(x + 2.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            *text,
            font_id.clone(),
            color,
        );
        x += column_width + spacing;
    }

    let pointer_x = response.hover_pos().map(|pos| pos.x);
    if let Some(full_text) =
        hovered_cell_full_text(pointer_x, rect.left(), &cells, widths, spacing, |text| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_string(), font_id.clone(), color)
                    .size()
                    .x
            })
        })
    {
        response = response.on_hover_text(full_text.to_string());
    }

    response
}
fn show_archive_inspector_panel(
    ui: &mut egui::Ui,
    state: &mut ArchiveInspectorState,
    clipboard: &mut dyn ClipboardBackend,
) -> bool {
    let close = widgets::show_tools_overlay_header(ui, "Archive Inspector");
    ui.horizontal(|ui| {
        ui.label("Archive:");
        let path_text = state.archive_path.display().to_string();
        ui.add(egui::Label::new(&path_text).selectable(true).wrap());
        if ui.small_button("Copy").clicked() {
            let _ = clipboard.set_text(path_text.clone());
        }
    });
    ui.add_space(4.0);

    match &state.status {
        ArchiveInspectorStatus::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Inspecting archive - this runs in the background.");
            });
            return close;
        }
        ArchiveInspectorStatus::Error(message) => {
            ui.colored_label(ui.visuals().error_fg_color, message);
            return close;
        }
        ArchiveInspectorStatus::Ready(_) => {}
    }
    let ArchiveInspectorStatus::Ready(report) = &state.status else {
        unreachable!("every other status already returned above");
    };

    if report.truncated {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Showing the first {} of {} entries - this view is incomplete. Use a more \
                 specific search to find a particular entry.",
                report.entries.len(),
                report.total_entries_in_archive
            ),
        );
        ui.add_space(2.0);
    }

    ui.horizontal_wrapped(|ui| {
        for classification in InspectorEntryClassification::ALL {
            let count = report
                .entries
                .iter()
                .filter(|entry| entry.classification == classification)
                .count();
            summary_value(ui, classification.label(), count);
        }
    });
    ui.separator();

    ui.horizontal_wrapped(|ui| {
        ui.label("Search path:");
        show_text_edit_with_context_menu(ui, &mut state.search, clipboard, |text_edit| {
            text_edit
                .id_salt("archivefs_inspector_search")
                .desired_width(260.0)
        });
        ui.label("Classification:");
        egui::ComboBox::from_id_salt("inspector_classification_filter")
            .selected_text(
                state
                    .classification_filter
                    .map(InspectorEntryClassification::label)
                    .unwrap_or("All"),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.classification_filter, None, "All");
                for classification in InspectorEntryClassification::ALL {
                    ui.selectable_value(
                        &mut state.classification_filter,
                        Some(classification),
                        classification.label(),
                    );
                }
            });
        ui.label("Sort by:");
        egui::ComboBox::from_id_salt("inspector_sort_field")
            .selected_text(state.sort_field.to_string())
            .show_ui(ui, |ui| {
                for field in [
                    InspectorSortField::Path,
                    InspectorSortField::Size,
                    InspectorSortField::Classification,
                ] {
                    ui.selectable_value(&mut state.sort_field, field, field.to_string());
                }
            });
        ui.checkbox(&mut state.sort_ascending, "Ascending");
    });

    let visible = visible_inspector_entry_indices(
        &report.entries,
        &state.search,
        state.classification_filter,
        state.sort_field,
        state.sort_ascending,
    );
    ui.horizontal_wrapped(|ui| {
        summary_value(ui, "Entries shown", visible.len());
        summary_value(ui, "Total entries", report.entries.len());
    });
    ui.separator();

    if report.entries.is_empty() {
        ui.label("This archive has no entries.");
        return close;
    }
    if visible.is_empty() {
        ui.label("No entries match the current search/filter.");
        return close;
    }

    let row_height = ui
        .text_style_height(&egui::TextStyle::Body)
        .max(ui.spacing().interact_size.y);
    let spacing = ui.spacing().item_spacing.x;

    ui.strong("Entries");
    egui::ScrollArea::horizontal()
        .id_salt("inspector_entries_horizontal")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let start_of_frame_width = state.path_column_width;
            ui.set_min_width(start_of_frame_width + spacing + INSPECTOR_DETAILS_COLUMN_WIDTH);

            let mut path_header_rect = None;
            ui.horizontal(|ui| {
                let response = ui.add_sized(
                    [start_of_frame_width, row_height],
                    egui::Label::new(egui::RichText::new("Path").strong()),
                );
                path_header_rect = Some(response.rect);
                ui.add_sized(
                    [INSPECTOR_DETAILS_COLUMN_WIDTH, row_height],
                    egui::Label::new(egui::RichText::new("Details").strong()),
                );
            });
            if let Some(path_header_rect) = path_header_rect {
                let handle_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        path_header_rect.right() - COLUMN_RESIZE_HANDLE_WIDTH,
                        path_header_rect.top(),
                    ),
                    egui::vec2(COLUMN_RESIZE_HANDLE_WIDTH, path_header_rect.height()),
                );
                show_column_resize_handle(
                    ui,
                    egui::Id::new("inspector_path_column_resize"),
                    handle_rect,
                    &mut state.path_column_width,
                );
            }
            ui.separator();

            // Re-read after the resize handle, which may have just
            // changed `state.path_column_width` this very frame - the
            // rows below must always paint with *this* frame's width,
            // never a one-frame-stale copy (matches the Library table's
            // identical fix in `show_loaded_data`).
            let widths = [state.path_column_width, INSPECTOR_DETAILS_COLUMN_WIDTH];

            let body_height = ui.available_height().max(row_height);
            egui::ScrollArea::vertical()
                .id_salt("inspector_entries_vertical")
                .max_height(body_height)
                .auto_shrink([false, false])
                .show_rows(ui, row_height, visible.len(), |ui, row_range| {
                    for visible_index in row_range {
                        let entry_index = visible[visible_index];
                        let entry = &report.entries[entry_index];
                        let selected = state.selected_entry.as_deref() == Some(entry.name.as_str());
                        let response = show_inspector_row(ui, entry, row_height, selected, &widths);
                        if response.clicked() {
                            state.selected_entry = Some(entry.name.clone());
                        }
                    }
                });
        });

    let Some(selected_name) = state.selected_entry.clone() else {
        ui.label("Select an entry to view its details.");
        return close;
    };
    let Some(selected_entry) = report
        .entries
        .iter()
        .find(|entry| entry.name == selected_name)
    else {
        return close;
    };

    ui.separator();
    ui.strong("Selected entry");
    egui::Grid::new("inspector_selected_entry_details")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            detail_row_with_copy(ui, "Path", &selected_entry.name, clipboard);
            detail_row(
                ui,
                "Type",
                match selected_entry.kind {
                    InspectorEntryKind::File => "File",
                    InspectorEntryKind::Directory => "Directory",
                },
            );
            detail_row(ui, "Classification", selected_entry.classification.label());
            detail_row(
                ui,
                "Uncompressed size",
                &format_size(Some(selected_entry.uncompressed_size)),
            );
            detail_row(
                ui,
                "Compressed size",
                &format_size(selected_entry.compressed_size),
            );
            detail_row(
                ui,
                "Compression method",
                selected_entry
                    .compression_method
                    .as_deref()
                    .unwrap_or("Unknown"),
            );
        });

    close
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

/// Drops queued paths whose archive no longer exists in the live
/// snapshot (source removed, rescan, etc.). Deliberately keeps queued
/// archives that are merely no longer `Pending` (mounted meanwhile, or a
/// destination collision) - those stay visible on the Mount page with
/// their skip reason instead of vanishing silently.
fn prune_mount_queue(queue: &mut Vec<PathBuf>, records: &[ArchiveRecord]) {
    queue.retain(|path| {
        records
            .iter()
            .any(|record| record.mount_plan.archive.path == *path)
    });
}

/// The queued paths that a "Mount queue" run will actually attempt - in
/// queue order, `Pending` archives only, mirroring
/// `show_bulk_row_context_menu`'s contract that
/// `mount_all_items_for_paths` is only ever fed genuinely eligible
/// archives.
fn queued_pending_paths(queue: &[PathBuf], records: &[ArchiveRecord]) -> Vec<PathBuf> {
    queue
        .iter()
        .filter(|path| {
            records.iter().any(|record| {
                record.mount_plan.archive.path == **path
                    && record.mount_state == MountState::Pending
                    && record.is_mount_input()
            })
        })
        .cloned()
        .collect()
}

/// Case-insensitive substring match over the fields the Mount page
/// displays (name, platform, archive path, planned destination) - the
/// Mount page's counterpart of the Library's `search_text` matching.
fn mount_row_matches(record: &ArchiveRecord, filter: &str) -> bool {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    format!(
        "{} {} {} {}",
        record.identity.display_name,
        record.identity.platform.as_deref().unwrap_or(""),
        record.mount_plan.archive.path.display(),
        record.mount_plan.mount_path.display()
    )
    .to_lowercase()
    .contains(&needle)
}

/// What the Mount page asks `update` to do - executing a mount goes
/// through the app's own `start_mount_all` (the proven batch engine),
/// never directly from render code.
enum MountPageAction {
    MountQueue,
    Refresh,
    /// Navigate to the Mount page (the Selected page's plain "Open Mounts"
    /// shortcut into the real mount-queue workflow).
    GoToMount,
    /// Open the first-class Cheats & Mods workspace for this exact
    /// archive (the Selected page's entry point).
    OpenCheatsMods(PathBuf),
    /// Start the shared background RetroArch profile scan from the
    /// Selected page's entry section.
    ScanRetroArchProfiles,
}

/// What the user chose in the shared mount-queue confirmation strip.
enum QueueConfirmChoice {
    Mount,
    Cancel,
}

/// The inline "mount the queue" confirmation strip, used by the Mount
/// page's own queue review (`show_mount_page`). The Selected page no longer
/// renders any mount queue at all, so this is not shared with it anymore.
fn show_mount_queue_confirmation(
    ui: &mut egui::Ui,
    attempted: usize,
    busy: bool,
) -> Option<QueueConfirmChoice> {
    let mut choice = None;
    widgets::card(ui, |ui| {
        widgets::status_badge(ui, "Confirmation", widgets::StatusTone::Warning);
        if attempted == 1 {
            ui.strong("Mount 1 queued archive?");
        } else {
            ui.strong(format!("Mount {attempted} queued archives?"));
        }
        ui.label(
            "Only archives that are ready to mount are attempted; already-mounted \
             archives and existing destinations are skipped by the batch engine.",
        );
        ui.horizontal(|ui| {
            if widgets::action_button(
                ui,
                "Mount now",
                widgets::ActionStyle::Primary,
                !busy && attempted > 0,
            )
            .clicked()
            {
                choice = Some(QueueConfirmChoice::Mount);
            }
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                choice = Some(QueueConfirmChoice::Cancel);
            }
        });
    });
    choice
}

/// The Selected page's own view state - a game-details/review surface, not a
/// mount-queue screen. Queue review, manipulation, and the mount-queue
/// confirmation flow live only on the Mount page (`show_mount_page`,
/// `administration_pages.rs`), which already owns that machinery end to end;
/// this page never re-implements or duplicates it, and offers only a plain
/// "Open Mounts" shortcut into the real workflow.
struct SelectedPageViewState<'a> {
    selected_archive: Option<&'a Path>,
    selected_count: usize,
    retroarch_profiles: &'a RetroArchProfilesState,
    busy: bool,
    block_reason: Option<&'a str>,
}

/// Renders the Selected page: a focused game-details/review surface (Cheats
/// & Mods entry point here; identity evidence, launch readiness, identity
/// sources, plan preview, and RPCS3/PCSX2 panels rendered by the caller
/// immediately after this returns). Never renders or manipulates the mount
/// queue - see `SelectedPageViewState`'s own doc.
fn show_selected_page(
    ui: &mut egui::Ui,
    live: Option<&LoadedData>,
    view_state: SelectedPageViewState<'_>,
) -> Option<MountPageAction> {
    let SelectedPageViewState {
        selected_archive,
        selected_count,
        retroarch_profiles,
        busy,
        block_reason,
    } = view_state;
    let mut action = None;
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::SELECTED,
        "Game Details",
        "Review identity, launch readiness and available actions for this game.",
    );

    widgets::section_header(
        ui,
        "Cheats & Mods",
        Some("Open the dedicated workspace for the archive selected in Library."),
    );
    match selected_archive {
        Some(path) => {
            if widgets::path_value(ui, "Selected archive", path) {
                ui.ctx().copy_text(path.display().to_string());
            }
        }
        None => {
            ui.label("No archive is selected in the Library.");
        }
    }
    let entry_blocker = cheat_entry_blocker(
        selected_archive,
        selected_count,
        live.map(|data| data.records.as_slice()),
        retroarch_profiles,
    );
    ui.horizontal(|ui| {
        if widgets::action_button(
            ui,
            "Open Cheats & Mods",
            widgets::ActionStyle::Secondary,
            entry_blocker.is_none() && !busy,
        )
        .clicked()
            && let Some(path) = selected_archive
        {
            action = Some(MountPageAction::OpenCheatsMods(path.to_path_buf()));
        }
        // The only mount-related affordance this page offers: a plain
        // shortcut into the real Mount page/workflow. No queue is reviewed,
        // built, or confirmed here.
        if widgets::action_button(ui, "Open Mounts", widgets::ActionStyle::Quiet, !busy).clicked() {
            action = Some(MountPageAction::GoToMount);
        }
        if matches!(
            retroarch_profiles,
            RetroArchProfilesState::NotScanned | RetroArchProfilesState::Error(_)
        ) && widgets::action_button(
            ui,
            "Scan for RetroArch profiles",
            widgets::ActionStyle::Quiet,
            !busy,
        )
        .clicked()
        {
            action = Some(MountPageAction::ScanRetroArchProfiles);
        }
    });
    if let Some(reason) = entry_blocker {
        widgets::banner(ui, "Unavailable", reason, widgets::StatusTone::Pending);
    }
    if busy && let Some(reason) = block_reason {
        ui.label(reason);
    }
    action
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

enum PlatformArtworkManagerAction {
    Rescan,
    Import {
        platform_id: String,
        source: PathBuf,
    },
    PreviewFolder(PathBuf),
    ApplyFolder,
    Remove(String),
    OpenFolder,
}

fn current_artwork_source(
    root: Option<&Path>,
    platform_id: &str,
    status: Option<&archivefs_core::platform_artwork::PlatformArtworkStatus>,
) -> (&'static str, bool) {
    let is_valid = |path: &Path| {
        !status.is_some_and(|status| {
            status
                .invalid_custom_files
                .iter()
                .any(|invalid| invalid.path == path)
        })
    };
    let asset_id = canonical_platform_asset_id(platform_id);
    if custom_platform_artwork_path(root, &asset_id).is_some_and(|path| is_valid(&path)) {
        return ("Custom", true);
    }
    if bundled_platform_artwork(&asset_id).is_some() {
        return ("Bundled", false);
    }
    let category = platform_asset_category(platform_id);
    if category != PlatformAssetCategory::Unknown
        && custom_platform_artwork_path(root, category.asset_id())
            .is_some_and(|path| is_valid(&path))
    {
        return ("Category fallback (custom)", false);
    }
    if category == PlatformAssetCategory::Unknown {
        ("Unknown fallback", false)
    } else {
        ("Category fallback", false)
    }
}

fn show_platform_artwork_manager(
    ui: &mut egui::Ui,
    root: Option<&Path>,
    artwork_cache: &mut PlatformArtworkCache,
    manager: &mut PlatformArtworkManagerState,
    action: &mut Option<SettingsPageAction>,
) {
    let running = manager.task.is_some();
    // Drain a finished background file dialog: pick_file() ran on a worker
    // thread so the frame was never blocked. When it returns, process exactly
    // what the inline call used to.
    if let Some(pick) = manager.pending_pick.as_mut() {
        match drain_file_pick(&pick.receiver) {
            FilePickDrain::Pending => {
                // Picker still open: keep `pending_pick` so no second dialog
                // starts and the frame keeps draining.
            }
            FilePickDrain::Cancelled => {
                // User pressed Cancel: nothing changed, no error, buttons
                // become available again.
                manager.pending_pick = None;
            }
            FilePickDrain::Disconnected => {
                // Thread ended without a result: release the picker and tell
                // the user, so the buttons are usable again.
                manager.pending_pick = None;
                manager.message = Some((false, FILE_PICKER_DISCONNECTED_MESSAGE.to_string()));
            }
            FilePickDrain::Picked(source) => {
                let FilePickRequest {
                    platform_id,
                    custom,
                    ..
                } = manager.pending_pick.take().expect("just drained");
                if custom {
                    manager.pending_import = Some((platform_id, source));
                } else {
                    manager.replace_existing = false;
                    *action = Some(SettingsPageAction::PlatformArtwork(
                        PlatformArtworkManagerAction::Import {
                            platform_id,
                            source,
                        },
                    ));
                }
            }
        }
    }
    widgets::card(ui, |ui| {
        ui.label(format!(
            "Managed folder: {}",
            root.map_or_else(
                || "Unavailable".to_owned(),
                |path| path.display().to_string()
            )
        ));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new("Open artwork folder"))
                .clicked()
            {
                *action = Some(SettingsPageAction::PlatformArtwork(
                    PlatformArtworkManagerAction::OpenFolder,
                ));
            }
            if ui
                .add_enabled(!running, egui::Button::new("Rescan custom artwork"))
                .clicked()
            {
                *action = Some(SettingsPageAction::PlatformArtwork(
                    PlatformArtworkManagerAction::Rescan,
                ));
            }
            if ui
                .add_enabled(!running, egui::Button::new("Preview folder import"))
                .clicked()
                && let Some(folder) = rfd::FileDialog::new().pick_folder()
            {
                *action = Some(SettingsPageAction::PlatformArtwork(
                    PlatformArtworkManagerAction::PreviewFolder(folder),
                ));
            }
            if running {
                ui.spinner();
                ui.label("Validating artwork…");
            }
        });
        if let Some((succeeded, message)) = &manager.message {
            widgets::banner(
                ui,
                if *succeeded {
                    "Artwork updated"
                } else {
                    "Artwork error"
                },
                message,
                if *succeeded {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Blocked
                },
            );
        }
        if let Some(status) = &manager.status {
            ui.label(format!(
                "{} canonical platforms · {} custom · {} bundled · {} fallback-only · {} invalid · {} unknown · {} bytes",
                status.total_canonical_platforms,
                status.custom_images,
                status.bundled_images,
                status.fallback_only_platforms,
                status.invalid_custom_files.len(),
                status.unknown_files.len(),
                status.total_custom_disk_bytes
            ));
            if !status.invalid_custom_files.is_empty() || !status.unknown_files.is_empty() {
                ui.collapsing("Invalid and unknown files", |ui| {
                    for invalid in &status.invalid_custom_files {
                        ui.label(format!(
                            "Invalid: {} — {}",
                            invalid.path.display(),
                            invalid.reason
                        ));
                    }
                    for unknown in &status.unknown_files {
                        ui.label(format!("Unknown: {}", unknown.display()));
                    }
                    ui.weak("Rescan never deletes these files.");
                });
            }
        }
        if let Some(preview) = &manager.bulk_preview {
            let recognised = preview
                .entries
                .iter()
                .filter(|entry| {
                    entry.disposition
                        == archivefs_core::platform_artwork::BulkArtworkDisposition::Recognised
                })
                .count();
            let unknown = preview
                .entries
                .iter()
                .filter(|entry| {
                    entry.disposition
                        == archivefs_core::platform_artwork::BulkArtworkDisposition::UnknownFilename
                })
                .count();
            let invalid = preview
                .entries
                .iter()
                .filter(|entry| {
                    entry.disposition
                        == archivefs_core::platform_artwork::BulkArtworkDisposition::Invalid
                })
                .count();
            let duplicates = preview
                .entries
                .iter()
                .filter(|entry| {
                    entry.disposition
                        == archivefs_core::platform_artwork::BulkArtworkDisposition::DuplicateTarget
                })
                .count();
            ui.separator();
            ui.label(format!("Folder preview: {recognised} recognised · {unknown} unknown · {invalid} invalid · {duplicates} duplicate target(s)."));
            for entry in preview.entries.iter().take(10) {
                ui.label(format!(
                    "{:?}: {} — {}",
                    entry.disposition,
                    entry.source.display(),
                    entry.detail
                ));
            }
            if preview.entries.len() > 10 {
                ui.collapsing(
                    format!("Show all {} reviewed files", preview.entries.len()),
                    |ui| {
                        for entry in &preview.entries {
                            ui.label(format!(
                                "{:?}: {}",
                                entry.disposition,
                                entry.source.display()
                            ));
                        }
                    },
                );
            }
            ui.checkbox(
                &mut manager.replace_existing,
                "Replace existing custom artwork after confirmation",
            );
            if ui
                .add_enabled(
                    !running && recognised > 0,
                    egui::Button::new("Import recognised images"),
                )
                .clicked()
            {
                *action = Some(SettingsPageAction::PlatformArtwork(
                    PlatformArtworkManagerAction::ApplyFolder,
                ));
            }
        }
    });

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("Search platforms:");
        ui.text_edit_singleline(&mut manager.search);
        egui::ComboBox::from_id_salt("platform_artwork_filter")
            .selected_text(match manager.filter {
                ArtworkManagerFilter::All => "All platforms",
                ArtworkManagerFilter::Missing => "Missing artwork",
                ArtworkManagerFilter::Custom => "Custom artwork",
                ArtworkManagerFilter::FallbackOnly => "Fallback only",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut manager.filter,
                    ArtworkManagerFilter::All,
                    "All platforms",
                );
                ui.selectable_value(
                    &mut manager.filter,
                    ArtworkManagerFilter::Missing,
                    "Missing artwork",
                );
                ui.selectable_value(
                    &mut manager.filter,
                    ArtworkManagerFilter::Custom,
                    "Custom artwork",
                );
                ui.selectable_value(
                    &mut manager.filter,
                    ArtworkManagerFilter::FallbackOnly,
                    "Fallback only",
                );
            });
    });

    let search = manager.search.trim().to_ascii_lowercase();
    for platform in archivefs_core::platform::PLATFORMS {
        let (source_label, custom) =
            current_artwork_source(root, platform.id, manager.status.as_ref());
        let fallback = source_label.contains("fallback");
        let missing = bundled_platform_artwork(&canonical_platform_asset_id(platform.id)).is_none()
            && !custom;
        if !search.is_empty()
            && !platform.display_name.to_ascii_lowercase().contains(&search)
            && !platform.id.to_ascii_lowercase().contains(&search)
        {
            continue;
        }
        if !match manager.filter {
            ArtworkManagerFilter::All => true,
            ArtworkManagerFilter::Missing => missing,
            ArtworkManagerFilter::Custom => custom,
            ArtworkManagerFilter::FallbackOnly => fallback,
        } {
            continue;
        }
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                let (response, _) =
                    ui.allocate_painter(egui::vec2(72.0, 72.0), egui::Sense::hover());
                let asset_id = canonical_platform_asset_id(platform.id);
                paint_platform_artwork_at(
                    ui,
                    artwork_cache,
                    root,
                    PlatformArtworkPaint {
                        center: response.rect.center(),
                        size: 64.0,
                        color: ui.visuals().text_color().gamma_multiply(0.8),
                        asset_id: &asset_id,
                        fallback_asset_id: platform_asset_category(platform.id).asset_id(),
                    },
                );
                ui.vertical(|ui| {
                    ui.heading(platform.display_name);
                    ui.label(format!("Canonical ID: {}", platform.id));
                    ui.label(format!("Current source: {source_label}"));
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                !running && manager.pending_pick.is_none(),
                                egui::Button::new("Choose image"),
                            )
                            .clicked()
                        {
                            // Run the native dialog on a background thread: a
                            // blocking `pick_file()` on the egui thread freezes
                            // the UI until the dialog closes.
                            let (sender, receiver) = mpsc::channel();
                            std::thread::spawn(move || {
                                let picked = rfd::FileDialog::new()
                                    .add_filter("Static image", &["png", "jpg", "jpeg", "webp"])
                                    .pick_file();
                                let _ = sender.send(picked);
                            });
                            manager.pending_pick = Some(FilePickRequest {
                                platform_id: platform.id.to_owned(),
                                custom,
                                receiver,
                            });
                        }
                        if let Some((pending_platform, _)) = &manager.pending_import
                            && pending_platform == platform.id
                        {
                            ui.label("Replace the existing custom image?");
                            if ui
                                .add_enabled(!running, egui::Button::new("Confirm replacement"))
                                .clicked()
                                && let Some((platform_id, source)) = manager.pending_import.take()
                            {
                                manager.replace_existing = true;
                                *action = Some(SettingsPageAction::PlatformArtwork(
                                    PlatformArtworkManagerAction::Import {
                                        platform_id,
                                        source,
                                    },
                                ));
                            }
                            if ui.button("Cancel replacement").clicked() {
                                manager.pending_import = None;
                            }
                        }
                        if custom
                            && manager.pending_remove.as_deref() != Some(platform.id)
                            && ui
                                .add_enabled(!running, egui::Button::new("Remove custom image"))
                                .clicked()
                        {
                            manager.pending_remove = Some(platform.id.to_owned());
                        }
                        if manager.pending_remove.as_deref() == Some(platform.id) {
                            ui.label("Remove EmuWiz's custom copy?");
                            if ui
                                .add_enabled(!running, egui::Button::new("Confirm restore default"))
                                .clicked()
                            {
                                manager.pending_remove = None;
                                *action = Some(SettingsPageAction::PlatformArtwork(
                                    PlatformArtworkManagerAction::Remove(platform.id.to_owned()),
                                ));
                            }
                            if ui.button("Cancel").clicked() {
                                manager.pending_remove = None;
                            }
                        }
                    });
                });
            });
        });
        ui.add_space(4.0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrowDirection {
    Up,
    Down,
}

/// The persisted database row backing the selected archive, if the
/// library database knows about it - live or cache-only alike, unlike
/// `selected_record` (live only). This is what makes manual platform
/// assignment available for a cache-only/missing row: it is metadata
/// only, never a mount action, so it does not need `selected_record`'s
/// live-only restriction. Matches by exact path bytes (`PersistedArchive::absolute_path`),
/// never a lossy display string.
fn selected_persisted_archive<'a>(
    cached: Option<&'a CachedLibrarySnapshot>,
    selected_archive: Option<&Path>,
) -> Option<&'a PersistedArchive> {
    let selected_archive = selected_archive?;
    cached?
        .archives
        .iter()
        .find(|persisted| persisted.absolute_path == selected_archive)
}

fn selected_platform_details<'a>(
    cached: Option<&'a CachedLibrarySnapshot>,
    persisted: Option<&PersistedArchive>,
) -> Option<&'a PlatformProvenanceDetails> {
    cached?.platform_details.get(&persisted?.id)
}

fn available_action(mount_state: MountState) -> ArchiveAction {
    match mount_state {
        MountState::Mounted => ArchiveAction::Unmount,
        MountState::Pending | MountState::MountPathExists | MountState::NotMountable => {
            ArchiveAction::Mount
        }
    }
}

fn individual_actions_available(busy: bool) -> bool {
    !busy
}

fn confirmation_actions_available(busy: bool) -> bool {
    individual_actions_available(busy)
}

fn record_recovery_activity(
    history: &mut OperationHistory,
    action: ActivityAction,
    archive_path: &Path,
    outcome: ActivityOutcome,
    message: &'static str,
) {
    history.record(HistoryEntry::new(
        action,
        Some(archive_path.to_path_buf()),
        outcome,
        message,
    ));
}

fn advance_to_final_lazy_confirmation(
    warning_confirmation: &mut Option<PathBuf>,
    final_confirmation: &mut Option<PathBuf>,
    focus_final_cancel: &mut bool,
    archive_path: &Path,
) {
    *final_confirmation = Some(archive_path.to_path_buf());
    *warning_confirmation = None;
    *focus_final_cancel = true;
}

fn lazy_confirmation_available(
    confirmed_archive: &Path,
    offered_archives: &HashSet<PathBuf>,
    busy: bool,
) -> bool {
    !busy && offered_archives.contains(confirmed_archive)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipboardTextStatus {
    /// Usable, non-empty text is available to paste.
    Ready(String),
    /// The clipboard was reachable and read successfully, but has no
    /// text (empty, or holds a non-text format such as an image).
    Empty,
    /// The clipboard backend could not be reached at all, or the read
    /// itself failed for a reason other than "no text".
    Unavailable(String),
}

/// Clipboard access, isolated behind one small trait so the production
/// path (the real OS clipboard) and tests (a deterministic in-memory
/// stand-in) share every byte of the actual Cut/Copy/Paste logic above
/// it. Neither egui nor eframe exposes a way to read or write the
/// clipboard *synchronously*, at the moment a context-menu item is
/// clicked - `ViewportCommand::RequestPaste`/`RequestCopy`/`RequestCut`
/// are the only public hooks, and they are asynchronous round-trips
/// through eframe's platform backend that (per live testing) cannot be
/// relied on to land back on the field the user actually clicked. Direct
/// clipboard access was explicitly permitted for exactly this case.
pub(crate) trait ClipboardBackend {
    fn get_text_status(&mut self) -> ClipboardTextStatus;
    /// `Err` carries a short, safe error summary - never the text that
    /// failed to be written.
    fn set_text(&mut self, text: String) -> Result<(), String>;
}
fn clipboard_environment_summary() -> String {
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unset".to_string());
    let display = if std::env::var_os("DISPLAY").is_some() {
        "present"
    } else {
        "absent"
    };
    let wayland_display = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "present"
    } else {
        "absent"
    };
    format!("XDG_SESSION_TYPE={session_type} DISPLAY={display} WAYLAND_DISPLAY={wayland_display}")
}

/// The real OS clipboard via `arboard` - the same crate eframe's own
/// "clipboard" feature already links in transitively (see
/// `egui-winit`'s `Clipboard`, which every keyboard Ctrl+X/C/V in this
/// app already goes through); this only opens a second, independent
/// handle to the identical native mechanism; it does not add a
/// competing implementation. `archivefs-gui`'s own `Cargo.toml` enables
/// arboard's `wayland-data-control` feature explicitly - without it,
/// arboard's Linux backend is unconditionally X11 regardless of session
/// type (confirmed by reading arboard's own source), which is exactly
/// why Paste failed to see text copied from a native-Wayland Firefox on
/// the real Nobara session this was live-tested against.
///
/// Constructed once and kept for the app's entire lifetime (see
/// `ArchiveFsApp::clipboard`), never per-click: on X11, the application
/// that last copied something must keep its clipboard connection alive
/// to serve paste requests from other apps, so recreating and dropping a
/// connection on every click risks silently discarding what was just
/// copied.
///
/// `inner` is `None` if the platform clipboard could not be reached at
/// all (headless environment, no display server, etc.) - `init_error`
/// then holds *why*, so a broken clipboard is never silently reported as
/// merely empty. Every operation safely reports `Unavailable` instead of
/// panicking. Diagnostics (the environment summary, whether init
/// succeeded, and the exact init error if it failed) are printed to
/// stderr exactly once, at construction - never on every frame, and
/// never including clipboard content.
struct NativeClipboard {
    inner: Option<arboard::Clipboard>,
    init_error: Option<String>,
    /// The last `Unavailable` reason actually printed to stderr, so a
    /// repeated identical failure (e.g. the context menu re-checking
    /// Paste's enabled state on every frame it stays open) is logged
    /// once, not every frame - while a *new* or *changed* failure is
    /// still always visible.
    last_logged_error: Option<String>,
}

impl NativeClipboard {
    fn new() -> Self {
        eprintln!(
            "archivefs-gui: clipboard environment: {}",
            clipboard_environment_summary()
        );
        match arboard::Clipboard::new() {
            Ok(clipboard) => {
                eprintln!("archivefs-gui: clipboard backend initialised");
                Self {
                    inner: Some(clipboard),
                    init_error: None,
                    last_logged_error: None,
                }
            }
            Err(error) => {
                let message = error.to_string();
                eprintln!("archivefs-gui: clipboard backend initialisation failed: {message}");
                Self {
                    inner: None,
                    init_error: Some(message),
                    last_logged_error: None,
                }
            }
        }
    }

    /// Logs `message` to stderr, but only the first time (or the first
    /// time it changes) - see `last_logged_error`'s doc comment.
    fn log_error_once(&mut self, message: &str) {
        if self.last_logged_error.as_deref() != Some(message) {
            eprintln!("archivefs-gui: clipboard error: {message}");
            self.last_logged_error = Some(message.to_string());
        }
    }
}

impl ClipboardBackend for NativeClipboard {
    fn get_text_status(&mut self) -> ClipboardTextStatus {
        let Some(clipboard) = self.inner.as_mut() else {
            let message = self
                .init_error
                .clone()
                .unwrap_or_else(|| "clipboard backend not initialised".to_string());
            self.log_error_once(&message);
            return ClipboardTextStatus::Unavailable(message);
        };
        match clipboard.get_text() {
            Ok(text) if !text.is_empty() => ClipboardTextStatus::Ready(text),
            Ok(_) => ClipboardTextStatus::Empty,
            // `ContentNotAvailable` is arboard's own way of reporting "the
            // clipboard is empty or holds a non-text format" - a normal,
            // expected outcome, not a failure worth logging.
            Err(arboard::Error::ContentNotAvailable) => ClipboardTextStatus::Empty,
            Err(error) => {
                let message = error.to_string();
                self.log_error_once(&message);
                ClipboardTextStatus::Unavailable(message)
            }
        }
    }

    fn set_text(&mut self, text: String) -> Result<(), String> {
        let Some(clipboard) = self.inner.as_mut() else {
            let message = self
                .init_error
                .clone()
                .unwrap_or_else(|| "clipboard backend not initialised".to_string());
            self.log_error_once(&message);
            return Err(message);
        };
        clipboard.set_text(text).map_err(|error| {
            let message = error.to_string();
            self.log_error_once(&message);
            message
        })
    }
}

/// One item in the shared text-field context menu - see
/// `show_text_edit_with_context_menu`. Deliberately just these four:
/// exactly what a normal desktop text field's right-click menu offers,
/// nothing more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextEditContextMenuAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl TextEditContextMenuAction {
    const ALL: [Self; 4] = [Self::Cut, Self::Copy, Self::Paste, Self::SelectAll];

    fn label(self) -> &'static str {
        match self {
            Self::Cut => "Cut",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select all",
        }
    }
}
fn text_edit_context_menu_action_available(
    action: TextEditContextMenuAction,
    has_selection: bool,
    is_empty: bool,
    has_clipboard_text: bool,
) -> bool {
    match action {
        TextEditContextMenuAction::Cut | TextEditContextMenuAction::Copy => has_selection,
        TextEditContextMenuAction::Paste => has_clipboard_text,
        TextEditContextMenuAction::SelectAll => !is_empty,
    }
}
fn text_edit_selected_char_range(ctx: &egui::Context, id: egui::Id) -> Option<Range<usize>> {
    let range = egui::widgets::text_edit::TextEditState::load(ctx, id)?
        .cursor
        .char_range()?;
    (!range.is_empty()).then(|| range.as_sorted_char_range())
}

/// The field's current cursor position as a character range - a
/// selection if one exists, otherwise an empty range at the caret. Unlike
/// `text_edit_selected_char_range`, this always returns *something*:
/// Paste needs an insertion point even with no selection, falling back to
/// the end of `text` if the field has no persisted cursor state at all
/// (never out of bounds, and the only reasonable default with zero other
/// information).
fn text_edit_cursor_char_range(ctx: &egui::Context, id: egui::Id, text: &str) -> Range<usize> {
    egui::widgets::text_edit::TextEditState::load(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.as_sorted_char_range())
        .unwrap_or_else(|| {
            let end = text.chars().count();
            end..end
        })
}

/// Moves the field's cursor to a single position (no selection) and gives
/// it keyboard focus, so the edit this always follows is visible
/// immediately and further typing continues from the right place.
fn set_text_edit_caret(ctx: &egui::Context, id: egui::Id, char_index: usize) {
    let mut state = egui::widgets::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(char_index),
        )));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
}
fn apply_select_all(ctx: &egui::Context, id: egui::Id, text: &str) {
    let mut state = egui::widgets::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let full_range = egui::text::CCursorRange::two(
        egui::text::CCursor::new(0),
        egui::text::CCursor::new(text.chars().count()),
    );
    state.cursor.set_char_range(Some(full_range));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
}

/// Copy - writes exactly the selected substring (char-index sliced via
/// `TextBuffer::char_range`, so a selection ending mid-multi-byte
/// character is impossible) to the clipboard. A no-op if nothing is
/// selected; never clears or overwrites the clipboard in that case. A
/// clipboard write failure is safely discarded here (already logged by
/// `ClipboardBackend::set_text`); there is no field mutation to roll
/// back for Copy.
fn apply_copy(ctx: &egui::Context, id: egui::Id, text: &str, clipboard: &mut dyn ClipboardBackend) {
    if let Some(range) = text_edit_selected_char_range(ctx, id) {
        let _ = clipboard.set_text(text.char_range(range).to_string());
    }
}

/// Cut - copies exactly the selected substring, then removes exactly
/// that same range from `text` (via `TextBuffer::delete_char_range`, the
/// identical method `TextEdit`'s own Ctrl+X handling uses), leaving the
/// caret where the removed text started. A no-op if nothing is selected.
/// If the clipboard write fails, `text` is left completely untouched -
/// removing text the user could not actually cut anywhere would be a
/// silent data loss, not a safe degradation.
fn apply_cut(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    clipboard: &mut dyn ClipboardBackend,
) {
    let Some(range) = text_edit_selected_char_range(ctx, id) else {
        return;
    };
    if clipboard
        .set_text(text.char_range(range.clone()).to_string())
        .is_err()
    {
        return;
    }
    text.delete_char_range(range.clone());
    set_text_edit_caret(ctx, id, range.start);
}

/// Paste - inserts the clipboard's text at the caret (no selection), or
/// replaces exactly the selected range (a selection, partial or the
/// entire field) via `TextBuffer::insert_text`/`delete_char_range`, the
/// same char-index-safe methods `TextEdit`'s own Ctrl+V handling uses. A
/// no-op if the clipboard is empty *or* unavailable - both cases already
/// disable the Paste menu item (see `text_edit_context_menu_action_available`),
/// but this defends against being called anyway (e.g. a stale click).
fn apply_paste(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    clipboard: &mut dyn ClipboardBackend,
) {
    let clip_text = match clipboard.get_text_status() {
        ClipboardTextStatus::Ready(text) => text,
        ClipboardTextStatus::Empty | ClipboardTextStatus::Unavailable(_) => return,
    };
    let range = text_edit_cursor_char_range(ctx, id, text);
    if !range.is_empty() {
        text.delete_char_range(range.clone());
    }
    let inserted = text.insert_text(&clip_text, range.start);
    set_text_edit_caret(ctx, id, range.start + inserted);
}
fn clipboard_status_label(status: &ClipboardTextStatus) -> String {
    match status {
        ClipboardTextStatus::Ready(_) => "Clipboard ready".to_string(),
        ClipboardTextStatus::Empty => "Clipboard contains no text".to_string(),
        ClipboardTextStatus::Unavailable(reason) => format!("Clipboard unavailable: {reason}"),
    }
}

fn show_text_edit_with_context_menu(
    ui: &mut egui::Ui,
    text: &mut String,
    clipboard: &mut dyn ClipboardBackend,
    configure: impl FnOnce(egui::TextEdit<'_>) -> egui::TextEdit<'_>,
) -> egui::Response {
    let is_empty = text.is_empty();
    let text_edit = configure(egui::TextEdit::singleline(text));
    let output = text_edit.show(ui);
    let response = output.response.response;
    let id = response.id;
    let has_selection = output.cursor_range.is_some_and(|range| !range.is_empty());

    // Right-clicking to open this field's context menu also gives it
    // keyboard focus, exactly like a real desktop text field - so the
    // field visibly looks active even before any menu item is clicked.
    // This is cosmetic only now: every action below reaches the correct
    // field via its `id`, captured once above, regardless of whether
    // focus is still there by the time the user actually clicks.
    if response.secondary_clicked() {
        ui.memory_mut(|memory| memory.request_focus(id));
    }

    response.context_menu(|ui| {
        // One read per menu-open frame, shared by the status line and
        // Paste's enabled state - never a second clipboard read.
        let clipboard_status = clipboard.get_text_status();
        ui.small(clipboard_status_label(&clipboard_status));
        ui.separator();
        let has_clipboard_text = matches!(clipboard_status, ClipboardTextStatus::Ready(_));
        for action in TextEditContextMenuAction::ALL {
            let enabled = text_edit_context_menu_action_available(
                action,
                has_selection,
                is_empty,
                has_clipboard_text,
            );
            if ui
                .add_enabled(enabled, egui::Button::new(action.label()))
                .clicked()
            {
                match action {
                    TextEditContextMenuAction::Cut => apply_cut(ui.ctx(), id, text, clipboard),
                    TextEditContextMenuAction::Copy => apply_copy(ui.ctx(), id, text, clipboard),
                    TextEditContextMenuAction::Paste => apply_paste(ui.ctx(), id, text, clipboard),
                    TextEditContextMenuAction::SelectAll => apply_select_all(ui.ctx(), id, text),
                }
                ui.close();
            }
        }
    });

    response
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

fn matching_row_indices(rows: &[ArchiveRow], filter: &str) -> Option<Vec<usize>> {
    let normalized_filter = filter.trim().to_lowercase();
    if normalized_filter.is_empty() {
        return None;
    }

    Some(
        rows.iter()
            .enumerate()
            .filter_map(|(index, row)| row.matches(&normalized_filter).then_some(index))
            .collect(),
    )
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

/// Local text check for the artwork-picker tests (they live at the module
/// top level, outside the main test module's helper scope).
#[cfg(test)]
fn picker_output_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|shape| shape_contains(shape, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

#[test]
fn file_pick_drain_keeps_pending_while_empty() {
    let (sender, receiver) = mpsc::channel();
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Pending);
    // A second drain on the still-open channel is still Pending.
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Pending);
    drop(sender);
}

#[test]
fn file_pick_drain_disconnected_is_reported_and_repeated_drains_do_not_panic() {
    let (_sender, receiver) = mpsc::channel::<Option<PathBuf>>();
    drop(_sender);
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Disconnected);
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Disconnected);
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Disconnected);
}

#[test]
fn file_pick_drain_cancel_is_not_an_error() {
    let (sender, receiver) = mpsc::channel();
    sender.send(None).unwrap();
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Cancelled);
    // A cancelled channel then disconnects cleanly.
    drop(sender);
    assert_eq!(drain_file_pick(&receiver), FilePickDrain::Disconnected);
}

#[test]
fn file_pick_drain_picked_returns_the_path() {
    let (sender, receiver) = mpsc::channel();
    sender.send(Some(PathBuf::from("/tmp/pic.png"))).unwrap();
    assert_eq!(
        drain_file_pick(&receiver),
        FilePickDrain::Picked(PathBuf::from("/tmp/pic.png"))
    );
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

#[test]
fn a_disconnected_picker_releases_and_shows_a_friendly_error() {
    let (_sender, receiver) = mpsc::channel::<Option<PathBuf>>();
    drop(_sender);
    let mut manager = PlatformArtworkManagerState {
        pending_pick: Some(FilePickRequest {
            platform_id: "gamecube".to_string(),
            custom: false,
            receiver,
        }),
        ..Default::default()
    };
    let mut cache = PlatformArtworkCache {
        directory: None,
        entries: std::collections::HashMap::new(),
        bundled_entries: std::collections::HashMap::new(),
    };
    let mut action = None;
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_platform_artwork_manager(ui, None, &mut cache, &mut manager, &mut action);
        });
    });
    assert!(
        manager.pending_pick.is_none(),
        "a disconnected picker must release pending_pick so a new picker can start"
    );
    assert!(
        picker_output_text_contains(
            &output,
            "The image picker closed unexpectedly. Please try again."
        ),
        "the friendly error must be visible"
    );
}

#[test]
fn a_still_pending_picker_is_not_released_and_blocks_a_second_one() {
    let (sender, receiver) = mpsc::channel();
    let mut manager = PlatformArtworkManagerState {
        pending_pick: Some(FilePickRequest {
            platform_id: "gamecube".to_string(),
            custom: false,
            receiver,
        }),
        ..Default::default()
    };
    let mut cache = PlatformArtworkCache {
        directory: None,
        entries: std::collections::HashMap::new(),
        bundled_entries: std::collections::HashMap::new(),
    };
    let mut action = None;
    let context = egui::Context::default();
    let _output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_platform_artwork_manager(ui, None, &mut cache, &mut manager, &mut action);
        });
    });
    assert!(
        manager.pending_pick.is_some(),
        "an open picker stays pending; no second dialog can start"
    );
    drop(sender);
}

#[test]
fn a_cancelled_picker_releases_without_an_error() {
    let (sender, receiver) = mpsc::channel();
    sender.send(None).unwrap();
    let mut manager = PlatformArtworkManagerState {
        pending_pick: Some(FilePickRequest {
            platform_id: "gamecube".to_string(),
            custom: false,
            receiver,
        }),
        ..Default::default()
    };
    let mut cache = PlatformArtworkCache {
        directory: None,
        entries: std::collections::HashMap::new(),
        bundled_entries: std::collections::HashMap::new(),
    };
    let mut action = None;
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_platform_artwork_manager(ui, None, &mut cache, &mut manager, &mut action);
        });
    });
    assert!(
        manager.pending_pick.is_none(),
        "a cancelled picker is released"
    );
    assert!(
        !picker_output_text_contains(
            &output,
            "The image picker closed unexpectedly. Please try again."
        ),
        "cancelling must not show an error"
    );
    assert!(
        manager.pending_import.is_none() && action.is_none(),
        "cancelling changes nothing"
    );
}
