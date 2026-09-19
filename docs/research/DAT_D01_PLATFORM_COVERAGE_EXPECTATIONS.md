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
- Amiga is the intentionally narrow classic-media TOSEC mapping supported by
  the current architecture. TOSEC is not promoted to every legacy computer
  platform merely because a TOSEC file exists.
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
