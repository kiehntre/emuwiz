#!/usr/bin/env python3
"""Focused regression tests for merged-PR release eligibility."""
import importlib.util
import importlib.machinery
import json
import os
import subprocess
import tempfile
import time
import unittest
from unittest import mock


SCRIPT = os.path.join(os.path.dirname(__file__), "emuwiz-cargo-release-merged")
SPEC = importlib.util.spec_from_loader("release_merged", importlib.machinery.SourceFileLoader("release_merged", SCRIPT))
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


def git(repo, *args):
    return subprocess.check_output(["git", "-C", repo, *args], text=True).strip()


class MergedPRReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = self.tmp.name
        self.repo = os.path.join(self.root, "repo")
        os.mkdir(self.repo)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.name", "Test")
        git(self.repo, "config", "user.email", "test@example.invalid")
        with open(os.path.join(self.repo, "file"), "w") as f:
            f.write("base\n")
        git(self.repo, "add", "file")
        git(self.repo, "commit", "-qm", "base")
        self.base = git(self.repo, "rev-parse", "HEAD")
        git(self.repo, "checkout", "-qb", "pr")
        with open(os.path.join(self.repo, "file"), "w") as f:
            f.write("PR change\n")
        git(self.repo, "commit", "-qam", "PR head")
        self.head = git(self.repo, "rev-parse", "HEAD")
        git(self.repo, "checkout", "-q", "main")
        with open(os.path.join(self.repo, "file"), "w") as f:
            f.write("squash result\n")
        git(self.repo, "commit", "-qam", "squash merge")
        self.merge = git(self.repo, "rev-parse", "HEAD")
        git(self.repo, "update-ref", "refs/remotes/origin/main", self.merge)
        self.wt = os.path.join(self.root, "worktree")
        git(self.repo, "worktree", "add", "-q", self.wt, self.head)
        self.key = "a" * 20
        self.old = time.time() - 4 * 3600
        for base, dirs, files in os.walk(self.wt, topdown=False):
            for name in files:
                if name != ".git":
                    os.utime(os.path.join(base, name), (self.old, self.old))
            os.utime(base, (self.old, self.old))
        self.pr = {"number": 84, "headRefOid": self.head, "mergedAt": "2026-10-23T22:20:28Z",
                   "mergeCommit": {"oid": self.merge}}
        self.rec = {"owner": os.environ.get("USER", ""), "last_start": self.old,
                    "released": None, "active": [], "key": self.key,
                    "worktree": self.wt, "target": os.path.join(self.root, "target")}
        self.registry = os.path.join(self.root, "registry")
        os.makedirs(os.path.join(self.registry, "pins"))
        self.reg_patch = mock.patch.object(release, "REG", self.registry)
        self.reg_patch.start()
        self.addCleanup(self.reg_patch.stop)

    def evaluate(self, pr=None, head=None):
        return release.evaluate(self.rec, self.key, {head or self.head: pr or self.pr}, {}, [],
                                time.time() - 2 * 3600, "refs/remotes/origin/main")

    def test_squash_merge_uses_reachable_merge_commit_and_exact_pr_head(self):
        result = self.evaluate()
        self.assertIsInstance(result, tuple)
        self.assertEqual(result[1]["number"], 84)
        self.assertEqual(result[2], self.head)
        self.assertNotEqual(self.head, self.merge)
        self.assertNotEqual(subprocess.run(["git", "-C", self.wt, "merge-base", "--is-ancestor",
                                            self.head, "refs/remotes/origin/main"]).returncode, 0)

    def test_merge_commit_must_be_reachable(self):
        pr = dict(self.pr, mergeCommit={"oid": self.head})
        self.assertIn("merge commit", self.evaluate(pr=pr))

    def test_missing_merge_commit_fails_closed(self):
        pr = dict(self.pr, mergeCommit=None)
        self.assertIn("no recorded merge commit", self.evaluate(pr=pr))

    def test_worktree_head_must_still_match_merged_pr_head(self):
        self.assertIn("not the head of a merged pull request", self.evaluate(head=self.base))

    def test_merged_heads_requests_github_merge_commit_field(self):
        completed = subprocess.CompletedProcess([], 0, json.dumps([self.pr]), "")
        with mock.patch.object(release.subprocess, "run", return_value=completed) as run:
            rows = release.merged_heads(self.repo)
        self.assertEqual(rows[self.head]["mergeCommit"]["oid"], self.merge)
        self.assertIn("mergeCommit", run.call_args.args[0][-1])


if __name__ == "__main__":
    unittest.main()
