# ES-DE publication preservation: core repair

Baseline: a1b5e2a972489346243ced99441ed6aa19e6460b. This is a core-only candidate;
expanded frontend publication remains blocked pending coordinated GUI/RetroDECK
orchestration repair. No GUI files are changed by this candidate.

## Confirmed root causes

The prior apply path wrote the preview's replacement without checking its original
bytes or expected absence. Rollback restored the original snapshot unconditionally;
schema-1 restart recovery had no published after-image against which to check the
current file. Recovery-record existence was not exclusive publication ownership.
String matching of closing tags and encoded path fragments also accepted malformed
documents and confused comments with game entries. These are reachable core paths
used by production Playing Library and RetroDECK controllers.

## Core contract

The existing planner, apply, rollback and recovery entry points are retained.
Plans now capture shared hashed object identities plus device/inode bindings for
existing directory ancestors. Stale before-images, replaced files, unavailable
reviewed ancestors and symlinked ancestors refuse. Unknown XML is preserved as
bytes; a bounded structural walk identifies the root insertion span and decoded
game/path values. DTDs, malformed structure, invalid names/characters and unsupported
XML encodings refuse. Relative paths are not guessed into absolute identities.

Apply/recovery/rollback use a persistent sibling `.es-de-publish.lock` inode and
nonblocking advisory flock. Never delete that inode to clear a supposedly stale
lock: the kernel releases the lock when the holder exits. This is Unix support;
other hosts refuse rather than using weaker namespace/exclusion evidence.

The shared atomic text writer remains the mutation primitive. Fresh preconditions
are repeated after writing recovery evidence and before replacement. Reads are
bounded, no-follow at the leaf, and checked using the existing descriptor-bound
hashed object identity. A write/finalization conflict keeps recovery evidence.

Rollback requires exact after-image bytes, or returns AlreadyRestored without
writing when the before-image is already present. Detailed outcome helpers expose
this distinction; the original Result<()> wrappers remain compatible. A different
current image refuses, including deletion of a newly generated file that was
subsequently edited.

Schema 2 recovery records include before/after images, namespace evidence and an
integrity digest. The digest detects accidental modification; it is not a keyed
signature and does not authenticate a malicious local writer able to recompute it.
Schema 1 records have insufficient after-state evidence and remain review-required.
They are not silently upgraded, removed or used to restore over current files.
Successful publication still removes its recovery record; this is not a durable
combined link/metadata Undo receipt.

## Concurrency limitations

Advisory exclusion covers cooperating EmuWiz processes using the same destination
and unchanged lock inode. ES-DE/editors need not cooperate. Comparison and the
shared path-based atomic rename are separate operations: a non-cooperating writer
or ancestor swap after the final check can still race. Inverse remove/replace has
the same limitation. Atomic rename prevents partial file visibility, not lost
updates. A post-write conflict cannot recover unseen external bytes safely, so it
never triggers unconditional restoration. Frontend quiescence/cooperation or a
stronger reviewed replacement protocol remains necessary before claiming full
cross-application exclusion. Inverse operations bind namespace and exact content;
they cannot attribute an external replacement having identical after-image bytes
to its writer. A durable successful after-object receipt belongs to the combined
workflow follow-up. Directory syncing is best-effort on unsupported
filesystems; fault-injection tests model process interruption, not every power-loss
or filesystem ordering behaviour.

## Subsequent GUI/RetroDECK integration

* Invalidate stale confirmations on a core conflict; require fresh review.
* Consume structured ApplyOutcome and RollbackOutcome states, including inner
  failures returned inside Ok. Never publish the complete plan after partial links.
* Retain an immutable receipt binding the applied link transaction and exact
  gamelist publication. Later preview state must not replace that receipt.
* Journal combined phase progress, including metadata-written/finalization-failed,
  metadata reversal and incomplete link rollback. Preserve evidence on conflicts.
* Display legacy recovery as review-required, not a promise of safe restoration.
  Replace synthetic schema-1 success expectations with valid schema-2 fixtures;
  add explicit GUI refusal tests for old records.
* Keep History checks advisory and repeat full owning-workflow verification.

## Existing GUI ownership reconciliation

The original worktrees were read only. Dirty state alone cannot prove that a
session is active or abandoned; no ownership release was established.

* emuwiz-gui-organisation-playing-library: committed HEAD ac37f78e is an ancestor
  of baseline. Actual controller, presentation helper and equivalent-duplicates
  contents match main despite historical dirty/untracked status. Playing Library
  test differences retain older RomM fixture expectations. Organisation/tests
  also differ from current main and must be preserved.
* emuwiz-org-integration: committed HEAD fdc8acbd is an ancestor of baseline.
  Controller/presentation/equivalent-duplicates match main. Tests contain older
  RomM fixture expectations and a debugging print. Organisation/tests differ.
* emuwiz-mame-family-performance: HEAD 30c7542e is not an ancestor of baseline.
  Controller/tests differ substantially, including destination-change state
  invalidation absent from baseline; newer presentation and duplicate modules
  are absent. Do not integrate its whole old tree over main.

Coordination scope includes crates/archivefs-gui/src/playing_library_page.rs,
playing_library_page/tests.rs, playing_library_page/presentation.rs and related
gui_v2/organisation.rs and gui_v2/tests.rs. No dirty core publication or operation
files were found in other registered worktrees when establishing the core lane.

## Validation isolation

All new regressions use TempDir fixtures. Cargo runs through scripts/cargo-iso
with a fresh subprocess HOME and XDG data/config/cache roots, explicit existing
Rust toolchain/cache locations and a target unique to this repair worktree.
No real frontend, ROM, database or mount is a test fixture. No cache is deleted.

The first five added preservation regressions failed against the baseline before
the repair. The final repaired core runs passed:

| Selected core suite | Passed | Failed |
| --- | ---: | ---: |
| launch::es_de_publish::tests | 50 | 0 |
| playing_library (includes RetroDECK projection and multi-file/1G1R checks) | 130 | 0 |
| dat::rename_apply (includes shared identity, History and interrupted rollback) | 138 | 0 |
| operation::tests | 28 | 0 |

These are selected-suite execution totals, with some overlapping operation tests.
New fixtures cover stale bytes/absence, same-byte inode replacement, changed
parents and symlinks, missing ancestors, simultaneous attempts at barriers,
malformed XML, semantic paths and byte-preserved unknown metadata, guarded inverse
operations, legacy/tampered receipts, fresh idempotent replanning, and injected
before-write/after-write/finalization interruptions. No unrestricted suite was
run against the user's real HOME.

The three selected existing RetroDECK GUI tests passed (normal apply/Undo,
publication refusal with link cleanup, and unverified-preview blocking). The
existing ES-DE GUI module produced 13 passes and one failure:
`confirmed_recovery_restores_the_exact_prior_content_and_touches_nothing_else`
at `crates/archivefs-gui/src/playing_library_page/tests.rs:1438`. Its schema-1
fixture expects unconditional restoration over different current bytes. The
new core refuses with "manual review required", leaving the gamelist and receipt
intact. This is an intentionally changed contract; the failing assertion is
reported, not weakened or hidden. Updating this owned GUI fixture, and adding
new guarded-recovery controller coverage, belongs to the subsequent coordinated
GUI slice. This candidate is core-review-ready, not combined merge-ready.

The final `scripts/cargo-iso --low-debug check --workspace --all-targets` passed
with the same isolated subprocess environment, compiling production GUI callers
and all test targets. Targeted Rust 2024 formatting checks, `git diff --check`
and the allowed-file scope guard passed. Existing unrelated core/GUI warnings
remain unchanged. No release build, merge or push was performed.
