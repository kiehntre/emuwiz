# Mr Wiz guidance — Phase 1 current state

Phase 1 is the **backend-only, deterministic, offline** guidance engine. It adds
no GUI placement, no navigation, no network, no model, no persistence and no
dependency. It is the implementation of the typed model, selector, catalogue and
repeat model described in [the V1 design](MR_WIZ_GUIDANCE_AND_AI_ASSISTANT_V1.md);
that document and [the call-site inventory](MR_WIZ_EXISTING_CALL_SITE_INVENTORY.md)
remain the editorial source. Built on main `bdeb86f1`.

Mr Wiz explains product truth; it never determines it. The engine consumes typed
facts the caller already holds and returns *which message applies and why*. It
queries no database, filesystem, network, emulator or provider.

## What existed, and what was done with it

One engine already existed: `gui_v2/guidance.rs`, with 6 categories, 23 page
contexts, 11 mascot poses and 35 hard-coded messages in a single match tree, one
message per page, selected through a debug-string key and a rotation counter, with
no levels, actions, provenance or repeat policy. `pages.rs` builds a
`GuidanceContext` and calls `guidance::show`; three gui_v2 page tests assert its output.

No second engine was created. The existing file became the thin page-facing shell
(`GuidanceContext`, `GuidancePage`, `GuidanceState`, the egui frame in `show`) over
the new submodules under `gui_v2/guidance/`, which `guidance.rs` declares itself, so
`gui_v2/mod.rs`, `pages.rs` and the GUI crate `lib.rs` are untouched. What each page
shows today is byte-for-byte what it showed before: the 35 messages are catalogue
entries with their original keys and text, selected by the engine.

| Piece | Disposition |
| --- | --- |
| Six message categories, page enum, context/evidence shape, no-network contract | **Reused** unchanged (one new page, `Duplicates`, for script 29) |
| 35 message keys | **Reused** as `Legacy` catalogue entries; `replaced_by` names the designed script that supersedes each |
| Rotation counter and debug-string key | **Removed.** Selection is a pure function. |
| `MascotState` (11 workflow poses) | **Rewritten** to the design's six semantic states, adding `Concerned` |
| `GuidanceEvidence` optional fields | **Kept** and projected into typed facts; new `facts` field for page-owned adapters |
| `GuidanceTip` | **Kept** (key = stable script ID, Quick text) |
| 42 authored scripts, levels, placement and repeat rules, action table | **Implemented** as typed data and pure logic; *placement* is Phase 2 |
| AI extension (sections 12-15) | **Not started.** No extension point was added. |

## The model (`guidance/model.rs`)

* **Categories** (`GuidanceCategory`): Tip, Explain, WhyBlocked, Success, Warning,
  EmptyState. The design's 29 subject areas (plus the Activity supplement) are
  `GuidanceTopic`s, *not* more severities.
* **Levels** (`GuidanceLevel`): `Minimal < Quick < Explain < Technical`, one axis of
  verbosity. `Minimal` is the experienced-user equivalent of Quick; Explain and
  Technical are the design's Level 2 and 3. A level a script does not author
  resolves to Quick and the item reports that (`level` vs `requested_level`);
  text is never invented. Technical text is an *authored sentence* describing what
  the owning page can show, never assembled from runtime values.
* **Mascot** (`MascotState`): Neutral, Helpful, Thinking, Warning, Concerned,
  Success. No angry state; no artwork is selected. Consistency is enforced: Warning
  category uses the Warning state and nothing else does; Success likewise; Concerned
  is only for blockers that need recovery.
* **Facts** (`GuidanceFact` / `FactKind`): typed, with named parameters. Unknown,
  loading and failed are *absence*. A fact that promises a specific it does not carry
  (zero folders, an empty title or reason) is invalid and ignored before selection.
  `semantic_key` identifies a semantic event (a title, reason or operation) and
  ignores ordinary counts.
* **Actions** (`GuidanceAction`): 47 typed identifiers with authored labels. The
  engine only *offers* them. Nothing executes or navigates; a later adapter decides
  whether and how to render each, and must revalidate the selection on click.
  Conditional alternates replace or omit the primary action (for example Refresh
  artwork, Choose BIOS folder, or no action when no capability view exists), and a
  pure-navigation offer to the page the person is already on is dropped.

## Selection (`guidance/select.rs`)

`select(page, facts)` is a pure function: equal inputs give equal output, independent
of the order facts are supplied, navigation history, frame count or the clock.
A script is eligible when its page matches, every required fact kind is present, no
excluded kind is, and every placeholder in every authored level can be filled from
the required facts. Among eligible scripts the order is:

1. **priority band** (100, 90, 80, 70, 60, 50, 40, 10);
2. **scope** (operation, game, source, collection);
3. page-specific over any-page;
4. **category** (WhyBlocked, Warning, EmptyState, Explain, Tip, Success);
5. the **script ID**, so a tie is never decided by table order.

This deliberately differs from the design's literal order (scope specificity first):
with scope first, a narrow game-scoped *tip* could hide a collection-wide *refusal*,
which the requirements forbid. Bands are assigned by situation, so band-first keeps
the design's intent ("a problem with the selected action beats an unrelated global
suggestion") without that failure. The selection returns provenance: the band,
scope, matched fact kinds, every eligible script it outranked, and the other
eligible blockers, so a page can say "N other requirements" instead of alternating.

When nothing applies the answer is an explicit `None` (a hub, or a page with nothing
to say, shows nothing).

## Repeat and suppression (`guidance/exposure.rs`)

Pure session memory with an injected clock; no persistence. Selection and exposure
are separate: the same facts select the same script even while its presentation is
suppressed. Policies per script (`RepeatPolicy`): *Blocker* (collapsible, never
suppressed), *FirstUse* (once per session and scope, then a compact link; one
unsolicited tip per topic and a 30-minute cooldown, the stricter wins),
*PerOperation* (success: suppressed once acknowledged for that operation),
*CollapsibleWarning* (collapsed per evidence, shown again when the reason changes)
and *AlwaysShown* (the legacy behaviour). A message counts as seen only after a
visibility event. An explicit request always opens the explanation. Not yet
consulted by any page.

## The catalogue (`guidance/catalogue.rs`)

81 scripts: **42 numbered design scripts**, **4 documented variants**, **35 legacy**.
By category: WhyBlocked 17, Explain 41, Warning 6, EmptyState 7, Success 5, Tip 5.
Levels authored: Quick 81; Minimal, Explain and Technical 46 each (every design script
authors all four). IDs are stable (`first_run.choose_sources` ... `activity.queued_work`;
legacy keys unchanged).

The four variants are the design's own documented alternates, kept as separate IDs so
selectors stay disjoint and analysable: `identity.unknown.explain` (08 with no blocked
operation), `artwork.stale_usable_cache.source_available` (16, source available),
`patch.wrong_base.hash_only` (24, no revision known) and
`repair.completed_verification_pending.unavailable` (38, verification unavailable).

Every design script requires at least one fact the page must supply, so none can
appear merely because a page opened. The legacy scripts require only the legacy fact
kinds projected from the old optional evidence fields. Until a page-owned adapter
supplies design facts, production behaviour is therefore unchanged. Wording is the
approved wording; the only edits are plural forms made explicit (`{n?one|many}`) and
the number word in script 17's Explain text.

### Coverage against the design

* **Design categories:** all 29 numbered topics and the Activity supplement have at
  least one script (checked by test).
* **Scripts:** all 42 are present, each exactly once.
* **Deliberately deferred (present, but no adapter yet supplies their evidence):**
  16 and 40 (artwork cache staleness and alternatives; the design says they stay
  ineligible until explicit evidence plumbing exists), and every script whose fact
  needs a domain owner to expose it. No script was dropped.
* **Obsolete:** none from the 42. "Wizzy" is retired and appears in no catalogue text.
* **Legacy messages the design marks REMOVE or REWRITE** (always-on banners such as
  `museum-browse`, `settings-hints`, `games-browse`, `advanced-inspect`) are kept
  verbatim for now so no page changes; removal belongs with the Phase 2 placement
  work, not with the engine.
* **Current-main call sites with no catalogue coverage (Phase 2 inputs):** the empty-state
  badges in `pages.rs` (Home, Games, Museum, History, Activity), the Duplicates hero and
  empty states (which still say "Wizzy" in two places in `pages.rs`), the Problems hero,
  onboarding welcome, the Mods workshop header, and the family hubs. `guidance_context`
  supplies only library presence, selected-game identity, Problems counts and the
  running-job count; every other fact is unsupplied.

## Known limits, stated plainly

* The legacy `launch-identity-blocked` message still keys on the single
  `identified` boolean the page supplies and can contradict the richer launch
  readiness result. It is preserved for compatibility and superseded, in the catalogue,
  by `identity.unknown`, `identity.candidate_only` and `launch.no_compatible_emulator`
  once a page supplies readiness facts.
* `GuidanceState` holds the exposure model but does not consult it; guidance is
  still shown after the page body and still suppressed on short windows by `pages.rs`.
* Page adapters, placement, rendering of actions and levels, preference migration
  and the optional AI layer are all later work.

## Tests and tooling

`gui_v2/guidance/tests.rs` (57 tests) covers determinism and fact-order
independence; precedence (blocker over tip, warning over success, refusal never hidden
by an informational message, checked over every blocker/informational and
blocker/success pair); empty state; success; no evidence; explicit `None`; the four
levels; typed action projection and alternates; mascot consistency; no false authority
(unknown, candidate and unsupported never read as verified or ready, blocked stays
blocked beside contradictory good news, invalid facts invent nothing); the repeat
policies with an injected clock; and catalogue lints (unique IDs, exactly 42 numbered
scripts, 35 legacy keys, fixed priority bands, placeholders supplied by required facts,
explicit plurals, voice guide, every script reachable under the selector rules, no
duplicate selectors, every topic covered, and exactly one legacy answer per page for
every evidence combination). `guidance/audit.rs` reports counts, IDs, levels,
duplicates, uncovered topics and unreachable scripts:

```sh
cargo test -p archivefs-gui --lib print_catalogue_report -- --ignored --nocapture
```
