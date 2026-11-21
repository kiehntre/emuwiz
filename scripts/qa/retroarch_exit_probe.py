#!/usr/bin/env python3
"""How does RetroArch end when a real game is closed in the ordinary ways,
and what exit status does EmuWiz get to see?

    retroarch_exit_probe.py --work <new-folder> [--repeats 3] [--only native,flatpak]

A tiny synthetic Game Boy program is generated here (no real game is used).
It turns on the cartridge's battery memory, adds one to its first byte and
then idles, so every run performs a real emulated save write and RetroArch
writes a real `.srm` on exit. It runs in the Gambatte core.

For the native RetroArch (started directly, inside bwrap with no devices and
the real home read-only) and the Flatpak RetroArch (throwaway HOME, no
devices, no network, no host files), each way of closing is tried several
times:

  quit-command    RetroArch's own QUIT command (what the menu's Quit does)
  quit-key        the in-game quit key (Escape), typed through the X server
  window-close    the window's close request (clicking the X)
  frame-limit     RetroArch stops by itself after a number of frames
  terminate       a polite stop request (SIGTERM) to the emulator process
  failed-state    quit with "save state on exit" on and an unwritable state
                  folder (the case another session saw hang)

For every run: the exit status EmuWiz would see, how long closing took,
whether the process had to be given up on (hung), whether the log shows a
fault, and whether the save file was written and what its first byte is.

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

REAL_HOME = Path.home()
APP = "org.libretro.RetroArch"
NATIVE = "/usr/games/retroarch"
NATIVE_CORE = Path("/usr/lib/x86_64-linux-gnu/libretro/gambatte_libretro.so")
FLATPAK_CORE = REAL_HOME / ".var/app" / APP / "config/retroarch/cores/gambatte_libretro.so"
METHODS = ["quit-command", "quit-key", "window-close", "frame-limit", "terminate", "failed-state"]


def synthetic_rom():
    """32 KiB Game Boy image: MBC1 + RAM + battery; bumps the first save byte."""
    rom = bytearray(0x8000)
    rom[0x100:0x104] = bytes([0x00, 0xC3, 0x50, 0x01])  # nop; jp 0x0150
    rom[0x134:0x13F] = b"EMUWIZPROBE"
    rom[0x147] = 0x03  # MBC1 + RAM + battery
    rom[0x148] = 0x00  # 32 KiB ROM
    rom[0x149] = 0x02  # 8 KiB RAM
    program = bytes([
        0x3E, 0x0A, 0xEA, 0x00, 0x00,  # ld a,0x0a ; ld (0x0000),a   enable RAM
        0xFA, 0x00, 0xA0,              # ld a,(0xa000)
        0x3C,                          # inc a
        0xEA, 0x00, 0xA0,              # ld (0xa000),a               the save write
        0x18, 0xFE,                    # jr -2                       idle
    ])
    rom[0x150:0x150 + len(program)] = program
    checksum = 0
    for byte in rom[0x134:0x14D]:
        checksum = (checksum - byte - 1) & 0xFF
    rom[0x14D] = checksum
    return bytes(rom)


def config_text(saves, states, cores, auto_state):
    settings = {
        "stdin_cmd_enable": "true",
        "network_cmd_enable": "false",
        "quit_press_twice": "false",
        "input_exit_emulator": "escape",
        "input_driver": "x",
        "video_driver": "gl",
        "video_fullscreen": "false",
        "audio_enable": "false",
        "menu_driver": "rgui",
        "pause_nonactive": "false",
        "config_save_on_exit": "false",
        "savefile_directory": str(saves),
        "savestate_directory": str(states),
        "libretro_directory": str(cores),
        "sort_savefiles_enable": "false",
        "sort_savestates_enable": "false",
        "savestate_auto_save": "true" if auto_state else "false",
        "savestate_auto_load": "false",
    }
    return "".join(f'{key} = "{value}"\n' for key, value in settings.items())


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
            if os.path.basename(os.readlink(f"/proc/{pid}/exe")) == "retroarch":
                return pid
        except OSError:
            continue
    return None


def one_run(route, method, work, index):
    name = f"{route}-{method}-{index}"
    scratch = work / name
    scratch.mkdir(parents=True)
    result = {"route": route, "method": method, "run": index}
    with gui_sandbox.Sandbox() as sandbox:
        sandbox.start_display("1024x768x24")
        home = sandbox.home
        if route == "flatpak":
            base = home / ".var/app" / APP / "config/retroarch"
            cores = base / "cores"
            cores.mkdir(parents=True)
            shutil.copy(FLATPAK_CORE, cores / "gambatte_libretro.so")
        else:
            base = scratch / "profile"
            cores = NATIVE_CORE.parent
        saves, states, content = base / "saves", base / "states", base / "content"
        for folder in (saves, content):
            folder.mkdir(parents=True, exist_ok=True)
        if method == "failed-state":
            states.write_text("not a folder: a save state cannot be written here\n")
        else:
            states.mkdir(parents=True, exist_ok=True)
        rom = content / "emuwiz-probe.gb"
        rom.write_bytes(synthetic_rom())
        config = base / "retroarch.cfg"
        config.write_text(config_text(saves, states, cores, method == "failed-state"))
        runtime = Path(sandbox.root.name) / "run"
        runtime.mkdir(mode=0o700)
        extra = {
            "XDG_RUNTIME_DIR": str(runtime),
            "FLATPAK_USER_DIR": str(REAL_HOME / ".local/share/flatpak"),
            "LIBGL_ALWAYS_SOFTWARE": "1",
            "SDL_AUDIODRIVER": "dummy",
        }
        arguments = ["--verbose", "-L", str(cores / "gambatte_libretro.so"), str(rom)]
        if method == "frame-limit":
            arguments = ["--max-frames=600", *arguments]
        if route == "flatpak":
            argv = [
                "flatpak", "run", "--nodevice=all", "--unshare=network", "--nosocket=wayland",
                "--nosocket=pulseaudio", "--nofilesystem=host", "--nofilesystem=home",
                f"--env={gui_sandbox.TOKEN_VARIABLE}={sandbox.token}", "--env=LIBGL_ALWAYS_SOFTWARE=1",
                APP, *arguments,
            ]
        else:
            argv = [
                "bwrap", "--bind", "/", "/", "--dev", "/dev",
                "--ro-bind", str(REAL_HOME), str(REAL_HOME),
                "--bind", sandbox.root.name, sandbox.root.name,
                "--bind", str(work), str(work), "--",
                NATIVE, "--config", str(config), *arguments,
            ]
        log = open(scratch / "emulator.log", "wb")
        launcher = subprocess.Popen(
            argv, env=sandbox.environment(extra), start_new_session=True,
            stdin=subprocess.PIPE, stdout=log, stderr=log,
        )
        sandbox.children.append(launcher)
        # Let the game start and perform its save write.
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline and not windows(sandbox) and launcher.poll() is None:
            time.sleep(0.5)
        time.sleep(6)
        result["started"] = launcher.poll() is None
        inner = emulator_pid(sandbox)
        began = time.monotonic()
        note = ""
        try:
            if not result["started"]:
                note = "ended before it could be closed"
            elif method == "quit-command":
                launcher.stdin.write(b"QUIT\n")
                launcher.stdin.flush()
            elif method == "failed-state":
                launcher.stdin.write(b"QUIT\n")
                launcher.stdin.flush()
            elif method == "quit-key":
                for window in windows(sandbox)[:1]:
                    subprocess.run(["xdotool", "mousemove", "--window", str(window), "40", "40"],
                                   env=x_env(sandbox), timeout=20)
                subprocess.run(["xdotool", "key", "Escape"], env=x_env(sandbox), timeout=20)
            elif method == "window-close":
                sys.path.insert(0, str(Path(__file__).resolve().parent))
                from flatpak_exit_probe import send_close_request
                for window in windows(sandbox):
                    send_close_request(sandbox.display, window)
            elif method == "terminate":
                if inner is None:
                    note = "emulator process not found"
                else:
                    sandbox._signal(inner, signal.SIGTERM)
            # frame-limit: nothing to do, it stops by itself.
        except (BrokenPipeError, OSError, gui_sandbox.SandboxError) as error:
            note = f"could not deliver the request: {error!r}"[:160]
        hung = False
        try:
            launcher.wait(timeout=45)
        except subprocess.TimeoutExpired:
            hung = True
        result["seconds_to_close"] = round(time.monotonic() - began, 1)
        result["hung"] = hung
        result["exit_code"] = launcher.returncode
        result["note"] = note
        log.close()
        try:
            sandbox.stop()
        except gui_sandbox.SandboxError as error:
            result["sandbox_note"] = str(error)[:160]
        save = saves / "emuwiz-probe.srm"
        if save.is_file():
            data = save.read_bytes()
            result["save_written"] = True
            result["save_bytes"] = len(data)
            result["save_first_byte"] = data[0] if data else None
        else:
            result["save_written"] = False
        shutil.copytree(saves, scratch / "saves-after", dirs_exist_ok=True)
    text = (scratch / "emulator.log").read_text(errors="replace")
    result["log_fault"] = [word for word in ("Segmentation fault", "core dumped", "Aborted", "SIGSEGV") if word in text]
    result["log_tail"] = [line[:150] for line in text.strip().splitlines()[-3:]]
    code = result["exit_code"]
    if hung:
        result["what_happened"] = "hung: still running 45 s after being asked to close"
    elif code == 0:
        result["what_happened"] = "closed normally"
    elif code is not None and (code < 0 or code > 128):
        number = -code if code < 0 else code - 128
        result["what_happened"] = f"ended by signal {number}"
    else:
        result["what_happened"] = f"exited with code {code}"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--only", default="native,flatpak")
    parser.add_argument("--methods", default=",".join(METHODS))
    args = parser.parse_args()
    if args.work.exists():
        sys.exit(f"{args.work} already exists; the probe works in a new folder")
    args.work.mkdir(parents=True)
    results = []
    for route in args.only.split(","):
        if route == "flatpak" and not FLATPAK_CORE.is_file():
            results.append({"route": route, "what_happened": "not run: the Flatpak profile has no Gambatte core to copy"})
            continue
        for method in args.methods.split(","):
            for index in range(1, args.repeats + 1):
                try:
                    result = one_run(route, method, args.work, index)
                except Exception as error:
                    result = {"route": route, "method": method, "run": index, "what_happened": f"probe error: {error!r}"[:200]}
                results.append(result)
                print(f"{route:<8} {method:<13} #{index}  exit={result.get('exit_code')}  {result.get('what_happened')}  "
                      f"save={result.get('save_written')}:{result.get('save_first_byte')}  {result.get('note', '')}", flush=True)
                (args.work / "results.json").write_text(json.dumps(results, indent=1))
    sys.exit(0)


if __name__ == "__main__":
    main()
