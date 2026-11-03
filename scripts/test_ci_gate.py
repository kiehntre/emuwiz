#!/usr/bin/env python3
"""Tests for scripts/ci_gate.py and for the CI gate job that runs it."""
from __future__ import annotations

import json
import os
import pathlib
import re
import subprocess
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "ci_gate.py"
WORKFLOW = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
sys.path.insert(0, str(SCRIPT.parent))
import ci_gate  # noqa: E402

REQUIRED = (*ci_gate.ALWAYS, *ci_gate.PULL_REQUEST_ONLY)
BAD = ("failure", "cancelled", None, "neutral", "timed_out", "")


def needs(tier: str | None = "full", event: str = "pull_request") -> dict:
    """Every job as it finishes on a healthy run of this tier and event."""
    jobs = {name: {"result": "success", "outputs": {}} for name in
            (*ci_gate.ALWAYS, *ci_gate.PULL_REQUEST_ONLY, *ci_gate.FULL_TIER)}
    jobs["classify"]["outputs"] = {"tier": tier, "shards": "[]"}
    if event != "pull_request":
        jobs["repository-guardrails"]["result"] = "skipped"
    if event == "pull_request" and tier in ("core", "gui"):
        for name in ci_gate.FULL_TIER:
            jobs[name]["result"] = "skipped"
    return jobs


class Decision(unittest.TestCase):
    def test_every_healthy_run_passes(self) -> None:
        for tier, event in (("full", "pull_request"), ("core", "pull_request"), ("gui", "pull_request"),
                            ("full", "push"), ("full", "workflow_dispatch")):
            with self.subTest(tier=tier, event=event):
                self.assertEqual(ci_gate.problems(needs(tier, event), event), [])

    def test_a_required_job_that_does_not_succeed_fails_on_every_tier(self) -> None:
        for tier in ci_gate.TIERS:
            for name in REQUIRED:
                for result in (*BAD, "skipped"):
                    with self.subTest(tier=tier, job=name, result=result):
                        jobs = needs(tier)
                        jobs[name]["result"] = result
                        self.assertTrue(ci_gate.problems(jobs, "pull_request"))

    def test_a_full_tier_job_failing_or_cancelled_fails_even_where_it_may_skip(self) -> None:
        for tier in ci_gate.TIERS:
            for name in ci_gate.FULL_TIER:
                for result in BAD:
                    with self.subTest(tier=tier, job=name, result=result):
                        jobs = needs(tier)
                        jobs[name]["result"] = result
                        self.assertTrue(ci_gate.problems(jobs, "pull_request"))

    def test_a_skip_is_refused_on_the_full_tier_and_off_pull_requests(self) -> None:
        for tier, event in (("full", "pull_request"), ("full", "push"), ("gui", "push"), ("core", "workflow_dispatch")):
            for name in ci_gate.FULL_TIER:
                with self.subTest(tier=tier, event=event, job=name):
                    jobs = needs(tier, event)
                    jobs[name]["result"] = "skipped"
                    self.assertEqual(ci_gate.problems(jobs, event), [f"{name}: skipped"])

    def test_a_cascade_skip_after_a_failed_build_is_refused(self) -> None:
        jobs = needs("full")
        jobs["release-artifact"]["result"] = "failure"
        jobs["verify-artifact"]["result"] = "skipped"
        self.assertEqual(ci_gate.problems(jobs, "pull_request"),
                         ["release-artifact: failure", "verify-artifact: skipped"])

    def test_guardrails_may_only_skip_off_pull_requests(self) -> None:
        jobs = needs("full", "push")
        jobs["repository-guardrails"]["result"] = "success"
        self.assertTrue(ci_gate.problems(jobs, "push"))

    def test_a_failed_classification_fails_and_its_empty_tier_allows_no_skip(self) -> None:
        jobs = needs("gui")
        jobs["classify"] = {"result": "failure", "outputs": {}}
        found = ci_gate.problems(jobs, "pull_request")
        self.assertIn("unknown CI tier: None", found)
        self.assertIn("classify: failure", found)
        self.assertTrue(any("skipped" in item for item in found))

    def test_unknown_tier_event_or_job_set_fails(self) -> None:
        for tier in ("docs", "", None):
            self.assertTrue(ci_gate.problems(needs(tier), "pull_request"))
        self.assertTrue(ci_gate.problems(needs(), "schedule"))
        extra = needs()
        extra["new-job"] = {"result": "success"}
        self.assertEqual(ci_gate.problems(extra, "pull_request"),
                         ["unexpected job not known to the gate: new-job"])
        missing = needs()
        del missing["security"]
        self.assertEqual(ci_gate.problems(missing, "pull_request"),
                         ["job missing from the gate's needs: security"])

    def test_only_documented_jobs_can_skip_on_a_fast_tier(self) -> None:
        self.assertEqual(ci_gate.FULL_TIER, (
            "install-script-tests", "maintenance-script-tests", "release-artifact", "verify-artifact",
            "reproducibility-build", "reproducibility"))
        self.assertEqual(ci_gate.FAST_TIERS, ("core", "gui"))


class Command(unittest.TestCase):
    def run_gate(self, jobs: object, event: str = "pull_request") -> subprocess.CompletedProcess[str]:
        env = {**os.environ, "CI_GATE_NEEDS": json.dumps(jobs), "CI_GATE_EVENT": event}
        return subprocess.run([sys.executable, "-I", str(SCRIPT)], env=env, capture_output=True, text=True, check=False)

    def test_exit_status_and_named_cause(self) -> None:
        self.assertEqual(self.run_gate(needs("gui")).returncode, 0)
        jobs = needs("gui")
        jobs["validation"]["result"] = "cancelled"
        result = self.run_gate(jobs)
        self.assertEqual(result.returncode, 1)
        self.assertIn("::error title=CI gate::validation: cancelled", result.stdout)

    def test_unreadable_input_fails(self) -> None:
        env = {**os.environ, "CI_GATE_NEEDS": "not json", "CI_GATE_EVENT": "push"}
        result = subprocess.run([sys.executable, "-I", str(SCRIPT)], env=env, capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)


def workflow_jobs(workflow: str) -> dict[str, str]:
    """Each job id under `jobs:` with its body. Any two-space key is a job,
    whatever its characters (ids may use _ and capitals) or trailing comment,
    so a new job cannot hide inside the previous job's body."""
    body = workflow[workflow.index("\njobs:\n"):]
    parts = re.split(r"(?m)^  ([^\s#:][^:\n]*):[ \t]*(?:#[^\n]*)?\n", body)
    return dict(zip(parts[1::2], parts[2::2]))


class Workflow(unittest.TestCase):
    def jobs(self) -> dict[str, str]:
        return workflow_jobs(WORKFLOW)

    def test_every_job_id_form_is_found(self) -> None:
        for job_id, after in (("new-job", ""), ("new_job", ""), ("NewJob", ""), ("new-job", "  # a comment")):
            with self.subTest(job_id=job_id, after=after):
                job = f"\n  {job_id}:{after}\n    runs-on: ubuntu-latest\n\n  ci-gate:\n"
                workflow = WORKFLOW.replace("\n  ci-gate:\n", job)
                self.assertEqual(sorted(workflow_jobs(workflow)), sorted([*self.jobs(), job_id]))

    def test_the_gate_needs_exactly_every_other_job_and_the_script_knows_them(self) -> None:
        jobs = self.jobs()
        gate = jobs.pop("ci-gate")
        needed = re.search(r"(?ms)^    needs:\n((?:      - \S+\n)+)", gate).group(1).split()[1::2]
        self.assertEqual(sorted(needed), sorted(jobs))
        self.assertEqual(sorted(needed), sorted({*ci_gate.ALWAYS, *ci_gate.PULL_REQUEST_ONLY, *ci_gate.FULL_TIER}))

    def test_the_gate_always_runs_and_cannot_be_softened(self) -> None:
        gate = self.jobs()["ci-gate"]
        # A skipped required check counts as passed, so !cancelled() or success() would be wrong.
        self.assertIn("if: ${{ always() }}", gate)
        self.assertIn("name: CI gate", gate)
        self.assertIn("CI_GATE_NEEDS: ${{ toJSON(needs) }}", gate)
        self.assertIn("CI_GATE_EVENT: ${{ github.event_name }}", gate)
        self.assertNotIn("continue-on-error", gate)
        self.assertNotIn("uses: actions/cache", gate)


if __name__ == "__main__":
    unittest.main()
