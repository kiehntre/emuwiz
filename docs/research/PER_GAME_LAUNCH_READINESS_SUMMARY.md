# Per-Game Launch Readiness Summary

**Status:** implementation-ready product specification; research only.

**Starting point audited:** `33ecab378c54cfdfa4dd0303e1008b114b920878`.

**Purpose:** define a compact Game Details projection answering “Can I play this right now?” without creating a second readiness engine.

**Safety boundary:** the summary is read-only presentation. The existing launch planner and adapter preflight remain authoritative at the moment of Play.

## CURRENT AUTHORITATIVE FACTS

The current code already supplies the facts needed for a summary:

| Fact | Existing authority | Relevant behavior |
|---|---|---|
| Identity loading/unknown/conflicting | `LaunchReadinessInput` in [`launch_readiness_page.rs`](../../crates/archivefs-gui/src/launch_readiness_page.rs:96) and selected evidence | Unknown and conflicting identity are explicit input states; do not re-resolve identity in the summary. |
| Launch plan and candidates | `LaunchPlan` carried by `LaunchReadinessInput::Plan` | The summary consumes the same candidates used by the Play action. |
| Emulator/profile readiness | candidate readiness, `LaunchBlocker`, discovery contexts | Existing discovery and profile checks determine whether a candidate is usable. |
| Firmware | candidate `FirmwareReadiness` and platform-specific firmware evidence | Firmware is already part of candidate readiness; the summary must not run another BIOS scan. |
| Media/content | `ReadyToPlayEvidence` and candidate content | Source/path/container/mount facts remain native launch evidence. |
| Warnings/blockers | `LaunchWarning`, `LaunchBlocker`, `GamerBlockerKind` | The summary maps these to plain copy while retaining exact detail. |
| Candidate choice | remembered/sole-eligible filtering in `gamer_play_action` | A remembered or sole eligible candidate is preferred; multiple safe requests become an explicit choice, not a silent switch ([`launch_readiness_page.rs`](../../crates/archivefs-gui/src/launch_readiness_page.rs:432)). |
| Selected Game Details | `show_game_details` and selected evidence/readiness state | The selected game owns context and triggers current evidence/profile loading ([`selected_game_readiness.rs`](../../crates/archivefs-gui/src/selected_game_readiness.rs:253)). |
| Existing launch action | `gamer_play_action` and typed launch requests | The summary’s Play action delegates to the same typed request and final preflight ([`launch_readiness_page.rs`](../../crates/archivefs-gui/src/launch_readiness_page.rs:390)). |
| Problems/repair | Problems & Repair routes and typed problem presentation | Actionable issues link to existing problem/review surfaces; no new repair flow. |
| Guidance | route/evidence-aware Mr Wiz tips | Launch guidance already distinguishes identity verified, identity blocked, and setup checks ([`guidance.rs`](../../crates/archivefs-gui/src/gui_v2/guidance.rs:185)). |

The selected-game renderer currently loads evidence and relevant profile/firmware checks, then invokes the full launch-readiness panel after several evidence surfaces ([`selected_game_readiness.rs`](../../crates/archivefs-gui/src/selected_game_readiness.rs:264), [`selected_game_readiness.rs`](../../crates/archivefs-gui/src/selected_game_readiness.rs:395)). The proposed card should be a compact projection near the title/Play action; it should not replace the full panel.

## PRESENTATION MODEL

The visible card has one dominant state, one sentence, one primary action, and compact facts:

```text
READY TO PLAY
DuckStation
Identity: Verified · Firmware: Not required · Game file: Available
No warnings
[Play]
```

or:

```text
NEEDS ATTENTION
This system needs firmware before it can start.
Emulator: PCSX2 · Game identity: Verified
[Check BIOS / Firmware]
```

Advanced details may expand to show exact candidate, profile, executable/core, identity evidence, firmware evidence, source path/state, warnings, blockers, and freshness inputs. The first layer must not expose internal executable IDs, `library_name`, filesystem paths, or raw enum names.

### Rust-like presentation sketch

```rust
struct GameReadinessSummary {
    status: ReadinessPresentationState,
    headline: String,
    explanation: String,
    emulator: Option<EmulatorSummary>,
    firmware: FirmwareSummary,
    identity: IdentitySummary,
    source: SourceSummary,
    warnings: Vec<ReadinessWarning>,
    primary_action: ReadinessAction,
    secondary_actions: Vec<ReadinessAction>,
    advanced_details: Vec<DetailsRef>,
    freshness: ReadinessFreshness,
}

enum ReadinessPresentationState {
    Checking,
    Ready,
    ReadyWithWarnings,
    NeedsEmulator,
    NeedsFirmware,
    NeedsIdentityReview,
    SourceUnavailable,
    MediaUnreadable,
    Blocked,
    Stale,
}

enum ReadinessAction {
    Play,
    SetUpEmulator,
    CheckFirmware,
    ReviewIdentity,
    CheckGamesFolder,
    ReviewProblem,
    ChooseEmulator,
    ReviewDetails,
    RetryChecks,
}
```

This is a presentation type, not a replacement for `LaunchPlan`, `LaunchCandidate`, `LaunchBlocker`, `FirmwareReadiness`, or identity evidence.

## STATE PRECEDENCE

The summary must use a deterministic first-applicable precedence. A hard source/media failure should not be hidden by a ready emulator, and an optional provider failure should not downgrade a launch-ready game.

Recommended precedence, mapped to existing facts:

1. **Checking** when selected evidence, required profile discovery, or launch-plan inputs are not yet gathered.
2. **SourceUnavailable** when the selected game’s current source is unavailable or the persisted entry is absent from the latest live scan.
3. **MediaUnreadable** when existing inspection/content evidence says the selected media cannot be read or prepared for the launch path.
4. **Blocked** for an explicit non-source/non-media hard `LaunchBlocker`, including no safe emulator, invalid launch plan, unsupported required preparation, or unresolved launch-critical conflict.
5. **NeedsEmulator** when no usable candidate exists because an emulator is absent or setup/profile discovery is incomplete.
6. **NeedsFirmware** when the selected candidate requires firmware and existing `FirmwareReadiness` is missing, mismatched, or not verified.
7. **NeedsIdentityReview** when identity is unknown/conflicting and the existing launch policy refuses or requires review.
8. **ReadyWithWarnings** when at least one candidate is launchable and warnings remain.
9. **Ready** when a selected candidate is launchable with no warnings.
10. **Stale** when the summary’s inputs changed or became invalid before display/action; re-evaluate rather than presenting a cached ready state.

`Stale` is a freshness condition that can overlay any normal state. It is not a substitute for the underlying blocker. At Play time, existing adapter preflight must run again.

Where current launch policy treats identity as a hard requirement, identity must appear before candidate details in the resulting state. Where a candidate is explicitly launchable with a warning, the summary must say so rather than inventing a block.

## HARD BLOCKERS

Hard blockers prevent Play under existing policy. The summary should identify the first user-actionable blocker, while retaining all blockers in advanced details.

| Existing condition | Summary state | Novice explanation | Primary action |
|---|---|---|---|
| No current source / selected media unavailable | `SourceUnavailable` | “The game file is unavailable.” | **Check games folder** |
| Existing inspection/content failure | `MediaUnreadable` | “EmuWiz could not read this game file.” | **Review details** or existing problem route |
| No safe emulator candidate | `NeedsEmulator` or `Blocked` | “An emulator is needed for this game.” | **Set up emulator** |
| Emulator executable found but profile/setup incomplete | `NeedsEmulator` | “Emulator setup needs attention.” | **Set up emulator** |
| Required firmware missing/mismatched/unverified | `NeedsFirmware` | “This system needs firmware before it can start.” | **Check BIOS / Firmware** |
| Identity unknown/conflicting where launch policy requires it | `NeedsIdentityReview` | “EmuWiz cannot confirm this exact release yet.” | **Review identity** |
| Explicit invalid launch plan or unsupported required preparation | `Blocked` | “This game is not ready to launch with the current setup.” | **Review blocker** |
| Multiple safe candidates with no remembered/sole choice | `Blocked` or selection substate | “Choose how to play this game.” | **Choose emulator** |

The summary must not claim “firmware missing” merely because a global firmware page is incomplete. It must use the selected candidate’s relevant firmware state.

## WARNINGS

Warnings do not prevent Play under current policy. They should be visible as a count or short phrase, with details expandable.

Potential warnings:

- title-only or weak identity when the existing launch plan permits it;
- stale external/provider metadata;
- optional DAT not present when native identity is sufficient;
- alternate emulator available but not selected;
- provider/artwork/manual lookup unavailable;
- a candidate warning returned by the existing launch plan;
- previous launch failure, as context only, if current readiness is still valid.

The following are not merely warnings when current authoritative facts say otherwise:

- missing required firmware;
- source disconnected;
- unreadable media;
- invalid profile or no safe candidate;
- identity mismatch/conflict that the existing launch policy blocks.

Artwork, metadata, RomM, ScreenScraper, cheats, mods, manuals, and optional provider failures must never downgrade launch readiness.

## NOVICE COPY

| State | Headline | Explanation | Primary action |
|---|---|---|---|
| `Checking` | **Checking whether this game is ready** | “EmuWiz is checking the game, emulator, and any required system software.” | **Wait** or **Retry checks** if an error occurs |
| `Ready` | **Ready to play** | “The selected emulator and game checks are ready.” | **Play** |
| `ReadyWithWarnings` | **Ready, with one warning** | “You can play, but review this note if you want more certainty.” | **Play**; secondary **Review warning** |
| `NeedsEmulator` | **An emulator is needed** | “EmuWiz found the game, but no safe launch setup is ready.” | **Set up emulator** |
| `NeedsFirmware` | **This system needs firmware** | “The selected emulator needs system software before this game can start.” | **Check BIOS / Firmware** |
| `NeedsIdentityReview` | **This exact release is not confirmed** | “Review the available evidence before relying on this launch choice.” | **Review identity** |
| `SourceUnavailable` | **The game file is unavailable** | “The library remembers this game, but its current folder cannot be reached.” | **Check games folder** |
| `MediaUnreadable` | **The game file could not be read** | “EmuWiz could not complete the checks needed for this media.” | **Review details** |
| `Blocked` | **This game is not ready yet** | “Review the reason below before trying again.” | **Review blocker** |
| `Stale` | **Readiness needs to be checked again** | “The emulator, firmware, source, or game changed since this result.” | **Recheck readiness** |

Avoid “loadability verification failed”, raw `LaunchBlockerKind`, `library_name`, executable paths, or “candidate lane” in the novice layer. Keep those in `Advanced details`.

## PRIMARY ACTIONS

There is exactly one primary button per state:

- `Ready` → **Play**;
- `ReadyWithWarnings` → **Play**, with a secondary review link;
- `NeedsEmulator` → **Set up emulator** or **Choose emulator** when candidates exist but selection is required;
- `NeedsFirmware` → **Check BIOS / Firmware**;
- `NeedsIdentityReview` → **Review identity**;
- `SourceUnavailable` → **Check games folder**;
- `MediaUnreadable` → **Review details** or the existing relevant problem;
- `Blocked` → **Review blocker**;
- `Checking` → no competing action; show progress or **Retry checks** only on a genuine failed check;
- `Stale` → **Recheck readiness**.

Secondary actions such as Verify, Artwork, Mods, Saves, Manuals, Conversion, and Problems remain available below the card, but must not compete visually with the operational next step.

## EMULATOR DISPLAY

Show the selected/preferred candidate in a human label:

```text
Emulator: DuckStation
Emulator: PCSX2
Emulator: RetroArch · Beetle PSX HW
Emulator: Flycast
```

The summary must follow the existing remembered/sole-eligible candidate policy. If more than one safe request remains and no remembered choice exists, show **Choose how to play this game** and require explicit selection. Never silently switch away from a user-selected profile.

Novice view shows emulator/core display name and readiness. Advanced details may show executable path, profile ID, core `library_name`, version, installation authority, and discovery evidence.

## FIRMWARE DISPLAY

Firmware is shown only when relevant to the selected candidate/platform:

- **Not required** — the candidate has no firmware requirement;
- **Ready** — existing candidate firmware evidence is verified;
- **Missing** — required firmware is absent;
- **Unknown** — the required check has not completed or cannot prove the file;
- **Mismatch** — evidence exists but does not match the required firmware.

Novice copy should be “Firmware: Ready”, “Firmware: Not required”, or “This system needs firmware”. Advanced details retain the exact required identity, evidence source, and freshness. EmuWiz must not offer copyrighted firmware downloads.

## IDENTITY DISPLAY

Use a compact label with a tooltip/details expansion:

- **Verified** — native/authoritative identity meets existing policy;
- **Strong local identity** — strong native evidence, even if no DAT is present;
- **Launchable with warning** — current launch policy permits play, but identity certainty is limited;
- **Needs review** — identity is unknown or conflicting and action is required/encouraged;
- **Mismatch / blocked** — evidence conflicts with the selected launch requirement.

Do not equate DAT membership with universal launch permission. Do not call title-only or provider-only evidence “Verified”. Preserve region, revision, product code, hash, and provider distinctions in advanced detail.

## SOURCE/MEDIA DISPLAY

First layer:

- **Available** — selected source/media is present and current enough for the plan;
- **Unavailable** — persisted entry exists, but current source is disconnected or missing;
- **Unreadable** — inspection/content evidence failed;
- **Recheck needed** — source/media changed or the result is stale.

Do not show filesystem internals in the first layer. Advanced details may show relative path, archive/container kind, mount requirement, last observation, and evidence timestamp. A successful previous launch does not override a current unavailable or unreadable source state.

## MULTIPLE EMULATOR HANDLING

The card displays one selected/preferred candidate. Selection follows existing behavior:

1. remembered user choice;
2. sole eligible candidate;
3. explicit chooser when multiple safe candidates remain;
4. no candidate when all are blocked or unproven.

The summary must not display a noisy list of equal candidates. A **Change emulator** secondary action is available only when the existing launch-choice surface can preserve the selected GameId and return to Game Details.

Changing the selected profile invalidates the displayed summary and forces a fresh projection. A prior ready result must never survive a profile change as current truth.

## PROBLEMS & REPAIR HANDOFF

When the authoritative blocker has a relevant existing problem or repair destination, the card links directly to it with the selected `GameId` and a return target of Game Details.

Examples:

- missing/unavailable source → existing Game Folders/source route;
- identity conflict → existing verification/evidence review;
- media health problem → Problems & Repair or selected evidence details;
- emulator/profile issue → Emulator Setup or launch choices;
- firmware issue → BIOS / Firmware or existing doctor evidence.

The card must not create a special repair workflow. After returning, it recomputes from current state. If the user does nothing, the card remains blocked/warning and says so plainly.

## MR WIZ ROLE

Mr Wiz should explain why the summary has its state and what the user can safely do next. It should not duplicate the headline verbatim.

Examples:

- Ready: “The selected emulator and game checks agree. Play still remains an explicit choice.”
- Firmware blocker: “This game needs system software for the selected emulator. EmuWiz can show what is required, but does not provide copyrighted firmware.”
- Identity warning: “The file looks like this title, but the exact release is not confirmed. Review the evidence before relying on a revision-sensitive setup.”
- Source unavailable: “The saved library entry is still visible, but the current folder must be available before launch can be proven.”
- Prior failed launch: “The last attempt did not start successfully. This new readiness result is current only if the emulator, source, and profile checks still pass.”

Mr Wiz guidance should be contextual, stable while a blocker is visible, and expandable with technical evidence.

## POST-LAUNCH CONTEXT

The card may show recent launch context, but this is never readiness authority:

```rust
struct LaunchContextNote {
    last_attempt: Option<LaunchAttemptSummary>,
    current_readiness_freshness: ReadinessFreshness,
}
```

- successful launch → “Last launched successfully” as a secondary note;
- failed launch → “Last launch did not start” with a diagnostics link;
- cancelled launch → no failure classification;
- source/profile/firmware/media change → recompute and remove any stale success implication.

The launch result must come from the existing tracked launch lifecycle, not from process-spawn alone. A prior success cannot promote an otherwise blocked game to Ready.

## CONTROLLER FUTURE

The card should be a single focusable group near the top of Game Details:

1. focus card;
2. activate primary action;
3. open details;
4. choose/change emulator where allowed;
5. back to Game Details/library.

No required path entry or dense evidence table belongs in the happy-path card. Keyboard focus and later controller focus should use the same action order. A controller implementation is explicitly out of scope here.

## GAME DETAILS PLACEMENT

Place the compact summary immediately below the game title/platform/media identity and adjacent to the existing Play action, before artwork, screenshots, saves, manuals, cheats/mods, and specialist evidence.

Recommended order:

1. title, platform, media kind, cover;
2. **Launch readiness summary**;
3. Play / launch-choice action;
4. Verify / identity evidence;
5. Artwork & Metadata;
6. Saves & Backups;
7. Manuals & Guides;
8. Cheats & Mods;
9. conversion/disc/specialist panels;
10. advanced technical details.

The full existing launch-readiness panel can remain available from an “Explain this result” or expanded details action. The compact summary becomes the top-level operational status, not another buried panel.

## IMPLEMENTATION SEAMS

1. **Pure summary projector:** accept the already-built `LaunchReadinessInput`, selected evidence/source state, and current launch lifecycle context; return `GameReadinessSummary`.
2. **State mapper:** translate existing `LaunchReadinessInput`, candidate readiness, firmware, blockers, warnings, and media evidence into presentation states.
3. **Action mapper:** map each presentation action to existing routes/actions: Play, launch choices, Emulator Setup, BIOS/Firmware, identity review, Problems, source folders, or recheck.
4. **Freshness guard:** identify whether the inputs changed since projection; never cache Ready as independent truth.
5. **Context carrier:** preserve `GameId`, selected profile, and return route through blocker/problem/setup surfaces.
6. **Guidance adapter:** provide state-specific Mr Wiz text and technical disclosure.
7. **Launch-result adapter:** consume existing launch activity/result for contextual notes only.

Explicitly do not add:

- another launch planner;
- another identity or firmware checker;
- a new blocker enum in core;
- a summary-owned emulator registry;
- transaction/history state;
- a second Game Details page.

## TEST PLAN

Use synthetic authoritative inputs and projection tests. Do not launch real emulators or mutate media.

1. Fully ready candidate → `Ready`, selected emulator shown, Play primary.
2. Ready candidate with launch warning → `ReadyWithWarnings`, warning visible, Play remains allowed.
3. No emulator candidate → `NeedsEmulator`, setup action.
4. Executable present but profile invalid/incomplete → `NeedsEmulator`, not Ready.
5. Required firmware missing → `NeedsFirmware`, firmware action.
6. Source unavailable → `SourceUnavailable`, source action.
7. Media unreadable → `MediaUnreadable`, details/problem action.
8. Weak identity but launchable → warning or identity-review state according to existing plan, never falsely Verified.
9. Identity mismatch/conflict → `NeedsIdentityReview` or `Blocked` according to existing launch policy.
10. Multiple safe candidates → explicit chooser; no silent switch.
11. Stale source state → `Stale`/unavailable; no cached Ready.
12. Optional provider unavailable → readiness unchanged.
13. Prior failed launch with current-ready inputs → Ready plus contextual failed-attempt note.
14. Prior successful launch followed by profile/source/firmware change → recomputed non-ready state where appropriate.
15. Selected profile changes → summary invalidated and rebuilt.
16. Firmware removed after prior readiness → missing/unknown firmware state, never stale Ready.
17. Every primary action preserves GameId and return context.
18. Advanced detail exposes exact evidence without leaking technical IDs into headline copy.

## MVP

The minimum useful implementation is:

1. a pure `GameReadinessSummary` projection;
2. a compact card near Game Details Play;
3. states for Checking, Ready, ReadyWithWarnings, NeedsEmulator, NeedsFirmware, NeedsIdentityReview, SourceUnavailable, MediaUnreadable, Blocked, and Stale;
4. one primary action per state;
5. selected emulator, firmware, identity, source/media, and warning rows;
6. expandable technical details;
7. direct links to existing setup, identity, Problems, and launch-choice routes;
8. focused synthetic projection/navigation tests.

## NON-GOALS

- no new readiness truth or launch planner;
- no changes to emulator profile discovery, identity matching, firmware verification, routing/loadability, or transactions/history;
- no automatic emulator/firmware installation;
- no optional metadata/provider requirement;
- no ROM/media mutation;
- no controller implementation;
- no replacement of the full launch-readiness panel;
- no release-contract or packaging changes.

## IMPLEMENTATION ORDER

1. Map existing launch/evidence types into a pure presentation input contract.
2. Implement precedence and freshness tests before UI wiring.
3. Implement novice copy and one-primary-action mapping.
4. Place the compact card at the top of Game Details while retaining the full panel as advanced detail.
5. Wire direct problem/setup/identity/emulator/firmware handoffs with GameId return context.
6. Add launch-result context without allowing it to affect readiness truth.
7. Add synthetic projection, stale-state, multiple-candidate, and navigation tests.
8. Review the card for controller focus order and optional-feature separation.

## SPECIFICATION DECISION

Game Details should answer “Can I play this right now?” through a compact, dynamic readiness card backed by the existing launch plan and evidence. The card should make the selected emulator and next action obvious, expose hard blockers versus warnings, and remain honest whenever source, media, identity, firmware, or profile facts become stale.
