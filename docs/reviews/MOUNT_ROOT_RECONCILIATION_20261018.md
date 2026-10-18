# September mount-root reconciliation: focused recovery

Target: `/home/davedap/emuwiz-mount-root-reconciliation-final-20260902`.
Branch: `integration/mount-root-reconciliation-final-20260902`, exact tip
`372874391eb9504deb83095845a94d4f983e62d9`. Fresh comparison main:
`802af0676d0314674933967bdd9bd3959cfe73e9`.

**TREASURE:** recover the remaining lexical guards and validate ancestors before
probing write access. Retire the obsolete worktree only after promotion and fresh
preservation, ownership and lifecycle checks. Its original four dirty files are
never cleaned, reset or overwritten.

## What is already represented

The target tip is an ancestor of main and has no unique commits. The dirty changes
add config-first validation, proposed-text round-trip checking, path guards,
format-preserving assignment editing and persistent Sources feedback.
`2a6831c5` and `b07d51cd` already supply most of these responsibilities on main.
Current `setup_controller.rs`, `sources_ui_state.rs` and `sources_page.rs` retain
the feedback after configuration reload, with explicit success/failure details.
The old GUI root changes do not need replaying.

The current assignment editor preserves the existing quoted-value boundary,
spacing, comments and CRLF. Existing regressions cover malformed config before
probing, invalid proposed text, symlinked config, writable targets and reload.
Different source layouts and old test names do not establish missing behavior.

## Remaining correction

The shared destination validator rejects `..` and symlinked ancestors but accepts
filesystem roots and normalizes `.`. The mount-root setter previously checked
writability before invoking that validator. The recovery adds an I/O-free lexical
preflight for absolute, non-root, losslessly representable paths without raw dot
components or line breaks, then reuses the existing ancestor validator before
the writable-directory probe. Config parsing, round-trip verification and atomic
publication remain in the existing setter.

The focused regressions never probe `/`: root and invalid spelling cases call
the pure validator. Other cases use disposable config files and directories.
No production config, ROM, source scan or emulator is involved. Cemu scanner
integration `397cf3c7` remains untouched; the patch changes only mount-root code
and its focused tests, not RPX admission, cache bypass or provenance.

## Preservation and retirement

Audit evidence lives under `2026-10-18-mount-root-reconciliation-final` in the
existing audit ledger. All four original dirty files have exact local copies
and match the existing USB `20261016-uncommitted-worktrees` archive byte-for-byte.
All five per-worktree backup artifacts match recorded checksums. The full USB
history bundle checksum matches its original manifest and contains the exact
branch tip. Unique commits: zero. No preservation bundle is modified.

The existing Cargo worktree gate reports no isolated cache and CLEAR. A trusted
privileged host process census found no source path or inode references and no
userspace visibility gaps. These checks and exact HEAD/status/dirty hashes must
be repeated immediately before removal. The user's explicit target-specific
retirement instruction permits removing this preserved dirty worktree directly;
the earlier generic cull's no-force rule is not permission to reset its contents.
Any drift, untracked/ignored addition, process reference, lock or Cargo HOLD stops
retirement. Only this worktree and its exact obsolete local branch are in scope.

Final test/review/promotion evidence and the actual deletion receipt, including
allocated SSD bytes reclaimed, are recorded in the audit ledger and the existing
Treasure Hunt retirement receipts. No Git garbage collection or object pruning
is performed. Filesystem checks remain point-in-time validation; this recovery
does not introduce an atomic filesystem namespace transaction.
