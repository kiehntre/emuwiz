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

## Native dispatch follow-up: pinned release 2.1.1

Continued from `a258afb2852b357b88ad0663d866c1ce42958a05`, without reopening
historical identity work. Native dispatch remains disabled. This follow-up
establishes source contracts, but does **not** certify an installed executable
or supply the complete executable/profile binding needed to enable dispatch.

The official `2.1.1` tag resolves to
`137ae7ce3bf3f539d007e2c4ef3cb3b6c97672a1`, distinct from the development
commit inspected above. Official release metadata records the Linux x64 ZIP
SHA256 as `7a9947575cc198209f743fef83fb2b702b786ea705506bdf3f2aea01ab7c1ce9`.
This identifies the archive, **not** the installed `Mesen` executable or its
loaded native libraries. It must not be used as an executable hash or converted
into a trusted version string for an arbitrary executable discovered on PATH.
The candidate release is not yet an enabled supported Blue launch build.

Source contracts at that exact release:

- [Program.cs](https://github.com/SourMesen/Mesen2/blob/137ae7ce3bf3f539d007e2c4ef3cb3b6c97672a1/UI/Program.cs)
  chooses the profile, loads configuration and extracts dependencies before
  initializing single-instance behavior. There is no early side-effect-free
  version argument in this startup path.
- [CommandLineHelper.cs](https://github.com/SourMesen/Mesen2/blob/137ae7ce3bf3f539d007e2c4ef3cb3b6c97672a1/UI/Utilities/CommandLineHelper.cs)
  accepts an existing absolute media path and `--doNotSaveSettings`;
  [ConfigManager.cs](https://github.com/SourMesen/Mesen2/blob/137ae7ce3bf3f539d007e2c4ef3cb3b6c97672a1/UI/Config/ConfigManager.cs)
  supports the existing boolean safety overrides. These switches are not an
  early profile-selection, version, or single-instance bypass contract.
  Existing files are classified before switches, another reason the child
  working directory and argument interpretation need binding.
- Portable `settings.json` precedes Linux application-data `Mesen2` settings.
  [Configuration.Deserialize](https://github.com/SourMesen/Mesen2/blob/137ae7ce3bf3f539d007e2c4ef3cb3b6c97672a1/UI/Config/Configuration.cs)
  catches typed deserialization failures, attempts a settings backup and falls
  back to defaults. Valid JSON alone does not prove the selected settings took
  effect. The existing compact certified schema is not validation for complete
  saved user profiles, and must not be written over them.
- [SingleInstance.cs](https://github.com/SourMesen/Mesen2/blob/137ae7ce3bf3f539d007e2c4ef3cb3b6c97672a1/UI/Utilities/SingleInstance.cs)
  uses a profile `mesen.lock` for Linux NativeAOT and a global named mutex for
  other builds. Arguments use a fixed named pipe. Configuration determines
  forwarding before generic CLI overrides. A mere absence of a lock filename
  does not prove an instance is absent, and the lock behavior cannot establish
  which build was selected.
- [DependencyHelper.cs](https://github.com/SourMesen/Mesen2/blob/137ae7ce3bf3f539d007e2c4ef3cb3b6c97672a1/UI/Utilities/DependencyHelper.cs)
  extracts embedded dependencies into the profile, retains existing files when
  length and timestamp match, and suppresses extraction failures. Program's
  resolver loads profile `MesenCore.so`, `libSkiaSharp.so` and
  `libHarfBuzzSharp.so`. Thus a trusted executable alone does not bind the
  emulator core that will interpret media and safety settings.

Precise remaining prerequisites: trusted exact executable **and native payload**
identities for a pinned Linux release/acquisition path; validation that the
selected settings will load without fallback or unexpected folder overrides
(a proven strict subset is sufficient; arbitrary typed-schema recreation is
not required); build-specific instance conflict checks and immediate
pre-execution revalidation of that binding together with existing media/BIOS
readiness. None is supplied by the release ZIP digest, current discovery's
caller-supplied version output, or the current metadata-only process checks.
The source contract is available; the required trusted installed-build and
complete-profile contract is not established. Enabling the GUI hook now would
cross those unresolved gates.

No release binaries were downloaded or executed. Source packaging embeds
`Dependencies.zip`, including Satellaview `.bin` data; this task did not fetch
those payloads or treat emulator bundle acquisition as permission to download
firmware. No new launch arguments, profile rewrite, second resolver or
speculative dispatch implementation was added.

Focused existing-binary regressions were rerun with private HOME/XDG/TMP/PATH:
18 core and 2 Blue tests passed. This is a source-only documentation follow-up,
not a new native build or runtime emulator certification. Fresh source,
release metadata, binary hashes, logs and independent review are retained in
`/home/davedap/emuwiz-audit-ledger/2026-10-18-mesen-native-contract/`.
