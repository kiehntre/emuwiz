# WHDLoad trainer options on the FS-UAE launch command

Status: L5 (launch wiring) only. **L6 is not proven, and a real-binary probe suggests the channel is not consumed by the `fs-uae` binary.**

## What this adds

`build_fsuae_whdload_command_plan_with_trainer` (and `preflight_amiga_whdload_launch_with_trainer`) carry the options a person selected through the existing WHDLoad trainer model (`patch_manager::whdload_trainer`, rendered by `render_whdload_arguments` as `CUSTOMn=number`) onto the existing `--x-whdload-args=<slave> CUSTOMn=…` argument.

- No trainer, or an empty selection: the command is byte-identical to the baseline plan.
- A selection bound to another game, another slave, or containing anything other than `UPPERCASE_SLOT=digits` (or a duplicate slot) is refused by the planner.
- Preflight additionally re-hashes the selected slave and refuses a selection made against a different slave file, and refuses a command that does not carry exactly the selected options. The canonical preflight and spawn are unchanged and still used.
- Nothing edits the package, the slave or any configuration; the options exist only on the command line. Amiberry is unchanged (its launcher contract has no proven WHDLoad argument channel).

## What the real emulator showed (2026-10-03, FS-UAE 3.1.66, Debian/Ubuntu package, disposable HOME, Xvfb)

Command shape: `fs-uae --config=<disposable.fs-uae> --hard-drive-0=<file>.lha --x-whdload-args="My.Slave CUSTOM1=1"` with a dummy file named `.lha`.

- FS-UAE logged `hard drive file: …game.lha` and opened it as a raw hard-disk image (`HDF … opened, size=0K`). It did not treat the `.lha` as a WHDLoad package.
- Nothing in the log mentioned `x_whdload_args`; the option was silently ignored. The binary contains no such string. `x_*` options appear to be consumed by FS-UAE Launcher, which is not installed on this host (only `fs-uae` and `fs-uae-device-helper` are).

So the existing FS-UAE WHDLoad launch contract (`--hard-drive-0=<lha>` + `--x-whdload-args`) is **not demonstrated to launch a WHDLoad game with the plain `fs-uae` binary**, with or without trainer options. This candidate makes the trainer options travel the same channel the baseline uses; it cannot make the channel work.

## Before promoting

Establish the real WHDLoad launch mechanism for the installed FS-UAE (FS-UAE Launcher, or a generated WHDLoad boot directory) with open material. If it differs, the argument string built here should move to that channel; the binding and refusal logic above is channel-independent.

## Capability level

L1–L4 existed. This adds L5 (argv, unit-tested). L6 (emulator instructed to consume the options): **no**. L7: no.
