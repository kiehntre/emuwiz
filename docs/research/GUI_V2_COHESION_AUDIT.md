# GUI-v2 Cohesion / Novice-Flow Audit

**Scope:** read-only audit of the GUI-v2 product at `f535f4f71f8d28d32b4a22d2ec93d05cc6ace007`.

**Method:** inspected the rendered route map, primary navigation, onboarding/setup, selected-game page, guidance, problems/repair, organisation, RomM, artwork, saves, documents, cheats/mods, and activity/history surfaces. Findings below cite the current implementation rather than inferred backend capability. No production code was changed.

## CURRENT PRODUCT SHAPE

GUI-v2 is a safety-oriented review shell with a strong game catalogue and many native workflows. Its explicit product promises are visible in the route purposes in [`routes.rs`](../../crates/archivefs-gui/src/gui_v2/routes.rs): browse first, preview before changing anything, and expose history/recovery. The main rendered shell is a fixed sidebar plus scrollable page content in [`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:311).

The product currently has three overlapping mental models:

1. **Game-centric:** Home → Games → Selected game → Play / Verify / Artwork / Mods / Problems.
2. **Task-centric:** Setup & Doctor, Problems & Repair, Converter, Organisation, Cheats, Saves.
3. **Evidence/admin-centric:** DATs, RomM, Activity, History, Advanced diagnostics, specialist inspectors.

The first model is the best novice path, but the navigation makes the second model primary and the third model highly visible. Game Details is already close to the natural hub: it exposes Play, verification, artwork, mods, problem review, media/evidence panels, saves/backups, and local manuals. It does not yet expose every capability there (notably emulator choice, BIOS readiness, conversion, and history as first-class game actions).

## FIRST-RUN AUDIT

The intended novice path is represented in [`onboarding.rs`](../../crates/archivefs-gui/src/gui_v2/onboarding.rs:470): add a library/source, inspect the collection, review problems, configure an emulator, then optionally inspect metadata/providers. The actual first-run page is titled **Setup & Doctor**, with “System setup”, “Saves & States”, database upgrade, emulator checks, source availability, and problems links ([`onboarding.rs`](../../crates/archivefs-gui/src/gui_v2/onboarding.rs:71)).

| Flow | Entry / observed steps | Novice result | Main gap |
|---|---|---|---|
| First launch / guidance | Welcome or Setup & Doctor → source, emulator, DAT, problems cards | Helpful and read-only; next actions exist | “Setup & Doctor” and health counts are less direct than “Get started” / “Check my setup”; no single completion milestone |
| Add/import library | Setup → Game Folders → discovery/scan → Games | Scan confirmation explains that originals are not changed ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:438)) | Source setup, discovery, and scan are separate concepts; the novice must understand “configured folder” before seeing a game |
| DAT / identity setup | Library → DATs & Verification or Setup checklist → identification data | Guidance explains versioned/reviewable identity data | DAT is implementation vocabulary; the first-run path does not make “verify my games” the next obvious step |
| Emulator discovery/setup | Setup → Emulators → Setup & Readiness / Installed Emulators & Updates | Clear read-only boundary; installed is not equated with launchability | Per-game readiness is deferred to Game Details and Launch, so setup can feel complete before a game is actually playable |
| BIOS/firmware | Setup → BIOS / Firmware | Explicit that firmware is not supplied | A global list is separate from the game’s launch blocker; the user must connect the two |
| First launch | Games → game → Play → Launch checks | Strong safety message; no launch until checks | No first-run “launch this verified game” funnel joins scan, identity, emulator, firmware, and Play |

**Assessment:** the requested minimum path exists as disconnected links, not as one guided journey. The highest-value first-run improvement is an explicit progress checklist with a final “Launch a verified game” action, while retaining the existing read-only guarantees.

## GAME DETAILS AUDIT

[`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1678) already makes Game Details the strongest novice surface:

- **Play** leads to a separate launch-readiness route.
- **Verify**, **Artwork & Metadata**, **Mods & Cheats**, and **Fix Problems** are direct actions.
- The page shows platform, media kind, source path, verification status, emulator status, cover/screenshots, and expandable advanced details.
- Selected-media evidence is rendered before the main detail card, including specialist Dreamcast/Wii U/Saturn evidence.
- **Saves & Backups** and **Manuals & Guides** are local, clearly bounded panels ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1850)).

Missing or fragmented from this hub:

- emulator selection is a launch/setup concern rather than a visible game action;
- BIOS/firmware blockers appear through readiness, not as a compact “what this game needs” summary;
- conversion is not a game-specific action;
- history/undo is global rather than linked to the selected game;
- bezel/decorations are nested in the artwork surface rather than clearly named in the game action row;
- the page can become long and evidence-heavy as specialist panels accumulate.

**Conclusion:** Game Details should become the primary game-centric surface, but it should aggregate links and status rather than absorb every implementation panel. Keep advanced evidence in collapsible sections, as the current page already does.

## NAVIGATION MAP

The primary groups and labels are defined in [`primary.rs`](../../crates/archivefs-gui/src/navigation/primary.rs:109):

```text
Home
├─ Library: My Games, DATs & Verification, Ready to Play
├─ Setup: Game Folders, Emulators, BIOS / Firmware
├─ Organise & Export: Clean & Rename, Build Libraries, Duplicates
├─ Tools: Converter, Tape Inspector, Museum
├─ Enhance: Cheats, Mods & ROM Hacks, Saves
├─ Health & Recovery: Problems, Advanced Diagnostics
└─ Settings
```

Contextual/secondary routes add RomM, artwork, history, activity, selected evidence, MAME repair, archive inspection, and specialist media inspectors. `subviews()` adds alternate screens for emulator setup, sources, cheats, and health ([`primary.rs`](../../crates/archivefs-gui/src/navigation/primary.rs:177)). Less frequent mounts, media sets, journals, storage health, and repair history are intentionally retained as advanced entries ([`primary.rs`](../../crates/archivefs-gui/src/navigation/primary.rs:230)).

Observed navigation issues:

- **Duplicate concepts:** “Problems”, “Needs Attention”, “Doctor”, “Automatic Health Report”, “Configuration Diagnostics”, and “Repair & Recovery” all appear in the Health family. Their distinctions are real, but not obvious to a novice.
- **Duplicate entry paths:** Cheats are available from Enhance, Home cards, and selected-game actions; artwork is available from Enhance, Home, and selected-game actions; RomM is both a source subview and a native library surface.
- **Back/context risk:** global sections are easy to enter, but a selected-game action becomes a task route and the user must rely on the route’s back behavior to return to the game. This should be tested as a state-preservation contract, especially after refresh or background work.
- **Buried actions:** conversion, manuals, disc structure, and detailed evidence are discoverable mainly from a selected game or specialist surface, not from the main novice path.
- **Advanced leakage:** “DATs”, “1G1R”, “parent/clone”, and “MAME reconstruction” are visible before the user necessarily needs them.
- **No explicit controller model found:** the audited GUI-v2 code documents Tab/Enter/Alt+Left keyboard hints ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:326)), but no GUI-v2 controller navigation/hotplug abstraction was found in the inspected surface.

## NOVICE LANGUAGE ISSUES

The code already has a good pattern: simple status first, technical details in a collapsible section. Examples include `Needs attention` notices with `Technical details` ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:344)) and problem details with “What happened / Why it matters / What EmuWiz can do / Safety and undo” plus `Advanced details` ([`problems.rs`](../../crates/archivefs-gui/src/gui_v2/problems.rs:190)). Preserve this pattern.

| Technical term observed | Novice layer should say | Advanced detail should retain |
|---|---|---|
| DAT / identity evidence | “Trusted game identification data” / “Why this match is trusted” | DAT name/version, hashes, evidence precedence |
| loadability / launch readiness | “Can EmuWiz launch this game now?” | emulator/core path, profile and firmware facts |
| parent / clone | “This arcade set shares data with another set” | parent/clone relationship and preservation implications |
| revision mismatch | “This code or metadata may belong to a different release” | exact revision/product/hash evidence |
| transaction / journal | “Recorded change with undo information” | transaction ID, journal directory, state |
| provenance / provider precedence | “Where this information came from” | provider, snapshot, precedence/conflict details |
| opaque operation | “EmuWiz can preserve this instruction but cannot explain or safely apply it” | native opcode/expression and parser status |
| core `library_name` / provider IDs | Do not show by default | show only in diagnostics |

The current onboarding glossary explicitly explains “Parent / clone” and “MAME preservation vs playing library” ([`onboarding.rs`](../../crates/archivefs-gui/src/gui_v2/onboarding.rs:485)), which is useful but indicates that backend vocabulary is reaching the novice path. Move that material behind “Learn more” or advanced help after the first explanation.

## REPAIR UX

The native problem model carries the five answers a safe repair page needs: title, why, action, safety, undo, and technical evidence ([`problems.rs`](../../crates/archivefs-gui/src/gui_v2/problems.rs:20)). The rendered details page presents “What happened”, “Why it matters”, “What EmuWiz can do”, “Safety and undo”, and an advanced technical block ([`problems.rs`](../../crates/archivefs-gui/src/gui_v2/problems.rs:190)). This is a strong baseline.

| Repair question | Current status | Finding |
|---|---|---|
| What is wrong? | Yes | Problem title and category are explicit. |
| Why does it matter? | Yes | `why` is rendered separately. |
| What will EmuWiz change? | Partial | Action text is present, but many native problem entries route to review/verify rather than showing a concrete proposed change. |
| Is it reversible? | Yes for reviewed repair paths | Safety/undo text is shown; exact availability depends on the workflow. |
| What if I do nothing? | Partial | Consequences are often implied by `why`, not consistently stated as a separate “If you do nothing” sentence. |

MAME reconstruction is the clearest specialized implementation: it reports stale plans, unavailable journals, safe rollback, and review-required states in [`organisation.rs`](../../crates/archivefs-gui/src/gui_v2/organisation.rs:581). General Problems should adopt the same explicit preview/result vocabulary without exposing transaction internals by default.

## FEATURE DISCOVERABILITY

| Flow | Entry point(s) | Discoverability | Evidence |
|---|---|---|---|
| Artwork / metadata | Game Details action, Artwork & Metadata section, Home/provider cards | Good after a game is selected; weak as a first-run need | selected-game action row and artwork route in [`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1760) |
| Cheats | Enhance → Cheats, Home card, Game Details | Good, but global and game-specific contexts can blur | primary Enhance entries and task route |
| Mods | Enhance → Mods & ROM Hacks, Game Details | Good for selected game; novice may not distinguish mods from cheats | primary labels and `Section::Mods` |
| Saves / snapshots | Enhance → Saves and Game Details → Saves & Backups | Visible but restore/compare are explicitly unavailable in the foundation | [`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1857) |
| Manuals / guides | Game Details only | Discoverable only after opening a game | [`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1908) |
| Conversion | Tools → Converter; sometimes specialist/game context | Reasonably named, but not tied to selected-game intent | primary Tools group |
| MAME repair | Organise & Export → Build Libraries → Fix my MAME library | Too deep for affected users; good specialist copy once reached | [`organisation.rs`](../../crates/archivefs-gui/src/gui_v2/organisation.rs:830) |
| Multi-disc / disc evidence | Selected game specialist panels, Museum/inspectors | Advanced and format-dependent; this is appropriate, but status should be summarized on Game Details | [`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1690) |
| RomM | Sources subview and RomM Library route | Duplicate mental models: connection/setup vs read-only browsing | primary source subviews plus `Section::Romm` |
| History / undo | Health advanced entries and workflow result panels | Recovery is not prominent at the point a user worries about it | advanced entries in [`primary.rs`](../../crates/archivefs-gui/src/navigation/primary.rs:230) |

## DUPLICATED SURFACES

These are not necessarily bugs; they are places where labels and context need to converge:

- Home cards and primary sidebar both expose Cheats & Mods, artwork, setup, and problems.
- Sources contains Game Folders, Discovery, and RomM connection, while the library also has RomM browsing.
- Problems, Needs Attention, Doctor, and Repair History overlap from a user perspective even when their implementation roles differ.
- Selected Game, Selected Evidence, Museum, and Archive/Disc inspectors can all become “what is this file?” destinations.
- Verification appears in Library, Game Details, Problems, and launch readiness.

Recommended rule: global entries should explain the collection-level purpose; game-detail actions should explain the selected-game purpose. Avoid removing safe deep links until navigation tests prove context preservation.

## CONTROLLER READINESS

This is a code/affordance classification, not a claim that physical controller input was tested. GUI-v2 explicitly supports keyboard focus hints but the inspected code does not expose controller mapping or hotplug handling.

| Class | Surfaces | Reason |
|---|---|---|
| Controller-ready in principle | Home, platform/game browsing, simple selected-game action rows | Large section buttons and straightforward next actions; needs actual focus/controller testing. |
| Needs minor navigation work | Setup & Doctor, Problems, Game Details, Saves, artwork grids | Scrollable/collapsible content and action-row focus need predictable selection, back, and context retention. |
| Mouse/text-centric | Folder/source selection, DAT/provider setup, search/filter, emulator paths, RomM connection, manual/document association | File dialogs, text entry, dense metadata, and provider configuration dominate. |
| Fundamentally unsuitable without a separate interaction layer | Repair previews with path/file selection, advanced diagnostics, conversion configuration, history/journal inspection | These require precise text/path/evidence review and should remain expert/mouse workflows. |

For a future Console Mode, build a smaller controller-first projection of Home → Platforms → Games → Game Details → Launch. Do not attempt to make every diagnostic and configuration page controller-first.

## PERFORMANCE RISKS

Only obvious code-level risks are listed; no whole-application benchmark was run.

- The main page wraps route content in a vertical `ScrollArea`, while selected game details add another nested `ScrollArea` ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:373), [`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1698)). Large evidence/artwork pages may produce awkward nested scrolling and repeated layout work.
- Artwork and screenshots are rendered in the game detail path; the code intentionally limits initial screenshots and offers “Show all screenshots”, which is a good mitigation ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1810)). Keep lazy loading and avoid eager grids for large libraries.
- Library, DAT, MAME, and history pages should keep pagination/virtualization or bounded previews. The selected game currently gives a clear bounded “first 200 games” message in artwork-related browsing ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:2203)); extend this discipline consistently.
- Background activity is visible in a persistent bottom bar and jobs are followed in Activity ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:300)), which reduces the risk of users retrying long work, but completion should also refresh the originating page and preserve selection.

## TOP 10 UX GAPS

Priority combines user impact, frequency, confusion risk, implementation size, and release importance.

1. **P0 — No single first-run “first verified game” journey.** Setup links are present but disconnected; a novice can complete setup without being guided through scan → identity → emulator/firmware → launch. (High impact/frequency; medium size.)
2. **P0 — Launch readiness is not summarized on the game hub.** Game Details shows an emulator status and sends users to Launch, but does not compactly show the remaining blocker and next action for emulator/core/firmware/media. (High impact; medium size.)
3. **P1 — Health terminology is fragmented.** Problems, Needs Attention, Doctor, diagnostics, and repair/recovery are separate labels for adjacent concerns. (High confusion; medium size.)
4. **P1 — Recovery is not consistently visible at the action point.** MAME has strong history/undo language, but global history is advanced and other selected-game actions do not consistently link to affected history. (High trust impact; medium size.)
5. **P1 — RomM has two user mental models.** Connection/setup and read-only library browsing are both valid, but the relationship is not explained in one sentence at the entry point. (Medium frequency; small/medium size.)
6. **P1 — Technical terms appear before intent.** DAT, parent/clone, 1G1R, provenance, and provider details are available in novice-adjacent paths. (High confusion; small copy/layout size.)
7. **P1 — Game Details does not aggregate all game actions.** Conversion, BIOS need, bezel/decor, and history are not equally visible beside Play/Verify/Artwork/Mods/Problems. (High discoverability; medium size.)
8. **P1 — Controller-first operation is not evidenced.** Keyboard focus hints exist, but no controller navigation/hotplug contract is present in the audited GUI-v2 layer. (Important for Console Mode; potentially large.)
9. **P2 — Advanced evidence can dominate the selected-game page.** Specialist panels are valuable, but a long detail page risks burying the novice status and primary action. (Medium; small/medium layout work.)
10. **P2 — Search/filter and large-library performance boundaries are not consistently discoverable.** Existing bounded/lazy patterns should be made a visible product rule. (Medium; medium engineering.)

## QUICK WINS

- Add a first-run progress card with four plain-language checkpoints: “Folders scanned”, “Games identified”, “Emulator ready”, “First game ready to play”.
- Change novice-facing `DATs & Verification` to “Game verification” with “DAT” in the advanced explanation.
- On Game Details, show one compact readiness banner: “Ready to play”, “Needs firmware”, “Needs emulator setup”, or “Needs identity review”, with one action.
- Add “Back to game” context on task routes and return to the same selected game after completion.
- Put “History for this game” beside any reviewed change when a history record exists.
- Give RomM one explanatory subtitle: “Browse the cached RomM library; connection and refresh settings are under Sources.”
- Keep current `Advanced details` sections, but move raw IDs/paths/timings there consistently.
- In repair results, add an explicit “If you do nothing” sentence where currently only `why` implies the consequence.
- Add a visible “No changes made by opening this page” reassurance to conversion/diagnostic entry cards where appropriate.

## STRUCTURAL FIXES

1. Make Game Details the stable context root for game-specific work. Use task routes with a preserved `game` ID and a consistent “Back to game” action.
2. Introduce a user-facing “Readiness” vocabulary shared by first-run, Game Details, and Launch; keep the technical loadability/profile facts expandable.
3. Collapse Health into a novice “Problems & recovery” entry with separate advanced subviews for diagnostics, journals, storage, and repair history.
4. Separate “Browse my games” from “Manage sources/providers” in labels and subtitles, especially for RomM and DATs.
5. Define a controller-ready route subset rather than forcing controller interaction onto evidence/configuration pages.
6. Add route-level state-preservation tests for selected game, platform, search/filter, and scroll context when opening task routes or background activity.

## DO-NOT-CHANGE AREAS

- Keep preview/read-only behavior as the default; the scan dialog explicitly says it does not rename, repair, or change originals ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:438)).
- Keep simple explanation plus expandable technical evidence. Do not remove provenance, hashes, paths, or parser details; relocate them behind advanced sections.
- Keep native identity stronger than external/provider metadata. Do not turn RomM or artwork presence into verified identity.
- Keep explicit confirmation and recovery language for repair/organisation. The MAME workflow’s stale-plan and rollback refusal behavior is a useful safety baseline.
- Do not collapse specialist Saturn/Dreamcast/Wii U/disc evidence into generic prose; summarize it for novices and preserve the typed detail for advanced users.
- Do not touch cheat adapters, routing/loadability, transaction/history, emulator profiles, or concurrent reconciliation work as part of this audit.

## RECOMMENDED IMPLEMENTATION ORDER

1. Add a first-run progress model and “first verified game” completion state without changing scanning or identity semantics.
2. Add the compact selected-game readiness banner and consistent back-to-game/context preservation.
3. Rename/reframe novice labels and move implementation terminology into advanced explanations.
4. Unify Problems/Needs Attention/Doctor entry copy while retaining their distinct technical routes.
5. Link affected-game history/undo from action results and selected-game context.
6. Clarify RomM/source separation and expose conversion/disc/firmware status from Game Details.
7. Add controller-ready route tests and implement only the minimal focus/back/hotplug work needed for the Console Mode subset.
8. Audit large-library virtualization and nested scrolling after the navigation model stabilizes.

## CONCLUSION

GUI-v2 already contains most of the safe backend capability a novice needs, and its strongest pattern is “plain-language status first, advanced evidence on demand.” The principal product gap is orchestration: setup, verification, readiness, launch, recovery, and game-specific tools are individually present but not yet presented as one obvious journey. The recommended work is therefore cohesion and context preservation, not a broad feature rewrite.
