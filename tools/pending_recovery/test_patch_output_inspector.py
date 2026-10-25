#!/usr/bin/env python3
"""Synthetic, legal fixtures for the read-only patch-output projection."""
from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from inspector import inspect_all, inspect_patch_output


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class PatchOutputInspectorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.source = self.root / "source.bin"; self.source.write_bytes(b"source-fixture")
        self.patch = self.root / "patch.ips"; self.patch.write_bytes(b"patch-fixture")
        self.destination = self.root / "output.bin"
        self.temporary = self.root / "output.bin.tmp"
        self.provenance = self.root / "output.bin.emuwiz-patch.json"
        self.journal = self.root / ".emuwiz-patch-output-fixture.json"

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write(self, state: str, checkpoints: list[str], *, output: bool = False, temp: bool = False) -> dict:
        output_bytes = b"patched-fixture"
        if output: self.destination.write_bytes(output_bytes)
        if temp: self.temporary.write_bytes(output_bytes)
        if output: self.provenance.write_text("fixture provenance\n")
        value = {
            "schema_version": 1, "operation_id": "fixture", "created_at_unix": 1, "updated_at_unix": 2,
            "patch_format": "ips", "source_path": str(self.source), "source_size": self.source.stat().st_size,
            "source_sha256": digest(self.source), "patch_path": str(self.patch), "patch_size": self.patch.stat().st_size,
            "patch_sha256": digest(self.patch), "destination_path": str(self.destination),
            "temporary_output_path": str(self.temporary), "provenance_path": str(self.provenance),
            "provenance_sha256": digest(self.provenance) if output else None,
            "expected_output_size": len(output_bytes), "expected_output_sha256": hashlib.sha256(output_bytes).hexdigest(),
            "destination_preexisting": False, "previous_destination_sha256": None, "backup_path": None,
            "state": state, "checkpoints": checkpoints, "failure_reason": None,
        }
        self.journal.write_text(json.dumps(value))
        return value

    def inspect(self, **kwargs):
        value = self.write(**kwargs)
        return inspect_patch_output(self.journal, value, __import__("inspector").HashBudget(16 * 1024 * 1024))

    def test_intent_and_partial_temp_need_review(self) -> None:
        self.assertEqual(self.inspect(state="planned", checkpoints=["intent_durable"])["suggested_status"], "REVIEW_REQUIRED")
        self.write(state="verifying_temporary", checkpoints=["temporary_write_started"], temp=True)
        self.temporary.write_bytes(b"partial-output")
        result = inspect_patch_output(self.journal, json.loads(self.journal.read_text()), __import__("inspector").HashBudget(16 * 1024 * 1024))
        self.assertEqual(result["suggested_status"], "REVIEW_REQUIRED")

    def test_verified_temp_and_before_publish_are_safe_resume(self) -> None:
        for state, checkpoint in (("prepared", "prepared"), ("verifying_temporary", "temporary_verified"), ("publishing", "before_publish")):
            result = self.inspect(state=state, checkpoints=[checkpoint], temp=True)
            self.assertEqual(result["suggested_status"], "SAFE_RESUME_CANDIDATE")
            self.assertTrue(result["resume_possible"])

    def test_published_before_final_verification_is_rollback_candidate(self) -> None:
        result = self.inspect(state="published", checkpoints=["published"], output=True)
        self.assertEqual(result["suggested_status"], "SAFE_ROLLBACK_CANDIDATE")
        self.assertTrue(result["rollback_possible"])
        self.assertTrue(result["resume_possible"])

    def test_completed_and_rolled_back_are_history(self) -> None:
        for state in ("completed", "rolled_back"):
            result = self.inspect(state=state, checkpoints=["before_completed"])
            self.assertEqual(result["state"], "Completed" if state == "completed" else "RolledBack")
            self.assertEqual(result["suggested_status"], "NO_ACTION")

    def test_changed_source_or_patch_is_do_not_touch(self) -> None:
        self.write(state="prepared", checkpoints=["prepared"], temp=True)
        self.source.write_bytes(b"changed-source")
        value = json.loads(self.journal.read_text())
        result = inspect_patch_output(self.journal, value, __import__("inspector").HashBudget(16 * 1024 * 1024))
        self.assertEqual(result["suggested_status"], "DO_NOT_TOUCH")
        self.assertEqual(result["state"], "UnsafeToResume")
        self.write(state="prepared", checkpoints=["prepared"], temp=True)
        self.patch.write_bytes(b"changed-patch")
        value = json.loads(self.journal.read_text())
        result = inspect_patch_output(self.journal, value, __import__("inspector").HashBudget(16 * 1024 * 1024))
        self.assertEqual(result["suggested_status"], "DO_NOT_TOUCH")

    def test_changed_destination_requires_review(self) -> None:
        self.write(state="published", checkpoints=["published"], output=True)
        self.destination.write_bytes(b"changed-destination")
        value = json.loads(self.journal.read_text())
        result = inspect_patch_output(self.journal, value, __import__("inspector").HashBudget(16 * 1024 * 1024))
        self.assertEqual(result["suggested_status"], "REVIEW_REQUIRED")
        self.assertFalse(result["rollback_possible"])

    def test_missing_temp_unknown_schema_and_corrupt_journal_are_safe_failures(self) -> None:
        result = self.inspect(state="prepared", checkpoints=["prepared"])
        self.assertEqual(result["suggested_status"], "REVIEW_REQUIRED")
        unknown = self.write(state="prepared", checkpoints=["prepared"]); unknown["schema_version"] = 99
        self.assertEqual(inspect_patch_output(self.journal, unknown, __import__("inspector").HashBudget(16 * 1024 * 1024))["state"], "UnknownSchema")
        self.journal.write_text("{not-json")
        report = inspect_all(self.root, self.root, False, 16 * 1024 * 1024)
        self.assertTrue(any(item["state"] == "Unreadable" for item in report["operations"]))

    def test_discovery_is_read_only_and_reports_patch_details(self) -> None:
        before = self.journal.exists()
        self.write(state="prepared", checkpoints=["prepared"], temp=True)
        report = inspect_all(self.root, self.root, False, 16 * 1024 * 1024)
        self.assertEqual(len(report["operations"]), 1)
        item = report["operations"][0]
        self.assertEqual(item["original_format"], "patch_output_journal")
        self.assertEqual(item["patch_format"], "ips")
        self.assertIn("filesystem_evidence", item)
        self.assertEqual(before, False)


if __name__ == "__main__":
    unittest.main()
