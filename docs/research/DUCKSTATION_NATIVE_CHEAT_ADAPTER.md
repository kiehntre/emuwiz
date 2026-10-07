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

The conservative normalizer proves only the three direct Gameshark writes,
verified against upstream `src/core/cheats_private.h` (`InstructionCode`):
`0x30` ConstantWrite8, `0x80` ConstantWrite16 and `0x90` ExtConstantWrite32.
**`0xA0` is `ExtCompareEqual32`, a conditional instruction, not a write**; an
earlier version of this adapter mapped `0xA0` to a 32-bit write, which was
wrong. Every other opcode, including `0xA0`, stays a raw, preserved,
unsupported line. The second code word is a hex number of 1-8 digits (upstream
parses `80123456 03E7` as valid); 8/16-bit writes use only its low 8/16 bits.
No database is downloaded and no code leaves the machine.

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

## Native adapter V1 (`duckstation_native.rs`)

V1 turns the foundation above into a usable adapter for **verified single-disc
PS1 serials**: preview, write `SERIAL.cht`, update the per-game INI, verify,
receipt, undo. It is backend-only; see "GUI status".

### Verified upstream behaviour it relies on

(From `cheats.cpp`, `cheats_private.h`, `settings.cpp` at master.)

- Cheats are enabled only in the per-game INI: `[Cheats] EnableCheats = true`
  plus one `Enable = <name>` line per cheat. A fresh INI has neither, so the
  adapter creates `[Cheats]`, `EnableCheats` and the entry.
- `[Folders] Cheats` / `GameSettings` in `settings.ini` override the defaults
  `cheats` / `gamesettings`; a relative value joins the data root, an absolute
  value is used as-is.
- **Every** `<serial>*.cht` file in the cheats folder is loaded, and a later
  cheat with the same name overwrites an earlier one. Any file other than
  exactly `<serial>.cht` therefore makes the effective set order-dependent and
  is refused (`HashSpecificVariantPresent`).
- Community-database cheats load from `cheats.zip` unless the game INI sets
  `[Cheats] LoadCheatsFromDatabase = false`, and an on-disk cheat overwrites a
  database cheat of the same name. The database cannot be seen offline, so a
  write must either find that setting `false` or be explicitly acknowledged
  (`DatabaseShadowingUnknown` otherwise).

### Scope and typed refusals

Only PS1, DuckStation's own format, a verified serial (`AAAA-NNNNN`), a proven
single disc, the hashless `SERIAL.cht` and one profile. Refusals are the typed
`DuckStationNativeRefusal`: `MissingVerifiedSerial`, `AmbiguousSerial`,
`UnsupportedMultiDisc`, `DiscTopologyUnproven`, `HashSpecificVariantPresent`,
`CheatNameConflict`, `CheatNotFound`, `DestinationChangedAfterPreview`,
`InvalidDuckStationProfile`, `UnsafeCustomFolder`, `DatabaseShadowingUnknown`,
`IniUpdateConflict`, `ExistingFileChanged`, `UnsupportedCheatCode`,
`InvalidCheat`, `ConfirmationRequired`, `InvalidOperationId`. Each has a
plain-language `explain()`.

### Editing

`SERIAL.cht` is edited line by line, so every untouched byte (comments, unknown
metadata, spacing, line endings, unsupported code lines, other cheats) is kept
exactly. A new cheat is appended; a same-name cheat with different content is a
`CheatNameConflict`. Replacing a cheat (`Update`) needs the digest of the
section being replaced (`duckstation_native_section_digest`), proving the
caller previewed it. Only direct write lines (`30`/`80`/`90`) are accepted for
writing. The game INI edit keeps unrelated sections and keys, creates what is
missing, rewrites a non-true `EnableCheats` to `true` (reported in the preview)
and refuses an INI with two `[Cheats]` sections or duplicate `EnableCheats`.

### Transaction

The `.cht` and the INI are one logical operation built from two single-file
shared transactions (they can live in different folders, which one shared plan
cannot express; the shared preview also needs a `<directory>/<file>` destination,
so each file is rooted at its folder's parent). Lifecycle: identify, preflight,
preview (no writes), confirm, stage both outputs privately and verify them,
publish the `.cht` (inert without the INI) then the INI, verify both from disk
(planned bytes, intended sections, unrelated sections and INI lines preserved),
write a receipt with before/after SHA-256 for each file. A failure after the
first publication rolls back the published side through its shared journal and
verifies the file is restored; success is never reported with one side applied.
Undo reverts the INI first (a partial undo is inert) and then the `.cht`, and
refuses when either file changed since apply.

### GUI status

Not wired in this change. The Cheats page has a full per-adapter workflow
(profile choice, candidate selection, preview, apply, history); one existing
adapter touches ~38 places across a 6,100-line controller and an 8,700-line
renderer. Adding DuckStation there is a separate, invasive follow-up. The seam
is ready: `discover_duckstation_profiles` -> `configuration_path` is the
`profile_root`; `plan_duckstation_native` gives the preview or a typed refusal;
`apply_duckstation_native_plan` / `undo_duckstation_native` do the rest.

### Deferred

Multi-disc routing, hash-specific `SERIAL_<hash>.cht`, `discset.yaml`, reading
the community database to prove no name collision, Assembly cheats and the
non-write Gameshark opcodes, and the GUI.
