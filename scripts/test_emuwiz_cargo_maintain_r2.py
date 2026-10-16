#!/usr/bin/env python3
"""R2 regression tests: strict policy-generation reconciliation, history provenance,
active-cycle policy races and tolerant budget configuration.  Synthetic fixtures only.
Each test reproduces a defect confirmed by the re-acceptance of commit 20d3950b."""
import json
import os
import shutil
import subprocess
import sys
import time
import unittest
from unittest import mock

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import test_emuwiz_cargo_maintain_remediation as R  # noqa: E402

M = R.M
rj, wj, rt, wt = R.rj, R.wj, R.rt, R.wt
LIBX = ("debug", "deps", "libx.rlib")


class Base(R.Base):
    def hist(self):
        return os.path.join(self.reg, "maintenance", "history.jsonl")

    def enabled(self):
        st, pol, _ = self.m.load_policy()
        return st == "ok" and bool(pol["enabled"])

    def put(self, path, text, mode=0o600):
        wt(path, text)
        os.chmod(path, mode)

    def intact(self, t):
        return os.path.exists(os.path.join(t, *LIBX))

    def maintain_proc(self, *args):
        return subprocess.Popen([sys.executable, "-I", M.MAINTAIN, *args], env=self.env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


class Replay(Base):
    def test_exact_replay_counterexample_stays_disabled(self):
        self.approve(enable=True)
        old_state = rt(self.m.enable_path())
        old_enabled = [l for l in rt(self.hist()).splitlines() if '"policy_enabled"' in l][-1]
        self.call("policy", "disable")
        self.put(self.hist(), rt(self.hist()) + old_enabled + "\n")      # replay the OLD enable event
        self.put(self.m.enable_path(), old_state)                      # and the OLD state record
        self.assertFalse(self.enabled())

    def test_duplicate_conflicting_out_of_order_and_malformed_generations(self):
        wt_, t = self.released("gens")
        self.approve(enable=True)
        base = rt(self.hist())
        state = rj(self.m.enable_path())
        pol = self.m.load_policy()[1]
        ev = lambda name, gen, digest=None: json.dumps({"event": name, "digest": digest or pol["digest"],
                                                         "generation": gen, "at": 1.0}) + "\n"
        top = state["generation"]
        cases = {
            "duplicate generation": base + ev("policy_enabled", top),
            "out of order lower": base + ev("policy_disabled", top - 1) + ev("policy_enabled", top),
            "conflicting same-generation disable": base + ev("policy_disabled", top),
            "malformed generation": base + json.dumps({"event": "policy_enabled", "digest": pol["digest"],
                                                        "generation": "9", "at": 1.0}) + "\n",
            "boolean generation": base + json.dumps({"event": "policy_enabled", "digest": pol["digest"],
                                                      "generation": True, "at": 1.0}) + "\n",
            "missing digest": base + json.dumps({"event": "policy_enabled", "generation": top + 1, "at": 1.0}) + "\n",
            "garbage line": base + "{torn\n",
            "unrelated final transition": base + ev("policy_approved", top + 1, "0" * 64),
        }
        for why, text in cases.items():
            self.put(self.hist(), text)
            self.put(self.m.enable_path(), json.dumps(state))
            self.assertFalse(self.enabled(), why)
            self.call("run")
            self.assertTrue(self.intact(t), why)
        # state generation not equal to the highest recorded generation
        self.put(self.hist(), base)
        self.put(self.m.enable_path(), json.dumps(dict(state, generation=top - 1)))
        self.assertFalse(self.enabled())
        self.put(self.m.enable_path(), json.dumps(state))
        self.assertTrue(self.enabled())          # control: the genuine pair is accepted

    def test_missing_transition_history_is_unreconcilable(self):
        self.approve(enable=True)
        self.put(self.hist(), "")
        self.assertFalse(self.enabled())
        os.remove(self.hist())
        self.assertFalse(self.enabled())

    def test_reapproval_heals_a_corrupt_history_without_deleting_it(self):
        self.approve(enable=True)
        self.put(self.hist(), rt(self.hist()) + "{torn\n")
        self.assertFalse(self.enabled())
        self.approve(enable=True, owners="tester,other")
        self.assertTrue(self.enabled())
        aside = [n for n in os.listdir(os.path.dirname(self.hist())) if n.startswith("history.jsonl.unreconciled-")]
        self.assertEqual(len(aside), 1)
        self.assertIn("{torn", rt(os.path.join(os.path.dirname(self.hist()), aside[0])))

    def test_disable_and_reapproval_semantics_preserved(self):
        self.approve(enable=True)
        self.assertTrue(self.enabled())
        self.call("policy", "disable")
        self.assertFalse(self.enabled())
        self.approve(enable=False, owners="tester,other")
        self.assertFalse(self.enabled())
        self.call("policy", "enable")
        self.assertTrue(self.enabled())


class HistoryProvenance(Base):
    def setUp(self):
        super().setUp()
        self.approve(enable=True)
        self.assertTrue(self.enabled())

    def test_group_world_writable_history_is_refused(self):
        for mode in (0o660, 0o606, 0o666, 0o620):
            os.chmod(self.hist(), mode)
            self.assertFalse(self.enabled(), oct(mode))
        os.chmod(self.hist(), 0o600)
        self.assertTrue(self.enabled())

    def test_symlinked_history_is_refused(self):
        real = self.hist() + ".real"
        shutil.move(self.hist(), real)
        os.symlink(real, self.hist())
        self.assertFalse(self.enabled())

    def test_history_directory_or_registry_substitution_is_refused(self):
        state_dir = os.path.dirname(self.hist())
        os.chmod(state_dir, 0o770)
        self.assertFalse(self.enabled())
        os.chmod(state_dir, 0o700)
        self.assertTrue(self.enabled())
        os.chmod(self.reg, 0o757)               # world-writable parent: refused
        self.assertFalse(self.enabled())
        os.chmod(self.reg, 0o775)               # the default (umask 002); owner-checked maintenance dir below it
        self.assertTrue(self.enabled())
        os.chmod(self.reg, 0o755)
        real = state_dir + ".real"
        shutil.move(state_dir, real)
        os.symlink(real, state_dir)
        self.assertFalse(self.enabled())

    def test_foreign_owner_is_refused_including_for_a_root_euid(self):
        # an account that is neither the effective user nor the registry owner: refused,
        # whatever the euid (no root wildcard).  Root's own files are the only extra uid.
        st, _, why = self.m.load_policy(trusted_uids={os.getuid() + 4242})
        self.assertEqual(st, "invalid", why)
        with mock.patch.object(self.m.os, "geteuid", return_value=0):
            uids = self.m.trusted_policy_uids()
        self.assertIn(0, uids)
        self.assertIn(os.getuid(), uids)         # the registry owner stays trusted
        pol = self.m.load_policy()[1]
        self.assertFalse(self.m.verify_enablement(pol, {os.getuid() + 4242}))
        self.assertTrue(self.m.verify_enablement(pol, uids))

    def test_truncated_history_is_refused_and_identity_is_stable(self):
        text = rt(self.hist())
        self.put(self.hist(), text[: len(text) - 15])
        self.assertFalse(self.enabled())
        self.put(self.hist(), text)
        self.assertTrue(self.enabled())
        # identity swap while being read: lstat/fstat mismatch => unreadable authority
        real_open = os.open
        swapped = {}

        def evil(path, flags, *a, **k):
            fd = real_open(path, flags, *a, **k)
            if path == self.hist() and not swapped:
                swapped["x"] = True
                tmp = path + ".swap"
                self.put(tmp, text)
                os.replace(tmp, path)
            return fd
        with mock.patch.object(self.m.os, "open", evil):
            self.assertFalse(self.enabled())


class CycleRaces(Base):
    def test_disable_waits_for_the_running_executor_and_is_then_effective(self):
        wt_, t = self.released("race-a")
        self.limits()
        ex = self.m.executor()
        orig = ex.main
        box = {}

        def slow(argv):
            box["proc"] = self.maintain_proc("policy", "disable")
            time.sleep(1.5)
            box["alive_during_executor"] = box["proc"].poll() is None
            return orig(argv)
        with mock.patch.object(ex, "main", slow):
            self.call("run")
        out, err = box["proc"].communicate(timeout=30)
        self.assertTrue(box["alive_during_executor"], "disable returned while the executor was running")
        self.assertEqual(box["proc"].returncode, 0, err)
        self.assertIn("waiting", err)              # the wait is reported, not silent
        self.assertEqual(self.last_cycle()["outcome"], "RETIRED")   # already-started cycle completed whole
        self.assertFalse(self.intact(t))
        self.assertFalse(self.enabled())
        # nothing new may start after disable returned
        wt2, t2 = self.released("race-b")
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.assertTrue(self.intact(t2))

    def test_replacement_policy_waits_and_invalidates_the_old_authorisation(self):
        wt_, t = self.released("race-c")
        self.limits()
        ex = self.m.executor()
        orig = ex.main
        box = {}

        def slow(argv):
            box["proc"] = self.maintain_proc("policy", "approve", "--owners", "tester,other")
            time.sleep(1.5)
            box["alive"] = box["proc"].poll() is None
            return orig(argv)
        with mock.patch.object(ex, "main", slow):
            self.call("run")
        box["proc"].communicate(timeout=30)
        self.assertTrue(box["alive"])
        self.assertFalse(self.enabled())           # replacement needs a fresh explicit enable
        wt2, t2 = self.released("race-d")
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.assertTrue(self.intact(t2))

    def test_policy_swapped_after_planning_is_caught_before_the_executor(self):
        wt_, t = self.released("race-e")
        self.limits()
        build = self.m.build_manifest

        def build_then_swap(policy, plan, measured):
            manifest = build(policy, plan, measured)
            pol = rj(self.m.policy_path())
            pol["scope"]["owners"] = ["someone-else"]
            pol["digest"] = self.m.policy_digest(pol)
            wj(self.m.policy_path(), pol)
            os.chmod(self.m.policy_path(), 0o600)
            return manifest
        with mock.patch.object(self.m, "build_manifest", build_then_swap):
            self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(self.intact(t))

    def test_disable_directly_after_authorisation_is_caught_by_final_revalidation(self):
        wt_, t = self.released("race-f")
        self.limits()
        build = self.m.build_manifest

        def build_then_disable_state(policy, plan, measured):
            manifest = build(policy, plan, measured)
            st = rj(self.m.enable_path())
            self.put(self.m.enable_path(), json.dumps(dict(st, state="DISABLED")))
            return manifest
        with mock.patch.object(self.m, "build_manifest", build_then_disable_state):
            self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(self.intact(t))

    def test_restart_after_a_completed_cycle_does_not_repeat_it(self):
        wt_, t = self.released("race-g")
        self.limits()
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "RETIRED")
        m2 = M.load_maintain()
        self.full_census(m2)
        self.call("run", m=m2)
        self.assertNotEqual([h for h in m2.read_history() if h.get("event") == "cycle"][-1]["outcome"], "RETIRED")
        self.assertEqual(self.m.worktree_gate(wt_)["status"], "CLEAR")


class BudgetConfiguration(Base):
    BAD = ("abc", "-1", "0", "1e3", "inf", "nan", " ", "99999999999999999999999", "12.5")

    def test_malformed_budget_never_blocks_builds_or_finish_task(self):
        wt_, t = self.cache("budget-warm")
        cold = self.worktree("budget-cold")
        for var in ("EMUWIZ_HARD_BUDGET_BYTES", "EMUWIZ_SOFT_BUDGET_BYTES"):
            for val in self.BAD:
                env = dict(self.env)
                env[var] = val
                r = self.iso(wt_, "check", env=env)
                self.assertEqual(r.returncode, 0, (var, val, r.stderr[-300:]))
                self.assertNotIn("Traceback", r.stderr)
                r = self.iso(wt_, "--finish-task", env=env)
                self.assertEqual(r.returncode, 0, (var, val, r.stderr[-300:]))
                if val.strip():
                    self.assertIn(var, r.stderr)           # warned, with the variable named
        r = self.iso(cold, "check", env=dict(self.env, EMUWIZ_HARD_BUDGET_BYTES="abc"))
        self.assertEqual(r.returncode, 0, r.stderr[-300:])

    def test_valid_values_still_apply_and_the_free_space_guard_is_unchanged(self):
        wt_, t = self.cache("budget-ok")
        env = dict(self.env, EMUWIZ_HARD_BUDGET_BYTES="1000000", EMUWIZ_SOFT_BUDGET_BYTES="500000")
        self.assertEqual(self.iso(wt_, "check", env=env).returncode, 0)
        env = dict(self.env, EMUWIZ_HARD_BUDGET_BYTES="abc", EMUWIZ_MIN_FREE_GB="100000000")
        self.assertEqual(self.iso(self.worktree("nospace2"), "check", env=env).returncode, 4)

    def test_automatic_retirement_uses_only_the_validated_policy_limits(self):
        wt_, t = self.released("budget-auto")
        self.limits()
        old = dict(os.environ)
        try:
            os.environ["EMUWIZ_HARD_BUDGET_BYTES"] = "abc"
            os.environ["EMUWIZ_SOFT_BUDGET_BYTES"] = "999999999999999"
            ns = self.m.make_args(self.m.load_policy()[1])
            self.assertEqual(ns.soft_budget, 10_000)         # policy value from approve(soft_gb=0.00001)
            self.call("run")
        finally:
            os.environ.clear()
            os.environ.update(old)
        self.assertEqual(self.last_cycle()["outcome"], "RETIRED")


if __name__ == "__main__":
    unittest.main()
