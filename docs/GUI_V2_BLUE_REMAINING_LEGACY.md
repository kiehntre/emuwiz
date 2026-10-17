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

### Shared bounded media preview and exact subset selection

Sources → Discovery also offers an explicit **Read-only media preview** for one
folder inside an enabled game source, including a narrower subfolder without
changing configuration. It reuses `ArchiveScanner`, the existing Spectrum
inspectors, checksum helpers, ZIP/7z member readers and DAT indexes. Traversal,
parsing and hashing run on a worker. Preparing the preview never writes library
records or DAT history. A sealed allowlist can now proceed through **Review
bounded import… → Confirm and apply reviewed allowlist**, on the existing source
action worker with its shared busy-state gates. Apply only hashes/revalidates the
reviewed paths; it never starts a root scan. Existing full-source scans remain
separate and do not consume the preview's exclusions.

Bounded Apply follows the shared scanner's existing regular-file media kinds
and canonical platform identities, rather than a Spectrum-only extension list.
The existing structural Spectrum inspectors still guard snapshots/disks. Amiga
HDF/HDFX now reuse the existing RDB/flat-AmigaDOS inspector at the discovery gate;
unknown/malformed HDF files remain excluded. This is format recognition, not a
claim that a filesystem or game is usable. Legacy positive/negative HDF cache
entries cannot bypass structural inspection; unsafe/special files are refused.
Unreadable HDF inspection makes coverage partial and preserves existing presence.

**Select an exact bounded batch / saved evidence** supports individual stable
candidate IDs or a bounded first batch (1–1,000 records). Save native reviewed
evidence outside source roots and retain its whole-file SHA-256 separately;
**Load pinned evidence** refuses corruption or a different digest. The pin is an
integrity check for a trusted native artifact, not authentication of arbitrary
edited JSON. Legacy external investigation JSONL files are not native evidence
and cannot be turned into Apply capabilities by loading them.

**Prepare from reviewed paths without a root scan** bootstraps native evidence
from a pinned request containing 1–1,000 explicit paths, canonical expected
platforms, optional preserved full hashes/filesystem fingerprints, and exclusions.
It freshly hashes/parses only those paths and native companions, then reuses the
same saved-evidence stable-ID subset verifier and confirmation boundary. It never
deserializes an external classification into write authority or enumerates the
source directory. Header-only disc/cartridge checks also avoid layout inspection.
The read-only `emuwiz-cli discovery preview-paths` command uses this same core
entry point and saves native evidence for Blue; it has no Apply command.

Explicit registered catalogue IDs may carry independently retained SHA-256 pins.
Enabled file registrations are freshly hashed and parsed; canonical platform
identity must agree with the selection and any saved platform assignment. This
supports installed TOSEC projections without inventing legacy health fields.
Parser notes retain their existing neutral severity; real warnings/errors refuse
comparison. Completeness covers the selected catalogue set, not every installed
catalogue or unique-game identity. Registry and DAT audit state are never written.

Existing LHA readers now feed both root and exact-path previews. A complete member
verification plus exactly one structurally valid WHDLoad slave permits one intact
Amiga package, including unrecognised resource extensions. Multiple slaves,
inspection warnings, nested archives and recognised conflicting-platform members
remain explicit refusals. Packages are not flattened into member records.

**Prepare exact subset preview** does not invoke `ArchiveScanner` or traverse
the root. It rechecks selected file proofs, native recognition, configured source
authority, protected library records and explicit archive/catalogue references,
then seals a fresh plan. Unknown/repeated IDs refuse selection. Changed, missing,
protected, unsupported or conflicting candidates have explicit refusal reasons.
Unselected paths become exclusions and cannot enter Apply. Selection changes
revoke pending confirmation. Saved, selected, eligible, refused, excluded and
projected insertion counts remain distinct; Apply reports actual inserted rows.

ZIP/7z member comparisons remain bounded and platform-specific. Proven indexed
archive-member copies cannot enter the subset. Unsupported/nested/conflicting
archive members and unavailable comparison readers block eligibility for that
platform. RAR/LHA content comparisons retain their specialist limitation.
CUE/GDI/playlist descriptors, extracted folders, WHDLoad/romset and other specialist
import paths are not redirected through this importer. Supported regular disc
images and archives retain their existing record representation. No automatic
multi-disk grouping, format merging or alternate-release merging is added.

Configuration, source roles/overrides/aliases, accepted source binding, database
identity, existing records, catalogue inputs and reference files must remain
current. Reviews expire after 30 minutes. An unbound/replaced source still needs
the existing source review/rebind workflow. Hashing, parsing, saved-file loading
and subset validation run off the GUI thread; candidate rows are paged at 25.

New rows use the existing archive upsert savepoint and platform precedence inside
one catalogue transaction. No existing record, missing marker, source scan status,
DAT history or ROM is updated. Cancellation/errors roll back the entire batch;
committed imports have no automatic Undo. Confirmation requires acknowledgement
of a consistent database backup; restoring it also loses later database changes.
Counts are projected media-row insertions, not unique games or a database diff. The
review and confirmation expose SHA-256 approval fingerprints for exact candidate
path/content/platform/kind identities and excluded path membership, so a saved approval plan
can be compared without relying on counts alone.

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
merge formats, releases or parts. Automatic catalogue loading uses registered,
enabled, fresh validated file sources with matching registered/parsed platforms;
explicit pinned catalogue choices use the fresh validation described above.
Lookups never cross canonical platform boundaries. An absent match does not
establish game uniqueness or emulator usability.

Root comparison inputs are charged against 4 GiB, with a separate 256 MiB decoded-member
budget. Explicit-path bootstrap and subset revalidation each have a 4 GiB input
bound; the displayed charge includes both passes. Indexed archive references are
revalidated, while unselected loose media are not read. These are bounded input
charges rather than measured disk I/O; discovery/parser reads are additional. Unsupported readers, unavailable references, limits and partial coverage
remain explicit. Cancellation discards partial preview results after the existing
walk/catalogue preparation returns or between comparison chunks. Source settings,
GUI generation and root changes invalidate the result; compared loose files and
archives are checked again before publishing it. Filters and paging stay bounded
at 50 rows, with primary controls above the scrolling results. No production scan
or import is performed by opening the page.

### Cross-platform evidence check (2026-10-17)

Read-only inventory, existing records and native history projection show distinct
causes. No production import, rescan, DAT audit or identity correction was run.
Counts below remain unchanged after development; eligibility is not uniqueness.

| Platform | Current physical media | Indexed available paths | Native current / historical rows | Finding |
| --- | ---: | ---: | ---: | --- |
| ZX Spectrum | Saved inventory: 62,287 total files | 39,357 | 39,509 / 516 | Saved gap: 22,131 eligible; 32 archive copies, 117 refusals, 646 unsupported, 4 metadata. No repeated inventory. |
| Amiga | 1 IPF, 1,490 LHA, 2,128 HDF | 1 | 1 / 0 | One visible item reflects one actual row. LHA recognition landed after last successful scan; HDF had the confirmed gate mismatch. Three sampled HDF headers are AmigaDOS; all 2,128 were not structurally audited. |
| Atari ST | 682 ST, 196 STX; no MSA found | 878 | 878 / 191 | All current media indexed; unavailable legacy `atarist` paths explain history. ST/MSA/STX recognition already exists. |
| SNES | 4,023 ZIP | 4,023 | 4,508 / 3,555 | All current ZIPs indexed; 17 additional root records are unavailable. All ZIPs contain `.unh` members unsupported by current recognition. Folder-based SNES assignments are not exact platform verification; no automatic relabeling. |

Amiga has 10,227 artwork/manual/metadata files, Atari ST 14,793 and SNES 14,922;
these are not missing game records. None of the four platform populations has
persisted DAT identity rows in this snapshot. Missing DAT audits do not imply
missing Discovery records. Native history counts use existing supersession logic
without rename-journal inputs; physical availability is reported separately.
Read-only per-file evidence and the projection harness are retained in the local
`emuwiz-audit-ledger/2026-10-17-shared-discovery` directory.

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
