#!/usr/bin/env bash
set -euo pipefail
repo_root="$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
exec python3 "$repo_root/tools/pending_recovery/inspector.py" "$@"
