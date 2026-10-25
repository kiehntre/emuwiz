# Tape / Cassette Format Support Audit — EmuWiz (RESEARCH ONLY)

> **Research snapshot** — This audit records repository findings at the time it was written. It is not current capability documentation; see the [README](../../README.md), [adapter support matrix](../ADAPTER_SUPPORT_MATRIX.md), and [roadmap](../../ROADMAP.md) for present guidance.

**Scope:** every tape/cassette extension claimed by any canonical EmuWiz platform — Commodore TAP/T64/CAS, ZX Spectrum TAP/TZX, Amstrad CDT/VOC, Acorn UEF, MSX CAS, Atari 8-bit CAS, plus WAV/PZX/CSW assessment
**Branch:** `feature/archivefs-unified-platform`
**Method:** static source analysis only — no source modified, no commits. Tape-format specs below are documented for future parser work and marked as needing the crate's two-source verification bar before any implementation.

---

## 1. COMPLETE REPOSITORY INVENTORY

Tape-aware registries (verified against the full tables):

| Ext | Platform claim(s) | Strength | content_registry | media_registry | inspector | Parser | Production caller | Identity | DAT | Tests | Real corpus |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `.tap` | ZX Spectrum, C64, C128, VIC-20 (all weak) | weak ×4 | ✅ `TapeImage` (`:128`) | ✅ (`:143`) | ✅ | **none** | n/a | none | whole-file generic | none | none |
| `.tzx` | ZX Spectrum (**strong**, `:1747`) | strong | ✅ `TapeImage` (`:129`) | ✅ (`:147`) | ✗ | **none** | n/a | none | whole-file generic | none | none |
| `.cdt` | Amstrad CPC (**strong**, `:552`) | strong | ✅ `TapeImage` (`:127`) | ✅ (`:139`) | ✗ | **none** | n/a | none | whole-file generic | none | none |
| `.t64` | C64 (**strong**, `:802`) | strong | ✗ | ✗ | ✅ (`inspector.rs:118`) | **none** | n/a | none | — | none | none |
| `.cas` | Atari 8-bit, MSX, C64, VIC-20 (all weak) | weak ×4 | ✗ | ✗ | ✗ | **none** | n/a | none | — | none | none |
| `.uef` | BBC Micro, Acorn Electron (both weak) | weak ×2 | ✗ | ✗ | ✗ | **none** | n/a | none | — | none | none |
| `.voc` | Amstrad CPC (weak, `:553`) | weak | ✗ | ✗ | ✗ | **none** | n/a | none | — | none | none |
| `.pzx` / `.csw` / `.wav` | **claimed by no platform row** | — | ✗ | ✗ | ✗ | none | n/a | none | — | none | none |

Status classifications: `.tap`/`.tzx`/`.cdt` = **REGISTERED-ONLY** (recognised media, zero structure); `.t64`/`.cas`/`.uef`/`.voc` = **UNSUPPORTED at every scanner layer** (platform claims only); `.pzx`/`.csw`/`.wav` = **not claimed** (deliberately absent).

**Correction to the task premise:** §10 assumes *"the already completed UEF evidence work"*. **No UEF code exists anywhere in the repository** — `rg -i uef` matches only the substring inside `CheatCatalogue**F**ormat`-style identifiers; there is no UEF module, detector, test, or registry row. Nothing is being misjudged as immature — it simply has never been built. UEF enters this audit as a *new-work* format, not a wiring job.

**What tape files experience today:** `.tap`/`.tzx`/`.cdt` are discovered as `ContentKind::TapeImage` (persisted as `"tape_image"`, `database.rs:1504`) with platform from folder aliases only; `.t64`/`.cas`/`.uef`/`.voc` skip discovery as `UnsupportedExtension` (`discovery.rs:583-585`). `game_identity` contains **zero tape references**; no tape detector exists in `archive_member_content_evidence.rs:163-172`; no fusion rule, no coverage row, no emulator planner touches tape.

## 2. EXTENSION COLLISION MAP

| Ext | Claimants | Actual formats | Byte-level discriminator | Current behavior | Risk |
|---|---|---|---|---|---|
| `.tap` | ZX Spectrum, C64, C128, VIC-20 | **Commodore TAP** (ASCII `C64-TAPE-RAW` @0, 8-bit-machine byte @12) vs **ZX Spectrum TAP** (no signature: LE u16 block length, flag byte, data, XOR checksum) | C64 magic presence (8 bytes of fixed ASCII) — decisive | ambiguous; platform from folder only | **High** — the same file class resolves differently by folder; no structural separator exists |
| `.cas` | Atari 8-bit, MSX, C64, VIC-20 | **Atari CAS** (`FUJI` magic @0 + chunk framing) vs **MSX CAS** (headerless; short/long leaders + `0x1F 0xA6`-style block headers with type/filename/addresses) vs C64/VIC-20 (no distinct CAS convention — claims are vestigial) | `FUJI` → Atari; `0x1F`-header block chain at a valid pilot offset → MSX; neither → ambiguous | ambiguous; folder-only | **High** — four claimants, two real formats, zero parsing |
| `.tzx` / `.cdt` | ZX Spectrum / Amstrad CPC | **same container** (TZX spec; CDT = CPC-convention TZX) | `ZXTape!\x1a` signature @0 (identical in both) — extension is **not** a valid discriminator; folder/DAT is the only honest resolver | **LIVE FALSE POSITIVE** — a valid `.cdt` receives ZX Spectrum `Signature` evidence and is reported *Probable ZX Spectrum*; CPC gets no equivalent candidate | **HIGH / P0** — confirmed live platform misidentification, not theoretical ambiguity; fixed by `cpc-zxtape-signature-parity` (§31) |
| `.uef` | BBC Micro, Acorn Electron | same format (UEF = EUG/UEF chunked tape+dps container, gzip-wrapped) | none — family-level by design (row explanations already say so) | unsupported | Low (honest weakness) |
| `.t64` | C64 only | no collision | T64 signature (`"C64 tape image file"`, see §4) | unsupported | Low |
| `.voc` | CPC only | Creative VOC audio | VOC block framing | unsupported | Low |
| `.wav` | none claimed | sampled audio | RIFF/WAVE | absent | N/A — see §14 |

### 2a. CDT/TZX collision — the exact live path (P0)

`platform/detect.rs::signature_evidence` scans **every** platform's magic rules against the file's leading bytes, regardless of the file's extension. The ZX Spectrum registry row currently owns the only `ZXTape!\x1a` `MagicRule` (at `MagicConfidence::Corroborated`); the Amstrad CPC row owns none. Because CDT is byte-for-byte TZX-compatible, a genuine Amstrad `.cdt` beginning `ZXTape!` today follows:

```
.cdt file
  -> signature_evidence global magic scan (extension ignored)
  -> `ZXTape!\x1a` matches the ZX Spectrum row's Corroborated signature
  -> Amstrad CPC row contributes NO equivalent signature candidate
  -> only-strong-tier evidence points at ZX Spectrum
  => reported: "Probable ZX Spectrum"   (for an Amstrad CPC tape)
```

This is a **live false positive**, not merely shared-extension ambiguity: with no folder or DAT evidence, EmuWiz returns a specific wrong platform rather than "ambiguous". It is the single confirmed live tape-platform misidentification found by this audit.

**Proposed safe behavior:** apply the *same* `Corroborated` `ZXTape!\x1a` `MagicRule` to the Amstrad CPC row. Both platforms then receive equal `Signature`-tier evidence, producing **honest ambiguity between Amstrad CPC and ZX Spectrum** whenever no stronger (folder alias / DAT hash / structural) evidence exists. Constraints: **do not** resolve CDT vs TZX by extension alone (both are the same container); **do not** promote `ZXTape!` to `Strong` (it proves "TZX/CDT container", never a single platform).

## 3. COMMODORE TAP (spec for future parser)

- Signature: ASCII `C64-TAPE-RAW` at offset 0; version byte @0x0C (0/1/2); machine byte @0x0D (C64=0, VIC-20=1, C16=2 — **a real, spec-level machine discriminator inside a shared-collision extension**); header size field; LE u32 data length.
- Pulse stream: each byte = pulse width in 8×64 µs units; `0` = overflow pulse (10-bit-width cascade, version-specific semantics).
- Bounded parser feasibility: **high** — fixed 20-byte header, magic + version + machine validation, data length vs file length accounting. Yields `Strong`-grade container evidence for the **Commodore 8-bit family** (C64/VIC-20/C16 — note the machine byte can *narrow within the family* but the family spans three canonical platforms, so per-platform claims need care: it is structural family evidence, never a single-platform proof). No program decoding — pulse bytes are opaque.
- Two-source bar: TAP v0/v1/v2 documented in VICE's `tape.c` and the TAP 2.0 specification (Final TAP); verify both before merge.

## 4. T64 (spec + architectural answer)

- Signature `"C64 tape image file"` (24 bytes incl. version), 2 header entries: max entries (usually 64), used entries; 32-byte directory entries: entry type (fixed/free), C64S file type (PRG/SEQ/…), start/end load addresses (LE), payload start/end offsets (LE), 16-byte C64 filename (PETSCII).
- Malformed-input rules: entry offsets must be monotonic within `[header_end, file_len]`; `end−start` address deltas should match payload span; free entries skipped; overlapping payload windows must refuse.
- **Architectural answer:** T64 is a **tape container with PRG-like members** — best modeled as `TapeImage` at platform level with *bounded member enumeration* (like archive-member evidence: enumerate entries, expose filenames/addresses as provenance facts), **not** as an Archive content kind and **not** via recursive extraction. No member is a launchable file by itself. EmuWiz should expose contained members as display/provenance facts only.

## 5. ZX SPECTRUM TAP (spec)

- Structure: sequence of blocks; each = LE u16 length + flag byte (`0x00` header / `0xFF` data) + payload + single XOR/ADD checksum byte. Header blocks carry: type (Program/Number array/Bytes/Code), 10-byte filename, param1/param2 (line/positions or start/length), data length.
- What proves Spectrum tape structure: **only the block framing discipline** (lengths/chaining/checksums coherent to EOF). Filenames and BASIC line numbers are **provenance only**; DAT hash is exact identity. No magic exists — a valid ZX TAP is a *pattern* claim (Corroborated at best, and weaker than Commodore TAP's ASCII signature).
- **Collision note:** because ZX TAP has no signature, the C64 magic is the only safe first-discriminator for shared `.tap`; a file without `C64-TAPE-RAW` that validates as ZX block chains may be reported family-ambiguous across the four `.tap` claimants.

## 6. TZX (spec + safety analysis — the dangerous format)

- Signature `ZXTape!` + major/minor version; then an ID-tagged block stream. Block families: standard/turbo/pure-tone/pulse-sequence/pure-data/direct-recording/generalized-data (IDs 0x10–0x19), pauses (0x20), group start/end (0x21/0x22), jump (0x23), loop start/end (0x24/0x25), call sequence/return (0x26/0x27), select (0x28), stop (0x2A), text/description/archive-info (0x30–0x32), hardware type (0x33), custom/unknown (0x35+), CSW recording (0x18/0x15 variants), `emu-state` etc.
- **Why it is dangerous:** IDs 0x23/0x24/0x25/0x26/0x28 are a *program* — a parser that naively follows jumps/loops/calls walks an attacker-controlled control-flow graph with unbounded iteration; 0x18/CSW blocks carry RLE-expansion fields; direct/generalized data blocks declare huge pulse counts; metadata blocks declare huge text.
- **Structural validation without executing the tape program:** parse block framing linearly (tag + length + skip), never follow control flow. Validate: signature/version, per-block declared lengths vs remaining bytes, terminator (0x2B?) handling, unknown-block forward compatibility (skip by length). Emit: container Strong evidence + block-type inventory + counts.
- **Mandatory caps (crate-convention numbers, to be finalized against `disk_format/mod.rs` limits):**
  - max input bytes: reuse the tape-size ceiling (≤ 8 MiB proposal)
  - max blocks: 65,536; max block length: 8 MiB (direct-recording blocks legitimately reach megabytes); max metadata (text/archive-info) length: 4,096 B per block
  - max loop nesting: **0** (loops are counted, never followed: a Loop-Start without a matching Loop-End is still frameable — count pairs, never iterate)
  - max jump/call targets: **0 followed** (record presence/count only)
  - max referenced data bytes: sum of declared block lengths vs file length (must account exactly)
  - max CSW-in-TZX expansion: refuse or cap at the CSW caps below
  - work factor: linear pass only; no per-pulse iteration above the declared cap; parse time bounded by bytes-read
- Two-source bar: TZX 1.20/1.21 spec (ZX Format / world-of-spectrum) + a reference implementation (`tzxcat`/`tape2wav`/Fuse `tzx.c`).

## 7. PZX (spec + priority)

- Signature `PZXT` + version; then length-prefixed chunks: `PULS` (pulse count + timing table), `DATA` (bitstream w/ b-pulse/t-pulse params), `PAUS`, `BRWS` (browse info), `STOP` (pause/stop flags), plus unknown chunks skipped by length. Chunk framing is uniform (4-byte ID + u32 LE length) — **strictly easier and safer to validate than TZX**: no control-flow blocks at all, flat length-accounting.
- Priority: **P1** (after TZX) — the same consumer need, lower risk, and the parser is nearly mechanical. If TZX lands first, PZX is a small sibling; if the family needs one preservation-timing format first, PZX is the safer choice.

## 8. CSW

- Signatures `CSWMP` (v1) / `COMPRESSED SPECTRUM WAVE` (v2); sample rate (u32), compression type 1 (RLE) / 2 (Z-RLE), pulse-policy flags; the body is a compressed pulse stream (RLE: byte + count; Z-RLE: two-byte escapes).
- **Bomb risk:** expansion ratio is data-dependent (each 2-byte escape can expand to 255 bytes; a stream can be gigabytes when expanded). Any implementation must cap expanded bytes (proposal: ≤ 64 MiB expanded, streaming counting without materialization) — otherwise parse **container only** (magic/version/rate/compression-type) and stay hash-only.
- Recommendation: **container-only now; bounded streaming expansion only if a real consumer needs pulse counts.** Expanded-size must never be allocated up front.

## 9. AMSTRAD CDT

- CDT **is TZX with CPC conventions** (same block grammar; CPC-specific loader timings live inside standard block parameters). The audit's architecture answer: **one shared TZX parser, two platform-context consumers** — the parser emits container/block facts; the CPC row's `cdt` claim and ZX row's `tzx` claim stay folder/DAT-corroborated. **Do not build a second engine.**

## 10. ACORN UEF

**Task-premise correction (verified): no UEF support exists in this repository** — no module, no test, no registry row, no coverage row (`rg -i uef` matches are the substring inside `CheatCatalogueFormat` and similar). UEF is new work:
- Format: gzip-wrapped chunk stream; each chunk = 2-byte ID + 2-byte length + payload; tape chunks 0x0100-0x0110 (gap/tone/data/integer-data/secure-data/…), provenance chunks 0x0000-0x0005 (originator info/title/publisher/…).
- **BBC/Electron ambiguity is by design** (both rows claim `.uef` weak; both explanations already say family-level) — a UEF parser must remain family evidence.
- gzip wrapping inherits the ADZ-style bounded-decompression question: output cap mandatory (UEF tapes ≤ a few MB).
- Priority: P1 (CPC/BBC families are otherwise tape-dark), classified as a **new parser** (Small–Medium), not a rebuild.

## 11. MSX CAS (spec for future parser)

- Headerless byte stream: pilot/leader tones, then repeating block structure: sync bytes (`0x1F 0xA6`-family per MSX tape spec), block header with file type (`0xD0` binary / `0xD3` BASIC / `0xEA` ASCII), 6-byte filename, start/end execution/load addresses, short data blocks (`0x20`-type continuation blocks), checksums (XOR of header fields).
- Discrimination vs Atari CAS: **Atari begins with the literal `FUJI` magic; MSX begins with pilot tones** (no fixed magic — detection is structural: `0x1F`-headed blocks with valid XOR checksums after leader runs). Neither proves a platform alone; both give family-level structural evidence, corroborated by folder. C64/VIC-20 `.cas` claims: no real CAS convention — those claims are vestigial and should be dropped or documented as such.
- Two-source bar: MSX Red Book tape spec + blueMSX/openMSX `cassette.cpp`.

## 12. ATARI CAS (spec — the collision answer)

- `FUJI` magic (4 ASCII bytes) @0 + 2 version bytes + optional origin field; then chunk records: LE u32 length + 2-byte type (`0x00` FUJI chunk w/ baud, `0x01` baud, `0x02` data, `0x03` offset/unknown passthrough), data chunks carry the FSK-decoded byte stream; unknown types must be length-skipped.
- **Valid `FUJI` CAS is strong Atari-8-bit-family structural evidence** — the fixed ASCII magic immediately separates Atari from MSX (the audit's long-standing `.cas` four-way collision, flagged in the Atari review). Family-level: the magic says "Atari 8-bit tape", and the platform row is exactly that (`Atari 8-bit` folds 400/800/XL/XE). Caps: chunk length vs remaining bytes, max chunks, max data chunk (400/800 baud × minutes ⇒ ≤ a few MB), total accounted == file length.

## 13. OTHER CAS/TAPE SYSTEMS

Verified against the full platform registry: **Oric, Dragon/Tandy CoCo, Thomson, TI and other micros have no platform rows at all** — no tape claims exist for them. The only other tape-adjacent claim is CPC's `.voc` (audio, §14) and the ZX strong `tzx`/weak `tap`. Nothing is omitted: the claimant list in §1-2 is exhaustive.

## 14. WAV / VOC / AUDIO TAPE

- `.wav` is claimed by **no platform**; `.voc` is weak-claimed by CPC. No RIFF parser exists.
- **Decision: sampled audio should never be decoded to blocks by EmuWiz core.** Reasons: pulse decoding requires normalization against speed variation/noise (guaranteed heuristics), stereo/mono and sample-rate variance multiply edge cases, resource cost is unbounded for long tapes, and the crate's bar (two-source, deterministic, fail-closed) is structurally unattainable for DSP recognition. Audio tapes stay **hash/media-only**. If conversion is ever wanted, it belongs in an **external-tool workflow** (e.g. a documented `audio2tzx` pre-step producing a TZX EmuWiz can parse), not in core. VOC: same answer; treat as audio-provenance media at most.

## 15. LOGICAL BLOCK VS PULSE PRESERVATION — TAXONOMY

| Class | Formats | Identity semantics | Evidence ceiling |
|---|---|---|---|
| **LOGICAL BLOCK** | ZX TAP, T64 | Content-adjacent: files/blocks are enumerable; internal names = provenance; whole-image hash = identity | `Strong` container + `Corroborated` member facts |
| **STRUCTURED TIMING** | TZX, CDT, PZX | Loader/timing structure is preservation-meaningful; block framing parseable; internal text = provenance | `Strong` container + block-type inventory; **no** timing interpretation |
| **RAW PULSE** | Commodore TAP, CSW | Pulse stream is opaque unless decoded; width bytes/counts are preservation data | `Strong` container (signature-verified); everything else hash-only |
| **SAMPLED AUDIO** | WAV, VOC | No structural tape meaning without DSP | `HASH-ONLY` (media facts at most) |

Confidence must differ by class: a signature-verified raw-pulse container is `Strong` *container* evidence but can never yield per-file facts; a logical-block container can enumerate members; structured-timing sits between. This taxonomy should drive which facts each parser is *allowed* to emit.

## 16. TURBO / COPY PROTECTION / CUSTOM LOADERS

- **Commodore TAP:** pulses are the *recording*; turbo loaders (Riot, Novaload) and protection live entirely in pulse widths — opaque bytes to EmuWiz, perfectly preserved. No interpretation attempted.
- **ZX TAP:** captures only standard ROM framing — turbo/custom loaders are **not representable** (that is TZX's purpose). A `.tap` that isn't ROM-framed simply won't validate — honest refusal.
- **TZX/CDT/PZX:** turbo (ID 0x11), pilot/sync overrides, pure-data, direct-recording and generalized-data blocks preserve custom loaders *structurally* — EmuWiz validates their framing and inventories their types **without interpreting timing**. Copy protection survives as whatever bytes/blocks it is; EmuWiz claims no understanding.
- Program-level parsing (BASIC tokenization, loader disassembly) is **out of scope permanently** — the crate proves structure, not semantics.

## 17. DAT / HASH SEMANTICS

- Ecosystems: TOSEC (all families above), No-Intro ( tapes where published), plus the generic Logiqx pipeline — all whole-image hashing, all already supported once a file is ingestible (`.tap`/`.tzx`/`.cdt` are; `.t64`/`.cas`/`.uef` are not).
- **Normalization: do NOT normalize any pulse/timing format.** TZX/CDT/PZX/CSW/TAP bytes *are* the preservation artifact (timing streams with no canonical logical form — two dumps of the same tape differ legitimately in pauses/turbo framing). Normalizing would break DAT identity and invent equivalence that preservationists explicitly reject. The single safe variant class is T64 member-ordering (not worth it).
- Member/block hashing (T64 entries, ZX TAP blocks): useful as *provenance/display* facts only; never DAT keys.
- Headered/unheadered: N/A (no header-stripping convention exists in tape formats).

## 18. INTERNAL FILENAMES / TITLES

| Format | May contain | Status |
|---|---|---|
| ZX TAP | 10-byte filename, BASIC line, load/start addresses | PROVENANCE ONLY |
| T64 | 16-byte C64 filename (PETSCII), load/end addresses | PROVENANCE ONLY |
| TZX/CDT | Text/description/archive-info blocks (title, author, publisher, remarks) | PROVENANCE ONLY |
| PZX | BRWS text | PROVENANCE ONLY |
| UEF | originator/title/publisher chunks | PROVENANCE ONLY |
| Commodore TAP / CSW / WAV | none (pulse/audio) | N/A |

No canonical rename from tape metadata, ever, without a verified DAT — consistent with every family audit.

## 19. ARCHIVE MEMBER SUPPORT

- ZIP/7z/RAR/LHA members: the bounded member-evidence layer (`archive_member_content_evidence.rs`, 64 KiB prefix, classified members only) currently runs **no tape detector**; `.tap`/`.tzx`/`.cdt` members get generic classification only. A tape container parser must be prefix-friendly (all candidates above validate from the first ≤ 24 bytes except ZX TAP, whose block chains need whole-member reads — cap at the member's declared size with the standard bounds).
- **T64 members-as-members:** T64 internal files should behave like archive members for *display* (enumerable, bounded, non-extractable) — via a bounded directory walk over the T64 entry table, never extraction. No unbounded recursion anywhere.

## 20. EMULATOR LAUNCH

No tape-capable emulator adapter exists in the repo — VICE, Fuse, Caprice, openMSX, Atari800, BBC emulators, PUAE-tape modes: all absent as planners (PUAE exists only as an Amiga core hint; FS-UAE/Amiberry inspect configs but never tape media). RetroArch could pass `.tap`/`.tzx` to cores generically **once tape platforms have identity variants and launch rows** — but no autoload/Play-flag semantics are modeled, and none should be invented per-format. Launch today: **none** for any tape format.

## 21. DOCTOR (proposed findings)

| Finding | Class |
|---|---|
| Malformed tape container (bad magic/framing) | BLOCKING (identity) |
| Truncated block/chunk (declared > remaining) | WARNING |
| Unsupported/unknown block type | INFO (forward-compatible skip is valid) |
| Unsafe control-flow structure (TZX unbalanced loop/jump) | INFO (counted, never followed) |
| Ambiguous platform (shared ext, no discriminator) | WARNING |
| Valid raw-pulse image, loader unknown | INFO |
| Audio cassette (WAV/VOC) | INFO — unsupported by design |
| Emulator cannot autoload | WARNING (once launch rows exist) |
| Stale DAT identity | existing generic machinery |

## 22. GUI (novice-facing summary)

Default: *`Valid ZX Spectrum tape — 12 blocks, 3 files, verified against TOSEC`*. Expanded detail only: block-type histogram, checksums-ok flags, internal names (provenance), format version, timing class (logical/structured/raw). **Do not turn the library view into an oscilloscope** — no pulse visualizations, no per-block dumps in normal views.

## 23. PARSER SAFETY LIMITS (concrete proposal)

| Limit | Value | Applies to |
|---|---|---|
| Max input bytes | 8 MiB | all tape parsers |
| Max blocks/chunks | 65,536 | TZX/CDT, PZX, UEF, CAS |
| Max single block/chunk | 8 MiB | TZX direct-recording; CAS data |
| Max metadata text | 4,096 B per block | TZX 0x30-0x32, PZX BRWS, UEF 0x0000-0x0005 |
| Max T64 entries | 64 declared / walk bounded by file length | T64 |
| TZX loop nesting / jumps followed | **0 followed** (count only) | TZX |
| Max pulse count | 16,777,216 declared-pulses accounted, never iterated per-pulse above cap | TZX 0x10-0x19, PZX PULS |
| Max CSW expanded bytes | 64 MiB, streamed, never materialized | CSW, TZX CSW blocks |
| Max gzip (UEF/ADZ) output | ≤ 16 MiB, streamed | UEF, ADZ |
| Max RLE expansion | bounded per-record (255×) with global cap | CSW, DMS-class |
| Work factor | single linear pass; no per-pulse loops beyond cap | all |

All reuse the existing bounded-reader conventions (`disk_format/mod.rs` caps, `safe_read`, checked arithmetic, refuse-don't-truncate).

## 24. REAL-CORPUS COVERAGE

`coverage_inventory.rs` contains **zero tape rows** — every tape format is **NoCoverage**, and no tape fixtures exist anywhere (all statuses above are structural absence, not validation debt). Any future parser starts at SyntheticValidated honestly, like every other family did.

## 25. ORPHANED CODE

**None.** There is no tape parser, helper, detector, or evidence function anywhere in the tree to orphan — the tape layer is genuinely absent, not miswired. (Contrast: most families in this audit series had mature-but-unwired parsers; tape has neither.)

## 26. DO NOT REBUILD

- **Nothing tape-specific is mature** — the only reusable machinery is *generic*: the bounded-reader discipline (`disk_format/mod.rs` caps/`BoundedReader`), `safe_read`, the member-evidence layer, the `ContentKind::TapeImage` + `"tape_image"` persistence (`database.rs:1504`), the shared-extension denylist, and the folder-alias precedence tests. Build tape parsers **inside** those conventions; do not build a parallel tape framework.

## 27. MATURITY MATRIX

| | C64 | VIC-20 | Spectrum | CPC | Acorn | MSX | Atari8 | Other |
|---|---|---|---|---|---|---|---|---|
| TAP | REGISTERED-ONLY (weak; no Commodore parser) | REGISTERED-ONLY | REGISTERED-ONLY (weak; no ZX-block parser) | N/A | N/A | N/A | N/A | N/A |
| T64 | REGISTERED-ONLY (strong claim, no parser/registration) | N/A | N/A | N/A | N/A | N/A | N/A | N/A |
| TZX | N/A | N/A | REGISTERED-ONLY (strong claim, dangerous format, no parser) | N/A | N/A | N/A | N/A | N/A |
| CDT | N/A | N/A | N/A | REGISTERED-ONLY (strong claim, no parser) | N/A | N/A | N/A | N/A |
| PZX | MISSING (unclaimed) | — | MISSING | — | — | — | — | — |
| CSW | MISSING (unclaimed) | — | MISSING | — | — | — | — | — |
| UEF | N/A | N/A | N/A | N/A | **UNSUPPORTED** (claimed weak ×2, no parser/registration) | — | — | — |
| CAS | UNSUPPORTED (vestigial claim) | UNSUPPORTED (vestigial) | N/A | N/A | N/A | UNSUPPORTED (no parser; 4-way collision) | UNSUPPORTED (no parser; FUJI spec ready) | — |
| WAV/audio | MISSING (unclaimed; deliberately out of scope) | — | — | REGISTERED-ONLY (`.voc` weak) | — | — | — | — |

Every non-MATURE cell is the same root cause: **no tape parser exists at any layer**; the only maturity is registry-level recognition for three extensions.

## 28. BROKEN JOINS (top 16)

1. **CDT/TZX live false positive — P0.** `platform/detect.rs::signature_evidence` scans every platform's magic irrespective of extension; the `ZXTape!\x1a` `MagicRule` lives only on the ZX Spectrum row, so a valid Amstrad `.cdt` gets ZX Spectrum `Signature` evidence, the CPC row contributes no equivalent candidate, and the file is reported **Probable ZX Spectrum** (see §2a). Both ends exist — the magic table and the CPC registry row — they are just not joined. Fixed by `cpc-zxtape-signature-parity` (§31). **This is the one confirmed live tape-platform misidentification in the audit.**
2. `.tzx` is a **strong** ZX Spectrum extension with zero parser and a dangerous-format design waiting on caps — the highest-stakes registration in the family.
3. `.cdt` strong CPC + `.tzx` strong ZX → **one shared TZX parser** would light up both; no engine exists.
4. `.tap` four-way collision → **C64-TAPE-RAW magic parser** is the decisive discriminator and is unbuilt.
5. `.cas` four-way collision → **`FUJI` magic parser** unbuilt (flagged by the Atari review; still open).
6. `.t64` strong C64 claim → not even content-registered (inspector-only).
7. `.uef` claimed by two platforms → unregistered, unparsed, no coverage row.
8. `.voc` CPC claim → unregistered, unparsed, no policy.
9. `ContentKind::TapeImage` + `"tape_image"` persistence exist → **no parser ever emits tape facts**; the kind is an empty envelope.
10. `database.rs` discovery treats `ComputerDisk|TapeImage` uniformly → no tape-specific validation hook exists.
11. Archive-member layer classifies `.tap`/`.t64` as likely content → **no member-level tape evidence**.
12. Folder-alias platform resolution for `.tap`/`.cas` conflicts → no structural second leg (the exact gap every other family closed with a parser).
13. `coverage_inventory` has zero tape rows → the ledger cannot see the family at all.
14. No `IdentityPlatform` tape eligibility (GB/N64-style loose-ROM path) — even a perfect parser would need identity variants first (Atari/C64-class gaps, already documented).
15. `.pzx`/`.csw` are unparsed **and unclaimed** — no platform even vouches for preservation formats.
16. GUI shows `tape_image` content kind → **zero tape facts behind it** (no block counts, no names).

## 29. HARDEST FORMATS (ranked, honestly)

1. **TZX/CDT** — a block *program* (loops/jumps/calls/select) plus embedded CSW and megabyte direct-recording blocks; the only format where a naive parser executes attacker control flow. Safe only with the §6 caps and zero control-flow following.
2. **CSW** — expansion-ratio bombs; safe only as container-only or strictly streamed with a global expanded cap.
3. **WAV/VOC (if ever)** — DSP normalization is inherently heuristic; correctly out of scope permanently.
4. **Commodore TAP v2** — overflow-pulse cascades are version-specific; container validation is easy, but any future pulse-level feature must pin version semantics precisely.
5. **UEF (gzip-wrapped)** — decompression-bound + chunk zoo; safe only with the output cap and chunk-length accounting.

(No conclusion was forced: TZX/CSW earn their reputation on structural grounds — control flow and expansion — not reputation.)

## 30. EASIEST HIGH-VALUE FORMATS (ranked)

1. **Commodore TAP** — fixed ASCII magic, trivial bounded header; *existing-parser-class wiring is N/A, but it is a "simple new structural parser"* and the shared-`.tap` discriminator.
2. **Atari CAS** — same shape (FUJI magic + chunk walk); resolves the long-standing four-way `.cas` collision.
3. **T64** — fixed signature + bounded directory table; strong C64 claim becomes real.
4. **UEF** — gzip + uniform chunk framing; two platforms gain their only tape format.
5. **TZX/CDT** — highest value (two strong claims), but **complex**: build last, behind the caps.

## 31. BEST IMPLEMENTATION TASKS (13)

**P0 — broken joins / smallest unblocks**

| # | Slug | Format/Platform | Objective | Files | Reused | Spec sources | Collision policy | Non-goals | Bounds | Tests | Benefit | Dep | Size |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `cpc-zxtape-signature-parity` | CDT+TZX / Amstrad CPC + ZX Spectrum | Give the Amstrad CPC registry row the **same** `Corroborated` `ZXTape!\x1a` `MagicRule` the ZX Spectrum row owns, so a `.cdt`/`.tzx` beginning `ZXTape!` yields **honest CPC⇔Spectrum ambiguity** instead of *Probable ZX Spectrum* when no stronger evidence exists | `platform/mod.rs` (add `MagicRule` to the Amstrad CPC `Platform`), `platform/detect.rs` tests, `platform/tests.rs`, `platform_evidence_fusion` collision tests | existing `MagicRule` / `MagicConfidence::Corroborated`, `conflicts_with`, `MAX_MAGIC_READ_BYTES` (64 ≥ 8-byte sig) | TZX 1.20 spec (CDT == TZX container) + Fuse `tzx.c` / a CPC tool | container is shared; **never resolve CDT vs TZX by extension alone**; `ZXTape!` stays **Corroborated, never Strong** | no TZX parser here; no new platform split; no Strong upgrade; no extension-gated magic | single static table entry; no I/O change | `.cdt`/`.tzx` with `ZXTape!` + no folder → `Ambiguous {Amstrad CPC, ZX Spectrum}`; `.tzx` in `spectrum/` → ZX Spectrum; `.cdt` in `amstrad-cpc/` → Amstrad CPC | removes the one confirmed live tape misidentification (§2a, §28.1) | none | **Tiny** |
| 2 | `tape-zx-block-tap` | ZX TAP / ZX Spectrum | Validate block chains (length/flag/checksum to EOF) as Corroborated family evidence for shared `.tap` | new `tape/zx_tap.rs` (or `content_evidence` sibling), discovery route | bounded-reader caps, `ContentKind::TapeImage` | ZX ROM loading docs + one reference parser | ZX vs Commodore decided by C64 magic first; neither → ambiguous | no BASIC interpretation; no rename | §23 all | chain fixtures, truncated, bad checksum, C64-magic disambiguation | shared-`.tap` folderless disambiguation improves | 4 | **Medium** |
| 3 | `tape-commodore-tap` | Commodore TAP / C64·VIC-20·C16 | `C64-TAPE-RAW` header + version/machine byte → Strong family evidence | new `tape/commodore_tap.rs` | same | VICE `tape.c` + TAP 2.0 spec | machine byte narrows within family only | no pulse decoding | §23 | version/machine/overflow fixtures | discriminator for the 4-way `.tap` collision | 4 | **Small** |
| 4 | `tape-atari-cas` | Atari CAS / Atari 8-bit | `FUJI` header + chunk walk → Strong family evidence; closes the `.cas` collision's Atari leg | new `tape/atari_cas.rs` | same | Atari8 CAS spec + one tool | FUJI ⇒ Atari; else MSX check; else ambiguous | no FSK decode | §23 | chunk/baud/truncated fixtures | Atari leg of `.cas` collision | 4 | **Small** |
| 5 | `tape-msx-cas` | MSX CAS / MSX | structural block-header validation (0x1F-type chains, XOR checksums) after pilot heuristics | new `tape/msx_cas.rs` | same | MSX tape spec + openMSX | never platform-proof alone; family candidate | no BASIC/binary interpretation | §23 | block fixtures, Atari-vs-MSX cross-refusal | MSX leg of `.cas` | 4 | **Medium** |

**P1 — core tape coverage**

| # | Slug | Format/Platform | Objective | Files | Reused | Spec sources | Collision policy | Non-goals | Bounds | Tests | Benefit | Dep | Size |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 6 | `tape-t64-container` | T64 / C64 | Bounded directory walk + member provenance facts (names/addresses, no extraction) | new `tape/t64.rs` | member-evidence precedent | T64 docs ×2 | C64-only claim | no Archive kind; no extraction | §23 | entry/overlap/free fixtures | strong C64 claim becomes real; members visible | 4 | **Medium** |
| 7 | `tape-tzx-shared-parser` | TZX+CDT / ZX+CPC | Linear block-framing parser with §6 caps; block-type inventory; **zero control-flow following** | new `tape/tzx.rs` + dispatch | bounded-reader caps, member detectors, `cpc-zxtape-signature-parity` collision output | TZX 1.20-1.21 spec + Fuse `tzx.c` | container shared; platform stays folder/DAT; extension never resolves CDT vs TZX | no timing interpretation; no loop/jump following; no CSW expansion (defer) | §6 caps verbatim | every block family + hostile loop/jump + truncated | lights two strong claims; the dangerous format done safely | 1, 3 | **Large** |
| 8 | `tape-uef-new-parser` | UEF / BBC+Electron | gzip-bounded chunk walk; tape + provenance chunks; family-level evidence | new `tape/uef.rs` | ADZ-style gzip caps, chunk discipline | UEF spec (EUG) + one tool | family-level only (rows already say so) | no DFS/tape decoding | gzip cap §23 | chunk fixtures, gzip-bomb refuse | Acorn family gains its tape format | none | **Medium** |
| 9 | `tape-pzx-parser` | PZX / (ZX-class) | Flat chunk framing (PULS/DATA/PAUS/BRWS/STOP) | new `tape/pzx.rs` | TZX parser conventions | PZX spec + reference | same family policy as TZX | no pulse interpretation | §23 | chunk fixtures | safest structured-timing format | 7 | **Small** |

**P2 — preservation / completion**

| # | Slug | Format/Platform | Objective | Files | Reused | Spec sources | Collision policy | Non-goals | Bounds | Tests | Benefit | Dep | Size |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 10 | `tape-csw-container` | CSW / multi | Container-only validation (magic/version/rate/compression); expansion refused or streamed-capped | new `tape/csw.rs` | CSW caps §23 | CSW spec + one tool | multi-claimant; family-level | no pulse decoding; no materialization | expanded-cap §23 | v1/v2 + bomb fixtures | preservation container recognised | 7 | **Small** |
| 11 | `tape-member-evidence` | all / ZIP+7z+RAR+LHA | Tape detectors in `archive_member_content_evidence` member set | `archive_member_content_evidence.rs` | member layer, prefix reads | same as each parser | member-level = provenance | no extraction | 64 KiB prefix + declared-size bounds | member fixtures | archive users see tape facts | 3-9 as each lands | **Small** |
| 12 | `tape-doctor-gui-summary` | all | Doctor findings + novice summary (blocks/files/class) per §21-22 | `diagnostics/`, GUI identity surfaces | finding taxonomy | — | INFO/WARNING/BLOCKING split | no oscilloscope GUI | — | finding tests | tape health visible | 1-9 | **Small** |
| 13 | `tape-coverage-and-policy-rows` | all | Coverage rows for tape formats; platform claims reconciled (`.cas` vestigial claims on C64/VIC-20 documented/dropped; `.pzx`/`.csw` policy: deliberately unclaimed) | `coverage_inventory.rs`, `platform/mod.rs` | row patterns | — | documented vestigial claims | no new platform splits | — | inventory tests | honest ledger + claim hygiene | 3-9 | **Tiny** |

## 32. FINAL QUESTIONS

**One confirmed live tape-platform misidentification found by this audit:** a valid Amstrad `.cdt` (TZX-compatible, begins `ZXTape!\x1a`) is today reported as **Probable ZX Spectrum**. `platform/detect.rs::signature_evidence` scans every platform's magic regardless of extension; the `ZXTape!` `MagicRule` exists only on the ZX Spectrum row (`Corroborated`), the Amstrad CPC row has no equivalent, so the only `Signature`-tier evidence points at ZX Spectrum and — absent folder or DAT evidence — a specific wrong platform is returned rather than "ambiguous". Everything else in this audit is absent support or theoretical shared-extension ambiguity, not a live wrong answer. Fix: `cpc-zxtape-signature-parity` (§31, P0) — see §2a and §28.1.

1. **What does EmuWiz really support today?** Registry-level recognition only: `.tap`/`.tzx`/`.cdt` are discovered as `TapeImage` media with folder-based platforms; `.t64` is only a likely-content extension (`inspector.rs`), not media-registered; `.cas`/`.uef`/`.voc`/`.pzx`/`.csw` are not registered anywhere; **no production tape parser, no UEF implementation, and no tape evidence/identity/DAT-specific/launch code exist at any layer**. Generic whole-file DAT hashing already works for any tape file once it is discovered as media.
2. **Which extensions make unsafe or misleading claims?** `.cdt` — **the one live false positive**: a valid CDT is reported *Probable ZX Spectrum* because CPC lacks the `ZXTape!` signature parity the ZX Spectrum row has (§2a). Then: `.tzx` (strong ZX claim over the family's most dangerous format, unparsed), `.cdt` also a strong CPC claim with no parser, `.t64` (strong C64 claim, not even registered), `.cas` on **C64/VIC-20** (vestigial — no real CAS convention exists for them; the two real formats are Atari and MSX), and `.tap` (four claimants, two incompatible formats, zero discrimination).
3. **Minimum format set for trustworthy tape support?** Commodore TAP + ZX TAP (the shared-`.tap` discriminator pair), Atari CAS + MSX CAS (the `.cas` pair), T64, and the shared TZX/CDT parser — six pieces cover every claimant honestly.
4. **Which can safely prove a platform?** Commodore TAP (family: C64/VIC-20/C16 via machine byte) and Atari CAS (family: Atari 8-bit) — signature-verified containers. T64 (C64) is close behind. TZX/CDT/UEF/PZX prove **structure**, staying family/folder-corroborated. Nothing tape proves a *release*.
5. **Which stay family-level only?** ZX TAP (no signature), TZX/CDT, PZX, UEF, CSW, MSX CAS.
6. **Which remain DAT/hash-only?** All of them for release identity; additionally CSW/WAV/VOC and Commodore TAP are hash-only even for *platform* purposes beyond container validation.
7. **Which should NOT be implemented before release?** TZX/CDT (unless the §6 caps are implemented verbatim and reviewed), CSW expansion (container-only at most), WAV/VOC decoding (never in core), PZX (after TZX).
8. **How should turbo/custom loaders be handled?** Preserve the structure, inventory the block types, never interpret timing or semantics; refusal only for framing violations. The crate proves preservation structure, not loader behavior.
9. **Should WAV ever be decoded by EmuWiz?** No — not in core, not heuristically. If ever needed: an external conversion workflow producing TZX/PZX, which EmuWiz parses structurally.
10. **Five highest-value pre-release tape tasks?** #1 `cpc-zxtape-signature-parity` (Tiny — kills the one live misidentification), #3 Commodore TAP, #4 Atari CAS, #6 T64, then #2 ZX TAP — resolve every unsafe claim and light up the family's biggest platforms.
11. **Which are the real bane, and why?** TZX/CDT (attacker-controlled control flow + embedded expansion) and CSW (expansion bombs) — dangerous on *resource and control-flow* grounds, not reputation; everything else is ordinary bounded parsing.
12. **What parser-safety architecture should every future tape parser share?** The §15 taxonomy as the fact-vocabulary, the §23 caps as hard constants in the crate's bounded-reader style, linear framing-only passes with zero control-flow following, streaming-with-caps for any expansion, and the crate's two-source verification bar before any magic table is committed.
