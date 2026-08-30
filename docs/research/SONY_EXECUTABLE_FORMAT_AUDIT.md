# Sony Executable / Container Format Audit — EmuWiz (RESEARCH ONLY)

**Scope:** PS-X EXE (PS1) · ELF/BOOT2 + executable-CRC pipeline (PS2) · EBOOT.PBP / PARAM.SFO / DATA.PSAR / PRX (PSP) · EBOOT.BIN / SELF / ELF / PARAM.SFO (PS3)
**Branch:** `feature/archivefs-unified-platform`
**Method:** static source analysis only — no source modified, no commits.
**Companion to:** `docs/research/SONY_PLAYSTATION_SUPPORT_AUDIT.md` (platform/launch/registry findings live there; this doc is the executable-format layer).

---

## 1. PS1 — PS-X EXE

**Parser that exists**
- `playstation_boot_evidence.rs`: `PS_X_EXE_MAGIC = b"PS-X EXE"` (`:55`), `looks_like_psx_exe` (`:194-199`, first 8 bytes only), and `PsxExeDetector` — a `ContentDetector` over the boot executable's header bytes.
- The `SYSTEM.CNF` side (`parse_system_cnf_boot:100`) parses `BOOT=`/`BOOT2=` with case tolerance, first-line-wins, and `normalize_boot_target:161` (strips `cdrom:`/`cdrom0:`, `;version`, refuses empty/`..`/oversized components — a path-safety review in itself).

**Metadata extracted**
- Executable magic only. No PS-X EXE header fields (t_addr, pc0, text size, section headers at 0x800) are decoded anywhere.

**Identity facts produced**
- `BootStructure` (`"BOOT"`, Corroborated) + `ProductCode` (serial candidate from the boot filename, Corroborated, explicitly "not proof of any one platform", `:217-225`).
- Production: `disc_evidence_collector.rs:319` runs `PsxExeDetector` on the header read from the disc boot path — **production-wired**.

**Persisted / consumed**
- `IdentityKind::Ps1Serial` (verified from `SYSTEM.CNF` content during identity inspection) feeds the DuckStation planner (`verified_ps1_serial`, fail-closed) and `patch_manager/duckstation_local.rs` ("DuckStation consumes identity that core has already established").

**Structural vs identity:** magic detection is structural; the serial (from `SYSTEM.CNF`, not the EXE) is the identity fact. The EXE's own bytes contribute no identity fact today.

**Would deeper parsing add anything?** No for identity — the serial already pins the release, and Redump/hash identity covers exact bytes. PS-X EXE t_addr/pc0/text-size would only matter for homebrew/patching tooling, which is out of scope. **Leave the header unparsed.**

## 2. PS2 — ELF @ BOOT2 + executable CRC pipeline

**Parser that exists**
- `ps2_boot_evidence.rs`: deliberately a *thin combination* of two reviewed pieces — `parse_ps2_system_cnf:65` (accepts `BOOT2=` only; a PS1 `BOOT=` line is rejected, tested) and `executable_signatures::looks_like_elf` (first 4 bytes). `observe_ps2_evidence:105` implements the Batch-6 reviewed upgrade: the `BOOT2` fact is promoted **Corroborated → Strong only when the executable named by `BOOT2=` is independently confirmed to be a real ELF** — "verified against real bytes, not just a filename"; `None`/`Some(false)` stays Corroborated. The ELF fact itself stays `Weak` (generic format, never platform evidence).
- No ELF header fields (e_entry/e_machine/program headers) are parsed anywhere in the crate.

**Identity facts produced — the production CRC pipeline** (`game_identity.rs`, `inspect` arm for PS2 ISOs, `:3960-4069`):
1. ISO-locate `SYSTEM.CNF` → `parse_system_cnf_boot2` → **`Ps2Serial` Verified** ("serial derived from the exact boot executable path, not an archive filename", `:3974-3982`).
2. ISO `find_path` of the BOOT2 executable (component-split, traversal-safe).
3. Bounded read — `MAX_EXECUTABLE_BYTES` = 32 MiB, `ResourceLimitReached` beyond (`:4016-4028`).
4. ELF magic validation (`Invalid` if not ELF, `:4047-4059`).
5. **`pcsx2_executable_crc`** (`:4850-4854`): XOR-fold of little-endian u32 words over the whole ELF — the PCSX2 cheat-CRC convention — emitted as **`IdentityKind::Pcsx2ExecutableCrc` Verified, `IdentityConfidence::ExactBytes`**, formatted `{:08X}`.

**Persisted / consumed**
- `patch_manager/pcsx2_identity.rs::from_report` (`:39-99`) converts the report into `Pcsx2GameIdentity` — **never promotes candidate/filename-derived CRC** (`:37-38`), maps every non-Verified status to a truthful terminal state with plain-language reasons ("EmuWiz could not prove the game CRC required for PCSX2 cheats", "Game identification is not available for this image format yet" — the Deferred state is exactly what PS2 `.chd` hits today).
- Region derived from the *serial* only (`pcsx2_region_for_serial:110-123`, documented prefix families); cheats directory resolution refuses `cheats_ws` (`:186-202`, widescreen is a separate category).
- `patch_manager/pcsx2.rs::normalize_crc` (`:329-333`) — 8-hex-uppercase gate at the consumer edge.
- Launch: `pcsx2_command.rs:118` requires the verified serial; the CRC is the cheats/mods key.

**Would deeper ELF parsing add anything beyond Ps2Serial + Pcsx2ExecutableCrc?** **No.** The CRC is `ExactBytes` over the whole executable — it already subsumes anything the ELF header could say about identity. Entry point/machine/segment data would add per-game emulator-config facts, which are PCSX2's runtime domain and not modeled (correctly). The only marginal candidate — checking `e_machine == EM_MIPS` to strengthen the ELF leg — is redundant once the BOOT2+ELF strong leg exists. **Do not parse deeper.**

## 3. PSP — EBOOT.PBP / PARAM.SFO / DATA.PSAR / ELF/PRX

**Parsers that exist** (`psp_pbp_evidence.rs`, complete but **unwired** — no `ContentDetector` registration, no registry row, no identity arm; `.pbp` is a strong extension on PSX and PSP):
- `looks_like_pbp:87` (magic), `PbpHeaderFact:97` (version + 7-section offset table), `parse_pbp_header:107`, `validate_pbp_offsets:180` (monotonicity, first-offset-after-header, DATA.PSAR-within-EOF — zero-length sections legal, tested).
- `read_pbp_param_sfo:226` — bounded section slice through the **shared** `param_sfo::parse_param_sfo` (never a second SFO implementation).
- `read_data_psar_prefix:235` — a *bounded* prefix of DATA.PSAR (which can be gigabytes); the plumbing for magic-level inspection without ever reading the section.
- `observe_pbp_evidence:253` — `Container="PBP"` Strong + `ProductCode` (DISC_ID) via the shared `product_code_evidence` helper.

**PARAM.SFO** (`param_sfo.rs`): generic key/table parser (`SfoEntry:84`, `SfoObservation:93`, `get/get_text:99-108`, `parse_param_sfo:125`); `product_code_evidence:201` emits deliberately "candidate only" product codes. Consumers: `psp_boot_evidence` (DISC_ID/title/category/disc_version from `PSP_GAME/PARAM.SFO`), `ps3_boot_evidence` (TITLE_ID/TITLE/CATEGORY/APP_VER from `PS3_GAME/PARAM.SFO`), PBP (unwired). **One parser, three consumers — no duplication.**

**DATA.PSAR / PS1-Classics discrimination (investigation 2):** currently *no* PSAR magic check exists — the prefix reader is in place but unused. The safe discrimination is a **bounded magic check only**:
- PSP digital full-game PBP: DATA.PSAR is the game archive (update/install data; `PSAR`-family container).
- PS1 Classics (PSX2PSP-style eBoot): DATA.PSAR's own header carries `PSISOIMG0000` (single-disc) / `PSTITLEIMG000000` (multi-disc) markers.
Both are plaintext markers within the first bytes of the section — distinguishable **without any decompression or decryption**, via `read_data_psar_prefix` + a two-source-verified magic table, exactly the crate's SFB/PKG precedent (bounded fixed header, corroborated by two independent references). This would produce a genuinely new, useful fact: **content class** ("PSP game" vs "PS1 Classic on PSP") — a classification no current fact expresses. It must never touch the (encrypted) payload.

**ELF/PRX:** `PSP_GAME/SYSDIR/EBOOT.BIN` is not parsed beyond the PSP layout evidence; PRX (relocatable PSP modules) has no parser anywhere — correctly, since PRX identity adds nothing beyond DISC_ID + ISO hash.

**Persisted / consumed:** `PspDiscId` (Verified) → PPSSPP planner (fail-closed, `ppsspp_command.rs:102`) + `ppsspp_local.rs` inspection. For PBP-sourced dumps today: nothing — the files never reach identity (the broken join from the companion audit).

## 4. PS3 — EBOOT.BIN / SELF / ELF / PARAM.SFO

**Parsers that exist**
- `executable_signatures.rs`: `looks_like_self:79` (`SCE\0` magic) + `SelfDetector` — `Strong` `ContentSignature` "SELF (Signed ELF) container magic present", doc: "Never decrypts or interprets the signed/encrypted body — only the leading magic is inspected."
- `ps3_boot_evidence.rs`: `PS3_LAYOUT_PATHS` (`:24-27` — `PS3_GAME`, `USRDIR`, `USRDIR/EBOOT.BIN`, `PARAM.SFO`), `Ps3LayoutObservation` with `title_id/title/category/app_version` via the shared SFO parser, `check_eboot_self_magic:64` (pure, header-supplied). The PS3↔PSP `EBOOT.BIN` collision is handled by the `USRDIR` path shape (`:8-10`).
- `ps3_disc_evidence.rs`: `PS3_DISC.SFB` magic-only (`.SFB` @ `:73`; TITLE_ID/HYDRid extraction deliberately not implemented — "single-source corroboration bar"); **PKG** = bounded fixed 0x80-byte header, two-source corroborated (PS3 Dev wiki `pkg_files` + PS3Py `pkg.py`).

**What SELF parsing can safely expose (investigation 1):**
- **Executable type**: yes, and already done — SELF-vs-ELF-vs-unknown via magic (SELF magic `Strong`, reusing `SelfDetector`). ELF *inside* a SELF is present only for unprotected/re-signed files; retail `EBOOT.BIN` SELF bodies are encrypted, so **ELF metadata is not extractable without decryption** (unselfing is a key-using transformation — out of scope by the crate's rules).
- **SDK/build/version metadata**: the SELF metadata section that carries these fields is **encrypted on protected retail SELF files** — not safely extractable in general. Any parser claiming it would violate the crate's fail-closed bar. Correctly absent.
- **Embedded content identifiers**: for *NPDRM* (PSN) SELF files an `NPD` block (`NPD\0` magic) sits in plaintext and carries the content ID; for retail disc SELF files it does not exist. The repo has **zero NPD references**. A bounded `NPD`-magic + content-ID fact *is* theoretically safe (plaintext, fixed offset), but it applies to one distribution subclass, needs two-source verification, and — since PSN PS3 content is already `PKG`-identified via TITLE_ID — adds a fact with no consumer. **Not recommended now.**
- **Bottom line**: the only universally safe SELF facts are the ones already implemented (magic/type). Everything deeper is either encrypted (SDK/ELF) or redundant with TITLE_ID (NPD content-id). **Leave SELF at magic.**

**PARAM.SFO relationship**: one shared parser feeds both PSP and PS3 observations; keys are looked up by name (`TITLE_ID`, `DISC_ID`, `CATEGORY`, `APP_VER`); product codes are always `ProductCode`-kind candidates, never identity claims. This is the right shape — keep it.

**Persisted / consumed**: `Ps3TitleId` (Verified) → `rpcs3_command.rs:111` (fail-closed) + firmware-readiness gate (`:142-145`); `rpcs3_local.rs` inspection consumes core-established identity.

## 5. Cross-cutting answers

**Q3 — PS2 ELF beyond serial+CRC:** nothing (see §2). The CRC *is* the executable identity; the ELF header is a validation leg, not an information source.

**Q4 — should executable identity be persisted as a reusable catalogue fact?**
Today it is *recomputed and re-verified* at every use: launch planners require "freshly re-confirmed" serials (`duckstation_command.rs:142`), and `Pcsx2GameIdentity::from_report` derives state from a fresh report. Nothing persists `Ps1Serial`/`Ps2Serial`/`Pcsx2ExecutableCrc`/`PspDiscId`/`Ps3TitleId` as catalogue rows.
Recommendation: **persist them as derived, re-verifiable facts** — keyed to the file-identity discipline `launch/execution.rs` already uses (`(device, inode, size, mtime)` shape), so a cache hit can never outlive the bytes it verified. Benefits: pre-warn "launch will fail: no verified serial" in Doctor/library *before* the user tries; avoid repeated 32 MiB ELF reads; expose serials/CRC in GUI columns. The re-verification discipline at launch/cheat time must stay — persistence is an accelerator and a UI fact, never a trust anchor.

**Q5 — do cheats/mods rediscover executable identity independently?**
**No.** There is exactly one identity pipeline. `Pcsx2GameIdentity::from_report` filters the *same* `GameIdentityReport` the launch layer uses (`pcsx2_identity.rs:78-88`); the cheat provider consumes `verified_crc()`/serial from that struct; `duckstation_local.rs` states the policy outright ("DuckStation consumes identity that core has already established"). What *is* recomputed is the report itself at operation time — deliberate freshness, not duplicate implementations. **No de-duplication work exists to do.**

## 6. Production-wiring summary

| Component | Parser | Registered detector | Identity arm | Persisted | Consumers |
|---|---|---|---|---|---|
| PS1 `SYSTEM.CNF`/PS-X EXE | yes | **yes** (`disc_evidence_collector.rs:319`) | yes (`Ps1Serial`) | re-verified per launch | DuckStation planner, duckstation_local |
| PS2 BOOT2/ELF | yes | via evidence modules | yes (`Ps2Serial`, `Pcsx2ExecutableCrc`) | re-verified per launch | PCSX2 planner, pnach pipeline |
| PSP layout/SFO | yes | via evidence modules | yes (`PspDiscId`) | re-verified per launch | PPSSPP planner, ppsspp_local |
| PSP/PSX PBP | **complete** | **no** | **no** | — | **nothing (orphaned)** |
| PS3 layout/SFO/SELF | yes | via evidence modules | yes (`Ps3TitleId`) | re-verified per launch | RPCS3 planner, rpcs3_local |
| PS3 SFB/PKG | yes (bounded, two-source) | no registry row for `.pkg` | no `.pkg` arm | — | nothing (specimen-validated only) |
| SFO (shared) | yes | — | via hosts | — | PSP + PS3 + PBP hosts |

## 7. Conclusions

**Wire now** (all join-fixes, no new parsing):
1. The `.pbp` end-to-end join — registry rows + identity arm consuming the *already-complete* `psp_pbp_evidence` module (header, offsets, embedded SFO, `observe_pbp_evidence`). This is the single highest-value executable-format task: the parser is finished and tested; only the last hop is missing.
2. The `.pkg` identity arm — registry row + arm consuming the already-reviewed `parse_pkg_header`, emitting `Ps3TitleId` for digital installs.
3. Persistence of the five verified identity facts as cache-shaped catalogue facts (per §5-Q4), using the existing file-identity key discipline.

**Parse deeper (bounded, two-source gated):**
4. `DATA.PSAR` bounded magic check (`PSISOIMG0000` / `PSTITLEIMG000000` / PSAR family) via the existing `read_data_psar_prefix` — a safe content-class fact (PSP game vs PS1 Classic) with no decompression. Verify the exact offset/marker conventions against two independent sources before merging; never touch the payload.

**Explicitly leave alone:**
- PS-X EXE header fields (t_addr/pc0/text size) — no identity or readiness value.
- PS2 ELF headers beyond magic — `Pcsx2ExecutableCrc` (ExactBytes) already subsumes them.
- PS3 SELF internals — SDK/version metadata and the inner ELF are encrypted on retail content; decryption is out of scope by design; NPD content-id parsing has no consumer.
- PRX modules — no fact beyond DISC_ID + hash exists to extract.
- The single shared `param_sfo` parser and the single identity pipeline feeding both launch and cheats — there is nothing duplicated to consolidate.
