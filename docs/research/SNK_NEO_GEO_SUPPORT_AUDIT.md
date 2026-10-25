# SNK / Neo Geo Family Support Audit — EmuWiz (RESEARCH ONLY)

> **Research snapshot** — This audit records repository findings at the time it was written. It is not current capability documentation; see the [README](../../README.md), [adapter support matrix](../ADAPTER_SUPPORT_MATRIX.md), and [roadmap](../../ROADMAP.md) for present guidance.

**Scope:** Neo Geo MVS/AES · Neo Geo CD · Neo Geo Pocket · Neo Geo Pocket Color — MAME, FBNeo, RetroArch, standalone NGP/NGCD emulators
**Branch:** `feature/archivefs-unified-platform`
**Method:** static source analysis only — no source modified, no commits.
**Cross-reference:** the NEC audit's SNK sidebar (`docs/research/NEC_SUPPORT_AUDIT.md` §13) — every claim in it re-verified here; all still true.

---

## A. PLATFORM MODEL

| | NeoGeo (`platform/mod.rs:1009-1027`) | NeoGeo64 (`:1028-1037`) | Neo Geo CD (`:1041-1052`) | Neo Geo Pocket (`:1054-1065`) | NGPC (`:1066-1077`) |
|---|---|---|---|---|---|
| aliases | `neogeo`, **`neogeoaes`**, **`neogeomvs`**, `snkneogeo`, `neogeoarcade` | `neogeo64` | `neogeocd`, `snkneogeocd`, `ngcd`, `neocd`, `neocdz` | `neogeopocket`, `ngp`, `snkneogeopocket` | `neogeopocketcolor`, `ngpc`, `snkneogeopocketcolor` |
| strong ext | **`neo`** | *(none)* | *(none)* | `ngp` | `ngc` |
| weak ext | `zip`, `7z`, `chd` | `zip`, `bin` | `iso`, `cue`, `bin`, `chd`, `img` | `bin`, `zip` | **`ngp`**, `bin`, `zip` |
| magic | none | none | none | none | none |
| conflicts | Neo Geo CD, **Arcade** | — | NeoGeo, Sega CD | NGPC | NGP |

- **MVS/AES are folded into one `NeoGeo` platform** via aliases — the arcade/console split is *not* modeled, and the row's own explanation is honest about why: "Neo Geo cartridge sets are MAME-style `.zip` archives, which prove nothing by themselves, so folder evidence carries the identification." No split is warranted.
- The **`Arcade` row** (`:603-614`, aliases `mame`/`fbneo`/`finalburnneo`/`fba`) *conflicts* with `NeoGeo` — MAME-oriented folders and Neo Geo folders are deliberately distinct resolutions. Correct shape: **no separate Neo Geo set resolver should be built; the Arcade/DAT machinery owns set identity** (§J).
- **IdentityPlatform: zero SNK variants** — no `NeoGeo`/`NeoGeoCd`/`Ngp`/`Ngpc` (`game_identity.rs:265-288`; grep verified empty). No SNK `IdentityKind`s, no `evidence_bridge` mappings, **no fusion rules** (`platform_evidence_fusion.rs` grep for neogeo: empty).
- ES-DE: **no `neogeo`/`neogeocd`/`ngp`/`ngpc` rows** (`es_de_export.rs`). RomM outbound: **`Neo Geo CD → neo-geo-cd` mapped** (`romm_platform_mapping.rs:151-156`); **NeoGeo/NGP/NGPC have no rows**. Launch: **no SNK or Arcade rows** in `LAUNCH_COMPATIBILITY`. Coverage: NGP/NGPC (`:433-449`, SyntheticValidated) + Neo Geo CD (`:469-476`, RealValidated) exist; **no NeoGeo row**.
- Naming drift: `NeoGeo`/`NeoGeo64` (no space) vs `Neo Geo CD`/`Neo Geo Pocket` (space) — the same id-vs-display hazard class the Atari review flagged.

## B. NEO GEO MVS / AES (cartridge sets)

- Cartridge content is **MAME/FBNeo-set-first by design**: the row defers identification to folder evidence, the `Arcade` conflict keeps MAME folders separate, and set identity resolves through the **generic DAT machinery** — `identity_source/mame_listxml/` and `identity_source/fbneo/` (convert/import), with `dat/identity.rs` already resolving DAT machine names to canonical platforms (Neo Geo CD tests at `dat/identity.rs:830-967`) and `platform/tests.rs:292-294` pinning `neogeo`/`neogeoaes`/`neogeomvs` → `NeoGeo`.
- Loose ROMs are unsupported as content: `.zip`/`.7z`/`.chd` weak, `.neo` strong (the Neo Geo homebrew/DIY flash-kit container) — **none registered** in `content_registry`/`media_registry`. AES-vs-MVS: no byte distinction exists (mode is a console setting) — correctly absent.
- **Verdict:** the right backend for MVS/AES is Arcade/DAT set identity + folder evidence; nothing is missing there except the identity/launch wiring every SNK platform lacks.

## C. NEO GEO BIOS

- **BIOS dependency modeling exists — generically.** `dat/dependency/resolve.rs` includes BIOS in its requirement graph (`resolve_bios`, `:287-288`) with an explicit honesty constant `BIOS_RUNTIME_SELECTION_NOT_MODELLED` (`:708-711`) — "which BIOS variant" is deliberately out of scope.
- neogeo.zip / UniBIOS / regional BIOS files: **not modeled as firmware** — `FirmwareSystem` is PS/PS2/Xbox only; no Neo Geo entries in `dat/firmware_evidence.rs`.
- Doctor/GUI: BIOS requirements surface only through the generic dependency graph's set verdicts; **no SNK-specific Doctor logic exists**.
- **Verdict:** Arcade dependency = the modeled form; firmware = unmodeled; **do not build a second BIOS system** — project the existing verdicts if a consumer appears.

## D. NEO GEO CD

**The IPL.TXT parser is the family's crown jewel and is production-wired:**
- `neogeocd_boot_evidence.rs`: `parse_ipl_txt` + `observe_neogeocd_evidence` — IPL.TXT at the disc root, **8 KiB bound** (`MAX_IPL_TXT_BYTES:51`), **32-entry cap** (`:52`), `0x1A` terminator validation, structurally validated entries; two-source corroborated against the NeoGeo Development Wiki IPL page. Crucially honest: *"there is no serial/catalog/product-code field anywhere in IPL.TXT"* — IPL structure is platform evidence, **never** release identity.
- **Production call site:** `disc_evidence_collector.rs:399-407` — `find_path(media, observation, "IPL.TXT")` → bounded read → `observe_neogeocd_evidence`, inside the shared `collect_disc_boot_evidence` switch.
- **Where production stops:** the evidence dies there. No fusion rule consumes it (`RULES` grep empty) → a Neo Geo CD disc with valid IPL.TXT **cannot resolve its platform from content**; no `IdentityPlatform::NeoGeoCd` → `.cue`/`.chd`/`.iso` arms can't route; the `.chd` arm covers PlayStation/Saturn/DC/SegaCd/ThreeDo/Pcfx/PcEngineCd — **Neo Geo CD absent → CHD `Deferred`** (same match-arm defect as PS2/PC-FX, now thrice-confirmed). Platform resolution today = folder aliases; DAT identity = generic hashes + the RomM `neo-geo-cd` mapping.
- Region/BIOS variants (front-loader/top-loader/CDZ): unmodeled. Audio/mixed-mode: shared optical stack rules, not to be loosened.

## E–F. NEO GEO CD CHD / BIOS

- CHD dispatch: one match-arm entry + a `NeoGeoCd` variant away (`game_identity.rs:823-836`); the CHD reader is already safe for track-1 NGCD data discs. Do not loosen track/pregap rules to reach it.
- BIOS/firmware: NGCD BIOS is an emulator runtime fact (NeoCD-class emulators); EmuWiz models none — correctly nothing fabricated. The `FirmwareSystem` pattern accepts a variant later **only if** a verified hash source appears.

## G–H. NGP / NGPC

**Parser (`ngp_header_evidence.rs`) — complete, tested (14 tests), structurally discriminating:**
- `NgpHeaderFact` (`:54-66`): `copyright` + **`copyright_recognized`** (exact match against the two documented strings — SNK / licensed — a multi-word signature, not a magic byte), `entry_point` (LE u32), **`software_id`** (LE u16 product code), `version`, **`system_flag`** (`:23` — `0x00` Monochrome / `0x10` Color / `Unknown(other)`), `title`. Fails closed on short buffers; unrecognised copyright still parses (GB-parser precedent).
- Evidence: **`Strong` `BootStructure` only when `copyright_recognized`**; the color/mono fact rides separately.
- **Wired:** registered in `archive_member_content_evidence.rs:172` (`NgpHeaderDetector`) — **ZIP members only**.
- **Orphaned from:** loose-file discovery (`.ngp`/`.ngc` in **no** scanner registry — verified against the full `CONTENT_FORMATS`/`MEDIA_FORMATS` tables), identity (no variants), fusion (no rules), DAT (member evidence never reaches hashing), launch.
- **Collision policy (§H):** the header's own `system_flag` is the structural NGP-vs-NGPC discriminator — **better than extension**, and the parser already carries it. The dual-mode question (mono games on color hardware) is representable via the GBC `CgbEnhanced` precedent: mono titles resolve NGP with a corroborating "runs on color" fact; Color-flag titles are NGPC-exclusive. **The fact is parsed; the policy is not implemented.** Extension-only resolution is what happens today.

## I. DAT ECOSYSTEMS

| Platform | Primary ecosystem | EmuWiz support |
|---|---|---|
| NeoGeo MVS/AES | MAME / FBNeo sets | generic set resolution (`mame_listxml`, `fbneo`; `dat/identity` machine names) — mature |
| Neo Geo CD | Redump / TOSEC | generic disc hashing + `neo-geo-cd` RomM slug; IPL facts as provenance |
| NGP/NGPC | No-Intro | generic hash matching once files are ingestible (today: members only) |
| BIOS | MAME BIOS sets | generic `dat/dependency` BIOS requirements |

All hash types/stale/multi-disc handling is generic. **No SNK-specific DAT machinery is needed** — the one SNK hook worth wiring is NGP header facts, not new DAT code.

## J. ARCADE INTERACTION

- The mature chain is **set resolution, not launch**: `mame_listxml`/`fbneo` imports → `dat/identity` machine name → platform → `dat/dependency` (incl. BIOS) → set verdicts/persistence. A Neo Geo set flows through this exactly like any MAME set — **no Neo Geo special-casing exists or is needed**.
- **Correction to the task premise:** there is **no finished standalone MAME launch work** in the repo — `launch/` contains zero MAME modules and `LAUNCH_COMPATIBILITY` has no Arcade/NeoGeo rows. What is finished is DAT/set identity. Launch for Arcade *and* NeoGeo alike is missing and should be built once, generically — not twice.

## K–L. EMULATORS / RETROARCH

| Emulator | State in repo |
|---|---|
| MAME (standalone) | set/DAT identity only; **no launch adapter** |
| FBNeo (standalone) | set identity only; no adapter |
| FBNeo / neogeo (RetroArch cores) | generic RA chain works; **no SNK core hints** |
| NeoCD / NGCD emulators | absent (zero references) |
| Mednafen Neo Pop / Beetle NeoPop, RACE | absent (zero references) |

RetroArch's dynamic `.info` resolution (`platform_map.rs:212-248`) already resolves "Neo Geo"/"Neo Geo Pocket" systemnames via the alias tables — **once identity variants + launch rows exist, RA covers the whole SNK family with zero new emulator code**. No standalone NGP/NGCD adapter is justified.

## M. MAME / FBNEO LAUNCH

Nothing to special-case: no Neo Geo launch path exists; the missing piece is the *generic* Arcade/MAME launch story (out of SNK scope but blocking NeoGeo launch). BIOS dependencies already block correctly inside the dependency graph's verdicts.

## N. CHEATS / PATCHES / MODS

No SNK-specific cheat handling exists (`cheat_catalogue`/`cheat_provider` greps empty for fbneo/neogeo). RetroArch `.cht` cheats resolve generically via platform aliases once platforms reach the cheat layer — same pattern as Atari. Honest absence; do not invent.

## O. MULTI-DISC

Neo Geo CD multi-disc releases: generic `MultiDiscSet`/companion machinery (the GC/Wii-proven path) applies unchanged; no SNK-specific grouping exists or is needed; no SNK multi-disc test exists (noted in §V).

## P–Q. ROMM / ES-DE

- RomM: **Neo Geo CD → `neo-geo-cd` ✅ mapped**; **NeoGeo, NGP, NGPC missing** (RomM has `neogeo`, `ngp`, `ngpc` slugs). MVS/AES map to the single `neogeo` platform — correct.
- ES-DE: **all four rows missing** (`neogeo`, `neogeocd`, `ngp`, `ngpc` are ES-DE system names; verify exact fullnames against `es_systems.xml` per module discipline). No duplicate maps.

## R. GUI-HIDDEN FACTS (source-proven)

NGP/NGPC `software_id` (product code), `system_flag` (mono/color), `title`, `copyright_recognized`; IPL.TXT entry structure; MAME/FBNeo set completeness + BIOS-dependency verdicts; DAT source family per match. None reach the GUI.

## S. DOCTOR

Cannot report today: missing neogeo.zip (BIOS verdicts live only in the generic dependency graph, never projected), incomplete MAME/FBNeo set, missing NGCD BIOS, malformed NGCD disc (IPL refusal is evidence-level only), unresolved NGP/NGPC identity, SNK emulator unavailability. **Reuse the dependency-graph verdicts; build zero SNK-specific Doctor logic for MVS** — the gap is projection, like every family audited.

## T. SECURITY / FAIL-CLOSED

- `.zip` ≠ NeoGeo: the row says so in its own explanation; `.zip` is weak + shared-denylist.
- `.neo` strong: extension-only claim (no magic rule); the container is unverified — registry-level until a parser exists.
- `.ngp` extension ≠ monochrome: the *header* system_flag decides; extension never does (and today nothing decides at all — fail-closed).
- `.ngc`/`ngpc` filename: never identity — `software_id`/`system_flag` come from bytes.
- `.iso` ≠ Neo Geo CD: no ISO arm exists; IPL.TXT *content* (validated structure, not the filename) is the only NGCD-specific fact, and it proves platform structure — never release identity (no serial field exists, per the parser's own documentation).
- IPL.TXT *filename alone* ≠ proof: the parser validates the 0x1A terminator, entry count, and field shapes.
- Shell execution: N/A (no SNK planners exist). **No unsafe SNK promotion path found.**

## U. REAL-CORPUS COVERAGE

| Platform | Status |
|---|---|
| NeoGeo | **NoCoverage** (no row) |
| Neo Geo CD | **RealValidated** — `neogeocd_boot_evidence` (IPL.TXT), coverage `:469-476` |
| NGP | **SyntheticValidated** — `ngp_header_evidence`, "no real specimen" note |
| NGPC | **SyntheticValidated** — same module |
| Arcade/MAME/FBNeo sets | generic DAT machinery (not evidence-coverage tracked) |

## V. TEST COVERAGE

Present: `neogeocd_boot_evidence` (IPL parser — terminator/entry-count/field validation; 21 tests per the NEC audit), `ngp_header_evidence` (14 tests: copyright gate, system flag, software_id, fail-closed), `archive_member_content_evidence` (NgpHeaderDetector registered), `dat/identity` Neo Geo CD machine-name tests, `platform/tests` alias pins (`neogeoaes`/`neogeomvs`), mame_listxml/fbneo import tests, `dat/dependency` BIOS-resolution tests.
**Missing:** NGP/NGPC loose-file discovery (no path), IPL→platform fusion (no rule), NGCD `.chd` identity (no arm), NGP mono/color platform policy (no rule), SNK multi-disc election, any SNK launch/Doctor test.

## W. MATURITY MATRIX

| | NeoGeo | NeoGeoCD | NGP | NGPC |
|---|---|---|---|---|
| Platform registry | MATURE (MVS/AES folded; honest explanation) | MATURE | MATURE | MATURE |
| Media registration | REGISTERED-ONLY (`.zip`/`.7z`/`.chd` weak; `.neo` strong, unregistered) | REGISTERED-ONLY (weak disc exts) | **ORPHANED** — `.ngp`/`.ngc` in no registry; parser exists | **ORPHANED** — same parser |
| Structural evidence | MISSING (sets are DAT/folder territory — correct) | PARTIAL — IPL evidence production-collected, never resolved | PARTIAL — parser Strong-gated, member-only | PARTIAL — same |
| Stable product/game ID | N/A (set name = identity) | MISSING (IPL has no serial — honest) | PARTIAL — `software_id` parsed, unwired | PARTIAL — same |
| Exact DAT/hash identity | MATURE (generic set hashing) | MATURE (generic) | PARTIAL — members only; loose files uninjestible | PARTIAL — same |
| Persistence | MATURE | MATURE | PARTIAL | PARTIAL |
| BIOS/firmware | PARTIAL — generic dependency graph only; no readiness/Doctor | MISSING (correctly nothing fabricated) | N/A | N/A |
| Emulator discovery | MISSING (no MAME/FBNeo/RA-SNK adapters) | MISSING | MISSING (RetroArch generic only) | MISSING |
| Readiness / Planning / Execution / GUI launch | MISSING | MISSING | MISSING | MISSING |
| Doctor | MISSING (dependency verdicts unprojected) | MISSING | MISSING | MISSING |
| Cheats / Mods | MISSING (honest) | MISSING | MISSING | MISSING |
| Rename / Duplicates / 1G1R / Playing Library | MATURE (generic) | MATURE | MATURE | MATURE |
| RomM | MISSING row | MATURE (`neo-geo-cd`) | MISSING row | MISSING row |
| ES-DE | MISSING row | MISSING row | MISSING row | MISSING row |
| Multi-disc | N/A | PARTIAL — generic companions, untested | N/A | N/A |
| Real corpus | NoCoverage | RealValidated | SyntheticValidated | SyntheticValidated |

## X. BROKEN JOINS (top 15)

1. **IPL.TXT evidence is collected → no fusion rule resolves "Neo Geo CD" from it** (`RULES` empty for SNK).
2. **No `IdentityPlatform::NeoGeo/NeoGeoCd/Ngp/Ngpc`** — the identity pipeline cannot carry any SNK fact.
3. **NGP parser registered for archive members only** — loose `.ngp`/`.ngc` files are in no registry and skip discovery.
4. **NGCD `.chd` arm excludes Neo Geo CD** — Deferred despite collected evidence (thrice-seen defect).
5. **`evidence_bridge` has no SNK mappings** — collected evidence can't become launch identity.
6. RomM: NeoGeo/NGP/NGPC outbound rows missing (`neo-geo-cd` exists — the asymmetry proves the table works).
7. ES-DE: all four SNK rows missing.
8. `LAUNCH_COMPATIBILITY`: no SNK or Arcade rows; RetroArch `.info` alias resolution already names every SNK system.
9. **NGP `system_flag` discriminator parsed → no mono/color platform policy** (the GBC `CgbEnhanced` precedent is unused).
10. **NGP `software_id` parsed → no product-code fact kind/consumer.**
11. NeoGeo coverage row missing (NGP/NGPC/NGCD have rows).
12. BIOS dependency verdicts (`dat/dependency`) → never projected to readiness/Doctor.
13. **No Arcade/MAME launch path at all** — blocks NeoGeo launch irrespective of SNK wiring (task-premise correction: MAME *set resolution* is finished; MAME *launch* does not exist).
14. NGP/NGPC synthetic-only corpus — no real-specimen validation path documented.
15. `.neo` strong extension with no parser/registration — container unverified (registry-level claim only).

## Y. ORPHANED CODE

| Module/function | Missing seam | Size |
|---|---|---|
| `neogeocd_boot_evidence::observe_neogeocd_evidence` (collected at `disc_evidence_collector.rs:399-407`) | fusion `RULES` entry + `IdentityPlatform::NeoGeoCd` + `.chd` arm | Small |
| `ngp_header_evidence::parse_ngp_header` / `NgpHeaderDetector` (member-only) | `content_registry`/`media_registry` rows + loose-file identity dispatch + mono/color policy rule | Small |
| `dat/dependency::resolve_bios` verdicts | readiness/Doctor projection | Small |
| `platform_for_alias("neogeo"…)` resolutions in RetroArch `.info` | `LAUNCH_COMPATIBILITY` rows + identity variants | Tiny |

## Z. DO NOT REBUILD

- **`neogeocd_boot_evidence.rs`** — two-source IPL.TXT parser with honest no-serial documentation; already production-collected.
- **`ngp_header_evidence.rs`** — copyright-gated Strong evidence, system-flag discriminator, software_id; complete.
- **`identity_source/mame_listxml` + `identity_source/fbneo` + `dat/identity` machine-name resolution** — the Neo Geo set backend; MVS/AES need nothing new here.
- **`dat/dependency` BIOS requirement graph** (incl. `BIOS_RUNTIME_SELECTION_NOT_MODELLED`) — extend consumers, never the graph.
- **Generic optical/CHD stack** (track/pregap rules), **generic DAT/1G1R/multi-disc machinery**, **RetroArch dynamic core resolution + execution** (`spawn_retroarch`), **`platform/tests.rs` alias pins**.

## AA. PRIORITISED BACKLOG + BEST 10 TASKS

**P0:** identity variants (4 SNK), SNK fusion rules (IPL → Neo Geo CD; NGP copyright/system-flag → NGP/NGPC with mono/color policy), `.ngp`/`.ngc`/`.neo` registration, NGCD `.chd` arm, evidence_bridge mappings.
**P1:** ES-DE rows ×4, RomM rows ×3 (NeoGeo/NGP/NGPC), launch rows with RA core hints (`fbneo`/`neogeo`/`beetle_neopop`-class), coverage row NeoGeo, BIOS-verdict → readiness projection.
**P2:** generic Arcade/MAME launch (out of SNK scope but blocking), NGP/NGPC real-corpus validation, `.neo` container parser (research-first), SNK Doctor wording.

| # | Slug | Objective | Files | Reused | Missing join | Non-goals | Tests | Benefit | Dep | Size |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `snk-identity-variants` | `IdentityPlatform::{NeoGeo,NeoGeoCd,Ngp,Ngpc}` + catalogue aliases + labels | `game_identity.rs:265-348` | variant pattern | enum gap | no inspect arms yet (honest Unsupported) | catalogue round-trips | keystone for all SNK identity | none | **Tiny** |
| 2 | `ngcd-ipl-fusion-rule` | Fusion rule: IPL `BootStructure` → resolve "Neo Geo CD" from content | `platform_evidence_fusion.rs` RULES | `observe_neogeocd_evidence` output shape, `neogeocd_boot_evidence` tests | evidence→resolution hop | no serial claims (none exist); no CHD-rule loosening | IPL fixtures resolve; non-IPL discs don't | folder-less NGCD identification | 1 | **Small** |
| 3 | `ngcd-chd-arm` | Add `NeoGeoCd` to the `.chd` match arm + `inspect` routing | `game_identity.rs:823-836` | CHD reader (track-1-safe) | match-arm exclusion | no track-rule changes | real/synthetic NGCD CHD; truncated refuse | NGCD CHD becomes identity-eligible | 1,2 | **Small** |
| 4 | `ngp-loose-wiring` | Register `.ngp`/`.ngc` + loose-file identity dispatch + mono/color policy rule | `content_registry.rs`, `media_registry.rs`, `game_identity.rs`, `platform_evidence_fusion.rs` | `parse_ngp_header`, GBC `CgbEnhanced` precedent | member-only parser | no extension-only mono/color claims | fixtures: mono→NGP, color→NGPC, dual corroborating, unknown flag refuse | NGP/NGPC leave member-only limbo | 1 | **Small** |
| 5 | `snk-romm-rows` | RomM outbound `neogeo`/`ngp`/`ngpc` | `romm_platform_mapping.rs` | row pattern (`neo-geo-cd` proves it) | mapping gap | no unverified slugs | slug tests | export for the family | none | **Tiny** |
| 6 | `snk-esde-rows` | ES-DE `neogeo`/`neogeocd`/`ngp`/`ngpc` rows | `launch/es_de_export.rs` | row pattern + fullname verification | mapping gap | no unverified fullnames | mapping tests | ES-DE/RetroDECK export | 1 | **Tiny** |
| 7 | `snk-launch-rows` | `LAUNCH_COMPATIBILITY` rows + RA core hints (`fbneo`, `neogeo`, `beetle_neopop`-class) | `launch/platform_map.rs` | alias resolution already works | launch gap | no standalone adapters | candidate-generation tests | RA launch covers the family | 1 | **Tiny** |
| 8 | `neogeo-coverage-row` | Coverage-inventory row for NeoGeo | `coverage_inventory.rs` | row pattern | ledger gap | none | inventory tests | honest coverage | none | **Tiny** |
| 9 | `bios-verdict-projection` | Project dependency-graph BIOS verdicts into readiness/Doctor | `launch/readiness.rs`, `diagnostics/profiles.rs` | `resolve_bios` verdicts | verdict→surface seam | no BIOS-runtime selection (honesty const) | projection tests | "missing neogeo.zip" becomes visible | none | **Small** |
| 10 | `ngp-real-corpus-validation` | Validate NGP/NGPC parser against real specimens; update coverage rows | `coverage_inventory.rs` (+ test fixtures) | parser unchanged | synthetic-only status | no parser changes without two-source cause | real-fixture tests | honest validation status | none | **Small** |

*(Generic Arcade/MAME launch is deliberately **not** in this list: it is the pre-requisite for NeoGeo launch but belongs to the Arcade family audit.)*

## AB. FINAL QUESTIONS

1. **How complete is Neo Geo MVS/AES really?** As an *arcade/DAT-set* platform: nearly complete — set resolution, machine-name→platform mapping, BIOS dependencies, and hash identity all work generically. As a *launchable EmuWiz platform*: zero — no identity variant, no launch row, no Arcade/MAME launch path exists in the repo at all.
2. **Is Arcade already the correct backend for MVS?** Yes — and the row model (`NeoGeo` conflicting with `Arcade`, aliases `neogeomvs`/`neogeoaes`) already encodes the split. Do not build a Neo Geo set resolver; the missing piece is a *generic* MAME/FBNeo launch path, which is Arcade-family work.
3. **How close is Neo Geo CD to full identity + launch?** Evidence-collected but unresolved: the IPL parser runs in production, so the remaining distance is one fusion rule, one identity variant, one `.chd` arm, and the standard launch rows — the smallest evidence-to-launch distance of any platform audited with zero new parsing.
4. **Is NGP/NGPC mostly parser wiring?** Yes — the parser is complete and discriminating (system_flag beats extension); what's missing is registry rows, the mono/color policy rule (GBC precedent), and identity variants.
5. **What BIOS work remains?** None new: the dependency graph models BIOS requirements generically (with honest runtime-selection limits); the only work is *projecting* those verdicts to readiness/Doctor. No Neo Geo firmware hashes should be embedded.
6. **Which SNK formats genuinely need new parsing?** Only `.neo` (container, research-first). IPL and NGP headers are already parsed. MVS sets are MAME's domain.
7. **Which should remain DAT/hash-driven?** All release identity; NGCD release identity especially (IPL has no serial field — by design). MVS/AES identity is set-name + hash, already correct.
8. **What five changes give the biggest pre-release user benefit?** Tasks 1, 2, 4, 6, 7 — identity variants, IPL fusion, NGP loose wiring, ES-DE rows, launch rows. All Tiny/Small; zero new parsers.
9. **What should wait until after release?** Generic Arcade/MAME launch (Arcade-family scope), NGCD BIOS/firmware modeling, `.neo` parsing, real-corpus NGP validation, SNK cheat/Doctor wording.
10. **If EmuWiz stopped adding SNK features today, what prevents the family from feeling complete?** Not parsing — *connection*. The IPL and NGP parsers are finished and (in IPL's case) already running in production; nothing carries their verdicts to a platform decision, and no launch/ES-DE/RomM row exists to carry a decision to the user. Roughly ten table-sized entries plus two Small fusion/dispatch tasks separate SNK from parity with the Nintendo cartridge family.
