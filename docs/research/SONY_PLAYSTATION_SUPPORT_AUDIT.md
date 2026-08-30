# Sony PlayStation Family Support Audit — EmuWiz (READ-ONLY)

**Scope:** PS1, PS2, PSP, PS3
**Branch:** `feature/archivefs-unified-platform`
**Method:** static source analysis only — no builds, no edits to source, no commits.
**Related:** `docs/research/NEC_SUPPORT_AUDIT.md` (shared optical/CHD findings reused, not restated).

---

## A. PLATFORM MODEL

`platform/mod.rs`:

| | PSX (`:1616-1639`) | PS2 (`:1641-1657`) | PS3 (`:1659-1670`) | PSP (`:1672-1688`) |
|---|---|---|---|---|
| display | Sony PlayStation | Sony PlayStation 2 | Sony PlayStation 3 | Sony PlayStation Portable |
| aliases | `psx`, `ps1`, `playstation`, `playstation1`, `sonyplaystation`, `sonyplaystation1` | `ps2`, `playstation2`, `sonyplaystation2` | `ps3`, `playstation3`, `sonyplaystation3` | `psp`, `playstationportable`, `sonypsp`, `sonyplaystationportable` |
| strong ext | `pbp`, `ecm` | *(none)* | `pkg` | `cso`, `pbp` |
| weak ext | iso, cue, bin, img, chd, mdf, ccd, zip | iso, cue, bin, img, chd, mdf, zip | iso, zip | iso, chd, zip |
| magic | `"PLAYSTATION"` @ 0x8008 (ISO9660 system id) — **Corroborated** | same magic, **Corroborated** ("confirms the family, folder separates generations") | none | none |
| conflicts | PS2, Sega CD, CD-i, 3DO, PC Engine CD | PSX | — | — |

- `IdentityPlatform::PlayStation | PlayStation2 | Psp | PlayStation3` all exist (`game_identity.rs:266-269`).
- Launch rows: all four present in `LAUNCH_COMPATIBILITY` (`launch/platform_map.rs:84-107`).
- ES-DE: `psx`, `ps2`, `ps3`, `psp` all mapped (`launch/es_de_export.rs:111-131`).
- RomM outbound (`platform_evidence_fusion/romm_platform_mapping.rs`): PSX→`ps` (`:165-170`), PSP→`psp` (`:172`), PS3→`ps3` (`:179`); **PS2 has no outbound row** (grep for `PS2` in that table: empty). Inbound slug aliases cover the family.
- coverage_inventory: all four have rows with **real-corpus validation** — PSX (`ps1_system_cnf_boot`, real CHD specimen, `:167-178`), PS2 (`ps2_system_cnf_boot2_strong`, real ISO, `:180-192`), PSP (`psp_umd_data_bin_strong`, real UMD ISO, `:193-205`), PS3 (real 3.5 GB `.pkg` specimen, `:206-217`).
- **Drift:** none between subsystem names; the one cross-subsystem hazard (PS1↔PS2 shared `PLAYSTATION` magic) is explicitly modeled as family-confirmation + conflict pair.

## B. PS1 MEDIA / IDENTITY

- **BIN/CUE**: `discover_cue` (`ingestion/discovery.rs:376+`) resolves sheets; `.cue` identity arm covers `PlayStation` (`game_identity.rs:786-800`, `inspect_cue`).
- **ISO**: generic `.iso` arm (`:801-809`) + platform magic `"PLAYSTATION"`@0x8008.
- **CHD**: `.chd` arm covers `PlayStation` (`:823-836`, `inspect_disc_chd`); track-1/zero-pregap rules are strict and fail-closed (`chd_logical_media.rs:234-258`). Real CHD specimen validated end-to-end (coverage row).
- **CCD/IMG/SUB, MDF/MDS**: weak extensions only; **no CCD/SUB parser** (subchannel data unread; `ccd` is on the shared-denylist).
- **ECM**: a PSX **strong** extension with **no decompressor and no registry row** outside `inspector.rs:121` — a `.ecm` file skips discovery as `UnsupportedExtension`.
- **PBP**: strong on PSX *and* PSP; a complete parser exists (`psp_pbp_evidence.rs`: `looks_like_pbp:87`, `PbpHeaderFact:97`, `validate_pbp_offsets:180`, `read_pbp_param_sfo:226`, `read_data_psar_prefix:235`, `observe_pbp_evidence:253`) — **but it is wired to nothing** (no `ContentDetector` impl, no registry row, no identity arm). PSN-style PBP dumps are invisible end-to-end.
- **SYSTEM.CNF / serial**: `playstation_boot_evidence.rs` parses `BOOT=` (PS1) via `parse_system_cnf_boot:100` with key-case tolerance and first-line-wins; `PS-X EXE` magic (`:55`); serial normalization `serial_from_boot_path` (`game_identity.rs:4825-4839` — strict `SLUS_123.45` → `SLUS-12345`, fails closed) + `is_supported_ps1_serial:4841`.
- **Persistence of serial**: `IdentityKind::Ps1Serial` exists (`game_identity.rs:187`) with a verified-value getter (`:545-557` family). Serials are *re-verified at launch* ("freshly re-confirmed", `duckstation_command.rs:142`); the scanner→catalogue→persisted-serial join runs through the launch-layer `evidence_bridge` (`launch/evidence_bridge.rs`), not through stored catalogue rows.
- **Three concepts kept separate**: platform (magic + SYSTEM.CNF boot leg), serial (validated `Ps1Serial` fact), release identity (Redump/hash via the generic DAT pipeline). The fusion rule `ps1_system_cnf_boot` resolves platform; nothing claims a title from a serial.
- **Multi-disc**: DAT/set machinery (`dat/set.rs`, `library_grouping.rs` `MultiDiscSet:171-176`) + TOSEC naming; DuckStation multi-disc via `.m3u` is **not modeled in the launch layer** (no `m3u` reference under `launch/`).

## C. PS2 MEDIA / IDENTITY

- **ISO**: `.iso` arm + `ps2_boot_evidence.rs` — `parse_ps2_system_cnf:65` accepts **`BOOT2=` only** (a `BOOT=` PS1 fact is rejected), bounded ELF magic check (`observe_ps2_boot:50`); doc correctly states `BOOT2` + ELF are "still only two" corroborating facts, with the strong leg reviewed in Batch 6 (coverage row: real God of War ISO "Resolved: PS2, not just a candidate").
- **CHD**: `.chd` arm covers `PlayStation | Saturn | Dreamcast | SegaCd | ThreeDo` (`game_identity.rs:823-836`) — **`PlayStation2` is absent**, so a PS2 `.chd` falls into the `Deferred` catch-all ("format has no existing safe bounded reader", `:837-842`) even though the reader itself is safe for PS2's track-1 layout. Same defect class PC-FX had (see NEC audit §11).
- **Serial + executable CRC**: `IdentityKind::Ps2Serial` and `IdentityKind::Pcsx2ExecutableCrc` both exist with verified getters (`:545-557`). `patch_manager/pcsx2_identity.rs` joins them: PS2 cheats require *both* (`:84`), with PCSX2-region derived from the serial prefix (`pcsx2_region_for_serial`, `:92,108`).
- **PCSX2 linkage**: `pcsx2_pnach.rs` is a full `.pnach` document pipeline (`parse_pnach_document:140`, `merge_managed_pnach_cheats:197`, `extract_managed_blocks:293`, `append_raw_managed_blocks:321`, `remove_managed_blocks:374`) with managed-block IDs and original-byte retention; `pcsx2_provider.rs`, `pcsx2_local.rs`, `pcsx2_install_plan.rs`, `pcsx2_firmware/` complete the PCSX2 stack.
- **Launch**: `pcsx2_command.rs:118` requires a **verified PS2 serial** ("a verified PS2 serial is required, not merely preferred", `:114-118`) and a *direct regular `.iso`* (`:14`) — PCSX2 planning is deliberately ISO-only; CHD/other containers are not launch-eligible today.
- **Duplication check**: none found — one SYSTEM.CNF parser family (`game_identity`'s reviewed parser + the pure `ps2_boot_evidence` observer that reuses it), one CRC kind, one pnach module.
- **No-Intro PS2 (DVD)** would flow through generic DAT hashing; nothing PS2-specific is hardcoded.

## D. PSP

- **ISO**: `.iso` arm + `psp_boot_evidence.rs` — `PspLayoutObservation` (`:33-63`): `PSP_GAME/PARAM.SFO` + `UMD_DATA.BIN` presence, `DISC_ID`/title/category/`disc_version` via the generic `param_sfo` parser (`param_sfo.rs:125` `parse_param_sfo`, `:201` `product_code_evidence` — deliberately "candidate only" semantics).
- **UMD strong leg**: `UMD_DATA.BIN` at the disc root is `Strong` `BootStructure` ("PSP-UMD-exclusive, no other Sony optical format uses this file", `:75-93`) — but a *digital PSN-style dump* without it "still never resolves" (`:89-91`), honest.
- **CSO**: a PSP **strong** extension, inspector-listed, but the `"chd" | "cso" | ...` identity arm is the **Deferred** catch-all (`game_identity.rs:837`) — CSO is registered-but-uninspectable (no bounded CSO reader).
- **CHD**: weak PSP extension; excluded from the `.chd` arm like PS2.
- **PBP**: strong extension + complete parser — **unwired** (see §B).
- **Launch/readiness**: `ppsspp_command.rs:67` requires a **verified PSP disc ID** (fails closed, `:102-105`); `ppsspp_firmware_readiness()` is a constant (`launch/readiness.rs:403` — PPSSPP bundles what it needs, so no fake BIOS gate); `patch_manager/ppsspp_local.rs` provides bounded profile inspection.
- **Encryption assumptions**: none made — no encrypted-ISO handling claimed anywhere; nothing pretends to decrypt.

## E. PS3

What "a PS3 game" means today, kept as four distinct notions:

1. **Extracted disc game** — `ps3_boot_evidence.rs`: `PS3_GAME/`, `PS3_GAME/USRDIR`, `USRDIR/EBOOT.BIN`, `PARAM.SFO` layout (`:24-27`); `TITLE_ID`/title/category/`app_version` via PARAM.SFO (`:45-57`); bounded SELF magic check (`check_eboot_self_magic:64`). `Corroborated`-level evidence; PS3/PSP layout collision handled by the `USRDIR/EBOOT.BIN` path distinction (`:8-10`).
2. **Disc-image game** — `ps3_disc_evidence.rs`: `PS3_DISC.SFB` checked as **magic-only** (`.SFB` @ `:73`, `looks_like_ps3_disc_sfb`); `TITLE_ID`/`HYBRID_FLAG` extraction deliberately not implemented ("single-source corroboration bar", `:30-41`).
3. **Digital package** — **PKG**: bounded fixed 0x80-byte header parser, two-source corroborated (PS3 Dev wiki `pkg_files` + PS3Py `pkg.py`, `:26-40` module doc).
4. **Installed RPCS3 game / launchable** — `rpcs3_command.rs:72` requires a **verified PS3 TITLE_ID** (fails closed `:111`) *and* RPCS3 firmware readiness ≠ Unknown (`:142-145` — "the firmware location could not be verified").

**License material (RAP/EDAT)**: absent — no RAP/license concepts modeled. Correctly not pretended.
**Platform row**: `.pkg` is a PS3 **strong** extension but is registered in **no scanner registry** (not even `inspector.rs`) — the real `.pkg` specimen was validated by the evidence modules directly, not through end-to-end discovery.

## F. DISC CONTAINER LAYER

Sony rides the generic optical stack everywhere: ISO9660 primary-VD parsing with bounded `find_path` (`iso9660.rs`), CUE/BIN resolution (`ingestion/cue_bin.rs`), CHD logical media with strict track-1/zero-pregap rules and GD-ROM-style specialist refusal (`chd_logical_media.rs`, `chd_identity.rs`, `disc_evidence_collector.rs`). `collect_disc_boot_evidence` (`disc_evidence_collector.rs:253+`) carries the Sony branches (SYSTEM.CNF → PS1/PS2; PSP layout; no UDF/Joliet/Rock-Ridge/El-Torito parsing — supplementary descriptors are skipped, not parsed). **No Sony-specific duplication of optical parsing found** — `psp_boot_evidence`/`ps3_boot_evidence` consume `iso9660::find_path` results rather than re-walking trees.

## G. DAT ECOSYSTEMS

| Platform | Expected primary DAT | Actual support | Hash types | Serial retained | Multi-disc |
|---|---|---|---|---|---|
| PS1 | Redump | generic `dat/audit` + `identity_source/redump` | track/CUE-aware disc hashing, CHD SHA-1 | `Ps1Serial` fact | DAT set membership; no m3u launch grouping |
| PS2 | Redump (DVD) + No-Intro | generic | ISO/disc hashes + `Pcsx2ExecutableCrc` | `Ps2Serial` | same |
| PSP | No-Intro (PSN) + Redump | generic; PBP dumps not ingestible | ISO hashes; SFO DISC_ID as fact | `PspDiscId` | same |
| PS3 | (no standard DAT ecosystem; Redump for disc images) | generic | PKG/SFB structural facts; file hashes | `Ps3TitleId` | same |

Stale handling: `identity_source/stale.rs` (generic). **No platform-specific DAT hacks exist or are needed.**

## H. FIRMWARE / BIOS

- **PS1**: `duckstation_firmware_readiness(DuckStationBiosState)` (`launch/readiness.rs:319`) — state produced by `diagnostics/profiles.rs` + `patch_manager/duckstation_local.rs` (bounded profile/config reads, `DUCKSTATION_MAX_PROFILES: 16`, 256 KB config / 512 KB cheat caps). GUI: doctor page renders DuckStation/PPSSPP profile inspections (`gui/tests/doctor_and_repair.rs:2416`).
- **PS2**: the deepest story — `FirmwareSystem::PlayStation2` + Redump-BIOS DAT verification (`dat/firmware_evidence.rs`, `patch_manager/pcsx2_firmware/`), `pcsx2_firmware_readiness(Pcsx2BiosVerification)` (`readiness.rs:333`), dedicated **GUI `pcsx2_page.rs`**.
- **PSP**: constant readiness (`readiness.rs:403`) — correct: nothing to verify.
- **PS3**: `rpcs3_firmware_readiness(Rpcs3FirmwareStatus)` (`readiness.rs:347`) + dedicated **GUI `rpcs3_page.rs`**; RPCS3 *firmware installation* is modeled as a status (with unknown → launch blocker in the command planner), **not** as a BIOS file — exactly the distinction the task asks for. RAP/EDAT licenses: absent.
- Unknown/stale firmware behavior: Unknown readiness blocks RPCS3 planning; stale-DAT handling is generic.
- **Stale/unknown firmware never fabricates a pass.**

## I. EMULATOR DISCOVERY / READINESS

| Emulator | Detection | Readiness | Planning | Execution |
|---|---|---|---|---|
| DuckStation | Yes (`diagnostics/profiles.rs`, `patch_manager/duckstation_local.rs`) | Yes (BIOS state) | Yes (`duckstation_command.rs`, serial-gated, fail-closed) | Yes (`launch/duckstation_execution/` + tests) |
| PCSX2 | Yes (`patch_manager/pcsx2_local.rs`, `resolved_emulator_profile.rs`) | Yes (Redump-verified BIOS) | Yes (`pcsx2_command.rs`, serial-gated, ISO-only) | Yes (`launch/pcsx2_execution/` + tests) |
| PPSSPP | Yes (`patch_manager/ppsspp_local.rs`) | Constant (nothing needed) | Yes (`ppsspp_command.rs`, disc-ID-gated) | Yes (`launch/execution.rs` discipline; `ppsspp` in `launch/`) |
| RPCS3 | Yes (`patch_manager/rpcs3_local.rs`) | Yes (firmware status; GUI page) | Yes (`rpcs3_command.rs`, TITLE_ID + firmware-gated) | Yes (`launch/rpcs3_execution.rs`) |

Flatpak/AppImage handling: generic executable probing exists in the environment layer (`emulator_environment/`, `FsProbe`/`ExecutableProbe`); PSX-era AppImage naming appears in ES-DE tests. **No complete-but-unwired Sony adapter exists** — all four are wired end-to-end at the launch layer.

## J. LAUNCH CHAIN

Verified game → platform (magic/SYSTEM.CNF/UMD/layout legs) → required identity fact (**PS1 serial / PS2 serial / PSP disc ID / PS3 TITLE_ID — every planner fails closed without it**) → emulator profile → firmware readiness (RPCS3 gate) → command plan (`build_*_command_plan`, argv-shaped, never shell strings) → execution adapters with re-verification ("freshly re-confirmed"). **Serial/title IDs are genuinely load-bearing at launch** — the strongest identity discipline of any family in the repo. **Break points**: PS2 `.chd` and PSP `.cso`/`.pbp` never reach identity (Deferred/unsupported), so those containers cannot launch at all; multi-disc `.m3u` launching is not modeled in the launch layer.

## K. CHEATS / PATCHES / MODS

- **PS1**: `patch_manager/duckstation_local.rs` — bounded, read-only profile/cheat inspection; "DuckStation consumes identity that core has already established"; Redump/CHD evidence reused, never re-derived. Serial linkage via `Ps1Serial`.
- **PS2**: the flagship join — `pcsx2_identity.rs` (serial + executable CRC + region-from-serial) feeds `pcsx2_pnach.rs` (validated document surgery with managed-block rollback via `cheat_rollback.rs`); GUI pcsx2 page renders workflow states (`gui/tests/doctor_and_repair.rs:150`).
- **PSP**: `ppsspp_local.rs` bounded inspection; disc-ID linkage.
- **PS3**: `rpcs3_local.rs` bounded inspection; TITLE_ID linkage; no patch-manager module for RPCS3 patch databases.
- **Identity rediscovery duplication: none** — the cheat layer consumes `verified_value(IdentityKind::…)` from the same identity report the launch layer uses.

## L. LIBRARY / GUI

An ordinary user sees: platform (registry), DAT verification (generic identity pages), emulator readiness (doctor + pcsx2/rpcs3 pages), cheat/mod workflow states. **Backend facts that exist but are not surfaced in normal library views**: verified serial/disc-ID/TITLE-ID (identity-report scoped, launch-time), executable CRC, PSP disc_version, PS3 app_version, SFO titles (provenance-grade), region-from-serial. Serial persistence into ordinary catalogue rows is the main hidden gap (they live in identity reports/evidence bridge, re-verified per launch).

## M. RENAME / 1G1R / PLAYING LIBRARY

- Canonical rename is hash-authoritative (`dat/rename_plan`/`rename_apply`); SFO titles and serials never rename.
- Duplicate/1G1R/Playing-Library machinery is generic (`dat/divergence.rs`, `clone_report.rs`, `playing_library::elect_family` with `ElectionExplanation`).
- **Multi-disc risk**: `MultiDiscSet` (`library_grouping.rs:171-176`) groups by `(platform, base_title)` with declared totals — but its consumers (`library_grouping`, `set_destination`, `full_library_report`) are the example-stage fusion layer; the production election path (`playing_library`) has **no evidence of disc-number-aware election**. A required multi-disc release could elect a single representative per family election; no code was found that guarantees all discs of an elected release. Treat as a real, unproven-until-tested risk (P1), not a confirmed defect.
- PS1/PS2 regional variants and demo/beta/prototype: TOSEC/Redump metadata via generic DAT naming (`identity_source/tosec/filename_metadata.rs`).

## N. ROMM / ES-DE

- ES-DE: `psx`/`ps2`/`ps3`/`psp` rows exist with reviewed fullnames; folder names match ES-DE's own; launch paths remain valid because planners are emulator-argv based.
- RomM: PSX→`ps`, PSP→`psp`, PS3→`ps3` mapped; **PS2 outbound missing** — a real export gap for the biggest PS platform.
- Multi-file grouping: RomM projection relies on the generic playing-library plan; CHD/CUE pairing survives via generic container handling; **PS3 folder games have no reviewed projection story** (extracted `PS3_GAME/` directories are not a RomM-file concept; nothing breaks, nothing projects either).
- PSP PBP/CSO: correctly *not* projected today because they are not ingestible (§D).

## O. DOCTOR

Already exists (do not re-add): emulator profile inspections (DuckStation/PPSSPP doctor rows, `doctor_and_repair.rs:2416,2510,2622`), PCSX2/RPCS3 pages with workflow states, firmware status presentation, DAT staleness (generic). **Missing Sony-specific Doctor findings**: "identity verified but no verified serial → launch will fail closed" (the most common real-world Sony blocker), "container Deferred (PS2 .chd / PSP .cso/.pbp) so identity can never verify", "PS3 firmware unknown", "RAP/license material absent" (once modeled). None of these are destructive-fix candidates.

## P. SECURITY / FAIL-CLOSED BEHAVIOR

Verified sound:
- `.bin`/`.iso` never prove PS1/PS2 — the row explanations and the magic `Corroborated` (not Strong) + conflict pairs enforce it (`platform/mod.rs:1629-1639, 1652-1656`).
- Serial-looking filenames are never serials — `serial_from_boot_path` reads `SYSTEM.CNF`'s `BOOT=`/`BOOT2=` *content*, strictly format-checked; a serial-named file proves nothing.
- `PARAM.SFO` presence alone is never PS3/PSP identity — layout + magic + product-code-as-candidate semantics (`psp_boot_evidence.rs:11-12`, `ps3_boot_evidence.rs:13`).
- No shell-string execution anywhere in the launch layer — argv-shaped plans only (`duckstation_command.rs:5` doc).
- PKG/SFB: bounded fixed headers, two-source corroboration, no payload interpretation.
- One soft spot: the `"PLAYSTATION"`@0x8008 magic is shared by PS1/PS2 by design; folder evidence is the separator, and an unaliased PS2 ISO resolves only through the `ps2_system_cnf_boot2_strong` leg — correct, but worth knowing.

## Q. TEST COVERAGE

- **Evidence parsers**: `playstation_boot_evidence` (SYSTEM.CNF/PS-X EXE), `ps2_boot_evidence` (BOOT2/ELF), `psp_boot_evidence` (layout/UMD/SFO), `psp_pbp_evidence` (offsets/SFO/PSAR), `ps3_boot_evidence` (layout/SELF), `ps3_disc_evidence` (SFB/PKG two-source) — all with malformed/truncated cases per the crate's fail-closed house style.
- **Launch**: `duckstation_execution/tests.rs`, `pcsx2_execution/tests.rs`, `rpcs3_execution.rs` + command-planner tests (fail-closed serial gates covered).
- **Firmware**: `dat/firmware_evidence/tests.rs` (PS/PS2/Xbox pinned, "no other variants" test), `patch_manager/pcsx2_firmware/tests.rs`.
- **Cheats**: `pcsx2_pnach` document tests; cheat-rollback tests (generic).
- **GUI/Doctor**: `doctor_and_repair.rs` (profile inspections, workflow rows), `emulator_profiles_and_setup.rs`.
- **Real corpus**: all four coverage rows are `RealValidated`.
- **Gaps with no tests**: PS2 `.chd` identity (Deferred — no test because no path), PSP `.cso`/`.pbp` end-to-end (none exist), `.pkg` end-to-end discovery (parser tested, discovery path absent), multi-disc election behavior for Sony releases (no test found).

## R. MATURITY MATRIX

| | PS1 | PS2 | PSP | PS3 |
|---|---|---|---|---|
| Platform registry | MATURE | MATURE | MATURE | MATURE |
| Media registration | PARTIAL — `.pbp`/`.ecm` strong exts skip discovery (`inspector.rs:121` only) | MATURE (iso/cue/chd flow) | PARTIAL — `.cso`/`.pbp` skip discovery | PARTIAL — `.pkg` strong ext has no registry row at all |
| Structural evidence | MATURE (SYSTEM.CNF + magic + CHD leg) | MATURE (BOOT2+ELF strong leg) | MATURE (UMD_DATA.BIN strong leg) | MATURE (layout/SFB/PKG, corroborated) |
| Stable game ID | MATURE (`Ps1Serial`, launch-gated) | MATURE (`Ps2Serial` + CRC) | MATURE (`PspDiscId`) | MATURE (`Ps3TitleId`) |
| Exact DAT/hash identity | MATURE | MATURE | PARTIAL — PBP dumps can't be hashed (not ingestible) | PARTIAL — PKG discovery-invisible |
| Persistence | PARTIAL — serials re-verified per launch, not stored as catalogue rows | PARTIAL — same | PARTIAL — same | PARTIAL — same |
| Firmware | MATURE (BIOS state) | MATURE (Redump-verified) | MATURE (correctly constant) | MATURE (install status, distinct from BIOS) |
| Emulator discovery | MATURE | MATURE | MATURE | MATURE |
| Readiness | MATURE | MATURE | MATURE | MATURE (Unknown blocks launch) |
| Command planning | MATURE (fail-closed serial) | MATURE (fail-closed serial, ISO-only) | MATURE (fail-closed disc ID) | MATURE (fail-closed TITLE_ID + firmware) |
| Execution | MATURE | MATURE | MATURE | MATURE |
| Doctor | PARTIAL — profile rows exist; no "identity-verified-but-no-serial" finding | PARTIAL — same shape | PARTIAL | PARTIAL — firmware shown; license concept absent |
| GUI | MATURE | MATURE (dedicated page) | PARTIAL — no dedicated page | MATURE (dedicated page) |
| Cheats/patches | PARTIAL — local inspection only; no DuckStation cheat provider | MATURE (pnach pipeline, CRC-gated) | PARTIAL — local inspection only | PARTIAL — local inspection only |
| Mods | PARTIAL | PARTIAL (pnach-managed blocks are the model) | MISSING | MISSING |
| Rename | MATURE (hash-authoritative) | MATURE | MATURE | MATURE |
| 1G1R | MATURE (generic) | MATURE | MATURE | MATURE |
| Playing Library | MATURE | MATURE | MATURE | PARTIAL — folder games lack a projection story |
| RomM | MATURE (`ps`) | PARTIAL — **no outbound slug** | MATURE (`psp`) | MATURE (`ps3`) |
| ES-DE | MATURE | MATURE | MATURE | MATURE |
| Multi-disc handling | PARTIAL — `MultiDiscSet` is fusion-stage; no m3u launch; election risk untested | PARTIAL — same | PARTIAL — same | N/A (folder/PKG model differs) |

## S. BROKEN JOINS (both ends exist, not connected)

1. **PSP/PSX `.pbp`**: complete parser (`psp_pbp_evidence.rs`, incl. `read_pbp_param_sfo` → DISC_ID) + strong platform extensions + inspector listing — but no registry row, no detector, no identity arm. PSN-style dumps are the *modern* PSP distribution form and are entirely invisible.
2. **PS2 `.chd`**: the CHD reader is safe and PlayStation-proven (PS1 real specimen); the `.chd` identity arm just doesn't list `PlayStation2` (the exact PC-FX defect already diagnosed in the NEC audit).
3. **PSP `.cso`**: strong extension + inspector listing + a trivially bounded container format — sits in the `Deferred` catch-all.
4. **PS3 `.pkg`**: strong extension + two-source-corroborated bounded parser (`ps3_disc_evidence::parse_pkg_header`) — no registry row, no discovery route; validated only as a direct specimen.
5. **PS2 RomM export**: outbound rows exist for PSX/PSP/PS3/Vita; PS2 alone is missing from `romm_platform_mapping.rs`.
6. **Serial → catalogue**: `IdentityKind::Ps1Serial`/`Ps2Serial`/`PspDiscId`/`Ps3TitleId` are verified at identity time and consumed at launch/cheat time, but never persisted as ordinary catalogue facts, so normal library views and Doctor cannot show "launch will fail: no serial" before the user tries.
7. **Multi-disc**: `MultiDiscSet` grouping exists (fusion layer) + launch planners exist — but no m3u/disc-sequence projection connects them for PS1/PS2 multi-disc releases.

## T. DO NOT REBUILD LIST

- **The four command planners + execution adapters** (`launch/{duckstation,pcsx2,ppsspp,rpcs3}_command.rs`, `*_execution/`) — serial-gated, fail-closed, argv-shaped, tested. Wire, don't touch.
- **`serial_from_boot_path` + `IdentityKind::{Ps1Serial,Ps2Serial,PspDiscId,Ps3TitleId,Pcsx2ExecutableCrc}`** — the identity spine; every Sony join already hangs off it.
- **`playstation_boot_evidence` / `ps2_boot_evidence` / `psp_boot_evidence` / `ps3_boot_evidence` / `ps3_disc_evidence`** — reviewed, real-corpus-validated, honest about evidence strength.
- **`param_sfo.rs`** — generic, shared by PSP/PS3, "candidate only" product-code semantics.
- **`patch_manager/pcsx2_pnach.rs` + `pcsx2_identity.rs`** — the managed-block pnach pipeline with rollback; the model for any future cheat provider.
- **`patch_manager/{duckstation,ppsspp,rpcs3,pcsx2}_local.rs`** — bounded read-only emulator inspection with explicit caps.
- **Generic optical stack** (`iso9660.rs`, `cue_bin.rs`, `chd_logical_media.rs`, `disc_evidence_collector.rs`) — including the strict track rules; extend arms, never loosen.
- **Firmware infrastructure** (`dat/firmware_evidence.rs` hashing/matching, `patch_manager/pcsx2_firmware/`, `readiness.rs` projections).
- **Generic DAT machinery + 1G1R/clone/Playing-Library election** — identity stays hash-driven.
- **The `PLAYSTATION`-magic conflict pair** (PSX↔PS2 and vs Sega CD/CD-i/3DO/PE-CD) — correct as designed.

## U. PRIORITISED BACKLOG

**P0 (joins where both ends exist)**
1. Register + wire `.pbp` (PSX+PSP): registry rows, `PbpDetector`-style route or identity arm; unlocks PSN-style PSP/PS1 dumps and their DISC_ID evidence (parser already complete).
2. Add `PlayStation2` to the `.chd` identity arm (`game_identity.rs:823-836`) — one-line join, mirrors the already-fixed PC-FX pattern; then PS2 CHD becomes launch-eligible (needs a PCSX2-command eligibility decision: keep ISO-only or extend).
3. Register `.pkg` (PS3) end-to-end: content/media registry rows + identity arm consuming `parse_pkg_header`; bounded header already two-source reviewed.

**P1**
4. PSP `.cso` bounded reader (simple, fixed-format) + identity arm — removes the last "registered-but-uninspectable" Sony container.
5. Persist verified Sony identity facts (`Ps1Serial`/`Ps2Serial`/`PspDiscId`/`Ps3TitleId`) as catalogue-level facts so Doctor/library views can pre-warn "launch will fail closed without a serial" and GUI can display serials.
6. RomM outbound row for PS2 (`ps2`).
7. Multi-disc: project `MultiDiscSet` membership into launch (m3u for DuckStation) and make Playing-Library election disc-number-aware for elected multi-disc releases; add the missing election test.
8. Doctor findings: identity-verified-but-no-serial; container-Deferred; PS3 firmware Unknown (exists) + "no license material" only if RAP is ever modeled.

**P2**
9. `.ecm` decision: decompress (bounded ECM is simple) or drop the strong-extension claim — currently a dead claim.
10. DuckStation/PPSSPP/RPCS3 cheat *providers* modeled on the PCSX2 pnach pipeline; RPCS3 patch-db module.
11. CCD/SUB subchannel awareness; PS3 folder-game RomM projection convention.

**BEST 5 TASKS**

1. **`ps2-chd-identity-arm`** — add `IdentityPlatform::PlayStation2` to the `.chd` match arm (`game_identity.rs:823-836`), reuse `inspect_disc_chd` unchanged; non-goals: no CHD-rule loosening, no PCSX2 command changes; tests: real PS2 CHD specimen resolves serial + platform, truncated CHD refuses; benefit: the single most common modern PS2 format becomes identity-verifiable and launchable-adjacent.
2. **`psp-pbp-end-to-end`** — register `.pbp` in `media_registry`/`content_registry` (DiscImage), add a `.pbp` identity arm calling `psp_pbp_evidence::{parse_pbp_header, validate_pbp_offsets, read_pbp_param_sfo}` to emit `PspDiscId` (and `Ps1Serial` for PS1 PBP via DATA.PSAR inner check only if two-source verifiable); non-goals: no PSAR decompression, no encryption handling; tests: offset-validated PBP → DISC_ID fact → PPSSPP planner unblocks; malformed offsets refuse; benefit: PSN-style dumps (the dominant PSP preservation form) become first-class.
3. **`ps3-pkg-discovery`** — registry rows for `.pkg` + identity arm consuming `ps3_disc_evidence::parse_pkg_header` → `Ps3TitleId`; non-goals: no PKG payload extraction, no license/RAP modeling; tests: real-header PKG → TITLE_ID → RPCS3 planner unblocks; truncated header refuses; benefit: PS3 digital installs (the strong-extension case) finally flow scanner→launch.
4. **`sony-serial-persistence`** — persist verified `Ps1Serial`/`Ps2Serial`/`PspDiscId`/`Ps3TitleId` as catalogue identity facts + GUI columns + a Doctor pre-warning when a mapped platform's content lacks its launch-gating fact; non-goals: no identity-from-filename; tests: persist → display → Doctor round-trip; benefit: users see *why* launch refuses before launching.
5. **`ps2-romm-outbound`** — add the PS2 row to `romm_platform_mapping.rs` (slug `ps2`, RomM 5.0 provenance like its siblings) + regression test in `every_required_platform_is_mapped`-style tests; non-goals: no inbound changes; benefit: PS2 (the largest Sony library) exports to RomM.

## V. FINAL QUESTION

**"If we stopped adding Sony features today, what are the smallest changes required to make PS1, PS2, PSP and PS3 feel complete to an ordinary EmuWiz user?"**

- **PS1** — already feels complete for ISO/CUE/CHD users: platform, serial-gated DuckStation launch, BIOS readiness, DAT identity all work. The smallest completions: surface the verified serial in the library/Doctor (so a missing-serial launch refusal is explained *before* it happens), and decide `.ecm` (support or drop the claim).
- **PS2** — two one-liners away for the common case: the `.chd` identity arm (PS2 CHD is the dominant modern format and the reader is already proven on PS1) and the RomM outbound `ps2` row. With those, plus the existing serial-gated PCSX2 launch and BIOS verification, PS2 matches PS1.
- **PSP** — ISO users are complete today. The gap is everything that *isn't* an ISO: `.pbp` (parser exists, unwired) and `.cso` (registered, uninspectable). Wiring those two is the entire PSP backlog.
- **PS3** — the identity and launch spine (TITLE_ID + firmware status + RPCS3 planner/execution) is done; what's missing is the *front door*: `.pkg` files — the strong extension's own format — never reach discovery. One registry row + one identity arm closes it. License (RAP) material is a real-world RPCS3 requirement that is honestly unmodeled; until it is, PKG-based setups will fail outside EmuWiz's control and Doctor should say so plainly.

The pattern across all four: **the hard backend (identity spine, fail-closed planners, firmware, execution) is done and tested; what remains is a handful of registry rows, one match-arm entry, and persistence of facts the system already computes.**
