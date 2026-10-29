#!/usr/bin/env bash
set -euo pipefail

# Real synthetic two/five-disc acceptance tests need the upstream Linux writer.
# The archive is fetched for this test only, integrity-checked, then removed.
archive_sha=378ecb4d2310c8f72fc77ec092e50797427de79d6e001ab0b56eaad4699890da
if [[ -z ${EMUWIZ_PSXPACKAGER_V163:-} ]]; then
  test_dir=$(mktemp -d /tmp/emuwiz-pbp-writer-XXXXXXXX)
  trap 'rm -rf -- "$test_dir"' EXIT
  curl --fail --location --retry 3 --silent --show-error \
    https://github.com/RupertAvery/PSXPackager/releases/download/v1.6.3/psxpackager-linux-x64.tar.gz \
    -o "$test_dir/writer.tar.gz"
  printf '%s  %s\n' "$archive_sha" "$test_dir/writer.tar.gz" | sha256sum --check --status
  tar -xzf "$test_dir/writer.tar.gz" -C "$test_dir"
  export EMUWIZ_PSXPACKAGER_V163="$test_dir/psxpackager-linux-x64/psxpackager"
fi

command -v genisoimage >/dev/null
test -f "$EMUWIZ_PSXPACKAGER_V163"
if [[ ${CI:-} == true ]]; then
  cargo test -p archivefs-core --lib repair::ps1_pbp_export -- --include-ignored
else
  scripts/cargo-iso --low-debug test -p archivefs-core --lib repair::ps1_pbp_export -- --include-ignored
fi
