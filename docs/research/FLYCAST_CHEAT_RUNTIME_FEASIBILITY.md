# Flycast cheat runtime feasibility

Status: **blocked, no adapter added.** EmuWiz level for Flycast cheats at
launch time: **L0 (no launch wiring).** The earlier native cheat *file*
management (`patch_manager/flycast_cheats.rs`, `flycast_local.rs`) is a
separate, already-landed feature and is not a launch seam.

## Evidence base

- Host `saltbox26`, 2026-10-04. **Standalone Flycast is not installed**
  (no `flycast` binary, no Flatpak, no `~/.config/flycast`,
  `~/.local/share/flycast`). Only the libretro Flycast core under RetroArch
  exists, which is a different program with a different cheat path.
  Therefore **no real probe was possible** and no installed version exists to
  report.
- Everything below is read from upstream source at
  `flyinghead/flycast` master `59ed35a7ea7c1940d4c8ac221a662d0e6d6dc9ea`
  (latest release tag `v2.7`, 2026-08-19). Master may differ from v2.7; this is
  unverified against a shipped build. Files read: `core/cheats.cpp`,
  `core/cfg/cl.cpp`, `core/cfg/cfg.cpp`, `core/cfg/ini.cpp`,
  `core/cfg/option.cpp`, `core/emulator.cpp`, `core/linux-dist/main.cpp`.

## Answers

1. **External cheat files?** Yes: the `.cht` INI format (`cheatN_desc`,
   `_address`, `_cheat_type`, `_value`, `_enable`, ...), as already modelled in
   `docs/research/FLYCAST_DREAMCAST_NATIVE_CHEATS.md`.
2. **Association with a game.** Flycast's game id is the IP.BIN product number
   (`settings.content.gameId`, `emulator.cpp`). The mapping is **not** a
   `<game-id>.cht` file lookup as the older EmuWiz note says. It is:
   - the `[cheats]` section of `emu.cfg`: `<gameId> = <path to .cht>`
     (`config::loadStr("cheats", gameId)`); or, if absent,
   - an auto-locate in the `Dreamcast.CheatPath` directories by the **content
     file name** (`<rom file name>.cht` / `.txt`), i.e. filename-based, not
     id-based.
   UI-created cheats are saved to `<save prefix>.cht` and recorded in
   `[cheats]`.
3. **CLI.** The only CLI option is `-config section:key=value,...`, which sets
   *transient* values "not saved to emu.cfg". There is no cheat-file option.
   Both `cheats:<gameId>=<file>` and `config:Dreamcast.CheatPath=<dir>` could
   in principle be passed this way.
4. **Per-launch config/data dir.** Linux honours `XDG_CONFIG_HOME` and
   `XDG_DATA_HOME` (`find_user_config_dir`/`find_user_data_dir`), so a child
   could be given a scratch profile through its environment.
5. **Enable state.** `loadCheatFile` reads `cheatN_enable`; Flycast itself
   *writes* `enable = false` for every cheat it saves. A staged file with
   `enable = true` would probably load enabled, unlike MAME. Unverified.

## Why launch-scoped wiring is not safe today

- **Flycast writes the permanent config when it loads a cheat.** After a
  successful `loadCheatFile` it calls
  `config::saveStr("cheats", gameId, filename)`. `saveStr` stores a
  non-transient value and auto-saves `emu.cfg`. Even if the path was supplied
  transiently, the real `emu.cfg` is rewritten with `[cheats] <gameId> =
  <scratch path>`, a pointer into a workspace EmuWiz then deletes. The same
  happens on the `CheatPath` auto-locate branch. This violates the
  no-permanent-config-mutation rule and leaves a dangling entry.
- **The only way around that is a scratch `XDG_CONFIG_HOME`/`XDG_DATA_HOME`,**
  which replaces the user's whole Flycast profile: settings, controller
  mappings, BIOS (`data/dc_*.bin`), VMUs and saves. Making that safe means
  seeding BIOS/config into the scratch profile and deciding what to do with
  saves and VMUs written there. That is a design of its own (the RetroArch
  adapter's seeded-profile model), not a small adapter, and could not be
  validated without an installed Flycast.
- **Auto-locate is filename guessing**, which the task forbids as production
  behaviour; only the explicit `cheats:<gameId>` mapping is acceptable.
- **No consumption evidence is available** here: no Flycast to run, so L5 and
  L6 could not be shown.

## Smallest change that would unblock it

Any one of:

- upstream: make `saveStr("cheats", ...)` skip transient entries, or add a
  documented read-only/ephemeral cheat option (for example `-cheat <file>`);
- product: a seeded scratch-profile design for standalone Flycast (BIOS and
  config copied in, saves/VMUs explicitly handled) plus an installed Flycast
  to probe against.

## Next step when Flycast is available

Install a known Flycast, record `emu.cfg` and the data dir hashes, run once
with `-config cheats:<gameId>=<scratch>.cht` against a throwaway `HOME`, and
confirm whether `emu.cfg` in the *scratch* profile gains the `[cheats]` line
and whether the cheat loads enabled (`"N cheats loaded"` in Flycast's log).
Then decide between the scratch-profile adapter and waiting for upstream.

## Correction to an earlier note

`FLYCAST_DREAMCAST_NATIVE_CHEATS.md` says cheats are normally named
`<game-id>.cht` under Flycast's cheat directory. Current upstream source
instead keys by the `[cheats]` map in `emu.cfg` with filename-based
auto-locate under `Dreamcast.CheatPath`. The persistent-write path in that
feature should be re-checked against a real Flycast build.
