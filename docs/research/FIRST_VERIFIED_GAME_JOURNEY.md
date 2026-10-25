# First Verified Game Guided Journey

**Status:** implementation-ready product specification; research only.

**Starting point audited:** `f535f4f71f8d28d32b4a22d2ec93d05cc6ace007`.

**Constraint:** this document proposes orchestration and presentation only. It does not add a new identity, scan, readiness, launch, transaction, or emulator truth source.

## CURRENT CAPABILITIES REUSED

The current GUI already contains the primitives needed for this journey:

| Need | Existing source | Reuse rule |
|---|---|---|
| First-run explanation | [`gui_v2/onboarding.rs`](../../crates/archivefs-gui/src/gui_v2/onboarding.rs:450) | Replace/add a persistent journey projection; retain the existing “nothing is locked” principle. |
| Configured folders and availability | `EnvironmentSnapshot` and Setup checklist in [`onboarding.rs`](../../crates/archivefs-gui/src/gui_v2/onboarding.rs:516) | Read source count, available sources, and attention state; do not duplicate source validation. |
| Scan | Scan confirmation and `self.load(true)` in [`gui_v2/pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:440) | Link to the existing scan action and Activity progress. |
| Games/platform detection | library/platform pages and platform details in [`onboarding.rs`](../../crates/archivefs-gui/src/gui_v2/onboarding.rs:603) | Use current library rows and platform counts. |
| Identity evidence | selected evidence states and DAT identity sections in [`selected_game_panel.rs`](../../crates/archivefs-gui/src/selected_game_panel.rs:345) | Project verified/unverified/inspection-error states; do not equate DAT presence with verification. |
| Emulator discovery | lifecycle projection and Emulator Setup routes in [`onboarding.rs`](../../crates/archivefs-gui/src/gui_v2/onboarding.rs:624) | Use installed/profile/readiness results supplied by existing setup and launch code. |
| Firmware/BIOS | BIOS/Firmware route and launch readiness | Use per-game launch requirements; a global firmware catalogue is not sufficient to claim game readiness. |
| Problems | typed problem model and Problems & Repair page | Link to the existing problem, preview, and recovery workflows. |
| Launch blockers | launch readiness page and [`guidance.rs`](../../crates/archivefs-gui/src/gui_v2/guidance.rs:185) | Map existing blocker reasons to novice copy without changing blocker policy. |
| Mr Wiz | contextual guidance selected by route and evidence in [`guidance.rs`](../../crates/archivefs-gui/src/gui_v2/guidance.rs:131) | Add journey-specific explanations as a presentation layer. |
| Game Details | `Route::Game` and selected-game action routes in [`gui_v2/pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1690) | End the journey at the existing selected-game hub. |
| Activity | persistent activity bar in [`gui_v2/pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:300) | Show scan and inspection progress without blocking navigation. |

Existing onboarding already says that metadata, artwork, cheats, and mods are optional and that browsing does not change originals ([`onboarding.rs`](../../crates/archivefs-gui/src/gui_v2/onboarding.rs:450)). The guided journey must preserve that boundary.

## NOVICE JOURNEY

The journey is a persistent, dismissible checklist available from Home and Setup & Doctor. It is not a modal wizard. A user can enter any step, leave it, use the sidebar, and return later.

```text
Open EmuWiz
  → choose a games folder
  → scan the folder
  → review what was found
  → choose a game
  → verify identity when useful/required
  → check emulator readiness
  → check required BIOS/firmware
  → resolve only blockers that affect the chosen game
  → open Game Details
  → explicit Play action
  → successful first launch
```

The checklist should show one recommended next action, but never prevent other navigation. If a step is already complete, it is collapsed or marked complete and the next incomplete step becomes prominent.

### Visible checklist

1. **Choose your games folder**
2. **Scan your games**
3. **Review what EmuWiz found**
4. **Choose a game to try**
5. **Check whether it is identified**
6. **Check emulator readiness**
7. **Check required BIOS or firmware**
8. **Resolve blockers, if any**
9. **Play your first verified game**

“Verified” here means the selected game has sufficient native identity evidence for the existing launch policy. It does not mean every game in the library must be DAT-verified before any game can launch.

## STATE MODEL

The journey is a projection of current state. It should not become a parallel state machine with independent truth.

```rust
enum FirstGameJourneyState {
    NoLibrary,
    LibraryReady { source_count: usize },
    ScanInProgress { activity_id: ActivityId },
    ScanComplete { game_count: usize, platform_count: usize },
    CandidateSelection { candidates: Vec<GameCandidate> },
    CandidateNeedsIdentity { game_id: GameId, reason: IdentityReason },
    CandidateIdentityReady { game_id: GameId, confidence: IdentityConfidence },
    EmulatorNeeded { game_id: GameId, requirements: Vec<EmulatorNeed> },
    EmulatorNeedsSetup { game_id: GameId, details: ReadinessDetails },
    FirmwareNeeded { game_id: GameId, requirements: Vec<FirmwareNeed> },
    GameBlocked { game_id: GameId, blockers: Vec<LaunchBlocker> },
    ReadyToLaunch { game_id: GameId },
    LaunchInProgress { game_id: GameId, launch_id: LaunchId },
    FirstLaunchSucceeded { game_id: GameId },
}

struct FirstGameJourneyProjection {
    state: FirstGameJourneyState,
    checklist: Vec<ChecklistItem>,
    recommended_candidate: Option<GameCandidate>,
    optional_items: Vec<OptionalItem>,
    source_of_each_fact: Vec<FactSource>,
}
```

The names above are conceptual. The implementation should consume existing source, library, evidence, environment, launch-readiness, and Activity types. It must not serialize a duplicate copy of these facts.

### State precedence

When several facts disagree, the journey follows existing policy:

1. current source/library availability;
2. native selected-media evidence and verified identity;
3. existing launch-readiness result;
4. emulator/profile and firmware facts;
5. external metadata as context only.

A stale or unavailable result is shown as stale/unknown, never silently converted to ready.

### State transitions

```text
NoLibrary --choose folder--> LibraryReady
LibraryReady --scan--> ScanInProgress
ScanInProgress --completed--> ScanComplete
ScanComplete --games exist--> CandidateSelection
CandidateSelection --user selects--> candidate-specific state
candidate --identity/readiness checks--> ReadyToLaunch | GameBlocked
GameBlocked --user reviews linked surface--> candidate-specific state
ReadyToLaunch --user chooses Play--> LaunchInProgress
LaunchInProgress --confirmed successful return--> FirstLaunchSucceeded
```

Cancellation, interruption, application restart, and navigation away return to a derived state. They do not create a failed permanent wizard state.

## CHECKLIST MODEL

Each checklist item has:

```rust
struct ChecklistItem {
    id: ChecklistId,
    title: &'static str,
    status: ChecklistStatus,       // NotStarted, InProgress, Complete, Warning, Blocked
    primary_action: JourneyAction,
    explanation: String,
    advanced_details: Option<DetailsRef>,
    mr_wiz_tip: Option<GuidanceTip>,
    skip_effect: SkipEffect,
}
```

| Step | Completion source | Blocked/warning state | Primary action | Advanced detail | Mr Wiz guidance | If skipped |
|---|---|---|---|---|---|---|
| Choose games folder | At least one configured and available source | No source; configured source unavailable | **Add a games folder** / **Check folder** | configured roots and last scan | “Choose a folder that already contains your games. EmuWiz reads it before changing anything.” | No scan or candidate can be derived. |
| Scan your games | Existing scan completion/activity and library refresh | Scan running, interrupted, or failed | **Scan configured folders** / **View Activity** | scan job, timestamps, error details | “Scanning reads your configured folders; it does not rename or repair originals.” | Existing library may remain usable, but new games are not represented. |
| Review what was found | Non-empty library and platform/grouped results | No games, unknown/uncatalogued entries, disconnected source | **Open Games** | paths, media kind, platform evidence | “Review the result before choosing a game.” | User can continue only if a known candidate already exists. |
| Choose a game | User explicitly selects a game | No plausible candidate or all candidates unavailable | **Choose a game** | candidate scoring facts | “You choose the game; EmuWiz will not apply a filename-only guess.” | Journey waits; no silent selection. |
| Check identity | Existing selected evidence/identity and current game identity | Unknown, conflicting, unreadable, or weak identity | **Review identity** / **Open verification** | evidence sources, DAT facts, conflicts | “A title match is a hint, not proof of a release.” | Launch may remain possible when existing launch policy permits; status says less certain. |
| Check emulator | Existing emulator inventory/profile/launch readiness | No supported emulator, invalid/stale profile, wrong core | **Open Emulator Setup** / **Review launch choices** | executable, profile, core, version | “EmuWiz checks what is installed; it does not silently install or replace emulators.” | Game cannot launch through that target until resolved. |
| Check BIOS/firmware | Existing per-game launch requirement and firmware evidence | Required firmware missing/unknown | **Inspect BIOS / Firmware** | required system software and evidence | “Some systems need system software before they can start.” | Only affects games that require it; unrelated games remain eligible. |
| Resolve blockers | Launch-readiness result and Problems & Repair | One or more hard blockers | **Review blocker** | typed blocker and evidence | “Review the reason and preview any repair before confirming it.” | Game remains blocked or uncertain; no automatic repair occurs. |
| Play first verified game | Existing `ReadyToLaunch` result and explicit Play | launch process failure or user cancellation | **Play** | launch plan, emulator target, diagnostics | “Everything needed for this game is ready; Play is still explicit.” | No success state is recorded. |

The checklist must distinguish **blocked** from **warning**. A warning never becomes a fake completion and never automatically prevents a launch that existing policy allows.

## REQUIRED VS RECOMMENDED VS OPTIONAL

### Required for this journey’s selected game

- a source folder that is currently available;
- a completed or usable scan result;
- a user-selected game whose media can be inspected;
- whatever identity, emulator, media, and firmware requirements the existing launch planner declares necessary;
- explicit user confirmation to launch.

### Recommended, but not universally required

- trusted DAT identity where it improves release certainty;
- resolving a known non-launch blocker;
- choosing a preferred emulator/profile when more than one safe option exists;
- reviewing the selected game’s evidence before Play.

### Optional enrichment

- artwork and metadata providers;
- RomM, ScreenScraper, or other external sources;
- cheats and mods;
- manuals/guides;
- playing-library projection, 1G1R, export, and cosmetic setup;
- cloud/account services and achievement features.

No internet access, account, RomM connection, ScreenScraper, RetroAchievements, cloud save, perfect artwork, or complete DAT catalogue may be a universal journey prerequisite.

## FIRST GAME CANDIDATE RULES

The user retains final choice. EmuWiz may suggest candidates only when the suggestion is explainable.

### Eligibility filter

A candidate may be suggested when:

- the game is present in the current library/source view;
- the media is readable enough for the existing inspection path;
- the platform is known or strongly resolved;
- no current file/source problem makes it unavailable;
- at least one emulator route can be evaluated;
- identity is verified, or the existing launch policy explicitly permits launch with a warning;
- required firmware evidence is ready or not required;
- no hard launch blocker is present.

### Ranking

Rank only after filtering, in this order:

1. launch-ready with verified native identity;
2. launch-ready with strong non-DAT native identity;
3. launch-ready with an explicit warning that the user can review;
4. readable candidate needing one clear setup step.

Use stable deterministic tie-breakers such as platform/title/library ID. Never rank by filename similarity alone, provider artwork, or first filesystem enumeration order.

### Presentation

Show at most one **Suggested first game** card plus **Choose another game**. The card must state why it was suggested, for example: “Verified identity · emulator ready · firmware not required.” The first suggestion is not auto-launched and is not silently selected as the user’s game.

## BLOCKER LANGUAGE

Every blocker is rendered in two layers:

1. a short plain-language sentence;
2. an expandable evidence/diagnostic section preserving the existing technical reason.

| Internal situation | Novice copy | Required distinction |
|---|---|---|
| No emulator target | **An emulator is needed** | Play is impossible through EmuWiz until one is available. |
| Installed executable but no valid profile | **Emulator setup needs attention** | The program exists, but launch configuration is not proven. |
| Required BIOS absent | **This system needs firmware before it can start** | Hard only for systems/games that require it. |
| Identity weak | **EmuWiz cannot confirm this exact release yet** | May be a warning or a hard block according to existing launch policy. |
| Wrong revision/update | **This game may not match the selected emulator or evidence** | Do not imply that a title-only match is safe. |
| Media unreadable | **The game file could not be read** | Explain whether inspection failed or the source disappeared. |
| Source unavailable | **The games folder is unavailable** | Existing catalogue may remain visible, but current launch is not proven. |
| Scan interrupted | **The scan stopped before it finished** | Resume/retry; do not report a complete library. |
| Launch process failure | **The emulator did not start this game** | Link to launch diagnostics and preserve the exact attempt result. |
| User cancellation | **Launch cancelled** | Not a product failure; return to the selected game. |

Each blocker must answer: what is wrong, what EmuWiz can do, whether the user must act, and whether play is impossible or merely less certain.

## RESUME MODEL

Progress should be derived dynamically from current state wherever possible. Persist only user experience preferences and non-authoritative context:

```rust
struct FirstJourneyUiState {
    dismissed: bool,
    last_selected_game: Option<GameId>,
    expanded_steps: Set<ChecklistId>,
}
```

Do not persist `IdentityReady`, `EmulatorReady`, `FirmwareReady`, or `GameReady` as wizard flags. Recompute them from current library, evidence, environment, and launch-readiness state. A scan, source change, profile change, firmware removal, or media edit must immediately change the projection.

Resume behavior:

- app restart: reopen the checklist at the first incomplete step, retaining the last selected game only if it still exists;
- navigation away: no state loss;
- scan in progress: show Activity and keep the checklist at **Scan your games**;
- scan interrupted: show retry/resume action and retain previous catalogue visibility as stale;
- source removed: return to **Choose your games folder** with a clear explanation;
- candidate becomes stale: re-evaluate and ask the user to choose again;
- successful launch: record the success in session/activity state, not as a permanent identity fact.

## FAILURE PATHS

| Situation | Journey behavior | Recovery link |
|---|---|---|
| No games found | Explain that scan completed but found no supported/readable game entries; do not call this a broken library automatically. | Game Folders / scan details |
| Unknown platform | Keep the item visible as unresolved; explain that platform assignment or stronger evidence is needed. | Game Details / identity review |
| Emulator absent | Mark selected game blocked only for launch; do not require emulator setup before browsing. | Emulator Setup |
| Emulator installed, profile invalid | Show “Emulator setup needs attention”; preserve executable/version diagnostics in advanced details. | Emulator Setup / launch choices |
| Firmware missing | Identify the exact system software requirement; do not suggest downloading copyrighted firmware. | BIOS / Firmware |
| Identity uncertain | Offer evidence review and explain warning vs hard block. | Verify / Game Details |
| Game unreadable | Distinguish missing source, malformed media, and inspection failure; keep original bytes untouched. | Problems / source folder / Game Details |
| Launch fails | Keep the selected game and launch attempt context; show the existing diagnostic result and retry only explicitly. | Launch diagnostics / Game Details |
| User cancels | Return to Game Details; checklist remains ready to resume. | Game Details |
| Scan interrupted | Show incomplete state, current progress if available, and retry; never promote partial results to complete. | Activity / Game Folders |

The journey must not auto-repair, auto-download software/firmware, auto-select a weak identity, or silently change source files.

## MR WIZ ROLE

Mr Wiz is a contextual explainer, not a second workflow controller. The existing guidance system already varies tips for Home, Sources, Launch, Problems, DATs, firmware, emulator setup, and Games ([`guidance.rs`](../../crates/archivefs-gui/src/gui_v2/guidance.rs:131)). The journey should provide a small set of additional contexts:

- **Choose folder:** what EmuWiz reads and what it will not change;
- **Scan:** why scanning can take time and where progress appears;
- **Choose game:** why the user chooses and why filename-only matching is insufficient;
- **Identity:** verified evidence versus external metadata;
- **Readiness:** the difference between installed software and a proven launch path;
- **Blocked:** what must happen, what is optional, and whether doing nothing is safe;
- **Success:** celebrate the first launch, then offer optional exploration.

Guidance should be one short paragraph or sentence with an optional “Advanced details” disclosure. It must not rotate away a blocker explanation before the user has acted on it.

## GAME DETAILS HANDOFF

Game Details remains the journey’s destination and the permanent game-centric hub. The journey should navigate to the existing `Route::Game(game_id)` rather than create a new “first game” management page.

On handoff, Game Details should receive only contextual presentation state, conceptually:

```rust
struct JourneyHandoff {
    game_id: GameId,
    reason: HandoffReason, // ReviewIdentity, ReviewReadiness, ReadyToPlay, LaunchSucceeded
}
```

The selected game page already renders identity/evidence, media, verification, emulator status, Play, Verify, Artwork & Metadata, Mods & Cheats, Fix Problems, saves/backups, and manuals ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1690)). The journey should highlight the relevant existing action, not duplicate it.

After a successful launch, show a compact success state:

> **Your first game launched successfully.**
>
> You can return to your library, explore Game Details, or review optional artwork, manuals, cheats, and mods.

Actions: **Return to Library**, **Open Game Details**, **Review remaining problems**. Optional features must not be presented as unfinished required steps.

## CONTROLLER FUTURE

The model must be usable later by a controller without making every technical screen controller-first. The journey’s controller-ready subset should be:

```text
Home → Journey checklist → Games/platforms → Game Details → Play
```

Requirements for a future implementation:

- one focused primary action per step;
- deterministic order of focusable steps;
- confirm/back actions that mirror route transitions;
- no required text entry for the happy path;
- blockers reachable from a single focused action;
- return to the same checklist/game after cancel or failure;
- controller disconnect/reconnect leaves the journey resumable.

Folder paths, DAT/provider configuration, diagnostics, detailed repair review, and conversion setup may remain mouse/keyboard-oriented expert surfaces.

## IMPLEMENTATION SEAMS

Implementation should add a thin projection/coordinator seam in GUI-v2, not alter core truth:

1. **Journey projector:** accepts existing environment snapshot, library snapshot, selected evidence, launch-readiness result, and Activity state; returns `FirstGameJourneyProjection`.
2. **Action mapper:** maps checklist actions to existing routes/actions: Sources, scan, Games, Game Details, Check, Emulators, Firmware, Problems, Launch.
3. **Candidate selector:** consumes existing game rows and readiness/evidence results; produces explainable suggestions without performing identity resolution itself.
4. **Context carrier:** preserves selected `GameId`, return route, and filter/platform context through task navigation.
5. **Success observer:** consumes existing launch lifecycle/activity result; it must not infer success merely because a process was spawned.
6. **Guidance adapter:** maps projection states to existing Mr Wiz guidance categories and expandable advanced detail.

Explicit non-seams:

- no new scan engine;
- no new identity matcher or DAT authority;
- no new emulator/profile registry;
- no new firmware database;
- no new launch planner or blocker enum;
- no journey-owned transaction/history state.

## TEST PLAN

Tests should validate projection and navigation contracts with synthetic state, not launch copyrighted games or real emulators.

### Projection tests

- no source → `NoLibrary` and Add folder action;
- unavailable source → source blocker with stale/previous-result distinction;
- scan running/completed/interrupted;
- zero games, games found, unknown platform;
- optional DAT unavailable while a game remains launch-eligible;
- verified identity versus weak/title-only identity;
- no emulator, invalid profile, ready emulator;
- firmware required/missing/not required;
- hard blocker versus warning;
- deterministic candidate ranking and user override;
- success state only from an explicit launch result.

### Navigation tests

- every checklist action reaches an existing route;
- selected `GameId` survives Game Details → Verify/Problems/Launch and return;
- source/platform/search context is preserved where the current router supports it;
- leaving and re-entering recomputes state;
- no dead end for cancellation, failure, or scan interruption;
- success offers Library, Game Details, and remaining-problems actions;
- optional metadata/provider features never become required checklist blockers.

### Copy/UX tests

- each blocker has novice text and expandable technical evidence;
- guidance appears for every journey state without masking the primary action;
- “installed emulator” is not rendered as “ready to play” without launch readiness;
- no title-only candidate is labelled verified;
- no firmware download or account requirement is implied.

## MVP

The smallest useful implementation is:

1. a persistent Home/Setup checklist;
2. dynamic projection from existing source, library, environment, evidence, and launch-readiness state;
3. actions for folder selection, scan, Games, Game Details, Emulator Setup, BIOS/Firmware, Problems, and Play;
4. one explainable suggested candidate, with explicit user selection;
5. scan interruption and navigation-away handling;
6. a first-launch success message;
7. focused projection/navigation tests.

The MVP does not need new persistence beyond dismissal and last selected game, and it does not need any new backend capability.

## NON-GOALS

- no modal wizard that locks the user into a sequence;
- no replacement for Game Details;
- no new scan, identity, DAT, emulator, firmware, launch, transaction, or history implementation;
- no requirement for perfect metadata, artwork, RomM, ScreenScraper, accounts, internet, achievements, or cloud saves;
- no automatic repair, download, ROM/media mutation, or title-only launch approval;
- no controller implementation in this specification;
- no broad redesign of unrelated GUI-v2 workflows.

## IMPLEMENTATION ORDER

1. Define the projection input/output types against existing state types.
2. Implement pure checklist/state derivation with no UI mutations.
3. Add deterministic candidate eligibility/ranking and explicit-selection semantics.
4. Wire checklist actions to existing routes and preserve the selected game/return context.
5. Add novice blocker copy and Mr Wiz journey contexts with advanced disclosure.
6. Add scan interruption, stale-state, and launch-success presentation.
7. Add focused projection and navigation tests.
8. Perform a controller-readiness pass over the happy-path focus order only.
9. Validate that optional enrichment remains optional and that no production reconciliation areas are touched.

## SPECIFICATION DECISION

The first verified game journey should be a dynamic checklist projection anchored on the existing Game Details hub. It should orchestrate current capabilities, explain the next safe action, and remain interruptible. The authoritative state stays in the existing library, evidence, environment, launch-readiness, and activity systems.
