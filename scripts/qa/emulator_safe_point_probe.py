#!/usr/bin/env python3
"""Start each installed emulator for real, in isolation, and check that
`emuwiz-cli continuity safe-point` says the right thing before, during and
after.

    emulator_safe_point_probe.py --cli <emuwiz-cli> --work <new-folder> [--only a,b]

For every launch route below the probe checks, on a save file in a scratch
folder:

  before launch   verified_safe   (nothing of that emulator is running)
  while running   active          (the emulator is seen, whatever started it)
  after it ends   verified_safe   (no leftover helper process blocks it for good)

and that no *other* emulator's saves are reported active because of it.

Isolation, so the person's real profile cannot be touched:

* a private Xvfb display and a throwaway HOME (gui_sandbox.Sandbox);
* native binaries and AppImages run inside bwrap with a minimal /dev (no
  /dev/input, no hidraw, no uinput: a connected controller is never opened)
  and the real home directory mounted read-only;
* Flatpak apps run with --nodevice=all, no network, no host filesystem, and
  the throwaway HOME, so their data folder is the scratch one;
* no session bus is passed on;
* afterwards the real emulator folders are searched for anything changed since
  the probe began, and a change fails the route.

Processes are stopped only through gui_sandbox (pidfd + ownership token). An
emulator that is not installed is reported as "not installed", never as passing.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gui_sandbox  # noqa: E402

REAL_HOME = Path.home()
EMULATORS = ["PCSX2", "RetroArch", "DuckStation", "Dolphin", "RPCS3", "PPSSPP"]

# name -> (emulator, kind, program or app id, arguments, real folders to watch)
ROUTES = {
    "pcsx2-flatpak": ("PCSX2", "flatpak", "net.pcsx2.PCSX2", [], [".var/app/net.pcsx2.PCSX2", ".config/PCSX2"]),
    "pcsx2-wrapper-script": ("PCSX2", "wrapper", "net.pcsx2.PCSX2", [], [".var/app/net.pcsx2.PCSX2", ".config/PCSX2"]),
    "retroarch-native": ("RetroArch", "native", "/usr/games/retroarch", ["--verbose"], [".config/retroarch"]),
    "retroarch-flatpak": ("RetroArch", "flatpak", "org.libretro.RetroArch", [], [".var/app/org.libretro.RetroArch"]),
    "duckstation-appimage": (
        "DuckStation",
        "appimage",
        str(REAL_HOME / "Applications/DuckStation/DuckStation.AppImage"),
        [],
        [".local/share/duckstation", ".config/duckstation", "Applications/DuckStation"],
    ),
    "dolphin-native": ("Dolphin", "native", "dolphin-emu", [], [".local/share/dolphin-emu", ".config/dolphin-emu"]),
    "dolphin-flatpak": ("Dolphin", "flatpak", "org.DolphinEmu.dolphin-emu", [], [".var/app/org.DolphinEmu.dolphin-emu"]),
    "rpcs3-flatpak": ("RPCS3", "flatpak", "net.rpcs3.RPCS3", [], [".var/app/net.rpcs3.RPCS3", ".config/rpcs3"]),
    "ppsspp-flatpak": ("PPSSPP", "flatpak", "org.ppsspp.PPSSPP", [], [".var/app/org.ppsspp.PPSSPP", ".config/ppsspp"]),
}


def installed(kind, program):
    if kind in ("flatpak", "wrapper"):
        listed = subprocess.run(["flatpak", "info", program], capture_output=True, text=True)
        return listed.returncode == 0
    return shutil.which(program) is not None or Path(program).is_file()


def verdict(cli, scratch, emulator, save):
    env = dict(os.environ, EMUWIZ_DATA_HOME=str(scratch / "emuwiz-data"), EMUWIZ_CONFIG_HOME=str(scratch / "emuwiz-config"))
    out = subprocess.run(
        [cli, "continuity", "safe-point", "--emulator", emulator, str(save), "--json"],
        capture_output=True, text=True, env=env, timeout=60,
    )
    if out.returncode != 0:
        return {"verdict": "error", "detail": out.stderr.strip()[-300:]}
    return json.loads(out.stdout)


def wait_for(cli, scratch, emulator, save, wanted, seconds):
    deadline = time.monotonic() + seconds
    seen = None
    while time.monotonic() < deadline:
        seen = verdict(cli, scratch, emulator, save)
        if seen["verdict"] == wanted:
            return seen
        time.sleep(0.5)
    return seen


def describe(pid):
    proc = Path("/proc") / str(pid)
    try:
        cmdline = (proc / "cmdline").read_bytes().split(b"\0")
        cgroup = (proc / "cgroup").read_text().strip().rsplit("/", 1)[-1]
        return {
            "pid": pid,
            "name": (proc / "comm").read_text().strip(),
            "exe": os.readlink(proc / "exe"),
            "command": " ".join(part.decode(errors="replace") for part in cmdline[:4] if part)[:160],
            "cgroup": cgroup,
        }
    except OSError:
        return None


def changed_since(marker, folders):
    """Files under the real profile folders changed after the probe began."""
    changed = []
    stamp = marker.stat().st_mtime_ns
    for folder in folders:
        root = REAL_HOME / folder
        for base, _dirs, files in os.walk(root, followlinks=False):
            for name in files:
                path = Path(base) / name
                try:
                    info = path.lstat()
                except OSError:
                    continue
                if max(info.st_mtime_ns, info.st_ctime_ns) > stamp:
                    changed.append(str(path))
    return changed


def command(sandbox, kind, program, arguments, scratch):
    if kind == "flatpak" or kind == "wrapper":
        run = [
            "flatpak", "run", "--nodevice=all", "--unshare=network", "--nosocket=wayland",
            "--nosocket=pulseaudio", "--nofilesystem=host", "--nofilesystem=home",
            f"--env={gui_sandbox.TOKEN_VARIABLE}={sandbox.token}", "--env=LIBGL_ALWAYS_SOFTWARE=1",
            program, *arguments,
        ]
        if kind == "wrapper":
            # The same thing through a shell script, as ~/.local/bin wrappers do.
            script = scratch / "wrapper.sh"
            script.write_text("#!/usr/bin/env bash\nexec " + " ".join(f'"{part}"' for part in run) + ' "$@"\n')
            script.chmod(0o755)
            return [str(script)]
        return run
    isolate = [
        "bwrap", "--bind", "/", "/", "--dev", "/dev",
        "--ro-bind", str(REAL_HOME), str(REAL_HOME),
        "--bind", sandbox.root.name, sandbox.root.name,
        "--bind", str(scratch), str(scratch), "--",
    ]
    return isolate + [program, *arguments]


def probe(name, cli, work):
    emulator, kind, program, arguments, watched = ROUTES[name]
    result = {"route": name, "emulator": emulator, "kind": kind}
    if not installed(kind, program):
        result["outcome"] = "not_installed"
        result["detail"] = f"{program} is not installed on this machine; this route was not run"
        return result
    scratch = work / name
    scratch.mkdir(parents=True)
    save = scratch / "save.bin"
    save.write_bytes(b"synthetic save for the safe-point probe\n" * 64)
    marker = scratch / "began"
    marker.write_text("")
    time.sleep(0.05)
    steps = result.setdefault("steps", {})

    before = verdict(cli, scratch, emulator, save)
    steps["before"] = before["verdict"]
    if before["verdict"] != "verified_safe":
        result["outcome"] = "inconclusive"
        result["detail"] = f"{emulator} was already running (or could not be checked) before the probe started it"
        return result

    with gui_sandbox.Sandbox() as sandbox:
        sandbox.start_display("1280x800x24")
        runtime = Path(sandbox.root.name) / "run"
        runtime.mkdir(mode=0o700)
        extra = {
            "XDG_RUNTIME_DIR": str(runtime),
            "FLATPAK_USER_DIR": str(REAL_HOME / ".local/share/flatpak"),
            "LIBGL_ALWAYS_SOFTWARE": "1",
            "QT_QPA_PLATFORM": "xcb",
            "SDL_AUDIODRIVER": "dummy",
            "SDL_VIDEODRIVER": "x11",
            "APPIMAGE_EXTRACT_AND_RUN": "1",
            "TMPDIR": str(scratch),
        }
        log = open(scratch / "emulator.log", "wb")
        sandbox.spawn(command(sandbox, kind, program, arguments, scratch), extra, log)
        during = wait_for(cli, scratch, emulator, save, "active", 90)
        steps["during"] = during["verdict"]
        steps["during_state"] = during.get("state")
        time.sleep(2)  # let helpers and children appear before listing them
        result["processes"] = [info for info in map(describe, sandbox.owned()) if info and info["name"] != "Xvfb"]
        # Another emulator's saves must not be blocked by this one.
        result["others_blocked"] = [
            other for other in EMULATORS
            if other != emulator and verdict(cli, scratch, other, save)["verdict"] == "active"
        ]
        sandbox.stop()
        log.close()
    after = wait_for(cli, scratch, emulator, save, "verified_safe", 30)
    steps["after"] = after["verdict"]
    result["real_profile_changed"] = changed_since(marker, watched)
    ok = (
        steps["during"] == "active"
        and steps["after"] == "verified_safe"
        and not result["others_blocked"]
        and not result["real_profile_changed"]
    )
    result["outcome"] = "pass" if ok else "fail"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cli", required=True)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--only", default="")
    args = parser.parse_args()
    if args.work.exists():
        sys.exit(f"{args.work} already exists; the probe works in a new folder")
    args.work.mkdir(parents=True)
    names = [name for name in ROUTES if not args.only or name in args.only.split(",")]
    results = []
    for name in names:
        try:
            result = probe(name, args.cli, args.work)
        except Exception as error:  # a broken route must not hide the others
            result = {"route": name, "emulator": ROUTES[name][0], "outcome": "error", "detail": repr(error)[:300]}
        results.append(result)
        steps = result.get("steps", {})
        print(
            f"{name:<24} {result['outcome']:<14} "
            f"before={steps.get('before', '-')} during={steps.get('during', '-')} after={steps.get('after', '-')} "
            f"{result.get('detail', '')}",
            flush=True,
        )
    (args.work / "results.json").write_text(json.dumps(results, indent=1))
    sys.exit(0 if all(result["outcome"] in ("pass", "not_installed") for result in results) else 1)


if __name__ == "__main__":
    main()
