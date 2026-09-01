# Legacy Branch Backlog

## Purpose and scope

This is a documentation-only inventory of the surviving non-authoritative EmuWiz branches identified by `emuwiz-backlog-branch-audit-20260901.txt`, plus the four live worktrees and four explicitly preserved reference branches named by the backlog brief.

The authoritative branch `feature/archivefs-unified-platform` at `1a79cb4` is intentionally not classified. The supplied audit's unique-patch counts are retained for the 28 audit-listed feature branches. For the additional branches, counts are measured as commits not reachable from `1a79cb4`; live uncommitted work is called out separately. A unique commit is not automatically a safe cherry-pick: each branch still requires an API, behavior, and conflict review against the current baseline.

## Inventory summary

| Classification | Count | Meaning |
|---|---:|---|
| ACTIVE | 4 | Live worktrees; keep under active ownership. |
| P1 — MODERNISE SOON | 8 | Bounded identity/media work with high near-term value. |
| P2 — VALUABLE BACKLOG | 12 | Useful capability, but larger scope, overlap, or validation cost. |
| ARCHITECTURAL / LATER | 6 | Cross-cutting persistence, launch, DAT, or GUI architecture. |
| RESEARCH / HISTORICAL REFERENCE | 4 | Design history and research; preserve as reference, not replay sources. |
| SUPERSEDED / SAFE TO DELETE LATER | 2 | Content is represented by newer/current work; retain until deliberate cleanup. |
| **Total named non-authoritative branches** | **36** | 28 audit-listed branches + 4 active worktrees + 4 explicit reference branches. |

The authoritative checkout currently has unrelated untracked scratch directories (`amiga`, `c64`, `gb`, `gba`, `gbc`, `n64`, `nes`, `ps2`, `saturn`, and `static`). They are outside this catalogue and were left untouched.

## ACTIVE

These branches have live worktrees and must remain active. The first two have no committed divergence from `1a79cb4`, but do contain live uncommitted work and therefore are not cleanup candidates.

| Branch | Tip SHA; last commit subject | Unique patch count | Rough scope and value | Difficulty | Conflict surface | Recommended action |
|---|---|---:|---|---|---|---|
| `feature/dat-disk-only-sets` | `a7217e9`; `feat(gui): support duplicate quarantine review and apply (#65)` | 0 committed; live dirty worktree | DAT disk-only dependency/set emission and duplicate-quarantine review; current dirty core edits are part of active work. | High | DAT; GUI | Continue current work; audit before any later integration. |
| `feature/non-cheat-mod-foundation` | `3a55df3`; `feat(gui): complete PCSX2 cheat apply and undo` | 0 committed; live dirty worktree | Non-cheat mod package foundation and controlled PCSX2 apply/undo groundwork; current `mod_package.rs` work is active. | High | core identity/provider; GUI; launch | Continue current work; do not replay older branches over it. |
| `feature/ps4-identity-phase1` | `c7bd006`; `feat(identity): add bounded PS4 PARAM.SFO identity` | 1 | Bounded PS4 `PARAM.SFO` identity evidence and platform wiring. | Medium | core identity; launch | Continue active phase; replay only after its own review is complete. |
| `feature/tape-identity-phase2` | `c362cc1`; `feat(identity): add bounded Spectrum and CDT tape inspection` | 1 | Bounded Spectrum/CDT tape inspection and evidence plumbing. | Medium | core identity | Continue active phase; audit against the current tape policy before landing. |

## P1 — MODERNISE SOON

These are the recommended near-term candidates because they add bounded, testable media or identity evidence. “Replay onto current baseline” means a deliberate, reviewed replay of the useful patch, not a blind branch merge.

| Branch | Tip SHA; last commit subject | Unique patch count | Rough scope and why valuable | Difficulty | Conflict surface | Recommended action |
|---|---|---:|---|---|---|---|
| `feature/acorn-dfs-evidence` | `596c021`; `feat(identity): add Acorn DFS disk evidence` | 1 | DFS structural parsing, disk ingestion, platform evidence, and tests; fills a concrete Acorn media gap. | Medium | core identity | Audit tests/API, then replay onto current baseline. |
| `feature/d64-structural-evidence` | `ca1da32`; `feat(identity): add D64 structural evidence` | 1 | D64 geometry/structure evidence and discovery tests; gives Commodore matching stronger evidence than extension hints. | Medium | core identity | Audit against current Commodore policy, then replay the focused patch. |
| `feature/nes-fds-production-wiring` | `4675cf7`; `feat(identity): wire NES and FDS production evidence` | 1 | NES header and FDS production registration/discovery; small enough for a focused modernization. | Low | core identity | Replay onto current baseline after test/API audit. |
| `feature/spectrum-content-identity` | `3115904`; `feat(identity): add Spectrum snapshot and +3 disk evidence` | 1 | Snapshot and +3/Dsk evidence, ingestion, and database/GUI seams; materially improves Spectrum content identity. | High | core identity; GUI | Audit in pieces; replay only the bounded core portions first. |
| `feature/spectrum-trdos-evidence` | `e333f37`; `feat(identity): add Spectrum TR-DOS media evidence` | 1 | TRD/SCL media parsing and ingestion tests; closes a specific Spectrum disk-evidence gap. | Medium | core identity | Audit format/policy interactions, then replay focused core work. |
| `feature/x68000-xdf-dim-evidence` | `988b1ad`; `feat(identity): add X68000 XDF/DIM evidence` | 1 | XDF/DIM structural evidence and media registration; bounded platform coverage with strong test value. | Medium | core identity | Replay after a current-registry/API audit. |
| `feature/psp-pbp-production-wiring` | `b7362d1`; `feat(identity): wire PBP evidence into production` | 1 | PSP PBP production discovery, media registration, and evidence wiring. | Medium | core identity; GUI | Audit the current PSP model, then replay the focused backend patch. |
| `feature/ps3-pkg-production-wiring` | `1f21a90`; `feat(identity): wire PS3 PKG discovery` | 1 | PS3 PKG discovery, bounded identity evidence, and media registration. | Medium | core identity; GUI | Audit the GUI touch and replay backend pieces first. |

## P2 — VALUABLE BACKLOG

These branches contain valuable work, but their size, external-data assumptions, duplicated prerequisites, or broader conflict surface makes them second-wave modernization candidates.

| Branch | Tip SHA; last commit subject | Unique patch count | Rough scope and why valuable | Difficulty | Conflict surface | Recommended action |
|---|---|---:|---|---|---|---|
| `feature/amiga-hdf-filesystem-traversal` | `09b4223`; `feat(core): add bounded Amiga filesystem traversal` | 2 | Bounded Amiga filesystem traversal plus TOSEC and WHDLoad import/conversion work; high-value evidence for Amiga collections. | High | core identity; DAT | Audit the large combined patch and split reusable filesystem work before replay. |
| `feature/apple-media-registration` | `a5b6c14`; `feat(identity): register Apple media` | 1 | Apple media registration, inspection, discovery, and platform tests. | Medium | core identity; GUI | Audit current Apple registry, then replay backend pieces. |
| `feature/arcade-version-live-data` | `3db194b`; `feat(doctor): feed live arcade version provenance` | 1 | Arcade DAT/version provenance, source validation, diagnostics, CLI, and GUI reporting. | High | DAT; GUI; docs/research | Audit data policy and diagnostics contracts before replay. |
| `feature/atari-p0-identity-wiring` | `1b194d1`; `feat(identity): wire Atari platform evidence` | 1 | Atari platform evidence through identity, inspection, media, and launch bridges. | High | core identity; launch | Audit evidence precedence and launch implications first. |
| `feature/gamehacking-browser-assisted-import` | `7c2ba05`; `Import GameHacking.org pages and exports through the user's own browser` | 1 | Browser-assisted GameHacking import with provider logic, fixtures, CLI, and GUI integration. | High | core identity/provider; GUI | Preserve fixtures and core parser ideas; audit browser/GUI boundaries first. |
| `feature/mame-software-list-ingestion` | `baf2082`; `feat(core): add FBNeo evidence ingestion` | 3 | MAME listxml, Redump CHD bridge, FBNeo ingestion, DAT classification, and lineage tests. | High | core identity; DAT | Split by source and audit current DAT model before replay. |
| `feature/modern-nintendo-identity-platforms` | `b04b074`; `feat(identity): add modern Nintendo platform variants` | 1 | Adds modern Nintendo identity/platform variants and launch bridge coverage. | Medium | core identity; launch | Audit against current platform vocabulary, then replay if still absent. |
| `feature/n64-cic-real-corpus-validation` | `ca46a0b`; `test(identity): validate N64 CIC against real IPL3 corpus` | 2 | N64 CIC/header evidence and real-corpus validation; preserves useful correctness evidence even where prerequisite work overlaps. | Medium | core identity; docs/research | Extract corpus/tests and audit prerequisite overlap before replay. |
| `feature/nintendo-launch-rows` | `0734aa9`; `feat(launch): add Nintendo compatibility rows` | 2 | Nintendo launch compatibility rows, platform maps, and ES-DE export adjustments. | High | launch; GUI | Audit against current launch policy; replay only verified rows. |
| `feature/pcengine-cd-firmware-readiness` | `5f44552`; `feat(firmware): add PC Engine CD System Card readiness` | 1 | PC Engine CD boot evidence, System Card readiness, and patch-manager tests. | High | core identity; launch | Audit firmware safety and identity boundaries before replay. |
| `feature/snes-production-wiring` | `f56d3c8`; `feat(identity): wire SNES header evidence into production` | 1 | SNES header evidence production wiring in the core identity model. | Medium | core identity | Audit current header model, then replay the single focused change. |
| `feature/whdload-dat-reconciliation` | `7b3376c`; `feat(dat): wire WHDLoad slave identity` | 1 | WHDLoad slave identity conversion, reconciliation, discovery, and tests. | High | core identity; DAT | Audit against Amiga/TOSEC work and replay only after reconciliation review. |

## ARCHITECTURAL / LATER

These branches are substantial seams rather than isolated features. They should be redesigned around the current baseline before any selective replay.

| Branch | Tip SHA; last commit subject | Unique patch count | Rough scope and why valuable | Difficulty | Conflict surface | Recommended action |
|---|---|---:|---|---|---|---|
| `feature/esde-integration` | `6f3c5b4`; `feat(core): add read-only ES-DE export/launch-entry plan` | 1 | Read-only ES-DE export and launch-entry planning; useful integration boundary, but a large new launch module. | High | launch | Audit contract and keep as a later design source. |
| `feature/gui-dat-identity-wiring` | `34f016c`; `feat(gui): wire persisted DAT identity into library details` | 7 | DAT identity persistence, DOS evidence/launch readiness, migration 0008, and broad GUI wiring. | High | core identity; DAT; GUI; launch | Redesign/audit architecture first; do not replay wholesale. |
| `feature/gui-selected-evidence-view` | `cd0a747`; `feat(gui): add No-Intro source resolution for selected evidence` | 2 | Selected-evidence presentation and No-Intro resolution registry/UI. | High | core identity; GUI | Preserve the interaction model; redesign against current GUI state. |
| `feature/gui-standalone-launch-wiring` | `c49a201`; `feat(gui): wire standalone emulator launch contexts` | 1 | Standalone launch contexts, readiness page, platform map, and GUI tests. | High | GUI; launch; emulator setup | Audit launch safety and current setup contracts before replay. |
| `feature/set-member-vocabulary` | `5ab7280`; `dat: name the set/file/archive/member containment levels` | 1 | Explicit DAT set/file/archive/member containment vocabulary and audit plumbing. | Medium | DAT | Audit terminology compatibility; modernize as a deliberate model change. |
| `feature/verified-identity-fact-persistence` | `4c6173f`; `feat(identity): persist verified catalogue identity facts` | 1 | Verified identity cache, diagnostics, database changes, migration 0008, and GUI/CLI seams. | High | core identity; GUI; launch | Audit persistence and migration requirements first; later architecture candidate. |

## RESEARCH / HISTORICAL REFERENCE

These branches are valuable records of decisions, constraints, and prior UX reasoning. They are not direct cherry-pick candidates and should not be treated as production-ready implementation sources.

| Branch | Tip SHA; last commit subject | Unique patch count | Rough scope and why valuable | Difficulty | Conflict surface | Recommended action |
|---|---|---:|---|---|---|---|
| `design/user-controlled-dat-cheat-policy` | `ea1373d`; `docs: finalise DAT and cheat policy decisions` | 4 | DAT/cheat policy audits, GUI design, migration design, and model decisions; records trust and ownership constraints. | High | DAT; GUI; docs/research | Preserve only; consult when modernizing policy. |
| `docs/preserved-emuwiz-research` | `29c3d82`; `docs: restore preserved compatibility rename research` | 6 | Preserved compatibility, DAT, archive, security, naming, and completeness research corpus. | High | docs/research | Preserve only; use as background evidence. |
| `feature/gui-history-recovery-view` | `3c977ac`; `feat(gui): add exact resume recovery controls` | 10 | Exact-resume, transaction history, controlled apply/rollback, recovery pages, and extensive GUI tests. | High | GUI; launch; DAT | Keep as historical UX and safety reference; never replay wholesale. |
| `research/encrypted-action-replay-licensing` | `b17db2b`; `docs: correct encrypted AR text format terminology to base-32-style` | 2 | Licensing and encrypted Action Replay research with terminology corrections. | High | docs/research | Preserve only; no implementation replay. |

## SUPERSEDED / SAFE TO DELETE LATER

These branches should not be replayed. They remain listed so cleanup can be deliberate and evidence-based rather than accidental.

| Branch | Tip SHA; last commit subject | Unique patch count | Rough scope and disposition | Difficulty | Conflict surface | Recommended action |
|---|---|---:|---|---|---|---|
| `feature/disk-only-set-emission` | `b47ed81`; `fix(dat): emit borrowed disk-only dependency sets` | 2 | Earlier CHD/disk-only dependency and set-emission implementation; the active/current disk-only work supersedes this older shape. | Medium | DAT | Delete later after confirming the active branch/current baseline retains the needed semantics. |
| `feature/macintosh-dc42-evidence` | `89a6c49`; `test(disk): update PASTI budget-bound fixture` | 3 | Older DC42, PC Engine CD, and PASTI-fixture chain; useful history, but DC42 production evidence is represented by newer current work and the chain is not a clean replay unit. | High | core identity; DAT | Never replay the chain; delete later after retaining the audit record and confirming current coverage. |

## Recommended modernization order

The first ten should prioritize bounded, low-conflict identity/media work. Each item still needs a current-baseline audit before replay.

1. `feature/nes-fds-production-wiring` — small production wiring and focused tests.
2. `feature/x68000-xdf-dim-evidence` — bounded media evidence with limited surface.
3. `feature/acorn-dfs-evidence` — self-contained disk structure and identity evidence.
4. `feature/d64-structural-evidence` — strengthen Commodore structural evidence under the current fail-closed policy.
5. `feature/psp-pbp-production-wiring` — focused container/media production wiring.
6. `feature/ps3-pkg-production-wiring` — bounded package discovery, keeping GUI changes separate.
7. `feature/spectrum-trdos-evidence` — focused TRD/SCL evidence after tape-policy review.
8. `feature/spectrum-content-identity` — extract snapshot/+3 core work from its larger GUI/database seam.
9. `feature/snes-production-wiring` — small header-evidence addition after current identity review.
10. `feature/modern-nintendo-identity-platforms` — reconcile platform vocabulary before launch-row work.

After these, prioritize the P2 branches in source-sized slices: N64 corpus tests, Apple/Atari evidence, Amiga filesystem traversal, WHDLoad reconciliation, MAME source bridges, and only then the larger browser-assisted and PC Engine readiness work. Keep persistence, ES-DE, and GUI history branches as architecture/reference inputs until their current contracts are explicitly designed.

## Reading this inventory

- **Already integrated another way** means the branch's useful behavior is known to exist in newer/current work, even when the original commit IDs are not ancestors of `1a79cb4`.
- **Audit first** means inspect behavior, tests, API drift, and overlap before selecting any patch.
- **Preserve only** means retain the branch for decisions, fixtures, or historical reasoning; it is not a replay queue.
- **Delete later** is a proposed cleanup state only. No branch deletion is performed by this catalogue.

This catalogue makes no claim that any branch is production-ready merely because it has tests or a coherent commit message.
