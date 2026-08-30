# Modern Nintendo Support Audit — EmuWiz (RESEARCH ONLY)

**Scope:** GameCube · Wii · Wii U · 3DS · Switch — Dolphin · Cemu · Citra/Lime3DS · Ryujinx
**Branch:** `feature/archivefs-unified-platform`
**Method:** static source analysis only — no source modified, no commits.
**Companion:** `docs/research/BROKEN_JOINS_AUDIT.md` (GUI-launch finding re-verified for §J).

---

## A. PLATFORM MODEL

`platform/mod.rs`:

| | GameCube (`:1194-1211`) | Wii (`:1238-1255`) | WiiU (`:1256-1268`) | Nintendo 3DS (`:1080-1092`) | Switch (`:1212-1237`) |
|---|---|---|---|---|---|
| display | Nintendo GameCube | Nintendo Wii | Nintendo Wii U | Nintendo 3DS | Nintendo Switch |
| aliases | `gamecube`, `nintendogamecube`, `gcn`, `gc`, `ngc` | `wii`, `nintendowii` | `wiiu`, `nintendowiiu` | `nintendo3ds`, `n3ds`, `new3ds` | `switch`, `nintendoswitch` |
| strong ext | `gcm`, `gcz`, `rvz` | `wbfs`, **`wad`** | `wud`, `wux`, `rpx` | `3ds`, `cia`, `cci`, `cxi` | `xci`, `nsp`, **`nca`** |
| weak ext | `iso`, `ciso`, `zip` | `iso`, `gcz`, `rvz`, `ciso`, `wia`, `zip` | `iso`, **`wad`**, `zip` | `zip` | `zip` |
| magic | **`0xC2339F3D` @ 0x1C — Strong** | **`0x5D1C9EA3` @ 0x18 — Strong** | none | none | none |
| conflicts | Wii | GameCube | — | — | — |

- The GC/Wii magic pair is the family's structural backbone: "the disc magic word is what separates a GameCube `.iso` from a Wii one" — `.iso` is weak on both rows and **never** self-identifies.
- `IdentityPlatform::GameCube`/`Wii` exist; **no `WiiU`, `ThreeDS`, or `Switch` variants** (`game_identity.rs:265-288`) — identity eligibility is literally documented as "currently supports PS2, GameCube, and Wii" (`:750`).
- ES-DE: `gc` + `wii` rows only (`launch/es_de_export.rs:143-151`, incl. the reviewed "gc, not gamecube" note at `:470-473`); **no WiiU/3DS/switch rows**. RomM outbound: **GameCube → `ngc` only** (`romm_platform_mapping.rs:158-163`); **no Wii/WiiU/3DS/Switch rows**. `LAUNCH_COMPATIBILITY`: GameCube + Wii rows only. coverage_inventory: GameCube (`:245`) + Wii (`:258`) rows only.
- **Drift:** WiiU's weak extensions include **`wad`** — a Wii (and Virtual Console) format, not Wii U media; and Wii's *strong* `wad` has no identity arm anywhere (§B). Both are registry-table errors, not gaps.

## B. GAMECUBE / Wii FORMATS

**Structural evidence is genuinely deep and production-wired:**
- `gamecube_wii_boot_evidence.rs` — `observe_gc_wii_disc:129` parses the 0x400-byte disc header (game ID, disc/version bytes, magics at 0x18/0x1C), **Wii partition tables** (`WiiPartitionFact:83`), and the data partition's **apploader (with decoded date string), FST, and `main.dol` presence** (`DataPartitionMetaFact:94-103`) — "never decrypted". Called from production at `disc_evidence_collector.rs:272`.
- Identity arms (`game_identity.rs`): `iso|gcm` (`:801`), **`rvz` (GC|Wii, `:814`)**, **`ciso` (GC|Wii, `:817`)**, **`wbfs` (Wii, `:820`)** — real container parsers with fixtures (`:6212-6340`). `zip` generic.
- **`gcz`** — a GameCube *strong* extension — falls into the `Deferred` catch-all (`:837`) with no dedicated reader. **`wad`** — a Wii *strong* extension — has **no arm at all** (`Unsupported`).
- **Game ID spine:** `IdentityKind::DolphinGameId` (plus `DolphinRevision`, `DolphinDiscNumber`, `DolphinRegion`) verified from disc content, getter `:535`, provenance documented (`:196-202`-region of the enum).
- WIA/NKit/trimmed dumps: absent (`wia` weak-ext only, no parser). Extracted FST folders: absent (the evidence module documents the concepts but no folder-layout parser exists).

**What proves what:** platform = disc magic word (Strong, byte-verified); Game ID = `DolphinGameId` from the disc header (ExactBytes-grade, consumed downstream); release identity = whole-image hash via generic DAT. Three concepts, cleanly separated.

## C. DOLPHIN — end-to-end (mature; do not rewrite)

| Stage | State | Evidence |
|---|---|---|
| Detection | ✅ | `patch_manager/dolphin_*` + `diagnostics/profiles.rs`; native/Flatpak/AppImage handled in the profile layer (AppImage naming in `emulator_environment/es_de/tests.rs`) |
| Readiness | ✅ | `FirmwareReadiness` projections; `launch/readiness.rs` |
| Planning | ✅ | `dolphin_command.rs` — native slice accepts **only direct `.iso`/`.gcm`** (`DOLPHIN_SUPPORTED_EXTENSIONS:41`; "RVZ/CISO/WBFS and any archive/mount-input format are refused", `:40`); requires a verified Dolphin Game ID (`game_id: resolved.game_key`, `:241`) |
| Execution | ✅ | `dolphin_execution/` + `preflight_and_launch_dolphin` (`launch_readiness_page.rs:60`), `DolphinDirectoryIdentity` (device/inode discipline, `launch/execution.rs:105-127`) |
| GUI Launch | ✅ | **one of the four wired GUI contexts** (broken-joins audit `:99` — Dolphin is in) |
| Cheats/Mods | ✅ deep | `bsfree_gamecube`/`bsfree_wii` (Gecko/AR pipelines, end-to-end tests), `gamehacking_gamecube_{provider,install_plan}`, `gamehacking_wii_provider` (`WiiGameIdentity::from_report:88` — **reuses the same identity report**, `verified_game_id:180`, `normalize_wii_game_id:187`); texture packs via GUI `dolphin_texture_mod_page.rs` |
| NAND | not modeled in the launch slice | no NAND references in `dolphin_command.rs`; game-path validation is the planner's container check. Honest absence. |

No duplication: the cheats/mods stack consumes `verified_value(IdentityKind::DolphinGameId)` from the one identity pipeline — same shape as PCSX2/Xenia.

## D. WII U FORMATS

Platform row only. `wud`/`wux`/`rpx`/`wua`/`app.xml`/`cos.xml`/`meta.xml`/Title-ID/version/region: **no parser, no registry row, no IdentityPlatform variant, no coverage row** (grep for `wud|wux|wua|rpx` outside the platform row: empty). `wua` isn't even an extension on the row. Update/DLC layouts: absent. Everything in this section is **absent**, not deferred.

## E. CEMU

**Absent.** Zero references to Cemu in `launch/`, `patch_manager/`, `emulator_environment/`, or GUI (the `-l` hits were substrings elsewhere). No detection, readiness, planning, execution, mlc01/keys.txt/gameProfiles/graphic-packs modeling. Nothing is invented here — per the task, none of it is modeled, and none of it is claimed.

## F. 3DS FORMATS

Platform row claims `3ds`/`cia`/`cci`/`cxi` strong — but **none are registered in `media_registry` or `content_registry`**; `3ds`/`cia` appear only in `inspector.rs:116` (LikelyContent labels). **No NCSD/NCCH/ExHeader/ExeFS/RomFS parser exists** (`rg -i ncch|ncsd|exefs|romfs` across `crates/`: empty). Decrypted-vs-encrypted content: not modeled. Title ID/Program ID/product code/region/version: no parser, no facts. `cci`/`cxi`: platform-row only.

## G. 3DS KEYS / FIRMWARE

**Nothing modeled** — no AES keys, seeddb, movable.sed, system-archive, firmware, or title-key concepts anywhere (grep: empty). Correctly no fake readiness gate; also nothing to leak. The `FirmwareSystem` enum remains PS/PS2/Xbox only.

## H. 3DS EMULATOR SUPPORT

**Absent.** No Citra, Lime3DS, or any 3DS-profile adapter exists in `launch/`, `patch_manager/`, `emulator_environment/`, or GUI (zero name references). The Citra→Lime3DS naming-drift question is moot in-repo: there is nothing to drift. (No dead/legacy adapters either.)

## I. SWITCH FORMATS

Platform row claims `xci`/`nsp`/`nca` strong — **none registered** in any scanner registry (not even inspector). **No XCI/NSP/NSZ/XCZ/NCA/NRO/NSO/CNMT/PFS0 parser exists** (grep: empty). Cartridge-vs-digital, Title ID, version, update/DLC: nothing parsed, nothing modeled.

## J. SWITCH KEYS / FIRMWARE

**Nothing modeled** — no prod.keys/title.keys/firmware/key-generation concepts anywhere. No Doctor states to distinguish because no key/firmware surface exists. Correctly nothing bundled or sourced.

## K. SWITCH EMULATOR SUPPORT

**Absent.** No Ryujinx, Yuzu-derived, or other Switch-profile adapter anywhere (zero references in `launch/`, `patch_manager/`, `emulator_environment/`, GUI). No dead/legacy adapters to clean.

## L. TITLE ID / GAME ID SPINE

| Platform | Fact kind | Source | Wired? |
|---|---|---|---|
| GameCube | `DolphinGameId` (+Revision/DiscNumber/Region) | disc header via `inspect_direct_iso`/container arms | ✅ production; consumed by gamehacking/bsfree + Dolphin planner |
| Wii | same kinds | disc header/partitions | ✅ same |
| Wii U | none | — | no `IdentityPlatform` variant |
| 3DS | none | — | no variant |
| Switch | none | — | no variant |

Broken joins: none *within* GC/Wii (the spine is complete: parser → identity → planner → cheats); the break is that three of five platforms have no spine at all — `from_catalogue` maps their names to `Other`, and `inspect_game_identity` refuses with "shared identity inspection currently supports PS2, GameCube, and Wii" (`game_identity.rs:748-756`).

## M. DAT / PRESERVATION

- GC: Redump (disc) — generic DAT machinery; RVZ/CISO/GCZ are Dolphin-derived containers, hashed as-is; no normalization claims.
- Wii: Redump (disc) + WBFS as a *container* (hashed as-is); WIA/NKit absent.
- Wii U / 3DS / Switch: No-Intro-class ecosystems in the wild; in-repo support = **none** (nothing ingested to hash).
- Stale handling/multi-content sets: generic machinery only. **No platform-specific DAT plumbing exists or is warranted.**

## N. UPDATES / DLC

Nothing modeled for any of the five (GC/Wii have no update/DLC content class in-scope; WiiU/3DS/Switch have no parsers to extract title IDs/content metadata from). The reusable generic seams for a future model: `dat/dependency/clone_report.rs`, `dat/set.rs` MemberClass/loadflags, and the STFS-style "raw field, never interpreted" precedent from the Xbox audit. Not built here.

## O. MODS / PATCHES / CHEATS

- **Dolphin (GC/Wii):** the deepest mods story in the repo — Gecko/AR via bsfree (with catalogue, install plans, rollback, end-to-end tests), gamehacking providers for GC and Wii (identity-reusing), texture packs (`dolphin_texture_mod_page.rs`), cheats/mods preview and shared transaction machinery (`patch_manager/shared_preview.rs`, `shared_transaction.rs`). Riivolution: absent.
- **Cemu / 3DS / Switch:** absent (no emulators, no mod formats modeled).

## P. MULTI-DISC / MULTI-CONTENT

- GC/Wii multi-disc: generic DAT grouping + `DolphinDiscNumber` fact; election risk same shape as Sony/Xbox (disc-count awareness untested for this family — no test found).
- WiiU/3DS/Switch base/update/DLC: nothing to group (no parsers).
- RomM/ES-DE projection: GC/Wii via generic paths (`ngc`/`gc`); others unmapped.

## Q. DOCTOR

**Can report today:** Dolphin/PCSX2/Flycast/RetroArch-class emulator/profile findings; identity-unresolved/conflict blockers for GC/Wii; unsupported-container states (gcz Deferred, wad Unsupported) surface as identity statuses. **Cannot report:** WiiU/3DS/Switch anything (no identity/adapter layer); missing keys/firmware (nothing modeled); missing base game/update compatibility (nothing modeled). Informational-vs-repairable separation already exists via the blocker/status vocabulary.

## R. SECURITY / LEGAL BOUNDARIES

- `.iso` never proves GC or Wii — both rows require their byte-verified magic words (`Strong`), and the evidence module documents that apploader/FST/main.dol are shared concepts while the header magic decides (`gamecube_wii_boot_evidence.rs:43-45`).
- No key/firmware material is modeled, bundled, hashed, or sourced anywhere in the family — the strongest legal posture of any audited family, simply because the surface doesn't exist yet.
- No filename-derived Title IDs/version claims (nothing reads them at all).
- Dolphin launch slice refuses archives/mount-inputs rather than approximating (`dolphin_command.rs:40`).

## S. TEST COVERAGE

Present: `gamecube_wii_boot_evidence` (header/magic/partition/apploader/FST), GC/Wii identity fixtures (gcm `:6212`, rvz `:6235`, ciso `:6340`), Dolphin execution/planning tests, bsfree GC/Wii end-to-end tests (`tests/bsfree_*`), gamehacking provider tests, Dolphin GUI launch tests, `es_de_export` gc/wii mapping tests.
**Missing:** `.gcz` Deferred behavior is loop-tested (`:6686` includes `"gcz"`) but no dedicated reader exists; `wad` unsupported-state test; **everything WiiU/3DS/Switch** (no tests because no code); multi-disc election test for GC/Wii; WiiU `wad`-drift guard.

## T. MATURITY MATRIX

| | GC | Wii | WiiU | 3DS | Switch |
|---|---|---|---|---|---|
| Platform registry | MATURE | MATURE | PARTIAL — row exists but `wad` weak-ext is a Wii format | MATURE | PARTIAL — `nca` (an internal content format, not a user media file) as a *strong extension* is a category error |
| Media registration | MATURE | PARTIAL — `wad` strong with no reader; `wia` weak, unparsed | **ORPHANED** — `wud`/`wux`/`rpx` in no registry | **ORPHANED** — `3ds`/`cia` inspector-only; `cci`/`cxi` nowhere | **ORPHANED** — `xci`/`nsp`/`nca` nowhere |
| Structural evidence | MATURE (magic+header+partitions+apploader) | MATURE (same) | MISSING | MISSING | MISSING |
| Stable game/title ID | MATURE (`DolphinGameId`) | MATURE | MISSING (no `IdentityPlatform` variant) | MISSING | MISSING |
| Exact DAT/hash identity | MATURE | MATURE | MISSING (not ingestible) | MISSING | MISSING |
| Persistence | MATURE | MATURE | MISSING | MISSING | MISSING |
| Keys/firmware | N/A (none needed) | N/A | MISSING | INTENTIONALLY UNSUPPORTED (nothing modeled; nothing to leak) | INTENTIONALLY UNSUPPORTED |
| Emulator discovery | MATURE (Dolphin) | MATURE (Dolphin) | MISSING (no Cemu) | MISSING (no Citra/Lime3DS) | MISSING (no Ryujinx) |
| Readiness | MATURE | MATURE | MISSING | MISSING | MISSING |
| Planning | MATURE (iso/gcm only, honest) | MATURE (iso/gcm only) | MISSING | MISSING | MISSING |
| Execution | MATURE | MATURE | MISSING | MISSING | MISSING |
| GUI launch | MATURE (wired) | MATURE (wired) | MISSING | MISSING | MISSING |
| Doctor | MATURE | MATURE | MISSING | MISSING | MISSING |
| Cheats | MATURE (bsfree GC) | MATURE (bsfree Wii + gamehacking) | MISSING | MISSING | MISSING |
| Mods | MATURE (texture packs) | PARTIAL (no Riivolution) | MISSING | MISSING | MISSING |
| Updates | N/A | N/A | MISSING | MISSING | MISSING |
| DLC | N/A | N/A | MISSING | MISSING | MISSING |
| Rename | MATURE | MATURE | MISSING (nothing to rename) | MISSING | MISSING |
| Duplicates | MATURE | MATURE | MISSING | MISSING | MISSING |
| 1G1R | MATURE | MATURE | MISSING | MISSING | MISSING |
| Playing Library | MATURE | MATURE | MISSING | MISSING | MISSING |
| RomM | MATURE (`ngc`) | PARTIAL — no outbound row | MISSING | MISSING | MISSING |
| ES-DE | MATURE (`gc`) | MATURE (`wii`) | MISSING | MISSING | MISSING |
| Multi-content grouping | PARTIAL — election risk untested | PARTIAL — same | MISSING | MISSING | MISSING |

## U. BROKEN JOINS

1. **`gcz` is a GameCube strong extension that identity refuses** — a `Deferred` catch-all entry (`game_identity.rs:837`) where a bounded GCZ reader would complete the chain (the RVZ/CISO readers prove the pattern).
2. **WiiU/3DS/Switch rows have no `IdentityPlatform` variants** — platform registry and identity enum disagree; even perfect media registration today would dead-end at `from_catalogue → Other`.
3. **`3ds`/`cia` Inspector LikelyContent vs zero registration** — the three-registry drift already diagnosed for `.pce`/`.fds`/`.pbp`, now on 3DS.
4. **`wad` claims on two rows, zero support** — Wii strong (no arm) and WiiU weak (wrong family); both rows need the claim corrected or an arm built.
5. **RomM/ES-DE missing rows for Wii** — Wii is fully mature backend-to-GUI yet cannot export to RomM (`wii` outbound absent) or ES-DE (`wii` ES-DE row *does* exist; RomM is the gap). GameCube has both; Wii is half-exported.
6. **Dolphin planner refuses RVZ/CISO/WBFS** while identity fully verifies them — RVZ is arguably the dominant modern GC/WII preservation format; the planner's ISO/GCM-only slice is the last untranslated leg.
7. **Multi-disc election risk (GC/Wii)** — `DolphinDiscNumber` exists; no election/grouping test consumes it (same shape as the Sony/Xbox finding).

## V. ORPHANED PARSERS

- None in the strict sense (no parser exists without any caller). The orphaned *assets* are registry-level: `3ds`/`cia` inspector labels with no registry row; `xci`/`nsp`/`nca`/`wud`/`wux`/`rpx`/`cci`/`cxi`/`wia` platform claims with no scanner presence; `gcz`/`wad` platform claims with no identity arm. The missing seams are all in `media_registry.rs`, `ingestion/content_registry.rs`, and the `game_identity.rs` extension match — not in parsing.

## W. DO NOT REBUILD

- **`gamecube_wii_boot_evidence.rs`** — disc header/partition/apploader/FST/`main.dol` evidence, production-wired, honest decryption boundary.
- **The GC/Wii magic-word pair** in the platform registry — the correct structural discriminator, "preserved exactly as the existing header check behaved".
- **The Dolphin adapter chain** (`dolphin_command.rs`, `dolphin_execution/`, `DolphinDirectoryIdentity` device/inode discipline, GUI launch context) — the repo's reference standalone-adapter implementation.
- **The GC/Wii cheats/mods stack** — `bsfree_gamecube`/`bsfree_wii`, `gamehacking_gamecube_*`, `gamehacking_wii_provider` (identity-reusing, rollback-capable, end-to-end tested), `dolphin_texture_mod_page`.
- **RVZ/CISO/WBFS identity readers** (`inspect_rvz`/`inspect_ciso`/`inspect_wbfs`).
- **Generic DAT/hash/1G1R/multi-disc machinery** and the `es_de_export`/`romm_platform_mapping` row patterns (copy for the missing rows, never fork).

## X. PRIORITISED BACKLOG + BEST 7 TASKS

**P0 — broken joins**
1. `IdentityPlatform::WiiU` / `ThreeDS` / `Switch` variants + catalogue aliases (prerequisite for everything on those platforms; the enum + `from_catalogue` + display-name tables are the whole change).
2. Media registration for `3ds`/`cia`/`cci`/`cxi`, `xci`/`nsp`, `wud`/`wux`/`rpx` (DAT-hashable catalogue entries even before any structural parser exists).
3. `.gcz` bounded reader (or an explicit downgrade of the strong claim) — closes the only GC identity hole.

**P1 — user-visible completeness**
4. RomM outbound rows for Wii (`wii`), WiiU, 3DS, Switch — Wii especially (fully mature platform, half-exported).
5. ES-DE rows for WiiU/3DS/Switch (`wiiu`, `n3ds`/`3ds`, `switch` — verify exact ES-DE system names against its reference like every existing row).
6. WiiU `wad` weak-ext removal (registry drift fix) + `wad` decision on the Wii row (drop the strong claim or scope it to Virtual Console explicitly).
7. GC/Wii multi-disc election regression test consuming `DolphinDiscNumber`.

**P2 — genuinely new**
8. Cemu / Citra-lineage / Ryujinx adapters (detection → readiness → planning → execution, following the Dolphin reference); Wii U WUD/WUX structural parsing; Switch XCI/NSP header parsing (Title ID/version as facts); keys/firmware readiness surfaces per the Xbox/Sony readiness patterns.

**BEST 7 TASKS**

1. **`modern-nintendo-identity-variants`** — add `WiiU`/`ThreeDS`/`Switch` to `IdentityPlatform` + `from_catalogue` aliases + display names (`game_identity.rs:265-348`); non-goals: no parsers, no identity arms yet (they land in the `Other`-style honest Unsupported state); tests: catalogue-alias round-trips, inspection returns honest Unsupported; benefit: unblocks every other modern-Nintendo task. **Small.**
2. **`modern-nintendo-media-registration`** — register `3ds`/`cia`/`cci`/`cxi`, `xci`/`nsp`, `wud`/`wux`/`rpx` in `media_registry.rs` + `content_registry.rs` (Commodore/Apple row pattern); non-goals: no structural parsers, no identity claims; tests: end-to-end discovery + extension-coverage harness + DAT-hash identity; benefit: three platforms become ingestible and hash-identifiable at all. **Small.**
3. **`gcz-identity-reader`** — bounded GCZ reader following `inspect_rvz`/`inspect_ciso` (Dolphin's documented format; two-source verify before merge); non-goals: no decompression-into-persistence, no Wii `wad` scope creep; tests: fixture round-trip + Deferred→Verified transition + malformed refuse; benefit: a GameCube strong extension stops failing closed. **Medium.**
4. **`wii-romm-outbound`** — RomM outbound row `Wii → wii` (+ WiiU/3DS/Switch rows with RomM 5.0 provenance, mirroring the Xbox/PS3 row discipline); non-goals: no inbound changes; tests: `romm_slug_targets` update + mapping-registry tests; benefit: the most mature modern platform exports to RomM. **Tiny.**
5. **`modern-nintendo-esde-rows`** — ES-DE rows for WiiU/3DS/Switch with reviewed fullnames (the `gc`-not-`gamecube` precedent shows why verification matters); non-goals: no launch-path changes; tests: `every_required_platform_is_mapped` updates; benefit: ES-DE/RetroDECK export for three platforms. **Tiny.**
6. **`gcwii-multidisc-election-test`** — regression test consuming `DolphinDiscNumber` through `MultiDiscSet`/election (Sony/Xbox-shaped risk, GC/WII-shaped facts); non-goals: no grouping redesign; tests: multi-disc GC set election retains all discs; benefit: proves (or surfaces) the "elected release lost its second disc" risk for the most-used emulators in the app. **Small.**
7. **`wad-claim-cleanup`** — remove `wad` from WiiU weak extensions; either drop `wad` from Wii strong extensions or scope+document it (Virtual Console) with an explicit Unsupported identity state and Doctor wording; non-goals: no WAD parser; tests: registry guards + identity-status test; benefit: registry stops promising what the pipeline refuses. **Tiny.**

## Y. FINAL QUESTION

**"If EmuWiz stopped adding Modern Nintendo features today, what are the smallest changes required to make GameCube, Wii, Wii U, 3DS and Switch feel complete to an ordinary user?"**

- **GameCube:** effectively complete today for `.iso`/`.gcm` users — magic-verified platform, Game-ID identity, Dolphin launch from the GUI, Gecko/AR cheats, texture packs. Two holes: `.gcz` (a strong extension identity refuses) and the Dolphin planner's ISO/GCM-only slice vs identity-verified RVZ/CISO/WBFS. Fix the first, widen the second, and GC is done.
- **Wii:** identical story plus one embarrassment — the platform is fully mature but has **no RomM outbound row**, so it half-exports. Add `wii`, and decide what `wad` (a strong extension with no reader) is supposed to be.
- **Wii U / 3DS / Switch:** *complete* is currently unreachable, and honesty demands saying so: there are no parsers, no emulators, no identity variants, and barely any media registration. The smallest honest completion is the P0 pair — identity variants plus media registration — which turns three dead platform rows into catalogued, DAT-hashable libraries with truthful "no structural evidence / no emulator wired yet" Doctor answers. Real completeness for these three is a multi-milestone effort (container parsers + Cemu/Citra/Ryujinx adapters), and pretending otherwise would be the only dishonest move available.
- The family splits cleanly in two: **GC/Wii need two one-line-class fixes each; Wii U/3DS/Switch need their foundations laid.**
