# EmuWiz final worktree reconciliation — 2026-10-26

## Disposition and approval state

PR [#164](https://github.com/kiehntre/emuwiz/pull/164) was merged normally as `591aff8810340b828a7b9a568bce5498634df95e` after all required CI passed for approved head `a6fe07a647f3d1a2d02b729734b04382a8f6eb58`. The merge was verified on `origin/main`. The reviewed command was installed in a separate versioned root-owned location; the running cleaner installation, timer and policy were left unchanged.

Post-merge legacy reconciliation removed **51 checkouts** in bounded batches, with **13 blocked decisions**. The registry measured **131 before → 80 after**. Removed checkout content occupied **7,157,305,344 allocated bytes**. Concurrent filesystem/worktree activity is reported separately from removals attributable to this task. Historical Cargo cache retirement remains **UNKNOWN** for every legacy disposal; no retirement receipt was manufactured and no external cache was manually deleted.

The previously reported 69 missing-target candidates describe the batch-5 checkpoint. Eligibility was decided individually using the approved independent procedure and the existing disposition records; no broad archaeology was repeated. Present-cache, dirty/artifact, active/pinned, production-deployment and other unverified cases remain protected.

## Completed cleanup batches

The pre-lean bounded cleanup removed 20 checkouts (**213 → 193**), measuring **2,856,280,064 allocated bytes** removed and **2,786,131,968 bytes** of immediate observed free-space increase. Five more worktrees existed when lean recovery started at 198. Two further worktrees were added concurrently before batch 3.

| Batch | Registry before → after | Removed | Allocated checkout bytes removed | Observed free-space increase |
| --- | ---: | ---: | ---: | ---: |
| batch1 | 198 → 183 | 15 | 2,157,002,752 | 2,037,764,096 |
| batch2 | 183 → 170 | 13 | 1,875,730,432 | 1,878,134,784 |
| batch3 | 172 → 165 | 7 | 945,618,944 | 946,434,048 |
| batch4-A | 165 → 158 | 7 | 881,467,392 | 979,968,000 |
| batch4-A2 | 158 → 155 | 3 | 388,210,688 | 389,636,096 |
| batch4-B | 155 → 148 | 7 | 828,354,560 | 829,186,048 |
| batch5-A | 148 → 144 | 4 | 1,164,759,040 | 1,164,718,080 |
| batch5-B | 144 → 143 | 1 | 129,257,472 | 129,474,560 |

Lean batches 1–5 removed **57 checkouts**, measuring **8,370,401,280 allocated bytes**. For those earlier records, including the 20-path batch, the subtotal is **77 removed checkouts** and **11,226,681,344 allocated bytes**. The allocation measurements describe removed checkout content. Observed filesystem free-space movement includes concurrent activity and is not an isolated attributable measurement; those deltas must not be summed into a claimed exact reclaim total.

All recorded removals used `git worktree remove` without `--force`. Git refs were retained; detached unique histories required verified bundles. Later batches also preserved approved dirty source/evidence before disposing of completed checkouts. External Cargo cache retirement by the existing maintenance trial is separate from these checkout measurements and is not credited to this PR.

## Preserved development and evidence

The original lean inventory identified 23 valuable worktrees grouped into 22 unfinished recovery candidate HEAD groups. The preservation ledger records 87 histories/local-change sets, including four verified detached-history bundles and 30 verified bounded source/evidence archives from the initial preservation pass. Later batch records identify additional verified bundles and archives. None of those private files is uploaded by this PR.

Promising preserved work includes DAT-authoritative canonical rename with transactional Apply/Undo, Dolphin Flatpak launch, WHDLoad saved-trainer launch wiring, coordinated ES-DE/RetroDECK publication recovery, MAME selected-catalogue repair authority, and the uncommitted PCSX2 readiness and DAT identity-panel work. Existing source-review findings were reused; they are not new discovery or runtime-validation claims. Subsequent focused feature recovery is separate from cleanup authorization.

Job #22, Job #41, the Sunshine visual-test build and its protected checkout/target, active development jobs and valuable unfinished groups remain excluded from disposal. A clean or merged HEAD alone does not authorize removal.

## R2/R3 production findings

Existing source-review evidence identifies the R2 initial-user-namespace ownership registry, fail-closed legacy Apply/Undo boundary, privileged broker/capture/release integration, and R3 rename durability/recovery work as production-sensitive preserved lineages. Deployment and ownership conditions are not resolved by this cleanup change. Production recovery implementation is unchanged.

The legacy command refuses unresolved R2/R3 identities found in branch, checkout or lifecycle task metadata as `PRODUCTION_DEPLOYMENT_UNRESOLVED`, with `deployment_conditions: UNKNOWN`. Pins, live recorded jobs, lifecycle locks and host process references are checked independently. An old checkout HOLD note is not reported as proof of a currently live owner; a Git worktree lock still remains an explicit disposal prohibition. No production-deployment override is introduced.

## Missing Cargo targets: separate checkout reconciliation

The old `worktree-check` gate continues to require verified Cargo retirement proof. Its `ARTIFACTS_MISSING_UNVERIFIED` outcome is unchanged. The new explicit `worktree-reconcile` path permits independently safe Git checkout disposal only when `lstat` proves the target is genuinely absent. It never synthesizes a receipt, rewrites `last_retired`, or asserts that the historical cache deletion was safe.

Any present target object—including a reused directory, regular file or dangling symlink—must use normal trusted Cargo retirement. Computed and recorded targets/keys must agree; shared target ownership, another target owned by the checkout, corrupt records, unknown pins, live jobs and incomplete process visibility refuse.

Checkout eligibility requires an unlocked registered secondary worktree, no unfinished Git operation, no tracked dirt, untracked files or ignored artifacts, and either HEAD contained in `origin/main` or an explicitly supplied retained local ref at exact HEAD. Supplying a preservation ref is archival disposition, not a claim that code is integrated. Valuable features remain outside cleanup until a separate recovery decision.

With `--remove`, the command holds the exclusive target lifecycle and registry metadata locks, records an intent, repeats checks and invokes normal Git worktree removal. Its outcome is `CHECKOUT_REMOVED` with `historical_cache_retirement: UNKNOWN`. Dry-run results and old intents never become reusable authorization. Existing Cargo deletion, pin, release, approval and production recovery behavior is not weakened.

The final merge gate also identified an inode-alias process-reference defect in the initially approved head `3446c8df`: a live external hard link could escape pathname-only checks. That head's independent PASS was withdrawn after reproduction. The correction reuses the existing checkout identity measurement and `Census.match_inodes` primitives in both authorization passes, refuses incomplete/cross-device scans, and adds real-process hard-link and incomplete-scan regressions. The corrected exact head received independent PASS, renewed expected-head approval and green required CI before any legacy disposal.

## Validation and remaining authorization

Focused deterministic regressions exercise missing targets; recreated directories/files/symlinks; target reuse after intent; active and uninspectable processes; dirty, untracked and ignored evidence; pins and build locks; shared/alternate ownership; malformed registry records; exact-ref preservation; changed source after intent; R2/R3 conditions; and preservation of external evidence and Git refs during normal removal. Synthetic fixtures are private temporary repositories and registries, with no Rust builds or production cleanup.

Independent safety review reproduced two preservation defects in the initial candidate using synthetic files: ignored evidence arriving during the final census and tracked edits hidden by assume-unchanged/skip-worktree flags. The candidate now rechecks checkout artifacts after the final census and refuses those index flags; deterministic regressions cover both. The corrected exact head subsequently received independent PASS and explicit owner approval. Required CI passed before the guarded merge. The original 96-test evidence, corrected 19-test regressions and independent review records were retained locally. No new Rust builds, broad archaeology or unchanged-HOLD removal batches were run by this cleanup.

## Verified removal register

Only checkout basenames and Git identifiers are published below. Private archive contents, session logs, production data, credentials and local application evidence are excluded. The full local receipts retain per-path process, lifecycle, preservation and allocation checks.

### batch1

- `archivefs-cc-blue-dosbox-play` — `5e50b975ea78`.
- `archivefs-cc-blue-scummvm-play` — `2acf8878a289`.
- `archivefs-cc-classic-poke-association` — `c5244e775707`.
- `emuwiz-blue-classic-pinning-20261025` — `33201aea7d8a`.
- `emuwiz-blue-classic-poke-20261024` — `79251e8c5236`.
- `emuwiz-blue-launch-readiness-20261025` — `6f06adeb7d71`.
- `emuwiz-blue-xroar-tosec-20261024` — `06c1df971e38`.
- `emuwiz-ci-perf-round2-20261025` — `5711910f029c`.
- `emuwiz-dosbox-ci-20261024` — `f8162b7d26b1`.
- `emuwiz-gold-blue-dat-repair-diagnostics` — `3f406beaba74`.
- `emuwiz-job10-dat-identity` — `bc9bd031aee2`.
- `emuwiz-job11-classic-poke` — `f09d3f04c82d`.
- `emuwiz-job16-blue-dat-navigation` — `a5223f2b7c0b`.
- `emuwiz-job17-dat-tmpdir` — `8d382ea3875d`.
- `emuwiz-job21-xroar-play` — `5489b40a3986`.

### batch2

- `emuwiz-codeql-triage-20261026` — `e66ec898d221`.
- `emuwiz-job24-c64-vice-20261025` — `cb9492cc692c`.
- `emuwiz-job24-vice-trainers-20261025` — `2296e9a6e6bc`.
- `emuwiz-job26-manuals-gui` — `b2bb4dc0c597`.
- `emuwiz-job33-saturn` — `b1a84af467ba`.
- `emuwiz-job34-gamecube` — `5940c7286153`.
- `emuwiz-job35-dosbox` — `f6b941feebe1`.
- `emuwiz-job8-ci-flakes` — `3c438b0cf310`.
- `emuwiz-ps2-codebreaker-20261025` — `a780a7852fda`.
- `emuwiz-retroarch-selection-20261024` — `7c5323ec7e40`.
- `emuwiz-scummvm-history-isolation-20261025` — `18a05900b4c2`.
- `emuwiz-job20` — `f2be8c441ae0`.
- `emuwiz-job29-pcsx2` — `b2bb4dc0c597`.

### batch3

- `emuwiz-cheat-conversion-blue` — `6434cd3c3994`.
- `emuwiz-deps-tls-20261023` — `f13495e73bea`.
- `emuwiz-exact-duplicate-validation-20261018` — `920defc10db9`.
- `emuwiz-gui-slice2b` — `04df0109acad`.
- `emuwiz-mame-catalogue-guard-successor` — `4b01498ab5ea`.
- `emuwiz-resolver-main-integration` — `2c3247a5c49e`.
- `emuwiz-review-ecd340eb-main-replay` — `c187d2f6b0c9`.

### batch4-A

- `emuwiz-cargo-journal-recovery-20261021` — `199883229c97`.
- `emuwiz-cbr-support` — `3b166b523907`.
- `emuwiz-final-integration` — `ee807a9ac433`.
- `emuwiz-gui-slice2a` — `cf5122495015`.
- `emuwiz-installer-race-20261023` — `5049eda626a1`.
- `emuwiz-mods-audit` — `7f42c9876b75`.
- `emuwiz-research-gui-legacy-v2-parity-audit` — `7f421daf6463`.

### batch4-A2

- `emuwiz-update-baseline` — `dca470eecd78`.
- `emuwiz-recon-explicit` — `d338925427b7`.
- `emuwiz-review-slice2-r1` — `0922dde518cf`.

### batch4-B

- `archivefs-romm-manual` — `9e211d12f00c`.
- `emuwiz-hackhash-phase2` — `f70127331399`.
- `emuwiz-identity-providers-poc` — `429e514887b7`.
- `emuwiz-old-backlog-save-directory-restore` — `1b7a467d7006`.
- `emuwiz-org-integration` — `fdc8acbdb7c8`.
- `emuwiz-pcsx2-texture-integration` — `ea4b6d370f71`.
- `emuwiz-job31-flycast` — `77f0a3f36999`.

### batch5-A

- `emuwiz-job42-catalogue` — `bf92d2c40c8b`.
- `emuwiz-job43-dat-identity` — `25340f2ef1de`.
- `emuwiz-job36-artwork` — `6e1b5652b6ad`.
- `emuwiz-blue-optical-layout-20261024` — `f6f370d63595`.

### batch5-B

- `emuwiz-zip-review-49808e1b` — `49808e1be327`.

## Evidence identity

The following hashes identify the completed local batch records without publishing their private contents.

| Record | SHA-256 |
| --- | --- |
| `batch1.json` | `98f2376b2c376b1951ae6e03453c2605eac724c991d2b4a249d8bfb118d75949` |
| `batch2.json` | `5f06143fb2b947663c17ca7c65eadb7d522446d56aa61eb0605322618003e5b1` |
| `batch3.json` | `771bfbab1d4ae103a9115d0cc0aa047dbf36795d9e5a9f4d6714a39c750e7bed` |
| `batch4-A.json` | `ccc4600fb58a4c2c4371e58077a252d9f1e74c05d06e283ab2214cc18221e595` |
| `batch4-A2.json` | `0ce087c480dcc5827b1aca2e5aab8229a0021c6861832f11c1783ea4da8bb56d` |
| `batch4-B.json` | `54d9338fdc523415fbabd1c49af076877f6230d2d61839add6559124331f2565` |
| `batch5-A.json` | `d2b35d8bdceaadf17b6f8422a247c7b387493ebe847137f133cbad05278696c4` |
| `batch5-B.json` | `5a3bbab20fc40f4e8381d9b8e31f2c60c14bfb7336ec6022e2bb3ba729cf72fb` |


## Actual post-merge legacy disposal results

| Internal batch | Registry before → after | Removed | Blocked | Allocated bytes removed | Observed free-space delta |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 131 → 119 | 12 | 3 | 1,685,479,424 | 1,674,502,144 |
| 2 | 119 → 106 | 13 | 2 | 1,810,677,760 | 1,831,133,184 |
| 3 | 106 → 95 | 11 | 4 | 1,547,014,144 | 1,558,872,064 |
| 4 | 95 → 84 | 11 | 4 | 1,548,705,792 | 1,555,820,544 |
| 5 | 84 → 80 | 4 | 0 | 565,428,224 | 565,719,040 |

The free-space deltas include concurrent activity; allocated checkout bytes are the removed-content measurement. External Cargo cache bytes reclaimed by this legacy procedure: **0**. Every removal was performed by the independently reviewed `worktree-reconcile --remove` command using normal `git worktree remove` without force.

### Removed checkouts

- `emuwiz-blue-amiga-adf-play-20261024` — `1f2377a38d59`; cache history **UNKNOWN**.
- `emuwiz-blue-eight-regressions-20261017` — `07fd7d1bbc70`; cache history **UNKNOWN**.
- `emuwiz-blue-identification-20261016` — `82ada656feb1`; cache history **UNKNOWN**.
- `emuwiz-blue-mod-controls-20261018` — `61dc60362e25`; cache history **UNKNOWN**.
- `emuwiz-blue-nkit-20261018` — `01e094f9b785`; cache history **UNKNOWN**.
- `emuwiz-blue-patch-output-recovery` — `76511e2b14b9`; cache history **UNKNOWN**.
- `emuwiz-blue-polish-20261017` — `b7fe89e0809a`; cache history **UNKNOWN**.
- `emuwiz-blue-save-snapshots-20261023` — `da0c370b7eaa`; cache history **UNKNOWN**.
- `emuwiz-blue-smoke-20261020` — `35f8c587c4e7`; cache history **UNKNOWN**.
- `emuwiz-blue-usability-20261021` — `fa054dd52a0d`; cache history **UNKNOWN**.
- `emuwiz-cargo-immediate-retirement-20261017` — `003f84659ae9`; cache history **UNKNOWN**.
- `emuwiz-cheat-concurrency-20261020` — `60e9fe8ae455`; cache history **UNKNOWN**.
- `emuwiz-cheat-followups-20261021` — `230eb4417f16`; cache history **UNKNOWN**.
- `emuwiz-cheat-staging-housekeeping-20261021` — `7a86ea5523d3`; cache history **UNKNOWN**.
- `emuwiz-dat-health-blue-20261017` — `62d07d9188b1`; cache history **UNKNOWN**.
- `emuwiz-dat-import-20261021` — `dbef0d96bcb7`; cache history **UNKNOWN**.
- `emuwiz-data-integrity-promote-20261016` — `9dcd9b676c8e`; cache history **UNKNOWN**.
- `emuwiz-dcp-workflow-20261018` — `e7cd0f84cc95`; cache history **UNKNOWN**.
- `emuwiz-dreamcast-gdi-blue-20261018` — `618f53009a6e`; cache history **UNKNOWN**.
- `emuwiz-ds-cheats` — `b2efca912d98`; cache history **UNKNOWN**.
- `emuwiz-esde-integrate-20261016` — `4979cec3b91a`; cache history **UNKNOWN**.
- `emuwiz-exact-duplicate-integration-20261018` — `dd9dc0552c94`; cache history **UNKNOWN**.
- `emuwiz-gui-artefact-clippy-20261022` — `b61eddb408de`; cache history **UNKNOWN**.
- `emuwiz-gui-history-undo-repair` — `60e9f0ca7109`; cache history **UNKNOWN**.
- `emuwiz-integration-nine-20261022` — `e2283d1a5e65`; cache history **UNKNOWN**.
- `emuwiz-library-discovery-journey-20261017` — `978c1a8f29ea`; cache history **UNKNOWN**.
- `emuwiz-mame-cheat-route-20261020` — `6b1ead7880bf`; cache history **UNKNOWN**.
- `emuwiz-mame-finalise-20261020` — `cff89a250941`; cache history **UNKNOWN**.
- `emuwiz-managed-dat-ps1-20261017` — `4071a52dd8e3`; cache history **UNKNOWN**.
- `emuwiz-melonds-cheat-identity-20261020` — `3bdba08cd929`; cache history **UNKNOWN**.
- `emuwiz-melonds-contract-20261020` — `50f7efd53d21`; cache history **UNKNOWN**.
- `emuwiz-mesen-fds-20261018` — `6fb066491fc9`; cache history **UNKNOWN**.
- `emuwiz-platform-coverage-20261022` — `9fba0b0047a8`; cache history **UNKNOWN**.
- `emuwiz-playing-library-ux-20261023` — `afcf9f81f843`; cache history **UNKNOWN**.
- `emuwiz-psp-analyser-20261023` — `33c52928ee9e`; cache history **UNKNOWN**.
- `emuwiz-recon-support-files-20261017` — `c6cd0bcd83fc`; cache history **UNKNOWN**.
- `emuwiz-release-installer-safety` — `175810b8f2f6`; cache history **UNKNOWN**.
- `emuwiz-release-rc-20261021` — `e2283d1a5e65`; cache history **UNKNOWN**.
- `emuwiz-retroarch-cheat-readiness-20261024` — `5bc90cc3572f`; cache history **UNKNOWN**.
- `emuwiz-retroarch-cheats-core-20261020` — `ac7cd28aa42a`; cache history **UNKNOWN**.
- `emuwiz-retroarch-journey-20261022` — `ab355abce766`; cache history **UNKNOWN**.
- `emuwiz-retroarch-refusal-20261022` — `efe8ab09e183`; cache history **UNKNOWN**.
- `emuwiz-retroarch-verify-20261020` — `45cda4b413d9`; cache history **UNKNOWN**.
- `emuwiz-romm-manual-viewer-20261020` — `22e185c73960`; cache history **UNKNOWN**.
- `emuwiz-savestate-guidance-20261017` — `6c4989a40c08`; cache history **UNKNOWN**.
- `emuwiz-scummvm-trainer-20261020` — `8d80472d3560`; cache history **UNKNOWN**.
- `emuwiz-scummvm-validate-20261021` — `10ebb1ae52ed`; cache history **UNKNOWN**.
- `emuwiz-tape-identity-audit-20261018` — `ad8978e5afa7`; cache history **UNKNOWN**.
- `emuwiz-verified-game-fixture-20261021` — `4e5bdffd7925`; cache history **UNKNOWN**.
- `emuwiz-whdload-fsuae-config-20261024` — `4d78e74cf5c1`; cache history **UNKNOWN**.
- `emuwiz-zx-spectrum-20261022` — `e2283d1a5e65`; cache history **UNKNOWN**.

### Protected or unverified candidates

- `emuwiz-blue-amiga-readiness-20261023` — `CHECKOUT_NOT_CLEAN`; retained.
- `emuwiz-blue-arcade-input-20261024` — `TARGET_PRESENT_USE_NORMAL_LIFECYCLE`; retained.
- `emuwiz-blue-frontend-export-20261017` — `PRODUCTION_DEPLOYMENT_UNRESOLVED`; retained.
- `emuwiz-cue-dat-final-20261023` — `TARGET_PRESENT_USE_NORMAL_LIFECYCLE`; retained.
- `emuwiz-homebrew-discovery-phase1-integration` — `TARGET_PRESENT_USE_NORMAL_LIFECYCLE`; retained.
- `emuwiz-job37-playing-library` — `TARGET_PRESENT_USE_NORMAL_LIFECYCLE`; retained.
- `emuwiz-mame-catalogue-guard` — `TARGET_PRESENT_USE_NORMAL_LIFECYCLE`; retained.
- `emuwiz-mame-optional-merged-disk-successor` — `TARGET_PRESENT_USE_NORMAL_LIFECYCLE`; retained.
- `emuwiz-openrom-regression-review` — `TARGET_PRESENT_USE_NORMAL_LIFECYCLE`; retained.
- `emuwiz-rename-dir-durability-r3-syncsafety` — `LIFECYCLE_LOCKED`; retained.
- `emuwiz-rescue-cue-freshness-20261019` — `TARGET_PRESENT_USE_NORMAL_LIFECYCLE`; retained.
- `emuwiz-retrobios-viewer-20261023` — `LIFECYCLE_LOCKED`; retained.
- `emuwiz-saturn-ssp-20261016` — `PRODUCTION_DEPLOYMENT_UNRESOLVED`; retained.

The valuable unfinished feature groups and explicitly protected jobs/builds were excluded from the cleanup plan. R2/R3 unresolved deployment conditions were never converted into claims of live ownership or silently cleared. Full process, inode, pin, Git, ownership and preservation checks and private receipts remain in the local maintenance ledger.

## Additional completed ordinary-lifecycle batches

The existing maintenance ledger also contains the following ordinary lifecycle-CLEAR removals, completed before this legacy-disposal phase. They account for additional registry movement while PR CI was running and are reported separately; they are not attributed to the new legacy procedure. Their recorded Git bundles and local-change archive checksums were verified from the existing evidence. These receipts include archived R2/R3 review/probe checkouts; they do not establish that surviving production deployment or ownership conditions are resolved.

| Existing batch | Registry before → after | Removed | Allocated checkout bytes removed | Observed free-space delta |
| --- | ---: | ---: | ---: | ---: |
| batch6-A | 146 → 138 | 8 | 1,581,850,624 | 1,263,849,472 |
| batch6-B | 138 → 131 | 7 | 914,817,024 | 899,956,736 |
| batch6-B2 | 132 → 131 | 1 | 131,825,664 | 130,695,168 |

These ordinary receipts cover **16 additional checkouts** and **2,628,493,312 allocated bytes**. Together with the earlier records and this report’s 51 legacy disposals, the complete ledger records **144 removed checkouts** and **21,012,480,000 allocated checkout bytes**. Concurrent additions and other filesystem activity prevent treating the initial-to-final registry difference or filesystem free-space delta as a task-attributed removal measurement.

### Additional recorded removals

- `archivefs-dat-summary-hidden-fields` — `e385e1dec606`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-blue-bezel-history-20261024` — `6fc4677c9641`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-canon-rename` — `e2dbec5b288f`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-cue-chd-current-main` — `f6981c8dae45`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-esde-rereview` — `3a80cf03588d`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-internal-gold` — `0d62bc805452`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-main-release-fix` — `daa6d86fb768`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-mame-chd-repair-boundary-successor` — `4d972b63ce20`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-mame-family-performance` — `30c7542e4636`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-r2-cutover-review` — `68c3cbb22c68`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-r2-integration-20261023` — `5fda390d9499`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-r2-legacy-probes-20261013` — `26c71a084cdc`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-r2-write-journal-hook-20261014` — `1909589bee4e`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-r3-integration-20261013` — `2fb61afc3586`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-zip-reviewed-preview` — `aa9eedc4febf`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.
- `emuwiz-r2-readonly-probes-20261013` — `36a5a35e1463`; ordinary lifecycle-CLEAR receipt and preserved Git/evidence.

### Additional receipt identities

| Receipt | SHA-256 |
| --- | --- |
| `batch6-A.json` | `0c088d9bfc78c5a99d79fc0968b34b59da545f543211fd2095775cdef725cd0f` |
| `batch6-B.json` | `62a18bb251c3fa0fafa5b79589bee6e1d3417563c1dea8ea1a1fcbc17aaecbf5` |
| `batch6-B2.json` | `2180b104b34c4a1c585ebdc0c593b0d132ac899532f2d4f13bf86acc1f551f92` |

## Post-cleanup preservation findings

### Reflog-only history

`git worktree remove` deletes a checkout's own HEAD reflog and `ORIG_HEAD`. None of today's removal paths (lean batches 1–6 and the 51 legacy disposals) inspected them, so commits named only there became unreferenced. No history has been lost:

- No `git gc` or `git prune` has run on the shared repository since 2026-08-13 (the oldest pack index still present; a gc consolidates packs). Unreachable objects therefore still exist.
- `git fsck --unreachable --no-reflogs` (reflogs not treated as roots) found **1,864 unreachable commits** at 08:45, before the legacy batches, and the identical set at 08:58, after them: **0** newly orphaned commits outside the snapshot, **0** missing.
- All 1,864 are preserved in one bundle built from a private repository that only reads the shared object store through an alternate. It verifies, and its heads equal the unreachable set exactly. SHA-256 `3565e98aa1badf77c200ff095b7ad7f36ce820a702e3281e73f78474012b5c99` (7,384,339 bytes), kept with its commit list in the local ledger outside any worktree. **4** of these commits were already inside earlier preservation bundles; **1,860** are newly preserved.
- Not establishable: which removed checkout's reflog named which commit. The reflogs are gone, so attribution cannot be reconstructed; the commit contents are preserved regardless. Most are expected to be superseded rewrites.
- Do not run `git gc`, `git prune` or object-expiring maintenance on the shared repository until the owner has reviewed this snapshot.

The legacy `worktree-reconcile` command now refuses such history (see `docs/CARGO_CACHE_RETENTION.md`).

### Valuable-work guard in the ordinary batch runner

Nine checkouts classified **VALUABLE UNMERGED** in an earlier inventory were removed in ordinary batches batch6-A/B after that runner's guard was relaxed to accept entries recorded in the final 2026-10-26 preservation manifest: `archivefs-dat-summary-hidden-fields`, `emuwiz-canon-rename`, `emuwiz-cue-chd-current-main`, `emuwiz-esde-rereview`, `emuwiz-internal-gold`, `emuwiz-main-release-fix`, `emuwiz-mame-chd-repair-boundary-successor`, `emuwiz-mame-family-performance` and `emuwiz-zip-reviewed-preview`. An independent read-only verification found **preservation PASS for all nine**: every off-main commit is on a retained branch and in a bundle whose SHA-256 matches the record (including all 47 off-main commits of `emuwiz-internal-gold`); every recorded tracked edit and untracked file is in a checksummed archive (all 28 for `emuwiz-internal-gold`, all 20 for `emuwiz-main-release-fix`), each tracked diff reproduces the archived bytes from HEAD, and no ignored content existed. Equality with the live files at deletion rests on the runner's own byte comparison, corroborated by earlier archive snapshots. The default protection is restored: VALUABLE UNMERGED checkouts are refused unless an individually recorded owner authorisation names that exact path.

### Single disposal mechanism

`worktree-reconcile` remains the only legacy disposal mechanism. A separately developed and independently reviewed tool was used only as a reference for porting the missing safeguards above; it is not published. No further legacy removals are run until this hardening is merged.

