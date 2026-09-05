# ES-DE Platform Parity Audit

Audit point: `2efd998d6a17d4c6a1620be96fe03c4625e78efc` on
`feature/archivefs-unified-platform`. This is a source audit only; no Cargo
build was run and no implementation change is included.

## 1. Executive summary

EmuWiz already has the complete publication safety spine requested by this
audit: Playing Library/1G1R selection, an ES-DE projection, explicit preview,
confirmation before mutation, idempotent `<path>` insertion, atomic writes,
durable crash recovery, and same-session rollback. The publication code fails
closed when a canonical platform has no reviewed ES-DE target.

The authoritative platform registry contains **76 canonical platforms**. The
reviewed `ES_DE_SYSTEM_MAP` contains **20 rows**: **19 COMPLETE**, **1
PARTIAL** (`MegaDrive`, because the code deliberately chooses one of two
valid ES-DE regional system names), and **56 MISSING**. “Missing” means
missing from EmuWiz's reviewed mapping, not necessarily unsupported by ES-DE.
It is a production failure for that platform: publication returns
`PlatformUnmapped` before a gamelist is read or changed.

The biggest gaps are the unmapped systems that have already landed elsewhere
in EmuWiz—PS Vita, Nintendo 3DS, Wii U, Arcade/MAME/FBNeo, Amiga CD32/CDTV,
ScummVM, DOS, and most of the long tail. Multi-emulator support does not by
itself require ES-DE-specific emulator mappings: ES-DE consumes a system
folder and a game path, while emulator/core selection remains a separate
launch concern. Artwork is not published by this path at all; media
directories are discovered but unused.

## 2. Current ES-DE architecture

The integration is distributed across these production files:

| Area | File | Finding |
|---|---|---|
| ES-DE system export mapping | `crates/archivefs-core/src/launch/es_de_export.rs` | Reviewed canonical-platform -> ES-DE short-name/full-name table; unmapped platforms fail closed. |
| Gamelist planning/publication | `crates/archivefs-core/src/launch/es_de_publish.rs` | Reads, previews, appends, atomically writes, and recovers one `gamelist.xml`; preserves unrelated bytes. |
| ES-DE discovery/profile | `crates/archivefs-core/src/emulator_environment/es_de.rs` | Discovers Native, Explicit, AppImage, and Portable profile shapes; reads custom/bundled system definitions and derives gamelist/media locations. |
| Playing Library bridge | `crates/archivefs-core/src/playing_library/retrodeck_projection.rs` | Projects elected operations into `roms/<es-de-system>` and obtains the publication plan. |
| GUI journey | `crates/archivefs-gui/src/playing_library_page.rs` | Platform picker, preview, confirmation, publish result, unresolved-recovery gate, and restore action. |
| Canonical identity | `crates/archivefs-core/src/platform/mod.rs` and `src/lib.rs` | Single 76-row registry; normalized aliases resolve to canonical IDs, with ambiguity refused. |

There is one ES-DE profile shape conceptually, with four installation
provenance variants (`Native`, `Explicit`, `AppImage`, `Portable`). There is
no frontend profile per emulator and no ES-DE-specific launch-command table.

## 3. Existing publication journey

1. Playing Library builds an evidence-backed 1G1R plan. The ES-DE bridge uses
   only elected games whose launcher operations survived conflict filtering;
   it does not re-elect or re-identify content.
2. The GUI discovers an ES-DE profile and lets the user choose one canonical
   platform from the reviewed ES-DE options.
3. Preview calls `plan_es_de_gamelist_publication`. It resolves the canonical
   platform through `es_de_system_for_platform`, finds the matching discovered
   ES-DE system, resolves its `gamelist.xml`, reads it with bounds, and
   reports `added` versus `already_present` entries.
4. Confirmation calls `apply_es_de_gamelist_publication`. New entries contain
   only escaped `<path>` and `<name>` fields and are inserted before
   `</gameList>`; existing bytes are otherwise retained.
5. Before the real write, a path-derived JSON recovery record stores the exact
   prior content (or prior absence). The gamelist is then written atomically;
   the recovery record is removed only after successful finalization.
6. A leftover record blocks another publication. The GUI offers explicit
   restore, which restores the exact prior bytes after restart and then removes
   the record. Same-session rollback also protects against overwriting changes
   made after publication.

## 4. Canonical platform inventory

The inventory below is taken from `platform::PLATFORMS`, not from the current
ES-DE table. Registry aliases are normalized whole-folder aliases; `Y` means
additional aliases exist, `N` means the canonical ID/display spelling is the
only registry route visible in the row. The registry currently has no
filename aliases in these rows. Equivalent pairs are separately retained for
`PC Engine`/`TurboGrafx-16` and `PC-98`/`NEC PC-9801`.

| Canonical ID | Display name | Registry aliases | ES-DE status |
|---|---|---:|---|
| 3DO | 3DO Interactive Multiplayer | N | MISSING |
| Acorn Archimedes | Acorn Archimedes | N | MISSING |
| Acorn Electron | Acorn Electron | Y | MISSING |
| AmigaCD32 | Amiga CD32 | Y | MISSING |
| Amstrad CPC | Amstrad CPC | N | MISSING |
| Apple II | Apple II | N | MISSING |
| Macintosh | Apple Macintosh | N | MISSING |
| Arcade | Arcade | Y (`mame`, `fbneo`, `fba`) | MISSING |
| Atari2600 | Atari 2600 | Y | MISSING |
| Atari5200 | Atari 5200 | Y | MISSING |
| Atari7800 | Atari 7800 | Y | MISSING |
| Atari 8-bit | Atari 8-bit | N | MISSING |
| Atari Jaguar | Atari Jaguar | Y | MISSING |
| Atari Lynx | Atari Lynx | Y | MISSING |
| AtariST | Atari ST | Y | COMPLETE |
| WonderSwan | Bandai WonderSwan | Y | MISSING |
| WonderSwan Color | Bandai WonderSwan Color | Y | MISSING |
| BBC Micro | BBC Micro | N | MISSING |
| ColecoVision | ColecoVision | Y | MISSING |
| Commodore 128 | Commodore 128 | Y | MISSING |
| Commodore 64 | Commodore 64 | Y | MISSING |
| Amiga | Commodore Amiga | N | COMPLETE |
| Commodore CDTV | Commodore CDTV | Y | MISSING |
| VIC-20 | Commodore VIC-20 | Y | MISSING |
| FM Towns | Fujitsu FM Towns | Y | MISSING |
| Vectrex | GCE Vectrex | Y | MISSING |
| Intellivision | Mattel Intellivision | Y | MISSING |
| Xbox | Microsoft Xbox | Y | COMPLETE |
| Xbox360 | Microsoft Xbox 360 | Y | COMPLETE |
| DOS | MS-DOS | Y | MISSING |
| MSX | MSX | Y | MISSING |
| MSX2 | MSX2 | Y | MISSING |
| NEC PC-8801 | NEC PC-8801 | Y | MISSING |
| PC-98 | NEC PC-98 | Y | MISSING |
| NEC PC-9801 | NEC PC-9801 | Y | MISSING |
| NeoGeo | Neo Geo | N | MISSING |
| NeoGeo64 | Neo Geo 64 | Y | MISSING |
| Neo Geo CD | Neo Geo CD | Y | MISSING |
| Neo Geo Pocket | Neo Geo Pocket | Y | MISSING |
| Neo Geo Pocket Color | Neo Geo Pocket Color | Y | MISSING |
| Nintendo 3DS | Nintendo 3DS | Y | MISSING |
| N64 | Nintendo 64 | Y | COMPLETE |
| Nintendo DS | Nintendo DS | N | MISSING |
| NES | Nintendo Entertainment System | N | COMPLETE |
| Game Boy | Nintendo Game Boy | Y | COMPLETE |
| Game Boy Advance | Nintendo Game Boy Advance | Y | COMPLETE |
| Game Boy Color | Nintendo Game Boy Color | Y | COMPLETE |
| GameCube | Nintendo GameCube | Y | COMPLETE |
| Switch | Nintendo Switch | Y | MISSING |
| Virtual Boy | Nintendo Virtual Boy | Y | MISSING |
| Wii | Nintendo Wii | Y | COMPLETE |
| WiiU | Nintendo Wii U | Y | MISSING |
| NGage | Nokia N-Gage | Y | MISSING |
| PC | PC | Y | MISSING |
| PC Engine | PC Engine / TurboGrafx-16 | N | MISSING |
| PC Engine CD | PC Engine CD / TurboGrafx-CD | Y | MISSING |
| PC-FX | PC-FX | Y | MISSING |
| Philips CD-i | Philips CD-i | Y | MISSING |
| ScummVM | ScummVM | Y | MISSING |
| Sega 32X | Sega 32X | Y | MISSING |
| Dreamcast | Sega Dreamcast | Y | COMPLETE |
| GameGear | Sega Game Gear | Y | MISSING |
| MasterSystem | Sega Master System | N | MISSING |
| MegaDrive | Sega Mega Drive / Genesis | N | PARTIAL |
| Sega CD | Sega Mega-CD / Sega CD | N | COMPLETE |
| Saturn | Sega Saturn | Y | COMPLETE |
| Sharp X68000 | Sharp X68000 | N | MISSING |
| PSX | Sony PlayStation | N | COMPLETE |
| PS2 | Sony PlayStation 2 | Y | COMPLETE |
| PS3 | Sony PlayStation 3 | Y | COMPLETE |
| PS4 | Sony PlayStation 4 | Y | MISSING |
| PSP | Sony PlayStation Portable | N | COMPLETE |
| PlayStation Vita | Sony PlayStation Vita | Y | MISSING |
| SNES | Super Nintendo Entertainment System | N | COMPLETE |
| TurboGrafx-16 | TurboGrafx-16 | Y | MISSING |
| ZX Spectrum | ZX Spectrum | N | MISSING |

## 5. Platform mapping matrix

The reviewed rows currently present in `ES_DE_SYSTEM_MAP` are:

| Canonical ID | ES-DE system identifier | Mapping | Exact/ambiguous | Launch metadata | Artwork/media paths | BIOS/config assumptions |
|---|---|---|---|---|---|---|
| PSX | `psx` | exists | exact | sufficient: path/name; launch adapter is separate | gamelist/media location discovered; artwork not written | none in publication |
| PS2 | `ps2` | exists | exact | sufficient | same | none |
| PS3 | `ps3` | exists | exact | sufficient | same | none |
| PSP | `psp` | exists | exact | sufficient | same | none |
| Xbox | `xbox` | exists | exact | sufficient | same | none |
| Xbox360 | `xbox360` | exists | exact | sufficient | same | none |
| GameCube | `gc` | exists | exact | sufficient | same | none |
| Wii | `wii` | exists | exact | sufficient | same | none |
| Dreamcast | `dreamcast` | exists | exact | sufficient | same | none |
| Saturn | `saturn` | exists | exact | sufficient | same | none |
| Sega CD | `segacd` | exists | exact | sufficient | same | none |
| AtariST | `atarist` | exists | exact despite canonical `AtariST` casing | sufficient | same | none |
| Amiga | `amiga` | exists | exact | sufficient | same | none |
| Game Boy | `gb` | exists | exact | sufficient | same | none |
| Game Boy Color | `gbc` | exists | exact | sufficient | same | none |
| Game Boy Advance | `gba` | exists | exact | sufficient | same | none |
| NES | `nes` | exists | exact | sufficient | same | none |
| SNES | `snes` | exists | exact; ES-DE fullname spelling is preserved verbatim | sufficient | same | none |
| MegaDrive | `megadrive` | exists | **partial**: ES-DE also has `genesis` and `megadrivejp`; EmuWiz chooses `megadrive` as the reviewed default | sufficient | same | none |
| N64 | `n64` | exists | exact | sufficient | same | none |

For every other canonical row in section 4, the target is `—` (no reviewed
row), mapping is MISSING, launch metadata is not reached, and publication
fails with `EsDePublicationError::PlatformUnmapped`. The matrix is therefore
complete even where ES-DE itself may have a compatible system: the missing
fact is EmuWiz's reviewed target, not a claim about ES-DE's catalogue.

There are no duplicate entries in the current EmuWiz map. Canonical alias
resolution is normalized and ambiguity-safe, but aliases do not create
additional ES-DE targets. In particular, `mame` and `fbneo` normalize to the
canonical `Arcade` identity and currently stop at the missing ES-DE map row.

## 6. Missing/ambiguous mappings

The missing set is the 56 rows marked MISSING above. Highest-value gaps are:

- already-supported modern launch identities: `PlayStation Vita`, `Nintendo
  3DS`, `WiiU`, and `Nintendo DS`;
- requested computer/frontend families: `DOS`, `ScummVM`, `AmigaCD32`,
  `Commodore CDTV`, and `Arcade`;
- common console families: `PS4`, `Switch`, `MasterSystem`, `GameGear`,
  `MegaDrive`'s alternate regional naming, `Sega 32X`, and the PC Engine
  variants;
- the remaining 8/16-bit, computer, arcade, and regional systems in the
  canonical registry.

The only current mapping ambiguity is intentional `MegaDrive` target choice.
The map documentation records that ES-DE exposes `genesis`, `megadrive`, and
`megadrivejp`; these are regional alternatives, not distinct EmuWiz hardware
identities. A future slice should either preserve the reviewed default or add
an explicit region/profile policy—never silently choose based on an alias.

An unmapped canonical platform can absolutely make publication fail. The
failure is early and safe: no gamelist read/write occurs. A mapped platform
can still fail later when its ES-DE profile lacks the configured system,
contains an unsafe/unreadable gamelist path, or has unresolved recovery.

## 7. Multi-emulator platform handling

ES-DE publication currently needs only the canonical platform -> ES-DE system
folder and the playing-library launcher path. It does not select an emulator,
libretro core, BIOS, or command line. Consequently:

- `Arcade` may launch through MAME or FBNeo, but publication should use one
  ES-DE system policy. The current canonical identity intentionally treats
  `mame`, `fbneo`, `finalburnneo`, and `fba` as aliases of `Arcade`; no
  emulator-specific ES-DE mapping exists.
- `Amiga` has Amiberry, FS-UAE, WHDLoad, and RetroArch compatibility in the
  launch table. `AmigaCD32` and `Commodore CDTV` have Amiberry adapters, but
  neither has an ES-DE publication row.
- Game Boy-family launch candidates include SameBoy/mGBA/RetroArch-related
  paths, while ES-DE correctly needs only `gb`, `gbc`, or `gba`. The three
  mapped rows are not split by emulator.
- `ScummVM` uses directory-based game identity and its own adapter, but the
  publication writer can still publish a path/name entry once a reviewed
  `scummvm` system mapping exists. The current code does not provide it.

The later implementation boundary should be a frontend system policy only
unless a product requirement explicitly asks EmuWiz to author ES-DE's
emulator configuration. No current code does that, and `es_settings.xml` is
only existence-probed.

## 8. Recovery/idempotency review

The current semantics are strong and complete for the implemented scope:

- **Prior state intact on ordinary failure:** the recovery record is written
  first; if the gamelist atomic write fails, the old gamelist is not replaced
  and the record remains as an unresolved safety barrier.
- **Crash-safe recovery:** the record survives process restart and contains
  exact prior content or prior absence. Recovery restores that state and
  removes the record.
- **No unsafe guessing:** malformed, oversized, symlinked, schema-mismatched,
  or path-mismatched recovery records fail closed.
- **Idempotent repeat:** planning compares exact escaped `<path>` text;
  existing entries go to `already_present`; applying an unchanged publication
  is a no-op and does not write.
- **Byte preservation:** unrelated existing gamelist content is not
  reserialized. New entries are escaped and inserted before the closing tag.
- **Explicit UX gate:** the GUI does not offer publication while recovery is
  unresolved and offers restore instead.

The remaining caveat is scope, not a demonstrated defect: the recovery record
protects one gamelist publication at a time. The playing-library symlink
transaction and ES-DE gamelist write are separate operations; there is no
single cross-resource transaction that rolls both back together. The current
GUI deliberately describes ES-DE publication as a subsequent operation and
the publication itself never touches source/master ROMs.

## 9. Frontend profile coverage

There are four profile variants, not four different frontend schemas:

| Profile kind | Coverage | Audit result |
|---|---|---|
| Native | `~/ES-DE`, PATH `es-de` executable | COMPLETE |
| Explicit | caller-supplied home/executable, including unusual layouts | COMPLETE |
| AppImage | caller-supplied AppImage plus optional config root | COMPLETE |
| Portable | caller-supplied executable and home pair | COMPLETE |

The profile discovers custom systems and optionally supplied bundled systems;
the custom file complements, rather than replaces, ES-DE's bundled system
list. It derives `gamelists/<system>/gamelist.xml` and
`downloaded_media/<system>` locations. It does not parse `es_settings.xml`
content, author `es_systems.xml`, select an emulator, configure BIOS, or write
artwork/media. No current supported canonical platform falls outside the
profile *shape*; unmapped platforms fall outside the publication *system
table*.

## 10. Small implementation slices

| Slice | Size | Scope |
|---|---|---|
| Expand the reviewed canonical -> ES-DE table in batches, starting with PS Vita, 3DS, Wii U, DS, DOS, ScummVM, Arcade, AmigaCD32, and CDTV | MEDIUM | Add source-verified IDs/full names, aliases/tests, and GUI options; preserve fail-closed behavior. |
| Add a table-driven parity test asserting every canonical platform is classified and every mapped row has a unique target | SMALL | Prevent silent registry/map drift. |
| Resolve MegaDrive region policy explicitly | SMALL | Retain `megadrive` default or make region selection a deliberate profile setting. |
| Add ES-DE media/artwork publication | DEFER | Separate product scope; current path only publishes path/name and only discovers media directories. |
| Add emulator-specific ES-DE configuration generation | DEFER | Not required for system-folder publication; would need a new, separately reviewed config contract. |
| Make Playing Library links and gamelist publication one transaction | DEFER | Current two-step operation is explicit and safe; cross-resource atomicity is a larger design. |

## 11. Definition of Done

- Every row in the 76-platform canonical registry has a reviewed disposition:
  exact ES-DE target, explicit intentional non-target, or documented
  unsupported case.
- Every supported row has one canonical ES-DE identifier, verified fullname,
  alias/normalization tests, and no duplicate target ambiguity.
- Publication preview reports unmapped, unconfigured, unreadable, and
  already-present states distinctly before confirmation.
- Platform expansion does not change the existing Playing Library/1G1R
  election or active Dolphin/local-cheat workflows.
- Multi-emulator platforms remain system-mapped unless an explicit frontend
  configuration requirement is approved.
- Existing atomic-write, recovery, rollback, byte-preservation, and
  idempotency tests remain green.
- Artwork/media and BIOS/config work is either implemented under a separately
  reviewed contract or explicitly excluded from ES-DE parity claims.

### Audit report

- HEAD inspected: `2efd998d6a17d4c6a1620be96fe03c4625e78efc`
- Canonical EmuWiz platforms found: **76**
- ES-DE classification: **19 COMPLETE, 1 PARTIAL, 56 MISSING**
- Biggest parity gaps: missing modern adapters and frontend families; no
  artwork/media publication; no explicit MegaDrive regional policy.
- Likely production files later: `launch/es_de_export.rs`,
  `launch/es_de_publish.rs`, `emulator_environment/es_de.rs`,
  `playing_library/retrodeck_projection.rs`, and
  `playing_library_page.rs`, plus focused platform/map tests.
- Dolphin/Cheats files touched: **none by this audit**; existing unrelated
  local Dolphin changes were preserved.
- Cargo/build: **not run**.

ES-DE PLATFORM PARITY AUDIT COMPLETE
