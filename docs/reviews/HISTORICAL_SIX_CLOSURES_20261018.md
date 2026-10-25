# Seven historical recovery closures — 18 October 2026

The owner explicitly closed these seven investigations and authorised retirement of their
obsolete local names and redundant archive refs. Fresh baseline: `29654c7ad540112291ce730b068d84ef8624533d`.
The identity-persistence addition was checked against `3595c4097864494a2d441b5f894171fee048245e`.
This is a closure of historical recovery work, not certification of every related runtime
feature. No original patch was replayed and no application code changed.

## Exact provenance and disposition

All seven original `refs/heads/feature/...` names were already absent at preflight.
For each row below, the retired local archive ref is
`refs/archive/emuwiz/2026-10-16/` followed by the exact historical branch name.
No ref was selected by a branch-name deletion pattern.

| Investigation | Historical branch | Exact tip |
|---|---|---|
| GameHacking browser import | `feature/gamehacking-browser-assisted-import` | `7c2ba059ed5d9478387d2b862404ac05a7f3c4c2` |
| PSP PBP | `feature/psp-pbp-production-wiring` | `b7362d110e1069c8ba7ddf85b00f902c1a02eb2a` |
| PS3 PKG | `feature/ps3-pkg-production-wiring` | `1f21a904906ba285c7294fa6ff46a238c08d854f` |
| Nintendo Launch rows | `feature/nintendo-launch-rows` | `0734aa930888fca20db82b4dabb200e66113bcba` |
| Set-Member Vocabulary | `feature/set-member-vocabulary` | `5ab728031de3b1dd01dd4369305b2f3ba6f585dc` |
| MAME Software Lists | `feature/mame-software-list-ingestion` | `baf2082ba37009f61f2d83ab7c4e847d083c8852` |
| Verified Identity Persistence | `feature/verified-identity-fact-persistence` | `4c6173fd3315002ebb54be37781ac6e20a8d495e` |

## Functional evidence and boundaries

### GameHacking browser import

Blue routes saved files, pasted/clipboard text and supported exports through the same bounded import engine, provider cache, provenance and normal preview/apply workflow. The absent historical CLI entry points are diagnostic convenience; the completed investigation explicitly rejects their restoration.

The existing closure was recorded by task `01a14bfd-6a0d-7b61-a802-718d30f3f927`
at 2026-10-17 23:01:40 UTC against `3592674a85130037cf41a1c8b83cd0a252e072c0`,
an ancestor of the fresh baseline. It traced Blue action routing, controller import and
provider-preview handoff; it did not rerun tests. A bounded fresh diff confirms the
import engine differs only in moving the shared Cloudflare classifier import; test
changes construct a hostile URL fixture and install private test roots. The historical
CLI commands are absent, and CLI interface equivalence is explicitly not claimed.
See [`GAMEHACKING_BROWSER_IMPORT.md`](../GAMEHACKING_BROWSER_IMPORT.md),
[`native_workflows.rs`](../../crates/archivefs-gui/src/gui_v2/native_workflows.rs),
[`controller.rs`](../../crates/archivefs-gui/src/cheats_mods/controller.rs) and
[`gamehacking_browser_import.rs`](../../crates/archivefs-core/src/patch_manager/gamehacking_browser_import.rs).

### PSP PBP

Production PBP structure/offset/PARAM.SFO discovery and identity wiring remain. Historical function bodies and original tests are retained; neither old generic GUI labels nor current metadata establish PSAR integrity or authorize native PBP launch.

The completed joint investigation compared historical/current function bodies and
all six original production-wiring tests, including the old generic GUI label arms.
Its owner ran 119 focused tests; independent source review passed. Structural metadata
identity is not payload integrity, installation or launch authority. The report is
`2026-10-18-psp-pbp-ps3-pkg/REPORT.txt` in the audit ledger.
See [`game_identity.rs`](../../crates/archivefs-core/src/game_identity.rs),
[`discovery.rs`](../../crates/archivefs-core/src/ingestion/discovery.rs), and the
PPSSPP/RPCS3 command and evidence-bridge modules under
[`launch`](../../crates/archivefs-core/src/launch).

### PS3 PKG

Production bounded PKG header, Content ID/TITLE_ID and discovery wiring remain. Payload validation, installation and native PKG launch were not supplied by the historical branch and remain intentionally unsupported.

The completed joint investigation compared historical/current function bodies and
all six original production-wiring tests, including the old generic GUI label arms.
Its owner ran 119 focused tests; independent source review passed. Structural metadata
identity is not payload integrity, installation or launch authority. The report is
`2026-10-18-psp-pbp-ps3-pkg/REPORT.txt` in the audit ledger.
See [`game_identity.rs`](../../crates/archivefs-core/src/game_identity.rs),
[`discovery.rs`](../../crates/archivefs-core/src/ingestion/discovery.rs), and the
PPSSPP/RPCS3 command and evidence-bridge modules under
[`launch`](../../crates/archivefs-core/src/launch).

### Nintendo Launch rows

All seven historical NES/SNES/GB/GBC/GBA/N64/Virtual Boy compatibility/export capabilities are represented by the current registry/RetroArch metadata and ES-DE mappings. Missing Cemu/Azahar Blue launch authorization is separate work the old rows never implemented.

The report `2026-10-18-nintendo-launch-treasure/comparison.md` records 22 owner
checks and 14 independently executed checks. It compared the exact old platform/export
patch with current candidate generation and the real Blue identity/dispatch boundaries.
Cemu/Azahar still lack end-to-end Blue launch authorization; installing an emulator or
observing structural title metadata does not supply it. See
[`platform_map.rs`](../../crates/archivefs-core/src/launch/platform_map.rs),
[`es_de_export.rs`](../../crates/archivefs-core/src/launch/es_de_export.rs), and
[`evidence_bridge.rs`](../../crates/archivefs-core/src/launch/evidence_bridge.rs).

### Set-Member Vocabulary

The unused positional diagnostic helper has no demonstrated missing product consumer. Modern nested DatMemberKey/MemberLocation, confidence verdicts, set attribution and whole-archive rename checks supersede its responsibilities; the old counter API remains absent and is not claimed equivalent.

The report `2026-10-17-set-member-vocabulary/REPORT.txt` traces all seven historical
tests and confirms that the original module had no production callers. Its owner ran
215 focused current tests; independent source review passed. The absent diagnostic
counters are acknowledged rather than represented as an equivalent current API.
See [`index.rs`](../../crates/archivefs-core/src/dat/index.rs),
[`audit.rs`](../../crates/archivefs-core/src/dat/audit.rs),
[`set.rs`](../../crates/archivefs-core/src/dat/set.rs), and
[`rename_plan/plan.rs`](../../crates/archivefs-core/src/dat/rename_plan/plan.rs).

### MAME Software Lists

Current software-list/listxml/FBNeo ingestion, nested member/disk metadata and CHD bridge retain the old behavior, with stronger bounds. All 16 historical software-list test bodies and the CHD bridge are retained. Disk-only emission and managed-DAT corrections remain separately owned.

The report `2026-10-18-mame-software-list-completeness/REPORT.txt` and `CLOSURE.json`
compare the original ingestion ancestry, software-list/listxml/FBNeo test definitions
and exact CHD bridge bodies. Seven static assertions passed; Rust tests were not rerun.
Software lists are not arcade machine sets; a complete media set or CHD header match is
not full payload integrity or runnable-machine proof. Provider-backed/all-borrowed disk-only emission was subsequently closed by promoted
`9c8e64fe` (see the addendum below); managed-DAT identity/persistence remains separately owned.
See [`mame_software_list`](../../crates/archivefs-core/src/identity_source/mame_software_list),
[`logiqx.rs`](../../crates/archivefs-core/src/dat/parsers/logiqx.rs), and
[`mame_redump_bridge.rs`](../../crates/archivefs-core/src/platform_evidence_fusion/mame_redump_bridge.rs).

### Verified Identity Persistence (same-batch addition)

The owner added this closure after the initial six-ref transaction. The existing
`2026-10-18-verified-identity-persistence-treasure/report.txt` and `evidence.json`
establish that the historical cache implementation, 21 cache tests, 11 diagnostic
tests and renumbered migration are byte-identical on main. A fresh bounded byte
comparison against `3595c409` confirms those recorded results; the integrated
successor `746434c0de402d2e3c53930a2030b4b6f04455d9` is an ancestor. No port or new
build is required. Cached explanatory facts never replace launch reverification.

The possible cached-size freshness weakness is a source-level concern, not a
reproduced runtime failure or historical restoration gap. The existing managed-DAT
agent received a handoff: saved size/default hashes may permit equal-size flat-file
changes to appear Current without a fresh observation. Its existing scope should
cover reopen plus equal-size edit/replacement (including preserved mtime) and a
missing file. No separate task or duplicate implementation was created. The active
managed-DAT candidate and selected-evidence owner's worktrees/refs remain untouched.

## Recovery and preservation

The complete, independently restore-tested USB bundle remains unchanged:
`/mnt/usbdrive/emuwiz-git-preservation/20261016-full-history/emuwiz-archivefs-all-refs-20261016.bundle`.
SHA-256: `9d44a755bbfcfb5f73f534ba1f3d66c0b7e0e8371b17beae594aa4db63144285`.
The original `MANIFEST.tsv` SHA-256 remains
`29826d9b75c6944564836218ce94fee3b0960aa9d155df120ac94c18815085b4`.
Fresh checksum and complete-history checks cover all seven original branch heads at their
exact tips. Restore history with `git clone --mirror <bundle> recovery.git`; consult the
unchanged USB manifest for the original names. Neither USB file is rewritten or deleted.
No Git garbage collection or unreachable-object pruning is performed.

Recorded closure-evidence digests (source reports remain in the audit ledger):

| Report | SHA-256 |
|---|---|
| `2026-10-18-retire-six-historical/gamehacking-existing-closure-messages.json` | `b05c798ea07a29f7e15084a4347bb49ccaa31809a127bc1518e70d90269a4827` |
| `2026-10-17-set-member-vocabulary/REPORT.txt` | `34008232a1f5f2de3206979438bee6333ed39e603e317546b746a10997dc5a7d` |
| `2026-10-18-psp-pbp-ps3-pkg/REPORT.txt` | `b7fe6a689e093aa1da05115843d48681ba029e7e91d41ae71ce7be68adc67b9f` |
| `2026-10-18-nintendo-launch-treasure/comparison.md` | `93d8ff93a8e807f207c85b5c0b3a57d44f9da65c71c666e6e022533d888d8039` |
| `2026-10-18-mame-software-list-completeness/REPORT.txt` | `d5f6c299251e0a2c2087b279d3ba2f4cb569fc0d0a2290b8383c0ca7271ddc49` |
| `2026-10-18-mame-software-list-completeness/CLOSURE.json` | `de8b1f03daebf1d94380848750a2acf511becb1716c033c09d5fed921c2af727` |
| `2026-10-18-verified-identity-persistence-treasure/report.txt` | `e0a26cffb319196a70ccf67f7def10a12bd0244ffa7d88b2dce2c9bb2fca39b3` |
| `2026-10-18-verified-identity-persistence-treasure/evidence.json` | `24b5f44ed3abbb6a7ba8e4cd820041fd4c0abc5ba6fabfd00a2922383614260a` |

Exact deletion manifest, independent review, per-item receipts, ref snapshots and
worktree/Cargo HOLD reasons live in
`/home/davedap/emuwiz-audit-ledger/2026-10-18-retire-six-historical/`.
The original preservation manifest and closure reports remain intact; the new deletion
manifest records their hashes. The original upstream GameHacking branch and its local
remote-tracking ref are outside the local-head/archive-ref retirement scope.

## Remaining candidates after the additional closures

The five PARTIALLY REPRESENTED entries in
[`LEGACY_BRANCH_BACKLOG.md`](../LEGACY_BRANCH_BACKLOG.md) retain their exact preservation
refs. Research references, active/dirty/pinned/ambiguous worktrees, commits, production
data, ROMs and Cargo evidence are not discarded by this closure. Worktree removal
requires the existing trusted maintenance gate to return fresh CLEAR; a completed
investigation alone does not override a Cargo HOLD or source ownership.

## Five additional completed closures (18 October 2026)

This addendum reconciles completed reports against fresh `origin/main` `405ffc5f`.
It does not repeat feature investigations or run Cargo tests. Exact integrated-successor
ancestry, closure digests, original manifest rows, USB bundle heads and local expected
ref tips are checked in one deletion manifest. The authoritative backlog now contains
12 closed historical candidates, 16 patch-verified superseded entries and 5 unresolved
partial entries (33 implementation candidates total); no VERIFIED MISSING item remains.

| Historical branch | Exact historical tip | Closure and functional evidence |
|---|---|---|
| `feature/arcade-version-live-data` | `3db194ba05d5092090396cecfba6d85df01fed17` | Exact historical patch is integrated as `999e22e6` (stable patch ID matches). Doctor consumes saved DAT revisions and a bounded MAME version probe; Blue shows catalogue version/source/provenance, machine/software distinctions and stale/unknown evidence. Version compatibility remains advisory. |
| `feature/snes-production-wiring` | `f56d3c81e904a2b03d8b35690ed93b5a4da3edf7` | Completed SNES comparison confirms production header wiring through `53e5581a`; core functions are identical and current native verification/launch infrastructure is reused. Managed-DAT and Discovery work remain separately owned. |
| `feature/tape-identity-phase2` | `c362cc13da4ce2d72eccde63d3dd459b645106c0` | Tape identity code is retained through separate integration `10be8f0d`, not original-tip ancestry. Independently reviewed regression fix `afd35956` preserves shared TZX/CDT platform neutrality; 91 focused and 91 independent checks passed before the disjoint rebase. The source worktree/target remains on lifecycle HOLD. |
| `feature/disk-only-set-emission` | `b47ed81e0ab7ab5f5da7eb5e9fd9566d375a645a` | Completed comparison of all 17 historical tests led to promoted correction `9c8e64fe`, with 327 focused checks and independent exact-commit review. Provider-anchored all-borrowed emission reuses current policy; CHD header identity is not payload verification, runnability or archive provenance. |
| `feature/gui-history-recovery-view` | `3c977acd889e4f87fa6745715c9e2b4fa3763a3f` | Completed capability comparison plus promoted `a35d36cd` connects Blue History to existing Repair History and explains resume authority. Historical forward replay is deliberately not restored: current-generation/approved-plan authority is required. Independent final checks passed 76/76; no second executor or journal system was added. |

The table above extends the earlier closure table: each row names an exact original
historical tip, rather than a newer source-worktree branch. Tape's original tip is not
claimed to be an ancestor; `10be8f0d` is its adapted integration and `afd35956` is the
separately promoted shared-container regression correction. Neither current ref is deleted.
Arcade's stable patch ID is `3913d9dc3b0360ea3ee2a276a1dea67eef2471b9` for both
`3db194ba` and `999e22e6`.

Completed evidence used, without repeating tests:

- Arcade: `2026-10-18-arcade-version-data-comparison/closure.json`; static source review
  and patch equality, not a claimed fresh runtime GUI test.
- SNES: `2026-10-18-snes-production-wiring/CLOSURE.json` and `REPORT.txt`; no new code
  or tests required by that closure. Active DAT/Discovery ownership remains separate.
- Tape: `2026-10-18-tape-identity/REPORT.json` and `independent-review.txt`; 91 focused
  and 91 independent checks passed before a disjoint rebase; reviewed files were unchanged.
- Disk-only: `2026-10-18-disk-only-set-emission/REPORT.txt`, `PROMOTION.txt` and
  `INDEPENDENT_REVIEW.txt`; 327 passed and independent exact-commit source review PASS.
- History: `2026-10-17-blue-history-recovery/validation.txt`, `promotion.txt` and
  `docs/design/blue-history-recovery-comparison.md`; 284/286 owner checks, both failures
  reproduced on baseline, followed by 76/76 independent final checks. Historical forward
  exact resume differs from Undo and is intentionally not restored without current authority.

The exact new manifest and focused independent safety review are retained under
`/home/davedap/emuwiz-audit-ledger/2026-10-18-retire-newly-closed/`. Its actions delete
only these four archive refs and one original local head with expected-old SHA checks:

- `refs/archive/emuwiz/2026-10-16/feature/arcade-version-live-data`
- `refs/archive/emuwiz/2026-10-16/feature/snes-production-wiring`
- `refs/archive/emuwiz/2026-10-16/feature/tape-identity-phase2`
- `refs/heads/feature/disk-only-set-emission`
- `refs/archive/emuwiz/2026-10-16/feature/gui-history-recovery-view`

All five original heads are retained in the same complete USB recovery bundle and
unchanged `MANIFEST.tsv` described above. Fresh full checksums, complete-history verification
and exact original-head matching precede deletion. No USB recovery file is rewritten,
no remote branch is deleted and no Git garbage collection or pruning is performed.
Source worktrees and targets remain intact regardless of completion: Tape records a
registry-lock lifecycle HOLD; disk-only and History sources have been reused or await
maintenance; the running Blue executable/pin and other agents' dirty work are outside scope.

The five remaining historical refs are NES/FDS, PC Engine CD firmware readiness, GUI DAT
identity wiring, GUI Selected Evidence and GUI standalone launch wiring. The completed
Selected Evidence report explicitly leaves a real presentation gap and an unvalidated patch
blocked by managed-DAT ownership; it is not closed. The other four lack a completed closure
in this bounded batch. They remain preserved, without asserting all four are proven missing
features. Research references and active implementation branches remain unchanged.

New deletion manifest SHA-256: `1f7997411a1d60842e8b7a3934cf9f951705e259d4e2dd8501c8612e17eaaa12`.

Pinned new closure evidence (paths relative to the external audit ledger):

| Evidence | SHA-256 |
|---|---|
| `2026-10-18-arcade-version-data-comparison/closure.json` | `7b34d706a8b1c2e2ab8282992bedae3c7f8f430e28e170e87170d18923bd93dd` |
| `2026-10-18-snes-production-wiring/CLOSURE.json` | `061c0164b418627b6d3c8910dac8e110ef920b68af21b3ed459d504043264be0` |
| `2026-10-18-snes-production-wiring/REPORT.txt` | `c89ccabaa6815fadd5bb7a2be2b749beece17a6f16ba6ead111a58144fc45769` |
| `2026-10-18-tape-identity/REPORT.json` | `6497adb2d8c8466240d339dd21cca60dbef6c6b7da6e1fc7a744c2c014448dca` |
| `2026-10-18-tape-identity/independent-review.txt` | `dc74941bb34c8a11a2a1f7d5a6cb3f6cd950520e39829d44c56136220f71d3a6` |
| `2026-10-18-tape-identity/task-completion-status.json` | `25bb6b356d35e1eb65dc2a55408ecf984d22c9cf60876b49434cd101d398ef92` |
| `2026-10-18-disk-only-set-emission/REPORT.txt` | `ba6d35689d5e63b8af9bd7a8a1dbf6668bf2e0ada5002b68b140bb5c4b5caf2e` |
| `2026-10-18-disk-only-set-emission/PROMOTION.txt` | `860e83dcf70eb981ef2617f9f43f0bdd56917be61d5d344290f1134cd1228343` |
| `2026-10-18-disk-only-set-emission/INDEPENDENT_REVIEW.txt` | `d24cd7f856d2db1307b117a2547d80c9044d66586bc98d888f3cc79895947065` |
| `2026-10-17-blue-history-recovery/validation.txt` | `98adfa950a33a95df696585b208cc6800d27ee1425bc4167c25d813283171510` |
| `2026-10-17-blue-history-recovery/promotion.txt` | `78a17eceb9f50cc84615b764459eb8b19fd175b10c3800adb2865afd35a29c71` |
| `/home/davedap/emuwiz-blue-history-recovery-20261017/docs/design/blue-history-recovery-comparison.md` | `d0cbf7d6175ee1e660fafa2ef97f62904aed6d81260e6e6eb44efd4bc381ad79` |
