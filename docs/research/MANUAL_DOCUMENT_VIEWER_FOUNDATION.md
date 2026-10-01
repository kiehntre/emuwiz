# Manual / strategy guide viewer: backend foundation

Branch `feature/manual-guide-viewer-foundation-current-main`. This delivers the
**backend and viewer state** for an internal PDF / CBZ / CBR viewer. It does not
deliver a viewer screen: see "GUI status".

## What already existed (reused, not duplicated)

| Existing | Where | Used how |
|---|---|---|
| Discovery, association, kind, open capability, reading state | `gui_v2/documents.rs` (`GameDocument`, `GameDocumentFormat`, ...) | Kept as the *discovery/association* layer. Its private PDF byte-scan counter, CBZ lister and natural sort were **removed**; `inspect_capability` now asks the new inspector. |
| Generic viewer input mapping | `ViewerInput` / `ViewerCommand` (was dead code) | Kept. `ViewerCommand::viewer_action()` maps it onto the new canonical actions. |
| External open via the desktop opener | `RommManual` opener seam, `xdg-open` probe | Unchanged. External open remains the path for anything the internal viewer cannot show. |
| `zip` 8.6, `image` (png/jpeg/webp), `flate2` | workspace deps | Used. **No new dependency was added.** |
| Natural-sort helper in core | none existed (only a private GUI one) | Defined once, in `manual_document::order`. |
| PDF reader, RAR reader | none in the workspace | Gaps, stated below. |
| Gamepad navigation infrastructure | only the gamer-view rail | Not touched; actions are device-agnostic. |

## Canonical model: `archivefs_core::manual_document`

`ManualDocumentKind` (Pdf, Cbz, Cbr), `ManualInspection`, `ManualDocument`
(opened, reads pages on demand), `ManualPage`, `ManualViewerError`,
`ManualReadiness`, `ManualCapabilityGap`, `ManualLimits`, `ManualViewerState`,
`ManualViewerAction`. Read-only throughout; nothing is extracted, executed or
written, and no file is created beside a document.

## Format status

| Format | Status |
|---|---|
| **PDF** | **Inspect only.** Structural reader (xref tables and streams, hybrid files, `/Prev` chains, object streams, trailing padding): declared page count, title/author/producer/date, encrypted state, active-content flags (`/OpenAction`, `/AA`, JavaScript names, embedded files, AcroForm) that are reported and **never run**. Rendering is an explicit gap: `ManualReadiness::InspectOnly { PdfRenderer }`. Damaged cross-reference data is refused, not reconstructed. Encrypted PDFs report `Encrypted`; the message does not claim a password is required, because many are owner-password-only. |
| **CBZ** | **Viewable.** Inspection plus on-demand page read and decode. |
| **CBR** | **Unsupported, named.** Recognised by RAR4/RAR5 signature (including a RAR named `.cbz`). `ManualReadiness::Unsupported { RarReader }`. Reading needs a RAR decompressor that is not a workspace dependency, and no tool is spawned without a supervised EmuWiz execution primitive. Nothing is bundled or downloaded. |

Detection is by content signature (`PK..`, `Rar!..`, `%PDF-` within the first
KiB); the extension is only recorded and a mismatch is reported as a warning.

## Resource bounds (`ManualLimits`, clamp-only)

| Bound | Default |
|---|---|
| Container file | 4 GiB |
| Archive members | 10,000 (checked against the end-of-central-directory declaration, ZIP64 included, before indexing) |
| One member / all members (declared uncompressed) | 64 MiB / 2 GiB, checked-add |
| Expansion ratio | 200:1 for members over 1 MiB (zero packed size also refused) |
| Pages | 10,000 |
| Image side / pixels / decoded RGBA bytes | 16,384 / 100 MP / 400 MB, checked from the header **before** decode |
| Member name / metadata string | 512 bytes / 256 chars |
| PDF: xref sections / objects / stream / object window / nesting / tail padding | 256 / 500,000 / 16 MiB / 4 MiB / 32 / 16 MiB |

Callers can only tighten limits, never exceed them.

## Safety rules (CBZ)

Refused: `..` traversal (including backslash forms), absolute and drive/UNC
paths, control characters, over-long names, symlink members, non-regular
members, duplicate names, nested archives, encrypted members, and decompression
bombs (including a bomb hidden in a non-page member). The `zip` crate silently
collapses duplicate names, so the declared count is compared with what it
indexed. Ignored without complaint: `ComicInfo.xml`, `Thumbs.db`, `__MACOSX/`,
dotfiles. GIF/BMP/TIFF/AVIF and similar are recognised but not decodable here:
skipped with a warning, or `UnsupportedImage` when nothing else is left. Pages
are checked against content as well as extension at decode time. The file's
identity (path, size, mtime, device/inode) is re-checked before every read.

## Page order (deterministic and total)

1. Root-level `cover` / `front` / `frontcover` (case and punctuation ignored,
   exact stem) first; `back` / `backcover` / `rearcover` last. A `cover.jpg` in
   a sub-folder is an ordinary page; `cover_art_2` and `discover` are not covers.
2. Everything else by natural path order, folder by folder: digit runs compare
   by value (any length, no overflow), so `1, 2, 3, 10` and `page02` before
   `page10`; text is case-insensitive; a number sorts before text; a prefix
   before a longer name.
3. Remaining ties break on the raw name bytes.

## Viewer state and controller-ready actions

`ManualViewerAction`: `NextPage`, `PreviousPage`, `FirstPage`, `LastPage`,
`ZoomIn`, `ZoomOut`, `FitWidth`, `FitPage`, `ToggleFullscreen`, `Close`.
`ManualViewerState` is `(document identity, page count, current page, zoom,
fullscreen)`. Page movement stops at both ends; zoom steps through
25-400% (fit modes step from 100%); a *different* document resets page and zoom
(fullscreen is a viewing mode and carries over); reopening the *same* file keeps
the position clamped to the page count; a file that changed at the same path is
a different document; `Close` resets everything; a closed viewer ignores all
actions. Nothing is persisted.

## GUI status: not wired (collision)

No viewer screen was added. Any surface needs a module registration and an
entry point in `gui_v2/mod.rs` and `gui_v2/pages.rs`, and the GUI crate's
`lib.rs`; all are being edited in other active worktrees right now (checked
twice). Per the task rules GUI wiring stopped there. Wiring recipe for later:
read `ManualDocument::open`, drive `ManualViewerState` from
`ViewerCommand::viewer_action()` plus a keyboard map, upload
`decode_page(state.current_page())` as a texture, and fall back to the existing
external open for `InspectOnly` / `Unsupported` documents.

## Validation against real files (read-only, nothing committed)

A temporary probe (removed before commit) inspected 23,389 real PDFs on this
machine, read-only: 22,860 inspected, 522 encrypted, 4 truncated downloads
(correctly refused), 3 not PDFs. Mean about 1.6 ms per file; slowest about 1 s
(debug build). It found two real-world cases the synthetic fixtures had missed,
both now handled and tested: PDFs padded with NULs to a round size, and update
chains of about 100 sections. No CBZ/CBR files exist on this machine, so CBZ was
validated with synthetic fixtures only. No personal document is a fixture.

## Remaining gaps

1. No embedded viewer screen (collision above).
2. No PDF page rendering (no renderer dependency).
3. No CBR reading (no RAR decompressor / supervised tool).
4. PDF page count is the page-tree root's declared `/Count`, not a walk of the tree.
5. Damaged PDF cross-reference data is refused, not reconstructed.
6. Encrypted PDFs are not decrypted (including empty-password ones).
7. No reading-position persistence beyond the existing `DocumentReadingState`.
8. Discovery still finds candidates by extension (cheap); the content check
   happens at inspection.
