#!/usr/bin/env python3
"""Decide the single `CI gate` result from every CI job's result.

Test shard and reproducibility check names change with the CI tier, so branch
protection cannot require them by name. This job has one stable name and fails
unless each job below finished exactly as its tier and event allow. It fails
closed: any failure, cancellation, unknown job, missing job, unknown tier or
result not explicitly allowed fails the gate.

  CI_GATE_NEEDS  the workflow's toJSON(needs)
  CI_GATE_EVENT  github.event_name
"""
from __future__ import annotations

import json
import os
import sys

# Must succeed on every event and tier.
ALWAYS = (
    "classify", "validation", "test-shard-coverage", "clippy", "r2-filesystem-gate",
    "dosbox-sandbox-enforcement", "security",
)
# Must succeed on pull requests; the job only runs there.
PULL_REQUEST_ONLY = ("repository-guardrails",)
# Must succeed on the full tier and off pull requests. May skip only on a
# classified core or gui pull request. A job skipped because something it
# needs failed is not an intended skip, and is refused on the full tier.
FULL_TIER = (
    "install-script-tests", "maintenance-script-tests", "release-artifact", "verify-artifact",
    "reproducibility-build", "reproducibility",
)
FAST_TIERS = ("core", "gui")
TIERS = (*FAST_TIERS, "full")
EVENTS = ("pull_request", "push", "workflow_dispatch")


def problems(needs: dict, event: str) -> list[str]:
    known = {*ALWAYS, *PULL_REQUEST_ONLY, *FULL_TIER}
    found = [f"unexpected job not known to the gate: {name}" for name in sorted(set(needs) - known)]
    found += [f"job missing from the gate's needs: {name}" for name in sorted(known - set(needs))]
    if found:
        return found
    if event not in EVENTS:
        return [f"unknown event: {event!r}"]
    tier = (needs["classify"].get("outputs") or {}).get("tier")
    if tier not in TIERS:
        found.append(f"unknown CI tier: {tier!r}")
    is_pull_request = event == "pull_request"
    may_skip_full = is_pull_request and tier in FAST_TIERS

    def require(names: tuple[str, ...], *allowed: str) -> None:
        for name in names:
            result = needs[name].get("result")
            if result not in allowed:
                found.append(f"{name}: {result}")

    require(ALWAYS, "success")
    require(PULL_REQUEST_ONLY, *(("success",) if is_pull_request else ("skipped",)))
    require(FULL_TIER, *(("success", "skipped") if may_skip_full else ("success",)))
    return found


def main() -> int:
    needs = json.loads(os.environ["CI_GATE_NEEDS"])
    event = os.environ["CI_GATE_EVENT"]
    for name, job in sorted(needs.items()):
        print(f"{name}: {job.get('result')}")
    found = problems(needs, event)
    tier = (needs.get("classify", {}).get("outputs") or {}).get("tier")
    print(f"event: {event}, tier: {tier}")
    if found:
        print("::error title=CI gate::" + "; ".join(found))
        return 1
    print("CI gate passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
