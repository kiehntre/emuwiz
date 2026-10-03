# Cheat runtime: current status (main `d112b7f3`, 2026-10-03)

Scope: what current main code proves about cheats at launch time. Derived only
from code at this commit; nothing was executed and no emulator was launched.
Companion to `docs/design/CHEAT_PER_LAUNCH_WORKSPACE_V1.md`,
`docs/research/CHEAT_EMULATOR_CAPABILITY_MATRIX.md`,
`docs/research/RETROARCH_APPENDCONFIG_PERSISTENCE_TEST.md` and
`docs/cheats/SAFE_LAUNCH_COMPOSITION.md`.

Levels: 0 research, 1 parse, 2 normalise, 3 validate, 4 stage/install,
5 launch wiring, 6 runtime execution, 7 verified runtime effect.
Levels 1-4 are not proof that a cheat works in game.

## Planner, not executor

`crates/archivefs-core/src/launch/cheat_launch_plan.rs` is a pure planner
(`plan_cheat_launch`). Its own header says materialising, spawning, verifying
and cleanup "belong to a later executor". The only composer is RetroArch's
(`command_with_cheat_launch_plan` appends `--config <path>` to an existing
`RetroArchCommand`); `cheat_launch_capability` marks every other adapter
`PersistentInstallOnly`/not launch-scoped. Outside `launch/mod.rs` re-exports
and tests, nothing calls `plan_cheat_launch` or `command_with_cheat_launch_plan`,
and the derivative bytes are never written. Every `launch/*_execution.rs`
checked states it never touches cheats.

Highest level reached for any emulator in product-reachable code: **4**
(persistent staging/install, plus a config-based loadability check in
`cheat_loadability.rs`, which itself says EmuWiz has no runtime evidence that a
cheat executed). RetroArch has a library-level level-5 composer that is not
product-reachable. No emulator has level 6 or 7.

## What stages/installs

`cheat_apply_support` (`patch_manager/cheat_route.rs`) returns `Supported` for
RetroArch, `pcsx2`, `dolphin`, `xenia`, `duckstation`, `ppsspp`, `mgba`,
`mame`, `rpcs3`, `flycast`, `amiga_whdload` and `scummvm`. These publish files
through the shared journalled transaction (`shared_preview` ->
`shared_transaction`), with undo. That is persistent installation, not
launch-scoped.

## Adapters not reachable through that route

`melonds`, `mednafen`, `fbneo`, `azahar` and `vice` fall through to
`Unsupported`. Modules exist (`melonds_cheat.rs`, `mednafen_cheat.rs`,
`fbneo_cheat.rs`, `three_ds_cheat.rs`, `vice_c64_cheat.rs`) but a route-table
miss means the GUI cannot offer them. `three_ds_cheat.rs` and
`vice_c64_cheat.rs` have no shared-transaction apply at all (parse/validate/
projection only). `mame_cheat.rs` and `fbneo_cheat.rs` expose a
`RuntimeEnableRequired` state: staging does not enable the cheat in the
emulator. Classic Game Genie and Action Replay/GameShark decoders are
decode/preview only; encrypted and CodeBreaker-style input is retained as
opaque or rejected.

## WHDLoad trainer options

`whdload_trainer.rs` stages options and offers
`project_whdload_trainer_launch_options`, but `launch/amiga_whdload_command.rs`
contains no trainer/CUSTOM handling, so the options do not reach the FS-UAE
`--x-whdload-args` command. Still true at this commit.

## RetroArch: planner vs runtime

The persistence test shows `--appendconfig` values leaked into the base config
and `config_save_on_exit=false` suppressed one automatic rewrite (one core, no
content, RetroArch 1.22.2 Flatpak); overrides and explicit saves were not
covered. `cheat_launch_plan.rs` uses that as `PersistenceProof` and requires
state fencing. It is a planned design, not an observed in-game cheat.

## Next implementation boundary

Per-launch executor: materialise the derivative in launch-owned scratch, spawn
with the composed command, verify the state expectation, and clean up. Then
route additional emulators beyond RetroArch, wire FS-UAE trainer arguments, and
only then collect level 6-7 evidence.
