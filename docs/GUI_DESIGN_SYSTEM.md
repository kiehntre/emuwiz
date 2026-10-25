# EmuWiz GUI Design System

**Status:** Canonical implementation specification
**Visual reference:** Stitch “Archival Retrograde” direction, refined for the real EmuWiz Rust/egui application
**Target:** Linux desktop, Sunshine/Moonlight, 1100×720 minimum, 1080p/1440p/4K preferred

---

## 1. Purpose

EmuWiz is a game-library frontend, emulator setup assistant, verification tool, repair environment, and advanced collection manager.

The GUI must make those capabilities feel like **one product**.

A backend feature is not considered complete from the user's point of view unless the user can:

1. find it;
2. understand what it does;
3. start it;
4. see that it is working;
5. understand the result;
6. recover when something goes wrong.

The GUI must never require a beginner to understand EmuWiz's internal architecture merely to complete a normal task.

---

## 2. EmuWiz visual identity

### North-star description

EmuWiz should feel like a **premium retro game library and collector's archive**, with the practicality of a modern desktop application and the readability of a living-room frontend.

It should feel:

- game-first;
- warm and inviting;
- tactile rather than glossy;
- technical when needed, but not dominated by engineering evidence;
- distinctive without becoming theatrical;
- comfortable for long sessions;
- equally credible with a mouse/keyboard or through Sunshine/Moonlight.

### It must not feel like

- a generic SaaS/AI dashboard;
- a cheat trainer or hacking utility;
- an esports/RGB control panel;
- a database administration application;
- a mobile UI stretched across a desktop;
- an imitation of Steam, PlayStation, Xbox, LaunchBox, or any single existing frontend.

### Character

Use subtle cues from physical retro media, dark computer chassis, collector shelving, archival labels, console interfaces, and classic computer hardware.

Avoid fake scanlines, gratuitous CRT effects, excessive glow, faux-terminal theatrics, and decorative telemetry that does not help the user.

---

## 3. Color system

The Stitch palette is the starting point, but implementation should use **semantic roles**, not hard-coded color assumptions scattered across pages.

### Core roles

| Role | Suggested starting value | Purpose |
|---|---:|---|
| App background | `#0D1513` | Main canvas |
| Deep background | `#08100E` | Recessed/technical areas |
| Card/panel | `#161D1B` | Primary content surfaces |
| Raised card | `#1E2825` | Hover/focus/secondary elevation |
| Border subtle | `#23322D` | Normal structure |
| Border focus | `#3B524A` | Keyboard/gamepad focus |
| Primary text | `#F1F5F9` | Titles and important values |
| Secondary text | `#94A3B8` | Explanations and metadata |
| Dim technical text | `#64748B` | Hashes, paths, evidence |
| Teal | `#03C6B2` | Healthy/active/navigation identity |
| Warm amber | `#F59E0B` | Primary action, selected emphasis, attention |
| Success | `#10B981` | Proven success/verified |
| Error | `#EF4444` | Real failure/destructive state |

### Guardrails

- Teal/slate should dominate the chrome.
- Amber is an accent, **not a wash**.
- Amber is appropriate for one primary action, selected emphasis, and attention states.
- Yellow/amber must not make EmuWiz resemble trainer/cheat software.
- Red is reserved for real failures, destructive operations, or data-risk conditions.
- Do not use color alone to communicate meaning.
- Platform-specific accenting may be used sparingly in artwork/selection, never as a replacement for the semantic status palette.

---

## 4. Typography

The Stitch export names Space Grotesk, Inter, and JetBrains Mono. These are **references, not mandatory dependencies**.

### Implementation rule

Prefer fonts already safely available to the application. Do not add font files or new packaging complexity solely to match a mockup.

Use three semantic text families:

1. **Display / title**
   Strong, clean sans serif for page titles, selected-game titles, and major section headings.

2. **Interface / body**
   Highly readable sans serif for descriptions, labels, buttons, and normal application text.

3. **Technical**
   Monospace only for data that benefits from fixed-width presentation:
   - hashes;
   - file paths;
   - identifiers;
   - CRC/serial/Game ID;
   - raw technical evidence.

### Hierarchy

- Selected-game title: largest text on Gamer View.
- Page title: clearly visible but must not overpower selected content.
- Section heading: visually distinct from card titles.
- Card title: compact and readable.
- Body text: never tiny merely to fit more content.
- Technical text: subordinate in size/contrast, but still readable.

### TV / Sunshine rule

If text cannot be comfortably read at normal streaming distance, it is too small.

Do not treat 10–12 px web mockup text as an egui implementation requirement.

---

## 5. Spacing and geometry

Use a consistent spacing scale rather than page-specific improvisation.

Suggested semantic steps:

- XS: 4 px
- SM: 8 px
- MD: 12 px
- LG: 16 px
- XL: 24 px
- 2XL: 32 px

### General rules

- Minimum outer margin at 1100×720: approximately 16–24 px.
- Cards should generally use 16–24 px internal padding.
- Distinct sections should normally have 20–32 px separation.
- Primary buttons should have a minimum practical height around 40–44 px.
- Interactive targets must remain comfortable for mouse and streamed/TV use.
- Avoid deeply nested borders.
- Prefer spacing and typography over drawing a rectangle around every concept.

### Shape language

Use restrained corner radii.

- Small controls: ~4 px
- Cards/media: ~6–8 px
- Large panels/dialogs: ~8–12 px
- Pills only where semantically appropriate, primarily short status chips.

Do not turn every label into a pill.

---

## 6. Global application shell

The shell should provide orientation, not compete with the current task.

### Should contain

- EmuWiz identity/brand;
- current high-level location;
- global search where useful;
- compact global health/status entry;
- navigation appropriate to current mode.

### Should not contain by default

- CPU/GPU telemetry;
- bus speeds;
- clock-like decorative metrics;
- provider IDs;
- diagnostic counters that belong to another page.

Telemetry may live in an optional technical/system area.

---

## 7. Gamer View

Gamer View is the visual heart of EmuWiz.

### Primary goals

The user should understand within seconds:

- which game is selected;
- which system it belongs to;
- what it looks like;
- which emulator will launch it;
- whether it is ready;
- how to Play;
- how to reach Cheats & Mods;
- whether verification or repair needs attention.

### Required structure

#### Platform context
Use platform hardware artwork and a restrained system selector.

The user should be able to change system/platform naturally.

#### Selected-game hero
The selected game is the strongest visual object.

Include where available:

- box art/poster;
- screenshots/media;
- game title;
- platform;
- concise metadata;
- selected launch emulator;
- verification status;
- health/readiness status.

#### Actions
One obvious primary action:

**Play**

Secondary actions may include:

- Cheats & Mods;
- Verify;
- game details;
- save-state related actions only where actually supported.

Do not invent actions because a mockup contained them.

#### Library browse
Use covers/posters/screenshots where available.

Fallbacks should be deliberate and platform-aware, not blank grey rectangles.

### Technical evidence
Hashes, file paths, raw identity evidence, provider details, and similar information belong under Details/Technical details.

---

## 8. Home

Home remains the task-first orientation surface already intended by EmuWiz.

Do not invent a second task dashboard elsewhere.

Home should answer:

- What can I do now?
- Is anything important broken?
- What recently changed?
- What deserves my attention?

### Rules

- Clear hierarchy.
- Strong visual identity consistent with Gamer View.
- Avoid a grid of equally weighted status cards.
- Recovery problems must have direct human-readable actions.
- “Recovery required” without an explanation/action is unacceptable.
- Existing direct actions such as Add Games, Check Emulators, or Check the Problem should be visually prominent when relevant.

---

## 9. Verify Games / DAT / Collection Coverage

This area should be organized around **verification and collection coverage**, not around managing hundreds of catalogue records.

### Primary surface

For each selected platform/catalogue relationship, show the important outcome first:

- Expected by catalogue;
- Owned;
- Verified;
- Missing;
- Stale;
- Ambiguous;
- Duplicates;
- completion percentage;
- Full Set / Incomplete / Cannot determine.

### Degraded states

Never display fake zeros.

Examples:

- Catalogue not assigned → explain that coverage cannot be calculated and offer assignment.
- Expected inventory stale → explain and offer Validate.
- Expected inventory missing → explain and offer Validate.
- Duplicate canonical names → show useful counts but clearly say Full Set cannot be proven.

### Missing drill-down

- Bounded/paged list.
- Clear “View missing games” action.
- Do not render enormous inventories by default.

### Catalogue management

The 431-source management surface must **not sit in front of the main verification task**.

Source management belongs lower in hierarchy, filtered, searchable, or under an advanced/manage-catalogues section.

---

## 10. Emulator Setup / Doctor

The primary view must answer:

- Which emulators are ready?
- Which need attention?
- What can EmuWiz fix?
- What does the user need to do?

### Summary first

Example conceptual hierarchy:

- PCSX2 — Ready
- Dolphin — Ready
- DuckStation — Needs attention
- RPCS3 — Not checked

Primary action:

**Check Emulators**

### Recovery actions

If a problem has a safe existing fix path, present the action directly.

Prefer:

> DuckStation profile not found
> **[Check again] [Open setup]**

over raw diagnostic dumps.

### Technical details

Raw discovered paths, configuration parsing, evidence, logs, and profile internals remain available under Technical details / Full diagnostics.

---

## 11. Cheats & Mods

This screen must feel connected to the selected game, not like a separate engineering workspace.

### Top-level context

Show:

- selected game;
- target emulator/profile;
- current Play target;
- activation readiness;
- installed/relevant state where provable.

### Important distinctions

EmuWiz must distinguish:

- installed;
- active/enabled in the emulator;
- activation not confirmed;
- target mismatch.

Example:

> Cheats target RetroArch
> This game is currently set to Play with DuckStation.
> These cheats will not affect that launch.

### Workflow

A normal user should be able to:

1. choose source/import;
2. choose cheats/mod package;
3. preview;
4. confirm;
5. see progress;
6. understand result;
7. undo/remove where supported.

Do not expose provider IDs, hashes, trust internals, or raw formats as the primary interface.

### Mods

Generic mod results must preserve the honest status semantics already implemented:

- Mod installed
- Mod only partly applied
- Mod was not applied
- Mod removed

Persistent installed-mod visibility remains a separate product gap; do not fake it in the redesign.

---

## 12. Problems & Repair

Problems & Repair should be organized around **what is wrong and what the user can do**, not around internal diagnostic categories.

### Each problem should show

- plain-language problem;
- impact;
- affected item/system;
- safe next action;
- optional technical details.

### Fix Now

When a safe, already-implemented repair action exists, surface it directly.

Do not invent a “Fix Now” button for problems the backend cannot actually repair.

### Language

Prefer:

> This folder is already in your library.

over:

> Failed: config error...

Prefer:

> EmuWiz needs to restore an interrupted library change.

over:

> Recovery required.

The technical error may still appear under Details.

---

## 13. Progress and long-running work

This is a first-class product requirement.

A user must never wonder whether a click worked.

### Short operations
For operations that normally finish quickly:

- inline spinner;
- disable duplicate trigger;
- replace with success/failure state.

### Long operations
When total work is known, show:

- progress bar;
- completed / total count;
- current item or phase;
- elapsed information when useful;
- cancel only when the operation is genuinely safe to cancel.

When total is unknown:

- spinner/activity indicator;
- current phase;
- current item where available.

### Cancellation

Do **not** promise Cancel everywhere.

Only expose Cancel where the backend has safe cancellation semantics.

### Completion

Always show a useful completion result.

Example:

> Checked 68,853 files
> 68,201 verified · 442 need attention · 210 unavailable

Exact wording depends on the real operation.

---

## 14. Artwork and media

Artwork is a first-class part of EmuWiz, particularly in Gamer View and library browsing.

Use:

- platform hardware artwork;
- game covers/posters;
- screenshots;
- media artwork.

### Rules

- Preserve source aspect ratio where practical.
- Avoid aggressive cropping.
- Use consistent framed containers.
- Fall back gracefully when artwork is missing.
- Fallbacks should identify the system/game clearly.
- Avoid decorative imagery in technical/repair flows where it competes with action.

The existing EmuWiz platform hardware artwork should be used rather than ignored.

---

## 15. Responsive behavior

### 1100×720
Hard minimum for the current desktop GUI.

Requirements:

- no horizontal scrolling for primary pages;
- no clipped primary actions;
- cards may stack or collapse secondary regions;
- selected-game information remains usable;
- technical details may become collapsible.

### 1080p / 1440p
Use increased width for:

- media;
- richer browse rows;
- side-by-side selected-game detail;
- more generous spacing.

Do not merely stretch cards to fill space.

### 4K / large TV
Increase scale/spacing appropriately.

Do not respond to 4K by fitting twice as much tiny information onto screen.

### Sunshine / Moonlight
Prioritize:

- readable text;
- obvious focus;
- large targets;
- low ambiguity;
- stable layouts during streaming.

---

## 16. Status language

Use consistent human-facing language.

### Good top-level states

- Ready
- Verified
- Needs attention
- Working…
- Not checked
- Not available
- Not confirmed
- Could not complete
- Partly completed

### Avoid as standalone beginner messages

- Failed
- Recovery required
- Unavailable evidence
- Candidate conflict
- Projection incomplete
- Transaction blocked

Those terms may appear in technical details when they are the correct internal state.

---

## 17. Error presentation

Errors must answer three questions:

1. What happened?
2. What does it affect?
3. What can I do next?

Avoid persistent global error bars for harmless/recoverable actions after the relevant context has passed.

Example:

Instead of:

> Failed — config error: /mnt/usbdrive/games is already configured

show:

> **Folder already added**
> `/mnt/usbdrive/games` is already part of your library. Nothing changed.

---

## 18. Technical details policy

EmuWiz must remain transparent.

Technical evidence is **subordinate, not removed**.

Use disclosures such as:

- Details
- Technical details
- Why EmuWiz thinks this
- View exact changes
- View log

Technical information may include:

- hashes;
- paths;
- DAT source/revision;
- provider provenance;
- game IDs/serials/CRCs;
- emulator profile paths;
- transaction history.

Never require a terminal simply to retrieve evidence that EmuWiz already knows.

---

## 19. Accessibility and input

- Pair status color with text/icon.
- Make focus obvious.
- Minimum practical click target approximately 40–44 px.
- Keyboard interaction must remain usable.
- Gamepad support may display input hints where the application actually supports those bindings.
- Do not put fake `[A] [X] [Y]` glyphs on every action unless real input bindings exist.
- Do not rely on hover-only information.

---

## 20. Performance UX

Visual performance and computational performance both matter.

Audit and optimize:

- 431+ DAT source rendering;
- huge diagnostic lists;
- library browsing over tens of thousands of archives;
- DAT audits;
- catalogue validation;
- cleanup/repair operations;
- Expand All / Collapse All;
- media loading.

Large lists should use appropriate lazy/virtualized rendering where egui architecture permits.

Do not block the render thread with work that belongs in a worker.

---

## 21. Implementation constraints

This design system must be implemented against the real Rust/egui architecture.

### Do

- create reusable visual primitives;
- centralize semantic colors/spacing;
- progressively replace old page-specific styling;
- preserve backend behavior while redesigning presentation;
- physically test meaningful clusters of GUI work.

### Do not

- rewrite the entire GUI in one branch;
- convert EmuWiz to a web frontend merely because Stitch produced web concepts;
- duplicate core business logic in GUI code;
- invent backend capabilities to satisfy a mockup;
- hide safety confirmations;
- remove diagnostic evidence;
- silently change emulator configuration.

---

## 22. Stitch concepts that are references, not requirements

The original Stitch documents contain useful visual ideas but also generated product assumptions.

The following must **not** be treated as EmuWiz requirements unless the real repo already supports them:

- 5-star ratings;
- invented publisher/release metadata;
- “Auto-Fix Missing BIOS” as a universal action;
- downloading “legal BIOS dumps”;
- guaranteed gamepad glyph bindings on every control;
- unconditional cancellation of every long operation;
- JIT/memory-hook telemetry as persistent GUI decoration;
- bus-rate/live-I/O telemetry;
- fake “pristine vault” health semantics;
- physical media ratios applied rigidly to every platform;
- mobile/handheld layouts not currently targeted;
- any provider, scraper, cheat, mod, emulator, or repair capability not present in the real application.

Mocks communicate **layout, hierarchy, mood, and interaction intent**. The repository remains authoritative for actual functionality.

---

## 23. Implementation order

Recommended overhaul sequence:

1. **Design tokens + reusable primitives**
   - colors
   - spacing
   - typography hierarchy
   - buttons
   - cards
   - status banners
   - progress components
   - technical disclosure

2. **Gamer View**
   - establish the visual soul of EmuWiz
   - artwork/media
   - selected-game hero
   - primary Play action
   - emulator/readiness/verification context

3. **Home**
   - reuse the same design language
   - task-first
   - recovery/actions

4. **Verify Games / Collection Coverage**
   - coverage first
   - catalogue management subordinate
   - missing drill-down
   - performance

5. **Emulator Setup / Doctor**
   - summary and direct action first
   - technical diagnostics second

6. **Cheats & Mods**
   - selected-game context
   - activation/target truthfulness
   - preview/apply/result/undo

7. **Library and remaining tools**
   - align older screens with the established system

8. **Problems & Repair / History / advanced surfaces**
   - consolidate language and technical-detail hierarchy

---

## 24. Physical build checkpoints

Do not build after every tiny change.

When the user calls **“time for a build”**:

1. stop starting new GUI-affecting work;
2. finish or cleanly park major in-flight GUI jobs;
3. integrate/commit to a coherent authoritative state;
4. build release GUI;
5. launch into the real Sunshine/XFCE session;
6. physically inspect;
7. record a complete batch of defects;
8. fix in sensible groups.

Physical inspection is part of GUI development, not a final ceremony.

---

## 25. Definition of done for a redesigned screen

A screen is not complete merely because it matches the mockup.

It is complete when:

- the main task is obvious;
- primary action is obvious;
- normal user language is understandable;
- long work has feedback;
- errors have next actions;
- backend state is accurately represented;
- advanced evidence remains available;
- layout works at 1100×720;
- layout scales sensibly upward;
- mouse/keyboard use is comfortable;
- Sunshine/Moonlight readability is acceptable;
- the real physical build has been inspected.

---

## 26. Canonical rule

**Design from the user's task outward. Preserve the backend's truth inward.**

The mockups define the desired experience.

The EmuWiz repository defines what the product can truthfully do.
