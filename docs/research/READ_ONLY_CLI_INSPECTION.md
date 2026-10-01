# Read-only CLI inspection (current-main reconciliation)

Base: `1872e1cbdb562e50d2746f6466ea2692dd44e730`. Both candidates were 202
commits behind main (merge base `ee807a9a`) and were reviewed, not cherry-picked.

## Candidate verdicts

| Candidate | Verdict |
| --- | --- |
| `a47a05d2` "add read-only save state CLI" | **Superseded.** The save-state CLI is already on main (`21452b2d`). The commit only reorders imports in `saves.rs` and `save_state_orchestration.rs` (older rustfmt ordering that current `cargo fmt` rejects). Nothing ported. |
| `0f985d55` "polish read-only CLI inspection" | Mixed; each hunk classified below. |

`0f985d55` hunks:

| Hunk | Classification |
| --- | --- |
| `saves inspect <id>` takes the id as a record id, not an emulator filter | **Still useful, ported.** On main `saves inspect <id> --json` filtered by emulator `<id>`, printed an empty report and exited 0. Now an unknown id is an error in both modes. Regression test added. |
| import reordering in `saves.rs` / `save_state_orchestration.rs` | Superseded (formatting only). |
| `emulators status` | **Requires adaptation; not ported.** `inspect_discovered_emulator_lifecycles` runs `--version` probes against emulator binaries and queries package managers/flatpak. That executes external programs, which this read-only surface must not do. The debug-formatted JSON (`format!("{:?}")`) also fails the typed-output requirement. |
| `activity` / `history` | **Unrelated / collision risk.** Reads operation receipts including database backup/restore history via `default_database_path`. Outside file inspection and next to the database area, so left out. |
| fast `status --json` + `statuses_from_archive_index` | **Unrelated; changes an existing command's output shape.** Not ported. |

## What exists on main (reused, not duplicated)

`platform-detect <path>` (platform evidence), `game-identity-inspect` (catalogued
identity), `media-set inspect|explain|plan` (topology), `saves` (state inventory).
They are separate commands with different inputs and no combined answer.

## New: `emuwiz-cli inspect <path>`

One read-only view that composes the existing reports for one path:

- platform detection (`detect_platform_report`, bounded content read)
- catalogued game identity (`inspect_catalogued_game_identity_in_roots`)
- media-set topology (`inspect_paths` + `resolve_index`; no walk, hashing, DB or subprocess)
- the pure launch-topology projection (`project_media_set_for_launch`)

Views: `--identity --evidence --readiness --media --provenance` (default all),
`--json`, `--platform <name>`, `--root <dir>`. No new identity or evidence model.

### Evidence classes

Core statuses are re-labelled so a claim is never stronger than its source:

- `verified_fact`: Verified and read from the bytes (exact bytes or structured metadata).
- `catalogue_context`: Verified status but only restates a supplied hint or catalogue context.
  (With no hint the core still reports `Platform = Verified`; that is not evidence about the file.)
- `filename_inference`, `candidate`, `ambiguous`, `invalid`, `incomplete`, `unsupported_or_unknown`.

Each fact also has a category: `identity` (serials, title/product ids), `checksum`,
`attribute`, `platform_context`. The mapping is an exhaustive match, so a new
`IdentityKind` must be classified before this builds. A checksum is **not** an
identity: a ROM with a verified SHA-256 reports `verified_checksum: true`,
`verified_identity: false` and an `insufficient_evidence` blocker.

### Outcomes and blocker reasons

`outcome`: `recognised_verified`, `recognised_unverified`, `ambiguous`, `not_recognised`.
Blocker `reason`: `not_recognised`, `ambiguous`, `insufficient_evidence`,
`unsupported`, `media_set_blocked`, `review_required`, `incomplete`.
Input errors are distinct: `not found or not accessible`,
`unsafe/refused` (FIFO, device, socket, or a symlink to one, never opened),
`unsupported option`, `exactly one path is required`.

### Safety

- Never opens the database or catalogue (`"database": "not opened"`), never
  creates config/cache, never spawns a process, discovers or launches an
  emulator, or writes. User review decisions and provider data are therefore not
  consulted and the output says so.
- Emulator/firmware readiness is reported as *not evaluated*: it needs an
  emulator selection that a path cannot supply.
- A directory target caps the media walk at 64 files.
- **ScummVM directory identity is skipped, not run.** The core identity
  inspector (and the media record that reuses it) asks the installed ScummVM
  executable to `--detect` a game directory, which spawns a process and writes a
  temporary config. Review found `inspect --platform scummvm <dir>` did exactly
  that (confirmed with `strace -f -e execve`). `inspect` now reports
  `identity.evaluated: false` with a reason for that one case and does not pass
  the hint to the media record. An `strace` sweep over 13 input/hint
  combinations (files, directories, archives, FIFO-free, spaced and non-ASCII
  paths, hints `ps2`, `gamecube`, `ps4`, `scummvm`) shows exactly one `execve`
  (the binary itself) and no write, network or link syscalls.
- The topology projection reports `UNSUPPORTED` for representations the topology
  engine does not model (for example cartridge ROMs); the output states this is
  not a statement about emulator support.

## Pre-existing behaviour outside `inspect`

- `saves ...` (on main) calls `inventory_configured_state`, which calls
  `inspect_discovered_emulator_lifecycles` and therefore runs emulator `--version`
  probes and package-manager/flatpak queries (about 13 s on this host).
  `game-identity-inspect --platform scummvm --path <dir>` also runs the ScummVM
  detector. Neither is part of `inspect`'s contract and neither was changed here.

## Pre-existing, unrelated

- `crates/archivefs-cli/tests/{rename_clean_install,repair_rollback_exit_code}.rs`
  used `CARGO_BIN_EXE_archivefs-cli`; the binary is `emuwiz-cli`, so the crate's
  integration tests did not compile. Fixed (one identifier each).
- `cheatbase::tests::release_packaging_has_no_database_input_or_database_member`
  fails on main: it asserts `scripts/build-release.sh` contains `archivefs-cli`
  but the script now says `emuwiz-cli`. Release packaging is outside this task;
  not changed.
