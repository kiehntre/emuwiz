#!/usr/bin/env python3
"""Generate and verify an offline EmuWiz Rust SBOM and licence bundle."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.parse
from collections import Counter, defaultdict
from dataclasses import dataclass
from typing import Any, Iterable


GENERATOR_VERSION = "1.0.0"
CYCLONEDX_SPEC_VERSION = "1.5"
CYCLONEDX_SCHEMA = "http://cyclonedx.org/schema/bom-1.5.schema.json"
BUNDLE_SCHEMA_VERSION = 1
OUTPUT_FILES = (
    "emuwiz-sbom.cdx.json",
    "third-party-licenses.json",
    "THIRD_PARTY_LICENSES.txt",
    "dependency-summary.json",
)
CHECKSUM_FILE = "SBOM_SHA256SUMS"
CRATES_IO_SOURCES = {
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
}
LICENCE_PREFIXES = ("license", "licence", "copying", "notice", "copyright")
MAX_LICENCE_BYTES = 4 * 1024 * 1024


class SbomError(Exception):
    pass


@dataclass(frozen=True, order=True)
class PackageKey:
    name: str
    version: str
    source: str | None

    @property
    def package_id(self) -> str:
        suffix = f" ({self.source})" if self.source else ""
        return f"{self.name} {self.version}{suffix}"

    @property
    def bom_ref(self) -> str:
        source = self.source or "workspace"
        return f"cargo:{self.name}@{self.version}|{source}"


@dataclass
class LocalMaterial:
    metadata: dict[str, Any]
    texts: list[tuple[str, bytes]]
    available: bool


def json_bytes(value: Any) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode()


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def atomic_write(path: pathlib.Path, value: bytes) -> None:
    temporary = path.with_name(f".{path.name}.incomplete")
    with temporary.open("wb") as output:
        output.write(value)
        output.flush()
        os.fsync(output.fileno())
    os.chmod(temporary, 0o644)
    os.replace(temporary, path)


def run(args: list[str], cwd: pathlib.Path) -> subprocess.CompletedProcess[str] | None:
    if shutil.which(args[0]) is None:
        return None
    try:
        return subprocess.run(
            args,
            cwd=cwd,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None


def repository_root() -> pathlib.Path:
    return pathlib.Path(__file__).resolve().parents[2]


def git_identity(source_root: pathlib.Path) -> tuple[str, bool]:
    head = run(["git", "rev-parse", "HEAD"], source_root)
    status = run(["git", "status", "--porcelain=v1", "--untracked-files=normal"], source_root)
    if head is None or head.returncode != 0 or status is None or status.returncode != 0:
        raise SbomError("source root is not a readable Git worktree")
    return head.stdout.strip(), not bool(status.stdout.strip())


def timestamp() -> tuple[int, str]:
    raw = os.environ.get("SOURCE_DATE_EPOCH")
    if raw is None:
        epoch = int(dt.datetime.now(tz=dt.timezone.utc).timestamp())
    else:
        try:
            epoch = int(raw)
        except ValueError as error:
            raise SbomError("SOURCE_DATE_EPOCH must be a non-negative integer") from error
        if epoch < 0:
            raise SbomError("SOURCE_DATE_EPOCH must be a non-negative integer")
    rendered = dt.datetime.fromtimestamp(epoch, tz=dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    return epoch, rendered


def load_lock(source_root: pathlib.Path) -> tuple[list[dict[str, Any]], str]:
    lock_path = source_root / "Cargo.lock"
    try:
        raw = lock_path.read_bytes()
        parsed = tomllib.loads(raw.decode("utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        raise SbomError(f"Cargo.lock is unreadable: {error}") from error
    packages = parsed.get("package")
    if not isinstance(packages, list):
        raise SbomError("Cargo.lock has no package inventory")
    return packages, sha256_bytes(raw)


def workspace_metadata(source_root: pathlib.Path, metadata_file: str | None = None) -> tuple[dict[str, Any], str]:
    if metadata_file:
        try:
            return json.loads(pathlib.Path(metadata_file).read_text(encoding="utf-8")), "supplied-metadata-fixture"
        except (OSError, ValueError) as error:
            raise SbomError(f"supplied Cargo metadata is unreadable: {error}") from error
    full = run(["cargo", "metadata", "--format-version", "1", "--locked", "--offline"], source_root)
    if full is not None and full.returncode == 0:
        return json.loads(full.stdout), "cargo-metadata-offline"
    shallow = run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked", "--offline"],
        source_root,
    )
    if shallow is not None and shallow.returncode == 0:
        return json.loads(shallow.stdout), "cargo-metadata-no-deps-lockfile-graph"
    return manifest_workspace_metadata(source_root), "workspace-manifest-lockfile-graph"


def manifest_workspace_metadata(source_root: pathlib.Path) -> dict[str, Any]:
    root = tomllib.loads((source_root / "Cargo.toml").read_text(encoding="utf-8"))
    workspace = root.get("workspace", {})
    inherited = workspace.get("package", {})
    packages: list[dict[str, Any]] = []
    for member in workspace.get("members", []):
        manifest_path = source_root / member / "Cargo.toml"
        parsed = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        package = parsed["package"]
        version = package.get("version")
        if isinstance(version, dict) and version.get("workspace") is True:
            version = inherited.get("version")
        packages.append(
            {
                "name": package["name"],
                "version": version,
                "id": f"path+workspace#{package['name']}@{version}",
                "manifest_path": str(manifest_path),
                "license": package.get("license"),
                "license_file": package.get("license-file"),
                "repository": package.get("repository"),
                "homepage": package.get("homepage"),
            }
        )
    return {"packages": packages, "workspace_members": [item["id"] for item in packages], "resolve": None}


def project_version(source_root: pathlib.Path) -> str:
    root = tomllib.loads((source_root / "Cargo.toml").read_text(encoding="utf-8"))
    value = root.get("workspace", {}).get("package", {}).get("version")
    if not isinstance(value, str):
        raise SbomError("workspace.package.version is missing")
    return value


def package_key(package: dict[str, Any]) -> PackageKey:
    return PackageKey(str(package["name"]), str(package["version"]), package.get("source"))


def parse_dependency(value: str) -> tuple[str, str | None, str | None]:
    source = None
    body = value
    if body.endswith(")") and " (" in body:
        body, source = body.rsplit(" (", 1)
        source = source[:-1]
    fields = body.split()
    if not fields:
        raise SbomError(f"empty Cargo.lock dependency record: {value!r}")
    return fields[0], fields[1] if len(fields) > 1 else None, source


def resolve_graph(packages: list[dict[str, Any]]) -> tuple[dict[PackageKey, list[PackageKey]], list[str]]:
    keys = [package_key(package) for package in packages]
    by_name: dict[str, list[PackageKey]] = defaultdict(list)
    for key in keys:
        by_name[key.name].append(key)
    graph: dict[PackageKey, list[PackageKey]] = {}
    errors: list[str] = []
    for package, owner in zip(packages, keys, strict=True):
        resolved: list[PackageKey] = []
        for raw in package.get("dependencies", []):
            if not isinstance(raw, str):
                errors.append(f"{owner.package_id}: unsupported dependency record {raw!r}")
                continue
            name, version, source = parse_dependency(raw)
            candidates = list(by_name.get(name, []))
            if version is not None:
                candidates = [candidate for candidate in candidates if candidate.version == version]
            if source is not None:
                candidates = [candidate for candidate in candidates if candidate.source == source]
            if len(candidates) != 1:
                errors.append(f"{owner.package_id}: cannot uniquely resolve {raw!r}")
                continue
            resolved.append(candidates[0])
        graph[owner] = sorted(set(resolved))
    return graph, sorted(errors)


def source_type(key: PackageKey, workspace: set[PackageKey]) -> str:
    if key in workspace:
        return "workspace"
    if key.source is None:
        return "path-outside-workspace"
    if key.source in CRATES_IO_SOURCES:
        return "crates.io-registry"
    if key.source.startswith("git+"):
        return "git"
    if key.source.startswith("registry+") or key.source.startswith("sparse+"):
        return "other-registry"
    return "unknown"


def is_licence_name(name: str) -> bool:
    lowered = name.lower()
    return any(lowered.startswith(prefix) for prefix in LICENCE_PREFIXES)


def bounded_text(name: str, value: bytes) -> tuple[str, bytes] | None:
    if len(value) == 0 or len(value) > MAX_LICENCE_BYTES or b"\x00" in value:
        return None
    try:
        value.decode("utf-8")
    except UnicodeDecodeError:
        return None
    return name, value


def material_from_directory(directory: pathlib.Path) -> LocalMaterial:
    manifest_path = directory / "Cargo.toml"
    if not manifest_path.is_file():
        return LocalMaterial({}, [], False)
    try:
        package = tomllib.loads(manifest_path.read_text(encoding="utf-8")).get("package", {})
    except (OSError, UnicodeError, tomllib.TOMLDecodeError):
        return LocalMaterial({}, [], False)
    names: set[pathlib.Path] = set()
    declared_file = package.get("license-file")
    if isinstance(declared_file, str):
        names.add(pathlib.Path(declared_file))
    for child in directory.iterdir():
        if child.is_file() and not child.is_symlink() and is_licence_name(child.name):
            names.add(pathlib.Path(child.name))
    texts: list[tuple[str, bytes]] = []
    for relative in sorted(names, key=lambda item: item.as_posix()):
        candidate = directory / relative
        try:
            if candidate.is_file() and not candidate.is_symlink():
                accepted = bounded_text(relative.as_posix(), candidate.read_bytes())
                if accepted:
                    texts.append(accepted)
        except OSError:
            continue
    return LocalMaterial(dict(package), texts, True)


def material_from_archive(archive_path: pathlib.Path) -> LocalMaterial:
    try:
        with tarfile.open(archive_path, "r:gz") as archive:
            members = [member for member in archive.getmembers() if member.isfile() and not member.issym()]
            manifest_member = next(
                (member for member in members if pathlib.PurePosixPath(member.name).name == "Cargo.toml" and len(pathlib.PurePosixPath(member.name).parts) == 2),
                None,
            )
            if manifest_member is None:
                return LocalMaterial({}, [], False)
            extracted = archive.extractfile(manifest_member)
            if extracted is None:
                return LocalMaterial({}, [], False)
            package = tomllib.loads(extracted.read().decode("utf-8")).get("package", {})
            declared = package.get("license-file")
            selected: list[tarfile.TarInfo] = []
            for member in members:
                pure = pathlib.PurePosixPath(member.name)
                relative = pathlib.PurePosixPath(*pure.parts[1:])
                if (len(relative.parts) == 1 and is_licence_name(relative.name)) or (
                    isinstance(declared, str) and relative.as_posix() == declared
                ):
                    selected.append(member)
            texts: list[tuple[str, bytes]] = []
            for member in sorted(selected, key=lambda item: item.name):
                if member.size > MAX_LICENCE_BYTES:
                    continue
                extracted = archive.extractfile(member)
                if extracted is None:
                    continue
                pure = pathlib.PurePosixPath(member.name)
                accepted = bounded_text(pathlib.PurePosixPath(*pure.parts[1:]).as_posix(), extracted.read())
                if accepted:
                    texts.append(accepted)
            return LocalMaterial(dict(package), texts, True)
    except (OSError, tarfile.TarError, UnicodeError, tomllib.TOMLDecodeError):
        return LocalMaterial({}, [], False)


class MaterialFinder:
    def __init__(self, cargo_home: pathlib.Path):
        self.source_roots = sorted((cargo_home / "registry" / "src").glob("*"))
        self.cache_roots = sorted((cargo_home / "registry" / "cache").glob("*"))

    def find(self, key: PackageKey) -> LocalMaterial:
        leaf = f"{key.name}-{key.version}"
        for root in self.source_roots:
            candidate = root / leaf
            if candidate.is_dir():
                return material_from_directory(candidate)
        for root in self.cache_roots:
            candidate = root / f"{leaf}.crate"
            if candidate.is_file():
                return material_from_archive(candidate)
        return LocalMaterial({}, [], False)


TOKEN = re.compile(r"\s*(\(|\)|AND\b|OR\b|WITH\b|[A-Za-z0-9][A-Za-z0-9.+-]*)")


def valid_spdx_expression(value: str) -> bool:
    position = 0
    tokens: list[str] = []
    while position < len(value):
        match = TOKEN.match(value, position)
        if not match:
            return False
        tokens.append(match.group(1))
        position = match.end()
    if not tokens:
        return False
    index = 0

    def atom() -> bool:
        nonlocal index
        if index >= len(tokens):
            return False
        if tokens[index] == "(":
            index += 1
            if not expression() or index >= len(tokens) or tokens[index] != ")":
                return False
            index += 1
            return True
        if tokens[index] in {"AND", "OR", "WITH", ")"}:
            return False
        index += 1
        if index < len(tokens) and tokens[index] == "WITH":
            index += 1
            if index >= len(tokens) or tokens[index] in {"AND", "OR", "WITH", "(", ")"}:
                return False
            index += 1
        return True

    def expression() -> bool:
        nonlocal index
        if not atom():
            return False
        while index < len(tokens) and tokens[index] in {"AND", "OR"}:
            index += 1
            if not atom():
                return False
        return True

    return expression() and index == len(tokens)


def licence_classification(material: LocalMaterial) -> tuple[str, str | None, str | None]:
    expression = material.metadata.get("license")
    licence_file = material.metadata.get("license-file")
    if isinstance(expression, str) and expression.strip():
        expression = expression.strip()
        if not valid_spdx_expression(expression):
            return "AMBIGUOUS", expression, licence_file if isinstance(licence_file, str) else None
        classification = "MULTIPLE_DECLARED" if re.search(r"\b(?:AND|OR)\b", expression) else "DECLARED"
        return classification, expression, licence_file if isinstance(licence_file, str) else None
    if isinstance(licence_file, str) and licence_file.strip():
        return "DECLARED", None, licence_file
    if not material.available:
        return "UNAVAILABLE_LOCALLY", None, None
    return "MISSING_DECLARATION", None, None


def canonical_inventory(packages: list[dict[str, Any]]) -> bytes:
    records = [
        {
            "package_id": item["package_id"],
            "license_classification": item["license_classification"],
            "license_expression": item["license_expression"],
            "license_text_hashes": item["license_text_hashes"],
        }
        for item in packages
    ]
    return json_bytes(sorted(records, key=lambda item: item["package_id"]))


def generate(args: argparse.Namespace) -> pathlib.Path:
    started = dt.datetime.now(tz=dt.timezone.utc)
    source_root = pathlib.Path(args.source_root).expanduser().resolve()
    output = pathlib.Path(args.output_dir).expanduser().resolve()
    cargo_home = pathlib.Path(args.cargo_home).expanduser().resolve()
    packages_raw, lock_hash = load_lock(source_root)
    metadata, metadata_mode = workspace_metadata(source_root, args.metadata_file)
    source_sha, source_clean = git_identity(source_root)
    epoch, generated_at = timestamp()
    version = project_version(source_root)
    graph, graph_errors = resolve_graph(packages_raw)
    package_by_key = {package_key(package): package for package in packages_raw}
    workspace_member_ids = set(metadata.get("workspace_members", []))
    workspace_names_versions = {
        (str(package["name"]), str(package["version"]))
        for package in metadata.get("packages", [])
        if package.get("id") in workspace_member_ids
    }
    workspace = {
        key for key in package_by_key
        if key.source is None and (key.name, key.version) in workspace_names_versions
    }
    direct = {dependency for owner in workspace for dependency in graph.get(owner, []) if dependency not in workspace}
    finder = MaterialFinder(cargo_home)
    metadata_by_name_version: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for item in metadata.get("packages", []):
        metadata_by_name_version[(str(item["name"]), str(item["version"]))].append(item)
    text_records: dict[str, dict[str, Any]] = {}
    licence_packages: list[dict[str, Any]] = []
    components: list[dict[str, Any]] = []
    warnings = list(graph_errors)
    registry_missing_checksums: list[str] = []
    unusual_sources: list[str] = []

    for key in sorted(package_by_key):
        locked = package_by_key[key]
        kind = source_type(key, workspace)
        metadata_candidates = metadata_by_name_version.get((key.name, key.version), [])
        matching_metadata = next(
            (
                item for item in metadata_candidates
                if item.get("source") == key.source
                and ((kind == "workspace") == (item.get("id") in workspace_member_ids))
            ),
            None,
        )
        if kind == "workspace":
            workspace_item = matching_metadata or {}
            manifest_path = pathlib.Path(workspace_item.get("manifest_path", source_root / "Cargo.toml"))
            material = material_from_directory(manifest_path.parent)
        elif kind == "crates.io-registry" or kind == "other-registry":
            material = finder.find(key)
        elif matching_metadata and matching_metadata.get("manifest_path"):
            material = material_from_directory(pathlib.Path(matching_metadata["manifest_path"]).parent)
        else:
            material = LocalMaterial({}, [], False)
        if matching_metadata:
            for field in ("license", "repository", "homepage"):
                if material.metadata.get(field) is None and matching_metadata.get(field) is not None:
                    material.metadata[field] = matching_metadata[field]
        classification, expression, declared_file = licence_classification(material)
        text_hashes: list[str] = []
        third_party_texts = material.texts if key not in workspace else []
        for filename, value in third_party_texts:
            digest = sha256_bytes(value)
            record = text_records.setdefault(
                digest,
                {"sha256": digest, "filenames": set(), "packages": set(), "text": value.decode("utf-8")},
            )
            record["filenames"].add(filename)
            record["packages"].add(key.package_id)
            text_hashes.append(digest)
        text_hashes = sorted(set(text_hashes))
        text_status = "AVAILABLE" if text_hashes else ("SOURCE_UNAVAILABLE" if not material.available else "NOT_FOUND")
        repository = material.metadata.get("repository")
        homepage = material.metadata.get("homepage")
        checksum = locked.get("checksum")
        if kind in {"crates.io-registry", "other-registry"} and not checksum:
            registry_missing_checksums.append(key.package_id)
        if kind in {"git", "other-registry", "path-outside-workspace", "unknown"}:
            unusual_sources.append(f"{key.package_id}: {kind}")
        classification_scope = "workspace" if key in workspace else ("direct" if key in direct else "transitive")
        licence_entry = {
            "name": key.name,
            "version": key.version,
            "package_id": key.package_id,
            "source": key.source,
            "source_type": kind,
            "dependency_classification": classification_scope,
            "license_classification": classification,
            "license_expression": expression,
            "declared_license_file": declared_file,
            "license_text_status": text_status,
            "license_text_hashes": text_hashes,
            "repository": repository if isinstance(repository, str) else None,
            "homepage": homepage if isinstance(homepage, str) else None,
        }
        if key not in workspace:
            licence_packages.append(licence_entry)
        component: dict[str, Any] = {
            "type": "library",
            "bom-ref": key.bom_ref,
            "name": key.name,
            "version": key.version,
            "purl": f"pkg:cargo/{urllib.parse.quote(key.name)}@{urllib.parse.quote(key.version)}",
            "properties": [
                {"name": "emuwiz:cargo:package_id", "value": key.package_id},
                {"name": "emuwiz:cargo:source", "value": key.source or "workspace/path"},
                {"name": "emuwiz:cargo:source_type", "value": kind},
                {"name": "emuwiz:dependency_classification", "value": classification_scope},
                {"name": "emuwiz:license_classification", "value": classification},
                {"name": "emuwiz:license_text_status", "value": text_status},
            ],
        }
        if checksum:
            component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        if expression:
            component["licenses"] = [{"expression": expression}]
        elif declared_file:
            component["licenses"] = [{"license": {"name": f"License file: {declared_file}"}}]
        references = []
        if isinstance(repository, str) and repository:
            references.append({"type": "vcs", "url": repository})
        if isinstance(homepage, str) and homepage:
            references.append({"type": "website", "url": homepage})
        if references:
            component["externalReferences"] = references
        components.append(component)

    licence_packages.sort(key=lambda item: item["package_id"])
    inventory_hash = sha256_bytes(canonical_inventory(licence_packages))
    serialised_texts = [
        {
            "sha256": digest,
            "filenames": sorted(record["filenames"]),
            "packages": sorted(record["packages"]),
            "text": record["text"],
        }
        for digest, record in sorted(text_records.items())
    ]
    counts = Counter(item["license_classification"] for item in licence_packages)
    expressions = Counter(
        item["license_expression"] or ("License file" if item["declared_license_file"] else item["license_classification"])
        for item in licence_packages
    )
    source_counts = Counter(source_type(key, workspace) for key in package_by_key)
    versions_by_name: dict[str, set[str]] = defaultdict(set)
    for key in package_by_key:
        versions_by_name[key.name].add(key.version)
    duplicate_versions = [
        {"name": name, "versions": sorted(versions)}
        for name, versions in sorted(versions_by_name.items()) if len(versions) > 1
    ]
    missing_or_ambiguous = [
        item["package_id"] for item in licence_packages
        if item["license_classification"] in {"MISSING_DECLARATION", "AMBIGUOUS", "UNAVAILABLE_LOCALLY"}
    ]
    if registry_missing_checksums:
        warnings.append(f"registry packages missing checksums: {len(registry_missing_checksums)}")
    if missing_or_ambiguous:
        warnings.append(f"third-party packages with unresolved licence metadata: {len(missing_or_ambiguous)}")
    warnings.extend(unusual_sources)

    sbom = {
        "$schema": CYCLONEDX_SCHEMA,
        "bomFormat": "CycloneDX",
        "specVersion": CYCLONEDX_SPEC_VERSION,
        "version": 1,
        "metadata": {
            "timestamp": generated_at,
            "tools": {"components": [{"type": "application", "name": "emuwiz-sbom-generator", "version": GENERATOR_VERSION}]},
            "component": {
                "type": "application",
                "name": "EmuWiz",
                "version": version,
                "properties": [
                    {"name": "emuwiz:source_commit", "value": source_sha},
                    {"name": "emuwiz:cargo_lock_sha256", "value": lock_hash},
                    {"name": "emuwiz:metadata_mode", "value": metadata_mode},
                ],
            },
        },
        "components": sorted(components, key=lambda item: item["bom-ref"]),
        "dependencies": [
            {"ref": key.bom_ref, "dependsOn": [dependency.bom_ref for dependency in graph.get(key, [])]}
            for key in sorted(package_by_key)
        ],
    }
    licence_json = {
        "schema_version": BUNDLE_SCHEMA_VERSION,
        "generator_version": GENERATOR_VERSION,
        "cargo_lock_sha256": lock_hash,
        "source_commit": source_sha,
        "third_party_inventory_sha256": inventory_hash,
        "packages": licence_packages,
        "license_texts": serialised_texts,
    }
    summary = {
        "schema_version": BUNDLE_SCHEMA_VERSION,
        "generator": {"name": "emuwiz-sbom-generator", "version": GENERATOR_VERSION},
        "cyclonedx_spec_version": CYCLONEDX_SPEC_VERSION,
        "workspace_version": version,
        "source_commit": source_sha,
        "source_clean": source_clean,
        "cargo_lock_sha256": lock_hash,
        "source_date_epoch": epoch if os.environ.get("SOURCE_DATE_EPOCH") is not None else None,
        "generated_at": generated_at,
        "metadata_mode": metadata_mode,
        "counts": {
            "unique_packages": len(package_by_key),
            "workspace_packages": len(workspace),
            "direct_third_party_dependencies": len(direct),
            "transitive_dependencies": len(package_by_key) - len(workspace) - len(direct),
            "duplicate_version_crates": len(duplicate_versions),
            "license_classifications": dict(sorted(counts.items())),
            "license_expressions": dict(sorted(expressions.items())),
            "source_types": dict(sorted(source_counts.items())),
            "deduplicated_license_texts": len(serialised_texts),
        },
        "duplicate_versions": duplicate_versions,
        "git_dependencies": sorted(key.package_id for key in package_by_key if source_type(key, workspace) == "git"),
        "non_crates_io_sources": sorted(unusual_sources),
        "registry_missing_checksums": sorted(registry_missing_checksums),
        "missing_or_ambiguous_license_metadata": sorted(missing_or_ambiguous),
        "graph_complete": not graph_errors and len(graph) == len(package_by_key),
        "graph_errors": graph_errors,
        "third_party_inventory_sha256": inventory_hash,
        "warnings": sorted(set(warnings)),
    }
    notice_lines = [
        "EmuWiz third-party licence inventory",
        f"Source commit: {source_sha}",
        f"Cargo.lock SHA-256: {lock_hash}",
        f"Dependency inventory SHA-256: {inventory_hash}",
        "",
        "Licence texts are reproduced verbatim from locally available Cargo package sources.",
        "Identical texts are listed once with all associated packages.",
        "",
    ]
    for record in serialised_texts:
        notice_lines.extend(
            [
                "=" * 78,
                f"Licence text SHA-256: {record['sha256']}",
                f"Observed filenames: {', '.join(record['filenames'])}",
                "Packages:",
                *(f"- {package_id}" for package_id in record["packages"]),
                "",
                record["text"].rstrip("\n"),
                "",
            ]
        )
    unresolved = [item for item in licence_packages if item["license_classification"] in {"MISSING_DECLARATION", "AMBIGUOUS", "UNAVAILABLE_LOCALLY"}]
    notice_lines.extend(["=" * 78, "Missing, ambiguous, or locally unavailable licence metadata", ""])
    if unresolved:
        for item in unresolved:
            notice_lines.append(
                f"- {item['package_id']} | {item['license_classification']} | "
                f"declared={item['license_expression'] or item['declared_license_file'] or 'none'} | "
                f"text={item['license_text_status']}"
            )
    else:
        notice_lines.append("None.")
    notice_lines.append("")

    strict_failures = list(graph_errors)
    strict_failures.extend(f"unresolved licence metadata: {item}" for item in missing_or_ambiguous)
    strict_failures.extend(f"registry checksum missing: {item}" for item in registry_missing_checksums)
    if args.strict and strict_failures:
        raise SbomError("strict generation failed:\n- " + "\n- ".join(strict_failures))

    if output.exists() and any(output.iterdir()):
        if not args.overwrite:
            raise SbomError(f"output directory is not empty: {output}")
        summary_path = output / "dependency-summary.json"
        try:
            existing = json.loads(summary_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            raise SbomError("refusing to replace an output not owned by this generator") from error
        if existing.get("generator", {}).get("name") != "emuwiz-sbom-generator":
            raise SbomError("refusing to replace an output not owned by this generator")
        shutil.rmtree(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = pathlib.Path(tempfile.mkdtemp(prefix=f".{output.name}.incomplete-", dir=output.parent))
    try:
        atomic_write(staging / OUTPUT_FILES[0], json_bytes(sbom))
        atomic_write(staging / OUTPUT_FILES[1], json_bytes(licence_json))
        atomic_write(staging / OUTPUT_FILES[2], ("\n".join(notice_lines)).encode())
        elapsed = (dt.datetime.now(tz=dt.timezone.utc) - started).total_seconds()
        summary["generation_runtime_seconds"] = round(elapsed, 3)
        # Runtime is deliberately omitted from reproducible outputs. It is printed only.
        runtime = summary.pop("generation_runtime_seconds")
        atomic_write(staging / OUTPUT_FILES[3], json_bytes(summary))
        checksums = "".join(f"{sha256_file(staging / name)}  {name}\n" for name in sorted(OUTPUT_FILES))
        atomic_write(staging / CHECKSUM_FILE, checksums.encode())
        os.replace(staging, output)
    except Exception:
        try:
            atomic_write(staging / "INCOMPLETE", b"SBOM generation did not complete\n")
        except OSError:
            pass
        raise
    for warning in summary["warnings"]:
        print(f"warning: {warning}", file=sys.stderr)
    print(f"generated {len(package_by_key)} packages in {runtime:.3f}s at {output}")
    return output


def checksum_records(path: pathlib.Path) -> dict[str, str]:
    records: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9._-]+)", line)
        if not match or match.group(2) in records:
            raise SbomError("SBOM_SHA256SUMS contains an invalid or duplicate record")
        records[match.group(2)] = match.group(1)
    return records


def verify(args: argparse.Namespace) -> pathlib.Path:
    source_root = pathlib.Path(args.source_root).expanduser().resolve()
    bundle = pathlib.Path(args.sbom_dir).expanduser().resolve()
    required = set(OUTPUT_FILES) | {CHECKSUM_FILE}
    if not bundle.is_dir():
        raise SbomError(f"SBOM directory does not exist: {bundle}")
    actual = {path.name for path in bundle.iterdir() if path.is_file() and not path.is_symlink()}
    if actual != required:
        raise SbomError(f"SBOM file set mismatch; expected {sorted(required)}, found {sorted(actual)}")
    records = checksum_records(bundle / CHECKSUM_FILE)
    if set(records) != set(OUTPUT_FILES):
        raise SbomError("SBOM_SHA256SUMS does not cover the exact generated file set")
    for name, expected in records.items():
        if sha256_file(bundle / name) != expected:
            raise SbomError(f"generated checksum mismatch: {name}")
    try:
        sbom = json.loads((bundle / OUTPUT_FILES[0]).read_text(encoding="utf-8"))
        licences = json.loads((bundle / OUTPUT_FILES[1]).read_text(encoding="utf-8"))
        summary = json.loads((bundle / OUTPUT_FILES[3]).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise SbomError(f"generated JSON is unreadable: {error}") from error
    if sbom.get("bomFormat") != "CycloneDX" or sbom.get("specVersion") != CYCLONEDX_SPEC_VERSION:
        raise SbomError("unsupported CycloneDX schema")
    if licences.get("schema_version") != BUNDLE_SCHEMA_VERSION or summary.get("schema_version") != BUNDLE_SCHEMA_VERSION:
        raise SbomError("unsupported EmuWiz SBOM bundle schema")
    packages_raw, lock_hash = load_lock(source_root)
    if summary.get("cargo_lock_sha256") != lock_hash or licences.get("cargo_lock_sha256") != lock_hash:
        raise SbomError("Cargo.lock SHA-256 differs from the recorded inventory")
    source_sha, _ = git_identity(source_root)
    if summary.get("source_commit") != source_sha or licences.get("source_commit") != source_sha:
        raise SbomError("source repository SHA differs from the recorded inventory")
    metadata_properties = {
        item.get("name"): item.get("value")
        for item in sbom.get("metadata", {}).get("component", {}).get("properties", [])
    }
    if metadata_properties.get("emuwiz:source_commit") != source_sha:
        raise SbomError("CycloneDX source repository SHA differs from the current repository")
    if metadata_properties.get("emuwiz:cargo_lock_sha256") != lock_hash:
        raise SbomError("CycloneDX Cargo.lock identity differs from the current lockfile")
    components = sbom.get("components")
    dependencies = sbom.get("dependencies")
    if not isinstance(components, list) or not isinstance(dependencies, list):
        raise SbomError("CycloneDX component graph is missing")
    if len(components) != len(packages_raw) or len(dependencies) != len(packages_raw):
        raise SbomError("CycloneDX package graph is incomplete")
    refs = {item.get("bom-ref") for item in components}
    if None in refs or len(refs) != len(components):
        raise SbomError("CycloneDX component references are invalid")
    for relation in dependencies:
        if relation.get("ref") not in refs or any(item not in refs for item in relation.get("dependsOn", [])):
            raise SbomError("CycloneDX dependency relationship references an unknown package")
    locked = {package_key(item).package_id: item for item in packages_raw}
    component_ids: set[str] = set()
    component_refs_by_id: dict[str, str] = {}
    observed_classifications: Counter[str] = Counter()
    observed_source_types: Counter[str] = Counter()
    for component in components:
        properties = {item["name"]: item["value"] for item in component.get("properties", [])}
        package_id = properties.get("emuwiz:cargo:package_id")
        if package_id not in locked:
            raise SbomError(f"SBOM package is not present in Cargo.lock: {package_id}")
        component_ids.add(package_id)
        component_refs_by_id[package_id] = component["bom-ref"]
        observed_classifications[properties.get("emuwiz:dependency_classification", "missing")] += 1
        observed_source_types[properties.get("emuwiz:cargo:source_type", "missing")] += 1
        expected_checksum = locked[package_id].get("checksum")
        observed = {item.get("content") for item in component.get("hashes", []) if item.get("alg") == "SHA-256"}
        if expected_checksum and expected_checksum not in observed:
            raise SbomError(f"registry checksum differs from Cargo.lock: {package_id}")
    if component_ids != set(locked):
        raise SbomError("CycloneDX components do not exactly match Cargo.lock packages")
    expected_graph, graph_errors = resolve_graph(packages_raw)
    if graph_errors:
        raise SbomError("current Cargo.lock graph cannot be resolved completely")
    expected_relationships = {
        component_refs_by_id[key.package_id]: sorted(component_refs_by_id[item.package_id] for item in values)
        for key, values in expected_graph.items()
    }
    observed_relationships = {
        item["ref"]: sorted(item.get("dependsOn", [])) for item in dependencies
    }
    if observed_relationships != expected_relationships:
        raise SbomError("CycloneDX dependency relationships differ from Cargo.lock")
    counts = summary.get("counts", {})
    if counts.get("unique_packages") != len(packages_raw):
        raise SbomError("dependency summary package count is inconsistent")
    if counts.get("workspace_packages") != observed_classifications["workspace"]:
        raise SbomError("dependency summary workspace count is inconsistent")
    if counts.get("direct_third_party_dependencies") != observed_classifications["direct"]:
        raise SbomError("dependency summary direct count is inconsistent")
    if counts.get("transitive_dependencies") != observed_classifications["transitive"]:
        raise SbomError("dependency summary transitive count is inconsistent")
    if counts.get("source_types") != dict(sorted(observed_source_types.items())):
        raise SbomError("dependency summary source counts are inconsistent")
    licence_packages = licences.get("packages")
    if not isinstance(licence_packages, list):
        raise SbomError("third-party licence package inventory is missing")
    inventory_hash = sha256_bytes(canonical_inventory(licence_packages))
    if inventory_hash != licences.get("third_party_inventory_sha256") or inventory_hash != summary.get("third_party_inventory_sha256"):
        raise SbomError("third-party licence inventory identity mismatch")
    notice = (bundle / OUTPUT_FILES[2]).read_text(encoding="utf-8")
    if f"Dependency inventory SHA-256: {inventory_hash}" not in notice:
        raise SbomError("THIRD_PARTY_LICENSES.txt does not correspond to dependency inventory")
    component_package_ids: set[str] = set()
    for component in components:
        properties = {item.get("name"): item.get("value") for item in component.get("properties", [])}
        if properties.get("emuwiz:dependency_classification") != "workspace":
            package_id = properties.get("emuwiz:cargo:package_id")
            if not isinstance(package_id, str):
                raise SbomError("third-party component has no Cargo package identity")
            component_package_ids.add(package_id)
    if {item.get("package_id") for item in licence_packages} != component_package_ids:
        raise SbomError("third-party licence inventory does not cover the exact third-party component set")
    classification_counts = Counter(item.get("license_classification") for item in licence_packages)
    if counts.get("license_classifications") != dict(sorted(classification_counts.items())):
        raise SbomError("dependency summary licence counts are inconsistent")
    text_records = licences.get("license_texts")
    if not isinstance(text_records, list):
        raise SbomError("licence text inventory is missing")
    known_texts: dict[str, set[str]] = {}
    for record in text_records:
        digest = record.get("sha256")
        text = record.get("text")
        packages_for_text = record.get("packages")
        if not isinstance(digest, str) or not isinstance(text, str) or not isinstance(packages_for_text, list):
            raise SbomError("licence text record is malformed")
        if sha256_bytes(text.encode("utf-8")) != digest:
            raise SbomError(f"licence text digest mismatch: {digest}")
        package_set = set(packages_for_text)
        if not package_set <= component_package_ids:
            raise SbomError(f"licence text references an unknown third-party package: {digest}")
        known_texts[digest] = package_set
        if f"Licence text SHA-256: {digest}" not in notice:
            raise SbomError(f"THIRD_PARTY_LICENSES.txt omits licence text {digest}")
    reverse_texts: dict[str, set[str]] = defaultdict(set)
    for package in licence_packages:
        package_id = package.get("package_id")
        for digest in package.get("license_text_hashes", []):
            if digest not in known_texts:
                raise SbomError(f"package references an unknown licence text: {package_id}")
            reverse_texts[digest].add(package_id)
    if reverse_texts != known_texts:
        raise SbomError("licence text package cross-references are inconsistent")
    unresolved = [
        item["package_id"] for item in licence_packages
        if item.get("license_classification") in {"MISSING_DECLARATION", "AMBIGUOUS", "UNAVAILABLE_LOCALLY"}
    ]
    if args.strict and (unresolved or not summary.get("graph_complete") or summary.get("registry_missing_checksums")):
        raise SbomError("strict verification failed because unresolved metadata or an incomplete graph remains")
    print(f"verified {len(components)} Cargo packages in {bundle}")
    return bundle


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    generate_parser = subcommands.add_parser("generate")
    generate_parser.add_argument("--source-root", default=str(repository_root()))
    generate_parser.add_argument("--cargo-home", default=os.environ.get("CARGO_HOME", str(pathlib.Path.home() / ".cargo")))
    generate_parser.add_argument("--metadata-file", help=argparse.SUPPRESS)
    generate_parser.add_argument("--output-dir", required=True)
    generate_parser.add_argument("--strict", action="store_true")
    generate_parser.add_argument("--overwrite", action="store_true")
    generate_parser.set_defaults(function=generate)
    verify_parser = subcommands.add_parser("verify")
    verify_parser.add_argument("--source-root", default=str(repository_root()))
    verify_parser.add_argument("--strict", action="store_true")
    verify_parser.add_argument("sbom_dir")
    verify_parser.set_defaults(function=verify)
    return parser


def main(argv: list[str] | None = None) -> int:
    try:
        args = build_parser().parse_args(argv)
        args.function(args)
        return 0
    except SbomError as error:
        print(f"sbom: error: {error}", file=sys.stderr)
        return 2
    except (OSError, ValueError, tarfile.TarError) as error:
        print(f"sbom: tool failure: {error}", file=sys.stderr)
        return 3


if __name__ == "__main__":
    raise SystemExit(main())
