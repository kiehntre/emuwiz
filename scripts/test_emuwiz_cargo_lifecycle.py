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


if __name__ == '__main__': unittest.main()
