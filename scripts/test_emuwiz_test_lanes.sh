#!/usr/bin/env bash
# Targeted tests for scripts/emuwiz-test-lanes using a throwaway repo with stub
# scripts/cargo-iso and scripts/ci_test_shards.py (no Cargo builds, no real locks).
# usage: scripts/test_emuwiz_test_lanes.sh [PATH/TO/emuwiz-test-lanes]  (default: the one beside it)
set -u
SCRIPT=$(readlink -f "${1:-$(dirname "$0")/emuwiz-test-lanes}"); pass=0 fail=0
work=$(mktemp -d /tmp/etltest.XXXXXX); trap 'rm -rf "$work"' EXIT
repo=$work/repo; mkdir -p "$repo/scripts"; git -C "$repo" init -q
cp "$SCRIPT" "$repo/scripts/emuwiz-test-lanes"
cat > "$repo/scripts/ci_test_shards.py" <<'PY'
import json, sys
SH = {"recovery": ["r::"], "dat": ["d::"], "gui-v2": ["g::"], "rest": ["--skip", "r::"]}
cmd = sys.argv[1]
if cmd == "packages": print("--workspace")
elif cmd == "shards": print(json.dumps(list(SH)))
elif cmd == "selection": print("\n".join(SH[sys.argv[2]]))
PY
cat > "$repo/scripts/cargo-iso" <<'SH'
#!/usr/bin/env bash
# stub: record arguments; per-shard behaviour from STUB_SILENT / STUB_FAIL (filter text)
echo "$*" >> "$STUB_LOG"
case " $* " in *" --no-run "*) echo "Finished build"; exit 0;; esac
for f in ${STUB_SILENT:-}; do [[ " $* " == *" $f "* ]] && exit 0; done
for f in ${STUB_FAIL:-}; do [[ " $* " == *" $f "* ]] && { echo "test x ... FAILED"; exit 101; }; done
echo "test result: ok. 1 passed"
SH
chmod +x "$repo/scripts/cargo-iso" "$repo/scripts/emuwiz-test-lanes"
realflock=$(command -v flock)
run() { # name, expected rc, expected output regex, extra env...
  local name=$1 want=$2 re=$3; shift 3
  local out; out=$(cd "$repo" && env STUB_LOG="$work/args.$name" "$@" scripts/emuwiz-test-lanes -c "" -o "$work/out.$name" 2>&1); local rc=$?
  if [[ $rc == "$want" ]] && grep -qE "$re" <<<"$out"; then pass=$((pass+1)); echo "ok   $name (rc=$rc)"
  else fail=$((fail+1)); echo "FAIL $name: rc=$rc (want $want)"; sed 's/^/     /' <<<"$out"; fi
}
# 1. all shards pass -> PASSED, rc 0, all four shards ran
run all-pass 0 'PASSED'
[[ $(grep -c -- '--test-threads' "$work/args.all-pass") == 4 ]] && { pass=$((pass+1)); echo "ok   all-pass ran 4 shards"; } || { fail=$((fail+1)); echo "FAIL all-pass shard count"; }
# 2. no --low-debug (or any debug switch) is ever passed: the target's recorded mode applies
! grep -qE -- '--(low|full)-debug' "$work/args.all-pass" && { pass=$((pass+1)); echo "ok   no debug-mode switch passed"; } || { fail=$((fail+1)); echo "FAIL debug flag passed"; }
# 3. a failing shard -> FAILED, rc 1
run one-shard-fails 1 'FAILED' STUB_FAIL="d::"
# 4. the shard queue lock fails (only the queue lock; cargo-iso is untouched) -> FAILED, never PASSED
shim=$work/shim; mkdir "$shim"
printf '#!/bin/sh\ncase "$1" in *queue.lock) exit 1;; esac\nexec %s "$@"\n' "$realflock" > "$shim/flock"; chmod +x "$shim/flock"
run queue-lock-fails 1 'only 0 of 4 shards ran' PATH="$shim:$PATH"
grep -q PASSED <(cd "$repo" && env STUB_LOG=/dev/null PATH="$shim:$PATH" scripts/emuwiz-test-lanes -c "" -o "$work/out.q2" 2>&1) \
  && { fail=$((fail+1)); echo "FAIL queue-lock-fails printed PASSED"; } || { pass=$((pass+1)); echo "ok   queue-lock-fails never prints PASSED"; }
# 5. a shard that leaves no output (missing shard log) -> FAILED, rc 1
run missing-shard-log 1 'shard gui-v2 has no log' STUB_SILENT="g::"
# 6. flock not installed -> refuses to start, rc 2
nofl=$work/nofl; mkdir "$nofl"
for t in bash git python3 nproc date mkdir mktemp sed grep wc tee cat chmod rm basename sort head readlink env dirname; do
  p=$(command -v $t) && ln -s "$p" "$nofl/$t"; done
run no-flock 2 'needs flock' PATH="$nofl"
# 7. outside a worktree -> rc 2
out=$(cd "$work" && "$repo/scripts/emuwiz-test-lanes" 2>&1); rc=$?
[[ $rc == 2 ]] && { pass=$((pass+1)); echo "ok   outside a worktree (rc=2)"; } || { fail=$((fail+1)); echo "FAIL outside worktree rc=$rc"; }
echo "passed $pass, failed $fail"; (( fail == 0 ))
