# Shared patch execution reconciliation

Base: `cf603f3ee68bcbb0bee7b9c999967ff997221461`, authoritative main at
`/home/davedap/emuwiz-main-release-fix`, matching local origin/main and clean
tracked files. Work is isolated on `integration/shared-patch-execution-current-main`.

## Conflict evidence (read only)

`/home/davedap/emuwiz-xdelta-live-integration-2`, branch
`integration/xdelta-live-validated-2`, HEAD
`55dab4495a456a0e73149a88fb7407bc8761d73e`, has an unmerged index entry for
`crates/archivefs-core/src/standalone_patch.rs`:

- Stage 1, common-base blob: `91ab2b603ebfd7bbb4680e9a7b1b12f37ae7279f`.
- Stage 2, ours: `9df52a1142bebcde7847d48c121d336ef4dbf19e`.
- Stage 3, incoming: `b9de9956bf909c8930dc58a30c80ab4c92157b0d`.
- CHERRY_PICK_HEAD: `5e7a26d84b73cbdc8fd710d5e4f8ef2094ab5cc1`.

There is no MERGE_HEAD. This is an unfinished cherry-pick, not a merge of
current main. Ours contains the RFC VCDIFF magic correction from `70a3a583`;
incoming adds an early xdelta/PPF implementation built against the old magic.
The working file has no conflict markers and exactly equals stage 3; the
index still records UU. Absence of markers does not mean a resolved index or
correct integration. Both the old branch HEAD and incoming commit are already
ancestors of current main. No state was changed in that worktree.

| Concern | Current main | Incoming xdelta side | Decision |
| --- | --- | --- | --- |
| VCDIFF format detection | Correct `d6 c3 c4`, header validation | Retains transposed magic | Keep main |
| xdelta execution | Supervised process with time/resource/output limits | Direct unsupervised Command | Keep main |
| PPF3 execution/provenance | Present | Earlier implementation | Keep main |
| Source/output checks | Reviewed hashes and declared size/CRC checks | Older checks | Keep main |
| Durable output/recovery | Single-file journal, resume and rollback | Older publication | Keep main |
| Real xdelta tests and GUI apply | Present on main | Earlier implementation/tests | Keep main |
| Complete directory publication | Missing | Missing | Focused shared extension |

No unique required xdelta hunk was identified. Do not take either conflict
side wholesale or clean up the old index as part of this task.

## Current single-file pipeline

`standalone_patch.rs` inspects bounded patch bytes, parses format/size/checksum
facts and builds a reviewed plan binding base and patch hashes and the output
path. Header adjustment is explicit. `apply_standalone_patch` rechecks hashes,
decodes IPS/BPS/UPS/PPF3 internally or invokes supervised xdelta3, verifies
output size/CRC when declared, and calls `publish_durable_patch_output`.

That publisher records durable intent/checkpoints beside the destination,
writes and syncs a temporary regular file, verifies its hash, hard-links it
no-clobber to the destination, verifies publication, and finalizes provenance.
Final provenance rechecks the base hash after file publication. A failure
there can require recovery; an error does not imply nothing was published. Inspection, resume and rollback are in
`patch_output_recovery.rs`. GUI local mod package apply reaches this pipeline
through `local_mod_package_page.rs`; no GUI change is needed here.

`prepare_standalone_patch_output` returns verified bytes without publication;
it is reusable inside an independently staged component tree. Its external
xdelta temporary file is created beside the supplied base, so a component
backend must use a scratch base rather than an authoritative source path.

The existing journal carries one destination/temporary file and one output
hash. It is not a directory manifest and hard-link publication cannot publish
a directory. `patch_manager::shared_transaction` does journaled multi-entry
writes but carries adapter/preview policy and config/materialized-output
limits; it is not a whole optical-tree publication primitive. The DAT/repair
identity and no-clobber rename primitives are reusable; file mutation proofs
do not themselves certify an entire directory.

## Saturn

`ffcbd9094c873993ab705cd43888e3d11d08a11a` adds the backend and module
registration, with no standalone-patch change.
`1f6ce22dac875df91ed9772bf9f241903674baad` adds validation/tests and three IPS
resize hunks. Both literal and RLE record-growth corrections are already on
main (`b17f17ac`), with dedicated regressions. The EOF hunk changes explicit
final-size truncation into grow-only behavior: reject it. Main deliberately
retains exact EOF resizing, including truncation. No Saturn standalone-patch
hunk remains to port.

Retain Saturn's target/component selection, source manifest binding,
size-preservation requirement and independent rebuilt-manifest checks during
its eventual integration. Replace private plain-rename publication and
recursive-delete rollback with the shared tree contract. Backend-specific
verification must still prove the complete output set and optical layout.

## Dreamcast

`566f8fcce3ab85e1aec7f55e499d5323b6b88772` stages an extracted filesystem,
performs selected package operations, validates IP.BIN, then uses plain rename
and an in-memory receipt; rollback hash-checks then recursively deletes.
`b43256c404563d2aa5421a3d97be6f730c5f2f25` adds a real package-file hash
recheck at apply. Retain that recheck and domain validation. The candidate's
check-then-rename can replace a destination appearing after preflight; its
private receipt is not durable crash recovery. Do not copy that transaction
layer wholesale. No DCP decoding, IP.BIN behavior or backend is added here.

## Focused extension and integration contract

Decision **B: small shared patch-execution extension needed**.
`patch_output_recovery::tree` is Linux-only and reuses `capture_identity` and
`rename_noreplace`. It accepts a reviewed complete input set (source tree or
explicit components, plus every patch/package) and one new destination:

1. Bind input files/trees and destination parent at preview.
2. Create a private sibling stage. Backend materializes there only.
3. Snapshot all output members, invoke independent backend verification,
   require unchanged verified output, and revalidate inputs.
4. Sync files/directories and seal an immutable bounded receipt beside stage.
5. Explicit publish/resume revalidates inputs and the exact output tree,
   then performs one no-clobber root rename and checks the published tree.
6. Inspect derives Staged/Published from the two possible tree locations.
   Missing, substituted, changed or conflicting objects fail closed.
7. Explicit undo moves the unchanged published tree back to staging. Bytes
   remain available for inspection/resume; it never recursively deletes.

Receipt locks serialize cooperating publish/undo calls. The new API has its
own schema; existing single-file journal schema/behavior is unchanged.
No generic file-operation engine, backend integration or GUI is added.
Bounds: 16,384 entries, depth 64, 16 MiB receipt. `TreePatchPlan::review`
retains the 512 MiB byte default. Optical callers must use
`review_with_max_total_bytes` to select their supported source/package budget,
within the helper's absolute 8 GiB ceiling (zero and larger limits refuse).
This ceiling provides headroom for Dreamcast GD-ROM and Saturn CD component
sets and their patch inputs; it does not claim new format or rebuild support.
Backend-specific limits remain the adapters' responsibility when integrated.

The configured byte limit applies to the combined input set and separately to
the complete staged/published tree. Logical lengths count, including sparse
holes; all accumulated sizes use checked addition. Complete content hashing
still streams through the existing fixed-memory identity capture. Oversized
output refuses before semantic verification, receipt sealing, or publication;
failed staging remains retained as before. The immutable plan/receipt carries
the limit through revalidation, inspection, publish, and undo. Old receipts
without that field retain the original 512 MiB default. Entry/depth bounds,
identity checks, no-clobber rename, and recovery semantics are unchanged.
Symlinks, hardlinks, special files, output-inside-input and existing targets
are refused. Backends must include provenance/receipts inside staging before
sealing when they are part of the required complete output set.

A complete root is the publication unit, not a sequence of file renames: no
partially published component set is intentionally exposed. If parent sync
or post-publication verification fails, the call returns error and preserves
the receipt/output for inspection; it never claims rollback happened. Failure
before sealing retains an unpublished stage (its location is in the error),
without an automatic cleanup/resume claim. Discovery/UI for these tree
receipts is deferred to backend wiring; explicit receipt-path recovery works.

Limits are deliberate: no overwrite/merge of existing trees, cross-filesystem
publication, save export or automatic deletion. Undo is a retained-tree move,
not destructive cleanup. Backends, not this helper, supply domain verification
and complete dependency selection. Like the existing identity/rename layer,
this uses phase-boundary pathname checks, not adversarial filesystem locking
or protection from malicious same-user journal edits.

## Validation

Isolated build directory: `/tmp/emuwiz-shared-patch-target`; offline, locked
Cargo dependencies and synthetic temporary inputs only. No real user media.

- New tree suite: 14 passed, 0 failed.
- Existing standalone patch suite: 32 passed, including 11 IPS regressions
  and the installed `/usr/bin/xdelta3` real encode/decode round trip.
- Existing single-file recovery tests: 5 passed; no-clobber primitive: 3 passed.
- Full core library on the starting base plus candidate: 10,098 passed,
  0 failed, 3 ignored (133.96 seconds).
- Offline locked workspace check passed, retaining five existing GUI
  unused/dead-code warnings. Workspace format check, diff check and scoped
  postcheck passed.

Main advanced independently to `1872e1cbdb562e50d2746f6466ea2692dd44e730`
(optical tests only), with local origin/main still at `cf603f3e` and main's
tracked tree clean. Read-only `git apply --check` passes against that tip.
No rebase/merge, main edit, push, GUI edit or backend promotion was performed.
The old conflicted working file still hashes to stage 3, and all three
unmerged index entries remain unchanged.

## Size-policy follow-up on current main

Recreated `b7d59440` on `1872e1cb` as `626afa87`, then changed only tree
byte policy, its tests, and this document. The original fourteen tree tests,
publication/undo functions, standalone decoder and single-file recovery are
unchanged. Six added tests cover sparse logical input above 512 MiB, combined
input/staged-output limits, hard-ceiling rejection, checked overflow,
persisted policy through recovery, and defaulting of older receipts.

Validation: tree 20 passed; standalone filter 33 passed (includes the tree IPS
composition test); recovery 25 passed; no-clobber primitive 3 passed. Full core:
10,108 passed, 0 failed, 3 ignored. Offline locked workspace check, formatting,
diff and scope checks passed. Five existing GUI warnings remain. Full-core
validation used two test threads and loopback access for the provider tests.
Main remained at `1872e1cb` with fetched origin parity; no push or promotion.
