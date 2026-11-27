#!/usr/bin/env bash
# CI runner for the Cargo retention, retirement and worktree safety tests. They
# use fixtures, fake cargo and private HOMEs; the privileged file also uses real
# root through sudo -n, so run this directly only on a disposable machine.
#
# Files are found by glob so a new test file cannot be left out. Every file runs
# even after a failure, and the privileged tests must really run: their only
# accepted skip is the one test gated behind EMUWIZ_TEST_SYSTEMD.
#
# --group N (1-4) runs one balanced quarter, for a CI matrix with one fresh
# runner per group. Groups 1-3 are named below; group 4 takes every other file
# the glob finds, so a new test file always runs somewhere and never twice. A
# named file that no longer exists fails every group. Without --group every
# file runs, as before.
set -uo pipefail

GROUP_COUNT=4
# Balanced from CI timings of 2026-11-26 (about 300 s each). Group 3 also runs
# scripts/test-cargo-iso.sh. Every file not named here belongs to group 4.
group_of() {
  case $1 in
    test_emuwiz_cargo_maintain.py|test_emuwiz_cargo_routine_release.py|\
    test_emuwiz_cargo_retire_resolution.py|test_emuwiz_cargo_signing.py) echo 1 ;;
    test_emuwiz_cargo_retire.py|test_emuwiz_cargo_maintain_remediation.py|\
    test_emuwiz_cargo_retire_residue.py|test_emuwiz_cargo_immediate.py|test_emuwiz_ra_cargo.py|\
    test_emuwiz_cargo_release_merged.py|test_emuwiz_cargo_maintain_redaction.py) echo 2 ;;
    test_emuwiz_legacy_worktree.py|test_emuwiz_cargo_privileged.py|\
    test_emuwiz_cargo_retire_restart.py|test_emuwiz_cargo_incremental.py) echo 3 ;;
    *) echo "$GROUP_COUNT" ;;
  esac
}
# The same names, for the stale-list check: a renamed file must not silently
# fall through to group 4 under its new name while the old name lingers here.
NAMED=$(sed -n 's/^    \(test_emuwiz_[a-z0-9_|.\\]*\)) echo [123] ;;$/\1/p; s/^    \(test_emuwiz_[a-z0-9_|.]*\)|\\$/\1/p' "$0" | tr '|\\' '  ')

group=
if (($#)); then
  [[ $# -eq 2 && $1 == --group && $2 =~ ^[1-9]$ ]] && ((10#$2 <= GROUP_COUNT)) ||
    { echo "usage: $0 [--group 1-$GROUP_COUNT]" >&2; exit 2; }
  group=$2
fi
selected() { [[ -z $group ]] || [[ $(group_of "$1") == "$group" ]]; }

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

for f in $NAMED; do
  [[ -f scripts/$f ]] || { echo "::error::the maintenance groups name a missing test file: scripts/$f"; exit 1; }
done

ran=0
if [[ -z $group || $group == 3 ]]; then
  run scripts/test-cargo-iso.sh bash scripts/test-cargo-iso.sh
  ran=$((ran + 1))
fi
for test in scripts/test_emuwiz_*.py; do
  selected "$(basename "$test")" || continue
  ran=$((ran + 1))
  run "$test" python3 -W error::ResourceWarning "$test"
  if [[ "$test" == */test_emuwiz_cargo_privileged.py ]] && ! grep -qx 'OK (skipped=1)' "$log"; then
    echo "::error::the real-root tests did not run as expected: $(tail -n 1 "$log")"
    failed+=("$test (skips)")
  fi
done
rm -f "$log"
((ran > 0)) || { echo "::error::maintenance group ${group:-all} ran no test files"; exit 1; }
echo "maintenance group ${group:-all}: $ran test files"

if ((${#failed[@]})); then
  echo "::error::maintenance script tests failed: ${failed[*]}"
  exit 1
fi
