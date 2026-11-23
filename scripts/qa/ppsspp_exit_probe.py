#!/usr/bin/env python3
"""What exit status does the Flatpak PPSSPP give when a running game is
closed in ordinary ways, and when it is ended abnormally on purpose?

    ppsspp_exit_probe.py --work <new-folder> --game <game.iso> [--repeats 3]

Deliberately small. One game, one install route (Flatpak), four ways of
ending, a few repeats each:

  window-close   the window's close request (clicking the X)
  quit-key       PPSSPP's own "Exit App" action, bound to a key in the
                 scratch profile and typed through the X server
  terminate      a polite stop request (SIGTERM) to the emulator process
  fault          a deliberate abnormal end (SIGSEGV) to the emulator process

The game file is never opened from where it lives: a disposable copy is made
inside the work folder and only that copy is given to the sandbox, read-only.
PPSSPP runs with a throwaway HOME, no devices, no network and no access to
host files, so its settings and saves are the scratch ones.

For every run the probe records the exit status EmuWiz would see, whether
the process had to be given up on (hung), what the log's last lines say, and
a screenshot taken just before the request, so "a game was running" can be
checked by eye rather than assumed. It records what happened; it draws no
conclusion about crashes.

Only processes this probe started are ever signalled (gui_sandbox ownership).
"""

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gui_sandbox  # noqa: E402
from flatpak_exit_probe import send_close_request  # noqa: E402

REAL_HOME = Path.home()
APP = "org.ppsspp.PPSSPP"
METHODS = ["window-close", "quit-key", "terminate", "fault"]

# Scratch profile: bind "Exit App" to the Q key (device 1 = keyboard,
# Android key code 45 = Q). If this PPSSPP names the action differently the
# key does nothing, and the run is recorded as "request had no effect".
CONTROLS = "[ControlMapping]\nExit App = 1-45\n"
SETTINGS = (
    "[General]\nFirstRun = False\nCheckForNewVersion = False\nAutoRun = True\n"
    "[Graphics]\nGraphicsBackend = 0 (OPENGL)\nFullScreen = False\n"
    "[Sound]\nEnable = False\n"
)


def x_env(sandbox):
    env = dict(os.environ, DISPLAY=sandbox.display)
    env.pop("XAUTHORITY", None)
    return env


def windows(sandbox):
    found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "."],
                           capture_output=True, text=True, env=x_env(sandbox), timeout=20)
    return [int(line) for line in found.stdout.split()]


def emulator_pid(sandbox):
    for pid in sandbox.owned():
        try:
            if os.path.basename(os.readlink(f"/proc/{pid}/exe")) == "PPSSPPSDL":
                return pid
        except OSError:
            continue
    return None


def one_run(method, work, index, game_copy):
    name = f"{method}-{index}"
    scratch = work / name
    scratch.mkdir(parents=True)
    result = {"method": method, "run": index}
    with gui_sandbox.Sandbox() as sandbox:
        sandbox.start_display("1024x768x24")
        system = sandbox.home / ".var/app" / APP / "config/ppsspp/PSP/SYSTEM"
        system.mkdir(parents=True)
        (system / "ppsspp.ini").write_text(SETTINGS)
        (system / "controls.ini").write_text(CONTROLS)
        runtime = Path(sandbox.root.name) / "run"
        runtime.mkdir(mode=0o700)
        extra = {
            "XDG_RUNTIME_DIR": str(runtime),
            "FLATPAK_USER_DIR": str(REAL_HOME / ".local/share/flatpak"),
            "LIBGL_ALWAYS_SOFTWARE": "1",
            "SDL_AUDIODRIVER": "dummy",
        }
        argv = [
            "flatpak", "run", "--nodevice=all", "--unshare=network", "--nosocket=wayland",
            "--nosocket=pulseaudio", "--nofilesystem=host", "--nofilesystem=home",
            f"--filesystem={game_copy.parent}:ro",
            f"--env={gui_sandbox.TOKEN_VARIABLE}={sandbox.token}", "--env=LIBGL_ALWAYS_SOFTWARE=1",
            APP, str(game_copy),
        ]
        log = open(scratch / "emulator.log", "wb")
        launcher = subprocess.Popen(
            argv, env=sandbox.environment(extra), start_new_session=True,
            stdin=subprocess.DEVNULL, stdout=log, stderr=log,
        )
        sandbox.children.append(launcher)
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline and not windows(sandbox) and launcher.poll() is None:
            time.sleep(0.5)
        time.sleep(20)  # let the game get past its loading screens
        result["started"] = launcher.poll() is None
        subprocess.run(["import", "-window", "root", str(scratch / "before-request.png")],
                       env=x_env(sandbox), timeout=30, capture_output=True)
        inner = emulator_pid(sandbox)
        began = time.monotonic()
        note = ""
        try:
            if not result["started"]:
                note = "ended before it could be closed"
            elif method == "window-close":
                for window in windows(sandbox):
                    send_close_request(sandbox.display, window)
            elif method == "quit-key":
                for window in windows(sandbox)[:1]:
                    subprocess.run(["xdotool", "mousemove", "--window", str(window), "40", "40"],
                                   env=x_env(sandbox), timeout=20)
                subprocess.run(["xdotool", "key", "q"], env=x_env(sandbox), timeout=20)
            elif inner is None:
                note = "emulator process not found"
            elif method == "terminate":
                sandbox._signal(inner, signal.SIGTERM)
            elif method == "fault":
                sandbox._signal(inner, signal.SIGSEGV)
        except (OSError, gui_sandbox.SandboxError) as error:
            note = f"could not deliver the request: {error!r}"[:160]
        hung = False
        try:
            launcher.wait(timeout=40)
        except subprocess.TimeoutExpired:
            hung = True
        result["seconds_to_end"] = round(time.monotonic() - began, 1)
        result["still_running_after_40s"] = hung
        result["exit_code"] = launcher.returncode
        result["note"] = note
        log.close()
        saves = sandbox.home / ".var/app" / APP / "config/ppsspp/PSP/SAVEDATA"
        result["save_folders"] = sorted(p.name for p in saves.iterdir()) if saves.is_dir() else []
        try:
            sandbox.stop()
        except gui_sandbox.SandboxError as error:
            result["sandbox_note"] = str(error)[:160]
    text = (scratch / "emulator.log").read_text(errors="replace")
    result["log_tail"] = [line[:150] for line in text.strip().splitlines()[-3:]]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--game", required=True, type=Path)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--methods", default=",".join(METHODS))
    args = parser.parse_args()
    if args.work.exists():
        sys.exit(f"{args.work} already exists; the probe works in a new folder")
    if not args.game.is_file():
        sys.exit(f"{args.game} is not a file")
    args.work.mkdir(parents=True)
    content = args.work / "content"
    content.mkdir()
    game_copy = content / "probe-game.iso"
    shutil.copyfile(args.game, game_copy)  # the original is only read, once
    game_copy.chmod(0o444)
    results = []
    for method in args.methods.split(","):
        for index in range(1, args.repeats + 1):
            try:
                result = one_run(method, args.work, index, game_copy)
            except Exception as error:
                result = {"method": method, "run": index, "probe_error": repr(error)[:200]}
            results.append(result)
            print(f"{method:<13} #{index}  exit={result.get('exit_code')}  "
                  f"still-running={result.get('still_running_after_40s')}  {result.get('note', '')}{result.get('probe_error', '')}",
                  flush=True)
            (args.work / "results.json").write_text(json.dumps(results, indent=1))
    sys.exit(0)


if __name__ == "__main__":
    main()
