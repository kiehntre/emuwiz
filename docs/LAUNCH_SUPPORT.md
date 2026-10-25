# Launch support

EmuWiz does not provide a universal emulator frontend. It builds a launch
plan only after the game identity and content path are sufficiently verified,
checks the selected emulator profile and firmware state, and fails closed on
ambiguous or incomplete evidence.

## What the launch states mean

- **Launch supported** — EmuWiz can re-check the game and profile, build the
  command, and start the supported emulator process.
- **Ready/planned** — EmuWiz can identify a compatible target and show its
  readiness or command plan, but this target does not currently have the same
  native execution path.
- **Blocked** — required identity, media, profile, firmware, or safety
  evidence is missing or contradictory.

## Current execution targets

The current production launch code contains execution paths for selected
RetroArch, Dolphin, PCSX2, DuckStation, Flycast, MAME, PPSSPP, RPCS3,
ScummVM, xemu, and Xenia workflows. The exact platform and media coverage
varies by emulator. A listed emulator is not a promise that every game or
format for that emulator can launch.

PlayStation 4 is recognised conservatively from explicit PS4 folder names
and shared package candidates. shadPS4 installation evidence can be shown in
Emulator Setup, but PS4 PARAM.SFO identity validation, extracted-layout
verification, and shadPS4 launch planning are not yet implemented. EmuWiz
therefore must not present a PS4 item as verified or launchable solely because
it has a `.pkg`, `eboot.bin`, or `sce_sys` path.

The GUI exposes launch readiness and supported launch actions from the game
details workflow. The CLI and GUI use the shared launch planning and
identity evidence, but launch execution is currently primarily surfaced by
the desktop workflow.

## Safety boundaries

Before execution, EmuWiz re-inspects the relevant content, rediscovers the
emulator environment where required, rebuilds the plan and command, and
checks the remaining preconditions. It invokes a configured executable with
an argument vector rather than a shell command. It does not modify emulator
configuration, download firmware, mount archives as part of launch, or
execute unverified content.

RetroArch and selected standalone emulator paths may use direct loose media.
Archive-member, multi-disc, CHD, and other container layouts are supported
only where the relevant identity and input projection are implemented.

For the implementation reference, see the [launch module](../crates/archivefs-core/src/launch/mod.rs)
and [adapter support matrix](ADAPTER_SUPPORT_MATRIX.md).
