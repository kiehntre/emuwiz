# Mesen NES/FDS native launch status

Bounded implementation from origin/main `7f3d6d13ccc0132154844a9c461fff5ce6bd23be`.
The historical NES/FDS investigation was not repeated.

Blue connects the existing NES platform/content identity and launch planner to
read-only Mesen profile and FDS BIOS inspection. Inspection is asynchronous,
selection-bound and bounded. Every Mesen candidate remains blocked; the native
launch action is disabled and explains the unresolved executable/settings contract.
Recognized BIOS bytes do not authorize a launch or establish game runnability.

## Established upstream contract

Primary source inspected at SourMesen/Mesen2 commit
`b9fa69ddc6d0a331fb103fdb5eef6904305703c2`:

- [ConfigManager.cs](https://github.com/SourMesen/Mesen2/blob/b9fa69ddc6d0a331fb103fdb5eef6904305703c2/UI/Config/ConfigManager.cs):
  portable `settings.json` beside the executable takes precedence over the
  Linux application-data `Mesen2` profile. Arbitrary explicit profile roots
  are not automatically honored.
- [FirmwareTypeExtensions.cs](https://github.com/SourMesen/Mesen2/blob/b9fa69ddc6d0a331fb103fdb5eef6904305703c2/UI/Interop/FirmwareTypeExtensions.cs),
  [FirmwareHelper.h](https://github.com/SourMesen/Mesen2/blob/b9fa69ddc6d0a331fb103fdb5eef6904305703c2/Core/Shared/FirmwareHelper.h):
  FDS requires 8,192 BIOS bytes; inspection compares SHA256 against upstream's
  two accepted identities. `Firmware/disksys.rom` takes precedence over
  `Firmware/FdsBios.bin`. A same-sized unrecognized primary shadows the alternate.
  EmuWiz conservatively refuses unsafe/wrong-size primaries instead of falling
  through. Filenames and sizes alone never establish verification.
- [Fds.cpp](https://github.com/SourMesen/Mesen2/blob/b9fa69ddc6d0a331fb103fdb5eef6904305703c2/Core/NES/Mappers/FDS/Fds.cpp):
  `Nes.OverwriteOriginalRom=true` writes the original image. The command builder
  explicitly disables it and disables automatic sibling patches, alongside
  `--doNotSaveSettings`.
- [Program.cs](https://github.com/SourMesen/Mesen2/blob/b9fa69ddc6d0a331fb103fdb5eef6904305703c2/UI/Program.cs):
  startup selects the profile, loads configuration and initializes single-instance
  forwarding before generic CLI overrides. There is no safe early version probe
  in this contract. A probe can create/extract configuration/dependency files.

## Precise remaining blocker

Automatic discovery establishes a regular executable named `Mesen`, not its
supported Mesen 2 generation. It supplies no trusted version evidence.
Furthermore, syntactically valid JSON does not establish that Mesen's typed
configuration loader will accept it. For example, a `Nes` property containing
`42` can trigger fallback to defaults even when `Preferences.SingleInstance`
is explicitly false. Defaults can forward to a different running profile and
its unchecked BIOS directory. A late CLI override cannot fix that binding.

The core preflight therefore requires supplied Mesen 2 version evidence and
only accepts the currently certified compact settings schema:
`{"Preferences":{"SingleInstance":false}}`. This is a narrow backend contract,
not a recommendation to replace a user's settings. Rich production profiles
are refused without modifying them. Blue deliberately does not manufacture
version evidence or expose this backend as an executable native launch.

A future native integration needs an approved way to bind a supported executable
and its complete typed settings/instance behavior, using existing installation
selection infrastructure. A filename, a generic executable bit, JSON syntax or
BIOS presence cannot substitute for that contract. No executable probing,
firmware downloads, profile rewriting or game-media rewriting is implemented.

## Validation boundaries

Focused tests use temporary synthetic NES/FDS images, fake executable files that
are never run, and unrecognized BIOS bytes. They exercise structural and exact
identity gates, missing/unrecognized firmware, supported-version refusal,
configuration ambiguity, portable precedence and symlink refusal. No real BIOS
payload is embedded; no real emulator/game was launched. No production library,
database, ROM, BIOS, save, profile or running Blue installation was changed.
