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

The installer resolves the starter-config directory exactly as the
application does: `EMUWIZ_CONFIG_HOME` (the directory itself), else
`XDG_CONFIG_HOME/emuwiz`, else `~/.config/emuwiz` (or legacy
`~/.config/archivefs` when only that exists). A set but non-absolute override
is refused by both: the installer then installs binaries, writes no config and
exits non-zero. The starter config is created with a no-clobber link and never
replaces or writes through an existing file or dangling symlink.

### Installer requirements and failure behaviour

`install.sh` is Linux-only. Publishing the release assets needs Python 3,
procfs, `O_PATH` (Linux 2.6.39+) and descriptor-relative hardlink support on
the install filesystem. Writing the **starter config** additionally needs:

- Python 3 with `os.O_TMPFILE`, `os.O_DIRECTORY`, `os.O_CLOEXEC` and `ctypes`
  (for `linkat`). A Python without `os.O_TMPFILE` (non-Linux builds, very old
  versions) is detected up front and refused by name.
- A configuration-directory filesystem that supports `O_TMPFILE` anonymous
  files and hardlinks. Some network and FUSE filesystems do not; the kernel then
  reports `EOPNOTSUPP`, `EISDIR` or `EINVAL`.
- A mounted `/proc`: the anonymous file is published with
  `linkat(AT_SYMLINK_FOLLOW)` through `/proc/self/fd/N`.

Publication is atomic and no-clobber: the kernel refuses the link when the name
exists (a dangling symlink included), so a racing writer's file is never
replaced. There is no staging pathname and nothing is ever unlinked, and there
is no fallback to a named temporary file, to overwriting, or to elevated
privileges.

If the environment is unsupported, or the step fails before publication, the
starter-config step is refused with an explicit message (never a Python
traceback), `install.sh` exits 1, and everything already installed - binaries,
launchers, icons and the ownership manifest - remains in place. Create the
config yourself by copying `config.toml.example`. If the installer is signalled
(SIGINT, SIGTERM or SIGHUP) it exits 130 and states whether the starter config
was published (and is complete), was not published, or could not be
established; it never reports that nothing changed when the file was published.

**Permissions.** A newly written starter config is mode `0600` (owner
read/write only). It is created with that mode by `O_TMPFILE`, and the result is
the same under `umask 000`, `002`, `022`, `027` and `077`. Earlier installers
produced the umask-dependent default, typically `0644`. An existing config is
never modified, replaced or re-permissioned. (A umask that removes the owner's
own write or search bit, such as `0277`, breaks directory creation for the whole
installer, not only the config.)

**Configuration directory.** Any configuration directory the installer has to
create (and any missing parent) is created mode `0700` at creation, under
whatever umask the caller has: the creation runs under `umask 077` in a
subshell, so there is no later `chmod` and no window. This matters because the
file's `0600` does not protect it against a directory that other users can write
to: they could delete and replace `config.toml`, or plant one first. An
**existing** directory is never chmod-ed, re-owned or moved. Before publishing,
the installer judges the directory on the pinned descriptor it publishes into
(so symlinked paths are judged on their real target) and:

- refuses, changing nothing and exiting non-zero with the assets left in place, a
  directory not owned by the installing user or writable by every user (any
  `o+w`, sticky or not, because sticky does not stop a pre-created file);
- continues with a warning for a group-writable directory (for example `0775`,
  common under `umask 002`), since a private per-user group cannot be told from a
  shared one here. Members of that group can replace `config.toml`; tighten it
  yourself if the group is shared.

If a config already exists the installer leaves it untouched and, on stderr
only, warns when its directory is writable by every user or not owned by you, or
the file is not owned by you.


Release-candidate checks are orchestrated by
`scripts/release/run-rc-acceptance.py`. The release gate does not silently
turn unavailable GUI smoke into a pass.

## Historical compatibility

EmuWiz retains runtime compatibility with legacy ArchiveFS directories and
names where required for migration. The old `archivefs-*.tar.gz` naming is
historical only and must not appear in current quick-install instructions.
