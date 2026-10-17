# EmuWiz Blue: remaining legacy interfaces

Status after Blue GUI Batches 10 and 11 (branch `feature/emuwiz-blue-gui-20261016`).
These are **outstanding Blue GUI migrations, not completed work**. Each stays
available, unchanged, in the specialist interface (Blue: All Tools → Opens
elsewhere → Advanced → "Open specialist interface", a separate window). None was
redesigned without an approved board, and no capability was removed.

## Owner-named (Batch 10 decision): keep in the specialist interface

| Tool | Where in the specialist interface | Blue today | Verified |
|---|---|---|---|
| Collection Discovery | Sources page → Discovery | partial: Sources lists up to 200 discovered folders with "Add this folder"; no summary, skip reasons, platform breakdown, paged details or skipped-files window | opens with live data |

## Native repair workflow

Organisation → Repair Review / Repair History embeds the existing repair controllers. Load or scan a plan, inspect evidence and select Safe proposals, then explicitly confirm Apply. Preview alone never executes repairs. History shows the same transaction records, filesystem reverify snapshot, partial/failure/recovery guidance, confirmed Undo and supported history housekeeping. Refresh and filesystem verification run in the background; actions retain existing backend revalidation and limitations. Leaving the page does not cancel an accepted operation. The Classic entry remains available, but is no longer required for these workflows.

## Native Library Views

All Tools → Organise & export → Library Views, or Library View History → Open Views, embeds the existing saved-view editor in Blue. Create/edit named views, select source folders and platforms, choose supported profiles, enable/disable, Preview and filter the plan, explicitly confirm Apply/Repair, and use the existing confirmed Remove workflow. Refresh loads definitions and catalogue sources in the background; unreadable or invalid configuration is shown as unavailable and blocks editing. Large preview details are paged at 100 entries without changing the complete plan or counts. Existing definitions, profile mappings, symlink engine and history remain authoritative; master archives and DAT/platform evidence are not rewritten. Classic specialist access remains available.

Library Views are managed symlink folder trees, not saved Browse & Play search presets. Browse & Play already provides platform/search/evidence filters, grid/list preferences and selected-game navigation; arbitrary saved filter/sort presets and new layout templates are not added by this migration. Verification evidence is not a claim of launch readiness.

## Also legacy-only (no Blue page)

Library → Duplicates tab (catalogue duplicate report) ·
Recently Found · Doctor deep scan with per-finding repair review · CheatBase ·
cheat reconciliation review · Dolphin cheat catalogue manager · RetroArch cheat
database manager · standalone BSFree game browser · PPSSPP texture mod panel ·
ScreenScraper batch enrichment · About · Gamer view (by design: Browse & Play
replaces it).

## Partial in Blue (the rest is legacy-only)

- Library table: batch enrichment, bulk select and sort.
- Library → Health: mount-failure, retryable, terminal and recovery filters.
- Ready to Play: the library-wide table (Blue is per game).
- Selected game: Plan Preview and RPCS3 panels.
- Sources → Libraries: RetroArch cheat-database and BSFree cards.
- Cheat Sources: per-source enable/priority and "Save preferences" (providers and
  previews are native).
- Emulator Manager: updates, staged update review, Undo and recovery (Blue shows
  read-only cards).
- Saves: PS2 Save Vault is embedded natively in Blue (card inspection, export, explicit restore/replacement preview, guarded Apply, journal recovery and confirmed Undo). Other save types remain read-only inventory; no additional restore format is claimed.
- Automatic Health Report and Configuration Diagnostics: per-check lists.
- Database status: "View recently found" and the skipped-files drill-down.
- Settings: EmuWiz locations, validate configuration, RetroArch profiles,
  environment report, run setup again (handed off).

Already in Blue though the integration matrix listed them as legacy-only (shown
inside Blue pages through the hosted legacy renderers in
`gui_v2/native_workflows.rs`): DAT Duel, DAT coverage and authority dashboard,
RetroArch core setup, emulator download, platform artwork manager, Import your
cheat files, mods hub and rollback, Doctor "Copy summary".

## Related follow-up

43 user-facing strings produced by `archivefs-core` (for example doctor recovery
paths such as "Library → Health, Retry") still contain `→`/`✓`, which egui's
default fonts draw as an empty box. The GUI crate's own strings were fixed in
Batch 11; changing core wording also changes CLI output and needs its own task.

## Native frontend export (2026-10-17)

Audit against `b55c0a2a` found the existing Blue Organisation workflows already
supported ES-DE linked-library creation and separate safe `gamelist.xml`
publication, RomM preparation and RetroDECK preparation. These remain the same
workflows, using their existing destination settings, previews, confirmations,
backups and recovery. ES-DE preserves existing entries and requires a fresh
preview after a conflicting edit; this migration adds no new metadata writer.

Organisation → Frontend export profiles now hosts the existing Publisher Profiles
controller, previously Classic-only. Build a Playing Library preview first, then
choose RomM or ES-DE, an absolute destination and canonical system. Preview shows
the target, destination, game and companion paths, existing-file states, conflicts
and blocked/unsupported items. Choose hardlinks or symlinks, review execution,
type the exact confirmation and Apply. The existing backend revalidates the
reviewed transaction immediately before writing. Results retain transaction IDs,
partial/failure details and reconciliation guidance; supported completed
transactions offer separately confirmed rollback. Preview, execution review,
Apply and rollback run off the GUI thread and accepted work survives navigation.

| Capability | Classification after migration |
|---|---|
| ES-DE linked library and safe gamelist preview/publish/recovery | ALREADY BLUE |
| RomM and RetroDECK established preparation workflows | ALREADY BLUE |
| Publisher Profiles: RomM/ES-DE targets, destination, preview, conflicts, explicit execution, results and rollback | ALREADY BLUE (migrated here) |
| Copies, additional frontend formats/backends, profile configuration/artwork generation | UNSUPPORTED; no write control |
| Occupied/conflicting destinations, unsafe source/mapping, unavailable hardlinks, changed reviewed filesystem identities | BLOCKED by existing planners/executor; no overwrite fallback |
| Frontend export functions requiring Classic | None identified |

Profiles create missing game links only. They do not replace gamelists,
configuration, artwork or occupied game paths, and introduce no backup/Undo
promise beyond the existing transaction backend. For ES-DE metadata use the
ES-DE gamelist export button or Organisation → Export to ES-DE. Preview never
exports automatically; source ROM bytes and Launch resolution remain unchanged.

Validation: 118 focused GUI tests (publisher page, Blue frontend navigation,
Playing Library and Organisation), 42 core Publisher Profiles tests, 128 launch
ES-DE tests and 60 ES-DE environment/metadata tests passed. Fixtures use disposable
frontend directories and synthetic gamelists. Coverage includes target selection,
preview, conflicts, unsupported mappings, user-file preservation through Apply
and rollback, changed destination refusal, off-page job completion, retained
terminal outcomes and keyboard confirmation/Apply visibility at 1024×600.
Independent review findings were repaired: source-preview changes retain earlier
write outcomes until explicit refresh, and worker failures require reconciliation
without claiming no filesystem changes occurred.
