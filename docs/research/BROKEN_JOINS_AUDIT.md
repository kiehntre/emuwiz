# Broken Joins Audit — capabilities that exist but never reach the user

**Repo:** `/home/davedap/archivefs` · **Branch:** `feature/archivefs-unified-platform`
**Scope:** whole-repository, read-only research audit. Every claim below was verified against source on this branch. This audit deliberately hunts **broken joins** — two mature pieces with a missing connection — over new features.

---

## 1. Method and classification

For every capability found, the trace applied is:

```
parser/evidence → production registration → scanner/discovery → persisted catalogue
→ DAT identity → GUI → readiness → launch → downstream consumers (cheats, RomM, ES-DE, …)
```

| Label | Meaning |
|---|---|
| **CONNECTED** | Reachable from the normal production path end-to-end (possibly with documented honest refusals). |
| **PARTIALLY WIRED** | Some production consumers exist; at least one downstream link is missing. |
| **ORPHANED** | The capability exists, is tested, and has **zero production callers** — reachable only from examples/tests. |
| **DUPLICATED** | Two implementations compete for the same fact (identity, mapping, or platform list). |
| **DEAD/LEGACY** | Superseded code still present. (Almost none found — this codebase removes its dead.) |
| **INTENTIONALLY INTERNAL** | Deliberately example/test-only or diagnostics-only by documented design. |
| **MISSING** | No implementation exists. |

Production entry points used as ground truth for "reachable":

- **Scanner/persistence:** `archive_kind`/`media_registry.rs` → `ArchiveScanner` → `database::scan_and_persist` (plus the ingestion second pass, persisted as discovery-detail rows).
- **Identity:** `game_identity::inspect_game_identity` → persisted via `identity_report_json` (migration `0006_game_identity_reports.sql`).
- **DAT:** GUI-managed `run_dat_audit`/`run_combined_dat_audit` (`dat_sources_page.rs`) over `dat/sources/audit_run.rs`.
- **Launch:** GUI `launch_readiness_page` (the only GUI consumer of `launch::planning::build_launch_plan`).
- **Doctor:** `diagnostics/` (database, environment, profiles, shared-apply history).

Structural note verified up front: the working tree is clean and there are only 7 migrations, none DAT-verdict-related — so every "durable DAT identity" gap below is real in this tree, and no in-flight persistence work exists to account for.

---

## 2. Capability matrix

Legend: **M** MATURE · **P** PARTIAL · **O** ORPHANED · **–** MISSING · **n/a**. ✦ = non-obvious cell is evidenced in §3–§7.

Columns: **Med** media registration · **Ev** structural evidence · **DAT** exact DAT/hash identity · **ID** stable platform/game ID · **Pers** persistence · **Fw** firmware · **Emu** emulator discovery · **Rdy** readiness · **Plan** planning · **Exec** execution · **Doc** Doctor · **GUI** normal GUI · **DATg** DAT GUI · **Ch** cheats · **Mo** mods · **Ren** rename · **Dup** duplicates · **1G** 1G1R · **PL** Playing Library · **RomM** · **ESDE**.

### Fully / strongly supported platforms

| Platform | Med | Ev | DAT | ID | Pers | Fw | Emu | Rdy | Plan | Exec | Doc | GUI | DATg | Ch | Mo | Ren | Dup | 1G | PL | RomM | ESDE |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| NES | M | M | M | M | M | n/a | M (RA) | M | M | M | – | P ✦ | M | M | – | M | M | M | M | M | M |
| SNES | M | M | M | M | M | n/a | M (RA) | M | M | M | – | P ✦ | M | M | – | M | M | M | M | M | M |
| Game Boy / Color | M | M | M | M | M | n/a | M (RA) | M | M | M | – | P | M | M | – | M | M | M | M | M | M |
| Game Boy Advance | M | M | M | M | M | n/a | M (RA) | M | M | M | – | P | M | M | – | M | M | M | M | M | M |
| N64 | M | M | M | M | M | n/a | M (RA) | M | M | M | – | P | M | P | – | M | M | M | M | M | M |
| Mega Drive | M | M | M | M | M | n/a | M (RA) | M | M | M | – | P | M | M | – | M | M | M | M | M | M |
| Master System / Game Gear | M | M | M | P ✦ | M | n/a | M (RA) | M | M | M | – | P | M | M | – | M | M | M | M | M | M |
| **Dreamcast** | M | M | M | M | M | **M** (pinned BIOS) | M (Flycast) | M | M | M | – | **M** (GUI launch) | M | P | – | M | M | M | M | M | M |
| **PS2** | M | M | M | M | M | **M** (hash BIOS) | M (PCSX2) | M | M | M | – | **M** (GUI launch) | M | M | P (.pnach) | M | M | M | M | **–** ✦ | M |
| **PSX** | M | M | M | M | M | **M** | M (DuckStation) | M | M | M | – | P ✦ | M | M | P | M | M | M | M | M (`ps`) | M |
| **PSP** | M (CSO✦) | M | M | M | M | n/a (honest) | M (PPSSPP) | M | M | M | – | P ✦ | M | M | P | M | M | M | M | M | M |
| **PS3** | P (pkg✦) | M | M | M | M | P (presence) | M (RPCS3) | M | M | M | – | P ✦ | M | – | P (map✦) | M | M | M | M | M | M |
| GameCube / Wii | M | M | M | M | M | M (Dolphin) | M | M | M | M | – | M (GUI launch) | M | M | M (textures) | M | M | M | M | M | M |
| Xbox | M | M | M | M | M | M (xemu) | M | M | M | M | – | P ✦ | M | P | – | M | M | M | M | M | M |
| Xbox 360 | P (iso✦) | M | M | M | M | n/a | M (Xenia) | M | M | M | – | P ✦ | M | P | – | M | M | M | M | M | M |
| **ScummVM** | M | M | M | M | M | n/a | M | M | **O** ✦ | **O** ✦ | – | P | M | – | – | M | M | M | M | – | M |
| **Arcade** | M | n/a (design) | M | M | P ✦ | n/a | P (RA) | P | **–** ✦ | – | – | P | M | P | – | M | M | M | M | **–** | **–** |
| Neo Geo | M | n/a (design) | M | M | P | P (DAT) | P | P | – | – | – | P | M | P | – | M | M | M | M | – | – |
| Saturn | M | M | M | M | M | – | – | – | **–** | – | – | P | M | – | – | M | M | M | M | M | M |
| Sega CD | M | M | M | M | M | – | P (RA hint) | P | – | – | – | P | M | – | – | M | M | M | M | M | M |

### Partial / orphaned-evidence platforms

| Platform | Med | Ev | DAT | ID | Pers | Notes |
|---|---|---|---|---|---|---|
| Sega 32X | **–** ✦ | O ✦ | – | – | M | `.32x` strong extension unregistered in both registries; detector reachable only via example-only member evidence |
| Neo Geo CD | M (iso/chd) | O ✦ | M (disk SHA-1) | – | M | IPL.TXT parser example-only; no `game_identity` dispatch (CHD → `Deferred`) |
| Neo Geo Pocket / Color | **–** ✦ | O ✦ | – | – | M | Verified header parser with zero production callers; `.ngp/.ngc` unregistered |
| 3DO | M | M | M | M | M | Identity exists; no launch path (no row/adapter) |
| PC-FX | M | M | M | M | M | Same shape as 3DO |
| Atari 7800 / Lynx | M | M | P | P | M | Header normalization + detectors; Lynx real-validated |
| Atari 2600 / 5200 | M | – | M | – | M | DAT-only identity |
| Amiga | M | M | M | M | M | **Execution orphaned** ✦ (row + discovery + projection exist; no command/execution adapter) |
| Atari ST | M | M (st/stx structural) | M | M | M | **Execution orphaned** ✦ (same shape as Amiga) |
| Acorn BBC / Electron / Archimedes | **–** ✦ | – | M | – | M | Strong extensions (`ssd/dsd/jfd`) unregistered; zero parsers; folder-alias-only |
| ZX / C64 / Amstrad / tape families | M (tap/tzx/cdt) | – | M | – | M | Generic media + DAT only |
| WonderSwan / similar | varies | O / P | – | – | M | Detectors synthetic-validated, unwired |
| PC Engine CD / Jaguar | M | Deferred (documented) | M | – | M | Intentionally deferred with documented reasons |
| Extension-stub rows (PS Vita, N-Gage, Switch, WiiU, MSX, PC-98, FM Towns, X68000, Apple II, Mac, DOS, Vectrex, Intellivision, Coleco…) | varies | – | – | – | M | Extension/alias placeholders; no parsers or launch paths (mostly honest placeholders) |

**Cross-cutting observation:** the dominant failure mode is **not** missing parsers. It is (a) platform-registry extensions that no scanner registry knows, (b) finished adapters whose last hop (GUI launch, Doctor, persistence, execution) is missing, and (c) rich verdicts that are computed and then thrown away.



---

## 3. The 20 best broken joins (ranked)

Ranking: user-visible benefit × existing code reused × low risk × architectural leverage. Every join is "connect A to B", not "write a parser".

1. **GUI launch for DuckStation / PPSSPP / RPCS3 / Xenia / Xemu.** All five have complete core adapters (profile discovery, readiness, command planning, execution — `duckstation_command/execution.rs`, `ppsspp_*`, `rpcs3_*`, `xenia_*`, `xemu_*` — with `LAUNCH_COMPATIBILITY` rows and `DiscoveredStandaloneProfile` variants in `launch/integration.rs`). The GUI launch panel (`launch_readiness_page.rs` + `main.rs:6245-6498`) passes contexts **only** for RetroArch, Dolphin, PCSX2, Flycast. A PS1/PSP/PS3/Xbox/360 user can never press Launch even though the entire chain behind the button exists.
2. **ScummVM execution adapter orphaned by a missing table row.** `scummvm_command.rs`/`scummvm_execution.rs` (+ `resolve_scummvm_native_launch_binding`) are complete, but `LAUNCH_COMPATIBILITY` (`launch/platform_map.rs:82-155`) has **no `ScummVM` row**, so `build_standalone_candidates` returns empty. One struct literal connects it.
3. **PS2/PSP CHD identity deferred despite a mature reader.** `game_identity.rs:778-788` whitelists CHD for `PlayStation | Saturn | Dreamcast | SegaCd` only; PS2/PSP fall to the `Deferred` arm ("no safe bounded reader") even though `open_chd_iso9660`/`chd_logical_media` is the reader PS1 itself uses and `inspect_ps2_iso`/`inspect_psp_iso` already accept a `LogicalMedia`. `coverage_inventory.rs` even real-validated a PSX specimen *through a CHD*.
4. **`psp_pbp_evidence` fully orphaned.** `psp_pbp_evidence.rs` (header parse, offset validation, PARAM.SFO extraction, DATA.PSAR prefix, evidence emission — with tests) has **zero production callers**. PSP `.pbp` is a *strong* extension on the PSP platform row. Join: add a `"pbp"` dispatch arm in `game_identity` for PSP.
5. **PS3 `.pkg` strong extension unregistered and undispatched.** `platform/mod.rs` declares `pkg` strong for PS3; `ps3_disc_evidence.rs` contains a bounded PKG observer (`looks_like_pkg`, `parse_pkg_header`); a real-corpus PKG specimen is recorded RealValidated (`coverage_inventory.rs`, PS3 entry). But `pkg` is absent from `media_registry::MEDIA_FORMATS` **and** `content_registry::CONTENT_FORMATS`, and `game_identity` has no `pkg` arm.
6. **Persisted identity facts never shown.** Migration 0006 persists the full `GameIdentityReport` (PS1 serial, PS2 serial + executable CRC, PSP DISC_ID, PS3 TITLE_ID, N64 canonical hash…) on every archive row. No GUI evidence/library view surfaces any of these ID facts (the only GUI match for a Sony verified-ID field is `rpcs3_page.rs`'s title-ID mapping display).
7. **Doctor is blind to facts the app already computed.** `diagnostics/` covers database/environment/profiles only. It never consumes hash-verified BIOS states (`DuckStationBiosState`, `Pcsx2BiosVerification`), RPCS3 firmware presence (`Rpcs3FirmwareStatus`), persisted identity reports, or DAT set verdicts. Both ends exist; no adapter between them.
8. **DAT set verdicts are ephemeral.** `run_dat_audit` produces `Vec<SetResolution>` with full dependency reports and discards them after display — no table, no link to library archive rows, no DAT-revision binding. Rename, 1G1R, RomM export, launch, and Doctor all lack durable DAT identity as a result; the DAT audit page is the only consumer.
9. **Archive member-content evidence is example-only.** `archive_member_content_evidence::member_detectors()` registers the NES/SNES/GB/GBA/MD/32X/SMS-GG/A7800/Lynx/NGP/header-normalization detectors for ZIP/7z members — but `observe_zip_member_content`/`classify_archive_content` are called only from `examples/cartridge_probe.rs`. The production scanner never runs member evidence.
10. **`disc_evidence_collector::collect_disc_boot_evidence` example-only.** The combined disc collector (PS1/PS2 SYSTEM.CNF, Dreamcast IP.BIN, Saturn/Sega CD/3DO/PC-FX boot sectors, PSP PARAM.SFO, **Neo Geo CD IPL.TXT**) is reachable only from `examples/disc_probe.rs`/`library_plan_probe.rs`; `game_identity` re-implements per-platform paths instead of consuming it, and Neo Geo CD has no dispatch at all.
11. **Neo Geo CD IPL.TXT parser orphaned at the identity layer.** `neogeocd_boot_evidence.rs` is complete and Strong-evidence-graded; there is no `IdentityPlatform::NeoGeoCd`, no dispatch arm, no fusion rule — a Neo Geo CD CHD reports `Deferred`.
12. **NGP/NGPC: verified parser, zero production callers.** `ngp_header_evidence.rs` (copyright/system-flag/title, tested) appears only in the example-only member-detector list; `gather_structural_evidence` (`selected_evidence_page.rs:90-118`) has no NGP arm; no fusion rules; `.ngp/.ngc` are in neither scanner registry — while `inspector.rs:116` *does* classify them as likely content (three-way registry drift).
13. **Registry drift: platform strong extensions no scanner knows.** `.32x`, `.68k`, `.gdi`, `.cdi` (Sega), `.ssd`, `.dsd`, `.uef`, `.adl`, `.jfd`, `.hfe` (Acorn/Archimedes), `.pkg` (PS3), `.neo` (NeoGeo) — declared in `PLATFORMS` strong/weak lists, absent from `media_registry::MEDIA_FORMATS`/`content_registry::CONTENT_FORMATS`, several also absent from `inspector::LIKELY_CONTENT_EXTENSIONS`. Loose files with these extensions are never catalogued and are watcher-blind.
14. **Amiga / Atari ST launch rows without execution adapters.** `LAUNCH_COMPATIBILITY` has `AtariST→hatari` and `Amiga→amiga_whdload`; `hatari_local.rs`/WHDLoad discovery and `project_hatari_launch_input`/`project_amiga_whdload_launch_input` exist — but there is **no** `hatari_command.rs`/`hatari_execution.rs`/amiga equivalent in `launch/`. The inverse of join #2: the planner ends where ScummVM's adapter begins.
15. **PS2 has no RomM mapping.** `romm_platform_mapping.rs` maps PSX (`ps`), PSP (`psp`), PS3 (`ps3`) — **no `PS2` row** — so the most launch-complete Sony platform fails RomM projection.
16. **Two ES-DE layers that don't share the mapping.** `launch/es_de_export.rs` + `es_de_publish.rs` own the reviewed `ES_DE_SYSTEM_MAP`; `library_views.rs` (frontend layout manifests) explicitly documents "no ES-DE system mapping exists yet" and fails closed. The reviewed table is not reused by the layout generator.
17. **Xenia/Xemu GUI launch absent** — same shape as #1; both also have `inspect_*_game` APIs with no per-game GUI display.
18. **`rebuild_to` parsed, never consumed.** RomVault's `rebuildto` is carried on every `DatGameEntry` and constructed as `None` everywhere; the transaction engine (`rom_organisation`, `rename_apply`) could drive split↔merged rebuild planning from it. No consumer exists.
19. **PCSX2 widescreen/60fps patch databases not integrated.** `.pnach` recombination exists (`patch_manager/adapter.rs:132`, `patches/<serial>_<crc>.pnach`) and per-emulator *settings* widescreen toggles exist, but community widescreen patch databases are not a cheat source; PS2 mods are limited to user-supplied `.pnach`.
20. **Flycast Naomi/Atomiswave variants unused.** `FlycastPlatform::{Naomi,Naomi2,Atomiswave}` exist (`flycast_local.rs:46-52`) for profile eligibility, but `input_projection.rs:345` hardcodes `FlycastPlatform::Dreamcast` — the adapter's multi-platform capability is unreachable (no arcade platform rows to project from).

---

## 4. Orphaned parsers

| Module | Format/platform | Evidence already produced | Missing link | Wiring size |
|---|---|---|---|---|
| `psp_pbp_evidence.rs` | PSP `.pbp` | PBP header, offset sanity, PARAM.SFO, DATA.PSAR prefix | `game_identity` `"pbp"` dispatch arm (PSP) | tiny |
| `neogeocd_boot_evidence.rs` | Neo Geo CD IPL.TXT | Strong `BootStructure`, fail-closed | `IdentityPlatform::NeoGeoCd` + iso/cue/chd dispatch + fusion rule | small |
| `ngp_header_evidence.rs` | NGP/NGPC | Strong copyright, mono/color flag, title, software ID | registry rows, `gather_structural_evidence` arm, fusion rules, `IdentityPlatform` variants | small |
| `ps3_disc_evidence.rs` PKG observer | PS3 `.pkg` | Bounded header facts, `PS3_GAME` location | `pkg` registration + identity arm | small |
| `sega32x_header_evidence.rs` | 32X | Weak (honest) console-name leg | `.32x` registration + `IdentityPlatform::Sega32X` | small |
| `archive_member_content_evidence.rs` | all cartridge ZIP/7z members | 12 member detectors | production caller in the scan path | medium |
| `disc_evidence_collector.rs` | combined disc evidence (incl. Neo Geo CD) | per-platform boot evidence via `LogicalMedia` | production consumer | medium |
| `sms_gg_header_evidence` (member lane) | SMS/GG ZIP members | TMR SEGA facts | production member-evidence caller | shares above |
| `header_normalization` detectors | NES/FDS/Lynx/A7800/SNES copier headers | header-strip evidence | production member-evidence caller | shares above |
| `disk_format` (structural layer) | Acorn DFS/ADFS, `.ssd/.dsd/.adl/.uef/.jfd` | `NoAdapter` today — `PLATFORMS` rows exist with zero adapters | new adapters + registration | larger (parsers) |

Pattern: every orphaned parser is *already tested*, and most need only a dispatch arm and/or a registry row. None requires redesign.


---

## 5. Orphaned / partial emulator support

| Emulator | Detected | Doctor | Readiness | Planning | Execution | GUI launch | Missing adjacent link |
|---|---|---|---|---|---|---|---|
| DuckStation | ✅ (`duckstation_local`) | ❌ | ✅ (`DuckStationBiosState` → `FirmwareReadiness`) | ✅ | ✅ | ❌ | GUI launch context (join #1); Doctor BIOS consumption (join #7) |
| PCSX2 | ✅ | ❌ | ✅ (hash-verified BIOS) | ✅ | ✅ | ✅ | complete for Sony; RomM row missing (join #15) |
| PPSSPP | ✅ | ❌ | ✅ (`NotRequired`, honest) | ✅ | ✅ | ❌ | GUI launch context (join #1) |
| RPCS3 | ✅ (`rpcs3_page`) | ❌ | ✅ (`Rpcs3FirmwareStatus`, presence-only) | ✅ | ✅ | ❌ | GUI launch context (join #1); patch *apply* is map-only by design |
| Xenia | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ | GUI launch context (joins #1/#17) |
| xemu | ✅ | ❌ | ✅ (4-way system-file health) | ✅ | ✅ | ❌ | GUI launch context (joins #1/#17) |
| Flycast | ✅ | ❌ | ✅ (pinned-hash BIOS) | ✅ | ✅ | ✅ | Naomi/Atomiswave projection (join #20) |
| Dolphin | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | — |
| RetroArch | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | — |
| Hatari | ✅ | ❌ | ✅ (`HatariTosHealth`) | ✅ | ❌ no command/execution adapter | ❌ | `hatari_command/execution` (join #14) |
| Amiga WHDLoad | ✅ | ❌ | ✅ | ✅ | ❌ no command/execution adapter | ❌ | same (join #14) |
| ScummVM | ✅ | ❌ | ✅ | ❌ no row | ✅ | ❌ | `ScummVM` row in `LAUNCH_COMPATIBILITY` (join #2) |

The **Doctor column is uniformly ❌ by design today** — the diagnostics module consumes none of the adapter health states. That is join #7.

---

## 6. Duplicated identity

The identity architecture is unusually clean: one shared extraction point (`game_identity::serial_from_boot_path` serves PS1 and PS2), one persisted `GameIdentityReport`, and adapter consumption through `patch_manager::emulator_request_bridge`'s `inspect_*_game_for_verified` functions, which take *already-verified* facts rather than re-deriving them (`duckstation_request`, `ppsspp_request`, `pcsx2`, `rpcs3` variants). Cheats/mods therefore **do not independently rediscover IDs** — the notable exception is the GUI, which re-plumbs verified facts per page.

Duplications found:

1. **Two ES-DE mapping layers** — `launch/es_de_export.rs::ES_DE_SYSTEM_MAP` (reviewed) vs `library_views.rs`'s own frontend-platform vocabulary with no mapping (join #16). Recommendation: make `library_views` consume `es_de_system_for_platform`.
2. **GUI-side verified-fact re-plumbing** — `launch_readiness_page.rs` and `pcsx2_page.rs` each assemble PCSX2 preflight contexts (verified serial etc.) from overlapping state. A shared GUI-side "verified identity for selection" helper would remove the drift risk.
3. **`platform_artwork.rs` STATIC_TABLE / `romm_platform_mapping` / `es_de_export` / `romm_platform_mapping` aliases** — these are *mappings*, not duplicates (each keys the same canonical ids and is test-consistency-checked), but any new platform must be added to four tables; a consistency test tying them to `PLATFORMS` exists for coverage/romm only. Low risk, worth a parity test for es_de too.

No duplicated *parsing* was found: PS1 serial, PS2 serial/CRC, PSP DISC_ID, PS3 TITLE_ID, GameCube/Wii game ID, Dreamcast product code, Xbox title ID each have exactly one extractor, and Amiga/Atari-ST identity flows through their structural disk parsers.

---

## 7. DAT consumer gaps

DAT evidence today reaches: the DAT audit page (`DATg`), rename planning (`rename_plan` reads audit results), duplicate grouping (`SameDatRelease`/`SameGameDifferentDump` classes), and Playing Library election (caller-supplied verified candidates). It does **not** reach:

- **The normal Library** — archive rows carry `GameIdentityReport` (content identity), never DAT verdicts; there is no per-item "verified against DAT X rev Y" fact.
- **Persistence** — no `SetResolution`/dependency-outcome storage exists (joins #8); the audit is recomputed per run.
- **Problems & Repair** — `diagnostics/` and the repair pages have no DAT-aware findings; a set that is `Incomplete` because a parent archive is missing is invisible there.
- **Launch** — `build_launch_plan` consumes `GameIdentityReport` facts only; a DAT-verified arcade set and an unaudited one are indistinguishable at launch time.
- **RomM export** — `library_plan_export` carries set/support context but no DAT provenance.
- **Stale provenance** — managed DAT snapshots keep current/previous revisions (`dat/updates.rs`), but audit results are never bound to the revision that produced them, so a DAT update cannot invalidate prior verdicts.

Accounting for current work: the tree contains **no** library-DAT-summary, persisted-arcade-set-verdict, or per-item DAT persistence code (only 7 migrations; none DAT-related). Join #8 is therefore fully open.

---

## 8. GUI–backend gaps (facts that exist but users cannot see)

Prioritised; each is verified against the GUI sources:

1. **Verified Sony-style identity facts** — PS1 serial, PS2 serial + executable CRC, PSP DISC_ID, PS3 TITLE_ID are computed, persisted (migration 0006), consumed by cheats and launch internally, and displayed **nowhere** in the library/evidence views.
2. **Firmware/BIOS state** — `DuckStationBiosState::Verified`, `Pcsx2BiosVerification`, `Rpcs3FirmwareStatus::Present(version)`, `FlycastSystemFileState::Verified` are computed with real hashes; shown only inside scattered emulator pages, and the launch panel shows them solely for PCSX2.
3. **DAT set verdicts / dependency completeness** — `Complete`/`Incomplete`/`NeedsReview` + "which parent/BIOS/device is missing" exist per audit run and never leave the DAT page; library rows say nothing.
4. **Emulator readiness for the five unwired adapters** — profile discovery + firmware health exist for DuckStation/PPSSPP/RPCS3/Xenia/Xemu with no launch-panel presence.
5. **Stale provenance** — managed-DAT current/previous revisions exist; no UI ties a verdict or a library row to "verified against DAT rev X".
6. **Multi-disc relationships** — generic `(Disc N of M)` grouping and m3u/cue anchoring exist in `playing_library`/`library_grouping`; the normal library view does not present disc-set membership.
7. **Cheat/mod compatibility** — cheat coverage reports (`cheat_coverage.rs`) exist per provider; not surfaced on the game row.
8. **N64 canonical (byte-order-normalized) hash** — computed and persisted as distinct evidence; not displayed.

---

## 9. Leave-alone list (proven mature — do not rewrite)

- **`PLATFORMS` registry + alias/equivalence/conflict machinery** (`platform/mod.rs`) — reviewed MagicConfidence discipline, exact-component alias matching, drift tests.
- **DAT parse → index → audit → set → dependency stack** (`dat/`): Logiqx/ClrMamePro/listxml parsers, positional member attribution, `SetState`/`MemberClass` semantics, the eight-kind fail-closed dependency resolver with downgrade-only combine, CHD v5 header identity + disk audit. Extend, never restructure.
- **Optical stack**: `iso9660.rs`, `ingestion/cue_bin.rs`, `raw_cd_logical_media`/`raw_cd_sector`, `chd_logical_media` + optional specialist, `optical_fingerprint` + `repair/optical_*` conversion with provenance.
- **`game_identity`** dispatch and per-platform extractors (PS1/PS2 serial, PSP UMD, PS3 layout, Dolphin, XBE/XEX, IP.BIN, Saturn/Sega CD/3DO/PC-FX boot structures) — mature, real-corpus-validated, fail-closed.
- **Emulator adapters**: `duckstation_*`, `pcsx2_*`, `ppsspp_*`, `rpcs3_*`, `xenia_*`, `xemu_*`, `flycast_*`, `dolphin_*`, `retroarch_*`, `scummvm_*` — command/execution split, strict blockers, honest readiness vocabulary.
- **Firmware verification primitives** (`dat/firmware_evidence.rs` + `duckstation_firmware`/`pcsx2_firmware`) — two-source hash discipline; no filename verification anywhere.
- **`patch_manager::emulator_request_bridge`** — the verified-identity→adapter-request seam; keep it the only join between scanner identity and cheat/mod adapters.
- **`platform_evidence_fusion`** (rules + lineage + duplicate taxonomy), **`playing_library`** (election + transactional symlink apply), **`es_de_publish`** (byte-preserving gamelist writer).
- **Transaction engine** (`rename_apply`, `rom_organisation::transaction`) — journal, checkpoint, no-clobber moves, rollback.
- **`media_registry`/`content_registry` single-source-of-truth pattern** — the fix for registry drift is registering extensions there, not bypassing them.

---

## 10. Roadmap impact

### P0 — broken joins worth doing before any new platform expansion
1. GUI launch contexts for DuckStation/PPSSPP/RPCS3/Xenia/Xemu (join #1) + the `ScummVM` row (#2).
2. Persist DAT audit verdicts + per-item binding to library rows and DAT revisions (#8).
3. PS2/PSP CHD identity dispatch (#3).
4. Register every orphaned strong/weak extension in `media_registry`/`content_registry` (+ inspector parity test) (#13).
5. Doctor adapters over existing facts: identity reports, firmware states, set verdicts (#7).

### P1 — small completeness gaps
6. Wire `psp_pbp_evidence` (#4) and the PS3 `pkg` observer (#5).
7. RomM row for PS2 (#15); ES-DE mapping reuse in `library_views` (#16).
8. Hatari / Amiga-WHDLoad command+execution adapters over the existing projection (#14).
9. Production member-content evidence pass in the scanner (#9) + Neo Geo CD / NGP identity wiring (#10–#12).
10. GUI surfacing of verified ID facts and firmware state (#6, §8).

### P2 — genuinely new parsers/features
11. Acorn DFS/ADFS/UEF adapters; `.cso` reader; ECM/CCD/MDS (registered-but-unsupported honesty is acceptable meanwhile).
12. Arcade launch adapter + MAME version compatibility.
13. `rebuild_to`-aware repack planning over the transaction engine (#18).
14. 1G1R arcade dimensions (`runnable`, bootleg/hack classification).
15. Flycast Naomi/Atomiswave platform projection (#20).

### The 10 integration tasks that would make EmuWiz feel most complete without adding a single new format

1. Launch button for PS1 (DuckStation), PSP (PPSSPP), PS3 (RPCS3), Xbox (xemu), Xbox 360 (Xenia).
2. ScummVM launch row (one struct literal; the adapter already exists).
3. PS2/PSP CHD identity (removes the most common honest `Deferred` Sony users see).
4. Durable DAT verdicts on library rows ("Verified · <release> · <DAT> rev <n>") — the single biggest perceived-quality change.
5. Registry parity sweep: catalogue every extension the platform registry already claims.
6. Show serial/title-ID/DISC_ID + DAT verification on the game row and evidence page.
7. Doctor findings from existing state: missing BIOS (named), RPCS3 firmware missing, identity conflicts, set incompleteness with the named dependency.
8. PBP identity so PSP digital titles stop being dead rows.
9. Hatari + Amiga launch adapters (finishing the computer families the rows already promise).
10. ES-DE/RomM mapping completion (PS2 RomM row; `library_views` reusing `ES_DE_SYSTEM_MAP`) so exported libraries match what EmuWiz itself resolved.

---

## 11. Conclusion

EmuWiz's pattern across every family audited is consistent, and it is a *good* problem to have: the evidence and adapter layers are consistently ahead of the wiring layer. Of the twenty highest-value joins found, **eighteen are pure connections between existing, tested components** and none requires a new parser. The rare gaps that do require new code (Hatari/Amiga command adapters, Acorn disk adapters, a CSO reader) are small, templated by five existing adapters, and deliberately deferred to P1/P2 — because until joins #1/#2/#3/#8 land, users cannot see the sophistication that already exists.

