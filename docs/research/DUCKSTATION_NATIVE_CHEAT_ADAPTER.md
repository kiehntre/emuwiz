# DuckStation native cheat adapter

## Format and sources

The adapter follows the current DuckStation core implementation rather than
guessing a legacy format. DuckStation discovers per-game cheat files as
`SERIAL.cht`, or `SERIAL_HASH.cht` when a game hash disambiguates revisions.
Each file contains named INI-like sections, metadata such as `Type` and
`Activation`, and raw code lines. DuckStation's source currently identifies
`Gameshark` and `Assembly` code types and `Manual`/`EndFrame` activation modes.

Primary references:

- [DuckStation cheats implementation](https://raw.githubusercontent.com/stenzek/duckstation/master/src/core/cheats.cpp)
- [DuckStation cheats API](https://github.com/stenzek/duckstation/blob/master/src/core/cheats.h)
- [DuckStation community database README](https://github.com/duckstation/chtdb)
- [DuckStation code format](https://github.com/duckstation/chtdb/blob/master/cheat-format.txt)

The source also confirms that per-game enablement is represented by the game
settings `[Cheats] Enable` list, with `[Cheats] EnableCheats` as the local
cheat-enable setting. EmuWiz reports reload as required after changing either
the CHT file or game settings. It does not edit global settings.

## Adapter boundary

`duckstation_cheat.rs` provides bounded parse, deterministic render, merge,
single-entry removal, and per-code enable/disable projections. It retains
unknown code lines as typed unsupported entries and records malformed sections,
duplicate names, missing bodies, and resource-limit conditions instead of
silently discarding them.

Only verified serial identity is eligible for a destination. A title hint is
display-only and cannot establish an apply target. Hash-qualified filenames
are accepted only when the hash is the documented 16-hex game discriminator.

The conservative normalizer currently proves direct Gameshark writes for the
documented 8-bit, 16-bit, and 32-bit write forms. Other operations remain raw
and unsupported. No database is downloaded and no code leaves the machine.

## Apply and rollback

Merged bytes are supplied to the existing shared preview and transaction
planner. The adapter requires the staged source digest, approved profile root,
verified serial identity, destination precondition, atomic publication,
post-write verification, journal entry, and shared rollback path. Existing
wrong-content destinations are handled by the shared conflict/replacement
policy rather than silently overwritten. The original CHT source and unrelated
game settings are not modified while parsing or previewing.

The enable/disable helper changes only the selected serial's `[Cheats] Enable`
list and preserves unrelated settings. Removing a cheat removes only its named
section. A future GUI caller can use the returned loadability facts to present
the exact CHT path, serial settings path, per-code state, and reload notice.

## GUI and concurrent routing work

The shared preview labels DuckStation as a distinct adapter and identity kind,
and exposes a narrow backend seam for the Cheats page. Generic cheat routing
and loadability policy are intentionally not changed here; a concurrent routing
feature can consume the adapter's destination, identity, per-code state, and
reload facts without duplicating parser or writer logic.

## Limitations

- Full DuckStation Assembly semantics are retained but not normalized.
- This feature does not acquire or distribute cheat databases.
- Applying a generated file requires the caller to stage the exact bytes under
  the approved profile root before invoking the shared transaction executor.
- Emulator reload/restart is reported as required; EmuWiz does not inject a
  live reload command.
