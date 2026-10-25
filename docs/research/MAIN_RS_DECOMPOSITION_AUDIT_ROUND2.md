# `main.rs` Decomposition Audit — Round 2

Audit date: 2026-09-15  
Worktree: `/home/davedap/emuwiz-main-release-fix`  
Starting revision: `2908c3c5e80592f2dcfe17594188966020a04519`  
Scope: read-only architecture audit; no Rust moves or behaviour changes.

## 1. Executive Summary

`crates/archivefs-gui/src/main.rs` is 14,629 lines at the audited revision. It
is not one undifferentiated file: several useful seams already exist in
`navigation.rs`, `library_view.rs`, `platform_source_actions/`, and the many
feature page modules. The remaining problem is that `ArchiveFsApp` still owns
almost every long-lived state slot and the central `update`/dispatch path still
knows about every worker, dialog, page, and action.

The safest decomposition is therefore incremental:

1. extract the pure activity/history model and formatters;
2. extract the pure navigation types/mappings and shell policy;
3. finish the existing library-view seam by moving its action state and
   controller helpers together;
4. complete the existing source-action seam by moving the remaining app
   orchestration behind a narrow event API;
5. extract database loading and setup/diagnostics workers only after their
   generation/channel contracts have tests around them.

The first two are low-risk and high-collision-value. The source and database
moves are medium/high risk because they cross `ArchiveFsApp`, background
receivers, refresh generations, history, and the single-writer gates.

The recommended end state is not a collection of tiny controllers. `main.rs`
should retain top-level composition, app state ownership, and a thin event
router, while feature modules own their typed state, pure policy, worker
protocol, and page-specific rendering. A controller should return events or
mutate a focused context; it should not receive `&mut ArchiveFsApp` merely to
avoid designing an interface.

Current dirty areas were preserved. In particular, the audit did not edit
`main.rs`, `storage_health_page.rs`, or any production file.

## 2. Current `main.rs` Responsibility Map

| Region | Lines | Actual responsibility | Assessment |
|---|---:|---|---|
| imports/module declarations | 1–329 | imports almost every core and GUI feature plus existing extracted modules | primary coupling hotspot; should shrink only as seams are extracted |
| library layout helpers | 330–454 | column sizing, dialog sizing, card layout, library-view form validation | pure UI policy; low-risk extraction with library-view work |
| activity/history model | 455–819 | `ActivityAction`, outcomes, history filters, entries, in-memory log, display text | cleanest independent extraction |
| process bootstrap/logging | 820–944 | logger, icon, clipboard self-check, `main` | remains in bootstrap; logging can stay |
| load/display row model | 945–1369 | snapshot wrapper, archive rows, filters, bulk confirmation gates | mixed: row model can move later; selection/action gates are app-facing |
| live load and setup state | 1370–1728 | refresh generations, load/diagnostic states, safety/readiness helpers | split pure generation/readiness policy from worker orchestration |
| database state/load | 1729–2105 | database generations, cached snapshot, worker, schema/health classification, snapshot loading | strong `database_load.rs` candidate, but channel ownership is delicate |
| action/dialog declarations | 2106–2520 | setup, BSFree, source/library-view dialog and action state, duplicate/health filters | move with owning feature; do not create one dialogs junk drawer |
| top-level view enums/mappings | 2547–3399 | `MainView`, tabs, overlays, titles, width, scroll policy, archive context/config | navigation seam is real; `ArchiveContext` belongs near selection, not routing |
| `ArchiveFsApp` state | 3504–4142 | approximately 130 fields covering all pages, workers, dialogs, selections, caches | remains central initially; progressively replace feature fields with state bundles |
| app construction/navigation/startup | 4213–4881 | `ArchiveFsApp` impl construction, navigation, refresh, first workers | top-level coordinator; navigation methods can become adapters |
| inspection/preparation/load polling | 4860–5667 | archive workers, database polling, diagnostics polling, health cache | database and inspector seams; polling must remain central until event API exists |
| page/action dispatch | 5687–7335 | Sources, DAT, setup, library views, missing removal, operations, selected game, launch | largest behavioural collision region; extract one workflow at a time |
| operation model | 7336–7513 | mount/unmount request/result/progress, feedback, activity bridges, `Drop` | operation worker protocol; keep central until mount modules expose events |
| main egui composition/update | 7514–10209 | frame polling, nav, menus, overlays, page selection, dialogs, all page calls | must remain composition root, but should lose feature internals |
| remaining page helpers | 10210–12611 | startup, setup UI, doctor, inspector, library-view messages, mount/selected page | move in cohesive slices, not by line count |
| shared clipboard/text UI | 12612–13047 | native clipboard backend, UTF-8-safe editing context menu, metric cards | clipboard is a reusable UI utility; low collision but not first priority |
| GUI mode/RomM | 13048–14369 | mode persistence, RomM loading/operations, file verification, artwork/cover fetch | separate RomM/artwork seam exists conceptually but is highly coupled today |
| tests | 14370–14629 | focused pure/helper tests for picker, mode, clipboard | useful anchors; no snapshot rendering suite exists |

The file currently has 42 enum declarations, 40 structs, 8 type aliases, 36
constants, and 160 top-level functions by a declaration inventory (286 named
declarations; 320 declaration/`impl` sites including 34 impl blocks). The
counts include test helpers and small UI utilities, so they are a scope
measure, not a proposed module count.

## 3. Top-Level Symbol Inventory

The following is the complete audited declaration register, grouped only where
the symbols share one responsibility and the same extraction decision. Each
row records the declaration line range, callers/callees at the boundary,
state/I/O profile, destination, and difficulty. `R`, `W`, and `B` mean the
cluster reads, writes, or both to `ArchiveFsApp` state. Pure helpers have no
app-state access. Worker entries include filesystem/database/thread effects.

| Symbols (line range) | Responsibility; boundary callers/callees | State / I/O | Destination; difficulty |
|---|---|---|---|
| `COLUMN_WIDTHS`, `COLUMN_HEADERS`, `MIN_RESIZABLE_COLUMN_WIDTH`, `MAX_RESIZABLE_COLUMN_WIDTH`, `COLUMN_RESIZE_HANDLE_WIDTH`, `HEALTH_METRIC_MIN_WIDTH`, `HEALTH_METRIC_HEIGHT`, `LIBRARY_VIEW_DIALOG_MAX_WIDTH`, `LIBRARY_VIEW_DIALOG_MAX_HEIGHT` (330–392) | layout constants used by library/health renderers | pure | `library_view.rs` / `ui`; LOW |
| `LibraryColumnWidths`, `Default`, `as_array` (351–379) | persistent library table widths; called by library rendering and app construction | app field R/W through callers; no I/O | `library_view.rs`; LOW |
| `responsive_library_column_widths`, `responsive_card_columns`, `library_view_dialog_size`, `library_view_selections_side_by_side`, `library_view_submit_blocker`, `library_view_form_profile` (380–454) | pure library-view sizing/validation helpers; called by render/action code | pure except dialog input | `library_view.rs`; LOW |
| `SEARCH_FILTER_TEXT_EDIT_ID`, `HISTORY_LIMIT`, activity panel constants, unmount/recovery wording constants (455–474) | library search, history panel, mount recovery copy | pure | history/mount modules; LOW |
| `ActivityAction`, `ALL_ACTIVITY_ACTIONS`, `ActivityOutcome`, `ALL_ACTIVITY_OUTCOMES`, `HistoryLogFilters`, `history_entry_visible`, `history_entry_matches_text`, `visible_history_entries`, `Display` impls (477–771) | typed activity categories/outcomes and filtering; called by operation/source/setup/catalogue renderers | pure; filters R | `activity_history.rs`; LOW |
| `HistoryEntry`, `HistoryEntry::new`, `OperationHistory`, `record`, `clear`, `entries`, `remove` (772–819) | bounded in-memory session history; called by nearly every action poller and activity panel | `ArchiveFsApp.history` R/W; no external I/O | `activity_history.rs`; LOW |
| `gui_version_line`, `StderrLogger`, logger impl, `init_logging`, `resolve_log_level`, `LINUX_APP_ID`, `APP_ICON_PNG`, `app_icon`, `main`, `run_clipboard_check` (820–944) | process bootstrap and logging | process/env/stdout; no app state except startup | remain in `main.rs`/bootstrap; LOW to move, low collision value |
| `LoadedData`, `LoadedData::from_snapshot`, `RowOrigin`, `RowOrigin::label`, `gamer_view_label`, `ArchiveRow`, its constructors/filters/color helpers (945–1159) | turn live/persisted snapshots into display rows; consumed by library/selection | reads core snapshots and filesystem existence; no app mutation | `library_rows.rs` or `library_view.rs`; MEDIUM |
| `build_display_rows`, `LibraryRowFilters`, `is_active`, `matches` (1169–1315) | filters/sorts visible library rows | reads snapshot and app filter state via callers | `library_rows.rs`; MEDIUM |
| `show_bulk_action_typed_count_gate`, threshold and confirmation helpers (1317–1368) | safety gate for destructive/bulk actions | pure; caller reads dialog state | `bulk_confirmation.rs` (already exists) or keep there; LOW |
| `RefreshGeneration`, `next`, `gather_selected_evidence_with_registry*` (1370–1497) | stale-result protection and selected evidence adapters | reads app snapshot/registry through explicit args; background evidence may scan/read | `selected_evidence_pipeline.rs`; MEDIUM |
| `LoadResult`, `LoadMessage`, `DiagnosticsMessage`, `LoadState`, `DiagnosticsState`, `generation`, `SetupAction`, `DiagnosticsUiAction`, diagnostic/readiness helpers (1499–1727) | app startup/live snapshot and setup diagnostic state machines | B; receiver/thread/config identities; filesystem/config reads | split `load_state.rs` (pure types) and `setup_controller.rs`; MEDIUM |
| `DatabaseGeneration`, `next`, `CachedLibrarySnapshot`, `DatabaseOutcome`, `DatabaseLoadError`, `DatabaseLoadResult`, `DatabaseMessage`, `DatabaseState`, `snapshot`, `is_loading`, `is_scanning`, `status_label` (1729–1858) | persistent catalogue state and cached snapshot contract | B; owns receiver/worker and cached DB data | `database_load.rs`; MEDIUM |
| `start_database_load`, `load_database_snapshot`, `load_database_snapshot_at`, `classify_unhealthy_database`, `load_snapshot_from` (1859–2105) | background database load/scan, migration and snapshot assembly | DB open/read/write for explicit scan; filesystem/config; thread/channel | `database_load.rs`; HIGH because paths/generations and `DatabaseState` are coupled |
| `RunningSetupAction`, `BsFreeManagerState`, `BsFreeOperation`, `BsFreeOperationResult`, `RunningBsFreeOperation`, `BsFreeGuiState` (2106–2169) | BSFree/catalogue worker state | app fields R/W; network/cache operations through callers | `cheats_mods` / source page; MEDIUM |
| `SourcesAddDialogState`, `SourcesRemoveDialogState`, `RunningMissingRemoval` (2170–2191) | source and stale-entry dialog/worker state | B; path/config/database write via action controller | source/missing-removal modules; MEDIUM |
| `LibraryViewAction`, `LibraryViewActionOutcome`, `RunningLibraryViewAction`, `LibraryViewFormDialogState`, `LibraryViewRemoveDialogState`, `LibraryViewPlanFilter` and impl (2192–2354) | plan/apply/repair/configure library views | B; background filesystem/link/config operations | finish `library_view_controller.rs`; MEDIUM |
| `DuplicateReviewFilters`, `DuplicateSortField`, `Display`, `DuplicateGroupIdentity`, conversion (2356–2406) | duplicate review presentation/filter identity | reads cached report; no worker | `exact_duplicate_review_page.rs`; LOW |
| `HealthIssueFilter`, impl, `HealthDashboardFilters`, `HealthSortField`, `Display`, `HealthReportCacheKey`, `HealthReportCache` (2407–2546) | health dashboard filters/cache | R/W cache; core health computation may read snapshot | `storage_health_page.rs`/health module; MEDIUM, but dirty-file collision |
| `MainView`, `LibraryTab`, `TOOLS_MENU_WORKFLOWS`, gamer menu constants (2547–2776) | top-level destinations and user-facing labels | enum values only; app fields read/write at adapters | `navigation.rs`; LOW |
| `setup_check_summary`, `home_library_snapshot`, `main_view_for_home_card`, `main_view_for_library_tab`, `library_tab_for_main_view`, `library_tab_label`, `ProblemsRepairTab`, its mappings, `SourcesTab`, its mappings, `sources_tab_label`, `show_library_shell_header` (2778–3063) | home/tab/view conversions and shell tab row | pure except shell receives UI; app navigation methods call mappings | `navigation.rs`; LOW/MEDIUM for shell UI |
| `ToolsOverlay`, `InspectorSortField`/`Display`, `InspectorMessage`, `ArchiveInspectorStatus`, `ArchiveInspectorState`, preparation state and impl (3064–3194) | diagnostics/inspector overlay state | B; background inspection/preparation threads | `archive_inspector.rs`; HIGH if moved before an event boundary |
| `DEFAULT_INSPECTOR_PATH_COLUMN_WIDTH`, `catalogue_status_load_needed`, `main_view_title`, `main_view_content_width`, `main_view_uses_page_scroll`, `romm_readiness_label` (3195–3398) | global navigation presentation policy | pure except catalogue manager input | `navigation.rs`; LOW |
| `ArchiveContext`, `select_only`, `clear_selection`, `prune`, `active_cheats`, `GuiConfigSnapshot`, load/reload/source roots, `load_default_gui_config` (3400–3503) | selection identity and GUI config cache | B; config filesystem reads/writes via reload/save | `selection_context.rs` plus config helper; MEDIUM |
| `ArchiveFsApp` (3504–4142) | aggregate state owner for all workflows | B across all fields; owns workers, receivers, selections, dialogs, caches | remains central initially; HIGH to split wholesale |
| `ArtworkManagerFilter`, `PlatformArtworkTaskResult`, `PlatformArtworkManagerState`, `FilePickRequest`, `FilePickDrain`, `drain_file_pick`, picker error constant (4143–4212) | artwork/picker task state | B; picker channel/filesystem/artwork worker | artwork/picker modules; MEDIUM |
| `ArchiveFsApp::is_busy`, `new` (4213–4488) | app construction and initial worker/state setup | W; config and environment reads, threads/channels | remain composition root; HIGH to split constructor |
| `navigate_to_library_tab`, `navigate_to_missing_catalogue_review`, `navigate_to_problems_repair_tab`, `navigate_to_sources_tab`, `navigate_to_home_card`, `navigate_to_main_view`, `switch_to_advanced_view_at_home`, reconcile methods (4489–4839) | sanctioned navigation transitions | B on `view`, tab fields, overlays, filters, selection | navigation adapter; LOW/MEDIUM |
| `feature_discovery_context`, `museum_selected_game` (4583–4804) | selected-game cross-feature projection | R from many app fields; no mutation/I/O | selected-game adapter; HIGH if bundled with navigation |
| `refresh`, archive inspection/preparation start/poll/reconcile/finalize/view/path helpers (4840–5243) | asynchronous archive inspection and preparation | B; threads/channels, filesystem reads, generation checks | `archive_inspector_controller.rs`; HIGH |
| `poll_load` (5244–5319) | applies load worker result, updates snapshot/staleness/history | B; receiver join/generation | `database_load` event adapter; HIGH |
| `start_database_action`, `poll_database_load` (5320–5501) | database scan/load dispatch and result application | B; DB scan/migration worker and refresh | `database_load.rs`; HIGH |
| `prune_selection`, `refresh_diagnostics`, `cached_health_issues`, `poll_diagnostics` (5502–5623) | selection/cache/diagnostic refresh loops | B; receivers, core diagnostic reads | split health cache and setup controller; MEDIUM/HIGH |
| `start_setup_action`, `poll_setup_action` (5624–5678) | setup worker dispatch/results, feedback, config reload | B; filesystem/config writes depending action, receiver/thread | `setup_controller.rs`; HIGH |
| page show adapters for ROM organisation, publisher, optical, history, sources, DAT, identify/rename (5687–6148) | page composition and action translation | B; page state plus source/catalogue workers | remain thin in main; move action handling, not routing |
| `start_catalogue_status_load`, `start_catalogue_retrieval`, `handle_catalogue_manager_action`, `poll_catalogue_manager` (6235–6440) | cheat catalogue cache/retrieval worker | B; network/cache, cancellation channels, history | `cheats_mods_controller.rs`; MEDIUM/HIGH |
| `library_view_action_available`, reload/start/poll action, `library_view_current_skip_count`, logging/message helpers, `run_library_view_action` (6443–6593, 11559–11790) | library-view controller and worker protocol | B; config/filesystem/database and history | finish existing library seam; MEDIUM |
| missing-removal availability/reason/start/poll, generic `start_operation*`, progress polling and mount action handling (6594–7033) | destructive catalogue removal and mount operation orchestration | B; DB/filesystem/mount threads and confirmations | `repair_controller`/`mount_operations`; HIGH |
| `review_identity`, `open_emulator_setup_for`, `show_game_details`, profile remember/persist (7034–7335) | selected-game action adapter and emulator setup routing | B/R; config persistence/profile discovery | selected-game/setup adapter; MEDIUM |
| `ArchiveAction`, `OperationRequest`, `AppOperationRequest`, conversion, operation result/progress/failure/success, cleanup outcomes, activity helpers, `RunningOperation`, feedback, `Drop` (7336–7513) | mount/unmount transaction protocol and app feedback | B; worker threads, mount/filesystem mutation, history | `mount_operations.rs` plus feedback adapter; HIGH |
| artwork task start/poll (7514–7647) | platform artwork background work | B; filesystem/cache/thread | `platform_artwork_manager`; MEDIUM |
| `eframe::App::update` and `ui` (7648–10209) | frame polling, sidebar/home/menu routing, overlays, page composition | B on nearly every app field; UI only plus dispatch | remains composition root; HIGH to change, should become thinner |
| `start_load`, `start_diagnostics`, `open_default_config_folder`, `load_data`, missing-removal helpers, platform provenance/scan-format messages (10210–10670) | startup/scan helpers and library wording | mixed; filesystem/config/core reads | load/setup/rows modules; MEDIUM |
| `missing_config_is_first_run`, `show_setup_diagnostics`, activity panel actions/render, doctor summary/report/render (10671–11182) | setup/diagnostics and activity UI | R/W selected UI state; no direct worker spawn in render | setup/diagnostics/history modules; MEDIUM |
| inspector filters/details/row/panel, column constant (11183–11558) | archive inspector UI | R/W inspector state; inspection result already loaded | `archive_inspector.rs`; MEDIUM |
| `SourcePlatformState`, source helpers, mount queue/filter/confirmation, `SelectedPageViewState`, selected page, settings/artwork page helpers (11791–12549) | source platform labels, mount/selected/settings page composition | mixed; UI reads/writes app state, may emit actions | respective page modules; MEDIUM/HIGH because currently central |
| recovery activity/confirmation helpers (12550–12611) | lazy unmount safety and recovery UI | R/W confirmation/offers/history | `mount_operations.rs`; MEDIUM |
| clipboard trait, `NativeClipboard`, text edit context menu enum/impl/helpers, status/renderer (12612–13015) | shared native clipboard and edit menu | clipboard filesystem/process APIs; UI mutable text | `ui/clipboard.rs`; LOW/MEDIUM |
| `SummaryMetric`, health metric cards, `matching_row_indices` (12955–13047) | reusable health cards and filtering | pure UI | `ui/components`; LOW |
| `GuiMode`, mode path/load/save helpers (13048–13139) | persisted Gamer/Advanced mode | filesystem read/write; no app state except callers | `view_mode.rs` (already exists); LOW |
| RomM snapshot/load/operation and file confinement/verification/cover/screenshot/preview helpers (13140–14369) | RomM browse, network/cache, artwork, local verification | B; filesystem, network/cache, threads/async-like workers | `romm_*`, `gamer_artwork`, `romm_browse`; HIGH |
| tests: `benign_loose_rom...`, picker tests, GUI-mode tests (14370–14629) | focused pure/worker-state regression tests | temp files/env; no production catalogue | remain near extracted modules; preserve tests during moves; LOW |

This register deliberately treats an `impl` block as part of the symbol’s
owning cluster rather than pretending that moving a type without its methods is
an extraction. It also records existing extracted source action symbols in the
cluster that still consumes them from `main.rs`; they are not candidates for a
second source model.

## 4. Collision Heatmap

The recent history is heavily integration-shaped. The last 20 commits touching
`main.rs` include BIOS projection, emulator lifecycle/update review,
Ready-to-Play, publisher/media views, DAT authority, Needs Attention, database
restore, and artwork/museum features. The same commits commonly add a state
field, a navigation branch, a poll call, and a render branch in one file.

| Area | Heat | Evidence from current code/history | Practical implication |
|---|---|---|---|
| `update`/`ui` routing and shared polling, 7648–10209 | HOT | every feature adds a branch; latest feature commits add main branches; all workers are polled here | do not extract by bulk formatting; establish event APIs first |
| `ArchiveFsApp` field block/constructor, 3504–4488 | HOT | every page adds state and constructor defaults; 100+ feature fields | bundle state only as part of a controller extraction |
| source action dispatch/poll, 5753–6018 and 7699/8050/8533/10179 | HOT | Sources page, Home/Gamer View, and scan paths all call the same methods; source worker chains Add→Scan | first source extraction must preserve single-writer gates and chained scan |
| page routing/navigation, 2547–3399 and 4489–4839 | HOT/WARM | multiple recent feature commits add `MainView` variants and mapping arms | pure mappings are low-risk, but keep one central `MainView` until all arms are exhaustive |
| shared action polling, 5244–5667 and 6594–7513 | HOT | load, diagnostics, source, library-view, mount and setup workers share refresh/history/feedback | only move one protocol at a time; stale generations are safety boundaries |
| database load, 1729–2105 and 5320–5501 | WARM | database restore/scan and cache work changed this region; explicit scan is the only writer path | extract state/worker as a pair after tests, not loader functions alone |
| library-view action block, 2192–2354 and 6443–6593/11559–11790 | WARM | already cohesive and has directly associated `library_view.rs` | best medium slice after pure history/navigation |
| activity/history, 455–819 and 10863–11073 | WARM | many features append activity but the model is stable | extract model/formatters first; retain a small `record` adapter in main |
| setup/diagnostics, 1499–1728 and 5506–5678/10671–11182 | WARM | emulator/BIOS/Readiness integrations recently touched this area | separate pure state/policy first; defer worker ownership |
| clipboard, mode, metric cards, tests | COLD | no recent feature coupling; already self-contained | optional cleanup, not a collision-reduction priority |

`git diff` at audit start showed `main.rs` dirty. No attempt was made to
attribute uncommitted hunks to a worker or to edit around them. Any first
extraction must start from a clean coordination point or use a separate worktree.

## 5. Navigation Cluster

The navigation cluster is a viable focused module, with one deliberate
boundary: mappings and presentation policy move; app state transitions remain
thin methods or become returned navigation events.

Exact symbols:

- `MainView`, `LibraryTab`, `ProblemsRepairTab`, `SourcesTab`, `ToolsOverlay`;
- `TOOLS_MENU_WORKFLOWS` and Gamer menu labels;
- `main_view_for_home_card`, `main_view_for_library_tab`,
  `library_tab_for_main_view`, `library_tab_label`;
- `main_view_for_problems_repair_tab`,
  `problems_repair_tab_for_main_view`;
- `main_view_for_sources_tab`, `sources_tab_for_main_view`,
  `sources_tab_label`;
- `main_view_title`, `main_view_content_width`,
  `main_view_uses_page_scroll`, `catalogue_status_load_needed`;
- `show_library_shell_header` and `setup_check_summary` only if the module is
  allowed to depend on page presentation types.

The pure mapping functions are called by `ArchiveFsApp` navigation methods,
`update`, Home cards, sidebar/menu handling, and shell renderers. They read no
app fields and touch no I/O. `navigate_to_*` currently writes `view`, the
remembered tab, overlay state, and sometimes selection/filter state. It should
initially remain a five-line adapter in `main.rs`, calling module functions,
because moving it without moving the state contract gains little.

Answer: yes, this can become one `navigation.rs` module with minimal coupling.
In fact, `navigation.rs` already exists; the next extraction is to move the
remaining duplicated mapping/presentation helpers there, not create a second
navigation module. The only circularity risk is `home_page::HomeCard` and
`MainView`; solve it with a navigation function that imports the enum, not by
making `home_page` own global routing.

## 6. Sources Cluster

The source feature is partly extracted already:
`platform_source_actions/{state,controller,mod}.rs` owns `SourceAction`,
`SourceActionOutcome`, `RunningSourceAction`, source availability, worker
start/poll, core calls, messages, and source-specific tests.

The remaining main symbols are:

- `source_action: Option<RunningSourceAction>`, source scan echo fields,
  add/remove dialog fields, and source-related single-writer gates in
  `ArchiveFsApp`;
- `show_sources_page`, `show_sources_libraries_tab`, and the Sources/DAT
  adapters at 5753–6148;
- calls from Home/Gamer View and `update` that invoke
  `start_source_action`/`poll_source_action`;
- source scan result handoff to `pending_source_scan_summary`,
  `sources_last_scan`, `database_state`, feedback, and history.

The controller API can be narrow, but not stateless. Recommended shape:

```text
SourceControllerState
  running action + dialogs + last scan presentation

SourceControllerEvent
  Add(path), ScanOne(path), ScanAll, AssignPlatform(...), SetEnabled(...),
  Remove(...)

SourceControllerEffect
  Feedback, RefreshDatabase, Navigate(Discovery), ClearDialog,
  GamerFirstScanReview
```

`main.rs` supplies a focused context containing database-writer availability,
history sink, config reload callback, and refresh request. It should not pass
the whole app into a controller. The worker may still call existing core
functions and retain the worker join semantics.

Exact blockers:

- source and database actions share the single-writer gate;
- an Add from Gamer View chains a ScanOne and changes the next scan wording;
- scan results must be attached to the correct source scope and carried into
  the subsequent database snapshot reload;
- successful source changes reload GUI config and can refresh diagnostics;
- navigation from skipped/unknown discovery and source tabs is central;
- Source Role editing is not represented by the current `SourceAction` set.
  The GUI can show role and edit enabled/platform assignment, but the missing
  semantic role persistence dispatch requires shared `main.rs` integration.
  This is the exact reason that work remains blocked today: the page can emit
  an intent, but the central dispatch/state/reload path must own the persisted
  mutation and coordinate its writer gate. This audit does not reissue that
  task.

Recommendation: finish the existing `platform_source_actions` boundary first,
but do so as a controller/event extraction, not by duplicating `SourceAction`
or moving only the render function.

## 7. Library View Cluster

This is a strong existing seam. `library_view.rs` owns substantial page
presentation, while `main.rs` owns action declarations, worker state, plan
application, dialog state, feedback, and history wording.

Exact symbols are `LibraryViewAction`, `LibraryViewActionOutcome`,
`RunningLibraryViewAction`, `LibraryViewFormDialogState`,
`LibraryViewRemoveDialogState`, `LibraryViewPlanFilter`, the layout constants
and form helpers at 330–454, `library_view_action_available`,
`reload_library_views`, `start_library_view_action`, `poll_library_view_action`,
`library_view_action_log_category`, `library_view_action_started_message`,
`library_view_apply_summary_message`, `library_view_action_success_message`,
`library_view_current_skip_count`, and `run_library_view_action`.

It reads/writes `library_views`, `library_view_action`,
`library_view_last_plan`, form/remove dialogs, `feedback`, and `history`. It
touches config/filesystem/database through the existing core functions and
uses a worker receiver. It does not need all of `ArchiveFsApp` if given:

```text
LibraryViewContext {
  writer_available, history sink, feedback sink,
  current views, last plan, dialog state
}
```

The controller returns `LibraryViewEvent::{Reload, PlanReady, Applied,
ClosedDialog, Feedback}`. The page continues to render a state object. Do not
move `MainView` or generic `ActionFeedback` into this module.

Difficulty is MEDIUM. The main risk is preserving the invalidation rules that
clear an old plan after edit/apply/repair/remove and the shared writer gate with
source/platform/alias/missing-removal actions.

## 8. Activity / History Cluster

This is the lowest-risk extraction. It is a real independent model rather than
a controller:

- `ActivityAction`, `ALL_ACTIVITY_ACTIONS`, `ActivityOutcome`,
  `ALL_ACTIVITY_OUTCOMES`;
- `HistoryLogFilters`, visibility/matching/visible-entry helpers;
- `HistoryEntry`, `OperationHistory`, display implementations and constants;
- `activity_outcome_tone`, `activity_summary_entry`, and the history portion of
  `show_activity_panel`.

Callers are source, setup, catalogue, library-view, mount, RomM, repair, and
scan pollers. The model only needs `egui` for the panel and `widgets::StatusTone`
for presentation. It has no database or filesystem dependency. `ArchiveFsApp`
would retain `history: OperationHistory` and `history_filters` initially, or a
small `ActivityHistoryState`, while action pollers receive an `&mut
OperationHistory` sink.

Difficulty LOW. Existing tests for formatting/filtering should move with the
types; add no broad UI rewrite. This extraction removes a high fan-in model
from the file without changing dispatch.

## 9. Database Load Cluster

The database cluster is coherent but not low risk. Exact symbols are:
`DatabaseGeneration`, `CachedLibrarySnapshot`, `DatabaseOutcome`,
`DatabaseLoadError`, `DatabaseLoadResult`, `DatabaseMessage`, `DatabaseState`,
their methods, `start_database_load`, `load_database_snapshot`,
`load_database_snapshot_at`, `classify_unhealthy_database`, and
`load_snapshot_from`. The app methods `start_database_action`,
`poll_database_load`, and the first part of `poll_load` are its adapters.

Worker/channel ownership is explicit:

- `start_database_load` creates an `mpsc` channel and spawns one thread;
- the worker sends `(DatabaseGeneration, DatabaseLoadResult)` and requests a
  repaint;
- `DatabaseState::Loading` owns the receiver, generation, optional worker
  handle, previous snapshot, and scanning flag;
- `poll_database_load` is the sole consumer and joins/clears the worker;
- generation checks prevent stale results from replacing current state;
- explicit scan may migrate/open/write the database; ordinary load is
  read-only;
- successful results update cached source views, duplicate reports, DAT
  identities, and pending scan summaries.

The module should expose a state machine and pure worker functions, but return
an application event instead of updating history, feedback, navigation, and
diagnostic flags internally. A focused context needs current generation,
previous snapshot, pending source summary, and callbacks for refresh/feedback.

Do not pass `&mut ArchiveFsApp`; doing so would preserve the collision surface.
Do not split `DatabaseState` from its receiver protocol: that would make stale
message handling harder to review. Difficulty HIGH. Tests must cover no-database,
outdated/newer schema, failed load, scan-first, previous snapshot retention,
generation mismatch, and source-scan-summary handoff.

## 10. Setup / Diagnostics Cluster

The current cluster contains `LoadState`, `DiagnosticsState`, `SetupAction`,
`DiagnosticsUiAction`, `RunningSetupAction`, `diagnostics_can_continue`,
`starter_config_available`, `diagnostics_state_can_continue`,
`latest_generation_actions_safe`, `archive_action_block_reason`,
`snapshot_identity`, `action_readiness_debug_lines`, `start_diagnostics`,
`refresh_diagnostics`, `start_setup_action`, `poll_setup_action`,
`missing_config_is_first_run`, `show_setup_diagnostics`, and doctor summary /
report helpers.

Separate these into two concepts:

1. a pure safety/readiness policy module for generation, config identity, and
   action-block reasons;
2. a setup/diagnostics worker controller for receivers, core diagnostic calls,
   feedback, and history.

The policy reads `state` generation/staleness, `diagnostics`, and snapshot
identity. The controller writes `diagnostics`, `setup_action`, `feedback`,
`history`, `config_previously_confirmed`, and sometimes `mount_root_feedback`.
It reads config, snapshot, and mount-root state and may write configuration for
explicit setup actions. Difficulty MEDIUM for policy, HIGH for worker/UI.

The current `DoctorScanState` and page modules already provide useful state
boundaries. Do not create a second diagnostics model. The first extraction can
move only the pure helpers and tests, leaving polling in main.

## 11. Dialog Ownership

There are many dialogs, but they do not justify a generic `dialogs.rs` file.
Move each with the feature that understands its validation and consequence.

| State in `main.rs` | Owner | Keep central? | Reason |
|---|---|---|---|
| `SourcesAddDialogState`, `SourcesRemoveDialogState`, source picker feedback | sources/platform source actions | no | source path validation, Add→Scan chaining, and source writer gate |
| `LibraryViewFormDialogState`, `LibraryViewRemoveDialogState` | library-view controller | no | form validation, plan invalidation, apply/remove semantics |
| `confirm_remove_missing`, typed count and `RunningMissingRemoval` | missing-entry repair | no | destructive catalogue action and confirmation threshold |
| `confirm_mount_all`, `confirm_mount_selected`, typed counts, queue confirmations | mount operations/bulk confirmation | no | mount safety and exact selection re-derivation |
| `confirm_unmount*`, lazy-final confirmation, recovery offers | mount operations | no | filesystem/mount risk and recovery wording |
| `confirm_cheat_archive_change`, cheat picker | cheats/mods | no | fetched catalogue state and archive identity |
| DAT/cheat catalogue review and retrieval states | DAT/cheat source module | no | retrieval/cancellation/provenance |
| `doctor_selected_finding`, `doctor_repair_review`, `doctor_repair_result` | doctor/repair page | no | finding identity and repair review/result |
| `database_restore_plan`, confirmation, feedback | database/repair controller | no | verified restore transaction |
| `show_about`, `tools_overlay` | app-global shell | yes, initially | overlays are global composition state; `ToolsOverlay` type may move to navigation |
| onboarding state | `onboarding.rs` | no | already has its own module and persistence contract |
| `romm_config_draft`, browse/game panels | RomM modules | no | provider-specific state |

Feature-specific dialogs should return typed actions to main. Truly global
modal arbitration (only one shell overlay, focus flags) can remain central.

## 12. `ArchiveFsApp` Coupling Matrix

| Proposed cluster | Fields read | Fields written | Access | Recommended boundary |
|---|---|---|---|---|
| activity/history | `history_filters`, action context | `history` | BOTH | `ActivityHistoryState` plus `&mut OperationHistory` sink |
| navigation | `view`, `library_tab`, `problems_repair_tab`, `sources_tab`, `tools_overlay`, `library_filters`, selection | same fields, sometimes selection | BOTH | pure mapping functions + `NavigationEvent`; thin app adapter |
| sources | `state`/database readiness, `gui_config`, `history`, gamer scan flags | `source_action`, dialogs, feedback, database reload flags, scan summaries, history | BOTH | `SourceControllerContext` and returned effects |
| library view | database state, writer availability | views, running action, plan, dialogs, feedback/history | BOTH | focused controller state/context |
| database load | generation, database state, pending scan summary | database state, refresh/snapshot, feedback/history | BOTH | owns receiver/worker in `DatabaseControllerState`; event to app |
| setup/diagnostics policy | load state, diagnostics, generations, config identity | none | READ | pure functions with explicit inputs |
| setup/diagnostics worker | config, snapshot, source action status | diagnostics, setup action, config flags, feedback/history | BOTH | setup context with callback/events |
| selected game | focused archive, snapshot, evidence, emulator states | navigation, selected-page state, remembered profiles | BOTH | selected-game adapter; no global router ownership |
| mount operations | live records, selection, diagnostics, refresh generation | operation/queue/confirmations, feedback/history, mount state | BOTH | existing mount operation modules + event reducer |
| artwork/RomM | caches, selected path, config | worker/cache/page state | BOTH | existing feature modules; later/high risk |

The key rule is that a context should contain only the fields a workflow can
legitimately read/write. A callback such as `request_refresh()` is preferable
to granting a source controller access to every app field.

## 13. Proposed Module Graph

The justified target graph is:

```text
main.rs (bootstrap, ArchiveFsApp, frame composition, event reduction)
  ├── navigation.rs
  ├── activity_history.rs
  ├── library_view_controller.rs -> library_view.rs, archivefs_core
  ├── platform_source_actions/ -> sources_page.rs, archivefs_core
  ├── database_load.rs -> archivefs_core::Database, config, diagnostics
  ├── setup_controller.rs -> doctor_page.rs, archivefs_core diagnostics
  ├── archive_inspector_controller.rs -> inspector/preparation page types
  ├── mount_operations/ -> archivefs_core mount/repair APIs
  └── feature page modules -> typed page state and render functions
```

Only the first six are part of this decomposition plan. `archive_inspector`
and mount work are shown because their current state explains why the central
polling loop remains large; they should not be extracted in the first tranche.

Module contracts:

- `navigation.rs`: imports `egui` and `home_page::HomeCard`; receives/returns
  enums and labels; no app state; no circular risk beyond HomeCard.
- `activity_history.rs`: imports `egui` widgets only; owns history model and
  panel; returns no app event except optional clear/remove action; low risk.
- `library_view_controller.rs`: imports core library-view APIs and page types;
  receives a focused context; returns action outcomes/effects; avoid importing
  `ArchiveFsApp`.
- `platform_source_actions/`: already imports core/source page state and
  currently uses `crate::*`; its next cleanup should replace the glob with a
  focused context only after behaviour is covered. It returns source outcomes;
  no new relationship model.
- `database_load.rs`: imports core database/config/health types and standard
  channels/threads; receives paths/config and generation; returns
  `DatabaseLoadResult` plus reducer events; no navigation import.
- `setup_controller.rs`: imports diagnostics core and setup page feedback;
  receives generation/config identity and returns diagnostic/setup events; no
  `ArchiveFsApp` import.

The `crate::*` in the existing source controller is a real coupling warning,
not evidence that the old source extraction failed. It is the first thing to
narrow once the app-level context is defined.

## 14. Extraction Order

1. **Activity/history model (LOW).** Move types, filters, display, and pure
   formatters; keep a central history field and call sites. This removes many
   declarations with almost no routing or worker risk.
2. **Navigation mappings/policy (LOW).** Consolidate the already-existing
   `navigation.rs` mappings, labels, widths, and scroll policy. Keep thin
   `ArchiveFsApp::navigate_to_*` adapters and `MainView` central.
3. **Library-view controller (MEDIUM).** Move action state, message helpers,
   worker start/poll, and feature dialogs as a single protocol. Preserve plan
   invalidation and writer gates.
4. **Source controller completion (MEDIUM/HIGH).** Build on
   `platform_source_actions`; move app-specific scan summary/config reload /
   Gamer View effects behind events. This is the first slice relevant to
   Source Role editing, but it must wait for shared `main.rs` ownership.
5. **Database load controller (HIGH).** Move state and worker together; keep
   generation, previous snapshot retention, scan-first write semantics, and
   pending source summary tests intact.
6. **Setup/diagnostics (MEDIUM then HIGH).** First extract pure safety helpers;
   then move worker/polling and doctor rendering only after event reduction is
   established.

This order reduces collision risk before touching the hottest shared poll loop.
It is intentionally not a “move all page methods” exercise.

## 15. Commit Plan

| Commit | Files | Symbols / expected reduction | Tests | Risk |
|---|---|---|---|---|
| 1 `refactor(gui): extract activity history model` | add `activity_history.rs`; minimal imports in `main.rs` | activity enums, history structs/filter helpers, ~300–450 lines | `cargo test -p archivefs-gui`; existing activity/history tests; `cargo check` | LOW; no routing |
| 2 `refactor(gui): centralize navigation policy` | extend existing `navigation.rs`; no new navigation file | view/tab enums, mappings, labels, width/scroll policy, ~250–400 lines; retain adapters | navigation and Home-card tests; GUI check | LOW/MEDIUM; exhaustive match drift |
| 3 `refactor(gui): isolate library view actions` | add `library_view_controller.rs` or place controller beside existing module | action/result/running state, dialog state, messages, worker reducer, ~450–650 lines | library-view plan/apply/repair tests, writer-gate tests, focused GUI tests | MEDIUM; filesystem/config worker |
| 4 `refactor(gui): reduce source action coupling` | narrow `platform_source_actions` context; optionally add controller state file | remaining source result effects, ~250–450 lines | source add/scan/remove/enable/platform tests, chained Gamer scan, source summary tests | HIGH/WARM; shared `main.rs` and Source Role collision |
| 5 `refactor(gui): isolate database loading` | add `database_load.rs` | DB state/worker/load functions, ~500–700 lines | all database/catalogue load/restore/scan-generation tests | HIGH/HOT; channel ownership and dirty DB code |
| 6 `refactor(gui): separate setup diagnostics policy` | add `setup_controller.rs` and/or `setup_policy.rs` | pure safety helpers first, worker second, ~350–600 lines | diagnostics generation/staleness, starter config, setup action tests | MEDIUM/HIGH; recent BIOS/emulator changes |

Do not combine commits 3–6. Each commit should move code with minimal import
changes, avoid rustfmt of unrelated blocks, and preserve the existing module
tests in their first location until the new boundary compiles.

## 16. Expected `main.rs` End State

After all six slices, a realistic estimate is 8,000–9,500 lines, not a tiny
file. The remaining size is justified by top-level egui composition and the
large `ArchiveFsApp` state reducer. The estimate assumes roughly 5,100–6,600
lines move; it is intentionally a range because feature adapters may remain
central for safety.

`main.rs` should still own:

- process bootstrap and `eframe::App` implementation;
- `ArchiveFsApp` top-level state ownership and construction, initially with
  grouped controller state fields;
- frame ordering: poll workers, reduce events, then render;
- top-level `MainView` selection and page composition;
- global modal arbitration and final navigation adapters;
- thin calls into source/library/database/setup/mount controllers;
- app-global feedback and refresh coordination where no feature owns it.

It should not still own:

- the activity/history data model and filters;
- pure navigation mappings, labels, width, or scroll policy;
- library-view action protocol, plan messages, or dialogs;
- source worker implementation and source-specific outcome formatting;
- database worker/channel/state implementation;
- setup/diagnostic pure policy or worker implementation;
- feature-specific dialogs that have no global arbitration role;
- RomM, artwork, clipboard, or text-edit implementation details where current
  modules already provide a natural home.

## 17. Risks / Circular Dependencies

- **`crate::*` imports:** existing source controller and page modules rely on
  main’s namespace. Narrow imports only after defining public(crate) state and
  event types; otherwise extraction produces accidental re-export churn.
- **Navigation ↔ Home:** `HomeCard` is defined by Home while `MainView` is
  central. Keep the conversion function in navigation and make Home emit a
  card, not a view.
- **History fan-in:** many workers record activity. Move the model first, but
  do not make every controller own a separate history; use one app-owned sink.
- **Database ↔ source:** source scans write/refresh the database and carry scan
  detail into the next snapshot. Keep a typed `RefreshDatabase` event and the
  pending summary in one reducer.
- **Setup ↔ mount safety:** `latest_generation_actions_safe` gates mounts using
  snapshot and diagnostics identity. Moving one half can silently weaken the
  safety gate; extract policy as pure code before receivers.
- **Global feedback:** `ActionFeedback` is shared UI state. Controllers should
  return typed feedback, not import the whole app or create competing feedback
  stores.
- **Dialogs:** a single `dialogs.rs` would create a new high-fan-in module and
  obscure feature validation. Keep only overlay arbitration central.
- **Dirty/shared files:** `main.rs`, `storage_health_page.rs`, and core files
  were dirty at baseline. No first extraction should be attempted until the
  worker owning `main.rs` coordinates a clean boundary.

The decomposition must avoid modules under roughly 100 lines unless they are a
stable public boundary. `setup_policy.rs` may be small if it owns a tested
safety contract; a one-function `home_navigation_adapter.rs` or a generic
`dialogs.rs` should not be created.

## 18. Validation Plan

No production validation was run because this task is design-only. Each future
tranche should use an isolated target and validate proportionally:

| Tranche | Required checks | Hidden regressions to watch |
|---|---|---|
| history | `cargo test -p archivefs-gui`, activity/history focused tests, `cargo check -p archivefs-gui` | formatting/category drift, bounded history, clear/remove behaviour |
| navigation | navigation/Home/Problems/Sources mapping tests, GUI check | remembered tab restoration, overlays cleared, exhaustive view title/width/scroll policy |
| library view | library-view focused tests and GUI check | stale plan invalidation, confirmation gates, writer concurrency, dialog retention on error |
| sources | source controller tests, discovery/source tests, GUI check | Add→Scan chaining, scan scope, config reload, source summary and skipped-file handoff |
| database | database/catalogue/restore tests, core and GUI checks | generation mismatch, previous snapshot retention, read-only vs scan-first writes, schema classification |
| setup | diagnostics/readiness/mount-gate tests, GUI check | config identity mismatch, stale snapshot refusal, BIOS/emulator setup result routing |

There is no broad GUI snapshot suite in the inspected tests. Existing tests are
mostly pure state/formatting, worker protocol, and temporary-fixture tests;
therefore each move needs a before/after focused test for event ordering and a
manual review of the `update` frame ordering. No production catalogue or live
filesystem scan is required for the extraction tests.

### First extraction acceptance criteria

The first commit should be accepted only if:

- `main.rs` remains behaviourally identical apart from imports/module paths;
- no `ArchiveFsApp` field is renamed or removed;
- activity category/outcome strings and filter ordering are unchanged;
- all existing GUI tests pass in an isolated target;
- `git diff --check` is clean;
- the diff contains no unrelated rustfmt churn;
- concurrent dirty files remain byte-for-byte untouched.

## Decision Summary

The correct strategy is selective, staged extraction around existing seams—not
a rewrite and not six independent state managers. Start with activity/history,
then navigation. The first high-value collision reduction after those is the
library-view controller; source-role persistence remains blocked by the shared
central dispatch boundary and should be scheduled only when `main.rs` ownership
is available.

