# Contract 1-I candidate: FD publication, holding cleanup and default-OFF integration

Gate 1 accepted the test-only FD publication and holding experiment at
`e3e1e2249ec2b49eb501c74b0e848f09a7342b20` with conditions. Phase 2 compiles
that implementation into the Linux core behind an explicit evaluation controller
and a runtime switch whose Default is OFF. No application, CLI or GUI caller
installs the controller. Compiling it is not production activation. Gate 2
requires separate exact-commit acceptance; see the evidence ledger for status.

## Contract and comparison

Private journals and `.emuwiz-r2-holding` directories are trusted. Library
names are not. Hostile modification of private directories, power loss,
unvalidated filesystems, immutable bytes in the presence of external writable
descriptors, and external actions after completion are outside this candidate's
contract. Mode 0700 validates the limited private-directory assumption; it does
not exclude hostile same-UID programs.

G3 uses the same requested ordinary Undo as its primary control, including
actual refusal or failed outcomes. It does not force ordinary Undo to complete.
No-operation controls are diagnostic only. Deterministic external processes act
at corresponding logical boundaries. Comparisons include surviving original
identities, bytes, foreign files and authoritative receipts.

Four reproduced problems required focused corrections:

1. Publishing first left an old library alias. An external rename of that alias
   overwrote an unrelated file that ordinary Undo preserved. Capture the old
   library slot into private holding before FD publication instead.
2. Returning a captured foreign object recreated the same dangerous library
   alias after failed ordinary Undo had removed it. Retain foreign objects for
   explicit review. No G3-safe automatic foreign-return rule is enabled.

3. A crash before capture lost the original FD. Ext4 reused the deleted inode;
   a replacement with matching bytes/mtime passed legacy identity checks and
   was incorrectly reported restored. Unanchored restart now refuses. New
   explicit FD enrollment pins the verified object under admission, publishes
   the existing authoritative base, then creates a private FD-bound hardlink
   anchor. Missing setup evidence refuses. This anchor prevents inode reuse
   until verified cleanup; it is not a generation-number authenticity claim.

4. A legacy unknown field named like FD metadata was interpreted without
   enrollment, and missing FD state could be accepted as completed History.
   All FD interpretation now requires explicit immutable enrollment. Enrolled
   Undo states require the FD state record; missing fields refuse rather than
   falling back to legacy assumptions. Cancellation persists a stopped intent
   and keeps ownership until an authorized verified terminal outcome.

The original failing assertions remain runnable with
`EMUWIZ_CLEANUP_PUBLISH_FIRST=1` and `EMUWIZ_CLEANUP_RETURN_FOREIGN=1`.
Earlier syscall tests for successful/blocked no-overwrite return remain intact;
they prove only those controlled cases, not the G3 contract under interference.

## Ordering and authoritative evidence

The existing compact immutable base retains enrollment, original paths,
transaction identity and object evidence. A versioned FD runtime value is stored
in the existing compact checkpoint, using the existing writer, materializer,
admission, signed restart verification and one-shot consumption. Unrelated
immutable fields are not made mutable. There is no second authoritative log.

Enrolled anchors remain private through Apply and interrupted Undo. They are
verified, included in signed observations and removed only during verified
private cleanup. Interrupted setup never recreates anchors from guessed library
names. The entry flow records capture intent, captures with RENAME_NOREPLACE, inspects
the actual held object, records capture, publishes through the verified open FD,
records publication/cleanup intent, verifies the private holding object again,
and unlinks only that trusted private entry. A known missing/wrong destination
retains the original holding link. The unlink remains non-conditional; private
holding trust is essential. Untrusted library entries are never unlinked by this
cleanup path.

Foreign entries retain identity, original path through the immutable entry,
transaction ID and explicit incomplete state. They are not returned, deleted or
automatically collected. Further mutation requires review. Journal removal and
legacy two-path reconciliation refuse these records.

If the final library link disappears after the cleanup check, a live FD can
support best-effort salvage. Successful salvage is verified bytes in a NEW
inode, never the original object or completed Undo. Copy/write/sync failures
retain partial evidence and report that the original may be unrecoverable.
SIGKILL after the last unlink loses the FD; no name scan reconstructs it.
Completed claims are revalidated by the authoritative reader. Contradictory evidence
is retained and refused rather than projected as successful History.

## Eligibility, costs and hygiene

Only disposable local ext4 roots have been exercised. Eligibility probes perform
actual same-mount FD linking, rename/link no-overwrite and identity checks.
Different mount identities, untested filesystem types and non-4096-byte ext4 blocks refuse. The eligibility check runs before FD admission or anchor setup, so an unsupported root acquires no ownership claims or recovery artifacts. FICLONE returned
EOPNOTSUPP locally, so no btrfs/XFS/reflink safety or cost claim is made. Supporting
those roots requires separate approved filesystem evidence, not a filesystem
label. FS_IOC_GETVERSION was available; availability is not proof that generation
numbers form an immutable authority. The identity reuse check is a separate gate.

F_SETLEASE/F_RDLCK refused when an O_WRONLY FD or writable MAP_SHARED mapping
already existed. This does not promise lasting exclusion, immutable bytes or
protection after lease break. An actual 8 MiB salvage operation is measured in
the deterministic logs; stat blocks include allocation rounding, not just logical
size. Reflink metadata/COW allocation is unmeasured on this ext4 root.

The holding inventory is explicitly per transaction: entries, associated
transaction/evidence, device/inode, link count, known library aliases and allocated
blocks deduplicated by inode. It does not claim whole-root coverage, exact
reclaimable bytes, or that a hardlink allocates another full data copy. Probe
artifacts are separately named and retained. No automatic garbage collector.

The local DAT/library/duplicate scanner, ingestion discovery and arcade walkers,
and catalogue reconciliation walker exclude `.emuwiz-r2-holding` and
`emuwiz-enrolled-recovery-v3` before traversal/hashing, including an explicitly
selected descendant or a selected root resolving through a symlink alias. Per-entry
checks are lexical; selected roots are resolved once. These reserved namespaces are not evidence of
ownership or permission to delete. Missing/corrupt journals must not turn held
data back into ordinary games. Recovery inventory remains separate.

## Bounded shared-engine evaluation

`compact_recovery::controller::RuntimeSwitch::default()` refuses enrollment and
restart. Only an explicit `enabled_for_evaluation()` value permits the core
controller. Existing ordinary callers continue using legacy Apply/Undo with its
existing wrong-file race limitation; this candidate does not silently advertise
those paths as protected. An enrolled transaction without its admitted controller
refuses instead of falling back.

The controller routes new Apply through the existing executor and Undo through
`rollback_transaction_confined`, preserving approval sets, generation, cancellation
and nested outcome checks. Both directions use capture-before-FD-publication.
Apply retains a private enrollment anchor until verified Undo cleanup. New
shared-engine descriptors are `emuwiz-enrolled-fd-v4`; old readers must refuse
rather than reinterpret them. No existing legacy transaction is migrated.

The accepted adapter's limits remain: newly enrolled regular RenameMove entries,
at most eight, direct children of one canonical, private owned collection root,
one compact container per root, local ext4 with 4096-byte blocks and passing actual
syscall probes. A root inside a recovery namespace is ineligible. No real root
permissions are changed. Ineligibility refuses explicit enrollment; normal
unenrolled legacy operations are unchanged. Provisioning authority, a production
issuer/operator approval interface, root lifecycle/capacity and broader operation
support are still separate deployment work. The test issuer stays test-only.

Interrupted Apply is discoverable with its original anchor and exact runtime
phase, but automatic Apply resume is deliberately unsupported by the existing
bounded bridge. A signed request cannot override that refusal. Supported Undo
resume uses the existing signed verifier, trusted issuer configuration, current
observations, durable one-shot consumption and retained ownership. Ambiguous
publication, missing last-link evidence, salvage and foreign captures require
operator review. No scan reconstructs lost authority or a lost file descriptor.

## Interrupted Apply recovery (signed Undo)

An Apply that dies or fails part-way through a batch is recovered by the same
signed Undo, through `fd_contract_recovery.rs`. Each entry is resolved from its
exact runtime evidence and from trusted private objects pinned through open
descriptors (the enrollment anchor, which Apply never retires, and the holding
entry), never from a library name or a journal state alone:

| Entry evidence at recovery | Trusted observation | Outcome |
| --- | --- | --- |
| No Apply intent (Planned, preflight refusal, Skipped), or `CapturePending` with the holding slot empty | Source names the anchored inode, destination/holding/salvage absent | `Skipped` with `Undo/Unmutated` evidence recorded first, then the anchor is retired (`NoMutation`) |
| `CapturePending` or `Captured` with the original in holding and the destination absent | Holding identity matches | Adopted as `Undo/Captured` (`UndoIntent` from an unresolved Apply) and reversed by the ordinary trusted-holding Undo |
| `Captured` with the destination naming the inode pinned by holding and anchor | Publication proven, record lost | `Apply/CleanupPending` recorded, holding unlinked, `Apply/Complete`, then ordinary Undo |
| `CleanupPending` with holding present | Destination verified | Existing Apply cleanup finishes, then ordinary Undo |
| `CleanupPending` with holding gone, destination anchored, source free | Completion proven by the anchor | `Apply/Complete` recorded, then ordinary Undo |
| Foreign holding object, foreign or deleted destination, occupied source, holding present at both names | Contradiction | `ApplyFailed` with `Apply/ForeignHeld`, `DestinationOccupied` or `Incomplete`; transaction `RollingBack`; further attempts refuse until review |
| Anchor missing, holding missing after recorded capture, replaced source before capture, salvage slot occupied | Continuity unprovable | Refused before any mutation; all evidence retained |

The materializer accepts one additional transition, `ApplyUnresolved →
UndoIntent → UndoUnresolved`; every other fact rule, the slot budget and the
frame format are unchanged, and an older reader refuses such a stream as a
transition error instead of reinterpreting it. The accepted Undo-direction
refusals are untouched: an Undo interrupted after its own publication or
holding unlink still parks as `DestinationOccupied` or `PossiblyUnrecoverable`.
Recovery is idempotent: every step re-verifies and skips work already done, a
second grant after completion changes nothing, and release still requires the
verified seal. Process death with `_exit` at every boundary is the tested crash
model; power loss is not claimed.

See `r2-history-holding-backlog.md` for the separately approved GUI job. The
experiment does not implement that GUI or permit cleanup of unresolved records.

## Validation record

Exact commands, source hashes, failures and retained synthetic fixtures are in
`/home/davedap/emuwiz-audit-ledger/2026-10-15-r2-contract1i/`.
Historical results do not stand in for final-tree validation. The stress catalogue
is fixed at six adversaries. The owner reduced the target to 1,000 paired cases
per adversary with a 20-minute remaining-run ceiling. Completed, fully asserted
cases from the original 10,000-target run count toward that requirement;
interrupted cases do not. Per-adversary selection only schedules the remaining
samples; it does not change assertions or deterministic pause boundaries. These
fixed sequences use no random seed. Exact coverage and any timeout remain in
the ledger and independent verdict. Any unresolved Gate 1 blocker prevents
shared-engine integration and Gate 2.

The subsequent disabled-by-default operator provisioning/approval implementation
and exact activation prerequisites are documented in `r2-deployment.md`. It does
not widen any Contract 1-I filesystem, capacity, recovery or trust limit above.
