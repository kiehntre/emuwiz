#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-rc-selftest.XXXXXX")"
trap 'rm -rf -- "$ROOT"' EXIT
set +e
python3 "$SCRIPT_DIR/run-rc-acceptance.py" --source-tree "$SCRIPT_DIR/../.." \
  --gui-binary "$ROOT/missing-gui" --cli-binary "$ROOT/missing-cli" --output "$ROOT/evidence" \
  >"$ROOT/out" 2>&1
status=$?
set -e
[[ "$status" -eq 3 ]] || { cat "$ROOT/out" >&2; echo "missing-binary self-test failed" >&2; exit 1; }
set +e
python3 "$SCRIPT_DIR/run-rc-acceptance.py" --source-tree "$SCRIPT_DIR/../.." \
  --gui-binary "$ROOT/missing-gui" --cli-binary "$ROOT/missing-cli" --output "/tmp" \
  >"$ROOT/unsafe-out" 2>&1
status=$?
set -e
[[ "$status" -eq 3 ]] || { cat "$ROOT/unsafe-out" >&2; echo "unsafe-output self-test failed" >&2; exit 1; }
python3 -m py_compile "$SCRIPT_DIR/run-rc-acceptance.py"
python3 "$SCRIPT_DIR/run-rc-acceptance.py" --self-test
echo "RC ACCEPTANCE SELF-TEST: PASS"
