> **STATUS NOTE (recovered 2026-10-03; recovery baseline main `d112b7f3`).**
> This is the design document referenced by `crates/archivefs-core/src/launch/cheat_launch_plan.rs` (and by `docs/cheats/SAFE_LAUNCH_COMPOSITION.md`), recovered verbatim from branch `design/cheat-per-launch-workspace` (`70dbaefe`, 2026-09-29, written against main `4c980d18`).
> - Current main has the *planner* half of this design (`cheat_launch_plan.rs`: `plan_cheat_launch`, a RetroArch-only command composer `command_with_cheat_launch_plan`, and `capture_baseline`/`verify_expectation`) plus persistent per-emulator staging/install adapters under `patch_manager/`.
> - Current main does **not** have the runtime executor: nothing materialises the per-launch derivative, spawns with it, verifies, or cleans up, and the planner has no product caller (see `docs/research/CHEAT_RUNTIME_CURRENT_STATUS.md`).
> - Parsing, normalising, validating, staging/installing (levels 1-4) and a config-based "the emulator should load it" check are **not** proof that a cheat works in-game. No level 6-7 evidence exists.
> - Where implementation code differs from this historical design, the code is authoritative. Statements below about "current" architecture describe main `4c980d18`, not today.

---

*Historical document follows unchanged.*

# Cheat Per-Launch Workspace and State-Fencing Architecture (V1 design)

> **Status: design only.** No production Rust, GUI, `main`, adapter, or test was changed. Nothing was pushed.
> **Baseline:** `origin/main` @ `4c980d184584dd5f1a22b5fbbf67e6c58ff204c1` (local `main` == `origin/main`, tracked tree clean). Research input: `docs/research/CHEAT_EMULATOR_CAPABILITY_MATRIX.md` (Part 2, commit `e43ae942`, on branch `research/cheat-capability-audit-2`, not yet on main).
> **Evidence tags:** `[REPO]` read in this repo at the baseline · `[SRC]` upstream source read on 2026-09-29 (raw GitHub master) · `[DOC]` official documentation · `[INFER]` reasoned from the above, **not run** · `[UNKNOWN]`.
> **Nothing was executed.** Conclusions marked VERIFIED mean "verified by reading source/code", never "observed at runtime". Where runtime proof is required the document says so and lists it in §16.

---

## 1. Current architecture (facts)

### 1.1 The per-launch primitive that exists: `resource_grants` + one consumer
`crates/archivefs-core/src/launch/resource_grants.rs` (548 lines) `[REPO]`:
- **What a grant is.** A declarative `LaunchResourceGrant { launch_id, role, source_path, presented_path, access, projection, lifetime, scope, provenance, reason }`. Its module doc says it "deliberately does not project paths, change permissions, or enforce access at the operating-system boundary. It describes intent for a later launch executor and rejects internally contradictory contracts." So a grant is *intent + validation*, not an isolation mechanism. `READ_ONLY` is intent (`retroarch_resource_projection.rs` says the same: "FI1 does not claim that a symlink by itself is an OS sandbox").
- **Vocabulary.** Roles: `GameMedia, BiosFirmware, DependencyRom, DeviceRom, Config, Profile, SaveData, MemoryCard, Nvram, Nand, HddImage, Cache, TemporaryRuntime, Metadata, Artwork, Unknown`. Access: `ReadOnly, ReadWrite, CreateOnly`. Lifetime: `LaunchOnly, Session, Persistent`. Projection: `DirectPath, SymlinkFile, SymlinkDirectory, ReadOnlyBind, BindMount, Reflink, ScratchCopy, TempCopy, GeneratedFile, GeneratedDirectory, ConfigOverride, NoProjection`. Scope: `StrictMinimal, NarrowDirectory, EmulatorProfileRoot, LegacyBroadAccess`.
- **Validation rules** (`LaunchResourceGrant::validate`): projections that read a source require `source_path`; every projection except `NoProjection` requires `presented_path`; generated/`ConfigOverride`/`NoProjection` grants must **not** carry a source; paths must be absolute and traversal-free; symlink/bind/copy projections may not alias source==presented (note `DirectPath` is exempt, which is how saves pass through); `GeneratedFile` cannot be `ReadWrite`; `GeneratedDirectory` cannot be `ReadOnly`; **`SaveData/MemoryCard/Nvram/Nand/HddImage` may not be writable with `LaunchOnly` lifetime** (persistent user state must be declared `Persistent`); `LegacyBroadAccess` with `NoProjection` is invalid.
- **Set semantics** (`LaunchResourceGrantSet`): a `presented_path` may appear once (identical duplicates are collapsed; different grants for the same presented path are `PresentedPathConflict`). This is the whole collision-avoidance model; there is no cross-launch or cross-process collision model.
- **Callers.** `grep -rn 'resource_grants\|LaunchResourceGrant' crates` finds only `launch/mod.rs` re-exports (`:120`, `:385-397`) and `retroarch_resource_projection.rs` plus its tests. **No production caller** and no GUI/CLI caller exists. `spawn`/`execution` never invoke it.

### 1.2 RetroArch consumer: `retroarch_resource_projection.rs` (899 lines)
Traced in full `[REPO]`:
- **Plan** (`plan_retroarch_resource_grants`, pure): grants `GameMedia` (`DirectPath`, `ReadOnly`, `LaunchOnly`), `TemporaryRuntime` system directory (`GeneratedDirectory`, `CreateOnly`, `Session`) at `<launch_root>/retroarch/system`, one `BiosFirmware` symlink per `VerifiedMatch` BIOS (`SymlinkFile`, `ReadOnly`, `Session`; ambiguous/filename-only evidence is refused; the BIOS root is never granted), `SaveData` (`DirectPath`, `ReadWrite`, `Persistent`, source == presented == the real save directory), and `Config` (`GeneratedFile`, `CreateOnly`, `LaunchOnly`) at `<launch_root>/retroarch/config/emuwiz-append.cfg`.
- **What the append config contains** (`config_contents`, line ~305): exactly two keys:
  ```
  system_directory = "<launch_root>/retroarch/system"
  savefile_directory = "<real save dir>"
  ```
  Nothing else: no `savestate_directory`, `cheat_database_path`, `apply_cheats_after_load`, `config_save_on_exit`, screenshot, cache, remap, shader, or content/core override keys.
- **Materialise** (`materialize_retroarch_resource_plan`): validates the grant set, requires `launch_root` to be under `std::env::temp_dir()/emuwiz/retroarch-launches` (`approved_retroarch_launch_root`), not a symlink, writes an ownership marker `.emuwiz-retroarch-launch` containing `EMUWIZ_RETROARCH_LAUNCH\n<launch_id>\n` (refuses if a marker already exists), creates directories, symlinks BIOS files (source must be a regular non-symlink file), writes the append config with `create_new` + `sync_all`, and checks (does not create) that the save directory exists and is a real directory. It returns a receipt listing created paths. It is RetroArch-specific: unknown projections are ignored (`_ => {}`), and the `GeneratedFile` branch requires the destination to equal `plan.config_path`.
- **Cleanup** (`cleanup_retroarch_projection`): re-validates the approved root and non-symlink root, checks the marker's existence, type and launch id, then `remove_dir_all` (which removes symlink entries without following them). It is a plain function the *caller* must invoke; there is no `Drop` guard, no process binding and no stale-tree sweeper.
- **Command** (`command_with_retroarch_resource_plan`) appends `--appendconfig <path>` **after** the existing `-L <core> <content>` arguments (`retroarch_command.rs:292-296` builds the base argv). Upstream RetroArch's optstring (`"hs:fvVS:A:U:DN:d:e:…"`, `retroarch.c:7822`) does not begin with `+`, so glibc `getopt_long` permutes options after positionals `[SRC]`; RetroArch's bundled compat getopt on non-glibc platforms was not checked `[UNKNOWN]`.
- **Ownership today:** `system_directory` and the append config are EmuWiz-owned (launch-only). `savefile_directory` is user state passed through. Everything else (savestates, screenshots, cheat database, remaps, shaders, cache, core options, content history, logs) remains whatever the user's real `retroarch.cfg` and RetroArch defaults say, i.e. **real-profile-owned and uncontrolled**.

### 1.3 Process lifecycle: `process_spawn.rs`
`spawn_watched_process(&PreparedProcessCommand)` `[REPO]`: argv only (no shell), stdin/stdout null, stderr piped and bounded to 64 KiB, **"No environment variables are injected or overridden"**, no timeout, no kill; a background thread only drains stderr and `wait()`s, reporting `ProcessExitReport { status, stderr }`; the caller polls `WatchedProcess::poll()` (GUI-frame safe). `PreparedProcessCommand` has only `{executable, arguments, working_directory}`. Consequences: env-based redirection (`LIBRETRO_CHEATS_DIRECTORY`, `XDG_*`, `OPENMSX_USER_DATA`) is currently **not expressible**; exit handling is a polled callback owned by whoever holds the `WatchedProcess`; if EmuWiz itself dies the watcher thread dies with it and nothing cleans the scratch tree.
`CapturedFileIdentity {device, inode, size, modified}` + `capture_file_identity` is the existing cheap freshness primitive (also mirrored in `shared_transaction`).

### 1.4 Cheat stack in `patch_manager` (relevant subset)
`cheat_ir`, decoders (`action_replay`, `classic_game_genie`, `n64_gameshark`, `saturn_action_replay`), native per-emulator modules (`duckstation_cheat`, `mgba_cheats`, `flycast_cheats`, `melonds_cheat`, `mednafen_cheat`, `mame_cheat`, `fbneo_cheat`, `rpcs3_patch`, `ppsspp_cwcheat`, `three_ds_cheat`, `vice_c64_cheat`, `whdload_trainer`, `scummvm_trainer`), `cheat_route` (routing + `CheatApplySupport`), `cheat_loadability` (post-install "will it load" facts), `cheat_compatibility` (conflict analyser), and the persistent-install stack `shared_preview` → `shared_transaction` (`build_shared_transaction_plan`, `execute_shared_apply`, `execute_shared_rollback`, journal + backups under `default_shared_history_root/backup_root`). All apply paths publish **permanent** files and journal them; none is launch-scoped.

### 1.5 The Cheat Core Batch as found in the repository
| Batch | Where it lives on/after the baseline |
|---|---|
| 1 parser hardening | branch `fix/retroarch-cheat-parser-hardening` (1 commit ahead of main: `7a47de05`, `cht_document.rs`, `retroarch_inventory.rs`, `user_cheat_import.rs`, `docs/cheats/RETROARCH_PARSER_HARDENING.md`) |
| 2 duplicate/conflict handling | on main (`cheat_compatibility.rs`, `docs/research/CHEAT_COMPATIBILITY_CONFLICT_ANALYSER.md`); branch `feature/cheat-duplicates-conflicts` is at main |
| 3 provenance | branch `feature/cheat-provenance-evidence` (`f0713e5b`, 27 files, `docs/cheats/PROVENANCE_EVIDENCE.md`) |
| 4 applicability | branch `feature/cheat-applicability-status` (`b605a269`, `cheat_applicability.rs` 810 lines, `docs/CHEAT_APPLICABILITY_STATUS.md`) |
| 5 safe launch composition | **no branch, commit or document was found** |
| 6 pack preview | **no branch, commit or document was found** |
Batches 5 and 6 are therefore unreviewed here; §14 states the contract Batch 5 should consume rather than a diff review.

---

## 2. Verified problems (each backed by evidence above)

| # | Problem | Evidence | Severity |
|---|---|---|---|
| P1 | The RetroArch projection can leak scratch paths into the user's `retroarch.cfg` on normal exit (§3). | `[SRC]` | High |
| P2 | `resource_grants` has **no verification or ownership-of-cleanup contract**; "grant" ≠ enforcement. | `[REPO]` | High |
| P3 | Cleanup is a caller-invoked function; no `Drop`, no process binding, no stale sweeper; scratch under `$TMP` with default permissions (no explicit `0700`). | `[REPO]` `create_dir_all`/`create_dir` | Medium |
| P4 | The materialiser is RetroArch-specific; generic grants (e.g. `GeneratedDirectory` for cheat material) are not executed for other adapters. | `[REPO]` | Medium |
| P5 | `spawn_watched_process` cannot inject environment, so env-based redirection is unavailable; and gives no exit hook beyond polling. | `[REPO]` | Medium |
| P6 | No way to declare a *protected, must-not-change* real file (`NoProjection` forbids a source path), so post-exit verification has no data model. | `[REPO]` `validate` | High |
| P7 | The launch path is not wired: nothing calls the projection, and **no cheat content** is ever composed at launch except ScummVM's `--config` trainer. | `[REPO]` | Info |
| P8 | Routing/capability truth is duplicated in ≥6 hard-coded tables and they disagree (§11). `cheat_apply_support` says mGBA is `Supported` but mGBA has no `PreviewAdapter` and applies through its own private plan/rollback (`apply_mgba_cheat_plan`, `rollback()`). | `[REPO]` | High |
| P9 | The append-config precedence is *base < append < user core/content/game override files* (§3), so a user's per-game override can silently repoint any key EmuWiz sets (including `savefile_directory`, `cheat_database_path`). | `[SRC]` | High for RetroArch |
| P10 | Session/Launch lifetime semantics are ambiguous: EmuWiz-owned generated dirs and BIOS links are declared `Session` although cleanup happens per launch; `LaunchOnly` is used for the config and media. | `[REPO]` | Low |

---

## 3. RetroArch exit-save hypothesis — verification

**Question.** Can `retroarch --appendconfig scratch.cfg` persist values from `scratch.cfg` into the user's real `retroarch.cfg` on normal exit?

**Answer A (persistence possible): STRONGLY_SUPPORTED.** Source-inferred, not run.

Evidence chain (RetroArch master, files fetched 2026-09-29):
1. `config_save_on_exit` is a normal boolean setting (`configuration.c:2135`) with `#define DEFAULT_CONFIG_SAVE_ON_EXIT true` (`config.def.h:699`).
2. `retroarch_main_quit()` (`retroarch.c:9652+`) reads `settings->bools.config_save_on_exit` and, if true, issues `CMD_EVENT_MENU_SAVE_CURRENT_CONFIG` (`:9677` static builds, `:9761` `HAVE_DYNAMIC` builds). `CMD_EVENT_SHUTDOWN` does the same (`:4993`, `:5010`).
3. `command_event_save_current_config(OVERRIDE_NONE)` (`command.c:2468-2495`) calls `command_event_save_config(path_get(RARCH_PATH_CONFIG))` **unless** `RUNLOOP_FLAG_OVERRIDES_ACTIVE` is set (then it logs "overrides active, not saving").
4. `config_save_file(path)` (`configuration.c:8931`) opens the existing file (to preserve unknown keys), then writes every known setting from the **live `settings_t`** (`populate_settings_*`). It returns early only when `RUNLOOP_FLAG_OVERRIDES_ACTIVE` (`:8969`). The region `8931-9640` contains **no reference to append config** (0 matches for `append/APPEND`), so it cannot distinguish base-file values from appended values.
5. `--appendconfig` values are merged into the same `settings_t` during `config_load_file` (`configuration.c:6724-6760`, `config_append_file`), so they are in the object being saved.
6. The **path settings loop** (`configuration.c:9115-9163`) writes every path setting unconditionally (except keychain-sensitive ones). `override`-gating (`retroarch_override_setting_is_set`) exists only in the array/float/int/uint/size/bool loops (`:9221-9415`) and applies to values set by dedicated CLI flags (e.g. `-s`, `-S`, verbosity), not to `--appendconfig`.

**B. Values that may leak:** every known setting the append file sets: all path settings (so **`system_directory`, `savefile_directory`, `cheat_database_path`** if set), booleans such as `apply_cheats_after_load`, and `config_save_on_exit` itself. In *minimal* mode (`config_save_minimal`) only values differing from defaults are written, which does not help here since scratch values differ from defaults. Existing EmuWiz projection: `system_directory` (a scratch path that will not exist next launch) and `savefile_directory` (the real one, harmless) are the two exposed keys. The stale `system_directory` would make BIOS loading fail on the *next normal* RetroArch start. `[INFER]`

**C. Does command-line append config alter the active config object that is saved?** Yes (STRONGLY_SUPPORTED): appended values populate `settings_t`, `config_save_file` serialises `settings_t`.

**D. Does `config_save_on_exit = "false"` inside the append config reliably prevent persistence? STRONGLY_SUPPORTED for the two quit-time saves; UNCERTAIN as a *reliable* guarantee.**
- It works because `config_save_on_exit` is read from the merged live settings at quit time (`retroarch.c:9657`), i.e. after the append merge.
- Precedence hazard (P9): `config_load_file` merges **base → `--appendconfig` (`:6724-6760`) → override files** (`RARCH_PATH_CONFIG_OVERRIDE`, `:6770-6795`); `config_load_override` (`:7581+`) stacks the user's core/content-dir/game `.cfg` overrides and explicitly "prevents `--appendconfig` from being ignored" by re-appending, but the override files still come **after**. A user's per-game override can therefore set `config_save_on_exit = true`, `savefile_directory`, or `cheat_database_path` and beat EmuWiz. When any override is active `RUNLOOP_FLAG_OVERRIDES_ACTIVE` suppresses the quit-time save (`command.c`, `configuration.c:8969`), which incidentally protects `retroarch.cfg` in that case; but EmuWiz's intended save directory is no longer guaranteed. The projection must therefore *detect* existing `config/<core>/<core>.cfg`, `config/<core>/<content-dir>.cfg`, `config/<core>/<game>.cfg` and treat them as a blocker or a declared conflict.
- Not runtime-tested; a mis-ordering in a particular RetroArch build cannot be excluded.

**E. Other config-write paths on exit (identified, not all audited):**
- `config_save_file_salamander()` unconditionally on non-`HAVE_DYNAMIC` builds (`retroarch.c:9673-9678`); irrelevant for typical Linux desktop builds `[INFER]`.
- `input_remapping_deinit(settings->bools.remap_save_on_exit)` (`retroarch.c:4102`; `DEFAULT_REMAP_SAVE_ON_EXIT true`, `config.def.h:705`) — remap files in the remap directory, which the projection does not redirect; unaudited whether it writes when unchanged.
- User-driven menu actions (Save Current/New Config, Save Core/Game Override, Save Game Cheats) — user-initiated; not fenceable but observable.
- Not audited: core options file (`retroarch-core-options.cfg`), content history/favourites playlists, runtime logs, SRAM/state auto-save (intended user state).

**F. Crash vs clean shutdown:** the config write is in the graceful quit/shutdown path only; `SIGKILL`/crash performs no write, so a crash cannot leak config, but it also skips EmuWiz's own cleanup and leaves the scratch tree (P3). Signal handling for `SIGTERM/SIGINT` (whether it routes through `retroarch_main_quit`) was not read `[UNKNOWN]`.

**Overall classification: STRONGLY_SUPPORTED that the current projection leaks `system_directory` (and any future `cheat_database_path`) into the real `retroarch.cfg` on a normal exit when the user has no active override. Not VERIFIED at runtime. Not DISPROVEN.** The mitigation `config_save_on_exit="false"` is STRONGLY_SUPPORTED, not proven. No code was changed; `docs/research/CHEAT_EMULATOR_CAPABILITY_MATRIX.md` §5.1 already states this correctly (its claim that the default constant was unread is now resolved: it is `true`), so the matrix was **not modified** (it lives on `research/cheat-capability-audit-2`, not on this branch); only its "constant unread" caveat is now resolved.

Additional RetroArch facts used by this design `[SRC]`:
- Cheat load is `cheat_manager_load_game_specific_cheats(path_cheat_database)` → `<db>/<core library_name>/<content basename>` (`cheat_manager.c:811-850`); `command_event_init_cheats` (`command.c:1803`) **loads whenever the content loads** (skipped for netplay data-init or BSV movie) and only *applies* enabled entries when `apply_cheats_after_load` is true (default `false`, `config.def.h:1508`). So both `cheat_database_path` and `apply_cheats_after_load` must be set, and `cheatN_enable=true` per selected entry.
- Env override `LIBRETRO_CHEATS_DIRECTORY` is applied after config load (`configuration.c:7010`) but is unusable today (P5) and would leak the same way.

---

## 4. Resource ownership model

Project conventions are kept: existing `LaunchResourceRole/Access/Lifetime/Projection/Scope` are unchanged. The design adds one **derived classification**, `LaunchStateClass`, computed from role + lifetime + access (plus one new role, `CheatMaterial`, and one new non-grant concept, `ProtectedReference`, §7). It is the vocabulary for fencing and verification rules.

| State class | Existing roles / grants | Examples | Owner |
|---|---|---|---|
| `ReadOnlySource` | `GameMedia, BiosFirmware, DependencyRom, DeviceRom`, **new `CheatMaterial`** (source cheat file) | ROM, disc, BIOS, user's `.cht`/`.pnach`/`.cheats` | user/library |
| `RealUserState` | `SaveData, MemoryCard, Nvram, Nand, HddImage` (`Persistent`, `ReadWrite`, `DirectPath`/`Symlink*`) | `.srm`/`.sav`, PS2 memcards, Wii NAND, Xbox HDD, ScummVM saves | user |
| `EphemeralConfig` | `Config` with `GeneratedFile`/`ConfigOverride`, `LaunchOnly` | append config, `-gamecfg` INI, MAME Lua script, mGBA cheat file, ScummVM owned ini | EmuWiz |
| `EphemeralRuntime` | `TemporaryRuntime`, `Cache` with `GeneratedDirectory`, `LaunchOnly` | scratch dirs, MAME `output.xml`, transient logs | EmuWiz |
| `PersistentOptional` | `Cache`, `Artwork`, `Metadata`, `Profile` subsets | screenshots, shader cache, controller mappings, remaps, core options, playlists | user/emulator; passthrough by default |
| `ProtectedConfig` (**new, verification-only**) | `Config`/`Profile` real files | `retroarch.cfg`, `PCSX2.ini`, Dolphin `Config/`, `xenia-canary.config.toml`, `stella.cht`, user's `.cheats` | user; must remain unchanged |

Permitted operations (✔ allowed, ✘ forbidden, ◐ conditional):

| Class | Read directly | Bind/pass through | Copy | Generate | Discard | Reconcile | Verify unchanged |
|---|---|---|---|---|---|---|---|
| `ReadOnlySource` | ✔ | ✔ `ReadOnly` (symlink file, never dir of unrelated data) | ◐ only for disposable scratch media (out of scope here) | ✘ | ✘ | ✘ | ✔ identity+size+mtime (hash for small cheat files) |
| `RealUserState` | ✔ | ✔ `DirectPath`/link, **never redirected** | ✘ (a copy forks progress) | ✘ | ✘ | ✘ (emulator writes are the point) | ◐ existence + not truncated/deleted; *changes expected* |
| `EphemeralConfig` | n/a | ✘ | ✘ | ✔ | ✔ always | ✘ | n/a |
| `EphemeralRuntime` | n/a | ✘ | ✘ | ✔ | ✔ always | ✘ | n/a |
| `PersistentOptional` | ✔ | ✔ default passthrough | ◐ per adapter policy | ✘ | ✘ | ◐ | ◐ report-only |
| `ProtectedConfig` | ✔ (baseline only) | ✘ | ◐ *seed* copy into an ephemeral file when the emulator requires a complete config (e.g. `-gamecfg`) | ✘ | ✘ | ◐ restore-from-baseline (Stella, §6.6) | ✔ **key-scoped** |

Invariants:
- I1 **No path handed to an emulator ever names a `ProtectedConfig` file for writing** unless the emulator has no alternative *and* the adapter declares `restore_on_exit` (Stella-style), which is a `Reconcile` not a passthrough.
- I2 **A `RealUserState` grant is never `Copy`, `Scratch`, `LaunchOnly`-writable** (already enforced by `validate`).
- I3 **Only `EphemeralConfig/Runtime` are recursively deleted, only under the approved root, only with a matching marker** (generalising `cleanup_retroarch_projection`).
- I4 Every generated file is created with `create_new`, mode `0600`, inside a `0700` per-launch directory.

`resource_grants` **can naturally express** real saves + temporary config + temporary cheat material + temporary cache **without copying the profile**: `SaveData/DirectPath/ReadWrite/Persistent`; `Config/GeneratedFile/CreateOnly/LaunchOnly`; new `CheatMaterial/GeneratedFile/CreateOnly/LaunchOnly`; `Cache/GeneratedDirectory/CreateOnly/LaunchOnly`. It **cannot** express a protected real file (P6) or an expectation about process exit; those are added as sibling types (§7), not by bending `NoProjection`.

---

## 5. Lifecycle

```
Plan (pure)            adapter capability + selected cheats + applicability guard → grants + expectations
Preflight              re-verify identity (CapturedFileIdentity), detect blockers (RetroArch overrides, running emulator, hardcore)
Baseline capture       fingerprints for ReadOnlySource, ProtectedConfig (key-scoped), RealUserState (existence/size)
Materialise            create 0700 launch root + marker (lease incl. EmuWiz pid + start time), generate files, links
Spawn                  spawn_watched_process(argv [+ env, once supported])
Watch                  poll() until ProcessExitReport (owner: launch session object)
Post-exit verify       compare against baseline → LaunchVerificationReport
Cleanup                delete EphemeralConfig/Runtime under approved root (marker-checked)
Report                 surface warnings/violations; write a launch receipt for diagnostics
```
State machine with abnormal paths:
- **Materialise failure:** roll back created paths (receipt-driven) and do not spawn (fail closed).
- **Spawn failure:** cleanup, no verification needed (nothing ran).
- **Non-zero exit / crash:** verification still runs (files may have changed), cleanup still runs; the report flags `AbnormalExit`.
- **EmuWiz dies during the launch:** the tree remains. Required: **startup sweeper** that scans the approved root, reads each marker + lease (`pid`, process start time, `launch_id`), and deletes trees whose owning EmuWiz process is gone *and* whose emulator process is not running; trees younger than a grace period are skipped. Sweeper also reports "unverified launch" for those, since verification could not run.
- **Cleanup failure** (`PartialDeletion`): retain the tree, surface a diagnostic, retry in the sweeper; never widen deletion scope.
- **Concurrency:** launch roots are per `launch_id` (marker refuses reuse). Two launches touching the *same* real save directory are not fenced by the grant model; the workspace layer should take an advisory per-`SaveData` path lock file (`.emuwiz-launch.lock` **outside** the save dir, e.g. in EmuWiz state) and refuse or warn on overlap. `[INFER]` Design choice; not existing behaviour.
- **Ownership of the session:** a `LaunchWorkspaceSession` (name provisional) owns plan, receipt, watched process and baseline; its `Drop` never deletes recursively while the emulator may be running — cleanup is explicit after exit, with `Drop` limited to logging "left for sweeper".

---

## 6. State fencing

### 6.1 Rules
- **F1 Redirect only what you must.** Prefer an emulator's *layered override* (game-settings file, `-C` layer, `--appendconfig`, `--config`) over relocating a data root.
- **F2 A relocated root implies a fence list.** If a lever moves a root (`-u`, `-datapath`, `--storage_root`, `XDG_*`, `-basedir`), every child that is `RealUserState` or `PersistentOptional` must be passed through by link/explicit setting, or the lever is rejected for that emulator.
- **F3 Config for the emulator must be complete when it *replaces* rather than *layers*** (PCSX2 `-gamecfg`, ScummVM `--config`): seed it from the real file, then modify.
- **F4 Save directories are pinned explicitly** in the generated config wherever the emulator has such a key (RetroArch `savefile_directory`/`savestate_directory`, ScummVM `savepath`, Xenia `content_root`).
- **F5 Key-scoped protection:** protected configs are verified by *keys*, not bytes, because emulators rewrite their configs every run (Xenia `SaveConfig()`, RetroArch, PCSX2).
- **F6 Detect user overrides that outrank the launch layer** (RetroArch override files; PCSX2 per-game INI replaced by `-gamecfg`; ScummVM per-target keys) and surface them.
- **F7 Achievement/hardcore modes silently disable cheats** (DuckStation, PCSX2, PPSSPP `[SRC]`): the applicability guard must warn; EmuWiz cannot fence this.

### 6.2 RetroArch — config layer + save passthrough + suppress exit save
- Append config *must* contain: `config_save_on_exit = "false"`; `savefile_directory`/`savestate_directory` = the real directories; `system_directory` = launch view; `cheat_database_path` = `<launch_root>/retroarch/cheats`; `apply_cheats_after_load = "true"`. Cheat file: `<launch_root>/retroarch/cheats/<core library_name>/<content basename>.cht` containing **only the selected entries** with `cheatN_enable = "true"`.
- Also set `remap_save_on_exit = "false"` (proposed; not audited) and never redirect remap/shader/screenshot/config directories: they are `PersistentOptional` passthrough.
- Blockers/warnings (F6): existing `config/<core>/*.cfg` overrides; netplay/BSV; running RetroArch instance sharing the config; hardcore/achievements.
- Verify: `retroarch.cfg` key probes (`system_directory`, `savefile_directory`, `savestate_directory`, `cheat_database_path`, `apply_cheats_after_load`, `config_save_on_exit`) equal baseline; the real cheat database directory listing/`<core>/<content>.cht` unchanged.

### 6.3 Dolphin — do not use `-u` as the cheat lever
- Levers `[SRC]`: `-C <System>.<Section>.<Key>=<Value>` (command-line config layer) enables `Main.Core.EnableCheats`; code lists live in `GameSettings/<GameID>.ini` under the *user directory*, which also holds `GC` (memory cards), `Wii` (NAND/saves), `Config`, `Cache`, etc.
- **Definition and activation share one file** (`[Gecko]` + `[Gecko_Enabled]` in the same INI), so a launch-scoped enablement needs a *different user dir* holding a generated INI. Fence list for `-u <scratch>`: link `GC/`, `Wii/`, `Config/`, `Load/`, `ScreenShots/` etc. from the real user dir; *create* `GameSettings/` in scratch holding copies of the real INIs plus the generated one. `[INFER]`
- Verdict: capability is `PersistentInstallOnly` today (current EmuWiz install path); `LaunchScopedConfig` **only** after a proof harness demonstrates the linked overlay does not fork saves and Dolphin does not rewrite through the links in a harmful way (it will legitimately update `Config/Dolphin.ini` through the link — an expected write, key-probed).

### 6.4 PCSX2 — layered game settings, no data-root move
- Lever `[SRC]`: `-gamecfg <file>` sets `s_game_settings_override`; `VMManager::GetGameSettingsPath` returns it instead of `<GameSettings>/<serial>_<crc>.ini`; enabled cheat names come from `[Cheats] Enable=` read through the layered settings only when `EmuConfig.EnableCheats` (`Patch.cpp:583-590`). So the scratch INI carries `[EmuCore] EnableCheats=true` + `[Cheats] Enable=<names>`.
- Fence: `-datapath`/`-portable` are **not** used (they move memory cards, BIOS, inis). F3: seed the scratch INI from the user's real per-game INI (if any) because `-gamecfg` *replaces* it. The pnach *definition* remains a persistent, journaled, EmuWiz-owned file in `EmuFolders::Cheats` (`PersistentInstallOnly` component); it is inert unless named.
- Verify: real `PCSX2.ini` key probes (`EnableCheats`, `Folders`), the real per-game INI unchanged (PCSX2 may write the *override* file if the user edits per-game settings; that goes to scratch and is lost — surfaced as an info note), memory-card directory listing unchanged in existence, pnach definition hash == journal.

### 6.5 Xenia — two roots
- Levers `[SRC]`: cvars `storage_root`, `content_root`, `portable`, and `--config`; `apply_patches` default `true`; patches read from `<storage_root>/patches` (`Patcher(storage_root_)`); config re-saved by `SaveConfig()`.
- Fence: scratch `storage_root` containing only `patches/` (selected patch TOML) **and** `content_root` pinned to the real content directory (where saves/profile content live), otherwise saves fork. If the launch also depends on other `storage_root` children (cache, plugins) they must be linked. Whether cvars are accepted as CLI flags in the shipped build, and which file `SaveConfig()` writes when `--config` is scratch, are `[UNKNOWN]`.
- Verify: real `xenia-canary.config.toml` key probes; content directory untouched (listing).

### 6.6 ScummVM — owned config must carry the save keys
- Levers `[SRC]`: `-c/--config=CONFIG`, `--savepath=PATH`. `[REPO]` `ScummVmTrainerLaunchBinding` already emits `--config <owned> <target>`; `scummvm_trainer.rs` edits an *EmuWiz-owned per-game* ini and states global configs are not accepted.
- Gap G1: who seeds that owned file? ScummVM's `savepath`, `extrapath`, `themepath`, `plugins_path` live in the global `[scummvm]` section (and per-target keys). If the owned file lacks them, saves go to ScummVM's default and **fork**. Fence: seed `[scummvm]` global keys from the real `scummvm.ini` (F3) or pass `--savepath=<real>` explicitly (F4) — recommend both. ScummVM rewrites the alternate config on exit (expected write to an EmuWiz-owned file).
- Verify: the real `scummvm.ini` unchanged; real save directory not emptied.

### 6.7 Stella — the persistence-bearing CLI
- `[SRC]` `-cheat <code>`: `CheatManager::loadCheats` parses CLI codes into the active list; `saveCheats(md5)` serialises **the whole list including CLI codes**, compares to the stored entry (`changed` is true) and sets `myListIsDirty`; `saveCheatDatabase()` on shutdown writes `stella.cht`. **STRONGLY_SUPPORTED that CLI cheats persist** for that ROM MD5. `-basedir <path>` overrides "the base directory for all config files" (so `stella.cht` and config move; `-statedir`, `-userdir` are separate keys; nvram location unread `[UNKNOWN]`).
- Fence options: (a) `-basedir <scratch>` with passthrough/seed of config and nvram (fork risk for nvram/state); (b) **restore-from-baseline reconciliation**: capture `stella.cht` before launch, after exit compare; if the only difference is the expected CLI-derived entry for this MD5, restore the baseline bytes; if the user changed cheats in the UI (unexpected difference) keep the file and warn. Option (b) is preferred because it moves nothing; it needs `Reconcile` support and a crash caveat (crash before `saveCheatDatabase` leaves nothing to reconcile).
- Verdict: `LaunchScopedMemoryCommands`-like (CLI code) **with `RequiresStateFencing`**, not READY.

### 6.8 Side-effect table (what redirection would also redirect)

| Lever | Also redirects | Required fence |
|---|---|---|
| RetroArch `--appendconfig` | only keys present; but exit-save writes them back | `config_save_on_exit=false`; pin save/state dirs |
| PCSX2 `-gamecfg` | replaces per-game INI | seed from real per-game INI |
| PCSX2 `-datapath` | memcards, BIOS, inis, snaps, logs | do not use |
| Dolphin `-u` | GC cards, Wii NAND, Config, caches, screenshots | link all but `GameSettings` |
| Dolphin `-C` | nothing (layer) | none |
| Xenia `storage_root` | patches, plugins, cache | `content_root` = real |
| RPCS3 config dir / `XDG_CONFIG_HOME` | `dev_hdd0`, savestates, `patch_config.yml` | do not use |
| ScummVM `--config` | global `[scummvm]` keys | seed + `--savepath` |
| Stella `-basedir` | config, `stella.cht`, (nvram?) | prefer reconcile instead |
| mGBA `-c FILE` | nothing | none (SDL frontend only, unverified for Qt) |
| MAME `-cheatpath`, `-autoboot_script` | first cheatpath entry receives `output.xml`/`output.json` | scratch first entry |

---

## 7. Post-exit verification contract

### 7.1 Data model (conceptual)
```
LaunchStateExpectation {
    path            : absolute path (real or scratch)
    class           : LaunchStateClass
    expectation     : MustRemainUnchanged | MayChange | MustNotExistAfter | MustExistAfter | KeysMustEqual(keys)
    fingerprint     : FileIdentity            // device, inode, size, mtime (existing CapturedFileIdentity)
                    | Sha256                  // small files only (cheat sources, configs < 1 MiB)
                    | KeyProbe { format, keys -> baseline values }   // INI/CFG/TOML/YAML key subset
                    | DirectoryProbe { names: allowlist, exists, not_truncated }   // targeted; no recursive hashing
    severity        : Info | Warning | LaunchAffecting | Corruption
    remediation     : None | RestoreFromBaseline | Quarantine | AskUser
}
LaunchVerificationReport { launch_id, exit_status, results: Vec<{expectation, outcome, detail}>, unverified: Vec<reason> }
```
Expectations are produced by the *capability declaration* (§8) and the plan, not hand-written per launch.

### 7.2 Expected outcomes
- **MUST remain unchanged:** ROM/media (`FileIdentity`, plus SHA-256 only if identity changed), source cheat files (SHA-256), *protected emulator config* (`KeyProbe`, F5), the user's own per-game INI/override files.
- **MAY change:** normal save files, memory cards, SRAM, NAND (recorded as `Changed` with size/mtime for the diagnostic; never an error), emulator-owned churn files listed in an allowlist (e.g. `Dolphin.ini` non-cheat keys, `xenia-canary.config.toml` re-save).
- **EPHEMERAL:** generated config, scripts, scratch cheat material, `output.xml`/`output.json` — asserted present-then-deleted, or reported as `LeftBehind`.
- **Targeted evidence, not whole-directory hashing:** directories use `DirectoryProbe` (named children exist, not shorter than baseline). A save directory is never hashed; only "not emptied/truncated" (`size >= baseline` for append-only formats is *not* assumed).

### 7.3 Surfacing violations
| Finding | Severity | Action |
|---|---|---|
| Media identity changed | Corruption | post-run diagnostic + persistent banner on the game; **no automatic rollback** (evidence preservation); offer re-verify against DAT |
| Source cheat file changed | Warning | diagnostic; re-hash |
| Protected config key drift (cheat-relevant key) | LaunchAffecting | diagnostic; `RestoreFromBaseline` only for keys EmuWiz proves it wrote (e.g. leaked `cheat_database_path`); otherwise ask user |
| Protected file changed by the emulator only in allowlisted churn | Info | none |
| `stella.cht` gained the CLI entry | Warning | `RestoreFromBaseline` if difference == expected entry; else keep + warn |
| Scratch tree left behind | Warning | sweeper retry |
| Save unexpectedly missing/truncated vs baseline | Corruption | diagnostic; direct user to Save Vault (existing `SAVE_RESTORE_GAP_AUDIT`) |
| Verification could not run (EmuWiz crashed) | Warning | sweeper marks `UnverifiedLaunch` |
Pre-launch failures (identity changed since preview, blocker present, root not approved, marker conflict) are **launch failures** (fail closed). Post-run findings are diagnostics; rollback exists only where EmuWiz *wrote* the change (leaked config keys, Stella reconcile, persistent install through the existing shared journal).

---

## 8. Canonical cheat-launch capability declaration

One core-owned record per emulator adapter (proposed location: alongside `cheat_route.rs`, e.g. `patch_manager/cheat_capability.rs`; **not implemented here**), keyed by the existing stable adapter id (`"pcsx2"`, `"retroarch"`, …), replacing hard-coded per-emulator tables.

```
CheatEmulatorCapability {
    adapter_id            : &'static str                    // same keys as launch::platform_map / cheat_route
    display_name          : &'static str
    modes                 : set<CheatLaunchMode>
    formats               : set<CheatNativeFormat>          // ChtRetroArch, Pnach, MgbaCheats, ...
    identity              : CheatIdentityRequirement        // ExactSerialAndCrc | TitleId+Hash | Md5 | CoreAndContent | MachineName | ...
    region_revision       : RegionRevisionPolicy            // FromIdentity | NameEncoded | None
    multiple_selected     : bool
    conflict_detection    : EmulatorDetects | EmuWizAnalyserOnly
    requires_derivative   : bool                            // a generated file/script is produced from selection
    isolation             : set<IsolationNeed>              // ConfigLayer | SaveDirPin | SeedConfigCopy | LinkOverlay | None
    save_passthrough      : SavePassthrough                 // NotApplicable | PinnedByConfig | LinkedDirs | NotRedirected
    persistent_writes     : Vec<PersistentWrite>            // e.g. RetroArchExitSave{keys}, StellaCht, Flycast emu.cfg, PnachDefinition
    cleanup               : CleanupStrategy                 // DeleteScratchTree | RestoreBaseline | JournalRollback | None
    persistence_proof     : Proof                           // Unproven | ProvenByHarness(version)
    achievements_disable  : bool
    headless              : Tri                             // Yes | No | Unknown
    route                 : CheatRouteFacts                 // platforms, process-name fragments, loadability conventions
}
enum CheatLaunchMode {
    Unsupported,
    PersistentInstallOnly,        // shared_transaction install, user-confirmed, journaled
    LaunchScopedConfig,           // generated config/layer file only
    LaunchScopedScript,           // generated script (MAME Lua, Mesen Lua, openMSX Tcl)
    LaunchScopedMemoryCommands,   // generated command file/args (VICE -moncommands, Stella -cheat, mGBA -c)
    GuiOnly,
    RequiresStateFencing,         // modifier: cannot be used unless fences in `isolation` are honoured
    Unknown,
}
```
Rules:
- `READY` (adapter may be auto-applied at launch) requires **all** of: `persistence_proof = ProvenByHarness`, `isolation` satisfied by the workspace, `identity` resolvable to an exact verified identity, `cleanup` deterministic, and selected-cheats-only composition. This encodes the matrix's READY_FOR_SAFE_ADAPTER criteria as a machine-checkable predicate, and today the predicate is false for every adapter.
- `PersistentInstallOnly` is *not* launch-scoped and must be presented as an install (existing UX), never as "applied for this launch".
- The declaration lives beside `cheat_route`, reuses `CheatRouteTarget`/`CheatApplySupport`, and *derives* the existing tables (§9).

---

## 9. Reconciling the routing/capability tables

### 9.1 Inventory of hard-coded decisions (all `[REPO]`)
| # | Table | Location | Keys |
|---|---|---|---|
| T1 | `cheat_apply_support` | `patch_manager/cheat_route.rs:~120-135` | `Supported`: RetroArch + `pcsx2, dolphin, xenia, duckstation, ppsspp, mgba, mame, rpcs3, flycast, amiga_whdload, scummvm`; everything else `Unsupported` (`InventoryOnly` variant unused in that match) |
| T2 | GUI `CheatEmulatorAdapter` via `cheat_adapter_for_decision` | `archivefs-gui/src/cheats_mods/render.rs:1687-1701`, `state.rs:595` | `RetroArch, Pcsx2, Dolphin, Xenia`, all else `Unsupported` |
| T3 | `PreviewAdapter` + `adapter_write_support` | `shared_preview.rs:29+`, `shared_transaction.rs:152-172` | 17 variants all `ApplyAndRollback` incl. `MelonDs, Mednafen, Fbneo, LocalModPackage, CemuGraphicPack, Rpcs3OrdinaryMod`; **no `Mgba`, no `Azahar`** |
| T4 | `standalone_display_name`, format label | `cheat_route.rs:~95-150` | per-id strings incl. `mgba` |
| T5 | `process_name_fragments` | `cheat_loadability.rs:~565-580` | 10 ids (no `scummvm`, `amiga_whdload`) |
| T6 | `platform_map::LAUNCH_COMPATIBILITY.standalone_adapters` and `STANDALONE_OWNED_PLATFORMS` | `launch/platform_map.rs`, `cheat_route.rs:~30` | platform → adapters |
| T7 | Doc-side lists (`CHEAT_FORMAT_ADAPTER_COVERAGE_AUDIT.md`, matrix) | docs | stale by construction |

### 9.2 Disagreements
| Emulator | T1 (core) | T2 (GUI workflow) | T3 (shared transaction) | Note |
|---|---|---|---|---|
| duckstation, ppsspp, rpcs3, flycast, mame | Supported | Unsupported | ApplyAndRollback (variant exists) | GUI has no install workflow for them; core says they can apply |
| mgba | **Supported** | Unsupported | **no variant** | apply exists as private `build_mgba_cheat_apply_plan/apply_mgba_cheat_plan` with its own `rollback()`: a *third* install stack that bypasses journal/history |
| scummvm, amiga_whdload | Supported | Unsupported | ApplyAndRollback (`ScummVmTrainer`, `AmigaWhdloadTrainer`) | trainers, not memory cheats; launch-wired only for ScummVM |
| melonds, mednafen, fbneo | Unsupported | Unsupported | ApplyAndRollback | modules exist, route says no |
| azahar | Unsupported | Unsupported | no variant | parse/merge only (consistent, but `process_name_fragments` knows it) |
| xenia, pcsx2, dolphin, retroarch | Supported | Supported | ApplyAndRollback | consistent |
| vice | not routed | – | – | projection exists, unwired |

Note the GUI does already *call* the core router (`route_cheat_install`, `applicable_target`); the GUI enum is a **workflow selector** layered on top, which is why its narrower set silently overrides core truth (a `Supported` decision maps to `Unsupported` and "offers no apply").

### 9.3 Authority and migration
- **Authoritative:** the core `CheatEmulatorCapability` registry (§8), a `const` table of records keyed by adapter id. `cheat_route`, `cheat_loadability`, `shared_transaction::adapter_write_support` and `standalone_display_name` become *projections* (functions over the registry). The GUI consumes `CheatRouteDecision` + `CheatEmulatorCapability` and selects a **generic** workflow from `modes`; the `CheatEmulatorAdapter` enum shrinks to a UI-rendering hint (`InstallWorkflow(kind)`), never a capability statement.
- **Migration (no behaviour change first):**
  1. Add the registry with values copied from T1/T3 and a **consistency test** asserting every T1 `Supported` id has a T3 variant (fails today for `mgba`: forces an explicit decision) and every T3 variant maps to an id or is explicitly `Mod`.
  2. Re-express T1/T4/T5 as registry projections; keep public signatures.
  3. Add the GUI projection test asserting `Supported && has_workflow ⇔ GUI adapter != Unsupported`; then add generic workflows for adapters that already have `ApplyAndRollback`.
  4. Decide mGBA: either add `PreviewAdapter::Mgba` and move its apply into the shared journal, or downgrade T1 to `InventoryOnly` until then.
  5. Delete T7 hand-maintained lists in docs in favour of a generated table.

---

## 10. `patch_manager` boundary

**Stays in `patch_manager` (format & persistent-install domain):**
format parsing/rendering/merge (`*_cheat.rs`, `cht_document`, `pcsx2_pnach`, `gecko_document`, `xenia_patch_document`); neutral IR and decoders; validation; reconciliation and duplicate/conflict analysis (`cheat_compatibility`, `cheat_reconciliation_plan`); provenance/applicability (Batches 3/4); routing facts and the **capability registry**; loadability facts; persistent installs and their journaling/rollback (`shared_preview`, `shared_transaction`); *rendering the selected-only derivative bytes* (a pure function `selection → bytes`).

**Moves OUT to `launch/` (per-process domain):**
per-launch workspace ownership and directories; generated launch config/script *files on disk*; save passthrough and pinning; process lifetime and exit handling; environment injection (once added); baseline capture and post-exit verification; stale-tree sweeping; launch receipts. `launch/` calls patch_manager's pure renderers and reads the capability registry; `patch_manager` does not spawn, delete recursively, or touch save paths.

**Seam:** `patch_manager` returns `CheatLaunchDerivative { relative_name, bytes, kind, sha256 }` (pure, deterministic, size-bounded); `launch` decides where it lives and grants it (`CheatMaterial/GeneratedFile`). No `SharedTransactionPlan` is created for launch-scoped material.

---

## 11. Emulator flows (conceptual; nothing implemented)

Common shape: `selected cheats → applicability guard → capability lookup → workspace resources → generated config/script/file → emulator process → post-exit verification → cleanup`.

### 11.1 RetroArch (proof adapter candidate)
1. Selection: chosen `.cht` entries (from `cht_document`), exact `core library_name`, content basename.
2. Guard: exact core evidence (existing `retroarch_core_required`), identity strength (Batch 4), no netplay/BSV, no override `.cfg` conflict, hardcore, conflict analysis (Batch 2).
3. Capability: `LaunchScopedConfig` + `RequiresStateFencing`; `persistent_writes=[RetroArchExitSave]`; proof required.
4. Resources: `GameMedia` (ro), `SaveData` real (rw, existing), `Config` append (gen), `TemporaryRuntime` system + cheats dir (gen), `CheatMaterial` `<db>/<core>/<content>.cht` (gen).
5. Generated: append cfg incl. `config_save_on_exit=false`, `cheat_database_path`, `apply_cheats_after_load=true`; selected-only `.cht` with `enable=true`.
6. Spawn: existing `-L core content --appendconfig cfg`.
7. Verify: `retroarch.cfg` key probes; real cheat DB untouched; media identity.
8. Cleanup: delete launch root.

### 11.2 PCSX2
Selection: pnach cheat names → guard: verified serial + CRC (`pcsx2_identity`), hardcore → capability `LaunchScopedConfig` (`-gamecfg`), plus the persistent pnach definition (`PersistentInstallOnly` component with journal) → resources: seeded `Config` scratch INI (`[EmuCore] EnableCheats`, `[Cheats] Enable=names`), no data-root move → args `-gamecfg <ini>` → verify `PCSX2.ini` probes, real per-game INI unchanged, pnach hash == journal, memcards untouched → cleanup scratch INI.

### 11.3 mGBA (closest to READY)
Selection: sets from `mgba_cheats` → guard: ROM SHA-256/verified identity, **frontend is SDL `mgba`** (Qt unverified → block) → capability `LaunchScopedMemoryCommands` (`-c FILE`) → resources: `CheatMaterial` scratch `.cheats` (only selected sets, states enabled), no config → args `-c <file>` (optionally `-C cheatAutosave=0`) → verify user `.cheats` unchanged (SHA-256), ROM identity, save files may change → cleanup file.

### 11.4 MAME
Selection: cheats reduced to **direct writes** (`cheat_ir`) with machine-specific `(cpu tag, space, address)` → guard: exact machine shortname; address-space map required (not currently in `mame_cheat`) → capability `LaunchScopedScript` → resources: scratch `Config` Lua script + scratch `-cheatpath` dir as `EphemeralRuntime` (absorbs `output.xml`/`output.json`) → args `-autoboot_script <lua> -cheatpath <scratch>` → verify real cheat dir untouched, media identity → cleanup scratch. The XML cheat route stays `GuiOnly`.

### 11.5 ScummVM (already half-built)
Selection: Hypno option toggles → guard: verified `engine:game` → `LaunchScopedConfig` + `RequiresStateFencing` (save keys) → resources: seeded owned ini (`[scummvm]` globals + target section) → args `--config <ini> --savepath <real> <target>` → verify real `scummvm.ini` unchanged, save dir not emptied → cleanup or keep the owned per-game file (it is the *persistent EmuWiz-owned layer* by current design; decide launch-only vs persistent, §16 item 7).

---

## 12. Implementation phases (derived from the audit)

| Phase | Deliverable | Depends | Exit criteria |
|---|---|---|---|
| 0 | Reconcile evidence: verify RetroArch exit-save leak empirically (one scripted run, key-probe before/after) | none | leak confirmed/denied; decides whether the existing projection needs a defect fix independent of cheats |
| 1 | Capability registry + routing unification (§8, §9), consistency tests, decide mGBA | none | tables T1–T5 derived; GUI projection test |
| 2 | Generic per-launch workspace on `resource_grants`: `CheatMaterial` role, generic materialiser (`GeneratedFile/Directory`, `SymlinkFile`, `DirectPath`), `0700/0600`, marker+lease, receipt, stale sweeper, env support in `PreparedProcessCommand` (if any adapter needs it) | 1 | RetroArch projection re-expressed on the generic layer with identical behaviour + tests |
| 3 | Verification harness: `LaunchStateExpectation`, fingerprints, `KeyProbe` parsers (INI/cfg/TOML/YAML subsets), report model, `Reconcile` restore | 2 | fixture-driven pass/fail matrix; no whole-directory hashing |
| 4 | RetroArch proof adapter: append cfg with `config_save_on_exit=false`, cheat db redirect, override-file blocker, real-emulator proof run | 2, 3 | persistence proof recorded → first `READY` |
| 5 | mGBA (SDL) then PCSX2 (`-gamecfg`), then MAME Lua, ScummVM seeding | 4 | per-adapter proof; Qt frontend blocked until proven |
| 6 | Stella reconcile, Dolphin overlay (only if a harness proves link fencing) | 3 | optional |
Each phase is independently mergeable and leaves launch behaviour unchanged until Phase 4 opts an adapter in.

## 13. Testing strategy
- **Unit (pure):** grant/expectation validation; capability→plan; selected-only rendering byte-exact; RetroArch append config includes required keys; blocker detection for override files (fixture directory); registry consistency tests (§9).
- **Filesystem fixtures (temp dirs):** materialise/cleanup incl. marker mismatch, symlink root, pre-existing destination, abnormal cleanup (`PartialDeletion`), sweeper with dead lease, `0700/0600` modes, concurrent same-save-dir lock.
- **Fake-emulator integration:** a tiny script standing in for the emulator that (a) writes a "config on exit" file, (b) mutates a save, (c) crashes — to test verification and cleanup on each exit path, without a real emulator (pattern already used by `launch/*_execution` tests).
- **Real-emulator proof harness (opt-in, not CI):** per adapter, run headless where possible, assert key probes/hashes before/after, record emulator version in `persistence_proof`. RetroArch's proof must cover: clean quit, `SIGTERM`, `SIGKILL`, with and without a per-game override file.
- **Golden diff test:** the current RetroArch projection tests must pass unchanged after re-basing on the generic layer.
- No GUI tests are needed for the workspace layer.

## 14. Relationship to the Cheat Core Batch (Batch 5 contract)
Batch 5 ("safe launch composition") should **consume**: (1) `CheatEmulatorCapability` (§8) as its only source of "can this be launch-scoped"; (2) the applicability result from Batch 4 (`cheat_applicability.rs`, `CheatSelectedGame`, verified-vs-candidate identity) as the guard; (3) conflict output from Batch 2 (`cheat_compatibility`) for selected-only composition; (4) provenance from Batch 3 to record *what* was composed in the launch receipt; (5) Batch 1's hardened `.cht` parser/renderer for RetroArch derivatives; (6) the per-launch workspace + verification contract (this document) for lifecycle; and (7) `patch_manager` *renderers* only, never `execute_shared_apply`.
**Design conflicts to watch (Batch 5 not reviewed; none can be confirmed):**
- If Batch 5 composes launch material by calling the persistent shared-transaction stack, it conflicts with §10 (journals/permanent files for ephemeral state).
- If Batch 5 uses `spawn_watched_process` unchanged and needs env redirection, it conflicts with P5.
- If Batch 5 adds another per-emulator match table, it reintroduces P8.
- If Batch 5 treats `Supported` from `cheat_apply_support` as "launch-scoped ready", it conflicts with §8 (that flag only means *persistent install*).
- The three unmerged batch branches change `patch_manager/mod.rs`; the registry module should be introduced as a **new file + one export line** to minimise conflicts.
Batch 6 (pack preview) should show, per selected cheat, the capability mode (persistent vs launch-scoped vs GUI-only) and the fences that would apply, from the same registry.

## 15. Answers to the requested audit points (summary)
- **`resource_grants` assessment:** correct vocabulary and validation for real saves + temp config + temp cheat material + temp cache without copying the profile; missing: verification/protected-file model, generic materialiser, process/exit ownership, stale sweeping, permissions, environment support, lifetime clarity.
- **RetroArch persistence:** STRONGLY_SUPPORTED (§3); mitigation STRONGLY_SUPPORTED, not proven; override-file precedence hazard STRONGLY_SUPPORTED.
- **Best proof adapter:** RetroArch, because the workspace, real save passthrough and command wiring already exist; it also has the largest reach. mGBA is the lowest-risk *second* proof (no fences needed) once its SDL/Qt question is settled.

## 16. Known unknowns
1. Runtime behaviour of every claim in §3 (nothing executed): whether `config_save_on_exit=false` in the append file suppresses both quit saves in the shipped RetroArch build; `SIGTERM` routing; remap/core-option/history writers.
2. RetroArch compat `getopt` permutation (non-glibc) for `--appendconfig` after positionals.
3. PCSX2 `[Folders]` config keys and the exact layer semantics of `-gamecfg` (source read only for path selection and enabled-list read).
4. Whether Xenia cvars are accepted as CLI flags in shipped builds; which file `--config`+`SaveConfig()` rewrites; Stella nvram location; Dolphin whether the command-line layer is excluded from `Config::Save`.
5. mGBA Qt frontend handling of `-c`.
6. MAME per-machine address-space data availability; Lua behaviour under `-video none`/headless.
7. Whether the ScummVM owned config is seeded with save keys (Gap G1) and whether the owned file should be launch-only or persistent.
8. Batches 5 and 6 contents.
9. Cross-launch locking policy (advisory lock design choice).

## 17. Files examined
`crates/archivefs-core/src/launch/{resource_grants,retroarch_resource_projection,retroarch_command,process_spawn,scummvm_command,dolphin_command,pcsx2_command,stella_command,mgba_command,mod}.rs`; `patch_manager/{cheat_route,cheat_loadability,shared_preview,shared_transaction,scummvm_trainer,mgba_cheats,mame_cheat,duckstation_cheat,whdload_trainer,melonds_cheat}.rs`; `crates/archivefs-gui/src/cheats_mods/{render,state}.rs`; `docs/research/{SAFE_LAUNCH_SANDBOX_V1,EMULATOR_FILE_ISOLATION_ARCHITECTURE_AUDIT,RETROARCH_CHEAT_AUTOLOAD_PATH,LAUNCH_OPTION_TRAINER_FAMILY_RESEARCH,CHEAT_EMULATOR_CAPABILITY_MATRIX}.md`; branch docs `docs/cheats/PROVENANCE_EVIDENCE.md`, `docs/CHEAT_APPLICABILITY_STATUS.md` (via `git show`). Upstream: RetroArch `retroarch.c`, `configuration.c`, `command.c`, `cheat_manager.c`, `config.def.h`; PCSX2 `QtHost.cpp`, `VMManager.cpp`, `Patch.cpp`; Xenia `patcher.cc`, `patch_db.cc`, `emulator.cc`, `xenia_main.cc`, `config.cc`; Stella `Settings.cxx`, `OSystem.cxx`, `src/cheat/CheatManager.cxx`; mGBA `commandline.c`, `core.c`; Dolphin `CommandLineParse.cpp`, `MainSettings.cpp`, `CommonPaths.h`; ScummVM `commandLine.cpp`.
