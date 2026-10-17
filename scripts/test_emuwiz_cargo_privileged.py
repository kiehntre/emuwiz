#!/usr/bin/env python3
"""Privileged-execution tests (F-1 symlink safety, F-2 ownership, F-3 trusted code).

These run the real scripts as REAL ROOT through `sudo -n`, against disposable fixtures
only: a private HOME/registry/targets under a temp directory, and a root-owned
disposable installation under /var/lib/emuwiz-privtest.* (removed afterwards; it
includes a root-owned interpreter copy, since this host's /usr/bin is not root-owned).  Nothing
here installs a unit, touches systemd or reads a real cache.  Skipped when `sudo -n`
is unavailable.
"""
import hashlib
import json
import os
import pwd
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import test_emuwiz_cargo_maintain as M  # noqa: E402

SCRIPTS = ("emuwiz-cargo-maintain", "emuwiz-cargo-retention", "emuwiz-cargo-retire")
ME = pwd.getpwuid(os.getuid()).pw_name
DAY = 86400
INSTALL = None
PYTHON = None


def sudo(*cmd, check=True):
    return subprocess.run(["sudo", "-n", *cmd], capture_output=True, text=True, check=check)


def have_sudo():
    try:
        return os.getuid() != 0 and subprocess.run(["sudo", "-n", "true"], capture_output=True).returncode == 0
    except OSError:
        return False


def install_copy(directory, mode="0755"):
    """A root-owned installation of the scripts under test (the documented procedure)."""
    sudo("install", "-d", "-o", "root", "-g", "root", "-m", mode, directory)
    sudo("install", "-o", "root", "-g", "root", "-m", "0644", *[os.path.join(HERE, n) for n in SCRIPTS], directory)


def setUpModule():
    global INSTALL, PYTHON
    if not have_sudo():
        raise unittest.SkipTest("needs passwordless sudo (sudo -n) for real-root tests")
    base = sudo("mktemp", "-d", "/var/lib/emuwiz-privtest.XXXXXX").stdout.strip()
    assert base.startswith("/var/lib/emuwiz-privtest.")
    sudo("chmod", "0755", base)
    INSTALL = base
    sudo("install", "-d", "-o", "root", "-g", "root", "-m", "0755", os.path.join(base, "bin"))
    PYTHON = os.path.join(base, "bin", "python3")
    sudo("install", "-o", "root", "-g", "root", "-m", "0755", os.path.realpath(sys.executable), PYTHON)
    install_copy(os.path.join(base, "libexec"))
    sudo("install", "-d", "-o", "root", "-g", "root", "-m", "0700", os.path.join(base, "protected"))


def tearDownModule():
    if INSTALL and INSTALL.startswith("/var/lib/emuwiz-privtest."):
        sudo("rm", "-rf", "--", INSTALL, check=False)


def protected_listing():
    return sudo("ls", "-A", os.path.join(INSTALL, "protected")).stdout.split()


class Priv(M.MaintBase):
    def setUp(self):
        super().setUp()
        self.full_census()  # only for the in-process (unprivileged) fixture helpers
        self.addCleanup(lambda: sudo("find", os.path.join(INSTALL, "protected"), "-mindepth", "1", "-delete",
                                     check=False))
        # the real root census must find nothing using these fixtures
        os.chdir("/")

    def lib(self, name="emuwiz-cargo-maintain", directory=None):
        return os.path.join(directory or os.path.join(INSTALL, "libexec"), name)

    def root(self, script, *argv, flags=("-I", "-S"), as_user=ME, extra_env=(), python=None):
        env = [f"HOME={self.home}", f"EMUWIZ_TARGET_ROOT={self.targets}", f"EMUWIZ_RETENTION_DIR={self.reg}",
               "EMUWIZ_OWNER=tester", "PATH=/usr/bin:/bin", *extra_env]
        head = ["--as-user", as_user] if as_user else []
        if script.endswith("emuwiz-cargo-maintain"):
            argv = head + list(argv)
        else:
            argv = list(argv) + head
        return sudo("env", "-i", *env, python or PYTHON, *flags, script, *argv, check=False)

    def released(self, name):
        wt, t = self.cache(name)
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        return wt, t

    def root_owned(self):
        return sudo("find", self.tmp, "-user", "root").stdout.split()

    def lock_path(self, t):
        return os.path.join(self.reg, "locks", self.key_of(t) + ".lock")

    def artifacts_present(self, t):
        return os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib"))

    def cycles(self):
        return [h for h in self.m.read_history() if h.get("event") == "cycle"]


class OwnershipF2(Priv):
    def test_root_cycle_works_as_the_owner_and_a_later_user_build_is_tracked(self):
        wt, t = self.released("f2")
        self.approve()
        r = self.root(self.lib(), "run")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.cycles()[-1]["outcome"], "RETIRED", self.cycles()[-1])
        self.assertFalse(self.artifacts_present(t))
        # nothing anywhere in the fixture is root-owned: records, locks, journal,
        # receipts, manifests, usage and history all belong to the owner
        self.assertEqual(self.root_owned(), [])
        rec = json.load(open(os.path.join(self.reg, "targets", self.key_of(t) + ".json")))
        self.assertIn("last_retired", rec)
        # the owner's worktree gate can read and verify the evidence
        rc, out = self.call("worktree-check", wt, "--json")
        self.assertEqual(json.loads(out)["status"], "CLEAR", out)
        # an ordinary cargo-iso build afterwards is TRACKED (no "building untracked")
        b = self.iso(wt, "check")
        self.assertEqual(b.returncode, 0, b.stderr)
        self.assertNotIn("tracking unavailable", b.stderr)
        rec = json.load(open(os.path.join(self.reg, "targets", self.key_of(t) + ".json")))
        self.assertNotIn("released", rec)
        self.assertIn("reopened_at", rec)
        # the reopened target (even once old and with output again) is never retired
        self.make_cache_again(t)
        self.age(t, 10)
        self.age_record(t, 10)
        r = self.root(self.lib(), "run")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertNotEqual(self.cycles()[-1]["outcome"], "RETIRED")
        self.assertTrue(self.artifacts_present(t))
        self.assertEqual(self.root_owned(), [])

    def make_cache_again(self, t):
        os.makedirs(os.path.join(t, "debug", "deps"), exist_ok=True)
        with open(os.path.join(t, "debug", "deps", "libx.rlib"), "wb") as handle:
            handle.write(b"x" * 40000)

    def test_untracked_rebuild_after_release_is_not_retired_on_the_stale_record(self):
        wt, t = self.released("stale")
        path = os.path.join(self.reg, "targets", self.key_of(t) + ".json")
        rec = json.load(open(path))
        rec["released"]["at"] = time.time() - 10 * DAY
        json.dump(rec, open(path, "w"))
        # a build the wrapper could not record (tracking off) writes new output after
        # the release; both are older than the idle period
        env = dict(self.env, EMUWIZ_RETENTION="0")
        self.assertEqual(self.iso(wt, "check", env=env).returncode, 0)
        with open(os.path.join(t, "debug", "deps", "librebuilt.rlib"), "wb") as handle:
            handle.write(b"y" * 40000)
        self.age(t, 5)
        self.approve()
        r = self.root(self.lib(), "run")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertNotEqual(self.cycles()[-1]["outcome"], "RETIRED")
        self.assertTrue(self.artifacts_present(t))
        st = self.state_of(self.status(), t)
        self.assertIn("WRITTEN_AFTER_RELEASE", st["reasons"], st)
        rc, out = self.call("worktree-check", wt, "--json")
        self.assertEqual(json.loads(out)["code"], "WRITTEN_AFTER_RELEASE", out)
        # review finding 4: releasing again (as the message says) clears it
        self.finish(wt)
        st = self.state_of(self.status(), t)
        self.assertNotIn("WRITTEN_AFTER_RELEASE", st["reasons"], st)

    def test_root_census_still_sees_an_owner_process_by_cargo_target_dir_only(self):
        wt, t = self.released("envref")
        self.approve()
        env = dict(os.environ, CARGO_TARGET_DIR=t + "/")
        proc = subprocess.Popen(["sleep", "60"], cwd="/", env=env)
        try:
            time.sleep(0.3)
            r = self.root(self.lib(), "run")
        finally:
            proc.terminate()
            proc.wait()
        self.assertTrue(self.artifacts_present(t), r.stdout + r.stderr)
        self.assertNotEqual(self.cycles()[-1]["outcome"], "RETIRED")

    def test_root_owned_legacy_lock_holds_instead_of_crashing(self):
        wt, t = self.released("legacy")
        self.approve()
        lock = self.lock_path(t)
        sudo("chown", "root:root", lock)
        r = self.root(self.lib(), "run")
        self.assertTrue(self.artifacts_present(t))
        self.assertNotEqual(self.cycles()[-1]["outcome"], "RETIRED", r.stdout + r.stderr)
        sudo("chown", f"{os.getuid()}:{os.getgid()}", lock)


class SymlinksF1(Priv):
    """A planted symlink must never make root create, open or write a protected file."""

    def plant_and_run(self, rel_or_fn, name):
        wt, t = self.released("f1-" + name)
        self.approve()
        victim = os.path.join(INSTALL, "protected", "victim-" + name)
        path = rel_or_fn(t) if callable(rel_or_fn) else os.path.join(self.reg, rel_or_fn)
        if os.path.isdir(path) and not os.path.islink(path):
            shutil.rmtree(path)
        elif os.path.lexists(path):
            os.unlink(path)
        os.symlink(victim, path)
        r = self.root(self.lib(), "run")
        self.assertEqual(protected_listing(), [], r.stdout + r.stderr)
        self.assertEqual(self.root_owned(), [])
        return t, r

    def test_lifecycle_lock_symlink(self):
        t, r = self.plant_and_run(self.lock_path, "lock")
        self.assertTrue(self.artifacts_present(t), "a target whose lock is a symlink is never retired")

    def test_registry_meta_lock_symlink(self):
        self.plant_and_run("locks/registry.meta", "meta")

    def test_receipts_directory_symlink(self):
        t, r = self.plant_and_run("maintenance/receipts", "receipts")
        self.assertTrue(self.artifacts_present(t))

    def test_manifests_directory_symlink(self):
        t, r = self.plant_and_run("maintenance/manifests", "manifests")
        self.assertTrue(self.artifacts_present(t))

    def test_usage_file_symlink(self):
        self.plant_and_run("maintenance/usage.json", "usage")

    def test_record_symlink_to_a_root_only_file(self):
        def record(t):
            return os.path.join(self.reg, "targets", self.key_of(t) + ".json")
        sudo("sh", "-c", f"echo '{{}}' > {INSTALL}/protected/secret && chmod 600 {INSTALL}/protected/secret")
        wt, t = self.released("f1-record")
        self.approve()
        os.unlink(record(t))
        os.symlink(os.path.join(INSTALL, "protected", "secret"), record(t))
        r = self.root(self.lib(), "run")
        self.assertTrue(self.artifacts_present(t))
        self.assertEqual(sudo("cat", os.path.join(INSTALL, "protected", "secret")).stdout.strip(), "{}")
        sudo("rm", "-f", os.path.join(INSTALL, "protected", "secret"))

    def test_manual_executor_lock_symlink(self):
        wt, t = self.managed_cache("man-lock")
        lock = self.lock_path(t)
        os.unlink(lock)
        victim = os.path.join(INSTALL, "protected", "victim-manual")
        os.symlink(victim, lock)
        path, digest = self.manifest([self.group(wt, t)])
        r = self.root(self.lib("emuwiz-cargo-retire"), "--manifest", path, "--approved-sha256", digest,
                      "--execute", "--receipt-dir", self.receipts, "--json")
        self.assertEqual(protected_listing(), [], r.stderr)
        out = json.loads(r.stdout)
        self.assertEqual(out["groups"][0]["outcome"], "HELD", out["groups"][0])
        self.assertIn("LIFECYCLE_LOCK_UNAVAILABLE", [h["code"] for h in out["groups"][0]["holds"]])
        self.assertTrue(self.artifacts_present(t))
        self.assertEqual(self.root_owned(), [])


class TrustedCodeF3(Priv):
    def test_root_refuses_to_run_from_a_user_writable_worktree(self):
        wt, t = self.released("f3-wt")
        self.approve()
        marker = os.path.join(self.tmp, "marker")
        evil = os.path.join(self.tmp, "evil")
        os.makedirs(evil)
        for n in SCRIPTS:
            shutil.copy(os.path.join(HERE, n), evil)
        with open(os.path.join(evil, "emuwiz-cargo-retention"), "a") as handle:
            handle.write(f"\nopen({marker!r}, 'w').write(str(os.geteuid()))\n")
        for script in (os.path.join(HERE, "emuwiz-cargo-maintain"), os.path.join(evil, "emuwiz-cargo-maintain"),
                       os.path.join(evil, "emuwiz-cargo-retire")):
            r = self.root(script, "run" if script.endswith("maintain") else "--help")
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("untrusted location", r.stderr)
        self.assertFalse(os.path.exists(marker))
        self.assertTrue(self.artifacts_present(t))
        self.assertEqual(self.cycles(), [])

    def test_root_refuses_a_root_owned_copy_in_a_writable_or_user_owned_place(self):
        bad = os.path.join(INSTALL, "writable")
        install_copy(bad, mode="0777")
        r = self.root(self.lib(directory=bad), "verify-install")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("group/world writable", r.stderr)
        owned = os.path.join(INSTALL, "userfile")
        install_copy(owned)
        sudo("chown", ME, os.path.join(owned, "emuwiz-cargo-retire"))
        r = self.root(self.lib(directory=owned), "verify-install")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("not root", r.stderr)
        link = os.path.join(INSTALL, "linked")
        install_copy(link)
        sudo("rm", os.path.join(link, "emuwiz-cargo-retention"))
        sudo("ln", "-s", os.path.join(HERE, "emuwiz-cargo-retention"), os.path.join(link, "emuwiz-cargo-retention"))
        r = self.root(self.lib(directory=link), "verify-install")
        self.assertNotEqual(r.returncode, 0)

    def test_root_refuses_an_interpreter_in_a_user_writable_directory(self):
        mine = os.path.join(self.tmp, "py")
        os.makedirs(mine)
        shutil.copy(PYTHON, mine)
        r = self.root(self.lib(), "verify-install", python=os.path.join(mine, "python3"))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("untrusted location", r.stderr)

    def test_root_requires_isolated_no_site_interpreter_and_an_owner(self):
        for flags in (("-I",), ("-S",), ()):
            r = self.root(self.lib(), "verify-install", flags=flags)
            self.assertNotEqual(r.returncode, 0, flags)
            self.assertIn("-I -S", r.stderr)
        r = self.root(self.lib(), "status", as_user=None)
        self.assertIn("--as-user", r.stderr)
        r = self.root(self.lib(), "status", as_user="root")
        self.assertNotEqual(r.returncode, 0)
        for argv in (("policy", "enable"), ("task-complete",), ("admit", "--target", self.targets)):
            r = self.root(self.lib(), *argv)
            self.assertNotEqual(r.returncode, 0, argv)
            self.assertIn("owner action", r.stderr)
        r = self.root(self.lib(), "run", extra_env=("EMUWIZ_RETENTION_NOW=99999999999",))
        self.assertIn("EMUWIZ_RETENTION_NOW", r.stderr)

    def test_root_refuses_registry_or_targets_not_owned_by_the_owner(self):
        self.released("f3-own")
        self.approve()
        other = os.path.join(INSTALL, "protected")
        r = sudo("env", "-i", f"HOME={self.home}", f"EMUWIZ_TARGET_ROOT={other}", f"EMUWIZ_RETENTION_DIR={self.reg}",
                 PYTHON, "-I", "-S", self.lib(), "--as-user", ME, "run", check=False)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("not a real directory owned by", r.stderr)

    def test_verify_install_reports_hashes_of_the_trusted_copy(self):
        r = self.root(self.lib(), "verify-install")
        self.assertEqual(r.returncode, 0, r.stderr)
        out = json.loads(r.stdout)
        for n in SCRIPTS:
            with open(os.path.join(HERE, n), "rb") as handle:
                self.assertEqual(out["files"][n], hashlib.sha256(handle.read()).hexdigest())

    def test_units_name_only_the_installed_copy(self):
        rc, out = self.call("--as-user", ME, "units", "--install-dir", os.path.join(INSTALL, "libexec"),
                            "--python", PYTHON)
        self.assertEqual(rc, 0, out)
        start = [l for l in out.splitlines() if l.startswith("ExecStart=")]
        self.assertEqual(start, [f"ExecStart={PYTHON} -I -S {self.lib()} --as-user {ME} run"])
        self.assertNotIn(HERE, out)
        for line in ("NoNewPrivileges=yes", "ProtectSystem=strict", f"ReadWritePaths={self.reg} {self.targets}",
                     "Nothing is installed or enabled"):
            self.assertIn(line, out)


    def test_gate_runs_before_the_script_imports_any_other_module(self):
        """Review finding 2: an interpreter whose stdlib is owner-controlled is refused
        before the script imports anything from it (argparse here).  The interpreter's own
        start-up imports (encodings) cannot be covered: that is the documented residual."""
        alt = os.path.join(INSTALL, "altpy")
        sudo("install", "-d", "-o", "root", "-g", "root", "-m", "0755", alt, os.path.join(alt, "bin"),
             os.path.join(alt, "lib"))
        sudo("install", "-o", "root", "-g", "root", "-m", "0755", PYTHON, os.path.join(alt, "bin", "python3"))
        lib = os.path.join(alt, "lib", "python3.12")
        sudo("cp", "-r", "/usr/lib/python3.12", lib)
        sudo("rm", "-rf", os.path.join(lib, "site-packages"), os.path.join(lib, "test"))
        sudo("chown", "-R", ME, lib)
        marker = os.path.join(self.tmp, "argparse-ran")
        with open(os.path.join(lib, "argparse.py"), "a") as handle:
            handle.write(f"\nopen({marker!r}, 'w').write(str(__import__('os').getresuid()))\n")
        for script in (self.lib(), self.lib("emuwiz-cargo-retire")):
            r = self.root(script, "verify-install" if script.endswith("maintain") else "--help",
                          python=os.path.join(alt, "bin", "python3"))
            self.assertNotEqual(r.returncode, 0, r.stdout)
            self.assertIn("untrusted location", r.stderr)
            self.assertFalse(os.path.exists(marker), "owner-controlled module ran as root")

    def test_units_refuse_an_untrusted_interpreter_or_install(self):
        for argv in (("--install-dir", os.path.join(INSTALL, "libexec"), "--python", "/tmp/evil/python3"),
                     ("--install-dir", HERE, "--python", PYTHON)):
            rc, out = self.call("--as-user", ME, "units", *argv)
            self.assertNotEqual(rc, 0, out)

    def test_an_exception_at_elevation_never_leaves_euid_0(self):
        """Review finding 1: an exception raised right after seteuid(0) (the window a
        signal handler could use) still restores the owner's euid; a signal sent while
        elevated is only delivered after the restore."""
        code = ("import os,signal,importlib.machinery as m;"
                f"F=m.SourceFileLoader('f',{self.lib('emuwiz-cargo-retention')!r}).load_module();"
                f"F.drop_to_owner({ME!r});real=os.seteuid\n"
                "def boom(u):\n real(u)\n if u==0: raise KeyboardInterrupt\n"
                "os.seteuid=boom\n"
                "try:\n F.Census(['/nonexistent'])\nexcept KeyboardInterrupt: pass\n"
                "os.seteuid=real\nprint('after', os.getresuid())\n"
                "seen=[]\nsignal.signal(signal.SIGUSR1, lambda *a: seen.append(os.geteuid()))\n"
                "with F.elevated():\n os.kill(os.getpid(), signal.SIGUSR1); inside=list(seen)\n"
                "print('inside', inside, 'delivered', seen)\n")
        r = sudo(PYTHON, "-I", "-S", "-c", code, check=False)
        self.assertEqual(r.returncode, 0, r.stderr)
        uid = os.getuid()
        self.assertIn(f"after (0, {uid}, 0)", r.stdout)
        self.assertIn(f"inside [] delivered [{uid}]", r.stdout)

    def test_foundation_and_cargo_iso_refuse_root(self):
        wt, t = self.managed_cache("iso-root")
        r = sudo("env", f"HOME={self.home}", f"EMUWIZ_TARGET_ROOT={self.targets}", f"EMUWIZ_RETENTION_DIR={self.reg}",
                 PYTHON, "-I", "-S", os.path.join(HERE, "emuwiz-cargo-retention"), "status", check=False)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("must not run as root", r.stderr)
        self.assertEqual(self.root_owned(), [])

    def test_children_never_keep_root(self):
        """git (the only child the cycle starts) runs with uid 0 nowhere in its credentials."""
        probe = os.path.join(self.tmp, "probe-bin")
        os.makedirs(probe)
        log = os.path.join(self.tmp, "ids")
        with open(os.path.join(probe, "git"), "w") as handle:
            handle.write(f"#!/bin/sh\n/usr/bin/grep -E '^(Uid|Gid|CapPrm|CapEff):' /proc/self/status >> {log}\nexit 1\n")
        os.chmod(os.path.join(probe, "git"), 0o755)
        code = ("import os,sys,importlib.machinery as m;"
                f"F=m.SourceFileLoader('f',{self.lib('emuwiz-cargo-retention')!r}).load_module();"
                f"F.drop_to_owner({ME!r});os.environ['PATH']={probe!r};F.head_of('/');"
                "print(os.getresuid())")
        r = sudo(PYTHON, "-I", "-S", "-c", code, check=False)
        self.assertEqual(r.returncode, 0, r.stderr)
        uid = str(os.getuid())
        with open(log) as handle:
            lines = dict(l.split(":", 1) for l in handle.read().splitlines())
        self.assertEqual(lines["Uid"].split(), [uid] * 4)
        self.assertEqual(int(lines["CapPrm"].strip(), 16), 0)
        self.assertEqual(int(lines["CapEff"].strip(), 16), 0)


class ManualProcedure(Priv):
    """The documented manual (sudo) executor procedure on disposable fixtures."""

    def test_documented_manual_dry_run_then_execute(self):
        wt, t = self.managed_cache("manual")
        self.finish(wt)  # an exact manifest does not substitute for task completion
        path, digest = self.manifest([self.group(wt, t)])
        x = self.lib("emuwiz-cargo-retire")
        # wrong digest: refused before anything else
        r = self.root(x, "--manifest", path, "--approved-sha256", "0" * 64, "--json")
        self.assertEqual(json.loads(r.stdout)["refused"]["code"], "MANIFEST_DIGEST_MISMATCH")
        r = self.root(x, "--manifest", path, "--approved-sha256", digest, "--json")
        dry = json.loads(r.stdout)
        self.assertEqual(dry["process_census"]["unreadable_foreign"], 0, "root census must be complete")
        self.assertEqual(dry["groups"][0]["outcome"], "WOULD_RETIRE", dry["groups"][0]["holds"])
        self.assertTrue(self.artifacts_present(t))
        r = self.root(x, "--manifest", path, "--approved-sha256", digest, "--execute",
                      "--receipt-dir", self.receipts, "--json")
        done = json.loads(r.stdout)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(done["groups"][0]["outcome"], "RETIRED")
        self.assertFalse(self.artifacts_present(t))
        self.assertTrue(os.path.exists(done["receipt_file"]) and os.path.exists(done["journal_file"]))
        self.assertEqual(self.root_owned(), [], "no chown-back step is needed any more")
        # and the next ordinary build of that target is tracked
        b = self.iso(wt, "check")
        self.assertNotIn("tracking unavailable", b.stderr)

    def test_manual_executor_from_the_worktree_is_refused(self):
        wt, t = self.managed_cache("manual-wt")
        path, digest = self.manifest([self.group(wt, t)])
        r = self.root(os.path.join(HERE, "emuwiz-cargo-retire"), "--manifest", path, "--approved-sha256", digest,
                      "--execute", "--receipt-dir", self.receipts)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("untrusted location", r.stderr)
        self.assertTrue(self.artifacts_present(t))
        self.assertFalse(os.path.exists(self.receipts))

    def test_manual_executor_as_root_needs_an_owner(self):
        wt, t = self.managed_cache("manual-owner")
        path, digest = self.manifest([self.group(wt, t)])
        r = self.root(self.lib("emuwiz-cargo-retire"), "--manifest", path, "--approved-sha256", digest, as_user=None)
        self.assertIn("--as-user", r.stderr)
        self.assertNotEqual(r.returncode, 0)


@unittest.skipUnless(os.environ.get("EMUWIZ_TEST_SYSTEMD") == "1",
                     "set EMUWIZ_TEST_SYSTEMD=1 to run the generated service once under systemd")
class SystemdService(Priv):
    """The generated service, unchanged except for its name, run once by systemd as a
    transient runtime unit (/run/systemd/system, removed afterwards) against the
    disposable fixtures.  The production unit name and the timer are never installed."""

    def test_generated_service_runs_a_complete_census_cycle_as_the_owner(self):
        wt, t = self.released("svc")
        self.approve()
        out = os.path.join(self.tmp, "units")
        os.makedirs(out)
        rc, text = self.call("--as-user", ME, "units", "--install-dir", os.path.join(INSTALL, "libexec"),
                             "--python", PYTHON, "--output-dir", out)
        self.assertEqual(rc, 0, text)
        verify = subprocess.run(["systemd-analyze", "verify", os.path.join(out, "emuwiz-cargo-maintain.service"),
                                 os.path.join(out, "emuwiz-cargo-maintain.timer")], capture_output=True, text=True)
        self.assertEqual(verify.returncode, 0, verify.stderr)
        name = "emuwiz-cargo-maintain-selftest-%s.service" % os.path.basename(INSTALL).split(".")[-1].lower()
        unit = os.path.join("/run/systemd/system", name)
        sudo("install", "-o", "root", "-g", "root", "-m", "0644", os.path.join(out, "emuwiz-cargo-maintain.service"), unit)
        try:
            sudo("systemctl", "daemon-reload")
            start = sudo("systemctl", "start", name, check=False)
            show = sudo("systemctl", "show", name, "-p", "Result", "-p", "ExecMainStatus").stdout
            log = sudo("journalctl", "-u", name, "--no-pager", "-o", "cat", check=False).stdout
            self.assertEqual(start.returncode, 0, start.stderr + log)
            self.assertIn("Result=success", show, show + log)
        finally:
            sudo("rm", "-f", unit, check=False)
            sudo("systemctl", "daemon-reload", check=False)
            sudo("systemctl", "reset-failed", name, check=False)
        cyc = self.cycles()[-1]
        self.assertEqual(cyc["outcome"], "RETIRED", (cyc, log))
        self.assertFalse(self.artifacts_present(t))
        self.assertEqual(self.root_owned(), [])
        rc, out = self.call("worktree-check", wt, "--json")
        self.assertEqual(json.loads(out)["status"], "CLEAR", out)
        self.assertFalse(os.path.exists("/etc/systemd/system/emuwiz-cargo-maintain.timer"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
