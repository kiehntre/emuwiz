# Novice End-to-End UX Audit

Audit date: 2026-09-15  
Audited worktree: `/home/davedap/emuwiz-main-release-fix`  
Starting SHA: `7921a0e0c48b98a4282af30403ca63843174a03e`

## 1. Executive Summary

EmuWiz has a credible safety-oriented front door. Home asks “What would you
like to do?”, first launch can open a five-step tour, Sources explains that a
source is a folder, scans are read-only, and Problems & Repair consolidates
diagnostics and repair review. A user can reach the library, inspect evidence,
configure emulators, and launch several supported platforms.

The novice experience still feels like a preservation toolkit after the first
successful scan. The product often reports a state but does not answer “what
should I do next?” in the same place. The largest gaps are: no single
collection health/next-action dashboard; BIOS and emulator setup are largely
inspection-only; launch blockers do not consistently deep-link to their fix;
arcade authority/dependency results are not surfaced in a player-facing form;
and DAT, save, mod, and cheat workflows retain specialist vocabulary.

The audit found 4 P0, 6 P1, 8 P2, and 4 P3 findings. P0 means a normal novice
journey can stop without a clear route to a playable result. P1 means a major
capability or explanation is missing from the main workflow.

## 2. Current Navigation / Surface Map

The application has two presentation modes: Gamer View and Advanced View.
Advanced View uses a scrollable grouped sidebar. The live navigation groups
are:

| Group | Current destinations |
|---|---|
| Home | Home, Needs Attention |
| Library | Library, Ready-to-Play, Quick Rename, Library Organisation, Publisher / Frontend Library |
| Tools & Workflows | Duplicate Finder, Disc Conversion, Emulator Setup, Emulator Manager, BIOS / Firmware, RomM |
| Mounts | Mounts, Active mounts |
| Cheats & Mods | Cheats & Mods |
| Sources | Sources; this contains Libraries, DATs, Cheats, and Discovery tabs |
| Media | Media Sets |
| History & Journals | History & Logs, Library View History |
| Diagnostics | Problems & Repair, Automatic health report |
| Settings | Settings |

Other live surfaces include Selected/game details, archive inspection,
database status/skipped files, tape inspection, emulator update review,
PCSX2/save-card inspection, cheat-source management, local mod inspection,
RomM, duplicate review, disc conversion, and the read-only BIOS projection.

This is a lot of power, but it is still a tool map rather than a novice task
map. “Sources”, “Media Sets”, “Library View History”, “Publisher / Frontend
Library”, and “Automatic health report” are not obvious first-run concepts.

## 3. First-Run Journey

### Observed path

1. Start the application. On a genuine fresh install, onboarding opens with
   Welcome to EmuWiz.
2. Read the promise that EmuWiz will not silently rename, reorganise, or
   configure an emulator. Continue to Add a source.
3. Choose the existing games folder in Sources → Libraries. The page says a
   source is an existing readable folder and that adding it does not change
   files.
4. Choose Scan now for the source, or Scan all enabled. Wait for the
   background scan.
5. Inspect the last-scan summary. It can show archives, loose ROMs, disc
   images, game folders, unknown items, and skipped items. Discovery has a
   deeper skipped/unknown view.
6. Continue to optional DAT / identification setup. Register or inspect a DAT,
   validate it, save the registry, and optionally audit/identify/rename.
7. Continue to Emulator Setup. Run the emulator/availability checks and then
   separately use Emulator Manager for installed versions and update review.
8. Continue to Verify. If a DAT was added, compare the library against it;
   otherwise the flow honestly says verification can happen later.
9. Browse Library, select an item, inspect evidence, open Ready-to-Play, and
   use the platform-specific launch action.

### Interaction cost

The minimum guided path is approximately 9–13 deliberate clicks plus waiting:
open onboarding/continue through five steps, choose a folder, scan, inspect
results, optionally configure DAT, optionally check emulators, open Library,
select a game, and launch. The user must make at least two specialist
decisions: whether to add a DAT and whether an emulator/profile is configured.
Without onboarding, the user must discover Sources, choose the Libraries tab,
discover scan controls, then find Library and Ready-to-Play in the sidebar.

The journey is safe, but it is not yet a closed “add folder → play” journey.
The user must know that DAT is optional, that BIOS is separate from a game,
that an emulator may be detected but not launch-ready, and that Ready-to-Play
is a read-only report rather than the launch page itself.

## 4. Source & Discovery UX

Sources is one of the stronger surfaces. It says “Choose the existing readable
folder where your games live”, states that EmuWiz will not reorganise files,
shows the source role, enabled state, platform assignment, scan status, last
scan, found-content breakdown, skipped count, and “Inspect skipped”. Discovery
also distinguishes recognised-but-unmatched content from ordinary skipped
files.

Remaining novice issues:

- “Source”, “Source Role”, “mount root”, “catalogue”, and platform assignment
  still require interpretation. The onboarding defines source, but the normal
  page does not consistently define the other terms.
- A games folder, BIOS folder, DAT folder, save folder, and incoming/unsorted
  folder are all paths in the same broad workflow. The UI does not provide a
  simple “This folder contains…” explanation before the user commits to a
  role.
- Role editing/persistence remains a known blocked area because dispatch needs
  shared `crates/archivefs-gui/src/main.rs` ownership. This audit records the
  user impact only: when the inferred role is wrong, correction is not a
  clearly available, durable novice action.
- Scan completion is visible on Sources, but the post-scan next action is not
  unified. A user may see a successful scan and still not know whether to open
  Library, DATs, Needs Attention, or Emulator Setup.

## 5. Library / Ready-to-Play UX

Library provides search/filtering, source filtering, platform assignment,
archive/file paths, mount paths, missing-entry views, duplicate views, recent
finds, and selected-item evidence. This is transparent, but normal library
presentation still exposes archive/catalogue concepts that answer “where is
this object stored?” better than “can I play it?”.

Ready-to-Play presents filters for Ready, Ready with warnings, Needs attention,
Blocked, Unsupported, and Unknown. It explicitly says the view does not rescan,
change settings, or launch. It shows reasons and platform, and has useful
original-control details.

The weakness is actionability. A badge plus a reason is not always a fix. A
novice needs a short primary sentence such as “Play is blocked because the PS2
BIOS is missing” and one button such as “Open BIOS setup”. Current reasons can
remain behind “Why?”/Details; the primary view should avoid making users decode
`Unknown`, evidence, identity, or platform IDs.

## 6. Problems & Repair

Problems & Repair now has Overview, Diagnostics, and Repair / Recovery tabs.
Overview says whether the last diagnostic run was healthy, not checked, or
needs attention; Diagnostics can review findings; Repair / Recovery reviews
plans and history. This is a meaningful improvement over three competing
sidebar destinations.

The loop is still incomplete for several classes:

- Diagnostics can identify an emulator or data problem without always placing
  a “Fix”, “Open setup”, or “Locate” action beside the finding.
- Repair is plan/review oriented. A novice may not know whether “repair” means
  renaming, reorganising, deleting stale catalogue entries, or changing source
  files.
- Refusals are generally honest and technically safe, but the next safe choice
  is not always explicit.
- “Doctor”, “diagnostics”, “repair plan”, “transaction”, “recovery”, and
  “reverify” are internal workflow terms. They belong in Details, not as the
  first explanation.

## 7. BIOS

BIOS / Firmware is discoverable in Advanced View and is also represented in
PCSX2/emulator readiness. The page asks for a “master BIOS root”, performs a
bounded inspection, shows emulator projection plans, match status, target,
method, and warnings, and explicitly says it does not apply or download
anything.

The page is inspection-only. It does not provide a novice completion path to
locate a missing file, organise a verified BIOS, or understand which game or
platform is blocked. “Master BIOS root”, “projection”, “source match”,
“writable emulator state”, and “ROM-set dependency model” are specialist
terms. Licensing language is cautious, but the user is left to acquire and
place firmware manually.

Biggest gap: the backend knows a great deal about verified/candidate/missing
firmware, but the GUI does not turn that into “Platform X needs BIOS Y; locate
an existing copy; review; apply safely” (where such an action is actually
supported).

## 8. Emulator Setup

Emulator Setup/Doctor checks availability and readiness. Emulator Manager
discovers installed installations, versions, channels, install roots,
executables, update capability, EmuWiz preference, warnings, and update status.
It supports review/confirmation, staged updates, verification, and rollback
for supported managed updates.

This is strong evidence and weak onboarding. A novice sees installation roots,
channels, install types, update methods, preferred installations, and “scan
inventory” concepts. Missing emulator handling normally stops at “not found”;
the user still has to install software, locate it, configure profiles/cores,
and return to EmuWiz. The product does not consistently say “you can play this
once you install/configure X”.

## 9. Launch

Selected/game details and Launch Readiness provide per-emulator launch plans.
The UI can show Ready/Ready with warnings, blockers, firmware state, profile
requirements, and platform-specific launch controls such as “Play — Launch
RetroArch”, “Launch Dolphin”, and “Launch PCSX2”. Launch failure banners are
usually short and hide the detailed process output behind technical context.

The launch surface is comparatively mature, but diagnosis is fragmented. A
failure can be caused by content identity, BIOS, emulator installation,
profile/configuration, permissions, or the external process. The user often
gets a truthful message but not a single route to the responsible page. The
normal view should say “Game problem”, “BIOS problem”, “Emulator setup
problem”, or “Launch configuration problem”; command lines, paths, PIDs, and
stderr belong under Advanced.

## 10. Arcade

Current core support includes MAME and FBNeo compatibility, dependency-aware
evidence, authority provenance/refresh impact, alternative complete layouts,
Ready-to-Play, recommendation policy, and collection statistics. The inspected
GUI does not provide a dedicated player-facing arcade explanation of these
features.

A novice can be forced to encounter “MAME”, “FBNeo”, “parent”, “clone”, BIOS,
device, CHD, DAT, or archive layout concepts without being told the answer:
“EmuWiz recommends FBNeo for this game” or “MAME supports this game, but the
required shared firmware is not available.”

The GUI should consume the existing typed result and show:

- recommended emulator and why;
- playable now / needs attention / unknown;
- one plain-language blocker;
- optional “Show arcade details” for parent/clone, BIOS, device, CHD, DAT
  authority, and alternative layouts.

No evidence was found that collection-wide MAME statistics or authority
refresh impact are currently shown to a player. They are backend-ready but GUI
absent.

## 11. DAT

Sources → DATs is a real management surface. It supports local DAT registry
entries, validation, health/status, provenance, coverage, audits, No-Intro
pack import, managed sources, updates, platform assignment, and read-only
rename planning. The page explains that a DAT is a trusted list of known-good
game files and repeatedly says that validation/audit does not rename files.

The main novice risk is that the page is still a catalogue-management tool.
“DAT”, “Logiqx XML”, “ClrMamePro”, “TOSEC”, “coverage”, “audit”, “pack
selection”, “managed source”, and “participating sources” are not first-run
language. The page tells users what a DAT is, but does not lead with whether
they need one for their current library or what improves after adding it.

Recommended product sentence: “Optional: add a trusted game list so EmuWiz
can identify files and verify whether a release is complete. You can still
browse and play without one where emulator evidence is sufficient.”

## 12. Cheats

Cheats & Mods and Cheat Sources expose several real workflows: local cheat
inspection, provider/source ordering, RetroArch catalogue retrieval and
verification, PCSX2 serial/CRC matching, Dolphin GameSettings/Gecko, Xenia,
and install/remove flows with confirmation and undo where supported.

The safest paths are explicit, but the surface is overloaded. A novice must
understand sources, catalogues, exact IDs, serials, CRCs, profiles, INI files,
Gecko definitions, and emulator-specific file locations. Some messages are
excellent (“EmuWiz needs a verified Dolphin Game ID before it can search
BSFree”); others lead with implementation details.

The desired path is: choose a game → show compatible cheat sources → preview
the effect → confirm install → show which emulator profile was changed → offer
Undo. Current GUI can perform pieces of this, but there is no single consistent
cross-emulator novice flow.

## 13. Mods

The current GUI visibly supports local mod package inspection and, for several
platforms, compatibility checks, previews, confirmation, safe apply, and undo.
It also has Cheats & Mods provider/browse surfaces. The local mod page correctly
refuses installation when exact game identity or a safe target folder is not
available.

The GUI does not yet present one understandable acquisition-to-application
journey. Download transport is concurrently in progress and is not assumed
available here. The shortest future flow is:

`Find a mod → review compatibility → acquire → inspect → preview changes → apply → undo`

“Payload”, “package”, “target folder”, “identity evidence”, and exact file
details should be secondary wording. The primary outcome should say which game
will change and whether the original files are protected.

## 14. Save Vault

PCSX2 is currently reached through emulator setup/selected-game context rather
than a clearly named standalone Save Vault destination. The PCSX2 surface can
check the emulator, show BIOS status, map a verified PS2 serial, inspect
per-game/shared memory cards, show patches/textures/save states, display card
health and memory-card contents, export one regular file, and export a complete
save as a PSU container.

The current wording is unusually safe and clear: “EmuWiz reads the card only.
Exporting does not modify the PS2 memory card.” Confirmation shows card, save,
file, logical size, destination, and identity binding; success shows destination,
size, SHA-256, and unchanged-source wording.

The discoverability problem remains significant. A novice thinks in terms of
“my PS2 save” and “memory card”, not “PCSX2 assets” or a PSU container. Card,
save directory, save file, and whole-save export are adjacent but not clearly
hierarchical. Restore/import is absent and should be stated plainly. Save Vault
deserves a user-facing task entry, even if its implementation remains under
PCSX2.

## 15. Playing Library / 1G1R

Playing Library / Library Organisation provides destination-root selection,
region order, language preferences, exclusions, preview, explicit confirmation,
apply, transaction history, and Undo. It explains that source files are never
changed and that the output is a separate RetroDECK/ES-DE library.

The safety model is good. “Playing Library”, “1G1R”, “canonical”, “election”,
“link targets”, “source root”, and “published library” are not novice terms.
The UI should lead with “Build a separate menu of the versions you prefer” and
then describe region/language preferences. Filesystem/link details belong in
Advanced. The flow is actionable once understood, but the acronym and scary
path terminology create avoidable hesitation.

## 16. Storage Health

Storage Health is explicitly read-only and reports scanned items, logical and
allocated size, efficient items, compression candidates, topology-sensitive
items, duplicates/shared candidates, and unsupported/unknown items. It says
archive pack/unpack execution is explicit and source-preserving.

This answers “what is happening?” better than “what should I do?”. It is a
future-analysis surface, not a repair path. “Logical size”, “allocated size”,
“topology-sensitive”, and “duplicate/shared candidates” need plain-language
tooltips. A novice should see whether action is recommended, whether EmuWiz can
perform it, and why it refuses otherwise.

## 17. Language / Terminology

### Representative classification

| Class | Current examples | Audit result |
|---|---|---|
| A — novice-friendly | “What would you like to do?”, “Choose where your games live”, “Your files won't be renamed unless you approve it”, “No source folders yet”, “Play — Launch RetroArch” | Keep and reuse |
| B — understandable but technical | “Ready with warnings”, “Inspect skipped”, “Emulator Manager”, “BIOS / Firmware”, “Review Update”, “Library Organisation” | Add one-line explanation/action |
| C — developer/internal | “source role”, “master BIOS root”, “projection”, “catalogue coverage”, “identity evidence”, “serial mapping”, “rompath”, “logical size”, “transaction”, “reverify”, “launch binding” | Move behind Details/Advanced or translate |
| D — actionless/misleading risk | “No readiness projection is available yet”, “No source match was found”, “No conventional BIOS is required”, “Inventory has not been scanned yet”, “Unsupported/unknown” without a destination action | Pair with next action and consequence |

### Canonical novice terms

| Internal/variant terms | Recommended normal term |
|---|---|
| Source / source folder / root | Games folder |
| ROM / archive / media item | Game file (use ROM only when format matters) |
| DAT / catalogue / authority | Trusted game list; explain “DAT” once |
| Problem / issue / finding | Problem |
| Repair / apply / fix | Review fix, then Apply fix |
| Ready / playable / launch-ready | Ready to play |
| Memory card / save directory / save file | PS2 memory card → save → file, shown as a hierarchy |
| Evidence / provenance / identity | How EmuWiz knows; keep technical details expandable |
| 1G1R / canonical organisation | Preferred versions library |
| projection / dependency | Setup requirement / shared requirement |

## 18. Advanced vs Normal Information

Normal view should contain: outcome, consequence, one next action, safe-state
promise, and the selected game/platform/emulator. Advanced or Details should
contain: paths, hashes, DAT source identity, parser/version provenance, ROM
member names, parent/clone relationships, BIOS/device/CHD graph details,
mount/container paths, command lines, process IDs, source roles, transaction
IDs, and raw provider errors.

“Why?” should be a short intermediate layer: one plain explanation plus the
technical evidence. “Diagnostics” can expose the full report. This preserves
trust without making a first-time player read a preservation-engine report.

## 19. Journey Scorecard

| Flow | Discoverable? | Understandable? | Actionable? | Safe? | Backend complete? | GUI complete? | Novice-ready? |
|---|---|---|---|---|---|---|---|
| First run | YES | YES | PARTIAL | YES | YES | YES | PARTIAL |
| Add games | YES | YES | YES | YES | YES | YES | YES |
| Scan/discovery | YES | PARTIAL | PARTIAL | YES | YES | YES | PARTIAL |
| Fix BIOS | YES | PARTIAL | NO | YES | PARTIAL | NO | NO |
| Install/configure emulator | YES | PARTIAL | PARTIAL | YES | PARTIAL | PARTIAL | PARTIAL |
| Identify game | YES | PARTIAL | PARTIAL | YES | YES | PARTIAL | PARTIAL |
| Launch game | YES | PARTIAL | PARTIAL | YES | YES | YES | PARTIAL |
| Arcade game | PARTIAL | NO | PARTIAL | YES | YES | NO | NO |
| Import DAT | YES | PARTIAL | YES | YES | YES | PARTIAL | PARTIAL |
| Use cheat | YES | PARTIAL | PARTIAL | YES | YES | PARTIAL | PARTIAL |
| Use mod | YES | PARTIAL | PARTIAL | YES | YES | PARTIAL | PARTIAL |
| Export PS2 save | NO | PARTIAL | YES | YES | YES | PARTIAL | PARTIAL |
| Build Playing Library | YES | PARTIAL | YES | YES | YES | YES | PARTIAL |
| Diagnose storage issue | YES | PARTIAL | NO | YES | PARTIAL | PARTIAL | NO |

## 20. Backend Done / GUI Missing

| Backend capability | Current GUI state | Smallest useful GUI slice |
|---|---|---|
| MAME/FBNeo recommendation | Not exposed as a player-facing arcade explanation | Add one recommendation card to selected arcade game with “why” details |
| Authority refresh impact | No visible collection impact report | Add a read-only “authority update affects N games” panel linked to affected sets |
| Alternative complete layouts | Core model/evaluator exists; no normal GUI explanation | Show “another valid layout satisfies this set” under arcade Details |
| MAME collection statistics | Core summary exists; no GUI surface | Add collection health summary to Arcade/Problems, with explicit denominators |
| BIOS inventory/projection | Inspection-only BIOS page | Add per-platform requirement/action handoff; keep apply capability honest |
| Emulator update/rollback | Manager supports staged review/update/rollback | Add novice recommendation and direct setup handoff, not more raw inventory |
| PS2 single-file and PSU export | Present in PCSX2 context, buried and terminology-heavy | Promote “Save Vault” task entry and explain card/save/file hierarchy |
| Local mod safe inspection/apply | Present for supported targets, fragmented | Standardise review → preview → apply → undo framing |
| Cheat compatibility/install | Multiple provider/emulator lanes exist | Add one selected-game action surface that routes to the correct lane |
| Discovery skipped/unknown detail | Present under Sources/Discovery | Add one post-scan “What should I do next?” summary |

## 21. GUI Present / Loop Incomplete

- Scan completes → counts are shown, but no single “Open my playable games” or
  “Review the three things blocking play” route is consistently offered.
- BIOS missing → status is shown, but locating/acquiring/organising the safe
  next step is manual.
- Emulator absent → inventory says none detected, but installation/setup is
  external and not connected to the game blocker.
- Ready-to-Play blocked → reasons appear, but fix destinations are not
  consistently attached.
- MAME dependency incomplete → core can explain the dependency, but the normal
  UI does not translate it into a shared-requirement action.
- DAT audit mismatch → audit/report/rename planning exist, but “what improves
  if I add this DAT?” is not the main question answered.
- Mod/cheat discovery → individual workflows exist, but acquisition, preview,
  compatibility, and apply are not a single common loop.
- Save export → export succeeds safely, but Save Vault is not a discoverable
  product task and restore absence is not prominent.
- Storage candidate → analysis exists, but the user is not told whether to act
  or whether EmuWiz can act.

## 22. Top 10 V1 UX Blockers

| Rank | Priority | Scenario and current behavior | Why it matters | Smallest fix | Likely files/modules | Backend? | Collision risk |
|---:|---|---|---|---|---|---|---|---|
| 1 | P0 | A new user scans a folder and receives no unified “next step” route. | They cannot tell whether scanning worked or what unlocks play. | Add a post-scan/Home summary with counts and one primary next action. | `home_page.rs`, `sources_page.rs`, `main.rs` dispatch | Exists | High: `main.rs` ownership |
| 2 | P0 | A game is blocked by BIOS/emulator setup, but the user must find the relevant page manually. | The normal play journey stops at a truthful but actionless verdict. | Add typed blocker actions: Open BIOS, Open Emulator Setup, Review content. | `ready_to_play_page.rs`, `launch_readiness_page.rs`, `main.rs` | Mostly exists | High: launch/main routing |
| 3 | P0 | Arcade content is supported by MAME/FBNeo but the recommendation/dependency explanation is invisible. | Novices cannot know which emulator to use or why a set is not ready. | Add selected-arcade recommendation card consuming existing core output. | selected-game surface, `arcade_recommendation` adapter | Exists | Medium |
| 4 | P0 | BIOS page identifies missing/candidate firmware but offers no completion handoff. | A blocked game remains blocked despite strong backend evidence. | Add per-platform “Locate existing BIOS”/“Review requirement” action where safe. | `bios_projection_page.rs`, BIOS adapters, main routing | Partial | Medium |
| 5 | P1 | Save export is buried under PCSX2 and card/save/file terminology is unclear. | Users cannot discover a high-value safe operation and may confuse PSU with a file. | Add Save Vault entry/section and plain hierarchy; state restore is unavailable. | `pcsx2_page.rs`, navigation/main | Exists | High: main navigation ownership |
| 6 | P1 | Emulator Manager shows inventory/update mechanics rather than “what this game needs”. | Manual emulator safari remains necessary. | Add game-context setup handoff and recommended emulator/profile wording. | `emulator_inventory_page.rs`, `emulator_setup_page.rs`, launch page | Partial | Medium |
| 7 | P1 | DAT management is discoverable but asks users to become catalogue administrators. | Users do not know whether adding a DAT benefits them. | Add “why add this trusted game list?” and result-oriented setup copy. | `dat_sources_page.rs`, onboarding | Exists | Low/medium |
| 8 | P1 | Problems & Repair reports findings but fixes are split across diagnostics, repair, and pages. | Users cannot finish a detected problem from the finding. | Add one typed action per major finding family and preserve Details. | `problems_repair_page.rs`, `doctor_page.rs`, `main.rs` | Partial | High: shared main/dirty areas |
| 9 | P1 | Cheats/mods work in several specialist lanes but lack one common selected-game workflow. | Users must understand emulator-specific formats and sources. | Add a common selected-game entry card routing to the existing lane. | `cheats_mods/*`, `local_mod_package_page.rs`, selected surface | Mostly exists | Medium |
| 10 | P2 | Storage/Playing Library surfaces expose technical filesystem concepts before outcomes. | Safe features appear risky or incomprehensible. | Replace primary wording with outcome-first copy; retain technical disclosures. | `storage_health_page.rs`, `playing_library_page.rs`, `rom_organisation_page.rs` | Exists/partial | High: storage page currently dirty |

## 23. Recommended Next 5 Implementation Slices

### Slice 1 — Post-scan “What next?” summary

Use existing scan summary, ingestion counts, skipped details, library count,
and loaded setup state to present one card: “Found X games; Y need review; Z
are ready to browse.” Give one primary route to Library or Needs Attention and
secondary links to DAT/Emulator Setup. This is the highest leverage and should
avoid new backend work.

Likely collision: `main.rs` and Sources/Home state ownership. Coordinate before
editing because `main.rs` is dirty/shared.

### Slice 2 — Typed blocker actions from Ready-to-Play

Define a small GUI action mapping over existing readiness reason families:
Open BIOS/Firmware, Open Emulator Setup, Review identity, Review content, or
Open configuration. Keep command/path details in Advanced. This closes the
scan → play loop for non-arcade platforms.

Likely collision: `launch_readiness_page.rs` and `main.rs` dispatch; do not touch
`launch/mod.rs`.

### Slice 3 — Selected-game arcade recommendation card

Render the existing MAME/FBNeo recommendation, readiness state, and one plain
reason in the selected-game surface. Add expandable details for parent/clone,
BIOS/device/CHD, authority, and alternatives. Do not add another evaluator.

Likely collision: selected-game/main routing; core is already complete.

### Slice 4 — Save Vault task entry and terminology pass

Expose the existing PCSX2 memory-card surface as a named Save Vault task, with
“memory card → save → file” hierarchy, clear “Export File” vs “Export whole
save as PSU”, and a prominent statement that restore/import is not available.
Reuse existing safe export code.

Likely collision: navigation/main ownership; this is separate from dirty
`storage_health_page.rs`.

### Slice 5 — Finding-to-action bridge for BIOS/emulator problems

Add action links to Problems & Repair/Ready-to-Play for the two most common
blockers, BIOS and missing emulator/profile. This can initially route to
inspection pages without inventing an apply path. Track whether a page is
inspect-only so the button says Review rather than Fix.

Likely collision: `main.rs`, `problems_repair_page.rs`, `bios_projection_page.rs`,
and emulator setup state. Source Role Editing remains blocked and should not be
reissued as part of this slice.

## 24. Deferred Polish

- Replace remaining “DAT”, “projection”, “authority”, “evidence”, and
  “dependency” primary labels with outcome-first text while preserving exact
  technical details behind disclosure.
- Add consistent empty states with one action and one sentence explaining what
  is not known yet.
- Standardise status vocabulary around “Ready to play”, “Ready with warnings”,
  “Needs review”, “Blocked”, “Not supported”, and “Not checked yet”.
- Add visible collection-wide arcade statistics only after the selected-game
  recommendation card proves the wording and denominator model.
- Consider a dedicated Save Vault route after the first task-entry slice; do
  not create a parallel PCSX2 state model.
- Consolidate cheat/mod provider language after the common selected-game entry
  exists.
- Revisit Source Role editing when shared `main.rs` ownership is available.

