#!/usr/bin/env bash
# Classify paths for the CI fast tier. Anything outside the explicit Rust
# source areas, or matching a preservation/write boundary, gets full CI.
set -euo pipefail

if (($# == 0)); then
  echo "usage: $0 <changed-path>..." >&2
  exit 2
fi

saw_gui=false
saw_core=false
for path in "$@"; do
  lower=$(printf '%s' "$path" | tr '[:upper:]' '[:lower:]')

  case "$lower" in
    *filesystem*|*/fs/*|*/fs.rs|*atomic*|*rename*|*install*|*repair*|*undo*|*history*|*journal*|*transaction*|*write*|*apply*|*rom*|*patch*|*mount*|*preserv*|*rollback*|*delete*|*remove*|*restore*|*cue*|*chd*|*cleanup*|*stage*|*publish*|*convert*|*extract*|*replace*|*copy*|*move*|*truncate*|*symlink*)
      echo full
      exit 0
      ;;
  esac

  case "$path" in
    crates/archivefs-gui/src/*.rs|crates/archivefs-gui/src/**/*.rs)
      saw_gui=true
      ;;
    crates/archivefs-core/src/*.rs|crates/archivefs-core/src/**/*.rs)
      saw_core=true
      ;;
    *)
      echo full
      exit 0
      ;;
  esac
done

if [[ "$saw_gui" == true && "$saw_core" == true ]]; then
  echo full
elif [[ "$saw_core" == true ]]; then
  echo core
else
  echo gui
fi
