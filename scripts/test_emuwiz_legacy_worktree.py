#!/usr/bin/env python3
"""Legacy checkout reconciliation: private synthetic Git/registry fixtures only."""
import contextlib
import fcntl
import io
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

    # --- history that `git worktree remove` deletes with the checkout -------------

    def commit_in_wt(self, name):
        Path(self.wt, name).write_text(name + '\n')
        self.git('-C', self.wt, 'add', name)
        self.git('-C', self.wt, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', name)
        return self.m.F.head_of(self.wt)

    def admin(self):
        return subprocess.run(['git', '-C', self.wt, 'rev-parse', '--absolute-git-dir'],
                              capture_output=True, text=True, check=True).stdout.strip()

    def test_reflog_only_commit_holds_until_retained_by_a_ref(self):
        main = self.m.F.head_of(self.wt)
        self.git('-C', self.wt, 'checkout', '-q', '--detach')
        lost = self.commit_in_wt('lost.txt')
        self.git('-C', self.wt, 'checkout', '-q', '--detach', main)
        self.hold('GIT_HISTORY_NOT_PRESERVED')
        self.git('-C', self.wt, 'update-ref', 'refs/preservation/lost', lost)
        self.assertEqual(self.check()['status'], 'CLEAR')

    def test_old_id_of_the_only_surviving_reflog_entry_is_protected(self):
        main = self.m.F.head_of(self.wt)
        self.git('-C', self.wt, 'checkout', '-q', '--detach')
        self.commit_in_wt('x.txt')
        y = self.commit_in_wt('y.txt')
        self.git('-C', self.wt, 'checkout', '-q', '--detach', main)
        log = Path(self.admin(), 'logs', 'HEAD')
        last = log.read_text().splitlines()[-1]
        self.assertTrue(last.startswith(y))
        log.write_text(last + '\n')   # expiry left only the "y -> main" entry
        self.hold('GIT_HISTORY_NOT_PRESERVED')

    def test_orig_head_must_be_an_object_id(self):
        Path(self.admin(), 'ORIG_HEAD').write_text('--all\n')
        self.hold('GIT_STATE_UNVERIFIED')

    def test_per_worktree_refs_and_sequencer_hold(self):
        self.git('-C', self.wt, 'update-ref', 'refs/worktree/keep', 'HEAD')
        self.hold('GIT_HISTORY_NOT_PRESERVED')
        self.git('-C', self.wt, 'update-ref', '-d', 'refs/worktree/keep')
        self.git('-C', self.wt, 'update-ref', 'refs/bisect/bad', 'HEAD')
        self.hold('GIT_HISTORY_NOT_PRESERVED')
        self.git('-C', self.wt, 'update-ref', '-d', 'refs/bisect/bad')
        os.mkdir(Path(self.admin(), 'sequencer'))
        self.hold('GIT_WORKTREE_PROTECTED')

    def test_replace_refs_cannot_fake_merged_history(self):
        main = self.m.F.head_of(self.wt)
        unique = self.commit_in_wt('unique.txt')
        self.git('-C', self.wt, 'update-ref', 'refs/remotes/origin/main', main)
        self.git('-C', self.wt, 'replace', unique, main)
        self.hold('GIT_HISTORY_NOT_PRESERVED')

    def test_grafts_cannot_fake_merged_history(self):
        main = self.m.F.head_of(self.wt)
        unique = self.commit_in_wt('unique.txt')
        self.git('-C', self.wt, 'checkout', '-q', '--detach', main)
        self.git('-C', self.wt, 'update-ref', 'refs/preservation/unique', unique)
        self.git('-C', self.wt, '-c', 'user.name=t', '-c', 'user.email=t@t',
                 'commit', '-q', '--allow-empty', '-m', 'next')
        tip = self.m.F.head_of(self.wt)
        self.git('-C', self.wt, 'update-ref', 'refs/remotes/origin/main', tip)
        self.git('-C', self.wt, 'checkout', '-q', '--detach', unique)
        common = subprocess.run(['git', '-C', self.wt, 'rev-parse', '--path-format=absolute', '--git-common-dir'],
                                capture_output=True, text=True, check=True).stdout.strip()
        os.makedirs(os.path.join(common, 'info'), exist_ok=True)
        Path(common, 'info', 'grafts').write_text(f'{tip} {unique}\n')   # claims unique is on main
        self.hold('GIT_HISTORY_NOT_PRESERVED')

    def test_inherited_git_dir_cannot_redirect_the_checks(self):
        main = self.m.F.head_of(self.wt)
        self.commit_in_wt('unique.txt')
        self.git('-C', self.wt, 'update-ref', 'refs/remotes/origin/main', main)
        other = Path(self.wt).parent / 'other-repo'
        subprocess.run(['git', 'init', '-q', '-b', 'main', str(other)], check=True)
        subprocess.run(['git', '-C', str(other), '-c', 'user.name=t', '-c', 'user.email=t@t',
                        'commit', '-q', '--allow-empty', '-m', 'x'], check=True)
        subprocess.run(['git', '-C', str(other), 'update-ref', 'refs/remotes/origin/main', 'HEAD'], check=True)
        with patch.dict(os.environ, {'GIT_DIR': str(other / '.git'), 'GIT_WORK_TREE': str(self.wt)}):
            self.hold('GIT_HISTORY_NOT_PRESERVED')


class HistoryGates(LegacyCheckout):
    """Both gates must prove the checkout's history before any early return."""
    def setUp(self):
        super().setUp()
        self.env = dict(os.environ, GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t',
                        GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t')
        self.base = self.m.F.head_of(self.wt)
        self.git_dir = subprocess.check_output(
            ['git', '-C', self.wt, 'rev-parse', '--absolute-git-dir'], text=True).strip()

    def run_git(self, *args):
        return subprocess.check_output(['git', '-C', self.wt, *args], text=True,
                                       env=self.env, stderr=subprocess.STDOUT).strip()

    def reflog_only_commit(self):
        self.run_git('commit', '--allow-empty', '-qm', 'history only in the reflog')
        orphan = self.run_git('rev-parse', 'HEAD')
        self.run_git('reset', '-q', '--hard', self.base)
        os.unlink(os.path.join(self.git_dir, 'ORIG_HEAD'))
        return orphan

    def gate_when_lifecycle_clear(self):
        clear = dict(worktree=self.wt, target=self.target, worktree_exists=True,
                     status='CLEAR', code='CLEAR', explanation='fixture')
        with patch.object(self.m, 'lifecycle_gate', return_value=clear):
            return self.m.worktree_gate(self.wt)

    def assert_gate_holds(self, code):
        gate = self.gate_when_lifecycle_clear()
        self.assertEqual((gate['status'], gate['code']), ('HOLD', code), gate)

    def test_clean_merged_checkout_stays_clear(self):
        self.assertEqual(self.gate_when_lifecycle_clear()['status'], 'CLEAR')
        self.assertEqual(self.check()['status'], 'CLEAR')

    def test_reflog_only_commit_holds_the_lifecycle_gate(self):
        self.reflog_only_commit()
        self.assert_gate_holds('GIT_HISTORY_NOT_PRESERVED')

    def test_present_target_cannot_hide_reflog_only_history(self):
        self.reflog_only_commit()
        os.mkdir(self.target)
        self.hold('GIT_HISTORY_NOT_PRESERVED')

    def test_missing_artifacts_hold_still_reports_the_history_problem(self):
        self.reflog_only_commit()
        gate = self.m.worktree_gate(self.wt)
        self.assertEqual(gate['code'], 'ARTIFACTS_MISSING_UNVERIFIED')
        self.assertEqual(gate['history_preservation']['code'], 'GIT_HISTORY_NOT_PRESERVED')

    def test_orig_head_only_commit_holds(self):
        orphan = self.reflog_only_commit()
        os.unlink(os.path.join(self.git_dir, 'logs', 'HEAD'))
        Path(self.git_dir, 'ORIG_HEAD').write_text(orphan + '\n')
        self.assert_gate_holds('GIT_HISTORY_NOT_PRESERVED')

    def test_detached_head_kept_only_by_this_checkout_holds(self):
        self.run_git('checkout', '-q', '--detach')
        self.run_git('commit', '--allow-empty', '-qm', 'detached')
        os.unlink(os.path.join(self.git_dir, 'logs', 'HEAD'))
        self.assert_gate_holds('GIT_HISTORY_NOT_PRESERVED')

    def test_symlinked_reflog_or_orig_head_fails_closed(self):
        for name in ('logs/HEAD', 'ORIG_HEAD'):
            with self.subTest(name=name):
                path = os.path.join(self.git_dir, name)
                saved = path + '.saved'
                if os.path.lexists(path):
                    os.rename(path, saved)
                    self.addCleanup(os.replace, saved, path)
                os.symlink('/nonexistent', path)
                self.assert_gate_holds('GIT_STATE_UNVERIFIED')
                os.unlink(path)

    def never_built(self, name):
        """A real checkout with no target and no record: the lifecycle gate is CLEAR."""
        wt = os.path.join(os.path.dirname(self.wt), name)
        self.run_git('worktree', 'add', '-q', '--detach', wt, self.base)
        self.assertEqual(self.m.lifecycle_gate(wt)['status'], 'CLEAR')
        return wt

    def cli(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                rc = self.m.main(list(argv))
            except SystemExit as exit_:
                rc = exit_.code
        return rc, out.getvalue() + err.getvalue()

    def test_cli_exit_code_holds_unpatched(self):
        wt = self.never_built('cli-hold')
        self.assertEqual(self.cli('worktree-check', wt)[0], 0)
        Path(wt, 'unsaved').write_text('work')
        rc, text = self.cli('worktree-check', wt)
        self.assertEqual(rc, self.m.EXIT_WORKTREE_HOLD, text)
        self.assertIn('HOLD', text)

    def test_cargo_state_only_is_not_blocked_by_git_state(self):
        wt = self.never_built('cargo-only')
        Path(wt, 'unsaved').write_text('work')
        self.assertEqual(self.cli('worktree-check', '--cargo-state-only', wt)[0], 0)

    def test_cargo_iso_gives_a_dirty_new_worktree_low_debug(self):
        wt = self.never_built('cargo-iso-new')
        scripts = os.path.dirname(os.path.abspath(self.m.__file__))
        Path(wt, 'unsaved').write_text('work')
        out = subprocess.run([os.path.join(scripts, 'cargo-iso'), '--print-target'], cwd=wt,
                             capture_output=True, text=True)
        self.assertRegex(out.stdout + out.stderr, r'debug: +low \(no compiler output to keep')

    def test_task_complete_on_never_built_dirty_worktree_succeeds_and_reports_hold(self):
        wt = self.never_built('finish-dirty')
        Path(wt, 'unsaved').write_text('work')
        rc, text = self.cli('task-complete', '--worktree', wt, '--task', 'fixture')
        self.assertEqual(rc, 0, text)
        self.assertIn('worktree: HOLD', text)
        self.assertIn('KEEP this worktree', text)

    def test_text_output_reports_history_behind_a_cargo_hold(self):
        self.reflog_only_commit()
        text = self.cli('worktree-check', self.wt)[1]
        self.assertIn('ARTIFACTS_MISSING', self.m.worktree_gate(self.wt)['code'])
        self.assertIn('history: GIT_HISTORY_NOT_PRESERVED', text)

    def test_hidden_index_edits_hold_the_lifecycle_gate(self):
        source = Path(self.wt, 'source.rs')
        source.write_text('committed')
        self.run_git('add', 'source.rs')
        self.run_git('commit', '-qm', 'source')
        self.run_git('update-ref', 'refs/remotes/origin/main', 'HEAD')
        for flag, undo in [('--assume-unchanged', '--no-assume-unchanged'),
                           ('--skip-worktree', '--no-skip-worktree')]:
            with self.subTest(flag=flag):
                self.run_git('update-index', flag, 'source.rs')
                source.write_text('UNSAVED')
                self.assert_gate_holds('CHECKOUT_INDEX_UNVERIFIED')
                self.run_git('update-index', undo, 'source.rs')
                source.write_text('committed')

    @unittest.skipIf(os.geteuid() == 0, 'root ignores directory modes')
    def test_unreadable_checkout_fails_closed(self):
        os.chmod(self.wt, 0)
        self.addCleanup(os.chmod, self.wt, 0o755)
        self.assert_gate_holds('GIT_STATE_UNVERIFIED')

    def test_recorded_checkout_that_lost_its_git_entry_holds(self):
        self.reflog_only_commit()
        os.unlink(os.path.join(self.wt, '.git'))
        self.assert_gate_holds('GIT_STATE_UNVERIFIED')

    def test_plain_directory_and_vanished_path_are_not_checkouts(self):
        plain = os.path.join(self.root_for_bundle(), 'plain')
        os.mkdir(plain)
        self.assertIsNone(self.m.history_preservation_problem(plain))
        self.assertIsNone(self.m.history_preservation_problem(plain + '-gone'))

    def test_empty_git_directory_or_subdirectory_cannot_borrow_the_parent_repo(self):
        nested = os.path.join(self.wt, 'nested')
        os.mkdir(nested)
        self.assertEqual(self.m.history_preservation_problem(nested)[0], 'CHECKOUT_PATH_UNVERIFIED')
        os.mkdir(os.path.join(nested, '.git'))
        self.assertEqual(self.m.history_preservation_problem(nested)[0], 'CHECKOUT_PATH_UNVERIFIED')

    def test_unfinished_operations_hold_the_lifecycle_gate(self):
        for name in self.m.UNFINISHED_GIT_STATE:
            with self.subTest(name=name):
                marker = Path(self.git_dir, name)
                marker.touch()
                self.assert_gate_holds('GIT_WORKTREE_PROTECTED')
                marker.unlink()

    def test_git_file_pointing_at_another_checkout_holds(self):
        other = self.never_built('other-checkout')
        other_git = Path(other, '.git').read_text()
        Path(self.wt, '.git').write_text(other_git)
        self.assert_gate_holds('CHECKOUT_PATH_UNVERIFIED')

    def test_gate_does_not_write_the_index(self):
        source = Path(self.wt, 'tracked.txt')
        source.write_text('x')
        self.run_git('add', 'tracked.txt')
        self.run_git('commit', '-qm', 'tracked')
        index = os.path.join(self.git_dir, 'index')
        os.utime(source)  # stale stat data: an optional refresh would rewrite the index
        before = os.stat(index).st_mtime_ns
        self.m.history_preservation_problem(self.wt)
        self.assertEqual(os.stat(index).st_mtime_ns, before)

    def test_commit_retained_under_refs_preservation_clears(self):
        orphan = self.reflog_only_commit()
        self.run_git('update-ref', 'refs/preservation/fixture', orphan)
        self.assertEqual(self.gate_when_lifecycle_clear()['status'], 'CLEAR')
        self.assertEqual(self.check()['status'], 'CLEAR')

    def test_bundle_counts_only_once_imported_under_refs_preservation(self):
        orphan = self.reflog_only_commit()
        self.run_git('update-ref', 'refs/tmp-bundle', orphan)
        bundle = os.path.join(self.root_for_bundle(), 'history.bundle')
        self.run_git('bundle', 'create', '-q', bundle, 'refs/tmp-bundle', '^' + self.base)
        self.run_git('bundle', 'verify', '-q', bundle)
        self.run_git('update-ref', '-d', 'refs/tmp-bundle')
        self.assert_gate_holds('GIT_HISTORY_NOT_PRESERVED')
        self.run_git('fetch', '-q', bundle, 'refs/tmp-bundle:refs/preservation/from-bundle')
        self.assertEqual(self.gate_when_lifecycle_clear()['status'], 'CLEAR')

    def root_for_bundle(self):
        return os.path.dirname(self.wt)

    def test_reflog_naming_a_deleted_object_fails_closed(self):
        gone = 'a' * 40
        with open(os.path.join(self.git_dir, 'logs', 'HEAD'), 'a') as log:
            log.write(f'{gone} {self.base} t <t@t> 1 +0000\tcheckout: fixture\n')
        self.assert_gate_holds('GIT_STATE_UNVERIFIED')
        self.hold('GIT_STATE_UNVERIFIED')

    def test_uncommitted_untracked_and_ignored_work_holds(self):
        tracked = 'tracked-file'
        Path(self.wt, tracked).write_bytes(b'original')
        self.run_git('add', tracked)
        self.run_git('commit', '-qm', 'tracked fixture')
        self.run_git('update-ref', 'refs/remotes/origin/main', 'HEAD')
        original = Path(self.wt, tracked).read_bytes()
        Path(self.wt, tracked).write_bytes(original + b'changed')
        self.assert_gate_holds('CHECKOUT_NOT_CLEAN')
        os.mkdir(self.target)
        self.hold('CHECKOUT_NOT_CLEAN')
        os.rmdir(self.target)
        Path(self.wt, tracked).write_bytes(original)
        Path(self.wt, 'untracked-work').write_text('x')
        self.assert_gate_holds('CHECKOUT_NOT_CLEAN')
        os.unlink(os.path.join(self.wt, 'untracked-work'))
        exclude = self.run_git('rev-parse', '--git-path', 'info/exclude')
        with open(os.path.join(self.wt, exclude) if not os.path.isabs(exclude) else exclude, 'a') as f:
            f.write('ignored-evidence\n')
        Path(self.wt, 'ignored-evidence').write_text('x')
        self.assert_gate_holds('CHECKOUT_NOT_CLEAN')

    def test_git_that_cannot_answer_fails_closed(self):
        with patch.object(self.m.F, 'run_as_owner', side_effect=OSError('no git')):
            self.assert_gate_holds('GIT_STATE_UNVERIFIED')

    def test_malformed_reflog_fails_closed(self):
        with open(os.path.join(self.git_dir, 'logs', 'HEAD'), 'a') as log:
            log.write('not-an-id also-not\tfixture\n')
        self.assert_gate_holds('GIT_STATE_UNVERIFIED')


if __name__ == '__main__':
    unittest.main()
