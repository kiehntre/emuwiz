# Contract 1-I candidate: test-only FD publication and holding cleanup

This work extends the accepted FD publication experiment, not the production
rename engine. All new mutation code remains below `cfg(all(test,
target_os = "linux"))`. No production activation, push or GUI changes are
included. Gate 1 and Gate 2 require separate exact-commit acceptance.

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
Completed claims are revalidated by the test-only reader. Contradictory evidence
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

Current scanner integration points are `dat/sources/audit_run.rs`'s local
breadth-first scan (also used by library/duplicate scanning), ingestion discovery
and arcade walkers, and `catalogue_reconciliation/walk.rs`. They currently do not
exclude the experimental holding namespace. Before any production caller exists,
exclude the authoritative root's holding directory before traversal/hashing,
including explicit scans into it; keep a separate recovery/attention inventory.
Do not turn a filename exclusion into a security or deletion guarantee.

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
