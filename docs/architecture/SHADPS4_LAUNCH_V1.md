# shadPS4 native PS4 launch adapter V1

Base: 6d22a2b0d333eb9055278f701632f0a40850c261. Linux backend only;
no PS5 platform, GUI redesign, downloading, installation or emulator-configuration writer.

## Upstream contract checked on 2026-10-07

- [CLI](https://github.com/shadps4-emu/shadPS4/blob/main/src/main.cpp):
  `-g <executable-path>` and optional `--fullscreen true|false`.
  No guessed version flag, shell, title-ID search, debugger or installer flags.
- [User paths](https://github.com/shadps4-emu/shadPS4/blob/main/src/common/path_util.cpp):
  an existing `cwd/user` takes precedence over `XDG_DATA_HOME/shadPS4`, otherwise
  `HOME/.local/share/shadPS4`. Initialization creates directories before main,
  so even a help/version probe is not read-only. This adapter never probes.
- [Settings](https://github.com/shadps4-emu/shadPS4/blob/main/src/core/emulator_settings.cpp):
  current `config.json`; legacy `config.toml`. JSON takes precedence. Settings
  include General/sys_modules_dir (legacy sysModulesPath) and GPU/full_screen
  (legacy Fullscreen). Configuration is inspected with a 1 MiB read bound.
- [Executable loader](https://github.com/shadps4-emu/shadPS4/blob/main/src/core/loader/elf.cpp)
  and [structures](https://github.com/shadps4-emu/shadPS4/blob/main/src/core/loader/elf.h):
  PS4 ELF64, little endian, FreeBSD ABI, x86-64, SCE executable types. V1 also
  recognizes SELF wrapping that ELF only with bounded segment tables and no
  encrypted/compressed segments; neither is a body-integrity verifier.
- [Sysmodules](https://github.com/shadps4-emu/shadPS4/wiki/I.-Quick-start-%5BUsers%5D):
  requirements depend on the game. There is no invented universal firmware list.

## Backend workflow

1. `discover_shadps4` uses existing bounded executable/PATH discovery and the
   known AppImage location scanner. Only explicit paths and fixed locations are
   considered, without running files or crawling games. No generic Auto-Resolver
   exists on this base; this function is its future adapter seam. A configured
   path marks explicit user selection; PATH/filename discovery does not.
2. `inspect_shadps4_profile` checks the existing native executable policy,
   ELF header and static shadPS4 core CLI marker. AppImages require type-2 ELF
   container recognition AND explicit selection of a trusted core AppImage:
   their name/header cannot establish inner emulator identity. Evidence is typed;
   none authenticates the publisher. Version is honestly unknown. A renamed
   native core is accepted by its marker, not its filename. QtLauncher and shell
   wrappers without that core contract are refused.
3. The caller supplies an absolute working directory and XDG data root (existing
   `KnownInstallRoots` supplies the latter). This binds portable/XDG precedence;
   the child receives XDG_DATA_HOME through existing per-child environment support,
   without changing the parent. Existing config/user roots are required. Arbitrary
   config-root overrides and relative sysmodule paths are unsupported. Unsafe
   symlinked user roots/configs are refused, not silently followed.
4. `inspect_shadps4_game` accepts an extracted root or its exact `eboot.bin`,
   requiring the existing bounded, symlink-safe PS4 SFO identity. CUSA TITLE_ID
   is authoritative; CONTENT_ID and SFO provenance are retained separately.
   PKG/ISO/archives, arbitrary ELF paths, non-PS4 executables and update/DLC
   CATEGORY values are refused. Missing CATEGORY is not promoted to identity.
   No game installation, decryption or asset-tree enumeration occurs.
5. Existing launch evidence now routes verified PS4 TITLE_ID to canonical PS4
   identity. Game identity parsing/rules are unchanged. The platform map registers
   shadps4 only for PS4. `DiscoveredStandaloneProfile::ShadPs4` and
   `launch_profile_input()` integrate with the existing pure standalone planner.
   The adapter's sealed plan additionally requires the canonical PS4 key to
   match the inspected source; Unknown/Conflicting/filename-only never authorize.
6. `plan_shadps4_launch` returns a typed LaunchTarget, LaunchCommandSpec preview,
   profile, source identity/provenance, executable SHA-256 and warnings. Readiness
   is ReadyWithWarnings. Typed refusals distinguish missing/not-runnable/unconfirmed
   executables, unsupported layout, identity, unavailable configuration, missing
   declared sysmodules, changed sources and spawn failure. Required sysmodules
   are supplied explicitly as an enum; bound file presence never proves firmware
   authenticity or compatibility. No requirements supplied means unknown game
   dependencies, not proof that none are needed.
7. `preflight_shadps4_launch` rechecks/rebuilds before `launch_shadps4` uses the
   existing process_spawn watcher. Preflight VerifiedReady refers ONLY to the
   verified binding: not game compatibility, all-assets integrity or host Vulkan/
   CPU capability. Native/AppImage argv is direct, with no extraction wrapper.
   PID, bounded stderr, running state and exit result reuse WatchedProcess.
   That infrastructure has no stop/cancel method; V1 adds none.

## Bounds and freshness

Executables/boot files/declared modules are fingerprinted up to 1 GiB each using
64 KiB streaming hash buffers. The existing ExecutableBinding is retained; the
additional full SHA-256 binding covers AppImages beyond its 128 MiB hash threshold.
PS4 headers read at most 64 KiB; SELF/ELF program tables have fixed count limits.
Only SFO/config data is held in full, at their existing/explicit 1 MiB limits.

Source executable, SFO, emulator executable, selected global config, required
module files and optional title JSON/TOML overrides are checked for drift. New
portable roots, JSON superseding TOML, or appearance of title overrides invalidate
preview. A metadata-preserving executable edit is caught by SHA-256. Replaced game
roots/SFO paths must still satisfy the existing no-symlink PS4 policy.

The adapter performs no source/config writes. An authorized emulator process can
write its own user/config/log state and apply existing patches/DLC/per-game config;
those are disclosed in preview, not interpreted as unchanged source identity.
As with the shared path-based spawn infrastructure, the final check and exec are
not atomic. V1 does not hash the entire game tree, isolate runtime game writes,
validate all settings, verify sysmodule contents or claim the game boots.

## GUI V1 completion

Backend promoted on 8061b04c53713d684cbfe5ba757821fbab54e1d0, replayed
onto the generic Auto-Resolver base adbb161d. GUI completion is a separate
candidate and does not change the backend's game identity or launch rules.

Emulator Setup derives the PS4/shadPS4 row from the shared platform map and
uses existing executable-override persistence. Open a verified PS4 game and
its Launch details to inspect the executable/profile/config pair, TITLE_ID,
separate CONTENT_ID, exact game root/eboot.bin and structured argv. Selection
uses the promoted resolver's AUTO/PREFERRED/FORCED modes and atomic selection
persistence. Overrides are discovery evidence, not a second selection engine.
AppImages require explicit consent; discovery/filename evidence never supplies
publisher authentication. Preferred bypasses and forced refusals remain visible.

The focused `emulator_setup/shadps4.rs` GUI controller retains one background
worker and a single latest selection. Its generation-tagged, one-slot reply
cannot publish a preview for another game. Metadata drift invalidates visible
previews and triggers a fresh inspection; Recheck refreshes discovery/config
precedence and dependency observations. Metadata-preserving edits cannot be
noticed without reading bytes: every authorized launch therefore re-resolves
the exact pair and uses the backend's SHA-256/source/config preflight before
spawn. No filesystem scan or hashing occurs in rendering. The GUI then uses
`StandaloneLaunchRequest`, existing watched-process state and normal polling;
it adds no shell, detached wrapper, Stop/Kill control or compatibility promise.

shadPS4 always remains ReadyWithWarnings when a sealed plan exists. Only this
adapter's bound request can cross that warning state into launch; other GUI
adapter policies are unchanged. Unsupported package/archive/image inputs,
missing configuration, unverified/mismatched SFO identity and stale bindings
have typed refusals. Global Doctor checks do not guess shadPS4 readiness or run
a version probe; the existing inventory probe explicitly refuses shadPS4.
Only EmuWiz's override/selection files are written by explicit UI controls;
inspection and tests do not write emulator configuration or game files.

Deferred: trusted version metadata, Flatpak/macOS/Windows backends, host/FUSE/
Vulkan checks, broader SELF variants, package/installed-ID/archive launching,
exhaustive game-specific dependencies and stop support in the common supervisor.
Existing provider/download machinery is unchanged except exhaustive enum wiring;
no new network or download action is introduced.
