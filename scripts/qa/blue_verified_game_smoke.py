#!/usr/bin/env python3
"""Run the real Blue binary against the verified-game fixture under Xvfb.

The candidate must be named explicitly: the worktree it was built from and
the executable itself. There is no default and no fallback, so an old
build from another checkout can never be smoke-tested by accident. Before
anything starts, the executable's provenance is checked against that
worktree (clean HEAD, binary inside the worktree's own cargo-iso target,
no tracked source newer than the binary) and recorded as evidence.

  blue_verified_game_smoke.py --repo <worktree> --binary <emuwiz-v2> \\
      --fixture-builder <verified_game_profile> --root /tmp/emuwiz-smoke-x \\
      --evidence <dir> [--sizes 1280x800,700x520] [--check-only]
  blue_verified_game_smoke.py --selftest
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

SOURCE_PREFIXES = ("crates/", "src/", "Cargo.toml", "Cargo.lock", "build.rs")


class Refused(RuntimeError):
    """The candidate is not acceptable; nothing was run."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git(repo: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(repo), *args], check=True, text=True,
                          capture_output=True).stdout.strip()


def target_dir(repo: Path, override: Path | None) -> Path:
    if override is not None:
        return override.resolve()
    out = subprocess.run([str(repo / "scripts/cargo-iso"), "--print-target"], cwd=repo,
                         check=True, text=True, capture_output=True).stdout
    for line in out.splitlines():
        if line.startswith("target:"):
            return Path(line.split(":", 1)[1].strip()).resolve()
    raise Refused("cargo-iso did not report a target directory")


def check_provenance(repo: Path, binary: Path, override: Path | None, role: str) -> dict:
    """Refuse a binary that is not demonstrably built from this worktree's HEAD."""
    repo = repo.resolve()
    if not (repo / ".git").exists():
        raise Refused(f"{repo} is not a git worktree")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise Refused(f"{role}: {binary} is not an executable file")
    head = git(repo, "rev-parse", "HEAD")
    dirt = git(repo, "status", "--porcelain", "--untracked-files=no")
    if dirt:
        raise Refused(f"{role}: worktree {repo} has uncommitted changes; the binary cannot be "
                      f"tied to {head[:12]}:\n{dirt}")
    target = target_dir(repo, override)
    real = binary.resolve()
    if target not in real.parents:
        raise Refused(f"{role}: {real} is not inside the worktree's own target {target}")
    built_at = real.stat().st_mtime
    newer = []
    for rel in git(repo, "ls-files", "-z").split("\0"):
        if rel and rel.startswith(SOURCE_PREFIXES):
            path = repo / rel
            if path.exists() and path.stat().st_mtime > built_at:
                newer.append(rel)
    if newer:
        raise Refused(f"{role}: STALE - {len(newer)} tracked source file(s) are newer than the "
                      f"binary, e.g. {newer[:3]}; rebuild from {head[:12]} first")
    version = subprocess.run([str(real), "--version"], text=True, capture_output=True,
                             timeout=30)
    return {
        "role": role, "repo": str(repo), "git_head": head,
        "git_branch": git(repo, "rev-parse", "--abbrev-ref", "HEAD"),
        "binary": str(real), "binary_sha256": sha256(real), "binary_size": real.stat().st_size,
        "binary_mtime_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(built_at)),
        "target_dir": str(target),
        "version_output": (version.stdout or version.stderr).strip(),
    }


def read_env(path: Path) -> dict[str, str]:
    env = {}
    for line in path.read_text().splitlines():
        key, _, value = line.partition("=")
        env[key] = value
    return env


def start_xvfb(width: int, height: int, root: Path) -> tuple[subprocess.Popen, str]:
    fdfile = root / f"displayfd-{width}x{height}-{os.getpid()}"
    with fdfile.open("w") as handle:
        proc = subprocess.Popen(["Xvfb", "-displayfd", str(handle.fileno()), "-screen", "0",
                                 f"{width}x{height}x24", "-nolisten", "tcp"],
                                pass_fds=(handle.fileno(),), stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL)
    for _ in range(80):
        text = fdfile.read_text().strip()
        if text:
            return proc, f":{text}"
        time.sleep(0.25)
    proc.kill()
    raise RuntimeError("Xvfb did not report a display")


def run_blue(binary: Path, profile_env: dict[str, str], width: int, height: int, root: Path,
             shot: Path, log: Path, settle: float) -> dict:
    xvfb, display = start_xvfb(width, height, root)
    env = {k: v for k, v in profile_env.items()}
    env.update({"PATH": "/usr/bin:/bin", "DISPLAY": display, "LIBGL_ALWAYS_SOFTWARE": "1",
                "WINIT_UNIX_BACKEND": "x11", "XDG_RUNTIME_DIR": str(root / "runtime")})
    (root / "runtime").mkdir(exist_ok=True)
    os.chmod(root / "runtime", 0o700)
    result = {"size": f"{width}x{height}", "display": display, "screenshot": str(shot)}
    app = None
    try:
        with log.open("ab") as handle:
            app = subprocess.Popen([str(binary)], env=env, stdout=handle, stderr=handle,
                                   cwd=str(root))
        wid = ""
        for _ in range(120):
            found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "EmuWiz"],
                                   env={"DISPLAY": display, "PATH": env["PATH"]},
                                   text=True, capture_output=True).stdout.split()
            if found:
                wid = found[0]
                break
            if app.poll() is not None:
                raise RuntimeError(f"Blue exited early with {app.returncode}; see {log}")
            time.sleep(0.5)
        if not wid:
            raise RuntimeError(f"no EmuWiz window appeared on {display}; see {log}")
        xenv = {"DISPLAY": display, "PATH": env["PATH"]}
        subprocess.run(["xdotool", "windowmove", wid, "0", "0", "windowsize", "--sync", wid,
                        str(width), str(height)], env=xenv, check=False)
        subprocess.run(["xdotool", "windowfocus", "--sync", wid], env=xenv, check=False)
        time.sleep(settle)
        geometry = subprocess.run(["xdotool", "getwindowgeometry", wid], env=xenv, text=True,
                                  capture_output=True).stdout
        result["window_geometry"] = geometry.strip().replace("\n", " ")
        subprocess.run(["import", "-window", "root", str(shot)], env=xenv, check=True)
        result["window_id"] = wid
        result["screenshot_sha256"] = sha256(shot)
    finally:
        if app is not None and app.poll() is None:
            app.send_signal(signal.SIGTERM)
            try:
                app.wait(10)
            except subprocess.TimeoutExpired:
                app.kill()
        result["exit_code"] = app.returncode if app else None
        xvfb.terminate()
    text = log.read_text(errors="replace")
    if "panicked at" in text:
        raise RuntimeError(f"Blue panicked; see {log}")
    return result


def smoke(args: argparse.Namespace) -> int:
    evidence = {"started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "candidate": check_provenance(args.repo, args.binary, args.target_dir, "blue"),
                "fixture_builder": check_provenance(args.repo, args.fixture_builder,
                                                    args.target_dir, "fixture-builder")}
    if evidence["candidate"]["git_head"] != evidence["fixture_builder"]["git_head"]:
        raise Refused("the fixture builder and Blue come from different HEADs")
    args.evidence.mkdir(parents=True, exist_ok=True)
    out = args.evidence / "evidence.json"
    if args.check_only:
        out.write_text(json.dumps(evidence, indent=2) + "\n")
        print(json.dumps(evidence, indent=2))
        return 0
    for tool in ("Xvfb", "xdotool", "import"):
        if shutil.which(tool) is None:
            raise Refused(f"{tool} is required")
    root = args.root.resolve()
    if root.exists():
        raise Refused(f"root {root} already exists; choose a fresh disposable path")
    root.mkdir(parents=True)
    profile = root / "profile"
    build = subprocess.run([str(args.fixture_builder), "build", str(profile), "--fake-retroarch"],
                           text=True, capture_output=True)
    if build.returncode != 0:
        raise RuntimeError(f"fixture build failed: {build.stderr}")
    profile_env = read_env(profile / "profile.env")
    evidence["profile"] = {"root": str(profile), "env": profile_env}
    # Open straight on the Library, as the old harness did, without the welcome.
    (profile / "config" / "gui-v2.json").write_text(
        '{"route":"BrowsePlay","welcome_dismissed":true}\n')
    runs = []
    log = args.evidence / "blue.log"
    sizes = [tuple(int(v) for v in size.split("x")) for size in args.sizes.split(",")]
    for index, (width, height) in enumerate(sizes, 1):
        runs.append(run_blue(args.binary, {k: v for k, v in profile_env.items()
                                           if not k.startswith(("ARCHIVE_", "PLATFORM", "DATABASE"))},
                             width, height, root, args.evidence / f"run{index}-{width}x{height}.png",
                             log, args.settle))
    # Restart on the same profile, then prove the stored identity is unchanged.
    width, height = sizes[0]
    runs.append(run_blue(args.binary, {k: v for k, v in profile_env.items()
                                       if not k.startswith(("ARCHIVE_", "PLATFORM", "DATABASE"))},
                         width, height, root, args.evidence / f"restart-{width}x{height}.png",
                         log, args.settle))
    verify = subprocess.run([str(args.fixture_builder), "verify", str(profile)], text=True,
                            capture_output=True)
    evidence["runs"] = runs
    evidence["identity_after_restart"] = {"exit_code": verify.returncode,
                                          "stdout": verify.stdout.strip(),
                                          "stderr": verify.stderr.strip()}
    evidence["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    out.write_text(json.dumps(evidence, indent=2) + "\n")
    if verify.returncode != 0:
        print(f"FAIL: identity did not survive the restart: {verify.stderr}", file=sys.stderr)
        return 1
    print(f"PASS: {len(runs)} Blue runs, identity verified after restart; evidence in {out}")
    return 0


def self_test() -> int:
    """The harness refuses every unproven candidate and accepts a proven one."""
    with tempfile.TemporaryDirectory(prefix="emuwiz-smoke-selftest-") as temp:
        base = Path(temp)
        repo = base / "repo"
        (repo / "crates").mkdir(parents=True)
        (repo / "scripts").mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t",
                        "commit", "-q", "--allow-empty", "-m", "base"], check=True)
        source = repo / "crates" / "lib.rs"
        source.write_text("fn main() {}\n")
        subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
        subprocess.run(["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t",
                        "commit", "-q", "-m", "source"], check=True)
        target = base / "target"
        (target / "debug").mkdir(parents=True)
        binary = target / "debug" / "emuwiz-v2"
        binary.write_text("#!/bin/sh\necho emuwiz selftest\n")
        binary.chmod(0o755)
        past = time.time() - 100
        os.utime(source, (past, past))

        def attempt(**overrides):
            args = argparse.Namespace(repo=repo, binary=binary, fixture_builder=binary,
                                      target_dir=target, check_only=True,
                                      evidence=base / "evidence", root=base / "root",
                                      sizes="100x100", settle=0.0)
            for key, value in overrides.items():
                setattr(args, key, value)
            try:
                return smoke(args), None
            except Refused as refused:
                return 1, str(refused)

        code, why = attempt()
        assert code == 0 and why is None, why
        evidence = json.loads((base / "evidence" / "evidence.json").read_text())
        assert evidence["candidate"]["git_head"] == git(repo, "rev-parse", "HEAD")
        assert evidence["candidate"]["binary_sha256"] == sha256(binary)
        assert "selftest" in evidence["candidate"]["version_output"]

        outside = base / "elsewhere"
        shutil.copy(binary, outside)
        outside.chmod(0o755)
        code, why = attempt(binary=outside)
        assert code == 1 and "not inside the worktree's own target" in why, why

        os.utime(source, None)  # the checkout moved on after the build: stale
        code, why = attempt()
        assert code == 1 and "STALE" in why, why
        os.utime(source, (past, past))

        source.write_text("fn main() { dirty }\n")
        os.utime(source, (past, past))
        code, why = attempt()
        assert code == 1 and "uncommitted changes" in why, why
        subprocess.run(["git", "-C", str(repo), "checkout", "--", "."], check=True)
        os.utime(source, (past, past))

        # The same binary presented from another checkout whose files were
        # written after the build (what a branch switch does) is stale too.
        other = base / "other-repo"
        shutil.copytree(repo, other)
        os.utime(other / "crates" / "lib.rs", None)
        code, why = attempt(repo=other)
        assert code == 1 and "STALE" in why, why
    # Missing --repo/--binary is refused by the parser itself.
    missing = subprocess.run([sys.executable, "-I", __file__], text=True, capture_output=True)
    assert missing.returncode == 2 and "required" in missing.stderr, missing.stderr
    print("selftest: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--repo", type=Path, help="the worktree the candidate was built from")
    parser.add_argument("--binary", type=Path, help="the emuwiz-v2 executable under test")
    parser.add_argument("--fixture-builder", type=Path,
                        help="the verified_game_profile example binary from the same worktree")
    parser.add_argument("--root", type=Path, help="fresh disposable profile root")
    parser.add_argument("--evidence", type=Path, help="directory for screenshots and evidence.json")
    parser.add_argument("--sizes", default="1280x800,700x520")
    parser.add_argument("--settle", type=float, default=6.0)
    parser.add_argument("--target-dir", type=Path, help="override cargo-iso's target (self-test)")
    parser.add_argument("--check-only", action="store_true", help="stop after provenance")
    args = parser.parse_args()
    if args.selftest:
        return self_test()
    for name in ("repo", "binary", "fixture_builder", "evidence"):
        if getattr(args, name) is None:
            parser.error(f"--{name.replace('_', '-')} is required; there is no default candidate")
    if not args.check_only and args.root is None:
        parser.error("--root is required")
    try:
        return smoke(args)
    except Refused as refused:
        print(f"REFUSED: {refused}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
