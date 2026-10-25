# Agent Worktree Guardrails

EmuWiz is developed concurrently in several worktrees. These guardrails make
ownership visible, keep generated data out of the repository, and protect the
GUI composition root while decomposition proceeds.

## Why `main.rs` is protected

`crates/archivefs-gui/src/main.rs` coordinates application state and page
composition. Feature logic placed there increases collision risk and makes
future extraction harder. Activity/History has already been extracted; the
navigation seam is the next intended cleanup area.

## The ratchet

`scripts/check-main-rs-boundary.sh` measures a diff, not an absolute file-size
limit. It allows up to 30 net added lines, allows any shrinkage, and fails
larger growth. This keeps small wiring changes practical while ensuring that
extractions naturally lower the future baseline. It intentionally does not
encode the current line count as a permanent maximum.

For an explicit, reviewed exception only:

```sh
EMUWIZ_ALLOW_MAIN_RS_GROWTH=1 scripts/check-main-rs-boundary.sh
```

The exception prints a warning. CI does not set it.

## Before a task

```sh
scripts/task-preflight.sh --baseline /tmp/emuwiz-nav-baseline.txt
```

The baseline stores canonical dirty/untracked paths only. It is not a license
to edit those paths; it merely lets the scope guard distinguish concurrent
pre-existing work from new task changes.

## After a task

```sh
scripts/task-postcheck.sh \
  --baseline /tmp/emuwiz-nav-baseline.txt \
  --allow crates/archivefs-gui/src/navigation.rs \
  --allow crates/archivefs-gui/src/tests/
```

The postcheck validates scope, `main.rs` growth, whitespace errors, and prints
all changed paths. It does not build or test automatically.

## Formatting and temporary fixtures

Use targeted rustfmt on touched Rust files. Use `cargo fmt --all -- --check`
only as a read-only validation unless broad formatting was explicitly requested.
Keep generated test artifacts in `TempDir` or `/tmp/emuwiz-<task>...`; use a
checked-in fixture directory only when the fixture is intentional and reviewed.
Never hide possible fixtures with broad ignore patterns.

## Scope examples

GUI navigation task:

```sh
scripts/task-postcheck.sh --baseline /tmp/emuwiz-nav.txt \
  --allow crates/archivefs-gui/src/navigation.rs \
  --allow crates/archivefs-gui/src/tests/navigation.rs
```

Core-only task:

```sh
scripts/task-postcheck.sh --baseline /tmp/emuwiz-core.txt \
  --allow crates/archivefs-core/src/feature.rs \
  --allow crates/archivefs-core/src/feature/
```

Exact paths are safest. A path ending in `/` permits files beneath that
directory. Staged, unstaged, and untracked paths are all checked.

## Concurrency

If a required file is already dirty, stop and report the exact overlap. Do not
reset, stash, restore, clean, discard, overwrite, or stage unrelated work.
Never absorb another worker's files just because they are nearby or make a
format/build command easier.
