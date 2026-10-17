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
| Publisher profiles / Frontend export | Organise & Export → Build Libraries → Export | none | opens |

## Native repair workflow

Organisation → Repair Review / Repair History embeds the existing repair controllers. Load or scan a plan, inspect evidence and select Safe proposals, then explicitly confirm Apply. Preview alone never executes repairs. History shows the same transaction records, filesystem reverify snapshot, partial/failure/recovery guidance, confirmed Undo and supported history housekeeping. Refresh and filesystem verification run in the background; actions retain existing backend revalidation and limitations. Leaving the page does not cancel an accepted operation. The Classic entry remains available, but is no longer required for these workflows.

## Also legacy-only (no Blue page)

Library Views editor (create/edit/apply/remove; Library View History is native
and hands off here) · Library → Duplicates tab (catalogue duplicate report) ·
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
