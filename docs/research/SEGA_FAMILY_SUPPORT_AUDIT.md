# Sega Family Support Audit — EmuWiz end-to-end

**Repo:** `/home/davedap/archivefs` · **Branch:** `feature/archivefs-unified-platform`
**Scope:** Master System, Game Gear, Mega Drive/Genesis, 32X, Mega CD/Sega CD, Saturn, Dreamcast. Read-only research; every claim verified against source on this branch (CDI format facts additionally reference the repository's own two-source-verified `dreamcast_cdi` module documentation and the `opticaldiscs` 0.15 dependency, plus common CDI tooling knowledge — flagged inline).

---

## A. Platform model

| Canonical ID | Display | Aliases (examples) | Strong ext | Weak ext | Conflicts | Coverage row | RomM | ES-DE | Launch row |
|---|---|---|---|---|---|---|---|---|---|
| `MasterSystem` | Sega Master System | mastersystem, sms, segamarkiii… | `sms` | bin, rom, zip | GameGear | ✅ `Partial` (real DAT overlap; fusion not end-to-end) | ✅ | ❌ | ❌ |
| `GameGear` | Sega Game Gear | gamegear, segagg, gg | `gg` | bin, rom, zip | MasterSystem | ✅ `RealValidated` | ❌ | ❌ | ❌ |
| `MegaDrive` | Sega Mega Drive / Genesis | genesis, segagenesis, smd, md… | `smd`, `68k` | md, bin, gen, zip, chd | Sega CD, 32X, ScummVM | ✅ `RealValidated` (checksum-verified specimen) | ✅ | ✅ `megadrive` | ❌ |
| `Sega 32X` | Sega 32X | sega32x, 32x, mega32x… | `32x` | bin, md, smd, zip | MegaDrive, Sega CD | ✅ `RealValidated` (candidate-only by design) | ✅ | ❌ | ❌ |
| `Sega CD` | Sega Mega-CD / Sega CD | segacd, megacd, scd… | — (none) | iso, cue, bin, chd, ccd, mdf, img | MegaDrive, 32X, PSX, AmigaCD32 | ✅ | ✅ | ✅ `segacd` | ✅ RA hint only (`genesis_plus_gx`) |
| `Saturn` | Sega Saturn | saturn, segasaturn | — (none) | iso, cue, bin, chd, mdf, ccd, img | Sega CD, Dreamcast | ✅ | ❌ | ✅ `saturn` | ❌ |
| `Dreamcast` | Sega Dreamcast | dreamcast, segadreamcast | `gdi`, `cdi` | iso, cue, bin, chd, mdf | Saturn, Philips CD-i | ✅ | ✅ | ✅ `dreamcast` | ✅ `flycast` (Exact) |

**Naming drift findings:**
- Canonical ID is `MegaDrive` with `genesis` as an alias; ES-DE exports `megadrive` (a documented, reviewed judgment call in `es_de_export.rs`). No drift in practice.
- `Sega CD` (with space) vs `IdentityPlatform::SegaCd` (enum) vs RomM/ES-DE `segacd` — three spellings, all mapping-reviewed; `launch/platform_map.rs:255` has a dedicated `retroarch_platform_matches` special case for it.
- `Sega 32X` (space) is the canonical id; `32x` is only an alias/extension.
- `Dreamcast` is consistent everywhere (RomM slug family `dc`-adjacent rows, ES-DE `dreamcast`).
- **Real gaps:** RomM rows missing for `GameGear` and `Saturn`; ES-DE rows missing for `GameGear`, `MasterSystem`, `Sega 32X`.

---

## B. Master System / Game Gear

- **Header parsing (`sms_gg_header_evidence.rs`)**: mature — `TMR SEGA` at the BIOS-checked offsets 0x7FF0/0x3FF0/0x1FF0, 16-byte header, checksum exposed as *declared fact only* (size-dependent range rules deliberately not implemented without two-source corroboration), product code kept as raw BCD (digit order uncorroborated), region/system nibble → `SmsGgSystem` incl. honest `Unknown(nibble)`.
- **Platform registry**: `TMR SEGA` magic rules at all three offsets, `Corroborated`; both platforms conflict with each other — the header cannot separate SMS from GG, and the registry says so.
- **Scanner**: `.sms`/`.gg` registered in **both** `media_registry` and `ingestion/content_registry`; watcher-relevant.
- **Production evidence**: `gather_structural_evidence` (`selected_evidence_page.rs:109`) has a live `"sms" | "gg"` arm → the Selected Evidence page shows TMR SEGA facts. CONNECTED.
- **Identity — the broken join**: `IdentityPlatform` (`game_identity.rs:265-288`) has **no MasterSystem/GameGear variants** (both map to `Other`), and `inspect_loose_rom` covers only MegaDrive/Snes/Nes/GB/GBC/GBA/N64. A catalogued `.sms`/`.gg` therefore gets **no SHA-256 and no header facts** in its persisted identity report, despite a verified parser.
- **What proves what**: container/platform = extension (strong) + folder; system nibble = SMS-vs-GG hint (never a decision); product code = raw BCD only; exact release = DAT hash only (TOSEC real-validated: SMS ×47, GG ×96 via the generic audit).
- **`.bin`**: safely ambiguous — weak extension on both rows, never self-evidencing.
- **No filename inference anywhere.**

## C. Mega Drive / Genesis

- **`megadrive_header_evidence.rs` is mature — leave alone**: verified Plutiedev layout (console name 0x100, titles, serial 0x180, BE checksum 0x18E, region 0x1F0), bounded parse (0x1F3 bytes), opt-in whole-ROM checksum `verify_megadrive_checksum` real-validated against a corpus specimen ("3 Ninjas Kick Back .md", exact).
- **`smd_normalization.rs` is mature and honest**: 512-byte Super Magic Drive header strip + 16 KiB even/odd de-interleave, verified against uCON64 FAQ and pyromatch, reversible, transform id `smd-deinterleave-to-bin`, deliberately `Weak` evidence (AA/BB signature rejected). **Not wired into identity hashing** — `inspect_loose_rom` explicitly records "bytes were not header-stripped or deinterleaved" (`game_identity.rs:989`); no SMD canonical-hash analogue of `push_n64_canonical_evidence`.
- **Registration**: `.smd` self-evidencing (`MegaDriveRom` kind); `.md`/`.bin`/`.gen` corroboration-gated (folder → root → header) with a *positive refusal* for contradicting folders (`lib.rs:3637-3644`, the ScummVM `RESOURCE.GEN` precedent). `.68k` is a strong extension registered in **neither** scanner registry.
- **Identity gap**: Mega Drive loose ROMs get bounded whole-file SHA-256 + extension format + normalized title — but **zero header facts** (serial/region/titles) are promoted into the `GameIdentityReport` even though `parse_megadrive_header` exists.
- **No duplicate implementations**; no extension-only promotion beyond the by-design `.smd` self-evidencing; no SRAM metadata modeling (correct — not header-encodable).

---

## D. 32X — verified orphan

- **Module**: `sega32x_header_evidence.rs` (229 lines, tested). Deliberately layered on the Mega Drive parser; the second leg is the `"32X"` substring in the reused console-name field, graded `Weak` because no primary spec pins it as unique (module doc explains why; the SNES-copier precedent is cited).
- **Evidence strength**: base leg = Corroborated Mega Drive header; 32X leg = Weak `ContentSignature` only, never a platform decision.
- **Validation**: `coverage_inventory.rs` marks `Sega 32X` **RealValidated** (Doom (32X) specimen, "candidate-only by design").
- **Production callers**: registered in `archive_member_content_evidence::member_detectors()` — whose only caller is the `cartridge_probe` **example**. No `IdentityPlatform::Sega32X` variant; `.32x` registered in **neither** scanner registry.
- **Verdict**: exactly the suspected tiny join — an `IdentityPlatform` variant + a `supported_loose_rom_format` entry + a registry row + (optionally) the production member-evidence pass connect a tested parser to users.

## E. Mega CD / Sega CD

- **Optical reuse — no duplicate stack**: identity flows through the generic `iso9660`/`cue_bin` (Mode1 2048 cooked / 2352 raw readers) / `chd_logical_media` machinery. `segacd_boot_evidence.rs` adds only the Sega-specific facts: `SEGADISCSYSTEM` boot signature (Strong) at offset 0 (raw-2352) **and** offset 0x10 (ISO-2048 dumps — both registry rules), plus the documented Disc-ID product field at 0x180 with `GM T-12345-00`-style normalization.
- **Identity**: `inspect_sega_cd_source` (`game_identity.rs:2804`) verifies the boot signature then extracts the product code — dispatched from ISO, CUE, and CHD arms.
- **Track rules**: single data track via the generic CUE resolver; audio excluded; mixed-mode handled by the shared cooked/raw readers. Multi-disc: generic `(Disc N of M)` grouping + m3u anchoring.
- **BIOS**: absent (see §M). **Launch**: the only Sega platform with a reviewed RetroArch core hint (`genesis_plus_gx`) plus the shared-family `database`-field special case (`retroarch_platform_matches`). No standalone adapter — acceptable while RetroArch works. **RomM/ES-DE**: both mapped.

## F. Saturn

- **`saturn_boot_evidence.rs` is mature**: full System ID parse per Sega's ST-040-R4 spec (hardware ID, maker, product number, version, release date, device info, packed area symbols, peripherals, title), verified against Mednafen `ss.cpp` as second source; `Strong` boot evidence; fail-closed on truncation.
- **Registry**: `SEGA SEGASATURN` at offset 0, `Strong` — the best fail-closed posture in the family (no strong extensions to abuse).
- **Identity**: `inspect_saturn_source` (`game_identity.rs:2631`) verifies the product number from ISO/CUE/CHD — mature, real-validated (`coverage_inventory.rs`).
- **Detection / Readiness / Planning / Execution / GUI launch**: **Detection M; Readiness/Planning/Execution MISSING.** Exact break point: `launch_compatibility_for_platform("Saturn")` is `None` (asserted by test, `platform_map.rs:348`), no core hints exist, and no Saturn adapter exists anywhere in the repo. `IdentityKind::SaturnProductNumber` already projects into `VerifiedIdentityFact::SaturnProductCode` (`evidence_bridge.rs:221`) — a ready-made game key with zero consumers.
- **Doctor/GUI/cheats/mods**: identity facts visible only via the generic evidence path; no Saturn-specific coverage anywhere downstream.

## G. Dreamcast — the family's mature vertical

- **`dreamcast_boot_evidence.rs`**: two-source-verified 256-byte IP.BIN layout (Marcus Comstedt + KallistiOS `makeip`); `SEGA SEGAKATANA` (and `SEGAMARIO`) recognized; product number, version, area symbols (region), boot filename extracted; `Strong` boot + `Corroborated` product-code evidence.
- **All-format identity convergence**: ISO, CUE, GDI, CDI, and GD-ROM CHD all feed the single `inspect_dreamcast_source` (`game_identity.rs:2717`) — no per-format identity forks.
- **GD-ROM CHD specialist routing is complete** (explicitly verified, not to be redone): `chd_identity::needs_specialist_optical_backend` metadata detection + optional `chd-optical-specialist` feature wrapping MAME's chdman core via `opticaldiscs`/`libchdman-rs`; the default build honestly reports `Unsupported` (`game_identity.rs:4440-4449`).
- **Persistence → launch → GUI**: verified `DreamcastProductCode` → `VerifiedIdentityFact::DreamcastProductCode` → Flycast binding requires it → `flycast_command` argv plan → `flycast_execution` → **GUI launch panel wired** (`main.rs:6450-6498`). The only Sega platform with a full GUI launch path.
- **Flycast readiness**: `dc_boot.bin`/`dc_flash.bin` verified against pinned two-source identities (libretro-sourced); the boot ROM is a strict launch gate; flash verified-when-present with a documented rationale.


---

## H. CDI deep dive

**First, correcting the premise: a CDI parser exists, is complete, and is production-wired — `.cdi` is *not* merely registered/inspector-visible.**

- **Parser**: `dreamcast_cdi.rs` (662 lines, tested) on the `opticaldiscs` 0.15 crate (`Cargo.toml:115`), `dreamcast-cdi` feature **default-on**. `opticaldiscs::discjuggler` is a pure-Rust, bounds-checked port of cdemu's `libmirage` `image-cdi/parser.c` field-for-field — the closest thing to an authoritative reference for this closed, reverse-engineered format (DiscJuggler never published a spec).
- **Format structure** (per the crate's own verified module documentation plus common CDI tooling knowledge — cdmage, libmirage/cdemu, DiscImageChef, `opticaldiscs`): metadata lives in an **end-of-file trailer** walked backwards (session descriptors → track descriptors), not a header; descriptor classes are versioned (`CDI 0020/0021` ≈ V2, `0030/0031` ≈ V3, `0036`-era ≈ V4 with larger descriptors); per-track sector forms include MODE1/2352, MODE1/2048, MODE2/2336 and audio/2352, each descriptor carrying header/subheader sizes and the track's `base_lba`.
- **Multi-session / GD-ROM**: a GD-ROM rip records two sessions; the high-density session's first track carries an absolute `base_lba` at/above the documented GD-ROM high-density boundary. `dreamcast_cdi::select_dreamcast_data_track` reuses the identical `GDROM_HIGH_DENSITY_START_FRAME` boundary and the identical "exactly one eligible data track, or refuse" rule as `.gdi`; audio tracks are never selected.
- **Sector/offset safety**: the declared byte range is cross-checked against the real file length (`ImpossibleOffset`); unsupported sector geometries (`UnsupportedSectorLayout`), ambiguous candidate tracks (`AmbiguousDataTracks`), oversized files (`MAX_CDI_BYTES`), too many tracks, and a poisoned parse mutex are all refused. Because upstream exposes only a whole-file `parse_discjuggler` (no `Read+Seek`), concurrent calls serialize on a documented mutex.
- **Identity without conversion**: IP.BIN sits at logical offset 0 of the selected data track's cooked view; the module wraps it in a `LogicalMedia` and feeds the *same* `inspect_dreamcast_source` every other Dreamcast source uses. **Useful identity is already extracted without converting the image.**
- **Residual risks** (not regressions): re-saved/up-converted CDIs with altered descriptors and C2/subchannel presence quirks are handled by refusal rather than guessing; directory browsing of the selected track (ISO 9660 extents) is explicitly out of scope; a feature-off build has a fail-closed twin (`game_identity.rs:4487-4496`).
- **Conclusion**: *feasible bounded parser* — **already implemented**. Building another would rewrite mature, feature-gated, tested work. The genuine CDI gap is elsewhere: `.cdi` is absent from `media_registry`, so loose CDI files never become library rows and the watcher is blind — a registration join, not a parsing problem.

## I. Dreamcast GDI

- **Parser**: `ingestion/gdi.rs` — bounded descriptor read (≤64 KiB), per-line track table, LBA sanity bound (10M), 2352-only sector acceptance (2336 refused), declared-track-count consistency, exactly-one-high-density-data-track selection, duplicate track/LBA refusal, and `canonicalize` + `starts_with` companion-file resolution that defeats `..`-traversal and symlink escape (mirroring the CUE module's documented safety design).
- **Identity join — CONNECTED**: `inspect_gdi` (`game_identity.rs:1257`) resolves the high-density track, opens it through the shared raw/cooked CD logical-media readers, and feeds `inspect_dreamcast_source`; provenance records the resolved track file (`relative_member_path`).
- **Missing companion files** → `GdiError::MissingTrackFile` → identity `Invalid` with the error text — an honest refusal, never a partial guess.
- **Archive-member support**: none — GDI is inherently multi-file; ZIP-contained GDI sets are not resolved (the inspector merely classifies `.gdi` members as likely content).
- **Broken join**: `.gdi` is a **strong extension** on the Dreamcast row and has a full parser + identity path, but is absent from `media_registry::MEDIA_FORMATS` — a loose `.gdi` never becomes a library archive row and `is_watch_relevant_extension("gdi")` is false. The scanner never reaches the parser.

## J. CHD

- **Identity match arms**: `game_identity.rs:778-788` whitelists CHD for `PlayStation | Saturn | Dreamcast | SegaCd` — **all Sega optical platforms are covered**; Saturn/Sega CD route through `open_chd_iso9660` (pure-Rust track-1 MODE1_RAW/FORM-1 reader — sufficient for both) into their standard inspectors.
- **Dreamcast specialist routing**: metadata-only `needs_specialist_optical_backend` detects the multi-track GD-ROM shape; the optional `chd-optical-specialist` feature handles it; default builds refuse honestly. Audio-first GD-ROM low-density areas are never mistaken for the game.
- **Strict logical-media rules** (track 1, zero pregap, MODE1_RAW/FORM-1) and pregap/mixed-mode refusals are generic-CHD behavior, not Sega code — no loosening recommended.
- **Sega-side omission found**: the `MegaDrive` platform row lists `chd` as a weak extension, but `IdentityPlatform::MegaDrive` has no CHD arm — a Mega Drive CHD falls to `Deferred`. Unlike the PS2 case this is *arguably correct* (Mega Drive CHDs are cartridge-space dumps with no boot-sector identity), but the honest outcome should be an explicit decision, not silent `Deferred`.


---

## K. Cartridge product codes

| Platform | Product-code source | Parsed? | Normalized? | Persisted to identity? | Consumed by launch/cheats? | Filename metadata used? |
|---|---|---|---|---|---|---|
| Mega Drive | serial field @ 0x180 (`parse_megadrive_header`) | ✅ | raw ASCII (no normalization needed) | ❌ **not promoted** into `GameIdentityReport` | ❌ | ❌ never |
| 32X | inherits Mega Drive console-name/serial legs | ✅ | same | ❌ | ❌ | ❌ |
| SMS / GG | header product code (BCD) + system nibble | ✅ (raw BCD, undecoded by documented choice) | ❌ | ❌ (no identity dispatch at all) | ❌ | ❌ |

- **Checksums**: Mega Drive has an opt-in, real-validated whole-ROM checksum verifier (`verify_megadrive_checksum`) that the identity path does not invoke; SMS/GG checksum deliberately unvalidated (range rules uncorroborated).
- **Key gap**: the family's cartridge product codes are *parsed and tested* but never reach `IdentityKind` evidence, persistence, launch, or cheats — cartridge Sega identity is hash/DAT-only in practice.
- **Safety**: no product-code-looking filename is ever trusted anywhere in the Sega path.

## L. DAT ecosystem

| Platform | Expected primary DAT | Actual support | Hash types | Provenance / stale | Multi-disc |
|---|---|---|---|---|---|
| SMS / GG / MD / 32X | No-Intro / TOSEC (Logiqx/ClrMamePro) | ✅ generic; real-validated TOSEC counts 47 / 96 / 58 (`coverage_inventory.rs`) | CRC/MD5/SHA1 (+SHA256 model) | managed snapshots exist for MAME-SL/Redump only; user DATs keep header-text version; audit verdicts unbound to revisions | n/a |
| Sega CD / Saturn / Dreamcast | Redump | ✅ Redump parser + `<disk sha1>` lane; generic normalized matching | track SHA-1, CHD `overall_sha1` | same | `(Disc N of M)` tokens + m3u anchoring |

**No Sega-specific DAT machinery exists or is needed** — the generic pipeline (member-hash audit, set classification, dependency closure) already covers everything; the only gap is durable verdict binding (shared with all platforms).

## M. BIOS / firmware

| Platform | Modeled? | Hashes computed? | Known-good built in? | Region handling | Readiness consumed? | Doctor/GUI |
|---|---|---|---|---|---|---|
| Dreamcast | ✅ `flycast_local.rs:666-743` | ✅ size+CRC32+MD5+SHA-1 | ✅ `TRUSTED_DC_BOOT_BIN` / `TRUSTED_DC_FLASH_BIN` (two libretro sources) | ❌ single pinned record | ✅ strict native-launch gate | shown in Flycast adapter context; not in Doctor |
| Sega CD | ❌ | — | — | ❌ (bios_CD_E/U/J trio unmodeled) | ❌ RetroArch firmware needs are only ever `Unknown` | ❌ |
| Saturn | ❌ | — | — | ❌ | ❌ | ❌ |

- No copyrighted firmware is bundled anywhere (verified); the pinned-hash (Flycast) and user-supplied-DAT (`dat/firmware_evidence.rs`, whose `FirmwareSystem` enum is the documented extension point) patterns are the two acceptable routes for Sega CD/Saturn BIOS work — neither weakens verification.

## N. Flycast end-to-end

| Stage | Status | Evidence |
|---|---|---|
| Detection | ✅ | `discover_flycast_profiles` + `FlycastProfileDiscoveryRoots` |
| Readiness | ✅ strict | pinned-hash `dc_boot.bin` gate; `FlycastSystemFileState` taxonomy; `flycast_firmware_readiness` |
| Planning | ✅ | `flycast_command.rs` — argv contract, verified-product-code requirement, direct `.iso/.cue/.gdi/.chd/.cdi` only, multi-track CHD refused |
| Execution | ✅ | `flycast_execution.rs` (672 lines) with strict blocker checks |
| Doctor | ❌ | diagnostics module has no adapter consumption |
| GUI Emulator Setup | P | `FlycastProfilesState` scan/poll exists in `main.rs` (no dedicated page like `pcsx2_page`) |
| GUI Launch | ✅ | `main.rs:6450-6498` builds standalone profile inputs + binding |

**Variants**: `FlycastPlatform::{Dreamcast, Naomi, Naomi2, Atomiswave, Other}` (`flycast_local.rs:46-52`) — the non-Dreamcast variants are used only for profile eligibility filtering; `input_projection.rs:345` hardcodes `FlycastPlatform::Dreamcast`. They are **implemented but unreachable from content projection** (no arcade platform rows exist to project from) — *unreachable*, not documented as deferred.

## O. Saturn emulator support

- **No Saturn emulator exists in the repo**: zero references to yabause/Yabause/Kronos/YabaSanshiro/Mednafen-Saturn/Beetle Saturn (`grep` across all crates, non-test). RetroArch generic candidate generation *can* resolve a Saturn core whose `.info` says "Sega Saturn" (alias `segasaturn` exists), but no reviewed hint row guides it.
- **Classification of the missing join**: identity (mature) + ES-DE row (present) + DAT machinery (present) + BIOS (absent) + **launch row/adapter (absent)** → Saturn is a "ready game key, no launcher" case; the minimal join is a `LAUNCH_COMPATIBILITY` row with reviewed core hints (the Sega CD precedent), not a new adapter.

## P. RetroArch

- Sega CD: the only reviewed Sega mapping — `genesis_plus_gx` hint + the `retroarch_platform_matches` database-field special case (`platform_map.rs:255-260`), with tests.
- Generic candidates: Mega Drive/Master System/Game Gear/32X/Saturn can all resolve through `.info` `systemname`/`database` alias resolution (e.g. "Sega - MS/GG/MD/CD" → GameGear; "Sega - Mega Drive - Genesis" → MegaDrive) — but with **no reviewed rows** for those platforms, no hints, and firmware readiness permanently `Unknown`.
- GUI: RetroArch launch is fully exposed in the launch panel; safe core binding is alias-based, fail-closed, and never trusts `corename`.
- **Recommendation**: for MD/SMS/GG the generic RetroArch path is adequate once hint rows are added; a standalone Mega Drive adapter would duplicate a working path with no user-visible gain (per the brief's guidance).


---

## Q. Cheats / mods / patches

- **Dreamcast / Flycast**: cheats are keyed on the *verified* Dreamcast product code — `inspect_cheats(&profile.cheats_path.join(format!("{key}.cht")))` (`flycast_local.rs:396-398`), with a `FlycastCheatInventory` (exists/enabled/warnings). Identity is **reused** from `VerifiedIdentityFact::DreamcastProductCode`, never rediscovered.
- **RetroArch cheat library**: `patch_manager/retroarch_cheat_library.rs` maps MegaDrive platform titles; the shared RetroArch cheat flow consumes verified identity via `emulator_request_bridge`.
- **Saturn / Sega CD / SMS / GG / MD / 32X**: no platform-specific cheat or mod paths. No widescreen/60fps patch databases; Flycast's `FlycastTextureInventory` covers DC texture *inspection* only.
- **No rediscovery problems found** — existing joins are clean; the gaps are absence, not duplication.

## R. Multi-disc

- **Mechanism**: DAT-verdict-gated `(Disc N of M)` token grouping (`library_grouping.rs:187-224`, keyed by resolved platform + base title) plus m3u/cue anchoring in `playing_library` (`matching.rs:490-520` — strict tokens, "referenced discs belong to different multidisc releases" refusal). Applies uniformly to Sega CD, Saturn, Dreamcast m3u sets.
- **Election risk** (the Sony-audit concern): election excludes `(Beta)/(Proto)/(Demo)/(Sample)` and groups m3u-anchored sets, but **loose per-disc ISOs** ("(Disc 1 of 2)" without an m3u) remain individual candidates; region/revision election can elect one disc of a required set. The guard exists for anchored sets only — no Sega-specific test covers a loose two-disc Saturn election.
- **RomM/ES-DE**: multi-file sets travel through `library_plan_export` set context; GDI companion files ride support-attachment anchoring. No Sega-specific defects found.

## S. RomM

| Platform | Row | Notes |
|---|---|---|
| Dreamcast / MegaDrive / Sega 32X / MasterSystem / Sega CD | ✅ | mapped (Sega CD documents its dual alias) |
| **GameGear** | ❌ | missing row → projection fails closed |
| **Saturn** | ❌ | missing row → projection fails closed |

GDI/CHD/CDI relationships: CHD single-file (fine); GDI/CDI multi-file sets rely on generic set/support anchoring (no Sega-specific faults).

## T. ES-DE

- Reviewed rows (`launch/es_de_export.rs`): `dreamcast`, `saturn`, `segacd`, `megadrive` — names verified against ES-DE's reference `es_systems.xml` (module doc, 2026-08-23); the `megadrive`-over-`genesis` choice is documented.
- **Missing rows**: `gamegear`, `mastersystem`, `sega32x` → export fails closed for three cartridge platforms.
- **Duplicate-table risk**: `library_views.rs` still documents "no ES-DE system mapping exists yet" while `es_de_publish`/`es_de_export` own the reviewed map — the cross-family join stands.

## U. Doctor

Doctor (`diagnostics/`) currently reports none of: missing Sega BIOS, unresolved Sega platform, missing GDI companion tracks, unsupported CDI/CHD (feature-off builds), missing product code, emulator unavailable, launch blockers, or stale DAT identity. Every one of those facts *exists somewhere* (adapter states, identity reports, GDI errors, set verdicts) — the Doctor join is absent, not the data; none is classified actionable/informational because the mapping layer does not exist.

## V. Security / fail-closed

Verified safe across the family:
- `.bin` never proves Mega Drive — weak, corroboration-gated, with **positive refusal** in contradicting folders (the ScummVM `RESOURCE.GEN` precedent, `lib.rs:3604-3644`).
- `.iso`/`.cue`/`.chd` never prove Saturn or Sega CD alone — only boot-sector signatures do (`SEGA SEGASATURN` Strong; `SEGADISCSYSTEM` deliberately Corroborated).
- `.gdi` filename alone proves nothing — identity requires descriptor parse + high-density track + IP.BIN.
- `.cdi` extension alone proves nothing — identity requires trailer parse + track selection + recognized boot signature; feature-off builds refuse.
- No product-code-looking filename is trusted anywhere; no shell-string execution (argv is `OsString`-carried per the Flycast contract).
- Weak spots, honestly named: `.smd` is self-evidencing media by design (a renamed file claims the media kind, and with a folder alias the platform — the strongest extension claim in the family, accepted under the registry's documented discipline); the `TMR SEGA` header cannot separate SMS from GG (registry documents this).


---

## W. Test coverage

| Area | Tests | Notes |
|---|---|---|
| SMS/GG header | ✅ in-module (offsets, nibble, honesty) | no identity-dispatch tests (feature absent) |
| Mega Drive header + checksum | ✅ in-module, real-validated | promotion-to-identity untestable (absent) |
| SMD normalization | ✅ in-module incl. platform-signature corroboration | no canonical-hash integration tests |
| 32X | ✅ in-module incl. negative controls | no identity tests |
| Sega CD boot/product code | ✅ in-module | — |
| Saturn System ID | ✅ in-module | — |
| Dreamcast IP.BIN | ✅ in-module | — |
| GDI | ✅ `ingestion/tests.rs` + in-module (traversal, missing files, ambiguity) | no GUI-level missing-companion flow |
| CDI | ✅ in-module + `game_identity` CDI tests | — |
| CHD | ✅ reader/identity/specialist tests, real-corpus codec tests | Mega Drive CHD outcome untested (no arm) |
| BIOS (Flycast) | ✅ firmware-hash tests | Sega CD/Saturn BIOS: nothing to test |
| Flycast | ✅ command/execution/local tests | GUI panel covered by GUI tests |
| RetroArch | ✅ Sega CD special-case tests in `platform_map` | — |

---

## X. Maturity matrix

**M** MATURE · **P** PARTIAL · **O** ORPHANED · **–** MISSING · **n/a**

| | SMS | GG | MD | 32X | SegaCD | Saturn | Dreamcast |
|---|---|---|---|---|---|---|---|
| Platform registry | M | M | M | M | M | M | M |
| Media registration | M | M | M | **–** a | P b | P b | P c |
| Structural evidence | M | M | M | **O** d | M | M | M |
| Stable product/game ID | **–** e | **–** e | **P** f | **–** e | M | M | M |
| Exact DAT/hash identity | M | M | M | – (none recorded) | M | M | M |
| Persistence | M | M | M | M | M | M | M |
| BIOS/firmware | n/a | n/a | n/a | n/a | **–** g | **–** g | M |
| Emulator discovery | M (RA) | M (RA) | M (RA) | P (RA generic) | P (RA hint) | **–** | M (Flycast) |
| Readiness | P (Unknown) | P | P | P | P | – | M |
| Planning | P (RA generic) | P | P | P | P (RA) | **–** | M |
| Execution | P (RA) | P | P | P | P (RA) | **–** | M |
| GUI launch | P (RA) | P | P | P | P (RA) | **–** | M |
| Doctor | – | – | – | – | – | – | – |
| Cheats | P (RA lib) | P | P (RA lib) | – | – | – | M (Flycast cht) |
| Mods | – | – | – | – | – | – | P (textures inspect) |
| Rename | M | M | M | M | M | M | M |
| Duplicates | M | M | M | M | M | M | M |
| 1G1R | M | M | M | M | M | M | M |
| Playing Library | M | M | M | M | M | M | M |
| RomM | M | **–** h | M | M | M | **–** h | M |
| ES-DE | **–** i | **–** i | M | **–** i | M | M | M |
| Multi-disc | n/a | n/a | n/a | n/a | M | M | M |

a `.32x`/`.68k` absent from both scanner registries · b `.gdi`/`.cdi` (and Saturn cue/bin sets) not persisted as library rows · c `.gdi/.cdi` unregistered; `.iso/.chd` fine · d parser tested but production-reachable only via example-only member evidence · e no `IdentityPlatform` variant → no identity inspection, no persisted ID facts · f header facts parsed but never promoted into the identity report; SMD hashes physical-only · g nothing modeled · h RomM table has no row → fail-closed · i ES-DE table has no row → fail-closed.

---

## Y. Broken joins (ranked by user benefit)

1. **SMS/GG identity dispatch** — parser + platform rows + scanner registration all exist; `IdentityPlatform` variants + `inspect_loose_rom` coverage are the only missing pieces. Every SMS/GG user gets an identity report with no hash and no header facts.
2. **`.gdi`/`.cdi` scanner registration** — Dreamcast's two *strong* extensions, with complete parsers and launch paths, never become library rows; the watcher ignores them.
3. **32X join** — tested parser + platform row + real-validated specimen, zero production callers; needs an `IdentityPlatform` variant, a registry row, and a dispatch arm.
4. **Saturn launch** — mature identity, projected game key, ES-DE row, DAT machinery — and no `LAUNCH_COMPATIBILITY` row, no core hints, no adapter. One reviewed row + hints unlocks RetroArch launch.
5. **Mega Drive header facts → identity report** — `parse_megadrive_header` is finished; the promotion into `IdentityKind` evidence is the missing hop.
6. **SMD canonical hash** — `normalize_smd_to_bin` + transform-id pattern exist; the `push_n64_canonical_evidence` template shows exactly the missing wire.
7. **RomM GameGear/Saturn rows + ES-DE GameGear/MasterSystem/32X rows** — table literals with verified slugs/names.
8. **Sega CD BIOS readiness** — the `FirmwareSystem` extension point and the Flycast pinned-hash pattern exist; `bios_CD_E/U/J` readiness is unmodeled for the one Sega platform with a live launch path.
9. **Member-evidence production pass** — reconnects the 32X and SMS/GG member lanes (and every other cartridge family) inside normal scanning.
10. **Mega Drive CHD honesty** — decide and document (or dispatch) instead of silent `Deferred` for a weak-listed extension.

## Z. Orphaned parsers (exact modules/functions and missing joins)

| Module / function | Evidence produced | Missing join |
|---|---|---|
| `sega32x_header_evidence::{observe_sega32x_candidate, observe_sega32x_evidence, Sega32xDetector}` | Weak 32X console-name leg over a verified MD base | `IdentityPlatform::Sega32X` + `.32x` registration + identity dispatch |
| `sms_gg_header_evidence` (member-detector lane via `archive_member_content_evidence`) | TMR SEGA facts for ZIP members | production member-evidence caller (example-only today) |
| `disc_evidence_collector::collect_disc_boot_evidence` (Sega CD/Saturn/Dreamcast legs) | combined boot evidence over `LogicalMedia` | production consumer (identity re-implements per-platform instead) |
| `smd_normalization::normalize_smd_to_bin` | reversible canonical MD image + transform id | identity canonical-hash lane (N64 template) |
| `megadrive_header_evidence::parse_megadrive_header` (identity lane) | serial/region/title facts | promotion into `GameIdentityReport` |
| `ingestion/gdi.rs` (scanner lane) | GDI track resolution | `media_registry` registration so loose `.gdi` reach the parser at all |

## AA. Do not rebuild

- `megadrive_header_evidence.rs` (incl. `verify_megadrive_checksum`) — spec-verified, real-corpus checksum-validated.
- `sms_gg_header_evidence.rs` — two-source discipline incl. the deliberate checksum refusal.
- `saturn_boot_evidence.rs`, `segacd_boot_evidence.rs`, `dreamcast_boot_evidence.rs` — spec/wiki/SDK-verified with named second sources.
- `dreamcast_cdi.rs` + the `opticaldiscs::discjuggler` integration — complete, feature-gated, fail-closed (§H).
- `ingestion/gdi.rs` — bounded, traversal-safe, refusal-complete.
- The generic CHD stack incl. `needs_specialist_optical_backend` routing — explicitly complete.
- The Flycast adapter (`flycast_local/command/execution`) — strict, pinned-hash-gated, GUI-wired.
- `platform/mod.rs` Sega rows and their MagicConfidence rationale; the `media_registry` single-source-of-truth pattern.
- The DAT audit/set/dependency stack and the launch planner/readiness vocabulary — generic, mature, reused.

| Launch | ✅ per-adapter | Saturn has nothing to test |
| Doctor / loose multi-disc election | ❌ | absent features |
| Real corpus | ✅ per-platform entries in `coverage_inventory.rs` | SMS/GG fusion not exercised end-to-end (`Partial`) |

**Most important missing tests**: SMS/GG/32X identity dispatch (once wired), loose multi-disc election behavior, RomM GameGear/Saturn rows, Doctor Sega findings.


---

## AB. Prioritised backlog

### P0 — tiny / high-impact broken joins
1. **SMS/GG + 32X identity dispatch** (joins #1, #3).
2. **Register `.gdi`/`.cdi`/`.68k` in `media_registry`** (+ inspector parity) (join #2).
3. **RomM GameGear/Saturn rows + ES-DE GameGear/MasterSystem/32X rows** (join #7).
4. **Saturn `LAUNCH_COMPATIBILITY` row + reviewed core hints** (join #4).

### P1 — user-visible completeness
5. Mega Drive header-fact promotion + SMD canonical hash (joins #5, #6).
6. Sega CD BIOS readiness (region trio) via the pinned-hash or DAT-evidence pattern (join #8).
7. Production member-evidence pass (join #9) — benefits every cartridge family, Sega included.
8. Mega Drive CHD decision (document-or-dispatch) (join #10).
9. ES-DE mapping reuse in `library_views`; GUI surfacing of Sega ID facts.

### P2 — genuinely new parsers/features
10. Saturn standalone adapter (only if RetroArch proves insufficient).
11. Production-grade Saturn BIOS readiness.
12. Arcadia/System-16-adjacent extension (out of scope today — not represented at all).

**3. "Saturn launch row + reviewed core hints" — Small**
- Objective: `LAUNCH_COMPATIBILITY` row for `Saturn` with verified core `.info` text hints (yabause-family; verify against real `.info` files per the module's protocol) + readiness wiring.
- Files: `launch/platform_map.rs`, `launch/tests.rs` (update `unsupported_platform_has_no_row`).
- Reused: generic `retroarch_platform_candidate`, existing `SaturnProductCode` game key.
- Join fixed: mature identity with zero launch candidates. Non-goals: standalone adapter, BIOS verification.
- Tests: hint resolution → candidate; unresolvable `.info` → none. Benefit: Saturn users get a working launch path via RetroArch.

**4. "Mega Drive header facts + SMD canonical hash in identity" — Medium**
- Objective: promote serial/region/titles into `GameIdentityReport`; add canonical SMD SHA-256 via `normalize_smd_to_bin`.
- Files: `game_identity.rs` (`inspect_loose_rom`, new `push_megadrive_header_evidence`/`push_smd_canonical_evidence`), reuse `megadrive_header_evidence`, `smd_normalization`.
- Join fixed: parsed-but-hidden facts; physical-only SMD hashes. Non-goals: copier-header strip for raw `.bin`, checksum enforcement.
- Tests: `.md` with known serial → serial fact; `.smd` → canonical hash equals deinterleaved hash; malformed SMD → warning + physical hash retained. Regression: N64 canonical tests (template), MD loose-ROM tests.

**5. "Sega CD BIOS readiness (region trio)" — Medium**
- Objective: `bios_CD_E/U/J.bin` verification for the RetroArch Sega CD path, via a `FirmwareSystem` extension + pinned two-source hashes or user-DAT evidence.
- Files: `dat/firmware_evidence.rs` (new variants), `launch/readiness.rs` (projection), `platform_map.rs` (firmware expectation for the Sega CD row).
- Reused: `hash_firmware_file`/`matching_firmware_records`, Flycast strict-gate precedent.
- Join fixed: live launch path with unmodeled firmware. Non-goals: bundling, downloading, Saturn BIOS.
- Tests: verified/present-unverified/missing states; honest region handling. Benefit: Sega CD launch readiness stops being permanently `Unknown`.

**6. "RomM/ES-DE Sega row completion" — Tiny**
- Objective: RomM rows for `GameGear`/`Saturn`; ES-DE rows for `gamegear`/`mastersystem`/`sega32x` — each verified against RomM 5.0 / the ES-DE reference per the tables' own protocols.
- Files: `romm_platform_mapping.rs`, `es_de_export.rs` + tests.
- Join fixed: two fail-closed mapping tables. Benefit: export/projection stops silently refusing half the family.

**7. "Production member-content evidence pass" — Medium**
- Objective: call `observe_zip_member_content`/`classify_archive_content` from the scan path (bounded), feeding structural evidence and fusion.
- Files: `database.rs` (scan integration), `archive_member_content_evidence.rs` (already complete), fusion wiring.
- Join fixed: example-only member evidence (32X, SMS/GG member lanes, header normalization). Non-goals: new detectors.
- Tests: ZIP with MD/32X/SMS members → evidence recorded; encrypted/truncated refusals preserved; bounded-member limits honored. Regression: `cartridge_probe` behaviors, inspector tests.

---

## AC. Final question

**"If EmuWiz stopped adding Sega features today, what are the smallest changes required to make SMS, Game Gear, Mega Drive, 32X, Mega CD, Saturn and Dreamcast feel complete to an ordinary user?"**

- **SMS / Game Gear**: add the two `IdentityPlatform` variants and let the existing TMR SEGA parser feed the standard loose-ROM identity (task 1). One change converts them from "extension-catalogued, identity-blind" to first-class cartridge platforms.
- **Mega Drive**: promote the already-parsed header facts (serial/region) and optionally the SMD canonical hash (task 4). Everything else — hashing, DAT, rename, 1G1R, RetroArch — already works.
- **32X**: task 1's dispatch arm plus one `MEDIA_FORMATS` row. A tested parser has been sitting behind two one-line joins.
- **Mega CD**: BIOS readiness for the RetroArch path (task 5). Identity, DAT, launch hints, and mappings are done.
- **Saturn**: task 3 — one reviewed compatibility row with core hints. Identity and ES-DE are already done; this is the entire remaining distance to "launchable".
- **Dreamcast**: register `.gdi`/`.cdi` (task 2). Detection, identity, BIOS verification, cheats, launch, GUI are the most complete vertical in EmuWiz; registration is the only gap an ordinary user can hit.
- **Across all seven**: the RomM/ES-DE row completions (task 6) so exported libraries stop failing closed for GameGear and Saturn.

Total: two `IdentityPlatform` variants plus a handful of dispatch arms, three registry rows, two mapping-table literals, one compatibility row, and one firmware projection — every one a connection between pieces that already exist and already have tests.


### Best 7 isolated implementation tasks

**1. "Sega 8-bit + 32X loose-ROM identity" — Medium**
- Objective: `IdentityPlatform::{MasterSystem, GameGear, Sega32X}` + dispatch + loose-ROM hashing + TMR-SEGA/32X header-fact promotion.
- Files: `game_identity.rs` (`IdentityPlatform`, `from_catalogue`, `supported_loose_rom_format`, `inspect_loose_rom`, new `push_*_evidence` helpers modeled on `push_n64_canonical_evidence`), `media_registry.rs` (`32x`, `68k`), `content_registry.rs`, `coverage_inventory.rs`.
- Reused: `sms_gg_header_evidence`, `sega32x_header_evidence`, the bounded-hash+stability `inspect_loose_rom` pattern.
- Join fixed: parsers with zero identity reachability. Non-goals: checksum validation, BCD decoding, platform-fusion changes.
- Tests: synthetic TMR SEGA at 3 offsets → Verified hash + nibble fact; `.32x` recognized; headerless files hash-only; untrusted platform → `Ambiguous`. Regression: all existing detector suites, `media_registry` tests, `game_identity` MD tests.
- Benefit: SMS/GG/32X owners get real identity + persisted hashes for the first time.

**2. "Register Dreamcast `.gdi`/`.cdi` (and `.68k`) as library media" — Tiny**
- Objective: one-line `MEDIA_FORMATS` entries (+ tests) so loose GDI/CDI become catalogued rows and watcher events.
- Files: `media_registry.rs`; optional `content_registry` parity.
- Reused: everything (identity/Flycast already work).
- Join fixed: strong extensions invisible to the scanner. Non-goals: cue/bin registration, multi-file set persistence.
- Tests: `kind_for_extension("gdi"/"cdi")`, watcher relevance, discovery rows; regression: registry uniqueness tests.

