#!/usr/bin/env python3
"""Unit tests for gui_wait.py with a fake clock, fake windows and fake screen captures."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import gui_wait  # noqa: E402


class Clock:
    def __init__(self):
        self.t = 0.0

    def __call__(self):
        return self.t

    def sleep(self, seconds):
        self.t += seconds


def waiter(argv, find=None, capture=None, alive=None):
    clock = Clock()
    args = gui_wait.parse(["--display", ":99", *argv])
    w = gui_wait.Waiter(args, find=find or (lambda n, d: []), capture=capture or (lambda d, w=None: "x"),
                        alive=alive or (lambda p: False), clock=clock)
    return w, clock


class GuiWaitTests(unittest.TestCase):
    def test_window_appears_returns_early(self):
        w, clock = waiter(["--window", "EmuWiz"],
                          find=lambda n, d: ["42"] if clock.t >= 3 else [])
        rc, msg = w.run(sleep=clock.sleep)
        self.assertEqual(rc, 0, msg)
        self.assertLess(clock.t, 4)

    def test_timeout_names_the_unmet_condition(self):
        w, clock = waiter(["--window", "EmuWiz", "--timeout", "5"])
        rc, msg = w.run(sleep=clock.sleep)
        self.assertEqual(rc, 1)
        self.assertIn("EmuWiz", msg)
        self.assertGreaterEqual(clock.t, 5)
        self.assertLess(clock.t, 6)

    def test_window_gone(self):
        w, clock = waiter(["--window-gone", "Organise"],
                          find=lambda n, d: [] if clock.t >= 2 else ["7"])
        self.assertEqual(w.run(sleep=clock.sleep)[0], 0)

    def test_stable_waits_for_the_screen_to_stop_changing(self):
        # The screen changes every tick until t=4, then stays the same.
        w, clock = waiter(["--stable", "2"], capture=lambda d, w=None: str(min(clock.t, 4.0)))
        rc, _ = w.run(sleep=clock.sleep)
        self.assertEqual(rc, 0)
        self.assertGreaterEqual(clock.t, 6)
        self.assertLess(clock.t, 7)

    def test_all_conditions_must_hold_together(self):
        w, clock = waiter(["--window", "EmuWiz", "--stable", "1"],
                          find=lambda n, d: ["1"] if clock.t >= 5 else [],
                          capture=lambda d, w=None: "same" if w == "1" else None)
        rc, _ = w.run(sleep=clock.sleep)
        self.assertEqual(rc, 0)
        self.assertGreaterEqual(clock.t, 5)

    def test_stable_with_window_measures_only_that_window(self):
        seen = []
        w, clock = waiter(["--window", "EmuWiz", "--stable", "1"], find=lambda n, d: ["0x2a"],
                          capture=lambda d, win=None: seen.append(win) or "same")
        self.assertEqual(w.run(sleep=clock.sleep)[0], 0)
        self.assertTrue(seen and set(seen) == {"0x2a"})

    def test_log_pattern_and_new_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "blue.log"
            log.write_text("old: scan finished\n")
            w, clock = waiter(["--log", str(log), "--pattern", "scan finished"])
            self.assertEqual(w.run(sleep=clock.sleep)[0], 0)
            self.assertEqual(clock.t, 0)
            w, clock = waiter(["--log", str(log), "--pattern", "scan finished", "--new-only",
                               "--timeout", "3"])
            self.assertEqual(w.run(sleep=clock.sleep)[0], 1, "old text must not count")
            w, clock = waiter(["--log", str(log), "--pattern", "scan finished", "--new-only"])
            with log.open("a") as handle:
                handle.write("new: scan finished\n")
            self.assertEqual(w.run(sleep=clock.sleep)[0], 0)

    def test_missing_log_is_unmet_not_an_error(self):
        w, clock = waiter(["--log", "/nonexistent/blue.log", "--pattern", "x", "--timeout", "1"])
        self.assertEqual(w.run(sleep=clock.sleep)[0], 1)

    def test_pid_exit(self):
        w, clock = waiter(["--pid-exit", "123"], alive=lambda p: clock.t < 2)
        self.assertEqual(w.run(sleep=clock.sleep)[0], 0)

    def test_real_pid_alive(self):
        proc = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        try:
            self.assertTrue(gui_wait.pid_alive(proc.pid))
        finally:
            proc.kill()
            proc.wait()
        self.assertFalse(gui_wait.pid_alive(proc.pid))

    def test_usage_errors(self):
        for argv in ([], ["--log", "f"], ["--pattern", "x"], ["--window", "a", "--timeout", "0"],
                     ["--log", "f", "--pattern", "("]):
            with self.assertRaises(SystemExit) as cm:
                gui_wait.parse(["--display", ":99", *argv])
            self.assertEqual(cm.exception.code, 2, argv)
        with self.assertRaises(SystemExit):
            gui_wait.parse(["--display", "", "--window", "EmuWiz"])

    def test_never_acts_on_the_gui(self):
        source = (Path(__file__).parent / "gui_wait.py").read_text()
        for verb in ("mousemove", "click", "key", "type", "windowkill", "kill(", "os.remove", "unlink"):
            self.assertNotIn(f'"{verb}' if verb.isalpha() else verb, source, verb)


if __name__ == "__main__":
    unittest.main()
