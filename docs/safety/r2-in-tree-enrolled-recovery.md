# R2 in-tree enrolled recovery candidate

This candidate demonstrates participating-writer integration in the actual
`archivefs-core` crate. It does not activate production compact mutation. Only
Linux core unit tests compile the enrollment, admission, bridge, issuer and
restart execution entry points. Application builds compile the read-only
compact dispatcher/scanner so discovery, recovery and History see the same
current transaction. A production writer refuses an existing compact receipt.
That refusal is **not** exclusion of an old build or an outside tool.

## Ownership and baseline

- Original implementation owner: Codex `/root`.
- Worktree: `/home/davedap/emuwiz-r2-real-integration-20261015`.
- Branch: `integration/r2-real-recovery-20261015`.
- Fetched `origin/main`: `ce8815d63c6d2797fdc480c9d1f7d840035ca6f1`.
- Candidate starts at that exact main, with no merges of the donor branches.
  `c0251f17`, `1909589b`, `37ee56b9`, `4ab8eefb`, and `2fb61afc` are not ancestors
  of this starting main. `f4b9142b` is historical only.
- Owned files: `crates/archivefs-core/src/dat/rename_apply/`, the core manifest,
  the lockfile's two direct test-dependency entries, and this document.
- Fresh target: `~/.cache/emuwiz-cargo-targets/emuwiz-r2-real-integration-20261015-472c32041506`.
  All Cargo commands use `scripts/cargo-iso --low-debug`; its 8 GiB guard remains
  enabled. Tests use the repository's private environment constructor and private
  temporary file roots. No real libraries are used.
- All accepted worktrees, previous evidence and the specified R3 support cache
  are preserved. No cleanup, push, merge, release, or production activation.

Evidence lives in
`/home/davedap/emuwiz-audit-ledger/2026-10-15-r2-real-integration/`.
`OWNERSHIP.txt` records the claim. Initial current-main rename tests passed
206/206. Existing unrelated compiler warnings remain; they are not repaired here.

## Failed experiment: reproduced, not adopted

Fresh source build of exact `37ee56b9`, with `EMUWIZ_R2_COMPACT_JOURNAL=1`, produced
175 passes and 33 failures out of 208 tests. The earlier reported result was
174/32 of 206. The complete fresh failing-test list and output are retained as
`evidence/diagnostic-failing-tests.txt` and `evidence/diagnostic-37ee-enabled.log`.

| Failure group | Count | Root cause |
| --- | ---: | --- |
| Directory sync and restart classification | 22 | Readers return the Planned base instead of Applying/RollingBack records; in-flight work can appear SafeToResume |
| Checkpoint and repeated/cancelled Undo | 7 | Persisted state and failure history are stale, including RollbackFailed |
| Replacement recovery | 2 | Global interception routes an unsupported operation into the small-record logger |
| Reconciliation | 1 | A later correction is absent from the base-only read |
| Observing seam legacy test | 1 | The global switch intercepts an ostensibly legacy/observing test |

The experiment's `journal_connection.rs` is a separate append logger. It neither
uses the accepted Store/Bridge nor connects any reader. Its `dropped_updates`
explicitly counts fields it cannot encode. No part of that logger is imported.
All existing current-main safety assertions are retained.

`1909589b` supplies the checkpoint-seam idea; the public immutable
`write_journal` signature remains unchanged. `c0251f17` supplies evidence for
shared admission, but its Python wrapper and old support rlib are not used in
this build. `2fb61afc` is left intact and is not represented as production custody.

## One representation and one dispatcher

The minimum selected `4ab8eefb` foundations are its preallocated Store,
no-follow/path binding checks, encoded payload publication and commit-marker
protocol, semantic materializer, Bridge, and Ed25519 context/grant consumption.
The current-main executor, directory durability, reconciliation, no-clobber and
Undo implementations remain the mutation algorithms. The donor
`journal_store`, capacity policy and retained whole-journal publisher are not
ported. The operation lease is replaced by the shared fixture admission guard.
Dependencies are existing std/libc/serde/SHA-256 and SQLite; ring/getrandom are
explicit test dependencies already present in the workspace lockfile.

The accepted 111-byte fact vocabulary cannot represent exact diagnostics,
timestamps, preflight outcomes or transaction-level cancellation/failure. The
necessary correction is **one versioned checkpoint record** containing both
semantic facts and the exact mutable fields, inside the same preallocated
container. There is no second transaction log. Each record binds the immutable
base and predecessor digest. Its semantic facts are checked by the existing
materializer; runtime fields must agree. The container is version 3 to avoid
misreading the older test format.

| Information | Authoritative location |
| --- | --- |
| Transaction ID, generation, classifier, creation time, root | Immutable original base |
| Paths, object identities/freshness, operation, basenames | Immutable original base |
| Unknown nested/root metadata | Original base bytes, preserved verbatim |
| Overall and entry states | Exact runtime fields in the committed checkpoint, validated against semantic facts |
| Preflight boolean and all findings | Exact runtime checkpoint fields |
| Failure reason and Apply/Undo timestamps | Exact runtime checkpoint fields; no placeholder text or invented times |
| Created directories, resolution, exact-resume envelope | Bound immutable fields; unsupported enrollment refused before mutation |
| Filesystem completion on pending-record repair and release | Existing filesystem classifier / exact identity checks, under retained admission |

Enrollment supports 1–8 fresh regular-file RenameMove entries, direct children
of one private existing ext4 root with 4096-byte blocks. Directory creation,
copy/link/replacement, existing state, exact-resume envelopes and cross-directory
moves are refused before admission/mutation. Such transactions otherwise retain
their explicit legacy route. The 75/100-entry tests exercise that legacy route
with this candidate's dispatcher installed.

Each container reserves `12 * entries + 32` blocks of 65536 bytes plus its encoded
immutable base, within a 64 MiB container cap. Records write only their bounded
payload, not the base/opaque metadata. Over-budget payloads or exhausted retry
slots fail closed and retain ownership. The descriptor is published before any
file mutation; it is dispatch metadata, not an approval or lease. Its original
base copy must exactly match the container base. This is a bounded test path,
not a claim of unlimited retry capacity or universal storage reservation.

`read_journal` dispatches compact evidence to the shared scanner. Therefore
`list_journals`, recovery discovery, rollback discovery and History summaries
receive current state. A missing descriptor still produces a discovery problem
from retained container evidence. Corrupt/unreadable compact evidence cannot
fall through to a legacy base. Other IDs and opaque legacy metadata containing
the format text do not enroll a transaction. Public journal deletion is refused
for compact evidence; verified release retains all evidence.

A completely written but uncommitted record can be repaired only after fresh
signed approval and filesystem checks for claimed completion. A partial runtime
payload cannot be recreated from facts; it is retained and refused. This avoids
inventing error text, timestamps or a completed state after a short write.

## Participating admission, approval and release

All fixture participants receive the same independently configured authority.
A stable flock serializes its SQLite changes and actual operations. Durable
claims cover `(device,inode)` and both `(parent device,parent inode,name)` slots.
Different IDs and hardlink aliases cannot acquire overlapping claims. The guard
is held across revalidation, mutation, checkpoint publication, approval
consumption and terminal verification. Process exit only drops the serialization
lock: unresolved claims remain in SQLite.

Restart uses the accepted signed context and durable, exclusive grant tombstone.
Workers receive a public verifier and a grant; signing keys stay with the
separate test controller. Signature, issuer, context, filesystem observations,
action and complete receipt are checked. Tests isolate the intended refusal:
notably replay is attempted after consumption **without changing the container**,
so a stale-image check cannot falsely prove replay protection.

Release requires a committed rollback seal, no pending checkpoint, exact original
source identities, absent destinations and an unchanged bound store. The caller
checks the nested `RollbackOutcome.result`; `Ok(RollbackFailed)` and partial Undo
retain ownership. The atomic release transaction marks the owner released and
removes claims while the guard remains held. Interruption before release retains
claims; interruption after commit leaves verified terminal state and no claims.

## Validation and acceptance

Run from the worktree above:

```sh
# A: executed at unchanged ce8815d6 before candidate edits
scripts/cargo-iso --low-debug test --locked -p archivefs-core --lib dat::rename_apply::

# B: candidate dispatcher present, no enrolled bridge installed by these tests
scripts/cargo-iso --low-debug test --locked -p archivefs-core --lib dat::rename_apply:: -- --skip compact_recovery::storage::integration::tests::

# C: real enrolled writes, current reads, restart and signed Undo
scripts/cargo-iso --low-debug test --locked -p archivefs-core --lib dat::rename_apply::

scripts/cargo-iso --low-debug test --locked -p archivefs-core --lib platform_evidence_fusion::plan_transaction::
scripts/cargo-iso --low-debug test --locked -p archivefs-gui --lib repair_history_page::
scripts/cargo-iso --low-debug test --locked -p archivefs-gui --lib gui_v2::organisation::
scripts/cargo-iso --low-debug check --locked --workspace --all-targets
scripts/cargo-iso --low-debug fmt --all -- --check
```

The enrolled suite exercises exact bytes/identities, fresh-process multi-file
Apply/Undo, checkpoints before/after publication and acknowledgement, approval
consumption, ownership release, negative signatures/issuers/receipts/replays,
two participating processes, both slots, aliases, corruption/truncation,
injected I/O errors, cancellation and partial Undo. Core outcome, persisted
transaction, discovery, recovery assessment and History summary are compared.

The separate nonparticipating process characterization asserts **NOT PREVENTED**:
an unaware process replaces the destination after validation; Undo moves the
wrong object, detects failure, and retains unresolved ownership. This passing
characterization is evidence of a remaining limitation, not a safety success.

Final command counts and independent verdict are recorded in the accompanying
audit evidence and handoff after execution against the exact committed tree.

## Production gates remain closed

No claim is made for exclusion of old/nonparticipating writers, protecting
signing keys from hostile same-UID code, detecting a fully rolled-back authority
store, hardware power-loss durability, or unrestricted deployment. A stat,
advisory lock, format marker, directory name, process scan or warning does not
exclude an unaware writer. This candidate does not restart the deployment
custody design discussion or authorize activation.
