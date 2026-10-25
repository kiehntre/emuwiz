# GUI v2 milestone 1: parallel front door

This is a review build, **not a claim of live acceptance**. It does not replace
the legacy GUI and must not land on main before visual/user review.

## Run and return safely

Build from the GUI-v2 worktree:

```sh
CARGO_TARGET_DIR=/home/davedap/.cache/emuwiz-cargo-target CARGO_INCREMENTAL=0 \
  cargo build --release -p archivefs-gui --bin emuwiz-v2
/home/davedap/.cache/emuwiz-cargo-target/release/emuwiz-v2
```

Run the second command in a terminal in the desktop session being tested. Do not
kill an existing EmuWiz window. The `emuwiz`, `emuwiz-gui` and `archivefs-gui`
entry points still run the legacy GUI. V2's **Legacy / Advanced interface** action
starts its own executable with an intentional legacy-host argument, so the
handoff always uses the same build, not an unrelated older installed binary.
The legacy window is labelled; “Return to GUI v2” closes it, leaving v2 open.
Game paths are passed as exact OS argv, not shell strings. No launch, repair,
provider activation or install happens just because a task page is opened.

## Ownership and implementation boundary

`crates/archivefs-gui/src/gui_v2/` owns the new interface. The GUI library root
adds only a module declaration; the existing app, pages, controllers, CLI and
core remain unchanged.

| Module | Responsibility |
|---|---|
| `mod.rs` | Independent eframe bootstrap, event coordination, generation checks |
| `routes.rs` | Stable task locations, six Home tasks, bounded Back history |
| `pages.rs` | Paint-only shell, browser, game page, Activity and explicit handoffs |
| `library.rs` | Immutable lightweight catalogue/filter/status projections |
| `backend.rs` | Background catalogue/config/detail/discovery work and explicit actions |
| `activity.rs` | Shared queued/running/complete/failed/cancelled jobs and result routes |
| `media_sources.rs` | Existing ES-DE, LaunchBox and RomM read-only media indexes |
| `artwork.rs` | Bounded scheduling, generations, off-screen cancellation, texture LRU |
| `thumbnail.rs` | Bounded decode, resize, persistent immutable local thumbnails, timings |
| `legacy.rs` | Explicit selected-game/task handoff to unchanged legacy workflows |
| `tests.rs` | Navigation, page anatomy, state, read-only data, cache and worker regression tests |

The UI thread performs layout, immutable lookups, queue sends and at most four
small texture deliveries per frame. No database query, filesystem read, network
request or image decode is performed by the page renderer. A single backend
worker serializes ordinary catalogue/preferences/detail actions. Search is
debounced and runs on that worker; stale replies cannot replace newer filters.
The previous catalogue stays usable if a reload fails. The browser draws only
visible rows using egui's virtualized grid/list scroll area.

The catalogue load intentionally avoids the legacy full-snapshot path's per-item
DAT-summary/provenance queries, duplicate analysis, mod inventory and source
coverage work. Those belong to task/detail requests, not every game card.
Selected-game inspection stats the chosen source and loads its saved check
records on the worker; it does not hash/reidentify the entire library.

An installed program is displayed as **Found**, not automatically **Ready to
play**. V2's saved identity comes from core's existing identity bridge; filenames
cannot become proof. Full game/firmware/media preflight and execution remain in
the existing selected-game Play workflow. Existing exact-path imported synopsis,
genres, year and player information are display-only, never identity evidence.

## Page and navigation contract

The persistent sidebar has Home, Library tasks, Play/setup, Mods & Cheats,
Artwork & Metadata, Sources, Activity, History, Settings and Advanced. At narrow
desktop widths it becomes narrower and scrollable, not hidden. Buttons participate
in normal Tab/Shift+Tab focus and Enter activation. Alt+Left/Escape returns Back;
Alt+Home returns Home. Nested pages show the game/task location explicitly.

Home uses six single-column explained task cards. Every page has its purpose,
location, a useful next action, a signpost and a safe Back/Home route. Handoffs
explain what will happen *before* opening another window. The selected game is
carried into Play and Mods; verification remains platform/folder-oriented in
the existing workflow. Normal body/button text is 18 px, headings 27 px, minimum
standard targets 40 px and primary targets 44 px. Status is written, not only
coloured. Screenshots are not requested until “Show screenshots” is selected,
and then only visible screenshots are requested.

Location/filter preferences use a separate, asynchronously written, atomic
`gui-v2.json` under the effective EmuWiz config directory. Legacy mode/config is
not changed. Reloading lists, inspecting games and exploring pages do not change
source media. Explicit rescan requires confirmation and uses the existing
scanner; it updates EmuWiz's catalogue, never original game files. A failed folder
is reported rather than claiming the entire scan succeeded.

## Artwork pipeline and safety

There are two local decode/cache-probe workers, one remote worker and one
background metadata-index worker. At most 96 requests wait in the normal queue
(an in-flight local probe may transfer to the remote lane); the reply channel
holds at most 32 results. Only visible cards request pictures. Scrolling away
marks requests cancelled; queued jobs skip work, in-flight work is discarded on
delivery and remote fetches receive core's cancellation flag. Cancellation does
not forcibly interrupt an image decoder or a currently blocked network read;
core network timeouts still apply.

Cached RomM thumbnails are probed on local lanes, so an uncached remote picture
cannot block them behind network work. All remote fetching still goes through
the existing core ArtworkCache and approved endpoint/token/mapping policy. The
one remote writer avoids concurrent writes to its existing shared index. No
new remote provider, scraper or arbitrary-URL fetch path was added.

Local pictures use existing ES-DE/LaunchBox indexes without depending on a RomM
record. LaunchBox uses exact path/platform lookup, never title-only matching.
Competing RomM path records are refused for artwork association. Local media is
preferred over waiting on remote pictures. Media discovery happens once per
library reload, not per card. PNG, JPEG and WebP are supported for local pictures;
the manifest adds the existing image crate's JPEG/WebP features (and the WebP
codec's lockfile entry), not a new media backend.

The local cache is `gui-v2-thumbnails-v1` under the effective EmuWiz data directory.
Keys include canonical source path, size, high-resolution mtime and Unix inode/
change time where available. Images are resized to at most 240×320. Encoded input
is capped at 32 MiB, dimensions at 8192×8192 and decoder allocation at 64 MiB.
First uncached local decoding may need a bounded full-source decode: image's
general PNG/JPEG/WebP reader does not provide a common reduced-resolution decode
API. That cost happens once on a worker; later loads decode only cached thumbnails.
It is **not** correct to claim the first decode reads only thumbnail-sized pixels.

Cache writes use same-directory temporary files and atomic no-clobber publication.
Existing unrelated files are never overwritten; malformed cache entries are
ignored and the source can still be displayed. Source changes during decoding
refuse that result. Traversing relative paths and symlinked cache roots are
refused. Original images/media are not modified. The in-memory cache retains at
most 192 entries after a frame (roughly 56 MiB of full-sized RGBA thumbnails).
The disk-cache soft budget is 512 MiB, trimmed on index refresh, selecting only
the managed digest-PNG naming scheme and leaving other files/symlinks alone.

## Activity and instrumentation

Catalogue loading, scanning, game inspection, media indexing, artwork, opening
folders and opening legacy workflows have real Activity entries. Jobs expose
elapsed time, actual known counts, explicit completion/failure and a result route.
Artwork counts include off-screen cancellations and report failed pictures.
There are no synthetic percentages or invented ETAs. The existing scanner has
no safe cooperative cancel/progress callback at this seam, so it is explicitly
indeterminate and non-cancellable, rather than pretending to support either.

V2 Activity is session-scoped (bounded completed history), not a replacement for
durable repair/mod journals. Legacy operations report progress in their own
window; v2 reports the handoff, not fictitious progress for those operations.

`EMUWIZ_LOG=debug` enables request timing logs without source paths, URLs or tokens.
Game → Advanced details shows metadata lookup, local/cache lookup, network time,
thumbnail decode, resize, hit/miss and UI-delivery time. Settings → Advanced
details shows library and media-index timing. The existing core fetcher performs
its own decode/resize/cache publication; without changing core those internal
stages are honestly reported together as `provider_processing`, separate from
network time. V2's decode/resize fields measure its delivered-thumbnail stages.

Read-only real-library measurement (does not open a GUI or fetch pictures):

```sh
/home/davedap/.cache/emuwiz-cargo-target/release/emuwiz-v2 --measure-library
```

Initial debug measurements on this host, not release promises:

| Check | Observed |
|---|---|
| Project 50,000 synthetic persisted records, including sorting/search keys | 187–235 ms |
| Filter those 50,000 records by system + text | 8–12 ms |
| First local 1200×1600 JPEG → persistent 240×320 thumbnail | 540–569 ms, worker-only |
| Repeat persistent thumbnail load/decode | 7–8 ms |
| Revisit an in-memory picture | Existing texture reused; no new worker request |
| Browser with 10,000 games | Fewer than 30 game titles emitted at 1280×820 |

Release measurements against the actual local catalogue on 2026-09-19:
103,165 entries / 52 platforms loaded in **432 ms**; the read-only artwork index
associated **18,940 covers / 5,095 screenshot groups in 2,162 ms**. No picture
network requests were made by this measurement. These are one-run observations,
not benchmark averages or end-to-end visual acceptance guarantees.

The tests print the measurements rather than enforcing brittle wall-clock
thresholds on shared CI. Cold startup still waits for the read-only metadata
index before it can associate persisted pictures with games; cards and navigation
are available during that work. Measure cold-start and network behaviour in live
review rather than treating cache-unit-test numbers as end-to-end guarantees.

## Live review checklist (not yet signed off)

1. Start `emuwiz-v2` without closing any existing GUI. Home should explain six
   tasks and show an obvious Add My Games action on an empty collection.
2. At desktop and 1024×600 sizes, use sidebar, keyboard focus and Back/Home. Try
   640×480 as well. Check that primary actions are legible and visible.
3. Browse a real system in Platforms, switch grid/list, search and clear filters.
   Scroll quickly: cards must appear before pictures and navigation must respond.
4. Open a known game and an unknown one. Read the saved identity/file state and
   discovered-emulator wording; do not mistake installation for launch readiness.
5. Find Play, Verify, Mods, Fix and Open Folder. Deliberately hand off one workflow
   and verify its selected game, label and return route. Do not apply anything
   merely as part of exploration.
6. Open screenshots. Revisit the same game/browser and check texture reuse. Try
   unavailable/offline artwork and Retry; the page must remain usable.
7. Watch Activity while the library/artwork loads. Cancel artwork. Confirm a
   rescan only if desired, observe indeterminate progress and review any failures.
8. Explore for ten minutes. No provider activation, ROM changes or accidental
   installs should result. Restart v2 and confirm location/filter persistence.

Milestone 2 should migrate the complete native Check Games/Arcade journey first,
including its current local MAME-data joins, optional executable, ambiguity
confirmation, folder acceptance, real audit progress and understandable results.
Full launch readiness/execution, durable History and per-platform Mods remain
intentional future migrations, not extra backend development.

## Validation record

All commands used the shared Cargo target and `CARGO_INCREMENTAL=0`. No local
target directory or `/dev/shm` was used.

| Check | Result |
|---|---|
| Focused `gui_v2` tests | 34 passed |
| Existing GUI navigation filter | 29 passed; 1 known Converter naming failure |
| Existing GUI `database_and_catalogue` | 105 passed |
| Existing GUI `gamer_artwork` | 75 passed |
| Core library filter | 518 passed |
| Core artwork filter | 74 passed |
| Core media resolver filter | 7 passed |
| `cargo test -p archivefs-core --lib` | 9,518 passed; 3 ignored |
| `cargo test -p archivefs-gui --lib` | 2,771 passed; 3 known legacy failures; 2 ignored |
| Workspace/all-target/all-feature Clippy with `-D warnings` | Passed |
| Targeted rustfmt check, diff check and task postcheck | Passed |
| Release build, `emuwiz-v2 --version`, read-only measurement | Passed |
| Isolated native Xvfb startup, 1280×820 and 1024×600 screenshots | Passed; visually inspected |

The native startup check used a private virtual display and temporary
`EMUWIZ_CONFIG_HOME`/`EMUWIZ_DATA_HOME` roots. The sandbox initially refused access
to its X display; the same isolated check passed outside that sandbox. No
Sunshine session or existing GUI was killed/restarted. This first-run screenshot
check supplements the populated-library/rendering tests; it is not a substitute
for the real-user ten-minute exploration acceptance.

Unchanged pre-existing GUI failures (no code/test changes made to hide them):

- `health_and_platform_actions::library_renders_multiple_complete_rows_at_desktop_and_small_viewports`
  — Advanced Library 1024×600 layout.
- `platform_shelf_and_library_shell::every_navigation_destination_has_a_title_and_width_policy`
  — Converter vs Disc Conversion.
- `platform_shelf_and_library_shell::major_workflows_are_reachable_from_home_sidebar_and_top_menu`
  — Converter vs Disc Conversion.

Live user acceptance remains pending. Main is deliberately not fast-forwarded.
