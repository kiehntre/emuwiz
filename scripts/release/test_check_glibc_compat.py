#!/usr/bin/env python3
"""check_glibc_compat.py against real ELF files (system binaries) and fakes."""
import io
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import unittest
from unittest import mock

import check_glibc_compat as checker


def make_archive(path: Path, members: dict[str, bytes]) -> None:
    with tarfile.open(path, "w:xz") as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))


class GlibcCompatTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.elf = Path(shutil.which("ls")).read_bytes()
        self.host = max(checker.glibc_requirements(Path(shutil.which("ls"))), key=checker.parse)

    def test_passes_when_limit_covers_requirement(self):
        make_archive(self.tmp / "a.tar.xz", {"r/bin/emuwiz": self.elf, "r/README": b"text"})
        self.assertEqual([], checker.check(self.tmp / "a.tar.xz", self.host, ("bin/emuwiz",)))

    def test_rejects_newer_requirement(self):
        make_archive(self.tmp / "a.tar.xz", {"r/bin/emuwiz-cli": self.elf})
        problems = checker.check(self.tmp / "a.tar.xz", "2.0", ())
        self.assertEqual(1, len(problems))
        self.assertIn("r/bin/emuwiz-cli requires GLIBC_", problems[0])

    def test_rejects_missing_expected_binary(self):
        make_archive(self.tmp / "a.tar.xz", {"r/README": b"text"})
        self.assertEqual(["expected ELF not found: bin/emuwiz"],
                         checker.check(self.tmp / "a.tar.xz", "2.39", ("bin/emuwiz",)))

    def test_directory_input_and_version_ordering(self):
        (self.tmp / "bin").mkdir()
        (self.tmp / "bin/emuwiz").write_bytes(self.elf)
        self.assertEqual([], checker.check(self.tmp, self.host, ()))
        self.assertGreater(checker.parse("2.100"), checker.parse("2.39"))  # numeric, not lexical

    def test_main_exit_codes(self):
        make_archive(self.tmp / "a.tar.xz", {"r/bin/emuwiz": self.elf})
        self.assertEqual(0, checker.main([str(self.tmp / "a.tar.xz"), "--max", self.host]))
        self.assertEqual(1, checker.main([str(self.tmp / "a.tar.xz"), "--max", "2.0"]))


if __name__ == "__main__":
    unittest.main()
