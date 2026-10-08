# PS2 PSU independent FAIL review: corrective validation

The complete review supplied in the task was read before editing. Starting
candidate: `2357435428762261cd2cab5fbd217cdc73f93444`. Read-only remote verification
returned main `dca470eecd78fc3587cb4fbc4b279bf1091f156d`. The original candidate and
unrelated worktrees are untouched; correction uses a separate worktree.

## Independent reproduction before production fixes

New isolated synthetic tests reproduced all three P0 findings. Test-only
thread-local syscall failure injection forced unsupported exchange errors.

- Apply with a card replaced after the final check returned success and lost
  the replacement when exchange returned ENOSYS.
- Undo with a late replacement returned success and lost the replacement when
  exchange returned EINVAL.
- The real `/proc` detector reported Running while this test process held its
  synthetic card open. A proc root containing only the real current process
  allowed the initial scan, then refused publication after the restore pinned
  the card itself.
- Direct raw FAT-word checks after guarded restore showed the restored file
  tail as `0x7fffffff` (free) and the released old directory cluster as
  `0xffffffff` (allocated). This oracle does not use EmuWiz's chain reader.

Command: `scripts/cargo-iso --low-debug test --offline -p archivefs-core --lib
review_regressions -- --nocapture`, through the isolated child-environment runner.
Result: **5 passed, 0 failed, 0 ignored, 11670 filtered out**, 4.68s test time.
Production code was unchanged apart from a cfg(test)-only syscall seam. Logs
remain under `/tmp/emuwiz-ps2-psu-p0-execution/reproduction.log`.

## Source verification

Verified the reported apply/undo fallbacks, exchange implementation, pin/scan
ordering, FAT writer and permissive reader, unconditional unwind rename,
allocation from directory reachability, default journal discovery and GUI
in-memory-only Undo, corrupt-journal filtering, late staging intent, swallowed
journal errors, mode-only preservation, and public deprecated legacy writers.
Line numbers refer to the original candidate and change as corrections land.

The FAT-bit contract was independently checked in PCSX2's actual
[MemoryCardFolder.h](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/SIO/Memcard/MemoryCardFolder.h)
and [MemoryCardFolder.cpp](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/SIO/Memcard/MemoryCardFolder.cpp):
bit 31 marks allocation, chain-end is the low-bit end marker with the allocation
bit set, and free selection tests the allocation bit. The implementation is
used as reference data only; no PCSX2 code is copied into the application.
The public-domain [mymc reference](https://github.com/ps2dev/mymc/blob/master/ps2mc.py)
agrees. No real card, save or emulator was used.

Corrections and final validation will be recorded below. Expanded restore
remains unpromoted and requires independent re-review.

## Corrective changes

| Finding | Corrective path |
| --- | --- |
| Unsupported exchange destroys replacement | `restore_guard/exchange.rs`: apply and Undo require atomic exchange; EINVAL/ENOSYS/ENOTSUP are explicit refusals. No ordinary card rename fallback remains. |
| Self detection / aliases / ambiguous scans | `restore_guard/proc_scan.rs`: exclude this process, match open files by device/inode, identify executable names rather than arbitrary command arguments, expose unreadable live PIDs; readable zombies and vanished processes do not poison scans. |
| FAT markers / allocation | `memory_card_inventory.rs`: allocated chain-end is FFFFFFFF, released chains use 7FFFFFFF, free selection checks allocation bits and preserves allocated unreachable clusters. Readers reject a free marker in an allocated chain. |
| Additional FAT lookup defect | Resolve superblock IFC list -> indirect cluster -> FAT table -> FAT entry. Reject aliased metadata tables during planning/execution. Synthetic fixtures now encode this actual two-level layout; a second-table regression checks raw bytes. |
| Unconditional rollback / failed reversal | Conditional identity-and-hash checks plus exchange-and-verify; directory fsync; preserve both paths on uncertainty. Failure messages name the displaced artifact. |
| Public legacy alternate writers | Deprecated apply and Undo functions always return RecoveryRequired before I/O. Removed their ordinary rename writer. Guarded API tests replace the old unsafe compatibility expectation. |
| Missing durable creation intent | Persist backup path before backup creation and staged path/hash before staged creation. Retain partial artifacts and mark NeedsAttention after interruption. Undo intent covers its deterministic staging path; completed Undo identity is recorded before publication. |
| Restart Undo / corrupt journals | `pcsx2_page/restore_recovery.rs`: cached read-only discovery, explicit refresh, Published records offer Review Undo then Confirm Undo, corrupt records remain visible with preservation guidance. Recovery results use explanatory text. No automatic card restore or corrupt-record deletion. |
| Swallowed journal errors / concurrent operations | Journal write failures remain explicit. A nonblocking directory flock serializes cooperating apply/Undo/recovery operations without lock sidecars; journals are reloaded after acquiring the lock. |
| Metadata | Preserve mode, owner and group; refuse multiple hard links and existing xattrs/ACLs rather than discard them. Backups and atomically replaced journals use private file permissions. |

The journal schema adds optional Undo identity with a serde default for older
records. Older interrupted Undo records without that identity fail closed when
the completed image cannot be proven; no implicit card write repairs them.

## Safety contract and limitations

Planning remains read-only. Apply binds the reviewed PSU and card bytes to
identity/hash checks, records intent, creates and verifies a non-overwriting
backup, stages and verifies the result, then exchanges and verifies the
displaced inode. Unsupported exchange stops the operation. Undo verifies the
Published binding and backup and uses the same exchange mechanism. Rollback
refuses a changed live or displaced image. Backups are never deleted. Recovery
never publishes a card: it judges recorded states, preserves uncertain
artifacts, and updates journals. Corrupt journals remain untouched.

The directory lock coordinates EmuWiz operations; it does not lock out an
external emulator or privileged writer. Exchange verification detects changes
and retains artifacts on uncertainty; it is not a claim to prevent every
external write. Detection continues to refuse inaccessible *live* processes.
This intentionally does not waive Unknown: the GUI lists inaccessible PIDs on
explicit operations and offers a retry after processes close or proc access is
corrected. Environments with persistent proc restrictions can still refuse
restore. No manual "trust me" bypass was introduced.

The allocation-area upper-bound interpretation remains conservative: the
existing reader can refuse valid chains near the card's upper allocation
limit. This correction does not broaden geometry support. FAT semantics were
checked against independent PCSX2 source and raw synthetic assertions; no
console, emulator import, real card or real save acceptance is claimed.
528-byte spare/ECC cards remain refused, with no ECC implementation. Cards with
metadata the writer cannot preserve are refused. GUI history/actions remain
synchronous; moving heavy card operations off the UI thread is future work.

## Isolation and promotion boundary

Execution uses `/tmp/emuwiz-ps2-psu-p0-execution/run.py` to create child-only
HOME, all XDG roots, EMUWIZ_CONFIG_HOME and EMUWIZ_DATA_HOME overrides, with a
dedicated cargo-iso target under the same test-only root. Root and application
root permissions are 0700. Toolchain caches are reused; no production profiles,
configuration, approval files, journals, ROMs, cards or saves are opened.
The real `/proc` tests inspect process metadata and descriptors and bind only
to synthetic files; they launch a test executable, never an emulator. GUI tests
use headless egui contexts, not a launched GUI.

No other worktree was edited. The original candidate remains at 23574354.
This branch corrects that reviewed candidate; it does not promote or merge it
onto main. Main was verified read-only at dca470ee. Expanded restore remains
blocked from promotion until fresh independent re-review. No push or release
was performed.

## Original source anchors verified

At immutable candidate `23574354`, the reported locations match production
code: `restore_guard.rs:469–500` (exchange syscall), `610` / `758` (pin before
second scan), `771–778` (apply fallback), `874` (unconditional rollback),
`1031–1045` (Undo fallback), and `1285` (corrupt-journal exclusion).
`memory_card_inventory.rs:991` / `1222` emit a free chain tail, `1209` / `1213`
mark released entries allocated, `2288` accepts both end markers, and `1334` /
`1475` expose the legacy writers. The GUI's original interrupted-notice helper
and success-dialog-only Undo path were also checked. These are historical
anchors, not line references to the corrected files.

## Validation history

The initial five synthetic reproductions passed against the unsafe candidate
before fixes (they asserted its faulty behavior). They are preserved as positive
regressions in `restore_guard/review_regressions.rs`; failing behavior now makes
the corresponding regression fail. New fixtures are generated afresh per test.
The first repair compile caught a borrow conflict; it was corrected. A later
core run passed 74 tests and failed the old test asserting the public unsafe
API should work; that expectation was replaced by explicit refusal coverage.
The headless GUI fixture initially hit the SHA-2 0.11 digest-formatting API and
was corrected. These intermediate failures were not treated as validation.
The final crash-window regression also covers a complete staged image created
before its identity was persisted: it is retained with NeedsAttention.

A final cleanup audit found another boundary worth protecting: a staging name
must never identify the live card or backup. Cleanup now rejects both direct
path collisions and matching identities; creation refuses colliding paths.
A synthetic regression uses a card named with the staging prefix and a
colliding deterministic Undo name and asserts its bytes survive cleanup and
Undo refusal. This does not add any implicit restore/delete action.

## Final isolated validation

All Cargo commands below ran through `scripts/cargo-iso --low-debug` and the
isolated child-environment runner, with offline dependency resolution.

| Command after the wrapper | Exact result |
| --- | --- |
| `test --offline -p archivefs-core --lib memory_card_inventory::restore_guard -- --nocapture` | 59 passed, 0 failed, 0 ignored, 11636 filtered; 32.66s test time |
| `test --offline -p archivefs-core --lib memory_card_inventory -- --nocapture` | 80 passed, 0 failed, 0 ignored, 11615 filtered; 30.67s (includes the 59 guard tests) |
| `test --offline -p archivefs-gui --lib pcsx2 -- --nocapture` | 60 passed, 0 failed, 0 ignored, 3578 filtered; 1.30s |
| `check --offline --workspace --all-targets` | Exit 0; 1m47s compiler time |
| `fmt --all -- --check` | Exit 0 |
| `git diff --check` and scoped task postcheck | Exit 0; only the ten declared files changed; GUI root growth 0 |

The final real-proc observation returned Closed with no target holders,
emulator-name matches, unreadable live processes or listing failure in this
isolated execution context. The child-holder test separately detected the
synthetic hardlink alias through real `/proc`. The self-only real-proc
end-to-end test passed apply and Undo while their own descriptors were open.
These observations do not establish acceptance on another machine with
inaccessible live processes or a different filesystem.

Regression coverage includes all unsupported exchange errno variants in apply
and Undo, late replacement bytes and inode preservation, failed reversal in
both directions, conditional rollback after a late change, journal persistence
failure, partial backup/stage recovery, a complete unbound stage, cleanup/card
path collisions, FAT markers and second-table indirection, allocated orphans,
blocked legacy APIs, cooperating-operation locks, hardlink refusal, corrupt
batch recovery, restarted GUI Undo confirmation, cached discovery and read-only
missing-history viewing. Existing new-save, stale Undo, interrupted publication,
interrupted Undo and spare/ECC refusal tests also passed.

Logs are preserved at `/tmp/emuwiz-ps2-psu-p0-execution/validated-*.log`, with
commands and exit codes in `validation-summary.log`. Existing unrelated
unused-import/dead-code warnings remain; they were not modified. No test was
ignored. Final tests followed the last production correction. Only this report
was completed after the Cargo checks; the final diff/scope check follows it.

Commit is local-only on `fix/ps2-psu-independent-review-p0`. The resulting SHA
is supplied in the task completion response; independent re-review is required
before any promotion. No push, merge, release or real-data operation occurred.
