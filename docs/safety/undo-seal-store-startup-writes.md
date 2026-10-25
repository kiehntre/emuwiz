# Undo seal-store startup writes: fixed, remaining profile limitations

Status: read-only startup writes **fixed** 2026-10-22; cross-profile false
refusals remain open. The original observation and review questions below are
historical (candidate `54876e13`), retained to explain the correction in
[Read-only registration](#read-only-registration-fixed-2026-10-22).

## Original observation (before the correction)

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

Disposition (2026-10-22):

- `r2_deployment`, `repair_apply_exit_code`: test isolation corrected (spawned
  binaries get a private empty process table). Not a production defect.
- `repair_rollback_exit_code`: test isolation corrected (the spawned CLI now
  shares the test process's isolated HOME, so it sees the seal store the
  in-process conversion wrote). The refusal it hit is correct fail-closed
  behaviour; in production the writer and the Undo resolve the same data
  directory.
- `inspect_read_only`, `read_only_status`, `rename_clean_install`: a genuine
  (minor) production defect, fixed 2026-10-22 (see "Read-only registration").
  Skipping startup registration for read-only commands was tried and
  reverted after independent review: an unregistered read-only process counts
  as an older writer, so a concurrent Blue Apply/Undo would refuse, possibly
  part-way through. That changes guard semantics and was not accepted.

## Read-only registration (fixed 2026-10-22)

- `--version`, `-V`, `--help`, `-h`, `help`, `emuwiz --version`, `inspect`, `saves`, `emulators`, `activity`, `history`, `config-check`,
  `library-status` and `database-check` register only in the per-user runtime
  registry `/run/user/<uid>/emuwiz/` (path from the kernel uid; every level
  must be an owner-only directory of this user; the base is never created).
  The record is the same one, verified against `/proc`, so sealing writers
  still recognise them. They never fall back to the data directory.
- Limitation: where `/run/user/<uid>` is missing or not private (containers,
  minimal servers, `su` without a login session) those commands run
  unregistered, and a concurrent EmuWiz pauses organising, renaming and Undo
  (fails closed) until they end; `-v` says so.
- Limitation: development builds made between the seal store (19 October)
  and this change look only in the data directory, so they treat a running
  new read-only command as an older writer (fail closed). No published
  release is affected (v0.9.0 does not seal).
- Unchanged: write-path registration, `writer_gate::check()`, older-process
  detection and seal verification. A command that turns out to write journals
  still registers in the data directory through `check()` first.
- Not EmuWiz state: in a brand-new HOME, `saves` runs `flatpak`, which
  initialises its own empty `~/.local/share/flatpak` repository.

## Cross-profile false refusals (open)

Registration is looked up only in the checking process's own data directory
(`registrations_dir()`), so a current EmuWiz running under another data
directory (another profile, or an isolated test) also looks like an older
writer and blocks Rename/Apply/Undo everywhere else. The runtime registry
above could also carry writers' records to fix this; not done in v0.10.0.

Fixed in `bec8952a`: journal removal, compact enrollment/reopening,
descriptor publication and compact record/checkpoint commits now enforce
`writer_gate::check()`. Refusal preserves journals, seals, claims and source
bytes. These paths remain separate from read-only commands.

## Original cause (before the correction)

`writer_gate::register_at_startup()` runs first in both entry points
(`crates/archivefs-cli/src/main.rs` `main`, `crates/archivefs-gui/src/gui_v2/mod.rs`
`run`). It calls `seal::self_check()`, which opens and prepares the seal store,
so that a running Blue never mistakes the process for an older, non-sealing
build.

## Original effect (before the correction)

No user data is changed or removed; the files are EmuWiz's own state. It does
contradict the release contract that a direct run (and `--version`) writes
nothing, and the packager's `--version` identity probe therefore creates the
store in the builder's data directory.

## Original review question (resolved above)

Whether a process that does no journal work (`--version`, `--help`, read-only
status commands) may skip registration without a concurrent sealing writer
treating it as an older build, and what that writer then does (expected:
refuse Apply/Undo while it runs, which fails closed).
