#!/usr/bin/env python3
"""Probe regressions; do not launch an emulator or read a real BIOS."""

import hashlib
from pathlib import Path
import tempfile
import unittest

from probe_pcsx2_temp_cheats import copy_bios, fixture_elf, observations


class ProbeTests(unittest.TestCase):
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
