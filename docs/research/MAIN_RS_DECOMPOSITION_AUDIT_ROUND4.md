# MAIN.RS Decomposition Audit — Round 4

Audit date: 2026-09-15  
Worktree: /home/davedap/emuwiz-main-release-fix  
Starting revision: 399e488cfb836e156169f2306fd9be2779044a23  
Scope: read-only forensic architecture, state-blob, dead-code, crate-boundary, and performance audit. No production Rust was modified.

## 1. Executive Summary

Round 2 and Round 3 removed 4,444 lines from main.rs, reducing it from 14,629 to 10,185 lines. The ten focused extractions are structurally sound. The remaining size is now concentrated in the aggregate application state and the eframe shell rather than in missing worker boundaries.

The current root has 234 ArchiveFsApp fields and 76 methods. update is approximately 2,556 lines and ui approximately 2,154 lines. They poll or compose almost every subsystem: library rows, sources, DAT, doctor, emulators, cheats/mods, mounts, evidence, artwork, RomM, and repair. Several page bodies and action reducers are still embedded in the root.

The best next step is state encapsulation, followed by typed effect reduction and page ownership. The recommended immediate slice is LibraryUiState: a focused bundle for library presentation, selection-facing filters, platform/alias actions, and related dialog/action state. It offers field and collision reduction at moderate risk without duplicating database_load.rs or moving destructive operations.

A future app.rs split is recommended. A 100–300-line main.rs is feasible only after ArchiveFsApp, update, and ui are relocated to app.rs. A healthy app.rs is realistically 2,800–4,200 lines. No new Cargo crate is justified now.

## 2. Cleanup Campaign Progress

| Stage | main.rs lines | Result |
|---|---:|---|
| Round 2 starting point | 14,629 | monolithic coordination root |
| After Round 2 | 12,883 | activity, navigation, library-view, source, database, setup seams |
| After Round 3 MUST slices | 10,185 | live load, mount, inspector, selected readiness, RomM worker family |
| Current reduction | 4,444 | 30.4% |
| Current ArchiveFsApp fields | 234 | unchanged by the RomM worker extraction |
| Current ArchiveFsApp methods | 76 | unchanged by the RomM worker extraction |

HEAD 399e488 is the requested RomM extraction. Preflight found a clean worktree.

## 3. Current main.rs Anatomy

| Region | Approx. lines | Responsibility | Assessment |
|---|---:|---|---|
| imports/module declarations | 1–400 | imports almost every core and GUI subsystem; all three GUI bins include this source | primary coupling hotspot |
| row/layout models | 401–1,360 | ArchiveRow, LoadedData, filters, column policy, bulk gates | existing library owners should absorb this gradually |
| evidence/load/config policy | 1,017–1,920 | refresh generations, selected-evidence wrappers, health/config helpers | mixed pure policy and app adapters |
| route and shell policy | 1,422–1,920 | view/tab enums, labels, title/width/scroll | mostly already extracted to navigation.rs |
| ArchiveFsApp declaration | 1,923–2,632 | 234 fields for all pages, workers, dialogs and caches | largest coupling domain |
| construction/navigation | 2,633–3,060 | defaults, worker initialization, route synchronization | eventual app.rs; constructors should call bundle defaults |
| live/database/diagnostic reducers | 3,061–3,452 | refresh, DB installation, health cache, setup polling | typed effects and feature contexts |
| source/DAT/catalogue/library-view actions | 3,518–4,350 | page adapters and polling | high collision; existing owners should absorb |
| repair/preview/profile persistence | 4,350–4,710 | missing removal, plan preview, remembered profiles | repair/emulator owners |
| artwork and update scheduler | 4,729–5,950 | artwork worker and update function | keep ordering, delegate polling families |
| eframe UI shell | 7,420–approximately 9,400 | menus, sidebars, overlays, global feedback, page dispatch | app.rs shell plus page renderers |
| remaining page helpers | approximately 7,695–9,400 | setup, activity, doctor, inspector, mount, selected, artwork | page-specific candidates |
| clipboard/health/mode helpers | approximately 9,400–9,925 | reusable UI utilities and settings persistence | existing UI/settings owners |
| tests | approximately 9,926–10,185 | picker, mode, clipboard and helper tests | move only with caller proof |

## 4. ArchiveFsApp Field Inventory

Every declared field is accounted for in the following ownership groups. Names are retained for an auditable inventory; wrapped declarations such as the Dolphin catalogue result/check fields remain in their original types.

| Owner | Fields |
|---|---|
| live library | state, filter, filtered_rows, archive_context |
| mount operations | operation, mount_all, unmount_all, confirm_mount_all, focus_mount_all_cancel, mount_all_result, mount_queue, mount_search, confirm_mount_queue, active_mounts_confirm_unmount, confirm_unmount_all, focus_unmount_all_cancel, confirm_unmount_selected, focus_unmount_selected_cancel, unmount_all_result, confirm_unmount, confirm_lazy_unmount, confirm_lazy_unmount_final, focus_lazy_cancel, focus_final_lazy_cancel, lazy_unmount_offers, remount_offers, cleanup_after_unmount, mount_all_typed_count, unmount_all_typed_count, confirm_mount_selected, focus_mount_selected_cancel, mount_selected_typed_count |
| global history/rollback | history_filters, shared_history, shared_history_operation, shared_rollback, history |
| database restore | database_restore_plan, database_restore_confirmation, database_restore_feedback |
| emulator/readiness | retroarch_profiles, retroarch_core_directory_override, retroarch_core_folder_rejected_pick, emulator_setup_focus, emulator_setup_page, emulator_inventory_page, bios_projection_page, ready_to_play_page, emulator_setup_overrides, pcsx2_profiles, dolphin_profiles, dolphin_local_profiles, pcsx2_launch_profiles, flycast_profiles, pcsx2_firmware_evidence, xenia_profiles, remembered_emulator_profiles |
| tape and launch | tape_inspector_filter, launch_retroarch, launch_dolphin, launch_pcsx2, launch_standalone, launch_amiga_whdload |
| cheats/mods | cheat_workflow, user_cheat_import_page, dolphin_texture_mod, local_mod_package, cheat_archive_picker, confirm_cheat_archive_change |
| global feedback | feedback |
| setup/doctor | diagnostics, config_previously_confirmed, onboarding_state, onboarding_auto_open_checked, doctor_scan, doctor_scan_generation, doctor_selected_finding, doctor_repair_review, doctor_repair_result, doctor_repair_finished_at_unix_seconds, setup_action |
| emulator probes | rpcs3_status, rpcs3_status_generation, pcsx2_status, pcsx2_status_generation, pcsx2_status_archive_path |
| source/DAT/media | cheat_sources_page, cheat_sources_ui, dat_sources_page, dat_sources_ui, media_sets_page, quick_rename_mode, source_action, mount_root_draft, mount_root_feedback |
| BSFree/catalogues | bsfree_manager, bsfree_operation, bsfree_ui, catalogue_manager, catalogue_review, catalogue_retrieval, catalogue_generation, catalogue_last_result, dolphin_catalogue_manager, dolphin_catalogue_review, dolphin_catalogue_retrieval, dolphin_catalogue_generation, dolphin_catalogue_last_result, dolphin_catalogue_remove_confirm, dolphin_catalogue_update_available, dolphin_catalogue_update_check |
| refresh/database projection | refresh_error, snapshot_stale, refresh_generation, snapshot_generation, database_state, database_generation, needs_attention, dat_authority, pending_source_scan_summary, sources_last_scan |
| library actions/presentation | library_filters, library_platform_query, platform_action, platform_choice, platform_custom_text, alias_action, missing_removal, confirm_remove_missing, new_alias_text, new_alias_platform_choice, bulk_platform_action, bulk_platform_choice, sort_field, sort_ascending, library_scroll_offset, library_source_filter, library_column_widths |
| duplicate/health | duplicate_filters, duplicate_sort_field, duplicate_sort_ascending, selected_duplicate_group, selected_duplicate_archive, health_filters, health_sort_field, health_sort_ascending, selected_health_issue, diagnostics_refresh_generation, health_report_cache |
| shell/navigation | view, library_tab, problems_repair_tab, sources_tab, tools_overlay, show_activity, show_about, show_skipped_files, skipped_files_filter, select_all_visible_requested, ui_mode, gamer_view_screen |
| RomM | gui_config, romm_snapshot, verify_romm_summary, romm_operation, romm_generation, romm_ui, romm_config_draft, romm_preview, romm_browse, romm_stale_progress, romm_game, romm_hash_progress |
| selected evidence/plan | selected_evidence, selected_evidence_generation, selected_evidence_cancel, selected_evidence_enrichment, no_intro_source_cache, identity_sources, identity_sources_generation, scummvm_readiness, scummvm_check, scummvm_check_generation, plan_preview, plan_preview_generation |
| library views and source dialogs | sources_add_dialog, gamer_view_pending_first_scan, gamer_view_scan_review_available, gamer_view_scan_pending_review, sources_remove_dialog, library_views, library_view_action, library_view_last_plan, library_view_form_dialog, library_view_remove_dialog, library_view_focus_archive, library_view_plan_filter |
| archive inspection | archive_inspector, archive_inspector_generation, archive_preparation, archive_preparation_generation |
| artwork/media/museum | custom_platform_artwork_directory, platform_artwork_cache, platform_artwork_manager, platform_artwork, gamer_covers, gamer_screenshots, museum_page, museum_hero, gamer_cover_worker, gamer_cover_worker_allowed, gamer_cover_library, selected_game_metadata, game_metadata_worker, game_metadata_worker_allowed, gamer_alpha_jump, es_de_media, launchbox_local_media |

This is a field ownership inventory, not a recommendation to create 19 state
objects. The first migration should combine only the strongest lifecycle
clusters.

## 5. Field Cluster Analysis

| Cluster | Approx. fields | Methods/regions | Risk | Proposed state | Slot reduction |
|---|---:|---|---|---|---:|
| mount/operation | 27 | handle_mount_page_action, update, ui, operation reducers | HIGH | MountUiState | 26 |
| emulator/readiness | 22 | setup, profile polls, BIOS/readiness and launch | HIGH | EmulatorReadinessState | 21 |
| RomM | 12 | romm controller calls and Sources render | MEDIUM | RommUiState | 11 |
| catalogue/BSFree | 17 | catalogue start/poll and Cheats & Mods render | HIGH | CatalogueState | 16 |
| source/DAT | 15 | Sources/DAT rendering and source polling | HIGH | SourcesUiState | 14 |
| selected evidence/plan | 11 | selected workers and Selected page | MEDIUM/HIGH | SelectedEvidenceUiState | 10 |
| health/duplicates | 10 | health/duplicate pages and cache | MEDIUM | HealthAndDuplicateUiState | 9 |
| library presentation/action | 17 | rows, filters, platform/alias actions and table UI | MEDIUM | LibraryUiState | 16 |
| inspector/preparation | 4 | inspector polling and detail panel | MEDIUM | ArchiveInspectionState | 3 |
| artwork/media | 16 | artwork poll, Museum, Gamer View and metadata | HIGH | ArtworkMediaState | 15 |
| doctor/repair | 13 | diagnostics, doctor, repair and removal | HIGH | DoctorRepairState | 12 |
| shell/global | 16 | construction, update, ui and navigation | LOW | retain in shell | 0 |

Some fields cross lifecycle boundaries, especially database/source refresh
fields, so the table is a coupling scorecard rather than a disjoint sum.
A bundle should use private members and focused contexts, not expose the old
field list publicly.

## 6. God-Object Method Analysis

The 76 app methods fall into global coordination, feature coordination,
rendering, persistence, database, worker, navigation, and utility groups.
The highest-risk methods are:

| Method | Approx. lines | Subsystems | Decision |
|---|---:|---:|---|
| new | 267 | all state families, config and workers | retain initially; delegate bundle constructors |
| update | 2,556 | all workers, DB/DAT tick, invalidation, repaint | retain ordering in app shell; delegate polling |
| ui | 2,154 | shell, menus, every page and action effects | retain composition; move page bodies |
| show_sources_libraries_tab | 228 | sources, mount root, catalogue, BSFree, RomM, history | move to Sources owner in stages |
| show_dat_sources_page_mode | 100–120 | DAT page, authority, quick rename, history | move to dat_sources_page |
| poll_database_load | 150 | DB result, rows, duplicates, health and selection | reducer should return typed effects |
| poll_catalogue_manager | about 100 | catalogue, feedback/history, refresh | existing cheats/mods controller |
| persist_remembered_profile | 110 | config filesystem, feedback/history | emulator setup owner |
| show_selected_page | 110+ | evidence, readiness, launch, cheats and metadata | selected-game owner |
| show_archive_inspector_panel | 170+ | inspector state and clipboard | inspector page owner |
| start/poll platform artwork | 90+ combined | worker, cache and media state | artwork owner |

The unrestricted receiver is the core smell. Methods touching five or more
feature clusters are new, update, ui, show_sources_libraries_tab, and
poll_database_load. They should become shell calls with focused inputs/effects.

## 7. Update/UI Hotspot Analysis

update performs reconciliation, history polling, live/database loading, DAT
authority ticking, diagnostics, setup/doctor, emulator probes, source and
catalogue workers, library views, inspector, evidence, profiles, cheat
workflow, and managed downloads. It also contains view-gated automatic starts
and repaint scheduling.

The ordering is legitimate and should not be flattened casually. The boundary
should become:

update → poll typed feature state → reduce explicit AppEffect → repaint.

A small AppEffect is justified after the first state bundle. Its initial cases
should be Feedback, History, ReloadDatabase, RefreshLibrary, Navigate, and
RequestRepaint. Feature payloads and egui widgets should not enter the enum.

ui mixes menus/sidebar, overlay arbitration, feedback/activity, Gamer View
action translation, page dispatch, page rendering, and action starts. The one
page-dispatch match may remain in the shell; the large page bodies and action
reducers around it should move to feature owners. There is no full-frame UI
snapshot suite, so each extraction needs focused state/action tests.

## 8. Page Ownership

| Surface | Classification | Main residue | Destination |
|---|---|---|---|
| Home | A/B | builds inputs and routes HomeCard | home owns view; shell adapter remains |
| Needs Attention | A | calls page and routes destination | already clean |
| Sources/Libraries | C | 228-line composition of source, mount-root, catalogue, BSFree and RomM cards | sources_page plus source/controller effects |
| Sources/DAT | B/C | mode rendering and action translation | dat_sources_page |
| Sources/Cheats/Discovery | A/B | modules render most content; main translates actions | keep thin adapter |
| Library/Archives | C | filters, rows, health/duplicates, views and actions | LibraryUiState then library owner |
| Library Views | B | availability/reload/poll still in app | library_view_controller effect API |
| Ready-to-Play | A/B | page separate; app builds selected/readiness inputs | selected adapter, not core |
| Emulator Setup/Inventory | B | profile orchestration and page state | emulator setup controller |
| BIOS/Firmware | A/B | projection page exists; state/probe routing in app | readiness bundle |
| Cheats & Mods | C | large render/action branch and catalogue workers | existing cheats_mods tree |
| Mounts/Active Mounts | C | operation state and confirmation render | existing mount modules |
| Selected | C | evidence/readiness/launch wiring | selected-game owner |
| Archive Inspector | B | controller exists; panel render remains | inspector owner |
| History & Logs | B | model extracted; panel/rollback presentation remains | history page plus shell sink |
| Problems & Repair | C | doctor, removal, restore and repair mixed | doctor/repair owner |
| Storage Health | B | cache/filter/cards remain in main; dirty-file ownership matters | defer until boundary is free |
| Museum/Gamer | B | page renders; selected/artwork/worker setup in app | artwork/media bundle |
| Media Sets/Tape | A/B | renderer mostly external; state remains in app | later page bundle |
| RomM | B | UI controller and worker module now separate | keep; bundle fields later |
| Settings/About | A/B | shell and mode persistence | settings owner after app split |

## 9. Rendering Candidates

Highest-value candidates, all requiring behavior-preserving effect seams:

1. show_sources_libraries_tab, about 228 lines. Move source list, scan
   controls, mount-root card, catalogue/BSFree cards and RomM card composition
   to the existing Sources owner.
2. show_dat_sources_page_mode, about 100–120 lines. The DAT page already owns
   most domain rendering.
3. show_selected_page, about 110+ lines. Consume selected evidence/readiness
   projections in the selected-game owner; launch execution stays where it is.
4. show_archive_inspector_panel, about 170 lines. Move egui rendering to the
   existing inspector module once state/effect inputs are explicit.
5. show_mount_queue_confirmation, about 60 lines. Move with mount queue page,
   retaining typed-count safety gates.
6. show_setup_diagnostics, about 150 lines. Move rendering to setup/doctor page;
   setup_controller already owns workers.
7. show_activity_panel, about 170 lines. Move presentation while retaining
   global history storage.
8. health metric cards plus duplicate/health filter helpers. Move to existing
   health/duplicate owners when storage_health_page ownership is available.

Do not create a generic render module or dialogs.rs.

## 10. Dialog/Modal Ownership

Feature-owned state remains for source add/remove, library-view form/remove,
mount all/selected/unmount/lazy/queue confirmations, cheat archive selection,
doctor repair, database restore, DAT/cheat drafts, and skipped/selection
dialogs. RomM state/controller is already separate from the root's worker
helpers.

Truly shell-owned overlays are About, skipped-files, and overlay arbitration.
The other dialogs should travel with the owning page/state bundle. The key
duplication is interleaving page rendering and action reduction in ui, not
the existence of a single dialog type.

## 11. Action Polling / Effects

The repeated pattern is poll worker → check generation/result → update feature
state → set feedback → record history → reload database/library → navigate.
It occurs in sources, catalogues, library views, removal, mount, setup, evidence,
RomM and emulator paths.

A small AppEffect reducer should be introduced only after one focused state
bundle proves the boundary. Preserve existing wording and outcome policy.
Do not make every click an effect or move feature payloads into the global
enum.

## 12. Feedback/History Duplication

At least eight worker families write feedback and at least seven record
activity in their reducers. The exact message wording differs by feature;
RomM has special offline/result semantics and must not be mechanically merged
with generic operation wording.

Centralize only the sink reduction first: accept typed feedback/history effects
and apply them in one shell location. Do not centralize wording or couple
feature controllers to egui toast widgets.

## 13. Selected-Game / Evidence Wrapper Cleanup

The four wrappers are:

- gather_selected_evidence_with_registry
- gather_selected_evidence_with_registry_and_platform
- gather_selected_evidence_with_registry_at
- gather_selected_evidence_with_registry_at_and_platform

They differ by registry, platform, and timestamp inputs. The at variants support
deterministic/replay-style callers. They are near-duplicates, not proven dead;
a later private input struct can preserve all four seams while removing repeated
plumbing.

## 14. Per-Frame Performance

| Work | Frequency | Cost/risk | Cache/invalidation key |
|---|---|---|---|
| receiver polling and scheduler branches | every frame | mostly cheap try_recv; broad control complexity | worker generation |
| dat_authority.tick | relevant views every frame | registry freshness/projection work | DAT generation + view |
| page composition/visibility | every frame | large egui tree and transient allocations | page/state generation |
| Gamer/Museum delivery drain | relevant frames | clone/texture work | artwork generation |
| user_cheat_library construction | Cheats & Mods frames | scales with live record count | DB snapshot generation |
| selected readiness/detail projection | Selected frames | evidence and recommendation projection | selected path + evidence generation |
| health/duplicate refresh calls | page/snapshot changes | possible report rebuilding | DB snapshot + filter generation |
| RomM card preparation | Sources frames | smaller; progress/result clones | RomM operation generation |

No unconditional scan, hash, network request, DB open, or TOML/JSON parse was
found in update. Worker starts are mostly view-gated. The largest suspected
cost is shell/page composition and collection projection, not a measured hot
loop.

## 15. Clone/Allocation Hotspots

Cheap and required clones include egui Context, worker-owned PathBufs, and
small operation identifiers. Potentially expensive allocations include the
full user_cheat_library vector, workflow metadata clones, artwork reply clones,
source-root vectors for workers, and repeated selected/readiness display data.

Likely avoidable later are library row/filter vectors when both snapshot
generation and filters are unchanged, and selected-game projections when
selection/evidence generation is unchanged. Do not remove worker ownership
clones without changing channel contracts. Add generation assertions or
measurements before optimizing.

## 16. Cache Opportunities

Candidate caches are:

- visible library indexes keyed by live/database generation and filter identity;
- selected readiness keyed by selected archive, evidence generation, and
  emulator/BIOS inventory generation;
- DAT projections keyed by DAT authority and database generation;
- health reports keyed by existing snapshot/config/diagnostic identity;
- RomM card data keyed by snapshot/operation generation;
- artwork keyed by worker/library/config identity;
- mount queue eligibility keyed by archive snapshot and queue filter.

Every input affecting visible output must be in the key. Unknown/partial
evidence must never be cached as Missing or another negative result.

## 17. Dead Code Archaeology Round 2

No item is proven safe to delete from source inspection alone. The three GUI
binary targets, public/test seams, callbacks, and feature-gated modules make
compiler warnings necessary but insufficient.

Sixteen candidate groups remain:

1. stale imports for moved database/setup/library-view/operation symbols;
2. HISTORY_LIMIT and default inspector-width imports;
3. unused page methods reported by cargo check;
4. allow(dead_code) modules and variants retained for compatibility/fixtures;
5. embedded Sources navigation aliases;
6. public GUI modules with alternate-target/test consumers;
7. fixture-looking files if they reappear;
8. ArchiveFS compatibility names;
9. duplicate action-summary wrappers;
10. overlapping source/platform label helpers;
11. recovery helpers behind legacy routes;
12. test-only picker/clipboard seams;
13. RomM/page adapters after worker extraction;
14. health/duplicate filter helpers;
15. stale re-exports and wildcard-import namespaces;
16. comments/temporary compatibility scaffolding describing removed architecture.

### Safe Delete

None proven safe now.

### Needs Proof

All 16 groups above. For each candidate, map production caller → symbol →
tests, then check all three GUI binaries and public API concerns.

### Keep

Round 2/3 controllers, typed readiness/DAT/MAME/save boundaries, safety tests,
legacy config/data fallback, and binary compatibility aliases remain live.

## 18. Orphan Tests

Four groups remain candidates for caller mapping:

1. picker/mode/clipboard tests at the end of main.rs;
2. selected-context/BSFree tests after selected adapter extraction;
3. RomM transaction tests after worker/controller extraction;
4. tests asserting legacy navigation aliases.

The RomM-focused run after the extraction passed 318/318 tests, so those are live
regressions, not deletion candidates. Test-only helpers are intentional until
the production/test seam is documented.

## 19. Legacy Residue

| Residue | Classification |
|---|---|
| archivefs-gui binary | KEEP FOR COMPAT; explicitly configured alias |
| emuwiz and emuwiz-gui | KEEP during rename; later thin wrappers |
| ArchiveFS internal names/comments | NEEDS PROOF; separate rename debt from data compatibility |
| legacy config/database fallbacks | KEEP FOR COMPAT |
| embedded Sources route aliases | NEEDS PROOF; live route tests remain |
| stale roadmap comments | retire only in documentation cleanup |

## 20. Round 2/3 Module Quality

| Module | Lines | Assessment |
|---|---:|---|
| activity_history.rs | 354 | clear independent model |
| navigation.rs | 654 | broad but coherent policy |
| library_view_controller.rs | 412 | coherent action/state seam |
| source_controller.rs | 35 | intentionally thin future role seam |
| database_load.rs | 395 | persisted DB loading owner |
| setup_controller.rs | 273 | focused setup worker/state |
| live_library_controller.rs | 115 | small reducer seam |
| mount_operation_controller.rs | 476 | focused safety controller |
| archive_inspector_controller.rs | 502 | coherent worker/state owner |
| selected_game_readiness.rs | 447 | adapter, not second readiness engine |
| romm_operation_controller.rs | 1,246 | large but coherent provider worker family |

No module should be merged merely because it is small. The next concern is
unrestricted field access from main.rs. The 1,246-line RomM module is a
bounded provider worker family, not a generic state bag.

## 21. Large Module Audit

Largest GUI files by bytes include:

- dat_sources_page.rs — 524,330;
- main.rs — 471,748;
- administration_pages.rs — 174,787;
- user_cheat_import_page.rs — 117,035;
- launch_readiness_page.rs — 116,503;
- library_view.rs — 114,817;
- sources_page.rs — 114,372;
- gamer_view.rs — 112,362;
- selected_evidence_page.rs — 104,265;
- playing_library_page.rs — 89,814.

Do not replace one god-file with another. Extend an existing owner only when the
moved logic is genuinely that page's responsibility.

## 22. app.rs End-State

The safe migration is:

1. create app.rs containing ArchiveFsApp and its impl blocks;
2. keep main.rs as crate root and bootstrap temporarily;
3. re-export only compatibility items needed by child modules/tests;
4. establish explicit feature contexts before changing privacy;
5. convert the three Cargo targets to thin wrappers only after one shared
   entrypoint is proven.

The principal risk is privacy: current child modules use ancestor-private imports,
including some broad imports. Moving the type changes paths and can expose
accidental coupling. app.rs relocates complexity; it does not reduce it by
itself.

## 23. Tiny main.rs Feasibility

A 100–300-line main.rs is feasible as a final binary entrypoint containing
logging, icon/options, window configuration, ArchiveFsApp construction, and
eframe::run_native. It is not feasible while main.rs owns ArchiveFsApp,
update/ui, and the current private compatibility surface.

## 24. ArchiveFsApp State Split Plan

| State object | Fields/lifecycle | Owner | Reduction | Risk |
|---|---|---|---:|---|
| LibraryUiState | filter, filtered_rows, library_filters/platform query, sort, scroll, source filter, column widths, platform/alias action fields and dialogs | library owner | 15–18 to 1 | MEDIUM |
| RommUiState | snapshot, verify summary, operation/generation, card/config/preview/browse/game/progress | romm tree | 12 to 1 | MEDIUM |
| HealthAndDuplicateUiState | health/duplicate filters, selections, sorts, cache and refresh generation | existing page owners | 10 to 1 | MEDIUM |
| SelectedEvidenceUiState | selected workers, caches, identity sources, plan preview and generations | evidence/plan modules | 11 to 1 | MEDIUM/HIGH |
| MountUiState | operation workers, queues, confirmations, offers and typed counts | mount modules | 27 to 1 | HIGH |
| EmulatorReadinessState | profiles, setup pages, BIOS evidence, launch adapters, remembered profiles | emulator setup/readiness | 22 to 1 | HIGH |
| DoctorRepairState | diagnostics, doctor scan/review/result, removal/restore state | setup/repair | 13 to 1 | HIGH |
| CatalogueState | BSFree plus RetroArch/Dolphin catalogue managers, review/retrieval/update state | cheats/mods | 17 to 1 | HIGH |

These are migration targets, not a request for one mega-state. Keep members
private and expose focused contexts/effects.

## 25. ArchiveFsApp Method Split Plan

1. Introduce LibraryUiState accessors and library action translation; keep DB
   reload and global navigation in the shell.
2. Move Sources/Libraries rendering/action reduction into existing sources
   owners.
3. Move catalogue/BSFree polling into the existing cheats_mods tree.
4. Move doctor/repair result reduction behind typed repair effects.
5. Move mount page rendering/action handling into mount modules.
6. Move selected page rendering and readiness action projection.
7. Move artwork/media worker lifecycle.
8. Then relocate ArchiveFsApp, update, and ui to app.rs.

The shell should retain ordering, one page-dispatch match, modal arbitration,
global feedback/history sinks, and cross-feature effect reduction only.

## 26. Crate Candidates Round 2

| Candidate | Decision | Reason |
|---|---|---|
| RomM | MAYBE LATER | coherent provider domain, but current GUI orchestration has no second consumer |
| Save Vault/PCSX2 | MAYBE LATER | coherent safety domain, limited consumers and GUI boundary |
| Arcade/MAME | NO | typed authority/dependency/completeness already belongs in core |
| DAT/authority | MAYBE LATER | reusable core domain, but current core ownership is adequate |
| Emulator management | NO | setup state is GUI-oriented; reusable probes already live in core |
| Cheats/Mods | MAYBE LATER | substantial domain, but patch-manager core is existing reusable boundary |
| generic app/effects | NO | artificial public API and increased coupling |

No YES NOW candidate meets the independent-consumer and compile-time-benefit
threshold.

## 27. Compile-Time Opportunities

Narrow broad core and wildcard page imports after effect boundaries stabilize.
Move pure row/filter models into existing library owners. Move domain logic to
core only when it has a second consumer. The dominant GUI cost remains eframe;
moving code between GUI modules will improve reviewability more than build time.

## 28. Binary Target Simplification

Cargo currently points all three GUI targets at src/main.rs: emuwiz,
emuwiz-gui, and legacy archivefs-gui. This repeats warnings but not
implementations. Keep aliases during the rename, then use one shared library
entrypoint and thin wrappers. Deprecate archivefs-gui only after launcher and
configuration compatibility evidence.

## 29. Source Role Structural Status

The source controller seam exists, but Role editing still needs a typed action,
source-specific validation/persistence, shared writer-gate decision, config/DB
reload, global refresh/history effects, and Sources-page wiring. The feature can
live outside main.rs once it returns typed effects; the remaining coordination
is in source_controller.rs, sources_page.rs, and the eventual app/effect reducer.
It remains unimplemented and was not reissued here.

## 30. Post-Scan What Next Structural Status

Summary computation can live in home_page.rs or a source/discovery projection
over existing scan summaries. A task-oriented next action can be returned to
the shell, while app.rs performs navigation and worker starts. Likely files are
home_page.rs, sources_page.rs, navigation.rs, and later app.rs. This is a product
slice, not a structural extraction in this audit.

## 31. 250k Scanner P0 Structural Status

The scanner ceiling remains in core discovery/scan modules. Its refusal/summary
flows through source and database scan orchestration into Sources/Discovery.
Main cleanup does not affect the implementation boundary. A later fix should
start with the core scanner limit and scan result types, then update GUI summaries
and tests. No production scanner code was changed.

## 32. Future Library Target Research Hook

Reserve a multi-target library profile audit for RomM, Gaseous, Retrom,
Gameyfin, ES-DE, RetroDECK, Playnite, and generic/custom projections. The
likely home is a core provider/profile abstraction plus GUI source adapters,
not one ArchiveFsApp field per provider.

## 33. Jolly Good/QTea Research Hook

Reserve a managed-emulator audit for Nestopia JG, bsnes JG, Cega, JollyCV,
Geolith, and CEN64. First identify shared executable/profile/ROM conventions
and safe read-only discovery facts. Likely owners are emulator inventory/core
probes and a GUI setup adapter.

## 34. Priority Buckets

### P0 structural

- introduce a focused LibraryUiState bundle;
- separate update polling from effect reduction;
- move Sources/Libraries body to its existing owner;
- establish app.rs migration seam.

### P1 structural

- bundle mount state behind existing mount modules;
- move selected page rendering/action projection;
- move catalogue/BSFree orchestration into cheats_mods;
- move doctor/repair reduction behind typed effects.

### P2 cleanup

- remove proven stale imports/re-exports;
- consolidate selected-evidence wrappers after caller proof;
- retire proven route/test fossils;
- narrow wildcard imports;
- resolve fixture provenance.

### P3 polish

- reduce repeated render allocations;
- improve full-frame regression coverage;
- simplify binary aliases;
- unify normal/advanced wording.

## 35. Next 5 Implementation Slices

| Slice | Lines removed | Fields removed | Risk | Likely files | Main touched |
|---|---:|---:|---|---|---|
| LibraryUiState bundle | 50–120 | 15–18 | MEDIUM | main.rs, library owner, focused tests | yes |
| Sources/Libraries body | 180–260 | 0–3 | HIGH | main.rs, sources_page/controller, source tests | yes |
| Catalogue/BSFree reducer | 250–450 | 12–17 | HIGH | main.rs, cheats_mods tree | yes |
| Explicit AppEffect reduction | 120–250 | 0–2 | MEDIUM/HIGH | main.rs/app.rs, controller tests | yes |
| ArchiveFsApp/update/ui to app.rs | relocation; 100–250 net | 0 | HIGH | main.rs, app.rs, Cargo targets/tests | yes |

The first slice is deliberately not the largest: it reduces field coupling and
creates a stable input/effect boundary for later page and app.rs work.

## 36. Final Size Estimates

| Milestone | Estimate |
|---|---:|
| current main.rs | 10,185 |
| after P0 structural work | 7,800–8,800 |
| after P1 ownership work | 6,300–7,500 |
| after dead-code cleanup | 5,800–7,000 |
| healthy app.rs | 2,800–4,200 |
| healthy tiny main.rs | 150–280 |

A 5,000-line main.rs is possible only with substantial page relocation or by
counting the app.rs move. A 3,000-line single main.rs is not realistic while
it remains the composition root. A 100–300-line main.rs is realistic as the
binary entrypoint after the app split.

## 37. Round 4 Scorecard

| Measure | Result |
|---|---:|
| main.rs declaration/impl sites | 227 approximate sites after RomM worker move |
| ArchiveFsApp fields | 234 |
| ArchiveFsApp methods | 76 |
| major field clusters | 14 |
| giant methods over 100 lines | 8+ |
| giant methods over 200 lines | 4 |
| large match blocks | 9+ |
| dialog families | 9 feature families plus shell overlays |
| background worker families | 20+ |
| page-specific rendering blocks | 15+ |
| dead-code groups | 16 |
| orphan-test groups | 4 |
| duplicate-helper groups | 6 |
| per-frame performance smells | 8 |
| cache opportunities | 7 |
| crate candidates investigated | 7; none YES NOW |
| state-object candidates | 8 |

Counts use the Round 3 convention where applicable. The declaration count is
approximate because nested impl/test declarations are grouped differently by
simple scanners.

## 38. Recommended Immediate Next Task

Implement one narrow structural slice:

Introduce LibraryUiState for library presentation/action fields. Scope it to
state encapsulation and mechanical access updates. Do not move database loading,
live-load workers, mount operations, health computation, or page rendering in
the same commit. Preserve selection, filtering, sorting, platform/alias action,
confirmation, refresh, and navigation behavior.

## Validation

- task preflight passed with clean worktree;
- no production Rust changed;
- git diff --check and task postcheck to run after writing this document.

