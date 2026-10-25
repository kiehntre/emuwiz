#!/usr/bin/env python3
"""Clippy warning ratchet against a per-warning baseline.

Each existing warning is recorded once as a fingerprint: lint code, file and
message. Line and column are left out so unrelated edits that move a warning
do not count as new. A warning whose fingerprint (or an extra copy of one)
is not in the baseline fails the check, even when another warning was fixed
and the total stays the same.

  clippy_ratchet.py check CLIPPY_JSON BASELINE
  clippy_ratchet.py count CLIPPY_JSON
  clippy_ratchet.py write CLIPPY_JSON BASELINE [--initial]

`write` only removes fixed warnings from an existing baseline; it refuses to
add any. `--initial` creates a baseline that does not exist yet.
"""
from __future__ import annotations

import collections
import json
import pathlib
import sys

Fingerprint = tuple[str, str, str]

HEADER = (
    "# Clippy warning baseline: one JSON [lint, file, message] per existing warning.\n"
    "# Fix warnings and regenerate with `scripts/clippy-ratchet.sh --write-baseline`;\n"
    "# that refuses to add entries. Never add entries to make a PR pass.\n"
)


def warnings(clippy_json: pathlib.Path) -> collections.Counter[Fingerprint]:
    # One warning is reported once per target that compiles the file, so
    # deduplicate on its exact location before dropping line and column.
    seen: set[tuple[str, str, int, int, str]] = set()
    for line in clippy_json.read_text(encoding="utf-8").splitlines():
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if not isinstance(msg, dict) or msg.get("reason") != "compiler-message":
            continue
        diag = msg["message"]
        if diag.get("level") != "warning" or not diag.get("spans"):
            continue
        span = next((s for s in diag["spans"] if s.get("is_primary")), diag["spans"][0])
        code = (diag.get("code") or {}).get("code", "")
        seen.add((code, span["file_name"], span["line_start"], span["column_start"], diag["message"]))
    return collections.Counter((code, file, message) for code, file, _, _, message in seen)


def read_baseline(path: pathlib.Path) -> collections.Counter[Fingerprint]:
    entries: collections.Counter[Fingerprint] = collections.Counter()
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        item = json.loads(line)
        if not (isinstance(item, list) and len(item) == 3 and all(isinstance(x, str) for x in item)):
            raise ValueError(f"{path}:{number}: expected [lint, file, message]")
        entries[tuple(item)] += 1
    return entries


def write_baseline(path: pathlib.Path, entries: collections.Counter[Fingerprint]) -> None:
    lines = sorted(json.dumps(list(fp), ensure_ascii=False) for fp in entries.elements())
    path.write_text(HEADER + "".join(f"{line}\n" for line in lines), encoding="utf-8")


def describe(fingerprint: Fingerprint, copies: int) -> str:
    code, file, message = fingerprint
    extra = f" (+{copies})" if copies > 1 else ""
    return f"  {file}: [{code or 'warning'}] {message}{extra}"


def main(argv: list[str]) -> int:
    if len(argv) < 2 or argv[0] not in {"check", "count", "write"}:
        print(__doc__, file=sys.stderr)
        return 2
    command, current = argv[0], warnings(pathlib.Path(argv[1]))
    if command == "count":
        print(sum(current.values()))
        return 0
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    baseline_path = pathlib.Path(argv[2])
    if command == "write":
        if "--initial" in argv[3:]:
            if baseline_path.exists():
                print(f"error: {baseline_path} already exists; --initial only creates one.", file=sys.stderr)
                return 1
        else:
            added = current - read_baseline(baseline_path)
            if added:
                print("error: refusing to add warnings to the baseline; fix them instead:", file=sys.stderr)
                for fingerprint, copies in sorted(added.items()):
                    print(describe(fingerprint, copies), file=sys.stderr)
                return 1
        write_baseline(baseline_path, current)
        print(f"Clippy baseline written: {sum(current.values())} warnings")
        return 0

    baseline = read_baseline(baseline_path)
    new, fixed = current - baseline, baseline - current
    print(f"Clippy warnings: {sum(current.values())} (baseline {sum(baseline.values())})")
    if new:
        print(f"error: {sum(new.values())} Clippy warning(s) not in {baseline_path}; fix them:", file=sys.stderr)
        for fingerprint, copies in sorted(new.items()):
            print(describe(fingerprint, copies), file=sys.stderr)
        return 1
    if fixed:
        print(f"note: {sum(fixed.values())} baseline warning(s) fixed; run "
              "scripts/clippy-ratchet.sh --write-baseline to lock in the improvement.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
