# EmuWiz release engineering contract

This is the authoritative release contract for the transparent Linux
artifact. Other packaging lanes (AppImage, DEB/RPM, and the deferred Flatpak
lane) must not redefine this contract.

## Canonical artifact

The canonical artifact is:

```text
emuwiz-<version>-linux-<arch>.tar.xz
emuwiz-<version>-linux-<arch>.tar.xz.sha256
```

The archive has exactly one top-level directory named like the artifact. It
is reproducibly staged when `SOURCE_DATE_EPOCH` and `--reproducible` are used.
The maintained packager is `scripts/release/package_release.py`, normally
invoked by `scripts/release/package-release.sh` or `scripts/build-release.sh`.

## Payload contract

Every canonical archive contains, with manifest and checksum coverage:

```text
bin/emuwiz                 native GUI-v2 executable
bin/emuwiz-cli             CLI executable
install.sh                 safe current-user installer
config.toml.example        reference only; never overwrites real config
assets/linux/*.desktop.in  canonical desktop template
assets/branding/*.png      canonical application icons
docs/README.txt            extracted-artifact instructions
docs/LICENSES.txt
docs/licenses/*
BUILD_INFO.txt
VERIFY.txt
manifest.json
SHA256SUMS
SBOM/*                     optional, only when generated and declared
```

The packager rejects symlinks, special files, unsafe paths, forbidden user
state, and unexpected executable payload files. It also records GUI-v2
identity, architecture, source provenance, and artifact hashes.

## User paths

The archive is directly usable after extraction:

```sh
./bin/emuwiz
./bin/emuwiz-cli --version
```

Direct run uses normal XDG config/data/cache/state locations, does not rewrite
`HOME`, does not install files, does not require root, and does not require a
network connection.

The bundled installer is the supported install path:

```sh
./install.sh
```

It performs a per-user install under `~/.local/bin` and XDG application/icon
directories, keeps an ownership manifest, refuses foreign files, preserves
existing configuration, and supports safe upgrade/uninstall boundaries. A
system-wide install is not part of this contract. The installer works from
the extracted `bin/` layout and does not require Git metadata, a repository
checkout, or build directories.

## Runtime requirements

Required to start the current canonical Linux artifact: the published Linux
architecture (currently x86_64), a standard ELF runtime, X11 or Wayland, a
functional host graphics/OpenGL/EGL stack, and writable user XDG locations
when EmuWiz creates state.

Optional feature-specific tools include ratarmount/FUSE, 7z, unrar,
xdg-open/viewers, emulator executables, AppImages, Flatpak emulators,
RomM/providers, and online DAT/metadata services. Missing optional archive
tools do not prevent EmuWiz itself from starting.

## Trust and verification

Users verify the downloaded sidecar before extraction:

```sh
sha256sum -c emuwiz-<version>-linux-<arch>.tar.xz.sha256
```

`SHA256SUMS` verifies every extracted payload file except itself; the
manifest is represented in `SHA256SUMS` to avoid a self-hash cycle.
`BUILD_INFO.txt` records source/build/GUI identity. An SBOM is included only
when generated and declared. A detached signature is available only when the
release was signed; signing is not claimed for every artifact.

## Release gate

Before publication:

1. build the workspace with the locked Cargo graph;
2. package the canonical tar.xz and sidecar;
3. run strict extracted-directory and archive verification;
4. confirm `bin/emuwiz --version` and `bin/emuwiz-cli --version`;
5. run isolated installer tests and offline CLI smoke;
6. run packaged GUI first-frame smoke when a display/Xvfb harness exists;
7. record `SKIPPED: <reason>` when GUI smoke cannot run;
8. confirm no source-tree-only path or network dependency is required.

Release-candidate checks are orchestrated by
`scripts/release/run-rc-acceptance.py`. The release gate does not silently
turn unavailable GUI smoke into a pass.

## Historical compatibility

EmuWiz retains runtime compatibility with legacy ArchiveFS directories and
names where required for migration. The old `archivefs-*.tar.gz` naming is
historical only and must not appear in current quick-install instructions.
