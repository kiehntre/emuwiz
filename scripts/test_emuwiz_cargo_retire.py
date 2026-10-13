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
            def __init__(inner, targets):
                super().__init__(targets)
                inner.unreadable_foreign = 0

        self.mod.F.Census = Full

    # --- fixtures
    def managed_cache(self, name="wt-r", age_days=10, build=True):
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

    def run_retire(self, path, digest, *extra, execute=False, window=False):
        args = ["--manifest", path, "--approved-sha256", digest, "--receipt-dir", self.receipts, "--json"]
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

    def test_legacy_target_needs_adoption_verification_and_an_attested_window(self):
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
        rc, rec = self.run_retire(path, digest, execute=True)
        self.assertIn("NONPARTICIPATING_WRITERS_NOT_EXCLUDED", {h["code"] for h in rec["groups"][0]["holds"]})
        self.assertTrue(self.exists(t, "debug/deps"))
        rc, rec = self.run_retire(path, digest, execute=True, window=True)
        self.assertEqual(rec["groups"][0]["outcome"], "RETIRED", rec["groups"][0]["holds"])
        self.assertFalse(self.exists(t, "debug/deps"))
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
        self.assertIn("RECENT_ACTIVITY", codes)
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
        self.assertEqual(r["groups"][0]["outcome"], "RETIRED", r["groups"][0]["holds"])
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


if __name__ == "__main__":
    unittest.main(verbosity=2)
