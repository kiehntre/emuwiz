#!/usr/bin/env python3
"""Partition one CI tier's `cargo test` run into parallel shards.

Each shard owns an ordered group of libtest name filters (substring matches).
Shard k runs the tests matching any of its own filters and `--skip`s every
filter of the shards before it; the last shard skips every filter. A test
therefore runs in exactly one shard: the first whose group it matches, or
the last. Every shard keeps the tier's package selection, so the unified
feature set (and the rust-cache key) is the same as an unsharded run.

  ci_test_shards.py shards TIER        JSON list of shard names
  ci_test_shards.py packages TIER      cargo package arguments, one per line
  ci_test_shards.py selection SHARD    libtest arguments, one per line
  ci_test_shards.py check DIR          verify DIR/<shard>.all and DIR/<shard>.selected

`check` reads `cargo test ... -- --list` output that every shard captured
from its own build: the complete enumeration (`.all`) must be identical in
every shard, and the shards' selected enumerations must be pairwise disjoint
and together equal it. Ignored tests are listed too, so they are covered.
"""
from __future__ import annotations

import collections
import json
import pathlib
import re
import sys

# Groups balance wall time only (from CI logs of 2026-10-24): moving a module
# can shift the balance but never drops or duplicates a test.
SHARDS: list[tuple[str, list[str]]] = [
    # One ~530 s and one ~410 s crash-injection test; run beside the others.
    ("recovery", ["interrupted_apply_tests::", "memory_card_inventory::"]),
    ("dat", ["dat::"]),
    ("gui-v2", ["gui_v2::"]),
    ("rest", []),
]

PACKAGES = {
    "gui": ["-p", "archivefs-gui"],
    "core": ["-p", "archivefs-core"],
    "full": ["--workspace"],
}

RUNNING = re.compile(r"^\s*(Running \S+(?: \S+)*?|Doc-tests \S+)\s*$")
HASH = re.compile(r"-[0-9a-f]{16}(?=\)$)")
LISTED = re.compile(r"^(.*): (test|bench)$")


def shards(tier: str) -> list[str]:
    if tier not in PACKAGES:
        raise SystemExit(f"unknown CI tier: {tier}")
    return ["all"] if tier == "gui" else [name for name, _ in SHARDS]


def selection(shard: str) -> list[str]:
    if shard == "all":
        return []
    names = [name for name, _ in SHARDS]
    if shard not in names:
        raise SystemExit(f"unknown test shard: {shard}")
    index = names.index(shard)
    own = SHARDS[index][1]
    if (index == len(SHARDS) - 1) != (not own):
        raise SystemExit("only the last shard may (and must) have no filters")
    # The last shard has no filters of its own, so it skips every group.
    skips = [arg for _, group in SHARDS[:index] for f in group for arg in ("--skip", f)]
    return [*own, *skips]


def parse_listing(text: str) -> collections.Counter[str]:
    """Each listed test keyed by the test binary that lists it."""
    tests: collections.Counter[str] = collections.Counter()
    binary = None
    for raw in text.splitlines():
        line = re.sub(r"\x1b\[[0-9;]*m", "", raw)
        header = RUNNING.match(line)
        if header:
            binary = HASH.sub("", header.group(1))
            continue
        listed = LISTED.match(line)
        if listed:
            if binary is None:
                raise SystemExit(f"test listed before any test binary: {line}")
            tests[f"{binary} :: {listed.group(1)}"] += 1
    return tests


def check(directory: pathlib.Path) -> None:
    alls = sorted(directory.glob("*.all"))
    if not alls:
        raise SystemExit(f"no shard enumerations in {directory}")
    names = {path.stem for path in alls}
    selected_paths = {path.stem for path in directory.glob("*.selected")}
    if names != selected_paths:
        raise SystemExit(f"shards without both enumerations: {sorted(names ^ selected_paths)}")
    expected = set(json.loads((directory / "shards.json").read_text()))
    if names != expected:
        raise SystemExit(f"enumerated shards {sorted(names)} differ from planned {sorted(expected)}")
    reference = None
    union: collections.Counter[str] = collections.Counter()
    for path in alls:
        complete = parse_listing(path.read_text())
        if not complete:
            raise SystemExit(f"{path.name}: no tests enumerated")
        if reference is None:
            reference = complete
        elif complete != reference:
            raise SystemExit(f"{path.name}: complete enumeration differs between shards")
        chosen = parse_listing(path.with_suffix(".selected").read_text())
        if chosen - complete:
            raise SystemExit(f"{path.stem}: selected tests missing from its own build: "
                             f"{sorted(chosen - complete)[:5]}")
        union += chosen
        print(f"{path.stem}: {sum(chosen.values())} of {sum(complete.values())} tests")
    duplicated = sorted(name for name, count in union.items() if count > reference[name])
    missing = sorted(reference - union)
    if duplicated or missing:
        raise SystemExit(f"shard partition is not exact: {len(missing)} missing {missing[:5]}, "
                         f"{len(duplicated)} in more than one shard {duplicated[:5]}")
    print(f"exact partition: {sum(reference.values())} tests, each in exactly one shard")


def main(argv: list[str]) -> None:
    if len(argv) != 3:
        raise SystemExit(__doc__)
    command, value = argv[1], argv[2]
    if command == "shards":
        print(json.dumps(shards(value), separators=(",", ":")))
    elif command == "packages":
        if value not in PACKAGES:
            raise SystemExit(f"unknown CI tier: {value}")
        print("\n".join(PACKAGES[value]))
    elif command == "selection":
        for arg in selection(value):
            print(arg)
    elif command == "check":
        check(pathlib.Path(value))
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main(sys.argv)
