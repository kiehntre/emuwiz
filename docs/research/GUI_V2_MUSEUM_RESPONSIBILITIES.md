# GUI-v2 Museum: current responsibilities and overlap

Status: descriptive note only. No Museum behaviour was changed in the first
human-smoke repair batch, apart from removing a duplicated title and
introduction that came from the shared hub-header bug. Whether Museum is
redesigned, merged into Browse & Play, or removed is a separate decision.

## What Museum does today

Code: `Section::Museum` (`gui_v2/routes.rs`), rendered by `App::museum` in
`gui_v2/pages.rs`. It sits in the sidebar under TOOLS and in the Artwork &
Extras family.

- Reads the same in-memory catalogue as Games (`self.library`); it has no data
  of its own. Its counts ("N catalogued games", "N platforms") are the Games
  counts.
- Triggers one artwork-index discovery on entry (`refresh_artwork_index`), so
  covers can be shown. That is the only work it does that Games does not
  itself trigger on entry.
- Step 1: a row of platform chips ("All systems" plus one chip per platform).
  Choosing a chip sets `self.filter.platform`, the same filter state Games and
  Browse & Play use, so the choice carries over to them.
- Step 2 (a platform must be chosen): a virtualised grid of cover cards, each
  with title, a status line, **Details** (opens Game Details) and **Play**
  (opens the launch planner for that game).
- "Open this platform in Games" hands the same filter to the Games page.
- With an empty library it shows an empty state that routes to Sources.

## What it does not do

- No artwork or metadata management (that is Artwork, Manuals & Extras).
- No collection statistics, timeline, shelves, or any "museum" curation. There
  is no data model for one.
- No filtering beyond platform (no search box).
- No state, selection or preferences of its own.

## Overlap with Browse & Play

Browse & Play is the top-level presentation mode: search, platform chips,
Grid / Compact List, artwork states, a selected-game panel with Play and
Game Details, and contextual routes. Museum is a strict subset:

| Capability | Museum | Browse & Play |
| --- | --- | --- |
| Platform chips | yes | yes |
| Cover grid | yes (platform must be chosen first) | yes (Grid and Compact List) |
| Search | no | yes |
| Selected-game panel / readiness | no | yes |
| Details / Play | per card | selected-game panel |
| Uses the shared catalogue and platform filter | yes | yes |

The human smoke test observed Museum behaving as "another platform/game
browser". That matches the code: it is one.

## Route and navigation footprint

- Sidebar: TOOLS > Museum. Family: Artwork & Extras (`family_for_route`).
- Legacy hand-off destination: `legacy::destination` maps `Section::Museum` to
  the legacy `MainView::Museum`.
- Guidance: `GuidancePage::Museum` ("views the current catalogue by
  platform; it does not alter games").

## Options for the separate decision (not acted on here)

1. Merge: fold the platform-first cover browse into Browse & Play as a view
   option and remove the Museum route and sidebar entry.
2. Redesign: give Museum a distinct purpose that Browse & Play does not have
   (for example curated shelves or preservation-focused views), which needs a
   data model first.
3. Remove: drop the route; nothing else depends on its state.

Any of these should keep the shared platform filter behaviour, since Games and
Browse & Play rely on it.
