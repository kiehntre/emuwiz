#!/usr/bin/env python3
"""Regression tests for the Phase 3 acceptance findings (F1 byte cap, F2/F3 worktree
gate and receipt provenance, advisory admission, policy validation, ResourceWarning).

Synthetic only: private HOME/registry/targets under a temp directory.  Each test is
the reproduction of a defect from the independent acceptance report; it fails on
commit 17ccb480 and passes on the corrective commit.
"""
import gc
import hashlib
import json
import os
import shutil
import subprocess
import sys
import unittest
import warnings

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import test_emuwiz_cargo_maintain as M  # noqa: E402


def rj(path):
    with open(path) as handle:
        return json.load(handle)


def wj(path, value):
    with open(path, "w") as handle:
        json.dump(value, handle)


def rt(path):
    with open(path) as handle:
        return handle.read()


def wt(path, text):
    with open(path, "w") as handle:
        handle.write(text)


class Base(M.MaintBase):
    def setUp(self):
        super().setUp()
        self.full_census()

    def released(self, name, age_days=10, extra_bytes=0):
        wt, t = self.cache(name, age_days=age_days)
        if extra_bytes:
            with open(os.path.join(t, "debug", "deps", "extra.rlib"), "wb") as handle:
                handle.write(b"x" * extra_bytes)
            self.age(t, age_days)
        self.finish(wt)
        self.age(t, age_days)
        self.age_record(t, age_days)
        return wt, t

    def record_path(self, t):
        return os.path.join(self.reg, "targets", self.key_of(t) + ".json")

    def record(self, t):
        return rj(self.record_path(t))

    def write_record(self, t, rec):
        wj(self.record_path(t), rec)
        os.chmod(self.record_path(t), 0o600)

    def limits(self, max_bytes=None, max_targets=None, enable=True):
        """Approve a policy (disabled), set exact byte limits, then enable it."""
        self.approve(enable=False)
        pol = rj(self.m.policy_path())
        if max_bytes is not None:
            pol["limits"]["max_bytes_per_cycle"] = max_bytes
        if max_targets is not None:
            pol["limits"]["max_targets_per_cycle"] = max_targets
        pol["digest"] = self.m.policy_digest(pol)
        wj(self.m.policy_path(), pol)
        os.chmod(self.m.policy_path(), 0o600)
        if enable:
            rc, out = self.call("policy", "enable")
            self.assertEqual(rc, 0, out)

    def plan(self):
        rc, out = self.call("plan", "--json")
        self.assertEqual(rc, 0, out)
        return json.loads(out)

    def names(self, plan):
        return [s["name"] for s in plan["selected"]]

    def delete_artifacts(self, t):
        for sel in self.m.F.select_artifacts(t, None)[0]:
            (shutil.rmtree if sel["type"] == "dir" else os.remove)(sel["path"])

    def retire(self, wt, t):
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))
        self.approve()
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "RETIRED")
        self.assertEqual(self.m.worktree_gate(wt)["status"], "CLEAR")
        return self.last_cycle()


class ByteCap(Base):
    def test_cap_equal_to_estimate_selects_and_one_byte_less_does_not(self):
        wt, t = self.released("one")
        self.limits(max_bytes=10**12)
        est = self.plan()["selected"][0]["estimated_recoverable"]
        self.assertGreater(est, 0)
        self.limits(max_bytes=est)
        self.assertEqual(self.names(self.plan()), [os.path.basename(t)])
        self.limits(max_bytes=est - 1)
        plan = self.plan()
        self.assertEqual(plan["selected"], [])
        self.assertTrue(any(s["reason"] == "BYTE_CAP_WOULD_BE_EXCEEDED" for s in plan["skipped"]))
        # and a real cycle under the too-small cap deletes nothing
        self.call("run")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_multi_target_total_never_exceeds_cap_and_stays_oldest_first(self):
        ts = [self.released(f"m{i}", age_days=10 + i)[1] for i in range(3)]   # m2 is the oldest
        self.limits(max_bytes=10**12)
        ests = {s["name"]: s["estimated_recoverable"] for s in self.plan()["selected"]}
        e = max(ests.values())
        for cap, expect in ((2 * e, 2), (3 * e - 1, 2), (e - 1, 0), (3 * e, 3)):
            self.limits(max_bytes=cap)
            plan = self.plan()
            self.assertEqual(len(plan["selected"]), expect, (cap, plan))
            self.assertLessEqual(plan["estimated_total"], cap)
        self.limits(max_bytes=2 * e)
        self.assertEqual(self.names(self.plan()),
                         [os.path.basename(ts[2]), os.path.basename(ts[1])])

    def test_oversized_oldest_is_skipped_but_never_bypasses_the_cap(self):
        _, big = self.released("big", age_days=30, extra_bytes=400_000)
        _, small = self.released("small", age_days=12)
        self.limits(max_bytes=10**12)
        ests = {s["name"]: s["estimated_recoverable"] for s in self.plan()["selected"]}
        cap = ests[os.path.basename(small)]
        self.assertGreater(ests[os.path.basename(big)], cap)
        self.limits(max_bytes=cap)
        plan = self.plan()
        self.assertEqual(self.names(plan), [os.path.basename(small)])
        self.assertLessEqual(plan["estimated_total"], cap)
        self.call("run")
        self.assertTrue(os.path.exists(os.path.join(big, "debug", "deps", "extra.rlib")))


class Gate(Base):
    def test_unreleased_target_with_missing_artifacts_holds(self):
        wt, t = self.cache("unrel")
        self.delete_artifacts(t)
        g = self.m.worktree_gate(wt)
        self.assertEqual(g["status"], "HOLD", g)

    def test_released_target_with_missing_artifacts_and_no_receipt_holds(self):
        wt, t = self.released("rel-nogate")
        self.delete_artifacts(t)
        g = self.m.worktree_gate(wt)
        self.assertEqual(g["status"], "HOLD", g)

    def test_built_target_whose_directory_vanished_holds(self):
        wt, t = self.cache("vanished")
        shutil.rmtree(t)
        self.assertEqual(self.m.worktree_gate(wt)["status"], "HOLD")

    def test_never_created_is_clear_and_record_without_build_is_clear(self):
        self.assertEqual(self.m.worktree_gate(self.worktree("never"))["status"], "CLEAR")
        wt, t = self.cache("rec")
        shutil.rmtree(t)
        rec = self.record(t)
        for k in ("last_start", "last_end", "last_use", "last_exit", "last_status", "active"):
            rec.pop(k, None)
        rec["active"] = []
        self.write_record(t, rec)
        self.assertEqual(self.m.worktree_gate(wt)["status"], "CLEAR")

    def test_contradictory_record_holds(self):
        wt, t = self.cache("contra")
        rec = self.record(t)
        rec["last_retired"] = {"at": 1, "bytes": 5, "receipt": "/nonexistent"}
        self.write_record(t, rec)
        # artifacts still present AND a retirement is claimed: never CLEAR
        self.assertEqual(self.m.worktree_gate(wt)["status"], "HOLD")
        self.delete_artifacts(t)
        self.assertEqual(self.m.worktree_gate(wt)["code"], "RECEIPT_UNVERIFIED")


class Provenance(Base):
    def setUp(self):
        super().setUp()
        self.wt, self.t = self.released("prov")
        self.cyc = self.retire(self.wt, self.t)
        self.good = self.record(self.t)

    def hold(self, why):
        g = self.m.worktree_gate(self.wt)
        self.assertEqual(g["status"], "HOLD", (why, g))
        return g

    def test_original_forged_pair_outside_the_receipts_dir_is_refused(self):
        rf = os.path.join(self.tmp, "fake-receipt.json")
        jf = os.path.join(self.tmp, "fake.journal.jsonl")
        wj(rf, {"groups": [{"directory": self.t, "outcome": "RETIRED"}], "journal_file": jf})
        wt(jf, '{"event":"end"}\n')
        rec = dict(self.good)
        rec["last_retired"] = {"at": 1, "bytes": 1, "receipt": rf}
        self.write_record(self.t, rec)
        self.hold("forged pair")

    def test_genuine_receipt_is_bound_to_this_target_and_manifest(self):
        self.assertEqual(self.m.worktree_gate(self.wt)["status"], "CLEAR")
        rec = dict(self.good)
        rec["last_retired"] = dict(rec["last_retired"], manifest_sha256="0" * 64)
        self.write_record(self.t, rec)
        self.hold("manifest identity")

    def test_receipt_content_tamper_holds(self):
        path = self.cyc["receipt"]
        data = rj(path)
        data["totals"]["accounted_freed_bytes"] = 1
        wj(path, data)
        os.chmod(path, 0o600)
        self.hold("receipt digest")

    def test_journal_tamper_and_missing_end_or_group_plan_hold(self):
        path = self.cyc["journal"]
        original = rt(path)
        lines = original.splitlines()
        for variant in ([l for l in lines if '"event": "end"' not in l],
                        [l for l in lines if '"event": "group_plan"' not in l]):
            wt(path, "\n".join(variant) + "\n")
            self.hold("journal edit")
        wt(path, original)
        self.assertEqual(self.m.worktree_gate(self.wt)["status"], "CLEAR")

    def test_symlink_substitution_and_wrong_permissions_hold(self):
        path = self.cyc["receipt"]
        os.chmod(path, 0o666)
        self.hold("group/world writable receipt")
        os.chmod(path, 0o600)
        moved = path + ".real"
        os.rename(path, moved)
        os.symlink(moved, path)
        self.hold("symlinked receipt")
        os.unlink(path)
        os.rename(moved, path)
        self.assertEqual(self.m.worktree_gate(self.wt)["status"], "CLEAR")
        rdir = os.path.dirname(path)
        os.rename(rdir, rdir + ".real")
        os.symlink(rdir + ".real", rdir)
        self.hold("symlinked receipts dir")

    def test_directory_replaced_after_retirement_holds(self):
        os.rename(self.t, self.t + ".old")      # keep the old inode alive so the new one differs
        os.makedirs(os.path.join(self.t, "debug"))
        self.hold("replaced directory identity")

    def test_hand_made_pair_inside_the_receipts_dir_without_history_holds(self):
        rdir = os.path.dirname(self.cyc["receipt"])
        rf = os.path.join(rdir, "retire-execute-forged.json")
        jf = os.path.join(rdir, "retire-forged.journal.jsonl")
        data = rj(self.cyc["receipt"])
        data["manifest_sha256"] = data["approved_sha256"] = "f" * 64
        data["journal_file"] = jf
        wj(rf, data)
        wt(jf, rt(self.cyc["journal"]))
        os.chmod(rf, 0o600)
        os.chmod(jf, 0o600)
        rec = dict(self.good)
        rec["last_retired"] = {"at": 1, "bytes": 1, "receipt": rf}
        self.write_record(self.t, rec)
        self.hold("no matching cycle in history")


class Admission(Base):
    def over_budget_env(self, **extra):
        return dict(self.env, EMUWIZ_HARD_BUDGET_BYTES="1000", **extra)

    def test_over_budget_cold_build_is_advised_not_refused_by_default(self):
        self.cache("existing")
        wt = self.worktree("fresh")
        r = self.iso(wt, "check", env=self.over_budget_env())
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("advisory", r.stderr)
        self.assertIn("hard budget", r.stderr)
        self.assertTrue(os.path.isdir(self.target_of(wt)))      # the build went ahead
        events = [json.loads(l) for l in rt(os.path.join(self.reg, "maintenance", "history.jsonl")).splitlines()]
        self.assertTrue(any(e["event"] == "admission_advisory" for e in events))

    def test_admit_never_returns_the_refusal_status(self):
        self.cache("existing")
        env = dict(self.over_budget_env(), EMUWIZ_ALLOW_OVER_BUDGET="")
        for mode in ("", "advisory", "enforce", "bogus", "0", "off"):
            e = dict(env)
            if mode:
                e["EMUWIZ_ADMISSION"] = mode
            r = subprocess.run([sys.executable, "-I", M.MAINTAIN, "admit", "--target",
                                self.target_of(self.worktree("w" + mode))], env=e,
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, (mode, r.stderr))

    def test_enforce_is_unavailable_and_says_so(self):
        self.cache("existing")
        env = self.over_budget_env(EMUWIZ_ADMISSION="enforce")
        r = self.iso(self.worktree("enf"), "check", env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("enforce mode is not available", r.stderr)

    def test_malformed_negative_and_nonfinite_values_are_rejected_loudly(self):
        self.cache("existing")
        for var, val in (("EMUWIZ_COLD_TARGET_GB", "abc"), ("EMUWIZ_COLD_TARGET_GB", "-500"),
                         ("EMUWIZ_COLD_TARGET_GB", "nan"), ("EMUWIZ_COLD_TARGET_GB", "inf"),
                         ("EMUWIZ_HARD_BUDGET_BYTES", "-5"), ("EMUWIZ_HARD_BUDGET_BYTES", "inf")):
            env = dict(self.env)
            env[var] = val
            r = subprocess.run([sys.executable, "-I", M.MAINTAIN, "admit", "--target",
                                self.target_of(self.worktree("bad-" + var.lower()[6:10] + val.replace("-", "n").replace(".", "")))], env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, (var, val, r.stderr))
            self.assertIn("ignored invalid", r.stderr, (var, val))
            self.assertNotIn("Traceback", r.stderr)

    def test_disabling_admission_is_recorded(self):
        self.cache("existing")
        env = self.over_budget_env(EMUWIZ_ADMISSION="0")
        r = self.iso(self.worktree("off"), "check", env=env)
        self.assertEqual(r.returncode, 0)
        events = [json.loads(l) for l in rt(os.path.join(self.reg, "maintenance", "history.jsonl")).splitlines()]
        self.assertTrue(any(e["event"] == "admission_disabled" for e in events))

    def test_recreated_retired_cache_counts_as_cold(self):
        wt, t = self.released("retired")
        Provenance.retire(self, wt, t)
        r = self.iso(wt, "check", env=self.over_budget_env())
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("hard budget", r.stderr)      # the (now artifact-free) directory was assessed as cold

    def test_free_space_guard_is_unchanged(self):
        env = dict(self.env, EMUWIZ_MIN_FREE_GB="100000000")
        self.assertEqual(self.iso(self.worktree("nospace"), "check", env=env).returncode, 4)


class PolicyValidation(Base):
    def test_ownership_is_enforced_even_for_root(self):
        self.approve(enable=False)
        self.assertEqual(self.m.load_policy()[0], "ok")
        # a different owner than the registry's trusted uids is invalid, whatever our euid
        st, _, why = self.m.load_policy(trusted_uids={os.getuid() + 12345})
        self.assertEqual(st, "invalid", why)
        self.assertIn("owned by", why)
        self.assertNotIn("os.getuid() != 0", rt(M.MAINTAIN))

    def test_hand_edited_enabled_flag_is_not_approval(self):
        wt, t = self.released("flag")
        self.approve(enable=False)
        pol = rj(self.m.policy_path())
        pol["enabled"] = True
        with open(self.m.policy_path(), "w") as handle:
            json.dump(pol, handle)
        os.chmod(self.m.policy_path(), 0o600)
        st, loaded, _ = self.m.load_policy()
        self.assertFalse(loaded and loaded["enabled"])
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_forged_enablement_without_history_is_not_approval(self):
        wt, t = self.released("forge-enable")
        self.approve(enable=False)
        pol = self.m.load_policy()[1]
        forged = {"schema": 1, "policy_digest": pol["digest"], "enabled_by": "tester",
                  "enabled_at": 1.0, "uid": os.geteuid()}
        os.makedirs(self.m.state_dir(), exist_ok=True)
        wj(self.m.enable_path(), forged)
        os.chmod(self.m.enable_path(), 0o600)
        self.assertFalse(self.m.load_policy()[1]["enabled"])
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")

    def test_reapproval_requires_a_fresh_enable(self):
        self.approve(enable=True)
        self.assertTrue(self.m.load_policy()[1]["enabled"])
        self.approve(enable=False, owners="tester,other")
        self.assertFalse(self.m.load_policy()[1]["enabled"])

    def test_missing_fields_fail_closed_cleanly(self):
        wt, t = self.released("fields")
        self.approve()
        base = rj(self.m.policy_path())
        for mutate in (lambda p: p["scope"].pop("kinds"),
                       lambda p: p["scope"].update(kinds="per-worktree"),
                       lambda p: p["limits"].pop("max_bytes_per_cycle"),
                       lambda p: p["limits"].update(max_targets_per_cycle=-1),
                       lambda p: p["limits"].update(soft_budget_bytes=10**13),
                       lambda p: p.pop("scope")):
            pol = json.loads(json.dumps(base))
            mutate(pol)
            try:
                pol["digest"] = self.m.policy_digest(pol)
            except KeyError:
                pass
            wj(self.m.policy_path(), pol)
            os.chmod(self.m.policy_path(), 0o600)
            self.assertEqual(self.m.load_policy()[0], "invalid")
            rc, out = self.call("run")
            self.assertEqual(rc, 1, out)
            self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_manifest_checksum_is_never_authority(self):
        wt, t = self.released("manifest")
        self.limits()
        pol = self.m.load_policy()[1]
        ns, items, census, measured, s = self.m.snapshot(pol)
        plan = self.m.plan_cycle(pol, items, s, census)
        manifest = self.m.build_manifest(pol, plan, measured)
        # a self-consistent manifest naming another policy digest or an out-of-scope dir
        bad = json.loads(json.dumps(manifest))
        bad["authorisation"]["policy_digest"] = "0" * 64
        with self.assertRaises(SystemExit):
            self.m.verify_manifest_against_policy(bad, pol, plan)
        bad = json.loads(json.dumps(manifest))
        bad["cargo_artifact_groups"][0]["directory"] = os.path.join(self.tmp, "elsewhere")
        with self.assertRaises(SystemExit):
            self.m.verify_manifest_against_policy(bad, pol, plan)
        extra = json.loads(json.dumps(manifest))
        extra["cargo_artifact_groups"].append(dict(extra["cargo_artifact_groups"][0],
                                                   directory=os.path.join(os.path.dirname(t), "not-planned")))
        with self.assertRaises(SystemExit):
            self.m.verify_manifest_against_policy(extra, pol, plan)


class PolicyState(Base):
    """The persistent enablement state machine: one authoritative state record with
    monotonic generations, cross-checked against the append-only history."""

    def enabled(self):
        st, pol, _ = self.m.load_policy()
        return st == "ok" and bool(pol["enabled"])

    def events(self):
        return [json.loads(l) for l in rt(os.path.join(self.reg, "maintenance", "history.jsonl")).splitlines()
                if json.loads(l).get("event") in self.m.TRANSITIONS]

    def run_outcome(self):
        self.call("run")
        return self.last_cycle()["outcome"]

    def test_approve_enable_disable(self):
        _wt, t = self.released("sm1")
        self.approve(enable=False)
        self.assertFalse(self.enabled())
        self.assertEqual(self.call("policy", "enable")[0], 0)
        self.assertTrue(self.enabled())
        self.assertEqual(self.call("policy", "disable")[0], 0)
        self.assertFalse(self.enabled())
        state = rj(self.m.enable_path())
        self.assertEqual(state["state"], "DISABLED")        # persisted, not deleted
        gens = [e["generation"] for e in self.events()]
        self.assertEqual(gens, sorted(set(gens)))            # strictly increasing
        self.assertEqual([e["event"] for e in self.events()], ["policy_approved", "policy_enabled", "policy_disabled"])
        self.assertEqual(self.run_outcome(), "DISABLED")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_disable_reapprove_requires_explicit_enable(self):
        self.approve(enable=True)
        self.call("policy", "disable")
        self.approve(enable=False, owners="tester,other")
        self.assertFalse(self.enabled())
        self.assertEqual(rj(self.m.enable_path())["state"], "DISABLED")
        self.call("policy", "enable")
        self.assertTrue(self.enabled())
        # re-approval of an ENABLED policy also disables it again
        self.approve(enable=False, owners="tester,third")
        self.assertFalse(self.enabled())

    def test_old_approval_cannot_resurrect_an_enabled_policy(self):
        self.approve(enable=True)
        old_state = rt(self.m.enable_path())
        old_policy = rt(self.m.policy_path())
        self.call("policy", "disable")
        wt(self.m.enable_path(), old_state)                 # restore the stale ENABLED record
        os.chmod(self.m.enable_path(), 0o600)
        self.assertFalse(self.enabled())
        self.call("policy", "enable")                       # legitimate re-enable at a new generation
        self.assertTrue(self.enabled())
        newer = rt(self.m.enable_path())
        self.call("policy", "disable")
        self.approve(enable=False, owners="tester,other")
        for stale in (old_state, newer):
            wt(self.m.enable_path(), stale)
            os.chmod(self.m.enable_path(), 0o600)
            self.assertFalse(self.enabled())
        wt(self.m.policy_path(), old_policy)                 # old policy file restored too
        os.chmod(self.m.policy_path(), 0o600)
        wt(self.m.enable_path(), old_state)
        os.chmod(self.m.enable_path(), 0o600)
        self.assertFalse(self.enabled())

    def test_interrupted_transitions_stay_disabled(self):
        from unittest import mock
        _wt, t = self.released("sm-int")
        self.approve(enable=False)
        # enable interrupted after the state record, before the history event
        with mock.patch.object(self.m, "append_history", side_effect=OSError("disk gone")):
            with self.assertRaises(OSError):
                self.m.main(["policy", "enable"])
        self.assertFalse(self.enabled())
        self.assertEqual(self.run_outcome(), "DISABLED")
        self.call("policy", "enable")
        self.assertTrue(self.enabled())
        # disable interrupted after the history event, before the state record: still disabled
        with mock.patch.object(self.m, "write_state", side_effect=OSError("disk gone")):
            with self.assertRaises(OSError):
                self.m.main(["policy", "disable"])
        self.assertEqual(rj(self.m.enable_path())["state"], "ENABLED")      # stale record
        self.assertFalse(self.enabled())
        self.assertEqual(self.run_outcome(), "DISABLED")
        # approve interrupted between the transition and the policy write: new generation is DISABLED
        self.call("policy", "enable")
        self.assertTrue(self.enabled())
        with mock.patch.object(self.m, "write_policy", side_effect=OSError("disk gone")):
            with self.assertRaises(OSError):
                self.m.main(["policy", "approve", "--owners", "tester"])
        self.assertFalse(self.enabled())
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_missing_malformed_or_tampered_records_refuse_retirement(self):
        _wt, t = self.released("sm-bad")
        self.approve(enable=True)
        good = rt(self.m.enable_path())
        hist_path = os.path.join(self.reg, "maintenance", "history.jsonl")
        hist = rt(hist_path)
        enable = rj(self.m.enable_path())

        def check(why):
            self.assertFalse(self.enabled(), why)
            self.assertEqual(self.run_outcome(), "DISABLED", why)
            self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")), why)

        os.remove(self.m.enable_path())
        check("missing state record")
        for why, text in (("garbage", "{not json"), ("array", "[]"),
                          ("old schema", json.dumps(dict(enable, schema=1))),
                          ("bad state", json.dumps(dict(enable, state="MAYBE"))),
                          ("bool generation", json.dumps(dict(enable, generation=True))),
                          ("other digest", json.dumps(dict(enable, policy_digest="0" * 64))),
                          ("disabled", json.dumps(dict(enable, state="DISABLED")))):
            wt(self.m.enable_path(), text)
            os.chmod(self.m.enable_path(), 0o600)
            check(why)
        wt(self.m.enable_path(), good)
        os.chmod(self.m.enable_path(), 0o666)
        check("group/world writable")
        os.chmod(self.m.enable_path(), 0o600)
        self.assertTrue(self.enabled())
        # forged generation ahead of the history
        wt(self.m.enable_path(), json.dumps(dict(enable, generation=enable["generation"] + 5)))
        os.chmod(self.m.enable_path(), 0o600)
        check("generation not matched by history")
        # symlink substitution
        real = self.m.enable_path() + ".real"
        wt(real, good)
        os.chmod(real, 0o600)
        os.remove(self.m.enable_path())
        os.symlink(real, self.m.enable_path())
        check("symlinked state record")
        os.remove(self.m.enable_path())
        wt(self.m.enable_path(), good)
        os.chmod(self.m.enable_path(), 0o600)
        self.assertTrue(self.enabled())
        # history truncated / enable event removed / newer disable appended by another process
        wt(hist_path, "\n".join(l for l in hist.splitlines() if '"policy_enabled"' not in l) + "\n")
        check("enable event missing from history")
        wt(hist_path, hist + "garbage-line\n" + json.dumps({"event": "policy_disabled", "digest": enable["policy_digest"],
                                                          "generation": enable["generation"] + 1, "at": 1.0}) + "\n")
        check("newer disable in history")

    def test_concurrent_transitions_keep_unique_generations_and_one_state(self):
        self.approve(enable=False)
        procs = [subprocess.Popen([sys.executable, "-I", M.MAINTAIN, "policy", op], env=self.env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                 for op in ("enable", "disable", "enable", "disable", "enable", "disable")]
        for p in procs:
            p.communicate()
            self.assertEqual(p.returncode, 0)
        gens = [e["generation"] for e in self.events()]
        self.assertEqual(len(gens), len(set(gens)))
        self.assertEqual(gens, sorted(gens))
        last = self.events()[-1]
        state = rj(self.m.enable_path())
        self.assertEqual(state["generation"], last["generation"])
        self.assertEqual(self.enabled(), last["event"] == "policy_enabled")

    def test_maintain_script_has_no_deletion_primitive_and_guard_test_is_unmodified(self):
        src = rt(M.MAINTAIN)
        for banned in ("rmtree", "os.remove", "os.unlink(", "os.rmdir", "shutil.rm", "unlinkat", "os.system", "rm -"):
            self.assertNotIn(banned, src)
        guard = rt(os.path.join(HERE, "test_emuwiz_cargo_maintain.py"))
        self.assertIn('for banned in ("rmtree", "os.remove", "os.unlink(", "os.rmdir", "shutil.rm", "unlinkat"):', guard)


class Hygiene(Base):
    def test_gate_does_not_leak_file_handles(self):
        wt, t = self.released("leak")
        Provenance.retire(self, wt, t)
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always", ResourceWarning)
            self.m.worktree_gate(wt)
            gc.collect()
        self.assertEqual([str(w.message) for w in caught if issubclass(w.category, ResourceWarning)], [])


if __name__ == "__main__":
    unittest.main()
