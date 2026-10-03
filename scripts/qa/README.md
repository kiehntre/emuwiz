# QA scripts

Disposable-state tools for testing EmuWiz without touching real data. None of
them builds the project, contacts a provider or modifies ROMs, and each works in
temporary or explicitly chosen folders.

| Tool | What it does | More |
|---|---|---|
| `release-smoke.sh` | Runs the shipped CLI against a disposable profile: first start, restart, a disposable source, SQLite integrity, isolation and clean shutdown | below |
| `release-smoke-selftest.sh` | Tests the smoke harness itself (missing binary, unsafe paths, failure, timeout, cleanup) | |
| `synthetic_library.py` and the `build-`, `validate-`, `run-` and `remove-synthetic-library.sh` wrappers | Builds, checks, scans and removes a deterministic synthetic game library | [Synthetic Library Lab](../../docs/QA_SYNTHETIC_LIBRARY_LAB.md) |
| `tests/test_synthetic_library.py`, `test_synthetic_ux.py` | Self-tests for the lab generator and its GUI recovery fixtures | same |
| `upgrade_preflight.py`, `upgrade-preflight-selftest.sh` | Read-only check of which EmuWiz and ArchiveFS folders are active, with an optional backup manifest | [README-upgrade-preflight.md](README-upgrade-preflight.md) |
| `pending-recovery-inspector.sh`, `pending-recovery.py` | Read-only inspector for unfinished operations | [Pending operation recovery](../../docs/QA_PENDING_OPERATION_RECOVERY.md) |
| `sunshine-acceptance-helper.sh`, `sunshine-acceptance-helper-selftest.sh` | Prints the manual real-desktop (Sunshine/Moonlight) acceptance checklist and records run metadata (source SHA, artifact hash before and after, DISPLAY, PID, leftovers). It builds nothing, drives nothing, and writes records outside the repository (`EMUWIZ_QA_RUN_DIR`, default `~/.local/state/emuwiz-qa/sunshine-runs`) | [Real emulator launch matrix](../../docs/qa/REAL_EMULATOR_LAUNCH_MATRIX.md) |

## Release smoke

`release-smoke.sh` creates a temporary `HOME`, XDG folders and EmuWiz config and
data folders, runs the CLI with a scrubbed environment (no inherited tokens or
provider credentials), and removes everything after a successful run. It checks
that nothing appeared under your real config, data, cache or state folders.

It finds a binary from `EMUWIZ_BINARY` (a path to `emuwiz-cli`) or from
`CARGO_TARGET_DIR` (an existing Cargo target folder). It never builds one:

```sh
EMUWIZ_BINARY=/path/to/emuwiz-cli scripts/qa/release-smoke.sh
CARGO_TARGET_DIR=/path/to/existing/target scripts/qa/release-smoke.sh
```

Controls:

- `SMOKE_TIMEOUT=30` bounds every launched process (seconds).
- `KEEP_SMOKE_STATE=1` keeps the temporary folder for debugging.
- `SMOKE_HEADLESS=1` (the default) uses the headless CLI path; GUI automation is
  never attempted.
- A failing run keeps its temporary folder and prints its location. Logs
  (stdout, stderr, environment summary, database checks, filesystem summary) are
  under that folder's `logs/`.

A passing run ends with `RELEASE SMOKE: PASS`.

For packaged-release checks (verification, SBOM, signing, packaged GUI smoke)
see `scripts/release/README.md` and `scripts/release/README-RC-ACCEPTANCE.md`.
