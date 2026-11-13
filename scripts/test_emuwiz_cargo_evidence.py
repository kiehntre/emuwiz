#!/usr/bin/env python3
"""F1 (ignored in-tree build output) and historical retirement attestation.
Synthetic worktrees, fake cargo, private HOME/registry/targets; test-only signing keys."""
import json
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_maintain as M  # noqa: E402


class InTreeBuildOutput(M.MaintBase):
    def checkout_with_target(self, name):
        wt = self.worktree(name)
        with open(os.path.join(wt, ".gitignore"), "w") as handle:
            handle.write("target/\n__pycache__/\n.env\n")
        self.git("-C", wt, "add", ".gitignore")
        self.git("-C", wt, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "ignore")
        target = os.path.join(wt, "target")
        self.make_cache(target)
        os.unlink(os.path.join(target, "debug", "UNKNOWN.txt"))
        for rel in ("debug/emuwiz-test-env/profile/library.db", "debug/emuwiz-test-env/profile/journal.jsonl"):
            os.unlink(os.path.join(target, rel))
        with open(os.path.join(target, ".rustc_info.json"), "w") as handle:
            handle.write("{}")
        with open(os.path.join(target, "CACHEDIR.TAG"), "wb") as handle:
            handle.write(self.m.CACHEDIR_TAG_SIGNATURE + b"\n")
        with open(os.path.join(target, "debug", ".cargo-lock"), "w"):
            pass
        return wt, target

    def ignored_entries(self, wt):
        out = subprocess.run(["git", "-C", wt, "status", "--porcelain=v1", "-z", "--untracked-files=all",
                              "--ignored=matching"], capture_output=True, text=True, check=True).stdout
        return [e for e in out.split("\0") if e]

    def problem(self, wt):
        return self.m.reproducible_ignored_problem(wt, self.ignored_entries(wt))

    def test_recognised_compiler_output_and_pycache_are_reproducible(self):
        self.full_census()
        wt, _ = self.checkout_with_target("wt-f1-ok")
        os.makedirs(os.path.join(wt, "tools", "__pycache__"))
        with open(os.path.join(wt, "tools", "__pycache__", "x.cpython-312.pyc"), "wb") as handle:
            handle.write(b"\0")
        self.assertIsNone(self.problem(wt))

    def test_unknown_file_in_target_is_unverified(self):
        self.full_census()
        wt, target = self.checkout_with_target("wt-f1-unknown")
        with open(os.path.join(target, "notes.txt"), "w") as handle:
            handle.write("my notes")
        self.assertEqual(self.problem(wt)[0], "IN_TREE_BUILD_OUTPUT_UNVERIFIED")

    def test_data_in_preserved_test_env_is_unverified(self):
        self.full_census()
        wt, target = self.checkout_with_target("wt-f1-env")
        with open(os.path.join(target, "debug", "emuwiz-test-env", "profile", "library.db"), "w") as handle:
            handle.write("db")
        self.assertEqual(self.problem(wt)[0], "IN_TREE_BUILD_OUTPUT_UNVERIFIED")

    def test_symlink_in_target_is_unverified(self):
        self.full_census()
        wt, target = self.checkout_with_target("wt-f1-link")
        os.symlink(self.victim, os.path.join(target, "debug", "deps", "link"))
        self.assertEqual(self.problem(wt)[0], "IN_TREE_BUILD_OUTPUT_UNVERIFIED")

    def test_forged_cachedir_tag_is_unverified(self):
        self.full_census()
        wt, target = self.checkout_with_target("wt-f1-tag")
        with open(os.path.join(target, "CACHEDIR.TAG"), "w") as handle:
            handle.write("not a tag")
        self.assertEqual(self.problem(wt)[0], "IN_TREE_BUILD_OUTPUT_UNVERIFIED")

    def test_non_pyc_in_pycache_is_unverified(self):
        self.full_census()
        wt, _ = self.checkout_with_target("wt-f1-py")
        os.makedirs(os.path.join(wt, "__pycache__"))
        with open(os.path.join(wt, "__pycache__", "secret.txt"), "w") as handle:
            handle.write("x")
        self.assertEqual(self.problem(wt)[0], "IN_TREE_BUILD_OUTPUT_UNVERIFIED")

    def test_other_ignored_or_untracked_files_still_block(self):
        self.full_census()
        wt, _ = self.checkout_with_target("wt-f1-env2")
        with open(os.path.join(wt, ".env"), "w") as handle:
            handle.write("TOKEN=x")
        self.assertEqual(self.problem(wt)[0], "CHECKOUT_NOT_CLEAN")
        os.unlink(os.path.join(wt, ".env"))
        with open(os.path.join(wt, "new.rs"), "w") as handle:
            handle.write("fn main() {}")
        self.assertEqual(self.problem(wt)[0], "CHECKOUT_NOT_CLEAN")

    def test_incomplete_process_visibility_fails_closed(self):
        wt, _ = self.checkout_with_target("wt-f1-vis")
        base = self.m.F.Census

        class Partial(base):
            def __init__(inner, targets, **kw):
                super().__init__(targets, **kw)
                inner.unreadable_foreign = 3

        self.m.F.Census = Partial
        self.addCleanup(setattr, self.m.F, "Census", base)
        self.assertEqual(self.problem(wt)[0], "IN_TREE_BUILD_OUTPUT_UNVERIFIED")


class HistoricalAttestation(M.MaintBase):
    """A retirement executed by the manual route before signed scoped runs existed (as on
    2026-11-12): the executor wrote a receipt and journal but no `last_retired`."""

    def historical_retirement(self, name):
        self.approve(enable=False)                         # a valid policy covering this root
        wt, t = self.managed_cache(name, completed=True)
        g = self.group(wt, t)
        path, digest = self.manifest([g], name + ".json")
        saved = {k: getattr(self.mod, k) for k in ("check_scoped_run", "consume_scoped_run", "record_signed_retirements")}
        self.inner_layer_only()                            # stands in for the old, pre-gate executor
        try:
            rc, receipt = self.run_retire(path, digest, execute=True, window=True)
        finally:
            for k, v in saved.items():
                setattr(self.mod, k, v)
        self.assertEqual({x["outcome"] for x in receipt["groups"]}, {"RETIRED"}, receipt)
        os.unlink(os.path.join(t, "debug", "UNKNOWN.txt"))     # owner-reviewed stray fixture file
        evidence = os.path.join(self.tmp, name + "-approval.txt")
        with open(evidence, "w") as handle:
            handle.write(f"Owner approval quoted from the session: retire manifest {digest}\n")
        return wt, t, receipt, digest, evidence

    def attest(self, receipt, digest, evidence, *extra):
        return self.call("attest-retirement", "--receipt", receipt["receipt_file"], "--manifest-sha256", digest,
                         "--evidence", evidence, *extra)

    def gate(self, wt):
        return self.m.lifecycle_gate(wt)

    def test_attestation_turns_an_unverifiable_historical_retirement_into_evidence(self):
        self.full_census()
        wt, t, receipt, digest, evidence = self.historical_retirement("wt-att")
        self.assertEqual(self.gate(wt)["code"], "ARTIFACTS_MISSING_UNVERIFIED")
        rc, out = self.attest(receipt, digest, evidence)
        self.assertEqual(rc, 0, out)
        g = self.gate(wt)
        self.assertEqual((g["status"], g["code"]), ("CLEAR", "CLEAR"), g)
        self.assertIn("historical owner attestation", g["explanation"])
        # original receipt untouched, attestation is evidence only
        att = json.load(open(out.split(": ")[-1].strip()))
        self.assertEqual(att["grants"], "RETIREMENT_EVIDENCE_ONLY")
        self.assertEqual(att["historical"]["finished"], receipt["finished"])

    def test_repeat_attestation_is_refused(self):
        self.full_census()
        wt, t, receipt, digest, evidence = self.historical_retirement("wt-att2")
        self.assertEqual(self.attest(receipt, digest, evidence)[0], 0)
        rc, out = self.attest(receipt, digest, evidence)
        self.assertNotEqual(rc, 0)

    def test_wrong_digest_or_evidence_without_the_digest_is_refused(self):
        self.full_census()
        wt, t, receipt, digest, evidence = self.historical_retirement("wt-att3")
        self.assertNotEqual(self.attest(receipt, "0" * 64, evidence)[0], 0)
        other = os.path.join(self.tmp, "vague.txt")
        with open(other, "w") as handle:
            handle.write("yes go ahead")
        self.assertNotEqual(self.attest(receipt, digest, other)[0], 0)
        self.assertEqual(self.gate(wt)["code"], "ARTIFACTS_MISSING_UNVERIFIED")

    def test_tampering_after_attestation_withdraws_the_evidence(self):
        self.full_census()
        wt, t, receipt, digest, evidence = self.historical_retirement("wt-att4")
        self.assertEqual(self.attest(receipt, digest, evidence)[0], 0)
        with open(receipt["receipt_file"], "a") as handle:
            handle.write(" ")
        self.assertNotEqual(self.gate(wt)["status"], "CLEAR")

    def test_a_build_after_the_retirement_withdraws_the_evidence(self):
        self.full_census()
        wt, t, receipt, digest, evidence = self.historical_retirement("wt-att5")
        self.assertEqual(self.attest(receipt, digest, evidence)[0], 0)
        self.assertEqual(self.iso(wt, "check").returncode, 0)        # used again, artifacts may exist
        self.assertNotEqual(self.gate(wt)["status"], "CLEAR")

    def test_other_holds_still_apply_after_attestation(self):
        self.full_census()
        wt, t, receipt, digest, evidence = self.historical_retirement("wt-att6")
        self.assertEqual(self.attest(receipt, digest, evidence)[0], 0)
        with open(os.path.join(t, "debug", "stray.bin"), "w") as handle:
            handle.write("x" * 100)
        self.assertEqual(self.gate(wt)["code"], "UNRECOGNISED_RESIDUE")
        os.unlink(os.path.join(t, "debug", "stray.bin"))
        self.assertEqual(self.ret("pin", t, "--reason", "running-gui").returncode, 0)
        self.assertEqual(self.gate(wt)["code"], "PINNED")

    def test_an_attestation_never_authorises_a_deletion(self):
        self.full_census()
        wt, t, receipt, digest, evidence = self.historical_retirement("wt-att7")
        rc, out = self.attest(receipt, digest, evidence)
        att_path = out.split(": ")[-1].strip()
        wt2, t2 = self.managed_cache("wt-att7b", completed=True)
        path, d2 = self.manifest([self.group(wt2, t2)], "m7b.json")
        rc, r = self.run_retire(path, d2, execute=True, scoped=att_path)
        self.assertEqual(r["refused"]["code"], "SCOPED_RUN_INVALID")
        self.assertTrue(self.exists(t2, "debug/deps"))


class TemporaryEnable(M.MaintBase):
    def ready(self, name):
        wt, t = self.cache(name)
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        return wt, t

    def intact(self, t):
        return os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib"))

    def enabled(self):
        st, policy, _ = self.m.load_policy()
        return st == "ok" and policy["enabled"]

    def signed(self, name):
        import test_emuwiz_cargo_signing as S
        rc, out = self.call("request-retirement", "--only", name, raw=True)
        self.assertEqual(rc, 0, out)
        record = out.splitlines()[0].strip()
        S.sign(record)
        return record

    def test_signed_cycle_retires_and_restores_disabled(self):
        self.full_census()
        wt, t = self.ready("te-ok")
        self.approve(soft_gb="0.00001", hard_gb="0.0001", enable=False)
        rec = self.signed(os.path.basename(t))
        rc, out = self.call("run", "--only", os.path.basename(t), "--scoped-run", rec, "--enable-for-cycle")
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.intact(t))
        self.assertFalse(self.enabled())
        g = self.m.lifecycle_gate(wt)
        self.assertNotEqual(g["code"], "ARTIFACTS_MISSING_UNVERIFIED", g)   # receipt recognised (OWNER_SIGNED)

    def test_gate_reverifies_the_owner_signature(self):
        import test_emuwiz_cargo_signing as S
        self.full_census()
        wt, t = self.ready("te-forge")
        self.approve(soft_gb="0.00001", hard_gb="0.0001", enable=False)
        rec = self.signed(os.path.basename(t))
        rc, out = self.call("run", "--only", os.path.basename(t), "--scoped-run", rec, "--enable-for-cycle")
        self.assertEqual(rc, 0, out)
        self.review_unrecognised(wt, t)
        self.assertEqual(self.m.lifecycle_gate(wt)["status"], "CLEAR")
        # same-account forgery: the agent re-signs the consumed record with its own key
        S.sign(rec, S.keys()["attacker"])
        g = self.m.lifecycle_gate(wt)
        self.assertEqual((g["status"], g["code"]), ("HOLD", "RECEIPT_UNVERIFIED"), g)
        self.assertIn("owner signature does not verify", g["explanation"])

    def test_exception_and_signal_during_the_cycle_restore_disabled(self):
        self.full_census()
        wt, t = self.ready("te-sig")
        self.approve(soft_gb="0.00001", hard_gb="0.0001", enable=False)
        for boom in (RuntimeError("boom"), self.m.TerminatedDuringCycle(15)):
            rec = self.signed(os.path.basename(t))
            real = self.m.locked_cycle

            def raising(*a, **k):
                self.assertTrue(self.enabled())       # it really was enabled for the cycle
                raise boom
            self.m.locked_cycle = raising
            try:
                rc, out = self.call("run", "--only", os.path.basename(t), "--scoped-run", rec, "--enable-for-cycle")
            except RuntimeError:
                rc = None
            finally:
                self.m.locked_cycle = real
            self.assertFalse(self.enabled(), boom)
            self.assertTrue(self.intact(t))
            if isinstance(boom, self.m.TerminatedDuringCycle):
                self.assertEqual(rc, 143)

    def test_stale_marker_after_a_kill_is_recovered_before_anything_else(self):
        self.approve(enable=True)
        self.assertTrue(self.enabled())
        self.m.durable_write(os.path.join(self.m.state_dir(), self.m.TEMP_ENABLE_MARKER),
                             {"state": "ACTIVE", "pid": 1, "cycle": "x"})
        rc, out = self.call("status", "--json")
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.enabled())
        self.assertEqual(self.m.recover_stale_temporary_enable(), "NONE")

    def test_enable_for_cycle_needs_an_exact_batch(self):
        wt, t = self.ready("te-scope")
        self.approve(enable=False)
        rc, out = self.call("run", "--enable-for-cycle", raw=True)
        self.assertEqual(rc, 1, out)
        rc, out = self.call("run", "--scoped-run", "/nonexistent", raw=True)
        self.assertEqual(rc, 1, out)
        self.assertFalse(self.enabled())
        self.assertTrue(self.intact(t))

    def test_routine_cannot_lift_the_disabled_switch(self):
        self.full_census()
        wt, t = self.ready("te-routine")
        self.approve(soft_gb="0.00001", hard_gb="0.0001", enable=False)
        rc, out = self.call("run", "--only", os.path.basename(t), "--enable-for-cycle")
        self.assertEqual(rc, 1, out)
        self.assertTrue(self.intact(t))
        self.assertFalse(self.enabled())
        # enabled through `policy enable`, the routine batch runs and its receipt is recognised
        self.call("policy", "enable")
        rc, out = self.call("run", "--only", os.path.basename(t))
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.intact(t))
        self.review_unrecognised(wt, t)
        self.assertEqual(self.m.lifecycle_gate(wt)["status"], "CLEAR")

    def test_source_changed_between_plan_and_deletion_is_held(self):
        self.full_census()
        wt, t = self.ready("te-race")
        self.approve(soft_gb="0.00001", hard_gb="0.0001")
        X = self.m.executor()
        real = X.evaluate_group
        state = {"done": False}

        def racing(*a, **k):
            if not state["done"] and a[5]:              # execute=True: an agent commits mid-cycle
                state["done"] = True
                with open(os.path.join(wt, "late-wip.rs"), "w") as handle:   # HEAD unchanged, tree dirty
                    handle.write("fn wip() {}")
            return real(*a, **k)
        X.evaluate_group = racing
        try:
            rc, out = self.call("run")
        finally:
            X.evaluate_group = real
        self.assertTrue(self.intact(t), out)
        self.assertIn("ROUTINE_CONDITION_CHANGED", json.dumps(self.last_cycle()))

    def test_routine_cycle_excludes_unmerged_and_dirty_sources(self):
        self.full_census()
        wt1, t1 = self.ready("te-unmerged")
        self.git("-C", wt1, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "wip")
        wt2, t2 = self.ready("te-dirty")
        with open(os.path.join(wt2, "wip.rs"), "w") as handle:
            handle.write("fn wip() {}")
        self.approve(soft_gb="0.00001", hard_gb="0.0001")
        st = self.status()
        self.assertIn("SOURCE_NOT_MERGED", self.state_of(st, t1)["reasons"])
        self.assertIn("SOURCE_HISTORY_UNPRESERVED", self.state_of(st, t2)["reasons"])
        rc, out = self.call("run")
        self.assertTrue(self.intact(t1) and self.intact(t2), out)

    def test_concurrent_cycle_is_refused_nonzero(self):
        import fcntl
        self.full_census()
        wt, t = self.ready("te-busy")
        self.approve(soft_gb="0.00001", hard_gb="0.0001", enable=False)
        rec = self.signed(os.path.basename(t))
        fd = os.open(os.path.join(self.m.state_dir(), ".policy-transition.lock"), os.O_RDWR | os.O_CREAT, 0o600)
        fcntl.flock(fd, fcntl.LOCK_EX)
        try:
            rc, out = self.call("run", "--only", os.path.basename(t), "--scoped-run", rec, "--enable-for-cycle")
        finally:
            os.close(fd)
        self.assertEqual(rc, 1, out)
        self.assertIn("in progress", out)
        self.assertTrue(self.intact(t))
        self.assertFalse(self.enabled())

    def test_exit_codes(self):
        wt, t = self.ready("te-exit")
        rc, _ = self.call("run", raw=True)
        self.assertEqual(rc, 75)                       # no policy / disabled: nothing deleted
        self.approve(enable=False)
        self.assertEqual(self.call("run", raw=True)[0], 75)
        self.assertEqual(self.call("run", "--dry-run", raw=True)[0], 0)
        self.assertTrue(self.intact(t))


if __name__ == "__main__":
    unittest.main()
