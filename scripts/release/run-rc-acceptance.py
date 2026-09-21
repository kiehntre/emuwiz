#!/usr/bin/env python3
"""One-command, isolated release-candidate acceptance gate.

This file is intentionally an orchestrator.  Release correctness remains in
the existing packager, SBOM, smoke, synthetic-library, recovery, and preflight
tools; this command only composes their evidence and applies release-gate
policy.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path
from typing import Any

STAGES = [
    "Source provenance", "Binary validation", "Fresh SBOM generation",
    "SBOM verification", "Release packaging", "Release verification",
    "Archive extraction verification", "CLI release smoke", "Synthetic library",
    "Synthetic scan smoke", "SQLite integrity", "Pending recovery check",
    "Upgrade fixture checks", "Reproducibility comparison", "Evidence bundle",
]


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def inside(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(root.resolve(strict=False))
        return True
    except ValueError:
        return False


class Gate:
    def __init__(self, args: argparse.Namespace) -> None:
        self.source = Path(args.source_tree).resolve()
        self.gui = Path(args.gui_binary).resolve()
        self.cli = Path(args.cli_binary).resolve()
        self.output = Path(args.output).resolve()
        self.timeout = args.timeout
        self.keep = args.keep
        self.evidence = self.output / "rc-evidence"
        self.logs = self.evidence / "logs"
        self.work = self.evidence / "work"
        self.results: list[dict[str, Any]] = []
        self.warnings: list[str] = []
        self.failures: list[str] = []
        self.started = time.monotonic()
        self.env = self._isolated_env()
        self.env["SOURCE_DATE_EPOCH"] = "0"

    def _isolated_env(self) -> dict[str, str]:
        root = self.work / "user"
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LANG": "C",
               "LC_ALL": "C", "TZ": "UTC", "TERM": "dumb",
               "RUST_BACKTRACE": "1", "HOME": str(root / "home"),
               "XDG_CONFIG_HOME": str(root / "config"),
               "XDG_DATA_HOME": str(root / "data"),
               "XDG_CACHE_HOME": str(root / "cache"),
               "XDG_STATE_HOME": str(root / "state"),
               "EMUWIZ_CONFIG_HOME": str(root / "config" / "emuwiz"),
               "EMUWIZ_DATA_HOME": str(root / "data" / "emuwiz"),}
        # Cargo metadata needs the already-installed registry index.  This is
        # a read-only build cache, not an EmuWiz user-state root; no Cargo
        # command in the gate builds or updates it.
        env["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))
        env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
        env["CARGO_NET_OFFLINE"] = "true"
        return env

    def setup(self) -> None:
        if not self.source.is_dir() or not (self.source / ".git").exists():
            raise RuntimeError("--source-tree must be a git worktree")
        self.env["SOURCE_DATE_EPOCH"] = subprocess.check_output(
            ["git", "-C", str(self.source), "log", "-1", "--format=%ct"], text=True
        ).strip()
        for label, binary in (("GUI", self.gui), ("CLI", self.cli)):
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise RuntimeError(f"{label} binary is missing or not executable: {binary}")
        if self.output in (Path("/"), Path("/tmp"), Path("/home")):
            raise RuntimeError("refusing unsafe output root")
        for forbidden in (Path.home() / ".config", Path.home() / ".local/share",
                          Path.home() / ".cache", Path("/mnt/games")):
            if inside(self.output, forbidden) or inside(self.work, forbidden):
                raise RuntimeError(f"output would overlap real user state: {forbidden}")
        if inside(self.output, self.source):
            raise RuntimeError("output must not be inside the source tree")
        self.logs.mkdir(parents=True, exist_ok=True)
        self.work.mkdir(parents=True, exist_ok=True)
        for key in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"):
            Path(self.env[key]).mkdir(parents=True, exist_ok=True)
        (self.evidence / "smoke").mkdir(exist_ok=True)
        (self.evidence / "synthetic").mkdir(exist_ok=True)
        (self.evidence / "upgrade").mkdir(exist_ok=True)
        (self.evidence / "recovery").mkdir(exist_ok=True)
        (self.evidence / "sbom").mkdir(exist_ok=True)
        (self.evidence / "package").mkdir(exist_ok=True)
        (self.evidence / "checksums").mkdir(exist_ok=True)
        (self.evidence / "environment.json").write_text(json.dumps(self.env, indent=2) + "\n")

    def command(self, name: str, argv: list[str], *, cwd: Path | None = None,
                env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        out = self.logs / f"{len(self.results)+1:02d}-{name}.stdout.log"
        err = self.logs / f"{len(self.results)+1:02d}-{name}.stderr.log"
        merged = dict(self.env)
        if env:
            merged.update(env)
        start = time.monotonic()
        try:
            result = subprocess.run(argv, cwd=cwd or self.source, env=merged,
                                    text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=self.timeout, check=False)
            out.write_text(result.stdout)
            err.write_text(result.stderr)
            return result
        except subprocess.TimeoutExpired as exc:
            out.write_text((exc.stdout or "") if isinstance(exc.stdout, str) else "")
            err.write_text(f"timeout after {self.timeout}s\n")
            return subprocess.CompletedProcess(argv, 124, "", "timeout")
        finally:
            (self.logs / f"{len(self.results)+1:02d}-{name}.timing").write_text(
                f"{time.monotonic()-start:.3f}s\n")

    def stage(self, title: str, fn) -> None:
        number = len(self.results) + 1
        start = time.monotonic()
        status, detail = "PASS", ""
        try:
            value = fn()
            if isinstance(value, str):
                detail = value
        except Exception as exc:  # stage failures are evidence, not tracebacks
            status, detail = "FAIL", str(exc)
            self.failures.append(f"{title}: {detail}")
        self.results.append({"number": number, "stage": title, "status": status,
                             "detail": detail, "seconds": round(time.monotonic()-start, 3)})
        print(f"[{number:02d}/15] {title:<34} {status}")

    def git(self, *args: str) -> str:
        result = self.command("git-" + args[0], ["git", *args])
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or "git command failed")
        return result.stdout.strip()

    def provenance(self) -> str:
        data = {"source_sha": self.git("rev-parse", "HEAD"),
                "source_dirty": bool(self.git("status", "--porcelain")),
                "cargo_lock_sha256": sha256(self.source / "Cargo.lock")}
        (self.evidence / "provenance.json").write_text(json.dumps(data, indent=2) + "\n")
        if data["source_dirty"]:
            self.warnings.append("source tree is dirty")
        return data["source_sha"]

    def binaries(self) -> str:
        data = {"gui": {"path": str(self.gui), "sha256": sha256(self.gui)},
                "cli": {"path": str(self.cli), "sha256": sha256(self.cli)}}
        (self.evidence / "binary-hashes.json").write_text(json.dumps(data, indent=2) + "\n")
        for name, path in (("gui", self.gui), ("cli", self.cli)):
            result = self.command(f"{name}-version", [str(path), "--version"])
            if result.returncode:
                raise RuntimeError(f"{name} --version failed")
        return "binary hashes and --version checks recorded"

    def sbom(self) -> str:
        sbom = self.evidence / "sbom" / "SBOM"
        result = self.command("sbom-generate", ["bash", "scripts/release/generate-sbom.sh",
            "--source-root", str(self.source), "--output-dir", str(sbom), "--overwrite"])
        if result.returncode:
            raise RuntimeError("SBOM generation failed")
        unresolved = re.search(r"unresolved licence metadata: (\d+)", result.stderr)
        if unresolved:
            self.warnings.append(f"SBOM contains {unresolved.group(1)} unresolved licence entries")
        self.sbom_dir = sbom
        return str(sbom)

    def verify_sbom(self) -> str:
        sbom = getattr(self, "sbom_dir", self.evidence / "sbom" / "SBOM")
        result = self.command("sbom-verify", ["bash", "scripts/release/verify-sbom.sh", str(sbom)])
        if result.returncode:
            raise RuntimeError("SBOM verification failed")
        return "SBOM bundle verified"

    def package(self) -> tuple[Path, Path]:
        root = self.evidence / "package" / "run1"
        sbom = self.evidence / "sbom" / "SBOM"
        result = self.command("package", ["bash", "scripts/release/package-release.sh", "--gui", str(self.gui),
            "--cli", str(self.cli), "--source-root", str(self.source), "--output-root", str(root),
            "--sbom-dir", str(sbom), "--require-sbom", "--reproducible", "--archive"])
        if result.returncode:
            raise RuntimeError("release packaging failed")
        archives = sorted(root.glob("*.tar.xz"))
        if not archives:
            raise RuntimeError("packager produced no archive")
        return root, archives[0]

    def verify_package(self, package: tuple[Path, Path]) -> str:
        root, archive = package
        result = self.command("package-verify", ["bash", "scripts/release/verify-release.sh", "--strict",
            "--checksum", str(archive) + ".sha256", "--source-root", str(self.source), str(archive)])
        if result.returncode:
            raise RuntimeError("release verification failed")
        with tarfile.open(archive, "r:*") as tf:
            names = tf.getnames()
            if any(Path(n).is_absolute() or ".." in Path(n).parts for n in names):
                raise RuntimeError("unsafe archive path")
        return f"archive_sha256={sha256(archive)}"

    def smoke(self) -> str:
        smoke = self.evidence / "smoke"
        env = {"EMUWIZ_BINARY": str(self.cli), "KEEP_SMOKE_STATE": "1",
               "SMOKE_TIMEOUT": str(self.timeout),
               # Cargo build artifacts are not EmuWiz user state.  Ignore
               # concurrent compiler churn while retaining checks for config,
               # data, cache, and state roots.
               "SMOKE_EXTERNAL_IGNORE_PREFIX": str(Path.home() / ".cache/emuwiz-cargo-target")}
        result = self.command("release-smoke", ["bash", "scripts/qa/release-smoke.sh"], env=env)
        (smoke / "result.txt").write_text(result.stdout + result.stderr)
        retained = re.search(r"Smoke root retained at: (\S+)", result.stderr)
        if retained:
            self.smoke_db = Path(retained.group(1)) / "data" / "library.sqlite3"
        if result.returncode:
            raise RuntimeError("release smoke failed")
        return "isolated CLI smoke passed"

    def synthetic(self) -> Path:
        root = self.evidence / "synthetic" / "library"
        result = self.command("synthetic-build", ["bash", "scripts/qa/build-synthetic-library.sh",
            "--profile", "standard", "--output", str(root), "--recreate", "--verify"])
        if result.returncode:
            raise RuntimeError("synthetic library generation failed")
        result = self.command("synthetic-validate", ["bash", "scripts/qa/validate-synthetic-library.sh", str(root)])
        if result.returncode:
            raise RuntimeError("synthetic library validation failed")
        return root

    def synthetic_scan(self, root: Path) -> str:
        result = self.command("synthetic-scan", ["bash", "scripts/qa/run-synthetic-scan.sh", str(root)],
                              env={"EMUWIZ_CLI": str(self.cli)})
        if result.returncode:
            raise RuntimeError("synthetic scan failed")
        (self.evidence / "synthetic" / "scan.json").write_text(result.stdout)
        return "synthetic scan evidence retained"

    def sqlite_check(self) -> str:
        db = getattr(self, "smoke_db", None)
        if not db or not db.is_file():
            raise RuntimeError("isolated smoke did not produce a database")
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        try:
            check = con.execute("PRAGMA quick_check").fetchone()[0]
            schema = con.execute("PRAGMA user_version").fetchone()[0]
        finally:
            con.close()
        if check != "ok":
            raise RuntimeError(f"quick_check={check}")
        (self.evidence / "smoke" / "sqlite.json").write_text(json.dumps(
            {"path": str(db), "schema": schema, "quick_check": check}, indent=2) + "\n")
        return f"schema={schema}, quick_check=ok"

    def recovery(self) -> str:
        root = self.evidence / "recovery" / "fixtures"
        (root / "rename-transactions").mkdir(parents=True, exist_ok=True)
        (root / "rename-transactions" / "clean.json").write_text('{"transaction_id":"clean","state":"applied","entries":[]}\n')
        (root / "rename-transactions" / "pending.json").write_text('{"transaction_id":"pending","state":"applying","entries":[]}\n')
        result = self.command("recovery", ["python3", "tools/pending_recovery/inspector.py",
            "--data-root", str(root), "--config-root", str(root), "--json",
            str(self.evidence / "recovery" / "report.json")])
        if result.returncode not in (0, 1, 2):
            raise RuntimeError("recovery inspector failed")
        return "clean and review-required fixtures inspected"

    def upgrades(self) -> str:
        root = self.evidence / "upgrade" / "fixtures"
        for version in (19, 20, 21):
            data = root / f"schema-{version}" / "data"
            config = root / f"schema-{version}" / "config"
            data.mkdir(parents=True, exist_ok=True); config.mkdir(parents=True, exist_ok=True)
            db = sqlite3.connect(data / "library.sqlite3")
            db.execute(f"PRAGMA user_version={version}"); db.commit(); db.close()
            source = root / f"schema-{version}" / "source"
            source.mkdir(parents=True, exist_ok=True)
            (config / "config.toml").write_text(f"[[source]]\npath='{source}'\n")
            result = self.command(f"upgrade-{version}", ["python3", "scripts/qa/upgrade_preflight.py",
                "--config-root", str(config), "--data-root", str(data), "--json",
                str(root / f"schema-{version}.json")])
            expected = 2 if version == 21 else (1 if version == 19 else 0)
            if result.returncode != expected:
                raise RuntimeError(f"schema {version} returned {result.returncode}, expected {expected}")
        missing = root / "missing-mount"
        (missing / "config").mkdir(parents=True, exist_ok=True)
        (missing / "data").mkdir(parents=True, exist_ok=True)
        db = sqlite3.connect(missing / "data" / "library.sqlite3")
        db.execute("PRAGMA user_version=20"); db.commit(); db.close()
        (missing / "config" / "config.toml").write_text("[[source]]\npath='/mnt/emuwiz-rc-no-mount'\n")
        result = self.command("upgrade-missing-mount", ["python3", "scripts/qa/upgrade_preflight.py",
            "--config-root", str(missing / "config"), "--data-root", str(missing / "data"),
            "--json", str(missing / "report.json")])
        if result.returncode != 2:
            raise RuntimeError("missing mount fixture was not blocked")
        for label in ("legacy-only", "both-roots"):
            fixture = root / label
            config = fixture / "config"; data = fixture / "data"
            legacy_config = fixture / "legacy-config"; legacy_data = fixture / "legacy-data"
            for path in (config, data, legacy_config, legacy_data):
                path.mkdir(parents=True, exist_ok=True)
            (data / "library.sqlite3").write_bytes(b"not-a-real-db")
            (legacy_data / "library.sqlite3").write_bytes(b"not-a-real-db")
            result = self.command(f"upgrade-{label}", ["python3", "scripts/qa/upgrade_preflight.py",
                "--config-root", str(config), "--data-root", str(data),
                "--legacy-config-root", str(legacy_config), "--legacy-data-root", str(legacy_data),
                "--json", str(fixture / "report.json")])
            if result.returncode not in (2, 3):
                raise RuntimeError(f"{label} fixture was unexpectedly accepted")
        return "schema 19/20/21, legacy, conflict, and missing-mount fixtures passed"

    def repro(self, package: tuple[Path, Path]) -> str:
        sbom = self.evidence / "sbom" / "SBOM"
        for n in (2,):
            root = self.evidence / "package" / f"run{n}"
            result = self.command(f"package-{n}", ["bash", "scripts/release/package-release.sh", "--gui", str(self.gui),
                "--cli", str(self.cli), "--source-root", str(self.source), "--output-root", str(root),
                "--sbom-dir", str(sbom), "--require-sbom", "--reproducible", "--archive"])
            if result.returncode:
                raise RuntimeError("second reproducibility package failed")
        first = next(package[0].glob("*.tar.xz")); second = next((self.evidence / "package" / "run2").glob("*.tar.xz"))
        if first.read_bytes() != second.read_bytes():
            raise RuntimeError("archives are not byte-identical")
        (self.evidence / "checksums" / "archive.sha256").write_text(f"{sha256(first)}  {first.name}\n")
        return f"archive_sha256={sha256(first)}"

    def write_final(self) -> None:
        sbom_summary_path = self.evidence / "sbom" / "SBOM" / "dependency-summary.json"
        sbom_summary = json.loads(sbom_summary_path.read_text()) if sbom_summary_path.is_file() else {}
        sbom_components = self.evidence / "sbom" / "SBOM" / "emuwiz-sbom.cdx.json"
        component_count = len(json.loads(sbom_components.read_text()).get("components", [])) if sbom_components.is_file() else None
        archive_hash = None
        checksum_files = list((self.evidence / "checksums").glob("*.sha256"))
        if checksum_files:
            archive_hash = checksum_files[0].read_text().split()[0]
        summary = {"source_sha": json.loads((self.evidence / "provenance.json").read_text())["source_sha"],
                   "binary_hashes": json.loads((self.evidence / "binary-hashes.json").read_text()),
                   "cargo_lock_sha256": json.loads((self.evidence / "provenance.json").read_text())["cargo_lock_sha256"],
                   "sbom_package_count": component_count,
                   "licence_unresolved_count": len(sbom_summary.get("missing_or_ambiguous_license_metadata", [])),
                   "archive_sha256": archive_hash,
                   "stages": self.results, "timings_seconds": round(time.monotonic()-self.started, 3),
                   "warnings": self.warnings, "failures": self.failures,
                   "result": "FAIL" if self.failures else ("PASS_WITH_WARNINGS" if self.warnings else "PASS")}
        (self.evidence / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        lines = ["# EmuWiz Release Candidate Acceptance", "", f"Result: **{summary['result']}**", ""]
        lines += [f"- {r['stage']}: {r['status']} ({r['seconds']}s)" for r in self.results]
        if self.warnings: lines += ["", "Warnings:", *[f"- {x}" for x in self.warnings]]
        if self.failures: lines += ["", "Failures:", *[f"- {x}" for x in self.failures]]
        (self.evidence / "SUMMARY.md").write_text("\n".join(lines) + "\n")

    def run(self) -> int:
        self.setup()
        package: tuple[Path, Path] | None = None
        self.stage("Source provenance", self.provenance)
        self.stage("Binary validation", self.binaries)
        self.stage("Fresh SBOM generation", self.sbom)
        self.stage("SBOM verification", self.verify_sbom)
        def package_stage():
            nonlocal package
            package = self.package(); return str(package[1])
        self.stage("Release packaging", package_stage)
        self.stage("Release verification", lambda: self.verify_package(package) if package else (_ for _ in ()).throw(RuntimeError("package unavailable")))
        self.stage("Archive extraction verification", lambda: self.verify_package(package) if package else (_ for _ in ()).throw(RuntimeError("package unavailable")))
        self.stage("CLI release smoke", self.smoke)
        synthetic_root: Path | None = None
        def synthetic_stage():
            nonlocal synthetic_root
            synthetic_root = self.synthetic(); return str(synthetic_root)
        self.stage("Synthetic library", synthetic_stage)
        self.stage("Synthetic scan smoke", lambda: self.synthetic_scan(synthetic_root) if synthetic_root else (_ for _ in ()).throw(RuntimeError("synthetic library unavailable")))
        self.stage("SQLite integrity", self.sqlite_check)
        self.stage("Pending recovery check", self.recovery)
        self.stage("Upgrade fixture checks", self.upgrades)
        self.stage("Reproducibility comparison", lambda: self.repro(package) if package else (_ for _ in ()).throw(RuntimeError("package unavailable")))
        self.stage("Evidence bundle", self.write_final)
        # The stage callback runs before its own result is appended; refresh
        # once so summary.json/SUMMARY.md include all fifteen stage records.
        self.write_final()
        if self.failures:
            print("RELEASE CANDIDATE ACCEPTANCE: FAIL")
            return 1
        print("RELEASE CANDIDATE ACCEPTANCE: PASS" if not self.warnings else "RELEASE CANDIDATE ACCEPTANCE: PASS_WITH_WARNINGS")
        return 0


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="isolated EmuWiz release-candidate acceptance gate")
    p.add_argument("--self-test", action="store_true", help="run harness failure-path self-tests")
    p.add_argument("--source-tree")
    p.add_argument("--gui-binary")
    p.add_argument("--cli-binary")
    p.add_argument("--output")
    p.add_argument("--timeout", type=int, default=120)
    p.add_argument("--keep", action="store_true", help="retain evidence after a failed setup")
    return p


def self_test() -> int:
    """Exercise the gate's safety primitives without binaries or user state."""
    root = Path(tempfile.mkdtemp(prefix="emuwiz-rc-selftest-"))
    try:
        source = Path(__file__).resolve().parents[2]
        output = root / "evidence"
        fake = root / "missing-binary"
        try:
            Gate(argparse.Namespace(source_tree=str(source), gui_binary=str(fake),
                                    cli_binary=str(fake), output=str(output), timeout=1,
                                    keep=False)).setup()
        except RuntimeError:
            pass
        else:
            raise AssertionError("missing/invalid binary was accepted")
        try:
            Gate(argparse.Namespace(source_tree=str(source), gui_binary=str(fake),
                                    cli_binary=str(fake), output="/tmp", timeout=1,
                                    keep=False)).setup()
        except RuntimeError:
            pass
        else:
            raise AssertionError("unsafe output root was accepted")
        try:
            subprocess.run(["sleep", "2"], timeout=0.05, check=True)
        except subprocess.TimeoutExpired:
            pass
        else:
            raise AssertionError("timeout fixture did not time out")
        bad = root / "bad.tar.xz"
        with tarfile.open(bad, "w:xz") as archive:
            info = tarfile.TarInfo("../escape"); info.size = 1
            import io
            archive.addfile(info, io.BytesIO(b"x"))
        with tarfile.open(bad, "r:*") as archive:
            if not any(Path(member.name).is_absolute() or ".." in Path(member.name).parts
                       for member in archive.getmembers()):
                raise AssertionError("unsafe archive fixture not detected")
        corrupt = root / "corrupt.sqlite3"; corrupt.write_bytes(b"not sqlite")
        try:
            sqlite3.connect(f"file:{corrupt}?mode=ro", uri=True).execute("PRAGMA quick_check").fetchone()
        except sqlite3.DatabaseError:
            pass
        else:
            raise AssertionError("corrupt SQLite fixture was accepted")
        (root / "archive-a").write_bytes(b"a"); (root / "archive-b").write_bytes(b"b")
        if sha256(root / "archive-a") == sha256(root / "archive-b"):
            raise AssertionError("non-reproducible archive fixture not detected")
        print("RC ACCEPTANCE SELF-TEST: PASS")
        return 0
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    try:
        args = parser().parse_args()
        if args.self_test:
            raise SystemExit(self_test())
        missing = [name for name in ("source_tree", "gui_binary", "cli_binary", "output") if not getattr(args, name)]
        if missing:
            parser().error("missing required options: " + ", ".join("--" + name.replace("_", "-") for name in missing))
        raise SystemExit(Gate(args).run())
    except Exception as exc:
        print(f"RELEASE CANDIDATE ACCEPTANCE: FAIL\nReason: {exc}", file=sys.stderr)
        raise SystemExit(3)
