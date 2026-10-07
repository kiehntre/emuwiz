# CLAUDE.md

Read `AGENTS.md` first: it holds the repository's agent rules (worktree safety, file scope,
formatting, temporary files, commits). Everything there applies to Claude Code.

## Cargo builds

Run every Cargo command through `scripts/cargo-iso` (for example
`scripts/cargo-iso test -p archivefs-gui --lib`). It builds into a target directory unique
to the current worktree, overrides any inherited `CARGO_TARGET_DIR`, refuses the legacy
shared target `~/.cache/emuwiz-cargo-target`, and refuses to build when under 8 GiB is
free. Never run bare `cargo` for builds, never export a shared `CARGO_TARGET_DIR`, and never
clean or delete a target directory you did not create. See the "Cargo builds" section of
`AGENTS.md` for details, including shells that inherited the old shared target.
