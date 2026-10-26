# MAME internal repair Apply in Blue

Baseline: `e249df1d8344f478f78b85928200fd460c4c23de` (origin/main, includes
merged #159 and #166). Historical branch
`fix/mame-chd-internal-repair-boundary-successor` was inspected as reference;
its implementation was not copied.

## Existing implementation and missing connection

The core already has `build_mame_internal_repair_plan`,
`plan_mame_internal_repair_apply`, `apply_mame_internal_repair_plan`, and
shared sealed journal, rollback and recovery handling. Blue's active health
route calls `HealthPage`, whose worker already resolves an explicit registered
catalogue and inspects the collection. It never built a repair plan. The older
`show_with_apply_plan` renderer was unreachable from that route and normally
received no Apply projection. It is not the new approval authority.

The focused health `repair` controller now connects the existing planner and
executor to the active route. No ROM-matching, copy engine, Undo engine or
journal format is duplicated in the GUI.

## Supported journey

1. Register an existing MAME arcade `-listxml` catalogue in Verification Data,
   then explicitly select it and an absolute collection folder in MAME Health.
   The worker requires a supported declared release and current listxml schema.
   User-registered catalogues are user trust decisions; registration does not
   authenticate their upstream publisher. No catalogue is downloaded implicitly.
2. Inspect. Parsing and SHA-256 refer to one private catalogue byte snapshot.
   A private, non-deserializable reviewed authority carries the reference,
   catalogue SHA-256, versioned repair plan, exact copy projection and original
   transaction identities. Its digest also binds those catalogue/plan facts.
3. Review every source/destination, expected size/SHA-1, relationship and
   unresolved requirement. Only missing ordinary ROM members in existing
   exploded machine directories are actionable; nested destination names,
   archive writes and CHDs are unsupported. Existing content is never replaced.
   Ambiguous declarations, sources and unresolved dependency declarations block
   Apply. Incomplete inspection and known supplied-version mismatch also block.
4. Type `APPLY` for that exact preview and press Apply. A worker repeats the
   selected catalogue resolution and collection inspection, compares the full
   reviewed projection and source identities, then executes the ORIGINAL
   reviewed transaction. Changed snapshots refuse before transaction execution.
   Source SHA-1 proof is bound to the transaction's SHA-256 proof with stable
   identity stamps. Engine preflight repeats source and destination checks.
5. Stage progress is literal: evidence revalidation, then transaction execution.
   The UI does not invent per-file percentages or call a failed/rolled-back
   transaction successful. A disconnected worker means unknown outcome.
   Filesystem completion verifies copy bytes; it does not run MAME or establish
   runnability. Reinspect for current health.
6. Open History & Undo / recovery. This opens the established journal controller,
   reloading its records without replacing a running Undo. Blue History labels
   sealed internal repair receipts as MAME repair. Undo uses the shared engine.

## Shared copy safety corrections needed before exposure

Independent baseline review reproduced a symlink clobber in the old
absence-check plus `fs::copy` temporary-file path. Copy Undo/recovery also
recognized content alone, allowing a foreign equal-byte file to be adopted.

The existing transaction engine now prepares copies under pinned directory
handles in an exclusively created mode-0700 `.emuwiz-copy-*` directory, using
exclusive/no-follow file creation and a retained source descriptor. It bounds
copy bytes, syncs and verifies data, and records the actual created inode and
private directory identities BEFORE the Applying checkpoint and publication.
Publication uses the existing descriptor-bound no-clobber rename and directory
sync. No failed-path cleanup deletes a potentially substituted file.

Undo moves only the exact created copy back to its journalled staging slot,
using no-clobber rename and directory sync. It intentionally RETAINS that copy
as recovery data; it does not delete it. A failure after the move stays in flight
for exact-slot reconciliation. Recovery binds and verifies both slots and never
adopts an equal-byte replacement. Legacy copies without created-object receipts
remain inspectable and fail closed for automatic Undo/recovery. Retained copies
and explicitly selected private recovery descendants/aliases are excluded from
collection and discovery inputs through the existing reserved-path helper.

## Limits and recovery

This path requires Linux's verified no-clobber primitive. Other platforms refuse
copy execution. No concurrent external collection writer should run during
Apply/Undo. Descriptor binding prevents symlink redirection of writes, but the
existing transaction infrastructure documents that observing containment is not
an enforced exclusion lock against another same-user process moving directories
or changing names after verification. No power-loss or atomic batch claim is made.

A crash before staging evidence is checkpointed can leave a private staging
folder. It is retained and excluded from scans; it is not automatically deleted
or silently adopted. The sealed journal and recorded per-entry failure reason
are the recovery authority once checkpointed. Occupied/foreign slots, missing
private directory identity or unavailable evidence remain unresolved for review.

All acceptance data is synthetic and disposable. Tests exercise the actual
rendered Apply button, no-confirmation gate, Apply/journal reload/Undo, stale
catalogue and collection rejection, CHD and dependency refusal, equal-byte foreign
replacement refusal, interrupted publication and Undo, and private recovery
exclusion. Core fault injection covers temporary symlink substitution, held-copy
identity substitution, changed staging before publish and failure after each
mutation. Production collections are never test targets.
