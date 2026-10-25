#!/usr/bin/env python3
"""Completion, not elapsed time, authorises disposable Cargo retirement.

Synthetic caches and real own-process references only; never real build targets.
"""
import os
import subprocess
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import test_emuwiz_cargo_maintain as M
import test_emuwiz_cargo_retention as T


class Immediate(M.MaintBase):
    def fresh(self, name="fresh"):
        wt, target = self.managed_cache(name, age_days=0)
        self.finish(wt, "--task", "completed-fixture")
        self.full_census()
        return wt, target

    def execute(self, wt, target):
        path, digest = self.manifest([self.group(wt, target)])
        return self.run_retire(path, digest, execute=True)[1]["groups"][0]

    def test_immediate_clear_and_verified_receipt_without_aging(self):
        wt, target = self.fresh()
        self.approve()
        self.assertEqual(self.state_of(self.status(), target)["state"], "RETIRABLE")
        self.assertEqual(self.m.worktree_gate(wt)["code"], "AWAITING_RETIREMENT")
        rc, out = self.call("run")
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.exists(target, "debug/deps"))
        self.assertEqual(self.m.worktree_gate(wt)["status"], "CLEAR")
        for path in self.preserved_paths(target):
            self.assertTrue(os.path.exists(path), path)

    def test_immediate_manual_manifest_execution(self):
        wt, target = self.fresh()
        self.assertEqual(self.execute(wt, target)["outcome"], "RETIRED")

    def test_old_unfinished_task_is_held_even_with_exact_approved_manifest(self):
        wt, target = self.managed_cache("unfinished", age_days=100)
        # A completed Cargo command does not attest that the task is complete.
        self.full_census()
        group = self.execute(wt, target)
        self.assertIn("NOT_RELEASED", {h["code"] for h in group["holds"]})
        self.assertTrue(self.exists(target, "debug/deps"))

    def test_active_open_file_refuses_immediate_retirement(self):
        wt, target = self.fresh()
        proc = subprocess.Popen([sys.executable, "-c",
                                 "import sys,time; f=open(sys.argv[1],'rb'); print('ready',flush=True); time.sleep(30)",
                                 os.path.join(target, "debug/deps/libx.rlib")],
                                stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(proc.stdout.readline().strip(), "ready")
            group = self.execute(wt, target)
            self.assertIn("LIVE_PROCESS_REFERENCE", {h["code"] for h in group["holds"]})
            self.assertTrue(self.exists(target, "debug/deps"))
        finally:
            proc.terminate()
            proc.wait()
            proc.stdout.close()

    def test_running_pinned_executable_is_preserved(self):
        wt, target = self.fresh()
        binary = os.path.join(target, "debug/emuwiz")
        with open("/bin/sleep", "rb") as src, open(binary, "wb") as dst:
            dst.write(src.read())
        self.finish(wt)
        self.assertEqual(self.ret("pin", wt, "--reason", "running-gui").returncode, 0)
        proc = subprocess.Popen([binary, "30"])
        try:
            group = self.execute(wt, target)
            codes = {h["code"] for h in group["holds"]}
            self.assertIn("PINNED", codes)
            self.assertIn("LIVE_PROCESS_REFERENCE", codes)
            self.assertTrue(os.path.exists(binary))
        finally:
            proc.terminate()
            proc.wait()

    def test_ambiguous_completion_owner_is_held(self):
        wt, target = self.fresh()
        path = os.path.join(self.reg, "targets", self.key_of(target) + ".json")
        rec = T.jload(path)
        rec["released"]["by"] = "another-agent"
        T.jdump(rec, path)
        group = self.execute(wt, target)
        self.assertIn("OWNER_UNVERIFIED", {h["code"] for h in group["holds"]})
        self.assertTrue(self.exists(target, "debug/deps"))

    def test_rebuild_reopens_task_and_cannot_retire_by_age(self):
        wt, target = self.fresh()
        self.assertEqual(self.iso(wt, "check").returncode, 0)
        group = self.execute(wt, target)
        self.assertIn("NOT_RELEASED", {h["code"] for h in group["holds"]})
        self.assertTrue(self.exists(target, "debug/deps"))

    def test_write_within_two_seconds_requires_a_new_completion_release(self):
        wt, target = self.fresh()
        record_path = os.path.join(self.reg, "targets", self.key_of(target) + ".json")
        before = T.jload(record_path)["released"]["at"]
        artifact = os.path.join(target, "debug/deps/libx.rlib")
        with open(artifact, "ab") as handle:
            handle.write(b"late plain-cargo output")
        # Pin the reproduced write inside the former two-second tolerance.
        os.utime(artifact, (before + 0.001, before + 0.001))
        group = self.execute(wt, target)
        self.assertIn("WRITTEN_AFTER_RELEASE", {h["code"] for h in group["holds"]})
        self.assertTrue(os.path.exists(artifact))
        self.finish(wt)
        self.assertGreater(T.jload(record_path)["released"]["at"], before)
        self.assertEqual(self.execute(wt, target)["outcome"], "RETIRED")


if __name__ == "__main__":
    unittest.main()
