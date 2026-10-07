# Manual Viewer V1

Game → Manuals & Guides offers internal CBZ/PDF reading and the existing
explicit desktop-open action. The viewer consumes `GameDocument`; discovery,
association precedence and the local RomM mapping remain authoritative.
WeakFilename/Unmatched records cannot initiate either viewer action. CBR stays
recognised-but-unsupported. No downloading, fuzzy matching or extraction is added.

`gui_v2::manual_viewer` owns presentation and async coordination. The salvaged
CBZ concepts are a single worker, single pending-request/result slots,
generation-tagged replies, core viewer actions, texture ownership, fit/zoom and
keyboard routing. The current core CBZ reader supplies bounded enumeration,
natural page order and requested-member-only decoding. Corrupt pages can be
skipped. Stale generation/document/page replies cannot replace the current page.
Closing invalidates pending work and refreshes document discovery without
changing the game route/history.

PDF structural inspection and `ManualDocument::decode_page` retain their
existing contracts. `manual_document::pdf_render` is a separate rendering seam:
Hayro 0.8.0 (MIT OR Apache-2.0), with built-in fonts/CMaps and no external resource
loader. Each requested page is rendered by a fresh process of the native GUI
executable, entered before logging, database or window initialization. The
existing bounded inspector and verified page-tree index run before rasterization;
the renderer's page count must agree with that index. Actions, scripts,
attachments and URLs are never executed/opened.

Hayro's decoding allocations have no configurable total memory bound. V1
therefore enables internal PDF rendering only on Linux, where the child installs
hard limits of 512 MiB virtual address space, 10 CPU seconds and no core dumps.
The parent kills/reaps it after 15 wall seconds. Sources are capped at 64 MiB,
output at 2048 × 2048 RGBA pixels (also tightened to the texture ceiling), and
IPC input/output are bounded. Invalid dimensions, oversized output, process
failure and timeout leave external-open available. Other platforms retain PDF
inspection and external opening until comparable process isolation exists.
`cargo run -p archivefs-core --example manual_pdf_render_helper` exposes the
same isolated entrypoint for synthetic smoke tests. It expects the private
bounded JSON request on stdin and returns a bounded binary RGBA reply on stdout.

Reading state uses the existing atomic `gui-v2.json` preferences writer. Records
now include `ManualDocumentId` (path, size, modification time, device/inode where
available), last one-based page and fit/percentage zoom. Same-file positions are
clamped to the current count. Changed files and legacy records without a
fingerprint start at page one/Fit Page. The fingerprint detects ordinary file
changes/replacement, not adversarial same-metadata edits; it is not a content
hash. Sources are checked against discovery identity on open and against their
open identity on each read. Desktop opening revalidates the absolute source path
and fingerprint immediately before using the existing opener.

V2 items: portable PDF isolation; text/search/selection; links and annotations;
PDF tile rendering or a bounded cache to improve large-page zoom; gamepad event
routing through existing device-neutral viewer commands; CBR with a separately
vetted reader. V1 reruns bounded PDF parsing per page and keeps one texture;
zooming scales that texture without re-decoding. No speed claim is made.
