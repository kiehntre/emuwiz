#!/usr/bin/env bash
# CI runner for the Cargo retention, retirement and worktree safety tests. They
# use fixtures, fake cargo and private HOMEs; the privileged file also uses real
# root through sudo -n, so run this directly only on a disposable machine.
#
# Files are found by glob so a new test file cannot be left out. Every file runs
# even after a failure, and the privileged tests must really run: their only
# accepted skip is the one test gated behind EMUWIZ_TEST_SYSTEMD.
set -uo pipefail

sudo -n true || { echo "::error::passwordless sudo is required so the real-root tests run"; exit 1; }

cd "$(dirname "$0")/.."
log=$(mktemp)
failed=()

run() {
  local name=$1 started=$SECONDS status=0
  shift
  "$@" > "$log" 2>&1 || status=$?
  printf '%-50s exit %s, %3ss: %s\n' "$name" "$status" "$((SECONDS - started))" "$(tail -n 1 "$log")"
  if ((status != 0)); then
    cat "$log"
    failed+=("$name")
  fi
}

run scripts/test-cargo-iso.sh bash scripts/test-cargo-iso.sh
for test in scripts/test_emuwiz_*.py; do
  run "$test" python3 -W error::ResourceWarning "$test"
  if [[ "$test" == */test_emuwiz_cargo_privileged.py ]] && ! grep -qx 'OK (skipped=1)' "$log"; then
    echo "::error::the real-root tests did not run as expected: $(tail -n 1 "$log")"
    failed+=("$test (skips)")
  fi
done
rm -f "$log"

if ((${#failed[@]})); then
  echo "::error::maintenance script tests failed: ${failed[*]}"
  exit 1
fi
