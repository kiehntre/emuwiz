#!/usr/bin/env bash
# Build EmuWiz .deb packages from a clean, isolated copy of the committed
# tree (git HEAD - never the working tree's uncommitted changes, so this
# never depends on, or risks, another lane's in-progress edits).
#
# Never requires sudo to run. Never installs the resulting package.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd -P)"
OUTPUT_DIR="$REPO_ROOT/dist"
BUILD_ROOT=""
KEEP_BUILD_ROOT=0
REF="HEAD"

die() { printf 'build-deb: error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: packaging/debian/build-deb.sh [options]

Builds binary .deb packages (emuwiz, emuwiz-cli) into ./dist by default.

Options:
  --output-dir DIR   Explicit destination directory (default: ./dist).
  --ref REF          git ref to package (default: HEAD). Uncommitted
                      changes are never included.
  --keep-build-root  Do not delete the isolated build directory afterwards
                      (its path is printed) - useful for inspecting a
                      failure.
  -h, --help         Show this help.

Requirements (all host-provided, none installed by this script):
  git, dpkg-buildpackage, debhelper (>= 13), cargo, rustc, fakeroot.

CARGO_BUILD_JOBS defaults to 2 (override via the environment). The build
runs entirely inside an isolated temporary copy of the repository, so it
never competes with, or is affected by, cargo/rustc activity in the real
working tree's target/ directory.
EOF
}

while (($#)); do
    case "$1" in
        --output-dir) (($# >= 2)) || die "--output-dir requires a directory"; OUTPUT_DIR=$2; shift 2 ;;
        --ref) (($# >= 2)) || die "--ref requires a value"; REF=$2; shift 2 ;;
        --keep-build-root) KEEP_BUILD_ROOT=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done

command -v git >/dev/null || die "git not found"
command -v dpkg-buildpackage >/dev/null || die "dpkg-buildpackage not found (install dpkg-dev)"
command -v cargo >/dev/null || die "cargo not found"
command -v fakeroot >/dev/null || die "fakeroot not found"

for f in \
    "$REPO_ROOT/assets/linux/io.github.kiehntre.emuwiz.desktop.in" \
    "$REPO_ROOT/assets/linux/io.github.kiehntre.emuwiz.metainfo.xml"
do
    [[ -f "$f" ]] || die "required shared asset missing: $f"
done
for size in 32 64 128 256 512; do
    f="$REPO_ROOT/assets/branding/emuwiz-logo-$size.png"
    [[ -f "$f" ]] || die "required branding icon missing: $f"
done

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

# dpkg-buildpackage drops .deb/.changes/.buildinfo in the PARENT of the
# source directory, so that parent must be private to this run (never bare
# /tmp) or the glob below could pick up unrelated files.
BUILD_PARENT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-deb-build.XXXXXX")"
BUILD_ROOT="$BUILD_PARENT/src"
mkdir -p "$BUILD_ROOT"
cleanup() {
    if [[ "$KEEP_BUILD_ROOT" -eq 0 ]]; then
        rm -rf "$BUILD_PARENT"
    else
        printf 'build-deb: build root kept at %s\n' "$BUILD_PARENT" >&2
    fi
}
trap cleanup EXIT

printf 'build-deb: staging %s into isolated tree...\n' "$REF" >&2
git -C "$REPO_ROOT" archive --format=tar "$REF" | tar -x -C "$BUILD_ROOT"
cp -a "$SCRIPT_DIR" "$BUILD_ROOT/debian"

mkdir -p "$OUTPUT_DIR"

printf 'build-deb: running dpkg-buildpackage (CARGO_BUILD_JOBS=%s)...\n' "$CARGO_BUILD_JOBS" >&2
(
    cd "$BUILD_ROOT"
    dpkg-buildpackage -us -uc -b
)

shopt -s nullglob
moved=0
for f in "$BUILD_PARENT"/*.deb "$BUILD_PARENT"/*.buildinfo "$BUILD_PARENT"/*.changes; do
    [[ -e "$f" ]] || continue
    mv "$f" "$OUTPUT_DIR/"
    moved=1
done
[[ "$moved" -eq 1 ]] || die "no .deb produced"

printf 'build-deb: done. Artifacts in %s:\n' "$OUTPUT_DIR" >&2
ls -1 "$OUTPUT_DIR"/*.deb
