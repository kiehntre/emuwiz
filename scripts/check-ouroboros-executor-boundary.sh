#!/usr/bin/env bash
# Project Ouroboros: checks compiled artefacts for the test-only executor
# authority. Builds nothing; reads the files given.
#
#   scripts/check-ouroboros-executor-boundary.sh --absent  <binary|rlib>...
#   scripts/check-ouroboros-executor-boundary.sh --present <binary>...
#
# --absent fails if any file contains a way to construct the authority or
# to attach it to a context (the CLI and Blue binaries in every profile and
# feature set, their test binaries, core's integration-test binary, core's
# rlibs). --present is the positive control: core's own unit-test binary
# must contain them, which shows that the check can see them. The
# crate-private type itself and its read-only `admits` check exist in every
# build by design and are reported for information only.
#
# Binaries and rlibs are checked through the symbol tables of their object
# code (`nm -C`). A file without one fails closed. An rlib's metadata also
# holds the crate's doc comments, which name these items on purpose (the
# compile_fail doctests in transform/authority.rs), so a plain text match in
# an rlib is reported for information only and does not fail.
set -euo pipefail
FORBIDDEN='transform::authority::ScratchAuthority>?::(new|strict|provision_root|adopt_from_child)([^a-z_]|$)|transform::engine::Context(<[^>]*>)?>?::with_authority|transform::plan::plan_relocation_scratch'
NAMES='provision_root|adopt_from_child|plan_relocation_scratch|with_authority'
mode=${1:?usage: --absent|--present files...}
shift
status=0
for f in "$@"; do
  if [ ! -f "$f" ]; then echo "MISSING $f"; status=1; continue; fi
  case "$f" in
    *.rlib)
      n=$(grep -caE "$NAMES" "$f" || true)
      echo "rlib $(basename "$f"): metadata text mentions=$n (doc comments, informational)";;
  esac
  syms=$(nm -C --defined-only "$f" 2>/dev/null || true)
  if [ -z "$syms" ]; then echo "NO SYMBOL TABLE $f (cannot verify)"; status=1; continue; fi
  forbidden=$(grep -cE "$FORBIDDEN" <<<"$syms" || true)
  info=$(grep -cE 'ScratchAuthority>?::admits' <<<"$syms" || true)
  echo "$(basename "$f"): constructors/attach=$forbidden (admits=$info, informational)"
  if [ "$mode" = --absent ] && [ "$forbidden" != 0 ]; then
    grep -E "$FORBIDDEN" <<<"$syms" | head -5; status=1
  fi
  if [ "$mode" = --present ] && [ "$forbidden" = 0 ]; then status=1; fi
done
exit $status
