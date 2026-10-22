#!/usr/bin/env python3
"""Tests for restart reconciliation of interrupted executions in scripts/emuwiz-cargo-retire.

Disposable synthetic targets only (the shared fixtures of test_emuwiz_cargo_retire);
no production journal, registry or cache is read or written.
"""
import hashlib
import json
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_retire as R  # noqa: E402

# SIGTERM after the second entry's files are unlinked, before its directory goes: the
# 2026-10-21 16:22 shape (outcome PARTIAL with interrupted: true, then `interrupted`).
SIGTERM_MID_ENTRY = (
    "state = {'entries': 0}\n"
    "def hook(stage):\n"
    "    if stage.startswith('before-entry'):\n"
    "        state['entries'] += 1\n"
    "    if stage.startswith('before-rmdir') and state['entries'] == 2:\n"
    "        os.kill(os.getpid(), signal.SIGTERM)\n"
    "mod._hook = hook\n"
)
SIGKILL_MID_ENTRY = SIGTERM_MID_ENTRY.replace("SIGTERM", "SIGKILL")


def slurp(path):
    with open(path, "rb") as handle:
        return handle.read()


class Restart(R.Corrections):
    def interrupted(self, body=SIGTERM_MID_ENTRY, name="wt-restart"):
        wt, t, path, digest = self.cache(name)
        r = self.run_driver(body, path, digest)
        self.assertEqual(r.returncode, 143 if "SIGTERM" in body else -9, r.stderr)
        (journal,) = self.journals()
        return t, path, digest, journal

    def records(self, journal):
        return [json.loads(line) for line in slurp(journal).splitlines()]

    def rewrite(self, journal, records, renumber=True):
        if renumber:
            records = [dict(r, seq=i) for i, r in enumerate(records, 1)]
        with open(journal, "wb") as handle:
            handle.write(b"".join(json.dumps(r, sort_keys=True).encode() + b"\n" for r in records))

    def assert_pending(self, journal, why):
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])
        verdict = self.mod.clean_interruption(journal)
        self.assertFalse(verdict["reconciled"])
        self.assertIn(why, verdict["why"])

    # ------------------------------------------------------------ reconciled
    def test_clean_sigterm_mid_entry_reconciles_on_restart_without_touching_evidence(self):
        t, path, digest, journal = self.interrupted()
        self.assertEqual(self.events(journal)[-1], "interrupted")
        states = self.states(journal)
        self.assertEqual(states["debug/deps"], "KNOWN_COMPLETED")
        self.assertEqual(states["debug/.fingerprint"], "KNOWN_PARTIAL")
        self.assertEqual(states["debug/incremental"], "KNOWN_UNTOUCHED")
        before = slurp(journal)
        rc, r2 = self.run_retire(path, digest, execute=True)
        self.assertNotIn("refused", r2)
        self.assertEqual(slurp(journal), before, "the interrupted journal is evidence and is never modified")
        self.assertFalse(os.path.lexists(self.mod.resolution_path(journal)), "reconciliation writes no receipt")
        expected = [{"journal": journal, "journal_sha256": hashlib.sha256(before).hexdigest(),
                     "rule": "CLEAN_SELF_RECORDED_INTERRUPTION"}]
        self.assertEqual(r2["reconciled_interrupted"], expected)
        new = [j for j in self.journals() if j != journal]
        self.assertEqual(len(new), 1)
        begin = self.records(new[0])[0]
        self.assertEqual(begin["reconciled_interrupted"], expected)
        self.assertEqual(begin["acknowledged_incomplete"], [])
        self.assertEqual(self.events(new[0])[-1], "end")
        # replay: the same bytes give the same verdict on every later restart
        for _ in range(2):
            self.assertEqual(self.mod.journal_backlog(self.receipts), ([], expected))
        self.assertEqual(slurp(journal), before)

    def test_interrupt_between_entries_reconciles(self):
        wt, t, path, digest = self.cache("wt-restart-between")
        count = {"n": 0}

        def hook(stage):
            if stage.startswith("before-entry"):
                count["n"] += 1
                if count["n"] == 2:
                    raise KeyboardInterrupt()
        self.set_hook(hook)
        rc, _ = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 130)
        (journal,) = self.journals()
        self.assertTrue(self.mod.clean_interruption(journal)["reconciled"])
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [])

    def test_inspect_journal_reports_the_restart_verdict(self):
        t, path, digest, journal = self.interrupted()
        report = self.mod.inspect_journal(journal)
        self.assertEqual(report["restart_reconciliation"],
                         {"reconciled": True, "rule": "CLEAN_SELF_RECORDED_INTERRUPTION", "why": None})

    def test_reconciliation_does_not_bypass_the_executor_gates(self):
        t, path, digest, journal = self.interrupted()
        rc, r2 = self.run_retire(path, "0" * 64, execute=True)
        self.assertEqual(r2["refused"]["code"], "MANIFEST_DIGEST_MISMATCH")
        self.assertTrue(self.exists(t, "debug/incremental"))
        self.assertEqual(len(self.journals()), 1, "a refused run must not start a journal")

    # ------------------------------------------------------------ refused
    def test_sigkill_without_an_interrupted_record_stays_pending(self):
        t, path, digest, journal = self.interrupted(SIGKILL_MID_ENTRY, "wt-restart-kill")
        self.assert_pending(journal, "did not record its own interruption")
        rc, r2 = self.run_retire(path, digest, execute=True)
        self.assertEqual((rc, r2["refused"]["code"]), (6, "INCOMPLETE_JOURNAL_PRESENT"))
        self.assertTrue(self.exists(t, "debug/incremental"))

    def test_untrusted_journal_shapes_stay_pending(self):
        t, path, digest, journal = self.interrupted()
        good = self.records(journal)
        intents = [i for i, r in enumerate(good) if r["event"] == "intent"]
        outcomes = [i for i, r in enumerate(good) if r["event"] == "outcome"]
        partial = next(i for i in outcomes if good[i]["outcome"] == "PARTIAL")
        cases = {
            "torn": None,
            "the final record is torn": None,
            "did not record its own interruption": good[:-1] + [dict(good[0], event="note")],
            "no matching open intent": [r for i, r in enumerate(good) if i != intents[0]],
            "has an intent but no recorded outcome": [r for i, r in enumerate(good) if i != partial],
            "another host": [dict(good[0], host="elsewhere")] + good[1:],
            "second begin or an end": good[:-1] + [dict(good[0])] + good[-1:],
            "unplanned": good[:intents[0]] + [dict(good[intents[0]], path="/elsewhere")] + good[intents[0] + 1:],
            "not known from recorded outcomes": [
                dict(r, outcome="INTERRUPTED", files=0, dirs=0) if i == partial else r for i, r in enumerate(good)],
            "unexpected note record": good[:-1] + [{"event": "note"}] + good[-1:],
        }
        for why, records in cases.items():
            with self.subTest(why):
                if why == "torn":
                    self.rewrite(journal, good)
                    with open(journal, "ab") as handle:
                        handle.write(b'{"event": "outc')
                    self.assert_pending(journal, "torn")
                    continue
                if why == "the final record is torn":
                    self.rewrite(journal, good)
                    with open(journal, "rb+") as handle:
                        handle.truncate(os.path.getsize(journal) - 1)
                    self.assert_pending(journal, why)
                    continue
                self.rewrite(journal, records)
                self.assert_pending(journal, why)
        # gaps in the sequence (a record removed without renumbering) are refused too
        self.rewrite(journal, good[:2] + good[3:], renumber=False)
        self.assert_pending(journal, "not contiguous")
        self.rewrite(journal, good)
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [])

    def test_a_live_writer_keeps_the_journal_pending(self):
        t, path, digest, journal = self.interrupted()
        self.mod._writer_may_be_running = lambda p: 4242
        self.assert_pending(journal, "still running")

    def test_unsafe_journal_files_stay_pending(self):
        t, path, digest, journal = self.interrupted()
        os.chmod(journal, 0o620)
        self.assert_pending(journal, "group/world writable")
        os.chmod(journal, 0o600)
        # a symlink named like a journal is never followed
        moved = os.path.join(self.tmp, "elsewhere.journal.jsonl")
        os.rename(journal, moved)
        os.symlink(moved, journal)
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])
        self.assertFalse(self.mod.clean_interruption(journal)["reconciled"])

    def test_changing_a_reconciled_journal_is_judged_afresh(self):
        t, path, digest, journal = self.interrupted()
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [])
        with open(journal, "ab") as handle:
            handle.write(b'{"event": "note", "seq": 99}\n')
        self.assert_pending(journal, "not contiguous")

    # ------------------------------------------------------------ legacy receipts
    def test_deployed_schema_one_receipts_are_honoured_only_when_bound(self):
        t, path, digest, journal = self.interrupted(SIGKILL_MID_ENTRY, "wt-restart-legacy")
        sha = hashlib.sha256(slurp(journal)).hexdigest()
        legacy = {"schema": 1, "journal": os.path.basename(journal), "journal_sha256": sha,
                  "operator": "davedap (approved in test)", "accepted_uncertain": [], "note": "review record only"}
        receipt = self.mod.resolution_path(journal)
        for bad in ({"journal_sha256": "0" * 64}, {"journal": "other.journal.jsonl"}, {"operator": " "},
                    {"operator": None}, {"schema": 2}):
            with self.subTest(bad):
                with open(receipt, "w") as handle:
                    json.dump(dict(legacy, **bad), handle)
                os.chmod(receipt, 0o600)
                self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])
        with open(receipt, "w") as handle:
            json.dump(legacy, handle)
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [])
        with open(journal, "ab") as handle:
            handle.write(b'{"event": "note"}\n')
        self.assertEqual(self.mod.incomplete_journals(self.receipts), [journal])


if __name__ == "__main__":
    unittest.main()
