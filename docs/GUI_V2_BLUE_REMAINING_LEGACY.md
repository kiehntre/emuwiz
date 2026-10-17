# EmuWiz Blue: remaining legacy interfaces

Status after Blue GUI Batches 10 and 11, updated for completed native workflows below.
The remaining specialist interfaces listed below are **outstanding Blue GUI
migrations**. Completed native workflows are described separately. Each remaining
interface stays available in the specialist interface (Blue: All Tools → Opens
elsewhere → Advanced → "Open specialist interface", a separate window). None was
redesigned without an approved board, and no capability was removed.

## Native Collection Discovery

Sources → Discovery now hosts the existing Collection Discovery renderer: content
summary, attention reasons, platform breakdown, filtered/paged saved item details
and the existing skipped-files window. The retained "Add this folder" sample stays
available separately and is labelled as a bounded sample. Library-record counts
(new, updated, unchanged, missing and errors) come from the saved scan, including
the newest partial run after restart. Partial coverage and source failures are
explicit; incomplete coverage does not become a clean result.

| Capability inspected against Classic | Previous Blue classification | Native Blue now / retained limitation |
|---|---|---|
| Add roots, enable/disable, one-root/all-enabled scan | Already present | Reused; Scan and Enable now require scope review and confirmation. Enabling retains its existing immediate-scan semantics. |
| System assignment, folder roles, aliases and manual assignments | Specialist-only controls | Remain specialist-only; review shows saved roles, assigned systems and known catalogued systems. No identification changes. |
| Post-scan summary, platform breakdown, skip reasons and paged details | Missing | Existing read-only renderer hosted in Blue; attention shortcuts filter persisted queries as well as bounded samples. |
| New/existing/missing library-record counts | Missing from Discovery | Saved counters shown separately from ingestion item counts. |
| Unrecognised, ambiguous, unsupported and unreadable items | Missing full detail view | Existing filters and per-item evidence; uncertain platforms stay unassigned. |
| Source access/permission failures | Already present on source cards; missing Discovery detail | Source cards retained; saved errors, partial coverage and in-session folder warnings shown in Discovery. |
| Review before scan writes; cancel before starting | Explicit scan controls existed; scope review missing | Review lists selected/excluded roots and system scope. Cancel/Escape starts nothing. Stale GUI generations and changed saved source state refuse execution. |
| Dry-run database diff, per-system scan filter, running cancellation, incremental scan progress | Unsupported by existing source scanner | Remain unsupported. The page explains this before confirmation and reports an indeterminate running worker until results return. |

Browsing, filtering, paging and reloading saved status never start a scan. Accepted
scans use the existing background source-action worker and backend validation;
leaving Sources does not cancel them. Saved configuration and source metadata are
rechecked read-only immediately before the existing action, without introducing a
new mutation or cross-process locking protocol. Already-configured roots can also
receive their first scan: saved scope loads on a read-only worker, confirmation
names the database to be created, and a database appearing before execution
invalidates the review. Content/platform aggregates and
the bounded skipped sample remain session-only backend data; saved item pages,
library counters and partial-run errors survive restart. Existing Sources Mr Wiz
guidance and Blue styling/navigation are retained. The specialist entry remains
available, but is no longer needed to inspect Collection Discovery results.

### Spectrum media coverage and read-only preview

Sources → Discovery also offers an explicit **Read-only media preview** for one
folder inside an enabled game source, including a narrower subfolder without
changing configuration. It reuses `ArchiveScanner`, the existing Spectrum
inspectors, checksum helpers, ZIP/7z member readers and DAT indexes. Traversal,
parsing and hashing run on a worker. The preview has no Apply/import action and
never writes library records or DAT history. Existing confirmed scans remain
separate; they do not automatically consume the preview's content exclusions.
This is a media-candidate review, not a database diff or immutable import plan.

Z80, SNA, SZX, TRD and SCL now pass through their existing structural inspectors
before catalogue inclusion; a suffix alone is insufficient. Legacy positive and
negative cache entries cannot bypass this validation. Z80 interrupt flags accept
the documented zero/nonzero encoding; SNA ULA output bytes retain their border-bit
interpretation. Snapshot reads use the shared safe-open policy, a hard bounded
read and changed-file checks. Unreadable snapshots/disks make scan coverage partial
rather than making old records appear missing. The legacy unsupported counter
retains its compatibility projection (including structurally refused media);
per-item ingestion and skipped-file reasons distinguish structural refusals.

Preview filters distinguish eligible media candidates, exact SHA-256 archive
representations, catalogue relationships (exact game name versus weak matching
title), hash-backed multipart labels, and unsupported/ambiguous items. They do not
merge formats, releases or parts. Catalogue comparison uses only registered,
enabled, fresh validated Spectrum file sources whose parsed headers also identify
Spectrum; no match never proves a new unique game or emulator usability.

Comparison inputs are charged against 4 GiB, with a separate 256 MiB decoded-member
budget; discovery/parser reads are additional and these are not measured disk-I/O
counts. Unsupported readers, unavailable references, limits and partial coverage
remain explicit. Cancellation discards partial preview results after the existing
walk/catalogue preparation returns or between comparison chunks. Source settings,
GUI generation and root changes invalidate the result; compared loose files and
archives are checked again before publishing it. Filters and paging stay bounded
at 50 rows, with primary controls above the scrolling results. No production scan
or import is performed by opening the page.

## Native repair workflow

Organisation → Repair Review / Repair History embeds the existing repair controllers. Load or scan a plan, inspect evidence and select Safe proposals, then explicitly confirm Apply. Preview alone never executes repairs. History shows the same transaction records, filesystem reverify snapshot, partial/failure/recovery guidance, confirmed Undo and supported history housekeeping. Refresh and filesystem verification run in the background; actions retain existing backend revalidation and limitations. Leaving the page does not cancel an accepted operation. The Classic entry remains available, but is no longer required for these workflows.

## Native Library Views

All Tools → Organise & export → Library Views, or Library View History → Open Views, embeds the existing saved-view editor in Blue. Create/edit named views, select source folders and platforms, choose supported profiles, enable/disable, Preview and filter the plan, explicitly confirm Apply/Repair, and use the existing confirmed Remove workflow. Refresh loads definitions and catalogue sources in the background; unreadable or invalid configuration is shown as unavailable and blocks editing. Large preview details are paged at 100 entries without changing the complete plan or counts. Apply/Repair retain the existing behavior of rebuilding the current view plan at execution: preview counts can change, and this is not immutable Preview-to-Apply binding. Existing definitions, profile mappings, symlink engine and history remain authoritative; master archives and DAT/platform evidence are not rewritten. Classic specialist access remains available.

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
- Database status: "View recently found" (the skipped-files drill-down is available in native Sources → Discovery).
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
