#!/usr/bin/env bash
# Tests for scripts/cargo-iso. Uses a fake cargo and temporary git worktrees under
# /tmp; it builds nothing, deletes nothing outside its own temp directory, and never
# touches ~/.cache/emuwiz-cargo-target(s).
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd -P)
iso="$here/cargo-iso"
tmp=$(mktemp -d /tmp/emuwiz-cargo-iso-test.XXXXXX)
trap 'rm -rf -- "${tmp:?}"' EXIT
pass=0
fail=0

ok() { pass=$((pass + 1)); echo "ok   - $1"; }
bad() { fail=$((fail + 1)); echo "FAIL - $1" >&2; }
check() { # check "name" <command...>  (command succeeds = pass)
  local name=$1; shift
  if "$@" >/dev/null 2>&1; then ok "$name"; else bad "$name"; fi
}

# A fake cargo that records its environment and arguments, and exits as told.
fake="$tmp/fake-cargo"
cat >"$fake" <<'E'
#!/usr/bin/env bash
{
  echo "target=${CARGO_TARGET_DIR-<unset>}"
  echo "build_target=${CARGO_BUILD_TARGET_DIR-<unset>}"
  echo "dev_debug=${CARGO_PROFILE_DEV_DEBUG-<unset>}"
  echo "test_debug=${CARGO_PROFILE_TEST_DEBUG-<unset>}"
  echo "argc=$#"
  i=0; for a in "$@"; do i=$((i + 1)); echo "arg$i=$a"; done
} >"${FAKE_CARGO_LOG:?}"
exit "${FAKE_CARGO_EXIT:-0}"
E
chmod +x "$fake"

# Two real worktrees of one throwaway repo, plus one with an awkward name.
repo="$tmp/repo"
git init -q "$repo"
git -C "$repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
git -C "$repo" worktree add -q "$tmp/wt-one" -b one
git -C "$repo" worktree add -q "$tmp/wt two (odd) \$name" -b two
mkdir -p "$tmp/home"
export HOME="$tmp/home"
export EMUWIZ_TARGET_ROOT="$tmp/targets"
export EMUWIZ_CARGO="$fake"
export FAKE_CARGO_LOG="$tmp/log"
legacy="$HOME/.cache/emuwiz-cargo-target"
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_TEST_DEBUG EMUWIZ_CARGO_LOW_DEBUG

target_of() { (cd "$1" && "$iso" --print-target | sed -n 's/^target: *//p'); }
logged() { sed -n "s/^$1=//p" "$FAKE_CARGO_LOG"; }

# ---- target uniqueness and stability
t1=$(target_of "$tmp/wt-one")
t2=$(target_of "$tmp/wt two (odd) \$name")
check "each worktree gets a target under the root" test "${t1#"$EMUWIZ_TARGET_ROOT"/}" != "$t1"
check "two worktrees get different targets" test "$t1" != "$t2"
check "the target is stable across calls" test "$t1" = "$(target_of "$tmp/wt-one")"
check "a subdirectory of the worktree maps to the same target" \
  test "$t1" = "$(mkdir -p "$tmp/wt-one/sub/dir" && target_of "$tmp/wt-one/sub/dir")"
ln -s "$tmp/wt-one" "$tmp/link-to-one"
check "a symlinked path maps to the canonical worktree's target" test "$t1" = "$(target_of "$tmp/link-to-one")"
check "the name has a 12-hex hash suffix" bash -c "[[ '$(basename "$t1")' =~ ^wt-one-[0-9a-f]{12}$ ]]"

# ---- path safety
base2=$(basename "$t2")
check "an awkward worktree name is made safe" bash -c "[[ '$base2' =~ ^[A-Za-z0-9._-]+$ ]]"
check "the target never escapes the root" test "$(dirname "$t2")" = "$EMUWIZ_TARGET_ROOT"

# ---- read-only --print-target
rm -rf "$EMUWIZ_TARGET_ROOT"
(cd "$tmp/wt-one" && "$iso" --print-target >/dev/null)
check "--print-target creates nothing" test ! -e "$EMUWIZ_TARGET_ROOT"
check "--print-target does not run cargo" test ! -e "$FAKE_CARGO_LOG"

# ---- normal execution: arguments, environment, exit codes
rm -f "$FAKE_CARGO_LOG"
(cd "$tmp/wt-one" && "$iso" test -p "some crate" --lib -- --test-threads=2 "a b" >/dev/null 2>&1); rc=$?
check "exit code 0 is preserved" test "$rc" = 0
check "the target directory is created for the build" test -d "$t1"
check "CARGO_TARGET_DIR is the worktree's target" test "$(logged target)" = "$t1"
check "all arguments pass through, including spaces" test "$(logged argc)" = 7
check "argument order and content are preserved" bash -c "
  [ \"\$(sed -n 's/^arg1=//p' '$FAKE_CARGO_LOG')\" = test ] &&
  [ \"\$(sed -n 's/^arg3=//p' '$FAKE_CARGO_LOG')\" = 'some crate' ] &&
  [ \"\$(sed -n 's/^arg7=//p' '$FAKE_CARGO_LOG')\" = 'a b' ]"
check "no debug override by default" test "$(logged dev_debug)" = "<unset>"
(cd "$tmp/wt-one" && FAKE_CARGO_EXIT=7 "$iso" check >/dev/null 2>&1); rc=$?
check "cargo's non-zero exit code is propagated (7)" test "$rc" = 7
(cd "$tmp/wt-one" && FAKE_CARGO_EXIT=101 "$iso" test >/dev/null 2>&1); rc=$?
check "cargo's test-failure exit code is propagated (101)" test "$rc" = 101

# ---- inherited environment is overridden
(cd "$tmp/wt-one" && CARGO_TARGET_DIR="$legacy" CARGO_BUILD_TARGET_DIR="$legacy" "$iso" check >/dev/null 2>"$tmp/err")
check "an inherited legacy CARGO_TARGET_DIR is overridden" test "$(logged target)" = "$t1"
check "CARGO_BUILD_TARGET_DIR is unset" test "$(logged build_target)" = "<unset>"
check "overriding the legacy target prints a note" grep -q "legacy shared target" "$tmp/err"
(cd "$tmp/wt-one" && CARGO_TARGET_DIR=/some/other/dir "$iso" check >/dev/null 2>&1)
check "any other inherited CARGO_TARGET_DIR is overridden too" test "$(logged target)" = "$t1"

# ---- the legacy shared target can never be used
(cd "$tmp/wt-one" && EMUWIZ_TARGET_ROOT="$legacy" "$iso" check >/dev/null 2>&1); rc=$?
check "a target root equal to the legacy target is refused (3)" test "$rc" = 3
(cd "$tmp/wt-one" && EMUWIZ_TARGET_ROOT="$legacy/inner" "$iso" check >/dev/null 2>&1); rc=$?
check "a target root inside the legacy target is refused (3)" test "$rc" = 3
rm -f "$FAKE_CARGO_LOG"
(cd "$tmp/wt-one" && "$iso" build --target-dir "$legacy" >/dev/null 2>&1); rc=$?
check "--target-dir pointing at the legacy target is refused (3)" test "$rc" = 3
(cd "$tmp/wt-one" && "$iso" build "--target-dir=$legacy/debug" >/dev/null 2>&1); rc=$?
check "--target-dir=<inside legacy> is refused (3)" test "$rc" = 3
check "a refused build never runs cargo" test ! -e "$FAKE_CARGO_LOG"
(cd "$tmp/wt-one" && "$iso" build --target-dir "$tmp/elsewhere" >/dev/null 2>&1); rc=$?
check "an unrelated explicit --target-dir is the caller's choice (allowed)" test "$rc" = 0
(cd "$tmp/wt-one" && "$iso" test -- --target-dir "$legacy" >/dev/null 2>&1); rc=$?
check "arguments after -- are not inspected as cargo options" test "$rc" = 0

# ---- disk-space guard
rm -f "$FAKE_CARGO_LOG"
(cd "$tmp/wt-one" && EMUWIZ_MIN_FREE_GB=99999999 "$iso" check >/dev/null 2>"$tmp/err"); rc=$?
check "too little free disk is refused (4)" test "$rc" = 4
check "a disk refusal never runs cargo" test ! -e "$FAKE_CARGO_LOG"
check "the refusal says nothing was deleted" grep -q "nothing was deleted" "$tmp/err"
(cd "$tmp/wt-one" && EMUWIZ_MIN_FREE_GB=0 "$iso" check >/dev/null 2>&1); rc=$?
check "a zero threshold lets the build run" test "$rc" = 0
(cd "$tmp/wt-one" && EMUWIZ_MIN_FREE_GB=abc "$iso" check >/dev/null 2>&1); rc=$?
check "a non-numeric threshold is a usage error (2)" test "$rc" = 2
check "the default threshold is 8 GiB" bash -c "cd '$tmp/wt-one' && '$iso' --print-target | grep -q 'minimum 8 GiB'"

# ---- low-debug mode
(cd "$tmp/wt-one" && "$iso" --low-debug check >/dev/null 2>&1)
check "--low-debug disables dev debug info" test "$(logged dev_debug)" = 0
check "--low-debug disables test debug info" test "$(logged test_debug)" = 0
check "--low-debug is not passed on to cargo" bash -c "! grep -q 'low-debug' '$FAKE_CARGO_LOG'"
(cd "$tmp/wt-one" && EMUWIZ_CARGO_LOW_DEBUG=1 "$iso" check >/dev/null 2>&1)
check "EMUWIZ_CARGO_LOW_DEBUG=1 does the same" test "$(logged dev_debug)" = 0

# ---- usage errors
(cd "$tmp" && "$iso" check >/dev/null 2>&1); rc=$?
check "outside a git worktree is a usage error (2)" test "$rc" = 2
(cd "$tmp/wt-one" && "$iso" >/dev/null 2>&1); rc=$?
check "no cargo command is a usage error (2)" test "$rc" = 2
check "--help works and exits 0" bash -c "'$iso' --help | grep -q 'unique to THIS git worktree'"

# ---- never deletes
mkdir -p "$t1/debug" && : >"$t1/debug/precious-artifact"
(cd "$tmp/wt-one" && "$iso" check >/dev/null 2>&1)
check "existing build artefacts are never removed" test -e "$t1/debug/precious-artifact"
check "the script contains no delete or clean command" bash -c "! grep -E '(^|[^a-z_])(rm |rmdir|unlink|find .*-delete|cargo clean)' '$iso'"

echo
echo "cargo-iso tests: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
