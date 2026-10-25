# DAT D-01: platform coverage expectations

D-01 adds a core-only expectation layer in
`archivefs_core::dat::coverage_expectations`.

The mapping resolves an existing canonical platform identifier or exact
platform-registry alias to the authoritative evidence normally expected for
that platform. It does not inspect the DAT registry and does not claim that a
source is installed, active, current, or verified; those are inputs for the
future D-02 coverage projection.

## Authority boundary

- Supported cartridge families explicitly mapped by the current No-Intro
  importer expect `DatEcosystem::NoIntro`.
- Redump expectation follows all 17 reviewed systems in current main's typed
  `RedumpGameSystem` table. This records the authoritative ecosystem; current
  acquisition support is a separate capability and remains unavailable for
  some systems.
- Arcade has official MAME listxml as its primary source and FBNeo as a
  supplementary supported source. Both are retained deterministically.
- ScummVM expects the official detector evidence, not generic DAT matching.
- Classic computer platforms with a reviewed TOSEC mapping expect TOSEC as the
  primary source (see "Reviewed classic-computer mapping" below). TOSEC is not
  promoted to any other platform merely because a TOSEC file exists.
- ScreenScraper, RomM, metadata providers, generic DATs, and user/local DATs
  never appear as expected authoritative sources.

Known canonical platforms without an explicit mapping return
`NoKnownAuthoritativeSource`. Unknown text and absent platform identity return
`UnsupportedOrUnknown`; the mapping never infers from extensions, folders, or
fuzzy names.

`PlatformCoverageState` reserves the later D-02 states: covered, expected but
missing/inactive/stale/unmanaged, no expected source, and unknown platform.
No GUI, acquisition, update, freshness, or source-selection behavior is wired
by D-01.

## Reviewed classic-computer mapping

TOSEC availability is not authority. A platform was added only where its media
is ingestible content, a TOSEC system group catalogues it, the shared exact-hash
DAT path verifies the delivered file, and the repository's own audit
(`docs/MEDIA_SUPPORT_AUDIT.md`) names TOSEC for it. The coverage layer operates
on an *already canonical* platform; a TOSEC name never resolves a platform
(BBC Micro and Acorn Electron remain separate entries because their media is
shared and TOSEC text cannot decide between them).

| Canonical platform | Expected sources | TOSEC system (exact) |
|---|---|---|
| Amiga | TOSEC (primary) | `Commodore Amiga` |
| BBC Micro | TOSEC (primary) | `Acorn BBC` |
| Acorn Electron | TOSEC (primary) | `Acorn Electron` |
| Amstrad CPC | TOSEC (primary) | `Amstrad CPC` |
| Commodore 64 | TOSEC (primary) | `Commodore C64` |
| Commodore 128 | TOSEC (primary) | `Commodore C128` |
| VIC-20 | TOSEC (primary) | `Commodore VIC20` |
| AtariST | TOSEC (primary) | `Atari ST` |
| ZX Spectrum | TOSEC (primary) | `Sinclair ZX Spectrum` |
| Apple II | TOSEC (primary) | `Apple II`, `Apple IIGS` (the registry folds IIGS into Apple II) |
| Atari 8-bit | No-Intro (primary, unchanged) + TOSEC (secondary) | `Atari 8bit` |

Left `NoKnownAuthoritativeSource` on purpose: MSX/MSX2 (no reviewed MSX disk/ROM
path beyond WAV; likely dual-ecosystem), DOS/PC (no IBM PC group in the pack;
folder-installed games), Macintosh (partial support), Dragon/CoCo, Oric,
Thomson, Enterprise, the Japanese PCs, X68000, FM Towns, Acorn Archimedes.

`dat::tosec_system_projection` holds the exact `platform <-> TOSEC system`
table used for the (future) flow *platform -> expected TOSEC ecosystem ->
relevant system groups -> explicit user approval*. Matching is exact; unknown,
neighbouring (`Commodore C64DTV`, `Sinclair ZX81`) and decorated text fails
closed. It enables nothing.

Known limitation found by the real-pack probe: the release-pack classifier
projects `X - Compilations - Games - [..]` catalogue names to the system
string `X - Compilations`, so those groups do not project to a platform
(fail-closed, never guessed). Most media tokens in real names (`[D64]`) are
classified `Other`. Both are classifier concerns, not mapping concerns.
