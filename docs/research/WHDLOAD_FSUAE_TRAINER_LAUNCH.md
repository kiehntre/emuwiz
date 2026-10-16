# WHDLoad trainer options on the FS-UAE launch command

Status: core launch wiring complete and tested (L5). **No GUI launch path calls it yet, and FS-UAE
consuming the channel (L6) is not proven:** a real-binary probe shows plain `fs-uae` ignores
`--x-whdload-args`.

## Flow

```text
plan_fsuae_whdload_launch_with_saved_trainers(&WHDLoadLaunchInput, &AmigaGameInspection, option_file)
  -> (AmigaWHDLoadCommandPlan, WhdloadLaunchTrainerResolution)
preflight_and_launch_amiga_whdload(&plan)   // the command carries the trainer and re-proves it
```

1. Only an `InstalledDirectory` target (an extracted game folder holding the selected host slave
   directly) on FS-UAE can carry options. LHA packages, HDF slaves, unverified games and Amiberry
   resolve to `NotApplicable` and get exactly the baseline plan.
2. `discover_whdload_trainer_options(inspection, &[])` must be `Ready` with a verified identity
   that equals the launch input's identity, and its slave must be the launch input's slave.
   `SlaveChanged` is refused.
3. `option_file(&discovery)` is core's storage contract, `whdload_trainer_option_file`
   (`<data>/whdload-trainer-options/<sha256(identity)>.options`). It is passed in so this code never
   builds that path itself. A storage error is refused.
4. The file is read once with `O_NOFOLLOW`, checked on the open handle (absent = `NoSavedOptions`;
   a symlink, folder, FIFO, more than 1 MiB or non-UTF-8 is refused). As in the editor, only
   `CUSTOM1`-`CUSTOM5` / `C1`-`C5` keys are trainer data; other lines such as `PRELOAD`,
   `CUSTOMSAVE` or `CUSTOM9` are ignored and never reach the command. Each slot line must be
   `=` a decimal u32, once per slot.
5. Discovery runs again with those bytes. The slave hash must be unchanged. Each saved slot must be
   declared by the slave with `Ready`, non-opaque options. Bit (`X`/`M`) values may set only
   declared bits, `L` must be one of the declared choice numbers, and `B` must be 0 or 1.
6. Zero values are dropped (WHDLoad treats an absent CUSTOMn as 0), so "everything off" is
   byte-identical to no trainer. The rest go through `project_whdload_trainer_launch_options`.
7. The planner adds them to `--x-whdload-args`, bound to the identity, slave path/hash and
   option-file hash. It refuses a trainer on an LHA target or on another game or slave, and it
   refuses anything that is not `CUSTOM1-5=digits`. It also refuses a slave name with spaces,
   quotes, control characters or `=`, which could split the shared WHDLoad argument string.
8. The command stores the trainer, so the canonical `preflight_amiga_whdload_launch` (used by
   every launch path) re-hashes the slave and the option file and checks the exact argv before
   spawning. Any change between planning and spawn is refused.

EmuWiz never writes the game, the slave or the option file. The installed folder is passed with
`--hard-drive-0-read-only=1` (FS-UAE's `hard_drive_0_read_only`, present in the 3.1.66 binary)
because FS-UAE mounts folder hard drives writable by default. A WHDLoad game that needs to save
into its own folder will need a separate writable save path; that is not wired. Arguments are
separate argv elements, and no shell is involved.

## Exact arguments

Installed folder `/games/My Game` with `MyGame.slave`. Its declarations are `C1:X:Infinite lives:0`,
`C1:X:Infinite energy:1` and `C1:X:In-game keys:3`. Saved option file: `CUSTOM1=11`.

```text
fs-uae --config=<profile> "--hard-drive-0=/games/My Game" --hard-drive-0-read-only=1 "--x-whdload-args=MyGame.slave CUSTOM1=11"
```

With no saved options, or all options off: `--x-whdload-args=MyGame.slave`, byte-identical to the
installed-folder baseline. An LHA package is unchanged: `--hard-drive-0=<pkg>.lha --x-whdload-args=<slave>`.

## What the real emulator showed (2026-10-03, re-checked 2026-10-16, FS-UAE 3.1.66)

On 2026-10-03, with a disposable HOME under Xvfb, a dummy `.lha` was opened as a raw HDF, and
`x_whdload_args` never appeared in the log. On 2026-10-16, `strings /usr/bin/fs-uae` showed only
`whdload_quit_key` and no `whdload_args`. The `x_*` keys appear to be consumed by FS-UAE Launcher,
which is not installed here.

So the existing WHDLoad channel, LHA or folder, with or without trainers, is **not demonstrated** to
boot WHDLoad with the plain binary. This work makes saved options travel that channel correctly and
safely; it cannot make the channel work.

## Remaining before trainers can be called active at launch

1. **Storage API on main:** `patch_manager::whdload_trainer_storage` (Blue GUI commit `bca52596`).
   Callers pass `|d| whdload_trainer_option_file(d).map_err(|e| e.to_string())`.
2. **GUI caller:** `emulator_setup/controller.rs` builds only LHA requests. A request for an installed
   folder needs the inspection kept and a call to `plan_fsuae_whdload_launch_with_saved_trainers`
   instead of `build_fsuae_whdload_command_plan`.
3. **Real WHDLoad boot mechanism for FS-UAE:** either FS-UAE Launcher or a generated WHDLoad boot
   folder. If it differs from `--x-whdload-args`, only the final argv string moves; the binding and
   refusal logic does not depend on the channel.

## Capability level

L1–L4 existed. L5 (argv from saved options, unit-tested with disposable fixtures) is done here.
L6 (emulator consumes the options): **no**. L7: no.
