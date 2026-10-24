#!/usr/bin/env python3
"""Tests for the Clippy warning ratchet and the CI conditions that run it."""
from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "clippy_ratchet.py"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"


def warning(code: str, file: str, line: int, message: str) -> str:
    return json.dumps({"reason": "compiler-message", "message": {
        "level": "warning", "message": message, "code": {"code": code},
        "spans": [{"is_primary": True, "file_name": file, "line_start": line, "column_start": 5}]}})


DEAD = ("dead_code", "src/a.rs", "constant `X` is never used")
COLLAPSE = ("clippy::collapsible_if", "src/b.rs", "this `if` statement can be collapsed")


def job_block(workflow: str, job: str) -> str:
    """The text of one top-level job, by its two-space-indented key."""
    match = re.search(rf"^  {re.escape(job)}:\n(.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)", workflow, re.M | re.S)
    if match is None:
        raise AssertionError(f"job {job} not found in {WORKFLOW}")
    return match.group(1)


class RatchetTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="emuwiz-clippy-ratchet-")
        self.root = pathlib.Path(self.tmp.name)
        self.baseline = self.root / "baseline"

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def clippy(self, *lines: str) -> pathlib.Path:
        path = self.root / f"clippy{len(list(self.root.iterdir()))}.json"
        path.write_text("\n".join([*lines, '{"reason":"build-finished","success":true}']) + "\n")
        return path

    def run_tool(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([sys.executable, "-I", str(SCRIPT), *map(str, args)],
                              capture_output=True, text=True, check=False)

    def initial(self, *lines: str) -> None:
        result = self.run_tool("write", self.clippy(*lines), self.baseline, "--initial")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_baseline_warnings_pass_even_when_lines_move(self) -> None:
        self.initial(warning(*DEAD[:2], 10, DEAD[2]), warning(*COLLAPSE[:2], 20, COLLAPSE[2]))
        moved = self.clippy(warning(*DEAD[:2], 99, DEAD[2]), warning(*COLLAPSE[:2], 120, COLLAPSE[2]))
        result = self.run_tool("check", moved, self.baseline)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Clippy warnings: 2 (baseline 2)", result.stdout)

    def test_duplicate_reports_of_one_warning_count_once(self) -> None:
        self.initial(warning(*DEAD[:2], 10, DEAD[2]))
        twice = self.clippy(warning(*DEAD[:2], 10, DEAD[2]), warning(*DEAD[:2], 10, DEAD[2]))
        self.assertEqual(self.run_tool("count", twice).stdout.strip(), "1")
        self.assertEqual(self.run_tool("check", twice, self.baseline).returncode, 0)

    def test_a_new_warning_fails_and_is_named(self) -> None:
        self.initial(warning(*DEAD[:2], 10, DEAD[2]))
        result = self.run_tool("check", self.clippy(
            warning(*DEAD[:2], 10, DEAD[2]), warning(*COLLAPSE[:2], 20, COLLAPSE[2])), self.baseline)
        self.assertEqual(result.returncode, 1)
        self.assertIn("src/b.rs: [clippy::collapsible_if]", result.stderr)

    def test_a_new_warning_fails_even_when_another_was_fixed(self) -> None:
        # The old count-only ratchet accepted this swap.
        self.initial(warning(*DEAD[:2], 10, DEAD[2]))
        result = self.run_tool("check", self.clippy(warning(*COLLAPSE[:2], 20, COLLAPSE[2])), self.baseline)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Clippy warnings: 1 (baseline 1)", result.stdout)

    def test_another_copy_of_a_baseline_warning_fails(self) -> None:
        self.initial(warning(*DEAD[:2], 10, DEAD[2]))
        result = self.run_tool("check", self.clippy(
            warning(*DEAD[:2], 10, DEAD[2]), warning(*DEAD[:2], 30, DEAD[2])), self.baseline)
        self.assertEqual(result.returncode, 1)

    def test_fixed_warnings_pass_and_write_lowers_the_baseline(self) -> None:
        self.initial(warning(*DEAD[:2], 10, DEAD[2]), warning(*COLLAPSE[:2], 20, COLLAPSE[2]))
        fixed = self.clippy(warning(*DEAD[:2], 10, DEAD[2]))
        result = self.run_tool("check", fixed, self.baseline)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("1 baseline warning(s) fixed", result.stdout)
        self.assertEqual(self.run_tool("write", fixed, self.baseline).returncode, 0)
        self.assertEqual(self.run_tool("check", self.clippy(
            warning(*DEAD[:2], 10, DEAD[2]), warning(*COLLAPSE[:2], 20, COLLAPSE[2])), self.baseline).returncode, 1)

    def test_write_refuses_to_add_warnings(self) -> None:
        self.initial(warning(*DEAD[:2], 10, DEAD[2]))
        before = self.baseline.read_text()
        result = self.run_tool("write", self.clippy(
            warning(*DEAD[:2], 10, DEAD[2]), warning(*COLLAPSE[:2], 20, COLLAPSE[2])), self.baseline)
        self.assertEqual(result.returncode, 1)
        self.assertIn("refusing to add", result.stderr)
        self.assertEqual(self.baseline.read_text(), before)

    def test_initial_refuses_to_replace_an_existing_baseline(self) -> None:
        self.initial(warning(*DEAD[:2], 10, DEAD[2]))
        result = self.run_tool("write", self.clippy(warning(*COLLAPSE[:2], 20, COLLAPSE[2])),
                               self.baseline, "--initial")
        self.assertEqual(result.returncode, 1)

    def test_non_warning_messages_are_ignored(self) -> None:
        note = json.loads(warning(*DEAD[:2], 10, DEAD[2]))
        note["message"]["level"] = "note"
        self.assertEqual(self.run_tool("count", self.clippy(json.dumps(note), "not json")).stdout.strip(), "0")

    def test_the_committed_baseline_is_well_formed(self) -> None:
        result = self.run_tool("check", self.clippy(), ROOT / ".github" / "clippy-warning-baseline")
        self.assertEqual(result.returncode, 0, result.stderr)


class WorkflowConditionTests(unittest.TestCase):
    workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_ci_runs_on_every_pull_request_and_on_main(self) -> None:
        trigger = re.search(r"^on:\n(.*?)(?=^\S)", self.workflow, re.M | re.S).group(1)
        self.assertRegex(trigger, r"(?m)^  pull_request:\s*$")
        self.assertNotRegex(trigger, r"(?m)^  pull_request:\n    ")  # no branch/path filter
        self.assertRegex(trigger, r"(?m)^  push:\n    branches:\n      - main$")

    def test_clippy_ratchet_is_not_gated_on_tier_or_event(self) -> None:
        clippy = job_block(self.workflow, "clippy")
        self.assertNotRegex(clippy, r"(?m)^    if:")
        self.assertNotIn("tier", clippy)
        self.assertNotIn("github.event_name", clippy)
        self.assertIn("run: scripts/clippy-ratchet.sh", clippy)
        self.assertIn("scripts/test_clippy_ratchet.py", clippy)

    def test_full_workspace_tests_stay_tier_gated(self) -> None:
        validation = job_block(self.workflow, "validation")
        self.assertIn("CI_TIER: ${{ needs.classify.outputs.tier }}", validation)
        # Only the full tier runs the whole workspace; gui/core PRs stay narrow.
        self.assertIn('case "$CI_TIER/$SHARD" in', validation)
        self.assertEqual(validation.count("cargo test --workspace"), 1)
        self.assertIn('full/dat|full/rest) cargo test --workspace --no-fail-fast -- "${selection[@]}" ;;',
                      validation)
        self.assertIn("gui/all) cargo test -p archivefs-gui --no-fail-fast ;;", validation)
        self.assertNotRegex(validation, r"(?m)^    if:")
        self.assertIn("fail-fast: false", validation)
        self.assertIn("*) shards='[\"dat\",\"rest\"]' ;;", job_block(self.workflow, "classify"))
        self.assertIn("gui) shards='[\"all\"]' ;;", job_block(self.workflow, "classify"))
        # Every tier's tests run on the qualified ext4 TMPDIR (tmpfs /tmp on
        # ubuntu-26.04 cannot host ext4-only publication).
        ext4 = re.search(r"(?ms)^      - name: Create qualified disposable ext4.*?(?=^      - )", validation).group(0)
        self.assertNotRegex(ext4, r"(?m)^        if:")
        self.assertIn('echo "TMPDIR=$mnt" >> "$GITHUB_ENV"', ext4)
        for job in ["install-script-tests", "release-artifact", "verify-artifact", "reproducibility"]:
            with self.subTest(job=job):
                self.assertIn("if: needs.classify.outputs.tier == 'full' || github.event_name != 'pull_request'",
                              job_block(self.workflow, job))

    def shard_commands(self, tier: str, shard: str) -> list[list[str]]:
        """Run the real test step with a stub cargo; return each cargo argv."""
        validation = job_block(self.workflow, "validation")
        filters = re.search(r'(?m)^      DAT_SHARD_FILTERS: "(.*)"$', validation).group(1)
        step = re.search(r"(?ms)^      - name: Run tests selected for the change\n.*?^        run: \|\n(.*?)(?=^      - )",
                         validation).group(1)
        with tempfile.TemporaryDirectory(prefix="emuwiz-ci-shard-") as tmp:
            log = pathlib.Path(tmp) / "argv"
            stub = pathlib.Path(tmp) / "cargo"
            stub.write_text('#!/bin/sh\nprintf "%s\\037" "$@" >> "$ARGV_LOG"\necho >> "$ARGV_LOG"\n')
            stub.chmod(0o755)
            env = {"PATH": f"{tmp}:/usr/bin:/bin", "ARGV_LOG": str(log), "CI_TIER": tier, "SHARD": shard,
                   "DAT_SHARD_FILTERS": filters}
            result = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", step], env=env,
                                    capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            return [line.split("\x1f")[:-1] for line in log.read_text().splitlines()]

    def test_sharded_tiers_run_every_test_exactly_once(self) -> None:
        for tier, package in [("full", ["--workspace"]), ("core", ["-p", "archivefs-core"])]:
            with self.subTest(tier=tier):
                dat = self.shard_commands(tier, "dat")[0]
                rest = self.shard_commands(tier, "rest")[0]
                # Same package selection (so the same unified features) on both shards.
                prefix = ["test", *package, "--no-fail-fast", "--"]
                self.assertEqual(dat[:len(prefix)], prefix)
                self.assertEqual(rest[:len(prefix)], prefix)
                filters, skips = dat[len(prefix):], rest[len(prefix):]
                self.assertIn("dat::", filters)
                # libtest: positional filters and --skip are both substring matches,
                # so "match any filter" and "skip every filter" partition all tests.
                self.assertEqual(skips, [arg for f in filters for arg in ("--skip", f)])
        self.assertEqual(self.shard_commands("core", "rest")[1],
                         ["check", "-p", "archivefs-cli", "-p", "archivefs-gui", "--locked"])
        self.assertEqual(self.shard_commands("gui", "all"),
                         [["test", "-p", "archivefs-gui", "--no-fail-fast"]])

    def test_unknown_tier_or_shard_fails(self) -> None:
        for tier, shard in [("full", "all"), ("gui", "dat"), ("full", "other"), ("other", "rest")]:
            with self.subTest(tier=tier, shard=shard), self.assertRaises(AssertionError):
                self.shard_commands(tier, shard)

    def test_release_artifact_does_not_wait_for_installer_tests(self) -> None:
        release = job_block(self.workflow, "release-artifact")
        self.assertIn("needs: [classify, security]", release)
        self.assertNotIn("install-script-tests", re.search(r"(?m)^    needs:.*$", release).group(0))


if __name__ == "__main__":
    unittest.main()
