#!/usr/bin/env python3
"""A private X display for GUI checks that can only ever stop its own processes.

Why this exists: a test teardown once selected `xfwm4` by name on a machine
with a live XFCE desktop and killed the owner's window manager. Selecting a
process by name, by a bare PID or by process group cannot prove ownership.

The rules here, which test_gui_sandbox.py enforces:

* Every process is started by `spawn`, in its own session, with a random token
  in its environment. A process counts as owned only if `/proc/<pid>/environ`
  holds that exact token. Descendants inherit it; nothing else can have it.
* `spawn` returns only once the child's `/proc/<pid>/environ` shows the token.
  Right after fork the kernel can still show the parent's environment, or an
  empty one, so a just-started child is briefly unprovable.
* Signals go only through a pidfd, opened first and then checked, so a reused
  PID can never be hit. `os.kill` and `os.killpg` are not used at all.
* The display comes from `Xvfb -displayfd` and is refused if it is the one this
  process inherited. Children get a minimal environment with no session bus or
  session manager, so a window manager here cannot join the real desktop.
* `stop` checks that the desktop's processes are the same before and after,
  and raises if any of them changed.
"""

import os
import select
import signal
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

TOKEN_VARIABLE = "EMUWIZ_GUI_SANDBOX_TOKEN"
# Read-only watch list: compared, never signalled.
DESKTOP_COMMANDS = {
    "Xorg",
    "lightdm",
    "sunshine",
    "xfce4-panel",
    "xfce4-session",
    "xfdesktop",
    "xfsettingsd",
    "xfwm4",
}
# Children inherit only these. In particular not DBUS_SESSION_BUS_ADDRESS,
# SESSION_MANAGER or XAUTHORITY.
INHERITED_ENVIRONMENT = ("LANG", "LC_ALL", "PATH", "RUST_LOG", "TERM")
STOP_TIMEOUT_SECONDS = 5.0
SPAWN_TIMEOUT_SECONDS = 5.0


class SandboxError(RuntimeError):
    pass


def _environ(pid, proc):
    try:
        return (proc / str(pid) / "environ").read_bytes().split(b"\0")
    except OSError:
        return []


def _start_time(pid, proc):
    """Field 22 of stat; the command name before it may contain spaces."""
    try:
        text = (proc / str(pid) / "stat").read_text()
    except OSError:
        return None
    return text[text.rindex(")") + 2 :].split()[19]


def _command(pid, proc):
    try:
        return (proc / str(pid) / "comm").read_text().strip()
    except OSError:
        return None


def _pids(proc):
    return [int(entry.name) for entry in proc.iterdir() if entry.name.isdigit()]


def desktop_snapshot(proc=Path("/proc"), token=None):
    """(pid, start time, command) of desktop processes this sandbox does not own."""
    marker = f"{TOKEN_VARIABLE}={token}".encode() if token else None
    found = set()
    for pid in _pids(proc):
        command = _command(pid, proc)
        if command in DESKTOP_COMMANDS and not (marker and marker in _environ(pid, proc)):
            found.add((pid, _start_time(pid, proc), command))
    return found


class Sandbox:
    def __init__(self, proc="/proc"):
        self.proc = Path(proc)
        self.token = uuid.uuid4().hex
        self.marker = f"{TOKEN_VARIABLE}={self.token}".encode()
        self.root = tempfile.TemporaryDirectory(prefix="emuwiz-gui-sandbox-")
        self.home = Path(self.root.name) / "home"
        self.home.mkdir()
        self.display = None
        self.children = []
        self.desktop = desktop_snapshot(self.proc)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        try:
            self.stop()
        finally:
            self.root.cleanup()

    def environment(self, extra=None):
        env = {key: os.environ[key] for key in INHERITED_ENVIRONMENT if key in os.environ}
        share = self.home / ".local/share"
        env.update(
            HOME=str(self.home),
            XDG_CACHE_HOME=str(self.home / ".cache"),
            XDG_CONFIG_HOME=str(self.home / ".config"),
            XDG_DATA_HOME=str(share),
            XDG_STATE_HOME=str(self.home / ".local/state"),
        )
        if self.display:
            env["DISPLAY"] = self.display
        env.update(extra or {})
        env[TOKEN_VARIABLE] = self.token
        return env

    def spawn(self, argv, extra_env=None, log=subprocess.DEVNULL):
        child = subprocess.Popen(
            argv,
            env=self.environment(extra_env),
            start_new_session=True,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
        )
        self.children.append(child)
        self._await_token(child)
        return child

    def _await_token(self, child):
        """Wait until the child is provably ours, or has already exited."""
        deadline = time.monotonic() + SPAWN_TIMEOUT_SECONDS
        while self.marker not in _environ(child.pid, self.proc):
            if child.poll() is not None:
                return
            if time.monotonic() >= deadline:
                raise SandboxError(f"child {child.pid} never showed this sandbox's token")
            time.sleep(0.001)

    def start_display(self, size="1600x900x24"):
        read_end, write_end = os.pipe()
        try:
            child = subprocess.Popen(
                ["Xvfb", "-displayfd", str(write_end), "-screen", "0", size, "-nolisten", "tcp"],
                env=self.environment(),
                pass_fds=(write_end,),
                start_new_session=True,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            self.children.append(child)
            self._await_token(child)
            os.close(write_end)
            write_end = None
            if not select.select([read_end], [], [], 10)[0]:
                raise SandboxError("Xvfb did not report a display")
            number = os.read(read_end, 32).decode().strip()
        finally:
            os.close(read_end)
            if write_end is not None:
                os.close(write_end)
        display = f":{number}"
        inherited = os.environ.get("DISPLAY", "").split(".")[0]
        if not number or display == inherited or number == "0":
            raise SandboxError(f"refusing display {display!r}: it may be a real desktop")
        self.display = display
        return display

    def start_window_manager(self):
        if not self.display:
            raise SandboxError("start the private display first")
        return self.spawn(["xfwm4", "--display", self.display, "--compositor=off"])

    def owned(self):
        """Every live process carrying this sandbox's exact token."""
        return [pid for pid in _pids(self.proc) if self.marker in _environ(pid, self.proc)]

    def _signal(self, pid, number):
        """Pidfd first, ownership second: the checked process is the signalled one."""
        if pid <= 1 or pid == os.getpid():
            raise SandboxError(f"refusing to signal {pid}")
        try:
            pidfd = os.pidfd_open(pid)
        except ProcessLookupError:
            return False
        try:
            if self.marker not in _environ(pid, self.proc):
                raise SandboxError(f"refusing to signal {pid}: it is not ours")
            signal.pidfd_send_signal(pidfd, number)
            return True
        except ProcessLookupError:
            return False
        finally:
            os.close(pidfd)

    def _reap(self):
        for child in self.children:
            child.poll()

    def stop(self):
        for number in (signal.SIGTERM, signal.SIGKILL):
            for pid in self.owned():
                self._signal(pid, number)
            deadline = time.monotonic() + STOP_TIMEOUT_SECONDS
            while time.monotonic() < deadline:
                self._reap()
                if not self.owned():
                    break
                time.sleep(0.02)
            if not self.owned():
                break
        self._reap()
        left = self.owned()
        changed = self.desktop ^ desktop_snapshot(self.proc, self.token)
        if left:
            raise SandboxError(f"sandbox processes still running: {left}")
        if changed:
            raise SandboxError(f"desktop processes changed during this run: {sorted(changed)}")
