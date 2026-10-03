#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=scripts/release-common.sh
source "$SCRIPT_DIR/release-common.sh"

usage() {
    cat <<'HELP'
Usage: scripts/compare-release-builds.sh [--output-dir DIR]
       scripts/compare-release-builds.sh --archives FIRST.tar.xz SECOND.tar.xz

Build the same commit twice in separate disposable checkouts, Cargo targets,
temporary directories and package/output roots. Require identical archive bytes
and checksum files.
--archives compares existing artifacts without building or extracting them.
HELP
}

REPO_ROOT="$(release_repo_root)"
OUTPUT_DIR="$REPO_ROOT/target/reproducibility"
ARCHIVE_ONE=""
ARCHIVE_TWO=""
while (($#)); do
    case "$1" in
        --output-dir)
            (($# >= 2)) || release_die "--output-dir requires a directory"
            OUTPUT_DIR=$2
            shift 2
            ;;
        --archives)
            (($# == 3)) || release_die "--archives requires exactly two archives"
            ARCHIVE_ONE=$2
            ARCHIVE_TWO=$3
            shift 3
            ;;
        -h | --help) usage; exit 0 ;;
        *) release_die "unknown argument: $1" ;;
    esac
done

for command in cmp python3 sha256sum; do
    release_require_command "$command"
done
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/archivefs-repro.XXXXXXXX")"
cleanup() { rm -rf -- "$TEMP_ROOT"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ -z "$ARCHIVE_ONE" ]]; then
    release_require_clean_repository "$REPO_ROOT"
    if [[ "$OUTPUT_DIR" != /* ]]; then OUTPUT_DIR="$PWD/$OUTPUT_DIR"; fi
    mkdir -p "$OUTPUT_DIR"
    OUTPUT_DIR="$(CDPATH= cd -- "$OUTPUT_DIR" && pwd -P)"
    [[ -z "$(find "$OUTPUT_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
        release_die "reproducibility output directory must be empty: $OUTPUT_DIR"

    VERSION="$(release_workspace_version "$REPO_ROOT")"
    BUNDLE_NAME="$(release_bundle_name "$VERSION" "$(release_target_name)")"
    SOURCE_SHA="$(git -C "$REPO_ROOT" rev-parse HEAD)"
    export SOURCE_DATE_EPOCH="$(git -C "$REPO_ROOT" log -1 --format=%ct)"
    # A fresh target must not silently fetch compiled products from sccache or
    # another caller-configured compiler wrapper.
    export RUSTC_WRAPPER="" RUSTC_WORKSPACE_WRAPPER="" CARGO_INCREMENTAL=0
    for run in 1 2; do
        SOURCE_ROOT="$TEMP_ROOT/source$run"
        export TMPDIR="$TEMP_ROOT/tmp$run"
        export TMP="$TMPDIR" TEMP="$TMPDIR"
        mkdir -p "$TMPDIR"
        # Only immutable Git objects are shared. No registered worktrees, build
        # products, untracked files, or package staging directories are reused.
        git clone --quiet --shared --no-checkout "$REPO_ROOT" "$SOURCE_ROOT"
        git -C "$SOURCE_ROOT" -c advice.detachedHead=false checkout --quiet --detach "$SOURCE_SHA"
        release_note "build $run: source=$SOURCE_ROOT target=$TEMP_ROOT/target$run temporary=$TMPDIR staging/output=$OUTPUT_DIR/run$run commit=$SOURCE_SHA"
        started=$SECONDS
        "$SOURCE_ROOT/scripts/build-release.sh" \
            --output-dir "$OUTPUT_DIR/run$run" \
            --target-dir "$TEMP_ROOT/target$run"
        release_note "build $run completed in $((SECONDS - started)) seconds"
    done
    ARCHIVE_ONE="$OUTPUT_DIR/run1/$BUNDLE_NAME.tar.xz"
    ARCHIVE_TWO="$OUTPUT_DIR/run2/$BUNDLE_NAME.tar.xz"
fi

manifest() {
    python3 - "$1" <<'PY'
import hashlib
import json
import sys
import tarfile

with tarfile.open(sys.argv[1], "r:xz") as archive:
    for member in archive:
        digest = None
        if member.isfile():
            with archive.extractfile(member) as stream:
                hasher = hashlib.sha256()
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    hasher.update(chunk)
                digest = hasher.hexdigest()
        # Preserve archive order and include PAX/link/owner fields so a metadata
        # mismatch is diagnosable even when uncompressed payload bytes match.
        print(json.dumps(dict(name=member.name, type=member.type.decode("ascii"),
            mode=member.mode, uid=member.uid, gid=member.gid,
            uname=member.uname, gname=member.gname, mtime=member.mtime,
            size=member.size, linkname=member.linkname,
            pax_headers=member.pax_headers, sha256=digest), sort_keys=True))
PY
}

for archive in "$ARCHIVE_ONE" "$ARCHIVE_TWO"; do
    [[ "$archive" == *.tar.xz && -f "$archive" && -f "$archive.sha256" ]] ||
        release_die "expected current .tar.xz artifact and checksum: $archive"
    release_note "$(sha256sum "$archive") ($(wc -c < "$archive") bytes)"
done
manifest "$ARCHIVE_ONE" >"$TEMP_ROOT/manifest1"
manifest "$ARCHIVE_TWO" >"$TEMP_ROOT/manifest2"
cmp --silent "$TEMP_ROOT/manifest1" "$TEMP_ROOT/manifest2" || {
    diff -u "$TEMP_ROOT/manifest1" "$TEMP_ROOT/manifest2" >&2 || true
    release_die "archive member metadata or file hashes differ (including binaries, manifest and SBOM when present)"
}
cmp "$ARCHIVE_ONE" "$ARCHIVE_TWO" ||
    release_die "archive encoding differs despite identical member manifests; cmp reports the first differing byte above"
cmp "$ARCHIVE_ONE.sha256" "$ARCHIVE_TWO.sha256" ||
    release_die "generated checksum files are not reproducible"
for archive in "$ARCHIVE_ONE" "$ARCHIVE_TWO"; do
    (cd "$(dirname -- "$archive")" && sha256sum --check --strict "$(basename -- "$archive").sha256")
done
release_note "byte-for-byte archive reproduction verified (SHA-256, member metadata and sidecars match)"
