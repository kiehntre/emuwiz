# Blue large-library measurements (2026-10-18)

## Scope and reproducibility

Baseline: fresh main `e5c3684f033dbfb270a7b2f9a1c17cd8b7e06721`, with the
measurement-only commit `374fa9e2051c1eab18f280050cc70a2ed96238de`.
The ignored test `gui_v2::tests::large_library_performance::blue_large_library_measurement`
creates exactly 130,000 SQLite catalogue records, one configured catalogue source,
assignments across eight platforms, deterministic permuted titles, and no saved DAT audits.
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

Comparable runs use the identical harness and fixture above. Times are milliseconds;
CPU means measured thread CPU, not wall time. Warm renderer medians exclude the
first two frames; full-coordinator medians/p95 cover 32 paced frames. Browse/search
full-coordinator phases finish with `filter_pending=false`. Renderer-only search
frames can show pending and are not used as completed-view comparisons.

| Measurement | Before | After |
|---|---:|---:|
| Catalogue load/projection/title sort, median CPU of 3 | 5720.5 | 4414.1 |
| Async startup to loaded dashboard, wall | 7777.2 | 4552.2 |
| Home initial renderer CPU / warm median | 277.9 / 2.0 | 300.7 / 2.0 |
| Platform navigation initial CPU / warm median | 46.1 / 3.0 | 46.0 / 3.0 |
| Games grid warm CPU median | 4.0 | 4.0 |
| Games list warm CPU median | 3.0 | 3.0 |
| Full Browse entry first-frame CPU | 173.6 | 81.2 |
| Full search-change first-frame CPU | 94.5 | 4.5 |
| Full search CPU median / p95 | 5.7 / 10.9 | 3.7 / 4.9 |
| Full Game Details CPU median / p95 | 21.8 / 62.6 | 3.8 / 26.9 |
| Museum platform warm renderer CPU | 10.0 | 6.5 |
| Platform filter CPU | 7.5 | 0.6 |
| Unscoped search CPU: all / region / no match | 64.5 / 51.7 / 47.9 | 79.8 / 69.0 / 73.0 |
| Full History CPU median / p95 | 2.7 / 26.5 | 2.5 / 22.4 |
| Full Activity CPU median / p95 | 3.3 / 4.6 | 2.7 / 4.5 |
| No-provider artwork-index CPU | 2202.7 | 22.0 |
| Retained resolved metadata entries / warnings | 130000 / 2 | 0 / 2 |
| RSS while separate artwork index is live, KiB | 838700 | 510432 |
| RSS at end of full scenario, KiB | 1317412 | 708740 |

The largest improvements are avoiding per-game neutral metadata allocation and
moving large search scans out of paint. Unscoped filtering itself is **not faster**
in these unoptimized runs; it now executes on the existing debounced worker.
The whole-scenario cumulative process CPU (including fixture construction) was
56.38 versus 40.92 seconds. These single controlled runs establish large local
improvements, not precise speedups for every library shape.

No broad History/Activity speedup is claimed. The separate 1,000-receipt History
renderer median was 12.0 versus 12.0 ms; Activity with 100 retained jobs was 20.0
versus 30.2 ms, showing remaining layout cost/variation rather than improvement.
The full Browse CPU p95 increased from 6.9 to 61.6 ms during first-entry settling,
even though its first frame and median improved (4.8 to 3.1 ms). Cold rendering,
asset work and completion frames remain capable of exceeding a frame budget.

| SQL phase | Statements before / after | PROFILE lifetime ms before / after |
|---|---:|---:|
| First catalogue load | 7 / 7 | 2114 / 2156 |
| Async startup | 22 / 22 | 4083 / 3268 |
| Full Home, Browse and search, each | 0 / 0 | 0 / 0 |
| Full Game Details | 18 / 18 | 755 / 706 |
| Full History | 3 / 3 | 0 / 0 |
| Full Activity | 0 / 0 | 0 / 0 |

There is no query-count improvement claim. Archive hydration and source-folder
unknown counts dominate catalogue SQL lifetimes (about 0.86 and 1.30 seconds in the
final first load); background summary aggregation also contributes. These paths
remain unchanged. PROFILE lifetimes include Rust row processing and may overlap
across background connections, so they must not be summed as exclusive UI stalls.

Raw logs, SQL shapes, pre-fix regression failures and review/promotion evidence
are retained in `/home/davedap/emuwiz-audit-ledger/2026-10-18-large-library/`:
`baseline-controlled.log`, `final-controlled.log`, `final-tests.log`,
`focused-corrections.log` and `async-before-red.log`.

## Validation and remaining limits

The final affected core/GUI gate passed 122 tests (19 core and 103 GUI), with one
pre-existing production-library test ignored. Coverage includes supersession,
metadata aggregation, provider matching/diagnostics, library filtering, imagery,
Browse & Play, Home, Game Details, History and Activity. New regressions cover
large-search pending presentation, stale worker replies, same-size snapshot
replacement, platform-filter/order parity, title twins, exact neutral metadata
and provenance parity, and the supersession fast path.

The ignored benchmark is run separately, including its isolated subprocess.
The production/new test files pass targeted rustfmt. The module-declaration file
`gui_v2/tests.rs` has the same pre-existing formatting differences on main; only
six module-declaration lines were added there. Diff whitespace, scope and GUI-root
boundary checks pass. No unrelated formatting was applied.

Baseline measurements use main `e5c3684f`; final measurements use the unchanged
harness on `7f49bb1b`, rebased on main `003f8465`. Intervening mount-root and ScummVM
trainer promotions were preserved. The latter adds History work, so these runs
are not evidence of a History performance improvement. Independent review checks
source semantics and the exact final patch, separately from the implementation
owner's executed tests and measurements.

These are warm-cache, unoptimized test-build measurements on one synthetic shape,
not release-build FPS or a prediction of the owner's resident memory. Cold asset
layout/decoding, source-folder counting and archive hydration remain significant.
Provider-populated libraries, large decoded artwork caches, GPU stalls and real
window-system responsiveness need separate isolated measurement. No live-library
profiling or production game operation was performed.
