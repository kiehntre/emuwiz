# MAME merged reconstruction: current architecture

This port is a projection over the current MAME evidence path. The historical
`dat::mame_normalizer` aggregate plan and its private repair journal are not the
authority for this feature.

| Reconstruction fact | Current authority |
| --- | --- |
| Parent/clone identity | Parsed MAME DAT `DatGameEntry.clone_of`, selected by the exact DAT revision |
| ROM ownership | Persisted `ArcadeJoinEvidence` and `ArcadeMemberEvidence` after checksum matching |
| Physical provenance | `mame_arcade_join_paths_for_dat` archive path plus evidence `current_name` |
| Catalogue freshness | SHA-bound join `dat_sha256`, version, and non-stale persisted audit query |
| Destination planning | `MameMergedReconstructionPlan`, with deterministic member ordering |
| Collision policy | The family plan retains its collision; explicit publication review may authorize checksum-proven existing-target replacement |
| Mutation boundary | Current staged-output and shared `rename_apply` transaction primitives |
| Recovery/undo | Shared transaction state/journal/reconcile/rollback only; no stronger promise is made |
| GUI | GUI-v2 Organisation area, using the typed plan as its preview model |

Filenames are display/provenance fields only. A member is actionable only when
the persisted observation carries a checksum identity that resolves to exactly
one required DAT ROM. Missing, duplicate, stale, conflicting, and incomplete
evidence remains blocked. Donor archives are inputs and are never rewritten. Replacing the explicitly reviewed target is a separate publication action.

The existing staged writer accepts extracted source-set files and packed ZIP
members through the canonical bounded member copier. Discovery, DAT ownership,
parent/clone rules and duplicate-candidate refusal are unchanged.

## Reviewed existing-target publication

`review_reconstruction_publication(plan, target_evidence)` builds an immutable,
read-only publication review around the existing family plan. It exposes
`Create`, `Unchanged`, or `ReplaceExisting { original }`, the exact destination,
and the planned member inventory/hashes. No GUI action is added.

A complete checksum-verified target is a no-op, without staging or journal
writes. An incomplete/bad/wrong-member target needs a matching DAT-version/SHA
parent join and at least one live, checksum-proven **parent-owned** member that
matches the join. A filename, stale catalogue join, or an entirely unrecognisable
archive cannot authorize replacement. Missing members, ambiguous donors,
unresolved ownership and other existing family blockers remain refusals. Only
the specific reviewed destination collision is discharged by publication review.

The review binds target and donor file identities, sizes, full-precision mtime
and SHA-256 through existing `rename_apply::identity`. Before staging and again
before publication, those bindings must still match; changed inputs require a
new review. Donor ZIPs are hashed once per distinct path per binding pass, not
per required member. Queries remain family-scoped; no whole-DAT rescan is added.

`ReviewedReconstructionPublication::apply` streams the existing planned members
into an exclusive new staging file. Reusing a staging path is refused so an
original preserved by a previous operation cannot be destroyed. The output is
reopened through `ZipArchiveSource`, retaining canonical member-count, logical
size, compression-ratio, codec, encryption and safe-read limits. Every expected
name must occur exactly once; unexpected/duplicate members fail. All available
DAT SHA-1 **and** CRC values and sizes must match. Verification streams bounded
hashes instead of buffering complete members. Inventory lookup is indexed by
member name. Output verification is required before any target mutation and is
bound between matching strong staged-file identity captures. The returned
canonical transaction state, not merely an `Ok` result, determines success.

## Canonical transaction and exact-original undo

The only new canonical operation is
`dat::rename_apply::TransactionOperation::ReplaceExisting`, carrying the original
target's identity and destination root alongside the existing staged-source
identity. Ordinary create/move behavior and old journals remain unchanged.

On Linux, the executor durably records `Applying`, syncs both files, revalidates
both, then uses `renameat2(RENAME_EXCHANGE)`. The verified replacement appears at
the target while the exact original moves to the staged source path in the same
atomic syscall. Original preservation and publication are indivisible: there is
no interval between them and no missing-target window. Both parent directories
are synced and both identities confirmed before reporting success. Cross-device,
unsupported filesystem/platform, symlink, hardlink, foreign or changed targets
fail closed. There is no overwrite fallback.

The **staged source path is now the original backup**. Callers must retain it and
the canonical journal for undo/recovery. No MAME-private journal or backup format
exists. A failure before exchange leaves both inputs untouched. A failure after
exchange keeps the entry `Applying` and the transaction failed; restart/undo
reconciliation classifies both objects and never guesses success. A lost final
journal update is similarly recoverable from the durable pre-exchange receipt.

Undo verifies both the published replacement and preserved original, then
exchanges them back. The original target bytes and inode are restored exactly;
the replacement is retained at staging. Changed user output or backup blocks
undo without clobbering either. Repeated completed undo is safe. No source ROM or
donor archive is modified, and no cleanup, emulator execution or automatic retry
is introduced. As with the existing transaction framework, this assumes trusted
same-user journal storage and exposes pathname check/use races; it is not an OS
sandbox against a hostile concurrent writer. Unexpected exchanged objects remain
preserved for manual recovery rather than being deleted or reported as success.

## Scope and tests

Synthetic tests exercise missing-target create, complete-target no-op, incomplete
and bad-hash replacement, verified packed donors, exact original preservation,
restart recovery, repeated undo, stale source/target and changed-user-output
refusal, foreign/symlink targets, malformed/bounded verification, unexpected
members, CRC-only verification, journal failure, pre-exchange failure and
post-exchange failure. Existing family-scoped query/planner tests remain in use.
The largest added synthetic family has 259 members, including a 32 MiB member;
review/staging/publication took 49.7 seconds in the isolated unoptimised test
build on this host. This is a regression fixture, not a release-build benchmark.
Strong identity binding adds bounded streaming hash passes; family discovery and
query complexity are unchanged.

Deferred: GUI Publish/Undo exposure, targets with no checksum-proven parent
anchor, unsupported/encrypted donor formats, unresolved duplicate donor choices,
and any archive case refused by the current canonical packed-member reader.
No DAT authority, dependency ownership, database schema or dependency changes.

Validation against main `cefaac0a087245ee22a6a09bcddef81c38f1a0e0`:
19 reconstruction, 134 canonical transaction/recovery/undo, 33 ZIP safety/copy,
10 MAME evidence and 12 normalizer/scoped tests passed. The full core library
suite passed 10,833 tests (3 ignored). Offline locked workspace check, formatting
check and diff check passed; the workspace check reported four warnings in
untouched GUI code. No GUI, migration or dependency changes are included.
