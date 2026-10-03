#!/usr/bin/env bash
# Opt-in large-DAT memory benchmark driver (synthetic data only, under $DIR).
# usage: bench-large-dat-memory.sh BINARY LABEL [records...]   (default 100000 500000 1000000)
# Prints one TSV row per (records, mode): peak RSS KiB and wall seconds from /usr/bin/time -v.
set -euo pipefail
bin=$1; label=$2; shift 2
dir=${DIR:-/tmp/emuwiz-largedat}
sizes=("$@"); [ ${#sizes[@]} -gt 0 ] || sizes=(100000 500000 1000000)
for n in "${sizes[@]}"; do
  [ -f "$dir/synthetic-$n.dat" ] || "$bin" gen "$dir" "$n" >/dev/null
  for mode in parse persist index; do
    out=$( { /usr/bin/time -v "$bin" "$mode" "$dir" "$n"; } 2>&1 )
    rss=$(awk '/Maximum resident/{print $NF}' <<<"$out")
    wall=$(awk '/Elapsed \(wall/{print $NF}' <<<"$out")
    extra=$({ grep -E '^(db_bytes|lookup=|phase=index_build)' <<<"$out" || true; } | tr '\n' ' ')
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$n" "$mode" "$rss" "$wall" "$extra"
  done
  rm -f "$dir/bench-$n.sqlite"
done
