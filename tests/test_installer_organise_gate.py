#!/usr/bin/env python3
"""install.sh refuses to change EmuWiz binaries while Organise has unfinished
game-folder operations (see ORGANISE RECOVERY GATE in install.sh).

Every case runs a private copy of install.sh against a synthetic bundle and a
disposable HOME, data and prefix tree; nothing here touches a real
installation.
"""
import json
import os
from pathlib import Path
import unittest

from test_installer_upgrade_preservation import (
    NEW_CLI,
    NEW_GUI,
    OLD_CLI,
    OLD_GUI,
    UpgradePreservationTests,
)


class OrganiseGateTests(UpgradePreservationTests):
    """Reuses the preservation tests' fixture; only the tests below run here."""

    def setUp(self):
        super().setUp()
        # The gate reads EMUWIZ_DATA_HOME: never the developer's own.
        self.env.pop("EMUWIZ_DATA_HOME", None)

    def record(self, state, root=None, journal="rename-transactions-sealed-v1", name="folder-move-1.json", text=None):
        folder = (root or self.data / "emuwiz") / journal / "folder-moves"
        folder.mkdir(parents=True, exist_ok=True)
        path = folder / name
        path.write_text(text if text is not None else json.dumps({"version": 1, "id": name[:-5], "state": state}))
        return path

    def assert_refused(self, result, *needles):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("unfinished game-folder operations", result.stderr)
        self.assertIn("Nothing was installed, replaced or removed", result.stderr)
        self.assertIn("Finish interrupted folder moves", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        for needle in needles:
            self.assertIn(needle, result.stderr)

    def test_an_interrupted_folder_undo_blocks_a_downgrade_and_keeps_the_newer_binary(self):
        self.installed_old()  # the "newer" release that wrote the record
        path = self.record("Undoing")
        self.release(NEW_CLI, NEW_GUI)  # any other release, older or newer
        self.assert_refused(self.run_install(), str(path), "'Undoing'")
        self.assert_old_install_intact()
        self.assertFalse(self.previous.exists(), "nothing was preserved or published")
        # Settled by the installed EmuWiz: the install proceeds.
        self.record("Committed")
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), NEW_CLI)

    def test_uninstall_is_refused_too(self):
        self.installed_old()
        self.record("Planned")
        self.assert_refused(self.run_install("--uninstall"), "'Planned'")
        self.assert_old_install_intact()

    def test_unknown_unreadable_and_odd_records_block_in_every_location(self):
        cases = [
            ("unknown state", lambda: self.record("Relocating")),
            ("no state", lambda: self.record(None, text=json.dumps({"version": 1}))),
            ("damaged", lambda: self.record(None, text="{ not json")),
            ("legacy journal", lambda: self.record("Undoing", journal="rename-transactions")),
            ("legacy data root", lambda: self.record("Undoing", root=self.data / "archivefs")),
            ("not a file", lambda: (self.data / "emuwiz/rename-transactions-sealed-v1/folder-moves/x.json").mkdir(parents=True)),
            ("newer record version", lambda: self.record(None, text=json.dumps({"version": 2, "state": "Committed"}))),
            ("boolean version", lambda: self.record(None, text=json.dumps({"version": True, "state": "Committed"}))),
            ("no version", lambda: self.record(None, text=json.dumps({"state": "Committed"}))),
            ("list state", lambda: self.record(None, text=json.dumps({"version": 1, "state": ["Committed"]}))),
            ("object state", lambda: self.record(None, text=json.dumps({"version": 1, "state": {"x": 1}}))),
            ("not an object", lambda: self.record(None, text=json.dumps(["Committed"]))),
        ]
        for label, make in cases:
            with self.subTest(label):
                self.setUp()
                self.installed_old()
                make()
                self.release(NEW_CLI, NEW_GUI)
                self.assert_refused(self.run_install())
                self.assert_old_install_intact()

    def test_an_explicit_data_root_is_checked(self):
        self.installed_old()
        explicit = self.root / "explicit-data"
        self.env["EMUWIZ_DATA_HOME"] = str(explicit)
        path = self.record("Undoing", root=explicit)
        self.release(NEW_CLI, NEW_GUI)
        self.assert_refused(self.run_install(), str(path))
        self.assert_old_install_intact()

    def test_settled_and_review_records_do_not_block(self):
        for state in ("Committed", "Undone", "RolledBack"):
            self.record(state, name=f"folder-move-{state}.json")
        review = self.record("NeedsReview", name="folder-move-review.json")
        self.release(OLD_CLI, OLD_GUI)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("needs review", result.stderr)
        self.assertIn(str(review), result.stderr)

    def test_the_gate_only_reads(self):
        self.installed_old()
        path = self.record("Undoing")
        before = (path.read_bytes(), os.stat(path).st_mtime_ns)
        self.release(NEW_CLI, NEW_GUI)
        self.assert_refused(self.run_install())
        self.assertEqual((path.read_bytes(), os.stat(path).st_mtime_ns), before)


def load_tests(loader, tests, pattern):
    # Only the gate's own tests: the inherited ones run in their own file.
    suite = unittest.TestSuite()
    for name in loader.getTestCaseNames(OrganiseGateTests):
        if name in OrganiseGateTests.__dict__:
            suite.addTest(OrganiseGateTests(name))
    return suite

if __name__ == "__main__":
    unittest.main()
