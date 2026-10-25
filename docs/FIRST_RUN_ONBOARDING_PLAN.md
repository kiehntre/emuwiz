# First-Run Onboarding Plan

Baseline: `8425042` ("feat(gui): install local PCSX2 cheat files safely") — one commit ahead of the `db8092d` checkpoint this task was framed against (`db8092d` + the immediately following PCSX2 cheat-install commit, `8425042`, which is untouched by this document and by everything proposed here). Read-only investigation; no production code was changed to produce this plan.

Scope: source-level review of `crates/archivefs-gui` (Home, Sources, DAT Sources, Doctor, Emulator Setup, Playing Library, RomM, navigation, config persistence) and `crates/archivefs-core` (config, source folders, DAT audit, launch compatibility, diagnostics). Cheats & Mods / `cheat_journey` / local cheat install (PCSX2 and generic) code was read only as a reference pattern, per instruction, and is not touched or referenced as an implementation dependency anywhere below.

## 1. Executive summary

EmuWiz already has almost all of the raw material a first-run flow needs — it just has no single guided path stitching it together. Home (`crates/archivefs-gui/src/home_page.rs`) already renders an honest fresh-install banner and task-oriented cards with real readiness state; the Setup/Diagnostics screen (`main.rs::show_setup_diagnostics`) already contains a "Welcome to EmuWiz" message that correctly states the app's core promises (folders are scanned without being changed, DATs/Cheat Sources/RomM are optional, audits are read-only). What is missing is not explanation content — most of the honest wording already exists — but a short, resumable, *sequenced* path for a person who does not yet know that Home's cards are the map.

This plan recommends a **hybrid**: a compact 5-step onboarding overlay that reuses Home's own data (`HomeInputs`/`build_home_view`) and existing pages by deep-linking into them, rather than duplicating their logic in new screens. The four step *bodies* that touch existing functionality (Add a source, DAT setup, Emulator discovery, Verify) are REUSE — they render existing pages' own logic in place. The remaining new work — the Welcome content screen, the step-tracker shell itself, its persisted state flag, and the "Run setup again" entry point — is all SMALL. A single unified "is my library basically usable" readiness rollup does not exist today in a directly reusable single-call form; it is not required for v1 (step 4 works from the existing per-candidate list instead) and is called out in §8 as a separable, optional SMALL follow-up. Nothing in the recommended v1 requires DEFERred work to ship a genuinely useful first pass.

Proposed step count: **5** (Welcome → Add a source → Optional DAT setup → Emulator setup → Verify & finish), with RomM and ES-DE explicitly kept *out* of the guided steps and instead only mentioned as "later, optional" links, because forcing either would misrepresent them as required.

## 2. Current first-run experience

Today, a brand-new user:

1. Launches EmuWiz. If no config file exists yet, `missing_config_is_first_run` (`crates/archivefs-gui/src/main.rs:21659`) evaluates `true` (config was never previously confirmed present this session), and the Setup/Diagnostics screen shows a "Welcome to EmuWiz" group box (`main.rs:21712-21732`) explaining: EmuWiz is not configured yet, that's expected; select "Create Starter Config", then add a source folder on Sources; DAT Sources and Cheat Sources are optional and start empty; RomM is optional; audits are always read-only.
2. After confirming the starter config, the user lands on Home. `build_home_view` (`home_page.rs:238`) computes `HomeBanner::FreshInstall` and shows a second, shorter "Welcome to EmuWiz" banner plus a full grid of task cards. When `source_folder_count == 0`, the "Add your games" card is promoted to the first primary slot with accent colour and the message "Choose the folder where your games are stored. EmuWiz will scan it without changing the files." (`home_page.rs:306-333`, `home_page.rs:461-468`) — this already reflects the fix documented in `docs/BEGINNER_UX_AUDIT.md` H1.
3. From here the user is on their own: nothing sequences "now go check DAT Sources," "now check Emulator Setup," or "now verify." Every other card (Verify, Set up emulators, RomM, Cheats & Mods, Organise, Convert discs, Settings) is presented with equal visual weight once a source exists, and a user has to intuit the order (source → maybe DAT → emulators → verify) themselves.
4. There is no dedicated onboarding wizard, no step tracker, no "Run setup again" entry point, and no persisted "onboarding complete/dismissed" flag anywhere in the codebase (confirmed by grep across `archivefs-gui` and `archivefs-core` for `onboarding`, `first_run`, `welcome`, `first launch` — the only durable state is the `config_previously_confirmed` in-session boolean and the Doctor/diagnostics "expected first-run absence" classifications, neither of which is a cross-session onboarding-progress flag).
5. `gamer_view.rs`/`gamer_view/stage.rs` has its own, narrower first-run affordance: when the (separate) Gamer View library is empty, the stage shows an "Add games" call-to-action (`gamer_view_shows_add_games_button`, `gamer_view.rs:561`) that only appears for the true first-run empty state, not for an already-populated-but-currently-filtered view. This is a good precedent for "only claim first-run when it's real," but it is local to Gamer View's stage and does not sequence anything beyond the one folder-add action.

Net: the pieces are honest and non-duplicative, but a novice currently has to read Home's whole card grid to reconstruct the intended sequence themselves.

## 3. Existing reusable seams

| Concern | Existing backend | Existing GUI page/module | Notes |
|---|---|---|---|
| Fresh-install detection | `missing_config_is_first_run(bool)` (`main.rs:21659`); `Config::load_default`/`load_from` (`archivefs-core/src/lib.rs:608-635`) | `show_setup_diagnostics` welcome banner (`main.rs:21712`); `HomeBanner::FreshInstall` (`home_page.rs:150,239-247`) | Two banners already agree on the same predicate; onboarding should reuse the predicate, not add a third. |
| Add a source folder | `add_source_folder_default` / `add_source_folder_at` (`archivefs-core/src/lib.rs:3181-3189`) | `sources_page.rs` (Sources page, "Libraries" tab); folder-add flow already wired from Home's "Add game folder" action and from Gamer View's empty-stage CTA | Folder picker + scan already exist; onboarding step 2 should deep-link here, not reimplement a picker. |
| DAT / identification | `archivefs_core::dat::*` (parsing, `audit_files` in `dat/audit.rs:179`, managed sources in `dat/managed_sources.rs`) | `dat_sources_page.rs` ("DAT Sources" page — register/validate/audit); `verify_summary.rs` (compact Verify health rollup); `dat_coverage_panel.rs` | Explicitly documented as read-only in the page's own module doc ("Nothing here writes to a ROM... no rename, move, delete... none is deferred behind a flag — the capability is simply not present"). Optional today (0 sources is a normal, non-error state per `home_page.rs:282-291`). |
| Verify / library health | `verify_summary::build` (`verify_summary.rs:39`) turning `DatSourcesPageView` state into `VerifyHealthView` | Rendered inside DAT Sources page (Verify tab); `home_page.rs` "Verify your games" card reuses `dat_sources_registered_count` | Purely a projection of already-loaded state — never triggers scans/audits itself. |
| Doctor / setup health | `archivefs_core::diagnostics` (`SetupDiagnostics`, `runner.rs`); `SetupCheckSummary` (`home_page.rs:219-233`) built from `ArchiveFsApp::doctor_scan` | `doctor_page.rs` (manual deep scan, read-only findings + explicit confirm-to-repair flow) | Doctor findings are read-only by construction; only the confirm screen (`DoctorPageAction::ConfirmRepair`) mutates, and only after an explicit review step — the exact "preview-then-confirm" pattern this plan should point to. |
| Emulator discovery & readiness | `archivefs_core::launch::{LAUNCH_COMPATIBILITY, LaunchCompatibility}` (`launch/platform_map.rs:82`); per-platform/adapter evidence | `emulator_setup_page.rs` (candidate-first list; `CandidateState`: Ready / Warnings / NeedsSetup / Blocked / NotChecked, each with an evidence list) | Module doc: candidates are "deliberately projected from core's reviewed launch compatibility table," never invented from a plausible-sounding executable name. This is the natural target for onboarding's emulator step. |
| Playing Library / "Playing Library" concept | `archivefs_core::playing_library::{build_playing_library_plan, build_playing_library_transaction}` | `playing_library_page.rs`, reached as a *mode* of Library Organisation, not its own sidebar destination | 1G1R planning + RetroDeck/RomM projection preview; apply always goes through the shared `rename_apply` executor with typed confirmation above a threshold (`playing_library_confirmation_phrase`). Not the same thing as "browsing your library" — it is an opt-in curation/export feature, correctly optional. |
| RomM | `archivefs_core::identity_source::romm::*` (capability, import, linkage, mapping) | `romm_source.rs` card on Sources → Libraries; Home's "Connect RomM" card (`home_page.rs:447-458`) | Explicitly "treated as a read-only source: nothing in your RomM library is ever changed." Correctly optional and already labelled `NotConfigured` by default. |
| ES-DE publication | `archivefs_core::launch::es_de_export`/`es_de_publish` (`plan_es_de_gamelist_publication`, `apply_es_de_gamelist_publication`) | Inside `playing_library_page.rs` ("Publish to ES-DE" flow) | Plan/apply split — publication is previewed before it writes a gamelist. Correctly optional and buried behind the Playing Library mode, not a top-level destination. |
| Reference safety pattern | N/A (read-only reference) | Cheats & Mods local-file install pages (`local_cheat_install*`, `pcsx2_install_plan` — **not modified, not depended on** by this plan) | Cited only as evidence the app already has an "explicit preview, then typed/explicit confirm before anything mutates" convention EmuWiz can point new users to; onboarding does not touch or reimplement this code. |
| GUI-only preference persistence pattern | `archivefs_core::app_dirs::config_path(name)` + a plain sidecar file, e.g. `retroarch_core_directory_override.txt` (`main.rs:33450-33485`: `load_retroarch_core_directory_override_at`/`save_retroarch_core_directory_override_at`, best-effort, GUI-only, deliberately kept out of `Config`) | — | This is the exact pattern a new "onboarding state" flag should follow: a small sidecar file under the app config directory, loaded best-effort at startup, never blocking if absent/unreadable. |

No existing single call answers "is this whole library basically ready to use" — Home currently expresses readiness per-card (source count, Doctor summary, DAT count, RomM state) rather than as one rollup. That gap is called out in §8 as the one non-REUSE piece of new work.

## 4. Proposed onboarding journey

Recommended approach: **hybrid — a thin step-tracker overlay that deep-links into existing pages**, not a set of duplicate custom screens, and not a bare "guided route" with no shared shell either. Reasoning:

- A pure "guided route through existing pages" (just highlighting/scrolling to things on Sources/DAT/Emulator Setup/Verify) has no place to show a step counter, no way to say "2 of 5" persistently, and no natural home for the Welcome/principles step, which is not really "a page" today — it's a banner fragment inside Setup/Diagnostics. A user could easily lose track of being "in onboarding" at all.
- A pure "custom step screens" wizard risks exactly what this task explicitly warns against: screens that duplicate Sources/DAT/Emulator Setup logic and can drift out of sync with the real pages (two truths about the same DAT count, two folder pickers, two Doctor summaries).
- The hybrid keeps one thin overlay (a step tracker: title, 1-line purpose, Skip/Continue, progress dots) that never re-implements page logic — each step's body *is* the real page (or a focused region of it) rendered in place, exactly the way `emulator_setup_page.rs` already renders `LAUNCH_COMPATIBILITY`-derived cards without copying that table. This matches the codebase's own stated architecture pattern (pure view-model + drawing function, `main.rs` supplies already-loaded state) rather than fighting it.

Proposed flow (5 steps, not 6 — RomM/ES-DE/Cheats are deliberately left out of the sequence, see §7 and §8):

1. **Welcome / principles** — new content: a short screen stating what EmuWiz will never do silently (rename/move/delete without an explicit reviewed step; download without asking; auto-configure an emulator; require a DAT provider account). Reuses the *wording* already proven honest in `show_setup_diagnostics`'s welcome box, promoted to its own first step instead of being buried in Setup/Diagnostics.
2. **Add a source** — deep-links to Sources (Libraries tab), reuses `add_source_folder_default`/`sources_page.rs` folder picker and scan flow, required.
3. **Optional DAT / identification setup** — deep-links to DAT Sources, reuses `dat_sources_page.rs`, explicitly optional with a "Skip for now" that is exactly as prominent as "Add a catalogue."
4. **Emulator discovery & readiness** — deep-links to Emulator Setup, reuses `emulator_setup_page.rs`'s candidate list and `LAUNCH_COMPATIBILITY`-derived readiness, informational (nothing to "complete" — the user is shown what was found and what, if anything, needs manual setup, and can always proceed).
5. **Verify & finish** — deep-links to the Verify view already inside DAT Sources (`verify_summary.rs`), or skips straight to a finish screen if no DAT catalogue was added in step 3 (verification without a catalogue has nothing to check, so this step must not claim readiness it can't back up); ends by returning to Home and persisting `onboarding_state = Completed`.

## 5. Step-by-step UX

### Step 1 — Welcome / principles
- **Purpose:** state, in plain language, what a source folder is, that DATs/RomM/ES-DE are optional, and the four "never silently" guarantees.
- **Required/optional:** required to view once; cannot be meaningfully "skipped" (it's a single screen with one Continue button), but closing onboarding entirely from here is allowed (see Skip behaviour below).
- **Backend reused:** none — pure static content plus `missing_config_is_first_run`/`Config::load_default` only to decide whether onboarding should offer to start at all.
- **GUI reused:** none directly; wording lifted from `main.rs::show_setup_diagnostics`'s existing welcome text and `home_page.rs`'s `HomeBanner::FreshInstall` copy, so the three surfaces (Setup/Diagnostics, Home banner, onboarding step 1) never say three different things.
- **New thin UI:** one new step screen (~80-120 lines), styled with existing `widgets`/`theme` components — no new visual language.
- **Skip behaviour:** "Skip setup entirely" closes the overlay and returns to Home exactly as it looks today; persists `onboarding_state = Skipped`.
- **Failure/degraded behaviour:** none possible — no I/O on this step besides the config check already made at app start.
- **State persisted:** `onboarding_state` transitions `NotStarted → InProgress(step=1)`.

### Step 2 — Add a source
- **Purpose:** get at least one source folder configured, because every other step depends on it.
- **Required/optional:** required to proceed past this step with a *populated* library, but the user may still choose "Skip for now" and reach Home with zero sources (Home already handles that state honestly today).
- **Backend reused:** `add_source_folder_default`/`add_source_folder_at` (`archivefs-core/src/lib.rs:3181-3189`); the existing scan pipeline invoked by Sources.
- **GUI reused:** `sources_page.rs` folder-add UI and last-scan summary banner (`show_sources_last_scan_banner`), rendered inline as this step's body.
- **New thin UI:** just the step-tracker chrome around the existing Sources body; no new picker.
- **Skip behaviour:** explicit "Skip for now" button, same visual weight as "Add a folder." Skipping does not block later steps — DAT/Emulator Setup/Verify steps degrade gracefully with 0 sources (they already do, per `home_page.rs`'s `NotConfigured` states).
- **Failure/degraded behaviour:** invalid/duplicate folder errors are exactly the ones `add_source_folder_at` already returns; surfaced the same way Sources shows them today. No new error handling needed.
- **State persisted:** `onboarding_state` step advances only; the actual source folder itself persists in `Config` exactly as it does when added from Sources directly — onboarding adds no parallel storage.

### Step 3 — Optional DAT / identification setup
- **Purpose:** explain, in one paragraph, what a DAT is ("a trusted list of known-good game files"), that it is entirely optional, and that adding one only enables read-only verification — never a required account or provider sign-up.
- **Required/optional:** optional. Framed as "Skip for now" being the expected choice for most users, not a lesser path.
- **Backend reused:** `dat::managed_sources`, `dat::parsers::parse_dat_file`, `dat::classification` exactly as `dat_sources_page.rs` already calls them.
- **GUI reused:** `dat_sources_page.rs`'s "add a catalogue" flow (local file or managed source), rendered inline.
- **New thin UI:** step chrome only.
- **Skip behaviour:** "Skip for now" is a first-class, equally-sized button next to "Add a catalogue" — never a small link, per the UX constraints.
- **Failure/degraded behaviour:** if catalogue validation fails, the page's existing `DiagnosticSeverity`-based error reporting shows as-is; onboarding does not add a second error model.
- **State persisted:** step advance only; DAT registry persists via the existing `save_managed_dat_sources_to`/local registry file, unchanged.

### Step 4 — Emulator discovery & readiness
- **Purpose:** show what EmuWiz found on this machine (installed emulators, RetroArch cores, managed AppImages) and which platforms are ready to launch versus need manual setup — informationally, not as a gate.
- **Required/optional:** informational only; there is nothing to "complete" here besides reading the summary, so this step always has a single "Continue" (no meaningful skip needed, but one is still offered for consistency and because a user with zero interest in a given platform should not feel stuck).
- **Backend reused:** `archivefs_core::launch::{LAUNCH_COMPATIBILITY, LaunchCompatibility}` and whatever per-adapter discovery `emulator_setup_page.rs` already calls into (`DOSBOX_SUPPORTED_PLATFORM_ID`, `SAMEBOY_SUPPORTED_PLATFORM_IDS`, etc.) — read-only discovery, no configuration is written.
- **GUI reused:** `emulator_setup_page.rs`'s candidate list rendering (`EmulatorSetupCandidate`, `CandidateState`), rendered as this step's body, possibly filtered/collapsed to a shorter "top candidates for platforms you actually have games for" view once step 2 has run a scan (nice-to-have, not required for v1).
- **New thin UI:** step chrome; optionally a one-line summary computed from the existing candidate list (count of Ready / NeedsSetup / Blocked) — this is pure aggregation of data `emulator_setup_page.rs` already has, not new backend work.
- **Skip behaviour:** "I'll set up emulators later" always available; never blocks Home.
- **Failure/degraded behaviour:** zero installed emulators found is not an error — it renders the same `NeedsSetup`/`NotChecked` states the page already uses, with copy explaining that games can still be identified/organized without a launchable emulator.
- **State persisted:** step advance only; no emulator configuration is written here — this step is inherently read-only, consistent with "no silent emulator configuration."

### Step 5 — Verify & finish
- **Purpose:** if a DAT catalogue was added in step 3, show the read-only Verify rollup once; either way, close onboarding with a clear "what's next" pointer back to Home and to "Run setup again."
- **Required/optional:** the Verify sub-view only appears if step 3 wasn't skipped (showing an audit rollup with zero catalogues would be a fake-readiness claim, which is explicitly disallowed); otherwise this step is just the finish screen.
- **Backend reused:** `verify_summary::build` (`verify_summary.rs:39`), reading the already-loaded `DatSourcesPageView` — never triggers a new audit by itself.
- **GUI reused:** the Verify tab content inside `dat_sources_page.rs`, rendered inline when applicable.
- **New thin UI:** the finish screen itself (short, static, plus the persisted-state write).
- **Skip behaviour:** N/A — this is the terminal step; "Finish" is the only action, always available.
- **Failure/degraded behaviour:** if no catalogue exists, this step never claims a verification result — it shows the same `NotConfigured` framing Home already uses for "No trusted catalogues added yet."
- **State persisted:** `onboarding_state = Completed`, plus the timestamp, written via the sidecar-file pattern in §6.

### "Run setup again" entry point
Add a single nav item to the existing **Settings** page (`MainView::Settings`, already home to preference-style, non-destructive controls) labelled "Run setup again," which simply resets in-memory onboarding overlay state to `InProgress(step=1)` and opens the same overlay — it does not clear source folders, DAT registrations, or anything else the user already configured; those steps just render their current (already-populated) state instead of an empty one. This avoids adding a second sidebar destination for something that is, correctly, a transient overlay rather than a page.

## 6. Persistence/state model

New GUI-only sidecar file, following the exact `retroarch_core_directory_override.txt` pattern (`main.rs:33450-33485`):

- Path: `archivefs_core::app_dirs::config_path("onboarding_state.txt")`.
- Contents: one of `not_started` / `in_progress:<step>` / `skipped` / `completed`, loaded best-effort at startup (`load_onboarding_state_at`, mirroring `load_retroarch_core_directory_override_at`) — an absent or unreadable file is treated as `not_started`, never as an error.
- Never stored inside `archivefs_core::Config` — onboarding progress is not a scan/mount-relevant setting, exactly the reasoning already documented for keeping the RetroArch core-directory override out of `Config`.
- Written only on: step 1 start (`in_progress:1`), each step advance, explicit skip, and completion. Never written mid-render, never blocks a step if the write fails (best-effort, matching the existing pattern's own comment: "a persistence failure never blocks the in-memory value").
- "Run setup again" does not delete this file — it overwrites it back to `in_progress:1`, so an interrupted or re-run onboarding never looks like a first run to `missing_config_is_first_run`'s unrelated config-presence check (the two predicates are deliberately independent: one is about the config file existing, the other is about whether the *guided tour* has been shown).

## 7. Safety/degraded-state behaviour

Checked against what was actually found, not asserted:

- **Local-first:** every backend call the flow reuses (`add_source_folder_*`, `dat::*`, `launch::*`, `verify_summary::build`) operates on local files/config already read by the existing pages; onboarding introduces no network calls of its own. RomM (network) is deliberately excluded from the guided steps.
- **Read-only by default:** steps 3 (DAT add) and 2 (source add) are the only steps that write anything, and both write exactly what their existing pages already write when used directly (a registry entry, a config source-folder entry) — no new write path. Steps 1, 4, 5 are strictly read-only, confirmed by `emulator_setup_page.rs`'s own module doc ("the final adapter preflight remains the authority for launching") and `verify_summary.rs`'s doc ("does not parse catalogues, walk the library, contact providers").
- **Explicit-over-automatic:** onboarding never auto-adds a source, auto-registers a DAT, or auto-runs Doctor's repair actions; each of those remains a click inside the page it deep-links to, using that page's own existing confirm affordances (e.g. Doctor's `ReviewRepair`/`ConfirmRepair` split, `doctor_page.rs:34-41`, is untouched and still the only way anything Doctor-related mutates).
- **No silent downloads:** no step downloads a DAT, emulator, or artwork; DAT catalogues are user-supplied local files or explicit managed-source opt-ins exactly as `dat_sources_page.rs` already requires.
- **No silent emulator configuration:** step 4 is discovery/read-only; it does not write RetroArch core paths, AppImage configs, or adapter settings. (The one place in the codebase that *does* write emulator-side state, `managed_appimage_bootstrap.rs`'s explicit first-run AppImage initialization, already requires its own separate user confirmation per `emulator_download_page.rs`'s `ConfirmNativeFirstRun`-style flow and is not invoked by onboarding.)
- **No silent renames/moves:** neither Playing Library nor Organise/Quick Rename is part of the guided steps; both remain reachable only from Home, after onboarding, with their own existing typed-confirmation gates (`playing_library_confirmation_phrase`) untouched.
- **No fake readiness claims:** step 5's Verify sub-view is conditionally shown specifically to avoid reporting a verification result when no catalogue exists — mirrors the existing `CardReadiness::NotConfigured` vs `Ready` distinction rather than inventing a "verified" state with a zero denominator.
- **No forced DAT account/provider setup:** step 3 supports local DAT files and existing managed-source options exactly as `dat_sources_page.rs` does today; nothing about onboarding requires signing up for anything, and "Skip for now" is a first-class, not diminished, option.

## 8. Required production changes

| Item | Classification | Reasoning |
|---|---|---|
| Onboarding overlay shell (step tracker, progress dots, Skip/Continue/Finish chrome) | SMALL | New, but thin: a state machine over 5 known steps plus rendering delegated to existing page bodies. No new business logic. |
| `onboarding_state.txt` sidecar load/save | SMALL | Directly copies the proven `retroarch_core_directory_override` pattern; same file, same best-effort semantics, different name. |
| Step 1 (Welcome) content screen | SMALL | New static content, wording lifted from existing, already-reviewed copy in `show_setup_diagnostics` and `home_page.rs`. |
| Steps 2/3/4/5 bodies | REUSE | Render existing page logic (`sources_page.rs`, `dat_sources_page.rs`, `emulator_setup_page.rs`, `verify_summary.rs`) in place; no new backend calls, no duplicated state. |
| "Run setup again" entry in Settings | SMALL | One new button wired to the existing overlay state machine; no new page. |
| A single unified "is this library basically ready" rollup | SMALL, but explicitly **not required for v1** | Today Home expresses readiness per-card, not as one score; the proposed step 4 only needs the existing per-candidate list (`EmulatorSetupCandidate`/`CandidateState`), which already exists, so v1 does not need this rollup. Worth a small follow-up (aggregate count of Ready/NeedsSetup/Blocked candidates) but is separable from onboarding shipping. |
| RomM-in-onboarding | DEFER (explicitly out of scope) | RomM is a network-dependent, credential-bearing integration; forcing it into first-run setup — even as an optional step — risks it reading as expected/normal rather than a deliberate later choice. Home's existing "Connect RomM" card is the right, already-correct entry point. |
| ES-DE-in-onboarding | DEFER (explicitly out of scope) | ES-DE publication lives inside the Playing Library *mode* of Library Organisation, a curation feature a user should only reach after they understand their base library; introducing it during first-run would front-load a second frontend's concept model before the user has one working library view. |
| Cheats & Mods-in-onboarding | DEFER (explicitly out of scope, and untouched per task constraints) | Cheats/mods are a post-library-setup enrichment feature; onboarding should at most footnote the app's "preview then confirm" safety pattern in step 1's principles copy, using Cheats & Mods' local-install flow only as a cited example, never as a step users are walked through. |
| Doctor deep-scan as its own onboarding step | DEFER | Doctor's manual deep scan (`doctor_page.rs`) overlaps heavily with what step 4 (Emulator Setup candidates) already communicates for a first-run audience; adding a second, separate "run Doctor now" step would duplicate messaging Home's "Set up emulators" card already surfaces post-onboarding. Doctor remains reachable from Home/Problems & Repair as today. |

## 9. Test plan

Following the codebase's existing pattern (pure view-model function + `#[test]`s against it, e.g. `home_page/tests.rs`, `mounts_and_history.rs`'s `missing_config_reads_as_first_run_only_when_never_previously_confirmed`):

- Unit tests for the onboarding state machine (pure function: `(OnboardingState, OnboardingEvent) -> OnboardingState`), covering: fresh install starts at step 1; skip at any step transitions to `Skipped` and never auto-advances; completing step 5 transitions to `Completed`; "Run setup again" from `Completed` or `Skipped` re-enters at step 1 without touching `Config`, DAT registry, or source folders.
- Unit tests for `load_onboarding_state_at`/`save_onboarding_state_at`, mirroring the existing `retroarch_core_directory_override` persistence tests (`main.rs:34888-34950`): missing file reads as `NotStarted`; corrupt/unreadable file reads as `NotStarted` rather than erroring; round-trip save/load for each state variant.
- A property/unit test asserting step 5 never renders a Verify sub-view result when `dat_sources_registered_count == 0` (i.e., no fake readiness claim) — same shape as the existing `first_run_hero_uses_onboarding_language_without_inventing_counts` test in `home_page/tests.rs:347`.
- A headless-render test (matching the pattern in `home_page/tests.rs`'s `rendered_text_contains` helpers) asserting: every optional step (3 and 4) renders a "Skip for now" control with a size/enabled-state equal to its primary action; step 1's principles text contains no claim not covered by an existing safety property in §7.
- A navigation-reachability test alongside the existing ones in `navigation.rs`'s test suite confirming "Run setup again" appears under Settings and dispatches to the onboarding overlay without altering `MainView`'s existing routing table shape (`PRIMARY_NAVIGATION_DESTINATIONS`).
- Regression assertion that adding a source folder, a DAT catalogue, or viewing Emulator Setup *through* onboarding produces byte-identical `Config`/DAT-registry state to doing the same action directly on the existing pages (guards against onboarding silently forking a second code path).

## 10. Definition of Done

- A brand-new user with zero config, launching EmuWiz for the first time, is offered the 5-step onboarding overlay (or can decline it entirely) and, having completed or skipped it, lands on the exact same Home page that exists today — onboarding adds no new persistent Home state.
- Every optional step (DAT, and the informational Emulator Setup/Verify steps) has an equally-weighted "Skip for now"/"Continue" pair; no step requires network access, an account, or a provider sign-up to proceed.
- No step writes anything the corresponding existing page would not otherwise write for the same user action — verified by the regression test in §9 comparing onboarding-driven and direct-page-driven state.
- `onboarding_state.txt` persists across restarts using the same best-effort semantics as `retroarch_core_directory_override.txt`, and its absence or corruption never blocks app startup or Home rendering.
- "Run setup again" is reachable from Settings and re-enters the flow without clearing or mutating any already-configured source folder, DAT registration, or preference.
- RomM, ES-DE publication, Cheats & Mods, and Doctor's manual deep-scan/repair flow remain entirely outside the guided steps, each still reachable exactly where it is today (Home cards / Sources / Library Organisation / Problems & Repair), with no new capability, download, or configuration path introduced anywhere in this codebase to build v1.
