#!/usr/bin/env python3
"""Compare the released crate and reviewed revision using local pinned sources."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess


def run(argv, **kwargs):
    return subprocess.run(argv, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, help="private directory with fixed/, cb2util/, libbig_int/")
    args = parser.parse_args()
    root = args.root.resolve()
    here = Path(__file__).resolve().parent
    repo = here.parents[1]
    checker = root / "checker"
    (checker / "src").mkdir(parents=True, exist_ok=True)
    (checker / "Cargo.toml").write_text('''[package]
name = "emuwiz-cbcheck"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
bytemuck = "=1.25.1"
codebreaker = "=0.4.0"
codebreaker_fixed = { package = "codebreaker", path = "../fixed" }
[[bin]]
name = "baseline"
path = "src/baseline.rs"
[[bin]]
name = "fixed"
path = "src/fixed.rs"
[profile.dev]
opt-level = 2
''')
    source = (here / "check.rs").read_text()
    for version, prefix in [("baseline", ""), ("fixed", "use codebreaker_fixed as codebreaker;\n")]:
        (checker / "src" / f"{version}.rs").write_text(prefix + source)
    cb = root / "cb2util"
    big = root / "libbig_int"
    run(["gcc", "-O1", "-w", "-DCODE_ENCRYPTION", f"-I{big / 'include'}", f"-I{cb}",
         str(here / "gen.c"), *map(str, [cb / p for p in ["cb2_crypto.c", "arcfour.c", "shs.c"]]),
         *map(str, sorted((big / "src").glob("*.c"))),
         *map(str, sorted((big / "src/low_level_funcs").glob("*.c"))), "-o", str(root / "gen")])
    build = run([str(repo / "scripts/cargo-iso"), "build", "--offline", "--manifest-path",
                 str(checker / "Cargo.toml"), "--message-format=json"], cwd=repo,
                capture_output=True, text=True)
    (root / "build.log").write_text(build.stdout + build.stderr)
    binaries = {}
    for line in build.stdout.splitlines():
        event = json.loads(line)
        if event.get("reason") == "compiler-artifact" and event.get("executable"):
            binaries[event["target"]["name"]] = event["executable"]
    rows = []
    cases = [(0, 0, False), (1, 0, False), (0, 1, False), (0, 1, True),
             (0, 2, False), (1, 2, False), (0, 3, False), (1, 3, False),
             (0, 4, False), (1, 4, False), (0, 5, False), (1, 5, False),
             (1, 1, False), (1, 1, True)]
    for mode, profile, zero in cases:
        label = f"m{mode}-p{profile}" + ("-zero" if zero else "")
        env = os.environ.copy()
        env.pop("ZV", None)
        if zero:
            env["ZV"] = "1"
        data = run([str(root / "gen"), str(mode), "4000", str(profile)],
                   env=env, capture_output=True).stdout
        for version in ["baseline", "fixed"]:
            result = run([binaries[version]], input=data, capture_output=True)
            (root / f"{label}-{version}.log").write_bytes(result.stdout + result.stderr)
            summary = result.stdout.decode().splitlines()[-1]
            row = {"case": label, "version": version,
                   **{k: int(v) for k, v in re.findall(r"(\w+)=(\d+)", summary)}}
            rows.append(row)
            print(json.dumps(row), flush=True)
        (root / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
    assert all(r["c_compat_mismatches"] == r["encrypt_mismatches"] == 0
               for r in rows if r["version"] == "fixed")
    assert all(r["mismatches"] == r["c_roundtrip_mismatches"]
               for r in rows if r["version"] == "fixed")
    golden = run([binaries["fixed"], "--golden"], capture_output=True, text=True)
    (root / "golden.log").write_text(golden.stdout + golden.stderr)
    print(golden.stdout, end="")


if __name__ == "__main__":
    main()
