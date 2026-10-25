# GUI-v2 Gamer View Recovery

> **Recovered historical research — status against current main (`b66422c2`).**
> Source: branch `research/gui-v2-gamer-view-recovery` at `549f3df9`. Recovered unchanged below this block except where marked `[refreshed]`.
> - **Historical research/design; current GUI v2 is authoritative.** Written against main `7f421daf` and an earlier GUI-v2 candidate. The old Gamer View shell is not being resurrected; a game-first **Browse & Play** route now exists in GUI v2 (`Route::BrowsePlay`).
> - Kept only for its rationale: a low-noise, game-first front door, recovery routes from a blocked game back to the relevant fix, and preserving the user's game context across navigation.


- Status: research/design only
- Audited: authoritative main `7f421daf64639e2c80ecd869a20086eb25a73f40` and the prepared GUI-v2 candidate `64397113508b7cc1929d25127e70071cf22acfa7`
- Production code changed by this research: none

## Executive conclusion

Gamer View existed as a genuine legacy GUI mode. It was a low-noise, game-first front door backed by the same library, mount operations, artwork workers, evidence state, and launch planner used elsewhere. It was not merely a different label for the management Library page.

The prepared GUI-v2 candidate retains most of the underlying capabilities, but presents them through a management-oriented route/sidebar shell:

- `Route::Section(Section::Games)` → search, filters, grid/list, platform selection, and game rows
- `Route::Game(id)` → Game Details
- `Route::Section(Section::Launch)` / task routes → launch/readiness surfaces
- family homes → management workflows such as DATs, repair, sources, rename, and diagnostics

What is missing is the mode boundary and composition: a single visually focused browse/play surface with platform browsing, artwork-forward cards, selected-game actions, and minimal technical noise. The right recovery is therefore a GUI-v2-native **Browse & Play** top-level mode, not a wholesale copy of the legacy renderer and not another management feature family.

## Evidence from the legacy GUI

### Mode and entry points

The legacy mode is defined in `crates/archivefs-gui/src/view_mode.rs`:

- `GuiMode::Simple`
- `GuiMode::GamerView`
- `GuiMode::AdvancedView`

The mode is persisted as `simple`, `gamer`, or `advanced` in `gui_mode.txt`. `crates/archivefs-gui/src/app.rs` owns `ui_mode` and `gamer_view_screen`; `crates/archivefs-gui/src/app_shell.rs` exposes `Return to Gamer View`, the Gamer menu, setup, advanced view, add-folder, and scan actions.

The legacy page dispatch is in `crates/archivefs-gui/src/app_pages.rs`. When `ui_mode == GuiMode::GamerView`, the normal management destinations are deliberately prevented from rendering except for the selected-game Cheats & Mods handoff. The same file builds the shared launch-readiness input, selected evidence, artwork, cover requests, metadata, and launch state, then calls `show_gamer_view`.

### Gamer View renderer

The primary implementation is `crates/archivefs-gui/src/gamer_view.rs`, especially:

- `show_gamer_view` — the main screen and Details transition;
- `GamerViewScreen::{GameList, Details}` — session-only list/details state;
- `GamerLibrarySnapshot` — one authoritative filtered/count snapshot;
- `gamer_search_text`, `gamer_row_matches_platform`, and empty-state guidance;
- `gamer_readiness` and `gamer_archive_readiness` — mount/preparation/launch reconciliation;
- `GamerViewAction` — typed actions returned to `app_pages.rs` rather than performing backend work in the renderer;
- `show_gamer_details_panel` — read-only selected-game details;
- `show_gamer_launch_blocker` — friendly blocker copy with technical details available.

The extracted submodules make the composition explicit:

- `gamer_view/layout.rs` — deterministic stage, platform strip, and browsing-rail geometry;
- `gamer_view/stage.rs` — dominant selected-game cover, title, readiness, Play, and secondary actions;
- `gamer_view/rail.rs` — virtualised game-card browsing rail, search, A–Z jump, and bounded cover scheduling;
- `gamer_view/alpha_jump.rs` — alphabetical navigation over the visible result set.

### Legacy feature behavior

The legacy view provides:

- platform shelf/chips with counts and an All/Unknown path;
- case-insensitive, trimmed search;
- virtualised game cards/list rows and A–Z navigation;
- cover, screenshot, platform-art fallback, and metadata enrichment;
- selected-game stage with title, platform, media identity, readiness, and a prominent action;
- mount, unmount, archive preparation, and multi-member selection;
- Play only when the shared launch planner produces a typed safe request;
- typed launch blockers with setup/review next actions;
- Game Details/read-only identity and artwork information;
- Cheats & Mods handoff, copy-location, undo where a reversible transaction exists;
- Add games, scan for new games, first-scan review, and cached-information refresh;
- handoff to Advanced View for identity review, emulator setup, and diagnostics.

`crates/archivefs-gui/src/gamer_artwork.rs` owns the Gamer View cover worker/cache, while `artwork_media_state.rs` keeps the session-owned artwork and alpha-jump state. `library_rows.rs` supplies shared row labels and identity/mount presentation vocabulary.

## GUI-v2 audit

### Authoritative main

Current main has the GUI-v2 stack before the prepared reconciliation commits. Its route model is `crates/archivefs-gui/src/gui_v2/routes.rs`:

- `Route::Home`
- `Route::Section(Section)`
- `Route::Game(i64)`
- `Route::Task { section, game }`

`Section::Games` is the browse route; `Section::Platforms` is a platform picker; `Section::Launch` is the play-oriented route; `Route::Game(id)` is Game Details. `pages.rs` renders those through `games`, `platforms`, `game_detail`, and launch/task functions. The GUI-v2 shell already has a persistent sidebar, central scroll surface, activity bar, and management-oriented page header/toolbar.

The main GUI-v2 equivalents are:

| Legacy concern | Main GUI-v2 equivalent | Assessment |
|---|---|---|
| Browse all games | `Route::Section(Section::Games)` → `App::games` | Equivalent capability; management presentation |
| Search | `LibraryFilter.search` in `App::games` | Equivalent, less game-first |
| Platform browsing | `Section::Platforms` → `App::platforms`; platform filter in Games | Partial: exists, but not one cohesive browse/play surface |
| Game cards/artwork | Games grid plus `artwork_metadata`, `museum`, and platform artwork | Partial/buried across routes |
| Selected game | `Route::Game(id)` and `App::game_detail` | Equivalent route; no dedicated browse/play stage |
| Play | `Section::Launch`, `Route::Task { section: Launch, game }`, `App::launch` | Equivalent backend/readiness path; not prominent in Games |
| Game Details entry | game-row navigation to `Route::Game(id)` | Equivalent |
| Cheats / Mods | `Section::Mods`, task route, family shell in candidate | Equivalent management destination |
| Saves | `Section::Saves` | Equivalent management destination |
| Manuals/documents | Game Details/artwork/document surfaces | Partial and not a Browse & Play shortcut |
| Problems | `Section::Problems` | Equivalent global inbox |
| Recent games | activity/history/recent-scan concepts | Partial; no Gamer View recent shelf |
| Favourites | no authoritative favourites model found | Lost/not implemented |
| Controller navigation | keyboard hints and egui focus only | Lost as a dedicated controller-first model |
| Low-noise mode | none in GUI-v2 | Lost |

### Prepared candidate

The candidate adds:

- `TopBottomPanel::top("v2_app_chrome")` in `gui_v2/pages.rs`;
- Back/Home, family-derived breadcrumbs, selected-game context, and one Jump menu;
- the full feature-family model in `gui_v2/routes.rs`;
- family landing pages and Easy/Normal/Advanced action labels;
- canonical `Route::QuickRename` under DATs & Verification;
- preserved sidebar direct access and widget-ID isolation.

It does not add a Gamer View equivalent. `gui_v2/pages.rs` still renders `Section::Games` as a filter-heavy Games page, with a grid/list toggle, health/attention filters, platform filter, and technical filtering details. `Route::Game(id)` remains a valid details destination. The candidate therefore improves navigation coherence but does not recover the old presentation mode.

## Legacy feature classification

| Feature | Classification | Proof / current state |
|---|---|---|
| Separate Gamer View mode | LOST from GUI-v2 | Legacy `GuiMode::GamerView` and `show_gamer_view`; no corresponding GUI-v2 mode or route in either audited tree |
| One-screen browse/play composition | LOST | Legacy stage + shelf + rail are composed by `show_gamer_view`; GUI-v2 splits Games, Platforms, Game Details, Launch, Artwork, and Organisation |
| Search | EQUIVALENT | GUI-v2 Games search edits `self.filter.search`; legacy normalises text in `gamer_search_text` |
| Platform counts and chips | PARTIAL | GUI-v2 has platform filter and `Platforms`; legacy has a dedicated counted shelf integrated into the browse screen |
| Artwork-first cards | PARTIAL | GUI-v2 has grid artwork and separate Artwork/Museum pages; legacy has cover worker, fallback art, selected hero, and bounded scheduling in one screen |
| Selected-game summary | PARTIAL | `Route::Game(id)` and `game_detail` exist; the legacy stage makes readiness and Play the dominant selected-game surface |
| Prominent Play action | BURIED | GUI-v2 launch routes exist, but Games rows do not provide the legacy stage’s single primary action/readiness projection |
| Shared launch-readiness safety | EQUIVALENT | Both legacy Gamer View and GUI-v2 use the existing launch planner/readiness machinery rather than inventing launch rules |
| Mount/archive preparation | EQUIVALENT backend, BURIED UI | Legacy typed `GamerViewAction` exposes preparation/member selection directly; GUI-v2 routes the user through task/organisation/detail workflows |
| Friendly launch blockers | PARTIAL | GUI-v2 has guidance/readiness copy; legacy has a dedicated `show_gamer_launch_blocker` with blocker-specific next actions |
| Game Details | EQUIVALENT | Legacy `GamerViewScreen::Details`; GUI-v2 `Route::Game(id)` / `App::game_detail` |
| Add games / scan | EQUIVALENT | Both reuse source actions; legacy Gamer View gives them first-class front-door actions |
| Cheats / Mods shortcut | EQUIVALENT | Legacy selected-game action and GUI-v2 Mods route/family action |
| Saves shortcut | PARTIAL | GUI-v2 route/family exists; legacy Gamer View’s selected-game secondary shortcut is not reproduced in the v2 Games composition |
| Manuals/documents shortcut | PARTIAL | Document/artwork capability exists, but not as a browse/play secondary action |
| Problems shortcut | EQUIVALENT destination, BURIED context | Global Problems route exists; legacy selected-game surface linked operationally to it through review/setup flows |
| Undo | EQUIVALENT backend, BURIED | History/undo exists; legacy Gamer View conditionally exposes undo for a selected reversible transaction |
| Recent/favourites | PARTIAL / LOST | Recent scan/activity/history exist; no authoritative favourites feature or Gamer View recent shelf was found |
| Keyboard navigation | PARTIAL | GUI-v2 documents Tab/Enter/Alt+Left; legacy library has keyboard shortcut guards and row navigation |
| Controller-first navigation | LOST | No controller/gamepad abstraction or D-pad/A/B model was found in the audited GUI paths |
| Technical-noise suppression | LOST | GUI-v2 pages intentionally expose health, filter, source, and diagnostic controls; that is correct for Manage but not Browse & Play |

“Intentionally retired” applies only to legacy management duplication, not to the Gamer View presentation. The old mode should not be copied as a second backend or a second history/library store; its user-facing browse/play composition is the part worth recovering.

## Recommended modern model: Browse & Play

### Mode boundary

Use two presentations over the same application state:

**Browse & Play**

- games first;
- artwork and platform browsing first;
- one selected game at a time;
- Play and Game Details are the dominant actions;
- only concise readiness/blocker wording by default;
- secondary shortcuts to canonical Cheats/Mods, Saves, Manuals, and Problems destinations;
- Add games and scan available without exposing source/database internals.

**Manage**

- DATs & Verification;
- repair/problems;
- rename and organisation;
- conversion;
- sources/providers;
- emulators/firmware;
- history/undo;
- advanced/diagnostics.

Both presentations must share:

- the same library snapshot and game identity;
- the same selected-game identity, preferably represented by the route rather than a second selected-game store;
- the same launch-readiness planner and executor;
- the same Game Details route/surface;
- the same app chrome, Back/Home behavior, breadcrumbs, and Jump menu;
- the same backend transaction/history stores.

### Top-level mode or feature family

Recommend **A: a primary top-level mode outside feature families**.

Feature families in the candidate are an information architecture for management workflows. Browse & Play is a presentation mode spanning Games, Platforms, Game Details, Launch, artwork, and selected-game shortcuts. Making it a thirteenth family would incorrectly imply that browsing is another management domain and would force family children to own routes they should only project.

The app chrome should expose a stable Browse & Play entry alongside the family Jump menu. Its breadcrumb can be:

`Browse & Play › [platform or All Games] › [game]`

The family menu remains the single management access model. A family shortcut from Browse & Play opens the canonical family home or canonical selected-game route; it does not embed a second Cheats, Saves, or Problems implementation.

### Suggested route/state shape

This is a design target, not an implementation in this commit:

- `Route::BrowsePlay` (or an equivalent top-level mode location);
- `Route::BrowsePlayGame(id)` only if the existing `Route::Game(id)` cannot carry the selected context cleanly;
- prefer reusing `Route::Game(id)` and a shared `selected_game` projection rather than introducing parallel details state;
- Browse filter state should contain only search, platform, sort, and optional recent/favourite presentation filters;
- management filters must not leak into Browse & Play;
- launch state remains the existing per-emulator launch state machines and shared planner.

The route transition into Game Details should preserve the selected game and allow Back to return to the same Browse & Play filter/platform context. A refresh or async artwork delivery must not clear that route context.

## Practical wireframes

### 1. Browse & Play home

```text
┌ Browse & Play ─────────────────────────────── Jump ▾ ┐
│ All Games     Search games…                 [Manage]  │
├ Platforms: [All 124] [NES 18] [SNES 27] [PS2 31] … ──┤
│                                                       │
│                         Choose a game                 │
│        Select a card to see artwork, readiness,       │
│                       and Play.                      │
│                                                       │
├ Games                                                 │
│ [cover] Mario          [cover] Zelda       [cover] …  │
│        NES · Ready             SNES · Needs setup     │
└───────────────────────────────────────────────────────┘
```

An empty library replaces the card area with **Add your games**, **Add folder**, and a concise explanation. No DAT IDs, source paths, or health checkboxes appear by default.

### 2. Platform selected

```text
┌ Browse & Play › SNES ─────────────────────── Jump ▾ ┐
│ [All Games] [SNES selected]       Search this platform │
├ SNES · 27 games ─────────────────────────────────────┤
│ [cover] Zelda      [cover] Metroid     [cover] …     │
│        Ready               Ready                     │
│                                                       │
│ [A] [B] [C] … alphabetical jump / scroll             │
└───────────────────────────────────────────────────────┘
```

The platform shelf is a filter projection over the same library, not a new platform catalogue.

### 3. Game selected

```text
┌ Browse & Play › SNES › Zelda ───────────────── Jump ▾ ┐
│                                                       │
│  ┌──────────────┐  Zelda                              │
│  │              │  SNES · cartridge                   │
│  │    COVER     │  Ready to play                      │
│  │              │                                      │
│  └──────────────┘  [ PLAY ]                           │
│                    [Game Details] [Cheats / Mods]    │
│                    [Saves] [Manuals] [Problems]       │
│                                                       │
├ More games below / Back returns to the same shelf ────┤
```

If blocked, replace Play with the planner’s concise blocker and one typed next action, such as **Open Emulator Setup** or **Review identity**. Technical detail stays behind an expandable disclosure.

### 4. Game Details transition

```text
Browse & Play › SNES › Zelda
                         [Game Details]

Game Details
  identity/evidence · artwork · files · readiness
  [Play] [Back to Browse & Play]
```

This is the canonical Game Details surface, not a second details implementation. Back restores the browse route and selection.

### 5. Empty library

```text
┌ Browse & Play ────────────────────────────────────────┐
│                                                       │
│                  Add your games                      │
│  Choose the folder where your games are kept.         │
│  EmuWiz will scan it without changing your files.     │
│                                                       │
│                    [ Add games ]                      │
│                                                       │
│  Already added a folder? [Scan for new games]         │
└───────────────────────────────────────────────────────┘
```

### 6. Launch failure

```text
Zelda
Needs setup
The selected game has no safe emulator launch plan yet.

[Open Emulator Setup]   [Review Game Details]

Technical details ▸
```

The failure must come from the existing typed launch blocker, never from string parsing or a duplicate readiness rule.

### 7. Future controller navigation state

```text
Browse & Play
  Platforms:  All  NES  SNES  PS2
  Games:      Zelda  Metroid  Mario
  Selected:   Zelda
  Action:     [ PLAY ]

Focus ring: Games > Zelda > Play
Hint: D-pad move · A select · B back · L/R platform shelf
```

The focus model is semantic (`Platform`, `GameCard`, `PrimaryAction`, `SecondaryAction`, `Search`) rather than widget-ID-specific. egui IDs remain stable and scoped, but controller focus should not depend on screen coordinates.

## Controller-first future path

Do not implement this in the recovery specification, but reserve the structure now:

- D-pad left/right moves across platform tabs or cards; up/down moves between shelf, selected stage, game grid, and action rows;
- A/select activates the focused platform, game, Play, or secondary action;
- B/back returns from Game Details to Browse & Play, clears a transient search overlay, or returns from a platform to All;
- shoulder buttons switch platform groups or page through platform shelf windows;
- a dedicated Search command opens an overlay with keyboard/controller text entry and keeps the previous browse context;
- a contextual action menu on a selected game exposes Play, Details, Cheats/Mods, Saves, Manuals, and Problems;
- launch blockers move focus directly to the typed next action;
- focus restoration is route-based: returning from Details restores the selected game card and prior platform/search state.

This makes Console Mode a presentation/input layer over Browse & Play, rather than a new library, route family, or launch implementation.

## Phase plan

### Phase 1 — mouse-first Browse & Play shell

Create the top-level Browse & Play entry and composition using existing GUI-v2 state:

- add a Browse & Play route/mode boundary in `crates/archivefs-gui/src/gui_v2/routes.rs`;
- add the page dispatch and app-chrome integration in `crates/archivefs-gui/src/gui_v2/pages.rs`;
- prefer a new `crates/archivefs-gui/src/gui_v2/browse_play.rs` for page-specific composition rather than expanding the management page indefinitely;
- reuse `App.library`, `App.filter`/a Browse-specific projection, `Route::Game(id)`, existing `game_detail`, `launch`, artwork projections, and `family_home`/family routes;
- add only the minimal Browse filter state to the GUI-v2 `App` state owner in `crates/archivefs-gui/src/gui_v2/mod.rs` or its existing state module;
- keep source actions, launch actions, history, and selected-game identity in existing owners;
- add focused egui tests in `gui_v2/tests.rs` for empty state, platform selection, selected-game Back behavior, blocker routing, and no duplicate state store.

### Phase 2 — artwork/card polish

- reuse the platform artwork and artwork metadata projections;
- add bounded cover scheduling and stable card IDs;
- add optional screenshots/metadata only when the selected card or stage requests them;
- add recent presentation if a product decision supplies an authoritative source; do not invent a second recent store;
- decide whether favourites are needed before adding persistence.

### Phase 3 — controller navigation

- introduce semantic focus targets and transitions;
- map D-pad/A/B/shoulders/search to those targets;
- test focus restoration across async refresh, Details, launch blockers, and family handoffs;
- retain full mouse/tab accessibility.

### Phase 4 — Console Mode

- add a controller-first chrome/presentation profile over Browse & Play;
- enlarge cards and focus rings, reduce management chrome, and preserve explicit Manage escape;
- keep all routes, library state, launch planning, Game Details, and family destinations shared.

## Guardrails

- Do not copy `gamer_view.rs` wholesale into GUI-v2. Its readiness and action protocols are valuable evidence, but its legacy shell ownership and state wiring are not the target architecture.
- Do not create a second library, selected-game store, artwork worker, launch planner, history store, or feature-family action implementation.
- Do not make Browse & Play a thirteenth feature family.
- Do not expose management filters in the primary browse surface unless they are explicitly converted into harmless presentation filters.
- Do not route packed MAME set/member mutation through generic Play or rename actions; preserve the existing MAME handoff semantics.
- Do not change backend semantics as part of the presentation recovery.

## Audit report

**LEGACY GAMER VIEW FOUND:** yes

**LEGACY FILES:**

- `crates/archivefs-gui/src/view_mode.rs`
- `crates/archivefs-gui/src/app.rs`
- `crates/archivefs-gui/src/app_shell.rs`
- `crates/archivefs-gui/src/app_pages.rs`
- `crates/archivefs-gui/src/gamer_view.rs`
- `crates/archivefs-gui/src/gamer_view/layout.rs`
- `crates/archivefs-gui/src/gamer_view/rail.rs`
- `crates/archivefs-gui/src/gamer_view/stage.rs`
- `crates/archivefs-gui/src/gamer_view/alpha_jump.rs`
- `crates/archivefs-gui/src/gamer_artwork.rs`
- `crates/archivefs-gui/src/artwork_media_state.rs`
- `crates/archivefs-gui/src/navigation/primary.rs`
- `crates/archivefs-gui/src/library_view.rs`
- `crates/archivefs-gui/src/museum_page.rs`
- `crates/archivefs-gui/src/selected_game_panel.rs`
- `crates/archivefs-gui/src/launch_readiness_page.rs`

**CURRENT GUI-V2 EQUIVALENTS:** Games/search/filter/grid/list, Platforms, artwork/Museum, `Route::Game(id)` Game Details, Launch/task routes, activity/history, and the prepared candidate’s app chrome/family navigation. These are capability equivalents, not a recovered Browse & Play mode.

**WHAT WAS LOST:** The separate mode boundary, one-screen browse/play composition, artwork-first selected stage, prominent Play/readiness action, concise blocker handoffs, first-class selected-game secondary shortcuts, and a future-ready controller focus model. Favourites were not found as an authoritative legacy feature and should not be claimed as lost behavior.

**RECOMMENDED MODERN MODEL:** Top-level GUI-v2 Browse & Play presentation sharing library, selected game, launch planner, Game Details, artwork, app chrome, and canonical family destinations with Manage.

**TOP-LEVEL MODE OR FAMILY:** Top-level mode outside the 12 feature families.

**BROWSE & PLAY WIREFRAME:** See the seven practical wireframes above.

**GAME DETAILS RELATIONSHIP:** Reuse the canonical `Route::Game(id)`/Game Details surface; Back restores Browse & Play context.

**CONTROLLER-FIRST PATH:** Semantic focus targets and route-based restoration in Phase 3, then a presentation/input profile in Phase 4.

**PHASE 1 FILES:** `gui_v2/routes.rs`, `gui_v2/pages.rs`, new `gui_v2/browse_play.rs`, GUI-v2 state owner (`gui_v2/mod.rs` or existing state module), and focused `gui_v2/tests.rs` additions.

**PRODUCTION CODE CHANGED:** MUST BE NONE; this commit contains documentation only.
