#!/usr/bin/env python3
"""Read-only inspector for EmuWiz transaction journals.

This tool deliberately implements only bounded parsing and filesystem evidence
checks.  It never invokes a transaction engine and has no mutation command.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import hashlib
import json
import os
import stat
import sys
import time
from pathlib import Path, PurePath
from typing import Any, Iterable

TOOL_VERSION = "1.0.0"
REPORT_SCHEMA_VERSION = 1
# Large rename batches legitimately contain thousands of entry checkpoints.
# Keep parsing bounded without rejecting normal completed journals observed in
# the real data root (a 4,579-entry journal is about 3.6 MiB).
MAX_JOURNAL_BYTES = 16 * 1024 * 1024
MAX_ES_DE_RECOVERY_BYTES = 256 * 1024 * 1024
MAX_DIRECTORY_ENTRIES = 4096
MAX_PATCH_OUTPUT_JOURNALS = 512
PATCH_OUTPUT_PREFIX = ".emuwiz-patch-output-"
PATCH_OUTPUT_SUFFIX = ".json"
DEFAULT_MAX_HASH_BYTES = 16 * 1024 * 1024
DEFAULT_TOTAL_HASH_BYTES = 256 * 1024 * 1024
ES_DE_RECOVERY_SUFFIX = ".es-de-publish-recovery.json"

NORMAL_STATES = {
    "Completed", "RolledBack", "Applying", "ApplyFailed", "RollingBack",
    "RollbackFailed", "NeedsReview", "UnsafeToResume", "UnknownSchema", "Unreadable",
}


class InspectorError(RuntimeError):
    pass


@dataclasses.dataclass
class HashBudget:
    per_file: int
    remaining: int = DEFAULT_TOTAL_HASH_BYTES

    def hash_file(self, path: Path) -> tuple[str | None, str | None]:
        try:
            info = path.stat(follow_symlinks=False)
        except OSError as error:
            return None, f"cannot stat: {error}"
        if not stat.S_ISREG(info.st_mode):
            return None, "not a regular file"
        if info.st_size > self.per_file:
            return None, f"hash skipped: {info.st_size} bytes exceeds per-file limit {self.per_file}"
        if info.st_size > self.remaining:
            return None, "hash skipped: inspection hash budget exhausted"
        digest = hashlib.sha256()
        try:
            with path.open("rb") as handle:
                for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                    digest.update(chunk)
        except OSError as error:
            return None, f"cannot hash: {error}"
        self.remaining -= info.st_size
        return digest.hexdigest(), None


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def path_is_within(path: Path, root: Path) -> bool:
    try:
        path.absolute().relative_to(root.absolute())
        return True
    except ValueError:
        return False


def has_parent_component(path: Path) -> bool:
    return any(part == ".." for part in PurePath(os.fspath(path)).parts)


def decode_shared_path(value: Any) -> tuple[Path | None, str | None]:
    if not isinstance(value, dict) or not isinstance(value.get("display"), str):
        return None, "shared path record is missing its display value"
    encoded = value.get("unix_bytes_hex")
    if encoded is None:
        return Path(value["display"]), None
    if not isinstance(encoded, str):
        return None, "shared path byte encoding is not text"
    try:
        raw = bytes.fromhex(encoded)
    except ValueError:
        return None, "shared path contains invalid hexadecimal bytes"
    return Path(os.fsdecode(raw)), None


def decode_cheat_path(value: Any) -> tuple[Path | None, str | None]:
    if not isinstance(value, dict) or not isinstance(value.get("display"), str):
        return None, "cheat path record is malformed"
    if value.get("lossy") is True:
        return None, "cheat path is lossy and cannot authorize recovery"
    return Path(value["display"]), None


def symlink_component(path: Path, *, include_leaf: bool = True) -> str | None:
    """Return the first existing symlink component without following it."""
    if not path.is_absolute():
        return "path is not absolute"
    start = Path(path.anchor)
    parts = path.parts[1:] if include_leaf else path.parts[1:-1]
    for part in parts:
        start = start / part
        try:
            info = start.lstat()
        except FileNotFoundError:
            continue
        except OSError as error:
            return f"cannot inspect path component {start}: {error}"
        if stat.S_ISLNK(info.st_mode):
            return f"symlink component: {start}"
    return None


def inspect_path(
    path: Path | None,
    budget: HashBudget,
    *,
    expected_root: Path | None = None,
    expected_digest: str | None = None,
    hash_when_unrecorded: bool = False,
) -> dict[str, Any]:
    evidence: dict[str, Any] = {
        "path": os.fspath(path) if path is not None else None,
        "expected_root": os.fspath(expected_root) if expected_root else None,
        "exists": False,
        "kind": "missing",
        "symlink_target": None,
        "size": None,
        "expected_sha256": expected_digest,
        "observed_sha256": None,
        "identity_matches": None,
        "safe_path": False,
        "problem": None,
    }
    if path is None:
        evidence["problem"] = "path is unavailable or cannot be decoded losslessly"
        return evidence
    if not path.is_absolute() or has_parent_component(path):
        evidence["problem"] = "path is not an absolute normalized path"
        return evidence
    if expected_root is not None and not path_is_within(path, expected_root):
        evidence["problem"] = "path escapes its recorded root"
        return evidence
    # A leaf symlink is useful evidence for an operation which intentionally
    # created one.  A symlink in any parent component is never trusted because
    # it changes the filesystem object named by the recorded path.
    component_problem = symlink_component(path, include_leaf=False)
    try:
        info = path.lstat()
    except FileNotFoundError:
        evidence["safe_path"] = component_problem is None
        if component_problem:
            evidence["problem"] = component_problem
        return evidence
    except OSError as error:
        evidence["problem"] = f"cannot inspect: {error}"
        return evidence
    evidence["exists"] = True
    evidence["size"] = info.st_size
    if stat.S_ISLNK(info.st_mode):
        evidence["kind"] = "symlink"
        try:
            evidence["symlink_target"] = os.fspath(os.readlink(path))
        except OSError as error:
            evidence["problem"] = f"cannot read symlink: {error}"
        else:
            evidence["problem"] = component_problem or "leaf path is a symlink"
        return evidence
    if stat.S_ISREG(info.st_mode):
        evidence["kind"] = "regular_file"
    elif stat.S_ISDIR(info.st_mode):
        evidence["kind"] = "directory"
    else:
        evidence["kind"] = "special"
    if component_problem:
        evidence["problem"] = component_problem
        return evidence
    evidence["safe_path"] = True
    if evidence["kind"] == "regular_file" and (expected_digest or hash_when_unrecorded):
        digest, problem = budget.hash_file(path)
        evidence["observed_sha256"] = digest
        if problem:
            evidence["problem"] = problem
        if expected_digest and digest:
            evidence["identity_matches"] = digest.lower() == expected_digest.lower()
    return evidence


def read_json(path: Path, max_bytes: int = MAX_JOURNAL_BYTES) -> dict[str, Any]:
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        raise InspectorError("journal is not a regular non-symlink file")
    if info.st_size > max_bytes:
        raise InspectorError(f"journal exceeds {max_bytes}-byte limit")
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise InspectorError(f"journal cannot be read: {error}") from error
    try:
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise InspectorError(f"journal is not valid UTF-8 JSON: {error}") from error
    if not isinstance(value, dict):
        raise InspectorError("journal root is not a JSON object")
    return value


def new_operation(subsystem: str, operation_id: str, journal_path: Path) -> dict[str, Any]:
    return {
        "subsystem": subsystem,
        "operation_id": operation_id,
        "journal_path": os.fspath(journal_path),
        "state": "NeedsReview",
        "normalized_state": "NeedsReview",
        "original_state": None,
        "category": "REVIEW_REQUIRED",
        "created_at": None,
        "updated_at": None,
        "source_paths": [],
        "destination_paths": [],
        "backup_paths": [],
        "temporary_paths": [],
        "created_directories": [],
        "entries_total": 0,
        "entries_completed": 0,
        "entries_pending": 0,
        "rollback_possible": False,
        "resume_possible": False,
        "suggested_status": "DO_NOT_TOUCH",
        "recommended_status": "DO_NOT_TOUCH",
        "rollback_evidence": [],
        "resume_evidence": [],
        "needs_review_reason": None,
        "schema_version": None,
        "original_format": None,
        "evidence": [],
        "entry_checkpoints": [],
        "blockers": [],
    }


def add_unique(target: list[str], value: Path | str | None) -> None:
    if value is None:
        return
    text = os.fspath(value)
    if text not in target:
        target.append(text)


def map_state(original: str | None, *, family: str) -> str:
    normalized = (original or "").lower().replace("-", "_")
    common = {
        "applied": "Completed", "success": "Completed", "legacy_complete": "Completed",
        "rolled_back": "RolledBack", "applying": "Applying", "planned": "Applying",
        "apply_failed": "ApplyFailed", "partial_failure": "ApplyFailed", "failed": "ApplyFailed",
        "rolling_back": "RollingBack", "rollback_failed": "RollbackFailed",
        "needs_review": "NeedsReview", "unsafe_to_resume": "UnsafeToResume",
    }
    if family == "cheat_rollback" and normalized == "success":
        return "RolledBack"
    return common.get(normalized, "NeedsReview")


def classify_rename_subsystem(entries: list[dict[str, Any]]) -> str:
    destinations = [str(entry.get("destination_path", "")) for entry in entries]
    sources = [str(entry.get("source_path", "")) for entry in entries]
    kinds = []
    for entry in entries:
        operation = entry.get("operation", {})
        if isinstance(operation, dict):
            kinds.append(operation.get("kind", "rename_move"))
        elif isinstance(operation, str):
            kinds.append(operation)
    if any(any(part.startswith(".emuwiz-chd-") for part in Path(path).parts) for path in sources) or any("optical-conversion" in Path(path).parts for path in destinations):
        return "Disc Conversion"
    if any(".emuwiz-quarantine" in Path(path).parts for path in destinations):
        return "Duplicate Quarantine"
    if kinds and all(kind in {"create_symlink", "create_hardlink"} for kind in kinds):
        return "Playing Library Link"
    return "Rename / Organisation"


def expected_freshness_digest(identity: Any) -> str | None:
    if not isinstance(identity, dict):
        return None
    freshness = identity.get("freshness")
    if not isinstance(freshness, dict) or freshness.get("version") != 1:
        return None
    raw = freshness.get("sha256")
    if isinstance(raw, list) and len(raw) == 32 and all(isinstance(item, int) and 0 <= item <= 255 for item in raw):
        return bytes(raw).hex()
    if isinstance(raw, str) and len(raw) == 64:
        return raw.lower()
    return None


def exact_stat_identity(path: Path, identity: dict[str, Any], budget: HashBudget) -> tuple[bool, dict[str, Any]]:
    digest = expected_freshness_digest(identity)
    evidence = inspect_path(path, budget, expected_digest=digest)
    if not evidence["safe_path"] or evidence["kind"] != "regular_file" or digest is None:
        return False, evidence
    try:
        info = path.lstat()
    except OSError:
        return False, evidence
    expected_kind = str(identity.get("kind", "")).lower()
    checks = [
        expected_kind == "regular_file",
        identity.get("size_bytes") == info.st_size,
        identity.get("modified_unix") == int(info.st_mtime),
        identity.get("ino", info.st_ino) == info.st_ino,
        identity.get("dev", info.st_dev) == info.st_dev,
        evidence["identity_matches"] is True,
    ]
    evidence["identity_matches"] = all(checks)
    return all(checks), evidence


def inspect_rename(path: Path, value: dict[str, Any], budget: HashBudget) -> dict[str, Any]:
    entries = value.get("entries")
    if not isinstance(entries, list) or not isinstance(value.get("transaction_id"), str):
        raise InspectorError("rename journal is missing transaction_id or entries")
    operation = new_operation(classify_rename_subsystem(entries), value["transaction_id"], path)
    original = value.get("state", "planned")
    operation.update({
        "state": map_state(original, family="rename"), "original_state": original,
        "created_at": value.get("created_at_unix"), "updated_at": value.get("recovery_resolved_at_unix"),
        "entries_total": len(entries), "schema_version": "rename-transaction-v1-compatible",
        "original_format": "rename_transaction",
    })
    source_root = Path(value["source_scan_root"]) if isinstance(value.get("source_scan_root"), str) and value["source_scan_root"] else None
    settled = original in {"applied", "rolled_back"}
    active = original in {"planned", "applying", "apply_failed", "rolling_back", "rollback_failed"}
    exact_envelope = value.get("emuwiz_exact_resume_envelope")
    exact_state = value.get("emuwiz_exact_resume_state")
    if exact_envelope is not None:
        operation["evidence"].append({"kind": "exact_resume_envelope", "state": exact_state, "format_version": exact_envelope.get("format_version") if isinstance(exact_envelope, dict) else None})
        operation["resume_evidence"].append({
            "exact_resume_envelope_present": True,
            "state": exact_state,
            "proven": False,
            "detail": "the immutable envelope is present, but this standalone inspector was not given the current plan generation/digest and never invokes the locked core resume preflight",
        })
        if not isinstance(exact_envelope, dict) or exact_envelope.get("format_version") != 1:
            operation["blockers"].append("exact-resume envelope is malformed or has an unsupported version")
    safe_applied = 0
    completed = 0
    ambiguous = False
    unsafe = False
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            unsafe = True; operation["blockers"].append(f"entry {index} is not an object"); continue
        state = str(entry.get("state", "planned"))
        if state in {"applied", "rolled_back"}: completed += 1
        source = Path(entry["source_path"]) if isinstance(entry.get("source_path"), str) else None
        destination = Path(entry["destination_path"]) if isinstance(entry.get("destination_path"), str) else None
        add_unique(operation["source_paths"], source); add_unique(operation["destination_paths"], destination)
        operation_kind = entry.get("operation", {"kind": "rename_move"})
        if not isinstance(operation_kind, dict): operation_kind = {"kind": str(operation_kind)}
        kind = operation_kind.get("kind", "rename_move")
        checkpoint: dict[str, Any] = {
            "index": index,
            "state": state,
            "operation": kind,
            "captured_source_identity": entry.get("identity"),
        }
        # Settled transactions are history, not active recovery work.  Keep
        # their path projection but do not probe/hash thousands of historical
        # destinations merely because --include-history was requested.
        if not settled and state == "applied" and destination is not None:
            if kind == "create_symlink":
                root = Path(operation_kind["destination_root"]) if isinstance(operation_kind.get("destination_root"), str) else None
                expected_target = operation_kind.get("expected_target")
                evidence = inspect_path(destination, budget, expected_root=root)
                checkpoint["destination"] = evidence; operation["evidence"].append(evidence)
                valid = evidence["kind"] == "symlink" and evidence["symlink_target"] == expected_target and evidence["problem"] == "leaf path is a symlink"
                # A leaf symlink is expected for this operation; parent symlinks remain unsafe.
                if valid: safe_applied += 1
                else: unsafe = unsafe or evidence["problem"] not in {None, "leaf path is a symlink"}; ambiguous = True
            elif kind == "create_hardlink":
                root = Path(operation_kind["destination_root"]) if isinstance(operation_kind.get("destination_root"), str) else None
                evidence = inspect_path(destination, budget, expected_root=root, expected_digest=expected_freshness_digest(entry.get("identity")))
                checkpoint["destination"] = evidence; operation["evidence"].append(evidence)
                source_valid, source_evidence = exact_stat_identity(source, entry.get("identity", {}), budget)
                checkpoint["source"] = source_evidence; operation["evidence"].append(source_evidence)
                try:
                    source_info = source.lstat() if source is not None else None
                    destination_info = destination.lstat()
                    same_object = source_info is not None and source_info.st_ino == destination_info.st_ino and source_info.st_dev == destination_info.st_dev
                except OSError: same_object = False
                if source_valid and evidence["safe_path"] and evidence["identity_matches"] is True and same_object: safe_applied += 1
                else: ambiguous = True
            else:
                root = source_root if source_root and destination and path_is_within(destination, source_root) else None
                if root is None:
                    checkpoint["root_binding"] = "destination is outside source_scan_root; journal has no separate trusted destination root"
                    ambiguous = True
                valid, evidence = exact_stat_identity(destination, entry.get("identity", {}), budget)
                checkpoint["destination"] = evidence; operation["evidence"].append(evidence)
                source_evidence = inspect_path(source, budget, expected_root=source_root)
                checkpoint["source"] = source_evidence; operation["evidence"].append(source_evidence)
                if valid and not source_evidence["exists"] and root is not None: safe_applied += 1
                else: ambiguous = True
        elif state in {"applying", "rolling_back", "rollback_failed"}:
            ambiguous = True
        operation["entry_checkpoints"].append(checkpoint)
    operation["entries_completed"] = completed
    operation["entries_pending"] = len(entries) - completed
    for directory in value.get("created_directories", []):
        if isinstance(directory, str): add_unique(operation["created_directories"], directory)
    if settled:
        operation["category"] = "COMPLETED_HISTORY"
        operation["suggested_status"] = "NO_ACTION"
    elif unsafe:
        operation.update(state="UnsafeToResume", category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH")
    elif active and safe_applied > 0 and safe_applied == sum(1 for item in entries if isinstance(item, dict) and item.get("state") == "applied") and not ambiguous:
        operation.update(category="RECOVERABLE", rollback_possible=True, suggested_status="SAFE_ROLLBACK_CANDIDATE")
    elif active:
        operation.update(category="REVIEW_REQUIRED", suggested_status="REVIEW_REQUIRED")
        if not ambiguous:
            operation["blockers"].append("no exact rollback or independently provable resume action is available")
    operation["resume_possible"] = False
    if operation["category"] == "REVIEW_REQUIRED":
        operation["needs_review_reason"] = "; ".join(operation["blockers"]) or "filesystem identity or trusted-root ownership is not fully proven"
    return operation


def shared_subsystem(value: dict[str, Any]) -> str:
    context = value.get("context", {})
    adapter = context.get("adapter") if isinstance(context, dict) else None
    if adapter in {"local_mod_package", "cemu_graphic_pack", "rpcs3_ordinary_mod", "pcsx2", "ppsspp", "xenia"}:
        return f"Shared Mod Transaction ({adapter})"
    return f"Shared Transaction ({adapter or 'unknown adapter'})"


def shared_destination(entry: dict[str, Any]) -> tuple[Path | None, Path | None, str | None]:
    plan = entry.get("plan_entry", {})
    if not isinstance(plan, dict): return None, None, "entry has no plan_entry"
    root, root_error = decode_shared_path(plan.get("destination_root"))
    relative, relative_error = decode_shared_path(plan.get("destination_relative_path"))
    if root_error or relative_error or root is None or relative is None:
        return None, root, root_error or relative_error
    if relative.is_absolute() or has_parent_component(relative):
        return None, root, "destination relative path is absolute or contains traversal"
    destination = root / relative
    if not path_is_within(destination, root):
        return None, root, "destination escapes recorded root"
    return destination, root, None


def inspect_shared_durable(path: Path, value: dict[str, Any], budget: HashBudget) -> dict[str, Any]:
    operation_id = value.get("operation_id")
    entries = value.get("entries")
    if not isinstance(operation_id, str) or not isinstance(entries, list):
        raise InspectorError("durable shared journal is missing operation_id or entries")
    operation = new_operation(shared_subsystem(value), operation_id, path)
    schema = value.get("schema_version")
    original = value.get("transaction_state")
    operation.update({"schema_version": schema, "original_format": "shared_durable_pending", "original_state": original, "state": map_state(original, family="shared"), "created_at": value.get("timestamp_unix_seconds"), "entries_total": len(entries)})
    if schema != 1:
        operation.update(state="UnknownSchema", category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH", needs_review_reason="unknown durable shared journal schema")
        operation["blockers"].append("unknown newer schema")
        return operation
    terminal = original in {"applied", "rolled_back", "legacy_complete"}
    unsafe = False; ambiguous = False; rollback_proven = True; completed = 0
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict): unsafe = True; rollback_proven = False; continue
        state = str(entry.get("state", "planned")); plan = entry.get("plan_entry", {})
        checkpoint = {"index": index, "state": state, "backup_prepared": state in {"backup_created", "destination_temp_written", "destination_replaced", "destination_verified", "applied", "rolling_back", "rolled_back"}, "temp_written": state in {"destination_temp_written", "destination_replaced", "destination_verified", "applied"}, "replacement_published": state in {"destination_replaced", "destination_verified", "applied", "rolling_back", "rolled_back"}, "verification_complete": state in {"destination_verified", "applied", "rolled_back"}, "rollback_checkpoint": state if state in {"rolling_back", "rolled_back"} else None}
        destination, destination_root, error = shared_destination(entry)
        if error: unsafe = True; rollback_proven = False; operation["blockers"].append(f"entry {index}: {error}")
        add_unique(operation["destination_paths"], destination)
        expected_new = plan.get("source_digest") if isinstance(plan, dict) and isinstance(plan.get("source_digest"), str) else None
        expected_old = plan.get("destination_pre_digest") if isinstance(plan, dict) and isinstance(plan.get("destination_pre_digest"), str) else None
        source, source_error = decode_shared_path(plan.get("source_path")) if isinstance(plan, dict) else (None, "missing plan")
        approved_root, approved_error = decode_shared_path(value.get("approved_source_root"))
        add_unique(operation["source_paths"], source)
        if source_error or approved_error or source is None or approved_root is None or not path_is_within(source, approved_root):
            unsafe = True; operation["blockers"].append(f"entry {index}: source path is not safely bound to approved_source_root")
        destination_evidence = inspect_path(destination, budget, expected_root=destination_root, expected_digest=expected_new)
        checkpoint["destination"] = destination_evidence; operation["evidence"].append(destination_evidence)
        if destination_evidence["problem"] and "symlink" in destination_evidence["problem"]:
            unsafe = True
            rollback_proven = False
            operation["blockers"].append(f"entry {index}: destination path traverses or is a symlink")
        backup, backup_error = decode_shared_path(entry.get("backup_path")) if entry.get("backup_path") is not None else (None, None)
        backup_root, backup_root_error = decode_shared_path(value.get("backup_root"))
        add_unique(operation["backup_paths"], backup)
        backup_required = bool(plan.get("backup_required")) if isinstance(plan, dict) else False
        backup_expected = entry.get("backup_digest") if isinstance(entry.get("backup_digest"), str) else expected_old
        backup_evidence = inspect_path(backup, budget, expected_root=backup_root, expected_digest=backup_expected) if backup is not None else None
        if backup_evidence: checkpoint["backup"] = backup_evidence; operation["evidence"].append(backup_evidence)
        if backup_evidence and backup_evidence["problem"] and "symlink" in backup_evidence["problem"]:
            unsafe = True
            rollback_proven = False
            operation["blockers"].append(f"entry {index}: backup path traverses or is a symlink")
        if backup_error or backup_root_error: unsafe = True; rollback_proven = False
        temporary, temporary_error = decode_shared_path(entry.get("temporary_path")) if entry.get("temporary_path") is not None else (None, None)
        add_unique(operation["temporary_paths"], temporary)
        if temporary is not None:
            temporary_evidence = inspect_path(temporary, budget, expected_root=destination_root)
            checkpoint["temporary"] = temporary_evidence
            operation["evidence"].append(temporary_evidence)
            if temporary_evidence["problem"] and "symlink" in temporary_evidence["problem"]:
                unsafe = True
                operation["blockers"].append(f"entry {index}: temporary path traverses or is a symlink")
        if temporary_error:
            unsafe = True
            operation["blockers"].append(f"entry {index}: temporary path cannot be decoded")
        preexisted = entry.get("destination_existed_before_apply")
        new_matches = destination_evidence["identity_matches"] is True
        old_matches = bool(
            preexisted is True
            and expected_old
            and destination_evidence["observed_sha256"]
            and destination_evidence["observed_sha256"].lower() == expected_old.lower()
        )
        absent_matches = preexisted is False and not destination_evidence["exists"] and destination_evidence["safe_path"]
        pre_state_matches = old_matches or absent_matches
        backup_matches = bool(backup_evidence and backup_evidence["identity_matches"] is True)
        backup_needed = preexisted is True or backup_required
        mutation_states = {"destination_replaced", "destination_verified", "applied"}
        pre_mutation_states = {"planned", "applying", "backup_created", "destination_temp_written"}
        if state in mutation_states:
            if new_matches:
                completed += 1
                checkpoint["reconciled_state"] = "applied"
            else:
                ambiguous = True; rollback_proven = False
            if backup_needed and not backup_matches:
                ambiguous = True; rollback_proven = False; operation["blockers"].append(f"entry {index}: required backup is missing or does not match")
        elif state in pre_mutation_states:
            if new_matches:
                # The replacement crossed the crash boundary before its next
                # checkpoint became durable.  Exact bytes reconcile it as
                # applied; this still never authorizes automatic resume.
                completed += 1
                checkpoint["reconciled_state"] = "applied_after_checkpoint_gap"
                if backup_needed and not backup_matches:
                    ambiguous = True; rollback_proven = False; operation["blockers"].append(f"entry {index}: replacement is present but its required backup is unproven")
            elif pre_state_matches:
                checkpoint["reconciled_state"] = "not_applied"
                if state in {"backup_created", "destination_temp_written"} and backup_needed and not backup_matches:
                    ambiguous = True; rollback_proven = False; operation["blockers"].append(f"entry {index}: checkpointed backup is missing or changed")
            else:
                ambiguous = True; rollback_proven = False
                operation["blockers"].append(f"entry {index}: destination matches neither recorded pre-state nor expected output")
        elif state == "rolling_back":
            if pre_state_matches:
                completed += 1
                checkpoint["reconciled_state"] = "rolled_back_after_checkpoint_gap"
            elif new_matches:
                checkpoint["reconciled_state"] = "still_applied"
                completed += 1
                if backup_needed and not backup_matches:
                    ambiguous = True; rollback_proven = False; operation["blockers"].append(f"entry {index}: rollback still needed but backup is unproven")
            else:
                ambiguous = True; rollback_proven = False
        elif state == "rolled_back":
            if preexisted is False:
                if destination_evidence["exists"]: ambiguous = True
                else:
                    completed += 1
                    checkpoint["reconciled_state"] = "rolled_back"
            elif expected_old:
                old_evidence = inspect_path(destination, budget, expected_root=destination_root, expected_digest=expected_old)
                checkpoint["rolled_back_destination"] = old_evidence
                if old_evidence["identity_matches"] is not True: ambiguous = True
                else:
                    completed += 1
                    checkpoint["reconciled_state"] = "rolled_back"
        elif state in {"needs_review", "unsafe_to_resume"}:
            ambiguous = True
        operation["entry_checkpoints"].append(checkpoint)
    operation["entries_completed"] = completed; operation["entries_pending"] = len(entries) - completed
    for item in value.get("created_root_directories", []):
        if isinstance(item, dict):
            directory, _ = decode_shared_path(item.get("path")); add_unique(operation["created_directories"], directory)
    if terminal:
        operation.update(category="COMPLETED_HISTORY", suggested_status="NO_ACTION")
    elif unsafe:
        operation.update(state="UnsafeToResume", category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH")
    elif rollback_proven and completed > 0 and not ambiguous and original in {"applying", "apply_failed", "needs_review"}:
        operation.update(category="RECOVERABLE", rollback_possible=True, suggested_status="SAFE_ROLLBACK_CANDIDATE")
    elif ambiguous or original in {"rollback_failed", "unsafe_to_resume", "needs_review"}:
        operation.update(category="REVIEW_REQUIRED", suggested_status="REVIEW_REQUIRED")
    else:
        operation.update(category="REVIEW_REQUIRED", suggested_status="REVIEW_REQUIRED")
        operation["blockers"].append("no exact rollback or independently provable resume action is available")
    operation["resume_possible"] = False
    if operation["category"] == "REVIEW_REQUIRED":
        operation["needs_review_reason"] = "; ".join(operation["blockers"]) or "destination or backup identity is ambiguous"
    return operation


def inspect_shared_legacy(path: Path, value: dict[str, Any], budget: HashBudget) -> dict[str, Any]:
    operation_id = value.get("operation_id")
    entries = value.get("entries")
    if not isinstance(operation_id, str) or not isinstance(entries, list):
        raise InspectorError("legacy shared journal is missing operation_id or entries")
    operation = new_operation(shared_subsystem(value), operation_id, path)
    schema = value.get("schema_version"); original = value.get("status")
    operation.update({"schema_version": schema, "original_format": "shared_legacy_complete", "original_state": original, "state": map_state(original, family="shared"), "created_at": value.get("timestamp_unix_seconds"), "entries_total": len(entries), "entries_completed": len(entries), "entries_pending": 0, "category": "COMPLETED_HISTORY", "suggested_status": "NO_ACTION"})
    if schema != 1:
        operation.update(state="UnknownSchema", category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH", needs_review_reason="unsupported shared journal schema")
        return operation
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            operation["warnings"] = [f"legacy entry {index} is malformed"]
            continue
        destination, root, error = shared_destination(entry); add_unique(operation["destination_paths"], destination)
        backup, _ = decode_shared_path(entry.get("backup_path")) if entry.get("backup_path") else (None, None); add_unique(operation["backup_paths"], backup)
        if error:
            operation["warnings"] = [f"legacy entry {index} has an invalid destination path"]
    operation["evidence"].append({
        "kind": "legacy_final_only",
        "detail": "completed history was projected without probing historical destinations",
    })
    operation["rollback_possible"] = False
    return operation


def inspect_shared_rollback_receipt(path: Path, value: dict[str, Any]) -> dict[str, Any]:
    """Inspect the completed marker written only after a shared rollback."""
    operation_id = value.get("original_operation_id")
    entries = value.get("entries")
    if not isinstance(operation_id, str) or not operation_id or not isinstance(entries, list):
        raise InspectorError("shared rollback receipt is missing original_operation_id or entries")
    operation = new_operation("Shared Transaction Rollback", operation_id, path)
    schema = value.get("schema_version")
    operation.update({
        "schema_version": schema,
        "original_format": "shared_rollback_receipt",
        "original_state": "rolled_back",
        "state": "RolledBack",
        "entries_total": len(entries),
        "entries_completed": len(entries),
        "entries_pending": 0,
        "category": "COMPLETED_HISTORY",
        "suggested_status": "NO_ACTION",
    })
    if schema != 1:
        operation.update(
            state="UnknownSchema",
            category="REVIEW_REQUIRED",
            suggested_status="DO_NOT_TOUCH",
            needs_review_reason="unknown shared rollback receipt schema",
        )
        return operation
    successful = {"removed_installed_file", "restored_backup", "no_change_required"}
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            operation.update(
                state="Unreadable",
                category="REVIEW_REQUIRED",
                suggested_status="DO_NOT_TOUCH",
                needs_review_reason=f"shared rollback receipt entry {index} is malformed",
            )
            continue
        destination, destination_error = decode_shared_path(entry.get("destination")) if entry.get("destination") else (None, None)
        backup, backup_error = decode_shared_path(entry.get("backup")) if entry.get("backup") else (None, None)
        add_unique(operation["destination_paths"], destination)
        add_unique(operation["backup_paths"], backup)
        if destination_error or backup_error or entry.get("outcome") not in successful:
            operation.update(
                state="NeedsReview",
                category="REVIEW_REQUIRED",
                suggested_status="DO_NOT_TOUCH",
                needs_review_reason="rollback marker does not contain a fully successful, losslessly decoded result",
            )
    return operation


def inspect_cheat(path: Path, value: dict[str, Any], budget: HashBudget, *, rollback: bool) -> dict[str, Any]:
    family = "cheat_rollback" if rollback else "cheat_install"
    operation_id = value.get("run_id")
    entries = value.get("entries")
    if not isinstance(operation_id, str) or not isinstance(entries, list): raise InspectorError("cheat journal is missing run_id or entries")
    operation = new_operation("Cheat Rollback" if rollback else "Cheat Install", operation_id, path)
    schema = value.get("schema_version"); original = value.get("status"); completed_at = value.get("completed_at_unix_seconds")
    operation.update({"schema_version": schema, "original_format": family, "original_state": original, "state": map_state(original, family=family), "created_at": value.get("started_at_unix_seconds"), "updated_at": completed_at, "entries_total": len(entries)})
    if schema != 1:
        operation.update(state="UnknownSchema", category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH", needs_review_reason="unknown cheat journal schema")
        return operation
    operation["entries_completed"] = sum(1 for entry in entries if isinstance(entry, dict) and (entry.get("applied") or entry.get("wrote") or str(entry.get("outcome", "")).startswith(("already_", "no_change"))))
    operation["entries_pending"] = max(0, len(entries) - operation["entries_completed"])
    if completed_at is None:
        operation.update(state="RollingBack" if rollback else "Applying", category="REVIEW_REQUIRED", suggested_status="REVIEW_REQUIRED", needs_review_reason="current cheat schemas are final-result journals; missing completion evidence cannot prove a recovery action")
    else:
        operation.update(category="COMPLETED_HISTORY", suggested_status="NO_ACTION")
    root_value = value.get("destination_root")
    root, root_error = decode_cheat_path(root_value) if root_value is not None else (None, "destination root absent")
    safe_rollback = completed_at is not None and root_error is None
    for entry in entries:
        if not isinstance(entry, dict): safe_rollback = False; continue
        destination, error = decode_cheat_path(entry.get("destination_path")) if entry.get("destination_path") else (None, None)
        add_unique(operation["destination_paths"], destination)
        expected = entry.get("expected_installed_hash") if rollback else entry.get("resulting_destination_hash") or entry.get("expected_source_hash")
        evidence = inspect_path(destination, budget, expected_root=root, expected_digest=expected if isinstance(expected, str) else None) if destination else None
        if evidence: operation["evidence"].append(evidence)
        backup, backup_error = decode_cheat_path(entry.get("backup_path")) if entry.get("backup_path") else (None, None)
        add_unique(operation["backup_paths"], backup)
        previous = entry.get("expected_previous_hash") if rollback else entry.get("previous_destination_hash")
        backup_evidence = inspect_path(backup, budget, expected_digest=previous if isinstance(previous, str) else None) if backup else None
        if backup_evidence: operation["evidence"].append(backup_evidence)
        if error or backup_error: safe_rollback = False
        if not rollback and entry.get("applied"):
            if evidence is None or evidence["identity_matches"] is not True: safe_rollback = False
            if entry.get("previous_destination_state") == "present_different" and (backup_evidence is None or backup_evidence["identity_matches"] is not True): safe_rollback = False
    operation["rollback_possible"] = safe_rollback and not rollback and any(entry.get("applied") for entry in entries if isinstance(entry, dict))
    return operation


PATCH_STATES = {
    "planned": "Applying",
    "preparing": "Applying",
    "prepared": "Applying",
    "verifying_temporary": "Applying",
    "publishing": "Applying",
    "published": "Applying",
    "verifying_published": "Applying",
    "completed": "Completed",
    "failed": "ApplyFailed",
    "rolling_back": "RollingBack",
    "rolled_back": "RolledBack",
    "rollback_failed": "RollbackFailed",
    "needs_review": "NeedsReview",
    "unsafe_to_resume": "UnsafeToResume",
}


def patch_checkpoint_details(checkpoints: Any) -> dict[str, Any]:
    values = [str(item) for item in checkpoints] if isinstance(checkpoints, list) else []
    return {
        "checkpoints": values,
        "publication_checkpoint": next((item for item in reversed(values) if item in {"before_publish", "published", "before_published_verification", "published_verified", "before_completed"}), None),
        "verification_checkpoint": next((item for item in reversed(values) if item in {"temporary_verified", "published_verified", "verifying_temporary", "before_published_verification"}), None),
    }


def inspect_patch_output(path: Path, value: dict[str, Any], budget: HashBudget) -> dict[str, Any]:
    """Read-only projection of patch_output_recovery.rs schema 1.

    The branch that owns patch execution remains authoritative for recovery
    eligibility. This projection deliberately only returns the same safe cases
    that its inspection routine proves from recorded hashes and filesystem
    state; it never calls a recovery API.
    """
    schema = value.get("schema_version")
    operation_id = value.get("operation_id")
    if schema != 1:
        operation = new_operation("Standalone Patch Output", str(operation_id or path.stem), path)
        operation.update(schema_version=schema, original_format="patch_output_journal", original_state=value.get("state"), state="UnknownSchema", category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH", needs_review_reason="unknown patch-output journal schema")
        operation["blockers"].append("patch-output schema is not version 1")
        return operation
    required = ("operation_id", "patch_format", "source_path", "source_size", "source_sha256", "patch_path", "patch_size", "patch_sha256", "destination_path", "temporary_output_path", "provenance_path", "state", "checkpoints")
    missing = [key for key in required if key not in value]
    if missing or not isinstance(operation_id, str) or not isinstance(value.get("state"), str):
        raise InspectorError("patch-output journal is missing required fields: " + ", ".join(missing or ["operation_id/state"]))
    operation = new_operation("Standalone Patch Output", operation_id, path)
    original = value["state"]
    normalized = PATCH_STATES.get(original)
    if normalized is None:
        operation.update(schema_version=1, original_format="patch_output_journal", original_state=original, state="UnknownSchema", category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH", needs_review_reason="unknown patch-output state")
        operation["blockers"].append(f"unsupported patch-output state: {original}")
        return operation
    operation.update({
        "schema_version": 1, "original_format": "patch_output_journal", "original_state": original,
        "state": normalized, "created_at": value.get("created_at_unix"), "updated_at": value.get("updated_at_unix"),
        "entries_total": 1, "entries_completed": 1 if normalized in {"Completed", "RolledBack"} else 0,
        "entries_pending": 0 if normalized in {"Completed", "RolledBack"} else 1,
        "patch_format": value.get("patch_format"),
        "patch_identity": {"path": value.get("patch_path"), "size": value.get("patch_size"), "sha256": value.get("patch_sha256")},
        "publication_checkpoint": patch_checkpoint_details(value.get("checkpoints"))["publication_checkpoint"],
        "verification_checkpoint": patch_checkpoint_details(value.get("checkpoints"))["verification_checkpoint"],
    })
    source = Path(value["source_path"]) if isinstance(value.get("source_path"), str) else None
    patch = Path(value["patch_path"]) if isinstance(value.get("patch_path"), str) else None
    destination = Path(value["destination_path"]) if isinstance(value.get("destination_path"), str) else None
    temporary = Path(value["temporary_output_path"]) if isinstance(value.get("temporary_output_path"), str) else None
    provenance = Path(value["provenance_path"]) if isinstance(value.get("provenance_path"), str) else None
    for target, collection in ((source, "source_paths"), (patch, "source_paths"), (destination, "destination_paths"), (temporary, "temporary_paths")):
        add_unique(operation[collection], target)
    if value.get("backup_path") is not None:
        add_unique(operation["backup_paths"], Path(value["backup_path"]) if isinstance(value["backup_path"], str) else None)
    source_evidence = inspect_path(source, budget, expected_digest=value.get("source_sha256")) if source else None
    patch_evidence = inspect_path(patch, budget, expected_digest=value.get("patch_sha256")) if patch else None
    temporary_evidence = inspect_path(temporary, budget, expected_root=destination.parent if destination else None, expected_digest=value.get("expected_output_sha256")) if temporary else None
    destination_evidence = inspect_path(destination, budget, expected_root=destination.parent if destination else None, expected_digest=value.get("expected_output_sha256")) if destination else None
    provenance_evidence = inspect_path(provenance, budget, expected_root=destination.parent if destination else None, expected_digest=value.get("provenance_sha256")) if provenance else None
    evidence = [item for item in (source_evidence, patch_evidence, temporary_evidence, destination_evidence, provenance_evidence) if item is not None]
    operation["evidence"].extend(evidence)
    operation["filesystem_evidence"] = {
        "source": source_evidence, "patch": patch_evidence, "temporary_output": temporary_evidence,
        "destination": destination_evidence, "provenance": provenance_evidence,
    }
    source_matches = bool(source_evidence and source_evidence["identity_matches"] is True)
    patch_matches = bool(patch_evidence and patch_evidence["identity_matches"] is True)
    temporary_matches = bool(temporary_evidence and temporary_evidence["identity_matches"] is True)
    destination_matches = bool(destination_evidence and destination_evidence["identity_matches"] is True)
    provenance_present = bool(provenance_evidence and provenance_evidence["exists"] and provenance_evidence["safe_path"])
    publication_states = {"publishing", "published", "verifying_published"}
    temporary_states = {"prepared", "verifying_temporary", "publishing"}
    if normalized in {"Completed", "RolledBack"}:
        operation.update(category="COMPLETED_HISTORY", suggested_status="NO_ACTION")
    elif not source_matches or not patch_matches:
        operation.update(state="UnsafeToResume", category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH", needs_review_reason="source or patch changed since the durable plan")
        operation["blockers"].append("source or patch digest does not match the journal")
    elif original in publication_states and destination_matches and provenance_present:
        operation.update(category="RECOVERABLE", resume_possible=True, rollback_possible=not bool(value.get("destination_preexisting")), suggested_status="SAFE_ROLLBACK_CANDIDATE" if not value.get("destination_preexisting") else "SAFE_RESUME_CANDIDATE")
        operation["resume_evidence"].append("destination matches the verified output and provenance is present; core recovery may finalize the journal")
        if operation["rollback_possible"]:
            operation["rollback_evidence"].append("destination matches the verified output and was not pre-existing; core rollback may remove only owned output")
    elif original in temporary_states and temporary_matches and destination_evidence and not destination_evidence["exists"]:
        operation.update(category="RECOVERABLE", resume_possible=True, suggested_status="SAFE_RESUME_CANDIDATE")
        operation["resume_evidence"].append("verified temporary output is present and destination is absent")
    elif normalized == "ApplyFailed":
        operation.update(category="REVIEW_REQUIRED", suggested_status="REVIEW_REQUIRED", needs_review_reason=value.get("failure_reason") or "patch output failed before completion")
    else:
        operation.update(category="REVIEW_REQUIRED", suggested_status="REVIEW_REQUIRED", needs_review_reason="filesystem state does not match a safe patch-output recovery case")
    return operation


def inspect_database_restore(
    path: Path,
    value: dict[str, Any],
    budget: HashBudget,
    data_root: Path,
) -> dict[str, Any]:
    operation_id = value.get("operation_id")
    plan = value.get("plan")
    if not isinstance(operation_id, str) or not operation_id or not isinstance(plan, dict):
        raise InspectorError("database restore receipt is missing operation_id or plan")
    operation = new_operation("Database Restore", operation_id, path)
    schema = value.get("schema_version")
    original = value.get("state")
    operation.update({
        "schema_version": schema,
        "original_format": "database_restore_receipt",
        "original_state": original,
        "created_at": value.get("created_at_unix"),
        "updated_at": value.get("updated_at_unix"),
        "entries_total": 1,
    })
    if schema != 1:
        operation.update(
            state="UnknownSchema",
            category="REVIEW_REQUIRED",
            suggested_status="DO_NOT_TOUCH",
            needs_review_reason="unknown database restore receipt schema",
        )
        return operation
    state_map = {
        "planned": "Applying",
        "validating": "Applying",
        "emergency_backup_created": "Applying",
        "restore_staged": "Applying",
        "restore_applied": "Applying",
        "verification": "Applying",
        "completed": "Completed",
        "failed": "ApplyFailed",
        "rollback_attempted": "RollingBack",
        "rolled_back": "RolledBack",
        "rollback_failed": "RollbackFailed",
    }
    operation["state"] = state_map.get(str(original), "NeedsReview")
    live = Path(plan["live_database_path"]) if isinstance(plan.get("live_database_path"), str) else None
    selected = Path(plan["selected_backup_path"]) if isinstance(plan.get("selected_backup_path"), str) else None
    emergency = Path(value["emergency_backup_path"]) if isinstance(value.get("emergency_backup_path"), str) else None
    add_unique(operation["source_paths"], selected)
    add_unique(operation["destination_paths"], live)
    add_unique(operation["backup_paths"], emergency)
    expected_name = f"{live.name}.restore-{operation_id}.json" if live is not None else None
    structural_safe = bool(
        live is not None
        and live.is_absolute()
        and path_is_within(live, data_root)
        and live.parent == data_root
        and path.parent == data_root
        and path.name == expected_name
        and "/" not in operation_id
    )
    if not structural_safe:
        operation.update(
            state="UnsafeToResume",
            category="REVIEW_REQUIRED",
            suggested_status="DO_NOT_TOUCH",
            needs_review_reason="receipt filename or live database path is not exactly bound to the inspected data root",
        )
        return operation
    settled = original in {"completed", "rolled_back"}
    operation["entries_completed"] = 1 if original in {"restore_applied", "verification", "completed", "rollback_attempted", "rolled_back", "rollback_failed"} else 0
    operation["entries_pending"] = 0 if settled else 1
    operation["entry_checkpoints"].append({
        "state": original,
        "emergency_backup_captured": original in {"emergency_backup_created", "restore_staged", "restore_applied", "verification", "completed", "rollback_attempted", "rolled_back", "rollback_failed", "failed"},
        "replacement_staged": original in {"restore_staged", "restore_applied", "verification", "completed", "rollback_attempted", "rolled_back", "rollback_failed"},
        "replacement_published": original in {"restore_applied", "verification", "completed", "rollback_attempted", "rolled_back", "rollback_failed"},
        "rollback_started": original in {"rollback_attempted", "rolled_back", "rollback_failed"},
        "rollback_completed": original == "rolled_back",
    })
    if settled:
        operation.update(category="COMPLETED_HISTORY", suggested_status="NO_ACTION")
        return operation

    selected_digest = plan.get("selected_backup_sha256") if isinstance(plan.get("selected_backup_sha256"), str) else None
    live_pre_digest = plan.get("expected_live_sha256") if isinstance(plan.get("expected_live_sha256"), str) else None
    applied_digest = value.get("applied_database_sha256") if isinstance(value.get("applied_database_sha256"), str) else None
    emergency_digest = value.get("emergency_backup_sha256") if isinstance(value.get("emergency_backup_sha256"), str) else None
    selected_evidence = inspect_path(selected, budget, expected_digest=selected_digest)
    live_expected = applied_digest if original in {"restore_applied", "verification", "rollback_attempted", "rollback_failed"} else live_pre_digest
    live_evidence = inspect_path(live, budget, expected_root=data_root, expected_digest=live_expected)
    emergency_evidence = inspect_path(emergency, budget, expected_root=data_root, expected_digest=emergency_digest) if emergency is not None else None
    operation["evidence"].extend([selected_evidence, live_evidence])
    if emergency_evidence:
        operation["evidence"].append(emergency_evidence)
    sidecars = [
        live.with_name(f"{live.name}-wal"),
        live.with_name(f"{live.name}-shm"),
    ]
    present_sidecars = [os.fspath(item) for item in sidecars if item.exists()]
    if present_sidecars:
        operation["blockers"].append("live database has WAL/SHM sidecars")
    selected_valid = selected_evidence["identity_matches"] is True
    live_valid = live_evidence["identity_matches"] is True
    emergency_required = original not in {"planned", "validating"}
    emergency_valid = not emergency_required or bool(emergency_evidence and emergency_evidence["identity_matches"] is True)
    pre_apply_states = {"planned", "validating", "emergency_backup_created", "restore_staged", "failed"}
    rollback_states = {"restore_applied", "verification", "rollback_attempted"}
    if original in pre_apply_states and selected_valid and live_valid and emergency_valid and not present_sidecars:
        operation.update(category="RECOVERABLE", resume_possible=True, suggested_status="SAFE_RESUME_CANDIDATE")
        operation["resume_evidence"].append("selected backup and unchanged live database exactly match the approved restore plan")
    elif original in rollback_states and live_valid and emergency_valid and not present_sidecars:
        operation.update(category="RECOVERABLE", rollback_possible=True, suggested_status="SAFE_ROLLBACK_CANDIDATE")
        operation["rollback_evidence"].append("live database matches the applied restore and the emergency backup matches its recorded SHA-256")
    else:
        operation.update(category="REVIEW_REQUIRED", suggested_status="REVIEW_REQUIRED")
        if not selected_valid and original in pre_apply_states:
            operation["blockers"].append("selected restore backup is missing, too large to hash, or changed")
        if not live_valid:
            operation["blockers"].append("live database does not match the state recorded by the receipt")
        if not emergency_valid:
            operation["blockers"].append("emergency backup is missing, too large to hash, or changed")
        if original == "rollback_failed":
            operation["blockers"].append("the receipt records a failed rollback whose cause is not safely reproducible by this inspector")
    if operation["category"] == "REVIEW_REQUIRED":
        operation["needs_review_reason"] = "; ".join(operation["blockers"]) or "database restore state is not safely classifiable"
    return operation


def inspect_es_de_recovery(
    path: Path,
    value: dict[str, Any],
    budget: HashBudget,
    gamelists_root: Path,
) -> dict[str, Any]:
    operation_id = "es-de-" + hashlib.sha256(os.fsencode(path)).hexdigest()[:16]
    operation = new_operation("ES-DE Publication Recovery", operation_id, path)
    operation.update({
        "schema_version": value.get("schema_version"),
        "original_format": "es_de_gamelist_recovery",
        "original_state": "recovery_record_present",
        "state": "Applying",
        "entries_total": 1,
        "entries_pending": 1,
    })
    if value.get("schema_version") != 1:
        operation.update(
            state="UnknownSchema",
            category="REVIEW_REQUIRED",
            suggested_status="DO_NOT_TOUCH",
            needs_review_reason="unknown ES-DE recovery schema",
        )
        return operation
    recorded = value.get("gamelist_path")
    previous = value.get("previous_content")
    if not isinstance(recorded, str) or (previous is not None and not isinstance(previous, str)):
        raise InspectorError("ES-DE recovery record has malformed gamelist_path or previous_content")
    gamelist = Path(recorded)
    expected_gamelist = path.with_name(path.name.removesuffix(ES_DE_RECOVERY_SUFFIX))
    add_unique(operation["destination_paths"], gamelist)
    add_unique(operation["backup_paths"], path)
    structural_safe = bool(
        gamelist.is_absolute()
        and gamelist == expected_gamelist
        and path_is_within(gamelist, gamelists_root)
        and path_is_within(path, gamelists_root)
        and not has_parent_component(gamelist)
    )
    if not structural_safe:
        operation.update(
            state="UnsafeToResume",
            category="REVIEW_REQUIRED",
            suggested_status="DO_NOT_TOUCH",
            needs_review_reason="recovery filename, recorded gamelist path, and inspected ES-DE root do not match exactly",
        )
        return operation
    previous_digest = hashlib.sha256(previous.encode("utf-8")).hexdigest() if previous is not None else None
    current = inspect_path(gamelist, budget, expected_root=gamelists_root, expected_digest=previous_digest)
    operation["evidence"].append(current)
    operation["rollback_evidence"].append({
        "previous_file_existed": previous is not None,
        "previous_content_sha256": previous_digest,
        "previous_content_bytes": len(previous.encode("utf-8")) if previous is not None else 0,
        "current_gamelist": current,
    })
    if current["problem"] and "symlink" in current["problem"]:
        operation.update(
            state="UnsafeToResume",
            category="REVIEW_REQUIRED",
            suggested_status="DO_NOT_TOUCH",
            needs_review_reason="ES-DE gamelist path traverses or is a symlink",
        )
    elif (previous is None and not current["exists"] and current["safe_path"]) or (previous is not None and current["identity_matches"] is True):
        operation.update(category="RECOVERABLE", rollback_possible=True, suggested_status="SAFE_ROLLBACK_CANDIDATE")
        operation["rollback_evidence"].append("current state already matches the exact recorded pre-publication state; recovery would only confirm it and remove the marker")
    else:
        operation.update(
            category="REVIEW_REQUIRED",
            suggested_status="REVIEW_REQUIRED",
            needs_review_reason="the record contains exact prior bytes but no expected published-output hash, so a divergent gamelist could be a later user change",
        )
        operation["blockers"].append("current gamelist cannot be proven to be the interrupted publication output rather than a later mutation")
    return operation


def unreadable_operation(subsystem: str, path: Path, detail: str) -> dict[str, Any]:
    operation = new_operation(subsystem, path.stem, path)
    operation.update(state="Unreadable", original_state=None, category="REVIEW_REQUIRED", suggested_status="DO_NOT_TOUCH", needs_review_reason=detail)
    operation["blockers"].append(detail)
    return operation


def direct_files(root: Path, *, suffix: str | None = None) -> tuple[list[Path], str | None]:
    try:
        info = root.lstat()
    except FileNotFoundError:
        return [], None
    except OSError as error:
        return [], f"cannot inspect known journal root {root}: {error}"
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        return [], f"known journal root is not a non-symlink directory: {root}"
    try:
        paths = []
        for index, item in enumerate(root.iterdir()):
            if index >= MAX_DIRECTORY_ENTRIES:
                return [], f"known journal root exceeds the {MAX_DIRECTORY_ENTRIES}-entry inspection bound: {root}"
            if suffix is None or item.name.endswith(suffix):
                paths.append(item)
        paths.sort(key=lambda item: os.fsencode(item.name))
    except OSError as error:
        return [], f"cannot list known journal root {root}: {error}"
    return paths, None


def patch_output_files(root: Path) -> tuple[list[Path], str | None]:
    """Find sibling patch journals without following symlinks or unbounded trees."""
    if not root.exists():
        return [], None
    found: list[Path] = []
    try:
        pending: list[tuple[Path, int]] = [(root, 0)]
        while pending:
            directory, depth = pending.pop()
            info = directory.lstat()
            if not stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode):
                continue
            children = sorted(directory.iterdir(), key=lambda item: os.fsencode(item.name))
            if len(children) > MAX_DIRECTORY_ENTRIES:
                return [], f"patch-output root exceeds the {MAX_DIRECTORY_ENTRIES}-entry inspection bound: {directory}"
            for child in children:
                child_info = child.lstat()
                if stat.S_ISREG(child_info.st_mode) and child.name.startswith(PATCH_OUTPUT_PREFIX) and child.name.endswith(PATCH_OUTPUT_SUFFIX):
                    found.append(child)
                elif depth < 4 and stat.S_ISDIR(child_info.st_mode) and not stat.S_ISLNK(child_info.st_mode):
                    pending.append((child, depth + 1))
    except OSError as error:
        return [], f"cannot enumerate patch-output journals under {root}: {error}"
    found.sort(key=lambda item: os.fsencode(os.fspath(item)))
    if len(found) > MAX_PATCH_OUTPUT_JOURNALS:
        return found[:MAX_PATCH_OUTPUT_JOURNALS], f"patch-output journals truncated at {MAX_PATCH_OUTPUT_JOURNALS} entries"
    return found, None


def inspect_library_view_history(path: Path, value: dict[str, Any]) -> dict[str, Any]:
    view_id = value.get("view_id")
    if not isinstance(view_id, str) or not view_id:
        raise InspectorError("Library View history is missing view_id")
    profile = str(value.get("profile_kind", "generic")).lower().replace("_", "-")
    subsystem = {
        "romm": "RomM Library View History",
        "esde": "ES-DE Library View History",
        "es-de": "ES-DE Library View History",
    }.get(profile, "Library View History")
    operation = new_operation(subsystem, f"{view_id}:{path.stem}", path)
    schema = value.get("schema_version")
    operation.update({
        "schema_version": schema,
        "original_format": "library_view_history",
        "original_state": value.get("operation"),
        "created_at": value.get("timestamp"),
        "updated_at": value.get("timestamp"),
        "entries_total": value.get("planned_count", 0) if isinstance(value.get("planned_count", 0), int) else 0,
    })
    if schema != 1:
        operation.update(
            state="UnknownSchema",
            category="REVIEW_REQUIRED",
            suggested_status="DO_NOT_TOUCH",
            needs_review_reason="unknown Library View history schema",
        )
        return operation
    operation["state"] = "Completed"
    operation["entries_completed"] = operation["entries_total"]
    operation["entries_pending"] = 0
    operation["category"] = "COMPLETED_HISTORY"
    operation["suggested_status"] = "NO_ACTION"
    if isinstance(value.get("destination_root"), str):
        add_unique(operation["destination_paths"], value["destination_root"])
    if isinstance(value.get("manifest_path"), str):
        add_unique(operation["destination_paths"], value["manifest_path"])
    if value.get("success") is False:
        operation["blockers"].append("completed history reports one or more failed entries; no durable resume/rollback envelope exists")
    return operation


def es_de_gamelists_roots(explicit_roots: list[Path] | None) -> list[Path]:
    if explicit_roots:
        roots = [root if root.name == "gamelists" else root / "gamelists" for root in explicit_roots]
    else:
        home = Path(os.environ.get("HOME", "/nonexistent-home"))
        roots = [home / "ES-DE/gamelists"]
    if any(not root.is_absolute() for root in roots):
        raise InspectorError("ES-DE roots must be absolute")
    return sorted(set(roots), key=lambda item: os.fsencode(os.fspath(item)))


def es_de_recovery_files(root: Path) -> tuple[list[Path], str | None]:
    try:
        info = root.lstat()
    except FileNotFoundError:
        return [], None
    except OSError as error:
        return [], f"cannot inspect ES-DE gamelists root {root}: {error}"
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        return [], f"ES-DE gamelists root is not a non-symlink directory: {root}"
    found: list[Path] = []
    try:
        children = []
        for index, child in enumerate(root.iterdir()):
            if index >= MAX_DIRECTORY_ENTRIES:
                return [], f"ES-DE gamelists root exceeds the {MAX_DIRECTORY_ENTRIES}-entry inspection bound: {root}"
            children.append(child)
        children.sort(key=lambda item: os.fsencode(item.name))
        for child in children:
            child_info = child.lstat()
            if stat.S_ISREG(child_info.st_mode) and child.name.endswith(ES_DE_RECOVERY_SUFFIX):
                found.append(child)
            elif stat.S_ISDIR(child_info.st_mode) and not stat.S_ISLNK(child_info.st_mode):
                entries = []
                for index, item in enumerate(child.iterdir()):
                    if index >= MAX_DIRECTORY_ENTRIES:
                        return [], f"ES-DE system directory exceeds the {MAX_DIRECTORY_ENTRIES}-entry inspection bound: {child}"
                    entries.append(item)
                entries.sort(key=lambda item: os.fsencode(item.name))
                found.extend(item for item in entries if item.name.endswith(ES_DE_RECOVERY_SUFFIX))
    except OSError as error:
        return [], f"cannot enumerate ES-DE recovery records under {root}: {error}"
    return found, None


def finalize_operation(item: dict[str, Any]) -> None:
    if item["state"] not in NORMAL_STATES:
        item["state"] = "NeedsReview"
        item["category"] = "REVIEW_REQUIRED"
        item["suggested_status"] = "DO_NOT_TOUCH"
        item["blockers"].append("inspector produced an unrecognized normalized state")
    item["normalized_state"] = item["state"]
    item["recommended_status"] = item["suggested_status"]
    if not item["rollback_evidence"]:
        item["rollback_evidence"] = [{
            "proven": item["rollback_possible"],
            "observations": item["evidence"],
        }]
    if not item["resume_evidence"]:
        item["resume_evidence"] = [{
            "proven": item["resume_possible"],
            "detail": "no exact, executable resume envelope was proven by this read-only inspection",
        }]


def derive_roots(data_root: Path | None, config_root: Path | None) -> tuple[Path, Path]:
    home = Path(os.environ.get("HOME", "/nonexistent-home"))
    if data_root is None:
        override = os.environ.get("EMUWIZ_DATA_HOME")
        if override:
            data_root = Path(override)
        else:
            base = Path(os.environ.get("XDG_DATA_HOME", home / ".local/share"))
            preferred, legacy = base / "emuwiz", base / "archivefs"
            data_root = preferred if preferred.exists() or not legacy.exists() else legacy
    if config_root is None:
        override = os.environ.get("EMUWIZ_CONFIG_HOME")
        if override:
            config_root = Path(override)
        else:
            base = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
            preferred, legacy = base / "emuwiz", base / "archivefs"
            config_root = preferred if preferred.exists() or not legacy.exists() else legacy
    if not data_root.is_absolute() or not config_root.is_absolute(): raise InspectorError("data and config roots must be absolute")
    return data_root, config_root


def inspect_all(
    data_root: Path,
    config_root: Path,
    include_history: bool,
    max_hash_bytes: int,
    explicit_es_de_roots: list[Path] | None = None,
) -> dict[str, Any]:
    started = time.monotonic(); budget = HashBudget(max_hash_bytes)
    operations: list[dict[str, Any]] = []; warnings: list[str] = []; inspection_errors: list[str] = []
    roots = {
        "data_root": os.fspath(data_root), "config_root": os.fspath(config_root),
        "rename_transactions": os.fspath(data_root / "rename-transactions"),
        "shared_transactions": os.fspath(data_root / "shared-cheat-history"),
        "cheat_install_runs": os.fspath(data_root / "cheat-install-runs"),
        "cheat_rollback_runs": os.fspath(data_root / "cheat-rollback-runs"),
        "library_view_history": os.fspath(data_root / "library_views/history"),
        "database_restore_receipts": os.fspath(data_root),
        "patch_output_journals": os.fspath(data_root),
        "es_de_gamelists": [os.fspath(root) for root in es_de_gamelists_roots(explicit_es_de_roots)],
    }

    families = [
        ("Rename / Organisation", data_root / "rename-transactions", "rename"),
        ("Shared Transaction", data_root / "shared-cheat-history", "shared"),
        ("Cheat Install", data_root / "cheat-install-runs", "cheat_install"),
        ("Cheat Rollback", data_root / "cheat-rollback-runs", "cheat_rollback"),
        ("Library View History", data_root / "library_views/history", "library_view"),
    ]
    usable = 0
    for label, root, family in families:
        paths, problem = direct_files(root)
        if problem:
            inspection_errors.append(problem); continue
        for path in paths:
            if family == "rename" and path.name == "recovery-history-state":
                try:
                    sidecar = read_json(path)
                    archived = sidecar.get("archived_transaction_ids", [])
                    if not isinstance(archived, list): raise InspectorError("archived_transaction_ids is not a list")
                    if include_history:
                        item = new_operation("Recovery History Sidecar", "visibility-state", path)
                        item.update(state="Completed", original_state="visibility_only", category="COMPLETED_HISTORY", suggested_status="NO_ACTION", schema_version="recovery-history-v1", original_format="recovery_history_sidecar", entries_total=len(archived), entries_completed=len(archived), entries_pending=0)
                        operations.append(item); usable += 1
                except (InspectorError, OSError) as error:
                    detail = str(error); operations.append(unreadable_operation("Recovery History Sidecar", path, detail)); inspection_errors.append(f"{path}: {detail}")
                continue
            if path.suffix != ".json":
                continue
            try:
                value = read_json(path)
                if family == "rename": item = inspect_rename(path, value, budget)
                elif family == "shared" and (path.name.endswith(".rollback.json") or "preview_id" in value): item = inspect_shared_rollback_receipt(path, value)
                elif family == "shared" and "transaction_state" in value: item = inspect_shared_durable(path, value, budget)
                elif family == "shared": item = inspect_shared_legacy(path, value, budget)
                elif family == "cheat_install": item = inspect_cheat(path, value, budget, rollback=False)
                elif family == "cheat_rollback": item = inspect_cheat(path, value, budget, rollback=True)
                else: item = inspect_library_view_history(path, value)
                usable += 1
            except (InspectorError, OSError, ValueError, TypeError) as error:
                detail = str(error); item = unreadable_operation(label, path, detail); inspection_errors.append(f"{path}: {detail}")
            if include_history or item["category"] != "COMPLETED_HISTORY": operations.append(item)
    patch_paths, patch_problem = patch_output_files(data_root)
    if patch_problem:
        inspection_errors.append(patch_problem)
    for path in patch_paths:
        try:
            item = inspect_patch_output(path, read_json(path), budget)
            usable += 1
        except (InspectorError, OSError, ValueError, TypeError) as error:
            detail = str(error)
            item = unreadable_operation("Standalone Patch Output", path, detail)
            inspection_errors.append(f"{path}: {detail}")
        if include_history or item["category"] != "COMPLETED_HISTORY":
            operations.append(item)
    database_paths, database_problem = direct_files(data_root)
    if database_problem:
        inspection_errors.append(database_problem)
    else:
        for path in database_paths:
            if not (path.name.startswith("library.sqlite3.restore-") and path.name.endswith(".json")):
                continue
            try:
                item = inspect_database_restore(path, read_json(path), budget, data_root)
                usable += 1
            except (InspectorError, OSError, ValueError, TypeError) as error:
                detail = str(error)
                item = unreadable_operation("Database Restore", path, detail)
                inspection_errors.append(f"{path}: {detail}")
            if include_history or item["category"] != "COMPLETED_HISTORY":
                operations.append(item)
    for gamelists_root in es_de_gamelists_roots(explicit_es_de_roots):
        recovery_paths, recovery_problem = es_de_recovery_files(gamelists_root)
        if recovery_problem:
            inspection_errors.append(recovery_problem)
            continue
        for path in recovery_paths:
            try:
                item = inspect_es_de_recovery(
                    path,
                    read_json(path, MAX_ES_DE_RECOVERY_BYTES),
                    budget,
                    gamelists_root,
                )
                usable += 1
            except (InspectorError, OSError, ValueError, TypeError) as error:
                detail = str(error)
                item = unreadable_operation("ES-DE Publication Recovery", path, detail)
                inspection_errors.append(f"{path}: {detail}")
            operations.append(item)
    for item in operations:
        finalize_operation(item)
    operations.sort(key=lambda item: (item["category"] == "COMPLETED_HISTORY", item["subsystem"], item["operation_id"], item["journal_path"]))
    active = [item for item in operations if item["category"] != "COMPLETED_HISTORY"]
    summary = {
        "pending": len(active),
        "pending_only": sum(item["category"] == "PENDING" for item in operations),
        "recoverable_candidates": sum(item["category"] == "RECOVERABLE" for item in operations),
        "safe_rollback_candidates": sum(item["suggested_status"] == "SAFE_ROLLBACK_CANDIDATE" for item in operations),
        "safe_resume_candidates": sum(item["suggested_status"] == "SAFE_RESUME_CANDIDATE" for item in operations),
        "review_required": sum(item["suggested_status"] == "REVIEW_REQUIRED" for item in operations),
        "unsafe": sum(item["suggested_status"] == "DO_NOT_TOUCH" for item in operations),
        "do_not_touch": sum(item["suggested_status"] == "DO_NOT_TOUCH" for item in operations),
        "completed_history": sum(item["category"] == "COMPLETED_HISTORY" for item in operations),
        "subsystems": sorted({item["subsystem"] for item in operations}),
    }
    return {
        "report_schema_version": REPORT_SCHEMA_VERSION, "tool_version": TOOL_VERSION,
        "generated_at": utc_now(), "roots": roots, "operations": operations,
        "summary": summary, "warnings": warnings, "inspection_errors": inspection_errors,
        "inspection_runtime_seconds": round(time.monotonic() - started, 6),
        "read_only": True, "usable_journals": usable,
    }


def exit_code(report: dict[str, Any]) -> int:
    if report["inspection_errors"] and report["usable_journals"] == 0:
        return 3
    if report["summary"]["review_required"] or report["summary"]["do_not_touch"]:
        return 2
    if report["summary"]["pending"]:
        return 1
    return 0


def redact(text: str, enabled: bool) -> str:
    if not enabled: return text
    home = os.environ.get("HOME")
    if not home: return text
    return text.replace(home, "$HOME")


def human_report(report: dict[str, Any], redact_home: bool, verbose: bool = False) -> str:
    summary = report["summary"]
    lines = [
        "EMUWIZ RECOVERY INSPECTOR", "",
        f"Pending operations: {summary['pending']}",
        f"Safe rollback candidates: {summary['safe_rollback_candidates']}",
        f"Safe resume candidates: {summary['safe_resume_candidates']}",
        f"Needs review: {summary['review_required']}",
        f"Unsafe: {summary['unsafe']}",
    ]
    if summary["completed_history"]: lines.append(f"Completed history: {summary['completed_history']}")
    for index, item in enumerate(report["operations"], 1):
        lines.extend(["", f"[{index}] {item['subsystem']}", f"    ID: {item['operation_id']}", f"    State: {item['state']} (original: {item['original_state']})", f"    Category: {item['category']}", f"    Entries: {item['entries_completed']} / {item['entries_total']} complete", f"    Rollback possible: {'yes' if item['rollback_possible'] else 'no'}", f"    Resume possible: {'yes' if item['resume_possible'] else 'no'}", f"    Suggested status: {item['suggested_status']}", f"    Journal: {redact(item['journal_path'], redact_home)}"])
        if item["original_format"] == "patch_output_journal":
            lines.append(f"    Patch format: {item.get('patch_format', 'unknown')}")
            lines.append(f"    Patch identity: {redact(json.dumps(item.get('patch_identity', {}), sort_keys=True), redact_home)}")
            lines.append(f"    Temporary output: {redact((item.get('temporary_paths') or ['unknown'])[0], redact_home)}")
            lines.append(f"    Publication checkpoint: {item.get('publication_checkpoint') or 'none'}")
            lines.append(f"    Verification checkpoint: {item.get('verification_checkpoint') or 'none'}")
            if verbose:
                lines.append("    Filesystem evidence:")
                lines.append("      " + redact(json.dumps(item.get("filesystem_evidence", {}), sort_keys=True, ensure_ascii=False), redact_home))
        if item["needs_review_reason"]: lines.append(f"    Review reason: {redact(item['needs_review_reason'], redact_home)}")
        touched = item["destination_paths"] + item["backup_paths"] + item["temporary_paths"] + item["created_directories"]
        if touched:
            lines.append("    Recovery could touch:")
            lines.extend(f"      - {redact(path, redact_home)}" for path in touched)
        if verbose:
            if item["source_paths"]:
                lines.append("    Sources:")
                lines.extend(f"      - {redact(path, redact_home)}" for path in item["source_paths"])
            if item["entry_checkpoints"]:
                lines.append("    Entry checkpoints:")
                lines.append("      " + redact(json.dumps(item["entry_checkpoints"], sort_keys=True, ensure_ascii=False), redact_home))
            if item["rollback_evidence"]:
                lines.append("    Rollback evidence:")
                lines.append("      " + redact(json.dumps(item["rollback_evidence"], sort_keys=True, ensure_ascii=False), redact_home))
            if item["resume_evidence"]:
                lines.append("    Resume evidence:")
                lines.append("      " + redact(json.dumps(item["resume_evidence"], sort_keys=True, ensure_ascii=False), redact_home))
    if report["inspection_errors"]:
        lines.extend(["", "Inspection errors:"])
        lines.extend(f"  - {redact(error, redact_home)}" for error in report["inspection_errors"])
    lines.extend(["", "No action has been performed."])
    return "\n".join(lines) + "\n"


def recovery_plan(report: dict[str, Any]) -> dict[str, Any]:
    entries = []
    for item in report["operations"]:
        recommendation = item["recommended_status"]
        entries.append({
            "operation_id": item["operation_id"], "subsystem": item["subsystem"],
            "recommended_action": recommendation,
            "evidence": {"rollback": item["rollback_evidence"], "resume": item["resume_evidence"]},
            "blocking_conditions": item["blockers"],
            "affected_paths": {"sources": item["source_paths"], "destinations": item["destination_paths"], "backups": item["backup_paths"], "temporary": item["temporary_paths"], "created_directories": item["created_directories"]},
            "required_preconditions": ["Re-read the journal", "Revalidate every exact path and digest", "Require explicit caller approval", "Use the owning transaction engine", "Never recover an ambiguous operation automatically"],
        })
    return {"schema_version": 1, "tool_version": TOOL_VERSION, "generated_at": report["generated_at"], "descriptive_only": True, "executes_actions": False, "operations": entries}


def write_new_json(path: Path, value: dict[str, Any], forbidden_roots: Iterable[Path]) -> None:
    if not path.is_absolute(): raise InspectorError("output path must be absolute")
    if has_parent_component(path): raise InspectorError("output path must be normalized and contain no '..' components")
    for root in forbidden_roots:
        if path_is_within(path, root): raise InspectorError("report/plan output must not be written inside EmuWiz data or config roots")
    if not path.parent.is_dir(): raise InspectorError("output parent directory must already exist")
    component_problem = symlink_component(path, include_leaf=False)
    if component_problem: raise InspectorError(f"output path is unsafe: {component_problem}")
    try:
        with path.open("x", encoding="utf-8") as handle:
            json.dump(value, handle, indent=2, sort_keys=True, ensure_ascii=False)
            handle.write("\n")
    except FileExistsError as error:
        raise InspectorError(f"refusing to overwrite output file: {path}") from error
    except OSError as error:
        raise InspectorError(f"cannot write explicit output file {path}: {error}") from error


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-root", type=Path)
    parser.add_argument("--config-root", type=Path)
    parser.add_argument("--include-history", action="store_true")
    parser.add_argument("--json", dest="json_path", type=Path, metavar="PATH")
    parser.add_argument("--emit-plan", type=Path, metavar="PATH")
    parser.add_argument("--redact-home", action="store_true")
    parser.add_argument("--verbose", action="store_true")
    parser.add_argument("--es-de-root", action="append", type=Path, default=[], metavar="PATH", help="explicit ES-DE home or gamelists root; repeatable")
    parser.add_argument("--max-hash-bytes", type=int, default=DEFAULT_MAX_HASH_BYTES)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.max_hash_bytes < 0 or args.max_hash_bytes > 1024 * 1024 * 1024:
            raise InspectorError("--max-hash-bytes must be between 0 and 1 GiB")
        data_root, config_root = derive_roots(args.data_root, args.config_root)
        es_de_roots = es_de_gamelists_roots(args.es_de_root or None)
        report = inspect_all(data_root, config_root, args.include_history, args.max_hash_bytes, args.es_de_root or None)
        forbidden = [data_root, config_root, *es_de_roots]
        if args.json_path: write_new_json(args.json_path, report, forbidden)
        if args.emit_plan: write_new_json(args.emit_plan, recovery_plan(report), forbidden)
        sys.stdout.write(human_report(report, args.redact_home, args.verbose))
        return exit_code(report)
    except InspectorError as error:
        print(f"pending-recovery-inspector: {error}", file=sys.stderr)
        return 3


if __name__ == "__main__":
    raise SystemExit(main())
