# Library Organisation & Export Parity Audit

> **Recovered historical research — status against current main (`b66422c2`).**
> Source: branch `research/organisation-export-parity` at `14e01d5c`. Recovered unchanged below this block except where marked `[refreshed]`.
> - **Historical capability/parity research; the current GUI v2 and code are authoritative.** Written against main `41c9a645` (2026-09-21), before the GUI v2 rebuild, the Mega Pass and the RomM native browser.
> - The **"GUI and route parity"** and **"Novice workflow model"** sections, and any build-order or implementation-sequencing suggestions, are **historical** and describe the older GUI; do not treat them as the current plan.
> - Still useful: the capability matrix, backend inventory (organisation, DAT rename apply, Playing Library/1G1R), RomM and ES-DE status, other frontend targets, safety audit and the sets/multi-disc/support-file notes — each should be rechecked against current code before use.


**Status:** research only; no production Rust or GUI changes.
**Inspection date:** 2026-09-21
**Authority:** `/home/davedap/emuwiz-main-release-fix` at
`41c9a645464a3fec9fa68cad7de78cfadd19cf12`

## Scope and equivalent-audit check

No single equivalent organisation/export parity audit was present in the
authoritative tree. The following focused documents already exist and were
used as evidence rather than duplicated:

- `docs/ESDE_PARITY_AUDIT.md`
- `docs/ESDE_FINAL_PARITY_PLAN.md`
- `docs/ESDE_FINAL_GAP_DECISIONS.md`
- `docs/design/CANONICAL_ROM_ORGANISATION_STAGE1.md`
- `docs/design/DAT_RENAME_PLANNING_STAGE1.md`
- `docs/design/DAT_RENAME_APPLY_STAGE1.md`
- `docs/RETROARCH_PLAYLISTS.md`
- `docs/research/MANAGED_EMULATOR_INSTALL_PROVENANCE_AUDIT.md`

This audit maps the complete set of organisation/export paths currently
present, including the distinction between the RomM identity source and the
RomM library projection. Playing Library/1G1R is treated as complete and is
not redesigned.

## Executive result

The current project has two mature mutation families:

1. **DAT rename/organisation transactions**: verified rename planning,
   canonical platform folders, in-place rename, real-file move, symlink-object
   organisation, linked-library creation, durable journals, stale-plan checks,
   no-clobber preflight, rollback, recovery, and history.
2. **Playing Library projections**: verified 1G1R election with companion/set
   preservation, then journaled symlink publication to Generic, RomM,
   ES-DE/RetroDECK-shaped destinations, with ES-DE gamelist publication and
   recovery.

The principal parity gap is not missing backend capability for those two
families. It is that the native v2 surface must expose the existing actions
with clear source/destination/preview/apply/history semantics, while several
other things users may call “export” are only read-only evidence:

- RomM server integration is a read-only identity/cache provider; local RomM
  library projection is a separate explicit Playing Library destination.
- ES-DE gamelist publication is implemented, but only as a follow-up to an
  existing Playing Library plan and selected ES-DE profile.
- RetroArch playlists are read as identity/core evidence; EmuWiz does not
  create or rewrite `.lpl` playlists.
- LaunchBox support is local metadata/media discovery/import evidence, not a
  LaunchBox export writer.
- Pegasus export and arbitrary EmulationStation-variant export are absent.
- Media-set inspection is read-only and explicitly does not execute swaps or
  create playlists.

## Capability matrix

| Workflow | Backend status | Mutates? | Transactional? | Rollback? | Frontend target | v2 native? | Legacy/route | Missing piece | Recommended future action |
|---|---|---:|---:|---:|---|---:|---|---|---|
| DAT canonical rename in place | COMPLETE | Yes | Yes | Yes | EmuWiz canonical library | Partial/native backend | DAT Sources and Quick Rename | Native Organisation page wiring/wording parity | Expose existing plan/apply/recovery in v2 |
| Move verified real files into platform folders | COMPLETE | Yes | Yes | Yes | EmuWiz canonical library | Partial/native backend | Library Organisation | Same as above | Keep as explicit “Move real files” mode |
| Reorganise existing symlink objects | COMPLETE | Yes, link object only | Yes | Yes | EmuWiz canonical library | Advanced/legacy | Library Organisation Advanced | Novice UI should not imply target mutation | Keep Advanced-only and document clearly |
| Build linked Playing Library | COMPLETE | Creates symlinks only | Yes | Yes | Generic/Playing Library | Existing route | Library Organisation → Playing Library | None backend-side | Preserve as primary clean-library workflow |
| Playing Library 1G1R election | COMPLETE | No during plan | N/A | N/A | Generic/RomM/ES-DE/RetroDECK | Existing route | Playing Library page | None in scope | Do not duplicate election logic |
| RomM identity import/cache | COMPLETE read-only | Cache/config only | Atomic cache publication | Cache replacement is safe; not ROM rollback | RomM provider | Provider route | CLI `identity source romm`; RomM GUI | No local library organisation by itself | Keep provider and projection separate |
| RomM Playing Library projection | COMPLETE | Creates destination symlinks | Shared rename transaction | Yes | RomM `roms/<slug>` | Existing route | Playing Library → RomM | Requires verified same-path visibility | Preserve exact visibility gate and expose plainly |
| Combined multi-platform RomM plan | COMPLETE | Apply delegated | Shared per-platform transactions | Yes per transaction | RomM | Backend/native caller | Playing Library RomM path | Future UI needs aggregate apply/history presentation | Add v2 report only; reuse backend |
| ES-DE system mapping/entry projection | COMPLETE/read-only | No | N/A | N/A | ES-DE | Backend/native caller | `launch/es_de_export` | None for mapped systems | Reuse reviewed map; fail closed on absent systems |
| ES-DE `gamelist.xml` publication | COMPLETE | Yes | Recovery record + atomic write | Exact previous-content restore | ES-DE | Follow-up route | Playing Library/RetroDECK path | Native v2 action presentation | Expose as explicit “Publish ES-DE metadata” step |
| RetroDECK projection | COMPLETE | Symlinks + ES-DE publication | Shared transaction + ES-DE recovery | Yes | RetroDECK | Backend/native caller | Playing Library → RetroDECK | Same-path sandbox visibility must be shown | Keep separate from generic ES-DE wording |
| Generic platform-folder organisation | COMPLETE | Depends on selected mode | Rename transaction | Yes | EmuWiz | Partial/native backend | Library Organisation | v2 destination/profile UI | Migrate as “Organise canonical library” |
| RetroArch `.lpl` playlist read/evidence | READ-ONLY | No | N/A | No | RetroArch | No export action | RetroArch environment/launch evidence | Writer and safe ownership contract absent | Do not promise playlist export; keep evidence-only |
| Media-set inspection | READ-ONLY | No | N/A | No | None | No mutation route | Media Sets page | Swap/playlist writer intentionally absent | Keep inspection-only unless a separate design is approved |
| LaunchBox local provider import/discovery | READ-ONLY | Local cache/index only | Provider cache semantics | Cache refresh only | LaunchBox metadata/media | No export action | Sources artwork/media state | No writer or reverse export | Keep as local evidence provider |
| LaunchBox ROM/library export | ABSENT | No | No | No | LaunchBox | No | None | Export schema, ownership, media policy | Do not invent; future research required |
| Pegasus export | ABSENT | No | No | No | Pegasus | No | None | No backend/module found | Do not add to parity checklist until designed |
| Arbitrary EmulationStation variant export | ABSENT | No | No | No | Other ES variants | No | None | No generic XML/schema writer | Support only explicit reviewed variants |
| Duplicate-aware rename presentation | COMPLETE backend / partial route | Yes if approved | Rename/quarantine transactions | Yes | EmuWiz | Partial/native | Problems/Repair, Duplicate Review | v2 organisation should link to existing repair history | Reuse repair transaction; do not duplicate |
| Exact-duplicate quarantine | COMPLETE | Moves files to quarantine | Journaled transaction | Yes where identity remains valid | EmuWiz repair workflow | Separate repair route | Duplicate Review | Not an organisation export | Keep separate; never fold into clean-library mode |
| DAT-driven region/version selection | COMPLETE in plan/evidence | Only via approved rename/library operation | Uses selected transaction | Yes via transaction | EmuWiz/Playing Library | Existing backend | DAT Sources/Playing Library | v2 explanation of election/selection | Show evidence, do not recalculate in UI |

## Existing backend inventory

### Canonical organisation and rename

`dat/rom_organisation` provides four explicit modes:

- `RenameInPlace`: canonical filename in the current directory;
- `MoveRealFile`: real regular file into the canonical platform folder;
- `OrganiseSymlinkOnly`: move the symlink object, never its target;
- `BuildLinkedLibrary`: leave regular sources where they are and create a
  canonical symlink tree.

`dat/rom_organisation/plan.rs` is read-only. It gates on resolved platform
identity, content policy, object kind, safe basename, extension compatibility,
canonical platform layout folders, and destination collisions. Generic
organisation does not consult RomM slugs; RomM-specific slugs belong only to
explicit RomM projection code.

`dat/rom_organisation/transaction.rs` converts approved entries into the
shared `rename_apply` transaction. It captures source identity, checks plan
generation/classifier freshness, journals before mutation, records only
directories it actually created, refuses cross-filesystem moves, and delegates
per-entry no-clobber/apply/rollback/recovery to the shared engine.

### DAT rename apply

`dat/rename_apply` is the common mutation engine for rename and linked-library
operations. It provides:

- identity capture without following the final source symlink;
- preflight of source identity and destination conflicts;
- `RenameMove` and `CreateSymlink` operations;
- no-clobber destination creation;
- durable JSON journals written before mutation and after state transitions;
- per-entry states and transaction states;
- stale-plan and classifier-version rejection;
- exact-resume envelope/reconciliation for interrupted rename work;
- rollback and rollback history;
- activity/history projections used by GUI routes.

There is no copy+delete fallback for canonical organisation. Cross-filesystem
move is refused before mutation. A linked library changes only its destination
tree; source regular files remain untouched.

### Playing Library / 1G1R

`playing_library` owns verified election and grouping. It preserves launcher
plus companion files, release relationships, multi-disc structures, support
associations, and the selected DAT evidence. `apply_adapter` turns the plan
into the same journaled symlink transaction; it does not create a second
filesystem mutation engine.

This workflow is already complete. The future v2 Organisation page should
consume its plan and apply/history projections rather than reproduce election,
region/version selection, or companion matching.

## RomM status

RomM has two separate capabilities that must not be conflated.

### RomM as identity source

`identity_source/romm` is a local/private-network, read-only client. It can
configure a URL/token path, test the server, import and atomically publish a
bounded identity cache, refresh records, inspect mappings/conflicts, compare
published hashes for one file, report stale local records, and maintain an
artwork cache. The CLI explicitly states that it does not write to RomM,
trigger a RomM scan, edit RomM metadata, or touch ROM files during identity
operations.

RomM paths are normalized according to the server's declared relative or
absolute path shape. Mapping uses whole path components, longest prefix wins,
and rejects traversal/unsafe path forms. Existing metadata is preserved in the
cache/provider records; it is not rewritten by local organisation.

### RomM as a local library projection

`playing_library/romm_projection.rs` projects an already elected Playing
Library into `destination_root/roms/<reviewed RomM slug>`. It uses strong DAT
platform identity, reviewed platform-to-slug mapping, launcher/companion
operations, and destination collision refusal. It creates symlinks; it does
not move or copy source ROMs.

Apply is blocked unless visibility is explicitly verified. The current safe
contract is a same-path host/container bind, because an absolute symlink target
that exists on the host may not exist at the same path inside RomM's container.
This is a deliberate fail-closed requirement, not a cosmetic preview flag.

The multi-platform `romm_library_plan` adds missing-source, unsafe-source,
occupied-destination, and duplicate-destination blocks, then delegates apply to
the existing per-platform rename transaction engine. RomM metadata is not
replaced by a local export file; the output is a filesystem layout RomM can
scan/serve.

**Classification:** COMPLETE backend and safety model; native v2 presentation
is the remaining parity work. RomM identity import itself is complete and
read-only, not a library export.

## ES-DE status

`launch/es_de_export.rs` contains a reviewed platform-to-ES-DE system map and
fails closed for unresolved/conflicting identity, unmapped platform, absent
configured system, unresolved content, mounted/unrunnable content, or
ambiguous emulator choice. It does not invent an ES-DE system name.

`launch/es_de_publish.rs` plans and publishes only the elected Playing Library
subset. It reads bounded existing `gamelist.xml`, preserves existing bytes,
appends only missing `<game>` entries, refuses malformed or oversized files,
and does not modify `es_systems.xml`, `es_settings.xml`, or unrelated metadata.

Publication writes a durable path-derived recovery record before touching the
gamelist and uses atomic writing. Restart recovery restores exact previous
content or removes a file that did not exist. In-memory rollback is also
available, and unresolved recovery blocks a second publication.

The ES-DE path is therefore complete as a backend workflow, but it is not a
general export of every EmuWiz item. It is a follow-up publication for a
verified Playing Library plan and a discovered ES-DE profile. A future native
page should show that dependency explicitly:

`Build clean library` → `Publish ES-DE metadata`.

**Classification:** COMPLETE backend; v2 needs native presentation of the
existing explicit follow-up and recovery states.

### RetroDECK

`playing_library/retrodeck_projection.rs` is an ES-DE-compatible projection
with a separate RetroDECK visibility contract. It creates symlink destinations
under `roms/<ES-DE system>` and prepares ES-DE publication. Apply is blocked
unless both source and destination roots are verified visible at the same
absolute paths inside the sandbox. Multi-file releases and rollback tests are
present.

**Classification:** COMPLETE backend; keep distinct from ordinary ES-DE because
the sandbox visibility precondition is different.

## Other frontend/library targets

### RetroArch playlists

RetroArch playlist (`.lpl`) files are discovered and read as environment/core
identity evidence. The project documentation explicitly keeps playlist
identity fields as evidence and does not write, repair, create, or rewrite
`.lpl` playlists. Core selection may be upgraded by unambiguous playlist
evidence, but this is not playlist export.

**Classification:** READ-ONLY. There is no safe export workflow to migrate.
Do not label the current evidence reader “Build RetroArch playlist”.

### LaunchBox

`identity_source/launchbox_local.rs` is a local provider index for LaunchBox
games and media. It parses/discovers local LaunchBox metadata and maps media
roles/provider evidence into EmuWiz's identity/enrichment vocabulary. The
source comments and callers treat it as a provider that does not contact
LaunchBox and does not replace RomM. No LaunchBox XML/database writer or
filesystem export transaction was found.

**Classification:** READ-ONLY import/discovery. LaunchBox export is ABSENT, not
partial. A future export needs a separate schema/ownership and conflict design.

### Pegasus and other EmulationStation variants

No Pegasus writer, Pegasus collection schema, or generic EmulationStation
variant exporter was found. ES-DE is implemented through its own reviewed
system map and XML publication; that cannot be generalized safely to arbitrary
forks/variants.

**Classification:** ABSENT. Do not invent parity work until a target-specific
format and rollback/ownership policy exists.

## Safety audit

| Operation | Source behavior | Destination behavior | Overwrite/clobber | Confirmation | Journal/history | Stale-plan check | Cross-filesystem |
|---|---|---|---|---|---|---|---|
| Rename in place | Source object moved/renamed | Same parent | No-clobber | Explicit approval | Yes | Generation, classifier, identity, preflight | Not applicable/same filesystem requirement |
| Move real file | Source real file moved | Canonical platform folder | No-clobber | Explicit approval | Yes | Revalidation immediately before apply | Refused; no copy+delete |
| Organise symlink only | Symlink object moved; target untouched | Canonical folder | No-clobber | Advanced explicit approval | Yes | Identity/preflight | Refused across filesystem |
| Build Playing Library | Source regular files untouched | New symlinks under selected root | No-clobber; occupied destination blocks | Explicit approval | Yes | Plan generation and source/preflight checks | Destination policy/visibility applies |
| RomM projection | Source untouched | Absolute symlinks under RomM layout | Occupied/duplicate destinations block | Explicit confirmation | Shared transaction | Strong identity, visibility, preflight | Same-path visibility contract |
| ES-DE gamelist publish | ROM sources untouched | Atomic `gamelist.xml` update | Existing bytes preserved; malformed file blocks | Explicit follow-up confirmation | Durable recovery record | Profile/system/plan revalidation | Atomic file replacement; no whole-tree atomicity |
| RetroDECK projection | Source untouched | Sandbox-visible symlinks and gamelist | Blocks absent visibility/occupancy | Explicit confirmation | Shared transaction + gamelist recovery | Strong identity and visibility | Same-path sandbox contract |
| Exact duplicate quarantine | Selected duplicate source moved to quarantine | Quarantine root | Transaction/no-clobber policy | Repair confirmation | Repair journal/history | Identity and preflight | Shared rename constraints |
| RetroArch playlist read | Untouched | No destination | None | None | No mutation journal | Read-only | N/A |
| LaunchBox provider import | ROMs untouched; cache/index may publish | EmuWiz provider cache | Atomic cache replacement | Provider action | Cache provenance | Provider-specific | N/A |

The only important remaining safety concern is presentation: the future native
page must not make a read-only projection look like an export, or make a
symlink-only operation look like a copy. Existing backend safety is stronger
than the current menu-level vocabulary in several places.

## Sets, multi-disc media, and support files

### Optical and multi-disc sets

Playing Library matching treats `.m3u` as a launcher and resolves its disc
entries plus nested `.cue`/BIN companions. A complete multi-disc release is
represented as one elected game with one launcher and all required companions.
Missing or escaping playlist references reject the launcher rather than
creating a partial library. RomM, RetroDECK, and ES-DE projections carry the
launcher/companion relationship through their projected operations.

The media-set page is inspection-only. It explicitly says it will not execute
swaps or create a playlist. Therefore “create an M3U” is not an existing
organisation/export capability.

### Floppy and tape sets

The source contains media-set evidence, tape inspection, archive-set identity,
and platform-specific structural evidence. These are analysis/projection
inputs, not a general set-move or playlist writer. Organisation must preserve
the associated files when the current DAT/Playing Library matcher represents
them as companions; unsupported/incomplete sets remain review-only.

### Arcade parent/clone/support relationships

MAME/FBNeo identity, set, parent/clone, and support evidence exists in the
catalogue/identity layers. This audit found no separate “export an arcade set
tree” writer. Generic folder organisation must not flatten a parent/clone
relationship or move BIOS/support files as ordinary games. Such files are
selected/excluded by the current evidence/classification pipeline, not by a
new frontend exporter.

### BIOS/support exclusion

The current plan/export models carry content classification, support roles,
associations, release relationships, and blockers. “Games only” refuses
unknown/non-game content rather than guessing. A future Organisation page
must show excluded support/BIOS files and their reason; it must not silently
drop them from an operation whose user believes is complete.

## GUI and route parity

Current navigation exposes an Organise area with:

- **Plan Libraries** → `CanonicalOrganisation` / `rom_organisation_page`;
- **Export** → `PublisherProfiles` / `publisher_profile_page`;
- **Identify/Rename** → DAT Sources and Quick Rename routes;
- **Duplicates** → duplicate review and repair routes;
- separate RomM browsing/configuration/provider routes;
- Playing Library as a mode inside Library Organisation, with Generic, RomM,
  ES-DE, and RetroDECK destinations;
- Repair History and History/Logs for transaction review.

The route map is not proof that every backend action is native v2. The parity
classification is:

| Capability | Current reachability | Native v2 parity conclusion |
|---|---|---|
| Canonical rename plan/apply/rollback | DAT Sources, Quick Rename, Library Organisation | Backend complete; needs one coherent v2 Organisation action surface |
| Generic folder organisation | Library Organisation page | Backend complete; destination/mode language needs migration |
| Playing Library | Library Organisation mode / dedicated state module | Complete; preserve as the canonical clean-library flow |
| RomM projection | Playing Library RomM mode and RomM-related publisher state | Backend complete; v2 must show visibility proof and symlink semantics |
| ES-DE publication | Playing Library follow-up action | Backend complete; v2 must make publication dependency/recovery visible |
| RetroDECK projection | Playing Library destination | Backend complete; v2 must preserve sandbox visibility gate |
| Publisher Profiles | Existing Export route | It is an existing route, not evidence of a new universal exporter |
| RetroArch playlist export | No writer route | No parity work; keep evidence-only |
| LaunchBox export | No writer route | Absent; no migration target |
| Pegasus export | No route/backend | Absent; no migration target |
| Media-set playlist/swap | Inspection-only Media Sets route | Intentionally read-only |
| Duplicate quarantine | Problems/Repair and Exact Duplicate Review | Separate repair workflow, not organisation export |

No GUI was changed by this audit. The most important future v2 contract is to
make the operation kind, source mutation, destination type, and reversibility
visible before confirmation.

## Novice workflow model

These are future labels for existing backend actions only:

### “Organise canonical library”

- **Source:** selected verified game files and current source roots.
- **Destination:** configured EmuWiz master root and canonical platform folders.
- **Preview:** DAT evidence, proposed names, platform folder, conflicts,
  content exclusions, and source mutation mode.
- **Confirmation:** explicit approval of rename/move/symlink mode.
- **Apply:** existing `rom_organisation` + `rename_apply` transaction.
- **Activity:** transaction counts and per-entry outcome.
- **History:** durable rename journal and Repair History.
- **Undo:** existing rollback while identities still match.

### “Build a clean playing library”

- **Source:** verified catalogue/library evidence.
- **Destination:** Generic, RomM, ES-DE/RetroDECK-compatible root selected by
  the user.
- **Preview:** elected release, region/version reasoning, companions, sets,
  blockers, visibility proof, and symlink semantics.
- **Confirmation:** explicit typed/action confirmation already used by the
  Playing Library route.
- **Apply:** existing Playing Library transaction and destination projection.
- **Activity:** applied links and publication result.
- **History:** shared rename journal plus operation/history projection.
- **Undo:** rollback removes transaction-created links and restores ES-DE
  gamelist content where applicable.

### “Organise for RomM”

- **Source:** an already-built Playing Library, not arbitrary RomM records.
- **Destination:** selected local path shaped as `roms/<reviewed slug>`.
- **Preview:** platform slug, source visibility, launcher/companions,
  occupied/duplicate destinations, and symlink target paths.
- **Confirmation:** explicit apply after verified same-path visibility.
- **Apply:** existing RomM projection transaction.
- **Activity/History/Undo:** shared transaction mechanisms.

### “Export to ES-DE”

- **Source:** existing Playing Library projection and discovered ES-DE profile.
- **Destination:** exact mapped `gamelist.xml` plus already-created linked ROM
  root.
- **Preview:** mapped system, entries to append, existing-file preservation,
  and any unresolved recovery record.
- **Confirmation:** explicit publication step.
- **Apply:** existing durable gamelist publication.
- **Activity/History:** publication result and recovery state; linked-ROM
  transaction remains separately visible.
- **Undo:** exact previous gamelist restore and link transaction rollback.

### “Rename verified games”

- **Source:** DAT-verified candidates.
- **Destination:** same directory or selected canonical folder according to the
  chosen existing mode.
- **Preview/confirmation/apply/history/undo:** existing DAT rename flow.

### “Create frontend playlists”

This label is **not currently backed by an action**. RetroArch playlists are
read-only evidence and Media Sets explicitly do not create playlists. Do not
put this in v2 until a target-specific writer, ownership policy, atomic update,
conflict model, and rollback/re-export behavior are designed.

## Complete, partial, legacy-only, and obsolete classification

### COMPLETE

- DAT canonical rename/apply/rollback/history.
- Canonical folder organisation with explicit modes.
- Linked Playing Library/1G1R, including companion/multi-disc handling.
- RomM local projection, multi-platform plan, visibility gate, and shared
  transaction apply.
- ES-DE system mapping and gamelist publication/recovery.
- RetroDECK projection and ES-DE publication path.
- Exact duplicate quarantine as a separate repair transaction.

### PARTIAL

- Native v2 parity presentation for the complete backend actions.
- Aggregate activity/history UX for multi-stage Playing Library + ES-DE flows.
- Uniform provider-level report that combines all adapter/backend projections
  without rediscovering evidence.

### READ-ONLY

- RomM identity provider import/refresh/status/hash verification.
- RetroArch playlist inspection/evidence.
- LaunchBox local provider import/media evidence.
- Media-set inspection and multi-disc/set reports.
- Frozen `LibraryPlanExport` data boundary; it exports plan data, not an
  executable apply action.

### LEGACY-ONLY / ADVANCED ROUTES

- Advanced symlink-object organisation.
- DAT Sources / Quick Rename entry points where v2 intends to centralise the
  same backend.
- Publisher Profiles route as the older export-oriented surface, where the
  backend is still reused by the newer Playing Library flow.

### ABSENT, NOT MIGRATION TARGETS

- LaunchBox export writer.
- Pegasus export.
- Generic EmulationStation-variant exporter.
- RetroArch playlist writer.
- Media-set swap/playlist creation.

## What should be retired rather than migrated

Do not retire backend code that is still the safety authority. Eventually retire
duplicate **routes and UI implementations**, not the underlying engines:

1. Consolidate old DAT Sources/Quick Rename and Library Organisation entry
   points around one v2 organisation controller, while retaining the shared
   `rename_apply` journal/recovery engine.
2. Stop presenting Publisher Profiles as a second election/organisation
   implementation if it only delegates to the existing Playing Library and
   frontend projections.
3. Keep Advanced symlink-only organisation available, but remove it from
   novice top-level choices unless the user explicitly chooses an advanced
   mode.
4. Keep RomM identity-provider controls separate from “Organise for RomM”; do
   not merge them into a legacy route that implies server writes.
5. Do not create placeholder migration work for absent LaunchBox/Pegasus/
   playlist writers. Their absence is a documented capability boundary.
6. Keep duplicate quarantine under Repair/Duplicates rather than migrating it
   into ordinary organisation, because its destructive intent and rollback
   semantics differ.

## Recommended v2 implementation order

1. Build one v2 Organisation controller around existing plan/apply/history
   APIs; do not add another transaction engine.
2. Migrate canonical rename/move and linked-library preview rows with explicit
   mutation labels and source/destination roots.
3. Make Playing Library the shared selection/election stage for Generic, RomM,
   ES-DE, and RetroDECK destinations.
4. Add RomM visibility proof, slug, path-shape, and symlink explanations to the
   preview; preserve the read-only identity-provider route separately.
5. Add ES-DE publication as a visibly separate follow-up with recovery status
   and exact rollback wording.
6. Project shared Activity/History/Undo cards from existing journals,
   including multi-file companions and gamelist publication.
7. Leave RetroArch playlists, LaunchBox export, Pegasus, and arbitrary
   EmulationStation exports out of v2 until target-specific backend contracts
   exist.

## Tests and source evidence

Focused tests exist across the audited modules, including:

- `dat/rom_organisation/tests.rs` and `linked_library_tests.rs`;
- `dat/rename_apply` planner, executor, journal, recovery, and rollback tests;
- `playing_library` matching, election, companion, linked-library, RomM, and
  RetroDECK tests;
- `identity_source/romm/tests.rs` for path shapes, mapping, cache, conflicts,
  hash verification, and stale records;
- `launch/es_de_export.rs` and `launch/es_de_publish/tests.rs`;
- `platform_evidence_fusion/library_plan_export/tests.rs`;
- GUI tests for organisation, Playing Library, RomM, ES-DE, navigation, repair,
  and history views.

The source evidence consistently separates read-only planning from mutation,
requires explicit confirmation, refuses stale or ambiguous plans, and routes
filesystem changes through journaled transaction code. This audit made no code
changes and therefore did not run a production build/test suite.

## Final parity statement

The future native Organisation page needs to migrate and present existing
canonical rename, linked Playing Library, RomM projection, ES-DE publication,
RetroDECK projection, history, and rollback capabilities. It does not need a
new Playing Library implementation, RomM identity importer, RetroArch playlist
writer, LaunchBox exporter, Pegasus exporter, or generic frontend exporter.
