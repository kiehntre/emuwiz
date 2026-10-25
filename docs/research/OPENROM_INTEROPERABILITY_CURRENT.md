# Current OpenROM interoperability assessment

Research date: 2026-10-03 UTC. **Import deferred; no production parser or behaviour change.**

OpenROM currently offers useful local-file observations, but not a versioned
catalogue interchange contract. Its command-specific JSON has no schema identifier
or schema version. Treating arbitrary saved CLI output as a supported catalogue
would require guessing its producer, operation, version and evidence scope. That
fails this task's explicit versioning and provenance requirements. This document
records the current format and the conditions for a future read-only adapter;
it does not invent an OpenROM schema.

## Revisions and scope

| Item | Revision / observation |
|---|---|
| Authoritative EmuWiz main and origin/main at start | `6c584b3152e0dd581138fd3ac9f394302b904eba` |
| Live origin main at initial network check, without fetching or moving refs | Same starting SHA |
| Main/origin/main at final review; candidate parent | `1b7a467d7006d31ca7b25146aae0682cc2771693` |
| EmuWiz tracked main tree | Clean; existing untracked research/cache files preserved |
| Research branch | `feature/openrom-interoperability-foundation` |
| Worktree | `/home/davedap/emuwiz-openrom-interoperability-foundation` |
| Current upstream studied | [M5Devs/OpenROM main](https://github.com/M5Devs/OpenROM/tree/d9434b9385392d3a9e845b4f24082e543ee3c043), `d9434b9385392d3a9e845b4f24082e543ee3c043` |
| Upstream head timestamp and subject | 2026-10-02 22:54:13 UTC; SSP patch-builder implementation, PR #118 |
| Latest published non-prerelease | [v3.6.1](https://github.com/M5Devs/OpenROM/releases/tag/v3.6.1), published 2026-09-26 02:49:33 UTC |
| Release tag commit | `b0c5c083603cc2c20f80ad960ce77edd4ec4ddae` |
| Head relative to that release | 13 commits ahead, as reported by GitHub's compare API |
| Supported OpenROM import schemas | **NONE** |
| Changed-file scope | This research document only |

GitHub's current API, rather than cached search summaries, supplied the head,
release, complete 266-entry tree and recent commit history. Bounded read-only
downloads of 35 source/reference files totalled 238,263 bytes. No upstream
executable, GUI or conversion operation was run. The existing
[v2.7.0 audit](OPENROM_V270_PLATFORM_DETECTION_AUDIT.md) is historical context,
not evidence of the current Dart implementation.

Another lane advanced main/origin/main during research with
`1b7a467d7006d31ca7b25146aae0682cc2771693`, the PS4 extracted-folder evidence
fusion commit. Its only change to the inspected model files makes an existing
PS4 path-safety helper `pub(crate)`; the mapped types and authority rules remain
unchanged. The research branch was rebased onto that main before committing.
This task did not move main or origin/main; its diff against the updated main
contains this document only.

## What the current project provides

OpenROM is an offline ROM conversion, compression, patching and file-management
toolkit with a Dart core/headless CLI and Flutter desktop application. It
consumes third-party No-Intro/Redump DATs for its renamer; it is not an independent
preservation catalogue or game-metadata publisher. The inspected tree contains
no game database, catalogue dataset or catalogue JSON Schema. Theme JSON,
application assets and packaging manifests describe the application, not games.
[Current README](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/README.md).

The repository was created on 2026-08-07 and remains active, with feature work on
2026-10-01/02 and several September releases. It is a young, rapidly evolving
toolkit. Activity is established; a stable metadata interchange contract is not.
This maturity judgement is an inference from the dated history and inspected
interfaces, not a claim that all its tools are unstable.
[Repository API](https://api.github.com/repos/M5Devs/OpenROM),
[pinned commit](https://api.github.com/repos/M5Devs/OpenROM/commits/d9434b9385392d3a9e845b4f24082e543ee3c043).

### Actual data surfaces

| Surface | Shape and meaning | Import verdict |
|---|---|---|
| `--json --detect FILE` | One JSON object describing a local file/container; no envelope or schema version | External observation only; no supported import contract |
| `--json --scan-roms FOLDER` | Line-oriented progress/result JSON; result records differ on match, non-match and error | Not a catalogue export; record provenance is incomplete |
| `--import-dat` / `--list-dats` | Operation-result JSON and DAT-header summaries | Original DAT remains the source; use existing DAT paths independently |
| `--json --read-ipbin FILE` | `done`/`success` object containing named header fields | Header claims require independent structural/offset verification |
| `--read-gdi FILE` | Bare JSON array of track summaries, unlike the other response shapes | Track hints only; lossy descriptor projection |
| M3U generation | Text playlist in caller-supplied order; JSON reports the output path | Existing playlist/media paths own interpretation |
| Conversion/verification output | Progress, log and completion events | Operation provenance; completion is not DAT identity |

These are operational interfaces, not one universal JSON record format.
[CLI dispatch and serialization](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/bin/openrom.dart),
[GDI serializer](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/lib/src/gdi_reader.dart),
[M3U generator](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/lib/src/m3u_generator.dart).

### Versioning is an implementation blocker

At the exact studied head:

| Version surface | Value |
|---|---|
| Latest public release tag | `v3.6.1` |
| Root `VERSION` | `3.5.0` |
| GUI package | `3.5.0+1` |
| Core package | `3.0.0` |
| CLI fallback when working-directory `VERSION` is absent | `v3.0.0` |
| Data/protocol/schema version in the inspected responses | Absent |

The CLI reads `VERSION` relative to its working directory, so its displayed
application version cannot bind a saved response to a schema. No inspected
response supplies a producer revision, schema discriminator, source snapshot
digest or stable record identifier. An application release number must not be
substituted for a data-schema version.
[VERSION](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/VERSION),
[core package](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/pubspec.yaml),
[GUI package](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/gui/pubspec.yaml),
[CLI version handling](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/bin/openrom.dart#L94).

A future importer must accept only an actually published, studied schema.
Unknown/newer declared versions must yield `UNSUPPORTED_VERSION`. Legacy
unversioned responses must remain unsupported; do not label them an invented
version 1 or infer a version from their keys. No such refusal API was added in
this research-only change.

## EmuWiz models and authority boundary

Read-only inspection used these current-main models:

| Model | Existing responsibility / reuse boundary |
|---|---|
| [game_identity.rs](../../crates/archivefs-core/src/game_identity.rs) | `GameIdentityReport`, typed `IdentityEvidence`, confidence/status and file/member/method provenance from local inspection |
| [launch/input_projection.rs](../../crates/archivefs-core/src/launch/input_projection.rs) | `VerifiedIdentityFact` contains already-verified platform-specific facts; external strings cannot create one |
| [verified_identity_cache.rs](../../crates/archivefs-core/src/verified_identity_cache.rs) | Persisted facts are bound to device/inode/size/mtime and freshness; a cache row never independently authorizes an operation |
| [dat/model.rs](../../crates/archivefs-core/src/dat/model.rs) | DAT source/game/ROM/disk declarations; CRC32, MD5, SHA-1 and SHA-256 checksum validation; original ecosystem/source remains authoritative for its own claims |
| [dat/identity.rs](../../crates/archivefs-core/src/dat/identity.rs) | DAT-source platform evidence differs from per-game identity and cannot authorize renames |
| [platform/mod.rs](../../crates/archivefs-core/src/platform/mod.rs) | Single canonical platform registry; exact IDs and whole normalized aliases; no fuzzy substring matching |
| [identity_source/model.rs](../../crates/archivefs-core/src/identity_source/model.rs) | External records, algorithm-tagged hashes, confidence and retained conflicts; external evidence never outranks verified local identity |
| [identity_source/matching.rs](../../crates/archivefs-core/src/identity_source/matching.rs) | Current path/file/platform/hash comparison; presently RomM-oriented, so not a drop-in OpenROM adapter |
| [media_set/model.rs](../../crates/archivefs-core/src/media_set/model.rs) | Media identities, explicit ordinals, variants, provenance, confidence and conflicts; metadata is weaker than trusted DAT/native evidence |
| [metadata_aggregation.rs](../../crates/archivefs-core/src/metadata_aggregation.rs) | Descriptive candidates and conflicts; field-specific local overrides outrank provider metadata and verified identity outranks provider cache |
| [dat/custom_dat.rs](../../crates/archivefs-core/src/dat/custom_dat.rs) | Local DAT syntax/header does not establish official source authority; stronger official evidence is preserved or a conflict surfaced |
| [identity_source/managed_snapshot.rs](../../crates/archivefs-core/src/identity_source/managed_snapshot.rs) | Existing immutable source lifecycle with parser-version/trust/source/digest provenance; activation is separate from parsing |

Initial OpenROM authority must be **EXTERNAL PROVENANCE / IDENTITY HINT**.
An exact platform-label mapping is not verified platform evidence. Agreement
with a locally verified, same-scope hash can corroborate that match; a CRC-only
claim cannot establish a unique release. Disagreement must retain both claims.
Never replace verified DAT identity, source authority, catalogue-health truth,
or user-selected metadata. Never construct a `VerifiedIdentityFact` directly
from an imported header field or a successful upstream tool result.

The existing provider enums are closed: `IdentityProvider` currently contains
RomM/MAME/ScummVM, and descriptive providers have their own closed enum. Calling
OpenROM data `Romm`, `Local`, `Official` or `TrustedDat` would misstate provenance.
Any future provider registration needs a separately coordinated change.

## Field-by-field mapping

“Conditional” below means suitable only after a real versioned contract,
strict validation and source binding exist. It does **not** mean currently
supported. Every retained field requires original value, producer revision,
operation/schema, import snapshot digest and record locator provenance.

| OpenROM field / source | EmuWiz equivalent | Authority level | Safe to import? | Provenance required? | Unsupported / ambiguous detail |
|---|---|---|---|---|---|
| detect `filepath`, `filename` | External source path / file locator | External provenance | Conditional, inert strings | Yes, original path and import root | Not a stable game ID; no automatic filesystem traversal |
| detect `format` | Observed representation format | External hint | Conditional | Yes, detection operation | Format does not determine platform or logical content |
| detect `platform` | Canonical platform candidate + mapping outcome | External hint | Conditional | Yes, original label and mapping basis | Composite/generic labels do not select a platform |
| detect `size_bytes` | External file-size claim | External hint | Conditional, checked `u64` | Yes, which file/representation | Container size is not decoded media/track size |
| detect `size_str` | Display-only text | Display provenance | Unnecessary; derive from validated size | Yes if retained | Never parse this back into identity/size |
| detect `paired_cue`, `paired_bin` | Related-file hints | External hint | Conditional | Yes, original references | Pair discovery is not verified CUE ownership/topology |
| detect `chd_type` | Candidate media class | External hint | Conditional | Yes, header method/version | Not verified platform, CHD logical hash, or complete topology |
| detect `needs_ecm_decode` | Representation capability hint | Operational | Not identity import | Yes if displayed | Does not authorize decompression |
| detect `valid_targets`, `badge_color` | No identity equivalent | Operational / presentation | No | Keep only with original evidence if needed | Conversion capability and UI colour are not catalogue metadata |
| scan `file` | External filename locator | External provenance | Conditional | Yes, original scan root required | CLI supplies basename, not a durable source binding |
| scan `crc32` | `ExternalHash(Crc32)` / same-scope local comparison | External hash hint | Conditional, exact 8 hex digits | Yes, whole-file algorithm/scope | CRC collision risk; not unique release proof |
| scan `matched` | Reported upstream verdict | External provenance | Retain only as a claim | Yes, exact scan operation | Never translate directly to EmuWiz “verified” |
| scan `canonical_name`, `suggested_filename` | Title / proposed name claim | External metadata hint | Conditional; no rename | Yes, originating DAT needed | Names are not stable external IDs |
| scan `error`, event `type` | Diagnostics / event discriminator | Operational provenance | Conditional, bounded | Yes, operation and record location | Progress/log/done are not game records |
| DAT summary `name`, `system` | DAT header text / source-platform hint | External provenance | Conditional | Yes, original DAT snapshot | `system` repeats header name; not a canonical platform ID |
| DAT summary `source`, `url` | Claimed ecosystem / attribution | External provenance | Conditional, inert reference | Yes, original header | URL substring recognition does not establish official authority |
| DAT summary `game_count`, `stored_path` | Reported count / source-file locator | External provenance | Conditional | Yes, original DAT/root | Count is informational; path is not permission to open another file |
| Original DAT `game.name`, `description`, ROM `name`, `size` | Existing DAT declarations | Original DAT evidence | Use existing DAT importer separately | Yes, original publisher/file digest/trust | OpenROM is an intermediary, not the DAT publisher |
| Original DAT ROM `crc`, `md5`, `sha1` | Existing algorithm-tagged DAT checksums | Original DAT hash claims | Existing DAT path, strict validation | Yes, exact ROM/hash scope | MD5/SHA-1 are read into OpenROM's index but absent from scan JSON |
| SHA-256 | `DatChecksum(Sha256)` where appropriate | No current OpenROM claim | No field to import | Required if a future contract adds it | External identity `HashAlgorithm` currently lacks SHA-256; do not relabel it |
| Stable game/platform/file external IDs | Namespaced provider IDs | Not supplied | No | Required for future IDs | Do not promote filenames, titles or array positions to upstream IDs |
| IP.BIN `hardware_id`, `maker_id` | Header provenance / structural hint | External hint | Conditional, reinspection needed | Yes, exact header bytes/source | Upstream hardware check accepts a broad `SEGA` substring |
| IP.BIN `product_number`, `product_version` | Candidate product / revision | External hint | **No current semantic mapping** | Yes, byte offsets and extraction revision | Field offsets disagree with EmuWiz's reviewed layout, below |
| IP.BIN `region_code`, derived `regions` | Candidate region metadata | External hint | **No current semantic mapping** | Yes, raw region bytes | Same offset incompatibility; no inferred release region |
| IP.BIN `release_date`, `boot_filename`, `software_type`, `peripherals` | Header observations, not catalogue authority | External hint | Defer | Yes, bytes/field semantics | Layout and interpretation require independent verification |
| IP.BIN `product_name`, `product_name_2` | Descriptive title candidates | External hint | Defer | Yes, extraction offsets/encoding | Not a verified title; differs from reviewed title-field layout |
| GDI `number`, `lba`, `type`, `sector_size` | Track topology observations | External hint | Conditional, descriptor revalidation | Yes, original descriptor | Track number is not disc ordinal; summary omits file offset |
| GDI `filename`, `filesize` | Track-component locator / size claim | External hint | Conditional | Yes, descriptor root and original path | Serialization strips directories; cannot prove ownership/coverage |
| M3U entries | Explicit playlist order / related media | External metadata hint | Through existing playlist rules | Yes, playlist and author/order basis | Order does not prove release grouping or complete disc count |
| Parent/clone relationships | Existing DAT relationships, when supplied by original DAT | Not supplied by OpenROM export | No OpenROM mapping | Required if later exported | Current renamer projection does not preserve those relationships |
| Artwork/game metadata references | Provider-owned descriptive/media references | Not supplied | No | Required if later supplied | Application icon/theme assets are not game artwork |
| Record-level publisher, timestamp, source digest, schema | Managed-source / import provenance | Not supplied | Must be acquired explicitly | Yes | Operational log timestamps do not supply this evidence |

### Hash and provenance hazards

OpenROM's renamer hashes a whole file using CRC32 with a 1 MiB read buffer.
Its DAT index is keyed only by CRC; duplicate keys overwrite earlier entries,
and scanning picks the first matching DAT. It does not compare the stored
size/MD5/SHA-1 to resolve that match. The emitted result omits its internal DAT
source and the underlying entry's stronger hashes. Therefore `matched: true`
is a tool claim with a collision risk, not independently verified identity.
[Current renamer](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/lib/src/rom_renamer.dart).

The renamer also pads DAT CRC strings to eight characters and reads the complete
DAT text before parsing, then retains its index. Do not copy those policies.
EmuWiz's `ExternalHash::parse` and `DatChecksum::parse` already require exact
algorithm-specific hexadecimal lengths; invalid input must remain a reported
invalid field, not become valid through padding. Container CRC, archive-member
hash, canonical payload hash and CHD logical hash are different scopes.
Cross-scope equality must never become a match.

### Header-field incompatibility

| Dreamcast field | OpenROM inspected offset / length | EmuWiz reviewed offset / length |
|---|---|---|
| Product number | `0x20` / 10 | `0x40` / 10 |
| Product version | `0x2A` / 6 | `0x4A` / 6 |
| Region | `0x50` / 8 | `0x30` / 8 |
| Boot filename | `0x38` / 8 | `0x60` / 16 |
| Title | `0x60` + `0x70`, 16 bytes each | `0x80` / 128 |

This comparison establishes a concrete incompatibility, not an alternative
identity authority. Reuse EmuWiz's reviewed local inspector rather than
reinterpreting OpenROM's labelled strings as verified facts.
[OpenROM field table](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/lib/src/ipbin_editor.dart),
[EmuWiz reviewed fields](../../crates/archivefs-core/src/dreamcast_boot_evidence.rs).

## Conservative platform mapping

Use `platform_by_id` first, then `platform_for_alias` against the whole label.
Preserve both the raw label and its mapping result. The registry normalizes
ASCII case/punctuation for an exact known alias; it does not fuzzy-match text.
An alias returning no match does not justify a guess.

| Observed upstream label | Proposed outcome | Existing canonical ID / reason |
|---|---|---|
| `GameCube`, `Wii`, `Saturn`, `Dreamcast`, `Sega CD`, `Neo Geo CD`, `PS2`, `PSP`, `Xbox` | EXACT | Same literal canonical IDs |
| `PS1` | KNOWN_ALIAS | `PSX`, via existing `ps1` alias |
| `PC-Engine CD` | KNOWN_ALIAS | `PC Engine CD`, via `pcenginecd` |
| `Wii U` | KNOWN_ALIAS | `WiiU`, via `wiiu` |
| `Xbox 360` | KNOWN_ALIAS | `Xbox360`, via `xbox360` |
| `GameCube / Wii`, `PSP / PS2`, `PS2 / GC`, `PS2 / Xbox` | AMBIGUOUS | Explicitly observed composite labels; preserve candidate sets, never pick the first |
| `ISO Image`, `CD Image`, `CHD Archive`, `ROM File`, an unrecognized future label | UNKNOWN | Container/generic names or no exact registry match |

Composite handling must be an explicit allowlist of studied labels, not a
generic split/fuzzy-string algorithm. Even EXACT means exact **label mapping**,
not confirmed identity: upstream also uses extension/size fallbacks and ordered
signature checks without emitting their evidence strength or conflicts.
[Current detector](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/lib/src/detector.dart).

## Media, multidisc and conflict projection

GDI output describes tracks within one medium, not multiple discs. Its summary
omits the descriptor's per-file offset and collapses track type to Data/Audio.
It is insufficient to create a verified optical layout. M3U ordering is
explicit caller ordering; it supplies neither a stable release identity nor
proof of expected disc count. No current OpenROM export supplies revision
families, alternate-release relationships, explicit multidisc identities or
parent/clone topology. Do not infer ordinals or grouping from filenames.

For a future supported contract, project relationships as `MediaEvidence` with
`EvidenceKind::Metadata` and `Equivalence::None` until stronger existing
evidence proves a relationship. Preserve missing ordinals/counts as unknown.
Run existing media conflict rules; do not create automatic launch/swap plans.

Conflicts need a typed, deterministic read-only projection containing field,
external value, verified/local value, both provenances and comparison basis.
Examples below are synthetic requirements, not current exported records:

| Conflict | Required explanation / existing model boundary |
|---|---|
| Title | “OpenROM says Synthetic A; verified DAT says Synthetic B”; retain descriptive candidates in metadata conflicts |
| Region | Retain both region claims and their native/DAT/provider origins; metadata/media variant conflict |
| Revision | Retain differing revision values as a variant conflict; never assume version strings are interchangeable |
| Platform | Retain external label, canonical mapping and verified platform; existing identity platform-conflict concept |
| Hash | Include algorithm and byte scope alongside both values; no conflict resolution by source popularity |
| Size / file state | Preserve stale/mismatched file evidence; never reuse a stale successful observation |

`IdentityConflict::ConflictField` currently covers Platform/Hash/FileSize/
Signature/FileState, not Title/Region/Revision. Metadata aggregation already
retains descriptive conflicts, and media sets retain variant conflicts; use
those existing responsibilities rather than pretending the identity enum
already expresses every case. Current `match_record` is RomM-oriented and
short-circuits hash agreement; any future all-field conflict projection needs
its own explicit comparison contract without changing RomM's active lane here.

## Licensing and redistribution

OpenROM identifies its own implementation as **GNU GPL v3**; the pinned
repository includes the GPL version 3 text and GPL v3 source notices.
This research does not settle a project-specific “or later” grant from the
standard licence's illustrative appendix. Preserve the upstream stated
licence and notices if implementation is ever reused.
[Pinned LICENSE](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/LICENSE).

The README lists separately licensed bundled tools, including GPL, MIT, ISC and
Apache-licensed components. Those claims are not a completed dependency or
binary-distribution licence audit. This task copies no implementation, bundled
binary, theme, artwork or game data into EmuWiz. An independently written
interchange parser would still need the actual data contract and attribution;
there is no reason to port OpenROM's conversion/patch implementation.
[Tool attribution table](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/README.md#-bundled-tools).

An imported No-Intro/Redump DAT remains third-party data. The toolkit's GPL does
not establish permission to redistribute those DATs or future provider artwork.
No separate OpenROM game-dataset licence or redistribution grant was found,
because no such dataset was present in the inspected tree. Verify each actual
data source's terms before any future redistribution; local interpretation and
bundling are separate decisions. No upstream communication was sent.

## Future implementation gate and bounded-input design

Reopen implementation only when an explicit local export contract supplies:

1. A producer/format discriminator and independently versioned schema, with
   normative field types and backward-compatibility rules.
2. Operation, producer revision and originating DAT/header/file provenance;
   stable IDs where claimed, and defined hash algorithms and byte scopes.
3. Independently verified header/media semantics and a lossless representation
   of any relationships offered for import.
4. Public synthetic fixtures and licence/attribution terms for distributed data.

Prefer an upstream documented record stream over accumulating a whole
catalogue. Do not create a private wrapper and advertise it as an OpenROM
standard. Existing DATs, GDI descriptors and playlists can continue through
their existing EmuWiz paths; that does not need an OpenROM importer.

The following are **proposed future parser ceilings, not implemented support**:

| Resource | Proposed bound / behaviour |
|---|---|
| Local export file | 64 MiB; enforce during reads, including growth after open |
| Records | 100,000; stop with an explicit limit error |
| I/O buffer | 64 KiB |
| Encoded record | 256 KiB, enforced before unbounded allocation |
| String | 4 KiB UTF-8 bytes; IDs 256 bytes; reject invalid types/encodings |
| Container nesting | 16 levels, checked during tokenization |
| Per-field list | 256 elements; object keys 128; reject duplicate keys |
| Numbers | Checked nonnegative `u64`; checked narrower media ordinals/sector fields; reject floats/overflow where integers are required |
| Processing work | Linear in bounded input and emitted fields; no recursive file discovery, network, executable invocation or implicit payload hashing |
| Retained state | One bounded typed record plus bounded comparison results; stream to caller, do not collect every record into a `Vec` |

Bounds must apply before constructing arbitrary `serde_json::Value` trees.
Use a bounded reader and typed streaming/token visitor, not unbounded
`read_to_end` or a whole-catalogue deserialize. A supported parser should have
no database, activation, fetch or mutation capability. Imported paths/URLs stay
inert provenance. Compare only caller-supplied current verified evidence of the
same scope; retain invalid field diagnostics and fail closed on malformed
identity-bearing records. Preserve original bytes/record locations within the
bounded import snapshot, rather than silently normalizing away malformed data.

### Required synthetic validation for a future parser

No runtime fixtures or parser tests were added, because there is no supported
upstream schema to make a “valid OpenROM record” fixture honest. The future
implementation must test these cases against the contract that actually lands:

| Test group | Required cases |
|---|---|
| Format/version | Supported studied schema; unsupported newer version; missing schema; another operation's JSON refused |
| Malformed records | Wrong types, missing required identity fields, duplicate keys, truncated input; missing optional fields remains absent |
| Platform | Exact `Wii`; existing alias `PS1` -> `PSX`; studied composite ambiguous; unfamiliar/generic label unknown |
| Hashes | Valid CRC/MD5/SHA-1/SHA-256 only where supplied/supported; wrong length, non-hex and padded invalid values refused; scope mismatch refused |
| Identity | Same-scope match strengthens an existing verified match; conflicting hash/platform/title/region/revision retains both claims; stale local evidence refused |
| Media | Explicit disc order/count where supported; missing ordinal remains unknown; track number cannot become disc number; conflicting relationships visible |
| Resource limits | File at/beyond bound, record/list/string/count/nesting bounds, numeric overflow, growing file; no oversized allocation before refusal |
| Projection | Deterministic output/conflict ordering, original values retained, producer/schema/source/digest/record provenance preserved |
| Side effects | Read-only handles; unchanged source/catalogue/provider files; no database writes, network, subprocess or automatic metadata selection |
| Memory | Synthetic 1,000 versus 100,000 records within file limits; peak RSS does not grow with retained catalogue size; document measured fixed overhead |

Likely future code belongs in a focused `identity_source/openrom.rs` plus tests,
with a minimal module registration only after the export gate and collision
checks pass. Reuse the canonical platform registry and checksum validators.
Provider enum/conflict/SHA-256 additions, if truly required, need coordinated
small changes to their owning modules; do not alter DAT authority or add a new
journal, migration, download manager or GUI path for parser development.

## Collision audit and validation

All 432 existing worktrees were inspected at start: 53 had tracked changes.
The requested document had no tracked **or untracked** collision in any of
them. Prospective shared provider registration/model files were already dirty:

| File | Dirty worktrees observed |
|---|---|
| `crates/archivefs-core/src/identity_source/mod.rs` | `/home/davedap/archivefs`; `/home/davedap/emuwiz-082-batch2-media-dryrun`; `/home/davedap/emuwiz-082-batch2b-media-gui`; `/home/davedap/emuwiz-identity-providers-poc` |
| `crates/archivefs-core/src/identity_source/model.rs` | Same four worktrees |
| `crates/archivefs-core/src/platform/mod.rs` | `/home/davedap/archivefs`; `/home/davedap/emuwiz-082-9f-tape-refactor`; `/home/davedap/emuwiz-082-batch1-dryrun`; `/home/davedap/emuwiz-082-batch1a-dryrun`; `/home/davedap/emuwiz-082-batch1b-dryrun`; `/home/davedap/emuwiz-082-batch3-launch`; `/home/davedap/emuwiz-082-batch4-dat-dryrun`; `/home/davedap/emuwiz-082-batch4-dat-v2`; `/home/davedap/emuwiz-082-first-wave-reapply-check`; `/home/davedap/emuwiz-082-integration` |

These files were read only. The research-only lane does not require editing
them, so no concurrent work was overwritten. `patch_manager/mod.rs`, GUI,
conversion/Wii U, standalone patching, DAT/No-Intro, RomM, catalogue-health,
MAME reconstruction and migrations have **ZERO changes** in this candidate.

Validation for this candidate: **69 research evidence/document checks passed**,
including all 35 downloaded source-file digests and **14 local document links**.
The task preflight/postcheck, `cargo fmt --all -- --check` and staged/unstaged
`git diff --check` passed. The formatting check used an isolated
`CARGO_TARGET_DIR` at `/tmp/emuwiz-openrom-interoperability-N8sK58/target`.
Production Rust
tests: **0 added, 0 run**. OpenROM focused tests, core suite and offline workspace
compilation are not applicable to a document-only change; no previous task's
test results are claimed as this task's validation. Any future Rust validation
must use an isolated `CARGO_TARGET_DIR`.

Public research evidence is retained outside the worktree at
`/tmp/emuwiz-openrom-interoperability-N8sK58/`: API responses, complete tree,
release comparison, bounded source files and SHA-256 manifest, starting main
status, full dirty-worktree audit and document-collision checks. Only this
document is intended for the focused commit.

## Update/watch recommendation and future presentation

Keep OpenROM on the interoperability watch list. Review its next release or
an explicit catalogue/export-schema announcement; a scheduled watcher or
network acquisition feature was not created. Watch the CLI serialization,
renamer provenance, header-field semantics and schema documentation, not just
the version tag. Pin the next studied commit and rerun the gate before coding.

A future GUI could show the original OpenROM claim beside verified DAT/native
evidence, label exact/alias/ambiguous/unknown mapping, distinguish whole-file
versus logical-media hashes, and list unresolved conflicts and provenance.
That is a presentation possibility, not GUI work or automatic catalogue
mutation authorized by this candidate.

**Result:** current OpenROM observations have potential value as external
hints, but no safe versioned import contract is established. Parser implemented:
**NO**. Schema support: **NONE**. Production/catalogue/provider writes: **ZERO**.
Promotion is safe for this research document only; runtime import remains
deferred.
