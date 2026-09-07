# EmuWiz Fedora/Nobara RPM packaging foundation.
#
# Pre-release versioning follows Fedora convention for an alpha snapshot:
#   Cargo:  0.8.1-alpha
#   RPM:    Version 0.8.1, Release 0.1.alpha%{?dist}
# ("0.1." keeps this sorting BELOW the eventual 0.8.1-1 final release, per
# Fedora's pre-release packaging guidelines.)

%global app_id io.github.kiehntre.emuwiz
%global forgeurl https://github.com/kiehntre/emuwiz

Name:           emuwiz
Version:        0.8.1
Release:        0.1.alpha%{?dist}
Summary:        Verify, organise, and play a retro-game library

License:        MIT
URL:            %{forgeurl}
# Local/offline packaging source: a plain tarball of the committed tree,
# produced by build-rpm.sh via `git archive` (see that script and
# docs/LINUX_PACKAGING.md - Task B3, "Source / build model"). This is not
# yet a published release tarball.
Source0:        emuwiz-%{version}.tar.gz

BuildRequires:  cargo >= 1.75
BuildRequires:  rust >= 1.75
BuildRequires:  gcc
BuildRequires:  pkgconfig
BuildRequires:  mesa-libGL-devel
BuildRequires:  libxkbcommon-devel
BuildRequires:  libxkbcommon-x11-devel
BuildRequires:  wayland-devel
BuildRequires:  libX11-devel
BuildRequires:  libXrandr-devel
BuildRequires:  libXi-devel
BuildRequires:  libXcursor-devel
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

Recommends:     %{name}-cli = %{version}-%{release}
Recommends:     ratarmount
Recommends:     fuse3
Recommends:     xdg-utils
Recommends:     p7zip
Recommends:     p7zip-plugins
Suggests:       unrar

%description
EmuWiz helps you turn a messy emulation collection into a verified,
organised, and more playable library. It identifies supported games
from their files and archive contents, checks them against
preservation databases, highlights missing or questionable files,
helps discover and prepare emulator profiles, builds organised
libraries for tools such as RomM and ES-DE, and provides safe cheat,
patch, and selected mod workflows.

Potentially destructive operations are previewed before they run and
use confirmation, verification, and rollback safeguards where
supported. EmuWiz can also mount supported ZIP, 7z, and RAR archives
read-only via ratarmount/FUSE, so you can browse and use their
contents without permanently extracting everything. It runs locally
on Linux, and your collection stays yours.

This package provides the graphical application.

%package cli
Summary:        Verify, organise, and play a retro-game library (CLI)
Recommends:     ratarmount
Recommends:     fuse3
Recommends:     p7zip
Recommends:     p7zip-plugins
Suggests:       unrar

%description cli
EmuWiz helps you turn a messy emulation collection into a verified,
organised, and more playable library. See the main emuwiz package
description for details.

This package provides the emuwiz-cli command-line tool and has no
dependency on the GUI package, so it can be installed standalone
(e.g. for headless/server use).

%prep
%autosetup -n emuwiz-%{version}

%build
export CARGO_BUILD_JOBS=%{?_smp_mflags:%{_smp_mflags}}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
cargo build --release --locked \
    -p archivefs-gui --bin emuwiz \
    -p archivefs-cli --bin emuwiz-cli

%install
install -Dm755 target/release/emuwiz %{buildroot}%{_bindir}/emuwiz
install -Dm755 target/release/emuwiz-cli %{buildroot}%{_bindir}/emuwiz-cli

sed 's/@EMUWIZ_EXEC@/emuwiz/' assets/linux/%{app_id}.desktop.in \
    > %{_builddir}/emuwiz-%{version}/%{app_id}.desktop
install -Dm644 %{_builddir}/emuwiz-%{version}/%{app_id}.desktop \
    %{buildroot}%{_datadir}/applications/%{app_id}.desktop

install -Dm644 assets/linux/%{app_id}.metainfo.xml \
    %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml

for size in 32 64 128 256 512; do
    install -Dm644 assets/branding/emuwiz-logo-${size}.png \
        %{buildroot}%{_datadir}/icons/hicolor/${size}x${size}/apps/%{app_id}.png
done

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstream-util validate-relax --nonet \
    %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml || true

%files
%license LICENSE
%{_bindir}/emuwiz
%{_datadir}/applications/%{app_id}.desktop
%{_metainfodir}/%{app_id}.metainfo.xml
%{_datadir}/icons/hicolor/32x32/apps/%{app_id}.png
%{_datadir}/icons/hicolor/64x64/apps/%{app_id}.png
%{_datadir}/icons/hicolor/128x128/apps/%{app_id}.png
%{_datadir}/icons/hicolor/256x256/apps/%{app_id}.png
%{_datadir}/icons/hicolor/512x512/apps/%{app_id}.png

%files cli
%license LICENSE
%{_bindir}/emuwiz-cli

%changelog
* Mon Sep 07 2026 David Armstrong <kiehntre@users.noreply.github.com> - 0.8.1-0.1.alpha
- Initial Fedora/Nobara RPM packaging foundation (packaging only; not yet
  released). See docs/LINUX_PACKAGING.md.
