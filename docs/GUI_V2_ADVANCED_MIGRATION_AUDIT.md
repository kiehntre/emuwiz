# GUI v2 Advanced Workflows Audit and Migration Plan

Audit snapshot: 2026-09-20  
Worktree: `/home/davedap/emuwiz-gui-v2`  
Branch: `feature/gui-v2-shell`  
Starting HEAD: `52af8de26da572f36fc57fcd3447d91fc33553dc`

This is an audit and migration plan only. It does not delete legacy pages, change
production Rust, change the GUI, push a branch, or alter backend authority.

## Executive result

The requested base is intact: HEAD is `52af8de26da572f36fc57fcd3447d91fc33553dc`,
the worktree was clean, and `52af8de2` is an ancestor of HEAD.

The v2 shell has one live legacy handoff family, not a hidden automatic fallback:
the explicit `Advanced` route opens a separate legacy window at `MainView::DatSources`.
All normal v2 task sections currently render native v2 pages. The compatibility
handoff table still contains destinations for future contextual launches, but the
v2 dispatcher does not route normal exploration to them. The existing tests make
this contract explicit: accidental section exploration must not start a legacy
window or a scan (`gui_v2/tests.rs`, `gui_v2/legacy.rs`).

The recommended direction is therefore selective:

* Migrate high-value, user-facing journeys whose backend is already safe and
  reviewable: DAT/provider management, cheat-specific controls, and the remaining
  full organisation/export journeys.
* Keep specialist inspection, mount/storage/tape tools, and technical diagnostics
  under Advanced intentionally.
* Retire duplicate normal-workflow labels after parity, not before.
* Keep developer and recovery internals as compatibility/backend surfaces unless a
  concrete novice workflow needs them.

## Current routing and handoff proof

### Live v2 route graph

`gui_v2/routes.rs` defines the v2 sections. `gui_v2/pages.rs` renders these sections
directly: Games/Launch, Emulator Setup, Sources, Artwork & Metadata, Mods & Cheats,
Check Games, Problems & Repair, Build Library, Platforms, Activity, History and
Settings. The only section rendered by `handoff()` is `Section::Advanced`.

The exact live handoffs are:

| User action | v2 route | Handoff call | Separate-window destination | Native/partial status |
|---|---|---|---|---|
| Sidebar `Advanced` | `Route::Section(Section::Advanced)` | `pages.rs::handoff` → `App::legacy(Section::Advanced)` → `gui_v2::backend::Command::Legacy` | `gui_v2::legacy::open` starts the same executable with `--legacy`; `LegacyHost` sets `GuiMode::AdvancedView` and `MainView::DatSources` | Advanced catalogue management is not native v2; Sources and verification summaries are native/partial elsewhere |
| `Open full technical interface` in the Advanced handoff page | Same `Route::Section(Section::Advanced)` | Same as above | Same `MainView::DatSources` destination | Deliberate specialist escape hatch |

`LegacyHost` adds a return banner and keeps v2 open. It passes a selected archive path
only when a future contextual route supplies one. There is no download, mutation or
automatic legacy fallback caused by merely visiting a v2 section.

### Compatibility mappings that remain reachable outside ordinary v2 exploration

`gui_v2/legacy.rs::destination` retains these exact mappings for compatibility and
future contextual actions:

| v2 section | Legacy destination | Current assessment |
|---|---|---|
| Check | `MainView::CheckGames` | Native v2 Check Games exists; compatibility mapping is redundant |
| Problems | `MainView::Problems` | Native v2 Problems & Repair exists; compatibility mapping is redundant |
| Build | `MainView::CanonicalOrganisation` | Native v2 Build Library is only a planning/subset surface; parity is incomplete |
| Launch, selected game | `MainView::Selected` | Native v2 launch/readiness path exists; selected-game legacy detail remains richer |
| Launch, no selection | `MainView::ReadyToPlay` | Native v2 Launch/Readiness exists; compatibility mapping is redundant for the normal journey |
| Emulators | `MainView::EmulatorSetup` | Native v2 Emulator Setup exists; legacy page is shared backend presentation |
| Mods | `MainView::CheatsMods` | Native v2 Mods & Cheats exists, but the legacy workspace still owns deep controls |
| Artwork | `MainView::Settings` | This is a stale/inaccurate compatibility mapping, not a valid Artwork destination; retire it when the unused handoff path is removed |
| Sources | `MainView::SourcesDiscovery` | Native v2 Sources exists; legacy discovery is still a deeper specialist workflow |
| History | `MainView::HistoryLogs` | Native v2 History exists; the legacy operation log remains a distinct technical journal |
| Advanced | `MainView::DatSources` | The one live v2 Advanced handoff; intentional until DAT/provider management is migrated |
| Settings | `MainView::Settings` | Native v2 Settings is intentionally limited; legacy settings remain a fallback |
| Other/unknown | `MainView::Library` | Defensive fallback only; no normal v2 route currently produces it |

The v2 route match handles Check, Problems, Build, Launch, Emulators, Mods, Artwork,
Sources, Activity, History and Settings before the generic handoff arm. Consequently,
the mappings above are compatibility inventory, not evidence that those flows are
currently opened by ordinary v2 navigation.

## Complete Advanced/legacy destination inventory

The inventory below combines the compatibility catalogue in
`navigation.rs::ADVANCED_NAV_GROUPS`, the `MainView` dispatch, and the
`ToolsOverlay` dispatch. “Mutates” means the page can cause a write when the user
explicitly confirms an operation; read-only pages may still refresh caches or logs.

### A — migrate to native v2

| Destination / exact page | User purpose and backend | Mutates / safety | Current v2 overlap | Decision |
|---|---|---|---|---|
| `MainView::DatSources` — DAT Sources / Sources → DATs | Register, import, validate, update and inspect DAT/catalogue providers; `dat_sources_page`, DAT authority, managed provider snapshots | Imports/configures metadata and may update local DAT state; existing validation, activation and provenance controls are the safety boundary | v2 Sources and Check Games expose summaries, but not full provider lifecycle | Migrate first as “Verification Sources”; keep existing page until a provider lifecycle/readiness journey is complete |
| `MainView::CheatSources` — Cheat Sources | Configure cheat providers, order, enablement, source health and reconciliation; `cheat_sources_page`, provider snapshots | Changes provider configuration and may retrieve metadata; explicit actions and retained snapshots exist | v2 Mods & Cheats has a native user journey, but source administration is still deep | Migrate after core Mods journey; expose simple source status, retain expert reconciliation under Advanced |
| `MainView::CheatsMods` — Cheats & Mods workspace | Per-game cheats, RetroArch catalogue, CheatBase, user imports, emulator-specific mods and rollback; `cheats_mods/*`, patch manager and journals | Can write cheat/mod files and emulator destinations; review/confirm, hash/base matching and rollback exist | v2 Mods & Cheats is native at the task level but does not yet own all deep controls | Migrate the cheat-specific controls as a complete journey; do not duplicate install engines |
| `MainView::CanonicalOrganisation` — Library Organisation / Build Libraries | Preview and apply organisation/rename/move plans; `rom_organisation_page`, transaction/journal/rollback backends | Writes/moves files only after explicit approval; collision checks and journaled recovery exist | v2 Build Library explains and previews a smaller playing-library flow | Migrate only the full plan → approval → progress → recovery journey |
| `MainView::PublisherProfiles` — Publisher / Frontend Library / Export | Project or publish frontend libraries and launch recipes; `publisher_profile_page`, playing-library and publisher backends | Export can write generated frontend metadata/launch files; projection pages are read-only until publish | v2 Build Library has an export concept but not full profile capability | Migrate user-facing supported profiles after organisation primitives; keep unsupported profile diagnostics out of novice navigation |
| `MainView::SourcesDiscovery` — Collection Discovery | Discover candidate folders/files and review source identity; `collection_discovery_page`, source discovery backend | Adds/configures source state only after explicit review; scanning is bounded and activity-visible | v2 Sources performs native source setup and discovery entry points | Fold into native Sources after the source lifecycle has one explanation and one activity path |
| `MainView::EmulatorInventory` — Emulator Manager / Installed Emulators & Updates | Inspect installed emulator profiles, versions/channels and staged update state; `emulator_inventory_page`, emulator update backend | Update/rollback can replace emulator installation files; staged plan/review/confirm/rollback safeguards exist | v2 Emulator Setup is native readiness; it does not own lifecycle management | Migrate as a separate advanced setup journey, not into novice launch; preserve explicit update review |
| `MainView::BiosProjection` — BIOS / Firmware | Show master BIOS inventory and project requirements to emulator contexts; `bios_projection_page`, firmware/readiness backends | Read-only projection; local files are user-owned and no acquisition is implied | v2 Emulator Setup/Launch already surfaces readiness summaries | Migrate useful explanations into native readiness; retain technical projection details under Advanced |

### B — keep under Advanced intentionally

| Destination / exact page/tool | Purpose and backend | Mutates / safety | Why normal users do not need it as a primary route |
|---|---|---|---|
| `MainView::Museum` — Museum | Curated catalogue/media exploration; `museum_page` and loaded artwork/evidence | Read-only; artwork lookup is bounded | Valuable browsing, but not required to play, verify, repair or organise; expose later as an optional Library tool |
| `MainView::MediaSets` — Media Sets | Inspect multi-file/disc topology and swap plans; media-set projection | Read-only | Specialist explanation for CUE/GDI/M3U/disc topology; surface a contextual explanation from Launch when needed |
| `MainView::Mount` / `ActiveMounts` — Mounts / Active mounts | Inspect and stop active mounts; mount controller and cleanup state | Stop/unmount can alter runtime mount state; existing stop/cleanup safeguards | Useful during troubleshooting, not a normal novice task; keep under Health & Recovery |
| `MainView::StorageHealth` — Storage Health | Catalogue-backed disk/storage health and usage | Read-only | Maintenance/diagnostic information; not a play readiness step |
| `MainView::TapeInspector` — Tape Inspector | Bounded tape analysis for selected media | Read-only | Format specialist tool; no normal user need unless the selected media is tape-like |
| `MainView::LibraryViewHistory` — Library View History | Durable apply/remove history for library views | Remove/restore is guarded by history/recovery rules | Keep under History; it is an expert audit trail rather than the normal Build flow |
| `MainView::HistoryLogs` — History & Logs | Session operation log, activity export and technical journal | Clearing/exporting log changes log state or creates an export; no game mutation | v2 Activity/History cover user-facing progress and recovery; technical logs remain useful for support |
| `ToolsOverlay::SaveVault` — PCSX2 Save Vault | Inspect/copy/manage PCSX2 save data around verified PS2 context; `pcsx2_page` | Can copy or alter save destinations; selection/verification context is the guard | Specialist, emulator-specific, and not needed for ordinary launch |
| `ToolsOverlay::PlatformAliases` — Platform Aliases | Review or add platform alias assignments | Writes platform alias configuration; explicit action | Rare setup correction; no novice reason to edit aliases directly |
| `ToolsOverlay::DatabaseStatus` — Database Status | Inspect database load/scan/upgrade state, recently found/skipped files | Scan/upgrade changes catalogue state; database safeguards and retry paths exist | Native v2 Activity/Check surfaces the normal action; internal status remains Advanced |
| `MainView::About` — About EmuWiz | Version, paths, schema and environment details | Read-only, except copy actions | Support information, not a workflow |

### C — retire after parity or because the route is obsolete

| Destination / surface | Evidence of duplication/obsolescence | Retirement condition |
|---|---|---|
| Legacy `MainView::CheckGames` route as a separate normal workflow | v2 Check Games renders natively; accidental exploration test confirms no handoff is needed | Remove the compatibility handoff branch after contextual links and deep verification parity are audited |
| Legacy `MainView::Problems` route label/workflow | v2 Problems & Repair already owns overview, diagnostics and repair/recovery tabs | Retire duplicate navigation, not the underlying repair renderers, after all deep links target the consolidated tabs |
| Legacy `MainView::ReadyToPlay` as a separate top-level launch entry | v2 Launch and native readiness are the novice-facing journey | Keep the underlying readiness coordinator; remove the duplicate entry after selected-game launch parity |
| Legacy `MainView::Sources` top-level label | v2 Sources is native and task-oriented | Retire duplicate route labels after source discovery, provider setup and recovery are all represented |
| Legacy `MainView::Settings` as a normal workflow | v2 Settings exists; specialist overlays are separate | Retire only duplicated settings controls after config migration and fallback behavior are tested |
| `legacy.rs` Artwork → `MainView::Settings` compatibility mapping | It does not open an Artwork page and is not a truthful destination | Remove the mapping with the unused generic handoff path; do not preserve a misleading fallback |
| `Section::Advanced` handoff page after DAT/provider migration | It currently has no directory and only opens DAT Sources | Replace with a native Advanced directory first; remove separate legacy host only when every retained specialist tool has a deliberate route |

### D — backend-only / diagnostic; not normal UI

| Surface | Purpose/backend | Classification |
|---|---|---|
| `ToolsOverlay::Diagnostics` — Configuration Diagnostics | Read-only setup/config diagnostics and refresh; `diagnostics`/Doctor state | Keep as support-facing diagnostics; link from Settings/Health, not Home |
| `ToolsOverlay::DoctorChecks` — Doctor Checks / Automatic health report | Detailed per-check report, copyable summary; Doctor runner | Keep Advanced/Health; the native Problems & Repair summary should be the novice projection |
| `MainView::Doctor` — Advanced Diagnostics / Emulator Setup backend | Shared Doctor scan and emulator readiness checks | Native setup projection is appropriate; raw report remains diagnostic |
| `ToolsOverlay::ArchiveInspector` — Archive Inspector | Inspect one archive/container internals | Backend diagnostic; open only from an explicit technical action |
| `ToolsOverlay::Onboarding` | Step tracker dispatching to real setup pages | Transitional orchestration, not a destination users should discover as a tool |
| `MainView::RepairReview` — Repair Review | Preview a saved whole-library repair plan from CLI/backend | Read-only review; apply/recovery authority remains in repair engines | Keep as a specialist review surface until v2 Problems can consume plan files |
| `MainView::RepairHistory` — Repair History | Journaled rename transactions and safe undo | Undo can mutate files, but only through transaction reversibility checks | Keep under Problems & Repair → Repair & Recovery; not a novice first action |
| CLI/developer/test utilities and page-level test helpers | Test fixtures, render mirrors, assertions and CLI-only operations | Not product workflows | Do not expose or migrate; preserve only as test/backend contracts |

## Requested specialist categories

The audit finds the requested categories in these exact places:

| Category | Current location | Recommendation |
|---|---|---|
| Advanced settings / configuration editors | `MainView::Settings`, `ToolsOverlay::PlatformAliases`, `ToolsOverlay::DatabaseStatus`, setup actions in `administration_pages.rs` | Keep simple settings native; keep aliases/database/config-folder and raw paths Advanced |
| Diagnostic tools | `MainView::Doctor`, `ToolsOverlay::Diagnostics`, `ToolsOverlay::DoctorChecks`, `ToolsOverlay::ArchiveInspector` | Native summary plus Advanced detail; no broad promotion |
| DAT management details | `MainView::DatSources`, `dat_sources_page.rs`, DAT authority/managed provider backend | Highest-value migration candidate; provider lifecycle needs native specialist directory |
| Converter tools | `MainView::DiscConversion`, `optical_conversion_page.rs`, CHD plan/execute/rollback | Already first-class in legacy navigation; migrate only if v2 Build/Organise can show full safety journey |
| Museum / retro workshop | `MainView::Museum`, `museum_page.rs` | Keep Advanced/optional Library tool |
| Cheat-specific deep controls | `MainView::CheatsMods`, `MainView::CheatSources`, cheat import/preview/reconciliation panels | Migrate user-facing per-game controls; keep source reconciliation internals Advanced |
| Provider diagnostics | `ToolsOverlay::Diagnostics`, provider cards in Sources/Cheat Sources/identity pages | Keep diagnostics Advanced; expose status and next action in native Sources/Artwork |
| Repair internals | `MainView::RepairReview`, `RepairHistory`, Doctor repair state | Keep under Problems & Repair; do not add a second repair system |
| Database/catalogue diagnostics | `ToolsOverlay::DatabaseStatus`, DAT authority dashboard, catalogue panels | Keep Advanced; native Check/Activity shows actionable result, not internals |
| Self-update/debug tools | `MainView::EmulatorInventory` staged update/review/rollback; diagnostics/about | Emulator updates are specialist Advanced; raw debug remains backend/support only |
| Import/export specialists | `MainView::PublisherProfiles`, user cheat import, log export, DAT/provider imports | Migrate supported user journeys in phases; retain technical import/export under Advanced |
| Developer/test utilities | Tests, CLI contracts, archive inspector internals and debug-only projections | Backend-only / never normal UI |
| Legacy configuration editors | legacy Settings, aliases, provider/source setup | Migrate only settings needed for normal setup; keep high-risk editors Advanced |

## Native v2 overlap and the nine named duplicated workflows

| Legacy workflow | Native v2 state | Audit conclusion |
|---|---|---|
| Sources | `Section::Sources`, `NativeWorkflows::show_sources`; native folder/source activity | Already partially native. Legacy Sources/Discovery is redundant for normal setup but still contains deeper provider and folder-role controls. |
| Artwork & Metadata | `Section::Artwork`, native artwork/metadata projections | Native summary is present; full provider configuration/enrichment is not parity. Legacy provider pages are a migration candidate, not a second novice route. |
| Build Library | `Section::Build`, native playing-library planning | Partially native. Full organisation, publisher profiles and export are not parity. |
| Problems & Repair | `Section::Problems`, native problem summary and repair guidance | Partially native. Doctor, repair plan review and history are shared/deeper surfaces. |
| Emulator Setup | `Section::Emulators`, `NativeWorkflows::show_setup` | Native readiness presentation exists over the shared Doctor/coordinator state. Legacy page should eventually become an Advanced detail view, not a competing setup journey. |
| Launch | `Section::Launch`, native launch readiness and selected-game route | Native for the normal path. Legacy selected/ReadyToPlay routes should be retained only for missing deep controls, then retired. |
| Mods & Cheats | `Section::Mods`, native v2 mods flow | Partially native. Deep cheat sources, imports, provider reconciliation and emulator-specific panels remain. |
| Activity | `Section::Activity`, v2 bounded jobs/activity | Native for v2 work; legacy session history is not the same data and should remain technical until durable parity is explicit. |
| History | `Section::History`, v2 history projection | Native user-facing history; Library View History, operation logs and repair journals remain distinct and should not be flattened. |

The rule is one owner per normal workflow. Existing backend engines and shared page
renderers may remain during migration, but v2 must not create a second mutation,
repair, provider or launch implementation.

## Recommended migration order

The ranking is by user value and backend readiness, not code size.

1. **DAT/provider management and verification sources.** The v2 shell already has
   Sources, Check Games, Activity and evidence language. Migrate provider registration,
   snapshot/provenance/status and safe import/review first. Do not move raw provider
   diagnostics into the novice path.
2. **Cheat-specific controls.** Keep the existing patch/install/journal engines and
   migrate the user journey: choose game → inspect match confidence → preview → confirm
   → progress → result/undo. Cheat-source ordering and reconciliation remain Advanced.
3. **Full Build Library / organisation.** Complete plan → approval → write → journal →
   recovery parity. The current v2 Build page is useful but does not replace canonical
   organisation or publisher/export profiles.
4. **Provider and metadata enrichment.** Unify native Artwork & Metadata with the
   existing ScreenScraper/RomM/provider pages only after source permissions, cache
   freshness and explicit acceptance are represented.
5. **Emulator lifecycle and self-update.** Keep Emulator Setup novice-facing; migrate
   inventory/update review as a bounded specialist workflow with staged update and
   rollback evidence.
6. **Museum and optional exploration.** Add only if the Library information architecture
   has room; it is a value-add, not a readiness gap.
7. **Diagnostics and developer internals.** Do not migrate merely for completeness.
   Preserve support access, test contracts and backend-only diagnostics.

Converter/Disc Conversion should be scheduled alongside Build/Organise only if users
actually need it in the v2 product flow. Its backend is already safe and reviewable,
but it is not a novice-first task and is not blocked by the current Advanced handoff.

## Advanced landing-page cleanup

The current v2 Advanced page is a generic handoff, not a directory. A future small
GUI-only cleanup should replace it with a bounded directory whose rows each contain:

* title and plain-language purpose;
* who needs it (for example “when a DAT source needs updating”);
* whether it is read-only or changes files/configuration;
* whether it opens a legacy window or an in-process technical page;
* a short state such as “native”, “legacy fallback”, or “planned”.

Recommended first rows are:

| Row | Destination | Mutates? | Audience |
|---|---|---|---|
| Verification Sources | `MainView::DatSources` | Imports/configures metadata; no game files | Users maintaining DAT/provider coverage |
| Emulator Manager | `MainView::EmulatorInventory` | May update/rollback emulator files after confirmation | Users maintaining emulator installations |
| Repair & Recovery | `MainView::RepairReview` / `RepairHistory` | May undo a reversible repair | Users reviewing a repair plan or recovery |
| Technical Diagnostics | Doctor/diagnostics overlays | Read-only except explicit maintenance actions | Support and troubleshooting |
| Specialist Library Tools | Museum, Media Sets, Storage Health, Tape Inspector | Read-only | Preservation/format specialists |
| Provider and alias details | Cheat Sources, Platform Aliases, Database Status | Changes metadata/configuration | Experienced maintainers |

This cleanup is intentionally not implemented in this audit because changing labels
or route rendering would require focused navigation tests. The current generic handoff
is safe but should not become a permanent dumping ground.

## Retirement and dependency plan

Legacy pages should move through these states:

1. **Visible as native v2:** normal task route owns discovery, explanation, action,
   progress, result and recovery.
2. **Legacy fallback:** old page remains reachable from Advanced or a contextual
   technical action while parity is verified.
3. **Hidden from normal navigation:** compatibility/deep links remain, but no novice
   route advertises the duplicate.
4. **Removed or retained indefinitely:** remove only after route tests, persisted state,
   CLI/deep-link behavior, activity/history semantics and rollback paths are covered;
   retain diagnostic/developer tools indefinitely where they provide evidence not
   represented by a novice projection.

Dependencies preventing immediate removal include:

* `MainView` remains the single dispatch key for many existing renderers and tests;
* `ADVANCED_NAV_GROUPS` is still used by navigation reachability tests and compatibility
  documentation;
* `LegacyHost` is the only deliberate separate-window escape for the full technical
  interface;
* selected-game context and launch readiness still have deeper legacy detail;
* provider/DAT pages own lifecycle, provenance and recovery state not yet represented
  in v2;
* operation history, repair history and library-view history are distinct stores and
  must not be merged just to remove labels;
* CLI plan files and repair journals are external contracts;
* settings/configuration migration must not silently change the user's persisted
  legacy configuration.

## Tests and validation performed for this audit

No production Rust or GUI files were changed, so no focused navigation test was added.
The existing source-level checks inspected for this plan include:

* `gui_v2::tests::gui_v2_legacy_handoff_preserves_game_and_workflow` — records the
  compatibility mappings, including Advanced → `MainView::DatSources`.
* `gui_v2::tests::gui_v2_accidental_exploration_never_runs_scan_or_legacy` — verifies
  ordinary v2 section exploration does not open legacy or start a scan.
* `navigation::primary` tests — verify native primary destinations, specialist entries,
  titles and Advanced grouping.
* Existing GUI page tests for DAT sources, provider sources, Doctor/repair, emulator
  setup, cheats/mods, database/catalogue and history remain the implementation
  evidence; they were not altered by this document-only audit.

The final document-only validation is recorded in the delivery report: `git diff
--check`, the repository task postcheck, focused navigation tests, and workspace
Clippy were run with the mandated shared target configuration. A full GUI suite was
not required by the no-code-change scope; known limitations are listed below.

## Known blockers and limitations

* No live visual audit was possible from source inspection alone; labels and route
  intent were traced statically.
* The v2 Advanced section has no real directory yet, so its only current destination
  is DAT Sources.
* Full parity for provider lifecycle, cheat-source administration, publisher/export
  profiles, repair-plan files and emulator updates requires additional focused work.
* The compatibility Artwork mapping currently points to Settings and should be removed
  when its unused generic handoff path is retired.
* “Legacy” is a presentation boundary, not a backend boundary: many native v2 pages
  deliberately reuse the existing safety-critical engines and renderers.
* Diagnostics and history have multiple legitimate scopes; collapsing them without a
  data-model decision would lose provenance or recovery information.

## Source anchors

* `crates/archivefs-gui/src/gui_v2/routes.rs` — v2 section names and purposes.
* `crates/archivefs-gui/src/gui_v2/pages.rs` — native dispatch and the explicit handoff page.
* `crates/archivefs-gui/src/gui_v2/legacy.rs` — separate-window launch and exact mappings.
* `crates/archivefs-gui/src/gui_v2/native_workflows.rs` — native Sources, Setup, Launch and metadata bridges.
* `crates/archivefs-gui/src/navigation.rs` — `MainView`, `ToolsOverlay`, compatibility catalogue and legacy groups.
* `crates/archivefs-gui/src/navigation/primary.rs` — current primary/sidebar and specialist routes.
* `crates/archivefs-gui/src/app_pages.rs` — exact legacy page and overlay dispatch.
* `docs/GUI_V2_FEATURE_MAP.md` — prior native/handoff/planned capability map.
* `docs/RECALBOX_COMPETITIVE_AUDIT.md` — prior warning that BIOS-pack projects are not EmuWiz data sources.

