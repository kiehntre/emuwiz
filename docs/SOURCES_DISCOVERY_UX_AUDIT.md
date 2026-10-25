# Sources / Discovery UX audit

## 1. Executive summary

The Sources destination has sound underlying behaviour but does not yet make
that behaviour easy to understand at a glance. It already uses the shared
source configuration, scanner, database, and discovery seams; no new ingestion
architecture is justified. The smallest useful redesign is therefore a GUI
information-hierarchy pass over the Libraries tab, with the existing Discovery
tab remaining the detailed result view.

The main issue is terminology and hierarchy. The page says that it manages
folders EmuWiz scans for *archives*, while the implementation also discovers
loose ROMs, discs, images, folders, and other recognised content. It opens with
an overview that includes cheat-database state, then presents a path-led list,
and places the read-only promise chiefly in the mount-destination section.
A novice can add a folder, but cannot immediately tell that a source is a
folder of their games, what scan will do, or where its outcome belongs.

Recommended direction: retain the existing `Sources` destination and its four
tabs, name the Libraries tab's primary object **Game source**, promote an
add-source CTA and local/read-only explanation, and render each existing source
as a responsive card whose health, last result, and next action lead before its
full path. Do not merge DATs, Cheats, RomM, or Discovery into this flow.

This can be predominantly GUI-only. `SourceFolderView`, `ScanPersistSummary`,
and the persisted most-recent discovery run already supply configuration,
availability derived from scan history, last scan, archive count, discovery
counts, failures, disable/remove, and recovery actions. A fresh “reachable
now” claim is the one meaningful backend gap: `Available` currently means
enabled with no failed scan (including never scanned), not a live probe.

## 2. Current page architecture

- `crates/archivefs-gui/src/sources_page.rs` renders the shared Sources header,
  the Libraries-tab overview, source-folder list, add/remove dialogs, last-scan
  banner, and the Discovery-tab bridge. The header describes Sources as games,
  DAT catalogues, and cheats; the tab row is Libraries, DATs, Cheats, Discovery.
- `ArchiveFsApp::show_sources_page` in `main.rs` dispatches those tabs to their
  existing renderers. `show_sources_libraries_tab` merges configured roots with
  the cached database snapshot; it starts the existing background source actions
  rather than scanning or persisting in the UI.
- Core `SourceFolderConfig` persists `path`, `enabled`, and optional creation
  time in config. `SourceFolderView` joins those fields to database-held scan
  history, last successful scan, last archive count, platform assignment, and
  unknown count. Writes are atomic; add, enable/disable, and remove call the
  same core functions as the CLI.
- `scan_source_folder_*` and `scan_all_enabled_sources_*` share
  `scan_and_persist_folders`. Scans are read-only against source files. Each
  folder is isolated: a failed folder records a failure and preserves its prior
  catalogue rows while other folders can finish.
- The scanner persists exact aggregate scan counts and pageable discovery
  details. `collection_discovery_page.rs` renders the most recent completed
  run after restart, or the current-session summary where available.
- Home’s “Add your games” / “Build my library” card routes to Sources. Its copy
  is notably clearer than Sources: it says the user chooses where games are
  stored and that EmuWiz will scan without changing files.
- Onboarding’s Add Source step embeds `show_sources_page(..., Libraries)`
  directly. It already has a source explanation and does not maintain a second
  source workflow.

## 3. Current user journey

| Journey | What currently happens | Audit finding |
| --- | --- | --- |
| A. First-ever source | Home/onboarding reaches Sources; Add folder opens a path dialog; shared validation requires an existing readable absolute directory; add persists and registers it. | Correct and safe, but the page’s main explanatory copy is less direct than Home/onboarding. Normal Sources adds do not automatically scan. |
| B. Add another source | The same dialog validates against all configured paths. | Safe; duplicate and parent/child overlap are rejected, but the recovery wording is raw backend text and there is no pre-submit explanation. |
| C. Missing/offline path | A scan records `Unavailable` if its error says not found; old catalogue rows remain. | The card labels availability and shows raw last-scan error, but does not give a plain recovery action. |
| D. Removable drive unavailable | It follows the same `Unavailable` route. | Correctly never relocates or alters configuration, but does not say “connect this drive, then rescan.” |
| E. Network mount unavailable | It follows the same unavailable/permission/scan-failed classifications. | Correctly never mounts a network location, but cannot distinguish an offline mount from another missing path. |
| F. Duplicate/overlap | Canonicalised add validation rejects exact aliases and parent/child roots. | Strong backend rule; surface the reason and selected conflicting source in beginner language. |
| G. Empty recognised result | A successful scan can persist zero archive rows and ingestion counts; Discovery can show what was or was not recognised. | The source row says `Archives: 0`, which is too archive-specific and does not make the next step obvious. |
| H. Partial success | Per-folder failures are retained in `folder_errors`; completed folders persist normally; the last-scan banner has totals and skipped inspection. | Global banner can imply completion without foregrounding the failing folder; Discovery is a separate tab with no source-specific handoff. |
| I. Remove/disable | Disable preserves catalogue and excludes Scan All; enable intentionally scans; Remove defaults to keeping catalogue and never deletes source files. | Semantics are safe but the action labels need concise consequence copy. “Remove” and “Keep catalogue entries” are technical for a novice. |
| J. Return later | Rows show enabled state, derived availability, archive count, platform, last scan, and error. | Useful data exists, but path is visually first and status meaning/staleness is not explained. |

## 4. First-source experience

The first-run route is architecturally correct: onboarding hands over to the
real Sources Libraries tab and its actual add dialog, then only gates
Continue on whether a source exists. Keep that contract. Do not add an
onboarding-only picker, scan, state model, or duplicate validation.

The destination itself should make the handoff self-sufficient:

1. Header: “Game sources” with one sentence: “A game source is a folder where
   your games already live. Scanning reads it to build your library; it never
   moves, renames, or deletes files.”
2. Empty state: “Add your first game folder” and a single primary **Add game
   folder** action. Keep “existing readable folder” as useful supporting text.
3. After add: show a configured-but-not-scanned card, with **Scan now** as the
   clear next action. This honours the existing explicit-add/no-automatic-scan
   contract; no hidden rescan is proposed.
4. After scan: show a focused completed/result card and a clear **View scan
   details** handoff to the existing Discovery tab.

This also reconciles the current mismatch: onboarding and Home use “games” and
read-only language, while the main list calls them “source folders” and says it
scans “archives.”

## 5. Existing-source management

Use a source card rather than a path-first grouped row. It should be a layout
change over the existing `SourceFolderView`, actions, dialogs, and widgets:

- Friendly primary label derived locally from the path's final component, with
  a deterministic fallback such as “Game folder”; keep the complete path as a
  secondary copyable value. Do not persist a display-name field in this pass.
- Status strip first: **Ready**, **Not scanned yet**, **Offline**,
  **Permission needed**, **Scan needs attention**, or **Disabled**. Explain
  that “Ready” means last known scan state until a live probe exists.
- Facts below: “Items found” (with existing archive count labelled precisely as
  “archive entries” where necessary), “Last scan”, and optional source-platform
  assignment. Do not present discovery totals as durable per-source facts until
  the data model supports that scope.
- One context-sensitive primary action: **Scan now** / **Rescan** / **Try
  again** / **Enable and scan**. Keep Scan All only as a compact page action
  when there are two or more enabled sources.
- Quiet actions: **View scan details**, **Disable**, and **Remove from
  EmuWiz**. Keep platform assignment in an advanced/details menu; it should not
  compete with first-run ingestion.
- Removal dialog: retain its default “keep catalogue” safety, but phrase it as
  “Remove this folder from EmuWiz” and “Keep previously found library entries
  (recommended). This does not delete your folder or games.”

The existing 320px list cap and horizontally packed controls are likely to
become cramped at narrow widths. Follow Home/Doctor/Emulator Setup’s card,
status-strip, wrapped-action approach: stack card facts and actions below a
roughly tablet-width breakpoint; never truncate the only recovery message;
show a shortened visual path with copy/full-path disclosure rather than making
long mount paths define the card width.

## 6. Offline/error/degraded states

Existing core semantics are conservative and should remain so.

- Offline/missing, permission denied, and other failed scans are distinct
  `SourceAvailability` states. Doctor receives one source-health issue per
  affected source, not one per historic archive. Existing entries are retained.
- A Scan All run continues after a folder failure. `ScanPersistSummary` retains
  `folder_errors`; successful folders and their Discovery details can still be
  shown.
- Disabled takes precedence over failure history, correctly avoiding a false
  alarm for a user-directed state. Re-enabling deliberately scans; disabling
  does not.
- The UI must not claim live health from a past scan. Current `Available` is
  optimistic for a newly validated or previously successful path; it does not
  test a removable or network source merely by opening Sources.

For the present capability, use “Last scan succeeded” rather than “Reachable”
when that precision matters. A card with a failed status should say what was
preserved and offer the existing explicit retry. Suggested recovery copy:

- Missing/offline: “This folder was not available when EmuWiz last scanned it.
  Connect or mount it, then try again. Your previous library entries are kept.”
- Permission: “EmuWiz could not read this folder. Check its permissions, then
  try again. Your previous library entries are kept.”
- Other scan failure: “The last scan could not finish. Try again; technical
  details are available if it continues.”
- Empty result: “The scan finished, but found no supported game items here.
  Check that you selected the folder containing the games, then view details
  or choose another folder.”

Do not attempt source relocation, mount detection/mounting, network mounting,
or automatic retry. A future live probe must be explicit (for example
Refresh status), bounded to metadata/readability, and clearly reported.

## 7. Information hierarchy problems

1. **Source scope is unclear.** “Sources” spans game folders, DATs, and cheats;
   the Libraries tab’s “configured folders EmuWiz scans for archives” is both
   narrower than real ingestion and less friendly than Home.
2. **The primary job is diluted.** A global overview includes cheat-database
   readiness before source setup, and the Libraries tab continues into unrelated
   catalogue, BSFree, and RomM blocks. These are already collapsed later but
   still make the destination feel like administration rather than “add games.”
3. **The path leads the card.** Long absolute paths receive the strongest
   visual treatment, while health and outcome require scanning badges, labels,
   and prose.
4. **Configured, last-scanned, and currently reachable are conflated.** The
   data model is honest but `Available` reads as a current check. A never
   scanned source is also classified available after add validation.
5. **Result terminology is inconsistent.** Rows count “Archives,” while the
   last-scan banner knows about loose ROMs, discs, disks, and folders. Discovery
   describes universal ingestion separately.
6. **Recovery is weakly actioned.** Raw per-row scan error is visible, but the
   recommended next action and preservation guarantee are not made explicit.
7. **Discovery is valuable but disconnected.** It has detailed persisted,
   pageable facts and suggested actions, yet the source card only has a global
   “Scan / detect” and no obvious result handoff.
8. **Action wording is technical/duplicated.** “Scan / detect,” “Re-run
   platform detection,” and “Assign platform” coexist; Scan All and row scan
   have no clear scope explanation. Remove/disable are safe but their effects
   are not visible until dialog/prose.

DAT verification should be framed as a later, optional **Verify Games** task:
it compares files against a trusted catalogue and does not change sources or
their contents. Playing Library/RomM should be framed as an optional later
connection/view of the already configured game library, not another way to add
or scan a source. This is copy and navigation only; it does not join workflows.

## 8. Proposed page structure

```text
Sources
  Libraries | DATs | Cheats | Discovery

Game sources                                      [Add game folder]
A game source is a folder where your games already live. Scans are read-only.
Health summary: 2 ready · 1 needs attention       [Scan all enabled]

[Game folder: Retro USB]              [Offline · last scan]
 /media/retro/roms
 Previous library entries kept · 428 archive entries · Last scan: …
 Connect or mount the drive, then try again.
 [Try again] [View scan details] [••• Disable / Remove]

[Game folder: Handheld]               [Not scanned yet]
 /home/me/Games/Handheld
 Added … · No scan results yet
 [Scan now] [••• Remove]

Optional next steps
 Verify Games with a DAT (optional; read-only)    [Open Verify Games]
 Connect Playing Library/RomM later (optional)    [Open library connections]
```

Only the Libraries-tab top area and source cards are P0. “Optional next steps”
should be a small cross-navigation panel or links to existing destinations,
not embedded DAT/RomM setup or new work. The existing Collection Discovery tab
remains the single detailed explanation and item list; its empty state should
retain the existing direction to scan from Sources.

## 9. Existing seams to reuse

| Capability | Source of truth / reuse seam | Classification |
| --- | --- | --- |
| Add existing readable folder, safe config bootstrap, duplicate/overlap rejection | `validate_new_source_folder`, `add_source_folder_*`, existing Add dialog | REUSE |
| Enable/disable/remove with catalogue-preservation default | `set_source_folder_enabled_*`, `remove_source_folder_*`, existing dialogs | REUSE |
| Per-source enabled, scan history, last success, count, platform, error | `SourceFolderView` and `merge_configured_sources` | REUSE |
| Scan one/all with per-folder isolation and persisted results | existing `SourceAction` and `scan_*` calls | REUSE |
| Scan result counts, skipped reasons, platform breakdown, pageable details | `ScanPersistSummary`, persisted Discovery run, `collection_discovery_page` | REUSE |
| One source health finding in Doctor | `source_health_issues` and existing Doctor inputs | REUSE |
| Friendlier display label, status/copy mapping, card hierarchy, responsive wrapping, tab handoff | GUI-only projection over existing data/actions | SMALL |
| Make global partial failure obvious and carry a user to Discovery after a scan | existing summary plus GUI state/navigation | SMALL |
| Explicit fresh reachability check with a timestamp/outcome, without scanning | new read-only core probe and view field; must not run on render | MEDIUM |
| Persist exact per-source universal-ingestion totals and per-source Discovery filtering | schema/query/report additions; existing run aggregate is not sufficient | MEDIUM |
| User-renamable source labels | config schema/migration and editing UX | DEFER |
| Automatic relocation, automatic network mounting, silent/background rescans, provider scraping | contrary to scope and safety model | DEFER |

## 10. Required production changes

No production change is made by this audit. For the bounded redesign, change
only the following later, in order:

1. `crates/archivefs-gui/src/sources_page.rs`: replace the Libraries-tab top
   copy, empty state, source-row hierarchy, status wording, actions, removal
   copy, and narrow-layout behaviour; add the existing Discovery-tab handoff.
2. `crates/archivefs-gui/src/main.rs`: only route any new GUI action/result
   handoff through the current `SourcesPageAction` and `SourceAction` lifecycle.
   Do not duplicate configuration, scanner, or ingestion logic.
3. `crates/archivefs-gui/src/home_page.rs`: only align wording/CTA labels if
   needed so Home and Sources name the same thing consistently.
4. `crates/archivefs-gui/src/collection_discovery_page.rs`: only improve the
   no-result / partial-scan explanatory copy and an existing navigation handoff
   if P0 testing shows it is needed.
5. Only if P1 live health is accepted: narrowly extend
   `crates/archivefs-core/src/lib.rs` (or a focused source-health module) and
   the view/snapshot seam; add no scanner mutation or implicit probe on render.
6. Only if per-source Discovery totals are accepted: extend database schema,
   persistence, and query APIs deliberately. Do not infer per-source totals
   from the run-wide aggregate.

Do not change onboarding logic: its direct call into the real page is the
integration to preserve. Do not touch launch/AppImage, ES-DE mappings, Cheats
& Mods, onboarding implementation, or LBC for this work.

## 11. Test plan

Preserve existing core coverage for validation, overlap, source availability,
partial scan isolation, enable/disable, and safe removal. Add GUI-focused tests
alongside `tests/library_views_and_sources.rs` that assert rendered text and
returned actions for:

- empty first-source state: read-only reassurance and one Add game folder CTA;
- unscanned newly configured source: “Not scanned yet” and Scan now;
- available last-scan state: facts and Rescan, without saying it was freshly
  checked;
- unavailable removable/network-like path, permission denied, and generic scan
  failure: distinct human recovery copy, retained-entry assurance, retry;
- zero result and partial multi-source result: plain next step and Discovery
  handoff, while completed sources remain visibly successful;
- duplicate and parent/child overlap: existing validation is rendered in
  user-facing wording and creates no action;
- disable, enable-and-scan, and remove default: labels accurately state what
  remains and no filesystem deletion is offered;
- narrow and desktop render sizes: cards stack, action labels remain reachable,
  and full paths do not force horizontal layout;
- Home and Sources use compatible “game folder/source” and read-only language;
- onboarding still uses the same page/action path (existing onboarding flow
  test remains the guard, with no separate implementation).

If P1/P2 backend items are authorised later, add core tests for explicit probe
semantics and non-mutation; add database migration/query tests for per-source
ingestion totals. Do not run a scan as part of mere page rendering tests.

## 12. Definition of Done

The redesign is done when a novice can open Sources without onboarding and
correctly answer: a source is an existing game folder; scanning is read-only;
what was last found; whether a scan last succeeded or needs attention; what to
do next; and that DAT verification and Playing Library/RomM are optional later
tasks. The same page must clearly distinguish configured, disabled,
not-yet-scanned, last-scan-successful, and failed states without claiming fresh
reachability it has not checked.

All folder changes must continue through the existing core APIs. Duplicate and
overlap rejection, no automatic source relocation/mounting/rescan, per-folder
partial-scan preservation, and remove-without-file-deletion guarantees must
remain intact. Discovery remains the one detailed ingestion-results view and
onboarding continues to invoke the real Sources page directly. Existing and
new focused tests pass.

## Implementation backlog

### P0

- **SMALL:** Reword/restructure the Libraries tab into a game-source first
  experience: read-only explanation, Add game folder CTA, novice empty state,
  source cards, status-first facts, and responsive action layout.
- **SMALL:** Map existing status/error/scan data to plain recovery guidance;
  make remove/disable consequences explicit; add a View scan details handoff.
- **SMALL:** Add focused GUI render/action tests and preserve the current
  onboarding-to-real-page path.

### P1

- **MEDIUM:** Add an explicit, user-triggered, read-only reachability probe and
  a timestamped result only if “reachable now” is a product requirement.
- **SMALL:** Align Home and Sources terminology where P0 changes require it.

### P2

- **MEDIUM:** Persist/query exact per-source universal-ingestion totals and
  enable a source-filtered Discovery result view, if evidence shows aggregate
  results are insufficient.

### DEFER

- **DEFER:** Editable source display names.
- **DEFER:** Automatic relocation, network mounting, silent/background scans,
  new ingestion pipelines, or provider scraping.
