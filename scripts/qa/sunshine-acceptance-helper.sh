#!/usr/bin/env bash
set -euo pipefail

# QA tooling only: prints the standard Sunshine/Moonlight desktop acceptance
# checklist (sourced from the recurring real-desktop QA procedure documented
# in scripts/release/README-RC-ACCEPTANCE.md and
# docs/qa/REAL_EMULATOR_LAUNCH_MATRIX.md) and records the run metadata a person
# doing that QA pass tends to forget between runs: source SHA, artifact SHA,
# DISPLAY, PID, timestamps, log path, pre/post binary hash, and any process
# leftovers. It does not build anything (see scripts/build-release.sh /
# scripts/verify-release-artifact.sh for that) and it never drives the GUI —
# a human performs and answers the checklist by hand.

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd -P)"
# Run records are never written into the repository: they default to the
# user state directory and may be redirected with EMUWIZ_QA_RUN_DIR.
RECORD_DIR="${EMUWIZ_QA_RUN_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/emuwiz-qa/sunshine-runs}"

die() {
    printf 'sunshine-qa: error: %s\n' "$*" >&2
    exit 1
}

note() {
    printf 'sunshine-qa: %s\n' "$*"
}

usage() {
    cat <<'EOF'
Usage:
  scripts/qa/sunshine-acceptance-helper.sh checklist
      Print the standard Sunshine/Moonlight desktop acceptance checklist.

  scripts/qa/sunshine-acceptance-helper.sh start --artifact PATH [--pid PID] [--log PATH]
      Record the start of a Sunshine acceptance run: source SHA, artifact
      SHA-256, DISPLAY, PID (if known yet), startup timestamp, log path, and
      the artifact's pre-run hash. Prints a run ID and writes a record file
      under the run directory (default ~/.local/state/emuwiz-qa/sunshine-runs,
      override with EMUWIZ_QA_RUN_DIR; never inside the repository).

  scripts/qa/sunshine-acceptance-helper.sh end RUN_ID [--pid PID] [--pattern NAME]
      Record the end of a run: end timestamp, the artifact's post-run hash
      (must match the pre-run hash — the artifact must stay immutable), and
      any leftover processes still running (matched by --pid and/or a
      case-insensitive process-name --pattern, e.g. "emuwiz"; matched
      against the process name, not the full command line).

  scripts/qa/sunshine-acceptance-helper.sh show RUN_ID
      Print a previously recorded run file.

This tool does not launch, build, or click anything. It only prints the
checklist and records what a human observed while following it by hand.
EOF
}

print_checklist() {
    cat <<'EOF'
=== EmuWiz Sunshine/Moonlight desktop acceptance checklist ===
(Standard steps for a real-desktop QA pass — see
 scripts/release/README-RC-ACCEPTANCE.md and
 docs/qa/REAL_EMULATOR_LAUNCH_MATRIX.md. Answer each by hand; this script
 records metadata only, it does not verify GUI behaviour for you.)

Before launch:
  [ ] DISPLAY, XAUTHORITY, and XDG_RUNTIME_DIR are exported for this shell.
  [ ] `xdpyinfo` against DISPLAY exits 0 (real Xorg/XFCE session is reachable).
  [ ] The Sunshine / Moonlight desktop session is connected and visible.
  [ ] Run `scripts/qa/sunshine-acceptance-helper.sh start --artifact <path>` now, before
      launching, so the pre-run artifact hash and source SHA are captured.

During the run:
  [ ] The window opens without a startup failure or obvious clipping.
  [ ] Core navigation renders: Home, Sources/DATs, Emulator Setup, and any
      feature pages relevant to this QA pass.
  [ ] No real user libraries or BIOS files were used unless the pass
      explicitly calls for them.
  [ ] Screenshots of anything notable are saved under a disposable path
      (e.g. /tmp/emuwiz-desktop-qa-<timestamp>/), never in the repository.
  [ ] Any launched emulator/game reaches real content, not just a version or
      about screen, if the pass is testing a launch path.
  [ ] Exit is clean (no forced kill) if the pass is testing shutdown/stop
      behaviour.

After the run:
  [ ] Run `scripts/qa/sunshine-acceptance-helper.sh end <run-id>` to capture the end
      timestamp, confirm the artifact hash is unchanged, and check for
      leftover processes.
  [ ] Note any observed defects against the specific checklist line above,
      not as a vague "looked fine" summary.

This checklist is intentionally generic across QA passes; a specific task's
own spec (e.g. a named real-launch QA directive) takes precedence over this
list where the two differ.
EOF
}

guard_record_dir() {
    mkdir -p "$RECORD_DIR"
    local resolved
    resolved="$(CDPATH= cd -- "$RECORD_DIR" && pwd -P)"
    case "$resolved/" in
        "$REPO_ROOT"/*) die "run records must not be written inside the repository: $resolved" ;;
    esac
}

hash_file() {
    local path=$1
    [[ -f "$path" ]] || die "not a file: $path"
    sha256sum "$path" | awk '{print $1}'
}

cmd_start() {
    local artifact="" pid="" log=""
    while (($#)); do
        case "$1" in
            --artifact)
                (($# >= 2)) || die "--artifact requires a path"
                artifact=$2
                shift 2
                ;;
            --pid)
                (($# >= 2)) || die "--pid requires a value"
                pid=$2
                shift 2
                ;;
            --log)
                (($# >= 2)) || die "--log requires a path"
                log=$2
                shift 2
                ;;
            *) die "unknown argument: $1" ;;
        esac
    done
    [[ -n "$artifact" ]] || die "--artifact PATH is required"

    guard_record_dir
    local run_id
    run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
    local record="$RECORD_DIR/$run_id.txt"

    local source_sha
    source_sha="$(cd "$REPO_ROOT" && git rev-parse HEAD 2>/dev/null || echo unknown)"
    local artifact_sha
    artifact_sha="$(hash_file "$artifact")"

    {
        printf 'run_id: %s\n' "$run_id"
        printf 'phase: start\n'
        printf 'source_sha: %s\n' "$source_sha"
        printf 'artifact_path: %s\n' "$(CDPATH= cd -- "$(dirname -- "$artifact")" && pwd -P)/$(basename -- "$artifact")"
        printf 'artifact_sha256_pre: %s\n' "$artifact_sha"
        printf 'display: %s\n' "${DISPLAY:-unset}"
        printf 'xdg_runtime_dir: %s\n' "${XDG_RUNTIME_DIR:-unset}"
        printf 'pid: %s\n' "${pid:-unknown}"
        printf 'startup_timestamp_utc: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
        printf 'log_path: %s\n' "${log:-unrecorded}"
    } >"$record"

    note "run recorded: $record"
    note "run id: $run_id"
    print_checklist
}

find_leftovers() {
    local pid=$1 pattern=$2
    local leftovers=""
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        leftovers+="pid $pid still running"$'\n'
    fi
    if [[ -n "$pattern" ]]; then
        # Match on process name (comm), not full command line: -f would
        # match this script's own invocation, since its argv contains
        # $pattern (it was just passed in as --pattern), including transient
        # self-matches from the subshell pgrep itself runs in.
        local matches
        matches="$(pgrep -i -- "$pattern" 2>/dev/null || true)"
        if [[ -n "$matches" ]]; then
            leftovers+="processes matching '$pattern':"$'\n'"$matches"$'\n'
        fi
    fi
    printf '%s' "$leftovers"
}

cmd_end() {
    local run_id=${1:-}
    [[ -n "$run_id" ]] || die "RUN_ID is required"
    shift
    local pid="" pattern=""
    while (($#)); do
        case "$1" in
            --pid)
                (($# >= 2)) || die "--pid requires a value"
                pid=$2
                shift 2
                ;;
            --pattern)
                (($# >= 2)) || die "--pattern requires a value"
                pattern=$2
                shift 2
                ;;
            *) die "unknown argument: $1" ;;
        esac
    done

    local record="$RECORD_DIR/$run_id.txt"
    [[ -f "$record" ]] || die "no such run record: $record"

    local artifact_path artifact_sha_pre
    artifact_path="$(awk -F': ' '/^artifact_path:/{print $2}' "$record")"
    artifact_sha_pre="$(awk -F': ' '/^artifact_sha256_pre:/{print $2}' "$record")"

    local artifact_sha_post="unavailable"
    local integrity="UNKNOWN (artifact missing at end time)"
    if [[ -f "$artifact_path" ]]; then
        artifact_sha_post="$(hash_file "$artifact_path")"
        if [[ "$artifact_sha_post" == "$artifact_sha_pre" ]]; then
            integrity="OK (unchanged)"
        else
            integrity="MISMATCH — artifact changed during the run"
        fi
    fi

    local leftovers
    leftovers="$(find_leftovers "$pid" "$pattern")"
    [[ -n "$leftovers" ]] || leftovers="none observed"

    {
        printf 'phase: end\n'
        printf 'end_timestamp_utc: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
        printf 'artifact_sha256_post: %s\n' "$artifact_sha_post"
        printf 'artifact_integrity: %s\n' "$integrity"
        printf 'end_pid_checked: %s\n' "${pid:-none}"
        printf 'end_pattern_checked: %s\n' "${pattern:-none}"
        printf 'process_leftovers:\n%s\n' "$leftovers"
    } >>"$record"

    note "run updated: $record"
    note "artifact integrity: $integrity"
    note "process leftovers: $leftovers"
}

cmd_show() {
    local run_id=${1:-}
    [[ -n "$run_id" ]] || die "RUN_ID is required"
    local record="$RECORD_DIR/$run_id.txt"
    [[ -f "$record" ]] || die "no such run record: $record"
    cat "$record"
}

main() {
    local cmd=${1:-}
    case "$cmd" in
        checklist)
            print_checklist
            ;;
        start)
            shift
            cmd_start "$@"
            ;;
        end)
            shift
            cmd_end "$@"
            ;;
        show)
            shift
            cmd_show "$@"
            ;;
        -h | --help | "")
            usage
            ;;
        *)
            die "unknown command: $cmd (see --help)"
            ;;
    esac
}

main "$@"
