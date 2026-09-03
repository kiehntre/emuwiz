# EmuWiz GUI Redesign — Coding Agent Brief

Read these before touching production code:

1. `docs/GUI_DESIGN_SYSTEM.md`
2. `docs/GUI_REDESIGN_IMPLEMENTATION_PLAN.md`
3. `/home/davedap/archivefs/EMUWIZ_FULL_DUPLICATION_AUDIT.md`
4. the current source for the screen being changed

The approved Stitch mockups are visual references, not functional specifications.

## Core instruction

Implement the approved EmuWiz visual direction in Rust/egui **without inventing product behavior**.

The repository is authoritative for:

- supported actions;
- emulator support;
- DAT semantics;
- cheat/mod behavior;
- repair capabilities;
- cancellation safety;
- persistence;
- launch behavior.

The design documents are authoritative for:

- hierarchy;
- visual language;
- readability;
- component consistency;
- progress presentation;
- beginner vs technical information;
- artwork/media prominence;
- physical-build expectations.

## Do not

- rewrite the whole GUI;
- add fake buttons to satisfy a mockup;
- add fake ratings/metadata;
- add BIOS downloads merely because Stitch suggested them;
- change backend semantics without a separately reviewed task;
- hide safety confirmations;
- remove diagnostics;
- use raw Debug output as user-facing copy;
- turn every status into a pill;
- use tiny text to fit more information;
- block the render thread with expensive work.

## Required workflow per substantial screen slice

1. audit existing code and already-landed visual work;
2. identify what can be reused;
3. state intended files;
4. implement only the screen/component slice;
5. add focused tests;
6. run cargo check/fmt/diff-check;
7. do not claim visual success from tests alone;
8. wait for the user’s physical-build checkpoint before treating the redesign as visually accepted.
