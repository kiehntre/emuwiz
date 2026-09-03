# EmuWiz GUI Redesign Implementation Plan

This plan accompanies `docs/GUI_DESIGN_SYSTEM.md`.

## Objective

Transform the existing Rust/egui GUI from a collection of individually functional technical surfaces into one coherent, game-first product without throwing away the backend work already completed.

## Non-negotiable rules

- Preserve safety semantics and explicit confirmations.
- Do not duplicate backend logic in the GUI.
- Do not invent functionality from Stitch mockups.
- Do not rewrite the entire GUI at once.
- Use isolated worktrees for substantial slices.
- Read `/home/davedap/archivefs/EMUWIZ_FULL_DUPLICATION_AUDIT.md` before major work.
- Treat physical builds as mandatory milestone validation when the user calls for one.

## Phase 0 — GUI archaeology and component inventory

Before implementation:

1. locate the current Gamer View visual/media code;
2. locate current artwork/media assets and loading paths;
3. locate current shared widgets/components;
4. locate Home task-first work;
5. locate Fix Now / repair actions;
6. locate responsive / large-window work;
7. locate status/progress primitives;
8. identify old and new competing page paths;
9. identify dead/unreachable visual work;
10. map current render-thread expensive operations.

Output: a concise “reuse / replace / retire” map.

## Phase 1 — Design foundation

Build semantic design tokens and reusable egui primitives.

Required categories:

- palette roles;
- spacing;
- typography sizes/weights;
- status tones;
- primary / secondary / destructive buttons;
- standard content card;
- hero card;
- status/recovery banner;
- progress row;
- empty state;
- technical-details disclosure;
- media frame.

Do not redesign all pages in this phase.

## Phase 2 — Gamer View pilot

Use the approved Stitch Gamer View as the north star.

Deliver:

- platform context/artwork;
- selected-game hero;
- poster/cover/screenshot region;
- Play as obvious primary action;
- launch emulator target;
- Verify status;
- Cheats & Mods action;
- health/recovery status;
- natural game browsing;
- technical details subordinate.

This is the first physical design checkpoint.

## Phase 3 — Home

Preserve Home’s task-first purpose.

Deliver:

- stronger EmuWiz identity;
- clear primary tasks;
- direct recovery actions;
- no giant equal-weight card grid;
- actionable state rather than raw engineering state.

## Phase 4 — Verify / Collection Coverage

Make collection coverage the main verification outcome.

Deliver:

- platform/catalogue context;
- Expected / Owned / Verified / Missing;
- completion;
- Full Set / Incomplete / Cannot determine;
- bounded missing list;
- clear Validate action;
- catalogue management moved visually below/behind the user task;
- performant 400+ source handling.

## Phase 5 — Emulator Setup / Doctor

Deliver:

- emulator readiness summary;
- Check Emulators primary action;
- direct next actions;
- concise error wording;
- Full diagnostics subordinate;
- avoid dumping profile internals into the default view.

## Phase 6 — Cheats & Mods

Deliver:

- selected-game context retained visually;
- emulator target and Play target;
- activation readiness;
- target mismatch warning;
- source/import flow;
- preview;
- explicit confirmation;
- clear working state;
- honest result;
- discoverable undo where currently supported.

## Phase 7 — Older library/tool surfaces

Apply the established language to:

- Library;
- Quick Rename;
- Library Organisation;
- Duplicate Finder;
- Disc Conversion;
- RomM;
- Mounts;
- Sources;
- History.

Do not blindly preserve every old collapsible section.

## Phase 8 — Problems & Repair

Reframe problems around:

- what happened;
- impact;
- next action;
- Fix Now when genuinely supported;
- Technical details for evidence.

## Phase 9 — Performance and progress

Profile real workflows:

- startup scan;
- DAT audit;
- DAT validation;
- cleanup;
- large source lists;
- giant diagnostic lists;
- expand/collapse behavior;
- artwork/media loading.

Move inappropriate work off the render thread and add truthful progress UI.

## Phase 10 — Whole-product usability

Physical journey tests:

1. add games;
2. browse;
3. verify;
4. understand missing;
5. set up emulator;
6. Play;
7. Cheats;
8. Mods;
9. repair;
10. restart and confirm state remains understandable.

If a journey requires internal architecture knowledge, log it as a GUI defect.
