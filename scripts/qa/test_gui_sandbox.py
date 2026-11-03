#!/usr/bin/env python3
"""Proves the sandbox's limits using decoys. Never signals anything it did not start."""

import os
import re
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import gui_sandbox  # noqa: E402

REPO = Path(__file__).resolve().parents[2]


def write_decoy(path):
    """A one-process Python script: /proc/<pid>/comm is its file name.

    Copies of `sleep` cannot be renamed: ubuntu-26.04's coreutils is a
    multi-call binary that picks the program from the executed file name.
    """
    path.write_text(f"#!{sys.executable} -I\nimport time\ntime.sleep(60)\n")
    path.chmod(0o755)
    return path


def decoy_named(directory, name):
    """An unrelated process whose name is a real desktop's, run by this test."""
    path = write_decoy(Path(directory) / name)
    return subprocess.Popen([str(path)], start_new_session=True)


class Limits(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.decoy = decoy_named(self.directory.name, "xfwm4")
        self.addCleanup(self.decoy.wait)
        self.addCleanup(self.decoy.terminate)  # exact child handle

    def alive(self, process):
        return process.poll() is None

    def test_a_same_named_process_outside_the_sandbox_survives(self):
        with self.assertRaises(gui_sandbox.SandboxError):
            # The decoy started after the snapshot, so stop reports the change.
            with gui_sandbox.Sandbox() as sandbox:
                late = decoy_named(self.directory.name, "xfdesktop")
                self.addCleanup(late.wait)
                self.addCleanup(late.terminate)
                child = decoy_named_in(sandbox, self.directory.name, "xfwm4")
        self.assertIsNotNone(child.poll(), "owned process stopped")
        self.assertTrue(self.alive(self.decoy), "the unowned xfwm4 was touched")
        self.assertTrue(self.alive(late))

    def test_only_token_holders_are_owned_and_descendants_are_included(self):
        with gui_sandbox.Sandbox() as sandbox:
            shell = sandbox.spawn(["sh", "-c", "sleep 60 & wait"])
            deadline = time.monotonic() + 5
            while len(sandbox.owned()) < 2 and time.monotonic() < deadline:
                time.sleep(0.01)
            owned = sandbox.owned()
            self.assertIn(shell.pid, owned)
            self.assertEqual(len(owned), 2)
            self.assertNotIn(self.decoy.pid, owned)
            self.assertNotIn(os.getpid(), owned)
        self.assertEqual(sandbox.owned(), [])

    def test_a_daemonised_descendant_is_still_stopped(self):
        with gui_sandbox.Sandbox() as sandbox:
            sandbox.spawn(["sh", "-c", "setsid sleep 60 & exit 0"])
            deadline = time.monotonic() + 5
            while not sandbox.owned() and time.monotonic() < deadline:
                time.sleep(0.01)
        self.assertEqual(sandbox.owned(), [])

    def test_signalling_anything_unowned_is_refused(self):
        with gui_sandbox.Sandbox() as sandbox:
            with self.assertRaises(gui_sandbox.SandboxError):
                sandbox._signal(self.decoy.pid, 15)
            with self.assertRaises(gui_sandbox.SandboxError):
                sandbox._signal(1, 15)
            with self.assertRaises(gui_sandbox.SandboxError):
                sandbox._signal(os.getpid(), 15)
        self.assertTrue(self.alive(self.decoy))

    def test_a_spawned_child_is_owned_as_soon_as_spawn_returns(self):
        with gui_sandbox.Sandbox() as sandbox:
            for _ in range(50):
                child = sandbox.spawn(["sleep", "60"])
                # Read at once: a scan of all of /proc is slow enough to hide the race.
                self.assertTrue(sandbox._signal(child.pid, 9))
                child.wait()

    def test_a_reused_pid_is_not_signalled(self):
        with gui_sandbox.Sandbox() as sandbox:
            child = sandbox.spawn(["sleep", "60"])
            pid = child.pid
            sandbox._signal(pid, 9)
            child.wait()
            # The kernel may hand this number to anyone now; it holds no token.
            self.assertFalse(sandbox._signal(pid, 15) and pid in sandbox.owned())

    def test_a_pid_reused_between_listing_and_signalling_does_not_stop_the_shutdown(self):
        # Deterministic stand-in for the race: stop() lists a PID as ours, it
        # exits and the kernel gives the number to an unrelated process (the
        # decoy) before the pidfd opens. That PID must be skipped, never
        # signalled, and every process still ours must be stopped.
        sandbox = gui_sandbox.Sandbox()
        listed = sandbox.owned
        child = None
        try:
            child = sandbox.spawn(["sleep", "60"])
            calls = []

            def owned_with_a_reused_pid():
                calls.append(None)
                current = listed()
                return [self.decoy.pid, *current] if len(calls) == 1 else current

            sandbox.owned = owned_with_a_reused_pid
            sandbox.stop()
            self.assertIsNotNone(child.poll(), "the owned process was stopped")
            self.assertTrue(self.alive(self.decoy), "the reused PID was not signalled")
        finally:
            sandbox.owned = listed
            if child is not None and child.poll() is None:
                sandbox._signal(child.pid, 9)
                child.wait()
            sandbox.root.cleanup()

    def test_children_cannot_reach_the_session_bus_or_session_manager(self):
        names = {"DBUS_SESSION_BUS_ADDRESS": "x", "SESSION_MANAGER": "y", "XAUTHORITY": "z"}
        previous = {name: os.environ.get(name) for name in names}
        os.environ.update(names)
        try:
            with gui_sandbox.Sandbox() as sandbox:
                env = sandbox.environment()
        finally:
            for name, value in previous.items():
                os.environ.pop(name, None) if value is None else os.environ.update({name: value})
        for name in names:
            self.assertNotIn(name, env)
        self.assertEqual(env["HOME"], str(sandbox.home))

    def test_a_desktop_display_is_refused(self):
        with gui_sandbox.Sandbox() as sandbox:
            with self.assertRaises(gui_sandbox.SandboxError):
                sandbox.start_window_manager()

    def test_desktop_snapshot_finds_a_process_by_identity_including_start_time(self):
        before = gui_sandbox.desktop_snapshot()
        self.assertTrue(any(entry[0] == self.decoy.pid for entry in before))
        entry = next(entry for entry in before if entry[0] == self.decoy.pid)
        self.assertEqual(entry[2], "xfwm4")
        self.assertIsNotNone(entry[1])


def decoy_named_in(sandbox, directory, name):
    target = Path(directory) / "owned" / name
    target.parent.mkdir(exist_ok=True)
    if not target.exists():
        write_decoy(target)
    return sandbox.spawn([str(target)])


# Dispatches on its own executed file name, like ubuntu-26.04's coreutils.
FILE_NAME_DISPATCHING_SLEEP = """#!/bin/sh
case "${0##*/}" in
  sleep) exec /bin/sleep "$@" ;;
  *) echo "coreutils: unknown program '${0##*/}'" >&2; exit 1 ;;
esac
"""


class DecoyIdentity(unittest.TestCase):
    """The decoys must not depend on how the host's sleep picks its program."""

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.directory = Path(directory.name)
        bin_dir = self.directory / "bin"
        bin_dir.mkdir()
        (bin_dir / "sleep").write_text(FILE_NAME_DISPATCHING_SLEEP)
        (bin_dir / "sleep").chmod(0o755)
        original = os.environ["PATH"]
        os.environ["PATH"] = f"{bin_dir}{os.pathsep}{original}"
        self.addCleanup(os.environ.__setitem__, "PATH", original)

    def assert_lone_decoy(self, process, name):
        time.sleep(0.3)
        self.assertIsNone(process.poll(), f"the {name} decoy exited at once")
        proc = Path("/proc") / str(process.pid)
        self.assertEqual((proc / "comm").read_text().strip(), name)
        self.assertEqual(os.path.realpath(proc / "exe"), os.path.realpath(sys.executable))
        self.assertEqual((proc / "task" / str(process.pid) / "children").read_text().strip(), "")

    def test_decoys_live_alone_under_their_desktop_names_and_stop_on_sigterm(self):
        decoy = decoy_named(self.directory, "xfwm4")
        self.addCleanup(decoy.wait)
        self.addCleanup(decoy.terminate)  # exact child handle
        self.assert_lone_decoy(decoy, "xfwm4")
        with gui_sandbox.Sandbox() as sandbox:
            owned = decoy_named_in(sandbox, self.directory, "xfdesktop")
            self.assert_lone_decoy(owned, "xfdesktop")
        self.assertIsNotNone(owned.poll(), "the sandbox stopped its own decoy")
        self.assertIsNone(decoy.poll(), "the unowned decoy was touched")
        decoy.terminate()
        self.assertEqual(decoy.wait(timeout=5), -15)


class Source(unittest.TestCase):
    def test_the_sandbox_has_no_unchecked_way_to_signal(self):
        source = (Path(__file__).parent / "gui_sandbox.py").read_text()
        code = re.sub(r'""".*?"""', "", source, flags=re.S)
        code = re.sub(r"#.*", "", code)
        for banned in ("os.kill", "killpg", ".terminate(", ".kill(", "shell=True"):
            self.assertNotIn(banned, code)
        self.assertEqual(code.count("pidfd_send_signal"), 1)

    def test_no_tracked_script_stops_processes_by_name(self):
        tracked = subprocess.run(
            ["git", "-C", str(REPO), "ls-files"], capture_output=True, text=True, check=True
        ).stdout.split()
        pattern = re.compile(r"^(?!\s*#).*\b(pkill|killall|xkill)\s", re.M)
        offenders = []
        for name in tracked:
            path = REPO / name
            if path.suffix not in {"", ".sh", ".py", ".yml", ".yaml"} or not path.is_file():
                continue
            if path.name == Path(__file__).name:
                continue
            try:
                if pattern.search(path.read_text()):
                    offenders.append(name)
            except (UnicodeDecodeError, OSError):
                continue
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
