#!/usr/bin/env python3
"""Offline fixture tests for EmuWiz SBOM generation and verification."""

from __future__ import annotations

import io
import json
import os
import pathlib
import subprocess
import sys
import shutil
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

    def supplemented(self, crate_sha: str | None = None, upstream_sha: str | None = None,
                     crate_path: str = "fonts/OFL.txt") -> tuple[pathlib.Path, subprocess.CompletedProcess[str]]:
        """Generate with a reviewed supplement manifest: one nested notice
        read from the published crate, one stored upstream text."""
        import hashlib
        ofl, upstream = "SIL Open Font License fixture\n", "Copyright (c) Upstream Fixture\nMIT text\n"
        self.fixture._crate("checksummed", "1.0.0", 'license = "MIT"', {"LICENSE": "MIT fixture text\n", "fonts/OFL.txt": ofl})
        texts = self.root / "supplements" / "missingfile-1.0.0"
        texts.mkdir(parents=True)
        (texts / "LICENSE").write_text(upstream)
        manifest = self.root / "supplements.json"
        manifest.write_text(json.dumps({"schema_version": 1, "packages": [
            {"name": "checksummed", "version": "1.0.0", "crate_files": [
                {"path": crate_path, "sha256": crate_sha or hashlib.sha256(ofl.encode()).hexdigest()}]},
            {"name": "missingfile", "version": "1.0.0", "upstream_files": [
                {"file": "LICENSE", "origin": "upstream-commit", "url": "https://example.invalid/LICENSE",
                 "commit": "a" * 40, "sha256": upstream_sha or hashlib.sha256(upstream.encode()).hexdigest()}]},
            {"name": "missing", "version": "1.0.0", "unresolved": "No notice published upstream."},
            {"name": "good", "version": "1.0.0", "reason": "Declared MIT; standard text.",
             "notice_gap": "Upstream publishes no copyright notice."},
        ]}))
        output = self.root / "supplemented"
        return output, self.run_tool(
            "generate", "--source-root", str(self.fixture.repo), "--cargo-home", str(self.fixture.cargo_home),
            "--metadata-file", str(self.fixture.metadata), "--output-dir", str(output),
            "--licence-supplements", str(manifest))

    def test_14_licence_supplements_are_included_verbatim_with_provenance(self) -> None:
        output, generated = self.supplemented()
        self.assertEqual(generated.returncode, 0, generated.stderr)
        self.assertEqual(self.verify(output).returncode, 0)
        packages = {item["name"]: item for item in self.load(output, "third-party-licenses.json")["packages"]}
        self.assertEqual(packages["missingfile"]["license_text_status"], "AVAILABLE_SUPPLEMENTED")
        self.assertEqual(packages["missingfile"]["license_text_supplements"][0]["commit"], "a" * 40)
        self.assertEqual(packages["checksummed"]["license_text_status"], "AVAILABLE")
        self.assertEqual(packages["checksummed"]["license_text_supplements"][0]["origin"], "published-crate")
        self.assertEqual(packages["missing"]["license_text_unresolved"], "No notice published upstream.")
        notice = (output / "THIRD_PARTY_LICENSES.txt").read_text()
        self.assertIn("SIL Open Font License fixture\n", notice)
        self.assertIn("Copyright (c) Upstream Fixture\nMIT text\n", notice)
        self.assertIn("No notice published upstream.", notice.split("Licence texts that could not be sourced")[1])
        self.assertEqual(packages["good"]["license_notice_gap"], "Upstream publishes no copyright notice.")
        gaps = notice.split("Declared licences without an upstream licence file or notice")[1]
        self.assertIn("Declared MIT; standard text.", gaps.split("Licence texts that could not be sourced")[0])

    def test_15_a_supplement_that_does_not_match_its_pin_is_fatal(self) -> None:
        for kwargs in ({"crate_sha": "0" * 64}, {"upstream_sha": "0" * 64}, {"crate_path": "../escape"}):
            with self.subTest(**kwargs):
                shutil.rmtree(self.root / "supplements", ignore_errors=True)
                output, generated = self.supplemented(**kwargs)
                self.assertNotEqual(generated.returncode, 0, kwargs)
                self.assertFalse(output.exists())

    def test_17_a_symlinked_supplement_text_or_missing_manifest_is_fatal(self) -> None:
        output, generated = self.supplemented()
        self.assertEqual(generated.returncode, 0, generated.stderr)
        stored = self.root / "supplements" / "missingfile-1.0.0" / "LICENSE"
        real = self.root / "elsewhere-LICENSE"
        real.write_bytes(stored.read_bytes())
        stored.unlink()
        stored.symlink_to(real)
        shutil.rmtree(output)
        common = ["generate", "--source-root", str(self.fixture.repo), "--cargo-home", str(self.fixture.cargo_home),
                  "--metadata-file", str(self.fixture.metadata)]
        rerun = self.run_tool(*common, "--output-dir", str(output),
                              "--licence-supplements", str(self.root / "supplements.json"))
        self.assertNotEqual(rerun.returncode, 0)
        self.assertIn("licence supplement file is missing", rerun.stderr)
        self.assertFalse(output.exists())
        absent = self.run_tool(*common, "--output-dir", str(self.root / "absent"),
                               "--licence-supplements", str(self.root / "no-such.json"))
        self.assertNotEqual(absent.returncode, 0)
        self.assertIn("manifest is missing", absent.stderr)

    def test_16_the_shipped_supplement_manifest_matches_the_real_lockfile(self) -> None:
        output = self.root / "real"
        generated = self.run_tool("generate", "--output-dir", str(output))
        self.assertEqual(generated.returncode, 0, generated.stderr)
        self.assertNotIn("licence supplement matches no locked package", generated.stderr)
        notice = (output / "THIRD_PARTY_LICENSES.txt").read_text()
        for required in ("SIL OPEN FONT LICENSE", "UBUNTU FONT LICENCE", "Mozilla Public License Version 2.0",
                         "Copyright (c) 2018-2021 Emil Ernerfeldt"):
            self.assertIn(required, notice)
        # affs-read: the declared MIT text is supplied and labelled; the
        # missing upstream notice stays documented; nothing is unsourced.
        packages = {item["name"]: item for item in self.load(output, "third-party-licenses.json")["packages"]}
        affs = packages["affs-read"]
        self.assertEqual(affs["license_text_status"], "AVAILABLE_SUPPLEMENTED")
        self.assertEqual(affs["license_text_supplements"][0]["origin"], "declared-licence-standard-text")
        self.assertIn("no copyright notice", affs["license_notice_gap"])
        gaps = notice.split("Declared licences without an upstream licence file or notice")[1]
        self.assertIn("affs-read 0.4.1", gaps.split("Licence texts that could not be sourced")[0])
        self.assertEqual(notice.split("Licence texts that could not be sourced")[1].split("=" * 78)[0].strip(), "None.")

    def reviewed_fixture(self, name: str, *, published_crate: bool = True, **changes):
        import hashlib
        fixture = Fixture(self.root / name, rich=False)
        fixture._crate("good", "1.0.0", 'license = "MIT/Apache-2.0"',
                       {"LICENSE-MIT": "MIT fixture text\n", "LICENSE-APACHE": "Apache fixture text\n"})
        import tarfile
        archive = next(fixture.cargo_home.rglob("good-1.0.0.crate"))
        with tarfile.open(archive) as source:
            manifest = source.extractfile("good-1.0.0/Cargo.toml").read()
        review = {
            "declared": "MIT/Apache-2.0", "expression": "MIT OR Apache-2.0",
            "crate_sha256": "1" * 64, "basis": "Synthetic explicit dual grant.",
            "evidence": [{"path": "Cargo.toml", "sha256": hashlib.sha256(manifest).hexdigest(),
                          "url": "https://example.invalid/published-crate"}],
        }
        review.update(changes)
        if not published_crate:
            archive.unlink()
        supplements = fixture.root / "review.json"
        supplements.write_text(json.dumps({"schema_version": 1, "packages": [
            {"name": "good", "version": "1.0.0", "license_review": review}]}))
        output = fixture.root / "SBOM"
        result = self.run_tool("generate", "--source-root", str(fixture.repo),
            "--cargo-home", str(fixture.cargo_home), "--metadata-file", str(fixture.metadata),
            "--licence-supplements", str(supplements), "--output-dir", str(output), "--strict")
        return output, result

    def test_18_reviewed_expression_keeps_original_declaration_and_evidence(self) -> None:
        output, result = self.reviewed_fixture("reviewed")
        self.assertEqual(result.returncode, 0, result.stderr)
        package = self.load(output, "third-party-licenses.json")["packages"][0]
        self.assertEqual(package["license_expression"], "MIT OR Apache-2.0")
        self.assertEqual(package["license_expression_review"]["declared"], "MIT/Apache-2.0")
        self.assertEqual(package["license_classification"], "MULTIPLE_DECLARED")
        component = next(x for x in self.load(output, "emuwiz-sbom.cdx.json")["components"] if x["name"] == "good")
        self.assertEqual(component["licenses"], [{"expression": "MIT OR Apache-2.0"}])
        self.assertIn("MIT/Apache-2.0", (output / "THIRD_PARTY_LICENSES.txt").read_text())
        verified = self.run_tool("verify", "--strict", "--source-root", str(self.root / "reviewed/repo"), str(output))
        self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_19_review_refuses_changed_declaration_checksum_evidence_or_invalid_expression(self) -> None:
        for index, changes in enumerate([
            {"declared": "Apache-2.0/MIT"}, {"crate_sha256": "0" * 64},
            {"evidence": []}, {"basis": ""}, {"expression": "MIT/Apache-2.0"},
            {"evidence": [{"path": "LICENSE-MIT", "sha256": "0" * 64, "url": "https://example.invalid/license"}]},
            {"evidence": [{"path": "absent", "sha256": "0" * 64, "url": "https://example.invalid/license"}]},
        ]):
            with self.subTest(changes=changes):
                _, result = self.reviewed_fixture(f"invalid-review-{index}", **changes)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("licence review", result.stderr)

    def test_22_review_of_a_crate_absent_from_the_cargo_home_names_the_missing_source(self) -> None:
        # A Linux build never downloads platform-only crates such as bitflags 1.3.2.
        _, result = self.reviewed_fixture("review-unfetched", published_crate=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("published crate is not in the Cargo home", result.stderr)
        self.assertIn("good 1.0.0", result.stderr)

    def test_20_unknown_slash_declaration_stays_ambiguous(self) -> None:
        self.fixture._crate("dual", "1.0.0", 'license = "MIT/Apache-2.0"', {"LICENSE": "fixture grant\n"})
        output, result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        package = next(x for x in self.load(output, "third-party-licenses.json")["packages"] if x["name"] == "dual")
        self.assertEqual(package["license_classification"], "AMBIGUOUS")
        self.assertEqual(package["license_expression"], "MIT/Apache-2.0")

    def test_21_real_inventory_resolves_all_26_and_retains_bundled_notices(self) -> None:
        output = self.root / "reviewed-real"
        generated = self.run_tool("generate", "--strict", "--output-dir", str(output))
        self.assertEqual(generated.returncode, 0, generated.stderr)
        summary = self.load(output, "dependency-summary.json")
        self.assertEqual(summary["missing_or_ambiguous_license_metadata"], [])
        packages = self.load(output, "third-party-licenses.json")["packages"]
        reviewed = {x["name"]: x for x in packages if "license_expression_review" in x}
        self.assertEqual(len(reviewed), 26)
        self.assertEqual(reviewed["brotli-decompressor"]["license_expression"], "BSD-3-Clause AND MIT")
        self.assertEqual(reviewed["bzip2-sys"]["license_expression"], "(MIT OR Apache-2.0) AND bzip2-1.0.6")
        self.assertEqual(reviewed["zstd-sys"]["license_expression"], "(MIT OR Apache-2.0) AND BSD-3-Clause")
        notice = (output / "THIRD_PARTY_LICENSES.txt").read_text()
        for required in ["Copyright 2013 Google Inc.", "copyright (C) 1996-2019 Julian R Seward",
                         "Copyright (c) Meta Platforms, Inc. and affiliates.", "Copyright 2016-2026 Frank Denis."]:
            self.assertIn(required, notice)
        for name in ["brotli-decompressor", "bzip2-sys", "zstd-sys", "siphasher"]:
            self.assertTrue(reviewed[name]["license_text_supplements"])

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
