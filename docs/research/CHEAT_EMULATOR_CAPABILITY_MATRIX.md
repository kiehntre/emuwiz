> **STATUS NOTE (recovered 2026-10-03; recovery baseline main `d112b7f3`).**
> Recovered verbatim from branch `research/cheat-capability-audit-2` (`e43ae942`, 2026-09-29, written against main `4c980d18`). The evidence tables and ratings below are historical and have not been edited.
> **CURRENT STATUS (code-proven only):** since that baseline, main gained native cheat adapter modules under `crates/archivefs-core/src/patch_manager/` (e.g. `mame_cheat.rs`, `melonds_cheat.rs`, `flycast_cheats.rs`, `rpcs3_patch.rs`, `mednafen_cheat.rs`, `fbneo_cheat.rs`, `three_ds_cheat.rs`, `vice_c64_cheat.rs`, `whdload_trainer.rs`). Statements below that "no MAME/Mednafen/melonDS/... cheat code exists" are therefore outdated as statements about the repository. Which adapters are routable and what they prove is summarised in `docs/research/CHEAT_RUNTIME_CURRENT_STATUS.md`. Upstream-emulator behaviour claims (`[SRC]`, `[DOC]`) were not re-verified and nothing was executed for this note.
> `docs/research/CHEAT_FORMAT_ADAPTER_COVERAGE_AUDIT.md`, which this document supersedes in part, is not tracked on main.

---

*Historical document follows unchanged.*

# Cheat Emulator Capability Matrix (EmuWiz) — Part 2

> **Research snapshot, 2026-09-29.** Research and documentation only. No Rust, GUI, `main`, or adapter code was changed.
> **Baseline:** `origin/main` @ `4c980d184584dd5f1a22b5fbbf67e6c58ff204c1` (local `main` == `origin/main`; tracked tree clean).
> **Supersedes** the Part 1 document (`afa8e778`, written from the old dirty `feature/archivefs-unified-platform` worktree at `701a9785`). Part 1 was imported by cherry-pick (`e7abb4fc`) and revalidated against current main; several of its repository claims were **wrong for main** and are corrected in §1.
>
> **Evidence tags:** `[DOC]` official documentation · `[SRC]` upstream source read directly (master, fetched 2026-09-29) · `[SRC-NEG]` absence in an upstream file-tree scan (weak evidence of absence, not proof) · `[REPO]` this repository at the baseline · `[REPO-DOC]` an EmuWiz research doc that itself cites upstream · `[COMMUNITY]` · `[UNKNOWN]`.
> **Nothing was executed.** No emulator was launched. Every "no persistence" claim below is source-inferred and is therefore *not* proof. That is why no emulator is rated READY_FOR_SAFE_ADAPTER.

---

## 1. Revalidation of Part 1 repository claims against current main

| # | Part 1 claim | Status on `4c980d18` | Correction / evidence |
|---|---|---|---|
| 1 | `patch_manager` has cheat preview/apply/journal/rollback | **Correct, much larger** | `shared_preview.rs`, `shared_transaction.rs` (`build_shared_transaction_plan`, `execute_shared_apply`), `cheat_installer.rs`, `cheat_rollback.rs`, `cheat_history.rs`, plus `cheat_route.rs`, `cheat_loadability.rs`, `cheat_compatibility.rs` (conflict analyser). |
| 2 | PCSX2 `.pnach` writes to live profile | **Correct** | `pcsx2_pnach.rs`, `pcsx2_install_plan.rs`, `local_cheat_install_pcsx2.rs`. |
| 3 | Dolphin Gecko/AR INI writes to live profile | **Correct** | `dolphin_gecko_install_plan.rs`, `local_cheat_install_dolphin.rs`, `dolphin_code.rs`. |
| 4 | Xenia TOML writes to live profile | **Correct** | `xenia_install_plan.rs`, `xenia_patch_document.rs`. |
| 5 | RetroArch `.cht` writes to live profile | **Correct, destination changed** | Destination is now `<cheat_database_path>/<core library_name>/<content basename>.cht` and fails closed without an exact core (`docs/research/RETROARCH_CHEAT_AUTOLOAD_PATH.md`; the source path is confirmed in §5.1). |
| 6 | PPSSPP, DuckStation, RPCS3, Flycast are read-only inspection only | **OUTDATED** | Native adapters now exist: `ppsspp_cwcheat.rs`, `duckstation_cheat.rs`, `rpcs3_patch.rs`, `flycast_cheats.rs`; `cheat_apply_support` (`cheat_route.rs`) returns `Supported` for `pcsx2, dolphin, xenia, duckstation, ppsspp, mgba, mame, rpcs3, flycast, amiga_whdload, scummvm` and RetroArch. All are *persistent installs through the shared transaction*, not launch-scoped. |
| 7 | Launch adapters do not compose cheats | **Mostly correct, one exception** | Every `launch/*_execution.rs` still states it never touches cheats. Exception: `scummvm_command.rs` has `ScummVmTrainerLaunchBinding` and `build_scummvm_command_plan_with_trainer` (emits `--config <owned ini> <target>`). Nothing else in `launch/` references trainer or cheat composition. |
| 8 | `safe_launch_sandbox` exists and is used by Atari800, Caprice32, b-em, np2kai, Oricutron | **WRONG for main** | `git log --all` shows no history for `launch/safe_launch_sandbox.rs`, and the file, `atari800`, `caprice32`, `b_em`, `np2kai`, `oricutron` do not exist on main. Part 1 read **uncommitted work in a dirty worktree**. On main, `docs/research/SAFE_LAUNCH_SANDBOX_V1.md` is *design only* (its "Implementation decision" chose docs-only). |
| 9 | `safe_launch_sandbox` refuses disc/HDD media | **Design-only claim** | The design doc refuses `HardDisk`/`Optical` in V1; no code on main. |
| 10 | Amiga and Hatari had no cheat integration | **Amiga changed; Hatari correct** | `whdload_trainer.rs` (WHDLoad `CUSTOM1..5` trainer options, `amiga_whdload` is `Supported` in `cheat_apply_support`). Hatari: still nothing. |

**New primitives on main that Part 1 missed (they matter more than the sandbox design):**
- `launch/resource_grants.rs` — typed `LaunchResourceGrant` (roles incl. `SAVE_DATA`, `MEMORY_CARD`, `NVRAM`, `CONFIG`, `TEMPORARY_RUNTIME`; projection methods incl. `GENERATED_FILE`, `CONFIG_OVERRIDE`, `SYMLINK_*`; lifetime `LAUNCH_ONLY`; access `READ_ONLY/READ_WRITE/CREATE_ONLY`). Vocabulary and validation only.
- `launch/retroarch_resource_projection.rs` — **implemented** first consumer: plans and materialises an EmuWiz-owned launch tree under `$TMP/emuwiz/retroarch-launches/…` with marker file, generated append-config, BIOS symlinks, **real save directory passed through** (`savefile_directory`), and cleanup. `command_with_retroarch_resource_plan` appends `--appendconfig <generated>`. It is re-exported from `launch/mod.rs` but **no production caller** was found (only tests).
- `docs/research/EMULATOR_FILE_ISOLATION_ARCHITECTURE_AUDIT.md` — the design for least-privilege launch grants.
- Existing shared cheat services: `cheat_route.rs`, `cheat_loadability.rs`, `cheat_compatibility.rs`, `cheat_ir.rs`, `action_replay.rs`, `classic_game_genie.rs`, `n64_gameshark.rs`, `saturn_action_replay.rs`.

---

## 2. Complete adapter inventory (discovered from current main)

Method: `ls crates/archivefs-core/src/launch/*_execution.rs`, `*_command.rs`, the `standalone_adapters` in `platform_map.rs`, and `patch_manager/*cheat*`, `*_local.rs`. **34 launch adapters** (32 with `*_execution.rs` + RetroArch (`retroarch_command.rs`, projection) + Fuse (`fuse_command.rs` only)). Six further systems have native-adapter *audits or cheat modules but no launch adapter* (§2.2). Nothing is silently omitted.

Status legend: **R** = researched with source/doc evidence · **R-neg** = researched, no cheat subsystem found · **U** = UNKNOWN after attempted research · **N/A** = not a cheat target.

### 2.1 Launch adapters

| # | Adapter | Platform(s) | EmuWiz cheat code on main | Research status |
|---|---|---|---|---|
| 1 | retroarch | multi (libretro) | `.cht` catalogue/install (`cht_document`, `retroarch_cheat_setup`, `retroarch_materialization`); per-launch projection exists but has no cheat content | R |
| 2 | dolphin | GameCube, Wii | Gecko/AR/OnFrame install (persistent) | R |
| 3 | pcsx2 | PS2 | PNACH install (persistent) | R |
| 4 | duckstation | PS1 | `duckstation_cheat.rs` (parse/merge/enable, persistent apply) | R |
| 5 | ppsspp | PSP | `ppsspp_cwcheat.rs` (persistent apply) | R |
| 6 | rpcs3 | PS3 | `rpcs3_patch.rs` (writes `imported_patch.yml` + `patch_config.yml`) | R |
| 7 | xenia | Xbox 360 | patch TOML install | R |
| 8 | mame | Arcade | `mame_cheat.rs` (XML, definition-only install) | R |
| 9 | fbneo | Arcade | `fbneo_cheat.rs` (parse/merge; route says apply unsupported) | R |
| 10 | mgba | GB/GBC/GBA | `mgba_cheats.rs` (`.cheats` merge/apply) | R |
| 11 | melonds | NDS | `melonds_cheat.rs` (`.mch`; parse/merge, apply not routed) | R |
| 12 | desmume | NDS | none (DS AR only in `cheat_ir`) | R |
| 13 | azahar | 3DS | `three_ds_cheat.rs` (parse/merge only, no apply) | R |
| 14 | flycast | Dreamcast | `flycast_cheats.rs` (persistent apply) | R |
| 15 | stella | Atari 2600 | none | R |
| 16 | snes9x | SNES | none | R (limited) |
| 17 | mesen | NES/SNES/GB… | none | R |
| 18 | sameboy | GB/GBC | none | R (limited) |
| 19 | rmg | N64 | none (`n64_gameshark.rs` decoder only) | R |
| 20 | scummvm | ScummVM games | `scummvm_trainer.rs` + **launch binding** (Hypno options) | R |
| 21 | dosbox | DOS | none; trainer adapter **rejected** by `LAUNCH_OPTION_TRAINER_FAMILY_RESEARCH.md` | R-neg |
| 22 | amiga_whdload | Amiga | `whdload_trainer.rs` (per-game CUSTOM options; **not wired to launch**) | R |
| 23 | amiberry | Amiga | none | R-neg |
| 24 | amiberry_cd | Amiga CD | none | R-neg |
| 25 | fsuae | Amiga | none | U (a `cheats` option exists, semantics undocumented) |
| 26 | hatari | Atari ST | none | R-neg |
| 27 | vice | Commodore | `vice_c64_cheat.rs` projection (not wired to launch) | R |
| 28 | fuse | ZX Spectrum | none (audit says `.pok` not implemented) | R |
| 29 | openmsx | MSX | none | R |
| 30 | xroar | Dragon/CoCo | none | U |
| 31 | tsugaru | FM Towns | none | U |
| 32 | cemu | Wii U | none (graphic packs are mods, not cheats) | R-neg |
| 33 | vita3k | PS Vita | none | R-neg |
| 34 | xemu | Xbox | none | R-neg |

### 2.2 Systems with audits or modules but no launch adapter on main

| System | Where | Status |
|---|---|---|
| Mednafen | `mednafen_cheat.rs`, `MEDNAFEN_NATIVE_CHEAT_ADAPTER.md` (no launch adapter) | N/A for launch; format only |
| BBC Micro (b-em) | `BBC_MICRO_NATIVE_ADAPTER_AUDIT.md` | audit only, no cheat evidence there; U |
| Amstrad CPC (Caprice32) | `AMSTRAD_CPC_NATIVE_ADAPTER_AUDIT.md` | audit only; U |
| PC-98 (NP2kai) | `PC98_NP2KAI_NATIVE_ADAPTER_AUDIT.md` | audit only; U |
| X68000 (PX68k) | `X68000_NATIVE_ADAPTER_AUDIT.md` | audit only; U |
| Atari 8-bit (Atari800), Oricutron, ep128emu | no adapter/audit on main | Atari800 R-neg (§4.4) ; others U |

**Counts:** 34 launch adapters. Researched with source/doc evidence and a classification beyond UNKNOWN: **31**. UNKNOWN after research: **3 launch adapters** (fsuae, xroar, tsugaru) **plus 4 audit-only systems** (b-em, Caprice32, NP2kai, PX68k; not launch adapters). RetroArch is counted once.

---

## 3. Capability matrix

Readiness states: `READY_FOR_SAFE_ADAPTER`, `POSSIBLE_WITH_TEMP_CONFIG` (per-launch files/flags only), `POSSIBLE_WITH_CONFIG_AND_STATE_FENCING` (per-launch config **and** something in the redirected state must be passed through or fenced), `GUI_ONLY`, `GLOBAL_CONFIG_RISK` (activation needs global emulator config), `PERSISTENT_STATE_RISK` (activation needs persistent per-game/emulator state, launch-unscoped), `MEDIA_MUTATION_REQUIRED`, `UNSUPPORTED`, `UNKNOWN`.

**No emulator is READY_FOR_SAFE_ADAPTER.** The criteria (media untouched, saves preserved, no uncontrolled persistent state, deterministic cleanup, explicit selected-cheat composition) can each be met on paper for the best candidates, but none has empirical proof of "no persistence on exit", so none qualifies. Nothing was found that requires **MEDIA_MUTATION_REQUIRED**.

| Emulator | Cheats | Formats | Activation | Per-launch isolated? | Persistent writes needed | Selected-cheat composition | CLI | Identity | Readiness | Conf. |
|---|---|---|---|---|---|---|---|---|---|---|
| RetroArch | yes | `.cht` | file lookup + `apply_cheats_after_load` + per-entry `cheatN_enable` | via `--appendconfig` **but** exit-save leak (§5.1) | leaks unless `config_save_on_exit=false` appended | yes (entries in generated `.cht`) | `--appendconfig`, env `LIBRETRO_CHEATS_DIRECTORY` | core `library_name` + content basename | POSSIBLE_WITH_CONFIG_AND_STATE_FENCING | Medium |
| mGBA | yes | native `.cheats` (GS/AR/CB/VBA) | `-c/--cheats FILE` | yes (SDL `mgba`) | none inferred (`autosave` only set on the autoload path) | yes (file holds only selected sets) | `-c`, `-C KEY=VAL` | ROM SHA/verified id | POSSIBLE_WITH_TEMP_CONFIG | Medium (Qt frontend UNVERIFIED) |
| PCSX2 | yes | `.pnach` | `[EmuCore] EnableCheats` + `[Cheats] Enable=<names>` in settings layers | yes for activation: `-gamecfg <ini>` overrides game settings | pnach *definitions* persist in `cheats/` (inert unless named) | yes (name list) | `-gamecfg`, `-datapath`, `-portable` | serial + CRC in filename | POSSIBLE_WITH_TEMP_CONFIG | Medium-Low |
| Dolphin | yes | Gecko, AR (in game INI) | `[Core] EnableCheats` + game INI codes | partial: `-C` layer; codes need `GameSettings/<ID>.ini` in the **user dir** | user dir holds GC cards/Wii NAND | yes | `-C`, `-u` | Game ID | POSSIBLE_WITH_CONFIG_AND_STATE_FENCING | Medium |
| Xenia | yes ("patches") | `.patch.toml` | `apply_patches` cvar (default **true**), per-patch `is_enabled` | plausible: `--storage_root`/`--content_root`/`--config` cvars | patch dir, config re-save | yes | cvar flags | title id + module hash | POSSIBLE_WITH_CONFIG_AND_STATE_FENCING | Medium-Low |
| MAME | yes (XML + Lua plugin) | XML; plugin JSON | XML: UI only; **Lua**: `-autoboot_script` can write memory | yes | writes `output.xml`/`output.json` into first cheatpath entry | Lua script: yes | `-cheat`, `-cheatpath`, `-autoboot_script` | machine shortname / media CRC | POSSIBLE_WITH_TEMP_CONFIG (Lua route); XML route GUI_ONLY | Medium-Low |
| Mesen (Mesen2) | yes | settings JSON / Lua | `.lua` on CLI, `--doNotSaveSettings` | yes | settings only if not disabled | Lua yes | `.lua` arg, `--doNotSaveSettings` | none (address based) | POSSIBLE_WITH_TEMP_CONFIG | Low-Medium |
| openMSX | yes (`trainer`) | Tcl trainer defs | `-command "trainer <game> …"` | yes (`-command`, `-script`, `-setting`) | user-data trainer defs optional | yes (cheat numbers) | `-script`, `-command` | game **name** string (weak) | POSSIBLE_WITH_TEMP_CONFIG | Low-Medium |
| VICE | no cheat feature; monitor writes | monitor commands | `-moncommands <file>` | yes | none | one-shot pokes only | `-moncommands`, `-initbreak` | none | POSSIBLE_WITH_TEMP_CONFIG (limited) | Low-Medium |
| ScummVM | engine options only (Hypno) | config keys | `--config <owned ini> <target>` | yes (already implemented) | owned config only | yes | `-c/--config`, `--savepath` | `engine:game` + folder | POSSIBLE_WITH_CONFIG_AND_STATE_FENCING | Medium-High |
| Amiga WHDLoad | trainer options, not memory cheats | `CUSTOM1..5` | WHDLoad launch args/tooltypes | designed | EmuWiz-owned layer | yes | n/a (via Amiberry/FS-UAE) | slave SHA-256 + game id | POSSIBLE_WITH_TEMP_CONFIG (unwired) | Medium |
| DuckStation | yes | `.cht` (native, PCSX, libretro, EPSXe) | `[Cheats] Enable` in `gamesettings/<serial>.ini`, `EnableCheats` | no per-launch settings/override flag found | game-settings edit + `cheats/` | yes (per-code list) | none found | serial (+ hash) | PERSISTENT_STATE_RISK | Medium |
| PPSSPP | yes | CWCheat `.ini` | `EnableCheats` (**PER_GAME** setting) | not proven | cheat ini + per-game ini | yes (`_C0/_C1`) | none confirmed | `DISC_ID` | PERSISTENT_STATE_RISK | Medium |
| RPCS3 | yes ("patches") | `patch.yml` | `patch_config.yml` in config dir | XDG isolation forks `dev_hdd0`/saves | `imported_patch.yml`, `patch_config.yml` | yes | none found | serial + app version + PPU hash | PERSISTENT_STATE_RISK | Medium |
| Flycast | yes | native `cheatN_*` | file located via `emu.cfg` `[cheats]` key or `<save prefix>.cht` | no | **writes `emu.cfg` mapping on discovery/load** | yes (`cheatN_enable`) | none seen | product id (Disc 1 only) | PERSISTENT_STATE_RISK | Medium |
| Stella | yes | codes / `stella.cht` | `-cheat <code>` | CLI exists | `stella.cht` persistence of CLI codes **UNRESOLVED** | one code per flag | `-cheat` | ROM MD5 | POSSIBLE_WITH_CONFIG_AND_STATE_FENCING | Low |
| Snes9x | yes | `.cht` | cheat file + CLI parse hook | unclear | `S9xSaveCheatFile` on exit | unclear | limited | ROM | PERSISTENT_STATE_RISK | Low |
| FBNeo | yes | per-set INI | native dialog (persists) | no | INI + dialog state | yes | none found | exact shortname | GUI_ONLY | Medium |
| melonDS | yes | `.mch` | Qt CheatsDialog; sibling of ROM by default | no CLI | file beside ROM asset unless cheat path configured | yes (per-code flag) | none found | ROM SHA / game code | GUI_ONLY | Medium |
| DeSmuME | yes | `.dct` / usrcheat import | GUI (`cheatsGTK`) | no CLI found | DB file | yes | none found | serial/CRC | GUI_ONLY | Low-Medium |
| Azahar | yes | Gateway `.txt` per title id | `*citra_enabled` marker in file; UI | unknown | file in user dir | yes | none found | 16-digit title id | PERSISTENT_STATE_RISK | Medium |
| RMG | yes | `.cht` (CRC-keyed) | enabled state stored in **settings file** | no | settings file | yes | unknown | CRC1/CRC2/country or MD5 | GLOBAL_CONFIG_RISK | Medium |
| Fuse | yes (`.pok`) | `.pok` | poke selector **dialog** | partial: positional `.pok` arg | none | via UI | positional `.pok` | none | GUI_ONLY | Medium |
| SameBoy | yes (core `cheats.c`; GUI) | GameShark / Game Genie | GUI | unknown | unknown | unknown | none found | — | GUI_ONLY | Low |
| Hatari | no cheat engine | debugger scripts (`--parse`) | debugger | yes | none | ad-hoc | `--parse` | none | UNSUPPORTED (as cheats) | Medium |
| Atari800 | no | – | – | – | – | – | none | – | UNSUPPORTED | Medium |
| DOSBox (Staging/X/upstream) | no cheat subsystem found | – | – | – | – | – | `-conf` | – | UNSUPPORTED | Low-Medium |
| Amiberry / Amiberry CD | none found | – | – | – | – | – | – | – | UNSUPPORTED | Low |
| Cemu | none (graphic packs) | – | – | – | – | – | – | – | UNSUPPORTED | Low |
| Vita3K, xemu | none found | – | – | – | – | – | – | – | UNSUPPORTED | Low |
| FS-UAE | `cheats` boolean option, semantics undocumented | – | – | – | – | – | – | – | UNKNOWN | – |
| XRoar, Tsugaru, b-em, Caprice32, NP2kai, PX68k | not established | – | – | – | – | – | – | – | UNKNOWN | – |

---

## 4. Per-emulator notes

### 4.1 RetroArch — POSSIBLE_WITH_CONFIG_AND_STATE_FENCING
Full analysis in §5.1. `[SRC]` cheat file is `<cheat_database_path>/<core library_name>/<content basename>` (`cheat_manager_get_game_specific_filename`, `cheat_manager_load_game_specific_cheats`); load is invoked at content load with `apply_cheats_after_load`. Nothing is written back on load. `[REPO]` `RETROARCH_CHEAT_AUTOLOAD_PATH.md` matches. Multiple cheats: yes; conflicts: not detected by RetroArch (EmuWiz's `cheat_compatibility` can). Region/revision: none (content basename only). Headless: no evidence.

### 4.2 mGBA — POSSIBLE_WITH_TEMP_CONFIG (closest to READY)
- `[SRC]` `src/feature/commandline.c`: `-c, --cheats FILE  Apply cheat codes from a file`; `-C, --config OPTION=VALUE  Override config value`. `mArgumentsApplyFileLoads`: if `cheatsFile` is given it opens it `O_RDONLY`, `mCheatDeviceClear`, `mCheatParseFile`; only in the `else` branch does it call `mCoreAutoloadCheats`.
- `[SRC]` `core.c`: `device->autosave = true` is set **only inside `mCoreAutoloadCheats`** (when `cheatAutosave` is unset or non-zero). So with `-c` the user's `.cheats` file is neither loaded nor auto-saved over. Inference; to be proven.
- `[SRC]` `mArgumentsApplyFileLoads` is called by the SDL frontend (`src/platform/sdl/main.c:211`). No use of `cheatsFile` was found in the Qt frontend sources fetched (`main.cpp`, `Window.cpp`, `CoreController.cpp`), so **behaviour under `mgba-qt` is UNVERIFIED**. EmuWiz's `mgba_command.rs` takes an arbitrary discovered executable.
- Formats: native sets accept GameShark/PAR/CodeBreaker/VBA; mGBA is the interpreter. Saves: `.sav` beside ROM/config dirs unchanged; no redirect needed. Original ROM untouched (`--patch` not used).
- Existing EmuWiz: `mgba_cheats.rs` merges into the profile `.cheats`; no launch flag.

### 4.3 PCSX2 — POSSIBLE_WITH_TEMP_CONFIG
- `[DOC]` pcsx2.net writing-patches: `<Serial>_<CRC>.pnach`, `patches/` and `cheats/`, applied only when *Enable Cheats* on.
- `[SRC]` `QtHost.cpp` CLI: `-gamecfg <file>: Overrides the game settings with the ones in the specified INI file`, `-datapath`, `-portable`. `VMManager::GetGameSettingsPath` returns the override path when set; `Patch.cpp::ReloadEnabledLists` reads `[Cheats] Enable=` **through the (layered) settings** only when `EmuConfig.EnableCheats`. Thus a scratch game-settings INI with `[EmuCore] EnableCheats=true` and `[Cheats] Enable=<names>` gives *per-launch selected-cheat activation* without touching `PCSX2.ini` or the user's game INI (source-inferred; layer semantics not run).
- Caveats: (1) the pnach *definition file* still lives in `EmuFolders::Cheats` (EmuWiz-owned, inert unless named); (2) `-gamecfg` **replaces** the per-game INI wholesale, so the user's own per-game overrides must be copied in; (3) `-datapath` would redirect memory cards and BIOS too (Part 1's concern) — not needed for this route.
- CRC mismatch means silent non-application. Multiple cheats yes. Conflicts: not detected.

### 4.4 Dolphin — POSSIBLE_WITH_CONFIG_AND_STATE_FENCING
- `[SRC]` `-C <System>.<Section>.<Key>=<Value>` ("Set a configuration option") and `-u <user folder>`; `MAIN_ENABLE_CHEATS` = `Main/Core/EnableCheats` (default false); `Config::Save` iterates layers (whether the command-line layer is skipped was not read).
- Gecko/AR code lists live in `GameSettings/<GameID>.ini` under the **user directory** (`GAMESETTINGS_DIR` under `User`); the same user dir contains `GC` (memory cards) and `Wii` (NAND/saves) (`CommonPaths.h`). `-u <scratch>` therefore forks saves unless `GC/`, `Wii/` are passed through (symlinks) — state fencing needed.
- Alternative not evaluated: writing the game INI in the real user dir (what EmuWiz does today, persistent, journaled).
- Region/revision: Game ID + revision INI names.

### 4.5 Xenia — POSSIBLE_WITH_CONFIG_AND_STATE_FENCING
`[SRC]` `patcher.cc`: only patches whose `is_enabled` is set are applied, in memory (`memcpy` into guest heaps after protect changes); `patch_db.cc`: `apply_patches` cvar default `true`, patches read from `<patches_root>/patches`; `emulator.cc`: `Patcher(storage_root_)`; `xenia_main.cc`: cvars `storage_root`, `content_root`, transient `portable`; `config.cc`: `--config` handling and `SaveConfig()` re-saves the loaded config. A scratch `storage_root` (patch dir) with `content_root` pointed at the real content dir would isolate patches while passing saves through. Whether cvars are settable as CLI flags in the launched build was not verified.

### 4.6 MAME — POSSIBLE_WITH_TEMP_CONFIG (Lua route) / GUI_ONLY (XML route)
Follow-up result (§5.2). `[SRC]` `emuopts.h`: `-cheat`, `-cheatpath`, `-autoboot_script`, `-autoboot_delay`; `cheat.cpp`: XML cheat entries start `SCRIPT_STATE_OFF`; loads `<softlist>/<shortname>.xml`, `<machine>/<crc32>.xml`, else `<basename>.xml`; `save_all("output")` writes `output.xml` to the first `cheatpath` entry when any cheat is loaded. `[SRC]` `luaengine.cpp`: no cheat binding (0 matches). `[SRC]` `plugins/cheat/init.lua` exists (Lua cheat plugin, JSON) with per-entry `cheat.enabled`, hotkeys, menu; writes `output.json` and `<name>_hotkeys.json` into the first cheatpath entry. `[DOC]` Lua API supports `manager.machine.devices[tag].spaces[name]:write_u8(addr,val)` etc.; `[SRC]` `emu.add_machine_frame_notifier`, `emu.register_periodic`. So a generated `-autoboot_script` can write/freeze memory with no cheat XML and no UI. Requires CPU tag/address-space knowledge per machine (not in EmuWiz's XML adapter), so identity is machine-specific.

### 4.7 VICE — POSSIBLE_WITH_TEMP_CONFIG (limited)
Follow-up result (§5.3). `[DOC]` `-moncommands <Name>` executes a command file at startup ("just before the kernal reset routine" by default; `-initbreak reset|ready|<addr>` controls timing); monitor `>` (write), `fill`, `move`; `command <checknum> "<cmd>"` runs a monitor command on a checkpoint hit (the `x` command is not supported inside). No cheat/freeze/watch-loop feature and no cheat search was found in the manual text. `[REPO]` `vice_c64_cheat.rs` projects `Write8` to `radix H / > addr val / x` with a warning; not wired into `vice_command.rs`. Multiple pokes yes; persistence none; headless: `-console` exists but not verified with monitor.

### 4.8 ScummVM — POSSIBLE_WITH_CONFIG_AND_STATE_FENCING (implemented path)
`[SRC]` `base/commandLine.cpp`: `-c, --config=CONFIG  Use alternate configuration file path`, `--savepath=PATH`. Hypno exposes `GAMEOPTION_ORIGINAL_CHEATS`, `INFINITE_HEALTH`, `INFINITE_AMMO`, `UNLOCK_ALL_LEVELS` (`engines/hypno/detection.cpp`). `[REPO]` `scummvm_trainer.rs` edits an *EmuWiz-owned per-game config* via the shared transaction and the launch binding passes `--config <owned> <target>`. Fence: an alternate config file also carries `savepath`/`extrapath`; the owned config must reproduce them or saves fork. Only four Hypno keys are cheats; other engines have no cheat option in this scan.

### 4.9 Amiga (Amiberry, Amiberry CD, FS-UAE, WHDLoad)
`[REPO]` `whdload_trainer.rs` treats trainer options as launch configuration (`CUSTOM1..5` from `ws_config` declarations), verified against the slave SHA-256; never edits slave or media; writes an EmuWiz-owned option layer; `amiga_whdload` is `Supported` in routing but no launch adapter consumes the layer (`grep` found no trainer reference in `launch/amiga_whdload_command.rs`). Amiberry: no cheat-named file in its 2,419-path tree `[SRC-NEG]`. FS-UAE: `docs/options/cheats` exists ("Summary: Cheats, Type: Boolean, Default 0") with no description — **UNKNOWN semantics**; not a cheat file format. Amiga cheating is otherwise WHDLoad trainer flags or cracked-release menus.

### 4.10 DuckStation — PERSISTENT_STATE_RISK
`[SRC]` `cheats.cpp`: formats auto-detected (DuckStation native, PCSX `.cht`, libretro, EPSXe); files in `cheats/`, `patches/` (disk before embedded zip); enablement via game-settings `[Cheats] Enable`; serial + optional 16-hex game hash; disabled in hardcore/safe mode. `qthost.cpp` CLI list: `-batch -bios -fastboot -slowboot -resume -state -statefile -fullscreen -nofullscreen --` — **no settings-override flag and no cheat flag** `[SRC]`. So activation = edit `gamesettings/<serial>.ini` (persistent, per-game). `[REPO]` `duckstation_cheat.rs` implements journaled enable/disable projections; not launch-scoped.

### 4.11 PPSSPP — PERSISTENT_STATE_RISK
`[SRC]` `CwCheat.cpp`: `PSP/Cheats/<gameID>.ini`, never written back during play; `Config.cpp`: `EnableCheats` is `CfgFlag::PER_GAME` (so a per-game config can carry it, persistently). No CLI cheat flag located in `SDLMain.cpp` (grep for cheat/config/ini/memstick: none). Region: separate game ids per region.

### 4.12 RPCS3 — PERSISTENT_STATE_RISK
`[SRC]` `bin_patch.cpp`: `patches/`, `imported_patch.yml`, enablement in `patch_config.yml`; `System.cpp`: `append_global_patches()` and `append_title_patches(title_id)`; `Utilities/File.cpp::fs::get_config_dir` honours `XDG_CONFIG_HOME` on Linux, and `system_utils.cpp` places `data/`, `savestates/` (and the emulated HDD) under that config dir. So XDG redirection forks saves; enabling requires `patch_config.yml` in the real config dir. No CLI switch for patches located. Patch version `1.2` required (`[REPO-DOC]`).

### 4.13 Flycast — PERSISTENT_STATE_RISK
`[SRC]` `core/cheats.cpp`: `loadCheatFile`, `cheats` count and `cheatN_*` keys incl. `enable`; `CheatManager::reset` resolves the file with `config::loadStr("cheats", gameId)`, falls back to `.cht`/`.txt` candidates and **calls `config::saveStr("cheats", gameId, …)`** (i.e. writes the emulator config on discovery, lines ~440 and ~476); default file is `<save prefix>.cht` (next to save data). `[REPO-DOC]` Disc 2+ is refused (no disc field). Not launch-scoped.

### 4.14 Stella, Snes9x, Mesen2, SameBoy
- **Stella** `[SRC]`: help text `-cheat <code>  Use the specified cheatcode`; `OSystem.cxx` persists cheats per ROM MD5 into `stella.cht` on console close/shutdown. Whether CLI-supplied codes are saved is **unresolved** → needs fencing/proof.
- **Snes9x** `[SRC]` (summary of `unix.cpp`, limited): loads `<cheat dir>/<rom>.cht` when `Settings.ApplyCheats`, calls `S9xParseArgsForCheats()`, and `S9xSaveCheatFile` on exit → anything applied is written back.
- **Mesen2** `[SRC]` `CommandLineHelper.cs`: `.lua` arguments are queued as scripts; `--doNotSaveSettings` ("Prevent settings from being saved to the disk"); `--testRunner [lua script] [rom]` headless. Cheat manager exists in Core; cheat list persists in settings (`CheatCodes.cs`) — so the Lua route (memory writes) is the isolated one.
- **SameBoy** `[SRC-NEG/SRC]`: `Core/cheats.c`, `cheat_search.c`, Cocoa cheat UI; no cheat CLI found → GUI_ONLY, low confidence.

### 4.15 FBNeo, melonDS, DeSmuME, Azahar, RMG
- **FBNeo** `[REPO-DOC]` (cites FBNeo wiki): per-set INI, exact shortname, native dialog persists choice; `[SRC-NEG]` no CLI.
- **melonDS** `[REPO-DOC]`: `.mch` derived from the ROM asset path (sibling file) unless a cheat path is configured; Qt `CheatsDialog`; global `EnableCheats`. A sibling file would be written **into the user's ROM directory**, conflicting with source-tree preservation unless the configured cheat path is used. melonDS Qt `main.cpp` shows `CLI::ManageArgs` with ROM/boot/fullscreen options only.
- **DeSmuME** `[SRC-NEG]`: `cheatSystem.cpp` + GTK `cheatsGTK.cpp`; no CLI cheat evidence.
- **Azahar** `[REPO-DOC]`: `cheats/<TitleID>.txt`, `*citra_enabled` per-entry marker; Android/Qt UI.
- **RMG** `[SRC]` `Cheats.cpp`: shared `Cheats/` + user `Cheats-User/` `.cht` files keyed `CRC1-CRC2-C:Country` or MD5; **enabled state stored in the settings file** as `"Cheat \"name\" Enabled"` under `[MD5] Cheats`.

### 4.16 Fuse, openMSX, Hatari, Atari800, DOSBox, others
- **Fuse** `[SRC]` `pokefinder/pokemem.c`: `.pok` loading (`pokemem_read_from_file`, `pokemem_autoload_pokfile`), set from a positional `.pok` argument (`fuse.c`) or auto-found beside media (`pokemem_find_pokfile`, `utils.c`); selection is via the poke-selector **dialog** (`ui_pokemem_selector`). Auto-finding a sibling `.pok` would read the media directory; an explicit scratch path avoids that. EmuWiz has no `.pok` code.
- **openMSX** `[SRC]` `CommandLineParser.cc` registers `-setting`, `-control`, `-script`, `-command`; `share/scripts/_trainer.tcl` implements a `trainer <game> [<cheat>…]` command that re-applies on a repeat timer (`after`) and deactivates on `boot`/`machine_switch`; definitions are sourced from system data and **user data** `_trainerdefs.tcl`. Identity is a game *name* argument (weak). Redirecting `OPENMSX_USER_DATA` per launch would also hide user-installed machines/extensions — do not.
- **Hatari** `[SRC]` `options.c`: `--parse <file>` "Parse/execute debugger commands from <file>", `--saveconfig`; no cheat engine `[DOC]`. Debugger poke grammar not read; scripted pokes are one-shot and outside "cheats".
- **Atari800** `[DOC]` `DOC/USAGE`: no cheat option or file; monitor via F8 only.
- **DOSBox** `[SRC-NEG]` no cheat-named path in Staging (1,791 paths) or DOSBox-X (8,797 paths); `[REPO-DOC]` `LAUNCH_OPTION_TRAINER_FAMILY_RESEARCH.md` explicitly rejects `[autoexec]`/`-c` as a trainer channel (arbitrary command execution).
- **Cemu, Vita3K, xemu, Amiberry** `[SRC-NEG]` no cheat-named source files in the trees scanned (1,812 / 1,583 / 12,160 / 2,419 paths). Cemu's graphic packs are a mod/patch mechanism that is not a cheat subsystem and persists in its settings.

---

## 5. Follow-ups requested

### 5.1 RetroArch: can a generated per-launch `.cht` be used without touching the normal database/config? — *not proven; unsafe as-is*
Evidence (all `[SRC]`, RetroArch master):
1. **Lookup:** `cheat_manager_load_game_specific_cheats(path_cheat_database)` → `<db>/<core library_name>/<content basename>` (`cheat_manager.c:811-850`). Called via `command_event_init_cheats(apply_cheats_after_load, path_cheat_database, …)` at content load (`retroarch.c:8957`).
2. **Ways to redirect `cheat_database_path` for one launch:** (a) `--appendconfig=FILE` (help text `retroarch.c:7286`; merged in `config_load` at `configuration.c:6724-6760`); (b) environment variable **`LIBRETRO_CHEATS_DIRECTORY`**, applied *after* config load and overriding `paths.path_cheat_database` (`configuration.c:7010`); no CLI flag names a cheat file.
3. **The persistence problem:** on exit, `retroarch_main_quit` calls `command_event(CMD_EVENT_MENU_SAVE_CURRENT_CONFIG)` when `config_save_on_exit` is true (setting defined at `configuration.c:2135` with default constant `DEFAULT_CONFIG_SAVE_ON_EXIT`; the constant's value was not read, so "on by default" is UNVERIFIED here), which calls `command_event_save_config(path_get(RARCH_PATH_CONFIG))` unless `RUNLOOP_FLAG_OVERRIDES_ACTIVE` (`command.c:2468-2495`). `config_save_file` serialises the **live settings struct** into the real config path. The values from `--appendconfig` and the `LIBRETRO_*` env overrides are in that struct, so a scratch `cheat_database_path`, `savefile_directory`, `system_directory` would be **written into the user's `retroarch.cfg`** on a normal quit.
4. **Mitigation candidate:** append `config_save_on_exit = "false"` in the same generated config (also `config_save_on_exit` is a normal boolean setting, `configuration.c:2135`). Not proven: the append merges in the same load, but nothing was run.
5. `[REPO]` **Consequence for the existing projection:** `retroarch_resource_projection.rs` generates only `system_directory` and `savefile_directory`, and does *not* set `config_save_on_exit=false`. By the reading above, materialised launches may leak the scratch `system_directory`/`savefile_directory` into `retroarch.cfg` on exit. This is a finding to verify, not a confirmed bug; it was not modified.
6. Runtime network command interface (UDP `NETWORK_CMD`) was not researched; its documented commands were not checked for cheat operations. `[UNKNOWN]`.

Conclusion: RetroArch is the best-supported *shape* (real save passthrough grant, generated file, marker, cleanup) but is **not** READY until an empirical test shows `retroarch.cfg` and the real cheat database are byte-identical after exit with the mitigation applied.

### 5.2 MAME follow-up
| Question | Result |
|---|---|
| Lua can activate XML cheat entries? | **No binding found.** `luaengine.cpp` has 0 "cheat" matches; docs.mamedev.org `ref-core` lists no cheat API. `[SRC]` `[DOC]` |
| Any script/command interface toggling XML cheats? | Only the UI cheat menu, and the Lua **cheat plugin** (`plugins/cheat/init.lua`, `plugin.json` `start:false`, JSON cheats with `cheat.enabled`, hotkeys, menu). Plugin activation without UI was not established. |
| Writable dedicated output path? | `output.xml` (core) and `output.json`/`*_hotkeys.json` (plugin) are written to the **first** `cheatpath` entry (`emu_file(machine().options().cheat_path(), OPEN_FLAG_WRITE…)`; plugin uses `cheatpath:value():match("([^;]+)")`). Pointing `-cheatpath` at a scratch directory isolates them; cheat files must be in that scratch dir (or a second search-only entry after it). |
| Programmatic memory writes? | **Yes**, via Lua `-autoboot_script` and `spaces[...]:write_*` (`[DOC]` ref-mem; `[SRC]` option and frame notifiers). This bypasses cheat XML entirely. |
Classification: XML route GUI_ONLY; Lua-script route POSSIBLE_WITH_TEMP_CONFIG (per-machine address spaces required; not tested).

### 5.3 VICE follow-up
| Capability | Result |
|---|---|
| Watch/freeze | Checkpoints (`break`, `watch`, `trace`) with `condition`, and `command <n> "<cmd>"`, exist, but they are debugger checkpoints that stop into the monitor; `x` is unsupported in `command`. No documented freeze/"trainer". |
| Monitor scripting loops | Not documented in the fetched manual. |
| Cheat snapshot/database | None found. |
| Per-launch command file | Yes: `-moncommands <file>` and `-initbreak`. |
| External live poke | `-binarymonitor` (documented memory-set protocol) allows an external tool to poke repeatedly; that is a separate process/protocol, not a VICE cheat feature. |
Classification: one-shot poke only; **not equivalent to a persistent cheat**. `[DOC]` `vice.texi` sections on `-moncommands`, `-initbreak`, checkpoint commands.

### 5.4 Isolation side-effect analysis (does isolating config also redirect state?)

| Emulator | Isolation lever | Also redirects saves/cards/NVRAM | Other state redirected | Pass-through needed |
|---|---|---|---|---|
| RetroArch | `--appendconfig` / env | only what the config names; real save dir is granted via `savefile_directory` in EmuWiz's projection | `system_directory` (BIOS), cheat db, and exit-save leak (§5.1) | `config_save_on_exit=false`; savefile/savestate dirs |
| PCSX2 | `-gamecfg` | no (game settings layer only) | replaces user per-game INI | copy user per-game INI in |
| PCSX2 | `-datapath` | **yes** (memcards, BIOS, screenshots, logs, inis) | full profile | not needed for cheats; avoid |
| Dolphin | `-u` | **yes** (GC cards, Wii NAND/saves, screenshots, cache, controller profiles, game INIs) | everything under `User/` | link `GC/`, `Wii/`, `Config/` |
| Dolphin | `-C` | no | none (layer) | — |
| Xenia | `--storage_root` | patches, plugins, cache, config; content in `content_root` | config re-save | set `--content_root` to real |
| RPCS3 | `XDG_CONFIG_HOME`/config dir | **yes** (`dev_hdd0`, savestates, captures, recordings, logs, `patch_config.yml`) | all config | do not isolate |
| ScummVM | `--config` | only if config omits `savepath`; `--savepath` overrides | config write-back on exit | reproduce `savepath`/`extrapath` |
| mGBA | `-c` file only | no | none | none |
| MAME | `-cheatpath`, `-autoboot_script` | no | none | ensure scratch first entry |
| Mesen2 | `--doNotSaveSettings` | no | settings unsaved | none |
| openMSX | `-command`/`-script`/`-setting` | no | avoid redirecting `OPENMSX_USER_DATA` (machines/extensions) | — |
| DuckStation / PPSSPP / Flycast / Azahar / Stella / Snes9x / RMG | none found | n/a | n/a | n/a |

Metadata/achievement notes: DuckStation, PCSX2 and PPSSPP disable cheats in achievements hardcore mode (`[SRC]`), so an achievement-enabled profile silently ignores cheats — a state EmuWiz must surface, not fence. Screenshots/logs/caches follow the redirected root when a root is redirected (Dolphin `-u`, PCSX2 `-datapath`, RPCS3 config dir).

---

## 6. `safe_launch_sandbox` review (design doc) and the shipped primitives

`safe_launch_sandbox` **does not exist as code on main**; only `docs/research/SAFE_LAUNCH_SANDBOX_V1.md` (design). Review of the *design*:
- **Isolates:** private scratch copies of source *media* (never the source, fail-closed), plus an isolated config area. Config isolation is modelled as `XdgEnvironment` (HOME/XDG_* redirected into the workspace, "starts empty or seeded"), `ExplicitConfigPath{flag}` or `Combined`.
- **Refuses:** `HardDisk` and `Optical` kinds in V1; `DirectReadOnly` only for independently audited adapters; adapters that do not declare which state is persistent must not opt in.
- **Generalizable for cheat-config-only use?** Its media-copy core is irrelevant to cheats. Its config isolation is *empty-by-default*, which is exactly the "isolation forks saves" problem this audit found for Dolphin `-u`, PCSX2 `-datapath`, RPCS3. It requires a `persistent_state` declaration per adapter (design Task I/J) but that field is not part of the shipped API.
- **Save pass-through / copy config while saves stay real?** Not supported by the V1 design as written (seeding only from reviewed profiles, "never a path into the user's real profile", saves discarded). Cannot bind real save locations.
- **Cleanup/verification:** design has marker, lease, `CapturedFileIdentity` before/after, bounded lifetime, disk-space guard via `assess_storage`.
- **Verdict on the design: unsuitable as-is for cheats; reusable with extension (concepts only).**

What is actually usable on main: `resource_grants` + `retroarch_resource_projection` are **reusable with extension** as the per-launch primitive: typed grants for `SAVE_DATA` `READ_WRITE` pass-through, `CONFIG` `GENERATED_FILE`/`CONFIG_OVERRIDE`, `LAUNCH_ONLY` lifetime, marker-guarded recursive cleanup restricted to an approved root, and a symlink-safe materialiser. It is RetroArch-specific in its materialiser (`RetroArchResourceRequest`, fixed layout); a generic materialiser and adapter-declared grants are missing. `resource_grants.rs` documents itself as "does not project paths … or enforce access at the OS boundary" (`READ_ONLY` is intent).

---

## 7. Existing cheat architecture on main (to reuse, not replace)

| Component | What it provides | Reuse for launch-scoped cheats |
|---|---|---|
| `shared_preview.rs`, `shared_transaction.rs`, `destination_safety.rs`, `import_safety.rs` | preview → confirm → atomic publish → backup → journal → rollback, path/symlink safety | **Yes for persistent definition files** (PCSX2 pnach, RetroArch `.cht` library, MAME XML). Not for launch-scoped: transactions publish and remain. |
| `cheat_ir.rs` + decoders (`action_replay`, `classic_game_genie`, `n64_gameshark`, `saturn_action_replay`, DS AR) | neutral `Write8/16/32`, opaque retention, no guessing | **Yes**: the single source for "what is a direct write". |
| `cheat_route.rs` | selected-emulator routing, `CheatApplySupport` | **Yes**: extend with a launch-scope capability. |
| `cheat_loadability.rs` | "will the emulator load it" using file bytes, path convention, enable setting, restart requirement | **Yes**: extend per-emulator conventions (§5.1, §4.3). |
| `cheat_compatibility.rs` | conflict/stack analyser (memory ranges, conditions, master codes, revision evidence) | **Yes**: answers "conflicting cheats detectable" (emulators generally do not). |
| Per-emulator native modules (`duckstation_cheat`, `mgba_cheats`, `flycast_cheats`, `melonds_cheat`, `mednafen_cheat`, `mame_cheat`, `fbneo_cheat`, `rpcs3_patch`, `ppsspp_cwcheat`, `three_ds_cheat`) | bounded parse/render/merge/per-entry enable | **Yes**: renderers can emit the *selected-only* file into scratch. |
| `vice_c64_cheat.rs`, `whdload_trainer.rs`, `scummvm_trainer.rs` | already launch-configuration-shaped (runtime projection or owned config) | **Yes**: closest to per-launch already; only `scummvm` is wired. |
| GUI `CheatEmulatorAdapter` (`RetroArch|Pcsx2|Dolphin|Xenia|Unsupported`, `cheats_mods/state.rs`) | GUI routing | Note: **lags core** `cheat_apply_support`; the GUI enum does not yet name DuckStation, PPSSPP, mGBA, MAME, RPCS3, Flycast, ScummVM. |

---

## 8. Final architectural conclusions

**A. Top 5 safest adapter candidates (technical readiness, not popularity)**
1. **mGBA** (`-c FILE` on SDL `mgba`): no config, no redirect, saves untouched; prove `mgba-qt` behaviour and no `.cheats` autosave.
2. **ScummVM** (already implemented trainer + `--config`): only four Hypno options are cheats, so the value is narrow; needs the `savepath` fence.
3. **PCSX2** (`-gamecfg` scratch INI for enable + name list; inert pnach definition): selected-cheat activation without global config.
4. **MAME** (Lua `-autoboot_script` + scratch `-cheatpath`): per-launch, no persistence expected, but per-machine addresses and unproven.
5. **RetroArch** (`--appendconfig` or `LIBRETRO_CHEATS_DIRECTORY` + real save pass-through already built): highest reach, but blocked on the `config_save_on_exit` leak proof. (openMSX/Mesen2 are next; both are Lua/Tcl-script routes and rank below because identity is weak.)

**B. Must remain blocked from automatic (launch-time) application**
DuckStation, PPSSPP, RPCS3, Flycast, Azahar, Snes9x, Stella (until persistence resolved), RMG (settings file), and all GUI-only or UNKNOWN targets (FBNeo, melonDS, DeSmuME, SameBoy, Fuse, FS-UAE, XRoar, Tsugaru, b-em, Caprice32, NP2kai, PX68k). Also Dolphin `-u` and PCSX2 `-datapath` as *isolation mechanisms* (they fork saves). Persistent installs through the shared transaction remain acceptable **as installs** with explicit user confirmation; they must not be presented as launch-scoped.

**C. Shared primitives required before implementing adapters**
1. A generic **per-launch workspace + materialiser** built on `resource_grants` (generalise `retroarch_resource_projection`), with adapter-declared grants for `SAVE_DATA`/`MEMORY_CARD`/`NVRAM`/`CONFIG`.
2. An **explicit cheat-launch capability declaration** per adapter: `activation` (CLI file / config layer / script / UI / none), `persists_on_exit` (yes/no/unproven), `state_redirected` (list), `identity` (kind), `headless`.
3. **Selected-cheats-only composition** (render only chosen entries via the native modules; per-entry enable flags where the format has them).
4. **Post-exit verification** (hash real config, cheat DB, and save dirs before/after; report drift) and deterministic cleanup with an approved-root guard.
5. **Persistence proof harness** (a fixture-driven run per emulator) — required to move any adapter to READY.
6. **Identity/region guard** shared across serial/CRC/title-id/hash/machine-name, reusing `cheat_compatibility` revision evidence.
7. **Side-effect fencing table** (§5.4) as data, plus a check for achievements/hardcore modes that silently disable cheats.

**D. Should `patch_manager` be reused for cheat application?**
Yes for **format modules, IR, routing, loadability, conflict analysis and persistent-install transactions**. No for launch-scoped state: `execute_shared_apply` publishes and journals permanent files; per-launch application needs a separate, non-journaled, self-cleaning lifecycle that *calls* patch_manager renderers. Do not duplicate parsers.

**E. Should `safe_launch_sandbox` be extended into a config-only/per-launch-state primitive?**
No: it is only a design, and its empty-by-default XDG isolation is the wrong shape for cheats. Build the config-only primitive on `resource_grants` + the RetroArch projection (typed grants, save pass-through, generated files, marker-guarded cleanup); borrow the sandbox design's `CapturedFileIdentity` before/after check, lease, and disk-space guard. Keep the media scratch-copy design as a separate concern.

**F. Isolation class required**
- **Config isolation only:** mGBA, MAME (scratch `-cheatpath` + Lua), Mesen2, openMSX (`-command`), VICE (`-moncommands`), PCSX2 (`-gamecfg`).
- **Config + save pass-through:** RetroArch (real `savefile_directory`, plus `config_save_on_exit=false`), ScummVM (owned config must carry `savepath`), Xenia (`content_root` real).
- **State fencing:** Dolphin (`-u` overlay with linked `GC/`, `Wii/`), Stella (`stella.cht`), Snes9x (`.cht` write-back), RPCS3 (do not redirect XDG), Flycast (`emu.cfg`).
- **UI automation (out of scope, not recommended):** MAME XML cheats, FBNeo, melonDS, DeSmuME, SameBoy, Fuse poke dialog, DuckStation/PPSSPP toggles.
- **No feasible safe integration:** DOSBox (arbitrary-command channels only), Atari800, Hatari (no cheat engine), Cemu, Vita3K, xemu, Amiberry, and UNKNOWN systems until researched.

**G. Duplicated cheat architecture already present that the Cheat Core Batch must reuse**
`cheat_ir` and the decoders, `cheat_route` + `cheat_apply_support`, `cheat_loadability`, `cheat_compatibility`, the shared transaction stack, and the per-emulator native modules. Two overlapping routing tables already exist and should be unified rather than extended a third time: core `cheat_route.rs` vs GUI `CheatEmulatorAdapter`/`cheat_adapter_route`. Also note that `docs/research/CHEAT_FORMAT_ADAPTER_COVERAGE_AUDIT.md` (untracked in the main worktree, dated at an older SHA `f42c88f5`) is now stale in places (it lists MAME, mGBA, DuckStation, PPSSPP, RPCS3, Flycast as missing/read-only).

---

## 9. Sources and limitations

Upstream source (raw GitHub master, read 2026-09-29): RetroArch `cheat_manager.c`, `retroarch.c`, `configuration.c`, `command.c`; MAME `emuopts.h/.cpp`, `frontend/mame/cheat.cpp`, `luaengine.cpp`, `plugins/cheat/init.lua`; Dolphin `UICommon/CommandLineParse.cpp`, `Core/Config/MainSettings.cpp`, `Common/CommonPaths.h`, `Common/Config/Config.cpp`; PCSX2 `pcsx2-qt/QtHost.cpp`, `pcsx2/VMManager.cpp`, `pcsx2/Patch.cpp`; DuckStation `core/cheats.cpp`, `duckstation-qt/qthost.cpp`; PPSSPP `Core/CwCheat.cpp`, `Core/Config.cpp`, `SDL/SDLMain.cpp`; RPCS3 `Utilities/bin_patch.cpp`, `Utilities/File.cpp`, `rpcs3/Emu/System.cpp`, `system_utils.cpp`; Xenia Canary `patcher.cc`, `patch_db.cc`, `emulator.cc`, `xenia_main.cc`, `config.cc`; mGBA `feature/commandline.c`, `core/core.c`, `platform/sdl/main.c`; Stella `Settings.cxx`, `OSystem.cxx`; Flycast `core/cheats.cpp`; Mesen2 `UI/Utilities/CommandLineHelper.cs`; openMSX `CommandLineParser.cc`, `share/scripts/_trainer.tcl`; Hatari `src/options.c`; Fuse `fuse.c`, `utils.c`, `pokefinder/pokemem.c`; ScummVM `base/commandLine.cpp`, `engines/hypno/detection.cpp`; RMG `Source/RMG-Core/Cheats.cpp`; Snes9x `unix/unix.cpp` (summary only). Official docs: pcsx2.net/docs/advanced/writing-patches, docs.mamedev.org (`ref-mem`, command-line), vice-emu.sourceforge.io manual, Atari800 `DOC/USAGE`, docs.libretro.com cheat guide (mechanics not covered).
Repository: `docs/research/{SAFE_LAUNCH_SANDBOX_V1, EMULATOR_FILE_ISOLATION_ARCHITECTURE_AUDIT, RETROARCH_CHEAT_AUTOLOAD_PATH, LAUNCH_OPTION_TRAINER_FAMILY_RESEARCH, *_NATIVE_CHEAT_ADAPTER, VICE_C64_CHEAT_ADAPTER, WHDLOAD_TRAINER_CHEAT_ADAPTER, FLYCAST_DREAMCAST_NATIVE_CHEATS, AZAHAR_3DS_NATIVE_CHEATS}.md`; code under `crates/archivefs-core/src/{launch,patch_manager}/`.

**Dead links / gaps handled:** ScummVM and FS-UAE doc URLs returned 404 → replaced by upstream source (`commandLine.cpp`, `docs/options/cheats`); DOSBox Staging docs host unresolvable → file-tree scan + repo research doc; mesen.ca docs 404 → source; PCSX2 `Config.cpp` path 404 → `QtHost.cpp`/`Patch.cpp`/`VMManager.cpp`.
**Not established:** any dynamic behaviour (nothing was run); Qt-frontend behaviour for mGBA; Xenia cvars-as-flags in the shipped build; PPSSPP/DuckStation alternate-config flags beyond those listed; Snes9x details (summary only); Stella CLI-cheat persistence; RetroArch network command interface; XRoar, Tsugaru, b-em, Caprice32, NP2kai, PX68k cheat facilities (classified UNKNOWN; b-em/Caprice32/NP2kai/PX68k have only audits, no launch adapter, on main).
