# Production registry parity audit

Base: `50d4007`  
Branch: `feature/content-registry-parity`

This audit compares the existing registries without treating them as one
table:

| Source | Contract |
| --- | --- |
| `platform::PLATFORMS` | platform candidates and strong/weak extension evidence |
| `media_registry::MEDIA_FORMATS` | persistence as an `ArchiveKind` and watcher/scanner media recognition |
| `ingestion::content_registry` | coarse content category used by discovery and archive-member inspection |
| Inspector likely-content table | name-only archive-entry triage; never platform or content proof |
| `game_identity::supported_loose_rom_format` | bounded structural/exact identity dispatch for selected loose ROM families |
| `coverage_inventory::COVERAGE` | engineering validation status, not a production support registry |

The new `registry_parity` module derives a typed view from those sources. It
does not add an extension list or make Inspector, ingestion, media, and
platform semantics artificially identical.

## Inventory totals

- 75 canonical platform rows were examined.
- 111 unique strong-extension claims were examined.
- 53 media rows and 65 ingestion content rows were compared.
- 52 strong extensions have a registered media/content or bounded identity
  disposition.
- 59 strong extensions have an explicit `DeferredOrUnsupported` disposition
  because the base has no direct ingestion/identity route. They are not
  silently promoted to verified platform identity.

Classification summary for this sweep:

- A — BUG: 6 missing ingestion content dispositions fixed (`smd`, `gcm`,
  `gcz`, `rvz`, `wbfs`, `ciso`).
- B — FALSE CLAIM: 1 corrected: Amiga `.hdf` was strong despite the existing
  Amiga/X68000 collision; it is now weak and requires corroboration.
- C — INTENTIONAL HASH-ONLY: 0 new mismatches. Existing media rows remain
  persistence/discovery facts, not exact release identity.
- D/E — INTENTIONALLY DEFERRED or UNSUPPORTED: 59 strong claims are surfaced
  by the typed model for report-only follow-up. The current platform table
  does not encode a finer reason, so this audit does not invent one.
- F — DIFFERENT TABLE SEMANTICS: archive containers (`zip`, `7z`, `rar`) and
  Inspector-only likely-content entries are intentionally not required to
  appear as loose ingestion formats. Archive inspection is name-only triage;
  discovery uses its own container/member route.

## Findings and ownership

The following larger strong-extension families are report-only in this
branch. They have platform claims but no safe direct identity route in the
base, and must be assigned to their format owners before being promoted:

| Extensions | Platform family | Claiming subsystem | Missing production subsystem | Existing parser/route | Class | Recommended owner |
| --- | --- | --- | --- | --- | --- | --- |
| `32x` | Sega 32X | platform registry | loose identity/ingestion route | `sega32x_header_evidence` coverage only | D/E | Sega 32X production-wiring task |
| `3ds`, `cia`, `cci`, `cxi` | Nintendo 3DS | platform registry and Inspector | production identity/content route | no base production dispatch | D/E | 3DS task |
| `xci`, `nsp`, `nca` | Switch | platform registry and Inspector | production content/identity route | no base production dispatch | D/E | Switch task |
| `wud`, `wux`, `rpx` | Wii U | platform registry and Inspector | production content/identity route | no base production dispatch | D/E | Wii U task |
| `wad` | Wii | platform registry and Inspector | safe content route | no base WAD reader | D/E | Wii WAD task |
| `ngp`, `ngc` | Neo Geo Pocket families | platform registry and Inspector | production identity/content route | `ngp_header_evidence` is not dispatched by loose identity | D/E | NGP production-wiring task |
| `pbp`, `ecm` | PlayStation/PSP | platform registry and Inspector | complete cross-platform direct route | active PSP/PS1 worktrees own related wiring | D/E/overlap | PSP/PS1 owners |
| `pkg` | PS3 | platform registry | production package route | active PS3 worktree owns this | D/E/overlap | PS3 owner |
| `xbe`, `xex`, `xiso`, `god` | Xbox families | platform registry | complete per-format ingestion parity | active Xbox worktree owns related wiring | D/E/overlap | Xbox owner |
| `jfd`, `fdi`, `d88x`, `thd`, `xdf`, `dim` | computer disk families | platform registry | format-specific discovery/identity | partial disk readers only | D/E | respective computer-format owners |
| `cdi`, `gdi` | Dreamcast/disc families | platform registry/content | media persistence parity in the base | specialist discovery/identity branches exist | F/A follow-up | disc-media owner |

The table intentionally does not claim these formats are safe to register in
this task. Existing active worktrees for Apple media, NES/FDS, SNES, N64,
PSP, PS3, GBA/Virtual Boy, Nintendo launch rows, and MAME were not copied or
modified.

## Inspector parity

Inspector deliberately recognizes a broader name-only set than ingestion,
including entries such as `3ds`, `cia`, `wad`, `pbp`, and `ecm`. This lets a
ZIP listing say “likely content” without asserting that the scanner can parse,
catalogue, or assign a platform to the entry. The parity tests preserve this
distinction and do not force Inspector’s table to equal either registry.

## Shared-extension safety

Parity logic does not create strong platform claims for `bin`, `iso`, `img`,
`rom`, `dsk`, `tap`, `cas`, `adf`, `chd`, `zip`, `7z`, `rar`, or `hdf`.
`.hdf` was corrected from an existing erroneous Amiga strong claim to weak
evidence because Sharp X68000 uses the same extension. Generic `.iso` remains
generic optical media; the other shared extensions remain corroboration or
context candidates.

## Coverage inventory

The coverage manifest remains a validation manifest, not a dispatch table.
Its parser-backed rows for NES, SNES, N64, GBA, and related families now have
production routes in the base, while entries such as NGP/NGPC and Sega 32X
describe parser/fixture work that is not itself a loose discovery route.
Those are intentionally recorded as follow-up production-wiring work rather
than overstated as end-to-end support. No coverage status was changed by this
sweep.

## 74-versus-75 platform assertion

The GUI test was stale: the platform registry has 75 canonical rows, while
the actual artwork contract already iterates the registry and checks each row
for a unique valid asset id and a non-unknown fallback category. The literal
`74` assertion was removed. No platform artwork/category row was missing, so
changing the number to `75` would have preserved the brittle invariant rather
than testing the real one.

