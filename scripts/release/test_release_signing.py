#!/usr/bin/env python3
"""Disposable signing and packager integration tests.

Every private key is generated in a temporary directory and removed with the
test fixture. No key material is written to the repository or release tree.
"""

from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("package_release.py")


def gpg(home: pathlib.Path, *arguments: str, input_text: str | None = None) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["GNUPGHOME"] = str(home)
    return subprocess.run(
        ["gpg", "--batch", "--no-tty", *arguments],
        input=input_text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        check=False,
        timeout=30,
    )


class ReleaseSigningTests(unittest.TestCase):
    def setUp(self) -> None:
        if subprocess.run(["sh", "-c", "command -v gpg"], stdout=subprocess.DEVNULL, check=False).returncode != 0:
            self.skipTest("gpg is not installed")
        self.temporary = tempfile.TemporaryDirectory(prefix="emuwiz-release-signing-tests-")
        self.root = pathlib.Path(self.temporary.name)
        self.gnupg = self.root / "gnupg"
        self.gnupg.mkdir(mode=0o700)
        self.private_key = self.root / "private.asc"
        self.public_key = self.root / "public.asc"
        if not self._generate_key("Test Publisher"):
            self.skipTest("GPG cannot create a disposable key in this environment")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _generate_key(self, name: str) -> bool:
        params = "\n".join([
            "Key-Type: RSA",
            "Key-Length: 2048",
            f"Name-Real: {name}",
            f"Name-Email: {name.lower().replace(' ', '.')}@example.invalid",
            "Expire-Date: 0",
            "%no-protection",
            "%commit",
            "",
        ])
        generated = gpg(self.gnupg, "--generate-key", input_text=params)
        if generated.returncode != 0:
            return False
        exported_private = gpg(self.gnupg, "--armor", "--export-secret-keys")
        self.assertEqual(exported_private.returncode, 0, exported_private.stderr)
        self.private_key.write_text(exported_private.stdout)
        exported_public = gpg(self.gnupg, "--armor", "--export")
        self.assertEqual(exported_public.returncode, 0, exported_public.stderr)
        self.public_key.write_text(exported_public.stdout)
        return True

    def _generate_other_public_key(self) -> pathlib.Path:
        home = self.root / "other-gnupg"
        home.mkdir(mode=0o700)
        params = "\n".join([
            "Key-Type: RSA", "Key-Length: 2048", "Name-Real: Other Publisher",
            "Name-Email: other@example.invalid", "Expire-Date: 0", "%no-protection", "%commit", "",
        ])
        generated = gpg(home, "--generate-key", input_text=params)
        self.assertEqual(generated.returncode, 0, generated.stderr)
        exported = gpg(home, "--armor", "--export")
        self.assertEqual(exported.returncode, 0, exported.stderr)
        path = self.root / "other-public.asc"
        path.write_text(exported.stdout)
        return path

    def _package(self, output: pathlib.Path, *extra: str) -> subprocess.CompletedProcess[str]:
        gui = self.root / "gui-fixture"
        cli = self.root / "cli-fixture"
        gui.write_bytes(b"#!/bin/sh\nexit 0\n")
        cli.write_bytes(b"#!/bin/sh\nexit 0\n")
        gui.chmod(0o755)
        cli.chmod(0o755)
        return subprocess.run(
            [
                sys.executable, str(SCRIPT), "package",
                "--gui", str(gui), "--cli", str(cli),
                "--source-root", str(SCRIPT.resolve().parents[2]),
                "--output-root", str(output), "--arch", "x86_64",
                "--fixture-mode", "--reproducible", "--sign",
                "--signing-key", str(self.private_key),
                "--public-key-output", str(output / "publisher.asc"),
                *extra,
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={**os.environ, "SOURCE_DATE_EPOCH": "1700000000"},
            check=False,
        )

    def test_valid_signature_and_archive_round_trip(self) -> None:
        output = self.root / "release"
        result = self._package(output, "--archive")
        self.assertEqual(result.returncode, 0, result.stderr)
        release = output / "emuwiz-0.9.0-linux-x86_64"
        signature = output / "emuwiz-0.9.0-linux-x86_64.SHA256SUMS.asc"
        archive = output / "emuwiz-0.9.0-linux-x86_64.tar.xz"
        self.assertTrue(signature.is_file())
        self.assertTrue((output / "publisher.asc").is_file())
        self.assertTrue(archive.is_file())
        verified = subprocess.run(
            [sys.executable, str(SCRIPT), "verify", "--strict", "--verify-signature",
             "--public-key", str(self.public_key), str(release)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertIn("VALID SIGNATURE", verified.stdout)
        archive_verified = subprocess.run(
            [sys.executable, str(SCRIPT), "verify", "--strict", "--verify-signature",
             "--public-key", str(self.public_key), str(archive)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        self.assertEqual(archive_verified.returncode, 0, archive_verified.stderr)
        self.assertIn("VALID SIGNATURE", archive_verified.stdout)
        self.assertNotIn(b"PRIVATE KEY", b"".join(path.read_bytes() for path in release.rglob("*") if path.is_file()))

    def test_changed_checksum_and_signature_are_rejected(self) -> None:
        output = self.root / "release"
        result = self._package(output)
        self.assertEqual(result.returncode, 0, result.stderr)
        release = output / "emuwiz-0.9.0-linux-x86_64"
        signature = output / "emuwiz-0.9.0-linux-x86_64.SHA256SUMS.asc"
        sums = release / "SHA256SUMS"
        original = sums.read_bytes()
        sums.write_bytes(original.replace(b"  manifest.json", b"  changed.json"))
        invalid = subprocess.run(
            [sys.executable, str(SCRIPT), "verify", "--verify-signature", "--public-key", str(self.public_key), str(release)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        self.assertNotEqual(invalid.returncode, 0)
        sums.write_bytes(original)
        signature_bytes = signature.read_bytes()
        body_start = signature_bytes.find(b"\n\n") + 2
        changed = bytearray(signature_bytes)
        changed[body_start] = ord("B") if changed[body_start] != ord("B") else ord("C")
        signature.write_bytes(bytes(changed))
        invalid_signature = subprocess.run(
            [sys.executable, str(SCRIPT), "verify", "--verify-signature", "--public-key", str(self.public_key), str(release)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        self.assertNotEqual(invalid_signature.returncode, 0)
        self.assertIn("INVALID SIGNATURE", invalid_signature.stdout + invalid_signature.stderr)

    def test_wrong_key_missing_signature_and_symlink_key_fail_closed(self) -> None:
        output = self.root / "release"
        result = self._package(output)
        self.assertEqual(result.returncode, 0, result.stderr)
        release = output / "emuwiz-0.9.0-linux-x86_64"
        signature = output / "emuwiz-0.9.0-linux-x86_64.SHA256SUMS.asc"
        wrong_key = self._generate_other_public_key()
        wrong = subprocess.run(
            [sys.executable, str(SCRIPT), "verify", "--verify-signature", "--public-key", str(wrong_key), str(release)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        self.assertNotEqual(wrong.returncode, 0)
        self.assertIn("INVALID SIGNATURE", wrong.stdout + wrong.stderr)
        signature.unlink()
        missing = subprocess.run(
            [sys.executable, str(SCRIPT), "verify", "--verify-signature", "--public-key", str(self.public_key), str(release)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        self.assertEqual(missing.returncode, 0, missing.stderr)
        self.assertIn("SIGNATURE NOT PROVIDED", missing.stdout)
        signature = output / "emuwiz-0.9.0-linux-x86_64.SHA256SUMS.asc"
        # A symlinked signing-key input is rejected before any package tree is created.
        key_link = self.root / "key-link"
        key_link.symlink_to(self.private_key)
        failed = subprocess.run(
            [sys.executable, str(SCRIPT), "package", "--gui", str(self.root / "gui-fixture"), "--cli", str(self.root / "cli-fixture"),
             "--source-root", str(SCRIPT.resolve().parents[2]), "--output-root", str(self.root / "unsafe"),
             "--arch", "x86_64", "--fixture-mode", "--sign", "--signing-key", str(key_link)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        self.assertNotEqual(failed.returncode, 0)
        self.assertNotIn("PRIVATE KEY", failed.stdout + failed.stderr)


if __name__ == "__main__":
    unittest.main()
