# Durable conversion queue and restart recovery

Backend implementation: `archivefs_core::conversion_queue::durable`.
Original base: `fc8bda7be687e626d708a4633f73840bcbf15c32`.
The queue landed at `d7b8b2560df83b1bf54c531767fb9fbdc0de413b`, including
Dreamcast IP.BIN tooling and GUI missing-game review. WUD → WUX support extends
that main without changing the queue state machine or persistence limits.

## Existing architecture and integration boundary

- `conversion_queue` already provides read-only preview rows, estimates and GUI
  planning vocabulary. Its existing API remains compatible. The `durable`
  submodule supplies persistence and execution, not another converter engine.
- `conversion_planner` and `storage_conversion` describe capabilities; a preview
  alone does not authorize an executable queue job.
- The optical apply path binds CUE/BIN identities and optical fingerprints,
  stages/verifies CHD, then uses Repair transactions. It is not integrated here.
- The landed `wiiu_conversion` plan binds paths, metadata/object identity and
  bounded structural evidence. Its native WUX → WUD executor captures source
  SHA-256, streams into staging, independently verifies the output hash/size and
  WUD header, then publishes through Repair. This is the first queue adapter.
- Repair uses the existing `RenameTransaction` model, identity checks,
  no-clobber publication and durable journals. Those journals remain the
  authority for publication, History & Undo and rollback. Queue attempts expose
  their journal directories; existing recovery classification and operation
  registry APIs can inspect them. No second rollback or publication engine exists.

Converter changes serialize the existing typed evidence and expose internal
revalidation and staging-parent entry points. Ordinary callers retain their current
execution behavior. WUD → WUX checkpoint `60e640b0` is now reconciled with these
entry points. `ReviewedConversion::WudToWux` uses the same native executor,
independent streaming round-trip verification and Repair publication. Its tag
must match the original plan direction; admission and execution check both.

## Persistence and ownership

`default_queue_directory()` selects `<effective EmuWiz data directory>/conversion-queue`.
Callers can pass another absolute directory for isolated tests/profiles.
`queue.json` is a version-1 JSON snapshot containing the monotonic sequence,
original typed plans/options, full source identities, stable numeric job IDs,
timestamps, FIFO order, attempt states/paths, progress, diagnostics and results.
Job IDs are unique within the queue and never recycled by pruning.

Pre-encoder version-1 WUX → WUD plans/results remain readable and executable.
Their original four-part evidence binding is retained; an absent direction binding
means the only previously executable direction, WUX → WUD. Their exact output
size supplies the newly added maximum-size projection. No reviewed evidence is
regenerated or replaced. WUD → WUX plans bind direction explicitly and include
canonical writer geometry. No database migration or snapshot version bump is needed;
older binaries refuse the new conversion tag instead of running an unknown job.

`open()` holds an exclusive OS directory lock for its lifetime. Process exit or
crash releases the lock; another live owner refuses immediately. All queue
mutations and conversion execution run through that owner. Reads use `inspect()`
and do not create files, take a writer lock or perform recovery.

Transitions reuse `atomic_write_text`: same-directory temporary file, file
`sync_all`, atomic replacement, and directory sync. The queue additionally requires
a successful directory `sync_all`. Directory creation is synced to its parent.
The target is the existing Linux filesystem/publication support; unavailable
locking or directory durability fails closed. Filesystem/controller durability
still depends on the underlying storage honoring fsync.

The snapshot is bounded to 8 MiB, 256 retained jobs and 32 attempts per job.
Oversized, truncated, malformed, unsupported-version or inconsistent snapshots
refuse with diagnostics and remain untouched. A missing snapshot with other
queue files also refuses. The writer detects external snapshot/directory changes
before replacement. Failed persistence stops further mutations until reopen;
it never reports a durable transition based only on RAM.

## State machine and FIFO

`Queued → Running → Completed | Failed | Cancelled | BlockedStale`.
`Completed` is this project's equivalent of `SUCCEEDED` and includes a verified
converter result and publication transaction ID.

Before execution, missing source becomes `BlockedInputMissing`; changed source,
plan evidence, destination or conversion readiness becomes `BlockedStale`.
Cancelling a queued job persists `Cancelled` without running a converter.
Startup changes abandoned `Running` jobs and their current attempts to
`Interrupted`. A failed attempt setup persists `Failed`.

`enqueue()` admits a reviewed executable plan and captures a full source identity.
It never saves just a command string. Admission, explicit retry and worker
execution revalidate the saved original evidence. Revalidation may calculate fresh
facts for comparison, but never replaces the reviewed plan. File size, exact
mtime, object identity, SHA-256 and converter-specific structure are checked;
execution repeats the converter's own pre-publication checks.

Each accepted job receives a monotonic sequence. The worker selects the lowest
queued order. Retry preserves the stable job ID and original plan, and assigns a
new FIFO order behind already queued work. Destination equality and overlapping
paths are reserved against other queued/failed/interrupted/blocked jobs, checked
again before execution; existing filesystem entries (including dangling symlinks)
refuse. Repair still enforces no-clobber at the final publication race boundary.
Multiple jobs can read one source. One worker executes at a time per queue, so
queue jobs never concurrently publish into overlapping trees. Use one canonical
queue per profile; there is no parallel scheduler or global lock manager.

## Startup, retry and the meaning of resume

Before invoking the converter, the queue persists `Running` and an attempt with
its own `transactions/<job-id>/<attempt-number>/` directory and a private,
destination-adjacent `.emuwiz-conversion-queue-*` staging parent. The converter
uses its existing staging and Repair journal within those roots.

Startup never invokes a converter, publishes output, removes staging or assumes
old progress proves completeness. Every abandoned running job becomes
`Interrupted`. Pre-publication abandoned bytes are retained and never reused.
When there is no destination and no publication evidence, retry can revalidate
and restart from zero in a new staging directory. Fully rolled-back Repair
transactions also permit this, while preserving their original evidence.

Any applied/pending/corrupt/unknown publication evidence, occupied destination or
unsafe/missing attempt directory requires review. A crash after verified
publication but before the queue's final write therefore remains `Interrupted`,
not guessed success. Existing Repair recovery can inspect/roll back the journal;
the queue checks the settled evidence again on retry and execution. It does not
automatically complete or roll back ambiguous operations.

**True mid-file resume: NO.** “Retry/resume” means revalidate the original reviewed
job and run it from byte zero. No CHD or WUX byte-offset continuation is claimed.

## Progress, cancellation and retention

The live callback reports logical WUD bytes processed for either direction and phase. Before byte progress
is available, the persisted snapshot is indeterminate. Progress persistence is
throttled to at most once per second, in addition to required state transitions.
Reaching the byte total is not success: verification/publication must finish.
Startup labels old progress historical; retry clears it. A progress checkpoint
failure requests cooperative cancellation before further conversion phases.

A shared `AtomicBool` cancels the running converter at its supported boundaries:
32 KiB encode blocks, bounded table batches, 64 KiB decode/round-trip chunks and
phase boundaries. Existing full-source hashing and Repair steps can delay response. No arbitrary process killing
is added. Before publication, cancellation removes the converter's temporary
output and persists `Cancelled`; the queue's empty attempt parent is retained.
Publication errors retain journals/staging and require existing verified rollback
before cancellation can be claimed safe. A cancellation arriving after confirmed
publication does not turn a successful conversion into `Cancelled`.

`prune_completed(retain)` explicitly removes eligible old completed/cancelled
queue rows. It never prunes active, blocked, failed or interrupted jobs, or jobs
with failed/interrupted attempts, and never deletes Repair journals or staging.
These remain subject to existing reviewed recovery/undo policies. Capacity refuses
new work instead of silently deleting recovery evidence; no automatic disk cleanup
is introduced.

## Future GUI projection and validation

A later GUI can open the canonical queue on its worker thread, enqueue typed
reviewed plans, inspect snapshots concurrently, request queued cancellation,
set the running cancellation flag, and request explicit retry/pruning. Commands
that mutate the queue serialize through its owner. Queue state, retry disposition,
diagnostics, progress and journal locations provide the projection. This change
contains no GUI or CLI wiring and no database migration.

Tests use synthetic WUX containers through the real landed decoder, independent
verification and Repair publication. They cover FIFO and restart persistence,
stale/missing sources, duplicate/late destinations, source preservation,
verification failure, cancellation, corrupt/truncated/oversized state, read-only
inspection, live-owner exclusion, explicit pruning, actual child-process exit
with partial staging, and the crash-after-publication/review/rollback boundary.

Validation on the original base: 40 queue tests, 8 planner tests, 28 Wii U tests,
18 optical conversion tests, 123 rename/journal tests, 102 repair-related tests,
and 12 recovery tests pass. The full core library suite passes 10,692 tests
with 3 existing ignored tests. The full run includes a regression proving that
queue-owner drop explicitly releases a lock even when another descriptor was
inherited during a concurrent process spawn. The final full suite ran outside
the sandbox because existing proxy tests need to bind loopback sockets.
Build artifacts use the isolated `/tmp/emuwiz-durable-queue-target` directory.
`cargo check --offline --locked --workspace`, `cargo fmt --all -- --check`,
and `git diff --check` also pass; the workspace check reports four existing
warnings in untouched GUI code.

Encoder integration tests additionally cover persisted WUD → WUX plans/results,
real process exit with partial WUX staging and restart from byte zero, source and
destination revalidation, shared destination reservations, cancellation, failed
verification, FIFO retry and source preservation. A compatibility regression
loads pre-encoder queued/completed snapshots and executes the original decode plan.
