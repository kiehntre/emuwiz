# Cargo target isolation: global configuration migration

**Status: proposal only. Nothing here has been applied.** It needs separate approval and
coordination with active agents. The repository side (`scripts/cargo-iso`, `AGENTS.md`,
`CLAUDE.md`) is safe to use before this migration: the wrapper overrides whatever a shell
inherited.

## Why

`~/.bashrc` exports `CARGO_TARGET_DIR` and `~/.cargo/config.toml` sets
`build.target-dir`, both to `~/.cache/emuwiz-cargo-target`. That made every worktree (64
at the time of writing) share one target by default, and concurrent builds produced
misleading results. Because the environment variable beats any project config, a repository
`.cargo/config.toml` could not fix this on its own.

## Exact changes

`~/.bashrc` (keeps `CARGO_INCREMENTAL=0`; removes only the target export):

```diff
--- ~/.bashrc (current)
+++ ~/.bashrc (proposed)
@@ -180,6 +180,6 @@
 alias aiupdate='~/update-ollama-models.sh'
 alias updateemu='~/bin/update-emulators'

-# EmuWiz shared Rust build cache
-export CARGO_TARGET_DIR=/home/davedap/.cache/emuwiz-cargo-target
+# EmuWiz Cargo builds: do NOT export CARGO_TARGET_DIR here. Each worktree builds into its
+# own target through scripts/cargo-iso (see AGENTS.md).
 export CARGO_INCREMENTAL=0
```

`~/.cargo/config.toml` (the whole file is just the shared `target-dir`, so it is emptied or
removed):

```diff
--- ~/.cargo/config.toml (current)
+++ ~/.cargo/config.toml (proposed: file removed, empty)
@@ -1,2 +0,0 @@
-[build]
-target-dir = "/home/davedap/.cache/emuwiz-cargo-target"
```

Effect on other Rust projects: this global file applies to every Cargo project for this
user, not only EmuWiz. After the change, a project that is not built through
`scripts/cargo-iso` uses its own `./target` instead of the shared directory.

## Order of operations

1. **Land the repository side first** (`scripts/cargo-iso`, `AGENTS.md`, `CLAUDE.md`) and
   tell active agents to use the wrapper. Nothing else changes yet.
2. **Pick a quiet moment** and check nothing is building into the shared target: list the
   running `cargo` and `rustc` processes and confirm none of their `CARGO_TARGET_DIR` values
   is `~/.cache/emuwiz-cargo-target`.
3. **Back up, then apply** (backup both files with `cp -p`, for example to
   `~/.bashrc.pre-cargo-iso` and `~/.cargo/config.toml.pre-cargo-iso`, then make the two
   edits above by hand).
4. **Verify in a NEW shell** (existing shells keep their old environment):
   - `env | grep CARGO_TARGET_DIR` should print nothing;
   - `scripts/cargo-iso --print-target` should show a per-worktree target.
5. **Existing long-lived shells and agent sessions** (Claude Code, Codex, tmux panes) keep
   the old `CARGO_TARGET_DIR` in their environment until restarted. They are safe as long as
   they use `scripts/cargo-iso`, which overrides it. Do not edit their environment; let them
   restart naturally, or run `unset CARGO_TARGET_DIR` in interactive ones.
6. **Only then, and with its own approval,** consider removing the old shared target
   `~/.cache/emuwiz-cargo-target` (about 47.5 GB at the time of writing) once no process
   uses it. This migration does not delete it.

## Rollback

Copy the two backups back over the edited files and start new shells. `scripts/cargo-iso`
keeps working either way.

## Risks

- A shell or agent that was started before the change and runs bare `cargo` still builds
  into the shared target. Mitigation: `AGENTS.md` / `CLAUDE.md` forbid bare cargo; the
  wrapper overrides the inherited value.
- Non-EmuWiz Rust projects lose the shared default (see above). That is intended.
- Per-worktree targets use more disk in total than one shared target. The wrapper refuses to
  build under 8 GiB free, and `--low-debug` shrinks a target by roughly 4x.
