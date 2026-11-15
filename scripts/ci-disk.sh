#!/usr/bin/env bash
# Runner disk headroom for the CI test shards. Hosted ubuntu-latest runners are
# not uniform: most have 88 GiB free, a few only 16 GiB. Running out mid-build
# shows up as rustc IO errors, linker bus errors or a crashed runner worker that
# leaves no logs, so check first and name the cause.
#
#   ci-disk.sh ensure MIN_GIB   make MIN_GIB free on /, or fail as infrastructure
#   ci-disk.sh report           show usage; name disk exhaustion as the cause
#
# Runners are disposable, so `ensure` may delete image-provided SDKs, but only
# the ones listed here: nothing in the build or the tests uses them, and the
# Rust toolchain, apt packages and the workspace are never touched.
set -euo pipefail

# Cheapest first (seconds per GiB measured on the runner image); `ensure` stops
# as soon as MIN_GIB is free, so a roomy runner deletes nothing.
REMOVABLE=(
  /usr/local/.ghcup
  /usr/share/swift
  /usr/share/dotnet
  /opt/hostedtoolcache/CodeQL
  /usr/local/lib/android
)
# Below this the runner cannot reliably write its own logs.
LOW_MIB=1024

avail_mib() {
  df --output=avail -BM / | tail -n 1 | tr -dc '0-9'
}

identity() {
  echo "runner: image=${ImageOS:-unknown} version=${ImageVersion:-unknown} name=${RUNNER_NAME:-unknown}"
  df -h /
}

summary() {
  [[ -z "${GITHUB_STEP_SUMMARY:-}" ]] || printf '%s\n' "$1" >> "$GITHUB_STEP_SUMMARY" || true
}

ensure() {
  local min_gib=$1 min_mib dir
  [[ "$min_gib" =~ ^[1-9][0-9]*$ ]] || { echo "ci-disk: MIN_GIB must be a positive integer" >&2; exit 2; }
  min_mib=$((min_gib * 1024))
  identity
  if (($(avail_mib) >= min_mib)); then
    echo "ci-disk: at least $min_gib GiB free; nothing to remove"
    return
  fi
  for dir in "${REMOVABLE[@]}"; do
    (($(avail_mib) < min_mib)) || break
    sudo rm -rf -- "$dir"
    echo "ci-disk: removed $dir, $(($(avail_mib) / 1024)) GiB free"
  done
  if (($(avail_mib) >= min_mib)); then
    echo "::notice title=Small CI runner::Removed unused SDKs to reach $min_gib GiB free on this smaller runner"
    return
  fi
  local message="Only $(($(avail_mib) / 1024)) GiB free of the $min_gib GiB needed after removing unused SDKs. Stopped before building. This is not a code or test failure: rerun the failed job for a new runner."
  echo "::error title=Runner disk (infrastructure)::$message"
  summary "### Runner disk (infrastructure)"$'\n'"$message"
  exit 1
}

report() {
  df -h / || true
  du -sh target 2>/dev/null || true
  (($(avail_mib) < LOW_MIB)) || return 0
  # Test executables only: rust-cache may still save the dependency artifacts.
  find target/debug/deps -maxdepth 1 -type f -executable -delete 2>/dev/null || true
  local message="The runner disk filled up during this job. Any rustc IO error, linker bus error or missing log above is a consequence of that, not a code or test failure."
  echo "::error title=Runner disk exhausted (infrastructure)::$message"
  summary "### Runner disk exhausted (infrastructure)"$'\n'"$message"
}

case "${1:-}:$#" in
  ensure:2) ensure "$2" ;;
  report:1) report ;;
  *) echo "usage: $0 ensure MIN_GIB | report" >&2; exit 2 ;;
esac
