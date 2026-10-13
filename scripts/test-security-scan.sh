#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=scripts/release-common.sh
source "$SCRIPT_DIR/release-common.sh"

# Regression coverage for scripts/security-scan.sh credential-URL classification.
# Each case runs a copy of the scanner inside a disposable git repository.
# Fixture "secrets" are assembled at runtime so this file itself stays clean.

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/emuwiz-security-scan-tests.XXXXXXXX")"
trap 'rm -rf -- "$TEMP_ROOT"' EXIT INT TERM

run_case() {
    local label=$1 want=$2 content=$3
    local repo="$TEMP_ROOT/$label"
    mkdir -p -- "$repo/scripts"
    cp -- "$SCRIPT_DIR/security-scan.sh" "$SCRIPT_DIR/release-common.sh" "$repo/scripts/"
    printf '%s\n' "$content" >"$repo/sample.rs"
    git -C "$repo" init -q
    git -C "$repo" add -A
    local status=0
    "$repo/scripts/security-scan.sh" >"$repo/out" 2>&1 || status=$?
    if [[ "$want" == clean && $status -ne 0 ]] || [[ "$want" == flagged && $status -eq 0 ]]; then
        cat "$repo/out" >&2
        release_die "security-scan case $label: expected $want (exit $status)"
    fi
    release_note "security-scan case ok: $label ($want)"
}

at="@"
# Synthetic placeholders used by redaction/refusal tests are not credentials.
run_case placeholder-password clean "let u = \"https://user:password${at}example.test/a\";"
run_case placeholder-short clean "let u = \"http://user:pw${at}romm.local:80/x\";"
run_case placeholder-pass clean "let u = \"https://user:pass${at}www.moddb.com/m\";"
run_case placeholder-secret-upper clean "let u = \"https://user:SECRET${at}romm.example\";"
run_case placeholder-format-named clean "format!(\"https://user:{TOKEN}${at}example.test/a\")"
run_case placeholder-format-empty clean "format!(\"http://{}:{}${at}127.0.0.1:1\")"
run_case placeholder-fixture clean "\"http://fixture-user:fixture-password${at}localhost/p\""

# Real-looking credentials must still be reported, however they are hosted.
run_case real-password flagged "let u = \"https://deploy:Zq7vN2xK9mWp4Lr8${at}example.test/a\";"
run_case real-on-real-host flagged "curl https://alice:hunter2xyz${at}registry.example.org/x"
run_case placeholder-prefix-not-exempt flagged "https://user:password123${at}example.test/"
run_case template-suffix-not-exempt flagged "https://user:{TOKEN}x${at}example.test/"
# A placeholder earlier in the same file must not hide a real credential later.
run_case real-after-placeholder flagged "$(printf '%s\n%s' \
    "let a = \"https://user:password${at}example.test/\";" \
    "let b = \"https://svc:Xk39dLq0Vb7Rm2Tw${at}internal.example/\";")"
# Other detectors are unaffected.
gh="ghp_"; run_case github-token flagged "token = ${gh}abcdefghijklmnopqrstuvwxyz0123456789"
dashes="-----"; run_case private-key flagged "${dashes}BEGIN RSA PRIVATE KEY${dashes}"
name="GITHUB_""TOKEN"; run_case credential-assignment flagged "${name}=abc123def456"

release_note "security-scan classification tests passed"
