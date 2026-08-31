# EmuWiz Atari Family — Second-Pass Deep Audit

> **Research snapshot** — This audit records repository findings at the time it was written. It is not current capability documentation; see the [README](../../README.md), [adapter support matrix](../ADAPTER_SUPPORT_MATRIX.md), and [roadmap](../../ROADMAP.md) for present guidance.

**Repository:** `/home/davedap/archivefs`
**Branch:** `feature/archivefs-unified-platform`
**Scope:** Atari 2600, 5200, 7800, 8-bit (400/800/XL/XE), Lynx, Jaguar, Jaguar CD, ST/STE/TT/Falcon
**Method:** independent re-audit of every Atari-associated registry, parser, detector, launch adapter, and mapping table. Read-only; no source modified, no cargo run.

---

## 0. How This Audit Was Performed

Every claim below was verified by reading the actual source in `crates/archivefs-core/src`. The following files were inspected in full or at every Atari-relevant site:

- `platform/mod.rs` (PLATFORMS, EQUIVALENT_PLATFORM_IDS, SHARED_EXTENSIONS)
- `ingestion/content_registry.rs` (CONTENT_FORMATS)
- `ingestion/discovery.rs` (discover_direct_file / discover_cue / discover_archive)
- `media_registry.rs` (MEDIA_FORMATS)
- `inspector.rs` (LIKELY_CONTENT_EXTENSIONS)
- `disk_format/mod.rs` (inspect_disk_format dispatch + all adapters)
- `disk_format/atari_st.rs`, `disk_format/atari_stx.rs`
- `atari7800_header_evidence.rs`, `lynx_header_evidence.rs`
- `header_normalization.rs`
- `archive_member_content_evidence.rs` (member_detectors)
- `content_evidence_scope.rs` (SCOPE_CATALOG)
- `platform_evidence_fusion.rs` (RULES)
- `game_identity.rs` (IdentityPlatform, inspect_game_identity_with_platform_trust, supported_loose_rom_format)
- `launch/platform_map.rs` (LAUNCH_COMPATIBILITY), `launch/es_de_export.rs` (ES_DE_SYSTEM_MAP), `launch/readiness.rs`, `launch/input_projection.rs`, `launch/planning.rs`, `launch/execution.rs`, `launch/retroarch_command.rs`
- `patch_manager/hatari_local.rs`, `patch_manager/mod.rs` (module list)
- `platform_evidence_fusion/romm_platform_mapping.rs`
- `identity_source/romm/normalise.rs` (ROMM_SLUG_ALIASES)
- `coverage_inventory.rs` (COVERAGE)
- `diagnostics/profiles.rs` (Doctor adapter inventory)

---

## 1. PLATFORM MODEL — EXACT ROWS AND GAPS

### 1.1 Registry rows that exist

| Canonical ID | Display | Aliases (folders) | Strong exts | Weak exts | Magic | Conflicts | IdentityPlatform | Coverage | LAUNCH_COMPAT | ES-DE | RomM outbound |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `Atari2600` | Atari 2600 | atari2600, a2600, atarivcs | a26 | bin, rom, zip | — | Atari 8-bit | **missing** | **missing** | **missing** | **missing** | **missing** |
| `Atari5200` | Atari 5200 | atari5200, a5200 | a52 | bin, rom, car, zip | — | Atari 8-bit | **missing** | **missing** | **missing** | **missing** | **missing** |
| `Atari7800` | Atari 7800 | atari7800, a7800 | a78 | bin, rom, zip | `ATARI7800` @0x01 (Strong) | — | **missing** | SyntheticValidated | **missing** | **missing** | **missing** |
| `Atari 8-bit` | Atari 8-bit | atari8bit, atari800, atari8, atarixl, atarixe, atari400, atari130xe, atarixegs | atr, atx, xex, xfd | cas, bin, rom, car | — | Atari5200, AtariST | **missing** | **missing** | **missing** | **missing** | **missing** |
| `Atari Jaguar` | Atari Jaguar | atarijaguar, jaguar, jaguar64, atarijag | j64, jag | rom, bin, abs, cof | — | — | **missing** | Deferred | **missing** | **missing** | **missing** |
| `Atari Lynx` | Atari Lynx | atarilynx, lynx, atarilynxlynx, lynxii | lnx, lyx | bin, o | `LYNX` @0x00 (Strong) | — | **missing** | RealValidated (Joust.lnx) | **missing** | **missing** | **missing** |
| `AtariST` | Atari ST | atarist, atariste, atarifalcon, atarittu | st, stx, msa, mfm | dsk, ipf, zip | structural (st/stx) | Amstrad CPC, BBC Micro | **missing** | **missing** | **hatari** (+ hatari core hint) | **atarist** | inbound `atari-st` only |

### 1.2 Registry rows that do NOT exist

| Machine | Notes |
|---|---|
| **Atari Jaguar CD** | Entirely absent from PLATFORMS. `Atari Jaguar`'s own explanation says: *"Jaguar Jaguar CD titles are disc images; this build has no canonical platform for them, so they are not claimed here."* |
| **Atari STE** | Folded under `AtariST` via folder alias `atariste`. No separate row, no `equivalent_platform_ids` relation. |
| **Atari TT** | Folded under `AtariST` via `atarittu`. No separate row. |
| **Atari Falcon** | Folded under `AtariST` via `atarifalcon`. No separate row. |
| Atari 800 (computer) | Subsumed by `Atari 8-bit` alias `atari800`. |
| Atari XEGS | Subsumed by `Atari 8-bit` alias `atarixegs`. |

### 1.3 Naming drift

- `Atari2600` / `Atari5200` / `Atari7800` / `AtariST` (no space) vs `Atari 8-bit` / `Atari Jaguar` / `Atari Lynx` (space). `canonical_layout_folder` derives `Atari 2600` from the `Atari2600` id via the display name — storage is stable, but the id-vs-display inconsistency is a permanent hazard for any new table.
- STE/TT/Falcon have **no** canonical identity, so a library that stores "Atari Falcon" as a folder cannot round-trip through `platform_for_alias` unless the user keeps it inside an `atarifalcon` folder.

### 1.4 Platforms the registry knows but nothing downstream does

| Registry row | identity | ingestion | launch | ES-DE | RomM | Doctor | GUI display |
|---|---|---|---|---|---|---|---|
| Atari2600 | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ (display name) |
| Atari5200 | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |
| Atari7800 | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |
| Atari 8-bit | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |
| Atari Jaguar | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |
| Atari Lynx | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |
| AtariST | ✗ | partial (.st/.msa/.ipf) | partial (hatari row, no execution) | ✓ | inbound only | ✗ | ✓ |

---

## 2. MEDIA / FORMAT INVENTORY

Legend: **REGISTERED** (in a registry table) · **PARSED** (structural parser exists) · **PRODUCTION-WIRED** (parser reachable from discovery or identity) · **IDENTITY-WIRED** (feeds verified identity) · **HASH/DAT-ONLY** · **DEFERRED** · **UNSUPPORTED**

| Ext | Platform | CONTENT_FORMATS | media_registry | inspector | disk_format parser | game_identity | Status |
|---|---|---|---|---|---|---|---|
| .a26 | 2600 | **absent** | absent | likely | none | none | REGISTERED-ONLY (platform) → UNSUPPORTED in scanner |
| .bin | 2600/5200/8-bit/ST/… | absent (by design) | absent | likely | none | none | SHARED — no intrinsic identity |
| .rom | 2600/5200/8-bit/Jag | absent | absent | likely | none | none | SHARED — no intrinsic identity |
| .a52 | 5200 | **absent** | absent | likely | none | none | REGISTERED-ONLY → UNSUPPORTED |
| .a78 | 7800 | **absent** | absent | likely | none (header parser exists outside disk_format) | none | PARSED (atari7800_header_evidence) but ORPHANED from identity; UNSUPPORTED in scanner |
| .atr | 8-bit | **absent** | absent | absent | **none** | none | REGISTERED-ONLY → UNSUPPORTED |
| .xfd | 8-bit | **absent** | absent | absent | **none** | none | REGISTERED-ONLY → UNSUPPORTED |
| .atx | 8-bit | **absent** | absent | absent | **none** | none | REGISTERED-ONLY → UNSUPPORTED |
| .cas | 8-bit | absent | absent | absent | **none** | none | REGISTERED-ONLY (weak) → UNSUPPORTED |
| .car | 8-bit/5200 | absent | absent | absent | **none** | none | UNSUPPORTED |
| .xex | 8-bit | **absent** | absent | absent | **none** | none | REGISTERED-ONLY → UNSUPPORTED |
| .st | ST | ComputerDisk | absent | likely | **AtariStRawFloppy** | none | PARSED + PRODUCTION-WIRED (hatari_local + platform detect) but NOT identity-wired |
| .msa | ST | ComputerDisk | absent | absent | **none** | none | REGISTERED-ONLY → UNSUPPORTED structurally |
| .stx | ST | **absent** | absent | absent | **AtariStPasti** | none | PARSED + PRODUCTION-WIRED (hatari_local + platform detect) but NOT identity-wired, NOT in CONTENT_FORMATS |
| .ipf | ST | ComputerDisk | absent | absent | **none** | none | REGISTERED-ONLY (extension only) |
| .dim | ST | absent | absent | absent | **none** | none | UNSUPPORTED |
| .hdf | ST/Amiga | absent (Amiga collision documented) | absent | absent | none (hdi≠hdf) | none | UNSUPPORTED (deliberately, for Amiga safety) |
| .img | ST/… | ComputerDisk (generic) | absent | likely | none | none | REGISTERED-ONLY (generic) |
| .vhd | ST | absent | absent | absent | **none** | none | UNSUPPORTED |
| .lnx | Lynx | **absent** | absent | likely | none (header parser exists) | none | PARSED (lynx_header_evidence) but ORPHANED; UNSUPPORTED in scanner |
| .lyx | Lynx | **absent** | absent | absent | **none** | none | REGISTERED-ONLY → UNSUPPORTED |
| .j64 | Jaguar | **absent** | absent | likely | none | none | REGISTERED-ONLY → UNSUPPORTED |
| .jag | Jaguar | **absent** | absent | likely | none | none | REGISTERED-ONLY → UNSUPPORTED |
| .cue/.bin/.iso/.chd | Jaguar CD | DiscImage (iso/chd/gdi/cdi) | iso/chd | likely | optical stack | none for JagCD | no platform row; optical stack reuse possible but unclaimed |

**Headline:** of the ~20 Atari extensions, only `.st` and `.stx` have structural parsers; only `.st`, `.msa`, `.ipf` are in `CONTENT_FORMATS`; **none** reach `game_identity`.

---

## 3. ATARI 2600

- **No intrinsic header exists.** 2600 cartridges are raw ROM images. EmuWiz correctly does not claim a magic rule for `Atari2600` in `PLATFORMS`.
- **Mapper/bankswitch schemes** (F8, F6, FE, Superchip RIOT mirroring, DPC/DPC+, CDF/ARM, etc.) are **not** modeled anywhere. There is no `atari2600_header_evidence.rs` and no mapper heuristic. This is appropriate: mapper identification requires a game-name/DAT lookup (the Stella approach), never a header field.
- **Risk assessment — `.bin` → Atari2600:** The platform registry lists `.bin` as *weak* evidence for Atari2600 (shared with 5200, 8-bit, ST, Jaguar, NES, SNES, GB, MD, …). `detect_platform_report` never promotes a lone shared extension to Confirmed; it returns `Ambiguous`/`Unknown`. The scanner (`discover_direct_file`) treats `.bin` as a missing-paired-file candidate only when a `.cue` exists; a bare `.bin` is `MissingPairedFile`. **No unsafe `.bin`→2600 promotion exists.**
- **Real status:** HASH/DAT-ONLY. The only production identity is a SHA-256 loose-ROM hash, and even that requires an `IdentityPlatform` the codebase does not yet provide.

---

## 4. ATARI 5200

- `.a52` is strong-only; `.bin`/`.rom`/`.car` are weak. No structural parser distinguishes 5200 from Atari 8-bit cartridges.
- **5200 vs 8-bit:** both are 6502-family machines sharing the POKEY/ANTIC chipset and cartridge address space. A raw `.bin` cannot be separated by bytes alone; the 5200's `CART` header (magic `0x43415254` at 0x00) is the only structural distinguisher and is **not implemented**.
- **Emulator note:** RetroArch's `a5200` core requires `.a52`/`.bin`/`.car`; the `atari800` core runs 8-bit software. EmuWiz has no core-hint row for either.
- **Real status:** REGISTERED-ONLY; HASH/DAT-ONLY when a platform is externally known.

---

## 5. ATARI 7800 — THE MOST-WIRED-YET-ORPHANED SYSTEM

### 5.1 What `atari7800_header_evidence.rs` parses

Verified against the 8BitDev.org A78 header specification (the Concerto/cc7800 reference):

| Field | Offset | Meaning |
|---|---|---|
| header_version | 0x00 | 1..3 handled; 4+ extension fields out of scope |
| magic | 0x01 | `ATARI7800` (9 bytes) |
| cart_title | 0x11 (32B) | ASCII title; emitted as **Corroborated ProductCode** |
| rom_size | 0x31 (BE u32) | payload size excluding the 128-byte header |
| cart_type | 0x35 (BE u16) | raw bitfield; POKEY@$4000 (bit0), SuperGame bank-switch (bit1) decoded; other bits preserved raw |
| controller1/2 | 0x37/0x38 | 0=None, 1=Joystick, 2=Light Gun, … |
| tv_type | 0x39 | bit0: NTSC=0, PAL=1 |
| save_device | 0x3A | v2+ only; not surfaced |

No checksum field is present in the header (the format has none). Reserved/extension fields are deliberately not decoded.

### 5.2 Evidence produced

`observe_a78_evidence` → `BootStructure = "ATARI7800"` (Strong) + `ProductCode = cart_title` (Corroborated). `content_evidence_scope` classifies `ATARI7800` as **PlatformSpecific("Atari7800")**; `platform_evidence_fusion::RULES` has `atari7800_header` (Strong → Atari7800).

### 5.3 Wiring trace (each arrow is a gap)

```
atari7800_header_evidence (parser + tests)
   ├── archive_member_content_evidence::member_detectors()  ✓ (ZIP members only)
   ├── header_normalization::HeaderNormalizationKind::Atari7800_128 ✓ (reversible strip/restore)
   ├── platform_evidence_fusion::atari7800_header ✓ (only if evidence reaches fusion)
   ├── discovery (loose .a78)  ✗  (not in CONTENT_FORMATS)
   ├── game_identity            ✗  (no IdentityPlatform::Atari7800; supported_loose_rom_format lacks a78)
   ├── DAT normalized hashing   ✗  (strip_known_header not called for hashing)
   └── launch / ES-DE / RomM    ✗
```

### 5.4 How tiny the wiring task is

- Add `cf("a78", ContentKind::RomCartridge)` to `CONTENT_FORMATS` → scanner visibility.
- Add `IdentityPlatform::Atari7800` + `from_catalogue("atari7800")` + label → identity dispatch.
- In `inspect_loose_rom`/`supported_loose_rom_format`, allow `.a78` and call `parse_a78_header` on a bounded prefix; promote `BootStructure` fact and optionally the normalized payload SHA-256 (via `strip_known_header`).
- Add `ES_DE_SYSTEM_MAP` row `atari7800` and a `LAUNCH_COMPATIBILITY` row with `prosystem` core hint.

This is a **Small** task (≈2 focused commits) that turns a fully-tested parser into production identity.

### 5.5 Headerless .a78 ROMs

Headerless 7800 dumps exist (raw 32K/48K/64K/128K images). Without the `ATARI7800` magic, no structural claim is made — correctly fail-closed. DAT/hash matching is the only path, and it works only when the platform is externally known.

---

## 6. ATARI 8-BIT DISK FORMATS

### 6.1 ATR

- **Spec (from public ATR documentation, e.g. the AtariMax/Ape and SIO2PC references):** 16-byte header: `0x00` magic `0x0296` (little-endian `96 02`); `0x02` paragraph count (LE u16, ×16 = bytes); `0x04` sector size (128 or 256, LE u16); `0x06` first image sector (LE u16, usually 1); `0x08` flags (write-protect bit); `0x0A`/`0x0C`/`0x0E` reserved (often "ATR" ASCII in some tools, not required).
- **Geometry rules:** sectors = paragraph_count×16 ÷ sector_size; boot sectors = 3 (sector 1..3); DOS 2.x uses 128-byte sectors, SpartaDOS/MyDOS use 256. A valid ATR's header math must account for the file length exactly.
- **Current state:** **no parser.** `.atr` is registered as a strong extension only.
- **Bounded structural validation is straightforward:** read 16-byte header, verify magic, verify sector size ∈ {128,256}, verify `paragraph_count×16` equals file length (or file length minus an optional 16-byte "long" header in some tools — must be reviewed before accepting), cap total size at a few MB. This would give **Corroborated** evidence (header is simple; DOS type needs sector-360 scan, which should stay out of scope).

### 6.2 XFD

- **Spec:** raw, headerless sector dump; single density = 128-byte sectors (normally 720/1040 sectors), double density = 256-byte sectors. No magic.
- **Safe structural corroboration without size-only identity:** check `len % sector_size == 0` for both 128 and 256 and require the sector count to be a plausible 8-bit disk (e.g. 720, 1040, 1440, 2080); optionally validate that the FAT/boot area (sector 360) contains plausible Atari DOS markers — but that crosses into filesystem parsing. The honest recommendation: **XFD should remain weak evidence requiring folder/DAT corroboration**, matching how `.st` is handled today.

### 6.3 ATX

- **Spec:** VAPI/ATX is a copy-protected, per-sector flux/weak-bit container ("ATX" = "Atari XE/XL preserved image"). Header is a 6-byte magic `ATX\x1a\x01\x00`; a 128-byte main header + optional per-track/per-sector headers with timing and data CRC records.
- **Current state:** **no parser, no partial implementation** anywhere in the tree.
- **Practical bounded validation:** the magic and a fixed main-header length can be validated cheaply; full structural validation (track table walk with CRC-bounded reads) is feasible but requires the VAPI spec review against at least one independent implementation (VAPI tools / Altirra). Recommend **deferring ATX parsing**; treat as hash-only for now.

---

## 7. ATARI 8-BIT TAPE / CARTRIDGE / EXECUTABLE

### 7.1 CAS

- **Spec:** optional 8-byte `FUJI` magic + version (`FUJI` + `\x00\x00\x00\x00\x00\x00` or `FUJI` + `\x01\x01\x00\x00\x00\x00`); then a stream of chunk records: 4-byte little-endian length + 2-byte chunk type (`0x02` = baud rate, `0x03` = data). No checksum at the chunk layer (the tape data itself has Atari cassette framing).
- **Collision:** `.cas` is shared with **MSX** (also `.cas`!). EmuWiz lists `.cas` as weak for Atari 8-bit only; an MSX platform row exists but does not claim `.cas`. A structural parser recognizing the `FUJI` magic would be **strong** Atari-8-bit evidence and would safely disambiguate from MSX. Currently **no parser**.

### 7.2 CAR

- **Spec:** 16-byte header: 4-byte magic `CART` (0x43415254), 2-byte cart type (little-endian; 0x0000 = normal 8K, 0x0001 = banked, 0x0010 = 5200 32K, etc.), 2-byte extra RAM (banks), 8 bytes reserved (usually zeros). Type bytes follow documented tables in the Altirra/Atari800 cartridge docs.
- **Current state:** **no parser.** A `CAR` header parser would be strong, self-identifying structural evidence for both Atari 8-bit and 5200 (type field distinguishes). **This is the single highest-value missing Atari cartridge parser.**

### 7.3 XEX

- **Spec:** Atari binary load format: series of segments, each starting with `0xFF 0xFF` then LE u16 start, LE u16 end (inclusive), then payload; optional init vector (`0x02 0xE0` + LE u16) and run vector (`0xE0 0x02` + LE u16). Platform-specific (the 8-bit OS loader), but segment overlap/bounds checking is required for safety.
- **Current state:** **no parser.** `.xex` is a strong-extension claim only. A bounded segment-walk parser (cap segments, cap file size) would give **Strong** structural evidence, since the `FF FF` segment framing is distinctive to the Atari 8-bit loader.

---

## 8. ATARI ST RAW DISK — SAFETY FINDINGS (direct answer)

### 8.1 What `atari_st.rs` actually proves

- File is ≥512 bytes, ≤4 MiB, multiple of 512.
- Sector 0's BPB is coherent FAT12: 512 B/sector, power-of-two clusters 1..=8, 1–2 FATs, 16–1024 root entries, 1–2 sides, 8–12 sectors/track, 74–86 tracks, total sectors matches file length exactly, metadata area fits.
- **`proves_platform() == false`** — the module states in code and doc comments that a PC DOS 720 KB floppy satisfies every check. No OEM-string check, no Atari-specific media byte check, no boot-loader signature check is performed (and none is reliably available: TOS boot sectors vary and DOS 720K images share the geometry).

### 8.2 Direct answer: "Can a bare structurally-valid .st file be safely called Atari ST?"

**No — and the code already says so.** The platform detector returns `Probable` (not Confirmed) for `.st` unless the containing folder alias (`atarist`, `atariste`, `atarifalcon`, `atarittu`) agrees, which raises it to Confirmed only by *corroboration*, not by the bytes. The correct evidence level is **family/ambiguous**, and that is exactly what is implemented. **Do not weaken this.** The one caveat: `hatari_local.rs` runs `inspect_disk_format` on configured floppies with `DiskFormatContext::default()` (no folder), so a Hatari-configured `.st` reports Probable — which is honest, since Hatari's config already supplies the platform context downstream.

### 8.3 What a future improvement could add (research only)

Atari-specific boot-sector heuristics (e.g. the `TOS` boot flag / GEMDOS BPB quirks) are not standardized enough to promote `.st` to platform-proof; the current folder/DAT corroboration design is the right architecture.

---

## 9. STX / PASTI — PRODUCTION TRACE

### 9.1 Parser capabilities (`disk_format/atari_stx.rs`)

- Signature `RSY\0`; **version 3 only** (`SUPPORTED_VERSIONS = [3]`); revision 0..=2.
- 16-byte file header: sig, version, tool, reserved, track-record count (u8), revision.
- Per-track 16-byte records: record length (LE u32), fuzzy-mask length, sector count (LE u16), flags, MFM track length, track number + side bit, record type.
- Validation: record length ≥ 16; fuzzy length fits inside record; record chain stays inside file with checked arithmetic; declared count ≤ 168 (`MAX_PASTI_TRACK_RECORDS`); walk bounded by `MAX_DISK_FORMAT_OFFSET`/`MAX_DISK_FORMAT_BYTES_READ` (64 KiB).
- **Deliberately not read:** sector descriptors, fuzzy/weak-bit data, timing data, track data. The disk is never reconstructed.
- `proves_platform() == true` — Pasti is Atari-ST-only, so a valid container **does** settle the platform.

### 9.2 Production reachability trace

```
disk_format::atari_stx::inspect
   ├── platform::detect::structural_format_evidence ✓ (Probable→Confirmed w/ folder)
   ├── patch_manager::hatari_local::inspect_floppy_format ✓ (configured drives)
   ├── ingestion (loose .stx)  ✗  (not in CONTENT_FORMATS)
   └── game_identity            ✗  (no IdentityPlatform::AtariSt; no .stx dispatch)
```

**Verdict: NOT orphaned — it is production-wired for platform detection and Hatari config inspection, but missing identity dispatch and loose-file ingestion.** The parser is mature and must not be rewritten; the gaps are two registry/wiring entries.

---

## 10. MSA

### 10.1 Specification (cross-checked against two independent implementations: the MSA specification published with the original Magic Shadow Archiver tooling, and the Hatari/msa2st source conventions)

- **Header (10 bytes, big-endian):**
  - `0x00` u16 magic = `0x0E0F`
  - `0x02` u16 sectors per track (9/10/11 typical)
  - `0x04` u16 sides (0 = single-sided → 1 side; 1 = double-sided → 2 sides)
  - `0x06` u16 starting track (0-based, normally 0)
  - `0x08` u16 ending track (normally 79)
- **Track records:** one per track×side, in order (track 0 side 0, track 0 side 1, track 1 side 0, …). Each record starts with a u16 big-endian `track_len`.
  - Uncompressed: `track_len == sectors_per_track * 512` → raw sectors follow.
  - Compressed: `track_len < sectors_per_track * 512` → RLE data follows.
- **RLE semantics:** marker byte `0xE5`. On `0xE5`, read next byte `val` then u16 big-endian `count`; emit `val` × `count`. Any other byte is emitted as-is. (Both the original MSA spec and the Hatari decoder agree on this framing; the marker `0xE5` collides with a legal data byte only because the encoder guarantees a run never needs a literal `0xE5` — a literal is always encoded as a run.)
- **Decompression bounds:** a single track cannot expand beyond `sectors_per_track * 512` (a valid encoder never emits more); a full disk cannot exceed `tracks × sides × sectors_per_track × 512`, i.e. ≤ 2×80×11×512 = 901,120 bytes for a standard image.
- **Malformed runs:** a run whose `count` would overflow the per-track cap must be rejected; a run reading past `track_len` must be rejected; a track whose declared length is neither 0 (invalid) nor ≤ uncompressed size must be rejected; truncation mid-header or mid-record must fail closed.

### 10.2 Evidence strength

A valid MSA header + track table is **strong** evidence for "Atari ST-family disk" (the format exists only for ST/STE/TT/Falcon media; the `.dim` FM-TOWNS collision is different hardware and a different magic — `0x0E0F` is MSA-specific). Recommended confidence: **Corroborated/Strong for AtariST with folder; family-level without.**

### 10.3 Proposed explicit decompression caps

- `MAX_MSA_TRACK_BYTES = 11 * 512` (max uncompressed track)
- `MAX_MSA_TRACK_RECORDS = 2 * 85` (tracks × sides, mirroring `MAX_PASTI_TRACK_RECORDS`)
- `MAX_MSA_FILE_BYTES = 4 MiB` (mirror `MAX_RAW_FLOPPY_BYTES`)
- Never allocate from header claims; walk with a read budget like `BoundedReader`.

### 10.4 Current state

**No parser exists.** `.msa` is in `CONTENT_FORMATS` as `ComputerDisk` and registered as a strong extension, but `inspect_disk_format` has no `"msa"` adapter, and `hatari_local.rs` skips structural inspection for `.msa` (`inspect_floppy_format` only handles `St`/`Stx`). This is the **best new-parser candidate** in the whole Atari family (Small–Medium task).

---

## 11. IPF / SPS

- **No code exists** for IPF anywhere in the tree (only `.ipf` as a weak extension under `AtariST` and `ComputerDisk` in CONTENT_FORMATS).
- **Constraints:** IPF is a CAPS/SPS-preserved flux container; decoding requires the SPS library (license-restricted, not freely redistributable) or reimplementing the container from the (undocumented/obfuscated) CAPS format. **Structural inspection without CAPS/SPS is not practical.**
- **Recommendation:** keep IPF as a **preservation-container-only** extension; do not attempt a decoder. Hash-based identity (whole-file SHA-1 of the .ipf) is the only safe production path. Hatari can mount IPF directly, so launch support can pass the file through without EmuWiz understanding it — the same way `.zip` is passed to RetroArch today.

---

## 12. HARD DISK FORMATS (ST/TT/FALCON)

- `.hdf`: **no Atari parser.** The only `.hdf` handling in the codebase is the Amiga path (`discover_ambiguous_disk_image`), which explicitly refuses to call an `.hdf` an Amiga image without `inspect_amiga_image` succeeding and documents the Sharp X68000 collision. Atari ST hard-disk images (raw AHDI/ICD partitions, GEMDOS filesystems) are **not modeled**.
- `.vhd`: **no parser, no registration.**
- `.img` (ST hard disk): generic `ComputerDisk` registration only.
- **What would safely establish Atari hard-disk structure:** an AHDI/ICD partition table at sector 0 with the `AHI`/`ICD` marker, or a GEMDOS `BPB`-style boot sector with the Atari `TOS` filesystem flag — all requiring a dedicated, source-reviewed parser. **Recommendation: defer.** Raw HDD images are too collision-prone (DOS MBR, Amiga RDSK, X68000) to classify without a real partition-table parser, which is a new Medium feature, not a wiring fix.
- **Critical distinction:** Amiga `.hdf` handling must never be reused for Atari. The existing collision-safety logic is correct; keep it.

---

## 13. TOS FIRMWARE — EXACT TRACE

### 13.1 Discovery & verification

- TOS path comes from `hatari.cfg` `[ROM] szTosImageFileName` (via `HatariConfig.tos_path`).
- `inspect_tos()` computes **SHA-256 bounded to 2 MiB** (`HATARI_MAX_TOS_BYTES`) using `sha256_bounded` → `read_bounded`.
- Verification is against a **caller-supplied** `HatariTosReference { sha256, version, region }` list. **No known-hash table is embedded in the crate** (deliberately: the module doc says *"The adapter deliberately has no embedded TOS filename table: names have zero verification authority"* — and likewise no hash table, to avoid distributing copyrighted ROM hashes and to keep verification authority external).
- Result: `HatariTosHealth { NotConfigured, Missing, Unreadable, PresentUnverified, Verified }`.

### 13.2 Machine compatibility

`HatariMachineModel { St, Ste, Tt, Falcon, Unknown }` maps from `nModelType` (0=ST, 1=STE, 2=TT, 3=Falcon). No per-machine TOS-version matrix exists (that would require the external reference table to carry machine fields — it currently carries only `sha256`/`version`/`region`).

### 13.3 EmuTOS

No special handling. EmuTOS images would match a caller-supplied reference if one is provided; otherwise they surface as `PresentUnverified`. There is no embedded EmuTOS hash.

### 13.4 Chain trace

```
hatari.cfg szTosImageFileName
  → inspect_tos (SHA-256, 2 MiB bound)
  → HatariTosHealth
  → readiness::hatari_firmware_readiness (FirmwareReadiness::Verified/PresentUnverified/Missing/Unknown)
  → planning::build_launch_plan (blocker if Missing)
  → diagnostics (Doctor)  ✗  NO HATARI ADAPTER IN diagnostics/profiles.rs
  → GUI  (via readiness projection only)
  → launch execution  ✗  no hatari_command/execution
```

**Broken join:** TOS verification is fully implemented and tested but invisible to Doctor and unreachable by any launcher.

---

## 14. HATARI — ARCHITECTURE TRACE

### 14.1 What exists

| Layer | Location | Status |
|---|---|---|
| Profile discovery | `patch_manager/hatari_local.rs` (`discover_hatari_profiles`, native/Flatpak/portable/explicit/AppImage roots) | ✓ tested |
| Config parsing | `inspect_config` (Floppy A/B, HardDisk GEMDOS/ACSI/SCSI/IDE, ROM/TOS, Memory/save states, System/machine/CPU, Screen, MIDI) | ✓ tested |
| TOS health | `inspect_tos` (SHA-256, external refs) | ✓ tested |
| Game inspection | `inspect_hatari_game` (floppies, storage, identity association) | ✓ tested |
| Identity association | `associate_identity` / `is_atari_platform` (accepts "atari st/ste/stf/stfm/mega st/mega ste/tt/falcon") | ✓ tested |
| Firmware readiness | `launch/readiness.rs::hatari_firmware_readiness` | ✓ tested |
| Input projection | `launch/input_projection.rs::project_hatari_launch_input` → `HatariSelectedGameRequest` | ✓ tested |
| Launch compatibility row | `LAUNCH_COMPATIBILITY` → `AtariST`, `standalone_adapters: ["hatari"]`, core hint `hatari` | ✓ exists |
| **Command planning** | `launch/hatari_command.rs` | **MISSING** |
| **Execution adapter** | `launch/hatari_execution.rs` | **MISSING** |
| Doctor | `diagnostics/profiles.rs` | **MISSING** |

### 14.2 What a minimum safe command adapter would consume

- `HatariProfile` (`config_path`, `executable_candidates`) from `discover_hatari_profiles`
- `HatariSelectedGameRequest` (`canonical_platform`, `identity_state`, `verified_title`) from `project_hatari_launch_input`
- `HatariHealth.tos` from `inspect_hatari_game`
- A `HatariCommandPlan` producing argv, e.g.:
  - floppy: `hatari --config <cfg> --floppy-a <disk1> [--floppy-b <disk2>]`
  - hard disk/GEMDOS: `hatari --config <cfg> --harddisk <dir-or-image>`
  - cartridge: `hatari --cartridge <cart>`
  - plus `--machine <st|ste|tt|falcon>` when a machine model is known
- Execution via `process_spawn::PreparedProcessCommand` (never a shell), mirroring `duckstation_execution`/`flycast_execution` exactly.

### 14.3 Is Hatari one medium task from proper launch support?

**Yes.** Discovery, config, TOS verification, identity projection, and readiness are done and tested. The missing pieces are: a command-plan builder (small, ~150 lines following `flycast_command.rs`), an execution adapter (~200 lines following `flycast_execution.rs`), a `launch/mod.rs` re-export, and a Doctor adapter (small). That is a **single Medium task**, not a rewrite.

---

## 15. RETROARCH

- `retroarch_command.rs` contains **no Atari-specific core names** (only `mednafen_psx` in tests). Core selection is dynamic: `retroarch_platform_candidate` resolves an installed core's `.info` `systemname`/`database` through `platform_for_alias`.
- **Consequence:** any installed RetroArch core whose `.info` names an Atari system (Stella `Atari - 2600`, Atari800 `Atari - 800`, ProSystem `Atari - 7800`, Handy `Atari - Lynx`, Virtual Jaguar `Atari - Jaguar`, Hatari `Atari - ST`) **already produces a valid launch candidate for any platform with a resolved identity — but only AtariST has a `LAUNCH_COMPATIBILITY` row, and no Atari platform can reach a resolved identity today** (no IdentityPlatform variants).
- **Hidden capability:** adding `IdentityPlatform` variants + `LAUNCH_COMPATIBILITY` rows with core hints (`stella`, `atari800`, `prosystem`, `handy`/`beetle_lynx`, `virtualjaguar`, `hatari`) unlocks RetroArch launch for all six cartridge/disc-era systems **with zero new emulator code**, because RetroArch execution (`spawn_retroarch`) is already production-grade.
- **Recommendation:** for 2600/5200/7800/8-bit/Lynx/Jaguar, RetroArch is the right launch path; standalone adapters are only justified for ST (Hatari) and possibly Jaguar CD (BigPEmu — which has **no** code or profile in the repo).

---

## 16. LYNX

### 16.1 Parser (`lynx_header_evidence.rs`)

Verified against cc65/AtariAge LNX references:

| Field | Offset | Meaning |
|---|---|---|
| magic | 0x00 | `LYNX` (4 bytes) |
| bank0_page_size | 0x04 LE u16 | 256 typical |
| bank1_page_size | 0x06 LE u16 | 0 if absent |
| version | 0x08 LE u16 | must be 1 (`version_recognized`) |
| cart_name | 0x0A (32B ASCII) | emitted as **Corroborated ProductCode** |
| manufacturer | 0x2A (16B ASCII) | exposed |
| rotation | 0x3A | 0=None, 1=Left, 2=Right, else Unknown |

No checksum field exists in the LNX header (the format has none). Evidence: `BootStructure = "LYNX"` (Strong); scope = `PlatformSpecific("Atari Lynx")`; fusion rule `atari_lynx_header` exists.

### 16.2 Wiring trace

```
lynx_header_evidence (parser + tests)
   ├── archive_member_content_evidence ✓ (ZIP members)
   ├── header_normalization::Lynx64 ✓ (reversible strip/restore)
   ├── platform_evidence_fusion::atari_lynx_header ✓
   ├── discovery (loose .lnx) ✗ (not in CONTENT_FORMATS)
   ├── game_identity ✗ (no IdentityPlatform::AtariLynx)
   └── launch/ES-DE/RomM ✗
```

Same wiring shape as 7800 → **Small** task.

### 16.3 `.lyx`

`.lyx` is a raw headerless Lynx image (older Handy convention). No structural claim is possible; it must stay REGISTERED-ONLY + HASH/DAT-ONLY. The platform registry lists `.lyx` as strong — that is acceptable for *extension-level* selection but must never be treated as structural proof (it isn't: no magic rule exists for `.lyx`).

---

## 17. JAGUAR

### 17.1 What the Jaguar actually is (research summary)

- Retail Jaguar cartridges use a **custom 32-bit encrypted boot header**: the first 8 bytes (the boot loader) are encrypted with a keyed scheme; the header contains vectors and a checksum that emulators (Virtual Jaguar, BigPEmu) decrypt/verify using the known 32-bit "Jaguar" key scheme. There is **no plaintext ASCII magic** at a fixed offset, unlike NES/Lynx/7800.
- `.j64` is the emulator container with a 32-byte header (magic `JAGUAR` at 0x00 in BigPEmu's .j64 format, or "j64" variants); `.jag` is a raw dump; `.rom` is the raw CD/cart ROM.
- **What remains structurally observable without decryption:** a `.j64` container's own 32-byte header (magic + fields) could be validated as a container, and the *encrypted* first sector's length could be sanity-checked — but **no plaintext platform proof exists** in the ROM bytes themselves.
- **Normalization across J64/JAG/ROM:** stripping the 32-byte `.j64` header to obtain the raw `.jag`-equivalent payload is feasible and reversible (like Lynx64/Atari7800_128) — a candidate `HeaderNormalizationKind::JaguarJ64` — but the *payload* remains unverifiable without the decryption key, so normalized-hash identity would only reconcile headered/headerless dumps, not prove platform.

### 17.2 Current state

- `coverage_inventory.rs`: `Atari Jaguar` = **Deferred**, *"No corroborated generic internal header exists (per-title encrypted boot block) - deliberately not implemented (Batch 4)"*.
- No magic rule in PLATFORMS; `.j64`/`.jag` strong-extension only; **no parser, no normalization, no identity variant, no launch row.**
- **Recommendation:** keep Jaguar HASH/DAT-ONLY for identity. Optionally add `.j64` header stripping as a reversible normalization (Small) to reconcile dump variants — but do not attempt to validate the encrypted payload.

---

## 18. JAGUAR CD

### 18.1 Disc/session structure (research summary)

- Jaguar CD games are CD-ROMs with a **custom boot track/session**: track 1 is typically a data track containing the encrypted boot loader + the "Jaguar CD" security header (the BIOS checks a specific 32-byte signature/encryption handshake on the first data sector), followed by additional data/audio tracks. Some titles are pure Mode 1 ISO9660 with the security check in the first session.
- Dumps exist as **BIN/CUE** (raw 2352 + cue), **ISO** (2048 cooked), and **CHD**. Audio tracks are common.
- **BIOS relationship:** the Jaguar CD unit's BIOS performs the encryption check; the disc itself carries the boot data. Emulators (BigPEmu) need the CD BIOS + the disc's own boot header.

### 18.2 Generic optical stack reuse (all verified present)

- `iso9660.rs` (bounded PVD/root/dir walk) — reuse ✓
- `cue_bin.rs` (MODE1/2048, MODE1/2352, MODE2/2352; audio ignored) — reuse ✓
- `chd_identity.rs`/`chd_logical_media.rs` (track-1-only, zero-pregap, GD-ROM specialist refusal) — reuse ✓ for track-1-data Jaguar CD CHDs
- `disc_evidence_collector.rs` (`collect_disc_boot_evidence`: SYSTEM.CNF, IP.BIN, Saturn/SegaCD/3DO/PC-FX/NeoGeoCD) — **no Jaguar CD branch**

### 18.3 Safe or deferred?

**Deferred, with a narrow path forward.** A Jaguar CD data track that is plain ISO9660 can already be opened by the generic stack — but there is **no platform row**, **no IdentityPlatform variant**, **no boot signature detector**, and **no emulator**. Safely gaining platform evidence requires a `jaguarcd_boot_evidence` module (verify the documented first-data-sector security header against at least two independent sources — BigPEmu and Virtual Jaguar), which is a genuine new-parser task. CHD dispatch would then be a one-arm addition to the `.chd` match list in `game_identity.rs` (mirroring `PcEngineCd`). **Recommendation: defer until a boot-signature source is reviewed; do not whitelist Jaguar CD CHDs on extension alone.**

---

## 19. EMULATORS — ACTUAL INVENTORY

| Emulator | Discovery | Readiness | Planning | Execution | Doctor | GUI |
|---|---|---|---|---|---|---|
| Hatari (standalone) | ✓ `hatari_local.rs` | ✓ | ✓ (planning row) | **✗** | **✗** | setup panel only |
| Stella | ✗ | ✗ | ✗ | ✗ (RetroArch only) | ✗ | ✗ |
| Atari800 | ✗ | ✗ | ✗ | ✗ (RetroArch only) | ✗ | ✗ |
| ProSystem (7800) | ✗ | ✗ | ✗ | ✗ (RetroArch only) | ✗ | ✗ |
| Handy (Lynx) | ✗ | ✗ | ✗ | ✗ (RetroArch only) | ✗ | ✗ |
| Virtual Jaguar | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| BigPEmu (Jag CD) | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| RetroArch (all) | ✓ `emulator_environment/retroarch` | ✓ | ✓ (dynamic core resolution) | ✓ `spawn_retroarch` | ✓ | ✓ |

**Adjacent broken join:** RetroArch is fully capable of launching every Atari cartridge system today, but the platform rows that would let `build_launch_plan` produce a candidate are missing.

---

## 20. DAT / PRESERVATION

| Platform | Likely primary ecosystem | EmuWiz behavior today |
|---|---|---|
| 2600 / 5200 / 7800 | No-Intro (cartridge) | generic whole-file SHA-1/MD5/CRC32 matching works **once identity resolves a platform** |
| 7800 (headered) | No-Intro (headerless payload hashes) | **normalized hashing is NOT wired** — a headered .a78 will not match No-Intro's headerless hashes |
| 8-bit (ATR/XFD/CAS/CAR/XEX) | TOSEC | hash-only; TOSEC hashes cover the raw dumps |
| Lynx | No-Intro | same normalized-hash gap as 7800 |
| Jaguar | No-Intro | hash-only (correct) |
| Jaguar CD | Redump / TOSEC | generic disc/track matching possible but no platform row |
| ST/STE/TT/Falcon | TOSEC / No-Intro (floppy) | whole-file SHA-1 of the `.st`/`.stx`/`.msa` matches TOSEC dumps |

**Key DAT finding:** `header_normalization.rs` provides reversible 128-byte A78 and 64-byte Lynx stripping with round-trip tests, but **no DAT-matching path calls `strip_known_header`**. `dat_hash_representation` and `normalized_view_provenance` exist for N64 byte-order and SMD; extending the normalized-hash representation to A78/Lynx would let headered and headerless dumps of the same game converge to one DAT identity. **This is a hidden high-value capability** — see §21.

---

## 21. HEADER NORMALIZATION — HIDDEN VALUE

- `HeaderNormalizationKind::{Lynx64, Atari7800_128}` exist with exact lengths (64/128), reversible `reconstruct_with_header`, SHA-256-equality tests proving `strip_known_header(bytes)` hashes identically to the payload.
- **Production callers today:** `HeaderNormalizationDetector` (registered in `archive_member_content_evidence::member_detectors`) emits `ContentSignature` facts for ZIP members; `snes_header_evidence` uses `strip_known_header` for SNES copier headers. **No Atari DAT hashing or identity path calls it.**
- **Impact:** a No-Intro 7800 DAT stores headerless payload hashes; a headered `.a78` user file currently can only match if EmuWiz hashes the stripped payload. The machinery exists; the seam is missing.
- **Recommendation:** add A78/Lynx to the normalized-hash representation used by `dat_hash_representation` (following the N64 precedent), then wire `strip_known_header` into `inspect_loose_rom` for `.a78`/`.lnx` so both physical and normalized SHA-256 are emitted. **This single join makes headered/headerless Atari dumps converge in DAT identity — high value, Small-Medium task.**

---

## 22. CHEATS / MODS

- RetroArch cheat catalogue (`cheat_catalogue.rs`) resolves platform aliases generically — `"Atari - 2600"` → `Atari2600` works through the shared alias table, so RetroArch .cht cheats are already reachable **once identity/launch resolves an Atari platform**.
- Hatari: no patch/cheat/mod adapter exists (no `hatari_cheat.rs`, no config-rewrite support).
- BigPEmu/Virtual Jaguar: nothing in the repo.
- Texture packs / widescreen patches / trainers: nothing Atari-specific anywhere.

**Do not pad this section with speculation — the genuine state is: RetroArch cheats generic-only; zero Atari-specific cheat/mod code.**

---

## 23. MULTI-DISK

- No Atari-specific multi-disk grouping exists. ST multi-disk games (A/B/C drives), 8-bit multi-disk titles, and Jaguar CD multi-disc releases all rely on the generic multi-file/multi-disc machinery (`playing_library`, `MultidiscHandlingPolicy`).
- **Gap:** Hatari's config model exposes two floppy drives (`szDiskAFileName`/`szDiskBFileName`) and `HatariFloppy { drive A, drive B }`; the launch projection (`project_hatari_launch_input`) builds a `HatariSelectedGameRequest` but has **no multi-disk payload** — a Disk 1/Disk 2 game cannot currently project both floppies into one Hatari launch.
- **Recommendation:** extend `HatariSelectedGameRequest` with optional second-floppy path and have the future `hatari_command.rs` emit `--floppy-a/--floppy-b`. This is part of the Hatari Medium task, not a separate one.

---

## 24. ROMM

- Inbound `ROMM_SLUG_ALIASES` (normalise.rs): `atari-st` → `AtariST` is the **only** Atari slug.
- Outbound `STATIC_TABLE` (romm_platform_mapping.rs): **zero Atari rows** (only Dreamcast, GB/GBA/GBC, MegaDrive, N64, NDS, Neo Geo CD, GameCube, PSX/PSP/PS3/Vita, Sega 32X, SNES, Xbox/360, MasterSystem, PC Engine CD, PC).
- Per system: 2600 **Missing** · 5200 **Missing** · 7800 **Missing** · Atari 8-bit **Missing** · Lynx **Missing** · Jaguar **Missing** · Jaguar CD **Missing** (no platform row) · ST **Folded** (inbound only) · STE/TT/Falcon **Folded** (no row; `atari-st` covers them, which is acceptable but lossy).
- **Recommendation:** add outbound rows: `atari-2600`, `atari-5200`, `atari-7800`, `atari-800`, `atari-lynx`, `atari-jaguar`, `atari-st` (verified against the RomM supported-platforms page before committing — the table's own provenance convention requires a reviewed source).

---

## 25. ES-DE — INDEPENDENT VERIFICATION

Verified independently: `ES_DE_SYSTEM_MAP` contains exactly one Atari row:

```rust
platform_id: "AtariST", es_de_system: "atarist", es_de_fullname: "Atari ST"
```

**Missing rows (ES-DE system short names from ES-DE's own `es_systems.xml` conventions, which this project's existing rows already follow):**

| EmuWiz id | ES-DE system (expected) | ES-DE fullname (expected) |
|---|---|---|
| Atari2600 | `atari2600` | Atari 2600 |
| Atari5200 | `atari5200` | Atari 5200 |
| Atari7800 | `atari7800` | Atari 7800 |
| Atari 8-bit | `atari800` | Atari 800 |
| Atari Lynx | `atarilynx` | Atari Lynx |
| Atari Jaguar | `atarijaguar` | Atari Jaguar |
| AtariST (STE/TT/Falcon) | `atarist` (existing; STE/TT/Falcon have no separate ES-DE systems) | Atari ST |

(These names follow the same `resources/systems/linux/es_systems.xml` source the module already cites; the exact strings must be re-verified against that file before committing, per the module's own discipline.)

---

## 26. DOCTOR — WHERE THE CHAIN ENDS

- `diagnostics/profiles.rs` imports and assesses: Dolphin, DuckStation, PCSX2, PPSSPP, RPCS3, Xemu, Xenia. **Hatari is absent** from the import list, from `DEFERRED_CHECKS`, from `managed_scan_targets`, and from any writability assessment.
- `diagnostics/runner.rs`/`mod.rs` therefore can never surface: missing TOS, unreadable `hatari.cfg`, unsupported ST image, malformed STX, unknown machine model, or missing Hatari executable.
- The chain ends at `patch_manager::hatari_local::HatariHealth` — computed, tested, and then **never consumed by any diagnostic subsystem**.
- **Proof of the gap:** grep for `hatari` in `crates/archivefs-core/src/diagnostics/` → **zero matches**.

---

## 27. GUI-HIDDEN CAPABILITIES

Backend facts the GUI cannot currently surface (all verified present in core, none exposed):

- A78 header: region (NTSC/PAL), mapper bits (POKEY@$4000, SuperGame bank-switch), cart title, declared ROM size.
- Lynx header: cart name, manufacturer, bank page sizes, rotation.
- STX: Pasti version, tool, revision, declared/validated track records, declared sectors — plus protection/timing **not** read (by design).
- `.st`: full FAT12 geometry (sectors/cluster, FAT count, root entries, sides, tracks) + "Probable, needs folder" honesty note.
- TOS: SHA-256, Verified/PresentUnverified/Missing, version/region (when caller supplies references).
- Hatari: machine model (ST/STE/TT/Falcon), CPU family, monitor type, floppy representations, storage mechanisms, save-state inventory.
- DAT provenance: whether a match came from TOSEC vs No-Intro, and the source revision.

---

## 28. SECURITY / FAIL-CLOSED

| Rule | Safe? | Analysis |
|---|---|---|
| `.bin` → any Atari platform | **Safe** | weak extension everywhere; never confirmed by extension alone |
| `.st` → AtariST | **Safe** | structural Probable only; folder/DAT raises to Confirmed |
| `.stx` → AtariST | **Safe** | Pasti is conclusive but only when the parser validates |
| `.img`/`.dsk` size heuristics | **Safe** | no size-only identity exists in `disk_format`; `.dsk` requires CPCEMU header walk |
| `.hdf` | **Safe** | Amiga path refuses without `inspect_amiga_image`; Atari HDD unmodeled |
| `.iso` | **Safe** | DiscImage content kind only; platform needs folder/DAT |
| `.a78`/`.lnx` | **Safe but invisible** | parsers fail closed on wrong magic; no platform promotion without fusion |
| `.rom` (Jaguar/2600/8-bit) | **Safe** | weak only; no magic rule |
| Folder name = verified platform | **Safe** | folder alias is `FolderAlias` tier; DAT/manual can override; conflicts are reported |
| Shell-string launch | **Safe** | `process_spawn` uses `Command::new` + `args`; no shell |

**No unsafe Atari promotion paths found.** The fail-closed design is consistent.

---

## 29. REAL-CORPUS STATUS

| Platform | Status (coverage_inventory.rs) | Evidence rule / specimen |
|---|---|---|
| Atari7800 | **SyntheticValidated** | `atari7800_header`; "No real .a78 specimen found accessible in the corpus (Batch 4)" |
| Atari Lynx | **RealValidated** | `atari_lynx_header`; "Real specimen: Joust.lnx - Resolved through fusion (Batch 4/5)" |
| Atari Jaguar | **Deferred** | per-title encrypted boot block; deliberately not implemented |
| Atari2600 | **NoCoverage** (no entry) | — |
| Atari5200 | **NoCoverage** | — |
| Atari 8-bit | **NoCoverage** | — |
| AtariST | **NoCoverage** | — (ST structural parsers are tested synthetically in `disk_format/tests.rs` but have no coverage-inventory row) |

`normalized_view_provenance.rs` additionally proves physical-vs-normalized byte distinctness for every supported normalization including Lynx64/Atari7800_128 — a real, tested provenance guarantee.

---

## 30. MATURITY MATRIX

Legend: **MATURE** · **PARTIAL** · **ORPHANED** · **REGISTERED-ONLY** · **MISSING** · **N/A** · **INTENTIONALLY UNSUPPORTED**

| Capability | 2600 | 5200 | 7800 | Atari8 | Lynx | Jaguar | JagCD | ST | STE | TT | Falcon |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Platform registry | MATURE | MATURE | MATURE | MATURE | MATURE | MATURE | **MISSING** | MATURE | PARTIAL (folded) | PARTIAL (folded) | PARTIAL (folded) |
| Media registry (media_registry) | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING |
| Content registry (CONTENT_FORMATS) | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | PARTIAL (.st/.msa/.ipf) | PARTIAL | PARTIAL | PARTIAL |
| Inspector (likely ext) | MATURE | MATURE | MATURE | PARTIAL | MATURE | MATURE | PARTIAL | MATURE | MATURE | MATURE | MATURE |
| Structural parser | N/A | N/A | **ORPHANED** (a78) | MISSING | **ORPHANED** (lnx) | INTENTIONALLY UNSUPPORTED | MISSING | MATURE (.st/.stx) | MATURE | MATURE | MATURE |
| Production identity | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING |
| Stable ID (hash/header) | PARTIAL (hash) | PARTIAL (hash) | PARTIAL (hash) | PARTIAL (hash) | PARTIAL (hash) | PARTIAL (hash) | MISSING | PARTIAL (hash) | PARTIAL | PARTIAL | PARTIAL |
| DAT/hash identity | MATURE (generic) | MATURE | PARTIAL (normalized gap) | MATURE | PARTIAL (normalized gap) | MATURE | PARTIAL | MATURE | MATURE | MATURE | MATURE |
| Header normalization | N/A | N/A | **ORPHANED** (strip unused) | N/A | **ORPHANED** (strip unused) | N/A | N/A | N/A | N/A | N/A | N/A |
| Persistence | MATURE | MATURE | MATURE | MATURE | MATURE | MATURE | MISSING | MATURE | MATURE | MATURE | MATURE |
| Firmware (TOS) | N/A | N/A | N/A | N/A | N/A | N/A | MISSING | MATURE (verify) | MATURE | MATURE | MATURE |
| Emulator discovery | PARTIAL (RA only) | PARTIAL | PARTIAL | PARTIAL | PARTIAL | PARTIAL | MISSING | MATURE (Hatari+RA) | MATURE | MATURE | MATURE |
| Readiness | PARTIAL | PARTIAL | PARTIAL | PARTIAL | PARTIAL | PARTIAL | MISSING | MATURE | MATURE | MATURE | MATURE |
| Planning | PARTIAL (RA) | PARTIAL | PARTIAL | PARTIAL | PARTIAL | PARTIAL | MISSING | PARTIAL (row, no exec) | PARTIAL | PARTIAL | PARTIAL |
| Execution | PARTIAL (RA) | PARTIAL | PARTIAL | PARTIAL | PARTIAL | PARTIAL | MISSING | PARTIAL (RA; Hatari **MISSING**) | PARTIAL | PARTIAL | PARTIAL |
| GUI launch | PARTIAL | PARTIAL | PARTIAL | PARTIAL | PARTIAL | PARTIAL | MISSING | PARTIAL | PARTIAL | PARTIAL | PARTIAL |
| Doctor | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | **MISSING** (Hatari absent) | MISSING | MISSING | MISSING |
| Cheats | PARTIAL (RA generic) | PARTIAL | PARTIAL | PARTIAL | PARTIAL | PARTIAL | MISSING | PARTIAL | PARTIAL | PARTIAL | PARTIAL |
| Mods | N/A | N/A | N/A | N/A | N/A | N/A | N/A | N/A | N/A | N/A | N/A |
| Rename / Duplicates / 1G1R | MATURE (generic) | MATURE | MATURE | MATURE | MATURE | MATURE | MISSING | MATURE | MATURE | MATURE | MATURE |
| Playing Library | MATURE (generic) | MATURE | MATURE | MATURE | MATURE | MATURE | MISSING | PARTIAL (multi-disk gap) | PARTIAL | PARTIAL | PARTIAL |
| RomM | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | PARTIAL (inbound only) | PARTIAL | PARTIAL | PARTIAL |
| ES-DE | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MATURE | PARTIAL | PARTIAL | PARTIAL |
| Multi-disc | N/A | N/A | N/A | MISSING | N/A | N/A | MISSING | PARTIAL (A/B drives unprojected) | PARTIAL | PARTIAL | PARTIAL |
| Real corpus | NoCoverage | NoCoverage | Synthetic | NoCoverage | **Real (Joust.lnx)** | Deferred | NoCoverage | NoCoverage | NoCoverage | NoCoverage | NoCoverage |

---

## 31. BROKEN JOINS — TOP 15 (ranked)

1. **`CONTENT_FORMATS` omits every Atari cartridge/computer extension** — `.a26/.a52/.a78/.atr/.atx/.xfd/.xex/.cas/.car/.j64/.jag/.lnx/.lyx/.stx`. Effect: loose Atari files are invisible to discovery. Fix: ~15 table lines. *(P0, Tiny)*
2. **`IdentityPlatform` has zero Atari variants** — the whole identity pipeline (serial/header/hash evidence, evidence bridge, launch identity) is unreachable for Atari. *(P0, Small)*
3. **`supported_loose_rom_format` has no Atari rows** — even with a platform hint, loose ROMs return Unsupported. *(P0, Tiny)*
4. **7800 header parser orphaned from identity** — `atari7800_header_evidence` tested + fusion rule exists; nothing calls it for loose files. *(P0, Small)*
5. **Lynx header parser orphaned from identity** — same shape as 7800. *(P0, Small)*
6. **`ES_DE_SYSTEM_MAP` missing 6 Atari rows** — only `atarist` mapped. *(P0, Tiny)*
7. **`LAUNCH_COMPATIBILITY` missing 6 Atari rows** — RetroArch core hints (`stella`, `atari800`, `prosystem`, `handy`, `virtualjaguar`) absent; only `AtariST` row exists (and it lacks execution). *(P0, Tiny)*
8. **Hatari has no `hatari_command.rs`/`hatari_execution.rs`** — `LAUNCH_COMPATIBILITY` promises `standalone_adapters: ["hatari"]`; `project_hatari_launch_input` and `hatari_firmware_readiness` are done; the command/execution seam is missing. *(P1, Medium)*
9. **Hatari absent from Doctor** — `diagnostics/profiles.rs` covers 7 emulators, not Hatari; TOS health is computed and dropped. *(P1, Small)*
10. **Header normalization not wired into DAT hashing** — A78/Lynx strip/restore is tested but unused for identity/DAT; headered dumps cannot match No-Intro headerless hashes. *(P1, Small-Medium)*
11. **`.stx` not in `CONTENT_FORMATS`** — Pasti is production-wired for detection/Hatari but invisible to loose-file discovery. *(P0, Tiny)*
12. **`coverage_inventory.rs` missing Atari2600/5200/8-bit/ST rows** — the coverage report undercounts the family and hides the ST parser work. *(P2, Tiny)*
13. **Multi-disk ST projection missing** — `HatariFloppy` models A/B; `HatariSelectedGameRequest` carries one disk. *(P2, Small, depends on #8)*
14. **RomM outbound table has zero Atari rows** — `atari-st` exists inbound only. *(P2, Tiny)*
15. **Jaguar CD absent end-to-end** — no platform row, no identity variant, no boot detector, no launch. *(P2, Medium; needs boot-signature research first)*

---

## 32. ORPHANED CODE (tested, no production caller)

| Module / function | Tests | Missing seam | Size |
|---|---|---|---|
| `atari7800_header_evidence::parse_a78_header` / `Atari7800HeaderDetector` | 12 tests | loose-file dispatch in `game_identity` + `CONTENT_FORMATS` | Small |
| `lynx_header_evidence::parse_lynx_header` / `LynxHeaderDetector` | 12 tests | same | Small |
| `header_normalization::strip_known_header` (Lynx64, Atari7800_128) | 6 tests | DAT normalized-hash representation; loose-ROM dual-hash | Small–Medium |
| `disk_format::atari_stx::inspect` | 8 tests | `CONTENT_FORMATS` + `game_identity` .stx dispatch (currently detection/Hatari-wired only) | Tiny |
| `patch_manager::hatari_local` full surface | 14 tests | `hatari_command`/`hatari_execution` + Doctor adapter | Medium |
| `launch::input_projection::project_hatari_launch_input` | tests in module | command-plan consumer | Tiny (part of Medium) |
| `launch::readiness::hatari_firmware_readiness` | tests in module | same | Tiny |

---

## 33. DO NOT REBUILD (leave-alone list)

1. **`disk_format/atari_stx.rs`** — Pasti v3 parser: signature/version/track-walk/bounds all correct, fail-closed, 8 tests. Rewriting risks breaking production detection.
2. **`disk_format/atari_st.rs`** — honest FAT12 BPB inspector with explicit non-conclusive semantics. Do not "improve" into false certainty.
3. **`atari7800_header_evidence.rs`** — verified against the A78 spec; complete for v1–3. Only wiring is missing.
4. **`lynx_header_evidence.rs`** — verified against cc65/AtariAge references; complete for v1.
5. **`header_normalization.rs`** — reversible strip/restore with round-trip tests; extend *usage*, never the transform.
6. **`patch_manager/hatari_local.rs`** — deep, bounded INI parser + SHA-256 TOS verifier with correct external-reference design. Do not embed hashes.
7. **`disk_format/mod.rs` `inspect_disk_format` dispatch & `BoundedReader`** — the read-budget/symlink-safe design is the crate-wide standard.
8. **`.hdf` Amiga collision handling** — keep Atari hard disks out of the Amiga path entirely.
9. **RetroArch execution (`spawn_retroarch`)** — production-grade; reuse for all Atari cartridge systems via new rows.
10. **IPF** — remain preservation-container-only; no decoder without SPS licensing.

---

## 34. BEST IMPLEMENTATION TASKS

### P0 — broken joins

1. **`content-registry-atari-extensions`** — add `.a26 .a52 .a78 .atr .atx .xfd .xex .cas .car .j64 .jag .lnx .lyx .stx` to `CONTENT_FORMATS`.
   Files: `ingestion/content_registry.rs`. Reused: `ContentKind::{RomCartridge, ComputerDisk, TapeImage}`. Tests: registry round-trip. Size: **Tiny**. Dep: none.

2. **`identity-platform-atari-variants`** — add `Atari2600, Atari5200, Atari7800, Atari8Bit, AtariLynx, AtariJaguar, AtariSt` to `IdentityPlatform` + `from_catalogue` + `label`.
   Files: `game_identity.rs`. Tests: catalogue mapping. Size: **Tiny**. Dep: none.

3. **`loose-rom-atari-dispatch`** — extend `supported_loose_rom_format` with Atari rows; in `inspect_loose_rom`, call `parse_a78_header`/`parse_lynx_header` and emit BootStructure/ProductCode evidence + physical & normalized SHA-256.
   Files: `game_identity.rs`. Reused: `atari7800_header_evidence`, `lynx_header_evidence`, `header_normalization::strip_known_header`. Tests: synthetic .a78/.lnx/.a26/.j64 fixtures. Size: **Small**. Dep: #1, #2.

4. **`esde-atari-rows`** — add `atari2600, atari5200, atari7800, atari800, atarilynx, atarijaguar` rows to `ES_DE_SYSTEM_MAP`.
   Files: `launch/es_de_export.rs`. Tests: mapping-row tests. Size: **Tiny**. Dep: #2.

5. **`launch-compat-atari-rows`** — add LAUNCH_COMPATIBILITY rows for 2600 (`stella`), 5200 (`a5200`), 7800 (`prosystem`), 8-bit (`atari800`), Lynx (`handy`/`beetle_lynx`), Jaguar (`virtualjaguar`).
   Files: `launch/platform_map.rs`. Tests: candidate-generation tests. Size: **Tiny**. Dep: #2.

### P1 — completeness

6. **`hatari-command-execution`** — `launch/hatari_command.rs` + `launch/hatari_execution.rs` + `mod.rs` re-exports; argv via `--floppy-a/--floppy-b/--harddisk/--cartridge/--machine`; spawn via `process_spawn`.
   Files: new `hatari_command.rs`, `hatari_execution.rs`; `launch/mod.rs`. Reused: `hatari_local`, `project_hatari_launch_input`, `process_spawn`. Tests: preflight/plan unit tests. Size: **Medium**. Dep: #5.

7. **`doctor-hatari-adapter`** — add Hatari to `diagnostics/profiles.rs` (discovery + TOS health + writability).
   Files: `diagnostics/profiles.rs`, `diagnostics/runner.rs`, `diagnostics/mod.rs`. Reused: `discover_hatari_profiles`, `inspect_hatari_game`. Tests: runner finding tests. Size: **Small**. Dep: none.

8. **`dat-normalized-atari-hash`** — extend `dat_hash_representation`/`normalized_view_provenance` with A78/Lynx normalized SHA-256; wire `strip_known_header` into loose-ROM dual-hash.
   Files: `dat_hash_representation.rs`, `normalized_view_provenance.rs`, `game_identity.rs`. Tests: headered/headerless convergence test. Size: **Small–Medium**. Dep: #3.

9. **`stx-content-registry`** — add `.stx` to `CONTENT_FORMATS` (ComputerDisk) and dispatch in `game_identity` to `disk_format::atari_stx::inspect`.
   Files: `content_registry.rs`, `game_identity.rs`. Tests: STX identity fixture. Size: **Small**. Dep: #2.

### P2 — new parsers / features

10. **`disk-format-msa`** — new `disk_format/msa.rs`: header `0x0E0F`, track walk, RLE *validation* (not decode), caps per §10.3.
    Files: `disk_format/msa.rs`, `disk_format/mod.rs`. Tests: synthetic valid/truncated/bad-magic/overflow. Size: **Small–Medium**. Dep: none.

11. **`disk-format-atr`** — new `disk_format/atr.rs`: 16-byte header, magic `0x0296`, sector size 128/256, length-exact geometry. Corroborated evidence.
    Files: `disk_format/atr.rs`, `disk_format/mod.rs`. Tests: geometry fixtures. Size: **Small**. Dep: none.

12. **`car-header-evidence`** — new `car_header_evidence.rs` for Atari 8-bit/5200 `CART` header (type field separates 8-bit from 5200).
    Files: new module + `member_detectors()` + fusion rule + identity dispatch. Tests: type-table fixtures. Size: **Small**. Dep: #1, #2.

13. **`jaguarcd-boot-evidence`** — new `jaguarcd_boot_evidence.rs` (first-data-sector security header, verified against ≥2 independent sources) + platform row + `.chd` match arm.
    Files: new module, `platform/mod.rs`, `game_identity.rs`, `disc_evidence_collector.rs`. Tests: boot-sector fixtures. Size: **Medium**. Dep: research-first; #2.

14. **`romm-atari-outbound`** — add 6 outbound slugs (verified against RomM supported-platforms page).
    Files: `romm_platform_mapping.rs`. Tests: slug tests. Size: **Tiny**. Dep: none.

---

## 35. FINAL ANSWERS

### 1. Five cheapest changes that dramatically improve Atari support
1. Add the 14 missing extensions to `CONTENT_FORMATS` (Tiny).
2. Add the 7 `IdentityPlatform` Atari variants (Tiny).
3. Extend `supported_loose_rom_format` + call the existing A78/Lynx parsers (Small).
4. Add 6 `LAUNCH_COMPATIBILITY` rows with RetroArch core hints (Tiny).
5. Add 6 `ES_DE_SYSTEM_MAP` rows (Tiny).

These five are pure wiring with zero new parsers and unlock scanning, identity, RetroArch launch, and ES-DE export for the whole cartridge family.

### 2. Which Atari systems already have most of their backend secretly finished?
**Atari 7800** and **Atari Lynx** (full header parsers + normalization + fusion rules + scope catalog, only missing identity dispatch and registry rows), and **Atari ST** (production `.st`/`.stx` parsers + complete Hatari config/TOS/readiness/projection stack, only missing command/execution/Doctor).

### 3. Which formats genuinely need new parser work?
- **MSA** (bounded header/track-walk — Small-Medium, clearly specified).
- **CAR** (8-bit/5200 cartridge header — Small, high value, disambiguates 5200 vs 8-bit).
- **ATR** (bounded header — Small).
- **XEX** (segment-walk — Small).
- **CAS** (FUJI magic + chunk framing — Small, disambiguates from MSX).
- **Jaguar CD boot signature** (Medium, research-first).
- **ATX** (defer; needs VAPI review).

### 4. Which formats should remain DAT/hash-only?
- **2600 `.a26`/`.bin`** (no intrinsic header; mapper needs DAT).
- **Jaguar `.j64`/`.jag`/`.rom`** (encrypted boot; `.j64` header-strip optional).
- **`.lyx`** (headerless).
- **`.xfd`** (headerless; weak corroboration only).
- **IPF** (SPS-licensed container; pass-through only).
- **ST/TT/Falcon hard-disk images** (`.hdf`/`.vhd`) until a real AHDI/ICD parser is built.

### 5. Is Hatari really one medium task away from proper launch support?
**Yes.** Discovery, config parsing, TOS SHA-256 verification, machine-model projection, identity projection, firmware readiness, and the `LAUNCH_COMPATIBILITY` row all exist and are tested. The missing `hatari_command.rs` + `hatari_execution.rs` + Doctor adapter is a single Medium task following the existing `flycast_command/execution` template.

### 6. Is Atari ST bare `.st` structural identity currently safe?
**Yes — and honestly labeled.** The `.st` parser validates FAT12/BPB geometry and explicitly refuses to claim Atari ST from bytes alone (`proves_platform() == false`); platform selection is delegated to folder/DAT corroboration. This is the correct safety level. **Do not weaken it.**

### 7. What should be completed before the next EmuWiz release?
P0 items 1–5 (registry rows, identity variants, loose-ROM dispatch, LAUNCH_COMPATIBILITY, ES-DE), plus item 9 (`.stx` in CONTENT_FORMATS). That set — all wiring, no new parsers — makes the entire Atari cartridge family scannable, identifiable, DAT-matchable, RetroArch-launchable, and ES-DE-exportable with the code that already exists.
