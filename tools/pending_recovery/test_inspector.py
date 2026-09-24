#!/usr/bin/env python3
"""Disposable self-tests for the pending-operation recovery inspector."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("inspector.py")
SPEC = importlib.util.spec_from_file_location("pending_recovery_inspector", MODULE_PATH)
assert SPEC and SPEC.loader
inspector = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = inspector
SPEC.loader.exec_module(inspector)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def shared_path(path: Path) -> dict[str, str]:
    return {"display": os.fspath(path), "unix_bytes_hex": os.fsencode(path).hex()}


class Fixture:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.data = root / "data"
        self.config = root / "config"
        self.sources = root / "sources"
        self.destinations = root / "destinations"
        self.backups = root / "backups"
        self.es_de_home = root / "ES-DE"
        self.es_de_gamelists = self.es_de_home / "gamelists"
        for path in (self.data, self.config, self.sources, self.destinations, self.backups, self.es_de_gamelists):
            path.mkdir(parents=True, exist_ok=True)

    def write_json(self, relative: str, value: dict) -> Path:
        path = self.data / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, sort_keys=True), encoding="utf-8")
        return path

    def durable(
        self,
        operation_id: str,
        transaction_state: str,
        entry_state: str,
        *,
        schema: int = 1,
        destination_bytes: bytes | None = None,
        backup_bytes: bytes | None = None,
        source_bytes: bytes = b"synthetic-new",
        destination: Path | None = None,
        backup: Path | None = None,
    ) -> Path:
        source = self.sources / f"{operation_id}.bin"
        source.write_bytes(source_bytes)
        destination = destination or self.destinations / f"{operation_id}.bin"
        backup = backup or self.backups / f"{operation_id}.bak"
        if destination_bytes is not None:
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(destination_bytes)
        if backup_bytes is not None:
            backup.parent.mkdir(parents=True, exist_ok=True)
            backup.write_bytes(backup_bytes)
        value = {
            "schema_version": schema,
            "transaction_state": transaction_state,
            "operation_id": operation_id,
            "operation_type": "shared_mod_apply",
            "plan_id": f"plan-{operation_id}",
            "timestamp_unix_seconds": 1_700_000_000,
            "context": {
                "adapter": "local_mod_package",
                "selected_archive": shared_path(self.sources / "package.zip"),
                "verified_game_identity": "SYNTHETIC-0001",
                "profile_id": "test",
                "source_mode": "fixture",
            },
            "approved_source_root": shared_path(self.sources),
            "destination_root": shared_path(self.destinations),
            "backup_root": shared_path(self.backups),
            "entries": [{
                "plan_entry": {
                    "adapter": "local_mod_package",
                    "selected_archive": shared_path(self.sources / "package.zip"),
                    "verified_game_identity": "SYNTHETIC-0001",
                    "source_path": shared_path(source),
                    "source_digest": digest(source_bytes),
                    "destination_root": shared_path(self.destinations),
                    "destination_relative_path": shared_path(destination.relative_to(self.destinations)),
                    "destination_pre_state": "regular_file_different",
                    "destination_pre_digest": digest(b"synthetic-old"),
                    "proposed_action": "replace_existing",
                    "backup_required": True,
                    "parent_creation_approved": False,
                    "content_verification": {"kind": "local_mod_package"},
                },
                "state": entry_state,
                "observed_destination_digest": digest(b"synthetic-old"),
                "backup_path": shared_path(backup),
                "backup_digest": digest(b"synthetic-old"),
                "resulting_destination_digest": digest(source_bytes) if destination_bytes == source_bytes else None,
                "temporary_path": None,
                "destination_existed_before_apply": True,
            }],
            "created_root_directories": [],
            "rollback_operation_id": None,
            "rollback_of_operation_id": None,
        }
        return self.write_json(f"shared-cheat-history/{operation_id}.pending.json", value)

    def rename(
        self,
        operation_id: str,
        state: str,
        entry_state: str,
        *,
        quarantine: bool = False,
        exact_resume: bool = False,
    ) -> Path:
        scan_root = self.root / f"rename-{operation_id}"
        scan_root.mkdir()
        source = scan_root / "before.rom"
        destination = scan_root / (".emuwiz-quarantine/item.rom" if quarantine else "after.rom")
        payload = b"renamed synthetic payload"
        if entry_state in {"applied", "rolling_back", "rollback_failed"}:
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(payload)
            identity_path = destination
        else:
            source.write_bytes(payload)
            identity_path = source
        info = identity_path.lstat()
        identity = {
            "size_bytes": info.st_size,
            "modified_unix": int(info.st_mtime),
            "kind": "regular_file",
            "ino": info.st_ino,
            "dev": info.st_dev,
            "freshness": {"version": 1, "sha256": list(hashlib.sha256(payload).digest())},
        }
        value = {
            "transaction_id": operation_id,
            "plan_generation": 1,
            "classifier_version": "fixture",
            "created_at_unix": 1_700_000_000,
            "source_scan_root": os.fspath(scan_root),
            "state": state,
            "entries": [{
                "source_path": os.fspath(source),
                "destination_path": os.fspath(destination),
                "original_basename": source.name,
                "proposed_basename": destination.name,
                "identity": identity,
                "operation": {"kind": "rename_move"},
                "state": entry_state,
            }],
            "created_directories": [],
        }
        if exact_resume:
            value["emuwiz_exact_resume_envelope"] = {
                "format_version": 1,
                "transaction_id": operation_id,
                "operations": [],
            }
            value["emuwiz_exact_resume_state"] = "interrupted"
        return self.write_json(f"rename-transactions/{operation_id}.json", value)

    def cheat(self, operation_id: str, *, rollback: bool, complete: bool) -> Path:
        root_name = "cheat-rollback-runs" if rollback else "cheat-install-runs"
        value = {
            "schema_version": 1,
            "run_id": operation_id,
            "status": "failed" if not complete else "success",
            "started_at_unix_seconds": 1_700_000_000,
            "completed_at_unix_seconds": 1_700_000_001 if complete else None,
            "destination_root": {"display": os.fspath(self.destinations), "lossy": False},
            "entries": [],
        }
        return self.write_json(f"{root_name}/{operation_id}.json", value)

    def report(self, include_history: bool = False) -> dict:
        return inspector.inspect_all(
            self.data,
            self.config,
            include_history,
            1024 * 1024,
            [self.es_de_home],
        )

    def database_restore(
        self,
        operation_id: str,
        state: str,
        *,
        live_bytes: bytes = b"database-before",
        selected_bytes: bytes = b"database-selected",
        emergency_bytes: bytes | None = None,
        applied_bytes: bytes | None = None,
        schema: int = 1,
    ) -> Path:
        live = self.data / "library.sqlite3"
        selected = self.data / "selected-backup.sqlite3"
        selected.write_bytes(selected_bytes)
        if applied_bytes is not None:
            live.write_bytes(applied_bytes)
        else:
            live.write_bytes(live_bytes)
        emergency = self.data / "library.sqlite3.before-restore.backup"
        if emergency_bytes is not None:
            emergency.write_bytes(emergency_bytes)
        value = {
            "schema_version": schema,
            "operation_id": operation_id,
            "state": state,
            "plan": {
                "plan_id": operation_id,
                "generated_at_unix": 1_700_000_000,
                "live_database_path": os.fspath(live),
                "selected_backup_path": os.fspath(selected),
                "selected_backup_sha256": digest(selected_bytes),
                "backup_schema_version": 1,
                "expected_live_size_bytes": len(live_bytes),
                "expected_live_modified_unix_seconds": None,
                "expected_live_sha256": digest(live_bytes),
                "current_live_schema_version": 1,
            },
            "emergency_backup_path": os.fspath(emergency) if emergency_bytes is not None else None,
            "emergency_backup_sha256": digest(emergency_bytes) if emergency_bytes is not None else None,
            "applied_database_sha256": digest(applied_bytes) if applied_bytes is not None else None,
            "created_at_unix": 1_700_000_000,
            "updated_at_unix": 1_700_000_001,
            "message": "synthetic",
        }
        return self.write_json(f"library.sqlite3.restore-{operation_id}.json", value)

    def es_de_recovery(
        self,
        system: str,
        previous: str | None,
        current: str | None,
        *,
        schema: int = 1,
    ) -> Path:
        directory = self.es_de_gamelists / system
        directory.mkdir()
        gamelist = directory / "gamelist.xml"
        if current is not None:
            gamelist.write_text(current, encoding="utf-8")
        recovery = gamelist.with_name(gamelist.name + inspector.ES_DE_RECOVERY_SUFFIX)
        recovery.write_text(json.dumps({
            "schema_version": schema,
            "gamelist_path": os.fspath(gamelist),
            "previous_content": previous,
        }), encoding="utf-8")
        return recovery


def filesystem_snapshot(root: Path) -> list[tuple]:
    result = []
    for path in sorted(root.rglob("*"), key=lambda item: os.fsencode(os.fspath(item))):
        info = path.lstat()
        relative = os.fspath(path.relative_to(root))
        if path.is_symlink():
            payload = ("symlink", os.readlink(path))
        elif path.is_file():
            payload = ("file", path.read_bytes())
        else:
            payload = ("other",)
        result.append((relative, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ino, payload))
    return result


class InspectorTests(unittest.TestCase):
    def test_completed_only_exits_zero_and_history_is_optional(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.durable("complete", "applied", "applied", destination_bytes=b"synthetic-new", backup_bytes=b"synthetic-old")
            report = fx.report()
            self.assertEqual(inspector.exit_code(report), 0)
            self.assertEqual(report["operations"], [])
            history = fx.report(include_history=True)
            self.assertEqual(history["summary"]["completed_history"], 1)
            self.assertEqual(history["operations"][0]["category"], "COMPLETED_HISTORY")

    def test_pending_without_proven_action_requires_review(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.durable("planned", "planned", "planned", destination_bytes=b"synthetic-old")
            report = fx.report()
            self.assertEqual(inspector.exit_code(report), 2)
            self.assertEqual(report["summary"]["review_required"], 1)
            self.assertFalse(report["operations"][0]["resume_possible"])

    def test_exact_matching_backup_and_destination_is_rollback_candidate(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.durable("safe", "apply_failed", "applied", destination_bytes=b"synthetic-new", backup_bytes=b"synthetic-old")
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["suggested_status"], "SAFE_ROLLBACK_CANDIDATE")
            self.assertTrue(operation["rollback_possible"])
            self.assertEqual(inspector.exit_code(fx.report()), 1)

    def test_missing_or_changed_evidence_blocks_rollback(self) -> None:
        for name, destination, backup in (
            ("missing-backup", b"synthetic-new", None),
            ("changed-destination", b"unrelated change", b"synthetic-old"),
            ("changed-backup", b"synthetic-new", b"wrong old content"),
        ):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                fx = Fixture(Path(temporary))
                fx.durable(name, "apply_failed", "applied", destination_bytes=destination, backup_bytes=backup)
                operation = fx.report()["operations"][0]
                self.assertFalse(operation["rollback_possible"])
                self.assertEqual(operation["category"], "REVIEW_REQUIRED")
                self.assertEqual(inspector.exit_code(fx.report()), 2)

    def test_unknown_schema_and_corrupt_journal_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.durable("future", "applying", "applied", schema=99)
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["state"], "UnknownSchema")
            self.assertEqual(operation["suggested_status"], "DO_NOT_TOUCH")
            self.assertEqual(inspector.exit_code(fx.report()), 2)
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            path = fx.data / "rename-transactions/broken.json"
            path.parent.mkdir()
            path.write_bytes(b"{not-json")
            report = fx.report()
            self.assertEqual(report["operations"][0]["state"], "Unreadable")
            self.assertEqual(inspector.exit_code(report), 3)

    def test_unsafe_symlink_is_do_not_touch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            redirected = fx.root / "redirected"
            redirected.mkdir()
            link = fx.destinations / "unsafe-parent"
            link.symlink_to(redirected, target_is_directory=True)
            destination = link / "unsafe.bin"
            fx.durable("unsafe", "apply_failed", "applied", destination=destination, destination_bytes=b"synthetic-new", backup_bytes=b"synthetic-old")
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["suggested_status"], "DO_NOT_TOUCH")
            self.assertEqual(inspector.exit_code(fx.report()), 2)

    def test_required_fixture_states_and_families_are_recognized(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.rename("applying-rename", "applying", "planned", exact_resume=True)
            fx.rename("failed-rename", "apply_failed", "applied", quarantine=True)
            fx.rename("rolling-rename", "rolling_back", "rolling_back")
            fx.rename("rollback-failed-rename", "rollback_failed", "rollback_failed")
            fx.durable("partial-mod", "applying", "destination_replaced", destination_bytes=b"synthetic-new", backup_bytes=b"synthetic-old")
            fx.cheat("incomplete-cheat", rollback=False, complete=False)
            fx.cheat("incomplete-cheat-rollback", rollback=True, complete=False)
            report = fx.report()
            states = {item["state"] for item in report["operations"]}
            subsystems = {item["subsystem"] for item in report["operations"]}
            self.assertTrue({"Applying", "ApplyFailed", "RollingBack", "RollbackFailed"}.issubset(states))
            self.assertIn("Duplicate Quarantine", subsystems)
            self.assertIn("Cheat Install", subsystems)
            self.assertIn("Cheat Rollback", subsystems)
            exact = next(item for item in report["operations"] if item["operation_id"] == "applying-rename")
            self.assertTrue(any(item.get("kind") == "exact_resume_envelope" for item in exact["evidence"]))
            partial = next(item for item in report["operations"] if item["operation_id"] == "partial-mod")
            self.assertTrue(partial["entry_checkpoints"][0]["replacement_published"])
            self.assertEqual(partial["suggested_status"], "SAFE_ROLLBACK_CANDIDATE")

    def test_checkpoint_gap_reconciles_expected_output_without_resuming(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.durable("checkpoint-gap", "applying", "applying", destination_bytes=b"synthetic-new", backup_bytes=b"synthetic-old")
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["entry_checkpoints"][0]["reconciled_state"], "applied_after_checkpoint_gap")
            self.assertEqual(operation["suggested_status"], "SAFE_ROLLBACK_CANDIDATE")
            self.assertFalse(operation["resume_possible"])

    def test_playing_library_leaf_symlink_and_recovery_sidecar(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            source = fx.sources / "playing-source.rom"
            source.write_bytes(b"playing library source")
            playing_root = fx.root / "playing"
            playing_root.mkdir()
            destination = playing_root / "Game.rom"
            destination.symlink_to(source)
            info = source.lstat()
            journal = {
                "transaction_id": "playing-link",
                "plan_generation": 1,
                "created_at_unix": 1_700_000_000,
                "source_scan_root": os.fspath(fx.sources),
                "state": "apply_failed",
                "entries": [{
                    "source_path": os.fspath(source),
                    "destination_path": os.fspath(destination),
                    "original_basename": source.name,
                    "proposed_basename": destination.name,
                    "identity": {
                        "size_bytes": info.st_size,
                        "modified_unix": int(info.st_mtime),
                        "kind": "regular_file",
                        "ino": info.st_ino,
                        "dev": info.st_dev,
                        "freshness": {"version": 1, "sha256": list(hashlib.sha256(source.read_bytes()).digest())},
                    },
                    "operation": {
                        "kind": "create_symlink",
                        "expected_target": os.fspath(source),
                        "destination_root": os.fspath(playing_root),
                    },
                    "state": "applied",
                }],
                "created_directories": [],
            }
            fx.write_json("rename-transactions/playing-link.json", journal)
            fx.write_json("rename-transactions/recovery-history-state", {"archived_transaction_ids": ["old-id"]})
            report = fx.report(include_history=True)
            link = next(item for item in report["operations"] if item["operation_id"] == "playing-link")
            self.assertEqual(link["subsystem"], "Playing Library Link")
            self.assertEqual(link["suggested_status"], "SAFE_ROLLBACK_CANDIDATE")
            sidecar = next(item for item in report["operations"] if item["subsystem"] == "Recovery History Sidecar")
            self.assertEqual(sidecar["category"], "COMPLETED_HISTORY")

    def test_shared_rollback_receipt_is_completed_history(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            receipt = {
                "schema_version": 1,
                "preview_id": "synthetic-preview",
                "journal_path": shared_path(fx.data / "shared-cheat-history/original.json"),
                "original_operation_id": "original-operation",
                "destination_root": shared_path(fx.destinations),
                "entries": [{
                    "destination": shared_path(fx.destinations / "restored.bin"),
                    "backup": shared_path(fx.backups / "restored.bak"),
                    "expected_installed_digest": digest(b"new"),
                    "observed_destination_digest": digest(b"new"),
                    "observed_backup_digest": digest(b"old"),
                    "outcome": "restored_backup",
                    "failure": None,
                }],
                "available": True,
            }
            fx.write_json("shared-cheat-history/original-operation.rollback.json", receipt)
            self.assertEqual(inspector.exit_code(fx.report()), 0)
            history = fx.report(include_history=True)
            self.assertEqual(history["summary"]["completed_history"], 1)
            self.assertEqual(history["operations"][0]["state"], "RolledBack")

    def test_library_view_history_is_completed_not_pending(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.write_json("library_views/history/0001.json", {
                "schema_version": 1,
                "timestamp": "2026-01-01T00:00:00Z",
                "operation": "Apply",
                "view_id": "romm-synthetic",
                "view_name": "Synthetic RomM",
                "profile_kind": "Romm",
                "destination_root": os.fspath(fx.root / "romm"),
                "manifest_path": os.fspath(fx.data / "library_views/romm.manifest.json"),
                "planned_count": 2,
                "created": 2,
                "repaired": 0,
                "removed": 0,
                "unchanged": 0,
                "failed": 0,
                "skipped_or_collision": 0,
                "success": True,
                "warnings": [],
            })
            self.assertEqual(inspector.exit_code(fx.report()), 0)
            self.assertEqual(fx.report()["operations"], [])
            history = fx.report(include_history=True)
            operation = history["operations"][0]
            self.assertEqual(operation["subsystem"], "RomM Library View History")
            self.assertEqual(operation["state"], "Completed")
            self.assertEqual(operation["suggested_status"], "NO_ACTION")

    def test_empty_root_exits_zero(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            report = Fixture(Path(temporary)).report()
            self.assertEqual(report["summary"]["pending"], 0)
            self.assertEqual(inspector.exit_code(report), 0)

    def test_database_restore_exact_resume_and_rollback_candidates(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.database_restore("resume", "validating")
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["subsystem"], "Database Restore")
            self.assertEqual(operation["recommended_status"], "SAFE_RESUME_CANDIDATE")
            self.assertTrue(operation["resume_possible"])
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.database_restore(
                "rollback",
                "restore_applied",
                emergency_bytes=b"database-before",
                applied_bytes=b"database-selected",
            )
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["recommended_status"], "SAFE_ROLLBACK_CANDIDATE")
            self.assertTrue(operation["rollback_possible"])

    def test_database_restore_missing_backup_requires_review(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.database_restore("missing-emergency", "restore_applied", applied_bytes=b"database-selected")
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["recommended_status"], "REVIEW_REQUIRED")
            self.assertFalse(operation["rollback_possible"])

    def test_es_de_exact_prior_state_is_candidate_but_divergence_is_review(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.es_de_recovery("psx", "<gameList/>", "<gameList/>")
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["subsystem"], "ES-DE Publication Recovery")
            self.assertEqual(operation["recommended_status"], "SAFE_ROLLBACK_CANDIDATE")
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.es_de_recovery("psx", "<gameList/>", "<gameList><game/></gameList>")
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["recommended_status"], "REVIEW_REQUIRED")
            self.assertIn("later user change", operation["needs_review_reason"])

    def test_es_de_unknown_schema_is_never_safe(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.es_de_recovery("psx", None, None, schema=99)
            operation = fx.report()["operations"][0]
            self.assertEqual(operation["normalized_state"], "UnknownSchema")
            self.assertEqual(operation["recommended_status"], "DO_NOT_TOUCH")

    def test_inspection_does_not_mutate_fixture_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.durable("readonly", "apply_failed", "applied", destination_bytes=b"synthetic-new", backup_bytes=b"synthetic-old")
            fx.database_restore("readonly-database", "validating")
            fx.es_de_recovery("psx", "<gameList/>", "<gameList/>")
            before = filesystem_snapshot(fx.root)
            fx.report(include_history=True)
            after = filesystem_snapshot(fx.root)
            self.assertEqual(before, after)

    def test_cli_writes_valid_descriptive_outputs_only_when_requested(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            fx.durable("safe-plan", "apply_failed", "applied", destination_bytes=b"synthetic-new", backup_bytes=b"synthetic-old")
            report_path = fx.root / "report.json"
            plan_path = fx.root / "plan.json"
            before = filesystem_snapshot(fx.data)
            result = subprocess.run(
                [sys.executable, os.fspath(MODULE_PATH), "--data-root", os.fspath(fx.data), "--config-root", os.fspath(fx.config), "--es-de-root", os.fspath(fx.es_de_home), "--verbose", "--json", os.fspath(report_path), "--emit-plan", os.fspath(plan_path)],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertEqual(before, filesystem_snapshot(fx.data))
            report = json.loads(report_path.read_text(encoding="utf-8"))
            plan = json.loads(plan_path.read_text(encoding="utf-8"))
            self.assertTrue(report["read_only"])
            self.assertTrue(plan["descriptive_only"])
            self.assertFalse(plan["executes_actions"])
            self.assertEqual(plan["operations"][0]["recommended_action"], "SAFE_ROLLBACK_CANDIDATE")
            self.assertNotIn("command", plan["operations"][0])
            self.assertIn("EMUWIZ RECOVERY INSPECTOR", result.stdout)
            self.assertIn("No action has been performed.", result.stdout)

    def test_output_inside_data_root_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fx = Fixture(Path(temporary))
            result = subprocess.run(
                [sys.executable, os.fspath(MODULE_PATH), "--data-root", os.fspath(fx.data), "--config-root", os.fspath(fx.config), "--json", os.fspath(fx.data / "forbidden.json")],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 3)
            self.assertFalse((fx.data / "forbidden.json").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
