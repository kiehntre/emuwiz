#!/usr/bin/env bash
# Clippy warning ratchet: fail if the workspace has more unique Clippy warnings
# than .github/clippy-warning-baseline. Compiler errors and deny-level lints
# still fail Clippy itself (non-zero exit). Lower the baseline as warnings are
# fixed; never raise it to make a PR pass.
#   CARGO=scripts/cargo-iso scripts/clippy-ratchet.sh   # local isolated build
#   scripts/clippy-ratchet.sh --count                    # print count only
set -euo pipefail
cd "$(dirname "$0")/.."
baseline_file=.github/clippy-warning-baseline
json=$(mktemp)
trap 'rm -f "$json"' EXIT

${CARGO:-cargo} clippy --workspace --all-targets --locked --message-format=json >"$json"

# One warning is reported once per target that compiles the file, so count
# unique (code, file, line, column, message).
count=$(python3 -I - "$json" <<'PY'
import json, sys
seen = set()
for line in open(sys.argv[1], encoding="utf-8"):
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    if msg.get("reason") != "compiler-message":
        continue
    diag = msg["message"]
    if diag.get("level") != "warning" or not diag.get("spans"):
        continue
    span = next((s for s in diag["spans"] if s.get("is_primary")), diag["spans"][0])
    code = (diag.get("code") or {}).get("code", "")
    seen.add((code, span["file_name"], span["line_start"], span["column_start"], diag["message"]))
print(len(seen))
PY
)

if [ "${1:-}" = "--count" ]; then
  echo "$count"
  exit 0
fi
baseline=$(tr -d '[:space:]' <"$baseline_file")
echo "Clippy warnings: $count (baseline $baseline)"
if [ "$count" -gt "$baseline" ]; then
  echo "error: Clippy warnings increased by $((count - baseline)); fix the new warnings." >&2
  exit 1
fi
if [ "$count" -lt "$baseline" ]; then
  echo "note: warnings dropped; lower $baseline_file to $count to lock in the improvement."
fi
