#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 [--baseline <file>]" >&2
    exit 2
}

repo_root="$(git rev-parse --show-toplevel)"
baseline=""
while (($# > 0)); do
    case "$1" in
        --baseline)
            (($# >= 2)) || usage
            baseline="$2"
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
echo "Repository root: $repo_root"
echo "Branch: $(git branch --show-current)"
echo "HEAD: $(git rev-parse HEAD)"
echo "Status:"
git status --short
echo "GUI library root lines: $(wc -l < crates/archivefs-gui/src/lib.rs)"
echo "GUI library root fixed maximum: none; the 30-line diff ratchet is reported by the boundary guard."
echo "Reminder: create a scope baseline before editing task files."
if [[ -n "$baseline" ]]; then
    "$repo_root/scripts/check-working-tree-scope.sh" --write-baseline "$baseline"
fi
