# Save / Savestate Portability Audit

> **Recovered historical research — status against current main (`b66422c2`).**
> Source: branch `research/save-state-portability` at `c73c9526`. Recovered unchanged below this block except where marked `[refreshed]`.
> - **Research only; no save code was changed.** Written against main `41c9a645` (2026-09-21).
> - **Key claims, still the policy:** there is no universal save conversion and no cross-emulator conversion service; savestates are emulator/version/configuration-sensitive and are never silently copied across emulators or versions; restores require identity binding and refuse ambiguous account, firmware, region or profile matches; refusal wording should say why.
> - **Superseded statement:** the section "Existing EmuWiz capability" says there is no provider-neutral save-state compatibility model or restore planner. Current main has [`save_migration_planner.rs`](../../crates/archivefs-core/src/save_migration_planner.rs), [`save_state_orchestration.rs`](../../crates/archivefs-core/src/save_state_orchestration.rs), [`persistent_state_inventory.rs`](../../crates/archivefs-core/src/persistent_state_inventory.rs) and [`save_snapshots.rs`](../../crates/archivefs-core/src/save_snapshots.rs) (single-file restore with undo; multi-file restore refused). What `save_migration_planner` covers exactly was not re-audited here.


Status: research only. No save, configuration, ROM, or emulator data was modified.

Authoritative source inspected: `/home/davedap/emuwiz-main-release-fix` at
`41c9a645464a3fec9fa68cad7de78cfadd19cf12` (2026-09-21).

## Executive conclusion

EmuWiz should treat a game-native save as potentially portable only after its
format, game identity, emulator family, profile/account, and destination path
are known. A savestate is a different object: it is normally a serialized
emulator machine, core, firmware, and game execution state. It must therefore
remain emulator/version/configuration-bound unless a specific format contract
proves otherwise.

The safest future model is:

* preserve the original source tree;
* create an immutable, metadata-bearing backup before any restore;
* preview and confirm restores;
* bind a save to exact title/serial/disc identity where available;
* refuse ambiguous account, firmware, region, or profile matches;
* never silently copy a savestate when changing emulator or emulator version;
* report “update availability unknown” offline rather than inferring it from a
  missing network check.

There is no evidence in the current core of a universal save migration or
cross-emulator conversion service. The existing Save Vault-like implementation
is a focused PS1/PS2 memory-card inventory and PS2 PSU/file export/restore
workflow. That narrow scope is appropriate and should not be generalized by
copying opaque emulator state files.

## Existing EmuWiz capability

The current source contains the following relevant pieces:

* `memory_card_inventory.rs` inventories readable PS1/PS2 card images and
  validates PS2 geometry, directory entries, save files, and health.
* PS2 regular-file export is create-only and leaves the source card unchanged.
* PS2 PSU export is create-only and validates a complete save directory before
  writing the package.
* PS2 PSU restore requires a healthy destination card, previews the operation,
  creates a caller-selected backup before mutation, verifies the written card,
  and exposes an undo operation using that backup.
* The PCSX2 page labels this area “Save Vault · Memory Card Contents” and
  explicitly says that restore previews, backs up, and re-checks the card.
* `SourceRole::MemoryCards` and `SourceSubsystemRoute::MemoryCardInventory`
  expose memory-card sources to the existing source model.
* launch integration exposes writable memory-card and save-state paths for
  profiles, but launch does not own backup, restore, or migration.
* RetroArch environment discovery exposes configured save and savestate
  directories.
* readiness has a `save_state_version_sensitive` field, but there is not yet a
  provider-neutral save-state compatibility model or startup restore planner.

The existing Save Vault scope is therefore PS1/PS2 memory-card inspection plus
PS2 file/PSU operations, not a general backup of every emulator's user data.

## State taxonomy

| Type | Meaning | Default portability rule |
|---|---|---|
| `NativeSave` | A game-created save in the game's documented format | Potentially portable within the same game/platform and compatible emulator family; require identity evidence |
| `MemoryCard` | A container holding one or more native saves | Copy the whole validated card or use a proven export format; preserve slot, geometry, region, and card identity |
| `SaveState` | A serialized running emulator/machine snapshot | Emulator-, core-, version-, machine-, firmware-, and often configuration-bound |
| `NandOrVirtualDisk` | Persistent virtual console/handheld/PC storage | Treat as a container with system/account state, not as an individual save; copy only with a complete manifest |
| `ConfigBoundState` | Config, per-game settings, remaps, shaders, caches, or paths that affect interpretation | Not a save; copy with explicit review and version/path metadata |
| `CloudManaged` | State whose authoritative copy is an external service | Do not claim a local backup is authoritative; require provider/account semantics |
| `Unknown` | Any opaque or undocumented persistent file | `DoNotTouch` until a format contract exists |

## Backup classes

| Class | Use when | Required metadata |
|---|---|---|
| `SafeToCopy` | Documented game-native bytes with exact identity and no account binding | emulator family, game identity, source path, timestamp, checksum |
| `CopyWithMetadata` | Portable bytes are plausible but container/slot/profile context matters | all of the above plus card/container format, slot, region, profile |
| `VersionBound` | Format is portable but emulator/core/firmware version can change interpretation | emulator and version, core/driver, firmware/TOS/BIOS, configuration fingerprint |
| `EmulatorBound` | Savestate or emulator-private serialized state | exact executable/core/version, machine profile, firmware, game identity, config fingerprint |
| `NeedsReview` | Account, region, title ID, path, or conversion assumptions are unresolved | complete provenance plus the unresolved condition |
| `DoNotTouch` | Unknown, cloud-authoritative, destructive container, or identity mismatch | record refusal reason; do not copy or overwrite |

## Identity binding

Evidence should be evaluated in this order:

1. exact platform title ID, serial, disc ID, or cartridge/software ID;
2. internal save metadata and region/profile identifiers;
3. validated emulator/container metadata, including card geometry and slot;
4. content hash or known-good image identity for the game that owns the save;
5. filename, folder name, or fuzzy title matching only as a review hint.

A file hash proves the bytes of the backup, not that the bytes belong to the
selected game. A path or filename alone is never sufficient. A restore should
fail closed if exact identity, account/profile, or destination container
preconditions cannot be established.

## Emulator matrix

The paths below are representative Linux/XDG locations, not promises. Each
adapter must first honor its configured datapath, portable mode, Flatpak
sandbox, and explicit user paths. `~/.var/app/<ID>/` is the usual Flatpak
boundary, but the application ID must be verified for the installed build.

| Emulator | State type / typical location | Portable? / version-bound? | Identity and profile binding | Backup class | Restore risk / notes |
|---|---|---|---|---|---|
| RetroArch | `NativeSave`: configured `savefile_directory`, commonly `~/.config/retroarch/saves`; `SaveState`: configured `savestate_directory`, commonly `~/.config/retroarch/states`; core/content overrides and remaps beside the config | Native saves can move between compatible cores only with proof; states are core-, version-, content-, and config-bound | Core plus content path/CRC/playlist identity; save format is core-dependent | Native `CopyWithMetadata`; states `EmulatorBound`; overrides `NeedsReview` | Standalone core and RetroArch state formats are not interchangeable by filename; preserve core and content identity |
| DuckStation | Memory cards and states under the DuckStation profile/data directory, commonly XDG data/config locations or the configured profile; Flatpak sandbox applies | Cards are generally more portable within DuckStation-compatible versions; states remain version/config-bound | Disc serial/region, card slot, profile; per-game card selection matters | Cards `CopyWithMetadata`; states `EmulatorBound` | Do not replace a per-game card with a global card without preview; verify serial and region |
| PCSX2 | `.ps2` memory cards under the configured datapath, commonly `~/.config/PCSX2/memcards`; states and per-game settings under that datapath; Flatpak uses its sandbox | PS2 card images are portable within compatible PCSX2-family formats; states are strongly version/machine/BIOS bound | Disc serial, region, card slot, BIOS/profile; PSU is a game-save interchange package, not a complete card | Card `CopyWithMetadata`; PSU `SafeToCopy` after validation; states `EmulatorBound` | Existing EmuWiz PS2 inventory/PSU flow is the proven narrow restore path; do not restore opaque states |
| RPCS3 | Native saves under the RPCS3 data tree, typically `dev_hdd0/home/00000001/savedata`; config and caches are separate | Saves may move between RPCS3 installs only with title, user, account, and version checks; no generic portable savestate contract should be assumed | PS3 title ID and user/account linkage; some saves contain console/account cryptographic context | `CopyWithMetadata` or `NeedsReview`; unknown states `DoNotTouch` | Never silently merge `dev_hdd0`; preserve user/profile context and treat a full virtual HDD as a system container |
| PPSSPP | Native `PSP/SAVEDATA` on the configured memstick; states in the PPSSPP profile/state area; Linux and Flatpak use different data roots | PSP save folders are commonly portable across PPSSPP platforms/versions when title/region matches; states remain version-bound | PSP title ID, region, memstick/profile; save folder names are meaningful but not enough alone | Native `SafeToCopy`/`CopyWithMetadata`; states `EmulatorBound` | Copying `PSP/SAVEDATA` is not equivalent to copying `PSP/PPSSPP_STATE`; keep them separate |
| Dolphin | GameCube raw memory cards or GCI-folder cards under the Dolphin user directory; Wii NAND/title saves under the configured NAND; states under state-save directories | GCI can be converted to/from a raw card with slot/region evidence; Wii NAND is a system container; states are version/machine bound | Game ID, region, card slot, Wii title ID, user/NAND context | GCI/card `CopyWithMetadata`; NAND `NeedsReview`; states `EmulatorBound` | A raw card is shared container state; do not overwrite it to move one game without a card-level backup |
| Flycast | VMU save files, flash/NVRAM, config, and states in the configured Flycast data directory, commonly XDG data/config paths | VMU formats may be portable among compatible Dreamcast emulators; flash/NVRAM and states are emulator/system bound | Disc/game identity, VMU slot, Dreamcast region/system context | VMU `CopyWithMetadata`; flash/NVRAM `VersionBound`; states `EmulatorBound` | VMU, flash, BIOS, and state files are different state classes; do not bundle them as one save |
| MAME | `nvram`, `cfg`, `sta`, input, and memory-card trees controlled by `-nvram_directory`, `-cfg_directory`, `-state_directory`, `-homepath`, and related options | NVRAM can be usable only for the same machine/software/driver and compatible MAME version; `.sta` is version/machine bound | Exact MAME system/ROM/software-set identity and driver version | NVRAM `VersionBound`; states `EmulatorBound`; cfg `NeedsReview` | MAME paths contain controls and machine state as well as saves; never infer game identity from a directory alone |
| Hatari | Writable disk images/save disks and Hatari configuration/NVRAM/state files in configured XDG or profile paths | A writable disk image can be copied as a complete image; snapshots are Hatari/machine/TOS/version bound | Disk image identity, machine model, TOS version, boot configuration | Save disk `CopyWithMetadata`; snapshots `EmulatorBound` | Atari save disks are media, not arbitrary extracted native saves; preserve write-enabled image bytes |
| FS-UAE | Save disks/HDFs, configuration, Kickstart/firmware references, and states under the FS-UAE base directory or explicit `--base-dir` | Disk/HDF state can move with the complete compatible setup; states are version/machine/Kickstart bound | Amiga game/disk set, HDF geometry, Kickstart, hardware profile | Disk/HDF `VersionBound`; states `EmulatorBound` | A path-only copy can omit Kickstart, config, or companion disks; require a manifest |
| xemu | Xbox virtual HDD, EEPROM/flash, and emulator data under the configured XDG/Flatpak data root; game saves live inside the virtual HDD | Whole HDD/EEPROM is emulator/system bound; no generic per-game save extraction should be assumed | Xbox title plus virtual console/account/system context | `NandOrVirtualDisk` / `NeedsReview` | Do not copy or replace the HDD as though it were a normal save file; preserve the complete image and firmware context |
| Xenia | User/content data and emulator configuration are platform/version dependent; current upstream is primarily Windows and there is no stable Linux save-path contract to automate | Unknown for a Linux EmuWiz portability workflow; state files should be treated as emulator-bound | Xbox title/user/profile context, where present | `NeedsReview` or `DoNotTouch` | Report configured paths only; do not invent Linux lifecycle or restore semantics from Windows layouts |
| Cemu | Title saves in the configured `mlc01/usr/save/00050000/...` tree; portable mode may place `mlc01` beside the executable; profile/config/keys are separate | Native title saves can move between Cemu installations only with title/profile/version checks; no general cross-emulator state path | Wii U title ID, account/profile, region, MLC/config context | `CopyWithMetadata` or `NeedsReview`; states `DoNotTouch` unless documented | `mlc01` is a system/user container; replacing it can affect installed titles, accounts, updates, and saves together |
| Vita3K | User/title save data lives inside the Vita3K user filesystem; config/data/cache commonly use XDG or the configured portable directory | Portable only within compatible Vita3K versions and matching title/user/firmware context; no cross-emulator contract | Vita title ID, user profile, installed title and firmware/filesystem context | `NeedsReview` / `VersionBound` | Copy the documented save subtree only after identifying it; do not replace the whole Vita3K filesystem for one title |

No row above should be interpreted as permission to copy defaults blindly. The
adapter must resolve the effective configured path and record it in any future
manifest.

## Emulator-specific observations

### Native saves versus savestates

Native saves generally describe game progress in a format intended to be read
by the game. They can still depend on title ID, region, account, card slot,
firmware, or emulator-specific container details.

Savestates instead capture a live machine. Even where a state file is a single
portable-looking blob, it can contain serialized RAM, device state, emulator
version assumptions, core/driver state, timing state, firmware/TOS state, and
host-dependent data. A change of emulator build, core, machine model, BIOS/TOS,
renderer, or game revision can invalidate it. EmuWiz should warn before an
emulator update when existing readiness evidence marks states as sensitive and
should never silently restore a state after an emulator switch.

### Container state

RPCS3 `dev_hdd0`, Dolphin Wii NAND, Cemu `mlc01`, and xemu virtual HDDs are
containers. They may include installed content, accounts, system databases,
firmware metadata, and multiple games. A container backup is not evidence that
one selected game's save can safely be extracted or merged. The future UI
should label these as persistent system storage and offer whole-container
backup only with explicit scope and size confirmation.

### Disk-based systems

Hatari and FS-UAE commonly persist progress by writing to a disk image, save
disk, or HDF. The image itself is the save-bearing medium. Source bytes should
remain untouched; a future backup should copy the complete image and record
machine, firmware/Kickstart/TOS, geometry, and mounted disk set.

### MAME

MAME separates NVRAM, configuration, and save-state concepts. NVRAM is closer
to native machine memory than to a universally portable game save. `.sta` is a
machine snapshot and should be treated as `EmulatorBound`. `cfg` and input
files are user configuration, not progress. The exact software/ROM set and
MAME driver version are required for meaningful restore checks.

## Cross-emulator opportunities

| Migration | Classification | Conditions |
|---|---|---|
| PS1 raw memory card between compatible emulators | `KnownPortable` in the common raw-card format | Validate card size/format, preserve slot and title serial/region, and back up the destination card first |
| DuckStation ↔ a RetroArch PS1 core memory card | `Convertible` / `NeedsReview` | Core-specific card format and configured card path must match; do not infer compatibility from the frontend name |
| PS2 memory-card image between compatible PCSX2-family implementations | `KnownPortable` with format validation | Preserve card geometry and use the existing PS2 health checks; BIOS/game identity still matters |
| PS2 PSU export/import | `KnownPortable` / `Convertible` | Existing EmuWiz code validates a complete save and verifies restore; it is not a general card-image converter |
| PSP `PSP/SAVEDATA` between PPSSPP installations | `KnownPortable` when title/region/profile match | Copy native save folders only; do not include PPSSPP savestates |
| Dolphin GCI folder ↔ raw GameCube card | `Convertible` | Card region, slot, free space, and collision behavior require a preview and destination backup |
| Dreamcast VMU among compatible emulators | `Convertible` / `NeedsReview` | Validate VMU format and slot; flash/NVRAM and states are separate |
| PS3/RPCS3, Wii U/Cemu, Vita3K, Xbox/xemu containers | `EmulatorSpecific` | Account, title, firmware, and system-container context prevent blanket migration |
| Any opaque savestate across different emulators | `Unknown` / `DoNotTouch` | Require an explicit documented format contract; filename or extension is insufficient |

Architecture and endianness must be treated as format questions, not blanket
rules. A documented raw card format may be independent of host architecture,
while an opaque savestate can embed host- or emulator-dependent state. A byte
for byte backup preserves data but does not make an unsupported conversion safe.

## Restore policy for a future Save Vault

Every restore should produce a read-only plan containing:

* source and destination paths;
* source checksum and captured timestamp;
* emulator, executable/core version, and profile;
* exact game identity and region evidence;
* account/user/firmware/TOS/BIOS context where relevant;
* state type and backup class;
* destination pre-state checksum and backup location;
* explicit overwrite, merge, conversion, or refusal decision.

The operation should preview first, require confirmation for overwrite or
conversion, back up the destination before mutation, verify the result, and
retain an undo path. It must refuse when identity, account, profile, container
geometry, or version preconditions are ambiguous. There should be no silent
cross-emulator restore and no automatic savestate migration.

## Save Vault gap analysis

Current strengths:

* validated PS1/PS2 card inspection;
* PS2 regular-file and complete-save PSU export;
* create-only export behavior;
* explicit PS2 restore preview;
* destination backup before restore;
* post-write verification and undo for the PS2 PSU path.

Current gaps for a generalized vault:

* no provider-neutral manifest for all state classes;
* no common identity record covering title ID, serial, disc hash, region,
  profile, account, firmware, and emulator version;
* no generic save-versus-savestate portability classification;
* no adapter registry that reports effective configured save roots and sandbox
  roots consistently;
* no general snapshot catalog, retention policy, or multi-path restore plan;
* no cross-emulator converter framework;
* no universal startup warning for stale or interrupted save operations;
* no evidence that opaque NAND/HDD/VMU/state blobs can be safely merged by
  game without adapter-specific rules.

What can be generalized later is metadata, planning, checksums, destination
backup, verification, and refusal/reporting. Format parsing, PS2 PSU handling,
card geometry, Dolphin GCI conversion, and any future container/account logic
should remain platform-specific adapters.

## Release and lifecycle warnings

Future lifecycle/update UX should warn when:

* an emulator update can invalidate existing savestates;
* changing a RetroArch core, standalone emulator, or emulator profile changes
  the effective save/state root;
* switching native, Flatpak, portable, or AppImage installations changes the
  sandbox or data path;
* a BIOS/TOS/Kickstart/firmware change changes machine compatibility;
* a system container or account/profile is being replaced rather than one
  native save;
* a memory card is shared by multiple games and a restore will affect all of
  them;
* a destination already contains newer or unrelated data.

Offline operation must still report configured paths, installed version,
available saves, and readiness. Update availability should be `Unknown` when
the provider cannot be queried.

Suggested future user-facing labels are: `Native save`, `Memory card`,
`Savestate — emulator/version bound`, `System storage`, `Backup with metadata`,
`Needs review`, and `Do not restore automatically`.

## Sources and evidence

Project evidence:

* [EmuWiz memory-card inventory and PS2 export/restore](../../crates/archivefs-core/src/memory_card_inventory.rs)
* [EmuWiz PCSX2 Save Vault UI](../../crates/archivefs-gui/src/pcsx2_page.rs)
* [EmuWiz emulator file-isolation boundary](EMULATOR_FILE_ISOLATION_ARCHITECTURE_AUDIT.md)
* [EmuWiz launch recipe boundary](LAUNCH_RECIPE_ARCHITECTURE_AUDIT.md)
* [EmuWiz readiness boundary](READY_TO_PLAY_ARCHITECTURE_AUDIT.md)
* [PS2 memory-card format audit](PS2_MEMORY_CARD_FORMAT_AUDIT.md)
* [PS2 PSU interoperability validation](PS2_PSU_INTEROPERABILITY_VALIDATION.md)

Primary project documentation to keep current when implementing this later:

* [RetroArch configuration documentation](https://docs.libretro.com/guides/retroarch-configuration/)
* [PCSX2 documentation](https://pcsx2.net/docs/)
* [DuckStation documentation](https://github.com/stenzek/duckstation/wiki)
* [RPCS3 wiki](https://wiki.rpcs3.net/)
* [PPSSPP documentation](https://www.ppsspp.org/docs/)
* [Dolphin wiki save management](https://wiki.dolphin-emu.org/index.php?title=Save_Management)
* [Flycast project](https://github.com/flyinghead/flycast)
* [MAME documentation](https://docs.mamedev.org/)
* [Hatari documentation](https://github.com/hatari/hatari/tree/main/doc)
* [FS-UAE paths and directories](https://fs-uae.net/docs/paths-and-directories/)
* [xemu documentation](https://xemu.app/docs/)
* [Xenia project](https://github.com/xenia-project/xenia)
* [Cemu documentation](https://github.com/cemu-project/Cemu)
* [Vita3K project](https://vita3k.org/)

These upstream links describe project behavior and formats; they do not turn a
default path into a guaranteed path for every Linux packaging channel.

## Recommended implementation order

1. Add a read-only per-adapter state inventory with effective paths and
   emulator/profile/version provenance.
2. Generalize the existing PS2 plan/backup/verify metadata into a provider-
   neutral manifest without changing PS2 semantics.
3. Add native-save adapters only where exact identity evidence is available:
   PS1/PS2 cards, PSP save folders, and Dolphin card/GCI paths are the strongest
   candidates.
4. Add explicit savestate inventory and warnings, but keep restore disabled by
   default across versions and emulator families.
5. Handle system containers only as whole-container, metadata-bearing backups.
6. Add startup/update warnings and visible refusal reasons before considering
   any conversion or automatic restore.

This is a research result, not an implementation authorization.
