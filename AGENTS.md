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
