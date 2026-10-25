# Arcade Manager → EmuWiz Arcade Audit

**Status:** research only. No production code, DAT logic, Save Vault, Publisher Profile, ROM files, downloads, or GUI were changed by this audit.

## Executive conclusion

Arcade Manager is useful evidence for a separate arcade curation policy, but it is not an authoritative ROM-set validator. Its strongest transferable ideas are:

- keep parent/clone topology separate from console-style regional 1G1R;
- retain a working clone when the preferred parent is known not to work;
- treat BIOS, device ROMs, samples, parent/ROM-source relationships, and CHDs as a dependency closure;
- bind ROM-set evidence to an emulator family and ROM-set version;
- keep controls and curated “best games” as separate metadata overlays.

EmuWiz already covers most of the difficult *structural* work. `DatGameEntry` preserves MAME/FBNeo-style `cloneof`, `romof`, `sampleof`, `is_bios`, `isdevice`, `runnable`, BIOS sets, devices, samples, ROM merge references, disks, and CHD metadata. The dependency resolver distinguishes parent set, ROM source, merged ROM/disk, BIOS, device, sample, and CHD-parent requirements and fails closed on missing, ambiguous, malformed, cyclic, or partial evidence. The missing capability is an arcade-specific election/readiness layer that consumes those facts without changing the existing console election engine.

The recommended near-term model is read-only: `READY`, `READY_WITH_WORKING_CLONE`, `MISSING_DEPENDENCY`, `ROMSET_VERSION_MISMATCH`, `EMULATOR_VERSION_UNKNOWN`, `KNOWN_NOT_WORKING`, `IMPERFECT`, and `REVIEW_REQUIRED`. Do not add ROM-set conversion, community downloading, subjective “best” lists as truth, or destructive filtering in this phase.

## Sources and exact inspection point

### Arcade Manager

- Repository: [cosmo0/arcade-manager](https://github.com/cosmo0/arcade-manager)
- Reference branch inspected: `master`.
- Release source point inspected: short commit `9159d3b`, shown by the repository’s releases page for ArcadeManager `26.1-alpha1`; the public page exposed the short hash, not the full hash. Re-pin the full hash before any implementation work: [release history](https://github.com/cosmo0/arcade-manager/releases), [commit tree](https://github.com/cosmo0/arcade-manager/tree/9159d3b).
- Exact repository documents inspected: `README.md`, `FILTERS.md`, repository `LICENSE.md`, and the companion data repository’s `README.md` and list directories.
- Companion data: [arcade-manager-data](https://github.com/cosmo0/arcade-manager-data). It contains the generated/filter lists and manually curated classics lists used by the application.

The GitHub source browser was available for repository/document inspection, but did not expose a stable, complete source-file/function index for every implementation class in this environment. Therefore this audit names the authoritative filter/list inputs and the externally observable semantics rather than inventing unverified class names. The exact implementation claims below are limited to `FILTERS.md`, `README.md`, release notes, and the official MAME semantics they consume.

### External technical references

- [Arcade Manager filters](https://github.com/cosmo0/arcade-manager/blob/master/FILTERS.md)
- [Arcade Manager README](https://github.com/cosmo0/arcade-manager/blob/master/README.md)
- [Official MAME documentation](https://docs.mamedev.org/)
- [MAME ROM loading implementation](https://github.com/mamedev/mame/blob/master/src/emu/romload.cpp)
- [Libretro arcade dependency guide](https://github.com/libretro/docs/blob/master/docs/guides/arcade-getting-started.md)

### EmuWiz files/functions compared

- `crates/archivefs-core/src/dat/model.rs`: `DatGameEntry`, `DatRomEntry`, disk/device/BIOS/sample models, `DatEcosystem` and DAT provenance.
- `crates/archivefs-core/src/dat/dependency/graph.rs`: name/ID resolution, duplicate refusal, bounded chain walking.
- `crates/archivefs-core/src/dat/dependency/resolve.rs`: parent, ROM-source, merged member, BIOS, device, sample, and CHD-parent resolution.
- `crates/archivefs-core/src/dat/dependency/clone_report.rs`: deterministic clone relationship and family-root reporting.
- `crates/archivefs-core/src/dat/dependency/mod.rs`: dependency vocabulary and the explicit `BIOS_RUNTIME_SELECTION_NOT_MODELLED` boundary.
- `crates/archivefs-core/src/dat/set.rs` and `dat/archive/chd.rs`: direct storage and CHD evidence.
- `crates/archivefs-core/src/dat/parsers/mame_listxml.rs`, `dat/parsers/clrmamepro.rs`, and `dat/index.rs`: MAME/DAT ingestion and identity indexing.
- `crates/archivefs-core/src/diagnostics/arcade_dat_version.rs`, `diagnostics/arcade_version_probe.rs`: MAME and FBNeo version parsing/probing boundaries.
- `crates/archivefs-core/src/emulator_environment/mame.rs` and `emulator_environment/fbneo.rs`: executable/environment evidence.
- `crates/archivefs-core/src/playing_library/model.rs`, `playing_library/mod.rs`, `playing_library/evidence.rs`, and `playing_library/romm_library_plan.rs`: current family grouping, evidence-ranked console election, and projections.
- `crates/archivefs-core/src/launch/mame_command.rs`, `launch/fbneo_command.rs`, and launch integration: runtime command/environment seams.

## A. Parent/clone semantics

MAME topology is not a console region list. A DAT/listxml relationship is a machine-family statement. `cloneof` identifies the parent lineage; `romof` identifies the ROM source and may be distinct. MAME listxml also provides machine flags and provenance such as `runnable`, `isbios`, `isdevice`, `romof`, `cloneof`, source file, samples, ROMs, and disks. EmuWiz preserves and resolves these independently; that is the correct foundation.

Arcade Manager’s `set-noclone` retains parents. `filter-clones` produces the inverse view. Its documented `filter-workingclones-noparent` script removes clones except when the parent is not working and the clone is working. This is a curation rule, not a claim that a clone is independent of its parent or that the parent may be deleted from the source library.

Regional and revision clones remain distinct arcade machines. Bootlegs, hacks, prototypes, and console-derived drivers are not safely reducible to console region/revision preference without source-specific evidence. Arcade Manager exposes filters for consoles and slow/driver families, but the inspected material does not establish a universal authoritative ranking among all regional/revision/bootleg/prototype candidates.

**Finding:** EmuWiz needs a typed arcade election policy, not a second general election engine and not console `1G1R` applied unchanged. The policy should group by DAT family, prefer a configured parent when it is runnable and dependency-complete, then permit a deterministic working-clone fallback. Equal candidates or ambiguous topology must remain reviewable.

## B. Working-clone fallback

Arcade Manager’s documented base accepts `working` and `imperfect` emulation/driver status, while excluding `not working`; it also requires acceptable sound, color, graphics, and protection status in its working-oriented base. This means “working” in its practical lists is broader than perfect emulation. The status source is the ArcadeItalia/MAME-derived CSV and MAME driver metadata, not a runtime test by Arcade Manager.

The documented fallback is simple: retain a clone when the parent is non-working and that clone is working. The inspected filters do not document a universal weighted ranking for several equally suitable working clones, nor a preference ordering across sound versus graphics versus protection. Determinism therefore comes from the input list/order and generated list process, not a demonstrated quality score. EmuWiz should not infer a ranking that the source does not prove.

Recommended evidence states:

| State | Meaning |
|---|---|
| `PREFERRED_PARENT` | Parent is identified, playable status is acceptable, and dependencies are complete. |
| `WORKING_CLONE_FALLBACK` | Parent is known not working or unavailable for play, and a specific clone is independently evidenced as the selected playable candidate. |
| `IMPERFECT` | Driver/emulation/audio/video/protection metadata is imperfect but not marked non-working. |
| `KNOWN_NOT_WORKING` | Source metadata explicitly says the machine is not working. |
| `REVIEW_REQUIRED` | Multiple candidates, conflicting source fields, or absent/ambiguous status prevents deterministic election. |
| `UNPLAYABLE_SET` | The set or required dependency is explicitly non-runnable or missing. |

Do not silently delete a parent because a clone was elected. A selected clone still carries its declared dependency closure.

## C. BIOS, devices, parents, samples, and CHDs

MAME’s set is a dependency graph, not necessarily one ZIP. A clone can borrow ROM members from a parent; a machine can require a BIOS set, device ROMs, samples, or a CHD; a CHD can itself declare a parent image. MAME’s official documentation distinguishes these runtime/storage relationships, and the ROM loader resolves BIOS/device/disk requirements rather than treating filenames as sufficient identity.

Arcade Manager’s release history confirms that dependency handling is material: an older release specifically improved BIOS retention/copying, and a later release fixed copying of additional files beyond samples and BIOSes and a CHD crash. That is evidence that simple “copy the selected ZIP” handling is unsafe. It is not evidence that dependencies should be flattened.

EmuWiz already has the important structural coverage:

- `ParentSet` and `RomSource` remain distinct;
- `merge=` is resolved only against the named provider’s declared member;
- BIOS, device, and sample namespaces remain distinct;
- CHD parent links are checked from CHD header evidence, separately from DAT disk `merge=`;
- duplicate/absent/malformed/cyclic references fail closed;
- partial scans cannot claim a complete set;
- runtime BIOS variant selection is explicitly not modelled and must not be presented as proof of runnability.

**Genuine gap:** there is no final arcade readiness/election policy that turns these already-present dependency reports into “this selected family candidate is launch-ready.” Do not add another dependency resolver.

## D/E. Merged, split, and non-merged sets

Operationally, following MAME terminology:

| Set form | Operational meaning | Storage | Launch independence |
|---|---|---:|---|
| Merged | Parent archive contains the parent and its clone data; clone archives need not be standalone. | Lowest for shared content. | Lowest; the correct merged parent archive is required. |
| Split | Parent archive contains shared files; each clone archive contains only its unique files. | Balanced. | A clone depends on its parent. |
| Non-merged | Every game archive contains everything required for that game’s ROM set. | Highest redundancy. | Highest for ROM members, subject to BIOS/device/CHD rules. |

These labels describe archive packing, not a different game identity. CHDs remain path- and set-related and must be included with the matching set; converting ZIP members without preserving CHD relationships is unsafe.

Arcade Manager documents conversion to non-merged, while other merged/split conversions are delegated to ClrMamePro. Its non-merged approach is useful as a planning concept, but EmuWiz should initially support:

1. read-only detection when evidence proves the packing shape;
2. planning/reporting of the storage and dependency consequences;
3. no conversion execution.

For EmuWiz’s disk-minimisation goal, merged is generally lowest storage, split is the balanced default, and non-merged is most redundant but easiest to move/run as a standalone clone. Non-merged must never be the default recommendation unless the user explicitly wants clone independence and accepts duplication.

## F. MAME / FBNeo compatibility

Arcade Manager’s filters explicitly map common frontend labels to MAME versions, including MAME 0.37b5, 0.78, 0.139, 0.159, and 0.174. Its README assumes the user starts with a working ROM set corresponding to the target MAME/FBNeo version. The data repository also separates MAME-era lists from FBNeo lists.

ROM-set version and emulator version are related but not identical:

- MAME ROM names, hashes, parent relationships, driver behavior, and required files can change between releases.
- A DAT generated for one MAME release is not proof that another MAME release will load the same set.
- FBNeo has its own driver/set ecosystem and DAT; it must not be treated as “MAME with different executable naming.”
- A numeric version match is useful evidence, but a family/provider/format/target compatibility tuple is safer than comparing two strings.

EmuWiz already has MAME version parsing and an honest FBNeo unknown-version boundary, plus `DatEcosystem` and DAT provenance. Ready-to-Play should make both ROM-set target and emulator target first-class, with `UNKNOWN`/`REVIEW_REQUIRED` when either side cannot be safely compared. Do not turn unknown FBNeo version into a false ready result.

## G. Controls and input filtering

Arcade Manager’s control filters are driven by ArcadeItalia/MAME-derived CSV fields. The documented categories include joystick, buttons, pedal, trackball, dial, paddle, analog, wheel, lightgun, mouse, keyboard, mahjong, hanafuda, keypad, positional, triple/double joystick, gambling, and alternative controls. A small script can classify wheel plus shifted stick as dial/stick. The lists include “stick only,” “pad,” “analog,” and “alternative” collections.

These fields are useful objective machine metadata, but not a complete playability guarantee:

- control naming and classification can change between metadata sources and emulator versions;
- “gamepad playable” is a curation/device-profile claim, not a MAME identity fact;
- a host may lack the required physical device or mapping;
- multiple players, analog ranges, lightgun calibration, pedals, and cabinet wiring are runtime concerns.

Use typed capabilities as collection/readiness evidence: `JOYSTICK`, `BUTTONS`, `TRACKBALL`, `DIAL`, `PADDLE`, `WHEEL`, `PEDAL`, `LIGHTGUN`, `KEYBOARD`, `MOUSE`, `MAHJONG`, `KEYPAD`, and `ALTERNATIVE`. Keep source, version, and confidence. A future controller-aware collection may filter on them, but a control tag alone should yield `SPECIAL_CONTROL_REQUIRED` or `REVIEW_REQUIRED`, not `READY`.

## H. Quality and playability

Objective or source-backed fields include runnable/device/BIOS flags, emulation status, driver status, sound/color/graphics status, protection status, and explicit mechanical/screenless/casino/mature/mahjong classifications. They still require source/version provenance.

Subjective or policy-dependent fields include “best,” “classic,” “lite,” “playable on a gamepad,” and “slow.” Arcade Manager’s base CSV and ProgettoSnaps-derived quality lists can be useful evidence, but neither should be promoted to identity truth. `working` and `imperfect` should remain separate, and source disagreement should remain visible.

## I. Curated collections

The companion data repository contains manually curated classics lists of approximately 50 or 200–250 entries, control-specific lists, clone/non-working filters, and quality lists sourced from ProgettoSnaps. The README/FILTERS material names sources such as Ranker, BMI Gaming, TechRadar, Arcade-Museum, ArcadeItalia, and ProgettoSnaps.

Classification:

| Source/list | Classification | EmuWiz treatment |
|---|---|---|
| MAME/listxml/DAT fields | `OBJECTIVE_METADATA` | Identity/dependency evidence with version/provenance. |
| ArcadeItalia CSV control/driver fields | `OBJECTIVE_METADATA` with source caveat | Typed overlay; not standalone readiness. |
| ProgettoSnaps quality data | `COMMUNITY/PROVIDER METADATA` | Optional quality evidence with provenance/license review. |
| Classics/best lists | `CURATED_OPINION` / `MANUAL_LIST` | Separate collection overlay only. |
| Ranker, BMI Gaming, TechRadar, Arcade-Museum selections | `COMMUNITY_RANKING` / `EDITORIAL_LIST` | Never identity or completeness evidence. |

EmuWiz should support collection overlays only if they cannot alter identity, dependency, or integrity verdicts. A collection membership can recommend a game; it cannot make an incomplete set ready.

## J. DAT, INI, CSV transformations

Arcade Manager merges/splits lists, converts DAT/INI, applies filters, imports/exports CSV, and creates list outputs for frontend/filter consumption. These are useful ideas for a future read-only export layer. They do not justify mutating the authoritative DAT or source library.

EmuWiz already has provider-neutral DAT models, Logiqx and ClrMamePro parsers, MAME listxml ingestion, DAT provenance/version fields, clone/dependency reports, archive/CHD verification, and deterministic plan/report seams. The genuine gap is not another DAT converter; it is a safe, source-preserving arcade view/export that records which source fields and version produced a filtered collection. Any future transformation must preserve the original DAT, source hashes, source version, and explicit filter policy.

## K. Proposed read-only Ready-to-Play model

| State | Required evidence |
|---|---|
| `READY` | Correct ecosystem; set storage and all declared dependencies verified; target ROM-set version compatible with the selected emulator; no known non-working flag; control requirements satisfied or intentionally accepted. |
| `READY_WITH_WORKING_CLONE` | Same as `READY`, but the elected candidate is a deterministic working clone because the preferred parent is known non-working/unavailable. Parent/dependency closure remains reported. |
| `MISSING_BIOS` | Dependency resolver identifies an unsatisfied BIOS requirement. |
| `MISSING_DEVICE_ROM` | Unsatisfied `device_ref` or device storage requirement. |
| `MISSING_PARENT_DEPENDENCY` | Unsatisfied parent, ROM-source, merge, sample, or other declared set dependency. |
| `MISSING_CHD` | Required disk or CHD parent cannot be verified. |
| `ROMSET_VERSION_MISMATCH` | DAT/set target is known and conflicts with the selected emulator’s target. |
| `EMULATOR_VERSION_UNKNOWN` | Emulator version cannot be safely established; do not claim strict ready. |
| `SPECIAL_CONTROL_REQUIRED` | Required control is known but no compatible cabinet/profile is evidenced. |
| `KNOWN_NOT_WORKING` | Source metadata explicitly reports non-working. |
| `IMPERFECT` | Source metadata reports imperfect but not non-working behavior. |
| `REVIEW_REQUIRED` | Ambiguous topology, partial scan, unsupported structure, conflicting metadata, or unresolved version/control evidence. |
| `UNSUPPORTED` | Ecosystem/format/packing/runtime feature is outside the supported evidence model. |

`READY` must not mean “the ZIP exists.” It is a closure and compatibility result. The existing `SetState::Complete` is storage/dependency evidence and must not be overloaded to mean runtime playability because BIOS variant selection is explicitly not modelled.

## L. Playing Library and 1G1R impact

**Answer: yes, use a dedicated typed arcade policy.** The architecture should reuse the existing family grouping, deterministic explanations, conflict handling, and transaction seams, but supply a policy equivalent to:

```text
Console1G1R:
  region/revision/language preference

Arcade:
  parent/clone topology
  runnable/working/imperfect status
  dependency closure
  ROM-set/emulator compatibility
  control capability/profile
  explicit user collection preferences
```

Do not create a second general election engine if the existing engine can accept a typed policy and arcade evidence. The arcade policy must refuse to elect a candidate when parent/clone identity is ambiguous, dependencies are incomplete, the source is partial, or multiple candidates have equal supported rank.

## M. Filter and copy safety rules

The following must be hard safety rules for any future planner:

- never delete or copy a parent/clone in isolation from its resolved dependency closure;
- never treat a working clone as permission to omit its parent, ROM source, BIOS, device, sample, or CHD requirement;
- never drop a BIOS or device archive merely because it is not itself a runnable game;
- never copy a ZIP without its required CHD and correct set-relative path;
- never infer merged/split/non-merged from filename alone;
- never mix MAME and FBNeo DAT/set evidence;
- never turn a partial scan into a complete or ready verdict;
- never apply a subjective list as an integrity filter;
- never mutate the source library during filtering/planning;
- preserve a before/after inventory and source hashes for any future conversion proposal.

## N. Overlays and bezels

Arcade Manager’s overlay/bezel installation is relevant to a future frontend publishing layer, not to ROM identity or readiness. It should be classified **USEFUL LATER**: discover and describe bezel/overlay metadata, source, target emulator/frontend, aspect ratio, and license; do not add downloads or installation in this audit or current arcade priority. A bezel must never be a dependency for deciding whether a ROM set is valid.

## O. Licensing and data provenance

Arcade Manager’s application is marked GPL-3.0 in its repository. The companion `arcade-manager-data` repository is marked MIT in its README. Those are different licenses and do not automatically license upstream ArcadeItalia, ProgettoSnaps, editorial lists, screenshots, overlays, or downloaded metadata. The cited MAME code and MAME data also have their own project licensing terms.

EmuWiz should implement the documented concepts independently. Do not copy Arcade Manager GPL source into EmuWiz unless the project’s licensing strategy explicitly permits that combination. Do not redistribute third-party curated lists or metadata without checking the upstream terms and attribution requirements. Preserve source URL, version, retrieval date, and license/provenance when optional metadata is used.

## P. EmuWiz gap matrix

| Feature | Arcade Manager approach | EmuWiz current support | Genuine gap? | Value | Risk | Recommendation |
|---|---|---|---:|---:|---:|---|
| Parent/clone topology | DAT/CSV clone filters and family views | `cloneof`/`romof`, graph, clone report, family roots | No structural gap | High | Wrong source mapping | **ALREADY COVERED**; add arcade policy consumer |
| Working-clone fallback | Keep working clone if parent non-working | No arcade election status/ranking | Yes, policy gap | High | Subjective/ambiguous status | **ADOPT** read-only deterministic fallback |
| BIOS/device/parent/CHD closure | Copy/filter support and release fixes | Distinct dependency resolver and CHD evidence | No major structural gap | High | Runtime selection boundary | **ALREADY COVERED**; expose readiness |
| Merged/split/non-merged detection | Non-merged conversion; other tools for conversion | DAT packing provenance, not arcade packing detector | Small detection/reporting gap | Medium | Misclassifying archives | **RESEARCH FURTHER**, read-only only |
| Set conversion | Converts/plans non-merged | Not implemented | Intentional | Medium | Duplication, corruption, licensing | **DO NOT ADOPT** now |
| Version compatibility | Version-specific lists and matching assumptions | MAME parser, FBNeo unknown-safe boundary, DAT provenance | Policy/readiness gap | High | False compatibility | **ADOPT** as readiness input |
| Controls | CSV categories and generated lists | No arcade capability overlay | Yes | Medium | Stale/subjective controls | **RESEARCH FURTHER**, typed evidence |
| Quality/working | Base CSV plus ProgettoSnaps quality | DAT metadata preserved, no arcade curation policy | Yes, overlay gap | Medium | Opinion presented as fact | **ADOPT** objective-only overlay |
| Curated classics/best | Manual lists and external rankings | No collection overlay | Yes, optional | Low/medium | License and authority confusion | **USEFUL LATER** |
| DAT/INI/CSV list transforms | Merge/split/filter/export | Strong DAT parsing/provenance; no arcade view exporter | Small export gap | Medium | Losing provenance | **RESEARCH FURTHER** |
| Emulator environment | Assumes target version/set | MAME and FBNeo environment seams | No foundation gap | High | Unknown FBNeo version | **ALREADY COVERED**; feed readiness |
| Overlays/bezels | Install packs | No feature | Not current priority | Low | Downloads/licensing | **USEFUL LATER** |

## Q. Explicit answers

1. **Does EmuWiz need a dedicated arcade election policy?** Yes. Reuse the existing engine/seams, but do not apply console regional/revision 1G1R rules to arcade families.
2. **Is working-clone fallback a genuine missing feature?** Yes, as a read-only arcade policy. The underlying clone and status evidence exists or can be carried; the deterministic election state does not.
3. **Does current dependency/topology handling cover BIOS/device/parent/CHD requirements?** Structurally, yes, with the documented boundary that runtime BIOS selection is not modelled. The genuine gap is presentation/election/readiness, not another resolver.
4. **Should EmuWiz support merged/split/non-merged detection?** Yes, read-only detection/planning is useful when proven from archive contents and DAT relationships.
5. **Should EmuWiz perform conversion?** No in the current roadmap. Preserve bytes and source; use established specialist tooling if a user explicitly chooses conversion later.
6. **Should ROM-set version vs emulator version become a Ready-to-Play check?** Yes. Both should be recorded and compared as a compatibility tuple; unknown must remain reviewable.
7. **Is controls metadata reliable enough for controller-aware collections?** Reliable enough as versioned, sourced filtering evidence; not reliable enough by itself to prove runtime playability.
8. **Which filters are genuinely useful?** Parent/clone, explicit working/non-working, dependency completeness, ROM-set version, objective control capabilities, and source-backed driver flags. “Best,” “classic,” “lite,” and gamepad-playable lists are subjective overlays.
9. **Top five arcade-specific gaps worth implementing:** (a) typed arcade election policy; (b) working-clone fallback with deterministic review behavior; (c) combined ROM-set/emulator compatibility readiness; (d) dependency-closure projection into selected candidates; (e) versioned typed controls/quality overlays that cannot override integrity.
10. **What should EmuWiz explicitly not copy?** GPL implementation code; unverified list-generation assumptions; destructive filtering/conversion; community downloads; subjective lists as truth; filename-only set semantics; MAME/FBNeo conflation; and overlay installation/download workflows.

## Ranked recommendations

### ADOPT

1. Add a typed arcade policy over existing family/dependency evidence.
2. Add deterministic working-clone fallback and explicit `REVIEW_REQUIRED` ties.
3. Feed DAT target, emulator family/version, and compatibility evidence into Ready-to-Play.
4. Project dependency closure and control requirements into read-only plan explanations.

### RESEARCH FURTHER

1. Prove merged/split/non-merged detection from archive/member and DAT evidence.
2. Define a versioned ArcadeItalia/MAME control capability schema and provenance contract.
3. Confirm full Arcade Manager source paths and pin the full upstream commit before any implementation comparison.
4. Define a lossless filtered-list export format that records source and policy.

### ALREADY COVERED

- DAT provider/version provenance.
- MAME/FBNeo ecosystem distinction and version parsing boundaries.
- Parent/clone and ROM-source graph resolution.
- Merge-member, BIOS, device, sample, and CHD-parent dependency resolution.
- Ambiguity/cycle/partial-scan fail-closed behavior.
- Direct archive and CHD integrity evidence.
- Deterministic Playing Library explanations and no-clobber planning seams.

### USEFUL LATER

- Separate curated collection overlays.
- Bezel/overlay discovery and frontend metadata.
- Read-only archive packing reports.
- Export adapters for frontend list formats.

### DO NOT ADOPT

- Arcade Manager GPL code copied into EmuWiz without an explicit licensing decision.
- Default non-merged conversion or automatic merged/split conversion.
- Deleting parents, BIOS, devices, samples, or CHDs based on a filtered list.
- Community downloads or automatic community-save/list imports.
- Subjective classics/best/gamepad lists as identity, completeness, or readiness evidence.
- A single global “working” score that silently ranks incomparable clones.
- Treating MAME and FBNeo sets as interchangeable.
- Overlay/bezel downloads or installation in the current priority.

## Genuine gaps only

The audit found four material gaps rather than a need to rewrite existing DAT/1G1R logic:

1. No dedicated arcade election policy consuming existing topology/status/dependency evidence.
2. No working-clone fallback state with explicit tie/review behavior.
3. No unified arcade Ready-to-Play compatibility result combining ROM-set target and emulator target.
4. No versioned typed control/curation overlay model that is explicitly prevented from overriding integrity evidence.

Archive packing detection, curated lists, list conversion, and overlays are useful future work but are not prerequisites for safe structural arcade auditing.

## Phase and safety decision

This audit does **not** authorize ROM-set conversion, copy/delete operations, downloaders, or GUI changes. The next safe step is a read-only Phase 3-style arcade policy design using the current DAT/dependency reports. Any later mutation must preserve the source library, calculate a complete dependency closure, record before/after hashes, and require explicit user approval.
