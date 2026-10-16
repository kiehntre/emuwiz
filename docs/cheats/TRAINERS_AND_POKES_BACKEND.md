# Trainers and POKEs: backend interfaces for GUI-v2

Status as of branch `feature/cheats-mods-trainers-backend-20261016`. Core only;
no GUI file was changed. Everything here is read-only until the existing
preview → explicit apply path runs.

## WHDLoad trainer options (CUSTOM1-5)

Trainer options are launch configuration, never memory writes. The slave is
never modified.

1. **Discover** the selected game's options:
   `discover_whdload_trainer_options(&AmigaGameInspection, configuration_bytes)
   -> WhdloadTrainerDiscovery`.
   - Input is the existing `inspect_amiga_whdload_game` result plus the current
     content of the EmuWiz per-game option file (empty if absent).
   - The slave is re-read once; declarations and SHA-256 come from the same
     bytes, and a slave that changed since inspection is refused.
   - `state` is one of `Ready`, `IdentityUnverified`, `NoSlave`,
     `AmbiguousSlaves` (never auto-chosen), `SlaveInsideHdf` (options listed
     read-only), `SlaveChanged`, `SlaveUnreadable`, `NoTrainerOptions`.
     `state.plain_label()` gives beginner wording, `state.can_preview()` gates
     the editor's Preview button.
   - `options: Vec<TrainerOption>` lists every C1-C5 declaration, including
     `Opaque` ones (readiness `Unsupported`), which must stay visible but disabled.
   - `identity` is present only when `Ready` and feeds `WhdloadTrainerRequest`.
2. **Select**: build `TrainerOptionSelection { custom_slot: option.selection_key, value }`.
   - `selection_key` equals the bare slot (`CUSTOM2`) when one declaration uses
     the slot. When several share it (the common `C1:X:Lives:0;C1:X:Energy:1;`),
     each has its own key (`CUSTOM1:X:0`, `CUSTOM1:X:1`) and the backend merges
     their bits into one `CUSTOM1=` value, preserving unrelated bits.
   - `value: None` removes the option. For a bit option (`X`, or `M` with a
     bit range) that clears only its own bits and keeps the slot's other bits;
     for a whole-word option (`B`, `L`, `N`) it removes the `CUSTOMn=` line.
     Setting and removing options on the same slot in one request is refused.
   - A bare shared slot is refused with `AmbiguousSlot`. The current legacy
     editor keys selections by `custom_slot`. That still works for single-use
     slots, and shared slots now fail closed instead of silently writing the wrong bit.
3. **Preview / apply / undo**: unchanged: `build_whdload_trainer_preview`,
   `apply_whdload_trainer_preview`, shared-transaction rollback.
   Conflicts are typed `TrainerOptionConflictKind`; new kinds are
   `AmbiguousSlot` and `UnreadableCurrentValue` (an existing non-decimal value
   blocks a bit merge rather than overwriting the other bits).
4. **Launch**: `project_whdload_trainer_launch_options` gives FS-UAE
   arguments; Amiberry is preview-only. No launch path consumes the arguments yet.

## ScummVM trainer options

Hypno engine only (documented gameplay options). Changes: current values are
read from the EmuWiz target section the preview writes; duplicate keys in that
section are all rewritten (ScummVM honours the last one); a game folder path
containing control characters is refused (`UnsafePath`).

## Classic POKEs (ZX Spectrum, CPC, C64, Atari ST, MSX, BBC/Acorn)

Interchange and preview model; only Fuse (`.pok`) and VICE (monitor
commands) can produce an artifact, and only for a verified identity.

- Parsing accepts the platform's own hex notation (`&` CPC/BBC/Acorn, `&H`
  MSX, `$`/`0x` everywhere) and BBC `?address=value` byte writes. Word writes
  (`!`), expressions and BASIC control flow are refused.
- Titles must be one line (no control characters); the Fuse projection also
  refuses such a title (`InvalidTitle`) and an empty trainer (`NoOperations`).
- VICE projection uses the same C64 memory classifier as the VICE cheat
  adapter: I/O (`MemorySpaceUnproven`), ROM-shadowed and cartridge-banked
  areas (`BankingUnproven`) are refused; RAM and colour RAM project.
- Conflicts are only reported between cheats for the same platform.
- ZX `.pok` import: a UTF-8 BOM is accepted, a multibyte record letter is an
  `InvalidLine` (it used to panic), a trainer still open at `Y` is kept (it
  used to be dropped), and value 256 (ask the user at runtime) is refused as
  `ZxPokError::UserValuePrompt`.

## Cheat file import

A `.cht` using named `[sections]` without a `cheats =` count (DuckStation's
format) is reported as `UnsupportedFormat` with a message naming the format,
instead of being indexed as a RetroArch file. No new `UserCheatFormat`
variant was added (the GUI matches it exhaustively).

## Blue GUI Batch 8 contract (WHDLoad trainers)

Batch 8 wires the existing backend; it adds no new trainer behaviour.

1. For the selected Amiga game, call `discover_whdload_trainer_options(&inspection, &current_option_file_bytes)`.
   Render `state.plain_label()` and `detail` for every non-`Ready` state; enable Preview only when
   `state.can_preview()` and `identity.is_some()`.
2. List every entry of `options`, including `Opaque` / `Unsupported` ones, which stay visible but
   disabled with their reason. Never hide an option the slave declares.
3. Identify each control by `option.selection_key` and send
   `TrainerOptionSelection { custom_slot: option.selection_key.clone(), value }`.
   Never key by `option.custom_slot`: shared slots are refused with `AmbiguousSlot`.
4. Reuse the existing `build_whdload_trainer_preview` → explicit confirmation →
   `apply_whdload_trainer_preview` → shared-transaction Undo path. Show `TrainerOptionConflict`s
   (including `AmbiguousSlot`, `UnreadableCurrentValue`, `SameSlotDifferentValue`) as blocking reasons.
5. Tests: at least two independent `X` options on one CUSTOM slot (for example the
   `C1:X:Infinite lives:0;C1:X:Infinite energy:1;C1:X:In-game keys:3;` declaration) can be enabled
   and disabled independently, and disabling one keeps the others.

**Remaining dependency, not part of Batch 8:** no launch path consumes
`project_whdload_trainer_launch_options` yet, so applied options are not passed to FS-UAE as
`CUSTOMn=` arguments at launch. Until that wiring exists, the GUI must say options are saved to
EmuWiz's per-game option file and must not claim the trainer is active when the game is launched.
