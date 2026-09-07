# EmuWiz master media capability audit (V1)

**Authority and date.** This ledger was audited against the authoritative local
tree at `f892e15fb712076540d38ee118d4b31278348ac0` on branch
`feature/archivefs-unified-platform`, including its uncommitted files. Local
code wins over older remote history. The GitHub remote is
`kiehntre/emuwiz`; its fetched `origin/*` branches, commit history, legacy
branches, and research documents were searched for media/container terms.
The fetched `origin/feature/archivefs-unified-platform` currently stops at
`28e5606`, so several later local commits are intentionally recorded as local
authority. The public web repository page was not available to the browsing
cache; no conclusion below relies on that failure.

## Executive summary

- **Media families audited:** 28 (cartridges/ROMs, floppy/disk, tape, optical,
  LaserDisc sets, Amiga packages, hard-disk images, archives, executable and
  firmware media).
- **Individual formats/variants audited:** 68 (including explicitly deferred
  formats so they cannot be reassigned as if forgotten).
- **Mature/user-facing families:** standard loose-ROM ingestion, common
  archives, WHDLoad installs, major optical identity paths, and launch-ready
  emulator profiles where listed below.
- **Backend-only families:** most structural disk parsers, tape waveform
  decoders, CHD logical reading, CD-i evidence, and LaserDisc set verification.
- **Research-only families:** several Apple and Japanese disk variants, raw
  preservation formats without parsers, and unsupported UEF/CAS chunk families.
- **Genuinely unsupported:** MP3/FLAC/OGG tape decoding, raw LaserDisc RF,
  unsupported archive codecs, many modern package formats, and unimplemented
  disk/optical variants listed as `NONE`.
- **Largest roadmap discoveries:** WHDLoad slave parsing, Amiga HDF traversal,
  Atari STX/Pasti, Acorn DFS, PC Engine CD, tape waveform families, CD-i, and
  LaserDisc verification already exist in local history or current code.
- **Highest-value real gaps:** broader disk geometry/filesystem inspection,
  GUI surfacing for structural media evidence, CHD/DAT logical parity, missing
  Amiga launch command planning, and deferred container families beyond the
  bounded UEF/CAS bridge.

## Capability and maturity keys

Capabilities are deliberately atomic: `NONE`, `RESEARCH_ONLY`, `DETECT`,
`PARSE`, `IDENTIFY`, `VERIFY`, `INSPECT_CONTENTS`, `EXTRACT`, `CONVERT`,
`REPAIR`, `DAT_MATCH`, `GUI`, `LAUNCH`, and `READINESS`. A cell lists only
capabilities proven by current code/tests; an absent capability is not implied
by a platform registry row.

Maturity: **0** none; **1** research only; **2** detect only; **3** structural
parsing; **4** verified identity/integrity; **5** user-facing GUI/workflow;
**6** full product workflow (safe readiness and launch/apply path).

## Capability matrix

| Family | Format | Detect | Parse | Identify | Verify | Inspect | DAT | GUI | Launch | Readiness | Maturity | Main module | Tests / evidence | Remaining gap |
|---|---|---|---|---|---|---|---|---|---|---|---:|---|---|---|
| Nintendo | NES/iNES/UNF | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `nes_header_evidence`, `cartridge_header` | `platform/tests.rs`, header tests | FDS/modern variants remain separate |
| Nintendo | SNES/SFC/SMC | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `snes_header_evidence`, `smd_normalization` | header and normalization tests | copier/mapper edge cases |
| Nintendo | N64 z64/v64/n64 | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `n64_header_evidence`, `n64_byte_order`, `n64_cic_evidence` | N64/CIC tests | broader CIC corpus |
| Nintendo | GB/GBC | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `gb_header_evidence` | header tests | none material for current scope |
| Nintendo | GBA | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `gba_header_evidence` | header tests | none material |
| Nintendo | DS `.nds` | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `cartridge_header`, DS launch adapter | cartridge/launch tests | 3DS content is separate |
| Nintendo | 3DS | EXT | NO | DAT | NO | NO | YES | SUMMARY | YES | YES | 4 | platform registry, Azahar launch | launch tests | CIA/3DS structural parser |
| Nintendo | FDS `.fds` | YES | YES | FAMILY | YES | YES | YES | SUMMARY | YES | YES | 6 | `disk_format/fds` | disk-format tests | richer filesystem view |
| Nintendo | Virtual Boy `.vb/.vboy` | YES | NO | DAT | NO | NO | YES | SUMMARY | YES | YES | 4 | `content_registry` | negative/header tests | header identity |
| Nintendo | GameCube `.gcm/.iso/.rvz/.wbfs` | YES | YES | YES | YES | YES | REDUMP/MAME | SUMMARY | YES | YES | 6 | `gamecube_wii_boot_evidence`, `disc_evidence_collector` | optical/launch tests | deeper FST parity |
| Nintendo | Wii `.wbfs/.wia/.rvz` | YES | YES | YES | YES | YES | REDUMP/MAME | SUMMARY | YES | YES | 6 | same shared optical + Dolphin | launch tests | WIA details |
| Nintendo | Wii U `.wud/.wux` | EXT | NO | DAT | NO | NO | NO | SUMMARY | NO | NO | 2 | platform registry | registry tests | WUD/WUX reader |
| Sega | Master System/Game Gear | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `sms_gg_header_evidence` | header tests | mapper edge cases |
| Sega | Mega Drive/Genesis `.md/.gen/.smd` | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `megadrive_header_evidence`, `smd_normalization` | header/normalization tests | none material |
| Sega | 32X | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `sega32x_header_evidence` | header tests | none material |
| Sega | SG-1000 | EXT | NO | DAT | NO | NO | YES | SUMMARY | YES | YES | 4 | registry | negative tests | header semantics |
| Sega | Saturn | YES | YES | YES | YES | YES | REDUMP | SUMMARY | YES | YES | 6 | `saturn_boot_evidence`, raw-sector stack | real Athlete Kings + synthetic tests | specialist tracks |
| Sega | Mega CD/Sega CD | YES | YES | YES | YES | YES | REDUMP | SUMMARY | YES | YES | 6 | `segacd_boot_evidence` | synthetic boot tests | more real-corpus validation |
| Sega | Dreamcast/GDI/CHD | YES | YES | YES | YES | YES | REDUMP | SUMMARY | YES | YES | 6 | `dreamcast_boot_evidence`, CHD specialist | boot/CHD tests | GD multi-track parity |
| NEC | PC Engine/TurboGrafx HuCard | EXT | NO | DAT | NO | NO | YES | SUMMARY | YES | YES | 4 | registry | platform tests | HuCard header parser |
| NEC | PC Engine CD/TG-CD | YES | YES | YES | YES | YES | REDUMP | SUMMARY | YES | YES | 6 | `pcengine_cd_boot_evidence` | IPL tests | more track/session parity |
| NEC | PC-FX | YES | YES | YES | YES | YES | REDUMP | SUMMARY | YES | YES | 6 | `pcfx_boot_evidence` | boot-sector tests | real samples |
| SNK | Neo Geo cartridge | EXT | NO | DAT | SET | SET | YES | SUMMARY | YES | YES | 5 | registry/DAT, `dat::set`, `dat::dependency`, `dat::neogeo_set` | platform tests, `neogeo_set` tests | none material by design (multi-ROM set identity, not a single-file header - see `docs/research/STRANGE_CARTRIDGE_IDENTITY_AUDIT.md`); MVS/AES mode is deliberately not forced from set contents alone |
| SNK | Neo Geo CD | YES | YES | YES | YES | YES | REDUMP | SUMMARY | YES | YES | 6 | `neogeocd_boot_evidence` | IPL tests | more real media |
| SNK | Neo Geo Pocket/Color | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `ngp_header_evidence` | header tests | none material |
| Bandai | WonderSwan/Color | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `ws_header_evidence` | footer/header tests | none material |
| Atari | 2600/5200/7800 | YES | PARTIAL | DAT | PARTIAL | NO | YES | SUMMARY | YES | YES | 4 | `atari7800_header_evidence`, registry | header tests | 2600/5200 semantics |
| Atari | Lynx | YES | YES | YES | YES | NO | YES | SUMMARY | YES | YES | 6 | `lynx_header_evidence` | header tests | none material |
| Atari | Jaguar | EXT | NO | DAT | NO | NO | YES | SUMMARY | NO | NO | 2 | registry | negative tests | Jaguar CD research |
| Atari | 8-bit ATR/XFD | YES | YES | FAMILY | YES | YES | YES | SUMMARY | YES | YES | 6 | `disk_format`, `atari_tape` | disk/tape tests | richer filesystem |
| Atari ST | ST/STA raw FAT12 | YES | YES | PLAUSIBLE | PARTIAL | YES | YES | SUMMARY | YES | PARTIAL | 4 | `disk_format/atari_st` | geometry tests | platform identity ambiguity |
| Atari ST | MSA | YES | YES | FAMILY | YES | YES | YES | SUMMARY | YES | YES | 4 | `disk_format` | disk tests | fuller sector inspection |
| Atari ST | STX/Pasti | YES | YES | YES | YES | NO | YES | SUMMARY | NO | NO | 4 | `disk_format/atari_stx` | Pasti bounds tests | preservation semantics |
| Amiga | ADF/ADZ | YES | YES | YES | YES | YES | TOSEC | SUMMARY | YES | YES | 6 | `amiga_disk`, `amiga_adz`, `discovery` | ADF discovery tests, ADZ decompression tests | none material for ADF; ADZ launch-input wiring remains ADF-only |
| Amiga | HDF/RDB | YES | YES | YES | YES | YES | TOSEC | SUMMARY | YES | PARTIAL | 5 | `amiga_disk`, HDF traversal | HDF tests | richer filesystem/launch |
| Amiga | DMS/IPF/HFE/SCP | EXT | NO | NO | NO | NO | TOSEC | NO | NO | NO | 1 | research docs | NO TEST COVERAGE FOUND | parsers and preservation model |
| Amiga | WHDLoad directory/package | YES | YES | YES | YES | YES | TOSEC/WHDLoad | SUMMARY | PROJECT | YES | 6 | `identity_source/whdload`, `amiga_whdload_local`, `amiga_whdload_archive` | slave/discovery/launch projection tests, archive-member slave tests | no Amiga command executor |
| Amiga | LHA/LZH | YES | LIST | NO | SLAVE | MEMBERS | TOSEC | SUMMARY | NO | NO | 4 | archive inspector, `dat/archive/lha`, `amiga_whdload_archive` | archive tests, WHDLoad archive-member tests | exact DAT-hash identity for archive-embedded slaves; extraction of non-slave content |
| Amiga | CD32/CDTV optical | YES | ISO | FAMILY | PARTIAL | YES | REDUMP | SUMMARY | PARTIAL | PARTIAL | 3 | shared optical stack | generic optical tests | Amiga-specific boot evidence |
| Commodore | C64/C128 D64/D71/D81/G64/CRT | YES | YES | FAMILY | YES | YES | TOSEC | SUMMARY | YES | YES | 6 | `disk_format/d64`, CRT | disk-format tests | G64/NIB preservation depth |
| Commodore | C64/T64/TAP | YES | YES | YES | YES | YES | TOSEC | SUMMARY | YES | YES | 6 | `commodore_tape`, `tape_analysis` | tape tests | custom waveform remains generic |
| Amstrad | CPC DSK/EDSK/CDT | YES | YES | FAMILY | YES | YES | TOSEC | SUMMARY | YES | YES | 6 | `disk_format/dsk`, CPC tape | DSK/CPC WAV tests | protected-sector semantics |
| BBC/Acorn | DFS SSD/DSD | YES | YES | FAMILY | YES | YES | TOSEC | SUMMARY | YES | YES | 6 | `disk_format/dfs` | DFS tests | ADFS parser |
| BBC/Acorn | UEF tape | EXT | NO | FAMILY | YES | YES | TOSEC | SUMMARY | NO | YES | 4 | `uef_tape`, `tape_analysis` | synthetic UEF tests | unsupported/bit-level chunks remain bounded |
| Apple II | DO/PO/DSK | YES | YES | FAMILY | YES | SUMMARY | TOSEC | SUMMARY | NO | PARTIAL | 4 | `apple2_disk` | synthetic DOS 3.3/ProDOS/negative tests | broader catalogue/filesystem and launch |
| Apple II | 2MG | YES | YES | FAMILY | PARTIAL | SUMMARY | TOSEC | SUMMARY | NO | PARTIAL | 3 | `apple2_disk` | bounded header/range tests | recursive payload identity |
| Apple II | WOZ/NIB | YES | YES | NO | PARTIAL | SUMMARY | TOSEC | SUMMARY | NO | NO | 3 | `apple2_disk` | signature/map/geometry tests | flux/GCR interpretation |
| Macintosh | DC42/HFV/SIT | YES | PARTIAL | FAMILY | PARTIAL | NO | TOSEC | SUMMARY | NO | NO | 3 | `disk_format/dc42`, registry | DC42 tests | HFS/SIT depth |
| Japanese PCs | D88/HDI/NHD/XDF/DIM | YES | YES | FAMILY | YES | YES | TOSEC | SUMMARY | PARTIAL | PARTIAL | 5 | `disk_format/d88/hdi/x68000`, `pc98_boot_evidence`, `pc98_container_evidence`, `fmtowns_boot_evidence`, `fmtowns_container_evidence` | disk-format + PC-98/FM Towns boot-evidence tests | exact titles, filesystem/launch; raw/optical Towns paths and non-PC-98 Japanese media remain scoped separately |
| DOS/PC | IMG/IMA/RAW FAT12/16 | YES | YES | FAMILY | YES | YES | TOSEC | SUMMARY | YES | YES | 6 | `dos_boot_evidence`, `disk_format` | DOS boot tests | IMD/TD0/DMF |
| ZX Spectrum | TAP/TZX | YES | YES | YES | YES | YES | TOSEC | DETAILED | YES | YES | 6 | `tape_identity`, `tape_analysis` | TZX semantic tests | named families only Alkatraz |
| ZX Spectrum | WAV ROM/custom | YES | YES | YES | YES | YES | NO | DETAILED | NO | YES | 5 | `tape_audio`, `tape_analysis` | synthetic WAV tests | broader custom loaders |
| BBC Micro | WAV standard/custom | YES | YES | YES | YES | YES | NO | DETAILED | NO | YES | 5 | `bbc_tape` | BBC waveform tests | named/custom semantics deferred |
| MSX | WAV standard/custom | YES | YES | YES | YES | YES | NO | DETAILED | NO | YES | 5 | `msx_tape` | MSX waveform tests | no CAS parser; named loaders deferred |
| Atari 8-bit | WAV standard/custom | YES | YES | YES | YES | YES | NO | DETAILED | NO | YES | 5 | `atari_tape` | Atari WAV tests | broader formats |
| Dragon/CoCo | CAS/WAV | EXT | NO | FAMILY | YES | YES | TOSEC | SUMMARY | NO | YES | 4 | `dragon_coco_tape`, `tape_analysis` | synthetic CAS tests | ordinary blocks only; turbo/custom deferred |
| Optical containers | ISO | YES | YES | NO alone | PARTIAL | YES | REDUMP | SUMMARY | PROFILE | PARTIAL | 4 | `iso9660`, collector | ISO tests | platform-specific boot breadth |
| Optical containers | CUE/BIN, multi-BIN | YES | YES | NO alone | PARTIAL | YES | REDUMP | SUMMARY | PROFILE | PARTIAL | 4 | `ingestion/cue_bin`, raw media | CUE tests | session/subchannel parity |
| Optical containers | CHD | YES | YES | NO alone | PARTIAL | YES | MAME/REDUMP | SUMMARY | PROFILE | PARTIAL | 5 | `chd_identity`, `chd_logical_media`, `chd_redump` | CHD/redump track tests | specialist layouts and broader multi-track parity |
| Optical containers | GDI/CDI/CCD/IMG/MDS/NRG/CSO/RVZ/WBFS/WUD/WUX | EXT | PARTIAL | FAMILY | PARTIAL | NO | PARTIAL | SUMMARY | PROFILE | PARTIAL | 3 | registry + selected adapters | extension/negative tests | per-container readers |
| Philips CD-i | ISO/BIN/CHD logical media | YES | YES | YES | YES | YES | REDUMP | SUMMARY | NO | PARTIAL | 4 | `cdi_disc_evidence`, `platform` | CD-i synthetic tests | raw Mode 2/session parity |
| LaserDisc sets | Daphne framefile | YES | YES | SET | VERIFY | YES | NO | NO | PROFILE | YES | 4 | `laserdisc_set` | synthetic set tests | video metadata/frame-range checks |
| LaserDisc sets | Hypseus/Singe | YES | PARTIAL | SET | PARTIAL | YES | NO | NO | PROFILE | PARTIAL | 3 | `laserdisc_set` | synthetic set tests | script semantics |
| LaserDisc sets | MAME LD | PARTIAL | NO | NO | NO | NO | MAME | NO | PROFILE | PARTIAL | 2 | `laserdisc_set` config marker | limited tests | software-list integration |
| Archives | ZIP/7z/RAR/TAR/GZIP/BZ2/XZ | YES | LIST | NO | SAFETY | MEMBERS | NO | SUMMARY | NO | NO | 3 | `inspector`, archive resolver | archive/member tests | extraction policy varies; encrypted archives |
| Packages | local `emuwiz.mod.json` | YES | YES | ID requirement | VERIFY | YES | NO | GUI | NO | YES | 5 | `mod_package` | package safety tests | apply is explicit separate workflow |
| Executables | ELF/XBE/XEX/SELF/EBOOT/PBP/PKG | YES | PARTIAL | FAMILY | PARTIAL | NO | DAT | SUMMARY | PROFILE | PARTIAL | 4 | `executable_signatures`, `param_sfo`, `psp_pbp_evidence` | signature tests | package-specific parsers |
| Firmware | BIOS/Kickstart/System Card/PARAM.SFO | YES | YES | VERSION | HASH/PARTIAL | NO | DAT | Doctor | PROFILE | YES | 5 | `dat/firmware_evidence`, firmware adapters | firmware tests | broader BIOS catalogue/readiness |
| Preservation | raw 2352 CD sectors/subchannel | YES | PARTIAL | NO | PARTIAL | NO | REDUMP | NO | NO | NO | 3 | `raw_cd_sector`, `raw_cd_logical_media` | sector tests | subchannel/weak-sector fidelity |
| Preservation | G64/NIB/WOZ/IPF/STX/SCP/flux | EXT | NO/PARTIAL | NO | NO | NO | TOSEC | NO | NO | NO | 1–2 | registry/research docs | format-specific only | preservation parsers |

## Already implemented — do not assign again without new evidence

Verified in the current tree and/or local history (local authority wins):

- Saturn optical identity and raw-sector evidence (`saturn_boot_evidence`,
  `d5171d1`/earlier optical work).
- Mega CD/Sega CD identity (`segacd_boot_evidence`).
- PC Engine CD/TurboGrafx-CD IPL evidence (`pcengine_cd_boot_evidence`).
- 3DO OperaFS/volume evidence (`threedo_boot_evidence`).
- Neo Geo CD IPL evidence (`neogeocd_boot_evidence`).
- Philips CD-i `CD-RTOS` identity plus structural filesystem/startup evidence
  (`platform`, `cdi_disc_evidence`, commit `22d0136`).
- Shared ISO9660, CUE/BIN, raw-sector and CHD logical-media layers.
- ZX Spectrum TAP/TZX semantics, WAV ROM recovery, generic/custom waveform
  evidence, and Alkatraz-only named-family evidence.
- Commodore, Amstrad CPC, BBC Micro, MSX, and Atari 8-bit standard/custom WAV
  families (commits `e42148c`, `dea8ae8`/`ea24591`, `7c61195`/`82f2e63`,
  `8412975`/`d7a86f8`, `5abf045`/`08a98bc`).
- Tape GUI details (`98510b5`, `cb039b0`).
- WHDLoad slave parsing, discovery, DAT reconciliation hooks, Amiberry/FS-UAE
  profile inspection, and Kickstart readiness (`20de545`, `bfda17c`,
  `423b839`, `7b3376c`).
- ADF/HDF Amiga image inspection and bounded filesystem work.
- Bounded ADZ (gzip-wrapped ADF) decompression reusing the existing ADF
  parser unchanged (`amiga_adz`), and bounded WHDLoad `.slave` discovery
  inside LHA/LZH archives reusing the existing archive-member reader and
  slave parser, never auto-picking between multiple valid candidates
  (`amiga_whdload_archive`).
- PC-98 boot evidence is now wired through validated D88, HDI and NHD layouts
  (`pc98_container_evidence`); generic FAT, geometry-only and conflicting
  Japanese-platform cases remain fail-closed.
- FM Towns IPL4 evidence is now wired through validated D88, HDI and NHD
  layouts (`fmtowns_boot_evidence`, `fmtowns_container_evidence`). A valid
  IPL4 signature plus x86 transfer is strong platform evidence; a bounded
  `FBIOS` probe can add TownsOS evidence on contiguous HDI/NHD payloads. Generic
  FAT, container/geometry-only, malformed and filename-only cases remain
  non-Towns.
- Atari STX/Pasti, Acorn DFS, D64, DSK, D88, HDI/NHD, XDF/DIM and other disk
  structural parsers listed in `disk_format`.
- LaserDisc set/framefile verification (`laserdisc_set`, `f892e15`).

These are not claims that every format has GUI or launch parity; consult the
matrix for depth and tests.

## Confirmed implementation gaps

### P0

None found in this audit that are both user-critical and already structurally
specified but entirely absent. Existing safety gates should not be weakened.

### P1

1. **Amiga launch execution completion.** WHDLoad identity/profile/readiness is
   real, but `project_amiga_whdload_launch_input` stops before a dedicated
   Amiga command/execution planner. Files: `launch/input_projection.rs`,
   `launch/amiberry_*`, `launch/fsuae_*`. Requires an explicit emulator command
   contract and BIOS policy.
2. **CHD/DAT logical verification parity.** Bounded per-track comparison now
   exists for the proven logical track and preserves explicit mismatch,
   incomplete, unsupported and unverified outcomes. Full raw/track
   reconstruction, non-zero-pregap seeking, subchannel bytes and specialist
   multi-track parity remain intentionally bounded. Files:
   `chd_redump.rs`, `chd_logical_media.rs`, `dat/archive/chd.rs`,
   `disc_evidence_collector.rs`.
3. **Apple II filesystem breadth and preservation interpretation.** Bounded DOS
   3.3/ProDOS/container evidence now exists in `apple2_disk`; full catalogue
   traversal, GCR decoding, and WOZ/NIB logical identity remain out of scope.
   Japanese disk filesystem evidence is still mostly family or DAT-level.
   Files: `apple2_disk`, `disk_format/*`, `platform_evidence_fusion`.

### P2

4. **Broader UEF/CAS semantics.** The bounded ordinary UEF/CAS bridge is
   implemented; bit-level UEF chunks, richer CAS variants, and turbo/custom
   loader semantics remain out of scope.
5. **LaserDisc frame/media metadata.** Set coherence is implemented, but no
   bounded ffprobe integration, exact frame-count/rate validation, or Singe
   script parser exists.
6. **Raw preservation formats.** IPF, SCP/flux, NIB/WOZ and subchannel/weak-bit
   semantics are not implemented beyond selected detection/registry evidence.

### P3

7. **GUI detail parity for structural media.** Tape has a detailed page; most
   disk/optical/CHD/CD-i/LaserDisc facts are backend-only or compact summaries.
8. **Modern/less common containers.** WUD/WUX, NRG, MDS/MDF, CCD/SUB, IMD/TD0,
   DMF, and similar variants have no safe dedicated readers.
9. **Broader real-corpus validation.** Several families have synthetic tests
   only; the coverage inventory records this honestly rather than upgrading
   maturity from code presence alone.

## Launch/readiness and GUI conclusions

Identity is not launch. Launch adapters/planners exist for many platforms
(Dolphin, PCSX2, DuckStation, PPSSPP, RetroArch, MAME/FBNeo, DOSBox, ScummVM,
FS-UAE/Amiberry profiles, Hatari, Vice, mGBA, SameBoy, Cemu, Citra/Azahar,
Xemu/Xenia, RMG and others), with readiness checks where the adapter requires
profiles/BIOS. The `launch/platform_map.rs`, `launch/*_command.rs`,
`launch/*_execution.rs`, and GUI Launch Readiness page are the authority.
Several content families above intentionally stop at identity or projection;
they must not be advertised as launch-ready.

GUI coverage is currently strongest for selected-game identity, launch
readiness, cheats/mods, DAT/Doctor, tape details, and Playing Library. Disk,
CHD, CD-i and LaserDisc evidence is primarily core/summary-level. No GUI
configuration editor or automatic repair is implied by an evidence row.

## DAT and preservation policy

No-Intro is primarily cartridge/ROM; Redump is optical; TOSEC covers many
computer/floppy/tape ecosystems; MAME software lists publish machine/software
and CHD disk identities; FBNeo and custom Logiqx/ClrMamePro sources are typed
separately. Container identity (e.g. CHD header SHA-1) and logical-content
identity (e.g. Redump track hashes) remain separate. A DAT disagreement is
preserved, not auto-resolved. `identity_source/*`, `dat/*`,
`platform_evidence_fusion/*`, and `coverage_inventory.rs` implement this
policy.

Raw/preservation formats are never marked preservation-fidelity capable merely
because an extension is recognized. Weak bits, intentional CRC errors,
nonstandard geometry, subchannels, custom sectors, and copy-protection
structures are reported only where a dedicated parser proves them. EmuWiz
does not circumvent protection.

## GitHub archaeology and implementation provenance

The fetched Git history confirms the repeated features that prompted this
ledger: `78faa88` (platform registry), `ab77e0f` (Amiga ADF discovery),
`20de545`/`bfda17c`/`423b839` (WHDLoad), `50d4007` (Acorn DFS), `72cefb3`
(Neo Geo CD), `32320fe` (PC Engine CD), `e42148c` onward (tape), and the
recent local commits listed above (CD-i, LaserDisc). Remote branches include
specialized Atari, Apple, X68000, DAT, emulator-adapter, and WHDLoad audits;
those are reference/archaeology unless their changes are present in the
authoritative local tree. `docs/LEGACY_BRANCH_BACKLOG.md` records many of the
same branches as intentionally deferred or superseded.

## Recommended next media lanes (only proven gaps)

1. **[P1] Complete Amiga WHDLoad launch command/execution planning** from the
   existing verified identity/profile projection.
2. **[P1] Extend bounded CHD logical/Redump verification** to additional
   proven track layouts, including non-zero pregap and specialist-backed
   subchannel parity; the V1 single-track/core comparison is now available.
3. **[P1] Extend Apple II filesystem evidence** with bounded catalogue
   traversal and optional GCR-aware identity; the V1 DOS 3.3/ProDOS/container
   gate is already implemented.
4. **[P2] Implement a minimal UEF/CAS container bridge** only after format
   semantics and existing tape-analysis handoff are specified.
5. **[P2] Extend LaserDisc verification with bounded media metadata and frame
   range checks**, retaining unknown results for variable-frame-rate assets.
6. **[P2] Add Atari STX/IPF/flux preservation evidence** only with documented
   weak-bit/intentional-error semantics.
7. **[P3] Surface backend disk/optical/CD-i/LaserDisc evidence in GUI details**
   without creating a configuration editor or repair workflow.

## Audit boundaries and safety

This was an inventory/documentation task. No production code was modified, no
files were reset/cleaned/stashed/restored, no media was downloaded or changed,
and no heavy Cargo build was started. The known dirty
`crates/archivefs-core/src/ingestion/container.rs` plus dirty cheat/core files
remain unrelated and unstaged.
