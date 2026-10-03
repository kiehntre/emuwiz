# EmuWiz GUI Information Architecture / Feature-Family Specification

> **Status (2026-10-03): historical design input, preserved for rationale.**
> This specification was written on 2026-09-27 as research. The feature-family
> navigation shell it recommends landed the same day (`128afc7b`), and later GUI
> v2 work built on it. Much of it is therefore implemented; do not read it as a
> proposal. **Current code is authoritative**; routes and
> labels are in `crates/archivefs-gui/src/gui_v2/routes.rs`.
>
> Checked against main (`d709fbcf`):
>
> - **Implemented:** the twelve feature families (DATs & Verification, Cheats &
>   Mods, Saves & States, Emulators, MAME, Artwork & Extras, Conversion,
>   Organisation, Problems & Repair, Sources & Providers, History & Undo,
>   Advanced / Diagnostics) exist as `FeatureFamily`, each with a canonical-home
>   mapping (`family_for_route`), a "... overview" page in a sidebar `FAMILIES`
>   group, and Easy / Advanced action variants (`FamilyVariant`). Breadcrumbs
>   show the owning family. The top bar has Back, Home, a breadcrumb and a
>   "Jump to..." menu.
> - **Different from this document:** Platforms, Duplicates, Multi-disc games,
>   Storage and RomM Library are their own sidebar pages (this document placed
>   them under Games, Organisation, Advanced and Sources). Manuals are under
>   "Artwork, Manuals & Extras". Family names read "Sources & Providers" and
>   "History & Undo" where this document writes "Sources / Providers" and
>   "History / Undo".
> - **Not found on main:** a global "Open global [family]" escape on
>   game-scoped pages, and the full menu-depth and discoverability tests as
>   automated checks. Treat them as open ideas, not facts.
>
> The sections "Migration from current GUI" and "Implementation phases" describe
> the GUI as it was on 2026-09-27; they are kept for their reasoning, not as
> current routes or a plan. The family rationale (one canonical home per
> capability, Easy and Advanced as two depths over one backend plan, and
> specialist tools staying directly reachable) remains useful for UX review.


Research specification only. This document does not change navigation, widget IDs, page implementations, backend behaviour, or production GUI code.

## DESIGN PRINCIPLES

EmuWiz should have a stable structure with multiple obvious entry points. The structure should expose feature families, not conceal capability behind a small set of vague buckets.

1. Every major capability has exactly one canonical home.
2. Related tasks are adjacent in that home. Contextual shortcuts may point to the same destination, but do not create a second owner.
3. Easy and Advanced are presentation depths over the same backend plan, identity, provenance, preview, transaction and undo mechanisms.
4. Specialist tools remain directly addressable. “Advanced” is a detail level and diagnostics destination, not a hiding place for important features.
5. Game Details is the selected-game hub, never the only way to reach a feature.
6. A global directory, search/jump command and breadcrumbs make route knowledge optional.
7. The navigation shell owns location. Pages own their existing workflows and safety semantics.
8. Mutating actions show scope, destination, preview and recovery before confirmation.
9. A global inbox can aggregate problems, but each problem retains a canonical family owner.
10. There is one authoritative operation/history store; family pages and Game Details show filtered projections.

The current GUI-v2 route registry is a useful substrate: it already contains direct routes such as Saves & States, Check Games, Problems & Repair, Organisation, Converter, Emulator Setup, BIOS / Firmware, Mods & Cheats, Artwork & Metadata, Sources, DAT Management, Activity, History and Advanced. The proposed IA keeps that directness while making family ownership more explicit.

## FEATURE FAMILIES

The proposed primary directory has twelve visible families plus Home and Games. The names are user-facing and intentionally specific:

| Family | Owns | Does not own |
|---|---|---|
| DATs & Verification | DAT sources, checks, identity, trusted rename, DAT-based repair | generic organisation, MAME set reconstruction |
| Cheats & Mods | cheats, patches, mods, availability, conflicts, compatibility | saves, emulator configuration |
| Saves & States | native saves, save states, snapshots, memory cards, restore | cheat/mod rollback |
| Emulators | installations, profiles, setup, BIOS handoff, launch tests | DAT identity |
| MAME | arcade set health, set naming, dependencies, merged reconstruction | generic ROM rename |
| Artwork & Extras | artwork, metadata, screenshots, bezels, manuals and guides | provider connection ownership |
| Conversion | format conversion, queues, platform-specific converters, conversion history | rename and organisation |
| Organisation | canonical library, duplicates, Playing Library / 1G1R, frontend publication | evidence-based filename identity |
| Problems & Repair | global inbox and repair review | ownership of the underlying issue |
| Sources / Providers | local folders, DAT feeds, metadata providers, RomM and remote health | the actions performed with acquired data |
| History / Undo | global operation history, receipts, rollback and filtered projections | a second per-feature journal |
| Advanced / Diagnostics | technical inspection, mounts, storage, database status, configuration diagnostics | normal user entry points |

Home, Games and Game Details remain stable cross-family surfaces. MAME is visible because its set/dependency model is materially different from normal library naming. Manuals remain visible under Artwork & Extras and from Game Details; they are not hidden in a generic “media” drawer.

## DATs & VERIFICATION

Canonical landing page: **DATs & Verification**.

Visible tasks:

- Manage DATs: local files, folders, managed DAT sources and update status.
- Check Games: unknown, missing, damaged, mismatched and verified results.
- Easy Rename: “Rename verified games to trusted DAT names.”
- Advanced Rename: collision strategy, destination/output mode, specialist filters and transaction detail.
- Repair from DAT evidence: preview repairs whose evidence is supplied by a selected DAT.
- Verification details: hashes, match basis, DAT identity, source and revision/provenance.
- History / Undo: a filtered projection of the authoritative operation history.

Easy Rename and Advanced Rename are two presentations of the same rename-plan backend. Easy Rename selects safe defaults, scopes the operation visibly and always previews. Advanced Rename adds collision handling, output policy, transaction controls, hashes and provenance. Neither path invents a separate rename engine.

“Evidence”, “projection” and “loadability” may remain in technical details, but primary labels should say “DAT match”, “What will change”, “Can this game be played?” and “Why this result?”.

## CHEATS & MODS

Canonical landing page: **Cheats & Mods**. The global page is explicitly global: “Choose a game” is required before game-specific actions, and no unrelated game’s history is shown as if it belonged to the current selection.

Visible tasks:

- Cheats: browse, import, preview, install and undo.
- Mods: browse, install, package-specific options and undo.
- Installed: what is active for the selected game or across the library.
- Available: provider-backed or local candidates, with trust and compatibility status.
- Conflicts: overlapping files, incompatible variants and resolution choices.
- Compatibility: emulator/platform/game identity constraints.
- Cheat sources and provider updates: source configuration and refresh status.
- History / Undo: global history filtered to cheat/mod transactions.

Game Details links to Cheats & Mods with a `GameId` context. It must be clear whether the page is global or selected-game mode. The existing Cheats & Mods workflow, cheat sources, RetroArch catalogue path and adapter-specific mod pages are projections into this family, not separate top-level families.

## SAVES & STATES

Canonical landing page: **Saves & States**.

The page must keep these nouns distinct in both labels and data presentation:

- **Native save** — the emulator/game’s ordinary save data.
- **Save state** — an emulator-created execution snapshot, often emulator/version dependent.
- **Memory card** — a card/container with its own filesystem or emulator ownership.
- **Snapshot** — an EmuWiz or source snapshot of a file/container for recovery or portability.

Visible tasks are Saves, Save States, Snapshots, Memory Cards, Restore and History. Restore is a first-class action with a preview of source, destination, overwrite policy and recovery path. Game Details opens the same page with `GameId` context; it must not create a selected-game-only save store.

## EMULATORS

Canonical landing page: **Emulators**.

Visible tasks:

- Installed / Discovered.
- Setup and profiles.
- BIOS / Firmware handoff (with a direct cross-link to the Firmware family).
- Test Launch.
- Managed downloads where supported.
- Diagnostics and readiness.

Required status vocabulary:

| Status | Meaning |
|---|---|
| Ready | An installation/profile satisfies the current check. |
| Not found automatically | No supported installation was discovered; the user may provide a path. |
| Profile needs setup | An emulator exists, but a usable profile is incomplete. |
| Multiple installations | More than one candidate exists and selection is unresolved. |
| Externally managed | EmuWiz can observe it but must not adopt or mutate its ownership. |
| Unsupported | The installation or requested operation is outside the supported contract. |

“Emulator Manager”, “Doctor” and “Setup & Readiness” may remain implementation names in diagnostics, but the primary route is Emulators.

## MAME

Canonical landing page: **MAME**. This is a first-class family, not a tab hidden under generic DAT rename.

Visible tasks:

- Collection Health.
- Rename / Set Names.
- Repair Missing Members.
- Dependencies and BIOS/device relationships.
- Reconstruct Merged Sets.
- Verify.
- Playing Library.
- Cheats.
- History / Undo.

MAME repair must speak in set/member/dependency terms. Generic “rename game” must not silently route a user into MAME reconstruction. MAME DAT identity can link to DATs & Verification, but the canonical repair destination is MAME because the user’s mental model is a set, not an individual filename.

## ARTWORK & EXTRAS

Canonical landing page: **Artwork & Extras**.

Visible tasks: Artwork, Metadata, Screenshots, Bezels / Decorations, Manuals / Guides and provider settings relevant to those assets. Manuals remain a visible task because “find the manual” is a direct user intent and current Game Details already exposes a Manual action. Platform artwork, ROMM artwork, ScreenScraper enrichment, Cemu graphic packs, Dolphin/PPSSPP textures and bezel workflows link here according to asset type.

Provider credentials and refresh policy are owned by Sources / Providers; the asset workflow remains owned here. Technical cache paths and fetch timings belong in Advanced details.

## CONVERSION

Canonical landing page: **Conversion**.

Visible tasks: Easy Conversion, Advanced Conversion, Queue, platform-specific tools and History. Easy Conversion uses a named safe recipe, for example “Create a CHD copy and keep the original.” It shows source, destination, free-space expectation, verification and whether the original remains untouched. Advanced Conversion exposes tool/backend details, destination policy, verification policy, queue ordering and specialist options.

Current optical Disc Conversion, ZIP Converter, conversion queue, Wii U conversion planners, xdelta/PPF/patching and tape-specific conversion/inspection are linked from this family where they are conversion actions. Tape Inspector remains a specialist inspection route under Advanced / Diagnostics with a prominent Conversion link when an actionable conversion exists.

## ORGANISATION

Canonical landing page: **Organisation**.

Visible tasks: Easy Organiser, Advanced Organiser, Playing Library / 1G1R, RomM / ES-DE / RetroDECK publication, duplicate handling and History / Undo. Easy Organiser uses a safe preview and conservative defaults. Advanced Organiser exposes region/language/revision preferences, destination policy, duplicate policy and transaction detail.

DAT evidence used to decide a name remains owned by DATs & Verification. The organisation page may offer a “Use DAT-verified names” shortcut, but it routes to the same DAT-backed plan and does not claim ownership of identity evidence.

## PROBLEMS & REPAIR

Problems & Repair is both a visible family and a cross-cutting inbox.

The inbox aggregates actionable findings with severity, affected game/system, owning family, explanation and a direct “Open in …” action. It never becomes a second repair implementation.

Canonical resolution map:

| Problem | Canonical resolution |
|---|---|
| DAT mismatch, unknown identity, unsafe name | DATs & Verification |
| MAME missing member, dependency or merged-set issue | MAME |
| Emulator missing/profile/launch readiness | Emulators |
| Missing BIOS or firmware | Emulators → Firmware |
| Conversion failure or queue issue | Conversion |
| Organisation collision or duplicate choice | Organisation |
| Cheat/mod conflict | Cheats & Mods |
| Save restore conflict | Saves & States |
| Missing artwork/manual/provider asset | Artwork & Extras or Sources / Providers, depending on cause |
| Mount/storage/database/configuration issue | Advanced / Diagnostics |

Repair Review and Repair History stay reachable from the inbox, but History / Undo remains the authoritative transaction view. “Repair” describes the safety workflow; it does not erase feature-family ownership.

## SOURCES / PROVIDERS

Canonical landing page: **Sources / Providers**.

The page distinguishes:

- Local sources: game folders, archive roots and inclusion/exclusion policy.
- DAT sources: local, managed and remote DAT catalogues.
- Metadata providers: ScreenScraper, LaunchBox/local metadata and other configured sources.
- RomM: connection, cached library, browse and import preview.
- Remote health: authentication, connectivity, cache age, snapshot status and retry.

Provider configuration is a source concern; using the resulting data is owned by DATs & Verification, Artwork & Extras, Cheats & Mods or Games as appropriate. RomM may have a direct directory entry because users think “I need RomM”, but its connection remains canonical here and its library browsing can be a contextual projection.

## HISTORY / UNDO

There is one authoritative history system for operations and receipts. It records operation kind, affected scope, source/destination, provenance, preview/confirmation, result, reversibility and rollback status.

Reachability:

- Global: History / Undo in the primary directory.
- Family: each family shows a filtered History / Undo projection.
- Game Details: selected-game history projection, only for that `GameId`.
- Problems / Repair: links to the same receipt and recovery action.

These are projections, not duplicate stores. Library View History, recent activity, repair history, conversion history and adapter-specific journals must either identify themselves as projections of the authoritative system or remain explicitly separate when their semantics truly differ (for example, a read-only view-history audit). The UI must say which is which.

## ADVANCED / DIAGNOSTICS

Canonical landing page: **Advanced / Diagnostics**.

It contains technical inspection that is useful to specialists but is not the only route to important capability: Archive Inspector, mounts and active mounts, storage health, database status, configuration diagnostics, doctor checks, raw provider/DAT provenance, parser/tool versions and detailed logs.

Advanced is not where DATs, cheats, saves, firmware, MAME, conversion or manuals disappear. Each remains directly reachable in its family; Advanced adds deeper inspection of the same operation and state.

## GAME DETAILS RELATIONSHIP

Game Details remains the stable selected-game hub. It presents identity, platform, readiness and the next safe action, then links into families using `GameId` context:

- Cheats & Mods.
- Saves & States.
- Manuals / Guides in Artwork & Extras.
- Problems & Repair filtered to this game.
- Conversion.
- History / Undo filtered to this game.
- DAT verification details.
- Emulator and firmware readiness.

Each link is a shortcut to the canonical route. The destination header must show the selected game context and offer “Open global [family]” so the user can switch from one game to library-wide work without losing orientation.

## GLOBAL DIRECT ACCESS

The sidebar or main menu is a full directory, not a sparse set of buckets. It should expose the primary families directly. A global command/search/jump control searches family names, task names, aliases and recent destinations. Examples: “DATs”, “firmware”, “cheats”, “MAME”, “undo” and “manual” must each produce an immediate destination.

The Home page may show recent/frequent destinations and recommended tasks, but it must not be the only place where a feature can be found. Breadcrumbs show `EmuWiz / DATs & Verification / Easy Rename` and retain the selected-game context when present.

Common intents are one deliberate selection from the global directory; a family landing page to a task is the second click. Search/jump may be one action.

## TOP BAR CONTRACT

The future mouse-first top bar must provide:

- Back, disabled when no route history exists.
- Home.
- Current location and breadcrumb, clickable at each valid ancestor.
- Global search/jump.
- Direct family access, preferably a compact directory/menu rather than duplicated sidebar content.
- Selected-game context when relevant, with a clear “global” escape.

It must not duplicate the entire sidebar as a second competing navigation system. The sidebar remains the directory; the top bar is orientation, history and fast jump.

## EASY / ADVANCED CONTRACT

Easy is appropriate when a safe, common recipe can be named in user terms: verified rename, keep-original conversion, restore a selected save, install a compatible mod or publish a reviewed library. Easy mode:

- uses safest defaults;
- presents one clear primary action;
- avoids backend jargon;
- shows scope, source, destination and expected change;
- previews before mutation;
- never weakens confirmation, rollback or error reporting.

Advanced is appropriate when the user must choose among valid technical policies. It exposes source/destination details, collision strategy, hashes/provenance, transaction options, queue controls, tool/backend selection and specialist filters.

Both modes build the same typed backend plan and use the same preview, executor, receipt and undo mechanisms. Easy is not a second implementation and Advanced is not a bypass around safety.

## CANONICAL HOME TABLE

| Feature | Canonical home | Contextual shortcuts | Easy path | Advanced path |
|---|---|---|---|---|
| Manage DATs | DATs & Verification | Home, Sources / Providers, onboarding | Add/select trusted DAT | source precedence, revisions, parser/provenance |
| Check Games | DATs & Verification | Games, Problems inbox, Game Details | Check my games | filters, match basis, hashes, batch scope |
| Quick / Easy Rename | DATs & Verification | Organisation, Check Games, Game Details | verified names + safe defaults | same rename plan with policies |
| Advanced Rename | DATs & Verification | Easy Rename details, Organisation | n/a | collisions, outputs, transaction options |
| Repair from DAT evidence | DATs & Verification | Problems inbox, Check Games | preview safe repair | evidence selection and transaction detail |
| Cheats | Cheats & Mods | Game Details, Home | choose game → install compatible cheat | source, reconciliation and conflict controls |
| Mods | Cheats & Mods | Game Details | choose game → preview install | package/filesystem/provider details |
| Saves | Saves & States | Game Details | browse/restore native save | portability and overwrite policy |
| Save states | Saves & States | Game Details | choose state → restore preview | emulator/version and transaction details |
| Memory cards | Saves & States | Game Details, Emulators | safe card backup/restore | filesystem/container details |
| Snapshots | Saves & States | History / Undo | restore named snapshot | provenance and retention policy |
| Emulator setup | Emulators | Game Details, Problems, Home | fix next readiness issue | profiles, paths and diagnostics |
| BIOS / firmware | Emulators → Firmware | Game Details, Problems | set up required firmware | projection, hashes and target policy |
| MAME repair | MAME | Problems, DATs & Verification | repair missing members preview | set/dependency/merged-set controls |
| Artwork | Artwork & Extras | Game Details, Games | fetch/apply safe artwork | provider, cache and asset policy |
| Manuals | Artwork & Extras | Game Details | Open Manual | provider/provenance and cache detail |
| Conversion | Conversion | Game Details, Problems | named keep-original recipe | backend, destination, queue and verification |
| Duplicates | Organisation | Games, Problems | review exact duplicates | canonical choice and quarantine policy |
| Playing Library / 1G1R | Organisation | Games, Home | preview publish | region/language/revision rules |
| RomM / ES-DE / RetroDECK publication | Organisation | Sources / Providers, Home | preview publication | profile, destination and rollback details |
| Problems inbox | Problems & Repair | Home, family badges, Game Details | open next issue | cross-family diagnostics and receipts |
| Sources | Sources / Providers | Home, onboarding | add game folder/provider | precedence, cache and remote health |
| Undo a change | History / Undo | every family, Game Details | undo latest reversible operation | receipt, reverify and rollback diagnostics |
| Advanced diagnostics | Advanced / Diagnostics | Problems, family technical details | n/a | mounts, storage, database, raw logs |

## CONTEXTUAL SHORTCUTS

Shortcuts are allowed when they answer the user’s current context, but they must state the canonical destination. Examples: “Rename with DATs” in Organisation; “Check this game” in Game Details; “Install cheat” beside a selected game; “Open firmware setup” in launch readiness; “View conversion history” after a conversion; “Resolve in MAME” from a MAME problem.

Shortcut labels should use verbs and destination names, for example “Open DATs & Verification → Easy Rename”, not “Projection” or “Evidence”. A shortcut must not create a second selected state, history store or mutation path.

## MENU DEPTH RULES

- Common task: at most two deliberate clicks from global navigation.
- Specialist task: at most three deliberate clicks.
- Search/jump: one action to the destination, with optional second action to choose a task.
- No important feature requires guessing that it lives under Advanced.
- A family landing page may use tabs or task cards, but the task names remain visible and searchable.
- Contextual entry points must preserve breadcrumbs and selected-game context.

## NAMING RULES

Use the user’s noun or intent first: DATs, Verification, Cheats, Mods, Saves, Save States, Firmware, MAME, Artwork, Manuals, Conversion, Organisation, History and Undo.

Avoid primary labels such as Evidence, Projection, Loadability, Provider precedence, Catalogue, Doctor, Repair Center and Museum when a clearer user-facing term exists. Technical terms can remain in Advanced details, with a plain-language summary first. Use consistent title case and do not alternate between “Cheats & Mods” and “Mods & Cheats” for the same destination.

## DISCOVERABILITY TEST

| Intent | Natural destination | Result |
|---|---|---|
| rename games | DATs & Verification → Easy Rename | unambiguous |
| fix wrong names | DATs & Verification → Check Games / Repair | unambiguous |
| repair MAME | MAME → Collection Health / Repair | unambiguous |
| install cheat | Cheats & Mods → Cheats | unambiguous |
| install mod | Cheats & Mods → Mods | unambiguous |
| restore save | Saves & States → Restore | unambiguous |
| set BIOS | Emulators → Firmware | unambiguous |
| configure emulator | Emulators → Setup | unambiguous |
| convert game | Conversion → Easy Conversion | unambiguous |
| find manual | Artwork & Extras → Manuals / Guides | unambiguous |
| change artwork | Artwork & Extras → Artwork | unambiguous |
| update DAT | DATs & Verification → Manage DATs, with Sources link | unambiguous |
| check unknown games | DATs & Verification → Check Games | unambiguous |
| undo a change | History / Undo | unambiguous |
| publish Playing Library | Organisation → Playing Library / 1G1R | unambiguous |
| inspect advanced diagnostics | Advanced / Diagnostics | unambiguous |

Current architecture failures found in the repository are naming and ownership failures, not missing backend capability: the legacy sidebar puts DATs under Library, Quick Rename under Organise, saves under an Enhance overlay, and mounts/history under Health; GUI-v2 has direct routes but still separates Dat, Mods, Artwork, RomM and Sources in a way that can obscure which page owns configuration versus use. GUI-v2 also currently maps the Artwork handoff to a legacy Settings destination, which is a route-quality defect to resolve during migration. The `Advanced` route has also historically meant DATs and is migrated to DAT in the current route code; that ambiguity must not return.

## PROPOSED NAVIGATION MAP

```text
EmuWiz
├── Home
├── Games
│   ├── My Games
│   ├── Platforms
│   ├── Ready to Play
│   └── Game Details
├── DATs & Verification
│   ├── Check Games
│   ├── Easy Rename
│   ├── Advanced Rename
│   ├── Repair from DAT evidence
│   ├── Verification details
│   ├── Manage DATs
│   └── History / Undo
├── Cheats & Mods
│   ├── Cheats
│   ├── Mods
│   ├── Installed
│   ├── Available
│   ├── Conflicts
│   ├── Compatibility
│   ├── Cheat sources
│   └── History / Undo
├── Saves & States
│   ├── Saves
│   ├── Save States
│   ├── Snapshots
│   ├── Memory Cards
│   ├── Restore
│   └── History / Undo
├── Emulators
│   ├── Installed / Discovered
│   ├── Setup & Profiles
│   ├── Test Launch
│   ├── BIOS / Firmware
│   ├── Managed Downloads
│   └── Diagnostics
├── MAME
│   ├── Collection Health
│   ├── Set Names
│   ├── Repair Missing Members
│   ├── Dependencies
│   ├── Reconstruct Merged Sets
│   ├── Verify
│   ├── Playing Library
│   ├── Cheats
│   └── History / Undo
├── Artwork & Extras
│   ├── Artwork
│   ├── Metadata
│   ├── Screenshots
│   ├── Bezels / Decorations
│   └── Manuals / Guides
├── Conversion
│   ├── Easy Conversion
│   ├── Advanced Conversion
│   ├── Queue
│   ├── Platform Tools
│   └── History / Undo
├── Organisation
│   ├── Easy Organiser
│   ├── Advanced Organiser
│   ├── Playing Library / 1G1R
│   ├── RomM / ES-DE / RetroDECK publication
│   ├── Duplicates
│   └── History / Undo
├── Problems & Repair
│   ├── All Problems
│   ├── Needs Attention
│   ├── Repair Review
│   └── Open in owning family
├── Sources / Providers
│   ├── Local Sources
│   ├── DAT Sources
│   ├── Metadata Providers
│   ├── RomM
│   └── Remote Health
├── History / Undo
│   ├── All Operations
│   ├── Reversible Changes
│   └── Filter by Family / Game
└── Advanced / Diagnostics
    ├── Archive Inspector
    ├── Mounts
    ├── Storage Health
    ├── Database Status
    ├── Configuration Diagnostics
    ├── Doctor Checks
    └── Technical Logs
```

## MIGRATION FROM CURRENT GUI

*Historical snapshot (2026-09-27). Route names below describe the GUI of that date, not current main.*

The classifications below describe route ownership, not deletion of functionality.

| Current route / label | Classification | Proposed home |
|---|---|---|
| GUI-v2 `Home` | KEEP | Home |
| GUI-v2 `Games` / legacy `Library` | KEEP | Games |
| GUI-v2 `Platforms` | MOVE | Games → Platforms |
| GUI-v2 `Launch` / `Ready to Play` | SHORTCUT | Games and Emulators; canonical action remains Game Details / launch readiness |
| GUI-v2 `Saves` | RENAME | Saves & States |
| GUI-v2 `Duplicates` | MOVE | Organisation → Duplicates |
| GUI-v2 `Check` / legacy `Check Games` | MOVE | DATs & Verification → Check Games |
| GUI-v2 `Dat` / legacy `DatSources` | RENAME | DATs & Verification → Manage DATs |
| legacy `IdentifyRename` / `Quick Rename` | MOVE | DATs & Verification → Easy Rename |
| legacy advanced rename/repair workflow | KEEP | DATs & Verification → Advanced Rename / Repair |
| GUI-v2 `Mods` / legacy `CheatsMods` | RENAME | Cheats & Mods |
| legacy `CheatSources` | MERGE | Cheats & Mods → Cheat Sources, with Sources / Providers shortcut |
| GUI-v2 `Emulators` / legacy `EmulatorSetup` | KEEP | Emulators |
| legacy `EmulatorInventory` | MERGE | Emulators → Installed / Discovered |
| GUI-v2 `Firmware` / legacy `BiosProjection` | MOVE | Emulators → BIOS / Firmware |
| GUI-v2 MAME collection-health workflow | MOVE | MAME |
| GUI-v2 `Artwork` | RENAME | Artwork & Extras |
| legacy platform artwork/media managers | MERGE | Artwork & Extras |
| Game Details Manual action | SHORTCUT | Artwork & Extras → Manuals / Guides |
| GUI-v2 `Converter` / legacy `DiscConversion` | RENAME | Conversion |
| `ZipConverter`, conversion queue and platform converters | MERGE | Conversion → Platform Tools / Queue |
| GUI-v2 `Build` / legacy `CanonicalOrganisation` | RENAME | Organisation |
| `PublisherProfiles` / Playing Library | MERGE | Organisation → Playing Library / 1G1R |
| `Sources` / `SourcesDiscovery` | RENAME | Sources / Providers |
| `Romm` | SHORTCUT | Sources / Providers; browsing projection may remain directly accessible |
| legacy `Problems`, `NeedsAttention`, `RepairReview` | MERGE | Problems & Repair inbox and review |
| legacy `RepairHistory` | REMOVE DUPLICATE | History / Undo projection |
| GUI-v2 `Activity` | MOVE | History / Undo → Activity projection |
| GUI-v2 `History` / legacy `HistoryLogs` | MERGE | History / Undo |
| legacy `LibraryViewHistory` | MERGE | History / Undo, explicitly labelled as view-history audit |
| legacy `Mount`, `ActiveMounts`, `StorageHealth` | MOVE | Advanced / Diagnostics |
| legacy `Doctor`, diagnostics overlays and database status | MOVE | Advanced / Diagnostics; problem cards remain in Problems |
| GUI-v2 `Tape` / `TapeInspector` | KEEP | Advanced / Diagnostics → Tape Inspector, with Conversion shortcut |
| GUI-v2 `Museum` | KEEP | Games → Museum / collection browsing |
| GUI-v2 `Settings` | KEEP | Settings |
| GUI-v2 `Advanced` | RENAME | Advanced / Diagnostics; never use as DAT alias |

Current route labels to keep because they are already clear: Home, Games, Saves & States, Converter/Conversion after the family rename, Emulators, BIOS / Firmware, Problems & Repair, Artwork & Metadata as a transitional alias, Sources, History and DAT Management as a transitional alias. Current labels to avoid as primary labels: Enhance, Organise & Export, Tools, Health & Recovery, Doctor, Catalogue Views, Evidence and Projection.

## IMPLEMENTATION PHASES

*Historical recommendation (2026-09-27). Phases 1 to 3 and parts of 5 and 6 have since been implemented; see the status note at the top.*

Research recommendation, with no implementation performed here:

1. Define stable family and task route types, aliases and `GameId` context. Preserve existing route serialization and widget IDs during the concurrent stability sweep.
2. Add a family menu shell and canonical-home metadata without changing page bodies or backend plans.
3. Add contextual shortcuts that route to canonical destinations and show breadcrumbs/context.
4. Split Easy and Advanced presentations around existing backend plan builders; add preview and safety-contract tests before changing mutation entry points.
5. Add the mouse-first top bar under the contract above; keep it as an orientation/quick-jump layer, not a second sidebar.
6. Add global search/jump over family names, task names, aliases and recent destinations.
7. Migrate legacy routes and persisted/deep-link aliases; verify every old route still reaches a live capability.
8. Remove redundant visual entry points only after their replacements, aliases, tests and contextual shortcuts exist. Do not remove functionality or the authoritative history store.

Each phase should be independently reviewable and should keep GUI-v2 render/widget-ID stability as a hard constraint. No navigation rewrite should be bundled with backend, MAME, DAT, emulator discovery, saves, cheats/mods or release changes.

## RESEARCH STATUS

The proposed model is intentionally more explicit than the current legacy eight-group shell and more family-oriented than the current GUI-v2 flat `Section` list. It preserves direct access to specialist tools while assigning each capability one owner. The principal unresolved implementation decisions are route/type compatibility and the exact MAME sub-workflow surface; neither blocks the IA decision.

GUI INFORMATION ARCHITECTURE SPEC COMPLETE
