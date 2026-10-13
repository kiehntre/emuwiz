#!/usr/bin/env bash
# Runs the cargo-iso and cargo-retention tests. Fake cargo, private HOME and temp
# worktrees only; builds nothing and never touches ~/.cache/emuwiz-cargo-target(s).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd -P)
bash "$here/test-cargo-iso.sh"
python3 -W error::ResourceWarning "$here/test_emuwiz_cargo_retention.py"
python3 -W error::ResourceWarning "$here/test_emuwiz_cargo_retire.py"
