# EmuWiz Save Vault — Phase 0 research and architecture

> **Recovered historical research — status against current main (`b66422c2`).**
> Source: branch `feature/save-vault-research` at `cdd4cd23`. Recovered unchanged below this block except where marked `[refreshed]`.
> - **Historical design input only.** The current Save Vault implementation is authoritative: immutable local snapshots and single-file restore in [`save_snapshots.rs`](../../crates/archivefs-core/src/save_snapshots.rs), with [`memory_card_inventory.rs`](../../crates/archivefs-core/src/memory_card_inventory.rs), [`persistent_state_inventory.rs`](../../crates/archivefs-core/src/persistent_state_inventory.rs), [`save_state_orchestration.rs`](../../crates/archivefs-core/src/save_state_orchestration.rs) and [`save_migration_planner.rs`](../../crates/archivefs-core/src/save_migration_planner.rs).
> - The private `save_vault.rs` snapshot/journal executor and its GUI that followed this document are **obsolete**; they were not recovered. Mutation now goes through the shared transaction/journal.
> - **Multi-file / directory restore is not on main**: `save_snapshots.rs` still refuses it ("requires atomic set publication"). That work lives on other branches and is not specified by this document.
> - Still useful from here: the pre/post-exit protection design (section 9), privacy boundary (15), restore-safety model (10) and risk matrix (17), and the **pre-launch snapshot policy concept** in section 23: `ASK` as the conservative default, with `OFF` and an "always when a save exists" mode, and snapshot failure modelled as warn-and-allow or block.
> - Sections 1–18 are the v0.9.0-baseline research (`7069cdfc`). Sections 19–23 record the phase decisions (3A restore planning, 3B execution, 3C GUI, 4A pre-launch planning) of the **abandoned private implementation**; treat them as design history, not as behaviour of current main.


Status: research and design only. This document does not implement Save Vault,
does not write save files, does not create backups or restores, does not change
emulator configuration, and does not add GUI or launch integration.

Baseline: EmuWiz v0.9.0, `7069cdfc2b5a4da184e3419964701de591b15e60`.
Evidence reviewed: the v0.9.0 source tree, existing emulator/profile/doctor/
launch documentation, and a read-only inventory of this machine. No network
was used. Machine paths below are metadata-only observations; save contents
were not opened or reported.

## 1. Executive findings

EmuWiz already has useful seams for a future Save Vault: platform/game
identity evidence, read-only profile discovery, installation-kind-aware profile
models, launch planning, watched process execution, and doctor diagnostics.
It does not currently have a save-artifact model, save-path resolver, snapshot
store, or save lifecycle integration.

The safe Phase 1 boundary is therefore read-only discovery only:

1. resolve candidate save locations from a selected, already-discovered
   emulator profile;
2. enumerate regular files and directories without following symlinks;
3. collect size, mtime, type hypothesis, profile provenance, and a hash only
   when the user explicitly requests an inventory scan;
4. classify confidence and ambiguity;
5. show/report findings without copying, writing, locking, renaming, launching,
   changing configuration, or claiming that a shared container belongs to one
   game.

Phase 1 must not create a vault, snapshot, manifest, restore plan, or backup.

## 2. What exists in EmuWiz today

### Current emulator/profile/doctor surface

The release source has a broad launch module and profile-discovery surface. The
following adapters are present in the source tree or launch compatibility map:

| Adapter/family | Current EmuWiz evidence | Save Vault implication |
|---|---|---|
| RetroArch | environment/profile discovery, core/config/system paths, launch command | many cores and multiple save policies; core and content identity are mandatory context |
| PCSX2 | profile discovery, configuration inspection, PS2 serial/CRC identity, launch path | memory cards are commonly shared containers; `Mcd*.ps2` must not be assigned to one title |
| RPCS3 | profile discovery and launch command; PS3 title-ID-shaped content/data is observable | per-title savedata can be scoped; RPCS3 virtual HDD and emulator state are mixed roots |
| PPSSPP | profile discovery and launch command | save directories can be per-title, but profile/install kind and user-selected paths matter |
| DuckStation | profile discovery and launch command | per-game card files and shared cards both occur; inspect configuration before classifying |
| Dolphin | profile discovery, GameCube/Wii ID evidence, launch path, read-only adapter docs | GameCube cards and Wii NAND/save data are container/platform-sensitive |
| Citra/Azahar | Azahar profile/launch support; Citra paths exist in the release-era source and host | title-ID-shaped data is not sufficient without emulator/profile provenance |
| Ryujinx/Ryubing | not a first-class release launch adapter in the same way as the core adapters; host Flatpak/data is present | inventory may observe it only as an external profile/data root until an adapter is reviewed |
| xemu | profile discovery, launch command, Xbox identity support, xemu data roots | EEPROM and virtual HDD are distinct; HDD is a large shared system container |
| FS-UAE | command/execution and Amiga launch compatibility are present | save semantics vary by game/WHDLoad/ADF and user-configured paths; default discovery is weak |
| Hatari | profile/command/execution and Atari launch compatibility are present | NVRAM is machine/profile state, not automatically a per-game save |
| VICE | command/execution/profile work is present, including C64 adapter seams | cartridge/tape disk images and emulator snapshots are not interchangeable with saves |
| ScummVM | detector, command/execution, and directory-based game identity are present | game-internal save directories are likely per-game, but must follow ScummVM config/path evidence |
| DOSBox/DOSBox Staging | DOSBox command/execution and compatibility row are present | saves are often inside the emulated filesystem; host-side discovery is inherently heuristic |
| MAME/FBNeo | MAME/FBNeo command/execution and MAME profile/list-XML evidence are present | NVRAM/EEPROM/SRAM are machine/software-specific and often not safely attributable by filename |
| Other existing adapters | Cemu, DeSmuME, Flycast, MelonDS, mGBA, SameBoy, Mesen, openMSX, RMG, Stella, Xenia, Vita3K, Fuse, XRoar, Amiberry, WHDLoad, Snes9x, and related launch/profile modules are present in varying readiness states | Save Vault must consume reviewed profile capabilities, not infer support merely from a launch module |

“Present” here means source support or inspection surface exists. It does not
mean Save Vault support exists, nor that every adapter has native launch
execution or a verified save-path resolver. The release README itself says
launch support is platform- and emulator-dependent and that some targets only
provide readiness or command planning.

### Existing identity that should be reused

EmuWiz's identity model is layered: format/platform evidence, structural
inspection, verified facts, and DAT/hash authority. Filename and folder names
remain candidates, not verified identity. Existing verified examples include:

- PS2 product serial and reviewed executable CRC;
- PS3 title IDs where the title/data context proves them;
- GameCube/Wii six-byte Game IDs and revision evidence where available;
- platform-specific direct evidence and verified local content hashes;
- DAT/release identity when an existing trusted match is available;
- ScummVM directory identity as an adapter-specific, path-aware identity;
- launch identity: the exact content, profile, adapter, core, and command
  context selected for a launch.

Save Vault should reference this evidence and its provenance, not duplicate or
replace the library identity model.

## 3. Save artifact taxonomy

The proposed `SaveArtifactType` is deliberately typed and may contain an
`UNKNOWN_SAVE_ARTIFACT` fallback. A path must not receive a stronger type just
because its filename looks familiar.

| Type | Meaning | Backup default | Restore/migration caution |
|---|---|---|---|
| `BATTERY_SAVE` | standalone persistent game save associated with a title | include when identity is verified | restore to the same emulator family first |
| `MEMORY_CARD` | card/container holding one or many title records | include as a container | never present as per-game-only if shared; restore whole container or use a verified record-level tool later |
| `SAVE_STATE` | emulator snapshot of running emulation | exclude by default; opt-in | same emulator/core/version/content identity only |
| `EMULATOR_PROFILE_SAVE` | emulator-managed profile/user save area | include only with explicit adapter classification | may include unrelated settings or identities |
| `GAME_INTERNAL_SAVE` | files inside an emulated filesystem or game directory | include when path and title identity are verified | path structure and permissions are part of restore |
| `NVRAM` | non-volatile machine/software state | include with machine/software identity | shared or machine-wide; do not call it a game save without evidence |
| `EEPROM` | small persistent hardware/config memory | include when adapter identifies it | often console-wide or title-specific depending on emulator |
| `SRAM` | battery-backed cartridge RAM | include when content identity is verified | content-relative association is stronger than filename |
| `FLASH` | flash memory image, including cartridge/console persistent storage | include with platform/content context | can be a shared or system container |
| `RTC` | real-time-clock state | opt-in with its owning artifact | restoring time state can have gameplay effects; keep separate |
| `CONFIG_DEPENDENT_SAVE` | save whose interpretation/location depends on emulator config/profile | include only with config provenance | keep config reference separate and restore only after review |
| `UNKNOWN_SAVE_ARTIFACT` | candidate cannot be safely classified | inventory only | `REVIEW_REQUIRED`; no automatic backup/restore |

Emulator configuration is not a save artifact. A restore may record a
configuration dependency or required profile fingerprint, but Save Vault must
not silently restore emulator configuration as a side effect.

## 4. Path discovery contract

Save locations must be resolved by an adapter-specific, evidence-producing
resolver. The resolver returns zero or more `SaveLocation` values, not one
hard-coded path.

| Discovery source | Confidence | Rule |
|---|---|---|
| `VERIFIED_FROM_CONFIG` | highest | parse the selected profile/config using a bounded, read-only parser and retain the exact source path/key |
| `KNOWN_DEFAULT` | high but conditional | use only when the emulator version/install kind and platform convention match; retain the assumption |
| `PROFILE_DERIVED` | medium-high | derive from the exact discovered profile, sandbox root, portable root, or selected data root |
| `HEURISTIC` | low | report as candidate only; never auto-backup or restore |
| `UNKNOWN` | none | show that discovery is unavailable; do not scan broad home directories |

The resolver must preserve installation kind (`Native`, `Flatpak`, portable,
AppImage, explicit), profile identity, config path, sandbox root, user-selected
overrides, and whether a path is content-relative, per-game, or shared.
Arbitrary emulator settings must never be replaced by assumptions. If a config
allows a user-selected save directory, the config value outranks a default.

Initial adapter research conclusions:

| Emulator | Candidate location strategy | Phase 0 confidence |
|---|---|---|
| RetroArch | selected profile's savefile/savestate/system directories, core-specific subdirectories, Flatpak sandbox equivalent, and explicit overrides | `PROFILE_DERIVED`; often `VERIFIED_FROM_CONFIG` only after core/config parsing |
| PCSX2 | selected PCSX2 profile/config plus `memcards` and any configured memory-card paths | `PROFILE_DERIVED`; card ownership is not implied |
| RPCS3 | selected data root, `dev_hdd0/home/<user>/savedata`, and `savestates/<title>` when the profile confirms them | `PROFILE_DERIVED`; broad RPCS3 root is mixed data |
| PPSSPP | profile/config save and state directories, including Flatpak sandbox path | `PROFILE_DERIVED`; per-title folder identity still needs verification |
| DuckStation | profile/config memory-card, save, and state paths; inspect card-sharing mode | `PROFILE_DERIVED` or `VERIFIED_FROM_CONFIG` |
| Dolphin | profile-derived GC card paths, Wii NAND/save paths, and configured state paths | `PROFILE_DERIVED`; GC card likely shared |
| Citra/Azahar | profile-derived SD/NAND/title-save roots and selected portable/sandbox root | `PROFILE_DERIVED`; title ID is a candidate until adapter verification |
| Ryujinx/Ryubing | only after an explicit profile adapter supplies configured user/profile roots | `UNKNOWN` in v0.9.0 Save Vault terms |
| xemu | profile-derived `xemu` data root; distinguish EEPROM from virtual HDD and backups | `PROFILE_DERIVED` |
| FS-UAE | explicit config, hardfile/ADF/WHDLoad game path, and user-selected save path | `UNKNOWN`/`HEURISTIC` unless config names it |
| Hatari | profile/config NVRAM path and machine profile | `VERIFIED_FROM_CONFIG` when config is explicit; not necessarily game-owned |
| VICE | profile/config snapshot, cartridge/tape/disk paths, and emulator-specific state | `UNKNOWN` for generic “save” discovery |
| ScummVM | ScummVM config savepath plus game directory identity | `VERIFIED_FROM_CONFIG` when `savepath` is read; otherwise `PROFILE_DERIVED` |
| DOSBox | explicit DOSBox config, mounted host directories, and game-internal files | `VERIFIED_FROM_CONFIG` only for declared mounts; otherwise `HEURISTIC` |
| MAME/FBNeo | configured home/save/nvram paths plus software-list/shortname identity | `PROFILE_DERIVED`; per-software classification requires adapter evidence |

The table is a research boundary, not a promise that these paths are already
parsed by EmuWiz. A future implementation should add capability records per
adapter rather than a global extension list.

## 5. Game and platform identity

`SaveIdentity` should be a structured, confidence-ranked association:

```text
SaveIdentity {
  platform_id,
  game_key,                 // existing verified game/release identity if any
  identity_facts[],         // serial, title ID, disc ID, Game ID, DAT/hash...
  content_hash?,
  launch_identity?,        // exact content/profile/core/adapter context
  evidence_provenance[],
  confidence,
}
```

Preferred evidence order is:

1. verified existing EmuWiz identity for the exact content/release;
2. emulator-native title/serial identity tied to the discovered artifact;
3. verified content hash and platform identity;
4. explicit user association recorded as a reviewable fact;
5. filename/folder heuristic only as an unresolved candidate.

Filename alone is never enough where a serial, title ID, Game ID, DAT/release
identity, verified content hash, or launch identity exists. A snapshot can be
valid without a known game identity, but it must then be a profile/container
snapshot with `UNKNOWN` or `REVIEW_REQUIRED` status rather than silently
appearing in a game's history.

## 6. Memory-card and shared-container model

The core distinction is:

- `PER_GAME_SAVE`: the artifact is independently attributable to one game by
  verified path, format, and identity evidence;
- `SHARED_MEMORY_CONTAINER`: one artifact may contain records for multiple
  games or a machine-wide state, and its ownership is a set or unknown.

`MEMORY_CARD` therefore has a container identity independent of any contained
game records:

```text
MemoryContainer {
  container_id,              // stable vault identity, not filename
  emulator_family,
  platform,
  original_path,
  container_format?,
  owning_profile,
  sharing: SHARED | PER_GAME | UNKNOWN,
  member_records?: [
    { game_identity?, slot?, record_hash?, parser_confidence }
  ],
}
```

Research findings:

- PS1 cards can be shared across many titles. A `.mcr`/`.mcd`-like file is a
  whole-card artifact; record-level association requires a format-aware parser.
- PS2 `Mcd001.ps2`/`Mcd002.ps2` are card containers and commonly shared. The
  card must be backed up as a whole; a title association is a member-level
  fact, not container ownership.
- GameCube cards are shared card images with many possible title records.
  Dolphin Game ID is useful for identifying a member, not for claiming the
  entire card belongs to that game.
- Dreamcast VMU data may be represented as shared container/VMU artifacts; the
  same “container first, member second” model applies.
- Wii/PS3/Xbox virtual storage and NAND-like trees are shared machine/system
  containers unless an adapter proves a per-title subtree.

Default safety: snapshot a shared container atomically as one artifact, record
all known or unknown member scope, and never offer a destructive per-game
restore against it. A future record-level restore needs a reviewed parser,
transactional rewrite, pre-restore container snapshot, and explicit preview.

## 7. Snapshot and vault model

Proposed typed structures (names are design names, not Rust additions in this
phase):

```text
SaveArtifact
SaveArtifactType
SaveLocation
SaveIdentity
SaveSnapshot
SaveSnapshotManifest
SaveVault
SaveBackupPlan
SaveRestorePlan
SaveConflict
SaveSafety
SaveProvenance
```

Minimum shape:

```text
SaveArtifact {
  artifact_id,
  artifact_type,
  location: SaveLocation,
  identity: SaveIdentity,
  shared_container_status,
  file_size,
  modified_time,
  sha256?,
  format_signature?,
  corruption_observations[],
}

SaveLocation {
  original_path,
  discovery_confidence,
  discovery_source,
  emulator_profile_id,
  config_path?,
  path_scope: PER_GAME | SHARED_CONTAINER | PROFILE | UNKNOWN,
}

SaveSnapshot {
  snapshot_id,
  artifact_id,
  emulator,
  emulator_version?,
  core?,
  platform,
  game_identity?,
  original_path,
  file_size,
  modified_time,
  sha256,
  artifact_type,
  snapshot_time,
  provenance,
  shared_container_status,
  trigger: MANUAL | PRE_LAUNCH | POST_EXIT | IMPORTED,
  safety_status,
}
```

`SaveProvenance` must include the exact profile/config evidence, resolver
version, source path identity where available, observation timestamp, and
whether the source was stable during scanning. It should not contain save
content or private metadata beyond what is needed to identify the artifact.

`SaveSafety` is a classification, not permission to mutate:

- `SAFE_TO_BACKUP`: stable regular source, destination available, identity and
  type adequate for the requested operation;
- `REVIEW_REQUIRED`: ambiguity, shared scope, unstable source, weak identity,
  or unsupported format;
- `BLOCKED`: missing source, symlink/path safety failure, active emulator,
  failed verification, or destination conflict.

### Content-addressed storage

The recommended future local layout is:

```text
vault/
  objects/
    ab/cd/<full-sha256>
  snapshots/
    <snapshot-id>/manifest.json
```

The object is immutable and addressed by SHA-256. Identical bytes deduplicate.
Manifests remain human-readable and contain original paths, identity, type,
timestamps, provenance, and safety facts. Restore selects a verified object
through a manifest and destination plan; it never relies solely on a filename.

Phase 2A proves this layout: objects are staged and hash-verified before an
atomic rename, while manifests are written to `manifest.json.staging`, synced,
and atomically renamed to `manifest.json`. A manifest is therefore the
completion marker; a staging-only directory is incomplete. Object identity is
the full lowercase SHA-256 hex digest, with the first four hex characters used
as two directory levels. Existing objects are re-hash-verified before reuse.
Snapshot creation reads each selected regular file with bounded before/after
size and modification-time checks, retrying once if it changes. Selection is
explicit and source paths are never modified. Phase 2A supports multiple
selected files as deterministic safe relative entries, but does not implement
restore, retention, or latest-pointer mutation.

## 8. History semantics

`SnapshotId` is a unique immutable event identity. The content hash is not the
snapshot identity: the same bytes observed at two meaningful times may be two
history events, while storage deduplicates their object bytes.

History is grouped by the strongest available identity scope:

```text
verified game identity
  -> emulator/profile/core compatibility scope
    -> artifact/container identity
      -> ordered snapshot events
```

Each event records `MANUAL`, `PRE_LAUNCH`, `POST_EXIT`, or `IMPORTED`. A
`latest` pointer is a derived index and may move forward only after verified
object/manifest completion. A `pinned` flag prevents future pruning. Manual
snapshots are retained by default; automatic retention is a future policy and
must not prune in Phase 0.

Deduplication rules:

- identical content uses one immutable object;
- a new observation can still create a new event if its trigger/time/provenance
  matters;
- an unchanged post-exit observation may be coalesced by policy, but the
  decision must remain visible;
- shared containers deduplicate by container bytes, never by inferred game
  member name.

## 9. Pre-launch and post-exit integration design

### Pre-launch

Future optional sequence:

```text
launch request
  -> resolve exact profile/content/identity
  -> discover candidate save artifacts
  -> verify sources are stable and emulator is not already using them
  -> snapshot current state if the plan requires it
  -> launch existing command
```

Safe default: pre-launch backup is opt-in per profile/adapter and `WARN` is
preferred over blocking for an optional backup, provided the user explicitly
accepted that policy. A user may configure `BLOCK` for high-safety workflows.
If required backup fails under `BLOCK`, do not launch. If a source is active,
changing, ambiguous, or shared and no safe whole-container snapshot can be
made, fail closed for that backup; never pretend it succeeded.

No launch code is changed in this phase.

### Post-exit

The future coordinator should use the watched process exit event where
available, then rediscover the same profile/path set and compare size, mtime,
and hash. Hash comparison is authoritative for byte changes; mtime is a cheap
candidate signal only. It should handle:

- clean process exit: snapshot changed artifacts;
- emulator crash: mark the event `CRASH_EXIT`, snapshot stable changed files if
  possible, and retain the warning;
- process still running or child process active: do not scan as final;
- shared card changed: snapshot the whole container and mark all game members
  potentially affected;
- no byte change: record no new object, optionally retain an observation event.

There is no daemon or watcher implementation here.

## 10. Restore safety model

Restore is a plan with explicit states:

```text
PREVIEW
  -> VERIFY DESTINATION
  -> SNAPSHOT CURRENT DESTINATION
  -> RESTORE EXACT OBJECT
  -> VERIFY RESTORED HASH/shape
  -> COMPLETE
```

The destination must be re-resolved immediately before mutation and compared
with the previewed path/profile/container identity. The emulator must not be
running against the destination. The current destination is preserved first;
there is no overwrite-in-place shortcut. A failed post-write verification
must leave the pre-restore snapshot available and report `REVIEW_REQUIRED` or
`BLOCKED`, not claim success.

Classification:

- `SAFE_TO_RESTORE`: exact destination, compatible emulator/profile scope,
  verified object hash, no active process, and an automatic pre-restore
  snapshot completed;
- `REVIEW_REQUIRED`: shared container, weak identity, format uncertainty,
  emulator-version mismatch, user override, or converter involved;
- `BLOCKED`: destination missing/changed, active emulator, object/hash failure,
  symlink/path safety failure, incompatible artifact, or pre-restore snapshot
  failure.

Restore must never depend solely on a filename and must never silently restore
emulator configuration.

## 11. Cross-emulator migration findings

Migration is a compatibility claim over bytes, format, identity, and emulator
version—not merely a copy operation.

| Route | Classification | Boundary |
|---|---|---|
| DuckStation ↔ RetroArch PS1 normal battery saves | `UNKNOWN` until exact format/core path is verified; potentially `BYTE_COMPATIBLE` for a matching raw format | require verified content identity, same save format, and destination preview |
| PCSX2 cards to another PCSX2 profile/version | `BYTE_COMPATIBLE` at whole-card level when the card format/version is accepted | shared card remains shared; do not extract one game automatically |
| PCSX2 card ↔ unrelated PS2 core/container | `UNKNOWN` or `INCOMPATIBLE` without a reviewed format contract | no promise from platform equality alone |
| Dolphin standalone ↔ RetroArch Dolphin | `UNKNOWN`/potentially `BYTE_COMPATIBLE` only for the same underlying card/save format and compatible core | profile paths and shared GC/Wii containers require explicit verification |
| PPSSPP save directories between installs | often `BYTE_COMPATIBLE` when same title ID and directory layout are preserved | version/profile/config dependencies must be recorded |
| RPCS3 savedata between RPCS3 installs | potentially `BYTE_COMPATIBLE` within the same title ID/user scope | encryption, user/profile, and virtual-HDD context can block migration |
| xemu EEPROM/HDD between xemu installs | `BYTE_COMPATIBLE` only as a complete compatible machine scope | never treat virtual HDD as a per-game save |
| MAME/FBNeo NVRAM/SRAM across cores | `UNKNOWN` by default; sometimes `CONVERTIBLE` only with a reviewed software/format adapter | shortname/platform match is not enough |
| save states across any emulator/core/version | `INCOMPATIBLE` by default; same exact emulator/core/version may be `BYTE_COMPATIBLE` | restore only under strict captured launch identity |

No conversion is promised in Phase 0. A future `CONVERTIBLE` route requires a
reviewed converter, source/destination format versions, fixture coverage,
preview, output hash, and rollback path.

## 12. Save states

Save states are more fragile than ordinary persistent saves. They commonly
depend on emulator version, core version, core options, renderer/runtime,
content bytes, BIOS, memory layout, and sometimes host architecture. A state
file can hash cleanly and still be unusable.

Recommendation:

- discover and back up save states only as opt-in `SAVE_STATE` artifacts;
- exclude them from default automatic backup plans;
- restore only when emulator family, exact core, emulator version policy,
  content identity/hash, relevant profile fingerprint, and state format are
  compatible;
- mark a version-upgrade mismatch `REVIEW_REQUIRED`, never auto-migrate;
- preserve the original state object even if a restore attempt fails.

## 13. Corruption and anomaly detection

Safe Phase 0 observations:

- zero-byte file detection;
- file size and mtime capture;
- SHA-256 hash when explicitly requested and the file remains stable;
- sudden size drop compared with a previous observation;
- read failure, permission failure, symlink, and disappearing-file detection;
- known magic/signature checks only where a bounded, reviewed parser exists;
- container parser validation for a future adapter, without repair.

No repair, truncation, normalization, timestamp change, or “fix” is allowed.
Hash is an integrity observation, not proof that a save is semantically valid.

## 14. Destinations: local first, NAS later

Use a future destination abstraction:

```text
LOCAL_VAULT
NAS
REMOVABLE_STORAGE
```

Cloud is deferred. Each destination needs availability, capability, free-space,
path-safety, and atomic-write semantics. A NAS/removable plan must tolerate an
offline destination, interrupted copies, reconnects, and partial objects:

1. write a temporary object;
2. close and hash it;
3. compare with the source/object hash;
4. atomically rename the object;
5. atomically write the manifest last;
6. treat a manifest without a verified object as incomplete.

The local vault is the canonical Phase 2 target. No remote provider, upload,
telemetry, or network synchronization is part of this design or repository
change.

## 15. Privacy and security boundary

Save Vault is local-first and has no telemetry, upload, or remote provider.
Save metadata may contain usernames, timestamps, profile identifiers, title
IDs, paths, and emulator version information. Manifests should minimize this
data, make path disclosure explicit, and avoid reading game-save content for
display.

The vault should be treated as sensitive local user data. Future export or NAS
support must state exactly which paths and metadata leave the source machine;
cloud support remains future work. Hashes can still be identifying metadata and
must not be sent anywhere implicitly.

## 16. Read-only inventory of this machine

The following is an approximate metadata inventory from 2026-09-13. Counts
include regular files under the named root and may include non-save files when
the root is a mixed emulator data tree. Sizes are decimal bytes rounded here
for readability. No file contents were dumped.

| Emulator/data family | Discovered root | Artifact observation | Approx. files | Approx. total |
|---|---|---|---:|---:|
| RetroDECK | `~/retrodeck/saves` | mixed saves; visible `.srm` cartridge saves, Amiga save/support files, one small Xbox artifact | 79 | 2.23 MB |
| RetroDECK | `~/retrodeck/states` | state root present; no meaningful regular state file found in the direct per-platform scan | 1 | 132 B |
| RetroArch | `~/.config/retroarch/saves` | save root | 118 | 81.95 KB |
| RetroArch | `~/.config/retroarch/states` | state root | 1 | 1.50 MB |
| PCSX2 | `~/.config/PCSX2` | `memcards/Mcd001.ps2`, `memcards/Mcd002.ps2`; mixed config/log/cache files | 25 | 67.53 MB |
| RPCS3 | `~/.config/rpcs3` | mixed profile/data tree; title-ID savedata and one `.SAVESTAT.zst` observed | 16,698 | 76.60 GB |
| Citra | `~/.config/citra-emu` | mixed title-ID-shaped/config tree; no save classification asserted | 644 | 1.70 MB |
| Azahar | `~/.config/azahar-emu` | mixed config/title-ID-shaped tree; no save classification asserted | 4 | 36.03 KB |
| Ryujinx | `~/.config/Ryujinx` | config/profile/key/log files; no save artifact asserted | 13 | 148.25 KB |
| Dolphin | `~/.config/dolphin-emu` + `~/.local/share/dolphin-emu` + `~/.local/share/dolphin` | profile/config and support data; no card/save file asserted from names alone | 25 | 33.55 KB |
| ScummVM | `~/.config/scummvm` + `~/.local/share/scummvm` | config/savepath-related roots; no content classification asserted | 4 | 24.64 KB |
| Hatari | `~/.hatari` | `hatari.nvram` | 1 | 50 B |
| FS-UAE | `~/Documents/FS-UAE` + `~/.local/share/fs-uae` | profile/data roots; no save file confidently isolated | 1 | 5.48 KB |
| xemu | `~/.local/share/xemu` | `eeprom.bin`, `xbox_hdd.qcow2`, backup/corrupt-backup artifacts; mixed runtime data | 125 | 1.19 GB |
| Flatpak DuckStation | `~/.var/app/org.duckstation.DuckStation` | installation root present, no regular files in bounded scan | 0 | 0 B |
| Flatpak Dolphin | `~/.var/app/org.DolphinEmu.dolphin-emu` | mixed sandbox data | 390 | 286.43 MB |
| Flatpak RPCS3 | `~/.var/app/net.rpcs3.RPCS3` | mixed sandbox data | 35,067 | 8.41 GB |
| Flatpak PCSX2 | `~/.var/app/net.pcsx2.PCSX2` | mixed sandbox data | 4,733 | 24.89 MB |
| Flatpak PPSSPP | `~/.var/app/org.ppsspp.PPSSPP` | profile/data root, no save classification asserted | 5 | 65.40 KB |
| Flatpak xemu | `~/.var/app/app.xemu.xemu` | mixed sandbox data including large machine artifacts | 47 | 1.19 GB |
| Flatpak Ryubing | `~/.var/app/io.github.ryubing.Ryujinx` | mixed sandbox data | 6,899 | 382.50 MB |
| Flatpak Azahar | `~/.var/app/org.azahar_emu.Azahar` | mixed sandbox data | 42 | 549.89 KB |

Installed commands observed read-only: `/usr/bin/retroarch`, `/usr/bin/dolphin`,
`/usr/bin/fs-uae`, and `/usr/games/scummvm`. No native command was found for
PCSX2, RPCS3, PPSSPP, DuckStation, Citra/Azahar, Ryujinx/Ryubing, xemu, Hatari,
VICE, DOSBox, or MAME in the checked command path. This does not negate the
Flatpak, portable, or explicit profile roots above.

The inventory deliberately does not report filenames containing personal
game/user names, save contents, usernames, title lists, or file hashes.

## 17. Risk matrix

| Artifact | Backup safety | Restore safety | Migration safety | Identity confidence required |
|---|---|---|---|---|
| ordinary per-game save | high after stable read/hash | high with exact destination and pre-restore snapshot | medium; same format/emulator preferred | verified game/content identity |
| per-game memory card | medium-high as whole file | medium; verify card format and scope | medium within same emulator family | verified title plus card ownership evidence |
| shared memory card | high as whole-container snapshot | medium-low; whole-card restore only by default | low-medium | verified container/profile; game member identity is optional but never exclusive |
| save state | medium only opt-in | low; exact version/core/content required | very low | exact launch identity and compatibility fingerprint |
| emulator NVRAM | medium | medium-low; machine/profile scope | low | verified machine/software scope |
| config-coupled save | medium with provenance | low-medium; config is separate and reviewable | low/unknown | verified config path plus game identity |

General defaults: backup is safer than restore; restore is safer than
migration; unknown/shared/state artifacts remain reviewable or excluded from
automatic plans.

## 18. Recommended implementation phases

1. **Phase 1 — read-only save discovery.** Adapter capability records,
   profile/config-derived locations, non-following enumeration, typed
   hypotheses, confidence, metadata, optional explicit hash scan, and a report.
   No vault writes and no GUI required initially; a CLI/report seam is enough.
2. **Phase 2 — manual snapshots.** Content-addressed local objects and
   human-readable manifests, with explicit user action and immutable source
   reads. Add shared-container and state opt-in gates.
3. **Phase 3 — safe restore.** Preview, destination re-resolution, active
   process check, automatic current-destination snapshot, exact object/hash
   verification, and fail-closed status.
4. **Phase 4 — pre-launch snapshot.** Optional integration with launch
   planning, configurable WARN/BLOCK policy, and no mutation of launch command
   semantics.
5. **Phase 5 — post-exit snapshot.** Use watched process exit, change
   detection, crash provenance, shared-container handling, and idempotent
   automatic history events.
6. **Phase 6 — migration, retention, and destinations.** Reviewed converters,
   pinned/latest/pruning policy, NAS/removable destination transactions, and
   compatibility tests. Cloud remains a separate future decision.

### Exact Phase 1 boundary

Phase 1 may read:

- selected EmuWiz emulator/profile records and their config files;
- known profile-derived save roots and explicit user-configured paths;
- directory entries and regular-file metadata under those bounded roots;
- file bytes only for an explicitly requested SHA-256 scan or a bounded,
  reviewed format-signature check.

Phase 1 must not:

- copy, write, rename, delete, restore, back up, or repair any save;
- create `vault/`, objects, manifests, indexes, or retention state;
- write emulator configuration or change profile paths;
- launch, stop, attach to, or alter an emulator process;
- infer exclusive game ownership for shared containers;
- scan the whole home directory or follow symlinks;
- call a network, NAS, cloud, telemetry, or remote-provider service;
- modify GUI, navigation, library, publishing, or publisher-profile code.

## 19. Phase 3A restore-planning decision

Phase 3A adds planning only. A restore request names a verified snapshot, an
explicit trusted current destination root, current emulator/profile identity,
and whether the emulator is active. The planner validates the complete
manifest and every referenced object before inspecting destinations. It
classifies each deterministic relative entry as `CREATE_NEW`,
`ALREADY_MATCHES`, or `REPLACE_EXISTING`; a replacement remains eligible only
because execution must create and verify a `REQUIRES_PRE_RESTORE_SNAPSHOT`
before any publish.

Destination inspection rejects symlinks and special files, checks bounded
content hashes, rejects unsafe relative paths and case-fold collisions, and
never uses manifest absolute source paths as write targets. Shared containers
remain shared and force review; unknown game identity and save-state
compatibility do not become safe automatically. An active emulator blocks the
plan. A deterministic plan hash binds snapshot, request, and inspected
destination state for future stale-plan enforcement. Phase 3A performs no
restore, pre-restore snapshot, overwrite, or emulator-directory write.

Phase 3A.1 adds optional PS3 evidence without changing ordinary RPCS3-native
semantics. Manifest schema version 2 adds optional PS3 representation,
`PARAM.SFO`/`PARAM.PFD` observations, binding, protection, and provenance
fields; schema version 1 remains loadable with those fields unknown. Bounded
SFO inspection extracts title identifiers and presentation fields only. PFD
presence is recorded as unverified; no resigning, encryption, checksum repair,
or secret handling exists.

RPCS3-native to RPCS3-native planning does not require hardware account/PSID or
PFD evidence. Native PS3 targets require proven representation, binding, and
protection integrity; account mismatch, required resigning, unsupported secure
formats, and invalid/unverified native integrity are typed blocking or
unsupported reasons. USB/native/cross-representation plans remain review-only
or blocked, and raw console identifiers are never stored in ordinary
manifests.

## 20. Validation and scope confirmation

For this Phase 0 change, validation is documentation-only plus `git diff
--check`. The worktree must contain exactly the research document as the
tracked change. The machine inspection was read-only; no save files or emulator
configs were changed. No network was used.

Publisher Profiles, `publisher_profile/*`, Library Organisation, Playing
Library, RomM publishing, ES-DE publishing, main navigation, publisher GUI,
publisher documentation, and current publisher-profile files are outside this
worktree change and remain untouched.

## 21. Phase 3B execution decision

Phase 3B adds a core-only, byte-preserving execution engine. It accepts only
plans that are `SAFE_TO_RESTORE` or `ALREADY_MATCHES`; shared-container,
save-state, PS3, active-emulator, identity, and path-safety review states
remain non-executable. A plan that replaces differing destination bytes is
safe only through the mandatory sequence of final stale-plan recheck,
verified pre-restore snapshot, staged object reads, atomic rename where the
filesystem supports it, and post-write hash verification.

The vault records transactions beneath `restore-journal/` using atomic JSON
publication. Staged destination content is kept below a controlled temporary
directory and source snapshot objects are never modified. Journals distinguish
planned, pre-snapshot-complete, staged, partially-published, completed,
failed, rollback-complete, and needs-reconciliation states. Rollback removes
transaction-created files only when their bytes still match the transaction;
replacements are restored from the verified pre-restore snapshot and both
paths are verified. Uncertain rollback or interrupted publication is never
reported as successful.

Phase 3B does not expose GUI execution, restore buttons, deletion, retention,
PS3 resigning, encryption/decryption, checksum repair, migration, or remote
storage. Restore execution tests use disposable synthetic files only.

## 22. Phase 3C GUI restore boundary

Phase 3C extends the existing `Tools & Workflows → Save Vault` page with one
continuous workflow: verified snapshot history → restore preview → execution
review → typed `RESTORE N ITEMS` confirmation → Phase 3B execution → verified
result → typed `ROLL BACK RESTORE` confirmation. The GUI does not implement
filesystem writes, staging, verification, journaling, or rollback; it calls
the existing `execute_restore` and `rollback_restore` APIs only.

Restore controls are available only for `SAFE_TO_RESTORE` plans with at least
one executable item. `ALREADY_MATCHES` is presented as a no-op, while review,
blocked, unsupported, active-emulator, corrupt-snapshot, PS3 binding/protection,
and representation cases remain fail-closed with no override. Execution repeats
the core final stale-plan preflight immediately after confirmation. Replacement
plans show the verified pre-restore safety snapshot stage and expose its
history identifier on success. Recent shared restore journals are shown in the
same page; no second history subsystem was added.

The GUI smoke uses disposable synthetic state/execution fixtures. Existing
display-based application startup is available where the environment permits;
full mouse click automation is an environment-dependent validation item.
Automatic backup, migration, resigning, cross-representation conversion,
metadata/configuration changes, retention, and remote destinations remain out
of scope.

## 23. Phase 4A pre-launch protection planning boundary

Phase 4A adds a pure `save_vault_pre_launch` planner and a read-only
`Save Vault → Pre-launch Save Protection` panel. It reuses the existing
`LaunchPlan` game/platform/candidate identity and bounded Save Vault discovery;
it does not create a second launch or save-discovery system. Ordinary saves are
selected only with exact game identity. Shared containers may be selected only
with matching platform and emulator/profile evidence and are labelled as
whole-container scope because they may contain saves for multiple games.

The initial model is conservative: `ASK` is the default policy, with `OFF` and
`ALWAYS_WHEN_SAVE_EXISTS` represented but not executed. Save states remain
separate and excluded from the ordinary-save scope by default. The plan records
selection decisions, warnings, latest verified snapshot awareness where the
existing history is available, and a deterministic plan hash. Snapshot failure
is modelled as `WARN_AND_ALLOW_LAUNCH` or `BLOCK_LAUNCH`; neither changes launch
behaviour in this phase. No snapshot, directory, configuration, process, or
network mutation occurs.
