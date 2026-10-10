# GUI Slice 2: Responsive Foundation (frozen specification)

Status: specification frozen 10 October 2026 against baseline
`1203d8bcba77da51461c2b04260518fe56820234`. Blue Shell 0/1 and the sidebar and
Manage Library focus-reveal hardening were promoted afterwards (main
`64f22902a6a202dc1670233704194a5905d8501e`). This file preserves the
specification and the real-display findings; it is a record, not a work plan.

## Scope: GUI Slice 2 Responsive Foundation

1. Shared page scaffold and scrolling frame.
2. Shared UI scaling for Blue and Classic.
3. Four distinct typography roles: page title, section title, card title, body.
4. Profile-aware accent, focus, primary-action and information tokens.
5. Shared card, empty-state and Mr Wiz presentation.
6. Visible keyboard focus and general reveal-on-focus.
7. Destination-aware breadcrumbs and Back navigation.
8. Title elision for Library, Game Details and headers.
9. Compact status bar.
10. Scale-safe Library toolbar wrapping.
11. Shared overflow and gutter fixes.

Classic receives only the scaling, overflow, focus, reveal and header
corrections it needs. At 100%, Classic should remain visually equivalent apart
from those shared corrections.

## Acceptance criteria

- Test Blue and Classic at 80%, 100%, 130% and 160%.
- Test at 1024×640, 1280×720 and 1920×1080.
- Every audited page must have a usable, reachable body or scroll appropriately.
- Settings must allow recovery from 160% scaling on a 1024×640 display,
  including keyboard access to Smaller and Reset size.
- Library Play must remain fully visible at 1280×720, 100%.
- Library search and Grid/List controls must never overlap.
- Long titles and breadcrumbs must elide without obscuring Back, Jump to or the
  mode switch.
- Keyboard focus must be visible, and focused controls must be brought into view.
- Blue must not inherit Classic teal/green styling in shared components, except
  legitimate semantic status colours.
- Breadcrumbs must be absent on the seven Simple destinations and show
  appropriate parents on deeper pages.
- No horizontal control clipping.
- Switching Blue and Classic must preserve the existing `ui_scale` preference.
- Ctrl+Plus/Minus and Settings scaling must use one consistent saved value.
- No new backend commands, routes or feature behaviour.

**Deliberate test change:** the `simple_shell.rs` expectation that Classic zoom
is always 1.0 conflicts with this specification. Classic supports the selected
scale while retaining its appearance at 100%. Changed in Slice 2A.

Deferred from the original reconciliation: duplicate search fields, competing
grids, terminology, cosmetic text corrections, unrelated banners, and broader
visual redesign. Dialog behaviour and native keyboard interaction remain
evidence gaps.

## Real-display findings (five, confirmed on X11)

Observed on a real XFCE/X11 desktop (1920×1080, scale factor 1) with a
disposable profile and an empty library. Original evidence, which must not be
overwritten: `/tmp/emuwiz-slice2-display-l0ktqj5e/` (`report.json`,
`events.jsonl`, `screenshots/`, `snapshots/`).

1. **Keyboard zoom is independent of the saved scale.** Ctrl+Plus took Blue
   100% to 110% (120% after two presses) while saved `ui_scale` stayed 1.0 and
   the Settings label said 100%. The same happened in Classic with saved 1.6.
   Blue keyboard zoom also exceeded the saved maximum (160% to 170%). Ctrl+0
   rendered 100% without changing saved 1.6.
2. **Keyboard focus is visible** on the sampled Blue controls (blue outline or
   fill) and Classic controls (white outline). Visibility was not verified for
   every widget, and an indicator can vanish when its widget is clipped.
3. **General Settings controls do not reveal on focus.** Blue sidebar and the
   five Manage Library cards do. A Blue off-screen beginner-hints checkbox was
   toggled with no prior scroll, and Classic reverse-Tab focus on Return Home
   stayed off-screen.
4. **Settings is not recoverable at 1024×640 and 160%.** With hints on, fixed
   chrome left about 60 px for the body and the scale buttons did not fit. With
   hints off, Smaller was reachable by scrolling but Reset size lay beyond the
   right edge. Keyboard activation worked blind. Classic hid the scale controls
   entirely.
5. **Saved scale survives Blue/Classic switching, but rendering does not.**
   Saved 1.6 persisted through switching, yet Blue 160% rendered Classic at
   100% and back to Blue at 160%.

Limitations of that evidence: empty profile only (no populated Library, Game
Details or long titles); only 1280×820 and 1024×640 observed; no dialogs; keys
were sent with XTest on one desktop and keyboard layout.

## Adjustments the findings required

- Intercept egui's keyboard zoom: Ctrl+Plus, Ctrl+Equals, Ctrl+Minus and
  Ctrl+0 act on the saved value, with the same bounds as Settings.
- Extend visible focus and general reveal; keep the working sidebar and Manage
  reveal, and do not scroll on pointer presses or wheel input.
- Require visible, unclipped recovery controls with hints both on and off.
- Show Smaller, Larger and Reset size in Classic too.
- Keep preference retention on mode switches; remove Classic's rendered reset
  to 1.0.

## Delivery status

| Item | State |
| --- | --- |
| Blue Shell 0/1, sidebar reveal, Manage Library reveal | Promoted (`64f22902`) |
| Slice 2A: shared scale, zoom shortcuts, Settings recovery, general reveal | Candidate, awaiting review |
| Typography roles, cards, breadcrumbs, title elision, toolbar wrapping, wider layout | Later slices |
