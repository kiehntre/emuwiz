# PPSSPP local cheat installation in Blue

Blue offers a bounded Preview → Confirm → Install → History → guarded Undo path
through the shared transaction engine. This installs **disabled entries**, not enabled
or effective cheats. It never changes PPSSPP settings or launches an emulator.

The supported executable is the official PPSSPP v1.19.3 Linux x86-64 AppImage:
`PPSSPP-v1.19.3-anylinux-x86_64.AppImage`, 69,059,338 bytes,
SHA-256 `540f317c1ed6661894167d2dce21b62295857e1a912274982da7048cf2256466`.
Unknown builds are refused. Choose the executable and existing default native
`$XDG_CONFIG_HOME/ppsspp` profile explicitly (`$HOME/.config` is the XDG fallback).
Flatpak, data-directory and custom layouts are outside this slice. PPSSPP must be
closed. AppImage portable sidecars and runtime overrides are refused.

The selected local game must yield a complete, unambiguous verified PSP Disc ID
from fresh native identity inspection. The existing shared materialized guard's
512 MiB file limit applies to game inspection; larger games are refused, not exempted
from guards. Sources must be local UTF-8 CWCheat INI files within the existing parser
budget, for that exact ID, containing uniquely named supported Write32 entries.
Unsupported directives and operations are refused. Entries render as `_C0` regardless
of their source enable flags. Preview shows the complete paginated output and target.

Only a **new** `PSP/CHEATS/<DiscID>.ini` is installed. Existing files are preserved
and cause refusal. Source path and SHA-256 remain in the durable shared receipt.
Executable, global configuration, game and source hashes are guarded at installation;
changed evidence requires another Preview. No production cheats are downloaded.

History is scoped to this workflow, game, profile and cheat root. Undo requires a
fresh preview and explicit confirmation and preserves files changed since installation.
Global and per-game `EnableCheats`, individual enable flags and Hardcore restrictions
can prevent execution. Installation is never presented as proof of runtime effect.

## Contract evidence

- [Official release](https://github.com/hrydgard/ppsspp/releases/tag/v1.19.3).
- PPSSPP source at commit `e49c0bd8836a8a8f678565357773386f1174d3f5`:
  [`UI/NativeApp.cpp`](https://github.com/hrydgard/ppsspp/blob/e49c0bd8836a8a8f678565357773386f1174d3f5/UI/NativeApp.cpp)
  establishes the Linux default profile;
  [`Core/System.cpp`](https://github.com/hrydgard/ppsspp/blob/e49c0bd8836a8a8f678565357773386f1174d3f5/Core/System.cpp)
  establishes `PSP/CHEATS`;
  [`Core/CwCheat.cpp`](https://github.com/hrydgard/ppsspp/blob/e49c0bd8836a8a8f678565357773386f1174d3f5/Core/CwCheat.cpp)
  binds the exact Disc ID, loads its INI and implements `_C0`/`_C1` and Write32.
- The digest-verified artifact was unpacked without running the emulator. Its AppRun
  is sharun 0.7.2 (source `5cd69df23accc4ccfebc58f550f796578d4783e7`), with
  no bundled `.env`; its uruntime 0.4.3 (source
  `5f1b6d25e82dba4450a94a007677ae15d07949ee`) documents/implements portable
  sidecars and redirection. These packaging contracts are part of the executable
  pin, rather than assumptions about all AppImages.

Synthetic tests exercise installation, receipts, guarded Undo, changed evidence,
existing files, unknown builds, runtime overrides, process conflicts, incompatible
sources, identity conflicts, approval invalidation and small-screen presentation.
No game or emulator was launched to establish runtime effectiveness.
