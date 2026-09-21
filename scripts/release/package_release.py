#!/usr/bin/env python3
"""Package and verify already-built EmuWiz release artifacts.

This module deliberately never executes a supplied artifact and never invokes
Cargo to build one.  It uses only the Python standard library and optional
read-only host inspection tools.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import importlib.util
import json
import os
import pathlib
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from typing import Any, Iterable

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
_previous_bytecode_setting = sys.dont_write_bytecode
sys.dont_write_bytecode = True
try:
    from release_signing import (  # noqa: E402
        SigningError,
        inspect_signing_key,
        sign_detached_gpg,
        verify_detached_gpg,
    )
finally:
    sys.dont_write_bytecode = _previous_bytecode_setting


SCHEMA_VERSION = 1
PACKAGER_VERSION = "1.0.0"
SBOM_BUNDLE_SCHEMA = 1
SBOM_FILES = (
    "emuwiz-sbom.cdx.json",
    "third-party-licenses.json",
    "THIRD_PARTY_LICENSES.txt",
    "dependency-summary.json",
)
SBOM_CHECKSUM_FILE = "SBOM_SHA256SUMS"
MARKER_NAME = ".emuwiz-release-package.json"
FINAL_METADATA = {"manifest.json", "SHA256SUMS"}
EXIT_INPUT = 1
EXIT_VERIFY = 2
EXIT_UNSAFE = 3
EXIT_PROVENANCE = 4
EXIT_TOOL = 5
SECRET_PATTERNS = (
    re.compile(rb"token\s*=", re.IGNORECASE),
    re.compile(rb"api_key\s*=", re.IGNORECASE),
    re.compile(rb"password\s*=", re.IGNORECASE),
    re.compile(rb"Authorization\s*:", re.IGNORECASE),
    re.compile(rb"Bearer\s+", re.IGNORECASE),
)
TEXT_SUFFIXES = {".txt", ".md", ".json", ".toml", ".yaml", ".yml", ".ini", ".cfg"}
DENIED_PAYLOAD_NAMES = {
    ".config",
    ".local",
    "library.sqlite3",
    "config.toml",
    "rename-transactions",
    "identity",
    "managed-dats",
    "roms",
    "bios",
    "saves",
    "journals",
}


class ReleaseError(Exception):
    def __init__(self, message: str, code: int = EXIT_TOOL):
        super().__init__(message)
        self.code = code


def log(stage: int, message: str) -> None:
    print(f"[{stage}/9] {message}", flush=True)


def run_readonly(args: list[str], cwd: pathlib.Path | None = None) -> str | None:
    if shutil.which(args[0]) is None:
        return None
    try:
        result = subprocess.run(
            args,
            cwd=cwd,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def json_bytes(value: Any) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode()


def atomic_write(path: pathlib.Path, data: bytes, mode: int = 0o644) -> None:
    temporary = path.with_name(f".{path.name}.incomplete")
    with temporary.open("wb") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    os.chmod(temporary, mode)
    os.replace(temporary, path)


def repository_root() -> pathlib.Path:
    return pathlib.Path(__file__).resolve().parents[2]


def project_version(source_root: pathlib.Path) -> str:
    cargo_toml = source_root / "Cargo.toml"
    if not cargo_toml.is_file():
        raise ReleaseError(f"Cargo workspace metadata not found: {cargo_toml}", EXIT_PROVENANCE)
    try:
        import tomllib

        parsed = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
        value = parsed.get("workspace", {}).get("package", {}).get("version")
    except (ImportError, OSError, ValueError):
        value = None
    if not isinstance(value, str) or not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+-]*", value):
        raise ReleaseError("workspace.package.version is missing or invalid", EXIT_PROVENANCE)
    return value


def source_provenance(source_root: pathlib.Path) -> dict[str, Any]:
    commit = run_readonly(["git", "rev-parse", "HEAD"], source_root)
    branch = run_readonly(["git", "branch", "--show-current"], source_root)
    status = run_readonly(["git", "status", "--porcelain=v1", "--untracked-files=normal"], source_root)
    if commit is None or status is None:
        raise ReleaseError("source root is not a readable Git worktree", EXIT_PROVENANCE)
    return {
        "commit": commit,
        "branch": branch or "detached",
        "clean": not bool(status),
    }


def tool_commit(relative_path: str) -> str:
    result = run_readonly(["git", "log", "-1", "--format=%H", "--", relative_path], repository_root())
    if result:
        return result
    fallback = run_readonly(["git", "rev-parse", "HEAD"], repository_root())
    if not fallback:
        raise ReleaseError(f"release tool provenance is unavailable: {relative_path}", EXIT_PROVENANCE)
    return fallback


def sbom_tool() -> Any:
    path = repository_root() / "scripts" / "release" / "generate_sbom.py"
    spec = importlib.util.spec_from_file_location("emuwiz_release_sbom", path)
    if spec is None or spec.loader is None:
        raise ReleaseError("SBOM verifier could not be loaded", EXIT_TOOL)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    previous = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = previous
    return module


def validate_sbom_input(path: pathlib.Path, source_root: pathlib.Path, product_source_sha: str) -> dict[str, Any]:
    path = path.expanduser().resolve()
    if path.is_symlink() or not path.is_dir():
        raise ReleaseError(f"SBOM directory is missing or unsafe: {path}", EXIT_VERIFY)
    actual_entries = list(path.iterdir())
    expected_names = set(SBOM_FILES) | {SBOM_CHECKSUM_FILE}
    if {entry.name for entry in actual_entries} != expected_names:
        raise ReleaseError("SBOM directory contains an unexpected or missing member", EXIT_VERIFY)
    for entry in actual_entries:
        if entry.is_symlink() or not entry.is_file() or bool(entry.stat().st_mode & 0o111):
            raise ReleaseError(f"SBOM member is not a non-executable regular file: {entry.name}", EXIT_VERIFY)
    verifier = sbom_tool()
    try:
        verifier.verify(argparse.Namespace(source_root=str(source_root), sbom_dir=str(path), strict=False))
    except Exception as error:
        raise ReleaseError(f"SBOM verification failed: {error}", EXIT_VERIFY) from error
    try:
        summary = json.loads((path / "dependency-summary.json").read_text(encoding="utf-8"))
        sbom = json.loads((path / "emuwiz-sbom.cdx.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ReleaseError(f"SBOM metadata is unreadable: {error}", EXIT_VERIFY) from error
    if summary.get("schema_version") != SBOM_BUNDLE_SCHEMA:
        raise ReleaseError("unsupported SBOM bundle schema", EXIT_VERIFY)
    if sbom.get("bomFormat") != "CycloneDX" or sbom.get("specVersion") != "1.5":
        raise ReleaseError("unsupported CycloneDX SBOM schema", EXIT_VERIFY)
    if summary.get("source_commit") != product_source_sha:
        raise ReleaseError("SBOM product source SHA does not match packaged source", EXIT_PROVENANCE)
    if summary.get("sbom_tool_sha") != verifier.tool_commit():
        raise ReleaseError("SBOM tool provenance does not match the verified generator", EXIT_PROVENANCE)
    return {
        "summary": summary,
        "cyclonedx": sbom,
        "product_source_sha": summary["source_commit"],
        "sbom_tool_sha": summary["sbom_tool_sha"],
        "cargo_lock_sha256": summary["cargo_lock_sha256"],
    }


def copy_sbom_payload(source: pathlib.Path, destination: pathlib.Path) -> list[dict[str, Any]]:
    destination.mkdir(parents=True, exist_ok=True)
    records: list[dict[str, Any]] = []
    for name in (*SBOM_FILES, SBOM_CHECKSUM_FILE):
        target = destination / name
        shutil.copyfile(source / name, target)
        os.chmod(target, 0o644)
        records.append({"path": f"SBOM/{name}", "kind": "checksum" if name == SBOM_CHECKSUM_FILE else "sbom"})
    return records


def normalise_arch(value: str) -> str:
    aliases = {"amd64": "x86_64", "x64": "x86_64", "arm64": "aarch64"}
    value = aliases.get(value.lower(), value.lower())
    if not re.fullmatch(r"[a-z0-9_+-]+", value):
        raise ReleaseError(f"invalid architecture: {value}", EXIT_INPUT)
    return value


def elf_info(path: pathlib.Path) -> dict[str, Any]:
    header = path.read_bytes()[:20]
    if len(header) < 20 or header[:4] != b"\x7fELF":
        return {"is_elf": False, "elf_arch": None, "elf_class": None, "needed": [], "rpath": []}
    endian = "little" if header[5] == 1 else "big"
    machine = int.from_bytes(header[18:20], endian)
    arches = {3: "x86", 40: "arm", 62: "x86_64", 183: "aarch64", 243: "riscv64"}
    info: dict[str, Any] = {
        "is_elf": True,
        "elf_arch": arches.get(machine, f"elf-machine-{machine}"),
        "elf_class": {1: "ELF32", 2: "ELF64"}.get(header[4], "unknown"),
        "needed": [],
        "rpath": [],
    }
    dynamic = run_readonly(["readelf", "-dW", str(path)])
    if dynamic:
        for line in dynamic.splitlines():
            needed = re.search(r"\(NEEDED\).*\[([^]]+)]", line)
            if needed:
                info["needed"].append(needed.group(1))
            route = re.search(r"\((?:RPATH|RUNPATH)\).*\[([^]]+)]", line)
            if route:
                info["rpath"].append(route.group(1))
    info["needed"] = sorted(set(info["needed"]))
    info["rpath"] = sorted(set(info["rpath"]))
    return info


def validate_binary(
    path: pathlib.Path,
    requested_arch: str,
    allow_symlink: bool,
    require_elf: bool,
) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except FileNotFoundError as error:
        raise ReleaseError(f"required artifact is missing: {path}", EXIT_INPUT) from error
    if stat.S_ISLNK(metadata.st_mode):
        if not allow_symlink:
            raise ReleaseError(f"artifact is a symlink (use --allow-symlink explicitly): {path}", EXIT_INPUT)
        resolved = path.resolve(strict=True)
        metadata = resolved.stat()
        path = resolved
    if not stat.S_ISREG(metadata.st_mode):
        raise ReleaseError(f"artifact is not a regular file: {path}", EXIT_INPUT)
    if metadata.st_size == 0:
        raise ReleaseError(f"artifact is zero bytes: {path}", EXIT_INPUT)
    if not os.access(path, os.X_OK):
        raise ReleaseError(f"artifact is not executable: {path}", EXIT_INPUT)
    info = elf_info(path)
    if require_elf and not info["is_elf"]:
        raise ReleaseError(f"artifact is not ELF: {path}", EXIT_INPUT)
    if info["is_elf"] and info["elf_arch"] != requested_arch:
        raise ReleaseError(
            f"artifact architecture {info['elf_arch']} does not match requested {requested_arch}: {path}",
            EXIT_INPUT,
        )
    absolute_rpaths = [entry for entry in info["rpath"] if any(part.startswith("/") for part in entry.split(":"))]
    info["unexpected_absolute_rpath"] = absolute_rpaths
    info["source_path"] = path
    info["size"] = metadata.st_size
    return info


def validate_gui_identity(path: pathlib.Path, fixture_mode: bool) -> dict[str, str]:
    """Prove that the release GUI is the native v2 entrypoint."""
    if fixture_mode:
        return {
            "target": "emuwiz",
            "entrypoint": "crates/archivefs-gui/src/bin/emuwiz.rs",
            "generation": "fixture",
            "version_probe": "fixture",
        }
    version = run_readonly([str(path), "--version"])
    if not version or "GUI v2" not in version:
        raise ReleaseError(
            f"GUI binary does not identify the native v2 release experience: {path}",
            EXIT_INPUT,
        )
    return {
        "target": "emuwiz",
        "entrypoint": "crates/archivefs-gui/src/bin/emuwiz.rs",
        "generation": "native-v2",
        "version_probe": version,
    }


def safe_output_root(path: pathlib.Path) -> pathlib.Path:
    path = path.expanduser().resolve()
    home = pathlib.Path.home().resolve()
    forbidden = {
        pathlib.Path("/"),
        home,
        pathlib.Path("/home"),
        pathlib.Path("/mnt"),
        pathlib.Path("/usr"),
        pathlib.Path("/opt"),
        pathlib.Path("/etc"),
    }
    if path in forbidden:
        raise ReleaseError(f"unsafe output root refused: {path}", EXIT_UNSAFE)
    return path


def owned_release_directory(path: pathlib.Path, expected_name: str) -> bool:
    marker = path / MARKER_NAME
    try:
        value = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    return value.get("owner") == "emuwiz-release-packager" and value.get("release_directory") == expected_name


def remove_owned_release(path: pathlib.Path, expected_name: str) -> None:
    if not owned_release_directory(path, expected_name):
        raise ReleaseError(
            f"refusing to replace unowned output directory (missing valid {MARKER_NAME}): {path}", EXIT_UNSAFE
        )
    shutil.rmtree(path)


def discover_binaries(args: argparse.Namespace) -> dict[str, pathlib.Path]:
    values: dict[str, pathlib.Path] = {}
    target = pathlib.Path(args.target_dir).expanduser() if args.target_dir else None
    release_dir = None
    if target is not None:
        release_dir = target / "release"
        if target.name == "release":
            release_dir = target
    for kind, name in (("gui", "emuwiz"), ("cli", "emuwiz-cli")):
        explicit = getattr(args, kind)
        if explicit:
            values[kind] = pathlib.Path(explicit).expanduser()
        elif release_dir is not None:
            values[kind] = release_dir / name
        else:
            raise ReleaseError(f"--{kind} or --target-dir is required", EXIT_INPUT)
    return values


def timestamp_from_environment(reproducible: bool) -> tuple[int, str]:
    raw = os.environ.get("SOURCE_DATE_EPOCH")
    if reproducible and raw is None:
        raise ReleaseError("--reproducible requires SOURCE_DATE_EPOCH", EXIT_PROVENANCE)
    if raw is not None:
        try:
            epoch = int(raw)
        except ValueError as error:
            raise ReleaseError("SOURCE_DATE_EPOCH must be a non-negative integer", EXIT_PROVENANCE) from error
        if epoch < 0:
            raise ReleaseError("SOURCE_DATE_EPOCH must be a non-negative integer", EXIT_PROVENANCE)
    else:
        epoch = int(dt.datetime.now(tz=dt.timezone.utc).timestamp())
    rendered = dt.datetime.fromtimestamp(epoch, tz=dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    return epoch, rendered


def payload_files(root: pathlib.Path) -> list[pathlib.Path]:
    files: list[pathlib.Path] = []
    for candidate in root.rglob("*"):
        relative = candidate.relative_to(root)
        if any(part.lower() in DENIED_PAYLOAD_NAMES for part in relative.parts):
            raise ReleaseError(f"user-state path is forbidden in release payload: {relative}", EXIT_UNSAFE)
        if candidate.is_symlink():
            raise ReleaseError(f"release payload contains a symlink: {relative}", EXIT_UNSAFE)
        if candidate.is_file():
            files.append(candidate)
        elif not candidate.is_dir():
            raise ReleaseError(f"release payload contains a special file: {relative}", EXIT_UNSAFE)
    return sorted(files, key=lambda item: item.relative_to(root).as_posix())


def scan_secrets(root: pathlib.Path) -> None:
    failures: list[str] = []
    for path in payload_files(root):
        if path.suffix.lower() not in TEXT_SUFFIXES and path.name not in {"SHA256SUMS"}:
            continue
        data = path.read_bytes()
        if any(pattern.search(data) for pattern in SECRET_PATTERNS):
            failures.append(path.relative_to(root).as_posix())
    if failures:
        for relative in failures:
            print(f"possible secret marker found in {relative}", file=sys.stderr)
        raise ReleaseError("payload secret-marker scan failed", EXIT_INPUT)


def copy_license_payload(source_root: pathlib.Path, docs: pathlib.Path) -> list[str]:
    candidates = sorted(
        path for path in source_root.iterdir()
        if path.is_file() and re.fullmatch(r"(?:LICENSE|COPYING|NOTICE)(?:\.[A-Za-z0-9._-]+)?", path.name)
    )
    copied: list[str] = []
    license_dir = docs / "licenses"
    license_dir.mkdir(parents=True, exist_ok=True)
    for source in candidates:
        destination = license_dir / source.name
        shutil.copyfile(source, destination)
        os.chmod(destination, 0o644)
        copied.append(f"licenses/{source.name}")
    inventory = ["EmuWiz licence files included:"]
    inventory.extend(f"- {name}" for name in copied)
    if not copied:
        inventory.append("- no repository licence file found")
    inventory.extend(["", "Known limitation: third-party licence inventory not generated.", ""])
    atomic_write(docs / "LICENSES.txt", "\n".join(inventory).encode())
    return copied


def write_generated_docs(root: pathlib.Path, version: str, release_platform: str) -> None:
    docs = root / "docs"
    docs.mkdir(parents=True, exist_ok=True)
    readme = f"""EmuWiz {version} for {release_platform}

bin/emuwiz is the native EmuWiz GUI v2 application.
bin/emuwiz-cli is the command-line application.

Verify this directory from its top level with:
  sha256sum -c SHA256SUMS
  scripts/release/verify-release.sh DIRECTORY

ROMs, BIOS/firmware, saves, configuration, catalogues, and user data are not
included. Configuration and data directories are created at runtime.
"""
    verify = """EmuWiz release verification

Run `sha256sum -c SHA256SUMS` from this directory for a basic byte check.
If SBOM/ is present, also run:
  (cd SBOM && sha256sum -c SBOM_SHA256SUMS)
For manifest, size, ELF architecture, symlink, and strict-layout checks run:
  scripts/release/verify-release.sh --strict DIRECTORY

The verifier inspects files and never executes packaged binaries. Dynamic
dependency names are informational and do not prove availability on another
Linux installation.
"""
    atomic_write(docs / "README.txt", readme.encode())
    atomic_write(root / "VERIFY.txt", verify.encode())


def file_record(root: pathlib.Path, path: pathlib.Path) -> dict[str, Any]:
    return {
        "path": path.relative_to(root).as_posix(),
        "size": path.stat().st_size,
        "sha256": sha256_file(path),
        "executable": bool(path.stat().st_mode & 0o111),
    }


def create_archive(release_dir: pathlib.Path, archive_path: pathlib.Path, epoch: int) -> None:
    temporary = archive_path.with_name(f".{archive_path.name}.incomplete")
    with tarfile.open(temporary, "w:xz", format=tarfile.PAX_FORMAT, preset=9) as archive:
        paths = [release_dir] + sorted(release_dir.rglob("*"), key=lambda item: item.relative_to(release_dir).as_posix())
        for path in paths:
            relative = pathlib.Path(release_dir.name) / path.relative_to(release_dir)
            info = archive.gettarinfo(str(path), arcname=relative.as_posix())
            info.uid = 0
            info.gid = 0
            info.uname = ""
            info.gname = ""
            info.mtime = epoch
            info.mode = 0o755 if path.is_dir() or bool(path.stat().st_mode & 0o111) else 0o644
            if info.isfile():
                with path.open("rb") as source:
                    archive.addfile(info, source)
            else:
                archive.addfile(info)
    os.chmod(temporary, 0o644)
    os.replace(temporary, archive_path)


def safe_archive_members(archive: tarfile.TarFile) -> tuple[str, list[tarfile.TarInfo]]:
    members = archive.getmembers()
    if not members:
        raise ReleaseError("archive is empty", EXIT_VERIFY)
    roots: set[str] = set()
    names: set[str] = set()
    for member in members:
        pure = pathlib.PurePosixPath(member.name)
        if pure.is_absolute() or not pure.parts or ".." in pure.parts:
            raise ReleaseError(f"unsafe archive member path: {member.name}", EXIT_VERIFY)
        roots.add(pure.parts[0])
        if member.name in names:
            raise ReleaseError(f"duplicate archive member: {member.name}", EXIT_VERIFY)
        names.add(member.name)
        if not (member.isfile() or member.isdir()):
            raise ReleaseError(f"unsafe archive member type: {member.name}", EXIT_VERIFY)
    if len(roots) != 1:
        raise ReleaseError("archive must contain exactly one top-level directory", EXIT_VERIFY)
    root = next(iter(roots))
    root_member = next((item for item in members if item.name.rstrip("/") == root), None)
    if root_member is None or not root_member.isdir():
        raise ReleaseError("archive top-level member must be a directory", EXIT_VERIFY)
    return root, members


def read_manifest(root: pathlib.Path) -> dict[str, Any]:
    path = root / "manifest.json"
    if path.is_symlink() or not path.is_file():
        raise ReleaseError("manifest.json is missing or unsafe", EXIT_VERIFY)
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ReleaseError("manifest.json is unreadable", EXIT_VERIFY) from error
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise ReleaseError(f"unsupported manifest schema: {manifest.get('schema_version')}", EXIT_VERIFY)
    return manifest


def parse_checksum_file(path: pathlib.Path) -> dict[str, str]:
    if path.is_symlink() or not path.is_file():
        raise ReleaseError("SHA256SUMS is missing or unsafe", EXIT_VERIFY)
    result: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^\r\n]+)", line)
        if not match or match.group(2) in result:
            raise ReleaseError("SHA256SUMS contains an invalid or duplicate record", EXIT_VERIFY)
        result[match.group(2)] = match.group(1)
    return result


def verify_packaged_sbom(root: pathlib.Path, manifest: dict[str, Any], source_root: pathlib.Path | None) -> None:
    section = manifest.get("sbom")
    if not isinstance(section, dict) or section.get("present") is not True:
        raise ReleaseError("SBOM manifest section is invalid", EXIT_VERIFY)
    if section.get("format") != "CycloneDX" or section.get("spec_version") != "1.5":
        raise ReleaseError("unsupported packaged SBOM schema", EXIT_VERIFY)
    if section.get("bundle_schema") != SBOM_BUNDLE_SCHEMA:
        raise ReleaseError("unsupported packaged SBOM bundle schema", EXIT_VERIFY)
    declared = section.get("files")
    expected_names = set(SBOM_FILES) | {SBOM_CHECKSUM_FILE}
    manifest_files = {
        item.get("path"): item
        for item in manifest.get("files", [])
        if isinstance(item, dict)
    }
    if not isinstance(declared, list) or any(not isinstance(item, dict) for item in declared):
        raise ReleaseError("packaged SBOM file declaration is incomplete", EXIT_VERIFY)
    declared_names = {item.get("path", "").removeprefix("SBOM/") for item in declared}
    if declared_names != expected_names:
        raise ReleaseError("packaged SBOM file declaration is incomplete", EXIT_VERIFY)
    for item in declared:
        relative = item.get("path") if isinstance(item, dict) else None
        if (
            not isinstance(relative, str)
            or relative not in manifest_files
            or item.get("kind") != ("checksum" if relative == f"SBOM/{SBOM_CHECKSUM_FILE}" else "sbom")
            or item.get("size") != manifest_files[relative].get("size")
            or item.get("sha256") != manifest_files[relative].get("sha256")
        ):
            raise ReleaseError("packaged SBOM file declaration disagrees with manifest", EXIT_VERIFY)
    bundle = root / "SBOM"
    if bundle.is_symlink() or not bundle.is_dir():
        raise ReleaseError("packaged SBOM directory is missing or unsafe", EXIT_VERIFY)
    for entry in bundle.iterdir():
        if entry.is_symlink() or not entry.is_file() or bool(entry.stat().st_mode & 0o111):
            raise ReleaseError(f"unsafe SBOM payload member: {entry.name}", EXIT_VERIFY)
    inner = parse_checksum_file(bundle / SBOM_CHECKSUM_FILE)
    if set(inner) != set(SBOM_FILES):
        raise ReleaseError("SBOM_SHA256SUMS does not cover the expected files", EXIT_VERIFY)
    for name, digest in inner.items():
        if sha256_file(bundle / name) != digest:
            raise ReleaseError(f"SBOM checksum mismatch: {name}", EXIT_VERIFY)
    try:
        cyclonedx = json.loads((bundle / SBOM_FILES[0]).read_text(encoding="utf-8"))
        summary = json.loads((bundle / SBOM_FILES[3]).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ReleaseError(f"packaged SBOM metadata is unreadable: {error}", EXIT_VERIFY) from error
    if cyclonedx.get("bomFormat") != "CycloneDX" or cyclonedx.get("specVersion") != "1.5":
        raise ReleaseError("unsupported packaged CycloneDX schema", EXIT_VERIFY)
    if summary.get("schema_version") != SBOM_BUNDLE_SCHEMA:
        raise ReleaseError("unsupported packaged SBOM bundle schema", EXIT_VERIFY)
    release = manifest.get("release", {})
    if summary.get("source_commit") != release.get("commit"):
        raise ReleaseError("packaged SBOM source provenance does not match release", EXIT_PROVENANCE)
    for key in ("cargo_lock_sha256", "product_source_sha", "sbom_tool_sha"):
        if section.get(key) != (summary.get("cargo_lock_sha256") if key == "cargo_lock_sha256" else summary.get({"product_source_sha": "source_commit", "sbom_tool_sha": "sbom_tool_sha"}[key])):
            raise ReleaseError(f"packaged SBOM provenance mismatch: {key}", EXIT_PROVENANCE)
    if source_root is not None:
        verifier = sbom_tool()
        try:
            verifier.verify(argparse.Namespace(source_root=str(source_root), sbom_dir=str(bundle), strict=False))
        except Exception as error:
            raise ReleaseError(f"packaged SBOM verification failed: {error}", EXIT_VERIFY) from error


def verify_directory(
    root: pathlib.Path,
    strict: bool,
    quiet: bool = False,
    source_root: pathlib.Path | None = None,
) -> dict[str, Any]:
    root = root.resolve()
    if not root.is_dir():
        raise ReleaseError(f"release directory not found: {root}", EXIT_VERIFY)
    manifest = read_manifest(root)
    checksums = parse_checksum_file(root / "SHA256SUMS")
    records = manifest.get("files")
    artifacts = manifest.get("artifacts")
    if not isinstance(records, list) or not isinstance(artifacts, list):
        raise ReleaseError("manifest files/artifacts are invalid", EXIT_VERIFY)
    gui = manifest.get("gui")
    if (
        not isinstance(gui, dict)
        or gui.get("target") != "emuwiz"
        or gui.get("generation") not in {"native-v2", "fixture"}
        or gui.get("entrypoint") != "crates/archivefs-gui/src/bin/emuwiz.rs"
    ):
        raise ReleaseError("manifest does not prove the canonical native GUI target", EXIT_VERIFY)
    expected: dict[str, dict[str, Any]] = {}
    for record in records:
        if not isinstance(record, dict) or not isinstance(record.get("path"), str):
            raise ReleaseError("manifest file record is invalid", EXIT_VERIFY)
        relative = pathlib.PurePosixPath(record["path"])
        if relative.is_absolute() or ".." in relative.parts or record["path"] in expected:
            raise ReleaseError(f"unsafe or duplicate manifest path: {record.get('path')}", EXIT_VERIFY)
        expected[record["path"]] = record
    expected_checksum_paths = set(expected) | {"manifest.json"}
    if set(checksums) != expected_checksum_paths:
        raise ReleaseError("SHA256SUMS file set does not agree with manifest", EXIT_VERIFY)
    for relative, record in expected.items():
        path = root / pathlib.Path(relative)
        if path.is_symlink() or not path.is_file():
            raise ReleaseError(f"expected file missing or symlinked: {relative}", EXIT_VERIFY)
        if path.stat().st_size != record.get("size"):
            raise ReleaseError(f"size mismatch: {relative}", EXIT_VERIFY)
        if bool(path.stat().st_mode & 0o111) != record.get("executable"):
            raise ReleaseError(f"executable-mode mismatch: {relative}", EXIT_VERIFY)
        digest = sha256_file(path)
        if digest != record.get("sha256") or digest != checksums.get(relative):
            raise ReleaseError(f"SHA-256 mismatch: {relative}", EXIT_VERIFY)
    manifest_digest = sha256_file(root / "manifest.json")
    if checksums.get("manifest.json") != manifest_digest:
        raise ReleaseError("manifest SHA-256 does not agree with SHA256SUMS", EXIT_VERIFY)
    for artifact in artifacts:
        relative = artifact.get("path") if isinstance(artifact, dict) else None
        if relative not in expected:
            raise ReleaseError("artifact does not reference a manifest file", EXIT_VERIFY)
        path = root / relative
        observed = elf_info(path)
        if observed["elf_arch"] != artifact.get("elf_arch"):
            raise ReleaseError(f"ELF architecture mismatch: {relative}", EXIT_VERIFY)
        if artifact.get("sha256") != expected[relative]["sha256"]:
            raise ReleaseError(f"artifact hash disagrees with file record: {relative}", EXIT_VERIFY)
        if artifact.get("size") != expected[relative]["size"]:
            raise ReleaseError(f"artifact size disagrees with file record: {relative}", EXIT_VERIFY)
    gui_record = next((item for item in artifacts if item.get("path") == "bin/emuwiz"), None)
    if gui_record is None or gui.get("elf_sha256") != gui_record.get("sha256"):
        raise ReleaseError("manifest GUI ELF identity is missing or inconsistent", EXIT_VERIFY)
    actual = {path.relative_to(root).as_posix() for path in payload_files(root)}
    allowed = set(expected) | FINAL_METADATA
    extras = sorted(actual - allowed)
    has_sbom = isinstance(manifest.get("sbom"), dict) and manifest["sbom"].get("present") is True
    if has_sbom:
        verify_packaged_sbom(root, manifest, source_root)
    elif strict and (root / "SBOM").exists():
        raise ReleaseError("SBOM directory exists but is not declared in manifest", EXIT_VERIFY)
    if extras and strict:
        raise ReleaseError(f"unexpected payload files in strict mode: {', '.join(extras)}", EXIT_VERIFY)
    if extras and not quiet:
        print(f"warning: unexpected payload files: {', '.join(extras)}", file=sys.stderr)
    if not quiet:
        print(f"verified {len(expected)} payload files in {root}")
    return {"manifest": manifest, "extras": extras}


def verify_archive(
    path: pathlib.Path,
    strict: bool,
    expected_sha256: str | None = None,
    reference_root: pathlib.Path | None = None,
    source_root: pathlib.Path | None = None,
    signature_file: pathlib.Path | None = None,
    public_key_file: pathlib.Path | None = None,
) -> dict[str, Any]:
    path = path.resolve()
    if path.is_symlink() or not path.is_file():
        raise ReleaseError(f"archive not found or unsafe: {path}", EXIT_VERIFY)
    if expected_sha256 and sha256_file(path) != expected_sha256:
        raise ReleaseError("archive checksum mismatch", EXIT_VERIFY)
    with tarfile.open(path, "r:*") as archive:
        root_name, members = safe_archive_members(archive)
        with tempfile.TemporaryDirectory(prefix="emuwiz-release-verify-") as temporary:
            destination = pathlib.Path(temporary)
            archive.extractall(destination, members=members, filter="data")
            extracted = destination / root_name
            result = verify_directory(extracted, strict=strict, quiet=True, source_root=source_root)
            if signature_file is not None and public_key_file is not None:
                try:
                    valid_signature = verify_detached_gpg(extracted / "SHA256SUMS", signature_file, public_key_file)
                except SigningError as error:
                    raise ReleaseError(str(error), EXIT_VERIFY) from error
                if not valid_signature:
                    print("INVALID SIGNATURE")
                    raise ReleaseError("INVALID SIGNATURE", EXIT_VERIFY)
            for relative in result["manifest"]["files"]:
                candidate = extracted / relative["path"]
                if sha256_file(candidate) != relative["sha256"]:
                    raise ReleaseError(f"extracted archive differs: {relative['path']}", EXIT_VERIFY)
            if reference_root is not None:
                reference_root = reference_root.resolve()
                extracted_files = {
                    item.relative_to(extracted).as_posix(): sha256_file(item)
                    for item in payload_files(extracted)
                }
                reference_files = {
                    item.relative_to(reference_root).as_posix(): sha256_file(item)
                    for item in payload_files(reference_root)
                }
                if extracted_files != reference_files:
                    raise ReleaseError("extracted archive does not exactly match packaged directory", EXIT_VERIFY)
    print(f"verified archive {path.name} (one safe top-level directory)")
    return result


def package(args: argparse.Namespace) -> pathlib.Path:
    source_root = pathlib.Path(args.source_root).expanduser().resolve()
    output_root = safe_output_root(pathlib.Path(args.output_root))
    epoch, timestamp = timestamp_from_environment(args.reproducible)
    log(1, "Inspecting source")
    version = project_version(source_root)
    provenance = source_provenance(source_root)
    if args.sbom_dir:
        sbom_source = pathlib.Path(args.sbom_dir).expanduser()
        sbom_info = validate_sbom_input(sbom_source, source_root, provenance["commit"])
    elif args.require_sbom:
        raise ReleaseError("--require-sbom requires --sbom-dir", EXIT_INPUT)
    else:
        sbom_source = None
        sbom_info = None
    packaging_sha = tool_commit("scripts/release/package_release.py")
    if not provenance["clean"]:
        print("WARNING: source worktree is dirty; provenance records clean=false", file=sys.stderr)
        if args.require_clean:
            raise ReleaseError("--require-clean refused dirty source state", EXIT_PROVENANCE)
    arch = normalise_arch(args.arch or platform.machine())
    release_platform = args.platform or f"linux-{arch}"
    if not re.fullmatch(r"[A-Za-z0-9._+-]+", release_platform):
        raise ReleaseError(f"invalid release platform: {release_platform}", EXIT_INPUT)
    release_name = f"emuwiz-{version}-{release_platform}"
    final = output_root / release_name
    archive_path = output_root / f"{release_name}.tar.xz"
    signature_path = output_root / f"{release_name}.SHA256SUMS.asc"
    public_key_path = pathlib.Path(args.public_key_output).expanduser() if args.public_key_output else None
    signing_identity: dict[str, Any] | None = None
    if args.sign:
        if not args.signing_key:
            raise ReleaseError("--sign requires --signing-key", EXIT_INPUT)
        try:
            signing_identity = inspect_signing_key(pathlib.Path(args.signing_key).expanduser())
        except SigningError as error:
            raise ReleaseError(str(error), EXIT_INPUT) from error
    elif args.signing_key or args.public_key_output:
        raise ReleaseError("--signing-key and --public-key-output require --sign", EXIT_INPUT)
    output_root.mkdir(parents=True, exist_ok=True)
    replacing_owned_package = False
    if final.exists():
        if not args.overwrite:
            raise ReleaseError(f"output already exists: {final}; use --overwrite for an owned package", EXIT_UNSAFE)
        replacing_owned_package = owned_release_directory(final, release_name)
        if not replacing_owned_package:
            raise ReleaseError(
                f"refusing to replace unowned output directory (missing valid {MARKER_NAME}): {final}",
                EXIT_UNSAFE,
            )
    if archive_path.exists():
        if not args.overwrite:
            raise ReleaseError(f"archive already exists: {archive_path}", EXIT_UNSAFE)
        if not replacing_owned_package:
            raise ReleaseError("refusing to replace archive without an associated owned package", EXIT_UNSAFE)
        if not (output_root / f"{release_name}.tar.xz.sha256").is_file():
            raise ReleaseError("refusing to replace archive without its checksum sidecar", EXIT_UNSAFE)
    if signature_path.exists():
        if not args.overwrite:
            raise ReleaseError(f"signature sidecar already exists: {signature_path}", EXIT_UNSAFE)
        if signature_path.is_symlink() or not signature_path.is_file():
            raise ReleaseError(f"refusing to replace unsafe signature sidecar: {signature_path}", EXIT_UNSAFE)
        signature_path.unlink()
    if public_key_path is not None and public_key_path.exists():
        raise ReleaseError(f"refusing to replace existing public-key output: {public_key_path}", EXIT_UNSAFE)
    if replacing_owned_package:
        remove_owned_release(final, release_name)
    if archive_path.exists():
        archive_path.unlink()
        (output_root / f"{release_name}.tar.xz.sha256").unlink()

    log(2, "Validating binaries")
    binaries = discover_binaries(args)
    inspected: dict[str, dict[str, Any]] = {}
    for kind, path in binaries.items():
        inspected[kind] = validate_binary(path, arch, args.allow_symlink, not args.fixture_mode)
    gui_identity = validate_gui_identity(inspected["gui"]["source_path"], args.fixture_mode)
    appimages: list[tuple[pathlib.Path, dict[str, Any]]] = []
    for value in args.appimage:
        path = pathlib.Path(value).expanduser()
        appimages.append((path, validate_binary(path, arch, args.allow_symlink, not args.fixture_mode)))
    for kind, info in inspected.items():
        if info["unexpected_absolute_rpath"]:
            print(f"warning: {kind} has absolute RPATH/RUNPATH entries", file=sys.stderr)

    staging = pathlib.Path(tempfile.mkdtemp(prefix=f".{release_name}.incomplete-", dir=output_root))
    release = staging / release_name
    try:
        log(3, "Creating release tree")
        (release / "bin").mkdir(parents=True)
        marker = {"owner": "emuwiz-release-packager", "release_directory": release_name, "schema_version": 1}
        atomic_write(release / MARKER_NAME, json_bytes(marker))
        artifact_records: list[dict[str, Any]] = []
        for kind, destination_name in (("gui", "emuwiz"), ("cli", "emuwiz-cli")):
            info = inspected[kind]
            destination = release / "bin" / destination_name
            shutil.copyfile(info["source_path"], destination)
            os.chmod(destination, 0o755)
            artifact_records.append({
                "path": f"bin/{destination_name}",
                "kind": kind,
                "size": destination.stat().st_size,
                "sha256": sha256_file(destination),
                "elf_arch": info["elf_arch"],
                "elf_class": info["elf_class"],
                "dynamic_dependencies": info["needed"],
                "rpath_runpath": info["rpath"],
            })
        for position, (source, info) in enumerate(appimages, start=1):
            safe_name = source.name
            if safe_name in {"emuwiz", "emuwiz-cli"} or not re.fullmatch(r"[A-Za-z0-9._+-]+", safe_name):
                safe_name = f"emuwiz-{position}.AppImage"
            destination = release / "bin" / safe_name
            if destination.exists():
                raise ReleaseError(f"duplicate AppImage output filename: {safe_name}", EXIT_INPUT)
            shutil.copyfile(info["source_path"], destination)
            os.chmod(destination, 0o755)
            artifact_records.append({
                "path": f"bin/{safe_name}",
                "kind": "appimage",
                "filename": safe_name,
                "channel": args.appimage_channel,
                "version": args.appimage_version,
                "size": destination.stat().st_size,
                "sha256": sha256_file(destination),
                "elf_arch": info["elf_arch"],
                "elf_class": info["elf_class"],
                "dynamic_dependencies": info["needed"],
                "rpath_runpath": info["rpath"],
            })
        write_generated_docs(release, version, release_platform)
        copied_licenses = copy_license_payload(source_root, release / "docs")
        if sbom_source is not None and sbom_info is not None:
            sbom_records = copy_sbom_payload(sbom_source, release / "SBOM")
        else:
            sbom_records = []

        log(4, "Generating provenance")
        rustc_version = run_readonly(["rustc", "--version"])
        cargo_version = run_readonly(["cargo", "--version"])
        build_info = [
            "EmuWiz release build provenance",
            f"version: {version}",
            f"source_commit: {provenance['commit']}",
            f"source_branch: {provenance['branch']}",
            f"source_clean: {str(provenance['clean']).lower()}",
            f"packaging_timestamp: {timestamp}",
            f"build_profile: {args.build_profile or ('release' if args.target_dir else 'unknown')}",
            f"platform: {release_platform}",
            f"host_architecture: {normalise_arch(platform.machine())}",
            f"linux_kernel: {platform.release()}",
            f"rustc: {rustc_version or 'unavailable'}",
            f"cargo: {cargo_version or 'unavailable'}",
            f"packager_version: {PACKAGER_VERSION}",
            f"packaging_tool_sha: {packaging_sha}",
            f"gui_target: {gui_identity['target']}",
            f"gui_entrypoint: {gui_identity['entrypoint']}",
            f"gui_generation: {gui_identity['generation']}",
            f"gui_version_probe: {gui_identity['version_probe']}",
            "dynamic_dependency_note: names are informational, not proof of target availability",
            "",
        ]
        if signing_identity is not None:
            build_info.extend([
                "Signing:",
                "  Enabled: yes",
                f"  Algorithm: {signing_identity['algorithm']}",
                f"  Key fingerprint: {signing_identity['key_fingerprint']}",
                "  Signature: detached, outside reproducible archive",
                f"  Signature file: {signature_path.name}",
                f"  Public key file: {public_key_path.name if public_key_path else 'published separately'}",
                "",
            ])
        if sbom_info is not None:
            summary = sbom_info["summary"]
            counts = summary.get("counts", {})
            build_info.extend([
                "SBOM:",
                "  Included: yes",
                "  Format: CycloneDX 1.5",
                f"  Bundle schema: {SBOM_BUNDLE_SCHEMA}",
                f"  Cargo.lock SHA-256: {sbom_info['cargo_lock_sha256']}",
                f"  Packages: {counts.get('unique_packages', 'unknown')}",
                f"  Licence metadata unresolved: {len(summary.get('missing_or_ambiguous_license_metadata', []))}",
                f"  SBOM tool SHA: {sbom_info['sbom_tool_sha']}",
                "",
            ])
        for record in artifact_records:
            build_info.extend([
                f"artifact: {record['path']}",
                f"  size: {record['size']}",
                f"  sha256: {record['sha256']}",
                f"  elf_arch: {record['elf_arch'] or 'not-elf'}",
                f"  needed: {', '.join(record['dynamic_dependencies']) or 'none detected'}",
                f"  rpath_runpath: {', '.join(record['rpath_runpath']) or 'none detected'}",
            ])
        atomic_write(release / "BUILD_INFO.txt", ("\n".join(build_info) + "\n").encode())

        log(5, "Calculating checksums")
        pre_manifest = [file_record(release, path) for path in payload_files(release)]
        manifest = {
            "schema_version": SCHEMA_VERSION,
            "packager": {"name": "emuwiz-release-packager", "version": PACKAGER_VERSION},
            "release": {
                "name": "EmuWiz",
                "version": version,
                "commit": provenance["commit"],
                "branch": provenance["branch"],
                "source_clean": provenance["clean"],
                "source_kind": "git-worktree",
                "platform": release_platform,
                "architecture": arch,
                "build_profile": args.build_profile or ("release" if args.target_dir else "unknown"),
                "packaging_timestamp": timestamp,
                "source_date_epoch": epoch if os.environ.get("SOURCE_DATE_EPOCH") is not None else None,
                "reproducible_mode": args.reproducible,
            },
            "host": {
                "architecture": normalise_arch(platform.machine()),
                "linux_kernel": platform.release(),
                "rustc": rustc_version,
                "cargo": cargo_version,
            },
            "artifacts": sorted(artifact_records, key=lambda item: item["path"]),
            "gui": {
                **gui_identity,
                "elf_sha256": next(
                    item["sha256"] for item in artifact_records if item["path"] == "bin/emuwiz"
                ),
            },
            "files": sorted(pre_manifest, key=lambda item: item["path"]),
            "licenses": copied_licenses,
            "provenance": {
                "product_source_sha": provenance["commit"],
                "packaging_tool_sha": packaging_sha,
                "sbom_tool_sha": sbom_info["sbom_tool_sha"] if sbom_info else None,
            },
            "checksum_scope": "all files except SHA256SUMS; manifest.json is represented only in SHA256SUMS to avoid a self-hash",
            "variable_fields_when_not_reproducible": ["release.packaging_timestamp"],
        }
        if signing_identity is not None:
            manifest["signing"] = {
                **signing_identity,
                "signed_file": "SHA256SUMS",
                "signature_file": signature_path.name,
                "public_key_file": public_key_path.name if public_key_path else None,
            }
        if sbom_info is not None:
            summary = sbom_info["summary"]
            manifest["sbom"] = {
                "present": True,
                "format": "CycloneDX",
                "spec_version": "1.5",
                "bundle_schema": SBOM_BUNDLE_SCHEMA,
                "cargo_lock_sha256": sbom_info["cargo_lock_sha256"],
                "product_source_sha": sbom_info["product_source_sha"],
                "sbom_tool_sha": sbom_info["sbom_tool_sha"],
                "packaging_tool_sha": packaging_sha,
                "package_count": summary.get("counts", {}).get("unique_packages"),
                "files": [
                    {
                        **file_record(release, release / record["path"]),
                        "kind": record["kind"],
                    }
                    for record in sbom_records
                ],
            }
        atomic_write(release / "manifest.json", json_bytes(manifest))
        checksum_records = [(record["path"], record["sha256"]) for record in manifest["files"]]
        checksum_records.append(("manifest.json", sha256_file(release / "manifest.json")))
        checksum_text = "".join(f"{digest}  {relative}\n" for relative, digest in sorted(checksum_records))
        atomic_write(release / "SHA256SUMS", checksum_text.encode())
        if signing_identity is not None:
            try:
                sign_detached_gpg(
                    release / "SHA256SUMS",
                    pathlib.Path(args.signing_key).expanduser(),
                    signature_path,
                    public_key_path,
                )
            except SigningError as error:
                raise ReleaseError(str(error), EXIT_TOOL) from error

        log(6, "Scanning payload")
        scan_secrets(release)
        verify_directory(release, strict=True, quiet=True)
        os.replace(release, final)
        staging.rmdir()

        if args.archive:
            log(7, "Creating archive")
            create_archive(final, archive_path, epoch)
            archive_hash = sha256_file(archive_path)
            atomic_write(
                output_root / f"{release_name}.tar.xz.sha256",
                f"{archive_hash}  {archive_path.name}\n".encode(),
            )
            log(8, "Verifying extracted archive")
            verify_archive(
                archive_path,
                strict=True,
                expected_sha256=archive_hash,
                reference_root=final,
                source_root=source_root,
            )
        else:
            log(7, "Archive creation not requested")
            log(8, "Verifying release directory")
            verify_directory(final, strict=True, quiet=True, source_root=source_root)
        log(9, "Complete")
        print(final)
        if args.archive:
            print(archive_path)
        return final
    except Exception:
        if release.exists():
            try:
                atomic_write(release / "INCOMPLETE", b"packaging did not complete\n")
            except OSError:
                pass
        if archive_path.exists():
            archive_path.unlink()
        if signature_path.is_file() and not signature_path.is_symlink():
            signature_path.unlink()
        if public_key_path is not None and public_key_path.is_file() and not public_key_path.is_symlink():
            public_key_path.unlink()
        raise


def verify_command(args: argparse.Namespace) -> None:
    target = pathlib.Path(args.target).expanduser()
    if target.is_dir():
        if args.checksum:
            raise ReleaseError("--checksum applies only to an archive", EXIT_INPUT)
        verify_directory(
            target,
            args.strict,
            source_root=pathlib.Path(args.source_root).expanduser().resolve() if args.source_root else None,
        )
        if args.verify_signature:
            signature = pathlib.Path(args.signature).expanduser() if args.signature else target.parent / f"{target.name}.SHA256SUMS.asc"
            if not signature.exists():
                print("SIGNATURE NOT PROVIDED")
                return
            if not args.public_key:
                raise ReleaseError("--public-key is required to verify a provided signature", EXIT_INPUT)
            try:
                valid = verify_detached_gpg(target / "SHA256SUMS", signature, pathlib.Path(args.public_key).expanduser())
            except SigningError as error:
                raise ReleaseError(str(error), EXIT_VERIFY) from error
            if not valid:
                print("INVALID SIGNATURE")
                raise ReleaseError("INVALID SIGNATURE", EXIT_VERIFY)
            print("VALID SIGNATURE")
    else:
        expected = args.archive_sha256
        if args.checksum:
            checksum_path = pathlib.Path(args.checksum).expanduser()
            try:
                line = checksum_path.read_text(encoding="utf-8").strip()
            except OSError as error:
                raise ReleaseError(f"archive checksum file is unreadable: {checksum_path}", EXIT_INPUT) from error
            match = re.fullmatch(r"([0-9a-f]{64})  ([^\r\n]+)", line)
            if not match or match.group(2) != target.name:
                raise ReleaseError("archive checksum file is invalid or names a different archive", EXIT_INPUT)
            expected = match.group(1)
        signature = None
        public_key = None
        if args.verify_signature:
            signature = pathlib.Path(args.signature).expanduser() if args.signature else target.parent / f"{target.name.removesuffix('.tar.xz')}.SHA256SUMS.asc"
            if not signature.exists():
                print("SIGNATURE NOT PROVIDED")
                signature = None
            elif not args.public_key:
                raise ReleaseError("--public-key is required to verify a provided signature", EXIT_INPUT)
            else:
                public_key = pathlib.Path(args.public_key).expanduser()
        verify_archive(
            target,
            args.strict,
            expected,
            source_root=pathlib.Path(args.source_root).expanduser().resolve() if args.source_root else None,
            signature_file=signature,
            public_key_file=public_key,
        )
        if signature is not None:
            print("VALID SIGNATURE")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    subcommands = result.add_subparsers(dest="command", required=True)
    package_parser = subcommands.add_parser("package", help="package already-built binaries")
    package_parser.add_argument("--gui")
    package_parser.add_argument("--cli")
    package_parser.add_argument("--target-dir")
    package_parser.add_argument("--appimage", action="append", default=[])
    package_parser.add_argument("--appimage-channel", default="unspecified")
    package_parser.add_argument("--appimage-version", default="unknown")
    package_parser.add_argument("--output-root", default=str(repository_root() / "dist"))
    package_parser.add_argument("--source-root", default=str(repository_root()))
    package_parser.add_argument("--arch")
    package_parser.add_argument("--platform")
    package_parser.add_argument("--build-profile")
    package_parser.add_argument("--require-clean", action="store_true")
    package_parser.add_argument("--sbom-dir", help="verified SBOM bundle to include")
    package_parser.add_argument("--require-sbom", action="store_true")
    package_parser.add_argument("--reproducible", action="store_true")
    package_parser.add_argument("--archive", action="store_true")
    package_parser.add_argument("--overwrite", action="store_true")
    package_parser.add_argument("--sign", action="store_true", help="create a detached GPG signature for SHA256SUMS")
    package_parser.add_argument("--signing-key", help="private signing key file; imported only into a temporary keyring")
    package_parser.add_argument("--public-key-output", help="optional separate output path for the armored public key")
    package_parser.add_argument("--allow-symlink", action="store_true")
    package_parser.add_argument("--fixture-mode", action="store_true", help=argparse.SUPPRESS)
    package_parser.set_defaults(function=package)
    verify_parser = subcommands.add_parser("verify", help="verify a release directory or archive")
    verify_parser.add_argument("--strict", action="store_true")
    verify_parser.add_argument("--archive-sha256")
    verify_parser.add_argument("--checksum", help="one-record archive SHA-256 sidecar")
    verify_parser.add_argument("--source-root", help="optional source tree for full SBOM re-verification")
    verify_parser.add_argument("--verify-signature", action="store_true")
    verify_parser.add_argument("--public-key", help="public key used to verify the detached signature")
    verify_parser.add_argument("--signature", help="detached signature path; defaults to the release sidecar")
    verify_parser.add_argument("target")
    verify_parser.set_defaults(function=verify_command)
    return result


def main(argv: list[str] | None = None) -> int:
    try:
        args = parser().parse_args(argv)
        args.function(args)
        return 0
    except ReleaseError as error:
        print(f"release-packager: error: {error}", file=sys.stderr)
        return error.code
    except SigningError as error:
        print(f"release-packager: signing error: {error}", file=sys.stderr)
        return EXIT_TOOL
    except (OSError, tarfile.TarError) as error:
        print(f"release-packager: tool failure: {error}", file=sys.stderr)
        return EXIT_TOOL


if __name__ == "__main__":
    raise SystemExit(main())
