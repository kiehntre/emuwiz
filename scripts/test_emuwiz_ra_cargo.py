#!/usr/bin/env python3
"""rust-analyzer routing (scripts/emuwiz-ra-cargo, cargo-iso --rust-analyzer) and the
retirement of its sub-target.  Synthetic repo, fake cargo, private HOME and targets."""
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_retire as R  # noqa: E402
import test_emuwiz_cargo_retention as T  # noqa: E402

RA_CARGO = os.path.join(HERE, "emuwiz-ra-cargo")
RA = os.path.join(HERE, "emuwiz-rust-analyzer")


class RaCargo(R.Base):
    def emuwiz_checkout(self, name):
        wt = self.worktree(name)
        os.makedirs(os.path.join(wt, "scripts"))
        os.makedirs(os.path.join(wt, "crates", "archivefs-core"))
        for rel in ("Cargo.toml", "scripts/cargo-iso"):
            with open(os.path.join(wt, rel), "w") as handle:
                handle.write("# fixture\n")
        return wt

    def log(self):
        try:
            with open(self.env["FAKE_LOG"]) as handle:
                return [(line.split(None, 1) + [""])[1].strip() for line in handle if line.strip()]
        except OSError:
            return []

    def ra(self, cwd, *args, env=None):
        return subprocess.run([RA_CARGO, *args], cwd=cwd, env=env or self.env, capture_output=True, text=True)

    def test_emuwiz_checkout_builds_into_the_managed_sub_target_never_in_tree(self):
        wt = self.emuwiz_checkout("wt-ra")
        r = self.ra(wt, "check", "--workspace", "--message-format=json")
        self.assertEqual(r.returncode, 0, r.stderr)
        target = os.path.realpath(self.target_of(wt))
        self.assertEqual(self.log(), [os.path.join(target, "rust-analyzer")])
        self.assertFalse(os.path.exists(os.path.join(wt, "target")))
        # registered and locked like a normal build: the lifecycle record exists
        self.assertTrue(os.path.exists(os.path.join(self.reg, "targets", self.key_of(target) + ".json")))

    def test_inherited_target_dir_and_explicit_target_dir_cannot_redirect_it(self):
        wt = self.emuwiz_checkout("wt-ra2")
        env = dict(self.env, CARGO_TARGET_DIR=os.path.join(wt, "target"))
        self.assertEqual(self.ra(wt, "check", env=env).returncode, 0)
        self.assertEqual(self.log(), [os.path.join(os.path.realpath(self.target_of(wt)), "rust-analyzer")])
        r = self.ra(wt, "check", "--target-dir", os.path.join(wt, "target"))
        self.assertEqual(r.returncode, 2)
        self.assertEqual(len(self.log()), 1, "nothing may be built on refusal")
        self.assertFalse(os.path.exists(os.path.join(wt, "target")))

    def test_low_disk_fails_closed_without_falling_back_to_plain_cargo(self):
        wt = self.emuwiz_checkout("wt-ra3")
        r = self.ra(wt, "check", env=dict(self.env, EMUWIZ_MIN_FREE_GB="999999"))
        self.assertEqual(r.returncode, 4, r.stderr)
        self.assertEqual(self.log(), [])
        self.assertFalse(os.path.exists(os.path.join(wt, "target")))

    def test_unresolvable_git_fails_closed_instead_of_plain_cargo(self):
        wt = self.emuwiz_checkout("wt-ra5")
        with open(os.path.join(wt, ".git"), "w") as handle:
            handle.write("gitdir: /nonexistent/broken\n")
        r = self.ra(os.path.join(wt, "crates", "archivefs-core"), "check")
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertEqual(self.log(), [])
        self.assertFalse(os.path.exists(os.path.join(wt, "target")))
        other = os.path.join(self.tmp, "plain")
        os.makedirs(other)
        r = self.ra(other, "check", "--manifest-path", os.path.join(wt, "Cargo.toml"))
        self.assertEqual(r.returncode, 2, r.stderr)

    def test_debug_mode_record_is_untouched(self):
        wt = self.emuwiz_checkout("wt-ra4")
        self.assertEqual(self.ra(wt, "check").returncode, 0)
        name = os.path.basename(os.path.realpath(self.target_of(wt)))
        self.assertFalse(os.path.exists(os.path.join(self.targets + ".debug-mode", name)))

    def test_other_repositories_use_plain_cargo(self):
        other = os.path.join(self.tmp, "other")
        subprocess.run(["git", "init", "-q", other], check=True)
        self.assertEqual(self.ra(other, "check").returncode, 0)
        self.assertEqual(len(self.log()), 1)
        self.assertNotIn("rust-analyzer", self.log()[0])

    def test_launcher_exports_the_wrapper_and_runs_the_real_server(self):
        fake = os.path.join(self.tmp, "fake-ra")
        with open(fake, "w") as handle:
            handle.write('#!/bin/sh\necho "CARGO=$CARGO TD=${CARGO_TARGET_DIR:-unset} ARGS=$*"\n')
        os.chmod(fake, 0o755)
        env = dict(self.env, EMUWIZ_REAL_RUST_ANALYZER=fake, CARGO_TARGET_DIR="/somewhere")
        r = subprocess.run([RA, "--version"], env=env, capture_output=True, text=True)
        self.assertEqual(r.stdout.strip(), f"CARGO={RA_CARGO} TD=unset ARGS=--version")

    def test_sub_target_is_selected_and_retired_like_the_target(self):
        wt, target = self.managed_cache("wt-ra-retire")
        sub = os.path.join(target, "rust-analyzer")
        self.make_cache(sub)
        with open(os.path.join(sub, ".rustc_info.json"), "w") as handle:
            handle.write("{}")
        self.age(target, 10)
        selected, preserved = self.mod.F.select_artifacts(target, None)
        paths = {s["path"] for s in selected}
        self.assertIn(os.path.join(sub, "debug", "deps"), paths)
        self.assertIn(os.path.join(sub, ".rustc_info.json"), paths)
        self.assertIn(os.path.join(sub, "debug", "emuwiz-test-env"), {p["path"] for p in preserved})
        # and it is retired by an owner-signed execution, evidence preserved
        g = self.group(wt, target, ["debug/deps", "rust-analyzer/debug/deps", "rust-analyzer/.rustc_info.json"])
        path, digest = self.manifest([g])
        rc, receipt = self.run_retire(path, digest, execute=True)
        self.assertEqual(rc, 0, receipt)
        self.assertEqual({g["outcome"] for g in receipt["groups"]}, {"RETIRED"}, receipt["groups"])
        self.assertFalse(os.path.exists(os.path.join(sub, "debug", "deps")))
        self.assertTrue(os.path.exists(os.path.join(sub, "debug", "emuwiz-test-env")))


if __name__ == "__main__":
    unittest.main()
