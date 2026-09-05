# EmuWiz V1 packaging feasibility audit (Linux)

Status: research audit only — no packaging was implemented, no manifests were
changed. Evidence gathered from the repository at commit
`bd98790a587010d4f669d6c37451229f79168572` (branch
`feature/archivefs-unified-platform`, workspace version `0.8.1-alpha`).

Question: is AppImage a viable PRIMARY Linux distribution format for EmuWiz
V1, and is Flatpak a viable SECONDARY format? Every claim below is anchored
in a file that exists in this repository today.

## 1. Runtime binaries and helper processes

Produced by the workspace (`Cargo.toml`, three crates):

| Binary | Crate | Notes |
|---|---|---|
| `emuwiz` (GUI) | `archivefs-gui` | eframe/egui 0.34.3, `glow` backend, `wayland` + `x11` features; aliases `emuwiz-gui`, `archivefs-gui` run the same code (`crates/archivefs-gui/Cargo.toml`) |
| `emuwiz-cli` | `archivefs-cli` | alias `archivefs-cli`; same code (`crates/archivefs-cli/Cargo.toml`) |
| legacy aliases | install.sh | `archivefs-cli`/`emuwiz-gui`/`archivefs-gui` symlinks created at install time |

There is no daemon and no helper binary of our own (`docs/security.md`:
"There is no separate daemon process"). Everything else that runs is an
external command spawned as exact argv, never via a shell
(`crates/archivefs-core/src/launch/process_spawn.rs`).

External commands EmuWiz actually invokes in production code:

| Command | Where | Purpose |
|---|---|---|
| `ratarmount` (configurable via `ratarmount_bin`, default PATH) | `lib.rs` `RatarmountBackend::mount` | mounts ZIP/7z/RAR read-only via FUSE: `ratarmount <archive> <mount_path>` |
| `fusermount3` / `fusermount` / `umount` | `lib.rs` `lazy_unmount_path` | `fusermount3 -uz`, `umount -l` for unmount and stale-mount cleanup |
| `7z` | `dat/archive/rar.rs`, `dat/archive/lha.rs` | RAR and LHA/LZH member extraction for DAT audit (timeout-bounded, process-group cleanup) |
| `xdg-open` | GUI `main.rs` (browser + folders), `platform_artwork.rs`, `identity_source/romm/manual.rs` | open URLs in browser and folders in the file manager |
| managed emulator AppImages (PPSSPP, PCSX2) | `emulator_download.rs`, `managed_appimage_bootstrap.rs` | downloaded from GitHub releases into `~/.local/share/emuwiz/emulators/<id>/`, first-run bootstrapped by spawning the AppImage directly |

Not invoked by EmuWiz: `flatpak` (never planned or executed as
`flatpak run ...` for launch — see section 3), `unsquashfs`, `file`,
`appimagetool`, `zip`/`unzip` (ZIP listing is in-process via the `zip`
crate). `/proc/self/mountinfo` is read directly to verify mount/unmount
postconditions (`lib.rs:7895`); no external mount-state helper.

## 2. Filesystem access requirements

- **Arbitrary user-selected source folders.** Any number of independent
  absolute source folders (`config.toml.example`), scanned recursively for
  archives and direct media. Sources are read-only by contract
  (ARCHITECTURE.md; ROADMAP.md path-validation guarantees).
- **/mnt and mount roots.** `mount_root` defaults to `/mnt/archivefs`
  (`config.toml.example`). EmuWiz creates the directory itself
  (`lib.rs:5021`, `fs::create_dir_all`); the user needs write/traverse rights
  there. Mount points are created under this root only.
- **Removable disks / network mounts.** Source-root validation does not
  restrict paths to a fixed tree; any mounted filesystem reachable by the
  user works. Hashing follows symlinks under configured roots with bounded
  reads and explicit out-of-root refusal (`identity_source/net_policy.rs`,
  `safe_read.rs`, SOURCE_ROOT_PATH_AUDIT.md).
- **Symlinks / read-only sources.** Library Views are symlink-based.
  SOURCE_ROOT_PATH_AUDIT.md documents that all persisted paths are absolute
  user-selected roots, never relocated automatically.
- **Config / data / cache directories** (`crates/archivefs-core/src/app_dirs.rs`):
  - config: `~/.config/emuwiz` (legacy `~/.config/archivefs` reused when it
    exists). Resolution is literal `$HOME/.config/...` — the XDG environment
    variables are deliberately *not* consulted for these paths.
  - data: `~/.local/share/emuwiz` — SQLite catalogue (`library.sqlite3`,
    bundled SQLite via rusqlite), journals, caches (including the 1 GiB RomM
    artwork LRU cache), and `emulators/` for managed emulator installs.
  - installer manifest: `$XDG_DATA_HOME/emuwiz-installer/manifest`
    (`install.sh`).
  - reads/writes into *emulator-owned* config trees: `~/.config/<emulator>`
    and `~/.var/app/<flatpak-app-id>/...` for cheat/patch workflows
    (`patch_manager/*_local.rs`).
- **Writable library destinations.** Library Views / RomM layouts apply
  symlink trees into user-approved destination roots (explicit plan + apply).


## 3. Emulator discovery and launching

- **Native executables.** Discovery and profile inventory for native,
  Flatpak, portable, and user-configured emulator profiles (README "Emulator
  Setup & Launch"; `diagnostics/profiles.rs`). Launch *execution* is
  native-only today: RetroArch launch refuses Flatpak/AppImage profiles at
  the profile-kind gate (`launch/execution.rs:8,134,293`), and the PCSX2
  adapter likewise "never launches Flatpak/Portable/AppImage/NativeAlternate"
  (`launch/pcsx2_execution.rs:29`).
- **Flatpak-installed emulators.** Detected (user `~/.local/share/flatpak/app`,
  system `/var/lib/flatpak/app`; `diagnostics/profiles.rs:197-205`) and their
  sandbox config trees (`~/.var/app/<app-id>`) are read/written for cheats
  and patches (`patch_manager/ppsspp_local.rs`, `duckstation_local.rs`, ...).
  They are *not* launched.
- **AppImages.** Read-only RetroArch AppImage detection in five fixed `$HOME`
  roots plus XDG desktop-entry roots (docs/RETROARCH_APPIMAGE.md); managed
  PPSSPP/PCSX2 AppImage download + first-run bootstrap lane
  (`managed_appimage_bootstrap.rs`). Not launched as regular content.
- **User-supplied emulator paths.** Supported via explicit configuration;
  emulator executable paths are `USER_SELECTED_EXTERNAL`
  (SOURCE_ROOT_PATH_AUDIT.md).
- **Environment inheritance and working directories.**
  `spawn_watched_process` spawns the exact verified argv with **no
  environment injection or override — the child inherits EmuWiz's
  environment exactly** — optional explicit `current_dir`, stdin null,
  stderr piped and drained with a bounded capture limit, never a shell
  (`launch/process_spawn.rs:151-164`). DOSBox sets `current_dir` to the game
  directory (`launch/dosbox_execution.rs`). No launch arguments are
  synthesized at spawn time; argv always comes from reviewed launch plans.

## 4. GPU / display requirements

- EmuWiz itself: **OpenGL only** (eframe `glow`), with both Wayland and X11
  support compiled in (`crates/archivefs-gui/Cargo.toml`), plus the Wayland
  data-control clipboard (`arboard`, `wayland-data-control`). **No Vulkan and
  no wgpu** anywhere in the workspace.
- The GUI's windowing/GL libraries are **dlopen'd at run time**: `ldd` on a
  real `target/release/emuwiz` build shows only `libgcc_s.so.1`, `libm.so.6`,
  `libc.so.6` (plus vdso/loader). Nothing from the GL/Wayland/X11 stack is a
  DT_NEEDED entry.
- NVIDIA proprietary / Wayland/X11: because nothing GPU-related is linked or
  would be bundled, driver selection stays entirely with the host. An
  AppImage introduces no obvious GL/Vulkan/NVIDIA conflict *as long as no
  GL/Wayland/X11 libraries are bundled into it* (see section 7).
- Emulators are separate processes spawned with inherited env; their GPU
  stacks (Vulkan, GL, NVIDIA) are untouched by how EmuWiz itself is packaged.

## 5. FUSE / archive mounting — what EmuWiz expects

Exact contract, all in `crates/archivefs-core/src/lib.rs`:

1. `ratarmount` (or a path from `ratarmount_bin`) must be executable and on
   PATH; Doctor/setup diagnostics probe it with `command_available`
   (`lib.rs:1056`, `1965`) and print plain-language install guidance when
   missing (`lib.rs:1539-1540`). `install.sh` step 8 prints the same
   guidance.
2. Mounting = one subprocess call `ratarmount <archive> <mount_path>`
   (`RatarmountBackend::mount`, `lib.rs:5069-5075`). Success is *never*
   trusted from the child's exit status: the mount must appear in
   `/proc/self/mountinfo` (`verify_mounted`, `lib.rs:5076-5086`).
3. Unmounting uses `fusermount3 -uz`, falling back to `fusermount` /
   `umount -l` (`lib.rs:7967-8002`), again verified against mountinfo.
4. `mount_root` is created by EmuWiz if missing (`lib.rs:5021`).

Implications for packaging:

- ratarmount is a Python application needing `fusermount3` and the FUSE
  kernel interface (`/dev/fuse`). It must **remain a host system dependency**
  — bundling a Python/ratarmount/FUSE stack into an AppImage would fight the
  host's FUSE setup for no benefit and should not be attempted for V1.
- Because EmuWiz performs no privileged operations itself (mounts happen
  through the user-space ratarmount/fusermount3 setuid path), an AppImage can
  support mounting fully and unconditionally. No host dependency has to move
  inside the package.
- The only soft spot is the default `mount_root = "/mnt/archivefs"`: on
  typical distros `/mnt` is root-owned, so first-run creation there can fail
  for a non-root user unless the path is pre-created or the config is
  changed. This is a pre-existing UX/config concern, not a packaging blocker,
  but release engineering should surface it.

## 6. RomM / local-service integration

- HTTP via `ureq` 3.x with **rustls** (`crates/archivefs-core/Cargo.toml`) —
  no system OpenSSL, no dlopen'd TLS provider. Talks to a user-configured
  RomM instance (localhost or LAN), with endpoint validation in
  `identity_source/net_policy.rs` (approved endpoints, SSRF-guarded artwork
  fetch from the configured instance only).
- Credentials: a RomM token is held redacted in memory and, if saved,
  persisted as a `0600` plaintext file under a caller-named config path;
  there is deliberately no keyring/secret store
  (`identity_source/romm/config.rs:1-25`).
- Browser/open-url: the GUI launches `xdg-open <url>` through
  `DesktopBrowserLauncher` (`crates/archivefs-gui/src/main.rs:14666`,
  `20896`); the CLI and core also open folders with `xdg-open`.
- Native folder picking uses **rfd with the xdg-desktop-portal backend**
  (D-Bus FileChooser), portal-aware and in-process
  (`crates/archivefs-gui/Cargo.toml:57-63`).
- EmuWiz opens no listening server; all network use is outbound (RomM,
  GameHacking.org, DAT mirrors, GitHub for managed emulator downloads).
  DNS and localhost/LAN connections work unchanged under an AppImage.


## 7. Native library dependencies — bundle vs. do-not-bundle

Authoritative evidence: `ldd` against an existing release-mode build
(`target/release/emuwiz`, `target/release/emuwiz-cli`) shows the *only*
DT_NEEDED libraries are `libgcc_s.so.1`, `libm.so.6`, `libc.so.6`.

**Must NOT be bundled** (host-provided; bundling causes driver/shadowing
conflicts):

- `libGL*`, `libEGL*`, `libOpenGL*` — OpenGL for the GUI (eframe glow loads
  GL dynamically; NVIDIA proprietary drivers supply their own).
- `libvulkan*` — not used by EmuWiz; emulators own their own Vulkan usage.
- NVIDIA driver libraries (`libGLX_nvidia`, `libnvidia-*`) — never.
- `libwayland-client*`, `libxkbcommon*`, the X11/XCB stack — the display
  stack is dlopen'd by winit/smithay-client-toolkit and is present on every
  desktop distro; bundling risks protocol/ABI mismatches with the running
  compositor.
- `libc`/`libm`/`ld-linux` — glibc itself; AppImages must run against the
  host loader.
- Anything Python/FUSE related for ratarmount — host dependency (section 5).

**Likely must be bundled or verified:** effectively nothing beyond the two
binaries themselves. SQLite is compiled into the binary (rusqlite
`bundled`), TLS is rustls, zstd/disc-image codecs compile into the binary,
fonts are embedded (`default_fonts`), PNG/JPEG decoding is Rust-native.
The AppImage payload can be just `emuwiz`, `emuwiz-cli` (or symlinks), plus
AppImage metadata — an unusually small bundle surface.

**glibc risk:** the build must happen against the oldest supported glibc
baseline (the interpreter reference is `/lib64/ld-linux-x86-64.so.2`, GNU
3.2.0 symbol floor). Release engineering must pin the build container
accordingly; this is a build-hosting decision, not an architecture change.

**FUSE/desktop-integration risks:** none beyond keeping ratarmount external
and shipping the `.desktop`/icon story (section 8).

## 8. Desktop integration

Current state (already implemented for the tarball/install.sh channel):

- `.desktop` template: `assets/linux/io.github.kiehntre.emuwiz.desktop.in`
  with `Exec=@EMUWIZ_EXEC@`, `Icon=io.github.kiehntre.emuwiz`,
  `StartupWMClass=io.github.kiehntre.emuwiz`.
- The GUI sets its Wayland/X11 app id to the exact same value
  (`LINUX_APP_ID = "io.github.kiehntre.emuwiz"`,
  `crates/archivefs-gui/src/main.rs:812-847`), so taskbar grouping and
  Wayland app-id matching already line up.
- Icons: `assets/branding/emuwiz-logo-{32,64,128,256,512}.png`; install.sh
  installs the launcher + hicolor icons under `$XDG_DATA_HOME` and records
  every path in its SHA-256 ownership manifest.
- File associations: none today (`Categories=Utility;Archiving;` only).
- Update mechanism: none in-app; updates are replacement-artifact installs
  via install.sh (`docs/RELEASE_ENGINEERING.md`; GitHub tag-driven release
  workflow in `.github/workflows/release.yml`).

For an AppImage the desktop-entry work is *additive*: an AppImage needs its
own embedded `.desktop` + icons for menu integration (typically via
AppImageLauncher / appimaged, or a small optional install step). The app id
and WM class already match, so no code change is required. The existing
install.sh channel does not have to be retired.

## 9. Flatpak comparison — where sandboxing complicates EmuWiz

- **Arbitrary /mnt access & whole-home filesystem.** EmuWiz needs every
  source folder, mount roots, emulator config trees, and library
  destinations. A useful Flatpak needs `--filesystem=host` (plus
  `--filesystem=/mnt` style grants), which reduces the sandbox to a
  permission formality. AMBER, not fatal.
- **External emulator launching.** Flatpak apps are expected to launch other
  Flatpak apps via portals/`flatpak-spawn --host`; spawning arbitrary host
  native/AppImage emulators by exact argv, with inherited env and arbitrary
  working directories (`launch/process_spawn.rs`), is not the Flatpak model.
  Supporting it needs `--talk-name=org.freedesktop.Flatpak` and an
  architectural change to wrap or replace direct spawning. RED for V1.
- **FUSE.** Mounts created inside a Flatpak sandbox live in the sandbox's
  mount namespace and are not visible to host processes by default. The
  entire point of EmuWiz mounts is that *other host programs* (emulators,
  file managers) see them. Either ratarmount would need to run outside the
  sandbox or the mounts become sandbox-private — both are unacceptable or
  fragile for V1. RED.
- **Host binaries.** ratarmount, `7z`, and `fusermount3` are host binaries;
  inside a sandbox they are absent from the runtime (ratarmount is not in
  any Flatpak runtime). Bundling Python+FUSE inside Flatpak and reaching the
  host FUSE device is possible but compounds the namespace problem above.
  RED (same root cause).
- **Local services (RomM).** `--share=network` covers localhost/LAN HTTP and
  DNS. GREEN with standard permissions.
- **GPU access.** Flatpak runtimes handle GL/Vulkan/NVIDIA cleanly via the
  freedesktop runtime's driver machinery. GREEN.
- **Portals.** The folder picker already uses xdg-desktop-portal directly
  (rfd). GREEN. `xdg-open` also works through runtime/portal bridges.
- **Emulator config trees.** `~/.var/app/<app-id>` paths are host-side;
  writing to *other* Flatpaks' trees from inside a sandbox requires broad
  filesystem grants. AMBER.

Net: Flatpak can be made to work only by granting away most of the sandbox
and still cannot deliver host-visible FUSE mounts or host emulator launching
without new architecture. It is a poor V1 secondary; the existing
tarball + install.sh channel already fulfils the "portable secondary"
role.

## 10. Decision table

| Requirement | AppImage | Flatpak | Risk | Proposed solution |
|---|---|---|---|---|
| GUI runtime (OpenGL/Wayland/X11) | works; dlopen'd host stack | works via runtime | GREEN | bundle nothing GL/display-related |
| CLI runtime | works | works | GREEN | — |
| glibc baseline | only libc/libm/libgcc_s needed | N/A (runtime) | AMBER | build against oldest supported glibc; document floor |
| ratarmount / FUSE mount | works; PATH lookup unaffected | RED — sandbox mount namespace hides mounts from host | AMBER (AppImage) / RED (Flatpak) | keep ratarmount external; Doctor guidance already exists |
| fusermount3 / umount | works | RED (same root cause) | AMBER / RED | host dependency, unchanged |
| `7z` (RAR/LHA DAT audit) | works via PATH | must be granted/bundled | GREEN / AMBER | PATH lookup, unchanged |
| xdg-open (browser, folders) | works | works via portal bridge | GREEN | — |
| Arbitrary source folders, /mnt, removable, network mounts | works | needs `--filesystem=host` | GREEN / AMBER | — / broad grants |
| Symlinks, read-only sources | works | works | GREEN | — |
| Config/data/cache dirs (`~/.config/emuwiz`, `~/.local/share/emuwiz`) | works (literal $HOME paths) | needs explicit grants | GREEN / AMBER | — / grants |
| Emulator discovery (native/Flatpak/AppImage/user paths) | works ($HOME roots unchanged) | works but partly meaningless inside sandbox | GREEN / AMBER | — |
| Launch execution (exact argv, inherited env, workdir) | works, BUT AppImage runtime exports `APPDIR`, `LD_LIBRARY_PATH` etc. to every child — collides with the "inherits exactly" spawn contract | RED — needs flatpak-spawn architecture change | AMBER (AppImage) / RED (Flatpak) | bundle no shadowing libs; verify spawned emulators/ratarmount still resolve host libs; document env policy in release engineering |
| Managed emulator AppImage downloads (PPSSPP/PCSX2) | works | works (network) but downloads into sandbox-visible path | GREEN / AMBER | unchanged data dir |
| RomM (localhost/LAN HTTP, token file) | works | works with `--share=network` | GREEN | — |
| Folder picker (xdg-desktop-portal) | works (D-Bus session) | works (native case) | GREEN | — |
| Desktop entry + icons | works; AppImage needs embedded `.desktop` + integration story | works | AMBER | embed desktop entry/icons; recommend AppImageLauncher or optional install step; reuse existing app id |
| File associations | none required today | — | GREEN | out of scope |
| Update mechanism | none in-app today; unchanged | Flathub would impose its own model | AMBER | keep artifact-replacement model; do not commit to AppImageUpdate for V1 |
| Filesystem watcher (inotify via `notify`) | works | works | GREEN | — |
| Doctor diagnostics (ratarmount/fusermount probes) | work unchanged | probes would probe the sandbox | GREEN / AMBER | — |

## 11. Issue classification

**GREEN (works naturally under AppImage):** GUI/CLI runtime; display stack;
FUSE mounting via host ratarmount; unmount tools; arbitrary source folders
incl. /mnt, removable and network mounts; symlinks and read-only sources;
config/data/cache directories; emulator discovery (native, Flatpak,
AppImage, user paths); managed emulator AppImage downloads; RomM networking
and token persistence; portal folder picker; `xdg-open`; inotify watcher;
Doctor probes; desktop entry id/WM-class alignment.

**AMBER (solvable packaging/config work):**
1. **Child-environment contract.** The AppImage runtime exports
   `APPDIR`/`APPIMAGE`/`ARGV0`/`LD_LIBRARY_PATH` into EmuWiz's environment,
   and `spawn_watched_process` deliberately passes the environment through
   to emulators and ratarmount. With a zero-library bundle this is harmless
   in practice, but it must be verified (spawn a host emulator and ratarmount
   from a packed AppImage; confirm no host-lib shadowing) and documented as
   an explicit exception or handled by a pre-spawn environment policy.
2. **glibc build floor.** Build images must target the oldest supported
   baseline; document the minimum glibc in release notes.
3. **Desktop integration for the AppImage format.** Embed `.desktop` + icons
   and define the integration path (AppImageLauncher/appimaged/manual) —
   reusing the existing `io.github.kiehntre.emuwiz` id.
4. **Default `mount_root` permissions.** `/mnt/archivefs` may not be
   user-writable; surface it in install/Doctor guidance (pre-existing issue,
   more visible once packaged).

**RED (architectural blockers):** none for AppImage. For Flatpak: (a) FUSE
mounts created inside the sandbox are invisible to the host processes they
exist for; (b) launching arbitrary host emulators by exact argv contradicts
the sandbox model and would require a spawn-architecture change. Both are
disqualifying for V1.

## 12. Recommendation

- **AppImage primary: CONDITIONAL YES.** Viable as the primary V1 format:
  the binaries are nearly self-contained (glibc-only DT_NEEDED), every
  external dependency (ratarmount, fusermount3, 7z, xdg-open) is a PATH
  lookup that works unchanged, and there is no sandbox to fight. The four
  AMBER items above are release-engineering tasks, not code architecture
  changes; item 1 (child-environment verification) must be *proven* before
  an AppImage ships, because the exact-argv/exact-env spawn contract is a
  core guarantee of this codebase.
- **Flatpak secondary: LATER.** Do not attempt for V1. The FUSE
  mount-namespace problem and host-emulator launching are architectural;
  revisiting only makes sense after those have an accepted design (or after
  EmuWiz accepts sandbox-only mounts and `flatpak-spawn` launching).
- **Keep the existing tarball + install.sh channel** during V1; it already
  provides the portable/secondary distribution role and the ownership
  manifest machinery.

### Blockers that must be solved before AppImage packaging work begins

1. A written, verified decision on the child-process environment policy
   under the AppImage runtime (verify no host-library shadowing for spawned
   emulators, ratarmount, and 7z; add the verification to release QA).
2. Selection of the glibc build baseline and build container for the
   AppImage job.
3. The desktop-integration approach for AppImage (embedded `.desktop`/icons
   + AppImageLauncher/appimaged/manual install recommendation).
4. Release-engineering documentation for the external host dependencies
   (ratarmount, fusermount3, optional 7z) as AppImage-non-bundlable
   requirements, mirroring the existing Doctor messaging.

Nothing in this audit requires changes to Cargo manifests, the spawn layer,
or the mount backend before packaging can start.
