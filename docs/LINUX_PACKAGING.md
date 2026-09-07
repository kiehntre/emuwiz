# EmuWiz Linux packaging

This is a packaging-only lane: it produces and validates DEB/RPM packaging
inputs and (where host tooling allows) built artifacts. It never publishes a
release, tags a version, or installs anything onto the authoring host. See
each build script's `--help` for the exact flags.

Current project version: `0.8.1-alpha` (pre-1.0). Nothing here fabricates a
1.0 release; the alpha status is carried through every packaging format's own
pre-release versioning convention.

## Formats

| Format | Status | Entry point |
| --- | --- | --- |
| AppImage | Existing, supported | [`docs/APPIMAGE_PACKAGING.md`](APPIMAGE_PACKAGING.md) |
| Tarball + `install.sh` | Existing, supported | repository root `install.sh` |
| DEB (Debian/Ubuntu) | V4 artifact build and disposable install/uninstall QA passed | `packaging/debian/build-deb.sh` |
| RPM (Fedora/Nobara) | V4 artifact build and disposable install/uninstall QA passed | `packaging/rpm/build-rpm.sh` |
| Flatpak | **Deferred** | see below |

### Flatpak status

Flatpak is deferred for V1/V2. EmuWiz's core value is mounting archives
read-only via FUSE (`ratarmount`) so they can be **browsed and used by other,
host-installed emulators/tools without full extraction** - that means a
Flatpak sandbox would need either a broken-by-design read-only FUSE mount the
rest of the host cannot see, or a portal/host-FUSE integration this project
has not designed or audited. Launching host emulators from inside a Flatpak
sandbox has the same problem. Until that architecture is deliberately
designed (tracked as future work, not started here), Flatpak stays off the
table rather than shipping a build that silently breaks EmuWiz's main
feature. This status is unchanged from the AppImage-era assessment; nothing
in this lane revisits it.

## Shared assets (never duplicated)

Both DEB and RPM stage the **same** files DEB/RPM packaging never copies or
forks:

- `assets/linux/io.github.kiehntre.emuwiz.desktop.in` - the `@EMUWIZ_EXEC@`
  placeholder is rendered to `emuwiz` at package-build time (`sed`, done in
  `debian/rules` and the RPM `%install` step respectively - not a new script).
- `assets/linux/io.github.kiehntre.emuwiz.metainfo.xml` - installed verbatim.
- `assets/branding/emuwiz-logo-{32,64,128,256,512}.png` - installed, renamed,
  into `hicolor/<n>x<n>/apps/io.github.kiehntre.emuwiz.png` (debhelper's
  `.install` files cannot rename on copy, so both `packaging/debian/rules`
  and `packaging/rpm/emuwiz.spec` stage these with a small `install -Dm644`
  loop instead of a second copy of the icon set).

If a future asset changes, both packaging trees pick it up automatically -
there is nothing in `packaging/debian/` or `packaging/rpm/` to keep in sync.

## Installed layout (identical between DEB and RPM)

```
<prefix>/bin/emuwiz                                            (GUI)
<prefix>/bin/emuwiz-cli                                         (CLI)
<prefix>/share/applications/io.github.kiehntre.emuwiz.desktop
<prefix>/share/metainfo/io.github.kiehntre.emuwiz.metainfo.xml
<prefix>/share/icons/hicolor/{32,64,128,256,512}/apps/io.github.kiehntre.emuwiz.png
```

`<prefix>` is `/usr` for both DEB and the RPM `%{_bindir}`/`%{_datadir}`
macros.

## Package split

Two binary packages from one source, matching the existing workspace split
(`archivefs-gui` -> `emuwiz`, `archivefs-cli` -> `emuwiz-cli`):

- **`emuwiz`** - the GUI application. `Recommends: emuwiz-cli` (DEB) /
  `Recommends: emuwiz-cli` (RPM) - useful together, neither hard-requires the
  other, so a headless install of just the CLI stays possible.
- **`emuwiz-cli`** - the command-line tool, independently installable.

This mirrors the workspace's own crate split rather than inventing a new
grouping, and avoids needless fragmentation (no separate `-common`,
`-data`, or per-icon-size packages).

## Version mapping

Cargo workspace version (`Cargo.toml`): **`0.8.1-alpha`**.

| Format | Field(s) | Value | Why |
| --- | --- | --- | --- |
| Debian | `debian/changelog` version | `0.8.1~alpha-1` | `~` sorts *before* nothing, so `0.8.1~alpha-1 < 0.8.1-1`: an eventual final `0.8.1` release correctly supersedes this alpha in `dpkg --compare-versions`. |
| Fedora/RPM | `Version:` / `Release:` | `0.8.1` / `0.1.alpha%{?dist}` | Fedora's pre-release convention: `Release` starting `0.` sorts below the eventual `1%{?dist}` final release of the same `Version`. |

When the upstream version changes, update **both**
`packaging/debian/changelog` (new stanza) and `packaging/rpm/emuwiz.spec`
(`Version`/`Release` and a matching `%changelog` entry) - there is no
single source of truth transform yet; this is intentionally manual so a
packaging-only lane never silently reinterprets an upstream version bump.

## Runtime dependencies

Audited against actual `Command::new(...)` call sites in `crates/*/src`
(`ratarmount`, `fusermount`/`fusermount3`, `7z`, `rar`, `xdg-open`), the
`diagnostics` module's own doctor-check categorisation (`ratarmount` /
"unmount tool" is a `Configuration`-category check, i.e. EmuWiz **starts and
runs without it**), and **real package-availability queries** run inside
disposable `ubuntu:24.04`/`fedora:41` containers (packaging QA V3 -
`apt-cache policy`/`apt-cache search` and `dnf list --available`, not
guessed):

| Tool | DEB name (confirmed) | RPM name (confirmed) | Why |
| --- | --- | --- | --- |
| `ratarmount` | **not packaged** (Suggests only) | **not packaged** (Suggests only) | Confirmed absent from both Ubuntu 24.04's apt repos (`apt-cache search ratarmount` -> zero hits) and Fedora 41's dnf repos (`dnf list --available ratarmount` -> "No matching packages"). It is pip-installable (`pip3 install ratarmount`) - `python3-pip` is listed in `Recommends` on both formats as the practical path, since a `Recommends`/`Suggests` on a name apt/dnf can never resolve is misleading metadata even though it doesn't break the install. Read-only archive mounting is a core *feature*, not a startup requirement (see doctor-check note above), so this stays non-blocking either way. |
| `fuse3` | `fuse3` | `fuse3` | Confirmed present, exact name, both distros. `ratarmount`'s mount backend. |
| 7z support | `7zip` \| `p7zip-full` | `p7zip` + `p7zip-plugins` | Confirmed: Ubuntu 24.04's `p7zip-full` is now a transitional dummy package pulling in `7zip` (real successor, in `universe`); Fedora 41 has **not** made that switch and only ships `p7zip`/`p7zip-plugins` (`7zip` itself: "No matching packages" on `dnf`). DEB and RPM correctly differ here. |
| `unrar` | Suggests | Suggests | Confirmed present on both (Ubuntu `1:7.0.7-1build1`, Fedora `0.3.1-1.fc41`); optional, non-free-adjacent, genuinely a "suggest". |
| `xdg-utils` | `xdg-utils` | `xdg-utils` | Confirmed present, exact name, both distros. Opening files/links via the desktop environment; not required to start. |

Nothing here is a hard `Depends:`/`Requires:` beyond what `dh_shlibdeps` /
RPM's automatic ELF dependency generator adds for the binaries themselves
(`${shlibs:Depends}`, `${misc:Depends}` on DEB; RPM's find-requires on the
spec side). V4 inspected the generated dependency metadata in disposable
Ubuntu 24.04 and Fedora 41 containers.

## DEB packaging

```
packaging/debian/
├── build-deb.sh          # wrapper: stages an isolated git-archive copy, runs dpkg-buildpackage
├── changelog              # version mapping lives here (0.8.1~alpha-1)
├── control                 # emuwiz + emuwiz-cli binary packages
├── copyright               # DEP-5, mirrors repository LICENSE (MIT)
├── emuwiz.install           # target/release/emuwiz -> usr/bin
├── emuwiz-cli.install       # target/release/emuwiz-cli -> usr/bin
├── rules                    # debhelper 13 (dh sequencer); builds both bins,
│                             #   renders the desktop file, stages metainfo + icons
└── source/format             # "3.0 (native)"
```

Build:

```sh
CARGO_BUILD_JOBS=2 packaging/debian/build-deb.sh --output-dir "$PWD/dist"
```

`build-deb.sh` packages `git archive HEAD` into a private temp directory -
**never** the working tree's uncommitted changes - so it is safe to run
alongside any other in-progress, uncommitted lane in this repository. Output:
`emuwiz_0.8.1~alpha-1_amd64.deb`, `emuwiz-cli_0.8.1~alpha-1_amd64.deb`, plus
`.buildinfo`/`.changes`.

Root-level `debian/`: not created. `dpkg-buildpackage` conventionally wants
`debian/` at the tree root, so `build-deb.sh` copies
`packaging/debian` -> `<isolated tmp copy>/debian` before building - the
real working tree is never touched, and no root-level `debian/` is ever
committed.

## RPM packaging (Fedora/Nobara)

```
packaging/rpm/
├── build-rpm.sh    # wrapper: git-archive source tarball + rpmbuild --define _topdir <tmp>
└── emuwiz.spec      # emuwiz + emuwiz-cli subpackage, same install layout as DEB
```

Build:

```sh
CARGO_BUILD_JOBS=2 packaging/rpm/build-rpm.sh --output-dir "$PWD/dist"
```

Also builds from `git archive HEAD` into a private `%_topdir`, never the
working tree.

### Source / build model (Task B3)

This is a **controlled Cargo build**, matching the project's existing local
release practice (the AppImage build also runs a plain `cargo build
--release` - see `packaging/appimage/build-appimage.sh`): `cargo build`
fetches crates.io as normal during `%build`. It does **not** vendor
dependencies and does **not** produce an offline/reproducible source RPM
suitable for Fedora's official (Koji/COPR) build system, which requires
either vendored crates (`cargo vendor` + a `Source1: vendor.tar.gz` /
`%cargo_prep` macro flow) or network access explicitly granted to the build
root. That vendoring step is real, non-trivial work (pinning a large
dependency tree, keeping `Cargo.lock` and the vendor tarball in sync on every
change) and is **out of scope for this V2 foundation** - it is the documented
production route: before a real Fedora/COPR submission, switch `%build` to
`%cargo_prep`/`%cargo_build` with a vendored source tarball, per Fedora's
[Packaging Rust guidelines](https://docs.fedoraproject.org/en-US/packaging-guidelines/Rust/).

## Static validation performed

- `desktop-file-validate` on the rendered `.desktop` file: **pass**.
- `appstreamcli validate --no-net` on `assets/linux/io.github.kiehntre.emuwiz.metainfo.xml`: **pass** (1 pedantic note, pre-existing, not introduced here).
- `bash -n` on `packaging/debian/build-deb.sh` and `packaging/rpm/build-rpm.sh`: **pass**.
- `shellcheck`: not installed on this host; not run. No script uses anything shellcheck commonly flags (unquoted globs, word-splitting-sensitive expansions); re-run before a production release if available.

## Package builds: real containerised attempts (packaging QA V3)

Real builds were run in disposable, network-pulled `ubuntu:24.04` and
`fedora:41` containers (never against the host - `docker run --rm`, repo
bind-mounted `:ro`, output bound to `dist/packages/`). **No package was
installed on the authoring host at any point.**

### Toolchain finding: distro `cargo`/`rustc` are too old

This workspace's `rust-toolchain.toml` pins **`1.97.1`**. Real attempts with
each distro's own packaged compiler failed **before** reaching application
code:

- Ubuntu 24.04 apt `rustc 1.75.0` -> `crates/archivefs-core/Cargo.toml`
  fails to parse: `feature 'edition2024' is required` (stabilised in
  rustc 1.85, newer than 1.75).
- Fedora 41 dnf `cargo 1.91.1` (Fedora actively backports Rust, so this
  *did* clear the edition2024 floor) -> still fails:
  `crates/archivefs-gui/Cargo.toml:25: newlines are unsupported in inline
  tables` - a Cargo.toml syntax the workspace uses that even a fairly
  recent 1.91 toolchain's TOML parser rejects.

Both `Build-Depends`/`BuildRequires` on `cargo`/`rustc` were **removed**
from `packaging/debian/control` and `packaging/rpm/emuwiz.spec` as a direct
result (a real evidence-based fix, not a guess) - listing
`rustc (>= 1.75)` was actively misleading, since a build root that
satisfies it can still fail. The exact reproduction commands below install
the pinned `1.97.1` via rustup instead.

### Debian/Ubuntu real build

```sh
docker run --rm \
  -v "$PWD:/repo:ro" -v "$PWD/dist/packages:/out" ubuntu:24.04 bash -c '
    set -euo pipefail
    apt-get update -q
    apt-get install -y -q --no-install-recommends \
      git build-essential debhelper dpkg-dev pkg-config fakeroot curl \
      libgl1-mesa-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
      libx11-dev libxrandr-dev libxi-dev libxcursor-dev ca-certificates \
      desktop-file-utils appstream
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain 1.97.1 --profile minimal
    . "$HOME/.cargo/env"
    git config --global --add safe.directory /repo
    cd /repo
    CARGO_BUILD_JOBS=2 bash packaging/debian/build-deb.sh --output-dir /out'
```

With the pinned toolchain, `dpkg-checkbuilddeps` passed cleanly and a real
`cargo build --release` ran (confirmed compiling `archivefs-core`, `eframe`,
`egui_glow`, etc.) - see the actual blocker below for why it did not finish.

### Fedora/Nobara real build

```sh
docker run --rm \
  -v "$PWD:/repo:ro" -v "$PWD/dist/packages:/out" fedora:41 bash -c '
    set -euo pipefail
    dnf install -y -q \
      rpm-build rpmdevtools git gcc gcc-c++ pkgconf-pkg-config \
      mesa-libGL-devel libxkbcommon-devel libxkbcommon-x11-devel \
      wayland-devel libX11-devel libXrandr-devel libXi-devel libXcursor-devel \
      desktop-file-utils appstream ca-certificates curl
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain 1.97.1 --profile minimal
    . "$HOME/.cargo/env"
    git config --global --add safe.directory /repo
    cd /repo
    CARGO_BUILD_JOBS=2 bash packaging/rpm/build-rpm.sh --output-dir /out'
```

Same result: with the pinned toolchain and the `cargo`/`rust`
`BuildRequires` removed, `rpmbuild`'s dependency check passed and a real
`cargo build --release` ran to the same point as the DEB build.

*(A real, in-container bug was also found and fixed during this: the
spec's `%build` set `CARGO_BUILD_JOBS=%{_smp_mflags}`, which expands to a
make(1)-style flag like `-j2`, not the bare integer Cargo expects - so the
`:-2` fallback silently never applied. Fixed to
`export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"`.)*

### Actual blocker: a pre-existing compile error at HEAD (not a packaging bug)

Both real builds reached and began compiling `archivefs-gui`, then failed
identically on:

```
error[E0027]: pattern does not mention field `status`
   --> crates/archivefs-gui/src/onframe_install_state.rs:121:13
121 |         let Self::AwaitingConfirmation { binding } = self else {
    |             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ missing field `status`
```

This is **not uncommitted working-tree drift** - it reproduces from
`git archive HEAD` (a clean checkout of the committed tree), i.e. the
workspace does not currently build `cargo build --release -p archivefs-gui`
from a fresh clone at all, independent of packaging. It was introduced in
`a967b14 refactor(gui): add Dolphin OnFrame install workflow state` and is
inside `crates/archivefs-gui/src/onframe_install_state.rs`, a GUI/cheat
workflow file this packaging-QA lane is explicitly not permitted to touch.
**No `.deb`/`.rpm` was produced.** This needs a one-line fix
(`{ binding, status: _ }` or similar) from whoever owns that file before any
real package - or any plain `cargo build --release` from a clean checkout -
can succeed.

## Package inspection / install QA: blocked in V3 (superseded)

Tasks A2/B (DEB inspection, install/uninstall QA) and D2/E (RPM inspection,
install/uninstall QA) could not run - there is no artifact to inspect or
install. `dist/packages/` is empty; nothing was faked. Once the blocker
above is fixed upstream, the exact commands to run are:

```sh
# DEB inspection + disposable install/uninstall QA
dpkg-deb --info dist/packages/emuwiz_0.8.1~alpha-1_amd64.deb
dpkg-deb --contents dist/packages/emuwiz_0.8.1~alpha-1_amd64.deb
docker run --rm -v "$PWD/dist/packages:/pkgs:ro" ubuntu:24.04 bash -c '
  apt-get update -q &&
  apt-get install -y -q binutils &&
  apt-get install -y /pkgs/emuwiz_*_amd64.deb /pkgs/emuwiz-cli_*_amd64.deb &&
  test -x /usr/bin/emuwiz && test -x /usr/bin/emuwiz-cli &&
  /usr/bin/emuwiz-cli --help &&
  ldd /usr/bin/emuwiz | grep -qi "not found" && echo MISSING || echo OK &&
  test -f /usr/share/applications/io.github.kiehntre.emuwiz.desktop &&
  test -f /usr/share/metainfo/io.github.kiehntre.emuwiz.metainfo.xml &&
  test -f /usr/share/icons/hicolor/256x256/apps/io.github.kiehntre.emuwiz.png &&
  apt-get remove -y emuwiz emuwiz-cli &&
  ! test -f /usr/bin/emuwiz'

# RPM inspection + disposable install/uninstall QA
rpm -qpi dist/packages/emuwiz-0.8.1-0.1.alpha*.rpm
rpm -qpl dist/packages/emuwiz-0.8.1-0.1.alpha*.rpm
rpm -qpR dist/packages/emuwiz-0.8.1-0.1.alpha*.rpm
docker run --rm -v "$PWD/dist/packages:/pkgs:ro" fedora:41 bash -c '
  dnf install -y /pkgs/emuwiz-0.8.1-*.rpm /pkgs/emuwiz-cli-0.8.1-*.rpm &&
  test -x /usr/bin/emuwiz && test -x /usr/bin/emuwiz-cli &&
  /usr/bin/emuwiz-cli --help &&
  ldd /usr/bin/emuwiz | grep -qi "not found" && echo MISSING || echo OK &&
  test -f /usr/share/applications/io.github.kiehntre.emuwiz.desktop &&
  test -f /usr/share/metainfo/io.github.kiehntre.emuwiz.metainfo.xml &&
  test -f /usr/share/icons/hicolor/256x256/apps/io.github.kiehntre.emuwiz.png &&
  dnf remove -y emuwiz emuwiz-cli &&
  ! test -f /usr/bin/emuwiz'
```

`lintian`/`rpmlint` remain uninstalled on the authoring host and were not
run - there is nothing to lint without a built package.

## Static validation performed (V3)

- `desktop-file-validate` on the rendered `.desktop` file: **pass**.
- `appstreamcli validate --no-net` on `assets/linux/io.github.kiehntre.emuwiz.metainfo.xml`: **pass** (1 pre-existing pedantic note).
- `bash -n` on both build scripts: **pass**.
- `shellcheck`: still not installed on this host; not run.
- `lintian`/`rpmlint`: not installed; moot without a built package (see above).

## Package builds and install QA (V4)

V4 reran the real packaging scripts after the OnFrame compile fix (`acb3fbc`)
in fresh, disposable containers. The repository was bind-mounted read-only;
only `dist/packages/` was writable. No package was installed on the authoring
host.

Toolchain and images:

- `ubuntu:24.04`, rustup Rust `1.97.1` (`rustc 1.97.1`, `cargo 1.97.1`)
- `fedora:41`, rustup Rust `1.97.1` (`rustc 1.97.1`, `cargo 1.97.1`)
- both builds used `CARGO_BUILD_JOBS=2` and the existing build scripts

Debian artifacts (`0.8.1~alpha-1`, `amd64`):

| Artifact | SHA-256 |
| --- | --- |
| `emuwiz_0.8.1~alpha-1_amd64.deb` | `f5431f30c36d7248ef3f1655cc18519a31909314c0db1a8b453a28e40a899beb` |
| `emuwiz-cli_0.8.1~alpha-1_amd64.deb` | `217c8ce359ed2459f5015fc128c43c16c3b2962c6619a669199d45f9f66d7ced` |

RPM artifacts (`0.8.1-0.1.alpha.fc41`, `x86_64`):

| Artifact | SHA-256 |
| --- | --- |
| `emuwiz-0.8.1-0.1.alpha.fc41.x86_64.rpm` | `1d163597dfda3515462b97fe983e8b67dae9cdd82c1dbad818417ff4a3096f2f` |
| `emuwiz-cli-0.8.1-0.1.alpha.fc41.x86_64.rpm` | `60402d2a486fcadf136be0c29cf05cb4caa74d8907deefe7905c46b0becddde0` |
| `emuwiz-debuginfo-0.8.1-0.1.alpha.fc41.x86_64.rpm` | `a07c61245b1f0674d30b7a6bf358b109b4ed3fe78a16d54ffd0aac2c2795e2d8` |
| `emuwiz-cli-debuginfo-0.8.1-0.1.alpha.fc41.x86_64.rpm` | `b2f048c940ee04a33cdd4098c7cc7fd9d8555a15c2d859ed9b492a0e63678404` |
| `emuwiz-debugsource-0.8.1-0.1.alpha.fc41.x86_64.rpm` | `a157cfaaef9b6cadd0e0a6bc49e11d19c05fe06c0771bd02fc93bd6eee6d0ba5` |

Inspection and install QA passed in second fresh containers for each distro:

- `dpkg-deb --info/--contents` and `rpm -qpi/-qpl/-qpR` showed the expected
  GUI/CLI split, desktop entry, AppStream metadata, icons, and documentation.
- Debian dependencies were `libc6`/`libgcc-s1`; runtime recommendations were
  `fuse3`, `xdg-utils`, `7zip | p7zip-full`, `python3-pip` (GUI also recommends
  the CLI), with `ratarmount` and `unrar` suggested.
- RPM dependencies were automatic glibc/libgcc/libm/rtld requirements;
  recommendations were `fuse3`, `p7zip`, `p7zip-plugins`, `python3-pip`,
  `xdg-utils` (GUI also recommends the CLI), with `ratarmount` and `unrar`
  suggested.
- `emuwiz-cli --help` and `--version` worked after installation (`0.8.1-alpha`);
  `ldd` reported no missing libraries. Package-owned binaries, desktop files,
  metadata, and icons disappeared after `apt remove`/`dnf remove`.
- Debian `lintian` reported only the expected no-manual-page and initial-upload
  changelog warnings. RPM `rpmlint` reported spelling errors for the British
  “organise/organised” wording and no-manual-page/no-documentation warnings;
  these are metadata/style findings, not runtime defects.
- `desktop-file-validate` and `appstreamcli validate --no-net` passed for the
  rendered DEB and RPM payloads (one existing pedantic AppStream note).

V4 is local artifact/install QA, not a claim of official Debian or Fedora
repository compliance. RPM still builds with live Cargo dependency access;
vendoring remains the documented prerequisite for a Fedora/COPR submission.

## Package builds and install QA (V5 current release-candidate HEAD)

V5 rebuilt the packages from the current committed authority
`a2d51bbbac0088a56c4de53a323b57c6ece21c87` (`feat(identity): add PC-98 boot
evidence`). The preflight tree was clean, no Cargo/Rust or packaging-owner
process conflict was present, and the packaging scripts were unchanged from
their recorded history. The source was bind-mounted read-only into disposable
build containers; no package was installed on the authoring host.

Toolchain and build settings:

- `ubuntu:24.04`, rustup Rust `1.97.1` (`rustc 1.97.1`, `cargo 1.97.1`)
- `fedora:41`, rustup Rust `1.97.1` (`rustc 1.97.1`, `cargo 1.97.1`)
- both builds used `CARGO_BUILD_JOBS=2` and the existing packaging scripts

Debian artifacts (`0.8.1~alpha-1`, `amd64`):

| Artifact | Size | SHA-256 |
| --- | ---: | --- |
| `emuwiz_0.8.1~alpha-1_amd64.deb` | 71,258,814 bytes | `c08b111942bfd5cc0883626be166b99f4408fb3fd09329f6e72af6257d7dff7e` |
| `emuwiz-cli_0.8.1~alpha-1_amd64.deb` | 5,999,270 bytes | `1d0c34bcae45c4733886c3f48e62bc8f89bed06cf54e3b64f45a8b3ff304e340` |
| `emuwiz_0.8.1~alpha-1_amd64.buildinfo` | 9,185 bytes | `c8b3fa2b2ceae54fb4c7918e6aed3cb0d874b37492cc74673952d89a157592d5` |
| `emuwiz_0.8.1~alpha-1_amd64.changes` | 1,628 bytes | `0de1993b5dc2eac20580904e7e616a4a44d1170c2dc1f7c1d0e2b025700a85cf` |

RPM artifacts (`0.8.1-0.1.alpha.fc41`, `x86_64`):

| Artifact | Size | SHA-256 |
| --- | ---: | --- |
| `emuwiz-0.8.1-0.1.alpha.fc41.x86_64.rpm` | 71,454,812 bytes | `f5fc81c6ec10bc0f943484e680775d6d1782568b8664abbeb3578c5465de9120` |
| `emuwiz-cli-0.8.1-0.1.alpha.fc41.x86_64.rpm` | 6,148,306 bytes | `be7a64c57a64630f2d50a875a886cdcec381c647cc43731e5ded54a4d612d894` |
| `emuwiz-cli-debuginfo-0.8.1-0.1.alpha.fc41.x86_64.rpm` | 52,714,996 bytes | `d44dabd8f6eb5fa3d899abf64ad78882cf672fbe9d4070a28e75f13146fb9670` |
| `emuwiz-debuginfo-0.8.1-0.1.alpha.fc41.x86_64.rpm` | 86,933,242 bytes | `6d0009d64a1809c8bc668e9b363b803272ba37f04f01f329f991acc93e93c9ef` |
| `emuwiz-debugsource-0.8.1-0.1.alpha.fc41.x86_64.rpm` | 3,017,986 bytes | `c73739790ed009e93d25498067e3bf5eaeaff01fa2fde64dad40d2204df875f7` |

Package metadata and QA:

- Debian package dependencies are `libc6 (>= 2.39)` and `libgcc-s1 (>= 4.2)`.
  GUI recommendations are `emuwiz-cli`, `fuse3`, `xdg-utils`, `7zip |
  p7zip-full`, and `python3-pip`; CLI recommendations omit the GUI and retain
  the runtime tools. `ratarmount` and `unrar` are suggestions.
- RPM package dependencies are the generated glibc/libgcc/libm/rtld
  requirements. GUI recommendations are `emuwiz-cli` plus `fuse3`, `p7zip`,
  `p7zip-plugins`, `python3-pip`, and `xdg-utils`; CLI recommendations are
  the runtime tools without the GUI. `ratarmount` and `unrar` are suggestions.
- In fresh second containers, `emuwiz-cli --help`, `emuwiz-cli --version`,
  `ldd`, desktop-file validation, AppStream validation with
  `appstreamcli validate --no-net`, and all packaged icon checks passed.
- `apt remove emuwiz emuwiz-cli` and `dnf remove emuwiz emuwiz-cli` passed;
  all package-owned regular files and symlinks were absent afterward.
- `lintian` passed with warnings `initial-upload-closes-no-bugs` for each
  changelog and `no-manual-page` for each binary. `rpmlint` returned 64 with
  five spelling findings for the British `organise`/`organised` wording and
  `ratarmount`, plus four `no-manual-page-for-binary`/`no-documentation`
  warnings. These are metadata/style findings, not runtime failures.
- Build warnings were limited to the existing duplicate Cargo target-source
  warnings, Rust dead-code warnings in existing GUI/core code, Debian
  libc6-diversion notices, and RPM's `%{?dist}` comment, `dwz` single-file
  optimization, and unsupported `.debug_gdb_scripts` notices.
- `desktop-file-validate` and `appstreamcli validate --no-net` passed; the
  AppStream command emitted one existing pedantic note. `bash -n` passed for
  all shell scripts under `scripts/` and `packaging/`.

V5 is local current-head artifact/install QA, not a claim of official Debian
or Fedora repository compliance. No release, tag, or publish operation was
performed.

## Known blockers

1. RPM `%build` still does a live `cargo build` (crates.io network access) -
   fine for local/V2-V3 packaging QA, **not** acceptable for an official
   Fedora/COPR submission without vendoring (see Source/build model above;
   unchanged from V2).
2. Neither distro's packaged `cargo`/`rustc` is new enough to build this
   workspace (see Toolchain finding above); a real build - local or CI -
   must provide the pinned `1.97.1` toolchain itself (rustup), on PATH,
   ahead of `dpkg-buildpackage`/`rpmbuild`. This is now documented rather
   than silently assumed.
3. `ratarmount` is not an installable package on either distro (see Runtime
   dependencies) - moved to `Suggests`, `python3-pip` added to
   `Recommends` as the practical install path. This is now evidence-based,
   not a caveat.
