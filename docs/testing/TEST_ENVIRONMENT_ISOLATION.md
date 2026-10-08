# Test environment isolation

Status: implemented for Linux test binaries of `archivefs-core`, `archivefs-gui`
and `archivefs-cli`. This document records the design, what it fixed, what was
found along the way, and the isolation gaps that remain. It does **not** claim
that every test is hermetic.

## The problem

Application code finds the user's configuration, application data, caches and
emulator installs through `$HOME` and the XDG variables, in roughly a hundred
places (`app_dirs`, per-emulator discovery roots, GUI settings, caches). A test
that reaches any of them used to read or write the real user's files:

* writes: `~/.local/share/archivefs/rom_organisation_approvals.json`,
  `~/.config/archivefs/{gui_mode.txt,onboarding_state.txt}`, the Dolphin cheat
  catalogue and generated `GAFE01.ini`, `scummvm.ini`, the RPCS3 Flatpak tree,
  RomM artwork `index.lock`, Libretro cheat staging and GameHacking `.pnach`
  staging;
* reads: `library.sqlite3`, RetroArch configuration, emulator `.desktop` files,
  `~/ES-DE`, the user's `config.toml`, and installed-emulator profiles.

Besides being unsafe, this made tests depend on the machine: two tests passed
only because the developer had a `config.toml` / a PCSX2 profile (below).

## Design

`archivefs_core::test_environment` (compiled only for `cfg(test)` or the
`test-support` cargo feature) points every test process at a private tree
**once, before `main`**, so no test thread exists yet and nothing can race:

* `HOME` and `XDG_RUNTIME_DIR` are set to directories in
  `<target>/<profile>/emuwiz-test-env/<exe>-<pid>/`, beside the test binary.
  It is deliberately **not** under `/tmp`: Flatpak visibility logic treats the
  host `/tmp` as invisible and several launch tests correctly refuse a home
  there.
* `XDG_{CONFIG,DATA,CACHE,STATE}_HOME`, `EMUWIZ_DATA_HOME` and
  `EMUWIZ_CONFIG_HOME` are **removed**, not redirected. An explicit XDG root
  outranks the legacy-`archivefs`-directory rule, so redirecting them changes
  what the production resolver does; removing them lets everything resolve under
  the private `$HOME` exactly as for a user with no overrides.
* The tree is removed at process exit (`atexit`), and only a directory whose
  parent is named `emuwiz-test-env` is ever removed.
* At exit a **tripwire** compares a handful of sentinel files in the *real* home
  (approvals, `library.sqlite3`, `gui-v2.json`, `gui_mode.txt`,
  `onboarding_state.txt`, `config.toml`) with their state at start-up and prints
  a loud warning on change. A real EmuWiz running at the same time can trigger
  it; it is a tripwire, not a verdict.
* Production code is untouched: the module, the macro and the `test-support`
  feature exist only in test builds (`test-support` is enabled solely from
  `[dev-dependencies]`, including core's self dev-dependency for its integration
  tests). The only production-source changes in this slice are the ones named
  under "Changes to tests and test seams" below.

### Installing it in a test binary

Each test binary needs one line (a Linux `.init_array` constructor registered in
that binary):

```rust
// lib.rs / main.rs, for unit tests
#[cfg(test)]
archivefs_core::install_test_environment!();   // `crate::...` inside core

// each file in tests/
archivefs_core::install_test_environment!();
```

Every integration test file in the workspace has it. **A new `tests/*.rs` file
must add the line**; `tests/test_environment_isolation.rs` shows the pattern.
On non-Linux targets the macro expands to nothing (see gaps).

### Real-machine runs

A test that must see the real machine has to say so: set
`EMUWIZ_TEST_REAL_ENVIRONMENT=1` and the isolation is skipped for that run.
There are no such tests today. Fixtures that write into `$HOME` use
`test_environment::private_home_for_fixtures()`, which **fails** (rather than
writes) when the process is not isolated.

## Failures found under isolation, and their causes

Reproduced with the isolated environment; none was assumed environmental.

| Failure | Cause | Resolution |
| --- | --- | --- |
| 5 × CLI `rename_clean_install` | The first design *set* the XDG variables; an explicit XDG root outranks the legacy-directory rule those tests exercise. | Design change: XDG variables are removed, not set. |
| 3 × `cheat_runtime_retroarch` Flatpak tests | First design put the private home under `/tmp`, which sandbox visibility treats as host-only. | Private tree moved beside the test binary. |
| `diagnostics::profiles::pcsx2_executable_override_is_folded_into_discovery` | **Hidden dependency on an installed emulator.** The override only attaches to a PCSX2 profile discovery found, and the test passed only on a machine that already had a PCSX2 profile in `$HOME`. | The test now creates a profile in the private home. |
| `gui_v2_duplicate_scan_on_the_real_worker_emits_phase_progress`, `gui_v2_cancelled_duplicate_scan_and_preview_stop_with_the_cancel_marker_and_change_nothing` | **Hidden dependency on the user's `config.toml`.** The real backend worker calls `Config::load_default`. | Tests seed a minimal config in the private home (atomic write, refuses outside it). |
| `emuwiz-cli cheatbase::release_packaging_has_no_database_input_or_database_member` | **Stale test, not an isolation issue.** Release assembly moved to `scripts/release/package_release.py`; the test still grepped `build-release.sh` for old member names and failed on any machine. | Updated to check the packager's current canonical members; the "no database in the release" assertions now cover both scripts. README/CHANGELOG/LICENSE are no longer asserted: the packager no longer lists them under those names. |
| 27 × `repair_history_page` / `repair_review_page` and GUI duplicate-scan failures reported earlier | Produced by the earlier ad-hoc experiment (scratch `HOME` **and** a scratch `TMPDIR`), not by isolation as designed. None of the repair-page tests fails under the built-in isolation; the two duplicate-scan tests were the config dependency above. | No code change needed for the repair pages. |

Earlier slices (already on main): the approvals sidecar path is injected per
`RomOrganisationPageState`, and `EsDeMediaState` is disabled in test builds
because a real `~/ES-DE` scan finishing mid-frame refreshed every loaded cover
and flaked layout tests.

## Changes to tests and test seams

* `archivefs-core`: new `test_environment` module, `test-support` feature,
  self dev-dependency (adds one `archivefs-core` entry to `Cargo.lock`).
* Every test binary installs the constructor (one line each).
* Regression tests: `test_environment::tests` (private home, directory helpers
  resolve inside it, production path semantics unchanged, parallel writers,
  approvals written through the production path never reach the real home),
  `tests/test_environment_isolation.rs`, plus one assertion test each in the GUI
  and CLI test binaries.
* Fixes to `pcsx2_executable_override…`, the two GUI duplicate-scan tests and
  the stale CLI packaging test, as above.

## Remaining isolation gaps

Headline items:

* **Linux only.** The constructor uses `.init_array`; other targets run
  un-isolated.
* **Absolute system paths** are still read for real: `/usr`, `/etc`, `/proc`,
  `/sys`, `/var/lib/flatpak`, `PATH` lookups, mount tables and `statvfs` of
  real mounts. These are deliberate machine probes (storage/doctor/emulator
  discovery), not user data, but they make some tests machine-dependent in
  principle.
* **`CARGO_HOME` / `RUSTUP_HOME`** are inherited as absolute paths and are not
  touched.
* **Environment variables not cleared**: `APPIMAGE`, `FLATPAK_ID`,
  `XDG_DATA_DIRS`, `XDG_CURRENT_DESKTOP`, `DISPLAY`/`WAYLAND_DISPLAY` and
  similar are inherited and can still steer discovery.
* **Child processes** inherit the isolated environment (intended), but a test
  that spawns a tool which reads a hard-coded absolute path is not covered.
* **The tripwire only watches sentinel files**, and a concurrently running real
  EmuWiz can trip it.
* Tests that need a hermetic *emulator* installation still build their own
  fixtures; there is no shared fixture builder, so new tests can regress this
  by assuming the machine's state. The two found here are the pattern to look
  for: a test that passes only where a profile / config already exists.

## Validation record

On the committed tree, through `scripts/cargo-iso`:

* `cargo check --workspace --all-targets`, `cargo fmt --all -- --check` and
  `git diff --check` clean.
* `cargo test --workspace --no-fail-fast -- --test-threads=4`: 15944 passed,
  0 failed, 57 ignored (the ignored tests were already `#[ignore]`d; none was
  added by this change). No tripwire warning.
* All 35 test executables (3 library binaries and 32 integration binaries, CLI
  345 / core 11625 / GUI 3624 library tests; every one passing) were run under
  `strace -f`, logging every open-for-write/create, `mkdir`, rename, unlink,
  symlink, link, truncate or rmdir whose path is **anywhere under `/home/davedap`**,
  excluding only this worktree's own build directory. Result: zero
  open-for-write events; the only `mkdir` calls were the `create_dir_all`
  walk over the three existing ancestors of that build directory (`EEXIST`,
  nothing created); the remaining matches were read-only opens of repository
  files whose names contain `rename`/`link`. An earlier run limited to
  `~/.config`, `~/.local`, `~/.var` and `~/.cache` (excluding the whole shared
  `emuwiz-cargo-targets` directory) also found nothing; the broader run
  replaced it because that exclusion was too wide. A control open of a real
  file `O_RDWR` showed the filter fires.
* Not covered by that evidence: writes outside `/home/davedap` (for example
  `/tmp` or a mounted ROM share), reads of the real home, writes by processes
  that tests spawn without `strace -f` following them (it did follow children),
  and doc-tests (the workspace has none today; a future doc-test would run in a
  process without the constructor).
* Release-packager tests: `scripts/release/test_package_release.py` 22/22
  (including the new `test_22_...` which refuses every `DENIED_PAYLOAD_NAMES`
  entry as a file or directory). `test_release_packager_sbom.py` has one
  **pre-existing, unrelated** failure: `test_01_valid_verified_sbom_…` asserts
  `package_count == 493` but the workspace lock file has 517 packages (identical
  on `origin/main`); it is stale test data, not caused by this change, and was
  left alone.
* A release build (`--locked`) of `emuwiz` and `emuwiz-cli` contains no
  `test_environment` symbol and none of the module's strings; `cargo tree`
  shows `test-support` only on the dev-dependency edge.

To re-check after changing test infrastructure, repeat the strace run against the
test executables (`scripts/cargo-iso test --workspace --no-run` prints them).

## Historical incident: the real approvals file

`~/.local/share/archivefs/rom_organisation_approvals.json` is the user's real
ROM-organisation approval sidecar. What was observed, and what was not:

* **2026-10-07 22:51:22 (local)**: the file was found containing exactly
  `{"version":1,"approved":["/roms/game.iso"]}` (43 bytes). That is byte-for-byte
  the payload the old `persist_clears_the_warning_on_success` test wrote through
  the production data path (it approved `/roms/game.iso` on a default state and
  persisted it). So the *content* is explained: some run of a test binary built
  before the fix of commit `a1b5e2a9` wrote it.
* **Which process wrote it is not established.** Several test runs from different
  worktrees and sessions were using the real home at that time, including runs by
  the session that found the file. No evidence (process accounting, file
  history) was captured that distinguishes them, so no run is blamed here.
* **2026-10-08 00:44:56 (local)**: the file's content was observed as
  `{"version":1,"approved":[]}` (27 bytes), i.e. the synthetic entry was gone.
  The real GUI persisting an empty approval set, or another un-isolated test
  clearing approvals, would both produce this; the writer was not identified and
  is not attributed. The same window also showed changes to `gui-v2.json`,
  `gui_mode.txt` and `onboarding_state.txt`, which the real application or
  un-isolated test runs may have written.
* The file was **not** modified, restored or deleted by this work. It was read
  and `stat`ed (read-only) during diagnosis on 2026-10-07/08, before the final
  review; no later check opens it. Whether its current content reflects what
  the user wants is the user's decision; nothing here restores it
  automatically.

The isolation work makes a repeat impossible from test binaries that install it:
no `strace`d test process opened anything under the real home for writing (see
"Validation record").
