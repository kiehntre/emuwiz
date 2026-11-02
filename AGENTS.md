# Repository Agent Guardrails

These rules apply to coding agents working in EmuWiz.

## Worktree safety

- Run `git status --short` before editing.
- Treat pre-existing dirty and untracked files as owned by someone else.
- Never reset, stash, restore, clean, discard, or overwrite unrelated work.
- Never stage unrelated files or fix unrelated warnings/errors without permission.
- If a task-required file is already dirty from another task, stop and report the overlap.

## File-scope contract

- Every task must have an explicit allowed file list or allowed area.
- Compare actual changed files with that scope before committing.
- Report unexpected modified or untracked files; do not silently absorb them.

Use a baseline for concurrent work:

```sh
scripts/task-preflight.sh --baseline /tmp/emuwiz-task-baseline.txt
scripts/task-postcheck.sh --baseline /tmp/emuwiz-task-baseline.txt \
  --allow path/to/allowed-file.rs
```

## GUI root architecture policy

`crates/archivefs-gui/src/lib.rs` is a coordination boundary. (It was
`src/main.rs` until the GUI became a library with thin `src/bin/` launchers;
the policy is unchanged, only the path.) It may contain
application bootstrap, genuinely global `ArchiveFsApp` state, top-level
eframe/egui composition, thin cross-feature dispatch, and coordination that
really spans multiple subsystems.

It must not become the default home for feature business logic, parsers,
compatibility or repair calculations, feature-specific database logic, large
feature dialogs, action/outcome machinery, long rendering helpers, or feature
tests. Feature-specific source logic belongs in a focused controller/module.

Before adding significant code, state why an existing or new focused module is
not appropriate. If more than 30 net lines of feature logic would be added,
stop and reconsider placement. Small wiring changes are allowed.

## Feature ownership

- A feature owns its state, actions, rendering, and helpers.
- A controller owns focused orchestration and that feature's persistence/async coordination.
- the GUI library root owns app-wide coordination only.

Reuse focused modules where possible; do not create parallel models casually.

## Formatting policy

Use targeted `rustfmt` for touched Rust files. Do not run `cargo fmt --all` in
write mode during a focused task unless explicitly requested. `cargo fmt --all
-- --check` is suitable for validation. Avoid unrelated formatting churn.

## Cargo builds: one target directory per worktree

Run every Cargo command through `scripts/cargo-iso`, for example:

```sh
scripts/cargo-iso check --workspace --all-targets
scripts/cargo-iso test -p archivefs-gui --lib gui_v2::history_view
scripts/cargo-iso --low-debug test -p archivefs-gui --lib   # smaller target
scripts/cargo-iso --print-target                            # read-only: target + free disk
```

The wrapper builds into `~/.cache/emuwiz-cargo-targets/<worktree>-<12-hex path hash>`,
one directory per worktree, so concurrent worktrees can never corrupt each other's
build results or share a cache by accident. It passes all Cargo arguments and the exit
code through unchanged. It never cleans or deletes anything.

Rules:

- Do not run bare `cargo build/check/test/clippy` in a worktree, and never export a
  shared `CARGO_TARGET_DIR` or `build.target-dir`. Two worktrees must not share a target.
- Never use the legacy shared target `~/.cache/emuwiz-cargo-target`. The wrapper
  refuses it (including through `--target-dir`).
- Never run `cargo clean` on, or delete, a target directory that is not yours. Another
  agent may be using it.
- The wrapper refuses to build when less than 8 GiB is free (`EMUWIZ_MIN_FREE_GB`).
  Free space first; do not lower the threshold to force a build on a full disk.
- Release scripts (`scripts/build-release.sh`, `scripts/compare-release-builds.sh`) manage
  their own target explicitly and are not affected.

Keeping builds warm (a target is only reused by the same worktree path):

- A new worktree means a new, cold target: every dependency compiles again (several
  GiB and many minutes). Nothing is being deleted; the cost comes from creating
  worktrees. For review, rebase and integration work, reuse an existing worktree you own
  (`git switch` / `git switch --detach <sha>`) instead of adding one per candidate.
- The target is keyed by the worktree path, so it is reused across commits, rebases and
  test runs in that worktree. Do not rename or move a worktree you are still building in.
- Debug mode is recorded per target and kept. A target with no compiler output to keep
  (new worktree, or retired with a verified receipt: `worktree-check --cargo-state-only`
  says CLEAR) builds
  with `--low-debug` by default: full debug info
  is ~93% of each test executable (measured on `verified_game_fixture`: 563 MB -> 37 MB;
  cold target 3.2 GB -> 1.5 GB, build 3m08s -> 2m06s). An existing target built before
  the record keeps its old behaviour. `scripts/cargo-iso --print-target` shows the mode.
- Switching mode (`--low-debug` / `--full-debug`) recompiles the workspace and keeps both
  copies until the target is retired, so never switch an existing worktree mid-task. Use
  `--full-debug` from the start only when you need a debugger or line-numbered
  backtraces; panic and assert messages give `file:line` either way.
- A shell that exports `CARGO_INCREMENTAL=0` (older `~/.bashrc` files do) recompiles a
  whole crate on every edit. `scripts/cargo-iso --incremental ...` (or
  `EMUWIZ_CARGO_INCREMENTAL=1`) turns it back on for that run. Measured on the
  archivefs-gui tests: edit-to-rebuild ~92-104 s without, ~26-33 s with, after a one-off
  ~390 s priming build, and the target roughly doubles in size. Use it in long-lived
  worktrees with plenty of free disk.
- `scripts/cargo-iso --sccache ...` (opt-in; or `EMUWIZ_CARGO_SCCACHE=1`) routes rustc
  through sccache with its own 2 GiB-capped cache (`~/.cache/emuwiz-sccache`, port 4241),
  keeps each worktree's own target, and forces incremental off (it cannot be combined with
  `--incremental`). Measured on `build -p archivefs-core` across two worktrees at different
  commits: 139 hits / 352 compiles on the second worktree and 173.7 s against 183.3 s with
  no cache, so about 5% - modest. A cold sccache run adds some overhead. Do not expect
  it to replace reusing a worktree's own warm target.

Shells that still carry the old shared target: some long-lived shells and agent
sessions started before this change inherited
`CARGO_TARGET_DIR=~/.cache/emuwiz-cargo-target` from `~/.bashrc`. `scripts/cargo-iso`
overrides it (and prints a one-line note), so using the wrapper is always safe. Bare
`cargo` in such a shell would still use the shared target, so do not use it. In an
interactive shell you can also run `unset CARGO_TARGET_DIR`. Once the global defaults are
removed (a separate, coordinated step), new shells no longer carry it.

## Temporary files

Use `tempfile::TempDir`, `/tmp/emuwiz-<task>...`, or a deliberate checked-in
fixture directory for generated test data. Never drop generated files in the
repository root. Do not add broad `*.bin`, `*.sys`, or wildcard ignore rules
that could hide legitimate fixtures.

## Commits

Keep one coherent responsibility per commit where practical. Report starting
and resulting SHAs. Do not push unless explicitly requested.

## Guard commands

`main.rs` growth is checked by:

```sh
scripts/check-main-rs-boundary.sh
scripts/check-main-rs-boundary.sh --base <sha>
```

The threshold is 30 net added lines in a diff. `main.rs` may shrink freely;
this is a ratchet, not a permanent maximum line count. An explicit conscious
exception is available with `EMUWIZ_ALLOW_MAIN_RS_GROWTH=1`; it prints a
warning and must not be used by CI by default.

The scope guard accepts exact paths or directory prefixes and can ignore
pre-existing dirt recorded in a baseline:

```sh
scripts/check-working-tree-scope.sh --write-baseline /tmp/emuwiz-task.txt
scripts/check-working-tree-scope.sh --baseline-file /tmp/emuwiz-task.txt \
  crates/archivefs-gui/src/navigation.rs \
  crates/archivefs-gui/src/tests/
```

If a required file is already dirty, do not work around the ownership boundary
by overwriting it. Stop and report the exact path and overlap.

## Automatic repair and security policy

This policy applies equally to Claude Code and Codex. Automatic production
repairs apply to implementation tasks within their approved scope; they do not
authorise edits during read-only research or independent review. All existing
ownership, file-scope, Git, filesystem safety, testing and release requirements
remain in force. Task-specific restrictions still apply.

### Automatic defect repair

Investigate and repair bugs discovered within the assigned implementation scope.
When an error, failed test or security defect appears:

1. Determine the root cause.
2. Reproduce the failure using synthetic fixtures and capture pre-fix evidence.
3. Add a regression test that fails before the correction.
4. Implement the smallest safe correction.
5. Rerun the affected tests.
6. Continue until the defect is resolved or a genuine blocker is reached,
   subject to the attempt limit below.

Ordinary compilation errors and routine test failures do not require owner
permission to investigate, correct or retest within scope. Unrelated defects
must be reported rather than silently repaired outside the file-scope contract.

### Security and preservation first

Prioritise unexpected file deletion or overwriting, filesystem identity and
symlink races, database corruption and backup integrity, save-game preservation,
incorrect Undo or recovery results, stale user approvals, credential exposure,
unsafe temporary files, and misleading success messages.

Never weaken a security check just to make tests pass. Never remove a failing
test without a justified replacement that preserves its safety coverage.
Keep credentials out of fixtures, logs, command lines and reports. If safe
operation cannot be demonstrated, fail closed rather than claim success.

### Avoid endless repair loops

Allow at most three focused implementation attempts per defect. Track each
focused correction and its affected retest as one attempt. If the defect remains,
reconsider the underlying architecture instead of stacking additional checks or
resetting the count by renaming the defect.

Document the evidence, unresolved root-cause questions and design options. If
the design requires a substantial change, request owner approval before that
change. Do not keep rewriting unrelated code or exceed the approved scope.

### Independent review stays independent

When explicitly assigned a READ-ONLY INDEPENDENT REVIEW, do not modify the
candidate or perform production repairs. Reproduce suspected defects using
isolated fixtures or a separate harness, subject to the review's restrictions.
Report exact evidence and source locations, return PASS, PASS WITH CONDITIONS
or FAIL, and recommend the smallest safe correction.

The implementation owner, not the reviewer, performs production repairs.
Security-sensitive fixes require independent re-review of the exact final
commit; approval of an earlier version does not cover subsequent changes.

### Ownership and isolation

Before editing, check current Git state and active file ownership in addition
to the existing worktree and file-scope checks. Never overwrite another agent's
dirty work, and respect existing worktree boundaries.

Use `scripts/cargo-iso` for Cargo operations and private HOME/XDG/TMP/application
roots for tests. Never test destructive operations on real user data, including
libraries, databases, saves, profiles, installations or approval files. Use an
existing appropriately owned worktree when suitable; create an isolated worktree
only when needed. Do not create duplicate Cargo caches for the same worktree or
unnecessary build caches for documentation-only work.

### Cargo cache release and worktree removal order

Finishing a Cargo command is not finishing a task. When your task is done, **while the
worktree still exists**, run from it:

```
scripts/cargo-iso --finish-task --task <short-name>
```

A final `scripts/task-postcheck.sh` invocation can instead include
`--complete-task <short-name>`; it releases ownership only after the scope and integrity
checks succeed. Use this option only for conclusive task completion.

This releases the worktree's Cargo cache for automatic retirement (idempotent; pins remain).
The release refuses while a `cargo-iso` build holds the target's shared lifecycle lock; retry
after it exits. It reports whether the worktree may be removed. Keep the
worktree until `scripts/emuwiz-cargo-maintain worktree-check <worktree>` reports `CLEAR`.
Retirement verifies ownership from the source worktree, so removing the worktree first
strands the cache for manual cleanup. There is no mandatory inactivity delay. Release only
when the task is conclusively completed, promoted or explicitly abandoned, required commits
and evidence are preserved, and no agent owns or expects to reuse the target. Age alone
never authorises retirement. All process, open-handle, executable, pin, owner, path identity,
symlink/mount, approval and audit gates remain mandatory. Never release the running GUI target. The check never removes anything. There is no automatic hook into `git worktree
remove`; any cleanup workflow must call `worktree-check` before removing a worktree.

The explicit exception for a genuinely absent historical target is
`scripts/emuwiz-cargo-maintain worktree-reconcile <worktree> --remove`. This separate
command repeats independent Git, ownership, pin and host-process checks under lifecycle
locks and removes only the checkout; old cache retirement remains `UNKNOWN`. Never
substitute a saved reconciliation result for fresh authorization. See the legacy
reconciliation section of `docs/CARGO_CACHE_RETENTION.md`.

### Automatic completion

Within the approved implementation scope, continue through:

Implementation → Tests → Repair → Retest → Local commit → Report.

Stop only for genuine safety, ownership, scope, evidence or architectural
blockers, including an unresolved defect requiring architectural reconsideration
after the attempt limit. Explain the blocker and the evidence or decision needed
to proceed. Read-only assignments retain their own review/report completion path.

Never push, merge, release or enable dangerous operations without explicit
permission. A local candidate commit is not permission to promote or publish it.

### Reporting

At completion, report the defects discovered, root causes, repairs implemented,
new regression tests, final test results, remaining limitations, exact local
commit SHA, and whether independent review is required. Distinguish independently
executed validation from reported or proposed tests. If blocked, state what
remains unresolved and do not claim completion.

## AI Agent Efficiency and Usage Policy

This is the canonical usage policy for both Codex and Claude Code. Apply it
alongside all existing safety, permissions, filesystem, Undo, journal and Git
rules; efficiency never overrides those guarantees.

1. Assign one implementation owner per task and file set. Never let two agents
   modify the same files concurrently.
2. Default to one active implementation agent per task. Use additional active
   implementers only for explicitly authorised parallel work with disjoint file
   sets. Do not launch subagents automatically for routine work. Reserve
   independent review for high-risk preservation, filesystem, transaction,
   recovery and other safety-critical changes, or an explicit review requirement.
3. Never repeatedly poll builds, process lists, logs or Cargo locks using AI
   model turns. Use bounded shell waiters that write completion status and
   results to disk; inspect the result after completion or the bounded timeout.
4. Run focused tests during implementation. Freeze the candidate before running
   the full relevant final gate.
5. Reuse successful test and review evidence. Repeat it only when subsequent
   changes invalidate that evidence; safety-sensitive changes still require
   independent review of the exact final commit.
6. Read source selectively. Avoid repeatedly loading unchanged files, full logs
   or enormous diffs into context.
7. Keep command output concise. Save full diagnostic logs to disk and show only
   relevant errors, summaries and test totals.
8. After two unsuccessful attempts at the same approach, diagnose the underlying
   cause and change strategy instead of blindly retrying. The existing maximum
   of three focused correction/retest attempts per defect still applies; a
   strategy change does not reset it.
9. When changing tasks or replacing a large session, preserve a concise handoff:
   worktree, branch, HEAD, dirty files and ownership, completed tests and their
   candidate, blockers, and the next action.
10. Avoid excessive planning, speculative investigations, redundant documentation
    and unnecessary agent-to-agent status chatter.
11. Use appropriately priced models and reasoning levels for routine tasks when
    model selection is available and authorised. Reserve expensive reasoning for
    difficult or safety-sensitive work.
12. Avoid overlapping full builds. Respect the existing shared Cargo heavy-build
    lock as well as worktree-specific Cargo targets and retention rules.
13. Never sacrifice correctness, required independent safety review, final
    validation or preservation guarantees to save usage.
14. Do not start a new development campaign while assigned work is unfinished
    unless explicitly authorised.
15. After focused local tests and required source checks, push only when authorised,
    open/update the PR, verify CI targets the exact reviewed head, and check once
    for immediate failures. If checks remain running, report the PR, full head SHA,
    run links and PENDING, then end the development session. Never use
    `gh run watch`, repeated status/log polling or artifact downloads to wait for CI.
    GitHub's completion notifier handles the final notification; see
    `docs/ci-notifications.md`. After completion, perform a separate short
    merge-readiness check using completed results for that exact head. Collect
    detailed logs only for failures or a specific evidence requirement. Never
    claim green CI early or merge without approval; all security, preservation,
    reproducibility and exact-head merge protections remain mandatory.

### Completion standard

DONE means the assigned scope is fully implemented; Core and Blue are connected
where applicable; realistic user journeys are tested where applicable; confirmed
in-scope defects are fixed; relevant final checks pass on the exact candidate;
and independent review is complete where safety risk justifies it.
Documentation-only changes use formatting, scope and consistency checks rather
than Rust builds or application journeys.

For authorised integration tasks, DONE also requires a normal push and verification
of the exact SHA on GitHub main. Without explicit push permission, report a local
candidate awaiting authorised integration rather than claiming integrated DONE.
Never force-push or bypass existing promotion and preservation safeguards.
