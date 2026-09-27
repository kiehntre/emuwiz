# Manuals & Guides Completion Audit

Research only. No production code was changed by this audit. Starting SHA
`33ecab378c54cfdfa4dd0303e1008b114b920878` (main), audited from research
branch `research/manuals-guides-completion-audit` in an isolated worktree.

## CURRENT ARCHITECTURE

There are **two separate, unbridged manual subsystems** in the codebase today:

1. **GUI v2 local documents foundation** — `crates/archivefs-gui/src/gui_v2/documents.rs`
   (634 lines). Defines `GameDocument`, `GameDocumentKind`,
   `GameDocumentFormat`, `GameDocumentSource`, `GameDocumentAssociation`,
   `GameDocumentViewerCapability`, `DocumentReadingState`,
   `DocumentPreferences`, `ViewerInput`/`ViewerCommand`. Wired into Game
   Details via `documents_panel` in
   `crates/archivefs-gui/src/gui_v2/pages.rs:1876-1987`, into preferences
   persistence via `crates/archivefs-gui/src/gui_v2/backend.rs:37-57`
   (`Preferences::document_roots/document_associations/document_reading`)
   and `crates/archivefs-gui/src/gui_v2/mod.rs:276-277,349-350,1367-1372,1552-1554`,
   and into a Settings root picker at `pages.rs:2489-2514`. Opening a
   document goes through `Command::OpenDocument` in `backend.rs:610-620`,
   which delegates to `archivefs_core::identity_source::romm::manual::DesktopManualOpener`.

2. **RomM candidate-preview manual opener** — `crates/archivefs-core/src/identity_source/romm/manual.rs`
   (215 lines), `crates/archivefs-gui/src/romm_game.rs:71-91,153,229-230,354,
   441-444,952,1105,1139,1466-1512+`, and
   `crates/archivefs-gui/src/romm_operation_controller.rs:380-388,659`. This
   is a *different* flow: it opens a manual associated with a **RomM catalogue
   candidate** (a game not necessarily imported/local yet), validated through
   `ValidatedRommMediaMapping`/`resolve_romm_media_reference` (path-traversal
   and root-escape safe, see `manual.rs` tests at lines 160-183). It reuses
   `DesktopManualOpener` as its OS-open primitive, but it is **not** a
   `GameDocument` and never enters `discover_documents`, association
   tracking, resume state, or the Game Details "Manuals & Guides" panel.

Gap: a manual pulled in through RomM (`open_local_romm_manual`) does not
automatically appear as a `GameDocument` even if it lands inside the game's
directory tree, unless the local discovery heuristics in
`documents.rs:242-336` independently rediscover the same file by filename/
directory match. There is no explicit bridge and no shared identity key
between the two systems.

## DOCUMENT ASSOCIATION

`GameDocumentAssociation` (`documents.rs:93-116`) is an ordered precedence
enum: `Explicit > ExactGameIdentity > SameGameDirectory > ExactTitle >
PlatformAndTitle > WeakFilename > Unmatched`. However `ExactGameIdentity` is
**declared but never produced** — `discover_documents` (`documents.rs:215-347`)
only ever assigns `Explicit`, `SameGameDirectory`, `PlatformAndTitle`,
`ExactTitle`, `WeakFilename`, or `Unmatched` (see the if/else chain at
lines 289-303). `WeakFilename` and `Unmatched` results are explicitly
`continue`d past (lines 304-309) — **never surfaced, never auto-linked** —
which matches the "never silently bind ambiguous matches" requirement and is
verified by the `weak_ambiguous_title_is_not_auto_linked` test
(`documents.rs:593-612`).

Classification: **PARTIAL**. Filename/directory heuristics work and are
safely conservative. Strong evidence tiers that the rest of the codebase
already has available — `GameId` database identity, no-intro/DAT-verified
identity (`crates/archivefs-core/src/game_identity.rs`,
`identity_source/no_intro/*`), exact serial/product-code identity — are
**not wired in**. The `ExactGameIdentity` variant is a placeholder with no
producer, i.e. the precedence model anticipates the gap but the
implementation stops one tier short of the strongest evidence EmuWiz already
computes elsewhere for other features (organisation, rename-apply,
verified-identity diagnostics).

Explicit user association (`Command`-driven "Associate"/"Remove association"
buttons, `pages.rs:1961-1973`) is fully functional, in-memory + persisted
through `DocumentPreferences.associations: BTreeMap<PathBuf, i64>`
(`documents.rs:199-203`), and correctly takes precedence over everything
(`explicit` check at `documents.rs:272-280,289-290`).

## FORMAT MATRIX

| Format | OPEN | INDEX | PAGE COUNT | RESUME | CONTROLLER NAV | THUMBNAIL | SEARCH | EXTERNAL VIEWER | INTERNAL VIEWER |
|---|---|---|---|---|---|---|---|---|---|
| PDF | EXTERNAL-ONLY | N/A | PARTIAL (heuristic) | PARTIAL (page number only, no seek-to-page) | MISSING | MISSING | MISSING | COMPLETE | MISSING |
| CBZ | EXTERNAL-ONLY | COMPLETE (metadata-only) | COMPLETE | PARTIAL (page number only) | MISSING | MISSING | MISSING | COMPLETE (opens raw .cbz via OS handler, not guaranteed to exist) | MISSING |
| CBR | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING | MISSING (deliberately blocked, `ExternalViewerUnavailable`) | MISSING |
| EPUB | not recognised — no extension match in `GameDocumentFormat::from_path` (`documents.rs:56-69`) | — | — | — | — | — | — | — | — |
| TXT/HTML | not recognised, same reason | — | — | — | — | — | — | — | — |
| Scanned image folders | not recognised — discovery only enumerates files matching PDF/CBZ/CBR extensions (`is_document_path`, `documents.rs:349-354`); a bare folder of page images is invisible to discovery | — | — | — | — | — | — | — | — |

No internal renderer exists for any format; "Open" always shells out to the
OS default handler (`ManualOpener::open` in `romm/manual.rs:69-91`, called
via `backend.rs:616-618`). The `ViewerInput`/`ViewerCommand` enums and
`map_viewer_input` (`documents.rs:152-196`) are `#[allow(dead_code)]` and are
**not referenced anywhere outside `documents.rs` itself** (verified by
grep across `crates/archivefs-gui/src`) — they are a naming/mapping
scaffold only, with zero actual controller or keyboard wiring.

## PDF

- Recognition: extension-only (`documents.rs:64`).
- Page count: `pdf_page_count` (`documents.rs:414-426`) reads only the
  **first 4 MiB** of the file and counts raw byte occurrences of the
  literal pattern `b"/Type /Page"` not immediately followed by `s`. This is
  a heuristic, not a PDF object-graph parser. It will silently
  undercount/miscount on any PDF using compressed cross-reference streams or
  **compressed object streams** (`/Type /ObjStm`), which is the default
  output of most modern PDF producers (Ghostscript, many scanners, LibreOffice
  since ~2015) — the `/Type /Page` markers of interest are inside a
  zlib-compressed stream and invisible to a raw byte scan. It will also
  silently miscount if `/Type/Page` occurs without exact whitespace, or if a
  document exceeds 4 MiB before its page objects appear (common in large
  scanned manuals, since object definitions are often near the end/xref
  area). No PDF parsing crate is used — appropriately, per the constraint
  not to pull in a heavyweight renderer, but the audit needs to flag that the
  "page count" claim in the UI (`pages.rs:1915`) can be silently wrong for a
  large fraction of real-world PDFs, and there is no "unknown" fallback
  differentiation between "no pages found" and "heuristic failed" — both
  produce `None` (line 425: `(count > 0).then_some(count)`).
- Open-at-a-specific-page: **not supported**. `xdg-open`/`open`/`explorer`
  (`romm/manual.rs:71-89`) take only the file path; there is no
  `#page=N` fragment or reader-specific CLI flag construction. The "Resume
  page" UI (`pages.rs:1925-1954`) only *records* a page number for display —
  it cannot instruct the external viewer to open there. This is a real UX
  gap: EmuWiz shows "Resume at page 14" but opening the document always
  starts the external viewer at whatever page that viewer itself last
  remembered (if any), not the value EmuWiz tracked.
- Viewer-missing handling: `DesktopManualOpener::open` treats a
  nonzero/failed exit status as an error (`romm/manual.rs:82-89`) and
  surfaces it through `Result<(), String>` → GUI activity/error toast
  (`backend.rs:616-618` `.map_err`). This is adequate for "no viewer
  registered" on Linux (`xdg-open` exits nonzero) but was not verified
  against Windows/macOS "no association" behavior (those tools sometimes
  exit 0 even when nothing visibly opens, e.g. Windows `explorer.exe`
  historically returns 1 for many invocations regardless of outcome — this
  is a known cross-platform caveat, not fixed in this code).
- Sandbox/path safety: `Command::OpenDocument` requires the path be an
  existing regular absolute file before invoking the opener
  (`backend.rs:610-615`); no working-directory or environment sandboxing is
  applied to the spawned process itself (relies entirely on the OS opener).
  This mirrors the RomM manual path's approach and is consistent with
  "trusted local file, no argument injection surface since the path is the
  sole argument".
- Controller implications: none — PDF is 100% external-viewer, so EmuWiz
  controller mappings cannot reach it at all (confirmed no external-viewer
  focus/embedding code exists).

**External viewer remains the right short-term design.** The audit found no
evidence justifying pulling in a PDF rendering stack — there is no existing
embedding surface (no egui image/texture pipeline hooked to documents), and
the page-count heuristic, while imperfect, is cheap and non-blocking. The
gap is UX honesty (differentiate "count unknown" vs "count possibly wrong")
and the missing open-at-page capability, not the external-viewer choice
itself.

## CBZ

- Recognition + read-only ZIP metadata inspection: `inspect_cbz`
  (`documents.rs:428-463`), using `zip = "8.6"` with `default-features =
  false, features = ["deflate"]` (`crates/archivefs-gui/Cargo.toml:92`).
- Bounded entry count: `archive.len() > MAX_CBZ_ENTRIES` (10,000) refused
  (`documents.rs:432-434`). **No bound on uncompressed size per entry or
  total** — `inspect_cbz` never reads entry contents (only `entry.name()`),
  so a decompression-bomb risk does not apply to *indexing*, but nothing in
  this module bounds what would happen if/when an internal viewer is later
  added to actually decode these images; that safeguard does not exist yet
  and needs to be designed before any real decode path is added.
- Safe filenames: traversal (`..`) and absolute-path entries are rejected
  (`documents.rs:442-448`), covered by
  `unsafe_cbz_entries_are_refused` (`documents.rs:534-542`).
- Image ordering: natural sort (`natural_sort_key`, `documents.rs:465-496`)
  correctly orders `page1, page2, page10` (test `cbz_pages_use_natural_order`,
  `documents.rs:524-533`).
- Page count: `inspect_cbz(path).ok().map(|pages| pages.len())`
  (`documents.rs:399-402`) — accurate, since it is a straightforward ZIP
  central-directory read, not a heuristic.
- Malformed archive handling: `ZipArchive::new` failure is mapped to an
  `Err` string (`documents.rs:430-431`), test
  `malformed_cbz_and_unsupported_cbr_are_safe` (`documents.rs:543-553`).
- Extraction: **never extracts to disk** — confirmed, `inspect_cbz` only
  enumerates names via `by_index`; no `io::copy` or file write path exists
  in this module for CBZ contents.
- Resume page: same generic `DocumentReadingState`/`safe_resume_page`
  mechanism as PDF (see RESUME STATE below) — page count is *accurate* here
  (unlike PDF), so resume invalidation is strictly correct for CBZ.
- **Critical gap**: there is no internal image viewer. "Open" on a CBZ
  hands the raw `.cbz` file to the OS opener (`backend.rs:616-618`), which
  will fail or do nothing useful unless the user's OS has a comic-book
  reader registered for `.cbz` — most desktop Linux/Windows/macOS installs
  do **not** have one by default. This is the single biggest disconnect
  between what the code *can* tell the user (accurate page count, ordering)
  and what it can *do* (open it usefully). CBZ's `GameDocumentViewerCapability`
  is set to `ExternalViewer` (`documents.rs:399-402`) even though there is
  no verification that an external CBZ-capable viewer is actually
  registered — contrast with PDF, where `xdg-open` on most desktops *is*
  reliably associated. This makes the "Open" button optimistic for CBZ in a
  way it usually isn't for PDF.

## CBR

Recognized-only. `GameDocumentFormat::Cbr` is detected by extension
(`documents.rs:66`) but `inspect_capability` immediately returns
`(None, GameDocumentViewerCapability::ExternalViewerUnavailable)`
(`documents.rs:403-406`) — no RAR parsing occurs anywhere, confirmed by
grep (`rar`/`unrar` do not appear as dependencies in
`crates/archivefs-gui/Cargo.toml` or `crates/archivefs-core/Cargo.toml`).
The "Open" button is force-disabled for CBR
(`can_open` check, `pages.rs:1956-1960`) and the panel shows "No supported
local viewer" (`pages.rs:1974-1976`). This is **deliberately and safely
blocked**, not silently broken — test coverage exists
(`malformed_cbz_and_unsupported_cbr_are_safe`, `documents.rs:544-553`).

Smallest safe completion path: CBR is proprietary RAR format; no pure-Rust
unrar-compatible crate is vetted in this codebase today. Two low-risk
options for a future increment, neither implemented now: (1) shell out to a
system `unrar`/`unar` binary if present, purely to *open* the file (same
trust model as `xdg-open`, no in-process RAR parsing, so no new memory-safety
surface) with capability gated on a detected binary; (2) treat CBR
permanently as "recognized, externally-openable only" (skip indexing/page
count entirely) and document that RAR indexing is out of scope until a
licensed/audited RAR crate exists. Given the project's stated aversion to
new heavyweight/unaudited dependencies, option (2) plus a clearer message
("Install an unrar-capable comic viewer to open this file yourself outside
EmuWiz") is the lower-risk quick win; option (1) is a reasonable P1 if a
system-binary detection pattern already exists elsewhere in the codebase
(it does, in spirit, via `DesktopManualOpener`'s program selection).

## RESUME STATE

Keying model: **canonical file path** (`BTreeMap<PathBuf, DocumentReadingState>`
in `DocumentPreferences.reading`, `documents.rs:199-203`; discovery inserts
under `canonical` path at `documents.rs:317-334`). Not keyed by `GameId`,
not keyed by document hash/content.

`safe_resume_page` (`documents.rs:498-504`):
```rust
pub(crate) fn safe_resume_page(
    state: Option<&DocumentReadingState>,
    page_count: Option<usize>,
) -> Option<usize> {
    let page = state.and_then(|state| state.last_page)?;
    (page > 0 && page_count.is_none_or(|count| page <= count)).then_some(page)
}
```
Behavior by scenario:
- **Document changes** (same path, different content/page count): for CBZ,
  correctly invalidated if new page count < saved page (test
  `changed_document_invalidates_resume_page`, `documents.rs:614-621`). For
  PDF, protection is only as good as the heuristic page count — if the
  heuristic under/over-counts, a stale resume page can pass the `<= count`
  check while pointing at genuinely different content. **This is a real gap
  inherited directly from PDF page-count inaccuracy**, not from the resume
  logic itself.
- **Document disappears**: `discover_documents` simply won't produce that
  `GameDocument` again (file no longer readable via `fs::read_dir`/
  `fs::metadata`, `documents.rs:243-259`), so the panel shows nothing for it
  — but the **stale `DocumentReadingState` entry in preferences is never
  pruned**. It persists indefinitely in `document_reading` even though
  nothing references it. Not harmful (unbounded but small growth, keyed by
  path) but a minor cleanliness gap — no GC path exists.
- **Multiple manuals for one game**: each document has its own independent
  `DocumentReadingState` keyed by its own path — correctly independent, no
  interference between multiple manuals' resume points.
- **User changes edition/language**: since resume is keyed by path, not by
  "logical manual for this game", switching to a different edition/language
  file is treated as an entirely separate document with its own resume state
  — this is actually the *safe* default (no false resume), but it also means
  there is no concept of "the same logical manual, different edition" to
  carry a language preference across editions; see LANGUAGE/REGION below.

Classification: **PARTIAL**. The invalidation logic itself is sound and
tested; the weakness is entirely downstream of PDF page-count accuracy and
the lack of any GC for orphaned reading-state entries.

## MULTIPLE DOCUMENTS

`GameDocumentKind` (`documents.rs:22-46`) already models the requested
taxonomy: `Manual, StrategyGuide, ReferenceCard, Map, Magazine, Walkthrough,
Other`. `infer_kind` (`documents.rs:371-388`) assigns kind via a **filename
substring heuristic only** ("strategy"/"prima"/"guide" → StrategyGuide,
"map" → Map, "reference"/"card" → ReferenceCard, "magazine"/"monthly" →
Magazine, "walkthrough"/"hint" → Walkthrough, "manual"/"instruction" →
Manual, else Other). This is exactly the "flimsy filename heuristics" the
task description warns against relying on for anything beyond a soft label
— today it's used only as a **cosmetic label** in the panel (`ui.weak(document.kind.label())`,
`pages.rs:1916`), never as a filter/grouping key, never gating behavior, and
never blocking association. That is an appropriately low-stakes use of a
weak signal. There is **no explicit user-classification override** for
`kind` — a user cannot correct a mislabeled "Other" to "Strategy Guide"
today; only association (which game it belongs to) is user-editable, not
kind. Multiple documents for one game are already fully supported by the
data model (`Vec<GameDocument>` per game, no uniqueness constraint) and
rendered as a list (`pages.rs:1911-1985` loop).

Classification: **PARTIAL** — taxonomy and multi-document storage/display
exist; explicit user (re)classification of `kind` does not.

## LANGUAGE / REGION

**MISSING.** `GameDocument` (`documents.rs:118-132`) has no `language`,
`region`, `revision`, or `edition` field. Nothing in discovery parses
language/region hints from filenames or paths (grep confirms no
locale/language-code handling in `documents.rs`). Game Details therefore
cannot choose a "preferred" document by language — it lists all matched
documents without any language differentiation, sorted only by
`(association, normalized title, path)` (`documents.rs:338-344`). This
satisfies "never hides alternatives" by default (nothing is hidden — there's
no filtering at all) but provides no "preferred" selection mechanism, and no
edition/region metadata for a user to react to. This is a clean, additive
gap: the association/discovery model doesn't need restructuring, just an
optional metadata field plus (later) a lightweight filename/user-set tag and
a "preferred edition" pointer in `DocumentPreferences`.

## GAME DETAILS UX

Current panel (`documents_panel`, `pages.rs:1876-1987`) already renders,
per document: title, format + page count (or "page count unknown"), kind
label, association reason + source + full path, resume-page hint, an
editable "Resume page" `DragValue` (bounded to `1..=page_count`), Open
(enabled only when `viewer == ExternalViewer`), Associate/Remove
association, "No supported local viewer" note for CBR, and a nested
"View details" section (file size, provenance, association confidence).
Empty state: `"No manuals or guides are linked to this game yet."`
(`pages.rs:1908`) — appropriately neutral, matches the requested tone. The
requested compact summary format ("Manual: Available / Strategy guides: 2 /
Last read: Page 14 / Actions: Open / Choose document / Resume") is **not**
what exists today — today's panel is a full per-document list, always
expanded per item inside a collapsed "Manuals & Guides" outer section, with
no roll-up counts by kind and no single "Choose document" picker action (a
user scrolls the list rather than picking from a compact summary + expand).
This is a legitimate P1 UX gap: the underlying data (`kind`, counts,
per-document resume state) is already sufficient to build the requested
compact summary without any new discovery/association logic — it is a
presentation-layer-only change.

## CONTROLLER UX

**MISSING** beyond a disconnected type-level mapping. `ViewerInput` →
`ViewerCommand` (`documents.rs:154-196`, `map_viewer_input`) is pure,
tested (`controller_mapping_is_vendor_neutral`, `documents.rs:622-633`), and
**entirely unused** outside its own module (confirmed via grep across all of
`crates/archivefs-gui/src`). There is no gamepad/controller input system
found anywhere in `gui_v2` at all (`grep -rl gamepad` returns nothing) — the
whole GUI v2 surface is currently mouse/keyboard egui, so "controller
navigation" for manuals has no host system to plug into yet, independent of
the documents feature. Internal CBZ viewer path and external PDF viewer
path are correctly *conceptually* separated in the doc/mapping (the
`ViewerCommand` variants are viewer-agnostic), but since there is no
internal viewer for CBZ either, in practice **both paths are external
today**, and external apps do not obey EmuWiz controller mappings — the
existing research doc (`docs/research/MANUAL_STRATEGY_GUIDE_VIEWER.md:61-65`)
already states this plainly and the code matches: "delegates rendering to
the desktop viewer... a foundation for a later embedded CBZ/PDF surface
rather than pretending to control an external application."

## CONSOLE MODE FUTURE

No console/big-picture/gamepad-first mode exists anywhere in the GUI today
(confirmed via grep for `console_mode`/`ConsoleMode`/`big_picture`/
`BigPicture` — zero hits). For manuals to be usable in a future console mode
without falling back to a desktop file-association dialog (which typically
doesn't exist/work well in a locked-down console-style session), EmuWiz
would need, at minimum: (1) an internal image-sequence viewer for CBZ (the
one format where all the safety/indexing groundwork already exists —
ordered pages, bounded entries, no extraction), since PDF external-viewer
handoff is fundamentally incompatible with a controller-only, no-desktop
console session; (2) an actual gamepad input layer wired to
`ViewerInput`/`ViewerCommand` (today purely theoretical); (3) an
overlay/pause model to suspend/resume the running emulator around document
viewing, which does not exist in any form today (no pause/overlay hooks
found for any GUI feature, let alone manuals). This section is intentionally
research-only per the task; no implementation is proposed here.

## PERFORMANCE

- **500+ page CBZ**: `inspect_cbz` only reads ZIP central-directory metadata
  (names/sizes), never decompresses entries, so indexing itself scales fine
  even for very large archives up to the 10,000-entry cap
  (`documents.rs:432-434`). The real risk is entirely in a *future* internal
  viewer that would need to decode images — that path doesn't exist yet, so
  the risk is latent, not present. Recommend the eventual viewer decode
  lazily (one page at a time, generous LRU cache) rather than pre-decoding
  the whole archive.
- **Huge scanned PDFs**: `pdf_page_count` bounds itself to the first 4 MiB
  read (`documents.rs:417-419` `take(4 * 1024 * 1024)`), so it cannot be
  used as a resource-exhaustion vector regardless of file size — but, as
  noted, it becomes systematically less accurate for exactly the large,
  scanned manuals this feature targets (page markers pushed past the 4 MiB
  window or hidden behind compressed object streams).
- **Directory scanning**: bounded per directory
  (`MAX_FILES_PER_DIRECTORY = 512`), bounded total
  (`MAX_DOCUMENTS = 256`), bounded roots (`MAX_ROOTS = 16`)
  (`documents.rs:17-19`), no recursive descent — this is already a
  well-bounded, lazy-ish design; discovery reruns on every game selection
  change though (`needs_discovery` cache keyed by `(game_id, path)`,
  `pages.rs:1884-1899`) — cache is per-selected-game only, not a shared
  cross-game cache, so re-selecting games repeatedly re-walks directories
  each time (bounded cost per walk, but no memoization across visits in a
  session beyond the currently-selected game).
- **Thumbnail/cover generation**: does not exist at all today (see FORMAT
  MATRIX), so there is no current thumbnail-cache growth risk — this is a
  "when added" concern, not a present one. Recommend any future thumbnail
  feature cache to a bounded on-disk cache directory with an LRU eviction
  policy, keyed by canonical path + mtime + size (to naturally invalidate on
  document replacement), consistent with how `safe_resume_page` already
  treats page-count changes as invalidation signals.
- **Cache growth**: `document_reading` (resume state) and
  `document_associations` grow unboundedly with no pruning of entries for
  files that no longer exist (see RESUME STATE) — low absolute risk (tiny
  per-entry size, PathBuf + small struct) but worth a bounded/pruned design
  before this feature sees heavy multi-year use across large libraries.

## LEGAL / SOURCE MODEL

Confirmed via grep across `crates/` (`-i` for `replacementdocs|gamefaqs|
archive.org.*manual|download.*manual`) that **no scraping, remote-fetch, or
provider-download code exists for manuals/guides anywhere in the codebase**.
The only "remote" manual concept is `MediaReference.hosted_reference` in
`crates/archivefs-core/src/identity_source/model.rs`, which is RomM's own
already-configured/self-hosted media mapping (`ValidatedRommMediaMapping`),
resolved only against a **user-configured local root** the user already
trusts and pointed EmuWiz at (`resolve_romm_media_reference`,
`identity_source/romm/media_mapping.rs`) — not a scrape of a third-party
site. `open_local_romm_manual` explicitly documents "Remote manual URLs
remain metadata until a dedicated, approved URL-opening design exists"
(`romm/manual.rs:3-4`), i.e. the code itself already refuses to act on a
`public_reference` URL (test `no_mapping_and_public_only_reference_are_refused_without_opening`,
`romm/manual.rs:185-198`). This is fully consistent with the "local/
user-provided documents only" constraint. Any future "provider support"
(e.g. a manuals marketplace or fetch-from-source feature) would need its own
explicit licensing review before implementation — nothing in the current
architecture presumes or half-implements such a thing, so there is no
existing code to audit for licensing risk beyond what's described above.

## P0 GAPS

1. **CBZ "Open" is optimistic with no internal viewer and no OS-association
   check.** `GameDocumentViewerCapability::ExternalViewer` is granted
   unconditionally for CBZ (`documents.rs:399-402`) even though most
   desktops have no `.cbz` handler; users will click "Open" and get a
   failure or nothing. (`documents.rs:399-402`, `backend.rs:610-620`)
2. **PDF page count is a byte-pattern heuristic that misses compressed
   object streams**, a very common modern-PDF encoding — silently wrong
   page counts feed directly into resume-page safety bounds.
   (`documents.rs:414-426`)
3. **No bridge between the RomM manual-open pathway and the local
   `GameDocument` association/discovery/resume system** — two parallel,
   duplicate-effort manual features with no shared model.
   (`romm/manual.rs` vs `gui_v2/documents.rs`)
4. **No strong-identity association tier is actually produced.**
   `GameDocumentAssociation::ExactGameIdentity` exists in the enum
   (`documents.rs:96`) as the intended strongest non-explicit tier but has
   zero producers in `discover_documents` — the feature never uses the
   game-identity/DAT evidence the rest of the codebase already computes.
   (`documents.rs:215-347`)

## P1 GAPS

1. Game Details UX does not match the requested compact summary form
   (Manual: Available / count / Last read / Open / Choose document /
   Resume) — currently a flat always-detailed list. (`pages.rs:1876-1987`)
2. No language/region/edition/revision metadata field or preferred-edition
   selection. (`documents.rs:118-132`)
3. No user-editable `GameDocumentKind` override — classification is
   filename-heuristic-only and not correctable. (`documents.rs:371-388`)
4. No PDF open-at-specific-page capability — resume page is tracked but
   cannot actually be handed to the external viewer. (`romm/manual.rs:69-91`)
5. No thumbnail/cover extraction for any format. (confirmed absent via grep)
6. Stale `document_reading`/`document_associations` entries for deleted
   files are never pruned. (`documents.rs:199-203`, no GC path found)
7. CBR has no smallest-safe-completion path implemented (system-binary
   detection for opening only) — currently fully blocked.
   (`documents.rs:403-406`)

## QUICK WINS

1. Gate CBZ's `ExternalViewer` capability on an actual OS-association
   probe (or relabel the button/tooltip to "Open (needs a comic viewer)"
   when unverified) — no discovery-model change needed, just a truthful
   capability signal. (`documents.rs:399-402`)
2. Differentiate "page count unknown" from "page count possibly
   approximate" in the PDF UI label, given the heuristic's known
   limitations, without touching the counting logic itself. (`pages.rs:1915`)
3. Add pruning of `document_reading`/`document_associations` entries whose
   path no longer exists, run opportunistically alongside `discover_documents`
   for the currently selected game (bounded, no new filesystem walk).
   (`documents.rs:199-203`)
4. Add an explicit "Document type" override control next to "Associate"
   using the existing `GameDocumentKind` enum — no new discovery logic,
   just a persisted per-path override read before `infer_kind` is applied.
   (`documents.rs:371-388`, `pages.rs:1911-1985`)

## IMPLEMENTATION SEAMS

- `DocumentDiscoveryRequest`/`discover_documents` (`documents.rs:206-347`)
  is the single seam for adding a stronger `ExactGameIdentity` producer —
  it already receives `game_id`; adding a DAT/verified-identity lookup
  input here (rather than changing the precedence enum) is the natural
  extension point.
- `GameDocument` (`documents.rs:118-132`) is the seam for adding
  `language`/`region`/`edition` fields — it is already the per-document
  record, additive fields only.
- `DocumentPreferences` (`documents.rs:198-203`) is the seam for a
  "preferred edition per game" pointer and for kind-override persistence —
  same `BTreeMap<PathBuf, _>` pattern already used for associations/reading.
- `ManualOpener` trait (`romm/manual.rs:62-64`) is already the correct
  abstraction seam for adding a page-aware opener (e.g. PDF `#page=N`
  fragment construction) or a future CBR-via-system-binary opener without
  touching the discovery/association code at all.
- `documents_panel` (`pages.rs:1876-1987`) is the seam for the compact
  Game-Details summary — the data needed (kind counts, last-read page,
  association state) is already computed per document; this would be a
  presentation-only refactor.
- The two-subsystem bridge (RomM manual open ↔ local `GameDocument`
  discovery) would most naturally live as an optional "seed association"
  call from `romm_operation_controller.rs:380-388` into
  `DocumentPreferences.associations`, once a manual file is confirmed local
  — no changes to either existing safety model required.

## TEST PLAN

1. **One PDF manual** — discovery finds exactly one `GameDocument` with
   `format == Pdf`; open triggers `DesktopManualOpener`. Covered today by
   `pdf_discovery_and_page_count_are_local_and_read_only`
   (`documents.rs:555-575`).
2. **Multiple manuals** — a game directory with 2+ recognized documents
   yields a `Vec<GameDocument>` of matching length, each independently
   associated and independently resumable. Not directly tested today
   (existing tests use single-document fixtures); needs a new test.
3. **Exact GameId association** — once `ExactGameIdentity` gets a producer
   (P0 gap #4), needs a dedicated precedence test proving it outranks
   `SameGameDirectory`/`ExactTitle`. No producer exists today, so no test
   exists; write test alongside the implementation.
4. **Ambiguous filename** — covered today by
   `weak_ambiguous_title_is_not_auto_linked` (`documents.rs:593-612`),
   proving zero documents are returned rather than a low-confidence guess.
5. **CBZ ordered pages** — covered today by `cbz_pages_use_natural_order`
   (`documents.rs:524-533`).
6. **Malformed CBZ** — covered today by
   `malformed_cbz_and_unsupported_cbr_are_safe` (`documents.rs:543-553`,
   first half).
7. **Traversal archive member** — covered today by
   `unsafe_cbz_entries_are_refused` (`documents.rs:534-542`), both `../`
   and absolute-path cases.
8. **CBR without unrar** — covered today (implicitly) by the same
   `malformed_cbz_and_unsupported_cbr_are_safe` test's second half
   (`documents.rs:549-552`), asserting `ExternalViewerUnavailable`.
9. **CBR with supported backend** — no such backend exists; this case has
   no code path to test yet. Write once a system-binary/backend detection
   is implemented (P1 gap #7).
10. **Resume page** — covered today by `changed_document_invalidates_resume_page`
    (`documents.rs:614-621`), for the "resume is valid" half; add a
    positive-path test (page within bounds is honored end-to-end through
    `documents_panel`'s "Resume at page N" label).
11. **Document replaced** (same path, shorter/rewritten document) —
    partially covered for CBZ via the page-count-shrink case in
    `changed_document_invalidates_resume_page`; **not covered for PDF**,
    since PDF page count is heuristic — needs a test proving a PDF
    replaced-with-fewer-pages scenario using the same crude counting logic
    (documenting the known heuristic limitation rather than asserting
    perfect correctness).
12. **Document missing** (path in `document_associations`/`document_reading`
    but file deleted) — no current test; needs one asserting (a) the
    document no longer appears in `discover_documents`'s output and (b) a
    future pruning pass removes the orphaned preference entries (once quick
    win #3 lands).
13. **Different language manuals** — no `language` field exists yet; no
    test possible until LANGUAGE/REGION gap is addressed. Write once the
    field exists, asserting both appear un-hidden and a "preferred" pointer
    can select one without removing the other from the list.
14. **Huge document** — CBZ: add a test with an archive near
    `MAX_CBZ_ENTRIES` (10,000) proving indexing completes and stays
    read-only; PDF: add a test with a file >4 MiB proving `pdf_page_count`
    degrades to `None` rather than panicking/hanging, confirming the bound
    at `documents.rs:417-419` is actually load-bearing.
15. **Game Details return context** — after "Open" is invoked and the
    external viewer session ends, verify the GUI route/selection state is
    unchanged (still on the same game's detail page) — this is really a
    property of `Route::Game(game_id)` handling in `pages.rs:1958`/activity
    queueing, not of `documents.rs`; add a GUI-level test asserting the
    route is preserved across an `OpenDocument` command round-trip.
16. **External viewer unavailable** — add a test around
    `DesktopManualOpener::open` (or a fake `ManualOpener`) asserting a
    nonzero/failed process status surfaces as a clean, actionable
    `Err(String)` rather than a panic; the existing `opener_failure_is_returned_cleanly`
    test (`romm/manual.rs:200-214`) covers this for the RomM path via a
    `FakeOpener` — extend an equivalent test for the `gui_v2::documents`
    → `Command::OpenDocument` path, since that call site
    (`backend.rs:610-620`) is currently untested end-to-end.

## IMPLEMENTATION ORDER

1. P0 #1 — stop over-promising CBZ "Open" (truthful capability signal);
   lowest risk, highest user-trust payoff, no model changes.
2. P0 #2 — either accept and clearly label the PDF page-count heuristic's
   limitations, or invest in a minimal, still-heuristic-but-broader parser
   (e.g. also scan xref stream markers) — scope this as its own small
   research spike before committing to an approach.
3. P0 #4 — wire a real `ExactGameIdentity` producer using existing
   game-identity/DAT evidence; this strengthens association without any UI
   change (the precedence model already expects it).
4. P1 quick wins (kind override, pruning of orphaned preference entries) —
   cheap, additive, no architecture change.
5. P0 #3 — design (not necessarily implement in the same pass) the bridge
   between RomM manual-open and local `GameDocument` association, since it
   touches two different crates/teams' surfaces and deserves its own
   focused change.
6. P1 — Game Details compact-summary UX refactor (presentation-only,
   sequence after the data-model gaps above so the summary reflects
   accurate counts/kinds).
7. P1 — language/region/edition metadata plus preferred-edition selection.
8. P1 — PDF open-at-page and CBR smallest-safe-completion path (system
   binary detection), lowest priority since both require new external
   integration surface and more careful, separately reviewed design.
9. Console-mode prerequisites (internal CBZ viewer, gamepad input layer,
   pause/overlay model) — explicitly out of scope for near-term work per
   the task; revisit only after the above land and a console-mode product
   decision is made elsewhere.
