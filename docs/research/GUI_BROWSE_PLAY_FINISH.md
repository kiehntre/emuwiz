# Browse & Play: finish pass (one surface)

Baseline: `origin/main` @ `54d0503b`. Method: tour of every sidebar surface in a
fresh release build on the real 132,064-game catalogue (X11, 1400x1000),
then a close look at the surface that matters most.

## Audit (only what was observed)

| Surface | Observation |
|---|---|
| Home | Reasonable. "89763 need attention" is alarming without context. |
| **Browse & Play** | The main screen and the roughest. See below. |
| Setup & Doctor | Dense but ordered; jargon in counts ("multiple-installation cases"). |
| Problems & Repair | Sits on "Checking saved evidence" with a spinner and disabled buttons for a long time. |
| RomM Library | Blank page with "Loading the cached RomM snapshot..." |
| Converter | Good: the clearest page in the app. |
| Activity | Fine. |

### Browse & Play, before

1. The selected-game panel is a second column that scrolls away with the shelf,
   and it overlaps the third card column at 1400 px.
2. The "Problems & Repair" action inside it collapses to one letter per line.
3. About 300 px is spent above the first game: a Mr Wiz tip that repeats the page
   subtitle, a second "Browse & Play - Games first" heading, a search bar, and a
   platform strip with a large dead gap under it. At 1000 px tall barely one row of
   games is visible.
4. The platform strip is a horizontal scroller of 52 chips: most are off-screen and
   the scrollbar is far from them.
5. Cards are tall, with up to four-line titles, a separate "Select" button under
   each, and a repeated multi-line provider message in every cover.
6. Only the first 60 games can ever be reached ("Show 60 more").
7. "Games" and "Browse & Play" are both highlighted in the sidebar.
8. Copy: "launch it through the existing planner"; "Launch checks available".
9. Loading is indistinguishable from "no games yet".

## Completion contract

- The page is an application layout, not a scrolling document: title, one toolbar,
  a compact systems row, then shelf and selected-game panel side by side, both
  filling the window. Only the shelf scrolls.
- Play is visible without scrolling as soon as a game is selected.
- No overlap or clipping at 1000, 1400 or 1900 px wide; below ~900 px the panel
  becomes a compact strip above the shelf.
- Cards are one size, fully clickable, with a clear selected state, a two-line title
  and one "system - status" line. Long provider messages live in a tooltip and once
  above the shelf.
- Systems: largest few plus "All systems" and a dropdown for the rest; the active
  filter is always visible and one click clears it.
- The whole library is reachable (virtualised, no paging cap).
- Loading, empty, no-match and normal states are visibly different.
- No jargon, no repeated headings or tips. Selection and scroll position survive
  selecting a game and switching view mode.

## What changed

`browse_play.rs` was rewritten as a master/detail layout; small shell edits in
`pages.rs` (no outer scroll for this page, no repeated tip, plain subtitle, one
sidebar highlight, a short-detail picture variant for crowded shelves).
Nothing outside Browse & Play changed behaviour.

- Toolbar: one search box (with Clear) and Grid/List.
- Systems: All systems + the seven largest as pills (sized from their text, so
  they wrap instead of widening the page), a dropdown for the other 45, and the
  active system always visible.
- Shelf: virtualised, so all 132,064 games are reachable; whole-card click;
  two-line titles that never break mid-word; one "status - system" line with the
  status first. Cards share the row width.
- Selected game: a panel beside the shelf, always in view, with Play first, then
  Game details and four stacked actions; Advanced details stays collapsed.
- Summary line: "445 games  matching "mario" x  in Arcade x" with one click to
  remove each part; the RomM cover note is said once here, not on every card.
- Loading, empty and no-match states are distinct.

## Evidence

Before/after screenshots are in `docs/research/evidence/browse-play/`
(`before_*`, `b_*`, then `a6_*`, `c*`, `final_*`). Captured at 1400x1000 unless
named `narrow` (1000x800) or `wide` (1900x1000).

## Still imperfect

- Cards for games with no cover show a system icon and "No picture yet" 130,000
  times; that is honest but visually flat until artwork coverage improves.
- Very long system names are cut with an ellipsis on the card (full name in the
  tooltip and the detail panel).
- "Needs attention" is attached to most of the library; the page shows it, but the
  underlying counts (89,763 games) deserve their own review.
- Problems & Repair, RomM Library and Setup & Doctor still show long spinner
  states and jargon; see the audit table above.
