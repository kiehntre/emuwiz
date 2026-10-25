# EmuWiz Main Shell / Update Ownership Audit — Round 5

Audit date: 2026-09-16  
Worktree: `/home/davedap/emuwiz-main-release-fix`  
Starting revision: `bdbb86b020561ba93cf72053ff9000e0c2305723`  
Scope: read-only architecture audit. No production Rust was changed.

## Executive summary

`main.rs` is now 9,813 lines and `ArchiveFsApp` has 80 fields. The state-blob
campaign has materially reduced field coupling, but the shell remains a large
composition root. The main structural problem is no longer missing worker
modules: it is that `ArchiveFsApp::update` (lines 4,468–7,046, 2,579 lines)
owns the complete frame lifecycle, while its rendering/effect section also
contains page-specific action reducers.

The safest first Round 5 implementation slice is to extract the shell's
top-level layout and navigation input handling into `app_shell.rs`, using a
typed shell result for navigation/overlay requests. This is preferable to
moving `ArchiveFsApp` to `app.rs` immediately: it reduces collision in the
hottest method while preserving the existing worker ordering and page
borrowing. The next slice should separate the ordered polling/reconciliation
phase from result/effect application. Only after those seams are stable should
the `ArchiveFsApp` definition and eframe implementation move to `app.rs`.

A tiny binary `main.rs` of roughly 150–280 lines is realistic, but only after
the application type, `update`, and `ui` implementation have moved to `app.rs`.
The present root cannot safely reach 100–300 lines through shell extraction
alone. A healthy `app.rs` is more realistically 2,800–4,200 lines unless the
remaining page bodies and reducers are also relocated.

## Baseline and cleanup context

The required preflight passed with a clean worktree:

| Item | Result |
|---|---|
| HEAD | `bdbb86b020561ba93cf72053ff9000e0c2305723` |
| Branch | `main`, ahead of `origin/main` by 139 commits |
| main.rs | 9,813 lines |
| ArchiveFsApp fields | 80 |
| Round 4 state objects | complete |
| production changes in this audit | none |

The latest commits are the ten state/controller extractions and the Round 4
state-object sequence. The tree is clean before this document is written.

## Current main.rs anatomy

| Lines | Region | Responsibility | Long-term owner |
|---:|---|---|---|
| 1–383 | imports and module declarations | imports nearly every GUI/core subsystem and exposes three binary targets | `main.rs` initially; gradually `app.rs` and feature modules |
| 384–1,020 | library layout policy | column widths, card layout, row models, filters and bulk confirmation helpers | existing `library_view`/library page owners |
| 1,021–1,920 | evidence, route and small policy types | selected-evidence wrappers, duplicate/health policy, view/tab enums, config snapshot | existing evidence/navigation/health owners |
| 1,941–2,280 | `ArchiveFsApp` declaration | 80 remaining global, page, cached and transient fields | `app.rs` for shell fields; existing state modules for feature fields |
| 2,352–4,467 | construction, navigation and feature adapters | `new`, route synchronization, refresh/load reducers, page wrappers and selected operation bridges | `app.rs` for construction/navigation; existing feature owners for render/reducers |
| 4,468–7,046 | `update` | ordered polling, auto-starts, shell layout, overlays, page dispatch and action application | split between `app.rs`, `app_shell.rs`, and page owners |
| 7,047–7,051 | eframe adapter | `eframe::App::ui` delegates to `update` | `app.rs` after type relocation |
| 7,053–9,553 | free UI helpers | setup, activity, doctor, inspector, selected, artwork, clipboard and health presentation | existing page/modules; shell retains only global overlays |
| 9,554–9,813 | mode helpers and tests | GUI mode persistence plus clipboard/picker/config tests | settings/bootstrap module and test owners |

The root has no separate `ui` method: the eframe `ui` method is a five-line
adapter and all frame work is in `ArchiveFsApp::update`.

## Declaration inventory

The current source contains 176 top-level declaration sites by the audit's
simple top-level scanner: 24 structs, 26 enums, 24 constants, one trait, 101
free functions, and 22 impl blocks. The count includes test-only declarations
where they are top-level in this file. The principal non-test declarations are:

- Layout/library: `LibraryColumnWidths`, `LoadedData`, `RowOrigin`,
  `ArchiveRow`, `LibraryRowFilters`, `RefreshGeneration`.
- Worker/feature residue: `BsFreeManagerState`, `BsFreeOperation`,
  `BsFreeOperationResult`, `RunningBsFreeOperation`, `BsFreeGuiState`,
  `RunningMissingRemoval`, duplicate/health filter and cache types.
- Routing: `MainView`, `LibraryTab`, `ProblemsRepairTab`, `SourcesTab`,
  `ToolsOverlay`, `ArchiveContext`, `GuiMode`.
- Shell/application: `GuiConfigSnapshot`, `ArchiveFsApp`,
  `ArtworkManagerFilter`, `PlatformArtworkTaskResult`,
  `PlatformArtworkManagerState`, `FilePickRequest`, `FilePickDrain`,
  `AppOperationRequest`, `ActionFeedback`, `CleanupFeedback`.
- Presentation/utility: `ActivityPanelAction`, `SourcePlatformState`,
  `MountPageAction`, `QueueConfirmChoice`, `SelectedPageViewState`,
  `SettingsPageAction`, `PlatformArtworkManagerAction`, `ArrowDirection`,
  `ClipboardTextStatus`, `ClipboardBackend`, `NativeClipboard`,
  `TextEditContextMenuAction`, and `SummaryMetric`.

The top-level constants include bootstrap/icon constants, library sizing and
search IDs, unmount/cleanup wording, the typed bulk threshold, file-picker
wording, inspector sizing, and the unknown-platform explanation. These are
not all shell-owned: user-facing operation wording should remain with its
feature owner when those helpers move.

## ArchiveFsApp field inventory

All 80 fields are accounted for below. The first column is the current field;
the second is its present practical owner and the third is the Round 5
decision.

| Fields | Current ownership | Round 5 decision |
|---|---|---|
| `state`, `refresh_error`, `snapshot_stale`, `refresh_generation`, `snapshot_generation`, `database_state`, `database_generation`, `needs_attention` | live/database projection and cross-feature readiness | remain app-level until a typed refresh/effect boundary exists |
| `library_ui`, `archive_context` | library UI and primary archive selection | keep in their existing state/context owners; shell passes focused inputs |
| `mount_ui`, `missing_removal_typed_count`, `confirm_bulk_platform_action`, `focus_bulk_platform_cancel`, `bulk_platform_action_typed_count` | mount UI and shell-routed bulk confirmation | keep state bundle; confirmation rendering belongs with mount/library pages |
| `history_filters`, `shared_history`, `shared_history_operation`, `shared_rollback`, `feedback`, `history` | global activity, rollback and notification sinks | remain app-level coordination; presentation can move |
| `doctor_repair`, `setup_action` | diagnostics/doctor/repair | existing state/controller owners |
| `emulator_readiness`, `tape_inspector_filter`, `launch_retroarch`, `launch_dolphin`, `launch_pcsx2`, `launch_standalone`, `launch_amiga_whdload` | readiness, tape and launch trackers | existing readiness/setup/launch owners; app retains handoff |
| `cheat_workflow`, `user_cheat_import_page`, `dolphin_texture_mod`, `local_mod_package`, `cheat_archive_picker`, `confirm_cheat_archive_change` | Cheats & Mods page/session | existing `cheats_mods` tree; not shell state |
| `cheat_reconciliation_review`, `cheatbase_page`, `emulator_download_page`, `rom_organisation_page`, `publisher_profile_page`, `repair_review_page`, `repair_history_page`, `exact_duplicate_review_page`, `optical_conversion_page`, `storage_health_page`, `library_view_history_page` | page-local state | page modules; shell should only lazily create and dispatch |
| `clipboard` | process-wide clipboard backend | app-level resource; initialize in constructor, pass to pages |
| `view`, `library_tab`, `problems_repair_tab`, `sources_tab`, `tools_overlay`, `show_activity`, `show_about`, `show_skipped_files`, `skipped_files_filter`, `select_all_visible_requested`, `ui_mode`, `gamer_view_screen` | shell/navigation and global overlays | `app_shell.rs`/`app.rs`; this is the cleanest shell bundle candidate |
| `catalogue_bsfree_ui`, `gui_config`, `romm_ui`, `selected_evidence_ui` | existing feature/session state | keep existing owners; shell only triggers view-gated starts and passes projections |
| `gamer_view_scan_review_available`, `gamer_view_scan_pending_review`, `sources_ui` | source/Gamer View cross-page session | existing Sources/artwork owners; retain thin cross-feature bridge in app |
| `library_views`, `library_view_action`, `library_view_last_plan`, `library_view_form_dialog`, `library_view_remove_dialog`, `library_view_focus_archive`, `library_view_plan_filter` | Library Views page/controller | existing `library_view_controller` and page; shell should route effects |
| `archive_inspector`, `archive_inspector_generation`, `archive_preparation`, `archive_preparation_generation` | inspector/preparation | extracted inspector controller; shell only opens overlay and routes requests |
| `artwork_media` | artwork/media/museum | existing state object; shell owns lazy page entry only |

The notable remaining shell cluster is 12 fields: the view/tab/overlay/mode
and global overlay flags. It is coherent enough for `AppShellState`, but it
should not be introduced as a second large state migration before the shell
extraction because the current fields are read directly throughout `update`.

## ArchiveFsApp methods

There are 76 `ArchiveFsApp` methods by the established cleanup-campaign metric
(including methods implemented in the extracted feature impl modules). 54
method declarations remain physically in `main.rs`; the remainder are in
feature impl modules. The principal methods remaining in `main.rs` are:

### Construction/navigation (2,352–2,617)

`is_busy`, `new`, `navigate_to_library_tab`,
`navigate_to_missing_catalogue_review`, `navigate_to_problems_repair_tab`,
`navigate_to_sources_tab`, `navigate_to_home_card`, `navigate_to_main_view`,
`switch_to_advanced_view_at_home`, and the three tab reconciliation helpers.
These are app-shell methods, although `new` still initializes every feature
state and should be moved only after a stable `app.rs` boundary exists.

### Database/diagnostic coordination (2,598–3,573)

`refresh`, `poll_load`, `start_database_action`, `poll_database_load`,
`prune_selection`, `refresh_diagnostics`, `cached_health_issues`,
`poll_diagnostics`, `start_setup_action`, and `poll_setup_action`. The first
two are cross-feature reducers; database and setup worker ownership has already
been extracted, but result installation and global feedback remain here.

### Page adapters and residual reducers (3,023–4,171)

`show_rom_organisation_page`, `show_publisher_profile_page`,
`show_optical_conversion_page`, `show_library_view_history_page`,
`show_sources_page`, `show_sources_libraries_tab`, `show_dat_sources_page`,
`show_identify_rename_page`, `show_dat_sources_page_mode`, catalogue/library
view/removal start/poll methods, `handle_mount_page_action`, plan-preview
methods, and remembered-profile persistence. These are the main feature-specific
methods still physically in the root.

### Artwork and frame loop (4,335–7,051)

`start_platform_artwork_task`, `poll_platform_artwork_task`, `update`, and the
five-line eframe `ui` adapter. `update` is the dominant remaining method and
should be split by lifecycle phase rather than copied wholesale into another
god object.

## update() audit

`ArchiveFsApp::update` is lines 4,468–7,046 inclusive (2,579 lines). It runs
every frame. Its responsibilities are:

| Phase | Approx. lines | Every frame? | State touched | Destination |
|---|---:|---|---|---|
| Route reconciliation and worker polling | 4,468–4,565 | yes | tabs, evidence/preparation, needs-attention, history, artwork | `app.rs` ordered scheduler; feature poll methods remain in owners |
| View-gated automatic starts | 4,563–4,629 | yes, guarded | RomM, BSFree/catalogues, Dolphin inventory, identity/cheat preview, readiness | feature controllers expose idempotent `ensure_*` calls; shell retains ordering |
| Busy/readiness/repaint calculation | 4,630–4,666 | yes | loading, diagnostics, busy state and generation safety | app-level coordination; return a small frame context |
| Advanced/Gamer menu and sidebar | 4,675–4,968 | yes in selected mode | view, mode, overlay, navigation, source/database/setup actions | `app_shell.rs` |
| Activity and tools overlays | 4,969–5,443 | when visible, with arbitration every frame | activity, feedback, diagnostics, inspector, database status, clipboard | `app_shell.rs` plus existing overlay/page owners |
| Gamer View | 5,443–5,431 | only in Gamer mode | artwork workers, evidence, preparation, source actions, launch and feedback | `gamer_view`/artwork owners with an app adapter |
| Museum | 5,443 onward branch | only on Museum view | artwork delivery, selected game, navigation actions | `museum_page` plus artwork state |
| Home/Needs Attention/Sources | 5,508–5,602 | selected view | home inputs, diagnostics, source tabs | existing page modules; thin shell adapters |
| Feature page dispatch | 5,604–6,884 | selected view | nearly every page state, feedback/history and action requests | one shell dispatch is acceptable; page bodies/reducers should leave root |
| Global action application | 6,885–7,046 | after page render | navigation, refresh, history, feedback, inspector, source actions | `AppEffect` reducer in `app.rs` eventually |

The exact frame order is behaviorally significant: polling occurs before
rendering; some worker deliveries are deliberately drained before page render;
view-gated starts prevent unvisited pages from doing I/O; action application
can immediately alter the next route. Any extraction must preserve this order.

### Every-frame work that does not obviously need to be every frame

- Receiver polling is cheap when idle but is repeated for more than 20 worker
  families. It should remain ordered and event-driven at the API boundary.
- Busy/readiness/generation predicates are cheap but fan out across many
  fields, making the method difficult to reason about.
- Gamer View may compare library identity, drain cover/screenshot deliveries,
  build request vectors, gather selected evidence state, and build launch
  inputs on every displayed frame. This is likely meaningful for large lists,
  but requires measurement before caching.
- Museum creates a library projection and artwork render-assets value on the
  view's frames. Likely minor for small snapshots, needs measurement for large
  catalogues.
- `Media Sets` refreshes its page model from the database snapshot on every
  frame while that view is active (line 5,595 onward). This is a clear
  event/generation-cache candidate.
- Library dispatch rebuilds visible/index/filter projections through page
  calls. The cost is not measured; likely meaningful for large libraries.

## Shell ownership

The shell responsibilities are currently distributed across `main`,
`navigation.rs`, and `update`:

- `main` owns logging, CLI flags (`--version`, `--clipboard-check`), icon,
  viewport/options, and `eframe::run_native`.
- `navigation.rs` owns route tables, labels, destination enablement, and the
  reusable sidebar renderer.
- `ArchiveFsApp` owns the route source of truth (`view`), derived tab values,
  overlay arbitration, mode switching, menu actions, top/bottom panels, and
  central dispatch.
- Free helpers own activity, setup/doctor, inspector and selected panels, but
  are still called by the root's large frame closure.

`app_shell.rs` is a justified owner for top menu, gamer top bar, sidebar
requests, overlay arbitration and shell-level status/activity placement. It
should return typed requests rather than mutate all feature state. It should
reuse `navigation.rs`; it must not become a duplicate navigation policy module.
About, skipped-files and overlay arbitration are genuinely shell-owned.
Source add/remove, mount confirmations, RomM configuration, library-view
dialogs, doctor repair and settings dialogs are feature-owned and should not be
put into a generic `dialogs.rs`.

## Polling and global coordination

The current repeated pattern is:

`poll worker → reject stale generation → install feature result → set feedback
→ record history → reload/refresh → navigate or repaint`.

Feature polling already lives in extracted controllers for many families, but
the root still calls every poll and owns several result-side effects. The
first effect boundary should be deliberately small:

```text
ordered frame scheduler
    → feature poll / page action
    → AppEffect { Feedback | History | Refresh | Navigate | Repaint }
    → one root reducer
```

`AppEffect` should carry typed feature summaries, not egui widgets or a generic
string-formatting service. Feedback wording and history category should remain
created by the feature that knows the operation. The root should own only
global sinks and ordering.

## Page dispatch and rendering ownership

The central dispatch remains legitimate as a shell concern, but its branches
are not equally clean:

| Surface | Current classification | Main residue | Recommendation |
|---|---|---|---|
| Home / Needs Attention | A/B | input assembly and route result | keep thin shell adapter |
| Sources/Libraries | C | `show_sources_libraries_tab`, 232 lines, mixes source, mount-root, catalogue, BSFree, RomM and history | first page-level rendering extraction after shell |
| Sources/DAT | B/C | `show_dat_sources_page_mode`, 106 lines, action translation and history | move to existing DAT page owner |
| Cheats & Mods | C | large branch, workspace inputs and action reducer | existing `cheats_mods` tree |
| Library/Archives and Views | C/B | selection/filter/page actions and library-view reducer remain interleaved | existing library modules |
| Mount / Active Mounts | C | page call plus queue confirmation and effect application | existing mount modules |
| Selected / Ready-to-Play | C/A | selected page wiring and launch/readiness inputs | selected/readiness owners |
| Archive Inspector | B | 266-line `show_archive_inspector_panel` remains in root | extracted inspector module |
| Problems & Repair | C | page wrappers and restore/repair effects | doctor/repair modules |
| Museum/Gamer | B | worker delivery and action translation in root | artwork/gamer owners |
| History & Logs | B | page action and database restore effect handling | history page plus app sink |
| Settings/About | A/B | settings action routing and About overlay | settings/app shell |

The one dispatch point can remain in the shell if each branch becomes a small
adapter. Page-specific rendering and action translation should not be moved
into `app_shell.rs` merely to reduce the root's line count.

## Modal and dialog ownership

Shell-owned: About, skipped-files, activity placement, and the rule deciding
which tool overlay is visible. Feature-owned: mount-all/queue/unmount/lazy
confirmations, source add/remove, library-view add/edit/remove, RomM config and
browse windows, doctor repair, database restore, DAT/cheat drafts, and the
Cheats & Mods archive-change confirmation. The root currently arbitrates these
dialogs inside `update`; the future API should pass a narrow feature context and
return a request, not relocate all dialog code into one module.

## Giant method ranking

### Highest priority / relatively safe

1. `update`, 2,579 lines: split lifecycle phases, then move shell and effect
   routing. High user-facing importance and highest collision risk; high risk
   if reordered.
2. `show_sources_libraries_tab`, 232 lines: feature-specific and existing
   owner is clear; moderate risk because it touches source, library, RomM and
   mount-root state.
3. `show_archive_inspector_panel`, 266 lines: pure-ish egui rendering over
   inspector inputs; moderate safety once imports/clipboard are explicit.
4. `show_platform_artwork_manager`, roughly 360 lines: clearly feature-owned,
   but high state and worker coupling; defer until artwork page context is
   explicit.

### High value but higher coupling

- `poll_database_load`, 151 lines: result installation combines DB snapshot,
  rows, duplicates, health, selection and global feedback. Needs typed effect
  tests before moving.
- `poll_catalogue_manager`, 115 lines: feature-specific but crosses history,
  feedback and refresh; existing Cheats & Mods tree should absorb it through an
  effect result.
- `persist_remembered_profile`, about 134 lines: filesystem/config mutation,
  setup state and feedback; move only with emulator setup ownership.
- `show_dat_sources_page_mode`, 106 lines: moderate safety, but DAT quick
  rename and navigation make it a cross-page adapter.
- `show_selected_page`, 110 lines: selected evidence/readiness/launch wiring;
  launch execution must remain in its owner.

There are four root methods over 200 lines if `update` is counted as the
frame method and `show_sources_libraries_tab`, `show_archive_inspector_panel`,
and `show_platform_artwork_manager` are counted as rendering methods. There
are ten clearly material methods over 100 lines when `new`, `poll_database_load`,
`poll_catalogue_manager`, `persist_remembered_profile`, `show_dat_sources_page_mode`,
and `show_selected_page` are included. The exact threshold count depends on
whether free rendering functions are included; the named candidates are the
actionable set.

## Performance opportunities

| Smell | Frequency | Assessment | Later invalidation/key |
|---|---|---|---|
| media-set model refresh from snapshot | every Media Sets frame | likely meaningful and low conceptual complexity | database generation |
| Gamer View cover/screenshot delivery and request-vector construction | every Gamer frame | likely meaningful for large lists | library generation, artwork identity, visible set |
| Gamer selected metadata/evidence/readiness input assembly | every Gamer frame while selected | likely meaningful; needs measurement | focused archive path + evidence/readiness generations |
| library filtering/sorting/visible-row projections | active Library frames | likely meaningful for large catalogues | library snapshot generation + filters/sort |
| Museum snapshot/artwork asset projection | Museum frames | probably minor, needs measurement | database generation + artwork source identity |
| repeated `PathBuf`/record/string clones in action contexts | action/render dependent | mixed; some required for borrow boundaries | selected path and snapshot generation |
| broad worker polling | every frame | probably minor CPU cost, high control complexity | event-driven repaint / receiver activity |

These are audit findings only. No caching or per-frame optimization belongs in
this shell ownership commit.

## Recommended app.rs boundary

Introducing `crates/archivefs-gui/src/app.rs` is now appropriate, but it should
follow one shell extraction rather than be a blind file move. The initial
boundary should be:

`main.rs`:

- module declarations and narrow public re-exports required by the three bins;
- logging/version/clipboard-check CLI handling;
- icon and native viewport/options;
- `eframe::run_native` and `ArchiveFsApp::new` wiring.

`app.rs`:

- `ArchiveFsApp` declaration;
- constructor and `Drop` implementation;
- eframe `App` implementation;
- ordered frame scheduler;
- shell state/effect reduction;
- cross-feature navigation and global feedback/history sinks.

Feature modules continue to own state, worker protocols, persistence and page
rendering. `app.rs` must not become a new 9,000-line home: after page reducers
and rendering leave the root, a healthy target is approximately 2,800–4,200
lines. Moving the type first has almost no line-count benefit, but provides the
stable binary/library boundary needed to make `main.rs` tiny.

## Sequential extraction plan

### Slice 1 — extract app shell (FIRST)

- Files: new `crates/archivefs-gui/src/app_shell.rs`, `main.rs`, likely shell
  tests under `crates/archivefs-gui/src/tests/`.
- Move: top menu, Gamer top bar, sidebar request handling, shell-level overlay
  arbitration, activity/status placement; retain page bodies.
- Expected root reduction: 300–550 lines, depending on closure API.
- Risk: medium; avoid changing route ordering and overlay precedence.
- Focused tests: navigation destinations, mode switching, overlay arbitration,
  menu enablement, About/skipped-files/activity visibility.
- Direct consumers: `navigation.rs`, `activity_history.rs`, setup/doctor and
  inspector inputs; no feature worker implementation should move.

### Slice 2 — split ordered polling/reconciliation from update

- Files: `main.rs`/future `app.rs`, possibly a small `app_effects.rs`.
- Move: the first scheduler phase and typed `AppEffect` collection; preserve
  exact poll order and view-gated starts.
- Expected root reduction: 250–450 lines.
- Risk: medium/high because stale-generation and refresh ordering matter.
- Focused tests: generation rejection, refresh after source/database actions,
  repaint while busy, one-running-worker behavior.

### Slice 3 — move high-value page renderers to existing owners

- Files: `sources_page.rs`, `dat_sources_page.rs`, inspector module, mount/page
  owners; main/app adapters.
- Move: `show_sources_libraries_tab`, DAT mode rendering, inspector panel,
  mount queue confirmation as their inputs become explicit.
- Expected root reduction: 500–850 lines.
- Risk: medium; source and global feedback/history handoff is the boundary.
- Focused tests: sources/DAT/inspector/mount UI regressions.

### Slice 4 — relocate `ArchiveFsApp`, `update`, and eframe adapter to app.rs

- Files: `main.rs`, new `app.rs`, `crates/archivefs-gui/Cargo.toml` only if
  target wiring requires it, and test imports.
- Expected `main.rs` reduction: 100–250 net lines initially; most code moves
  to `app.rs` rather than disappearing.
- Risk: high due private visibility, three binary aliases, and test fixtures.
- Focused tests: full GUI suite plus binary `--version` and clipboard-check
  smoke tests.

### Slice 5 — reduce app.rs through feature context/effect adapters

- Files: app.rs plus existing feature modules, not a new generic controller.
- Move: selected page, catalogue, mount and artwork action reducers in coherent
  cuts; keep domain semantics in current owners.
- Expected root/app reduction: 700–1,200 lines across several commits.
- Risk: medium/high and feature-specific.
- Focused tests: per-owner suites, then full GUI regression.

## Blockers and risks

- `update` is order-sensitive: workers are polled before render and some
  deliveries are intentionally drained before the same frame's page render.
- `ArchiveFsApp` fields are still accessed directly by many modules through
  crate-private visibility; shell extraction must use contexts/results rather
  than reintroducing forwarding fields.
- Global feedback/history are sinks, but feature-specific wording and categories
  are produced at many call sites. A broad notification abstraction would risk
  semantic changes.
- The three Cargo binary targets all point directly to `src/main.rs`. Moving
  the app type requires test/private-item and target wiring review.
- There is no comprehensive frame-level UI snapshot suite; focused regression
  tests must surround each slice.
- `#[allow(dead_code)]` modules and compatibility re-exports are not proof of
  dead code. Deletion requires a separate call-graph audit.

## Round 5 recommendation

### Exact first implementation slice

**Extract the top-level shell layout/navigation request handling into
`crates/archivefs-gui/src/app_shell.rs`, returning typed shell actions while
leaving `ArchiveFsApp` and feature state in place.**

This slice gives the best combination of collision reduction, moderate risk,
and a concrete seam for the later `app.rs` move. It must preserve the current
menu/sidebar ordering, Gamer/Advanced mode behavior, tool-overlay precedence,
global activity placement, and navigation policy already owned by `navigation.rs`.

## Validation

`git diff --check` was clean before this document was added. After writing the
document, run it again and run the task postcheck with only this document
allowed. No production Rust changes are expected.
