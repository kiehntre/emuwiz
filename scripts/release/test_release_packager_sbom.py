"""Integration tests joining the release packager and SBOM verifier."""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest


RELEASE_SCRIPT = pathlib.Path(__file__).with_name("package_release.py")
SBOM_SCRIPT = pathlib.Path(__file__).with_name("generate_sbom.py")


class ReleasePackagerSbomTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="emuwiz-release-sbom-integration-")
        self.root = pathlib.Path(self.temporary.name)
        self.repo = RELEASE_SCRIPT.resolve().parents[2]
        self.gui = self._executable("gui", b"fixture gui\n")
        self.cli = self._executable("cli", b"fixture cli\n")
        self.env = os.environ.copy()
        self.env["SOURCE_DATE_EPOCH"] = "1700000000"
        self.sbom = self.root / "verified-sbom"
        generated = self._run_sbom(
            "generate",
            "--source-root", str(self.repo),
            "--cargo-home", os.environ.get("CARGO_HOME", str(pathlib.Path.home() / ".cargo")),
            "--output-dir", str(self.sbom),
        )
        self.assertEqual(generated.returncode, 0, generated.stderr)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _executable(self, name: str, data: bytes) -> pathlib.Path:
        path = self.root / name
        path.write_bytes(data)
        path.chmod(0o755)
        return path

    def _run_sbom(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SBOM_SCRIPT), *args],
            env=self.env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def _run_package(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(RELEASE_SCRIPT), "package", *args],
            env=self.env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def _run_verify(self, target: pathlib.Path, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(RELEASE_SCRIPT), "verify", "--strict", *extra, str(target)],
            env=self.env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def _copy_sbom(self, name: str = "sbom") -> pathlib.Path:
        destination = self.root / name
        shutil.copytree(self.sbom, destination)
        return destination

    def _package(self, name: str, sbom: pathlib.Path | None, archive: bool = False, require: bool = False) -> tuple[pathlib.Path, subprocess.CompletedProcess[str]]:
        output = self.root / name
        args = [
            "--gui", str(self.gui), "--cli", str(self.cli),
            "--source-root", str(self.repo), "--output-root", str(output),
            "--arch", "x86_64", "--fixture-mode", "--reproducible",
        ]
        if sbom is not None:
            args.extend(["--sbom-dir", str(sbom)])
        if require:
            args.append("--require-sbom")
        if archive:
            args.append("--archive")
        result = self._run_package(*args)
        return output / "emuwiz-0.9.0-linux-x86_64", result

    def test_01_valid_verified_sbom_is_packaged_and_verified(self) -> None:
        release, result = self._package("valid", self.sbom, require=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self._run_verify(release, "--source-root", str(self.repo)).returncode, 0)
        manifest = json.loads((release / "manifest.json").read_text())
        self.assertEqual(manifest["sbom"]["package_count"], 493)

    def test_02_optional_no_sbom_package_remains_supported(self) -> None:
        release, result = self._package("optional", None)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("sbom", json.loads((release / "manifest.json").read_text()))

    def test_03_require_sbom_without_bundle_fails(self) -> None:
        _, result = self._package("required-missing", None, require=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--require-sbom", result.stderr)

    def test_04_missing_bundle_member_fails(self) -> None:
        bundle = self._copy_sbom("missing-member")
        (bundle / "dependency-summary.json").unlink()
        _, result = self._package("missing-member-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_05_changed_cyclonedx_json_fails(self) -> None:
        bundle = self._copy_sbom("changed-cyclonedx")
        path = bundle / "emuwiz-sbom.cdx.json"
        path.write_text(path.read_text().replace('"version": 1', '"version": 2', 1))
        _, result = self._package("changed-cyclonedx-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_06_changed_license_text_fails(self) -> None:
        bundle = self._copy_sbom("changed-license")
        path = bundle / "THIRD_PARTY_LICENSES.txt"
        path.write_text(path.read_text() + "tampered\n")
        _, result = self._package("changed-license-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_07_changed_dependency_summary_fails(self) -> None:
        bundle = self._copy_sbom("changed-summary")
        path = bundle / "dependency-summary.json"
        data = json.loads(path.read_text())
        data["counts"]["unique_packages"] += 1
        path.write_text(json.dumps(data))
        _, result = self._package("changed-summary-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_08_changed_sbom_checksums_fail(self) -> None:
        bundle = self._copy_sbom("changed-checksums")
        path = bundle / "SBOM_SHA256SUMS"
        path.write_text(path.read_text().replace(path.read_text()[:64], "0" * 64, 1))
        _, result = self._package("changed-checksums-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_09_cargo_lock_mismatch_fails(self) -> None:
        bundle = self._copy_sbom("changed-lock")
        path = bundle / "dependency-summary.json"
        data = json.loads(path.read_text())
        data["cargo_lock_sha256"] = "0" * 64
        path.write_text(json.dumps(data))
        _, result = self._package("changed-lock-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_10_product_source_mismatch_fails(self) -> None:
        bundle = self._copy_sbom("changed-source")
        path = bundle / "dependency-summary.json"
        data = json.loads(path.read_text())
        data["source_commit"] = "0" * 40
        path.write_text(json.dumps(data))
        _, result = self._package("changed-source-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_11_symlink_replacement_fails(self) -> None:
        bundle = self._copy_sbom("symlink")
        target = bundle / "dependency-summary.json"
        target.unlink()
        target.symlink_to(bundle / "third-party-licenses.json")
        _, result = self._package("symlink-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_12_unexpected_file_fails(self) -> None:
        bundle = self._copy_sbom("unexpected")
        (bundle / "unexpected.bin").write_bytes(b"not an sbom")
        _, result = self._package("unexpected-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_13_unsupported_bundle_schema_fails(self) -> None:
        bundle = self._copy_sbom("unsupported-schema")
        path = bundle / "dependency-summary.json"
        data = json.loads(path.read_text())
        data["schema_version"] = 99
        path.write_text(json.dumps(data))
        _, result = self._package("unsupported-schema-output", bundle, require=True)
        self.assertNotEqual(result.returncode, 0)

    def test_14_release_checksum_tamper_fails(self) -> None:
        release, result = self._package("release-tamper", self.sbom, require=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        path = release / "SBOM/THIRD_PARTY_LICENSES.txt"
        path.write_text(path.read_text() + "tampered\n")
        self.assertNotEqual(self._run_verify(release).returncode, 0)

    def test_15_archive_extract_and_verify_succeeds(self) -> None:
        release, result = self._package("archive", self.sbom, archive=True, require=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = release.parent / f"{release.name}.tar.xz"
        self.assertTrue(archive.is_file())
        self.assertEqual(self._run_verify(archive, "--source-root", str(self.repo)).returncode, 0)

    def test_16_reproducible_archives_include_identical_sbom(self) -> None:
        first, first_result = self._package("repro-first", self.sbom, archive=True, require=True)
        second, second_result = self._package("repro-second", self.sbom, archive=True, require=True)
        self.assertEqual(first_result.returncode, 0, first_result.stderr)
        self.assertEqual(second_result.returncode, 0, second_result.stderr)
        first_archive = first.parent / f"{first.name}.tar.xz"
        second_archive = second.parent / f"{second.name}.tar.xz"
        self.assertEqual(hashlib.sha256(first_archive.read_bytes()).digest(), hashlib.sha256(second_archive.read_bytes()).digest())
        self.assertEqual((first / "SBOM/SBOM_SHA256SUMS").read_bytes(), (second / "SBOM/SBOM_SHA256SUMS").read_bytes())


if __name__ == "__main__":
    unittest.main(verbosity=2)
