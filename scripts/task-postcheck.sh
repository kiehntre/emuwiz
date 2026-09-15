#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 --baseline <file> --allow <path-or-prefix> [--allow <path-or-prefix>...]" >&2
    exit 2
}

repo_root="$(git rev-parse --show-toplevel)"
baseline=""
allowed=()
while (($# > 0)); do
    case "$1" in
        --baseline)
            (($# >= 2)) || usage
            baseline="$2"
            shift 2
            ;;
        --allow)
            (($# >= 2)) || usage
            allowed+=("$2")
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

[[ -n "$baseline" ]] || usage
((${#allowed[@]} > 0)) || usage
cd "$repo_root"
scope_args=(--baseline-file "$baseline")
for path in "${allowed[@]}"; do
    scope_args+=("$path")
done
"$repo_root/scripts/check-working-tree-scope.sh" "${scope_args[@]}"
"$repo_root/scripts/check-main-rs-boundary.sh"
git diff --check
echo "Final changed paths:"
{
    git diff --name-only
    git diff --cached --name-only
    git ls-files --others --exclude-standard
} | sed '/^$/d' | sort -u
