# GUI-v2 → legacy GUI handoff audit

**Audit basis:** `1f093cae591f270a64ddc86213a3e18d9bece0cc`  
**Scope:** `crates/archivefs-gui/src/gui_v2` and the legacy bridge it invokes.  
**Audit date:** 2026-09-25

## Method and counting rule

This inventory distinguishes an actual GUI-v2 control that can reach
`Command::Legacy` from a destination case that merely remains in the shared
legacy compatibility map. The total below counts live handoff controls. Mapping
entries with no current caller are listed separately as `DEAD/STALE`; counting
those as user-visible handoffs would overstate the remaining migration work.

The bridge is deliberately contextual: `gui_v2::legacy::open` starts the
current executable with `--legacy`, a serialized `Section`, and optionally the
selected game path (`gui_v2/legacy.rs:34-48`). The legacy host chooses a
`MainView` from that section (`gui_v2/legacy.rs:10-31`). No production code was
changed for this audit.

## Summary

| Category | Live handoff controls | Meaning |
|---|---:|---|
| REMOVE NOW | 0 | No live handoff had proven feature-complete native parity. |
| RETAIN | 3 | The legacy destination still supplies capability not present in the native page. |
| PARTIAL | 1 | Native coverage exists, but the contextual workflow still falls through to legacy. |
| DEAD/STALE | 0 live controls | Compatibility mappings with no current caller are listed below. |
| **TOTAL** | **4** | Direct GUI-v2 call sites that can currently invoke `self.legacy(...)`. |

## Complete live handoff inventory

### RETAIN — Advanced specialist interface

- **GUI-v2 source page/route:** `Route::Section(Section::Advanced)` — the
  Advanced tools page.
- **Legacy destination:** `Section::Advanced` → `MainView::Library`, with the
  legacy host placed in `GuiMode::AdvancedView`.
- **Purpose:** Open specialist mount, media, storage, and journal tooling that
  remains outside the native page.
- **Current user-facing label:** `Open specialist interface`.
- **Native replacement:** **partial**. Native archive inspection and DAT
  management are present, but the page explicitly identifies the remaining
  specialist interface as the place for mount/media/storage/history details.
- **Safe to remove now:** **no**.
- **Evidence:** `pages.rs:1964-1971` calls `self.legacy(Section::Advanced)`;
  `legacy.rs:26` and `legacy.rs:63-68` define its destination and advanced
  mode. The native route table separately renders `self.advanced(ui)` at
  `pages.rs:407`.
- **Missing parity:** Native equivalents for the remaining specialist
  mount/media/storage/journal workflows, including their existing safety and
  diagnostic behavior.

### PARTIAL — Selected-game Problems & Repair fallback

- **GUI-v2 source page/route:** Game detail → `Fix Problems` →
  `Route::Task { section: Section::Problems, game }`.
- **Legacy destination:** `Section::Problems` → `MainView::Problems`.
- **Purpose:** Review or repair problems for the selected game.
- **Current user-facing label:** `Review problems` (from
  `Section::Problems::action()`), preceded by the game-detail label
  `Fix Problems`.
- **Native replacement:** **partial**. A native Problems & Repair section
  exists, but the selected-game task is not handled by a native contextual
  route. It reaches the generic `handoff` arm.
- **Safe to remove now:** **no**.
- **Evidence:** `pages.rs:1758-1765` creates the task;
  `pages.rs:396` renders the section page natively but
  `pages.rs:428-432` sends unhandled task sections to `handoff`;
  `pages.rs:2240-2249` invokes `self.legacy(section)`. The explicit routing
  test confirms `native_route_for_handoff(Section::Problems, Some(41))` is
  `None` (`tests.rs:1541-1557`).
- **Missing parity:** A selected-game Problems & Repair route that preserves
  the game context and exposes equivalent native review, preview, repair, and
  recovery controls. Removing this handoff before that exists would strand the
  selected-game action or silently change its scope.

### RETAIN — Generic technical-interface escape

- **GUI-v2 source page/route:** The `Advanced details` disclosure in the
  generic contextual handoff page.
- **Legacy destination:** `Section::Advanced` → `MainView::Library` in
  advanced mode.
- **Purpose:** Provide an explicit escape to the complete technical
  interface when a workflow is not yet native.
- **Current user-facing label:** `Open full technical interface`.
- **Native replacement:** **no** for the complete technical interface.
- **Safe to remove now:** **no**.
- **Evidence:** `pages.rs:2258-2261` contains the button and calls
  `self.legacy(Section::Advanced)`; the legacy destination is defined at
  `legacy.rs:26` and `legacy.rs:63-68`.
- **Missing parity:** Full native coverage of the specialist technical
  workflows and their established confirmations/diagnostics.

### RETAIN — Existing application settings

- **GUI-v2 source page/route:** `Route::Section(Section::Artwork)` → the
  Artwork & Metadata page’s advanced details.
- **Legacy destination:** `Section::Settings` → `MainView::Settings`.
- **Purpose:** Open application settings that are not part of the native
  artwork/provider page.
- **Current user-facing label:** `Open existing application settings`.
- **Native replacement:** **partial**. Native artwork/provider browsing and
  setup exist; the complete application settings surface does not.
- **Safe to remove now:** **no**.
- **Evidence:** `pages.rs:392` and `pages.rs:2027-2050` render native artwork
  and provider setup, while `pages.rs:2351-2353` invokes the Settings
  handoff. `legacy.rs:29` maps that section to `MainView::Settings`.
- **Missing parity:** A native settings surface with parity for the existing
  application-wide configuration, plus preserved settings semantics.

## Area-by-area parity matrix

| Area | GUI-v2 native evidence | Legacy handoff evidence | Category / safe to remove | Missing parity |
|---|---|---|---|---|
| RomM browsing | `pages.rs:390` renders `romm_library_page`; the page is read-only and provenance-aware. | `legacy.rs:27` has `Section::Romm → MainView::Sources`, but no `self.legacy(Section::Romm)` call exists in GUI-v2. | **DEAD/STALE mapping; no live handoff** / no removal action in this audit. | None for browsing based on this audit. RomM mapping administration is not proven native and is not exposed by a live handoff here. |
| RomM refresh/import | Native RomM page controls and operation handling are in `pages.rs:74-107` and the RomM library/backend modules. | No live `self.legacy(Section::Romm)` caller. | **DEAD/STALE mapping; no live handoff**. | No removal of the compatibility map is proposed without a separate reachability/deletion decision. |
| RomM mapping/admin | No native mapping-administration route was found in the inspected GUI-v2 route or handoff call sites. | The only related legacy map is the stale `Section::Romm` case at `legacy.rs:27`; it has no current caller. | **RETAIN capability; no live handoff found** / not safe to claim removal. | Native mapping administration and its validation/conflict behavior are not evidenced. |
| Artwork/metadata | `pages.rs:392`, `pages.rs:420-423`, and `pages.rs:2027-2225` provide native metadata, provider, provenance, and conflict presentation. | `legacy.rs:22` maps Artwork to Settings, but the normal Artwork routes are rendered natively; the remaining Settings control is separately inventoried above. | **RETAIN** for the Settings control; Artwork mapping itself is **DEAD/STALE**. | Complete application settings parity, not artwork browsing parity. |
| Bezel | Native bezel panel is mounted from the artwork task at `pages.rs:2087-2095`. | No bezel section or bezel-specific legacy call site exists in the inspected GUI-v2 code. | **DEAD/STALE / no handoff**. | None identified for handoff removal. |
| Sources/providers | `pages.rs:389` renders Sources; native provider setup is in `native_workflows.rs:1272-1405`. | `legacy.rs:23` maps Sources to `SourcesDiscovery`, but no direct GUI-v2 `self.legacy(Section::Sources)` call exists. | **DEAD/STALE mapping; no live handoff**. | Any unsupported advanced provider administration must remain outside the native parity claim. |
| History/undo | `pages.rs:405` renders History; the page exposes transaction state and undo controls (`pages.rs:1539-1663`). | `legacy.rs:25` maps History to `HistoryLogs`, with no current GUI-v2 legacy caller. | **DEAD/STALE mapping; no live handoff**. | No native/legacy deletion is authorized by this audit. |
| Selected-ROM evidence | Game detail calls native selected-ROM evidence at `pages.rs:1721`; Launch has native task rendering at `pages.rs:416-419`. | `legacy.rs:17-18` retains Launch destinations, but `native_route_for_handoff` intercepts Launch (`pages.rs:2358-2363`), and tests cover this (`tests.rs:1508-1557`). | **DEAD/STALE Launch fallback mapping for current routes**. | None for the audited selected-ROM evidence path. |
| Conflicts/repair | Native Problems & Repair section is rendered at `pages.rs:396`; native history/undo is also present. | Selected-game Problems task reaches `handoff` and calls `self.legacy(Section::Problems)` (`pages.rs:1758-1765`, `2244-2249`). | **PARTIAL / retain**. | Selected-game contextual repair parity. |
| Diagnostics | Native check/problem pages and technical details exist, but the generic task fallback and specialist escape remain. | Generic fallback and Advanced escape are live (`pages.rs:2244-2261`). | **RETAIN/PARTIAL**, as detailed above. | Complete native diagnostic and specialist-tool parity. |
| Advanced technical tools | Native Advanced route and archive inspector exist (`pages.rs:407`, `424-427`, `1964-1971`). | Advanced specialist button and technical escape launch `Section::Advanced`. | **RETAIN**. | Mount/media/storage/journal capability and parity-level safety UX. |

## Legacy destination mappings with no current GUI-v2 caller

These are not counted as live handoffs. They are cases in
`gui_v2/legacy.rs::destination` that remain available to the bridge if some
future or external caller supplies the corresponding `Section`, but the audit
found no current GUI-v2 control that calls `self.legacy` with them:

| Section | Legacy destination | Current native/route evidence |
|---|---|---|
| `Romm` | `MainView::Sources` | Native RomM route at `pages.rs:390`; no caller of `self.legacy(Section::Romm)`. |
| `Artwork` | `MainView::Settings` | Native Artwork routes at `pages.rs:392`, `420-423`; only the separately listed Settings control is live. |
| `Sources` | `MainView::SourcesDiscovery` | Native Sources route at `pages.rs:389`; no caller of `self.legacy(Section::Sources)`. |
| `History` | `MainView::HistoryLogs` | Native History route at `pages.rs:405`; no caller of `self.legacy(Section::History)`. |
| `Launch` | `MainView::Selected` / `ReadyToPlay` | Native Launch route and explicit stale-task guard at `pages.rs:2235-2242`, `2358-2363`. |

The other destination cases remain reachable only through the generic legacy
method if a corresponding unsupported task is constructed. Their continued
presence is not evidence that a visible GUI-v2 handoff exists; the source of
each such task must be traced before removal.

## Conclusions and non-actions

1. No handoff is classified **REMOVE NOW**. Every live control either supplies
   an admitted specialist capability or preserves a contextual workflow whose
   native parity is incomplete.
2. No user-facing labels or production routing were changed. This task is an
   evidence-only audit.
3. The next safe reduction candidate is selected-game Problems & Repair, but
   only after a native contextual route is implemented and tested. The
   Advanced and Settings controls require broader parity work.
4. The stale mapping cases should not be deleted as part of this audit. They
   are compatibility code, and proving that no non-GUI-v2 caller can supply
   them is a separate change with its own tests.

