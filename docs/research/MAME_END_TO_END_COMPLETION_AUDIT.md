# MAME End-to-End Completion Audit

## Scope and evidence

This is a read-only audit of the MAME-specific core and GUI projections at
`33ecab378c54cfdfa4dd0303e1008b114b920878`. No collection scan, SHA-1 index
rebuild, ROM mutation, or GUI production change was performed.

The audit uses the existing catalogue, evidence-cache, repair, reconstruction,
playing-library, launch, cheat, and GUI code. The previously measured corpus
results (including exact identities already present elsewhere in the library)
are treated as external audit evidence; the product does not automatically
rescan that corpus as part of these surfaces.

The current implementation has two related but not yet unified workflows:

* the newer current-catalogue collection analyser, internal repair planner, and
  exploded-directory apply path; and
* the older verified MAME 0.174 merged-reconstruction workflow in Organisation,
  which stages a new merged ZIP and publishes it transactionally.

That distinction is the main end-to-end finding.

## CURRENT MAME ARCHITECTURE

| Surface | Status | Finding |
| --- | --- | --- |
| Catalogue parsing and MAME set model | COMPLETE | Parsed MAME data carries set names, clone/parent and ROM identities, machine flags, dependencies, dump status, and disks. |
| Physical member evidence | COMPLETE / PARTIAL | Version-bound, SHA-1/CRC-backed evidence is cached and refreshable; the caller must explicitly run the refresh and the evidence is not itself a complete live health session. |
| Collection health analysis | PARTIAL | Merged, split, non-merged, exploded, mixed, and unknown styles plus Good/Bad/Unknown health are represented. The user-facing report does not expose every preservation state as one coherent health vocabulary. |
| Internal repair planning | COMPLETE for planning | Exact SHA-1 matches, provenance, relationship, destination, ambiguity, BAD_DUMP, NO_DUMP, absence, and wrong-content cases are typed. It is read-only. |
| Internal repair apply | COMPLETE for a narrow scope | Safe exact-byte operations can materialise missing members in exploded directories through the shared transaction journal. Archives are deliberately refused. |
| Merged reconstruction | COMPLETE for its bounded workflow | A family plan can stage and verify a new ZIP, then publish through the journal with undo. It is a separate path from the internal repair planner. |
| Playing-library projection | COMPLETE for read-only selection | Parent/clone-aware deterministic selection and support-set retention exist. It is not yet a post-repair launch/publication pipeline. |
| MAME launch planning | PARTIAL | A strict command planner exists, but it consumes a separate canonical identity/set-resolution model and is not automatically refreshed from repair results. |
| History and undo | PARTIAL | Reconstruction and internal repair use journaled transactions; discoverability and cross-linking from health, repair, playing library, and launch are incomplete. |
| GUI journey | DUPLICATED / PARTIAL | Collection Health, Organisation's “Fix my MAME library”, playing-library preview, and Game Details cheat surfaces are separate projections with no single guided journey. |

## IDENTITY

The authoritative identity evidence is the selected MAME catalogue/DAT and its
machine shortname, bound to a SHA-256 catalogue identity/version. Member
identity is checksum-led: SHA-1 is preferred and CRC32 is a fallback in the
reconstruction join. Directory names and member filenames are candidate labels,
not proof of ROM identity.

The implementation therefore correctly avoids filename-only promotion for
repair and reconstruction. The evidence cache also records the physical path,
member name, size, modification time, observed hashes, catalogue source, and
whether the row is actionable.

The remaining weakness is not the member hash rule; it is the absence of one
shared binding object consumed by all later stages. The analyser, internal
repair planner, merged reconstruction, playing-library planner, and launch
planner each receive related evidence through different request types. A
successful filesystem repair does not automatically produce a new authoritative
`SetResolution` or launch verdict.

**Classification: COMPLETE for member identity; PARTIAL for end-to-end identity
continuity.**

## SET TOPOLOGY

The DAT model and join evidence distinguish exact sets, parents, clones, BIOS,
devices, mechanical machines, and non-runnable machines. The repair planner
represents game-specific, parent-shared, BIOS, device, PLD/GAL/PAL, CHD, and
unknown relationships. The playing-library planner groups parent plus
`clone_of` families, preserves meaningful regional/control-panel variants, and
retains support sets.

The topology is not entirely one model: reconstruction uses an
`ArcadeJoinEvidence` family and the playing library derives its own family
selection from parsed games and observed set names. `rom_of`, device edges, and
parent-shared support are more explicit in repair/join evidence than in the
playing-library selection algorithm.

**Classification: COMPLETE for catalogue representation; PARTIAL for one shared
topology consumed by every workflow.**

## DEPENDENCIES

Dependency explanation exists at the core level. The analyser can classify
game-specific ROMs, parent-shared ROMs, BIOS ROMs, device ROMs, PLD/GAL/PAL,
CHDs, NO_DUMP, BAD_DUMP, and unknown items. Join evidence also records typed
dependency edges and whether the target is present. Launch readiness has a
separate dependency state and refuses blocked/incomplete dependency results.

The GUI contains useful novice wording: a shared device ROM, a parent/shared
dependency, and “NO_DUMP items are preservation gaps, not ordinary missing
collection files.” It does not yet provide a per-selected-set dependency tree
that links each problem to its owning parent/BIOS/device set and then directly
to a repair operation.

**Classification: PARTIAL.** The semantic data is present; the explanatory
drill-down and repair linkage are not complete.

## HEALTH STATES

Current analyser health is `Good`, `Bad`, or `Unknown`, with separate style,
update-readiness, preservation, and dependency fields. The repair planner has
more useful dispositions: safe internal repair, present-but-BAD_DUMP, NO_DUMP,
genuinely absent, ambiguous, and wrong content under the expected name.

The requested user-facing vocabulary maps as follows:

* **GOOD** — available through analyser `Good` when all required checked items
  match.
* **BEST AVAILABLE / NO GOOD DUMP KNOWN** — represented indirectly by
  `NoVerifiedDumpExists`, but not as a first-class set health result.
* **NEEDS REDUMP** — represented as a preservation status for BAD_DUMP-related
  evidence, not consistently elevated to set health.
* **BAD** — analyser `Bad` and repair failure/refusal states.
* **UNRECOGNIZED** — generally `Unknown`, missing/ambiguous identity, or an
  unresolved join; the GUI does not consistently distinguish these causes.

In particular, BAD_DUMP and NO_DUMP are typed and counted, but the health
projection can still be too coarse for a user to tell “bad dump known” from
“missing ordinary dump” without opening the detailed problem data.

**Classification: PARTIAL.** A unified health-state projection is a P1 seam and
becomes P0 for any automatic “healthy after repair” claim.

## BAD_DUMP / NO_DUMP

The repair planner correctly separates these cases. NO_DUMP is not an ordinary
repair target and is excluded from the safe internal repair count. A matching
local byte for a BAD_DUMP requirement is reported as preservation evidence and
is not described as a healthy good dump. No download or redump acquisition is
invented.

The remaining issue is result language. The internal apply result can mark a
filesystem operation `RepairedAndVerified`, while its own post-apply message
explicitly says that targeted MAME verification was not run. That wording is
safe only if interpreted as “destination bytes verified”; it must not be used
as “the set is healthy”.

**Classification: COMPLETE for refusal/counting; PARTIAL for unified health and
post-apply semantics.**

## REPAIR

The current internal repair planner is conservative and useful:

* exact SHA-1 is the content identity, even when the source filename differs;
* source selection is deterministic and duplicate candidates remain visible;
* source and destination are confined to the configured arcade root;
* symlinks/special files, traversal escapes, missing sources, stale source
  hashes, wrong destination content, and unsupported destination types refuse;
* existing correct destinations are `AlreadySatisfied`;
* default apply is an independent copy, with optional hard-link support;
* all writes go through the shared journaled transaction executor;
* partial failures attempt confined rollback; and
* undo removes only destinations created by the transaction and refuses after
  external changes.

The real executable apply scope is **exploded directory → exploded directory**.
The `Reflink` strategy is represented in the type model but was not shown as an
implemented selection path in the audited executor. The safe default therefore
remains copy; hard links are opt-in and need explicit preservation-policy
review.

**Classification: COMPLETE for the bounded exploded-directory use case; PARTIAL
for collection-wide repair.**

## RECONSTRUCTION

Merged reconstruction is a separate, stronger archive-producing path. It
requires a complete parent/clone family, version-bound persisted join evidence,
checksum-backed sources, no missing/duplicate/unresolved ownership, and no
destination collision. It can read extracted members and verified ZIP members,
stage a new ZIP under a bounded byte budget, verify member count/size/SHA-1, and
publish the staged output through the shared transaction journal.

The source archive is never opened for writing and source members are not
renamed or removed. The Organisation UI requires an explicit confirmation
phrase and exposes transaction history/undo.

This path is not a general repair replacement: it reconstructs a new merged
parent output, does not repair arbitrary split/non-merged layouts, and is not
automatically connected to the newer internal repair plan or launch re-analysis.

**Classification: COMPLETE for reviewed merged-family reconstruction; PARTIAL
for general MAME repair/reconstruction.**

## ZIP STATUS

| Operation | Status | Evidence |
| --- | --- | --- |
| Inspect ZIP members | COMPLETE | Bounded archive readers verify members and hashes; packed sources can be discovered with checksum evidence. |
| Use ZIP members as reconstruction sources | COMPLETE | Verified members may be copied into a staged output. |
| Stage a new merged ZIP | COMPLETE | `stage_reconstruction_output` creates and verifies a new archive. |
| Publish a new merged ZIP | COMPLETE | Destination collision is refused; publication is journaled and undoable. |
| Repair a member inside an existing ZIP in place | MISSING / REFUSED | Internal repair apply requires exploded source and destination directories. |
| Transactionally rewrite an existing ZIP while preserving unrelated members/comments/metadata | MISSING | No reviewed archive-rebuild-and-publish primitive exists for this general case. |

The exact missing seam for packed repair is an archive transaction primitive:
read and verify the original archive, construct a deterministic replacement
archive preserving unrelated members according to an explicit policy, verify
the complete output, atomically publish beside a byte-identified original, and
journal exact rollback. The existence of ZIP read support must not be treated as
ZIP-member write support.

## CHD STATUS

CHD is represented in MAME dependency and coverage data and can affect set
health and launch readiness. The analyser can report CHD coverage and the
repair model can classify a CHD requirement. This is enough to explain that a
disk dependency is missing or shared.

There is no equivalent demonstrated CHD content repair/rebuild path in the
audited MAME internal apply workflow. No CHD acquisition is attempted, and a
generic ROM-member byte operation must not be presented as a CHD repair. The
launch path can remain blocked when the required disk is absent, but there is
no single GUI row that carries “required disk → parent/clone relationship →
launch blocker → safe local source/repair availability” end to end.

**Classification: PARTIAL for evidence and launch impact; PREVIEW-ONLY for
repair.**

## PLAYING LIBRARY

The read-only `MamePlayingLibraryPlan` is substantial. It supports deterministic
region order, parent/newest/English preferences, category exclusions, working
and imperfect policies, meaningful regional/control-panel variants, support-set
retention, unresolved cases, projected count, and a logical storage/savings
estimate. It never scans, hashes, copies, links, renames, or deletes the
archival collection.

It correctly avoids pretending every clone is redundant. BIOS/device support
sets are retained separately from ordinary playable selections.

The estimate is a logical sum of member sizes, not necessarily the compressed
on-disk size of a ZIP/CHD collection. More importantly, the planner uses the
analysis and DAT family view but does not itself consume the internal repair
plan to say “this selected set becomes playable after these exact repairs”. The
general playing-library publish/link machinery is a separate projection and is
not visibly joined to MAME repair results, MAME launch resolution, or MAME
history.

**Classification: COMPLETE for read-only curation; PREVIEW-ONLY for a complete
repair-to-playing-library publication journey.**

## LAUNCH

The MAME command planner is conservative and useful. It requires canonical
identity, Arcade/NeoGeo platform compatibility, a single set resolution,
complete set state, permitted dependencies, a discovered MAME executable, an
absolute ROM search path, and a matching selected archive parent. It launches
with an explicit `-rompath` and set name.

It distinguishes Ready, NeedsSetup, and Blocked rather than guessing from a
title or filename.

The gap is lifecycle integration. Internal repair verifies destination bytes
but does not invoke targeted `mame -verify` or rebuild a fresh launch verdict.
Playing-library selection likewise does not return a launch candidate tied to
the final repaired transaction. A user can therefore complete a repair and
still need to navigate to a separate identity/setup flow before launch.

**Classification: PARTIAL.** The launch primitive is safe; the handoff from
health/repair/playing-library is missing.

## HISTORY / UNDO

Merged reconstruction writes a workflow marker and parent metadata into the
shared rename journal. Organisation loads those transactions, shows state and
destination, and exposes confirmation-gated undo. Internal repair uses the
same shared transaction/history and confined rollback rules, preserving the
transaction ID and operation provenance.

The history model is transaction-oriented, not yet journey-oriented. It does
not consistently connect a transaction to the originating health problem,
selected set, repair plan digest, resulting MAME verification, playing-library
projection, and launch result in one user-readable record. The GUI health page
also has an apply-capable helper but its ordinary route is called without a
loaded plan, so history visibility depends on which controller path supplied
the plan.

**Classification: COMPLETE for transaction recovery; PARTIAL for MAME-specific
history continuity.**

## GUI JOURNEY

The intended journey is:

`MAME collection → health summary → selected problem → dependency explanation →
repair preview → apply → verify → playing-library preview → launch →
history/undo`.

Current surfaces provide pieces:

1. **Collection Health** renders summary placeholders, style/update labels,
   shared-problem wording, playing-library preview, and internal-repair
   preview when a controller supplies plans. Its normal page route calls the
   no-plan read-only view, so it does not itself load a collection or offer a
   selected-problem workflow.
2. **Organisation / Fix my MAME library** lets a user select a folder and DAT,
   refresh persisted evidence, preview a merged reconstruction, publish with an
   explicit phrase, and undo from history. It is a distinct workflow and its
   current helper is pinned to verified MAME 0.174 evidence, while the generic
   analyser is designed around the current supplied MAME catalogue.
3. **Playing Library** presents curation, but not a MAME-specific post-repair
   publish/launch handoff.
4. **Game Details / Cheats** can present native MAME cheat previews and apply
   history, but that identity path is intentionally separate from set health.

There is no dead-simple path from one selected missing dependency to an exact
source candidate, safe apply, targeted MAME verification, and launch. The
duplicate routes are valuable capabilities, but they need a controller-level
join rather than another independent page.

**Classification: DUPLICATED / PARTIAL.**

## CHEAT INTEGRATION

Native MAME cheat support is sensibly isolated from ROM repair. The adapter
binds the native XML to a MAME machine, preserves native expressions, only
recognises a narrow direct `address = value` form, retains opaque expressions,
and uses bounded parsing plus shared preview/transaction structures. GUI copy
accurately warns that native expressions may still require runtime enabling.

This is compatible with the set-identity model when the selected machine
shortname is passed explicitly. It should not be used as evidence that a ROM
set is complete or launchable, and it does not need to be redesigned as part of
the MAME journey audit.

**Classification: COMPLETE as a separate native-cheat preview/apply surface;
PARTIAL only in cross-linking it from MAME Game Details to collection health.**

## P0 GAPS

P0 means the gap prevents a safe generic end-to-end restore/repair/launch claim,
not merely that the UI could be nicer.

1. **No single authoritative post-repair verification handoff.** Internal
   repair proves destination SHA-1/size but does not run targeted MAME
   verification or update the launch `SetResolution`. The product must not
   claim “fixed” or “launchable” from a file copy alone.
2. **Packed ZIP repair is not executable.** Existing ZIP read/stage support
   does not authorize in-place member rewrite. A packed collection therefore
   has a preview/reconstruction path, not generic internal repair.
3. **MAME GUI workflows are not joined.** The live health route does not load
   the analyser/repair plans, while Organisation uses a separate merged
   reconstruction controller and a pinned 0.174 verified-DAT contract. A user
   cannot reliably complete the requested collection-to-launch journey from
   one selected problem.
4. **No unified repair-to-launch identity receipt.** Catalogue digest, set
   topology, repair plan digest, resulting destination hashes, targeted MAME
   verification, and launch candidate are not one typed durable object.
5. **Health status can understate preservation distinctions.** BAD_DUMP,
   NO_DUMP, unknown, and ordinary missing are typed in core data but are not
   always presented as distinct set-level outcomes. Any automation that maps
   coarse `Good`/`Bad` to “healthy” is unsafe until this is resolved.

## P1 GAPS

1. Add a dependency explanation view per selected set with owner relationship,
   source evidence, preservation status, and exact repair disposition.
2. Join MAME playing-library selection to repair projections so each selected
   representative shows current health, repairs needed, required support sets,
   CHD status, and launch readiness.
3. Add a bounded archive-rewrite primitive only if the project decides packed
   ZIP mutation is worth its preservation/rollback cost; otherwise make the
   refusal a first-class, discoverable outcome.
4. Replace the fixed Organisation MAME-0.174 contract with a current-catalogue
   controller or clearly label it as a legacy reconstruction workflow.
5. Expose actual loaded catalogue version/root/inspection time instead of the
   current Collection Health placeholders.
6. Record targeted MAME verification output in transaction history and expose
   “filesystem repaired; set still incomplete” distinctly.
7. Improve storage estimates by separating logical member bytes from packed
   archive/CHD physical estimates.
8. Add CHD-specific launch and repair explanation without implying CHD
   acquisition.

## QUICK WINS

* Rename internal-apply success language in the GUI/history to “exact
  destination bytes verified; MAME set verification pending” until a targeted
  verifier is actually run.
* Make the Collection Health controller accept and display the existing loaded
  analyser, playing-library, and repair plans instead of rendering the default
  placeholder route.
* Show the selected catalogue digest/version, root, and inspection timestamp in
  every repair/reconstruction preview.
* Add a visible “packed ZIP repair unsupported; use merged reconstruction or
  extract first” explanation beside refused operations.
* Link a repaired set to a refresh/re-analysis action and make launch remain
  blocked until the refreshed dependency/set verdict is complete.
* Present BAD_DUMP and NO_DUMP as preservation outcomes, not ordinary repair
  counts, in the first-level health summary.
* Keep MAME cheat status in Game Details but show the exact set shortname used
  for binding.

## IMPLEMENTATION SEAMS

The smallest reusable seams for a finished journey are:

* `MameSetIdentityReceipt`: catalogue SHA-256/version, set shortname,
  parent/clone topology, root, and evidence generation.
* `MameDependencyExplanation`: owner set, relationship kind, member/disk,
  expected identity, observed candidates, preservation status, and launch
  impact.
* `MameRepairVerification`: transaction ID, plan digest, destination hashes,
  targeted `mame -verify` result, remaining missing/bad/NO_DUMP dependencies,
  and a conservative final health state.
* `MameLaunchReadiness`: a refreshed `SetResolution` linked to the same
  catalogue/evidence receipt and repair verification.
* `MamePlayingLibraryProjection`: selected representative plus support sets,
  repair verification, CHD state, and launch readiness.
* `MameArchiveRepairPlan`: an explicit archive destination classification and,
  if ever implemented, complete archive preimage/output/rollback metadata.
* `MameJourneyHistoryEntry`: links health report, repair/reconstruction
  transaction, verification, playing-library choice, and launch attempt while
  retaining the existing shared transaction receipt.

These should compose existing types rather than replace the safe transaction,
DAT, archive, or cheat implementations.

## TEST PLAN

The following synthetic fixtures should be added to the eventual controller and
verification layer:

1. complete parent;
2. complete clone;
3. missing parent member;
4. missing BIOS;
5. missing device;
6. BAD_DUMP;
7. NO_DUMP;
8. exact SHA-1 repair source;
9. duplicate source candidates;
10. wrong-hash candidate;
11. exploded reconstruction;
12. packed ZIP preview/refusal;
13. missing CHD;
14. clone/parent playing-library choice;
15. successful repair and targeted MAME verification;
16. repair leaves another dependency failing;
17. rollback after repair;
18. external modification after repair;
19. stale catalogue/evidence digest;
20. launch blocked before refresh and ready after refreshed complete evidence.

Each apply test must verify source bytes unchanged, destination hash, journal
receipt, refusal on collision/symlink/traversal, and exact undo behavior. Each
GUI test should assert the plain-language distinction between missing,
BAD_DUMP, NO_DUMP, unsupported packed repair, and “verification still
required”.

## IMPLEMENTATION ORDER

1. Define the shared MAME identity/evidence receipt without changing existing
   parser semantics.
2. Build the dependency explanation projection from current DAT/join/repair
   types.
3. Add targeted MAME verification as a bounded, non-mutating post-transaction
   step and record its conservative result.
4. Reconnect Collection Health to loaded analyser/repair/playing-library plans
   and expose the same receipt in the preview.
5. Connect refreshed verification to MAME launch readiness.
6. Connect verified selection and support-set state to the playing-library
   preview/publication seam.
7. Unify history presentation while retaining existing transaction/undo records.
8. Only then decide whether a separate transactional ZIP-member rewrite
   primitive is warranted; otherwise retain and improve the explicit refusal.
9. Add CHD-specific explanation and the complete end-to-end GUI tests.

## Final assessment

MAME is not missing core preservation primitives. It already has strong
checksum evidence, conservative read-only planning, bounded exploded-directory
repair, verified merged reconstruction, journaled rollback, deterministic
playing-library selection, and strict launch planning. It feels unfinished
because those capabilities are split across two catalogue workflows and are
not joined by a post-repair verifier, a unified identity receipt, or one GUI
journey.
