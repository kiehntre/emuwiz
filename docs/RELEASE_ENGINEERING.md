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
SBOM/*                     SBOM and third-party licence texts (always from build-release.sh)
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
`BUILD_INFO.txt` records source/build/GUI identity. `scripts/build-release.sh`
always generates and packages `SBOM/`, whose `THIRD_PARTY_LICENSES.txt` holds
the licence texts and font notices; texts a published crate does not carry come
from `scripts/release/licence-supplements.json` (upstream file at the crate's
recorded commit, SHA-256 pinned), and any that could not be sourced are listed
there by name. A detached signature is available only when the
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

### Release integrity and upgrade preservation

**Integrity.** When the extracted tree has `SHA256SUMS` (every canonical
package does), `install.sh` checks every record before it creates or changes
anything: strict `<sha256>  <path>` records with no duplicates, relative paths
without `.`/`..`/empty components, no symlink in any path component, regular
files only, matching digests, and records for `install.sh` and both binaries.
The binaries are copied into a private `$TMPDIR` snapshot while being hashed,
and only those bytes are installed. A tree with `manifest.json` but no
`SHA256SUMS` is always refused as damaged, whatever flags are given.

A tree with neither file (a workspace `target/release` build or an old bundle)
is refused by default with nothing installed. Developers can install such a
build with the separate, explicit `--developer-unchecked-install` flag. It
never activates automatically, is ignored for a tree that has `SHA256SUMS`
(which is always verified), prints a prominent `WARNING: DEVELOPER UNCHECKED
INSTALL` on stderr, and skips only the digest comparison: the binaries must
still be regular files reached without following any symlink, and are
installed from the private snapshot, so a workspace whose `target/` is itself a
symlink is refused too (run the installer from the real target tree instead).
The flag has no effect with `--uninstall`. Previous-version preservation
applies in every mode. This catches
damaged or modified packages; it is not a signature check, because
`install.sh` comes from the same tree. Verify the downloaded archive with its
`.sha256` sidecar (or `scripts/release/verify-release.sh --strict`) before
extracting it.

**Previous version.** Owned binaries are updated in place, which is not
atomic. Before the first publication of a run, every owned binary whose bytes
will change is copied to
`$XDG_DATA_HOME/emuwiz-installer/previous/<slot>.<old sha256>` (fresh name,
fsync, re-hash, no-replace link) and journalled in
`emuwiz-installer/upgrade-pending` (fsynced). The in-place write is refused
unless that copy and journal line exist. The journal is removed only after the
new ownership manifest is written and the new binaries and manifest are
fsynced. `--uninstall` refuses, removing nothing, while a journal is pending. Kept copies are never deleted by the
installer, including by `--uninstall`; remove them yourself once the new
version works. Each successful upgrade, and `--uninstall`, prints the kept
directory, how many files it holds and roughly how much disk they use.
Automatic deletion stays disabled by owner decision. Explicitly confirmed
retirement of obsolete kept versions is recorded as a possible later
enhancement; there is no pruning or retention subsystem today.

The upgrade refuses with nothing installed when the copy cannot be made
safely: less free space than the copy plus 16 MiB (or than a larger new binary
needs), a filesystem without working `fsync`, an unexpected file at the kept
name, or a `previous` directory that is a symlink or writable by others.

**Interrupted upgrade.** The next run checks the journal before anything else.
A destination back at the previous digest, or holding the new digest that the
completed manifest already records, is resolved and the run continues.
A destination that is partly written, or holds the new version without a
manifest record, stops the run (exit 1, nothing changed) with the exact
restore steps, for example:

```
cp -- '/home/me/.local/share/emuwiz-installer/previous/bin-emuwiz.<sha256>' '/home/me/.local/bin/emuwiz'
sha256sum -- '/home/me/.local/bin/emuwiz'    # must print <sha256>
```

After restoring, run the installer again and it continues the upgrade. If the
kept copy is missing or changed, or the destination is now a symlink, a
directory or unreadable, the installer says so and prints no command.
It never restores automatically. If the ownership manifest itself was
truncated by an interruption, the existing strict-parser refusal still
applies after restoring (`--replace-foreign` then moves the restored binaries
aside before installing).

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

After linking, the installer verifies that the configured pathname remains
readable and reaches the same pinned directory and published inode. It compares
each directory/symlink binding before traversing it, retaining existing safe
symlink paths while refusing to traverse a substituted component. Missing,
different or unverifiable bindings produce an **UNCERTAIN** incomplete-install
outcome, never success; already installed assets and the linked config remain
untouched. Inspect the configured path and original directory before retrying.
These post-publication observations cannot guarantee that a pathname stays
unchanged against a concurrent external writer after it has been checked.

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

**Configuration directory.** Each missing configuration directory or parent is requested with private
`0700` access bits at creation, under
whatever umask the caller has: every missing component is passed an explicit
`0700` creation mode, so there is no later `chmod` and no window. Relying only
on `umask 077` is insufficient when a parent has a default ACL. The explicit
mode gives inherited named-user/group access entries an empty effective mask;
the installer checks the new directory's mode bits and refuses if the ACL also
restricts its owner below `0700`. Regression validation also
uses `getfacl` to check effective permissions, including named entries, on each
new parent/leaf and confirms existing ACLs remain unchanged. If `setfacl` or
`getfacl` is absent, the test explicitly reports that ACL coverage is unavailable;
those tools are not production installer requirements. This matters because the
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


**Installer test watchdog.** The complete controlling-terminal re-invocation
retains every assertion and uses a finite 1800-second overall budget for loaded
runs, plus a 120-second no-output progress watchdog. A no-progress timeout,
exhausted overall budget and functional failure are reported distinctly. The
complete inner log is retained in disposable TMPDIR; inner and outer outcomes
include exit codes and elapsed times. Watchdog regressions cover slow progress,
no progress, continuing progress at the total bound and ordinary nonzero exits.
Output is a progress signal, not proof of useful work; the overall bound remains
finite even if a stuck subprocess emits output. No assertion is weakened.

**Known validation limits.** The remaining reviewed error-reporting defects
(combined fsync/cleanup errors misattributed to interruption, missing-interface
tracebacks and an existing-config symlink warning) are unchanged. Crash durability
of the directory entry is not established, and there is no real unsupported
filesystem/host matrix; injected syscall refusals do not establish that coverage.

Release-candidate checks are orchestrated by
`scripts/release/run-rc-acceptance.py`. The release gate does not silently
turn unavailable GUI smoke into a pass.

## Historical compatibility

EmuWiz retains runtime compatibility with legacy ArchiveFS directories and
names where required for migration. The old `archivefs-*.tar.gz` naming is
historical only and must not appear in current quick-install instructions.
