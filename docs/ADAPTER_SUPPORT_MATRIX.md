# Adapter support matrix

## CURRENT BEHAVIOR

This matrix describes capability shape, not a promise that every emulator or
platform has equal coverage. The authoritative launch rows are in
[LAUNCH_SUPPORT.md](LAUNCH_SUPPORT.md).

These terms are intentionally separate:

- **Recognised** means the file/container shape is known.
- **Identified** means EmuWiz has a bounded platform/game identity result;
  it may still be ambiguous or unverified.
- **DAT-verified** means the identity matches an applicable managed DAT
  authority with recorded provenance and freshness.
- **Organised** means a Library View, Playing Library, RomM, or ES-DE
  projection can plan/apply a supported destination for that item.
- **Emulator-ready** means the selected emulator/profile/firmware and content
  preconditions pass readiness checks.
- **Launchable** means EmuWiz can revalidate and execute the supported launch
  path. Readiness or command planning alone is not launchability.
- **Cheats/mods**, **repair support**, and launch are separate workflows; one
  does not imply the others.

An item can therefore be recognised without being identified, DAT-verified,
organised, emulator-ready, launchable, or eligible for cheats/mods or repair.
Unknown, conflicting, stale, or incomplete evidence remains visible and fails
closed downstream.

| Area | Discovery/inventory | Preview | Apply / rollback |
|---|---|---|---|
| RetroArch cheats/patches | Read-only profiles, cores, playlists, artifacts | Yes | Supported catalogue-backed CHT materialization where identity and destination are exact |
| PCSX2 PNACH | Read-only profile and PNACH inventory | Yes | Supported selected verified PNACH installation through the shared transaction path |
| Dolphin GameSettings/Gecko | Read-only profile and INI inventory; provider retrieval is separate | Yes | Supported selected verified Gecko installation; texture-pack flow is separate |
| GameCube/Wii provider flows | Read-only source validation and staging preview | Yes | Selected supported provider records can use shared apply, journal, and rollback |
| Local mod packages | Bounded local inspection | Plan/preview | Only formats with an approved materializer; unsupported formats fail closed |

## State boundaries

| User-facing question | What must be true | What it does not imply |
|---|---|---|
| Is this media recognised? | The format/container observer accepts it | A game identity, DAT match, organisation, launch, cheats/mods, or repair path |
| Is this game identified? | Bounded evidence produces an identity result | DAT authority or launchability |
| Is it DAT-verified? | An applicable managed DAT match is present and fresh | A working emulator profile or a supported destination |
| Can it be organised? | A supported projection can build a safe plan | Emulator readiness or launch |
| Is it emulator-ready/launchable? | Profile, firmware, identity, content, and launch preconditions pass; launchable additionally has an execution path | Cheats/mods or repair support |
| Can cheats/mods be used? | The format, provider/local source, exact identity, destination, and transaction path are supported | Universal mod installation or launch |
| Can it be repaired? | The specific repair operation has bounded evidence, preview, revalidation, and a supported transaction | That other repair types are supported |

Read-only means discovery, provider browsing, source validation, inventory,
and preview do not mutate emulator files. Apply always requires selected
verified records, explicit confirmation, fresh revalidation, and the shared
transaction engine. Downloads and external installers are outside local mod
package Stage 1.

Some filenames retain READONLY_ADAPTER for compatibility with the historical
module boundary. The name describes the inspection adapter, not a claim that
all current workflows are universally read-only.
