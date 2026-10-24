#!/usr/bin/env python3
"""Deletion fencing concurrency: private HOME, fake Cargo and disposable caches only."""
import fcntl
import json
import multiprocessing
import os
import subprocess
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import test_emuwiz_cargo_retire as R
import test_emuwiz_cargo_retention as T


class Fencing(R.Base):
    def retire_paused(self, path, digest, action, *, occurrence=1, stage_prefix="before-unlink:", pause_census=False):
        parent, child = multiprocessing.get_context("fork").Pipe()

        def worker():
            parent.close()
            hits = 0

            def pause(stage):
                nonlocal hits
                if not pause_census and stage.startswith(stage_prefix):
                    hits += 1
                    if hits == occurrence:
                        child.send("paused")
                        child.recv()
            self.mod._hook = pause
            if pause_census:
                base = self.mod.F.Census
                calls = 0

                class PausedCensus(base):
                    def __init__(inner, targets, **kw):
                        nonlocal calls
                        calls += 1
                        if calls == 3:  # final census, after reservation
                            child.send("paused")
                            child.recv()
                        super().__init__(targets, **kw)
                self.mod.F.Census = PausedCensus
            try:
                child.send(self.run_retire(path, digest, execute=True))
            except BaseException as error:
                child.send(("error", repr(error)))
        proc = multiprocessing.get_context("fork").Process(target=worker)
        proc.start()
        child.close()
        self.addCleanup(parent.close)
        command = None
        try:
            self.assertTrue(parent.poll(15), "retirement did not reach its fixture boundary")
            self.assertEqual(parent.recv(), "paused")
            completed = True
            if callable(action):
                action()
            else:
                actions = action if isinstance(action[0], list) else [action]
                for argv in actions:
                    command = subprocess.Popen(
                        [sys.executable, T.RET, *argv, "--repo", self.repo],
                        env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                    )
                    # Tests progress while retirement remains paused, not deletion speed.
                    try:
                        stdout, stderr = command.communicate(timeout=2)
                    except subprocess.TimeoutExpired:
                        completed = False
                        break
                    self.assertEqual(command.returncode, 0, stderr)
        finally:
            # Let existing commands finish naturally, even on the pre-fix failure.
            parent.send("continue")
            proc.join(20)
            if command is not None and command.poll() is None:
                stdout, stderr = command.communicate(timeout=20)
        self.assertFalse(proc.is_alive(), "fixture retirement did not exit")
        self.assertEqual(proc.exitcode, 0)
        self.assertTrue(completed, "global metadata lock blocks lifecycle writes during paused deletion")
        if command is not None:
            self.assertEqual(command.returncode, 0, stderr)
        self.assertTrue(parent.poll(2))
        result = parent.recv()
        self.assertNotEqual(result[0], "error", result)
        proc.close()
        return result

    def test_unrelated_completion_progresses_during_guarded_retirement(self):
        wt, target = self.managed_cache("retiring")
        other, other_target = self.managed_cache("unrelated", completed=False)
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        rc, receipt = self.retire_paused(path, digest, ["finish", other, "--task", "unrelated-done"])
        self.assertEqual(rc, 0)
        self.assertEqual(receipt["groups"][0]["outcome"], "RETIRED")
        record = T.jload(os.path.join(self.reg, "targets", self.key_of(other_target) + ".json"))
        self.assertEqual(record["released"]["task"], "unrelated-done")

    def test_pin_acknowledged_before_unlink_preserves_the_entire_subtree(self):
        wt, target = self.managed_cache("pin-before-unlink")
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        rc, receipt = self.retire_paused(path, digest, ["pin", wt, "--reason", "independent-review"])
        self.assertEqual(receipt["groups"][0]["files_removed"], 0)
        self.assertTrue(os.path.exists(os.path.join(target, "debug/deps/libx.rlib")))

    def test_unrelated_completion_progresses_during_final_process_census(self):
        wt, target = self.managed_cache("census")
        other, _ = self.managed_cache("other-census", completed=False)
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        self.retire_paused(path, digest, ["finish", other], pause_census=True)

    def test_reopen_and_adoption_each_revoke_the_target_fence(self):
        for command in ("reopen", "adopt"):
            with self.subTest(command=command):
                wt, target = self.managed_cache(command)
                path, digest = self.manifest([self.group(wt, target, ["debug/deps"])], command+".json")
                if command == "adopt":
                    def action():
                        env = dict(self.env, EMUWIZ_OWNER="successor")
                        result = self.ret("adopt", wt, "--owner", "successor", env=env)
                        self.assertEqual(result.returncode, 0, result.stderr)
                    args = action
                else:
                    args = [command, wt]
                _, receipt = self.retire_paused(path, digest, args)
                group = receipt["groups"][0]
                self.assertEqual(group["files_removed"], 0)
                self.assertIn("RETIREMENT_FENCE_CHANGED", {h["code"] for h in group["holds"]})

    def test_identical_json_replacement_is_still_a_revoked_reservation(self):
        wt, target = self.managed_cache("aba")
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        record_path = os.path.join(self.reg, "targets", self.key_of(target)+".json")
        def replace():
            # Exact old bytes: proof must detect object replacement, not JSON equality.
            with open(record_path, "rb") as handle:
                body = handle.read()
            with open(record_path+".new", "wb") as handle:
                handle.write(body)
            os.replace(record_path+".new", record_path)
        _, receipt = self.retire_paused(path, digest, replace)
        self.assertEqual(receipt["groups"][0]["files_removed"], 0)
        self.assertIn("RETIREMENT_FENCE_CHANGED", {h["code"] for h in receipt["groups"][0]["holds"]})

    def test_old_pin_then_release_client_cannot_hide_aba(self):
        wt, target = self.managed_cache("pin-aba")
        self.assertEqual(self.ret("pin", wt, "--reason", "independent-review").returncode, 0)
        self.assertEqual(self.ret("release", wt).returncode, 0)
        pin_path = os.path.join(self.reg, "pins", self.key_of(target)+".json")
        before = T.jload(pin_path)
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        _, receipt = self.retire_paused(path, digest, [
            ["pin", wt, "--reason", "independent-review"], ["release", wt],
        ])
        self.assertEqual(T.jload(pin_path), before)
        self.assertEqual(receipt["groups"][0]["files_removed"], 0)

    def test_unknown_metadata_after_reservation_stops_deletion(self):
        wt, target = self.managed_cache("unknown")
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        record_path = os.path.join(self.reg, "targets", self.key_of(target)+".json")
        def corrupt():
            with open(record_path, "w") as handle:
                handle.write("not JSON")
        _, receipt = self.retire_paused(path, digest, corrupt)
        self.assertEqual(receipt["groups"][0]["files_removed"], 0)

    def test_late_pin_records_partial_and_stops_all_later_entries(self):
        wt, target = self.managed_cache("partial")
        path, digest = self.manifest([self.group(wt, target, ["debug/deps", "debug/.fingerprint"])])
        _, receipt = self.retire_paused(path, digest, ["pin", wt, "--reason", "independent-review"], occurrence=2)
        group = receipt["groups"][0]
        self.assertEqual(group["outcome"], "PARTIAL")
        self.assertEqual(group["files_removed"], 1)
        self.assertEqual(group["paths"][0]["outcome"], "PARTIAL")
        self.assertEqual(group["paths"][1]["outcome"], "HELD")
        self.assertTrue(os.path.exists(os.path.join(target, "debug/.fingerprint/x/lib-x")))
        rows, torn = self.mod.read_journal(receipt["journal_file"])
        self.assertFalse(torn)
        self.assertEqual(rows[-1]["event"], "end")
        partial = [r for r in rows if r["event"] == "outcome"]
        self.assertEqual(len(partial), 1)
        self.assertEqual((partial[0]["outcome"], partial[0]["files"]), ("PARTIAL", 1))

    def test_rmdir_is_fenced_after_children_have_been_deleted(self):
        wt, target = self.managed_cache("rmdir")
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        _, receipt = self.retire_paused(path, digest, ["reopen", wt], stage_prefix="before-rmdir:")
        self.assertEqual(receipt["groups"][0]["outcome"], "PARTIAL")
        self.assertTrue(os.path.isdir(os.path.join(target, "debug/deps")))

    def test_concurrent_executor_cannot_take_reserved_target(self):
        wt, target = self.managed_cache("concurrent")
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        def second_executor():
            previous = self.receipts
            self.receipts = os.path.join(self.tmp, "second-receipts")
            try:
                _, receipt = self.run_retire(path, digest, execute=True)
                self.assertIn("LIFECYCLE_LOCK_UNAVAILABLE", {h["code"] for h in receipt["groups"][0]["holds"]})
            finally:
                self.receipts = previous
        _, receipt = self.retire_paused(path, digest, second_executor)
        self.assertEqual(receipt["groups"][0]["outcome"], "RETIRED")

    def test_global_lock_is_held_through_each_unlink_and_rmdir(self):
        wt, target = self.managed_cache("atomic")
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        seen = []
        def guarded(function):
            def call(*args, **kw):
                if kw.get("dir_fd") is not None:
                    fd = os.open(os.path.join(self.reg, "locks", "registry.meta"), os.O_WRONLY)
                    try:
                        with self.assertRaises(BlockingIOError):
                            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        seen.append(function.__name__)
                    finally:
                        os.close(fd)
                return function(*args, **kw)
            return call
        from unittest.mock import patch
        with patch.object(self.mod.os, "unlink", guarded(os.unlink)), patch.object(self.mod.os, "rmdir", guarded(os.rmdir)):
            _, receipt = self.run_retire(path, digest, execute=True)
        self.assertEqual(receipt["groups"][0]["outcome"], "RETIRED")
        self.assertIn("unlink", seen)
        self.assertIn("rmdir", seen)

    def test_interrupt_inside_fenced_unlink_releases_locks_and_keeps_partial_journal(self):
        wt, target = self.managed_cache("interrupt-fence")
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        raw_unlink = os.unlink
        calls = 0
        def interrupted(*args, **kw):
            nonlocal calls
            if kw.get("dir_fd") is not None:
                calls += 1
                if calls == 2:
                    raise KeyboardInterrupt()
            return raw_unlink(*args, **kw)
        from unittest.mock import patch
        with patch.object(self.mod.os, "unlink", interrupted):
            rc, receipt = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 130)
        self.assertEqual(receipt["groups"][0]["files_removed"], 1)
        self.assertEqual(receipt["groups"][0]["paths"][0]["outcome"], "PARTIAL")
        report = self.mod.inspect_journal(receipt["journal_file"])
        self.assertEqual(report["entries"][0]["state"], "KNOWN_PARTIAL")
        for name in ("registry.meta", self.key_of(target)+".lock"):
            fd = os.open(os.path.join(self.reg, "locks", name), os.O_WRONLY)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            finally:
                os.close(fd)
        self.assertEqual(self.ret("pin", wt, "--reason", "independent-review").returncode, 0)

    def test_reserved_metadata_objects_remain_open_to_prevent_inode_reuse(self):
        wt, target = self.managed_cache("held-metadata")
        self.assertEqual(self.ret("pin", wt, "--reason", "independent-review").returncode, 0)
        self.assertEqual(self.ret("release", wt).returncode, 0)
        registry = self.mod.F.Registry(create=True)
        key = self.key_of(target)
        fence = self.mod.RetirementFence(registry, key, target)
        self.addCleanup(lambda: getattr(fence, "close", lambda: None)())
        originals = [registry.path(kind, key) for kind in ("targets", "pins")]
        identities = {(os.stat(path).st_dev, os.stat(path).st_ino) for path in originals}
        def held_objects():
            held = []
            for name in os.listdir("/proc/self/fd"):
                try:
                    st = os.fstat(int(name))
                except OSError:
                    continue
                if (st.st_dev, st.st_ino) in identities:
                    held.append(st)
            return held
        fence.reserve()
        self.assertEqual(len(held_objects()), 2, "original metadata objects must stay alive despite pathname replacement")
        for path in originals:
            with open(path, "rb") as handle:
                body = handle.read()
            with open(path+".new", "wb") as handle:
                handle.write(body)
            os.replace(path+".new", path)
        self.assertEqual([st.st_nlink for st in held_objects()], [0, 0])
        with self.assertRaises(self.mod.Refusal):
            fence.check()
        fence.close()
        self.assertEqual(held_objects(), [])
        with self.assertRaises(self.mod.Refusal):
            fence.check()

    def test_metadata_descriptors_close_after_completion_refusal_and_interrupt(self):
        from unittest.mock import patch
        for result in ("complete", "refuse", "interrupt"):
            with self.subTest(result=result):
                wt, target = self.managed_cache("close-"+result)
                path, digest = self.manifest([self.group(wt, target, ["debug/deps"])], result+".json")
                captured = []
                raw_open = self.mod.F.open_lock_file
                record = os.path.join(self.reg, "targets", self.key_of(target)+".json")
                def tracked_open(name, flags):
                    fd = raw_open(name, flags)
                    if name == record:
                        captured.append(fd)
                    return fd
                def hook(stage):
                    if stage.startswith("before-unlink:"):
                        if result == "interrupt":
                            raise KeyboardInterrupt()
                        if result == "refuse":
                            self.assertEqual(self.ret("reopen", wt).returncode, 0)
                self.mod._hook = hook
                try:
                    with patch.object(self.mod.F, "open_lock_file", tracked_open):
                        _, receipt = self.run_retire(path, digest, execute=True)
                finally:
                    self.mod._hook = None
                self.assertEqual(len(captured), 1)
                for fd in captured:
                    with self.assertRaises(OSError):
                        os.fstat(fd)
                self.assertEqual(receipt["groups"][0]["outcome"],
                                 {"complete": "RETIRED", "refuse": "HELD", "interrupt": "INTERRUPTED"}[result])

    def test_failed_second_metadata_open_closes_first_and_deletes_nothing(self):
        from unittest.mock import patch
        wt, target = self.managed_cache("reserve-failure")
        self.assertEqual(self.ret("pin", wt, "--reason", "independent-review").returncode, 0)
        self.assertEqual(self.ret("release", wt).returncode, 0)
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        key = self.key_of(target)
        raw_open = self.mod.F.open_lock_file
        captured = []
        def fail_pin_open(name, flags):
            if name == os.path.join(self.reg, "pins", key+".json"):
                raise OSError(24, "Too many open files")
            fd = raw_open(name, flags)
            if name == os.path.join(self.reg, "targets", key+".json"):
                captured.append(fd)
            return fd
        with patch.object(self.mod.F, "open_lock_file", fail_pin_open):
            _, receipt = self.run_retire(path, digest, execute=True)
        self.assertEqual(len(captured), 1)
        with self.assertRaises(OSError):
            os.fstat(captured[0])
        self.assertEqual(receipt["groups"][0]["files_removed"], 0)
        self.assertIn("RETIREMENT_FENCE_CHANGED", {h["code"] for h in receipt["groups"][0]["holds"]})
        self.assertEqual(self.ret("reopen", wt).returncode, 0)

    def test_newcomer_census_inspects_only_new_pids_but_sees_them_all(self):
        wt, target = self.managed_cache("newcomer")
        proc = subprocess.Popen(["sleep", "30"], cwd=target)
        self.addCleanup(proc.wait)
        self.addCleanup(proc.kill)
        everyone = self.mod.F.Census([target]).pids_seen
        census = self.mod.F.Census([target], skip_pids=everyone - {proc.pid})
        self.assertLessEqual(census.examined, 1 + len(census.pids_seen - everyone))
        self.assertIn(proc.pid, census.pids_seen)
        self.assertIn(proc.pid, [r["pid"] for r in census.refs[target]])
        skipped = self.mod.F.Census([target], skip_pids=everyone)
        self.assertNotIn(proc.pid, [r["pid"] for r in skipped.refs.get(target, [])])
        self.assertIn(proc.pid, skipped.pids_seen)

    def test_process_recheck_retries_outside_global_lock(self):
        wt, target = self.managed_cache("retry")
        registry = self.mod.F.Registry(create=True)
        fence = self.mod.RetirementFence(registry, self.key_of(target), target)
        fence.reserve()
        self.addCleanup(fence.close)
        checks = []
        def guard(check_only=False):
            fd = os.open(os.path.join(self.reg, "locks", "registry.meta"), os.O_WRONLY)
            try:
                if check_only:
                    with self.assertRaises(BlockingIOError):
                        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                else:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            finally:
                os.close(fd)
            checks.append(check_only)
            return checks != [False, True]  # a process appeared during the first wait
        with fence.mutation(guard):
            self.assertEqual(checks, [False, True, False, True])

    def test_executable_started_while_waiting_for_metadata_is_seen_before_unlink(self):
        import shutil
        wt, target = self.managed_cache("late-executable")
        executable = os.path.join(target, "debug/emuwiz")
        shutil.copyfile("/bin/sleep", executable)
        self.age(target, 10)
        path, digest = self.manifest([self.group(wt, target, ["debug/deps"])])
        parent, child = multiprocessing.get_context("fork").Pipe()
        self.addCleanup(parent.close)

        def worker():
            parent.close()
            raw_lock = self.mod.F.Registry.meta_lock
            calls = 0
            def tracked_lock(registry):
                nonlocal calls
                calls += 1
                if calls == 2:
                    child.send("waiting-for-metadata")
                return raw_lock(registry)
            self.mod.F.Registry.meta_lock = tracked_lock
            paused = False
            def pause(stage):
                nonlocal paused
                if stage.startswith("before-unlink:") and not paused:
                    paused = True
                    child.send("paused")
                    child.recv()
            self.mod._hook = pause
            child.send(self.run_retire(path, digest, execute=True))
        proc = multiprocessing.get_context("fork").Process(target=worker)
        proc.start()
        child.close()
        use = None
        fd = None
        try:
            self.assertTrue(parent.poll(15))
            self.assertEqual(parent.recv(), "paused")
            fd = os.open(os.path.join(self.reg, "locks", "registry.meta"), os.O_WRONLY)
            fcntl.flock(fd, fcntl.LOCK_EX)
            parent.send("continue")
            self.assertTrue(parent.poll(15))
            self.assertEqual(parent.recv(), "waiting-for-metadata")
            # This real executable did not exist as a process at the pre-lock census.
            use = subprocess.Popen([executable, "5"], env=self.env)
        finally:
            if fd is not None:
                os.close(fd)
            proc.join(20)
            if use is not None:
                use.wait(timeout=10)
        self.assertFalse(proc.is_alive())
        self.assertEqual(proc.exitcode, 0)
        self.assertTrue(parent.poll(2))
        _, receipt = parent.recv()
        self.assertEqual(receipt["groups"][0]["files_removed"], 0)
        self.assertIn("LIVE_PROCESS_REFERENCE", {h["code"] for h in receipt["groups"][0]["holds"]})
        self.assertTrue(os.path.exists(os.path.join(target, "debug/deps/libx.rlib")))
        proc.close()


if __name__ == "__main__":
    unittest.main()
