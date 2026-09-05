# ES-DE Final Gap Decisions

## Executive summary

This document renders one final, decision-ready classification for every
canonical EmuWiz platform still unresolved after ES-DE mapping Batch 5
(commit `298137d`, "feat(esde): expand platform mapping parity batch 5"),
which brought live parity to **68 COMPLETE / 1 PARTIAL (MegaDrive) / 7
MISSING** out of 76 canonical platforms.

Re-verifying the prior classifications in `docs/ESDE_FINAL_PARITY_PLAN.md`
against current EmuWiz code (`crates/archivefs-core/src/platform/mod.rs`,
`crates/archivefs-core/src/platform/identity.rs`,
`crates/archivefs-core/src/launch/es_de_export.rs`) and the current upstream
ES-DE user guide (`gitlab.com/es-de/emulationstation-de` `master` branch,
`USERGUIDE.md`, fetched 2026-09-05) produces **one material revision**:

- **TurboGrafx-16 is reclassified from `POLICY_REQUIRED` to `SAFE_MAPPING`.**
  Upstream ES-DE genuinely ships `tg16` as its own distinct system folder,
  separate from `pcengine` — this is not a duplicate-target collision, and
  no region-guessing is involved because EmuWiz already keeps `PC Engine`
  and `TurboGrafx-16` as two statically-assigned, never-merged canonical
  platforms. The "requires policy" framing in the prior plan was overly
  cautious; the exact 1:1 evidence was already there.

Everything else confirms the prior plan's evidence with one structural
clarification for PC-98/NEC PC-9801 (a concrete, non-destructive cleanup
mechanism is now specified) and firmer, code-sourced identity evidence for
NeoGeo64, Commodore 128, MegaDrive, Atari 8-bit, and PC.

**Final classes assigned in this pass:**

| Platform | Final class |
| --- | --- |
| MegaDrive | `INTENTIONALLY_PARTIAL` (already implemented; confirmed correct) |
| TurboGrafx-16 | `SAFE_MAPPING` (revised — safe to implement) |
| PC-98 / NEC PC-9801 | `CANONICAL_CLEANUP_REQUIRED` (confirmed; exact cleanup specified below) |
| Atari 8-bit | `DEFER_V1` (exact recommendation given; needs explicit product sign-off) |
| Commodore 128 | `NO_ESDE_TARGET` (confirmed) |
| NeoGeo64 | `NO_ESDE_TARGET` (confirmed) |
| PC | `DEFER_V1` (confirmed; only plausible target uses an incompatible export shape) |

No canonical platform ID is renamed, merged, or deleted by this document.
No code, tests, or Cargo commands were touched to produce it.

## Current parity baseline

Source: `crates/archivefs-core/src/platform/mod.rs` (`PLATFORMS`, 76 entries)
and `crates/archivefs-core/src/launch/es_de_export.rs` (`ES_DE_SYSTEM_MAP`,
69 real platform rows after Batch 5), cross-checked against
`docs/ESDE_FINAL_PARITY_PLAN.md` §1/§7 and the live test
`platforms_still_unmapped_after_batch_5_remain_refused`.

| State | Count |
| --- | ---: |
| Canonical platforms | 76 |
| COMPLETE | 68 |
| PARTIAL | 1 (MegaDrive) |
| MISSING | 7 |

The 7 MISSING: Atari 8-bit, Commodore 128, NEC PC-9801, NeoGeo64, PC, PC-98,
TurboGrafx-16.

## Decision matrix

| EMUWiZ PLATFORM | CURRENT CLASS | ES-DE TARGET | EVIDENCE | FINAL CLASS | V1 ACTION |
| --- | --- | --- | --- | --- | --- |
| MegaDrive | PARTIAL (implemented) | `megadrive` (Sega Mega Drive) | `es_de_export.rs` lines 249-253; upstream also ships `genesis`/`megadrivejp` as regional siblings | INTENTIONALLY_PARTIAL | None — retain as-is; this document is the formal record of the policy |
| Atari 8-bit | POLICY_REQUIRED | none mapped | `platform/mod.rs` lines 665-684 (one canonical id, aliases span `atari800`+`atarixe`); upstream ships `atari800`/Atari 800 and `atarixe`/Atari XE as two distinct folders, both via the Atari800 emulator (USERGUIDE.md rows, fetched 2026-09-05) | DEFER_V1 | Do not map yet; bring `atari800`-as-default to product sign-off (see §Atari 8-bit) |
| TurboGrafx-16 | POLICY_REQUIRED | `tg16` (NEC TurboGrafx-16) | `platform/mod.rs` lines 1749-1760 (distinct canonical id, `conflicts_with: ["PC Engine"]`, `EQUIVALENT_PLATFORM_IDS` relates it to `PC Engine` for comparison only); upstream ships `tg16` as its own folder distinct from `pcengine`, both via Beetle PCE/Geargrafx (USERGUIDE.md, fetched 2026-09-05) | SAFE_MAPPING | Add one `ES_DE_SYSTEM_MAP` row: `"TurboGrafx-16" -> "tg16" / "NEC TurboGrafx-16"` |
| PC-98 | CANONICAL_CLEANUP_REQUIRED | `pc98` (NEC PC-9800 Series) | `platform/mod.rs` lines 988-999; upstream ships exactly one `pc98` folder for the whole PC-9800 series (USERGUIDE.md, fetched 2026-09-05) | CANONICAL_CLEANUP_REQUIRED | Canonical survivor for the `pc98` export target — see §PC-98/NEC PC-9801 |
| NEC PC-9801 | CANONICAL_CLEANUP_REQUIRED | `pc98` (shared, not a second target) | `platform/mod.rs` lines 1000-1012 (`EQUIVALENT_PLATFORM_IDS` already relates it to `PC-98`); same single upstream `pc98` folder | CANONICAL_CLEANUP_REQUIRED | Resolve via equivalence fallback at export time, not a second table row — see §PC-98/NEC PC-9801 |
| Commodore 128 | NO_ESDE_TARGET | none exists | `platform/mod.rs` lines 790-801; upstream has no `c128`/Commodore 128 system — only `c64`'s VICE offers an "x128" *emulator command*, and that system remains Commodore 64 (USERGUIDE.md line ~4899, fetched 2026-09-05) | NO_ESDE_TARGET | None — remain unsupported |
| NeoGeo64 | NO_ESDE_TARGET | none exists | `platform/mod.rs` lines 1033-1044 (`display_name: "Neo Geo 64"`, folder evidence only, distinct from `neogeo`/`neogeocd`); upstream has no `neogeo64`/`hng64`/"Hyper Neo Geo 64" system anywhere in the systems table (verified by direct search of USERGUIDE.md, fetched 2026-09-05) | NO_ESDE_TARGET | None — remain unsupported |
| PC | DEFER | `pc` exists but is semantically IBM-PC/DOSBox, not modern Windows | `platform/mod.rs` lines 1287-1298 (`folder_aliases`: pc, pcgames, windows, windowsgames — broad modern-desktop identity); upstream `pc`/IBM PC row is DOSBox-driven and textually near-identical to `dos`/DOS (PC) (USERGUIDE.md rows, fetched 2026-09-05); the only semantically-plausible target, upstream `windows`/Microsoft Windows, is a shortcut/`.lnk`/AppImage launcher system, not a ROM/gamelist system | DEFER_V1 | None — no compatible export shape exists yet for a shortcut-based system |

## MegaDrive

**EmuWiz canonical identity:** `MegaDrive` (`platform/mod.rs` lines 1516-1542),
`display_name: "Sega Mega Drive / Genesis"`. `folder_aliases`: `megadrive,
genesis, segamegadrive, segagenesis, segamegadrivegenesis, smd, segamd,
megadrivegenesis, md`. One canonical identity covers both Western ("Genesis")
and Japanese/PAL ("Mega Drive") branding.

**Existing ES-DE mapping:** `es_de_export.rs` lines 249-253 — already maps to
`megadrive` / "Sega Mega Drive". Upstream (USERGUIDE.md, fetched 2026-09-05)
confirms three real, distinct regional folders exist: `genesis` (Sega
Genesis), `megadrive` (Sega Mega Drive), `megadrivejp` (Sega Mega Drive
[Japan]) — all driven by the same emulator set (Genesis Plus GX and others),
differing only in ROM naming convention/region.

**Is the mapping exact?** No — it is a many-to-one collapse by design: one
EmuWiz identity, three real upstream folders.

**Would it be lossy/ambiguous to pick one automatically per game?** Yes.
Region alone (even when verified via DAT/RomM evidence) does not reliably
predict which *folder* a user wants their existing library organized under,
and EmuWiz's own `PlatformIdentityEvidence`/`PlatformIdentityResolution`
model (`platform/identity.rs`) resolves *platform identity*, not *export
folder preference* — conflating the two would require new persisted state
that does not exist today.

**Explicit answers:**

- **Should EmuWiz retain `megadrive` as its deterministic global default?**
  Yes. It already does (`es_de_export.rs` lines 249-253), and this matches
  the existing folder most EmuWiz MegaDrive libraries already use.
- **Does region identity provide enough evidence to choose `genesis` or
  `megadrivejp` safely?** No. Region evidence (from DAT/RomM/manual sources)
  answers "what region is this game," not "which frontend folder does this
  user's library use" — those are different questions, and no per-user
  folder preference is currently persisted anywhere.
- **Would automatic region-specific export create instability?** Yes. A
  single mixed-region collection would non-deterministically split across
  three destination folders depending on per-game evidence quality, and a
  later re-scan with different/updated region evidence could silently move
  an already-published game to a different folder — an unannounced,
  non-idempotent republication the existing recovery/idempotency contracts
  are not designed to tolerate.
- **Should MegaDrive remain formally PARTIAL for V1 even though `megadrive`
  works?** Yes. PARTIAL is the correct signal: the mapping *works* and is the
  right default, but it is a policy choice covering one of three real
  upstream identities, not a complete 1:1 correspondence. Marking it
  COMPLETE would hide that a deliberate, documented choice was made.

**Final class:** `INTENTIONALLY_PARTIAL`. **V1 action:** none — already
correctly implemented; this document formalizes the policy so it is never
re-litigated as an oversight. Any future per-user region/export preference
is out of scope and would need its own settings, migration, and
republication design (per `ESDE_FINAL_PARITY_PLAN.md` §5 option C/D, which
remain correctly rejected for now).

## Atari 8-bit

**EmuWiz canonical identity:** `Atari 8-bit` (`platform/mod.rs` lines
665-684). `folder_aliases`: `atari8bit, atari800, atari8, atarixl, atarixe,
atari400, atari130xe, atarixegs`. This is a single canonical identity
spanning the entire 8-bit computer family (Atari 400/800/XL/XE/XEGS) — not
Atari 2600 (separate `id: "Atari2600"`), 5200 (`id: "Atari5200"`), 7800
(`id: "Atari7800"`), or Atari ST (`id: "AtariST"`), all of which are their own
distinct canonical platforms already correctly separated in the registry.

**Upstream ES-DE targets:** confirmed via USERGUIDE.md (fetched 2026-09-05):
`atari800` / "Atari 800" and `atarixe` / "Atari XE" — two distinct folders,
both driven by the same Atari800 emulator (and Altirra on Windows). There is
no unified `atari8bit` system upstream; ES-DE splits by model exactly the
way EmuWiz's own aliases (`atari800` vs `atarixe`) already anticipate.

**Does one ES-DE target safely represent the canonical family?** Not
losslessly — this is structurally identical to the MegaDrive situation: one
EmuWiz identity, two real upstream folders. A deterministic global default
is possible using the exact same reasoning already accepted for MegaDrive
(pick one folder, document it as a policy choice, mark the result PARTIAL,
never guess per-game). Recommended default if/when approved: **`atari800`**
— it is the more general, foundational model name (matches the family's most
common target and the folder alias already present in EmuWiz's own
`atari800` alias), leaving `atarixe`-specific libraries to remain manually
exported until a further policy decision is made, exactly mirroring how
`megadrivejp`/`genesis` remain unaddressed today.

**Why this is not decided as INTENTIONALLY_PARTIAL outright:** unlike
MegaDrive, this specific default has never been implemented, reviewed, or
recorded as an approved product decision anywhere in the codebase or prior
planning docs (`ESDE_PARITY_AUDIT.md`, `ESDE_BATCH4_MAPPING_PLAN.md`,
`ESDE_FINAL_PARITY_PLAN.md` all defer it). Adopting a default silently in
this document would be inventing product policy rather than deciding a
technical mapping question. This document therefore hands off an exact,
actionable recommendation rather than asserting it unilaterally.

**Final class:** `DEFER_V1` — evidence is sufficient to fully specify the
change, but it requires explicit product sign-off (identical in kind to the
MegaDrive decision, just not yet made) before implementation.
**V1 action:** none. **Recommended follow-up:** approve `atari800` as the
deterministic default, implement identically to MegaDrive (one
`ES_DE_SYSTEM_MAP` row, marked PARTIAL, documented as a frontend default).

## TurboGrafx-16

**Relationship to PC Engine:** EmuWiz keeps two separate, never-merged
canonical platforms: `PC Engine` (`platform/mod.rs` lines 1300-1320, already
mapped to ES-DE `pcengine`, `es_de_export.rs` lines 431-434) and
`TurboGrafx-16` (`platform/mod.rs` lines 1749-1760, `conflicts_with: ["PC
Engine"]`). Both are explicitly related only via `EQUIVALENT_PLATFORM_IDS`
(line ~209) — a comparison relation, never a merge — because, per the
registry's own module doc (lines 10-17), "existing libraries already store
[TurboGrafx-16] separately from PC Engine," and merging would silently
rewrite `platform_assignments.platform` values users already have.

**Upstream ES-DE evidence:** confirmed directly from USERGUIDE.md (fetched
2026-09-05): `tg16` / "NEC TurboGrafx-16" is a real, distinct system folder,
separate from `pcengine` / "NEC PC Engine," both driven by the same
Beetle-PCE-family emulators. This is the exact same pattern ES-DE already
uses for `neogeo`/`neogeocd` and `genesis`/`megadrive` — a real hardware
family split into region-named folders, each independently addressable.

**Decision, using the four options posed:**

- **A. map to the same ES-DE target as PC Engine (`pcengine`)** — REJECTED.
  `pcengine` is already claimed by the `PC Engine` canonical platform; giving
  it to `TurboGrafx-16` too would violate the existing duplicate-ES-DE-target
  safety invariant enforced by `es_de_export.rs`'s own tests.
- **B. become a canonical alias of PC Engine** — REJECTED. This would be a
  merge, contradicting the registry's own explicit stated reason for keeping
  the two platforms separate (avoiding a silent rewrite of stored
  `platform_assignments` data).
- **C. remain distinct** — **ACCEPTED.** `TurboGrafx-16` already is a fully
  distinct, statically-assigned canonical platform. ES-DE independently
  provides an exact, non-colliding target (`tg16`) for exactly this
  distinction. No per-game region guessing is involved: which canonical
  platform a game is assigned is decided once, by existing identity
  resolution (`platform/identity.rs`), not inferred at export time.
- **D. removed/merged through canonical cleanup** — REJECTED, same reasoning
  as B.

This reclassifies the prior plan's `POLICY_REQUIRED` judgment: the "visible
regional publication decision" it worried about is not actually a policy
question at all, because EmuWiz never merges these platforms and the two
ES-DE folders are genuinely distinct, non-conflicting targets — it is a
plain 1:1 mapping, structurally identical to already-accepted rows like
`Neo Geo CD -> neogeocd`.

**Final class:** `SAFE_MAPPING`. **V1 action:** add one row to
`ES_DE_SYSTEM_MAP`: `"TurboGrafx-16" -> "tg16" / "NEC TurboGrafx-16"`. No
canonical, alias, or test changes beyond the existing table-driven pattern;
implement through the same seam Batches 1-5 used.

## PC-98 / NEC PC-9801

**Do they represent the same family under two names, or distinct scopes?**
Same family, two historically-distinct canonical IDs, deliberately never
merged. `PC-98` (`platform/mod.rs` lines 988-999, `display_name: "NEC
PC-98"`) is explained in code as "the equivalent modern spelling" of `NEC
PC-9801` (lines 1000-1012), which is "retained unchanged because existing
libraries already store this identifier." Both are related via
`EQUIVALENT_PLATFORM_IDS` (line ~209) for comparison only — this is not
accidental duplication, it is a deliberate, already-documented
storage-compatibility decision, matching the same pattern as PC
Engine/TurboGrafx-16.

**Upstream ES-DE evidence:** confirmed via USERGUIDE.md (fetched
2026-09-05): exactly **one** system, `pc98` / "NEC PC-9800 Series." Unlike
TurboGrafx-16/PC Engine, there is no second, region- or model-specific
folder for PC-9801 specifically — the whole series (PC-9801 through later
PC-9821 models) shares one upstream target.

**Why this is genuinely CANONICAL_CLEANUP_REQUIRED (unlike TurboGrafx-16):**
mapping both `PC-98` and `NEC PC-9801` to `pc98` directly would create a
duplicate-ES-DE-target row, which the export table's own invariants
(enforced by existing tests) refuse to allow. Exactly one canonical platform
must own the `pc98` export target.

**Exact recommended cleanup:**

- **Canonical survivor (owns the `pc98` `ES_DE_SYSTEM_MAP` row):** `PC-98`.
  It is already documented in its own code as the modern-spelling successor
  identity, and its `display_name`/upstream fullname ("NEC PC-98" / "NEC
  PC-9800 Series") align directly with ES-DE's own series-level naming.
- **Alias / non-owning side:** `NEC PC-9801`. It keeps its existing canonical
  ID exactly as-is in the registry — **no rename, no deletion.**
- **Migration behaviour:** none required for storage. The recommended
  mechanism is a read-only fallback at export-lookup time only: when
  `es_de_system_for_platform` finds no row for a resolved platform id, it
  should additionally consult the existing `equivalent_platform_ids()`
  relation and, if an equivalent id *does* have a row, use that row's target
  instead of failing closed. This changes zero stored data — a game already
  persisted as `platform_assignments.platform = "NEC PC-9801"` keeps that
  exact string forever; only the export *lookup* gains one extra, already-
  data-driven fallback step.
- **Compatibility implications:** none for existing libraries; this is
  strictly additive (a previously-`MISSING` platform becomes exportable) and
  reuses a relation (`EQUIVALENT_PLATFORM_IDS`) that already exists and is
  already tested for correctness, so no new alias-conflict surface is
  introduced.
- **Whether persisted IDs could be affected:** no. This is precisely the
  scenario the registry's own module doc (lines 10-17) was written to
  protect against, and the recommended fallback mechanism honors that intent
  exactly — it resolves *equivalence for export purposes*, never *identity
  for storage purposes*.

This is the one place in this document where a "genuinely missing seam" is
identified (the export lookup does not currently consult
`equivalent_platform_ids()`), consistent with the instruction to flag such
seams explicitly rather than invent one silently. It is a narrow,
additive, read-only lookup change — not a redesign of canonical identity,
alias handling, or the export table's shape.

**Final class:** `CANONICAL_CLEANUP_REQUIRED` for both `PC-98` and `NEC
PC-9801` (they are cleaned up together, as one decision). **V1 action:**
record the survivor decision now (this document); implementation (the one
`ES_DE_SYSTEM_MAP` row plus the equivalence-fallback lookup change) is a
follow-up batch, not this research pass.

## Commodore 128

**EmuWiz canonical identity:** `Commodore 128` (`platform/mod.rs` lines
790-801). `folder_aliases`: `commodore128, c128, commodorec128, c128d`.
`conflicts_with: ["Commodore 64"]` — already explicitly kept distinct from
C64 in the registry.

**Does ES-DE genuinely lack a specific C128 system target?** Yes, confirmed
directly against the current upstream systems table (USERGUIDE.md, fetched
2026-09-05): no `c128`/"Commodore 128" row exists. The `c64` row does list
"VICE x128" among its *emulator* options, but the **system** itself remains
"Commodore 64" — that emulator entry runs C128-compatible software inside
the C64 library folder, it does not create a separate C128 system.

**Would mapping to `c64`/`vice`/another Commodore bucket be semantically
false?** Yes. `Commodore 128` is a distinct 8-bit machine with its own BIOS
modes, memory map, and (per EmuWiz's own registry) its own strong
extensions (`d71`, `d81`) distinguishing it from C64 media (`d64`, `g64`,
etc.). Routing it into the `c64` ES-DE folder would merge two genuinely
different libraries under one system, purely because one shared emulator
happens to support both — exactly the "emulator-based equivalence" this
audit is instructed to avoid.

**Final class:** `NO_ESDE_TARGET`. **V1 action:** none — retain unsupported.
Revisit only if upstream ES-DE ever adds a dedicated `c128` system, or a
separately-reviewed product policy explicitly accepts routing C128 content
into the C64 folder (not recommended).

## NeoGeo64

**What does EmuWiz's `NeoGeo64` actually represent?** Confirmed via code:
`platform/mod.rs` lines 1033-1044, `id: "NeoGeo64"`, `display_name: "Neo Geo
64"`, `folder_aliases: ["neogeo64"]` only, no strong extensions, no magic
bytes, no layout rule — explanation field states "Recognised from folder
evidence only; its dumps use generic containers." This is SNK's **Hyper Neo
Geo 64** arcade board (a 1997 3D arcade system, entirely distinct hardware
from the 2D cartridge-based Neo Geo/MVS/AES line), not an erroneous label
for ordinary Neo Geo and not a legacy synonym — it is `conflicts_with: []`,
meaning the registry does not even consider it adjacent to `Neo Geo` or `Neo
Geo CD` (both of which are separate, already-mapped canonical platforms:
`neogeo`/`neogeocd`).

**Does current ES-DE have a matching system?** No. Directly searched the
current upstream systems table (USERGUIDE.md, fetched 2026-09-05) for
`neogeo64`, `hng64`, and "Hyper Neo Geo 64" in any form — no matching system
exists. ES-DE's `neogeo` system (SNK Neo Geo, `platform/mod.rs`'s
`neogeo`-equivalent canonical row, already mapped) is the ordinary
cartridge/MVS/AES system and is explicitly a different machine family.

**Confirmation this is not mapped to ordinary Neo Geo:** correct — and this
document explicitly does not recommend doing so. Hyper Neo Geo 64 titles
would not run correctly if filed under the `neogeo` ES-DE folder (different
emulator core entirely — arcade-driver-based, typically MAME's `hng64`
driver, not FinalBurn Neo/Geolith), and doing so purely for coverage would
misrepresent the library.

**Final class:** `NO_ESDE_TARGET`. **V1 action:** none — retain unsupported.
Revisit only if upstream ES-DE adds a dedicated system for this hardware.

## PC

**Why is PC currently DEFER?** EmuWiz's `PC` canonical platform
(`platform/mod.rs` lines 1287-1298) is a broad, folder-evidence-only
identity: `folder_aliases`: `pc, pcgames, windows, windowsgames`,
`weak_extensions: ["exe", "msi", "iso", "zip", "7z", "rar"]`, explanation:
"PC releases use entirely generic containers, so only folder evidence
identifies them." This is a **modern/generic Windows desktop games**
identity, deliberately separate from `DOS` (`platform/mod.rs` lines 933-947,
already mapped to ES-DE `dos`, requires a real parsed `dosbox.conf` to
qualify) — EmuWiz already avoids conflating DOS-era and modern-PC content at
the canonical level (`PC.conflicts_with: ["DOS"]`).

**Does it overlap with DOS / Windows / generic desktop / ScummVM /
emulator-specific PC platforms?** By design, no — it is EmuWiz's intentional
catch-all for the "everything else that is a modern PC game" case, distinct
from `DOS` and from `ScummVM` (its own separate canonical platform, not
audited here since it already has a distinct, already-resolved identity).

**Does a genuinely matching ES-DE target exist?** Checked directly
(USERGUIDE.md, fetched 2026-09-05): ES-DE does have a system literally named
`pc` ("IBM PC"), but its row is textually near-identical to ES-DE's own
`dos` ("DOS (PC)") row — same emulator list (DOSBox-Pure, DOSBox-X, DOSBox
Staging, DREAMM, VirtualXT), same "see the DOS/PC section" note. Upstream
treats `pc` as effectively a second folder-naming convention for the *same*
DOS-era content as `dos`, not as a modern-Windows-games bucket. Mapping
EmuWiz's `PC` there would collapse two semantically different libraries
(1990s DOS software vs. modern Windows executables) into one folder purely
because the names happen to match — precisely the "invented equivalence"
this audit is instructed to avoid.

The one upstream system that *is* semantically aligned with "modern desktop
Windows games" is `windows` / "Microsoft Windows" (found in the
platform-specific desktop-apps section of the guide, not the ROM-console
table) — but it is a **shortcut/launcher system**: its content forms are
`.desktop`/`.app`/`.lnk` shortcut files, scripts, or AppImages, not ROM files
matched against a `gamelist.xml` entry the way every other row in
`ES_DE_SYSTEM_MAP` works. Supporting it would require an entirely different
export shape (generating launcher shortcut files rather than ROM
references) — a genuinely different mechanism, out of scope for this
research pass and not something to bolt onto the existing exact-keyed,
ROM-oriented export table.

**Does generic `PC` belong in ES-DE export for V1?** No — not without either
(a) accepting a semantically false collapse into ES-DE's DOS-oriented `pc`
system, or (b) building a new, separate shortcut-file export mechanism for
the real `windows` system. Neither is appropriate to decide or build in this
research pass.

**Final class:** `DEFER_V1`. **V1 action:** none. Avoid the junk-drawer
mapping to ES-DE's `pc`. If modern-Windows-game export is ever prioritized,
scope it as new design work targeting ES-DE's `windows` shortcut system, not
as an extension of the current ROM-table export mechanism.

## Canonical cleanup risks

- **PC-98 / NEC PC-9801** is the only item in this document requiring an
  actual cleanup decision. The recommended mechanism (equivalence-fallback
  at export-lookup time) touches no persisted data and reuses an existing,
  already-tested relation (`EQUIVALENT_PLATFORM_IDS`); risk is low, provided
  the fallback is implemented as *lookup-only* and never as a rename or
  merge of the two canonical IDs.
- **TurboGrafx-16 / PC Engine** and **PC-98 / NEC PC-9801** both remain
  deliberately un-merged. Any future temptation to "simplify" the registry
  by collapsing either pair must be rejected — the registry's own module doc
  states plainly why (avoiding a silent rewrite of
  `platform_assignments.platform` values already stored for real libraries).
- **MegaDrive** and (if approved) **Atari 8-bit** are default-folder
  policies, not identity changes — no canonical ID is touched by either
  decision, so there is no cleanup risk in either case, only an export-
  target policy choice.
- **Commodore 128** and **NeoGeo64** carry no cleanup risk because no
  action is recommended for either — they remain exactly as they are.

## Persisted-ID / migration implications

Per `platform/mod.rs` (lines 10-17, 111-112) and `platform/identity.rs`
(`canonical_platform`, lines 335-339; `PlatformIdentityEvidence`,
`PlatformIdentityResolution`, both `Serialize`/`Deserialize`), the canonical
`Platform::id` string is a storage contract: it is written verbatim into the
`platform_assignments.platform` database column
(`crates/archivefs-core/src/database.rs`) and into serialized platform-
identity evidence. A DB-backed `platform_aliases` table
(`database.rs`, `add_platform_alias`) also keys directly off exact canonical
ID strings.

**None of the decisions in this document rename, merge, or delete any
canonical platform ID.** Specifically:

- MegaDrive: no change (already implemented, ID untouched).
- TurboGrafx-16 (SAFE_MAPPING): adds an export-table row only; the canonical
  ID `TurboGrafx-16` is untouched.
- PC-98/NEC PC-9801 (CANONICAL_CLEANUP_REQUIRED): both canonical IDs remain
  valid, persisted-compatible values forever; only the ES-DE export lookup
  gains a fallback step. No migration tool is required or recommended.
- Atari 8-bit (DEFER_V1, pending approval): if later approved, adds an
  export-table row only; the canonical ID `Atari 8-bit` is untouched.
- Commodore 128, NeoGeo64, PC: no action, so no migration surface at all.

No migration/rewrite tooling exists in the codebase today (the `old_platform`
/`new_platform` columns found in `database.rs` are an audit trail for manual
per-game reassignment events, not a bulk canonical-ID-rename mechanism), and
none is needed for any decision in this document.

## Recommended implementation order

1. **TurboGrafx-16 -> `tg16`** (`SAFE_MAPPING`). Lowest risk, exact evidence,
   no policy dependency — implement first, through the same table-driven
   seam as Batches 1-5.
2. **PC-98/NEC PC-9801 cleanup** (`CANONICAL_CLEANUP_REQUIRED`). Requires
   the small, explicitly-scoped equivalence-fallback lookup change described
   above; implement once that seam is reviewed and accepted.
3. **Atari 8-bit default** (`DEFER_V1`, pending sign-off). Bring the
   `atari800`-as-default recommendation to product sign-off; once approved,
   implement identically to the existing MegaDrive pattern (one row, marked
   PARTIAL).
4. **Commodore 128, NeoGeo64, PC**: no implementation scheduled. Revisit
   only if upstream ES-DE adds dedicated targets, or (for PC only) if a
   separate shortcut-based export mechanism is scoped and approved.

## Expected final V1 parity

Applying only the two decisions classified safe to implement without
further product sign-off (TurboGrafx-16 `SAFE_MAPPING`, PC-98/NEC PC-9801
`CANONICAL_CLEANUP_REQUIRED`):

| State | Current | After TurboGrafx-16 + PC-98 cleanup |
| --- | ---: | ---: |
| COMPLETE | 68 | 71 (+TurboGrafx-16, +PC-98, +NEC PC-9801 via equivalence) |
| PARTIAL | 1 (MegaDrive) | 1 (MegaDrive) |
| MISSING | 7 | 3 (Atari 8-bit, Commodore 128, NeoGeo64) minus PC which moves to a permanent out-of-scope state rather than a resolvable gap |
| Canonical platforms | 76 | 76 |

If the Atari 8-bit default is subsequently approved and implemented:
COMPLETE stays 71, PARTIAL becomes 2 (MegaDrive, Atari 8-bit), and the only
permanently-irreducible gaps are **Commodore 128** and **NeoGeo64**
(`NO_ESDE_TARGET`, no upstream system exists) and **PC** (`DEFER_V1`, no
compatible ROM-style export shape exists) — a ceiling of **71 COMPLETE + 2
PARTIAL + 3 MISSING = 76**, with the 3 MISSING rows being genuinely
unresolvable without either an upstream ES-DE addition (Commodore 128,
NeoGeo64) or new shortcut-export design work (PC). A numerically complete
76/76 table is not achievable and is not the goal — matching real,
non-colliding upstream identity is.

## Definition of Done

This audit is done when:

- Every one of the 8 items in scope (MegaDrive plus the 7 MISSING platforms)
  has exactly one of the six allowed final classes (`SAFE_MAPPING`,
  `SAFE_ALIAS`, `CANONICAL_CLEANUP_REQUIRED`, `INTENTIONALLY_PARTIAL`,
  `NO_ESDE_TARGET`, `DEFER_V1`) — confirmed above, no vague classifications
  remain.
- Every classification is backed by exact file:line code evidence and exact
  current upstream ES-DE evidence, both cited above, with fetch date
  recorded (2026-09-05).
- No canonical platform ID is renamed, merged, or migrated by this document.
- No Rust source, test, or Cargo command was touched to produce it.
- Implementation of any recommendation here (TurboGrafx-16 mapping, PC-98
  cleanup, or a future Atari 8-bit default) is explicitly deferred to a
  separate batch, gated on the sign-offs noted above where applicable.
