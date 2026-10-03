#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd -P)"
OUTPUT_DIR="$REPO_ROOT/target/release-artifacts"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

usage() {
    cat <<'EOF'
Usage: scripts/build-release.sh [--output-dir DIR] [--target-dir DIR]

Build the canonical EmuWiz Linux release:
  emuwiz-<version>-linux-<arch>.tar.xz

The extracted archive supports both ./bin/emuwiz direct-run and ./install.sh
for a per-user installation. No system-wide installer is added here.
EOF
}

while (($#)); do
    case "$1" in
        --output-dir) (($# >= 2)) || { echo "--output-dir requires a directory" >&2; exit 1; }; OUTPUT_DIR=$2; shift 2 ;;
        --target-dir) (($# >= 2)) || { echo "--target-dir requires a directory" >&2; exit 1; }; TARGET_DIR=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

git -C "$REPO_ROOT" diff --quiet
git -C "$REPO_ROOT" diff --cached --quiet

if [[ "$OUTPUT_DIR" != /* ]]; then OUTPUT_DIR="$PWD/$OUTPUT_DIR"; fi
if [[ "$TARGET_DIR" != /* ]]; then TARGET_DIR="$PWD/$TARGET_DIR"; fi
mkdir -p "$OUTPUT_DIR" "$TARGET_DIR"
TARGET_DIR="$(CDPATH= cd -- "$TARGET_DIR" && pwd -P)"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$REPO_ROOT" log -1 --format=%ct)}"
export LC_ALL=C TZ=UTC

# Rust panic locations and include!-generated bindings retain absolute paths,
# even in release builds. Preserve caller flags; encoded flags also support
# build roots containing spaces. More-specific mappings must come last.
if [[ ! -v CARGO_ENCODED_RUSTFLAGS ]]; then
    read -r -a rust_flags <<< "${RUSTFLAGS:-}"
    printf -v CARGO_ENCODED_RUSTFLAGS '%s\x1f' "${rust_flags[@]}"
    CARGO_ENCODED_RUSTFLAGS=${CARGO_ENCODED_RUSTFLAGS%$'\x1f'}
fi
for mapping in \
    "${HOME:?}=/build/home" \
    "${CARGO_HOME:-$HOME/.cargo}=/build/cargo" \
    "${RUSTUP_HOME:-$HOME/.rustup}=/build/rustup" \
    "$REPO_ROOT=/build/source" \
    "$TARGET_DIR=/build/target"; do
    CARGO_ENCODED_RUSTFLAGS+="${CARGO_ENCODED_RUSTFLAGS:+$'\x1f'}--remap-path-prefix=$mapping"
done
export CARGO_ENCODED_RUSTFLAGS

(
    cd "$REPO_ROOT"
    CARGO_TARGET_DIR="$TARGET_DIR" cargo build --workspace --release --locked
)

SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$REPO_ROOT" log -1 --format=%ct)}" \
    "$SCRIPT_DIR/release/package-release.sh" \
      --target-dir "$TARGET_DIR" \
      --source-root "$REPO_ROOT" \
      --output-root "$OUTPUT_DIR" \
      --archive --reproducible --require-clean
