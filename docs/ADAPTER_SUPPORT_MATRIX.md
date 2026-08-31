# Adapter support matrix

## CURRENT BEHAVIOR

This matrix describes capability shape, not a promise that every emulator or
platform has equal coverage. The authoritative launch rows are in
[LAUNCH_SUPPORT.md](LAUNCH_SUPPORT.md).

| Area | Discovery/inventory | Preview | Apply / rollback |
|---|---|---|---|
| RetroArch cheats/patches | Read-only profiles, cores, playlists, artifacts | Yes | Supported catalogue-backed CHT materialization where identity and destination are exact |
| PCSX2 PNACH | Read-only profile and PNACH inventory | Yes | Supported selected verified PNACH installation through the shared transaction path |
| Dolphin GameSettings/Gecko | Read-only profile and INI inventory; provider retrieval is separate | Yes | Supported selected verified Gecko installation; texture-pack flow is separate |
| GameCube/Wii provider flows | Read-only source validation and staging preview | Yes | Selected supported provider records can use shared apply, journal, and rollback |
| Local mod packages | Bounded local inspection | Plan/preview | Only formats with an approved materializer; unsupported formats fail closed |

Read-only means discovery, provider browsing, source validation, inventory,
and preview do not mutate emulator files. Apply always requires selected
verified records, explicit confirmation, fresh revalidation, and the shared
transaction engine. Downloads and external installers are outside local mod
package Stage 1.

Some filenames retain READONLY_ADAPTER for compatibility with the historical
module boundary. The name describes the inspection adapter, not a claim that
all current workflows are universally read-only.
