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
        # The tier picks the packages (only full runs the whole workspace) and the
        # shard picks the tests; scripts/test_ci_test_shards.py proves the partition.
        self.assertEqual(validation.count("source scripts/ci-test-shard-args.sh"), 2)
        self.assertEqual(validation.count("cargo test "), 4)
        self.assertIn('cargo test "${packages[@]}" --no-fail-fast -- "${selection[@]}"', validation)
        self.assertIn('-- --list "${selection[@]}" > "$lists/$SHARD.selected"', validation)
        self.assertNotRegex(validation, r"(?m)^    if:")
        self.assertIn("fail-fast: false", validation)
        self.assertIn('shards=$(python3 -I scripts/ci_test_shards.py shards "$tier")',
                      job_block(self.workflow, "classify"))
        # Every tier's tests run on the qualified ext4 TMPDIR (tmpfs /tmp on
        # ubuntu-26.04 cannot host ext4-only publication).
        ext4 = re.search(r"(?ms)^      - name: Create qualified disposable ext4.*?(?=^      - )", validation).group(0)
        self.assertNotRegex(ext4, r"(?m)^        if:")
        self.assertIn('echo "TMPDIR=$mnt" >> "$GITHUB_ENV"', ext4)
        for job in ["install-script-tests", "release-artifact", "verify-artifact", "reproducibility-build",
                    "reproducibility"]:
            with self.subTest(job=job):
                self.assertIn("if: needs.classify.outputs.tier == 'full' || github.event_name != 'pull_request'",
                              job_block(self.workflow, job))

    def test_shard_coverage_is_checked_from_every_shards_enumeration(self) -> None:
        coverage = job_block(self.workflow, "test-shard-coverage")
        self.assertIn("needs: [classify, validation]", coverage)
        self.assertIn("if: ${{ !cancelled() && needs.classify.result == 'success' }}", coverage)
        self.assertNotIn("outputs.tier", coverage)
        self.assertIn("python3 -B scripts/test_ci_test_shards.py", coverage)
        self.assertIn('python3 -I scripts/ci_test_shards.py check "$RUNNER_TEMP/test-shard-lists"', coverage)
        self.assertIn("SHARDS: ${{ needs.classify.outputs.shards }}", coverage)

    def test_reproducibility_builds_twice_independently_and_compares_bytes(self) -> None:
        build = job_block(self.workflow, "reproducibility-build")
        self.assertIn("run: [1, 2]", build)
        self.assertIn('scripts/compare-release-builds.sh --run ${{ matrix.run }} --output-dir', build)
        self.assertNotIn("rust-cache", build)  # clean builds only
        compare = job_block(self.workflow, "reproducibility")
        self.assertIn("needs: [classify, reproducibility-build]", compare)
        self.assertIn('scripts/compare-release-builds.sh --archives "${first[0]}" "${second[0]}"', compare)
        self.assertIn("scripts/release/test_release_reproducibility.py", compare)

    def test_release_artifact_does_not_wait_for_installer_tests(self) -> None:
        release = job_block(self.workflow, "release-artifact")
        self.assertIn("needs: [classify, security]", release)
        self.assertNotIn("install-script-tests", re.search(r"(?m)^    needs:.*$", release).group(0))


if __name__ == "__main__":
    unittest.main()
