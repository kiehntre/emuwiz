#!/usr/bin/env bash
# Clippy warning ratchet: fail if the workspace has any Clippy warning that is
# not recorded in .github/clippy-warning-baseline (one entry per existing
# warning; see scripts/clippy_ratchet.py). Compiler errors and deny-level lints
# still fail Clippy itself (non-zero exit). Fixed warnings are removed with
# --write-baseline, which refuses to add entries; never add entries to make a
# PR pass.
#   CARGO=scripts/cargo-iso scripts/clippy-ratchet.sh   # local isolated build
#   scripts/clippy-ratchet.sh --count                    # print count only
#   scripts/clippy-ratchet.sh --write-baseline           # lower the baseline
set -euo pipefail
cd "$(dirname "$0")/.."
baseline_file=.github/clippy-warning-baseline
json=$(mktemp)
trap 'rm -f "$json"' EXIT

${CARGO:-cargo} clippy --workspace --all-targets --locked --message-format=json >"$json"

case "${1:-}" in
  --count) exec python3 -I scripts/clippy_ratchet.py count "$json" ;;
  --write-baseline) exec python3 -I scripts/clippy_ratchet.py write "$json" "$baseline_file" ;;
  "") exec python3 -I scripts/clippy_ratchet.py check "$json" "$baseline_file" ;;
  *) echo "unknown argument: $1" >&2; exit 2 ;;
esac
