#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
TOOL="$SCRIPT_DIR/upgrade_preflight.py"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-upgrade-preflight-test.XXXXXX")"
trap 'rm -rf -- "$ROOT"' EXIT

make_db() {
    local path="$1" version="$2"
    rm -f -- "$path"
    python3 - "$path" "$version" <<'PY'
import sqlite3, sys
path, version = sys.argv[1], int(sys.argv[2])
db = sqlite3.connect(path)
db.execute(f"PRAGMA user_version={version}")
db.execute("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT)")
db.execute("INSERT INTO schema_migrations VALUES (?, 'test')", (version,))
db.commit(); db.close()
PY
}

run_case() {
    local name="$1" expected="$2"
    shift 2
    set +e
    python3 "$TOOL" "$@" >"$ROOT/$name.out" 2>"$ROOT/$name.err"
    local status=$?
    set -e
    [[ "$status" == "$expected" ]] || {
        echo "self-test failed: $name expected $expected got $status" >&2
        cat "$ROOT/$name.out" "$ROOT/$name.err" >&2
        exit 1
    }
}

# The preflight's target schema is the library's newest migration.
REPO="$(cd -- "$SCRIPT_DIR/../.." && pwd -P)"
CURRENT="$(sed -n 's/^CURRENT_SCHEMA = \([0-9][0-9]*\)$/\1/p' "$TOOL")"
NEWEST="$(sed -n '/^const MIGRATIONS/,/^];/p' "$REPO/crates/archivefs-core/src/database.rs" \
    | grep -oE 'version: [0-9]+' | awk '{print $2}' | sort -n | tail -1)"
[[ -n "$CURRENT" && "$CURRENT" == "$NEWEST" ]] || {
    echo "self-test failed: upgrade_preflight.py CURRENT_SCHEMA=$CURRENT but the newest migration is $NEWEST" >&2
    exit 1
}

mkdir -p "$ROOT/fresh"
run_case fresh 0 --config-root "$ROOT/fresh/config" --data-root "$ROOT/fresh/data" \
    --legacy-config-root "$ROOT/fresh/legacy-config" --legacy-data-root "$ROOT/fresh/legacy-data"

mkdir -p "$ROOT/legacy-only/config" "$ROOT/legacy-only/data" "$ROOT/legacy-only/mount"
printf 'mount_root = "%s"\nsource_folders = []\n' "$ROOT/legacy-only/mount" > "$ROOT/legacy-only/config/config.toml"
make_db "$ROOT/legacy-only/data/library.sqlite3" "$CURRENT"
run_case legacy-only 1 --config-root "$ROOT/legacy-only/unused-config" --data-root "$ROOT/legacy-only/unused-data" \
    --legacy-config-root "$ROOT/legacy-only/config" --legacy-data-root "$ROOT/legacy-only/data"

mkdir -p "$ROOT/current/config" "$ROOT/current/data" "$ROOT/current/source" "$ROOT/current/mount"
printf 'mount_root = "%s"\n\n[[source]]\npath = "%s"\nenabled = true\n' "$ROOT/current/mount" "$ROOT/current/source" > "$ROOT/current/config/config.toml"
make_db "$ROOT/current/data/library.sqlite3" "$CURRENT"
run_case current 0 --config-root "$ROOT/current/config" --data-root "$ROOT/current/data" \
    --legacy-config-root "$ROOT/current/legacy-config" --legacy-data-root "$ROOT/current/legacy-data"

mkdir -p "$ROOT/mixed/config" "$ROOT/mixed/data" "$ROOT/mixed/mount" "$ROOT/mixed/source"
printf 'mount_root = "%s"\nsource_folders = []\n\n[[source]]\npath = "%s"\nenabled = true\n' "$ROOT/mixed/mount" "$ROOT/mixed/source" > "$ROOT/mixed/config/config.toml"
make_db "$ROOT/mixed/data/library.sqlite3" "$CURRENT"
run_case mixed-config 1 --config-root "$ROOT/mixed/config" --data-root "$ROOT/mixed/data" \
    --legacy-config-root "$ROOT/mixed/legacy-config" --legacy-data-root "$ROOT/mixed/legacy-data"

mkdir -p "$ROOT/invalid/config" "$ROOT/invalid/data"
printf 'this is not valid = [toml\n' > "$ROOT/invalid/config/config.toml"
make_db "$ROOT/invalid/data/library.sqlite3" "$CURRENT"
run_case invalid-config 3 --config-root "$ROOT/invalid/config" --data-root "$ROOT/invalid/data" \
    --legacy-config-root "$ROOT/invalid/legacy-config" --legacy-data-root "$ROOT/invalid/legacy-data"

make_db "$ROOT/current/data/library.sqlite3" 19
run_case old-schema 1 --config-root "$ROOT/current/config" --data-root "$ROOT/current/data" \
    --legacy-config-root "$ROOT/current/legacy-config" --legacy-data-root "$ROOT/current/legacy-data"
make_db "$ROOT/current/data/library.sqlite3" $((CURRENT + 1))
run_case new-schema 2 --config-root "$ROOT/current/config" --data-root "$ROOT/current/data" \
    --legacy-config-root "$ROOT/current/legacy-config" --legacy-data-root "$ROOT/current/legacy-data"
printf 'not sqlite' > "$ROOT/current/data/library.sqlite3"
run_case malformed-db 3 --config-root "$ROOT/current/config" --data-root "$ROOT/current/data" \
    --legacy-config-root "$ROOT/current/legacy-config" --legacy-data-root "$ROOT/current/legacy-data"

mkdir -p "$ROOT/both/config" "$ROOT/both/data" "$ROOT/both/legacy-config" "$ROOT/both/legacy-data" "$ROOT/both/mount"
printf 'mount_root = "%s"\n' "$ROOT/both/mount" > "$ROOT/both/config/config.toml"
printf 'source_folders = []\n' > "$ROOT/both/legacy-config/config.toml"
make_db "$ROOT/both/data/library.sqlite3" "$CURRENT"
printf x > "$ROOT/both/legacy-data/library.sqlite3"
run_case both-roots 2 --config-root "$ROOT/both/config" --data-root "$ROOT/both/data" \
    --legacy-config-root "$ROOT/both/legacy-config" --legacy-data-root "$ROOT/both/legacy-data"

mkdir -p "$ROOT/journal/config" "$ROOT/journal/data/rename-transactions" "$ROOT/journal/data/mod-journals"
make_db "$ROOT/journal/data/library.sqlite3" "$CURRENT"
printf '{"state":"Applying"}\n' > "$ROOT/journal/data/rename-transactions/one.json"
printf '{"state":"RollingBack"}\n' > "$ROOT/journal/data/mod-journals/mod.json"
run_case journal 2 --config-root "$ROOT/journal/config" --data-root "$ROOT/journal/data" \
    --legacy-config-root "$ROOT/journal/legacy-config" --legacy-data-root "$ROOT/journal/legacy-data"

mkdir -p "$ROOT/mount/config" "$ROOT/mount/data"
printf 'source_folders = ["/mnt/not-mounted/emuwiz"]\n' > "$ROOT/mount/config/config.toml"
make_db "$ROOT/mount/data/library.sqlite3" "$CURRENT"
run_case missing-mount 2 --config-root "$ROOT/mount/config" --data-root "$ROOT/mount/data" \
    --legacy-config-root "$ROOT/mount/legacy-config" --legacy-data-root "$ROOT/mount/legacy-data"

mkdir -p "$ROOT/secret/config" "$ROOT/secret/data" "$ROOT/secret/mount"
printf 'mount_root = "%s"\nromm_token = "DO_NOT_PRINT"\n' "$ROOT/secret/mount" > "$ROOT/secret/config/config.toml"
make_db "$ROOT/secret/data/library.sqlite3" "$CURRENT"
run_case secret 0 --config-root "$ROOT/secret/config" --data-root "$ROOT/secret/data" \
    --legacy-config-root "$ROOT/secret/legacy-config" --legacy-data-root "$ROOT/secret/legacy-data" \
    --json "$ROOT/secret/report.json"
if grep -R "DO_NOT_PRINT" "$ROOT/secret/secret.out" "$ROOT/secret/report.json" 2>/dev/null; then
    echo "self-test failed: secret leaked" >&2
    exit 1
fi

mkdir -p "$ROOT/cache/config" "$ROOT/cache/data/identity/artwork/thumbnails"
make_db "$ROOT/cache/data/library.sqlite3" "$CURRENT"
printf x > "$ROOT/cache/data/identity/artwork/thumbnails/thumb.png"
run_case cache 0 --config-root "$ROOT/cache/config" --data-root "$ROOT/cache/data" \
    --legacy-config-root "$ROOT/cache/legacy-config" --legacy-data-root "$ROOT/cache/legacy-data" \
    --backup-manifest "$ROOT/cache/backup.json"
if python3 - "$ROOT/cache/backup.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))
raise SystemExit(0 if any("thumbnails" in entry["path"] for entry in payload["entries"]) else 1)
PY
then
    echo "self-test failed: rebuildable cache entered backup manifest" >&2
    exit 1
fi

mkdir -p "$ROOT/managed/config" "$ROOT/managed/data/managed-installs/test"
make_db "$ROOT/managed/data/library.sqlite3" "$CURRENT"
printf '{"emulator":"test","version":"1","schema":1,"binary":"/missing/emulator"}\n' > "$ROOT/managed/data/managed-installs/test/manifest.json"
run_case managed-manifest 1 --config-root "$ROOT/managed/config" --data-root "$ROOT/managed/data" \
    --legacy-config-root "$ROOT/managed/legacy-config" --legacy-data-root "$ROOT/managed/legacy-data"

mkdir -p "$ROOT/readonly/config" "$ROOT/readonly/data"
make_db "$ROOT/readonly/data/library.sqlite3" "$CURRENT"
find "$ROOT/readonly/config" "$ROOT/readonly/data" -type f -printf '%p|%s|%T@\n' | sort > "$ROOT/readonly-before.txt"
run_case readonly 0 --config-root "$ROOT/readonly/config" --data-root "$ROOT/readonly/data" \
    --legacy-config-root "$ROOT/readonly/legacy-config" --legacy-data-root "$ROOT/readonly/legacy-data" \
    --json "$ROOT/readonly-report.json"
find "$ROOT/readonly/config" "$ROOT/readonly/data" -type f -printf '%p|%s|%T@\n' | sort > "$ROOT/readonly-after.txt"
cmp -s "$ROOT/readonly-before.txt" "$ROOT/readonly-after.txt" || {
    echo "self-test failed: inspected roots were modified" >&2
    exit 1
}

# Organise folder-move records: open, unknown and unreadable ones block; a
# settled one is verified against the files and, read-only, the library.
folder_case() {
    local name="$1" state="$2" at="$3" row_at="$4" expected="$5" body="${6:-}"
    local base="$ROOT/fm-$name"
    mkdir -p "$base/config" "$base/data/rename-transactions-sealed-v1/folder-moves" "$base/lib/mixed" "$base/lib/ScummVM"
    make_db "$base/data/library.sqlite3" "$CURRENT"
    case "$at" in
        source) mkdir -p "$base/lib/mixed/Adventure" ;;
        destination) mkdir -p "$base/lib/ScummVM/Adventure" ;;
    esac
    python3 - "$base" "$state" "$row_at" "$body" <<'PY'
import json, sqlite3, sys
base, state, row_at, body = sys.argv[1:5]
old, new = f"{base}/lib/mixed/Adventure", f"{base}/lib/ScummVM/Adventure"
db = f"{base}/lib/catalogue.sqlite3"
c = sqlite3.connect(db)
c.execute("CREATE TABLE archives(id INTEGER PRIMARY KEY, absolute_path_cached BLOB)")
c.execute("INSERT INTO archives VALUES (7, ?)", ((old if row_at == "old" else new).encode(),))
c.commit(); c.close()
assoc = lambda p: {"folder": 1, "root": f"{base}/lib", "root_identity": None, "relative": [], "absolute": list(p.encode())}
record = {"version": 1, "id": "folder-move-1", "source": old, "destination": new, "root": f"{base}/lib",
          "platform": "ScummVM", "database": db, "layout_digest": "", "entries": 1, "total_bytes": 1,
          "rows": [{"archive": 7, "old": assoc(old), "new": assoc(new), "fingerprint": False}],
          "created_dirs": [], "state": state, "at_unix": 0}
path = f"{base}/data/rename-transactions-sealed-v1/folder-moves/folder-move-1.json"
with open(path, "w", encoding="utf-8") as f:
    f.write(body if body else json.dumps(record))
PY
    run_case "fm-$name" "$expected" --config-root "$base/config" --data-root "$base/data" \
        --legacy-config-root "$base/legacy-config" --legacy-data-root "$base/legacy-data"
}
folder_case committed-agrees Committed destination new 0
folder_case undone-agrees Undone source old 0
folder_case planned Planned destination new 2
folder_case undoing Undoing source new 2
grep -q "earlier releases cannot see it" "$ROOT/fm-undoing.out" || { echo "self-test failed: Undoing not explained" >&2; exit 1; }
folder_case needs-review NeedsReview destination new 2
folder_case unknown-state Relocating destination new 2
folder_case unreadable Committed destination new 2 '{ not json'
folder_case committed-files-disagree Committed source new 2
folder_case committed-library-disagrees Committed destination old 2
folder_case unverifiable Committed nowhere new 1
grep -q "Finish interrupted folder moves" "$ROOT/fm-planned.out" || { echo "self-test failed: no way to finish a move named" >&2; exit 1; }
# A record version this preflight does not know blocks even with a settled
# state (Python's True == 1, so a boolean is not version 1).
folder_case newer-version Committed destination new 2 '{"version": 2, "state": "Committed", "source": "/nowhere/a", "destination": "/nowhere/b", "rows": []}'
grep -q "newer EmuWiz" "$ROOT/fm-newer-version.out" || { echo "self-test failed: newer record version not explained" >&2; exit 1; }
folder_case boolean-version Committed destination new 2 '{"version": true, "state": "Committed", "source": "/nowhere/a", "destination": "/nowhere/b", "rows": []}'
folder_case list-state Committed destination new 2 '{"version": 1, "state": ["Committed"]}'

# Two records for the same folder (Organise, Undo, Organise again; or a
# rolled-back move retried): only the newest one is checked against today's
# files and library. Ids carry the start time (folder-move-<s>-<pid>-<ns>).
folder_history_case() {
    local name="$1" older_state="$2" older_id="$3" newer_state="$4" newer_id="$5" at="$6" row_at="$7" expected="$8"
    local base="$ROOT/fmh-$name"
    mkdir -p "$base/config" "$base/data/rename-transactions-sealed-v1/folder-moves" "$base/lib/mixed" "$base/lib/ScummVM"
    make_db "$base/data/library.sqlite3" "$CURRENT"
    case "$at" in
        source) mkdir -p "$base/lib/mixed/Adventure" ;;
        destination) mkdir -p "$base/lib/ScummVM/Adventure" ;;
    esac
    python3 - "$base" "$older_state" "$older_id" "$newer_state" "$newer_id" "$row_at" <<'PY'
import json, sqlite3, sys
base, older_state, older_id, newer_state, newer_id, row_at = sys.argv[1:7]
old, new = f"{base}/lib/mixed/Adventure", f"{base}/lib/ScummVM/Adventure"
db = f"{base}/lib/catalogue.sqlite3"
c = sqlite3.connect(db)
c.execute("CREATE TABLE archives(id INTEGER PRIMARY KEY, absolute_path_cached BLOB)")
c.execute("INSERT INTO archives VALUES (7, ?)", ((old if row_at == "old" else new).encode(),))
c.commit(); c.close()
assoc = lambda p: {"folder": 1, "root": f"{base}/lib", "root_identity": None, "relative": [], "absolute": list(p.encode())}
for record_id, state in ((older_id, older_state), (newer_id, newer_state)):
    record = {"version": 1, "id": record_id, "source": old, "destination": new, "root": f"{base}/lib",
              "platform": "ScummVM", "database": db, "layout_digest": "", "entries": 1, "total_bytes": 1,
              "rows": [{"archive": 7, "old": assoc(old), "new": assoc(new), "fingerprint": False}],
              "created_dirs": [], "state": state, "at_unix": 0}
    with open(f"{base}/data/rename-transactions-sealed-v1/folder-moves/{record_id}.json", "w", encoding="utf-8") as f:
        json.dump(record, f)
PY
    run_case "fmh-$name" "$expected" --config-root "$base/config" --data-root "$base/data" \
        --legacy-config-root "$base/legacy-config" --legacy-data-root "$base/legacy-data"
}
folder_history_case undone-then-committed Undone folder-move-100-1-5 Committed folder-move-200-1-5 destination new 0
folder_history_case rolledback-then-committed RolledBack folder-move-100-1-5 Committed folder-move-100-2-9 destination new 0
# The newest record is still checked, and an open newest record still blocks.
folder_history_case newest-disagrees Undone folder-move-100-1-5 Committed folder-move-200-1-5 source old 2
folder_history_case newest-open Undone folder-move-100-1-5 Planned folder-move-200-1-5 destination new 2
# Without a start time in the id nothing is treated as replaced: the Undone
# record is checked on its own and the folder contradicts it.
folder_history_case order-unknown Undone folder-move-old Committed folder-move-200-1-5 destination new 2

# General history cases: SPEC is JSON {"dirs": [...], "rows": {id: path},
# "records": [{"id", "state", "src", "dst", "rows": [[id, old, new]], "version"?}]}
# with paths relative to the case's library folder.
folder_records_case() {
    local name="$1" expected="$2" spec="$3"
    local base="$ROOT/fmr-$name"
    mkdir -p "$base/config" "$base/data/rename-transactions-sealed-v1/folder-moves" "$base/lib"
    make_db "$base/data/library.sqlite3" "$CURRENT"
    python3 - "$base" "$spec" <<'PY'
import json, os, sqlite3, sys
base, spec = sys.argv[1], json.loads(sys.argv[2])
lib = f"{base}/lib"
for d in spec["dirs"]:
    os.makedirs(f"{lib}/{d}")
db = f"{lib}/catalogue.sqlite3"
c = sqlite3.connect(db)
c.execute("CREATE TABLE archives(id INTEGER PRIMARY KEY, absolute_path_cached BLOB)")
for archive, p in spec["rows"].items():
    c.execute("INSERT INTO archives VALUES (?, ?)", (int(archive), f"{lib}/{p}".encode()))
c.commit(); c.close()
assoc = lambda p: {"folder": 1, "root": lib, "root_identity": None, "relative": [], "absolute": list(f"{lib}/{p}".encode())}
for r in spec["records"]:
    record = {"version": r.get("version", 1), "id": r["id"], "source": f"{lib}/{r['src']}", "destination": f"{lib}/{r['dst']}",
              "root": lib, "platform": "P", "database": r.get("database", db), "layout_digest": "", "entries": 1, "total_bytes": 1,
              "rows": [{"archive": a, "old": assoc(o), "new": assoc(n), "fingerprint": False} for a, o, n in r["rows"]],
              "created_dirs": [], "state": r["state"], "at_unix": 0}
    with open(f"{base}/data/rename-transactions-sealed-v1/folder-moves/{r['id']}.json", "w", encoding="utf-8") as f:
        json.dump(record, f)
PY
    run_case "fmr-$name" "$expected" --config-root "$base/config" --data-root "$base/data" \
        --legacy-config-root "$base/legacy-config" --legacy-data-root "$base/legacy-data"
}
# An older record of an unknown version is never treated as replaced.
folder_records_case older-wrong-version 2 '{"dirs": ["Z"], "rows": {"1": "Z/g"}, "records": [
  {"id": "folder-move-100-1-0", "version": 2, "state": "Committed", "src": "X", "dst": "Y", "rows": [[1, "X/g", "Y/g"]]},
  {"id": "folder-move-200-1-0", "state": "Committed", "src": "Y", "dst": "Z", "rows": [[1, "Y/g", "Z/g"]]}]}'
# A later move of a different folder that shares one path does not hide the
# older record's library row that it never moved.
folder_records_case other-folder-one-path 2 '{"dirs": ["X", "Y"], "rows": {"1": "X/a", "2": "X/b"}, "records": [
  {"id": "folder-move-100-1-0", "state": "Committed", "src": "X", "dst": "Y", "rows": [[1, "X/a", "Y/a"]]},
  {"id": "folder-move-200-1-0", "state": "Committed", "src": "W", "dst": "X", "rows": [[2, "W/b", "X/b"]]}]}'
# The same, when the later move did cover the row: the history is consistent.
folder_records_case other-folder-row-covered 0 '{"dirs": ["Z"], "rows": {"1": "Z/a"}, "records": [
  {"id": "folder-move-100-1-0", "state": "Undone", "src": "X", "dst": "Y", "rows": [[1, "X/a", "Y/a"]]},
  {"id": "folder-move-200-1-0", "state": "Committed", "src": "X", "dst": "Z", "rows": [[1, "X/a", "Z/a"]]}]}'
# A state that is not a string blocks (exit 2), never a tool error.
folder_records_case list-state-with-id 2 '{"dirs": ["Y"], "rows": {"1": "Y/a"}, "records": [
  {"id": "folder-move-100-1-0", "state": ["Committed"], "src": "X", "dst": "Y", "rows": [[1, "X/a", "Y/a"]]},
  {"id": "folder-move-200-1-0", "state": "Committed", "src": "X", "dst": "Y", "rows": [[1, "X/a", "Y/a"]]}]}'
folder_records_case dict-state-with-id 2 '{"dirs": ["Y"], "rows": {"1": "Y/a"}, "records": [
  {"id": "folder-move-100-1-0", "state": {"s": "Committed"}, "src": "X", "dst": "Y", "rows": [[1, "X/a", "Y/a"]]}]}'

# The older record's location check is waived only when the newest record
# positively finds its folder: here the folder was put back at X by hand.
folder_records_case chain-unverifiable 2 '{"dirs": ["X"], "rows": {"1": "Z/a"}, "records": [
  {"id": "folder-move-100-1-0", "state": "Committed", "src": "X", "dst": "Y", "rows": [[1, "X/a", "Y/a"]]},
  {"id": "folder-move-200-1-0", "state": "Committed", "src": "Y", "dst": "Z", "rows": [[1, "Y/a", "Z/a"]]}]}'
# A replaced record's malformed row still blocks (exit 2), never a tool error.
folder_records_case replaced-list-archive 2 '{"dirs": ["Z"], "rows": {"1": "Z/a"}, "records": [
  {"id": "folder-move-100-1-0", "state": "Undone", "src": "X", "dst": "Y", "rows": [[[1], "X/a", "Y/a"]]},
  {"id": "folder-move-200-1-0", "state": "Committed", "src": "X", "dst": "Z", "rows": [[1, "X/a", "Z/a"]]}]}'
grep -q "malformed library row" "$ROOT/fmr-replaced-list-archive.out" || { echo "self-test failed: malformed row in a replaced record not reported" >&2; exit 1; }
# A record whose library is not a path takes no part in replacing: the older
# Undone record is checked on its own (its folder is at Y) and blocks.
folder_records_case list-database 2 '{"dirs": ["Y"], "rows": {"1": "Y/a"}, "records": [
  {"id": "folder-move-100-1-0", "state": "Undone", "src": "X", "dst": "Y", "rows": [[1, "X/a", "Y/a"]]},
  {"id": "folder-move-200-1-0", "state": "Committed", "src": "X", "dst": "Y", "rows": [[1, "X/a", "Y/a"]], "database": ["x"]}]}'

echo "UPGRADE PREFLIGHT SELF-TEST: PASS"
