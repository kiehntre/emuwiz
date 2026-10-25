# Emulator File Isolation Architecture Audit

## Executive summary

Least-privilege file exposure should be a core EmuWiz launch principle. A
launch plan should authorize an emulator to see only the resolved content,
firmware, configuration, and writable state required by that one launch. UI
visibility and emulator access are different concerns: a dependency can be
hidden from the library view and still be granted to a launch, while a visible
game should normally expose only its selected media path.

EmuWiz already has the correct authority boundaries to build on. `LaunchPlan`
is the candidate and readiness authority; media topology, firmware evidence,
source roles, and emulator profiles already provide upstream facts; command
builders produce typed argument vectors; execution performs fresh
revalidation. The missing architectural layer is a typed, adapter-facing
resource grant and a lifecycle-owned temporary projection. It must not become
a second planner or a generic command interpreter.

The recommended first implementation is deliberately narrow:

1. add a vocabulary-only `LaunchResourceGrant`/access-policy model;
2. add a read-only launch-access receipt and debug projection;
3. implement a reusable scratch-copy primitive for one adapter/media class
   whose emulator is not proven safe to write to directly;
4. pilot narrow RetroArch system/content/save paths, with explicit fallback
   when the core cannot be isolated;
5. defer mount namespaces, broad MAME closure projections, and large writable
   disk images until focused feasibility tests exist.

The default projection policy should be: direct read-only reference only when
the adapter has a documented and tested read-only guarantee; otherwise use a
verified, private scratch copy for media that may be written; isolate writable
state in an owned per-launch directory; never hardlink writable scratch; never
silently widen a strict grant to a broad directory.

## Current architecture

### Existing authorities

The local architecture audit and source inspection identify these boundaries:

| Existing authority | Current role | Isolation implication |
|---|---|---|
| `launch::planning::LaunchPlan` | Candidate selection, platform/game identity, readiness and launch target | Remains the sole launch authority; isolation must consume it, not replace it |
| `LaunchCandidate` / `LaunchTarget` | Emulator/profile/install candidate and typed target | Supplies the adapter and exact content role |
| `launch::evidence_bridge` | Converts already-verified identity/content evidence | Must remain the identity boundary; no projection may rediscover identity |
| `launch::topology` and media-set projection | Ordered discs, companions and launch entry | Supplies a resolved media set; isolation must not regroup or reorder media |
| firmware/BIOS evidence and emulator profiles | Read-only discovery and readiness facts | Supplies selected firmware/profile resources, not whole warehouses |
| `PreparedProcessCommand` and `process_spawn` | Typed executable, argv, working directory and watched process | Receives projected paths; must not accept arbitrary shell text |
| adapter command modules | Emulator-specific flags and path semantics | Retain exact flags in adapter code; share only resource vocabulary |
| Save Vault | Snapshot/restore ownership for saves | Does not become a launch sandbox or implicit copy-back policy |
| Publisher / Storage Health | Library projections and analysis | Never become emulator access authorities |

The existing launch module explicitly describes planning as data-only and
execution as the point that revalidates mutable facts immediately before
spawn. The local `SAFE_LAUNCH_SANDBOX_V1.md` research found no production
scratch-media implementation; its scratch-copy design is therefore a useful
research input, not an existing executor. The same document records the
existing `CapturedFileIdentity` stat-based freshness primitive and the
watched-process contract that a future isolation layer should reuse.

### Where access is currently broad

The present adapter shape generally carries an exact game path in a typed
launch request, but does not yet carry a complete access receipt. Examples
include:

- RetroArch receives content/core/profile information from the discovered
  RetroArch environment and command projection. Its environment model already
  identifies system, savefile, savestate, cache and core directories, but a
  launch-specific grant is not yet a standard contract.
- MAME has a configured `rompath`; its normal dependency resolution is based
  on set, parent, BIOS and device search semantics. A configured broad
  `rompath` can therefore expose much more than one selected set.
- native PCSX2, Dolphin, DuckStation and RPCS3 requests carry exact content
  paths but their ordinary profile/config/state roots are emulator-specific
  and may remain globally writable unless an adapter explicitly narrows them.
- xemu has a disc path/config path model while writable HDD and emulator state
  are materially different from immutable firmware.

This is an architectural observation, not a claim that every current launch
mutates source media. The safe default must account for undocumented writes,
save-on-exit behavior, caches, “last used” files, and emulator bugs.

### Existing design material

`LAUNCH_RECIPE_ARCHITECTURE_AUDIT.md` recommends a declarative future
`LaunchContract` and `ResolvedLaunchRecipe`, while keeping `LaunchPlan` as the
sole candidate/readiness authority. `SAFE_LAUNCH_SANDBOX_V1.md` recommends a
private runtime workspace, source identity capture before and after copying,
plain copies as the correctness baseline, and fail-closed launch if scratch
preparation fails. Those conclusions are adopted here; this document does
not propose another planner, recipe language, or scanner.

## Resource classes

The following classes are access-policy subjects, not new discovery stores.
Each grant should identify the existing evidence/reference that justified it.

| Resource class | Typical examples | Default mutability | Default exposure |
|---|---|---|---|
| `GAME_MEDIA` | ROM, ISO, CHD, CUE/BIN, GDI/XISO | Immutable source | Exact file or resolved topology projection, read-only |
| `FIRMWARE` | Console firmware, platform firmware | Immutable source | Only selected files or an adapter-required narrow tree |
| `BIOS` | PS1 BIOS, MAME BIOS set, Neo Geo BIOS | Immutable source | Exact required file/set closure, read-only |
| `DEPENDENCY_ROM` | Parent/shared ROM dependency | Immutable source | Exact closure member, read-only |
| `DEVICE_ROM` | MAME device ROM such as qsound | Immutable source | Exact device closure member, read-only |
| `CONFIG` | Emulator config, overrides | Usually persistent state | Read-only reference or isolated copy; never broad by default |
| `PROFILE` | Emulator user/profile directory | Mixed | Per-launch profile projection or explicitly declared profile root |
| `SAVE_DATA` | Battery saves, save directories | Persistent user state | One selected writable path or isolated state binding |
| `MEMORY_CARD` | PCSX2/PCSX memory-card image | Persistent user state | Explicit selected image; scratch copy if direct writes are not safe |
| `NVRAM` | Arcade NVRAM, xemu EEPROM | Persistent user state | Separate per-profile/per-title state path; never immutable master |
| `NAND` | Dolphin/xemu NAND | Persistent user state | Explicit writable profile root or copy-on-write/scratch strategy |
| `HDD_IMAGE` | xemu virtual HDD | Persistent user state | Explicit writable image; snapshot/scratch policy required |
| `DAT_METADATA` | DAT/XML/hash definitions | Immutable tooling data | No emulator access unless a specific adapter proves it needs it |
| `ARTWORK` | Box art, screenshots, media metadata | Immutable library metadata | No emulator access by default |
| `CACHE` | Shader, texture, compiled-core cache | Disposable or rebuildable | Private per-emulator/per-launch cache where feasible |
| `TEMPORARY_RUNTIME` | Projection root, generated config, launch log | Temporary | Private runtime root only |
| `UNKNOWN` | Unclassified path or dependency | Unknown | No access; fail closed until classified |

The important distinction is between *source role* and *access mode*. A
`BIOS_FIRMWARE` source role says what a file is in EmuWiz's library; it does
not grant an emulator access to that source folder. A save source role does
not grant access to all saves.

## Access policy model

The minimum conceptual model is:

```text
LaunchResourceGrant {
    launch_id
    resource_id / source_evidence_ref
    resource_class
    role
    access_mode
    projection_method
    projected_path
    source_identity
    lifetime
    cleanup_policy
    provenance
}
```

### Resource roles

Roles should describe why a path is present, for example:

- `PRIMARY_MEDIA`
- `SECONDARY_MEDIA`
- `BIOS_FILE`
- `FIRMWARE_FILE`
- `PARENT_ROM`
- `DEVICE_ROM`
- `CHD_DISK`
- `READ_ONLY_PROFILE_REFERENCE`
- `WRITABLE_SAVE`
- `MEMORY_CARD`
- `NVRAM`
- `NAND`
- `HDD_IMAGE`
- `TEMP_CONFIG`
- `RUNTIME_CACHE`

The role is explanatory and policy-bearing; it must not replace media topology
or emulator-specific dependency models.

### Access modes

Use explicit modes:

| Mode | Meaning | Typical use |
|---|---|---|
| `READ_ONLY` | Emulator may open/read but not write through the granted view | ROM, BIOS, firmware, DAT reference |
| `READ_WRITE` | Emulator may mutate the projected resource | Per-launch save directory, owned writable profile |
| `CREATE_ONLY` | Emulator may create new entries in an empty owned directory | New cache or save directory |
| `NO_ACCESS` | Explicitly denied/not projected | Whole BIOS warehouse, unrelated platform, artwork |

`READ_ONLY` is an intended boundary, not a chmod promise. A read-only file
permission does not prevent an emulator with other access from reopening the
source by path. The stronger guarantee comes from the process filesystem view
or from passing only a scratch copy. The grant must record which guarantee was
actually achieved.

### Projection methods

| Method | Recommended use | Risk/limitation |
|---|---|---|
| `DIRECT_PATH` | Proven read-only media or exact path where the adapter contract accepts direct access | Emulator can potentially write source; no isolation by itself |
| `SYMLINK_FILE` | Read-only reference where the emulator opens but never writes and source escape is controlled | Writes follow to source; not suitable for writable resources |
| `SYMLINK_DIRECTORY` | Only when directory semantics are required and the entire directory is approved | High overexposure; descendants and future files become visible |
| `READ_ONLY_BIND` | Stronger view isolation on supported Linux environment | Requires namespace/mount capability and careful TOCTOU handling |
| `BIND_MOUNT` | Narrow host path inside a private namespace | Not portable; ordinary bind is not automatically read-only |
| `TEMP_COPY` | Media or config that may be written, with source preservation | Uses storage/time; lifecycle and crash cleanup required |
| `REFLINK` | Future optimization only after filesystem-specific proof | Copy-on-write guarantees vary; not a correctness baseline |
| `SCRATCH_COPY` | Mandatory protection for writable/unknown media semantics | Must never be replaced with a hardlink or source symlink |
| `GENERATED_TEMP_DIR` | Empty private config/cache/state root | Must be owned, bounded and explicitly classified |
| `CONFIG_OVERRIDE` | Adapter-supported per-launch settings | Must be typed/allowlisted, never arbitrary text or shell |
| `NO_PROJECTION` | Resource not required or not safely exposable | Fail closed if the emulator requires it |

### Lifetime

Use `LAUNCH_ONLY`, `SESSION`, `PERSISTENT`, and `UNTIL_CLEANUP` explicitly.
Immutable source references are not owned by a launch and must never be
deleted as cleanup. Temporary roots belong to one launch transaction and must
have an owner marker and bounded cleanup policy. Persistent saves belong to
the selected emulator/profile and require separate Save Vault interaction.

## Minimal file exposure

The desired flow is:

```text
selected game
  -> existing LaunchPlan and adapter requirements
  -> exact resource/dependency closure
  -> typed resource grants
  -> direct references or private projections
  -> fresh preflight and source identity recheck
  -> typed argv / sandboxed process
  -> watched session
  -> verify, retain persistent state, clean temporary resources
```

The process should not receive the master BIOS root, the complete save tree,
the complete configuration directory, the complete ROM library, DAT files, or
artwork unless a reviewed adapter contract demonstrates that the emulator
requires that breadth. Such breadth is a named fallback, not an invisible
implementation convenience.

## Direct paths, symlinks and copies

### Direct references

Use `DIRECT_PATH` only when all of the following are true:

1. the launch plan identifies the exact path;
2. the adapter declares the resource read-only or has a proven read-only mode;
3. the path is within the approved source identity and is revalidated before
   spawn;
4. the emulator cannot use the same path to discover an unapproved directory;
5. no config or runtime behavior causes write-back beside the content.

This is efficient and avoids copying multi-gigabyte images, but it is not a
general preservation guarantee.

### File-level symlinks

File-level symlinks are preferable to directory-level symlinks only for
strictly read-only resources and only when the emulator requires a filesystem
name rather than a direct path. The target must be canonicalized/revalidated,
the link must be created beneath an owned projection root, and the grant must
record the exact target. A link is not a write barrier: an emulator opening the
link for writing changes the target.

Never use a source symlink for writable media, saves, memory cards, NVRAM,
NAND, or HDD images. Never use a directory symlink to expose a whole BIOS,
save, config, or ROM tree when a file-level grant or direct path is sufficient.

### Scratch copies

For an emulator without a proven read-only mode, the correctness-first method
is a private physical copy. The source is opened/read-only; its existing
`CapturedFileIdentity` is recorded before copying and checked after copying;
the scratch copy is passed to the emulator; mutations stay in the scratch
workspace and are discarded unless the resource was explicitly declared
persistent state. A hardlink is never an acceptable scratch copy because it
shares the inode with the source. A reflink is a later optimization, not the
baseline guarantee.

For large media, the planner should estimate required space and refuse if the
resource policy cannot guarantee enough space. It must not silently use a
direct source path when copying fails.

## Symlink risks and path confinement

The threat model includes malicious or merely unusual filenames, symlinked
ancestors, mutable sources, emulator deletion, and a source path being
replaced between validation and launch.

Required rules for any projection:

- synthesize projection names from bounded internal identifiers, not raw user
  filenames where possible;
- reject absolute/traversal components in generated relative paths;
- keep the projection root owner-only and mark it as EmuWiz-owned;
- verify the canonical source is the expected source identity;
- do not follow destination symlinks during cleanup or replacement;
- never remove a path outside the owned projection root;
- revalidate the destination parent immediately before creation;
- record whether a path was pre-existing or created by this launch;
- treat a broken link, unexpected file, or changed target as a conflict;
- do not clean while the emulator process or child process may still hold the
  projection.

For stronger protection, a future executor may use open-file-descriptor and
`openat`/`openat2`-style confinement where available. That is a platform
implementation detail, not something a path string alone can guarantee.

## Bind mounts and namespaces

Linux mount namespaces can give a process a private view whose mount changes do
not normally affect the parent namespace. `mount_namespaces(7)` documents that
the new namespace starts from a copy of the mount list and that propagation
must be made private to avoid unwanted mount events. A bind mount exposes the
same underlying content at another path; it is not automatically read-only.
The Linux mount documentation also notes that the classic read-only bind flow
uses a bind followed by a remount, and that this is not atomic. It documents a
TOCTOU hazard when path components are writable.

Bubblewrap is a practical user-space wrapper for this direction. Its manual
supports read-only binds, separate writable binds, user/mount/PID/network
namespace options, and an option to disable further user namespaces. It still
requires careful policy construction and environment-specific capability
checks.

### Recommendation

Bind mounts/namespaces are worth using as an optional hardening layer, not as
the first universal implementation. They are Linux-specific, can fail under
restricted kernels, interact with Flatpak's own sandbox, and increase cleanup
and diagnostics complexity. The first guarantee should come from exact typed
grants plus scratch copies for writable-risk media. A strict namespace mode can
then strengthen the same grant without changing its semantics.

The fallback must be explicit:

1. `STRICT_MINIMAL`: private namespace or exact file projections;
2. `NARROW_DIRECTORY`: approved adapter-required directory only;
3. `EMULATOR_PROFILE_ROOT`: whole profile root, read-only or writable as
   declared;
4. `LEGACY_BROAD_ACCESS`: broad path required by an unisolatable adapter.

The selected level must appear in the launch-access receipt. There is no
automatic widening from level 1 to level 4.

## Flatpak

Flatpak starts applications in a sandbox. Its official documentation states
that access to host files and other resources must be explicitly granted, and
that portals provide controlled interaction with host files without requiring
blanket static permissions.^1 The sandbox-permissions documentation recommends
portals and read-only `:ro` access where possible, and warns against full-home
or full-host grants.^2

The practical contract is:

- do not modify an installed application's permissions silently;
- do not assume a host path is visible inside the sandbox;
- use the exact app ID and known runtime-visible path mapping;
- prefer a per-launch temporary directory that the existing permission model
  already exposes, or a portal/file-descriptor mechanism when the application
  supports it;
- if a new `--filesystem` grant would be required, present it as an explicit
  permission change and do not apply it implicitly;
- use `:ro` for immutable resources and a separate `:rw`/owned directory for
  saves only when the permission is already approved and the path is bounded;
- treat `--filesystem=home`, `--filesystem=host`, and broad external-drive
  grants as legacy/broad access, never as the default fix for a missing path.

Flatpak's per-app XDG config/data/cache locations are a useful writable-state
boundary. They do not solve access to a game or BIOS outside the sandbox. A
future Flatpak adapter should therefore produce both a host-side grant and an
inside-sandbox path/portal binding, and should fail closed if the mapping is
not proven. EmuWiz must not call `flatpak override` as part of an ordinary
launch.

## RetroArch case study

RetroArch's documented command line supports a specific core and content path,
`--config` for a selected config, and `--appendconfig` for a small additional
configuration. Its directory documentation identifies separate system/BIOS,
savefile and savestate directories.^3,^4 The `system_directory` is therefore
the right conceptual replacement for exposing a master BIOS warehouse.

Recommended strict projection:

```text
core: selected verified core, read-only
content: selected game/media path, read-only or scratch by adapter policy
system: only required BIOS files in a private system projection
config: generated minimal config plus approved read-only profile values
savefile: one selected writable save directory
savestate: separate explicit state directory, not automatically included
cache: private disposable cache where supported
```

RetroArch per-core, per-content-directory and per-game override mechanisms are
useful for settings, but they can contain arbitrary supported settings and can
be saved by RetroArch. A future adapter should generate an allowlisted
temporary config/appendconfig and never expose the user's entire config tree
just to obtain one override. Core discovery and content launch remain the
existing adapter's responsibility.

The strict pilot should start with a core whose BIOS requirements are already
represented by existing EmuWiz evidence. If a core searches a fixed system
directory or needs files that cannot be mapped safely, classify the launch as
`NARROW_DIRECTORY` or unsupported rather than guessing. No controller or input
metadata changes this access policy.

## MAME case study

MAME is dependency-closure oriented. Official MAME documentation describes
`-rompath` as one or more semicolon-separated search paths and documents
`-listxml`, `-listroms`, `-listcrc`, `-verifyroms`, parent systems, BIOS systems
and device ROM lookup. Its asset-search documentation says MAME searches device
ROMs through device, parent-device, system, parent-system and corresponding BIOS
locations.^5,^6

The correct minimal projection is not “one ZIP for the selected game.” It is:

```text
selected machine
  + selected parent/clone closure
  + required BIOS closure
  + required device-ROM closure
  + required CHD/disk closure
  + only the approved samples/software dependencies if the launch contract says so
```

EmuWiz should ask the existing MAME compatibility/dependency model for this
closure. A future projection can create a temporary `rompath` containing
file-level links or read-only binds to the exact archives/directories. It must
respect merged, split and non-merged semantics and must not infer archive
self-containment from a filename. If MAME's search behavior cannot be
represented without a broad directory, the grant must say so and use the
explicit fallback level.

CHDs require special handling: a CHD may be referenced from a set-specific
disk directory and may have parent/clone relationships. Expose only its
approved containing path or exact file according to MAME's proven lookup rules;
do not make the whole CHD library visible. Large CHDs should normally use
direct read-only access or a read-only bind, not per-launch physical copies.

## FBNeo case study

FBNeo has its own core and ROM expectations. MAME's machine/dependency model
must not be reused as FBNeo truth. The same observed ROM bytes may be compared
against both emulators, but each adapter must provide its own expectation
closure, BIOS semantics and layout policy.

The strict projection should therefore be built from the installed FBNeo/core
expectation index and existing observed evidence:

- selected FBNeo set members only;
- FBNeo-specific BIOS/board dependencies only when its definition proves them;
- exact archive layout only when the core's search semantics are known;
- no automatic MAME-to-FBNeo mapping and no broad MAME `rompath` reuse.

If the installed core expects a directory-level collection and no narrow
content path is supported, classify the grant as `NARROW_DIRECTORY` or
`LEGACY_BROAD_ACCESS`, with the reason visible to the user.

## xemu case study

xemu separates immutable console firmware/media from writable console state.
Its official CLI documents `-dvd_path` for the disc image, `-config_path` for a
config file, and `-snapshot` to discard HDD writes after exit.^7 Its FAQ says
game saves live on the virtual HDD and snapshots are saved to that HDD image;
the Flatpak guidance also illustrates that the HDD must be in a writable
permitted location while BIOS/MCPX files have different access needs.^8

Recommended grant shape:

- MCPX/flash BIOS: exact read-only firmware references;
- game XISO: exact read-only media path, subject to xemu's format rules;
- config: generated or selected per-launch config path;
- EEPROM: explicit writable state resource, never the immutable master;
- HDD: explicit writable image, Save Vault-owned or per-profile as applicable;
- snapshot mode: only where the adapter can prove the semantics and user
  policy; do not infer that `-snapshot` protects all state.

The HDD is not a normal “save file.” It is a container for saves and xemu
snapshots, so copying or snapshotting it has a much higher cost and requires a
separate capacity/recovery policy. A strict launch should not silently create a
new blank HDD or point to a shared writable master.

## Dolphin case study

Dolphin's official documentation describes a user directory containing saves,
settings, screenshots and other data, and documents `-u`/`--user` for a custom
user directory for the current session.^9 The upstream command-line parser
also exposes `--user`, `--exec`, `--config` and save-state options.^10

This supports a strong separation:

- game disc: exact read-only content path;
- IPL/DSP/Sys: selected immutable system files, preferably from an approved
  per-profile system projection;
- user directory: per-launch or per-profile writable root chosen with `-u`;
- saves/NAND/SYSCONF: explicit persistent or scratch subresources, never the
  entire unrelated user directory;
- shader/cache/logs: disposable private subdirectories where practical.

The adapter must decide whether a profile copy is safe to seed and which
subdirectories are persistent. A whole user-directory projection is acceptable
only as an explicit fallback because Dolphin's user directory contains more
than saves.

## PCSX2, DuckStation and RPCS3

### PCSX2

PCSX2 documents `-datapath` for all application data, `-portable`, a direct
boot filename, and separate memory-card/save-state concepts.^11 Its memory-card
documentation distinguishes file memory cards from folder memory cards and
warns that folder cards have compatibility caveats.^12

The preferred plan is a per-launch datapath or profile projection containing
only approved config, BIOS reference, memory-card binding, save states if
explicitly selected, and cache. A memory-card image is writable user state;
it must not be hardlinked as scratch or exposed through a broad save tree. If a
selected card is shared, the launch receipt must say that it is shared and
that isolation is not complete.

### DuckStation

DuckStation's current EmuWiz adapter and profile discovery should remain the
source of truth for supported command/config paths. The same policy applies:
exact disc/CHD path read-only, selected BIOS files only, and explicit memory
card/state/config roots. A future adapter must first verify the available
portable/config-directory controls against the installed build rather than
assuming PCSX2 flags or layout.

### RPCS3

RPCS3 has a profile-like data tree and title-specific save data, firmware and
cache concerns. The existing RPCS3 firmware/binding evidence remains the
authority for eligibility. A future isolation contract should project only the
selected title's verified content and required firmware, while giving RPCS3 an
owned writable data root where its configuration and saves need to live. PS3
account-bound/protected saves remain opaque and governed by Save Vault/PS3
safety rules; isolation must not become a transformation or resigning path.

Until exact installed-version path overrides and write behavior are verified,
RPCS3 should use `EMULATOR_PROFILE_ROOT` or remain unsupported for strict
per-title isolation. Do not pretend a directory name alone proves safety.

## Configuration isolation

Configuration has three distinct strategies:

| Strategy | When appropriate | Boundary |
|---|---|---|
| `USE_EXISTING_CONFIG` | Existing config is proven read-only/compatible and does not write beside source | Explicit exception, not default |
| `READ_ONLY_CONFIG` | Emulator supports a read-only config path or namespace | Still isolate other writable state |
| `TEMP_CONFIG` | Emulator accepts a config path or append config | Seed only allowlisted values; discard after session |
| `CONFIG_OVERLAY` | Adapter has proven layered config semantics | Keep base read-only; writable layer owned by launch |
| `PROFILE_COPY` | Emulator writes broad profile state | Copy from a verified seed and never copy back implicitly |
| `PROFILE_REFERENCE` | Adapter requires a stable profile root | Grant exact root and report breadth |

Do not rewrite a user's config merely to create isolation. Do not let arbitrary
remote metadata or game filenames choose config paths. A generated config must
be written only under an owned temporary root and must have a bounded allowlist
of settings.

## Save/state boundary

Save Vault owns snapshot, restore, verification, and rollback policy. Launch
isolation owns only the access path presented to the emulator and the
lifecycle of disposable state. The two systems should exchange typed
references, not filesystem guesses.

Normal battery saves, memory cards, NVRAM, NAND and HDD images are persistent
state and should be granted `READ_WRITE` only when the launch contract names
them. Save states are separate, version-sensitive artifacts and should not be
silently included because ordinary saves are present. A launch may use a
temporary save-state path for an explicit resume operation, but that is not
the same as game readiness or ordinary save protection.

For a shared memory card/container, the grant must identify the whole
container and state that multiple games may be affected. Per-game surgery is
not a safe isolation primitive. A future pre-launch Save Vault snapshot can
protect the whole container; it must not pretend the selected game owns it.

## Temporary resource view

The standard shape should be an owned, per-session view where direct paths are
not sufficient:

```text
$XDG_RUNTIME_DIR/emuwiz/launch/<opaque-id>/
    media/       # scratch copies or exact read-only projections
    firmware/    # only selected firmware/BIOS files
    config/      # generated/seeded config if required
    data/        # isolated emulator data root if required
    saves/       # persistent binding or disposable state, explicitly typed
    cache/       # disposable cache
    runtime/     # logs, receipts, ownership marker
```

`XDG_RUNTIME_DIR` is a good first choice because it is intended for user
runtime data and normally has private permissions, but it may be tmpfs. The
workspace policy should choose a disk-backed temporary root when size and
durability requirements make tmpfs inappropriate. The launch ID should be
opaque and bounded, not derived from title or source filenames.

Every temporary root needs:

- owner marker and launch ID;
- creation timestamp and process identity;
- list of grants and source identities;
- explicit cleanup state;
- no source paths as writable targets;
- bounded total size and free-space preflight;
- reconciliation state if the process terminates unexpectedly.

## Cleanup and crash recovery

The lifecycle is:

```text
PREPARE -> PREFLIGHT -> LAUNCH -> MONITOR -> EXIT -> VERIFY -> CLEANUP
```

On normal exit, stop accepting process descendants, verify any persistent
state binding, then remove only temporary paths created by the launch. On
emulator crash, retain the workspace briefly for diagnostics unless it exceeds
the bounded retention policy; do not immediately remove a path while a child
process may still use it. On EmuWiz crash or power loss, a later startup can
enumerate only owned, stale-marked roots and offer safe cleanup after proving
no matching process remains.

Cleanup must be descriptor/path-confined and refuse to follow links outside the
owned root. It must never delete source media, pre-existing directories,
persistent saves, or unrelated files. Any uncertain cleanup result becomes
`NEEDS_RECONCILIATION`, not a claim of success.

## Performance

Least privilege does not require copying everything.

Use this preference order:

1. direct exact path for proven read-only content;
2. file-level symlink only for read-only name projection;
3. read-only bind/namespace for large immutable trees where the environment
   supports it;
4. plain scratch copy for writable-risk media/config;
5. future reflink only after filesystem-specific proof.

Cache projections by source identity, adapter contract/version and policy, but
never treat a projection cache as authoritative evidence. Invalidate when
source device/inode/size/mtime or stronger existing hash evidence changes, or
when emulator profile/contract changes. Avoid repeated scans: the existing
launch plan and topology/dependency evidence should produce an indexed grant
list in linear time in the selected closure.

## Security model

### Threats

- buggy emulator writes through a content path or symlink;
- emulator follows a directory link into a broader source tree;
- malicious archive/path metadata causes traversal;
- a source is replaced after preview and before copy/spawn;
- emulator deletes a support file or config beside its executable;
- temporary cleanup follows a link and deletes unrelated data;
- Flatpak permissions silently broaden access;
- a child process outlives the parent emulator;
- writable state is accidentally backed by an immutable master.

### Required controls

- typed argv with no shell interpolation;
- explicit path allowlist per launch;
- canonical/descriptor-based confinement where available;
- source identity capture and final revalidation;
- destination root ownership marker and mode `0700` where possible;
- no hardlinks for writable scratch;
- no automatic source-to-master copyback;
- explicit grant widening and user-visible fallback level;
- bounded copy size, timeout and free-space checks;
- no network or remote metadata access during resource preparation unless the
  existing authorized workflow explicitly requires it;
- process-tree and running-state checks before cleanup;
- audit receipt of resources granted and denied.

## Source roles and visibility versus access

Source roles should remain descriptive ownership/provenance facts:

| Source role | Library visibility | Launch access |
|---|---|---|
| `GAME_MEDIA` | User-selected/visible game | Selected file or topology closure only |
| `BIOS_FIRMWARE` | Usually technical/advanced visibility | Selected required firmware only |
| `SAVE_DATA` | User-visible Save Vault state | Selected writable binding only |
| `DAT_METADATA` | Tooling/technical view | Never by default |
| `ARTWORK` | Library presentation | Never by default |
| `TEMPORARY_RUNTIME` | Usually hidden | Only to the target process |

Thus `hidden` never means `inaccessible`, and `visible` never means “the
emulator receives the directory.” A launch contract should reference an
existing source role plus a specific resource identity and independently
choose the grant.

## User experience and debugging

Normal launch review should say:

> EmuWiz will give this emulator access to 1 game file, 2 BIOS files and its
> save directory.

For broad requirements:

> This emulator currently requires access to its full profile directory. EmuWiz
> could not narrow that access for this launch.

For unsupported isolation:

> EmuWiz could not prove a safe isolated view for this media, so the launch was
> not started.

An advanced launch-access view should show a bounded table:

| Resource | Access | View | Lifetime |
|---|---|---|---|
| `game.chd` | Read only | Direct path or read-only bind | Launch only |
| `scph5502.bin` | Read only | File projection | Launch only |
| `memory-card-1.mcd` | Read/write | Explicit selected state | Persistent |
| temporary config | Read/write | Owned generated file | Until cleanup |
| BIOS master folder | No access | Not projected | None |

Technical details should include source evidence, identity/freshness, adapter
contract, fallback level, projection paths and cleanup/reconciliation state.
The normal UI should not dump giant process logs or path internals, but the
receipt must make a reviewable security decision possible.

## Comparable systems

The useful patterns from comparable systems are:

- **Flatpak:** default sandbox, explicit filesystem permissions, portals and
  read-only grants; broad host/home permissions are a deliberate downgrade.
- **Bubblewrap:** process-specific filesystem namespace, read-only binds,
  separate writable binds and optional namespace isolation.
- **Firejail:** profile-based filesystem restrictions, useful conceptually but
  not a dependency EmuWiz should assume is installed or trustworthy.
- **Containers:** namespace and mount isolation, but they add runtime/image
  complexity and are not automatically suitable for desktop GPU/audio/input.
- **systemd sandboxing:** `ProtectSystem`, `ReadOnlyPaths`,
  `InaccessiblePaths` and `BindReadOnlyPaths` show a useful declarative model
  for deny-by-default service processes, but desktop launch integration and
  user-session ownership need careful handling.^13
- **Steam Linux Runtime / Proton prefixes:** per-title compatibility/state
  roots demonstrate the value of scoped writable state, but broad runtime
  compatibility trees should not be confused with game-media permissions.
- **Lutris runners:** per-game runner/config/environment bindings are useful
  UX patterns; arbitrary user-authored commands are not a safe EmuWiz trust
  model.
- **Wine prefixes:** isolated writable application state is a useful analogy,
  particularly for separating immutable installers/media from mutable prefix
  state.
- **RetroArch, Batocera and RetroDECK:** dedicated system, saves, configs and
  cores directories show that resource classes map well to practical emulator
  layouts, but their directory conventions do not prove per-launch isolation.

## Final decisions

### 1. Should least-privilege exposure be a core principle?

Yes. It is a launch safety invariant and should be part of future launch
contracts, not an optional GUI preference.

### 2. Which classes need explicit access modes?

All classes that can reach an emulator need an explicit mode, especially game
media, BIOS/firmware, dependency/device ROMs, config/profile, saves, memory
cards, NVRAM, NAND, HDD images, caches and temporary runtime paths. Unknown
resources receive `NO_ACCESS` until classified.

### 3. Should temporary launch views be standard?

Yes, whenever direct read-only access is not proven or when config/writable
state must be isolated. They should be a standard lifecycle capability, not a
second launch path.

### 4. Should file-level symlinks replace broad directory symlinks where possible?

Yes for read-only name projection. They do not protect against writes, so
scratch copies or read-only namespace views remain mandatory for writable-risk
resources.

### 5. Are bind mounts/namespaces worth using?

Yes as an optional Linux hardening and large-media optimization layer. They
should not be the first universal dependency or a hidden fallback. Capability
failure must be explicit.

### 6. How should Flatpak be handled?

Treat Flatpak as a second sandbox boundary. Use existing approved app paths,
portals or explicit narrow read-only grants where supported. Never silently
change permissions or grant home/host access to make a launch work.

### 7. How should writable state be isolated?

Use an explicitly typed persistent state binding or a private temporary copy.
Never point writable emulator state at immutable masters, never hardlink
writable scratch, and never copy emulator mutations back without a separate
reviewed workflow.

### 8. How should MAME closure be projected?

Use the existing MAME dependency model to resolve selected machine, parent,
BIOS, device and CHD closure. Project exact members into a temporary `rompath`
or a read-only namespace where MAME semantics permit it; otherwise report the
required narrow/broad fallback explicitly.

### 9. How should access widening be represented?

As a typed fallback level in the launch contract, plan and receipt:
`STRICT_MINIMAL`, `NARROW_DIRECTORY`, `EMULATOR_PROFILE_ROOT`, or
`LEGACY_BROAD_ACCESS`. Widening is never silent and never authorized merely by
missing implementation convenience.

### 10. What is the smallest safe implementation slice?

FI0: typed resource grants, access modes, projection methods, lifetimes and a
read-only receipt; no filesystem mutation. Then FI1: one RetroArch pilot using
existing verified BIOS/core/media evidence and an owned temporary config/system
view, with a focused source-immutability test. Only after that should native
adapters, MAME/FBNeo closure and optional namespaces be considered.

## Recommended roadmap

### FI0 — vocabulary and receipt

Define the grant model, fallback levels, source-evidence references and
read-only access debug projection. Add pure tests for deterministic ordering,
unknown-resource denial and no readiness effect.

### FI1 — RetroArch minimal BIOS/media projection

Use existing core/content/firmware evidence. Generate an owned per-launch
system/config/save view where the selected core supports it. Do not change
RetroArch permissions, use a broad BIOS folder, or introduce a second planner.

### FI2 — native emulator narrow grants

Pilot Dolphin first because its `-u` user-directory override is documented;
then verify PCSX2's `-datapath` and xemu's `-config_path`/state contract. Add
per-adapter scratch/config declarations only after focused tests.

### FI3 — MAME/FBNeo dependency-closure projections

Consume existing per-set compatibility and observed-evidence bridges. Build
an indexed exact closure, preserve each emulator's semantics, and project a
temporary read-only dependency view where safe.

### FI4 — temporary launch-view lifecycle

Add owned runtime roots, bounded scratch copying, source identity checks,
process-lifetime cleanup, crash reconciliation and launch-access receipts.

### FI5 — optional namespace/bind isolation

Add Linux capability detection and a strict bubblewrap/native namespace
backend. Keep direct/copy fallback semantics explicit, test Flatpak overlap,
and never require privileged mounts for ordinary launches.

## Do not build

EmuWiz should explicitly reject the following designs:

- exposing whole BIOS trees by default;
- exposing whole save or config trees because an adapter is inconvenient;
- broad access merely because setup is easier;
- copying multi-gigabyte ROMs for every launch without a size/policy reason;
- using hardlinks or source symlinks as writable scratch;
- deleting source files or copying emulator mutations back during cleanup;
- global permission changes or silent Flatpak overrides;
- treating hidden UI data as inaccessible, or source roles as permissions;
- letting emulator-controlled or remote metadata choose arbitrary paths;
- building a second scanner, dependency engine or launch planner;
- treating a MAME closure as FBNeo truth;
- allowing strict isolation failure to silently become legacy broad access.

## Sources

1. Flatpak, “Basic concepts,” <https://docs.flatpak.org/en/latest/basic-concepts.html>.
2. Flatpak, “Sandbox permissions,” <https://docs.flatpak.org/en/latest/sandbox-permissions.html>.
3. Libretro, “Directory configuration,” <https://docs.libretro.com/guides/change-directories/>.
4. Libretro, “CLI introduction,” <https://docs.libretro.com/guides/cli-intro/>.
5. MAME, “Universal command-line options,” <https://docs.mamedev.org/commandline/commandline-all.html>.
6. MAME, “How does MAME look for files?,” <https://docs.mamedev.org/usingmame/assetsearch.html>.
7. xemu, “Command line arguments,” <https://xemu.app/docs/cli/>.
8. xemu, “FAQ / troubleshooting,” <https://xemu.app/docs/faq/> and <https://xemu.app/docs/troubleshooting/>.
9. Dolphin Emulator, “Controlling the global user directory,” <https://dolphin-emu.org/docs/guides/controlling-global-user-directory/>.
10. Dolphin Emulator, “Command line parser,” <https://github.com/dolphin-emu/dolphin/blob/master/Source/Core/UICommon/CommandLineParse.cpp>.
11. PCSX2, “Command line options,” <https://pcsx2.net/docs/advanced/cli/>.
12. PCSX2, “Memory cards,” <https://pcsx2.net/docs/configuration/memcards/>.
13. systemd, `systemd.exec(5)`, <https://man7.org/linux/man-pages/man5/systemd.exec.5.html>.

Local design/research sources used for the EmuWiz-specific inventory and
boundaries:

- `docs/research/LAUNCH_RECIPE_ARCHITECTURE_AUDIT.md`.
- `docs/research/SAFE_LAUNCH_SANDBOX_V1.md`.
- `docs/research/MEDIA_TOPOLOGY_LAUNCH_INTEGRATION.md`.
- `docs/LAUNCH_SUPPORT.md`.
- `docs/EXTERNAL_EMULATOR_LAUNCH_PARITY_AUDIT.md`.

## Final recommendation

Adopt least-privilege emulator file exposure as a core launch invariant. Keep
`LaunchPlan` and existing evidence authorities unchanged, add a typed grant and
receipt layer, and implement one tested projection pilot before broadening to
other adapters. The first pilot should prefer exact RetroArch content/core
bindings and selected BIOS files, isolate writable paths, and fail closed when
the installed core cannot provide a narrow view. This produces measurable
safety value without changing launch selection, Save Vault policy, BIOS
projection, emulator configuration ownership, or any existing GUI behavior.
