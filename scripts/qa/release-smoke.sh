#!/usr/bin/env bash
set -Eeuo pipefail

# Disposable release smoke test. This intentionally uses only the shipped CLI
# entry points; it never needs a display server and never builds the project.

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd -P)"
SMOKE_TIMEOUT="${SMOKE_TIMEOUT:-30}"
KEEP_SMOKE_STATE="${KEEP_SMOKE_STATE:-0}"
SMOKE_HEADLESS="${SMOKE_HEADLESS:-1}"
FAILED=0
RETENTION_ANNOUNCED=0
SMOKE_ROOT=""
APP_BINARY=""
CLI_BINARY=""
LOG_INDEX=0

die_usage() {
    echo "Usage: $0 [--self-test]" >&2
    exit 2
}

fail_stage() {
    local stage="$1"
    local reason="$2"
    FAILED=1
    RETENTION_ANNOUNCED=1
    echo "FAILED: $stage" >&2
    echo "Reason: $reason" >&2
    echo "Smoke root retained at: $SMOKE_ROOT" >&2
    exit 1
}

path_is_inside() {
    local path="$1"
    local root="$2"
    [[ "$path" == "$root" || "$path" == "$root"/* ]]
}

validate_isolated_path() {
    local label="$1"
    local path="$2"
    [[ -n "$path" ]] || return 1
    path_is_inside "$path" "$SMOKE_ROOT" || {
        echo "$label resolves outside smoke root: $path" >&2
        return 1
    }
    case "$path" in
        /home/davedap/.config|/home/davedap/.config/*|\
        /home/davedap/.local/share|/home/davedap/.local/share/*|\
        /home/davedap/.cache|/home/davedap/.cache/*)
            echo "$label resolves to a real EmuWiz path: $path" >&2
            return 1
            ;;
    esac
}

external_snapshot() {
    local output="$1"
    : > "$output"
    local path
    for path in \
        /home/davedap/.config \
        /home/davedap/.local/share \
        /home/davedap/.cache \
        /home/davedap/.local/state; do
        if [[ -d "$path" ]]; then
            # These roots may contain unrelated large installations.  A
            # bounded inventory catches accidental top-level config/data,
            # journals, and histories without making the smoke gate hang on a
            # user's unrelated library or compiler cache.
            find "$path" -maxdepth 3 -type f -printf '%p\n' 2>/dev/null || true
        fi
    done | while IFS= read -r path; do
        if [[ -n "${SMOKE_EXTERNAL_IGNORE_PREFIX:-}" &&
            ( "$path" == "$SMOKE_EXTERNAL_IGNORE_PREFIX" || "$path" == "$SMOKE_EXTERNAL_IGNORE_PREFIX"/* ) ]]; then
            continue
        fi
        printf '%s\n' "$path"
    done | LC_ALL=C sort -u > "$output"
}

cleanup() {
    local status="$?"
    if [[ -z "$SMOKE_ROOT" || ! -d "$SMOKE_ROOT" ]]; then
        return "$status"
    fi
    if (( FAILED != 0 || status != 0 || KEEP_SMOKE_STATE == 1 )); then
        if (( RETENTION_ANNOUNCED == 0 )); then
            echo "Smoke root retained at: $SMOKE_ROOT" >&2
        fi
    else
        rm -rf -- "$SMOKE_ROOT"
    fi
    return "$status"
}
trap cleanup EXIT

run_command() {
    local stage="$1"
    shift
    local safe_name="${stage//[^A-Za-z0-9_.-]/_}"
    LOG_INDEX=$((LOG_INDEX + 1))
    local stdout_log="$SMOKE_ROOT/logs/${LOG_INDEX}-${safe_name}.stdout.log"
    local stderr_log="$SMOKE_ROOT/logs/${LOG_INDEX}-${safe_name}.stderr.log"
    local status

    if timeout --foreground --kill-after=2s "${SMOKE_TIMEOUT}s" \
        env -i \
        PATH="${PATH:-/usr/bin:/bin}" \
        HOME="$SMOKE_HOME" \
        XDG_CONFIG_HOME="$SMOKE_XDG_CONFIG" \
        XDG_DATA_HOME="$SMOKE_XDG_DATA" \
        XDG_CACHE_HOME="$SMOKE_XDG_CACHE" \
        XDG_STATE_HOME="$SMOKE_XDG_STATE" \
        EMUWIZ_CONFIG_HOME="$SMOKE_CONFIG_ROOT" \
        EMUWIZ_DATA_HOME="$SMOKE_DATA_ROOT" \
        LANG=C LC_ALL=C TERM=dumb RUST_BACKTRACE=1 \
        "$@" >"$stdout_log" 2>"$stderr_log"; then
        return 0
    else
        status=$?
    fi
    if (( status == 124 || status == 137 )); then
        echo "timeout after ${SMOKE_TIMEOUT}s; logs: $stdout_log, $stderr_log" >&2
    else
        echo "exit $status; logs: $stdout_log, $stderr_log" >&2
    fi
    return "$status"
}

select_binary() {
    local requested="${EMUWIZ_BINARY:-}"
    local target="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
    local candidate
    if [[ -n "$requested" ]]; then
        candidate="$requested"
    else
        candidate=""
        for name in emuwiz-cli archivefs-cli emuwiz; do
            for profile in release debug; do
                if [[ -x "$target/$profile/$name" ]]; then
                    candidate="$target/$profile/$name"
                    break 2
                fi
            done
        done
    fi
    if [[ -z "$candidate" || ! -f "$candidate" || ! -x "$candidate" ]]; then
        echo "No executable EmuWiz binary found." >&2
        echo "Set EMUWIZ_BINARY=/path/to/emuwiz-cli or CARGO_TARGET_DIR to an existing target." >&2
        return 1
    fi
    APP_BINARY="$(realpath -- "$candidate")"
    CLI_BINARY="$APP_BINARY"
    if [[ "$(basename -- "$APP_BINARY")" == "emuwiz" ]]; then
        local sibling="$(dirname -- "$APP_BINARY")/emuwiz-cli"
        if [[ -x "$sibling" ]]; then
            CLI_BINARY="$(realpath -- "$sibling")"
        else
            CLI_BINARY=""
        fi
    fi
}

write_environment_summary() {
    {
        echo "smoke_root=$SMOKE_ROOT"
        echo "home=$SMOKE_HOME"
        echo "xdg_config_home=$SMOKE_XDG_CONFIG"
        echo "xdg_data_home=$SMOKE_XDG_DATA"
        echo "xdg_cache_home=$SMOKE_XDG_CACHE"
        echo "xdg_state_home=$SMOKE_XDG_STATE"
        echo "emuwiz_config_home=$SMOKE_CONFIG_ROOT"
        echo "emuwiz_data_home=$SMOKE_DATA_ROOT"
        echo "binary=${APP_BINARY:-not-selected}"
        echo "cli=${CLI_BINARY:-not-available}"
        echo "smoke_timeout=$SMOKE_TIMEOUT"
        echo "network=disabled (env -i; no provider commands invoked)"
    } > "$SMOKE_ROOT/logs/environment-summary.txt"
}

self_test() {
    local test_root
    test_root="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-smoke-selftest.XXXXXX")"
    SMOKE_ROOT="$test_root"
    mkdir -p "$SMOKE_ROOT/logs"
    SMOKE_HOME="$SMOKE_ROOT/home"
    SMOKE_XDG_CONFIG="$SMOKE_ROOT/xdg-config"
    SMOKE_XDG_DATA="$SMOKE_ROOT/xdg-data"
    SMOKE_XDG_CACHE="$SMOKE_ROOT/xdg-cache"
    SMOKE_XDG_STATE="$SMOKE_ROOT/xdg-state"
    SMOKE_CONFIG_ROOT="$SMOKE_ROOT/config"
    SMOKE_DATA_ROOT="$SMOKE_ROOT/data"
    mkdir -p "$SMOKE_HOME" "$SMOKE_XDG_CONFIG" "$SMOKE_XDG_DATA" \
        "$SMOKE_XDG_CACHE" "$SMOKE_XDG_STATE" "$SMOKE_CONFIG_ROOT" "$SMOKE_DATA_ROOT"

    echo "[self-test] missing binary refusal"
    if EMUWIZ_BINARY=/definitely/missing select_binary 2>/dev/null; then
        echo "self-test failed: missing binary accepted" >&2
        return 1
    fi
    echo "[self-test] unsafe path refusal"
    if validate_isolated_path unsafe /home/davedap/.config 2>/dev/null; then
        echo "self-test failed: unsafe path accepted" >&2
        return 1
    fi
    echo "[self-test] failed process and timeout handling"
    if run_command self-failed sh -c 'exit 7'; then
        echo "self-test failed: failed process accepted" >&2
        return 1
    fi
    SMOKE_TIMEOUT=1
    if run_command self-timeout sh -c 'sleep 2'; then
        echo "self-test failed: timeout accepted" >&2
        return 1
    fi
    echo "[self-test] cleanup and retained-state behavior"
    local cleanup_root="$SMOKE_ROOT"
    KEEP_SMOKE_STATE=0
    cleanup_root="$SMOKE_ROOT"
    rm -rf -- "$cleanup_root"
    [[ ! -e "$cleanup_root" ]] || return 1
    SMOKE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-smoke-selftest-keep.XXXXXX")"
    KEEP_SMOKE_STATE=1
    local retained="$SMOKE_ROOT"
    cleanup_root="$retained"
    [[ -d "$retained" ]] || return 1
    rm -rf -- "$retained"
    SMOKE_ROOT=""
    echo "RELEASE SMOKE SELF-TEST: PASS"
}

main() {
    [[ "${1:-}" != --self-test || "$#" == 1 ]] || die_usage
    if [[ "${1:-}" == --self-test ]]; then
        trap - EXIT
        self_test
        return
    fi
    [[ "$#" == 0 ]] || die_usage
    [[ "$SMOKE_TIMEOUT" =~ ^[1-9][0-9]*$ ]] || {
        echo "SMOKE_TIMEOUT must be a positive integer" >&2
        return 2
    }

    echo "[1/8] Preparing isolated environment"
    SMOKE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-release-smoke.XXXXXX")"
    mkdir -p "$SMOKE_ROOT/logs" "$SMOKE_ROOT/home" "$SMOKE_ROOT/fixture/source" \
        "$SMOKE_ROOT/config" "$SMOKE_ROOT/data" "$SMOKE_ROOT/cache" "$SMOKE_ROOT/state"
    SMOKE_HOME="$SMOKE_ROOT/home"
    SMOKE_XDG_CONFIG="$SMOKE_ROOT/xdg-config"
    SMOKE_XDG_DATA="$SMOKE_ROOT/xdg-data"
    SMOKE_XDG_CACHE="$SMOKE_ROOT/cache"
    SMOKE_XDG_STATE="$SMOKE_ROOT/state"
    SMOKE_CONFIG_ROOT="$SMOKE_ROOT/config"
    SMOKE_DATA_ROOT="$SMOKE_ROOT/data"
    mkdir -p "$SMOKE_XDG_CONFIG" "$SMOKE_XDG_DATA"
    external_snapshot "$SMOKE_ROOT/logs/external-before.txt"
    write_environment_summary
    echo "Smoke root: $SMOKE_ROOT"

    echo "[2/8] Checking binary"
    select_binary || fail_stage "[2/8] Checking binary" "no executable binary was found"
    write_environment_summary
    if ! run_command version "$APP_BINARY" --version; then
        fail_stage "[2/8] Checking binary" "the selected binary could not report its version"
    fi

    echo "[3/8] Proving isolation"
    validate_isolated_path HOME "$SMOKE_HOME" || fail_stage "[3/8] Proving isolation" "HOME escaped"
    validate_isolated_path XDG_CONFIG_HOME "$SMOKE_XDG_CONFIG" || fail_stage "[3/8] Proving isolation" "XDG_CONFIG_HOME escaped"
    validate_isolated_path XDG_DATA_HOME "$SMOKE_XDG_DATA" || fail_stage "[3/8] Proving isolation" "XDG_DATA_HOME escaped"
    validate_isolated_path XDG_CACHE_HOME "$SMOKE_XDG_CACHE" || fail_stage "[3/8] Proving isolation" "XDG_CACHE_HOME escaped"
    validate_isolated_path XDG_STATE_HOME "$SMOKE_XDG_STATE" || fail_stage "[3/8] Proving isolation" "XDG_STATE_HOME escaped"
    validate_isolated_path EMUWIZ_CONFIG_HOME "$SMOKE_CONFIG_ROOT" || fail_stage "[3/8] Proving isolation" "config override escaped"
    validate_isolated_path EMUWIZ_DATA_HOME "$SMOKE_DATA_ROOT" || fail_stage "[3/8] Proving isolation" "data override escaped"
    if [[ -z "$CLI_BINARY" ]]; then
        echo "No headless CLI sibling found; GUI --version startup was verified, source ingestion is skipped." \
            | tee "$SMOKE_ROOT/logs/headless-limitation.txt"
    fi

    echo "[4/8] First launch"
    if [[ -n "$CLI_BINARY" ]]; then
        run_command first-config-check "$CLI_BINARY" config-check || \
            fail_stage "[4/8] First launch" "config-check failed"
        [[ ! -e "$SMOKE_CONFIG_ROOT/config.toml" && ! -e "$SMOKE_DATA_ROOT/library.sqlite3" ]] || \
            fail_stage "[4/8] First launch" "pristine startup created state before an explicit source was added"
    fi
    if [[ -n "$CLI_BINARY" && "$SMOKE_HEADLESS" != 1 ]]; then
        echo "SMOKE_HEADLESS is not 1; refusing display-dependent GUI automation." > "$SMOKE_ROOT/logs/headless-mode.txt"
    fi

    echo "[5/8] Disposable source ingestion"
    SOURCE="$SMOKE_ROOT/fixture/source"
    # A tiny legal NES-shaped fixture: no copyrighted payload and no external
    # network or emulator dependency.
    printf 'NES\032\001\000\000\000\000\000\000\000\000\000\000\000\000\000' > "$SOURCE/synthetic.nes"
    printf 'EmuWiz smoke fixture\n' > "$SOURCE/README.txt"
    if [[ -n "$CLI_BINARY" ]]; then
        run_command source-add "$CLI_BINARY" source add "$SOURCE" --json || \
            fail_stage "[5/8] Disposable source ingestion" "source add failed"
        [[ -f "$SMOKE_CONFIG_ROOT/config.toml" ]] || \
            fail_stage "[5/8] Disposable source ingestion" "source add did not create the disposable config"
        validate_isolated_path config "$SMOKE_CONFIG_ROOT/config.toml" || \
            fail_stage "[5/8] Disposable source ingestion" "created config escaped"
        run_command source-scan-1 "$CLI_BINARY" source scan "$SOURCE" --json || \
            fail_stage "[5/8] Disposable source ingestion" "first source scan failed"
        if ! grep -Eq '"(archives_new|archives_added|entries_added|catalogue_rows_added)"[[:space:]]*:[[:space:]]*[1-9]' \
            "$SMOKE_ROOT/logs/${LOG_INDEX}-source-scan-1.stdout.log"; then
            fail_stage "[5/8] Disposable source ingestion" "scan did not report a catalogue addition"
        fi
        run_command source-list-1 "$CLI_BINARY" library-list --json || \
            fail_stage "[5/8] Disposable source ingestion" "catalogue could not be listed"
        cp -- "$SMOKE_ROOT/logs/${LOG_INDEX}-source-list-1.stdout.log" "$SMOKE_ROOT/logs/catalogue-first.json"
        run_command source-scan-2 "$CLI_BINARY" source scan "$SOURCE" --json || \
            fail_stage "[5/8] Disposable source ingestion" "rescan failed"
        run_command source-list-2 "$CLI_BINARY" library-list --json || \
            fail_stage "[5/8] Disposable source ingestion" "catalogue could not be listed after rescan"
        cp -- "$SMOKE_ROOT/logs/${LOG_INDEX}-source-list-2.stdout.log" "$SMOKE_ROOT/logs/catalogue-second.json"
        if ! cmp -s "$SMOKE_ROOT/logs/catalogue-first.json" "$SMOKE_ROOT/logs/catalogue-second.json"; then
            fail_stage "[6/8] Disposable source ingestion" "rescan changed catalogue rows"
        fi
    else
        echo "SKIPPED: no CLI/headless binary available" > "$SMOKE_ROOT/logs/source-ingestion-skipped.txt"
    fi

    echo "[6/8] Second start"
    if [[ -n "$CLI_BINARY" ]]; then
        run_command second-config-check "$CLI_BINARY" config-check || \
            fail_stage "[6/8] Second start" "config could not be reopened"
        run_command second-library-list "$CLI_BINARY" library-list --json || \
            fail_stage "[6/8] Second start" "created catalogue could not be reopened"
    fi

    echo "[7/8] SQLite integrity"
    DB_PATH="$SMOKE_DATA_ROOT/library.sqlite3"
    [[ -f "$DB_PATH" ]] || fail_stage "[7/8] SQLite integrity" "expected disposable database was not created"
    validate_isolated_path database "$DB_PATH" || fail_stage "[7/8] SQLite integrity" "database path escaped"
    if [[ -n "$CLI_BINARY" ]]; then
        run_command database-check "$CLI_BINARY" database-check --json || \
            fail_stage "[7/8] SQLite integrity" "database-check command failed"
    fi
    timeout --foreground --kill-after=2s "${SMOKE_TIMEOUT}s" sqlite3 "$DB_PATH" 'PRAGMA quick_check;' \
        > "$SMOKE_ROOT/logs/database-quick-check.txt" \
        || fail_stage "[7/8] SQLite integrity" "sqlite3 could not inspect the database"
    QUICK_CHECK="$(tr -d '\r\n' < "$SMOKE_ROOT/logs/database-quick-check.txt")"
    [[ "$QUICK_CHECK" == ok ]] || fail_stage "[7/8] SQLite integrity" "PRAGMA quick_check returned $QUICK_CHECK"
    timeout --foreground --kill-after=2s "${SMOKE_TIMEOUT}s" sqlite3 "$DB_PATH" 'PRAGMA user_version;' \
        > "$SMOKE_ROOT/logs/database-schema-version.txt"
    echo "DB path: $DB_PATH"
    echo "schema/user_version: $(tr -d '\r\n' < "$SMOKE_ROOT/logs/database-schema-version.txt")"
    echo "quick_check: $QUICK_CHECK"

    echo "[8/8] Clean shutdown and filesystem escape guard"
    external_snapshot "$SMOKE_ROOT/logs/external-after.txt"
    if ! diff -u "$SMOKE_ROOT/logs/external-before.txt" "$SMOKE_ROOT/logs/external-after.txt" > "$SMOKE_ROOT/logs/external-diff.txt"; then
        fail_stage "[8/8] Clean shutdown and filesystem escape guard" "files appeared under real user data roots"
    fi
    find "$SMOKE_ROOT" -type f -printf '%p\n' | LC_ALL=C sort > "$SMOKE_ROOT/logs/filesystem-write-summary.txt"
    echo "No writes escaped the disposable root."
    echo "PASS Fresh start"
    echo "PASS Restart"
    echo "PASS Disposable source"
    echo "PASS SQLite integrity"
    echo "PASS Isolation"
    echo "PASS Clean shutdown"
    echo "RELEASE SMOKE: PASS"
}

main "$@"
