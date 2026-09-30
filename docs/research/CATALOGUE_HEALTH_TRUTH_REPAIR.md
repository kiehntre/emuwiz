# Catalogue Health Repair — Pass 1

Base: `dffed242fd9ad5e36febcaec8c93d23098d794b2`; local main equalled
fresh origin/main and was tracked-clean. Branch `fix/catalogue-health-truth`,
worktree `/home/davedap/emuwiz-catalogue-health-truth`. No promotion or push.
The prior audit was read directly from `e379d745`.

## Root cause proven by source and persisted history

Source 22 is `/mnt/usbdrive/games`, role `games`, containing a nested `arcade`
folder. Commit `d150a51d5` (20 September 2026, 13:15 BST) introduced the
specialist shortcut. The exact path is:

1. `scan_source_folder_at` registers all configured roots and selects source 22.
2. `scan_and_persist_folders_transaction` finds `folder.path/arcade`.
3. `arcade_specialist = arcade_root.is_some()` replaces the **entire source's**
   generic discovery with `ArchiveScanDiscovery::default()`.
4. The default has zero scan errors; `is_complete()` therefore returns true.
5. Extracted Arcade sets are appended, producing 28,899 directory records.
6. `persist_one_folder(..., complete=true)` passes only those IDs to
   `mark_unseen_archives_missing` with **source_folder_id 22**.
7. Every unseen non-Arcade row under the same mixed source is stamped missing.

This was not a global reconciliation across different source IDs: the source
scope was already isolated, but the platform/subdirectory subset falsely
claimed coverage of that entire source. Scan #196 saw 69,675 rows; #197 added
28,899 and recorded 69,675 `missing` observations, with zero errors. The missing
stamp is `2026-09-22T16:18:04Z`. This exactly matches the shortcut and explains
subsequent Arcade-only scans with zero discoveries of other platforms.

The mixed-source regression fixture contains SNES, PS2 and a nested extracted
Arcade set. The old shortcut observes one set and marks the other two rows
missing; the repaired scan observes all three. A separate three-source fixture
proves that only the successful Arcade source can reconcile absence while
unavailable SNES/PS2 sources retain all previous evidence.

## Repair and retained evidence

Generic discovery always walks its owning source, excluding only nested
configured roots and extracted sets already enumerated by the specialist.
Extracted-set read errors and enumeration bounds are explicit incomplete
coverage, including errors previously swallowed by `.ok()` and `continue`.

Migration 22 adds only `scan_source_coverage`: source/run IDs, typed coverage
state, exact excluded path bytes, source device/inode proof and diagnostics.
States are complete, partial, unavailable, failed, skipped, removed and not
attempted. Every registered source has a coverage record; historical runs have
no invented proof. Roles intentionally excluded from game scanning are skipped.
Targeted/disabled sources not attempted remain explicit. Incomplete game-source
coverage produces a persisted `partial` run; source success/error history and
last-known-good counts remain separate. Partial run status is understood by
the existing discovery reader. Committed positive additions from partial runs
remain available without describing the overall scan as completed.

The missing mutation boundary requires this run's complete coverage proof,
a still-available matching source device/inode, ownership outside excluded
roots, and actual `NotFound` evidence for the recorded path. Present files,
symlinks, wrong types and I/O errors cannot be called verified missing simply
because they were not emitted by discovery. Failed/interrupted/unproven runs
cannot authorize missing writes. Folder savepoints and the whole-refresh
transaction preserve rollback behavior.

`catalogue_health` reuses `FsProbe`, the existing identity evidence bridge,
source records and append-only archive observations. It adds the missing
catalogue classification: PresentVerified, PresentNotVerified, PossiblyMoved,
Missing, OrphanedSource and NotChecked. Verified presence requires existing
canonical verified identity and a matching current size/mtime fingerprint;
this is not a new identity verifier or an integrity guarantee. Removed roots
are read from all retained source rows, not the active-only GUI source list.
Orphan status takes precedence, with physical presence and candidates retained
as separate evidence. Never-verified presence is neutral.

Move candidates are indexed once over present catalogue observations. ASCII
case-folded basenames reproduce the audit's weak filename hints while an exact
basename flag preserves the distinction. Size, platform/source and relative
path relationships remain explicit. Only a historical exact-file SHA-256
matching a freshly hashed candidate strengthens evidence; hashing is on demand
and cached. Every candidate, including strong and ambiguous candidates, remains
review-only. No path is rewritten, and no row is deleted. Missing means the
recorded path is absent with no indexed candidate, not that all possible disks
have been searched.

The explicit presence apply takes a same-database preview, validates every
planned correction against current database and filesystem evidence, then
clears only proven-present missing flags atomically. It updates presence times
and appends `restored` observations to a named reconciliation run. Prior missing
observations, identity reports, paths, source history and unrelated rows remain
intact. A changed preview fails before updates. No automatic real apply exists.
If a removed-source row's path is present, stale absence can be corrected while
its source membership and OrphanedSource classification remain unchanged.

## Pending health column

`last_known_health` caches `ArchiveHealth`, whose variants describe mount/archive
inspection outcomes. Constructors use Pending; `FilesystemHealthProvider`
returns the archive's current value. Scan upserts cache that constructor value,
and there is no background catalogue health worker or mount-result-to-database
writer. The database design calls this a prospective last-observed reporting
cache, not canonical presence or identity. All real rows are Pending. Replacing
it with presence strings would conflate archive integrity, mounting and identity.
This pass therefore leaves that cache unchanged and supplies the narrow core
presence/evidence lifecycle separately. It does not invent an integrity worker.

## Database safety and initial measurements

Real DB: `/home/davedap/.local/share/archivefs/library.sqlite3`, schema/user version
21, 463,847,424 bytes, 132,064 rows. SQLite read-only backup:
`/tmp/emuwiz-catalogue-health-real-backup.sqlite3`, mode 0600, same size,
`PRAGMA quick_check = ok`. This backup was verified before mutation tests.
Tests use isolated fixture databases; the real DB is not migrated or written.

Initial directory-aware read-only measurement reproduced:

| Recorded path | Rows |
|---|---:|
| Present, stale missing flag | 69,034 |
| Present, clean | 28,899 |
| Absent, flagged | 20,729 |
| Absent, clean | 13,402 |
| Total | 132,064 |

Source 5 retains 13,405 rows: 13,402 unflagged and 3 flagged, all with that source
removed. Exact-basename hints number 30,951; the prior audit used case-folded
names (30,964). The comparator difference was confirmed by reading its probe,
not attributed to filesystem changes. The recorded initial warm presence/index
measurement took 2.381 seconds. The final report below uses the implemented core
model.

## Final real-catalogue dry run

The final read-only core example completed in **12.844015 seconds**, using schema 21
without migration. Every recorded path was probed once; present observations
were indexed by name and optional historical hash. No directory was walked per
row, and no file was hashed: the real catalogue has no archive-hash evidence.
Work is linear in catalogue rows plus emitted candidate relationships.

| Observation/classification | Rows |
|---|---:|
| Present, stale missing flag | 69,034 |
| Present, clean | 28,899 |
| PresentVerified | 15,658 |
| PresentNotVerified | 82,275 |
| PossiblyMoved | 18,606 |
| Missing | 2,120 |
| OrphanedSource | 13,405 |
| NotChecked (filesystem presence) | 0 |
| All absent rows with move candidates, including orphans | 30,964 |
| Ambiguous candidates (multiple distinct paths) | 101 |
| Strong SHA-256 move candidates | 0 |
| Safe stale-flag corrections proposed | 69,034 |
| Rows explicitly left untouched | 63,030 |
| Total catalogue rows | 132,064 |

The six classifications partition the catalogue. Present stale/clean and
candidate counts are separate evidence dimensions. Identity remains neutral:
116,399 rows without usable identity evidence do not mean 116,399 filesystem
NotChecked rows or problems. The existing bridge still resolves 15,665
identities; seven have absent recorded paths, leaving 15,658 PresentVerified.

The original audit's 69,034 stale flags, 34,131 absent paths, 30,964 filename
hints and 3,167 absent paths without hints are reproduced. Of the absent paths,
13,405 now classify as orphaned sources, including 12,358 with filename hints
and 1,047 without. The remaining configured-source paths are 18,606 move
candidates and 2,120 missing. The old exact-basename probe found 95 ambiguous
rows; case-folded indexing finds 101. These are weak, review-only hints.
All **34,131 absent rows** remain for later human review; no automatic relink or
deletion is proposed. The other 28,899 untouched rows are clean and present.

The real database SHA-256 remained
`2f2a69e31fbd0bd9df80a4eab7e88815472deb17878565ab4b9128ea759e0ab4`.
It still has schema 21, 132,064 archives and 89,763 missing flags. The preview
therefore did not fix the user's real catalogue during development.

## Controlled apply tested on a separate real-sized copy

`/tmp/emuwiz-catalogue-health-copy-apply.sqlite3` was created from the verified
backup, leaving both the original and backup untouched. The copy migrated to
22. Before mutation, its preview was required to propose exactly 69,034
corrections. The explicit library function applied those corrections in
**6.410 seconds**, appending exactly 69,034 restored observations and one named
reconciliation run. A second preview proposed zero changes; repeated apply was
a no-op. The copy retained all 132,064 rows and 20,729 genuine/still-unresolved
missing flags.

SQL comparisons against the untouched backup proved every archive column other
than the absence flag and presence/update timestamps unchanged. All earlier
observations, source records, platform assignments and verified/DAT identity
facts were retained. All 13,405 absent orphaned rows were unchanged. SQLite
quick_check passed. A later controlled real apply is reasonable only after a
fresh backup and preview, explicit approval and confirmation of then-current
counts; filesystem/database drift rejects the plan atomically.

## Focused validation

All commands used `--offline --locked -p archivefs-core`, with the cached target
at `/tmp/emuwiz-setup-portability-target`. No workspace test, GUI suite, release
build or GUI smoke was run.

| Cargo test filter | Result |
|---|---|
| `--test catalogue_health_truth` | 24 passed |
| `--lib database::catalogue_health_tests` | 7 passed |
| `--lib database::tests::` | 249 passed |
| `--lib ingestion::arcade` | 3 passed |
| `--lib tests::scanner_` | 7 passed; 1 existing ignored stress test |
| `--lib library_views::tests::` | 105 passed |

The fixtures cover complete scans, unavailable and failed roots, targeted and
partial platform coverage, the nested Arcade shortcut, stale presence,
verified absence, unscanned/excluded roots, weak and strong move evidence,
unknown optional size, multiple candidates, orphan retention, deterministic
repeat/recovery, immutable previews, atomic drift rejection, source replacement,
failed-run proof rejection, symlinks and schema-21 read-only compatibility.

Targeted `cargo check -p archivefs-core`, `cargo fmt --all -- --check`,
`git diff --check`, file-scope guard and GUI-root boundary guard all passed.
Only the read-only example was built. The refreshed final report reproduced
every count from the earlier 15.151-second run.

Reproduce the read-only report from this worktree:

```sh
cargo run --offline --locked -p archivefs-core \
  --example catalogue_health_preview -- \
  /home/davedap/.local/share/archivefs/library.sqlite3 \
  /home/davedap/.config/archivefs/config.toml
```

The example accepts only database/config paths and has no apply option.

## Deferred GUI work

No GUI file or wording changed. Pass 2 should project this classification into
Home, Browse, Game Details, Check Games and Problems & Repair; keep never-checked
identity neutral and candidates distinct from broken files. The stale empty
Problems summary/loading-generation bug, review/relink/removal flows and archive
integrity checks remain outside this pass. No Cheat Core branch was integrated.
