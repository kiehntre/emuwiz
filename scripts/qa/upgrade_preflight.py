#!/usr/bin/env python3
"""Read-only EmuWiz/ArchiveFS upgrade preflight and backup manifest generator."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shlex
import sqlite3
import stat
import sys
import tomllib
from urllib.parse import quote
from pathlib import Path
from typing import Any

TOOL_VERSION = "1"
CURRENT_SCHEMA = 20
KNOWN_HISTORICAL_SCHEMAS = {3, 6, 12, 16, 19, 20}
SECRET_KEY = re.compile(r"(token|secret|password|passwd|api[_-]?key|credential|bearer)", re.I)
ACTIONABLE_STATES = {
    "applying",
    "applyfailed",
    "rollingback",
    "rollbackfailed",
    "needsreview",
    "unsafetoresume",
    "interrupted",
    "pending",
    "inprogress",
    "partial",
}
COMPLETED_STATES = {"applied", "rolledback", "completed", "complete"}


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def is_present(path: Path) -> bool:
    try:
        path.lstat()
        return True
    except FileNotFoundError:
        return False
    except OSError:
        return True


def is_nonempty(path: Path) -> bool:
    if not is_present(path):
        return False
    if path.is_file() or path.is_symlink():
        return True
    try:
        return any(path.iterdir())
    except OSError:
        return True


def safe_size(path: Path) -> int | None:
    try:
        return path.stat().st_size if path.is_file() else None
    except OSError:
        return None


def redact(value: Any, key: str = "") -> Any:
    if SECRET_KEY.search(key):
        return "<redacted: configured>" if value not in (None, "", False) else "<redacted: not configured>"
    if isinstance(value, dict):
        return {str(k): redact(v, str(k)) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v, key) for v in value]
    return value


def absolute_strings(value: Any, key: str = "") -> list[tuple[str, str]]:
    found: list[tuple[str, str]] = []
    if SECRET_KEY.search(key):
        return found
    if isinstance(value, dict):
        for child_key, child in value.items():
            found.extend(absolute_strings(child, str(child_key)))
    elif isinstance(value, list):
        for child in value:
            found.extend(absolute_strings(child, key))
    elif isinstance(value, str) and os.path.isabs(value):
        found.append((key or "path", value))
    return found


def root_paths(args: argparse.Namespace) -> dict[str, Path]:
    home = Path(os.environ.get("HOME", "~")).expanduser()
    xdg_config = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    xdg_data = Path(os.environ.get("XDG_DATA_HOME", home / ".local" / "share"))
    if not xdg_config.is_absolute():
        xdg_config = home / ".config"
    if not xdg_data.is_absolute():
        xdg_data = home / ".local" / "share"
    emu_config = Path(args.config_root) if args.config_root else Path(os.environ.get("EMUWIZ_CONFIG_HOME", xdg_config / "emuwiz"))
    emu_data = Path(args.data_root) if args.data_root else Path(os.environ.get("EMUWIZ_DATA_HOME", xdg_data / "emuwiz"))
    legacy_config = Path(args.legacy_config_root) if args.legacy_config_root else xdg_config / "archivefs"
    legacy_data = Path(args.legacy_data_root) if args.legacy_data_root else xdg_data / "archivefs"
    return {
        "emuwiz_config": emu_config,
        "archivefs_config": legacy_config,
        "emuwiz_data": emu_data,
        "archivefs_data": legacy_data,
    }


def selected_root(primary: Path, legacy: Path) -> Path:
    return primary if is_present(primary) else legacy if is_present(legacy) else primary


def root_inventory(path: Path) -> dict[str, Any]:
    important = [
        "config.toml",
        "library.sqlite3",
        "index.json",
        "dat_sources.toml",
        "managed-dat-sources.toml",
        "managed-dats",
        "emulator_profiles.toml",
        "rename-transactions",
        "recovery-history-state",
        "library_views.json",
        "library_views",
        "identity",
        "provider",
        "providers",
        "managed-installs",
    ]
    entries = []
    for relative in important:
        child = path / relative
        if is_present(child):
            entries.append({"path": str(child), "relative": relative, "nonempty": is_nonempty(child), "size": safe_size(child)})
    return {"path": str(path), "exists": is_present(path), "empty": not is_nonempty(path), "important_files": entries}


def config_report(path: Path) -> tuple[dict[str, Any], list[tuple[str, str]], list[str]]:
    report: dict[str, Any] = {"path": str(path / "config.toml"), "status": "missing", "format": "absent", "legacy_source_keys": [], "absolute_paths": [], "credentials": []}
    warnings: list[str] = []
    config_path = path / "config.toml"
    if not is_present(config_path):
        return report, [], warnings
    try:
        raw = config_path.read_bytes()
        parsed = tomllib.loads(raw.decode("utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        report.update(status="invalid", format="invalid_toml", error=str(error))
        return report, [], [f"invalid config.toml: {error}"]
    legacy_keys = [key for key in ("source_folders", "sources") if key in parsed]
    current_sources = parsed.get("source", [])
    if not isinstance(current_sources, list):
        current_sources = []
    if legacy_keys and current_sources:
        fmt = "mixed_legacy_current"
        warnings.append("config contains both legacy and current source declarations")
    elif legacy_keys:
        fmt = "legacy_but_readable"
        warnings.append("legacy source declaration is readable but should be reviewed before upgrade")
    elif current_sources:
        fmt = "current_structure"
    else:
        fmt = "current_structure"
    paths = absolute_strings(parsed)
    credentials = []
    def collect_secret_keys(value: Any, prefix: str = "") -> None:
        if isinstance(value, dict):
            for key, child in value.items():
                full = f"{prefix}.{key}" if prefix else str(key)
                if SECRET_KEY.search(str(key)):
                    credentials.append({"field": full, "configured": child not in (None, "", False)})
                collect_secret_keys(child, full)
        elif isinstance(value, list):
            for child in value:
                collect_secret_keys(child, prefix)
    collect_secret_keys(parsed)
    report.update(
        status="readable",
        format=fmt,
        legacy_source_keys=legacy_keys,
        current_source_count=len(current_sources),
        mount_root=parsed.get("mount_root"),
        master_rom_root=parsed.get("master_rom_root"),
        ratarmount_configured=parsed.get("ratarmount_bin", parsed.get("ratarmount", "ratarmount")),
        credentials=credentials,
        absolute_paths=[{"field": key, "path": value} for key, value in paths],
    )
    return report, paths, warnings


def database_report(path: Path) -> tuple[dict[str, Any], list[str], list[str]]:
    report: dict[str, Any] = {"path": str(path), "exists": is_present(path), "status": "absent", "schema": None, "current_target": CURRENT_SCHEMA, "quick_check": None, "schema_migrations_latest": None}
    warnings: list[str] = []
    errors: list[str] = []
    if not is_present(path):
        return report, warnings, errors
    report["size"] = safe_size(path)
    uri = f"file:{quote(str(path.absolute()), safe='/')}?mode=ro"
    try:
        connection = sqlite3.connect(uri, uri=True)
        connection.row_factory = sqlite3.Row
        schema = int(connection.execute("PRAGMA user_version").fetchone()[0])
        report["schema"] = schema
        quick = str(connection.execute("PRAGMA quick_check").fetchone()[0])
        report["quick_check"] = quick
        tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if "schema_migrations" in tables:
            columns = {row[1] for row in connection.execute("PRAGMA table_info(schema_migrations)")}
            if "version" in columns:
                report["schema_migrations_latest"] = connection.execute("SELECT MAX(version) FROM schema_migrations").fetchone()[0]
        if quick != "ok":
            errors.append(f"database quick_check returned {quick}")
        if schema > CURRENT_SCHEMA:
            report["status"] = "newer_unsupported"
            errors.append(f"database schema {schema} is newer than supported schema {CURRENT_SCHEMA}")
        elif schema == CURRENT_SCHEMA:
            report["status"] = "current"
        elif schema in KNOWN_HISTORICAL_SCHEMAS or 1 <= schema < CURRENT_SCHEMA:
            report["status"] = "older_upgradable"
            warnings.append(f"database schema {schema} requires upgrade to {CURRENT_SCHEMA}")
        else:
            report["status"] = "unknown_schema"
            errors.append(f"database schema {schema} is not a recognized historical schema")
        connection.close()
    except (OSError, sqlite3.Error, ValueError) as error:
        report.update(status="unreadable", error=str(error))
        errors.append(f"database is unreadable: {error}")
    return report, warnings, errors


def path_health(label: str, path_string: str) -> dict[str, Any]:
    path = Path(path_string)
    item = {"label": label, "path": path_string, "status": "Unknown"}
    try:
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode):
            item["status"] = "Symlink"
        elif stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode):
            item["status"] = "Exists"
    except FileNotFoundError:
        item["status"] = "Missing"
    except PermissionError:
        item["status"] = "Unavailable"
    except OSError as error:
        item.update(status="Unknown", error=str(error))
    return item


def mount_warning(item: dict[str, Any]) -> str | None:
    if item["status"] != "Missing":
        return None
    path = item["path"]
    if path == "/mnt" or path.startswith("/mnt/") or path == "/media" or path.startswith("/media/"):
        parent = Path(path)
        while parent != parent.parent and str(parent) not in ("/mnt", "/media"):
            parent = parent.parent
        return f"MOUNT MAY BE UNAVAILABLE — DO NOT RESCAN: {path}"
    return None


def journal_files(data_root: Path) -> list[Path]:
    names = {"rename-transactions", "transactions", "recovery", "mod-journals", "cheat-journals", "rollback"}
    roots = [data_root / name for name in names]
    roots.append(data_root / "recovery-history-state")
    found: list[Path] = []
    for root in roots:
        if root.is_file():
            found.append(root)
        elif root.is_dir():
            try:
                for child in root.rglob("*"):
                    if child.is_file() and len(child.relative_to(root).parts) <= 3:
                        found.append(child)
            except OSError:
                continue
    return sorted(set(found))


def state_from_value(value: Any) -> str | None:
    if isinstance(value, dict):
        for key in ("state", "status", "phase", "outcome"):
            if key in value and isinstance(value[key], str):
                return value[key]
        for child in value.values():
            result = state_from_value(child)
            if result:
                return result
    elif isinstance(value, list):
        for child in value:
            result = state_from_value(child)
            if result:
                return result
    return None


def transactions_report(data_root: Path) -> tuple[dict[str, Any], list[str], list[str]]:
    counts: dict[str, int] = {}
    entries = []
    blockers: list[str] = []
    warnings: list[str] = []
    for path in journal_files(data_root):
        try:
            raw = path.read_text(encoding="utf-8")
            parsed = json.loads(raw)
            state = state_from_value(parsed)
            if not state:
                state = "UnknownSchema"
            normalized = re.sub(r"[^a-z]", "", state.lower())
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            state = "UnknownSchema"
            normalized = "unknownschema"
            warnings.append(f"journal could not be parsed conservatively: {path}")
        counts[state] = counts.get(state, 0) + 1
        entries.append({"path": str(path), "state": state, "recognized": normalized != "unknownschema"})
        if normalized in ACTIONABLE_STATES or normalized == "unknownschema":
            blockers.append(f"resolve transaction/recovery journal before upgrade: {path}")
    return {"files": entries, "state_counts": counts, "actionable_count": len(blockers)}, blockers, warnings


def managed_installs_report(config_root: Path, data_root: Path) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    manifests: list[Path] = []
    for base in (config_root, data_root):
        for relative in ("managed-installs", "emulators", "managed-emulators"):
            root = base / relative
            if root.is_dir():
                try:
                    manifests.extend(child for child in root.rglob("manifest.json") if len(child.relative_to(root).parts) <= 3)
                except OSError:
                    pass
    results = []
    warnings = []
    for path in sorted(set(manifests)):
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(document, dict):
                raise ValueError("manifest is not an object")
            binary = document.get("binary", document.get("executable"))
            install_root = document.get("install_root", str(path.parent))
            install_root_path = Path(install_root) if isinstance(install_root, str) else path.parent
            if not install_root_path.is_absolute():
                install_root_path = path.parent / install_root_path
            binary_path = Path(binary) if isinstance(binary, str) and os.path.isabs(binary) else install_root_path / binary if isinstance(binary, str) else None
            binary_status = path_health("managed binary", str(binary_path))["status"] if binary_path else "Unknown"
            results.append({"manifest": str(path), "emulator": document.get("emulator", document.get("name")), "version": document.get("version"), "schema": document.get("schema", document.get("manifest_schema")), "install_root": str(install_root_path), "configured_binary": str(binary_path) if binary_path else None, "binary_status": binary_status})
            if binary_status in {"Missing", "Unavailable"}:
                warnings.append(f"managed emulator binary is {binary_status.lower()}: {binary_path}")
        except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
            warnings.append(f"managed manifest unreadable: {path}: {error}")
            results.append({"manifest": str(path), "status": "unreadable"})
    return results, warnings


def dat_provider_report(config_root: Path, data_root: Path) -> tuple[dict[str, Any], list[tuple[str, str]], list[str]]:
    files = []
    absolute: list[tuple[str, str]] = []
    warnings: list[str] = []
    for relative in ("dat_sources.toml", "managed-dat-sources.toml", "emulator_profiles.toml", "providers.toml", "identity.toml"):
        path = config_root / relative
        if not is_present(path):
            continue
        item: dict[str, Any] = {"path": str(path), "status": "readable"}
        try:
            document = tomllib.loads(path.read_text(encoding="utf-8"))
            paths = absolute_strings(document)
            absolute.extend(paths)
            item["absolute_path_count"] = len(paths)
            item["configured_entries"] = len(document) if isinstance(document, dict) else 0
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            item.update(status="unreadable", error=str(error))
            warnings.append(f"DAT/provider state is unreadable: {path}")
        files.append(item)
    roots = []
    for path in (config_root / "providers", config_root / "identity", data_root / "identity", data_root / "managed-dats", data_root / "provider-cache"):
        if is_present(path):
            roots.append({"path": str(path), "status": "Exists" if is_nonempty(path) else "Empty"})
    snapshots = []
    managed_dat_root = data_root / "managed-dats"
    if managed_dat_root.is_dir():
        try:
            snapshots = [child.name for child in managed_dat_root.iterdir()][:100]
        except OSError:
            warnings.append(f"managed DAT root unavailable: {managed_dat_root}")
    return {"configuration_files": files, "provider_config_roots": roots, "managed_dat_entries_sample": snapshots, "absolute_paths": [{"field": key, "path": value} for key, value in absolute]}, absolute, warnings


def backup_entries(config_root: Path, data_root: Path, discovered: dict[str, Any]) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    seen: set[str] = set()
    def add(path: Path, category: str, priority: str, reason: str, rebuildable: bool, notes: str = "") -> None:
        key = str(path)
        if key in seen or not is_present(path):
            return
        seen.add(key)
        entries.append({"path": key, "category": category, "priority": priority, "reason": reason, "size": safe_size(path), "exists": True, "rebuildable": rebuildable, "notes": notes})
    for path, category, priority, reason in (
        (config_root / "config.toml", "configuration", "MUST_PRESERVE", "source selections and upgrade semantics"),
        (data_root / "library.sqlite3", "catalogue", "MUST_PRESERVE", "authoritative catalogue and schema state"),
        (config_root / "dat_sources.toml", "DAT configuration", "PRESERVE_IF_USED", "DAT source selections"),
        (config_root / "managed-dat-sources.toml", "managed DAT configuration", "PRESERVE_IF_USED", "managed DAT selections and provenance"),
        (data_root / "managed-dats", "managed DAT state", "PRESERVE_IF_USED", "selected DAT snapshots and state"),
        (config_root / "emulator_profiles.toml", "emulator bindings", "MUST_PRESERVE", "explicit emulator selections"),
        (data_root / "rename-transactions", "recovery journals", "MUST_PRESERVE", "transaction and rollback authority"),
        (data_root / "recovery-history-state", "recovery markers", "MUST_PRESERVE", "recovery state"),
        (config_root / "library_views.json", "library views", "PRESERVE_IF_USED", "user-defined view configuration"),
        (data_root / "library_views", "library view history", "PRESERVE_IF_USED", "view manifests and history"),
        (config_root / "identity", "provider configuration", "PRESERVE_IF_USED", "provider mappings and configuration"),
        (data_root / "managed-installs", "managed emulator manifests", "MUST_PRESERVE", "managed install ownership and versions"),
    ):
        add(path, category, priority, reason, False if priority != "MUST_PRESERVE" else False)
    # Cache directories are intentionally not entries in the default manifest.
    discovered["excluded_rebuildable"] = [str(data_root / "identity" / "artwork" / "thumbnails"), str(data_root / "scan-fingerprints"), str(data_root / "provider-cache")]
    return entries


def build_report(args: argparse.Namespace) -> dict[str, Any]:
    roots = root_paths(args)
    selected_config = selected_root(roots["emuwiz_config"], roots["archivefs_config"])
    selected_data = selected_root(roots["emuwiz_data"], roots["archivefs_data"])
    root_conflicts = []
    for kind, primary, legacy in (("config", roots["emuwiz_config"], roots["archivefs_config"]), ("data", roots["emuwiz_data"], roots["archivefs_data"])):
        both = is_present(primary) and is_present(legacy)
        meaningful = both and is_nonempty(primary) and is_nonempty(legacy)
        root_conflicts.append({"kind": kind, "emuwiz": root_inventory(primary), "archivefs": root_inventory(legacy), "both_exist": both, "meaningful_state_in_both": meaningful, "preferred_by_application": str(primary if is_present(primary) else legacy)})
    config, config_paths, config_warnings = config_report(selected_config)
    db, db_warnings, db_errors = database_report(selected_data / "library.sqlite3")
    path_items: list[dict[str, Any]] = []
    for key, value in config_paths:
        item = path_health(key, value)
        path_items.append(item)
    managed, managed_warnings = managed_installs_report(selected_config, selected_data)
    dat_provider, dat_paths, dat_warnings = dat_provider_report(selected_config, selected_data)
    transactions, transaction_blockers, transaction_warnings = transactions_report(selected_data)
    warnings = config_warnings + db_warnings + managed_warnings + dat_warnings + transaction_warnings
    blockers = db_errors + transaction_blockers
    mount_warnings = []
    path_items.extend(path_health(key, value) for key, value in dat_paths)
    for install in managed:
        if install.get("configured_binary"):
            path_items.append(path_health("managed emulator binary", install["configured_binary"]))
        if install.get("install_root"):
            path_items.append(path_health("managed emulator install root", install["install_root"]))
    for item in path_items:
        warning = mount_warning(item)
        if warning:
            mount_warnings.append(warning)
            blockers.append(warning)
        elif item["status"] in {"Missing", "Unavailable"}:
            warnings.append(f"configured path is {item['status'].lower()}: {item['path']}")
    for conflict in root_conflicts:
        if conflict["meaningful_state_in_both"]:
            blockers.append(f"both EmuWiz and ArchiveFS {conflict['kind']} roots contain meaningful state; no automatic merge is safe")
        elif conflict["both_exist"]:
            warnings.append(f"both EmuWiz and ArchiveFS {conflict['kind']} roots exist; application prefers EmuWiz wholesale")
    discovered = {}
    manifest_entries = backup_entries(selected_config, selected_data, discovered)
    manifest_paths = {entry["path"] for entry in manifest_entries}
    for conflict in root_conflicts:
        if not conflict["meaningful_state_in_both"]:
            continue
        for side in ("emuwiz", "archivefs"):
            root = Path(conflict[side]["path"])
            if str(root) not in manifest_paths:
                manifest_entries.append({"path": str(root), "category": f"{conflict['kind']} root conflict", "priority": "MUST_PRESERVE", "reason": "preserve both roots before resolving a precedence conflict", "size": None, "exists": True, "rebuildable": False, "notes": "do not merge automatically"})
                manifest_paths.add(str(root))
    report = {
        "tool_version": TOOL_VERSION,
        "generated_at": now(),
        "config_roots": {key: str(value) for key, value in roots.items() if "config" in key},
        "data_roots": {key: str(value) for key, value in roots.items() if "data" in key},
        "selected_roots": {"config": str(selected_config), "data": str(selected_data)},
        "root_conflicts": root_conflicts,
        "database": db,
        "config": redact(config),
        "persistent_state": {"inventory": root_inventory(selected_config) | {"data": root_inventory(selected_data)}, "excluded_rebuildable": discovered.get("excluded_rebuildable", [])},
        "transactions": transactions,
        "managed_installs": managed,
        "dat_provider": redact(dat_provider),
        "paths": path_items,
        "mount_warnings": mount_warnings,
        "backup_manifest_summary": {"entry_count": len(manifest_entries), "must_preserve": sum(1 for e in manifest_entries if e["priority"] == "MUST_PRESERVE"), "preserve_if_used": sum(1 for e in manifest_entries if e["priority"] == "PRESERVE_IF_USED"), "rebuildable_excluded": discovered.get("excluded_rebuildable", [])},
        "backup_manifest_entries": manifest_entries,
        "blockers": sorted(set(blockers)),
        "warnings": sorted(set(warnings)),
    }
    inspection_error = db.get("status") in {"unreadable", "unknown_schema"} or config.get("status") == "invalid"
    if inspection_error or (db.get("quick_check") not in (None, "ok") and db.get("status") != "newer_unsupported"):
        result = "UNKNOWN"
    elif report["blockers"]:
        result = "BLOCKED"
    elif report["warnings"]:
        result = "SAFE_WITH_WARNINGS"
    else:
        result = "SAFE"
    report["result"] = result
    return report


def human_report(report: dict[str, Any]) -> str:
    roots = report["selected_roots"]
    db = report["database"]
    lines = ["EMUWIZ UPGRADE PREFLIGHT", "", f"Active config root:\n  {roots['config']}", f"Active data root:\n  {roots['data']}", "", "Root conflicts:"]
    for conflict in report["root_conflicts"]:
        state = "both meaningful" if conflict["meaningful_state_in_both"] else "both exist" if conflict["both_exist"] else "single/absent"
        lines.append(f"  {conflict['kind']}: {state}; preferred: {conflict['preferred_by_application']}")
    lines += ["", "Database:", f"  Path: {db['path']}", f"  Schema: {db.get('schema', 'absent')}", f"  Current target: {CURRENT_SCHEMA}", f"  quick_check: {db.get('quick_check', 'not run')}", f"  Status: {db['status']}", "", "Config:", f"  {report['config']['format']}", "", "Transactions:"]
    lines += [f"  {key}: {value}" for key, value in report["transactions"]["state_counts"].items()] or ["  none found"]
    lines += ["", "DAT/provider state:", f"  configuration files: {len(report['dat_provider']['configuration_files'])}", f"  provider roots: {len(report['dat_provider']['provider_config_roots'])}", f"  managed DAT entries sampled: {len(report['dat_provider']['managed_dat_entries_sample'])}", "", "Paths:", f"  healthy: {sum(1 for item in report['paths'] if item['status'] == 'Exists')}", f"  missing/unavailable: {sum(1 for item in report['paths'] if item['status'] in {'Missing', 'Unavailable'})}", "", "Backup manifest:", f"  {report['backup_manifest_summary']['entry_count']} authority/config entries; rebuildable caches excluded by default", "", f"Result: {report['result']}"]
    if report["blockers"]:
        lines.append("\nBlockers:")
        lines.extend(f"  - {item}" for item in report["blockers"])
    if report["warnings"]:
        lines.append("\nWarnings:")
        lines.extend(f"  - {item}" for item in report["warnings"])
    return "\n".join(lines) + "\n"


def write_backup_manifest(report: dict[str, Any], path: Path) -> None:
    payload = {"tool_version": report["tool_version"], "generated_at": report["generated_at"], "result": report["result"], "selected_roots": report["selected_roots"], "entries": report["backup_manifest_entries"], "excluded_rebuildable": report["backup_manifest_summary"]["rebuildable_excluded"]}
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def ensure_output_outside_inspected_roots(path: Path, report: dict[str, Any]) -> None:
    candidate = path.absolute()
    roots = [Path(value) for value in report["selected_roots"].values()]
    for root in roots:
        if candidate == root or root in candidate.parents:
            raise ValueError(f"report output must be outside inspected root: {candidate}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Read-only EmuWiz/ArchiveFS upgrade preflight")
    parser.add_argument("--config-root")
    parser.add_argument("--data-root")
    parser.add_argument("--legacy-config-root")
    parser.add_argument("--legacy-data-root")
    parser.add_argument("--json", metavar="PATH", help="write machine-readable report")
    parser.add_argument("--backup-manifest", metavar="PATH", help="write a backup manifest; no files are copied")
    args = parser.parse_args()
    try:
        report = build_report(args)
        print(human_report(report), end="")
        if args.json:
            json_path = Path(args.json)
            ensure_output_outside_inspected_roots(json_path, report)
            json_path.parent.mkdir(parents=True, exist_ok=True)
            json_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        if args.backup_manifest:
            manifest_path = Path(args.backup_manifest)
            ensure_output_outside_inspected_roots(manifest_path, report)
            write_backup_manifest(report, manifest_path)
        if report["database"].get("status") == "unreadable" or report["config"].get("status") == "invalid":
            return 3
        return {"SAFE": 0, "SAFE_WITH_WARNINGS": 1, "BLOCKED": 2, "UNKNOWN": 3}[report["result"]]
    except Exception as error:  # inspection errors are distinct from a blocked installation
        print(f"UPGRADE PREFLIGHT ERROR: {error}", file=sys.stderr)
        return 3


if __name__ == "__main__":
    raise SystemExit(main())
