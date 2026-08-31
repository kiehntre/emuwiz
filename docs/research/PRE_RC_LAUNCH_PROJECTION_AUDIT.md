# Pre-RC Launch + Projection Gap Audit (RESEARCH ONLY)

**Repo:** `/home/davedap/archivefs` · **Branch:** `feature/archivefs-unified-platform`
**Live HEAD at report time:** `cc39b3b feat(launch): add DOSBox execution` (tree clean except research docs)
**Scope:** (1) RetroArch launch-compatibility rows, (2) NeoGeo↔MAME acceptance, (3) RomM/ES-DE projection parity. No parsers re-audited beyond what launch/export safety requires. **No modifications made.**

---

## 0. LIVE-STATE FACTS THE MATRIX DEPENDS ON (all verified this pass)

- `LAUNCH_COMPATIBILITY` currently has **14 rows**: PSX, PS2, PS3, PSP, Xbox, Xbox360, GameCube, Wii, Dreamcast, Sega CD, AtariST, Amiga, Arcade, ScummVM (`launch/platform_map.rs`). Nothing else.
- `launch_readiness_page.rs` constructs launch contexts for **DuckStation, PPSSPP, RPCS3, Xemu, Xenia** (+ Dolphin/PCSX2/Flycast/RetroArch) — the GUI side is no longer the gap.
- **DOSBox landed at HEAD** (`launch/dosbox_command.rs`, `dosbox_execution.rs`) but there is **no `DOS` row** in `LAUNCH_COMPATIBILITY` — the planner exists without a route.
- `evidence_bridge.rs` launch-identity mappings exist for: MegaDrive (:148), SNES (:149), NES (:150), Game Boy (:151), GBC (:152), GBA (:153), N64 (:154), AtariLynx (:166), AtariJaguar (:167), AtariST (:168), NeoGeoCd (:161), Atari2600/5200/7800/8Bit (:162-165) — **the identity→launch mapping already covers the whole cartridge long tail**.
- `supported_loose_rom_format` Atari rows cover a26/a52/a78/atr/atx/xex/xfd — **no `lnx`, no `j64`/`jag`** (Lynx/Jaguar loose dispatch gap).
- **No `IdentityPlatform::NintendoDs`** (grep empty) despite `nds` being registered and a RomM row existing.
- `mame_command.rs`: hard gate `MAME_SUPPORTED_PLATFORM_ID = "Arcade"` (`:15`); requires exactly one DAT set verdict (`SetState::Complete`), `identity.game_key == set shortname`, dependency state permitting (BIOS included via `dat/dependency::resolve_bios`). Set shortname is the only launch authority — never a filename.
- Repo-reviewed RomM slugs (inbound `ROMM_SLUG_ALIASES`, `identity_source/romm/normalise.rs`): `acpc, c-plus-4, c16, cpc, dc, fds, gb, gba, gbc, genesis-slash-megadrive, n64, nds, neo-geo-cd, ngc(→GameCube), pc-fx, ps, psvita, sega-cd, sega32, segacd, sfam, sms, snes, turbografx-16-slash-pc-engine-cd, win, xboxone`. **This is the only repository-authoritative slug evidence** — any slug not present or in an outbound row is RESEARCH REQUIRED.

## 1. RETROARCH LAUNCH ROW MATRIX

Row-sufficiency rule: a `LAUNCH_COMPATIBILITY` row alone is sufficient **iff** an `IdentityPlatform` variant, an `evidence_bridge` mapping, and loose-file dispatch all exist (identity then resolves via folder/DAT and the generic RA chain launches it). Hints are candidates only — dynamic `.info` alias resolution (`platform_map.rs:212-248`) covers the rest, so **empty hint lists are safe**.

| System | Identity variant | Bridge map | Loose dispatch | Registry rows | Launch row | Core hints (repo evidence) | BIOS blocker | Container gate | Row alone sufficient? |
|---|---|---|---|---|---|---|---|---|---|
| NES | ✅ Nes | ✅ :150 | ✅ nes/unf/fds | ✅ | **✗** | none in repo — RESEARCH REQUIRED / empty OK | none | none | **YES** |
| SNES | ✅ Snes | ✅ :149 | ✅ sfc/smc | ✅ | **✗** | none in repo — RESEARCH / empty OK | none | copier-header normalization exists | **YES** |
| Game Boy | ✅ | ✅ :151 | ✅ gb | ✅ | **✗** | RESEARCH / empty OK | none (boot ROM optional, unmodeled) | GB/GBC conflict handled by identity | **YES** |
| GBC | ✅ | ✅ :152 | ✅ gbc | ✅ | **✗** | RESEARCH / empty OK | none | CGB-only rule landed | **YES** |
| GBA | ✅ | ✅ :153 | ✅ gba | ✅ | **✗** | RESEARCH / empty OK | none (BIOS optional, unmodeled) | complement-check evidence | **YES** |
| N64 | ✅ | ✅ :154 | ✅ z64/n64/v64 | ✅ | **✗** | RESEARCH / empty OK | none (CIC in cart) | byte-order canonical identity | **YES** |
| Mega Drive | ✅ | ✅ :148 | ✅ md/gen/smd/bin | ✅ | **✗** | RESEARCH / empty OK | none | SMD interleaving normalization | **YES** |
| Nintendo DS | **✗ no variant** | ✗ | ✗ (nds registered, no dispatch) | ✅ | **✗** | RESEARCH | **yes — DS firmware/crypto unmodeled** | nds opaque | **NO — variant + identity work first; DEFER row** |
| Atari 2600 | ✅ | ✅ :162 | ✅ a26 | ✅ | **✗** | RESEARCH / empty OK | none | none (hash-only — correct) | **YES** |
| Atari 5200 | ✅ | ✅ :163 | ✅ a52 | ✅ | **✗** | RESEARCH / empty OK | none | none | **YES** |
| Atari 7800 | ✅ | ✅ :164 | ✅ a78 (+A78 header evidence) | ✅ | **✗** | RESEARCH / empty OK | none | A78 header Strong | **YES** |
| Atari Lynx | ✅ | ✅ :166 | **✗ no `lnx` dispatch** | ✅ | **✗** | RESEARCH / empty OK | none | LNX header Strong | **row + one dispatch line** |
| Atari Jaguar | ✅ | ✅ :167 | **✗ no `j64`/`jag` dispatch** | ✅ | **✗** | RESEARCH / empty OK | none modeled | hash-only (correct) | **row + dispatch lines** |
| NGP / NGPC | **✗ no variants** | ✗ | ✗ (registered, member-parser only) | ✅ | **✗** | RESEARCH | none | header mono/color discriminator unbuilt as policy | **NO — variants + policy first** |
| WonderSwan / WSC | **✗ no variants** | ✗ | ✗ (footer parser example-only) | ✅ | **✗** | RESEARCH | none | footer checksum evidence example-only | **NO — variants + wiring first** |
| Virtual Boy | **✗ no variant** | ✗ | ✗ | ✅ | **✗** | RESEARCH | none | none | **NO — variant first (trivial)** |

**Headline:** for **NES, SNES, GB, GBC, GBA, N64, Mega Drive, Atari 2600/5200/7800**, every prerequisite exists — the missing join is *literally the table row* (hints optional). Lynx/Jaguar need one dispatch line each alongside their rows.

## 2. NEOGEO ↔ MAME

**Why NeoGeo is refused:** `build_mame_command_plan` accepts only `identity.platform_id == "Arcade"` (`mame_command.rs:15,50-57`); a `neogeo/`-folder set resolves platform `NeoGeo` (aliases `neogeomvs`/`neogeoaes` verified), hits `MamePlatformMismatch`, and blocks. There is also no NeoGeo `LAUNCH_COMPATIBILITY` row, so the RetroArch path can't pick it up either.

**Option analysis:**
- **A. Widen accepted MAME platforms to `["Arcade", "NeoGeo"]`** — correct. Neo Geo cartridge sets *are* MAME sets (MAME shortnames, neogeo.zip BIOS dependency, parent/clone semantics) — every guard the planner runs (exactly-one Complete set verdict, set-shortname == authorized identity game key, dependency/BIOS state) applies unchanged. BIOS blockers already flow via `MameDependencyBlocked`.
- **B. Map NeoGeo→Arcade at projection only** — rejected: the registry deliberately separates `NeoGeo` from `Arcade` (mutual conflicts); relabeling at launch would misreport the platform in GUI/exports and break the conflict model.
- **C. Separate NeoGeo compatibility row alone** — necessary but insufficient while the planner hard-gates; do both.
- **D.** Neo Geo **CD must stay separate** — different platform row + IPL evidence; NGCD-in-MAME is software-list territory, out of scope; the planner must keep refusing it (its platform is "Neo Geo CD").

**Exact minimal fix:**
1. `mame_command.rs`: replace the constant with `MAME_SUPPORTED_PLATFORM_IDS: &[&str] = &["Arcade", "NeoGeo"]`; both gate sites (mismatch blocker + identity filter) accept either; mismatch detail names the resolved platform.
2. `platform_map.rs`: add a NeoGeo row — `standalone_adapters: ["mame"]`, `retroarch_core_hints: ["mame", "fbneo"]` (fbneo is repo-evidenced as an SNK-capable core family via `identity_source/fbneo`; final hint list verify-at-implementation).
3. Regression tests: `neogeo`-folder MVS set with Complete verdict + matching shortname → authorized plan; Neo Geo **CD** → still refused (platform mismatch); BIOS-incomplete neogeo set → `MameDependencyBlocked`; set-name mismatch → blocked; Arcade behavior byte-identical; `snes`-folder set → still refused.

## 3. ROMM / ES-DE PROJECTION PARITY MATRIX

Slug evidence rules: ✅ = outbound row exists today; `alias` = repo-reviewed inbound slug proves the slug (safe to mirror outbound); **RESEARCH REQUIRED** = no repository evidence — do not invent.

| Platform | Identity variant | RomM outbound | ES-DE row | Slug/system evidence | Safe to add now? |
|---|---|---|---|---|---|
| PSX / PSP / PS3 / Saturn / DC / MD / GB / GBC / GBA / N64 / NDS / GameCube / 32X / MasterSystem / PCE-CD / PC | per-platform ✅ (DS: no variant) | ✅ all | ✅ all | rows exist | n/a (done) |
| **PS2** | ✅ | **✗** | ✅ `ps2` | **RESEARCH REQUIRED** (no alias/outbound evidence) | RomM row after slug verification |
| **Amiga** | ⚠ WHDLoad channel (no standard variant) | **✗** | ✅ `amiga` | **RESEARCH REQUIRED** | after slug verification |
| Atari 2600/5200/7800/8-bit/Lynx/Jaguar/ST | ✅ (all seven) | **✗ all seven** | **✗ all except `atarist`** | **RESEARCH REQUIRED** (no Atari aliases in the reviewed table) | after verification (ST ES-DE row already exists) |
| NeoGeo (MVS/AES) | ✅ | **✗** | **✗** | **RESEARCH REQUIRED** | after verification |
| Neo Geo CD | ✅ | ✅ `neo-geo-cd` | **✗** (`neogeocd` expected) | ES-DE name **RESEARCH REQUIRED** | ES-DE row after verification |
| NGP / NGPC | **✗** | **✗** | **✗** | **RESEARCH REQUIRED** | after identity variants + slug research |
| CDTV / Amiga CD32 | ✗ (platform rows only) | **✗** | **✗** | **RESEARCH REQUIRED** | defer (identity incomplete) |
| Wii U / 3DS / Switch | ✅ | **✗** | **✗** | **RESEARCH REQUIRED** | rows after slug research |
| WonderSwan / WSC / Virtual Boy | **✗** | **✗** | **✗** | **RESEARCH REQUIRED** | defer until variants |
| DOS / PC | ⚠ (DOSBox landed; DOS variant/evidence "planned") | ✅ `win`→PC | **✗** (`pc` expected) | ES-DE name **RESEARCH REQUIRED** (`win` proves the RomM side only) | ES-DE row after verification; DOS launch row only when DOS identity resolves |
| C64 / C16 / Plus-4 | ✗ (no C64 variant) | ✅ C64 row exists (verify its slug value) | **✗** (`c64` expected) | ES-DE name **RESEARCH REQUIRED** | C64 ES-DE row after verification |
| PC-98 / FM Towns / X68000 | ✗ | **✗** | **✗** | **RESEARCH REQUIRED** | defer (identity incomplete; X68000 needs registration first) |
| AtariST / Amiga ES-DE | ✅/⚠ | ST ✗, Amiga ✗ | ✅ both | rows exist | n/a (done) |

## 4. RANKED GAPS

**P0 — identified games that cannot launch/export today:**
1. **RetroArch rows for NES, SNES, GB, GBC, GBA, N64, Mega Drive, Atari 2600/5200/7800** — pure table rows; bridge/dispatch/identity all exist. Files: `launch/platform_map.rs` (+ `platform/tests.rs`/`platform_map` tests). Result in plain English: "the biggest ROM libraries in the app get a Launch button."
2. **NeoGeo MAME join** — `mame_command.rs` + NeoGeo row (§2). Result: "AES/MVS sets launch like arcade sets, BIOS-incomplete sets still blocked."
3. **PS2 RomM row** — `romm_platform_mapping.rs`; slug research required first. Result: "PS2 libraries export to RomM."
4. **Atari Lynx / Jaguar loose dispatch + rows** — `game_identity.rs::supported_loose_rom_format` (two lines) + two rows. Result: "loose .lnx/.j64 launch."

**P1 — important completeness:**
5. **ES-DE rows**: `neogeocd`, atari 2600/5200/7800/800/lynx/jaguar, `pc`, `c64`, `wiiu`/`n3ds`/`switch` — each after its name verification. Files: `es_de_export.rs`.
6. **RomM rows batch**: neogeo/ngp/ngpc, atari family, wiiu/3ds/switch, amiga, amigacd32/cdtv — after slug research.
7. **DOS launch row** — `platform_map.rs`; only once the DOS identity resolves (DOSBox adapters already landed at HEAD).
8. **Identity prerequisites**: `IdentityPlatform::{Ngp,Ngpc}` (+ mono/color policy), `WonderSwan`(+WSC), `VirtualBoy`, `NintendoDs` — each is the gate for its future row (NGP parser exists; VB needs nothing; DS needs firmware thinking first → DS is P2).
9. **Atari Lynx/Jaguar dispatch lines** (fold into P0-4).

**P2 / DEFER:** CDTV/CD32 rows (identity folder-only), PC-98/FM Towns/X68000 rows (identity incomplete; X68000 registration first), DS row (firmware unmodeled), anything slug-uncertain.

## 5. PRE-RC BATCH DESIGN

- **Batch A — RetroArch classic-console rows (P0):** `platform_map.rs` rows for NES, SNES, GB, GBC, GBA, N64, Mega Drive, Atari 2600/5200/7800 (empty or verified-only hints), **plus** Lynx/Jaguar rows with their two `supported_loose_rom_format` lines. Self-contained; no parser/identity work; regression = existing-row tests + one candidate test per row. Do **not** include DS/NGP/NGPC/WS/VB (identity prerequisites).
- **Batch B — NeoGeo MAME join (P0):** planner widening + NeoGeo row + the six regression cases in §2. Self-contained.
- **Batch C — projection rows (P0/P1):** split into **C1 (repo-evidenced):** PS2 RomM, neogeocd ES-DE, atari/pc/c64 ES-DE+RomM **only after** their names/slugs are verified against authoritative sources — no batch lands an unverified string; **C2 (deferred):** every RESEARCH REQUIRED cell above.
- No batch combines parser work with rows; no batch touches identity variants (those are separate, pre-existing-pattern tasks).

## 6. WHAT WAITS UNTIL POST-RC

DS identity/row (firmware unmodeled), NGP/NGPC/WS/VB rows (identity variants first), CDTV/CD32 rows, PC-98/FM Towns/X68000, all RESEARCH-REQUIRED slugs without verification, any new standalone emulator adapters.

**NO MODIFICATIONS MADE** — report only; live HEAD `cc39b3b`.
