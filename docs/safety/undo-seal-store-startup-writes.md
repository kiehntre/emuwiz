# Open safety-review item: every process creates the Undo seal store

Status: **open**, recorded 2026-10-22 during v0.10.0 release acceptance.
Deliberately not changed in the v0.10.0 batch (owner decision): it belongs to
the sealed Undo protocol and needs its own independent review.

## Observed

Any invocation of `emuwiz` or `emuwiz-cli`, including `--version`, creates
these files in `$XDG_DATA_HOME/emuwiz/` (`~/.local/share/emuwiz/`) before it
parses its arguments:

```text
undo-seals.sqlite3
undo-seals.sqlite3-wal
undo-seals.sqlite3-shm
sealed-writers/<pid>                 (removed at exit)
finalise-capable-writers/<pid>       (removed at exit)
```

Reproduced with the v0.10.0 candidate under an empty, private HOME/XDG tree:
`emuwiz-cli --version`, `emuwiz --version` and `emuwiz-cli library-status`
each leave the three `undo-seals.sqlite3*` files behind. Also seen in the
authentic published-v0.9.0 upgrade test, where the first `--version` after
installing v0.10.0 added them to the user's data directory.

## Existing tests that fail because of it (main at 54876e13, 2026-10-22)

`scripts/cargo-iso test -p archivefs-cli` on current main:

- `inspect_read_only::inspection_changes_nothing_and_creates_no_config_database_or_cache`
  and 7 tests in `read_only_status`: HOME/data gains `undo-seals.sqlite3*`,
  `sealed-writers/` and `finalise-capable-writers/`.
- `rename_clean_install`: 1 test, `~/.local` must not exist after a clean
  install check.
- `repair_rollback_exit_code`: 9 tests; CHD Undo refuses the fixtures'
  journals as "legacy unsealed journal ... cannot be verified. Nothing was
  moved", that is, the sealed-journal gate fails closed on the old-format
  fixtures.
- `r2_deployment` (1) and `repair_apply_exit_code` (2) fail only when tests run
  in parallel ("an older EmuWiz is running": a sibling test's `emuwiz-cli`
  under a different private HOME is not registered in this test's data
  directory) and pass with `--test-threads=1`.

## Cause

`writer_gate::register_at_startup()` runs first in both entry points
(`crates/archivefs-cli/src/main.rs` `main`, `crates/archivefs-gui/src/gui_v2/mod.rs`
`run`). It calls `seal::self_check()`, which opens and prepares the seal store,
so that a running Blue never mistakes the process for an older, non-sealing
build.

## Effect

No user data is changed or removed; the files are EmuWiz's own state. It does
contradict the release contract that a direct run (and `--version`) writes
nothing, and the packager's `--version` identity probe therefore creates the
store in the builder's data directory.

## For the review

Whether a process that does no journal work (`--version`, `--help`, read-only
status commands) may skip registration without a concurrent sealing writer
treating it as an older build, and what that writer then does (expected:
refuse Apply/Undo while it runs, which fails closed).
