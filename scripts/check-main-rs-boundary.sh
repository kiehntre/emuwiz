#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 [--base <commit>]" >&2
    exit 2
}

repo_root="$(git rev-parse --show-toplevel)"
# The GUI coordination boundary is the library root; the three
# executables are thin launchers in src/bin/ with no logic to ratchet.
main_path="crates/archivefs-gui/src/lib.rs"
# The boundary was `src/main.rs` until the GUI became a library. Rename
# detection is enabled and the old path is kept in the pathspec so a diff
# whose base predates that move measures the real delta rather than
# reporting the whole file as new.
legacy_main_path="crates/archivefs-gui/src/main.rs"
mode="WORKTREE"
base=""

while (($# > 0)); do
    case "$1" in
        --base)
            (($# >= 2)) || usage
            mode="BASE-SHA"
            base="$2"
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        *)
            usage
            ;;
    esac
done

cd "$repo_root"
if [[ "$mode" == "BASE-SHA" ]]; then
    git rev-parse --verify "${base}^{commit}" >/dev/null
    diff_args=("${base}...HEAD")
else
    diff_args=(HEAD)
fi

numstat="$(git diff --numstat --find-renames "${diff_args[@]}" -- "$main_path" "$legacy_main_path")"
added=0
deleted=0
if [[ -n "$numstat" ]]; then
    read -r added deleted _ <<<"$numstat"
    [[ "$added" =~ ^[0-9]+$ ]] || { echo "Cannot measure binary GUI root diff" >&2; exit 1; }
    [[ "$deleted" =~ ^[0-9]+$ ]] || { echo "Cannot measure binary GUI root diff" >&2; exit 1; }
fi
net=$((added - deleted))
threshold=30

echo "GUI root boundary: file=$main_path mode=$mode added=$added deleted=$deleted net=$net threshold=$threshold"
if ((net > threshold)); then
    if [[ "${EMUWIZ_ALLOW_MAIN_RS_GROWTH:-}" == "1" ]]; then
        echo "WARNING: main.rs grew beyond the architectural threshold with EMUWIZ_ALLOW_MAIN_RS_GROWTH=1."
        echo "The GUI library root is a coordination boundary; move feature-specific logic to a focused module or justify the exception."
        exit 0
    fi
    echo "FAIL: the GUI library root is a coordination boundary." >&2
    echo "Move feature-specific logic to a focused module or justify an explicit exception." >&2
    exit 1
fi

echo "PASS: GUI library root growth is within the ratchet threshold."
