# Mr Wiz guidance and AI assistant — V1 design

Status: design for review; no implementation authorised by this document.

> **Phase 1 backend implemented.** The typed model, the deterministic selector, the script catalogue and the repeat model now exist in `crates/archivefs-gui/src/gui_v2/guidance/`; see [MR_WIZ_GUIDANCE_PHASE1_CURRENT_STATE.md](MR_WIZ_GUIDANCE_PHASE1_CURRENT_STATE.md) for what was built, where it deliberately differs from this design, and what is deferred. GUI placement and all AI work remain unimplemented.

Audited base: `b9d82821a7d6f1cbb5f9362ea7eaf52bc28738a5`, authoritative main at the start of this task. Worktree: `/home/davedap/emuwiz-mr-wiz-guidance-design`; branch: `docs/mr-wiz-guidance-ai-design`. Production code, dependencies and provider configuration are unchanged. This design selects no AI provider, model, SDK or deployment service.

## 1. Purpose and current audit

Mr Wiz helps someone understand a particular situation and take the next sensible step. He is an experienced companion to the collection, not another source of truth. The existing identity, readiness, provider, planning, verification and history systems remain authoritative. Guidance explains their results; it does not recompute them.

The complete offline product must answer: what happened, what matters now, what can I do, and what will that action change? Optional AI may later explain these answers conversationally. Disabling AI must remove no deterministic help, diagnostic evidence, navigation action or safety check.

### What exists today

The source audit and disposition of every discovered guidance/badge call site are in [the call-site inventory](MR_WIZ_EXISTING_CALL_SITE_INVENTORY.md). Locations refer to the audited commit, not an assumption about future main.

- `gui_v2/guidance.rs` contains six categories (Tip, Explain, WhyBlocked, Success, Warning, EmptyState), 23 page contexts, 11 mascot states and 35 message keys. `GuidanceTip` contains only key, category, mascot and one message.
- `GuidanceEvidence` has optional facts, but the central `pages.rs::guidance_context` populates only library presence, selected-game `identified`, Problems counts and running-job count. Source availability/scan time, explicit blocker, tape evidence and DAT name are not supplied by this production adapter. `operation_succeeded` is unused by the selector too. Some tests exercise richer contexts than the GUI supplies.
- Selection uses a debug-formatted context key and a mutable rotation counter. Each current branch produces one tip, so selection is effectively stable today. Adding two eligible tips would make the answer depend on navigation history. That is not the desired future deterministic rule.
- `guidance::show` is a coloured text frame labelled “Mr Wiz · …”. It does not render `selected.mascot`, an action, explanation, evidence disclosure or dismissal. There is no actual expression binding in this renderer.
- `pages.rs::show` appends guidance after page rendering inside the outer scroll area. Nested and virtualised page bodies can put it beyond the useful viewport. Guidance is entirely suppressed below 720 pixels high and when `beginner_hints_enabled` is false. This is unsuitable for essential blockers on a couch display.
- Home, empty states, Duplicates, Problems and workshop headers independently render the same bundled smiling badge. The inspected badge is an older white-haired, bespectacled character with teal/rainbow clothing, a wand and prominent glow. It is welcoming branding, not an expression set. No “angry” enum is present; the reported unsuitable expression cannot be attributed to a runtime angry state in this checkout. Reusing an upbeat badge beside a failure can still create a mismatch.
- The Problems selector already distinguishes checking, clear, findings and a blocker, and has tests against alarm-like ordinary guidance. Keep that distinction. Its clear statement must stay scoped to the current checked findings, not imply every file was verified.
- Family hubs deliberately return no guidance. Several selected-game task routes also fall through to no context (for example Artwork and Mods tasks). `MameWorkflow` receives generic Organisation guidance. These are coverage gaps, not permission to add a default tip everywhere.
- Existing page purpose text, empty-state actions, readiness summaries and the guidance footer often explain the same thing. “Wizzy” in Duplicates conflicts with the Mr Wiz name. “Copy in the mirror”, “bounded member evidence” and “identity … verified” often describe implementation instead of a useful decision.
- `launch_readiness_summary.rs` already has explicit statuses, actions, freshness, firmware and identity summaries, including `LaunchableWithWarning`. A guidance rule based only on `game.identified == false` can contradict the actual launch decision. Reuse the readiness projection.
- Existing tests verify a few evidence branches, state labels and basic strings. A length assertion (`message.len() > 30`) does not establish usefulness. A hints-off assertion comparing exactly with “Mr Wiz” does not catch the rendered “Mr Wiz · …” label. Tests using 2400/3200-pixel-high windows do not establish first-viewport visibility.

Keep the no-network/evidence-projection contract, stable message keys, optional evidence rather than fabricated values, clear Problems states, native controls, accessible text fallbacks, bounded asynchronous badge loading and canonical routing. Replace rotation, footer placement, disconnected state/copy, generic always-on tips and unqualified safety claims. Do not replace the existing domain engines.

## 2. Character and voice guide

Mr Wiz is a patient older computer and gaming enthusiast who has seen enough failed floppy disks to value a careful check. He knows the subject, respects the collection, and assumes the user has a sensible reason for asking. His name is always **Mr Wiz**. “Wizzy” is retired from product copy.

Quick copy normally uses 15–40 words in one or two sentences. Lead with the actual situation, then the next step. Use ordinary words before an acronym; explain the acronym on demand. Say “EmuWiz found…” for evidence and “I can explain…” for assistance. Never claim personal memories, consciousness or actions the software has not performed. The character may evoke an era without inventing a human biography.

Use calm specificity: “The folder could not be read” rather than “Something went wrong”; “I can’t confirm the revision yet” rather than “Identity unresolved”. Admit the limit and name a route forward. Do not blame the user. Do not promise recovery, source preservation, verification, compatibility or undo without evidence for this operation.

Success is restrained: “That’s sorted” is enough, and only when the claimed result is proven. Ordinary missing information is not an emergency. Use no exclamation marks by default. No baby talk, wizard spells, corporate welcome speeches, “obviously”, “just”, “easy”, “you forgot”, or a joke about losing someone’s collection.

Workshop flavour is optional, rare, and placed after the instruction. A term such as “bench” must never name the action instead of “Select a game”, “Review the preview” or “Choose a folder”. Nostalgia belongs mainly in optional explanations; errors and blockers need no joke. Do not inject flavour randomly to create variety.

| Avoid | Prefer |
| --- | --- |
| Put the game on the bench. | Select a game first. Then I can show you its cheats and patches. |
| Identity unresolved. | EmuWiz cannot safely identify this game yet. Review its evidence before renaming it. |
| Magical success! | The new file passed verification. Review the result in History. |
| That’s definitely the right game. | Its checksum matches this DAT entry. The evidence is available below. |
| Don’t worry, nothing can go wrong. | This preview does not change files. Review the destination before applying it. |
| Your collection is broken. | Three required files are missing from this set. Review it in MAME. |

## 3. Guidance architecture

Use one small, deterministic presentation pipeline:

```text
Existing page/domain snapshots + current route/selection + explicit help preference
    → page-owned evidence adapter (no I/O, no new inference engine)
    → pure eligibility and priority selection
    → authored script + validated parameters + permitted action reference
    → local exposure policy
    → Quick / Explain / Technical Details in the page's guidance slot
```

Proposed focused modules under `gui_v2/guidance/` are `model`, `select`, `catalogue`, `presentation`, `exposure`, `actions`, and page-specific adapter modules as justified by size. Existing `App` coordinates events; `pages.rs` provides slot wiring only. This is a proposed split of the current module, not a new application framework.

Domain owners supply immutable views: `EnvironmentSnapshot`, `GameReadinessSummary`, `ProblemSummary`, current MAME evidence, `MediaIndex.resolved`, `Picture`, conversion plans, current operation results and receipts. Adapters should live beside their page or in a focused guidance adapter, never in `lib.rs`. Avoid a giant bag of loosely related booleans.

Some desired facts need presentation plumbing before they can be used. In the audited model, `ResolvedMetadata.conflicts` contains metadata candidates, not a complete list of competing artwork; `ArtworkCandidate.cached` proves caching, not staleness or source unavailability. Script 40 must remain ineligible until existing artwork candidates can be exposed with the winner; script 16 needs explicit cache/source evidence rather than an age guess. No new resolver or cache policy is authorised. Likewise, GUI-v2's native Converter section does not currently provide a contextual Converter task route: opening it must disclose that the source still needs selecting.

The projection records scope (collection, selected game, source, operation), freshness, an evidence revision, and whether the relevant check has completed. Unknown, loading, absent, stale and failed are distinct. “Not loaded” cannot become “nothing found”. Receipts can describe their historical operation; they cannot establish current health.

The selector is pure: equal semantic snapshot and explicit preference produce equal selected scripts and actions, independent of frame count, wall clock and navigation order. The exposure policy separately decides whether optional content is expanded, compact, or suppressed using explicit session state and an injected clock. Equal full presentation inputs also produce equal presentation. No per-frame mutation, fetching, persistence or activity jobs in selection/painting.

Use an event reducer to record acknowledgement, explanation requests, user preference changes and visibility after layout. A view must not count as “seen” merely because its widget was constructed below the fold. At most one Mr Wiz page message and one relevant inline message for the currently focused issue; do not repeat the same script/fact in both.

## 4. Script schema and storage proposal

Prefer typed Rust catalogue entries for V1, grouped by domain in `guidance/catalogue/`. This matches the existing Rust enums, requires no dependency or runtime file loading, and makes route/action variants compile-checked. Do not scatter authored guidance into render functions. The document below is the initial editorial catalogue; future implementation converts it into typed entries.

Do not choose YAML/JSON/TOML plus a predicate language. The apparent flexibility would move safety and routing checks into strings. When localisation is funded, keep typed triggers, parameters and actions in Rust and move message text behind stable translation keys. Validate each locale's placeholder names/types and fall back to complete English messages. Do not build concatenated sentences or use “file(s)”; support singular/plural forms explicitly.

Conceptual schema (design notation, not a proposed executable DSL):

| Field | Contract |
| --- | --- |
| `script_id`, `copy_revision`, `translation_key` | Stable, unique ID; editorial revision does not create a new event identity. |
| `category`, `topics` | Six existing message categories retained; the 29 subject categories below are topics, not extra severity enums. |
| `contexts`, `scope` | Typed page/task and selected entity/operation scope. |
| `trigger`, `required_evidence`, `exclude_when` | Rust predicate ID and typed required facts. Missing required facts make the script ineligible. No expressions evaluated from files. |
| `priority`, `exclusive_group`, `surface` | Fixed band, stable tie-break and slot. No numeric urgency supplied by providers or AI. |
| `mascot_state` | Small semantic set from section 9, independent of workflow names. |
| `parameters` | Named, typed values: counts, display title, revision, provider label, diagnostic reason. No paths or secrets in Quick by default. |
| `short_message`, `experienced_message` | Quick copy and optional shorter equivalent. Facts, negation and safety are invariant. |
| `explanation`, `beginner_expansion` | Optional why/meaning/consequence paragraphs; no duplicate quick text. |
| `next_action_label`, `action_ref`, `action_requirements` | One primary action from a typed allowlist, resolved against current capability. A secondary link is permitted only when it answers a different explicit question. |
| `why_blocked` | A typed reason reference plus explanatory copy for blocked actions. Never a replacement safety decision. |
| `success_followup` | A conditional next step tied to the result and its verification/receipt state. |
| `advanced_note`, `evidence_refs` | Human preface plus existing structured diagnostics, hashes, provenance and exact errors; expanded on request. |
| `repeat_policy`, `dismissal_scope` | Explicit exposure policy with semantic event key; blockers are not permanently dismissible. |
| `claim_requirements` | Evidence required to say unchanged, verified, undoable, lossless, complete, or available. |

Example record: `firmware.missing` requires a current readiness result with a missing required firmware item for the selected emulator/game; category WhyBlocked, priority 90, state Helpful, scope game+emulator+requirement, primary action Review BIOS folder, explanation and expert copy from script 18. If firmware is merely unknown, this record is ineligible: use a checking/recheck explanation instead. If the canonical page lacks a folder chooser, the action says “Review BIOS / Firmware” and opens that page; it does not advertise an absent control.

Catalogue linting must reject unused IDs, duplicate IDs, missing placeholder bindings, unsupported action references and a success claim with no claim requirement. Rendering escapes provider/game text and limits length. Diagnostic values are data, never scripts or instructions.

## 5. Placement and interaction rules

Put help at the decision it explains. The page owns a named guidance slot; the shared renderer paints that slot. Remove the unconditional call after the route body. Preserve the current app shell.

| Surface | Placement and behaviour |
| --- | --- |
| Page | After title/selected-game strip, before filters or results. Quick text, one action and “Why?”; optional portrait 40–56 logical pixels. |
| Large list | A compact slot outside the virtualised body and above its viewport. Use the existing page region for a sticky blocker/next step; do not let the list consume its height first. |
| Card | Inside the selected/problem card immediately before its controls. Use an icon or text only for repeated rows; never a portrait per row. |
| Inline blocker | Adjacent to the disabled/reviewed action, with cause and recovery route. The action's normal validation remains visible when hints are off. |
| Dialog | Near the operation summary, above confirmation. Explain effects without weakening exact confirmation text. No competing conversation overlay. |
| Success | Compact result card next to the completed operation; one optional follow-up. A supplementary toast may acknowledge it, but never hold the only verification/undo evidence. |
| Help panel | User-opened explanation panel anchored to the current scope; contains levels 2/3 and, later, optional AI. Back returns to the same context. |

At 1280×720 and 1024×600, show the relevant Quick sentence and primary action in the first viewport. At 480×360, collapse art and optional explanations, wrap text and preserve the action and blocker; allow vertical scrolling within expanded help. Do not disable all guidance based on screen height. Target a two-line quick summary, but allow translation/text scaling to increase height; avoid a rigid clip rectangle.

“Why?” expands Explain (normally 40–100 words); “Technical details” separately exposes exact diagnostics. Expansion must not shove the affected action off-screen without keeping it reachable in the help panel. Keep keyboard focus stable, label links/actions meaningfully, support text scaling, reduced motion and screen-reader status announcements once per semantic transition. Expression/colour are supplementary, never the only warning.

Do not use Mr Wiz for field validation, every row status, byte counts, ordinary progress updates or every successful click. A plain label is often sufficient. If the existing readiness/empty-state card already answers the question and offers the action, use it as the guidance surface rather than adding a second banner.

## 6. Triggers, priority and action resolution

Priority is scope-aware: a problem with the selected action beats an unrelated global suggestion. Blocking the entire current workflow beats a routine local hint. Do not interrupt Play with an optional missing-cover warning.

| Band | Eligibility |
| --- | --- |
| 100 | Current apply/launch refused by an authoritative safety check, changed source/destination or read-only restriction. |
| 90 | Concrete missing prerequisite for the action the user is attempting: firmware, compatible emulator, required disc/member. |
| 80 | Current operation failed/incomplete, conflicting identity affecting this action, stale evidence needing recheck. |
| 70 | Current scoped warning with an available decision; historical warnings do not qualify. |
| 60 | Completion of the user's current operation, with precise verification status. |
| 50 | Completed check found a relevant empty state or setup gap. |
| 40 | User-requested explanation; displayed in its own help surface without displacing a blocker. |
| 10 | First-use orientation for an eligible, relevant task. Otherwise select nothing. |

Select by eligibility, scope specificity, fixed priority and finally lexical script ID. The same exclusive group has at most one winner per scope (e.g. artwork loading/failed/missing; identity unknown/candidate/conflicting/verified). Safety blockers cannot be rotated away. If several blockers exist, show the authoritative primary blocker and “N other requirements” opening existing details, rather than alternating advice. A warning uses Warning category/state; WhyBlocked need not use an alarm expression for routine setup.

Action resolution validates the selected game/source/operation and evidence revision again on click. Stale actions become “Check again” through the owning page, not execution against old paths. Labels must match actual capability. Proposed action aliases below refer to existing routes/controllers; they are not new operations:

| Guidance action | Existing destination/boundary |
| --- | --- |
| Open Sources / Configure sources | `Route::Section(Section::Sources)`; family overview may use `SourcesProviders`. |
| Review evidence | `Route::Game(id)` and its existing evidence disclosure. No automatic identity choice. |
| Manage DATs / Check Games | `Section::Dat` / `Section::Check`; existing platform selection and confirmation still apply. |
| Review MAME | `Route::MameWorkflow`; current route has no game argument. Preserve originating Game Details in router Back and state that the set must be selected there until a canonical contextual route exists. Do not invent a hidden selection field. |
| Choose/review emulator; Review BIOS | `Section::Emulators` / `Section::Firmware`; use existing native controller actions where exposed. |
| Review launch / failed launch | `Route::Task { section: Launch, game }`, using the existing readiness view and current diagnostics. |
| Review problem / repair | Existing `ProblemDestination` mapping, or scoped Problems task; MAME findings retain MAME routing. “Preview repair” appears only if that owner exposes a supported preview. |
| Review artwork / cheats | Canonical selected-game `Route::Task` for Artwork/Mods; use the existing tab/context adapter, never a parallel selected-game state. |
| Review conversion | `Section::Converter` for the native page. The audited `Task { section: Converter, ... }` falls through to legacy handoff; do not advertise it as a native selected-game deep link. Preserve the originating route for Back and explain source selection until the canonical page supports context. |
| Playing Library | `Section::Build` and its existing plan controller. No implicit apply. |
| Review duplicates | `Section::Duplicates`. Comparison/scanning is a separate explicit action. |
| View History / Undo review | `Section::History` (or HistoryUndo hub); select an existing receipt only where the view supports it. Undo is never executed by opening help. |
| View Activity / Open Doctor / RomM | `Section::Activity` / `Section::Setup` / `Section::Romm`. |

Selected-game context is the canonical route identity. Navigation stores the originating route via the existing router. On global setup return, re-resolve that game; if it was removed, explain that rather than selecting a similarly named game. A target section or action that cannot preserve context must disclose its global scope. Opening an existing page may invoke that page's normal read-only loading; it must not invent a new guidance fetch or an “opening help” background job.

## 7. Repeat suppression and fatigue

Keep behaviour local and minimal. Proposed preferences: Quick (default), More explanation, Minimal, plus a separate optional-tips toggle. Do not infer expertise from mistakes, time spent, number of games or success rate.

- Blockers remain beside their affected action until resolved. Users can collapse the mascot/explanation, not remove the underlying reason. A materially changed blocker updates immediately.
- Optional first-use tips appear once per session per script+semantic scope after confirmed viewport exposure. A repeated visit uses a compact “Why?” link. No rotation for novelty.
- An acknowledged success is suppressed for that operation ID for the rest of the session; a different operation can produce a new result. The receipt/result remains in the existing history system. Do not dismiss because a timer fired while it was off-screen.
- Non-blocking warnings can be collapsed for the same evidence revision. A changed severity/reason invalidates suppression. Ordinary counts or progress ticks must not generate a new semantic event.
- Optional tips have a proposed 30-minute minimum cooldown and at most one unsolicited tip per task per session; the stricter rule wins. Inject the clock in tests. Explicit “Help me with this” always opens the current explanation.
- V1 exposure data is session memory only. Persist only explicit help preference using existing GUI-v2 preferences. Later, an optional bounded local first-use acknowledgement set may be considered; do not store navigation trails or game names for suppression.
- No telemetry, remote profiling or cross-device behavioural tracking. “Reset guidance” clears local acknowledgement/preference state through the existing settings owner.

Selection and suppression are separate so tests can show that the same state chooses the same script even when its optional presentation is currently suppressed.

## 8. Beginner and experienced presentation

Quick is for everyone. More explanation expands the first relevant Level 2 paragraph and spells out terms. Minimal uses the experienced copy with an obvious “Why?” and Technical Details shortcut. Existing `beginner_hints_enabled=false` should migrate to Minimal with optional tips off; true migrates to Quick. Ask no expertise quiz.

All modes keep exact blockers, source/output behaviour, permissions, verification distinctions and existing confirmation controls. A short variant may omit a definition, but cannot omit “not verified”, “original changed”, “preview only”, “cannot undo”, or another material limitation. AI verbosity, if later offered, is independent of these safety rules.

## 9. Mascot-state matrix

Use six semantic states and one future activity overlay. Map old workflow poses to them; avoid a separate emotional state for each feature family.

| State | Permitted use | Current enum migration |
| --- | --- | --- |
| Neutral | Quiet presence, no urgent claim; usually no portrait needed | Welcome, Explorer, Archive → Neutral unless explaining |
| Helpful | Tip, Explain, EmptyState, routine WhyBlocked with a concrete next step | Explain, Organise, Tinker, Launch, Repair → Helpful according to evidence |
| Thinking | Evidence is loading or uncertainty needs review; label which, never imply background work merely because a model is uncertain | Thinking retained |
| Warning | Warning category: meaningful preservation risk, conflicting evidence affecting a decision, unsafe destination | Warning retained; no angry face |
| Concerned | A current failure that needs recovery; calm and attentive, not exaggerated sadness | New semantic mapping; use text/icon fallback until suitable approved art exists |
| Success | Confirmed result; scope says what succeeded and whether it was verified | Success retained |
| AI-active overlay (future) | Explicit AI request in progress; label local/external and allow Cancel | Small progress cue over Neutral/Helpful; not a promise of correctness |

Ordinary missing firmware uses Helpful, not Warning. BAD_DUMP/NO_DUMP reference limitations use Helpful, not a “damaged collection” alarm. Unknown identity uses Thinking without a spinner unless checking is actually active. Every Warning script uses Warning state; WhyBlocked may use Helpful or Concerned depending on its reason. Never use angry expressions.

Reuse the badge for small neutral branding where suitable. Do not pretend it is seven expressions. For warning/failure states, text plus a native status icon is preferable until separately approved, restrained art exists. No new art is produced here. Future art must match the older enthusiast character, keep glow subordinate to legibility, avoid baked-in status text and default to still images. Respect reduced motion.

## 10. Deterministic catalogue coverage

The following are real state conditions, not page-entry triggers. They require adapters to existing evidence; no additional backend engine is implied. If an owner cannot expose a fact, the corresponding script stays unavailable. “Offline mode” is explicitly not a global network detector: it means the requested optional source is known unavailable or the user explicitly selected local-only AI/help.

| Topic | Trigger and evidence owner | Examples |
| --- | --- | --- |
| 1 First run | `EnvironmentSnapshot.is_fresh`, welcome not dismissed, no usable library | 01 |
| 2 Home | Loaded nonempty library, no current higher-priority issue, optional first-use tip eligible | 02 |
| 3 Sources | No configured source or selected source availability failure from native source summary | 03–04 |
| 4 Scanning | Actual scan job running; terminal scan has folder errors | 05–06 |
| 5 Identity / DAT matching | DAT inventory empty; current identity candidate rather than verified report | 07, 09 |
| 6 Problems | Current `ProblemSummary` has actionable findings | 11 |
| 7 MAME | Required members missing, parent dependency, dump-status limitations from existing set evidence | 12–14, 41 |
| 8 Artwork / metadata | Resolver completed with no cover; usable stale cache; multiple candidates with a current winner | 15–16, 40 |
| 9 Emulator setup | Existing discovery reports multiple installations without explicit binding | 17 |
| 10 BIOS / firmware | Current required firmware evidence says Missing, not Unknown/NotRequired | 18 |
| 11 Launch readiness | Current readiness primary blocker is emulator selection; or current plan is Ready | 19–20 |
| 12 Failed launch | Actual launch outcome reports failure for this request | 21 |
| 13 Cheats | No selected game; or completed supported lookup has zero entries | 22–23 |
| 14 Mods / patches | Existing patch preflight rejects the base checksum/revision | 24 |
| 15 Conversion | Existing plan is previewable; or preservation classification is Unknown/sensitive | 25–26 |
| 16 Multi-disc | Existing media/set evidence identifies a required missing disc | 27 |
| 17 Playing Library / 1G1R | Current source-preserving link plan exists with selected counts/preferences | 28 |
| 18 Duplicates | Existing comparison reports exact file evidence, no automatic action | 29 |
| 19 History / Undo | Receipt and current undo eligibility available/refused | 30–31 |
| 20 RomM | Browser snapshot unavailable with actual reason | 32 |
| 21 Offline mode | Optional provider operation unavailable, local browsing still available | 33 |
| 22 Empty library | Library load succeeded, zero games, at least one configured source | 34 |
| 23 Unknown game | Current report has no sufficiently supported identity | 08 |
| 24 Conflicting evidence | Existing identity resolver reports incompatible candidates, not ordinary artwork alternatives | 10 |
| 25 Unsupported format | Capability result explicitly refuses the requested operation/format | 35 |
| 26 Read-only / safety block | Current planner refuses required write access or source change | 36, 39 |
| 27 Success states | Verification and operation result support exact success claim | 20, 37 |
| 28 Warnings | Explicit preservation uncertainty or incomplete check; no inferred corruption | 06, 26 |
| 29 Recovery from failure | Failed launch, changed source, or apply completed but verification pending/failed | 21, 38–39 |
| Supplement: Activity | Authoritative registry has queued and/or running jobs | 42 |

## 11. Authored example scripts

These 42 entries are editorial records for deterministic implementation, not deployed strings. In every entry: **Quick** is Level 1 and the beginner/default variant; **Explain** is Level 2; **Details** is Level 3 evidence, shown only when available; **Minimal** is the experienced variant. Next is one primary action. A conditional alternate replaces that action; it does not add a row of equal buttons. Required facts in Guard apply to every variant. Priority, exposure and semantic scope inherit sections 6–7 unless specified. Placeholders are typed and singular/plural inflection is required.

### 01 — `first_run.choose_sources`

Guard: fresh environment, welcome eligible, no higher-priority setup conflict. EmptyState / Helpful / page.

- Quick: “Welcome. Choose where EmuWiz should look for your games; you’ll review the folders before scanning them.”
- Next: **Choose game folders** → Sources.
- Explain: “Start with folders you already use. EmuWiz can list what it finds without renaming or repairing your games. Emulator setup and optional artwork sources can come afterwards.”
- Details: active configuration root, loaded source summary, environment snapshot freshness.
- Minimal: “Choose game folders, then review and scan them.”

### 02 — `home.browse_ready`

Guard: loaded nonempty library, first-use Home tip eligible, no active blocking workflow. Tip / Neutral / page; optional.

- Quick: “Your game list is ready to browse. Select a title to see what EmuWiz knows about it and whether it is ready to play.”
- Next: **Browse games** → Games.
- Explain: “Being listed is not the same as being verified or ready to launch. Game Details keeps those checks separate, along with artwork and associated documents.”
- Details: current catalogue counts and load revision, not a library dump.
- Minimal: “Browse games to review identity and launch readiness.”

### 03 — `sources.none_configured`

Guard: source configuration successfully loaded and empty; exclude configuration-read failure. EmptyState / Helpful / page.

- Quick: “No game folders are configured yet. Add a folder so EmuWiz knows where to look.”
- Next: **Add game folder** → existing Sources setup.
- Explain: “Choose a folder containing your own collection. Adding its location is separate from scanning it, and does not move the files.”
- Details: source-configuration status and active configuration location.
- Minimal: “No game folders configured. Add a source.”

### 04 — `sources.unavailable`

Guard: selected configured source currently unavailable, catalogue retained. Warning / Warning / source card.

- Quick: “This game folder is unavailable. Check that its drive is connected, then review the folder location.”
- Next: **Review game folder** → Sources.
- Explain: “The catalogue entry has been kept. An unavailable folder does not prove the games were deleted; the drive may be disconnected or its mount location may have changed.”
- Details: exact path, availability reason, last successful scan if known.
- Minimal: “Source unavailable; catalogue retained. Review its location.”

### 05 — `scan.running`

Guard: scan job phase Running; not Queued. Explain / Thinking / compact page slot.

- Quick: “EmuWiz is reading your game folders and updating the list. You can keep browsing while the scan runs.”
- Next: **View scan progress** → Activity.
- Explain: “This scan does not rename or repair your original games. Progress reflects what the scanner has reported; there is no completion estimate unless one is available.”
- Details: job ID, current item, reported counts and cancellation capability.
- Minimal: “Scan running. View Activity for progress.”

### 06 — `scan.partial_failure`

Guard: terminal scan reports unreadable folders and retained previous entries. Warning / Warning / result card.

- Quick: “The scan finished, but {folder_count} folders could not be read. Review those folders before relying on the new list as complete.”
- Next: **Review unavailable folders** → Sources.
- Explain: “EmuWiz kept the previous entries for those folders. Reconnect the drive or correct the location, then run the existing scan again when you are ready.”
- Details: per-folder errors, scan receipt/time and retained-entry evidence.
- Minimal: “Scan incomplete for {folder_count} folders; previous entries retained.”

### 07 — `dat.none_available`

Guard: loaded DAT inventory contains no usable source for the requested DAT check. WhyBlocked / Helpful / verification controls.

- Quick: “There is no usable DAT for this check yet. Add or review a DAT source so EmuWiz can compare these games with a reference catalogue.”
- Next: **Manage DATs** → DAT Management.
- Explain: “A DAT lists known releases and file checksums. Without a suitable one, this DAT-based check cannot confirm a match. Other evidence and ordinary browsing remain available.”
- Details: requested platform, DAT inventory status and parser/import errors if present.
- Minimal: “No usable DAT for this check. Manage DAT sources.”

### 08 — `identity.unknown`

Guard: current game identity unknown after available evidence was reviewed, and the requested rename/repair requires stronger identity according to its owner. WhyBlocked / Thinking / identity card. With no blocked operation, use Explain with the same facts; never infer a launch blocker from identity alone.

- Quick: “EmuWiz cannot safely identify this game yet. Review its evidence before choosing a rename or repair.”
- Next: **Review game evidence** → selected Game Details; offer Manage DATs there when appropriate.
- Explain: “A filename can suggest a title, but it is not enough to justify a destructive change. A matching checksum or other supported identification evidence gives EmuWiz a firmer basis.”
- Details: report status, inspected format, available checksums, candidate evidence and DAT provenance.
- Minimal: “Identity not confirmed. Review evidence before rename or repair.”

### 09 — `identity.candidate_only`

Guard: identity owner reports candidate/possible match, without verified match. Explain / Thinking / identity card.

- Quick: “This may be {title}, but the match has not been verified. Review the evidence before using that name for a change.”
- Next: **Review possible match** → Game Details.
- Explain: “A possible match is a lead, perhaps from a name or provider record. A verified match has passed the checks required by the relevant EmuWiz workflow. The two are not interchangeable.”
- Details: candidate source, comparison method, missing verification requirement and report revision.
- Minimal: “Possible match: {title}. Not verified; review before changing files.”

### 10 — `identity.conflicting_evidence`

Guard: existing resolver reports incompatible identity evidence relevant to the requested action. Warning / Warning / identity card.

- Quick: “The available evidence points to different game releases. Review the conflicting results before choosing an identity.”
- Next: **Compare identity evidence** → existing Game Details evidence.
- Explain: “Different DAT versions, revisions or provider records can disagree. EmuWiz will show their sources and any stronger local checks; neither a tidy filename nor a confident explanation settles the conflict.”
- Details: all relevant candidates, checksums, DAT versions, provenance and resolver outcome.
- Minimal: “Identity evidence conflicts. Compare the candidates and local checks.”

### 11 — `problems.review_findings`

Guard: current completed ProblemSummary has positive actionable count. Explain / Helpful / above findings list.

- Quick: “There are {finding_count} findings to review. Open one to see the cause and any supported next step.”
- Next: **Review first finding** → existing priority-sorted finding, if available; otherwise Review problems.
- Explain: “A finding may mean a missing file, uncertain identity or a setup requirement. It does not mean every item is damaged or repairable. EmuWiz will explain the available action before a change.”
- Details: severity, category, evidence source and scope of the selected finding.
- Minimal: “{finding_count} findings. Review the highest-priority item.”

### 12 — `mame.missing_members`

Guard: current MAME set report counts required missing files. WhyBlocked / Helpful / set summary.

- Quick: “This MAME set is missing {missing_count} required files. Review the complete set in MAME before changing individual files.”
- Next: **Review in MAME** → canonical MAME workflow.
- Explain: “MAME games are often sets of related files, sometimes shared with another set. Renaming one ROM cannot supply a missing member and may break the set's relationships. MAME review keeps those relationships in view.”
- Details: target set, parent/clone relationships, missing member checksums, DAT and supported repair/reconstruction evidence.
- Minimal: “MAME: {missing_count} required files missing. Review the complete set.”

### 13 — `mame.parent_dependency`

Guard: clone dependency on a parent is proven and the required dependency is missing. WhyBlocked / Helpful / set card.

- Quick: “This clone needs files from parent set {parent}. Review that dependency in MAME.”
- Next: **Review parent dependency** → MAME.
- Explain: “Some related arcade games share files. A clone can be correctly named and still need its parent set. EmuWiz checks the available member evidence before suggesting a reconstruction.”
- Details: parent/clone IDs, ownership/member evidence, availability and reconstruction blockers.
- Minimal: “Missing parent dependency: {parent}. Review in MAME.”

### 14 — `mame.bad_dump_reference`

Guard: reference evidence explicitly contains BAD_DUMP; do not infer local corruption. Explain / Helpful / affected member details.

- Quick: “The reference marks this as a known imperfect dump. That alone does not mean your copy has become damaged.”
- Next: **Review reference evidence** → MAME details.
- Explain: “BAD_DUMP describes the reference data's known limitation. It is different from your file failing a checksum comparison. This flag alone is not a reason to offer an ordinary repair.”
- Details: exact flag, member, DAT version, local comparison result and any independently supported action.
- Minimal: “BAD_DUMP in the reference. Review separately from local hash failures.”

### 15 — `artwork.cover_missing`

Guard: current selected-game resolver/delivery check completed with no usable cover; not loading or decode failure. EmptyState / Helpful / cover slot.

- Quick: “No usable cover has been matched to this game yet. Review the available artwork sources.”
- Next: **Review artwork sources** → Sources & Providers. If a supported explicit refresh is available and configuration is ready, replace with Refresh artwork. If identity is actually uncertain and prevents matching, replace with Review game evidence.
- Explain: “Missing cover art does not mean the game is unidentified or unplayable. The configured sources may simply have no usable image for this title.”
- Details: resolver candidates, provider availability, matching provenance and delivery status.
- Minimal: “No usable cover found. Review artwork sources.”

### 16 — `artwork.stale_usable_cache`

Guard: cache evidence explicitly proves a retained usable asset, staleness and source unavailability. Explain / Neutral / artwork card. For a stale copy whose source is available, use the authored alternate “Using an older cached cover. You can review the artwork details and refresh it when ready”; do not claim unavailability.

- Quick: “Using cached cover art. The source is currently unavailable, but this saved copy can still be displayed.”
- Next: **View artwork details** → current Artwork context; refresh remains an explicit existing control when supported.
- Explain: “A cached image is a local retained copy. It may be older than the source's current image; EmuWiz has not confirmed a newer one. There is no need to treat the usable copy as a failure.”
- Details: cache timestamp/key, provider, last error and current winner evidence.
- Minimal: “Using a stale cached cover; source unavailable.”

### 17 — `emulator.multiple_installations`

Guard: discovery reports exactly two Dolphin installations and no explicit applicable choice; plural template handles other names/counts. WhyBlocked / Helpful / emulator choice.

- Quick: “Two Dolphin installations were found. Review them and choose which one EmuWiz should use.”
- Next: **Review installations** → Emulator Setup.
- Explain: “Each installation can have its own version and configuration. Choosing one makes the launch target clear; finding two does not mean either installation is broken.”
- Details: paths, versions, discovery sources and existing binding/preference, without changing it.
- Minimal: “Two Dolphin installations; no applicable choice. Review installations.”

### 18 — `firmware.missing`

Guard: current selected emulator requires firmware and evidence says Missing. WhyBlocked / Helpful / readiness blocker.

- Quick: “This emulator needs system firmware, and EmuWiz has not found it in the checked locations. Review the BIOS folder.”
- Next: **Review BIOS folder** → BIOS / Firmware; label Choose BIOS folder only if the canonical owner exposes that action.
- Explain: “BIOS or firmware is the small system software that the original console uses to start and operate. Some emulators need a copy. EmuWiz only needs to read the files to detect them; it does not need to modify them for this check.”
- Details: emulator, required firmware kind, checked paths, accepted checksum evidence and exact detection result.
- Minimal: “Required firmware not found in checked locations. Review BIOS / Firmware.”

### 19 — `launch.no_compatible_emulator`

Guard: current readiness owner identifies no selected compatible emulator as the blocker. WhyBlocked / Helpful / Play controls.

- Quick: “I can see the game, but I can’t safely launch it yet because no compatible emulator has been selected.”
- Next: **Choose emulator** → existing emulator selection/setup.
- Explain: “The emulator needs to support this platform and the game's current representation. Review the available installations; EmuWiz will check readiness again after the selection.”
- Details: current launch plan, candidate compatibility, selection reason and any additional blockers.
- Minimal: “Launch blocked: no compatible emulator selected. Choose one.”

### 20 — `launch.ready`

Guard: current non-stale launch plan Ready, no unacknowledged warnings. Success / Success / readiness card; optional mascot.

- Quick: “The current launch checks are clear. You can use Play when you’re ready.”
- Next: **Review launch** → canonical launch view; keep its existing Play button as the only launch action.
- Explain: “These checks cover the evidence EmuWiz has for this launch. They do not guarantee that every part of the game will run correctly in the emulator.”
- Details: selected emulator, media target, firmware result, evidence revision and plan warnings (none for this script).
- Minimal: “Ready for the current launch plan.”

### 21 — `launch.failed`

Guard: actual launch request failed; reason is supplied by that outcome. WhyBlocked / Concerned / launch result.

- Quick: “The game did not start: {plain_failure_reason}. Review the launch details before trying again.”
- Next: **Review launch details** → failed launch context; choose a specific recovery route instead if the owner supplies one.
- Explain: “This is the result of the launch attempt, not proof that the game files are damaged. The details show the emulator and failure reported. If the cause is unknown, say so rather than guessing.”
- Details: exact error, exit status when available, executable, safe arguments, relevant bounded log and request ID.
- Minimal: “Launch failed: {plain_failure_reason}. Review details.”

### 22 — `cheats.no_game_selected`

Guard: Cheats context, canonical selected game absent. EmptyState / Helpful / selected-game strip.

- Quick: “Select a game first. Then I can show you any cheats EmuWiz knows about for it.”
- Next: **Choose a game** → Games; return using existing navigation.
- Explain: “Cheats can depend on the platform and game revision. Selecting the game gives EmuWiz the context needed to show relevant entries.”
- Details: current context has no selected game; do not create a diagnostic error.
- Minimal: “Select a game to view known cheats.”

### 23 — `cheats.none_known`

Guard: completed applicable cheat lookup, zero entries, no provider/parse failure. EmptyState / Neutral / Cheats content.

- Quick: “No cheats are currently known for this game. You can keep browsing its details.”
- Next: **Back to Game Details** → selected game.
- Explain: “This is an empty result, not an error. It does not mean the game is broken or that no cheat exists anywhere.”
- Details: checked sources and applicable revision evidence where available.
- Minimal: “No known cheats for this game.”

### 24 — `patch.wrong_base`

Guard: patch preflight rejects expected base and identifies a revision/region mismatch; operation has not applied and evidence confirms original untouched. WhyBlocked / Helpful / patch preview. If only a hash mismatch is known, substitute “This patch expects a different starting file. EmuWiz has not modified your original file” without guessing the revision or cause.

- Quick: “This patch expects a different game revision or region. EmuWiz has not modified your original file.”
- Next: **Review expected base** → existing patch preflight/evidence view.
- Explain: “A patch describes changes to one particular starting file. Applying it to a different release can produce unusable output. Compare the patch's required checksum with the selected game; a similar filename is not enough.”
- Details: expected/actual base hash, required revision/region only if known, patch format and exact rejection.
- Minimal: “Base ROM does not match the patch. Original unchanged.”

### 25 — `conversion.preview_available`

Guard: existing planner supports this source/target preview and explicitly reports separate output/source preservation. Explain / Helpful / conversion controls.

- Quick: “This conversion can create a separate {target_format} file. Preview the destination and checks before converting; your original stays unchanged.”
- Next: **Preview conversion** → existing preview action, available only under its normal prerequisites.
- Explain: “The preview shows what EmuWiz can actually perform, the output location and any warnings. A recognised format alone does not mean conversion is supported.”
- Details: source/target representation, backend capability, operation classification, collision rule and verification plan.
- Minimal: “Preview {source_format} → {target_format}; separate output, original retained.”

### 26 — `conversion.preservation_unknown`

Guard: current planner reports unknown preservation or an explicit layout warning. Warning / Warning / preview warning.

- Quick: “EmuWiz cannot prove that this conversion preserves all disc layout information. Review the warning before deciding what to do.”
- Next: **Review conversion warning** → current preview; never introduce Force or Apply.
- Explain: “A disc may contain several tracks, audio or timing information beyond a simple data image. Only mention those features when the source evidence confirms them. Unknown preservation is not the same as lossless.”
- Details: existing classification, track/audio evidence if present, unsupported properties and authoritative blocked/allowed state.
- Minimal: “Preservation not proven. Review the planner's warning or blocker.”

### 27 — `multidisc.required_disc_missing`

Guard: existing multi-disc report identifies an expected missing member; not a guessed numbered filename. WhyBlocked / Helpful / media set.

- Quick: “This game is missing a required disc from its known set. Review the disc list before launching or rebuilding it.”
- Next: **Review disc set** → existing media evidence in Game Details/owning workflow.
- Explain: “Some games use several discs as one release. EmuWiz needs evidence that those discs belong together; similar names alone do not establish a complete set.”
- Details: known disc identities/order, missing member, manifest/DAT provenance and readiness consequence.
- Minimal: “Required disc missing from the known set. Review media evidence.”

### 28 — `playing_library.plan_ready`

Guard: current supported link plan, explicit source-preserving behaviour, actual selected/output counts available. Explain / Helpful / plan summary.

- Quick: “The plan selects {set_count} sets and creates {link_count} links for a play-focused library. Review the choices; your original collection stays in place.”
- Next: **Review Playing Library plan** → existing plan view.
- Explain: “A Playing Library can make a collection easier to browse without reorganising the preserved originals. Where 1G1R and regional preferences are supported, the plan shows which release was selected and why.”
- Details: selected entries, existing preference rules, skipped candidates, output root, collisions and source-preservation evidence.
- Minimal: “{set_count} sets, {link_count} links planned. Originals remain in place.”

### 29 — `duplicates.exact_matches`

Guard: comparison completed and existing evidence classifies exact duplicates; exclude merely similar titles. Explain / Helpful / duplicate group.

- Quick: “These files match the duplicate check. Review which copy to keep before any supported quarantine action.”
- Next: **Review duplicate group** → Duplicates evidence/preview.
- Explain: “A duplicate check is separate from deleting or moving anything. Different regions or revisions must not be treated as duplicates just because their titles look alike.”
- Details: comparison method, hashes, source paths, protected roles and existing quarantine/undo eligibility.
- Minimal: “Exact duplicate evidence found. Review the group before quarantine.”

### 30 — `history.undo_available`

Guard: selected receipt has an available current undo review path. Explain / Helpful / receipt card.

- Quick: “This change has an undo option. Review what will be restored before confirming it.”
- Next: **Review undo** → existing history/rollback preview, not execution.
- Explain: “Undo depends on the files and recovery data still matching the recorded operation. EmuWiz checks those requirements again; an old receipt alone is not a guarantee.”
- Details: transaction ID, expected paths/hashes, retained recovery data and latest eligibility.
- Minimal: “Undo available for review; current checks still apply.”

### 31 — `history.undo_refused`

Guard: current undo eligibility refused with a concrete reason. WhyBlocked / Concerned / receipt card.

- Quick: “EmuWiz cannot safely undo this change because {plain_undo_reason}. Review the recovery details.”
- Next: **Review recovery details** → existing receipt diagnostics.
- Explain: “Undo must not overwrite a file that has changed since the operation. The recorded change is still available to inspect, even when automatic recovery is no longer safe.”
- Details: precise refusal, expected/current evidence, backup availability and supported recovery actions only.
- Minimal: “Undo blocked: {plain_undo_reason}. Review recovery evidence.”

### 32 — `romm.snapshot_unavailable`

Guard: RomM browser snapshot unavailable with known failure/configuration reason; exclude empty successful library. Explain / Helpful / RomM status.

- Quick: “RomM library information is unavailable here: {plain_source_reason}. Review the integration setup.”
- Next: **Review RomM setup** → canonical source/provider setup.
- Explain: “This view uses the information supplied by the current adapter. It does not imply native browsing or downloading beyond the controls already offered. Locally verified game evidence remains separate.”
- Details: snapshot status, cache availability, adapter capability and redacted raw error.
- Minimal: “RomM information unavailable: {plain_source_reason}.”

### 33 — `offline.optional_source_unavailable`

Guard: a requested optional remote source is unavailable; local catalogue available. Explain / Neutral / affected source only.

- Quick: “This online source is unavailable, but you can still browse the local game list. Review its status when you need data from it.”
- Next: **Review source status** → Sources & Providers.
- Explain: “Artwork or metadata from that source may be missing or cached. This does not establish that the whole computer is offline, and it does not by itself stop local emulation.”
- Details: affected provider, last observed failure, usable cache evidence; no new connectivity probe.
- Minimal: “Optional source unavailable. Local browsing remains available.”

### 34 — `library.loaded_empty`

Guard: library load completed with zero games; configured sources exist, scan not active. EmptyState / Helpful / library content.

- Quick: “The game list is empty. Review your configured folders and scan them when you’re ready.”
- Next: **Review game folders** → Sources.
- Explain: “An empty catalogue is different from a folder being unavailable or a scan still running. The Sources page shows where EmuWiz is configured to look and the latest scan information.”
- Details: catalogue load status, source count and last scan result, without guessing file absence.
- Minimal: “No games in the loaded catalogue. Review sources and scan status.”

### 35 — `format.operation_unsupported`

Guard: existing capability explicitly says requested operation unsupported for this format. WhyBlocked / Helpful / operation controls.

- Quick: “EmuWiz recognises {format}, but it cannot perform {operation} on this representation. Review the supported options.”
- Next: **Review supported options** → same workflow's capability view; omit an action if no such view exists and explain the limitation in place.
- Explain: “Being able to inspect a file does not mean EmuWiz can convert, repair or launch it. No unsupported operation is made available by this guidance.”
- Details: capability result, representation, backend/refusal reason and supported alternatives from the owner only.
- Minimal: “{operation} unsupported for {format}. See capability details.”

### 36 — `safety.destination_read_only`

Guard: planner reports read-only destination for an operation requiring writes. WhyBlocked / Helpful / destination control.

- Quick: “The chosen destination is read-only, so this operation cannot write its output there. Choose another destination.”
- Next: **Choose output folder** → existing destination chooser, if supported; otherwise Review destination.
- Explain: “EmuWiz will not try to override the restriction. Selecting a writable output folder is separate from changing permissions on your original collection.”
- Details: destination path, detected access restriction, operation effect and exact error.
- Minimal: “Output destination is read-only. Choose another folder.”

### 37 — `repair.completed_verified`

Guard: this repair completed, output verification passed, and receipt was recorded. Success / Success / operation result.

- Quick: “That’s sorted. The corrected file passed verification, and the change is recorded in History.”
- Next: **View repair in History** → existing receipt/history route.
- Explain: “This confirms the checks performed for this repair at that time. It does not mean every game in the collection was checked, or that a later file change would go unnoticed.”
- Details: operation ID, verification method/result/time, source/output behaviour and actual undo eligibility.
- Minimal: “Repair completed and verified. Receipt recorded in History.”

### 38 — `repair.completed_verification_pending`

Guard: operation completed but output verification is pending/incomplete, not failed. Explain / Helpful / operation result; higher priority than generic success.

- Quick: “The repair step finished, but the result has not been fully verified. Run the supported verification check before relying on it.”
- Next: **Verify result** → existing verification route/action. If verification is unavailable, say “Verification is unavailable for this result” and use Review result instead.
- Explain: “A completed operation is not the same as verified output. The verification view shows which checks remain; it must not turn a successful process exit into a claim about file health.”
- Details: completed stage, outstanding checks, current receipt and verification capability. A failed check uses a separate failure variant with its exact reason.
- Minimal: “Repair step complete; verification incomplete. Verify the result.”

### 39 — `recovery.source_changed_since_preview`

Guard: existing apply preflight refused a stale plan because source evidence changed; no writes occurred according to outcome. WhyBlocked / Concerned / apply controls.

- Quick: “The source changed after the preview, so EmuWiz stopped before applying it. Create a new preview from the current files.”
- Next: **Preview again** → existing preview action with normal prerequisites.
- Explain: “A preview describes particular files at a particular time. Reusing it after those files change could apply the wrong operation. Review the new plan before confirming anything.”
- Details: rejected plan ID, expected/current fingerprints and explicit no-write outcome.
- Minimal: “Apply refused: source changed since preview. Preview again.”

### 40 — `artwork.alternatives_available`

Guard: existing resolver selects a current winner and reports alternatives for the same asset. Explain / Neutral / artwork details.

- Quick: “Using {provider} for this image. {alternative_count} alternative sources are available to inspect.”
- Next: **View artwork sources** → current Artwork provenance/details.
- Explain: “Several sources can offer an image without anything being wrong. EmuWiz's existing precedence rules determine the current choice. Viewing alternatives here does not change those rules.”
- Details: winner, candidate list, precedence decision, provenance and delivery/cache evidence.
- Minimal: “Using {provider}; {alternative_count} alternatives. View provenance.”

### 41 — `mame.no_dump_reference`

Guard: reference explicitly marks a required member NO_DUMP. Explain / Helpful / member details.

- Quick: “The reference has no known dump for this member. EmuWiz cannot treat that as an ordinary missing file it can repair.”
- Next: **Review reference limitation** → MAME evidence.
- Explain: “NO_DUMP records a gap in the reference material. It is not a checksum failure in a file you already have, and it does not establish that a usable replacement exists.”
- Details: member, exact flag, DAT provenance and separate local availability evidence.
- Minimal: “NO_DUMP reference limitation. No ordinary repair is implied.”

### 42 — `activity.queued_work`

Guard: authoritative registry has queued jobs; supplied running count is separate. Explain / Helpful / Activity summary, normally plain status.

- Quick: “{queued_count} tasks are waiting to start; {running_count} are running. Open Activity to see their current states.”
- Next: **View Activity** → Activity; suppress this duplicate action when already there.
- Explain: “A waiting task has not started work. A completed, failed, cancelled or superseded task is retained as a result, but is not counted as active. No time estimate is inferred from the queue.”
- Details: actual job phases, current item, elapsed running time and cancellation support. Opening a page is not itself a background task.
- Minimal: “{running_count} running; {queued_count} waiting.”

## 12. Optional AI extension architecture

Add AI only as a user-requested explanation path beside complete deterministic help. Suggested entry points: “Explain more simply”, “Explain this evidence”, “What can I do next?” and “Ask Mr Wiz”. Label the resulting content **AI explanation** and the configured execution location. Do not blend generated claims into the verified status card.

```text
User asks for help on a specific scope
  → deterministic guidance and canonical evidence remain visible
  → context builder selects and minimises current facts
  → local privacy policy validates context and selected endpoint
  → optional HelpModel adapter streams an advisory response
  → response validator checks fact/action references
  → clearly labelled explanation with user-clicked canonical navigation
```

Keep conversation state scoped to the current request/game/operation. A selection change marks the answer “About the previous selection”; stale responses must not appear under the new game. Model errors, timeouts, cancellations and unavailable models return to the offline explanation. No AI required at startup, no automatic prompts on navigation, no automatic retry storms, and no rewording safety warnings into less specific prose.

AI may explain terminology, compare supplied DAT evidence, summarise supplied launch diagnostics and propose an existing review route. It may not change identity confidence, infer a missing checksum, certify safety or convert a candidate into a verified match. If facts are insufficient, it asks for a permitted existing check and says what is unknown.

## 13. AI safety and action boundaries

The model has no filesystem, shell, network, credential or mutation tool. In the first optional AI implementation, it returns explanation text plus proposed action IDs from the context's allowlist. The UI resolves those IDs locally; the model never supplies executable paths, commands, arbitrary URLs or route arguments.

Navigation/review suggestions include Open Sources, Review Evidence, Open Doctor, Review MAME, Choose Emulator and Review Repair. A future “Preview Repair” suggestion is usable only through the existing explicit preview handler after a user click, with selection, capabilities, snapshot revision and normal preconditions checked again. Any preview that creates staging data retains the owning workflow's disclosures. Apply, delete, rename, move, patch, download, change configuration, choose an uncertain identity and execute undo are never model-dispatched actions.

If the user asks an AI to repair something, explain and open the canonical review flow. The user's chat sentence is not the operation's existing confirmation. Plans and confirmations remain in their normal UI, with exact source/destination/effect and verification/undo evidence. Do not add a second natural-language confirmation mechanism.

Treat game titles, DAT text, artwork descriptions, logs and provider errors as untrusted data. Embedded instructions must not alter the assistant role or expand the allowlist. Validate action IDs and cited fact IDs; reject unknown IDs and stale scopes. Native UI renders the immutable verified-fact list itself. Generated prose is interpretation even when it cites a fact; validation cannot prove every generated sentence true. Contradictory or unsupported claims should be hidden or flagged, with the deterministic answer retained. Do not claim hallucination prevention is solved by prompting.

## 14. Local-first model/provider abstraction

Propose an app-owned `HelpModel` interface after the deterministic system is complete. Conceptual request fields: versioned context envelope, user question, selected permitted explanation task, response budget, cancellation token and permitted action IDs. Response events: Started, TextChunk, AdvisoryAction, Complete, Failed, Cancelled. Structured references and final validation are required before action chips become interactive.

Three configuration states:

1. **Disabled** (default): no model runtime and no endpoint access. Deterministic help is complete.
2. **Local** (explicitly configured later): adapter to a user-chosen local runtime, with bounded context, timeout and cancellation. A loopback address alone is not proof the runtime never forwards data; disclose the configured execution boundary accurately.
3. **External** (separately enabled later): explicitly named endpoint/account, visible outgoing context, per-request consent by default. Any later “remember this choice” control must state its provider and scope. No silent local-to-external failover.

Keep provider-specific transport, authentication and model options behind this boundary. Use the application's approved configuration/secret facilities where they exist; do not design a new credential store here. Adapter capabilities may include streaming and context limits, never broader action authority. No vendor comparisons, model download feature, AI dependency, client implementation or external probing belongs in the current task.

## 15. Context and privacy design

Build an allowlisted help envelope from existing snapshots for this request only:

| Field | Permitted default content |
| --- | --- |
| Request identity | Ephemeral request ID, context revision, locale, explicit verbosity, requested help task. |
| Page/scope | Canonical page, opaque game/source/operation reference; no navigation history. |
| Game | Selected title/platform/representation only when needed; user can replace title with “selected game”. |
| Facts | Typed value, fact ID, evidence source, checked-at/freshness, verified/candidate/unknown status and scope. |
| Current issue | Authoritative blocker/warning, requested action and current capability. |
| Readiness | Relevant emulator/firmware/media result; omit unrelated installations. |
| Artwork/provider | Relevant winner/candidates/cache status; not the full provider configuration. |
| Diagnostics | Bounded excerpts for the issue, redacted locally; exact logs only by explicit inclusion. |
| Actions | Locally resolved action IDs valid for this snapshot; no executable payloads. |

Never include the entire library, credentials, cookies, tokens, authentication headers, ROM contents, save data, manual contents or arbitrary directory listings by default. Full paths can reveal names, mounts and accounts: substitute aliases such as “game folder” and keep the path mapping local. Redact endpoint query strings and usernames. Hashes and unusual game titles can identify collection contents; include exact hashes only for a request that needs comparison and show them in the external-context review. Keep raw diagnostics available locally under Technical Details without automatically sending them.

Proposed limits: one selected game/operation, at most 20 relevant facts, at most 8 KiB of redacted diagnostic excerpts and a hard adapter input budget. If more evidence is needed, ask the user to select a smaller relevant subset; do not silently send more. Truncation must be marked. No automatic remote retrieval or uploads from a model response.

In the help panel, distinguish **Facts from EmuWiz**, **AI explanation**, and **Unknown / not checked**. Each factual citation resolves to the supplied immutable fact card. Conversation is memory-only by default and cleared when the user ends it. Saving/exporting a conversation would be a separate explicit feature, not a new history store hidden inside guidance. Disclose that external retention depends on the configured service; local UI controls cannot promise deletion from a provider. Do not put sensitive prompt/response bodies into ordinary debug logs.

## 16. Testing and acceptance strategy

No production tests are implemented in this design-only change. Future implementation tests should exercise decisions, safety invariants and rendered geometry, not merely mirror strings or enum lengths.

- Pure selection table tests: every trigger, absent/unknown/stale evidence, same state reached by different navigation orders, stable tie-break and mutual exclusion. Unknown library load must never select empty-library copy.
- Priority tests: blocker over success/tip; selected-game issue over unrelated optional provider warning; first-run conflict over welcome; known BAD_DUMP/NO_DUMP never creates generic repair.
- Route/action validation: every catalogue action resolves to a real route/controller; context survives Game Details → help → setup → Back; missing game clears safely; MAME never routes to generic unsafe repair/rename.
- Safety claims: “verified”, “original unchanged”, “lossless” and “undo available” require their evidence. A successful process and verified output remain different. Historical repair receipts cannot establish current health. Apply remains gated by the owning planner even if a script/action token is forged.
- Variants: Quick and Minimal share evidence bindings, material limitations and action authority. A content reviewer approves variants; structural tests alone cannot prove semantic equivalence.
- Mascot mapping: every Warning script maps to Warning; ordinary setup/help never maps to angry or exaggerated distress; missing asset falls back to text/icon without shifting controls.
- Placement: headless egui frames at 1280×720, 1024×600, 480×360 and increased text scale, including 100,000 virtualised rows. Assert guidance/action rectangles intersect the usable initial viewport and precede list allocation. Exercise nested-scroll cases, not only tall screenshots. Manual Sunshine pass checks legibility/focus without automatic DISPLAY launch.
- Exposure reducer: first visible exposure, repeated visits, off-screen construction, acknowledgement, cooldown with fake clock, new evidence revision, same-operation success replay, changed blocker and explicit “Why?”. Paint alone must not record acknowledgements or start jobs.
- Copy catalogue validation: unique IDs, exact typed placeholders, plural cases, valid references, nonempty actionable instruction or explicit reason no action exists. Golden snapshots of representative authored records provide reviewable copy diffs; do not snapshot all layout pixels as the sole correctness test.
- Integration tests: actual page adapters using real model fixtures for firmware missing, candidate-only match, wrong patch base, stale artwork, queued-only Activity and failed verification. Prove unused fields from the present implementation become connected or are removed.
- Future AI contract tests use a fake adapter: disabled makes zero calls; explicit requests only; cancellation/timeouts; stale streaming response; privacy redaction; malicious DAT/log instructions; unknown action/fact IDs; no external fallback; no mutation from generated text. Every failure retains deterministic help.

Editorial acceptance: a new user can identify the situation and next action from Quick without Level 3; an experienced user can reach the raw evidence without dismissing a tutorial. Test with humans as well as fixtures. Do not optimise for message engagement or time spent chatting.

## 17. Migration plan

1. Freeze the audited keys and record their replacement IDs using the companion inventory. Keep current domain checks and routing. Production main has changed since earlier smoke-test work; rebase the implementation plan on its actual current owner modules before coding.
2. Replace `GuidanceState` rotation with pure selection plus a separate exposure reducer. Introduce typed scope/freshness and an explicit None result. Preserve the existing six categories and theme roles.
3. Replace the after-body renderer with an opt-in slot contract on one representative dense page and one contextual page. Integrate existing readiness/empty-state cards to avoid duplicates. No broad page rewrite.
4. Connect a small set of existing evidence projections: readiness, Problems counts and selected-game identity. Add actual action resolution and three-level copy. Prove both entry routes and return context work.
5. Migrate old script families in measured batches. Remove disconnected evidence fields only after mapping each to a real adapter or documenting its deliberate absence. Replace “Wizzy” and metaphor-led copy when those call sites migrate.
6. Bind semantic mascot state only after suitable existing art/fallback is chosen. Decorative heroes remain separate. Do not delay actionable help for a new expression pack.
7. Migrate the explicit hint preference conservatively, then add local exposure rules. Blocker visibility is independent of optional tips.
8. Remove the old footer path only after coverage/placement tests pass and call-site ownership is clear. Keep a short compatibility adapter during migration if needed; never run both banners for one fact.

This design supersedes the older visual document only on guidance voice, semantic expression and decision placement. Its useful rule—art creates atmosphere, native controls communicate truth—remains. Runtime assets and broad shell redesign are not part of this migration.

## 18. Recommended implementation phases and risks

| Phase | Deliverable and acceptance gate |
| --- | --- |
| 1 — Deterministic core and placement | Typed catalogue/selection/action contracts, three-level renderer, visible slot, no-I/O tests; integrate only Problems and selected-game readiness as proofs. Old behaviour remains for other owners during migration. |
| 2 — Highest-value scripts | Firmware missing, unknown/candidate/conflicting identity, launch blocker/failure, patch wrong base, MAME safety, missing art and repair verification. Real evidence adapters and canonical actions required. |
| 3 — Presentation preferences and fatigue | Quick/More explanation/Minimal, hints-off migration, session exposure reducer, dismiss/acknowledge/Why and accessibility checks. |
| 4 — Catalogue coverage | Remaining topics/42 seed scripts, truthful unavailable variants, editorial review and couch/narrow-window acceptance. Coverage means real state triggers, not a page-entry banner on every page. |
| 5 — Optional AI interface design realised | App-owned advisory interface, fake adapter, request lifecycle, context review/redaction and allowlisted navigation. Disabled remains default; no live model/provider required. |
| 6 — Separately approved implementations | Evaluate and implement optional local/external adapters only after a new explicit approval covering provider/runtime, privacy, dependencies, configuration and maintenance. |

Recommended first implementation: Phase 1, scoped to a small engine/presentation slice and two existing page projections. Its review should show a selected game's real missing-firmware blocker beside Play, a Problems summary above a long list, an actionable expansion, and no newly available mutation. Do not start with a chat box or a mass copy rewrite.

Main risks are incorrect evidence wiring, contradictory duplicate messages, selection lost on global routes, broad safety promises, cramped layouts, and suppression hiding changed blockers. Mitigate them with explicit evidence/freshness, one owner per fact, route revalidation, claim requirements and viewport tests. Static Rust authoring trades some editorial convenience for clear compile-time boundaries; revisit localisation storage only when needed. Optional AI adds uncertainty, privacy and latency risk, so it remains an advisory layer with visible provenance and an immediate deterministic fallback.

The live smoke-test layout and expression problems are supported by the audited composition and reused badge, but this task did not run a Sunshine session. Proposed dimensions, cooldown and copy length are design targets to validate with human testing, not measurements of a new implementation.
