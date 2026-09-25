#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=scripts/release-common.sh
source "$SCRIPT_DIR/release-common.sh"

[[ $# -eq 1 ]] || release_die "usage: scripts/test-release-artifact-verifier.sh ARCHIVE.tar.xz"
VALID_ARTIFACT=$1
[[ -f "$VALID_ARTIFACT" ]] || release_die "archive not found: $VALID_ARTIFACT"
[[ "$VALID_ARTIFACT" == *.tar.xz ]] || release_die "artifact must use the current .tar.xz format"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-verifier-tests.XXXXXXXX")"
cleanup() {
    rm -rf -- "$TEMP_ROOT"
}
trap cleanup EXIT INT TERM

ARCHIVE_NAME="$(basename -- "$VALID_ARTIFACT")"

write_checksum() {
    local archive=$1
    (cd "$(dirname -- "$archive")" && sha256sum "$(basename -- "$archive")") >"$archive.sha256"
}

expect_failure() {
    local label=$1
    local archive=$2
    local expected=$3
    local output="$TEMP_ROOT/$label.out"
    if "$SCRIPT_DIR/release/verify-release.sh" --strict \
        --checksum "$archive.sha256" "$archive" >"$output" 2>&1; then
        release_die "verifier accepted malformed fixture: $label"
    fi
    grep -Fq "$expected" "$output" ||
        release_die "fixture $label failed for the wrong reason (expected '$expected'): $(cat "$output")"
    release_note "malformed artifact rejected: $label ($expected)"
}

mkdir -p "$TEMP_ROOT/bad-checksum"
cp -- "$VALID_ARTIFACT" "$TEMP_ROOT/bad-checksum/$ARCHIVE_NAME"
printf '%064d  %s\n' 0 "$ARCHIVE_NAME" >"$TEMP_ROOT/bad-checksum/$ARCHIVE_NAME.sha256"
expect_failure bad-checksum "$TEMP_ROOT/bad-checksum/$ARCHIVE_NAME" "archive checksum mismatch"

# Build every malformed archive from the current release archive itself. This
# keeps the fixture aligned with the package-release.py contract: PAX tar,
# xz compression, one release directory, generated manifest/checksums, and
# the current bin/docs/SBOM layout. The verifier remains the system under
# test; this helper only performs controlled archive mutations.
python3 - "$TEMP_ROOT" "$VALID_ARTIFACT" <<'PY'
import copy
import io
import pathlib
import sys
import tarfile

output_root = pathlib.Path(sys.argv[1])
source_archive = pathlib.Path(sys.argv[2])
archive_name = source_archive.name

with tarfile.open(source_archive, "r:xz") as source:
    source_members = []
    for member in source.getmembers():
        data = source.extractfile(member).read() if member.isfile() else None
        source_members.append((member, data))

root_name = next(
    member.name.rstrip("/")
    for member, _ in source_members
    if member.name.count("/") == 0 and member.isdir()
)


def find_member(members, relative):
    target = f"{root_name}/{relative}"
    for position, (member, data) in enumerate(members):
        if member.name.rstrip("/") == target:
            return position, member, data
    raise RuntimeError(f"member missing from current release fixture: {target}")


def write_case(name, mutation):
    directory = output_root / name
    directory.mkdir()
    output = directory / archive_name
    members = [(copy.copy(member), data) for member, data in source_members]
    mutation(members)
    with tarfile.open(output, "w:xz", format=tarfile.PAX_FORMAT, preset=9) as archive:
        for member, data in members:
            archive.addfile(member, io.BytesIO(data) if data is not None else None)


def missing_binary(members):
    position, _, _ = find_member(members, "bin/emuwiz-cli")
    del members[position]


def modified_binary(members):
    position, member, data = find_member(members, "bin/emuwiz")
    changed = bytearray(data)
    changed[-1] ^= 1
    members[position] = (member, bytes(changed))


def modified_manifest(members):
    position, member, data = find_member(members, "manifest.json")
    member.size = len(data) + 1
    members[position] = (member, data + b" ")


def corrupted_metadata(members):
    position, member, _ = find_member(members, "manifest.json")
    replacement = b"{ this is not JSON\n"
    member.size = len(replacement)
    members[position] = (member, replacement)


def wrong_permissions(members):
    _, member, _ = find_member(members, "bin/emuwiz-cli")
    member.mode = 0o644


def unexpected_member(members):
    info = tarfile.TarInfo(f"{root_name}/unexpected.txt")
    info.mode = 0o644
    info.uid = info.gid = 0
    info.size = len(b"unexpected\n")
    members.append((info, b"unexpected\n"))


def malformed_member(members):
    position, member, _ = find_member(members, "manifest.json")
    member.type = tarfile.DIRTYPE
    member.size = 0
    members[position] = (member, None)


write_case("missing-file", missing_binary)
write_case("modified-binary", modified_binary)
write_case("manifest-mismatch", modified_manifest)
write_case("corrupted-metadata", corrupted_metadata)
write_case("wrong-permissions", wrong_permissions)
write_case("unexpected-member", unexpected_member)
write_case("malformed-package", malformed_member)

# The current package intentionally contains no installer. Keep this assertion
# here so a future packaging change cannot silently revive an old fixture
# assumption without making the test reviewable.
if any(member.name.endswith("/install.sh") for member, _ in source_members):
    raise SystemExit("current tar.xz package unexpectedly contains historical install.sh")
PY

for label_and_reason in \
    "missing-file|expected file missing or symlinked: bin/emuwiz-cli" \
    "modified-binary|SHA-256 mismatch: bin/emuwiz" \
    "manifest-mismatch|manifest SHA-256 does not agree with SHA256SUMS" \
    "corrupted-metadata|manifest.json is unreadable" \
    "wrong-permissions|executable-mode mismatch: bin/emuwiz-cli" \
    "unexpected-member|unexpected payload files in strict mode: unexpected.txt" \
    "malformed-package|manifest.json is missing or unsafe"; do
    label=${label_and_reason%%|*}
    reason=${label_and_reason#*|}
    archive="$TEMP_ROOT/$label/$ARCHIVE_NAME"
    write_checksum "$archive"
    expect_failure "$label" "$archive" "$reason"
done

# Renaming the archive while retaining its original sidecar must fail before
# extraction: the current verifier binds a checksum record to the exact
# .tar.xz filename. This preserves the old renamed-package intent without
# reintroducing the historical .tar.gz naming convention.
mkdir -p "$TEMP_ROOT/renamed-package"
renamed="$TEMP_ROOT/renamed-package/emuwiz-renamed.tar.xz"
cp -- "$VALID_ARTIFACT" "$renamed"
cp -- "$VALID_ARTIFACT.sha256" "$renamed.sha256"
expect_failure renamed-package "$renamed" "archive checksum file is invalid or names a different archive"

release_note "current tar.xz artifact verifier negative tests passed"
