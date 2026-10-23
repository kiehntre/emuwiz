#!/usr/bin/env python3
"""Activation-safety tests for scripts/emuwiz-cargo-maintain.

An enablement is bound to the fingerprint of the three scripts that were installed when
`policy enable` ran.  Installing, replacing or modifying any of them (or a record that
predates fingerprints, like the one on the production host) leaves retirement DISABLED
until `policy enable` is run again deliberately.

Disposable synthetic HOME, registry and caches only; no production state is read or written.
"""
import json
import os
import shutil
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_maintain as M  # noqa: E402
import test_emuwiz_cargo_maintain_remediation as R  # noqa: E402

NAMES = ("emuwiz-cargo-maintain", "emuwiz-cargo-retention", "emuwiz-cargo-retire")


class Activation(R.Base):
    def retirable(self, name="act"):
        wt, t = self.released(name)
        return wt, t

    def alive(self, t):
        return os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib"))

    def install_copy(self, tag):
        """A disposable 'installed' copy of the three scripts; the module under test is
        pointed at it so the fingerprint is that of the copy."""
        d = os.path.join(self.tmp, "install-" + tag)
        os.makedirs(d, exist_ok=True)
        for name in NAMES:
            shutil.copy(os.path.join(HERE, name), os.path.join(d, name))
        self.m.HERE = d
        return d

    def enabled(self):
        st, pol, _ = self.m.load_policy()
        return st == "ok" and bool(pol["enabled"])

    def events(self):
        return [e for e in (json.loads(l) for l in R.rt(self.m.history_path()).splitlines())
                if e.get("event") in self.m.TRANSITIONS]

    def cycles(self):
        return [h["outcome"] for h in self.history() if h.get("event") == "cycle"]

    def strip_binding(self):
        """Rewrite the stored enablement into the shape written before fingerprints existed
        (the production enable.json, generation 2)."""
        state = R.rj(self.m.enable_path())
        state.pop("code_sha256")
        R.wj(self.m.enable_path(), state)
        os.chmod(self.m.enable_path(), 0o600)
        lines = [json.loads(l) for l in R.rt(self.m.history_path()).splitlines()]
        for event in lines:
            event.pop("code_sha256", None)
        R.wt(self.m.history_path(), "".join(json.dumps(e, sort_keys=True) + "\n" for e in lines))
        os.chmod(self.m.history_path(), 0o600)

    # ----------------------------------------------------------- stale enable.json
    def test_pre_fingerprint_enable_record_does_not_enable(self):
        wt, t = self.retirable()
        self.approve(enable=True)
        self.assertTrue(self.enabled())
        self.strip_binding()
        st, pol, why = self.m.load_policy()
        self.assertEqual(st, "ok")
        self.assertFalse(pol["enabled"])
        self.assertIn("does not belong to the code now running", why)
        self.assertEqual(R.rj(self.m.enable_path())["state"], "ENABLED")   # the stale record is still there
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.assertTrue(self.alive(t))

    def test_state_and_history_must_agree_on_the_code(self):
        wt, t = self.retirable()
        self.approve(enable=True)
        state = R.rj(self.m.enable_path())
        state["code_sha256"] = "0" * 64
        R.wj(self.m.enable_path(), state)
        os.chmod(self.m.enable_path(), 0o600)
        self.assertFalse(self.enabled())
        self.call("run")
        self.assertTrue(self.alive(t))

    # ------------------------------------- installation, replacement, timer invocation
    def test_replacing_the_scripts_resets_enablement_and_timer_runs_delete_nothing(self):
        wt, t = self.retirable()
        d = self.install_copy("v1")
        self.approve(enable=True)
        self.assertTrue(self.enabled())
        # the installer replaces one script: same policy, same enable.json, same history
        with open(os.path.join(d, "emuwiz-cargo-retire"), "a") as handle:
            handle.write("\n# replaced by an upgrade\n")
        self.assertFalse(self.enabled())
        for _ in range(3):                                   # repeated timer invocations
            rc, out = self.call("run")
            self.assertEqual(self.last_cycle()["outcome"], "DISABLED", out)
        self.assertTrue(self.alive(t))
        self.assertEqual(self.cycles(), ["DISABLED"] * 3)
        self.assertEqual(os.listdir(os.path.join(self.m.state_dir(), "receipts")) if
                         os.path.isdir(os.path.join(self.m.state_dir(), "receipts")) else [], [])

    def test_each_script_is_part_of_the_binding(self):
        for name in NAMES:
            with self.subTest(script=name):
                self.setUp()
                d = self.install_copy("each-" + name)
                self.approve(enable=True)
                self.assertTrue(self.enabled())
                with open(os.path.join(d, name), "a") as handle:
                    handle.write("# changed\n")
                self.assertFalse(self.enabled())
                os.unlink(os.path.join(d, name))             # unreadable code fails closed too
                self.assertFalse(self.enabled())

    def test_same_code_restart_and_repeat_invocations_stay_as_enabled(self):
        self.install_copy("same")
        self.approve(enable=True)
        for _ in range(3):
            self.m = M.load_maintain()
            self.m.HERE = os.path.join(self.tmp, "install-same")   # a fresh process, same install
            self.full_census()
            self.assertTrue(self.enabled())
        self.call("policy", "disable")
        self.m = M.load_maintain()
        self.m.HERE = os.path.join(self.tmp, "install-same")
        self.assertFalse(self.enabled())                     # a restart never re-enables a disable

    def test_reenable_after_upgrade_is_deliberate_and_binds_the_new_code(self):
        wt, t = self.retirable()
        d = self.install_copy("up")
        self.approve(enable=True)
        before = R.rj(self.m.enable_path())
        with open(os.path.join(d, "emuwiz-cargo-maintain"), "a") as handle:
            handle.write("# upgrade\n")
        self.assertFalse(self.enabled())
        self.assertEqual(self.call("policy", "enable")[0], 0)
        after = R.rj(self.m.enable_path())
        self.assertTrue(self.enabled())
        self.assertGreater(after["generation"], before["generation"])
        self.assertNotEqual(after["code_sha256"], before["code_sha256"])
        self.assertEqual(self.events()[-1]["code_sha256"], after["code_sha256"])

    def test_disable_clears_a_stale_enabled_record_so_a_rollback_cannot_revive_it(self):
        d = self.install_copy("rb")
        self.approve(enable=True)
        old_fp = R.rj(self.m.enable_path())["code_sha256"]
        with open(os.path.join(d, "emuwiz-cargo-retire"), "a") as handle:
            handle.write("# upgrade\n")
        self.assertFalse(self.enabled())
        self.assertEqual(self.call("policy", "disable")[0], 0)
        self.assertEqual(R.rj(self.m.enable_path())["state"], "DISABLED")
        # roll the code back to the exact bytes that were enabled: still disabled
        shutil.copy(os.path.join(HERE, "emuwiz-cargo-retire"), os.path.join(d, "emuwiz-cargo-retire"))
        self.assertEqual(self.m.code_fingerprint(), old_fp)
        self.assertFalse(self.enabled())

    def test_approving_a_policy_is_still_disabled_after_approval(self):
        self.install_copy("ap")
        self.approve(enable=False)
        self.assertFalse(self.enabled())
        self.assertNotIn("code_sha256", R.rj(self.m.enable_path()))

    # ---------------------------------------------------------- invalid / missing config
    def test_missing_or_damaged_configuration_fails_closed(self):
        wt, t = self.retirable()
        self.install_copy("cfg")
        self.approve(enable=True)
        good_state, good_hist = R.rt(self.m.enable_path()), R.rt(self.m.history_path())
        good_policy = R.rt(self.m.policy_path())      # a timer run may revoke a stale record and clear the flag
        cases = {
            "enable.json missing": lambda: os.unlink(self.m.enable_path()),
            "enable.json garbage": lambda: R.wt(self.m.enable_path(), "{not json"),
            "enable.json code not a string": lambda: R.wj(self.m.enable_path(), dict(json.loads(good_state), code_sha256=7)),
            "history missing": lambda: os.unlink(self.m.history_path()),
            "history truncated": lambda: R.wt(self.m.history_path(), good_hist.rstrip("\n")),
            "history code removed": lambda: R.wt(self.m.history_path(), good_hist.replace('"code_sha256"', '"x"')),
        }
        for label, damage in cases.items():
            with self.subTest(label):
                R.wt(self.m.enable_path(), good_state)
                R.wt(self.m.history_path(), good_hist)
                R.wt(self.m.policy_path(), good_policy)
                damage()
                for p in (self.m.policy_path(), self.m.enable_path(), self.m.history_path()):
                    if os.path.exists(p):
                        os.chmod(p, 0o600)
                self.assertFalse(self.enabled())
                self.call("run")
                self.assertTrue(self.alive(t))
        R.wt(self.m.enable_path(), good_state)
        R.wt(self.m.history_path(), good_hist)
        R.wt(self.m.policy_path(), good_policy)
        for p in (self.m.policy_path(), self.m.enable_path(), self.m.history_path()):
            os.chmod(p, 0o600)
        self.assertTrue(self.enabled())

    def test_unreadable_scripts_refuse_enable_and_fingerprint_none_disables(self):
        d = self.install_copy("gone")
        self.approve(enable=False)
        os.unlink(os.path.join(d, "emuwiz-cargo-retention"))
        self.assertIsNone(self.m.code_fingerprint())
        rc, out = self.call("policy", "enable")
        self.assertNotEqual(rc, 0)
        self.assertFalse(self.enabled())

    def test_absent_policy_deletes_nothing(self):
        wt, t = self.retirable()
        self.call("run")
        self.assertTrue(self.alive(t))

    # ------------------------------------ positive control and the retained protections
    def test_a_deliberately_enabled_current_install_still_retires(self):
        wt, t = self.retirable("control")
        self.install_copy("ctl")
        self.approve(enable=True)
        self.call("run")
        self.assertNotEqual(self.last_cycle()["outcome"], "DISABLED", self.last_cycle())
        self.assertFalse(self.alive(t))

    def test_enabled_current_install_still_honours_hold_pin_and_journal_blocks(self):
        wt, t = self.retirable("held")
        pin_wt, pinned = self.released("pinned-act")
        self.assertEqual(self.ret("pin", pin_wt, "--reason", "release-build").returncode, 0)
        self.install_copy("hold")
        self.approve(enable=True)
        # an unresolved executor journal in the receipt directory blocks the whole cycle
        rdir = os.path.join(self.m.state_dir(), "receipts")
        os.makedirs(rdir, exist_ok=True)
        journal = os.path.join(rdir, "retire-20260101T000000-1-1.journal.jsonl")
        R.wt(journal, "")
        os.chmod(journal, 0o600)
        self.call("run")
        self.assertTrue(self.alive(t))
        self.assertTrue(self.alive(pinned))
        self.assertNotEqual(self.last_cycle()["outcome"], "RETIRED")
        self.assertTrue(os.path.exists(journal))             # never removed or acknowledged
        os.unlink(journal)
        self.call("run")
        self.assertTrue(self.alive(pinned))                  # the pin holds even with nothing blocking

    # ------------------------------------------- both switches, rollback, mid-cycle change
    def test_policy_flag_and_fresh_approval_are_both_required(self):
        wt, t = self.retirable("both")
        self.install_copy("both")
        self.approve(enable=True)
        self.assertTrue(R.rj(self.m.policy_path())["enabled"])
        self.assertTrue(self.enabled())
        pol = R.rj(self.m.policy_path())
        pol["enabled"] = False                       # disabled deployment policy, valid stored approval
        R.wj(self.m.policy_path(), pol)
        os.chmod(self.m.policy_path(), 0o600)
        st, loaded, why = self.m.load_policy()
        self.assertFalse(loaded["enabled"])
        self.assertIn("policy file is disabled", why)
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.assertTrue(self.alive(t))
        self.call("policy", "disable")
        self.assertFalse(R.rj(self.m.policy_path())["enabled"])

    def test_upgrade_enable_then_rollback_invalidates_the_approval(self):
        d = self.install_copy("rbk")
        self.approve(enable=True)
        v1 = self.m.code_fingerprint()
        with open(os.path.join(d, "emuwiz-cargo-retire"), "a") as handle:
            handle.write("# v2\n")
        self.assertEqual(self.call("policy", "enable")[0], 0)
        self.assertTrue(self.enabled())
        shutil.copy(os.path.join(HERE, "emuwiz-cargo-retire"), os.path.join(d, "emuwiz-cargo-retire"))
        self.assertEqual(self.m.code_fingerprint(), v1)
        self.assertFalse(self.enabled())             # the v1 approval was superseded and cannot return

    def _cycle_with_midpoint(self, change):
        wt, t = self.retirable("mid")
        self.install_copy("mid")
        self.approve(enable=True)
        real = self.m.append_history
        fired = []

        def hook(event):
            real(event)
            if event.get("event") == "cycle_start" and not fired:
                fired.append(1)
                change(wt, t)
        self.m.append_history = hook
        try:
            self.call("run")
        finally:
            self.m.append_history = real
        self.assertTrue(fired, "the cycle never reached the executor gate")
        return wt, t

    def test_disable_after_manifest_before_executor_refuses(self):
        def change(wt, t):                      # the cycle holds the transition lock: edit the file directly
            pol = R.rj(self.m.policy_path())
            pol["enabled"] = False
            R.wj(self.m.policy_path(), pol)
            os.chmod(self.m.policy_path(), 0o600)
        wt, t = self._cycle_with_midpoint(change)
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(self.alive(t))

    def test_code_replaced_mid_cycle_refuses(self):
        def change(wt, t):
            with open(os.path.join(self.m.HERE, "emuwiz-cargo-retention"), "a") as handle:
                handle.write("# swapped mid-cycle\n")
        wt, t = self._cycle_with_midpoint(change)
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(self.alive(t))

    def test_pin_added_mid_cycle_is_honoured_by_the_executor(self):
        def change(wt, t):
            self.assertEqual(self.ret("pin", wt, "--reason", "release-build").returncode, 0)
        wt, t = self._cycle_with_midpoint(change)
        self.assertTrue(self.alive(t))
        self.assertNotEqual(self.last_cycle()["outcome"], "RETIRED")

    def test_unresolved_journal_appearing_mid_cycle_blocks_the_executor(self):
        def change(wt, t):
            rdir = os.path.join(self.m.state_dir(), "receipts")
            os.makedirs(rdir, exist_ok=True)
            j = os.path.join(rdir, "retire-20260101T000000-1-1.journal.jsonl")
            R.wt(j, "")
            os.chmod(j, 0o600)
        wt, t = self._cycle_with_midpoint(change)
        self.assertTrue(self.alive(t))
        self.assertNotEqual(self.last_cycle()["outcome"], "RETIRED")

    def test_a_b_a_rollback_cannot_revive_the_old_approval(self):
        wt, t = self.retirable("aba")
        d = self.install_copy("aba")
        self.approve(enable=True)
        original = R.rt(os.path.join(d, "emuwiz-cargo-retire"))
        with open(os.path.join(d, "emuwiz-cargo-retire"), "a") as handle:
            handle.write("# B\n")
        self.call("run")                                     # the timer fires while B is installed
        self.assertEqual(R.rj(self.m.enable_path())["state"], "DISABLED")
        self.assertEqual(self.events()[-1]["event"], "policy_disabled")
        with open(os.path.join(d, "emuwiz-cargo-retire"), "w") as handle:
            handle.write(original)                           # roll back to A, byte for byte
        self.assertFalse(self.enabled())
        for _ in range(2):
            self.call("run")
            self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.assertTrue(self.alive(t))


if __name__ == "__main__":
    unittest.main()
