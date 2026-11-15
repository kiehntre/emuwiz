# Project Ouroboros: transactional library transformation and recovery

Status: first slice. It covers verified relocation of one loose game file
between two enabled `games` library roots, on the same filesystem or across
filesystems. The code is in `crates/archivefs-core/src/transform/`; the CLI is
`emuwiz-cli transform`.

**Production apply does not exist in this build.** Apply, recovery and Undo
need a test-only `ScratchAuthority` (section 2.1), which release and ordinary
builds cannot construct, and they run only on supported ext4 filesystems
(section 4.4). Nothing in a game folder, a setting or an environment variable
grants execution. Real libraries would also need `errors=remount-ro` on both
drives.
Planning and preview work on any catalogue. Production cross-drive relocation
and retirement of production originals need separate owner approval after an
independent Tier-A review.

## 1. What already existed and is reused (gap analysis)

| Need | Existing facility (reused) | Gap closed here |
|---|---|---|
| Content-bound file identity | `rename_apply::identity::{capture_identity, identity_matches, content_identity_matches}`: SHA-256 plus full-precision mtime, size and inode/device; bounded reads; no symlink follow | none |
| Descriptor-bound directory operations | `rename_apply::dirsync::RenameDirs`: per-component `O_NOFOLLOW` walk from a trusted root, `renameat2(RENAME_NOREPLACE)`, directory fsync and re-verification | added `open_dir_beneath`/`entry_in` wrappers so mkdir, unlink and rmdir use the same walk |
| Catalogue path move | `database::organisation_paths::{bind, begin, CatalogueMove}`: DB-file and root identity, row CAS, size/mtime staleness, `scan_fingerprints` and `dat_set_audit_results` projections, reverse direction, "already committed" detection | exposed `already()`, the binding key, `catalogue_root` (read-only) and `audited_set_dependencies` (read-only) |
| Root identity | `catalogue_health::SourceRootBinding` (device, inode, filesystem type and id) | none |
| Set structure | `game_package::{descriptor_references, is_descriptor}` (CUE, GDI, M3U) | dependency graph with explicit unknowns (`transform::graph`) |
| Same-filesystem organise moves | `rom_organisation` + `rename_apply` | **cross-filesystem moves were refused** ("no copy+delete fallback"); the copy, verify, publish, commit and retain flow is new |
| Copy staging | `rename_apply::copy` (copy into a private directory, no-clobber publish) | had no source retirement, no catalogue step and no multi-resource recovery |
| Journals | `rename_apply::journal`: one mutable JSON per transaction, entry states | a step-level, append-only, hash-chained journal with intent/done records. The rename journal is unchanged and still used by Organise |
| Locks | per-feature `flock`s; writer gate | per-resource locks for transformations (`transform::lock`) |

Confirmed defects found in existing code: none that block this slice. Notes:

- `PackageResolver` deliberately ignores unreadable descriptors when
  attributing members. `transform::graph` instead treats an unreadable
  descriptor as an *unknown* dependency and blocks the move.
- No scan-wide lock exists. Scans are detected through the catalogue CAS and
  the unique-path constraint, not by locking.

Leviathan (issue #279) owns DAT evidence and schema 27. This
slice adds **no migration**. Saved DAT and verified-identity proof is bound
to path and file identity, so after relocation it reads non-current and must
be re-checked. It is never carried over, and the preview says so.

## 2. Protocol

A plan is immutable JSON. Its SHA-256 is the digest the user approves.
`apply` refuses any plan whose bytes differ from the approved digest. The
plan binds:

- the source's content proof;
- both roots' filesystem identities;
- the catalogue binding;
- the dependency-graph digest;
- the step list and point of no return;
- the retention policy;
- the effects the user was shown.

Journal: `<data>/transform-journals-v1/<txn>/`. It contains `plan.json`
(create-new, synced, mode 0400) and `log.jsonl`, where every record is
`fdatasync`ed and chained: each record carries `seq`, the previous record's
sum (the first carries the plan digest) and its own SHA-256 sum.

- A final line that is incomplete or unparseable is a **torn tail**. Recovery
  keeps its bytes as evidence (`RecoveryStarted.torn_tail_bytes`), truncates
  it and continues.
- Any other defect makes the journal **Corrupt**: a broken chain, a bad
  checksum, a gap, an unknown version, a non-canonical or edited plan, a
  missing file, or a claimed Complete with steps unfinished. Nothing acts on
  a Corrupt journal.

Each step writes `Intent`, does its work, then writes `Done{evidence}`. Every
step decides by *probing* the world. A crash between a syscall and its Done
record is resolved by re-running the step, which finds its effect either
present or absent.

Cross-filesystem relocation (`CopyVerifyRetire`):

| # | Step | Durability and verification |
|---|---|---|
| 0 | Check preconditions | Roots unchanged. Source re-hashed and matches the plan. Destination absent. Catalogue binding valid. Dependency digest unchanged. |
| 1 | Create private staging dir (destination side) | `mkdirat 0700`, then the parent is fsynced |
| 2 | Create private retirement dir (source side) | Created before commit, so a permission problem is found while the move can still be compensated |
| 3 | Copy to staging | Pinned source fd: dev/ino/size/mtime are checked. The bytes read are hashed while copying. The full-precision mtime is preserved. File and directory are fsynced, then the page cache is dropped (`POSIX_FADV_DONTNEED`). |
| 4 | Verify staged copy | Staged copy re-read and its SHA-256 compared. Source re-hashed. |
| 5 | Publish | `renameat2(RENAME_NOREPLACE)`, then both directories are fsynced and the identity is re-checked |
| 6 | Commit catalogue | Destination re-checked and source re-hashed immediately before the CAS (I6). Shared `CatalogueMove`. |
| 7 | Retire original **(point of no return)** | Original identity re-hashed. No-clobber rename into the retirement dir *on its own filesystem*. The original is never deleted. |
| 8 | Remove staging dir | `rmdir` only while it is empty |
| 9 | Final verification | Destination content and inode, source absent, retained original re-hashed, catalogue names the destination, no private dirs left |

Same-filesystem relocation (`AtomicRename`) runs four steps: check, one
no-clobber rename, commit (point of no return), verify. A rename and a
SQLite commit cannot be one atomic action. If the process dies between
them, the catalogue row still names the old path while the file is at the
new one. The journal shows the transaction as Interrupted (never Complete),
and recovery finishes the commit. Organise has the same window today. The
cross-filesystem protocol has no such window, because it commits before it
retires the original.

Every step that *finds* its effect already present (after a crash between
the syscall and its fsync) fsyncs the directories involved before it records
Done. A crash can leave a mutation visible but not durable, and recording it
unsynced would let a later power loss revert it after the outcome was
recorded. The model checker found this, and `already_present_without_fsync_is_caught`
is its negative control.

Direction rules:

1. With no failure or cancel, apply and recovery move **forward**. A crash
   alone always resumes to Complete.
2. A step that refuses before the point of no return switches to
   **compensation**: attempted steps are reversed in reverse order, each by
   probing.
   - The catalogue is reversed before the published copy is withdrawn, so
     the catalogue never names an empty location.
   - A copy is removed only while the original is proven intact (a
     byte-identical duplicate). Otherwise it is **retained** and listed as
     residue.
3. After the point of no return the engine only moves forward. A problem
   becomes **Blocked**, with the facts known to hold. Recovery can be
   retried after review.
4. A step that cannot classify the world is Blocked in either direction.
5. Cancellation is honoured only before the point of no return.

Undo is its own journalled transaction (`transform::undo`):

- Cross-filesystem: the retained original is restored by an atomic rename on
  its own filesystem. The relocated copy is moved into a private directory and
  deleted only while the restored original is verified (point of no return).
- Same-filesystem: a reverse rename.

Undo is offered only while all of these hold (I8):

- the relocation is Complete;
- no other Undo of it is complete or pending;
- the relocated file still has exactly the published identity;
- the original location is free;
- the retained original still matches;
- the catalogue still names the destination.

Concurrency:

- Every apply, recovery and Undo holds an `flock` per resource path (sorted,
  non-blocking) plus one on its journal directory. Overlapping
  transformations are refused, and a dead holder's locks are released by the
  kernel.
- Catalogue writers (scans, other EmuWiz processes) are detected by the CAS
  and the unique-path constraint.

Retention: `RetainUntilReleased`. A retained original stays byte-for-byte at
`<source folder>/.emuwiz-retired-<txn>/payload` until a person explicitly
releases it. **No release (deletion) operation exists in this build.**

### 2.1 Execution authority

Every mutating entry point (`apply`, `recover`, `recover_all`,
`apply_undo`) requires a `ScratchAuthority` in its `Context`.

- **Compile-time, not a feature:** the authority type is crate-private,
  and every way to construct it is `#[cfg(test)]`. rustc sets `cfg(test)`
  only when compiling `archivefs-core`'s own unit-test binary, never for a
  dependency. So no build of `emuwiz-cli` or Blue can contain or reach it
  (debug, release, any features including `--all-features`), and neither
  can their test binaries, core's integration tests or its doctests.
  `Context::with_authority` and the context's authority field are
  crate-private as well. Outside the crate `Context::new` is the only
  constructor and carries no authority. `apply`, `apply_undo`, `recover`
  and `recover_all` therefore refuse with `PRODUCTION_UNAVAILABLE` as their
  first action, before reading the plan or the journal. A separate test-only
  crate was rejected: core would have to export a public constructor for
  it, which the CLI could then call too. The benchmark is an ignored unit
  test for the same reason (`transform::bench`), not an example behind a
  feature. Proof:
  - `compile_fail` doctests in `transform/authority.rs`. Doctests compile
    as an external crate, so these hold under any feature set. They show
    that the type cannot be imported by either path, that a context cannot
    take an authority, that its field is private, and that the scratch
    planner is absent. A passing doctest checks that the same paths are
    otherwise valid.
  - `tests/ouroboros_executor_boundary.rs` links the library as the CLI
    does. A structurally valid, correctly digested plan, forged by hand with
    real directories and either policy, is refused by `apply` and
    `apply_undo`. An interrupted journal written with the public journal
    writer is refused by `recover` and `recover_all`. Nothing is written or
    appended.
  - Symbol checks on the compiled artefacts (section 4.6): the
    `ScratchAuthority` constructors are present in core's unit-test binary
    (positive control) and absent from the CLI and Blue binaries built debug,
    release and `--all-features`, and from their test binaries.
  - `scratch_authority_is_unreachable_from_production_builds` (defence in
    depth): no Cargo feature gates the authority, its gates stay
    `#[cfg(test)]`, the type stays crate-private, the release build command
    uses no feature or all-targets flag, and no production source names it.
- **Trusted state outside the roots:** the authority creates each scratch
  library root itself: a fresh 0700 directory owned by this user, at a
  canonical path, never adopting an existing one. It keeps the root's
  (device, inode, owner) in memory. Planning, and every step of execution
  and compensation, re-check that each plan root is still that directory.
  A replaced directory, a symlink, a bind mount over the path, or a remount
  on another device is refused. Before the point of no return the move
  compensates; if compensation is refused too, the transaction is Blocked.
- **Mount identity, every step:** the library root binding (device, inode,
  filesystem type and the `statfs` filesystem id, so a reformatted drive is
  a different root) and the mount facts recorded at planning must still
  hold. Those facts are mount point, device, type, per-mount and superblock
  options, and the ext4 driver's options. A remount with any changed option,
  a read-only bind on top, or a mount stacked over the root changes them and
  is refused. A same-device bind mount nested over a game folder keeps the
  device number, so the folder's own mount-table entry must be the root's
  mount. Several entries for one filesystem at one mount point that
  disagree on options are refused as uncertain. These checks are path-based
  and run before each step, so a change made between a check and the
  following system call is caught at the next step, not prevented.
  Directory operations then use descriptors opened from the root with
  `O_NOFOLLOW` per component.
- **Only planner-shaped plans execute:** the steps, point of no return and
  retained-original path are a pure function of the plan's kind, strategy,
  transaction ids, roots and game paths (`transform::shape`). The engine
  re-derives them at apply, at recovery and before every step, and refuses
  any difference. It also refuses a malformed transaction id, and a game
  path outside its root. So an edited `plan.json`, including one read back
  from a journal, cannot rename or unlink anything outside the two admitted
  roots or anything the planner would not have touched.
- **Bound to its own evidence:** the plan's catalogue must be the database
  its catalogue binding writes, and an Undo plan must match the relocation
  it reverses (`undo_matches_relocation`). That means the relocation's
  recorded plan digest and outcome checksum, and every field the Undo
  planner copies from it: catalogue and binding, roots, both paths, the
  original's identity, strategy, retention, retained path, policy and the
  published copy's recorded identity. This is checked before every step,
  so an Undo can only ever discard the copy its own relocation published
  (I9), never another file that merely has the same content.
- **Child-process tests:** trust flows one way. A child's environment names
  only two parent folders and the scenario. The child builds a fresh
  fixture inside them (new roots, a new game file, a new catalogue) with
  its own new authority, so whatever its environment says, it can only move
  a file it has just created. The parent test adopts the child's roots from
  the child's stdout pipe, and only as directories inside the folders the
  parent made for it. Recovery after the child's death runs in the parent.
  An earlier version let the child adopt roots, with their device and
  inode, from its environment. Independent review showed that this let the
  core test binary move real games, and it was removed.
- **No marker:** the earlier `.emuwiz-disposable-fixture` marker was
  removed. A copied or forged marker grants nothing (a test copies one into
  both roots and confirms the refusal).
- **Test filesystem policies (owner decision 2026-11-14, "Approve bounded
  test exception"):** an ordinary scratch authority selects `TestScratch`.
  This accepts ext4 `errors=continue` (common on development drives) and nothing else
  that production refuses. A strict scratch authority selects `TestStrict`,
  which is exactly the production policy, `errors=remount-ro` included, for
  disposable strict fixtures (section 4.3). Every other filesystem, mount,
  journalling, barrier and flush check is identical under all three
  policies, and the engine's step sequence and fsyncs do not depend on the
  policy. A plan's policy is bound into its digest. An authority executes
  only plans of its own policy, and no authority executes a Production
  plan, so test mode is never a route to moving a real game.

A future production move needs its own authorisation model, an independent
Tier-A review and separate deployment approval. Nothing here is a switch for
it.

## 3. Invariants

| | Enforcement | Counterexample tests |
|---|---|---|
| I1 original retired only after a verified successor | Retirement is step 7: after publish (synced and verified) and after commit; the original is re-hashed just before. Retirement is a rename into retention, never an unlink. | model: `checker_rejects_retire_before_commit` (negative control), every explored state; fs: `assert_safe` after every injected crash, abort and SIGKILL |
| I2 Done records carry evidence | `StepEvidence` on every Done; Complete only after `FinalVerify` re-proves the end state | `crash_everywhere` checks the end state for every crash point |
| I3 recovery repeatable without new changes | Probing steps; a terminal journal is returned with nothing appended | `complete_journey` and `crash_everywhere` compare record counts; model I3 check |
| I4 stale plans refused | Digest approval; source re-hashed at check, copy, verify, commit and retire | `tampered_or_unapproved_plans_are_refused`, `source_replaced_after_planning_is_refused` (same size and mtime, different bytes) |
| I5 dependencies not discarded | Graph digest bound in the plan and recomputed at check; set members, descriptors and unreadable descriptors block | `dependency_change_after_planning_is_refused`, `set_members_descriptors_and_unreadable_descriptors_block_relocation` |
| I6 user edits never overwritten by stale evidence | No-clobber renames; re-hash before commit and retire; Undo refuses an edited relocated file | `source_modified_during_copy_is_never_published`, `source_modified_after_commit_before_retirement_is_preserved`, `destination_appearing_is_never_overwritten`, `undo_is_withheld_when_its_preconditions_fail` |
| I7 Complete never early | `Outcome::Complete` only after every forward step is Done; `History::fold` rejects a journal claiming otherwise | model Complete-state check; Corrupt on a forged Complete |
| I8 Undo only when preconditions hold | `plan_undo`, `forward_still_undoable`, and Undo step 0 re-checked at apply | `undo_is_withheld_when_its_preconditions_fail`, `complete_journey` (no second Undo) |
| I9 ambiguity never authorises deletion | The only unlinks are of files inside the plan's own 0700 directories, with recorded identity and a verified keeper; content equality alone authorises nothing | `identical_content_with_a_different_owner_is_untouched`, graph ambiguity test |
| I10 uncertainty explicit | `Blocked`, `Corrupt`, `Interrupted`, `Running`; only `Complete` is success | corrupt-journal tests, storage-disappearing test, model "no outcome without interference" check |

## 4. What is modelled and what is proven on real filesystems

### 4.1 Durability assumptions (D1 to D5)

The model checker (`transform::model_check`) explores every interleaving of
engine micro-actions, process crashes, power losses, bounded user
interference (a source edit, a file created at the destination, with and
without the user fsyncing the folder) and cancellation. It checks the
invariants in every reachable state, under these assumptions and no others:

| | Assumption | Basis on supported ext4 |
|---|---|---|
| D1 | A directory-entry change (create, unlink, either half of a rename, mkdir, rmdir) is durable only after an fsync of the directory holding it. A file fsync makes **no** directory change durable, and one directory's fsync says nothing about another. | Deliberately weaker than ext4, where an fsync commits the whole jbd2 transaction. The engine never relies on that. |
| D2 | After a crash, a rename has happened entirely or not at all. | A rename is a single jbd2 metadata transaction. |
| D3 | Per-name linear history. After a crash each name holds one of the values it was given, and a durable later change to a name implies its earlier changes are durable. Changes to *different* names are unordered: any subset may be lost. | Weaker than ext4's in-order transaction commit. A name cannot durably hold a newer value while an older one is lost; that would be namespace corruption, which the journal exists to prevent. |
| D4 | A file's bytes are durable after its own `fsync`. A user's edit is a data write to the original inode. | `data=ordered` or `data=journal`, with barriers on. |
| D5 | A SQLite commit is atomic. It is visible as soon as SQLite unlinks the rollback journal, and durable only once the catalogue's folder is fsynced (a D1 directory change). | SQLite's own atomicity. Durability comes from `synchronous=EXTRA` at commit and an explicit persist when a commit is found already visible (section 4.4a). |

How the engine meets them:

- every mutating step fsyncs the directories it changed before writing its
  `Done`;
- every step that *finds* its effect already present fsyncs those
  directories too;
- a copy is fsynced (data) and so is its directory;
- a catalogue commit fsyncs the catalogue's folder (`EXTRA`), and a step
  that finds the commit already visible (recovery, final verification)
  fsyncs the database, any `-wal` and the folder before recording it;
- `Complete` and `Compensated` require every engine mutation to be durable.

Negative controls show the checker would catch a violation:

- `checker_rejects_retire_before_commit`
- `unsynced_publish_is_caught`
- `already_present_without_fsync_is_caught`
- `regression_ce3_rename_never_clobbers`
- `directory_changes_need_their_own_directory_fsync`
- `already_committed_catalogue_without_persisting_is_caught`

### 4.2 Counterexamples found on 2026-11-13 and what changed

The checker reported eight counterexamples. Each one has a regression test
(`regression_ce*` in `model_check.rs`).

| # | Counterexample | Classification | Change |
|---|---|---|---|
| CE-1 | Recovery found the published copy in place after a crash before its fsync, recorded Done and committed the catalogue. A power loss then reverted the publish, and the catalogue named an empty path. | **Engine** probe paths and model | The engine fsyncs on every "already present" path: an existing private dir, a removed dir, a removed duplicate, compensation's "never moved" rename paths, and a missing staged copy. The rename "already in place" path already did. The model mirrors this. |
| CE-2 | The same defect in compensation after a cancel. The staging dir was removed, then found gone without an fsync, and `Compensated` was recorded with the dir able to reappear. | **Engine** | as CE-1 |
| CE-3 | A destination created between the probe and the rename was overwritten. | **Model** only | The engine already used `RENAME_NOREPLACE`; the model's rename now refuses a present target and re-checks the moved identity, as the engine does. |
| CE-4 | A user's edit was lost after a forward rename and its compensating rename back, both unsynced: the power loss reverted only the first. | **Model** artefact | The old model allowed any subset of pending changes to be reverted, restoring a stale value over a later change to the same name. This is physically impossible under D3. And the edit is a data write to the inode (D4), so a revert of a rename moves the *edited* inode. |
| CE-5 | A user's file at the destination was removed: the publish and the withdrawal were unsynced, and the user created a file. | **Model** artefact | As CE-4, by D3. The user's create is *not* assumed durable without their own directory fsync (D1); both cases are explored. |
| CE-6 | Compensation retained an unsynced partial copy as residue, and a power loss could remove a path the outcome listed. | **Engine** | The retained copy's directory is fsynced. |
| CE-7 | A scan inserting a row at the destination made compensation of the commit Block. The shared `CatalogueMove` refuses any row at the other path. | **Engine** | Compensation reads where our own row points (`bound_row_at`; it writes no row, and persists the state it reports, CE-9). Organise's `CatalogueMove` is unchanged. |
| CE-8 | Same filesystem: a crash between the rename and the commit leaves the row naming the old path. | **Inherent, documented** | The test invariant was narrowed for `AtomicRename`. The window is journalled as Interrupted and closed by recovery (section 2). |

**CE-9 (independent review, 2026-11-14).** A process died between
SQLite's journal unlink and the directory fsync. Recovery saw the row at its
target and recorded the commit Done (the engine), or went on to record
Applied (Organise recovery). Both then retired the original or withdrew the
copy, and a later power loss rolled the row back. The model treated a
commit as atomic **and** durable, so it could not see this.

- **Classification:** engine, Organise and model.
- **Change, engine and Organise:** `CatalogueMove::open` persists any
  already-visible state before returning it.
- **Second review (same day):** two more readers acted on visible state.
  The engine's compensation probe (`bound_row_at`) and Organise's
  folder-move recovery (`folder_rows_moved`) now persist the same way
  before anything is recorded. The model's compensating commit already
  synced the catalogue, so this brings the engine in line with the model.
- **Change, model:** a commit is now visible before it is durable.
- **Tests:** a negative control (`already_committed_catalogue_without_persisting_is_caught`,
  which disables the sync on every probe, forward and compensating), and
  `recovering_a_folder_move_with_library_rows_makes_the_library_durable_first`.

An intermediate fix for CE-4 and CE-5 (2026-11-13, earlier the same day)
made a user's action commit the pending changes in its directory, i.e.
whole-journal ordering. It was withdrawn after review because it assumed a
file fsync persists earlier directory changes. The model now uses D1 to D3
instead. The adversarial space grew from 162,471 to 225,810 states, because
unsynced user files are now explored too.

### 4.3 Real filesystems

The real-filesystem tests run the Rust implementation on two genuinely
separate ext4 filesystems:

- the temporary directory;
- a folder beside the Cargo target directory, when that is on another
  filesystem, or
  `EMUWIZ_OUROBOROS_SECOND_FS`.

tmpfs is no longer used: it is not a supported filesystem. The tests run
in-process and in child processes, with:

- crashes at every point, and crashes during recovery;
- `abort()`, and `SIGKILL` during a 48 MiB copy;
- real kernel write failure from `RLIMIT_FSIZE` (EFBIG);
- `EACCES` from a read-only folder;
- injected ENOSPC, EACCES and EIO;
- a simulated read-only remount before retirement, a vanished mount and
  changed mount options mid-copy (`fscompat::test_override`).

They do **not** cut power to a real device. Page-cache contents survive a
process death, so D1 to D4 on real hardware are covered only by the model
plus the filesystem's documented behaviour. A true ENOSPC, a real remount and
a power cut need root or dedicated hardware. They are listed as follow-ups,
not claimed.

**Strict fixtures (production eligibility policy end to end).** Three
disposable 256 MiB ext4 images are loop-mounted `nosuid,nodev,noexec` under
a scratch folder (`<fixtures>/mnt/`). They hold no game,
catalogue or production data, and no production mount was changed:

- `strict-a` and `strict-b` are made with `mkfs.ext4 -e remount-ro`;
- `continue` is otherwise identical, made with `-e continue`. The driver
  options differ only in `errors=`.

Run with `EMUWIZ_OUROBOROS_STRICT_FS=<strict-a>:<strict-b>` and
`EMUWIZ_OUROBOROS_CONTINUE_FS=<continue>`:

- `strict_filesystems_end_to_end` completes a cross-filesystem and a
  same-filesystem move under `TestStrict` (the full production policy).
- `strict_policy_accepts_the_remount_ro_fixture_and_refuses_its_continue_twin`
  shows four things. Production and `TestStrict` accept `strict-a` with no
  notes, and refuse the `continue` twin for `errors=continue`. A strict
  authority cannot plan a move onto the twin. The ordinary production
  planner on the fully eligible strict fixture produces a Production plan
  that neither the ordinary context nor the strict authority will execute.

Results are labelled as follows. Process-crash recovery is proven where
tested, on `errors=continue` and on the strict fixtures alike. **Actual
power-loss durability is not established**: a loop device on a host drive
is still not a power cut.

### 4.4 Filesystem compatibility (fail closed)

`transform::fscompat` reads the mount table and `/proc/fs/ext4/<dev>/options`
at planning time and **before every step**, including the retirement of an
original. It never assumes a filesystem from a path.

| Filesystem / configuration | Planning and preview | Apply |
|---|---|---|
| ext4, rw, journal in use, `barrier`, `data=ordered` or `data=journal`, **`errors=remount-ro`** | yes | no executor in production builds (section 2.1); under test, the strict fixtures (`TestStrict`) |
| same, but `errors=continue` | yes, with the blocker explained | **refused** under the production and strict policies; accepted only under the test-only scratch authority (`TestScratch`), which says so in every preview |
| `errors=panic`, error behaviour unknown, ext4 options unreadable, `data=writeback`, `nobarrier`, `norecovery`/`noload`, read-only | yes, blocked | refused |
| ext2, ext3, tmpfs, FUSE (ratarmount, mergerfs, rclone, altmount), NTFS, exFAT, vfat, NFS/CIFS, overlayfs | yes, blocked | refused |
| a game or folder on a mount nested inside its library root (for example a FUSE archive mount inside a games folder) | yes, blocked | refused |
| ext4 with no journal (no jbd2 entry in `/proc/fs/jbd2`) | yes, blocked | refused |
| the **catalogue's drive** or the **transaction journal's drive** failing any row above | yes, blocked | refused: retirement depends on that commit and those records being durable |

A change of filesystem type or options during an operation, a change of
scratch-root identity, or a mount that disappears stops the step. Before the point
of no return the engine compensates. If compensation is also refused (the
drive is still read-only), the transaction is **Blocked** with its evidence
kept, and recovery resumes after the administrator restores the drive. The
mount option is a prerequisite, not a substitute: explicit fsyncs,
journalling and content re-verification stay in force.

### 4.4a SQLite durability (D5) and Organise

D5 is established, not assumed. The library catalogue is in rollback-journal
mode: its file header's format bytes are 1/1 and there is no `-wal` file.
In that mode a commit takes effect when the journal file is unlinked. At
SQLite's default `synchronous=FULL` that unlink is **not** directory-fsynced;
only `EXTRA` adds the directory fsync. So a power loss shortly after a
commit can resurrect the journal and roll the commit back.

Organise records a move as `Applied` (temp file, fsync, rename and directory
sync of its journal) *after* that commit. Under D1/D3 the two are unordered,
so the journal could say Applied while the catalogue still names the old
path:

- the file is at its new path and the row names the old one;
- Undo refuses ("catalogue association changed"), and the next scan sees a
  missing row plus a new one;
- this is a catalogue/file disagreement, **not** loss of the game file;
- process death cannot cause it, because the unlink is already in the page
  cache: only power loss or a kernel crash can. On ext4 the jbd2 commit
  order makes it unlikely in practice, but this design does not rely on
  that.

Fix: every organisation path move commits on `open_durable_connection`:

- Organise single-file moves, its recovery and Undo, and transformations,
  all through `CatalogueMove`;
- game-folder moves through `FolderCatalogueMove`.

That connection refuses a non-persistent journal mode and sets
`synchronous=EXTRA`. The cost is one directory fsync per commit. The
journal mode is never changed and the database is not migrated.

EXTRA covers a commit that completes. It does not cover recovery that
*finds* a commit already visible: the process may have died after the
journal unlink and before the directory fsync (CE-9). So every reader that
recovery acts on makes what it saw durable first: it fsyncs the database
file, any `-wal` or `-journal` (TRUNCATE and PERSIST modes commit by
changing that file), and the catalogue's folder. This applies to
`CatalogueMove::open` whenever it reports the row already at its target,
to the engine's compensation probe, and to Organise's folder-move recovery,
in every case before Organise recovery or the engine records anything.

Preview validation (`bind`, shared by Organise and transformations) now runs
its checks on a **read-only** connection in a deferred transaction. It no
longer takes SQLite's write reservation on the catalogue.

The catalogue's drive is part of the filesystem gate (section 4.4): its
facts are bound into the plan and re-checked before every step, under the
same policy as the library roots. So is the transaction journal's drive.

### 4.5 Preparing a drive (for the administrator; EmuWiz never does this)

EmuWiz never remounts a drive, edits `/etc/fstab`, runs `tune2fs` or `fsck`,
or changes host configuration. To assess a drive:

```sh
findmnt -no SOURCE,FSTYPE,OPTIONS -T <library mount>
cat /proc/fs/ext4/$(basename "$(readlink -f "$(findmnt -no SOURCE -T <library mount>)")")/options
sudo dumpe2fs -h /dev/sdX1 2>/dev/null | grep -Ei 'errors behavior|filesystem state'
sudo dmesg | grep -i 'ext4-fs' | tail
```

To change it, at a maintenance window with nothing using the drive (stop
EmuWiz, library scans and every other service using it first):

1. Check the filesystem state is `clean` and the kernel log shows no ext4
   errors. If it does not, repair first (`sudo umount`, then `sudo e2fsck -f`),
   not after.
2. Choose one of:
   - add `errors=remount-ro` to the drive's options in `/etc/fstab`; or
   - set the superblock default with `sudo tune2fs -e remount-ro /dev/sdX1`.
3. Remount, or reboot, and re-check with the first two commands above. The
   ext4 `options` file must list `errors=remount-ro`.

Note: a drive that appears more than once in the mount table (the same
device and peer group, a stacked mount) should be reviewed at the same
window.

### 4.6 Compiled-artefact checks (executor boundary)

`scripts/check-ouroboros-executor-boundary.sh` reads compiled files and
builds nothing. It looks for the authority's constructors (`new`, `strict`,
`provision_root`, `adopt_from_child`), `Context::with_authority` and
`plan_relocation_scratch`:

- in binaries, through the demangled symbol table, failing closed on a
  file without one;
- in rlibs, through the demangled symbol tables of their object code,
  failing closed without one. Item-name text in rlib metadata is reported
  for information only, because the crate's doc comments and compile_fail
  doctests name these items on purpose.

The crate-private type and its read-only `admits` check exist in every
build by design, and are reported for information only. Core's own
unit-test binary is the positive control, which shows the check can see
the symbols. It covers binaries only: no test build produces an rlib.

These checks are supporting evidence, not the boundary. A small or
`#[inline]` function can exist only as MIR, with no symbol, and an inlined
call leaves none in a binary either. The boundary itself is `#[cfg(test)]`
on every constructor, on `Context::with_authority` and on
`plan_relocation_scratch`, together with the compile_fail doctests in
`transform/authority.rs`.

Results (2026-11-14, on the commit that added this section, toolchain
1.97.1):

| artefact | constructors / attach |
| --- | --- |
| core unit-test binary (positive control) | 12, present as required |
| core integration-test binary (`ouroboros_executor_boundary`) | 0 |
| `archivefs-cli`, `archivefs-gui` and `emuwiz-cli` test binaries | 0 |
| `emuwiz`, `emuwiz-cli`, `emuwiz-r2`: debug, default features | 0 |
| the same: debug, `--workspace --all-features` | 0 |
| the same: release (`build-release.sh` command) | 0 |
| six core rlibs from those builds (object symbols) | 0; 3 doc-text mentions each, informational |

The compile_fail doctests (6) and the external-caller integration test (3)
passed in the same run.

### 4.7 Production enablement prerequisites (owner directive 2026-11-14)

Production relocation stays blocked unless the source, destination,
catalogue and journal filesystems all meet the strict policy. Nothing below
is done automatically: EmuWiz does not change mount settings, does not move
the database and does not enable execution. Any enablement plan must cover:

1. **Choose between two routes.**
   - *Route A:* change the existing ext4 filesystems to `errors=remount-ro`:
     the root filesystem (which holds the catalogue and the journal) and
     each library drive.
   - *Route B:* keep `/` as it is, and move the catalogue and the journal
     root to a dedicated ext4 volume that meets the policy.
2. **Root read-only impact.** With Route A, an ext4 error on `/` remounts
   the whole OS drive read-only. Other services, logs, package
   updates and EmuWiz itself would then stop writing until repair and
   reboot. Route B confines that behaviour to the volume EmuWiz needs.
3. **Verify the real mounts first.** Record the mount identity and the
   effective options (`/proc/self/mountinfo`, `/proc/fs/ext4/*/options`,
   `/proc/fs/jbd2`) for every drive involved. Resolve any stacked
   mount of a library drive before relying on it.
4. **Durability stays as it is.** Keep SQLite in rollback-journal or WAL
   mode with `synchronous=EXTRA` on every move connection. Keep the
   explicit file and directory fsync ordering (D1 to D5), and keep recovery
   (journal, compensation, Blocked) unchanged. Re-run the strict-fixture
   tests and the model checker against the final configuration.
5. **Backup, rollback and recovery.**
   - Before: a verified catalogue backup (SQLite online backup, integrity
     check, checksum), and a filesystem `clean` state with an empty error
     log.
   - Afterwards, with a documented rollback: the previous mount options or
     the previous catalogue location, and the steps to restore them.
   - A rehearsal of recovering an interrupted move after a remount
     read-only.
6. **A separate authorisation model, an independent Tier-A review and
   separate owner approval** for the production executor itself (section
   2.1). The strict policy is necessary, not sufficient.

## 5. Limitations (explicit)

- One loose file per transaction. CUE, GDI and M3U sets, game folders,
  archive members, arcade sets with DAT-declared dependencies, and BIOS,
  save and memory-card roots are blocked.
- Saves, patch provenance, per-game emulator settings, RetroArch and ES-DE
  playlists and RomM are not modelled. The preview says so; they are not
  rewritten.
- Verification re-reads after `POSIX_FADV_DONTNEED`, which narrows but cannot
  guarantee device-level read-back.
- Probing re-hashes whole files, so each large file is read several times.
  This is correctness first; see the benchmark report.
- Undo eligibility and status scan every journal (O(transactions)).
- A transaction killed before `plan.json` was durably written leaves a
  directory that reads as Corrupt (nothing was changed).
- No production executor: in this build the engine executes only under the
  test-only scratch authority (section 2.1).
- Only ext4 (section 4.4). This host's drives, including `/` where the
  catalogue lives, use `errors=continue`. A production move would therefore
  be refused even after approval, until the prerequisites in section 4.7 are
  met.
- The filesystem and mount checks are path-based and run before each step.
  A change made between a check and the following system call is caught at
  the next step, not prevented. Directory walks use `O_NOFOLLOW` per
  component, but not `RESOLVE_NO_XDEV`.
- The journal root's drive is policy-checked before every step, but its
  mount facts are not bound into the plan (the catalogue's and the roots'
  are).
- Locks and the one-Undo-at-a-time rule hold within one journal root. Two
  EmuWiz installations with different journal roots are not coordinated.
- A cancel after a torn point-of-no-return record in an Undo leaves it
  Blocked, although the files are already in their final state. It needs
  an operator's review.
- Under `errors=continue`, an fsync that follows an earlier, unnoticed I/O
  error can report success for data that was lost ("fsyncgate"). This is
  one reason production requires `errors=remount-ro`. Test results under
  `TestScratch` are not power-loss evidence.
- Injected test crashes are recognised by a marker in the error text. A
  real I/O error carrying that exact text would be treated as a crash
  (stop and recover later), not as a refusal.
- The Blue preview page re-hashes the file on each refresh.
- DiscardDuplicate (Undo's removal of the verified duplicate copy) is not
  in the model checker. Its ordering was reviewed by reading, and it is
  covered by the fault-injection tests.
