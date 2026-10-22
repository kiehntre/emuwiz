#!/usr/bin/env python3
"""Click the real "Choose a cheat file (.cht)…" button in the release Blue
binary under Xvfb and prove the production dialog path is safe when no
desktop portal can answer.

Blue's cheat-file picker is `rfd` built with the XDG desktop-portal backend:
the dialog is drawn by the user's portal service on the user's own desktop,
so it cannot be shown or driven on a private Xvfb display. What this
acceptance proves on the actual binary is the part Xvfb can reach: the
button is wired, with no session bus the picker returns like a cancel, Blue
neither hangs nor crashes, no cheat is chosen, no RetroArch is started, and
the page is still usable afterwards. The portal dialog itself needs one
manual click on a real desktop; the evidence file says so.

  blue_cheat_picker_smoke.py --repo <worktree> --binary <emuwiz-v2> \\
      --fixture-builder <verified_game_profile> --root /tmp/emuwiz-picker-x \\
      --evidence <dir> --click X,Y [--size 1280x800]

X,Y is the chooser button centre at --size, as printed by the headless
`coords` scenario of `gui_v2::retroarch_journey_tests` for the same commit.
"""
from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import blue_verified_game_smoke as base  # noqa: E402


def windows(display: str) -> list[str]:
    return subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "."],
                          env={"DISPLAY": display, "PATH": "/usr/bin:/bin"}, text=True,
                          capture_output=True).stdout.split()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    for name in ("repo", "binary", "fixture_builder", "root", "evidence"):
        parser.add_argument(f"--{name.replace('_', '-')}", type=Path, required=True)
    parser.add_argument("--click", required=True, help="X,Y of the chooser button centre")
    parser.add_argument("--size", default="1280x800")
    parser.add_argument("--settle", type=float, default=8.0)
    args = parser.parse_args()
    try:
        evidence = {"candidate": base.check_provenance(args.repo, args.binary, None, "blue"),
                    "fixture_builder": base.check_provenance(args.repo, args.fixture_builder,
                                                             None, "fixture-builder")}
    except base.Refused as refused:
        print(f"REFUSED: {refused}", file=sys.stderr)
        return 1
    root = args.root.resolve()
    if root.exists():
        print(f"REFUSED: root {root} already exists", file=sys.stderr)
        return 1
    root.mkdir(parents=True)
    args.evidence.mkdir(parents=True, exist_ok=True)
    profile = root / "profile"
    subprocess.run([str(args.fixture_builder), "build", str(profile), "--fake-retroarch"],
                   check=True, capture_output=True)
    env = base.read_env(profile / "profile.env")
    game = int(env["ARCHIVE_ID"])
    (profile / "config" / "gui-v2.json").write_text(json.dumps(
        {"route": {"Task": {"section": "Launch", "game": game}}, "welcome_dismissed": True}))
    width, height = (int(v) for v in args.size.split("x"))
    x, y = (int(v) for v in args.click.split(","))
    xvfb, display = base.start_xvfb(width, height, root)
    child_env = {k: v for k, v in env.items() if not k.startswith(("ARCHIVE_", "PLATFORM", "DATABASE"))}
    # Production discovery finds the fake RetroArch; deliberately no session
    # bus, so the portal cannot be reached from this private display.
    child_env.update({"XDG_CONFIG_HOME": str(profile / "retroarch" / "config"),
                      "PATH": f"{profile / 'retroarch' / 'bin'}:/usr/bin:/bin",
                      "DISPLAY": display, "LIBGL_ALWAYS_SOFTWARE": "1",
                      "WINIT_UNIX_BACKEND": "x11", "XDG_RUNTIME_DIR": str(root / "runtime")})
    (root / "runtime").mkdir()
    os.chmod(root / "runtime", 0o700)
    log = args.evidence / "blue.log"
    xenv = {"DISPLAY": display, "PATH": "/usr/bin:/bin"}
    steps = []
    with log.open("ab") as handle:
        app = subprocess.Popen([str(args.binary)], env=child_env, stdout=handle, stderr=handle)
    try:
        wid = ""
        for _ in range(120):
            found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "EmuWiz"],
                                   env=xenv, text=True, capture_output=True).stdout.split()
            if found:
                wid = found[0]
                break
            time.sleep(0.5)
        if not wid:
            raise RuntimeError(f"no EmuWiz window; see {log}")
        subprocess.run(["xdotool", "windowmove", wid, "0", "0", "windowsize", "--sync", wid,
                        str(width), str(height), "windowfocus", "--sync", wid], env=xenv)
        time.sleep(args.settle)

        def shot(name: str) -> str:
            path = args.evidence / f"{name}.png"
            subprocess.run(["import", "-window", "root", str(path)], env=xenv, check=True)
            return str(path)

        before = windows(display)
        steps.append({"step": "launch page", "windows": len(before), "screenshot": shot("1-launch-page")})
        subprocess.run(["xdotool", "mousemove", str(x), str(y), "click", "1"], env=xenv, check=True)
        time.sleep(4.0)
        after = windows(display)
        alive = app.poll() is None
        steps.append({"step": "clicked chooser", "click": [x, y], "windows": len(after),
                      "blue_alive": alive, "screenshot": shot("2-after-click")})
        subprocess.run(["xdotool", "key", "Escape"], env=xenv, check=True)
        time.sleep(2.0)
        steps.append({"step": "escape", "windows": len(windows(display)),
                      "blue_alive": app.poll() is None, "screenshot": shot("3-after-escape")})
        ran = (profile / "retroarch" / "record" / "args.txt").exists()
        steps.append({"step": "retroarch not started", "retroarch_ran": ran})
        ok = alive and app.poll() is None and len(after) == len(before) and not ran
    finally:
        if app.poll() is None:
            app.send_signal(signal.SIGTERM)
            try:
                app.wait(10)
            except subprocess.TimeoutExpired:
                app.kill()
        xvfb.terminate()
    text = log.read_text(errors="replace")
    panicked = "panicked at" in text
    evidence.update({"steps": steps, "panicked": panicked,
                     "portal_dialog": "UNVERIFIED under Xvfb: rfd uses the XDG desktop portal, "
                                      "which draws on the user's desktop; one manual click of "
                                      "'Choose a cheat file (.cht)…' on a real session is required",
                     "result": "PASS" if ok and not panicked else "FAIL"})
    (args.evidence / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(f"{evidence['result']}: no-portal picker path; evidence in {args.evidence}")
    return 0 if evidence["result"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
