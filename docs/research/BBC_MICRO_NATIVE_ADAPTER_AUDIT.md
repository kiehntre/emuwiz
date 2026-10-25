# BBC Micro Native Emulator Adapter Audit (V1)

**Status: AUDIT ONLY. No production code was written or modified to produce this document.**

This audit follows on from `docs/research/NATIVE_EMULATOR_ADAPTER_COVERAGE_AUDIT.md`, which
classifies BBC Micro / Acorn Electron as `GENERIC_RETROARCH_ONLY` — no dedicated native launch
adapter exists — and places BBC below P1 (openMSX execution seam) and P2 (Atari800) in priority.
That prior audit named BeebEm and b-em as candidate names but did not evaluate either. This
document performs that evaluation and defines a safe, conservative V1 scope for a future adapter,
**without implementing it**.

Starting HEAD for this audit: `eb7ac094ff8f8c36449f58ae8a59896c6138e7b` on
`feature/archivefs-unified-platform` (a shared, multi-agent working tree — other in-flight,
uncommitted changes belonging to concurrent sessions were present throughout and are unrelated to
this task; see Preflight below).

---

## Preflight

- `git branch --show-current` → `feature/archivefs-unified-platform`
- No branch or worktree anywhere in this repository owns a BBC Micro native adapter. `git log
  --all --oneline --decorate -200 | grep -Ei 'bbc micro|beebem|b-em|acorn'` and `git branch -a |
  grep -Ei 'bbc|beeb|b-em|acorn'` surface only **disk-parsing** evidence branches
  (`codex/acorn-dfs-final-integration`, `feature/acorn-dfs-evidence`,
  `feature/acorn-dfs-modernization`, `integration/acorn-dfs-final`,
  `integration/acorn-dfs-onto-tape`, `integration/acorn-dfs-promotion`), none of which touch
  `crates/archivefs-core/src/launch/`.
- `crates/archivefs-core/src/launch/` contains no `bbc_*`, `beebem_*`, or `b_em_*` files. No
  dedicated BBC Micro native adapter exists today, confirming the prior coverage audit's
  classification.
- No production code changes were required to perform this audit — it is documentation only.
- Shared launch-registration files (`launch/mod.rs`, `launch/integration.rs`,
  `launch/readiness.rs`, `launch/platform_map.rs`, `patch_manager/mod.rs`, GUI adapter
  registration) were **not modified**. Several of them appeared as uncommitted changes belonging
  to other concurrent sessions (openMSX execution seam, BSFree work); this audit did not touch,
  stage, or depend on any of that in-flight work.

**Verdict: no adapter exists, nothing collides, audit-only work can proceed.**

---

## Task A — Local Emulator Inventory (read-only, nothing installed)

| Check | Result |
|---|---|
| `command -v b-em` | NOT FOUND |
| `command -v bem` | NOT FOUND |
| `command -v beebem` | NOT FOUND |
| `command -v BeebEm` | NOT FOUND |
| Desktop entries / AppImage / Flatpak for any of the above | NOT FOUND |
| `mame` (full emulator binary) | NOT FOUND (only `mame-tools`, a utility package, is installed — not the emulator) |
| Any BBC/Acorn-specific firmware ROM file on this host (e.g. under `~/.config/retroarch/system`) | NOT FOUND — that directory contains ROMs for other platforms (MSX `basic20.rom`/`basic21.rom`, Videoton TVC `tvc_dos12d.rom`) but nothing named or shaped like a BBC OS/BASIC ROM |

No software was installed to perform this audit.

---

## Task B — Candidate Emulators

Three genuinely maintained Linux-capable candidates were identified and researched from
authoritative upstream sources (project READMEs, official docs, GitHub activity):

| Criterion | **b-em** (`stardot/b-em`) | BeebEm for UNIX (`stardot/beebem`) | b2 (`tom-seddon/b2`) |
|---|---|---|---|
| Linux support | Yes — native Linux/Allegro build, actively built alongside Win32 | Yes — dedicated UNIX/X11 port | Yes — via community Snap; also builds from source |
| Maintenance status | Active (2025 commits, e.g. Music 5000 sound chip support) | Minimal — only ~14 commits total on the UNIX port's repo; effectively legacy | Active, modern GPLv3 C++/Assembly codebase |
| Deterministic CLI | Yes — documented positional args + explicit flags (`-mX`, `-tX`, `-u`, `-i`, `-c`, `-fX`, `-fasttape`, `-spX`) | Minimal — bare filename autoload only; no model-select flag on the UNIX port (contrast: the separate Windows port has `-Model 0-3`) | Yes — explicit flags (`-0`, `-1`, `--0-direct`, `--1-direct`, `-b`, `-c`, `--vsync`, `--timer`) |
| Direct media attachment | Yes — bare positional arg for disc **or** tape image; `-u name.uef` for explicit tape | Yes — bare positional arg only | Yes — `-0 FILE` / `-1 FILE` for drive 0/1 |
| Machine selection | Yes — 22 explicit `-m` model codes | No documented flag on UNIX port | Yes — curated list: BBC B, B+, Master 128, Master Compact/Olivetti PC 128 S |
| Firmware handling | Expects a user-supplied ROM directory; no bundled default ROMs confirmed | Expects a user-supplied ROM directory | Ships its own default ROM set — simplest firmware story of the three |
| Process/watched-execution suitability | Good — deterministic argv, no shell needed | Weak — thin CLI gives little control, unclear exit/process semantics from docs alone | Good — deterministic argv |
| Configuration dependence | Moderate — machine/ROM paths set via `-c` config or flags | High — UNIX port leans on its own config file/GUI for most settings | Moderate — `-c` config plus explicit overrides |
| Disc/tape support | **Both** disc (SSD/DSD/ADF/ADL) and tape (UEF/CSW) via CLI | Disc only (tape not documented in the UNIX port CLI) | **Disc only** — tape explicitly unsupported |
| Write-protection story | Not documented in README; no explicit read-only CLI flag found | **Best documented**: "Discs are write protected when loaded to prevent any accidental data loss" (verbatim, official UNIX README), toggleable per-drive | **Best mechanism**: default disc mode is in-memory and never touches the source file unless the user explicitly saves; only opt-in `--X-direct` writes live |

Two conflicting third-party sources were found for b-em's CLI (a CyberITHub article claiming
`-disc`/`-disc1`/`-autoboot`/`-tape`/`-s` flags). These flags do **not** appear in b-em's own
`README.md`, which was fetched directly from the upstream repository twice for verbatim accuracy.
Per this task's "do not invent flags" instruction, the third-party claims are treated as
**unverified and likely inaccurate** (possibly describing a different fork or version), and are
excluded from the CLI contract below. Only the primary-source README is treated as ground truth.

### Chosen emulator: **b-em**

b-em is the only candidate that combines (a) active 2025 maintenance, (b) a genuinely
deterministic, documented CLI, and (c) native support for **both** disc and tape media in one
process. Given EmuWiz already has substantial tape-side investment (`bbc_tape.rs`, `uef_tape.rs`),
an emulator that cannot load tape images at all (b2) is a poor primary fit despite its excellent
write-protection model, and BeebEm for UNIX's CLI is too thin and its maintenance too weak to
serve as a general adapter target.

b2's in-memory-vs-direct write model and BeebEm's default disc write-protection are both retained
as reference points for Task G (write-protection design), since **b-em's own README documents no
equivalent read-only flag** — this is a real gap the future adapter must design around, not
assume away.

---

## Task C — Machine Coverage

EmuWiz's existing platform taxonomy (`crates/archivefs-core/src/platform/mod.rs`,
`platform/detect.rs`, `platform/tests.rs`, `platform_artwork.rs`) already models **"BBC Micro"**
and **"Acorn Electron"** as two distinct, separately-registered platforms — not merged. This audit
does not change that.

b-em's own documented `-m` model list (verbatim, all 22 entries) covers only BBC-family and
ARM-Evaluation-System machines: Model A, Model B (multiple sideways-RAM variants), B+64/128,
Master 128 (MOS 3.20 **and** 3.50 separately), Master 512, Master Turbo, Master Compact, and the
ARM Evaluation System. **b-em has no Acorn Electron model.** This was independently corroborated by
a second source (web search on b-em/Electron support), which found no mention of Electron support
anywhere in b-em's documentation or feature list.

This is a genuine, upstream limitation, not an EmuWiz gap: BBC-family emulators and Electron
emulators are historically separate projects (a dedicated Electron emulator, e.g. Elkulator, is a
different codebase entirely). It **confirms** rather than contradicts EmuWiz's existing decision to
keep "BBC Micro" and "Acorn Electron" as separate platforms — they should also be served by
separate native adapters, not one.

**Recommendation:**
- V1 scope is the **BBC Micro family only** (b-em's native scope). Acorn Electron is explicitly
  **out of scope** for a b-em-based adapter; a future Electron-specific audit (e.g. evaluating
  Elkulator) would be a separate task.
- Within the BBC family, V1 should model only the two machine identities that matter for real
  disk/DAT identity and that most existing evidence and DAT/platform mappings target: **BBC Model
  B** and **Master 128**. The other ~20 `-m` codes (B+ variants, Master 512/Turbo/Compact, ARM
  Eval System, alternate MOS revisions) are legitimate but comparatively rare hobbyist
  configurations; modeling all of them in V1 would violate Task I's "avoid over-modeling hobbyist
  configurations" instruction. They remain selectable later via an expanded profile list without
  any architectural change.
- No filename-based machine guessing is proposed anywhere in this design — machine choice is
  always an explicit profile field (see Task I).

---

## Task D — Media Formats

| Format | Emulator support (b-em) | EmuWiz structural evidence | Classification |
|---|---|---|---|
| SSD / DSD (DFS) | Yes — bare positional disc arg | Yes — mature structural parser, `disk_format/dfs.rs`, maturity 6 in `docs/MEDIA_SUPPORT_AUDIT.md` | **SAFE FOR V1** |
| UEF (tape) | Yes — bare positional arg or explicit `-u name.uef` | Yes — bounded gzip-chunk container parser, `uef_tape.rs` (`UEF_HEADER`, `MAX_UEF_OUTPUT`/`MAX_UEF_CHUNKS`/`MAX_UEF_TEXT` bounds), plus a `TapeFormat::BbcUef` type-system variant already wired in the GUI | **SAFE FOR V1** — this is the strongest-evidenced format of the whole set, and matches the emulator's own native tape input exactly |
| ADF / ADL (ADFS) | Yes — b-em accepts these as disc images | No — `docs/MEDIA_SUPPORT_AUDIT.md`'s own DFS row lists "ADFS parser" as a remaining gap; no ADFS structural parser exists. `.adf` also collides with the unrelated Amiga floppy format (`platform/mod.rs` documents this extension collision explicitly) | **NEEDS STRONGER IDENTITY** — defer until an ADFS structural parser exists; extension alone is untrustworthy per the platform layer's own documented collision |
| IMG | Yes (generic container b-em can sometimes load) | No BBC-specific structural evidence; `.img` is a widely shared, ambiguous extension across many platforms | **AMBIGUOUS — DEFER** |
| CSW (tape) | Yes — b-em accepts CSW as a tape image | No — the only CSW reference anywhere in `crates/archivefs-core/src` is a generic comment about RLE/CSW payload framing (`tape_identity.rs`), unrelated to BBC and with no BBC-specific CSW parser | **AMBIGUOUS — DEFER** |
| WAV (cassette audio) | No — b-em's CLI takes UEF/CSW/disc images, not raw WAV directly | Yes — `bbc_tape.rs` does full structural standard/custom cassette WAV analysis, maturity 5, `Identify=YES` (machine-exact, unlike the FAMILY-level DFS/UEF rows) | **SAFE FOR V1 as detection/identity evidence only** — not usable as a direct b-em launch input without a conversion step, which is out of scope for a safe, non-mutating launch adapter |
| ROM (sideways/cartridge) | N/A as a per-title launch media type — sideways ROMs are a machine/firmware configuration concern, not swappable per-title media | No per-title ROM-media evidence found; this is a profile/firmware concern (Task I), not a media-attachment concern | **DEFER / NOT APPLICABLE to V1 media scope** |
| Generic tape images (other) | Varies | None found | **DEFER** |

This directly answers the task's explicit UEF focus: UEF is not just safe, it is the
**best-evidenced** format for a BBC adapter — EmuWiz already has a bounded parser, a wired
`TapeFormat` variant, and b-em accepts UEF natively with no conversion.

---

## Task E — Existing EmuWiz Evidence

1. **Can BBC tape media already be recognized?** Yes, at two levels: `uef_tape.rs` recognizes UEF
   containers structurally (family-level: reports `platform: Some("BBC Micro / Acorn Electron")`),
   and `bbc_tape.rs` recognizes standard/custom cassette WAV audio with machine-exact
   (`Identify=YES`) confidence.
2. **Can disk media already be structurally distinguished?** Partially. `disk_format/dfs.rs`
   structurally parses DFS SSD/DSD catalogues (`SECOND_DSD_CATALOGUE_OFFSET = 0x0a00`), but the
   parser's own documentation states plainly: *"DFS is shared by BBC Micro/BBC Master and Acorn
   Electron; the structure does not identify one machine."* `platform/detect.rs` mirrors this,
   returning `["BBC Micro", "Acorn Electron"]` together as an explicit ambiguity set rather than
   guessing one.
3. **Are SSD/DSD sufficiently distinctive?** For *filing-system family* (DFS), yes. For *exact
   machine* (BBC vs Electron, or Model B vs Master 128), **no** — corroborating evidence or an
   explicit user choice is required.
4. **Are there current DAT/platform mappings?** Yes — `docs/ESDE_BATCH4_MAPPING_PLAN.md` and the
   platform registry (`platform/mod.rs`) already list "BBC Micro" and "Acorn Electron" as separate,
   verified platform identities with their own artwork keys (`platform_artwork.rs`).
5. **Where would explicit user profile selection still be required?** At minimum: (a) BBC family vs
   Electron (structurally ambiguous from DFS alone), and (b) exact machine within the BBC family
   (Model B vs Master 128 vs the rarer variants) — DFS/UEF evidence alone cannot resolve either.
   This is exactly why Task I specifies an explicit profile model rather than auto-detection.

No parsers were added or modified to answer this — all evidence above is pre-existing.

---

## Task F — CLI Contract (b-em, from upstream `README.md`, verbatim flag names)

```
b-em [options] [discimage|tapeimage|snapshot]
```

| Purpose | Flag / form | Notes |
|---|---|---|
| Executable invocation | `b-em` | Bare invocation launches the configured default machine with no media |
| Load disc or tape image | positional argument, e.g. `b-em game.ssd` / `b-em game.uef` | b-em infers disc-vs-tape from the file's own structure/extension; no separate drive-0 flag is documented for the *positional* form |
| Explicit tape attachment | `-u name.uef` | Documented explicit alternative to the positional form for UEF tape |
| Machine selection | `-mX` where `X` is one of 22 documented model codes (0–21), e.g. Model B, B+, Master 128 MOS 3.20, Master 128 MOS 3.50, Master Compact, ARM Evaluation System | Exact code table is in the upstream README; V1 only needs the codes for Model B and Master 128 |
| Tube/second-processor selection | `-tX` | Out of scope for V1 (Task J) |
| Fullscreen/windowed | not found as a distinct documented flag in the primary README | Needs a follow-up check against the in-app config screen before implementation — **not invented here** |
| Config/profile | `-c` | Selects a named configuration; exact semantics not fully detailed in the README excerpt reviewed — flag exists, exact behavior would need confirmation before use |
| Firmware/ROM path | not a CLI flag — set via the ROM directory referenced by config | b-em expects a user-populated ROM directory; no CLI override flag documented |
| Fast tape loading | `-fasttape` | Optional; not required for V1 |
| Sound/other options | `-fX` (sound-related), `-spX` (speed-related), `-i` (fullscreen? — unclear from excerpt, needs confirmation) | Not required for V1's safe minimal contract |

**No second floppy drive (drive 1) flag is documented anywhere in the primary README excerpt
reviewed.** This is treated as **unsupported/unconfirmed** rather than invented — V1 must not
claim drive-1 support. Two flags (`-i` and the exact `-c` config semantics) need direct
confirmation against the upstream README's full text or `--help` output before an actual adapter
is implemented; they are listed here as open items, not fabricated.

**Third-party claims explicitly rejected:** `-disc`, `-disc1`, `-autoboot`, `-tape`, `-s` (from a
CyberITHub article) do not appear in the primary-source README and must not be used.

---

## Task G — Write-Protection Risk

b-em's own README documents **no explicit read-only/write-protect CLI flag**. This is a real,
confirmed gap (not an assumption) — it was checked directly against the primary source, not
inferred. Left unmitigated, a BBC disk emulator can write back to whatever image file it has open
when a `*SAVE` or `*BACKUP` command runs on the emulated machine — this is standard behavior for
this whole emulator family (BeebEm's UNIX README explicitly documents write-protection as an
opt-out safety feature specifically because writes are the *default* otherwise). b-em may also
rewrite its own config file and could plausibly persist machine state; none of this was confirmed
either way in the README, so it is marked **UNKNOWN, must be assumed possible** for safety
purposes.

**Recommended mitigation for a future adapter (not implemented here):**
1. **Never open the user's source media file directly.** Copy the source SSD/DSD/UEF into a
   private, adapter-owned scratch location before invoking b-em, and always pass b-em the scratch
   copy's path.
2. Discard the scratch copy after the emulator process exits — do not attempt to detect or import
   "changes" back into the user's library.
3. If a future version of b-em is confirmed (via direct testing, not assumption) to expose a
   read-only/write-protect mechanism equivalent to BeebEm's, prefer that as an additional layer
   over the temp-copy strategy, not a replacement for it.
4. If neither a confirmed read-only flag nor a safe temp-copy path is available in some future
   constrained environment, the adapter must **refuse to launch** rather than risk mutating source
   media — this task's own stated fallback.

This mirrors b2's proven in-memory-vs-direct design pattern and BeebEm's default write-protection,
even though the chosen emulator (b-em) does not itself document either mechanism.

---

## Task H — Firmware / ROM Requirements

No BBC/Acorn OS or BASIC ROM files were found anywhere on this host (Task A). No firmware was
downloaded or fabricated as part of this audit.

| Requirement | Classification | Notes |
|---|---|---|
| OS ROM (machine operating system) | **MISSING** | Not present on this host; b-em expects a user-supplied ROM directory |
| BASIC ROM | **MISSING** | Same as above |
| DFS ROM | **UNKNOWN** | Not verified whether bundled with any local install (none installed) |
| ADFS ROM | **NOT_REQUIRED for V1** | ADFS is explicitly deferred (Task D); no ADFS support is proposed for V1 |
| Sideways ROMs (general) | **NOT_REQUIRED for V1** | Deferred per Task D/I; not part of the minimal media-attachment scope |
| Master 128 ROM set | **MISSING** | Distinct from the base BBC ROM set; not present on this host |
| Electron ROM requirements | **NOT_REQUIRED** | Electron is out of scope for this adapter (Task C) |

No hashes were fabricated. A future implementation must perform an existence/readiness check
against a user-configured ROM directory (Task I's "firmware set" profile field) and refuse to
launch with a clear error if the configured ROMs are absent — it must not attempt to acquire them.

---

## Task I — Config/Profile Model

A future adapter needs an explicit, minimal profile — not auto-detected, not filename-guessed:

| Field | Purpose | V1 scope |
|---|---|---|
| `machine` | b-em `-m` model code | Limited to Model B and Master 128 for V1 (Task C) |
| `filing_system` | DFS vs ADFS | DFS only for V1 (ADFS deferred, Task D) |
| `firmware_set` | Path/identifier for the configured ROM directory | Required; existence-checked only (Task H), not hash-verified in V1 |
| `emulator_executable` | Resolved path to `b-em` | Standard adapter pattern already used by other native adapters (e.g. `openmsx_command.rs`) |
| Expansion/tube settings | e.g. second processor | **Excluded from V1** — explicitly named as an over-modeling risk in the task prompt |

This keeps the profile shape consistent with the "BBC Model B + DFS" / "Master 128" examples given
in the task prompt, while deriving the actual field list from the evidence gathered rather than
assuming the prompt's example is complete.

---

## Task J — Safe V1 Scope

**Recommended V1:**
- Emulator: b-em
- Machines: BBC Model B, Master 128 (explicit profile selection only)
- Media: DFS SSD/DSD (existing structural evidence) and UEF tape (existing structural evidence) —
  both already have real EmuWiz-side identity work behind them
- Launch flow: typed argv built only from the confirmed flags in Task F, with the drive-0
  positional/`-u` form and `-m` machine selection; no drive-1, no tube selection, no fullscreen
  toggle until those flags are independently confirmed against the full upstream `--help` output
- Media safety: mandatory scratch-copy strategy per Task G before every launch
- Firmware: existence-checked ROM directory per profile; launch refused if absent

**V1 should explicitly NOT support:**
- Acorn Electron (no b-em support at all — Task C)
- ADFS / `.adf` / `.adl` (no structural parser, extension collision with Amiga — Task D)
- CSW tape (no EmuWiz-side identity work — Task D)
- Raw WAV as a direct b-em launch input (b-em doesn't accept it; conversion is out of scope)
- Sideways ROM/cartridge management
- A second floppy drive (undocumented in b-em's CLI)
- Master 512 / Master Turbo / Master Compact / B+ variants / ARM Evaluation System (real but rare;
  deferred to a later profile expansion, not a V1 blocker)
- Any filename-based machine guessing

---

## Task K — Test Plan (design only, nothing implemented)

1. Executable discovery — `b-em` present/absent on `PATH`
2. Explicit machine profile required — launch refused without one
3. Valid BBC disk (DFS SSD) launches with correct `-m`/media argv
4. Valid BBC tape (UEF) launches with correct `-u` argv
5. Wrong platform media (e.g. a ZX Spectrum TAP) is refused, not silently loaded
6. Ambiguous media (DFS disk with no machine/family disambiguation) is refused pending explicit profile choice
7. Missing firmware — launch refused with a clear "ROM directory not found" error, no fabricated ROM
8. Incompatible firmware/profile (e.g. Master-only ROM set selected with a Model B profile) is rejected before launch
9. Exact typed argv — assert the constructed argument vector matches Task F's documented flags exactly, byte-for-byte
10. Read-only source guarantee — assert the original source file's mtime/hash is unchanged after a full emulator run
11. Executable drift — b-em binary hash/version changes between preflight and launch is detected and refused
12. Content drift — source media file changes between selection and launch is detected and refused
13. Profile drift — profile changes mid-flow (e.g. machine changed after firmware check) invalidates the cached readiness result
14. No shell execution — process is spawned directly (argv vector), never through a shell string
15. No auto machine guessing — a test asserting that an ambiguous DFS image never silently resolves to a machine without an explicit profile
16. RetroArch alternative preserved — the existing generic RetroArch path for BBC Micro remains available and unaffected by adding a native adapter

---

## Task L — Rank Next Implementation

| Candidate | Value | Impl. complexity | Identity readiness | CLI quality | Firmware complexity | Collision risk |
|---|---|---|---|---|---|---|
| Atari800 (Atari 8-bit) | High — already P2 in the prior coverage audit | Low-medium | Good (per prior audit) | Good | Low-medium | **Currently blocked** — deferred specifically because openMSX owns the shared launch files |
| BBC Micro (b-em, this audit) | Medium-high — two well-evidenced media formats (DFS, UEF) already exist | Medium — write-protection gap (Task G) and two unconfirmed CLI flags (Task F) add real work | Good for DFS/UEF at family level; machine-exact selection still needs an explicit profile | Good, but two flags need direct confirmation before implementation | Medium — no local firmware, ROM directory model must be built | Low today, but same shared-launch-file collision risk as any new adapter once implementation starts |
| NP2kai (PC-98) | Medium | Unknown — no native adapter audit done yet | Partial — boot evidence exists (`a2d51bb`, `3ae28aa`) but that's identity, not launch | Unknown | Unknown | Low today |
| X68000 native adapter | Medium | Unknown — only Human68k disk evidence exists (`66c5695`), no CLI audit done | Partial | Unknown | Unknown | Low today |
| Amstrad CPC native adapter | Medium | Unknown — evaluated only as a name in the prior coverage audit, no dedicated audit performed | Unknown | Unknown | Unknown | Low today |
| XRoar extension beyond CAS | Low for BBC purposes — XRoar is a Dragon/Tandy CoCo emulator, unrelated to BBC Micro; included here only as a same-family launch-adapter comparison point since it was just implemented (`51cd8a8`) | N/A (already shipped for its own platform) | N/A | N/A | N/A | N/A |

**Verdict:** BBC Micro is a credible, well-evidenced next candidate — arguably the best-evidenced
of the group given the existing DFS and UEF work — but it is **not automatically next**. The
authoritative sequencing from the prior coverage audit still holds: **P1 is finishing the openMSX
execution/preflight seam**, and **P2 is Atari800**, which is currently blocked on the same shared
launch-file ownership issue this task was explicitly designed to avoid. Once openMSX clears, BBC
Micro (via b-em) is a strong candidate for the next *new* native adapter slot — competitive with or
ahead of NP2kai, X68000, and Amstrad CPC on identity readiness — but this audit does not force that
conclusion; PC-98, X68000, and Amstrad CPC have not yet had equivalent audits performed, so a
confident final ranking between them and BBC cannot be made from evidence gathered in this task
alone.

---

## Summary

- No BBC Micro native adapter existed before or exists after this audit.
- Chosen emulator: **b-em**, for its unique combination of active maintenance, disc+tape CLI
  support, and alignment with EmuWiz's existing DFS/UEF evidence investment.
- Safe media for V1: DFS SSD/DSD and UEF tape. ADF/ADL, IMG, CSW, and raw WAV-as-launch-input are
  deferred pending stronger identity work.
- Acorn Electron is explicitly out of scope — b-em does not support it, and this reinforces
  EmuWiz's existing decision to keep BBC Micro and Acorn Electron as separate platforms.
- The single largest open risk is write-protection: b-em documents no read-only flag, so a future
  adapter must use a mandatory scratch-copy strategy.
- Firmware is entirely absent locally and must be a runtime readiness check, never a download.
- BBC Micro is a strong but not automatically "first" candidate for the next native adapter slot;
  the existing P1 (openMSX)/P2 (Atari800) sequencing is unchanged by this audit.

**No production code was modified. No shared launch files were touched. No new BBC parser was
written. No extension-only identity was proposed. No firmware was downloaded. No user media was
modified. No openMSX files were touched. No Atari800 files were touched.**
