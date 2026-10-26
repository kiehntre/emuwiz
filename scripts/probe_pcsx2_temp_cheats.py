#!/usr/bin/env python3
"""Evidence-only PCSX2 probe. Never authorizes a production cheat launch.

Requires explicit approval, an already installed Flatpak, a local BIOS.
Every emulator write goes to a fresh private /tmp tree, retained as evidence.
No game, BIOS or emulator is downloaded. No production profile is read/copied.
"""

import argparse
import configparser
import functools
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import struct
import subprocess
import tempfile
import time


APP_ID = "net.pcsx2.PCSX2"
MISSING_PROOF = [
    "RealUserDataUntouched: no real memory-card/save routing exercise",
    "RealConfigurationUnchanged: no production-root before/after comparison",
    "PatchBehaviourPreserved: no normal-versus-staged game baseline",
    "SandboxSawRealUserFolders: real user-folder routing was not exercised",
    "Production staging, crash recovery and Blue handoff have not been validated",
]


def fixture_elf():
    """Original MIPS branch loop, no third-party game code or BIOS contents."""
    data = bytearray(0x1008)
    data[:16] = b"\x7fELF\x01\x01\x01" + b"\0" * 9
    struct.pack_into("<HHIIIIIHHHHHH", data, 16, 2, 8, 1, 0x100000,
                     52, 0, 0x20924001, 52, 32, 1, 0, 0, 0)
    struct.pack_into("<IIIIIIII", data, 52, 1, 0x1000, 0x100000,
                     0x100000, 8, 8, 7, 0x1000)
    struct.pack_into("<II", data, 0x1000, 0x1000FFFF, 0)
    crc = functools.reduce(lambda a, b: a ^ b,
                           (v[0] for v in struct.iter_unpack("<I", data)), 0)
    return bytes(data), f"{crc:08X}"


def observations(log, root, crc):
    """Only observations, never an activation authority or supported verdict."""
    cheat_path = root / "cheats" / f"{crc}.pnach"
    return {
        "data_root_observed": f"DataRoot Directory: {root}\n" in log,
        "cheats_folder_observed": f"Cheats Directory: {root / 'cheats'}\n" in log,
        "pnach_consumption_observed": f"Found 1 cheats in {cheat_path}." in log,
        "named_cheat_enabled_observed": bool(re.search(
            r"Enabled patch: EMUWIZ Probe(?:\r?\n|$)", log)),
        "in_game_effect": "NOT CONFIRMED",
        "production_support": "UNSUPPORTED_PENDING_PRESERVATION_PROOF",
        "missing_proof": MISSING_PROOF,
    }


def flatpak_instances(binary, env):
    result = subprocess.run([binary, "ps", "--columns=instance"], env=env,
                            capture_output=True, text=True, timeout=5, check=True)
    return set(result.stdout.split())


def stop_instance(binary, instance, env):
    if not instance.isdecimal() or len(instance) > 20:
        raise ValueError("Flatpak returned an invalid instance ID; no broad stop attempted")
    if instance in flatpak_instances(binary, env):
        subprocess.run([binary, "kill", instance], env=env, capture_output=True, timeout=5)
    deadline = time.monotonic() + 5
    while instance in flatpak_instances(binary, env):
        if time.monotonic() >= deadline:
            raise ValueError(f"probe instance {instance} is still listed; cleanup NOT CONFIRMED")
        time.sleep(0.1)


def capture_context(instance, workspace, output, env):
    if not instance.isdecimal() or len(instance) > 20:
        raise ValueError("Flatpak returned an invalid instance ID")
    runtime = Path(env.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    info = runtime / ".flatpak" / instance / "info"
    with os.fdopen(os.open(info, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 64 * 1024:
            raise ValueError("Flatpak context is not a bounded regular file")
        data = stream.read(64 * 1024 + 1)
    if len(data) > 64 * 1024:
        raise ValueError("Flatpak context grew beyond its size bound")
    context = configparser.ConfigParser(interpolation=None)
    context.read_string(data.decode())
    grants = context.get("Context", "filesystems", fallback="").split(";")
    grants = [grant for grant in grants if grant and not grant.startswith("!")]
    with output.with_suffix(".context.json").open("x") as stream:
        json.dump({"filesystem_grants": grants}, stream, indent=2)
    if set(grants) not in ({str(workspace)}, {str(workspace) + ":rw"}):
        raise ValueError("sandbox retained unexpected filesystem grants; boot refused")


def run_bounded(command, output, env, timeout, workspace=None):
    # Flatpak children can leave the host launcher's process group. The FD
    # binds cleanup to this exact instance, never an app-wide `flatpak kill`.
    instance_path = output.with_suffix(".instance")
    with output.open("xb") as stream, instance_path.open("xb") as instance_stream:
        descriptor = instance_stream.fileno()
        invocation = command[:2] + ["--die-with-parent", f"--instance-id-fd={descriptor}"] + command[2:]
        process = subprocess.Popen(invocation, stdout=stream, stderr=subprocess.STDOUT,
                                   env=env, pass_fds=(descriptor,), start_new_session=True)
        timed_out = False
        instance = ""
        try:
            if workspace is not None:
                deadline = time.monotonic() + 2
                while not instance_path.read_text().strip() and time.monotonic() < deadline:
                    if process.poll() is not None:
                        break
                    time.sleep(.02)
                instance = instance_path.read_text().strip()
                capture_context(instance, workspace, output, env)
            try:
                process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
        finally:
            try:
                instance = instance_path.read_text().strip()
                if instance:
                    stop_instance(command[0], instance, env)
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=3)
                # Recheck after the host has exited as well. Missing IDs are
                # not evidence of cleanup and must never become a success.
                if instance:
                    stop_instance(command[0], instance, env)
    if not instance:
        raise ValueError("Flatpak returned no instance ID; lifecycle NOT CONFIRMED")
    return {"exit_code": process.returncode, "bounded_stop": timed_out,
            "instance": instance, "instance_absent_after_stop": True,
            "filesystem_grants_verified": workspace is not None}


def copy_bios(source, target):
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
    with os.fdopen(os.open(source, flags), "rb") as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= 8 * 1024**2:
            raise ValueError("BIOS must be a regular non-symlink file of at most 8 MiB")
        data = stream.read(8 * 1024**2 + 1)
        after = os.fstat(stream.fileno())
        current = os.stat(source, follow_symlinks=False)
        identity = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
        if len(data) != before.st_size or identity(before) != identity(after) or identity(before) != identity(current):
            raise ValueError("BIOS source changed while copying")
    with target.open("xb") as stream:
        stream.write(data)
    return hashlib.sha256(data).hexdigest()


def probe(bios, use_display=False):
    root = Path(tempfile.mkdtemp(prefix="emuwiz-pcsx2-probe-", dir="/tmp"))
    # This file is deliberately present before starting any child process.
    report = {"production_support": "UNSUPPORTED_PENDING_PRESERVATION_PROOF",
              "in_game_effect": "NOT CONFIRMED", "missing_proof": MISSING_PROOF}
    report_path = root / "report.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Disposable evidence: {root}", flush=True)
    try:
        flatpak = shutil.which("flatpak")
        if not flatpak:
            raise ValueError("installed flatpak is required")
        deployment = subprocess.run([flatpak, "info", "--show-commit", APP_ID],
                                    capture_output=True, text=True, timeout=10, check=True)
        report["flatpak_deployment"] = deployment.stdout.strip()
        for name in ["home", "config", "data", "cache", "stage"]:
            (root / name).mkdir(mode=0o700)
        private_env = {**os.environ}
        command = [flatpak, "run", "--nosocket=wayland",
                   "--nofilesystem=host:reset", f"--filesystem={root}",
                   f"--env=QT_QPA_PLATFORM={'xcb' if use_display else 'offscreen'}"]
        if use_display:
            if not private_env.get("DISPLAY"):
                raise ValueError("--use-display requires DISPLAY (run under xvfb-run)")
            command.append("--socket=x11")
        else:
            command.extend(["--unshare=network", "--nosocket=x11"])
        for variable, folder in [("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                                  ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache")]:
            command.append(f"--env={variable}={root / folder}")
        command += [APP_ID, "-datapath", str(root / "stage"), "-earlyconsolelog"]
        report["configuration_probe"] = run_bounded(
            command + ["-testconfig"], root / "configuration.log", private_env, 15, root)
        data_root = root / "stage" / "PCSX2"
        configuration = data_root / "inis" / "PCSX2.ini"
        if report["configuration_probe"]["exit_code"] != 0 or not configuration.is_file():
            raise ValueError("installed runtime did not initialize the expected private data root")
        config = configparser.ConfigParser(interpolation=None, strict=True)
        config.optionxform = str
        config.read(configuration)
        # Modify only the newly generated disposable configuration.
        for section, key, value in [
            ("UI", "SetupWizardIncomplete", "false"),
            ("EmuCore", "EnableCheats", "true"),
            ("EmuCore", "SaveStateOnShutdown", "false"),
            ("EmuCore/GS", "Renderer", "11"),  # v2.8.2 null renderer
            ("SPU2/Output", "OutputModule", "nullout"),
            ("MemoryCards", "Slot1_Enable", "false"),
            ("MemoryCards", "Slot2_Enable", "false"),
            ("Logging", "EnableSystemConsole", "true"),
            ("Logging", "EnableFileLogging", "true"),
            ("Filenames", "BIOS", "probe-bios.bin"),
            ("Cheats", "Enable", "EMUWIZ Probe"),
            ("AutoUpdater", "CheckAtStartup", "false"),
        ]:
            if not config.has_section(section):
                config.add_section(section)
            config.set(section, key, value)
        with configuration.open("w") as stream:
            config.write(stream)
        (data_root / "bios").mkdir(exist_ok=True)
        report["bios_sha256"] = copy_bios(bios, data_root / "bios" / "probe-bios.bin")
        elf, crc = fixture_elf()
        report["fixture_crc"] = crc
        (root / "probe.elf").write_bytes(elf)
        (data_root / "cheats").mkdir(exist_ok=True)
        (data_root / "cheats" / f"{crc}.pnach").write_text(
            "[EMUWIZ Probe]\npatch=1,EE,00110000,word,12345678\n")
        report["boot_probe"] = run_bounded(
            command + ["-logfile", str(root / "emulog.txt"), "-nogui", "-elf", str(root / "probe.elf")],
            root / "boot.log", private_env, 10, root)
        log = (root / "boot.log").read_text(errors="replace")
        if (root / "emulog.txt").is_file():
            log += "\n" + (root / "emulog.txt").read_text(errors="replace")
        report.update(observations(log, data_root, crc))
    except Exception as error:
        report["probe_error"] = str(error)
    finally:
        report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    # Deliberately no success exit code suggesting production support.
    return 2


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bios", required=True, type=Path, help="local BIOS to copy read-only")
    parser.add_argument("--approve-disposable-activation", action="store_true", required=True,
                        help="approve synthetic cheat activation only in the private probe")
    parser.add_argument("--use-display", action="store_true",
                        help="use DISPLAY, e.g. under xvfb-run; enables X11 transport")
    args = parser.parse_args()
    raise SystemExit(probe(args.bios, args.use_display))
