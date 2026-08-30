# Amiga Family Support Audit — EmuWiz (RESEARCH ONLY)

**Scope:** Amiga OCS/ECS/AGA · CD32 · Commodore CDTV · WHDLoad installs · floppy games · HDF installations — Amiberry, FS-UAE, RetroArch/PUAE
**Branch:** `feature/archivefs-unified-platform`
**Method:** static source analysis only — no source modified, no commits.

---

## A. PLATFORM MODEL

| | Amiga (`platform/mod.rs:811-835`) | AmigaCD32 (`:826+`) | Commodore CDTV (`:837-848`) |
|---|---|---|---|
| aliases | `amiga`, `commodoreamiga`, `commodoreamiga500`, `amiga500`, `amigaocs`, `amigaaga` | `amigacd32`, `cd32`, `commodorecd32`, `amigacd32cd` | `cdtv`, `commodorecdtv`, `amigacdtv` |
| strong ext | `adz`, `ipf`, `dms`, `hdf`, `lha` | *(none)* | *(none)* |
| weak ext | `adf`, `zip`, `iso`, `lzx` | `iso`, `cue`, `bin`, `chd`, `ccd`, `mdf` | `iso`, `cue`, `bin`, `chd` |
| magic | **`DOS\0` @0 OFS/FFS boot block — Strong** | none | none |
| conflicts | AmigaCD32, Acorn Archimedes, CDTV | Amiga, CDTV, PSX | Amiga, AmigaCD32 |

- **OCS/ECS/AGA are folder aliases (`amigaocs`/`amigaaga`), not platforms** — machine capability, correctly. No architecture pressure to split; chipset facts would live in evidence, not rows.
- **`IdentityPlatform` has no Amiga variant** (`game_identity.rs:265-288` — zero Amiga references in the whole file). Amiga identity runs entirely through the WHDLoad evidence channel (`identity_source/whdload/convert.rs`: `EvidenceChannel::LocalWHDLoad`, `SourceFamily::WHDLoad`, `Representation::{WHDLoadSlave, WholeHdf, StructuralMetadata}`) and the launch projection — a deliberate parallel path, not the standard one.
- ES-DE: `amiga` row only (`es_de_export.rs:173-175`); **no `amigacd32`/`cdtv` rows**. RomM outbound: **no Amiga rows at all** (`romm_platform_mapping.rs` grep empty). Launch row: `Amiga` → `standalone_adapters: ["amiga_whdload"]`, `retroarch_core_hints: ["puae"]` (`launch/platform_map.rs:150-152`); **no CD32/CDTV rows**. coverage_inventory: **no Amiga/CD32/CDTV rows** (grep empty).

## B. ADF

**The ADF parser exists, is deep, and is orphaned.** `amiga_disk/`:
- `inspect_amiga_image:197` — RDB-partitioned *and* flat AmigaDOS images (`Partition:49`, `Rdb:65`, `AmigaDisk:77`).
- **`inspect_amiga_floppy:310`** — content-aware `.adf` inspection: container validation → first partition → **`inspect_amiga_filesystem`** (`filesystem.rs`) via the external **`affs_read`** crate: validates the OFS/FFS boot block *and* root block, decodes the AmigaDOS byte (`DOS\0`–`DOS\7`, `dos_type: u8`), OFS-vs-FFS (`FsType`), **volume label** (`volume_label`, display-only), bounded traversal limits, explicit `DiscoveredSlave` discovery inside volumes.
- Collision honesty is exemplary: the Acorn ADFS `.adf` collision is refused by content ("a file that merely ends in `.adf` but whose bytes do not present a valid AmigaDOS boot block and a structurally valid root block is refused, not trusted", `mod.rs:322-326`); `AmigaFloppyError::{Container,NoPartition,Filesystem}` refuses PFS/SFS/MuFS as detection-only.
- `structural_amiga_floppy_observation:331` converts to `Strong` platform evidence ("never from an extension alone"), release candidate always `None` — volume label is context, never identity.

**The broken join:** `discover_direct_file` routes only `.rdb` → `discover_amiga_image` (`discovery.rs:562`); a loose `.adf` falls through to the *generic* `AmigaImage` accept path — **`inspect_amiga_floppy` has zero callers outside `amiga_disk`** (grep verified). Bootblock checksum/root-block/volume-name evidence exists and never runs in discovery.
- Bootblock checksum: validated *inside* `affs_read`'s boot-block validation (the crate does not hand-roll it — correct reuse).
- Malformed/truncated: fail-closed via the three-variant error taxonomy.

## C. ADZ

- `cf("adz", AmigaImage)` registered (`content_registry.rs:114`); **no gzip decompression path exists in core** (flate2 appears only inside CHD zlib handling; no `GzDecoder` anywhere).
- Safe capped decompression is reasonable: gzip is self-terminating, `flate2` is already a dependency, and the ADF output is ≤ ~901 KB (or ≤ 4 MiB HDF-style) — a bounded `GzDecoder` with an output cap mirrors the existing DSK/Pasti read-budget discipline. **Feasible Small task; not implemented.**

## D. DMS

- `.dms` is a **strong** Amiga extension (`platform/mod.rs:823`) but is **absent from `content_registry`** and has **no parser anywhere** (grep: only the platform row and unrelated files).
- DMS (DiskMasher) classification per two-reference review: 56-byte header (`DMS!` magic, type/mode flags), per-track records with packed lengths, compression modes 0-6 (none/RLE/Quick LZ/LZ + Huffman/heavy 1/2), optional **encryption bit + password** fields, track checksums; expanded size ≤ ~901 KB/track-capped.
- **Classification: medium decompressor task with a bomb-risk gate.** Modes 0-1 are trivially bounded; modes 2-6 require real decompressor ports; the encryption flag must refuse (never decrypt with guessed passwords). Recommended sequencing: register + header/magic validation first (Tiny), full decompression defer. Nothing exists to orphan.

## E. IPF / CAPS/SPS

- `.ipf` weak ext + `cf("ipf", ComputerDisk)`; **no code**; `capsimg`/SPS library: absent. Licensing (SPS/CAPS is proprietary) rules out bundling — the Atari audit's identical conclusion applies. Emulators (FS-UAE/Amiberry via capsimg, PUAE) consume IPF directly, so EmuWiz should remain **pass-through**: hash-only identity, extension-level recognition, no container validation (the format is opaque without the library). Deliberately unsupported; do not revisit.

## F. SCP / flux / preservation formats

- SCP, KryoFlux streams, HFE, UAE extended-ADF: **zero references**. Correct posture: **register/hash only** (or defer entirely). These are preservation containers whose value is bit-exactness; structural parsing adds no identity beyond the hash. No action recommended.

## G. HDF / HARD DISK IMAGES

- **`inspect_hdf:82`** — a real RDB parser (partition table, DOS types, geometry) feeding `inspect_amiga_filesystem` traversal; `structural_hdf_observation:379` converts to `Representation::WholeHdf` evidence.
- Collision handling is the best in the repo: `.hdf`/`.hdfx` are **deliberately unregistered** (`content_registry.rs:105-111` — the real X68000-collision story) and route through `discover_ambiguous_disk_image` (`discovery.rs:651-690`): **Amiga-parse first** (`inspect_amiga_image`), else folder-hint `ComputerDisk`, else `AmbiguousPlatform`. Extension/size alone never proves Amiga — verified.
- VHD: absent. Raw non-RDB HDFs: refused unless flat-AmigaDOS validates. Atari/PC raw disks: never claimed (the Amiga parser validates RDB/AmigaDOS structures, not geometry).

## H. WHDLoad

**The WHDLoad stack is the deepest content-type subsystem in the repo:**
- **Slave parser** (`identity_source/whdload/slave.rs`): `parse_whdload_slave:81` — big-endian header, `runtime_version` (@12), `struct_size` per version, `flags` (@14), `base_mem_size` (@16), version-gated fields: `dont_cache`/`key_debug` (v4+), `key_exit`/`exp_mem` + **`name`** (v8+), **`copyright`/`info`/`kick_name`** (v10+), **`kick_size`/`kick_crc`** (v16+) — i.e. game name, copyright, info, and the **required Kickstart name/size/CRC** are all extracted, fail-closed on unsupported versions.
- **Production wiring:** `discovery.rs:436-462` — `FolderRole::WhdloadInstall` folders → `inspect_whdload_slave_file` → `GameDiscovery` ("WHDLoad install (N slave file(s) found)"; unreadable slave → refused, not guessed).
- **Evidence conversion** (`whdload/convert.rs`): `structural_slave_observation:32` ("a valid WHDLoad slave is strong structural evidence of Amiga software") and **`exact_slave_match_observation`** — slave-to-DAT reconciliation observations. **The latter is orphaned** (zero callers outside the module).
- **Local adapter** (`patch_manager/amiga_whdload_local.rs`): "Bounded, read-only inspection of local **Amiberry and FS-UAE** installations" — Flatpak IDs for both (`:34-35`), bounded configs (256 KB/8192 lines), `discover_amiga_profiles:263`, `parse_amiga_version:350`, `inspect_amiga_hdf:369`, `inspect_amiga_whdload_game:428`, and **Kickstart readiness**: `inspect_kickstart` → `AmigaKickstartState`/`AmigaKickstart { state, from_hdf }` (`:169-188, 441-449`).
- **Launch:** `LAUNCH_COMPATIBILITY` row (`standalone amiga_whdload`, `puae` core hints) + **`project_amiga_whdload_launch_input`** (`input_projection.rs:388-394`) → `AmigaGameRequest { verified_amiga_identity }` via `VerifiedIdentityFact::AmigaIdentity`. **Missing seam:** no `launch/amiga_command.rs`/`amiga_execution.rs` — the projection ends where Hatari's did before its (also missing) planners.
- **Fields not surfaced:** `flags` kept raw (AGA bit not decoded into a named fact); WHDLoad-tool options (JST/custom slaves) absent; trainer flags absent (see §W).

## I. WHDLoad FILENAME / DAT IDENTITY

- Slave *content* is the identity source: `name`/`copyright` come from the parsed header (structured metadata), and `exact_slave_match_observation` exists for hash-keyed DAT reconciliation. Filenames (`Game_v1.2_1234.hdf`, `_AGA_`) are never parsed into facts — no AGA-from-filename inference exists (grep: no AGA parsing outside the platform alias).
- Release identity remains hash/DAT-driven; slave `name` is structured provenance, not rename authority (consistent with the whole crate's rules). **No unsafe filename-identity promotion found.**

## J. LHA / LZX / ZIP packs

- `cf("lha", AmigaImage)` registered; **`dat/archive/lha.rs`** exists (member listing for DAT audit); ZIP via the shared zip stack with the bounded-member content-evidence machinery (`archive_member_content_evidence.rs`); 7z generic. `.slave`-inside-archive inspection: not implemented (the discovery path inspects *extracted* folders; `inspect_whdload_slave_file` takes a file path — an archive-member variant is a possible Small join, not present). LZX: **unregistered, no parser** (Amiga-only format; decompressor is nontrivial — defer). Path-traversal/resource bounds: inherited from the shared archive layer; no recursive extraction anywhere.

## K. KICKSTART / FIRMWARE

- **Discovery + state:** `inspect_kickstart` → `AmigaKickstartState` (state machine incl. from-HDF kickstarts), wired into `inspect_amiga_whdload_game`.
- **Verification:** slave-carried `kick_name`/`kick_size`/`kick_crc` (v10+/v16+) give per-game *requirements*; no embedded known-hash table (same external-reference discipline as Hatari TOS — correct, nothing bundled).
- **Machine distinctions:** A500/A600/A1200/CD32/CDTV Kickstart differences are **not** modeled as a matrix (only state + from_hdf); encrypted Amiga Forever ROM keys: absent.
- **Chain break:** Kickstart state is computed but reaches neither `launch/readiness.rs` (no Amiga projection exists there), nor Doctor (zero amiga references in `diagnostics/`), nor the GUI.

## L. CD32

- Platform row: no strong ext, weak disc extensions, three-way conflict (Amiga/CDTV/PSX) — honest.
- **No CD32-specific evidence exists**: `disc_evidence_collector.rs`'s boot branches are SYSTEM.CNF/IP.BIN/Saturn/SegaCD/Opera/PC-FX/IPL.TXT — no CD32. CD32 discs are ISO9660 (+ optional "EXTENDED ROM"/library tracks, unmodeled); generic optical stack (ISO9660/CUE/CHD with strict track rules) reads them fine.
- Kickstart requirement: CD32 needs its ROM (modeled nowhere). Identity today: folder-only. **A bare disc cannot be distinguished CD32-vs-CDTV-vs-generic-ISO today** — both rows are folder-only, correctly refusing to guess.

## M. CDTV

- Platform row identical in shape to CD32 (no strong ext, disc weak exts). No boot evidence, no firmware modeling, no ES-DE/RomM/launch rows, no coverage row. Distinguishing CDTV from CD32 from disc bytes: **not possible today and not attempted** — correct fail-closed posture (both need boot-structure research: CDTV discs boot via their own ROM load sequence; CD32 via extended Kickstart tracks).

## N. GENERIC OPTICAL REUSE

Amiga-family optical content rides the shared stack entirely (ISO9660 bounded walk, CUE/BIN resolution, CHD track-1/zero-pregap rules). **No Amiga-specific optical duplication exists.** Missing dispatch: `.chd` identity arm covers PlayStation/Saturn/DC/SegaCd/ThreeDo/Pcfx/PcEngineCd — no Amiga/CD32 (same match-arm defect class as PS2/PC-FX, though without CD32 evidence there is nothing to route to yet).

## O. EMULATOR DISCOVERY

| Emulator | Detection | Readiness | Planning | Execution | Doctor | GUI |
|---|---|---|---|---|---|---|
| Amiberry | ✅ (`amiga_whdload_local`, Flatpak ID) | ✅ (kickstart state) | ❌ | ❌ | ❌ | ❌ |
| FS-UAE | ✅ (same adapter, Flatpak ID) | ✅ | ❌ | ❌ | ❌ | ❌ |
| PUAE (RetroArch) | core-hint only (`retroarch_core_hints: ["puae"]`) | via generic RA chain | ✅ dynamic | ✅ `spawn_retroarch` | ✅ | ✅ |
| WinUAE/Wine | absent | — | — | — | — | — |

The standalone chain stops at projection (`project_amiga_whdload_launch_input` exists, planners/execution don't) — the exact Hatari shape.

## P. MACHINE PROFILE / CHIPSET SELECTION

- OCS/ECS/AGA: aliases only; **no chipset fact** is decoded (slave `flags` kept raw — the AGA bit is *present* in parsed data but unnamed).
- Kickstart machine compatibility: state + from_hdf only; no A500/A1200 matrix.
- No filename-derived machine selection anywhere (grep: no AGA/OCS inference). **No unsafe automatic selection exists** — machine choice is currently emulator-default, which is the safe (if unhelpful) default.

## Q. WHDLOAD LAUNCH

- Can project: a verified WHDLoad identity from facts → `AmigaGameRequest` (authorized) — **cannot command**: no `amiga_command.rs`/`amiga_execution.rs`; no shell strings anywhere (argv discipline holds).
- A `.slave` alone: discovery-level recognition only; no launch concept. Extracted WHDLoad directory: the intended unit (`FolderRole::WhdloadInstall` → identity → projection). HDF-containing WHDLoad: `inspect_amiga_hdf`/`inspect_amiga_whdload_game` inspect it; launch planning for HDF content: not modeled. LHA pack: not launchable (archive first).
- The four WHDLoad forms are kept distinct throughout — no conflation found.

## R. FLOPPY MULTI-DISK

- Generic machinery only: `MultiDiscSet`/`library_grouping`, TOSEC disc naming, and the **`companion_operations`** mechanism (Playing Library election carries companions — `playing_library`/`retrodeck_projection.rs:159-170`), which is the same generic path GC/Wii use. No Amiga-specific disk-swap semantics (FS-UAE/Amiberry config lists) are modeled; no Amiga multi-disk election test exists.

## S. WHDLoad VS FLOPPY RELEASE GROUPING

- Floppy release, WHDLoad install, HDF, CD32 are **different content kinds/platforms** (AmigaImage vs WhdloadInstall vs WholeHdf representation vs CD32 row) — never collapsed. 1G1R operates within each kind's DAT ecosystem; cross-kind duplicate detection has no join (a WHDLoad install and its floppy original are unrelated rows today). This is the honest state; "same game, different form" grouping would be new modeling (dat/dependency seams exist for it) — recorded, not built.

## T. DAT ECOSYSTEMS

- TOSEC (floppy/HDF), No-Intro (Amiga + CD32), Redump (CD32/CDTV discs), WHDLoad DATs (slave `exact match` observations exist for exactly this). Whole-image hashing generic; member-level identity via `dat/archive/lha.rs` + zip stack; stale handling generic. **`exact_slave_match_observation` is the WHDLoad-DAT reconciliation hook — implemented, orphaned.** No Amiga-specific DAT machinery needed beyond wiring it.

## U. ROMM

- Outbound: **no Amiga/CD32/CDTV rows** (verified empty). Inbound: implicit via aliases only. RomM's own model has `amiga`, `amigacd32`, `cdtv` platforms — three missing rows. WHDLoad folders/HDF/LHA representation in RomM: unmodeled (RomM treats them as files/folders); companions preserved by the generic projection where used.

## V. ES-DE / RETRODECK

- `amiga` row only (`es_de_system: "amiga"`, fullname "Commodore Amiga"); **no `amigacd32`/`cdtv` rows** (ES-DE ships `amigacd32` and `cdtv` systems — both missing). AGA folds into `amiga` (correct). RetroDECK projection inherits via `es_de_system_for_platform` (Amiga works; CD32/CDTV fail closed).

## W. CHEATS / MODS

- **Nothing Amiga-specific exists**: no trainer parsing, no WHDLoad `dont_cache`/trainer-flag consumers, no JST/options handling, no graphics patches. `patch_manager` has `amiga_whdload_local` (inspection only). Honest absence — do not invent.

## X. GUI-HIDDEN FACTS (source-proven)

OFS-vs-FFS + exact DOS byte; volume label; RDB partition layout/geometry; WHDLoad runtime version, flags, base/exp mem, name/copyright/info, **required Kickstart name/size/CRC**; slave count per install; Kickstart state (incl. from-HDF); Amiberry/FS-UAE version; HDF DOS types. None reach the GUI.

## Y. DOCTOR

`diagnostics/` has **zero** amiga/whdload references (grep verified): missing Kickstart, wrong-Kickstart/machine, malformed ADF, bad root block, missing slave/data files, unsupported archive, ambiguous AGA requirement, missing multi-disk companion, stale DAT — **none reportable**. The `AmigaKickstartState`/`AmigaFloppyError` vocabularies already separate informational from refusal-grade findings; they are just unreachable.

## Z. SECURITY / FAIL-CLOSED

- `.adf` ≠ Amiga: Acorn ADFS refused by content; boot+root validation required.
- `.hdf` ≠ Amiga: deliberate non-registration + Amiga-parse-first ambiguous route; X68000 collision documented from a real incident.
- `.lha` ≠ WHDLoad: slave content required (`.slave` read + parsed, else refused).
- "AGA" in filename: never read. `.slave` filename: never identity — header parsed. Folder name: `FolderRole` classification only.
- Shell execution: none (argv discipline; the missing Amiga planners don't exist to violate it).
- Decompression safety: DMS/LZX/ADZ decompression absent (nothing to bound yet); archive bounds inherited from the shared layer. **No unsafe Amiga assumption found.**

## AA. REAL-CORPUS COVERAGE

**No coverage-inventory rows exist for Amiga, CD32, or CDTV** (grep empty) — despite `disk_format`/`amiga_disk`/whdload tests being synthetic-fixture-rich and `hatari_local`-style adapters being tested. The X68000 `.hdf` collision note records a real-corpus *incident* (mislabelled members), but no `RealValidated` Amiga evidence rule exists. Status for every major rule: **NoCoverage** (with the one recorded real-corpus correction story for `.hdf`).

## AB. MATURITY MATRIX

| | ADF | WHDLoad | HDF | CD32 | CDTV |
|---|---|---|---|---|---|
| Platform model | PARTIAL — row exists, no `IdentityPlatform::Amiga` | PARTIAL — content class, not a platform (correct) | PARTIAL — Amiga row claims `hdf` strong | MATURE (honest folder-only row) | MATURE |
| Media registration | PARTIAL — `cf("adf")` but generic path | MATURE (`FolderRole::WhdloadInstall` + slave inspection) | PARTIAL — deliberate non-registration w/ ambiguous route | REGISTERED-ONLY (weak disc exts) | REGISTERED-ONLY |
| Structural evidence | **ORPHANED** — `inspect_amiga_floppy` complete, zero callers | MATURE — slave parser production-wired | PARTIAL — RDB parser wired only via the ambiguous route + whdload adapter | MISSING | MISSING |
| Stable game/install ID | MISSING | PARTIAL — slave name/kick facts; no persisted catalogue fact | PARTIAL — volume label/structures as provenance only | MISSING | MISSING |
| DAT/hash identity | MATURE (whole-file) | PARTIAL — `exact_slave_match_observation` orphaned | MATURE (whole-file) | MATURE (generic disc) | MATURE |
| Persistence | MATURE | MATURE | MATURE | MATURE | MATURE |
| Kickstart/firmware | N/A | PARTIAL — discovered+state, not projected/doctor-shown | PARTIAL — from-HDF kickstarts | MISSING (CD32 ROM unmodeled) | MISSING |
| Emulator discovery | N/A | MATURE (Amiberry+FS-UAE profiles) | N/A | MISSING | MISSING |
| Readiness | N/A | PARTIAL — kickstart state computed, not projected | N/A | MISSING | MISSING |
| Planning | MISSING | PARTIAL — projection done, command plan missing | MISSING | MISSING | MISSING |
| Execution | MISSING | MISSING (no amiga_execution) | MISSING | MISSING | MISSING |
| GUI launch | MISSING | MISSING (no context; unlike Dolphin/Hatari-class gap) | MISSING | MISSING | MISSING |
| Doctor | MISSING | MISSING (zero diagnostics references) | MISSING | MISSING | MISSING |
| Cheats / Mods | N/A | MISSING (honest) | N/A | N/A | N/A |
| Multi-disk | PARTIAL — generic companions only | N/A (installs bundle disks) | N/A | N/A | N/A |
| Rename/Duplicates/1G1R/Playing Library | MATURE (generic) | MATURE (generic) | MATURE | MATURE | MATURE |
| RomM | MISSING row | MISSING | MISSING | MISSING | MISSING |
| ES-DE | MATURE (`amiga`) | via `amiga` | via `amiga` | MISSING row | MISSING row |
| Real corpus | NoCoverage | NoCoverage | NoCoverage (one recorded incident) | NoCoverage | NoCoverage |

## AC. BROKEN JOINS (top 15)

1. `inspect_amiga_floppy` + `structural_amiga_floppy_observation` exist → **discovery's `.adf` path never calls them** (only `.rdb` routes to `amiga_disk`).
2. WHDLoad slave evidence channel exists → **no `IdentityPlatform::Amiga`** for the standard identity pipeline to carry it.
3. `project_amiga_whdload_launch_input` + `AmigaGameRequest` exist → **no `amiga_command.rs`/`amiga_execution.rs`**.
4. `exact_slave_match_observation` (WHDLoad↔DAT reconciliation) exists → **zero callers**.
5. Kickstart state (`AmigaKickstartState`) exists → **not in `launch/readiness.rs`, Doctor, or GUI**.
6. Amiberry/FS-UAE profile discovery exists → **absent from `diagnostics/profiles.rs`** (Hatari-pattern Doctor gap).
7. CD32/CDTV platform rows exist → **no ES-DE rows** (`amigacd32`/`cdtv` missing) although ES-DE ships both systems.
8. CD32/CDTV rows exist → **no RomM outbound rows** (`amiga`/`amigacd32`/`cdtv` all missing).
9. `LAUNCH_COMPATIBILITY` Amiga row exists → **no GUI launch context** (broken-joins #1 family).
10. `cf("adz")` registered → **no bounded gzip reader** (flate2 already in-tree via CHD).
11. `.dms` strong platform claim → not even in `content_registry`.
12. `.chd` identity arm covers seven platforms → **no Amiga/CD32 arm** (match-arm defect class).
13. Slave `flags`/`base_mem`/`exp_mem` parsed → no chipset/RAM readiness fact (AGA bit unnamed).
14. `dat/archive/lha.rs` member listing exists → no archive-member `.slave` inspection path.
15. Amiga/CD32/CDTV have **no coverage-inventory rows** — the evidence ledger undercounts the family (audit-hygiene join).

## AD. ORPHANED CODE

| Module/function | Missing seam | Size |
|---|---|---|
| `amiga_disk::inspect_amiga_floppy` + `structural_amiga_floppy_observation` | discovery `.adf` route (`discovery.rs:551-609`) | Tiny |
| `identity_source/whdload::exact_slave_match_observation` | DAT/identity consumer (slave-hash reconciliation) | Small |
| `patch_manager::amiga_whdload_local::inspect_kickstart`/`AmigaKickstartState` | `launch/readiness.rs` projection + Doctor adapter | Small |
| `launch::input_projection::project_amiga_whdload_launch_input` | `amiga_command.rs`/`amiga_execution.rs` (flycast/Hatari template) | Medium |
| `AmigaGameRequest`/`VerifiedIdentityFact::AmigaIdentity` | same command seam | (part of Medium) |

## AE. DO NOT REBUILD

- **`amiga_disk/`** — `inspect_amiga_image`/`inspect_hdf`/RDB + the `affs_read`-based OFS/FFS traversal with its collision taxonomy (Acorn ADFS, PFS/SFS refusal). The best collision-safe disk parser in the crate.
- **`identity_source/whdload/slave.rs`** — version-gated, fail-closed Slave parsing incl. Kickstart requirements. Extend usage, never the parser.
- **`patch_manager/amiga_whdload_local.rs`** — Amiberry/FS-UAE profile inspection with Flatpak IDs, bounded configs, kickstart states.
- **The `.hdf`/`.hdfx` deliberate non-registration + ambiguous route** — written from a real-corpus incident; untouchable.
- **`discovery.rs` WHDLoad folder flow** (`FolderRole::WhdloadInstall` → slave inspection).
- **The shared optical stack + `.hdf` collision logic + generic DAT/1G1R/companion machinery** — reuse only.

## AF. PRIORITISED BACKLOG + BEST 10 TASKS

**P0:** (1) wire `inspect_amiga_floppy` into `.adf` discovery; (2) `IdentityPlatform::Amiga`; (3) RomM/ES-DE rows (`amiga` ES-DE exists; add `amigacd32`/`cdtv` + RomM trio).
**P1:** (4) `amiga_command`/`amiga_execution` (+GUI context per broken-joins #1); (5) Kickstart readiness projection + Doctor adapter; (6) ADZ bounded gzip; (7) slave-DAT reconciliation wiring.
**P2:** (8) DMS (register → header-validate → decompress-later); (9) CD32/CDTV boot evidence (research-first); (10) archive-member `.slave` inspection.

| # | Slug | Objective | Files | Reused | Missing join | Non-goals | Tests | Benefit | Dep | Size |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `adf-discovery-wiring` | Route `.adf` through `inspect_amiga_floppy` in discovery | `ingestion/discovery.rs` | `inspect_amiga_floppy`, `structural_amiga_floppy_observation` | the orphaned ADF parser | no Acorn ADFS softening; no volume-label-as-identity | ADFS-refusal, OFS/FFS, truncated refuse | real structural Amiga evidence at scan time | none | **Tiny** |
| 2 | `amiga-identity-variant` | `IdentityPlatform::Amiga` + catalogue aliases | `game_identity.rs` | variant pattern | enum gap | no inspect arms yet | catalogue round-trips | unblocks standard identity path | none | **Tiny** |
| 3 | `amiga-command-execution` | AmigaWHDLoad command plan + execution (Amiberry/FS-UAE argv) | new `launch/amiga_command.rs`, `amiga_execution.rs`, `launch/mod.rs` | `AmigaGameRequest`, projection, `process_spawn`, flycast/Hatari template | projection→command seam | no shell strings; no PUAE-specific config | preflight/plan/execution | WHDLoad Launch button becomes possible | 2 | **Medium** |
| 4 | `amiga-romm-esde-rows` | RomM `amiga`/`amigacd32`/`cdtv` + ES-DE `amigacd32`/`cdtv` rows | `romm_platform_mapping.rs`, `es_de_export.rs` | row patterns | mapping gaps | no unverified slugs | mapping tests | export for the whole family | none | **Tiny** |
| 5 | `kickstart-readiness-projection` | Project `AmigaKickstartState` into launch readiness + Doctor | `launch/readiness.rs`, `diagnostics/profiles.rs` | `inspect_kickstart` | readiness/Doctor seams | no hash fabrication | state projections | "Kickstart missing" visible before launch | none | **Small** |
| 6 | `adz-bounded-gzip` | Bounded gzip decompress → ADF inspection | new `amiga_disk/adz.rs` or discovery route | `flate2`, `inspect_amiga_floppy`, cap discipline | ADZ registered-but-opaque | no streaming-to-disk; strict output caps | bomb/oversize refuses | compressed collections become structural | 1 | **Small** |
| 7 | `slave-dat-reconciliation` | Consume `exact_slave_match_observation` in DAT matching | `platform_evidence_fusion/dat_hash_representation.rs` | slave hashes, DAT infra | orphaned observation | no filename identity | slave↔DAT fixture | WHDLoad installs match DATs natively | none | **Small** |
| 8 | `dms-registration-and-header` | Register `.dms` + bounded header/magic validation | `content_registry.rs`, new `disk_format/dms.rs` (header only) | bounded-reader discipline | strong claim w/o registry | no decompression modes 2-6; encrypted refuse | header fixtures | DMS files become catalogued | none | **Small** |
| 9 | `cd32-cdtv-evidence-research` | Two-source review of CD32/CDTV boot signatures → `*_boot_evidence` + `.chd` arm + rows | new boot modules, `platform/mod.rs`, `game_identity.rs`, `disc_evidence_collector.rs` | optical stack, PcEngineCd pattern | no boot evidence at all | no extension-based CD claims | boot fixtures | CD32/CDTV leave folder-only limbo | research | **Medium** |
| 10 | `amiga-coverage-rows` | Coverage-inventory rows for Amiga/CD32/CDTV | `coverage_inventory.rs` | row pattern | ledger gap | none | inventory tests | honest coverage reporting | none | **Tiny** |

## AG. FINAL QUESTIONS

1. **ADF completeness:** the *parser* is near-complete (container + OFS/FFS + root + volume label + collisions) and the *wiring* is absent — loose `.adf` files are catalogued on extension alone today. One Tiny routing change makes ADF support real.
2. **WHDLoad completeness:** deeper than any other content-type subsystem — slave parsing, discovery, evidence, profiles, kickstart, launch projection all exist and are tested; what's missing is the last launch seam (command/execution), Doctor visibility, and DAT reconciliation wiring. ~80% built, 0% launchable from the GUI.
3. **Amiga HDF vs other raw disks:** yes — safely, by content (RDB parse + AmigaDOS validation), with the X68000 collision handled by deliberate non-registration and an Amiga-first ambiguous route. Do not weaken.
4. **Minimum safe DMS:** register + bounded header/magic/mode validation now; refuse encrypted; defer modes 2-6 decompression behind a two-source review. Hash-only identity meanwhile.
5. **IPF:** no — SPS licensing forbids bundling; emulators consume IPF directly; hash-only pass-through is the correct permanent posture.
6. **CD32/CDTV:** rows exist, everything else (evidence, identity, ES-DE/RomM rows, launch) is missing; both are honestly folder-only because no boot-signature work has been done. Research-first.
7. **Preferred standalone path:** the repo already chose — **Amiberry + FS-UAE** via the `amiga_whdload` adapter (both with Flatpak discovery), with PUAE as the RetroArch fallback. Build the command/execution seam for the standalone pair.
8. **Five biggest pre-release changes:** tasks 1, 2, 4, 5, 7 — ADF wiring, identity variant, mapping rows, Kickstart visibility, slave-DAT reconciliation. All Small/Tiny except the command seam.
9. **Wait until after release:** DMS decompression, LZX, CD32/CDTV boot evidence, SCP/flux formats, archive-member slave inspection, any mods/cheats work.
10. **What prevents completeness today:** not knowledge — *wiring*. The Amiga family has the repo's deepest parsers (`amiga_disk`, WHDLoad slave) with the most disconnected last miles: an orphaned ADF parser, an orphaned DAT observation, a projection without a planner, readiness without a Doctor. Everything a user needs exists except the connections.
