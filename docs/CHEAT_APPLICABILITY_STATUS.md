# Cheat applicability and status

> **Consolidated:** the seam described below as needing reconciliation is now reconciled; see [`cheats/CHEAT_CONSOLIDATION.md`](cheats/CHEAT_CONSOLIDATION.md).

Starting commit: `4c980d184584dd5f1a22b5fbbf67e6c58ff204c1`.
Branch: `feature/cheat-applicability-status`.
Worktree: `/home/davedap/emuwiz-cheat-applicability-status`.

Scope: the new `patch_manager/cheat_applicability.rs`, its focused test
module, public exports in `patch_manager/mod.rs`, and this audit/report.

## Campaign boundary

At audit time, the duplicate/conflict and provenance campaign worktrees
were both based on the same starting commit and contained uncommitted
work. This task created a clean, isolated worktree from that commit. The
user explicitly instructed us to leave those worktrees alone and report
what happened. Their changes were neither copied nor cherry-picked.

This implementation therefore consumes the existing committed
`CheatReconciliationResult`, `CheatRelationship`, `CheatDocument`, and
their source/provenance fields. It does not depend on the in-progress
Batch 2 classifications or Batch 3 `CheatRecordProvenance` API. Those
new APIs need to be reconciled with this seam during batch integration;
this task does not claim that their uncommitted evidence has been
integrated or validated. Every supplied committed-model source variant
and provenance record is retained, with no winner selected.

## Audit: proof and inference

* `game_identity.rs` already gives identity facts a typed kind, status,
  confidence, and provenance. Only `Verified` facts can establish an
  identifier/hash match. `Candidate` filename/header facts remain
  candidates. A report's overall platform hint is not independently
  verified platform evidence.
* DAT/library identity persistence keeps authoritative audit attribution
  separate from title/name hints. This evaluator accepts verified content
  hash facts from the identity inspector; it does not manufacture DAT
  attribution from titles or filenames, or reinterpret a DAT member hash
  as the hash of a whole archive.
* `cheat_candidates.rs` compares serial, hash, title, platform, region,
  revision, and filename signals. Its historical `VerifiedExact` can
  include title/platform/region agreement; it also extracts revisions
  from names. Neither is promoted into verified applicability here.
* `cheat_journey.rs` carries a caller-authoritative game identity and
  requires an explicit selection and approved preview before installation.
  GUI candidate/local-import request construction currently also carries
  catalogue/display metadata. That metadata is not silently upgraded into
  verified inspector evidence by this evaluator.
* Dolphin inspection stores the raw region byte. The report adapter reuses
  the existing Gecko provider's game-ID region decoder. GameCube header
  revision can be verified; Wii outer-header revision remains a candidate.
  Numeric revisions are not guessed into `1.0`, `Rev A`, or another label.
* `cheat_route.rs` determines the selected emulator and apply support;
  `launch/planning.rs` preserves canonical resolved/unknown/conflicting
  identity without resolving those conflicts. Applicability consumes a
  resolved route; it never changes launch planning or emulator selection.
* `cheat_ir.rs` conversion assessment proves only reviewed conversions.
  A supported route alone does not prove support for arbitrary code syntax.
  Native RetroArch parser/writer evidence can establish container support,
  while opaque code support in the selected core remains unknown.
* `cheat_loadability.rs` checks installed bytes, paths, emulator config,
  and restart needs separately. Neither that module nor applicability
  proves that a cheat executed at runtime.
* The GUI's Cheats & Mods preview/routing modules already project match
  confidence and install/loadability. The new core `presentation()` is the
  minimal serializable label/summary seam for future GUI use; the page and
  existing workflows are not redesigned or wired to new activation policy.

## Result contract

`assess_cheat_applicability` is pure and read-only. Its typed report keeps
the result, match strength, ordered typed findings, every blocker and
warning, selected-game identity facts, source association, region/revision
evidence, full code document, parser evidence, source format, selected
route, separate emulator/format/engine support, conversion details,
provenance, native parser record, and complete reconciliation result.

Identity confidence follows the existing inspector's verification boundary
and the hash-before-verified-identity distinction in
`CheatRevisionEvidence`: exact content hash; verified identifier; title
with verified platform; title only; filename association; manual
association; unknown. Executable CRC and WHDLoad slave identity do not
claim a whole-game content hash. Matching release metadata is retained
independently and does not turn a title match into an exact identity.

Known region/revision disagreement always blocks readiness, including
when a content hash or title matches. Missing/candidate release values
stay unknown. A verified identifier with an unknown release is an exact
game match with warnings, rather than Ready. An exact content hash can
prove matching game data despite missing release labels; those missing
labels still appear in its warnings and plain-English summary. A source's
required hash/identifier must be checked; title agreement cannot bypass
missing required identity evidence. Platform aliases use the canonical
registry. Comparisons normalize case/whitespace, but do not infer revisions
from filenames or change code data.

Conflicting or unproven related source implementations require review.
Matching duplicate records add corroboration evidence without claiming
independent sources. No variant is dropped, merged into another code,
enabled, or selected. Contradictory verified inspector facts require review
regardless of input order. Findings, warnings, and blockers have stable
ordering. Summary priority is deterministic and does not remove other
blockers: malformed/missing code, source conflicts, identity conflicts,
region/revision/game mismatch, unsupported emulator/format, missing
identity/parser evidence, unknown capability, then match/readiness.

Normal labels are Ready to use, Exact game match, Strong match, Possible
match, Wrong region, Wrong revision, Different game, Unsupported cheat
format, Unsupported emulator, Needs review, Malformed cheat, and Missing
required evidence. Conflict variants project to Needs review with an
explanation that the sources provide different versions. Detailed evidence
remains available in the report. Applicability has no selection or
activation output and does not modify the existing explicit approval policy.

## Focused validation

The test module covers all 22 requested scenarios and additional boundary
cases: candidate evidence, contradictory identities, multiple blockers,
unknown core/engine, native parser authority, required hashes, executable
CRC scope, platform aliases, source-document platform mismatch, report
adapter fidelity, and title/release matches that cannot claim exactness.

All focused checks passed:

| Filter (`cargo test --offline -p archivefs-core --lib`) | Passed |
| --- | ---: |
| `cheat_applicability` | 39 |
| `patch_manager::cheat_ir` | 32 |
| `patch_manager::cht_document` | 33 |
| `patch_manager::cheat_route` | 24 |

`cargo fmt --all -- --check`, `git diff --check`, the task file-scope guard,
and the GUI root boundary guard passed. A separate targeted cargo check was
unnecessary because the focused library tests compiled the affected crate.

The first test attempt waited on the shared Cargo target lock and was
interrupted before running tests. Validation then used
`CARGO_TARGET_DIR=/tmp/emuwiz-applicability-target`, seeded with existing
build-cache artifacts, and rebuilt this worktree's source. The initial 28
applicability cases passed; after adding the remaining boundary tests and
tightening confidence, the final 39 passed. No source changes from another
campaign worktree were imported as part of that cache reuse.

The resulting commit SHA is reported with the completed task. No full
workspace suite, full GUI-v2 suite, release workspace build, live GUI smoke,
push, or promotion ran.
