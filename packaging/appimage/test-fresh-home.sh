#!/usr/bin/env bash
# Fresh-user smoke QA for the packaged AppImage: exercises the checks that
# do not require brittle GUI automation, entirely inside a disposable
# HOME/XDG sandbox. Never touches the invoking user's real
# ~/.config/emuwiz, ~/.local/share/emuwiz, or their legacy
# ~/.config/archivefs, ~/.local/share/archivefs equivalents.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

die() {
    printf 'fresh-home qa: error: %s\n' "$*" >&2
    exit 1
}
info() {
    printf 'fresh-home qa: %s\n' "$*"
}

ARTIFACT="${1:-$SCRIPT_DIR/../../dist/EmuWiz-x86_64.AppImage}"
ARTIFACT="$(realpath -e -- "$ARTIFACT")" || die "AppImage not found: ${1:-<default dist path>}"
[[ -f "$ARTIFACT" && -x "$ARTIFACT" ]] || die "AppImage must be an executable regular file"

CHECKSUM="$ARTIFACT.sha256"
[[ -f "$CHECKSUM" ]] || die "checksum file not found: $CHECKSUM"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-fresh-home-qa.XXXXXXXX")"
cleanup() {
    rm -rf -- "$TEMP_ROOT"
}
trap cleanup EXIT INT TERM

FRESH_HOME="$TEMP_ROOT/home"
FRESH_XDG_CONFIG="$TEMP_ROOT/xdg-config"
FRESH_XDG_DATA="$TEMP_ROOT/xdg-data"
FRESH_XDG_CACHE="$TEMP_ROOT/xdg-cache"
mkdir -p "$FRESH_HOME" "$FRESH_XDG_CONFIG" "$FRESH_XDG_DATA" "$FRESH_XDG_CACHE"

info "artifact: $ARTIFACT"
info "disposable root: $TEMP_ROOT"

info "verifying checksum..."
(cd "$(dirname -- "$ARTIFACT")" && sha256sum --check --status "$(basename -- "$CHECKSUM")") ||
    die "AppImage checksum does not verify"
info "checksum OK"

run_clean() {
    # Never inherits the invoking shell's real HOME/XDG/PATH: every value the
    # launched process can see is named explicitly here.
    env -i \
        HOME="$FRESH_HOME" \
        XDG_CONFIG_HOME="$FRESH_XDG_CONFIG" \
        XDG_DATA_HOME="$FRESH_XDG_DATA" \
        XDG_CACHE_HOME="$FRESH_XDG_CACHE" \
        PATH="/usr/bin:/bin" \
        "$@"
}

info "checking --version (normal AppImage mode)..."
VERSION_OUTPUT="$(run_clean "$ARTIFACT" --version)" || die "--version failed in normal mode"
info "  -> $VERSION_OUTPUT"

info "checking --version (APPIMAGE_EXTRACT_AND_RUN=1)..."
VERSION_OUTPUT_EXTRACT="$(run_clean env APPIMAGE_EXTRACT_AND_RUN=1 "$ARTIFACT" --version)" ||
    die "--version failed under APPIMAGE_EXTRACT_AND_RUN"
info "  -> $VERSION_OUTPUT_EXTRACT"
[[ "$VERSION_OUTPUT" == "$VERSION_OUTPUT_EXTRACT" ]] ||
    die "normal and extract-and-run --version output differ"

info "verifying the disposable HOME/XDG dirs are still exactly as created (no surprise writes from --version)..."
FRESH_ENTRY_COUNT="$(find "$FRESH_HOME" "$FRESH_XDG_CONFIG" "$FRESH_XDG_DATA" "$FRESH_XDG_CACHE" -mindepth 1 | wc -l)"
[[ "$FRESH_ENTRY_COUNT" -eq 0 ]] ||
    die "--version alone should not create any files, found $FRESH_ENTRY_COUNT entr(y/ies)"
info "  -> clean, as expected"

info "checking host PATH tools are not masked (best-effort; a missing tool is not a failure)..."
for tool in ratarmount fusermount3 7z retroarch; do
    if command -v "$tool" >/dev/null 2>&1; then
        info "  host has $tool at $(command -v "$tool")"
    else
        info "  host does not have $tool (skipping)"
    fi
done

info "checking AppRun does not itself rewrite HOME/XDG/PATH..."
grep -Eq 'HOME=|XDG_[A-Z_]*=|PATH=' "$SCRIPT_DIR/AppRun" &&
    die "AppRun appears to assign HOME/XDG/PATH - packaging must not mutate the caller's environment"
info "  -> AppRun is clean"

info "all checks passed"
