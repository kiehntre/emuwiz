# Job 41: Blue GUI visual polish

Started from fetched `origin/main` at `0f941734144412d3e2103cdafee175512cf7079f`
in the single worktree `/home/davedap/emuwiz-job41-blue-visual`, branch
`fix/job41-blue-visual-polish`. All Cargo builds use that worktree's registered
`scripts/cargo-iso` target.

Visual references are the October 26 Sunshine `:0` captures in
`/home/davedap/emuwiz-qa-artifacts/blue-library-preview-20261026/`, especially
`blue-library-catalogue.png` and `blue-library-selected-game.png`.

## Changes

- Cards reserve two title rows, aligning metadata across short and long titles.
  Portrait artwork, landscape artwork and missing-picture placeholders retain
  the same card and artwork-slot dimensions; images preserve their aspect ratio.
- Bounded labels explicitly wrap before applying their row limit and ellipsis.
  Titles retain word boundaries and show their full text on hover.
- Game Details and readiness action groups use equal 64-point button heights,
  accommodating two-line labels without shifting neighboring buttons.
- Below the desktop breakpoint, the selected game uses a compact strip with
  title and actions beside each other. Short windows use the existing page
  scroller so both the actions and the game shelf remain reachable.
- Health & Recovery content is bounded to 1400 points. Stacked supporting panels
  are bounded to 760 points; narrow evidence rows stack their label and value.
  Supporting text and headings have more readable sizing and casing.

Game identity, verification decisions, action destinations and production data
are unchanged.

## Executed validation

The focused test gate covers the new `visual_polish` tests and the existing
`blue_home_library`, `slice2b_layout`, `title_fit`, `launch_readiness_summary`,
`needs_attention_page` and `blue_usability` suites. It includes the Blue/Classic
layout and keyboard matrix, card metadata alignment, equal action-button bounds,
compact action visibility, scrolling to another game, bounded health cards and
horizontal text clipping at both requested resolutions.

The library test executable is built with
`scripts/cargo-iso test -p archivefs-gui --lib --no-run`, then run with private
HOME, XDG and EmuWiz application roots. The ignored Linux-only native fixture
`gui_v2::tests::visual_polish::visual_polish_native_preview` paints the real Blue
UI in private Xvfb sessions. Its synthetic catalogue contains mixed title
lengths, portrait/landscape color-block artwork and missing artwork, plus one
synthetic saved attention finding. It polls imagery without loading a live
catalogue or polling the live backend. No game is launched.

Final focused result: **99 passed, 0 failed, 1 ignored** (the manual native
fixture). Twelve native screenshots were captured. Independent review and
remote CI are recorded in the PR. Formatting,
GUI-root boundary and file-scope checks also pass.

## Screenshots

| Screen | 1920 × 1080 | 1024 × 600 |
| --- | --- | --- |
| Library cards | [Desktop](job41/library-1920x1080.png) | [Compact](job41/library-1024x600.png) |
| Selected game and actions | [Desktop](job41/selected-1920x1080.png) | [Compact](job41/selected-1024x600.png) |
| Full-title tooltip | [Desktop](job41/title-tooltip-1920x1080.png) | [Compact](job41/title-tooltip-1024x600.png) |
| Game Details | [Desktop](job41/details-1920x1080.png) | [Compact](job41/details-1024x600.png) |
| Health & Recovery | [Desktop](job41/health-1920x1080.png) | [Compact](job41/health-1024x600.png) |

Compact scrolling examples: [game shelf](job41/selected-scrolled-1024x600.png)
and [recovery panels](job41/health-scrolled-1024x600.png).

Small windows use vertical scrolling for content below the visible actions.
The screenshot fixture validates presentation; existing tests retain action
routing and launch-readiness coverage.

Full local logs, baseline captures, the screenshot runner and image hashes are
preserved under `/home/davedap/emuwiz-qa-artifacts/job41-blue-visual/`.
