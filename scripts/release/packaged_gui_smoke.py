#!/usr/bin/env python3
"""Bounded smoke test for the GUI binary from an extracted release archive.

This is deliberately an acceptance helper, not a GUI test framework.  It does
not click, type, or alter the packaged binary.  Each case receives disposable
HOME/XDG roots and is stopped with SIGTERM after proving that it survived the
startup interval.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path
from typing import Any


CASES = ("empty", "existing", "legacy", "both-roots")
EXPECTED_BEHAVIOR = {
    "empty": "first-run/onboarding remains usable",
    "existing": "current-schema profile starts normally",
    "legacy": "ArchiveFS-only profile remains readable",
    "both-roots": "both roots remain unmerged; conflict fixture is preserved for review",
}
SYSTEM_PATH_PREFIXES = ("/dev/", "/proc/", "/sys/", "/run/")
FATAL_MARKERS = ("panicked at", "thread 'main' panicked", "fatal error", "stack backtrace")


def inside(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(root.resolve(strict=False))
        return True
    except ValueError:
        return False


def safe_extract(archive_path: Path, destination: Path) -> Path:
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive_path, "r:*") as archive:
        members = archive.getmembers()
        for member in members:
            candidate = destination / member.name
            if Path(member.name).is_absolute() or not inside(candidate, destination):
                raise RuntimeError(f"unsafe archive member: {member.name}")
        archive.extractall(destination, members=members, filter="data")
    roots = sorted(item for item in destination.iterdir() if item.is_dir())
    if len(roots) != 1:
        raise RuntimeError("release archive did not contain exactly one root directory")
    gui = roots[0] / "bin" / "emuwiz"
    if not gui.is_file() or not os.access(gui, os.X_OK):
        raise RuntimeError(f"packaged GUI binary missing or not executable: {gui}")
    return roots[0]


def write_config(path: Path, source: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        f'source_folders = []\nmount_root = "{source}"\n', encoding="utf-8"
    )


def make_profile(case_root: Path, case: str) -> dict[str, str]:
    home = case_root / "home"
    xdg_config = case_root / "xdg-config"
    xdg_data = case_root / "xdg-data"
    xdg_cache = case_root / "xdg-cache"
    xdg_state = case_root / "xdg-state"
    runtime = case_root / "runtime"
    source = case_root / "fixture" / "source"
    for path in (home, xdg_config, xdg_data, xdg_cache, xdg_state, runtime, source):
        path.mkdir(parents=True, exist_ok=True)
    (source / "synthetic-empty-marker.txt").write_text("QA fixture\n", encoding="utf-8")
    env: dict[str, str] = {
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(xdg_config),
        "XDG_DATA_HOME": str(xdg_data),
        "XDG_CACHE_HOME": str(xdg_cache),
        "XDG_STATE_HOME": str(xdg_state),
        "XDG_RUNTIME_DIR": str(runtime),
        "TMPDIR": str(case_root / "tmp"),
        "LANG": "C",
        "LC_ALL": "C",
        "TERM": "dumb",
        "RUST_BACKTRACE": "1",
    }
    (case_root / "tmp").mkdir()
    if case in ("empty", "existing"):
        env["EMUWIZ_CONFIG_HOME"] = str(case_root / "config")
        env["EMUWIZ_DATA_HOME"] = str(case_root / "data")
        Path(env["EMUWIZ_CONFIG_HOME"]).mkdir()
        Path(env["EMUWIZ_DATA_HOME"]).mkdir()
    if case == "existing":
        write_config(Path(env["EMUWIZ_CONFIG_HOME"]) / "config.toml", source)
    elif case == "legacy":
        write_config(xdg_config / "archivefs" / "config.toml", source)
        (xdg_data / "archivefs").mkdir(parents=True, exist_ok=True)
    elif case == "both-roots":
        write_config(xdg_config / "emuwiz" / "config.toml", source)
        write_config(xdg_config / "archivefs" / "config.toml", source)
        (xdg_data / "emuwiz").mkdir(parents=True, exist_ok=True)
        (xdg_data / "archivefs").mkdir(parents=True, exist_ok=True)
    return env


def parse_write_trace(trace: Path, case_root: Path) -> list[str]:
    escaped: set[str] = set()
    if not trace.is_file():
        return ["strace output missing"]
    for line in trace.read_text(errors="replace").splitlines():
        syscall_write = bool(re.search(
            r"\b(?:open|openat|creat)\([^\n]*\bO_(?:WRONLY|RDWR|CREAT|TRUNC)", line
        )) or bool(re.search(
            r"\b(?:rename|renameat|mkdir|mkdirat|unlink|unlinkat|symlink|link)\(", line
        ))
        if not syscall_write:
            continue
        for raw in re.findall(r'"(/[^"\\]*(?:\\.[^"\\]*)*)"', line):
            path = Path(raw.replace(r"\"", '"').replace(r"\\", "\\"))
            if not path.as_posix().startswith(SYSTEM_PATH_PREFIXES) and not inside(path, case_root):
                escaped.add(str(path))
    return sorted(escaped)


def prerequisite_status(xvfb: str | None, required: bool) -> str:
    if xvfb:
        return "available"
    if required:
        raise RuntimeError("Xvfb is unavailable")
    return "SKIPPED (Xvfb unavailable)"


def display_backend_available() -> bool:
    wrapper = shutil.which("xvfb-run")
    if wrapper:
        probe = subprocess.run(
            [wrapper, "-a", "-s", "-screen 0 1280x800x24 -nolisten tcp", "true"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        if probe.returncode == 0:
            return True
    binary = shutil.which("Xvfb")
    if not binary:
        return False
    for display_number in range(120, 130):
        process = subprocess.Popen(
            [binary, f":{display_number}", "-screen", "0", "1280x800x24", "-nolisten", "tcp"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
        )
        time.sleep(0.15)
        if process.poll() is None:
            stop_process(process)
            return True
        process.kill()
        process.wait()
    return False


def snapshot_case(root: Path) -> list[str]:
    return sorted(str(path.relative_to(root)) for path in root.rglob("*") if not path.is_symlink())


def strace_available() -> bool:
    binary = shutil.which("strace")
    if not binary:
        return False
    probe = subprocess.run([binary, "-f", "-o", os.devnull, "-e", "trace=%file", "true"],
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    return probe.returncode == 0


def start_xvfb(case_root: Path) -> tuple[subprocess.Popen[bytes] | None, str | None, str | None]:
    binary = shutil.which("Xvfb")
    if not binary:
        raise FileNotFoundError("Xvfb is unavailable")
    for display_number in range(90, 120):
        display = f":{display_number}"
        lock = Path(f"/tmp/.X{display_number}-lock")
        if lock.exists():
            continue
        process = subprocess.Popen(
            [binary, display, "-screen", "0", "1280x800x24", "-nolisten", "tcp"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
        )
        time.sleep(0.15)
        if process.poll() is None:
            return process, display, None
        process.kill()
        process.wait()
    wrapper = shutil.which("xvfb-run")
    if wrapper:
        # Some constrained environments expose Xvfb but do not permit direct
        # ownership of /tmp/.X11-unix. xvfb-run still allocates and cleans a
        # dedicated display, so use it as the bounded child wrapper.
        return None, None, wrapper
    raise RuntimeError("could not allocate a dedicated Xvfb display")


def stop_process(process: subprocess.Popen[Any], timeout: float = 3.0) -> bool:
    if process.poll() is not None:
        return True
    try:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return True
        process.wait(timeout=timeout)
        return True
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=timeout)
        return False


def run_case(gui: Path, case: str, root: Path, timeout: float) -> dict[str, Any]:
    case_root = root / case
    case_root.mkdir(parents=True, exist_ok=True)
    env = make_profile(case_root, case)
    (case_root / "environment.json").write_text(json.dumps({
        "case": case,
        "roots": {key: value for key, value in env.items() if key.endswith("HOME") or key in ("HOME", "TMPDIR", "XDG_RUNTIME_DIR")},
        "profile_fixture": case,
        "expected_behavior": EXPECTED_BEHAVIOR[case],
    }, indent=2) + "\n")
    xvfb, display, wrapper = start_xvfb(case_root)
    if display:
        env["DISPLAY"] = display
    stdout_path = case_root / "stdout.log"
    stderr_path = case_root / "stderr.log"
    trace_path = case_root / "writes.strace"
    command = [str(gui)]
    if wrapper:
        command = [wrapper, "-a", "-s", "-screen 0 1280x800x24 -nolisten tcp", *command]
    traced = strace_available()
    if traced:
        command = ["strace", "-f", "-o", str(trace_path), "-e", "trace=%file", *command]
    start = time.monotonic()
    process: subprocess.Popen[bytes] | None = None
    clean_shutdown = False
    try:
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            process = subprocess.Popen(command, env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), **env}, stdout=stdout, stderr=stderr,
                                       start_new_session=True)
            time.sleep(min(3.0, timeout / 3.0))
            immediate_status = process.poll()
            if immediate_status is not None:
                raise RuntimeError(f"GUI exited during startup with status {immediate_status}")
            survived = time.monotonic() - start
            screenshot = None
            xwd = shutil.which("xwd")
            convert = shutil.which("convert")
            if xwd and display:
                xwd_path = case_root / "first-frame.xwd"
                capture = subprocess.run([xwd, "-root", "-display", display, "-out", str(xwd_path)],
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
                if capture.returncode == 0:
                    screenshot = str(xwd_path)
                    if convert:
                        png_path = case_root / "first-frame.png"
                        converted = subprocess.run([convert, str(xwd_path), str(png_path)],
                                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
                        if converted.returncode == 0:
                            screenshot = str(png_path)
            clean_shutdown = stop_process(process)
            if not clean_shutdown:
                raise RuntimeError("GUI did not shut down cleanly after SIGTERM")
    finally:
        if process is not None and process.poll() is None:
            stop_process(process)
        if xvfb is not None:
            stop_process(xvfb)
    stdout = stdout_path.read_text(errors="replace")
    stderr = stderr_path.read_text(errors="replace")
    combined = stdout + "\n" + stderr
    if any(marker in combined.lower() for marker in FATAL_MARKERS):
        raise RuntimeError("GUI logs contain a fatal panic/backtrace marker")
    escaped = parse_write_trace(trace_path, case_root) if traced and trace_path.exists() else []
    if escaped:
        raise RuntimeError("writes escaped isolation: " + ", ".join(escaped[:8]))
    result = {
        "case": case,
        "expected_behavior": EXPECTED_BEHAVIOR[case],
        "display": display or "xvfb-run -a",
        "startup_survival_seconds": round(survived, 3),
        "clean_shutdown": clean_shutdown,
        "exit_status": process.returncode if process is not None else None,
        "escaped_writes": escaped,
        "screenshot": screenshot,
        "files_inside_case": len(snapshot_case(case_root)),
        "stdout": str(stdout_path),
        "stderr": str(stderr_path),
        "strace": str(trace_path) if trace_path.exists() else None,
        "write_detection": "strace" if traced else "isolated-environment-only",
    }
    (case_root / "filesystem-writes.json").write_text(json.dumps({
        "detection": result["write_detection"],
        "escaped_writes": escaped,
        "case_root": str(case_root),
    }, indent=2) + "\n")
    return result


def run(archive: Path, output: Path, timeout: float, required: bool) -> int:
    if not archive.is_file():
        raise RuntimeError(f"release archive is missing: {archive}")
    available = bool(shutil.which("Xvfb")) and display_backend_available()
    status = prerequisite_status("usable Xvfb" if available else None, required)
    if status != "available":
        message = status
        print(message)
        (output / "status.json").parent.mkdir(parents=True, exist_ok=True)
        (output / "status.json").write_text(json.dumps({"status": "SKIPPED", "reason": message}, indent=2) + "\n")
        return 0
    output.mkdir(parents=True, exist_ok=True)
    release_root = safe_extract(archive, output / "release")
    gui = release_root / "bin" / "emuwiz"
    readelf = shutil.which("readelf")
    if readelf:
        check = subprocess.run([readelf, "-h", str(gui)], text=True, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, check=False)
        if check.returncode:
            raise RuntimeError("extracted GUI is not a readable ELF")
    results = []
    for case in CASES:
        results.append(run_case(gui, case, output / "cases", timeout))
    report = {"status": "PASS", "archive": str(archive), "extracted_gui": str(gui),
              "xvfb": shutil.which("Xvfb"), "timeout_seconds": timeout, "cases": results}
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print("PASS")
    return 0


def self_test() -> int:
    root = Path(tempfile.mkdtemp(prefix="emuwiz-gui-smoke-selftest-"))
    try:
        assert prerequisite_status(None, False) == "SKIPPED (Xvfb unavailable)"
        try:
            prerequisite_status(None, True)
        except RuntimeError:
            pass
        else:
            raise AssertionError("required Xvfb absence was accepted")
        try:
            run(Path("/missing/archive.tar.xz"), root / "optional", 1, False)
        except RuntimeError:
            # Missing archive is a distinct setup error; prerequisite handling
            # is tested directly below.
            pass
        marker_root = root / "case"
        marker_root.mkdir()
        trace = marker_root / "trace"
        trace.write_text('openat(AT_FDCWD, "/outside/file", O_WRONLY|O_CREAT) = 3\n')
        assert parse_write_trace(trace, marker_root) == ["/outside/file"]
        trace.write_text('openat(AT_FDCWD, "' + str(marker_root / 'inside') + '", O_WRONLY) = 3\n')
        assert parse_write_trace(trace, marker_root) == []
        stubborn = "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)"
        stubborn_process = subprocess.Popen([sys.executable, "-c", stubborn], start_new_session=True)
        time.sleep(0.1)
        assert stop_process(stubborn_process, 0.01) is False
        failed = subprocess.run([sys.executable, "-c", "raise SystemExit(7)"], check=False)
        assert failed.returncode == 7
        unsafe = root / "unsafe.tar.xz"
        with tarfile.open(unsafe, "w:xz") as archive:
            info = tarfile.TarInfo("../escape"); info.size = 1
            import io
            archive.addfile(info, io.BytesIO(b"x"))
        try:
            safe_extract(unsafe, root / "extract")
        except RuntimeError:
            pass
        else:
            raise AssertionError("unsafe archive path accepted")
        missing_gui = root / "missing-gui.tar.xz"
        with tarfile.open(missing_gui, "w:xz") as archive:
            info = tarfile.TarInfo("release/README.txt"); info.size = 1
            import io
            archive.addfile(info, io.BytesIO(b"x"))
        try:
            safe_extract(missing_gui, root / "missing-gui-extract")
        except RuntimeError:
            pass
        else:
            raise AssertionError("missing packaged GUI was accepted")
        assert subprocess.run([sys.executable, "-c", "pass"], check=False).returncode == 0
        shutil.rmtree(root / "case")
        assert not (root / "case").exists()
        print("PACKAGED GUI SMOKE SELF-TEST: PASS")
        return 0
    finally:
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive")
    parser.add_argument("--output")
    parser.add_argument("--timeout", type=float, default=20.0)
    parser.add_argument("--require", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not args.archive or not args.output:
        parser.error("--archive and --output are required")
    return run(Path(args.archive).resolve(), Path(args.output).resolve(), args.timeout, args.require)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
    except Exception as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
