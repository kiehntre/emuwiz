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
| DEB (Debian/Ubuntu) | New in this lane (V2) | `packaging/debian/build-deb.sh` |
| RPM (Fedora/Nobara) | New in this lane (V2) | `packaging/rpm/build-rpm.sh` |
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
(`ratarmount`, `fusermount`/`fusermount3`, `7z`, `rar`, `xdg-open`) and the
`diagnostics` module's own doctor-check categorisation (`ratarmount` /
"unmount tool" is a `Configuration`-category check, i.e. EmuWiz **starts and
runs without it**, self-diagnoses its absence, and degrades the
archive-mount feature gracefully - it is not a hard startup requirement).

| Tool | DEB | RPM | Why |
| --- | --- | --- | --- |
| `ratarmount` | Recommends | Recommends | Read-only archive mounting - a core *feature*, not a startup requirement. |
| `fuse3` (`fusermount3`) | Recommends | Recommends | `ratarmount`'s mount backend. |
| `7zip` \| `p7zip-full` (DEB) / `p7zip` + `p7zip-plugins` (RPM) | Recommends | Recommends | 7z archive support. Debian/Ubuntu 24.04+ ships the `7zip` package (upstream `p7zip` is unmaintained); `p7zip-full` is offered as an alternative for older suites. Fedora's current package name is unverified against live repo metadata from this host (see Task B2 caveat below) - confirm with `dnf` on a real Fedora/Nobara host before a production build. |
| `unrar` | Suggests | Suggests | RAR read support; non-free, genuinely optional. |
| `xdg-utils` (`xdg-open`) | Recommends | Recommends | Opening files/links via the desktop environment; not required to start. |

Nothing here is a hard `Depends:`/`Requires:` beyond what `dh_shlibdeps` /
RPM's automatic ELF dependency generator adds for the binaries themselves
(`${shlibs:Depends}`, `${misc:Depends}` on DEB; RPM's find-requires on the
spec side) - **not yet verified against a real completed build** on this
host (see Blockers).

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

## Package builds performed / deferred

A real `packaging/debian/build-deb.sh` run was attempted on this host
(Ubuntu 24.04) and correctly reached `dpkg-checkbuilddeps` before stopping:

```
dpkg-checkbuilddeps: error: Unmet build dependencies: debhelper-compat (= 13) cargo rustc (>= 1.75) libgl1-mesa-dev libxkbcommon-x11-dev
```

This host's `cargo`/`rustc` are rustup-managed, not apt packages, and
`debhelper` plus the X11/GL dev headers are not installed. **Not installed
by this lane** (no sudo was run). To actually produce a `.deb`:

```sh
sudo apt-get install debhelper dpkg-dev cargo rustc pkg-config \
  libgl1-mesa-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-dev libxrandr-dev libxi-dev libxcursor-dev
CARGO_BUILD_JOBS=2 packaging/debian/build-deb.sh
```

`rpmbuild` is not installed on this Ubuntu host at all (expected - it is not
a Fedora system); a real RPM build must run on a Fedora/Nobara host or
container:

```sh
sudo dnf install rpm-build rust cargo gcc pkgconf-pkg-config \
  mesa-libGL-devel libxkbcommon-devel libxkbcommon-x11-devel \
  wayland-devel libX11-devel libXrandr-devel libXi-devel libXcursor-devel \
  desktop-file-utils appstream
CARGO_BUILD_JOBS=2 packaging/rpm/build-rpm.sh
```

Once either build succeeds, validate with:

```sh
dpkg-deb --info dist/emuwiz_0.8.1~alpha-1_amd64.deb
dpkg-deb --contents dist/emuwiz_0.8.1~alpha-1_amd64.deb
lintian dist/emuwiz_0.8.1~alpha-1_amd64.deb        # if installed

rpm -qpl dist/emuwiz-0.8.1-0.1.alpha*.rpm
rpmlint dist/emuwiz-0.8.1-0.1.alpha*.rpm            # if installed
```

## Disposable install QA (deferred)

Docker is available on this host; Podman is not. A real `.deb`/`.rpm` was
not produced in this pass (see above), so container install QA has nothing
to install yet. Once a package exists, run (never against the host package
database):

```sh
# Debian/Ubuntu
docker run --rm -v "$PWD/dist:/dist:ro" ubuntu:24.04 bash -c '
  apt-get update -q && apt-get install -y /dist/emuwiz_*_amd64.deb &&
  test -x /usr/bin/emuwiz && test -x /usr/bin/emuwiz-cli &&
  desktop-file-validate /usr/share/applications/io.github.kiehntre.emuwiz.desktop &&
  apt-get remove -y emuwiz emuwiz-cli &&
  ! test -f /usr/bin/emuwiz'

# Fedora
docker run --rm -v "$PWD/dist:/dist:ro" fedora:latest bash -c '
  dnf install -y /dist/emuwiz-*.rpm &&
  test -x /usr/bin/emuwiz && test -x /usr/bin/emuwiz-cli &&
  dnf remove -y emuwiz emuwiz-cli &&
  ! test -f /usr/bin/emuwiz'
```

Both pull a base image over the network the first time; deferred here to
avoid an unbounded download plus the cargo build inside the container while
another lane's cargo activity is in progress on this host.

## Known blockers (for the person who runs the real build)

1. `debhelper` (>= 13), `dpkg-dev`, and the X11/GL/Wayland `-dev` headers are
   not installed on this authoring host.
2. `rpmbuild` is not installed on this (Ubuntu) authoring host; needs a
   Fedora/Nobara host or container.
3. RPM `%build` currently does a live `cargo build` (crates.io network
   access) - fine for local/V2 packaging, **not** acceptable for an official
   Fedora/COPR submission without vendoring (see Source/build model above).
4. Exact `${shlibs:Depends}` / RPM auto-`Requires` for the GUI's dynamic
   library needs have not been verified against a completed build on this
   host - the `Build-Depends`/`BuildRequires` dev-header lists above are the
   conventional winit/egui/glow set, not yet confirmed by `ldd` on a real
   `emuwiz` binary. Re-check on the first real build.
5. Debian's exact `7zip`/`p7zip-full` availability and Fedora's `p7zip` /
   `p7zip-plugins` package names were not checked against live `apt`/`dnf`
   repo metadata from this host (Ubuntu 24.04 apt metadata was not queried
   either, to avoid an unrequested `apt update`). Confirm before relying on
   `Recommends` resolving cleanly.
