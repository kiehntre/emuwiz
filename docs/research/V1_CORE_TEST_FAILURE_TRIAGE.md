# EmuWiz V1 Core Test Failure Triage (I7)

## Scope and snapshot

This is a read-only triage of the current `archivefs-core` test suite. No
production code, catalogue, fixture, or concurrent work was changed.

Snapshot taken on 2026-09-14:

| Item | Value |
|---|---|
| Worktree | `/home/davedap/emuwiz-main-release-fix` |
| Branch | `main` |
| HEAD under test | `df59eb080bb81e974ca9ad6d66394a307a867fbf` |
| Build command | `cargo check -p archivefs-core` |
| Test command | `cargo test -p archivefs-core --no-fail-fast` |
| Test log | `/tmp/emuwiz-i7-core-test.log` |
| Result | `9145 passed; 47 failed; 2 ignored` |

The worktree was already dirty. Existing edits included archive workflow,
launch resource grants, library/scanner code, optical conversion, GUI files,
and an unrelated untracked research document. None were staged or modified by
this audit.

`cargo check -p archivefs-core` passed. It emitted existing warnings for an
unused `CanonicalIdentityStatus` import, an unused `Path` import, and dead
`app_dirs` helpers.

## Executive result

The raw failure count is **47**. The failures reduce to approximately **10
root-cause groups** when related stale expectations and environment leakage are
deduplicated. The captured evidence identifies **0 confirmed V1-blocking root
causes**. That is not a claim that every failing behavior is safe; it means no
failure was proven to be a production data-loss, security, parser-panic, or
supported-migration regression in this run.

The most urgent follow-up is to isolate the test environment from discovered
emulator installations. A large launch/discovery cluster sees real executables
under `/home/davedap/.local/bin` and then correctly refuses ambiguous binding,
but the fixtures expect a controlled synthetic environment. The highest-risk
individual test concern is the 60-second-plus `the_store_is_bounded` test; it
did not fail, but it is unsuitable for an unbounded full-suite timing path.

## Failure inventory

### Group A — arcade compatibility projection (3)

| Tests | Classification | Severity | Observed evidence |
|---|---|---|---|
| `arcade_compatibility::tests::absent_emulator_does_not_hide_other_result`<br>`arcade_compatibility::tests::one_observation_feeds_both_independent_engines`<br>`arcade_compatibility::tests::partial_observation_is_not_promoted_by_orchestration` | `REAL_REGRESSION` pending focused rerun | `V1_IMPORTANT` | Valid-looking MAME fixture expectations no longer produce the states asserted by the orchestration tests: `mame_compatible` is false, `BothIncompatible` replaces `MameOnly`, and partial evidence is not `Unknown`. |

This is a real compatibility-projection concern rather than a reason to weaken
MAME or FBNeo evidence rules. Follow-up should compare the fixture against the
current MAME engine contract and retain independent emulator results.

### Group B — BIOS projection fixture/contract (1)

| Test | Classification | Severity | Observed evidence |
|---|---|---|---|
| `bios_projection::tests::immutable_link_apply_and_rollback_are_source_preserving` | `STALE_TEST` | `V1_IMPORTANT` | The test now fails at planning with `StalePlan("MCPX has no concrete target path")`. This is a missing target in the test plan, not evidence that source-preserving rollback mutated a source. |

Smallest repair is to update the synthetic plan to provide the now-required
concrete target, then rerun the immutability assertions.

### Group C — database schema and migration expectations (4)

| Tests | Classification | Severity | Observed evidence |
|---|---|---|---|
| `database::tests::dat_expected_inventory::migrations_0011_and_0012_are_registered`<br>`database::tests::library_schema_contains_no_cheat_catalogue_journal_or_backup_tables`<br>`diagnostics::tests::stage_1a_introduces_no_database_migration`<br>`disk_format::tests::the_database_schema_and_migrations_are_unchanged` | `STALE_TEST` | `V1_IMPORTANT` | Current code registers migrations 0013–0017, including scan fingerprints and source roles. Tests still assert schema version 12 and migration list 0001–0012. One schema assertion also omits the now-present `scan_fingerprints` table. |

These are expected test updates for the current schema, not evidence that the
supported migrations fail. Migration compatibility must still be tested with
temporary databases in a separate focused run.

### Group D — platform alias/rescan behavior (5)

| Tests | Classification | Severity | Observed evidence |
|---|---|---|---|
| `database::tests::custom_alias_outranks_the_existing_filename_path_heuristic`<br>`database::tests::removing_alias_and_rescanning_restores_the_built_in_alias_fallback`<br>`database::tests::removing_alias_and_rescanning_restores_unknown_when_nothing_else_matches`<br>`database::tests::saved_source_assignment_reclassifies_unknown_rvz_on_rescan`<br>`database::tests::scan_while_manual_is_active_shadow_records_the_custom_alias_fallback` | `REAL_REGRESSION` | `V1_IMPORTANT` | Assertions show custom/built-in alias precedence and fallback restoration differ: `Xbox360` wins over expected `GameCube`, `GameCube` remains where `N64` or `Unknown` was expected, and an expected GameCube reclassification is absent. |

This group needs a focused semantic review. It is not safe to relabel as stale
without checking the current platform-assignment precedence contract.

### Group E — emulator inventory/update metadata (2)

| Tests | Classification | Severity | Observed evidence |
|---|---|---|---|
| `emulator_inventory::tests::version_parsing_fails_closed`<br>`emulator_update::tests::unknown_and_offline_fail_closed` | `REAL_REGRESSION` | `V1_IMPORTANT` | Version parsing returned `None` where a supplied `2509-1` was expected; offline/unknown comparison returned `VersionUnknown` instead of `ComparisonUnsupported`. |

These are metadata/fail-closed contract mismatches. They do not demonstrate
unsafe execution, but should be repaired or explicitly re-specified before v1
if those states feed launch diagnostics.

### Group F — native executable discovery contaminates fixtures (21)

| Tests | Classification | Severity | Observed evidence |
|---|---|---|---|
| `launch::hatari_execution::tests::preflight_rechecks_config_content_and_executable`<br>all 11 `launch::pcsx2_execution::tests::*` failures<br>all 4 `launch::rpcs3_execution::tests::*` failures<br>all 3 `launch::xemu_execution::tests::*` failures<br>`diagnostics::profiles::tests::rpcs3_readiness_is_only_assessed_for_eligible_profiles`<br>`diagnostics::profiles::tests::xemu_readiness_is_only_assessed_for_eligible_profiles` | `ENVIRONMENT_DEPENDENT` | `NOT_ACTIONABLE_NOW` (test reliability is `V1_IMPORTANT`) | Preflight reports `AmbiguousExecutable` because two viable executables are discovered. The failures include expected command/spawn tests and blocker-priority tests, which never reach their intended assertion. |

The test environment discovers host installations, including
`/home/davedap/.local/bin/xemu`. This is not a product regression by itself.
The smallest repair is fixture isolation or explicit executable injection in
the test harness; do not loosen ambiguity refusal.

### Group G — local emulator discovery tests see host state (6)

| Tests | Classification | Severity | Observed evidence |
|---|---|---|---|
| `patch_manager::dolphin_local::tests::modern_local_discovery_covers_native_flatpak_portable_and_supplied_version`<br>`patch_manager::hatari_local::tests::missing_executable_and_missing_explicit_config_are_neutral`<br>`patch_manager::hatari_local::tests::portable_and_custom_executable_version_are_safe_metadata`<br>`patch_manager::rpcs3_local::tests::an_eligible_profile_is_detected_even_without_a_discovered_executable`<br>`patch_manager::xemu_local::tests::explicit_portable_executable_and_version_are_preserved_without_execution`<br>`patch_manager::xemu_local::tests::native_launch_binding_refuses_a_missing_executable` | `ENVIRONMENT_DEPENDENT` | `NOT_ACTIONABLE_NOW` (harness reliability is `V1_IMPORTANT`) | Tests expected empty discovery or supplied metadata, but observed host discovery; version fields were absent in some fixture paths and a real xemu executable was found where absence was expected. |

This is the same root cause family as Group F and should be fixed through
controlled discovery roots, not production weakening.

### Group H — readiness projection expectations (2)

| Tests | Classification | Severity | Observed evidence |
|---|---|---|---|
| `ready_to_play::tests::ready_and_warning_states_are_pure_projections`<br>`ready_to_play::tests::unknown_does_not_become_missing_or_mask_a_blocker` | `REAL_REGRESSION` pending contract review | `V1_IMPORTANT` | The projection returned `Unknown` instead of `ReadyWithWarnings`, and `NeedsAttention` instead of `Blocked`. |

These assertions concern top-level user-facing readiness semantics and deserve
focused review. No new evidence or readiness state should be invented as a
triage response.

### Group I — launch resource grant order (1)

| Test | Classification | Severity | Observed evidence |
|---|---|---|---|
| `launch::resource_grants::tests::representative_retroarch_mame_xemu_and_dolphin_grants_validate` | `CONCURRENT_WORK` | `NOT_ACTIONABLE_NOW` | The expected first presented path is `/run/bios.bin`, while the observed path is `/run/card.mcr`. `crates/archivefs-core/src/launch/resource_grants.rs` was concurrently dirty during the audit. |

Do not repair or stage this file as part of I7. Re-run after the owner’s work
settles; this may be only an ordering expectation.

### Group J — source overlap/order expectations (2)

| Tests | Classification | Severity | Observed evidence |
|---|---|---|---|
| `tests::add_source_folder_rejects_an_overlap_through_the_orchestration_layer`<br>`tests::scanner_rejects_duplicate_and_shadows_nested_roots_but_not_prefix_siblings` | `STALE_TEST` | `V1_IMPORTANT` | Nested source addition returned `Ok` although the test expects rejection; scanner output was deterministic but child-first where the test expects parent-first. Current source-role/nested-source work explicitly supports independently configured child sources and shadowing. |

The smallest repair is to update expectations to the current nested-source
policy and make ordering intent explicit. Do not remove nested-source safety or
accept duplicate discovery.

## Long-running test

`identity_source::verification::tests::the_store_is_bounded` passed in the full
run, but emitted the harness warning that it had been running for over 60
seconds. A separate isolated run was allowed 45 seconds and reached the test
body without completing before timeout. Classification: **NONDETERMINISTIC**
for suite timing / resource sensitivity, severity **V1_IMPORTANT** for CI
reliability and **not a product failure** on current evidence.

The test allocates a deliberately large bounded workload. It should have an
explicit performance budget or a dedicated lane rather than silently delaying
the v1 baseline.

## Failure counts

| Measure | Count | Qualification |
|---|---:|---|
| Raw failing tests | 47 | Exact aggregate from the completed core lib test run |
| Unique root-cause groups | 10 | Groups F and G are one host-discovery family; the bounded-store timeout is a separate reliability observation, not a failed test |
| Confirmed V1-blocking root causes | 0 | No confirmed production panic, data-loss, security, or migration failure in this run |
| Stale-test failures | 7 | Schema/migration expectations (4) plus source overlap/order (2) plus BIOS target fixture (1) |
| Environment-dependent failures | 27 | Native launch/discovery groups F/G; count includes their 21 + 6 tests |
| Concurrency-related failures | 1 | Resource-grant ordering test with concurrently modified file |

The remaining 12 are the arcade projection (3), alias/rescan (5), inventory /
update (2), and readiness (2) groups; classification confidence for those
groups is recorded above.

## Panic, data-loss, security, and migration review

* The only explicit panics in the failure report are assertion panics and
  `unwrap` failures in tests. No captured failure is a parser panic on
  user-controlled ROM, archive, DAT, mod, or memory-card bytes.
* Archive/path-safety and transaction tests appearing before the failure report
  passed. No traversal, symlink escape, overwrite, rollback, source mutation,
  or arbitrary-command failure was observed.
* The BIOS test stopped before its apply/rollback assertions because of a stale
  plan target. It is not evidence of data loss, but apply/rollback remains
  important until the fixture is corrected.
* Migration tests fail because their expected version is 12 while current code
  intentionally registers migrations through 0017. No temporary-database
  migration failure was observed; a dedicated migration compatibility run is
  still required before calling this area fully green.

## Recent-feature revalidation

| Area | Status | Evidence in current run |
|---|---|---|
| MAME compatibility | `FAIL` | Three arcade orchestration tests fail; per-set engine semantics need focused review |
| FBNeo compatibility | `FAIL` | Shared arcade orchestration tests fail; no evidence that FBNeo’s independent parser is broken |
| Source Roles SR2 | `FAIL` | Nested-source expectation is stale against current child-source behavior; routing-specific failure not proven |
| Source Roles SR3 | `PASS` | Source-role subsystem tests in the captured run passed; no dedicated failure listed |
| Library visibility SR4 core | `PASS` | No library-visibility failure listed; visibility tests completed in the run |
| MOD0 | `PASS` | `mod_package` inspection, safety, ordering, and no-write tests passed |
| FI0 | `FAIL` | Representative grant test failed while `resource_grants.rs` was concurrently modified |
| FI1 | `PASS` | RetroArch resource projection tests completed; no FI1 failure listed |
| PS2 memory-card inventory | `PASS` | PS1/PS2 inventory, truncation, geometry, loop, timestamp, and no-mutation tests passed |

These statuses mean “current captured tests,” not a claim that every feature has
an exhaustive independent suite.

## Prioritized repair queue

1. **P0 — none evidenced.** Do not declare a v1 blocker from this run without
   reproducing a production panic, unsafe file operation, data-loss path, or
   broken supported migration in an isolated test.
2. **P1 — isolate emulator discovery tests.** Control HOME, PATH, Flatpak and
   configured search roots so synthetic tests cannot see host executables. Keep
   ambiguous executable refusal intact.
3. **P1 — review arcade and readiness projections.** Re-run the 3 arcade and 2
   readiness tests individually, then decide whether current expectations or
   production semantics are wrong.
4. **P1 — repair platform alias/rescan contract.** The five failures have
   contradictory observed outcomes and may represent a real identity regression.
5. **P1 — correct schema-era test expectations and BIOS target fixture.** Update
   only the smallest stale assertions after confirming migration intent.
6. **P2 — settle the concurrent resource-grant test.** Re-run after the dirty
   file is owned by one worker; do not merge an ordering change blindly.
7. **P2 — give the bounded-store test an explicit budget or integration lane.**
   Preserve the memory bound while preventing an opaque full-suite stall.
8. **P2 — review inventory/update metadata contracts.** These are small,
   deterministic failures but not currently security or launch-safety blockers.

## Final assessment

The current suite is not green, but the red count materially overstates the
number of independent v1 risks. The strongest immediate evidence is test
isolation failure, followed by likely stale schema/source expectations. The
arcade, alias, inventory/update, and readiness groups need focused owner-led
reruns before they can be safely downgraded. No broad repair campaign is
authorized by this audit.
