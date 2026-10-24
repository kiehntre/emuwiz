#!/usr/bin/env python3
"""Tests for scripts/ci_test_shards.py against a real libtest binary."""
from __future__ import annotations

import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "ci_test_shards.py"
sys.path.insert(0, str(SCRIPT.parent))
import ci_test_shards  # noqa: E402

# Names that hit every group, several groups at once, nested paths, and an
# ignored test; the same names appear in two binaries.
FIXTURE = """
mod dat { #[test] fn a() {} mod rename_apply { mod interrupted_apply_tests { #[test] fn slow() {} } } }
mod memory_card_inventory { #[test] fn c() {} #[test] #[ignore] fn ignored() {} }
mod gui_v2 { #[test] fn d() {} mod dat { #[test] fn overlap() {} } }
mod mame_dat { #[test] fn e() {} }
mod update { #[test] fn f() {} }
#[test] fn top() {}
"""


def cargo_listing(binaries: dict[str, pathlib.Path], args: list[str]) -> str:
    """What `cargo test ... -- --list ARGS 2>&1` prints for these binaries."""
    out = ["    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.1s"]
    for header, binary in binaries.items():
        out.append(f"     Running {header}")
        listed = subprocess.run([str(binary), "--list", *args], capture_output=True, text=True, check=True)
        out.append(listed.stdout)
    return "\n".join(out)


@unittest.skipIf(shutil.which("rustc") is None, "rustc is required for a real libtest binary")
class PartitionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="emuwiz-ci-shards-")
        self.root = pathlib.Path(self.tmp.name)
        source = self.root / "fixture.rs"
        source.write_text(FIXTURE)
        self.binaries = {}
        for header in ("unittests src/lib.rs (target/debug/deps/archivefs_core-0123456789abcdef)",
                       "tests/second.rs (target/debug/deps/second-fedcba9876543210)"):
            binary = self.root / header.split("/")[-1].split("-")[0]
            subprocess.run(["rustc", "--edition", "2021", "--test", str(source), "-o", str(binary)],
                           check=True, capture_output=True)
            self.binaries[header] = binary
        self.lists = self.root / "lists"
        self.lists.mkdir()

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def enumerate(self, tier: str) -> list[str]:
        shards = ci_test_shards.shards(tier)
        (self.lists / "shards.json").write_text(json.dumps(shards))
        for shard in shards:
            (self.lists / f"{shard}.all").write_text(cargo_listing(self.binaries, []))
            (self.lists / f"{shard}.selected").write_text(
                cargo_listing(self.binaries, ci_test_shards.selection(shard)))
        return shards

    def check(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run([sys.executable, "-I", str(SCRIPT), "check", str(self.lists)],
                              capture_output=True, text=True, check=False)

    def test_real_enumeration_is_an_exact_partition(self) -> None:
        for tier in ("full", "core", "gui"):
            with self.subTest(tier=tier):
                for path in self.lists.iterdir():
                    path.unlink()
                self.enumerate(tier)
                result = self.check()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("exact partition: 18 tests, each in exactly one shard", result.stdout)

    def test_first_matching_shard_owns_overlapping_names(self) -> None:
        self.enumerate("full")
        owners = {}
        for shard in ci_test_shards.shards("full"):
            for name in ci_test_shards.parse_listing((self.lists / f"{shard}.selected").read_text()):
                owners.setdefault(name.split(" :: ", 1)[1], set()).add(shard)
        self.assertEqual(owners["dat::rename_apply::interrupted_apply_tests::slow"], {"recovery"})
        self.assertEqual(owners["memory_card_inventory::ignored"], {"recovery"})
        self.assertEqual(owners["gui_v2::dat::overlap"], {"dat"})
        self.assertEqual(owners["mame_dat::e"], {"dat"})  # substring match, not a path segment
        self.assertEqual(owners["gui_v2::d"], {"gui-v2"})
        self.assertEqual(owners["update::f"], {"rest"})
        self.assertEqual(owners["top"], {"rest"})

    def test_missing_duplicated_or_inconsistent_enumerations_fail(self) -> None:
        cases = {
            "missing": lambda: (self.lists / "dat.selected").write_text(cargo_listing(self.binaries, ["--skip", ""])),
            "duplicated": lambda: (self.lists / "rest.selected").write_text(cargo_listing(self.binaries, [])),
            "different build": lambda: (self.lists / "gui-v2.all").write_text(
                cargo_listing(dict(list(self.binaries.items())[:1]), [])),
            "shard absent": lambda: [(self.lists / f"rest.{kind}").unlink() for kind in ("all", "selected")],
            "unplanned shard": lambda: (self.lists / "shards.json").write_text('["recovery","dat","gui-v2"]'),
        }
        for label, corrupt in cases.items():
            with self.subTest(label):
                for path in self.lists.iterdir():
                    path.unlink()
                self.enumerate("full")
                corrupt()
                result = self.check()
                self.assertNotEqual(result.returncode, 0, result.stdout)


WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"
STUB_CARGO = """#!/bin/bash
printf '%s\\037' "$@" >> "$ARGV_LOG"; echo >> "$ARGV_LOG"
if [[ " $* " == *" --list "* ]]; then
  echo "     Running unittests src/lib.rs (target/debug/deps/x-0123456789abcdef)" >&2
  echo "dat::a: test"
fi
"""


def workflow_step(name: str) -> str:
    text = WORKFLOW.read_text(encoding="utf-8")
    match = re.search(r"(?ms)^      - (?:id: \w+\n        )?name: " + re.escape(name)
                      + r"\n.*?^        run: \|\n(.*?)(?=^      - |\n  \S|\Z)", text)
    if match is None:
        raise AssertionError(f"step {name!r} not found in {WORKFLOW}")
    return re.sub(r"(?m)^          ", "", match.group(1))


class WorkflowStepTests(unittest.TestCase):
    """Run the real enumerate and test steps with a stub cargo."""

    def argv(self, tier: str, shard: str) -> list[list[str]]:
        with tempfile.TemporaryDirectory(prefix="emuwiz-ci-steps-") as tmp:
            bin_dir = pathlib.Path(tmp) / "bin"
            bin_dir.mkdir()
            (bin_dir / "cargo").write_text(STUB_CARGO)
            (bin_dir / "cargo").chmod(0o755)
            log = pathlib.Path(tmp) / "argv"
            env = {"PATH": f"{bin_dir}:/usr/bin:/bin", "ARGV_LOG": str(log), "CI_TIER": tier,
                   "SHARD": shard, "RUNNER_TEMP": tmp}
            for name in ("Build and enumerate this shard's tests", "Run tests selected for the change"):
                result = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", workflow_step(name)], cwd=ROOT,
                                        env=env, capture_output=True, text=True, check=False)
                self.assertEqual(result.returncode, 0, result.stderr)
            return [line.split("\x1f")[:-1] for line in log.read_text().splitlines()]

    def test_the_test_run_uses_exactly_the_enumerated_selection(self) -> None:
        for tier in ("gui", "core", "full"):
            for shard in ci_test_shards.shards(tier):
                with self.subTest(tier=tier, shard=shard):
                    packages = ci_test_shards.PACKAGES[tier]
                    selection = ci_test_shards.selection(shard)
                    calls = self.argv(tier, shard)
                    self.assertEqual(calls[0], ["test", *packages, "--no-run"])
                    self.assertEqual(calls[1], ["test", *packages, "--no-fail-fast", "--", "--list"])
                    self.assertEqual(calls[2], ["test", *packages, "--no-fail-fast", "--", "--list", *selection])
                    self.assertEqual(calls[3], ["test", *packages, "--no-fail-fast", "--", *selection])
                    extra = [["check", "-p", "archivefs-cli", "-p", "archivefs-gui", "--locked"]] \
                        if (tier, shard) == ("core", "rest") else []
                    self.assertEqual(calls[4:], extra)

    def test_an_unknown_tier_or_shard_fails_the_step(self) -> None:
        for tier, shard in (("other", "rest"), ("full", "other")):
            with self.subTest(tier=tier, shard=shard), self.assertRaises(AssertionError):
                self.argv(tier, shard)


class SelectionTests(unittest.TestCase):
    def test_last_shard_skips_every_filter_and_earlier_shards_skip_their_predecessors(self) -> None:
        names = [name for name, _ in ci_test_shards.SHARDS]
        self.assertEqual(len(names), len(set(names)))
        every = [f for _, group in ci_test_shards.SHARDS for f in group]
        self.assertEqual(ci_test_shards.selection(names[-1]), [a for f in every for a in ("--skip", f)])
        for index, (name, group) in enumerate(ci_test_shards.SHARDS[:-1]):
            self.assertTrue(group)
            earlier = [f for _, g in ci_test_shards.SHARDS[:index] for f in g]
            self.assertEqual(ci_test_shards.selection(name), [*group, *(a for f in earlier for a in ("--skip", f))])

    def test_tiers(self) -> None:
        self.assertEqual(ci_test_shards.shards("gui"), ["all"])
        self.assertEqual(ci_test_shards.selection("all"), [])
        for tier in ("core", "full"):
            self.assertEqual(ci_test_shards.shards(tier), [name for name, _ in ci_test_shards.SHARDS])
        self.assertEqual(ci_test_shards.PACKAGES["full"], ["--workspace"])

    def test_unknown_tier_or_shard_is_refused(self) -> None:
        for args in (["shards", "other"], ["packages", "other"], ["selection", "other"]):
            with self.subTest(args=args):
                result = subprocess.run([sys.executable, "-I", str(SCRIPT), *args],
                                        capture_output=True, text=True, check=False)
                self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
