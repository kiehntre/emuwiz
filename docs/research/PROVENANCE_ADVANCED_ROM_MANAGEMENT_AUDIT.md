# Provenance advanced ROM management audit for EmuWiz

Research-only product audit. No production code, Media Sets, Save Vault,
Publisher Profiles, conversion, patching, playlist files, or metadata were
changed.

## Scope and sources

The primary source was Provenance's current documentation, inspected 2026-09-14:

- [Advanced ROM Management](https://wiki.provenance-emu.com/using-provenance/roms/advanced-management)
- [Formatting ROMs](https://wiki.provenance-emu.com/using-provenance/roms/formatting-roms)
- [Customizing ROMs](https://wiki.provenance-emu.com/using-provenance/roms/customizing-roms)
- [Importing ROMs](https://wiki.provenance-emu.com/using-provenance/importing-roms)
- [Modding ROMs](https://wiki.provenance-emu.com/installation-and-usage/roms/mods)
- [Restoring Files](https://wiki.provenance-emu.com/advanced/restoring-files)
- [Troubleshooting](https://wiki.provenance-emu.com/help-and-community/troubleshooting)

The documentation is a user guide, not a complete implementation specification.
Claims below are therefore separated into documented behavior and EmuWiz
recommendations. The advanced page contains a few operationally optimistic
claims—such as deleting originals after checking that a CHD loads—which are not
treated as preservation or identity guarantees.

## Executive conclusion

Provenance has a clearer consumer UX for “one game, several discs”: the M3U is
the visible/launchable grouping object, and the emulator pause menu exposes
disc changing. Its guidance covers PlayStation, Sega CD, Saturn, PC Engine
CD/TurboGrafx-CD, and PC-FX, and the supported-format page also documents
multi-file Saturn, Sega CD, PC-FX, PC Engine CD, and PlayStation layouts.

EmuWiz is stronger underneath. Its `media_set` model distinguishes release,
medium, representation, role, ordinal, side, availability, confidence,
equivalence, and conflicts. Its M3U parser is bounded and rejects absolute or
parent-traversing references; M3U order is supporting evidence, not release
identity. Its launch projection already has a declarative `MediaSwapPlan` and
keeps topology separate from emulator readiness.

The justified product work is therefore presentation and workflow:

- show one release card with an expandable, ordered media list;
- make the existing M3U relationship visible and explain missing/conflicting
  members;
- add a read-only storage/savings projection;
- add display metadata overrides that never rewrite identity evidence;
- model patched content as a linked derivative when Mods/Patches work resumes;
- offer conversion savings previews only inside the existing analyse → verify
  → reference-update → cleanup boundary.

## A. Multi-disc UX

### Provenance’s documented pattern

Provenance says a plain-text `.m3u` groups a multi-disc game into a single
library entry with disc-swapping support. The example lists one disc image per
line, and the supported systems are PlayStation, Sega CD, Saturn, PC Engine
CD/TurboGrafx-CD, and PC-FX. The formatting guide says multi-disc games must
include an M3U in the archive and that disc filenames use the exact `(Disc #)`
convention. [Formatting ROMs](https://wiki.provenance-emu.com/using-provenance/roms/formatting-roms#multi-disc-games)

The documented archive shape is effectively:

```text
Final Fantasy VII (USA) (Disc 1).cue
Final Fantasy VII (USA) (Disc 1).bin
Final Fantasy VII (USA) (Disc 2).cue
Final Fantasy VII (USA) (Disc 2).bin
Final Fantasy VII (USA).m3u
```

The M3U is the user-facing group and launch entry; the CUE/BIN or CHD files
remain the actual media. The guide describes a pause-menu “Change Disc” action
with an ordered disc list. Artwork and metadata are discussed at the game
entry level, not as separate artwork records for each disc. A disc is a
component of the game presentation, not an independent title.

The documentation does not establish a general manifest format, a strong
release key, a complete-media validator, or a distinction between required,
optional, bonus, and data discs. It also gives users file naming rules that
can be confused with identity rules.

### EmuWiz comparison

EmuWiz already has the correct underlying abstraction in `media_set/model.rs`:
`MediaSetIdentity`, `MediaSetMember`, `MediaEvidence`, `MediaOrdinal`,
`MediaRole`, `ExpectedCount`, `MediaAvailability`, `MediaSetState`,
`Equivalence`, and explicit relationships. The existing topology documentation
also records that a CUE track is a companion, not another disc, and that M3U
order cannot prove release identity.

The existing `platform_evidence_fusion/cue_m3u_parsing.rs` and
`media_set/inspect.rs` parse M3U/CUE references read-only and within a fixed
budget. `media_set/plan.rs` and the launch integration project ordered media
and swap semantics without creating a second topology engine.

Therefore EmuWiz’s backend is already stronger than Provenance’s documented
model. EmuWiz’s product surface can still be better: the primary card should
be the release, with “3 discs / 3 present / ready” and an expandable list of
ordered members, representations, and blockers. Individual files should be
reachable from the detail view but should not compete with the game in the
main library.

### UX patterns worth adopting

- A single release row/card with `Disc 1`, `Disc 2`, etc. beneath it.
- A visible start member and a “change media” action based on the existing
  `MediaSwapPlan`.
- Per-member state: present, missing, conflicting, unverified, or alternate
  representation.
- Shared title/artwork at release level, with optional per-disc filename and
  format details.
- A compact completeness summary and an explanation when a group is not
  launch-safe.

Do not adopt Provenance’s implication that filename numbering alone proves the
group or that a disc loading successfully proves a safe replacement.

## B. M3U / playlist handling

### Documented syntax and behavior

Provenance’s examples are one relative filename per line, in disc order. The
formatting guide says the M3U must contain exactly all and only the `.cue` or
`.ccd` files for the game; the advanced guide also shows `.chd` lines. Its
examples use sibling filenames, not absolute paths. The M3U filename is
independent of the disc filenames, although a shortened game name is
recommended. [Advanced ROM Management](https://wiki.provenance-emu.com/using-provenance/roms/advanced-management#multi-disc-games-advanced)

The guide says to import the M3U and all disc files. Missing discs are treated
operationally as a broken/incomplete import rather than as an explicitly
modeled partial set. A historic issue also illustrates that multi-disc import
is user-sensitive and that archive folder layout matters.

### EmuWiz classification

| Capability | Finding | Classification |
|---|---|---|
| M3U discovery | Existing bounded parser and explicit descriptor inspection already discover references | ALREADY COVERED |
| M3U validation | Existing parser rejects absolute paths, `..`, empty lines, cycles, rejected references, and over-budget inputs; completeness remains evidence-gated | ALREADY COVERED |
| M3U ordering | Existing order becomes medium ordinal metadata, explicitly not release identity | ALREADY COVERED |
| M3U generation | No writer is present, deliberately; generation would be a write/mutation feature | RESEARCH FURTHER |
| M3U repair | Safe repair needs an already-proven set, canonical sibling paths, and an explicit user-approved write | RESEARCH FURTHER |
| M3U as primary launch entry | The media-set/launch projection can represent this, but GUI presentation and final emulator-specific serialization remain future work | ADOPT presentation only |
| Absolute/path-traversal references | Provenance examples do not define a security policy; EmuWiz already fails closed | DO NOT WEAKEN |

Recommendation: eventually support discovery and validation as first-class UI,
and generate M3U only as an explicit, reviewable projection from an already
resolved complete set. Do not make a free-standing playlist writer the source
of truth. Repair should never guess missing discs or silently rewrite a CUE.

## C. CHD and storage-management UX

### Provenance’s workflow

Provenance presents CHD as a lossless compressed disc format, gives illustrative
40–70% savings, and recommends `chdman createcd` for BIN/CUE conversion. The
documented post-conversion steps are: verify that the CHD loads, delete the
original BIN/CUE, and update M3U references. It emphasizes one-file storage and
lists PlayStation, Sega CD, Saturn, and Dreamcast as compatible examples.

This is a useful user journey but an insufficient preservation workflow. “The
emulator loads it” is not proof that all tracks, indexes, pregaps, subchannel
data, GD/DVD topology, or source identity survived. The batch shell example is
also unsafe as a general workflow because it does not show per-input topology
checks, output collision checks, verification, or playlist transactionality.

### EmuWiz flow to adopt

The product UX should expose:

`ANALYSE → ESTIMATE SAVINGS → CONVERT → VERIFY TOPOLOGY/CONTENT → UPDATE REFERENCES → OFFER ORIGINAL CLEANUP`

The existing `repair/optical_conversion.rs` is already aligned with the safety
boundary: it accepts a deliberately narrow supported CUE/BIN topology, stages
the output, and compares canonical optical fingerprints before the repair
transaction can finalize. The UI should surface that rigor rather than replace
it:

- show source members, topology, and current physical/logical sizes;
- estimate savings before any write;
- identify unsupported multi-track, GD-ROM, DVD, subchannel, or ambiguous cases;
- show verification scope and result in plain language;
- update only references proven to point to the converted representation;
- keep the original by default, or offer quarantine/cleanup only after a
  verified transaction and explicit confirmation.

No new conversion engine is justified by Provenance’s UX.

## D. Storage Health / Space Savings

Provenance exposes practical advice rather than a dedicated storage model: use
system folders, convert large disc images to CHD, delete duplicate regions,
optimize artwork, clear cache, and inspect the operating system’s storage page.
It gives example system totals and says CHD can make sync faster. It does not
appear to distinguish logical bytes, physical allocated bytes, hardlinks,
reflinks, duplicate content, or conversion eligibility as separate facts.

EmuWiz should eventually expose a read-only `Storage Health / Space Savings`
view. It should distinguish:

- logical content size;
- measured physical allocation when the filesystem reports it;
- representations already compressed;
- conversion candidates and estimated savings;
- duplicate content and duplicate representations;
- hardlink/reflink/shared-storage relationships;
- media-set completeness and reference impact;
- backup/export size by scope.

This view should be advisory. It must not equate “large” with “safe to delete,”
or “duplicate hash” with “safe to remove” where references, variants, shared
containers, or Save Vault boundaries differ.

Recommendation: high-value future product work, but needs a measured storage
model and explicit cleanup transaction design before implementation.

## E. Metadata matching

Provenance documents OpenVGDB matching and recommends No-Intro or Redump names,
region codes, revision markers, and removal of hack tags to improve matching.
It also acknowledges that failed checksums, translations, hacks, and homebrew
will not match automatically. Filename-based artwork matching is exact by
basename, and its troubleshooting guidance warns that loose `.bin` files can
be detected as the wrong system.

The simple UX lesson is valuable: show users what improves a match, expose a
clear “refresh metadata” action, and provide a concise explanation when a
checksum or naming mismatch prevents automatic enrichment.

The identity lesson is not transferable. EmuWiz’s evidence-resolution and DAT
layers must remain authoritative over filename-only matching. A No-Intro or
Redump name is useful as a naming/display hint; a verified DAT/hash result is a
different evidence class. M3U order and a clean title cannot repair a conflicting
native identity.

## F. Manual metadata overrides

Provenance lets users edit title, description, genre, publisher, developer,
release date, region, and artwork from Game Info/Edit flows. It supports reset
of editable fields, while play history can be reset but not manually edited.
The documentation also records that custom names and artwork may be lost on a
library refresh, which is an important migration/retention weakness.

EmuWiz should separate two layers:

| Layer | Meaning | Can a user override it? |
|---|---|---|
| Identity evidence | What bytes, topology, DAT, or trusted source support | No; user can annotate, dispute, or request review, but not turn a claim into evidence |
| Display metadata | How a proven/ambiguous item is presented | Yes, with source, timestamp, scope, and reset path |
| Platform selection | Which emulator/system view is used | Only as an explicit review/manual selection, never as retroactive proof |
| Group association | Which release/media set a file is shown under | Yes only as a visible user association, retaining conflict and provenance |

The correct UX is “Display this as X” or “Associate with this release,” not
“This file is proven to be X.” Overrides must survive rescans unless the user
removes them, while the underlying evidence report remains unchanged.

## G. Patched and hacked games

Provenance describes fan translations, ROM hacks, bug fixes, and texture packs.
Its workflow is external-tool based: verify the target checksum where possible,
apply a patch to an output path, import the patched ROM, edit its metadata, and
add artwork. It suggests descriptive names such as an original title followed
by a patch/hack name. It does not document a formal base-game/derivative
relationship, patch identifier, patch version, or cryptographic output record.

The concept fits EmuWiz’s future Mods/Patches work, but should be more explicit:

```text
BASE GAME
  └── DERIVED/PATCHED VARIANT
        ├── PATCH IDENTITY
        ├── PATCH PROVENANCE
        ├── PATCH VERSION
        ├── EXPECTED BASE HASH
        └── OUTPUT HASH / verification state
```

The original remains the authoritative base. The derivative is a separate
launchable content object with a link, not a replacement that changes the base
identity. A failed or absent hash match must remain `REVIEW_REQUIRED`; a
successful patch process is not itself proof that the output is correct.

## H. Patch application safety

The Provenance guidance explicitly recommends saving the patched output with a
descriptive name and importing it afterward. It does not document rollback,
automatic base preservation, output hashing, patch signature verification, or
metadata transactionality. It lists IPS, BPS, UPS, PPF, BSDiff/BDF, XDelta, and
DAT-style tools, plus PSX SBI files and N64 texture packs, but these are
different artifact classes and should not be flattened into one patch type.

EmuWiz’s safe future workflow remains:

`IDENTIFY BASE → VERIFY EXPECTED HASH → APPLY PATCH TO NEW OUTPUT → VERIFY OUTPUT → LINK DERIVED GAME TO BASE → PRESERVE ORIGINAL`

Useful Provenance ideas are descriptive naming, patch-format education, and
manual metadata/artwork for unmatched derivatives. Do not copy its implicit
“import and edit” model as the identity system.

## I. Large-library organization

Provenance recommends system folders, consistent No-Intro/Redump naming,
search by title/system/genre, favorites, recently played filters, and
system-specific views. It advises avoiding mass upload of multi-disc sets and
keeping archive contents flat because folder-in-archive layouts can break
imports. Its user-facing categories are intentionally simple.

EmuWiz already has stronger domain concepts: source/discovery separation,
Media Sets, evidence-ranked grouping, Playing Library projections, Ready-to-Play
and launch readiness vocabulary, DAT coverage, duplicate taxonomy, and bounded
library planning. The UX opportunity is to make those concepts feel simple:

- `Ready to Play` should show a release once, with media/dependency status;
- `Playing Library` should hide companion files by default;
- `Smart Collections` should filter by evidence, completeness, platform,
  format, storage, favorites, and attention state;
- duplicates and alternate representations should be visible in detail, not
  mistaken for separate games;
- multi-disc and patched variants should have explicit badges/relationships.

## J. Backup and migration scope

Provenance’s documented backup includes ROMs, save states, battery saves, BIOS,
custom artwork, and the metadata database. Its restore guide separately notes
controller skins and warns that save states may not survive app/core version
changes. Settings and controller mappings are not included in the advanced
page’s iCloud sync list, while the restore guide presents a broader app-data
inventory. This difference is a reason to show scope explicitly rather than
promise “everything.”

The useful EmuWiz product pattern is a preflight summary:

```text
WILL MOVE / BACK UP
  game content and representations
  media-set descriptors and playlists
  identity/evidence records
  display metadata and artwork
  emulator profiles/settings (if selected)
  Save Vault snapshots (separately identified)

WILL NOT MOVE / NEEDS REBUILD
  external tool installations
  machine-specific executable bindings
  unavailable source files
  secrets or device-specific credentials
  state formats known incompatible with the destination
```

This belongs in a future migration/export workflow and must not alter Save Vault
semantics. The summary should report counts, logical bytes, physical bytes when
known, unresolved references, and compatibility caveats before the user starts.

## K. iCloud and sync concepts

Provenance documents syncing ROM files, save states, battery saves, custom
artwork, metadata edits, BIOS files, and skins through iCloud/CloudKit, while
excluding app settings and controller mappings in the advanced guide. The
troubleshooting guidance says users should avoid simultaneous competing app
variants, let one device finish first, and that conflicting saves use the most
recent save. The restore guide distinguishes actual battery saves from
version-sensitive save states.

Useful later:

- a visible distinction between library-content sync and progress/save sync;
- sync scope and last-success status;
- per-artifact compatibility warnings;
- explicit conflict policy and a recoverable history.

Not relevant now: adopting iCloud, CloudKit, or a cloud vendor as an EmuWiz
requirement. Do not adopt “most recent wins” for identity, metadata, or media
topology conflicts; those need reviewable, provenance-preserving merges.

## L. EmuWiz comparison matrix

| Feature | Provenance approach | EmuWiz current state | Provenance UX better? | EmuWiz backend stronger? | Genuine gap? | Recommendation |
|---|---|---|---|---|---|---|
| Multi-disc presentation | M3U becomes one library entry; pause-menu disc list | Media Set and MediaSwapPlan already model ordered members; presentation is not the main library surface | Yes | Yes | Yes, presentation polish | HIGH VALUE / LOW RISK |
| M3U discovery | User creates/imports plain text playlist | Bounded read-only discovery and reference expansion | Simpler instructions | Yes | No | ALREADY COVERED |
| M3U validation | Exact filenames and all discs required operationally | Safe relative resolution, traversal/cycle/budget refusal, evidence-gated completeness | No | Yes | No | ALREADY COVERED |
| M3U generation | User creates text manually | No writer; topology is authoritative | No | Yes | Future convenience only | RESEARCH FURTHER |
| CHD conversion | `createcd`, load-test, delete originals, update M3U | Narrow staged conversion with canonical fingerprint verification | Provenance has clearer story | Yes | UX only | HIGH VALUE / NEEDS RESEARCH |
| Storage visibility | OS storage page, examples, tips | Duplicate/physical-storage foundations exist, but no unified savings view | No dedicated view | Yes | Yes | HIGH VALUE / NEEDS RESEARCH |
| Metadata matching | OpenVGDB plus names and hashes | Evidence/DAT resolution and identity lineage | Yes for novice guidance | Yes | Explanation polish | HIGH VALUE / LOW RISK |
| Manual overrides | Edit title/artwork/fields directly | Evidence and display layers are conceptually separate; override UI is future | Yes | Yes | Yes, if scoped display-only | HIGH VALUE / LOW RISK |
| Patched games | Import derived file, rename/edit metadata | Patch/cheat infrastructure exists, but no universal derivative identity model | No | Yes | Yes for future Mods/Patches | HIGH VALUE / NEEDS RESEARCH |
| Library organization | Folders, search, favorites, system views | Smart Collections, Playing Library, Media Sets and readiness vocabulary | Simpler surface | Yes | Presentation integration | HIGH VALUE / LOW RISK |
| Backup scope | Files plus database/artwork/saves, with caveats | Migration scope is not one unified user-facing summary | Yes | N/A | Yes | HIGH VALUE / NEEDS RESEARCH |
| Cloud sync | ROMs, saves, BIOS, artwork, metadata, skins; limited conflict policy | No equivalent required | Yes for convenience | N/A | Not a current requirement | USEFUL LATER |

## M. Explicit answers

1. **Is EmuWiz’s backend already stronger for multi-disc identity?** Yes. Its typed topology/evidence model distinguishes release identity from medium, representation, ordinal, role, and conflict; Provenance’s documentation centers on M3U grouping and names.
2. **Does EmuWiz need better multi-disc GUI presentation?** Yes. Show one release with expandable ordered media and clear completeness/readiness state.
3. **Should EmuWiz support M3U generation, or only validation/discovery?** Validation/discovery should remain the foundation. Generation is a later explicit projection from a resolved set, never the source of truth; repair needs separate research.
4. **What should EmuWiz borrow from Provenance’s CHD UX?** Pre-conversion savings preview, simple status language, visible reference-update impact, and an explicit cleanup choice—while retaining topology/content verification and rollback-safe transactions.
5. **Should EmuWiz add Storage Health / Space Savings?** Yes, eventually. It is high value but needs measured logical/physical/shared-storage semantics and must remain read-only initially.
6. **How should manual metadata overrides coexist with evidence identity?** Store them as scoped, provenance-bearing display projections. Never mutate or replace evidence; show “displayed as” separately from “identified as.”
7. **Should patched games be explicit derivatives?** Yes. Link a derived variant to a preserved base with patch identity, expected base hash, provenance, version, and output verification state.
8. **Which Provenance ideas fit future Mods/Patches?** Checksum-target education, descriptive derived naming, separate artwork, and explicit distinction between translation, hack, bug fix, texture pack, and support artifact.
9. **Which fit Ready-to-Play / Smart Collections?** One-row release presentation, media completeness badges, filters for ready/incomplete/conflicting/derived content, favorites, recently played, storage cost, and required dependencies.
10. **What should EmuWiz explicitly not copy?** Filename-only identity, “loads therefore safe to delete originals,” destructive cleanup defaults, implicit patch provenance, cloud “most recent wins” for identity conflicts, or a second topology engine.

## N. Ranked roadmap recommendations

| Candidate | Rank | Justification |
|---|---|---|
| Multi-disc presentation polish | HIGH VALUE / LOW RISK | UX gap only; consume existing MediaSet/MediaSwapPlan. |
| Display metadata overrides | HIGH VALUE / LOW RISK | Clear user value if kept separate from evidence and made durable. |
| Metadata-match explanations | HIGH VALUE / LOW RISK | Borrow Provenance’s simple remediation language without weakening identity. |
| Conversion savings preview | HIGH VALUE / NEEDS RESEARCH | Useful front door to existing safe conversion; must display topology/reference consequences. |
| Storage Health / Space Savings | HIGH VALUE / NEEDS RESEARCH | Valuable for large libraries; requires physical/shared-storage measurement policy. |
| Derived/patched-game model | HIGH VALUE / NEEDS RESEARCH | Important for Mods/Patches; needs base/output hash and provenance schema. |
| Backup/migration scope preview | HIGH VALUE / NEEDS RESEARCH | Prevents ambiguous “backup everything” promises and protects Save Vault boundaries. |
| M3U generation | USEFUL LATER | Convenience projection after complete-set resolution; not a backend identity feature. |
| M3U repair | USEFUL LATER | Only with explicit, bounded, reviewable repairs; never infer missing media. |
| iCloud/cloud sync | USEFUL LATER | Product-specific infrastructure, not justified by this UX audit. |
| Provenance-style filename identity | NOT WORTH BUILDING | Contradicts EmuWiz’s evidence model and creates false positives. |

## O. Explicit DO-NOT-ADOPT list

- Do not make a manually authored M3U authoritative for release identity.
- Do not flatten CUE/BIN, CHD, GDI, or other media topology into a single file
  identity merely because an emulator opens it.
- Do not delete originals immediately after a load test.
- Do not use filename-only matching to override native/DAT evidence.
- Do not store manual display edits as if they were byte-derived facts.
- Do not replace a base game with a patched output; retain a linked derivative.
- Do not use “most recent wins” for identity, topology, or metadata conflicts.
- Do not introduce a second media-set/topology engine.
- Do not treat ROMs, saves, BIOS, artwork, settings, playlists, and emulator
  state as one undifferentiated backup object.
- Do not adopt cloud sync or Provenance’s application/storage assumptions as an
  EmuWiz requirement.

## Final result

The strongest justified product change is a release-centric multi-disc detail
surface backed by the existing Media Set and launch-plan models. The strongest
future platform feature is a read-only Storage Health / Space Savings view,
followed by durable display overrides and an explicit base/derived model for
patched content. Provenance is a useful UX reference, but EmuWiz should retain
its stricter identity, topology, provenance, and fail-closed rules.

