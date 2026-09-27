# Game Details as the Primary Game Hub

**Status:** implementation-ready product specification; research only.

**Starting point audited:** `33ecab378c54cfdfa4dd0303e1008b114b920878`.

**Goal:** make `Route::Game(game_id)` the stable context root for one selected game without creating a second management page or duplicating backend truth.

## CURRENT GAME DETAILS

The current native Game Details renderer is `game_detail` in [`gui_v2/pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1704). It already provides:

| Capability | Current location/status | Classification |
|---|---|---|
| Title/platform/media | Header and selected-game card; cover, platform, media kind, source path | **INLINE** |
| Play | Primary Play button routes to a game-specific Launch task | **INLINE** |
| Identity / verification | Current verification, saved checks, identity status, and selected-media evidence | **INLINE** |
| DAT evidence | Reachable through Verify/DAT surfaces and selected evidence; not a compact top-level card | **LINKED / ADVANCED** |
| Artwork / metadata | Artwork action and cover/screenshots/about sections | **INLINE + LINKED** |
| Emulator readiness | `show_game_details` triggers existing profile/firmware checks and launch readiness; selected-game summary is displayed, but the primary action remains a separate launch task/panel | **INLINE but fragmented** |
| Firmware | Computed for relevant launch candidates and reachable through BIOS/Firmware/Doctor; not consistently summarized beside Play | **CONTEXTUAL / LINKED** |
| Launch readiness | Existing launch-readiness panel receives the shared launch input after selected evidence/media panels | **INLINE but too low in hierarchy** |
| Cheats | Game Details action opens game-specific Mods/Cheats task | **LINKED** |
| Mods | Same Mods/Cheats task and native Mods page | **LINKED** |
| Saves/backups | Collapsible Saves & Backups section with Open Saves & States; compare/restore remain disabled in the current foundation ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1852)) | **INLINE + LINKED** |
| Manuals/guides | Collapsible local Manuals & Guides panel ([`pages.rs`](../../crates/archivefs-gui/src/gui_v2/pages.rs:1876)) | **INLINE** |
| Problems & Repair | Game-specific Fix Problems action; full problem list is separate | **LINKED** |
| Conversion | Generic Converter/specialist routes; no consistent selected-game conversion action in the card | **GLOBAL-ONLY / MISSING CONTEXT LINK** |
| Multi-disc | Disc metadata/specialist evidence can appear, but there is no single compact disc-set control in the audited Game Details action row | **CONTEXTUAL / PARTIAL** |
| History / undo | Global Activity/History and workflow-specific receipts; no consistent “changes to this game” projection | **GLOBAL-ONLY** |
| Specialist media/evidence | Dreamcast IP.BIN, Wii U disc, Saturn manifest, tape/archive inspectors and evidence panels | **INLINE or ADVANCED by media type** |
| Bezel/decorations | Available in artwork-related surfaces rather than clearly represented as a game tool in the primary action row | **ADVANCED / LINKED** |

The renderer deliberately keeps technical evidence in collapsible sections and starts generation-guarded evidence loading for the focused path ([`selected_game_readiness.rs`](../../crates/archivefs-gui/src/selected_game_readiness.rs:253)). That is the right foundation for a primary hub.

## CURRENT FRAGMENTATION

The selected-game context is fragmented in five ways:

1. **Play versus readiness:** Game Details has Play, but the useful readiness explanation is rendered later in the selected-game flow and the task route carries the user away from the hub.
2. **Emulator and firmware:** profile/firmware checks are started from Game Details, but setup and BIOS actions live in global Setup/Doctor surfaces.
3. **Verification:** the game-specific action is present, but the route goes to a platform/global Check section rather than a consistently game-scoped evidence review.
4. **History:** cheat/mod/repair/organisation receipts exist, but users normally reach global history or workflow-specific result panels first.
5. **Conversion and multi-disc:** specialist capabilities exist, but availability is not summarized as a contextual game action.

The proposed hub reduces this fragmentation through projections and return context. It does not move the underlying engines or make every global tool inline.

## PRIMARY HUB PRINCIPLE

Game Details answers, in order:

1. What game and media is selected?
2. Is it healthy and available?
3. Can it be played now?
4. Which emulator/profile will be used?
5. What must happen before launch?
6. Which optional tools are available?
7. What has changed for this game?
8. What can safely be undone?

`Route::Game(game_id)` is the context root. A child workflow receives the GameId and returns to that route. Global pages remain global when their scope is the collection, provider, or system rather than one game.

## INFORMATION HIERARCHY

```text
Game Details
├── Title / Media Header
├── Play / Readiness
├── Identity / Verification
├── Game Tools
│   ├── Artwork & Metadata
│   ├── Cheats & Mods
│   ├── Saves & Backups
│   ├── Manuals & Guides
│   └── Conversion (only when supported)
├── Health / Repair
├── Disc / Media Structure (when applicable)
├── Recent Changes (when records exist)
└── Advanced Details
```

This is an information hierarchy, not a requirement to render every branch as a large dashboard. The card should use progressive disclosure and only show sections supported by the selected media/platform.

## ALWAYS-VISIBLE CONTENT

Near the title and primary artwork, always show:

- game title and platform;
- media/container kind and selected source availability;
- compact Play/readiness summary;
- important hard blocker or warning;
- the primary Play or next-action control.

The first layer should not require opening a global page to answer “Can I play this?”

## CONTEXTUAL CONTENT

Show these only when relevant:

- firmware status when the selected launch candidate requires it;
- selected emulator/core and Change emulator when supported;
- disc count/current disc for a grouped multi-disc game;
- active identity or media warning;
- conversion actions for the selected platform/media;
- current repair/problem summary when launch or health is affected;
- recent launch result as context, never as readiness authority;
- specialist manifest/IP.BIN/tape/archive evidence for matching media types.

## SECONDARY CONTENT

Secondary sections remain discoverable without competing with Play:

| Section | Compact status idea | Empty state |
|---|---|---|
| Artwork | `Artwork: cover available` / `Screenshots: 3` | “No artwork has been linked yet. This does not affect play.” |
| Cheats | `Cheats: 3 available` or `Cheats: none` | “No cheats are attached to this game.” |
| Mods | `Mods: 2 installed` / `Mods: none` | “No mods are installed. EmuWiz will not change the game just by browsing.” |
| Saves | `Snapshots: 2` / `Saves: found` | “No local save snapshots were found.” |
| Manuals | `Manual: available` | “No local manual or guide is linked.” |
| Conversion | `Conversion available` only when proven | “No supported conversion is available for this media.” |

“None” is neutral, not an error. Optional features must never create an unfinished-looking required checklist.

## ADVANCED CONTENT

Expandable advanced details retain:

- hashes, product/revision/region fields, and identity evidence;
- DAT/provider provenance and conflicts;
- exact source/archive paths and media topology;
- emulator executable/profile/core identifiers and versions;
- firmware identity and evidence source;
- launch-plan/candidate warnings and technical blocker details;
- transaction IDs, journal paths, and recovery state;
- specialist optical/tape/archive fields.

Do not delete technical evidence to simplify the novice view. Relocate it behind consistent disclosure.

## PLAY / READINESS

The readiness card is the operational top section, based on the separate readiness specification. It consumes the existing launch input/plan and reports:

```text
READY TO PLAY
Using: DuckStation
Identity: Verified · Firmware: Not required · Game: Available
[Play]
```

or:

```text
NEEDS ATTENTION
This system needs firmware before it can start.
Using: PCSX2 · Identity: Verified
[Check BIOS / Firmware]
```

The card must distinguish hard blockers from warnings and expose one primary action. The full launch-readiness panel remains available under explanation/advanced details. The launch planner and final adapter preflight remain authoritative.

## EMULATOR SELECTION

Show one selected/preferred candidate:

```text
Using: RetroArch · Beetle PSX HW
[Play] [Change emulator]
```

Rules:

- remembered user selection remains authoritative;
- a sole eligible candidate may be shown automatically;
- multiple safe candidates require explicit selection where current policy requires it;
- no silent substitution after a profile or emulator change;
- exact executable path, profile ID, and core `library_name` belong in Advanced Details;
- changing emulator invalidates the displayed readiness projection and recomputes it.

## FIRMWARE

Firmware is a compact contextual row, not a global catalogue dump:

- **Firmware: Not required**;
- **Firmware: Ready**;
- **Firmware: PS2 BIOS needed**;
- **Firmware: Unknown / check required**;
- **Firmware: Mismatch**.

Display it only when the selected game/candidate makes it relevant. The source remains existing candidate firmware evidence and launch checks. A global BIOS catalogue state must not be promoted to per-game Ready without the existing evidence path.

Primary action for a hard firmware issue: **Check BIOS / Firmware**. EmuWiz must not offer copyrighted firmware downloads.

## IDENTITY

Identity appears as a compact status plus a Review evidence link:

- **Verified**;
- **Strong local identity**;
- **Needs review**;
- **Launchable with warning**;
- **Mismatch / blocked**.

Show a short explanation such as “EmuWiz found matching local evidence” or “This title is plausible, but the exact release is not confirmed.” Advanced Details retain hashes, DAT facts, revision, region, provider evidence, and conflicts.

DAT membership is evidence, not a universal Play prerequisite. Title/filename similarity never becomes Verified by presentation.

## PROBLEMS & REPAIR

Game Details shows a compact current-problem summary only when a problem affects the selected game:

```text
Problem: The source folder is unavailable
Play impact: Cannot verify launch right now
Change: No files will change by reviewing this problem
Recovery: Review only / repair is reversible where stated
[Review problem]
```

The inline summary must answer:

1. what is wrong;
2. whether Play is affected;
3. what EmuWiz would change;
4. whether it is reversible;
5. what happens if ignored.

The button opens the existing Problems & Repair route with GameId context. Do not embed the full repair planner in Game Details.

## CHEATS / MODS

Show compact status and link to game-specific workflows:

- `Cheats: 3 available` or `Cheats: none`;
- `Mods: 1 installed` or `Mods: none`;
- identity/readiness warning if a selected cheat/mod cannot safely apply;
- **Open Cheats & Mods** as the child action.

The selected GameId must remain bound to the child workflow. Existing preview, conflict, transaction, and undo truth remains in the Mods/Cheats system. Do not show “installed” as equivalent to “enabled in emulator”.

## SAVES

Keep the current Saves & Backups section near secondary tools:

- snapshot count and latest status;
- save portability/binding summary when available;
- **Open Saves & States**;
- restore/compare only when existing policy makes them available;
- no implication that browsing creates a backup.

If a restore/apply action has no safe transaction, show that it is preview-only or unavailable. Do not invent per-game undo.

## MANUALS

Keep local Manuals & Guides as a compact collapsible section. Show availability, local association reason, format/page information, and resume page when present. Empty state: “No local manual or guide is linked. This does not affect launch.”

Manual discovery must remain local/read-only according to the current implementation. A missing manual is never a health blocker.

## CONVERSION

Conversion belongs on Game Details only as a contextual action when the selected media/platform has a proven supported conversion or planning workflow.

Examples:

- Convert to CHD for supported optical media;
- WUD/WUX planning for Wii U when the representation is understood;
- safe conversion planning for supported formats;
- Saturn rebuild/readiness/proof surfaces where applicable.

Do not show a generic Convert button for unsupported media. The contextual card should state whether the action is inspection, planning, or an actual reviewed conversion. It must link to the existing Converter route with the GameId/media context and preserve source immutability and preview requirements.

## MULTI-DISC

When the existing identity/topology model groups media, show:

```text
Disc 1 of 3 · current selection
Disc 2 · available
Disc 3 · needs attention
```

Include:

- grouped title/release identity;
- current disc and available disc set;
- current launch composition;
- missing/unreadable/problem disc;
- disc-specific evidence where relevant.

Do not turn grouped discs into unrelated games or silently substitute Disc 1 for another disc. If the current model cannot prove grouping, show separate media and an advanced uncertainty note instead.

## HISTORY / UNDO

Add a compact **Recent changes** projection when existing history records can be joined safely to the selected GameId/path/operation:

```text
Recent changes
• Cheat installed · review available
• Mod applied · Undo available
• Rename/repair completed · Undo unavailable
[View game history]
```

Rules:

- read existing Activity/History, mod receipts, repair journals, and workflow receipts;
- do not synthesize a history record for an unjournaled action;
- show **Undo available** only when existing transaction policy says rollback is safe;
- show **Undo unavailable — needs review** when appropriate;
- link **View history** to the existing global History page filtered/contextualized by GameId where supported;
- retain global history as the authoritative audit record.

If no records exist: “No recorded changes for this game.” This is neutral, not an error.

## RETURN CONTEXT

Use a narrow contextual carrier, not duplicate authoritative state:

```rust
struct GameContext {
    game_id: GameId,
    return_route: Route,
    selected_disc: Option<DiscId>,
    selected_emulator: Option<EmulatorProfileId>,
    source_filter_context: Option<SourceFilterContext>,
}
```

The carrier is navigation context only. Readiness, identity, profile, source, and transaction facts are re-read from their existing owners.

| Child action | Current behavior | Hub specification |
|---|---|---|
| Verify | Game Details opens Check with platform context rather than a consistently game-scoped child route | Preserve GameId and return to the same Game Details; keep collection verification available. |
| Problems | Game-specific action exists and opens Problems | Return to Game Details and refresh the compact problem/readiness summary. |
| Emulator Setup | Global setup/readiness surface | Open with GameId/required platform context; return to Game Details. |
| Firmware | Global BIOS/Firmware/Doctor surface | Open with the selected candidate requirement highlighted where supported; return to Game Details. |
| Cheats | Game-specific Mods/Cheats task route | Preserve GameId and selection; return to Game Details. |
| Mods | Same game-specific task route | Preserve GameId and selection; return to Game Details. |
| Saves | Current action opens global Saves & States | Pass GameId as context/filter and return to the same game. |
| Manuals | Currently inline | Keep inline; document open/resume actions should return to Game Details. |
| Conversion | Mostly global/specialist | Open only supported contextual conversion with GameId/media binding; return to Game Details. |
| History | Global/workflow-specific | Add GameId-filtered projection/link, then return to Game Details. |

After every child flow, recompute current state. Never restore a stale copied status just because it was true before navigation.

## EMPTY STATES

| Empty area | Recommended copy |
|---|---|
| Artwork | “No artwork has been linked yet. This does not affect play.” |
| Manual | “No local manual or guide is linked.” |
| Cheats | “No cheats are available for this game.” |
| Mods | “No mods are installed.” |
| Saves | “No local save snapshots were found.” |
| Problems | “No current problems are recorded for this game.” |
| Conversion | “No supported conversion is available for this media.” |
| History | “No recorded changes for this game.” |
| Multi-disc | “This game has no proven grouped disc set.” |

Empty states should not imply failure, required setup, or missing online access.

## MR WIZ

Mr Wiz should explain only contextual uncertainty or action:

- blocked launch: why it is blocked and the safest next step;
- uncertain identity: why exact release evidence matters;
- missing firmware: why the selected system needs it and that EmuWiz does not provide it;
- repair: what would change, reversibility, and what happens if ignored;
- conversion: whether the action is inspection, planning, or a preservation-sensitive change;
- multi-disc: why disc grouping/current disc matters.

Mr Wiz should not narrate obvious chips such as “Artwork available” or “Manual available”.

## CONTROLLER FUTURE

Recommended focus order:

1. readiness card / Play;
2. Change emulator or Review blocker;
3. Identity/Verify;
4. primary game tools: Cheats/Mods, Saves, Manuals, Artwork;
5. Health/Repair;
6. conversion/disc controls when relevant;
7. Recent changes;
8. Advanced Details.

Child screens must accept the same GameContext and provide a predictable Back to Game Details action. Dense hashes, path fields, repair plans, and provider setup remain expert surfaces.

## IMPLEMENTATION SEAMS

1. **GameContext carrier:** navigation-only GameId, return route, disc/profile selection, and filter context.
2. **Section projection models:** compact read-only status for readiness, identity, optional tools, health, conversion, disc set, and recent changes.
3. **Existing-route action mapping:** map cards to current Verify, Problems, Emulator Setup, Firmware, Cheats/Mods, Saves, Converter, and History routes.
4. **Readiness integration:** consume the existing per-game readiness summary/launch input; do not calculate readiness in the hub.
5. **Contextual availability:** determine whether a section is shown from existing media/platform support and current evidence.
6. **History-by-game projection:** join existing typed receipts/transactions only where their GameId/path association is authoritative; otherwise link to global history without inventing a match.
7. **Post-child refresh:** invalidate/recompute projections after returning from a child flow, profile change, source change, or launch result.

## MVP

1. Keep `Route::Game(game_id)` as the only permanent game-management surface.
2. Move/summary-project launch readiness near title and Play.
3. Add compact identity, source/media, firmware, and problem chips with primary actions.
4. Preserve existing inline Artwork, Saves, and Manuals sections.
5. Keep Cheats/Mods, Verify, Problems, and Emulator/Firmware setup as game-context child routes.
6. Add contextual Conversion and multi-disc cards only when supported.
7. Add a read-only Recent changes section from existing history sources.
8. Add return-context and stale-recompute tests.

## NON-GOALS

- no second Game Details page;
- no new identity, readiness, firmware, emulator, conversion, disc, or transaction truth;
- no universal inline rendering of every specialist tool;
- no automatic emulator/profile switching;
- no automatic firmware download;
- no optional artwork/provider/cheat/mod/manual requirement for Play;
- no new transaction or undo semantics;
- no ROM/media mutation;
- no controller implementation;
- no packaging, release, installer, or concurrent reconciliation changes.

## IMPLEMENTATION ORDER

1. Define the navigation-only GameContext and return behavior.
2. Place the compact readiness summary beside title/Play.
3. Add source/media, identity, firmware, and current-problem projections.
4. Convert existing game actions to context-preserving child routes.
5. Add compact optional-tool status and neutral empty states.
6. Add contextual conversion and multi-disc projections using existing support only.
7. Add read-only GameId history projection and safe undo links.
8. Add stale-state invalidation after child actions/profile/source changes.
9. Add route, projection, and return-context tests, then review controller focus order.

## SPECIFICATION DECISION

Game Details should become the stable game-centric hub by presenting operational readiness first, contextual identity/health second, optional tools third, and technical evidence last. Global pages remain the owners of collection-wide setup and administration; Game Details owns the user’s context and the route back to the selected game.
