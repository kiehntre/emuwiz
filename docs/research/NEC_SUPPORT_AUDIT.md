# NEC Family Support Audit — EmuWiz (READ-ONLY)

**Scope:** PC Engine / TurboGrafx-16, SuperGrafx, PC Engine CD / TurboGrafx-CD, TurboDuo, PC-FX
**Branch:** `feature/archivefs-unified-platform`
**Method:** static source analysis only — no builds, edits, or tree modifications

---

## 1. NEC SUPPORT MATRIX

| Platform | PLATFORMS registry | IdentityPlatform enum | COVERAGE manifest | Boot evidence module | `collect_disc_boot_evidence` | Fusion rule (RULES) | Inspect route (`.cue`/`.chd`/`.iso`) | LAUNCH_COMPATIBILITY | ES-DE map | RomM outbound | FirmwareSystem |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **PC Engine** (HuCard) | Yes — `id="PC Engine"` | **Missing** (no `Pce` variant) | Deferred, 0 detectors | None | N/A (cartridge) | None | None | None | None | None | None |
| **SuperGrafx** | Alias of PC Engine (not separate) | N/A | N/A (alias of PC Engine) | None | N/A | None | None | None | None | None | None |
| **PC Engine CD** | Yes — `id="PC Engine CD"` | **Missing** (no `PceCd` variant) | **No entry** | None | None (no evidence in collector) | None | `.cue` excluded, `.chd` excluded | None | None | Mapped slug only | None |
| **TurboDuo** | Covered by PC Engine CD aliases | **Missing** (no variant) | **No entry** | None | None | None | None | None | None | None | None |
| **PC-FX** | **No entry** (not in PLATFORMS) | Yes — `IdentityPlatform::Pcfx` | **No entry** | `pcfx_boot_evidence.rs` | Yes | None | `.cue` yes, `.chd` no (Deferred) | None | None | None | None |

**Key contradiction:** `evidence_bridge.rs` maps `IdentityPlatform::Pcfx => "PC-FX"` (line 159), but `"PC-FX"` has **no entry in `PLATFORMS`** — the downstream platform registry cannot resolve it. No such contradiction exists for other NEC platforms because they have no `IdentityPlatform` variant at all.

## 2. IDENTITY-QUALITY MATRIX

| Platform | Detection path | Confidence | Notes |
|---|---|---|---|
| PC Engine (HuCard) | `.pce` extension only (strong ext in PLATFORMS) | **FilenameOnly** — fails closed to `Unknown` via content evidence | HuCard header inspection deliberately not implemented (coverage_inventory.rs:428). No IdentityPlatform variant. No fusion rule. |
| SuperGrafx | Same `.pce`/`.sgx` as PC Engine | **Same as PC Engine** — indistinguishable from HuCard content | `.sgx` is a strong extension of "PC Engine" but SuperGrafx is not a separate platform. No content difference to prove the variant. |
| PC Engine CD | Folder aliases only; `.cue`/`.chd`/`.iso`/`bin` are weak/shared | **Ambiguous** (folder evidence only, no boot signature) | No `IdentityPlatform::PceCd`. `.cue`/`cue` arm excludes it. `.chd` arm excludes it. No boot evidence module. |
| TurboDuo | Same as PC Engine CD | **Same as PC Engine CD** | No TurboDuo-specific folder alias. |
| PC-FX | `.cue`/`.iso` → `inspect_pcfx_source` (disc hash) | **Verified** (when platform hint is "PC-FX") | PC-FX disc hash is RetroAchievements-compatible MD5. BUT: `.chd` excluded from inspect route. Platform hint impossible via folder detection (no PLATFORMS entry). |

## 3. SCANNER / DISCOVERY VISIBILITY

Three independent classification systems, none fully aligned for NEC:

**a) `inspector.rs` `LIKELY_CONTENT_EXTENSIONS`** (line 114–122): `.pce` IS listed at line 117 — classified as `InspectorEntryClassification::LikelyContent`. But `.sgx` is NOT listed.

**b) `content_registry.rs` `CONTENT_FORMATS`** (line 74): `.pce`/`.sgx` are **NOT** registered. `content_kind_for_extension("pce")` returns `None`.

**c) `media_registry.rs`** (the archive-centric scanner gatekeeper): no `.pce`/`.sgx` entries.

**Result in `discovery.rs`:** `discover_direct_file` (line 569) calls `content_kind_for_extension(&extension)`. For a `.pce` file, this returns `None`, so it falls through to line 579: `SkipReason::UnsupportedExtension` — "EmuWiz doesn't yet recognise the .pce extension." The file is **silently skipped**, invisible in the library view, despite being listed as "likely content" in the inspector. This is an **orphaned classification mismatch** across the three registries.

For `.cue` files: `discover_cue` (line 376) resolves the CUE sheet and always classifies as `ContentKind::DiscImage`, so a PC Engine CD `.cue` is **visible** as a disc image — but `identity_for` (line 690) calls `ArchiveIdentity::from_path`, which cannot resolve a platform (no `IdentityPlatform::PceCd`), so the identity is `None` and the discovery item is skipped with `RecognizedContentNoIdentityMatch`.

## 4. BIOS / FIRMWARE

`firmware_evidence.rs`: `FirmwareSystem` enum (line 58–62) has only **PlayStation**, **PlayStation2**, **Xbox**. No PC Engine CD System Card, no Super System Card, no Arcade Card, no PC-FX BIOS/KID/BIOS3/BIOS4. `redump_bios_evidence_from_dat` only supports the three PS/Xbox systems. BIOS verification pipeline (`hash_firmware_file` + `matching_firmware_records`) has zero NEC firmware records to check against. No emulator adapter consumes this for NEC (no `*-firmware` patch modules exist for NEC).

## 5. EMULATOR / LAUNCH SUPPORT

**`LAUNCH_COMPATIBILITY`** (`launch/platform_map.rs`, line 82): Contains only PSX, PS2, PS3, PSP, Xbox, Xbox360, GameCube, Wii, Dreamcast, Sega CD, AtariST, Amiga. **No NEC platforms.**

**`ES_DE_SYSTEM_MAP`** (`launch/es_de_export.rs`, line 111): Contains only PSX, PS2, PS3, PSP, Xbox, Xbox360, GameCube, Wii, Dreamcast, Saturn, Sega CD, AtariST, Amiga, NES, SNES, MegaDrive. **No NEC platforms.**

**RetroArch cores:** `retroarch_command.rs` only references `mednafen_psx`. No `mednafen_pce`, `mednafen_pcfx`, `beetle_pce`, `beetle_pce_fast`, `beetle_pcfx`, or `mednafen_pce_fast` core hints exist.

**Standalone adapters:** `patch_manager/` has no NEC emulator adapter modules (only PS2: `pcsx2_*`, PS1: `duckstation_*`, GameCube/Wii: `dolphin_*`, Xbox: `xemu_*`, Xbox360: `xenia_*`). No Ootake, no Mednafen PCE core, no Mednafen PC-FX core.

**RomM outbound mapping** (`romm_platform_mapping.rs`): Only "PC Engine CD" has a static slug (`turbografx-16-slash-pc-engine-cd`). No slug for PC Engine, TurboGrafx-16, or PC-FX. The inbound `ROMM_SLUG_ALIASES` maps `pc-fx → PC Engine` as explicitly approximate (line 429), and the module doc comment states this must NOT be inverted for outbound (line 398–405).

## 6. DAT / METADATA SUPPORT

**No-Intro:** `identity_source/no_intro/` infrastructure exists and is generic (hashes, platform-agnostic). No-Intro publishes separate DATs for PC Engine/TurboGrafx-16 (cartridge) and PC Engine CD/TurboGrafx-CD (disc). The system *could* match `.pce` HuCard hashes if the extension were registered and the DAT were imported, but: (a) no content evidence detector validates the HuCard format structurally, (b) No-Intro's PC Engine DAT uses headered dumps whose 512-byte header the current code cannot strip (no normalization rule exists for HuCard). 

**Redump:** `identity_source/redump/` infrastructure exists for disc track/SHA-1 matching. PC Engine CD and PC-FX disc DATs exist in Redump. The system *could* match CHD/logical CHD hashes, but: (a) no fusion rule maps PC Engine CD content evidence to a platform, (b) PC-FX CHD has no inspect route (excluded from `.chd` arm), (c) the PC Engine CD boot signature (`"PC Engine CD-ROM SYSTEM"`) is not checked anywhere in `collect_disc_boot_evidence`.

**TOSEC / ClrMamePro / MAME:** Generic DAT import parsers exist. MAME software lists support exists (`identity_source/mame_software_list/`), and the `neogeo` softwarelist is referenced in tests. No NEC-specific DAT parsing beyond the platform-agnostic machinery.

**Hash identity note:** The `dat::identity` layer's `IdentityKind` registry (game_identity.rs line ~265) has no `PceHuCardHash`, `PceCdDiscId`, or `PcfxDiscId` — only `SegaCdProductCode` and `PcfxDiscHash`.

## 7. CHD / OPTICAL STATUS

**CHD reading infrastructure** (`chd_logical_media.rs`, `chd_identity.rs`): Solid, reusable, fail-closed.

- `select_candidate_data_track` (line 764): picks the **lowest non-AUDIO track** — safe and platform-agnostic.
- `build_chd_track_logical_media` (line 234): restricts to **track 1 only with zero pregap** — verified by `UnsupportedTrackPosition`/`UnsupportedPregap` error variants and tests (lines 728–738, 562). This is **safe**: audio-first PC-FX CHDs (data on a later track) fail closed, which is correct behavior.
- `open_chd_iso9660` (disc_evidence_collector.rs line 145): refuses GD-ROMs via `needs_specialist_optical_backend`. Covers Sega CD/Saturn/PC-FX/PS boot evidence through `collect_disc_boot_evidence`.
- `collect_chd_evidence` (line 113): reads whole `.chd` file (bounded by `MAX_CHD_BYTES`), opens via `open_chd_iso9660`, then calls `collect_disc_boot_evidence`.

**`collect_disc_boot_evidence` (line 253):** Collects Sega CD (`SEGADISCSYSTEM`), Saturn, Dreamcast (IP.BIN), PS1/PS2/PSP (SYSTEM.CNF), 3DO (Opera header), PC-FX (magic strings), Neo Geo CD (IPL.TXT). **No PC Engine CD boot evidence exists in this function.** A PC Engine CD `.cue` or `.chd` produces no boot structure evidence — it relies entirely on folder hint + DAT hash.

**PC-FX CHD gap:** `.chd` arm of `inspect_game_identity_with_platform_trust` (game_identity.rs line 778–788) matches only `PlayStation | Saturn | Dreamcast | SegaCd`. **PC-FX is excluded** — a `.chd` file with platform "PC-FX" falls through to line 789, which sets `IdentityImageFormat::Deferred` and `IdentityStatus::Deferred` ("format has no existing safe bounded reader in EmuWiz"). The CHD reader infrastructure itself would work (track 1 only, zero pregap is safe for PC-FX data tracks); the gap is purely the **match-arm exclusion**.

**CUE/BIN support:** `cue_bin` resolver exists and is called by `discover_cue`. CUE sheets are resolved and visible as disc images. But without a PC Engine CD platform in the identity pipeline, the resolved CUE cannot be identified.

## 8. P0 / P1 / P2 BACKLOG

### P0 (blocks all NEC identity — must fix first)

1. **Register `.pce`/`.sgx` in `CONTENT_FORMATS`** — `content_registry.rs` line 74. Adding `cf("pce", ContentKind::RomCartridge)` and `cf("sgx", ContentKind::RomCartridge)` makes the discovery pipeline (`discover_direct_file`) recognize HuCard/SuperGrafx files as content instead of `UnsupportedExtension`. This is a 2-line fix that unblocks scanner visibility immediately.

2. **Add PC-FX to the PLATFORMS registry** — `platform/mod.rs` line 474. `IdentityPlatform::Pcfx` and `pcfx_boot_evidence.rs` exist and are reviewed; the platform registry has no `PC-FX` entry. Without it, `platform_for_alias("PC-FX")` returns `None` and `discover_file`'s `identity_for` can never produce a platform hint. The evidence_bridge already maps `Pcfx => "PC-FX"` — the registry just needs the entry to match.

3. **Add `IdentityPlatform::Pce` and `IdentityPlatform::PceCd`** — `game_identity.rs` line 265. Required before any identity inspection routing can work. `Pcfx` already exists; the two PCE variants are its natural siblings.

### P1 (enables real identification)

4. **Add a fusion rule for PC-FX boot magic** — `platform_evidence_fusion.rs` line 209 (`RULES` table). The `collect_disc_boot_evidence` function already produces `BootStructure = "PC-FX:Hu_CD-ROM"` evidence via `observe_pcfx_evidence`. A one-leg fusion rule (`Exact { kind: BootStructure, value: "PC-FX:Hu_CD-ROM", min_confidence: Strong } => "PC-FX"`) would let folder-less, extension-less PC-FX discs be identified from content. Without it, the boot evidence is collected but never resolved.

5. **Write `pcecd_boot_evidence.rs`** — A new module following the `segacd_boot_evidence.rs` / `neogeocd_boot_evidence.rs` pattern. PC Engine CD discs carry the `"PC Engine CD-ROM SYSTEM"` boot string at sector 0 (the string the PC-FX module's own doc comment references at pcfx_boot_evidence.rs line 30–31). This is the direct analog of Sega CD's `SEGADISCSYSTEM` signature. Needed for `collect_disc_boot_evidence` to produce platform-discriminating evidence for PC Engine CD.

6. **Add `IdentityPlatform::PceCd` to the `.cue`/`.chd`/.iso inspect arms** — `game_identity.rs` line 742–788. Add `IdentityPlatform::PceCd` (and a `inspect_pcecd_source` function) to the `.cue` arm, and to the `.chd` arm. Currently both are excluded.

7. **Add a HuCard header evidence detector for PC Engine** — `coverage_inventory.rs:428` says HuCard header inspection was "deliberately not implemented (Batch 4)." A `pce_header_evidence.rs` module (following the `nes_header_evidence.rs` / `megadrive_header_evidence.rs` pattern) would add structural content evidence for HuCard dumps. Note: No-Intro PC Engine DATs use headered dumps with a 512-byte copier header; a normalization step (like `header_normalization` for NES/Atari7800/Lynx) would be needed alongside it.

### P2 (enables full end-to-end experience)

8. **Add NEC platforms to `LAUNCH_COMPATIBILITY`** — `launch/platform_map.rs` line 82. Add rows mapping: PC Engine → `mednafen_pce_fast` / Beetle PCE, PC-FX → `mednafen_pcfx` / Beetle PC-FX, PC Engine CD → same as cartridge (HuCard + CD in same emulator) + System Card III/IV firmware requirement.

9. **Add NEC platforms to `ES_DE_SYSTEM_MAP`** — `launch/es_de_export.rs` line 111. ES-DE's system names for these: `pcengine` (PC Engine/TurboGrafx-16), `pcenginecd` (PC Engine CD/TurboGrafx-CD), `pcfx` (PC-FX). Requires a reviewed row, fail-closed currently.

10. **Add NEC platforms to the RomM outbound `STATIC_TABLE`** — `romm_platform_mapping.rs` line 100. Map: PC Engine → `pc-engine` (or `turbografx-16`), PC Engine CD → already has slug, PC-FX → `pcfx`. Currently only PC Engine CD has an outbound slug.

11. **Add NEC platforms to `FirmwareSystem`** — `firmware_evidence.rs` line 58. Add `PcEngineCd` (System Card / Arcade Card BIOS) and `Pcfx` (BIOS). Wire into `redump_bios_evidence_from_dat` for Redump BIOS DAT matching. Currently only PS/PS2/Xbox have firmware support.

12. **Add a `PcfxDiscHash` fusion rule** — Despite PC-FX content evidence existing in `collect_disc_boot_evidence`, there's no `FusionRule` entry for PC-FX in `RULES`. Without it, the evidence bridge's `IdentityPlatform::Pcfx => "PC-FX"` mapping can never fire from content evidence alone.

## 9. BEST PC ENGINE CARTRIDGE TASK (CC)

**Register `.pce`/`.sgx` in `CONTENT_FORMATS` and add `IdentityPlatform::Pce`.**

The smallest, highest-leverage task. Currently `.pce` is in `LIKELY_CONTENT_EXTENSIONS` (inspector.rs:117) but missing from `CONTENT_FORMATS` (content_registry.rs:74), causing a silent `UnsupportedExtension` skip in `discover_direct_file` (discovery.rs:579). Adding two lines to `CONTENT_FORMATS` makes PC Engine/TurboGrafx-16/SuperGrafx cartridges visible in the scanner.

After that, the platform can be distinguished by folder alias or DAT hash, but **structural HuCard-header validation should NOT be invented** — as `coverage_inventory.rs:428` correctly notes, the HuCard header is not standardized enough for the crate's two-source confidence bar. A `pce_header_evidence.rs` module should only be written after cross-referencing two independent preservation references (Mednafen source + a No-Intro/Redump technical reference) on the exact byte layout — a P1 task after the P0 registration fix.

File: `crates/archivefs-core/src/ingestion/content_registry.rs` line 86, and `crates/archivefs-core/src/game_identity.rs` line 265–287 (add `Pce` variant), 291–321 (add catalogue alias resolution), 334–348 (add display name).

## 10. BEST PC ENGINE CD TASK (CC)

**Create `pcecd_boot_evidence.rs`:** a pure, read-only module following the `segacd_boot_evidence.rs` and `neogeocd_boot_evidence.rs` pattern. PC Engine CD/TurboGrafx-CD discs carry the ASCII string `"PC Engine CD-ROM SYSTEM"` at the start of the first 2048-byte data sector (this is referenced but not checked in `pcfx_boot_evidence.rs:30–31`). The module should:

1. Define `PC_ENGINE_CD_BOOT_SIGNATURE: &[u8] = b"PC Engine CD-ROM SYSTEM"` (or the verified exact byte sequence)
2. Implement `looks_like_pcecd_boot_sector(bytes: &[u8]) -> bool` and `observe_pcecd_evidence(bytes: &[u8]) -> Vec<ContentEvidence>` producing `BootStructure` at `Strong` confidence
3. Be called from `collect_disc_boot_evidence` (disc_evidence_collector.rs line ~320, in the same `if !boot_signature_found` branch as Saturn/Sega CD/3DO/PC-FX)
4. Add a `FusionRule` entry in `platform_evidence_fusion.rs` mapping `BootStructure = "PC Engine CD-ROM SYSTEM"` → `"PC Engine CD"`

This mirrors exactly how Sega CD's `SEGADISCSYSTEM` signature is handled. The string is PCE-CD-specific (not shared with PC-FX, which uses `"PC-FX:Hu_CD-ROM"` or the PPPPHHHHHHOOOOOTTTTOOOOCCCCCDDDD secondary magic). It does NOT emit a `ProductCode` fact — the CD boot header carries no serial/catalog number that the developer's soundness bar would accept as verified identity.

**Critical safety constraint:** Do NOT hardcode a data-track selection for PC Engine CD CHDs. Verify against real specimens whether the data track is always track 1 with zero pregap. If PC Engine CD CHDs ever have audio before data, `build_chd_track_logical_media` will (correctly) refuse them — extend support only after verifying the track layout empirically.

File: new `crates/archivefs-core/src/pcecd_boot_evidence.rs`, wired into `disc_evidence_collector.rs` line 78 (import) + line ~322 (call site), plus a `FusionRule` entry in `platform_evidence_fusion.rs`.

## 11. BEST PC-FX TASK (CC)

**Add `IdentityPlatform::Pcfx` to the `.chd` inspect arm and add a PC-FX fusion rule.**

The PC-FX already has the strongest foundation of the NEC family:
- `pcfx_boot_evidence.rs` (Mednafen-sourced magic strings, disc hash, 29 tests)
- `inspect_pcfx_source` (full identity inspection via `PcfxDiscHash`)
- `collect_disc_boot_evidence` calls `observe_pcfx_evidence`
- `IdentityPlatform::Pcfx` exists with catalogue alias `"pc-fx" | "pcfx" | "nec pc-fx" | "nec pcfx"`
- `evidence_bridge.rs` maps it to platform_id `"PC-FX"`

**But three gaps remain:**

1. **PLATFORMS registry has no `"PC-FX"` entry** (platform/mod.rs). This is the P0 blocker — without it, `platform_for_alias` never resolves, and no PC-FX file can get a platform hint. The evidence_bridge's `"PC-FX"` mapping dangles.

2. **`.chd` arm excludes Pcfx** (game_identity.rs:778–788). Add `IdentityPlatform::Pcfx` to the match list so `.chd` files with a PC-FX platform hint route to `inspect_disc_chd`. The CHD reader (`open_chd_iso9660` → `build_chd_track_logical_media`) is safe for PC-FX: track-1-only, zero-pregap is correct for PC-FX disc layout (the data track IS track 1). This is the fix that makes PC-FX `.chd` go from `Deferred` to a real identity inspection.

3. **No fusion rule for PC-FX** (platform_evidence_fusion.rs RULES). The `collect_disc_boot_evidence` function produces `BootStructure = "PC-FX:Hu_CD-ROM"` evidence, but no `FusionRule` resolves it to `"PC-FX"`. Add a one-leg rule mirroring `segacd_boot_signature`.

Priority order: register `"PC-FX"` in PLATFORMS first (P0), then fix the `.chd` arm (P1), then add the fusion rule (P2). The existing `pcfx_disc_hash` is RetroAchievements-compatible and already tested via `pcfx_fixture` — no new evidence source needed; only plumbing.

Files: `crates/archivefs-core/src/platform/mod.rs` (add PC-FX Platform entry), `crates/archivefs-core/src/game_identity.rs` (line 778, add `IdentityPlatform::Pcfx` to `.chd` match), `crates/archivefs-core/src/platform_evidence_fusion.rs` (add FusionRule), `crates/archivefs-core/src/coverage_inventory.rs` (add PC-FX Coverage entry, real_validation=RealValidated since `inspect_pcfx_source` already has real-corpus tests).

## 12. EXACT FILES / FUNCTIONS

| Concern | File | Line(s) / Function |
|---|---|---|
| No `IdentityPlatform::Pce`/`PceCd` | `game_identity.rs` | 265–288 (enum), 290–348 (from_catalogue, display_name) |
| `.cue`/`.chd` exclude PCE platforms | `game_identity.rs` | 742–755 (`.cue`), 778–788 (`.chd`) |
| `.pce`/`.sgx` not in CONTENT_FORMATS | `content_registry.rs` | 74–124 (CONTENT_FORMATS table) |
| `.pce` orphaned in inspector | `inspector.rs` | 114–122 (LIKELY_CONTENT_EXTENSIONS), 148–150 (classify) |
| HuCard deliberately deferred | `coverage_inventory.rs` | 424–430 (COVERAGE entry "PC Engine") |
| No boot evidence for PCE CD | `disc_evidence_collector.rs` | 253–383 (`collect_disc_boot_evidence`) |
| PC-FX excluded from `.chd` | `game_identity.rs` | 778–788 |
| No PC-FX in PLATFORMS | `platform/mod.rs` | 474–1751 (PLATFORMS const) |
| No fusion rule for PCE/PC-FX | `platform_evidence_fusion.rs` | 209–530 (RULES table) |
| No launch compat for NEC | `launch/platform_map.rs` | 82–155 (LAUNCH_COMPATIBILITY) |
| No ES-DE for NEC | `launch/es_de_export.rs` | 111–194 (ES_DE_SYSTEM_MAP) |
| No RomM outbound for PCE/PCE-CD/PC-FX | `romm_platform_mapping.rs` | 100–268 (STATIC_TABLE) |
| PC-FX identity maps to unregistered platform | `launch/evidence_bridge.rs` | 139–163 (launch_platform_id), 331–338 (resolved_identity_for_platform) |
| No NEC firmware | `firmware_evidence.rs` | 58–62 (FirmwareSystem enum) |
| Track-1-only/pregap-zero CHD restriction | `chd_logical_media.rs` | 234–258 (build_chd_track_logical_media) |
| GD-ROM refusal | `chd_identity.rs` | 824–837 (needs_specialist_optical_backend) |
| HuCard not in content_evidence values | `content_evidence.rs` | 177–208 (value module) |
| SuperGrafx alias of PC Engine | `platform/mod.rs` | 1295–1315 (PC Engine entry), 1308 (strong_extensions) |
| TurboGrafx-16 equivalent to PC Engine | `platform/mod.rs` | 203–206 (EQUIVALENT_PLATFORM_IDS), 1706–1717 (TurboGrafx-16 entry) |
| PC Engine CD has no boot signature check | `disc_evidence_collector.rs` | 307–333 (boot signature branch) |

## 13. MATURE AREAS NOT WORTH TOUCHING

1. **`pcfx_boot_evidence.rs`** — Well-sourced from Mednafen (`pcfx.cpp`), has 29 passing tests, fail-closed parse functions, correct collision-safety separation from PC Engine CD (the doc comment at lines 27–33 explicitly distinguishes `"PC-FX:Hu_CD-ROM"` from `"PC Engine CD-ROM SYSTEM"`). Do not rewrite.

2. **`neogeocd_boot_evidence.rs`** — IPL.TXT parser cross-checked against the NeoGeo Development Wiki, 21 tests, structurally validates (terminator byte, entry count bounds, field format). Ready reference for the PCE CD module. Do not touch (it's correct; the gap is it lacks a `FusionRule` and `IdentityPlatform::NeoGeoCd`).

3. **`segacd_boot_evidence.rs`** — `SEGADISCSYSTEM` signature at offset 0, product field at `$0x180`, sourced from segaretro.org + SpritesMind + clownmdemu. 16 tests. The direct template for `pcecd_boot_evidence.rs`. Do not touch.

4. **`chd_logical_media.rs` / `chd_identity.rs` CHD track selection** — `select_candidate_data_track` (lowest non-AUDIO track) and `build_chd_track_logical_media` (track 1, zero pregap) are correct, fail-closed, and well-tested (21 tests including `unsupported_track_position_fails_closed`, `absent_trailing_hunk_bytes_read_as_zero_not_an_error`, `no_platform_inference_is_emitted`). The track-1-only restriction is correct for PC-FX and Sega CD. Do not widen this — fix the platform-specific match arms that exclude platforms instead.

5. **`platform/mod.rs` PLATFORMS entries for Sega CD, 3DO, Saturn, Dreamcast** — These are reviewed, complete, and correct reference entries. The Sega CD entry's `SEGADISCSYSTEM` magic rules are marked `Corroborated` (not `Strong`) with explicit documentation of why (line 1537–1542). Use as the exact template for a PC Engine CD entry.

6. **`collect_disc_boot_evidence`** (disc_evidence_collector.rs:253–383) — The evidence-gathering switch (SYSTEM.CNF → IP.BIN → Saturn sig → SEGADISCSYSTEM → Opera → PC-FX magic → IPL.TXT) is well-structured and safe. The gap is the missing PCE CD branch, not the existing branches. Do not refactor the existing branches.

7. **`iso9660.rs`** — Platform-agnostic ISO9660 observation. Correct and reusable. The PS1 `SYSTEM.CNF` discovery path through it is sound. Do not modify.

8. **`firmware_evidence.rs` hashing/matching** (`hash_firmware_file`, `matching_firmware_records`) — Solid `O_NOFOLLOW`, size-bounded, `Read`-streamed CRC32/MD5/SHA-1 implementation. The gap is `FirmwareSystem` only having PS/PS2/Xbox variants — add `PcEngineCd` and `Pcfx` variants, do not rewrite the hashing.

9. **RomM `STATIC_TABLE`** — The static table is correctly conservative (23 mapped entries out of 74 platforms). The `pc-fx → PC Engine` approximate inbound mapping is correctly NOT inverted. Add PC Engine and PC-FX outbound mappings, do not touch the approximate inbound entries.

---

**SNK note (discovered in-scope, not audited):** The `neogeocd_boot_evidence.rs` module and its `IPL.TXT` parser exist, are wired into `collect_disc_boot_evidence` (disc_evidence_collector.rs:63, 370–378), and the "Neo Geo CD" canonical platform is registered with folder aliases and weak extensions. However: (a) there is no `IdentityPlatform::NeoGeoCd` variant in game_identity.rs, so the `.cue`/`.iso`/`.chd` inspect arms never call it; (b) there is no fusion rule for Neo Geo CD in RULES; (c) "NeoGeo" (AES/MVS) and "Neo Geo Pocket"/"Neo Geo Pocket Color" have boot/header evidence modules (`ngp_header_evidence.rs`) and coverage entries but no IdentityPlatform variants; (d) no launch compatibility or ES-DE entries exist for any Neo Geo family platform. The full SNK audit should be run separately when requested.