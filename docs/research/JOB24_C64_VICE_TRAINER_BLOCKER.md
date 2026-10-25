# Job #24: C64 VICE trainer integration — blocked inventory

Audited 2026-10-25 from freshly fetched `origin/main`
`d10374baa87593d632b88b78ec36eeab567c7c30`, against
[issue #111](https://github.com/kiehntre/emuwiz/issues/111).

Disposition: **BLOCKED at the requested existing-Blue-launch-seam gate**.
No launch, parser, memory-write, approval, or emulator-setting code was changed.
This report does not close #111 or claim trainer activation.

## Exact missing integration

Gold implements standalone VICE discovery, command planning, fresh preflight,
and process spawning. Blue does not connect these to Play:

- `crates/archivefs-gui/src/launch_readiness_page.rs`:
  `launch_command_preview` has no `vice` adapter branch;
  `standalone_launch_request` has no `vice` branch and falls through to `None`;
  `StandaloneLaunchRequest` and `StandaloneProcess` have no VICE variant.
  Consequently no Blue VICE request reaches preflight, spawn, or process tracking.
- Searching all `crates/archivefs-gui/src` finds no caller of
  `discover_vice_profiles`, `build_vice_command_plan`, `preflight_vice_launch`,
  `spawn_vice`, `ViceMonitorScript::create`, or `spawn_vice_with_cheats`.
- `emulator_setup_page.rs` lists a VICE setup candidate using platform mappings.
  Its VICE/RetroArch coexistence tests prove card presentation, not a Play path.
- `launch_readiness_page/classic_pokes.rs::Chooser::show` takes a
  `RetroArchLaunchRequest`, returns early outside ZX Spectrum, and restricts
  activation to native RetroArch/Fuse. Its launch action cannot carry VICE.

The prerequisite is a Blue standalone VICE context/request and command preview,
with current identity and exact executable binding passed through Gold's existing
preflight, explicit acknowledgements, spawn, and process lifecycle. Adding that
baseline path here would expand beyond the user's stop condition. No substitute
RetroArch C64 core, new launch adapter, or weakened preflight was introduced.

## Installed executable and monitor interface

Read-only host inspection:

- Debian package `vice`: `3.7.1+dfsg1-2build3`.
- `/usr/bin/x64sc` and `/usr/bin/x64` are regular executable files.
  `/usr/bin/x64sc -console -version` reports `x64sc (VICE 3.7.1)`.
- SHA-256 of `/usr/bin/x64sc`:
  `e3fafd35e65ed271011cd694790998bfabe20a91b64acf9efe61d50795d5cc1c`.
  SHA-256 of `/usr/bin/x64`:
  `c695d1dfb36205d09b7529c4f4282a14a37b72f4850e86c55bb5dcaa6e0d83f8`.
- PATH resolves `x64sc` first to `~/.local/bin/x64sc`, a symlink to a shell
  wrapper supplying external system-ROM paths. Gold's `vice_local.rs` requires
  a regular, non-symlink executable named exactly `x64sc` or `x64`; that wrapper
  path must not be treated as the verified `/usr/bin/x64sc` binding.
- `/usr/bin/x64sc -console -help` exited 0 in private HOME/XDG directories and
  advertised `-moncommands <Name>`, `-initbreak <value>` and `+saveres`.
  Plain `-help` failed without a display; console mode resolved that probe.
- Installed `/usr/share/doc/vice/vice.texi.gz` documents monitor byte writes
  (`>`), execution of a command file, and the default pre-kernal-reset timing.
  It warns that startup can overwrite changes. `-initbreak ready` concerns
  kernal readiness, not proof that a particular game has loaded.

These checks establish executable presence and advertised interface support.
No game was launched, no monitor script was executed, and no memory write or
in-emulator activation was confirmed. No binary digest was newly approved.

## Existing reusable functionality and limits

| Area | Existing implementation | Evidence boundary |
| --- | --- | --- |
| Gold C64 parsing | `classic_poke_family.rs`: `parse_simple_pokes`, `manual_poke`, typed `PokeCheat` operations | Parses POKE values without executing BASIC or imported monitor text |
| Gold VICE projection | `project_poke_runtime` / `project_vice_monitor` | Explicit unbanked main-memory byte writes; refuses restoration, guards, conditions, wider writes, invalid values, I/O, ROM-mapped and cartridge-banked targets |
| Verified association | `classic_trainer_evidence.rs`: `assess_poke_cheats`, exact hash matching and trainer pin validation | A current DAT identity plus a matching trainer digest can establish association; title or filename alone cannot |
| Blue trainer evidence | `gui_v2/classic_trainers.rs` and `pinning.rs` | Already displays options, provenance, emulator compatibility and explicit pin preview/confirmation |
| Sidecar ingestion | `assess_sidecar_pok` and pin preview use `parse_zx_pok` / `normalize_zx_pok_family` | ZX banked operations and restoration fields remain ZX semantics; they are not supported C64 entries merely because a sidecar is beside a C64 game |
| Existing verified launch bridge | `classic_trainer_evidence/launch.rs` | ZX Spectrum, approved native RetroArch/Fuse pair, bounded 48K snapshot subset only |
| Gold VICE launch | `launch/vice_command.rs`, `vice_execution.rs`, `patch_manager/vice_local.rs` | Exact executable/content identities and fresh rediscovery; direct `.t64`, `.g64`, `.d81ns`, `.crt`; no arbitrary archive or weak-format fallback |
| Monitor artifact | `ViceMonitorScript`, `preflight_vice_launch_with_cheats`, `ViceCheatSession` | Generates text from typed commands and checks script digest/game key; temporary lifetime handling is not receipt-backed recovery |
| Shared receipts and cleanup | `launch/cheat_runtime.rs` and existing RetroArch bridge | Reusable preparation/process/activation stages, source bindings, cleanup and stale-workspace reporting; VICE's low-level session is not connected to this runtime |

A future narrow VICE selection can reuse Gold's unguarded, non-restoring,
unbanked 8-bit projection and conflict checks. The current classifier admits
ordinary RAM and colour RAM; that classification alone does not verify live
banking, colour-value semantics, safe machine-control addresses, or game-load
timing. Those need a bounded, empirically verified contract before activation.
Do not silently strip unsupported fields or reinterpret ZX entries as C64.
The current Blue sidecar/pin ingestion therefore also lacks a supported C64
entry journey despite Gold's C64 parser and generic association assessment.

After the baseline Blue launch seam exists, remaining #111 work includes a
default-OFF selection and distinct approval bound to exact entries, fresh game,
trainer, pin and executable observations, faithful projection refusals, and
receipt-backed artifact cleanup/recovery through the existing runtime.
Preparation, process start and confirmed activation must remain distinct.

## Validation and delivery

This is a source inventory and installed-executable probe, not a passing gameplay
or Rust regression run. Existing test bodies were inspected, not executed.
No launch/memory-write candidate exists requiring independent safety review;
that review and normal CI remain required for the eventual implementation.
Documentation validation uses scope, boundary and whitespace checks.

Dedicated worktree: `/home/davedap/emuwiz-job24-c64-vice-20261025`;
branch: `job24/c64-vice-trainer`. Cargo metadata was read through `cargo-iso`
to register its dedicated target
`~/.cache/emuwiz-cargo-targets/emuwiz-job24-c64-vice-20261025-80ea7e4173c3`;
no Rust compilation was needed for this blocked report. Game media, emulator
configuration, source ROMs, approval files and production library data were not
modified. The task stops here under the user's explicit missing-seam boundary.
