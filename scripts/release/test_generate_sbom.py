#!/usr/bin/env python3
"""Offline fixture tests for EmuWiz SBOM generation and verification."""

from __future__ import annotations

import io
import json
import os
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("generate_sbom.py")
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"


class Fixture:
    def __init__(self, root: pathlib.Path, rich: bool):
        self.root = root
        self.repo = root / "repo"
        self.cargo_home = root / "cargo-home"
        self.metadata = root / "metadata.json"
        self.repo.mkdir(parents=True)
        (self.repo / "app").mkdir()
        (self.repo / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["app"]\nresolver = "2"\n\n'
            '[workspace.package]\nversion = "1.2.3"\nedition = "2024"\n'
        )
        dependencies = {
            "good": '"1"',
        }
        if rich:
            dependencies.update(
                {
                    "dual": '"1"',
                    "duplicate": '"1"',
                    "gitdep": '{ git = "https://example.invalid/repo" }',
                    "pathdep": '{ path = "../pathdep" }',
                    "checksummed": '"1"',
                    "missing": '"1"',
                    "missingfile": '"1"',
                    "unavailable": '"1"',
                }
            )
        dependency_text = "\n".join(f"{name} = {value}" for name, value in dependencies.items())
        (self.repo / "app/Cargo.toml").write_text(
            '[package]\nname = "app"\nversion.workspace = true\nedition.workspace = true\n\n'
            f"[dependencies]\n{dependency_text}\n"
        )
        lock_dependencies = list(dependencies)
        if rich:
            lock_dependencies[lock_dependencies.index("duplicate")] = "duplicate 1.0.0"
            lock_dependencies[lock_dependencies.index("gitdep")] = (
                "gitdep 0.1.0 (git+https://example.invalid/repo#abc)"
            )
        packages = [
            self._lock_package("app", "1.2.3", None, None, lock_dependencies),
            self._lock_package("good", "1.0.0", CRATES_IO, "1" * 64, []),
        ]
        self._crate("good", "1.0.0", 'license = "MIT"', {"LICENSE": "MIT fixture text\n"})
        if rich:
            packages.extend(
                [
                    self._lock_package("dual", "1.0.0", CRATES_IO, "2" * 64, ["duplicate 2.0.0"]),
                    self._lock_package("duplicate", "1.0.0", CRATES_IO, "3" * 64, []),
                    self._lock_package("duplicate", "2.0.0", CRATES_IO, "4" * 64, []),
                    self._lock_package("gitdep", "0.1.0", "git+https://example.invalid/repo#abc", None, []),
                    self._lock_package("pathdep", "0.1.0", None, None, []),
                    self._lock_package("checksummed", "1.0.0", CRATES_IO, "5" * 64, []),
                    self._lock_package("missing", "1.0.0", CRATES_IO, "6" * 64, []),
                    self._lock_package("missingfile", "1.0.0", CRATES_IO, "7" * 64, []),
                    self._lock_package("unavailable", "1.0.0", CRATES_IO, "8" * 64, []),
                ]
            )
            self._crate(
                "dual",
                "1.0.0",
                'license = "MIT OR Apache-2.0"',
                {"LICENSE-MIT": "MIT fixture text\n", "LICENSE-APACHE": "Apache fixture text\n"},
            )
            self._crate("duplicate", "1.0.0", 'license = "BSD-3-Clause"', {"LICENSE": "BSD fixture\n"})
            self._crate("duplicate", "2.0.0", 'license = "Apache-2.0"', {"LICENSE": "Apache fixture text\n"})
            self._crate("checksummed", "1.0.0", 'license = "MIT"', {"LICENSE": "MIT fixture text\n"})
            self._crate("missing", "1.0.0", "", {})
            self._crate("missingfile", "1.0.0", 'license-file = "NOT-PRESENT"', {})
        lock = "# generated fixture\nversion = 4\n\n" + "\n".join(packages)
        (self.repo / "Cargo.lock").write_text(lock)
        manifest = str((self.repo / "app/Cargo.toml").resolve())
        package_id = "path+fixture#app@1.2.3"
        self.metadata.write_text(
            json.dumps(
                {
                    "packages": [
                        {
                            "name": "app",
                            "version": "1.2.3",
                            "id": package_id,
                            "manifest_path": manifest,
                            "license": None,
                            "license_file": None,
                            "repository": None,
                            "homepage": None,
                        }
                    ],
                    "workspace_members": [package_id],
                    "resolve": None,
                }
            )
        )
        subprocess.run(["git", "init", "-q"], cwd=self.repo, check=True)
        subprocess.run(["git", "config", "user.email", "fixture@example.invalid"], cwd=self.repo, check=True)
        subprocess.run(["git", "config", "user.name", "Fixture"], cwd=self.repo, check=True)
        subprocess.run(["git", "add", "Cargo.toml", "Cargo.lock", "app/Cargo.toml"], cwd=self.repo, check=True)
        env = os.environ.copy()
        env["GIT_AUTHOR_DATE"] = env["GIT_COMMITTER_DATE"] = "1700000000 +0000"
        subprocess.run(["git", "commit", "-q", "-m", "fixture"], cwd=self.repo, check=True, env=env)

    @staticmethod
    def _lock_package(name: str, version: str, source: str | None, checksum: str | None, dependencies: list[str]) -> str:
        lines = ["[[package]]", f'name = "{name}"', f'version = "{version}"']
        if source:
            lines.append(f'source = "{source}"')
        if checksum:
            lines.append(f'checksum = "{checksum}"')
        if dependencies:
            lines.append("dependencies = [")
            lines.extend(f' "{value}",' for value in dependencies)
            lines.append("]")
        return "\n".join(lines) + "\n"

    def _crate(self, name: str, version: str, package_fields: str, files: dict[str, str]) -> None:
        cache = self.cargo_home / "registry/cache/fixture"
        cache.mkdir(parents=True, exist_ok=True)
        archive_path = cache / f"{name}-{version}.crate"
        manifest = f'[package]\nname = "{name}"\nversion = "{version}"\n{package_fields}\n'
        with tarfile.open(archive_path, "w:gz") as archive:
            for relative, body in {"Cargo.toml": manifest, **files}.items():
                data = body.encode()
                info = tarfile.TarInfo(f"{name}-{version}/{relative}")
                info.size = len(data)
                info.mtime = 0
                archive.addfile(info, io.BytesIO(data))


class SbomTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="emuwiz-sbom-tests-")
        self.root = pathlib.Path(self.temporary.name)
        self.fixture = Fixture(self.root / "rich", rich=True)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_tool(self, *args: str) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env["SOURCE_DATE_EPOCH"] = "1700000000"
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
            env=env,
        )

    def generate(self, name: str = "SBOM", fixture: Fixture | None = None, strict: bool = False) -> tuple[pathlib.Path, subprocess.CompletedProcess[str]]:
        fixture = fixture or self.fixture
        output = self.root / name
        args = [
            "generate",
            "--source-root", str(fixture.repo),
            "--cargo-home", str(fixture.cargo_home),
            "--metadata-file", str(fixture.metadata),
            "--output-dir", str(output),
        ]
        if strict:
            args.append("--strict")
        return output, self.run_tool(*args)

    def verify(self, output: pathlib.Path, fixture: Fixture | None = None, strict: bool = False) -> subprocess.CompletedProcess[str]:
        fixture = fixture or self.fixture
        args = ["verify", "--source-root", str(fixture.repo)]
        if strict:
            args.append("--strict")
        args.append(str(output))
        return self.run_tool(*args)

    @staticmethod
    def load(output: pathlib.Path, name: str) -> dict:
        return json.loads((output / name).read_text())

    def test_01_valid_minimal_lockfile_fixture(self) -> None:
        fixture = Fixture(self.root / "minimal", rich=False)
        output, generated = self.generate("minimal-output", fixture, strict=True)
        self.assertEqual(generated.returncode, 0, generated.stderr)
        self.assertEqual(self.verify(output, fixture, strict=True).returncode, 0)

    def test_02_multiple_versions_same_crate(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        summary = self.load(output, "dependency-summary.json")
        self.assertIn({"name": "duplicate", "versions": ["1.0.0", "2.0.0"]}, summary["duplicate_versions"])

    def test_03_missing_licence_declaration(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        packages = self.load(output, "third-party-licenses.json")["packages"]
        missing = next(item for item in packages if item["name"] == "missing")
        self.assertEqual(missing["license_classification"], "MISSING_DECLARATION")

    def test_04_dual_licence_is_preserved(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        packages = self.load(output, "third-party-licenses.json")["packages"]
        dual = next(item for item in packages if item["name"] == "dual")
        self.assertEqual(dual["license_classification"], "MULTIPLE_DECLARED")
        self.assertEqual(dual["license_expression"], "MIT OR Apache-2.0")

    def test_05_git_dependency_is_flagged(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        summary = self.load(output, "dependency-summary.json")
        self.assertEqual(len(summary["git_dependencies"]), 1)
        self.assertIn("gitdep", summary["git_dependencies"][0])

    def test_06_workspace_and_external_path_are_distinguished(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        summary = self.load(output, "dependency-summary.json")
        self.assertEqual(summary["counts"]["workspace_packages"], 1)
        self.assertEqual(summary["counts"]["source_types"]["path-outside-workspace"], 1)

    def test_07_registry_checksum_is_copied(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        components = self.load(output, "emuwiz-sbom.cdx.json")["components"]
        component = next(item for item in components if item["name"] == "checksummed")
        self.assertEqual(component["hashes"], [{"alg": "SHA-256", "content": "5" * 64}])

    def test_08_modified_lockfile_is_rejected(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        with (self.fixture.repo / "Cargo.lock").open("a") as target:
            target.write("\n# modified\n")
        verified = self.verify(output)
        self.assertEqual(verified.returncode, 2)
        self.assertIn("Cargo.lock", verified.stderr)

    def test_09_tampered_sbom_is_rejected(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        with (output / "emuwiz-sbom.cdx.json").open("a") as target:
            target.write(" ")
        self.assertEqual(self.verify(output).returncode, 2)

    def test_10_missing_licence_file_is_reported(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        packages = self.load(output, "third-party-licenses.json")["packages"]
        item = next(package for package in packages if package["name"] == "missingfile")
        self.assertEqual(item["license_classification"], "DECLARED")
        self.assertEqual(item["license_text_status"], "NOT_FOUND")

    def test_11_generation_is_deterministic(self) -> None:
        first, first_result = self.generate("first")
        second, second_result = self.generate("second")
        self.assertEqual(first_result.returncode, 0, first_result.stderr)
        self.assertEqual(second_result.returncode, 0, second_result.stderr)
        for name in (*("emuwiz-sbom.cdx.json", "third-party-licenses.json", "THIRD_PARTY_LICENSES.txt", "dependency-summary.json"), "SBOM_SHA256SUMS"):
            self.assertEqual((first / name).read_bytes(), (second / name).read_bytes(), name)

    def test_12_strict_mode_fails_unresolved_metadata(self) -> None:
        _, result = self.generate(strict=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn("strict generation failed", result.stderr)

    def test_13_non_strict_mode_warns_and_verifies(self) -> None:
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("warning:", result.stderr)
        verified = self.verify(output)
        self.assertEqual(verified.returncode, 0, verified.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
