#!/usr/bin/env python3
"""Tests for the explicit `--only <target-name>` selection of emuwiz-cargo-maintain plan/run.

A batch is exactly the requested targets or nothing: unknown, ineligible, duplicate, empty,
malformed or over-cap requests refuse as a whole, never substitute an automatic target, and a
batch that changed between the dry-run and the execution is refused before any deletion.

Disposable synthetic HOME, registry and caches only.
"""
import json
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_maintain_remediation as R  # noqa: E402


class Only(R.Base):
    def caches(self, *names):
        out = {}
        for n in names:
            out[n] = self.released(n)
        return out

    def alive(self, t):
        return os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib"))

    def name(self, t):
        return os.path.basename(t)

    def plan_only(self, *targets):
        argv = ["plan", "--json"]
        for t in targets:
            argv += ["--only", os.path.basename(t) if t.startswith("/") else t]
        return self.call(*argv)

    def run_only(self, *targets):
        argv = ["run"]
        for t in targets:
            argv += ["--only", os.path.basename(t) if t.startswith("/") else t]
        return self.call(*argv)

    def receipt_files(self):
        d = os.path.join(self.m.state_dir(), "receipts")
        return sorted(os.listdir(d)) if os.path.isdir(d) else []

    # ------------------------------------------------------------------ positive
    def test_plan_selects_exactly_the_requested_targets(self):
        c = self.caches("a", "b", "c")
        self.limits(enable=False)
        rc, out = self.plan_only(c["b"][1], c["a"][1])
        self.assertEqual(rc, 0, out)
        names = sorted(s["name"] for s in json.loads(out)["selected"])
        self.assertEqual(names, sorted(self.name(c[k][1]) for k in ("a", "b")))

    def test_run_retires_only_the_requested_target(self):
        c = self.caches("a", "b", "c")
        self.limits()
        rc, out = self.run_only(c["b"][1])
        self.assertEqual(self.last_cycle()["outcome"], "RETIRED", out)
        batch = self.last_cycle()["batch"]
        self.assertEqual((batch["retired"], batch["remaining"], batch["atomic"]), ([self.name(c["b"][1])], [], False))
        self.assertIn("NOT atomic", out)
        self.assertFalse(self.alive(c["b"][1]))
        self.assertTrue(self.alive(c["a"][1]))
        self.assertTrue(self.alive(c["c"][1]))

    def test_without_only_selection_is_unchanged(self):
        c = self.caches("a", "b")
        self.limits()
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "RETIRED")
        self.assertFalse(self.alive(c["a"][1]) or self.alive(c["b"][1]))

    def test_disabled_policy_with_only_deletes_nothing(self):
        c = self.caches("a")
        self.limits(enable=False)
        self.run_only(c["a"][1])
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.assertTrue(self.alive(c["a"][1]))

    # ------------------------------------------------------------------ negative
    def assert_refused_whole(self, *requested, kept):
        rc, out = self.run_only(*requested)
        self.assertEqual(rc, 1, out)
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED", out)
        for t in kept:
            self.assertTrue(self.alive(t))
        self.assertEqual(self.receipt_files(), [])           # the executor was never entered

    def test_unknown_name_refuses_the_whole_request(self):
        c = self.caches("a", "b")
        self.limits()
        self.assert_refused_whole(c["a"][1], "no-such-target-123456789abc", kept=[c["a"][1], c["b"][1]])

    def test_an_ineligible_target_refuses_the_whole_request(self):
        c = self.caches("a")
        wt, held = self.cache("unreleased")              # task not finished
        self.limits()
        self.assert_refused_whole(c["a"][1], held, kept=[c["a"][1], held])

    def test_a_pinned_target_refuses_the_whole_request(self):
        c = self.caches("a", "p")
        self.assertEqual(self.ret("pin", c["p"][0], "--reason", "release-build").returncode, 0)
        self.limits()
        self.assert_refused_whole(c["a"][1], c["p"][1], kept=[c["a"][1], c["p"][1]])

    def test_malformed_empty_and_duplicate_selections_refuse(self):
        c = self.caches("a")
        self.limits()
        n = self.name(c["a"][1])
        for label, argv in {
            "duplicate": ["--only", n, "--only", n],
            "empty value": ["--only", ""],
            "path": ["--only", c["a"][1]],
            "traversal": ["--only", "../" + n],
            "wildcard": ["--only", "*"],
            "glob": ["--only", n[:-3] + "*"],
            "leading dash name": ["--only=-" + n],
            "space": ["--only", n + " x"],
        }.items():
            with self.subTest(label):
                rc, out = self.call("run", *argv)
                self.assertEqual(rc, 1, out)
                self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
                rc, out = self.call("plan", "--json", *argv)
                self.assertEqual(rc, 1, out)
                self.assertTrue(self.alive(c["a"][1]))
        self.assertEqual(self.receipt_files(), [])

    def test_a_batch_over_the_per_cycle_caps_refuses_whole(self):
        c = self.caches("a", "b", "c")
        self.approve(extra=("--max-targets", "2"))
        self.assert_refused_whole(c["a"][1], c["b"][1], c["c"][1], kept=[c[k][1] for k in "abc"])

    def test_the_byte_cap_refuses_whole_without_dropping_a_member(self):
        c = self.caches("a", "b")
        self.approve(extra=("--max-gb", "0.000001"))
        self.assert_refused_whole(c["a"][1], c["b"][1], kept=[c["a"][1], c["b"][1]])

    def test_under_soft_budget_refuses(self):
        c = self.caches("a")
        self.approve(enable=False, soft_gb="900", hard_gb="1000")
        self.call("policy", "enable")
        self.assert_refused_whole(c["a"][1], kept=[c["a"][1]])

    # ------------------------------------------------------------- stale selection
    def _stale(self, change, expect_refused_in=None):
        c = self.caches("a", "b")
        self.limits()
        rc, out = self.plan_only(c["a"][1], c["b"][1])
        self.assertEqual(rc, 0, out)                      # the dry-run was fine
        real = self.m.append_history
        fired = []

        def hook(event):
            real(event)
            if event.get("event") == "cycle_start" and not fired:
                fired.append(1)
                change(c)
        self.m.append_history = hook
        try:
            self.run_only(c["a"][1], c["b"][1])
        finally:
            self.m.append_history = real
        self.assertTrue(fired)
        return c

    def test_a_member_pinned_after_the_dry_run_refuses_the_whole_batch(self):
        def change(c):
            self.assertEqual(self.ret("pin", c["b"][0], "--reason", "release-build").returncode, 0)
        c = self._stale(change)
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(self.alive(c["a"][1]) and self.alive(c["b"][1]))
        self.assertEqual(self.receipt_files(), [])

    def test_a_member_rebuilt_after_planning_refuses_the_whole_batch(self):
        def change(c):
            deps = os.path.join(c["b"][1], "debug", "deps")
            os.replace(os.path.join(deps, "libx.rlib"), os.path.join(deps, "libx.rlib.old"))
            with open(os.path.join(deps, "libx.rlib"), "wb") as handle:        # new inode
                handle.write(b"rebuilt")
        c = self._stale(change)
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(self.alive(c["a"][1]))
        self.assertEqual(self.receipt_files(), [])

    def test_a_member_no_longer_eligible_before_a_second_run_never_gets_substituted(self):
        c = self.caches("a", "b", "spare")
        self.limits()
        self.assertEqual(self.ret("pin", c["b"][0], "--reason", "release-build").returncode, 0)
        self.run_only(c["a"][1], c["b"][1])
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(all(self.alive(c[k][1]) for k in ("a", "b", "spare")))   # "spare" was NOT picked instead

    def test_an_unresolved_journal_still_blocks_an_explicit_batch(self):
        c = self.caches("a")
        self.limits()
        rdir = os.path.join(self.m.state_dir(), "receipts")
        os.makedirs(rdir, exist_ok=True)
        j = os.path.join(rdir, "retire-20260101T000000-1-1.journal.jsonl")
        R.wt(j, "")
        os.chmod(j, 0o600)
        self.run_only(c["a"][1])
        self.assertTrue(self.alive(c["a"][1]))
        self.assertNotEqual(self.last_cycle()["outcome"], "RETIRED")
        self.assertTrue(os.path.exists(j))


if __name__ == "__main__":
    unittest.main()
