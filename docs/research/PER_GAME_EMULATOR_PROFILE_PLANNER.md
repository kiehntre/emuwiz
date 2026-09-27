# Per-game emulator profile recommendation planner

EmuWiz now has a read-only planner for reviewing settings from community or
provider-supplied emulator configurations.  It classifies candidate settings
for an exact game/emulator context; it does not import, write, or apply a
foreign configuration.

## Why full config imports are unsafe

An emulator configuration commonly mixes game compatibility fixes with host
GPU selection, display preferences, controller bindings, BIOS paths, memory
card names, library roots, and UI state.  Copying the whole file can redirect
runtime data, overwrite personal preferences, select the wrong firmware, or
make a game appear compatible only because a host-specific setting happened to
be present.

The planner therefore produces a recommendation for each key and never
creates a configuration writer or an automatic apply path.

## Classification model

Each recommendation retains the key, proposed value, classification,
applicability, provenance, and reason.  The classifications are:

- `PortableGameOverride`: game-specific compatibility settings that may be
  reviewed for portability.
- `HostSpecific`: GPU, renderer, display, refresh, or performance settings
  that depend on the host.
- `PersonalPreference`: fullscreen, OSD, theme, hotkeys, screenshots,
  controller, and achievement preferences.
- `GlobalDangerous`: BIOS, memory-card, library, HDD, log, updater, and UI
  paths/state that must never become portable game overrides.
- `Unknown` and `Unsupported`: settings outside the reviewed semantics.

## Current PCSX2 coverage

The initial classifier recognizes gamefix and speedhack toggles, cycle/VU
controls, skipdraw, half-pixel, blending, mipmapping, deinterlace, user hacks,
and texture preloading as portable candidates when exact identity is proven.
Adapter/renderer, extra threads, refresh, resolution/upscale, anisotropy, and
vsync are host-specific.  BIOS, memory-card, game/HDD/library/log/updater
paths, and UI state are global/dangerous.  Fullscreen, OSD, themes, hotkeys,
screenshots, controller/input, and achievements are personal preferences.
Unrecognized keys remain unknown.

These are conservative key semantics, not an assertion that every version
uses every spelling.  Version-specific evidence is required before a setting
is recommendable.

## Identity, provenance, and staleness

Recommendations require an exact platform, emulator, and game identity.  A
filename-only match is insufficient.  Profile bindings are checked when
present.  Evidence tied to another emulator version is marked stale rather
than promoted.  Every candidate retains its provider, source version/date,
identity binding, emulator version/profile, and reason.

If providers disagree on one key, the plan emits an explicit conflict and no
value is selected.  Identical values are deduplicated into one recommendation
while preserving all provenance entries.

## Future safe preview/apply boundary

A future UI or execution layer may render a plan for user review.  A separate,
version-aware adapter would have to validate the target configuration schema,
host applicability, identity, and rollback strategy before any apply feature
could be considered.  This module intentionally exposes no config writer and
performs no filesystem mutation.

## Research provenance

Community configuration collections informed the distinction between useful
game fixes, host settings, preferences, and dangerous global paths.  They are
research inputs only and are not trusted runtime providers.  A future provider
must supply explicit versioned provenance and exact game identity evidence.
