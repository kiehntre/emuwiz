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
  echo "incremental=${CARGO_INCREMENTAL-<unset>}"
  echo "wrapper=${RUSTC_WRAPPER-<unset>}"
  echo "sccache_dir=${SCCACHE_DIR-<unset>}"
  echo "sccache_size=${SCCACHE_CACHE_SIZE-<unset>}"
  echo "sccache_port=${SCCACHE_SERVER_PORT-<unset>}"
  echo "slot=${EMUWIZ_CARGO_SLOT-<unset>}"
  echo "argc=$#"
  i=0; for a in "$@"; do i=$((i + 1)); echo "arg$i=$a"; done
} >"${FAKE_CARGO_LOG:?}"
# Like a real build's first rustc: call the compiler wrapper (sccache) once.
if [ -n "${FAKE_CARGO_RUN_WRAPPER:-}" ] && [ -n "${RUSTC_WRAPPER:-}" ]; then
  "$RUSTC_WRAPPER" rustc -vV >/dev/null 2>&1 || true
fi
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
# The sections before "auto-designation" test the explicit policy on its own.
export EMUWIZ_CARGO_AUTO_DESIGNATE=0
unset EMUWIZ_CARGO_MAX_BUILDS EMUWIZ_CARGO_SLOT EMUWIZ_CARGO_SLOT_DIR EMUWIZ_CARGO_MAX_DEV_WORKTREES EMUWIZ_DESIGNATION_ROOTS_FILE EMUWIZ_DESIGNATION_LOCK
export EMUWIZ_DESIGNATION_ROOTS=""  # never count this host's real roots
# An inherited machine guard (e.g. EMUWIZ_MIN_FREE_GB=110) must not change the defaults tested here.
unset EMUWIZ_MIN_FREE_GB
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_TEST_DEBUG EMUWIZ_CARGO_LOW_DEBUG EMUWIZ_CARGO_FULL_DEBUG EMUWIZ_CARGO_INCREMENTAL EMUWIZ_CARGO_SCCACHE EMUWIZ_SCCACHE_DIR EMUWIZ_SCCACHE_SIZE EMUWIZ_SCCACHE_PORT RUSTC_WRAPPER SCCACHE_DIR

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
check "a new target defaults to low-debug" test "$(logged dev_debug)/$(logged test_debug)" = 0/0
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

# ---- debug mode is recorded per target and never switches silently
mode_of() { cat "$EMUWIZ_TARGET_ROOT.debug-mode/$(basename "$1")"; }
check "the new target's mode was recorded outside the target" test "$(mode_of "$t1")" = low
check "nothing was written into the target root for it" test ! -e "$EMUWIZ_TARGET_ROOT/.debug-mode"
(cd "$tmp/wt-one" && CARGO_PROFILE_DEV_DEBUG=2 "$iso" check >/dev/null 2>&1)
check "a recorded low mode is reused (inherited debug=2 does not switch it)" test "$(logged dev_debug)" = 0
(cd "$tmp/wt-one" && "$iso" --full-debug check >/dev/null 2>"$tmp/err")
check "--full-debug is an explicit override" test "$(logged dev_debug)/$(logged test_debug)" = "<unset>/<unset>"
check "an explicit switch is announced" grep -q "changes from low to full" "$tmp/err"
check "--full-debug is not passed on to cargo" bash -c "! grep -q 'full-debug' '$FAKE_CARGO_LOG'"
check "the switch is recorded" test "$(mode_of "$t1")" = full
(cd "$tmp/wt-one" && CARGO_PROFILE_DEV_DEBUG=0 "$iso" check >/dev/null 2>"$tmp/err")
check "a recorded full mode stays full (inherited debug=0 is cleared)" test "$(logged dev_debug)" = "<unset>"
check "reusing the recorded mode is silent" test ! -s "$tmp/err"
(cd "$tmp/wt-one" && "$iso" --print-target | grep -q '^debug: *full (recorded)')
check "--print-target shows the recorded mode" test $? = 0
(cd "$tmp/wt-one" && EMUWIZ_CARGO_LOW_DEBUG=1 EMUWIZ_CARGO_FULL_DEBUG=1 "$iso" check >/dev/null 2>&1); rc=$?
check "both debug env overrides at once is a usage error (2)" test "$rc" = 2
(cd "$tmp/wt-one" && "$iso" --low-debug check >/dev/null 2>&1)
check "--low-debug switches back and records it" test "$(mode_of "$t1")/$(logged dev_debug)" = low/0
# An existing target from before the record: unchanged behaviour, nothing recorded.
wt3="$tmp/wt-three"; git -C "$repo" worktree add -q "$wt3" -b three
t3=$(target_of "$wt3"); mkdir -p "$t3/debug"; : >"$t3/debug/old-artifact"
(cd "$wt3" && "$iso" check >/dev/null 2>&1)
check "an unrecorded existing target is built exactly as before" test "$(logged dev_debug)" = "<unset>"
check "an unrecorded existing target is not given a guessed record" test ! -e "$EMUWIZ_TARGET_ROOT.debug-mode/$(basename "$t3")"
(cd "$wt3" && CARGO_PROFILE_DEV_DEBUG=0 "$iso" check >/dev/null 2>&1)
check "an unrecorded existing target keeps inherited settings" test "$(logged dev_debug)" = 0
check "--print-target says it is unrecorded" bash -c "cd '$wt3' && '$iso' --print-target | grep -q '^debug: *inherited'"
check "its artefacts are untouched" test -e "$t3/debug/old-artifact"
# A record that cannot be read refuses; an explicit choice repairs it.
printf 'banana\n' >"$EMUWIZ_TARGET_ROOT.debug-mode/$(basename "$t3")"
rm -f "$FAKE_CARGO_LOG"
(cd "$wt3" && "$iso" check >/dev/null 2>"$tmp/err"); rc=$?
check "an invalid record refuses the build (3)" test "$rc" = 3
check "an invalid-record refusal never runs cargo" test ! -e "$FAKE_CARGO_LOG"
check "the refusal names the fix" grep -q -- "--low-debug or --full-debug" "$tmp/err"
check "--print-target reports an invalid record without failing" bash -c "cd '$wt3' && '$iso' --print-target | grep -q '^debug: *unknown'"
(cd "$wt3" && "$iso" --full-debug check >/dev/null 2>&1); rc=$?
check "an explicit mode repairs an invalid record" test "$rc/$(mode_of "$t3")" = 0/full
# A target that cannot be recorded is not built (it would fall back silently later).
wt4="$tmp/wt-four"; git -C "$repo" worktree add -q "$wt4" -b four
mv "$EMUWIZ_TARGET_ROOT.debug-mode" "$tmp/saved-modes"; : >"$EMUWIZ_TARGET_ROOT.debug-mode"
rm -f "$FAKE_CARGO_LOG"
(cd "$wt4" && "$iso" check >/dev/null 2>&1); rc=$?
check "an unrecordable new target is refused (3)" test "$rc" = 3
check "an unrecordable target never runs cargo" test ! -e "$FAKE_CARGO_LOG"
rm -f "$EMUWIZ_TARGET_ROOT.debug-mode"; mv "$tmp/saved-modes" "$EMUWIZ_TARGET_ROOT.debug-mode"
(cd "$wt4" && "$iso" check >/dev/null 2>&1)
check "the refused (still empty) target is new again afterwards" test "$(mode_of "$(target_of "$wt4")")" = low

# ---- usage errors
(cd "$tmp" && "$iso" check >/dev/null 2>&1); rc=$?
check "outside a git worktree is a usage error (2)" test "$rc" = 2
(cd "$tmp/wt-one" && "$iso" >/dev/null 2>&1); rc=$?
check "no cargo command is a usage error (2)" test "$rc" = 2
check "--help works and exits 0" bash -c "'$iso' --help | grep -q 'unique to THIS git worktree'"

# ---- incremental mode: one policy. explicit flag > EMUWIZ_CARGO_INCREMENTAL > worktree designation > off
run1() { (cd "$tmp/wt-one" && "$@" >/dev/null 2>&1); }
inc_file="$EMUWIZ_TARGET_ROOT.incremental-mode/$(basename "$t1")"
run1 env CARGO_INCREMENTAL=0 "$iso" check
check "undesignated: inherited CARGO_INCREMENTAL=0 is left alone" test "$(logged incremental)" = 0
run1 env -u CARGO_INCREMENTAL "$iso" check
check "undesignated: CARGO_INCREMENTAL stays unset" test "$(logged incremental)" = "<unset>"
run1 env CARGO_INCREMENTAL=0 "$iso" --incremental check
check "--incremental sets CARGO_INCREMENTAL=1" test "$(logged incremental)" = 1
check "--incremental is not passed on to cargo" bash -c "! grep -q 'incremental' <(grep '^arg' '$FAKE_CARGO_LOG')"
run1 env CARGO_INCREMENTAL=0 EMUWIZ_CARGO_INCREMENTAL=1 "$iso" check
check "EMUWIZ_CARGO_INCREMENTAL=1 does the same" test "$(logged incremental)" = 1
run1 env EMUWIZ_CARGO_INCREMENTAL=true CARGO_INCREMENTAL=0 "$iso" check
check "a malformed EMUWIZ_CARGO_INCREMENTAL is ignored, as before" test "$(logged incremental)" = 0
(cd "$tmp/wt-one" && EMUWIZ_CARGO_INCREMENTAL=true "$iso" --print-target >/dev/null 2>&1); rc=$?
check "a malformed EMUWIZ_CARGO_INCREMENTAL does not break --print-target" test "$rc" = 0
run1 env CARGO_INCREMENTAL=1 "$iso" --no-incremental check
check "--no-incremental forces CARGO_INCREMENTAL=0 over an inherited 1" test "$(logged incremental)" = 0
run1 env CARGO_INCREMENTAL=1 EMUWIZ_CARGO_INCREMENTAL=0 "$iso" check
check "EMUWIZ_CARGO_INCREMENTAL=0 forces 0 over an inherited 1" test "$(logged incremental)" = 0
run1 env CARGO_INCREMENTAL=1 "$iso" check
check "undesignated, no choice: an inherited 1 is left alone" test "$(logged incremental)" = 1
check "--print-target says incremental is off by default" bash -c "cd '$tmp/wt-one' && '$iso' --print-target | grep -q '^incremental: off (default'"

(cd "$tmp/wt-one" && "$iso" --designate-dev >/dev/null 2>&1); rc=$?
check "--designate-dev succeeds and runs no cargo" test "$rc" = 0
check "--designate-dev records outside the target" test "$(cat "$inc_file")" = on
check "--print-target reports the designation" bash -c "cd '$tmp/wt-one' && '$iso' --print-target | grep -q '^incremental: on (designated'"
run1 env CARGO_INCREMENTAL=0 "$iso" check
check "designated: incremental is on despite inherited CARGO_INCREMENTAL=0" test "$(logged incremental)" = 1
run1 env -u CARGO_INCREMENTAL "$iso" check
check "designated: incremental is on with nothing inherited" test "$(logged incremental)" = 1
run1 env CARGO_INCREMENTAL=0 "$iso" --no-incremental check
check "designated: --no-incremental opts out (inherited value untouched)" test "$(logged incremental)" = 0
check "--no-incremental is not passed on to cargo" bash -c "! grep -q 'incremental' <(grep '^arg' '$FAKE_CARGO_LOG')"
run1 env EMUWIZ_CARGO_INCREMENTAL=0 CARGO_INCREMENTAL=0 "$iso" check
check "designated: EMUWIZ_CARGO_INCREMENTAL=0 leaves inherited 0" test "$(logged incremental)" = 0
run1 env EMUWIZ_CARGO_INCREMENTAL=0 "$iso" --incremental check
check "the command line beats the environment" test "$(logged incremental)" = 1
check "another worktree is not affected by the designation" bash -c "cd '$tmp/wt two (odd) \$name' && CARGO_INCREMENTAL=0 '$iso' check >/dev/null 2>&1 && test \"\$(sed -n 's/^incremental=//p' '$FAKE_CARGO_LOG')\" = 0"
printf 'garbage\n' >"$inc_file"
run1 env CARGO_INCREMENTAL=0 "$iso" check
check "an invalid designation record fails closed (off)" test "$(logged incremental)" = 0
for combo in "--print-target" "--finish-task" "build"; do
  (cd "$tmp/wt-one" && "$iso" --designate-dev $combo >/dev/null 2>&1); rc=$?
  check "--designate-dev with '$combo' is refused (2)" test "$rc" = 2
done
(cd "$tmp/wt-one" && "$iso" --undesignate-dev build >/dev/null 2>&1); rc=$?
check "--undesignate-dev with a cargo command is refused (2)" test "$rc" = 2
rm -f "$inc_file"; ln -s /etc/hostname "$inc_file"
run1 env CARGO_INCREMENTAL=0 "$iso" check
check "a symlinked designation record is ignored (off)" test "$(logged incremental)" = 0
rm -f "$inc_file"
(cd "$tmp/wt-one" && "$iso" --designate-dev >/dev/null 2>&1 && "$iso" --undesignate-dev >/dev/null 2>&1); rc=$?
check "--undesignate-dev succeeds" test "$rc" = 0
run1 env CARGO_INCREMENTAL=0 "$iso" check
check "--undesignate-dev returns the worktree to non-incremental" test "$(logged incremental)" = 0

# ---- sccache mode (opt-in)
fakebin="$tmp/fakebin"; mkdir -p "$fakebin"; printf '#!/bin/sh\nexit 0\n' >"$fakebin/sccache"; chmod +x "$fakebin/sccache"
(cd "$tmp/wt-one" && "$iso" check >/dev/null 2>&1)
check "sccache is not used by default" test "$(logged wrapper)" = "<unset>"
(cd "$tmp/wt-one" && PATH="$fakebin:$PATH" CARGO_INCREMENTAL=1 "$iso" --sccache check >/dev/null 2>&1); rc=$?
check "--sccache runs" test "$rc" = 0
check "--sccache sets RUSTC_WRAPPER" test "$(logged wrapper)" = "$fakebin/sccache"
check "--sccache forces incremental off" test "$(logged incremental)" = 0
check "--sccache caps the cache at 2G by default" test "$(logged sccache_size)" = 2G
check "--sccache uses a dedicated dir under HOME" test "$(logged sccache_dir)" = "$HOME/.cache/emuwiz-sccache"
check "--sccache uses a private server port" test "$(logged sccache_port)" = 4241
check "--sccache keeps the per-worktree target" test "$(logged target)" = "$t1"
check "--sccache is not passed on to cargo" bash -c "! grep -q 'sccache' <(grep '^arg' '$FAKE_CARGO_LOG')"
(cd "$tmp/wt-one" && PATH="$fakebin:$PATH" "$iso" --sccache --incremental check >/dev/null 2>&1); rc=$?
check "--sccache with --incremental is refused (2)" test "$rc" = 2
(cd "$tmp/wt-one" && "$iso" --designate-dev >/dev/null 2>&1)
(cd "$tmp/wt-one" && PATH="$fakebin:$PATH" CARGO_INCREMENTAL=1 "$iso" --sccache check >/dev/null 2>&1); rc=$?
check "designated + --sccache runs (not an error)" test "$rc" = 0
check "designated + --sccache is built non-incremental" test "$(logged incremental)" = 0
check "designated + --sccache still sets RUSTC_WRAPPER" test "$(logged wrapper)" = "$fakebin/sccache"
(cd "$tmp/wt-one" && PATH="$fakebin:$PATH" "$iso" --sccache --incremental check >/dev/null 2>&1); rc=$?
check "designated + --sccache + --incremental is still refused (2)" test "$rc" = 2
(cd "$tmp/wt-one" && PATH="$fakebin:$PATH" EMUWIZ_CARGO_INCREMENTAL=1 "$iso" --sccache check >/dev/null 2>&1); rc=$?
check "--sccache + EMUWIZ_CARGO_INCREMENTAL=1 is refused (2)" test "$rc" = 2
(cd "$tmp/wt-one" && PATH="$fakebin:$PATH" "$iso" --sccache --no-incremental check >/dev/null 2>&1); rc=$?
check "--sccache + --no-incremental runs" test "$rc" = 0
check "--print-target explains designated + --sccache" bash -c "cd '$tmp/wt-one' && PATH='$fakebin:'\$PATH '$iso' --sccache --print-target | grep -q 'incremental: off (designated, but --sccache'"
(cd "$tmp/wt-one" && "$iso" --undesignate-dev >/dev/null 2>&1)
(cd "$tmp/wt-one" && PATH="/usr/bin-none:$PATH" EMUWIZ_CARGO_SCCACHE=0 "$iso" check >/dev/null 2>&1); rc=$?
check "EMUWIZ_CARGO_SCCACHE=0 is the default" test "$rc" = 0
(cd "$tmp/wt-one" && PATH="$fakebin:$PATH" EMUWIZ_SCCACHE_SIZE=lots "$iso" --sccache check >/dev/null 2>&1); rc=$?
check "a malformed cache size is refused (2)" test "$rc" = 2
(cd "$tmp/wt-one" && PATH="$fakebin:$PATH" EMUWIZ_SCCACHE_SIZE=512M EMUWIZ_SCCACHE_DIR="$tmp/sc" "$iso" --sccache check >/dev/null 2>&1)
check "size and dir overrides are honoured" test "$(logged sccache_size)/$(logged sccache_dir)" = "512M/$tmp/sc"

# ---- --sccache: the long-lived server never inherits a build's lifecycle lock or build slot
# A fake sccache that behaves like the real client: "--start-server" starts the server
# unless one runs, and a compiler call starts it too when none runs. The "server"
# records the descriptors it inherited, then lives until the test releases it.
fakebin2="$tmp/fakebin2"; mkdir -p "$fakebin2"
cat >"$fakebin2/sccache" <<'E'
#!/usr/bin/env bash
s="${FAKE_SCCACHE_STATE:?}"
if [ "${1:-}" = "--start-server" ] && [ -n "${FAKE_SCCACHE_START_FAILS:-}" ]; then exit 1; fi
if [ ! -e "$s/server.pid" ]; then
  (
    ls -l "/proc/$BASHPID/fd" >"$s/server-fds" 2>&1
    echo "${1:-}" >"$s/started-by"
    timeout 60 cat "$s/stop" >/dev/null
  ) </dev/null >/dev/null 2>&1 &
  echo $! >"$s/server.pid"
fi
exit 0
E
chmod +x "$fakebin2/sccache"
srv="$tmp/sccache-state"; mkdir -p "$srv"; mkfifo "$srv/stop"
server_alive() { [ -e "$srv/server.pid" ] && kill -0 "$(cat "$srv/server.pid")" 2>/dev/null; }
# Release the fake server (it is not our child, so wait by polling, bounded).
stop_server() {
  local n=0
  if server_alive; then echo >"$srv/stop"; fi
  while server_alive && [ "$n" -lt 100 ]; do n=$((n + 1)); sleep 0.05; done
  rm -f "$srv/server.pid" "$srv/server-fds" "$srv/started-by"
}
git -C "$repo" worktree add -q "$tmp/wt-sc" -b sccache-server
# The build runs inside a build-slot style wrapper holding fd 9 on a lock, as
# 2026-11-13-dev-batch/build-slot does.
sc_build() {
  (
    exec 9>"$tmp/outer-slot.lock"; flock 9
    cd "$tmp/wt-sc" && PATH="$fakebin2:$PATH" FAKE_SCCACHE_STATE="$srv" FAKE_CARGO_RUN_WRAPPER=1 "$@" "$iso" --sccache check
  ) >/dev/null 2>&1
}
sc_build env; rc=$?
check "--sccache with a server-starting compiler builds" test "$rc" = 0
check "the sccache server is started by cargo-iso before the build" test "$(cat "$srv/started-by" 2>/dev/null)" = "--start-server"
check "the sccache server holds no lifecycle lock" bash -c "test -s '$srv/server-fds' && ! grep -q 'emuwiz-cargo-retention/locks' '$srv/server-fds'"
check "the sccache server holds no build-slot lock" bash -c "test -s '$srv/server-fds' && ! grep -qE 'outer-slot.lock|emuwiz-cargo-build-slots' '$srv/server-fds'"
check "the sccache server outlives the build" server_alive
(cd "$tmp/wt-sc" && "$iso" --finish-task >"$tmp/finish-out" 2>&1); rc=$?
check "--finish-task releases the target while the server keeps running" bash -c "test '$rc' = 0 && ! grep -q 'active cargo-iso build' '$tmp/finish-out'"
check "the server is still running after --finish-task" server_alive
stop_server
# Protection is not weakened: a process the build itself starts still holds the
# lifecycle lock (here a server started by the compiler call because the early start
# failed), so completion is refused while it lives, exactly as before.
sc_build env FAKE_SCCACHE_START_FAILS=1; rc=$?
check "a failed early server start never blocks the build" test "$rc" = 0
check "the compiler call then starts the server inside the build" test "$(cat "$srv/started-by" 2>/dev/null)" = "rustc"
check "a process started inside the build still holds the lifecycle lock" grep -q 'emuwiz-cargo-retention/locks' "$srv/server-fds"
(cd "$tmp/wt-sc" && "$iso" --finish-task >"$tmp/finish-out" 2>&1)
check "--finish-task is refused while a process from inside the build lives" grep -q 'active cargo-iso build' "$tmp/finish-out"
stop_server
(cd "$tmp/wt-sc" && "$iso" --finish-task >"$tmp/finish-out" 2>&1); rc=$?
check "--finish-task releases once that process has exited" bash -c "test '$rc' = 0 && ! grep -q 'active cargo-iso build' '$tmp/finish-out'"

# ---- automatic designation is OFF by default (owner decision): opt in with EMUWIZ_CARGO_AUTO_DESIGNATE=1
git -C "$repo" worktree add -q "$tmp/wt-default" -b default
(cd "$tmp/wt-default" && env -u EMUWIZ_CARGO_AUTO_DESIGNATE CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>&1)
check "by default a new branch worktree is not designated" test -z "$(cat "$EMUWIZ_TARGET_ROOT.incremental-mode/$(basename "$(target_of "$tmp/wt-default")")" 2>/dev/null)"
check "by default a new branch worktree keeps the inherited 0" test "$(logged incremental)" = 0

# ---- opt-in auto-designation: a NEW developer worktree (on a branch) becomes incremental
export EMUWIZ_CARGO_AUTO_DESIGNATE=1
git -C "$repo" worktree add -q "$tmp/wt-new" -b new
git -C "$repo" worktree add -q --detach "$tmp/wt-detached"
git -C "$repo" worktree add -q "$tmp/wt-rel" -b rel
git -C "$repo" worktree add -q "$tmp/wt-explicit" -b explicit
git -C "$repo" worktree add -q "$tmp/wt-old" -b old
git -C "$repo" worktree add -q "$tmp/wt-off" -b off
inc_of() { cat "$EMUWIZ_TARGET_ROOT.incremental-mode/$(basename "$(target_of "$1")")" 2>/dev/null; }
check "--print-target says a new branch worktree will be incremental" bash -c "cd '$tmp/wt-new' && '$iso' --print-target | grep -q '^incremental: on (new developer worktree'"
check "--print-target records nothing" test -z "$(inc_of "$tmp/wt-new")"
(cd "$tmp/wt-new" && CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>"$tmp/err"); rc=$?
check "the first build of a new branch worktree succeeds" test "$rc" = 0
check "it is built incremental despite an inherited 0" test "$(logged incremental)" = 1
check "it is recorded as designated" test "$(inc_of "$tmp/wt-new")" = on
check "the designation is announced with its undo" grep -q 'undesignate-dev' "$tmp/err"
(cd "$tmp/wt-new" && CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>&1)
check "later builds stay incremental" test "$(logged incremental)" = 1
(cd "$tmp/wt-detached" && CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>&1)
check "a detached (review/verification) worktree is not designated" test -z "$(inc_of "$tmp/wt-detached")"
check "a detached worktree keeps the inherited 0" test "$(logged incremental)" = 0
(cd "$tmp/wt-rel" && CARGO_INCREMENTAL=0 "$iso" build --release >/dev/null 2>&1)
check "a first release build designates nothing" test -z "$(inc_of "$tmp/wt-rel")"
check "a first release build keeps the inherited 0" test "$(logged incremental)" = 0
(cd "$tmp/wt-explicit" && CARGO_INCREMENTAL=0 "$iso" --no-incremental check >/dev/null 2>&1)
check "an explicit choice on the first build designates nothing" test -z "$(inc_of "$tmp/wt-explicit")"
mkdir -p "$(target_of "$tmp/wt-old")"
(cd "$tmp/wt-old" && CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>&1)
check "an existing target is never auto-designated" test -z "$(inc_of "$tmp/wt-old")"
check "an existing undesignated target keeps the inherited 0" test "$(logged incremental)" = 0
(cd "$tmp/wt-off" && "$iso" --undesignate-dev >/dev/null 2>&1 && CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>&1)
check "an --undesignate-dev record is respected for a new target" test "$(inc_of "$tmp/wt-off")" = off
check "undesignated new target keeps the inherited 0" test "$(logged incremental)" = 0
(cd "$tmp/wt-new" && CARGO_INCREMENTAL=0 "$iso" build --release >/dev/null 2>&1)
check "a designated worktree's release build is not incremental" test "$(logged incremental)" = 0
(cd "$tmp/wt-new" && CARGO_INCREMENTAL=0 "$iso" build --profile release >/dev/null 2>&1)
check "--profile release is a release build too" test "$(logged incremental)" = 0
(cd "$tmp/wt-new" && CARGO_INCREMENTAL=0 "$iso" --incremental build --release >/dev/null 2>&1)
check "an explicit --incremental still wins for a release build" test "$(logged incremental)" = 1
(cd "$tmp/wt-new" && CARGO_INCREMENTAL=0 "$iso" bench --profile=bench >/dev/null 2>&1)
check "another non-dev profile is not incremental" test "$(logged incremental)" = 0
(cd "$tmp/wt-new" && CARGO_INCREMENTAL=0 "$iso" test --profile test >/dev/null 2>&1)
check "--profile test stays incremental in a designated worktree" test "$(logged incremental)" = 1
# owner limit: at most 2 designated worktrees in total (wt-new is one already)
git -C "$repo" worktree add -q "$tmp/wt-second" -b second
git -C "$repo" worktree add -q "$tmp/wt-third" -b third
(cd "$tmp/wt-second" && CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>&1)
check "a second new worktree is designated" test "$(inc_of "$tmp/wt-second")" = on
check "--print-target names the full designation limit" bash -c "cd '$tmp/wt-third' && '$iso' --print-target | grep -q '^incremental: off (new developer worktree, but 2 of 2'"
(cd "$tmp/wt-third" && CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>"$tmp/err"); rc=$?
check "a third new worktree still builds" test "$rc" = 0
check "a third new worktree is not designated" test -z "$(inc_of "$tmp/wt-third")"
check "a third new worktree is built non-incremental" test "$(logged incremental)" = 0
check "the limit is explained" grep -q '2 of 2 developer worktrees are already designated' "$tmp/err"
(cd "$tmp/wt-second" && "$iso" --undesignate-dev >/dev/null 2>&1)
rm -rf "$(target_of "$tmp/wt-third")"
(cd "$tmp/wt-third" && CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>&1)
check "freeing a designation lets the next new worktree be designated" test "$(inc_of "$tmp/wt-third")" = on
check "the limit is counted from records reading on" test "$(grep -lx on "$EMUWIZ_TARGET_ROOT.incremental-mode"/* | wc -l)" = 2
before=$(cat "$EMUWIZ_TARGET_ROOT.incremental-mode/$(basename "$t1")" 2>/dev/null)
(cd "$tmp/wt-one" && "$iso" --designate-dev >/dev/null 2>"$tmp/err"); rc=$?
check "an explicit --designate-dev at the limit is refused (3)" test "$rc" = 3
check "the refusal says how to free one" grep -q 'undesignate-dev in an idle worktree' "$tmp/err"
check "the refused record is unchanged" test "$(cat "$EMUWIZ_TARGET_ROOT.incremental-mode/$(basename "$t1")" 2>/dev/null)" = "$before"
(cd "$tmp/wt-new" && "$iso" --designate-dev >/dev/null 2>&1); rc=$?
check "re-designating an already designated worktree at the limit is fine" test "$rc" = 0
(cd "$tmp/wt-one" && EMUWIZ_CARGO_MAX_DEV_WORKTREES=3 "$iso" --designate-dev >/dev/null 2>&1 && "$iso" --undesignate-dev >/dev/null 2>&1); rc=$?
check "the limit can be raised explicitly" test "$rc" = 0
(cd "$tmp/wt-one" && EMUWIZ_CARGO_MAX_DEV_WORKTREES=x "$iso" check >/dev/null 2>&1); rc=$?
check "a malformed EMUWIZ_CARGO_MAX_DEV_WORKTREES is refused (2)" test "$rc" = 2

# ---- the limit is TWO IN TOTAL across target roots (home cache and e.g. /mnt/nvme2), not per root
home_root="$HOME/.cache/emuwiz-cargo-targets"
in_home() { env -u EMUWIZ_TARGET_ROOT "$@"; }
home_rec() { cat "$home_root.incremental-mode/$(basename "$(cd "$1" && in_home "$iso" --print-target | sed -n 's/^target: *//p')")" 2>/dev/null; }
(cd "$tmp/wt-third" && "$iso" --undesignate-dev >/dev/null 2>&1)
check "one designation left in the main root" test "$(grep -lx on "$EMUWIZ_TARGET_ROOT.incremental-mode"/* | wc -l)" = 1
git -C "$repo" worktree add -q "$tmp/wt-homeroot" -b homeroot
git -C "$repo" worktree add -q "$tmp/wt-cross" -b cross
(cd "$tmp/wt-homeroot" && in_home "$iso" --designate-dev >/dev/null 2>&1); rc=$?
check "one designation in the home root is allowed (one per root, two in total)" test "$rc/$(home_rec "$tmp/wt-homeroot")" = "0/on"
(cd "$tmp/wt-cross" && "$iso" --designate-dev >/dev/null 2>"$tmp/err"); rc=$?
check "a third designation in the main root is refused (3)" test "$rc" = 3
(cd "$tmp/wt-homeroot" && in_home "$iso" --designate-dev >/dev/null 2>&1); rc=$?
check "re-designating an already designated worktree at the limit still passes" test "$rc/$(home_rec "$tmp/wt-homeroot")" = "0/on"
main_root="$EMUWIZ_TARGET_ROOT"
(cd "$tmp/wt-cross" && EMUWIZ_TARGET_ROOT="$tmp/listed-root" EMUWIZ_DESIGNATION_ROOTS_FILE="$tmp/no-roots" EMUWIZ_DESIGNATION_ROOTS="$main_root" "$iso" --designate-dev >/dev/null 2>&1); rc=$?
check "roots named in EMUWIZ_DESIGNATION_ROOTS are counted without the roots file" test "$rc" = 3
check "the refusal lists the records of every root" grep -q "$home_root.incremental-mode" "$tmp/err"
(cd "$tmp/wt-cross" && in_home "$iso" --designate-dev >/dev/null 2>&1); rc=$?
check "a third designation in the home root is refused (3)" test "$rc/$(home_rec "$tmp/wt-cross")" = "3/"
(cd "$tmp/wt-cross" && EMUWIZ_TARGET_ROOT="$tmp/third-root" "$iso" --designate-dev >/dev/null 2>&1); rc=$?
check "a third designation in a root never used before is refused too (roots file)" test "$rc" = 3
check "the roots file lists the roots that built or designated" bash -c "grep -qxF '$EMUWIZ_TARGET_ROOT' '$HOME/.cache/emuwiz-cargo-designation-roots' && grep -qxF '$home_root' '$HOME/.cache/emuwiz-cargo-designation-roots'"
check "one designation lock is shared by every root" test -e "$HOME/.cache/emuwiz-cargo-designation.lock"
(cd "$tmp/wt-cross" && in_home env EMUWIZ_CARGO_AUTO_DESIGNATE=1 CARGO_INCREMENTAL=0 "$iso" check >/dev/null 2>&1)
check "automatic designation in the other root also respects the total" test "$(logged incremental)/$(home_rec "$tmp/wt-cross")" = "0/"
(cd "$tmp/wt-homeroot" && in_home "$iso" --undesignate-dev >/dev/null 2>&1)
(cd "$tmp/wt-cross" && "$iso" --designate-dev >/dev/null 2>&1); rc=$?
check "freeing the home-root designation frees a place in the main root" test "$rc" = 0
(cd "$tmp/wt-cross" && "$iso" --undesignate-dev >/dev/null 2>&1)
(cd "$tmp/wt-new" && "$iso" --undesignate-dev >/dev/null 2>&1)
export EMUWIZ_CARGO_AUTO_DESIGNATE=0

# ---- concurrent builds: at most EMUWIZ_CARGO_MAX_BUILDS (default 2), waiting on the lock
slots="$HOME/.cache/emuwiz-cargo-build-slots"
(cd "$tmp/wt-one" && "$iso" check >/dev/null 2>&1)
check "a build takes a slot and exports it" test "$(logged slot)" = 0
check "the slot files live under the user's cache" test -e "$slots/slot-0.lock"
mkfifo "$tmp/release-0" "$tmp/release-1"
flock "$slots/slot-0.lock" cat "$tmp/release-0" >/dev/null &
h0=$!
flock "$slots/slot-1.lock" cat "$tmp/release-1" >/dev/null &
h1=$!
# Bounded waits: a regression fails the check instead of hanging the suite.
wait_for() { local n=0; until "$@"; do n=$((n + 1)); [ "$n" -ge 200 ] && return 1; sleep 0.05; done; }
slots_held() { ! flock -n "$slots/slot-0.lock" true && ! flock -n "$slots/slot-1.lock" true; }
check "both slots are held by the test holders" wait_for slots_held
rm -f "$FAKE_CARGO_LOG"
(cd "$tmp/wt-one" && "$iso" check >/dev/null 2>"$tmp/wait-err") &
waiter=$!
check "with both slots taken the build says it waits" wait_for grep -q 'already running' "$tmp/wait-err"
check "with both slots taken the build waits" test ! -e "$FAKE_CARGO_LOG"
(cd "$tmp/wt-one" && EMUWIZ_CARGO_MAX_BUILDS=0 "$iso" check >/dev/null 2>&1); rc=$?
check "EMUWIZ_CARGO_MAX_BUILDS=0 removes the limit" test "$rc/$(logged slot)" = "0/<unset>"
(cd "$tmp/wt-one" && EMUWIZ_CARGO_SLOT=1 "$iso" check >/dev/null 2>&1); rc=$?
check "a nested cargo-iso inside a held slot does not wait" test "$rc/$(logged slot)" = "0/1"
(cd "$tmp/wt-one" && "$iso" --rust-analyzer check >/dev/null 2>&1); rc=$?
check "--rust-analyzer takes no slot" test "$rc/$(logged slot)" = "0/<unset>"
rm -f "$FAKE_CARGO_LOG"
echo >"$tmp/release-1"
wait "$h1"
rc=124
if wait_for bash -c "! kill -0 $waiter 2>/dev/null"; then wait "$waiter"; rc=$?; else kill "$waiter" 2>/dev/null; fi
check "the waiting build starts once a slot frees" test "$rc/$(logged slot)" = "0/1"
check "it says when it starts" grep -q 'slot free' "$tmp/wait-err"
echo >"$tmp/release-0"
wait "$h0"
(cd "$tmp/wt-one" && EMUWIZ_CARGO_MAX_BUILDS=two "$iso" check >/dev/null 2>&1); rc=$?
check "a malformed EMUWIZ_CARGO_MAX_BUILDS is refused (2)" test "$rc" = 2
(cd "$tmp/wt-one" && EMUWIZ_CARGO_MAX_BUILDS=1 "$iso" check >/dev/null 2>&1)
check "a limit of 1 uses slot 0 only" test "$(logged slot)" = 0
mv "$slots/slot-0.lock" "$tmp/slot-0.saved" && mkdir "$slots/slot-0.lock"
rm -f "$FAKE_CARGO_LOG"
(cd "$tmp/wt-one" && "$iso" check >/dev/null 2>"$tmp/err"); rc=$?
check "an unopenable slot file never blocks the build" test "$rc" = 0
check "it builds without a slot" test "$(logged slot)" = "<unset>"
check "it says the limit is unavailable" grep -q 'build-slot limit unavailable' "$tmp/err"
rmdir "$slots/slot-0.lock" && mv "$tmp/slot-0.saved" "$slots/slot-0.lock"

# ---- never deletes
mkdir -p "$t1/debug" && : >"$t1/debug/precious-artifact"
(cd "$tmp/wt-one" && "$iso" check >/dev/null 2>&1)
check "existing build artefacts are never removed" test -e "$t1/debug/precious-artifact"
check "the script contains no delete or clean command" bash -c "! grep -E '(^|[^a-z_])(rm |rmdir|unlink|find .*-delete|cargo clean)' '$iso'"

echo
echo "cargo-iso tests: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
