# shadPS4 native PS4 launch adapter V1

Base: 6d22a2b0d333eb9055278f701632f0a40850c261. Linux backend only;
no PS5 platform, GUI redesign, downloading, installation or configuration writer.

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

## Exact GUI follow-up

Backend seams are complete; no GUI files were changed. Integrate discovered
profiles into `crates/archivefs-gui/src/emulator_setup/controller.rs`, then add the
sealed shadPS4 plan/request/process route in `launch_readiness_page.rs`
(`StandaloneLaunchRequest`, preview selection/authorization and tracked launch
state). Native-v2 consumes that existing state through `selected_game_readiness.rs`.
AppImage explicit-selection evidence and dependency warnings need to be surfaced
there; do not replace them with filename-based automatic eligibility.

Deferred: GUI wiring, portable version metadata with trustworthy provenance,
Flatpak/macOS/Windows backends, host/FUSE/Vulkan checks, broader SELF variants,
package/installed-ID/archive launching, exhaustive dependencies and stop support
in the common supervisor. Existing provider/download machinery is untouched.
