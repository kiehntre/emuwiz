# Package / Installed-Media Identity Audit V1

**Status:** audit only; no implementation.  **Branch:** `feature/archivefs-unified-platform`.
**Authority:** the local working tree and `docs/MEDIA_SUPPORT_AUDIT.md` at the
audit start.  Existing dirty files were read but not edited.  Historical
cross-checks used the fetched `origin/*` refs, the local feature branches, and
the commits that introduced the relevant parsers, including `914016e`,
`1f21a90`/`0a967a0`, `20de545`, and the modern-Nintendo/Xbox research branches.

## Scope and reading rules

This audit distinguishes three different questions:

1. **Package identity:** what the container or executable declares (for
   example, a PS3 Content ID or Xbox 360 Title ID).
2. **Installed-title identity:** what an extracted directory or installed
   package can prove about the game title.
3. **Readiness/launch:** whether required files, licenses, firmware, keys,
   emulator configuration, and a supported launch input exist.

An internal ID is therefore not automatically a release identity, and a
structurally valid package is not automatically complete or installable.
Whole-file DAT/hash identity remains the release-level authority where a
format-specific catalogue is not available.

## Findings matrix

| Format | Current parser | Internal ID | Version/region | Readiness | Launch | Exact gap |
|---|---|---|---|---|---|---|
| WHDLoad directory/package | **MATURE** — `.slave` HUNK parser, bounded LHA member discovery, DAT reconciliation, HDF traversal support | SHA-256/SHA-1 of exact validated slave; DAT package identity; slave name/title fields are metadata | Slave runtime version and Kickstart requirement parsed; region is DAT/package context, not invented from filename | **MATURE** for validated slave + eligible Amiberry/FS-UAE profile + Kickstart; incomplete/multiple slaves are blockers | **MATURE** — Amiberry/FS-UAE command planning and execution path | No Amiga command executor beyond the existing WHDLoad execution path; no new identity parser needed |
| PS3 PKG | **MATURE parser / production-wired** — fixed 0x80-byte header, bounded range validation, Content ID grammar | Raw Content ID plus derived 9-character PS3 Title ID; package type/revision are container facts | Header revision/type parsed; region is encoded by title/content ID only when canonical metadata interprets it | **READINESS_ONLY** — valid header proves neither RAP/license, firmware compatibility, installation, nor playable payload | **No direct PKG launch**; RPCS3 launch consumes a verified title identity from an installed/disc-compatible target | Model PKG-vs-installed RPCS3 content and RAP/firmware/install-state readiness without decrypting or installing |
| PS3 installed folder (`PS3_GAME`) | **MATURE parser / production-wired** — `PARAM.SFO`, `USRDIR/EBOOT.BIN`, SELF magic, optional `PS3_DISC.SFB` magic | `TITLE_ID` from bounded `PARAM.SFO` → `Ps3TitleId` | `APP_VER`, `CATEGORY`, title are parsed; region is a title-ID/catalogue concern | **READINESS_ONLY** — folder shape and SELF do not prove license, firmware, or complete install | **IDENTITY_ONLY** for the folder path; RPCS3 planner is wired for verified PS3 identity, but install-state acceptance is not a package installer | Explicit installed-content completeness/license state and launch-input policy |
| PSP PBP | **MATURE parser / production-wired** — magic, version, eight offsets, bounded embedded `PARAM.SFO` | `DISC_ID` → `PspDiscId`; PBP is also a shared PS1 Classics container | PBP version and SFO `DISC_ID`; region is encoded in the product ID/catalogue, not inferred by parser | **READINESS_ONLY** — valid offsets/SFO do not prove DATA.PSAR is a complete game or distinguish all PS1 Classic payloads | **IDENTITY_ONLY** — PPSSPP launch consumes `PspDiscId`, but no PBP-specific extraction/mount/install path is added | Classify PSP game vs PS1 Classic using bounded PSAR markers and define PPSSPP PBP launch input |
| PSP installed/UMD folder (`PSP_GAME`) | **MATURE parser / production-wired** — UMD layout and `PARAM.SFO` evidence | `DISC_ID` → `PspDiscId` | SFO `DISC_VERSION`, title/category, and UMD marker; region from ID/catalogue | **MATURE identity; readiness-only for firmware/content completeness** | **MATURE** through PPSSPP planner/execution when the resolved input is accepted | No package installer; archive/folder-to-launch-input policy remains separate |
| Wii WAD | **No parser**; only Wii platform strong-extension registration (with current Wii/Wii U `wad` registry drift) | No ticket/title metadata or internal title ID exposed | None | **RESEARCH_REQUIRED** — WAD may be channel, VC, DLC, or system content; tickets/content encryption matter | None | Decide supported WAD classes and safe metadata boundary; remove misleading Wii U weak `wad` claim or document it |
| 3DS CIA | **No parser**; `.cia` is a platform strong extension only | No CIA title ID, content index, version, or product-code fact | None | **RESEARCH_REQUIRED** — encrypted NCCH/content, tickets, certificates, and seed/key context affect installability | None; no 3DS emulator adapter | Establish CIA content model and a no-decryption metadata boundary before wiring identity |
| Switch NSP | **No parser**; platform row recognizes extension only | No CNMT/NCA Title ID or content-role facts | None | **RESEARCH_REQUIRED** — NCA/PFS0 metadata is not a playable-title guarantee; keys/signing and base/update/DLC relationships are absent | None; no Switch adapter | Bounded PFS0/NCA/CNMT research, then separate base/update/DLC readiness from launch |
| Switch XCI | **No parser**; platform row recognizes extension only | No cartridge Header/Cert/NCA Title ID facts | None | **RESEARCH_REQUIRED** — encrypted/signed XCI/NCA content and key context; container validity is not game completeness | None; no Switch adapter | Research safe cartridge-header identity and distinguish cartridge container from extracted NCAs |
| Switch XCZ | **No parser or registry row**; not a current EmuWiz-supported format | None | None | **RESEARCH_REQUIRED** — compressed XCI semantics and keys are unresolved | None | Confirm whether XCZ is in scope and whether bounded outer-compression inspection is worthwhile |
| Original Xbox XBE | **MATURE parser / production-wired** — XBEH, virtual-address certificate offset, Title ID, UTF-16 title | `XbeTitleId` from XBE certificate; title name is display metadata | No executable revision/region authority; certificate fields are parsed, release region comes from DAT/catalogue | **MATURE** for XDVDFS + `default.xbe` disc evidence and xemu profile; direct loose XBE is identity-only | **MATURE** for verified Xbox disc/XISO through xemu; loose XBE is not a launch target | Broaden accepted direct/extracted layouts only if a safe xemu launch contract is defined |
| Xbox 360 XEX/XEX2 | **MATURE parser / production-wired** — XEX2 optional-header table, Title ID and Media ID | `XexTitleId`, `XexMediaId`; executable metadata is not disc/package identity | Execution-info fields include media/version facts; region is not asserted from XEX alone | **MATURE** for verified XEX/XDVDFS/Xenia profile; package completeness is separate | **MATURE** through Xenia title/media-ID gate; no installer | Join package/disc version and multi-disc facts to grouping; no deeper XEX parser justified |
| Xbox 360 STFS (`CON`/`LIVE`/`PIRS`, including GOD/XBLA/DLC envelope) | **MATURE metadata-only parser** — fixed header fields, no directory walk/extraction | Title ID, Media ID, content type, version/base version, disc number/set | Version/base-version and disc fields are parsed; region/content category is not decoded into a game claim | **READINESS_ONLY** — signatures/licenses are deliberately not verified; STFS envelope also contains saves, DLC, avatar items, and other content | **IDENTITY_ONLY / READINESS_ONLY** — Xenia path accepts suitable identity, but GOD/STFS install semantics are not modeled | Interpret content type for display/readiness and join version/disc fields to title grouping, without treating STFS as “a game” |
| Wii U RPX installed/executable layout | **No RPX parser**; `.rpx` is a Wii U strong extension | No RPX Title ID or executable identity fact | None | **RESEARCH_REQUIRED** — RPX is executable content inside a larger installed/title layout; tickets, meta, code/content/meta completeness and keys are separate | None; no Cemu adapter | Define installed Wii U layout and safe `meta.xml`/RPX boundary before launch wiring |
| Wii U WUD | **No parser**; extension is a weak/registered platform hint only | No disc header/title ID fact | None | **RESEARCH_REQUIRED** — optical container may be complete while encrypted partitions remain unusable without keys | None | WUD partition/header research and relationship to extracted/install layout |
| Wii U WUX | **No parser**; extension is a weak/registered platform hint only | No WUX/WUD inner title identity fact | None | **RESEARCH_REQUIRED** — compressed WUD wrapper; compression validity is not title completeness | None | Bound decompression/container inspection and preserve WUD/WUX-as-container vs installed-title distinction |

## Format notes and cross-cutting decisions

### WHDLoad benchmark

WHDLoad is the benchmark because it has all three layers: exact slave
identity, package/DAT reconciliation, and a real readiness/launch path. The
parser validates Amiga HUNK structure and WHDLoad security/ID fields; the
archive path discovers slaves inside LHA without trusting the archive name;
the identity source hashes the exact slave. `kick_name`, memory requirements,
profile eligibility, and Kickstart availability remain readiness facts. The
GUI surfaces discovery, DAT sources, and launch readiness. No missing parser
should be assigned here.

### Sony package versus installed-title identity

The current local tree wires both PS3 PKG and PSP PBP through discovery and
`game_identity`. PS3 PKG reads only the fixed header and derives a candidate
Title ID from the Content ID grammar. It deliberately does not read the
metadata table or payload. A valid PKG is therefore package identity, not an
installed RPCS3 directory and not proof that a RAP/license or compatible
firmware is present.

PBP reads the fixed table and embedded SFO. `DISC_ID` is enough for PSP
identity and PPSSPP's existing identity gate, but `DATA.PSAR` remains opaque.
The same container can represent PSP content or PS1 Classics, so the PS1
serial is not manufactured from the extension or filename.

PS3/PSP extracted layouts are separate parsers: `PS3_GAME/PARAM.SFO` and
`PSP_GAME/PARAM.SFO` are useful installed/disc identity seams, but `PARAM.SFO`
metadata does not prove encrypted content, licenses, or a complete install.
The shared SFO parser is correctly reused.

### Nintendo packages and optical containers

Current platform registration for Wii WAD, Wii U WUD/WUX/RPX, 3DS CIA, and
Switch NSP/XCI is not a parser. No internal IDs, versions, regions, content
roles, or readiness facts are currently extracted for these formats. Switch
XCI is the cartridge container; NSP is a package/install distribution; NCA is
an inner content container. They must not be collapsed into one “Switch ROM”
identity. XCZ is not currently registered and needs an explicit scope choice.

WUD/WUX similarly identify an optical/compressed container family, while RPX
belongs to an installed/executable layout. A future Cemu path must not treat a
WUD header as proof that an RPX/code/content/meta install is complete.

### Xbox executable versus package/disc identity

XBE and XEX are executable metadata. Original Xbox disc identity is the
XDVDFS + `default.xbe` structure; Xbox 360 disc identity is XDVDFS +
`default.xex`/XEX2. STFS is a package envelope shared by games, DLC, saves,
title updates, and other content. Its parsed Title ID/Media ID/version fields
are valuable evidence but its content type is intentionally raw and its
signatures/licenses are not verified. This separation is correct and should
not be replaced by filename or extension claims.

### DAT/hash role

DAT/hash matching is release identity, not package installability. For
WHDLoad, DAT reconciliation is format-aware and hashes the slave/package
artifact. For PS3/PSP/Xbox formats, the internal IDs corroborate the title
while the exact package/disc/executable bytes remain the hashable object.
Modern Nintendo formats have no current parser or ingestion path, so no DAT
identity can be safely projected from their advertised extensions.

## Classification summary

- **MATURE:** WHDLoad directory/package; PSP `PSP_GAME` layout; PS3
  `PS3_GAME` identity path; original Xbox XBE/XDVDFS; Xbox 360 XEX/XDVDFS;
  Xbox 360 STFS metadata.
- **PARSER_EXISTS_NOT_WIRED:** none among the requested bounded formats at
  this local-tree snapshot. Historical research documents called PS3 PKG and
  PSP PBP orphaned, but the current local tree contains their production
  discovery/identity joins; the older classification is superseded.
- **IDENTITY_ONLY:** direct XBE/XEX executable metadata where no disc/package
  context exists; PSP PBP/PS3 PKG package identity before readiness checks;
  RPX/Xbox package facts where the surrounding installed content is absent.
- **READINESS_ONLY:** PS3 PKG licensing/firmware/install state; PSP PBP
  payload completeness; STFS license/signature/content-class semantics.
- **DAT_HASH_LED:** exact WHDLoad slave/package hash and generic exact-byte
  hashes for PS3/PSP/Xbox artifacts.
- **RESEARCH_REQUIRED:** Wii WAD; 3DS CIA; Switch NSP/XCI/XCZ; Wii U RPX,
  WUD, and WUX; any encrypted package/install semantics requiring keys,
  signing, tickets, or license interpretation.
- **INTENTIONALLY_DEFERRED:** decryption, key handling, package installation,
  ticket/license verification, and full content extraction for all formats.

## Fast wins

There are no parser-exists-not-wired fast wins for the requested formats in
the current working tree. The closest safe wins are readiness/identity joins:

- add an explicit PS3 PKG readiness projection that reports “header valid”
  separately from RAP/license, firmware, and installed-content state;
- add bounded PBP content-class evidence for PSP versus PS1 Classic after
  two-source marker verification, without touching the payload;
- expose existing STFS `content_type`, `version`, `base_version`, and
  `disc_number/disc_in_set` as truthful GUI/readiness metadata and join the
  disc fields to grouping;
- correct the Wii/Wii U `wad` registry drift and add honest Doctor wording;
- document/directly test the existing PS3/PSP installed-folder launch-input
  boundaries without adding an installer.

## Research-gated

The modern Nintendo set is blocked by missing, unclear, or encrypted
semantics rather than by a small registry seam. CIA, NSP, XCI/XCZ, WAD,
WUD/WUX, and RPX need a format-specific evidence model before identity can be
made authoritative. Safe work may inspect fixed plaintext signatures and
bounded metadata once independently verified, but must not infer title
identity from an extension, decrypt content, process keys, verify licenses,
or install packages. The same rule applies to PS3 RAP/NPDRM and Xbox
certificate/license semantics.

## Top five real implementation opportunities

1. Model PS3 package/install readiness as separate states: validated PKG
   header, title identity, installed-content presence, RAP/license evidence,
   and RPCS3 firmware/profile readiness.
2. Complete the PSP PBP identity seam with a reviewed, bounded PSAR content
   class and an explicit PPSSPP launch-input policy for PBP versus extracted
   UMD content.
3. Promote Xbox 360 STFS metadata into display/readiness and multi-disc
   grouping while retaining the raw content-type and no-license-verification
   boundary.
4. Establish a research-backed modern Nintendo container contract, starting
   with Switch NSP/XCI separation and Wii U WUD/WUX versus RPX installed
   layout; stop at plaintext metadata until keys/signing semantics are scoped.
5. Repair platform/extension drift and installed-layout diagnostics: Wii WAD
   versus Wii U WAD, missing XCZ scope, and explicit PS3/PSP completeness
   blockers in GUI/Doctor.

## Audit result

The package/installed-media identity audit is complete. Existing production
code was not changed; the report records current local working-tree behavior,
not just the older committed research snapshots.
