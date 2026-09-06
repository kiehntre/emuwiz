#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-fresh-home-fuse-test.XXXXXXXX")"
trap 'rm -rf -- "$TEMP_ROOT"' EXIT INT TERM

make_artifact() {
    local name=$1 body=$2
    local artifact="$TEMP_ROOT/$name.AppImage"
    printf '%s\n' '#!/bin/sh' "$body" >"$artifact"
    chmod 0755 "$artifact"
    (cd "$TEMP_ROOT" && sha256sum "$(basename -- "$artifact")" >"$(basename -- "$artifact").sha256")
    printf '%s\n' "$artifact"
}

normal="$(make_artifact normal 'echo emuwiz-test')"
"$SCRIPT_DIR/test-fresh-home.sh" "$normal" >/dev/null

fuse="$(make_artifact fuse 'if [ "${APPIMAGE_EXTRACT_AND_RUN:-}" = 1 ]; then echo emuwiz-test; else echo "fuse: device not found" >&2; exit 1; fi')"
fuse_output="$("$SCRIPT_DIR/test-fresh-home.sh" "$fuse")"
grep -Fq 'CONTINUING WITH APPIMAGE_EXTRACT_AND_RUN=1' <<<"$fuse_output"

bad="$(make_artifact bad 'echo broken >&2; exit 1')"
if "$SCRIPT_DIR/test-fresh-home.sh" "$bad" >/dev/null 2>&1; then
    echo 'unrelated normal-mode failure was accepted' >&2
    exit 1
fi

bad_extract="$(make_artifact bad-extract 'if [ "${APPIMAGE_EXTRACT_AND_RUN:-}" = 1 ]; then exit 1; else echo "failed to open /dev/fuse" >&2; exit 1; fi')"
if "$SCRIPT_DIR/test-fresh-home.sh" "$bad_extract" >/dev/null 2>&1; then
    echo 'failed extract-and-run fallback was accepted' >&2
    exit 1
fi
