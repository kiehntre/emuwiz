# EmuWiz final worktree reconciliation — 2026-10-26

## Disposition and approval state

The last completed cleanup batch left **143 registered worktrees**. A fresh registry-only check for this report found **143**; no archaeology inventory was repeated. The legacy-reconciliation implementation in this PR is a candidate awaiting owner approval; the exact-commit independent safety review and focused test results are recorded in the PR conversation. It has not been installed, used to remove checkouts, or merged by this task. Actual legacy-path removals in this task: **0**. Historical missing-target retirement remains **UNKNOWN**.

The remaining **69 `ARTIFACTS_MISSING_UNVERIFIED` candidates** are the reported batch-5 disposition, carried forward from the existing maintenance evidence. They were not subjected to another unchanged-HOLD removal batch. This report does not assert that all 69 are disposable: current Git, artifacts, process, ownership and production-deployment checks can still refuse individual paths.

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

Lean batches 1–5 removed **57 checkouts**, measuring **8,370,401,280 allocated bytes**. Including the earlier 20-path batch, the completed records cover **77 removed checkouts** and **11,226,681,344 allocated bytes**. The allocation measurements describe removed checkout content. Observed filesystem free-space movement includes concurrent activity and is not an isolated attributable measurement; those deltas must not be summed into a claimed exact reclaim total.

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

## Validation and remaining authorization

Focused deterministic regressions exercise missing targets; recreated directories/files/symlinks; target reuse after intent; active and uninspectable processes; dirty, untracked and ignored evidence; pins and build locks; shared/alternate ownership; malformed registry records; exact-ref preservation; changed source after intent; R2/R3 conditions; and preservation of external evidence and Git refs during normal removal. Synthetic fixtures are private temporary repositories and registries, with no Rust builds or production cleanup.

Independent safety review must cover the exact final candidate before this PR can be approved. The owner requested this PR URL for approval, so deployment, merge and legacy-path cleanup remain pending. Existing lifecycle gates remain authoritative for any separately authorized normal cleanup; no unchanged-HOLD batch was repeated here. Subsequent bounded legacy cleanup results must append actual verified removals and final counts to this report after approval and merge, without repeating archaeology.

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
