#!/usr/bin/env bash
# Offline self-test for sunshine-acceptance-helper.sh. Uses a disposable run
# directory and artifact; touches nothing in the repository or real $HOME.
set -euo pipefail

script_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
helper="$script_dir/sunshine-acceptance-helper.sh"
repo_root="$(CDPATH= cd -- "$script_dir/../.." && pwd -P)"
root="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-sunshine-selftest.XXXXXX")"
trap 'rm -rf -- "$root"' EXIT
export EMUWIZ_QA_RUN_DIR="$root/runs"
export HOME="$root/home"
mkdir -p "$HOME"

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
pass() { printf 'PASS: %s\n' "$*"; }

printf 'qa binary\n' >"$root/artifact"

"$helper" checklist | grep -q 'desktop acceptance checklist' || fail "checklist did not print"
pass "checklist prints"

out="$("$helper" start --artifact "$root/artifact" --log "$root/log")"
run_id="$(printf '%s\n' "$out" | sed -n 's/^sunshine-qa: run id: //p')"
[ -n "$run_id" ] && [ -f "$root/runs/$run_id.txt" ] || fail "start did not write a record outside the repo"
pass "start records outside the repository"

"$helper" end "$run_id" | grep -q 'artifact integrity: OK (unchanged)' || fail "unchanged artifact not reported OK"
pass "unchanged artifact reported OK"

printf 'tampered\n' >>"$root/artifact"
"$helper" end "$run_id" | grep -q 'MISMATCH' || fail "changed artifact not detected"
pass "changed artifact detected"

"$helper" show "$run_id" | grep -q 'artifact_sha256_pre:' || fail "show did not print the record"
pass "show prints the record"

if EMUWIZ_QA_RUN_DIR="$repo_root/docs" "$helper" start --artifact "$root/artifact" >/dev/null 2>&1; then
    fail "a run directory inside the repository was accepted"
fi
[ -z "$(git -C "$repo_root" status --porcelain -- docs 2>/dev/null | grep -F -- '.txt' || true)" ] || fail "repository was polluted"
pass "repository run directory refused"

if "$helper" end no-such-run >/dev/null 2>&1; then fail "unknown run id accepted"; fi
pass "unknown run id refused"
