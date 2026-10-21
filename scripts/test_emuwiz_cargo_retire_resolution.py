#!/usr/bin/env python3
"""Tests for journal resolution in scripts/emuwiz-cargo-retire.

Disposable synthetic targets only (the shared fixtures of test_emuwiz_cargo_retire);
no production journal, registry or cache is read or written.
"""
import contextlib
import hashlib
import io
import json
import os
import shutil
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_retire as R  # noqa: E402

def slurp(path):
    with open(path, "rb") as handle:
        return handle.read()


KILL_MID_ENTRY = (
    "state = {'entries': 0}\n"
    "def hook(stage):\n"
    "    if stage.startswith('before-entry'):\n"
    "        state['entries'] += 1\n"
    "    if stage.startswith('before-rmdir') and state['entries'] == 2:\n"
    "        os.kill(os.getpid(), signal.SIGKILL)\n"
    "mod._hook = hook\n"
)


class Resolution(R.Corrections):
    def interrupted(self, name="wt-res"):
        wt, t, path, digest = self.cache(name)
        r = self.run_driver(KILL_MID_ENTRY, path, digest)
        self.assertEqual(r.returncode, -9, r.stderr)
        (journal,) = self.journals()
        self.assertEqual(self.states(journal)["debug/.fingerprint"], "UNCERTAIN_PARTIALLY_REMOVED")
        return wt, t, path, digest, journal

    def resolve(self, journal, *extra, receipt_dir=None):
        args = ["--resolve-journal", journal, "--receipt-dir", receipt_dir or self.receipts]
        args.extend(extra)
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            rc = self.mod.main(args)
        return rc, json.loads(buffer.getvalue())

    def approve(self, journal, report, uncertain=None, note="reviewed in test"):
        extra = ["--approve-resolution", report["review_digest"], "--operator-note", note]
        for p in (report["uncertain"] if uncertain is None else uncertain):
            extra += ["--accept-uncertain", p]
        return self.resolve(journal, *extra)

    def codes(self, report):
        return {b["code"] for b in report["blockers"]}

    @staticmethod
    def tree(root):
        return sorted((dp, tuple(sorted(dn)), tuple(sorted(fn))) for dp, dn, fn in os.walk(root))

    # ---------------------------------------------------------------- flow
    def test_review_is_read_only_and_names_the_uncertain_entries(self):
        wt, t, path, digest, journal = self.interrupted()
        before = (slurp(journal), self.tree(t))
        rc, rep = self.resolve(journal)
        self.assertEqual((rc, rep["outcome"]), (0, "REVIEW_ONLY"))
        self.assertEqual(rep["uncertain"], [os.path.join(t, "debug/.fingerprint")])
        self.assertEqual((slurp(journal), self.tree(t)), before)
        self.assertFalse(os.path.exists(self.mod.resolution_path(journal)))

    def test_uncertain_entries_are_never_acknowledged_automatically(self):
        wt, t, path, digest, journal = self.interrupted()
        _, rep = self.resolve(journal)
        # digest approved but the uncertain path not named, or something extra named
        for accept in ([], [os.path.join(t, "debug/deps")],
                       [os.path.join(t, "debug/.fingerprint"), os.path.join(t, "debug/deps")]):
            rc, out = self.approve(journal, rep, uncertain=accept)
            self.assertEqual(rc, 8, accept)
            self.assertIn("UNCERTAIN_ENTRIES_NOT_EXPLICITLY_ACCEPTED", self.codes(out))
        rc, out = self.approve(journal, rep, note="  ")
        self.assertEqual(rc, 8)
        self.assertIn("OPERATOR_NOTE_REQUIRED", self.codes(out))
        self.assertFalse(os.path.exists(self.mod.resolution_path(journal)))
        # blocked by the incomplete journal as before
        rc, r2 = self.run_retire(path, digest, execute=True)
        self.assertEqual(r2["refused"]["code"], "INCOMPLETE_JOURNAL_PRESENT")

    def test_approved_resolution_unblocks_without_touching_journal_or_targets(self):
        wt, t, path, digest, journal = self.interrupted()
        _, rep = self.resolve(journal)
        before = (slurp(journal), self.tree(t))
        rc, out = self.approve(journal, rep)
        self.assertEqual((rc, out["outcome"]), (0, "RESOLVED"))
        self.assertEqual((slurp(journal), self.tree(t)), before,
                         "resolution must not alter the journal or delete anything")
        receipt = json.loads(slurp(out["receipt"]))
        self.assertEqual(receipt["journal_sha256"], hashlib.sha256(before[0]).hexdigest())
        self.assertEqual(receipt["accepted_uncertain"], rep["uncertain"])
        self.assertIn("authorises no retirement", receipt["authority"])
        self.assertEqual(oct(os.stat(out["receipt"]).st_mode & 0o777), "0o600")
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [])
        rc, r2 = self.run_retire(path, digest, execute=True)
        self.assertNotEqual(r2.get("refused", {}).get("code"), "INCOMPLETE_JOURNAL_PRESENT")

    def test_repeated_resolution_is_idempotent_and_never_overwrites(self):
        wt, t, path, digest, journal = self.interrupted()
        _, rep = self.resolve(journal)
        self.approve(journal, rep)
        first = slurp(self.mod.resolution_path(journal))
        for _ in range(2):
            rc, out = self.approve(journal, rep)
            self.assertEqual((rc, out["outcome"]), (0, "ALREADY_RESOLVED"))
        self.assertEqual(slurp(self.mod.resolution_path(journal)), first)

    def test_a_changed_journal_or_damaged_receipt_blocks_again(self):
        wt, t, path, digest, journal = self.interrupted()
        _, rep = self.resolve(journal)
        self.approve(journal, rep)
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [])
        receipt_path = self.mod.resolution_path(journal)
        good = slurp(receipt_path)
        with open(journal, "ab") as handle:
            handle.write(b'{"event": "note"}\n')
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])
        for damaged in (b"not json", b"{}", good.replace(b'"journal_sha256"', b'"x"')):
            with open(receipt_path, "wb") as handle:
                handle.write(damaged)
            self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])
            rc, out = self.resolve(journal)
            self.assertEqual(rc, 8)
            self.assertIn("RESOLUTION_RECEIPT_PRESENT_BUT_INVALID", self.codes(out))
            self.assertEqual(slurp(receipt_path), damaged, "never overwritten")

    def test_group_writable_or_symlinked_receipt_is_not_trusted(self):
        wt, t, path, digest, journal = self.interrupted()
        _, rep = self.resolve(journal)
        self.approve(journal, rep)
        receipt_path = self.mod.resolution_path(journal)
        os.chmod(receipt_path, 0o660)
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])
        os.chmod(receipt_path, 0o600)
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [])
        moved = receipt_path + ".real"
        os.rename(receipt_path, moved)
        os.symlink(moved, receipt_path)
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])

    # -------------------------------------------------- fresh validation
    def test_inode_change_after_review_invalidates_the_approval(self):
        wt, t, path, digest, journal = self.interrupted()
        _, rep = self.resolve(journal)
        victim = os.path.join(t, "debug/incremental")
        shutil.rmtree(victim)
        os.makedirs(victim)
        rc, out = self.approve(journal, rep)
        self.assertEqual(rc, 8)
        self.assertIn("APPROVAL_DIGEST_MISMATCH", self.codes(out))
        self.assertFalse(os.path.exists(self.mod.resolution_path(journal)))

    def test_replaced_target_directory_refuses(self):
        wt, t, path, digest, journal = self.interrupted()
        held = t + ".moved"
        os.rename(t, held)
        shutil.copytree(held, t, symlinks=True)
        rc, out = self.resolve(journal)
        self.assertEqual(rc, 8)
        self.assertIn("DIRECTORY_IDENTITY_CHANGED", self.codes(out))

    def test_symlinked_target_component_refuses(self):
        wt, t, path, digest, journal = self.interrupted()
        held = t + ".moved"
        os.rename(t, held)
        os.symlink(held, t)
        rc, out = self.resolve(journal)
        self.assertEqual(rc, 8)
        self.assertIn("TARGET_PATH_HAS_SYMLINK", self.codes(out))

    def test_active_target_incomplete_visibility_and_locks_refuse(self):
        wt, t, path, digest, journal = self.interrupted()
        base = self.mod.F.Census

        class Referenced(base):
            def __init__(inner, targets):
                super().__init__(targets)
                inner.refs = {os.path.realpath(t): [{"comm": "rustc", "pid": 4242, "kind": "cwd"}]}

        self.mod.F.Census = Referenced
        rc, out = self.resolve(journal)
        self.mod.F.Census = base
        self.assertEqual(rc, 8)
        self.assertIn("LIVE_PROCESS_REFERENCE", self.codes(out))

        class Blind(base):
            def __init__(inner, targets):
                super().__init__(targets)
                inner.unreadable_foreign = 3

        self.mod.F.Census = Blind
        rc, out = self.resolve(journal)
        self.mod.F.Census = base
        self.assertIn("INCOMPLETE_PROCESS_VISIBILITY", self.codes(out))

        real = self.mod.F.Registry.probe_lock
        self.mod.F.Registry.probe_lock = lambda self_, key: True
        try:
            rc, out = self.resolve(journal)
        finally:
            self.mod.F.Registry.probe_lock = real
        self.assertIn("BUILD_LOCK_HELD_OR_UNKNOWN", self.codes(out))
        self.assertFalse(os.path.exists(self.mod.resolution_path(journal)))

    def test_running_writer_and_foreign_or_complete_journals_refuse(self):
        wt, t, path, digest, journal = self.interrupted()
        real = self.mod._writer_may_be_running
        self.mod._writer_may_be_running = lambda p: 4242
        try:
            rc, out = self.resolve(journal)
        finally:
            self.mod._writer_may_be_running = real
        self.assertIn("JOURNAL_WRITER_MAY_BE_RUNNING", self.codes(out))
        # a journal outside the receipt directory
        other = os.path.join(self.tmp, "elsewhere")
        os.makedirs(other, mode=0o700)
        copy = os.path.join(other, os.path.basename(journal))
        shutil.copy(journal, copy)
        rc, out = self.resolve(copy)
        self.assertIn("JOURNAL_NOT_IN_RECEIPT_DIR", self.codes(out))
        # a complete journal has nothing to resolve
        wt2, t2, path2, digest2 = self.cache("wt-complete")
        self.run_retire(path2, digest2, execute=True, extra_args=["--acknowledge-incomplete-journals"])
        complete = [j for j in self.journals() if self.mod.inspect_journal(j)["complete"]]
        self.assertTrue(complete)
        rc, out = self.resolve(complete[0])
        self.assertIn("JOURNAL_ALREADY_COMPLETE", self.codes(out))

    def test_only_interrupted_removal_of_the_recorded_object_can_be_accepted(self):
        wt, t, path, digest, journal = self.interrupted()
        shutil.rmtree(os.path.join(t, "debug/.fingerprint"))  # path now absent, outcome unrecorded
        rc, out = self.resolve(journal)
        self.assertEqual(rc, 8)
        self.assertIn("UNRESOLVABLE_ENTRY", self.codes(out))
        rc, out = self.approve(journal, out)
        self.assertEqual(rc, 8)
        self.assertFalse(os.path.exists(self.mod.resolution_path(journal)))

    def test_a_held_lifecycle_lock_blocks_the_approved_write(self):
        wt, t, path, digest, journal = self.interrupted()
        _, rep = self.resolve(journal)
        lock = self.mod.F.Registry(create=False).lifecycle_lock_path(self.mod.F.key_for(t))
        os.makedirs(os.path.dirname(lock), exist_ok=True)
        fd = os.open(lock, os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, fd)
        self.mod.F.fcntl.flock(fd, self.mod.F.fcntl.LOCK_EX)
        rc, out = self.approve(journal, rep)
        self.assertEqual(rc, 8)
        self.assertIn("BUILD_LOCK_HELD_OR_UNKNOWN", self.codes(out))
        self.assertFalse(os.path.exists(self.mod.resolution_path(journal)))

    def test_hardening_fifo_forged_receipt_garbage_and_receipt_dir(self):
        wt, t, path, digest, journal = self.interrupted()
        rpath = self.mod.resolution_path(journal)
        os.mkfifo(rpath)  # would hang an unguarded open()
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])
        os.unlink(rpath)
        # a hand-written receipt with a valid hash but no review is not honoured
        forged = {"schema": self.mod.RESOLUTION_SCHEMA, "journal_name": os.path.basename(journal),
                  "journal_sha256": self.mod.journal_sha256(journal), "review_digest": "x"}
        with open(rpath, "w") as handle:
            json.dump(forged, handle)
        os.chmod(rpath, 0o600)
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])
        os.unlink(rpath)
        # empty / garbage journal has no execution identity
        junk = os.path.join(self.receipts, "retire-20200101T000000-1-1.journal.jsonl")
        with open(junk, "w") as handle:
            handle.write("garbage\n")
        os.chmod(junk, 0o600)
        rc, out = self.resolve(junk)
        self.assertIn("JOURNAL_HAS_NO_EXECUTION_IDENTITY", self.codes(out))
        os.unlink(junk)
        # group-writable receipt dir
        os.chmod(self.receipts, 0o770)
        rc, out = self.resolve(journal)
        os.chmod(self.receipts, 0o700)
        self.assertIn("RECEIPT_DIR_UNSAFE", self.codes(out))
        # maintain-style writer pid is recognised
        self.assertIsNone(self.mod._writer_may_be_running(journal))

    # ------------------------------------------------ authority boundary
    def test_resolving_a_journal_does_not_authorise_deleting_unrelated_targets(self):
        wt, t, path, digest, journal = self.interrupted()
        wt_b, t_b = self.managed_cache("wt-unrelated")
        path_b, digest_b = self.manifest([self.group(wt_b, t_b, self.RELS)], name="b.json")
        # a write after the manifest was made: the normal gate must still hold it
        with open(os.path.join(t_b, "debug", "deps", "late.o"), "w") as handle:
            handle.write("fresh")
        before_b = self.tree(t_b)
        _, rep = self.resolve(journal)
        rc, out = self.approve(journal, rep)
        self.assertEqual(out["outcome"], "RESOLVED")
        self.assertEqual(self.tree(t_b), before_b)
        rc, result = self.run_retire(path_b, digest_b, execute=True)
        self.assertEqual(self.tree(t_b), before_b, "the unrelated target must stay held")
        held = {h["code"] for g in result["groups"] for h in g["holds"]}
        self.assertIn("ACTIVITY_SINCE_MANIFEST", held)


def load_tests(loader, tests, pattern):
    """Only this file's own tests; the inherited fixtures' tests live in test_emuwiz_cargo_retire."""
    suite = unittest.TestSuite()
    for name in sorted(vars(Resolution)):
        if name.startswith("test_"):
            suite.addTest(Resolution(name))
    return suite


if __name__ == "__main__":
    unittest.main(verbosity=2)
