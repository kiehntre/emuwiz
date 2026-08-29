# EmuWiz v0.8.1-alpha ("Alpha 2.1") release checklist

Concise checklist for this release only. Do not tag `v0.8.1-alpha` until
every box below is checked against the exact final commit. This does not
replace [`docs/release-checklist.md`](release-checklist.md),
[`docs/release-checklist-alpha-0.8.md`](release-checklist-alpha-0.8.md), or
[`docs/release-checklist-alpha-1.1.md`](release-checklist-alpha-1.1.md),
which record other, already-shipped releases.

## Source and version

- [x] Workspace version is `0.8.1-alpha` for `archivefs-core`,
      `archivefs-cli`, and `archivefs-gui` (`Cargo.toml`
      `[workspace.package].version`) - confirmed via
      `scripts/check-version-consistency.sh` at the release-prep commit.
- [x] `Cargo.lock` reflects `0.8.1-alpha` for all three workspace members.
- [x] CLI and GUI `--version` resolve from Cargo metadata, not a hardcoded
      string - unchanged code-structure fact.
- [x] `CHANGELOG.md` has a `## v0.8.1-alpha (unreleased)` heading with the
      derived user-facing highlights. It drops `(unreleased)` and gains the
      tag date in the finalize commit, not here.
- [x] `docs/releases/v0.8.1-alpha.md` exists as a draft with a
      "not yet tagged or published" status note (removed in the finalize
      commit).
- [x] `README.md` describes `main` as the `v0.8.1-alpha` release candidate
      while keeping `v0.8.0-alpha` as the current *published* release. The
      README download example is repointed to `v0.8.1-alpha` only after
      tagging, in a separate follow-up commit (same policy as 0.8.0).
- [x] No schema/migration change shipped in this release (confirmed against
      the diff: identity/optical/launch additions are read-only or reuse
      the existing journal engine; no new SQLite migration or column).
- [x] No ROM, disc image, BSFree database, secret, or build output is
      tracked or staged - `scripts/security-scan.sh` reports 806 tracked
      files, no credential-shaped secrets.

## Automated gates (release-prep commit)

- [x] `git diff --check` - PASS
- [x] `cargo fmt --all -- --check` - PASS
- [x] `cargo check --workspace --all-targets --all-features` - PASS
      (see "Fixed during release prep" below - the workspace did **not**
      compile before this pass).
- [x] `cargo test --workspace` - PASS after the fixes below, with one
      known parallelism flake (see "Known test issues").
- [x] `scripts/check-version-consistency.sh` (doc/version checks) - PASS
- [x] `scripts/test-check-version-consistency.sh` - PASS
- [x] `scripts/security-scan.sh` - PASS (806 tracked files, no secrets)

### Known-failing gate - must be resolved before tagging

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` - **FAIL.**
      As of the release-prep commit this reports ~115 lint findings
      promoted to errors (about 63 in `archivefs-core` lib, the rest in lib
      tests and the GUI crate), accumulated across the 0.8.1 development
      range. None is a miscompile - `cargo check` and `cargo test` pass -
      but CI's `clippy` job is red. This must be cleared in a dedicated
      lint-cleanup pass (no feature or semantic change) before the tag is
      cut. The GUI crate also carries ~10 pre-existing `cargo check`
      warnings (unused imports, dead code).

### Known test issues

- `launch::rpcs3_execution` / `launch::xemu_execution` (and the other
  `launch::*_execution`) tests write a fake executable script into a temp
  fixture and immediately `spawn` it. Under the full `cargo test` suite at
  high thread counts (observed on a 24-core host) one of them
  intermittently fails with `ETXTBSY` ("Text file busy") because a sibling
  thread still holds the just-written file open. It is a test-harness race,
  not a product defect: every one passes in isolation, and
  `cargo test -p archivefs-core --lib -- --test-threads=4` is fully green
  (7056 passed). CI's 2-core runner has not been observed to hit it.
  Fixing the fixture helper (retry-on-`ETXTBSY`, or serialising these
  tests) is a reasonable follow-up but is out of scope for release prep.

## Fixed during release prep

The pre-prep tip (`2e187cf`) shipped a red `cargo test --workspace` for
three independent, non-product reasons. All three fixes are call-site /
test-assertion catch-ups; **no** DAT, identity, launch, transaction, RomM,
cheats, or mods behaviour changed.

1. `crates/archivefs-cli` did not compile - three call sites had drifted
   behind `archivefs-core` struct changes:
   - `DoctorScanInputs` gained `xemu_readiness` / `xenia_readiness` /
     `ppsspp_readiness` / `rpcs3_readiness`; the CLI Doctor literal now
     passes `Gathered::NotLoaded` for all four (the CLI Doctor deliberately
     walks no discovery directories, the same policy already applied to
     RetroArch discovery in that same function).
   - `RefreshRequest` gained `import_timeout`; the CLI full-import path now
     passes `settings.effective_import_timeout()` (mirroring the
     `settings.effective_page_size()` already used two lines above).
   - a `#[cfg(test)]` `CoreInfoFinding::Found` fixture in
     `crates/archivefs-cli/src/main.rs` was missing `core_name` /
     `manufacturer` / `categories` / `database` / `firmware` and now
     supplies neutral values.
2. `crates/archivefs-cli/src/romm_identity/tests.rs`
   `a_failed_adaptive_import_preserves_the_previous_cache_byte_for_byte`
   asserted `page_size_reductions == 5`. Commit `4c49d5b` intentionally
   changed the adaptive-paging ladder so a third consecutive oversized
   response at one offset jumps straight to a single-record request
   (`100 -> 50 -> 25 -> 1`); the stale assertion is now `== 3`. Behaviour
   was already shipped and is covered by the core's own tests.
3. `crates/archivefs-gui/src/tests/doctor_and_repair.rs`
   `selecting_exact_duplicates_reaches_the_page` asserted the page renders
   the string `"Find exact copies"`, a label that shipped code reworded
   (the page now shows `"Duplicate review"` / `"Source folder:"`). The
   assertion now checks `"Source folder:"`, the same page-own text the
   sibling `the_page_starts_in_its_safe_empty_state` test already relies
   on. Navigation itself was never broken.

## Artifact-dependent gates - PENDING

Run against the exact final commit before tagging:

- [ ] `scripts/build-release.sh` (canonical release artifact build)
- [ ] `scripts/check-version-consistency.sh --binary-dir target/release
      --artifact ... --checksum ...`
- [ ] `scripts/verify-release-artifact.sh`
- [ ] `scripts/compare-release-builds.sh` (byte-for-byte reproducibility)
- [ ] `scripts/test-release-artifact-verifier.sh` (rejects malformed
      artifacts)
- [ ] `bash tests/test_install.sh`
- [ ] `cargo audit`
- [ ] Built `emuwiz-cli --version` and `emuwiz --version` both report
      `0.8.1-alpha`.

## Manual smoke gates

Execute against the exact commit being released, with disposable fixtures
(never irreplaceable ROMs), recorded with tester, date, commit SHA, and
outcome.

- [ ] **A. Verified identity for a new platform.** Inspect a disposable
      3DO ISO, a PC-FX ISO/MODE1 CUE, and a ScummVM folder; confirm each
      resolves a verified structured identity, and that a truncated or
      wrong-format fixture fails closed (no guess). Confirm a 3DO/PC-FX
      `.chd` is refused rather than mis-identified.
- [ ] **B. Verified CUE/BIN -> CHD conversion + rollback.** Convert a
      disposable CUE/BIN; confirm it only finalizes when the canonical
      optical fingerprint matches, the original is never deleted, and
      rollback restores the pre-conversion state. Repeat with the
      quarantine-source option and confirm it is reversible.
- [ ] **C. Whole-collection RomM planning.** Plan across two disposable
      platform libraries; confirm the combined plan is deterministic, that
      missing/unsafe/occupied/duplicate destinations are reported and never
      auto-resolved, and that applying two per-platform transactions that
      collide on one destination refuses the second without overwriting the
      first or any original.
- [ ] **D. Emulator launch/readiness.** For at least one of ScummVM
      (native launch) and PPSSPP / RPCS3 / xemu / Xenia (readiness), confirm
      the readiness view reports honestly and an unverified input fails
      closed rather than launching.
- [ ] **E. DAT completion, revision history, and local rollback.** Confirm
      the DAT Sources page shows collection completion, a managed source's
      revision history, and that rolling back to a previous local revision
      restores that revision without a network fetch.

## Publication gate

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes on the
      exact commit to be released.
- [ ] All other automated and artifact-dependent gates pass on that commit.
- [ ] All five manual smoke journeys (A-E) executed and signed off against
      that commit.
- [ ] Explicit authorization received to merge, tag, and publish
      `v0.8.1-alpha`.
- [ ] Annotated tag is exactly `v0.8.1-alpha` and points at the final main
      release commit.
- [ ] Published assets are exactly the verified
      `archivefs-v0.8.1-alpha-x86_64-linux.tar.gz` archive and its
      checksum, built against the final commit.

Do not create the tag until every box above is checked against the exact
final commit.
