#!/usr/bin/env python3
"""What does the process that started `flatpak run` learn about how the
emulator inside ended?

    flatpak_exit_probe.py --work <new-folder> [--only app-id,...]

For each installed Flatpak emulator, in an isolated scratch profile (private
display, throwaway HOME, no devices, no network, no host filesystem):

  1. The app's configured command, from its metadata.
  2. A shell inside the app's sandbox that exits 7, dies of SIGSEGV, dies of
     SIGKILL: what status does `flatpak run` return for each?
  3. The emulator's own binary asked for its version (a real, normal exit of
     the real program): what status comes back?
  4. The emulator started normally. The chain of processes from the one we
     started down to the emulator is written out (pid, parent, name, exe).
     Then the emulator process itself (ours: checked by the sandbox token and
     signalled through a pidfd) is sent SIGSEGV, and in a second run SIGTERM:
     what status does `flatpak run` return?
  5. The launcher itself is stopped while the emulator runs: what do we see,
     and does the emulator go with it?

Nothing is installed or changed. Only processes this probe started are ever
signalled (gui_sandbox ownership rules). Results go to <work>/results.json.
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gui_sandbox  # noqa: E402

REAL_HOME = Path.home()
ONLY_STAGES = set()
APPS = {
    "net.pcsx2.PCSX2": ("pcsx2-qt", ["-version"]),
    "net.rpcs3.RPCS3": ("rpcs3", ["--version"]),
    "org.ppsspp.PPSSPP": ("PPSSPPSDL", ["--version"]),
    "org.libretro.RetroArch": ("retroarch", ["--version"]),
}


def flatpak(sandbox, app, *rest, command=None):
    argv = [
        "flatpak", "run", "--nodevice=all", "--unshare=network", "--nosocket=wayland",
        "--nosocket=pulseaudio", "--nofilesystem=host", "--nofilesystem=home",
        f"--env={gui_sandbox.TOKEN_VARIABLE}={sandbox.token}", "--env=LIBGL_ALWAYS_SOFTWARE=1",
    ]
    if command:
        argv.append(f"--command={command}")
    return argv + [app, *rest]


def extra(sandbox, scratch):
    runtime = Path(sandbox.root.name) / "run"
    runtime.mkdir(mode=0o700, exist_ok=True)
    return {
        "XDG_RUNTIME_DIR": str(runtime),
        "FLATPAK_USER_DIR": str(REAL_HOME / ".local/share/flatpak"),
        "LIBGL_ALWAYS_SOFTWARE": "1",
        "QT_QPA_PLATFORM": "xcb",
        "SDL_AUDIODRIVER": "dummy",
        "TMPDIR": str(scratch),
    }


def status_text(code):
    if code is None:
        return "still running"
    if code < 0:
        return f"killed by signal {-code}"
    if code > 128:
        return f"exit code {code} (= 128 + signal {code - 128})"
    return f"exit code {code}"


def stat_of(pid):
    try:
        text = Path(f"/proc/{pid}/stat").read_text()
        name = text[text.index("(") + 1:text.rindex(")")]
        parent = int(text[text.rindex(")") + 2:].split()[1])
        try:
            exe = os.readlink(f"/proc/{pid}/exe")
        except OSError:
            exe = "?"
        cgroup = Path(f"/proc/{pid}/cgroup").read_text().strip().rsplit("/", 1)[-1]
        return {"pid": pid, "parent": parent, "name": name, "exe": exe, "cgroup": cgroup}
    except (OSError, ValueError):
        return None


def chain_to(pid, top):
    """pid, its parent, ... up to and including `top` (or as far as it goes)."""
    out = []
    while pid and pid != 1:
        info = stat_of(pid)
        if not info:
            break
        out.append(info)
        if pid == top:
            break
        pid = info["parent"]
    return out


def find_emulator(sandbox, binary, seconds=60):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        for pid in sandbox.owned():
            info = stat_of(pid)
            if info and os.path.basename(info["exe"]) == binary:
                return pid
        time.sleep(0.5)
    return None


def run_and_wait(sandbox, argv, env, scratch, name, seconds=60):
    with open(scratch / f"{name}.log", "wb") as log:
        child = sandbox.spawn(argv, env, log)
        try:
            child.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            pass
        return child.returncode


def with_running_emulator(app, binary, scratch, name, act):
    """Starts the app normally, finds the emulator inside, calls act()."""
    result = {}
    try:
        with gui_sandbox.Sandbox() as sandbox:
            sandbox.start_display("1280x800x24")
            with open(scratch / f"{name}.log", "wb") as log:
                launcher = sandbox.spawn(flatpak(sandbox, app), extra(sandbox, scratch), log)
                inner = find_emulator(sandbox, binary)
                if inner is None:
                    result["error"] = f"no {binary} process appeared"
                    result["launcher"] = status_text(launcher.poll())
                    return result
                time.sleep(3)
                result.update(chain=chain_to(inner, launcher.pid), launcher_pid=launcher.pid, emulator_pid=inner)
                result.update(act(sandbox, launcher, inner))
    except gui_sandbox.SandboxError as error:
        # Raised on leaving the sandbox (a helper process lingered): what was
        # observed before that still stands.
        result["sandbox_note"] = str(error)[:200]
    return result


def signal_inner(number):
    def act(sandbox, launcher, inner):
        sandbox._signal(inner, number)  # pidfd + ownership check
        try:
            launcher.wait(timeout=30)
        except subprocess.TimeoutExpired:
            pass
        return {"sent": f"signal {number} to the emulator process", "launcher_status": status_text(launcher.returncode),
                "launcher_code": launcher.returncode}
    return act


def send_close_request(display, window):
    """WM_DELETE_WINDOW, the message a window manager sends when the X is
    clicked. Sent through the X server on the private display only."""
    import ctypes
    import ctypes.util

    class ClientMessage(ctypes.Structure):
        _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                    ("display", ctypes.c_void_p), ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
                    ("format", ctypes.c_int), ("data", ctypes.c_long * 5)]

    class Event(ctypes.Union):
        _fields_ = [("client", ClientMessage), ("pad", ctypes.c_long * 24)]

    x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    handle = x11.XOpenDisplay(display.encode())
    if not handle:
        return "cannot open the private display"
    event = Event()
    event.client.type = 33  # ClientMessage
    event.client.window = window
    event.client.message_type = x11.XInternAtom(handle, b"WM_PROTOCOLS", 0)
    event.client.format = 32
    event.client.data[0] = x11.XInternAtom(handle, b"WM_DELETE_WINDOW", 0)
    x11.XSendEvent(handle, window, 0, 0, ctypes.byref(event))
    x11.XFlush(handle)
    x11.XCloseDisplay(handle)
    return ""


def close_window(sandbox, launcher, inner):
    """Asks the emulator's windows to close, as clicking the X does. This is
    a normal close request delivered through the X server (no input device)."""
    env = dict(os.environ, DISPLAY=sandbox.display)
    env.pop("XAUTHORITY", None)
    asked = []
    for _ in range(3):
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "."],
                               capture_output=True, text=True, env=env, timeout=20)
        for window in found.stdout.split():
            error = send_close_request(sandbox.display, int(window))
            asked.append({"window": window, "rc": 1 if error else 0, "err": error})
        try:
            launcher.wait(timeout=12)
            break
        except subprocess.TimeoutExpired:
            continue  # a "really quit?" window may have appeared: ask that too
    return {"sent": "close request to the emulator's windows", "windows_asked": asked,
            "launcher_status": status_text(launcher.returncode), "launcher_code": launcher.returncode,
            "emulator_still_alive": stat_of(inner) is not None}


def stop_launcher(sandbox, launcher, inner):
    # The launcher is this probe's own direct child, held by its Popen handle
    # (not yet reaped, so its pid cannot belong to anything else). After
    # `flatpak run` becomes bwrap it no longer carries the sandbox token, so
    # the token check cannot be used for it.
    launcher.kill()
    try:
        launcher.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass
    time.sleep(2)
    return {"sent": "SIGKILL to the launcher", "launcher_status": status_text(launcher.returncode),
            "emulator_still_alive_2s_later": stat_of(inner) is not None}


def probe(app, scratch):
    binary, version_args = APPS[app]
    out = {"app": app, "binary": binary}
    meta = subprocess.run(["flatpak", "info", "--show-metadata", app], capture_output=True, text=True)
    if meta.returncode != 0:
        out["outcome"] = "not_installed"
        return out
    out["configured_command"] = next(
        (line.split("=", 1)[1] for line in meta.stdout.splitlines() if line.startswith("command=")), None)
    scratch.mkdir(parents=True)
    with gui_sandbox.Sandbox() as sandbox:
        sandbox.start_display("1280x800x24")
        env = extra(sandbox, scratch)
        shell = {}
        for label, script in [("exit 7", "exit 7"), ("exit 0", "exit 0"), ("SIGSEGV", "kill -SEGV $$"), ("SIGKILL", "kill -KILL $$")]:
            code = run_and_wait(sandbox, flatpak(sandbox, app, "-c", script, command="sh"), env, scratch, f"shell-{label.replace(' ', '')}")
            shell[label] = {"code": code, "text": status_text(code)}
        out["shell_in_sandbox"] = shell
        # What the configured command is, if it is a script.
        peek = subprocess.run(
            flatpak(sandbox, app, "-c", f'c=$(command -v {out["configured_command"]}); echo "$c"; head -c 2 "$c" | od -An -c; head -n 12 "$c" 2>/dev/null | grep -a -n "exec\\|{binary}" | head -5', command="sh"),
            capture_output=True, text=True, env=sandbox.environment(env), timeout=60)
        out["configured_command_peek"] = peek.stdout.strip()[:600]
        code = run_and_wait(sandbox, flatpak(sandbox, app, *version_args, command=binary), env, scratch, "version")
        out["emulator_version_exit"] = {"code": code, "text": status_text(code)}
    stages = [
        ("window_closed_normally", "run-close", close_window),
        ("emulator_sigsegv", "run-segv", signal_inner(signal.SIGSEGV)),
        ("emulator_sigterm", "run-term", signal_inner(signal.SIGTERM)),
        ("launcher_killed", "run-launcher-killed", stop_launcher),
    ]
    for key, name, act in stages:
        if ONLY_STAGES and key not in ONLY_STAGES:
            continue
        holder = {}

        def keep(sandbox, launcher, inner, act=act, holder=holder):
            holder.update(act(sandbox, launcher, inner))
            return holder

        try:
            out[key] = with_running_emulator(app, binary, scratch, name, keep)
        except Exception as error:  # one stage must not lose the others
            out[key] = dict(holder, error=repr(error)[:300])
    out["outcome"] = "done"
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--only", default="")
    parser.add_argument("--stages", default="", help="comma-separated stage names; default all")
    args = parser.parse_args()
    ONLY_STAGES.update(stage for stage in args.stages.split(",") if stage)
    if args.work.exists():
        sys.exit(f"{args.work} already exists; the probe works in a new folder")
    args.work.mkdir(parents=True)
    results = []
    for app in APPS:
        if args.only and app not in args.only.split(","):
            continue
        try:
            result = probe(app, args.work / app)
        except Exception as error:
            result = {"app": app, "outcome": "error", "detail": repr(error)[:400]}
        results.append(result)
        print(json.dumps(result, indent=1), flush=True)
    (args.work / "results.json").write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()
