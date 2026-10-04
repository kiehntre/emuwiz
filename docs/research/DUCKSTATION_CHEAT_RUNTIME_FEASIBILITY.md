# DuckStation cheat runtime feasibility

Status: **not proven, no adapter added.** EmuWiz level for DuckStation cheats
at launch time: **L0 (no launch wiring).** The CHT file management
(`patch_manager/duckstation_cheat.rs`, on main; see
`DUCKSTATION_NATIVE_CHEAT_ADAPTER.md`) is a separate feature.

## Evidence base

- Host `saltbox26`, 2026-10-04. Standalone DuckStation exists only as
  AppImages, version `0.1-11894-gc66b2694d (dev)`:
  - `~/Applications/DuckStation/DuckStation.AppImage` with `portable.txt` and
    `settings.ini` beside it: **portable**, data root is that directory.
  - `~/Applications/emulators/DuckStation.AppImage` (what `~/.local/bin/duckstation`
    links to): not portable, data root `~/.local/share/duckstation`.
  - No native (distro/package) install. EmuWiz's DuckStation launch adapter
    (`launch/duckstation_execution.rs`) deliberately launches native profiles
    only and never Portable/AppImage/Explicit installs, so on this host the
    existing launch plan cannot start either copy.
- Installed `-help` options: `-batch -fastboot -slowboot -bios -resume -state
  -statefile -exe -fullscreen -nofullscreen -nogui -bigpicture -earlyconsole --`.
  **No cheat, settings-file or profile option.**
- Upstream source read at master `4122fed9` (`core.cpp`, `cheats.cpp`,
  `settings.cpp`, `system.cpp`, `qthost.cpp`); the installed build is a
  2026-09-12 dev build, close to it.

## Semantics (from source)

- **Format:** `<serial>.cht` or `<serial>_<16-hex hash>.cht` (sectioned
  `Type = Gameshark|Assembly`, `Activation = Manual|EndFrame`, code lines),
  looked up in `EmuFolders::Cheats`. A separate `Patches` mechanism exists.
- **Identity:** disc serial (from SYSTEM.CNF / game database), optionally the
  game hash. Title/filename are not used.
- **Enabling:** only through the *per-game* settings layer
  `<GameSettings>/<serial>.ini`: `[Cheats] EnableCheats = true` and
  `Enable = <cheat name>` (`Cheats::AreCheatsEnabled` says "Only in the
  gameini"). There is no CLI or global switch.
- **Folders are configurable:** `settings.ini` `[Folders] Cheats`,
  `GameSettings`, `SaveStates`, ..., `[MemoryCards] Directory` and `[BIOS]
  SearchDirectory` are read by `EmuFolders::LoadConfig` and may be absolute.
- **Data root (Linux, `Core::SetDataRoot`):** first, if `portable.txt` or
  `settings.ini` exists beside the (real) executable, that directory; else
  `$XDG_CONFIG_HOME/duckstation` when `XDG_CONFIG_HOME` is absolute; else
  `~/.local/share/duckstation`.

## What was probed (bounded, nothing real touched)

With the non-portable AppImage run from an extracted copy with
`XDG_CONFIG_HOME=<scratch>` and a seeded scratch `settings.ini`
(`[Folders] Cheats/GameSettings` and `[MemoryCards] Directory` pointing at
scratch paths, `[BIOS] SearchDirectory` at the real bios folder):
DuckStation read the scratch `settings.ini` (strace), created its folders under
the scratch data root, mapped the real BIOS through the absolute path, and
the real `~/.local/share/duckstation` and `~/Applications/DuckStation` trees
were byte-identical afterwards (md5 over all files). So a private data root
with absolute real-folder overrides is accepted by the emulator.

**Not proven:** a synthetic ISO9660 disc with SYSTEM.CNF and a tiny PS-EXE did
not boot under Xvfb (no boot log, the process idled until the time limit; a
modal error dialog is the likely cause but was not seen). No legal PS1 fixture
was available, so DuckStation was never observed opening
`<serial>.cht`, reading the per-game ini, or routing a memory card to a given
folder. No claim above L0 is made.

## Blockers and hazards

1. **Portable precedence.** The portable check runs before `XDG_CONFIG_HOME`,
   so the user's `~/Applications/DuckStation` copy cannot be redirected by the
   environment at all. Only non-portable installs qualify.
2. **No launchable install here.** The existing EmuWiz launch plan refuses
   AppImages; an adapter around it would be untestable end to end on this host.
3. **A viable design exists but is unproven.** Private data root through
   `XDG_CONFIG_HOME=<workspace>/config`, a seeded copy of the user's
   `settings.ini` with `Cheats` and `GameSettings` pointed into the workspace
   and BIOS, memory cards, save states, screenshots, textures, shaders, covers
   and input profiles pointed at the real directories by absolute path, and a
   staged `gamesettings/<serial>.ini` (a copy of the real one, if any, plus the
   two `[Cheats]` keys) and `cheats/<serial>.cht`. Memory cards and save states
   would then remain the real ones. This needs a boot probe before it can be
   trusted.
4. **Seeding hazards:** `settings.ini` can hold a RetroAchievements token
   (strip it, and tell the user achievements are signed out for that launch);
   per-game memory cards depend on `[MemoryCards] Card1Type`; any folder missed
   in the override silently becomes empty in the scratch root.

## Smallest change that would unblock it

- A boot-capable legal fixture (homebrew disc or `.exe`) and a working
  display setup for DuckStation, then one probe that hashes the real data root
  before and after and shows `strace` opening the staged `.cht` and per-game
  ini, plus a real-path memory card write going to the intended folder.
- Or upstream: a command-line `-settings <file>` / `-cheats <file>` option.
- For this host, a native (non-AppImage, non-portable) DuckStation, or a
  deliberate decision to let EmuWiz launch AppImage installs.

## Next step

Do the boot probe first. If it passes, the adapter is a thin
`cheat_runtime_duckstation.rs` over the canonical runtime in the style of
`cheat_runtime_melonds.rs` (child-only `XDG_CONFIG_HOME`, staged settings,
per-game ini and `.cht`), refusing portable installs and unverified serials.
