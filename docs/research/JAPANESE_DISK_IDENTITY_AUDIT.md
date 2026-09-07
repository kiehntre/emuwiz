# Japanese Computer Disk Identity Audit V1

Status: research-only, 2026-09-07

This audit describes the current authoritative tree after the narrow PC-98
container wiring follow-up. It does not add a new container parser, a
filename/geometry platform guess, or a launch adapter.

## Identity layers

These are separate claims:

1. **Container** — the bytes form D88, HDI, NHD, XDF, or DIM.
2. **Geometry** — cylinders, heads, sectors, sector size, and track layout.
3. **Filesystem** — a readable FAT/PC-98/Human68k/TownsOS directory and allocation structure.
4. **Boot** — a machine-specific IPL, boot sector, or boot record.
5. **Machine family** — PC-88, PC-98, X68000, or FM Towns.
6. **Exact software** — title/release/media set, normally from a catalogue or verified DAT.
7. **DAT/hash** — byte identity against an authoritative DAT/hash source.

A valid container is never treated as proof of the software title. An extension is dispatch input for a bounded parser, not identity evidence.

## Current implementation

The authoritative tree has:

- `disk_format/d88.rs`: bounded D88 header, track-table, sector-header and data-length validation. It records disk name, write-protect and media bytes, but does not walk a filesystem or boot sector.
- `disk_format/hdi.rs`: bounded Anex86 HDI and T98-Next NHD header/geometry validation. It intentionally reads no filesystem or boot bytes.
- `disk_format/x68000.rs`: bounded XDF 2HD validation (exact 77-cylinder/2-head/8-sector/1024-byte geometry plus an X68000 IPL/BPB shape) and DIM/DIFC header/track validation.
- `ingestion/discovery.rs`: D88 and HDI/NHD remain accepted only when independent folder/platform evidence supplies a machine identity. XDF/DIM require validated structure and an available platform identity in the discovery path; no filename-only acceptance is made.
- `platform/mod.rs`: canonical `NEC PC-8801`, `PC-98`, legacy `NEC PC-9801`, `Sharp X68000`, and `FM Towns` records. FM Towns has shared optical/floppy extensions only; no family-specific detector is registered.
- `content_registry.rs`: D88/HDI/NHD/XDF/DIM are `ComputerDisk` content extensions. This is content routing, not platform proof.
- `coverage_inventory.rs`: X68000 XDF/DIM are synthetic-validated; no real specimen is recorded as validated in this workspace.
- `pc98_boot_evidence.rs`: bounded logical 512-byte BPB inspection. A coherent
  FAT BPB with an `NEC` OEM marker is reported as strong PC-98 evidence;
  generic FAT remains explicitly generic. This is an evidence primitive, not a
  replacement D88/HDI/NHD parser or an exact software identifier.
- `pc98_container_evidence.rs`: consumes validated D88 track mapping and
  declared HDI/NHD payload geometry to read at most one bounded logical boot
  sector, then delegates interpretation to the primitive above. Only strong
  boot evidence becomes a PC-98 platform observation; generic FAT and missing
  sectors remain non-identifying.
- `x68000_human68k.rs`: bounded logical-sector evidence for the X68000 IPL
  branch (`0x60`) plus the documented 1024-byte Human68k BPB geometry. It
  emits typed boot/filesystem facts and shared content observations only for
  the strong combination; generic FAT remains non-identifying and exact title
  identity remains DAT/hash-led.

The existing media ledger correctly calls this area partial: structural format support exists, while PC-98/X68000 filesystem evidence and launch remain gaps (`docs/MEDIA_SUPPORT_AUDIT.md`). The older specialized branch `feature/x68000-xdf-dim-evidence` (commits `988b1ad`/`a821845`) is now represented in the authority; it is useful archaeology, not a reason to add a second implementation.

## Evidence matrix

`Strong` means the bytes themselves are distinctive enough for that layer. `Corroborated` means a structural result plus independent folder/catalogue/boot evidence. `Family-only` means a useful ecosystem narrowing, not a machine assignment. The final column is the safe V1 classification for an image without a title DAT match.

| Platform / format | Container | Geometry | Filesystem | Boot | Machine-family evidence | Exact software | DAT/hash | Safe V1 result |
|---|---|---|---|---|---|---|---|---|
| PC-88 + D88 | **Strong** D88 | **Strong** per-track C/H/R/N records | Not inspected | Not inspected | D88 is shared; valid D88 + `pc88`/PC-88 folder is corroboration only | No | Required for title/release | **CORROBORATED_PLATFORM** with independent PC-88 folder; otherwise **FAMILY_ONLY** |
| PC-98 + D88 | **Strong** D88 | **Strong** per-track geometry | Not inspected | **Strong** when validated track 0/0/1 carries NEC FAT BPB | Shared with PC-88, FM Towns and X68000; strong boot evidence is bytes-derived, generic FAT remains shared | No | Required for exact release | **STRONG_PLATFORM** with NEC boot; otherwise **FAMILY_ONLY** |
| FM Towns + D88 | **Strong** D88 | **Strong** | Not inspected | Not inspected | D88 is shared and does not identify Towns; folder or title/catalogue evidence is required | No | Required | **CORROBORATED_PLATFORM** only with independent Towns evidence; otherwise **FAMILY_ONLY** |
| X68000 + D88 | **Strong** D88 | **Strong** | Human68k candidate when a mapped logical sector is supplied | **Strong** with IPL/BPB evidence | D88 is shared; only mapped Human68k bytes strengthen X68000; geometry alone remains family-only | No | Required | **STRONG_PLATFORM** with Human68k bytes; otherwise **FAMILY_ONLY** |
| PC-98 + HDI | **Strong** HDI header | **Strong** C/H/S/sector-size fields | Not inspected | **Strong** when declared payload sector 0 carries NEC FAT BPB | Header/geometry alone remains shared; strong boot bytes are required | No | Required | **STRONG_PLATFORM** with NEC boot; otherwise **FAMILY_ONLY** |
| PC-98 + NHD | **Strong** `T98HDDIMAGE.R0` header | **Strong** C/H/S/sector-size fields | Not inspected | **Strong** when declared payload sector 0 carries NEC FAT BPB | Header/geometry alone remains shared; strong boot bytes are required | No | Required | **STRONG_PLATFORM** with NEC boot; otherwise **FAMILY_ONLY** |
| X68000 + HDI/NHD | **Strong** container if header validates | **Strong** | Human68k candidate when a mapped logical sector is supplied | **Strong** with IPL/BPB evidence | HDI/NHD geometry alone remains shared; mapped Human68k bytes are required for a strong conclusion | No | Required | **STRONG_PLATFORM** with Human68k bytes; otherwise **FAMILY_ONLY** |
| X68000 + XDF | **Strong** raw-layout validation | **Strong** 77×2×8×1024 | BPB shape only; no directory walk | **Strong-ish** X68000 IPL branch opcode plus BPB constraints | The combination is a strong X68000 floppy signature in the parser, but discovery still requires independent platform identity and the raw image has no self-describing container | No | Required for title/release | **CORROBORATED_PLATFORM** operationally; parser evidence is candidate **STRONG_PLATFORM** |
| X68000 + DIM | **Strong** DIFC header and track map | **Strong** media-specific geometry | Human68k candidate when a mapped logical sector is supplied | **Strong** with IPL/BPB evidence | DIM identifies an X68000-oriented container; filesystem bytes are still inspected through the shared sector mapping | No | Required | **STRONG_PLATFORM** with Human68k bytes; otherwise **CORROBORATED_PLATFORM** |
| FM Towns + ISO/CUE/BIN/CHD | Generic optical container | Track/sector geometry only | ISO may be readable, but ISO is not Towns identity | No Towns detector currently | Shared with many machines; Towns folder/catalogue/boot evidence is required | No | Required | **FAMILY_ONLY** / **DAT_HASH_REQUIRED** |
| FM Towns + HDM/raw floppy | Generic/raw or shared Japanese floppy | Geometry may be recoverable | No TownsOS/FAT traversal currently | No Towns boot detector currently | Extension and geometry collide with PC-98-compatible media | No | Required | **FAMILY_ONLY** |

The matrix intentionally does not turn the `DiskFormat::platform()` convenience labels (`NEC PC-8801`, `PC-98`, or `Sharp X68000`) into unconditional identity. The detection and discovery gates are the authority: shared D88/HDI/NHD results are suppressed until corroborating evidence exists.

## Format-specific findings and collision tests

### D88

The D88 header contains a disk name/comment, media flag, declared image size and up to 164 track offsets; each track contains sector headers with cylinder/head/record/size and status fields. The format specification explicitly permits multiple disks concatenated after the declared size. Therefore the parser can prove a coherent D88 container and geometry, but disk name is provenance, not a title or platform signature. D88 is used by PC-88, PC-98, FM Towns and X68000 tooling. A bare valid D88 is consequently **FAMILY_ONLY**, not PC-88.

### HDI and NHD

HDI provides a small geometry/type header; NHD provides the `T98HDDIMAGE.R0` signature, comment, header size and C/H/S geometry. Neither adapter reads the payload's partition table, BPB, root directory, or boot code. A valid HDI/NHD is therefore a hard-disk container plus geometry, not proof of PC-98, X68000, DOS, or a release. A PC-98 folder/DAT/hash can corroborate it; capacity and extension cannot.

### XDF and DIM

XDF is headerless raw media. Current validation correctly rejects a same-sized random file unless its first sector has the expected X68000 IPL/BPB fields. That is materially stronger than an `.xdf` suffix, but it still does not identify a title or provide a filesystem walk. DIM's `DIFC HEADER` and bounded media/track map provide a structured X68000-oriented container and geometry; its comments/labels are not identity authority. Keep a separate raw-image/DIM collision test in future work: PC-98-like 2HD geometry alone must not pass the XDF gate.

### FM Towns

FM Towns software commonly uses CD media as well as Japanese-compatible floppy formats. Towns system software can boot directly from CD; the current optical path still provides only generic disc evidence. Towns hard-disk images likewise need partition/boot evidence; a generic ISO/FAT result is not enough.

V1 now implements a bounded first-logical-sector IPL4 probe over the already
validated D88, HDI and NHD container layouts. The exact `IPL4` signature at
offset zero and a valid x86 short/near transfer are required for strong FM
Towns platform evidence. BPB fields are retained as corroborative filesystem
facts only; generic FAT remains generic. For contiguous HDI/NHD payloads, the
boot-declared IO.SYS range is checked without reading the range, and a bounded
`FBIOS` prefix probe may mark TownsOS as a candidate. No title is inferred.
Raw direct floppy access, partition traversal, optical IPL/system-volume
bridging, and launch readiness remain follow-up work.

### Filesystem and boot layers

PC-88/PC-98 media may contain FAT-like layouts, but FAT12/FAT16/BPB geometry is shared and does not settle the machine family. The bounded Human68k bridge now recognises only the X68000 IPL branch plus coherent 1024-byte BPB shape from a caller-supplied mapped sector. It does not walk directories, infer titles, or scan partitions. FM Towns/TownsOS remains covered by its separate IPL4 bridge.

## Exact gaps

1. There is no bounded sector/filesystem reader for PC-98/PC-88 (including FAT variants and non-filesystem/protected disks).
2. The reusable PC-98 BPB evidence primitive is now wired to validated D88,
   HDI and NHD layouts. It intentionally does not scan un-declared partitions,
   add `fdi`/`hdm`/`hd5`/`hd4` variants, or infer PC-98 from generic FAT.
3. Human68k filesystem evidence is now a bounded boot/BPB primitive for mapped sectors; partition traversal, FAT/18.3-directory walking, and HDD sector wiring remain unresolved.
4. FM Towns IPL4 evidence is resolved for the bounded D88/HDI/NHD V1 path;
   Towns-specific optical/system-volume evidence, raw direct floppy access,
   partition traversal, and launch readiness remain unresolved.
5. No Japanese-family DAT/hash normalisation bridge turns a validated disk plus a known catalogue into exact software identity.
6. D88 multi-disk boundary handling is not exposed as a first-class per-disk identity object; the format documentation warns that concatenation may only be inferred from the declared size versus file length.

## Recommended implementation order

1. **Extend PC-98 filesystem evidence:** keep the new bounded boot bridge and
   add partition/filesystem traversal only when real specimens establish safe
   offsets and collision tests; do not weaken the current boot gate.
2. **X68000 Human68k evidence:** inspect the already validated XDF/DIM payload for IPL, partition marker, BPB, and bounded root entries; add equivalent HDD evidence only after real specimens and a collision corpus are available.
3. **FM Towns optical/raw/HDD expansion:** extend the bounded IPL4/TownsOS
   evidence to independently validated raw floppy and optical/system-volume
   paths; do not infer Towns from ISO, D88, or geometry alone.
4. After those bridges, add DAT/hash identity and launch-profile wiring. Exact software identity should remain DAT/hash-driven even when platform evidence is strong.

## Top 3 real implementation opportunities

1. **PC-98 D88/HDI/NHD sector-and-boot evidence** — highest leverage because current containers and geometry already exist, while discovery must still rely on folders.
2. **X68000 Human68k payload inspection** — extend the existing XDF/DIM gate without replacing it; prove partition/boot/filesystem evidence and preserve fail-closed collisions.
3. **FM Towns IPL4/TownsOS evidence bridge** — add a bounded detector for floppy/CD/HDD boot records, validated against independent Towns specimens before changing classification.

## References

- D88 structure and its shared emulator use: [PC98.org D88 format notes](https://www.pc98.org/project/doc/d88.html).
- NHD header and C/H/S layout: [PC98.org NHD format notes](https://www.pc98.org/project/doc/nhd.html).
- DIM structure and X68000 implementation cross-checks already used by the code: [PC98.org DIM notes](https://www.pc98.org/project/doc/dim.html), [XEiJ `FDMedia`](https://stdkmd.net/xeij/source/xeij-FDMedia.java.htm), and [XDF builder notes](https://github.com/mikewolak/x68k_sprite_demo/blob/main/README.md).
- Human68k SxSI partition/boot layout: [erique/scsitools](https://github.com/erique/scsitools).
- FM Towns IPL4 boot-sector observation: [YSFLIGHT FM Towns bootloader notes](https://ysflight.in.coocan.jp/FM/towns/bootloader/e.html).
- FM Towns emulator/boot media context: [MAME FM Towns driver guide](https://wiki.mamedev.org/index.php?title=Driver%3AFMTowns).
- FM Towns ROM IPL4 and bounded IO.SYS boot flow: [Joe’s FM Towns boot article](https://duriansoftware.com/joe/how-the-fm-towns-boots-from-cd-rom).
- IPL4 sector shape and the following x86 jump: [OS/2 Museum FM Towns/FMR analysis](https://www.os2museum.com/wp/the-answer-to-0x49-fujitsu-fmr/).

No source above is used to claim exact game identity. DAT/hash verification remains the authority for that layer.
