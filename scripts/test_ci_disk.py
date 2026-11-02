#!/usr/bin/env python3
"""Tests for scripts/ci-disk.sh with fake df and sudo: nothing is deleted for real."""
from __future__ import annotations

import os
import pathlib
import re
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "ci-disk.sh"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"

# Free space grows by the amount each fake removal reclaims.
FAKE_DF = """#!/bin/bash
if [[ "$1" == --output=avail ]]; then
  echo Avail; echo "$(cat "$STATE/avail")M"
else
  echo "Filesystem Size Used Avail Use% Mounted on"
fi
"""
FAKE_SUDO = """#!/bin/bash
printf '%s\\n' "$*" >> "$STATE/sudo.log"
echo $(($(cat "$STATE/avail") + GAIN_MIB)) > "$STATE/avail"
"""


class DiskScript(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="emuwiz-ci-disk-")
        self.state = pathlib.Path(self.tmp.name)
        bin_dir = self.state / "bin"
        bin_dir.mkdir()
        for name, text in (("df", FAKE_DF), ("sudo", FAKE_SUDO)):
            (bin_dir / name).write_text(text)
            (bin_dir / name).chmod(0o755)
        self.bin = bin_dir
        self.summary = self.state / "summary"

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_script(self, *args: str, avail_gib: float, gain_gib: float = 5,
                   cwd: pathlib.Path | None = None) -> subprocess.CompletedProcess[str]:
        (self.state / "avail").write_text(str(int(avail_gib * 1024)))
        env = {"PATH": f"{self.bin}:/usr/bin:/bin", "STATE": str(self.state),
               "GAIN_MIB": str(int(gain_gib * 1024)), "GITHUB_STEP_SUMMARY": str(self.summary)}
        return subprocess.run(["bash", str(SCRIPT), *args], cwd=cwd or self.state, env=env,
                              capture_output=True, text=True, check=False)

    def removed(self) -> list[str]:
        log = self.state / "sudo.log"
        return log.read_text().splitlines() if log.exists() else []

    def test_a_roomy_runner_removes_nothing(self) -> None:
        result = self.run_script("ensure", "30", avail_gib=88)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.removed(), [])
        self.assertIn("nothing to remove", result.stdout)

    def test_exactly_enough_removes_nothing(self) -> None:
        self.assertEqual(self.run_script("ensure", "30", avail_gib=30).returncode, 0)
        self.assertEqual(self.removed(), [])

    def test_a_small_runner_stops_removing_once_there_is_enough(self) -> None:
        result = self.run_script("ensure", "30", avail_gib=16, gain_gib=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        # 16 + 5 + 5 + 5 reaches 30: the slowest removal, android, is never reached.
        self.assertEqual(self.removed(), ["rm -rf -- /usr/local/.ghcup", "rm -rf -- /usr/share/swift",
                                          "rm -rf -- /usr/share/dotnet"])
        self.assertIn("::notice title=Small CI runner::", result.stdout)

    def test_only_listed_sdk_paths_can_be_removed(self) -> None:
        text = SCRIPT.read_text()
        listed = re.search(r"(?ms)^REMOVABLE=\(\n(.*?)^\)", text).group(1).split()
        self.assertEqual(listed, ["/usr/local/.ghcup", "/usr/share/swift", "/usr/share/dotnet",
                                  "/opt/hostedtoolcache/CodeQL", "/usr/local/lib/android"])
        self.assertEqual(text.count("rm -rf"), 1)
        for forbidden in (".cargo", ".rustup", "/usr/bin", "/home", "GITHUB_WORKSPACE"):
            self.assertNotIn(forbidden, "\n".join(listed))

    def test_too_little_space_fails_as_infrastructure_before_building(self) -> None:
        result = self.run_script("ensure", "30", avail_gib=1, gain_gib=0)
        self.assertEqual(result.returncode, 1)
        self.assertIn("::error title=Runner disk (infrastructure)::", result.stdout)
        self.assertIn("not a code or test failure", result.stdout)
        self.assertIn("Runner disk (infrastructure)", self.summary.read_text())

    def test_bad_arguments_are_refused(self) -> None:
        for args in ((), ("ensure",), ("ensure", "0"), ("ensure", "ten"), ("report", "x"), ("other",)):
            with self.subTest(args=args):
                self.assertEqual(self.run_script(*args, avail_gib=88).returncode, 2)

    def test_report_is_silent_when_space_is_fine(self) -> None:
        result = self.run_script("report", avail_gib=64)
        self.assertEqual(result.returncode, 0)
        self.assertNotIn("::error", result.stdout)

    def test_report_names_exhaustion_and_keeps_cacheable_artifacts(self) -> None:
        deps = self.state / "repo" / "target" / "debug" / "deps"
        deps.mkdir(parents=True)
        executable = deps / "some_test-0123"
        executable.write_text("x")
        executable.chmod(0o755)
        rlib = deps / "libdep-0123.rlib"
        rlib.write_text("x")
        result = self.run_script("report", avail_gib=0.2, cwd=self.state / "repo")
        self.assertEqual(result.returncode, 0)
        self.assertIn("::error title=Runner disk exhausted (infrastructure)::", result.stdout)
        self.assertFalse(executable.exists())
        self.assertTrue(rlib.exists())


class WorkflowUse(unittest.TestCase):
    def test_every_shard_ensures_headroom_before_any_build_and_always_reports(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        job = re.search(r"(?ms)^  validation:\n.*?(?=^  \S)", text).group(0)
        ensure = job.index("run: scripts/ci-disk.sh ensure ")
        self.assertLess(job.index("uses: actions/checkout@v5"), ensure)
        self.assertLess(ensure, job.index("Swatinem/rust-cache"))
        self.assertLess(ensure, job.index("cargo "))
        self.assertIn("CARGO_PROFILE_DEV_DEBUG: 0", job)
        self.assertNotIn("CARGO_PROFILE_DEV_DEBUG", text.replace(job, ""))  # never a release build
        step = re.search(r"(?ms)- name: Ensure runner disk headroom.*?(?=^      - )", job).group(0)
        self.assertNotIn("if:", step)
        self.assertNotIn("continue-on-error", step)
        after = re.search(r"(?ms)- name: Disk space after tests.*?(?=^      - )", job).group(0)
        self.assertIn("if: always()", after)
        self.assertIn("scripts/ci-disk.sh report", after)


if __name__ == "__main__":
    unittest.main()
