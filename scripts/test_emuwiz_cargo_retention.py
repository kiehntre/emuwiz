#!/usr/bin/env python3
"""Tests for scripts/emuwiz-cargo-retention and its cargo-iso integration.

Private HOME, synthetic git worktrees, a fake cargo and temporary targets only.
Nothing here builds, deletes outside its own temp directory, or touches
~/.cache/emuwiz-cargo-target(s).
"""
import json
import os
import signal
import stat
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ISO = os.path.join(HERE, "cargo-iso")
RET = os.path.join(HERE, "emuwiz-cargo-retention")
DAY = 86400


def jload(path):
    with open(path) as handle:
        return json.load(handle)


def jdump(data, path):
    with open(path, "w") as handle:
        json.dump(data, handle)


class Env(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="emuwiz-retention-test.")
        self.addCleanup(self.rmtree, self.tmp)
        self.home = os.path.join(self.tmp, "home")
        os.makedirs(self.home)
        self.targets = os.path.join(self.tmp, "targets")
        self.reg = os.path.join(self.tmp, "registry")
        self.fake = os.path.join(self.tmp, "fake-cargo")
        with open(self.fake, "w") as f:
            f.write(
                "#!/usr/bin/env bash\n"
                'echo "$$ ${CARGO_TARGET_DIR}" >>"${FAKE_LOG:-/dev/null}"\n'
                'if [ -n "${FAKE_SLEEP:-}" ]; then exec sleep "$FAKE_SLEEP"; fi\n'
                'exit "${FAKE_EXIT:-0}"\n'
            )
        os.chmod(self.fake, 0o755)
        self.repo = os.path.join(self.tmp, "repo")
        self.git("init", "-q", self.repo)
        self.git("-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t",
                 "commit", "-q", "--allow-empty", "-m", "init")
        self.env = dict(os.environ)
        for var in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "EMUWIZ_CARGO_LOW_DEBUG"):
            self.env.pop(var, None)
        self.env.update({
            "HOME": self.home, "EMUWIZ_TARGET_ROOT": self.targets,
            "EMUWIZ_RETENTION_DIR": self.reg, "EMUWIZ_CARGO": self.fake,
            "EMUWIZ_MIN_FREE_GB": "0", "FAKE_LOG": os.path.join(self.tmp, "log"),
            "EMUWIZ_OWNER": "tester", "PATH": os.environ["PATH"],
        })
        self.now = time.time()

    @staticmethod
    def rmtree(path):
        subprocess.run(["chmod", "-R", "u+rwX", path], check=False)
        subprocess.run(["rm", "-rf", "--", path], check=False)

    def git(self, *args):
        subprocess.run(["git", *args], check=True, capture_output=True)

    def worktree(self, name):
        path = os.path.join(self.tmp, name)
        self.git("-C", self.repo, "worktree", "add", "-q", path, "-b", name)
        return path

    def iso(self, cwd, *args, env=None, **kw):
        return subprocess.run([ISO, *args], cwd=cwd, env=env or self.env,
                              capture_output=True, text=True, **kw)

    def ret(self, *args, env=None):
        e = dict(env or self.env)
        return subprocess.run([sys.executable, RET, *args, "--repo", self.repo],
                              env=e, capture_output=True, text=True)

    def target_of(self, wt):
        out = self.iso(wt, "--print-target").stdout
        return [l.split(None, 1)[1].strip() for l in out.splitlines() if l.startswith("target:")][0]

    def key_of(self, target):
        import hashlib
        return hashlib.sha256(os.path.realpath(target).encode()).hexdigest()[:20]

    def make_cache(self, target, old_days=10):
        """A synthetic cargo target with artifacts, test env and an unknown file."""
        deps = os.path.join(target, "debug", "deps")
        os.makedirs(deps)
        os.makedirs(os.path.join(target, "debug", ".fingerprint", "x"))
        os.makedirs(os.path.join(target, "debug", "incremental"))
        os.makedirs(os.path.join(target, "debug", "emuwiz-test-env", "profile"))
        for rel, size in (("debug/deps/libx.rlib", 40000), ("debug/deps/x.d", 100),
                          ("debug/.fingerprint/x/lib-x", 100), ("debug/emuwiz", 20000),
                          ("debug/emuwiz.d", 50), ("debug/emuwiz-test-env/profile/library.db", 8192),
                          ("debug/emuwiz-test-env/profile/journal.jsonl", 100),
                          ("debug/UNKNOWN.txt", 10), ("debug/incremental/blob", 5000)):
            with open(os.path.join(target, rel), "wb") as f:
                f.write(b"x" * size)
        os.chmod(os.path.join(target, "debug", "emuwiz"), 0o755)
        self.age(target, old_days)

    def age(self, path, days):
        stamp = self.now - days * DAY
        for root, dirs, files in os.walk(path):
            for n in dirs + files:
                try:
                    os.utime(os.path.join(root, n), (stamp, stamp), follow_symlinks=False)
                except OSError:
                    pass
        os.utime(path, (stamp, stamp))

    def adopt(self, target_name, owner="tester"):
        r = self.ret("adopt", target_name, "--owner", owner)
        self.assertEqual(r.returncode, 0, r.stderr)

    def plan(self, *extra):
        env = dict(self.env, EMUWIZ_RETENTION_NOW=str(self.now))
        r = self.ret("plan", "--json", "--accept-foreign-process-gap", *extra, env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        return json.loads(r.stdout)

    def held(self, plan, name):
        return next((h for h in plan["held"] if h["name"] == name), None)

    def selected_under(self, plan, target):
        return [s["path"] for s in plan["selected_paths"] if s["target"] == target]


class IsolationAndCompatibility(Env):
    def test_two_worktrees_keep_isolated_targets_and_records(self):
        a, b = self.worktree("wt-a"), self.worktree("wt-b")
        self.assertEqual(self.iso(a, "check").returncode, 0)
        self.assertEqual(self.iso(b, "check").returncode, 0)
        ta, tb = self.target_of(a), self.target_of(b)
        self.assertNotEqual(ta, tb)
        recs = {}
        for name in os.listdir(os.path.join(self.reg, "targets")):
            with open(os.path.join(self.reg, "targets", name)) as f:
                r = json.load(f)
            recs[r["target"]] = r
        self.assertEqual(set(recs), {os.path.realpath(ta), os.path.realpath(tb)})
        self.assertEqual(recs[os.path.realpath(ta)]["worktree"], os.path.realpath(a))
        self.assertEqual(len(recs[os.path.realpath(ta)]["head"]), 40)
        self.assertEqual(recs[os.path.realpath(ta)]["last_exit"], 0)
        self.assertEqual(recs[os.path.realpath(ta)]["active"], [])
        self.assertIn("dev", recs[os.path.realpath(ta)]["filesystem"])

    def test_print_target_is_read_only(self):
        wt = self.worktree("wt-ro")
        self.assertEqual(self.iso(wt, "--print-target").returncode, 0)
        self.assertFalse(os.path.exists(self.reg))
        self.assertFalse(os.path.exists(self.targets))

    def test_exit_codes_arguments_and_low_debug_are_preserved(self):
        wt = self.worktree("wt-args")
        for code in (0, 7, 101):
            env = dict(self.env, FAKE_EXIT=str(code))
            self.assertEqual(self.iso(wt, "test", "-p", "a b", "--", "--x", env=env).returncode, code)
        # argv and env reach the fake unchanged
        probe = os.path.join(self.tmp, "probe")
        with open(probe, "w") as f:
            f.write('#!/usr/bin/env bash\nprintf "%s\\n" "$#" "$@" "${CARGO_PROFILE_DEV_DEBUG-unset}" '
                    '>"$PROBE_OUT"\n')
        os.chmod(probe, 0o755)
        out = os.path.join(self.tmp, "probe.out")
        env = dict(self.env, EMUWIZ_CARGO=probe, PROBE_OUT=out)
        self.assertEqual(self.iso(wt, "--low-debug", "test", "-p", "a b", "--", "c d", env=env).returncode, 0)
        with open(out) as handle:
            lines = handle.read().split("\n")
        self.assertEqual(lines[:6], ["5", "test", "-p", "a b", "--", "c d"])
        self.assertEqual(lines[6], "0")

    def test_minimum_free_space_guard_still_refuses_before_tracking(self):
        wt = self.worktree("wt-space")
        env = dict(self.env, EMUWIZ_MIN_FREE_GB="99999999")
        r = self.iso(wt, "check", env=env)
        self.assertEqual(r.returncode, 4)
        self.assertFalse(os.path.exists(self.reg))
        self.assertFalse(os.path.exists(self.env["FAKE_LOG"]))

    def test_a_broken_registry_never_blocks_a_build(self):
        wt = self.worktree("wt-broken")
        blocker = os.path.join(self.tmp, "not-a-dir")
        with open(blocker, "w"):
            pass
        env = dict(self.env, EMUWIZ_RETENTION_DIR=os.path.join(blocker, "sub"), FAKE_EXIT="5")
        r = self.iso(wt, "check", env=env)
        self.assertEqual(r.returncode, 5)
        self.assertIn("building untracked", r.stderr)


class Protection(Env):
    def test_concurrent_builds_are_protected_and_lock_is_released_after(self):
        wt = self.worktree("wt-live")
        t = self.target_of(wt)
        self.make_cache(t)
        env = dict(self.env, FAKE_SLEEP="3")
        procs = [subprocess.Popen([ISO, "check"], cwd=wt, env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) for _ in range(2)]
        try:
            time.sleep(1.2)
            plan = self.plan()
            held = self.held(plan, os.path.basename(t))
            self.assertIn("BUILD_LOCK_HELD", held["holds"])
            self.assertEqual(self.selected_under(plan, os.path.realpath(t)), [])
            rec = jload(os.path.join(self.reg, "targets", self.key_of(t) + ".json"))
            self.assertEqual(len(rec["active"]), 2)
        finally:
            for p in procs:
                p.wait()
        plan = self.plan()
        held = self.held(plan, os.path.basename(t))
        self.assertNotIn("BUILD_LOCK_HELD", held["holds"] if held else [])

    def test_failed_build_releases_the_lock_and_records_the_failure(self):
        wt = self.worktree("wt-fail")
        t = self.target_of(wt)
        r = self.iso(wt, "check", env=dict(self.env, FAKE_EXIT="3"))
        self.assertEqual(r.returncode, 3)
        rec = jload(os.path.join(self.reg, "targets", self.key_of(t) + ".json"))
        self.assertEqual((rec["last_exit"], rec["active"]), (3, []))
        self.assertIs(self.free(t), True)

    def free(self, target):
        import fcntl
        fd = os.open(os.path.join(self.reg, "locks", self.key_of(target) + ".lock"), os.O_RDWR)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except OSError:
            return False
        finally:
            os.close(fd)

    def test_interrupted_build_releases_safely_and_dies_by_the_signal(self):
        wt = self.worktree("wt-int")
        t = self.target_of(wt)
        p = subprocess.Popen([ISO, "check"], cwd=wt, env=dict(self.env, FAKE_SLEEP="30"),
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.0)
        self.assertIs(self.free(t), False)
        p.send_signal(signal.SIGTERM)
        p.wait(timeout=10)
        self.assertEqual(p.returncode, -signal.SIGTERM)
        time.sleep(0.3)
        self.assertIs(self.free(t), True)

    def test_a_killed_wrapper_leaves_its_cargo_child_holding_the_lock(self):
        wt = self.worktree("wt-kill")
        t = self.target_of(wt)
        p = subprocess.Popen([ISO, "check"], cwd=wt, env=dict(self.env, FAKE_SLEEP="4"),
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.0)
        p.kill()
        p.wait()
        self.assertIs(self.free(t), False, "descendant keeps the lock after the wrapper dies")
        time.sleep(4.5)
        self.assertIs(self.free(t), True)

    def test_a_running_executable_prevents_retirement(self):
        wt = self.worktree("wt-exe")
        t = self.target_of(wt)
        self.make_cache(t)
        self.assertEqual(self.iso(wt, "check").returncode, 0)
        self.age(t, 10)
        self.age_record(t, 10)
        binary = os.path.join(t, "debug", "emuwiz")
        with open("/bin/sleep", "rb") as src, open(binary, "wb") as dst:
            dst.write(src.read())
        os.chmod(binary, 0o755)
        self.age(t, 10)
        proc = subprocess.Popen([binary, "30"])
        try:
            time.sleep(0.3)
            held = self.held(self.plan(), os.path.basename(t))
            self.assertIn("LIVE_PROCESS_REFERENCE", held["holds"])
            self.assertTrue(any(r["kind"] == "exe" for r in held["process_refs"]))
        finally:
            proc.kill()
            proc.wait()
        self.age(t, 10)  # running it touched the binary: that is real recent use
        plan = self.plan()
        self.assertIsNone(self.held(plan, os.path.basename(t)))
        self.assertTrue(self.selected_under(plan, os.path.realpath(t)))

    def age_record(self, target, days):
        path = os.path.join(self.reg, "targets", self.key_of(target) + ".json")
        rec = jload(path)
        for k in ("last_use", "last_end", "last_start", "first_seen"):
            if k in rec:
                rec[k] = self.now - days * DAY
        jdump(rec, path)

    def test_pins_survive_restart_and_are_owner_scoped(self):
        wt = self.worktree("wt-pin")
        t = self.target_of(wt)
        self.make_cache(t)
        self.assertEqual(self.iso(wt, "check").returncode, 0)
        self.age(t, 10)
        self.age_record(t, 10)
        r = self.ret("pin", wt, "--reason", "independent-review")
        self.assertEqual(r.returncode, 0, r.stderr)
        bad = self.ret("pin", wt, "--reason", "because")
        self.assertNotEqual(bad.returncode, 0)
        # a fresh process (restart) still sees the pin; age never releases it
        self.age_record(t, 400)
        plan = self.plan("--min-idle-hours", "1")
        self.assertIn("PINNED", self.held(plan, os.path.basename(t))["holds"])
        other = self.ret("release", wt, "--owner", "someone-else")
        self.assertNotEqual(other.returncode, 0)
        self.assertIn("PINNED", self.held(self.plan(), os.path.basename(t))["holds"])
        self.assertEqual(self.ret("release", wt).returncode, 0)
        self.assertIsNone(self.held(self.plan(), os.path.basename(t)))

    def test_recent_and_unknown_activity_and_unknown_owner_hold(self):
        recent = self.worktree("wt-recent")
        tr = self.target_of(recent)
        self.make_cache(tr, old_days=0.1)
        self.assertEqual(self.iso(recent, "check").returncode, 0)
        unowned = os.path.join(self.targets, "mystery-0123456789ab")
        self.make_cache(unowned, old_days=30)
        empty = os.path.join(self.targets, "empty-aaaaaaaaaaaa")
        os.makedirs(empty)
        os.utime(empty, (0, 0))
        plan = self.plan()
        self.assertIn("RECENT_ACTIVITY", self.held(plan, os.path.basename(tr))["holds"])
        self.assertEqual(self.selected_under(plan, os.path.realpath(tr)), [])
        self.assertIn("NO_LIFECYCLE_RECORD", self.held(plan, "mystery-0123456789ab")["holds"])
        self.assertEqual(self.selected_under(plan, os.path.realpath(unowned)), [])
        pending = [p["name"] for p in plan["pending_ownership_candidates"]]
        self.assertIn("mystery-0123456789ab", pending)

    def test_foreign_process_gap_holds_unless_explicitly_accepted(self):
        # Synthetic: simulate by asking for strict mode when /proc has foreign
        # user-space processes this user cannot read (the normal case on a
        # multi-user host). Only meaningful when such processes exist.
        wt = self.worktree("wt-gap")
        t = self.target_of(wt)
        self.make_cache(t)
        self.assertEqual(self.iso(wt, "check").returncode, 0)
        self.age(t, 10)
        self.age_record(t, 10)
        strict = self.ret("plan", "--json", env=dict(self.env, EMUWIZ_RETENTION_NOW=str(self.now)))
        data = json.loads(strict.stdout)
        gap = data["summary"]["process_visibility"]["unreadable_foreign_uid"]
        held = self.held(data, os.path.basename(t))
        if gap:
            self.assertIn("INCOMPLETE_PROCESS_VISIBILITY", held["holds"])
        else:
            self.assertIsNone(held)


class Planning(Env):
    def eligible_cache(self, name="wt-plan"):
        wt = self.worktree(name)
        t = self.target_of(wt)
        self.make_cache(t)
        self.assertEqual(self.iso(wt, "check").returncode, 0)
        self.age(t, 10)
        path = os.path.join(self.reg, "targets", self.key_of(t) + ".json")
        rec = jload(path)
        for k in ("last_use", "last_end", "last_start", "first_seen"):
            if k in rec:
                rec[k] = self.now - 10 * DAY
        jdump(rec, path)
        return wt, os.path.realpath(t)

    def test_plan_selects_only_compiler_artifacts_and_preserves_the_rest(self):
        _, t = self.eligible_cache()
        plan = self.plan()
        sel = self.selected_under(plan, t)
        rel = sorted(os.path.relpath(p, t) for p in sel)
        self.assertIn("debug/deps", rel)
        self.assertIn("debug/.fingerprint", rel)
        self.assertIn("debug/incremental", rel)
        self.assertIn("debug/emuwiz", rel)
        self.assertIn("debug/emuwiz.d", rel)
        joined = "\n".join(rel)
        for forbidden in ("emuwiz-test-env", "library.db", "journal", "UNKNOWN", ".sqlite"):
            self.assertNotIn(forbidden, joined)
        preserved = {os.path.relpath(p["path"], t) for p in plan["preserved_paths"]}
        self.assertIn("debug/emuwiz-test-env", preserved)
        self.assertIn("debug/UNKNOWN.txt", preserved)
        self.assertGreater(plan["estimated_recoverable_bytes"], 0)
        # nothing was deleted and no registry data other than records was written
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "emuwiz-test-env", "profile", "library.db")))
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))

    def test_data_looking_files_stop_selection_of_their_subtree(self):
        _, t = self.eligible_cache("wt-data")
        with open(os.path.join(t, "debug", "deps", "state.sqlite3"), "wb") as f:
            f.write(b"x")
        self.age(t, 10)
        plan = self.plan()
        rel = [os.path.relpath(p, t) for p in self.selected_under(plan, t)]
        self.assertNotIn("debug/deps", rel)
        why = {os.path.relpath(p["path"], t): p["why"] for p in plan["preserved_paths"]}
        self.assertIn("data file", why["debug/deps"])

    def test_symlinks_and_changed_identity_cannot_redirect_selection(self):
        wt, t = self.eligible_cache("wt-link")
        victim = os.path.join(self.tmp, "victim")
        os.makedirs(victim)
        with open(os.path.join(victim, "precious"), "w") as handle:
            handle.write("keep")
        # symlinked artifact dir and symlinked file inside deps
        os.rename(os.path.join(t, "debug", "incremental"), os.path.join(t, "debug", "inc-real"))
        os.symlink(victim, os.path.join(t, "debug", "incremental"))
        os.symlink(victim, os.path.join(t, "debug", "deps", "link"))
        plan = self.plan()
        sel = [os.path.relpath(p, t) for p in self.selected_under(plan, t)]
        self.assertNotIn("debug/incremental", sel)
        self.assertNotIn("debug/deps", sel)  # contains a symlink: held back whole
        self.assertTrue(all(victim not in p["path"] for p in plan["selected_paths"]))
        # a target that is itself a symlink to somewhere else is never inventoried as a cache
        link = os.path.join(self.targets, "evil-aaaaaaaaaaaa")
        os.symlink(victim, link)
        plan = self.plan()
        entry = self.held(plan, "evil-aaaaaaaaaaaa")
        self.assertIn("SYMLINK_OR_MISSING_TARGET", entry["holds"])
        self.assertEqual(self.selected_under(plan, link), [])
        # changed directory identity -> HOLD
        keyfile = os.path.join(self.reg, "targets", self.key_of(t) + ".json")
        rec = jload(keyfile)
        rec["identity"]["ino"] += 12345
        jdump(rec, keyfile)
        plan = self.plan()
        self.assertIn("DIRECTORY_IDENTITY_CHANGED", self.held(plan, os.path.basename(t))["holds"])
        self.assertTrue(os.path.exists(os.path.join(victim, "precious")))

    def test_unusual_target_names_are_held(self):
        weird = os.path.join(self.targets, "has space (odd)-aaaaaaaaaaaa")
        self.make_cache(weird, old_days=30)
        plan = self.plan()
        entry = self.held(plan, os.path.basename(weird))
        self.assertIn("UNUSUAL_TARGET_NAME", entry["holds"])
        self.assertEqual(self.selected_under(plan, os.path.realpath(weird)), [])

    def test_hardlinks_are_not_double_counted_and_external_links_retain(self):
        wt, t = self.eligible_cache("wt-hl")
        deps = os.path.join(t, "debug", "deps")
        big = os.path.join(deps, "big.rlib")
        with open(big, "wb") as f:
            f.write(b"y" * 1_000_000)
        os.link(big, os.path.join(deps, "big-alias.rlib"))          # both inside selection
        outside = os.path.join(self.tmp, "kept-link")
        shared = os.path.join(deps, "shared.rlib")
        with open(shared, "wb") as f:
            f.write(b"z" * 1_000_000)
        os.link(shared, outside)                                      # one link outside
        self.age(t, 10)
        env = dict(self.env, EMUWIZ_RETENTION_NOW=str(self.now))
        st = self.ret("status", "--json", "--accept-foreign-process-gap", env=env)
        total = json.loads(st.stdout)["total_allocated"]
        du = int(subprocess.run(["du", "-s", "--block-size=1", t], capture_output=True, text=True).stdout.split()[0])
        self.assertLessEqual(total, du + 4096 * 8)  # hardlinks counted once, not twice
        plan = self.plan()
        self.assertGreater(plan["retained_hardlink_bytes_not_counted"], 900_000)
        # freed estimate counts big.rlib (+alias) once and excludes shared.rlib
        allocated_big = os.lstat(big).st_blocks * 512
        self.assertGreaterEqual(plan["estimated_recoverable_bytes"], allocated_big)
        self.assertLess(plan["estimated_recoverable_bytes"] - allocated_big, 1_000_000)

    def test_status_reports_budgets_and_protected_blockers(self):
        wt, t = self.eligible_cache("wt-budget")
        self.assertEqual(self.ret("pin", wt, "--reason", "release-build").returncode, 0)
        env = dict(self.env, EMUWIZ_RETENTION_NOW=str(self.now))
        r = self.ret("status", "--accept-foreign-process-gap", "--soft-budget", "1000", "--hard-budget", "2000", env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("soft budget", r.stdout)
        self.assertIn("EXCEEDED", r.stdout)
        self.assertIn("protected floor", r.stdout)
        self.assertIn("PINNED", r.stdout)
        listing = self.ret("list", "--accept-foreign-process-gap", env=env).stdout
        self.assertIn("pin=release-build", listing)

    def test_plan_and_list_never_create_registry_or_touch_targets(self):
        os.makedirs(self.targets)
        before = sorted(os.listdir(self.targets))
        for cmd in ("status", "list", "plan"):
            self.assertEqual(self.ret(cmd, "--accept-foreign-process-gap").returncode, 0)
        self.assertFalse(os.path.exists(self.reg))
        self.assertEqual(sorted(os.listdir(self.targets)), before)


if __name__ == "__main__":
    unittest.main(verbosity=2)
