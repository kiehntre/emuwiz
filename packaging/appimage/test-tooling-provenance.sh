#!/usr/bin/env bash
# Contract checks for the pinned appimagetool/runtime inputs. No Cargo or
# packaging build is run, and intentionally invalid files are never executed.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
BUILD="$SCRIPT_DIR/build-appimage.sh"
CACHE="${XDG_CACHE_HOME:-${HOME:-/tmp}/.cache}/emuwiz/appimage-tools"
TOOL="${APPIMAGETOOL_TEST_PATH:-$CACHE/appimagetool-x86_64.AppImage}"
RUNTIME="${APPIMAGE_RUNTIME_TEST_PATH:-$CACHE/runtime-x86_64}"

[[ -x "$TOOL" && -f "$RUNTIME" ]] || {
    printf 'tooling provenance: missing pinned test fixtures; restore the documented cache first\n' >&2
    exit 1
}

"$BUILD" --verify-tooling --appimagetool "$TOOL" --runtime-file "$RUNTIME" >/dev/null

TMP="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-tooling-test.XXXXXXXX")"
trap 'rm -rf -- "$TMP"' EXIT INT TERM
cp -- "$TOOL" "$TMP/tool"
cp -- "$RUNTIME" "$TMP/runtime"
printf x >>"$TMP/tool"
if "$BUILD" --verify-tooling --appimagetool "$TMP/tool" --runtime-file "$TMP/runtime" >/dev/null 2>&1; then
    printf 'tooling provenance: wrong appimagetool checksum was accepted\n' >&2
    exit 1
fi
cp -- "$TOOL" "$TMP/tool"
printf x >>"$TMP/runtime"
if "$BUILD" --verify-tooling --appimagetool "$TMP/tool" --runtime-file "$TMP/runtime" >/dev/null 2>&1; then
    printf 'tooling provenance: wrong runtime checksum was accepted\n' >&2
    exit 1
fi
if "$BUILD" --verify-tooling --appimagetool "$TMP/missing-tool" --runtime-file "$RUNTIME" >/dev/null 2>&1; then
    printf 'tooling provenance: missing appimagetool was accepted\n' >&2
    exit 1
fi
if "$BUILD" --verify-tooling --appimagetool "$TOOL" --runtime-file "$TMP/missing-runtime" >/dev/null 2>&1; then
    printf 'tooling provenance: missing runtime was accepted\n' >&2
    exit 1
fi
printf 'tooling provenance: all checks passed\n'
