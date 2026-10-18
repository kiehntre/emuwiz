# Blue large-library measurements (2026-10-18)

## Scope and reproducibility

Baseline: fresh main `e5c3684f033dbfb270a7b2f9a1c17cd8b7e06721`, with the
measurement-only commit `374fa9e2051c1eab18f280050cc70a2ed96238de`.
The ignored test `gui_v2::tests::large_library_performance::blue_large_library_measurement`
creates exactly 130,000 SQLite catalogue records, one configured catalogue source,
eight platform assignments, deterministic permuted titles, and no saved DAT audits.
Media paths point into a disposable directory; no ROM files are created or read.
The normal test constructor supplies private HOME/XDG/application directories.
The measurement child has an empty private PATH and no inherited APPIMAGE/APPDIR.
No production database, GUI, profiles, game operation or emulator launch is used.

Run alone from the owned worktree:

```sh
nice -n 10 scripts/cargo-iso --low-debug --incremental test --offline -j 2 \
  -p archivefs-gui --lib \
  gui_v2::tests::large_library_performance::blue_large_library_measurement \
  -- --exact --ignored --nocapture
```

The fixture measures three repeated catalogue loads (including projection/title
sorting), headless Home/dashboard, platform navigation, grid/list browsing,
searches, Museum platform browsing, Game Details, 1,000 synthetic history receipts,
and Activity after 2,000 completed jobs (the existing retention policy keeps 100).
It separately measures the real asynchronous App constructor through the first
loaded dashboard, then paced full coordinator frames including background polling,
filter debounce, focus painting and end-of-frame handling. Routes are asserted.

SQLite PROFILE reports statement counts and elapsed statement lifetimes, with
millisecond resolution and Rust row hydration included; it is not exclusive VM CPU
time. `/proc/thread-self/schedstat` measures UI/operation thread CPU, excluding
scheduler delay. Wall time and process CPU ticks are also retained. RSS/HWM readings
are cumulative process/allocator observations; individual page deltas are not
allocation attribution. Renderer-only phases use the existing empty artwork test
indexer. Real no-provider indexing is measured separately and during asynchronous
startup/browsing. Native workflow test construction uses the existing operation
fixture. GPU/window-system/texture-upload cost, populated provider caches, cold
filesystem caches and the owner's actual desktop frame rate are outside this test.

## Demonstrated costs and bounded corrections

- Large Browse & Play searches scanned the complete catalogue during paint, even
  though the app already has a debounced, generation-checked Filter worker. Large
  snapshots now use that worker. Pending is shown explicitly, old rows are hidden,
  stale replies are discarded, and same-size snapshot replacement invalidates the
  cache. Catalogues of at most 10,000 rows retain immediate filtering.
- Museum and Game Details repeatedly scanned the catalogue for platform members
  and identical titles. Immutable library snapshots now derive ordered platform
  indices and exact-title twin IDs once. Search semantics and title sort order
  remain unchanged; per-platform searches visit their existing ordered subset.
- Artwork indexing allocated a complete local-only neutral metadata result and
  generic diagnostic string for every unmatched game. These are now generated on
  selection from the same snapshot. Matched, conflicting and provider-specific
  results remain retained, with the same matching, provenance, fallback and warning
  rules. An empty RomM record set also skips the wrapper-matching walk.
- Supersession built several complete path/name indexes even with no old rows.
  The pure resolver now returns its necessarily empty result before building those
  indexes. Missing/unconfigured source rows still follow the unchanged resolver.

No catalogue freshness, game verification, identity persistence, transaction or
launch-authorisation path was changed. DAT-selector performance work was excluded.

## Results

Final controlled before/after values and validation are recorded after measurement.
The audit ledger retains raw profiles, SQL statement shapes, regression failures,
review and promotion evidence.
