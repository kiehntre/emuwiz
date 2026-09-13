# EmuWiz 0.9 coherence pass

This is the final 0.9 presentation and handoff boundary over the existing
scanners, evidence engines, repair/recovery workflows, organisation planner,
and launch/readiness projections. It adds no new truth store or mutation
engine.

## One mental model

The novice journey is:

**Scan -> Understand -> Fix -> Organise -> Play**

- **Scan:** `Sources` selects the existing game folders that EmuWiz reads.
- **Understand:** `Library`, the Sources tabs (Libraries, DATs, Cheats,
  Discovery), `Media Sets`, and item detail explain identity, authority, and
  topology.
- **Fix:** `Needs Attention` is the main unresolved-issue entry. It routes to
  the existing DAT, repair, duplicate, emulator, organisation, or history
  workflow; opening an issue does not fix it.
- **Organise:** `Library Organisation` previews a separate Playing Library or
  existing publication target. The source library remains unchanged until
  its explicit confirmation step.
- **Play:** item detail and launch/readiness explain identity, media
  topology, emulator/profile readiness, BIOS requirements, and the explicit
  launch boundary.

Advanced tools such as mounts, diagnostics, cheat/mod management, and
operation history remain available without being part of first-run setup.

## Terminology

- **Source** is an existing folder containing games.
- **Library** is the catalogue of what EmuWiz found in those sources.
- **Playing Library** is a separate planned output containing selected
  releases; it is not the source library.
- **Sources** is the single navigation destination for Libraries, DATs,
  Cheats, and Discovery. DAT Sources and Cheat Sources remain internal deep
  links and tab content, not competing sidebar destinations.
- **Needs Attention** is the primary unresolved-issue queue. **Problems &
  Repair** is the detailed diagnostic/repair workspace, not a second queue.
- **Media Sets** describes optical discs, floppy disks/sides, and tapes using
  the topology engine's existing semantics.
- User-facing copy uses **Verified**, **Likely match**, **Needs review**,
  **Blocked**, and **Unsupported**. Raw resolver states, provenance, hashes,
  generations, and authority digests stay in detail views.

## Safety and handoffs

Scanning, identity resolution, DAT inspection, Needs Attention, Media Sets,
launch planning, and Playing Library previews are read-only when opened.
They do not rename, move, delete, download, launch, or alter catalogue truth
merely by being viewed. Existing repair, organisation, publication, and
recovery actions retain their explicit review, confirmation, receipt, and
rollback boundaries.

The supported handoff is: choose a source in Sources; understand the result
in Library, Sources -> DATs, Sources -> Discovery, or Media Sets; review
unresolved work in Needs Attention; preview organisation in Library
Organisation; then inspect the selected item's readiness before an explicit
launch. History & Logs explains what existing operations changed, when, and
whether recovery remains possible.

Launch readiness keeps content identity/topology separate from emulator and
BIOS readiness: a complete verified set can still be blocked because its
required BIOS or emulator profile is unavailable.

## Empty and degraded states

No source, no DAT authority, unknown expected count, no media-set evidence,
missing BIOS, stale evidence, and no launchable option are distinct facts.
They are not presented as successful readiness and each existing page remains
the source of its detailed explanation.

Technical evidence remains available for advanced users, including hashes,
parser/schema generations, authority digests, provenance chains, and recovery
details. Literal pointer-click automation is not available in the current
validation environment, so GUI smoke claims are limited to startup and the
routes exercised by the existing harness.

## Intentionally deferred after 0.9

- Pegasus, LaunchBox, and ROMNight publisher profiles
- Steam publishing
- Smart Collections
- a Ready-to-Play view if not already present
- Launch Recipes
- expansion of the Mods/Patches workflow
- Save Vault or memory-card backup
- Controller Profile Manager
- automatic emulator media swapping
- new publisher or front-end integrations

These are future product work, not implied parts of the 0.9 journey.
