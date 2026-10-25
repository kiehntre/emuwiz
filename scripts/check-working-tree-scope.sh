#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
Usage:
  check-working-tree-scope.sh --write-baseline <file>
  check-working-tree-scope.sh [--baseline-file <file>] <allowed-path-or-prefix>...
EOF
    exit 2
}

repo_root="$(git rev-parse --show-toplevel)"
baseline_file=""
write_baseline=""
allowed=()

while (($# > 0)); do
    case "$1" in
        --baseline-file)
            (($# >= 2)) || usage
            baseline_file="$2"
            shift 2
            ;;
        --write-baseline)
            (($# >= 2)) || usage
            write_baseline="$2"
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        --)
            shift
            allowed+=("$@")
            break
            ;;
        -*)
            usage
            ;;
        *)
            allowed+=("$1")
            shift
            ;;
    esac
done

cd "$repo_root"

collect_paths() {
    git diff --name-only
    git diff --cached --name-only
    git ls-files --others --exclude-standard
}

if [[ -n "$write_baseline" ]]; then
    mkdir -p "$(dirname "$write_baseline")"
    collect_paths | sed '/^$/d' | sort -u >"$write_baseline"
    echo "Wrote baseline: $write_baseline"
    cat "$write_baseline"
    exit 0
fi

if [[ -n "$baseline_file" ]]; then
    [[ -f "$baseline_file" ]] || {
        echo "Baseline file does not exist: $baseline_file" >&2
        exit 2
    }
fi
((${#allowed[@]} > 0)) || usage

declare -A baseline=()
if [[ -n "$baseline_file" ]]; then
    while IFS= read -r path; do
        [[ -n "$path" ]] && baseline["$path"]=1
    done <"$baseline_file"
fi

declare -A seen=()
unexpected=()
while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    [[ -n "${seen[$path]:-}" ]] && continue
    seen["$path"]=1
    [[ -n "${baseline[$path]:-}" ]] && continue
    permitted=0
    for prefix in "${allowed[@]}"; do
        if [[ "$prefix" == */ ]]; then
            if [[ "$path" == "$prefix"* ]]; then
                permitted=1
                break
            fi
        elif [[ "$path" == "$prefix" || "$path" == "$prefix/"* ]]; then
            permitted=1
            break
        fi
    done
    if ((permitted == 0)); then
        unexpected+=("$path")
    fi
done < <(collect_paths)

if ((${#unexpected[@]} > 0)); then
    echo "FAIL: unexpected changed paths outside the allowed scope:" >&2
    printf '  %s\n' "${unexpected[@]}" >&2
    exit 1
fi

echo "PASS: working-tree changes are within the allowed scope."
