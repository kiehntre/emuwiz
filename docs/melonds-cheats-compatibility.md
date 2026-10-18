# Blue melonDS native cheats

The Blue Cheats & Mods page reuses the MCH parser/renderer and shared
Preview/confirmation/Apply journals/History/guarded Undo. Opening it performs
no installation. Preview does not write staging files or launch the emulator.

Enabled installation is deliberately limited to:

- Official **1.1 Ubuntu x86-64 native** executable, verified by SHA-256
  `5f80351b63e2a8fd7b7cdf8b61c07761c6240d0c586601b72b788ad6147121ce`.
  The official release ZIP has SHA-256
  `99465129f5413b2aad332e4377e523cf3cda905dc329d47dcb1ad01ce2cb3f66`.
- Existing ordinary `$XDG_CONFIG_HOME/melonDS/melonDS.toml` (otherwise
  `$HOME/.config/melonDS/melonDS.toml`), using Instance0 and an explicit absolute
  `CheatFilePath` pointing to an existing safe directory.
- Direct `.nds` files, with actual bounded SHA-256 matching the exact ROM hash
  declared by the user's local cheat source. This binds bytes, not cheat
  authorship, game applicability, or in-game effectiveness.
- One explicitly selected entry with aligned 8/16/32-bit direct writes to DS
  main RAM. The new file contains that entry as a root code, with the reviewed
  enabled state. Category combinations, opaque/conditional operations, and
  replacement/merging of existing user files are unavailable.

Unknown builds/versions, 1.0 and earlier, AppImage, Flatpak, portable and
DSi and multi-instance profiles are refused. Ambiguous archive-member syntax,
non-Unicode paths and filenames that trigger the firmware-name fallback are
also refused; ordinary UTF-8 and multi-dot filenames retain native mapping. No configuration or ROM is modified.
No cheat database is bundled or downloaded. `usrcheat.dat` import remains an
emulator operation; it is not installed as MCH by EmuWiz.

Upstream 1.1 loads `<CheatFilePath>/<ROM stem>.mch` on ROM load; an empty path
falls back to the ROM directory, but this installer requires an explicit path.
Reload the ROM after installation. `Instance0.EnableCheats` and each code's
native enabled state control activation. Installation never toggles the global
switch or establishes that a cheat works in a game.

Reviewed upstream contracts:

- [1.1 ARCodeFile.cpp](https://github.com/melonDS-emu/melonDS/blob/1.1/src/ARCodeFile.cpp):
  ROOT, CAT flags, CODE, DESC, hexadecimal pairs; 127-byte names and 255-byte descriptions.
- [1.1 EmuInstance.cpp](https://github.com/melonDS-emu/melonDS/blob/1.1/src/frontend/qt_sdl/EmuInstance.cpp):
  getAssetPath, loadROM, loadCheats, EnableCheats and native filename mapping.
- [1.1 main.cpp](https://github.com/melonDS-emu/melonDS/blob/1.1/src/frontend/qt_sdl/main.cpp)
  and [Config.cpp](https://github.com/melonDS-emu/melonDS/blob/1.1/src/frontend/qt_sdl/Config.cpp):
  executable-adjacent portable precedence, Qt ConfigLocation, TOML and Instance0.
- [Official 1.1 release](https://github.com/melonDS-emu/melonDS/releases/tag/1.1)
  and [1.0 parser](https://github.com/melonDS-emu/melonDS/blob/1.0/src/ARCodeFile.cpp):
  release provenance and the older, different format.

Interrupted or uninterpretable pending evidence blocks another installation.
History is authoritative after interruption. Undo requires its own fresh
preview and confirmation; an externally edited file is protected.
