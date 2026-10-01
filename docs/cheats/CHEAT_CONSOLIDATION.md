# Cheat consolidation (current main)

Integration branch: `integration/cheat-consolidation-current-main`, based on
`main` @ `cf603f3e`. This reconciles seven independently built local cheat
branches into **one** model on top of what main already had. It does not add a
new cheat subsystem, and it does not replace the deterministic reconciliation
engine, the persisted review, the WHDLoad trainer workflow or the managed
patch/mod transaction machinery.

Status: **candidate for review, not promoted, not pushed.**

## What main already had (kept as-is)

`patch_manager/cheat_ir.rs` reconciliation, `cheat_reconciliation_plan.rs`
(`resolve_reviewed_cheat_plan`), `archivefs-gui` `cheat_reconciliation_review`
with persisted decisions, `patch_manager/whdload_trainer.rs`, the shared
patch/mod transaction path (`shared_preview` / `shared_transaction`),
`game_identity`, `platform_evidence_fusion::evidence_lineage`, and the existing
Cheats & Mods surfaces.

## Semantic diff of each candidate against current main

| Candidate | Classification | What happened |
|---|---|---|
| `7a47de05` RetroArch parser hardening | **Useful, portable** | Applied cleanly. It became the single `.cht` parser. |
| `bc93b26a` duplicates / conflicts | **Useful, manual adaptation** | Its inline parser rewrite was **superseded** by the hardened parser (which already classifies identical vs conflicting repeats). Kept: the cross-entry variant model (`cheat_ir/duplicates`), conflict-aware plan, review/persistence support. Ported onto the hardened parser: bounded `source_fields`, `conflicting_entry_indices`, `reconciliation_entries`. |
| `f0713e5b` provenance / evidence | **Useful, manual adaptation** | Its inline catalogue-parser edits were superseded. Kept: `CheatRecordProvenance` and friends; the catalogue's `source_evidence` and the document's original description/code now come from the hardened parser. |
| `b605a269` applicability status | **Useful, portable** (+ bridge) | Applied cleanly. It is *the* applicability model. Added one bridge, `CheatGameAssociation::from_entry`. |
| `dfc4ea09`, `2749aeaa` pack preview | **Useful, manual adaptation** | Applied, then its parallel types were collapsed (below). |
| `176fafec` safe launch composition | **Useful, manual adaptation** | Its applicability **stand-in was deleted**; the planner uses the real state. |
| `df036ddf`, `c1ec0f59` classic POKE | **Useful, manual adaptation** | Identity ladder and capability table collapsed onto canonical ones. |
| `7cc31028` appendconfig research | **Research only** | Its finding is respected by the launch planner (see below). |
| `e43ae942` capability audit | **Research only** | Consulted; no code. |
| `70dbaefe` per-launch workspace design | **Research only** | Consulted; the planner implements its planning half only. |

## One canonical model

| Concept | The one type | Replaced / collapsed |
|---|---|---|
| Parse a `.cht` | `cht_document::parse_cht_text` (bounded, hardened) | inline catalogue parser, inline duplicates parser |
| Cheat identity / entry | `CheatReconciliationEntry` (+ `CheatDocument`) | pack-preview's hand-built entries |
| Source / provenance | `CheatRecordProvenance` | ad hoc provenance strings; `local_with_sha256` shared by pack preview |
| What a source *claims* | `CheatApplicability` (region/revision/binary/engine) | — |
| How strongly a claim is evidenced | `CheatApplicabilityEvidence` (+ `ClaimStrength`) | — |
| Is it applicable to *this* game | `CheatApplicabilityState` via `assess_cheat_applicability` | launch stand-in enum; `CheatPackApplicability`; POKE identity ladder |
| Identity strength | `CheatApplicabilityMatch` | `PokeIdentityState` |
| Variants / conflicts | `CheatDuplicateKind` (+ `CorroboratingObservation`) | `CheatPackRelationship` |
| Same-title expectation | `CheatGameAssociation` / `CheatIdentityRequirement` | `CheatPackAssociation`, `CheatPackIdentityRequirement` (now aliases) |
| Selected review decision | `CheatReviewChoice` (existing, persisted) | — |
| Launch compatibility | `applicability_verdict(&report)` + `CheatLaunchPlan` | — |

The layers are *claims -> evidence -> assessment*, not parallel models:
`CheatGameAssociation::from_entry` turns an entry's claims and provenance
evidence into the expectation handed to `assess_cheat_applicability`. Claims are
never verification.

Two things are intentionally still distinct because they are different
questions, not duplicate answers: **variant conflicts** (the same cheat with
different payloads) and **POKE write overlaps** (two cheats writing one memory
location, `find_poke_conflicts`).

## Parser

Bounded input (8 MiB file, 8 KiB line, 4 KiB value, 256 code components, 16,384
distinct entries, 32 entry warnings, 256 document warnings), sparse and
out-of-order indices, any `u32` index, no allocation driven by a declared count,
malformed fields block the *entry* (fail conservative) while valid neighbours
stay selectable, backslashes stay literal. Conflicting repeats keep the first
value and bounded evidence of the later one and block that entry; identical
repeats are a non-blocking duplicate. Added here: values that differ only
beyond the retained bound are still a conflict.

## Duplicates and conflicts

Identical duplicates dedupe; conflicting variants stay distinct. Conflict
groups survive persistence (the review report round-trips through JSON, covered
by `cheat_consolidation_tests`). Nothing is last-write-wins: with no saved
choice the resolved plan selects nothing and lists the group as unresolved.

One deliberate policy: **file-wide metadata that conflicts** (for example two
different `cheat_delay` lines) does not make a *selected entry* uninstallable,
because the renderer never forwards globals, but it is carried to review as a
`SourceMetadataConflict` issue on every projected entry.

## Provenance

Typed source kind/quality, original artifact, original description/code,
normalization status, lineage and independence. Normal screens show only a file
name and a plain source label; paths, hashes and raw records are under Details.

## Applicability

`Ready`, `ExactGameMatch`, `StrongMatch`, `PossibleMatch`, `NeedsReview`,
`MissingRequiredEvidence`, `ConflictingVariants`, `WrongRegion`,
`WrongRevision`, `DifferentGame`, `UnsupportedFormat`, `UnsupportedEmulator`,
`Malformed`. Launch policy (`applicability_verdict`): only `Ready` and
`ExactGameMatch` launch freely; weaker states need explicit acknowledgement;
wrong game/region/revision, unsupported and malformed are blocked. A title that
merely looks similar is never enough (covered by an end-to-end test).

## Pack preview

Bounded, deterministic, read-only (`CheatPackPreview::can_apply()` is always
`false`). Wired into the existing **Add cheat folder** scan; it shows a plain
summary and always ends with "Nothing is enabled or installed by this preview."
No verified identity facts are supplied by that entry point, so matches there
stay title-level at best.

## Safe per-launch composition

**Planner and safety model only - not wired into a real launch.** It is typed,
pure, fail-closed, and encodes the appendconfig research: it never relies on
`--appendconfig` alone, hands RetroArch a generated scratch base config, pins
saves/states to the real directories, and treats the real config as a protected
reference verified by hash. A production request reports
`ProfileIsolationInsufficient` until an executor can supply a disposable
profile, so readiness is not faked.

## Classic microcomputer POKE

| Family | Status |
|---|---|
| ZX Spectrum (Fuse) | **Can be prepared** (`.pok` file) once exact identity is confirmed. No launch is wired. |
| Commodore 64 (VICE) | **Can be prepared** (monitor commands, unbanked byte writes only) once exact identity is confirmed. No launch is wired. |
| Atari ST (Hatari), CPC (Caprice32), MSX (openMSX), BBC/Acorn (BeebEm, b-em) | **Preview only.** Projection refuses even with verified identity. |

"Can be prepared" means the adapter generates an inspectable artifact; no
emulator is launched from it. ADF / non-WHD Amiga remains a separate unresolved
capability. The existing WHDLoad trainer workflow is untouched.

## Unresolved gaps (deliberately not papered over)

1. **No launch executor.** Per-launch composition cannot yet materialise,
   spawn, verify or clean up; "applied for this launch only" has no UI because
   there is nothing real to show. Failed-launch cleanup is a planner
   expectation (`MustNotExistAfter`) that nothing yet exercises.
2. **Applicability is not yet computed inside the main Cheats & Mods workflow.**
   The model and its bridge exist and are tested; the large existing GUI flow
   still uses its own match-confidence projection for the selected game.
3. **Opaque RetroArch codes cannot enter the generic resolved apply plan** (they
   are reported as unsupported); RetroArch installs keep using their own
   selection path.
4. **Pack preview from the GUI has no verified identity facts**, so it cannot
   report an exact match.
5. **Fuse/VICE preparation has no launch integration.**
6. `CheatGameAssociation::from_entry` maps only unambiguous claims; serial-style
   evidence is platform specific and stays in provenance.


## Four-finding review repair

Recreated the consolidation on authoritative `1a24276a` after verifying fetched
main/origin parity. The shared tree publication/recovery commits (`626afa87`,
`e306d7c2`) and subsequent CLI inspection work are retained unchanged. Launch variants now carry the whole
canonical applicability report. Hard findings (wrong game/region/revision,
conflicting identity, unsupported format/emulator, malformed or missing code)
refuse regardless of summary precedence, acknowledgement, or variant choice.
Purely reviewable conflicts still require explicit choice and acknowledgement.

Pack preview constructs native entries with the canonical parser projection,
uses the canonical assessor, and borrows the canonical grouping implementation.
Its catalogue indexes retrieve candidates only; they do not confer applicability.
Rows retain the assessed state and findings. `WouldAdd` is a read-only import
suggestion for verified identity without applicable blockers, never runtime
readiness: absent runtime-route capabilities remain visible in the report.
Declared but unverified region/revision constraints still require review.
Canonical classifications replace the old parallel labels (including CRC as a
verified identifier, and unproven mixed interpretations as ambiguous).

Repeated .cht fields retain a 32-byte SHA-256 of the complete decoded value in
addition to their bounded display prefix. The input file remains bounded and
hashing borrows its bytes; no full-value copy is retained. Legacy persisted
field evidence defaults the missing digest to unknown. Values differing only
beyond the retained prefix remain conflicting; identical huge values remain
observable duplicates and remain unselectable because of the existing bounds.

Fuse/VICE preparation requires both verified state and a concrete nonempty
identity. Other classic families remain preview-only. No launch executor,
RetroArch runtime isolation, ADF trainer, or broad GUI applicability wiring is
added by this repair.

### Repair validation

Authoritative base: `1a24276a1270114193490201eb4e542f7ef93f08` (clean,
fetched local main/origin parity). All 16 consolidation commits were replayed
unchanged before the focused repair; range-diff confirmed equivalence. The
external CLI promotion changed no core or GUI source.

| Validation | Result |
|---|---|
| Applicability | 40 passed |
| Launch cheat planning | 22 passed |
| Pack preview and canonical parity | 62 passed |
| CHT parsing/conflict evidence | 48 passed |
| Classic POKE | 15 passed |
| Core reconciliation | 10 passed |
| GUI persisted review | 27 passed |
| Full core library | 10,353 passed, 0 failed, 3 ignored |
| Core integration tests (19 harnesses) | 147 passed, 0 failed |
| Workspace excluding GUI/CLI, including doctests | 10,500 passed, 0 failed, 3 ignored |
| Full GUI library, two test threads | 3,160 passed, 2 failed, 3 ignored |
| Offline locked workspace check | Passed |
| Release workspace build | Passed |
| Workspace formatting check / diff check / scope guard | Passed |

The two GUI failures were rerun individually on both the repaired candidate
and a pristine detached worktree at the authoritative base. Their assertions
match:

- `convert_discs_home_card_lands_on_the_first_class_disc_conversion_page`:
  missing rendered `Source folder:` text.
- `adapter_routing_is_platform_authoritative`: actual `Unsupported`, expected
  `RetroArch`.

No unrelated GUI fixes were made. Complete validation output is retained in
`/tmp/emuwiz-cheat-four-logs`; build artifacts use isolated target directories.

Five GUI compiler warnings remain unchanged (one unused import and four
unused helpers/variants). No launch execution or production runtime isolation
is introduced. The repair is ready for focused re-review, with the two
confirmed baseline GUI failures explicitly retained. Authoritative main and
the original candidate worktree were not modified by this task; nothing was
pushed or promoted.
