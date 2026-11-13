#!/usr/bin/env python3
"""Wait for a GUI condition instead of a fixed `sleep 100`/`sleep 120`.

Returns as soon as EVERY given condition holds, or exits 1 at --timeout. Read-only:
it only looks (xdotool search, a root-window capture, a log file, /proc). It never
clicks, types, signals or writes anything.

  scripts/qa/gui_wait.py --window EmuWiz --stable 3            # window up, screen settled
  scripts/qa/gui_wait.py --log "$S/blue.log" --pattern 'scan finished' --timeout 600
  scripts/qa/gui_wait.py --window-gone 'Organise' --stable 2 --timeout 900
  scripts/qa/gui_wait.py --pid-exit 12345

Conditions (combine freely; all must hold at the same time):
  --window NAME       a visible window whose title matches NAME (xdotool search --name)
  --window-gone NAME  no visible window matches NAME (a dialog or progress window closed)
  --log FILE --pattern REGEX
                      REGEX matches text in FILE; with --new-only only text appended
                      after this helper started counts
  --stable SECONDS    the --window window (or, without --window, the whole screen) has
                      not changed for SECONDS (raw pixels via `xwd`, so an idle window
                      is truly identical). Prefer it with --window: a live desktop
                      rarely holds completely still
  --pid-exit PID      the process has exited

Exit status: 0 met (prints the elapsed seconds), 1 timed out (prints which condition
was still unmet), 2 usage error. DISPLAY comes from the environment or --display.
"""

import argparse
import hashlib
import os
import re
import subprocess
import sys
import time


def _x(argv, display):
    env = dict(os.environ)
    if display:
        env["DISPLAY"] = display
    return subprocess.run(argv, env=env, capture_output=True, timeout=30)


def find_windows(name, display):
    out = _x(["xdotool", "search", "--onlyvisible", "--name", name], display)
    return out.stdout.decode(errors="replace").split()


def capture_screen(display, window=None):
    target = ["-id", window] if window else ["-root"]
    out = _x(["xwd", *target, "-silent"], display)
    if out.returncode != 0:
        if window:
            return None  # the window went away mid-capture: treated as a change
        raise RuntimeError("xwd -root failed: " + out.stderr.decode(errors="replace").strip())
    return hashlib.sha256(out.stdout).hexdigest()


def pid_alive(pid):
    try:
        with open(f"/proc/{pid}/stat") as handle:
            # A zombie has exited even though its /proc entry remains.
            return handle.read().rsplit(")", 1)[1].split()[0] != "Z"
    except (FileNotFoundError, ProcessLookupError, IndexError):
        return False


class Waiter:
    def __init__(self, args, find=find_windows, capture=capture_screen, alive=pid_alive,
                 clock=time.monotonic):
        self.args = args
        self.find, self.capture, self.alive, self.clock = find, capture, alive, clock
        self.pattern = re.compile(args.pattern) if args.pattern else None
        self.log_start = 0
        if args.log and args.new_only:
            try:
                self.log_start = os.path.getsize(args.log)
            except OSError:
                self.log_start = 0
        self.last_frame = None
        self.frame_since = None

    def unmet(self):
        """The first unmet condition's description, or None when all hold now."""
        a = self.args
        unmet = None
        windows = self.find(a.window, a.display) if a.window else []
        if a.window and not windows:
            unmet = unmet or f"no visible window matching {a.window!r}"
        if a.window_gone and self.find(a.window_gone, a.display):
            unmet = unmet or f"a window matching {a.window_gone!r} is still visible"
        if a.log:
            text = ""
            try:
                with open(a.log, "rb") as handle:
                    handle.seek(self.log_start)
                    text = handle.read().decode(errors="replace")
            except OSError:
                pass
            if not self.pattern.search(text):
                unmet = unmet or f"{a.pattern!r} not yet in {a.log}"
        if a.pid_exit is not None and self.alive(a.pid_exit):
            unmet = unmet or f"process {a.pid_exit} is still running"
        if a.stable is not None:
            now = self.clock()
            if a.window:
                frame = self.capture(a.display, windows[0]) if windows else None
            else:
                frame = self.capture(a.display)
            if frame is None or frame != self.last_frame:
                self.last_frame, self.frame_since = frame, now
            if now - self.frame_since < a.stable:
                unmet = unmet or f"screen not yet unchanged for {a.stable:g} s"
        return unmet

    def run(self, sleep=time.sleep):
        start = self.clock()
        while True:
            unmet = self.unmet()
            elapsed = self.clock() - start
            if unmet is None:
                return 0, f"gui_wait: met after {elapsed:.1f} s"
            if elapsed >= self.args.timeout:
                return 1, f"gui_wait: TIMEOUT after {elapsed:.1f} s: {unmet}"
            sleep(self.args.interval)


def parse(argv):
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--window")
    p.add_argument("--window-gone")
    p.add_argument("--log")
    p.add_argument("--pattern")
    p.add_argument("--new-only", action="store_true")
    p.add_argument("--stable", type=float)
    p.add_argument("--pid-exit", type=int)
    p.add_argument("--timeout", type=float, default=120.0)
    p.add_argument("--interval", type=float, default=0.5)
    p.add_argument("--display", default=os.environ.get("DISPLAY", ""))
    args = p.parse_args(argv)
    if bool(args.log) != bool(args.pattern):
        p.error("--log and --pattern go together")
    if not (args.window or args.window_gone or args.log or args.stable is not None
            or args.pid_exit is not None):
        p.error("give at least one condition")
    if args.pattern:
        try:
            re.compile(args.pattern)
        except re.error as exc:
            p.error(f"bad --pattern: {exc}")
    if args.timeout <= 0 or args.interval <= 0 or (args.stable is not None and args.stable < 0):
        p.error("--timeout and --interval must be positive, --stable not negative")
    if (args.window or args.window_gone or args.stable is not None) and not args.display:
        p.error("no DISPLAY (set it or pass --display)")
    return args


def main(argv=None):
    args = parse(sys.argv[1:] if argv is None else argv)
    try:
        rc, message = Waiter(args).run()
    except (RuntimeError, OSError, subprocess.SubprocessError) as exc:
        print(f"gui_wait: error: {exc}", file=sys.stderr)
        return 2
    print(message, file=sys.stderr if rc else sys.stdout)
    return rc


if __name__ == "__main__":
    sys.exit(main())
