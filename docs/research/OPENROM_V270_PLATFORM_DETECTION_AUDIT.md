# OpenROM v2.7.0 platform-detection audit

Research-only audit. No production Rust, tests, GUI, Publisher Profiles, Save Vault, or disc-detection code was changed.

## Scope and revisions

- EmuWiz starting SHA: `feff8ea65357b1887b0f39ec22ac5083f57182ab`
- OpenROM tag: `v2.7.0`
- OpenROM commit inspected: `8ef3d49b8570905c4ce3e5adae1d80460fa9739e`
- OpenROM was inspected from a separate read-only checkout at `/tmp/openrom-v270-audit`.
- Platforms compared: 11. Already covered by EmuWiz evidence paths: 11. Confirmed gaps: 0.

The audit treats “detected” as a structural observation, not an automatic platform assignment. This is important because several signatures are shared by a filesystem family or can be present in deliberately malformed data.

## OpenROM source inspected

The relevant implementation is concentrated in:

- `core/detector.py`: `detect_file` (188–225), `_guess_platform` (265–334), Xbox helpers (337–349), platform helpers (352–398), and `_read_chd_type` (401 onward).
- `core/converter.py`: `Converter._to_chd` (145–173) and `Converter._from_chd` (175–196).
- `test_detector.py`: Xbox offsets and four new detector tests (43–83).
- `test_tools.py`: ISO-to-CHD command-selection tests (196–204).
- `README.md` and `flatpak/io.github.M5Devs.OpenROM.metainfo.xml`: release/tooling claims only; conclusions below are from source, not the announcement.

### OpenROM rules exactly as implemented

| Platform / rule | Signature and offset | Conditions, fallback, ambiguity |
|---|---|---|
| Saturn | `SEGA SEGASATURN` at `0x10`, or `SEGA SATURN ` at `0x00` | Checked for `ISO`, `BIN`, `IMG`, and `CUE`; first match wins. No field-length, maker/version, media, or conflict validation. The `0x10` rule is a broad fixed-byte check and does not validate a complete Saturn System ID. |
| Sega CD / Mega-CD | `SEGADISCSYSTEM` at `0x10`, or `SEGA_CD` at `0x00` | Same format gate and first-match behaviour. No product-field validation. The usual `SEGADISCSYSTEM`-at-zero layout is not the primary rule in this function. |
| PC Engine CD / TurboGrafx-CD | substring `PC Engine CD-ROM SYSTEM` anywhere in bytes `0..0x800` | No fixed offset, sector/read-mode check, boot-pointer validation, or filesystem condition. A match anywhere in the first 2048 bytes returns the platform. |
| Neo Geo CD | substring `NEO-GEO CD` anywhere in bytes `0..0x800` | No `IPL.TXT` lookup or manifest-structure validation. |
| Dreamcast | `.gdi` extension unconditionally returns Dreamcast | ISO/IMG/BIN/CUE only reach magic rules; there is no Dreamcast `IP.BIN` signature helper in `core/detector.py`. A generic ISO with no match falls through to size guessing. |
| PS1 | substring `PlayStation` in bytes `0..0xFFFF` | No `SYSTEM.CNF` lookup, `BOOT=` check, serial-family validation, or `PS-X EXE` validation. If absent, size fallback may return PS1 below 700 MiB. |
| PS2 | substring `PLAYSTATION` in PVD window `0x8000..0x87FF` | Only the window is bounded; there is no ISO9660 descriptor/type validation or `SYSTEM.CNF BOOT2=` / ELF check. If absent, size fallback may return `PS2 / GC` or `PS2 / Xbox`. |
| PSP | substring `PSP_GAME` or `UMD_DATA` anywhere in `0..0xFFFF` for ISO/IMG | No filesystem lookup. CSO/ZSO use size only: below 2 GiB PSP, otherwise `PSP / PS2`. |
| Xbox | `MICROSOFT*XBOX*MEDIA` at `0x10000` | ISO/IMG fast path; `0x10000` is logical sector 32 × 2048. No XDVDFS traversal or XBE validation. `XISO` extension is an unconditional Xbox override. |
| Xbox alternate layout | Same 20-byte signature at `0x2090000` | Separate slow seek for ISO/IMG only; exceptions return false. The comment calls this “Xbox 360 / alt layout”; the detector returns the generic `Xbox` label and does not disambiguate original Xbox vs Xbox 360. |
| GameCube | big-endian `0xC2339F3D` at `0x1C` | ISO/IMG only; exact four-byte field, no `nod` structure validation. First matching rule wins. |
| Wii | big-endian `0x5D1C9EA3` at `0x18` | ISO/IMG only; exact four-byte field, no partition validation. RVZ/WIA/WBFS/GCZ are format-labelled as GameCube/Wii or Wii rather than inspected here. |

All byte checks fail safely on short slices in practice because equality against the requested slice returns false; `_read_header` returns an empty buffer on I/O failure. There is no explicit conflict state: the ordered `if` chain returns the first platform.

## EmuWiz source and evidence inspected

Compared files/functions:

- `crates/archivefs-core/src/disc_evidence_collector.rs`: `collect_disc_boot_evidence` (300 onward), `collect_chd_evidence`, `open_chd_iso9660`, `open_chd_raw_track`, and `collect_gc_wii_evidence`.
- `saturn_boot_evidence.rs`: `parse_saturn_system_id`, `observe_saturn_evidence`, `SaturnSystemIdDetector`.
- `segacd_boot_evidence.rs`: `looks_like_sega_cd_boot_sector`, `parse_segacd_product_code`, `SegaCdBootDetector`.
- `pcengine_cd_boot_evidence.rs`: `parse_pce_cd_ipl` and constants for the first data-track second sector.
- `neogeocd_boot_evidence.rs`: `parse_ipl_txt`, `IplTxtFact::is_structurally_valid`, `observe_neogeocd_evidence`.
- `dreamcast_boot_evidence.rs`: `parse_ip_bin_meta`, `observe_ip_bin_evidence`; recognised IDs are `SEGA SEGAKATANA` and `SEGA SEGAMARIO` at logical offset zero.
- `playstation_boot_evidence.rs` and `ps2_boot_evidence.rs`: `parse_system_cnf_boot`, `parse_system_cnf_boot2`/PS2 wrapper, and bounded `PS-X EXE`/ELF checks.
- `psp_boot_evidence.rs`: `PspLayoutObservation` and `observe_psp_evidence`; `UMD_DATA.BIN` is the strong medium-specific leg.
- `xbox_boot_evidence.rs` and `xdvdfs_signature.rs`: XDVDFS at logical sector 32 (`0x10000`), plus bounded `/default.xbe` and `XBEH` evidence.
- `gamecube_wii_boot_evidence.rs`: `nod::Disc::new_with_options`, `header.is_gamecube()` / `header.is_wii()`, partition/apploader/FST/`main.dol` observations.
- `game_identity.rs`: specialist source inspectors at `inspect_saturn_source`, `inspect_dreamcast_source`, `inspect_sega_cd_source`, `inspect_pcengine_cd_source`, `inspect_neogeocd_source`, `inspect_ps1_iso`, `inspect_ps2_iso`, PSP/SFO inspection, `inspect_dolphin_header`, Xbox identity paths, and `inspect_disc_chd`.
- `chd_identity.rs`, `chd_logical_media.rs`, `chd_optical_specialist.rs`, `ingestion/gdi.rs`, and `repair/optical_conversion.rs`: CHD media classes, track selection, GD-ROM boundary handling, GDI selection, and existing verified CUE/BIN conversion.

EmuWiz’s current architecture is deliberately evidence-directed. `game_identity` may be asked to inspect a selected platform, but it does not treat a filename, generic ISO9660, or a weak substring as final identity. Structural evidence is retained separately from DAT-derived identity and launch projections.

## Comparison matrix

| Platform | OpenROM detection rule | Exact offset/signature | EmuWiz current rule | Equivalent? | OpenROM stronger? | EmuWiz stronger? | Potential missing primitive | False-positive risk | Recommendation |
|---|---|---|---|---|---|---|---|---|---|
| Sega Saturn | Two fixed checks | `0x10: SEGA SEGASATURN`; `0x00: SEGA SATURN ` | 0x100-byte System ID; exact `SEGA SEGASATURN` at `0x00`, fail-closed truncation, product field | Yes, same useful family and safer | No | Yes: correct field plus length/fields | None proven | High for OpenROM’s `0x10` rule; low for EmuWiz | ALREADY COVERED |
| Sega CD / Mega-CD | Two fixed checks | `0x10: SEGADISCSYSTEM`; `0x00: SEGA_CD` | `SEGADISCSYSTEM` at logical offset 0; 0x200-byte Disc ID and validated product field at `0x180` | Yes for real boot signature; implementations differ | No | Yes: correct offset and product validation | None proven | OpenROM can miss normal offset-zero discs and accept weak alternate bytes | ALREADY COVERED |
| PC Engine CD / TurboGrafx-CD | First-2-KiB substring | Anywhere in `0..0x7FF` | 128-byte IPL at first data-track LBA 1 (`0x800` logical byte offset); exact signature at record offset `32`; nonzero boot count and span bounds | Yes; EmuWiz is more exact | No | Yes: sector/topology/field validation | None proven | OpenROM can match unrelated text in sector 0; EmuWiz risk is bounded | ALREADY COVERED |
| Neo Geo CD | First-2-KiB substring | Anywhere in `0..0x7FF` | Root `IPL.TXT`, parsed 8.3/bank/offset entries, terminator `0x1A`, max 32 entries; identity path can require required loader extensions | Yes at platform-evidence level; not byte-equivalent | No | Yes: filesystem and manifest structure | None proven | OpenROM substring is readily forgeable/colliding | ALREADY COVERED |
| Dreamcast | `.gdi` override | Descriptor extension; no magic in detector | `IP.BIN` logical offset 0; exact `SEGA SEGAKATANA`/`SEGA SEGAMARIO`; GDI selects high-density data track | Yes for GDI; EmuWiz covers content more safely | No | Yes: boot signature and GD topology | None proven | OpenROM trusts extension; EmuWiz still fails closed on malformed/missing signature | ALREADY COVERED |
| PS1 | First-64-KiB substring or size fallback | `PlayStation` anywhere in `0..0xFFFF`; `<700 MiB` fallback | ISO9660 root `SYSTEM.CNF`, `BOOT=`, supported serial family, named file and `PS-X EXE` magic; CHD reuses decoded path | Yes as broad identification, not as equivalent authority | No | Yes | None proven | Very high for OpenROM substring/size | ALREADY COVERED |
| PS2 | PVD substring or size fallback | `PLAYSTATION` anywhere `0x8000..0x87FF`; size fallback | `SYSTEM.CNF` `BOOT2=`, named executable, ELF and reviewed PS2 identity/hash path; CHD reuse | Yes, EmuWiz stronger | No | Yes | None proven | PVD text can be forged; size overlaps GC/Xbox | ALREADY COVERED |
| PSP | First-64-KiB substring or CSO/ZSO size | `PSP_GAME`/`UMD_DATA` in first 64 KiB; CSO/ZSO size threshold | ISO9660 `PSP_GAME`, parsed `PARAM.SFO`, and strong root `UMD_DATA.BIN` evidence | Yes; EmuWiz stronger | No | Yes | None proven | OpenROM’s broad substring and size split are weak | ALREADY COVERED |
| Xbox | XDVDFS magic | `0x10000: MICROSOFT*XBOX*MEDIA` | Same logical sector 32 magic, then bounded XDVDFS traversal, `/default.xbe`, `XBEH` and certificate evidence | Yes, EmuWiz stronger | No | Yes | No platform-specific alternate offset missing from EmuWiz’s reviewed XDVDFS primitive | OpenROM magic is shared with Xbox 360 and does not disambiguate | ALREADY COVERED |
| GameCube | Four-byte header word | BE `0xC2339F3D` at `0x1C` | Same magic/offset, plus `nod` header, ID, partitions/apploader/FST/`main.dol` observations and container handling | Yes, EmuWiz stronger | No | Yes | None proven | Low for exact word; OpenROM lacks structural corroboration | ALREADY COVERED |
| Wii | Four-byte header word | BE `0x5D1C9EA3` at `0x18` | Same magic/offset, plus `nod` Wii partition and data-header structure | Yes, EmuWiz stronger | No | Yes | None proven | Low for exact word; platform conflict is fail-closed in EmuWiz | ALREADY COVERED |

## Genuine gaps only

No confirmed EmuWiz gap meets the requested definition. Every OpenROM primitive that is reliable enough to compare has an EmuWiz equivalent, and in the new four-platform set EmuWiz generally adds the missing topology or structural checks.

Items explicitly *not* counted as gaps:

- OpenROM’s extra Saturn/Sega-CD alternate strings: they are not proven improvements over the reviewed EmuWiz fields; the Saturn `0x10` rule is especially suspect because EmuWiz’s verified System ID places the hardware ID at `0x00`.
- OpenROM’s broad PCE/Neo Geo scans: broader search is not stronger evidence. EmuWiz’s fixed-sector IPL and parsed `IPL.TXT` rules are safer.
- OpenROM’s size fallbacks and CSO/ZSO threshold: these are ambiguity-producing fallbacks, not reliable platform primitives.
- OpenROM’s Xbox `0x2090000` alternate location: EmuWiz does not need to duplicate a raw alternate-offset probe because it has the actual XDVDFS logical-sector primitive and bounded filesystem/XBE path. No source evidence was found that the alternate offset identifies a distinct original-Xbox case EmuWiz otherwise misses.
- OpenROM’s unconditional GDI/XISO/format labels: format identity is not equivalent to platform identity for all shared containers.

If a future implementation task is opened, classifications should remain:

- `CONFIRMED_IDENTITY`: none newly justified by OpenROM.
- `SUPPORTING_EVIDENCE`: none newly justified; existing EmuWiz evidence already covers the useful observations.
- `HEURISTIC_ONLY`: OpenROM’s first-window scans, size thresholds, and extension overrides.
- `DO_NOT_ADOPT`: Saturn-at-`0x10` as a platform proof, broad PCE/Neo Geo substring scans, and generic Xbox magic as original-Xbox proof.

## CHD routing and media topology

OpenROM’s `Converter._to_chd` routes:

- `GDI`, `CUE`, `BIN`, and `CDI` to `chdman createcd`.
- `ISO` and `IMG` to `createcd` only for platform labels exactly `PS1` or `Dreamcast`; every other platform, including Saturn, Sega CD, Neo Geo CD, and PC Engine CD, goes to `createdvd`.
- Any other input falls through to `createcd`.

Its command templates independently advertise `ISO -> CHD` as `createdvd`, `GDI -> CHD` as `createcd`, and `IMG -> CHD` as `createdvd`. This is format/platform routing, not a proven media-topology decision. In particular, a correctly detected Saturn or Sega CD ISO will still be sent to `createdvd`; an unrecognised PS1/Dreamcast ISO will also be sent to `createdvd`. OpenROM’s `chd_type` reader is only for CHD extraction and uses v5 `unitbytes` at `0x3C` (`2448` CD, `512/2048/4096` DVD) or v4 flags at `0x10`, with a size fallback.

EmuWiz’s reviewed CHD path is materially more conservative:

- `chd_identity.rs` parses CHD v5 headers and metadata tags (`CHTR`/`CHT2`/`CHSE` CD-ROM, `CHGD`/`CHGT` GD-ROM, `DVD `, hard disk, laserdisc), retaining `CdRom` versus `GdRom` rather than guessing platform.
- `select_candidate_data_track` excludes audio and selects by track metadata. A GD-ROM high-density boundary at frame/LBA `45000` is detected by `needs_specialist_optical_backend`.
- `open_chd_iso9660` refuses specialist multi-track GD-ROM layouts instead of reading the low-density warning track as if it were the game. `chd_optical_specialist` can read high-density GD data by absolute LBA when that optional feature is enabled; default builds fail closed.
- `game_identity::inspect_disc_chd` routes PS1, Saturn, Sega CD, Neo Geo CD, PS2, PC Engine CD, 3DO, and Dreamcast through platform-specific decoded-media inspection. Dreamcast GD-ROM CHDs use the specialist branch; no silent low-density fallback is allowed.
- `ingestion/gdi::resolve_gdi_data_track` requires a validated data track at or beyond the GD high-density boundary and refuses ambiguous/unsafe layouts.
- `repair/optical_conversion.rs` is a deliberately narrow verified CUE/BIN-to-CHD path and uses `createcd`; it does not present a generic ISO-to-CHD mode that could silently choose DVD for CD media.

Finding: no wrong `createcd`/`createdvd` choice was found in the EmuWiz paths inspected. The real routing defect is OpenROM’s generic ISO handling: CD-based platforms other than the two exact labels can plausibly be sent to `createdvd`. This is an OpenROM finding, not an EmuWiz implementation gap. EmuWiz should preserve its fail-closed topology distinction. `GD-ROM` must not be collapsed to ordinary `CD` merely because both use CD-family CHD metadata.

## Licensing and code-use boundary

OpenROM’s repository states GPL v3 and includes a GPL v3 `LICENSE`. Its bundled tools have separate licenses, including MAME/chdman GPL v2 and nodtool MIT; those are not the license of OpenROM’s own detector/converter source.

The GPL permits copying and modification subject to GPL obligations. Reusing OpenROM source in EmuWiz would therefore require a proper derivative-work and licensing review, preservation of notices, and compliance with GPL v3 distribution/source obligations; it should not be copied casually into EmuWiz. The recommended boundary is independent implementation of independently verified, publicly documented format signatures and concepts. A byte signature or format fact is not itself OpenROM source code, but copied code structure, comments, tests, or non-trivial expression would require attribution and license review.

## Proposed fixture-style test vectors

These are research vectors only; no tests were added.

| Primitive | Synthetic positive | Nearby negative / truncation / conflict |
|---|---|---|
| Saturn System ID | 0x100-byte buffer; bytes `0x00..0x0F = "SEGA SEGASATURN"` padded; optional product at `0x20` | Change one byte at `0x00`; truncate to `0xFF`; place the string only at `0x10` and expect no confirmed Saturn evidence. For conflict, add a Sega-CD string elsewhere: Saturn parser must retain only its fixed-field result. |
| Sega CD | 0x200-byte buffer beginning `SEGADISCSYSTEM`; product field at `0x180` containing printable `GM T-12345-00` with documented spacing | `SEGADISCSYSTEM` at `0x10` only must not satisfy EmuWiz’s fixed offset; truncate before `0x200`; malformed/non-printable product must yield boot evidence only, not product identity. |
| PC Engine CD | 2048 bytes of sector 0 plus 128-byte record at logical offset `0x800`; record bytes `0x20..0x36 = "PC Engine CD-ROM SYSTEM"`, byte `3 = 1`, valid 24-bit start | Same string at sector 0 or at record offset 31; record with byte 3 = 0; input shorter than `0x800 + 128`; boot span beyond media must fail identity. |
| Neo Geo CD | Root `IPL.TXT` fixture with valid 8.3 entries, hex bank/offset, required loader types, and trailing `0x1A` | File named `IPL.TXT` with only arbitrary text, missing terminator, >32 entries, malformed bank/offset, or `NEO-GEO CD` bytes outside the file: no structural evidence. |
| Dreamcast IP.BIN | 0x100 bytes at logical offset 0; `SEGA SEGAKATANA` in `0x00..0x0F`, product at `0x40` | `SEGA SEGAMARIO` is accepted as the documented alternate; one-byte mutation or <0x100 input fails. A `1ST_READ.BIN` filename without IP.BIN signature remains insufficient. |
| PS1 | ISO9660 fixture with root `SYSTEM.CNF`, `BOOT=cdrom:\SLUS_123.45;1`, referenced file beginning `PS-X EXE` | `BOOT2=` instead of `BOOT=`, missing executable, wrong `PS-X EXE`, truncated CNF, or generic `PlayStation` text in unrelated volume metadata must not verify PS1 identity. |
| PS2 | ISO9660 fixture with `SYSTEM.CNF` `BOOT2=cdrom0:\SLUS_123.45;1`, referenced executable beginning ELF magic | `BOOT=` only, non-ELF executable, missing file, oversized CNF, and a PVD containing `PLAYSTATION` without the boot structure must fail PS2 verification. |
| PSP | Root `PSP_GAME/`, `PSP_GAME/PARAM.SFO`, and `UMD_DATA.BIN`; minimal valid SFO with `DISC_ID` | `PSP_GAME` without UMD file is supporting evidence only; malformed SFO, `UMD_DATA.BIN` in a non-root path, missing directory, or truncated ISO directory must not become strong PSP identity. |
| Xbox | Logical sector 32 at `0x10000` begins `MICROSOFT*XBOX*MEDIA`; bounded XDVDFS root contains `default.xbe` beginning `XBEH` | Change one magic byte; truncate at `0x10000 + 19`; XDVDFS without `default.xbe`; `default.xbe` filename without XBEH. The same XDVDFS fixture must be marked shared-family evidence, not original-Xbox proof alone. |
| Xbox alternate layout | Sparse fixture with the same 20-byte magic at `0x2090000` | Signature at `0x2090000 - 1`; file shorter than the complete signature; conflict with a valid GameCube header must preserve conflict/fail-closed handling rather than first-match assignment. |
| GameCube | At offset `0x1C`, bytes `C2 33 9F 3D`, plus minimal valid header/ID | One-byte mutation; input shorter than `0x20`; Wii magic at `0x18` simultaneously; invalid `nod` container around the bytes. |
| Wii | At offset `0x18`, bytes `5D 1C 9E A3`, plus a minimal structural header/partition fixture | One-byte mutation; input shorter than `0x1C`; GameCube magic at `0x1C` simultaneously; invalid/missing Wii data partition. |
| CHD media topology | Synthetic metadata-only CHD entries using `CHTR` and `CHGD`, with CD/GD track types and cumulative frames crossing `45000` | Audio-only track; malformed metadata chain; mixed CD/GD tags; GD track before but not beyond `45000`; nonzero parent SHA-1. Expected result is explicit class/refusal, never a platform guess. |

## Ranked recommendations

### ADOPT

None from OpenROM source. EmuWiz already has the useful primitives with stronger bounds and context.

### CONSIDER

1. Keep a future fixture corpus aligned with the vectors above, especially conflict and truncation cases for the four new platforms and CHD GD-ROM topology. This is validation work, not a detector change.
2. If a generic ISO-to-CHD operation is later introduced, require explicit media topology (`CD`, `DVD`, or `GD-ROM`) or a proven decoded-media classification before selecting `createcd`/`createdvd`; do not copy OpenROM’s platform-label shortcut.
3. Keep the optional Dreamcast GD-ROM specialist path visibly fail-closed in default builds and continue treating `GdRom` as media evidence, not Dreamcast identity.

### ALREADY COVERED

Saturn, Sega CD/Mega-CD, PC Engine CD/TurboGrafx-CD, Neo Geo CD, Dreamcast, PS1, PS2, PSP, Xbox including logical-sector XDVDFS handling, GameCube, and Wii all have current EmuWiz evidence or identity paths. Current evidence is generally more specific than OpenROM’s detector rules.

### REJECT

- Copying OpenROM’s broad first-window substring scans as confirmed platform identity.
- Adopting Saturn’s `0x10` check or Sega CD’s alternate `SEGA_CD` check without independent format proof and collision tests.
- Adding size-based PS1/PS2/PSP/Xbox fallbacks to authoritative identity.
- Treating XDVDFS magic as original-Xbox proof or treating a GD-ROM CHD as ordinary CD media.
- Reusing OpenROM implementation code without a GPL/source-licensing review.

## Final audit result

- Strongest candidate improvement: none confirmed; the most useful future hardening is topology-gated CHD routing if EmuWiz ever adds a generic ISO-to-CHD path.
- Highest false-positive-risk rule: OpenROM’s size fallback and broad PS1/PSP/PC Engine/Neo Geo substring checks; among fixed rules, Saturn `SEGA SEGASATURN` at `0x10` is the most questionable.
- CHD routing finding: OpenROM can plausibly send CD-based ISO platforms such as Saturn or Sega CD to `createdvd`; EmuWiz’s inspected paths preserve CD/GD/DVD distinctions and fail closed for unsupported GD layouts.
- License finding: OpenROM own source is GPL v3; independently reimplement documented signatures and concepts, and do not copy source without GPL compliance review.
- Resulting commit SHA: to be recorded only if this research document is committed.

