#!/usr/bin/env python3
"""Self-tests for the synthetic library generator. Temp directories only."""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import tempfile
import unittest
import zipfile
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "synthetic_library.py"
SPEC = importlib.util.spec_from_file_location("synthetic_library", MODULE_PATH)
assert SPEC and SPEC.loader
lab = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(lab)


class SyntheticLibraryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="emuwiz-lab-selftest-")
        self.base = Path(self.temp.name)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def build(self, name: str, profile: str = "tiny", seed: int = 1) -> Path:
        root = self.base / name
        lab.build_lab(root, profile, seed, 0, 0, False)
        return root

    def test_same_seed_produces_identical_manifest(self) -> None:
        first = self.build("first")
        second = self.build("second")
        self.assertEqual((first / "manifest.json").read_bytes(), (second / "manifest.json").read_bytes())

    def test_different_seed_changes_payload_hashes(self) -> None:
        first = json.loads((self.build("one", seed=1) / "manifest.json").read_text())
        second = json.loads((self.build("two", seed=2) / "manifest.json").read_text())
        hashes_one = {item["fixture_id"]: item["sha256"] for item in first["fixtures"]}
        hashes_two = {item["fixture_id"]: item["sha256"] for item in second["fixtures"]}
        self.assertNotEqual(hashes_one["platform.nes.good"], hashes_two["platform.nes.good"])

    def test_output_escape_is_refused(self) -> None:
        with self.assertRaises(lab.LabError):
            lab.safe_relative("../escape.bin")
        with self.assertRaises(lab.LabError):
            lab.safe_relative("/absolute.bin")

    def test_unowned_existing_directory_is_refused(self) -> None:
        root = self.base / "unowned"
        root.mkdir()
        (root / "keep.txt").write_text("owner data")
        with self.assertRaises(lab.LabError):
            lab.build_lab(root, "tiny", 1, 0, 0, True)
        self.assertEqual((root / "keep.txt").read_text(), "owner data")

    def test_ownership_marker_is_required_for_cleanup(self) -> None:
        root = self.base / "unowned-cleanup"
        root.mkdir()
        with self.assertRaises(lab.LabError):
            lab.safe_remove(root, yes=True)
        self.assertTrue(root.exists())

    def test_traversal_member_remains_inside_archive(self) -> None:
        root = self.build("full", profile="full")
        archive = root / "library/Archives/Traversal.zip"
        with zipfile.ZipFile(archive) as handle:
            self.assertEqual(handle.namelist(), ["../escape.bin"])
        self.assertFalse((root.parent / "escape.bin").exists())

    def test_symlink_targets_are_lexically_inside_root(self) -> None:
        root = self.build("links")
        manifest, failures, _, _ = lab.verify_lab(root, write=False)
        self.assertEqual(failures, [])
        links = [item for item in manifest["fixtures"] if item["kind"] == "symlink"]
        self.assertGreaterEqual(len(links), 5)
        for item in links:
            path = root / item["relative_path"]
            lexical = Path(os.path.normpath(str(path.parent / os.readlink(path))))
            lexical.relative_to(root)

    def test_profile_size_cap_is_enforced(self) -> None:
        original = lab.PROFILE_CAP["tiny"]
        lab.PROFILE_CAP["tiny"] = 1
        root = self.base / "too-large"
        try:
            with self.assertRaises(lab.LabError):
                lab.build_lab(root, "tiny", 1, 0, 0, False)
        finally:
            lab.PROFILE_CAP["tiny"] = original
        lab.safe_remove(root, yes=True)

    def test_validator_detects_tampering(self) -> None:
        root = self.build("tamper")
        with (root / "library/Nintendo Entertainment System/Synthetic Nintendo Entertainment System.nes").open("ab") as handle:
            handle.write(b"tamper")
        _, failures, _, _ = lab.verify_lab(root, write=False)
        self.assertTrue(any("mismatch" in failure for failure in failures))

    def test_cleanup_refuses_dangerous_roots(self) -> None:
        with self.assertRaises(lab.LabError):
            lab.safe_remove(Path("/tmp"), yes=True)
        home = os.environ.get("HOME")
        if home:
            with self.assertRaises(lab.LabError):
                lab.safe_remove(Path(home), yes=True)

    def test_cleanup_succeeds_for_owned_root(self) -> None:
        root = self.build("remove")
        lab.safe_remove(root, yes=True)
        self.assertFalse(root.exists())

    def test_standard_profile_builds_and_validates_offline(self) -> None:
        root = self.build("standard", profile="standard")
        manifest, failures, _, _ = lab.verify_lab(root, write=False)
        self.assertEqual(failures, [])
        self.assertEqual(manifest["profile"], "standard")
        represented = {item["platform"] for item in manifest["fixtures"]}
        self.assertIn("Atari ST", represented)
        self.assertIn("PS3", represented)
        self.assertIn("Nintendo 3DS", represented)

    def test_incomplete_owned_root_can_be_cleaned(self) -> None:
        root = self.base / "incomplete"
        lab.prepare_root(root, "tiny", 1, False)
        shutil.rmtree(root / lab.LOCK)
        lab.safe_remove(root, yes=True)
        self.assertFalse(root.exists())

    def test_concurrent_generator_lock_is_refused(self) -> None:
        root = self.base / "locked"
        lab.prepare_root(root, "tiny", 1, False)
        with self.assertRaises(lab.LabError):
            lab.prepare_root(root, "tiny", 1, True)
        shutil.rmtree(root / lab.LOCK)
        lab.safe_remove(root, yes=True)

    def test_scale_safety_cap(self) -> None:
        self.assertEqual(lab.main(["build", "--output", str(self.base / "capped"), "--scale", "50001"]), 2)
        self.assertFalse((self.base / "capped").exists())


class CleanupScopeTests(unittest.TestCase):
    """Cleanup must stay inside the generated lab (added after the UX recovery pass)."""

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="emuwiz-lab-scope-")
        self.base = Path(self.temp.name)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def build(self, name: str) -> Path:
        root = self.base / name
        lab.build_lab(root, "tiny", 1, 0, 0, False)
        return root

    def test_cleanup_leaves_sibling_directories_untouched(self) -> None:
        sibling = self.base / "sibling"
        sibling.mkdir()
        (sibling / "keep.txt").write_text("not part of the lab")
        root = self.build("lab")
        lab.safe_remove(root, yes=True)
        self.assertFalse(root.exists())
        self.assertEqual((sibling / "keep.txt").read_text(), "not part of the lab")

    def test_cleanup_does_not_follow_a_symlink_out_of_the_lab(self) -> None:
        outside = self.base / "outside"
        outside.mkdir()
        (outside / "precious.txt").write_text("outside the lab")
        root = self.build("lab")
        (root / "stray-link").symlink_to(outside, target_is_directory=True)
        lab.safe_remove(root, yes=True)
        self.assertFalse(root.exists())
        self.assertEqual((outside / "precious.txt").read_text(), "outside the lab")

    def test_cleanup_refuses_a_symlink_to_an_owned_lab(self) -> None:
        root = self.build("lab")
        link = self.base / "link-to-lab"
        link.symlink_to(root, target_is_directory=True)
        with self.assertRaises(lab.LabError):
            lab.safe_remove(link, yes=True)
        self.assertTrue(root.is_dir())

    def test_cleanup_refuses_while_the_generator_lock_is_live(self) -> None:
        root = self.base / "live"
        lab.prepare_root(root, "tiny", 1, False)  # lock records this live process
        try:
            with self.assertRaises(lab.LabError):
                lab.safe_remove(root, yes=True)
            self.assertTrue(root.exists())
        finally:
            shutil.rmtree(root / lab.LOCK)
            lab.safe_remove(root, yes=True)

    def test_cleanup_requires_explicit_confirmation(self) -> None:
        root = self.build("lab")
        with self.assertRaises(lab.LabError):
            lab.safe_remove(root, yes=False)
        self.assertTrue(root.exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
