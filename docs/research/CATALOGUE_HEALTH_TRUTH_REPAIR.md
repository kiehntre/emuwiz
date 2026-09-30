# Catalogue Health Repair — Pass 1

## Safety hardening after independent rejection — 30 September 2026

Starting candidate: `6487d135db4a93403b1c7bf523a27ac9c64abfc4`.
Independent review: `/tmp/emuwiz-catalogue-health-independent-review/REVIEW.txt`.
The root-cause diagnosis below remains confirmed. Its original validation and
readiness claims are historical; this section supersedes them. Main remains
`dffed242fd9ad5e36febcaec8c93d23098d794b2`. No promotion, push, GUI change,
real-database migration or real reconciliation has been performed.

### Five repaired boundaries

1. **Ancestor traversal:** the new focused `catalogue_health/path_probe.rs`
   uses Linux `openat2`, source descriptors, `RESOLVE_NO_SYMLINKS`,
   `RESOLVE_BENEATH` and `RESOLVE_NO_XDEV`. It checks the root itself without
   following ancestor symlinks, probes targets relative to a pinned root,
   compares two target observations and rechecks the root pathname. Final
   symlinks are detected through descriptor metadata. No canonicalize fallback
   or path-string stat is accepted as an ownership proof. A symlink, mount
   crossing, loop, changed target or unreliable probe cannot prove presence or
   absence. Hash reads also use the same anchored traversal.
2. **Source continuity:** migration 23 stores accepted source generations,
   including root device/inode, filesystem type and filesystem ID. Device/inode
   collisions alone cannot transfer authority to a different filesystem.
   Scans compare that binding before enumeration, after enumeration, after
   persistence and before the outer commit. Missing reconciliation holds its
   authorized root descriptor through probing and savepoint commit, verifies
   that descriptor against the preflight proof, and checks the generation,
   current ownership and root continuity again at the write boundary. Changed
   roots invalidate coverage and preserve prior evidence. A normal reopen or a
   bind mount of the same tree remains usable; a changed binding requires
   `rebind_source_after_review` with the reviewed generation and exact
   `SourceRootBinding::inspect` result. Rebinding changes no archive evidence
   and invalidates old coverage: a new scan is required. Previously successfully scanned legacy sources
   without a stored binding require explicit initial review (generation zero),
   rather than inheriting trust from whatever disk currently occupies the path.
   Fresh sources establish their first binding on their first actual scan.
3. **Ownership:** both scanners exclude configured descendants; logical Arcade
   sets cannot aggregate across a delegated descendant either. A configured
   extracted-set leaf below Arcade is catalogued by its own source, rather than
   lost or duplicated under the parent. Most-specific ownership is tested over
   three levels. Duplicate lexical roots and filesystem/bind aliases are
   diagnosed before catalogue persistence. Missing writes recheck the current
   delegation graph against the coverage proof and remain source scoped.
4. **Preview binding:** migration 23 adds a monotonic catalogue revision with
   triggers over archive/source/identity/observation/scan/coverage/binding and
   platform changes. Apply starts an immediate transaction, checks that exact
   revision, and rechecks ID, path, source, representation, missing stamp,
   root/volume binding and target device/inode plus nanosecond metadata.
   Ownership changes, delete/recreate ABA, remove/reattach and a new scan reject
   old previews. Any drift refuses the apply atomically; no subset silently
   succeeds. Even an empty plan rejects a stale database revision; schema-21/22
   read-only previews require a fresh preview after upgrade. A fresh preview can succeed. Current presence is checked before
   updates and again before commit. Empty fresh plans remain a no-op.
5. **I/O coverage:** owned-file metadata errors are explicit discovery errors;
   they no longer become unsupported-file skips. Non-archive cache checks reuse
   checked metadata instead of swallowing another stat error. EIO/read-dir/
   entry/permission failures prevent Complete coverage. An unreliable missing
   candidate probe also withholds the entire missing reconciliation. Safely
   observed positives remain usable with Partial enumeration.

Migration 22 is unchanged. Migration 23 only adds safety bookkeeping, triggers
and a coverage-generation column. Schema opening does not reconcile archives
or invent historical root bindings. Populated 16/21/22 fixtures test upgrades,
repeat opening and injected migration-bookkeeping rollback. A real-sized
schema-21 copy upgraded to 23 with every original table row unchanged.

The original broad failures were two stale schema assertions and the real
regression treating disabled sources as scan errors. Exact migration/table
inventories now include the legitimate additions; their feature boundaries
remain enforced. Scan All records deliberately disabled sources as Skipped,
without errors or false partial status; targeted unattempted sources remain
NotAttempted/partial. Two further exact schema/table assertions affected by
migration 23 were updated for the same reason.

### Adversarial and full validation

The independent 20 assertions were imported into
`tests/catalogue_health_adversarial.rs` without weakening them. Final result:
**20 passed, zero failed**, including all five process-local injection cases.
`tests/fixtures/catalogue_faults.c` retains disposable `/tmp` guards and also
hooks descriptor-relative probes. Seven additional special cases all passed:
root replacement during enumeration; replacement between preflight and opening
its descriptor; ancestor symlink insertion between both preview and apply probe
phases; bind/remount versus wrong filesystem; duplicate configured bind aliases;
and an actual mount detaching at candidate probing. Mount tests ran in private
user/mount namespaces, never changing host mounts.

Final full `cargo test --offline --locked -p archivefs-core`:
**10,027 library tests passed, zero failed; 21 integration binaries passed
(196 ordinary tests).** The final focused run passed all 15 ordinary independent
cases and 34 ordinary health cases. All 12 special cases skipped by ordinary
Cargo invocation were explicitly executed and passed with the fault shim or
private mount namespace. Remaining unexecuted existing ignores:

- `database::authority::tests::performance_100k_catalogue_and_inventory`
- `identity_source::no_intro::pack_import::tests::manual_real_love_pack_verification`
- `tests::scanner_streams_a_directory_larger_than_the_former_global_limit`

The full suite includes scanner, coverage, database, Arcade, migration and
library-view tests. New cases also cover every symlink position/loops, stale
path/missing/identity/representation/membership changes, source recovery and
reviewed rebinding, root filesystem-ID mismatch, deterministic duplicate-name
bounds and original content preservation. Core doctests passed (zero tests).
Workspace `cargo check --offline --locked --workspace` passed with the same five
existing GUI warnings. `cargo fmt --all -- --check` and `git diff --check` passed.
No GUI tests or GUI source changes were needed.

Reproduce the ordinary assertions with the two integration test targets. For
special cases compile the checked-in C shim with `cc -shared -fPIC -O2 ... -ldl`,
then run the built test binaries individually with `--include-ignored --exact
CASE --test-threads=1`, `TMPDIR=/tmp`, `LD_PRELOAD=<shim>` and
`EMUWIZ_INJECT_FAULTS=1`. Mount cases additionally run through
`unshare --user --map-root-user --mount` with `EMUWIZ_TEST_MOUNTS=1`; plain mount
cases do not require preload. Logs, exact IDs and copy comparisons are retained
under `/tmp/emuwiz-catalogue-health-hardening`.

### Exact real-sized result and preservation

Fresh read-only copies still classify:

| Fact | Rows |
|---|---:|
| Total | 132,064 |
| Present, stale missing | 69,034 |
| Present, clean | 28,899 |
| PresentVerified (existing identity/fingerprint) | 15,658 |
| PresentNotVerified | 82,275 |
| PossiblyMoved | 18,606 |
| Missing | 2,120 |
| OrphanedSource | 13,405 |
| NotChecked | 0 |
| Filename-candidate rows | 30,964 |
| Ambiguous candidate rows | 101 |
| Strong SHA candidate rows | 0 |

All **69,034 IDs** exactly match the independent review's ordered list, not
just its count. Artifact: `/tmp/emuwiz-catalogue-health-hardening/eligible-ids.json`.
SHA-256 of that list encoded with Python `json.dumps`:
`43bfdbc17e5ad0c12857849f6ea23a4a83f849a9fd11a17354165551c6c388b9`.

On a separate fresh copy the final rebuilt driver applied exactly 69,034 changes
and appended 69,034 restoration observations plus one named run. Second preview
proposed zero; repeated apply returned zero. Exact comparisons preserve archive
paths, source IDs, platforms, identity, original observations, source membership
and every untouched archive row, including orphan/move/missing rows. Remaining
missing flags: 20,729. SQLite integrity and foreign-key checks passed on migration
and apply copies. Opening alone changed no original catalogue/evidence rows.
All explicit audit, migration and reconciliation drivers used copies; the real
file was byte-copied and hashed. No migration or reconciliation was directed at
the real database.

Real database before SHA-256:
`2f2a69e31fbd0bd9df80a4eab7e88815472deb17878565ab4b9128ea759e0ab4`.
Real database after SHA-256:
`2f2a69e31fbd0bd9df80a4eab7e88815472deb17878565ab4b9128ea759e0ab4`.

### Nested filesystem boundaries (independent re-review blocker)

The scan walker records every directory whose device differs from its parent's
(a nested mount) in `source_nested_boundaries` (added to unreleased migration 23;
the first observation is never overwritten). On each later scan, and again at the
`mark_unseen_archives_missing` write boundary, every remembered boundary must
still resolve, beneath the pinned root without symlinks, to the identical
device/inode/filesystem type/filesystem ID. A vanished mount, a replacement
filesystem or an uninspectable mountpoint makes the source's coverage `Partial`
with a `nested filesystem boundary ... not proven continuous` diagnostic: no
`last_verified_missing_at` is written and existing evidence is preserved. Coverage
is source-wide (coarse by design). Remounting the same filesystem restores
authority; a replaced one stays refused until it matches again. NO_XDEV probing is
unchanged. Boundaries never observed before this change cannot be recognised, and
the read-only preview does not consult this table.

Final-review hardening of the same mechanism: boundaries are detected by kernel
mount ID (`statx` `STATX_MNT_ID_UNIQUE`, falling back to `STATX_MNT_ID`), so
same-device bind mounts count; a kernel without mount IDs makes the scan partial
rather than assuming no boundary. A boundary is accepted only if the exact mount
the walker saw is still there before and after the record is written; otherwise a
`NULL` (quarantined) record keeps the path unproven on later scans until a
continuous capture succeeds. Continuity is re-checked inside the Missing-writing
savepoint (before the writes and before commit) and at scan commit. The read-only
preview classifies entries beneath an unproven boundary as `NotChecked`, marks the
source `Partial` and lists a diagnostic; the 69,034-ID safe-repair set (hash
`43bfdbc1...c388b9`) is unchanged.

### Performance and limits

Indexing remains once per report. Basename and historical-SHA groups retain at
most 256 detail relationships each, with explicit truncation on oversized
groups and group-level ambiguity retained. The adversarial duplicate-name test
proves deterministic bounded details rather than a Cartesian-product explosion.
No relink, deletion or filename identity is introduced. Optional SHA reads have
a 256 MiB declared-byte budget and abort if a file grows past its observed size;
no payload was hashed on the real corpus (it has no archive hashes).

Measured previews include 9.30 seconds during the first copy apply, 12.43 seconds
with concurrent compilation, and 16.73 seconds for a fresh-copy run with substantial I/O waiting. Final
copy apply: 5.47-second preview and 12.17-second application. Peak standalone
preview RSS was approximately 312 MiB versus the independent 281 MiB baseline,
reflecting retained filesystem/preview bindings; CPU time remained close to the
baseline. Cold I/O explains the slower wall-clock observations, rather than
quadratic candidate work. The final warm repeat took 5.52 seconds, with 3.33 seconds user CPU and 2.06
seconds system CPU. Timing logs retain the slower observations too. No source payload hashing or recursive per-row directory enumeration occurs.

Linux `openat2` support is required; unsupported kernels fail closed without a
weaker fallback. Filesystem identifiers are metadata continuity evidence, not
cryptographic game identity: indistinguishable cloned filesystems need external
review, and unstable network/removable bindings may require reviewed rebinding.
Physical removable-device/reboot/NFS QA was not performed; actual isolated bind,
wrong-filesystem and detach tests were. GUI projection remains Pass 2. Pending
mount health, existing identity semantics and Cheat Core APIs remain unchanged.

## Original repair record (historical validation)

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
  DATABASE_COPY.sqlite3 \
  /home/davedap/.config/archivefs/config.toml
```

The example accepts only database/config paths and has no apply option.

## Deferred GUI work

No GUI file or wording changed. Pass 2 should project this classification into
Home, Browse, Game Details, Check Games and Problems & Repair; keep never-checked
identity neutral and candidates distinct from broken files. The stale empty
Problems summary/loading-generation bug, review/relink/removal flows and archive
integrity checks remain outside this pass. No Cheat Core branch was integrated.
