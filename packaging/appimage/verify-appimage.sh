#!/usr/bin/env bash
# Verify an AppImage by extracting it into a private temporary directory.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

die() {
    printf 'appimage verify: error: %s\n' "$*" >&2
    exit 1
}

CHECKSUM=""
while (($#)); do
    case "$1" in
        --checksum)
            (($# >= 2)) || die "--checksum requires a file"
            CHECKSUM=$2
            shift 2
            ;;
        *)
            [[ -z "${ARTIFACT:-}" ]] || die "accepts exactly one AppImage"
            ARTIFACT=$1
            shift
            ;;
    esac
done
[[ -n "${ARTIFACT:-}" ]] || die "AppImage is required"
ARTIFACT="$(realpath -e -- "$ARTIFACT")"
[[ -f "$ARTIFACT" && -x "$ARTIFACT" ]] || die "AppImage must be an executable regular file"
[[ "$(basename -- "$ARTIFACT")" == 'EmuWiz-x86_64.AppImage' ]] ||
    die "AppImage name must be EmuWiz-x86_64.AppImage"

CHECKSUM=${CHECKSUM:-"$ARTIFACT.sha256"}
CHECKSUM="$(realpath -e -- "$CHECKSUM")"
[[ "$(basename -- "$CHECKSUM")" == 'EmuWiz-x86_64.AppImage.sha256' ]] ||
    die "checksum name must be EmuWiz-x86_64.AppImage.sha256"
sha256sum --check --status "$CHECKSUM" || die "AppImage checksum does not verify"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-appimage-verify.XXXXXXXX")"
cleanup() {
    rm -rf -- "$TEMP_ROOT"
}
trap cleanup EXIT INT TERM

# --appimage-extract needs no package FUSE mount. Setting the standard
# extract-and-run fallback also permits an AppImage tool/runtime that checks it
# before dispatching extraction on systems without legacy libfuse2.
(
    cd "$TEMP_ROOT"
    APPIMAGE_EXTRACT_AND_RUN=1 "$ARTIFACT" --appimage-extract >/dev/null
)
[[ -d "$TEMP_ROOT/squashfs-root" ]] || die "AppImage did not produce squashfs-root"
"$SCRIPT_DIR/verify-appdir.sh" "$TEMP_ROOT/squashfs-root"
