#!/usr/bin/env python3
"""Realistic rustc incremental layout; private fixtures/fake cargo only, no build."""
import json
import fcntl
import os
import shutil
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import test_emuwiz_cargo_retire as R

CRATE = "example-1osdzgbzcmsi9"
SESSION = "s-hnb4rb79sn-0xwv2uo-3yvd1rvy7g196qvc60pdq0swy"
LOCK = "s-hnb4rb79sn-0xwv2uo.lock"


def populate_incremental(target):
    inc = os.path.join(target, "debug", "incremental")
    shutil.rmtree(inc)
    session = os.path.join(inc, CRATE, SESSION)
    os.makedirs(session)
    for name in ("dep-graph.bin", "query-cache.bin", "work-products.bin"):
        with open(os.path.join(session, name), "wb") as h:
            h.write(b"RSIC\x00\x00\x051.0.0" + b"x" * 64)
    with open(os.path.join(session, "0smmdd94ayydlsjbzswh02vzh.o"), "wb") as h:
        h.write(b"\x7fELF" + b"x" * 64)
    with open(os.path.join(session, "metadata.rmeta"), "wb") as h:
        h.write(b"rust" + b"\x00" * 64)
    with open(os.path.join(inc, CRATE, LOCK), "wb"):
        pass
    return inc


class Incremental(R.Base):
    def cache(self, name="wt-inc", completed=True):
        wt, target = self.managed_cache(name, completed=False)
        inc = populate_incremental(target)
        if completed:
            self.assertEqual(self.ret("finish", wt, "--task", "incremental-fixture").returncode, 0)
        return wt, target, inc

    def selected(self, target):
        return [p["path"] for p in self.mod.F.select_artifacts(target, None)[0]]

    def refuse(self, wt, target, code, group=None):
        group = group or self.group(wt, target, ["debug/incremental"])
        path, digest = self.manifest([group])
        _, result = self.run_retire(path, digest, execute=True)
        self.assertTrue(os.path.isdir(os.path.join(target, "debug", "incremental")))
        codes = {h["code"] for h in result["groups"][0]["holds"]}
        codes.update(p["reason"] for p in result["groups"][0]["paths"])
        self.assertIn(code, codes, result)
        self.assertEqual(result["groups"][0]["accounted_freed_bytes"], 0)

    def record_path(self, target):
        return os.path.join(self.reg, "targets", self.key_of(target) + ".json")

    def test_active_lease_refused(self):
        wt, target, _ = self.cache()
        registry = self.mod.F.Registry(create=False)
        fd = self.mod.F.open_lock_file(registry.lifecycle_lock_path(self.key_of(target)), os.O_RDWR)
        try:
            fcntl.flock(fd, fcntl.LOCK_SH)
            self.refuse(wt, target, "LIFECYCLE_LOCK_UNAVAILABLE")
        finally:
            os.close(fd)

    def test_paused_unfinished_target_refused(self):
        wt, target, _ = self.cache(completed=False)
        self.refuse(wt, target, "NOT_RELEASED")

    def test_unknown_owner_refused(self):
        wt, target, _ = self.cache()
        record = R.T.jload(self.record_path(target))
        record.pop("owner")
        R.T.jdump(record, self.record_path(target))
        self.refuse(wt, target, "NOT_ADOPTED")

    def test_pinned_running_gui_target_refused(self):
        wt, target, _ = self.cache()
        self.assertEqual(self.ret("pin", target, "--reason", "running-gui").returncode, 0)
        self.refuse(wt, target, "PINNED")

    def test_symlink_escape_refused(self):
        wt, target, inc = self.cache()
        leaf = os.path.join(inc, CRATE, SESSION, "query-cache.bin")
        os.unlink(leaf)
        os.symlink(os.path.join(self.victim, "precious.db"), leaf)
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        self.assertNotIn(inc, self.selected(target))
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        _, result = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(result)["debug/incremental"][0], "HELD")
        with open(os.path.join(self.victim, "precious.db")) as h:
            self.assertEqual(h.read(), "keep")

    def test_filesystem_identity_mismatch_refused(self):
        wt, target, _ = self.cache()
        group = self.group(wt, target, ["debug/incremental"])
        group["inode"] += 1
        self.refuse(wt, target, "DIRECTORY_IDENTITY_CHANGED", group)

    def test_source_worktree_and_audit_entries_refused(self):
        wt, target, _ = self.cache()
        rels = []
        for name in ("source", "worktree", "audit-ledger", "history", "undo"):
            path = os.path.join(target, "debug", name)
            os.mkdir(path)
            with open(os.path.join(path, "evidence.txt"), "w") as h:
                h.write("preserve")
            rels.append("debug/" + name)
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        group = self.group(wt, target, rels)
        path, digest = self.manifest([group])
        _, result = self.run_retire(path, digest, execute=True)
        self.assertTrue(all(p[0] == "HELD" for p in self.outcomes(result).values()))
        for rel in rels:
            self.assertTrue(os.path.isfile(os.path.join(target, rel, "evidence.txt")))
        # Even a structurally valid output tree cannot be retired outside the
        # exact registered target root (for example, inside the source checkout).
        group = self.group(wt, target, ["debug/incremental"])
        group["directory"] = wt
        group["device"], group["inode"] = os.stat(wt).st_dev, os.stat(wt).st_ino
        path, digest = self.manifest([group], "source.json")
        _, result = self.run_retire(path, digest, execute=True)
        self.assertIn("SHARED_OR_EXTERNAL_TARGET", {h["code"] for h in result["groups"][0]["holds"]})

    def test_header_and_unknown_files_refused_without_stranding_markers(self):
        for name, content in (("notes.txt", b"user notes"), ("query-cache.bin", b"not a rustc cache")):
            with self.subTest(name=name):
                wt, target, inc = self.cache("wt-" + name.replace(".", "-"))
                with open(os.path.join(inc, CRATE, SESSION, name), "wb") as h:
                    h.write(content)
                self.assertEqual(self.ret("finish", wt).returncode, 0)
                selected = self.selected(target)
                for rel in ("incremental", "deps", ".fingerprint"):
                    self.assertNotIn(os.path.join(target, "debug", rel), selected)

    def test_interrupted_working_session_with_partial_cache_selected(self):
        wt, target, inc = self.cache()
        working = os.path.join(inc, CRATE, SESSION.rsplit("-", 1)[0] + "-working")
        os.rename(os.path.join(inc, CRATE, SESSION), working)
        for name in os.listdir(working):
            if name != "dep-graph.bin":
                os.unlink(os.path.join(working, name))
        os.rename(os.path.join(working, "dep-graph.bin"), os.path.join(working, "dep-graph.part.bin"))
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        self.assertIn(inc, self.selected(target))
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        _, result = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(result)["debug/incremental"][0], "REMOVED")

    def test_in_place_change_after_snapshot_stops_unlink(self):
        wt, target, inc = self.cache()
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        cache = os.path.join(inc, CRATE, SESSION, "query-cache.bin")
        def change(stage):
            if stage == "before-unlink:query-cache.bin":
                st = os.stat(cache)
                with open(cache, "r+b") as h:
                    h.write(b"USER")
                os.utime(cache, ns=(st.st_atime_ns, st.st_mtime_ns))
        self.mod._hook = change
        _, result = self.run_retire(path, digest, execute=True)
        self.assertIn(self.outcomes(result)["debug/incremental"][0], ("REFUSED", "PARTIAL"))
        self.assertTrue(os.path.exists(cache))
        self.assertEqual(self.outcomes(result)["debug/incremental"][1], "INCREMENTAL_FILE_CHANGED")

    def test_late_user_file_not_in_snapshot_is_preserved(self):
        wt, target, inc = self.cache()
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        user = os.path.join(inc, CRATE, SESSION, "precious.db")
        def add(stage):
            if stage == "before-entry:" + inc:
                with open(user, "w") as h:
                    h.write("preserve")
        self.mod._hook = add
        _, result = self.run_retire(path, digest, execute=True)
        self.assertIn(self.outcomes(result)["debug/incremental"][0], ("REFUSED", "PARTIAL"))
        self.assertTrue(os.path.exists(user))

    def test_source_named_parent_is_not_a_target_triple(self):
        wt, target, inc = self.cache()
        os.makedirs(os.path.join(target, "source", "debug"))
        relocated = os.path.join(target, "source", "debug", "incremental")
        os.rename(inc, relocated)
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        self.assertNotIn(relocated, self.selected(target))
        path, digest = self.manifest([self.group(wt, target, ["source/debug/incremental"])])
        _, result = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(result)["source/debug/incremental"], ("HELD", "NOT_A_COMPILER_ARTIFACT"))
        self.assertTrue(os.path.isdir(relocated))

    def test_empty_orphan_has_no_compiler_evidence(self):
        wt, target, inc = self.cache()
        shutil.rmtree(inc)
        os.mkdir(inc)
        shutil.rmtree(os.path.join(target, "debug", "deps"))
        shutil.rmtree(os.path.join(target, "debug", ".fingerprint"))
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        self.assertNotIn(inc, self.selected(target))

    def test_compiler_crate_with_journal_in_its_name_is_not_a_user_journal(self):
        wt, target, inc = self.cache()
        os.rename(os.path.join(inc, CRATE), os.path.join(inc, "journal_tests-1osdzgbzcmsi9"))
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        self.assertIn(inc, self.selected(target))
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        _, result = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(result)["debug/incremental"][0], "REMOVED")

    def test_released_normal_incremental_selected_and_executed(self):
        wt, target, inc = self.cache()
        self.assertIn(inc, self.selected(target))
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        rc, dry = self.run_retire(path, digest)
        self.assertEqual(rc, 0)
        self.assertEqual(dry["groups"][0]["outcome"], "WOULD_RETIRE")
        self.assertTrue(os.path.exists(inc), "plan must not delete")
        rc, done = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 0)
        self.assertEqual(self.outcomes(done)["debug/incremental"][0], "REMOVED")
        self.assertFalse(os.path.exists(inc))
        self.assertTrue(os.path.isfile(os.path.join(target, "debug", "UNKNOWN.txt")))
        self.assertTrue(os.path.isfile(os.path.join(target, "debug", "emuwiz-test-env", "profile", "library.db")))

    def test_partial_prior_retirement_keeps_incremental_recognisable(self):
        wt, target, inc = self.cache()
        path, digest = self.manifest([self.group(wt, target, ["debug/deps", "debug/.fingerprint"])])
        self.assertEqual(self.run_retire(path, digest, execute=True)[0], 0)
        self.assertIn(inc, self.selected(target))
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])], "remaining.json")
        rc, done = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 0)
        self.assertFalse(os.path.exists(inc))

    def test_arbitrary_incremental_lookalike_is_not_a_compiler_artifact(self):
        wt, target = self.managed_cache()
        inc = os.path.join(target, "debug", "incremental")
        shutil.rmtree(inc)
        os.mkdir(inc)
        with open(os.path.join(inc, "blob"), "wb") as h:
            h.write(b"user content")
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        self.assertNotIn(inc, self.selected(target), "a directory name plus profile markers is not proof")
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        _, result = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(result)["debug/incremental"][0], "HELD")
        self.assertTrue(os.path.exists(os.path.join(inc, "blob")))

    def test_shared_compiler_files_across_sessions_are_accounted_once(self):
        wt, target, inc = self.cache()
        other = "s-hnb5fxdl5z-11l856z-c3cjd82qjft4u1ikzguli6fjx"
        os.mkdir(os.path.join(inc, CRATE, other))
        for name in os.listdir(os.path.join(inc, CRATE, SESSION)):
            os.link(os.path.join(inc, CRATE, SESSION, name), os.path.join(inc, CRATE, other, name))
        with open(os.path.join(inc, CRATE, "s-hnb5fxdl5z-11l856z.lock"), "wb"):
            pass
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        expected, _ = self.mod.F.reclaim_estimate([p for p in self.mod.F.select_artifacts(target, None)[0] if p["path"] == inc])
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        _, result = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(result)["debug/incremental"][0], "REMOVED")
        self.assertFalse(os.path.exists(inc))
        self.assertEqual(result["groups"][0]["accounted_freed_bytes"], expected)


class PidReuse(R.Base):
    """The incremental census skips PIDs it already inspected, by number.  That is only sound
    while the kernel cannot have recycled one of those numbers."""

    def guard(self, alive, before):
        guard = self.mod.PidReuseGuard()
        guard.arm(alive, before)
        return guard

    def safe_with(self, guard, state):
        from unittest.mock import patch
        with patch.object(self.mod, "read_fork_state", return_value=state):
            return guard.safe()

    def test_guard_is_safe_only_while_the_allocator_has_not_reached_an_inspected_pid(self):
        guard = self.guard({50, 150, 160}, (100, 1000, 1000))
        for state, expected, why in (
                ((100, 1000, 1000), True, "nothing happened"),
                ((149, 1000, 1000), True, "just below the nearest inspected PID"),
                ((150, 1000, 1000), False, "reached it (a failed fork or a thread does not move the fork counter)"),
                ((400, 1000, 1000), False, "went past it"),
                ((99, 1000, 1000), False, "wrapped"),
                ((120, 1000, 2000), False, "pid_max changed"),
                ((120, 1499, 1000), True, "just under half a lap of forks"),
                ((120, 1500, 1000), False, "half a lap of forks"),
                ((120, 999, 1000), False, "fork counter went backwards"),
                (None, False, "allocator unreadable")):
            self.assertEqual(self.safe_with(guard, state), expected, why)

    def test_guard_without_an_inspected_pid_ahead_only_watches_for_a_wrap(self):
        guard = self.guard({50, 60}, (100, 1000, 1000))
        self.assertTrue(self.safe_with(guard, (999, 1000, 1000)))
        self.assertFalse(self.safe_with(guard, (5, 1000, 1000)))

    def test_unarmed_or_unreadable_baseline_is_never_safe(self):
        self.assertFalse(self.safe_with(self.mod.PidReuseGuard(), (100, 1, 1000)))
        self.assertFalse(self.safe_with(self.guard({150}, None), (100, 1, 1000)))

    def test_allocator_state_must_be_readable_and_consistent(self):
        from unittest.mock import patch
        with patch("builtins.open", side_effect=OSError):
            self.assertIsNone(self.mod.read_fork_state())
        state = self.mod.read_fork_state()
        if state is not None:
            self.assertTrue(0 < state[0] < state[2])

    def deletion_censuses(self, states, expect_removed=True):
        """Censuses started while the executor removes a real incremental tree; the fork
        counter follows `states`, a function of how many times it has been read."""
        from unittest.mock import patch
        calls = []
        base = self.mod.F.Census

        class Light(base):
            def __init__(inner, targets, skip_pids=None):
                calls.append(skip_pids)
                inner.skip_pids, inner.targets = frozenset(skip_pids or ()), list(targets)
                inner.refs, inner.unreadable_foreign, inner.examined = {}, 0, 0
                inner._inode_refs = []
                inner.pids_seen = {int(p) for p in os.listdir("/proc") if p.isdigit()}

        self.mod.F.Census = Light
        wt, target = self.managed_cache("wt-reuse%d" % len(self.__dict__.setdefault("_runs", [])), completed=False)
        populate_incremental(target)
        self.assertEqual(self.ret("finish", wt, "--task", "reuse-fixture").returncode, 0)
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        self._runs.append(1)
        reads = [0]

        def state():
            reads[0] += 1
            return states(reads[0])

        with patch.object(self.mod, "read_fork_state", side_effect=state), \
                patch.object(self.mod, "FULL_CENSUS_INTERVAL", 3600):
            rc, result = self.run_retire(path, digest, execute=True)
        gone = not os.path.isdir(os.path.join(target, "debug", "incremental"))
        if expect_removed:
            self.assertTrue(gone, result)
            return calls
        self.assertFalse(gone, "nothing may be removed when PID reuse cannot be excluded")
        return calls, result

    def test_no_extra_censuses_while_no_pid_can_have_been_recycled(self):
        calls = self.deletion_censuses(lambda n: (2_000_000, 1000, 4194304))
        self.assertLessEqual(sum(skip is None for skip in calls), 4, calls)

    def test_a_wrapped_allocator_forces_one_more_full_census_without_skipping(self):
        steady = sum(skip is None for skip in self.deletion_censuses(lambda n: (2_000_000, 1000, 4194304)))
        wrapped = self.deletion_censuses(lambda n: (2_000_000 if n < 2 else 1_000, 1000, 4194304))
        self.assertGreater(sum(skip is None for skip in wrapped), steady, wrapped)

    def test_unreadable_allocator_refuses_instead_of_skipping_pids(self):
        calls, result = self.deletion_censuses(lambda n: None, expect_removed=False)
        codes = {h["code"] for h in result["groups"][0]["holds"]}
        codes.update(p["reason"] for p in result["groups"][0]["paths"])
        self.assertIn("INCOMPLETE_PROCESS_VISIBILITY", codes, result)
        self.assertTrue(all(skip is None for skip in calls), calls)

    def test_allocator_that_keeps_wrapping_during_every_census_refuses(self):
        calls, result = self.deletion_censuses(lambda n: (2_000_000 if n % 2 else 1_000, 1000, 4194304),
                                               expect_removed=False)
        codes = {h["code"] for h in result["groups"][0]["holds"]}
        codes.update(p["reason"] for p in result["groups"][0]["paths"])
        self.assertIn("INCOMPLETE_PROCESS_VISIBILITY", codes, result)


    def guard_run(self, states, light_missing_own_pid=False, full_interval=3600):
        """Like deletion_censuses, but also records the order of snapshots and censuses."""
        from unittest.mock import patch
        events, calls = [], []
        base = self.mod.F.Census

        class Light(base):
            def __init__(inner, targets, skip_pids=None):
                first_after_snapshot = events[-1:] == ["snapshot"] and events.count("snapshot") == 1
                events.append("census")
                calls.append(skip_pids)
                inner.skip_pids, inner.targets = frozenset(skip_pids or ()), list(targets)
                inner.refs, inner.unreadable_foreign, inner.examined, inner._inode_refs = {}, 0, 0, []
                inner.pids_seen = {int(p) for p in os.listdir("/proc") if p.isdigit()}
                if light_missing_own_pid and first_after_snapshot:  # the initial census of execute_group
                    inner.pids_seen.discard(os.getpid())  # only the first census: one "new" PID later

        self.mod.F.Census = Light
        wt, target = self.managed_cache("wt-order%d" % len(self.__dict__.setdefault("_runs", [])), completed=False)
        populate_incremental(target)
        self.assertEqual(self.ret("finish", wt, "--task", "order-fixture").returncode, 0)
        path, digest = self.manifest([self.group(wt, target, ["debug/incremental"])])
        self._runs.append(1)
        reads = [0]
        real_snapshot = self.mod.pid_snapshot

        def state():
            reads[0] += 1
            return states(reads[0])

        def snapshot():
            events.append("snapshot")
            return real_snapshot()

        with patch.object(self.mod, "read_fork_state", side_effect=state), \
                patch.object(self.mod, "pid_snapshot", side_effect=snapshot), \
                patch.object(self.mod, "FULL_CENSUS_INTERVAL", full_interval):
            rc, result = self.run_retire(path, digest, execute=True)
        return events, calls, result, os.path.isdir(os.path.join(target, "debug", "incremental"))

    def test_the_allocator_is_snapshotted_before_every_census(self):
        events, _, _, still_there = self.guard_run(
            lambda n: (1_000, 1000, 4194304) if n == 3 else (2_000_000, 1000, 4194304))
        self.assertFalse(still_there)
        self.assertGreater(events.count("snapshot"), 1, events)
        # the planning census before execution is not used for skipping; every census after the first
        # snapshot (the initial and all guard censuses) must have been preceded by its own snapshot
        events = events[events.index("snapshot"):]
        for i, event in enumerate(events):
            if event == "census":
                self.assertEqual(events[i - 1], "snapshot", events)

    def test_a_guard_census_that_cannot_be_proven_refuses_even_after_a_proven_start(self):
        from unittest.mock import patch
        proofs = []

        def safe(guard):
            proofs.append(1)
            return len(proofs) == 1  # the initial census is proven; nothing afterwards is

        with patch.object(self.mod.PidReuseGuard, "safe", autospec=True, side_effect=safe):
            events, calls, result, still_there = self.guard_run(lambda n: (2_000_000, 1000, 4194304))
        self.assertTrue(still_there, "nothing may be removed")
        codes = {h["code"] for h in result["groups"][0]["holds"]}
        codes.update(p["reason"] for p in result["groups"][0]["paths"])
        self.assertIn("INCOMPLETE_PROCESS_VISIBILITY", codes, result)
        details = " ".join(h.get("detail", "") for h in result["groups"][0]["holds"])
        self.assertIn("recycled", details)

    def test_an_incremental_census_is_redone_in_full_when_the_allocator_moved_during_it(self):
        # reads: 1 snapshot, 2 proof (initial); 3 guard check, 4 snapshot, 5 proof after the incremental census
        events, calls, result, still_there = self.guard_run(
            lambda n: (1_000, 1000, 4194304) if n == 5 else (2_000_000, 1000, 4194304), light_missing_own_pid=True)
        self.assertFalse(still_there, result)
        first_incremental = next(i for i, skip in enumerate(calls) if skip is not None)
        self.assertIsNone(calls[first_incremental + 1], calls)

    def test_unreadable_allocator_is_reported_distinctly(self):
        from unittest.mock import patch
        with patch.object(self.mod, "read_fork_state", return_value=None):
            self.assertIn("unreadable", self.mod.pid_reuse_detail(5))
        with patch.object(self.mod, "read_fork_state", return_value=(5, 5, 100)):
            self.assertIn("5 attempts", self.mod.pid_reuse_detail(5))


if __name__ == "__main__":
    unittest.main()
