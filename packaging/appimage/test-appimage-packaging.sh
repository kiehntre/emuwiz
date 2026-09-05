#!/usr/bin/env bash
# Lightweight no-Cargo regression checks for the AppDir templates and verifier.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd -P)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-appimage-test.XXXXXXXX")"
cleanup() {
    rm -rf -- "$TEMP_ROOT"
}
trap cleanup EXIT INT TERM
APPDIR="$TEMP_ROOT/AppDir"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications"

printf '%s\n' \
    '#!/bin/sh' \
    'printf "%s\\n" "${HOME}|${XDG_CONFIG_HOME:-}|${XDG_DATA_HOME:-}|${PATH}|$1"' \
    >"$APPDIR/usr/bin/emuwiz"
printf '%s\n' '#!/usr/bin/env sh' 'exit 0' >"$APPDIR/usr/bin/emuwiz-cli"
chmod 0755 "$APPDIR/usr/bin/emuwiz" "$APPDIR/usr/bin/emuwiz-cli"
install -m 0755 "$SCRIPT_DIR/AppRun" "$APPDIR/AppRun"
install -m 0644 "$SCRIPT_DIR/emuwiz.desktop" "$APPDIR/emuwiz.desktop"
install -m 0644 "$SCRIPT_DIR/emuwiz.desktop" "$APPDIR/usr/share/applications/emuwiz.desktop"
install -m 0644 "$REPO_ROOT/assets/branding/emuwiz-logo-256.png" "$APPDIR/io.github.kiehntre.emuwiz.png"
for size in 32 64 128 256 512; do
    icon_dir="$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$icon_dir"
    install -m 0644 "$REPO_ROOT/assets/branding/emuwiz-logo-$size.png" \
        "$icon_dir/io.github.kiehntre.emuwiz.png"
done

"$SCRIPT_DIR/verify-appdir.sh" "$APPDIR"
result="$(APPDIR="$APPDIR" HOME=/home/emuwiz-test XDG_CONFIG_HOME=/cfg XDG_DATA_HOME=/data PATH=/host/bin "$APPDIR/AppRun" /mnt/roms/game.rom)"
[[ "$result" == '/home/emuwiz-test|/cfg|/data|/host/bin|/mnt/roms/game.rom' ]]

printf '%s\n' '#!/usr/bin/env sh' >"$APPDIR/usr/bin/ratarmount"
chmod 0755 "$APPDIR/usr/bin/ratarmount"
if "$SCRIPT_DIR/verify-appdir.sh" "$APPDIR" >/dev/null 2>&1; then
    printf '%s\n' 'expected forbidden host helper rejection' >&2
    exit 1
fi
