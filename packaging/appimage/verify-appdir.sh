#!/usr/bin/env bash
# Validate the intentionally small, host-integrated EmuWiz AppDir payload.
set -euo pipefail

die() {
    printf 'appimage verify: error: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die "usage: $0 APPDIR"
APPDIR=$1
[[ -d "$APPDIR" ]] || die "AppDir is not a directory: $APPDIR"

required=(
    AppRun
    emuwiz.desktop
    io.github.kiehntre.emuwiz.png
    usr/bin/emuwiz
    usr/bin/emuwiz-cli
    usr/share/applications/emuwiz.desktop
)
for relative in "${required[@]}"; do
    [[ -f "$APPDIR/$relative" ]] || die "missing required AppDir file: $relative"
done
[[ -x "$APPDIR/AppRun" ]] || die "AppRun is not executable"
[[ -x "$APPDIR/usr/bin/emuwiz" ]] || die "packaged emuwiz is not executable"
[[ -x "$APPDIR/usr/bin/emuwiz-cli" ]] || die "packaged emuwiz-cli is not executable"
for size in 32 64 128 256 512; do
    [[ -f "$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps/io.github.kiehntre.emuwiz.png" ]] ||
        die "missing hicolor icon: ${size}x${size}"
done

desktop="$APPDIR/usr/share/applications/emuwiz.desktop"
# appimagetool may add its version field to the root desktop entry while
# preserving the installed copy. Validate the user-visible semantics of both
# instead of demanding byte identity across that intentional transformation.
for entry in "$APPDIR/emuwiz.desktop" "$desktop"; do
    grep -Fxq 'Name=EmuWiz' "$entry" || die "desktop Name must be EmuWiz"
    grep -Fxq 'Exec=emuwiz' "$entry" || die "desktop Exec must be emuwiz"
    grep -Fxq 'Icon=io.github.kiehntre.emuwiz' "$entry" || die "desktop Icon is incorrect"
    ! grep -qi 'archivefs' "$entry" || die "desktop entry contains stale ArchiveFS branding"
    if command -v desktop-file-validate >/dev/null 2>&1; then
        desktop-file-validate "$entry"
    fi
done

# AppRun must remain transparent to user data, source paths, host tools, and
# host graphics. The sole path it is allowed to use is its own executable.
app_run="$APPDIR/AppRun"
grep -Fq 'exec "${APPDIR:?AppImage runtime did not set APPDIR}/usr/bin/emuwiz" "$@"' "$app_run" ||
    die "AppRun does not exec the packaged emuwiz binary"
! grep -Eq '(^|[[:space:]])(HOME|XDG_CONFIG_HOME|XDG_DATA_HOME|PATH|LD_LIBRARY_PATH)=' "$app_run" ||
    die "AppRun rewrites a user or loader environment variable"

# This V1 payload is deliberately binary-and-assets only. Any library payload
# would need a separate reviewed closure/exclusion policy before it can be
# allowed to affect host emulator or FUSE-helper children.
[[ ! -e "$APPDIR/usr/lib" ]] || die "AppDir must not bundle shared libraries in V1"
for forbidden in ratarmount fusermount fusermount3 7z 7zz p7zip ffmpeg \
    retroarch RMG mesen mesen2 snes9x stella vice; do
    if find "$APPDIR" -type f -name "$forbidden" -print -quit | grep -q .; then
        die "forbidden host helper or emulator bundled: $forbidden"
    fi
done
if find "$APPDIR" -type f \( -name 'libGL*.so*' -o -name 'libEGL*.so*' -o \
    -name 'libOpenGL*.so*' -o -name 'libvulkan*.so*' -o -name 'libnvidia*.so*' -o \
    -name 'libGLX_nvidia*.so*' \) -print -quit | grep -q .; then
    die "forbidden host graphics library bundled"
fi
