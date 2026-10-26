#!/usr/bin/env python3
"""Focused automatic lifecycle safety regressions; private synthetic caches only."""
import os
import subprocess
import sys
import unittest
from unittest.mock import patch
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(__file__))
import test_emuwiz_cargo_immediate as I


class LifecycleSafety(I.Immediate):
    def test_process_arriving_after_snapshot_refuses(self):
        wt, target = self.fresh()
        proc = None
        def arrival(point):
            nonlocal proc
            if point == 'before-entry:' + target + '/debug/deps' and proc is None:
                proc = subprocess.Popen([sys.executable, '-c',
                    "import sys,time; f=open(sys.argv[1]); print('ready',flush=True); time.sleep(30)",
                    target + '/debug/deps/libx.rlib'], stdout=subprocess.PIPE, text=True)
                self.assertEqual(proc.stdout.readline().strip(), 'ready')
        try:
            with patch.object(self.mod, 'hook', arrival):
                group = self.execute(wt, target)
            self.assertIn('LIVE_PROCESS_REFERENCE', {h['code'] for h in group['holds']})
            self.assertTrue(os.path.exists(target + '/debug/deps/libx.rlib'))
        finally:
            if proc:
                proc.terminate(); proc.wait(); proc.stdout.close()

    def test_process_arriving_immediately_before_unlink_refuses(self):
        wt, target = self.fresh()
        proc = None
        def arrival(point):
            nonlocal proc
            if point == 'before-unlink:libx.rlib' and proc is None:
                proc = subprocess.Popen([sys.executable, '-c',
                    "import sys,time; f=open(sys.argv[1]); print('ready',flush=True); time.sleep(30)",
                    target + '/debug/deps/libx.rlib'], stdout=subprocess.PIPE, text=True)
                self.assertEqual(proc.stdout.readline().strip(), 'ready')
        try:
            with patch.object(self.mod, 'hook', arrival):
                group = self.execute(wt, target)
            self.assertIn('LIVE_PROCESS_REFERENCE', {h['code'] for h in group['holds']})
            self.assertTrue(os.path.exists(target + '/debug/deps/libx.rlib'))
        finally:
            if proc:
                proc.terminate(); proc.wait(); proc.stdout.close()

    def test_open_handle_through_alias_refuses(self):
        wt, target = self.fresh()
        alias = self.tmp + '/alias.rlib'
        os.link(target + '/debug/deps/libx.rlib', alias)
        proc = subprocess.Popen([sys.executable, '-c',
            "import sys,time; f=open(sys.argv[1]); print('ready',flush=True); time.sleep(30)", alias],
            stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(proc.stdout.readline().strip(), 'ready')
            group = self.execute(wt, target)
            self.assertIn('LIVE_PROCESS_REFERENCE', {h['code'] for h in group['holds']})
            self.assertTrue(os.path.exists(target + '/debug/deps/libx.rlib'))
        finally:
            proc.terminate(); proc.wait(); proc.stdout.close()

    def test_pinned_without_process_refuses(self):
        wt, target = self.fresh()
        self.assertEqual(self.ret('pin', wt, '--reason', 'preservation-evidence').returncode, 0)
        self.assertIn('PINNED', {h['code'] for h in self.execute(wt, target)['holds']})

    def test_missing_owner_refuses(self):
        import json
        wt, target = self.fresh()
        path = self.reg + '/targets/' + self.key_of(target) + '.json'
        with open(path) as h: rec = json.load(h)
        rec.pop('owner')
        with open(path, 'w') as h: json.dump(rec, h)
        self.assertIn('OWNER_UNVERIFIED', {h['code'] for h in self.execute(wt, target)['holds']})

    def test_same_device_mount_refuses(self):
        wt, target = self.fresh()
        real = self.mod.F.mountinfo
        with patch.object(self.mod.F, 'mountinfo', lambda *a: real(*a) + [(target + '/debug/deps', 'ext4')]):
            self.assertIn('CONTAINS_MOUNT_POINT', {h['code'] for h in self.execute(wt, target)['holds']})
        self.assertTrue(os.path.exists(target + '/debug/deps/libx.rlib'))

    def test_mount_visibility_failure_refuses(self):
        wt, target = self.fresh()
        with patch.object(self.mod.F, 'mountinfo', return_value=[]):
            self.assertIn('MOUNT_VISIBILITY_UNKNOWN', {h['code'] for h in self.execute(wt, target)['holds']})

    def test_free_reserve_triggers_below_soft_budget(self):
        wt, target = self.fresh()
        self.approve(soft_gb='100000', hard_gb='200000', extra=('--reserve-free-gib', '100000'))
        rc, out = self.call('plan', '--json')
        import json
        self.assertEqual(len(json.loads(out)['selected']), 1)

    def test_rebuild_after_retirement_reopens(self):
        wt, target = self.fresh()
        self.assertEqual(self.execute(wt, target)['outcome'], 'RETIRED')
        self.assertEqual(self.iso(wt, 'check').returncode, 0)
        import json
        with open(self.reg + '/targets/' + self.key_of(target) + '.json') as h: rec = json.load(h)
        self.assertNotIn('released', rec)
        self.assertTrue(rec['task'])

    def test_parent_moved_outside_target_refuses(self):
        wt, target = self.fresh()
        moved = self.tmp + '/moved-deps'
        def move(point):
            if point == 'before-unlink:libx.rlib' and not os.path.exists(moved):
                os.rename(target + '/debug/deps', moved)
        with patch.object(self.mod, 'hook', move):
            group = self.execute(wt, target)
        self.assertTrue(os.path.exists(moved + '/libx.rlib'))
        self.assertTrue(any(p.get('reason') == 'PARENT_IDENTITY_CHANGED' for p in group['paths']))

    def test_preservation_data_in_compiler_subtree_refuses(self):
        wt, target = self.fresh()
        for suffix in ('.dat', '.bios', '.bin', '.png'):
            with open(target + '/debug/deps/precious' + suffix, 'w') as h: h.write('keep')
        self.finish(wt)
        group = self.execute(wt, target)
        for suffix in ('.dat', '.bios', '.bin', '.png'):
            self.assertTrue(os.path.exists(target + '/debug/deps/precious' + suffix))

    def test_active_real_rust_compiler_refuses(self):
        import shutil
        import time
        import glob
        import pwd
        installed = sorted(glob.glob(os.path.join(pwd.getpwuid(os.getuid()).pw_dir,
                                                  '.rustup/toolchains/*/bin/rustc')))
        rustc = installed[0] if installed else shutil.which('rustc')
        if rustc and os.path.basename(os.path.realpath(rustc)) == 'rustup':
            rustc = None  # never download a toolchain into the private fixture HOME
        if not rustc:
            self.skipTest('rustc unavailable')
        wt, target = self.fresh()
        proc = subprocess.Popen([rustc, '-', '--crate-type', 'lib', '--emit', 'metadata',
                                 '-o', target + '/debug/deps/live.rmeta'], stdin=subprocess.PIPE,
                                env=dict(os.environ, CARGO_TARGET_DIR=target))
        try:
            time.sleep(0.2)  # rustc waits for source EOF; it never finishes the fixture build.
            self.assertIsNone(proc.poll())
            group = self.execute(wt, target)
            self.assertIn('LIVE_PROCESS_REFERENCE', {h['code'] for h in group['holds']})
            self.assertTrue(os.path.exists(target + '/debug/deps/libx.rlib'))
        finally:
            proc.terminate(); proc.wait(); proc.stdin.close()

    def test_unpinned_running_gui_executable_refuses(self):
        wt, target = self.fresh()
        binary = target + '/debug/emuwiz'
        with open('/bin/sleep', 'rb') as src, open(binary, 'wb') as dst: dst.write(src.read())
        self.finish(wt)
        proc = subprocess.Popen([binary, '30'])
        try:
            group = self.execute(wt, target)
            self.assertIn('LIVE_PROCESS_REFERENCE', {h['code'] for h in group['holds']})
            self.assertTrue(os.path.exists(binary))
        finally:
            proc.terminate(); proc.wait()

    def test_shared_target_refuses_even_with_completion(self):
        wt, target = self.fresh()
        with patch.object(self.mod.F, 'legacy_target', return_value=target):
            group = self.execute(wt, target)
        self.assertIn('SHARED_OR_EXTERNAL_TARGET', {h['code'] for h in group['holds']})
        self.assertTrue(os.path.exists(target + '/debug/deps/libx.rlib'))

    def test_saved_inventory_known_source_remains_hold(self):
        wt, target = self.managed_cache('unregistered', build=False)
        audit = self.m.load(os.path.join(os.path.dirname(__file__), 'emuwiz-cargo-lifecycle-audit'), 'audit_test')
        rows = [{'path': target, 'classification': 'UNKNOWN', 'worktree': wt,
                 'reasons': ['NO_LIFECYCLE_RECORD']}]
        result = audit.explain(rows, wt)[0]
        self.assertEqual(result['classification'], 'HOLD')
        self.assertEqual(result['ownership_evidence'], 'exact-source-path-hash')
        self.assertFalse(result['eligible_for_deletion'])

    def test_owner_fuse_retry_does_not_ignore_failure(self):
        import test_emuwiz_cargo_maintain as M
        census = M.load_maintain().F.Census.__new__(M.load_maintain().F.Census)
        census._inode_refs = []
        census._owner_inode_retries = []
        census.unreadable_foreign = 0
        with patch('os.stat', side_effect=PermissionError), patch('os.getresuid', return_value=(0, 0, 0)):
            census.note_inode('/proc/123/exe', 123, 'exe', owner_retry=True)
            self.assertEqual(len(census._owner_inode_retries), 1)
            self.assertEqual(census.unreadable_foreign, 0)
            census.note_inode('/proc/123/exe', 123, 'exe')
            self.assertEqual(census.unreadable_foreign, 1)

    def test_same_uid_inaccessible_process_is_gap(self):
        # Use original class, not the fixture's explicit full-visibility override.
        import test_emuwiz_cargo_maintain as M
        census = M.load_maintain().F.Census
        with patch.object(census, 'scan', return_value=False), patch.object(census, 'is_userspace', return_value=True):
            self.assertGreater(census([]).unreadable_foreign, 0)


    def census_of_one_pid(self, *, scan, exists, start, **patches):
        """Census over exactly one fake PID 4242 (root-owned, user-space command line)."""
        import test_emuwiz_cargo_maintain as M
        cls = M.load_maintain().F.Census
        census = cls.__new__(cls)
        census.skip_pids, census.unreadable_foreign, census.examined = frozenset(), 0, 0
        census.pids_seen, census.refs = set(), {}
        with patch('os.listdir', return_value=['4242']), \
                patch('os.stat', return_value=type('S', (), {'st_uid': 0})()), \
                patch.object(cls, 'scan', side_effect=scan), patch.object(cls, 'is_userspace', return_value=True), \
                patch.object(cls, 'start_time', side_effect=start), patch('os.path.exists', side_effect=exists), \
                patch.object(cls, 'EXIT_SETTLE', (0, 0, 0)):
            census._scan_all(0)
        return census

    def test_process_exiting_during_inspection_is_not_a_gap(self):
        # Short-lived processes ending mid-scan were counted as unreadable
        # (INCOMPLETE_PROCESS_VISIBILITY) while a retirement ran on a busy host.
        census = self.census_of_one_pid(scan=lambda *a: False, exists=[True, True, False], start=lambda b: 100)
        self.assertEqual(census.unreadable_foreign, 0)
        self.assertEqual(census.examined, 1)

    def test_process_that_stays_unreadable_is_still_a_gap(self):
        census = self.census_of_one_pid(scan=lambda *a: False, exists=lambda p: True, start=lambda b: 100)
        self.assertEqual(census.unreadable_foreign, 1)

    def test_start_time_that_cannot_be_compared_is_a_gap(self):
        census = self.census_of_one_pid(scan=lambda *a: False, exists=lambda p: True, start=lambda b: None)
        self.assertEqual(census.unreadable_foreign, 1)

    def test_reused_pid_is_inspected_again_not_assumed_gone(self):
        times = iter([100, 200, 200])  # first scan saw start 100; the PID now names start 200
        scans = iter([False, True])
        calls = []
        census = self.census_of_one_pid(scan=lambda *a: calls.append(a) or next(scans), exists=lambda p: True,
                                        start=lambda b: next(times))
        self.assertEqual(len(calls), 2, 'the process now holding the PID must be scanned itself')
        self.assertEqual(census.unreadable_foreign, 0)

    def test_reused_pid_whose_new_process_is_unreadable_is_a_gap(self):
        starts = iter(range(100, 1000, 100))
        census = self.census_of_one_pid(scan=lambda *a: False, exists=lambda p: True, start=lambda b: next(starts))
        self.assertEqual(census.unreadable_foreign, 1)

    def test_pid_reused_over_and_over_is_a_gap(self):
        starts = iter(range(1, 1000))
        census = self.census_of_one_pid(scan=lambda *a: False, exists=lambda p: True, start=lambda b: next(starts))
        self.assertEqual(census.unreadable_foreign, 1)

    def test_process_lookup_error_on_inode_check_means_exited(self):
        import test_emuwiz_cargo_maintain as M
        census = M.load_maintain().F.Census.__new__(M.load_maintain().F.Census)
        census._inode_refs, census._owner_inode_retries, census.unreadable_foreign = [], [], 0
        with patch('os.stat', side_effect=ProcessLookupError):
            census.note_inode('/proc/123/fd/3', 123, 'fd')
        self.assertEqual(census.unreadable_foreign, 0)
        with patch('os.stat', side_effect=PermissionError):
            census.note_inode('/proc/123/fd/3', 123, 'fd')
        self.assertEqual(census.unreadable_foreign, 1)
        with patch('os.stat', side_effect=OSError):
            census.note_inode('/proc/123/fd/3', 123, 'fd')
        self.assertEqual(census.unreadable_foreign, 2)

    def test_real_start_time_names_this_process_and_pid_one_differs(self):
        import test_emuwiz_cargo_maintain as M
        cls = M.load_maintain().F.Census
        mine, other = cls.start_time('/proc/self'), cls.start_time('/proc/1')
        self.assertIsInstance(mine, int)
        self.assertNotEqual(mine, other)
        self.assertIsNone(cls.start_time('/proc/999999999'))

    def test_new_process_holding_the_cache_is_still_found_after_a_vanished_one(self):
        import test_emuwiz_cargo_maintain as M
        cls = M.load_maintain().F.Census
        import tempfile
        with tempfile.TemporaryDirectory() as root:
            target = os.path.join(root, 't')
            os.mkdir(target)
            known = cls([target])
            holder = subprocess.Popen([sys.executable, '-I', '-c', 'import time;time.sleep(30)'], cwd=target)
            try:
                latest = cls([target], skip_pids=known.pids_seen)
                self.assertTrue(any(r['pid'] == holder.pid for r in latest.refs.get(target, [])))
                self.assertEqual(latest.unreadable_foreign, 0)
            finally:
                holder.kill()
                holder.wait()


if __name__ == '__main__': unittest.main()
