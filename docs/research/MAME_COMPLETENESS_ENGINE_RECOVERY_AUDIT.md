# MAME Completeness Engine Recovery / Integration Audit

Date: 2026-09-15

Authoritative main worktree: `/home/davedap/emuwiz-main-release-fix`

Preserved worktree: `/tmp/emuwiz-mame-completeness-engine`

Main audit baseline: `63fef06c86cfcd7294447ed248197e3b21426a92`

Preserved unique commits: `931798a` and `6cb0971`

This is a research-only audit. No Rust production code or GUI file was
modified, and the preserved worktree was not deleted or rewritten.

## 1. Executive Summary

The preserved feature is not safe to cherry-pick as a whole. It adds 4,491
lines across a second authority model, a second evidence scanner, a second
dependency graph, a second completeness evaluator, an impact report, CLI
wiring, and tests. Current main has subsequently grown a typed arcade
architecture that already covers the production questions of set
compatibility, dependencies, partial observation, readiness, provenance, and
MAME/FBNeo recommendation.

The preserved engine still contains real, potentially useful ideas:

- explicit evaluation of all three MAME packing modes when mode is unknown;
- a richer dependency-aware collection result with stable reason codes and
  expected locations;
- authority-to-authority impact reporting, especially newly introduced CHD
  and BIOS requirements;
- a broad synthetic test matrix for clone, BIOS, device, sample, CHD, software
  list, cycle, malformed-input, and partial-scan behavior.

Those ideas should be recovered selectively, not by importing the old public
model. The recommended strategy is **C: port selected primitives/tests only**.
The first and only next slice should be a current-architecture authority
impact projection built on existing `ParsedDat` / dependency evidence, with
tests for newly introduced dependency blockers. The old XML authority reader,
scanner, graph, engine, and CLI should remain discarded unless a separately
approved product requirement establishes a missing input boundary.

The hard safety rule is satisfied by current main: partial or not-gathered
evidence becomes `Unknown` / `EvidenceUnavailable`, never `Missing`, and never
promotes a set to complete. The preserved engine also implemented this rule,
but its implementation must not be transplanted in a way that creates a third
state vocabulary.

## 2. Preserved Engine Architecture

The preserved worktree is at `6cb0971`, with `931798a` introducing the engine
and `6cb0971` adding the blocked real-collection audit document.

### `authority.rs`

This is an explicit-input XML authority importer. It accepts MAME `listxml` /
Logiqx-style machine documents and software-list documents, hashes every exact
input document, retains a MAME version when present, and rejects malformed or
ambiguous authority. Its bounded XML validation checks roots, placement,
depth, element count, byte count, flags, names, hashes, duplicate identities,
and custom entities. It never downloads DTDs or accesses configuration.

It converts authority into the preserved `Machine`, `Rom`, `Disk`, `BiosSet`,
`SoftwareList`, `Software`, `Part`, and `Requirement` model. This is a safe
parser boundary, but it is a parallel authority contract rather than a
projection of current main's `ParsedDat` and imported-source provenance.

### `model.rs`

The model contains:

- machine and software-list declarations;
- ROM and CHD declarations, including merge, BIOS, optional, and dump status;
- clone, ROM-source, sample, device, BIOS, software-list, interface, and
  software-part relationships;
- `MergeMode::{NonMerged, Split, Merged}` and explicit `UnknownMode`;
- `CompletenessState::{Complete, Incomplete, PresentButDependencyMissing,
  Ambiguous, Unverified, Unsupported}`;
- stable `ReasonCode` values, deterministic `Reason` records, per-mode results,
  collection statistics, and cache keys bound to parser schema, authority
  digests, source identity, scan generation, and options.

It is expressive, but it overlaps current main's `DatGameEntry`, `SetState`,
`DependencyState`, `DependencyOutcome`, `MameSetCompatibilityState`, and
`ReadyToPlayState`.

### `evidence.rs`

This scanner walks explicitly supplied ROM and sample roots under bounded
limits. It records exact container/member namespace, size, CRC/SHA-1, CHD
header identity, evidence strength, source provenance, scan statistics, and a
`scan_complete` flag. ZIP directory metadata is not promoted to payload proof;
CHD v5 headers are used for combined SHA-1 identity without hashing multi-GB
payloads. It supports bounded optional payload verification and avoids writes.

The snapshot is useful as a standalone diagnostic input, but current main
already has archive-member and CHD audit evidence with freshness and
attribution semantics. Reusing this scanner would create competing evidence
snapshots and completeness rules.

### `graph.rs`

The preserved graph has typed nodes for machines, BIOS sets, disks, software
lists, software, and parts. Typed edges distinguish clone hierarchy, ROM
source, BIOS provider/variant, devices, samples, disks, software-list
references, containment, and requirements. It keeps parent-to-child,
BIOS-dependent, device-dependent, and list-to-machine reverse indexes. It
records malformed references and uses iterative strongly connected component
analysis / bounded traversal for cycles and long chains.

This is a useful explanatory graph, but current main already has a typed
`dat::dependency::DependencyGraph` and resolver with separate dependency kinds
and fail-closed outcomes.

### `engine.rs`

`CompletenessEngine` evaluates a machine, software item, or complete authority
collection. It evaluates non-merged, split, and merged alternatives; resolves
ROM borrowing and archive layout; selects or refuses to guess BIOS variants;
walks transitive devices; handles samples and CHD parent identity; evaluates
software-list media against caller-supplied currently-ready machines; memoizes
software media and compatibility; and produces deterministic reasons and
collection statistics.

It deliberately separates storage presence, completeness, and launchability.
It treats positive evidence as valid under a partial scan but turns negative
absence into evidence-unavailable. It does not authorize launches or mutate
collections.

### `impact.rs`

`refresh_impact(old, new)` compares two immutable authority snapshots and their
derived graphs. It reports changed machines/software, added/removed edges,
new BIOS edges, new non-optional CHD requirements, software-list changes, and
new graph issues. This is the clearest genuinely reusable product concept in
the preserved worktree.

### `tests.rs`

The preserved suite is approximately 800 lines and has 39 focused tests. It
uses synthetic XML and evidence only. It covers layout modes, split/merged
clones, parent archive/member distinctions, transitive BIOS and device media,
selected and ambiguous BIOS, required/optional CHDs, CHD parent scope,
samples, software-list compatibility and cycles, partial scans, unknown mode,
authority refresh impact, parser limits/entities/duplicates, bounded scanning,
long cycles, metadata retention, malformed hierarchy, stale parent evidence,
and physical software-media sizing.

### CLI wiring

The preserved `archivefs-cli/src/mame.rs` adds explicit-input commands:

- `mame audit` for a collection or selected set;
- `mame explain` for one machine or `list:software`;
- `mame impact --old ... --new ...` for authority refresh comparison.

The CLI emits JSON and accepts authority paths, ROM/sample roots, merge mode,
scan limits, selected BIOS, and currently-ready machines. It does not mutate
files. It is a diagnostic prototype, not an integration with current main's
catalogue, source generation, emulator readiness, or command authorization.

### Research document

`MAME_COMPLETENESS_ENGINE.md` records the original contract, architecture,
fail-closed semantics, authority provenance, graph, limits, tests, CLI, and
known adapter gaps. It explicitly says the feature does not change existing
DAT evaluation, launch authorization, recovery, collection files, or GUI.

`MAME_REAL_COLLECTION_AUDIT.md` records why a real collection audit was
blocked; its findings are summarized in section 10 below.

## 3. Current Main Equivalents

| Preserved concern | Current main equivalent | Classification |
|---|---|---|
| MAME set compatibility | `arcade_mame_compatibility::{MameSetExpectation, MameSetCompatibility, audit_mame_set}` over imported `ParsedDat` and observed evidence | A — already present for the current per-set contract |
| FBNeo compatibility | `arcade_fbneo_compatibility` | A — already present |
| Clone / parent / ROM-source dependencies | `dat::dependency::{graph,resolve}` with distinct `ParentSet`, `RomSource`, and merged-member outcomes | A for the production DAT pipeline |
| BIOS and transitive device relationships | `dat::dependency::resolve` and MAME dependency evidence; downgrade-only application | A, with an intentional runtime-BIOS-selection boundary |
| CHD identity and parent dependencies | `disk_audit`, CHD header identity, `ChdParent` outcomes | A for the existing audit path |
| Samples | Existing DAT dependency vocabulary and resolver | A for current DAT-backed dependency resolution |
| Partial observation | `ObservedEvidenceCompleteness`, `EvidenceUnavailable`, MAME/FBNeo `Unknown` states, orchestration tests | A — current main includes the corrected rule |
| Unsupported / ambiguous states | `MameSetCompatibilityState`, `FbNeoSetCompatibilityState`, dependency outcomes, readiness states | A — vocabulary differs but the safety behavior is present |
| Readiness / launchability | `ReadyToPlayState`, launch preflight and emulator-specific planners | A for current launch boundary |
| Provenance | `InstalledMameEvidence`, imported artifact SHA-256, source paths, parser/schema versions, timestamps, collection provenance flags | A/B — present, but the preserved authority cache key is broader |
| Full authority enumeration and all packing-mode alternatives | No equivalent current collection evaluator | C — genuinely missing as a product projection |
| Explicit dependency graph reverse impact | Current graph resolves dependencies but does not expose the preserved authority-refresh impact report | C — narrowly missing |
| Standalone MAME listxml/software-list authority reader | Existing importers parse the relevant sources, but not under the preserved standalone contract | D/B — largely duplicate, with some validation differences |
| Standalone bounded ROM-root scanner | Existing archive/member/CHD audit paths | D — duplicate evidence boundary |
| `mame audit/explain/impact` CLI | No current equivalent command | C for diagnostics, but product value is unproven until input adapters exist |

The important distinction is that current main's compatibility projection is
not a whole-collection completeness engine. It intentionally consumes evidence
already gathered by the catalogue/audit pipeline and keeps launch authorization
separate.

## 4. Unique Remaining Capabilities

The following capabilities are not fully represented by current main:

1. **Mode alternatives without guessing.** The preserved engine can evaluate
   non-merged, split, and merged layouts simultaneously when mode is unknown,
   and report each alternative. Current main has layout/dependency evidence but
   no equivalent collection-wide alternative report.

2. **Authority-wide expected inventory statistics.** The preserved result can
   enumerate every machine and software item in supplied authority and report
   complete, launch-complete, incomplete, dependency-incomplete, BIOS-gap,
   parent/clone-gap, device-gap, CHD-gap, sample-gap, ambiguous, unverified,
   and unsupported totals. Current main reports selected-set compatibility and
   persisted audit projections, not this standalone authority denominator.

3. **Authority refresh impact.** The preserved `AuthorityImpact` identifies
   newly introduced dependencies and graph faults between exact authority
   snapshots. Current main has no equivalent typed report.

4. **Software-list launchability as a collection query.** The preserved engine
   relates software parts/interfaces/filters to explicitly ready machines. Main
   has software-list identity and dependency support, but not this same
   authority-wide launchable-total projection.

5. **Rich expected locations in reasons.** Current mismatch and dependency
   records explain failures, but the preserved evaluator systematically reports
   layout-specific expected locations for all alternatives.

These are useful only if attached to current main's evidence and state
vocabulary. They do not justify a parallel `mame_completeness` model.

## 5. Obsolete/Duplicate Capabilities

The following preserved pieces should not be recovered wholesale:

- `Authority`, `Machine`, `Rom`, `Disk`, `Software`, and related parser types:
  duplicate current DAT/imported-source models and would split authority.
- The preserved ROM-root scanner: duplicate current archive-member attribution,
  disk audit, freshness, source-generation, and CHD evidence mechanisms.
- The preserved graph as a second resolver: current main already has a typed
  graph and resolver, and duplicate resolution risks conflicting clone/BIOS/
  device/CHD semantics.
- The preserved `CompletenessState` and reason vocabulary: overlapping but not
  interchangeable with current set/dependency/compatibility/readiness states.
- Preserved CLI wiring: useful as a product sketch, but it bypasses current
  catalogue inputs and has no real-collection validation gate.
- Any assumption that a ZIP directory CRC is payload verification, that a
  configured MAME root proves a collection, or that filename layout proves
  merge mode.

The preserved engine's explicit runtime BIOS-selection limitation is not
obsolete; current main has the same boundary and should keep it visible rather
than silently claiming launchability from storage completeness.

## 6. Partial-Observation Safety Review

The preserved engine **predates the current main fix in chronology**, but its
logic already contains the correct asymmetry. `EvidenceIndex` carries
`scan_complete`; its absence helper returns `Missing` only for a complete scan
and `EvidenceUnavailable` otherwise. The test
`partial_scan_never_asserts_absence` verifies that a missing parent under a
partial scan does not become `ParentArchiveMissing`. Positive verification is
left valid under partial observation.

The preserved `ObservedEvidenceCompleteness` rule in current main is stronger
at the compatibility boundary: `Partial`, `NotGathered`, and `Unknown` prevent
an otherwise compatible result from becoming proven and return `Unknown` with
`EvidenceUnavailable`. The orchestration test
`partial_observation_is_not_promoted_by_orchestration` protects that behavior.

Therefore:

- the preserved engine does not itself regress the rule;
- transplanting its state types or scanner would risk regression by creating a
  second absence/proof path;
- its partial-scan tests are valuable regression material and should be mapped
  onto current `ObservedEvidenceCompleteness`, `EvidenceUnavailable`, and
  recommendation behavior;
- no recovered code may convert partial absence to current `Missing`,
  `Incompatible`, or `Complete`.

## 7. Authority Model Review

| Preserved concept | Current status | Decision |
|---|---|---|
| Exact authority document SHA-256 list | Current imported MAME/FBNeo sources retain artifact hashes | Reuse/adapt through existing imported-source provenance |
| MAME version/build | `InstalledMameEvidence.reported_version` and imported metadata | Already present; preserve explicit capture, never infer from filename |
| Parser schema version | Current MAME/FBNeo compatibility schema versions and DAT parser provenance | Already present; use existing schema boundary |
| Separate software-list authority documents | Current Logiqx software-list import/index path | Adapt only where a current consumer lacks a version/hash binding |
| Explicit input-only parser limits and entity refusal | Current parsers and bounded readers have safety limits, but behavior is not byte-for-byte identical | Reuse principles, do not duplicate importer |
| Merge-mode provenance | Current main preserves dependency/layout facts but does not have the preserved `ModeEvidence` alternatives contract | Reuse the explicit-provenance principle if mode analysis is added |
| Scan generation / source identity | Current audit snapshots and observed evidence already bind freshness/source | Already present; stronger integration point than preserved standalone generation |
| Authority cache key | Current structures have source/parser/hash provenance, but no one combined completeness cache key | Adapt selectively if a collection projection is implemented |
| Authority refresh provenance | No current equivalent report | Reuse as a new projection, not a new authority model |

The authority lesson worth recovering is not the old structs. It is the rule
that every completeness or impact result must name the exact authority artifact,
parser schema, source observation generation, and relevant mode/readiness
assumptions.

## 8. Dependency Graph Review

The preserved graph is materially richer as an explanatory graph: it has
machine, BIOS-set, disk, software-list, software, and part nodes; typed edges;
reverse indexes; graph issues; and bounded cycle handling. It can answer
"which clones depend on this BIOS/device/parent?" directly.

Current main's `dat::dependency::DependencyGraph` already provides the critical
correctness semantics: unique/duplicate/absent set resolution, separate
dependency kinds, scoped member identity, transitive devices, BIOS, samples,
merged ROM/disk, CHD parent identity, cycles, contradictions, unsupported
states, and downgrade-only application to storage state. That is the graph that
must remain authoritative for current production verdicts.

The preserved graph therefore has one useful missing capability: **stable
reverse impact queries over the current dependency graph**. Its node/edge model
should not be ported as-is. Instead, derive impact from current graph facts or
add a current-architecture reverse-index projection with the existing
`DependencyKind`, `DependencyTarget`, and `DependencyOutcome` types.

Cycle handling is already present in current main's resolver tests; no old graph
implementation is needed. The preserved graph's distinction between a CHD
header parent and DAT disk merge is a useful test/design rule and should remain
explicit in current code.

## 9. Impact Analysis Review

In the preserved engine, “impact” means authority drift, not runtime
performance and not a count of missing files. Given old and new immutable
authority snapshots, it identifies:

- machines added, removed, or changed;
- software and software-list changes;
- added/removed dependency edges;
- newly introduced BIOS edges;
- newly required non-optional CHDs;
- newly visible graph issues;
- changed authority digests.

This can improve diagnostics by explaining why a previously complete or
cacheable result must be reconsidered. It can improve Problems & Repair by
showing that an authority refresh introduced a new required parent/BIOS/CHD,
without proposing a destructive repair. It can improve Arcade Manager by
ranking collection-wide consequences of a DAT refresh. It can improve
Ready-to-Play only indirectly: a set remains not-ready unless current evidence
proves the newly required dependency, and the impact report can explain the
changed requirement.

It should not be connected directly to launch or repair actions. The result is
diagnostic provenance and invalidation evidence, not authorization.

## 10. Real Collection Audit Findings

Commit `6cb0971` records a **blocked** audit dated 2026-09-13. It did not run a
version-matched MAME completeness audit.

The inspected active configuration selected `/mnt/usbdrive/games`, with scan
192 containing 68,853 unchanged archive observations, but no Arcade platform
assignments and no `mame/` or `arcade/` namespace. Two stale Arcade rows were
historical FBNeo sample paths and their current files were missing. A separate
`/mnt/local/games/roms/arcade` directory contained 17 ZIPs and 152 members,
zero CHDs, and no catalogue rows; its `manifest.json` described a generator,
not a MAME release.

No native MAME executable, configured MAME hash directory, MAME profile, or
matching MAME XML/DAT authority was found. Existing MAME-named DAT entries
were unrelated authorities. The bounded ZIP probe was read-only and fast, but
CRC directory metadata was not promoted to payload proof. Merge mode remained
unknown, and no collection denominator or dependency result was invented.

The blockers were missing collection/authority/version inputs and stale
catalogue scope, not proven missing BIOS/game dependencies. The document says
the required resume gate is an explicitly identified collection root, matching
machine/software-list authority, version provenance, source binding, and then
adapters/cancellation before a real audit.

Those blockers remain unresolved for this audit environment as far as the
preserved record establishes. No new environment probe or giant collection
scan was run here, per instruction. Current main's architecture does not
magically supply the absent MAME executable, matching authority, or current
Arcade source assignment.

## 11. Test Coverage Comparison

The preserved suite's useful groups compare as follows:

| Preserved tests | Current main assessment | Decision |
|---|---|---|
| Split/merged/non-merged clone layout and scoped parent-member borrowing | Current dependency tests cover clone/merge/member identity, but not the old all-mode collection projection | Port selected regression cases if mode projection is implemented |
| Parent archive present but required member absent | Current dependency tests distinguish missing dependency/member outcomes | Already covered; compare reason wording only if needed |
| BIOS variants: selected variant, ambiguous selection, transitive device ROMs | Current resolver covers BIOS/device safety; runtime selection remains intentionally unmodelled | Already covered semantically; preserve explicit boundary test |
| Required/optional CHD and delta-parent rules | Current disk audit/dependency tests cover CHD identity and parent semantics | Already covered; preserved cases are duplicate/stronger examples |
| Samples and sample namespace | Current dependency vocabulary/resolver covers samples | Already covered |
| Software-list interface/filter/part requirements and currently-ready machine | Current main has identity/dependency pieces but not the same authority-wide software launchability evaluator | Stronger than current; worth porting only with a product consumer |
| Partial scan never asserts absence | Current `Unknown` / `EvidenceUnavailable` tests cover the corrected rule | Duplicate safety intent; port as regression if gaps are found |
| Unknown mode computes all alternatives | Current main lacks this exact projection | Worth porting at projection level, not old engine types |
| Malformed XML, duplicate identities, custom entities, limits | Current importers have their own bounded/parser tests | Mostly duplicate; do not create a second parser contract |
| Bounded scanner, no writes, no payload hashing for CHD | Current audit pipeline has bounded/read-only evidence tests | Duplicate safety boundary |
| Authority refresh emits new BIOS/CHD impact | No current equivalent | Stronger than current; port as the next slice |
| Long cycles and deterministic reason order | Current dependency tests cover cycles and deterministic rollups | Already covered semantically |
| Physical software-media sizing and loading directives | Current software-list support is not equivalent | Potentially useful, but out of scope for the next narrow slice |

The preserved suite is valuable as a requirements catalogue, not as a drop-in
test file. Every port should first identify the current type and state that it
protects; tests that instantiate the old `Authority` or `EvidenceIndex` would
cement the duplicate architecture.

## 12. CLI Assessment

**Decision: superseded as an implementation, useful as a future diagnostic
shape.**

The old CLI commands are explicit, read-only, JSON-producing, and have a clear
operator contract. That makes them a useful design reference for a future
collection completeness diagnostic. However, the current CLI has no validated
adapter that binds the command to the current catalogue/source generation,
fresh archive-member evidence, imported MAME authority, emulator readiness,
and stale-result invalidation. Running the old command against arbitrary roots
would produce a parallel answer whose provenance is not the same as current
main's product answer.

Do not recover the CLI wiring until the current-architecture projection and
input adapter exist. No CLI code was added in this audit.

## 13. Interaction with Arcade Recommendation

Current `ArcadeEmulatorRecommendation` consumes MAME/FBNeo compatibility states
and deliberately treats `CompatibleWithWarnings` as usable while preserving
warnings and treating `Unknown` as uncertainty. It does not currently accept
the preserved collection completeness result.

The useful future interaction is a two-level explanation:

```text
MAME set compatibility proven
        + dependency/completeness projection says required BIOS/parent/device/CHD
          is not proven
        -> MAME compatible in the observed sense, but not ready-to-play
```

That must not be implemented by converting dependency-incomplete into
`Incompatible`, and must not let recommendation choose an emulator solely from
partial absence. A future adapter could pass a separate readiness/completeness
warning into recommendation or into its caller-facing explanation while
preserving the existing compatibility state and `Unknown` rule.

This audit does not change recommendation code.

## 14. Integration Decision

**Choose C: port selected primitives/tests only.**

Reasons:

- Whole cherry-pick would add a parallel authority/evidence/graph/model stack
  beside current main and would bypass current source-generation and freshness
  boundaries.
- Discarding everything would lose the only current design for authority
  refresh impact and the all-layout alternative projection.
- A redesign is unnecessary for the immediate recovery: current main already
  has the dependency semantics and partial-observation safety needed to host a
  small projection.
- The old real-audit record is blocked, so there is no evidence that the old
  CLI or scanner is production-ready against the current environment.

Selected recovery should be projection-first and authority-bound. It should
reuse current DAT/imported-source identities, current dependency graph facts,
current `DependencyOutcome` / `EvidenceUnavailable` states, and current
readiness evidence. It must not introduce `mame_completeness` as a second
production verdict namespace.

## 15. Exact Next Implementation Slice

Port **one** slice: a read-only `MameAuthorityImpact` projection over two
current, explicitly imported MAME authority snapshots, reporting only:

- old/new artifact identity and parser schema;
- added/removed/changed machine or set declarations;
- added/removed dependency relationships using current dependency kinds;
- newly required non-optional CHD, BIOS, device, or parent relationships;
- newly introduced malformed/ambiguous graph facts;
- deterministic reasons that a prior compatibility/completeness cache must be
  invalidated.

The slice must:

1. consume current `ParsedDat` / imported-source types rather than the
   preserved `Authority` model;
2. reuse current dependency graph and identity semantics rather than adding a
   second graph;
3. remain read-only and diagnostic;
4. bind output to exact old/new authority hashes and source provenance;
5. include regression tests for added BIOS/parent/device/CHD relationships,
   unchanged authority, malformed references, deterministic ordering, and the
   rule that partial/not-gathered evidence remains `Unknown` /
   `EvidenceUnavailable` rather than `Missing`;
6. have no GUI, launch, repair mutation, or old CLI wiring in that slice.

After that slice, reassess whether a separate current-architecture
all-layout completeness projection is justified by a real authority/source
input. Do not port the 4,491-line engine, its scanner, or its CLI before that
decision gate.

MAME COMPLETENESS RECOVERY AUDIT READY
