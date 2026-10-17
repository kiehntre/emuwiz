#!/bin/sh
# Shell-level tests for install.sh.
#
# Exercises install.sh entirely inside temporary directories with HOME
# overridden to a scratch path - it never reads or writes the real
# $HOME. Run it directly:
#   sh tests/test_install.sh
# Exits 0 if every test passes, 1 if any test fails.
set -eu
suite_started_seconds=$(date +%s)

# Never inherit the caller's TTY on stdin. install.sh's fresh-install first-
# source prompt is deliberately offered only when `[ -t 0 ]` (stdin is a
# terminal) - see its own comment above that check. Every install.sh
# invocation below is automated and expects no interactive input; run from
# a real terminal without this, `[ -t 0 ]` would be true for those
# invocations (this script's own stdin, inherited by every command and
# command substitution it runs), and a fresh-config install would open
# /dev/tty and block waiting for input that never arrives. Redirecting once,
# here, covers every current and future install.sh call in this file - the
# same effect `assert_success`/`assert_failure` already get for stdout/
# stderr via their own `>/dev/null 2>&1`, just for stdin, and just once
# instead of at each call site. This never affects install.sh's own
# production behavior (`[ -t 0 ]` is still checked honestly - it now
# genuinely reads "no" for these automated runs) and never affects a real
# interactive install run outside this test suite. CI already runs with no
# TTY on stdin, which is exactly why this bug never reproduced there.
exec </dev/null

# Never inherit the caller's configuration/data location overrides. install.sh
# honours EMUWIZ_CONFIG_HOME, XDG_CONFIG_HOME and XDG_DATA_HOME (correctly), so
# a runner or developer shell that exports one would redirect every install
# below away from the scratch $HOME these tests inspect, and would let the
# installer read or write the caller's real configuration. Tests that need an
# override set it explicitly for that one invocation with `env`.
unset EMUWIZ_CONFIG_HOME XDG_CONFIG_HOME XDG_DATA_HOME
# Test seam for tests/test_install_environment_isolation.sh.
if [ "${EMUWIZ_INSTALL_TEST_ENV_PROBE:-}" = 1 ]; then
    printf 'EMUWIZ_CONFIG_HOME=%s\nXDG_CONFIG_HOME=%s\nXDG_DATA_HOME=%s\n' \
        "${EMUWIZ_CONFIG_HOME+set}" "${XDG_CONFIG_HOME+set}" "${XDG_DATA_HOME+set}"
    exit 0
fi

script_dir=$(dirname -- "$0")
script_dir=$(CDPATH= cd -- "$script_dir" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
install_sh="$repo_root/install.sh"
config_example="$repo_root/config.toml.example"
branding_assets="$repo_root/assets/branding"
desktop_assets="$repo_root/assets/linux"

[ -f "$install_sh" ] || { printf 'test_install.sh: cannot find %s\n' "$install_sh" >&2; exit 1; }
[ -f "$config_example" ] || { printf 'test_install.sh: cannot find %s\n' "$config_example" >&2; exit 1; }

# fake_ratarmount_dir - a scratch directory on PATH holding a no-op
# `ratarmount` executable, prepended to PATH only for the handful of tests
# below that assert stderr is completely empty. Those tests are about
# ownership/foreign-collision warnings, not about whether ratarmount
# happens to be installed on whatever machine runs this suite - without
# this, they fail in CI (no ratarmount on PATH there) even though nothing
# about ownership behavior is wrong. This does not suppress or mask
# install.sh's real ratarmount warning: it makes ratarmount genuinely
# "found" for that one invocation, exactly like a real install would see
# on a machine that has it, which is exactly the "warns when ratarmount is
# not on PATH" test elsewhere in this file already covers from the other
# direction.
fake_ratarmount_dir=$(mktemp -d)
trap 'rm -rf -- "$fake_ratarmount_dir"' EXIT
printf '#!/bin/sh\nexit 0\n' >"$fake_ratarmount_dir/ratarmount"
chmod +x -- "$fake_ratarmount_dir/ratarmount"

pass_count=0
fail_count=0

# strip_dev_warning - drops only the deliberate developer-mode warning line,
# so "no warnings" assertions still catch every other stderr output.
strip_dev_warning() {
    grep -v '^install.sh: WARNING: DEVELOPER UNCHECKED INSTALL\. ' || true
}

ok() {
    pass_count=$((pass_count + 1))
    printf 'ok - %s\n' "$*"
}

bad() {
    fail_count=$((fail_count + 1))
    printf 'NOT OK - %s\n' "$*"
}

# assert_success DESCRIPTION CMD... : runs CMD, records ok/bad. Safe under
# `set -e` because the failing command only ever appears in an `if`
# condition, which POSIX exempts from triggering -e.
assert_success() {
    description="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        ok "$description"
    else
        bad "$description (expected success, command failed)"
    fi
}

# assert_failure DESCRIPTION CMD... : runs CMD, expects a nonzero exit.
assert_failure() {
    description="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        bad "$description (expected failure, command succeeded)"
    else
        ok "$description"
    fi
}

assert_file_exists() {
    if [ -f "$2" ]; then ok "$1"; else bad "$1 (missing: $2)"; fi
}

assert_no_such_path() {
    if [ -e "$2" ] || [ -L "$2" ]; then bad "$1 (still present: $2)"; else ok "$1"; fi
}

assert_executable() {
    if [ -x "$2" ]; then ok "$1"; else bad "$1 (not executable: $2)"; fi
}

assert_files_equal() {
    if cmp -s -- "$2" "$3"; then ok "$1"; else bad "$1 ($2 differs from $3)"; fi
}

assert_contains() {
    case "$2" in
        *"$3"*) ok "$1" ;;
        *) bad "$1 (did not find: $3)" ;;
    esac
}

# decode_desktop_exec EXEC_LINE - decodes an "Exec=..." Desktop Entry line
# and prints the resulting literal string. Mirrors the two-layer escaping a
# real consumer (e.g. desktop-file-utils' handle_exec_key()) applies: first
# the format's generic string-value unescape (only "\\" -> "\" matters
# here), THEN the Exec-specific quoted-argument unescape (\", \`, \$, \\,
# and %% -> %). Used to prove the value install.sh writes round-trips back
# to the exact original path, not just that it looks plausible.
decode_desktop_exec() {
    python3 - "$1" <<'PY'
import sys

line = sys.argv[1]
assert line.startswith('Exec=')
raw = line[len("Exec=") :]

# Layer 1: generic Desktop Entry string-value unescape.
layer1 = []
i = 0
while i < len(raw):
    if raw[i] == "\\" and i + 1 < len(raw) and raw[i + 1] == "\\":
        layer1.append("\\")
        i += 2
        continue
    layer1.append(raw[i])
    i += 1
layer1 = "".join(layer1)

assert layer1.startswith('"') and layer1.endswith('"'), layer1
inner = layer1[1:-1]

# Layer 2: Exec quoted-argument unescape.
out = []
i = 0
while i < len(inner):
    char = inner[i]
    if char == "\\" and i + 1 < len(inner) and inner[i + 1] in '"`$\\':
        out.append(inner[i + 1])
        i += 2
        continue
    if char == "%" and i + 1 < len(inner) and inner[i + 1] == "%":
        out.append("%")
        i += 2
        continue
    out.append(char)
    i += 1
sys.stdout.write("".join(out))
PY
}

# make_bundle DIR - populates DIR with a fake extracted release bundle:
# install.sh, stub emuwiz-cli/emuwiz, config.toml.example.
make_bundle() {
    mkdir -p -- "$1"
    cp -- "$install_sh" "$1/install.sh"
    printf '#!/bin/sh\necho fake-cli\n' >"$1/emuwiz-cli"
    printf '#!/bin/sh\necho fake-gui\n' >"$1/emuwiz"
    chmod +x -- "$1/emuwiz-cli" "$1/emuwiz"
    cp -- "$config_example" "$1/config.toml.example"
    mkdir -p -- "$1/assets"
    cp -R -- "$branding_assets" "$1/assets/branding"
    cp -R -- "$desktop_assets" "$1/assets/linux"
}

# make_canonical_bundle DIR - mirrors the packaged archive's bin/ layout.
make_canonical_bundle() {
    mkdir -p -- "$1/bin"
    cp -- "$install_sh" "$1/install.sh"
    printf '#!/bin/sh\necho canonical-cli\n' >"$1/bin/emuwiz-cli"
    printf '#!/bin/sh\necho canonical-gui\n' >"$1/bin/emuwiz"
    chmod +x -- "$1/bin/emuwiz-cli" "$1/bin/emuwiz" "$1/install.sh"
    cp -- "$config_example" "$1/config.toml.example"
    mkdir -p -- "$1/assets"
    cp -R -- "$branding_assets" "$1/assets/branding"
    cp -R -- "$desktop_assets" "$1/assets/linux"
}

# make_workspace DIR - populates DIR with a fake workspace checkout:
# install.sh at the root, stub binaries under target/release/,
# config.toml.example at the root.
make_workspace() {
    mkdir -p -- "$1/target/release"
    cp -- "$install_sh" "$1/install.sh"
    printf '#!/bin/sh\necho ws-cli\n' >"$1/target/release/emuwiz-cli"
    printf '#!/bin/sh\necho ws-gui\n' >"$1/target/release/emuwiz"
    chmod +x -- "$1/target/release/emuwiz-cli" "$1/target/release/emuwiz"
    cp -- "$config_example" "$1/config.toml.example"
    mkdir -p -- "$1/assets"
    cp -R -- "$branding_assets" "$1/assets/branding"
    cp -R -- "$desktop_assets" "$1/assets/linux"
}

echo "=== test: --help exits 0 and prints usage ==="
help_out=$(sh "$install_sh" --help)
assert_success "--help exits 0" sh "$install_sh" --help
assert_contains "--help mentions --uninstall" "$help_out" "--uninstall"

echo "=== test: fresh install from a release bundle ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

assert_success "install from bundle succeeds" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
assert_executable "emuwiz-cli installed and executable" "$bin_dir/emuwiz-cli"
assert_executable "archivefs-cli legacy alias installed" "$bin_dir/archivefs-cli"
assert_executable "emuwiz installed and executable" "$bin_dir/emuwiz"
assert_executable "emuwiz-gui alias installed" "$bin_dir/emuwiz-gui"
assert_executable "archivefs-gui legacy alias installed" "$bin_dir/archivefs-gui"
desktop_file="$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
assert_file_exists "desktop entry installed" "$desktop_file"
desktop_content=$(cat "$desktop_file")
assert_contains "desktop entry names the stable icon" "$desktop_content" \
    "Icon=io.github.kiehntre.emuwiz"
assert_contains "desktop entry uses the absolute canonical binary" "$desktop_content" \
    "Exec=\"$bin_dir/emuwiz\""
for size in 32 64 128 256 512; do
    installed_icon="$home/.local/share/icons/hicolor/${size}x${size}/apps/io.github.kiehntre.emuwiz.png"
    assert_files_equal "$size pixel application icon is the approved asset" \
        "$branding_assets/emuwiz-logo-$size.png" "$installed_icon"
done
if command -v desktop-file-validate >/dev/null 2>&1; then
    assert_success "installed desktop entry passes desktop-file-validate" \
        desktop-file-validate "$desktop_file"
else
    printf 'SKIP - desktop-file-validate is unavailable\n'
fi

assert_file_exists "config.toml created" "$home/.config/emuwiz/config.toml"
assert_files_equal "config.toml matches config.toml.example" \
    "$config_example" "$home/.config/emuwiz/config.toml"
cli_out=$("$bin_dir/emuwiz-cli")
assert_contains "installed emuwiz-cli runs (bundle stub)" "$cli_out" "fake-cli"
rm -rf -- "$work"

echo "=== test: fresh install from the canonical bin-layout release tree ==="
work=$(mktemp -d)
make_canonical_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"
assert_success "install from canonical extracted tree succeeds" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
assert_executable "canonical emuwiz-cli installed" "$bin_dir/emuwiz-cli"
assert_executable "canonical emuwiz installed" "$bin_dir/emuwiz"
canonical_cli_out=$("$bin_dir/emuwiz-cli")
assert_contains "canonical emuwiz-cli runs" "$canonical_cli_out" "canonical-cli"
rm -rf -- "$work"

echo "=== test: this suite does not block on stdin when launched from a real controlling terminal ==="
# Regression coverage for the bug this fix addresses. Without the top-level
# `exec </dev/null` above, running this suite from a real terminal (not "no
# TTY at all" - CI already covered that, and it would pass even with the
# bug present, since an absent TTY was never the distinguishing case) would
# inherit that terminal on fd 0, and the first fresh install above would
# hit install.sh's `[ -t 0 ]` prompt check, open /dev/tty, and block
# forever waiting for input nothing supplies.
#
# This manufactures a real controlling terminal (a pty, via `script`) and
# re-invokes this very script under it, completely unmodified - exactly
# what a developer running `sh tests/test_install.sh` from their own shell
# does - so the fix is proven at its actual entry point, not inferred from
# a synthetic stand-in for it. Bounded by `timeout` so a regression here
# fails loudly rather than hanging this run too.
#
# Guarded against recursing more than one level deep: the re-invoked copy
# below is this same script, which reaches this same block again. Without
# a guard, that copy would spawn its own pty and re-invoke a third copy,
# and so on - each level adding real `script`/process-spawn overhead on
# top of the ~249 already-run tests above it, until an outer `timeout`
# eventually fires and fails the whole chain even though the fix itself
# is fine. One level of real-TTY re-invocation is already a faithful,
# sufficient proof of the fix; it does not need to recurse further.
if [ "${TEST_INSTALL_TTY_REGRESSION_GUARD:-}" = 1 ]; then
    printf 'SKIP - already running under the controlling-terminal regression re-invocation; not recursing further\n'
elif command -v script >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
    # Keep all inner assertions. Allow 30 minutes of progressing loaded work,
    # but stop a run with no output for two minutes and report that separately.
    # Preserve the complete inner log and its actual exit/duration.
    pty_work=$(mktemp -d)
    cat >"$pty_work/watchdog.py" <<'PTY_WATCHDOG'
import os
from pathlib import Path
import selectors
import shlex
import signal
import subprocess
import sys
import time
import unittest


def run_pty(command, log, idle=120, total=1800):
    """A finite overall bound plus an independent no-output progress bound."""
    started = progress = time.monotonic()
    outcome = "completed"
    process = subprocess.Popen(["timeout", "--kill-after=5", str(total), "script", "-qec", command,
                                "/dev/null"], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, start_new_session=True)
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    with Path(log).open("wb") as output:
        while process.poll() is None:
            for key, _ in selector.select(0.5):
                data = os.read(key.fd, 65536)
                if data:
                    output.write(data)
                    output.flush()
                    progress = time.monotonic()
                else:
                    selector.unregister(key.fileobj)
            if outcome == "completed" and time.monotonic() - progress >= idle:
                outcome = "no-progress timeout"
                # GNU timeout forwards TERM and enforces --kill-after for this
                # early bound as well. Always wait for its final exit status.
                try:
                    process.send_signal(signal.SIGTERM)
                except ProcessLookupError:
                    pass
        rest, _ = process.communicate(timeout=10)
        output.write(rest)
    selector.close()
    elapsed = time.monotonic() - started
    if outcome == "completed" and process.returncode == 124:
        outcome = "overall timeout (slow progress exhausted the finite budget)"
    status = 124 if outcome != "completed" else process.returncode
    print("PTY inner: exit=%s process_exit=%s elapsed=%.3fs outcome=%s idle_limit=%ss total_limit=%ss log=%s" %
          (status, process.returncode, elapsed, outcome, idle, total, log), flush=True)
    if status:
        print(Path(log).read_text(errors="replace"), end="", flush=True)
    return status, outcome, elapsed


class WatchdogTests(unittest.TestCase):
    def command(self, code):
        return shlex.join([sys.executable, "-u", "-c", code])

    def run_case(self, code, idle, total, name):
        return run_pty(self.command(code), str(Path(sys.argv[0]).parent / (name + ".log")), idle, total)

    def assert_child_gone(self, name):
        lines = (Path(sys.argv[0]).parent / (name + ".log")).read_text().splitlines()
        pid = int(next(line for line in lines if line.startswith("child_pid=")).split("=", 1)[1])
        deadline = time.monotonic() + 5
        while True:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                return
            if time.monotonic() >= deadline:
                self.fail("watchdog left its PTY child alive: " + str(pid))
            time.sleep(0.05)

    def test_slow_progress_outlives_idle_budget(self):
        result, outcome, elapsed = self.run_case(
            "import time; [(print('progress', flush=True), time.sleep(1)) for _ in range(5)]", 3, 15, "progress")
        self.assertEqual(result, 0)
        self.assertEqual(outcome, "completed")
        self.assertGreater(elapsed, 3)

    def test_no_progress_has_a_distinct_failure(self):
        result, outcome, elapsed = self.run_case("import os,time; print('child_pid='+str(os.getpid()), flush=True); time.sleep(30)", 2, 15, "idle")
        self.assertEqual(result, 124)
        self.assertEqual(outcome, "no-progress timeout")
        self.assertLess(elapsed, 15)
        self.assert_child_gone("idle")

    def test_continuing_progress_still_has_a_finite_bound(self):
        result, outcome, elapsed = self.run_case(
            "import os,time; print('child_pid='+str(os.getpid()), flush=True); [(print('progress', flush=True), time.sleep(1)) for _ in range(30)]", 3, 4, "total")
        self.assertEqual(result, 124)
        self.assertIn("overall timeout", outcome)
        self.assertLess(elapsed, 15)
        self.assert_child_gone("total")

    def test_functional_exit_is_preserved(self):
        result, outcome, _ = self.run_case("raise SystemExit(9)", 3, 15, "functional")
        self.assertEqual(result, 9)
        self.assertEqual(outcome, "completed")


if sys.argv[1] == "self-test":
    unittest.main(argv=[sys.argv[0]], verbosity=2)
else:
    command = "sh " + shlex.quote(str(Path(sys.argv[1]) / "tests/test_install.sh"))
    sys.exit(run_pty(command, sys.argv[2])[0])
PTY_WATCHDOG
    assert_success "PTY watchdog distinguishes progress, hangs, total bound and functional errors" \
        python3 -B "$pty_work/watchdog.py" self-test
    if TEST_INSTALL_TTY_REGRESSION_GUARD=1 python3 -B "$pty_work/watchdog.py" "$repo_root" "$pty_work/inner.log"; then
        ok "the full suite completes under a real controlling terminal (does not block on /dev/tty)"
    else
        bad "the full suite completes under a real controlling terminal (see inner exit, duration and log above)"
    fi
else
    printf 'SKIP - script and/or timeout unavailable; cannot prove the controlling-terminal case directly\n'
fi

echo "=== test: custom XDG data home and a prefix containing spaces ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
data_home="$work/custom data"
bin_dir="$work/prefix with spaces/bin"
mkdir -p -- "$home"

assert_success "install with custom XDG data home and spaced prefix succeeds" \
    env HOME="$home" XDG_DATA_HOME="$data_home" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
desktop_file="$data_home/applications/io.github.kiehntre.emuwiz.desktop"
desktop_content=$(cat "$desktop_file")
assert_contains "spaced absolute executable is quoted in desktop entry" "$desktop_content" \
    "Exec=\"$bin_dir/emuwiz\""
assert_file_exists "custom XDG data home receives icons" \
    "$data_home/icons/hicolor/512x512/apps/io.github.kiehntre.emuwiz.png"
assert_no_such_path "default data home is not used when XDG_DATA_HOME is set" \
    "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"

cp -- "$desktop_file" "$work/desktop.before"
cp -- "$data_home/icons/hicolor/256x256/apps/io.github.kiehntre.emuwiz.png" \
    "$work/icon.before"
assert_success "reinstall is idempotent" \
    env HOME="$home" XDG_DATA_HOME="$data_home" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
assert_files_equal "reinstall leaves desktop content stable" "$work/desktop.before" "$desktop_file"
assert_files_equal "reinstall leaves icon content stable" "$work/icon.before" \
    "$data_home/icons/hicolor/256x256/apps/io.github.kiehntre.emuwiz.png"
rm -rf -- "$work"

echo "=== test: Exec value round-trips a prefix with shell-special characters ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
weird_name='weird $dollar `backtick` "quote" back\slash %percent'
bin_dir="$work/$weird_name"

assert_success "install with a shell-special-character prefix succeeds" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
desktop_file="$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
exec_line=$(grep '^Exec=' "$desktop_file")
decoded=$(decode_desktop_exec "$exec_line")
if [ "$decoded" = "$bin_dir/emuwiz" ]; then
    ok "Exec value decodes back to the exact original path"
else
    bad "Exec value decodes back to the exact original path (got: $decoded)"
fi
if command -v desktop-file-validate >/dev/null 2>&1; then
    assert_success "desktop entry with special-character Exec passes desktop-file-validate" \
        desktop-file-validate "$desktop_file"
else
    printf 'SKIP - desktop-file-validate is unavailable\n'
fi
rm -rf -- "$work"

echo "=== test: an install prefix containing a line break is rejected ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
newline=$(printf 'a\nb')
bin_dir="$work/pre${newline}post"
mkdir -p -- "$bin_dir"

assert_failure "install rejects a prefix containing a line break" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
assert_no_such_path "no desktop entry was written for the rejected install" \
    "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
rm -rf -- "$work"

echo "=== test: a relative XDG_DATA_HOME falls back to ~/.local/share ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

warn=$(env HOME="$home" XDG_DATA_HOME="relative/data" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null)
assert_contains "relative XDG_DATA_HOME triggers a warning" "$warn" \
    "ignoring relative XDG_DATA_HOME"
assert_file_exists "desktop entry falls back to the default data home" \
    "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
rm -rf -- "$work"

echo "=== test: a pre-existing symlink at a desktop/icon destination is foreign ==="
# A symlink at a path this installer owns is never written through *and*,
# under the ownership model, is never silently replaced either - it is
# exactly the "same-name symlink pointing somewhere unrelated" case the
# audit named. Default behavior: left alone, warned about. --replace-foreign:
# the symlink itself is moved aside (never followed) and a real file
# installed in its place. The canary the symlink points at must survive
# either way.
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share/applications" \
    "$home/.local/share/icons/hicolor/256x256/apps"
bin_dir="$work/bin"

canary="$work/canary"
printf 'canary contents\n' >"$canary"
ln -s -- "$canary" "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
ln -s -- "$canary" \
    "$home/.local/share/icons/hicolor/256x256/apps/io.github.kiehntre.emuwiz.png"

assert_failure "install without --replace-foreign exits non-zero (something was left foreign)" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
canary_content=$(cat "$canary")
if [ "$canary_content" = "canary contents" ]; then
    ok "symlink target was never written through"
else
    bad "symlink target was never written through (canary was modified)"
fi
if [ -L "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop" ]; then
    ok "desktop entry symlink was left in place without --replace-foreign"
else
    bad "desktop entry symlink was left in place without --replace-foreign"
fi
if [ -L "$home/.local/share/icons/hicolor/256x256/apps/io.github.kiehntre.emuwiz.png" ]; then
    ok "icon symlink was left in place without --replace-foreign"
else
    bad "icon symlink was left in place without --replace-foreign"
fi

assert_success "install --replace-foreign succeeds over symlink destinations" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign
canary_content=$(cat "$canary")
if [ "$canary_content" = "canary contents" ]; then
    ok "symlink target still untouched after --replace-foreign"
else
    bad "symlink target still untouched after --replace-foreign (canary was modified)"
fi
if [ -L "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop" ]; then
    bad "desktop entry symlink was replaced with a regular file after --replace-foreign"
else
    ok "desktop entry symlink was replaced with a regular file after --replace-foreign"
fi
rm -rf -- "$work"

echo "=== test: an existing legacy config directory is reused ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.config/archivefs"
marker="LEGACY CONFIG - KEEP USING"
printf '%s\n' "$marker" >"$home/.config/archivefs/config.toml"
bin_dir="$work/bin"

assert_success "install with only a legacy config succeeds" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
kept=$(cat "$home/.config/archivefs/config.toml")
assert_contains "legacy config content is preserved" "$kept" "$marker"
assert_no_such_path "no EmuWiz config is created beside a legacy profile" \
    "$home/.config/emuwiz/config.toml"
rm -rf -- "$work"

echo "=== test: EmuWiz config takes precedence when both directories exist ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.config/emuwiz" "$home/.config/archivefs"
primary_marker="PRIMARY CONFIG - KEEP USING"
legacy_marker="LEGACY CONFIG - DO NOT SELECT"
printf '%s\n' "$primary_marker" >"$home/.config/emuwiz/config.toml"
printf '%s\n' "$legacy_marker" >"$home/.config/archivefs/config.toml"
bin_dir="$work/bin"

assert_success "install with both config directories succeeds" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
kept=$(cat "$home/.config/emuwiz/config.toml")
assert_contains "EmuWiz config wins and is preserved" "$kept" "$primary_marker"
legacy_kept=$(cat "$home/.config/archivefs/config.toml")
assert_contains "unselected legacy config is untouched" "$legacy_kept" "$legacy_marker"
rm -rf -- "$work"

echo "=== test: install from a workspace checkout (target/release) ==="
work=$(mktemp -d)
make_workspace "$work/workspace"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

assert_success "install from workspace succeeds" \
    env HOME="$home" sh "$work/workspace/install.sh" --developer-unchecked-install --prefix "$bin_dir"
cli_out=$("$bin_dir/emuwiz-cli")
assert_contains "installed emuwiz-cli runs (workspace stub)" "$cli_out" "ws-cli"
rm -rf -- "$work"

echo "=== test: an existing config is never overwritten ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
marker="USER EDITED - DO NOT CLOBBER"
printf '%s\n' "$marker" >"$home/.config/emuwiz/config.toml"

assert_success "reinstall over an existing config succeeds" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
kept=$(cat "$home/.config/emuwiz/config.toml")
assert_contains "existing config content is preserved" "$kept" "$marker"
rm -rf -- "$work"

echo "=== test: starter config is no-clobber (dangling symlink, custom roots, races) ==="
# Fresh scratch HOME per case; install.sh never sees the real one.
cfg_case() {
    cfg_work=$(mktemp -d)
    make_bundle "$cfg_work/bundle"
    cfg_home="$cfg_work/home"
    mkdir -p -- "$cfg_home"
    cfg_bin="$cfg_work/bin"
}

# Dangling config.toml symlink: left alone, nothing created at its target.
cfg_case
mkdir -p -- "$cfg_home/.config/emuwiz"
ln -s -- "$cfg_work/elsewhere/config.toml" "$cfg_home/.config/emuwiz/config.toml"
assert_success "dangling config.toml symlink: install succeeds" \
    env HOME="$cfg_home" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin"
assert_no_such_path "dangling symlink target was not created" "$cfg_work/elsewhere"
if [ -L "$cfg_home/.config/emuwiz/config.toml" ] \
    && [ "$(readlink "$cfg_home/.config/emuwiz/config.toml")" = "$cfg_work/elsewhere/config.toml" ]; then
    ok "dangling symlink itself is untouched"
else
    bad "dangling symlink itself is untouched"
fi
rm -rf -- "$cfg_work"

# Existing config with an unusual byte pattern is preserved byte for byte.
cfg_case
mkdir -p -- "$cfg_home/.config/emuwiz"
printf 'keep\0me\n\377' >"$cfg_home/.config/emuwiz/config.toml"
cp -- "$cfg_home/.config/emuwiz/config.toml" "$cfg_work/before"
env HOME="$cfg_home" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" >/dev/null 2>&1
assert_files_equal "existing config bytes preserved" "$cfg_work/before" "$cfg_home/.config/emuwiz/config.toml"
# --replace-foreign concerns installed assets only; it never authorises
# replacing a user's configuration, including a dangling symlink.
env HOME="$cfg_home" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" --replace-foreign >/dev/null 2>&1 || true
assert_files_equal "--replace-foreign preserves existing config bytes" "$cfg_work/before" "$cfg_home/.config/emuwiz/config.toml"
if ls -A "$cfg_home/.config/emuwiz" | grep -q 'stage'; then
    bad "no staging file is left behind"
else
    ok "no staging file is left behind"
fi
rm -rf -- "$cfg_work"

# Destination appears between the existence check and publication: the
# publisher must lose the race without writing through or replacing it.
cfg_case
mkdir -p -- "$cfg_home/.config/emuwiz"
cfg_probe="$cfg_work/probe.sh"
{
    # Source only the helper, then pre-create the destination as the racing
    # writer would after the installer's own existence check.
    sed -n '/^starter_config_result=""$/,/^}$/p' "$install_sh"
    printf 'dest=%s\n' "'$cfg_home/.config/emuwiz/config.toml'"
    printf 'printf "RACER\\n" >"$dest"\n'
    printf 'publish_starter_config %s %s "$dest"; printf "%%s\\n" "$starter_config_result"\n' \
        "'$config_example'" "'$cfg_home/.config/emuwiz'"
} >"$cfg_probe"
race_out=$(sh "$cfg_probe" 2>&1) || true
assert_contains "race: publisher reports the destination exists" "$race_out" "exists"
assert_contains "race: racing writer's bytes are intact" "$(cat "$cfg_home/.config/emuwiz/config.toml")" "RACER"
rm -rf -- "$cfg_work"

# Same race, destination is a dangling symlink created by the racer.
cfg_case
mkdir -p -- "$cfg_home/.config/emuwiz"
ln -s -- "$cfg_work/nowhere" "$cfg_home/.config/emuwiz/config.toml"
{
    sed -n '/^starter_config_result=""$/,/^}$/p' "$install_sh"
    printf 'publish_starter_config %s %s %s; printf "%%s\\n" "$starter_config_result"\n' \
        "'$config_example'" "'$cfg_home/.config/emuwiz'" "'$cfg_home/.config/emuwiz/config.toml'"
} >"$cfg_work/probe.sh"
race_out=$(sh "$cfg_work/probe.sh" 2>&1) || true
assert_contains "dangling symlink at publication: reported as existing" "$race_out" "exists"
assert_no_such_path "dangling symlink at publication: nothing written through it" "$cfg_work/nowhere"
rm -rf -- "$cfg_work"

# Anonymous staging regressions. Instrument private helper copies only: there
# are no production environment hooks. Assert inode/type/content preservation,
# not merely successful exit. Replaces named-stage seams with the corresponding
# acquisition/write/publication/signal seams of the anonymous implementation.
if python3 -B - "$install_sh" "$config_example" <<'STAGING_TESTS'
import errno
import os
from pathlib import Path
import shlex
import signal
import shutil
import socket
import stat
import subprocess
import tempfile
import unittest

installer = Path(os.sys.argv[1]).read_text()
example = Path(os.sys.argv[2])
helper = 'starter_config_result=""\n' + installer.split('starter_config_result=""\n', 1)[1].split('\n}\n', 1)[0] + '\n}\n'

class AnonymousStagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="emuwiz-anonymous-stage-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.directory = self.root / "config"
        self.directory.mkdir()
        self.destination = self.directory / "config.toml"
        self.foreign = self.root / "foreign"
        self.foreign.write_bytes(b"FOREIGN\x00\xff\n")
        self.original = self.identity(self.foreign)

    def identity(self, path):
        info = path.lstat()
        value = os.readlink(path) if stat.S_ISLNK(info.st_mode) else path.read_bytes() if stat.S_ISREG(info.st_mode) else None
        return (info.st_dev, info.st_ino, info.st_mode, value)

    def run_helper(self, marker=None, mutation=None, shell="dash", inherited=False, shell_mutation=None, text=None, umask=None):
        content = helper if text is None else text
        if marker:
            self.assertEqual(content.count(marker), 1)
            content = content.replace(marker, marker + "\n" + mutation)
        if shell_mutation:
            marker = "    # ACQUIRE-WINDOW: no staging descriptor has been acquired."
            self.assertEqual(content.count(marker), 1)
            content = content.replace(marker, marker + "\n" + shell_mutation)
        script = self.root / "probe.sh"
        script.write_text(content + '\npublish_starter_config ' + ' '.join(shlex.quote(str(p)) for p in (example, self.directory, self.destination)) + '\nrc=$?\nif (: >&6) 2>/dev/null; then printf "FD6-OPEN\\n"; else printf "FD6-CLOSED\\n"; fi\nprintf "result=%s\\n" "$starter_config_result"\nexit "$rc"\n')
        command = [shell, str(script)]
        if inherited:
            command = [shell, "-c", 'exec 6<"$1"; exec "$2" "$3"', "probe", str(self.foreign), shell, str(script)]
        return subprocess.run(command, capture_output=True, text=True, timeout=10,
                              preexec_fn=(lambda: os.umask(umask)) if umask is not None else None)

    def no_stage(self):
        self.assertEqual(list(self.directory.glob(".config.toml.stage.*")), [])

    def unchanged(self):
        self.assertEqual(self.identity(self.foreign), self.original)

    def test_inherited_fd6_never_establishes_ownership(self):
        result = self.run_helper(inherited=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("FD6-CLOSED", result.stdout)
        self.unchanged()
        self.assertEqual(self.destination.read_bytes(), example.read_bytes())
        self.no_stage()

    def test_inherited_fd6_signal_before_acquisition(self):
        for sig in ("INT", "TERM", "HUP"):
            with self.subTest(signal=sig):
                path = self.directory / "foreign-stage"
                result = self.run_helper(inherited=True, shell_mutation='    mv -- ' + shlex.quote(str(self.foreign)) + ' ' + shlex.quote(str(path)) + '\n    kill -' + sig + ' "$$"')
                self.assertEqual(result.returncode, 130, result.stderr)
                self.assertEqual(self.identity(path), self.original)
                self.assertFalse(self.destination.exists())
                path.rename(self.foreign)

    def test_signals_at_every_ownership_transition(self):
        for marker in ("    # ANONYMOUS-OPEN-WINDOW:", "    # ACQUIRED-WINDOW:", "    # STAGE-WINDOW:", "    # PUBLISH-WINDOW:", "        # PUBLISHED-WINDOW:"):
            # Use the whole unique source line as marker.
            line = next(line for line in helper.splitlines() if line.startswith(marker))
            for sig in ("SIGINT", "SIGTERM", "SIGHUP"):
                with self.subTest(marker=marker, signal=sig):
                    mutation = ("        " if "PUBLISHED" in marker else "    ") + 'os.kill(os.getpid(), signal.' + sig + ')'
                    result = self.run_helper(line, mutation, inherited=True)
                    self.assertEqual(result.returncode, 130, result.stderr)
                    self.assertIn("installation interrupted", result.stderr)
                    self.assertIn("published and is complete" if "PUBLISHED" in marker else "nothing was written", result.stderr)
                    self.assertNotIn("nothing was changed", result.stderr)
                    self.unchanged()
                    if "PUBLISHED" in marker:
                        self.assertEqual(self.destination.read_bytes(), example.read_bytes())
                        self.destination.unlink()
                    else:
                        self.assertFalse(self.destination.exists())
                    self.no_stage()

    def test_foreign_objects_never_opened_or_cleaned(self):
        for kind in ("fifo", "socket", "regular", "symlink"):
            with self.subTest(kind=kind):
                stage = self.directory / ".config.toml.stage.foreign"
                sock = None
                if kind == "fifo": os.mkfifo(stage)
                elif kind == "socket":
                    sock = socket.socket(socket.AF_UNIX)
                    # sockaddr_un has a small pathname bound; use a relative
                    # fixture name so a deliberately long TMPDIR is supported.
                    previous_directory = os.getcwd()
                    try:
                        os.chdir(self.directory)
                        sock.bind(stage.name)
                    finally:
                        os.chdir(previous_directory)
                elif kind == "regular": stage.write_bytes(b"FOREIGN-STAGE\x00\xff")
                else: stage.symlink_to(self.foreign)
                before = self.identity(stage)
                try:
                    result = self.run_helper(inherited=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(self.identity(stage), before)
                    self.unchanged()
                    self.assertEqual(self.destination.read_bytes(), example.read_bytes())
                    self.destination.unlink()
                finally:
                    if sock: sock.close()
                    stage.unlink()

    def test_fifo_never_blocks_acquisition(self):
        # No reader holds the FIFO open. A named redirection would hang, so
        # run_helper's ten-second timeout is an independent blocking assertion.
        stage = self.directory / ".config.toml.stage.fifo"
        os.mkfifo(stage)
        before = self.identity(stage)
        result = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.identity(stage), before)

    def test_substitutions_across_failure_and_success_paths(self):
        markers = {"acquisition": "    # ACQUIRED-WINDOW:", "write": "    # STAGE-WINDOW:", "publication": "    # PUBLISH-WINDOW:", "success": "        # PUBLISHED-WINDOW:", "signal": "    # STAGE-WINDOW:"}
        for action, prefix in markers.items():
            for kind in ("regular", "symlink"):
                with self.subTest(action=action, kind=kind):
                    stage = self.directory / ".config.toml.stage.substituted"
                    line = next(line for line in helper.splitlines() if line.startswith(prefix))
                    indent = "        " if action == "success" else "    "
                    mutation = indent + ('Path(' + repr(str(stage)) + ').write_bytes(b"FOREIGN-SUBSTITUTION")' if kind == "regular" else 'os.symlink(' + repr(str(self.foreign)) + ', ' + repr(str(stage)) + ')')
                    mutation = indent + "from pathlib import Path\n" + mutation + '\n' + indent + 'before = os.lstat(' + repr(str(stage)) + ')\n'
                    mutation += indent + 'assert before.st_ino > 0\n' + indent + 'with open(' + repr(str(self.root / "identity")) + ', "w") as record: record.write(str(before.st_ino))'
                    if action in ("acquisition", "write", "publication"):
                        mutation += '\n' + indent + 'raise OSError("synthetic failure")'
                    elif action == "signal":
                        mutation += '\n' + indent + 'os.kill(os.getpid(), signal.SIGTERM)'
                    result = self.run_helper(line, mutation)
                    self.assertEqual(stage.lstat().st_ino, int((self.root / "identity").read_text()))
                    self.assertEqual(stage.read_bytes(), b"FOREIGN-SUBSTITUTION" if kind == "regular" else self.original[3])
                    if kind == "symlink": self.assertEqual(os.readlink(stage), str(self.foreign))
                    self.unchanged()
                    self.assertEqual(result.returncode, 0 if action == "success" else 130 if action == "signal" else 1, result.stderr)
                    if action == "success":
                        self.assertEqual(self.destination.read_bytes(), example.read_bytes())
                        self.destination.unlink()
                    else: self.assertFalse(self.destination.exists())
                    stage.unlink()

    def test_no_unlink_even_if_cleanup_metadata_unavailable(self):
        marker = next(line for line in helper.splitlines() if line.startswith("    # CLOSE-WINDOW:"))
        result = self.run_helper(marker, '    assert "unlink" not in globals()')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.no_stage()

    def test_unproven_descriptor_only_closed(self):
        # A negative control for an unexpected acquisition backend result.
        line = next(line for line in helper.splitlines() if line.startswith("    # ACQUIRED-WINDOW:"))
        result = self.run_helper(line, '    os.close(stage_fd)\n    stage_fd = os.open(' + repr(str(self.foreign)) + ', os.O_RDONLY)')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("ownership could not be established", result.stderr)
        self.unchanged()
        self.assertFalse(self.destination.exists())

    def test_anonymous_file_is_owned_regular_and_has_no_name(self):
        line = next(line for line in helper.splitlines() if line.startswith("    # STAGE-WINDOW:"))
        result = self.run_helper(line, '    assert state == "owned regular"\n    assert stat.S_ISREG(os.fstat(stage_fd).st_mode)\n    assert os.fstat(stage_fd).st_nlink == 0\n    assert os.fstat(stage_fd).st_mode & 0o777 == 0o600\n    assert os.listdir(directory) == []')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.no_stage()

    def test_missing_source_closes_owned_stage(self):
        text = helper.replace('with open(src, "rb") as source:', 'with open(src + ".missing", "rb") as source:')
        result = self.run_helper(text=text)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertFalse(self.destination.exists())
        self.no_stage()

    def test_unsupported_anonymous_creation_fails_closed(self):
        line = next(line for line in helper.splitlines() if line.startswith("    # ANONYMOUS-OPEN-WINDOW:"))
        result = self.run_helper(line, '    raise OSError(errno.EOPNOTSUPP, "synthetic unsupported filesystem")')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertFalse(self.destination.exists())
        self.no_stage()
        self.unchanged()

    def test_unavailable_linkat_fails_closed(self):
        line = next(line for line in helper.splitlines() if line.startswith("    # PUBLISH-WINDOW:"))
        result = self.run_helper(line, '    raise OSError(errno.ENOENT, "synthetic procfs unavailable")')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertFalse(self.destination.exists())
        self.no_stage()

    def test_existing_configuration_objects_preserved(self):
        for kind in ("regular", "symlink", "fifo", "directory"):
            with self.subTest(kind=kind):
                if kind == "regular": self.destination.write_bytes(b"USER CONFIG\x00\xff")
                elif kind == "symlink": self.destination.symlink_to(self.root / "missing")
                elif kind == "fifo": os.mkfifo(self.destination)
                else: self.destination.mkdir()
                before = self.identity(self.destination)
                result = self.run_helper()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("result=exists", result.stdout)
                self.assertEqual(self.identity(self.destination), before)
                if kind == "directory": self.destination.rmdir()
                else: self.destination.unlink()
                self.no_stage()

    def test_late_destination_creation_no_clobber(self):
        line = next(line for line in helper.splitlines() if line.startswith("    # PUBLISH-WINDOW:"))
        result = self.run_helper(line, '    with open(destination, "wb") as racer: racer.write(b"RACER\\x00\\xff")\n    with open(' + repr(str(self.root / "inode")) + ', "w") as record: record.write(str(os.stat(destination).st_ino))')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("result=exists", result.stdout)
        self.assertEqual(self.destination.read_bytes(), b"RACER\x00\xff")
        self.assertEqual(self.destination.stat().st_ino, int((self.root / "inode").read_text()))
        self.no_stage()

    def test_dash_sh_and_bash_wrappers(self):
        for shell in ("dash", "sh", "bash"):
            with self.subTest(shell=shell):
                result = self.run_helper(shell=shell)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.destination.read_bytes(), example.read_bytes())
                self.destination.unlink()
                self.no_stage()

    def test_parent_interruption_reaps_worker(self):
        for prefix in ("    # ACQUIRED-WINDOW:", "        # PUBLISHED-WINDOW:"):
            line = next(line for line in helper.splitlines() if line.startswith(prefix))
            indent = "        " if "PUBLISHED" in prefix else "    "
            record = self.root / "worker-pid"
            mutation = indent + 'with open(' + repr(str(record)) + ', "w") as output: output.write(str(os.getpid()))\n'
            mutation += indent + 'os.kill(os.getppid(), signal.SIGTERM)\n'
            mutation += indent + 'import time; time.sleep(1)'
            result = self.run_helper(line, mutation)
            self.assertEqual(result.returncode, 130, result.stderr)
            self.assertIn("installation interrupted", result.stderr)
            self.assertIn("published and is complete" if "PUBLISHED" in prefix else "nothing was written", result.stderr)
            pid = int(record.read_text())
            with self.assertRaises(ProcessLookupError): os.kill(pid, 0)
            self.unchanged()
            if "PUBLISHED" in prefix:
                self.assertEqual(self.destination.read_bytes(), example.read_bytes())
                self.destination.unlink()
            else: self.assertFalse(self.destination.exists())
            self.no_stage()

    def test_full_install_interruption_and_partial_reporting(self):
        for published in (False, True):
            with self.subTest(published=published):
                bundle = self.root / ("published-bundle" if published else "unpublished-bundle")
                bundle.mkdir()
                shutil.copytree(Path(os.sys.argv[1]).resolve().parent / "assets", bundle / "assets")
                shutil.copyfile(example, bundle / "config.toml.example")
                (bundle / "bin").mkdir()
                for name in ("emuwiz", "emuwiz-cli"):
                    (bundle / "bin" / name).write_text("#!/bin/sh\nexit 0\n")
                prefix = "        # PUBLISHED-WINDOW:" if published else "    # STAGE-WINDOW:"
                line = next(line for line in helper.splitlines() if line.startswith(prefix))
                indent = "        " if published else "    "
                script = installer.replace(line, line + "\n" + indent + "os.kill(os.getpid(), signal.SIGTERM)")
                (bundle / "install.sh").write_text(script)
                home = bundle / "home"
                home.mkdir()
                env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
                           XDG_DATA_HOME=str(home / "data"), XDG_CACHE_HOME=str(home / "cache"),
                           XDG_STATE_HOME=str(home / "state"), EMUWIZ_CONFIG_HOME=str(home / "config/emuwiz"))
                result = subprocess.run(["dash", str(bundle / "install.sh"), "--prefix", str(bundle / "installed"), "--developer-unchecked-install"],
                                        env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 130, result.stderr)
                self.assertIn("installation interrupted", result.stderr)
                self.assertIn("already installed assets remain", result.stderr)
                self.assertIn("published and is complete" if published else "nothing was written", result.stderr)
                self.assertIn("Installed emuwiz-cli and emuwiz", result.stdout)
                self.assertNotIn("Next steps:", result.stdout)
                self.assertTrue((home / "data/emuwiz-installer/manifest").is_file())
                self.assertTrue((bundle / "installed/emuwiz-cli").is_file())
                config = home / "config/emuwiz/config.toml"
                if published: self.assertEqual(config.read_bytes(), example.read_bytes())
                else: self.assertFalse(config.exists())
                self.assertEqual(list(config.parent.glob(".config.toml.stage.*")), [])

    # ---- post-publication signals, truthful reporting, supported environments, permissions ----

    def install_bundle(self, script, shell="dash", name="bundle", umask=None, config_bytes=None, config_mode=None):
        """A complete disposable install of a (possibly mutated) installer copy; returns (result, home, bundle)."""
        bundle = self.root / name
        bundle.mkdir()
        shutil.copytree(Path(os.sys.argv[1]).resolve().parent / "assets", bundle / "assets")
        shutil.copyfile(example, bundle / "config.toml.example")
        (bundle / "bin").mkdir()
        for tool in ("emuwiz", "emuwiz-cli"):
            (bundle / "bin" / tool).write_text("#!/bin/sh\nexit 0\n")
        (bundle / "install.sh").write_text(script)
        home = bundle / "home"
        home.mkdir()
        if config_bytes is not None:
            (home / "config/emuwiz").mkdir(parents=True)
            (home / "config/emuwiz/config.toml").write_bytes(config_bytes)
            (home / "config/emuwiz/config.toml").chmod(config_mode)
        env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
                   XDG_DATA_HOME=str(home / "data"), XDG_CACHE_HOME=str(home / "cache"),
                   XDG_STATE_HOME=str(home / "state"), EMUWIZ_CONFIG_HOME=str(home / "config/emuwiz"))
        result = subprocess.run([shell, str(bundle / "install.sh"), "--prefix", str(bundle / "installed"), "--developer-unchecked-install"],
                                env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60,
                                preexec_fn=(lambda: os.umask(umask)) if umask is not None else None)
        return result, home, bundle

    def mutated_installer(self, prefix, mutation):
        """The real installer with `mutation` inserted after the first worker line starting with `prefix`."""
        start = installer.index("<<'PSC_PY' &\n") + len("<<'PSC_PY' &\n")
        end = installer.index("\nPSC_PY\n", start)
        line = next(l for l in installer[start:end].splitlines() if l.startswith(prefix))
        position = installer.index(line, start)
        self.assertLess(position, end)
        return installer[:position] + line + "\n" + mutation + installer[position + len(line):]

    def assert_published_and_complete(self, inode_record=None):
        """Bytes, inode identity, link count and mode of the published config; no staging leftovers."""
        info = self.destination.lstat()
        self.assertEqual(self.destination.read_bytes(), example.read_bytes())
        self.assertEqual(stat.S_IMODE(info.st_mode), 0o600)
        self.assertEqual(info.st_nlink, 1)
        if inode_record is not None:
            self.assertEqual(inode_record.read_text(), "%d %d" % (info.st_dev, info.st_ino))
        self.no_stage()

    def test_signal_after_publication_is_reported_as_published(self):
        # A signal that lands right after linkat() (and, below, inside the descriptor cleanup)
        # must never be reported as a failure or as "nothing changed".
        windows = {"after publication": ("        # PUBLISHED-WINDOW:", "        "),
                   "start of cleanup": ("    # CLOSE-WINDOW:", "    "),
                   "during descriptor cleanup": ("            # CLOSE-LOOP-WINDOW:", "            ")}
        published_line = next(l for l in helper.splitlines() if l.startswith("        # PUBLISHED-WINDOW:"))
        for label, (marker, indent) in windows.items():
            line = next(l for l in helper.splitlines() if l.startswith(marker))
            for shell in ("dash", "bash"):
                for sig in ("SIGINT", "SIGTERM", "SIGHUP"):
                    with self.subTest(window=label, shell=shell, signal=sig):
                        record = self.root / "inode"
                        pidfile = self.root / "worker-pid"
                        # Record the staged inode's identity and the worker pid while its descriptor is still open.
                        text = helper.replace(published_line, published_line + "\n" +
                                              '        st = os.fstat(stage_fd)\n' +
                                              '        with open(' + repr(str(record)) + ', "w") as out: out.write("%d %d" % (st.st_dev, st.st_ino))\n' +
                                              '        with open(' + repr(str(pidfile)) + ', "w") as out: out.write(str(os.getpid()))', 1)
                        text = text.replace(line, line + "\n" + indent + 'os.kill(os.getpid(), signal.' + sig + ')', 1)
                        result = self.run_helper(text=text, shell=shell)
                        self.assertEqual(result.returncode, 130, result.stderr)
                        self.assertIn("installation interrupted", result.stderr)
                        self.assertIn("published and is complete", result.stderr)
                        for wrong in ("Traceback", "nothing was changed", "if published", "nothing was written"):
                            self.assertNotIn(wrong, result.stderr)
                        self.assert_published_and_complete(record)
                        with self.assertRaises(ProcessLookupError):  # the worker was reaped, not left behind
                            os.kill(int(pidfile.read_text()), 0)
                        self.unchanged()
                        self.destination.unlink()

    def test_signal_in_cleanup_full_install_reports_published(self):
        for label, marker, indent in (("after publication", "        # PUBLISHED-WINDOW:", "        "),
                                      ("start of cleanup", "    # CLOSE-WINDOW:", "    "),
                                      ("during descriptor cleanup", "            # CLOSE-LOOP-WINDOW:", "            ")):
            for shell in ("dash", "bash"):
                with self.subTest(window=label, shell=shell):
                    script = self.mutated_installer(marker, indent + "os.kill(os.getpid(), signal.SIGTERM)")
                    result, home, bundle = self.install_bundle(script, shell, name="cleanup-" + shell + "-" + str(len(label)))
                    self.assertEqual(result.returncode, 130, result.stderr)
                    self.assertIn("published and is complete", result.stderr)
                    self.assertIn("already installed assets remain", result.stderr)
                    for wrong in ("Traceback", "nothing was changed", "could not write a starter config"):
                        self.assertNotIn(wrong, result.stderr)
                    self.assertIn("Installed emuwiz-cli and emuwiz", result.stdout)
                    self.assertNotIn("Next steps:", result.stdout)
                    config = home / "config/emuwiz/config.toml"
                    self.assertEqual(config.read_bytes(), example.read_bytes())
                    self.assertEqual(stat.S_IMODE(config.stat().st_mode), 0o600)
                    self.assertTrue((bundle / "installed/emuwiz-cli").is_file())
                    self.assertTrue((home / "data/emuwiz-installer/manifest").is_file())
                    self.assertEqual(list(config.parent.glob(".config.toml.stage.*")), [])

    def test_signal_to_the_shell_after_publication_is_reported_as_published(self):
        # The installer itself (not just the worker) is signalled once the config is published.
        line = next(l for l in helper.splitlines() if l.startswith("        # PUBLISHED-WINDOW:"))
        record = self.root / "worker-pid"
        mutation = ('        with open(' + repr(str(record)) + ', "w") as out: out.write(str(os.getpid()))\n' +
                    '        os.kill(os.getppid(), signal.SIGTERM)\n        import time; time.sleep(1)')
        for shell in ("dash", "bash"):
            with self.subTest(shell=shell):
                result = self.run_helper(line, mutation, shell=shell)
                self.assertEqual(result.returncode, 130, result.stderr)
                self.assertIn("published and is complete", result.stderr)
                self.assertNotIn("nothing was changed", result.stderr)
                with self.assertRaises(ProcessLookupError):
                    os.kill(int(record.read_text()), 0)
                self.assert_published_and_complete()
                self.destination.unlink()

    def test_interruption_before_publication_says_nothing_was_written(self):
        for marker in ("    # ANONYMOUS-OPEN-WINDOW:", "    # ACQUIRED-WINDOW:", "    # STAGE-WINDOW:", "    # PUBLISH-WINDOW:"):
            line = next(l for l in helper.splitlines() if l.startswith(marker))
            for shell in ("dash", "bash"):
                with self.subTest(marker=marker, shell=shell):
                    result = self.run_helper(line, "    os.kill(os.getpid(), signal.SIGTERM)", shell=shell)
                    self.assertEqual(result.returncode, 130, result.stderr)
                    self.assertIn("before the starter config was published", result.stderr)
                    self.assertIn("nothing was written", result.stderr)
                    self.assertNotIn("published and is complete", result.stderr)
                    self.assertFalse(self.destination.exists())
                    self.no_stage()

    def test_worker_killed_is_unconfirmed_never_nothing_changed(self):
        # SIGKILL proves nothing about the filesystem, so the installer must say so.
        for label, marker, indent, published in (("after publication", "        # PUBLISHED-WINDOW:", "        ", True),
                                                 ("before publication", "    # STAGE-WINDOW:", "    ", False)):
            for shell in ("dash", "bash"):
                with self.subTest(window=label, shell=shell):
                    script = self.mutated_installer(marker, indent + "os.kill(os.getpid(), signal.SIGKILL)")
                    result, home, bundle = self.install_bundle(script, shell, name="killed-" + shell + str(published))
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertIn("ended unexpectedly", result.stderr)
                    self.assertIn("Inspect that path before retrying", result.stderr)
                    self.assertNotIn("nothing was changed", result.stderr)
                    self.assertNotIn("Traceback", result.stderr)
                    self.assertIn("remain in place", result.stderr)
                    self.assertTrue((bundle / "installed/emuwiz-cli").is_file())
                    self.assertEqual((home / "config/emuwiz/config.toml").exists(), published)

    def test_unsupported_environments_refuse_explicitly_without_traceback(self):
        cases = {
            "python without os.O_TMPFILE": ("del os.O_TMPFILE", "this Python has no os.O_TMPFILE"),
            "non-Linux platform": ('sys.platform = "darwin"', "not Linux"),
            "ctypes unavailable": ('sys.modules["ctypes"] = None', "ctypes or linkat() is unavailable"),
            "linkat unavailable": ("import ctypes; ctypes.CDLL = lambda *a, **k: object()", "ctypes or linkat() is unavailable"),
        }
        line = next(l for l in helper.splitlines() if l.startswith("# CAPABILITY-CHECK:"))
        for label, (mutation, expected) in cases.items():
            for shell in ("dash", "bash"):
                with self.subTest(case=label, shell=shell):
                    result = self.run_helper(line, mutation, shell=shell)
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertIn(expected, result.stderr)
                    self.assertIn("Nothing was attempted", result.stderr)
                    self.assertIn("no named staging fallback", result.stderr)
                    self.assertNotIn("Traceback", result.stderr)
                    self.assertIn("result=unsupported environment", result.stdout)
                    self.assertFalse(self.destination.exists())
                    self.assertEqual(list(self.directory.iterdir()), [])
                    self.unchanged()

    def test_unsupported_filesystem_and_procfs_get_specific_explanations(self):
        open_line = next(l for l in helper.splitlines() if l.startswith("    # ANONYMOUS-OPEN-WINDOW:"))
        result = self.run_helper(open_line, '    raise OSError(errno.EOPNOTSUPP, "synthetic")')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("does not support anonymous O_TMPFILE files", result.stderr)
        self.assertIn("No starter config was published", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        publish_line = next(l for l in helper.splitlines() if l.startswith("    # PUBLISH-WINDOW:"))
        result = self.run_helper(publish_line, '    raise OSError(errno.ENOENT, "synthetic")')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("/proc/self/fd is unavailable", result.stderr)
        self.assertIn("is /proc mounted", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_unsupported_environment_full_install_keeps_assets_and_explains(self):
        line = next(l for l in installer.splitlines() if l.startswith("# CAPABILITY-CHECK:"))
        script = installer.replace(line, line + "\ndel os.O_TMPFILE", 1)
        for shell in ("dash", "bash"):
            with self.subTest(shell=shell):
                result, home, bundle = self.install_bundle(script, shell, name="unsupported-" + shell)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("this Python has no os.O_TMPFILE", result.stderr)
                self.assertIn("could not write a starter config", result.stderr)
                self.assertIn("nothing was changed there", result.stderr)
                self.assertIn("remain in place", result.stderr)
                self.assertIn("copy config.toml.example to", result.stderr)
                self.assertNotIn("Traceback", result.stderr)
                self.assertTrue((bundle / "installed/emuwiz-cli").is_file())
                self.assertTrue((home / "data/emuwiz-installer/manifest").is_file())
                self.assertFalse((home / "config/emuwiz/config.toml").exists())

    def test_new_starter_config_is_0600_under_normal_and_restrictive_umasks(self):
        # Intentional policy (previously the mode followed the umask, typically 0644).
        for um in (0o000, 0o002, 0o022, 0o027, 0o077):
            for shell in ("dash", "bash"):
                with self.subTest(umask=oct(um), shell=shell):
                    result = self.run_helper(shell=shell, umask=um)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(stat.S_IMODE(self.destination.stat().st_mode), 0o600)
                    self.destination.unlink()

    def test_existing_config_permissions_and_bytes_are_never_changed(self):
        for mode in (0o644, 0o640, 0o600, 0o400):
            with self.subTest(mode=oct(mode)):
                result, home, bundle = self.install_bundle(installer, "dash", name="existing-%o" % mode,
                                                           config_bytes=b"USER CONFIG\x00\xff", config_mode=mode)
                self.assertEqual(result.returncode, 0, result.stderr)
                config = home / "config/emuwiz/config.toml"
                self.assertEqual(config.read_bytes(), b"USER CONFIG\x00\xff")
                self.assertEqual(stat.S_IMODE(config.stat().st_mode), mode)

    def test_unprovable_publication_state_is_reported_as_uncertain(self):
        # Simulate lost bookkeeping AFTER a real publication: the flag is cleared and the proof
        # descriptor is gone, so the worker cannot establish what happened. It must say so.
        line = next(l for l in helper.splitlines() if l.startswith("    # CLOSE-WINDOW:"))
        mutation = '    published = False\n    state = "owned regular"\n    os.close(stage_fd)'
        for shell in ("dash", "bash"):
            with self.subTest(shell=shell):
                result = self.run_helper(line, mutation, shell=shell)
                self.assertEqual(result.returncode, 130, result.stderr)
                self.assertIn("could not be established whether the starter config was published", result.stderr)
                self.assertIn("inspect " + str(self.destination), result.stderr)
                for wrong in ("published and is complete", "nothing was written", "nothing was changed", "Traceback"):
                    self.assertNotIn(wrong, result.stderr)
                self.assertTrue(self.destination.exists())  # it WAS published; the report must not deny it
                self.destination.unlink()

    def test_signal_handler_never_raises(self):
        # The root cause of the false "nothing was changed": an exception raised from a handler can
        # fire after linkat() or inside the cleanup. Handlers must only record.
        body = helper.split("def record_signal(signum, frame):", 1)[1].split("\n\n", 1)[0]
        self.assertNotIn("raise", body)
        self.assertIn("received.append(signum)", body)
        self.assertNotIn("signal.pthread_sigmask", helper)  # no signal masking is needed

    # ---- configuration directory creation and safety (never follows the umask, never chmods) ----

    def test_new_config_directories_are_private_under_every_umask(self):
        for um in (0o000, 0o002, 0o022, 0o027, 0o077):
            for shell in ("dash", "bash"):
                with self.subTest(umask=oct(um), shell=shell):
                    # The parent ("config") does not exist either: every directory this
                    # step creates must be 0700 from creation, whatever the caller's umask.
                    result, home, bundle = self.install_bundle(
                        installer, shell, name="fresh-%o-%s" % (um, shell), umask=um)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    for created in (home / "config", home / "config/emuwiz"):
                        self.assertEqual(stat.S_IMODE(created.stat().st_mode), 0o700, created)
                    config = home / "config/emuwiz/config.toml"
                    self.assertEqual(stat.S_IMODE(config.stat().st_mode), 0o600)
                    self.assertEqual(config.read_bytes(), example.read_bytes())

    def test_existing_directories_are_never_chmodded_and_group_writable_is_flagged(self):
        for mode in (0o700, 0o750, 0o755, 0o775):
            with self.subTest(mode=oct(mode)):
                self.directory.chmod(mode)
                (self.directory / "user-file").write_bytes(b"KEEP")
                result = self.run_helper()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(stat.S_IMODE(self.directory.stat().st_mode), mode)
                self.assertEqual((self.directory / "user-file").read_bytes(), b"KEEP")
                self.assertEqual(self.destination.read_bytes(), example.read_bytes())
                if mode & 0o020:
                    self.assertIn("group-writable", result.stderr)
                    self.assertIn("left unchanged", result.stderr)
                else:
                    self.assertEqual(result.stderr, "")
                self.destination.unlink()
        self.directory.chmod(0o700)

    def test_world_writable_existing_directory_is_refused_unchanged(self):
        for mode in (0o777, 0o757, 0o707, 0o1777, 0o702):
            for shell in ("dash", "bash"):
                with self.subTest(mode=oct(mode), shell=shell):
                    self.directory.chmod(mode)
                    (self.directory / "user-file").write_bytes(b"KEEP")
                    before = self.directory.stat()
                    result = self.run_helper(shell=shell)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("writable by every user", result.stderr)
                    self.assertIn("It was not changed", result.stderr)
                    self.assertNotIn("Traceback", result.stderr)
                    self.assertIn("result=cannot publish the starter config safely", result.stdout)
                    self.assertEqual(self.directory.stat().st_mode, before.st_mode)
                    self.assertEqual((self.directory / "user-file").read_bytes(), b"KEEP")
                    self.assertFalse(self.destination.exists())
                    self.no_stage()
        self.directory.chmod(0o700)

    def test_directory_owned_by_another_user_is_refused_unchanged(self):
        # Only the effective-uid comparison is simulated (the test cannot create a foreign uid).
        self.assertEqual(helper.count("os.geteuid()"), 2)
        text = helper.replace("dir_info.st_uid != os.geteuid()", "dir_info.st_uid != os.geteuid() + 1")
        before = self.directory.stat()
        result = self.run_helper(text=text)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("is owned by uid", result.stderr)
        self.assertIn("could replace config.toml", result.stderr)
        self.assertEqual(self.directory.stat().st_mode, before.st_mode)
        self.assertFalse(self.destination.exists())
        self.no_stage()

    def test_symlinked_configuration_directory_is_judged_on_its_target(self):
        real = self.root / "real-config"
        real.mkdir(mode=0o700)
        alias = self.root / "alias-config"
        alias.symlink_to(real)
        script = self.root / "alias.sh"
        def run(directory):
            script.write_text(helper + '\npublish_starter_config %s %s %s\nrc=$?\nprintf "result=%%s\\n" "$starter_config_result"\nexit "$rc"\n'
                              % tuple(shlex.quote(str(p)) for p in (example, directory, directory / "config.toml")))
            return subprocess.run(["dash", str(script)], capture_output=True, text=True, timeout=10)
        safe = run(alias)
        self.assertEqual(safe.returncode, 0, safe.stderr)
        self.assertEqual((real / "config.toml").read_bytes(), example.read_bytes())
        self.assertTrue(alias.is_symlink())
        (real / "config.toml").unlink()
        real.chmod(0o777)
        unsafe = run(alias)
        self.assertNotEqual(unsafe.returncode, 0)
        self.assertIn("writable by every user", unsafe.stderr)
        self.assertEqual(stat.S_IMODE(real.stat().st_mode), 0o777)
        self.assertFalse((real / "config.toml").exists())
        real.chmod(0o700)

    def test_unsupported_environment_still_creates_only_a_private_directory(self):
        import shutil as _shutil
        _shutil.rmtree(self.directory)
        text = helper.replace("if not hasattr(os, needed):", "if True:")
        result = self.run_helper(text=text, umask=0o000)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Nothing was attempted", result.stderr)
        self.assertEqual(stat.S_IMODE(self.directory.stat().st_mode), 0o700)
        self.assertFalse(self.destination.exists())
        self.no_stage()

    def test_existing_config_in_an_exposed_directory_is_untouched_but_flagged(self):
        for mode, expect in ((0o777, "writable by every user"), (0o775, None), (0o700, None)):
            with self.subTest(mode=oct(mode)):
                result, home, bundle = self.install_bundle(
                    installer, "dash", name="exposed-%o" % mode, config_bytes=b"USER CONFIG\x00\xff", config_mode=0o644)
                directory = home / "config/emuwiz"
                # (install_bundle already ran once at the default mode; rerun at the chosen one)
                directory.chmod(mode)
                again = subprocess.run(["dash", str(bundle / "install.sh"), "--prefix", str(bundle / "installed"), "--developer-unchecked-install"],
                                       env=dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
                                                XDG_DATA_HOME=str(home / "data"), XDG_CACHE_HOME=str(home / "cache"),
                                                XDG_STATE_HOME=str(home / "state"), EMUWIZ_CONFIG_HOME=str(directory)),
                                       stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
                self.assertEqual(again.returncode, 0, again.stderr)
                self.assertEqual((directory / "config.toml").read_bytes(), b"USER CONFIG\x00\xff")
                self.assertEqual(stat.S_IMODE((directory / "config.toml").stat().st_mode), 0o644)
                self.assertEqual(stat.S_IMODE(directory.stat().st_mode), mode)
                if expect:
                    self.assertIn(expect, again.stderr)
                    self.assertIn("left unchanged", again.stderr)
                else:
                    self.assertNotIn("writable by every user", again.stderr)
                directory.chmod(0o700)

    def test_full_install_with_exposed_directory_keeps_assets_and_exits_nonzero(self):
        result, home2, bundle = self.install_bundle(installer, "dash", name="full-exposed-run")
        # fresh run is private and succeeds; now repeat with a pre-existing 0777 directory
        exposed = bundle / "home2"
        (exposed / "config/emuwiz").mkdir(parents=True)
        (exposed / "config/emuwiz").chmod(0o777)
        env = dict(os.environ, HOME=str(exposed), XDG_CONFIG_HOME=str(exposed / "config"), XDG_DATA_HOME=str(exposed / "data"),
                   XDG_CACHE_HOME=str(exposed / "cache"), XDG_STATE_HOME=str(exposed / "state"),
                   EMUWIZ_CONFIG_HOME=str(exposed / "config/emuwiz"))
        run = subprocess.run(["dash", str(bundle / "install.sh"), "--prefix", str(bundle / "installed2"), "--developer-unchecked-install"],
                             env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
        self.assertEqual(run.returncode, 1, run.stderr)
        self.assertIn("writable by every user", run.stderr)
        self.assertIn("remain in place", run.stderr)
        self.assertTrue((bundle / "installed2/emuwiz-cli").is_file())
        self.assertEqual(stat.S_IMODE((exposed / "config/emuwiz").stat().st_mode), 0o777)
        self.assertFalse((exposed / "config/emuwiz/config.toml").exists())

    def test_trailing_separator_config_directory(self):
        text = helper.replace('src, directory, destination = sys.argv[1:]',
                              'src, directory, destination = sys.argv[1:]\ndirectory += "/"')
        result = self.run_helper(text=text)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.destination.read_bytes(), example.read_bytes())
        self.no_stage()

    def test_directory_substitution_cannot_report_success(self):
        # Both sides of linkat, including ancestor replacement and unreadable
        # ancestry. Only disposable installer copies acquire these test seams.
        for after in (False, True):
            for action in ("directory", "symlink", "ancestor", "inaccessible"):
                with self.subTest(after_publication=after, action=action):
                    prefix = "        # PUBLISHED-WINDOW:" if after else "    # PUBLISH-WINDOW:"
                    indent = "        " if after else "    "
                    statements = ["from pathlib import Path", "d = Path(directory)"]
                    if action == "ancestor":
                        statements += ["d.parent.rename(str(d.parent) + '.pinned')", "d.mkdir(parents=True)"]
                    elif action == "inaccessible":
                        statements += ["vault = d.parent / 'vault'", "vault.mkdir()", "d.rename(vault / 'pinned')",
                                       "vault.chmod(0)", "d.mkdir()"]
                    else:
                        statements += ["d.rename(str(d) + '.pinned')"]
                        if action == "symlink":
                            statements += ["outside = d.parent / 'outside'", "outside.mkdir()",
                                           "(outside / 'canary').write_bytes(b'KEEP')", "d.symlink_to(outside)"]
                        else:
                            statements += ["d.mkdir()"]
                    mutation = "\n".join(indent + line for line in statements)
                    script = self.mutated_installer(prefix, mutation)
                    result, home, bundle = self.install_bundle(script, name="binding-%s-%s" % (after, action))
                    directory = home / "config/emuwiz"
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertIn("UNCERTAIN", result.stderr)
                    self.assertIn("remain in place", result.stderr)
                    self.assertNotIn("Wrote a starter config", result.stdout)
                    self.assertNotIn("nothing was changed", result.stderr)
                    self.assertFalse((directory / "config.toml").exists())
                    self.assertTrue((bundle / "installed/emuwiz-cli").is_file())
                    self.assertTrue((home / "data/emuwiz-installer/manifest").is_file())
                    if action == "ancestor":
                        pinned = home / "config.pinned/emuwiz/config.toml"
                    elif action == "inaccessible":
                        with self.assertRaises(PermissionError):
                            (directory.parent / "vault/pinned/config.toml").read_bytes()
                        # Intrinsic fixture teardown needs access to its own vault.
                        (directory.parent / "vault").chmod(0o700)
                        pinned = directory.parent / "vault/pinned/config.toml"
                    else:
                        pinned = directory.parent / "emuwiz.pinned/config.toml"
                    self.assertEqual(pinned.read_bytes(), example.read_bytes())
                    if action == "symlink":
                        self.assertTrue(directory.is_symlink())
                        self.assertEqual((directory.parent / "outside/canary").read_bytes(), b"KEEP")
                        self.assertFalse((directory.parent / "outside/config.toml").exists())

    def test_post_publication_config_substitution_is_uncertain(self):
        line = next(line for line in helper.splitlines() if line.startswith("        # PUBLISHED-WINDOW:"))
        for kind in ("symlink", "fifo", "missing"):
            with self.subTest(kind=kind):
                mutation = "        os.rename(destination, destination + '.pinned')\n"
                if kind == "symlink":
                    mutation += "        os.symlink(" + repr(str(self.foreign)) + ", destination)"
                elif kind == "fifo":
                    mutation += "        os.mkfifo(destination)"
                result = self.run_helper(line, mutation)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("UNCERTAIN", result.stderr)
                self.assertNotIn("nothing was changed", result.stderr)
                self.assertEqual(Path(str(self.destination) + ".pinned").read_bytes(), example.read_bytes())
                self.unchanged()
                if kind != "missing":
                    self.destination.unlink()
                Path(str(self.destination) + ".pinned").unlink()

    def test_unverifiable_post_publication_path_is_uncertain(self):
        line = next(line for line in helper.splitlines() if line.startswith("        # PUBLISHED-WINDOW:"))
        for failure, signalled in (("EACCES", False), ("EIO", False), ("EACCES", True), ("EIO", True)):
            with self.subTest(failure=failure, signalled=signalled):
                mutation = "        def verify_configured_publication():\n            raise OSError(errno.%s, 'verification unavailable')" % failure
                if signalled:
                    mutation += "\n        os.kill(os.getpid(), signal.SIGTERM)"
                result = self.run_helper(line, mutation)
                self.assertEqual(result.returncode, 130 if signalled else 1, result.stderr)
                if signalled:
                    self.assertIn("installation interrupted", result.stderr)
                self.assertNotIn("published and is complete", result.stderr)
                self.assertIn("UNCERTAIN", result.stderr)
                self.assertNotIn("nothing was changed", result.stderr)
                self.assert_published_and_complete()
                self.destination.unlink()

    def test_private_creation_masks_inherited_named_acl_entries(self):
        if not shutil.which("setfacl") or not shutil.which("getfacl"):
            self.skipTest("setfacl/getfacl unavailable: default-ACL effective-access coverage not established")
        self.directory.rmdir()
        subprocess.run(["setfacl", "-m", "d:u::rwx,d:u:65534:rwx,d:g::rwx,d:g:65534:rwx,d:m::rwx,d:o::---",
                        str(self.root)], check=True)
        before = subprocess.check_output(["getfacl", "-cpn", str(self.root)], text=True)
        self.directory = self.root / "missing/config"
        self.destination = self.directory / "config.toml"
        result = self.run_helper(umask=0o077)
        self.assertEqual(result.returncode, 0, result.stderr)
        for created in (self.directory.parent, self.directory):
            self.assertEqual(stat.S_IMODE(created.stat().st_mode) & 0o777, 0o700)
            acl = subprocess.check_output(["getfacl", "-cpn", str(created)], text=True)
            print("ACL evidence: %s mode=%04o\n%s" % (created, stat.S_IMODE(created.stat().st_mode), acl))
            access = [line for line in acl.splitlines() if line and not line.startswith(("#", "default:"))]
            self.assertIn("mask::---", access)
            self.assertIn("other::---", access)
            self.assertTrue(any(line.startswith("user:65534:") for line in access))
            self.assertTrue(any(line.startswith("group:65534:") for line in access))
            for line in access:
                if line.startswith(("user:65534:", "group:65534:", "group::")):
                    self.assertIn("#effective:---", line)
        self.assertEqual(subprocess.check_output(["getfacl", "-cpn", str(self.root)], text=True), before)
        self.assert_published_and_complete()

    def test_no_named_cleanup_or_unsafe_fallback(self):
        self.assertNotIn("rm -", helper)
        self.assertNotIn("unlink(", helper)
        self.assertNotIn("set -C", helper)
        self.assertIn("os.O_TMPFILE", helper)
        self.assertIn("written = os.write(stage_fd, remaining)", helper)
        self.assertIn('"published"', helper)
        self.assertIn('"attempted/unproven"', helper)

unittest.main(argv=["anonymous-staging"], verbosity=2)
STAGING_TESTS
then
    ok "anonymous staging ownership and preservation regressions"
else
    bad "anonymous staging ownership and preservation regressions"
fi

# Permission-denied destination directory: warn, change nothing, exit 1.
# (Skipped when running as root, which ignores directory permissions.)
if [ "$(id -u)" -ne 0 ]; then
    cfg_case
    mkdir -p -- "$cfg_home/.config/emuwiz"
    chmod 555 -- "$cfg_home/.config/emuwiz"
    deny_err=$(env HOME="$cfg_home" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" 2>&1 >/dev/null) && deny_rc=0 || deny_rc=$?
    chmod 755 -- "$cfg_home/.config/emuwiz"
    if [ "$deny_rc" -ne 0 ]; then ok "permission-denied config dir: exits non-zero"; else bad "permission-denied config dir: exits non-zero"; fi
    assert_contains "permission-denied config dir: warning names the file" "$deny_err" "could not write a starter config"
    assert_no_such_path "permission-denied config dir: no config created" "$cfg_home/.config/emuwiz/config.toml"
    assert_executable "permission-denied config dir: binaries were still installed" "$cfg_bin/emuwiz-cli"
    rm -rf -- "$cfg_work"
fi

# Concurrent installers sharing one config directory: exactly one writer wins,
# the file equals the example, and no staging files remain.
cfg_case
for n in 1 2 3 4 5 6; do
    mkdir -p -- "$cfg_work/p$n"
    env HOME="$cfg_home" XDG_DATA_HOME="$cfg_work/p$n/data" \
        sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_work/p$n/bin" >"$cfg_work/p$n/out" 2>&1 &
done
wait
assert_files_equal "concurrent installers: config equals the example" \
    "$config_example" "$cfg_home/.config/emuwiz/config.toml"
wrote=$(cat "$cfg_work"/p*/out | grep -c '^Wrote a starter config' || true)
if [ "$wrote" -eq 1 ]; then ok "concurrent installers: exactly one reports writing the config"; else bad "concurrent installers: exactly one reports writing the config (got $wrote)"; fi
if ls -A "$cfg_home/.config/emuwiz" | grep -q 'stage'; then bad "concurrent installers: no staging leftovers"; else ok "concurrent installers: no staging leftovers"; fi
rm -rf -- "$cfg_work"

# EMUWIZ_CONFIG_HOME: the directory itself, no legacy fallback.
cfg_case
mkdir -p -- "$cfg_home/.config/archivefs"
printf 'legacy\n' >"$cfg_home/.config/archivefs/config.toml"
env HOME="$cfg_home" EMUWIZ_CONFIG_HOME="$cfg_work/explicit" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" >/dev/null 2>&1
assert_files_equal "EMUWIZ_CONFIG_HOME: starter config lands in the override" \
    "$config_example" "$cfg_work/explicit/config.toml"
assert_contains "EMUWIZ_CONFIG_HOME: legacy profile untouched" "$(cat "$cfg_home/.config/archivefs/config.toml")" "legacy"
assert_no_such_path "EMUWIZ_CONFIG_HOME: nothing created under ~/.config/emuwiz" "$cfg_home/.config/emuwiz"
rm -rf -- "$cfg_work"

# XDG_CONFIG_HOME: $XDG_CONFIG_HOME/emuwiz; EMUWIZ_CONFIG_HOME wins over it.
cfg_case
env HOME="$cfg_home" XDG_CONFIG_HOME="$cfg_work/xdg" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" >/dev/null 2>&1
assert_files_equal "XDG_CONFIG_HOME: starter config lands in <xdg>/emuwiz" \
    "$config_example" "$cfg_work/xdg/emuwiz/config.toml"
assert_no_such_path "XDG_CONFIG_HOME: nothing under ~/.config" "$cfg_home/.config"
rm -rf -- "$cfg_work"
cfg_case
env HOME="$cfg_home" XDG_CONFIG_HOME="$cfg_work/xdg" EMUWIZ_CONFIG_HOME="$cfg_work/explicit" \
    sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" >/dev/null 2>&1
assert_file_exists "EMUWIZ_CONFIG_HOME wins over XDG_CONFIG_HOME" "$cfg_work/explicit/config.toml"
assert_no_such_path "XDG location unused when explicit root is set" "$cfg_work/xdg"
rm -rf -- "$cfg_work"

# XDG_CONFIG_HOME never falls back to the legacy directory.
cfg_case
mkdir -p -- "$cfg_home/.config/archivefs"
env HOME="$cfg_home" XDG_CONFIG_HOME="$cfg_work/xdg" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" >/dev/null 2>&1
assert_file_exists "XDG root ignores a legacy profile" "$cfg_work/xdg/emuwiz/config.toml"
rm -rf -- "$cfg_work"

# Relative overrides are refused like the application does: binaries install,
# no config is written, exit is non-zero, and nothing is created in the cwd.
for var in EMUWIZ_CONFIG_HOME XDG_CONFIG_HOME; do
    cfg_case
    rel_rc=0
    ( cd "$cfg_work" && env HOME="$cfg_home" "$var=relative/cfg" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" ) >/dev/null 2>"$cfg_work/err" || rel_rc=$?
    if [ "$rel_rc" -ne 0 ]; then ok "relative $var: exits non-zero"; else bad "relative $var: exits non-zero"; fi
    assert_contains "relative $var: reason is reported" "$(cat "$cfg_work/err")" "not an absolute path"
    assert_no_such_path "relative $var: nothing created relative to cwd" "$cfg_work/relative"
    assert_no_such_path "relative $var: no config under home" "$cfg_home/.config/emuwiz/config.toml"
    assert_executable "relative $var: binaries still installed" "$cfg_bin/emuwiz-cli"
    rm -rf -- "$cfg_work"
done

# A broken ~/.config/emuwiz entry still has precedence over a legacy profile
# (matches the application), so no starter config is written to the legacy one.
cfg_case
mkdir -p -- "$cfg_home/.config/archivefs"
printf 'legacy\n' >"$cfg_home/.config/archivefs/config.toml"
ln -s -- "$cfg_work/gone" "$cfg_home/.config/emuwiz"
env HOME="$cfg_home" sh "$cfg_work/bundle/install.sh" --developer-unchecked-install --prefix "$cfg_bin" >/dev/null 2>&1 || true
assert_contains "broken emuwiz entry: legacy config untouched" "$(cat "$cfg_home/.config/archivefs/config.toml")" "legacy"
assert_no_such_path "broken emuwiz entry: symlink target not created" "$cfg_work/gone"
rm -rf -- "$cfg_work"

echo "=== test: uninstall precisely removes owned binaries and desktop assets ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
# A file install.sh did not create must survive uninstall untouched.
printf 'unrelated\n' >"$bin_dir/some-other-tool"
mkdir -p -- "$home/.local/share/applications" \
    "$home/.local/share/icons/hicolor/64x64/apps"
printf 'unrelated desktop\n' >"$home/.local/share/applications/unrelated.desktop"
printf 'unrelated icon\n' >"$home/.local/share/icons/hicolor/64x64/apps/unrelated.png"

assert_success "uninstall succeeds" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir"
assert_no_such_path "emuwiz-cli removed" "$bin_dir/emuwiz-cli"
assert_no_such_path "archivefs-cli removed" "$bin_dir/archivefs-cli"
assert_no_such_path "emuwiz removed" "$bin_dir/emuwiz"
assert_no_such_path "emuwiz-gui alias removed" "$bin_dir/emuwiz-gui"
assert_no_such_path "archivefs-gui alias removed" "$bin_dir/archivefs-gui"
assert_no_such_path "EmuWiz desktop entry removed" \
    "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
for size in 32 64 128 256 512; do
    assert_no_such_path "$size pixel EmuWiz application icon removed" \
        "$home/.local/share/icons/hicolor/${size}x${size}/apps/io.github.kiehntre.emuwiz.png"
done
assert_file_exists "unrelated file in bin dir is untouched" "$bin_dir/some-other-tool"
assert_file_exists "unrelated desktop entry is untouched" \
    "$home/.local/share/applications/unrelated.desktop"
assert_file_exists "unrelated icon is untouched" \
    "$home/.local/share/icons/hicolor/64x64/apps/unrelated.png"
assert_file_exists "config.toml survives uninstall" "$home/.config/emuwiz/config.toml"
assert_success "uninstalling again is a no-op, not an error" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir"
rm -rf -- "$work"

echo "=== test: uninstall keeps going when an owned path is a directory ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
# Something has occupied the desktop entry's path with a directory instead
# of the file install.sh wrote there (broken package state, another tool,
# a prior crashed run, etc).
rm -f -- "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
mkdir -p -- "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" 2>&1 1>/dev/null)
ok "uninstall did not abort when the desktop entry path is a directory"
assert_contains "uninstall warns about the unexpected directory" "$warn" \
    "found a directory"
assert_no_such_path "emuwiz-cli is still removed despite the desktop-entry collision" \
    "$bin_dir/emuwiz-cli"
for size in 32 64 128 256 512; do
    assert_no_such_path "$size pixel icon is still removed despite the desktop-entry collision" \
        "$home/.local/share/icons/hicolor/${size}x${size}/apps/io.github.kiehntre.emuwiz.png"
done
if [ -d "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop" ]; then
    ok "the colliding directory itself was left in place"
else
    bad "the colliding directory itself was left in place"
fi
rm -rf -- "$work"

echo "=== test: fails clearly when required binaries are missing ==="
work=$(mktemp -d)
empty_dir="$work/empty"
mkdir -p -- "$empty_dir"
cp -- "$install_sh" "$empty_dir/install.sh"
home="$work/home"
mkdir -p -- "$home"
errfile=$(mktemp)

if env HOME="$home" sh "$empty_dir/install.sh" --developer-unchecked-install --prefix "$work/bin" >/dev/null 2>"$errfile"; then
    bad "install fails when no binaries are present"
else
    ok "install fails when no binaries are present"
fi
missing_err=$(cat "$errfile")
assert_contains "failure message names the missing binaries" "$missing_err" "emuwiz-cli"
rm -f -- "$errfile"
rm -rf -- "$work"

echo "=== test: warns when ratarmount is not on PATH ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

# A minimal PATH that has core utilities but almost certainly not
# ratarmount (which is normally pip/AppImage-installed under a user or
# venv directory, not /usr/bin or /bin).
warn=$(env HOME="$home" PATH="/usr/bin:/bin" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null)
assert_contains "prints a ratarmount warning" "$warn" "ratarmount was not found"
assert_contains "warning includes install guidance" "$warn" "pip install ratarmount"
rm -rf -- "$work"

echo "=== test: does not modify shell startup files ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
for rc in .bashrc .profile .zshrc; do
    printf 'original contents\n' >"$home/$rc"
done

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$work/bin" >/dev/null

startup_files_ok=1
for rc in .bashrc .profile .zshrc; do
    rc_content=$(cat "$home/$rc")
    if [ "$rc_content" != "original contents" ]; then
        startup_files_ok=0
    fi
done
if [ "$startup_files_ok" -eq 1 ]; then
    ok "shell startup files are untouched"
else
    bad "shell startup files are untouched"
fi
rm -rf -- "$work"

# ===========================================================================
# Ownership manifest: install/reinstall/uninstall collision safety.
# ===========================================================================

manifest_path() {
    printf '%s/emuwiz-installer/manifest\n' "$1"
}

echo "=== test: fresh install writes an ownership manifest for every slot ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
manifest=$(manifest_path "$home/.local/share")
assert_file_exists "install creates an ownership manifest" "$manifest"
manifest_content=$(cat "$manifest")
assert_contains "manifest records schema_version" "$manifest_content" "schema_version 2"
assert_contains "manifest records the resolved bin_dir" "$manifest_content" "bin_dir $bin_dir"
assert_contains "manifest records a record_count" "$manifest_content" "record_count 11"
assert_contains "manifest ends with the end marker" "$manifest_content" "
end"
for slot in bin-emuwiz-cli bin-emuwiz alias-archivefs-cli alias-emuwiz-gui \
    alias-archivefs-gui desktop icon-32 icon-64 icon-128 icon-256 icon-512; do
    assert_contains "manifest records slot $slot" "$manifest_content" "$slot "
done
for slot in bin-emuwiz-cli bin-emuwiz desktop icon-32 icon-64 icon-128 icon-256 icon-512; do
    digest_line=$(grep "^$slot file " "$manifest")
    digest_field=$(printf '%s\n' "$digest_line" | awk '{print $3}')
    if [ "${#digest_field}" -eq 64 ]; then
        case "$digest_field" in
            *[!0-9a-f]*) bad "slot $slot records a 64-character lowercase-hex SHA-256 digest (got: $digest_field)" ;;
            *) ok "slot $slot records a 64-character lowercase-hex SHA-256 digest" ;;
        esac
    else
        bad "slot $slot records a 64-character lowercase-hex SHA-256 digest (wrong length: $digest_field)"
    fi
done
slot_lines=$(grep -vc '^#' "$manifest")
# 4 header lines (schema_version, bin_dir, data_home, record_count) + 11
# slots + 1 end marker = 16.
if [ "$slot_lines" -eq 16 ]; then
    ok "manifest has exactly 16 non-comment lines (4 header + 11 slots + end)"
else
    bad "manifest has exactly 16 non-comment lines (4 header + 11 slots + end) (got $slot_lines)"
fi
perm=$(stat -c '%a' "$manifest" 2>/dev/null || stat -f '%Lp' "$manifest" 2>/dev/null)
if [ "$perm" = "600" ]; then
    ok "manifest is written with mode 600"
else
    bad "manifest is written with mode 600 (got $perm)"
fi
rm -rf -- "$work"

echo "=== test: reinstall against its own manifest prints no foreign warnings ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
reinstall_err=$(env HOME="$home" PATH="$fake_ratarmount_dir:$PATH" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null)
reinstall_err=$(printf '%s\n' "$reinstall_err" | strip_dev_warning)
if [ -z "$reinstall_err" ]; then
    ok "reinstall against a matching manifest produces no warnings"
else
    bad "reinstall against a matching manifest produces no warnings (got: $reinstall_err)"
fi
assert_success "reinstall against a matching manifest exits 0" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
rm -rf -- "$work"

echo "=== test: a foreign binary at the destination is not overwritten ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home" "$work/bin"
bin_dir="$work/bin"

foreign_marker="totally-unrelated-tool-$$"
printf '#!/bin/sh\necho %s\n' "$foreign_marker" >"$bin_dir/emuwiz-cli"
chmod +x -- "$bin_dir/emuwiz-cli"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install warns about the foreign binary" "$warn" \
    "leaving foreign path untouched"
assert_contains "warning names --replace-foreign" "$warn" "--replace-foreign"
foreign_content=$(cat "$bin_dir/emuwiz-cli")
assert_contains "foreign binary content is unchanged" "$foreign_content" "$foreign_marker"
assert_failure "install without --replace-foreign exits non-zero when something was skipped" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
# Everything else must still have installed normally around the collision.
assert_executable "emuwiz (the other binary) still installed" "$bin_dir/emuwiz"
assert_file_exists "desktop entry still installed despite the binary collision" \
    "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"

assert_success "install --replace-foreign succeeds over the foreign binary" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign
cli_out=$("$bin_dir/emuwiz-cli")
assert_contains "emuwiz-cli now runs the real installed stub" "$cli_out" "fake-cli"
backup_found=0
for f in "$bin_dir"/.emuwiz-foreign-backup.*/emuwiz-cli; do
    [ -f "$f" ] || continue
    backup_found=1
    backup_content=$(cat "$f")
    assert_contains "foreign-backup file preserves the original content" "$backup_content" "$foreign_marker"
done
if [ "$backup_found" -eq 1 ]; then
    ok "--replace-foreign left a recoverable backup of the foreign binary"
else
    bad "--replace-foreign left a recoverable backup of the foreign binary"
fi
rm -rf -- "$work"

echo "=== test: a foreign desktop entry is not overwritten ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share/applications"
bin_dir="$work/bin"

desktop_file="$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
printf '[Desktop Entry]\nType=Application\nName=NotEmuWiz\n' >"$desktop_file"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install warns about the foreign desktop entry" "$warn" \
    "leaving foreign path untouched"
desktop_content=$(cat "$desktop_file")
assert_contains "foreign desktop entry content is unchanged" "$desktop_content" "NotEmuWiz"
assert_executable "emuwiz-cli still installed despite the desktop-entry collision" "$bin_dir/emuwiz-cli"

assert_success "install --replace-foreign succeeds over the foreign desktop entry" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign
desktop_content=$(cat "$desktop_file")
assert_contains "desktop entry now has real EmuWiz content" "$desktop_content" \
    "Icon=io.github.kiehntre.emuwiz"
rm -rf -- "$work"

echo "=== test: a foreign icon is not overwritten ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share/icons/hicolor/32x32/apps"
bin_dir="$work/bin"

icon_file="$home/.local/share/icons/hicolor/32x32/apps/io.github.kiehntre.emuwiz.png"
printf 'not a real png\n' >"$icon_file"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install warns about the foreign icon" "$warn" "leaving foreign path untouched"
icon_content=$(cat "$icon_file")
assert_contains "foreign icon content is unchanged" "$icon_content" "not a real png"
assert_files_equal "the other 4 icon sizes still installed despite the 32px collision" \
    "$branding_assets/emuwiz-logo-64.png" \
    "$home/.local/share/icons/hicolor/64x64/apps/io.github.kiehntre.emuwiz.png"

assert_success "install --replace-foreign succeeds over the foreign icon" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign
assert_files_equal "32px icon now matches the approved asset" \
    "$branding_assets/emuwiz-logo-32.png" "$icon_file"
rm -rf -- "$work"

echo "=== test: a symlink alias pointing at the wrong target is foreign ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home" "$work/bin"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
rm -f -- "$bin_dir/archivefs-cli"
ln -s -- /etc/passwd "$bin_dir/archivefs-cli"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install warns about the wrong-target alias symlink" "$warn" \
    "leaving foreign path untouched"
target=$(readlink -- "$bin_dir/archivefs-cli")
if [ "$target" = /etc/passwd ]; then
    ok "wrong-target alias symlink was left exactly as it was"
else
    bad "wrong-target alias symlink was left exactly as it was (target now: $target)"
fi

assert_success "install --replace-foreign fixes the wrong-target alias" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign
target=$(readlink -- "$bin_dir/archivefs-cli")
if [ "$target" = emuwiz-cli ]; then
    ok "alias symlink now points at the correct target"
else
    bad "alias symlink now points at the correct target (got: $target)"
fi
rm -rf -- "$work"

echo "=== test: a stale manifest does not permit deleting a replaced foreign binary (same second, no sleep) ==="
# Deliberately no sleep anywhere in this test: content is the only
# ownership authority now, so a same-second, same-mtime replacement must
# be caught exactly as reliably as one seconds apart.
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
owned_stamp_ref="$bin_dir/emuwiz"
rm -f -- "$bin_dir/emuwiz-cli"
printf 'foreign replacement\n' >"$bin_dir/emuwiz-cli"
chmod +x -- "$bin_dir/emuwiz-cli"
# Forge the mtime to be identical, to the second, to another genuinely
# owned file installed in the same run - the strongest same-second forgery
# available without sub-second timers.
touch -r "$owned_stamp_ref" -- "$bin_dir/emuwiz-cli"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" 2>&1 1>/dev/null)
assert_contains "uninstall warns about the replaced binary" "$warn" \
    "leaving foreign path untouched"
foreign_content=$(cat "$bin_dir/emuwiz-cli")
assert_contains "replaced binary content survives uninstall" "$foreign_content" "foreign replacement"
assert_no_such_path "the other, still-owned binary is removed" "$bin_dir/emuwiz"
assert_no_such_path "the desktop entry is removed" \
    "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
rm -rf -- "$work"

echo "=== test: touch -r from a genuinely owned file does not launder foreign content (reinstall) ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
owned_ref="$bin_dir/emuwiz"
rm -f -- "$bin_dir/emuwiz-cli"
printf 'forged-mtime foreign content\n' >"$bin_dir/emuwiz-cli"
chmod +x -- "$bin_dir/emuwiz-cli"
touch -r "$owned_ref" -- "$bin_dir/emuwiz-cli"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install still detects foreign content after touch -r forgery" "$warn" \
    "leaving foreign path untouched"
foreign_content=$(cat "$bin_dir/emuwiz-cli")
assert_contains "touch -r forged foreign binary content is preserved" "$foreign_content" \
    "forged-mtime foreign content"
rm -rf -- "$work"

echo "=== test: touch -d to the exact recorded epoch does not launder foreign content (uninstall) ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
install_time=$(date -r "$bin_dir/emuwiz" +%s 2>/dev/null || stat -c '%Y' "$bin_dir/emuwiz")
rm -f -- "$bin_dir/emuwiz-cli"
printf 'touch-d forged foreign content\n' >"$bin_dir/emuwiz-cli"
chmod +x -- "$bin_dir/emuwiz-cli"
touch -d "@$install_time" -- "$bin_dir/emuwiz-cli" 2>/dev/null || \
    touch -t "$(date -d "@$install_time" +%Y%m%d%H%M.%S)" -- "$bin_dir/emuwiz-cli"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" 2>&1 1>/dev/null)
assert_contains "uninstall still detects foreign content after touch -d forgery" "$warn" \
    "leaving foreign path untouched"
foreign_content=$(cat "$bin_dir/emuwiz-cli")
assert_contains "touch -d forged foreign binary content survives uninstall" "$foreign_content" \
    "touch-d forged foreign content"
rm -rf -- "$work"

echo "=== test: cp -p (timestamp-preserving replacement) does not launder foreign content ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
foreign_src="$work/foreign-source"
printf 'cp -p forged foreign content\n' >"$foreign_src"
chmod +x -- "$foreign_src"
touch -r "$bin_dir/emuwiz-cli" -- "$foreign_src"
rm -f -- "$bin_dir/emuwiz-cli"
cp -p -- "$foreign_src" "$bin_dir/emuwiz-cli"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install still detects foreign content after cp -p replacement" "$warn" \
    "leaving foreign path untouched"
foreign_content=$(cat "$bin_dir/emuwiz-cli")
assert_contains "cp -p replaced foreign binary content is preserved" "$foreign_content" \
    "cp -p forged foreign content"
rm -rf -- "$work"

echo "=== test: digest ownership refreshes correctly across a legitimate content/version upgrade ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
v1_out=$("$bin_dir/emuwiz-cli")
assert_contains "v1 binary runs the v1 stub" "$v1_out" "fake-cli"

# Simulate a new release: the bundle's own binary content changes.
printf '#!/bin/sh\necho fake-cli-v2\n' >"$work/bundle/emuwiz-cli"
chmod +x -- "$work/bundle/emuwiz-cli"

v2_err=$(env HOME="$home" PATH="$fake_ratarmount_dir:$PATH" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null)
v2_err=$(printf '%s\n' "$v2_err" | strip_dev_warning)
if [ -z "$v2_err" ]; then
    ok "upgrading to v2 content is recognised as a legitimate reinstall (no foreign warning)"
else
    bad "upgrading to v2 content is recognised as a legitimate reinstall (no foreign warning) (got: $v2_err)"
fi
v2_out=$("$bin_dir/emuwiz-cli")
assert_contains "binary now runs the v2 stub" "$v2_out" "fake-cli-v2"

# A further reinstall against the NEW (v2) recorded digest must also be silent -
# proving the manifest was actually refreshed to the new content's digest,
# not left pointing at v1's.
v2_again_err=$(env HOME="$home" PATH="$fake_ratarmount_dir:$PATH" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null)
v2_again_err=$(printf '%s\n' "$v2_again_err" | strip_dev_warning)
if [ -z "$v2_again_err" ]; then
    ok "manifest was refreshed to the v2 digest (a further v2 reinstall is silent)"
else
    bad "manifest was refreshed to the v2 digest (a further v2 reinstall is silent) (got: $v2_again_err)"
fi
rm -rf -- "$work"

echo "=== test: a malformed manifest fails safe (treated as no manifest) ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
manifest=$(manifest_path "$home/.local/share")
printf 'garbage this is not a valid manifest !!! ***\n\x00\x01binary junk\n' >"$manifest"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "malformed manifest refuses publication (fail closed)" \
    "$warn" "refusing to install managed files"
assert_executable "malformed manifest leaves the existing managed binary untouched" \
    "$bin_dir/emuwiz-cli"
assert_success "replace-foreign recovers from a malformed manifest" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign
rm -rf -- "$work"

echo "=== test: a truncated manifest fails safe ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
manifest=$(manifest_path "$home/.local/share")
head -c 25 "$manifest" >"$work/truncated"
mv -- "$work/truncated" "$manifest"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "truncated manifest refuses publication (fail closed)" \
    "$warn" "refusing to install managed files"
assert_executable "truncated manifest leaves the existing managed binary untouched" \
    "$bin_dir/emuwiz-cli"
rm -rf -- "$work"

echo "=== test: an adversarial manifest cannot make uninstall touch an arbitrary path ==="
# Both sub-cases below keep the header (schema_version/bin_dir/data_home)
# perfectly valid, singular, and matching this exact run, so the parser
# runs past header validation and actually reaches - reads, and evaluates
# the key of - the injected malicious line itself, rather than rejecting
# the whole manifest earlier for an unrelated header mismatch. Duplicate-
# header rejection is covered separately and explicitly by the schema
# parser test battery below.
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
manifest=$(manifest_path "$home/.local/share")

canary="$work/outside-canary"
printf 'must never be touched\n' >"$canary"

# Sub-case 1: an unknown key naming an arbitrary path, spliced in among the
# otherwise-valid slot records (before the "end" marker, not appended after
# a header mismatch). install.sh never reads a path out of the manifest at
# all - only the fixed, closed set of slot names is ever consulted - so
# there is no key this could ever be that would cause anything to be read
# from or written to $canary.
sed '$d' "$manifest" >"$work/adversarial-1"
printf 'evil-path %s\n' "$canary" >>"$work/adversarial-1"
printf 'end\n' >>"$work/adversarial-1"
cp -- "$work/adversarial-1" "$manifest"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" >/dev/null 2>&1 || true
canary_content=$(cat "$canary")
assert_contains "canary survives an adversarial manifest with an injected unknown key" \
    "$canary_content" "must never be touched"
assert_executable "the binary is left in place too (manifest was rejected outright, so it looks foreign)" \
    "$bin_dir/emuwiz"

# Reinstall clean before sub-case 2, so there is a fresh, valid manifest to
# adversarially edit again.
env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign >/dev/null
manifest=$(manifest_path "$home/.local/share")

# Sub-case 2: a slot's "fingerprint" field replaced with a path string
# instead of a valid 64-character hex digest - the field-shape validation
# itself (validate_file_digest) is what must reject this, reached and
# evaluated at that specific slot's own line.
sed "s|^bin-emuwiz-cli file .*|bin-emuwiz-cli file $canary 4|" "$manifest" >"$work/adversarial-2"
cp -- "$work/adversarial-2" "$manifest"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" >/dev/null 2>&1 || true
canary_content=$(cat "$canary")
assert_contains "canary survives an adversarial manifest with a path-shaped digest field" \
    "$canary_content" "must never be touched"
assert_executable "the binary is left in place too (path-shaped digest was rejected, so it looks foreign)" \
    "$bin_dir/emuwiz"
rm -rf -- "$work"

echo "=== test: uninstall preserves a foreign replacement while removing everything else ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
rm -f -- "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
printf 'foreign desktop replacement\n' \
    >"$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" >/dev/null 2>&1 || true
desktop_content=$(cat "$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop")
assert_contains "foreign desktop replacement survives uninstall" "$desktop_content" \
    "foreign desktop replacement"
assert_no_such_path "binaries are still removed" "$bin_dir/emuwiz-cli"
for size in 32 64 128 256 512; do
    assert_no_such_path "$size px icon is still removed" \
        "$home/.local/share/icons/hicolor/${size}x${size}/apps/io.github.kiehntre.emuwiz.png"
done
assert_file_exists "the manifest is kept (something was left foreign)" \
    "$(manifest_path "$home/.local/share")"
rm -rf -- "$work"

echo "=== test: a clean uninstall removes its own bookkeeping manifest ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" >/dev/null
assert_no_such_path "manifest is removed after a fully clean uninstall" \
    "$(manifest_path "$home/.local/share")"
rm -rf -- "$work"

echo "=== test: uninstall is idempotent even after a foreign collision was left behind ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
rm -f -- "$bin_dir/emuwiz-cli"
printf 'foreign\n' >"$bin_dir/emuwiz-cli"
env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" >/dev/null 2>&1 || true
assert_success "uninstalling a second time after a foreign collision is still not an error" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir"
foreign_content=$(cat "$bin_dir/emuwiz-cli")
assert_contains "foreign binary is still there and still untouched" "$foreign_content" "foreign"
rm -rf -- "$work"

echo "=== test: a directory at a binary destination is left alone on install ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home" "$work/bin/emuwiz-cli"
bin_dir="$work/bin"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install warns about the directory collision" "$warn" \
    "leaving foreign path untouched"
if [ -d "$bin_dir/emuwiz-cli" ] && [ ! -L "$bin_dir/emuwiz-cli" ]; then
    ok "the colliding directory at the binary destination is untouched"
else
    bad "the colliding directory at the binary destination is untouched"
fi
assert_executable "emuwiz (the other binary) still installed despite the collision" \
    "$bin_dir/emuwiz"
rm -rf -- "$work"

echo "=== test: reinstall after a partial (never-manifested) install recovers safely ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

# Simulate a run that was killed after copying the binaries but before the
# manifest was ever written: no manifest exists, but a plain, genuinely
# EmuWiz-installed binary already sits at the destination.
mkdir -p -- "$bin_dir"
cp -- "$work/bundle/emuwiz-cli" "$bin_dir/emuwiz-cli"
chmod +x -- "$bin_dir/emuwiz-cli"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "the interrupted run's own binary is treated as foreign, not silently claimed" \
    "$warn" "leaving foreign path untouched"
assert_success "--replace-foreign completes the interrupted install" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign
assert_file_exists "manifest now exists after the completed reinstall" \
    "$(manifest_path "$home/.local/share")"
assert_success "a further reinstall is now silent and clean" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
rm -rf -- "$work"

echo "=== test: legacy aliases still resolve to the real binaries after ownership tracking ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
cli_out=$("$bin_dir/archivefs-cli")
assert_contains "archivefs-cli alias still runs the real emuwiz-cli" "$cli_out" "fake-cli"
gui_target=$(readlink -- "$bin_dir/emuwiz-gui")
[ "$gui_target" = emuwiz ] && ok "emuwiz-gui alias points at emuwiz" || bad "emuwiz-gui alias points at emuwiz"
gui_target=$(readlink -- "$bin_dir/archivefs-gui")
[ "$gui_target" = emuwiz ] && ok "archivefs-gui alias points at emuwiz" || bad "archivefs-gui alias points at emuwiz"
rm -rf -- "$work"

# ===========================================================================
# --replace-foreign backup safety: exclusive allocation, no clobbering, no
# symlink-following, real mid-install failure recovery.
# ===========================================================================

echo "=== test: pre-existing decoys near the backup naming prefix do not interfere with --replace-foreign ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home" "$work/bin"
bin_dir="$work/bin"

foreign_marker="decoy-test-foreign-$$"
printf '#!/bin/sh\necho %s\n' "$foreign_marker" >"$bin_dir/emuwiz-cli"
chmod +x -- "$bin_dir/emuwiz-cli"

# Plant a regular file, a symlink, and a directory that all share the
# static ".emuwiz-foreign-backup." prefix this installer uses - the closest
# an attacker without the ability to predict mktemp's random suffix could
# plausibly pre-stage. None of these are the actual backup mktemp -d will
# allocate (its suffix is unpredictable), so none of them should be
# touched, read through, or interfered with.
printf 'decoy regular file - must survive\n' >"$bin_dir/.emuwiz-foreign-backup.decoyfile"
ln -s -- /etc/passwd "$bin_dir/.emuwiz-foreign-backup.decoylink"
mkdir -p -- "$bin_dir/.emuwiz-foreign-backup.decoydir"
printf 'decoy directory contents - must survive\n' >"$bin_dir/.emuwiz-foreign-backup.decoydir/canary"

assert_success "install --replace-foreign succeeds despite backup-prefix decoys" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign
assert_contains "decoy regular file is untouched" \
    "$(cat "$bin_dir/.emuwiz-foreign-backup.decoyfile")" "must survive"
decoy_link_target=$(readlink -- "$bin_dir/.emuwiz-foreign-backup.decoylink")
[ "$decoy_link_target" = /etc/passwd ] \
    && ok "decoy symlink still points where it always did (never followed)" \
    || bad "decoy symlink still points where it always did (never followed) (got: $decoy_link_target)"
assert_contains "decoy directory contents are untouched" \
    "$(cat "$bin_dir/.emuwiz-foreign-backup.decoydir/canary")" "must survive"

real_backup_found=0
for d in "$bin_dir"/.emuwiz-foreign-backup.*; do
    case "$d" in
        *decoyfile|*decoylink|*decoydir) continue ;;
    esac
    [ -d "$d" ] || continue
    real_backup_found=1
    backup_content=$(cat "$d/emuwiz-cli")
    assert_contains "the real (non-decoy) backup preserves the original foreign content" \
        "$backup_content" "$foreign_marker"
done
if [ "$real_backup_found" -eq 1 ]; then
    ok "a distinct, non-colliding backup directory was allocated alongside the decoys"
else
    bad "a distinct, non-colliding backup directory was allocated alongside the decoys"
fi
rm -rf -- "$work"

echo "=== test: multiple simultaneous foreign collisions each get their own distinct backup ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share/applications" "$work/bin"
bin_dir="$work/bin"

bin_marker="multi-foreign-binary-$$"
printf '#!/bin/sh\necho %s\n' "$bin_marker" >"$bin_dir/emuwiz-cli"
chmod +x -- "$bin_dir/emuwiz-cli"
desktop_file="$home/.local/share/applications/io.github.kiehntre.emuwiz.desktop"
printf '[Desktop Entry]\nType=Application\nName=MultiForeign\n' >"$desktop_file"

assert_success "install --replace-foreign resolves two simultaneous collisions in one run" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign

bin_backup_content=""
for d in "$bin_dir"/.emuwiz-foreign-backup.*; do
    [ -f "$d/emuwiz-cli" ] || continue
    bin_backup_content=$(cat "$d/emuwiz-cli")
done
assert_contains "the binary's own backup preserves its original content" \
    "$bin_backup_content" "$bin_marker"

desktop_backup_content=""
for d in "$home/.local/share/applications"/.emuwiz-foreign-backup.*; do
    [ -f "$d/io.github.kiehntre.emuwiz.desktop" ] || continue
    desktop_backup_content=$(cat "$d/io.github.kiehntre.emuwiz.desktop")
done
assert_contains "the desktop entry's own backup preserves its original content" \
    "$desktop_backup_content" "MultiForeign"

new_desktop_content=$(cat "$desktop_file")
assert_contains "the real desktop entry now has genuine EmuWiz content" \
    "$new_desktop_content" "Icon=io.github.kiehntre.emuwiz"
cli_out=$("$bin_dir/emuwiz-cli")
assert_contains "the real binary now runs the genuine stub" "$cli_out" "fake-cli"
rm -rf -- "$work"

echo "=== test: a real mid-install failure after a successful backup preserves the foreign object ==="
# Genuinely induced, not hand-constructed: the bundle's own source binary
# is made unreadable so cp actually fails partway through
# install_binary_slot, for real, right after backup_foreign_path has
# already succeeded and moved the foreign content aside - exercising the
# exact "backed up, then the next step fails" ordering rather than
# simulating its end state.
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home" "$work/bin"
bin_dir="$work/bin"

foreign_marker="pre-failure-foreign-$$"
printf '#!/bin/sh\necho %s\n' "$foreign_marker" >"$bin_dir/emuwiz-cli"
chmod +x -- "$bin_dir/emuwiz-cli"
# The installer now copies source binaries into a private snapshot before
# anything else, so an unreadable bundle binary is refused up front (checked
# below). To keep exercising "backed up, then the copy fails", a private
# installer copy makes its own snapshot of emuwiz-cli unreadable just after
# taking it.
sed 's|^src_gui="\$release_snapshot/emuwiz"$|&\nchmod 000 -- "$src_cli"|' "$install_sh" >"$work/bundle/install.sh"
grep -q '^chmod 000 -- "\$src_cli"$' "$work/bundle/install.sh" ||
    bad "test setup: could not inject the snapshot failure"

fail_out=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign 2>&1 1>/dev/null) || fail_status=$?
cp -- "$install_sh" "$work/bundle/install.sh"
if [ "${fail_status:-0}" -ne 0 ]; then
    ok "the genuinely-induced cp failure aborts the run with a non-zero exit"
else
    bad "the genuinely-induced cp failure aborts the run with a non-zero exit"
fi
assert_contains "the backup location was reported before the crash (deterministic recovery instruction)" \
    "$fail_out" "moved foreign path aside before installing"
assert_no_such_path "the destination is left absent, not half-written" "$bin_dir/emuwiz-cli"

backup_dir_found=""
for d in "$bin_dir"/.emuwiz-foreign-backup.*; do
    [ -f "$d/emuwiz-cli" ] || continue
    backup_dir_found=$d
done
if [ -n "$backup_dir_found" ]; then
    ok "the foreign object survives the crash, sitting exactly where it was backed up"
    backup_content=$(cat "$backup_dir_found/emuwiz-cli")
    assert_contains "the surviving backup still holds the original foreign content" \
        "$backup_content" "$foreign_marker"
else
    bad "the foreign object survives the crash, sitting exactly where it was backed up"
fi

# Recovery: the destination is absent now (the foreign object moved away,
# the new copy never landed), so a plain rerun - no --replace-foreign
# needed - completes the interrupted install cleanly, and the backup from
# the failed run is still there afterwards, never auto-deleted.
assert_success "a plain rerun after fixing the underlying cause completes cleanly" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
cli_out=$("$bin_dir/emuwiz-cli")
assert_contains "the real binary is now installed" "$cli_out" "fake-cli"
if [ -f "$backup_dir_found/emuwiz-cli" ]; then
    ok "the pre-crash backup is still there after recovery (nothing auto-deletes it)"
else
    bad "the pre-crash backup is still there after recovery (nothing auto-deletes it)"
fi
recovered_content=$(cat "$backup_dir_found/emuwiz-cli")
assert_contains "the pre-crash backup content is still exactly recoverable" \
    "$recovered_content" "$foreign_marker"
rm -rf -- "$work"

echo "=== test: checksum-less layouts are refused without the explicit developer flag ==="
for layout in bundle workspace; do
    work=$(mktemp -d)
    if [ "$layout" = bundle ]; then make_bundle "$work/src"; else make_workspace "$work/src"; fi
    home="$work/home"
    mkdir -p -- "$home"
    refuse_err=$(env HOME="$home" sh "$work/src/install.sh" --prefix "$work/bin" 2>&1 1>/dev/null) && refuse_status=0 || refuse_status=$?
    if [ "$refuse_status" -eq 1 ]; then
        ok "$layout without SHA256SUMS is refused by default"
    else
        bad "$layout without SHA256SUMS is refused by default (exit $refuse_status)"
    fi
    assert_contains "$layout refusal names the developer-only flag" "$refuse_err" "--developer-unchecked-install"
    assert_no_such_path "$layout refusal creates no install directory" "$work/bin"
    assert_no_such_path "$layout refusal creates no bookkeeping" "$home/.local/share/emuwiz-installer"
    rm -rf -- "$work"
done

echo "=== test: an unreadable source binary is refused before anything changes ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home" "$work/bin"
printf 'foreign\n' >"$work/bin/emuwiz-cli"
chmod 000 -- "$work/bundle/emuwiz-cli"
if [ "$(id -u)" -ne 0 ]; then
    assert_failure "unreadable source binary refuses" \
        env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$work/bin" --replace-foreign
    foreign_now=$(cat "$work/bin/emuwiz-cli")
    assert_contains "the foreign destination was not moved aside" "$foreign_now" "foreign"
    assert_no_such_path "no bookkeeping was created" "$home/.local/share/emuwiz-installer"
fi
chmod 755 -- "$work/bundle/emuwiz-cli"
rm -rf -- "$work"

# ===========================================================================
# Manifest-directory and manifest-file safety: the bookkeeping path itself
# is part of the security boundary.
# ===========================================================================

echo "=== test: the installer bookkeeping directory as a symlink is refused, and outside canaries survive ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share"
bin_dir="$work/bin"

outside="$work/outside-target"
mkdir -p -- "$outside"
printf 'must never be touched\n' >"$outside/canary.txt"
ln -s -- "$outside" "$home/.local/share/emuwiz-installer"

assert_failure "install refuses outright when the bookkeeping directory is a symlink" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
assert_no_such_path "nothing was installed at all (refusal is total, not partial)" \
    "$bin_dir/emuwiz-cli"
canary_content=$(cat "$outside/canary.txt")
assert_contains "the outside canary survives byte-for-byte" "$canary_content" "must never be touched"
if [ -L "$home/.local/share/emuwiz-installer" ]; then
    ok "the symlink itself was left exactly as it was (never replaced, never traversed-through)"
else
    bad "the symlink itself was left exactly as it was (never replaced, never traversed-through)"
fi
outside_listing=$(ls -a -- "$outside")
assert_contains "nothing new was created inside the symlink target" "$outside_listing" "canary.txt"
rm -rf -- "$work"

echo "=== test: uninstall also refuses to touch a symlinked bookkeeping directory ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
outside="$work/outside-target-2"
mkdir -p -- "$outside"
printf 'must never be touched either\n' >"$outside/canary.txt"
rm -rf -- "$home/.local/share/emuwiz-installer"
ln -s -- "$outside" "$home/.local/share/emuwiz-installer"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" >/dev/null 2>&1 || true
canary_content=$(cat "$outside/canary.txt")
assert_contains "the outside canary survives uninstall too" "$canary_content" "must never be touched either"
if [ -L "$home/.local/share/emuwiz-installer" ]; then
    ok "uninstall left the symlinked bookkeeping path exactly as it was"
else
    bad "uninstall left the symlinked bookkeeping path exactly as it was"
fi
# With the manifest unreachable (treated as no manifest), binaries have no
# fallback recognition and are correctly left in place, not removed.
assert_executable "binaries are left in place (manifest unreachable, so they look foreign)" \
    "$bin_dir/emuwiz-cli"
rm -rf -- "$work"

echo "=== test: an existing manifest that is a foreign regular file is left untouched ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share/emuwiz-installer"
bin_dir="$work/bin"

manifest=$(manifest_path "$home/.local/share")
printf 'this file belongs to something else entirely\n' >"$manifest"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install warns that the manifest itself looks foreign" "$warn" \
    "refusing to install managed files"
manifest_content=$(cat "$manifest")
assert_contains "the foreign manifest file content is preserved exactly" \
    "$manifest_content" "this file belongs to something else entirely"
assert_no_such_path "the foreign-manifest refusal leaves the binary unpublished" \
    "$bin_dir/emuwiz-cli"
rm -rf -- "$work"

echo "=== test: an existing manifest that is a symlink is left untouched (never mv'd through) ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share/emuwiz-installer"
bin_dir="$work/bin"

manifest=$(manifest_path "$home/.local/share")
outside_target="$work/manifest-symlink-target"
printf 'must never be overwritten via the manifest symlink\n' >"$outside_target"
ln -s -- "$outside_target" "$manifest"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install warns that the manifest symlink looks foreign" "$warn" \
    "refusing to install managed files"
if [ -L "$manifest" ]; then
    ok "the manifest symlink itself is untouched"
else
    bad "the manifest symlink itself is untouched"
fi
target_content=$(cat "$outside_target")
assert_contains "whatever the manifest symlink pointed at is untouched" \
    "$target_content" "must never be overwritten via the manifest symlink"
assert_no_such_path "the manifest-symlink refusal leaves the binary unpublished" \
    "$bin_dir/emuwiz-cli"
rm -rf -- "$work"

echo "=== test: an existing manifest path that is a directory is left untouched ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share/emuwiz-installer"
bin_dir="$work/bin"

manifest=$(manifest_path "$home/.local/share")
mkdir -p -- "$manifest"
printf 'must never be touched\n' >"$manifest/some-file-inside"

warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "install warns that the manifest-shaped directory looks foreign" "$warn" \
    "refusing to install managed files"
if [ -d "$manifest" ] && [ ! -L "$manifest" ]; then
    ok "the directory occupying the manifest path is untouched"
else
    bad "the directory occupying the manifest path is untouched"
fi
inside_content=$(cat "$manifest/some-file-inside")
assert_contains "contents inside that directory are untouched" \
    "$inside_content" "must never be touched"
assert_no_such_path "the manifest-directory refusal leaves the binary unpublished" \
    "$bin_dir/emuwiz-cli"
rm -rf -- "$work"

echo "=== test: --replace-foreign recovers a foreign manifest location, backing it up first ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home/.local/share/emuwiz-installer"
bin_dir="$work/bin"

manifest=$(manifest_path "$home/.local/share")
printf 'unrelated content that predates ownership tracking\n' >"$manifest"

assert_failure "install without --replace-foreign leaves something unrecorded (exits non-zero)" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
manifest_content=$(cat "$manifest")
assert_contains "the foreign manifest content is still there before --replace-foreign" \
    "$manifest_content" "unrelated content that predates ownership tracking"

replace_warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign 2>&1 1>/dev/null)
assert_contains "install --replace-foreign reports moving the foreign manifest aside" \
    "$replace_warn" "moved an unrecognised path at the ownership manifest location aside"
assert_success "install --replace-foreign succeeds" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign

backup_found=0
for d in "$home/.local/share/emuwiz-installer"/.emuwiz-foreign-backup.*; do
    [ -f "$d/manifest" ] || continue
    backup_found=1
    backup_content=$(cat "$d/manifest")
    assert_contains "the backed-up foreign manifest content is fully recoverable" \
        "$backup_content" "unrelated content that predates ownership tracking"
done
if [ "$backup_found" -eq 1 ]; then
    ok "the foreign manifest was backed up, not deleted"
else
    bad "the foreign manifest was backed up, not deleted"
fi
new_manifest_content=$(cat "$manifest")
assert_contains "a genuine ownership manifest now occupies the path" \
    "$new_manifest_content" "schema_version 2"
assert_success "a further plain reinstall against the new manifest is now silent" \
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
rm -rf -- "$work"

echo "=== test: a symlinked custom XDG_DATA_HOME is honoured, and unrelated canaries there survive ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
real_data_target="$work/real-data-target"
mkdir -p -- "$real_data_target"
printf 'unrelated pre-existing content - must survive\n' >"$real_data_target/unrelated-canary.txt"
data_home_link="$work/symlinked-data-home"
ln -s -- "$real_data_target" "$data_home_link"
bin_dir="$work/bin"

assert_success "install succeeds against a symlinked XDG_DATA_HOME" \
    env HOME="$home" XDG_DATA_HOME="$data_home_link" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
assert_file_exists "the desktop entry landed in the real target the symlink points to" \
    "$real_data_target/applications/io.github.kiehntre.emuwiz.desktop"
assert_file_exists "the manifest landed in the real target the symlink points to" \
    "$real_data_target/emuwiz-installer/manifest"
canary_content=$(cat "$real_data_target/unrelated-canary.txt")
assert_contains "the unrelated pre-existing canary in that real target survives" \
    "$canary_content" "must survive"

assert_success "reinstall against the symlinked XDG_DATA_HOME is silent and clean" \
    env HOME="$home" XDG_DATA_HOME="$data_home_link" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir"
rm -rf -- "$work"

# ===========================================================================
# Manifest parser: fail-closed on any structural ambiguity.
# ===========================================================================

echo "=== test: manifest schema parser rejects structural ambiguity ==="
work=$(mktemp -d)
make_bundle "$work/bundle"
home="$work/home"
mkdir -p -- "$home"
bin_dir="$work/bin"

env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" >/dev/null
manifest=$(manifest_path "$home/.local/share")
cp -- "$manifest" "$work/pristine-manifest"

# assert_manifest_rejected DESCRIPTION - attempts the mutation staged in
# $work/mutated-manifest, asserts publication is refused before any managed
# binary is touched, then reconciles state with --replace-foreign before the
# next sub-test.
assert_manifest_rejected() {
    description=$1
    cp -- "$work/mutated-manifest" "$manifest"
    warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
    assert_contains "$description" "$warn" "refusing to install managed files"
    assert_executable "$description leaves the existing binary in place" \
        "$bin_dir/emuwiz-cli"
    env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign >/dev/null
}

sed '/^schema_version /{p}' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "a duplicate schema_version line invalidates the whole manifest"

sed '/^bin_dir /{p}' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "a duplicate bin_dir line invalidates the whole manifest"

sed '/^data_home /{p}' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "a duplicate data_home line invalidates the whole manifest"

sed '/^bin-emuwiz-cli /{p}' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "a duplicate slot record invalidates the whole manifest"

sed '$d' "$work/pristine-manifest" >"$work/mutated-manifest"
printf 'totally-unknown-key some-value\n' >>"$work/mutated-manifest"
printf 'end\n' >>"$work/mutated-manifest"
assert_manifest_rejected "an unknown field in an otherwise-valid manifest invalidates it"

sed 's/^schema_version .*/schema_version 99/' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "an unsupported schema_version invalidates the manifest"

grep -E '^(schema_version|bin_dir|data_home|record_count) ' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "truncation immediately after a valid header (no records, no end) fails safe"

header_line_count=$(grep -n '^record_count ' "$work/pristine-manifest" | cut -d: -f1)
head -n "$((header_line_count + 5))" "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "truncation between valid slot records (no end, count too high) fails safe"

sed '$d' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "a manifest missing the end marker fails safe"

grep -v '^record_count ' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "a manifest missing record_count fails safe"

sed '/^record_count /{p}' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "a duplicate record_count line invalidates the whole manifest"

sed 's/^record_count 11/record_count 12/' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "an incorrect record_count (too high, otherwise complete) fails safe"

sed 's/^record_count 11/record_count 10/' "$work/pristine-manifest" >"$work/mutated-manifest"
assert_manifest_rejected "an incorrect record_count (too low, otherwise complete) fails safe"

# The parser contract is that nothing follows "end" - not a record, not a
# header field, and not even a comment. Confirmed on both install (the
# manifest must be rejected outright, exactly as if it had never existed)
# and uninstall (no destructive action may be taken on the strength of a
# manifest with trailing content after its own end marker: the binaries,
# which have zero fallback recognition of their own, must be left in
# place rather than removed).
cat -- "$work/pristine-manifest" >"$work/mutated-manifest"
printf '# trailing comment after end - must invalidate the whole manifest\n' >>"$work/mutated-manifest"
assert_manifest_rejected "a comment appended after the end marker invalidates the manifest (install)"

cp -- "$work/pristine-manifest" "$manifest"
printf '# trailing comment after end - must invalidate the whole manifest\n' >>"$manifest"
uninstall_warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --uninstall --prefix "$bin_dir" 2>&1 1>/dev/null) || true
assert_contains "uninstall also rejects a manifest with a trailing comment after end" \
    "$uninstall_warn" "leaving foreign path untouched"
assert_executable "uninstall took no destructive action: the binary is still there (looked foreign, correctly not removed)" \
    "$bin_dir/emuwiz-cli"
assert_executable "uninstall took no destructive action: the other binary is still there too" \
    "$bin_dir/emuwiz"
# Recover before the next sub-test.
cp -- "$work/pristine-manifest" "$manifest"

# A valid, intentionally partial manifest IS supported: omit just the
# bin-emuwiz slot's record (adjusting record_count to match) and confirm
# the manifest as a WHOLE is still accepted - bin-emuwiz-cli, still
# recorded, produces no warning at all, while bin-emuwiz, genuinely
# unrecorded in this otherwise-valid manifest, correctly falls back to "no
# recognition rule for binaries" and is reported foreign - proving the
# manifest parsed and was honoured, not merely rejected wholesale.
grep -v '^bin-emuwiz ' "$work/pristine-manifest" | sed 's/^record_count 11/record_count 10/' \
    >"$work/partial-manifest"
cp -- "$work/partial-manifest" "$manifest"
warn=$(env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
if printf '%s\n' "$warn" | grep -Fxq "install.sh: warning: leaving foreign path untouched (not EmuWiz-owned): $bin_dir/emuwiz"; then
    ok "a valid partial manifest is accepted: the un-recorded binary slot is treated as foreign"
else
    bad "a valid partial manifest is accepted: the un-recorded binary slot is treated as foreign (warn: $warn)"
fi
if printf '%s\n' "$warn" | grep -Fq "$bin_dir/emuwiz-cli"; then
    bad "a valid partial manifest is accepted: the still-recorded slot produces no warning at all (it warned anyway)"
else
    ok "a valid partial manifest is accepted: the still-recorded slot produces no warning at all"
fi
env HOME="$home" sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" --replace-foreign >/dev/null

# A complete, untouched, valid manifest round-trips silently - the
# baseline every mutation above is measured against.
cp -- "$work/pristine-manifest" "$manifest"
complete_err=$(env HOME="$home" PATH="$fake_ratarmount_dir:$PATH" \
    sh "$work/bundle/install.sh" --developer-unchecked-install --prefix "$bin_dir" 2>&1 1>/dev/null) || true
complete_err=$(printf '%s\n' "$complete_err" | strip_dev_warning)
if [ -z "$complete_err" ]; then
    ok "a complete, valid, unmutated manifest is accepted and produces no warnings"
else
    bad "a complete, valid, unmutated manifest is accepted and produces no warnings (got: $complete_err)"
fi
rm -rf -- "$work"

echo
printf 'Results: %s passed, %s failed\n' "$pass_count" "$fail_count"
suite_rc=0
[ "$fail_count" -eq 0 ] || suite_rc=1

# Separate deterministic seams instrument disposable installer copies only.
python3 -B "$repo_root/tests/test_installer_ownership.py" -v || suite_rc=1
suite_role=outer
[ "${TEST_INSTALL_TTY_REGRESSION_GUARD:-}" != 1 ] || suite_role=inner
printf 'Suite outcome: role=%s exit=%s elapsed=%ss\n' "$suite_role" "$suite_rc" "$(( $(date +%s) - suite_started_seconds ))"
exit "$suite_rc"
