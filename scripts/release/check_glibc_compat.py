#!/usr/bin/env python3
"""Fail if release ELF binaries need a newer glibc than the supported baseline.

Reads the GLIBC_x.y symbol-version requirements (`readelf -V`) of every ELF
file in a release .tar.xz (or an extracted directory) and exits non-zero when
any requirement exceeds --max (default: Ubuntu 24.04's glibc 2.39). Nothing is
extracted to disk by name: ELF members are copied to numbered temp files.
"""
import argparse
import os
import pathlib
import re
import subprocess
import sys
import tarfile
import tempfile

BASELINE = "2.39"
ELF_MAGIC = b"\x7fELF"
NEED = re.compile(r"\bGLIBC_(\d+(?:\.\d+)+)\b")


def parse(version: str) -> tuple[int, ...]:
    return tuple(int(part) for part in version.split("."))


def glibc_requirements(elf: pathlib.Path) -> set[str]:
    out = subprocess.run(["readelf", "-V", str(elf)], text=True, check=True,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, env={**os.environ, "LC_ALL": "C"}).stdout
    return set(NEED.findall(out))


def collect(source: pathlib.Path, scratch: pathlib.Path) -> dict[str, pathlib.Path]:
    """Map display name -> readable ELF path."""
    found: dict[str, pathlib.Path] = {}
    if source.is_dir():
        for path in sorted(source.rglob("*")):
            if path.is_file() and not path.is_symlink() and path.read_bytes()[:4] == ELF_MAGIC:
                found[str(path.relative_to(source))] = path
        return found
    with tarfile.open(source, "r:*") as archive:
        for index, member in enumerate(archive):
            if not member.isreg():
                continue
            handle = archive.extractfile(member)
            if handle is None or handle.read(4) != ELF_MAGIC:
                continue
            handle.seek(0)
            target = scratch / str(index)
            target.write_bytes(handle.read())
            found[member.name] = target
    return found


def check(source: pathlib.Path, maximum: str, require: tuple[str, ...]) -> list[str]:
    problems: list[str] = []
    with tempfile.TemporaryDirectory(prefix="glibc-compat.") as scratch:
        elves = collect(source, pathlib.Path(scratch))
        for needed in require:
            if not any(name == needed or name.endswith("/" + needed) for name in elves):
                problems.append(f"expected ELF not found: {needed}")
        for name, path in sorted(elves.items()):
            versions = glibc_requirements(path)
            highest = max(versions, key=parse, default=None)
            print(f"{name}: highest GLIBC {highest or 'none'}")
            if highest and parse(highest) > parse(maximum):
                problems.append(f"{name} requires GLIBC_{highest} (> GLIBC_{maximum})")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("source", type=pathlib.Path, help="release .tar.xz or extracted directory")
    ap.add_argument("--max", default=BASELINE, help=f"highest allowed glibc (default {BASELINE})")
    ap.add_argument("--require", action="append", default=[], metavar="NAME",
                    help="ELF that must be present, e.g. bin/emuwiz (repeatable)")
    args = ap.parse_args(argv)
    problems = check(args.source, args.max, tuple(args.require))
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
