# Emulator Lifecycle Management

> **Recovered historical research — status against current main (`b66422c2`).**
> Source: branch `research/emulator-lifecycle` at `2c3ce989`. Recovered unchanged below this block except where marked `[refreshed]`.
> - **Research only; no code was recovered or changed.** Written against main `41c9a645` (2026-09-21); current code has moved on (see [`emulator_lifecycle.rs`](../../crates/archivefs-core/src/emulator_lifecycle.rs), [`emulator_inventory.rs`](../../crates/archivefs-core/src/emulator_inventory.rs) and `managed_appimage_bootstrap/`). Its statement that no provider-neutral health projection exists was **not re-verified** against current main; check before relying on it.
> - **Principle that still holds:** detect broadly, prove narrowly, and automate only with explicit ownership.
> - **Ownership/update authority by install type:** a *system package* and a *Flatpak* belong to their package managers (EmuWiz reports, never updates); an *AppImage* or other *user-managed install* is reportable but not EmuWiz-owned; only an *EmuWiz-managed* AppImage inside an EmuWiz-owned root, with exact artifact digest, durable manifest, running-process exclusion and retained rollback, may be updated by EmuWiz.
> - A familiar filename, write permission or a successful `--version` probe never establishes ownership.


- **Status:** research only; no production Rust or GUI changes.
- **Inspection date:** 2026-09-21
- **Authority:** `/home/davedap/emuwiz-main-release-fix` at
  `41c9a645464a3fec9fa68cad7de78cfadd19cf12`
- **Research branch:** `research/emulator-lifecycle`

## Executive conclusion

EmuWiz already has the beginnings of a safe lifecycle boundary. It can perform
bounded executable inventory and `--version` probing, classify installation
types, inspect managed-install manifests, resolve official release metadata,
stage and verify selected AppImages, and execute a narrowly eligible staged
update with a rollback file. It does not yet have one provider-neutral health
projection or a complete discovery/update provider for every native adapter.

The safe future rule is:

> Detect broadly, prove narrowly, and only automate an installation when its
> ownership and update authority are explicit.

System packages and Flatpaks belong to their package managers. An existing
manual binary or AppImage is reportable but not EmuWiz-owned. EmuWiz-managed
AppImages can be updated only inside an EmuWiz-owned root with an exact
artifact digest, durable manifest, running-process exclusion, and retained
rollback. No lifecycle action should infer ownership from a familiar filename,
write permission, or a successful version probe.

## Sources and method

Primary project sources were checked live where available. Important sources
include:

- [Libretro GNU/Linux installation guide](https://docs.libretro.com/guides/install-gnu/)
  and [RetroArch Linux instructions](https://retroarch.com/index.php?page=linux-instructions)
  for Flatpak, distro packages, Snap, PPA, source, update ownership, and core
  distribution.
- [PCSX2 running documentation](https://pcsx2.net/docs/setup/running/) and
  [PCSX2 Flathub packaging](https://github.com/flathub/net.pcsx2.PCSX2) for
  official Linux AppImage/Flatpak support and sandboxed data paths.
- [PPSSPP introduction](https://www.ppsspp.org/docs/getting-started/introduction/)
  and [download page](https://dev.ppsspp.org/download/) for Linux AppImage and
  Flatpak availability.
- [Dolphin downloads](https://dolphin-emu.org/download/) for official Linux
  Flatpak, release cadence, and source builds.
- [Flycast upstream](https://github.com/flyinghead/flycast) for tagged builds,
  master/nightly channels, and the official Flathub application ID.
- [xemu Linux downloads](https://xemu.app/docs/download/) for AppImage,
  Flatpak, Ubuntu PPA, source, and sandbox data behavior.
- [Vita3K downloads](https://vita3k.org/download) and
  [Vita3K FAQ](https://vita3k.org/faq) for Linux AppImage, XDG paths, and
  release artifacts.
- [FS-UAE Linux downloads](https://fs-uae.net/download/linux/) and
  [paths documentation](https://fs-uae.net/docs/paths-and-directories/) for
  versioned tar archives, official distro repositories, portable mode, and
  data directories.
- [Hatari download page](https://www.hatari-emu.org/download.html) and its
  [current Unix man page](https://github.com/hatari/hatari/blob/main/doc/hatari.1)
  for source/package distribution and configuration/TOS paths.
- [Cemu official site](https://cemu.info/) and
  [official action artifacts](https://cemu.info/ActionBuilds.php) for Linux
  AppImage/zip artifacts and the distinction between releases and untested
  action builds.
- [MAME installation documentation](https://docs.mamedev.org/initialsetup/installingmame.html)
  and [command-line documentation](https://github.com/mamedev/mame/blob/master/docs/source/commandline/commandline-all.rst)
  for Linux distro packages/source builds and `~/.mame` path semantics.
- [Ryujinx's current project notice](https://github.com/Ryujinx) confirming
  that the original project is discontinued and should not be treated as a
  current official update provider.

The source audit also inspected existing EmuWiz files, especially:

- `crates/archivefs-core/src/emulator_inventory.rs`
- `crates/archivefs-core/src/emulator_update.rs`
- `crates/archivefs-core/src/emulator_download.rs`
- `crates/archivefs-core/src/managed_emulator_install.rs`
- `crates/archivefs-core/src/launch/readiness.rs`
- `crates/archivefs-core/src/emulator_environment/`
- native command/profile/readiness modules under `crates/archivefs-core/src/launch/`
  and `patch_manager/`
- `docs/research/MANAGED_EMULATOR_INSTALL_PROVENANCE_AUDIT.md`
- `docs/research/NATIVE_EMULATOR_ADAPTER_COVERAGE_AUDIT.md`

## Current EmuWiz capability

### Inventory and discovery

`emulator_inventory.rs` is a bounded, read-only inventory projection. Current
convenience scanning covers Dolphin, RPCS3, PCSX2, PPSSPP, DuckStation, and
xemu. It checks a bounded set of `PATH` names and probes bounded `--version`
output; it records the exact executable path, root, parsed version, confidence,
source, channel, installation type, update capability, preference, save-state
risk, and warnings. It does not scan the whole filesystem and it does not
establish ownership.

The adapter-specific discovery layer is richer than the generic inventory:
native profile/readiness modules exist for the mature adapters listed in the
coverage audit, including RetroArch, PCSX2, DuckStation, RPCS3, PPSSPP,
Dolphin, Flycast, Hatari, FS-UAE, xemu, Xenia, Cemu, Vita3K, MAME/FBNeo,
Amiberry, VICE, RMG, Stella, ScummVM, DOSBox, melonDS/DeSmuME, mGBA,
SameBoy, Mesen, Snes9x, and others. These modules are launch/readiness
evidence, not a universal lifecycle manager.

`emulator_environment` currently has deliberately read-only environment
discovery for RetroArch, MAME, and FBNeo. It has no generic
`EmulatorEnvironmentAdapter` contract, so lifecycle work should not create a
second profile discovery vocabulary.

### Version health and update primitives

`emulator_update.rs` already models `UpdateStatus` values such as
`UpToDate`, `UpdateAvailable`, `VersionUnknown`, `LatestUnknown`,
`ChannelMismatch`, `ComparisonUnsupported`, and `Offline`. Metadata results
carry a source/provenance and cache timestamp. Remote metadata is bounded and
failure is represented as unknown/offline rather than stale certainty.

Execution is intentionally narrower. A staged update requires a known version,
a non-running emulator, a still-matching target hash, supported installation
type/capability, HTTPS provenance, and a published SHA-256 for a verified
artifact. The implementation supports an AppImage/portable/managed lane;
system-package and Flatpak installations remain external. The update journal
has staged/published/failed/rolled-back/reconciliation states, but this is not
yet a provider-neutral lifecycle health model.

### Managed AppImage primitives

`emulator_download.rs` has a fixed official catalogue and deterministic
GitHub-release asset selection for a small set. It rejects non-HTTPS downloads,
bounds response size and redirects, validates an AppImage/ELF-shaped payload,
supports published SHA-256 verification, stages in the destination directory,
and publishes through an atomic replacement. It never accepts a remote URL as
an arbitrary destination or runs a shell installer.

`managed_emulator_install.rs` adds the stronger ownership boundary: an
EmuWiz-owned root, side-by-side install records, a manifest containing artifact
and executable hashes, current/previous lineage, source provenance, and
health checks for missing or changed managed binaries. Detection of an
external AppImage does not create ownership. This is the right foundation for
future lifecycle management; it should not be bypassed by a new adapter.

## Provider-neutral future health model

The following model is recommended for a future projection only; it is not
implemented by this research branch:

| State | Meaning | Network required? | Default action |
|---|---|---:|---|
| `InstalledCurrent` | Exact executable and version are known; selected provider says current. | No, if cached evidence is sufficient | Launch/readiness allowed. |
| `InstalledUpdateAvailable` | Exact installed version and authoritative newer version are known. | Usually yes or fresh cache | Report; update only under provider authority. |
| `InstalledUnknownVersion` | Executable is present but version probe/metadata is not trustworthy. | No | Report and allow launch if readiness passes; never automate update. |
| `InstalledUnsupportedVersion` | Version is known but outside the adapter/provider's supported range. | No | Warn/block only where launch compatibility requires it. |
| `Missing` | No eligible installation found. | No | Offer documented acquisition route, not arbitrary installation. |
| `Broken` | Manifest, executable, permissions, required profile, or provider state is inconsistent. | No | Needs attention; fail closed for mutation. |
| `MultipleInstallations` | More than one viable installation exists and no explicit choice is recorded. | No | Ask user; never silently switch. |
| `ManagedExternally` | Flatpak, system package, distro PPA, or other external authority owns updates. | No | Explain authority and hand off. |

An update check should also retain a separate result such as
`UpdateAvailabilityUnknown`/`Offline`; it must not collapse “could not ask the
provider” into “out of date”.

## Trust and automation categories

| Category | Meaning | Examples | EmuWiz action |
|---|---|---|---|
| `OfficialManaged` | EmuWiz owns a confined install, verified artifact, and manifest. | EmuWiz-managed AppImage; future versioned portable tree. | Safe to plan/update after revalidation and confirmation. |
| `OfficialBrowserHandoff` | The official project publishes a route but EmuWiz cannot prove or safely automate the install. | Official Cemu zip page, FS-UAE archive page, Hatari source/package page. | Open official page; never download/execute silently. |
| `FlatpakManaged` | Flatpak remote/application ID is authoritative. | `org.libretro.RetroArch`, `org.DolphinEmu.dolphin-emu`, `org.flycast.Flycast`, `app.xemu.xemu`. | Report and hand off to Flatpak; do not replace payloads. |
| `SystemPackageManaged` | Distribution package manager/official PPA owns the binary. | `apt`, `dnf`, `pacman`, xemu PPA, RetroArch PPA. | Report and hand off to the OS/package manager. |
| `PortableUserManaged` | User owns an archive/AppImage/manual binary outside EmuWiz's managed root. | Downloaded AppImage, extracted FS-UAE/Cemu/MAME tree. | Detect, validate, and report; update only after explicit adoption. |
| `Unknown` | Evidence does not establish origin or authority. | Wrapper, stale symlink, copied binary, ambiguous package path. | Version/readiness only; no mutation. |
| `DoNotAutomate` | No stable official Linux provider or safe lifecycle contract exists. | Original Ryujinx; Xenia Linux in current upstream state. | Do not offer install/update automation. |

Official project sources outrank community mirrors. GitHub Releases is an
official source when the upstream project owns the repository and publishes
the artifact there; a random GitHub fork, third-party PPA, or download site is
not equivalent. A community Flatpak can be useful for launch detection, but it
must not be represented as an official managed install unless the upstream
project says so.

## Linux channel matrix

The matrix records practical Linux channels as of the inspection date. “No”
means no authoritative upstream route was identified, not that a community
package cannot exist. Config paths are defaults/evidence locations, not a
permission to create or migrate them.

| Emulator | Install channels | Version detection | Update authority | Safe automation | Config/data location | Rollback / notes |
|---|---|---|---|---|---|---|
| RetroArch | Flatpak; distro package; official PPA/Snap; source; some official/Libretro binary routes | `--version`/inventory; package/Flatpak metadata | Flatpak, distro, PPA/Snap, or user-selected binary route | `FlatpakManaged` or `SystemPackageManaged`; managed binary only if separately proven | Native usually `~/.config/retroarch`; Flatpak under `~/.var/app/org.libretro.RetroArch/`; cores and system/BIOS may be separate | Package manager rollback; managed binary only with manifest. Core updates are a separate authority. |
| PCSX2 | Official AppImage; Flatpak; distro/community packages; source | Version probe plus release metadata; Flatpak/package metadata | AppImage owner or Flatpak/distro | `OfficialManaged` only for EmuWiz-owned AppImage; otherwise handoff | Native `~/.config/PCSX2`; Flatpak data/config under `~/.var/app/net.pcsx2.PCSX2/`; BIOS is user data | AppImage can retain previous verified binary; Flatpak/package manager owns rollback; BIOS/saves must not be copied or removed. |
| DuckStation | Official Linux AppImage/release builds; Flatpak; distro packages; source | Version probe where supported; release/package metadata | Upstream AppImage, Flatpak, or distro | Managed AppImage only after exact artifact proof | XDG config/data commonly under `~/.config/duckstation` and `~/.local/share/duckstation`; Flatpak under app sandbox | Preserve memory cards, saves, BIOS, and shader cache; do not infer paths from executable name alone. |
| RPCS3 | Official Linux AppImage; Flatpak/community package; distro/source builds | Version/build string; package/Flatpak metadata | Official AppImage owner, Flatpak, or distro | Managed AppImage only with artifact digest; external installs report-only | Native commonly `~/.config/rpcs3`; Flatpak under `~/.var/app/net.rpcs3.RPCS3/`; firmware/dev_hdd0/config are user data | Retain previous AppImage; never replace RPCS3 data tree as part of binary update. |
| PPSSPP | Official AppImage; Flatpak; distro packages; source | Version probe/release metadata | Flatpak/distro or AppImage owner | Managed AppImage only when manifest-owned | Native memstick commonly `~/.local/share/ppsspp`; Flatpak under `~/.var/app/org.ppsspp.PPSSPP/` | Keep `PSP/SAVEDATA`, savestates, cheats, shaders, and config; AppImage replacement is separable. |
| Dolphin | Official Flatpak; distro packages; source; project development builds | `--version`/profile evidence; Flatpak/package metadata | Dolphin Flatpak or distro; user-managed development binary | Flatpak/system handoff; portable management only after proof | XDG Dolphin data/config under `~/.local/share/dolphin-emu`/`~/.config/dolphin-emu` depending build; Flatpak sandbox differs | Flatpak/package rollback; never wipe Wii NAND, saves, shader cache, or controller profiles. |
| Flycast | Official GitHub releases; official Flathub ID; distro/source/nightly builds | Version output/release tag; Flatpak metadata | Upstream release, Flatpak, distro, or selected nightly channel | Report-only unless EmuWiz owns the artifact | Common XDG paths under `~/.config/flycast` and `~/.local/share/flycast`; verify profile evidence | Nightly/master/stable are different channels; preserve VMU, BIOS, saves, and shader cache. |
| MAME | Distro packages; source; official Windows binaries only; user-built Linux binaries | `mame -version`; package metadata | Distro package or user/source build | `SystemPackageManaged` or `PortableUserManaged`; no universal updater | `~/.mame` and paths selected by MAME options; ROM/sample/artwork paths are configuration | Versioned portable tree can roll back; preserve `cfg`, `nvram`, `sta`, `inp`, artwork, and ROM paths. |
| FBNeo | Usually RetroArch/libretro core; source/core packages; standalone builds are not a current EmuWiz lifecycle target | RetroArch/core `.info` and package evidence | RetroArch core/package authority | Do not create a separate FBNeo installer; use RetroArch authority | RetroArch config/core/system paths | Core replacement is not an emulator binary update; preserve RetroArch state. |
| Hatari | Distro package; source; official source/release page; some distro binaries | `--version` or package metadata; readiness also needs TOS evidence | Distro or user/source build | `SystemPackageManaged`/browser handoff; no managed updater identified | `~/.config/hatari/hatari.cfg`, NVRAM/snapshot files; TOS image from data dir or explicit path | Portable/versioned tree can roll back; TOS and user NVRAM/config remain external. |
| FS-UAE | Official Linux tar.xz; official distro repositories; source/portable tree | Version command/release filename; package metadata | User-managed archive or distro repository | `PortableUserManaged`; future managed tree possible but not present | `Documents/FS-UAE` or `$HOME/FS-UAE`; base-dir `~/.config/fs-uae/base-dir`; portable mode co-locates data | Versioned sibling tree is rollback-friendly; preserve Kickstarts, configs, saves, logs, and plugins. |
| xemu | Official AppImage; Flatpak; Ubuntu PPA; source | Version output/release metadata; Flatpak/package metadata | AppImage, Flatpak, PPA, or source owner | Managed AppImage if owned; otherwise external handoff | Native XDG data/config; Flatpak writable data is constrained to `~/.var/app/app.xemu.xemu/data/xemu/xemu` unless overridden | Keep HDD/flash/EEPROM and firmware; retain old AppImage only for managed installs. |
| Xenia | Upstream project is primarily Windows-oriented; source and community Linux experiments exist, but no stable official Linux lifecycle lane was established | Only explicit configured binary/profile evidence | Unknown/community | `DoNotAutomate` for Linux install/update | Profile-specific; no safe universal Linux path | Never route a Linux user to an unofficial updater; preserve explicit binary choice. |
| Cemu | Official Linux AppImage/zip artifacts and action builds; source; community packages | Version/build from binary or artifact; action builds are distinct | Upstream release artifact or user-managed archive | Browser handoff for action builds; managed portable/AppImage only after release/digest proof | Portable trees often keep config beside install; newer builds may use XDG; inspect actual profile | Versioned tree swap can roll back; never remove graphic packs, mlc01, keys, or saves. |
| Vita3K | Official Linux AppImage/nightlies; GitHub releases; source; community packages | Version/release tag; artifact SHA where release supplies it | User-selected upstream nightly/release or package | Managed AppImage possible with explicit channel selection; no silent nightly promotion | `~/.config/Vita3K/config.yml`; `~/.local/share/Vita3K/Vita3K` filesystem; `~/.local/share/Vita3K/patch`/`textures`; `~/.cache/Vita3K` logs/cache | Retain previous AppImage; Vita filesystem, installed titles, firmware, patches, and textures are user data. |
| Ryujinx / current Switch adapter | Original Ryujinx discontinued; no current official provider; current EmuWiz native adapter is absent | Existing configured binary only | None authoritative for original project | `DoNotAutomate`; do not revive old updater or mirror | Historical Ryujinx paths are not a current lifecycle contract | Preserve explicit user install; any fork needs a new upstream/provenance audit. |

### Reading the matrix

The same emulator may legitimately appear in several rows at once on one
machine. The row describes the selected installation, not a global emulator
fact. A `--version` result proves what binary ran; it does not prove who owns
the binary or which provider should replace it.

## AppImage lifecycle

### Discovery

Discovery should remain bounded and read-only. The existing RetroArch AppImage
work documents fixed search roots and rejects final-component symlinks for
AppImage candidates. A future generic inventory may reuse that bounded pattern:
configured exact path first, then a small set of documented user roots, then
`PATH` only for native executables. It must not recursively scan `$HOME`,
mount/extract an AppImage, execute an unknown candidate, or treat a filename
alone as identity.

### Validation and version identity

Validate regular-file type, executable bit, architecture/ELF/AppImage shape,
and (where practical) a bounded version probe. Filename normalization should
remove only known suffixes (`.AppImage`, architecture and release decorations)
for display; it must not be used as an update identity. The strong identity is
the local SHA-256 bound to an official release asset, plus the selected
emulator/channel/architecture and the exact path.

An AppImage whose version cannot be queried is `InstalledUnknownVersion`, not
`InstalledCurrent`. An AppImage whose bytes no longer match an EmuWiz manifest
is stale/broken ownership, not an automatically repairable install.

### Safe replacement

For an EmuWiz-owned AppImage:

1. resolve a fixed official metadata provider and exact asset;
2. require a published digest or other trustworthy signature evidence;
3. revalidate that the configured target and current bytes still match the
   review;
4. refuse while the emulator is running;
5. download to a same-directory temporary file with size and timeout bounds;
6. verify bytes, ELF/AppImage structure, executable mode, and version;
7. retain the old verified binary under a versioned rollback name;
8. publish the new binary and manifest atomically;
9. verify the published path/hash/manifest linkage;
10. report the transaction state and leave the old binary until the new install
    has passed a subsequent health check.

No update may rename or overwrite an external AppImage in place merely because
it is writable. Explicit adoption should copy into an EmuWiz-owned root first.
The current managed-install implementation already follows this ownership
direction; a broad universal AppImage updater would be unsafe.

### Update availability

For external AppImages, EmuWiz may compare a known local version against an
official release API and report an available update, but the action is
`OfficialBrowserHandoff` or explicit adoption—not silent replacement. For
managed AppImages, the existing staged update route can be used only when the
manifest, digest, channel, target hash, and provider all agree.

## Package-manager ownership

### Flatpak

Flatpak should own install, update, uninstall, runtime, permissions, and
rollback semantics. EmuWiz may inspect `flatpak list --app`/known application
IDs through a future bounded read-only provider, but should not write inside
`/var/lib/flatpak`, `~/.local/share/flatpak`, or an app sandbox. A launch
binding may be `flatpak run <app-id>` when the profile proves that exact app
ID; it should not invent an executable path inside the deployment.

Config, saves, BIOS, and game-library access differ by sandbox. The setup
surface must disclose the app ID and relevant filesystem permission issue.
“Managed by Flatpak” means “ask Flatpak to update”, not “EmuWiz can replace
the binary”.

### System package / PPA

The distribution package manager owns files under system prefixes, package
metadata, dependencies, desktop entries, and rollback expectations. EmuWiz may
report package/version evidence if a future provider reads it, but must not
run `apt`, `dnf`, `pacman`, a PPA bootstrap, or a privilege escalation from a
normal emulator setup action. PPA ownership should remain visibly distinct
from an Ubuntu/Debian archive package.

### Source/manual/portable

An extracted archive or source build is user-managed unless installed through a
known package manager or adopted into an EmuWiz-managed root. EmuWiz can
validate an explicit path and show a version/readiness result. It should not
replace files in an arbitrary tree because it cannot know whether plugins,
firmware, wrapper scripts, or user data share that tree.

## Multiple installations and explicit choice

The inventory must retain all candidates, not flatten them to one winner. It
should display installation type, exact path or app ID, version/channel,
ownership/provider, readiness, and whether the candidate is preferred.

Recommended behavior:

- system package plus Flatpak → `MultipleInstallations` until the user chooses;
- AppImage plus system binary → preserve the configured executable and show the
  other candidate as available;
- multiple AppImages → show each exact path/version/digest and do not normalize
  them into one installation;
- stale configured path → mark the configured choice broken/stale; do not
  silently substitute a newly discovered binary;
- configured binary version differs from discovered metadata → retain the
  configured path, surface the mismatch, and require review before update;
- a selected Flatpak should launch by its exact app ID, while a selected native
  candidate should launch by its exact validated path;
- explicit user choice should be durable, but a missing/changed selected path
  must fail closed rather than silently re-electing another candidate.

This preserves reproducibility and makes history meaningful: “launched
DuckStation AppImage at path X” is different from “launched Flatpak app ID Y”.

## Configuration, saves, firmware, and update boundaries

Binary replacement is not data migration. A lifecycle operation must treat the
following as separate evidence and ownership domains:

- executable/payload;
- configuration and controller mappings;
- save files, memory cards, savestates, NAND/HDD/VMU files;
- shader/cache/log files;
- BIOS, firmware, TOS, Kickstarts, keys, and system files;
- mods, texture packs, graphic packs, cores, plugins, and ROM/game paths.

XDG defaults are useful hints but are not universal. Portable mode, Flatpak
sandboxing, explicit command-line data roots, environment variables, and
version changes can relocate every category. Adapter readiness code should
remain the authority for BIOS/firmware evidence. Lifecycle code should record
the observed paths and warn when a version change has known save-state risk;
it must never delete or “reset” those paths as part of an update.

The project-specific examples are especially important:

- Hatari's TOS image and `~/.config/hatari` state are not the executable.
- FS-UAE's `Documents/FS-UAE` or configured base directory contains Kickstarts,
  floppy/CD paths, configs, saves, and plugins.
- Vita3K's Linux `config.yml`, `Vita3K` filesystem, patches/textures, and cache
  are separate XDG areas.
- xemu's HDD/flash data and Flatpak filesystem permissions are separate from
  the AppImage/package.
- PCSX2/RPCS3 firmware, BIOS, game data, and per-game configuration must not be
  treated as disposable binary-adjacent files.

## Offline and air-gapped behavior

These remain available without a network:

- selected executable/app ID/path;
- regular-file/executable validation;
- local version probe and cached manifest/package evidence;
- adapter profile/config discovery;
- BIOS/firmware/readiness checks;
- launch planning using the explicit selected installation.

These become unknown offline:

- a new upstream version;
- release cadence/current channel metadata;
- update availability when cache is absent or expired;
- a remote checksum/signature not already cached.

The user-facing result should be “Update availability unknown (offline)” rather
than “current” or “outdated”. A stale cached result may be displayed with its
timestamp and source, but must not authorize an update without fresh
revalidation.

## Future setup wording contract

The future GUI should be able to render provider-neutral facts without
pretending that all facts are equally actionable:

- **Installed** — exact selected path/app ID is present and locally valid.
- **Update available** — provider has a newer exact version; show provider and
  authority.
- **Managed by Flatpak** — use Flatpak to update/remove.
- **Managed by your system** — use the distribution package manager.
- **Using AppImage** — show exact path, version, channel, and ownership.
- **Multiple installations found** — require an explicit selection.
- **Version unknown** — launch may still be possible; update is blocked.
- **Needs attention** — broken manifest, missing executable, stale path, missing
  firmware, or other structured blocker.
- **Update availability unknown** — offline or provider metadata unavailable.

Setup must not use “Install” for a route that actually opens a browser, and it
must not call a package-manager command under a generic EmuWiz update button.

## Safe future adapter design

The smallest future core projection is a read-only lifecycle record built from
existing evidence:

```text
EmulatorLifecycleInstallation {
    emulator_id,
    exact_binding,          // native path or exact Flatpak app ID
    installation_type,
    ownership_category,
    version,
    version_source,
    channel,
    local_health,
    readiness,
    config/data evidence,
    update_authority,
    update_status,
    provenance,
}
```

It should consume existing inventory/profile/readiness providers instead of
re-probing each adapter. Provider implementations should be read-only and
bounded. A separate explicit action can then dispatch only to:

- EmuWiz's existing managed AppImage updater;
- the user's browser for an official download;
- the Flatpak/package-manager documentation or an explicitly approved external
  integration in a future scope.

No universal installer is justified by this audit.

## Blockers and recommendations

1. **No single adapter-wide provider registry exists.** Inventory coverage is
   narrower than native launch coverage, so lifecycle reporting must compose
   existing adapter evidence incrementally.
2. **External package ownership is not yet uniformly machine-proven.** A
   `PATH` binary should remain `Unknown`/`SystemPackageManaged` only when package
   evidence is actually available.
3. **Flatpak app IDs and permissions need a bounded provider.** Launch
   profiles can know an app ID, but inventory should not guess from a process or
   sandbox directory.
4. **Version parsing is not a universal identity.** Some builds report a
   release, commit, nightly, or no useful version. Preserve raw provenance and
   use `InstalledUnknownVersion` when needed.
5. **Portable trees need a manifest before safe automated updates.** Cemu,
   FS-UAE, MAME, and source builds may place important files beside the binary;
   a single-file replacement is not enough.
6. **Ryujinx is not a current official Linux provider.** Do not restore its
   updater or recommend mirrors. A current Switch adapter requires a fresh
   upstream and legal/provenance audit.
7. **Update cadence varies by channel.** Dolphin releases are periodic with
   development builds; Flycast exposes stable/master/nightly; Vita3K exposes
   nightlies; package repositories may lag. “Latest” must always be scoped to a
   provider and channel.

## Recommended delivery order

1. Reuse current `emulator_inventory`, managed-install manifests, and
   adapter-specific readiness as the evidence layer.
2. Add a read-only provider-neutral projection and explicit selected-install
   binding; do not alter launch selection implicitly.
3. Add bounded Flatpak/package metadata providers that report ownership but do
   not mutate installations.
4. Keep managed AppImage updates limited to manifest-owned installs with
   verified official digests and durable rollback.
5. Add individual provider lanes only when an emulator's official Linux channel,
   version identity, config boundary, and update authority are documented and
   tested.
6. Treat portable archive management as a separate transaction problem; never
   generalize single-file AppImage replacement to a whole emulator tree.

## Final recommendation

EmuWiz can safely report installed/version/readiness state offline today for
the evidence it already has. It can safely automate only its narrowly scoped,
verified, EmuWiz-owned AppImage lane. Everything else should be explicit,
provider-labelled, and non-destructive: Flatpak updates stay with Flatpak,
system updates stay with the OS, and manual/portable installs remain user
managed until a strict adoption boundary exists.
