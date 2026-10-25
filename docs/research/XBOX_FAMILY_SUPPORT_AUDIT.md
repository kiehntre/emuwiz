# Xbox Family Support Audit — EmuWiz (RESEARCH ONLY)

> **Research snapshot** — This audit records repository findings at the time it was written. It is not current capability documentation; see the [README](../../README.md), [adapter support matrix](../ADAPTER_SUPPORT_MATRIX.md), and [roadmap](../../ROADMAP.md) for present guidance.

**Scope:** Original Xbox · Xbox 360 — xemu · Xenia/Xenia Canary
**Branch:** `feature/archivefs-unified-platform`
**Method:** static source analysis only — no source modified, no commits.
**Companion:** `docs/research/BROKEN_JOINS_AUDIT.md` (its Xbox rows were re-verified against source for §L; the GUI-launch finding is quoted, not re-derived).

---

## A. PLATFORM MODEL

`platform/mod.rs`:

| | Xbox (`:902-913`) | Xbox360 (`:915-926`) |
|---|---|---|
| display | Microsoft Xbox | Microsoft Xbox 360 |
| folder_aliases | `xbox`, `xboxoriginal`, `microsoftxbox` | `xbox360`, `x360`, `microsoftxbox360` |
| strong ext | `xbe`, `xiso` | `xex` |
| weak ext | `iso`, `zip` | `iso`, `zip`, **`god`** |
| magic | none (XDVDFS deliberately *not* a platform magic — shared with 360) | none (same reason) |
| conflicts | Xbox360 | Xbox |

- `IdentityPlatform::Xbox` and `IdentityPlatform::Xbox360` exist (`game_identity.rs`); `IdentityKind::XbeTitleId` ("Xbox Title ID"), `XexTitleId` and `XexMediaId` ("Xbox 360 Title ID"/"Xbox 360 Media ID") are distinct fact types per generation (`:211-213, 254-256`) — **exactly the separation the task asks about**.
- Launch rows: both present in `LAUNCH_COMPATIBILITY` (`launch/platform_map.rs:108-119`).
- ES-DE: `xbox`/`xbox360` rows with reviewed fullnames (`launch/es_de_export.rs:133-142`).
- RomM outbound: Xbox→`xbox` (`romm_platform_mapping.rs:208`), Xbox360→`xbox360` (`:215`) — both Mapped with RomM 5.0 provenance.
- coverage_inventory: both rows `RealValidated` — Xbox: *Fable – The Lost Chapters (USA).iso* via fusion rule `xbox_original_disc` (`:215-225`); Xbox360: *Fable II* disc + *Double Dragon Neon* STFS package (`:228-241`).
- **Drift:** none between subsystems. The one shared-fact hazard (XDVDFS serves both generations) is explicitly modeled as family-level evidence, never platform proof (`xbox_boot_evidence.rs:43-47`).

## B. ORIGINAL XBOX CONTENT FORMATS

**XDVDFS structural parsing exists and is production-wired** — this is the standout of the family:

- `xdvdfs_signature.rs`: `XDVDFS_VOLUME_HEADER_MAGIC` ("MICROSOFT*XBOX*MEDIA") at `XDVDFS_VOLUME_DESCRIPTOR_OFFSET` (logical sector 32), `looks_like_xdvdfs`.
- `xdvdfs_traversal.rs`: a real bounded XDVDFS filesystem walker — `find_path` / `read_file_prefix` over an in-memory image and **`find_path_in_disc_image`** (streaming, used by identity).
- `xbox_boot_evidence.rs`: `observe_xbox_disc:96` — XDVDFS sig → `/default.xbe` lookup → bounded 8 KiB prefix (`XBE_PREFIX_READ_BYTES:90`) → `parse_xbe_header` + certificate (`XBE_CERTIFICATE_READ_BYTES` 512 B). Evidence: XDVDFS `Strong` `Filesystem` ("shared with Xbox 360, never platform proof on its own"), `default.xbe` `Corroborated` `BootStructure` ("a naming convention, not a format signature"), `XBEH` `Strong` `ContentSignature`, title-ID `Corroborated` `ProductCode`.
- Identity: `"iso" | "xiso" if platform == IdentityPlatform::Xbox => inspect_direct_xbox_disc` (`game_identity.rs:765-767`), which uses `find_path_in_disc_image` (`:2002`) — **an `.iso` is only claimed Xbox after the XDVDFS magic + default.xbe are found in the actual bytes**. Extension never proves Xbox. Real corpus: Fable ISO resolved through fusion.

**Format verdicts:**
- XISO/redump-style images: **structurally parsed** (XDVDFS + XBE, bounded).
- Extracted game folders / loose `default.xbe`: `inspect_direct_xbe` exists (`:1852`), but see §C/§V — loose `.xbe` never reaches it (registry gap).
- CCI: absent (no reference). CSO: not Xbox-relevant in-repo. Compressed XISO (e.g. CXI/extract-xiso compressed): absent. HDD/game-directory layouts (partition 0/2): **absent** — and `patch_manager/xemu_local.rs:253` states "HDD images are intentionally not mounted or parsed in this batch".

**Generic-ISO vs XISO discrimination:** solved by structure (XDVDFS magic at sector 32 + default.xbe traversal), not extension — exactly what the task demands. XDVDFS is also checked for 360 (`xbox360_boot_evidence.rs`) with the same "shared, never proof" honesty; the generation split comes from `default.xbe` vs `default.xex` *content* (XBEH vs XEX2 magic), not names.

## C. XBE IDENTITY

**Parser** (`executable_signatures.rs`): `XBEH` magic, `XbeHeaderFact`, `parse_xbe_header`, certificate chain walk (`xbe_certificate_file_offset`, `XBE_CERT_*_OFFSET` constants incl. title-name UTF-16 at 0xC, 40 units), bounded 512 B certificate read. `XbeDetector` emits `Strong` "XBEH" and a title-ID `ProductCode` (candidate).

**Extracted:** magic, header/certificate structure, **Title ID**, **title name** (certificate), bounded. Region/media flags/version: not decoded (fields exist in the cert but are not surfaced — no consumer).

**The chain:**
```
XbeDetector / parse_xbe_header
  → IdentityKind::XbeTitleId (Verified via inspect_direct_xbe)
  → evidence_bridge (:85-87 projects Xbe/Xex kinds into launch identity)
  → xemu_command:158,194 — verified_xbox_title_id REQUIRED, fail-closed
  → patch_manager/xemu_local.rs (profile/system-file inspection)
```
- **DAT matching**: generic hash pipeline (the ISO itself is the hashable object; the XBE title ID is a corroborating fact, never release identity).
- **Cheats/mods**: no original-Xbox trainer/patch subsystem exists (`patch_manager` has no xemu patch document — only profile/system-file inspection). Nothing orphaned there because nothing exists there.
- **GUI**: identity facts visible via identity reports; serial-style catalogue persistence is the same gap as Sony (facts re-verified per launch, not stored).
- **Orphaned facts**: none for XBE — but the *entry path* is broken for loose `.xbe` files (see §V #1): `inspect_direct_xbe` exists, is correct, and cannot be reached from discovery.

**Three concepts kept separate:** platform (XDVDFS+XBE content legs), Title ID (`XbeTitleId`, candidate→Verified), release identity (hash/DAT). The title-ID "candidate" wording is carried in the evidence text itself (`xbox_boot_evidence.rs:69-73`).

## D. XBOX 360 CONTAINERS

| Format | State | Evidence |
|---|---|---|
| ISO (XGD) | **structurally parsed** — XDVDFS sig + `/default.xex` + XEX2 magic (`xbox360_boot_evidence.rs:16-30`, `observe_xbox360_evidence`) | Real Fable II disc |
| XEX/XEX2 | **parsed** (`XexDetector` — XEX2 magic + optional-header table; `inspect_direct_xex:1518`) | — |
| STFS (CON/LIVE/PIRS) | **parsed, metadata-only** (`xbox360_stfs_evidence.rs` — two-source verified against Free60 + py360) | Real Double Dragon Neon LIVE package |
| GOD packages | **parsed as STFS** (CON-signed envelope); content_type never interpreted into "is a game" (`:44-49` collision policy) | — |
| XBLA/DLC/saves | same STFS envelope; `content_type` exposed **raw, never interpreted** | — |
| Title updates | same STFS envelope; no TU-specific version model (see §N) | — |
| Extracted folders / `default.xex` loose | `inspect_direct_xex` exists; loose `.xex` unreachable (registry gap, §V #1) | — |
| SVOD/DVD9-stripped/other XGD variants | absent (no references) | — |

The STFS module is exemplary scope discipline: fixed metadata fields only (`:32-42` field table: magic, content_type, content_size, **media_id**, version, **base_version**, **title_id**, platform, executable_type, **disc_number/disc_in_set**, save_game_id, display_name/title_name UTF-16BE first locale) — "no signature is verified, no license is checked, no directory entry is walked, no file is extracted, and nothing is ever decrypted".

## E. XEX / XEX2 IDENTITY

**Parser:** `XexDetector` (XEX2 magic + optional-header table, `executable_signatures.rs`); `inspect_direct_xex` (`game_identity.rs:1518-1672`) extracts **`XexTitleId`** and **`XexMediaId`** (`:1661-1672`), both with Verified getters (`:609-616`).

**Consumers:**
- **Xenia launch**: `xenia_command.rs:115,158` — *at least one of* verified title ID / media ID is required, fail-closed; both are carried on the plan (`:69`).
- **Patches/mods**: `xenia_patch_document.rs` — Xenia patch files declare `title_id`, module `hash`es, and an optional **`media_ids` constraint list** (`:11, 173-177`, max 32/file) — i.e. the XEX Media ID fact is exactly what Xenia's own patch format keys on, and EmuWiz parses both ends. `xenia_install_plan.rs` models candidate compatibility and patch selection (`XeniaCandidateCompatibility`, `XeniaPatchSelection`).
- **GUI**: `cheats_mods_preview.rs` renders Xenia patch candidates.
- **TU/DLC**: base_version/version fields exist in STFS metadata but no TU-version relationship model (§N).

**Deeper XEX parsing?** No — title/media ID + format magic are the facts every consumer (Xenia planner, patch documents, GUI) actually uses. Execution-info/optional-header exploration would produce no new consumer-visible fact. **Leave it.**

## F. TITLE ID / MEDIA ID SPINE

- **Sources:** Original Xbox → XBE certificate (`XbeTitleId`); 360 → XEX2 optional headers (`XexTitleId`, `XexMediaId`) and STFS fixed metadata (`title_id`/`media_id`/`base_version`/`disc_number`/`disc_in_set` as observation fields).
- **Distinct fact types per generation:** yes — three kinds, distinctly labeled (`:254-256`).
- **Verified vs candidate:** XBE/STFS product codes are emitted as `Corroborated` candidates in *evidence*, then Verified facts in *identity inspection* (bounded reads of the real bytes); launch consumes only Verified getters (`verified_xbox_title_id:602`, `verified_xex_title_id:609`, `verified_xex_media_id:616`).
- **Persisted:** same shape as Sony — recomputed and re-verified at use; not stored as catalogue rows.
- **Launch consumes them:** yes — both planners fail closed (xemu requires the title ID; Xenia requires title-ID *or* media-ID).
- **Cheats/mods rediscovery:** no — Xenia patch selection keys on the same title/media facts; nothing re-derives them from filenames.
- **Duplication:** none found. One XBE parser, one XEX parser, one STFS parser; the STFS module and XEX module read disjoint containers.

## G. DAT / PRESERVATION

| | Original Xbox | Xbox 360 |
|---|---|---|
| Primary ecosystem | Redump (disc) | Redump (disc) + No-Intro (XBLA/DLC as STFS) |
| EmuWiz support | generic `dat/audit` + redump source infra | same |
| Hash types | whole-image hashes; XDVDFS/XBE facts as provenance | whole-image hashes; STFS title/media facts as provenance |
| Source/revision provenance | TOSEC-style naming via generic machinery | STFS `version`/`base_version` fields available as facts (not yet joined to DAT revisions) |
| Stale handling | `identity_source/stale.rs` (generic) | same |
| Multi-disc | generic DAT sets; STFS `disc_number`/`disc_in_set` fields exist as facts but are not joined to grouping | same |

**No Xbox-specific DAT infrastructure exists or is warranted** — the facts the formats carry (title/media/disc numbers) are observation-level and the hash spine is generic.

## H. ORIGINAL XBOX FIRMWARE (xemu)

`patch_manager/xemu_local.rs` models **four system files** with explicit states (`:172-175, 234-237`): **MCPX boot ROM, flash BIOS, EEPROM, HDD image** — `XemuSystemFileState` per file, discovered from bounded profile inspection ("never launches xemu, follows symlinks, opens an HDD/DVD image" — `:4`); "HDD images are intentionally not mounted or parsed in this batch" (`:253`).
- Readiness: `xemu_firmware_readiness(XemuSystemFileState)` (`launch/readiness.rs:359`) + `XemuHealth`/`XemuLaunchBlocker` consumed by the planner (`xemu_command.rs:161`).
- **Hash manifests:** `FirmwareSystem::Xbox` exists in `dat/firmware_evidence.rs` with a careful DAT-name rule — "requires 'xbox' but *not* '360'" (`:159,184`) — and its own doc notes a caller matching it; the no-MCPX/EEPROM-variant test (`firmware_evidence/tests.rs:255`) pins that Redump publishes hashes for exactly the *flash BIOS* class, and MCPX/EEPROM hashes are not claimed. **No copyrighted firmware is bundled or hash-faked.**
- Doctor/GUI: xemu appears in doctor/profile inspections and GUI tests (`doctor_and_repair.rs`, `emulator_profiles_and_setup.rs`); the four-way system-file health is computed but — per the broken-joins audit (`:151`) — **not surfaced in the GUI launch panel**.

## I. XBOX 360 / XENIA

- **Xenia requires no firmware/system files** — `xenia_firmware_readiness()` is a constant (`launch/readiness.rs:412`), correctly refusing to fabricate a BIOS gate.
- Content: ISO/XDVDFS, XEX2, STFS — all covered in §D/E.
- Config/profile: `patch_manager/xenia_local.rs` (bounded inspection); install planning `xenia_install_plan.rs` (`build_xenia_candidates:170`, `load_xenia_destination:331`).
- Canary-vs-stable: **not modeled** as a distinction anywhere (a single Xenia adapter; no channel field). Native vs Wine/Proton on Linux: not modeled. Vulkan/D3D requirements: not modeled. All are honest absences — no fake readiness items.
- GUI: `cheats_mods_preview.rs` + doctor rows; launch panel gap as §L.

## J. EMULATOR DISCOVERY / READINESS

| | xemu | Xenia |
|---|---|---|
| Detection | ✅ (`xemu_local` profile discovery, `diagnostics/profiles.rs::XemuProfileDiscovery`) | ✅ (`xenia_local`) |
| Readiness | ✅ four-file system health (`readiness.rs:359`) | ✅ constant (correctly nothing needed) |
| Planning | ✅ `build_xemu_command_plan:156` — identity + platform + title-ID + binding + health gates | ✅ `build_xenia_command_plan:120` — identity + platform + title/media-ID gates |
| Execution | ✅ `launch/xemu_execution/` (tests included) | ✅ `launch/xenia_execution.rs` + tests |
| Doctor | ✅ (profile rows; system-file states computed) | ✅ (profile rows) |
| GUI Setup | ✅ profile pages/tests | ✅ |
| **GUI Launch** | **❌ — no launch context passed** (broken-joins #1/#17) | **❌ — same** |

Both chains are mature; the single missing link is the GUI launch context. Verified from `BROKEN_JOINS_AUDIT.md:99,150-151` against `launch_readiness_page.rs` + `main.rs:6245-6498` (contexts passed only for RetroArch, Dolphin, PCSX2, Flycast).

## K. LAUNCH

- **Original Xbox → xemu:** verified game (XDVDFS+XBE content legs) → platform `Xbox` → **`XbeTitleId` Verified (required)** → emulator binding (`XemuNativeLaunchBinding`) → system-file health → `XemuCommandPlan` (blockers enumerated: `IdentityUnresolved`, `IdentityConflict`, `XemuPlatformMismatch`, …) → execution. Accepted container: the verified ISO/XDVDFS image the candidate names. XBE-direct/XISO-named/GOD: not broadened — planners bind to the candidate content, and loose `.xbe` cannot even arrive (§V).
- **Xbox 360 → Xenia:** same shape; **title-ID or media-ID** satisfies the gate (`xenia_command.rs:158`); STFS/XBLA packages reach identity via the STFS observation path; GOD/STFS launch is not specially modeled beyond the package file itself.
- Neither planner accepts "theoretically openable" formats beyond what identity has verified — the discipline the task asks to preserve.

## L. GUI LAUNCH GAP — **confirmed, P0**

Re-verified per `BROKEN_JOINS_AUDIT.md:99`: *all five standalone adapters* (DuckStation, PPSSPP, RPCS3, **Xenia, Xemu**) have complete core chains — `LAUNCH_COMPATIBILITY` rows and `DiscoveredStandaloneProfile` variants in `launch/integration.rs` — but the GUI launch panel (`launch_readiness_page.rs`, context construction in `main.rs:6245-6498`) passes contexts **only for RetroArch, Dolphin, PCSX2, Flycast**. An Xbox/360 user cannot press Launch even though every backend piece behind the button exists and is tested. This is a GUI table/context join, not a backend gap → **P0**.

## M. CHEATS / PATCHES / MODS

- **Original Xbox:** no trainer/patch subsystem exists (nothing to orphan). `xemu_local.rs` is inspection-only.
- **Xbox 360:** the Xenia patch stack is real and identity-connected: `xenia_patch_document.rs` parses title-ID-keyed patch files with module hashes and **Media-ID constraint lists**; `xenia_install_plan.rs` selects compatible candidates per destination; `cheats_mods_preview.rs` renders them. Identity facts are **reused, never rediscovered** — the patch document's `title_id`/`media_ids` are checked against the same `XexTitleId`/`XexMediaId` facts the launch spine produced.
- No original-Xbox/xemu patch-format modeling (xemu has no community patch-file convention modeled here — honest absence).

## N. TITLE UPDATES / DLC

**What exists:** STFS fixed metadata carries `content_type` (raw, never interpreted — "XBLA games, DLC, saved games, avatar items… distinguished only by content_type, a plain numeric field this module exposes raw and **never** interprets", `xbox360_stfs_evidence.rs:44-49`), plus `version`/`base_version`/`disc_number`/`disc_in_set`.
**What does not:** base-game↔TU↔DLC relationship modeling, TU-version compatibility constraints, multiple-TU-version sets, installed-vs-source package distinction. The generic dependency/version machinery (`dat/dependency/clone_report.rs`, `dat/set.rs` MemberClass/loadflags) is content-shape-agnostic and could express relationships later, but no Xbox-specific join exists. Per the task: **not built here** — recorded as the design seam (`content_type` + `base_version` are the two fields a future TU/DLC model would key on).

## O. MULTI-DISC

- DAT grouping: generic (`MultiDiscSet`, `dat/set.rs`); TOSEC/Redump disc naming via generic machinery.
- **Xbox-specific fact left on the table:** STFS `disc_number`/`disc_in_set` are parsed but not joined to grouping — a natural, two-source-verified improvement for multi-disc 360 releases distributed as packages.
- Election/RomM/ES-DE projection: generic paths; the same Sony-shaped election risk (one representative elected per family, disc-count awareness untested) applies. **No multi-disc election test exists for Xbox-family content** — named in §T.

## P. ROMM

- Outbound rows verified: Xbox→`xbox`, Xbox360→`xbox360` (`romm_platform_mapping.rs:208-218`) — nothing missing.
- ISO/XISO: projected as files (generic). Extracted game directories and GOD/STFS folders: no reviewed projection convention (single-file RomM model) — honest limitation, not a broken join.
- Multi-disc: generic set grouping; no per-platform regression test.

## Q. ES-DE

- `xbox`/`xbox360` rows exist once each (no duplicate maps); folder naming matches ES-DE's own; command/backend expectations are the generic ES-DE export path; multi-file handling inherits the generic (untested-for-Xbox) behavior; launch-path validity is unaffected by the GUI launch gap (ES-DE launches its own emulators).

## R. DOCTOR

**Already reported** (verified): xemu profile inspection + four system-file states (mcpx/flash/eeprom/hdd each with `XemuSystemFileState`), Xenia profile rows, identity-unresolved/conflict launch blockers, platform-mismatch blockers, unsupported-container states via `Pcsx2GameIdentity`-style honest status vocabularies (Xenia equivalents in planner blockers).
**Not reported / distinguishable today:** per-file firmware-hash *verification* for MCPX/EEPROM (Redump publishes none — `firmware_evidence.rs:52-56` documents why); "identity verified but title-ID fact missing → launch will fail" pre-warning (the Sony-pattern persistence gap); TU/DLC compatibility findings (nothing modeled). Informational vs repairable is already separated by the blocker-kind vocabulary in the planners.

## S. SECURITY / FAIL-CLOSED

- **`.iso` ≠ Xbox:** enforced structurally — XDVDFS magic at sector 32 + default.xbe traversal + XBEH/XEX2 magic on the actual bytes; the row explanation says `.iso` "is shared with the Xbox 360 and every other disc system" and the platform claims **no ISO magic**.
- **`.xbe`/`default.xbe` filenames:** "a naming convention, not a format signature by itself" (`xbox_boot_evidence.rs:53-57`); identity reads the certificate from real bytes.
- **Title-ID-looking filenames:** never read; title IDs come from certificates/optional headers/STFS metadata only.
- **`.xex` without header:** the identity arm validates XEX2 magic before any fact; `default.xex` alone is a `Corroborated` filename fact, never proof.
- **STFS filename/`CON`-signed ≠ valid:** signatures/certificates are explicitly *not* verified (`xbox360_stfs_evidence.rs:38-49`); `content_type` never becomes "this is a game".
- **Shell execution:** none — plans are argv-shaped structs (`XemuCommandPlan`, `XeniaCommandPlan`) consumed by execution adapters.
- **Weak heuristics found: none.** This family has the cleanest fail-closed posture audited so far; the residual risks are *absences* (unregistered extensions), not overclaims.

## T. TEST COVERAGE

Present (verified): `xbox_boot_evidence` (XDVDFS sig, default.xbe, XBEH, synthetic XBE cert incl. title-id `0x4D5A0058`), `xbox360_boot_evidence` (shared-XDVDFS honesty), `xbox360_stfs_evidence` (two-source field table, CON/LIVE/PIRS, display-name decoding, collision policy), XEX/XBE identity arms (`game_identity.rs` tests incl. byte-order-adjacent fixtures), `xemu_execution/tests.rs`, `xenia_execution.rs` tests, `xenia_patch_document` (media-id constraints, blocking warnings), `xenia_install_plan` (candidate compatibility/confirmation), GUI doctor/profile tests, coverage real-corpus rows (Fable ISO, Fable II, Double Dragon Neon LIVE).
**Untested / missing tests:** loose `.xbe`/`.xex` end-to-end (no path exists — see §V #1); multi-disc election for Xbox-family sets; STFS `disc_number` join to grouping (no test because no join); TU/base-version relationships (unmodeled); xemu system-file-hash verification (deliberately absent — Redump publishes no MCPX/EEPROM hashes).

## U. MATURITY MATRIX

| | Xbox | Xbox 360 |
|---|---|---|
| Platform registry | MATURE | MATURE |
| Media registration | **PARTIAL** — `.xbe`/`.xiso` strong exts are in no scanner registry; only `.iso` flows | **PARTIAL** — `.xex`/`.god` unregistered; only generic `.iso` flows |
| Structural evidence | MATURE (XDVDFS + XBE, bounded, real-corpus) | MATURE (XDVDFS + XEX2 + STFS, two-source) |
| Stable Title ID | MATURE (`XbeTitleId`, launch-gated) | MATURE (`XexTitleId`, planner-gated) |
| Media ID | N/A (format has no equivalent) | MATURE (`XexMediaId` + STFS field; consumed by Xenia patch docs) |
| Exact DAT/hash identity | MATURE (generic) | MATURE (generic) |
| Persistence | PARTIAL — re-verified per launch, not stored (Sony-pattern gap) | PARTIAL — same |
| Firmware/system files | PARTIAL — four states discovered/ready; hash verification impossible for MCPX/EEPROM (no published hashes) | N/A — Xenia needs none (constant readiness, correctly) |
| Emulator discovery | MATURE | MATURE |
| Readiness | MATURE | MATURE |
| Planning | MATURE (fail-closed title ID) | MATURE (fail-closed title/media ID) |
| Execution | MATURE | MATURE |
| GUI launch | **ORPHANED** — full backend chain, zero GUI launch context (broken-joins #1/#17) | **ORPHANED** — same |
| Doctor | PARTIAL — profile/system-file rows exist; no pre-launch identity pre-warning | PARTIAL — same shape |
| Cheats | MISSING (no Xbox trainer subsystem; honest absence) | PARTIAL — Xenia patch parse/select exists; no full cheat-provider pipeline |
| Mods | MISSING (honest) | PARTIAL — patch documents with media-ID constraints rendered in GUI |
| Title Updates | MISSING (fields parsed, no relationship model — recorded seam) | MISSING (same) |
| DLC | MISSING (content_type deliberately uninterpreted) | MISSING (same) |
| Rename | MATURE (hash-authoritative) | MATURE |
| Duplicates | MATURE (generic) | MATURE |
| 1G1R | MATURE (generic) | MATURE (generic) |
| Playing Library | MATURE (generic; election risk untested) | MATURE (generic; election risk untested) |
| RomM | MATURE (`xbox`) | MATURE (`xbox360`) |
| ES-DE | MATURE | MATURE |
| Multi-disc | PARTIAL — generic grouping only; no disc-number-aware election test | PARTIAL — `disc_number`/`disc_in_set` parsed but unjoined |

## V. BROKEN JOINS (both ends exist, disconnected)

1. **Loose `.xbe`/`.xex` identity unreachable** — `inspect_direct_xbe` (`game_identity.rs:1852`) and `inspect_direct_xex` (`:1518`) are complete, tested, and platform-armed, but `xbe`/`xex` appear in **no** scanner registry (`media_registry`/`content_registry`: zero rows) — extracted-game users can never reach the inspectors that exist for them. (Disc images work; executables don't.)
2. **GUI launch for xemu/Xenia** — complete adapters + `LAUNCH_COMPATIBILITY` rows vs a launch panel that passes contexts for only four other emulators (`main.rs:6245-6498`). The family's single biggest user-visible gap.
3. **STFS disc-number facts → grouping** — `disc_number`/`disc_in_set` parsed (two-source verified) but never consumed by `MultiDiscSet`/election; a multi-disc 360 package set can elect one representative silently.
4. **`XbeTitleId`/`XexTitleId`/`XexMediaId` → catalogue persistence** — verified at identity time, consumed at launch, never stored; Doctor/library cannot pre-warn "launch will fail closed without it" (same Sony-pattern gap, same fix shape).
5. **STFS `base_version`/`version` → revision provenance** — fields parsed, DAT revision naming generic-only; no join.
6. **xemu four-file health → GUI launch panel** — health computed and planner-consumed, but not shown where the user decides to launch (subset of #2 but worth naming: it is the *readiness* half of the same join).
7. **Xenia patch documents ↔ selected candidate display** — parsed/selected backend-side, rendered only in the separate cheats/mods preview, not in the launch panel where a user would notice an incompatible patch before launching.

## W. ORPHANED PARSERS

- **`inspect_direct_xbe`** (`game_identity.rs:1852`) + `XbeDetector`/`parse_xbe_header` — complete, tested; **missing seam:** `xbe` row in `media_registry.rs`/`content_registry.rs` (the `inspect_game_identity` dispatch arm already exists).
- **`inspect_direct_xex`** (`game_identity.rs:1518`) + `XexDetector` — same shape; **missing seam:** `xex` registry row (the `"xex" if platform == Xbox360` arm already exists at `game_identity.rs:761`).
- **`.xiso`/`.god` extensions** — claimed strong/weak on platform rows; no registry rows anywhere; `.xiso` has no parser distinction (it *is* an XDVDFS image — the existing `inspect_direct_xbox_disc` arm covers it once registered).
- **STFS `disc_number`/`disc_in_set`** — parsed fields with no consumer (`xbox360_stfs_evidence.rs:366-367`); missing seam: a join into `MultiDiscSet` grouping.
- None of these are dead code — all are one-registry-row or one-join away from production.

## X. DO NOT REBUILD

- **`xdvdfs_signature.rs` + `xdvdfs_traversal.rs`** — a real bounded XDVDFS walker (in-memory *and* streaming `find_path_in_disc_image`); the foundation of every Xbox disc fact. Reused, never reimplemented.
- **`xbox_boot_evidence.rs` / `xbox360_boot_evidence.rs`** — bounded, honest about the shared-XDVDFS collision, real-corpus validated.
- **`executable_signatures.rs` XBE/XEX sections** — magic + certificate/optional-header parsing with exact offsets and bounded reads (`XBE_CERTIFICATE_READ_BYTES`, prefix caps).
- **`xbox360_stfs_evidence.rs`** — two-source field table with exemplary scope discipline (metadata only, never signatures/decryption) and an explicit content-type collision policy.
- **`launch/xemu_command.rs` + `xemu_execution/` + `launch/xenia_command.rs` + `xenia_execution.rs`** — fail-closed identity-gated planners and tested execution adapters.
- **`patch_manager/xemu_local.rs` / `xenia_local.rs` / `xenia_install_plan.rs` / `xenia_patch_document.rs`** — bounded inspection, candidate compatibility, patch documents with media-ID constraints.
- **`dat/firmware_evidence.rs` FirmwareSystem::Xbox rule** — the "xbox but not 360" DAT-name discipline and the honest no-MCPX-hash position.
- **Generic DAT/1G1R/Playing-Library/multi-disc machinery** — nothing Xbox-specific to add.

## Y. PRIORITISED BACKLOG + BEST 7 TASKS

**P0 — tiny, high-impact broken joins**
1. GUI launch contexts for xemu + Xenia (broken-joins #1/#17 — the whole family is one `main.rs` context-construction block away from a working Launch button).
2. Register `.xbe` + `.xex` (+ `.xiso`/`.god`) in `media_registry`/`content_registry` so the existing identity arms become reachable.

**P1 — user-visible completeness**
3. Persist `XbeTitleId`/`XexTitleId`/`XexMediaId` as re-verifiable catalogue facts + Doctor/library pre-warning (Sony-pattern).
4. Join STFS `disc_number`/`disc_in_set` into multi-disc grouping + add the missing election test.
5. Surface xemu four-file health in the launch panel (readiness half of #1).

**P2 — genuinely new**
6. TU/DLC relationship model keyed on STFS `content_type`/`base_version` (design recorded; do not build casually).
7. XDVDFS `.xiso` naming unification (`.xiso` row → same disc arm) and XBLA/STFS content-class display labels (raw `content_type` decoded for *display only*, two-source table).

**BEST 7 TASKS**

1. **`xbox-gui-launch-contexts`** — add Xemu/Xenia (and the other three adapters per broken-joins #1) launch-context construction in `main.rs:6245-6498`/`launch_readiness_page.rs`; reuse `DiscoveredStandaloneProfile` + planner outputs; non-goals: no backend changes; tests: GUI launch-context tests per platform; benefit: the Launch button works for both Xbox generations. **Tiny.**
2. **`xbox-executable-registration`** — add `xbe`/`xex` (and `.xiso`/`.god`) rows to `media_registry.rs` + `content_registry.rs` mirroring the `z64`/`sms` patterns; non-goals: no parser changes (arms exist); tests: end-to-end discovery → `inspect_direct_xbe`/`inspect_direct_xex` produce Verified title/media facts; extension-coverage harness; benefit: extracted-game users finally flow scanner→identity→launch. **Small.**
3. **`xbox-titleid-persistence`** — persist the three Xbox identity facts as catalogue facts keyed by the `(device, inode, size, mtime)` discipline; Doctor pre-warning "verified platform but no launch-gating title fact"; non-goals: no filename-derived facts; tests: persist→display→Doctor round-trip; benefit: pre-launch honesty + no repeated reads. **Small/Medium.**
4. **`stfs-multidisc-join`** — feed STFS `disc_number`/`disc_in_set` into `MultiDiscSet` grouping + add the Xbox-family multi-disc election regression test; non-goals: no STFS file-listing/signature work; tests: synthetic CON package set groups + election retains all discs; benefit: multi-disc 360 releases survive Playing-Library election. **Small/Medium.**
5. **`xemu-health-in-launch-panel`** — render `XemuSystemFileState` (mcpx/flash/eeprom/hdd) next to the launch button; non-goals: no firmware hash fabrication; tests: GUI row states; benefit: the most common xemu failure ("MCPX missing") is visible before launching. **Tiny.**
6. **`xenia-patch-launch-preview`** — surface the selected Xenia patch set (title/media-ID compatibility verdicts from `xenia_install_plan`) in the launch panel; non-goals: no new patch parsing; tests: incompatible-media-id patch shows blocking warning; benefit: patch compatibility is visible at decision time. **Small.**
7. **`stfs-content-class-labels`** — decode `content_type` into display-only class labels (XBLA/DLC/save/theme) from a two-source table; non-goals: never a platform/content-class *decision*; tests: label rendering + "never interprets" regression; benefit: users can tell a GOD game from a DLC package in the library. **Small.**

## Z. FINAL QUESTION

**"If EmuWiz stopped adding Xbox features today, what are the smallest changes required to make Original Xbox and Xbox 360 feel complete to an ordinary user?"**

- **Original Xbox:** two changes. (1) The GUI launch context for xemu — the entire backend (identity via XDVDFS/XBE, title-ID-gated planning, four-file system health, execution) is done and tested; the button simply isn't wired. (2) The `xbe` registry rows so extracted-game users reach the already-built identity path. With those, an ISO user's experience is complete end-to-end; the MCPX/BIOS/EEPROM/HDD states are already computed and would become visible the moment the launch panel renders them.
- **Xbox 360:** the same two changes (Xenia context + `xex` registration), plus one honesty upgrade: STFS packages (GOD/XBLA/DLC) already parse and identify, so once the GUI launch context exists, package users are served too. Everything beyond that — TU/DLC relationships, multi-disc joins — is genuinely new modeling, not completion.
- The pattern is identical to Sony's, and starker: **Xbox has the most complete backend of any family audited (both generations' planners are identity-gated and fail-closed) and the least visible one — its remaining work is almost entirely one GUI context block and a handful of registry rows.**
