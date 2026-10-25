# Beginner UX Audit

Baseline: `4fbaebc`

Scope: source-level, read-only review of Home, Gamer View, Library, Emulator Setup, Doctor / Problems & Repair, DAT Sources, Library Organisation, and Cheats & Mods. Dynamic values are written in braces. No implementation changes are included.

The recommended presentation order throughout is:

1. Simple answer
2. Next action
3. Technical details

Priority meanings:

- **P0 — confusing/wrong:** the interface makes a materially false or contradictory promise.
- **P1 — blocks novice action:** a beginner is unlikely to know what to do next.
- **P2 — clutter:** correct information is too prominent, duplicated, or technical by default.
- **P3 — cosmetic:** wording or presentation polish with little effect on task completion.

## Home

### H1 — The prerequisite task is visually secondary

- **SCREEN:** Home, fresh install / empty library
- **CURRENT COPY:** The first primary card is “My Games” with “Add a source folder first”. “Build my library” — “EmuWiz needs one or more source folders before it can scan for archives.” — appears after eight primary cards as a secondary card.
- **PROBLEM:** The page foregrounds a blocked destination and buries the action that unblocks it. “Archives” is also implementation terminology at the first-run moment.
- **PROPOSED COPY:** “Add your games” — “Choose the folder where your games are stored. EmuWiz will scan it without changing the files.”
- **ACTION:** Promote this card to the first primary action whenever no source folder exists. De-emphasise or disable “My Games” until the first scan succeeds.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/home_page.rs:247-307`, `build_home_view`

### H2 — Verification starts with an unexplained acronym

- **SCREEN:** Home, Verify collection card
- **CURRENT COPY:** “Check your games with DATs.” / “Open DAT Sources” / “No DAT sources registered yet”.
- **PROBLEM:** A novice has to understand DAT files and the name of the configuration screen before understanding the task. The card says “Verify collection”, but its button opens a page titled “Verify Games”, so the navigation label does not confirm where the user arrived.
- **PROPOSED COPY:** “Verify your games” — “Check game names, versions, and known-good file matches using trusted game catalogues.” Button: “Verify games”. Technical details: “These catalogues are commonly called DAT files.”
- **ACTION:** Use the task name for the primary action and move “DAT Sources” to a technical/configuration label inside the destination page.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/home_page.rs:340-351`; `crates/archivefs-gui/src/dat_sources_page.rs:6022-6027`

### H3 — Unknown readiness badges repeat the destination action

- **SCREEN:** Home, Cheats & Mods / Verify collection / RomM cards
- **CURRENT COPY:** “Open Cheat Sources to check status”, “Open DAT Sources to check status”, and “Open RomM to check status”.
- **PROBLEM:** These badges add a second sentence that only repeats the card’s navigation button. “Unknown” looks like a problem even though the page simply has not lazily loaded that state.
- **PROPOSED COPY:** Omit the badge until status is known. If a status must be shown: “Status checked when opened.”
- **ACTION:** Reserve readiness badges for actionable Ready / Setup needed / Unavailable states.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/home_page.rs:265-285`, `build_home_view`

### H4 — Configuration warning names an old route and provides no route

- **SCREEN:** Home, missing configuration banner
- **CURRENT COPY:** “Configuration file is no longer found” / “Check Doctor before starting a new task.”
- **PROBLEM:** “Doctor” is internal product terminology, the current navigation destination is “Problems & Repair”, and the banner has no button. The user is warned not to continue but is left to discover the route.
- **PROPOSED COPY:** “EmuWiz settings could not be found” — “EmuWiz found your settings earlier, but they are no longer available. Check the problem before starting another task.” Button: “Check the problem”.
- **ACTION:** Add a direct action to Problems & Repair → Diagnostics and place file-path detail under Technical details.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/home_page.rs:474-483`, `show_home_page`

### H5 — Settings introduces mount terminology on the task launcher

- **SCREEN:** Home, Settings card
- **CURRENT COPY:** “Set up EmuWiz: sources, mounts, and preferences.”
- **PROBLEM:** “Sources” and “mounts” describe internal storage mechanics, not a beginner goal.
- **PROPOSED COPY:** “Choose game folders and preferences. Advanced storage options are available when needed.”
- **ACTION:** Keep mount terminology inside advanced settings and technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/home_page.rs:371-380`, `build_home_view`

## Gamer View

### G1 — “Open location” does not open a location

- **SCREEN:** Gamer View, selected game secondary actions
- **CURRENT COPY:** Button: “Open location”. On click, the folder path is copied to the clipboard and feedback says “Copied the game's folder location to the clipboard.”
- **PROBLEM:** The button makes a false promise. A beginner expects a file manager to open and may click repeatedly because nothing opens.
- **PROPOSED COPY:** If clipboard behaviour is retained: “Copy folder location”. If the intended behaviour is opening a file manager: keep “Open folder” and actually open it.
- **ACTION:** Make label and behaviour identical; do not use “Open” for a clipboard action.
- **PRIORITY:** P0 — confusing/wrong
- **CODE LOCATION:** `crates/archivefs-gui/src/gamer_view.rs:1598-1607`; `crates/archivefs-gui/src/main.rs:17581-17594`

### G2 — A row can say “Ready” while the selected game says “Needs setup”

- **SCREEN:** Gamer View, game list and selected game card
- **CURRENT COPY:** A mount-free game’s list status is “Ready”. The selected card may then say “EmuWiz found the game, but it cannot safely launch it yet.”
- **PROBLEM:** “Ready” naturally means ready to play, but here it means only that the media needs no mount. The same game can present contradictory readiness at once.
- **PROPOSED COPY:** List status: “Game found” or no readiness label. Reserve “Ready to play” exclusively for a completed safe launch plan.
- **ACTION:** Use one launch-aware readiness projection in both the list and selected card, or use a clearly non-readiness list label.
- **PRIORITY:** P0 — confusing/wrong
- **CODE LOCATION:** `crates/archivefs-gui/src/gamer_view.rs:89-102`, `gamer_primary_action_short_label`; `crates/archivefs-gui/src/gamer_view.rs:1517-1552`

### G3 — The mount conflict has no direct next action

- **SCREEN:** Gamer View, selected game blocked by an existing mount destination
- **CURRENT COPY:** “A file already exists at this game's mount destination. Open Advanced View's Mount page to resolve this.”
- **PROBLEM:** The warning leads with internal mount terminology and describes a multi-step route through an icon-only gear menu. There is no button to perform the stated next step.
- **PROPOSED COPY:** “This game cannot be prepared because another file is using the place EmuWiz needs.” Button: “Review file conflict”. Technical details: current mount-destination explanation and path.
- **ACTION:** Deep-link to the exact conflict in the Mount page.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/gamer_view.rs:69-81`, `gamer_primary_action`; `crates/archivefs-gui/src/gamer_view.rs:1549-1555`

### G4 — Game-information recovery is hidden in a tooltip and the visible action cannot fetch

- **SCREEN:** Gamer View → Details → Game information
- **CURRENT COPY:** “We couldn't load game information right now.” The visible button is “Update game information”; its tooltip says it only rereads cached data and that fetching requires “Sync in Sources → RomM”.
- **PROBLEM:** In the unavailable state, the obvious action is unlikely to fix the problem. The real next step is hidden in hover text and uses an Advanced View route.
- **PROPOSED COPY:** “No game information source is ready.” — “Connect or sync a game-information source, then try again.” Buttons: “Set up game information” and “Try cached information again”.
- **ACTION:** Show the real sync/setup route as a visible action and distinguish “sync” from “reread cache”.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/gamer_view.rs:835-887`, `show_game_information_provenance`

### G5 — Launch blockers expose planner wording by default

- **SCREEN:** Gamer View, selected game Needs setup state
- **CURRENT COPY:** The UI removes only the prefix “Can’t play yet:” and displays the remainder of the planner reason directly, followed by “Open Emulator Setup”.
- **PROBLEM:** Reasons can contain core, executable, identity, profile, or preflight terminology. The page has a simple lead and action, but technical detail remains between them.
- **PROPOSED COPY:** “RetroArch needs setup for this game.” — “Check the emulator, then return here to play.” Button: “Set up RetroArch”. Technical details: the exact planner refusal.
- **ACTION:** Map planner refusal categories to a small set of human summaries and place the original reason in Technical details.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/gamer_view.rs:161-166`, `humanize_play_blocker`; `crates/archivefs-gui/src/gamer_view.rs:1528-1548`

### G6 — Emulator brand and duplicate status compete with the action

- **SCREEN:** Gamer View, selected game ready/mounted states
- **CURRENT COPY:** “Play — Launch RetroArch”; mounted games show both a mounted status and “Currently mounted.”; a general `block_reason` is also printed below the primary state.
- **PROBLEM:** The beginner action should be the game goal, not the executor name. Repeating state and backend block text makes the action area harder to scan.
- **PROPOSED COPY:** Primary button: “Play”. Status: one line only. Technical details: “Opens with RetroArch” and any operation-wide block reason.
- **ACTION:** Keep one status explanation in the primary area and move executor/busy diagnostics into Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/gamer_view.rs:1505-1558`

## Library

### L1 — “My Games” opens an archive/mount administration table

- **SCREEN:** Library → Archives
- **CURRENT COPY:** Page title: “My Games”. Active tab: “Archives”. Default columns: “Platform”, “State”, “Archive path”, “Mount path”. Search hint: “archive, mount path, platform, or state”.
- **PROBLEM:** The beginner task and the default content are at different levels. The two widest columns are raw storage paths, while game title is absent.
- **PROPOSED COPY:** Default tab: “Games”. Columns: “Game”, “System”, “Ready to play”, “Location” (friendly folder name). Technical view: archive state and exact archive/mount paths.
- **ACTION:** Make game metadata the default table and put storage administration behind an Advanced columns/details control.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:265`, `COLUMN_HEADERS`; `crates/archivefs-gui/src/main.rs:3808-3834`, `show_library_shell_header`; `crates/archivefs-gui/src/library_view.rs:880-899`

### L2 — Selected game details lead with three raw paths and backend state

- **SCREEN:** Library → Archives → Selected game details
- **CURRENT COPY:** “Archive path”, “Mount path”, “Source”, “Mount state”, “Health”, “Metadata source”. For catalogue-only entries: raw archive path, raw source, and “Known to the library database, not confirmed by the latest live snapshot.”
- **PROBLEM:** Paths and persistence/mount state are shown before the game answer and next action. “Unassigned / Legacy” and “live snapshot” are internal terms.
- **PROPOSED COPY:** “This game is available / missing / still being checked.” — next relevant action. Technical details: exact paths, storage state, metadata provenance, last-seen timestamp.
- **ACTION:** Split the card into Summary, Actions, and collapsed Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/selected_game_panel.rs:166-305`, `show_selected_archive`

### L3 — Identity internals are expanded in the normal selected-game flow

- **SCREEN:** Library → Archives → Selected game details
- **CURRENT COPY:** “Structural evidence”, confidence badges, raw evidence values, and “Verified identity evidence” render between the metadata grid and game options.
- **PROBLEM:** These are useful diagnostics but interrupt the common task. Terms such as structural evidence and identity kind are not needed to mount, inspect, or open Cheats & Mods.
- **PROPOSED COPY:** “Game identified” / “Game system uncertain”. Button when needed: “Review identification”. Technical details: existing structural and verified evidence rows.
- **ACTION:** Collapse the evidence renderer by default and surface only its user-level verdict.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/selected_game_panel.rs:309-337`; `crates/archivefs-gui/src/selected_evidence_page.rs:684-738`, `show_identity_evidence`

### L4 — A permanent Info banner explains an operation the game does not need

- **SCREEN:** Library → Archives → loose ROM selected
- **CURRENT COPY:** “No mount required” / “Loose ROM · no EmuWiz mount required. Inspect, Cheats & Mods, copy-path, and library metadata actions remain available.”
- **PROBLEM:** Healthy games receive a visually prominent Info box full of mount and copy-path terminology. It explains an absent requirement instead of helping the user act.
- **PROPOSED COPY:** Omit the banner. If reassurance is needed, show a small status: “Ready to use directly.”
- **ACTION:** Reserve banners for exceptions requiring attention.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/selected_game_panel.rs:358-368`

### L5 — Empty Library tells the user what to do but offers no action

- **SCREEN:** Library → Archives, empty state
- **CURRENT COPY:** “No games yet” / “Add a source or scan your library to find games.” No empty-state action is supplied.
- **PROBLEM:** The user has to leave the page and discover where a source or scan control lives.
- **PROPOSED COPY:** “No games found yet” — “Choose the folder where your games are stored, then EmuWiz will scan it.” Button: “Add game folder”. Secondary action when configured: “Scan again”.
- **ACTION:** Wire the empty-state action to Sources and the configured state to Scan.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/library_view.rs:1215-1223`; `crates/archivefs-gui/src/library_view.rs:2272`

### L6 — Recently Found exposes a scan run identifier

- **SCREEN:** Library → Recently Found
- **CURRENT COPY:** “Scan {scan_run_id}”, followed by Added / Updated / Skipped / Errors counters.
- **PROBLEM:** The scan run ID is a diagnostic identifier with no beginner meaning. It competes with the useful result count.
- **PROPOSED COPY:** “Latest scan” — “Found {added} new games and updated {updated} existing entries.” Technical details: scan ID, skipped categories, exact error count.
- **ACTION:** Put the run ID and detailed counters behind Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/library_view.rs:87-112`, `show_loaded_data`

## Emulator Setup

### E1 — RetroArch can have two contradictory readiness summaries

- **SCREEN:** Emulator Setup
- **CURRENT COPY:** The “Emulator readiness” list derives a RetroArch row from the last Doctor scan when available; the dedicated RetroArch card below derives its status from a separately refreshed profile/core scan.
- **PROBLEM:** One screen can show stale “Needs attention”, “Evidence found”, or “Not checked” beside a fresh dedicated “Ready” card, or the reverse. The same emulator has two authorities in the presentation.
- **PROPOSED COPY:** Show one RetroArch summary: “Ready for {n} systems” or “Needs setup”, from the dedicated current discovery. Put older Doctor evidence in Technical details with its timestamp.
- **ACTION:** Make the dedicated RetroArch readiness projection the single displayed authority and remove/merge the duplicate summary row.
- **PRIORITY:** P0 — confusing/wrong
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:6230-6312`, nested `show_emulator_setup_summary`; `crates/archivefs-gui/src/main.rs:6350-6429`, `show_retroarch_core_folder_card`

### E2 — “Run Doctor below” points to a hidden control

- **SCREEN:** Emulator Setup, before a Doctor scan
- **CURRENT COPY:** “Run Doctor below to collect the current emulator evidence.” The only Doctor UI is inside a collapsed “Full diagnostics” disclosure.
- **PROBLEM:** There is no visible “Run Doctor” button below unless the user first expands an unrelated-sounding technical section.
- **PROPOSED COPY:** “Emulator checks have not run yet.” Button: “Check emulators”. Secondary disclosure: “Full diagnostics”.
- **ACTION:** Put the scan action in the summary card; do not make the user expand diagnostics to perform the stated next step.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:6313-6322`, `show_emulator_setup_page`

### E3 — Nine empty rows repeat the same non-result

- **SCREEN:** Emulator Setup, no Doctor result
- **CURRENT COPY:** Every listed emulator shows “Not checked” and “No emulator-specific evidence was returned by the last scan.”
- **PROBLEM:** Repeating the same empty explanation up to nine times creates visual noise and makes supported-vs-installed status hard to understand.
- **PROPOSED COPY:** One empty state: “Emulators have not been checked.” After scanning, show discovered emulators first and put “Not found” emulators in a collapsed “Other supported emulators” group.
- **ACTION:** Group identical empty states and progressively disclose absent emulators.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:6238-6311`, nested `show_emulator_setup_summary`

### E4 — Finding explanations and installation evidence are dumped inline

- **SCREEN:** Emulator Setup, emulator readiness rows
- **CURRENT COPY:** Each match prints `finding.explanation`; installation matches add “Installation evidence:” followed by every evidence line.
- **PROBLEM:** Core finding prose and evidence may contain paths, profile identifiers, or diagnostic detail. It is shown by default beside the readiness badge rather than behind Technical details.
- **PROPOSED COPY:** “Ready”, “Found, setup incomplete”, or “Not found”, followed by one actionable sentence. Technical details: original explanation and evidence lines.
- **ACTION:** Translate finding states into a small presentation model and collapse raw evidence.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:6243-6310`, nested `show_emulator_setup_summary`

### E5 — Core-scan failure gives technical detail but not a recovery sequence

- **SCREEN:** Emulator Setup → RetroArch
- **CURRENT COPY:** “RetroArch core check could not finish” / “Open Technical details for the exact error.” Controls separately say “Rescan cores” and “Choose core folder”.
- **PROBLEM:** The warning answers what happened but not which visible action to try first. “Core”, “libretro core”, and “core folder” are also unexplained prerequisites.
- **PROPOSED COPY:** “RetroArch could not be checked.” — “Try checking again. If it still fails, choose RetroArch’s game-support folder.” Primary: “Try again”. Secondary: “Choose folder”. Technical details: “libretro core” terminology and exact error.
- **ACTION:** Order and label recovery actions in the sentence, then retain the raw failure under Technical details.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/retroarch_core_setup.rs:188-267`, `core_folder_readiness`; `crates/archivefs-gui/src/main.rs:6383-6429`

## Doctor / Problems & Repair

### D1 — The primary action uses the product metaphor, not the task

- **SCREEN:** Problems & Repair → Diagnostics
- **CURRENT COPY:** Button: “Run Doctor”; status: “Not run yet”; empty state: “No scan has run yet”.
- **PROBLEM:** “Doctor” is an internal feature name. A new user on a page already called Problems & Repair has to infer that running it means checking for problems.
- **PROPOSED COPY:** Button: “Check for problems”. Status: “Not checked yet”. Empty state: “No check has run yet.” Technical details/report title may retain “Doctor”.
- **ACTION:** Use task language on controls and reserve the feature name for technical/report contexts.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/doctor_page.rs:82-151`, `show_doctor_page`

### D2 — Two long safety explanations precede the result

- **SCREEN:** Problems & Repair → Diagnostics
- **CURRENT COPY:** A one-line list of checks plus a long paragraph beginning “This scan is read-only…” and enumerating creates, mounts, unmounts, repairs, rebuilds, free-space access, profiles, cheats, and patches.
- **PROBLEM:** Safety is important, but the full contract appears on every visit and delays the answer. Much of it is implementation-level reassurance.
- **PROPOSED COPY:** “Checking is read-only and will not change your files.” Technical details: the current complete safety contract and coverage list.
- **ACTION:** Keep one short reassurance by the action and move the exhaustive promise into Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/doctor_page.rs:98-106`; `crates/archivefs-gui/src/doctor_page.rs:793-804`, constants

### D3 — Healthy status is repeated across four surfaces

- **SCREEN:** Problems & Repair Overview and Diagnostics after a healthy scan
- **CURRENT COPY:** Overview: “Healthy” and “No problems were found…”; Diagnostics top badge: “Healthy”; severity counters; banner: “Healthy / No problems detected…”.
- **PROBLEM:** The answer is correct but repeated instead of using the space for the next useful choice or coverage caveat.
- **PROPOSED COPY:** One lead: “No problems found.” Actions: “Check again” and “See what was checked”.
- **ACTION:** Remove the duplicate Healthy banner/counters when all actionable counts are zero; keep coverage in one disclosure.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/problems_repair_page.rs:75-107`; `crates/archivefs-gui/src/doctor_page.rs:78-92`, `153-183`

### D4 — Every finding exposes its raw resource path by default

- **SCREEN:** Problems & Repair → Diagnostics, finding cards
- **CURRENT COPY:** “Resource: {affected.display}”. If text conversion is lossy, another sentence explains invalid path bytes.
- **PROBLEM:** Absolute paths and encoding diagnostics appear before the user asks for details. They can be long, alarming, and obscure the recommended action.
- **PROPOSED COPY:** Use a friendly affected item name in the card, for example “Game folder: PS2” or “RetroArch settings”. Technical details: exact path and path-encoding note.
- **ACTION:** Move `EncodedPath.display` and lossy-path diagnostics into the existing Details area.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/doctor_page.rs:509-526`, `show_doctor_finding_card`

### D5 — Actionable findings are allowed to have no next step

- **SCREEN:** Problems & Repair → Diagnostics, finding cards
- **CURRENT COPY:** The card always shows title and explanation, but “Recommended next step” renders only when optional `finding.next_step` is populated. Adapted Doctor checks and adapter failures can be Warning/Error findings without guidance.
- **PROBLEM:** Some cards can stop at “what happened”. The user sees a problem badge but neither an action nor an explicit “no action available” statement.
- **PROPOSED COPY:** Every non-informational finding should end with one of: “Do this next: …”, a direct “Review repair” action, or “EmuWiz cannot repair this automatically; see Technical details.”
- **ACTION:** Require a presentation fallback for missing guidance and audit the core adapters to supply specific next steps.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/doctor_page.rs:708-723`, `show_doctor_finding_details`; `crates/archivefs-core/src/diagnostics/mod.rs:764-779`, `finding_from_doctor_check`; `crates/archivefs-core/src/diagnostics/runner.rs:657-680`, `adapter_failure_finding`

### D6 — Details ends with a raw diagnostic code

- **SCREEN:** Problems & Repair → Diagnostics → Details
- **CURRENT COPY:** “Reported by {subsystem} · finding ID {finding.id}”.
- **PROBLEM:** The code is appropriately gated once, but it is presented as ordinary footer copy rather than clearly technical/copyable support data.
- **PROPOSED COPY:** “Technical reference” → “Check: {friendly subsystem}” and a copyable “Diagnostic ID: {id}”.
- **ACTION:** Put provenance and finding ID in a nested Technical details/support block.
- **PRIORITY:** P3 — cosmetic
- **CODE LOCATION:** `crates/archivefs-gui/src/doctor_page.rs:759-765`, `show_doctor_finding_details`

## DAT Sources / Verify Games

### T1 — Three safety/format notices appear before the task

- **SCREEN:** Verify Games / DAT Sources, top of page
- **CURRENT COPY:** Banner: “Your files are safe”; a second read-only promise; then “Supported formats: Logiqx XML, ClrMamePro text, MAME software-list XML”.
- **PROBLEM:** The banner and paragraph duplicate the same reassurance, while parser format names are technical detail. The user has not yet seen the primary verification action.
- **PROPOSED COPY:** “Verify without changing files.” — “Choose a trusted catalogue and the games to check.” Technical details: supported parser formats and the full read-only contract.
- **ACTION:** Keep one short safety line and move format support into Technical details/help.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/dat_sources_page.rs:6022-6060`, `show_dat_sources_page`

### T2 — Evidence acquisition assumes catalogue expertise

- **SCREEN:** Verify Games → Get evidence for your library
- **CURRENT COPY:** Phrases include “DAT-o-MATIC”, “internal metadata”, “durable pack resolver”, “System / Category / Media”, “Retroplay-derived LHA package checksums”, “fixed built-in MAME contract”, and “Logiqx / ClrMamePro DAT”.
- **PROBLEM:** The first-choice cards explain authority and adapter constraints before explaining which card a user with a Game Boy, PS2, Amiga, or arcade library should choose.
- **PROPOSED COPY:** Lead each card with game systems and one action: “Cartridge games — Nintendo, Sega, Atari and more. Download a No-Intro catalogue, then choose the ZIP.” Put resolver, checksum, metadata, and parser details under Technical details.
- **ACTION:** Add a “What games do you want to verify?” chooser/recommendation before source mechanics.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/dat_sources_page.rs:6263-6440`, `show_evidence_acquisition_section`

### T3 — Every local source card shows an internal ID and raw path

- **SCREEN:** Verify Games → Local DAT Sources
- **CURRENT COPY:** `ID: {row.id}` in monospace; `{kind_label} · {row.path}`; `Format: …`.
- **PROBLEM:** Registry identity and an absolute path are displayed before health and next action. A friendly source name already exists, so these details are redundant in the summary.
- **PROPOSED COPY:** “{display name}” — “Ready / Needs attention / Not checked.” Next action: “Check catalogue”. Technical details: source ID, kind, exact path, parser format.
- **ACTION:** Move ID/path/format into the existing Inspect or a Technical details disclosure.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/dat_sources_page.rs:8080-8142`, `show_source_row`

### T4 — The page always exposes its configuration-file path

- **SCREEN:** Verify Games, source toolbar
- **CURRENT COPY:** “File: {view.config_path}”.
- **PROBLEM:** The raw registry/configuration path has no role in adding, saving, or validating a catalogue.
- **PROPOSED COPY:** Omit it from the toolbar. Technical details: “Catalogue settings file: {path}” with Copy/Open-folder actions.
- **ACTION:** Move the path to Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/dat_sources_page.rs:7905-7992`, `show_toolbar`

### T5 — “Registry” is used as normal task language

- **SCREEN:** Verify Games, load/save/remove states
- **CURRENT COPY:** “Registry not read”, “Registry saved”, “Remove from registry”, and “will no longer be registered”.
- **PROBLEM:** “Registry” sounds like a system database or Windows Registry and does not tell the user that only EmuWiz’s catalogue list changes.
- **PROPOSED COPY:** “Catalogue list could not be loaded”, “Catalogue sources saved”, “Remove from EmuWiz”, and “EmuWiz will stop using this catalogue; the original file will remain.”
- **ACTION:** Replace registry terminology in beginner-facing copy; retain the storage term only in Technical details/logs.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/dat_sources_page.rs:6028-6039`, `7970-7982`, `8214-8244`

### T6 — Catalogue warnings say “needs attention” without a next step

- **SCREEN:** Verify Games, local source diagnostics
- **CURRENT COPY:** “{n} catalogue issues found” / “Some files could not be used and need your attention.” The only follow-up is a collapsed “What happened?” section containing parser diagnostics.
- **PROBLEM:** The warning does not say whether to re-download, remove, replace, split, or simply ignore a file. It offers technical evidence but no decision.
- **PROPOSED COPY:** “Some catalogue files could not be read.” — “Open the issue list to see which files to replace or remove.” Button: “Review issues”. Each issue should end with a source-specific next step or “No action needed”.
- **ACTION:** Rename the disclosure to an action and add guidance alongside each grouped diagnostic.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/dat_sources_page.rs:8391-8460`, `show_diagnostics_summary`; `crates/archivefs-gui/src/dat_sources_page.rs:8320-8390`, `show_diagnostic_group`

### T7 — One page exposes every acquisition and policy subsystem at once

- **SCREEN:** Verify Games / DAT Sources, full page
- **CURRENT COPY:** The default flow includes evidence acquisition, add/save toolbar, local sources, managed sources, MAME form, Redump BIOS, Redump games, TOSEC packs, policy editors, audit output, and rename planning.
- **PROBLEM:** The page is organised around source architecture rather than the user’s current task. A beginner cannot tell whether they need Local, Managed, TOSEC, a policy, or an audit target before verifying one game.
- **PROPOSED COPY:** First step: “Choose games to verify.” Second: “Recommended catalogue for these systems.” Third: “Check games.” Link: “Manage catalogues and matching preferences”.
- **ACTION:** Create a task-first verification summary and move source/policy administration behind Manage catalogues / Advanced.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/dat_sources_page.rs:6015-6257`, `show_dat_sources_page`; sections beginning at `7098`, `7648`, and `9026`

## Library Organisation

### O1 — “Build Playing Library” is an unexplained alternate workflow

- **SCREEN:** Organise, top action row
- **CURRENT COPY:** Button: “Build Playing Library”. It appears above the normal folder and organisation choices with no description until after it is opened.
- **PROBLEM:** A novice cannot distinguish a playing library from organising, moving, renaming, or building a linked library.
- **PROPOSED COPY:** “Create an emulator-ready library” — “Build a separate library layout for an emulator frontend while keeping your originals safe.”
- **ACTION:** Add a one-sentence explanation or move this alternate workflow into the mode chooser with a clear comparison.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/rom_organisation_page.rs:842-861`, `show_rom_organisation_page`

### O2 — The mode chooser asks for a storage strategy before recommending a safe goal

- **SCREEN:** Organise → Organisation mode
- **CURRENT COPY:** “Rename files where they are”, “Move files into organised folders”, “Build linked library”, and “Advanced: reorganise existing symlinks”. The default state is `MoveRealFile`.
- **PROBLEM:** The choices mix user goals with filesystem implementation. The default is the option that moves original files, while “linked library” and “symlink” are unexplained technical concepts.
- **PROPOSED COPY:** Recommended: “Create an organised copy of your library without moving originals”. Alternatives: “Rename originals in place” and “Move originals”. Technical options: linked-library/symlink terminology.
- **ACTION:** Lead with effect on original files, mark a non-destructive recommendation, and place the symlink-only mode under Advanced.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/rom_organisation_page.rs:106-144`, presentation helpers; `crates/archivefs-gui/src/rom_organisation_page.rs:1029-1048`

### O3 — Raw root paths are shown as ordinary status

- **SCREEN:** Organise, game library / linked library folder cards
- **CURRENT COPY:** “Folder: {root.display()}”.
- **PROBLEM:** The destination is important, but a long absolute path dominates the card. There is no simple folder name or role before the path.
- **PROPOSED COPY:** “Organised library: {folder name}” / “Original games: {folder name}”. Technical details: exact path with Copy and Change folder actions.
- **ACTION:** Show role + friendly folder name first and keep the exact path one disclosure away.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/rom_organisation_page.rs:867-1019`, `show_rom_organisation_page`

### O4 — Progress copy exposes identity-pipeline terminology

- **SCREEN:** Organise, preview preparation
- **CURRENT COPY:** “Preparing preview… scanning games and resolving platform identity.”
- **PROBLEM:** “Resolving platform identity” describes the backend rather than progress the user can understand.
- **PROPOSED COPY:** “Preparing your preview… checking each game and deciding where it belongs.”
- **ACTION:** Keep resolver/evidence detail in logs or Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/rom_organisation_page.rs:1067-1075`

### O5 — Every preview row leads with raw paths and provenance

- **SCREEN:** Organise, plan preview
- **CURRENT COPY:** `{source_path} → {destination_path}` in monospace, or “Source: {path} / Destination link: {path} / Source action: Untouched”; then `{platform_display_name} · {platform_source}`.
- **PROBLEM:** Exact paths are needed for a safe final review, but they are the primary presentation for every row. Internal provenance (`platform_source`) is also shown without a label. The user has to parse paths to learn what will happen.
- **PROPOSED COPY:** “{game name}” — “Will move to {system}” / “Will create a link; original stays where it is.” Technical details: exact source, destination, platform source, and conflict reason.
- **ACTION:** Add a plain per-game action summary and collapse exact path/provenance until requested; preserve full paths in final confirmation.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/rom_organisation_page.rs:1174-1227`, `show_plan`

### O6 — Preview safety is stated three times

- **SCREEN:** Organise, before and after preview
- **CURRENT COPY:** Page header: “Nothing moves until you approve it”; folder card: “organising always requires a preview and your explicit approval”; banner: “Preview only / Nothing changes until you review this preview and explicitly approve it.”
- **PROBLEM:** Repeated Info copy adds visual weight without adding a new decision.
- **PROPOSED COPY:** Keep one concise line near the first mutating action: “Preview only — originals will not change until you approve.”
- **ACTION:** Remove the duplicate banner/body notices and keep mode-specific impact in confirmation.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/rom_organisation_page.rs:830-837`, `1019-1027`, `1084-1093`

## Cheats & Mods

### C1 — RetroArch profile cards expose raw IDs and blocker codes

- **SCREEN:** Cheats & Mods, RetroArch profile selection
- **CURRENT COPY:** Radio label: `{profile.profile_id}`; first blocker: `{blocker.code} — {blocker.detail}`. Only additional blockers are placed in Technical details.
- **PROBLEM:** The most prominent choice and first error use diagnostic identifiers. A beginner cannot tell which profile corresponds to the RetroArch they use or what a blocker code means.
- **PROPOSED COPY:** “RetroArch — system installation / Flatpak / portable” with a friendly location name. “This profile cannot be used because {plain reason}.” Technical details: profile ID, blocker codes, exact paths.
- **ACTION:** Add a profile presentation model and put all raw blocker codes behind Technical details.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:29506-29619`, `show_cheat_workflow_step1`

### C2 — The beginner route starts with infrastructure choices

- **SCREEN:** Cheats & Mods, selected RetroArch game
- **CURRENT COPY:** “Choose a RetroArch profile”, then “Cheat source”, then “Existing RetroArch cheats” versus “EmuWiz cheat catalogue”, then source checking/retrieval before candidates.
- **PROBLEM:** The user’s goal is to see compatible enhancements, but the primary flow asks them to choose profile and catalogue architecture first. On a single eligible profile/source, these are unnecessary decisions.
- **PROPOSED COPY:** “Checking for compatible cheats…” then show results. If one destination is safe, select it visibly. Ask “Which RetroArch setup do you use?” only when multiple eligible installations exist. Put source choice under “Where these cheats came from”.
- **ACTION:** Auto-advance unambiguous safe choices while preserving explicit choice for ambiguity; make cheats/results the first visible task.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/cheats_mods_preview.rs:1123-1290`, `show_cheats_mods_page`; `crates/archivefs-gui/src/main.rs:28828-28915`, `show_cheat_source_modes`

### C3 — Candidate results display catalogue paths by default

- **SCREEN:** Cheats & Mods → Candidate matches
- **CURRENT COPY:** Each candidate card prints `candidate.catalogue_relative_path` beneath the title and cheat count.
- **PROBLEM:** A catalogue-relative filename is provenance/debug data, not part of deciding whether to use the candidate.
- **PROPOSED COPY:** Keep title, platform, region, match strength, and cheat count. Technical details: “Catalogue item: {relative path}”.
- **ACTION:** Move the path into candidate Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/cheats_mods_preview.rs:271-317`, `show_cheat_candidate_stages`

### C4 — The preview summary leads with transaction internals

- **SCREEN:** Cheats & Mods → Shared install preview
- **CURRENT COPY:** “Trusted source materialized”, “Snapshot ID”, “Upstream revision”, “Immutable snapshot”, “Catalogue index: …”; then “Adapter · {debug enum}”, hashed source/destination counts, total bytes, and paths inspected.
- **PROBLEM:** Technical provenance is valuable, but it precedes the simple answer about how many cheats will be installed, left unchanged, replaced, or blocked.
- **PROPOSED COPY:** “Ready to install {n} cheat file(s). {m} already installed; {k} need attention.” Technical details: snapshot, adapter, index, hashes, byte/path counts.
- **ACTION:** Reverse the hierarchy: result and next action first, provenance in Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/cheats_mods_preview.rs:652-738`, `show_shared_cheat_preview`

### C5 — Apply review is written as an engine contract

- **SCREEN:** Cheats & Mods → Review and controlled apply
- **CURRENT COPY:** Six numbered status badges; “Transaction engine available”; “Actionable materialized entries in this page state”; “General confirmation is operation-scoped. Replacement permission is separate…”
- **PROBLEM:** This is correct safety architecture, but the main review reads like a developer specification. A beginner has to decode “materialized”, “transaction engine”, and “operation-scoped” before seeing whether anything can be installed.
- **PROPOSED COPY:** “Ready to install” — “EmuWiz will back up changed files and verify the result. You can undo this install.” Button: “Review changes”. Technical details: the six-stage contract, materialization state, and replacement rules.
- **ACTION:** Replace the default contract card with a plain outcome/action summary; retain exact guarantees in Technical details and final confirmation.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/cheats_mods_preview.rs:875-970`, `show_shared_transaction_readiness`

### C6 — The PS2 path remains a numbered technical workflow

- **SCREEN:** Cheats & Mods, PlayStation 2 game
- **CURRENT COPY:** “Stage 1 · PCSX2 profile”, “Stage 2 · Existing PCSX2-managed files”, “Inspect existing PNACH files”, “Exact matching requires a verified CRC calculated from the complete bounded boot ELF”, and raw configuration/patch directory paths.
- **PROBLEM:** Unlike the simplified Dolphin presentation, the PS2 beginner route exposes profile discovery, PNACH format, CRC, ELF, writability, and inventory mechanics before compatible cheats. This is a major novice dead end.
- **PROPOSED COPY:** “Checking this PS2 game…” → “Game verified / Could not verify this game.” → compatible cheat checklist → “Install selected”. Details: PCSX2 profile, PNACH inventory, serial/CRC/ELF evidence, paths, and inspection limits.
- **ACTION:** Add a PS2 beginner summary using the existing safe identity/profile/install states and put the current numbered workflow under Details.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:23699-24080`, `show_pcsx2_workflow`; `crates/archivefs-gui/src/main.rs:26390-26520`, `show_pcsx2_inventory`

### C7 — The PS2 profile’s first blocker is a raw enum

- **SCREEN:** Cheats & Mods, PCSX2 profile card
- **CURRENT COPY:** Configuration and patch-directory paths are shown inline; the first blocker is `{:?} — {detail}`.
- **PROBLEM:** The raw enum variant is a diagnostic code and the paths dominate the selection. Additional blockers are technical details, but the first is not.
- **PROPOSED COPY:** “This PCSX2 setup is ready / cannot be used.” — specific plain next step. Technical details: profile ID, installation/scope, exact paths, and every blocker enum/detail.
- **ACTION:** Apply the same plain profile presentation proposed for RetroArch and move all blocker/path evidence together.
- **PRIORITY:** P1 — blocks novice action
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:26320-26383`, `show_pcsx2_profile_card`

### C8 — Safety status strips and result journals are shown as normal content

- **SCREEN:** Cheats & Mods, PS2 existing-file inspection and shared apply result
- **CURRENT COPY:** Five badges: “Unverified local content”, “Read-only”, “Uploaded · No”, “Executed · No”, “Changed · No”; result content includes “Operation ID”, debug-formatted outcomes/failure kinds, “Journal”, and “Journal failed after transaction work”.
- **PROBLEM:** Reassurance is fragmented into badges, and the post-action answer is mixed with journal/enum diagnostics. The user needs one safety sentence before the action and one success/failure/undo answer after it.
- **PROPOSED COPY:** Before: “This check stays on your computer and does not run or change cheat files.” After: “Installed and verified” / “Some cheats were not installed” with “Undo” or “Review problem”. Technical details: operation ID, journal path, enum outcome, backup and failure records.
- **ACTION:** Consolidate safety into one sentence and move operation/journal mechanics under Technical details.
- **PRIORITY:** P2 — clutter
- **CODE LOCATION:** `crates/archivefs-gui/src/main.rs:23798-23823`; `crates/archivefs-gui/src/cheats_mods_preview.rs:1000-1088`, `show_shared_transaction_readiness`

## Recommended implementation order

1. Fix P0 truth/consistency issues: “Open location”, Gamer View list readiness, and duplicate RetroArch authorities.
2. Add direct next actions for empty/error states: Home configuration, Library empty state, Gamer metadata/mount conflicts, Emulator Setup scan, Doctor guidance fallback, and DAT diagnostics.
3. Introduce reusable summary → action → Technical details presentation models for profiles, launch blockers, selected games, and transaction results.
4. Reduce default-path/ID/provenance exposure and remove duplicated Info/safety banners.
5. Polish remaining terminology and cosmetic diagnostic references.
