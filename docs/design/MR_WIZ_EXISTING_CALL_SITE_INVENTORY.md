# Existing Mr Wiz call sites and migration inventory

Audited at `b9d82821a7d6f1cbb5f9362ea7eaf52bc28738a5`. Companion to [the V1 design](MR_WIZ_GUIDANCE_AND_AI_ASSISTANT_V1.md). This inventory is a static source audit and one inspection of the bundled badge; it is not a live Sunshine test. No production changes are made by this document.

Scope: deterministic GUI-v2 guidance, every discovered runtime mascot-badge consumer, and adjacent page copy that duplicates or speaks as Mr Wiz. Ordinary status labels and unrelated poster images are not exhaustively catalogued. Line numbers below locate the audited snapshot; use the named symbol after code moves.

Dispositions: **KEEP** preserves an existing responsibility; **REWRITE** changes its contract or authored copy; **MOVE** relocates useful content; **REMOVE** deletes a redundant/incorrect guidance use, not its underlying feature; **AI-EXTEND-LATER** means optional user-requested explanation only, after deterministic help is complete. Secondary dispositions explain a separate concern at the same site.

## Runtime and presentation owners

All paths in this table are relative to `crates/archivefs-gui/src/`.

| Current location | Role/evidence | Disposition and reason |
| --- | --- | --- |
| `gui_v2/guidance.rs:1`, module contract | Local, evidence-only projection; no network or modal interaction | **KEEP**. This remains the architectural boundary. |
| `gui_v2/guidance.rs:11`, `GuidanceCategory` | Six categories plus colour/label mapping | **KEEP**, refine labels: Success is not always “Ready”; EmptyState should not always display “Nothing here yet”. |
| `gui_v2/guidance.rs:42`, `MascotState` | Eleven semantic/workflow names; Repair intentionally unused | **REWRITE** to six meaningful semantic states and a future AI progress overlay. No runtime art is presently selected from this enum. |
| `gui_v2/guidance.rs:87`, `GuidanceEvidence` | Optional evidence bag | **REWRITE** into scoped typed projections with freshness; retain unknown/not-loaded distinctions. |
| `gui_v2/guidance.rs:112`, context key; `:137`, `select` | Debug-string identity and rotation on context change | **REMOVE** rotation/debug key; replace with pure priority selection and semantic IDs. Current one-tip branches mask the history-dependence risk. |
| `gui_v2/guidance.rs:147`, `applicable_tips`; `:415`, `tip` | 35 authored keys in one match tree | **REWRITE** into a grouped typed catalogue, with action/explanation/claims and tests. Existing keys are mapped below. |
| `gui_v2/guidance.rs:429`, `show` | One text frame; category label and message; ignores mascot field | **REWRITE** as bounded Quick/Explain/Technical Details with explicit action intent, no action execution during paint. |
| `gui_v2/pages.rs:449–451`, guidance preparation/gate | Entire strip disabled when hints are off or viewport height <720 | **REWRITE**. Optional teaching can minimise; operational blocker text must survive smaller windows and hints-off. |
| `gui_v2/pages.rs:537–538`, only `guidance::show` call | After all route content, inside outer scrolling body | **MOVE** before relevant controls/list; remove unconditional trailing call after owner slots migrate. |
| `gui_v2/pages.rs:553`, `guidance_context` | Central route/evidence adapter | **REWRITE** as thin dispatch to page-owned snapshots. Do not duplicate domain inference. |
| `gui_v2/mod.rs:276,352`, `App.guidance` lifecycle | App-owned session state, default initialisation | **KEEP** coordination; replace rotation with explicit local exposure state. No second history store. |
| `gui_v2/pages.rs:2894`, beginner-hints settings; `backend.rs`, `Preferences` | Explicit user choice, persisted with GUI-v2 preferences | **KEEP**, later migrate to Quick/More explanation/Minimal plus optional-tips toggle. |
| `gui_v2/visual_pages.rs:66`, `home_hero` badge at `:74` | Library totals and setup actions alongside neutral branding | **KEEP** bounded badge; **REMOVE** a second generic Home tip when this card already provides the next step. |
| `gui_v2/pages.rs:803`, Home empty-state badge | “Let's find your games”, Add my games → Sources | **KEEP** action/loaded guard; **REWRITE** via source-aware catalogue to avoid duplicate `home-empty`. |
| `gui_v2/pages.rs:939`, Games empty-state badge | Distinguishes empty library and zero filter results | **KEEP** distinction and action. Reuse as the sole guidance surface for this state. |
| `gui_v2/pages.rs:1153`, Museum empty-state badge | Same catalogue, Open Sources action | **KEEP** truthful scope; a mascot is optional, not required to display the message. |
| `gui_v2/pages.rs:1424`, wide Duplicates empty state | “Wizzy compares file evidence…”, Find duplicates | **REWRITE** name/copy into Mr Wiz catalogue; keep comparison/preview boundary. |
| `gui_v2/pages.rs:1495`, `duplicates_hero` badge/caption at `:1512,1525` | “Mr Wiz checks the copy in the mirror before anything moves” | **REWRITE** to “Review the copy to quarantine and the copy to keep”; state actual evidence before metaphor. Do not imply the portrait is doing a check. |
| `gui_v2/pages.rs:1573`, `duplicate_narrow_empty_state` | Same Duplicates advice authored separately; badge at `:1577` | **REWRITE** using the same record as wide layout. Keep narrow layout, remove duplicated copy ownership. |
| `gui_v2/pages.rs:1646`, Problems hero | Generic badge framed like a diagnostic display | **MOVE** contextual guidance to summary/action; **KEEP** only small neutral branding if useful. No celebratory face as failure evidence. |
| `gui_v2/pages.rs:1862`, History empty-state badge | “No repair history yet” despite multiple history families | **REWRITE** to scope-aware history empty state. **REMOVE** duplicate `history-undo` when no applicable receipt exists. |
| `gui_v2/pages.rs:2856`, Activity empty-state badge | No jobs, Browse my games | **KEEP** empty state; **REMOVE** duplicate idle mascot strip. Running, queued and retained results remain native statuses. |
| `gui_v2/onboarding.rs:64`, `show`/`welcome`; badge at `:171` | Environment-based first run and no-emulator-checks empty state | **KEEP** real environment distinctions and canonical actions. **REWRITE** overlapping static setup guidance as a relevant first-run explanation. |
| `gui_v2/imagery.rs:221`, `mascot`; `:330`, `decode_mascot` | Bounded badge decoding/caching off UI thread | **KEEP** reusable loader/fallback. Expression selection would be separate, after approved assets exist. |
| `gui_v2/imagery.rs:345,356,378`, `EmptyArt::Mascot`/`empty_state` | Shared badge-based empty-state renderer with optional action | **KEEP** component pattern; **REWRITE** its future inputs to share catalogue content and responsive slots, not hard-wire an emotion to every empty state. |
| `ui/components.rs:13`, badge constant; `:156,170`, `workshop_light_header` | Small decorative badge/header, explicitly no diagnostic meaning | **KEEP** decorative role; do not silently bind it to warning/success semantics. |
| `gui_v2/mods.rs:86`, workshop header | Branded introduction and general safety badges | **REWRITE** general promise only where actual operation supports it; preserve existing selected-game/cheat empty-state owner. **REMOVE** duplicate trailing CheatsMods tip. |
| `administration_pages.rs:287`, Views header | Legacy/shared header calls badge component | **KEEP** decorative; **AI-EXTEND-LATER** only on existing selected-plan evidence if requested. No new legacy integration in initial phases. |
| `administration_pages.rs:972`, Mounts header | Legacy/shared header calls badge component | **KEEP** decorative. Do not reinterpret mount readiness through guidance. |
| `administration_pages.rs:1656`, History header | Legacy/shared header calls badge component | **KEEP** decorative; **AI-EXTEND-LATER** for a user-selected receipt summary with redaction. |
| `administration_pages.rs:3774`, Health header | Badge plus explicit caller-provided health status | **KEEP** native status ownership; optional explanation must consume that status. |
| `gui_v2/guidance.rs:449`, unit tests; `gui_v2/tests.rs:1604,4470+` | State/string tests and tall-window integration tests | **REWRITE**/extend to truth, priority, route, first-viewport geometry, actual hints-off labels and no paint mutation. Keep neutral Problems-state assertions. |

No other `Mr Wiz`/`Wizzy` speech sites, direct `Imagery::mascot` consumers or `workshop_light_header` call sites were found in the Rust GUI source search at this commit. Hero artwork containing a decorative character is not an authored runtime guidance script. The older [Retro Workshop design](EMUWIZ_RETRO_WORKSHOP.md) proposes six expression assets; these are a design proposal, not evidence that those runtime assets are present.

## Existing script catalogue: all 35 keys

Keys are in `gui_v2/guidance.rs`; the page/context adapter is in `pages.rs::guidance_context`.

| Existing key(s) | Production reachability / weakness | Disposition and target |
| --- | --- | --- |
| `home-empty` | Reached from empty `library.games` even without checking loaded state; duplicates Home empty card | **REWRITE** → first-run/source/loaded-empty distinctions (01,03,34). |
| `home-browse` | Generic repeat on nonempty Home | **REMOVE** as permanent banner; optional first-use 02 only. |
| `source-unavailable-history`, `source-unavailable`, `source-last-scan` | Source fields not populated by current adapter | **REWRITE** wiring/copy → source card 04; retain genuine last-scan evidence in Details. |
| `source-review` | Only ordinary Sources branch reached through the adapter | **MOVE** into canonical source configuration help, conditional on no source/selection. |
| `launch-identity-blocked` | Selected launch task supplies only `game.identified`; can contradict richer readiness | **REWRITE** from actual readiness/identity evidence (08,09,19), never from that bool alone. |
| `launch-verified` | Identity success is not complete launch readiness | **REWRITE** from current readiness (20) or scope explicitly to identity. |
| `launch-checks` | Default for global Launch, which renders Games | **REMOVE** from global game listing; show only for a real selected-game readiness check. |
| `problem-blocker` | Blocker field never supplied by current adapter | **REWRITE** selected finding integration; preserve reason, add recovery action. |
| `problem-checking`, `problem-none`, `problem-review` | Actual Problems counts connected; otherwise generic pluralisation and broad reassurance | **KEEP** state distinctions; **MOVE** above list; **REWRITE** scoped copy/action (11 and appropriate checking/clear variants). |
| `organisation-preview` | Always generic; also used by `MameWorkflow` | **REWRITE** actual selected plan effect; route MAME to set-specific scripts (12–14,41), Playing Library to 28. |
| `cheats-mods-review` | Section-only generic advice; selected Mods task lacks guidance mapping | **REWRITE** selected-game/task triggers (22–24), merge with native empty/preflight owner. |
| `museum-browse` | Duplicates page purpose and loaded/empty content | **REMOVE** recurring mascot strip; keep native page explanation. |
| `tape-structure` | Format/block fields not supplied; always appends TZX/TAP comparison | **REWRITE** connected inspector evidence; compare formats only when relevant. **AI-EXTEND-LATER** for user-requested explanation of provided structure. |
| `tape-review` | Static read-only explanation | **MOVE** to optional inspector Why disclosure; keep explicit file effects near controls. |
| `archive-review` | “bounded member evidence” is internal vocabulary | **REWRITE** to “View files inside this archive without extracting them”; optional Details for limits. |
| `dat-provenance` | DAT name not supplied by adapter; default Success may overstate identity | **REWRITE** current matching provenance (07–10); **AI-EXTEND-LATER** for conflicting provided results. |
| `dat-review` | Generic versioning statement without action | **REWRITE** from current DAT inventory/staleness/capability; use canonical Add/Manage DAT actions. |
| `firmware-review` | Generic copyright notice, no missing-firmware recovery | **REWRITE** required/missing/unknown states and action (18); keep any necessary policy text separate from routine mascot help. |
| `emulator-readiness` | Generic disclaimer ignores actual installations | **REWRITE** real discovery/selection status (17,19). |
| `setup-fix-first` | Tells user to fix “items” without selecting a concrete prerequisite | **REWRITE** current EnvironmentSnapshot's relevant issue; no problem script when checks are not loaded. |
| `check-platform` | Useful instruction duplicates platform chooser | **MOVE** to missing-platform control context; optional Explain defines verification scope. |
| `activity-idle`, `activity-busy` | Consumes running count only; queued-only state receives idle wording | **REWRITE** counts from authoritative queued/running state (42); plain native status preferred. |
| `history-undo` | Useful instruction repeated without a selected receipt | **MOVE** to selected receipt (30–31); **AI-EXTEND-LATER** explanation of this receipt only. |
| `saves-kinds` | Definitions useful, blanket “restore always…” must remain owner-backed | **MOVE** definitions to Why; **REWRITE** current restore preview/safety conditions. |
| `converter-preview` | “Originals … unless you choose otherwise” can suggest a nonexistent replacement option | **REWRITE** current plan's actual source/output behaviour and limits (25–26,35). |
| `artwork-game` | Static selected-game assertion even on global Artwork; task route lacks mapping | **REWRITE** canonical selection/asset/cache state (15–16,40); retain Sources handoff. |
| `romm-readonly` | General principle useful but no current status/action | **MOVE** source truth to view description; **REWRITE** failure/empty/cache context (32). |
| `advanced-inspect` | Blanket “these tools … without changing them” is too broad for a whole tool family | **REMOVE** universal promise; state effects on each actual operation. |
| `settings-hints` | Preferences discoverable directly; recursive hint about hints | **REMOVE** recurring mascot strip; keep native setting explanation. |
| `games-browse` | Repeats purpose and selection controls on ordinary visits | **REMOVE** permanent banner; conditional first-use/empty/selected evidence only. |

## Coverage and evidence gaps to resolve during implementation

- Sources availability/scan time, tape blocks/format, DAT provenance and explicit problem blocker have authored branches but no current central evidence wiring. `operation_succeeded` has neither production wiring nor a selection branch.
- All 23 `GuidancePage` variants are mapped from at least one route; missing cases are contextual variants and feature-family hubs, not a missing top-level enum arm. Family-hub absence is deliberate and should remain when no real situation needs help.
- `Route::Task` for Launch/Tape/Advanced receives the generic selected-game identity field, even where unused. Artwork/Mods/Problems contextual tasks do not all receive equivalent guidance mapping. The selected-game context must come from the same canonical route as the actual page.
- The existing Launch and Artwork/metadata summary models offer richer evidence than guidance consumes. Reuse those projections; do not make guidance another resolver.
- `ResolvedMetadata.conflicts` exposes metadata candidates rather than every competing artwork asset. Cached artwork is not automatically stale. The proposed artwork-alternative and stale-cache scripts require explicit evidence plumbing and must remain absent where that evidence is unavailable.
- Current art-direction names do not establish a matching visual expression. `GuidanceTip.mascot` is selected and tested, but the strip renderer ignores it. Ordinary badge/hero rendering is independent.
- The smiling badge and 720-pixel gate explain likely mismatch/visibility risks, but the user's specific live expression and exact scrolled position were not replayed during this design audit.
- No model/client/provider work is proposed at existing call sites now. Entries marked AI-EXTEND-LATER first require deterministic evidence, action and privacy contracts from the V1 design.
