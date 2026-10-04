# PCSX2 cheat runtime feasibility

Status: **blocked for this host, no adapter added.** EmuWiz level for PCSX2
cheats at launch time: **L0 (no launch wiring).** PNACH file management and
local cheat install (`patch_manager/pcsx2_pnach.rs`, `local_cheat_install_pcsx2.rs`)
are separate features already on main; `feature/pcsx2-cheat-gui-apply` is a
GUI apply lane, not a launch lane. No worktree has dirty PCSX2 files.

## Evidence base

- Host `saltbox26`, 2026-10-04. Standalone PCSX2 is present as:
  - `~/Applications/PCSX2/PCSX2.AppImage`, **v2.9.9**, with `portable.ini`
    beside it (portable; data root is that directory; its `PCSX2.ini` already
    has `EnableCheats = true`, `EnablePatches = true`, and a relative
    `Bios = ../../../../../../../run/user/1000/doc/...` document-portal path).
  - Flatpak `net.pcsx2.PCSX2` **v2.8.2** (`~/.local/bin/pcsx2-qt`), data root
    `~/.var/app/net.pcsx2.PCSX2/config/PCSX2`.
  - A third data tree `~/.config/PCSX2` (no matching binary seen).
  - No native install. EmuWiz's PCSX2 launch adapter
    (`launch/pcsx2_execution.rs`) launches native profiles only and never
    Flatpak, Portable, AppImage or NativeAlternate, so the existing launch
    plan cannot start any PCSX2 on this host.
- Installed `-help` (v2.9.9) options include `-portable`, `-datapath <path>`,
  `-logfile <path>`, `-elf`, `-fastboot`, `-batch`, `-nogui`, `-testconfig`.
  `-gamecfg` exists in master but is **not** in the installed v2.9.9 help.
  There is no cheat-path option.
- Upstream source at master `fb1373cc` (`Patch.cpp`, `Pcsx2Config.cpp`,
  `QtHost.cpp`, `VMManager.cpp`). Not matched to the 2.8.2 Flatpak.
- No PCSX2 process was started and no PCSX2 state was touched; no PS2 ELF/disc
  fixture is available and none of the user's games were used.

## Semantics (from source)

- **Format:** PNACH. Cheats live in `EmuFolders::Cheats`, bundled/user patches
  in `EmuFolders::Patches` (and `patches.zip`); both use `<serial>_<CRC>.pnach`,
  or `<CRC>.pnach` when there is no serial. Identity is serial + game CRC.
- **Cheats are not patches.** Cheats need `[EmuCore] EnableCheats`, and are
  enabled individually by name through `[Cheats] Enable` in the settings layer.
  Patches use `[Patches] Enable/Disable`, plus separate widescreen and
  no-interlacing switches. Unlabelled pnach groups are enabled automatically.
- **Evidence in logs:** `Found N cheats in <file>.` and `Enabled patch: <name>`
  are written by `Console.WriteLn` (to `emulog.txt` or `-logfile`). That would
  give emulator-side proof of consumption if a game can be booted.
- **Data root:** `ShouldUsePortableMode()` (portable.ini/portable.txt beside the
  executable, or `-portable`) has **absolute priority**; only otherwise is
  `-datapath` honoured, then `$XDG_CONFIG_HOME/PCSX2`, then `~/.config/PCSX2`.
- **Folders** (`[Folders] Cheats`, `Patches`, `MemoryCards`, `Savestates`,
  `Bios`, ...) are settings in `PCSX2.ini` and can differ from the data root.

## Why launch-scoped wiring is not available

1. **Portable wins.** The user's AppImage has `portable.ini`, so neither
   `-datapath` nor `XDG_CONFIG_HOME` can move its data. The only way to stage
   cheats for it is its real `cheats` folder, which is not allowed.
2. **No launchable install.** The existing EmuWiz launch plan refuses
   Flatpak/AppImage/portable PCSX2, and no native PCSX2 is installed, so an
   adapter could not be exercised end to end and would be dead code here.
3. **A design exists for non-portable installs but is unproven.** Start PCSX2
   with `-datapath <workspace>/data` (explicit CLI, no environment needed),
   seed `PCSX2.ini` from the user's, rewrite every folder that must stay real
   (`Bios`, `MemoryCards`, `Savestates`, `Snapshots`, `Textures`, `Cache`,
   `InputProfiles`, `Videos`, `Logs`...) to an **absolute** real path, point
   `Cheats` into the workspace, and stage `<serial>_<CRC>.pnach` plus a
   per-game ini with `[Cheats] Enable = <chosen names>`. Hazards: the user's
   own `PCSX2.ini` uses relative folder paths (see the Bios entry above) that
   would break in a copied ini; patches and the widescreen/no-interlacing
   switches must be carried over unchanged so a cheat launch does not change
   patch behaviour; per-game settings for that game must be copied, not lost;
   the CRC must be computed from the ELF the same way PCSX2 does.
4. **Not probed.** Without a bootable fixture (a PS2 ELF plus BIOS under a
   working display) nothing above was observed running, so no memory-card or
   save-routing claim can be made.

## Smallest change that would unblock it

- A native, non-portable PCSX2 (or a decision to let EmuWiz launch the
  AppImage/Flatpak), a legal homebrew ELF fixture and a display setup that
  boots PCSX2; then one probe with before/after hashes of the real data root
  showing `Found 1 cheats in <staged file>` in `-logfile` output and a memory
  card write landing in the real `memcards` folder.
- Or upstream: a per-launch cheat folder option, or portable mode that does not
  override an explicit `-datapath`.

## Next step

Do the boot probe first. If it passes, the adapter is a thin
`cheat_runtime_pcsx2.rs` over the canonical runtime in the style of
`cheat_runtime_melonds.rs`, using `-datapath` as the explicit argument and
refusing portable installs and unverified serial/CRC.
