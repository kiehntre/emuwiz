#!/usr/bin/env python3
"""Routine eligibility: forgotten --finish-task released only on verified evidence;
merged = ancestor of the freshly fetched origin/main or a squash-merged PR head; a failed
fetch keeps the cache.  Private HOME/registry/targets, synthetic git, fake cargo."""
import json
import os
import subprocess
import sys
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import test_emuwiz_cargo_maintain as M  # noqa: E402

G = ["-c", "user.name=t", "-c", "user.email=t@t"]


class Helpers(M.MaintBase):
    def setUp(self):
        super().setUp()
        self.full_census()

    def forgotten(self, name, quiet_hours=30, status="Merged abc1234; done"):
        """Built, merged, clean, idle, but nobody ran --finish-task.  `status` is its
        coordination-table row (None: no row)."""
        wt, t = self.cache(name)
        self.age(t, 10)
        self.age_record(t, 10)
        self.age(wt, quiet_hours / 24.0)
        gitdir = subprocess.run(["git", "-C", wt, "rev-parse", "--absolute-git-dir"], capture_output=True,
                                text=True).stdout.strip()
        self.age(gitdir, quiet_hours / 24.0)             # the checkout's own index/HEAD/reflog
        if status is not None:
            self.coordination(wt, status)
        return wt, t

    def coordination(self, wt, status):
        d = os.path.join(self.home, "emuwiz-audit-ledger", "coord")
        os.makedirs(d, exist_ok=True)
        path = os.path.join(d, "COORDINATION.md")
        if not os.path.exists(path):
            with open(path, "w") as h:
                h.write("# coordination\n\n| Task | Sole owner | Worktree | PR | Status / dependency |\n|---|---|---|---|---|\n")
        with open(path, "a") as h:
            h.write(f"| task {os.path.basename(wt)} | davedap-xx | {os.path.basename(wt)} (branch b) | #1 | {status} |\n")

    def skipped_reasons(self, t):
        policy = self.m.load_policy()[1]
        items = self.m.snapshot(policy)[1]
        _, skipped = self.m.finished_task_candidates(items, policy)
        return next((r for i, r in skipped if i["target"] == t), None)

    def intact(self, t):
        return os.path.exists(os.path.join(t, "debug", "deps", "libx.rlib"))

    def released(self, t):
        with open(os.path.join(self.reg, "targets", self.key_of(t) + ".json")) as h:
            rec = json.load(h)
        return rec.get("released")

    def row(self, t):
        return self.state_of(self.status(), t)


class RoutineRelease(Helpers):
    def test_abandoned_merged_task_is_released_then_retired_in_one_routine_cycle(self):
        wt, t = self.forgotten("rr-forgot")
        self.approve()
        rc, out = self.call("run", "--dry-run")
        self.assertEqual(rc, 0, out)
        self.assertIn("would first release 1", out)
        self.assertIsNone(self.released(t))                     # the dry run records nothing
        rc, out = self.call("run")
        self.assertEqual(rc, 0, out)
        rel = self.released(t)
        self.assertEqual(rel["task"], self.m.AUTO_RELEASE_TASK, rel)
        self.assertFalse(self.intact(t), out)

    def test_paused_overnight_task_is_not_released(self):
        # stopped for the night 12 h ago, row still says Paused: neither quiet long enough nor finished
        wt, t = self.forgotten("rr-paused", quiet_hours=12, status="Merged #1; paused overnight, resumes tomorrow")
        self.approve()
        self.assertEqual(self.skipped_reasons(t), ["OWNERSHIP_TASK_NOT_FINISHED"])
        self.call("run")
        self.assertIsNone(self.released(t))
        self.assertTrue(self.intact(t))
        # the same quiet 12 h with a finished row: still too recent for an inferred completion
        wt2, t2 = self.forgotten("rr-12h", quiet_hours=12)
        self.assertEqual(self.skipped_reasons(t2), ["CHECKOUT_RECENTLY_USED"])
        self.call("run")
        self.assertIsNone(self.released(t2))
        self.assertTrue(self.intact(t2))

    def test_waiting_hold_unlisted_or_mixed_ownership_is_not_released(self):
        cases = {"rr-wait": "Merged; waiting for owner", "rr-hold": "Merged; worktree HOLD",
                 "rr-active": "Active", "rr-handed": "Merged; handed to davedap-9d"}
        cases["rr-wip"] = "Not merged; agent running review (WIP)"
        made = {n: self.forgotten(n, status=st) for n, st in cases.items()}
        wt_dot, t_dot = self.forgotten("rr-dot", status=None)      # only a sibling `rr-dot.v1` has a row
        self.coordination(wt_dot + ".v1", "Merged; done")
        cases["rr-dot"] = None
        made["rr-dot"] = (wt_dot, t_dot)
        wt_none, t_none = self.forgotten("rr-unlisted", status=None)
        wt_note, t_note = self.forgotten("rr-note")             # row says Merged, a later note says HOLD
        with open(os.path.join(self.home, "emuwiz-audit-ledger", "coord", "COORDINATION.md"), "a") as h:
            h.write(f"- 2026-11-12 davedap-xx: merged. Worktree {wt_note} HOLD.\n"
                    "| task later | davedap-yy | elsewhere | #2 | Done |\n")
        made["rr-note"] = (wt_note, t_note)
        self.approve()
        for n, (wt, t) in made.items():
            if n in ("rr-hold", "rr-note"):           # a HOLD line is a HOLD source: held before release is considered
                self.assertIn("HOLD_COORDINATION", self.row(t)["reasons"], n)
            elif n == "rr-dot":
                self.assertEqual(self.skipped_reasons(t), ["OWNERSHIP_UNRESOLVED"], n)
            else:
                self.assertEqual(self.skipped_reasons(t), ["OWNERSHIP_TASK_NOT_FINISHED"], n)
        self.assertEqual(self.skipped_reasons(t_none), ["OWNERSHIP_UNRESOLVED"])
        self.call("run")
        for wt, t in [*made.values(), (wt_none, t_none)]:
            self.assertIsNone(self.released(t), t)
            self.assertTrue(self.intact(t), t)

    def test_explicitly_finished_task_needs_no_quiet_period_or_row(self):
        wt, t = self.cache("rr-explicit")
        self.finish(wt)                                          # the normal path: --finish-task
        self.age(t, 10)
        self.age_record(t, 10)                                   # checkout touched just now, no row
        self.approve()
        rc, out = self.call("run")
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.intact(t), out)
        self.assertNotEqual(self.released(t)["task"], self.m.AUTO_RELEASE_TASK)

    def test_checkout_named_by_a_recent_agent_session_is_not_released(self):
        wt, t = self.forgotten("rr-session")
        d = os.path.join(self.home, ".claude", "projects", "p", "session-1", "subagents")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "agent.jsonl"), "w") as h:    # a subagent, naming ~/<basename> only
            h.write(json.dumps({"command": f"cd ~/{os.path.basename(wt)} && cargo test"}) + "\n")
        self.approve()
        self.call("run")
        self.assertIsNone(self.released(t))
        self.assertTrue(self.intact(t))

    def test_process_in_checkout_blocks_release(self):
        wt, t = self.forgotten("rr-proc")
        proc = subprocess.Popen(["sleep", "600"], cwd=wt)
        try:
            time.sleep(0.3)
            self.approve()
            self.call("run")
        finally:
            proc.kill()
            proc.wait()
        self.assertIsNone(self.released(t))
        self.assertTrue(self.intact(t))

    def test_pinned_dirty_or_unmerged_are_not_released(self):
        wt1, t1 = self.forgotten("rr-pin")
        self.assertEqual(self.ret("pin", wt1, "--reason", "release-build").returncode, 0)
        wt2, t2 = self.forgotten("rr-dirty")
        with open(os.path.join(wt2, "wip.rs"), "w") as h:
            h.write("fn wip() {}")
        self.age(wt2, 1)
        wt3, t3 = self.forgotten("rr-unmerged")
        self.git("-C", wt3, *G, "commit", "-q", "--allow-empty", "-m", "wip")
        self.age(wt3, 1)
        self.approve()
        self.call("run")
        for t in (t1, t2, t3):
            self.assertIsNone(self.released(t), t)
            self.assertTrue(self.intact(t), t)

    def test_failed_fetch_preserves_the_cache_and_reports_why(self):
        wt, t = self.ready_released("rr-nofetch")
        self.git("-C", self.repo, "remote", "add", "origin", os.path.join(self.tmp, "no-such-remote"))
        self.approve()
        self.assertIn("SOURCE_GIT_UNVERIFIED", self.row(t)["reasons"])
        rc, out = self.call("run")
        self.assertTrue(self.intact(t), out)
        # branches and HEAD are never touched by the check
        self.assertEqual(self.git_out("-C", wt, "rev-parse", "--abbrev-ref", "HEAD"), "rr-nofetch")

    def ready_released(self, name):
        wt, t = self.cache(name)
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        return wt, t

    def git_out(self, *args):
        return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout.strip()

    def squash_setup(self, name):
        """origin is a bare clone; the branch has a commit that main received as a squash."""
        self.m.ALLOW_LOCAL_EVIDENCE_REMOTE = True          # a local bare repo stands in for GitHub
        origin = os.path.join(self.tmp, "origin.git")
        self.git("-C", self.repo, "branch", "-M", "main")
        self.git("clone", "-q", "--bare", self.repo, origin)
        self.git("-C", self.repo, "remote", "add", "origin", origin)
        wt, t = self.cache(name)
        with open(os.path.join(wt, "feature.rs"), "w") as h:
            h.write("fn f() {}")
        self.git("-C", wt, "add", "feature.rs")
        self.git("-C", wt, *G, "commit", "-q", "-m", "feature")
        head = self.git_out("-C", wt, "rev-parse", "HEAD")
        self.git("-C", wt, "push", "-q", "origin", f"HEAD:refs/heads/{name}")
        sq = os.path.join(self.tmp, "squash-wt")
        self.git("-C", self.repo, "worktree", "add", "-q", "--detach", sq, "main")
        with open(os.path.join(sq, "feature.rs"), "w") as h:
            h.write("fn f() {}")
        self.git("-C", sq, "add", "feature.rs")
        self.git("-C", sq, *G, "commit", "-q", "-m", "feature (#1)")
        squash = self.git_out("-C", sq, "rev-parse", "HEAD")
        self.git("-C", sq, "push", "-q", "origin", "HEAD:refs/heads/main")
        self.git("-C", self.repo, "worktree", "remove", "--force", sq)
        self.git("-C", self.repo, "fetch", "-q", "origin", f"+refs/heads/{name}:refs/remotes/origin/{name}")
        self.finish(wt)
        self.age(t, 10)
        self.age_record(t, 10)
        return wt, t, head, squash

    def test_squash_merged_pr_head_is_merged_after_a_fresh_fetch(self):
        wt, t, head, squash = self.squash_setup("rr-squash")
        self.m.merged_pr_heads = lambda _wt: {head: squash}
        self.approve()
        before = self.git_out("-C", self.repo, "for-each-ref")
        self.assertEqual(self.m.routine_source_problems(wt), [])
        rc, out = self.call("run")
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.intact(t), out)
        # the evidence lives in the maintenance state directory; the source repository is not written
        stores = [os.path.join(r, n) for r, ds, _ in os.walk(self.reg) for n in ds if n.endswith(".git")]
        self.assertEqual(len(stores), 1, stores)
        self.assertEqual(self.git_out("--git-dir", stores[0], "rev-parse", "refs/remotes/origin/main"), squash)
        self.assertEqual(self.git_out("-C", self.repo, "for-each-ref"), before)   # no source ref changed
        self.assertNotEqual(self.git_out("-C", self.repo, "rev-parse", "refs/heads/main"), squash)

    def test_squash_head_without_pr_evidence_is_not_merged(self):
        wt, t, head, squash = self.squash_setup("rr-nopr")
        self.m.merged_pr_heads = lambda _wt: {}
        self.assertEqual(self.m.routine_source_problems(wt), ["SOURCE_NOT_MERGED"])
        self.m._MAIN_REFS.clear()
        self.m.merged_pr_heads = lambda _wt: None               # gh unavailable
        self.assertEqual(self.m.routine_source_problems(wt), ["SOURCE_GIT_UNVERIFIED"])
        self.m.merged_pr_heads = lambda _wt: {head: "0" * 40}   # merge commit not in origin/main
        self.assertEqual(self.m.routine_source_problems(wt), ["SOURCE_NOT_MERGED"])
        self.approve()
        self.call("run")
        self.assertTrue(self.intact(t))


class HoldList(Helpers):
    """LOW-D: the HOLD list protects unconditionally, independent of pins."""

    def hold(self, *paths, text=None):
        with open(os.path.join(self.reg, "hold-list.txt"), "a") as h:
            h.write(text if text is not None else "".join(p + "  # test HOLD\n" for p in paths))

    def test_hold_listed_worktree_is_never_released_retired_or_removable(self):
        wt, t = self.ready_released("hl-released")
        wt2, t2 = self.forgotten("hl-forgotten")
        free_wt, free = self.ready_released("hl-free")
        self.hold(wt, wt2)
        self.approve()
        self.assertIn("HOLD_LISTED", self.row(t)["reasons"])
        rc, out = self.call("run")
        self.assertEqual(rc, 0, out)
        self.assertTrue(self.intact(t) and self.intact(t2), out)
        self.assertIsNone(self.released(t2))
        self.assertFalse(self.intact(free), out)                 # others are unaffected
        g = self.m.worktree_gate(wt)
        self.assertEqual((g["status"], g["code"]), ("HOLD", "HOLD_LISTED"), g)
        rc, out = self.call("run", "--only", os.path.basename(t), "--dry-run")
        self.assertNotIn(os.path.basename(t), json.dumps(self.last_cycle().get("would_select")))

    def test_hold_listed_worktree_stays_hold_even_without_a_cache(self):
        wt, t = self.ready_released("hl-nocache")
        self.approve()
        self.call("run")
        self.assertFalse(self.intact(t))
        self.review_unrecognised(wt, t)
        self.assertEqual(self.m.worktree_gate(wt)["status"], "CLEAR")
        self.hold(wt)
        g = self.m.worktree_gate(wt)
        self.assertEqual((g["status"], g["code"]), ("HOLD", "HOLD_LISTED"), g)

    def test_owner_signed_run_cannot_retire_a_hold_listed_cache(self):
        wt, t = self.ready_released("hl-signed")
        self.hold(wt)
        self.approve(enable=False)
        X = self.m.executor()
        group = self.group(wt, t, ["debug/deps"]) if hasattr(self, "group") else None
        if group is None:
            self.skipTest("no manifest helper")
        path, digest = self.manifest([group])
        _, result = self.run_retire_with(X, path, digest)
        self.assertTrue(self.intact(t), result)
        codes = {h["code"] for g in result.get("groups", []) for h in g.get("holds", [])}
        self.assertIn("HOLD_LISTED", codes, result)

    ready_released = RoutineRelease.ready_released

    def run_retire_with(self, X, path, digest):
        self.mod = X
        return self.run_retire(path, digest, execute=True)

    def registry(self, entries=(), **extra):
        doc = {"kind": "EMUWIZ_HOLD_REGISTRY", "schema": 1, "generated_at": time.time(), "max_age_days": 30,
               "sources": [], "entries": list(entries) or [{"id": "fixture", "status": "UNRESOLVED",
                                                             "branches": ["no-such-hold-branch"], "heads": [], "paths": []}]}
        doc.update(extra)
        with open(os.path.join(self.reg, "hold-registry.json"), "w") as h:
            json.dump(doc, h)

    def test_missing_stale_incomplete_or_malformed_hold_sources_hold_everything(self):
        wt, t = self.ready_released("hl-missing")
        self.approve()
        os.unlink(os.path.join(self.reg, "hold-registry.json"))
        self.assertIn("HOLD_REGISTRY_MISSING", self.row(t)["reasons"])
        self.call("run")
        self.assertTrue(self.intact(t))
        self.assertEqual(self.m.worktree_gate(wt)["code"], "HOLD_REGISTRY_MISSING")
        self.registry(generated_at=time.time() - 40 * 86400)
        self.assertIn("HOLD_REGISTRY_STALE", self.row(t)["reasons"])
        self.registry([{"id": "pr-1", "status": "UNRESOLVED", "paths": [], "branches": [], "heads": []}])
        self.assertIn("HOLD_REGISTRY_INVALID", self.row(t)["reasons"])     # unmatchable = incomplete
        self.registry()
        self.hold(text="relative/path\n")
        self.assertIn("HOLD_SOURCE_UNREADABLE", self.row(t)["reasons"])
        self.call("run")
        self.assertTrue(self.intact(t))

    def test_hold_entry_omitted_from_the_registry_cannot_become_eligible(self):
        # 1) named only by a coordination-table HOLD note
        wt1, t1 = self.ready_released("hl-coord")
        d = os.path.join(self.home, "emuwiz-audit-ledger", "coord")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "COORDINATION.md"), "w") as h:
            h.write(f"- 2026-11-12 davedap-xx: #1 merged. Worktree ~/{os.path.basename(wt1)} HOLD.\n")
        # 2) an UNRESOLVED PR (folder unknown): protected by its branch
        wt2, t2 = self.ready_released("hl-branch")
        # 3) an UNRESOLVED PR whose checkout is gone: its cache is protected by the recorded HEAD
        wt3, t3 = self.ready_released("hl-head")
        head3 = subprocess.run(["git", "-C", wt3, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
        self.git("-C", wt3, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x")
        rec = os.path.join(self.reg, "targets", self.key_of(t3) + ".json")
        with open(rec) as h:
            data = json.load(h)
        data["head"] = "ab" * 20                  # a build HEAD unique to this cache (fixtures share main)
        with open(rec, "w") as h:
            json.dump(data, h)
        self.registry([{"id": "pr-2", "status": "UNRESOLVED", "branches": ["hl-branch"], "heads": [], "paths": []},
                       {"id": "pr-3", "status": "UNRESOLVED", "branches": [], "heads": ["ab" * 20], "paths": []}])
        free_wt, free = self.ready_released("hl-free2")
        self.approve()
        reasons = {t: self.row(t)["reasons"] for t in (t1, t2, t3)}
        self.assertIn("HOLD_COORDINATION", reasons[t1], reasons)
        self.assertIn("HOLD_UNRESOLVED_MATCH", reasons[t2], reasons)
        self.assertIn("HOLD_UNRESOLVED_MATCH", reasons[t3], reasons)
        rc, out = self.call("run")
        self.assertEqual(rc, 0, out)
        for t in (t1, t2, t3):
            self.assertTrue(self.intact(t), (t, out))
        self.assertFalse(self.intact(free), out)
        for wt in (wt1, wt2):
            self.assertEqual(self.m.worktree_gate(wt)["status"], "HOLD")

class HoldSourcesMore(Helpers):
    ready_released = RoutineRelease.ready_released

    def test_memory_note_hold_protects_a_checkout_missing_from_every_other_source(self):
        wt, t = self.ready_released("hm-memory")
        d = os.path.join(self.home, ".claude", "projects", "-p", "memory")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "project_x.md"), "w") as h:
            h.write(f"- #9 merged; worktree ~/{os.path.basename(wt)} stays HOLD.\n")
        free_wt, free = self.ready_released("hm-free")
        self.approve()
        self.assertIn("HOLD_MEMORY_NOTE", self.row(t)["reasons"])
        self.call("run")
        self.assertTrue(self.intact(t))
        self.assertFalse(self.intact(free))
        self.assertEqual(self.m.worktree_gate(wt)["code"], "HOLD_MEMORY_NOTE")

    def test_unreadable_checkout_git_state_holds(self):
        wt, t = self.ready_released("hm-brokengit")
        with open(os.path.join(wt, ".git"), "w") as h:      # a .git that points nowhere
            h.write("gitdir: /nonexistent/worktrees/x\n")
        self.approve()
        self.assertIn("HOLD_SOURCE_UNREADABLE", self.row(t)["reasons"])

    def test_non_github_origin_is_never_merge_evidence(self):
        wt, t = self.ready_released("hm-local-origin")
        origin = os.path.join(self.tmp, "fake-origin.git")
        self.git("clone", "-q", "--bare", self.repo, origin)
        self.git("-C", self.repo, "remote", "add", "origin", origin)
        self.assertEqual(self.m.routine_source_problems(wt), ["SOURCE_GIT_UNVERIFIED"])


class HoldLifecycle(Helpers):
    """One authoritative registry with HELD / RELEASED / UNKNOWN; prose HOLD lines are
    indications reconciled by an owner-signed release, not permanent vetoes."""
    ready_released = RoutineRelease.ready_released

    def doc(self, *entries):
        doc = {"kind": "EMUWIZ_HOLD_REGISTRY", "schema": 2, "generated_at": time.time(), "max_age_days": 30,
               "sources": [], "entries": list(entries)}
        with open(os.path.join(self.reg, "hold-registry.json"), "w") as h:
            json.dump(doc, h)

    def note(self, text, where="memory"):
        d = (os.path.join(self.home, ".claude", "projects", "-p", "memory") if where == "memory"
             else os.path.join(self.home, "emuwiz-audit-ledger", "coord"))
        os.makedirs(d, exist_ok=True)
        name = "project_x.md" if where == "memory" else "COORDINATION.md"
        with open(os.path.join(d, name), "a") as h:
            h.write(text + "\n")

    def held(self, wt, eid="e1"):
        return {"id": eid, "state": "HELD", "paths": [os.path.realpath(wt)], "branches": [], "heads": [],
                "transitions": [{"state": "HELD", "at": time.time() - 3600}]}

    def release(self, eid, wt, key=None):
        import test_emuwiz_cargo_signing as S
        rc, out = self.call("hold-release-request", "--entry", eid, "--worktree", wt, raw=True)
        self.assertEqual(rc, 0, out)
        record = out.splitlines()[0].strip()
        S.sign(record, key=key, namespace="emuwiz-hold-release@1")
        return record

    def reasons(self, t):
        return self.row(t)["reasons"]

    def test_hold_lifted_lines_are_not_hold_indications(self):
        wt, t = self.ready_released("hl-lifted")
        self.note(f"- 2026-11-10: hold lifted on ~/{os.path.basename(wt)}; ready for cleanup.", "coordination")
        self.note(f"- ~/{os.path.basename(wt)}: HOLD released by owner 2026-11-10.")
        self.approve()
        self.assertFalse([r for r in self.reasons(t) if r.startswith("HOLD_")], self.reasons(t))
        self.call("run")
        self.assertFalse(self.intact(t))

    def test_verified_release_ignores_historical_mentions_but_keeps_every_other_gate(self):
        wt, t = self.ready_released("hl-rel")
        wt2, t2 = self.ready_released("hl-rel-pinned")
        self.assertEqual(self.ret("pin", wt2, "--reason", "release-build").returncode, 0)
        wt3, t3 = self.ready_released("hl-rel-dirty")
        for w in (wt, wt2, wt3):
            self.note(f"- worktree ~/{os.path.basename(w)} stays HOLD (2026-11-05).")
            self.note(f"| t | a | {os.path.basename(w)} | #1 | Merged; worktree HOLD |", "coordination")
        self.doc(self.held(wt, "e1"), self.held(wt2, "e2"), self.held(wt3, "e3"))
        self.approve()
        self.assertIn("HOLD_LISTED", self.reasons(t))
        for eid, w in (("e1", wt), ("e2", wt2), ("e3", wt3)):
            rec = self.release(eid, w)
            rc, out = self.call("hold-release-apply", "--record", rec, raw=True)
            self.assertEqual(rc, 0, out)
        with open(os.path.join(wt3, "wip.rs"), "w") as h:          # dirty after the release
            h.write("fn wip() {}")
        self.assertFalse([r for r in self.reasons(t) if r.startswith("HOLD_")], self.reasons(t))
        self.assertIn("PINNED", self.reasons(t2))
        self.assertIn("SOURCE_HISTORY_UNPRESERVED", self.reasons(t3))
        rc, out = self.call("run")
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.intact(t), out)                       # notes were never edited
        self.assertTrue(self.intact(t2) and self.intact(t3), out)
        ev = [h for h in self.history() if h.get("event") == "hold_release"]
        self.assertEqual(len(ev), 3)

    def test_new_hold_after_release_blocks_and_explicit_hold_overrides(self):
        wt, t = self.ready_released("hl-new")
        self.note(f"- ~/{os.path.basename(wt)} HOLD (historical)")
        self.doc(self.held(wt))
        self.approve()
        self.call("hold-release-apply", "--record", self.release("e1", wt))
        self.assertFalse([r for r in self.reasons(t) if r.startswith("HOLD_")])
        self.note(f"- 2026-11-12 owner: ~/{os.path.basename(wt)} back on HOLD for a regression check")
        self.assertIn("HOLD_INDICATION_UNRECONCILED", self.reasons(t))
        self.call("run")
        self.assertTrue(self.intact(t))
        rc, out = self.call("hold-set", "--entry", "e1", "--reason", "regression check", raw=True)
        self.assertEqual(rc, 0, out)
        self.assertIn("HOLD_LISTED", self.reasons(t))

    def test_conflicting_or_unverified_records_fail_closed(self):
        wt, t = self.ready_released("hl-conflict")
        e = self.held(wt)
        self.doc(e)
        self.approve()
        # (a) a RELEASED state written by hand, with no signed transition
        self.doc(dict(e, state="RELEASED", transitions=[]))
        self.assertIn("HOLD_UNRESOLVED_MATCH", self.reasons(t))
        # (b) a release signed by a key that is not the owner's
        self.doc(e)
        import test_emuwiz_cargo_signing as S
        rec = self.release("e1", wt, key=S.keys()["attacker"])
        rc, out = self.call("hold-release-apply", "--record", rec, raw=True)
        self.assertNotEqual(rc, 0, out)
        with open(rec) as h:
            at = json.load(h)["at"]
        self.doc(dict(e, state="RELEASED", transitions=e["transitions"] + [
            {"state": "RELEASED", "at": at, "release_record": os.path.basename(rec)}]))
        self.assertIn("HOLD_UNRESOLVED_MATCH", self.reasons(t))
        # (c) a valid owner release, then the checkout moves on: the release no longer applies
        self.doc(e)
        self.call("hold-release-apply", "--record", self.release("e1", wt))
        self.assertFalse([r for r in self.reasons(t) if r.startswith("HOLD_")])
        self.git("-C", wt, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x")
        self.assertIn("HOLD_UNRESOLVED_MATCH", self.reasons(t))
        # (d) two records disagree (one released, one HELD by branch): HELD wins
        with open(os.path.join(self.reg, "hold-registry.json")) as h:
            doc = json.load(h)
        doc["entries"].append({"id": "e2", "state": "HELD", "paths": [], "branches": ["hl-conflict"], "heads": []})
        with open(os.path.join(self.reg, "hold-registry.json"), "w") as h:
            json.dump(doc, h)
        self.assertIn("HOLD_LISTED", self.reasons(t))
        self.call("run")
        self.assertTrue(self.intact(t))


if __name__ == "__main__":
    unittest.main()
