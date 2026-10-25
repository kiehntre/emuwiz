# Manual and Strategy Guide Viewer Foundation

> **Superseded in part:** PDF page counting and CBZ inspection described below now go through the canonical, bounded `archivefs_core::manual_document` inspector. See [`MANUAL_DOCUMENT_VIEWER_FOUNDATION.md`](MANUAL_DOCUMENT_VIEWER_FOUNDATION.md). The "bounded header/body scan" and the GUI-private CBZ inspector no longer exist.

## Relevant existing support

GUI v2 already has a selected-game detail route, persisted GUI-v2
preferences, safe background commands, and read-only archive inspection. The
core also has a validated local RomM-manual opener and a desktop opener seam.
There is no embedded PDF renderer or generic document association model. The
existing ZIP inspector is bounded and read-only, but its broad archive report
does not define comic-book page ordering or document association, so this
feature adds a small focused layer.

## Document model

`gui_v2::documents` defines typed `GameDocument`, `GameDocumentKind`,
`GameDocumentFormat`, `GameDocumentSource`, `GameDocumentAssociation`, and
`GameDocumentViewerCapability` values. Each record carries its canonical path,
title, platform/game association, provenance, file size, optional page count,
viewer capability, and an explicit confidence reason.

Multiple documents are retained for one game. Association precedence is:

1. explicit persisted user association;
2. same game directory;
3. exact normalized title;
4. platform plus exact title;
5. weak filename similarity, which is refused rather than auto-linked.

The current GUI projection does not fabricate exact identity evidence from a
filename. A future identity-aware adapter can insert the stronger exact-game
identity tier without changing this model.

## Discovery and safety

Discovery is read-only, deterministic, and bounded to the game's directory,
`manuals/`, `docs/`, and `guides/` children, plus at most 16 configured roots.
Each directory contributes at most 512 files and the result is capped at 256
documents. Paths are canonicalized before use; documents outside their
configured root are ignored. No recursive filesystem crawl is performed.

## Format support

| Format | Foundation behavior |
|---|---|
| PDF | Recognised; a bounded header/body scan may report a cheap page count; opened with the existing desktop external-viewer seam. No renderer was invented. |
| CBZ | ZIP metadata is inspected read-only; image pages are enumerated in natural order; malformed, absolute, and `..` entries are refused. Nothing is extracted permanently. |
| CBR | Recognised, but marked `ExternalViewerUnavailable` because no robust licensed/open Rust RAR reader was already available in the GUI dependency set. |

The external viewer action validates that the path is an absolute regular
file, then uses the existing platform opener. EmuWiz never rewrites or
deletes a source document.

## GUI and controller foundation

Game Details now has a collapsed `Manuals & Guides` panel for real local game
rows, with title, kind, format, page count, source, association confidence,
file size, Open, Associate, Remove association, and a plain-language empty
state. Settings provides an optional local documentation-root picker and
removal control. No online acquisition control is present.

The viewer input model is vendor-neutral and maps confirm, back, page
navigation, jumps, pan, zoom, and menu to generic viewer commands. The first
milestone delegates rendering to the desktop viewer, so those commands are a
foundation for a later embedded CBZ/PDF surface rather than pretending to
control an external application.

Reading state is persisted by canonical document path with optional last page
and zoom. A saved page is used only when it is positive and within a known
current page count; changed/shortened documents therefore do not restore an
unsafe page. Association removal deletes only the preference mapping and
never the source file.

## Tests and limitations

Focused tests cover PDF discovery, CBZ natural ordering, traversal and
absolute-path refusal, malformed archives, unsupported CBR, explicit
association precedence, weak ambiguity refusal, read-only inspection, resume
invalidation, and controller mapping.

This foundation does not embed a PDF/CBZ renderer, does not inspect RAR
contents, does not fetch guides, and does not automatically infer an exact
identity from filenames. External viewers remain responsible for actual page
display until a reviewed embedded viewer capability is added.
