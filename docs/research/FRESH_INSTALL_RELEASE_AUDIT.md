# Fresh-install release audit

Audit basis: `f535f4f71f8d28d32b4a22d2ec93d05cc6ace007`, Linux source tree,
read-only inspection of current packaging, startup, app-directory, diagnostics,
and emulator-discovery code. No release artifact was present in `dist/`, and no
binary was rebuilt or installed during this audit.

Evidence references are repository paths so the findings can be checked against
the release commit directly.

## CURRENT RELEASE SHAPE

There are currently several packaging contracts in the tree:

| Surface | Current evidence | Assessment |
| --- | --- | --- |
| Metadata release packager | `scripts/release/package_release.py` | Emits `emuwiz-<version>-linux-<arch>.tar.xz`; payload root contains `bin/emuwiz`, `bin/emuwiz-cli`, generated docs, licences, provenance, checksums, and optional SBOM. |
| Maintained release engineering document | `docs/RELEASE_ENGINEERING.md` | Still describes `archivefs-v<version>-<arch>-linux.tar.gz`, a top-level `install.sh`, README, config example, desktop file, and icons. |
| User quick install | `README.md` | Downloads the old `archivefs-...tar.gz`, runs `tar -xzf`, then `./install.sh`. |
| AppImage | `packaging/appimage/build-appimage.sh`, `packaging/appimage/AppRun` | x86_64-only, host graphics/FUSE/tool dependencies, manual download-and-replace. |
| DEB/RPM | `packaging/debian/`, `packaging/rpm/`, `docs/LINUX_PACKAGING.md` | Separate package lanes exist, with historical/current-status caveats in the documentation. |
| Flatpak | `docs/LINUX_PACKAGING.md` | Explicitly deferred because FUSE mounts and host-emulator launching are not designed for the sandbox. |

The canonical GUI source entry point is
`crates/archivefs-gui/src/bin/emuwiz.rs`, which calls `archivefs_gui::gui_v2::run()`.
The packager verifies that the selected GUI reports `GUI v2` before packaging
(`validate_gui_identity` in `package_release.py`).

### Actual current tar.xz layout

The current packager stages approximately:

```text
emuwiz-0.9.0-linux-x86_64/
  bin/emuwiz                 0755
  bin/emuwiz-cli             0755
  docs/README.txt            0644
  docs/LICENSES.txt          0644
  docs/licenses/LICENSE      0644
  BUILD_INFO.txt             0644
  VERIFY.txt                 0644
  manifest.json              0644
  SHA256SUMS                 0644
  .emuwiz-release-package.json
  SBOM/...                   optional
```

The exact payload is determined by `write_generated_docs`, `copy_license_payload`,
and the manifest construction in `scripts/release/package_release.py`. It does
not copy the repository `install.sh`, `config.toml.example`, desktop metadata,
or icons. It is therefore directly launchable after extraction as
`bin/emuwiz`, but it is not installable by the documented `./install.sh` path.

## FRESH START BEHAVIOR

### No config, database, sources, or emulator

The path resolver is intentionally non-mutating (`crates/archivefs-core/src/app_dirs.rs`):

- fresh config resolves to `~/.config/emuwiz/config.toml`;
- fresh data resolves to `~/.local/share/emuwiz`;
- legacy `archivefs` roots are reused when they already exist;
- resolving paths creates no files.

The database is `library.sqlite3` below the effective data root and is created
only by `Database::open_or_create`, not by path resolution
(`crates/archivefs-core/src/database.rs`). A pristine CLI `config-check` is
designed not to create config or database state (`scripts/qa/release-smoke.sh`).

The GUI-v2 startup path gathers a read-only environment snapshot in
`crates/archivefs-gui/src/gui_v2/environment.rs`. Missing config, missing
database, zero sources, absent DAT data, and no discovered emulators are
represented as setup state rather than treated as a corrupt library. The home
page receives `first_run` from `setup_controller::missing_config_is_first_run`.
The setup page explicitly says that missing config is expected, offers “Create
Starter Config”, and tells the user to add a source folder
(`crates/archivefs-gui/src/setup_controller.rs`).

The first-run experience is therefore usable in principle: the GUI can open,
show onboarding/setup guidance, and remain useful for configuration and local
inspection without a source or emulator. No evidence was found that a missing
optional provider, DAT source, or emulator prevents the GUI from opening.

### No network

The core startup/environment projection is local and read-only. Network-backed
identity, metadata, RomM, and cheat/provider features are separate workflows.
The GUI labels optional provider state rather than requiring it. The release
smoke harness runs with `env -i`, no provider commands, and explicitly records
network-disabled operation (`scripts/qa/release-smoke.sh`).

The offline first launch can still configure sources, inspect local files, view
the catalogue, run local diagnostics, and prepare local plans. Provider-backed
enrichment, managed DAT refreshes, remote RomM access, and online cheat/catalogue
refreshes are unavailable or cache-dependent; they should remain visibly
optional rather than look like startup failures.

### No display server

The GUI is a desktop application and requires X11 or Wayland (`README.md`). The
CLI remains the headless release smoke path. The packaged GUI `--version` path
is intentionally tested without opening a window, but that does not demonstrate
full GUI startup without a display.

## RUNTIME DEPENDENCIES

### Required for the GUI to open

The tarball contains dynamically linked native binaries but no runtime library
bundle. The release manifest records ELF `NEEDED` names as informational only;
the packager explicitly says they do not prove availability on another Linux
installation (`package_release.py`). A release host therefore needs:

| Requirement | Evidence / impact |
| --- | --- |
| Linux x86_64 for the current tar.xz/AppImage lanes | Packager architecture validation and AppImage builder currently enforce x86_64; ARM64 is described for package lanes, not verified here as a tarball GUI artifact. |
| glibc, libgcc, libm, dynamic loader | Native ELF runtime dependencies. |
| X11 or Wayland desktop session | `README.md`; GUI uses eframe/winit. `crates/archivefs-gui/Cargo.toml` enables both `wayland` and `x11` eframe features. |
| Working OpenGL/EGL/GPU-compatible graphics stack | eframe/glow/winit dependency set; not bundled by AppImage or tarball. Software/driver failure is host-specific and was not claimed as universally supported. |
| Writable user config/data locations | Required when the user creates config, opens/creates the catalogue, or performs local workflows. |

The current release machinery does not provide a distro-specific dependency
preflight for the tarball. A user can therefore extract a valid artifact and
still receive a host-library or display failure when opening the GUI.

### Required only for specific workflows

| Capability | External requirement | Effect if absent |
| --- | --- | --- |
| Read-only archive mounting | `ratarmount`, Python environment, FUSE support, and `fusermount3` or `umount` | Mount workflows unavailable; startup should remain usable. `docs/LINUX_PACKAGING.md` classifies this as non-startup configuration. |
| 7z archive handling | `7z`/`7zz` according to distro | Relevant archive inspection/mount path unavailable. |
| RAR handling | `rar`/`unrar` as supported by the specific path | RAR-specific operations unavailable. |
| Opening manuals/guides or external files | `xdg-open` and a desktop-associated viewer | Open action unavailable; local catalogue remains usable. |
| Emulator launch | User-installed native executable, AppImage, Flatpak, or explicit path as supported by the adapter | Launch readiness reports unavailable/not configured; EmuWiz is not an emulator bundle. |
| RomM/provider workflows | Reachable configured service and credentials where required | Provider features unavailable; local identity/evidence remains separate. |

No evidence shows that archive readers, emulator binaries, PDF viewers, or
provider credentials are bundled in the tarball/AppImage. This is correct for a
local-first library tool, but the release UI and install notes must keep these
feature-specific gaps distinct from “EmuWiz will not start”.

## OPTIONAL DEPENDENCIES

Optional dependencies include `ratarmount`, FUSE, `7z`, `unrar`, `xdg-open`,
emulator executables, emulator AppImages, Flatpak-installed emulators, provider
services, and online metadata/DAT/cheat sources. The diagnostics path currently
reports missing ratarmount/unmount tools as errors in the doctor report even
though the packaging documentation says they are non-blocking for startup
(`crates/archivefs-core/src/lib.rs` setup diagnostics and `docs/LINUX_PACKAGING.md`).
This is a wording/priority issue, not evidence of a startup crash: the GUI can
still open, but a novice may read an archive-mount doctor error as a broken
installation.

## FILESYSTEM LOCATIONS

| Data | Current location/evidence | Fresh-install assessment |
| --- | --- | --- |
| Config | `$XDG_CONFIG_HOME/emuwiz/config.toml`, otherwise `~/.config/emuwiz/config.toml`; existing legacy `archivefs` root wins when present. | Good compatibility behavior; first-run path is predictable. |
| Catalogue database | `$XDG_DATA_HOME/emuwiz/library.sqlite3`, otherwise `~/.local/share/emuwiz/library.sqlite3`; legacy fallback applies. | Good; path resolution is non-mutating. |
| Managed identity/provider data | Under the effective data root, with subsystem-specific paths such as `identity/` and managed DAT state. | Must remain documented as user data/cache, not release payload. |
| Caches/artwork | Subsystem-specific cache roots under XDG cache/data or explicit roots; artwork and provider caches are bounded/read-only or explicitly managed by their workflows. | Optional and recoverable; missing cache should not block first start. |
| Journals/history/backups | Workflow-specific directories under effective data/config roots, including rename/shared transaction history and database backups. | Important for recovery; upgrade tests and docs should name these locations clearly. |
| Temporary files | `tempfile` or destination-adjacent transactional staging in relevant workflows; no release payload dependency. | No production `/home/davedap` dependency found in the audited startup/release paths. |
| Installer ownership manifest | `$XDG_DATA_HOME/emuwiz-installer/manifest` from `install.sh`, separate from application data. | Good separation; only applies when `install.sh` is actually shipped. |
| AppImage state | Normal EmuWiz XDG resolution, not AppDir; `AppRun` does not rewrite HOME/XDG/PATH. | Good portability property. |

The installer explicitly protects foreign files and uses content digests for
ownership, but the current tar.xz packager omits that installer. This makes the
ownership and upgrade guarantees unavailable through the current canonical
packager output unless the user installs from a separately obtained script.

## OFFLINE-FIRST RESULT

**Result: usable but not release-clear.** Local startup, configuration, source
registration, scanning, identity from local evidence, database inspection, and
read-only diagnostics do not require network access. Online providers and
managed refreshes are optional workflows. The release smoke harness deliberately
tests isolated offline CLI behavior, but there is no equivalent mandatory
packaged GUI smoke gate: `run-rc-acceptance.py` makes GUI smoke optional unless
`--require-gui-smoke` is supplied.

## EMULATOR DISCOVERY

Discovery is adapter-specific and largely read-only. Current code combines:

- bounded `$PATH`/known-standard-path discovery for supported native emulators;
- known Flatpak application metadata where available;
- caller-configured executable/profile roots;
- managed AppImage evidence only when already validated;
- explicit environment/config roots for adapters such as mGBA, Dolphin,
  PCSX2, Flycast, Azahar, Cemu, and others.

The GUI controller calls the relevant `...DiscoveryRoots::from_environment()`
and profile discovery functions (`crates/archivefs-gui/src/emulator_setup/controller.rs`).
The code does not use the audit worktree or `/home/davedap` as a production
default. Developer-like absolute paths appear in tests and comments only, not
as release discovery roots.

The main fresh-user limitation is discoverability, not path leakage: an
emulator absent from PATH or outside the adapter's known roots may appear as
missing until the user supplies an explicit path. The setup UI must make that
next step clear and must distinguish “not installed” from “not found by this
adapter”.

## ERROR UX

Evidence-backed strengths:

- missing config is explicitly framed as expected first run;
- config and data root paths are shown in setup diagnostics;
- diagnostics provide “why it matters” and “next step” fields;
- CLI config-load errors append an actionable `config-check`/starter-config
  hint when the file is genuinely absent (`crates/archivefs-cli/src/main.rs`);
- database health and migration state are surfaced through the environment and
  Doctor projections;
- release verification failures name the archive, checksum, manifest, path,
  or provenance mismatch.

Remaining clarity issues:

- a missing `ratarmount`/unmount tool is technically a feature-specific
  limitation but may be shown as an error in Doctor, without an equally
  prominent “EmuWiz itself can still start” sentence;
- missing dynamic graphics/display libraries are outside the artifact's
  current preflight and will be reported by the host loader/window backend;
- an extracted canonical tarball has no obvious top-level launcher or installer
  because `install.sh` is absent;
- the stale release documentation makes a correct checksum/download choice
  difficult before any application error can be shown.

## UPGRADE SAFETY

The codebase has meaningful upgrade primitives:

- schema versioning and migrations in `crates/archivefs-core/src/database.rs`;
- read-only diagnostics for database schema and migration readiness;
- retained database backup and restore/rollback APIs;
- legacy ArchiveFS directory reuse without automatic copying or overwriting;
- installer ownership manifest and foreign-file refusal in `install.sh`;
- release packager refuses unsafe/unowned output replacement and verifies
  manifests/checksums before archive acceptance.

The release acceptance harness exercises schema 19/20/21 upgrade preflight and
database integrity (`scripts/release/run-rc-acceptance.py`). It does not, from
the source audit alone, prove every real historical release migration or a
full GUI upgrade. No destructive downgrade behavior should be promised.

The biggest upgrade risk is release-path inconsistency: the packager's current
tar.xz payload does not include the installer whose manifest protects per-user
binary/desktop/icon replacement. A user manually replacing `bin/emuwiz` has no
installer ownership transaction unless they separately use the repository
script.

## PORTABILITY

Evidence supports these bounded statements:

- Linux is the supported OS (`README.md`); macOS and Windows are not supported.
- eframe is built with both X11 and Wayland features, so both are intended
  integration paths, but no full distro/session matrix is proven here.
- AppImage is x86_64-only in the current builder and uses host graphics/FUSE
  support.
- Ubuntu 22.04/24.04, Fedora/Nobara, KDE/Wayland, X11, and Nvidia systems are
  not all demonstrated by this source-only audit. Existing packaging notes
  contain Ubuntu 24.04/Fedora 41 historical dependency QA, but that is not a
  current universal compatibility claim.
- Nvidia behavior depends on the host OpenGL/EGL/Wayland/X11 stack and is not
  covered by the tarball verifier.

## PACKAGING OPTIONS

| Format | Strength | Fresh-user risk/status |
| --- | --- | --- |
| tar.xz | Small, transparent, unsandboxed, works with arbitrary source roots and host emulators. | Keep viable, but reconcile the payload with `install.sh`/docs before public release. |
| AppImage | Convenient single executable; extract-and-run fallback exists when AppImage FUSE is unavailable. | Current x86_64 lane; still requires host graphics, FUSE/runtime fallback, archive tools, and emulators. |
| DEB/RPM | Native dependency metadata and desktop integration. | Existing lanes need release-current artifact verification; not a substitute for one canonical cross-distro contract. |
| Flatpak | Strong sandboxed distribution model. | Correctly deferred until FUSE visibility and host-emulator launching are designed and tested. |

For the next release, tar.xz can remain primary if it is made internally
consistent. AppImage is a useful parallel convenience artifact. Do not claim
Flatpak portability until its filesystem/mount/launch architecture is solved.

## SECURITY / RELEASE TRUST

Current release tooling provides strong building blocks:

- per-file `SHA256SUMS` and archive `.tar.xz.sha256` sidecar;
- strict path, symlink, executable-mode, ELF architecture, secret-marker, and
  manifest checks;
- optional CycloneDX 1.5 SBOM bundle with provenance and `Cargo.lock` hash;
- optional detached GPG signature over `SHA256SUMS` with public-key output;
- `BUILD_INFO.txt` containing source commit, packaging tool SHA, compiler/tool
  versions, GUI entrypoint, and artifact hashes;
- release verification that inspects extracted files without executing them.

The trust gap is operational rather than cryptographic: signatures are
optional, public-key distribution is not established by the source tree, and
the normal README quick-install path does not instruct users to verify a
detached signature or SBOM. Checksums are available, but checksum authenticity
still depends on the release channel.

## P0 RELEASE BLOCKERS

1. **Canonical install path is broken/inconsistent.** `README.md` and
   `docs/RELEASE_ENGINEERING.md` instruct users to download an `archivefs-...tar.gz`,
   extract it, and run `./install.sh`. `package_release.py` currently emits an
   `emuwiz-...tar.xz` whose root has no `install.sh`, config example, icons, or
   desktop file. A fresh user following the maintained instructions cannot
   complete the documented install. This is an actual contract failure, not a
   theoretical dependency concern.
2. **No single release artifact contract is authoritative.** The tar.xz
   packager, release engineering document, README, and AppImage lane disagree
   about names/layout. Public release publication should be blocked until one
   artifact shape and verification/install procedure is selected and tested.

## P1 RELEASE ISSUES

1. **GUI host dependency preflight is incomplete.** Tarball/AppImage users do
   not receive distro dependency metadata; display/OpenGL/EGL failures can occur
   before EmuWiz's own error UX.
2. **Feature-specific missing tools can look like installation failure.**
   Ratarmount/FUSE/7z/RAR/xdg-open/emulators are optional by workflow but need
   consistently non-blocking wording in first-run/Doctor surfaces.
3. **GUI packaged smoke is optional in the RC harness.** A release can pass the
   default acceptance run without proving a packaged GUI first frame. A serious
   GUI release should run `--require-gui-smoke` on a host with a known display
   harness, or explicitly publish the limitation.
4. **Signature/SBOM verification is not in the novice quick path.** The
   cryptographic/provenance machinery exists but is not surfaced in the primary
   download instructions.
5. **Architecture/session claims need a tested matrix.** Existing notes are
   useful evidence for selected Ubuntu/Fedora package checks, but do not prove
   Ubuntu 22.04/24.04, Fedora/Nobara, KDE/Wayland, X11, and Nvidia coverage as
   a release-wide claim.

## QUICK WINS

- Make `package_release.py` and release docs agree: either bundle `install.sh`,
  `config.toml.example`, desktop/icon assets and use the documented layout, or
  rewrite all instructions to extract and run `bin/emuwiz` plus a separate
  installer procedure.
- Replace old `archivefs-...tar.gz` commands in README/release docs with the
  actual canonical artifact name/compression, or provide compatibility aliases
  deliberately and verify both.
- Add one short “Requirements” block to the artifact README: Linux, X11/Wayland,
  graphics stack, writable XDG roots, and optional archive/emulator tools.
- Make `ratarmount` and unmount-tool findings explicitly “archive mounting is
  unavailable; EmuWiz can still start”.
- Make packaged GUI smoke required for release candidates when the harness is
  available, and record a clear skipped reason otherwise.
- Put source commit, version, checksum, SBOM presence, and signature status in
  the public release instructions.

## TEST MATRIX

| Case | Evidence/status |
| --- | --- |
| Fresh config path resolution | Covered by `app_dirs` tests: fresh EmuWiz roots, legacy fallback, both-root precedence. |
| No-state CLI first start | Covered by `scripts/qa/release-smoke.sh` design; not executed here because it creates `/tmp/emuwiz-*` disposable state. |
| Fresh GUI `--version` | Packager validates GUI-v2 version probe; full GUI launch requires a display. |
| Empty sources/no database/no emulator | Read-only GUI environment and first-run setup code inspected; no production mutation required. |
| Offline local startup | Release smoke uses `env -i` and no provider commands; source audit supports local-first behavior. |
| Tar.xz layout/checksum | Packager and `test_package_release.py` cover strict manifest/archive verification; no artifact was available in this worktree. |
| Install.sh upgrade/foreign-file protection | Installer source contains digest-backed ownership and refusal rules; current tar.xz payload omission is the blocker. |
| SBOM | `generate-sbom.py`, `verify-sbom.sh`, and packager integration inspected; inclusion is optional unless `--require-sbom` is used. |
| Detached signature | `release_signing.py` and packager options inspected; signing is optional and requires external key/public-key distribution. |
| GUI first frame | Optional packaged GUI smoke in `run-rc-acceptance.py`; not run in this read-only audit. |
| Ubuntu/Fedora/KDE/Nvidia matrix | Existing historical packaging notes only; no new claims made. |

## RECOMMENDED RELEASE-GATE CHECKLIST

1. Select one canonical artifact contract and update packager, README, release
   engineering docs, installer, and release names together.
2. Build from a clean intended commit with pinned toolchain and
   `SOURCE_DATE_EPOCH`.
3. Require the packaged directory and extracted tar.xz to pass strict manifest,
   checksum, path, permission, architecture, and version verification.
4. Include and test the installer if the release contract promises installation;
   otherwise document the exact direct-launch/install steps.
5. Generate and verify the SBOM; record whether it is included in the public
   artifact.
6. Sign `SHA256SUMS` when the release channel has a documented public-key
   distribution path; otherwise state that signatures are unavailable.
7. Run isolated offline CLI smoke and packaged GUI smoke, requiring GUI smoke
   for a release candidate whenever Xvfb/display infrastructure is available.
8. Verify first launch with no config, database, sources, DATs, providers, or
   emulators; confirm the user sees a clear next step.
9. Verify an existing legacy ArchiveFS root, current EmuWiz root, and both-root
   conflict behavior without moving or deleting user data.
10. Test schema upgrade from every supported prior schema with backup and
    recovery evidence; do not claim downgrade support.
11. Exercise missing optional tools and an unwritable config/data root, checking
    that messages say what is unavailable and what remains usable.
12. Publish exact tested platform scope rather than implying universal Ubuntu,
    Fedora, KDE, Wayland, X11, or Nvidia support.

