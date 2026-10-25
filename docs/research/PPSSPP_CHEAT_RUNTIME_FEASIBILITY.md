# PPSSPP cheat runtime feasibility

Status: **blocked, no adapter added.** EmuWiz level for PPSSPP cheats at
launch time: **L0 (no launch wiring).** The CWCheat *file* management
(`patch_manager/ppsspp_cwcheat.rs`, already on main; see
`PPSSPP_CWCHEAT_NATIVE_ADAPTER.md`) is a separate feature, not a launch seam.

## Evidence base

- Host `saltbox26`, 2026-10-04. Standalone PPSSPP is installed as the user
  Flatpak `org.ppsspp.PPSSPP` **1.20.4** (flathub), launched through
  `~/.local/bin/PPSSPPSDL` (`exec flatpak run org.ppsspp.PPSSPP "$@"`).
  No RetroArch PPSSPP core was involved.
- Real memstick: `~/.var/app/org.ppsspp.PPSSPP/config/ppsspp/PSP/`
  (`SYSTEM/ppsspp.ini`, `Cheats`, `SAVEDATA`, `PPSSPP_STATE`, `TEXTURES`,
  `PLUGINS`, `GAME`). The real `ppsspp.ini` has `EnableCheats = False`.
- `PPSSPPSDL --help` (installed build) lists no cheat or profile option; the
  only config option is `--appendconfig=FILE`.
- Source read at tag `v1.20.4` (`3a31057b`, the installed version): `Core/Config.cpp`,
  `Core/CwCheat.cpp`, `Core/Util/PathUtil.cpp`, `UI/NativeApp.cpp`. Master
  `a2f4ce21` was read for comparison and agrees on every point below.
- No PPSSPP process was started and no PPSSPP state was touched, so there was
  nothing to hash or restore. A bootable PSP fixture is also not available.

## Semantics (all from source)

1. **Format:** CWCheat INI, `_S <game id>`, `_G`, `_C0`/`_C1` (off/on), `_L`
   code lines. Already modelled by the existing parser.
2. **Identity:** the booted game's disc id (`g_paramSFO.GetDiscID()`).
3. **Location:** only `<memstick>/PSP/Cheats/<discid>.ini`
   (`GetSysDirectory(DIRECTORY_CHEATS)`). There is no setting or option to
   point cheats elsewhere.
4. **Enabling:** `EnableCheats` (global, and `PER_GAME`: also readable from
   `PSP/SYSTEM/<discid>_ppsspp.ini`). Cheats start at boot (`__CheatInit`)
   only if it is on; a UI refresh path exists for later changes.
5. **Side effect on boot:** `CWCheatEngine::CreateCheatFile()` creates
   `PSP/Cheats/<discid>.ini` in the real memstick when absent.
6. **Logging:** no log line reports a successful cheat parse.

## Why launch-scoped wiring is not safe today

- **`--appendconfig` writes permanent config.** `Config::LoadAppendedConfig`
  merges the file and then calls `Save("Loaded appended config")`; with
  `bSaveSettings` on that rewrites `ppsspp.ini` and the game's per-game ini
  in the real memstick. So `--appendconfig=<file with EnableCheats=True>`
  would persist the setting, which is exactly the "edit the real config"
  route that is not allowed.
- **Cheat files can only live in the real memstick** (point 3), so staging
  material for one launch means writing into the user's real `Cheats`
  directory or redirecting the whole memstick.
- **Redirecting the memstick moves everything.** A private profile
  (`XDG_CONFIG_HOME`, or `flatpak run --env=...` plus a filesystem grant)
  changes `SAVEDATA`, `PPSSPP_STATE`, `TEXTURES`, `PLUGINS`, the installed
  `GAME` data and `SYSTEM` config. Without extra work the user's saves would
  simply not be there for that session, and new saves would land in the
  scratch profile and be deleted with the workspace.
- **The canonical runtime cannot express the fix.** Keeping saves reachable
  would need the scratch memstick to alias the real `SAVEDATA` and
  `PPSSPP_STATE` (symlinks or bind mounts). The runtime plans only create
  plain files and directories and refuses symlinks in a workspace
  (`SymlinkInWorkspace`), and bind mounts would be a new sandbox mechanism.
  Copying saves in and back is a data-loss risk and is not acceptable.

Verdict: no safe seam. Both reachable routes either persist configuration
(`--appendconfig`) or hide and orphan saves (private memstick).

## Smallest change that would unblock it

Any one of:

- upstream: a per-launch cheat path or file option, or make
  `LoadAppendedConfig` not call `Save` (or add an explicitly transient
  `--set key=value` option);
- upstream: a separate option to relocate only `PSP/Cheats`, leaving the
  memstick in place;
- EmuWiz: a deliberately designed, tested "aliased profile" mechanism that
  mounts the real `SAVEDATA`/`PPSSPP_STATE` into a scratch memstick (for
  example a bwrap/Flatpak bind), including crash and cleanup rules for the
  alias. That is a sandbox design of its own, not an adapter.

## Next step

Do not build an adapter first. If an aliased-profile design is wanted, prove
it with a throwaway Flatpak profile and a legal homebrew boot fixture,
hashing `SAVEDATA`, `PPSSPP_STATE`, `ppsspp.ini` and `Cheats` before and
after, and confirm that PPSSPP opens `Cheats/<discid>.ini` at boot (for
example with `strace`).
