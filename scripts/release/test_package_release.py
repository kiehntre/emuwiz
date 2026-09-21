#!/usr/bin/env python3
"""Disposable regression tests for the metadata-only release packager."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import pathlib
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("package_release.py")
SPEC = importlib.util.spec_from_file_location("package_release", SCRIPT)
assert SPEC and SPEC.loader
packager = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packager)


class ReleasePackagerTests(unittest.TestCase):
    maxDiff = None

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="emuwiz-packager-tests-")
        self.root = pathlib.Path(self.temporary.name)
        self.repo = SCRIPT.resolve().parents[2]
        self.gui = self._executable("gui-fixture", b"#!/bin/sh\nexit 0\n")
        self.cli = self._executable("cli-fixture", b"#!/bin/sh\nexit 0\n")
        self.epoch = "1700000000"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _executable(self, name: str, content: bytes) -> pathlib.Path:
        path = self.root / name
        path.write_bytes(content)
        path.chmod(0o755)
        return path

    def _run(self, *arguments: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment["SOURCE_DATE_EPOCH"] = self.epoch
        if env:
            environment.update(env)
        return subprocess.run(
            [sys.executable, str(SCRIPT), *arguments],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=environment,
            check=False,
        )

    def _package(self, name: str = "output", archive: bool = False) -> tuple[pathlib.Path, subprocess.CompletedProcess[str]]:
        output = self.root / name
        arguments = [
            "package",
            "--gui", str(self.gui),
            "--cli", str(self.cli),
            "--source-root", str(self.repo),
            "--output-root", str(output),
            "--arch", "x86_64",
            "--fixture-mode",
            "--reproducible",
        ]
        if archive:
            arguments.append("--archive")
        result = self._run(*arguments)
        return output / "emuwiz-0.9.0-linux-x86_64", result

    def _verify(self, target: pathlib.Path, strict: bool = True) -> subprocess.CompletedProcess[str]:
        args = ["verify"]
        if strict:
            args.append("--strict")
        args.append(str(target))
        return self._run(*args)

    @staticmethod
    def _rewrite_manifest_checksum(release: pathlib.Path) -> None:
        digest = hashlib.sha256((release / "manifest.json").read_bytes()).hexdigest()
        lines = (release / "SHA256SUMS").read_text().splitlines()
        lines = [f"{digest}  manifest.json" if line.endswith("  manifest.json") else line for line in lines]
        (release / "SHA256SUMS").write_text("\n".join(lines) + "\n")

    def test_01_valid_synthetic_executable_package(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        verified = self._verify(release)
        self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_02_missing_input_binary(self) -> None:
        result = self._run(
            "package", "--gui", str(self.root / "missing"), "--cli", str(self.cli),
            "--source-root", str(self.repo), "--output-root", str(self.root / "out"),
            "--fixture-mode", "--arch", "x86_64",
        )
        self.assertEqual(result.returncode, packager.EXIT_INPUT)

    def test_03_symlink_binary_refused(self) -> None:
        link = self.root / "gui-link"
        link.symlink_to(self.gui)
        result = self._run(
            "package", "--gui", str(link), "--cli", str(self.cli),
            "--source-root", str(self.repo), "--output-root", str(self.root / "out"),
            "--fixture-mode", "--arch", "x86_64",
        )
        self.assertEqual(result.returncode, packager.EXIT_INPUT)
        self.assertIn("symlink", result.stderr)

    def test_04_zero_byte_binary_refused(self) -> None:
        empty = self._executable("empty", b"")
        result = self._run(
            "package", "--gui", str(empty), "--cli", str(self.cli),
            "--source-root", str(self.repo), "--output-root", str(self.root / "out"),
            "--fixture-mode", "--arch", "x86_64",
        )
        self.assertEqual(result.returncode, packager.EXIT_INPUT)
        self.assertIn("zero bytes", result.stderr)

    def test_05_checksum_tamper(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        sums = release / "SHA256SUMS"
        sums.write_text(sums.read_text().replace(sums.read_text()[:64], "0" * 64, 1))
        self.assertEqual(self._verify(release).returncode, packager.EXIT_VERIFY)

    def test_06_missing_payload(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        (release / "bin/emuwiz-cli").unlink()
        self.assertEqual(self._verify(release).returncode, packager.EXIT_VERIFY)

    def test_07_manifest_artifact_hash_tamper(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest_path = release / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["artifacts"][0]["sha256"] = "0" * 64
        manifest_path.write_bytes(packager.json_bytes(manifest))
        self._rewrite_manifest_checksum(release)
        self.assertEqual(self._verify(release).returncode, packager.EXIT_VERIFY)

    def test_08_unexpected_executable_strict_mode(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        extra = release / "extra-tool"
        extra.write_text("#!/bin/sh\n")
        extra.chmod(0o755)
        self.assertEqual(self._verify(release, strict=False).returncode, 0)
        self.assertEqual(self._verify(release, strict=True).returncode, packager.EXIT_VERIFY)

    def test_09_unsafe_archive_member(self) -> None:
        archive_path = self.root / "unsafe.tar.xz"
        with tarfile.open(archive_path, "w:xz") as archive:
            info = tarfile.TarInfo("../escape")
            info.size = 1
            temporary = self.root / "byte"
            temporary.write_bytes(b"x")
            with temporary.open("rb") as source:
                archive.addfile(info, source)
        self.assertEqual(self._verify(archive_path).returncode, packager.EXIT_VERIFY)

    def test_10_reproducible_ordering_and_content(self) -> None:
        first, first_result = self._package("first")
        second, second_result = self._package("second")
        self.assertEqual(first_result.returncode, 0, first_result.stderr)
        self.assertEqual(second_result.returncode, 0, second_result.stderr)
        self.assertEqual((first / "manifest.json").read_bytes(), (second / "manifest.json").read_bytes())
        self.assertEqual((first / "SHA256SUMS").read_bytes(), (second / "SHA256SUMS").read_bytes())
        paths = [line.split("  ", 1)[1] for line in (first / "SHA256SUMS").read_text().splitlines()]
        self.assertEqual(paths, sorted(paths))

    def test_11_source_date_epoch_behavior(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads((release / "manifest.json").read_text())
        self.assertEqual(manifest["release"]["source_date_epoch"], int(self.epoch))
        self.assertEqual(manifest["release"]["packaging_timestamp"], "2023-11-14T22:13:20Z")
        missing = self._run(
            "package", "--gui", str(self.gui), "--cli", str(self.cli),
            "--source-root", str(self.repo), "--output-root", str(self.root / "missing-epoch"),
            "--fixture-mode", "--arch", "x86_64", "--reproducible",
            env={"SOURCE_DATE_EPOCH": ""},
        )
        self.assertEqual(missing.returncode, packager.EXIT_PROVENANCE)

    def test_12_temporary_extraction_is_cleaned(self) -> None:
        release, result = self._package(archive=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = release.parent / f"{release.name}.tar.xz"
        temp_parent = self.root / "verify-tmp"
        temp_parent.mkdir()
        verified = self._run("verify", "--strict", str(archive), env={"TMPDIR": str(temp_parent)})
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertEqual(list(temp_parent.iterdir()), [])

    def test_13_output_root_safety(self) -> None:
        result = self._run(
            "package", "--gui", str(self.gui), "--cli", str(self.cli),
            "--source-root", str(self.repo), "--output-root", "/",
            "--fixture-mode", "--arch", "x86_64",
        )
        self.assertEqual(result.returncode, packager.EXIT_UNSAFE)

    def test_14_secret_marker_refusal(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        suspect = release / "docs/suspect.txt"
        suspect.write_text("password=do-not-print-this-value\n")
        with self.assertRaises(packager.ReleaseError):
            packager.scan_secrets(release)

    def test_15_successful_archive_verify_round_trip(self) -> None:
        release, result = self._package(archive=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = release.parent / f"{release.name}.tar.xz"
        sidecar = archive.with_name(f"{archive.name}.sha256")
        verified = self._run("verify", "--strict", "--checksum", str(sidecar), str(archive))
        self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_16_one_binary_byte_changes(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        binary = release / "bin/emuwiz"
        data = bytearray(binary.read_bytes())
        data[-1] ^= 1
        binary.write_bytes(data)
        binary.chmod(0o755)
        self.assertEqual(self._verify(release).returncode, packager.EXIT_VERIFY)

    def test_17_symlink_replacement_is_rejected(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = release / "docs/README.txt"
        payload.unlink()
        payload.symlink_to(release / "VERIFY.txt")
        self.assertEqual(self._verify(release).returncode, packager.EXIT_VERIFY)

    def test_18_wrong_architecture_metadata_is_rejected(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest_path = release / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["artifacts"][0]["elf_arch"] = "aarch64"
        manifest_path.write_bytes(packager.json_bytes(manifest))
        self._rewrite_manifest_checksum(release)
        self.assertEqual(self._verify(release).returncode, packager.EXIT_VERIFY)

    def test_19_owned_output_required_for_overwrite(self) -> None:
        output = self.root / "owned-check"
        release = output / "emuwiz-0.9.0-linux-x86_64"
        release.mkdir(parents=True)
        (release / "foreign.txt").write_text("keep me")
        result = self._run(
            "package", "--gui", str(self.gui), "--cli", str(self.cli),
            "--source-root", str(self.repo), "--output-root", str(output),
            "--fixture-mode", "--arch", "x86_64", "--overwrite",
        )
        self.assertEqual(result.returncode, packager.EXIT_UNSAFE)
        self.assertTrue((release / "foreign.txt").is_file())

    def test_20_executable_mode_tamper_is_rejected(self) -> None:
        release, result = self._package()
        self.assertEqual(result.returncode, 0, result.stderr)
        binary = release / "bin/emuwiz-cli"
        binary.chmod(0o644)
        self.assertEqual(self._verify(release).returncode, packager.EXIT_VERIFY)

    def test_21_unowned_archive_is_not_deleted(self) -> None:
        output = self.root / "unowned-archive"
        output.mkdir()
        archive = output / "emuwiz-0.9.0-linux-x86_64.tar.xz"
        sidecar = output / f"{archive.name}.sha256"
        archive.write_bytes(b"foreign archive")
        sidecar.write_text("foreign checksum")
        result = self._run(
            "package", "--gui", str(self.gui), "--cli", str(self.cli),
            "--source-root", str(self.repo), "--output-root", str(output),
            "--fixture-mode", "--arch", "x86_64", "--overwrite", "--archive",
        )
        self.assertEqual(result.returncode, packager.EXIT_UNSAFE)
        self.assertEqual(archive.read_bytes(), b"foreign archive")
        self.assertEqual(sidecar.read_text(), "foreign checksum")


if __name__ == "__main__":
    unittest.main(verbosity=2)
