#!/usr/bin/env python3
"""Tests for scripts/emuwiz-cargo-retire (the manifest-authorised executor).

Synthetic worktrees, fake cargo, private HOME and temp targets only; nothing here
touches ~/.cache/emuwiz-cargo-target(s) or any real cache.
"""
import hashlib
import importlib.machinery
import importlib.util
import json
import os
import signal
import stat
import subprocess
import sys
import time
import unittest
from datetime import datetime, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_retention as T  # noqa: E402

RETIRE = os.path.join(HERE, "emuwiz-cargo-retire")
DAY = 86400


def load_retire():
    loader = importlib.machinery.SourceFileLoader("emuwiz_cargo_retire_under_test", RETIRE)
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


class Base(T.Env):
    def setUp(self):
        super().setUp()
        self.receipts = os.path.join(self.tmp, "receipts")
        for k in ("HOME", "EMUWIZ_TARGET_ROOT", "EMUWIZ_RETENTION_DIR", "EMUWIZ_OWNER"):
            old = os.environ.get(k)
            self.addCleanup(self.restore, k, old)
            os.environ[k] = self.env[k]
        os.environ.pop("EMUWIZ_RETENTION_NOW", None)
        self.mod = load_retire()
        self.full_visibility()
        self.victim = os.path.join(self.tmp, "victim")
        os.makedirs(self.victim)
        with open(os.path.join(self.victim, "precious.db"), "w") as handle:
            handle.write("keep")

    @staticmethod
    def restore(key, value):
        if value is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = value

    def full_visibility(self):
        base = self.mod.F.Census

        class Full(base):
            def __init__(inner, targets, **kw):
                super().__init__(targets, **kw)
                inner.unreadable_foreign = 0

        self.mod.F.Census = Full

    # --- fixtures
    def managed_cache(self, name="wt-r", age_days=10, build=True, completed=True):
        """A cargo-iso managed cache whose record is genuinely produced by the wrapper."""
        wt = self.worktree(name)
        target = self.target_of(wt)
        self.make_cache(target, old_days=age_days)
        with open(os.path.join(target, "debug", ".cargo-lock"), "w"):
            pass
        with open(os.path.join(target, ".rustc_info.json"), "w") as handle:
            handle.write("{}")
        if build:
            self.assertEqual(self.iso(wt, "check").returncode, 0)
        self.age(target, age_days)
        if build:
            self.age_record(target, age_days)
            if completed:
                self.assertEqual(self.ret("finish", wt, "--task", "completed-fixture").returncode, 0)
        return wt, os.path.realpath(target)

    def age_record(self, target, days):
        path = os.path.join(self.reg, "targets", self.key_of(target) + ".json")
        rec = T.jload(path)
        for k in ("last_use", "last_end", "last_start", "first_seen"):
            if k in rec:
                rec[k] = time.time() - days * DAY
        T.jdump(rec, path)

    def manifest(self, groups, name="manifest.json", digest_ok=True):
        data = {"mode": "PROPOSAL_ONLY", "cargo_artifact_groups": groups}
        path = os.path.join(self.tmp, name)
        with open(path, "w") as handle:
            json.dump(data, handle)
        with open(path, "rb") as handle:
            return path, hashlib.sha256(handle.read()).hexdigest()

    def group(self, wt, target, rels=None, extra=None):
        st = os.lstat(target)
        rels = rels or ["debug/deps", "debug/.fingerprint", "debug/incremental", "debug/emuwiz",
                        "debug/emuwiz.d", "debug/.cargo-lock", ".rustc_info.json"]
        paths = []
        for rel in rels:
            full = os.path.join(target, rel)
            s = os.lstat(full)
            paths.append({"path": full, "device": s.st_dev, "inode": s.st_ino, "mode": s.st_mode})
        paths.extend(extra or [])
        newest = max(os.lstat(os.path.join(r, n)).st_mtime
                     for r, d, f in os.walk(target) for n in d + f)
        return {
            "directory": target, "device": st.st_dev, "inode": st.st_ino,
            "source_head": self.mod.F.head_of(wt), "source_exists": True,
            "associated_source_paths": json.dumps([os.path.realpath(wt)]),
            "latest_mtime_utc": datetime.fromtimestamp(newest, timezone.utc).isoformat(),
            "deletion_paths_json": json.dumps(paths),
        }

    def run_retire(self, path, digest, *extra, execute=False, window=False, extra_args=()):
        args = ["--manifest", path, "--approved-sha256", digest, "--receipt-dir", self.receipts, "--json"]
        args.extend(extra_args)
        if execute:
            args.append("--execute")
        if window:
            args.append("--maintenance-window-attested")
        args.extend(extra)
        import io
        import contextlib
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            rc = self.mod.main(args)
        return rc, json.loads(buffer.getvalue())

    def outcomes(self, receipt):
        return {os.path.relpath(p["path"], g["directory"]): (p["outcome"], p["reason"])
                for g in receipt["groups"] for p in g["paths"]}

    def exists(self, target, rel):
        return os.path.lexists(os.path.join(target, rel))


class NoRepoWrites(unittest.TestCase):
    def test_executor_never_writes_bytecode_into_the_scripts_directory(self):
        cache = os.path.join(HERE, "__pycache__")
        before = set(os.listdir(cache)) if os.path.isdir(cache) else set()
        env = {k: v for k, v in os.environ.items() if k != "PYTHONDONTWRITEBYTECODE"}
        r = subprocess.run([sys.executable, RETIRE, "--help"], env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        after = set(os.listdir(cache)) if os.path.isdir(cache) else set()
        self.assertEqual({n for n in after - before if "cargo-retention" in n}, set())


class Retirement(Base):
    def test_synthetic_retirement_removes_artifacts_and_preserves_evidence(self):
        wt, t = self.managed_cache()
        path, digest = self.manifest([self.group(wt, t)])
        # dry run first: nothing changes, a receipt is written
        rc, dry = self.run_retire(path, digest)
        self.assertEqual(rc, 0)
        self.assertEqual(dry["groups"][0]["outcome"], "WOULD_RETIRE")
        self.assertTrue(self.exists(t, "debug/deps/libx.rlib"))
        self.assertTrue(os.path.exists(dry["receipt_file"]))
        rc, done = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 0)
        out = self.outcomes(done)
        for rel in ("debug/deps", "debug/.fingerprint", "debug/incremental", "debug/emuwiz", "debug/emuwiz.d",
                    ".rustc_info.json"):
            self.assertEqual(out[rel][0], "REMOVED", (rel, out[rel]))
            self.assertFalse(self.exists(t, rel), rel)
        self.assertEqual(out["debug/.cargo-lock"], ("HELD", "CARGO_INTERNAL_LOCK_FILE"))
        self.assertTrue(self.exists(t, "debug/.cargo-lock"))
        # synthetic recovery evidence and unknown files are untouched
        for rel in ("debug/emuwiz-test-env/profile/library.db", "debug/emuwiz-test-env/profile/journal.jsonl",
                    "debug/UNKNOWN.txt"):
            self.assertTrue(self.exists(t, rel), rel)
        g = done["groups"][0]
        self.assertGreater(g["accounted_freed_bytes"], 40000)
        self.assertEqual(g["outcome"], "RETIRED")
        self.assertIsNotNone(g["filesystem_free_before"])
        receipt = T.jload(done["receipt_file"])
        self.assertEqual(receipt["totals"]["paths_in_manifest"], 7)

    def test_hardlinks_to_outside_files_are_not_counted_as_freed_and_survive(self):
        wt, t = self.managed_cache("wt-hl2")
        outside = os.path.join(self.tmp, "outside-link")
        os.link(os.path.join(t, "debug", "deps", "libx.rlib"), outside)
        self.age(t, 10)
        path, digest = self.manifest([self.group(wt, t, ["debug/deps"])])
        rc, done = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(done)["debug/deps"][0], "REMOVED")
        self.assertTrue(os.path.exists(outside))
        self.assertGreater(done["groups"][0]["retained_hardlink_bytes"], 0)

    def test_dry_run_never_deletes_and_assume_adopted_is_dry_run_only(self):
        wt, t = self.managed_cache("wt-dry", build=False)
        path, digest = self.manifest([self.group(wt, t)])
        rc, dry = self.run_retire(path, digest, "--assume-adopted")
        self.assertEqual(rc, 0)
        self.assertIn("ASSUMED", dry["groups"][0]["note"])
        self.assertTrue(self.exists(t, "debug/deps"))
        with self.assertRaises(SystemExit):
            self.run_retire(path, digest, "--assume-adopted", execute=True, window=True)
        with self.assertRaises(SystemExit):
            os.environ["EMUWIZ_RETENTION_NOW"] = str(time.time())
            try:
                self.run_retire(path, digest, execute=True)
            finally:
                os.environ.pop("EMUWIZ_RETENTION_NOW")


class Authorisation(Base):
    def test_manifest_tampering_and_digest(self):
        wt, t = self.managed_cache("wt-tamper")
        path, digest = self.manifest([self.group(wt, t)])
        with open(path, "a") as handle:
            handle.write(" ")
        rc, receipt = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 3)
        self.assertEqual(receipt["refused"]["code"], "MANIFEST_DIGEST_MISMATCH")
        self.assertTrue(self.exists(t, "debug/deps"))
        rc, receipt = self.run_retire(path, "0" * 64, execute=True)
        self.assertEqual(receipt["refused"]["code"], "MANIFEST_DIGEST_MISMATCH")
        self.assertTrue(self.exists(t, "debug/deps"))

    def test_an_approved_but_malicious_entry_is_still_refused_per_path(self):
        wt, t = self.managed_cache("wt-evil")
        vstat = os.lstat(os.path.join(self.victim, "precious.db"))
        evil = [
            {"path": os.path.join(t, "debug", "..", "..", "victim", "precious.db"),
             "device": vstat.st_dev, "inode": vstat.st_ino, "mode": vstat.st_mode},
            {"path": os.path.join(self.victim, "precious.db"),
             "device": vstat.st_dev, "inode": vstat.st_ino, "mode": vstat.st_mode},
            {"path": os.path.join(t, "debug", "emuwiz-test-env"), "device": 0, "inode": 0, "mode": 0},
            {"path": os.path.join(t, "debug", "UNKNOWN.txt"), "device": 0, "inode": 0, "mode": 0},
            {"path": os.path.join(t, "debug", "deps"), "device": 1, "inode": 1, "mode": 16893},  # wrong identity
            {"path": t, "device": 0, "inode": 0, "mode": 0},
        ]
        path, digest = self.manifest([self.group(wt, t, rels=["debug/incremental"], extra=evil)])
        rc, done = self.run_retire(path, digest, execute=True)
        out = {p["path"]: (p["outcome"], p["reason"]) for p in done["groups"][0]["paths"]}
        self.assertEqual(out[evil[0]["path"]][0], "HELD")
        self.assertEqual(out[evil[1]["path"]], ("HELD", "PATH_OUTSIDE_TARGET"))
        self.assertEqual(out[evil[2]["path"]][1], "NOT_A_COMPILER_ARTIFACT")
        self.assertEqual(out[evil[3]["path"]][1], "NOT_A_COMPILER_ARTIFACT")
        self.assertEqual(out[evil[4]["path"]][1], "PATH_IDENTITY_MISMATCH")
        self.assertEqual(out[evil[5]["path"]][0], "HELD")
        self.assertTrue(os.path.exists(os.path.join(self.victim, "precious.db")))
        for rel in ("debug/emuwiz-test-env/profile/library.db", "debug/UNKNOWN.txt", "debug/deps"):
            self.assertTrue(self.exists(t, rel), rel)
        self.assertFalse(self.exists(t, "debug/incremental"))

    def test_untracked_adoption_only_and_unverified_ownership_hold(self):
        wt, t = self.managed_cache("wt-own", build=False)
        path, digest = self.manifest([self.group(wt, t)])
        rc, r = self.run_retire(path, digest, execute=True, window=True)
        codes = {h["code"] for h in r["groups"][0]["holds"]}
        self.assertIn("NOT_ADOPTED", codes)
        self.assertTrue(self.exists(t, "debug/deps"))
        # adoption record naming someone, but the source HEAD in the manifest is wrong
        self.assertEqual(self.ret("adopt", t, "--owner", "tester").returncode, 0)
        self.age_record(t, 10)
        bad = self.group(wt, t)
        bad["source_head"] = "0" * 40
        path2, digest2 = self.manifest([bad], "m2.json")
        rc, r = self.run_retire(path2, digest2, execute=True, window=True)
        self.assertIn("OWNER_UNVERIFIED", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))
        # source checkout gone -> ownership cannot be verified
        gone = self.group(wt, t)
        gone["source_exists"] = False
        path3, digest3 = self.manifest([gone], "m3.json")
        rc, r = self.run_retire(path3, digest3, execute=True, window=True)
        self.assertIn("SOURCE_MISSING", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))

    def test_external_target_refused_even_with_adoption_and_window(self):
        wt = self.worktree("wt-legacy")
        real = os.path.realpath(wt)
        name = "wt-legacy-" + hashlib.sha256(real.encode()).hexdigest()[:12]
        custom = os.path.join(self.tmp, "custom-root", name)
        self.make_cache(custom, old_days=10)
        t = os.path.realpath(custom)
        with open(os.path.join(t, "debug", ".cargo-lock"), "w"):
            pass
        with open(os.path.join(t, ".rustc_info.json"), "w") as handle:
            handle.write("{}")
        self.age(t, 10)
        path, digest = self.manifest([self.group(wt, t)])
        # adopt through the CLI (extra target), then age the adoption record
        r = self.ret("adopt", t, "--owner", "tester", "--extra-target", t)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.age_record(t, 10)
        self.assertEqual(self.ret("finish", t, "--extra-target", t).returncode, 0)
        rc, rec = self.run_retire(path, digest, execute=True)
        self.assertIn("NONPARTICIPATING_WRITERS_NOT_EXCLUDED", {h["code"] for h in rec["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))
        rc, rec = self.run_retire(path, digest, execute=True, window=True)
        self.assertIn("SHARED_OR_EXTERNAL_TARGET", {h["code"] for h in rec["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))
        self.assertTrue(self.exists(t, "debug/emuwiz-test-env/profile/library.db"))

    def test_recent_activity_pins_and_activity_since_manifest_hold(self):
        wt, t = self.managed_cache("wt-hold")
        path, digest = self.manifest([self.group(wt, t)])
        self.assertEqual(self.ret("pin", wt, "--reason", "independent-review").returncode, 0)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertIn("PINNED", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertEqual(self.ret("release", wt).returncode, 0)
        os.utime(os.path.join(t, "debug", "deps", "libx.rlib"))  # write now
        rc, r = self.run_retire(path, digest, execute=True)
        codes = {h["code"] for h in r["groups"][0]["holds"]}
        self.assertIn("ACTIVITY_SINCE_MANIFEST", codes)
        self.assertTrue(self.exists(t, "debug/deps/libx.rlib"))

    def test_incomplete_process_visibility_holds_with_the_real_census(self):
        wt, t = self.managed_cache("wt-vis")
        path, digest = self.manifest([self.group(wt, t)])
        census = importlib.machinery.SourceFileLoader("fresh", RETIRE).load_module()
        rc, r = self.mod_run_real(census, path, digest)
        gap = r["process_census"]["unreadable_foreign"]
        codes = {h["code"] for h in r["groups"][0]["holds"]}
        if gap:
            self.assertIn("INCOMPLETE_PROCESS_VISIBILITY", codes)
            self.assertTrue(self.exists(t, "debug/deps"))
        else:
            self.assertNotIn("INCOMPLETE_PROCESS_VISIBILITY", codes)

    def mod_run_real(self, module, path, digest):
        import io
        import contextlib
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            rc = module.main(["--manifest", path, "--approved-sha256", digest, "--json", "--execute",
                              "--receipt-dir", self.receipts])
        return rc, json.loads(buffer.getvalue())


class RegistryCorruption(Base):
    def test_corrupt_pin_record_holds_and_nothing_is_deleted(self):
        wt, t = self.managed_cache("wt-badpin")
        path, digest = self.manifest([self.group(wt, t)])
        pins = os.path.join(self.reg, "pins")
        os.makedirs(pins, exist_ok=True)
        with open(os.path.join(pins, self.key_of(t) + ".json"), "w") as handle:
            handle.write("{ this is not json")
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertIn("PIN_STATE_UNKNOWN", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))
        self.assertTrue(self.exists(t, "debug/emuwiz"))

    def test_wrong_shape_pin_record_holds(self):
        wt, t = self.managed_cache("wt-shapepin")
        path, digest = self.manifest([self.group(wt, t)])
        os.makedirs(os.path.join(self.reg, "pins"), exist_ok=True)
        T.jdump({"pins": "yes"}, os.path.join(self.reg, "pins", self.key_of(t) + ".json"))
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertIn("PIN_STATE_UNKNOWN", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))

    def test_malformed_lifecycle_record_holds_without_crashing(self):
        wt, t = self.managed_cache("wt-badrec")
        path, digest = self.manifest([self.group(wt, t)])
        keyfile = os.path.join(self.reg, "targets", self.key_of(t) + ".json")
        rec = T.jload(keyfile)
        rec["active"] = ["not-a-dict", 7]
        T.jdump(rec, keyfile)
        rc, r = self.run_retire(path, digest, execute=True)
        codes = {h["code"] for h in r["groups"][0]["holds"]}
        self.assertIn("REGISTRY_RECORD_INVALID", codes)
        self.assertTrue(self.exists(t, "debug/deps"))
        with open(keyfile, "w") as handle:
            handle.write("\x00garbage")
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertIn("REGISTRY_RECORD_INVALID", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))


class Concurrency(Base):
    def test_active_build_blocks_and_a_new_build_waits_for_the_executor(self):
        wt, t = self.managed_cache("wt-conc")
        path, digest = self.manifest([self.group(wt, t)])
        busy = subprocess.Popen([T.ISO, "check"], cwd=wt, env=dict(self.env, FAKE_SLEEP="3"),
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.0)
        rc, r = self.run_retire(path, digest, execute=True)
        busy.wait()
        self.assertIn("LIFECYCLE_LOCK_UNAVAILABLE", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))
        self.age(t, 10)
        self.age_record(t, 10)
        # now free: a build that starts DURING the deletion must wait for it
        self.assertEqual(self.ret("finish", wt).returncode, 0)
        started = {}

        def hook(stage):
            if stage == "after-open-target" and "p" not in started:
                started["p"] = subprocess.Popen([T.ISO, "check"], cwd=wt, env=self.env,
                                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                time.sleep(0.8)
                started["alive_during"] = started["p"].poll() is None
        self.mod._hook = hook
        self.addCleanup(setattr, self.mod, "_hook", None)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(r["groups"][0]["outcome"], "HELD", r["groups"][0]["holds"])
        self.assertIn("LIVE_PROCESS_REFERENCE", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))
        self.assertTrue(started["alive_during"], "the new build must block on the exclusive lock")
        self.assertEqual(started["p"].wait(timeout=30), 0)

    def test_stale_active_entry_from_a_crashed_process_does_not_block(self):
        wt, t = self.managed_cache("wt-crash")
        keyfile = os.path.join(self.reg, "targets", self.key_of(t) + ".json")
        rec = T.jload(keyfile)
        rec["active"] = [{"token": "x", "pid": 999999, "starttime": 1, "boot_id": "gone"}]
        T.jdump(rec, keyfile)
        path, digest = self.manifest([self.group(wt, t)])
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(r["groups"][0]["outcome"], "RETIRED", r["groups"][0]["holds"])

    def test_orphaned_cargo_child_of_a_killed_wrapper_still_blocks(self):
        wt, t = self.managed_cache("wt-orphan")
        path, digest = self.manifest([self.group(wt, t)])
        p = subprocess.Popen([T.ISO, "check"], cwd=wt, env=dict(self.env, FAKE_SLEEP="4"),
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.0)
        p.kill()
        p.wait()
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertIn("LIFECYCLE_LOCK_UNAVAILABLE", {h["code"] for h in r["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))

    def test_a_running_executable_blocks(self):
        wt, t = self.managed_cache("wt-run")
        binary = os.path.join(t, "debug", "emuwiz")
        with open("/bin/sleep", "rb") as src, open(binary, "wb") as dst:
            dst.write(src.read())
        os.chmod(binary, 0o755)
        self.age(t, 10)
        path, digest = self.manifest([self.group(wt, t)])
        proc = subprocess.Popen([binary, "30"])
        try:
            time.sleep(0.3)
            rc, r = self.run_retire(path, digest, execute=True)
            self.assertIn("LIVE_PROCESS_REFERENCE", {h["code"] for h in r["groups"][0]["holds"]})
            self.assertTrue(self.exists(t, "debug/deps"))
        finally:
            proc.kill()
            proc.wait()

    def test_a_crash_during_deletion_releases_the_lock(self):
        wt, t = self.managed_cache("wt-die")
        path, digest = self.manifest([self.group(wt, t)])

        def hook(stage):
            if stage.startswith("before-entry"):
                raise RuntimeError("simulated crash")
        self.mod._hook = hook
        self.addCleanup(setattr, self.mod, "_hook", None)
        with self.assertRaises(RuntimeError):
            self.run_retire(path, digest, execute=True)
        import fcntl
        fd = os.open(os.path.join(self.reg, "locks", self.key_of(t) + ".lock"), os.O_RDWR)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(fd)


class Corrections(Base):
    """F1/F2/F3 of the acceptance of 546bf01d: truthful partial deletion, durable
    interruption evidence and a deletion-time protected-file guard."""

    RELS = ["debug/deps", "debug/.fingerprint", "debug/incremental"]

    def set_hook(self, fn):
        self.mod._hook = fn
        self.addCleanup(setattr, self.mod, "_hook", None)

    def journals(self):
        if not os.path.isdir(self.receipts):
            return []
        return sorted(os.path.join(self.receipts, n) for n in os.listdir(self.receipts) if n.endswith(".journal.jsonl"))

    def events(self, journal):
        return [r["event"] for r in self.mod.read_journal(journal)[0]]

    def states(self, journal):
        return {os.path.relpath(e["path"], e["directory"]): e["state"]
                for e in self.mod.inspect_journal(journal)["entries"]}

    def cache(self, name, rels=None):
        wt, t = self.managed_cache(name)
        path, digest = self.manifest([self.group(wt, t, rels or self.RELS)])
        return wt, t, path, digest

    # ---- F1
    def test_f1_late_entry_after_progress_is_partial_and_accounting_agrees(self):
        for index, late in enumerate(("late.txt", "late-journal.jsonl")):
            wt, t, path, digest = self.cache(f"wt-f1-{index}", ["debug/deps"])
            fired = []

            def hook(stage, t=t, late=late):
                if stage.startswith("before-unlink") and not fired:
                    fired.append(1)
                    with open(os.path.join(t, "debug", "deps", late), "w") as handle:
                        handle.write("late")
            self.set_hook(hook)
            rc, r = self.run_retire(path, digest, execute=True)
            g = r["groups"][0]
            entry = g["paths"][0]
            self.assertEqual(entry["outcome"], "PARTIAL", entry)
            self.assertEqual(g["outcome"], "PARTIAL")
            self.assertEqual(g["paths_partial"], 1)
            self.assertEqual(r["totals"]["paths_partial"], 1)
            self.assertEqual(entry["files"], 2)  # libx.rlib and x.d were really removed
            self.assertEqual(g["files_removed"], entry["files"])
            self.assertGreater(g["accounted_freed_bytes"], 0)
            self.assertEqual(g["accounted_freed_bytes"], entry["freed_bytes"])
            self.assertEqual(sorted(os.listdir(os.path.join(t, "debug", "deps"))), [late])
            outs = [x for x in self.mod.read_journal(r["journal_file"])[0] if x["event"] == "outcome"]
            self.assertEqual(sum(x["files"] for x in outs), g["files_removed"])
            self.assertEqual(outs[0]["outcome"], "PARTIAL")

    def test_f1_reviewer_symlink_probes_report_consistently(self):
        for i, name in enumerate(["a", "h", "planted", "zz"]):
            wt, t, path, digest = self.cache(f"wt-f1s-{i}", ["debug/deps"])
            before = set(os.listdir(os.path.join(t, "debug", "deps")))

            def hook(stage, t=t, name=name):
                if stage.startswith("before-entry"):
                    os.symlink(self.victim, os.path.join(t, "debug", "deps", name))
            self.set_hook(hook)
            rc, r = self.run_retire(path, digest, execute=True)
            g = r["groups"][0]
            left = set(os.listdir(os.path.join(t, "debug", "deps"))) - {name}
            removed = before - left
            if removed:
                self.assertEqual(g["paths"][0]["outcome"], "PARTIAL", name)
            else:
                self.assertIn(g["paths"][0]["outcome"], ("REFUSED",), name)
                self.assertEqual(g["outcome"], "NOTHING_REMOVED")
            self.assertTrue(os.path.exists(os.path.join(self.victim, "precious.db")))

    # ---- F3
    def test_f3_late_protected_or_unknown_file_is_never_deleted(self):
        cases = {"late-journal.jsonl": "PROTECTED_FILE", "late.db": "PROTECTED_FILE",
                 "save.sav": "PROTECTED_FILE", "late.txt": "NOT_IN_PLANNED_SNAPSHOT"}
        for i, (late, code) in enumerate(cases.items()):
            wt, t, path, digest = self.cache(f"wt-f3-{i}", ["debug/deps"])
            planted = os.path.join(t, "debug", "deps", late)

            def hook(stage, planted=planted):
                if stage.startswith("before-entry"):
                    with open(planted, "w") as handle:
                        handle.write("precious")
            self.set_hook(hook)
            rc, r = self.run_retire(path, digest, execute=True)
            entry = r["groups"][0]["paths"][0]
            self.assertEqual((entry["outcome"], entry["reason"]), ("REFUSED", code), late)
            with open(planted) as handle:
                self.assertEqual(handle.read(), "precious")
            self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib")))
            self.assertEqual(r["groups"][0]["outcome"], "NOTHING_REMOVED")

    def test_f3_a_file_swapped_for_another_inode_before_unlink_is_refused(self):
        wt, t, path, digest = self.cache("wt-f3-swap", ["debug/deps"])
        victim_file = os.path.join(t, "debug", "deps", "libx.rlib")

        def hook(stage):
            if stage == "before-unlink:libx.rlib":
                # keep the old inode alive so the replacement cannot reuse its number
                os.link(victim_file, os.path.join(self.tmp, "old-inode-keeper"))
                os.unlink(victim_file)
                with open(victim_file, "w") as handle:
                    handle.write("replacement")
        self.set_hook(hook)
        rc, r = self.run_retire(path, digest, execute=True)
        with open(victim_file) as handle:
            self.assertEqual(handle.read(), "replacement")
        self.assertIn(r["groups"][0]["paths"][0]["outcome"], ("REFUSED", "PARTIAL"))

    # ---- F2
    def test_f2_keyboard_interrupt_leaves_journal_and_receipt_with_progress(self):
        wt, t, path, digest = self.cache("wt-f2-int")
        count = {"n": 0}

        def hook(stage):
            if stage.startswith("before-entry"):
                count["n"] += 1
                if count["n"] == 2:
                    raise KeyboardInterrupt()
        self.set_hook(hook)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 130)
        (journal,) = self.journals()
        ev = self.events(journal)
        self.assertEqual(ev[:2], ["begin", "group_plan"])
        self.assertIn("interrupted", ev)
        self.assertNotIn("end", ev)
        states = self.states(journal)
        self.assertEqual(states["debug/deps"], "KNOWN_COMPLETED")
        self.assertEqual(states["debug/.fingerprint"], "KNOWN_UNTOUCHED")
        self.assertEqual(states["debug/incremental"], "KNOWN_UNTOUCHED")
        self.assertFalse(self.exists(t, "debug/deps"))
        self.assertTrue(self.exists(t, "debug/.fingerprint"))
        self.assertTrue(r["interrupted"])
        self.assertGreater(r["groups"][0]["accounted_freed_bytes"], 0)

    def test_f2_sigterm_is_journalled_like_an_interrupt(self):
        import signal
        self.addCleanup(signal.signal, signal.SIGTERM, signal.getsignal(signal.SIGTERM))
        self.addCleanup(signal.signal, signal.SIGHUP, signal.getsignal(signal.SIGHUP))
        wt, t, path, digest = self.cache("wt-f2-term")
        count = {"n": 0}

        def hook(stage):
            if stage.startswith("before-entry"):
                count["n"] += 1
                if count["n"] == 2:
                    os.kill(os.getpid(), signal.SIGTERM)
        self.set_hook(hook)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 143)
        (journal,) = self.journals()
        self.assertIn("interrupted", self.events(journal))
        self.assertEqual(self.states(journal)["debug/deps"], "KNOWN_COMPLETED")

    def driver(self, body):
        """Run the executor in a child process whose hook may kill it."""
        script = os.path.join(self.tmp, "driver.py")
        with open(script, "w") as handle:
            handle.write(
                "import sys, os, signal, importlib.machinery, importlib.util\n"
                "sys.dont_write_bytecode = True\n"
                "RETIRE, ARGS = sys.argv[1], sys.argv[2:]\n"
                "loader = importlib.machinery.SourceFileLoader('r', RETIRE)\n"
                "spec = importlib.util.spec_from_loader('r', loader)\n"
                "mod = importlib.util.module_from_spec(spec); loader.exec_module(mod)\n"
                "base = mod.F.Census\n"
                "class Full(base):\n"
                "    def __init__(self, t, **kw):\n"
                "        super().__init__(t, **kw); self.unreadable_foreign = 0\n"
                "mod.F.Census = Full\n"
                + body +
                "sys.exit(mod.main(ARGS))\n"
            )
        return script

    def run_driver(self, body, path, digest):
        script = self.driver(body)
        return subprocess.run(
            [sys.executable, "-I", script, RETIRE, "--manifest", path, "--approved-sha256", digest,
             "--receipt-dir", self.receipts, "--json", "--execute"],
            env=self.env, capture_output=True, text=True)

    def test_f2_sigkill_mid_entry_leaves_classifiable_evidence(self):
        wt, t, path, digest = self.cache("wt-f2-kill")
        body = (
            "state = {'entries': 0}\n"
            "def hook(stage):\n"
            "    if stage.startswith('before-entry'):\n"
            "        state['entries'] += 1\n"
            "    if stage.startswith('before-rmdir') and state['entries'] == 2:\n"
            "        os.kill(os.getpid(), signal.SIGKILL)\n"
            "mod._hook = hook\n"
        )
        r = self.run_driver(body, path, digest)
        self.assertEqual(r.returncode, -9, r.stderr)
        (journal,) = self.journals()
        ev = self.events(journal)
        self.assertNotIn("end", ev)
        # entry 1 fully recorded; entry 2 had an intent, files unlinked, directory
        # still present, no outcome: deletion started, extent uncertain; entry 3 never started
        states = self.states(journal)
        self.assertEqual(states["debug/deps"], "KNOWN_COMPLETED")
        self.assertEqual(states["debug/.fingerprint"], "UNCERTAIN_PARTIALLY_REMOVED")
        self.assertEqual(states["debug/incremental"], "KNOWN_UNTOUCHED")
        self.assertTrue(self.exists(t, "debug/.fingerprint"))
        # the next execution refuses until the incomplete journal is acknowledged
        rc, r2 = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 6)
        self.assertEqual(r2["refused"]["code"], "INCOMPLETE_JOURNAL_PRESENT")
        self.assertTrue(self.exists(t, "debug/incremental"))
        before = open(journal, "rb").read()
        rc, r3 = self.run_retire(path, digest, execute=True, extra_args=["--acknowledge-incomplete-journals"])
        self.assertEqual(open(journal, "rb").read(), before, "incomplete evidence must be preserved")
        self.assertNotIn("refused", r3)

    def test_f2_sigkill_before_first_unlink_is_known_untouched(self):
        wt, t, path, digest = self.cache("wt-f2-kill0", ["debug/deps"])
        body = (
            "def hook(stage):\n"
            "    if stage.startswith('before-unlink'):\n"
            "        os.kill(os.getpid(), signal.SIGKILL)\n"
            "mod._hook = hook\n"
        )
        r = self.run_driver(body, path, digest)
        self.assertEqual(r.returncode, -9, r.stderr)
        (journal,) = self.journals()
        self.assertIn("intent", self.events(journal))
        self.assertEqual(self.states(journal)["debug/deps"], "KNOWN_UNTOUCHED")

    def test_f2_absent_path_without_outcome_is_uncertain_never_assumed_complete(self):
        wt, t, path, digest = self.cache("wt-f2-abs", ["debug/deps"])
        journal = os.path.join(self.tmp, "crafted.journal.jsonl")
        target = os.path.join(t, "debug", "deps")
        st = os.lstat(target)
        records = [
            {"event": "begin", "exec_id": "x", "manifest_sha256": digest},
            {"event": "group_plan", "directory": t, "paths": [
                {"path": target, "device": st.st_dev, "inode": st.st_ino, "mode": st.st_mode}]},
            {"event": "intent", "path": target, "device": st.st_dev, "inode": st.st_ino,
             "mode": st.st_mode, "files": 2, "dirs": 1},
        ]
        with open(journal, "w") as handle:
            for rec in records:
                handle.write(json.dumps(rec) + "\n")
            handle.write('{"event": "outc')  # torn final record
        import shutil
        shutil.rmtree(target)
        report = self.mod.inspect_journal(journal)
        self.assertTrue(report["torn_final_record"])
        self.assertEqual(report["entries"][0]["state"], "UNCERTAIN_ABSENT_UNCONFIRMED")
        # replaced by a different inode
        os.makedirs(target)
        self.assertTrue(self.mod.inspect_journal(journal)["entries"][0]["state"].startswith("UNCERTAIN"))

    def patch_journal_write(self, fail_when):
        real = self.mod.Journal.write
        calls = []

        def write(journal, record):
            calls.append(record["event"])
            if fail_when(record, calls):
                raise OSError(28, "No space left on device")
            return real(journal, record)
        self.mod.Journal.write = write
        self.addCleanup(setattr, self.mod.Journal, "write", real)
        return calls

    def test_f2_enospc_on_begin_refuses_before_any_mutation(self):
        wt, t, path, digest = self.cache("wt-f2-enospc0")
        self.patch_journal_write(lambda rec, calls: rec["event"] == "begin")
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 4)
        self.assertEqual(r["refused"]["code"], "JOURNAL_UNAVAILABLE")
        for rel in self.RELS:
            self.assertTrue(self.exists(t, rel), rel)

    def test_f2_enospc_on_intent_stops_the_run_without_deleting(self):
        wt, t, path, digest = self.cache("wt-f2-enospc1")
        self.patch_journal_write(lambda rec, calls: rec["event"] == "intent")
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 7)
        self.assertEqual(r["groups"][0]["outcome"], "ABORTED")
        for rel in self.RELS:
            self.assertTrue(self.exists(t, rel), rel)
        self.assertEqual(r["groups"][0]["accounted_freed_bytes"], 0)

    def test_f2_enospc_on_outcome_stops_after_the_recorded_unlink(self):
        wt, t, path, digest = self.cache("wt-f2-enospc2")
        self.patch_journal_write(lambda rec, calls: rec["event"] == "outcome")
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 7)
        g = r["groups"][0]
        self.assertEqual(g["outcome"], "ABORTED")
        self.assertFalse(self.exists(t, "debug/deps"))          # done, unrecorded
        self.assertTrue(self.exists(t, "debug/.fingerprint"))   # never touched
        self.assertIn("journal_error", g["paths"][0])
        self.assertEqual(self.states(self.journals()[0])["debug/deps"], "UNCERTAIN_ABSENT_UNCONFIRMED")

    def test_f2_failed_receipt_publication_keeps_a_complete_journal(self):
        wt, t, path, digest = self.cache("wt-f2-pub")
        real = self.mod.F.atomic_write

        def boom(*a, **k):
            raise OSError(28, "No space left on device")
        self.mod.F.atomic_write = boom
        self.addCleanup(setattr, self.mod.F, "atomic_write", real)
        import io
        import contextlib
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc, r = self.run_retire.__func__(self, path, digest, execute=True) if False else (None, None)
        # call main directly: finish() raises before printing the JSON receipt
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = self.mod.main(["--manifest", path, "--approved-sha256", digest,
                                "--receipt-dir", self.receipts, "--json", "--execute"])
        self.assertEqual(rc, 5)
        self.assertIn("receipt could not be published", err.getvalue())
        (journal,) = self.journals()
        ev = self.events(journal)
        self.assertIn("end", ev)
        self.assertIn("receipt_publication_failed", ev)
        self.assertFalse(self.exists(t, "debug/deps"))

    def test_f2_receipt_dir_inside_a_cache_is_refused(self):
        wt, t, path, digest = self.cache("wt-f2-inside")
        import io
        import contextlib
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = self.mod.main(["--manifest", path, "--approved-sha256", digest,
                                "--receipt-dir", os.path.join(t, "debug", "receipts"), "--json", "--execute"])
        self.assertEqual(rc, 4)
        self.assertEqual(json.loads(out.getvalue())["refused"]["code"], "RECEIPT_DIR_INSIDE_CACHE")
        for rel in self.RELS:
            self.assertTrue(self.exists(t, rel), rel)

    def test_complete_run_writes_a_complete_journal_and_agreeing_receipt(self):
        wt, t, path, digest = self.cache("wt-ok")
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 0)
        g = r["groups"][0]
        self.assertEqual(g["outcome"], "RETIRED")
        (journal,) = self.journals()
        ev = self.events(journal)
        self.assertEqual(ev[0], "begin")
        self.assertEqual(ev[-1], "end")
        self.assertEqual(ev.count("intent"), 3)
        self.assertEqual(ev.count("outcome"), 3)
        outs = [x for x in self.mod.read_journal(journal)[0] if x["event"] == "outcome"]
        self.assertEqual(sum(x["files"] for x in outs), g["files_removed"])
        self.assertEqual(sum(x["freed"] for x in outs), g["accounted_freed_bytes"])
        self.assertTrue(set(self.states(journal).values()) == {"KNOWN_COMPLETED"})
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "emuwiz-test-env", "profile", "library.db")))


class FilesystemRaces(Base):
    def test_symlinked_artifact_and_symlinked_target_component_are_refused(self):
        wt, t = self.managed_cache("wt-sym")
        os.rename(os.path.join(t, "debug", "incremental"), os.path.join(t, "debug", "inc-real"))
        os.symlink(self.victim, os.path.join(t, "debug", "incremental"))
        base = self.group(wt, t, ["debug/incremental", "debug/deps"])
        path, digest = self.manifest([base])
        rc, r = self.run_retire(path, digest, execute=True)
        out = self.outcomes(r)
        self.assertEqual(out["debug/incremental"][0], "HELD")
        self.assertTrue(os.path.exists(os.path.join(self.victim, "precious.db")))
        # target reached through a symlinked parent
        link = os.path.join(self.tmp, "link-to-targets")
        os.symlink(os.path.dirname(t), link)
        g = dict(base)
        g["directory"] = os.path.join(link, os.path.basename(t))
        p2, d2 = self.manifest([g], "m-link.json")
        rc, r = self.run_retire(p2, d2, execute=True)
        self.assertIn("TARGET_PATH_HAS_SYMLINK", {h["code"] for h in r["groups"][0]["holds"]})

    def test_symlink_planted_inside_a_subtree_after_evaluation_is_not_followed(self):
        wt, t = self.managed_cache("wt-plant")
        path, digest = self.manifest([self.group(wt, t, ["debug/deps"])])

        def hook(stage):
            if stage.startswith("before-entry"):
                os.symlink(self.victim, os.path.join(t, "debug", "deps", "planted"))
        self.mod._hook = hook
        self.addCleanup(setattr, self.mod, "_hook", None)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(r)["debug/deps"][0], "REFUSED")
        self.assertTrue(os.path.exists(os.path.join(self.victim, "precious.db")))

    def test_profile_directory_rebound_to_a_symlink_before_deletion(self):
        wt, t = self.managed_cache("wt-rebind-profile")
        path, digest = self.manifest([self.group(wt, t, ["debug/deps"])])

        def hook(stage):
            if stage.startswith("before-entry"):
                os.rename(os.path.join(t, "debug"), os.path.join(t, "debug.moved"))
                os.symlink(self.victim, os.path.join(t, "debug"))
        self.mod._hook = hook
        self.addCleanup(setattr, self.mod, "_hook", None)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(r)["debug/deps"][0], "REFUSED")
        self.assertTrue(os.path.exists(os.path.join(self.victim, "precious.db")))
        self.assertTrue(os.path.exists(os.path.join(t, "debug.moved", "deps", "libx.rlib")))

    def test_target_directory_rebound_to_a_different_directory_is_refused(self):
        wt, t = self.managed_cache("wt-rebind-target")
        path, digest = self.manifest([self.group(wt, t, ["debug/deps"])])
        moved = t + ".moved"

        def hook(stage):
            if stage == "after-open-target":
                os.rename(t, moved)
                os.makedirs(os.path.join(t, "debug", "deps"))
                with open(os.path.join(t, "debug", "deps", "evidence.rlib"), "w") as handle:
                    handle.write("new owner's data")
        self.mod._hook = hook
        self.addCleanup(setattr, self.mod, "_hook", None)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(r)["debug/deps"][0], "REFUSED")
        self.assertTrue(os.path.exists(os.path.join(t, "debug", "deps", "evidence.rlib")))
        self.assertTrue(os.path.exists(os.path.join(moved, "debug", "deps", "libx.rlib")))

    def test_entry_replaced_by_another_inode_is_refused(self):
        wt, t = self.managed_cache("wt-swap")
        path, digest = self.manifest([self.group(wt, t, ["debug/emuwiz.d"])])
        f = os.path.join(t, "debug", "emuwiz.d")
        replacement = f + ".new"
        with open(replacement, "w") as handle:
            handle.write("a different file at the same name")
        os.replace(replacement, f)  # new inode, same name
        self.age(t, 10)
        rc, r = self.run_retire(path, digest, execute=True)
        self.assertEqual(self.outcomes(r)["debug/emuwiz.d"], ("HELD", "PATH_IDENTITY_MISMATCH"))
        self.assertTrue(os.path.exists(f))

    def test_database_looking_file_inside_an_artifact_dir_stops_that_entry(self):
        wt, t = self.managed_cache("wt-data")
        with open(os.path.join(t, "debug", "deps", "profile.sqlite3"), "w") as handle:
            handle.write("x")
        self.age(t, 10)
        path, digest = self.manifest([self.group(wt, t, ["debug/deps", "debug/incremental"])])
        rc, r = self.run_retire(path, digest, execute=True)
        out = self.outcomes(r)
        self.assertEqual(out["debug/deps"][0], "HELD")
        self.assertIn("data", out["debug/deps"][1])
        self.assertEqual(out["debug/incremental"][0], "REMOVED")
        self.assertTrue(self.exists(t, "debug/deps/profile.sqlite3"))


class OrphanRoute(Base):
    """Source-less caches: SOURCE_MISSING stays a hard refusal except the narrow GIT_PRESERVED route."""

    def orphan(self, name="wt-orphan", on_main=True, remove="worktree", **kw):
        wt, t = self.managed_cache(name, **kw)
        head = self.mod.F.head_of(wt)
        if on_main:
            self.git("-C", self.repo, "update-ref", "refs/remotes/origin/main", head)
        if remove == "worktree":
            self.git("-C", self.repo, "worktree", "remove", "--force", wt)
        elif remove == "dir-only":
            self.rmtree(wt)
        g = self.group(self.worktree("scratch-" + name), t)
        g.update({"source_exists": False, "associated_source_paths": "[]", "source_head": head,
                  "orphan_route": "GIT_PRESERVED", "orphan_repo": os.path.realpath(self.repo)})
        return wt, t, g

    def holds(self, g, name="m.json", execute=True):
        path, digest = self.manifest([g], name)
        rc, r = self.run_retire(path, digest, execute=execute, window=True)
        return {h["code"] for h in r["groups"][0]["holds"]}, r

    def test_preserved_orphan_is_retired_with_a_journal_receipt(self):
        wt, t, g = self.orphan()
        codes, r = self.holds(g)
        self.assertEqual(codes, set(), r)
        self.assertFalse(self.exists(t, "debug/deps"))
        self.assertTrue(self.exists(t, "debug/.cargo-lock"))
        self.assertTrue(self.exists(t, "debug/emuwiz-test-env/profile/library.db"))
        self.assertTrue(any(f.startswith("retire-") for f in os.listdir(self.receipts)))

    def test_without_the_route_marker_source_missing_is_still_a_hard_refusal(self):
        wt, t, g = self.orphan()
        del g["orphan_route"]
        codes, _ = self.holds(g)
        self.assertIn("SOURCE_MISSING", codes)
        self.assertTrue(self.exists(t, "debug/deps"))

    def test_head_not_on_origin_main_holds(self):
        wt, t, g = self.orphan(on_main=False)
        codes, _ = self.holds(g)
        self.assertIn("GIT_HISTORY_NOT_PRESERVED", codes)
        self.assertTrue(self.exists(t, "debug/deps"))

    def test_existing_or_still_registered_worktree_holds(self):
        wt, t, g = self.orphan(remove=None)
        codes, _ = self.holds(g)
        self.assertIn("ORPHAN_SOURCE_PRESENT", codes)
        self.assertTrue(self.exists(t, "debug/deps"))
        wt2, t2, g2 = self.orphan("wt-registered", remove="dir-only")
        codes, _ = self.holds(g2, "m2.json")
        self.assertIn("ORPHAN_SOURCE_PRESENT", codes)
        self.assertTrue(self.exists(t2, "debug/deps"))

    def test_manifest_head_mismatch_and_unbound_name_hold(self):
        wt, t, g = self.orphan()
        bad = dict(g, source_head="1" * 40)
        codes, _ = self.holds(bad)
        self.assertIn("OWNER_UNVERIFIED", codes)
        self.assertTrue(self.exists(t, "debug/deps"))

    def test_unidentified_cache_without_a_recorded_build_stays_source_missing(self):
        wt, t, g = self.orphan("wt-never-built", build=False)
        self.assertEqual(self.ret("adopt", t, "--owner", "tester").returncode, 0)
        self.age_record(t, 10)
        codes, _ = self.holds(g)
        self.assertTrue(codes & {"SOURCE_MISSING", "NOT_ADOPTED"}, codes)
        self.assertTrue(self.exists(t, "debug/deps"))

    def test_other_gates_still_apply_to_the_orphan_route(self):
        wt, t = self.managed_cache("wt-pinned-orphan")
        self.assertEqual(self.ret("pin", wt, "--reason", "independent-review").returncode, 0)
        head = self.mod.F.head_of(wt)
        self.git("-C", self.repo, "update-ref", "refs/remotes/origin/main", head)
        self.git("-C", self.repo, "worktree", "remove", "--force", wt)
        g = self.group(self.worktree("wt-scratch-unused2"), t)
        g.update({"source_exists": False, "associated_source_paths": "[]", "source_head": head,
                  "orphan_route": "GIT_PRESERVED", "orphan_repo": os.path.realpath(self.repo)})
        codes, _ = self.holds(g)
        self.assertIn("PINNED", codes)
        self.assertTrue(self.exists(t, "debug/deps"))


class NoManualDeletionRoute(unittest.TestCase):
    def test_no_script_in_the_repository_bypasses_the_retirement_tool(self):
        root = os.path.dirname(HERE)
        out = subprocess.run(["git", "-C", root, "ls-files", "scripts"], capture_output=True, text=True).stdout.split()
        offenders = []
        for rel in out:
            if os.path.basename(rel).startswith("test_") or rel.endswith("emuwiz-cargo-retire"):
                continue
            try:
                with open(os.path.join(root, rel), errors="replace") as handle:
                    text = handle.read()
            except OSError:
                continue
            if "emuwiz-cargo-targets" in text and ("--one-file-system" in text or "shutil.rmtree" in text):
                offenders.append(rel)
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
