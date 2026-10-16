#!/usr/bin/env python3
"""Tests for scripts/emuwiz-cargo-maintain (phase 3 automatic retirement).

Everything is synthetic: private HOME/registry/targets under a temp directory, fake
cargo, temporary git worktrees.  Nothing here touches ~/.cache/emuwiz-cargo-targets
or any real cache.  The deletion engine under test is the real executor, run
in-process with a patched (complete) process census where noted.
"""
import contextlib
import fcntl
import importlib.machinery
import importlib.util
import io
import json
import os
import subprocess
import sys
import time
import unittest

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import test_emuwiz_cargo_retire as R  # noqa: E402
import test_emuwiz_cargo_retention as T  # noqa: E402

MAINTAIN = os.path.join(HERE, "emuwiz-cargo-maintain")
ISO = os.path.join(HERE, "cargo-iso")
DAY = 86400


def load_maintain():
    loader = importlib.machinery.SourceFileLoader("emuwiz_cargo_maintain_under_test", MAINTAIN)
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def with_full_census(cls):
    class Full(cls):
        def __init__(self, targets):
            super().__init__(targets)
            self.unreadable_foreign = 0
    return Full


class MaintBase(R.Base):
    def setUp(self):
        super().setUp()
        self.m = load_maintain()

    def full_census(self, m=None):
        m = m or self.m
        m.F.Census = with_full_census(m.F.Census)
        ex = m.executor()
        ex.F.Census = with_full_census(ex.F.Census)

    def call(self, *argv, m=None):
        m = m or self.m
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            try:
                rc = m.main(list(argv))
            except SystemExit as exit_:
                rc = exit_.code if isinstance(exit_.code, int) else 1
        return rc, out.getvalue()

    def status(self, m=None):
        rc, out = self.call("status", "--json", m=m)
        self.assertEqual(rc, 0, out)
        return json.loads(out)

    def approve(self, soft_gb="0.00001", hard_gb="0.0001", owners="tester", extra=(), enable=True):
        rc, out = self.call("policy", "approve", "--owners", owners, "--soft-gb", soft_gb,
                            "--hard-gb", hard_gb, *extra)
        self.assertEqual(rc, 0, out)
        if enable:
            rc, out = self.call("policy", "enable")
            self.assertEqual(rc, 0, out)

    def finish(self, wt, *extra):
        r = self.iso(wt, "--finish-task", *extra)
        self.assertEqual(r.returncode, 0, r.stderr)

    def state_of(self, status, target):
        name = os.path.basename(target)
        return next(t for t in status["targets"] if t["name"] == name)

    def history(self, m=None):
        m = m or self.m
        return m.read_history()

    def last_cycle(self):
        return [h for h in self.history() if h.get("event") == "cycle"][-1]

    def cache(self, name, age_days=10):
        wt, t = self.managed_cache(name, age_days=age_days)
        return wt, t

    def preserved_paths(self, t):
        return [os.path.join(t, "debug", "emuwiz-test-env", "profile", "library.db"),
                os.path.join(t, "debug", "emuwiz-test-env", "profile", "journal.jsonl"),
                os.path.join(t, "debug", "UNKNOWN.txt"),
                os.path.join(t, "debug", ".cargo-lock")]


class Lifecycle(MaintBase):
    def test_two_agents_release_one_retire_it_and_keep_the_other(self):
        full = self.full_census
        wt_a, ta = self.cache("agent-a")
        wt_b, tb = self.cache("agent-b")
        self.approve()
        full()
        # 1/2: both builds are running -> both ACTIVE and untouchable
        env = dict(self.env, FAKE_SLEEP="25")
        pa = subprocess.Popen([ISO, "check"], cwd=wt_a, env=env)
        pb = subprocess.Popen([ISO, "check"], cwd=wt_b, env=env)
        try:
            time.sleep(1.5)
            st = self.status()
            self.assertEqual(self.state_of(st, ta)["state"], "ACTIVE")
            self.assertEqual(self.state_of(st, tb)["state"], "ACTIVE")
            rc, out = self.call("run")
            self.assertEqual(self.last_cycle()["outcome"], "NOTHING_TO_DO")
            for p in self.preserved_paths(ta) + self.preserved_paths(tb):
                self.assertTrue(os.path.exists(p), p)
            # 3: agent A finishes its TASK while its build is still running -> held
            self.finish(wt_a, "--task", "agent-a-task")
            self.assertEqual(self.state_of(self.status(), ta)["state"], "ACTIVE")
        finally:
            pa.terminate()
            pa.wait()
        # 4: agent B keeps building; A's build is over and A is released + aged
        self.age(ta, 10)
        self.age_record(ta, 10)
        try:
            st = self.status()
            self.assertEqual(self.state_of(st, ta)["state"], "RETIRABLE")
            self.assertEqual(self.state_of(st, tb)["state"], "ACTIVE")
            # 5/6: only A's compiler artifacts are planned
            rc, out = self.call("plan", "--json")
            plan = json.loads(out)
            self.assertEqual([s["name"] for s in plan["selected"]], [os.path.basename(ta)])
            before_total = st["total_allocated"]
            # 7: the synthetic automatic cycle removes them
            rc, out = self.call("run")
            self.assertEqual(rc, 0, out)
            cyc = self.last_cycle()
            self.assertEqual(cyc["outcome"], "RETIRED", cyc)
            self.assertGreater(cyc["reclaimed_bytes"], 0)
            self.assertFalse(os.path.exists(os.path.join(ta, "debug", "deps")))
            self.assertFalse(os.path.exists(os.path.join(ta, "debug", ".fingerprint")))
            # 8: preserved evidence and the other agent's output are untouched
            for p in self.preserved_paths(ta) + self.preserved_paths(tb):
                self.assertTrue(os.path.exists(p), p)
            self.assertTrue(os.path.exists(os.path.join(tb, "debug", "deps", "libx.rlib")))
            # 9: budget fell and a durable receipt/journal records it
            after = self.status()
            self.assertLess(after["total_allocated"], before_total)
            self.assertTrue(os.path.exists(cyc["receipt"]))
            self.assertTrue(os.path.exists(cyc["journal"]))
            self.assertEqual(after["total_reclaimed_bytes"], cyc["reclaimed_bytes"])
            self.assertIsNotNone(after["last_successful_cleanup"])
        finally:
            pb.terminate()
            pb.wait()
        # 10: restart preserves state and does not repeat the retirement
        m2 = load_maintain()
        self.full_census(m2)
        rc, out = self.call("run", m=m2)
        again = [h for h in m2.read_history() if h.get("event") == "cycle"][-1]
        self.assertNotEqual(again["outcome"], "RETIRED")
        self.assertEqual(sum(h.get("reclaimed_bytes", 0) for h in m2.read_history() if h.get("event") == "cycle"),
                         cyc["reclaimed_bytes"])

    def test_a_new_build_reopens_a_released_target(self):
        wt, t = self.cache("reopen")
        self.finish(wt)
        self.age_record(t, 10)
        self.assertIn(self.state_of(self.status(), t)["state"], ("RELEASED", "RETIRABLE"))
        self.assertEqual(self.iso(wt, "check").returncode, 0)
        self.assertEqual(self.state_of(self.status(), t)["state"], "IDLE")


class Policy(MaintBase):
    def retirable(self, name="pol"):
        wt, t = self.cache(name)
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        return wt, t

    def test_default_is_disabled_and_nothing_is_deleted(self):
        wt, t = self.retirable()
        self.full_census()
        rc, out = self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))
        # approved but not enabled, and dry-run when enabled
        self.approve(enable=False)
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "DISABLED")
        self.call("policy", "enable")
        self.call("run", "--dry-run")
        self.assertEqual(self.last_cycle()["outcome"], "DRY_RUN")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_policy_integrity_and_bounds(self):
        wt, t = self.retirable()
        self.full_census()
        self.approve()
        pol = json.load(open(self.m.policy_path()))
        self.assertGreaterEqual(pol["limits"]["min_idle_hours"], 48)
        # tampering with the scope (digest no longer matches) fails closed
        pol["scope"]["owners"] = ["tester", "someone-else"]
        with open(self.m.policy_path(), "w") as handle:
            json.dump(pol, handle)
        os.chmod(self.m.policy_path(), 0o600)
        rc, out = self.call("run")
        self.assertEqual(rc, 1)
        self.assertEqual(self.last_cycle()["outcome"], "REFUSED")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))
        # group-writable policy file is refused
        self.approve()
        os.chmod(self.m.policy_path(), 0o664)
        st, _, why = self.m.load_policy()
        self.assertEqual(st, "invalid")
        os.chmod(self.m.policy_path(), 0o600)
        # soft must be below hard; owners are required
        rc, _ = self.call("policy", "approve", "--owners", "tester", "--soft-gb", "5", "--hard-gb", "4")
        self.assertNotEqual(rc, 0)
        rc, _ = self.call("policy", "approve")
        self.assertNotEqual(rc, 0)

    def test_owner_outside_the_policy_scope_is_never_retired(self):
        wt, t = self.retirable("scope")
        self.full_census()
        self.approve(owners="someone-else")
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "NOTHING_TO_DO")
        row = self.state_of(self.status(), t)
        self.assertIn("OWNER_NOT_IN_POLICY", row["reasons"])
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_max_targets_bound_and_oldest_first(self):
        wts = [self.retirable(f"b{i}") for i in range(3)]
        # make b1 the oldest inactive one
        self.age(wts[1][1], 40)
        self.age_record(wts[1][1], 40)
        self.full_census()
        self.approve(extra=("--max-targets", "1"))
        self.call("run")
        cyc = self.last_cycle()
        self.assertEqual(cyc["outcome"], "RETIRED")
        self.assertEqual([t["name"] for t in cyc["targets"]], [os.path.basename(wts[1][1])])
        self.assertTrue(os.path.exists(os.path.join(wts[0][1], "debug", "deps", "libx.rlib")))
        self.assertTrue(os.path.exists(os.path.join(wts[2][1], "debug", "deps", "libx.rlib")))

    def test_under_soft_budget_nothing_is_retired(self):
        wt, t = self.retirable("under")
        self.full_census()
        self.approve(soft_gb="100", hard_gb="200")
        self.call("run")
        self.assertEqual(self.last_cycle()["outcome"], "NOTHING_TO_DO")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))


class Holds(MaintBase):
    def setUp(self):
        super().setUp()
        self.full_census()

    def ready(self, name):
        wt, t = self.cache(name)
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        return wt, t

    def run_cycle(self):
        self.approve()
        return self.call("run")

    def intact(self, t):
        return os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib"))

    def test_unreleased_pinned_legacy_and_nonparticipating_are_held(self):
        idle_wt, idle = self.cache("idle")                    # tracked, never released
        pin_wt, pinned = self.ready("pinned")
        self.assertEqual(self.ret("pin", pin_wt, "--reason", "release-build").returncode, 0)
        legacy = os.path.join(self.targets, "legacy-0123456789ab")  # no record at all
        self.make_cache(legacy, old_days=30)
        adopted = os.path.join(self.targets, "adopted-aaaaaaaaaaaa")
        self.make_cache(adopted, old_days=30)
        self.assertEqual(self.ret("adopt", adopted, "--owner", "tester").returncode, 0)
        self.run_cycle()
        self.assertEqual(self.last_cycle()["outcome"], "NOTHING_TO_DO")
        st = self.status()
        for target, reason in ((idle, "NOT_RELEASED"), (pinned, "PINNED"),
                               (legacy, "NO_LIFECYCLE_RECORD"), (adopted, "NONPARTICIPATING")):
            row = self.state_of(st, target)
            self.assertFalse(row["auto_eligible"], target)
            self.assertIn(reason, row["reasons"], (target, row))
            self.assertTrue(self.intact(target), target)

    def test_corrupt_records_hold_and_never_block(self):
        wt, t = self.ready("corrupt")
        rec = os.path.join(self.reg, "targets", self.key_of(t) + ".json")
        open(rec, "w").write('{"active": 5}')
        self.run_cycle()
        self.assertEqual(self.last_cycle()["outcome"], "NOTHING_TO_DO")
        self.assertTrue(self.intact(t))
        self.assertEqual(open(rec).read(), '{"active": 5}')
        # corrupt pin state holds too
        wt2, t2 = self.ready("corrupt-pin")
        os.makedirs(os.path.join(self.reg, "pins"), exist_ok=True)
        open(os.path.join(self.reg, "pins", self.key_of(t2) + ".json"), "w").write("{")
        self.call("run")
        self.assertTrue(self.intact(t2))
        self.assertIn(self.state_of(self.status(), t2)["state"], ("PROTECTED",))

    def test_missing_privileges_fail_closed(self):
        wt, t = self.ready("priv")
        m = load_maintain()          # unpatched census: other users' processes are not inspectable
        self.approve()
        rc, out = self.call("run", m=m)
        cyc = [h for h in m.read_history() if h.get("event") == "cycle"][-1]
        if m.F.Census([]).unreadable_foreign == 0:
            self.skipTest("running with full process visibility (root)")
        self.assertEqual(cyc["outcome"], "NOTHING_TO_DO")
        self.assertIn("INCOMPLETE_PROCESS_VISIBILITY", cyc["why"])
        self.assertTrue(self.intact(t))

    def test_abandoned_build_record_does_not_block_retirement(self):
        wt, t = self.ready("abandoned")
        rec = os.path.join(self.reg, "targets", self.key_of(t) + ".json")
        data = T.jload(rec)
        data["active"] = [{"token": "x", "pid": 2 ** 22 - 3, "starttime": 1, "boot_id": "gone", "started": 1}]
        T.jdump(data, rec)
        self.run_cycle()
        self.assertEqual(self.last_cycle()["outcome"], "RETIRED")
        self.assertFalse(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_symlinked_target_is_never_followed(self):
        wt, t = self.ready("sym")
        real = t + ".real"
        os.rename(t, real)
        os.symlink(real, t)
        self.run_cycle()
        # the planner may still see the real directory, but the executor holds it:
        # the recorded identity no longer matches the path (fail closed)
        self.assertIn(self.last_cycle()["outcome"], ("NOTHING_TO_DO", "HELD"))
        self.assertEqual(self.last_cycle().get("reclaimed_bytes", 0), 0)
        self.assertTrue(os.path.exists(os.path.join(real, "debug", "deps", "libx.rlib")))

    def test_running_executable_blocks_retirement(self):
        wt, t = self.ready("exe")
        exe = os.path.join(t, "debug", "emuwiz")
        proc = subprocess.Popen([exe], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) \
            if self._runnable(exe) else None
        if proc is None:
            self.skipTest("synthetic executable not runnable")
        try:
            time.sleep(0.3)
            self.run_cycle()
            self.assertEqual(self.last_cycle()["outcome"], "NOTHING_TO_DO")
            self.assertTrue(self.intact(t))
        finally:
            proc.kill()
            proc.wait()

    @staticmethod
    def _runnable(path):
        try:
            with open(path, "w") as handle:
                handle.write("#!/bin/sh\nsleep 20\n")
            os.chmod(path, 0o755)
            return True
        except OSError:
            return False

    def test_failed_deletion_late_protected_file_is_partial_and_preserved(self):
        wt, t = self.ready("late")
        planted = os.path.join(t, "debug", "deps", "late-journal.jsonl")

        def hook(stage):
            if stage.startswith("before-unlink") and not os.path.exists(planted):
                with open(planted, "w") as handle:
                    handle.write("precious")
        self.m.executor()._hook = hook
        self.run_cycle()
        cyc = self.last_cycle()
        self.assertIn(cyc["outcome"], ("PARTIAL", "HELD"))
        self.assertEqual(open(planted).read(), "precious")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "emuwiz-test-env", "profile", "library.db")))

    def test_interrupted_execution_keeps_journal_and_is_not_auto_acknowledged(self):
        wt, t = self.ready("intr")
        count = {"n": 0}

        def hook(stage):
            if stage.startswith("before-entry"):
                count["n"] += 1
                if count["n"] == 2:
                    raise KeyboardInterrupt()
        self.m.executor()._hook = hook
        self.run_cycle()
        journals = [n for n in os.listdir(os.path.join(self.m.state_dir(), "receipts")) if n.endswith(".journal.jsonl")]
        self.assertEqual(len(journals), 1)
        before = open(os.path.join(self.m.state_dir(), "receipts", journals[0]), "rb").read()
        self.m.executor()._hook = None
        # the next cycle must refuse (incomplete journal) rather than acknowledge it
        self.call("run")
        cyc = self.last_cycle()
        self.assertNotEqual(cyc["outcome"], "RETIRED")
        self.assertEqual(open(os.path.join(self.m.state_dir(), "receipts", journals[0]), "rb").read(), before)

    def test_receipt_dir_inside_a_cache_creates_nothing_there(self):
        wt, t = self.ready("n1")
        ex = self.m.executor()
        group = {"directory": t, "device": os.lstat(t).st_dev, "inode": os.lstat(t).st_ino,
                 "deletion_paths_json": "[]", "associated_source_paths": json.dumps([wt])}
        path = os.path.join(self.tmp, "n1.json")
        json.dump({"cargo_artifact_groups": [group]}, open(path, "w"))
        import hashlib
        digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
        inside = os.path.join(t, "debug", "receipts")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = ex.main(["--manifest", path, "--approved-sha256", digest, "--receipt-dir", inside,
                          "--execute", "--json"])
        self.assertEqual(rc, 4)
        self.assertFalse(os.path.exists(inside))


class Admission(MaintBase):
    def test_cold_target_over_budget_is_advised_never_refused(self):
        # Phase 3 remediation: admission is ADVISORY ONLY.  The refusal (exit 5) and the
        # EMUWIZ_ALLOW_OVER_BUDGET override were removed; see test_..._remediation.py.
        wt, t = self.cache("big")           # existing cache gives a non-zero total
        new_wt = self.worktree("fresh")
        env = dict(self.env, EMUWIZ_HARD_BUDGET_BYTES="1000")
        r = self.iso(new_wt, "check", env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("hard budget", r.stderr)
        self.assertIn("not enforced", r.stderr)
        self.assertIn("finish-task", r.stderr)     # protected/active output is named when nothing is reclaimable
        self.assertTrue(os.path.isdir(self.target_of(new_wt)))
        events = open(os.path.join(self.reg, "maintenance", "history.jsonl")).read()
        self.assertIn("admission_advisory", events)

    def test_warm_targets_and_the_free_space_guard_are_unchanged(self):
        wt, t = self.cache("warm")
        env = dict(self.env, EMUWIZ_HARD_BUDGET_BYTES="1000")
        self.assertEqual(self.iso(wt, "check", env=env).returncode, 0)      # warm: no new allocation
        env2 = dict(self.env, EMUWIZ_MIN_FREE_GB="100000000")
        self.assertEqual(self.iso(self.worktree("nospace"), "check", env=env2).returncode, 4)

    def test_admission_can_be_disabled_and_tool_errors_never_block(self):
        self.cache("x")
        wt = self.worktree("y")
        env = dict(self.env, EMUWIZ_HARD_BUDGET_BYTES="1000", EMUWIZ_ADMISSION="0")
        self.assertEqual(self.iso(wt, "check", env=env).returncode, 0)

    def test_reclaimable_released_output_is_reported(self):
        wt, t = self.cache("rel")
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        self.full_census()
        self.approve()
        self.status()      # refreshes the usage cache with the eligible total
        env = dict(self.env, EMUWIZ_HARD_BUDGET_BYTES="1000")
        r = self.iso(self.worktree("cold"), "check", env=env)
        self.assertEqual(r.returncode, 0, r.stderr)      # advisory only
        self.assertIn("auto-eligible", r.stderr)


class Workflow(MaintBase):
    def test_finish_task_requires_the_owning_label(self):
        wt = self.worktree("never-built")
        r = self.iso(wt, "--finish-task")           # nothing was ever built: nothing to release or strand
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("CLEAR", r.stdout)
        wt2, t2 = self.cache("owned")
        env = dict(self.env, EMUWIZ_OWNER="somebody-else")
        r = self.iso(wt2, "--finish-task", env=env)
        self.assertNotEqual(r.returncode, 0)

    def test_a_finished_cargo_command_is_not_a_finished_task(self):
        wt, t = self.cache("cmd")
        self.age_record(t, 10)
        self.assertEqual(self.state_of(self.status(), t)["state"], "IDLE")

    def test_units_are_printed_not_installed(self):
        import pwd
        service, timer = self.m.unit_texts(pwd.getpwuid(os.getuid()).pw_name, self.m.DEFAULT_INSTALL_DIR)
        out = service + timer
        self.assertIn("ConditionPathExists=", out)
        self.assertIn("User=root", out)
        self.assertIn("OnCalendar=", out)
        self.assertNotIn(HERE, out)
        # units are never written for a copy that is not root-owned (this checkout)
        rc, out = self.call("units", "--install-dir", HERE)
        self.assertNotEqual(rc, 0)
        # the trusted, root-owned case is exercised as real root in test_emuwiz_cargo_privileged.py

    def test_no_second_deletion_engine(self):
        src = open(MAINTAIN).read()
        for banned in ("rmtree", "os.remove", "os.unlink(", "os.rmdir", "shutil.rm", "unlinkat"):
            self.assertNotIn(banned, src)


class WorktreeOrdering(MaintBase):
    """Finish-task, waiting out the inactivity rule, retirement, restart and only then
    worktree removal.  The gate never deletes; a release record alone never clears it."""

    def gate(self, wt, m=None):
        return (m or self.m).worktree_gate(wt)

    def retire_cycle(self, m=None):
        m = m or self.m
        self.full_census(m)
        self.call("run", m=m)

    def test_release_wait_retire_restart_then_removal(self):
        wt, t = self.managed_cache("ordering", age_days=1)     # used 1 day ago: inside the 48 h window
        self.approve()
        self.assertEqual(self.gate(wt)["code"], "NOT_RELEASED")
        r = self.iso(wt, "--finish-task", "--task", "ordering-task")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("KEEP this worktree", r.stdout)
        first = json.load(open(os.path.join(self.reg, "targets", self.key_of(t) + ".json")))["released"]
        # idempotent: a second release keeps the original record and does not fail
        r2 = self.iso(wt, "--finish-task", "--task", "other-name")
        self.assertEqual(r2.returncode, 0)
        again = json.load(open(os.path.join(self.reg, "targets", self.key_of(t) + ".json")))["released"]
        self.assertEqual(first, again)
        # waiting: released but younger than 48 h -> HOLD with the eligibility time
        g = self.gate(wt)
        self.assertEqual((g["status"], g["code"]), ("HOLD", "WAITING_INACTIVITY"))
        self.assertIn("eligible for retirement after", g["explanation"])
        self.retire_cycle()
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))   # not retired yet
        self.assertEqual(self.gate(wt)["status"], "HOLD")
        # time passes: eligible, still HOLD until the artifacts are actually retired
        self.age(t, 4)
        self.age_record(t, 4)
        g = self.gate(wt)
        self.assertEqual(g["code"], "AWAITING_RETIREMENT")
        self.retire_cycle()
        self.assertEqual(self.last_cycle()["outcome"], "RETIRED")
        self.assertFalse(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))
        # restart: a fresh process reads the same state and the gate is CLEAR with a verified receipt
        m2 = load_maintain()
        g = self.gate(wt, m2)
        self.assertEqual(g["status"], "CLEAR", g)
        self.assertIn("receipt verified", g["explanation"])
        for p in self.preserved_paths(t):
            self.assertTrue(os.path.exists(p), p)
        r = subprocess.run([sys.executable, MAINTAIN, "worktree-check", wt], env=self.env,
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue(os.path.isdir(wt))     # the gate never removed anything

    def test_release_without_retirement_never_clears_the_gate(self):
        wt, t = self.managed_cache("released-only", age_days=10)
        self.finish(wt)
        self.assertEqual(self.gate(wt)["status"], "HOLD")
        self.assertTrue(os.path.isdir(wt))

    def test_active_build_is_not_claimed_finished(self):
        wt, t = self.managed_cache("active", age_days=10)
        env = dict(self.env, FAKE_SLEEP="12")
        p = subprocess.Popen([ISO, "check"], cwd=wt, env=env)
        try:
            time.sleep(1.5)
            r = self.iso(wt, "--finish-task")
            self.assertEqual(r.returncode, 0, r.stderr)
            g = self.gate(wt)
            self.assertEqual(g["code"], "ACTIVE")
            self.assertEqual(self.state_of(self.status(), t)["state"], "ACTIVE")
        finally:
            p.terminate()
            p.wait()

    def test_pinned_cache_keeps_its_worktree_and_pins_survive_release(self):
        wt, t = self.managed_cache("pinned-order", age_days=10)
        self.assertEqual(self.ret("pin", wt, "--reason", "release-build").returncode, 0)
        self.finish(wt)
        self.age_record(t, 10)
        g = self.gate(wt)
        self.assertEqual(g["code"], "PINNED")
        pins = json.load(open(os.path.join(self.reg, "pins", self.key_of(t) + ".json")))["pins"]
        self.assertEqual(len(pins), 1)
        self.full_census()
        self.approve()
        self.call("run")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_failed_retirement_keeps_the_worktree_gate_held(self):
        wt, t = self.managed_cache("failed-order", age_days=10)
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        planted = os.path.join(t, "debug", "deps", "late-journal.jsonl")

        def hook(stage):
            if stage.startswith("before-unlink") and not os.path.exists(planted):
                open(planted, "w").write("precious")
        self.m.executor()._hook = hook
        self.approve()
        self.retire_cycle()
        self.assertEqual(open(planted).read(), "precious")
        g = self.gate(wt)
        self.assertEqual(g["status"], "HOLD", g)

    def test_missing_source_is_held_with_an_actionable_explanation(self):
        wt, t = self.managed_cache("gone", age_days=10)
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        import shutil
        shutil.rmtree(wt)                  # the worktree was removed before retirement
        g = self.gate(wt)
        self.assertEqual(g["code"], "SOURCE_MISSING")
        self.assertIn("adopt or discard", g["explanation"])
        self.approve()
        self.retire_cycle()
        self.assertEqual(self.last_cycle()["outcome"], "NOTHING_TO_DO")
        self.assertIn("SOURCE_MISSING", self.state_of(self.status(), t)["reasons"])
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_legacy_and_empty_and_tampered_receipt_cases(self):
        # legacy: artifacts but no lifecycle record
        wt = self.worktree("legacy-order")
        t = self.target_of(wt)
        self.make_cache(t, old_days=10)
        self.assertEqual(self.gate(wt)["code"], "NO_LIFECYCLE_RECORD")
        # no cache at all
        self.assertEqual(self.gate(self.worktree("nothing-built"))["status"], "CLEAR")
        # a retired cache whose receipt journal was removed is not trusted
        wt2, t2 = self.managed_cache("tamper", age_days=10)
        self.finish(wt2)
        self.age(t2, 10)
        self.age_record(t2, 10)
        self.approve()
        self.retire_cycle()
        self.assertEqual(self.gate(wt2)["status"], "CLEAR")
        cyc = self.last_cycle()
        os.rename(cyc["journal"], cyc["journal"] + ".moved")
        self.assertEqual(self.gate(wt2)["code"], "RECEIPT_UNVERIFIED")

    def test_agent_instructions_carry_the_completion_sequence(self):
        repo = os.path.dirname(HERE)
        text = open(os.path.join(repo, "AGENTS.md")).read()
        for needle in ("--finish-task", "worktree-check", "before removing"):
            self.assertIn(needle, text)
        post = open(os.path.join(HERE, "task-postcheck.sh")).read()
        self.assertIn("--finish-task", post)


if __name__ == "__main__":
    unittest.main(verbosity=2)
