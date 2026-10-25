# PS3 / Xbox 360 Mod Identity Evidence

**Status:** research only; no production code changed.  This document maps
read-only identity evidence to the existing EmuWiz mod and selected-game
contracts. It does not define an installer, title-update downloader, package
extractor, or GUI feature.

## 1. Executive Summary

The strongest first-party identity boundaries are already available locally:

| Platform | Strong local source | Facts it can prove | Important limit |
|---|---|---|---|
| PS3 disc/folder | `PS3_GAME/PARAM.SFO` | `TITLE_ID`, `TITLE`, `CATEGORY`, `APP_VER`, and other bounded SFO values | SFO identity does not prove a complete install, license, or exact executable revision |
| PS3 disc | `PS3_DISC.SFB` | Disc-root descriptor, `TITLE_ID`, disc `VERSION`, hybrid flags when its table is parsed | Current EmuWiz code checks only `.SFB` magic; field extraction needs a separately bounded implementation |
| PS3 package | fixed `.pkg` header and Content ID | package revision/type, Content ID, derived Title ID shape | fixed header does not prove payload, RAP/license, or installability |
| Xbox 360 executable | XEX2 execution-info optional header in `default.xex` or another reviewed XEX | Title ID and Media ID; execution-info version fields are available in the format | header identity is executable identity, not proof of a complete disc/package or patch compatibility |
| Xbox 360 digital/content package | STFS `CON ` / `LIVE` / `PIRS` fixed metadata | Title ID, Media ID, version, base version, content type, disc facts | STFS is an envelope for games, DLC, saves, updates, and other content; signatures/licenses are not currently verified |

Recommended precedence for a mod target is:

1. exact hash of the target file, if the mod declares it and EmuWiz can hash
   the bounded target;
2. platform-native executable identity, plus revision/version facts (`XEX`
   Title ID + Media ID/version; PS3 `PARAM.SFO` Title ID + `APP_VER` and,
   where available, a target-file hash);
3. package/container identity corroborated with the selected game
   (`CONTENT_ID`/Title ID, or STFS Title ID/Media ID/version);
4. exact verified game Title ID alone for family-level matching only;
5. catalogue declaration;
6. filename or directory-name hints, never as verified identity.

If a required identity component is absent or conflicts, the result must be
`Unknown`, `Ambiguous`, or an explicit mismatch. It must not silently fall
back to a weaker claim. A Title ID identifies a title family; it does not by
itself prove that a binary patch applies to a particular update or region.

Authoritative format references used here include the [PS3 PARAM.SFO
reference](https://www.psdevwiki.com/ps3/PARAM.SFO), [PS3_DISC.SFB
reference](https://www.psdevwiki.com/ps3/PS3_DISC.SFB), [PS3 PKG
reference](https://www.psdevwiki.com/ps3/index.php?section=55&title=PKG_files),
[Xenia's XEX handling](https://github.com/xenia-project/xenia/blob/master/src/xenia/emulator.cc),
and the [Free60 STFS format
reference](https://github.com/Free60Project/wiki/blob/master/docs/System-Software/Formats/STFS.md).

## 2. PS3 Identity Sources

### `PS3_GAME/PARAM.SFO`

`PARAM.SFO` is a little-endian System File Object with a bounded header,
index table, key table, and typed values. It is a general Sony format, so the
file and the key must be interpreted in layout context. The current local
`param_sfo` parser correctly preserves unsupported value types and fails
closed on malformed offsets, excessive entries, excessive values, and files
outside its size bound.

| Source / field | Type | Trust level | Present means | Absent means | Conflict behaviour |
|---|---|---|---|---|---|
| `PS3_GAME/PARAM.SFO` structure | bounded structured file | Strong structure / corroborated platform context | A conventional PS3 content layout can be inspected | No claim about PS3 content | Do not infer from a directory name alone |
| `TITLE_ID` | UTF-8 text, conventionally nine characters such as `BLUS30000` | Strong structured identity when valid in PS3 layout | Candidate PS3 product/title ID from the content's own metadata | Title identity is unresolved | Conflict with another internal Title ID is `Ambiguous`/mismatch |
| `TITLE` | UTF-8 text | Display metadata | Human-readable title supplied by content | No display title from SFO | Never overrides an ID conflict |
| `CATEGORY` | UTF-8 text, commonly `DG` for a disc game | Structured content-category evidence | Content declares a category | Category unknown | Preserve raw value; do not invent “game” from it |
| `APP_VER` | UTF-8 version string, commonly `01.00` | Structured application/update version | Application revision/version is declared | No application version claim | Do not compare as a release hash; malformed values remain unknown |
| `VERSION` | UTF-8 disc/package revision, commonly `01.00` | Structured package/disc metadata | Disc/package revision is declared | Revision unknown | Keep distinct from `APP_VER` |
| `PS3_SYSTEM_VER` | UTF-8 firmware requirement | Structured requirement | Content declares a minimum/system version field | Firmware requirement unknown | Does not prove the local emulator or console satisfies it |
| `CONTENT_ID` | UTF-8 Content ID when present | Corroborating package/content identity | Content associates itself with a Content ID | No Content ID claim | Compare with a package/header Content ID; disagreement is ambiguity |
| `ATTRIBUTE`, `BOOTABLE` | integer flags | Behaviour/category evidence | Flags describe content behaviour | No bootability conclusion | Never turn a missing flag into “not bootable” without the relevant category rules |

The PS3 Developer wiki documents `TITLE_ID`, `CATEGORY`, `APP_VER`,
`CONTENT_ID`, `VERSION`, and the distinction between `APP_VER` and
`VERSION`; it also notes that update packages normally retain the same Title
ID and increase `APP_VER`. These are useful matching facts, not a substitute
for hashing the modified target.

### Disc structure and `PS3_DISC.SFB`

A conventional PS3 disc has `PS3_DISC.SFB` at the disc root beside
`PS3_GAME/`, with `PS3_GAME/PARAM.SFO`, `PS3_GAME/USRDIR/EBOOT.BIN`, and
other content below it. `EBOOT.BIN` has a SELF container signature; the
signature proves a bounded executable/container format, not decrypted code or
completeness.

The PS3 Developer wiki describes the `.SFB` table as big-endian and documents
the following useful fields:

| Offset | Field | Type / meaning | Research status |
|---:|---|---|---|
| `0x00` | magic | `.SFB` | Current code checks this only |
| `0x04` | file version | 32-bit value | Candidate disc metadata |
| `0x20` | `HYBRID_FLAG` | descriptor key | Disc feature flags |
| `0x40` | `TITLE_ID` | descriptor key | Disc title-record key |
| `0x50` | title data offset/length | bounded table locator | Must validate before reading |
| `0x60` | `VERSION` | descriptor key | Disc revision key |
| `0x70` | version data offset/length | bounded table locator | Must validate before reading |
| `0x220` | title data | typically nine-character ID | Candidate only until cross-checked |
| `0x230` | version data | typically `01.00` | Candidate disc revision |

The existing `ps3_disc_evidence` module deliberately stops at `.SFB` magic
because earlier field-offset evidence was not sufficiently corroborated in
the implementation review. Future extraction should read only the fixed
header and declared small records, validate all offsets/lengths against a
small cap, and compare its Title ID with `PARAM.SFO`; it must not make the
`.SFB` value authoritative when the SFO disagrees.

### Installed `dev_hdd0/game/<TITLE_ID>` layouts

An installed PSN/game-data layout conventionally uses:

```text
dev_hdd0/game/<TITLE_ID>/PARAM.SFO
dev_hdd0/game/<TITLE_ID>/USRDIR/...
```

The directory name is an index/navigation convention, not proof. The internal
`PARAM.SFO/TITLE_ID` is the identity authority, corroborated by the expected
layout and, where relevant, `CONTENT_ID`, `CATEGORY`, and `USRDIR` contents.
The same rule applies to copied or renamed extracted directories: renamed
folders do not change the internal identity, and a folder named like a Title
ID without matching SFO evidence is unresolved.

### PS3 PKG metadata

The current local observer reads a fixed `0x80`-byte `.pkg` header, validates
the declared package ranges against the actual file length, and does not read
the metadata table or payload. It obtains:

| Field | Type | Trust level | Use |
|---|---|---|---|
| magic | 4 bytes `\x7fPKG` | Strong container signature | PS3/PSN package candidate |
| revision | big-endian `u16` | Structured package fact | Reject unsupported revisions |
| package type | big-endian `u16` | Structured package fact | Reject non-PS3 package types in the PS3 observer |
| metadata/data ranges | big-endian offsets/counts/sizes | Structural validity | Bounds checking only |
| Content ID | 48-byte text field | Corroborated package identity | Match package lineage and derive a candidate Title ID |
| Title ID from Content ID | exact grammar-shaped substring | Corroborated derived fact | Cross-check with SFO/selected game, never a release database lookup |

The fixed header does not prove the package's payload is complete, that an
RAP/RIF/license is available, that firmware is sufficient, or that installation
succeeded. PKG filenames are hints only; a renamed package remains identified
by its header or remains unknown if that header is unavailable.

### Region, edition, and firmware

The first letters of PS3 IDs have conventional distribution/region meaning,
but the ID grammar is not a complete region authority for every product. A
region label should therefore be a derived catalogue/display interpretation
of a verified ID, not a parser assertion. `TITLE_ID` plus `CONTENT_ID`, SFO
category, disc/package context, and a reviewed catalogue can corroborate a
region or edition. A filename suffix such as `(USA)` cannot do so.

`APP_VER` is the application/update version; `VERSION` describes disc/package
revision in the documented SFO usage. Firmware fields are requirements, not
proof of actual firmware or emulator readiness.

## 3. PS3 Evidence Precedence

For matching a PS3 mod to a selected game, use this order:

| Rank | Evidence | Result |
|---:|---|---|
| 1 | Exact cryptographic hash of the target file, with a declared expected hash | Exact file match; safest binary patch gate |
| 2 | `PARAM.SFO` Title ID plus target-file hash or independently verified `APP_VER`/revision | Strong title-and-revision match |
| 3 | `PARAM.SFO` Title ID corroborated by `PS3_DISC.SFB` or PKG Content ID | Strong title-family match; revision may remain unknown |
| 4 | Valid PKG Content ID and derived Title ID | Package identity only; do not claim installed-game identity |
| 5 | Title ID alone from internal metadata | Exact game-family match, not binary compatibility |
| 6 | Catalogue declaration | Compatibility claim only to the extent the catalogue is trusted and the local identity agrees |
| 7 | Folder/package/file name | Candidate hint only |

A mod that declares only a Title ID can be offered as “intended for this game
family” but must remain `PartiallyVerified` for an update-sensitive target. A
mod that declares Title ID plus `APP_VER`/target version can be matched only
when the selected content exposes the same field in a compatible context. A
mod that declares a target-file hash can be exact even when regional labels
are absent, provided the source file was hashed safely.

A conflicting SFO and SFB/PKG identity is not resolved by precedence into a
winner. It is an identity conflict requiring review. Missing `PS3_DISC.SFB`
on an installed/PSN layout is not itself a failure; missing `PARAM.SFO` in a
layout that requires it prevents verified identity.

## 4. PS3 Ambiguity Cases

- **Disc versus PSN:** both can expose a related Title ID, but they are
  different content contexts. Do not treat a PKG header as proof that the
  `PS3_GAME` disc payload is installed or interchangeable.
- **Regional releases:** one game may have `BLUS`, `BLES`, or other distinct
  IDs. A title-name match is insufficient; require the exact internal ID or a
  catalogue relationship explicitly declaring equivalence.
- **Updates:** a normal update retains the base Title ID and changes
  `APP_VER`; `VERSION` may reset or describe package/re-release revision.
  Matching only a larger numeric version is unsafe without the target/update
  relationship.
- **DLC and game data:** `CATEGORY`, Content ID type/label, and install path
  distinguish related content. Shared names do not make DLC a base game.
- **Multiple installed versions:** each observed path is a separate evidence
  record. Do not choose by directory mtime or folder spelling. If IDs agree
  but versions differ, expose both and keep version matching unresolved.
- **Extracted/decrypted layouts:** `PS3_GAME/PARAM.SFO` remains useful, but a
  missing SELF, missing expected files, or an incomplete tree is a readiness
  issue rather than permission to infer a version.
- **Renamed directories:** ignore the directory name as identity when it
  conflicts with internal metadata.
- **Incomplete dumps:** preserve the verified facts that were read, but keep
  completeness and exact mod applicability unknown.

## 5. Xbox 360 Identity Sources

### XEX2 / `default.xex`

Xbox 360 disc and extracted-game identity is anchored by an XEX2 executable,
normally `default.xex` in an XDVDFS disc layout. XEX2 has a bounded optional
header directory. Execution information is optional-header ID `0x40006`; the
execution-info record carries Media ID, version-related fields, and Title ID.
Xenia reads the execution info and uses its Title ID and version during game
setup; the format reference is also represented by Xenia's `xex2_info`
implementation.

The current local `game_identity` reader reads only the XEX2 header, optional
header table, and the fixed execution-info record fields needed for Title ID
and Media ID. The 24-byte execution-info layout also exposes version,
base-version, platform, executable-type, disc, and save-game-ID positions for
future narrowly scoped facts; those fields are not currently promoted into
EmuWiz identity kinds. The reader never executes the file and never
decompresses/decrypts the module body.

| Source / field | Type | Trust level | When present / absent |
|---|---|---|---|
| XEX2 magic | 4 bytes `XEX2` | Strong executable-container signature | Present identifies XEX format; absence is not an Xbox 360 identity |
| optional-header table | bounded big-endian entries | Strong structure if bounds validate | Missing/truncated table makes execution facts unavailable |
| execution-info Media ID (`+0x00`) | big-endian `u32`, conventionally eight uppercase hex digits | Strong native executable fact | Present binds the executable to a media lineage; absent means Media ID unknown |
| execution-info version (`+0x04`) | big-endian `u32` | Strong executable metadata when parsed | Present can distinguish executable revisions; absent means revision unknown |
| execution-info base version (`+0x08`) | big-endian `u32` | Structured executable metadata | Useful for update lineage when corroborated; absent means base revision unknown |
| execution-info Title ID (`+0x0c`) | big-endian `u32`, conventionally eight uppercase hex digits | Strong native executable fact | Present identifies the Xbox 360 title family; absent means Title ID unknown |
| execution-info platform (`+0x10`) | byte | Structured context | Context only; do not substitute for Title ID/Media ID |
| execution-info executable type (`+0x11`) | byte | Structured context | Distinguishes declared executable class only when value semantics are reviewed |
| execution-info disc number/total (`+0x12`/`+0x13`) | bytes | Multi-disc metadata | Useful grouping facts; absent or malformed means disc grouping unknown |
| execution-info execution/save-game ID (`+0x14`) | big-endian `u32` | Structured metadata | Useful only for the specific relationship it declares; not a patch hash |
| `default.xex` filename | filesystem name | Filename hint only | Renaming must not change identity |

Title ID alone is sufficient for a title-family gate such as a provider lookup.
Media ID is required for update-sensitive and binary-patch matching unless the
mod declares an exact target hash or another reviewed equivalent. A valid XEX
header does not prove that the surrounding XDVDFS tree is complete.

Original Xbox `default.xbe`/XBE certificate identity is a separate platform
boundary. An XBE Title ID must never be accepted as an Xbox 360 XEX Title ID,
and the presence of `default.xbe` is not evidence for a 360 mod target. The
existing EmuWiz `XbeTitleId` and `XexTitleId` kinds intentionally keep these
families distinct.

### XDVDFS / disc and extracted layouts

For a disc or extracted game, the useful boundary is:

```text
XDVDFS root/
└── default.xex
```

The directory name, ISO filename, and `default.xex` basename are not identity
authority. A future bounded directory observer may corroborate the root
structure and locate exactly one intended executable, but it should hand the
executable bytes to the existing XEX observer rather than create a second
XEX parser.

### STFS: `CON `, `LIVE`, and `PIRS`

STFS is a package envelope used for games, downloadable content, saved games,
profiles, title updates, marketplace items, and other content. Its magic
identifies signing/distribution form, not content class:

| Magic | Meaning | Identity limitation |
|---|---|---|
| `CON ` | console-signed package | Common for saves/profiles and other writable content; not automatically a game |
| `LIVE` | Microsoft/Xbox Live signed package | Not proof of a specific content class |
| `PIRS` | Microsoft-signed non-LIVE package | Often used for system/disc-delivered content; still not a game claim by itself |

The fixed metadata fields documented by Free60 and already parsed by
`xbox360_stfs_evidence` are:

| Offset | Field | Type | Trust/use |
|---:|---|---|---|
| `0x340` | Header Size | big-endian `u32` | Structural metadata |
| `0x344` | Content Type | big-endian `u32` | Raw content-class discriminator; do not over-interpret |
| `0x348` | Metadata Version | big-endian `u32` | Selects metadata layout |
| `0x34c` | Content Size | big-endian `u64` | Container metadata |
| `0x354` | Media ID | big-endian `u32` | Strong package metadata for update/media matching |
| `0x358` | Version | big-endian `u32` | Package/title-update version fact |
| `0x35c` | Base Version | big-endian `u32` | Base version relationship fact |
| `0x360` | Title ID | big-endian `u32` | Strong package title-family fact |
| `0x364` | Platform | byte | Context fact; `2` denotes Xbox 360 in the reference |
| `0x365` | Executable Type | byte | Raw metadata |
| `0x366`/`0x367` | Disc Number / Disc in Set | bytes | Multi-disc grouping facts |
| `0x368` | Save Game ID | big-endian `u32` | Raw package metadata |
| `0x411`, `0x1691` | Display/Title Name | fixed locale text blocks | Display only |

Current EmuWiz intentionally does not verify STFS signatures/licenses, walk
the file table, extract files, or classify every numeric Content Type as a
game. A parsed STFS header is therefore identity/readiness evidence, not proof
of playable content or patchable executable bytes.

### DLC, GOD, CON/LIVE/PIRS, and extracted content

Games-on-Demand installations and STFS packages can share the same Title ID
with different Media IDs, versions, content types, and package layouts. DLC
and saved content can also share a title family. Therefore:

- use Title ID to group related content;
- use Content Type and path role to distinguish base game, update, DLC, save,
  or other envelope content;
- use Media ID/version/base version for update-sensitive compatibility;
- never treat a `CON ` package or a folder named with a Title ID as a game
  executable without corroborating XEX/XDVDFS evidence.

## 6. Xbox Evidence Precedence

| Rank | Evidence | Safe conclusion |
|---:|---|---|
| 1 | Exact target-file hash | Exact binary target match |
| 2 | XEX Title ID + Media ID + executable version, optionally target hash | Strong executable/revision match |
| 3 | XEX Title ID + Media ID | Strong media lineage; version may remain unresolved |
| 4 | STFS Title ID + Media ID + version/base version/content role | Strong package/update relationship evidence |
| 5 | XEX or STFS Title ID alone | Title-family match only |
| 6 | Catalogue declaration | Supporting compatibility claim, not local proof |
| 7 | `TitleID`, `MediaID`, or game name in a filename/path | Candidate hint only |

If a mod or patch requires a module hash, current EmuWiz must refuse exact
verification unless it can safely compute that hash. The existing Xenia patch
provider deliberately does not decompress/decrypt the module body, so a hash-
constrained patch remains not independently verifiable by that path.

## 7. Title Update Matching

An Xbox 360 title update is itself content/package data and should be
identified from its native metadata, not from a `TU` filename. Locally useful
facts are:

- Title ID: target title family;
- Media ID: media/distribution lineage the update expects;
- version: update/package version;
- base version: relationship to the base/system title version;
- STFS content type and package role: distinguishes update-like content from
  saves/DLC/other packages;
- XEX execution info: the installed/base executable's Title ID, Media ID, and
  version facts when available.

The practical matching rule is Title ID **and** Media ID first, then version
and base-version relationship. Title ID alone is insufficient for a binary or
title-update compatibility claim. Community installation guidance also tells
users to select a title update by matching both Title ID and Media ID; this is
consistent with the native metadata model, though EmuWiz should not depend on
that website or a network lookup.

Update sequencing should be represented as observed numeric metadata and
explicit compatibility relations. EmuWiz can compare locally available
version/base-version fields, but cannot safely prove the complete cumulative
sequence, supersedence, or emulator behaviour without additional native
metadata and policy. It must not install, enable, download, or reorder title
updates in this research slice.

## 8. File-Level Mod Target Identity

For a mod targeting an executable or resource, the target contract should be
explicit:

1. target path is confined to the selected game root and is not trusted from
   a package filename;
2. target regular file is read-only opened and bounded before hashing or
   inspection;
3. exact expected SHA-256 (or a format-specific reviewed hash) is checked
   first when declared;
4. native identity is checked next: PS3 Title ID/APP_VER or Xbox XEX Title
   ID/Media ID/version;
5. a catalogue declaration is accepted only as an explanation of intent, not
   as proof against a contradictory local file;
6. filename hints never elevate the result.

Safe outcomes should be:

- **Exact compatible:** all required native facts and target hash match;
- **Compatible with warnings:** title/media identity matches, but the mod's
  revision/hash constraint is absent and the operation is explicitly
  non-exact;
- **Unknown / not proven:** required local fact is unavailable or observation
  is incomplete;
- **Incompatible:** a verified fact or exact hash contradicts the declaration;
- **Ambiguous:** two verified internal sources disagree.

No patch should become eligible merely because the game title or folder name
looks right. A conflict must block the apply stage before any write.

## 9. Existing EmuWiz Type Mapping

The current local architecture is already close to the required boundary:

| Research fact | Existing type/location | Assessment |
|---|---|---|
| PS3 Title ID | `IdentityKind::Ps3TitleId`, `GameIdentityReport::verified_ps3_title_id`, `VerifiedIdentityFact::Ps3TitleId` | Sufficient for title-family identity |
| PS3 SFO title/category/app version | `Ps3LayoutObservation` and shared `SfoObservation` | Present for bounded observation; app/version are not yet promoted as dedicated identity facts |
| PS3 Content ID / derived Title ID | `PkgHeaderFact`, `pkg_header_evidence`, Content ID grammar helper | Sufficient as package evidence; retain package-vs-installed distinction |
| PS3 SFB | `Ps3DirectoryObservation::disc_sfb_present` | Magic-only today; field extraction remains future research/implementation |
| Xbox 360 Title ID | `IdentityKind::XexTitleId`, `GameIdentityReport::verified_xex_title_id`, `VerifiedIdentityFact` via Xenia-specific gates | Sufficient |
| Xbox 360 Media ID | `IdentityKind::XexMediaId`, `GameIdentityReport::verified_xex_media_id` | Sufficient for XEX/Xenia patch matching |
| Xbox STFS Title/Media/version | `StfsHeaderFact` and `xbox360_stfs_evidence` | Sufficient as raw package metadata; not yet a general selected-game identity fact |
| Selected game | `SelectedGameForMod` with `GameIdentityReport` | Sufficient for identity-gated mod planning; future work can attach additional facts without changing path safety |
| Mod identity declaration | `ModIdentityKind`, `LocalModPackage`/patch-manager documents | Extend only with evidence-backed fields and preserve fail-closed matching |
| Standalone patch | existing `standalone_patch` and patch-manager flows | Keep native PS3/Xbox facts separate from generic file/path operations |

Recommended future typed additions, only if a concrete mod declaration and
local source require them:

- `Ps3AppVersion` and `Ps3DiscOrPackageVersion`, kept distinct;
- `Ps3ContentId`, kept distinct from `Ps3TitleId`;
- `XexVersion`/execution revision, kept distinct from Title ID and Media ID;
- `StfsContentType`, `StfsVersion`, and `StfsBaseVersion` as raw package facts;
- a typed `TargetFileSha256` bound to the selected game root and target path.

Do not add a generic “region” fact that guesses from filenames. A region fact
should be a typed interpretation with source/provenance, or remain absent.

## 10. Local Sample Findings

The bounded filesystem check was read-only and limited to shallow known paths;
it did not recursively hash or scan large trees.

- `/home/davedap/PlayStation3/PKGs` contains many `.pkg` files and `.rap`
  companions. A representative `Metal Gear Solid 4 Guns of the Patriots.pkg`
  is approximately 495 MB and begins with a valid PS3 PKG magic/header shape;
  its fixed header includes a Content ID. It was not extracted or migrated.
- The same PS3 area contains a large `pspemu/ISO` collection whose bracketed
  IDs are PSP/PSN-style names. Those names are useful navigation hints only
  and must not be misclassified as PS3 game identity.
- `/mnt/usbdrive/games` contains original-Xbox ISO material and extensive MAME
  content. No nearby `default.xex`, `.xex`, or STFS package was found within
  the bounded sample query, so no Xbox 360 real specimen was opened.
- No production catalogue, emulator install, or source tree was modified.

The local implementation and synthetic tests therefore provide the reliable
XEX/STFS/PS3 parser evidence for this document; real Xbox 360 field validation
remains a future fixture task.

## 11. Parser/Dependency Options

Prefer the existing local parsers:

- `param_sfo` for PS3 SFO values;
- `ps3_disc_evidence` for PS3 directory and bounded PKG header evidence;
- `game_identity`/`executable_signatures` for XEX2 execution-info identity;
- `xbox360_stfs_evidence` for fixed STFS metadata.

No new dependency is justified for the first implementation slice. External
references are useful for cross-checking only:

- PS3 Developer wiki and `Jasily/py.dataformat.sfo` for SFO structure;
- PS3 Developer wiki for SFB and PKG field descriptions;
- Xenia's `xex2_info` implementation for XEX execution-info semantics;
- Free60's STFS documentation and independent STFS readers for fixed offsets.

Do not add a general-purpose package installer, XEX loader, STFS filesystem
extractor, PS3 decrypter, RAP/RIF verifier, or network metadata client for
identity evidence.

## 12. Safety Requirements

Any future implementation must:

- open sources read-only and never require a mount or emulator process;
- use bounded prefix reads and fixed caps for SFO, SFB, PKG, XEX, and STFS
  metadata;
- validate every declared offset, count, length, integer conversion, and
  multiplication before allocation or access;
- reject malformed fields rather than returning a plausible partial identity;
- preserve raw bytes/strings when decoding is lossy or the field is unknown;
- never load, execute, decrypt, decompress, or interpret executable bodies;
- distinguish absent evidence from proven absence;
- confine path-based reads to caller-approved roots and reject symlink escapes
  or replaced roots according to existing safe-read policy;
- avoid recursive traversal of arbitrary game trees unless a separate bounded
  layout contract explicitly permits it;
- retain source path, source format, field, and evidence provenance;
- report conflicts as `Ambiguous` or mismatch, never resolve by filename;
- perform all compatibility checks before any future mod write;
- leave source content, package metadata, and user configuration unchanged.

## 13. Recommended Implementation Slice

**Next slice: read-only PS3 `PARAM.SFO` + Xbox 360 XEX identity extraction
projection for selected-game mod evidence.**

This is the smallest useful step because both bounded readers already exist,
both Title ID facts already have identity plumbing, and Xbox 360 Media ID is
already part of the XEX path. The slice should:

1. expose a small read-only evidence result containing source path/format,
   verified Title ID, optional PS3 `APP_VER`/category or Xbox XEX version, and
   optional Xbox Media ID;
2. connect that result to the existing selected-game/mod compatibility
   projection without changing GUI or apply code;
3. add synthetic malformed/truncated/conflict tests and one small fixture per
   format;
4. retain `Unknown` when a required field is absent;
5. defer PS3 SFB field parsing, STFS-to-selected-game joins, title-update
   installation, package extraction, hashes of executable bodies, and all
   network work.

The implementation must remain local, independently testable, bounded, and
read-only.

## 14. Deferred / Unknown Areas

- PS3 `PS3_DISC.SFB` field extraction is documented by one accessible format
  reference but is not yet independently implemented in EmuWiz; keep the
  current magic-only boundary until the field reader receives focused review.
- PS3 encrypted SELF/NPDRM, RAP/RIF/license state, firmware compatibility,
  and complete installed-content verification are outside identity evidence.
- PS3 edition and region equivalence across Title IDs requires a reviewed
  catalogue relationship; parser heuristics are insufficient.
- Xbox 360 XEX execution-info revision/execution-ID fields beyond the existing
  Title ID/Media ID reader need a focused fixture and exact field mapping.
- Xbox 360 STFS signatures, licenses, file listings, GOD/SVOD relationships,
  and content-type interpretation are not general game identity proof.
- Xbox 360 title-update supersedence and cumulative sequencing cannot be
  proven from a single local header; no network lookup or installation is
  implied.
- Module hashes requiring compressed/encrypted XEX body access remain
  unverified by the existing Xenia provider.
- DLC, saves, themes, avatar items, and other STFS content must not be
  promoted to base-game identity from Title ID alone.
- No real Xbox 360 specimen was opened during this bounded research pass.
- No GUI, launch, emulator discovery, installer, or mod-apply change belongs
  in this research document.
