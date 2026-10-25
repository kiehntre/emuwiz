# Platform Identity Gaps — Delta Audit (RESEARCH ONLY)

> **Research snapshot** — This delta audit records repository findings at the time it was written. It is not current capability documentation; see the [README](../../README.md), [adapter support matrix](../ADAPTER_SUPPORT_MATRIX.md), and [roadmap](../../ROADMAP.md) for present guidance.

**Repo:** `/home/davedap/archivefs` · **Branch:** `feature/archivefs-unified-platform` · **Tree: clean (all prior work committed)**
**Method:** delta re-verification of every family audited in this series, against the current tree. This is not a fresh survey — it measures **what still matters after the landing wave**.

---

## 0. DELTA BASELINE — WHAT HAS LANDED (verified this pass)

| Area | State now | Evidence |
|---|---|---|
| Atari extensions | **Registered everywhere** (a26/a52/a78/atr/atx/xfd/lnx/lyx/j64/jag + st/msa/ipf/stx) | `content_registry.rs`, `media_registry.rs` cf/extension lists |
| Atari identity | `IdentityPlatform::{Atari2600,Atari5200,Atari7800,Atari8Bit,AtariLynx,AtariJaguar,AtariST}`; loose-ROM dispatch rows for 2600/5200/7800/8-bit (atr/atx/xex/xfd); evidence_bridge maps 2600/5200/7800/8-bit | `game_identity.rs:321-322, :808+`, `evidence_bridge.rs:161-165` |
| Neo Geo CD | `IdentityPlatform::NeoGeoCd` + **IPL fusion rule `neogeocd_ipl_txt_boot_structure`** + evidence_bridge + `.chd` arm entry | `platform_evidence_fusion.rs:278-285`, `game_identity.rs`, `evidence_bridge.rs:161` |
| Wii U / 3DS / Switch | `IdentityPlatform::{WiiU,ThreeDS,Switch}` | `game_identity.rs` enum |
| Commodore CRT | **`disk_format/crt.rs` landed** — `DiskFormat::CommodoreCrt` → "Commodore 64", dispatched | `disk_format/mod.rs:52,191,226` |
| Commodore D64 | **`disk_format/d64.rs` landed** | `git status`/`disk_format/mod.rs` |
| ZX snapshots | `z80`/`sna`/`szx` registered (MachineSnapshot; `zx_spectrum_snapshot` module from the earlier merge) | `content_registry.rs` |
| Media registration breadth | Mac (`hfv`/`dc42`/`sit`), MSX carts (`mx1`/`mx2`), ZX disks (`trd`/`scl`), PC-98 (`d88`/`hdi`/`nhd`), D71/D81, XBE/XEX/XISO, CRT, WAD — all registered | registry lists |

**Still open from prior audits (re-verified this pass):** CPC `ZXTape!` parity (single magic hit — ZX row only); PS2 absent from the `.chd` match arm (count 0); `IdentityPlatform` still lacks Amiga/Ngp/Ngpc/FmTowns/Pc88/Pc98/X68000/VirtualBoy/Macintosh/DOS/PC; `observe_ws_evidence` still example-only; `xdf`/`dim`/`d88x`/`thd` still unregistered; no PBP/PKG identity arms; no CIC table; no DOS boot evidence; no UEF; no Mac container parser; no ATR/D88/HDI/NHD parsers (registered-only).

---

## 1. RANKED REMAINING GAPS

### P0 — small, high-leverage, blocks whole libraries

**P0-1 · Amiga — `IdentityPlatform::Amiga` + command/execution seam.**
- Strong identifier: OFS/FFS boot block + RDB (`amiga_disk`, `affs_read`), WHDLoad Slave header (name/kick CRC). Platform-only structural; title from Slave metadata (provenance); release = DAT/hash.
- In-file, two-source verified. Collisions already solved (`.hdf` X68000, `.adf` Acorn).
- Needs: identity variant (the only missing piece of the identity chain — everything else is built: profiles, kickstart, projection `AmigaGameRequest`), then `amiga_command`/`amiga_execution` (Hatari/flycast template).
- Unlocks: **launch for the entire WHDLoad/floppy/HDF library**; RomM/ES-DE `amiga` row already exists.
- Value: **P0** — biggest built-but-unlaunchable library in the repo.

**P0-2 · Amstrad CPC — `ZXTape!` signature parity (live false positive).**
- Verified still open: exactly one `ZXTape!\x1a` `MagicRule` (ZX row, Corroborated; its own comment says it proves the container, not the platform); CPC row `magic: &[]`. A real CPC `.cdt` scores *Probable ZX Spectrum* with no folder/DAT.
- Needs: one `MagicRule` on the CPC row + collision tests. No parser, no launch, no DAT changes.
- Unlocks: kills the only **confirmed live wrong-platform answer** in the tape/`family` space; makes mixed ZX/CPC folders honest (`Ambiguous`).
- Value: **P0** (Tiny).

**P0-3 · PS2 — `.chd` match-arm entry.**
- Strong identifier: ISO9660 + `SYSTEM.CNF` `BOOT2=` → verified serial + PCSX2 executable CRC (all landed). CHD is the dominant PS2 preservation container; the reader is proven on PS1/NGCD; the arm simply lacks `PlayStation2` (verified count 0).
- Needs: one match-arm entry (+ launch-eligibility decision for the PCSX2 planner, which is currently ISO-only by design).
- Unlocks: identity + serial extraction for the majority of modern PS2 sets; feeds 1G1R/RomM (`ps2` RomM row still missing — pair it).
- Value: **P0**.

**P0-4 · SNK — `IdentityPlatform::{Ngp,Ngpc}` + loose `.ngp`/`.ngc` wiring.**
- Strong identifier: NGP header — copyright-gated `Strong` (two exact SNK/licensed strings), `system_flag` mono/color discriminator (better than extension), `software_id` product code. Parser complete (14 tests), member-only.
- Needs: two identity variants, registry rows are already landed (ngp/ngc in both registries now — verified), loose-file dispatch + the GBC-`CgbEnhanced`-style mono/color policy rule.
- Unlocks: No-Intro-style DAT identity for NGP/NGPC loose files; honest mono-on-color corroboration; RomM `ngp`/`ngpc` rows.
- Value: **P0** (Small; parser exists, policy pattern exists).

### P1 — user-visible completeness

**P1-1 · PC Engine HuCard — `.pce`/`.sgx` still unregistered.**
- The one remaining NEC gap: PC-FX/PC-Engine-CD identity is mature (`PcEngineCd` + boot evidence + CHD arm); HuCard cartridges are registered nowhere. Coverage row deliberately defers a HuCard *header* parser (documented), so this is registration + DAT-hash-only by design.
- Needs: registry rows (+ `IdentityPlatform::PcEngine` variant if loose-ROM hash identity is wanted). No parser (per the coverage decision).
- Unlocks: PC Engine cartridge DAT matching. **P1** (Tiny).

**P1-2 · WonderSwan — wire the finished footer parser.**
- `observe_ws_evidence` still example-only (verified); `ws`/`wsc` now registered. Missing: `IdentityPlatform::{WonderSwan}` (also `VirtualBoy` — same shape, no parser needed) + loose dispatch + the footer-checksum family evidence.
- Unlocks: folder-less WS/WSC resolution; VB catalogue identity. **P1** (Small; parser mature).

**P1-3 · Macintosh classic containers — DC42/MacBinary/HFS signatures.**
- `hfv`/`dc42`/`sit` now registered; no `IdentityPlatform::Macintosh` variant, no parser. Disk Copy 4.2's 84-byte checksummed header and the HFS `BD`/MFS `DB` MDB sigWords are strong Mac-specific on-media identifiers (documented in the Apple audit).
- Needs: variant + `mac_disk_evidence` (DC42 + HFS/MFS signature) + MacBinary content detection. Sit/stuffit stay hash-only.
- Unlocks: Mac folder-less identity; safe HDV/HFV/HFS image classification; MacBinary `.bin` recognition without touching the shared-`.bin` denylist. **P1** (Medium).

**P1-4 · X68000 — register `xdf`/`dim`, then Human68k/partition evidence later.**
- Still unregistered (verified) despite X68000/HDF collision work being done. Registration + `IdentityPlatform::X68000` first; DIM header / XDF geometry parsing is a later Medium.
- Unlocks: catalogue + DAT for the largest Japanese-computer library. **P1** (Tiny for registration; parser P2).

**P1-5 · DOS/PC booter evidence (already planned).**
- DOS/PC identity is folder-only; the planned DOS evidence chain (FAT root-dir scan for `IO.SYS`/`MSDOS.SYS`/`COMMAND.COM`) is the right shape — keep it fail-closed (FAT alone proves nothing).
- Unlocks: DOS-vs-PC conflict resolution without folder; booter/floppy identity. **P1** (Medium).

**P1-6 · PC-88/PC-98 — D88/HDI/NHD structural evidence.**
- Now registered (landed) but parserless: registered-only. HDI/NHD 4096-byte headers are bounded, machine-neutral container evidence; D88 header/track-table likewise (serves PC-88 *and* PC-98). Generic FAT still proves nothing — keep it that way.
- Unlocks: Japanese-computer disk evidence beyond folder; multi-platform D88 ambiguity reduction. **P1** (Medium, shared adapter).

**P1-7 · ZX Spectrum disks — `trd`/`scl` registered, no parser.**
- TRDOS/SCL geometry checks are bounded and would complement the landed snapshot identity. **P2** (Small) — snapshots already carry ZX identity.

**P1-8 · Neo Geo MVS/AES / Arcade launch path (out of SNK scope, blocking NeoGeo).**
- Set identity + BIOS dependency graph are mature; no MAME/FBNeo launch adapter or Arcade/NeoGeo launch rows exist. Build once, generically. **P1** (Medium; Arcade-family task).

### P2 — polish / research-gated

- **N64 CIC table** (planned): bootcode-hash → CIC variant, enabling CRC validation; facts only, never title. Research-gated on a verified CIC/bootcode table. **P2**.
- **PS3 PKG / PSP PBP identity arms** (planned next): parsers already exist (`ps3_disc_evidence::parse_pkg_header`, `psp_pbp_evidence` complete); the work is registry/identity-arm wiring — effectively **P1-sized** when scheduled; listed here because they were declared "planned next" and nothing blocks them.
- **Atari ST identity wiring**: `AtariST` variant exists but has zero bridge/loose-dispatch mapping; `.st`/`.stx`/`.msa` flow via detection/Hatari only. **P1** (Tiny) — fold into the Atari polish pass.
- **Acorn**: `ssd`/`dsd`/`adl`/`uef` registered; DFS-catalogue and UEF parsers are new work; family-level evidence is already honest. **P2** (Medium).
- **Jaguar**: variant exists; encrypted-boot reality means DAT/hash-only stays correct; optional reversible J64 header-strip is research-gated. **P2/research-only**.
- **3DO**: **complete** (variant + Opera evidence + CHD arm + RomM/ES-DE). Nothing to do.
- **Sega cartridge/optical edge cases**: TMR-Sega checksum parser, disc stack, Saturn/DC/SegaCD/GG/SMS identity all mature; SG-1000/SC-3000 have no rows (verify demand before adding). **P2/research-only**.
- **ColecoVision `.col` / Vectrex `.vec` / Intellivision**: still unregistered (`col`/`vec` absent from registries; `int` inspector-only) — same one-row pattern as PCE. **P2** (Tiny each).
- **WAV/VOC/PZX/CSW tape**: correctly unclaimed; sample-audio stays out of core. ** research-only / low value**.
- **ATX/IPF**: deliberately deferred (VAPI review / SPS licensing). **Intentionally unsupported.**

## 2. STRONGEST IDENTIFIER / WHAT-IT-PROVES / COLLISION TABLE (condensed)

| Platform/format | Strongest on-media identifier | Distinguishes | Collision-prone exts | FP risk | Remedy |
|---|---|---|---|---|---|
| Amiga | OFS/FFS boot+root, RDB, WHDLoad slave | platform (+install title as provenance) | `adf`(Acorn), `hdf`(X68000) | solved by content | variant + launch seam |
| CPC tape | `ZXTape!\x1a` (shared with ZX — Corroborated) | container only | `cdt`/`tzx` identical bytes | **live FP today** | signature parity |
| PS2 | `SYSTEM.CNF BOOT2=` serial + ELF CRC | platform + serial + exact ELF | `iso`/`chd` shared | low (folder separates gens) | CHD arm |
| NGP/NGPC | copyright 2-string gate + `system_flag` + `software_id` | platform + mono/color + product code | `ngp` weak on both rows | low once wired | variants + policy rule |
| PC Engine HuCard | none (deliberate) | — | `pce` unclaimed by others | none | registration + DAT |
| WonderSwan | footer checksum (whole-file) | family | `ws`/`wsc` mutual-weak | low | variant + wiring |
| Mac | DC42 header checksums; HFS `BD` MDB | platform + image name (provenance) | `img`/`hfv` generic | low | parser + variant |
| PC-98/PC-88 | HDI/NHD headers; D88 track table | container only | `d88`(PC-88+PC-98), `hdi`(PC-98 vs generic) | medium — folder needed | shared bounded parser |
| X68000 | (future) DIM header/XDF geometry | container only | `xdf`/`dim` unregistered; `hdf` solved | low today (nothing claimed) | registration first |
| DOS/PC | DOS boot-file set in FAT root | OS family | `img`/`ima` generic | FAT-alone trap (documented) | planned DOS chain |
| Acorn | DFS catalogue (family-level) | family | `ssd` BBC↔Electron | by design | defer |
| N64 | CIC bootcode hash (future) | CIC variant only | byte-order (solved) | CIC≠title | planned table |

## 3. PRE-ALPHA/BETA vs WAIT

**Do before alpha/beta** (each small, each unblocks a library or kills a live wrong answer): P0-1 Amiga variant+seam; P0-2 CPC parity; P0-3 PS2 CHD arm (+ RomM `ps2` row); P0-4 NGP/NGPC; P1-1 `.pce` registration; P1-2 WonderSwan/VirtualBoy wiring; P1-4 X68000 registration; Atari ST bridge/loose rows (Tiny); PS3 PKG + PSP PBP arms (parsers exist).

**Safe to wait:** FM Towns identity (folder+Redump works), PC-98/PC-88/D88 parsers, Mac parsers, DOS chain details beyond the planned scan, Acorn UEF/DFS, N64 CIC, X68000 geometry, Arcade/MAME launch path, everything P2/research-only above.

**Leave alone (verified mature):** 3DO; Sega disc stack + TMR-Sega; GC/Wii container identity + Dolphin chain; SNES/GB/GBA/N64 header+normalization stack; PS1/PS3/PSP serial spine; Xbox XBE/XEX/STFS; Neo Geo CD IPL; Atari A78/Lynx parsers + new D64/CRT disk-format work; the optical/CHD track rules; the DAT/1G1R/companion machinery.

---

## 4. THE FIVE MOVES THAT MATTER MOST

1. **`IdentityPlatform::Amiga`** — one variant stands between the repo's deepest content subsystem (WHDLoad) and a launchable library.
2. **CPC `ZXTape!` parity** — the only confirmed live wrong-platform answer; Tiny.
3. **PS2 `.chd` arm** — the dominant PS2 container is still Deferred; one entry.
4. **NGP/NGPC variants + mono/color policy** — finished parser, missing three rows of wiring.
5. **PBP/PKG arms** — parsers exist; schedule the "planned next" pair and PSP/PS3 digital content becomes first-class.

Everything else in this audit can safely trail an alpha: the identity spine (variants → registry → evidence → bridge → launch) is now a proven, repeatable pattern, and the remaining gaps are instances of it, not new architecture.
