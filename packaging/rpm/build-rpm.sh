#!/usr/bin/env bash
# Build EmuWiz RPM packages from a clean, isolated copy of the committed
# tree (git HEAD - never the working tree's uncommitted changes).
#
# Never requires sudo to run. Never installs the resulting package.
#
# Requires `rpmbuild` (rpm-build / rpmdevtools on Fedora/Nobara). This
# script does NOT vendor Rust crates: `cargo build` fetches crates.io as
# usual, same as the project's existing local build practice. See
# docs/LINUX_PACKAGING.md - Task B3 for the offline/vendored-build route
# a production/COPR build would need instead.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd -P)"
OUTPUT_DIR="$REPO_ROOT/dist"
REF="HEAD"
KEEP_BUILD_ROOT=0

die() { printf 'build-rpm: error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: packaging/rpm/build-rpm.sh [options]

Builds emuwiz + emuwiz-cli RPMs into ./dist by default.

Options:
  --output-dir DIR   Explicit destination directory (default: ./dist).
  --ref REF          git ref to package (default: HEAD). Uncommitted
                      changes are never included.
  --keep-build-root  Do not delete the isolated rpmbuild tree afterwards.
  -h, --help         Show this help.

CARGO_BUILD_JOBS defaults to 2 (override via the environment).
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
command -v rpmbuild >/dev/null || die "rpmbuild not found (install rpm-build)"
command -v cargo >/dev/null || die "cargo not found"

VERSION="$(sed -n 's/^Version:[[:space:]]*//p' "$SCRIPT_DIR/emuwiz.spec" | head -1)"
[[ -n "$VERSION" ]] || die "could not read Version from emuwiz.spec"

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

RPMBUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-rpm-build.XXXXXX")"
cleanup() {
    if [[ "$KEEP_BUILD_ROOT" -eq 0 ]]; then
        rm -rf "$RPMBUILD_ROOT"
    else
        printf 'build-rpm: build root kept at %s\n' "$RPMBUILD_ROOT" >&2
    fi
}
trap cleanup EXIT

mkdir -p "$RPMBUILD_ROOT"/{SOURCES,SPECS,BUILD,RPMS,SRPMS,BUILDROOT}
mkdir -p "$OUTPUT_DIR"

printf 'build-rpm: staging %s as emuwiz-%s...\n' "$REF" "$VERSION" >&2
git -C "$REPO_ROOT" archive --format=tar --prefix="emuwiz-$VERSION/" "$REF" \
    | gzip -n > "$RPMBUILD_ROOT/SOURCES/emuwiz-$VERSION.tar.gz"
cp "$SCRIPT_DIR/emuwiz.spec" "$RPMBUILD_ROOT/SPECS/"

printf 'build-rpm: running rpmbuild (CARGO_BUILD_JOBS=%s)...\n' "$CARGO_BUILD_JOBS" >&2
rpmbuild --define "_topdir $RPMBUILD_ROOT" \
    --define "_smp_mflags -j${CARGO_BUILD_JOBS}" \
    -bb "$RPMBUILD_ROOT/SPECS/emuwiz.spec"

shopt -s nullglob
rpms=("$RPMBUILD_ROOT"/RPMS/*/*.rpm)
[[ ${#rpms[@]} -gt 0 ]] || die "no .rpm produced"
cp "${rpms[@]}" "$OUTPUT_DIR/"

printf 'build-rpm: done. Artifacts in %s:\n' "$OUTPUT_DIR" >&2
ls -1 "$OUTPUT_DIR"/*.rpm
