#!/usr/bin/env python3
"""Legacy checkout reconciliation: private synthetic Git/registry fixtures only."""
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import types
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(__file__))
import test_emuwiz_cargo_maintain as M
import test_emuwiz_cargo_retention as T


class LegacyCheckout(M.MaintBase):
    def setUp(self):
        super().setUp()
        self.wt, self.target = self.managed_cache('legacy-done', completed=True)
        shutil.rmtree(self.target)  # simulate historical loss ONLY inside this test
        self.git('-C', self.wt, 'update-ref', 'refs/remotes/origin/main',
                 self.m.F.head_of(self.wt))
        self.real_census = self.m.F.Census
        self.census = patch.object(self.m.F, 'Census', return_value=self.process_view())
        self.census.start()
        self.addCleanup(self.census.stop)

    def process_view(self, refs=None, unreadable_foreign=0):
        view = types.SimpleNamespace(refs=refs or {}, unreadable_foreign=unreadable_foreign,
                                     _inode_refs=[])
        view.match_inodes = types.MethodType(self.real_census.match_inodes, view)
        return view

    def check(self, **kw):
        return self.m.reconcile_legacy_worktree(self.wt, **kw)

    def hold(self, code, **kw):
        result = self.check(**kw)
        self.assertEqual(result['status'], 'HOLD', result)
        self.assertEqual(result['code'], code, result)
        self.assertTrue(os.path.isdir(self.wt))
        return result

    def test_absent_target_unknown_without_changing_normal_gate_or_record(self):
        record = self.reg + '/targets/' + self.key_of(self.target) + '.json'
        before = Path(record).read_bytes()
        self.assertEqual(self.m.worktree_gate(self.wt)['code'], 'ARTIFACTS_MISSING_UNVERIFIED')
        result = self.check()
        self.assertEqual(result['status'], 'CLEAR', result)
        self.assertEqual(result['historical_cache_retirement'], 'UNKNOWN')
        self.assertEqual(Path(record).read_bytes(), before)
        self.assertTrue(os.path.isdir(self.wt))
        self.assertEqual(self.m.worktree_gate(self.wt)['code'], 'ARTIFACTS_MISSING_UNVERIFIED')

    def test_present_reused_directory_file_and_dangling_link_all_refuse(self):
        for kind in ('dir', 'file', 'link'):
            with self.subTest(kind=kind):
                if kind == 'dir': os.mkdir(self.target)
                elif kind == 'file': open(self.target, 'w').close()
                else: os.symlink('/missing-legacy-fixture', self.target)
                self.hold('TARGET_PRESENT_USE_NORMAL_LIFECYCLE')
                if kind == 'dir': os.rmdir(self.target)
                else: os.unlink(self.target)

    def test_target_reappearing_after_intent_cannot_authorize_removal(self):
        original = self.m.append_history
        def recreate(event):
            original(event)
            if event.get('outcome') == 'CHECKOUT_REMOVAL_INTENT': os.mkdir(self.target)
        with patch.object(self.m, 'append_history', recreate):
            self.hold('TARGET_PRESENT_USE_NORMAL_LIFECYCLE', remove=True)

    def test_live_process_and_incomplete_visibility_refuse(self):
        for refs, unreadable, code in [({self.wt: [{'pid': 42}]}, 0, 'LIVE_PROCESS_REFERENCE'),
                                       ({}, 1, 'INCOMPLETE_PROCESS_VISIBILITY')]:
            with patch.object(self.m.F, 'Census', return_value=self.process_view(
                    refs=refs, unreadable_foreign=unreadable)):
                self.hold(code)

    def test_live_external_hardlink_handle_preserves_checkout(self):
        source = Path(self.wt, 'source.rs')
        source.write_text('tracked source must survive while in use')
        self.git('-C', self.wt, 'add', 'source.rs')
        self.git('-C', self.wt, '-c', 'user.name=t', '-c', 'user.email=t@t',
                 'commit', '-qm', 'source held through external alias')
        self.git('-C', self.wt, 'update-ref', 'refs/remotes/origin/main', self.m.F.head_of(self.wt))
        alias = Path(self.tmp, 'external-hardlink.rs')
        os.link(source, alias)
        child = subprocess.Popen([sys.executable, '-c',
            "import sys,time; f=open(sys.argv[1]); print('ready',flush=True); time.sleep(30)",
            str(alias)], cwd=self.tmp, stdout=subprocess.PIPE, text=True)
        # The fixture's sandbox cannot establish global visibility. Keep the real
        # process/inode census and suppress only unrelated fixture visibility holds.
        class FixtureCensus(self.real_census):
            def __init__(inner, *args, **kw):
                super().__init__(*args, **kw)
                inner.unreadable_foreign = 0
        try:
            self.assertEqual(child.stdout.readline().strip(), 'ready')
            with patch.object(self.m.F, 'Census', FixtureCensus):
                self.hold('LIVE_PROCESS_REFERENCE', remove=True)
            self.assertIsNone(child.poll())
            self.assertEqual(source.read_text(), 'tracked source must survive while in use')
        finally:
            child.terminate()
            child.wait()
            child.stdout.close()

    def test_incomplete_or_cross_device_checkout_identity_scan_refuses(self):
        for errors, cross_device in [(1, 0), (0, 1)]:
            with self.subTest(errors=errors, cross_device=cross_device):
                with patch.object(self.m.F, 'measure_tree', return_value={
                        'errors': errors, 'cross_device': cross_device, 'identities': set()}):
                    self.hold('CHECKOUT_IDENTITIES_UNVERIFIED', remove=True)

    def test_dirty_untracked_and_ignored_evidence_are_preserved(self):
        for filename, ignored in [('new-code.rs', False), ('evidence.log', True)]:
            with self.subTest(filename=filename):
                if ignored:
                    with open(self.wt + '/.gitignore', 'w') as h: h.write('evidence.log\n')
                    self.git('-C', self.wt, 'add', '.gitignore')
                    self.git('-C', self.wt, '-c', 'user.name=t', '-c', 'user.email=t@t',
                             'commit', '-qm', 'ignore synthetic evidence')
                    self.git('-C', self.wt, 'update-ref', 'refs/remotes/origin/main', self.m.F.head_of(self.wt))
                with open(self.wt + '/' + filename, 'w') as h: h.write('valuable')
                self.hold('CHECKOUT_NOT_CLEAN')
                self.assertEqual(Path(self.wt, filename).read_text(), 'valuable')
                os.unlink(self.wt + '/' + filename)

    def test_tracked_dirty_refuses(self):
        with open(self.wt + '/source', 'w') as h: h.write('committed')
        self.git('-C', self.wt, 'add', 'source')
        self.git('-C', self.wt, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'source')
        self.git('-C', self.wt, 'update-ref', 'refs/remotes/origin/main', self.m.F.head_of(self.wt))
        with open(self.wt + '/source', 'w') as h: h.write('unfinished')
        self.hold('CHECKOUT_NOT_CLEAN')

    def test_assume_unchanged_and_skip_worktree_cannot_hide_local_code(self):
        source = Path(self.wt, 'source.rs')
        source.write_text('committed')
        self.git('-C', self.wt, 'add', 'source.rs')
        self.git('-C', self.wt, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'source')
        self.git('-C', self.wt, 'update-ref', 'refs/remotes/origin/main', self.m.F.head_of(self.wt))
        for flag, clear in [('--assume-unchanged', '--no-assume-unchanged'),
                            ('--skip-worktree', '--no-skip-worktree')]:
            with self.subTest(flag=flag):
                self.git('-C', self.wt, 'update-index', flag, 'source.rs')
                source.write_text('UNIQUE UNFINISHED LOCAL CODE')
                self.hold('CHECKOUT_INDEX_UNVERIFIED', remove=True)
                self.assertEqual(source.read_text(), 'UNIQUE UNFINISHED LOCAL CODE')
                self.git('-C', self.wt, 'update-index', clear, 'source.rs')
                source.write_text('committed')

    def test_pin_active_job_and_lifecycle_lock_refuse_even_when_target_absent(self):
        key = self.key_of(self.target)
        pin = self.reg + '/pins/' + key + '.json'
        T.jdump({'pins': [{'owner': 'tester', 'reason': 'active-development'}]}, pin)
        self.hold('PINNED')
        os.unlink(pin)
        record = self.reg + '/targets/' + key + '.json'
        data = T.jload(record)
        data['active'] = [{'pid': os.getpid(), 'boot_id': self.m.F.boot_id(),
                          'starttime': self.m.F.proc_starttime(os.getpid())}]
        T.jdump(data, record)
        self.hold('RECORDED_BUILD_ALIVE')
        data['active'] = []
        T.jdump(data, record)
        with open(self.reg + '/locks/' + key + '.lock', 'a') as h:
            fcntl.flock(h, fcntl.LOCK_SH | fcntl.LOCK_NB)
            self.hold('LIFECYCLE_LOCKED')

    def test_shared_target_record_and_alternate_target_ownership_refuse(self):
        original = T.jload(self.reg + '/targets/' + self.key_of(self.target) + '.json')
        alias = dict(original, worktree=self.tmp + '/someone-else')
        alias_path = self.reg + '/targets/other.json'
        T.jdump(alias, alias_path)
        self.hold('TARGET_OWNERSHIP_UNVERIFIED')
        os.unlink(alias_path)
        alternative = self.targets + '/other-target'
        alias = dict(original, target=alternative, key=self.key_of(alternative))
        alias_path = self.reg + '/targets/' + self.key_of(alternative) + '.json'
        T.jdump(alias, alias_path)
        self.hold('TARGET_OWNERSHIP_UNVERIFIED')

    def test_invalid_registry_cannot_hide_second_owner(self):
        with open(self.reg + '/targets/bad.json', 'w') as h: h.write('{')
        self.hold('TARGET_OWNERSHIP_UNVERIFIED')

    def test_unmerged_state_requires_explicit_exact_preserved_ref(self):
        self.git('-C', self.wt, '-c', 'user.name=t', '-c', 'user.email=t@t',
                 'commit', '-q', '--allow-empty', '-m', 'valuable unfinished code')
        self.hold('GIT_HISTORY_NOT_PRESERVED')
        self.assertEqual(self.check(preserved_ref='refs/heads/legacy-done')['status'], 'CLEAR')
        self.hold('GIT_HISTORY_NOT_PRESERVED', preserved_ref='refs/remotes/origin/main')

    def test_git_lock_and_r2_deployment_conditions_stay_unresolved(self):
        self.git('-C', self.repo, 'worktree', 'lock', self.wt, '--reason', 'production hold')
        self.hold('GIT_WORKTREE_PROTECTED')
        self.git('-C', self.repo, 'worktree', 'unlock', self.wt)
        self.git('-C', self.wt, 'branch', '-m', 'fix/r2-production-old-name')
        result = self.hold('PRODUCTION_DEPLOYMENT_UNRESOLVED')
        self.assertEqual(result['deployment_conditions'], 'UNKNOWN')

    def test_external_evidence_and_git_refs_survive_normal_removal(self):
        evidence = self.reg + '/historical-evidence.json'
        with open(evidence, 'w') as h: h.write('incomplete old evidence')
        result = self.check(remove=True)
        self.assertEqual(result['outcome'], 'CHECKOUT_REMOVED', result)
        self.assertFalse(os.path.exists(self.wt))
        self.assertFalse(os.path.lexists(self.target))
        self.assertEqual(Path(evidence).read_text(), 'incomplete old evidence')
        self.git('-C', self.repo, 'rev-parse', '--verify', 'refs/heads/legacy-done')
        events = [e for e in self.m.read_history() if e['event'] == 'legacy_checkout_reconciliation']
        self.assertEqual([e['outcome'] for e in events], ['CHECKOUT_REMOVAL_INTENT', 'CHECKOUT_REMOVED'])
        self.assertTrue(all(e['historical_cache_retirement'] == 'UNKNOWN' for e in events))

    def test_changed_checkout_after_intent_cannot_authorize_removal(self):
        original = self.m.append_history
        def dirty(event):
            original(event)
            if event.get('outcome') == 'CHECKOUT_REMOVAL_INTENT':
                Path(self.wt, 'new-development.rs').write_text('keep')
        with patch.object(self.m, 'append_history', dirty):
            self.hold('CHECKOUT_NOT_CLEAN', remove=True)

    def test_ignored_evidence_arriving_during_final_census_is_preserved(self):
        excludes = Path(self.tmp, 'synthetic-excludes')
        excludes.write_text('late-evidence.json\n')
        self.git('-C', self.wt, 'config', 'core.excludesFile', str(excludes))
        scans = []
        evidence = Path(self.wt, 'late-evidence.json')
        def arrival(*args, **kw):
            scans.append(True)
            if len(scans) == 2:
                evidence.write_text('unique QA evidence')
            return self.process_view()
        with patch.object(self.m.F, 'Census', side_effect=arrival):
            result = self.hold('CHECKOUT_NOT_CLEAN', remove=True)
        self.assertEqual(result['outcome'], 'NOT_REMOVED')
        self.assertEqual(evidence.read_text(), 'unique QA evidence')

    def test_unknown_lock_and_unfinished_git_operation_fail_closed(self):
        admin = self.m.F.run_as_owner(['git', '-C', self.wt, 'rev-parse', '--absolute-git-dir'],
                                      capture_output=True, text=True).stdout.strip()
        Path(admin, 'index.lock').touch()
        self.hold('GIT_WORKTREE_PROTECTED')
        Path(admin, 'index.lock').unlink()
        with patch.object(self.m.F, 'open_lock_file', side_effect=PermissionError('unknown lock state')):
            self.hold('RECONCILIATION_UNVERIFIED')

    def test_unknown_recorded_build_and_r3_task_hold_without_claiming_active(self):
        record = self.reg + '/targets/' + self.key_of(self.target) + '.json'
        data = T.jload(record)
        data['active'] = [{'pid': 999999999, 'boot_id': None, 'starttime': None}]
        T.jdump(data, record)
        self.hold('RECORDED_BUILD_UNVERIFIED')
        data['active'] = []
        data['task'] = 'r3-deployment-not-resolved'
        T.jdump(data, record)
        self.hold('PRODUCTION_DEPLOYMENT_UNRESOLVED')


if __name__ == '__main__':
    unittest.main()
