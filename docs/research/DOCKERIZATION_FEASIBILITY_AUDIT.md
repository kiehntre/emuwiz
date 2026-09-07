# EmuWiz Dockerization Feasibility Audit V1

**Audit date:** 2026-09-07
**Local authority:** `dded58c111c62b4f99f212f36ad49b458289120c` on
`feature/archivefs-unified-platform`
**Scope:** design and evidence audit only. No Dockerfile, Compose file, or
production code was added.

## Executive conclusion

Docker is a good fit for a **headless, read-only analysis image** used on NASes,
CI runners, and remote servers. It is technically possible to display the egui
application through X11, Wayland, or VNC, but each option adds host-specific
socket, UID, GPU, clipboard, audio, and security requirements. It is not a
better general desktop distribution than the already QA-tested DEB/RPM and
AppImage paths. Launching the host's native emulators from a container is not a
safe or portable default.

Recommended policy: support an official **CLI/core image** first, with source
libraries read-only by default and emulator launching disabled. Treat GUI images
as experimental documentation examples only. Do not promise a full
game-launching container until a separately designed host-launch or
in-container-emulator architecture exists.

## Evidence inspected

The conclusions below are grounded in the current local tree, including its
uncommitted work (which was not modified):

- Workspace split and dependencies: `Cargo.toml`,
  `crates/archivefs-core/Cargo.toml`, `crates/archivefs-cli/Cargo.toml`, and
  `crates/archivefs-gui/Cargo.toml`.
- Runtime packaging and QA: `packaging/debian/control`,
  `packaging/rpm/emuwiz.spec`, `packaging/debian/build-deb.sh`,
  `packaging/rpm/build-rpm.sh`, `docs/LINUX_PACKAGING.md`, and
  `docs/APPIMAGE_PACKAGING.md`.
- State paths: `crates/archivefs-core/src/app_dirs.rs`.
- Launch boundaries: `crates/archivefs-core/src/launch/mod.rs`,
  `launch/process_spawn.rs`, adapter `*_command.rs`/`*_execution.rs`, and
  `crates/archivefs-gui/src/main.rs`.
- Media probing: `crates/archivefs-core/src/laserdisc_set.rs` (bounded
  `ffprobe`) and optical conversion/specialist modules.
- Historical packaging and launch archaeology: commits `400644d`, `b4ea8db`,
  `4abc8ec`, `c16f486`, `3eb033e`, and the repository's `origin` remote and
  branches. Local code is authoritative where history differs.

The working tree also contains unrelated dirty emulator, DAT, cheat, and media
files. They are outside this audit and remain untouched.

## What is actually in the binaries

### Core and CLI

The core is predominantly Rust and includes bundled SQLite (`rusqlite` with the
`bundled` feature), SHA/MD5/SHA-1 and in-tree CRC32, pure-Rust ZIP/TAR/7z
inspection, CHD logical support, optical/media evidence, and many read-only
identity and planning paths. Production archive inspection deliberately avoids
shelling out for ZIP/7z listing. The CLI is a separate binary package and has
no GUI dependency.

External commands are optional feature boundaries, not a universal startup
requirement: `ratarmount`/`fusermount3` for read-only archive mounts, `7z` and
`rar` for selected archive backends, and `xdg-open` for desktop opening. Media
metadata uses a bounded `ffprobe` subprocess when that probe is requested.
`chdman` is used only by the explicit optical conversion workflow; it is not
needed for read-only identity scans. These names and their optional status are
also recorded in `docs/LINUX_PACKAGING.md` and package metadata.

### GUI

`archivefs-gui` uses `eframe` with both `glow` and the `x11` and `wayland`
backends, plus default fonts. Native clipboard support includes Wayland data
control. Folder selection uses `rfd`/desktop portals. Consequently the GUI
needs a desktop graphics stack even though the core does not.

### Desktop/package integration

DEB and RPM install the GUI and CLI separately under `/usr/bin`, with desktop
entry, metainfo, and icons. AppImage is an existing distribution path. Flatpak
is explicitly deferred because FUSE mounts and launching host emulators do not
cross its sandbox safely (`docs/LINUX_PACKAGING.md`).

## Docker modes

| Mode | Fit | Findings |
|---|---|---|
| CLI/headless, read-only | **GOOD FIT** | Scans, identity/DAT audits, reports and pure plans work without a display or emulator. `--network=none` is viable for local data. |
| GUI via host X11 | **POSSIBLE / AWKWARD** | Requires `DISPLAY`, `/tmp/.X11-unix`, Xauthority handling, matching UID, graphics libraries, and weakening X access control or arranging an xauth cookie. It exposes the host display and is not portable. |
| GUI via Wayland | **POSSIBLE / AWKWARD** | Requires the compositor socket under `$XDG_RUNTIME_DIR`, matching user permissions, toolkit-compatible GL, and often XWayland fallback. Socket paths and portal/D-Bus behavior are host-specific. |
| GUI via VNC/noVNC | **AWKWARD** | A virtual display, VNC server, browser proxy, fonts, and another service are needed. It is useful for controlled remote demos, not a normal desktop install. |
| Browser/server UI | **BAD FIT** | Would require a new web product and security model; the current egui desktop application is not a server UI. |

## Display, GPU, audio, and controllers

Headless CLI has no display requirements. An X11 GUI needs the host X socket,
`DISPLAY`, Xauthority, and Mesa/GL libraries; a Wayland GUI needs the Wayland
socket and runtime directory. Hardware acceleration commonly needs `/dev/dri`
and matching Mesa/Vulkan ICDs. Intel/AMD Mesa is realistic when the container
uses compatible userspace libraries. NVIDIA requires the NVIDIA Container
Toolkit and host-driver/ICD matching; it should not be presented as a generic
portable recipe. A server without a desktop/GPU is not a sensible native GUI
target.

Emulator play additionally needs audio and input devices: `/dev/input`, udev
device visibility, SDL, and PulseAudio/PipeWire/ALSA sockets or devices. Hotplug
and group permissions are fragile in a general container. This is a strong
reason to keep launch disabled in the official analysis image.

## Paths, identity, permissions, and mounts

`app_dirs.rs` resolves state from `HOME`, preferring
`~/.config/emuwiz`/`~/.local/share/emuwiz` and falling back as a whole directory
to legacy `archivefs` paths. It deliberately does not use `XDG_CONFIG_HOME` or
`XDG_DATA_HOME`. A container therefore needs an explicit `HOME` and persistent
bind/volume for configuration and data; otherwise every run is a fresh profile.
The SQLite database, journals, caches, history, and managed metadata live below
that data root (for example `library.sqlite3`).

Recommended conceptual volumes (not implemented here):

```text
/config       -> persistent app config (HOME/.config/emuwiz)
/state        -> persistent app data/database/journals (HOME/.local/share/emuwiz)
/cache        -> optional disposable cache if a future split is introduced
/library-ro   -> source library, read-only by default
/library-rw   -> explicit destination/organisation root, writable only when needed
```

Absolute paths are part of library records, transaction journals, emulator
profiles, and launch inputs. If the host path `/mnt/games` is mounted as
`/library`, records created in the container contain `/library` and are not
meaningful to a native host process. The safest convention is to bind the
same absolute path inside and outside (for example `/mnt/games:/mnt/games:ro`)
and keep a separate container profile when that is impossible. A general path
translation layer would be a moderate-to-major architecture change.

UID/GID should normally be passed as the host user's numeric IDs and writable
volumes pre-created with matching ownership. Rootless Docker is preferable.
Symlinks can escape a nominal mount if not checked; bind mounts do not make
symlink policy disappear. Rename transactions require a writable destination,
and hardlinks require the same filesystem/device: `/library-ro` and
`/library-rw` on different mounts cannot be hardlinked and may return `EXDEV`.
Cross-device moves, inode/device identity checks, rollback journals, and
container-visible paths therefore need explicit refusal or a documented
copy/symlink mode. Docker does not preserve host inode identity across a
different storage driver.

Organisation should be treated as an opt-in writable mode with same-device
source/destination mounts. The default image should expose planning/reporting,
not apply, when those invariants cannot be proven.

## Native emulator launching

Launch planning is intentionally separate from execution. The execution layer
uses `std::process::Command` with verified argv and live file revalidation, and
adapter modules cover native programs such as Dolphin, PCSX2, DuckStation,
PPSSPP, RPCS3, Amiberry/FS-UAE, VICE, Fuse, RMG, and others. A container cannot
normally spawn a host binary outside its namespace.

| Option | Security/portability | Assessment |
|---|---|---|
| Install emulators in image | Isolated but huge; GPU/audio/input and profile lifecycle become image concerns | Suitable only for a purpose-built appliance, not the first image |
| Bind host binaries | Fragile library/ABI and path coupling; effectively grants host execution surface | Do not support as a general recipe |
| Host execution helper/bridge | Possible, but requires a new authenticated IPC protocol and path/identity translation | **Major architecture change** |
| Disable launch in Docker | Safest, deterministic, aligns with read-only mode | **Recommended** |
| Separate analysis and play modes | Keeps Docker useful while native packages own play | Recommended product boundary |

Host emulator configuration discovery also scans user XDG locations and
emulator-specific profile roots (including Flatpak/AppImage arrangements).
Binding all of `~/.config` or `~/.local/share` leaks unrelated user data and
still leaves host-path and socket mismatches. A Docker image should report
profiles as unavailable unless narrowly selected, read-only profile mounts are
provided, and launch is explicitly an in-container feature.

## Security and network model

The safe baseline is a read-only root filesystem, `--cap-drop=ALL`,
`--security-opt=no-new-privileges`, non-root UID/GID, minimal explicit bind
mounts, and no privileged mode. Do not use `--network=host`; `--network=none`
supports local scans and reports. Network is only needed for optional provider
or DAT downloads, metadata enrichment, RetroAchievements, or emulator/bootstrap
downloads. Those should be explicit network-enabled runs with separate policy.
FUSE archive mounting is not a good default container feature: it needs
`/dev/fuse` and mount permissions and produces mounts whose visibility to host
emulators is not guaranteed. The official image should not require
`--privileged` merely to start.

## Package, base-image, and architecture choices

The validated Debian package is the simplest runtime input for a Debian-based
image, while a multi-stage Rust build is more self-contained and allows a
minimal CLI-only artifact. RPM is useful for Fedora-native users but adds no
advantage to a first cross-host image. AppImage is a desktop distribution
format, not a useful container base; Flatpak is intentionally sandboxed for a
different integration model.

Recommended bases:

- **Headless:** Debian 12/13 slim (or Ubuntu 24.04 when matching existing QA),
  with the validated CLI artifact and only explicitly needed helper tools.
- **GUI experiment:** Debian/Ubuntu full enough for eframe X11/Wayland, Mesa,
  fonts, and portals; document host-specific socket setup rather than claiming
  universal portability.
- Fedora is reasonable for a Fedora-specific derivative. Distroless is a poor
  debugging/tooling fit. Initial publication should target **amd64**. Rust
  itself is portable to arm64, but GUI graphics, ffprobe/archive helpers,
  specialist CHD native builds, and emulator availability require a separate
  arm64 matrix before promising it.

Rough image ranges (base and selected tools, not measured release sizes):

| Image | Expected range | Main driver |
|---|---:|---|
| CLI/core | ~150–300 MB | Debian userspace, certificate store, binary, optional probe/archive tools |
| GUI + media tools | ~300–600 MB | graphics/window-system libraries, fonts, media tools |
| GUI + emulator bundle | multi-GB | emulator binaries, firmware, GL/Vulkan/audio stack and their updates |

Bundling every emulator would multiply size and update cadence and still would
not solve controller/GPU host variance.

## Conceptual Compose and NAS use

No Compose file is added by this audit. A future headless example should be
small and explicit:

```yaml
services:
  emuwiz:
    image: ghcr.io/kiehntre/emuwiz-cli:<version>
    read_only: true
    security_opt: [no-new-privileges:true]
    cap_drop: [ALL]
    network_mode: none
    volumes:
      - emuwiz-state:/state
      - /mnt/games:/mnt/games:ro
      - /mnt/playing:/mnt/playing:rw # only for an explicit apply command
```

The exact command and state environment still need a product decision; this is
illustrative only. GUI Compose is host-specific because display sockets,
Xauthority/Wayland credentials, `/dev/dri`, audio, and user IDs must be added.

Unraid, TrueNAS SCALE, Proxmox guests, and generic NAS hosts are strong
audiences for read-only scans of large archives, DAT reports, identity audits,
and exportable organisation plans. A container can run beside the library
without installing a desktop stack or emulator. A VM may be preferable when
users need a complete desktop and emulator access; an LXC/container with broad
device access does not remove the same security concerns.

## Docker versus native distribution

| Use case | Best fit | Reason |
|---|---|---|
| Desktop manage and launch games | DEB/RPM/AppImage (Flatpak only after its sandbox gaps are solved) | Native paths, profiles, display, audio, controllers, and emulator binaries work naturally. |
| NAS preservation scan/report | Docker headless | Reproducible dependencies, no desktop, easy scheduled jobs and isolated read-only mounts. |
| CI validation | Docker headless | Pinned image/toolchain and deterministic reports. |
| Remote server | Docker headless | Good isolation and persistent state volume. |
| Portable one-off inspection | CLI binary/AppImage | Lower setup than socket/device mounts. |
| Emulator launch/readiness | Native packages | Docker would need emulators, GPU/audio/input and profile exposure. |

## Hard blockers and scores

| Concern | Classification | Consequence |
|---|---|---|
| CLI/core execution | **NO CHANGE REQUIRED** | Existing local-first/read-only paths map well to a container. |
| Persistent absolute paths | **MODERATE CHANGE** | Same-path mounts or a future translation/profile model are required. |
| Cross-device rename/hardlink semantics | **MODERATE CHANGE** | Apply must fail closed or require same-device writable mounts. |
| X11/Wayland GUI | **SMALL–MODERATE CHANGE** | Mostly packaging/docs, but host security and toolkit variance remain. |
| Controllers/audio/GPU | **MAJOR for general users** | Device/socket passthrough is fragile and host-specific. |
| Host emulator execution | **MAJOR ARCHITECTURE CHANGE** | Needs an authenticated bridge or full in-container emulator stack. |
| Flatpak profile/FUSE integration | **MAJOR ARCHITECTURE CHANGE** | Existing native assumptions do not cross sandbox/container boundaries. |

| Mode | Difficulty (1 easy–10 hard) | User value | Recommendation |
|---|---:|---:|---|
| CLI/headless read-only | 3/10 | 9/10 for NAS/CI | Official Phase 1 |
| CLI with explicit organisation apply | 5/10 | 7/10 | Phase 2 after path/mount contract |
| GUI through X11/Wayland | 7/10 | 4/10 | Experimental only |
| GUI through VNC/noVNC | 8/10 | 3/10 | Do not make default |
| Full emulator-launching container | 10/10 | 3/10 | Not officially supported initially |

## Recommended phases

1. **Headless read-only image:** CLI scans, identity/DAT audits, media reports,
   and exportable plans; amd64 first; `--network=none` and read-only source
   mounts documented.
2. **Explicit writable planning/apply mode:** add a tested mount/path contract,
   same-device checks, UID/GID guidance, and transaction recovery procedures.
3. **Experimental GUI image:** package eframe dependencies and document X11
   first, then Wayland; no promise of host emulator launch.
4. **Only if demand justifies it:** design a host-launch helper or a narrowly
   scoped in-container emulator image. This is a new security/product lane, not
   a Docker packaging tweak.

## Final recommendation

Docker should be officially supported for **headless analysis and reporting**,
not as a replacement for native desktop packages. GUI Docker is technically
possible but should remain experimental and clearly labeled. Full play-mode
Docker should not be advertised until path translation, device access, profile
ownership, and launch security receive dedicated architecture work.

No production code, Dockerfile, Compose file, user media, or emulator
configuration was changed by this audit.
