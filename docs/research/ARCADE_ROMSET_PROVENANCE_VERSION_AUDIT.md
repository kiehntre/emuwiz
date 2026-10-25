# Arcade ROMset Provenance and Version Compatibility Audit

**Scope:** research only. This document records the evidence model and the
smallest implementation boundary for arcade ROMset provenance. It does not
change EmuWiz production code, ingest a DAT, mutate a catalogue or ROM tree,
or claim that the local collection has a particular ROMset revision.

## 1. Executive Summary

“MAME 0.xxx ROMset” means the set definitions expected by a particular MAME
source/release: set names, parent and clone relationships, BIOS and device
dependencies, ROM member names, sizes, CRC/SHA1 values, CHD requirements and
dump-status metadata. It does not mean that a directory labelled `0.xxx`, or
an archive named after a year, is necessarily that release.

The installed MAME binary can provide a strong, machine-readable expectation
for its own release through `-listxml`, and can check local content with
`-verifyroms`. A particular set can therefore be verified against installed
MAME even when the collection’s original revision is unknown, provided all
required content and dependency chains are checked. That is per-set evidence,
not proof of a collection-wide revision.

The safe model is two-dimensional:

1. per-set compatibility with a named emulator release; and
2. collection-level provenance confidence and revision evidence.

MAME and FBNeo must remain independent compatibility projections. Their set
definitions, DATs, cores and dependency rules are not interchangeable.

The current EmuWiz model already has useful pieces: `ArcadeDatVersionCompatibility`,
DAT set states, dependency states, arcade working status/election, launch
blockers, and the six-state Ready-to-Play projection. The missing research
boundary is a provenance-aware per-set audit that consumes authoritative
expectations without pretending to know the source collection revision.

## 2. Current EmuWiz Inventory

The following is an inventory of the existing types and what they prove. These
are observations of the current tree, not a proposal to duplicate them.

| Existing evidence | What it proves | Important limit |
|---|---|---|
| `ArcadeDatVersionCompatibility` in `crates/archivefs-core/src/diagnostics/arcade_dat_version.rs:226` | A coarse relationship between a DAT and emulator versions: matching, older, newer, unknown or not applicable | It is not proof that every local set matches the installed emulator, nor collection provenance |
| `DependencyState` in `crates/archivefs-core/src/dat/dependency/mod.rs:236` | Dependency evaluation can be satisfied, missing, ambiguous, contradictory, cyclic, unsupported or unavailable | It depends on gathered evidence and does not itself establish a ROMset revision |
| `SetState` and `SetResolution` in `crates/archivefs-core/src/dat/set.rs:269` | Storage/set completeness and metadata resolution | The source comments correctly limit this to storage completeness; complete does not guarantee runnable emulation |
| `ArcadeWorkingStatus` in `crates/archivefs-core/src/playing_library/model.rs:48` | Working, imperfect, not-working or unknown driver/status evidence | Driver quality is separate from ROM content validity |
| Arcade election in `crates/archivefs-core/src/playing_library/mod.rs` | Parent/clone policy and working-clone fallback | Election is a library choice, not a full compatibility audit |
| `LaunchReadiness` and `LaunchBlockerKind` in `crates/archivefs-core/src/launch/readiness.rs:53` | Whether the current launch planner can construct a safe launch and why it cannot | It is launch authority, not archival provenance authority |
| `ReadyToPlayState`, `ReadinessReasonFamily`, `Fixability` and pure `project_ready_to_play` in `crates/archivefs-core/src/ready_to_play.rs:18,29,48,243` | Deterministic projection into `READY`, `READY_WITH_WARNINGS`, `NEEDS_ATTENTION`, `BLOCKED`, `UNSUPPORTED` or `UNKNOWN` | It projects gathered evidence; it must not add an arcade scanner or perform I/O |
| `readiness_attention` in `ready_to_play.rs:420` | Readiness issues through the existing attention model | It should retain typed evidence/provenance rather than flattening it |

The existing research in `docs/research/READY_TO_PLAY_ARCHITECTURE_AUDIT.md`
also establishes that unknown evidence is not missing evidence, identity need
not be archival-perfect for launch, and a valid launch plan is the final gate.
No current type proves the original revision of the supplied arcade tree from
labels such as `rr-mame/2016`.

## 3. Source Inventory

| Source | Proves | Does not prove | Safe EmuWiz use |
|---|---|---|---|
| Installed MAME `-listxml` | The installed binary’s machine, software, ROM, disk, parent/clone, BIOS/device, driver and dump metadata | The revision of an external collection | Pin as an emulator-expectation snapshot, including its MAME version |
| MAME `-verifyroms` | Whether MAME can find and validate the requested set and dependencies using its configured paths | Why an external set was assembled or its original revision | Bounded read-only confirmation; preserve missing, wrong-size and bad-hash details |
| MAME `-listroms`, `-listcrc` | Expected ROM members and hashes for a selected machine/software item | Local presence unless separately verified | Expected-member display and audit input |
| Local files/archives and archive-member fingerprints | Actual names, sizes, hashes and layout observed locally | The intended source DAT or exact collection revision | Evidence only; never infer version from path names |
| Logiqx/ClrMamePro DAT header | A declared DAT name, version/date/author/source and merging/packing policy | That the files were scanned with that DAT, unless scan evidence is bound to it | Strong provenance when retained with the scan and source |
| FBNeo `-listinfo`/DAT/core metadata | FBNeo’s own game/ROM expectations and working metadata | MAME compatibility | Separate FBNeo projection |
| Pack manifest with hashes and exact DAT/release binding | Collection contents and, if trustworthy, the intended source revision | Current installed emulator compatibility until compared | Strong evidence, not an automatic compatibility result |

MAME’s asset-search documentation says short names identify systems, devices,
software lists and items and are exposed by `-listxml`, `-listfull`, `-listroms`
and `-listcrc`.[^1] It also documents parent, BIOS and device search behavior,
including that a device may be shared by multiple systems.[^1]

## 4. MAME Version Model

MAME releases are numbered releases of the emulator and its machine-definition
source. A release changes more than executable code. It may:

- rename, split, merge, add or remove machine/software short names;
- change parent/clone or BIOS relationships;
- add, remove or rename ROM and device members;
- change expected sizes, CRCs or SHA1s;
- change dump-status annotations;
- add or alter CHD/disk requirements and identities; and
- change driver status independently of the underlying dump data.

Thus “the ROMset for MAME 0.264” means the expected definitions shipped with
that release (or an independently published DAT demonstrably generated from
the same definitions), not merely files that happen to launch in a nearby
version. The source-level MAME ROM loader uses hash collections and parent
chains when opening ROM content.[^2]

The installed binary can prove its own expectation snapshot. It cannot prove
that a third-party tree labelled `rr-mame/2016` or `mame-2016.7z` came from an
exact MAME release. A year is not a ROMset revision, and an archive title is
not a cryptographic provenance chain.

Compatibility must therefore compare a specific set and dependency closure to
a named expectation snapshot. A collection may contain clean 0.264-compatible
sets beside stale, newer, incomplete or foreign sets.

## 5. Provenance Evidence Hierarchy

The following hierarchy is deliberately conservative.

### AUTHORITATIVE

- Metadata generated by the exact installed MAME binary, retained with its
  version and a hash of the metadata output.
- A trusted, reproducibly generated DAT whose source release and generation
  process are known.
- A signed or otherwise independently verifiable manifest explicitly bound to
  a release, with content hashes.

### STRONG_EVIDENCE

- A Logiqx/ClrMamePro DAT header containing version/date/source/URL, retained
  alongside the scan result and matching the expected definitions.
- An explicit pack manifest that names the exact DAT/release and includes
  hashes, even if the pack itself is not signed.
- A complete scan result whose expected names, sizes and hashes match the
  installed release, while still keeping collection provenance separate.

### WEAK_EVIDENCE

- DAT filename or a version string in an unverified text file.
- Torrent/archive title, uploader description or directory convention.
- A pack name containing a year or emulator version without a bound manifest.

### UNTRUSTED_LABEL

- `rr-mame/2016`.
- `mame-2016.7z`.
- A folder named `0.264` without independently checked content and metadata.

The two labels supplied for this collection remain weak or untrusted. They
must not be promoted to an exact revision. Provenance should be recorded as a
set of evidence items with source, acquisition/scan time, hash and confidence,
not as a single guessed version string.

## 6. Per-Set Compatibility

The useful audit result is per set and dependency closure. A future audit may
use the conceptual states below without adding a parallel launch vocabulary:

- `COMPATIBLE`: required members, parents, BIOS and devices are present and
  match the pinned MAME expectation; required CHDs also match identity.
- `PARTIALLY_COMPATIBLE`: some content is acceptable or best available but a
  non-fatal limitation remains, such as a known imperfect dump or incomplete
  optional evidence.
- `INCOMPATIBLE`: a required file is missing, wrong-sized, hash-mismatched or
  a required dependency cannot be satisfied.
- `UNKNOWN`: required evidence was not gathered or cannot currently be
  established.

Per-set compatibility can be proven without knowing the collection’s source
revision. For example, if `mslug` and its `neogeo` dependency match the
installed MAME 0.264 member definitions, that set can be verified against
0.264 even if the rest of the tree is mixed and the pack’s original revision
is unknown. The result must say “verified against MAME 0.264”, not “the whole
collection is a 0.264 ROMset”.

Evidence should check, in order appropriate to the artifact:

1. expected short name and set identity;
2. archive/member or directory presence;
3. expected size;
4. CRC and SHA1 (or the strongest available hash);
5. parent, BIOS and device closure;
6. CHD content identity and disk dependency closure; and
7. driver status as a separate dimension.

Filename presence alone is never enough. A wrong-size file is not valid merely
because its name exists.

## 7. Collection-Level Compatibility

Whole-collection provenance and per-set compatibility must remain separate.
The collection-level report should include:

- provenance confidence and the evidence supporting it;
- the expectation release(s) used for audits;
- counts or sets with `COMPATIBLE`, `PARTIALLY_COMPATIBLE`, `INCOMPATIBLE` and
  `UNKNOWN` results; and
- evidence that the scan was complete, bounded or selective.

It must not expose one global “MAME compatible” boolean. A mixed collection
can have many verified sets, stale sets, missing BIOS/device content and
unrelated revisions simultaneously. Collection provenance may remain unknown
even when individual launch candidates are clean.

## 8. Mismatch Taxonomy

The following mapping retains specific evidence while fitting current EmuWiz
dependency/readiness families:

| Evidence | Compatibility meaning | Likely current launch/readiness consequence |
|---|---|---|
| `REQUIRED_FILE_MISSING` | Required member absent | Missing dependency; attention or blocked according to launch semantics |
| `WRONG_SIZE` | Member exists but expected length differs | Proven invalid content; blocked |
| `CRC_MISMATCH` / `SHA1_MISMATCH` | Content does not match expectation | Proven mismatch; blocked |
| `NEEDS_REDUMP` / `BAD_DUMP` | Dump is known imperfect, not necessarily absent | Warning or attention; do not relabel as missing |
| `NO_DUMP` | No known dump is available for that definition | Unsupported/blocked if required; retain the exact status |
| `PARENT_MISSING` | Clone closure cannot be satisfied | Dependency blocked |
| `BIOS_MISSING` | Required BIOS member/set unavailable | Attention or blocked according to existing launch gate |
| `DEVICE_DEPENDENCY_MISSING` | Required non-runnable/shared device unavailable | Dependency blocked |
| `CHD_MISSING` / `CHD_HASH_MISMATCH` | Required disk absent or wrong | Blocked |
| `SET_RENAMED_OR_UNKNOWN` | Short name cannot be safely resolved | Unknown or review; no filename-only promotion |
| `VERSION_PROVENANCE_UNKNOWN` | Collection source revision cannot be established | Provenance warning, not automatically a launch blocker |
| `VERSION_MISMATCH_PROVEN` | Evidence is bound to a different expected revision and fails current checks | Blocked for the selected emulator if required content differs |
| `BEST_AVAILABLE_IMPERFECT` | MAME accepts content while marking dump quality imperfect | Warning/attention, not missing |
| `UNSUPPORTED_BY_INSTALLED_EMULATOR` | No supported adapter/driver/launch route | Unsupported |

These are evidence details, not permission to invent a new top-level readiness
state. Existing `DependencyState`, `LaunchBlockerKind`, reason families and
fixability should carry the detail and provenance.

## 9. Dump Status Semantics

Dump status describes the quality or existence of the physical dump. It does
not describe whether emulation is accurate.

- `NO_DUMP` means the expected physical dump has not been obtained or is not
  available in the definition. It is not the same as “the scanner failed”.
- `BAD_DUMP` means a known bad or suspect dump is represented; it may be the
  best available evidence and may be accepted by MAME for practical use.
- `NEEDS REDUMP` is a warning about dump confidence. It should preserve the
  accepted/best-available distinction when MAME reports the set usable.
- “best available” means the current known dump is accepted operationally but
  not archival-perfect. It should not be silently upgraded to byte-perfect.

The concrete `cdimono1` case is therefore not “missing” merely because it has
NEEDS REDUMP entries. Its result should retain the imperfect-dump evidence and
map to a warning/attention state according to the existing launch semantics.

## 10. Driver Status vs Content Validity

MAME’s driver metadata contains statuses such as good, imperfect and
preliminary. These describe emulation quality, implementation maturity or
confidence in the driver. ROM member status describes content quality. The
dimensions must be displayed and evaluated independently:

| Driver | ROM content | Interpretation |
|---|---|---|
| Good | Hash-valid | Best case; normally launch-ready |
| Imperfect | Hash-valid | Playable may be possible, with warning |
| Preliminary | Hash-valid | Playability/accuracy warning; not a ROM mismatch |
| Good | Wrong/missing required member | Content blocks despite a good driver |
| Unknown | Gathered content valid | Driver evidence remains unknown; do not fabricate quality |

The Ready-to-Play projection can turn a nonblocking imperfect driver into
`READY_WITH_WARNINGS`, while a proven required hash mismatch remains
`BLOCKED`.

## 11. BIOS / Device / Parent Dependencies

MAME can search parent sets, corresponding BIOS systems and device locations
when resolving content.[^1] A BIOS set is not itself necessarily a runnable
game; a device may be shared by several machines, and qsound is a useful
non-runnable device example. Parent and BIOS relationships are part of a
candidate’s dependency closure, not optional folder decoration.

The audit should distinguish:

- present and valid: expected member/hash is proven;
- present but mismatched: name exists but size/hash fails;
- missing: required evidence was searched and absent;
- unknown: the relevant location/member was not safely evaluated; and
- non-runnable dependency: a device/BIOS component is a dependency, not a
  user-selectable game.

For a clone, a self-contained non-merged archive, parent archive, BIOS set and
device closure may all participate. `qsound` should never be shown as a game
that can be elected simply because it is present; it should appear as a
dependency with its own evidence.

## 12. Merged / Split / Non-Merged

The conventional layouts are:

- **Merged:** parent and clone content are combined in one archive. A clone
  archive may not be self-contained; the scanner must understand the archive’s
  set membership.
- **Split:** parent content is in the parent archive and each clone carries
  only unique members. A clone commonly requires the parent archive plus BIOS
  and devices.
- **Non-merged:** each set is intended to contain its complete required ROM
  content, so clones can be moved independently, subject to shared BIOS/device
  dependencies.

The layout affects storage and scanner behavior, not the expected hash of a
member. MAME’s search rules can find content through parent paths, but a folder
name cannot prove merge mode. Archive member evidence and DAT/manifest merge
policy are required. If that evidence is absent, merge layout is `UNKNOWN`,
not an inferred fact.

ClrMamePro/Logiqx DAT headers can declare force merging, packing and no-dump
policy, which is useful provenance when retained with the DAT.[^7]

## 13. CHD Compatibility

CHD compatibility is about disk identity and dependency closure, not merely
the CHD container version. MAME searches CHDs by content digest and parent
delta behavior can affect where a disk is found.[^1] A CHD’s compressed bytes
can vary with compression choices while its content identity remains the same.

The audit should retain:

- expected CHD SHA1/content identity;
- parent/clone disk dependency;
- the observed CHD header/container version;
- whether the disk is complete and found in the expected topology; and
- any obsolete or changed expectation from the pinned MAME metadata.

A file hash of the CHD container alone is not a universal content identity.
Conversely, matching content identity does not prove the collection’s MAME
release. “CHD format version” and “MAME ROMset revision” are separate fields.
Missing or wrong CHDs are required-content failures; equivalent compression is
not automatically a mismatch.

## 14. FBNeo Compatibility

FBNeo is an independent ecosystem. Its CLI can emit game information and ROM
files in MAME-XML-like form through `-listinfo`, but that is FBNeo’s metadata,
not MAME’s authoritative set definition.[^5] The Libretro database keeps
separate FBNeo DATs, including split variants, and documents CRC/serial and
size/hash indexing fields.[^6]

Compatibility must be pinned to a FBNeo core/version or DAT provenance tuple:

- FBNeo core version/commit;
- FBNeo DAT or `-listinfo` generation source and date;
- merged/split/non-merged policy;
- ROM names, sizes and hashes expected by that source;
- BIOS/device dependencies where the core declares them; and
- RetroArch/core configuration evidence when that affects launch.

The FBNeo libretro core advertises compatibility with the latest FBNeo ROM
sets, which reinforces that core/set coupling exists rather than proving a
MAME equivalence.[^4] FBNeo’s own changelog also records ROMset/DAT changes
over time.[^8]

Do not treat a MAME DAT as an FBNeo DAT, or assume that a MAME-compatible ZIP
is FBNeo-compatible. The same filenames may carry different expected content
or dependency semantics.

## 15. Cross-Emulator Readiness

The safe conceptual structure is:

```text
Game identity
└── Arcade set / variant
    ├── MAME compatibility against pinned MAME evidence
    └── FBNeo compatibility against pinned FBNeo evidence
```

A set may be `READY` for FBNeo and `BLOCKED` for MAME because a MAME BIOS,
parent or hash is missing. The converse is also possible. A generic game
projection should include the selected emulator/profile as provenance; it
must not collapse two emulator results into one universal arcade truth.

Existing arcade parent/clone election remains useful for selecting a library
candidate, but it must not silently convert a MAME result into an FBNeo result.

## 16. Ready-to-Play Mapping

The current six states are sufficient:

| Evidence | Projection |
|---|---|
| Valid deterministic launch plan; identity, emulator, required content and dependencies verified | `READY` |
| Launch is valid, but driver is imperfect/preliminary or accepted best-available dump evidence is nonblocking | `READY_WITH_WARNINGS` |
| Fixable missing BIOS/configuration or review-only evidence under current launch semantics | `NEEDS_ATTENTION` |
| Invalid launch plan, required media/dependency incomplete, wrong size/hash, missing required CHD, or proven incompatible required content | `BLOCKED` |
| No supported platform/adapter/driver path | `UNSUPPORTED` |
| Required evidence has not been gathered or cannot currently be established | `UNKNOWN` |

Specific answers:

- Clean required MAME content and dependencies with a valid plan maps to
  `READY`.
- Valid content with an imperfect driver maps to `READY_WITH_WARNINGS` when
  the launch planner considers it launchable.
- An accepted best-available dump such as the stated `cdimono1` case maps to
  warning/attention according to the existing typed launch evidence, never to
  missing merely because it needs redump.
- Missing required BIOS follows existing launch semantics: `NEEDS_ATTENTION`
  when user-fixable/reviewable, or `BLOCKED` when execution cannot proceed.
- Wrong required size/hash is `BLOCKED`, not unknown.
- Unknown collection revision does not alone lower a selected set that
  verifies cleanly against installed MAME. The result may show a separate
  provenance warning, but a valid launch plan can still be `READY`.

Precedence remains deterministic: a proven blocker must not be erased by
unknown fields, and unknown must never be treated as missing. The projection
is informational; the launch planner remains final authority.

## 17. Fixability

Use the existing Doctor/KnownRecovery contract:

| Issue | Fixability |
|---|---|
| Collection revision unknown | `INFORMATION_ONLY` unless a user can provide trusted provenance |
| Missing BIOS/parent/CHD | Usually `USER_CAN_FIX` or `EMUWIZ_CAN_GUIDE`; never imply legal acquisition |
| Wrong ROM revision/hash/size | `USER_CAN_FIX`/`EMUWIZ_CAN_GUIDE` after obtaining a legitimate matching dump; no automatic repair |
| Unsupported driver/adapter | `UNSUPPORTED` or information-only |
| Bad/needs-redump content | Information or user-guided replacement; not an EmuWiz repair |
| Ambiguous set identity or layout | `EMUWIZ_CAN_GUIDE` to review evidence, otherwise information-only |
| Safe local metadata/configuration repair, if an existing recovery explicitly supports it | `EMUWIZ_CAN_REPAIR_SAFELY` only when that existing contract proves it |

Tool availability, a matching filename, or a DAT hit does not authorize ROM
download, repair, conversion or deletion.

## 18. User-Facing UX

Prefer explanations that lead with the result:

- “These files match what MAME 0.264 expects.”
- “This game is playable, but MAME marks one ROM as the best available
  imperfect dump.”
- “This collection’s original MAME revision is unknown, but this game’s files
  match your installed MAME.”
- “This BIOS file exists, but its size or hash does not match MAME 0.264.”
- “The clone is missing content from its parent set.”
- “This device is required by the game but is not itself a runnable game.”

Advanced details should expose set name, expected/observed size and hashes,
parent/BIOS/device closure, CHD identity, driver status, DAT/XML source and
scan timestamp. Jargon such as CRC, SHA1 and merged mode belongs in that
detail view, alongside plain-language summaries.

## 19. Performance / Caching

A 250,000-file tree should not trigger a complete validation on every render.
The scalable shape is:

- one bounded inventory that records file/archive-member fingerprints;
- cached expected-member evidence keyed by emulator release/DAT hash;
- lazy per-set dependency closure for selected or launch-candidate sets;
- archive-member evidence cached by archive identity and member metadata;
- CHD identity cached separately from container allocation/hash;
- incremental invalidation when a path, size, mtime or fingerprint changes;
- invalidation when emulator version, `-listxml` hash or DAT provenance changes;
- explicit scan completeness so a partial scan yields `UNKNOWN`, not clean;
- no repeated expensive work for a filter or presentation-only projection.

Expected hashes make exact checks deterministic. Where hashing an archive or
large disk is expensive, the result should say not gathered until a bounded
audit obtains the required evidence. No score, percentage or arbitrary
threshold is needed.

## 20. Security / Trust

Metadata and pack labels are untrusted input. The audit must defend against:

- archive-member path traversal or unsafe extraction assumptions;
- a DAT that claims a release without trusted provenance;
- malicious or malformed XML causing unbounded parsing;
- hash confusion between a container and its content;
- symlink/path redirection during local inspection; and
- stale cached evidence after emulator/DAT/content changes.

Read-only audit commands must resolve and log their source paths, bound work,
avoid network acquisition, and never mutate ROMs or catalogues. A hash match
is evidence about bytes, not a licence, provenance chain or emulation quality.

## 21. Real-Data Case Study

The following applies the evidence model to the supplied live MAME 0.264
results. “Good” below describes MAME’s verification result for the current
configured paths; driver status remains a separate field.

| Set/case | Live evidence | Expected compatibility/readiness interpretation |
|---|---|---|
| `neogeo` | BIOS set verified good | Required BIOS evidence is satisfied; not missing. A game depending on it still needs its own ROM and dependency closure |
| `aes` | BIOS set verified good | Same distinction: valid BIOS evidence, not a runnable game result by itself |
| `pgm` | BIOS set verified good; local MAME metadata marks driver imperfect | Content can be compatible; launch may be `READY_WITH_WARNINGS` if the selected game is otherwise valid |
| `skns` | BIOS set verified good | BIOS dependency satisfied; evaluate each dependent game separately |
| `qsound` | Device dependency verified good; non-runnable device | Count as dependency evidence, never as an elected standalone title |
| `awbios` | BIOS set verified good; metadata is preliminary | Satisfied content plus emulation-quality warning where relevant |
| `cdimono1` | MAME accepts best available content with NEEDS REDUMP evidence | Preserve `BEST_AVAILABLE_IMPERFECT`; warning/attention, never `MISSING` solely for redump status |
| `decocass` | Missing `v0c-.7e`, `dsp-3_p0-c.m9`, `dsp-3_p0-d.m9` | Proven incomplete required dependency/content; blocked where required |
| `naomi` | Missing and incorrect-length members plus redump evidence | Wrong-size/missing requirements are blocked; do not accept filename presence |
| `naomi2` | Missing `x76f100_eeprom.bin` and wrong/missing JVS/BIOS pieces | Proven incomplete dependency closure; blocked where required |
| `mslug` | MAME metadata identifies `romof="neogeo"`; local completeness must be checked with the BIOS closure | Parent/BIOS relationship is real evidence; do not call the whole collection a revision based on the directory label |
| `pacman` / `puckman` | MAME metadata provides clone/parent relationship; local presence must be independently established | Election and compatibility are distinct; a missing parent or clone member remains a dependency result |

The local collection labels `rr-mame/2016` and `mame-2016.7z` are not exact
ROMset provenance. The stated installed version is MAME 0.264. Unless an
independent DAT/manifest binds the source tree to an exact release, collection
compatibility is `UNKNOWN`/review-level even when individual sets verify cleanly.

## 22. Recommended Roadmap

The smallest bounded future slices are:

### A0 — Evidence vocabulary

Define a shared, provenance-bearing compatibility detail that reuses existing
dependency, launch blocker, reason-family and fixability types. Preserve
member-level mismatch and dump/driver dimensions.

### A1 — Per-set compatibility against installed MAME

Pin one installed MAME expectation snapshot (version and metadata hash), audit
selected set closures read-only, and expose compatible/partial/incompatible/
unknown evidence. Do not infer collection revision.

### A2 — Collection provenance metadata

Accept explicit DAT/manifest evidence, record confidence and scan completeness,
and report a distribution of per-set results. Keep provenance separate from
launch readiness.

### A3 — FBNeo compatibility

Add an independent FBNeo DAT/core expectation source and projection. Never
reuse MAME compatibility as a shortcut.

### A4 — GUI explanation

Expose plain-language selected-set results and expandable technical evidence,
including emulator release, source, mismatch details and dump/driver status.

### A5 — Optional guidance/planning

Offer read-only next-step guidance where existing Doctor contracts permit it.
Do not download, repair, replace or delete ROM content automatically.

## 23. Do-Not-Build

Explicitly reject:

- guessing a ROMset version from directory/archive names or years;
- automatic ROM downloading or a proprietary ROM-repair service;
- mutation of ROMs to “fix” compatibility;
- treating NEEDS REDUMP/BAD DUMP as Missing;
- conflating MAME driver quality with ROM validity;
- one collection-wide compatibility boolean;
- assuming MAME and FBNeo definitions match;
- treating a CHD container version as a MAME ROMset revision;
- claiming a filename match is a hash match; and
- making Ready-to-Play bypass the existing launch planner.

## 24. Final Decision

1. **Can per-set compatibility be proven without knowing the original
   collection revision?** Yes. A selected set and its parent/BIOS/device/CHD
   closure can be verified against a pinned installed MAME expectation using
   names, sizes and hashes. The result must be scoped to that emulator release.

2. **Can a game be Ready when collection revision is Unknown?** Yes. If the
   selected game’s required evidence is verified and the existing launch plan
   is valid, collection-level provenance uncertainty alone should not block
   launch. It may be surfaced as a provenance warning or advanced detail.

3. **Should whole-collection “MAME version” be treated separately from
   per-set compatibility?** Yes, absolutely. Collection provenance is an
   evidence/confidence property; per-set compatibility is a result against a
   named expectation. A mixed tree cannot be represented by one boolean.

4. **Can MAME and FBNeo compatibility ever be safely assumed equivalent?** No.
   They are separate ecosystems with independent DATs, core versions, merge
   policies, dependencies and expectations. Evidence must be gathered and
   reported separately.

5. **What is the smallest next implementation slice?** A0 followed by A1:
   pin installed MAME `-listxml` provenance, add a read-only per-set audit
   detail that reuses existing dependency/readiness evidence, and preserve
   unknown collection provenance. Do not add FBNeo or GUI work until that
   narrow MAME evidence path is tested.

## Sources

[^1]: [MAME documentation: Asset Searching](https://docs.mamedev.org/usingmame/assetsearch.html) — parent/clone, BIOS/device lookup, short names, ROM paths, CHD search and missing/incorrect content behavior.
[^2]: [MAME source: `romload.cpp`](https://github.com/mamedev/mame/blob/master/src/emu/romload.cpp) — ROM loading, hash collections and parent-chain implementation evidence.
[^3]: [MAME source: PGM driver](https://github.com/mamedev/mame/blob/master/src/mame/igs/pgm.cpp) — concrete `BAD_DUMP` metadata usage.
[^4]: [Libretro FBNeo core information](https://github.com/libretro/libretro-core-info/blob/master/fbneo_libretro.info) — FBNeo core/version and latest-ROMset coupling statement.
[^5]: [FBNeo command-line documentation](https://github.com/finalburnneo/FBNeo/wiki/Command-Line) — `-listinfo` and ROM/game metadata output.
[^6]: [Libretro database README](https://github.com/libretro/libretro-database) — separate MAME/FBNeo DATs, split variants, indexing fields and DAT provenance context.
[^7]: [ClrMamePro profiler documentation](https://www.clrmame.pro/en/profiler) — DAT header and merging/packing/nodump fields.
[^8]: [FBNeo `whatsnew.html`](https://github.com/finalburnneo/FBNeo/blob/master/whatsnew.html) — historical ROMset/DAT changes.
[^9]: [EmuWiz Ready-to-Play architecture audit](READY_TO_PLAY_ARCHITECTURE_AUDIT.md) — local authoritative projection and unknown-semantics design.

