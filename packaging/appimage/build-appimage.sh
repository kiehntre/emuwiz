#!/usr/bin/env bash
# Build the V1 EmuWiz x86_64 AppImage without changing user configuration/data.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd -P)"
OUTPUT_DIR="$REPO_ROOT/dist"
TARGET_DIR=""
APPIMAGETOOL="${APPIMAGETOOL:-}"
RUNTIME_FILE="${APPIMAGE_RUNTIME_FILE:-}"

die() {
    printf 'appimage build: error: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: packaging/appimage/build-appimage.sh [options]

Builds:
  EmuWiz-x86_64.AppImage
  EmuWiz-x86_64.AppImage.sha256

Options:
  --output-dir DIR      Explicit destination directory (default: ./dist).
  --target-dir DIR      Cargo target directory for an isolated build.
  --appimagetool PATH   Pinned/approved host appimagetool executable.
  --runtime-file PATH   Pinned AppImage type-2 runtime file.
  -h, --help            Show this help.

Requirements: bash, cargo, git, install, sha256sum, appimagetool, and an
approved type-2 runtime file. Both packaging inputs are explicit host build
dependencies: this repository does not download or vendor executable tools or
a moving AppImage runtime.
EOF
}

while (($#)); do
    case "$1" in
        --output-dir)
            (($# >= 2)) || die "--output-dir requires a directory"
            OUTPUT_DIR=$2
            shift 2
            ;;
        --target-dir)
            (($# >= 2)) || die "--target-dir requires a directory"
            TARGET_DIR=$2
            shift 2
            ;;
        --appimagetool)
            (($# >= 2)) || die "--appimagetool requires a path"
            APPIMAGETOOL=$2
            shift 2
            ;;
        --runtime-file)
            (($# >= 2)) || die "--runtime-file requires a path"
            RUNTIME_FILE=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *) die "unknown argument: $1" ;;
    esac
done

for command in cargo git install mktemp python3 realpath sha256sum; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done
[[ "$(uname -m)" == x86_64 ]] || die "V1 AppImage build currently supports x86_64 only"
[[ "$(git -C "$REPO_ROOT" rev-parse --show-toplevel)" == "$REPO_ROOT" ]] ||
    die "script must run from a repository worktree"
[[ -z "$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=normal)" ]] ||
    die "repository must be clean before building a release AppImage"
VERSION="$(cd "$REPO_ROOT" && cargo metadata --format-version 1 --no-deps | python3 -c '
import json
import sys
packages = json.load(sys.stdin)["packages"]
versions = {p["version"] for p in packages if p["name"] in {"archivefs-core", "archivefs-cli", "archivefs-gui"}}
if len(versions) != 1:
    raise SystemExit("EmuWiz workspace package versions disagree")
print(versions.pop())
')" || die "could not determine the workspace version"

if [[ -z "$APPIMAGETOOL" ]]; then
    APPIMAGETOOL="$(command -v appimagetool || true)"
fi
[[ -n "$APPIMAGETOOL" ]] || die "appimagetool is required; install an approved host tool or pass --appimagetool PATH"
APPIMAGETOOL="$(realpath -e -- "$APPIMAGETOOL")"
[[ -f "$APPIMAGETOOL" && -x "$APPIMAGETOOL" ]] || die "appimagetool is not executable: $APPIMAGETOOL"
[[ -n "$RUNTIME_FILE" ]] || die "a pinned type-2 runtime is required; pass --runtime-file PATH"
RUNTIME_FILE="$(realpath -e -- "$RUNTIME_FILE")"
[[ -f "$RUNTIME_FILE" && ! -L "$RUNTIME_FILE" ]] || die "runtime file must be a regular non-symlink: $RUNTIME_FILE"

if [[ "$OUTPUT_DIR" != /* ]]; then
    OUTPUT_DIR="$PWD/$OUTPUT_DIR"
fi
mkdir -p -- "$OUTPUT_DIR"
OUTPUT_DIR="$(realpath -e -- "$OUTPUT_DIR")"
[[ "$OUTPUT_DIR" != / && "$OUTPUT_DIR" != "$REPO_ROOT" ]] ||
    die "refusing unsafe output directory: $OUTPUT_DIR"
ARTIFACT="$OUTPUT_DIR/EmuWiz-x86_64.AppImage"
CHECKSUM="$ARTIFACT.sha256"
[[ ! -e "$ARTIFACT" && ! -e "$CHECKSUM" ]] ||
    die "output already exists; choose an empty destination or remove only the intended prior artifact"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-appimage.XXXXXXXX")"
cleanup() {
    rm -rf -- "$TEMP_ROOT"
}
trap cleanup EXIT INT TERM
APPDIR="$TEMP_ROOT/AppDir"

BUILD_ARGS=(build --release --locked -p archivefs-gui --bin emuwiz)
if [[ -n "$TARGET_DIR" ]]; then
    if [[ "$TARGET_DIR" != /* ]]; then
        TARGET_DIR="$PWD/$TARGET_DIR"
    fi
    mkdir -p -- "$TARGET_DIR"
    TARGET_DIR="$(realpath -e -- "$TARGET_DIR")"
fi

if [[ -n "$TARGET_DIR" ]]; then
    (cd "$REPO_ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
        cargo "${BUILD_ARGS[@]}")
    BUILD_ROOT="$TARGET_DIR/release"
else
    (cd "$REPO_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo "${BUILD_ARGS[@]}")
    BUILD_ROOT="$REPO_ROOT/target/release"
fi
[[ -x "$BUILD_ROOT/emuwiz" ]] || die "release GUI binary missing: $BUILD_ROOT/emuwiz"

# The CLI is intentionally a support companion, not a desktop entry. Build
# just that canonical target; never package archivefs compatibility aliases.
if [[ -n "$TARGET_DIR" ]]; then
    (cd "$REPO_ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
        cargo build --release --locked -p archivefs-cli --bin emuwiz-cli)
else
    (cd "$REPO_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build --release --locked -p archivefs-cli --bin emuwiz-cli)
fi
[[ -x "$BUILD_ROOT/emuwiz-cli" ]] || die "release CLI binary missing: $BUILD_ROOT/emuwiz-cli"

install -d -m 0755 "$APPDIR/usr/bin" "$APPDIR/usr/share/applications"
install -m 0755 "$SCRIPT_DIR/AppRun" "$APPDIR/AppRun"
install -m 0755 "$BUILD_ROOT/emuwiz" "$APPDIR/usr/bin/emuwiz"
install -m 0755 "$BUILD_ROOT/emuwiz-cli" "$APPDIR/usr/bin/emuwiz-cli"
install -m 0644 "$SCRIPT_DIR/emuwiz.desktop" "$APPDIR/emuwiz.desktop"
install -m 0644 "$SCRIPT_DIR/emuwiz.desktop" "$APPDIR/usr/share/applications/emuwiz.desktop"
for size in 32 64 128 256 512; do
    icon_dir="$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps"
    install -d -m 0755 "$icon_dir"
    install -m 0644 "$REPO_ROOT/assets/branding/emuwiz-logo-$size.png" \
        "$icon_dir/io.github.kiehntre.emuwiz.png"
done
install -m 0644 "$REPO_ROOT/assets/branding/emuwiz-logo-256.png" \
    "$APPDIR/io.github.kiehntre.emuwiz.png"

"$SCRIPT_DIR/verify-appdir.sh" "$APPDIR"

# appimagetool is deliberately the only external packager. Extract-and-run is
# set for the tool itself so an approved AppImage-distributed tool remains
# usable on a build host without legacy libfuse2; it is not exported to EmuWiz.
ARCH=x86_64 VERSION="$VERSION" APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" \
    --runtime-file "$RUNTIME_FILE" "$APPDIR" "$ARTIFACT"
[[ -f "$ARTIFACT" && -x "$ARTIFACT" ]] || die "appimagetool did not create an executable AppImage"
sha256sum -- "$ARTIFACT" >"$CHECKSUM"
"$SCRIPT_DIR/verify-appimage.sh" --checksum "$CHECKSUM" "$ARTIFACT"

printf 'AppImage: %s\nChecksum: %s\n' "$ARTIFACT" "$CHECKSUM"
