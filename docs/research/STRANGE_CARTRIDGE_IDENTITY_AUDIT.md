# Strange Cartridge Identity Audit V1

Status: research-only (2026-09-07).  This audit changes no parsers, hashes,
identity rules, launch adapters, or files in the library.

## Authority and method

The local `feature/archivefs-unified-platform` tree is authoritative at
`eb1ea221bf6cfe31afe3dd176f7e1ca39b8fe3d8`.  The worktree was already dirty
(`ingestion/container.rs`, identity/launch/GUI files, cheat files and Amiga
archive work); none of those changes were edited or staged.  The audit used
the local platform registry, content registry, `game_identity.rs`, launch
tables, coverage inventory, tests, and the existing media ledger.  Remote
history is archaeological only; local code wins when they differ.

## Classification vocabulary

* **STRONG_INTERNAL_ID** — an on-media identifier with documented scope and
  sufficient validation to identify software without a DAT.
* **CORROBORATED_INTERNAL_ID** — useful internal evidence, but requires an
  independent structural or catalogue leg before exact identity.
* **STRUCTURAL_ONLY** — geometry/container or family evidence; no reliable
  per-title identifier.
* **DAT_HASH_AUTHORITATIVE_BY_DESIGN** — the medium is intentionally
  headerless/variable and exact identity is supplied by a canonical hash/DAT.
* **RESEARCH_INSUFFICIENT** — evidence or implementation coverage is not yet
  strong enough to make a safe claim.

## Platform matrix

| Platform | Current EmuWiz | Strongest on-media evidence | Exact identity possible? | DAT/hash role | Final classification | Real gap |
|---|---|---|---|---|---|---|
| Nintendo Virtual Boy | `.vb`/`.vboy` are generic cartridge registrations; launch/platform aliases and DAT identity exist, but no Virtual Boy header parser (`content_registry.rs`, `platform/mod.rs`, `coverage_inventory.rs`) | No universally deployed retail header. Community “Virtual Boy ROM header” proposals are unofficial; RetroAchievements identifies VB by MD5 | Not safely from content alone; hashes can identify a dump | Required for exact title/revision | DAT_HASH_AUTHORITATIVE_BY_DESIGN | No parser gap unless a two-source, real-header standard is established |
| NEC PC Engine / TurboGrafx HuCard | `.pce` is a platform-specific registration and launch mapping; no HuCard parser (`platform/mod.rs`, `game_identity.rs`) | HuCards are raw ROM/mapper-dependent data with no common retail title/serial header; hardware exposes address/data bus rather than a standard metadata block | No general exact identity from bytes without catalogue/hash | Authoritative for title/revision/region; mapper metadata may be separate | DAT_HASH_AUTHORITATIVE_BY_DESIGN | Keep extension as platform hint; do not invent a header parser |
| Sega SG-1000 / SC-3000 | Referenced in launch/RetroArch platform mappings, but no dedicated cartridge structural module or strong extension row was found | Raw cartridge ROM; no universal title/serial header across releases | Not safely in general | DAT/hash or verified external catalogue | RESEARCH_INSUFFICIENT | Establish whether a scoped, authoritative header convention exists before implementation |
| Atari 2600 | `.a26` is a strong platform extension; `.bin`/`.rom` remain weak; Stella launch planning exists; no cartridge-header parser | Most dumps are headerless raw ROM. Optional copier/homebrew headers are tool/container metadata, not universal retail identity | No, except via external DAT/hash | Authoritative for exact title/revision/mapper | DAT_HASH_AUTHORITATIVE_BY_DESIGN | None for safe identity; optional header recognition would be separate and non-authoritative |
| Atari 5200 | `.a52` strong extension; `.bin`/`.rom`/`.car` weak; no internal parser | Common 5200 dumps are raw/headerless and overlap Atari 8-bit conventions | No general exact identity from content alone | Authoritative | DAT_HASH_AUTHORITATIVE_BY_DESIGN | No safe universal header identified |
| Atari Jaguar | `.j64`/`.jag` strong extensions; `.rom`/`.bin`/`.abs`/`.cof` weak; coverage explicitly records no generic internal header; launch mapping only | Jaguar boot blocks can contain per-title/protection/encrypted structures, but are not a universal public identity header | Exact title needs DAT/hash or corroborated external evidence | Authoritative; protection bytes must not be used as title identity | DAT_HASH_AUTHORITATIVE_BY_DESIGN | Research only for a documented, non-encrypted identity leg; do not decode protection |
| Neo Geo cartridge (MVS/AES) | `.zip`/`.7z` arcade sets are family/folder evidence; platform registry says archives alone prove nothing; no single-file cartridge parser | Identity is a multi-ROM set graph (P, S, M, V, C files), with MAME/FBNeo/software-list names and hashes; AES/MVS are distinct set contexts | Yes for a complete canonical set, not from an arbitrary single file | MAME/FBNeo DAT or software list is authoritative | CORROBORATED_INTERNAL_ID | Add set-graph inspection only if it can reuse existing DAT/archive infrastructure; never infer from ZIP filename |

## Platform notes and false-positive policy

Virtual Boy’s proposed header layouts are community/new-format proposals, not
a universal retail contract. The safe local policy therefore remains MD5/DAT
identity (RetroAchievements documents Virtual Boy hash identification).

HuCard byte streams vary by mapper and region and do not expose a common
consumer metadata record. A `.pce` extension distinguishes the ecosystem for
launch purposes, but cannot establish a title. This is intentionally complete
by design rather than a failed parser.

Atari 2600 and 5200 share the headerless-cartridge problem. Copier headers,
homebrew metadata and emulator sidecars must never be mistaken for a retail
identity signal. A 1400/2048/4096-byte size or a filename is not an identity.

Jaguar’s encrypted/protection-related boot material is explicitly excluded
from identity parsing. Protection analysis and title identity are separate
questions.

Neo Geo MVS/AES software is a set, not a universal “cartridge header”. A
complete ROM graph may be matched by MAME/FBNeo/software-list DATs; a lone
`P1` or ZIP basename is insufficient. No automatic MVS↔AES winner is allowed.

Generic `.bin`, `.rom`, `.zip`, `.car`, or `.dsk` content must remain
ambiguous when multiple platforms accept it. Directory and filename names are
context hints only, never exact identity.

## COMPLETE BY DESIGN

The following are intentionally DAT/hash-led because no universal trustworthy
retail identifier exists:

* Virtual Boy
* PC Engine / TurboGrafx HuCard
* Atari 2600
* Atari 5200
* Atari Jaguar (including encrypted/protection-bearing boot material)

This does not mean those platforms lack launch support; it means exact
software identity belongs to canonical content hashes and DAT provenance.

## GENUINE IMPLEMENTATION GAPS

1. **Sega SG-1000/SC-3000 evidence audit (P2).** The local tree exposes launch
   aliases but no dedicated cartridge identity module. Before coding, establish
   from authoritative dumps whether any stable, cross-release header exists;
   otherwise classify it with the same DAT-led policy as Atari/PC Engine.
2. **Neo Geo MVS/AES set-coherence inspection (P2).** Existing archive/DAT
   infrastructure can potentially verify required P/S/M/V/C members and
   distinguish arcade/home set context. This is a set-graph verifier, not a
   single-file header parser, and must preserve incomplete/ambiguous sets.

No other platform above has a proven missing internal identity parser. Adding
one based on extension, size, folklore offsets, or encrypted boot bytes would
increase false positives and is explicitly rejected.

## Evidence references

* Local implementation: [`content_registry.rs`](../../crates/archivefs-core/src/ingestion/content_registry.rs), [`platform/mod.rs`](../../crates/archivefs-core/src/platform/mod.rs), [`game_identity.rs`](../../crates/archivefs-core/src/game_identity.rs), [`coverage_inventory.rs`](../../crates/archivefs-core/src/coverage_inventory.rs).
* RetroAchievements game-identification guidance (Virtual Boy uses MD5): [Game Identification Methods](https://docs.retroachievements.org/developer-docs/game-identification.html).
* Community Virtual Boy header proposal (not treated as a retail standard): [Planet Virtual Boy discussion](https://www.virtual-boy.com/forums/t/establishing-a-rom-format/).
* Neo Geo hardware distinguishes AES/MVS mode in system ROM context; this is not a universal per-cartridge title header: [NeoGeo System ROM notes](https://wiki.neogeodev.org/index.php/System_ROM).
* HuCard is a ROM cartridge with variable capacity/mapper hardware, not a documented universal metadata container: [PC Engine cartridge pinout](https://allpinouts.org/pinouts/connectors/cartridges_expansions/pc-engine-cartridge/).

No production code, parser, identity semantic, launch behavior, or media file
was changed for this audit.
