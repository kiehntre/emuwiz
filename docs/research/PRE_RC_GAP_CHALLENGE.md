# Pre-RC Gap Challenge — EmuWiz (RESEARCH ONLY)

**Repo:** `/home/davedap/archivefs` · **Branch:** `feature/archivefs-unified-platform` · tree clean except one file
**Question:** does the current pre-RC shortlist miss any genuinely high-value **user-facing** gap?
**Method:** re-verified the live state of every projection/launch/Doctor surface this pass (`es_de_export.rs`, `romm_platform_mapping.rs`, `launch/platform_map.rs`, `launch/` module list, `launch_readiness_page.rs`, `diagnostics/profiles.rs`, `game_identity.rs` inspect arms).

---

## 1. WHAT HAS QUIETLY LANDED (verified — removes some assumed gaps)

- **GUI launch contexts for all five standalone adapters**: `launch_readiness_page.rs` now constructs `DuckStationLaunchRequest` (:1369), `PpssppLaunchRequest` (:1394), `Rpcs3LaunchRequest` (:1416), `XemuLaunchRequest` (:1438), `XeniaLaunchRequest` (:1462). The old broken-joins #1 is **fixed** — DuckStation/PCSX2/Flycast/RetroArch/Xemu/Xenia all have Launch buttons.
- **MAME/Arcade launch landed**: `launch/mame_command.rs` (`MAME_SUPPORTED_PLATFORM_ID = "Arcade"`, DAT-set-name-gated) + `mame_execution.rs` + an Arcade `LAUNCH_COMPATIBILITY` row with `mame`/`mame2003_plus`/`fbneo` core hints (`platform_map.rs:157-158`).
- **ES-DE map expanded to 16 rows**: PSX/PS2/PS3/PSP/Xbox/360/GC/Wii/DC/Saturn/SegaCD/**AtariST**/**Amiga**/NES/SNES/MegaDrive.
- **RomM outbound expanded to 27 rows**: adds **Amstrad CPC, Commodore 64, Sega CD** to the earlier set.
- **PBP/PKG identity arms landed**: `game_identity.rs:926` (`"pbp"`), `:880` (`"pkg"`) — the planned-next pair is in.
- Atari identity variants + loose dispatch + bridge rows (verified previously) are in.

## 2. CHALLENGE RESULT — gaps the shortlist misses

### NEW P0-class findings

**N1 · RetroArch launch rows for the cartridge-console long tail (incl. MegaDrive).**
- Pain: a normal user's largest libraries (NES, SNES, GB/GBC, GBA, N64, MegaDrive, DS, Atari carts, NGP/NGPC, WonderSwan/VB) now **scan and identify** but have **no `LAUNCH_COMPATIBILITY` row** — `launch_compatibility_for_platform` returns nothing, so no launch candidate is ever produced. Identity without a Launch button.
- Existing capability: identity variants exist (most families), `spawn_retroarch` is production-grade, RA `.info` alias resolution already names every one of these systems.
- Missing join: rows + core hints (`mesen/snes9x/mgba/mupen64plus/genesis_plus_gx/…`) in `launch/platform_map.rs`. **MegaDrive — a top-tier platform — has no row at all.**
- Size: Tiny per row (one batch). Safety: none (hints are candidates, never auto-select).
- **Verdict: P0 — the single biggest missing user-facing join left.**

**N2 · NeoGeo MVS/AES launch vs the Arcade-gated MAME adapter.**
- Pain: an AES/MVS set in a `neogeo` folder resolves platform `NeoGeo` (aliases verified), but `mame_command.rs` refuses any platform that is not `Arcade` (`:56` "resolved platform is …, not Arcade"), and `NeoGeo` has no launch row. Neo Geo users cannot launch what Arcade users can.
- Missing join: either widen the MAME adapter's accepted platforms to `["Arcade","NeoGeo"]` or add a NeoGeo row — one Tiny change, reusing the just-landed adapter.
- **Verdict: P0 (Tiny).**

**N3 · Projection rows for landed identity variants (RomM/ES-DE batch).**
- Pain: whole platforms with finished identity still fail closed at export: RomM missing **PS2, Amiga, Atari 2600/5200/7800/8-bit/Lynx/Jaguar/ST, NeoGeo, NGP, NGPC, CDTV, AmigaCD32, WiiU, 3DS, Switch, WonderSwan, VB, PC-98, FM Towns, X68000**; ES-DE missing **neogeo/neogeocd/ngp/ngpc, atari2600/5200/7800/800/lynx/jaguar, amigacd32, cdtv, wiiu, n3ds, switch, pc (DOS), c64, pc98, fmtowns, x68000**.
- Existing capability: the row patterns are proven (`neo-geo-cd`, CPC/C64 rows landed this wave); identity variants exist for most.
- Missing join: table rows only. **Verdict: P1 as one batch task** (Tiny per row; slug/fullname verification discipline per the module's own rules). Highest-value subset: **PS2 RomM, neogeo/neogeocd/ngp/ngpc (both), atari rows (both), amigacd32/cdtv (both), wiiu/n3ds/switch (both), pc (ES-DE), c64 (ES-DE)**.

**N4 · Doctor adapters for Hatari and Amiga/WHDLoad.**
- Pain: TOS health (`HatariTosHealth`) and Kickstart state (`AmigaKickstartState`) are computed and tested but never surface in Doctor — "why won't my ST/WHDLoad game launch" is unanswerable in the GUI.
- Missing join: `diagnostics/profiles.rs` adapters (zero hatari/amiga references — re-verified this pass).
- **Verdict: P1 (Small each).**

**N5 · Hatari and Amiga command/execution adapters.**
- Pain: `LAUNCH_COMPATIBILITY` promises `standalone_adapters: ["hatari"]` and `["amiga_whdload"]`, projections exist (`project_hatari_launch_input`, `project_amiga_whdload_launch_input`), but `launch/` contains **no** `hatari_command`/`hatari_execution`/`amiga_command`/`amiga_execution` (verified against the module list this pass). ST and WHDLoad users identify fine and cannot launch.
- Missing join: the flycast/mame template (Amiberry/FS-UAE and Hatari argv), plus GUI contexts (now a proven pattern).
- **Verdict: P1 (Medium each).** Amiga was already shortlisted ("launch seam") — this confirms it and adds Hatari as its sibling.

**N6 · Multi-disc m3u launch (PS1/PS2-class).**
- Pain: a 3-disc PS1 release elects as one release but the DuckStation planner receives a single content path; no `.m3u` composition exists in the launch layer. Users with verified multi-disc releases launch disc 1 only, manually.
- Existing capability: `MultiDiscSet` grouping + companions exist; planners are per-file.
- **Verdict: P1/P2 boundary** — real pain, but multi-disc *safety* is listed as landing; the launch-side m3u composition is the remaining half. Post-RC acceptable; pre-RC if cheap.

### Confirmed non-gaps (verified fixed — do not re-add to plans)
GUI launch contexts (N/A now), MAME/Arcade launch, PBP/PKG identity arms, Amiga ES-DE row, AtariST/Amiga ES-DE rows, CPC/C64/SegaCD RomM rows.

## 3. RANKING THE EXISTING SEVEN CANDIDATES (by user impact)

1. **PS2 CHD identity + RomM PS2** — largest library, dominant container still Deferred, RomM export missing. (RomM half re-confirmed missing this pass.)
2. **Amiga typed identity + launch seam** — whole platform identifies (now via WHDLoad evidence) but cannot launch; deepest finished stack without a Launch button.
3. **CPC `ZXTape!` parity** — the one **live wrong-platform answer** (verified still one-sided this pass). Tiny.
4. **NGP/NGPC wiring** — finished discriminating parser (mono/color), no variants → member-only limbo.
5. **PC Engine HuCard registration** — Tiny; unlocks DAT matching for a classic library (identity variant optional, DAT-hash-only by design).
6. **Macintosh DC42** — Medium parser; Mac libraries are real but smaller; DC42 checksums are a genuinely strong identifier.
7. **PASTI stale fixture** — hygiene; no user pain.

## 4. REVISED PRE-RC SHORTLIST

**Tier A — pre-RC (small, unblocks libraries / kills wrong answers):**
1. N1 RetroArch launch rows (cartridge long tail + MegaDrive) — batch.
2. P0-3 PS2 `.chd` arm + RomM `ps2`.
3. P0-1 Amiga identity + command/execution + GUI context.
4. N2 NeoGeo↔MAME acceptance (Tiny).
5. CPC `ZXTape!` parity (Tiny).
6. N3 projection-row batch (PS2/amiga already there for ES-DE; add RomM/ES-DE for neogeo family, atari family, wiiu/n3ds/switch, amigacd32/cdtv, pc/c64).
7. NGP/NGPC wiring; PCE HuCard registration (both Tiny/Small).

**Tier B — pre-RC if time allows:** N4 Doctor adapters (Hatari, Amiga), N5 Hatari command/execution, Mac DC42, PASTI fixture.

**Safe to wait (post-RC):** WiiU/3DS/Switch emulators (Cemu/Citra-lineage/Ryujinx — Large), Mac parsers beyond DC42, PC-98/FM Towns/X68000 parsers, Acorn UEF/DFS, tape parsers (TZX/CDT/TAP/CAS family), CSW/PZX, N64 CIC, Jaguar/J64, ATX/IPF, SG-1000, Coleco/Vectrex/Intellivision rows, multi-disc m3u launch composition (N6) if not cheap.

## 5. FINAL ANSWER

- **Missing P0s beyond the seven:** N1 (cartridge-long-tail RetroArch launch rows + MegaDrive), N2 (NeoGeo↔MAME), and the projection-row batch N3 (it is the same "identity landed, projection missing" defect that every family audit ended with — now the dominant remaining pattern).
- **Missing P1s:** N4 (Doctor Hatari/Amiga), N5 (Hatari command/execution — sibling of the shortlisted Amiga seam), N6 (multi-disc m3u launch, borderline).
- **Revised shortlist:** Tier A above (seven of the original candidates survive — PS2, Amiga, CPC parity, NGP/NGPC, PCE, Mac, PASTI — re-ordered with PS2 first; plus N1, N2, N3 as the newly discovered joins).
- **Safe to wait:** everything in Tier B's remainder and the post-RC list; none of them blocks scanning, identity, rename, Playing Library, RomM/ES-DE for the platforms users actually have.
- **No code changes made.**
