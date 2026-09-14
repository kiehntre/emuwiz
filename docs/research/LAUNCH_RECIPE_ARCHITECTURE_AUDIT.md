# Launch Recipe Architecture Audit

## Executive decision

An abstraction is justified, but only as a typed, declarative contract that
describes an adapter's launch requirements and the shape of a resolved launch
action. It must not become a second launch planner, a shell-script language, a
BIOS database, a media-topology engine, or a Save Vault policy owner.

The existing `LaunchPlan` should remain the sole authority for selecting a
candidate and assigning launch readiness. A future `LaunchContract` should be
an adapter-facing capability/requirement description. A future
`ResolvedLaunchRecipe` should be a plan detail produced by the existing
planner after identity, media topology, firmware, profile, installation, and
preflight evidence have already been resolved.

The smallest justified implementation is a vocabulary-only slice followed by
an internal projection for RetroArch and two native adapters. No user-authored
recipe files, no generic template interpreter, and no automatic configuration
mutation should be part of the first slice.

## Scope and evidence

The local inspection was performed against EmuWiz worktree
`/home/davedap/emuwiz-main-release-fix` at commit
`c11ddeceabd156c0ec7c16af8f67b16b243fa662`. The worktree contained unrelated
dirty and untracked changes; none were altered by this audit. This document is
research only.

External comparison focused on primary project documentation and upstream
configuration formats. The most useful sources were RetroArch's CLI and
directory documentation, xemu's CLI documentation, Pegasus metadata
documentation, Lutris's installer/configuration documentation, and LaunchBox's
command-variable documentation. These projects demonstrate useful concepts,
but their string-command flexibility is not an appropriate EmuWiz trust model.

## 1. Current EmuWiz architecture

### Core launch flow

The current architecture is deliberately layered:

| Layer | Local source | Current responsibility | Safety boundary |
|---|---|---|---|
| Launch module boundary | `crates/archivefs-core/src/launch/mod.rs` | Exposes the launch vocabulary and adapter modules | States that launch planning/execution does not mutate configs, download firmware, or mount content |
| Pure candidate planning | `launch/planning.rs` | Builds `LaunchPlan`, `LaunchCandidate`, `LaunchTarget`, `LaunchContentRef`, platform/game key, candidate preference, summary | No filesystem, network, writes, or process spawn |
| Platform routing | `launch/platform_map.rs` | Maps platform IDs to standalone adapter IDs and RetroArch candidates | Does not inspect installation state or reimplement adapter checks |
| Readiness projection | `launch/readiness.rs` | Projects firmware/readiness facts into `LaunchReadiness`, blockers, warnings, and fixability-adjacent detail | Reuses upstream evidence and does not launch |
| Media topology | `launch/topology.rs`, `media_set/*` | Projects a media set into a first launch medium, swap plan, missing-media facts, and launchable topology | Does not duplicate identity or choose a second media model |
| Input projection | `launch/input_projection.rs` | Converts verified identity facts into adapter-specific launch input requests | Does not read files, write configs, or spawn processes |
| Evidence bridge | `launch/evidence_bridge.rs` | Adapts canonical identity/game inspection into planner input | Identity remains owned by evidence/identity modules |
| RetroArch command planning | `launch/retroarch_command.rs` | Resolves one already-selected profile/core/executable into typed argv | No live discovery, writes, mount, shell, or spawn |
| Native command planning | `launch/*_command.rs` | Produces typed command plans for individual adapters | Each adapter validates its own supported content/profile quirks |
| Process execution | `launch/process_spawn.rs` | Spawns exact executable plus `OsString` arguments, captures bounded stderr, watches exit | No shell, no automatic kill, timeout, retry, or environment override |
| RetroArch execution | `launch/execution.rs` | Freshly revalidates content, identity, RetroArch environment, candidate/core, then spawns | Refuses stale or non-ready requests; current slice is intentionally narrow |
| Native execution | `launch/*_execution.rs` | Adapter-specific fresh preflight and process lifecycle | Adapter policy remains local |
| Emulator/profile evidence | `emulator_environment/*`, `patch_manager/*profile*`, adapter modules | Discovers profiles, executable provenance, config locations, firmware facts, and eligibility | Discovery is evidence, not permission to execute arbitrary paths |
| Firmware/BIOS | `bios_projection.rs`, `launch/readiness.rs`, emulator environment modules | Provides reusable firmware evidence and readiness projections | Recipes must reference these facts, not copy BIOS rules |

### Existing command duplication

There is meaningful structural duplication across native command modules. The
following modules each define a command, command selection, command plan,
blocker helper, content-path checks, executable/profile selection, argument
construction, and usually `working_directory: Option<PathBuf>`:

`pcsx2_command.rs`, `duckstation_command.rs`, `dolphin_command.rs`,
`rpcs3_command.rs`, `ppsspp_command.rs`, `xemu_command.rs`,
`fsuae_command.rs`, `hatari_command.rs`, `mame_command.rs`,
`fbneo_command.rs`, `retroarch_command.rs`, `flycast_command.rs`,
`amiberry_command.rs`, `amiberry_cd_command.rs`, `dosbox_command.rs`,
`cemu_command.rs`, `xenia_command.rs`, `vita3k_command.rs`, and the other
modules re-exported by `launch/mod.rs`.

This is not evidence that all command construction should be collapsed. The
duplication is currently carrying valuable adapter-specific refusal rules. For
example:

- PCSX2 and DuckStation restrict direct disc extensions and refuse unresolved
  mounted/archive inputs.
- PPSSPP intentionally supports only the narrow direct PSP path in its current
  execution slice and does not claim CSO, CHD, ZIP, or multi-file support.
- xemu has a specific `-dvd_path` contract and separate firmware blockers.
- FS-UAE requires an eligible profile, explicit configuration, and Kickstart
  readiness.
- MAME/FBNeo bind to set identity, search-path/profile evidence, and emulator
  compatibility rather than a generic “ROM filename” concept.

The genuine commonality is the *shape* of a launch contract, not the policy
that decides whether each field is valid.

### Existing safety boundaries worth preserving

The current code already establishes several principles a recipe design should
make explicit rather than weaken:

1. Launch planning is data-only and receives already-gathered evidence.
2. A command plan is typed argv, not concatenated shell text.
3. Paths with lossy display representations are refused where exact launch
   paths are required.
4. Archive/mount-input paths are not silently treated as runnable media.
5. The execution boundary revalidates mutable facts immediately before spawn.
6. The process layer inherits the environment and working directory unless a
   caller supplies them; it does not invent overrides.
7. A missing firmware fact and an unsupported adapter are distinct outcomes.
8. Media topology and firmware projections are upstream authorities, not
   adapter-local duplicates.

## 2. Recipe versus plan

The terms should be separated precisely:

| Concept | Meaning | Lifetime | Authority |
|---|---|---|---|
| `LaunchContract` | What an adapter can launch and what it requires | Stable across many games, subject to adapter/version changes | Adapter capability plus version/install constraints |
| `LaunchRecipe` | A declarative request describing one resolved launch shape | One game/candidate/machine launch attempt | Existing `LaunchPlan` supplies all resolved values |
| `LaunchPlan` | All currently knowable candidates for one canonical game, including readiness | Point-in-time projection | Sole candidate-selection/readiness authority |
| `PreparedProcessCommand` | Exact executable, argv, and optional CWD ready for spawn | Immediately before process start | Execution preflight |
| `LaunchSession` | Runtime ownership of temporary projections, process, cleanup, and reconciliation | From preparation through exit/cleanup | Lifecycle executor, not planner |

The important rule is one-way data flow:

```text
adapter contract + gathered evidence
        -> existing LaunchPlan / candidate selection
        -> resolved launch recipe
        -> preflight and PreparedProcessCommand
        -> LaunchSession lifecycle
```

`LaunchRecipe` must never choose between candidates, reinterpret identity,
recompute firmware validity, or bypass readiness blockers. If it can do those
things, it has become a competing planner.

## 3. Proposed conceptual model

The following is a design vocabulary, not a production API recommendation for
immediate implementation.

```text
LaunchContract {
    contract_version
    emulator_identity
    supported_platforms
    installation_kinds
    executable_requirement
    media_capabilities
    firmware_requirements
    config_capabilities
    writable_state_schema
    environment_policy
    working_directory_policy
    argv_schema
    preflight_requirements
    lifecycle_capabilities
}

ResolvedLaunchRecipe {
    contract_id
    contract_version
    selected_candidate_ref
    executable_binding
    media_bindings
    firmware_bindings
    config_binding
    writable_state_bindings
    environment_bindings
    working_directory
    argv
    temporary_projections
    preflight_receipt
    cleanup_policy
}
```

### Universal fields

The fields that are common enough to justify a shared vocabulary are:

- stable emulator/adapter identity;
- selected installation/profile reference;
- executable binding, expressed as a trusted resolved executable or approved
  launcher tuple;
- one or more typed media bindings;
- references to existing firmware/BIOS evidence;
- config/profile mode;
- typed writable-state bindings with ownership and persistence policy;
- explicit environment additions/removals, subject to an allowlist;
- working-directory policy;
- structured argv elements;
- preflight facts required before preparation;
- temporary projection ownership;
- cleanup and reconciliation policy;
- contract and emulator-version applicability.

### Adapter-specific fields

The following must remain adapter-specific or capability-specific:

- exact flags (`-dvd_path`, `-datapath`, `-e`, `--config`, core selection);
- BIOS filenames and firmware semantics;
- MAME/FBNeo set/dependency closure;
- PCSX2/FS-UAE profile formats;
- emulator-specific writable files such as xemu EEPROM/NAND/HDD behavior;
- disc swap commands and runtime media-change behavior;
- process-tree/launcher quirks;
- whether a configuration is read-only, copied, or mutated by the emulator;
- acceptable media formats and topology restrictions.

These can be represented behind typed contract variants or capability enums;
they should not be flattened into a string map.

## 4. Executable resolution

### Safe executable binding

Use an enum-like binding rather than a command string:

```text
ExecutableBinding::NativeResolved { exact_path, provenance }
ExecutableBinding::ManagedInstall { install_id, executable_role }
ExecutableBinding::AppImage { exact_path, verified_identity }
ExecutableBinding::Flatpak { app_id, branch, launcher: "flatpak" }
ExecutableBinding::Wrapper { trusted_wrapper_id, resolved_path }
ExecutableBinding::Wine { runner_id, executable_path, prefix_binding }
```

The resolved form should contain an executable plus arguments only after the
installation manager has selected and verified it. A Flatpak is not an
arbitrary path; its app ID and branch are structured data. A managed install
should resolve through installation provenance at planning/preflight time, not
hardcode a versioned filesystem path. A wrapper should be an EmuWiz-known
launcher role, not arbitrary user-authored shell text.

The current RetroArch path already illustrates this boundary. Its environment
report distinguishes native/AppImage/Flatpak profiles, exact executable
paths, core library paths, and profile references. The current executor
deliberately refuses Flatpak in its narrow real-launch slice because the report
does not yet prove an exact executable suitable for that executor. A contract
must preserve that honest refusal rather than “support” Flatpak by inventing a
launcher command.

### No arbitrary shell

`std::process::Command::new` plus `.args` is the correct primitive already used
by `launch/process_spawn.rs`. Every dynamic value remains one argument. The
recipe model must reject shell metacharacter interpretation, command pipes,
redirections, command substitution, and executable strings containing hidden
arguments.

## 5. Media bindings and topology

### Media binding vocabulary

A shared media binding can describe the *role* of content without taking over
media identity:

```text
MediaBinding::SingleFile { path, format, access: ReadOnly }
MediaBinding::OpticalProjection { projection_id, entry_path, topology_ref }
MediaBinding::MultiDiscProjection { topology_ref, launch_entry, ordered_media }
MediaBinding::Directory { path, layout, access }
MediaBinding::ArchiveMount { mount_ref, entry_path, lifecycle }
MediaBinding::Cartridge { path, format, access: ReadOnly }
MediaBinding::Tape { path, format, access }
MediaBinding::Floppy { path, format, access }
MediaBinding::HddImage { path, access: ScratchCopyRequired }
```

The binding must state whether the supplied path is:

- safe read-only source media;
- a writable user-state object;
- a scratch copy created for the session;
- a temporary projection that must be cleaned up;
- an archive/mount reference owned by another lifecycle.

An archive path is not automatically a media binding. It requires an already
prepared mount or extraction projection, with the source archive remaining
immutable.

### Multi-disc boundary

The recipe should request one `LaunchableMediaProjection` from the existing
media-set/topology layer. It should not group discs, parse M3U files, reorder
tracks, or invent swap semantics. For PS1, Saturn, Sega CD, Dreamcast, and PC
Engine CD, the topology engine remains responsible for the ordered media set,
missing entries, and first launch medium. The adapter contract only states
whether it accepts a CUE/M3U/direct-disc projection and whether runtime disc
swap is supported.

This retains the current `MediaTopologyLaunchProjection` shape and avoids a
second multi-disc engine.

### Access classification

| Media shape | Default access | Scratch copy? | Notes |
|---|---|---:|---|
| Cartridge/ROM | Read-only | No | Never turn a source ROM into writable runtime state |
| ISO/CHD/CUE/BIN/GDI | Read-only | Usually no | Topology may require a temporary playlist or mount projection |
| MAME/FBNeo set | Read-only set/dependency closure | No | Bind to set identity and closure, not one arbitrary filename |
| PSP ISO/CSO/PBP | Read-only | No | Save data is separate from the image |
| Memory card/EEPROM/NVRAM | Persistent user state or emulator state | Often yes for safety | Never classify as source media merely because it is a launch input |
| NAND/HDD image | Emulator system state | Frequently yes | xemu documents saves inside the virtual HDD; this is not a normal ROM input |
| Installed game directory | Adapter-defined | Depends | Must declare whether the emulator writes in place |
| Archive mount | Read-only projection | No source copy by default | Mount/unmount lifecycle belongs to the session |

## 6. Firmware and BIOS

Recipes should contain references such as:

```text
FirmwareBinding::NotRequired
FirmwareBinding::VerifiedRequirement { requirement_id }
FirmwareBinding::OneOfVerified { requirement_group_id }
FirmwareBinding::PresentUnverified { evidence_ref }
FirmwareBinding::Unknown { evidence_ref }
```

The `requirement_id` points to existing BIOS Projection/emulator-environment
evidence. It must not duplicate firmware filenames, hashes, or one-of-many
rules inside each recipe. The planner consumes the existing
`FirmwareReadiness`; a recipe only explains which existing requirement the
candidate depends on.

MAME/FBNeo are special because firmware, BIOS, device, parent, and CHD
dependencies are part of an emulator-specific set closure. A contract should
reference the compatibility evidence and closure result; it should not rebuild
the arcade compatibility subsystem.

## 7. Configuration and profiles

The useful shared modes are:

```text
ConfigBinding::ExistingProfile { profile_ref, access: ReadOnlyReference }
ConfigBinding::ExistingProfileWithOverride { profile_ref, override_ref }
ConfigBinding::TemporaryProfile { template_ref, destination_policy }
ConfigBinding::CopyOnLaunch { source_profile_ref, copy_policy }
ConfigBinding::RequiredButUnavailable { reason }
ConfigBinding::Unsupported
```

The contract describes whether a profile is required and whether the adapter
can run without it. It must not automatically edit a user's global config just
because the recipe requests a setting. A temporary config may be staged only by
a separately authorized lifecycle operation with atomic creation, ownership,
bounded paths, and cleanup.

RetroArch is a particularly strong example of configuration layering. Its
official documentation describes a base `retroarch.cfg`, core/content/game
overrides, remaps, and separate directory settings. The documented hierarchy
applies game, content-directory, and core settings over defaults, while the
system directory controls BIOS lookup and save locations can otherwise default
near ROMs.[^1][^2] EmuWiz should expose which layer is selected, not silently
rewrite the hierarchy.

## 8. Writable state and Save Vault boundary

Writable state needs an explicit type and owner:

```text
WritableStateBinding {
    kind: PersistentUserState
        | EmulatorSystemState
        | GameSaveState
        | TemporaryRuntimeState,
    path_or_projection: StateTarget,
    source_policy: NeverWriteSource | CopyBeforeUse | ExistingWritableTarget,
    persistence: Preserve | Reconcile | Discard,
    vault_visibility: InformOnly,
}
```

The key distinction is not merely file extension. A `.sav`, EEPROM, memory
card, NVRAM file, shader cache, config file, NAND image, and HDD image have
different owners and rollback requirements. Source ROMs/discs/images should
always be `NeverWriteSource`.

Save Vault remains the owner of backup, snapshot, restore, retention, and user
recovery policy. The launch contract may expose a pre-launch protection fact:
“this session will write PS2 shared memory card X” or “this session uses xemu
HDD image Y.” It must not call Save Vault, create backups, or decide retention.

For shared containers, a launch session can identify the entire writable object
as one protected target while separately exposing read-only contents/identity
facts. This is compatible with Save Vault's shared-container model.

## 9. Scratch-copy preservation

`SCRATCH_COPY_BEFORE_LAUNCH` is justified as a lifecycle capability, not a
boolean buried in a media path:

```text
ScratchPolicy::Never
ScratchPolicy::Required {
    source_ref,
    copy_kind,
    destination_root_policy,
    verify_identity,
    discard_after,
}
ScratchPolicy::RequiredAndReconcile {
    source_ref,
    output_state_kind,
    reconciliation_authority,
}
```

The source must be revalidated before copying; the copy must be created below
an EmuWiz-owned session root; the resulting identity must be checked; and
cleanup must be safe if launch, emulator startup, or the process crashes.
“Reconcile” is not automatically “copy everything back.” It requires an
adapter-specific, explicit merge contract and should initially be unsupported
for ambiguous or shared state.

## 10. Environment and working directory

### Environment

Environment bindings should be a constrained map of named roles:

```text
EnvironmentBinding::Inherit
EnvironmentBinding::RequirePresent { variable }
EnvironmentBinding::SetAllowlisted { variable, value_source }
EnvironmentBinding::UnsetAllowlisted { variable }
EnvironmentBinding::IsolateHome { root }
EnvironmentBinding::IsolateXdg { config, data, cache }
```

The inherited environment should be the default. Explicit requirements may
cover `DISPLAY`, `XAUTHORITY`, Wayland session variables, SDL controller
settings, Vulkan/driver selection, locale, `WINEPREFIX`, Proton runner state,
and Flatpak portal/session requirements. Values from ROM filenames, metadata,
or user strings must never become variable names or unvalidated values.

Display/session affinity is a launch-session concern. Sunshine/Moonlight can
provide a display and virtual controller environment, but those are not
universal emulator recipe fields. A generic contract should say “requires an
interactive graphics session” and “requires controller input capability”; a
streaming integration can satisfy those requirements separately.

### Working directory

The default should be explicit and stable, not accidental caller CWD:

```text
WorkingDirectory::ExecutableDirectory
WorkingDirectory::InstallRoot
WorkingDirectory::GameDirectory
WorkingDirectory::TemporarySessionRoot
WorkingDirectory::ExplicitTrustedPath
WorkingDirectory::InheritProcess
```

`InheritProcess` should be an explicit adapter capability, not an implicit
default. Current command plans already carry `working_directory: Option<PathBuf>`;
the contract can make the policy visible without changing each adapter's
quirks.

## 11. Structured argv

The safe model is an enum of argument atoms, resolved into `OsString` only at
the final command-plan boundary:

```text
ArgAtom::Literal("-L")
ArgAtom::ExecutableRole
ArgAtom::CoreLibrary
ArgAtom::SelectedMedia
ArgAtom::SelectedMediaList
ArgAtom::FirmwarePath(requirement_id)
ArgAtom::ConfigPath(config_binding)
ArgAtom::ProfileName
ArgAtom::GameKey
ArgAtom::AdapterLiteral(role)
```

An adapter may define an opaque typed argument sequence internally, but it
should not expose arbitrary user-editable format strings by default. Paths are
arguments, not quoted shell fragments. A safe debug view can render the argv
as a list with each argument separately escaped for display, while explicitly
stating that the display is not a copy-paste shell command.

RetroArch's documented minimal shape is executable plus `-L`, core library,
and content. Its CLI also supports explicit config selection and Flatpak
launch forms.[^1] EmuWiz's existing RetroArch command planner already keeps
those values separate and validates profile/core/platform matching; that code
should remain authoritative.

## 12. Preflight requirements

Preflight should be a reusable vocabulary that feeds existing blockers, not a
new readiness system:

- `ExecutableResolvedAndAuthorized`
- `ManagedInstallStillCurrent`
- `ProfileStillPresentAndEligible`
- `CanonicalIdentityStillMatches`
- `MediaProjectionExistsAndMatchesTopology`
- `MediaDependenciesComplete`
- `FirmwareRequirementSatisfied`
- `ConfigBindingReadable`
- `WritableTargetsAvailable`
- `ScratchCopyPreparedAndVerified`
- `EnvironmentRequirementSatisfied`
- `WorkingDirectoryAvailable`
- `NoConflictingLaunchSession`

Each requirement should map to an existing `LaunchBlockerKind`, warning, or
information fact. It must not create a parallel `RecipeReadiness` enum.

Preflight is necessarily time-sensitive. The current RetroArch execution code
correctly rebuilds identity/content/environment evidence immediately before
spawn instead of trusting an earlier readiness report. A future recipe should
be resolved and revalidated at the same boundary.

## 13. Cleanup and process lifecycle

### Temporary resources

The session should own a manifest of temporary resources:

```text
TemporaryProjection {
    id
    path
    resource_kind
    owner_session_id
    source_identity
    cleanup: RemoveIfOwned | UnmountIfOwned | RetainForRecovery
}
```

Cleanup must be idempotent, path-bounded, ownership-checked, and safe after
partial preparation. It must never recursively delete a user-selected source
root or a path merely because it resembles a temporary directory. Mounts and
archive projections should be torn down only if EmuWiz established them.

### Process lifecycle

The existing `process_spawn.rs` is intentionally minimal: one exact spawn,
bounded stderr capture, background wait, non-blocking polling, and no
automatic kill or timeout. A future `LaunchSession` can own process metadata
and cleanup state, but should not turn every adapter into a watchdog. The
contract should declare capabilities such as:

- `WaitForDirectChildExit`;
- `WrapperMaySpawnChildren`;
- `NeedsProcessGroupObservation`;
- `CleanupOnNormalExit`;
- `CleanupOnCrash`;
- `CleanupOnPreparationFailure`.

The implementation of process-tree discovery, Flatpak wrapper handling, and
crash cleanup should be separately researched before being generalized.

## 14. Adapter case studies

The table summarizes what a shared contract can express without erasing local
policy. “Current shape” describes the inspected command/planner modules, not a
claim that every adapter has a production spawn path.

| Adapter | Executable/profile | Media | Firmware/config | Writable state / lifecycle | Contract conclusion |
|---|---|---|---|---|---|
| RetroArch | Native/AppImage profile, exact executable, selected core | Direct content in current executor; topology projection future input | Core/platform match, system directory and core/profile evidence | Saves/configs exist in RetroArch directory model; current executor inherits environment and CWD | Best first contract projection; keep core/profile selection authoritative |
| PCSX2 | Discovered native profile; profile ID/path | Direct PS2 image in current command path; unresolved mounts refused | Existing PCSX2 firmware readiness; optional `-datapath` profile mode | Memory cards, config, states are profile-owned; no generic mutation in command planner | Good native pilot; datapath is typed config binding |
| DuckStation | Discovered profile | Direct PS1 content with adapter extension checks; topology must resolve first | DuckStation firmware/profile evidence | Memory cards, saves, settings are profile-owned | Good native pilot; multi-disc stays upstream |
| Dolphin | Profile and native executable binding | Direct `.iso`, `.gcm`, `.rvz`, `.ciso`, `.wbfs` path checks | Dolphin profile/config and GameCube/Wii identity evidence | Memory cards, Wii NAND, GameSettings, texture/state areas are distinct | Contract must distinguish disc media from NAND/memory-card state |
| RPCS3 | Profile/executable binding | Direct PS3 content path; mounted/archive paths refused | Firmware readiness can be unknown and block | Virtual filesystem, dev_hdd, saves/configs; high state complexity | Contract useful for explicit installed-content directory and state warnings |
| PPSSPP | Profile/executable binding | Current real path only direct PSP `.iso`; CSO/CHD/PBP not claimed | No separate firmware in current readiness projection | `memstick`, saves, states, textures, config are separate profile paths | Contract should encode narrow capability instead of overclaiming formats |
| xemu | Profile/executable, xemu-specific launch binding | Direct Xbox disc image with `-dvd_path` | MCPX/flash BIOS/EEPROM/HDD evidence | xemu documents saves in virtual HDD and snapshot behavior; scratch policy matters | Strong case for typed firmware plus HDD state and `-config_path` |
| FS-UAE | Eligible profile and safe executable, explicit config | HDF/ADF-style direct media as adapter allows | Kickstart/TOS-like Amiga firmware readiness and explicit config | Profile state and save-state paths are emulator-specific | Contract should support required config and firmware, not genericize Amiga semantics |
| Hatari | Profile/executable and machine/media command | Atari disk/tape/media formats per command module | TOS/firmware readiness | Emulator state/config needs adapter policy | Mostly common executable/media/CWD fields; firmware remains local |
| Atari800 | Adapter executable and Atari media binding | Cartridge/disk/tape variants | Machine/OS ROM requirements where applicable | Emulator config/state varies | Typed media capability useful; do not flatten formats |
| Caprice32 | Profile/executable and CPC media | Disk/tape/media input | Firmware/config varies by profile | State/config adapter-specific | Contract can describe media and config requirement only |
| MAME | MAME executable/profile and set-compatible target | Arcade set/dependency closure, not a single game path | BIOS/device/parent/CHD closure and MAME compatibility evidence | NVRAM, cfg, inp, sta and artwork paths may be writable | Contract references arcade compatibility result and state policy; no ROM filename abstraction |
| FBNeo | FBNeo executable/core/profile | FBNeo DAT/set-compatible ROM closure | Core/build and set evidence | RetroArch or standalone state depends on host | Keep FBNeo compatibility separate from MAME; contract only carries selected result |
| Flycast | Profile/executable | Dreamcast/GD/CD topology projection and supported direct format | BIOS/profile evidence | VMU/save/config state | Good topology-plus-firmware case; do not make recipe parse GDI |
| Amiberry / WHDLoad | Profile/executable/config/slave selection | Verified WHDLoad target and selected slave | Kickstart/profile evidence | Amiga config/save/state semantics | Contract may carry verified target and profile, but selection remains adapter-specific |
| DOSBox / ScummVM | Executable/profile/config or game ID | Directory/game-folder or executable-style media | Config/engine requirements | Config, save directory, runtime state | Shows that “media path” can be a directory or an identity key, not always a file |

The main finding is that the shared fields are real, but the shared *rules*
are few. A contract should make differences explicit rather than force every
adapter through a universal “ROM path + BIOS path” structure.

## 15. RetroArch case study

RetroArch should be the first contract projection because its existing planner
already has a clear separation:

```text
RetroArch profile
    + selected core
    + core/platform compatibility
    + system/BIOS directory evidence
    + resolved content path
    -> typed RetroArchCommand
```

The contract would describe that the adapter requires a profile, a core whose
`.info` evidence matches the platform, an executable binding, a system
directory policy, and a content projection. It would not select a core, parse
BIOS requirements, or bypass the existing candidate planner.

RetroArch's official CLI documents `retroarch -L core.so game.rom`, a Flatpak
form, verbose logging, and explicit `--config` support.[^1] Its directory
documentation separately identifies system/BIOS and save/state directories,[^2]
while override documentation defines core, content-directory, and game
settings layers.[^3] These are exactly the kinds of distinct bindings a recipe
should expose in a debug view.

## 16. Arcade boundary

MAME and FBNeo must not be reduced to a concrete content filename. Their
recipe input should be a resolved emulator-specific target containing:

- arcade set identity;
- emulator/build/profile identity;
- compatibility result from the existing MAME/FBNeo subsystem;
- BIOS/device/parent/CHD dependency closure;
- ROM search path/profile binding;
- state-directory policy.

The recipe can say “selected MAME set `X` with verified dependency closure” or
“FBNeo set target `Y` under core/build `Z`.” It must not infer compatibility
from a MAME result for FBNeo, and it must not re-run DAT/version logic in a
generic contract layer.

## 17. Managed installs

Managed installation provenance should resolve the selected installation by a
stable installation ID and executable role. The recipe should record:

- installation type (managed, native, AppImage, Flatpak, wrapper, Wine);
- provenance/reference ID;
- selected version/build if already proven;
- executable role, such as `retroarch`, `core`, `pcsx2`, or `xemu`;
- exact resolved path or structured launcher tuple at preflight;
- the evidence generation used to make the selection.

If the managed install changes between planning and launch, preflight must
re-resolve it and either produce a new valid command through the existing plan
or fail closed. A recipe must never retain a stale path merely because its
display name still matches.

## 18. Security model

The contract design must defend against:

- shell injection: no shell grammar or concatenated command strings;
- path traversal: all temporary outputs below owned roots, with canonical or
  nearest-existing-ancestor checks as appropriate;
- symlink escape: reject or revalidate symlinks for source, executable, and
  temporary resources according to the existing safe-read rules;
- untrusted filenames: pass as inert argv values, never interpolate into shell
  text or environment variable names;
- arbitrary executable selection: use discovered/managed/provenance-bound
  executable roles;
- untrusted configs: treat paths and metadata as evidence, not instructions;
- cleanup deletion: remove only session-owned resources with identity and
  ownership checks;
- stale plans: refresh identity, profile, executable, media, and firmware facts
  immediately before spawn.

Recipes are data. They are not executable scripts, downloaded instructions, or
an authorization to mutate anything.

## 19. Extensibility and versioning

### Recommended model: typed built-ins plus constrained data

Three options were considered:

| Option | Benefit | Risk | Decision |
|---|---|---|---|
| Typed Rust adapters only | Strongest safety and compile-time review | More code for each adapter | Appropriate for the first implementation |
| User/developer TOML/JSON recipes | Easy extension and inspection | Arbitrary executable/template/config injection; version drift | Reject for launch authorization |
| Typed built-ins plus constrained data-driven metadata | Reuses common vocabulary while preserving reviewed adapter behavior | Requires a small schema/version discipline | Recommended later |

Data-driven fields should be limited to reviewed literals, enum selections,
platform mappings, version ranges, and named argument roles. A recipe file must
not define an arbitrary executable path, shell template, cleanup command,
environment variable set, or post-launch script without a separate trusted
installation/adapter review boundary.

Every contract should carry a small schema version and an applicability
predicate:

```text
contract_version: 1
platforms: reviewed platform IDs
emulator_version: optional conservative range
install_kinds: reviewed set
host_os: reviewed set
```

Do not initially model a full SAT solver for version compatibility. A missing
or unproven version match should produce an existing unknown/unsupported
outcome, not a guessed compatibility result.

## 20. Debugging UX

The useful user-facing projection is:

> EmuWiz plans to launch this game with:

- emulator and selected profile/install;
- executable or structured launcher;
- core, if applicable;
- firmware/BIOS requirement and evidence state;
- media topology and selected launch projection;
- writable state targets and whether they are persistent, copied, or
  temporary;
- configuration mode and profile path role;
- working directory policy;
- environment requirements by name, without exposing secrets;
- temporary projections and cleanup promise;
- blockers/warnings from the existing readiness model.

Show argv as a structured argument list, not a raw shell command. Redact or
omit secret-like environment values. Make it clear which fields are proven,
which are selected preferences, and which are adapter capabilities.

## 21. Comparable projects

### RetroArch

RetroArch demonstrates explicit core/content/config/system-directory concerns.
The official CLI uses a compact executable/core/content model and supports
explicit config selection.[^1] Its directory and override documentation show
that save state, BIOS, core, content-directory, and game settings are separate
concerns.[^2][^3] EmuWiz should borrow the separation, not expose raw config
editing as a recipe language.

### ES-DE / EmulationStation-style systems

The established frontend pattern is a per-system launch declaration with
platform associations and placeholders. It is useful for a presentation layer
and system defaults, but a string command is not sufficient for EmuWiz's
evidence, writable-state, or cleanup guarantees. EmuWiz should keep typed
adapter commands underneath any future export/import view.

### Pegasus

Pegasus metadata supports collection-level launch commands, a working directory,
per-game overrides, and multiple files for one game.[^4][^5] This is a useful
UX lesson: a game can have multiple launchable files and a collection can have
defaults. It is not a sufficient backend model for EmuWiz because its launch
placeholder is replaced into a command string and the documentation explicitly
allows custom launch commands.

### LaunchBox

LaunchBox exposes built-in command-line variables across emulator settings and
game-specific overrides.[^6] This demonstrates the demand for transparent
parameter substitution, but EmuWiz should expose typed argument roles and
rendered debug data instead of accepting unrestricted substitutions.

### Lutris

Lutris separates game, system, and runner sections; supports runner-specific
configuration, environment variables, game IDs, required base games, and
extensions.[^7] Those distinctions map well to EmuWiz's conceptual separation
between emulator identity, environment, and derived content. Its YAML and
variable-substitution model also illustrates why untrusted declarative launch
files need a trust boundary: an installer/configuration system is much broader
than a safe launch recipe.

### Batocera, EmuDeck, RetroDECK, Heroic, and runner wrappers

These ecosystems commonly solve portability and convenience through wrapper
scripts, generated configuration, container/Flatpak paths, or environment
presets. The useful concepts are runner selection, profile isolation, and
session-specific environment. The parts EmuWiz should not copy are opaque
wrapper chains, implicit mutable global configuration, and shell command
templates whose safety is inferred from display text.

## 22. Final decisions

### 1. Is the abstraction justified?

Yes, narrowly. Repeated executable/profile/media/config/state/environment/CWD
fields are enough to justify a shared contract vocabulary. The abstraction is
not justified as a universal policy engine or a replacement for adapter
modules.

### 2. How is it different from `LaunchPlan`?

`LaunchPlan` answers which candidates exist, which one is selected, and whether
each is ready. A contract answers what a selected adapter requires and how a
resolved launch is shaped. The plan remains authoritative; the recipe is a
typed detail produced from it.

### 3. Which fields are universal?

Adapter/install identity, executable binding, media binding, firmware evidence
reference, config mode, writable-state binding, environment policy, working
directory policy, structured argv, preflight requirements, temporary-resource
ownership, cleanup policy, and contract version.

### 4. Which quirks remain adapter-specific?

Exact flags, core/set selection, firmware filenames and semantics, profile
formats, media extensions/topology restrictions, state reconciliation, process
trees, and cleanup behavior.

### 5. Rust, declarative data, or hybrid?

Start with compiled Rust typed contracts. Later, use constrained reviewed data
for literals, platform aliases, version ranges, and argument-role metadata.
Do not accept arbitrary user-authored executable/template recipes by default.

### 6. How represent writable state?

Typed targets with state kind, owner, source-write policy, persistence policy,
and Vault visibility. Source media must be explicitly non-writable.

### 7. How represent scratch copies?

As a session lifecycle policy with source identity, copy kind, owned destination,
verification, and discard/persist/reconcile outcome. Never as a casual boolean
that silently changes source semantics.

### 8. How resolve managed installs?

By stable installation/provenance ID and executable role, resolving the exact
path or launcher tuple at preflight and refusing stale or ambiguous results.

### 9. How expose debugging safely?

Show structured fields and separately rendered argv atoms, with provenance,
state ownership, redaction, and explicit blockers. Do not show an editable
shell command as the authority.

### 10. Smallest implementation slice

Define vocabulary types and a read-only projection trait, then project the
existing RetroArch command plan plus two native adapters without changing
selection, readiness, firmware, topology, Save Vault, or process behavior.

## 23. Recommended roadmap

| Phase | Scope | Outcome | Boundary |
|---|---|---|---|
| L0 | Vocabulary only | Typed contract/recipe concepts and invariants | No runtime behavior; no GUI |
| L1 | RetroArch + two native adapters | Read-only projection from existing plans | Existing planner/command modules remain authorities |
| L2 | Writable-state and scratch bindings | Pre-launch protection facts and owned temporary-resource descriptions | No Save Vault backup/restore; no automatic reconciliation |
| L3 | Managed-install resolution | Stable install IDs and preflight re-resolution | No hardcoded version paths |
| L4 | Inspection/debug view | Human-readable structured launch explanation | No launch/edit controls implied |
| L5 | Additional adapter migration | Incremental projection for PCSX2, Dolphin, xemu, RPCS3, arcade, etc. | Migrate only where semantics are proven |

## 24. Explicit do-not-build list

- A second launch planner.
- A recipe-owned readiness enum or duplicate blocker system.
- Arbitrary shell-script recipes.
- User-controlled raw executable and argument templates by default.
- Commands sourced from ROM names, metadata, downloaded manifests, or archive
  contents.
- Duplicate BIOS/firmware detection or hashes in recipe definitions.
- Duplicate media topology or multi-disc grouping logic.
- Recipe-owned Save Vault backup/restore/retention policy.
- Automatic config mutation merely because a contract requests a setting.
- Automatic source-media mutation or unverified scratch-to-source copyback.
- A universal assumption that every launch is one file plus one executable.
- Automatic process killing or broad recursive cleanup without an explicit,
  owned session lifecycle.
- Treating a MAME-compatible target as FBNeo-compatible, or vice versa.
- Making Sunshine/Moonlight assumptions universal to all emulator contracts.

## Sources

### Local EmuWiz source inspected

- `crates/archivefs-core/src/launch/mod.rs`
- `crates/archivefs-core/src/launch/planning.rs`
- `crates/archivefs-core/src/launch/readiness.rs`
- `crates/archivefs-core/src/launch/platform_map.rs`
- `crates/archivefs-core/src/launch/input_projection.rs`
- `crates/archivefs-core/src/launch/topology.rs`
- `crates/archivefs-core/src/launch/retroarch_command.rs`
- `crates/archivefs-core/src/launch/execution.rs`
- `crates/archivefs-core/src/launch/process_spawn.rs`
- `crates/archivefs-core/src/launch/pcsx2_command.rs`
- `crates/archivefs-core/src/launch/duckstation_command.rs`
- `crates/archivefs-core/src/launch/dolphin_command.rs`
- `crates/archivefs-core/src/launch/rpcs3_command.rs`
- `crates/archivefs-core/src/launch/ppsspp_command.rs`
- `crates/archivefs-core/src/launch/xemu_command.rs`
- `crates/archivefs-core/src/launch/fsuae_command.rs`
- `crates/archivefs-core/src/launch/hatari_command.rs`
- `crates/archivefs-core/src/launch/mame_command.rs`
- `crates/archivefs-core/src/launch/fbneo_command.rs`
- `crates/archivefs-core/src/bios_projection.rs`
- `crates/archivefs-core/src/emulator_environment/retroarch.rs`
- `crates/archivefs-core/src/emulator_environment/mame.rs`
- `crates/archivefs-core/src/emulator_environment/fbneo.rs`
- `crates/archivefs-core/src/media_set/model.rs`
- `crates/archivefs-core/src/media_set/engine.rs`
- `crates/archivefs-core/src/media_set/plan.rs`
- `crates/archivefs-core/src/patch_manager/resolved_emulator_profile.rs`
- `crates/archivefs-core/src/ready_to_play.rs`

### External sources

[^1]: Libretro, “Command-Line Interface (CLI),” RetroArch documentation,
      https://docs.libretro.com/guides/cli-intro/ .
[^2]: Libretro, “Directory Configuration,” RetroArch documentation,
      https://docs.libretro.com/guides/change-directories/ .
[^3]: Libretro, “Using Content, Folder, and Core Overrides for Custom Settings,”
      https://docs.libretro.com/guides/overrides/ .
[^4]: Pegasus Frontend, “Metadata files,”
      https://pegasus-frontend.org/docs/user-guide/meta-files/ .
[^5]: Pegasus Frontend, “How to add games,”
      https://pegasus-frontend.org/docs/user-guide/adding-games/ .
[^6]: LaunchBox, “Built-in Command Line Variables,”
      https://launchbox.featurebase.app/help/articles/7078334-launchbox-built-in-command-line-variables .
[^7]: Lutris, “Writing installers,” including game/system/runner configuration,
      environment, requirements, and extensions,
      https://github.com/lutris/lutris/blob/master/docs/installers.rst .
[^8]: xemu, “Command Line Arguments,” https://xemu.app/docs/cli/ .
[^9]: xemu, “FAQ,” including portable config, EEPROM, HDD save, and snapshot
      behavior, https://xemu.app/docs/faq/ .
