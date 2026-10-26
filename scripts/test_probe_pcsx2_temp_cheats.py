#!/usr/bin/env python3
"""Probe regressions; do not launch an emulator or read a real BIOS."""

import hashlib
import os
from pathlib import Path
import tempfile
import signal
import sys
import threading
import unittest

from probe_pcsx2_temp_cheats import capture_context, copy_bios, fixture_elf, observations, run_bounded


class ProbeTests(unittest.TestCase):
    def test_effective_context_refuses_inherited_game_library_grants(self):
        with tempfile.TemporaryDirectory(prefix="emuwiz-pcsx2-probe-test-") as tmp:
            root = Path(tmp)
            info = root / ".flatpak" / "424242" / "info"
            info.parent.mkdir(parents=True)
            workspace = root / "workspace"
            env = {"XDG_RUNTIME_DIR": str(root)}
            info.write_text(f"[Context]\nfilesystems={workspace};/mnt/games;\n")
            with self.assertRaisesRegex(ValueError, "unexpected filesystem grants"):
                capture_context("424242", workspace, root / "bad.log", env)
            info.write_text(f"[Context]\nfilesystems={workspace};\n")
            capture_context("424242", workspace, root / "good.log", env)

    def fake_flatpak(self, root):
        binary = root / "flatpak"
        binary.write_text(f"#!{sys.executable}\n" + '''
import os, pathlib, sys, time
p = pathlib.Path(__file__)
state = p.with_suffix('.running')
if sys.argv[1] == 'ps':
    if state.exists(): print('424242')
elif sys.argv[1] == 'kill':
    assert sys.argv[2:] == ['424242'], 'must never kill by application ID'
    state.unlink(missing_ok=True)
    p.with_suffix('.killed').write_text(sys.argv[2])
else:
    assert '--die-with-parent' in sys.argv
    fd = next(int(a.split('=')[1]) for a in sys.argv if a.startswith('--instance-id-fd='))
    state.write_text('running')
    os.write(fd, b'424242\\n')
    if '--exit-immediately' not in sys.argv: time.sleep(60)
''')
        binary.chmod(0o700)
        return binary

    def test_timeout_stops_only_owned_instance_and_verifies_absence(self):
        with tempfile.TemporaryDirectory(prefix="emuwiz-pcsx2-probe-test-") as tmp:
            root = Path(tmp)
            binary = self.fake_flatpak(root)
            report = run_bounded([str(binary), "run"], root / "boot.log", dict(os.environ), .2)
            self.assertTrue(report["bounded_stop"])
            self.assertTrue(report["instance_absent_after_stop"])
            self.assertEqual(binary.with_suffix(".killed").read_text(), "424242")
            self.assertFalse(binary.with_suffix(".running").exists())

    def test_host_exit_does_not_skip_sandbox_instance_cleanup(self):
        with tempfile.TemporaryDirectory(prefix="emuwiz-pcsx2-probe-test-") as tmp:
            root = Path(tmp)
            binary = self.fake_flatpak(root)
            report = run_bounded([str(binary), "run", "--exit-immediately"],
                                 root / "boot.log", dict(os.environ), 3)
            self.assertEqual(report["exit_code"], 0)
            self.assertTrue(report["instance_absent_after_stop"])
            self.assertEqual(binary.with_suffix(".killed").read_text(), "424242")

    def test_interrupted_probe_cleans_owned_instance(self):
        with tempfile.TemporaryDirectory(prefix="emuwiz-pcsx2-probe-test-") as tmp:
            root = Path(tmp)
            binary = self.fake_flatpak(root)
            timer = threading.Timer(.3, lambda: os.kill(os.getpid(), signal.SIGINT))
            timer.start()
            try:
                with self.assertRaises(KeyboardInterrupt):
                    run_bounded([str(binary), "run"], root / "boot.log", dict(os.environ), 5)
            finally:
                timer.cancel()
                timer.join()
            self.assertEqual(binary.with_suffix(".killed").read_text(), "424242")
            self.assertFalse(binary.with_suffix(".running").exists())

    def test_observed_consumption_and_enablement_never_authorize_production(self):
        _, crc = fixture_elf()
        root = Path("/private/PCSX2")
        report = observations(
            f"DataRoot Directory: {root}\nCheats Directory: {root}/cheats\n"
            f"Found 1 cheats in {root}/cheats/{crc}.pnach.\n"
            "Enabled patch: EMUWIZ Probe\n", root, crc)
        self.assertTrue(report["data_root_observed"])
        self.assertTrue(report["pnach_consumption_observed"])
        self.assertTrue(report["named_cheat_enabled_observed"])
        self.assertEqual(report["in_game_effect"], "NOT CONFIRMED")
        self.assertEqual(report["production_support"], "UNSUPPORTED_PENDING_PRESERVATION_PROOF")
        self.assertTrue(report["missing_proof"])

    def test_wrong_path_or_other_cheat_is_not_positive_evidence(self):
        _, crc = fixture_elf()
        report = observations(
            "DataRoot Directory: /other\nFound 1 cheats in /other/cheats.pnach.\n"
            "Enabled patch: EMUWIZ Probe plus another\n", Path("/private/PCSX2"), crc)
        self.assertFalse(report["data_root_observed"])
        self.assertFalse(report["pnach_consumption_observed"])
        self.assertFalse(report["named_cheat_enabled_observed"])

    def test_bios_source_is_only_read_and_existing_destination_refused(self):
        with tempfile.TemporaryDirectory(prefix="emuwiz-pcsx2-probe-test-") as tmp:
            root = Path(tmp)
            source, target = root / "source", root / "copy"
            source.write_bytes(b"synthetic BIOS fixture")
            expected = hashlib.sha256(source.read_bytes()).hexdigest()
            self.assertEqual(copy_bios(source, target), expected)
            self.assertEqual(source.read_bytes(), target.read_bytes())
            with self.assertRaises(FileExistsError):
                copy_bios(source, target)
            link = root / "link"
            link.symlink_to(source)
            with self.assertRaises(OSError):
                copy_bios(link, root / "must-not-exist")
            self.assertFalse((root / "must-not-exist").exists())

    def test_fifo_is_refused_without_blocking(self):
        import os
        with tempfile.TemporaryDirectory(prefix="emuwiz-pcsx2-probe-test-") as tmp:
            root = Path(tmp)
            source = root / "fifo"
            os.mkfifo(source)
            with self.assertRaises(ValueError):
                copy_bios(source, root / "must-not-exist")


if __name__ == "__main__":
    unittest.main()
