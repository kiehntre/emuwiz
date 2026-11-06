#!/usr/bin/env python3
"""Tests for the 2026-11-05 retirement reliability fix in scripts/emuwiz-cargo-retire.

Defect A: unlinking names in deps/ advanced the ctime of hardlinked incremental files, which the
tool's own evidence then read as an external change.  `incremental` is now retired first.
Defect B: a PARTIAL retirement that stopped after removing a session lock left the whole incremental
unit unrecognisable.  --resume-journal accepts that residue, and only that residue, from the
journal of the same approved manifest.

Disposable synthetic targets only (the shared fixtures of test_emuwiz_cargo_retire); no production
journal, registry or cache is read or written.
"""
import hashlib
import inspect
import json
import os
import shutil
import sys
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_retire as R  # noqa: E402

RELS = ["debug/deps", "debug/.fingerprint", "debug/incremental"]  # deps FIRST, as in the real manifest
INC = "debug/incremental"


def tree(path):
    """(relative path, size, mtime_ns, nlink) of everything below path, no ctime."""
    out = []
    for root, dirs, files in os.walk(path):
        for name in sorted(dirs + files):
            full = os.path.join(root, name)
            st = os.lstat(full)
            out.append((os.path.relpath(full, path), st.st_size, st.st_mtime_ns, st.st_nlink))
    return sorted(out)


class Residue(R.Base):
    # ----------------------------------------------------------------- fixtures
    def set_hook(self, fn):
        self.mod._hook = fn
        self.addCleanup(setattr, self.mod, "_hook", None)

    def journals(self):
        if not os.path.isdir(self.receipts):
            return []
        return sorted(os.path.join(self.receipts, n) for n in os.listdir(self.receipts) if n.endswith(".journal.jsonl"))

    def add_crate(self, t, crate, sid, link_to=None):
        """crate/session/lock in the observed rustc layout; optionally hardlink metadata.rmeta."""
        base = os.path.join(t, "debug", "incremental", crate)
        session = f"s-{sid}-{'0123456789abcdef' * 2}"
        os.makedirs(os.path.join(base, session))
        for name in ("dep-graph.bin", "query-cache.bin", "work-products.bin"):
            with open(os.path.join(base, session, name), "wb") as handle:
                handle.write(b"RSIC\x00\x00\x051.0.0" + b"x" * 1700)
        meta = os.path.join(base, session, "metadata.rmeta")
        if link_to:
            with open(link_to, "wb") as handle:
                handle.write(b"rust" + b"m" * 4000)
            os.link(link_to, meta)
        else:
            with open(meta, "wb") as handle:
                handle.write(b"rust" + b"m" * 4000)
        with open(os.path.join(base, f"s-{sid}.lock"), "wb"):
            pass
        return base, session

    def prepared(self, name, crates=3, link=True):
        wt, t = self.managed_cache(name)
        inc = os.path.join(t, "debug", "incremental")
        for index in range(crates):
            self.add_crate(t, f"crate{index}-0123456789abc", f"hnb{index}abcdef-0xwv2u{index}",
                           link_to=os.path.join(t, "debug", "deps", f"libshared{index}.rmeta") if link else None)
        self.age(t, 10)
        path, digest = self.manifest([self.group(wt, t, RELS)])
        return wt, t, path, digest

    def outcome(self, receipt, rel=INC):
        return self.outcomes(receipt)[rel]

    # ---- Defect A
    def test_a_hardlinks_between_deps_and_incremental_retire_cleanly(self):
        wt, t, path, digest = self.prepared("wt-a-links")
        rc, r = self.run_retire(path, digest, execute=True)
        out = self.outcomes(r)
        for rel in RELS:
            self.assertEqual(out[rel], ("REMOVED", ""), out)
        self.assertFalse(self.exists(t, INC))
        self.assertFalse(self.exists(t, "debug/deps"))
        # incremental was retired before deps: journal order is the execution order
        journal = self.journals()[-1]
        paths = [os.path.relpath(e["path"], t) for e in self.mod.read_journal(journal)[0] if e["event"] == "intent"]
        self.assertEqual(paths[0], INC, paths)

    def test_a_external_content_change_still_refuses(self):
        wt, t, path, digest = self.prepared("wt-a-content")

        def hook(stage):
            # Directory listing order differs between filesystems (ext4 vs tmpfs), so do not rely on
            # which file goes first: let the first unlink pass (progress), then change EVERY file.
            if not stage.startswith("before-unlink:"):
                return
            hook.calls = getattr(hook, "calls", 0) + 1
            if hook.calls == 2:
                for root, dirs, files in os.walk(os.path.join(t, "debug", "incremental")):
                    for name in files:
                        with open(os.path.join(root, name), "ab") as handle:
                            handle.write(b"external")
        self.set_hook(hook)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcome(r), ("PARTIAL", "INCREMENTAL_FILE_CHANGED"), r)
        self.assertTrue(self.exists(t, INC))

    def test_a_external_hardlink_ctime_only_change_still_refuses(self):
        wt, t, path, digest = self.prepared("wt-a-ctime", link=False)
        outside = os.path.join(self.tmp, "external-link")

        def hook(stage):
            if not stage.startswith("before-unlink:"):
                return
            hook.calls = getattr(hook, "calls", 0) + 1
            if hook.calls == 2:  # first unlink passes; then every file gets an extra link: content/mtime same, ctime and nlink move
                for root, dirs, files in os.walk(os.path.join(t, "debug", "incremental")):
                    for name in files:
                        os.link(os.path.join(root, name), f"{outside}-{len(os.listdir(self.tmp))}")
        self.set_hook(hook)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcome(r), ("PARTIAL", "INCREMENTAL_FILE_CHANGED"), r)
        self.assertTrue(any(n.startswith("external-link") for n in os.listdir(self.tmp)))

    # ---- Defect B
    def partial(self, name, crates=3):
        """Run until a session lock is gone and a file of its session changes: the 2026-11-05 shape."""
        wt, t, path, digest = self.prepared(name, crates=crates)
        outside = os.path.join(self.tmp, name + "-external-link")
        original = os.listdir

        def locks_first(arg="."):
            names = original(arg)
            return sorted(names, key=lambda n: (not n.endswith(".lock"), n)) if any(n.endswith(".lock") for n in names) else names

        def hook(stage):
            if not stage.startswith("before-unlink:") or getattr(hook, "done", False):
                return
            base = os.path.join(t, "debug", "incremental")
            for crate in original(base):
                entries = original(os.path.join(base, crate))
                for session in [e for e in entries if not e.endswith(".lock")]:
                    if f"{session.rsplit('-', 1)[0]}.lock" not in entries:  # lock gone, session still there
                        hook.done = True
                        # every file of that session, so the refusal never depends on which one is unlinked
                        # first (listing order differs between filesystems); the session stays intact
                        for n, name in enumerate(original(os.path.join(base, crate, session))):
                            os.link(os.path.join(base, crate, session, name), f"{outside}-{n}")
                        return
        self.set_hook(hook)
        with mock.patch.object(os, "listdir", locks_first):
            rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcome(r), ("PARTIAL", "INCREMENTAL_FILE_CHANGED"), r)
        (journal,) = self.journals()
        return wt, t, path, digest, journal

    def orphans(self, t):
        found = []
        base = os.path.join(t, "debug", "incremental")
        for crate in os.listdir(base):
            entries = os.listdir(os.path.join(base, crate))
            for session in [e for e in entries if not e.endswith(".lock")]:
                if f"{session.rsplit('-', 1)[0]}.lock" not in entries:
                    found.append((crate, session))
        return found

    def forged(self, journal, mutate, name="forged.journal.jsonl"):
        records = self.mod.read_journal(journal)[0]
        mutate(records)
        out = os.path.join(self.tmp, name)
        with open(out, "wb") as handle:
            os.fchmod(handle.fileno(), 0o600)
            handle.write(b"".join(json.dumps(r, sort_keys=True).encode() + b"\n" for r in records))
        return out

    def resume(self, path, digest, journal, execute=True):
        return self.run_retire(path, digest, execute=execute, extra_args=["--resume-journal", journal])

    def test_b_residue_is_refused_without_the_journal_and_resumed_with_it(self):
        wt, t, path, digest, journal = self.partial("wt-b-resume")
        (orphan,) = self.orphans(t)
        record = [r for r in self.mod.read_journal(journal)[0] if r["event"] == "outcome" and r["path"].endswith("/incremental")][0]
        lock_key = f"/{orphan[0]}/{orphan[1].rsplit('-', 1)[0]}.lock"
        self.assertIn(lock_key, record["removed_units"])  # new journals record what they unlinked
        # the defect: the unit is no longer recognised, so a plain run holds
        rc, r = self.run_retire(path, digest)
        self.assertEqual(self.outcome(r)[0], "HELD")
        self.assertIn("session/lock mismatch", self.outcome(r)[1])
        with open(journal, "rb") as handle:
            before = handle.read()
        # dry run with the journal selects, execute removes exactly the target's incremental
        # the real layout: deps and .fingerprint went in the first run, only incremental is left
        self.assertFalse(self.exists(t, "debug/deps") or self.exists(t, "debug/.fingerprint"))
        rc, r = self.resume(path, digest, journal, execute=False)
        self.assertEqual(self.outcome(r), ("SELECTED", ""), r)
        self.assertTrue(self.exists(t, INC))
        rc, r = self.resume(path, digest, journal)
        self.assertEqual(self.outcome(r), ("REMOVED", ""), r)
        for gone in ("debug/deps", "debug/.fingerprint"):  # already retired: not removed again, nothing refused
            self.assertIn(self.outcome(r, gone), (("ABSENT", "ALREADY_ABSENT"), ("HELD", "NOT_A_COMPILER_ARTIFACT")), r)
        self.assertNotIn("holds", {k for k, v in r["groups"][0].items() if k == "holds" and v}, r)
        self.assertFalse(self.exists(t, INC))
        with open(journal, "rb") as handle:
            self.assertEqual(handle.read(), before, "the original journal is preserved")
        self.assertEqual(r["resume_journal"]["journal"], os.path.realpath(journal))
        self.assertEqual(r["resume_journal"]["journal_sha256"], hashlib.sha256(before).hexdigest())

    def test_b_legacy_journal_without_removed_units_resumes_within_its_bound(self):
        wt, t, path, digest, journal = self.partial("wt-b-legacy")

        def strip(records):
            for rec in records:
                rec.pop("removed_units", None)
        legacy = self.forged(journal, strip)
        rc, r = self.resume(path, digest, legacy)
        self.assertEqual(self.outcome(r), ("REMOVED", ""), r)

    def test_b_legacy_bound_refuses_a_second_inconsistent_crate(self):
        wt, t, path, digest, journal = self.partial("wt-b-second")
        base = os.path.join(t, "debug", "incremental")
        victim = [c for c in sorted(os.listdir(base)) if not any(c == o[0] for o in self.orphans(t))][0]
        os.unlink(os.path.join(base, victim, [e for e in os.listdir(os.path.join(base, victim)) if e.endswith(".lock")][0]))
        legacy = self.forged(journal, lambda recs: [rec.pop("removed_units", None) for rec in recs])
        for journal_used in (journal, legacy):
            rc, r = self.resume(path, digest, journal_used)
            self.assertEqual(self.outcome(r)[0], "HELD", r)
            self.assertIn("more than one crate directory", self.outcome(r)[1])
            self.assertTrue(self.exists(t, INC))

    def test_b_exact_record_refuses_a_missing_lock_it_does_not_name(self):
        wt, t, path, digest, journal = self.partial("wt-b-unnamed")
        base = os.path.join(t, "debug", "incremental")
        crate, session = self.orphans(t)[0]
        # a second lock vanishes in the SAME crate (a second session) and is not in the journal
        other = f"s-hnbzzzzzzz-0xwv2u9-{'fedcba9876543210' * 2}"
        os.makedirs(os.path.join(base, crate, other))
        for name in ("dep-graph.bin", "query-cache.bin", "work-products.bin"):
            with open(os.path.join(base, crate, other, name), "wb") as handle:
                handle.write(b"RSIC\x00\x00\x051.0.0" + b"x" * 1700)
        self.age(t, 10)
        rc, r = self.resume(path, digest, journal)
        self.assertEqual(self.outcome(r)[0], "HELD", r)
        self.assertIn("not explained by the journal", self.outcome(r)[1])
        self.assertTrue(self.exists(t, INC))

    def test_b_journal_evidence_missing_or_mismatched_refuses(self):
        wt, t, path, digest, journal = self.partial("wt-b-bad")

        def outcome_of(recs):
            return [x for x in recs if x["event"] == "outcome" and x["path"].endswith("/incremental")][0]

        def intent_of(recs):
            return [x for x in recs if x["event"] == "intent" and x["path"].endswith("/incremental")][0]

        cases = {
            "missing file": os.path.join(self.tmp, "nope.journal.jsonl"),
            "other digest": self.forged(journal, lambda recs: recs[0].update(manifest_sha256="0" * 64), "f1.jsonl"),
            "other host": self.forged(journal, lambda recs: recs[0].update(host="elsewhere"), "f2.jsonl"),
            "not PARTIAL": self.forged(journal, lambda recs: outcome_of(recs).update(outcome="REMOVED"), "f3.jsonl"),
            "other reason": self.forged(journal, lambda recs: outcome_of(recs).update(reason="OS_ERROR"), "f4.jsonl"),
            "interrupted": self.forged(journal, lambda recs: outcome_of(recs).update(interrupted=True), "f5.jsonl"),
            "intent inode": self.forged(journal, lambda recs: intent_of(recs).update(inode=1), "f6.jsonl"),
            "no end": self.forged(journal, lambda recs: recs.pop(), "f7.jsonl"),
            "gap in seq": self.forged(journal, lambda recs: recs[3].update(seq=99), "f8.jsonl"),
            "plan inode": self.forged(journal, lambda recs: [x.update(inode=1) for x in recs if x["event"] == "group_plan"], "f9.jsonl"),
            "bad unit": self.forged(journal, lambda recs: outcome_of(recs).update(removed_units=["/../etc/passwd"]), "f10.jsonl"),
        }
        # a journal that is group/world writable is refused as well
        loose = self.forged(journal, lambda recs: None, "f11.jsonl")
        os.chmod(loose, 0o666)
        cases["writable"] = loose
        for label, bad in cases.items():
            rc, r = self.resume(path, digest, bad)
            self.assertEqual(rc, 3, label)
            self.assertEqual(r["refused"]["code"], "RESUME_JOURNAL_REFUSED", (label, r))
            self.assertTrue(self.exists(t, INC), label)
        # and nothing but the attested residue is ever accepted: without the flag it still holds
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcome(r)[0], "HELD")

    def test_b_journal_of_a_clean_run_does_not_open_a_foreign_target(self):
        # journal from target A, manifest for target B (different digest and plan): refused
        wt, t, path, digest, journal = self.partial("wt-b-a")
        wt2, t2, path2, digest2 = self.prepared("wt-b-b")
        base2 = os.path.join(t2, "debug", "incremental")
        crate = sorted(os.listdir(base2))[0]
        os.unlink(os.path.join(base2, crate, [e for e in os.listdir(os.path.join(base2, crate)) if e.endswith(".lock")][0]))
        before_b, before_a = tree(t2), tree(t)
        rc, r = self.resume(path2, digest2, journal)
        self.assertEqual((rc, r["refused"]["code"]), (3, "RESUME_JOURNAL_REFUSED"))
        self.assertEqual(tree(t2), before_b)
        self.assertEqual(tree(t), before_a)


    # ---- review fixes: legacy cap, opposite mismatch, truncated list, journal directory
    def lock_name(self, session):
        return session.rsplit("-", 1)[0] + ".lock"

    def bare_session(self, t, crate, sid):
        """A session without any lock: one more missing lock in the same crate."""
        base = os.path.join(t, "debug", "incremental", crate)
        session = f"s-{sid}-{'fedcba9876543210' * 2}"
        os.makedirs(os.path.join(base, session))
        for name in ("dep-graph.bin", "query-cache.bin", "work-products.bin"):
            with open(os.path.join(base, session, name), "wb") as handle:
                handle.write(b"RSIC\x00\x00\x051.0.0" + b"x" * 1700)
        self.age(t, 10)
        return session

    def lock_only(self, t):
        """The opposite residue: the session directory is gone and its lock is left behind."""
        (crate, session), = self.orphans(t)
        base = os.path.join(t, "debug", "incremental", crate)
        shutil.rmtree(os.path.join(base, session))
        with open(os.path.join(base, self.lock_name(session)), "wb"):
            pass
        self.age(t, 10)
        return crate, session

    def outcome_record(self, recs):
        return [x for x in recs if x["event"] == "outcome" and x["path"].endswith("/incremental")][0]

    def test_b_legacy_journal_accepts_exactly_one_missing_lock(self):
        wt, t, path, digest, journal = self.partial("wt-b-cap")
        (crate, _), = self.orphans(t)
        self.bare_session(t, crate, "hnbyyyyyyy-0xwv2u8")  # a second session without a lock
        legacy = self.forged(journal, lambda recs: self.outcome_record(recs).pop("removed_units", None))
        rc, r = self.resume(path, digest, legacy)
        self.assertEqual(self.outcome(r)[0], "HELD", r)
        self.assertIn("single pair", self.outcome(r)[1])
        self.assertTrue(self.exists(t, INC))
        # the exact journal names only the first lock: the second missing lock is not explained
        rc, r = self.resume(path, digest, journal)
        self.assertIn("not explained by the journal", self.outcome(r)[1])

    def test_b_truncated_list_takes_the_capped_legacy_branch(self):
        wt, t, path, digest, journal = self.partial("wt-b-trunc")

        def truncate(recs):
            record = self.outcome_record(recs)
            record.pop("removed_units", None)
            record["removed_units_truncated"] = True
        truncated = self.forged(journal, truncate)
        # a list that is also present is ignored when truncated: still exactly one pair
        also_listed = self.forged(journal, lambda recs: self.outcome_record(recs).update(removed_units_truncated=True), "listed.jsonl")
        (crate, _), = self.orphans(t)
        extra = self.bare_session(t, crate, "hnbyyyyyyy-0xwv2u8")
        for label, bad in (("truncated", truncated), ("truncated+listed", also_listed)):
            rc, r = self.resume(path, digest, bad)
            self.assertEqual(self.outcome(r)[0], "HELD", (label, r))
            self.assertIn("single pair", self.outcome(r)[1])
        shutil.rmtree(os.path.join(t, "debug", "incremental", crate, extra))  # back to one missing lock
        self.age(t, 10)
        for bad in (truncated, also_listed):
            rc, r = self.resume(path, digest, bad, execute=False)
            self.assertEqual(self.outcome(r), ("SELECTED", ""), r)
        rc, r = self.resume(path, digest, truncated)
        self.assertEqual(self.outcome(r), ("REMOVED", ""), r)

    def test_b_lock_left_without_its_session_needs_the_journal_to_attest_the_session(self):
        wt, t, path, digest, journal = self.partial("wt-b-lockonly")
        crate, session = self.lock_only(t)
        key = f"/{crate}/{session}"
        legacy = self.forged(journal, lambda recs: self.outcome_record(recs).pop("removed_units", None))
        rc, r = self.resume(path, digest, legacy)                      # legacy cannot attest a session removal
        self.assertIn("single pair", self.outcome(r)[1], r)
        rc, r = self.resume(path, digest, journal)                     # the lock is attested, the session is not
        self.assertIn("not explained by the journal", self.outcome(r)[1], r)
        self.assertTrue(self.exists(t, INC))
        attested = self.forged(journal, lambda recs: self.outcome_record(recs)["removed_units"].append(key), "attested.jsonl")
        rc, r = self.resume(path, digest, attested)
        self.assertEqual(self.outcome(r), ("REMOVED", ""), r)
        self.assertFalse(self.exists(t, INC))

    def test_b_several_lock_only_mismatches_refuse(self):
        wt, t, path, digest, journal = self.partial("wt-b-locks")
        crate, session = self.lock_only(t)
        base = os.path.join(t, "debug", "incremental", crate)
        with open(os.path.join(base, "s-hnbqqqqqqq-0xwv2uq.lock"), "wb"):
            pass
        self.age(t, 10)
        legacy = self.forged(journal, lambda recs: self.outcome_record(recs).pop("removed_units", None))
        rc, r = self.resume(path, digest, legacy)
        self.assertEqual(self.outcome(r)[0], "HELD", r)
        self.assertIn("single pair", self.outcome(r)[1])
        rc, r = self.resume(path, digest, journal)
        self.assertIn("not explained by the journal", self.outcome(r)[1])
        self.assertTrue(self.exists(t, INC))

    def test_b_journal_directory_must_be_safe(self):
        wt, t, path, digest, journal = self.partial("wt-b-dir")
        for label, mode in (("group writable", 0o770), ("world writable", 0o707)):
            directory = os.path.join(self.tmp, "j-" + label.split()[0])
            os.makedirs(directory)
            copy = os.path.join(directory, "copy.journal.jsonl")
            shutil.copyfile(journal, copy)
            os.chmod(copy, 0o600)
            os.chmod(directory, mode)
            rc, r = self.resume(path, digest, copy)
            self.assertEqual((rc, r["refused"]["code"]), (3, "RESUME_JOURNAL_REFUSED"), label)
            self.assertIn("directory", r["refused"]["detail"])
            os.chmod(directory, 0o700)
            rc, r = self.resume(path, digest, copy, execute=False)  # the same bytes in a safe directory pass
            self.assertEqual(self.outcome(r), ("SELECTED", ""), r)
        # a symlinked directory is not a real directory
        real = os.path.join(self.tmp, "j-real")
        os.makedirs(real)
        shutil.copyfile(journal, os.path.join(real, "copy.journal.jsonl"))
        os.chmod(os.path.join(real, "copy.journal.jsonl"), 0o600)
        link = os.path.join(self.tmp, "j-link")
        os.symlink(real, link)
        rc, r = self.resume(path, digest, os.path.join(link, "copy.journal.jsonl"))
        self.assertEqual((rc, r["refused"]["code"]), (3, "RESUME_JOURNAL_REFUSED"))
        self.assertTrue(self.exists(t, INC))

    def test_b_journal_is_read_once_and_the_digest_is_of_the_validated_bytes(self):
        wt, t, path, digest, journal = self.partial("wt-b-once")
        reads = []
        original = self.mod._read_regular

        def counting(p, limit):
            reads.append(p)
            return original(p, limit)
        with mock.patch.object(self.mod, "_read_regular", counting):
            rc, r = self.resume(path, digest, journal, execute=False)
        self.assertEqual(reads.count(journal), 1, reads)
        with open(journal, "rb") as handle:
            self.assertEqual(r["resume_journal"]["journal_sha256"], hashlib.sha256(handle.read()).hexdigest())

    # ---- scope
    def test_scope_resume_touches_only_the_authorised_target(self):
        wt, t, path, digest, journal = self.partial("wt-scope-a")
        wt2, t2 = self.managed_cache("wt-scope-b")  # another cache, intact
        self.age(t2, 10)
        other, worktrees = tree(t2), {w: tree(w) for w in (wt, wt2)}
        victim = tree(self.victim)
        rc, r = self.resume(path, digest, journal)
        self.assertEqual(self.outcome(r), ("REMOVED", ""), r)
        self.assertEqual(tree(t2), other)
        self.assertEqual({w: tree(w) for w in (wt, wt2)}, worktrees)
        self.assertEqual(tree(self.victim), victim)
        # only incremental of the authorised target went; everything the manifest did not name stays
        self.assertTrue(self.exists(t, "debug/emuwiz-test-env"))
        self.assertTrue(self.exists(t, "debug/.cargo-lock"))


class Defaults(R.Base):
    def test_strict_recognition_is_the_default_and_residue_is_opt_in(self):
        self.assertIs(inspect.signature(self.mod.F.incremental_evidence).parameters["residue"].default, None)
        self.assertEqual(self.mod.RESIDUE, {})  # nothing is attested unless --resume-journal named a verified journal


if __name__ == "__main__":
    unittest.main()
