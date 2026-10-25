# Main.rs Decomposition Audit — Round 3

Audit date: 2026-09-15  
Worktree: /home/davedap/emuwiz-main-release-fix  
Starting revision: 0085107ba67b1c038acd952f218d08b1808a27ac  
Scope: read-only forensic architecture, dead-code, and crate-boundary audit. Production Rust was not modified.

## 1. Executive Summary

Round 2 reduced main.rs from 14,629 lines to 12,883 lines by extracting Activity/History, navigation policy, Library View actions, the source seam, database loading, and setup/diagnostics state. Those extractions are valid and should remain.

The remaining file is now best understood as a composition root wrapped around several unextracted feature reducers. The largest collision domain is the combination of:

- ArchiveFsApp's 234-field aggregate state;
- update, approximately 2,556 lines, which polls and reduces roughly two dozen worker families;
- ui, approximately 2,154 lines, which performs shell layout, page dispatch, modal rendering, and feature action translation;
- live library loading, archive inspection/preparation, mount transactions, selected-game routing, and RomM orchestration still embedded in the root.

The safest immediate implementation is one focused slice: extract the live library refresh/load protocol (LoadState, LoadResult, LoadMessage, start_load, load_data, poll_load, and its minimal result/effect boundary). This is distinct from database_load.rs: live loading reads the archive index, while database_load.rs owns the persisted catalogue. It provides meaningful collision reduction without combining several high-risk workflows.

No new Cargo crate is justified now. Core already owns the reusable domain logic, while the remaining GUI logic is stateful presentation and orchestration. The realistic healthy size of one-file main.rs after more focused extractions is 7,000–9,000 lines. A later main.rs to app.rs split could make the binary entrypoint 100–300 lines, but would relocate rather than eliminate the application complexity.

## 2. Round 2 Results

| Revision | Responsibility | Result |
|---|---|---|
| 3a868795 | Activity / History | activity_history.rs |
| 85b347491 | Navigation policy | navigation.rs |
| 60fe0ea | Library View controller | library_view_controller.rs |
| 42939c4 | Source controller seam | source_controller.rs plus platform_source_actions |
| 1442362 | Database load controller | database_load.rs |
| 0085107 | Setup / Diagnostics controller | setup_controller.rs |

Measured state:

| Item | Lines |
|---|---:|
| Original Round 2 main.rs | 14,629 |
| Current main.rs | 12,883 |
| Round 2 reduction | 1,746 |
| Round 2 modules combined | 2,123 |

The current SHA contains the complete Round 2 lineage. Pre-existing dirty files and untracked root/module-looking artifacts were preserved. The earlier audit's boundaries remain sound, but its 8,000–9,500 estimate should be treated as an intermediate range: live loading, mount operations, inspector/preparation, selected-game coordination, and RomM remain to be assessed separately.

## 3. Current Navigation / Surface Map

The current GUI has Gamer View and Advanced View. Advanced navigation includes Home, Needs Attention, Library, Ready-to-Play, Quick Rename, Library Organisation, Publisher/Frontend Library, Duplicate Finder, Disc Conversion, Emulator Setup, Emulator Manager, BIOS/Firmware, RomM, Mounts, Active Mounts, Cheats & Mods, Sources, Media Sets, History & Logs, Library View History, Problems & Repair, Automatic health report, and Settings.

Sources contains Libraries, DATs, Cheats, and Discovery tabs. Selected/game details, archive inspection, emulator readiness, PCSX2, storage health, repair review/history, local mods, and provider panels are reached from those surfaces rather than all appearing as top-level navigation.

This is a task-oriented improvement over a raw page list, but the shell still knows too much about every destination. The main.rs dispatch match is legitimate as composition; the page-specific action and modal branches around it are not.

## 4. Full Symbol Inventory

The current declaration scan found:

- 238 top-level declaration/impl sites;
- 204 named top-level declarations;
- 34 impl blocks;
- 120 method/function declaration sites;
- 234 ArchiveFsApp fields;
- 76 ArchiveFsApp methods in the two main impl blocks, excluding ordinary free helpers.

The complete inventory is grouped below by exact symbol family and line region. Small methods within a family have the same ownership and are deliberately grouped rather than hidden.

| Lines | Symbols | Responsibility/callers | State and I/O | Destination/difficulty |
|---:|---|---|---|---|
| 368–461 | COLUMN_WIDTHS, COLUMN_HEADERS, resize/health constants, LibraryColumnWidths, responsive_library_column_widths, responsive_card_columns | library/health/library-view presentation | pure | library_view or ui; LOW |
| 462–586 | gui_version_line, StderrLogger, init_logging, resolve_log_level, app_icon, main, run_clipboard_check | process bootstrap | env/process/stdout | remain in binary; LOW |
| 587–958 | LoadedData, RowOrigin, ArchiveRow, constructors, build_display_rows, LibraryRowFilters | live/cache rows, library selection | snapshot reads, metadata reads | library_rows; MEDIUM |
| 959–1,011 | bulk typed-count gate helpers | mount/platform/removal safety confirmations | pure | bulk_confirmation; LOW |
| 1,012–1,140 | RefreshGeneration, gather_selected_evidence_with_registry and variants | selected evidence and refresh callers | snapshot/registry reads, worker evidence | selected-evidence pipeline; MEDIUM |
| 1,141–1,232 | LoadResult, LoadMessage, LoadState, BsFree state, RunningMissingRemoval | live load and feature worker state | receiver/thread state | live-load and feature controllers; MEDIUM/HIGH |
| 1,233–1,421 | duplicate and health filter/sort/cache types | duplicate and Storage Health pages | cache/presentation state | page owners; MEDIUM |
| 1,422–1,749 | MainView, LibraryTab, ProblemsRepairTab, SourcesTab, setup_check_summary, home_library_snapshot, show_library_shell_header, route labels/policies | sidebar/home/shell | mostly pure, shell UI | navigation.rs; LOW/MEDIUM |
| 1,750–1,929 | ToolsOverlay, inspector sort/message/status/state, ArchivePreparationState and methods | archive inspection/preparation | channels, filesystem, generations | inspector controller/page; HIGH |
| 1,888–2,033 | catalogue_status_load_needed, main_view_title/content width/scroll, romm_readiness_label, ArchiveContext, GuiConfigSnapshot and loaders | route presentation, selection, config | config reads/writes and selection | navigation/selection/config; MEDIUM |
| 2,034–2,672 | ArchiveFsApp | aggregate app state for all workflows | reads/writes DB/config/filesystem/network/thread state | central initially; HIGH to split |
| 2,673–2,742 | ArtworkManagerFilter, PlatformArtworkTaskResult, PlatformArtworkManagerState, FilePickRequest/Drain, drain_file_pick | artwork/picker workers | channels, picker/filesystem | artwork/picker; MEDIUM |
| 2,743–3,018 | is_busy, ArchiveFsApp::new | construction and initial workers | config/env reads, worker setup | app construction; HIGH |
| 3,019–3,369 | navigate_to_*, feature_discovery_context, museum_selected_game, reconcile_* | navigation and selected context | app state reads/writes | thin navigation adapters plus selected-game adapter; MEDIUM |
| 3,370–3,773 | refresh, archive inspection/preparation start/poll/reconcile/finalize/view/path | selected page and launch preparation | channels, filesystem, generation checks | archive_inspector_controller; HIGH |
| 3,774–4,031 | poll_load, start_database_action, poll_database_load, prune_selection | live/persisted snapshot application | receiver joins, row rebuild, selection | live-load and database event reducers; HIGH |
| 4,032–4,744 | refresh_diagnostics, cached_health_issues, poll_diagnostics, start_setup_action, source/DAT page adapters, poll_setup_action | setup, sources, DAT, health | receivers, config, feedback/history | feature controllers/pages; HIGH |
| 4,745–4,952 | catalogue status/retrieval/action/poll | Cheats & Mods/Cheat Sources | network/cache/receiver/history | cheats_mods controller; MEDIUM/HIGH |
| 4,953–5,094 | library-view availability/reload/start/poll | Library Views | config/filesystem/receiver/history | existing controller; MEDIUM |
| 5,095–5,207 | missing-removal availability/start/poll | repair/library | destructive DB action/confirmation | repair controller; HIGH |
| 5,208–5,549 | generic operation worker, progress, mount actions, plan preview | Mounts/repair/preview | channels, filesystem, mount state | mount/repair controllers; HIGH |
| 5,535–5,836 | review_identity, open_emulator_setup_for, show_game_details, profile persistence | Selected and Emulator Setup | config persistence/readiness | selected-game adapter; MEDIUM |
| 5,837–6,014 | ArchiveAction, OperationRequest/AppOperationRequest, progress/results/failure/success, cleanup, RunningOperation, feedback, Drop | mount/unmount transaction protocol | filesystem/mount writes, channels/history | mount_operations; HIGH |
| 6,015–6,148 | platform artwork start/poll | artwork manager | filesystem/cache/thread | artwork module; MEDIUM |
| 6,149–8,705 | ArchiveFsApp::update and poll scheduling | all workers/features | mutates almost every app field | composition root, but must delegate; HIGH |
| 8,706–10,860 | ArchiveFsApp::ui and page dispatch | shell/page/modal composition | egui and app state | composition root plus feature renderers; HIGH |
| 8,711–8,755 | start_load, load_data | live library refresh | archive index/filesystem reads in worker | live-load controller; MEDIUM |
| 8,756–8,799 | apply_missing_removal and _at | repair persistence | database/config writes | repair controller; MEDIUM/HIGH |
| 8,800–9,152 | platform/scan wording, perform_archive_action, cleanup helpers | scan feedback and mounts | formatting plus filesystem/mount | feature owners; MEDIUM/HIGH |
| 9,153–9,340 | missing_config_is_first_run, show_setup_diagnostics | setup presentation | egui and diagnostics state | setup page; MEDIUM |
| 9,341–9,609 | ActivityPanelAction, activity panel, Doctor summaries/checks/report text | history and Problems & Repair | egui/presentation | history/doctor pages; MEDIUM |
| 9,610–10,044 | inspector filters/details/rows/panel | archive inspector | egui/inspector state | inspector page; MEDIUM |
| 10,045–10,241 | SourcePlatformState, source labels, mount queue/filter/confirmations | Sources and Mounts | presentation and safety state | source/mount pages; MEDIUM |
| 10,242–10,397 | selected page and platform artwork renderers | Selected/Settings | app feature state | selected/artwork pages; MEDIUM/HIGH |
| 10,398–10,860 | lazy recovery, NativeClipboard, text edit context menu, health metric cards | mounts, all text fields, health | filesystem/process clipboard/pure UI | mount/ui components; LOW/MEDIUM |
| 10,861–11,302 | GuiMode and RetroArch override persistence | Settings/Gamer View | config-directory filesystem | view_mode/settings; LOW/MEDIUM |
| 11,303–12,624 | RomM snapshot/operation/preview/verification/cover/screenshot/local facts | Sources/RomM/Selected | network/cache/filesystem verification | existing romm modules; HIGH |
| 12,625–12,882 | picker, mode, clipboard, diagnostics unit tests | test support | test-only callers | move with owner when proven; LOW |

The main import block also exposes stale ownership: compiler output identifies moved database/setup/library-view symbols and HISTORY_LIMIT as unused in main.rs. That is namespace fossil evidence, not a reason to change behavior in this audit.

## 5. ArchiveFsApp Field Audit

There are 234 fields. The important clusters are:

| Cluster | Examples | Classification and recommendation |
|---|---|---|
| live library | state, filter, filtered_rows, refresh_error, snapshot generations, archive_context | live snapshot state; immediate controller candidate |
| mounts | operation, mount_all/unmount_all, queues, confirmations, typed counts, lazy/remount offers | feature state; MUST bundle behind mount controller |
| history/feedback | history_filters, history, feedback, cleanup flags | history model is extracted; keep global sinks and shell filters initially |
| setup/diagnostics | diagnostics, setup_action, doctor scan/finding/review/result, generations | worker state belongs to setup/doctor; global feedback remains central |
| emulator/readiness | profiles, firmware evidence, setup overrides, launch states, remembered profiles | feature state; bundle after selected-game boundary |
| cheats/mods | cheat workflow, archive picker, BSFree, catalogue managers/retrieval/review | feature state; controller candidates, not a new crate now |
| sources/DAT/library views | source action/dialogs/scan echoes, DAT state/UI, library views/action/plan/dialogs | feature state crossed by refresh effects; typed events needed |
| duplicate/health | filters, selected items, health cache/generation | page state/cache; move with page when shared file ownership permits |
| navigation/shell | view, tabs, tools overlay, about/activity flags, clipboard, UI mode | genuinely app-global shell state |
| artwork/Gamer/Museum/media | artwork tasks/caches, Gamer View, museum, media and metadata state | page-specific; later focused extraction |
| RomM/identity/evidence | provider state, selected evidence, identity sources, plan preview | feature-specific; selected context must remain separate from provider state |

Fields that always travel together and should become bundles are mount state,
cheat/catalogue state, RomM state, doctor/repair state, inspector/preparation
state, and the live snapshot group. Do not create a generic AppState bag solely
to reduce the field count.

## 6. ArchiveFsApp Method Audit

There are 76 ArchiveFsApp methods in the main impls. Classification:

- **A, genuinely global:** new, is_busy, update, ui, navigation adapters,
  reconcile methods, Drop. These coordinate or compose and should remain thin.
- **B, feature coordination:** archive inspection/preparation, catalogue
  polling, missing removal, mount actions, selected-game actions, RomM effects.
  These should move behind focused contexts/effects.
- **C, rendering:** show_sources_page, show_sources_libraries_tab,
  show_dat_sources_page/mode, show_setup_diagnostics, show_activity_panel,
  show_archive_inspector_panel, show_mount_queue_confirmation,
  show_selected_page, show_platform_artwork_manager. These belong with pages.
- **D, persistence:** persist_remembered_profile, GUI mode/RetroArch override
  helpers, and feature action persistence. Feature persistence should be behind
  existing APIs; only global result reduction stays central.
- **E, database:** start_database_action/poll_database_load, poll_load, and
  missing removal. Persisted DB implementation is extracted; live loading and
  destructive repair remain.
- **F, background workers:** archive inspection/preparation, catalogue, missing
  removal, mount operation, artwork, RomM. Worker state and receivers should
  travel together.
- **G, dialog/modal:** mount/unmount/bulk, source/library-view, cheat archive,
  doctor repair, database restore, DAT/cheat drafts, RomM UI. Feature dialog
  state should move; one global overlay arbitration rule may remain.
- **H, navigation:** adapters remain appropriate; pure policy is already in
  navigation.rs.
- **I, readiness/compatibility:** feature_discovery_context, museum_selected_game,
  review_identity, open_emulator_setup_for, show_game_details. Move after the
  live snapshot context is explicit and consume core projections.
- **J, utility:** row construction, scan wording, clipboard, metric cards, mode
  parsing. Move only where a stable owner exists.
- **K, suspect:** unused imports, compiler-dead methods, and allow(dead_code)
  families. Use a separate proof/deletion tranche.

## 7. Long Function / Match Hotspots

Measured spans:

| Symbol | Approx. lines | Responsibility count |
|---|---:|---|
| update | 2,556 | worker polling, auto-start decisions, invalidation, repaint |
| ui | 2,154 | shell, page dispatch, dialogs, feature action translation |
| new | 267 | all state construction and initial workers |
| show_sources_libraries_tab | 228 | source list, scan, health, dialogs, role/platform display |
| show_game_details | 198 | identity, evidence, readiness, emulator setup, actions |
| poll_database_load | 150 | generation, worker join, snapshot result, duplicate/row refresh |
| museum_selected_game | 134 | selected-game projection |
| persist_remembered_profile | 110 | profile config write and feedback |
| poll_operation | 109 | mount progress/result/recovery/history |
| show_dat_sources_page_mode | 105 | DAT page rendering and mode |
| poll_catalogue_manager | 102 | network/cache result reduction |

The line-span heuristic finds 39 functions/methods over 30 lines, 12 over 100
lines, and 4 over 200 lines. Large matches in ui and update are legitimate
only at the outermost dispatch layer; current branches still contain
feature-specific policy and field mutation, so they are not yet thin dispatch.

## 8. Rendering Ownership

Move with feature pages:

- Sources list, scan summary, source dialogs, and source labels;
- DAT source page and identify/rename presentation;
- setup diagnostics renderer;
- activity panel;
- archive inspector row/details/panel;
- mount queue and recovery confirmations;
- selected page/game details;
- platform artwork manager;
- health metric cards when storage-health ownership is available.

Keep in the composition root:

- sidebar/menu construction;
- one MainView to page match;
- global modal arbitration;
- page invocation and conversion of page events into navigation, feedback,
  history, and refresh effects;
- frame order: poll, reduce, reconcile, render.

The aim is not to remove all egui from main.rs. It is to remove feature
implementation from the shell.

## 9. Dialog / Action / Worker Ownership

Remaining major families:

| Family | Owner decision |
|---|---|
| BsFree operation/UI | Cheats/Mods controller; MEDIUM |
| Missing removal | repair controller; HIGH |
| platform/alias/bulk platform actions | library/platform controller; HIGH due writer gate |
| catalogue and Dolphin catalogue retrieval | Cheats/Mods controller; MEDIUM/HIGH |
| inspector/preparation | archive inspector controller; HIGH |
| mount operation/progress/recovery | mount controller; HIGH safety |
| artwork task | existing artwork module; MEDIUM |
| RomM operation/progress | existing RomM modules/controller; HIGH |
| doctor/repair review/result | doctor/repair controller/page; MEDIUM |

Do not create dialogs.rs. Feature-specific state belongs with the feature; only
shell-level modal arbitration belongs centrally.

## 10. Persistence / Database Leakage

Persisted catalogue loading is in database_load.rs, but main.rs still owns
start_database_action and the result reducer. That is appropriate temporarily,
because source scans carry a summary into the next database reload; the next
boundary should return typed effects rather than grant a controller the whole
app.

Live archive-index loading remains in start_load/load_data/poll_load and is the
immediate extraction candidate. Missing removal and mount operations still
perform destructive database/filesystem work from main.rs workers. Remembered
profile persistence and GUI mode persistence are small feature helpers.
RomM verification/cache operations are provider-specific and should remain in
romm modules.

The audit did not open the production database. Cargo check was compile-only
with an isolated target.

## 11. Import Pressure

The largest cluster is the archivefs_core import block around lines 320–350:
database, config, source, library-view, mount, scan, health, diagnostics, and
platform APIs all enter main.rs together. The patch_manager import block around
lines 45–124 is the second largest and pulls cheat/mod, provider, preview,
transaction, profile, and catalogue symbols into the composition root.

Wildcard imports are also significant: administration_pages, sources_page,
platform_source_actions, selected_evidence_pipeline, romm, and page modules
bring feature namespaces into main.rs. navigation.rs still uses super::* and
source_controller.rs deliberately re-exports the old source protocol. Narrow
imports are worthwhile but should be a separate mechanical cleanup.

Isolated cargo check confirmed unused moved imports in main.rs, including
HISTORY_LIMIT, database-load helpers/types, and library-view/setup/core
symbols. This is concrete stale-namespace evidence.

## 12. Collision Heatmap

**HOT**

- update and ui;
- ArchiveFsApp fields and constructor;
- source/DAT scan-summary/database-reload handoff;
- mount operation polling and confirmations.

**WARM**

- selected-game/readiness/emulator setup;
- catalogue/cheat/mod polling;
- archive inspection/preparation;
- health/doctor/repair handoff;
- import/module declaration block.

**COLD**

- clipboard helpers;
- GUI mode parsing;
- scan wording;
- metric cards;
- small test helpers.

Since 2026-08-01, files co-changing with main.rs include tests/mod.rs (54
commits), doctor_and_repair tests (31), emulator_profiles_and_setup tests (27),
dat_sources_page.rs (24), navigation.rs (19), launch_readiness_page.rs (16),
sources_page.rs (15), home_page.rs (14), and administration_pages.rs (12).
The raw main.rs path count was 218; it is not a normalized semantic measure.

## 13. Change Coupling

Strongest coupling is main.rs with the GUI test harness, doctor/setup tests,
DAT sources, navigation/home, and launch-readiness/gamer-view surfaces. This
shows the shell is still reducing action effects and selected-game state rather
than merely composing pages.

Recent PS2/storage-health co-change is lower than source/DAT and doctor/setup
coupling. Those pages remain important boundaries, but they are not the next
collision priority.

## 14. Dead Code Archaeology

The isolated cargo check completed successfully and reported:

- unused HISTORY_LIMIT in main.rs;
- unused CatalogueStats, CompletedScanSummary, DatabaseHealth, library-view,
  setup, and related core imports in main.rs;
- unused DatabaseLoadResult, DatabaseMessage, classify_unhealthy_database,
  load_database_snapshot_at, load_database_snapshot, and load_snapshot_from;
- EmulatorDownloadPageState::show is unused;
- ReadyToPlayPageState::set_original_controls is unused;
- core warnings for unused imports and config_dir_in/data_path_in.

Module-level allow(dead_code) markers occur on es_de_media_state,
launchbox_local_state, platform_artwork_manager, bios_projection_page,
onframe_install_session, onframe_install_state, dat_sources_page,
feature_discovery, launch_readiness_page, gamer_artwork, museum_page,
repair_review_page, romm_source, selected_evidence_page, and tape_analysis_page.
These are markers for proof work, not deletion proof: several modules are
staged integrations or public(crate) page seams.

Dead-code candidate groups: 16, counting the 14 allowance families and the two
compiler-confirmed GUI methods. Confirmed safe deletions: none.

## 15. Test-Only Fossils

The main-file test module covers picker draining, clipboard output/shape
inspection, GUI mode and RetroArch override persistence, and a diagnostics
helper. Most are intentional seams.

Candidates requiring production-caller proof:

- shape_contains and picker_output_text_contains are visible test helpers;
- picker drain tests should be checked against the production picker owner;
- mode/override round trips prove real persistence and are not dead;
- file_pick_drain is production-relevant despite high test density.

Do not infer deadness from a test-only-looking name. Four orphan-test candidate
groups exist; none is confirmed orphaned.

## 16. Orphan Test Candidates

Candidate review list:

| Candidate | Evidence | Action |
|---|---|---|
| main-file picker shape/text helpers | local test use is visible | move only after global search |
| tests importing moved names via super::* | compatibility re-exports preserve old architecture | verify all test crates |
| doctor/setup tests coupled to main.rs | high co-change | move after typed controller events |
| legacy binary-target assumptions | three bins share main.rs | compile all bins before altering visibility |

No tests were changed.

## 17. Export / Module Dead Code

Round 2 modules are private with pub(crate) members, which is appropriate.
Public or pub(crate) modules including bulk_confirmation,
game_presentation, selection_guard, status_wording, view_mode, and staged page
modules need caller proof before export changes. The source facade's
unused-import allowance is deliberate and should not be removed before its
event seam exists.

No export was classified safe to delete. The strongest low-risk follow-up is
unused import cleanup.

## 18. Duplicate Implementations

1. Live LoadState and persisted DatabaseState have similar generation/receiver
   protocols but different data and semantics. Do not merge into a generic
   second loader; share only a small event/reducer convention.
2. Navigation mappings are extracted, but app adapters and some shell labels
   remain in main.rs. This is a partial boundary, not a duplicate model.
3. Source actions live in platform_source_actions and are re-exported by the
   intentionally thin source facade.
4. Database-load helpers remain imported in main.rs after moving bodies.
5. Readiness/identity explanations exist in core, pages, and selected-game
   adapters. The shell should consume typed projections and not add another
   mapping.
6. GUI mode and RetroArch override persistence are separate settings, not one
   generic persistence model.

No duplicate MAME/arcade completeness engine was found.

## 19. Legacy ArchiveFS Residue

Legacy naming is required in several places:

- core app_dirs deliberately falls back to ~/.config/archivefs and
  ~/.local/share/archivefs;
- Cargo deliberately exposes archivefs-gui beside emuwiz and emuwiz-gui;
- persisted/provider/core identifiers such as archivefs_path may be compatibility
  data.

Classification: old directories and binary alias are required compatibility;
internal names are rename debt; comments are harmless; route aliases need
route-by-route proof. Do not rename or retire them in this audit.

## 20. Repository / Fixture Junk

The baseline contains empty, untracked root files:

- 1234567890123456789012345678901
- fragmented.bin
- icon.sys
- zero.bin

No code directly references those root paths, but PS2 validation/research
documents explicitly use those names as synthetic fixture members. They are
therefore NEEDS PROOF, not SAFE TO DELETE. A future cleanup should identify
their producer, compare them with documented fixtures, move legitimate fixtures
under a fixture directory, and remove only confirmed generated artifacts.
This audit did not touch them.

## 21. Round 2 Module Quality

| Module | Lines | Assessment |
|---|---:|---|
| activity_history.rs | 354 | coherent independent model; keep |
| navigation.rs | 654 | substantial policy/shell; narrow super::* later |
| library_view_controller.rs | 412 | real action/worker protocol; keep |
| source_controller.rs | 35 | intentionally thin facade/dialog seam; keep |
| database_load.rs | 395 | coherent state/worker/snapshot owner; keep |
| setup_controller.rs | 273 | coherent typed setup protocol; renderer remains central |

None should be undone. Source controller thinness is intentional because
platform_source_actions still owns the worker vocabulary; it is a future
SetSourceRole seam, not a failed extraction.

## 22. Cargo Crate Candidates

| Candidate | Decision | Reason |
|---|---|---|
| Mods/cheats | MAYBE LATER | substantial core logic and network/cache workflows, but GUI contracts remain coupled; extract a GUI controller first |
| Save Vault/PCSX2 | NO | focused page plus core inventory/export API; no independent reusable crate evidence |
| Arcade/MAME | NO | authority/compatibility belongs in core; GUI consumes typed projections |
| DAT/identity | NO | core owns reusable authority/identity; GUI is presentation/orchestration |
| emulator management | MAYBE LATER | multiple consumers and substantial core APIs, but setup/launch state remains app-coupled |

No candidate currently meets enough criteria to justify a new crate.

## 23. Compile-Time Dependency Opportunities

Narrow the two broad core import blocks and wildcard page imports after event
boundaries are stable. Move live-load/row code, mount workers, provider
controllers, and selected-game adapters behind explicit imports. This may
improve incremental builds and will materially improve reviewability, but the
dominant GUI compile cost remains eframe and the core crate. Do not create a
crate solely to move imports.

## 24. Runtime / Per-Frame Performance Smells

update calls many poll_* functions every frame. Most idle polls only try_recv,
so this is not proof of a performance regression. The main risks are:

- too many generation/reconciliation checks;
- dat_authority.tick in the frame path;
- temporary row/filter vectors in loading/preview render branches;
- accidental future filesystem/scan work added to update;
- repeated page projection if a feature bypasses the cached snapshot pattern.

Positive evidence: health reports are keyed/cached, and merged rows/filtered
indexes are recomputed on load/database completion rather than blindly each
frame.

The most expensive suspected per-frame work is the scheduler plus page
composition, not a measured hot loop. No benchmark was run.

## 25. MUST EXTRACT

1. Live library refresh/load controller: LoadState/Result/Message, start/load/poll,
   row rebuild and selection-pruning event boundary.
2. Mount/operation controller: ArchiveAction, progress/results, cleanup,
   queues, confirmations and recovery.
3. Archive inspector/preparation controller: worker protocol, selection and
   safe prepared-member transitions.
4. Selected-game/readiness adapter: discovery context, museum projection,
   identity review, emulator-setup routing, game-details renderer.
5. RomM operation controller: provider snapshot/operation/verification/cache
   orchestration using existing romm modules.

These are feature-specific/high-collision and should be separate commits.

## 26. COULD EXTRACT

Seven lower-priority regions:

- row models/filter/layout helpers;
- duplicate and health cache/filter state;
- source/DAT renderers after typed effects;
- BSFree/catalogue/Dolphin catalogue controller;
- artwork picker/task state;
- clipboard/context-menu and GUI mode helpers;
- doctor/repair rendering after action effects are typed.

They are useful boundaries but some are mostly presentation and should not be
split into tiny wrapper files.

## 27. SHOULD STAY

Keep bootstrap, ArchiveFsApp construction while bundles are introduced, frame
ordering, one page-dispatch match, global navigation adapters, overlay
arbitration, global feedback/history sinks, refresh requests, and cross-feature
effect reduction. The shell owns ordering and global effects; it should not own
a feature's worker implementation.

## 28. Safe Delete / Needs Proof / Keep

**SAFE TO DELETE NOW:** none.

**NEEDS PROOF:** unused imports; two compiler-dead GUI methods; all
allow(dead_code) families; four orphan-test groups; four root artifacts; stale
re-exports and super::* imports; old route aliases; public exports.

**KEEP:** legacy config/data fallback, archivefs-gui compatibility alias,
synthetic fixture names until provenance is known, Round 2 modules, typed
readiness/DAT/MAME/save boundaries, and safety-focused tests.

## 29. Next Extraction Commit Plan

| Commit | Slice | Estimated reduction | Risk/dependency |
|---|---|---:|---|
| 1 | extract live library load controller | 350–650 | MEDIUM; immediate next slice; generation/row/selection tests |
| 2 | extract archive inspector controller | 450–750 | HIGH; explicit snapshot context first |
| 3 | extract mount operation controller | 700–1,100 | HIGH; mount/recovery/progress tests |
| 4 | extract selected-game coordinator | 350–650 | MEDIUM/HIGH; after live snapshot context |
| 5 | isolate catalogue and RomM effects | 500–900 | HIGH; event sink first |
| 6 | narrow moved-module imports | 100–250 apparent cleanup | LOW; separate mechanical commit |
| 7 | remove proven GUI fossils | variable | only after caller proof |

Commit 1 can proceed without Source Role work or the dirty storage-health/core
files. Do not combine it with database_load.rs, mount operations, row cleanup,
or UX changes.

## 30. Dead-Code Cleanup Plan

After structural extraction, record warnings per binary, build a production
caller map for each dead_code family, move test-only helpers where justified,
remove stale imports/re-exports mechanically, and then delete one proven
orphan at a time with all GUI bins and focused tests. Handle root fixtures in a
separate provenance cleanup. Do not mix deletion with worker/controller moves.

## 31. Final main.rs Size Estimate

| State | Estimate |
|---|---:|
| current | 12,883 |
| after MUST EXTRACT | 8,800–10,000 |
| after COULD EXTRACT | 7,000–8,500 |
| healthy single-file architecture | 7,000–9,000 |

5,000 would require moving much of page composition; 3,000, 1,500, and 500
are not realistic without relocating the application to app.rs.

## 32. main.rs to app.rs End-State Analysis

There is no current app.rs or lib.rs. The eventual split is sound:

- main.rs: logging, native entrypoint, eframe run, compatibility wrappers;
- app.rs: ArchiveFsApp, top-level update/ui composition, effect reducer;
- feature modules: page state, controllers, workers and adapters;
- archivefs-core: reusable domain policy and persistence.

Recommendation: yes, eventually, but only after live-load, inspector, mount,
and selected-game seams are stable. Moving ArchiveFsApp wholesale now would
create a large collision move. A future main.rs can be 100–300 lines while
app.rs remains several thousand.

## 33. Binary Target Simplification

Cargo defines emuwiz, emuwiz-gui, and archivefs-gui with the same src/main.rs.
Cargo warns that one source is present in multiple targets. Keep the aliases
for compatibility now. Later use one shared app entrypoint plus thin wrappers,
then test packaging/launchers before retiring the legacy alias.

## 34. Round 3 Scorecard

| Metric | Result |
|---|---:|
| main.rs lines | 12,883 |
| top-level declaration/impl sites | 238 |
| named top-level declarations | 204 |
| impl blocks | 34 |
| method/function sites | 120 |
| ArchiveFsApp fields | 234 |
| ArchiveFsApp methods | 76 |
| spans over 30 lines | 39 |
| spans over 100 lines | 12 |
| large methods over 200 lines | 4 |
| major dialog/action/worker families | 9 |
| MUST EXTRACT regions | 5 |
| COULD EXTRACT regions | 7 |
| dead-code candidate groups | 16 |
| orphan-test candidate groups | 4 |
| crate candidates investigated | 5 |

Hottest collision area: ArchiveFsApp fields plus update/ui. Most expensive
suspected per-frame area: poll/reconcile scheduling plus page composition; not
measured.

## 35. Recommended Immediate Next Task

Implement exactly one slice:

refactor(gui): extract live library load controller

Move only LoadState, LoadResult, LoadMessage, start_load, load_data, poll_load,
and the minimal typed result/effect boundary for merged-row rebuilding and
selection pruning. Preserve the distinction between live archive-index evidence
and persisted catalogue evidence.

Focused tests should cover initial load, generation mismatch, disconnected
worker, previous snapshot retention, row/filter rebuild, selection pruning,
and failure feedback. Do not combine this with database_load.rs, mount
operations, row-model cleanup, or UX changes.

## Validation Record

The isolated command
CARGO_TARGET_DIR=/tmp/emuwiz-mainrs-round3-target cargo check -p archivefs-gui
completed successfully. It emitted the known multiple-bin warning and the
unused/dead-code warnings recorded above. No production database, catalogue,
scan, or live GUI was opened.

After writing this document, run git diff --check and the repository postcheck
with only this document allowed. Commit only this audit document.
