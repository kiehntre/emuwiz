# Atari Audit Review & Implementation Roadmap (RESEARCH ONLY)

> **Research snapshot** — This review records an earlier repository state and proposed work. It is not current capability documentation; see the [README](../../README.md), [adapter support matrix](../ADAPTER_SUPPORT_MATRIX.md), and [roadmap](../../ROADMAP.md) for present guidance.

**Repo:** `/home/davedap/archivefs` · **Branch:** `feature/archivefs-unified-platform`
**Subject under review:** `docs/research/ATARI_FAMILY_SUPPORT_AUDIT.md` (800 lines)
**Method:** every load-bearing claim re-checked against source; the gaps it missed hunted; its task list converted into an implementation-ready roadmap. No source modified, no commits.

---

## 1. VERIFICATION VERDICT

**Overall: the audit is substantially correct and unusually well-sourced.** Its platform-row table, A78/Lynx field tables, ST/STX parser semantics, Hatari architecture trace, normalization analysis, security table, and coverage status all match the source exactly. Three claims are wrong, one is mis-worded, several are vague, and it missed one collision fact that materially changes a task's framing.

### 1.1 Claims verified correct (spot-checked against source)

| Claim | Verified at |
|---|---|
| All 7 platform rows: ids, aliases, strong/weak exts, magic, conflicts | `platform/mod.rs:616-726` — byte-for-byte match incl. `Atari 8-bit`'s 8 aliases and `Atari Jaguar`'s `abs`/`cof` weak exts |
| Jaguar CD absent; the Jaguar row's own explanation says so | `platform/mod.rs:689` — quote verbatim (incl. the doubled "Jaguar Jaguar CD" awkwardness) |
| STE/TT/Falcon folded into `AtariST` aliases, no equivalence rows | `:714` aliases; `EQUIVALENT_PLATFORM_IDS` has no Atari entries |
| `IdentityPlatform` has zero Atari variants | `game_identity.rs:265-288` |
| `supported_loose_rom_format` has no Atari rows | `game_identity.rs:808-826` |
| A78 field table (title 0x11/32B, rom_size 0x31 BE, cart_type 0x35 BE, tv_type 0x39 bit0, POKEY@$4000 bit0 + SuperGame bit1 decoded, save_device v2+ unsurfaced) | `atari7800_header_evidence.rs:17-34, 62-65` |
| Lynx field table (cart_name 0x0A..0x2A, manufacturer 0x2A..0x3A, rotation 0/1/2, `version_recognized`) | `lynx_header_evidence.rs:18-20, 71-74, 100` |
| `.st` parser: FAT12/BPB geometry, `proves_platform() == false`, PC-DOS 720K collision honesty | `disk_format/atari_st.rs` + `disk_format/mod.rs:30-40` |
| STX: `RSY\0`, version 3 only, ≤168 track records, 64 KiB bounds, `proves_platform() == true` | `disk_format/atari_stx.rs` + `mod.rs:79-99` |
| STX production-wired for detection (`platform::detect`) + Hatari, but absent from `CONTENT_FORMATS` and identity | `disk_format/mod.rs:426-431` dispatch; `content_registry.rs` has no `stx` |
| Hatari: discovery/config/TOS-SHA-256/health/projection/readiness all exist and are tested; **`hatari_command.rs`/`hatari_execution.rs` do not exist** | `launch/` has no hatari modules; `launch/input_projection.rs:373` `project_hatari_launch_input`; `readiness.rs:389` |
| **Hatari absent from Doctor — zero `hatari` matches in `diagnostics/`** | `rg -i hatari crates/archivefs-core/src/diagnostics/` → empty |
| AtariST launch row: `standalone_adapters: ["hatari"]`, `retroarch_core_hints: ["hatari"]` | `launch/platform_map.rs:144-146` |
| TOS verification: external caller-supplied references only, no embedded hash table, 2 MiB bound, `HatariTosHealth` 5-state | `patch_manager/hatari_local.rs` |
| Coverage rows: A7800 SyntheticValidated, Lynx RealValidated (Joust.lnx), Jaguar Deferred — quotes verbatim | `coverage_inventory.rs:397-421` |
| No Atari rows in `media_registry`; `st`/`msa`/`ipf` in `CONTENT_FORMATS` as `ComputerDisk`; no `msa`/`atr`/`stx` adapter in `inspect_disk_format` | `media_registry.rs`, `content_registry.rs:118-121`, `disk_format/mod.rs:426-431` |
| Header normalization: Lynx64/Atari7800_128 reversible, round-trip tested; only production consumers are the member detector + SNES path; **no Atari DAT/identity path calls `strip_known_header`** | `header_normalization.rs`, `normalized_view_provenance.rs` |
| RomM outbound: zero Atari rows | `romm_platform_mapping.rs` — no Atari in `STATIC_TABLE` or `romm_slug_targets` |
| ES-DE: exactly one Atari row (`atarist`) | `es_de_export.rs:167-171` |
| `.bin` never promoted: `detect_platform_report` exists (`detect.rs:293`) and shared exts are deny-listed; bare `.bin` → `MissingPairedFile` in discovery | `platform/mod.rs:184-191`, `discovery.rs:570-577` |
| RetroArch core resolution is dynamic via `.info` aliases; no Atari core names hardcoded | `launch/platform_map.rs:212-248`, `retroarch_command.rs` |
| `MultidiscHandlingPolicy` exists; Hatari projection carries one disk, not A/B | `library_views.rs`; `input_projection.rs:372-373` |
| **`.lyx` IS a strong extension** (surprising but true) | `platform/mod.rs:698` — `strong_extensions: &["lnx", "lyx"]` |

### 1.2 Claims that are WRONG

1. **§7.1 CAS collision — the audit's central CAS claim is false.** It states: *"EmuWiz lists `.cas` as weak for Atari 8-bit only; an MSX platform row exists but does not claim `.cas`."* **MSX *does* claim `.cas`** (`platform/mod.rs:949` weak `["rom","dsk","cas","zip"]`) — and so do **Commodore 64** (`:803`) and **VIC-20** (`:855`). The real picture is a **four-platform weak-extension collision**, which makes a `FUJI`-magic CAS parser *more* valuable than the audit argues, not less: it would be the only structural disambiguator among four claimants. §2's format table also lists `.cas` as "8-bit" only — same error.
2. **§24 RomM inbound mechanism — wrong.** *"Inbound `ROMM_SLUG_ALIASES`: `atari-st` → `AtariST` is the only Atari slug."* **No Atari entry exists in `ROMM_SLUG_ALIASES`** (`rg -i atari identity_source/romm/normalise.rs` → empty; `romm_slug_targets` has none either). Inbound resolution works **implicitly**: the RomM slug `atari-st` normalizes to `atarist`, which is already a folder alias, so `platform_for_alias` resolves it. The conclusion (ST is inbound-reachable, lossy for STE/TT/Falcon) survives; the stated mechanism does not. Any task that "adds the missing inbound row" would be adding a no-op.
3. **§1.1 Jaguar LAUNCH_COMPAT cell says "Deferred"** — there is **no Jaguar row at all** in `LAUNCH_COMPATIBILITY` (`platform_map.rs` Atari rows: AtariST only, `:144`). "Deferred" is a readiness/coverage status, not a launch-table state. Wrong vocabulary; right conclusion (nothing to launch with).

### 1.3 Claims that are imprecise

4. **§32 test counts** — "12 tests" for each of `atari7800_header_evidence` and `lynx_header_evidence`; actual is **14 and 14** (`rg -c '#\[test\]'`). Trivial, but the review standard was "exact".
5. **§2 `.st` inspector column says "likely"** — the inspector's `LIKELY_CONTENT_EXTENSIONS` (`inspector.rs:117-118`) contains `a26/a52/a78/j64/lnx/rom/bin` but **not `st`**. A `.st` file is not Inspector-classified as likely content. (Consequence is nil today since `.st` flows via `content_registry`, but the table cell is wrong.)
6. **§9.2/§11 `inspect_floppy_format` "only handles St/Stx"** — it *delegates* to `inspect_disk_format` (`hatari_local.rs:779-786`), which dispatches by extension; a Hatari-configured `.dsk` floppy routes to the **CPCEMU adapter** (wrong family) and fails closed. Same practical conclusion (no Atari-relevant inspection for `.msa`/`.ipf`/`.dsk`), but the mechanism description is loose.
7. **§25 ES-DE list stops at `atarijaguar`** — ES-DE also ships an **`atarijaguarcd`** system. A future Jaguar CD platform row has an ES-DE target waiting; the audit's "no platform row → no ES-DE story" framing hides that one row + one map entry closes it once a boot detector exists.

### 1.4 What the audit missed

8. **The `.cas` four-way collision** (see #1) — the single biggest miss, because it changes the CAS parser task from "nice disambiguator" to "the only structural separator among four weak-extension claimants", and it should be scoped as family-shared evidence (Atari 8-bit claim requires the `FUJI` magic; the other three keep their own formats).
9. **`.car` is a 7800 container too.** The audit's CAR-parser task (§34 #12) scopes it to "8-bit/5200". In Atari800-family emulator ecosystems `.car` also carries 7800 titles, yet the `Atari7800` row doesn't claim `.car` at all (`:645-647`: weak `bin/rom/zip`). The CAR task should be tri-platform (8-bit/5200/7800 via the cart-type field), and the 7800 row's extension claims revisited.
10. **Lynx weak extension `"o"`** (`:699`) — an Atari 8-bit assembler-object extension sitting on the Lynx row (cc65-Lynx homebrew artifact, defensible but unexplained; nothing else claims `.o`, so no collision — but it should be documented or moved).
11. **Jaguar weak `abs`/`cof`** (`:686`) — defensible (Jaguar homebrew is built with ST toolchains: rmac/vlink emit `.abs`/`.cof`), but the audit never explains why a Jaguar row claims ST-executable extensions. One sentence would prevent a future "cleanup" from deleting a correct claim.
12. **Dual launch paths on one row** — AtariST's row carries *both* `standalone_adapters: ["hatari"]` and `retroarch_core_hints: ["hatari"]` (standalone Hatari vs the RetroArch Hatari *core* are different runtimes with different config surfaces). The audit treats the row as one Hatari story; the Hatari command task must decide which of the two it serves first (standalone, given `hatari_local`'s depth).
13. **`IdentityKind::LooseRomCanonicalSha256` is the ready-made mechanism for task #3/#8** — the audit says "emit physical & normalized SHA-256" but never names the existing kind (`game_identity.rs:196-202`: byte-order-normalized canonical hash, N64-only today, gated on "a tested, reversible byte-order normalization"). A78/Lynx normalization *qualifies by the kind's own stated criteria*; the wiring task should extend this kind rather than invent a second one.
14. **MSA evidence-confidence framing** — §10.2 proposes "Corroborated/Strong for AtariST with folder; family-level without", but MSA (`0x0E0F`) is *only* Atari-ST-family media (the `.dim` FM-TOWNS collision is a different magic), so a validated MSA is closer to the STX model (`proves_platform() == true`) than the `.st` model. The audit's own STX precedent argues for the stronger claim.

## 2. PER-PLATFORM STATUS (implementation-ready)

Legend: ✅ works today · 🟡 exists, orphaned/partial · ❌ missing.

### Atari 2600
- Works today: platform row + aliases; `.a26` strong ext (unregistered in scanners); hash identity generic.
- Parsers/evidence: none needed (no intrinsic header — correct).
- Wired: registry + display only. Orphaned: nothing. Requires new parsing: **no** (bankswitch = DAT/stella-db territory, correctly out of scope).
- DAT/hash-only: yes, entirely. Launch today: no (no `IdentityPlatform`/row → RetroArch path unreachable).
- Completeness blocker: the three P0 wiring rows (registry/identity/launch).

### Atari 5200
- Works today: platform row; `.a52` strong (unregistered).
- Parsers: none. **`.car` (CART header) is the only structural 5200-vs-8-bit discriminator and is unbuilt** (audit §4/§7.2 — verified).
- DAT/hash-only until CAR exists. Launch: no (same blockers).
- Blocker: P0 wiring + (optionally) `car-header-evidence`.

### Atari 7800
- Works today: platform row, `ATARI7800` magic rule (Strong), coverage row (SyntheticValidated).
- Parsers: `atari7800_header_evidence.rs` complete (14 tests) + `Atari7800_128` normalization + fusion rule `atari7800_header` + scope `PlatformSpecific("Atari7800")`.
- Wired: archive-member detector + fusion (example-layer). Orphaned from: loose-file discovery, identity, DAT normalized hashing, launch, ES-DE, RomM.
- Requires new parsing: no. DAT/hash-only: headerless dumps only.
- Launch today: no. Blocker: P0 wiring (#1-#5 of the audit's own list) — the audit's "most-wired-yet-orphaned" verdict is **correct**.

### Atari 8-bit
- Works today: platform row (8 aliases), strong `atr/atx/xex/xfd`, weak `cas/bin/rom/car`.
- Parsers: none — ATR/XFD/ATX/CAS/CAR/XEX all unparsed (audit §6-7 specs are sound and correctly bounded).
- Wired: nothing beyond registry. DAT/hash-only today.
- Launch today: no. Blockers: P0 wiring; then CAR (5200/7800/8-bit discriminator) and XEX segment-walk are the highest-value new parsers; ATR bounded header next; ATX defer (audit's call is right); CAS re-scoped per §1.2 #1.

### Lynx
- Works today: platform row (`lnx`+`lyx` strong, `LYNX` magic Strong), coverage RealValidated (Joust.lnx through fusion).
- Parsers: `lynx_header_evidence.rs` complete (14 tests) + `Lynx64` normalization + fusion rule.
- Wired: member detector + fusion. Orphaned from identity/discovery/DAT/launch.
- Headerless `.lyx`: correctly DAT/hash-only (strong-ext claim is fine — no magic is claimed for it).
- Launch today: no. Blocker: P0 wiring.

### Jaguar
- Works today: platform row only (`j64`/`jag` strong, `abs`/`cof` weak — homebrew toolchain extensions, should be documented not deleted); coverage Deferred with the exact Batch-4 rationale.
- Parsers: none; encrypted boot header means **no plaintext platform proof exists** — the audit's HASH/DAT-ONLY verdict is correct, and the optional reversible `JaguarJ64` 32-byte header-strip (§17.1) is research-grade, not committed fact (the ".j64 = 32-byte JAGUAR magic" attribution needs a source before any implementation).
- Launch today: no. Blocker: P0 wiring (+ Virtual Jaguar RetroArch row).

### Jaguar CD
- Works today: nothing (no platform row — the Jaguar explanation explicitly disclaims it).
- Exists: the generic optical stack it would reuse (ISO9660/CUE/CHD all verified, audit §18.2 ✓); ES-DE already ships `atarijaguarcd` (missed by the audit).
- Requires new parsing: `jaguarcd_boot_evidence` (two-source verified boot signature) — **research-first**; CHD arm + platform row + ES-DE row then follow the PcEngineCd pattern.
- Launch today: no. Blocker: everything above, in order.

### ST / STE / TT / Falcon
- Works today: `.st` (FAT12-geometry, Probable-only — correct), `.stx` (Pasti, conclusive) — both production-wired for *platform detection* and Hatari config inspection; `.msa`/`.ipf` extension-registered (CONTENT_FORMATS `ComputerDisk`); full Hatari config/TOS/machine-model stack; `atarist` ES-DE row; AtariST launch row (standalone + core hint).
- STE/TT/Falcon: folded aliases; no canonical identity (acceptable; the audit's lossy note stands).
- Orphaned: `.stx` from loose-file discovery; Hatari from launch/execution/Doctor.
- Requires new parsing: **MSA** (best new-parser candidate — with the §1.4 #14 confidence correction); IPF: never (SPS licensing — audit's call is right and final); HDD images: defer (collision-prone — right call).
- Can launch today: via RetroArch-Hatari core only in principle — **not in practice** (no identity resolution for AtariST: no `IdentityPlatform::AtariSt`), so effectively nothing launches today. Blocker: identity variants + Hatari command/execution + Doctor.

## 3. EXTENSION MASTER TABLE (corrected)

| Ext | Platform row claim | content_registry | media_registry | inspector | Parser | Verdict |
|---|---|---|---|---|---|---|
| a26 | strong | — | — | ✅ | none | registry-only |
| a52 | strong | — | — | ✅ | none | registry-only |
| a78 | strong | — | — | ✅ | **yes** (orphaned from identity) | parsed-but-orphaned |
| atr | strong | — | — | — | none | registry-only |
| atx | strong | — | — | — | none | registry-only (defer parsing) |
| xex | strong | — | — | — | none | registry-only |
| xfd | strong | — | — | — | none | registry-only (headerless forever) |
| cas | weak (8-bit) | — | — | — | none | **4-way collision** (MSX :949, C64 :803, VIC-20 :855) |
| car | weak (8-bit, 5200) | — | — | — | none | registry-only; 7800 doesn't claim it (should it?) |
| lnx | strong | — | — | ✅ | **yes** (orphaned from identity) | parsed-but-orphaned |
| lyx | **strong** | — | — | — | none | registry-only (headerless) |
| o | weak (Lynx) | — | — | — | none | misplaced/undocumented (cc65 artifact) |
| j64 / jag | strong | — | — | ✅ | none | registry-only |
| abs / cof | weak (Jaguar) | — | — | — | none | defensible homebrew exts — document, don't delete |
| st | strong | ✅ ComputerDisk | — | — | **yes** (detection-wired) | parsed; not identity-wired |
| stx | strong | — | — | — | **yes** (detection-wired) | parsed; discovery-orphaned |
| msa | strong | ✅ ComputerDisk | — | — | none | best new-parser candidate |
| mfm | strong | ✅* | — | — | none | (*via MSA row group; extension-only) |
| ipf | weak | ✅ ComputerDisk | — | ✅ | none (SPS-licensed; never) | pass-through to Hatari |
| dim | — | — | — | — | none | absent (FM-TOWNS collision, different magic) |
| hdf / vhd | — | — | — | — | none | deliberately unsupported (Amiga safety) |

*(audit §2 reproduced with the four corrections: `.cas` breadth, `.st` inspector cell, `.lyx` strength, `.o` note.)*

## 4. TOP 15 BROKEN JOINS (revised ranking)

1. `CONTENT_FORMATS` omits all Atari cartridge/computer extensions *(audit #1 — verified)*
2. `IdentityPlatform` has zero Atari variants *(audit #2 — verified; the keystone)*
3. Loose-ROM dispatch (`supported_loose_rom_format`) has no Atari rows *(#3 — verified)*
4. A78 parser orphaned from identity *(#4 — verified)*
5. Lynx parser orphaned from identity *(#5 — verified)*
6. **Normalized canonical hashing not extended to A78/Lynx** — upgraded: route through the *existing* `IdentityKind::LooseRomCanonicalSha256` (N64 precedent, `game_identity.rs:196-202`) rather than a bespoke path *(audit #10, mechanism sharpened)*
7. `ES_DE_SYSTEM_MAP` missing 6 Atari rows *(#6 — verified; +`atarijaguarcd` future-proofing note)*
8. `LAUNCH_COMPATIBILITY` missing 6 Atari rows *(#7 — verified; RetroArch execution needs zero new code)*
9. `.stx` missing from `CONTENT_FORMATS` *(#11 — verified)*
10. Hatari has no command/execution adapters *(#8 — verified; standalone path chosen first per §1.4 #12)*
11. Hatari absent from Doctor *(#9 — verified: zero grep matches)*
12. **RomM outbound zero Atari rows** *(#14 — verified; and the audit's inbound claim corrected: no explicit row needed, implicit alias already works)*
13. `.cas` four-way collision with no structural separator *(new — audit had this wrong)*
14. Coverage inventory missing 2600/5200/8-bit/ST rows *(#12 — verified)*
15. Multi-disk ST projection (A/B floppies) *(#13 — verified; depends on #10)*

*(Jaguar CD drops out of the top 15: it is research-gated new work, not a join — audit #15 misfiled it.)*

## 5. TOP 10 GENUINELY MISSING PARSERS/FEATURES (revised)

1. **MSA** header/track/RLE-*validation* (`disk_format/msa.rs`) — best candidate; caps specified by the audit §10.3 are sound; confidence upgraded to STX-style `proves_platform() == true`.
2. **CAR/CART header** — tri-platform (8-bit/5200/**7800**); the only 5200-vs-8-bit structural discriminator; audit under-scoped it.
3. **ATR** bounded header (magic 0x0296, sector size 128/256, exact-length geometry).
4. **XEX** bounded segment walk (`FF FF` framing; cap segments/size).
5. **CAS** `FUJI`-magic chunk framing — now justified by the 4-way collision.
6. **Jaguar CD boot signature** — research-first, two sources (BigPEmu/Virtual Jaguar), Medium.
7. **Hatari command/execution adapters** (not parsing, but the family's biggest feature gap).
8. **Hatari Doctor adapter** (`diagnostics/profiles.rs`).
9. **JaguarJ64 reversible header-strip** — only after a source for the 32-byte container layout is verified; still reconciliation-only.
10. **ATX** — deliberately deferred (VAPI review); keep deferred.

## 6. TOP 10 CHEAPEST HIGH-IMPACT TASKS (implementation-ready)

| # | Slug | Files touched | Reused | Non-goals | Tests | Deps | Size |
|---|---|---|---|---|---|---|---|
| 1 | `content-registry-atari-extensions` | `ingestion/content_registry.rs` | `ContentKind::{RomCartridge,ComputerDisk,TapeImage}` | no identity claims, no `.cas` without FUJI gate | registry round-trips + extension-coverage harness | none | **Tiny** |
| 2 | `identity-platform-atari-variants` | `game_identity.rs` enum+`from_catalogue`+labels | existing variant pattern (Pcfx/ThreeDo) | no inspect arms yet (honest Unsupported) | catalogue round-trips | none | **Tiny** |
| 3 | `loose-rom-atari-dispatch` | `game_identity.rs` (`supported_loose_rom_format`, `inspect_loose_rom`) | `parse_a78_header`, `parse_lynx_header`, `strip_known_header` | no mapper guesses; no `.lyx`/`.bin` structural claims | synthetic a78/lnx fixtures incl. wrong-magic refuse | 1,2 | **Small** |
| 4 | `atari-canonical-hash` | `game_identity.rs` (extend `LooseRomCanonicalSha256` eligibility), `normalized_view_provenance.rs` | existing kind + normalization round-trips | no new hash kind; never strips destructively | headered/headerless convergence | 3 | **Small** |
| 5 | `esde-atari-rows` | `launch/es_de_export.rs` | row pattern + fullname verification discipline | no unverified names (re-verify `es_systems.xml`) | mapping-row tests | 2 | **Tiny** |
| 6 | `launch-compat-atari-rows` | `launch/platform_map.rs` | `retroarch_core_hints` pattern | no standalone adapters beyond existing AtariST row | candidate-generation tests | 2 | **Tiny** |
| 7 | `hatari-command-execution` | new `launch/hatari_command.rs`, `launch/hatari_execution.rs`, `launch/mod.rs` | `hatari_local`, `project_hatari_launch_input`, `process_spawn`, `flycast_command/execution` template | no shell strings; no A/B multi-disk beyond `--floppy-a/b`; decide standalone-vs-RA-core first (§1.4 #12) | preflight/plan/execution tests | 2 | **Medium** |
| 8 | `doctor-hatari-adapter` | `diagnostics/profiles.rs`, `runner.rs`, `mod.rs` | `discover_hatari_profiles`, `inspect_hatari_game` | no writability inventions | runner findings | none | **Small** |
| 9 | `disk-format-msa` | new `disk_format/msa.rs` + dispatch | `BoundedReader`, caps discipline | no RLE *decompression* (validation only); `.dim` stays out | valid/truncated/bad-magic/overflow fixtures | none | **Small–Medium** |
| 10 | `car-header-evidence` | new `atari_car_header_evidence.rs` + member detectors + fusion + dispatch | A78/Lynx module pattern | no type-table guesses (two-source) | type fixtures (8-bit/5200/7800) | 1,2 | **Small** |

*(The audit's #9 `stx-content-registry` folds into task 1; #11 ATR and #14 RomM-outbound remain as listed there — ATR after MSA, RomM row after slug verification.)*

## 7. FINAL ANSWERS

**1. Which parts of the current Atari audit are correct?**
Nearly all of the source-grounded analysis: every platform row, both cartridge-header parsers' field tables, the ST/STX parser semantics and their exact production wiring, the Hatari stack trace (including the verified zero-Hatari Doctor gap and the missing command/execution adapters), the TOS verification design, the normalization hidden-value analysis, the security table, coverage status, and the do-not-rebuild list. Its own P0/P1 framing (five tiny wiring tasks unlocking the whole cartridge family) is verified correct.

**2. Which parts are incomplete or misleading?**
Three factual errors: the CAS collision claim (MSX/C64/VIC-20 *do* claim `.cas` — it's a four-way collision, not Atari-only); the RomM inbound mechanism (no `atari-st` slug row exists — resolution is implicit via alias normalization); the Jaguar "Deferred" launch-table cell (no row exists). Imprecisions: test counts (14, not 12), the `.st` inspector cell (absent, not "likely"), the `inspect_floppy_format` mechanism (delegates, doesn't filter), and the ES-DE section omitting that `atarijaguarcd` already exists as a target.

**3. What did it miss?**
The four-way `.cas` collision (changes the CAS task's value and scope); `.car` as a 7800 container (tri-platform CAR task); `IdentityKind::LooseRomCanonicalSha256` as the ready mechanism for normalized Atari hashing; the unexplained-but-correct `abs`/`cof` (Jaguar homebrew) and `o` (Lynx/cc65) extensions; the standalone-vs-RetroArch-Hatari dual path on one launch row; MSA's STX-grade confidence precedent.

**4. What should be done before the next EmuWiz release?**
Exactly the audit's P0 set, with two refinements: tasks 1-6 of §6 above (extensions, identity variants, loose-ROM dispatch + A78/Lynx parsing, canonical-hash extension, ES-DE rows, launch rows) — all wiring, zero new parsers — plus the registry-drift corrections this review found (`wad`-style claims don't exist here, but the `.cas` table error must not propagate into a CAS row without the FUJI gate). Hatari command/execution (task 7) is the one Medium item worth slotting if release scope allows: it converts the family's only standalone adapter from "row without a launcher" into a real path.

**5. What should explicitly wait until after the release?**
Every new parser: MSA, CAR, ATR, XEX, CAS, JaguarJ64, Jaguar CD boot evidence (research-gated); the Hatari Doctor adapter (if task 7 slips); RomM outbound rows (needs slug verification against RomM's supported-platforms page); multi-disk A/B projection; ATX (indefinitely, until a VAPI two-source review exists); IPF (forever — licensing); Atari HDD images (collision-prone, deferred).
