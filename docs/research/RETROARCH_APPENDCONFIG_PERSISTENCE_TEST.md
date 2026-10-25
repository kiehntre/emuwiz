> **STATUS NOTE (recovered 2026-10-03; recovery baseline main `d112b7f3`).**
> Empirical research recovered verbatim from branch `research/retroarch-appendconfig-test` (`7cc31028`, test date 2026-09-29, written against main `4c980d18`).
> **Context of the observations:** RetroArch 1.22.2 (Git `69a4f0ea1e`) from Flatpak `org.libretro.RetroArch`, with the cores named in the body (`2048_libretro.so`, later Mesen), on this one host. Results are observations for that setup, not universal guarantees about `--appendconfig`, other RetroArch versions, other cores, overrides or menu saves.
> Main's `cheat_launch_plan.rs` cites this file as its `PersistenceProof` evidence for RetroArch. It is research evidence, not an adapter-ready declaration.

---

*Historical document follows unchanged.*

# RetroArch `--appendconfig` persistence test

Date: 2026-09-29. Test/research only; no production code changed.

## Safety conclusion after continuation

`config_save_on_exit=false` is required for the proposed append-config launch
policy, but **is not sufficient**. Supported core/game overrides can turn it
back on; explicit menu saves and auxiliary writes remain possible. A writable
real user config/profile must not be used for an EmuWiz cheat session. The
content-loaded experiments below demonstrated a disposable Flatpak profile
without exposing the real profile. This is research evidence, not an adapter
READY declaration.

**Correction to the earlier source-only assumption:** forced termination does
not prevent persistence. Phase 1 observed a base rewrite shortly after core
load, before the forced kill. Never use kill/crash semantics as isolation.

## Phase 1: original no-content results

| Test | Result |
|---|---|
| 1. Default (`config_save_on_exit = "true"`), graceful quit | **VERIFIED LEAK** |
| 2. Append file adds `config_save_on_exit = "false"`, graceful quit | **PREVENTED BY config_save_on_exit=false** (base byte-identical) |
| 3. Default, forced kill | **VERIFIED LEAK** (no graceful exit needed) |
| 4. Effective runtime save path | Follows the append file (confirmed for `savefile_directory`, `system_directory`) |

Append-config values ARE written back into the base `retroarch.cfg` by default.
`config_save_on_exit = "false"` in the append file suppressed all base-config
writes in the scenario tested.

Important nuance: the write is **not only on exit**. The logs show
`[Config] Saved config to "<base.cfg>"` shortly after the core loads (before any
quit), and in Test 3 the base was rewritten even though the process was killed.
Test 2 logged **no** `Saved config` line at all, so the option also suppressed
this early write.

## Environment

- RetroArch 1.22.2 (Git 69a4f0ea1e), Flatpak `org.libretro.RetroArch`
  (host wrapper `~/.local/bin/retroarch`). X11, `DISPLAY=:0`.
- Core: `2048_libretro.so` (copied from the distro package), launched with **no
  content**, so no ROMs or real saves are involved.
- Disposable tree: `/tmp/ra-append-test`. The sandbox was given access with a
  per-invocation `--filesystem=/tmp/ra-append-test` (no persistent override).
- `network_cmd_enable`/port 55391 was set in the base config, used to send
  `QUIT` (graceful) and `GET_CONFIG_PARAM` (runtime values).

## Files

Base config (`base.orig`, reset to these exact bytes before every test, hash
`3aa86726ab37669ffb56baecd0fbeecb`):

```
system_directory = "/tmp/ra-append-test/sys_base"
savefile_directory = "/tmp/ra-append-test/saves_base"
cheat_database_path = "/tmp/ra-append-test/cheats_base"
sentinel_key = "SENTINEL_BASE"
network_cmd_enable = "true"
network_cmd_port = "55391"
config_save_on_exit = "true"
video_fullscreen = "false"
video_window_save_positions = "false"
pause_nonactive = "false"
audio_driver = "null"
menu_show_load_content_animation = "false"
quit_press_twice = "false"
```

`append1.cfg` (Tests 1, 3):

```
system_directory = "/tmp/ra-append-test/sys"
savefile_directory = "/tmp/ra-append-test/saves"
cheat_database_path = "/tmp/ra-append-test/cheats"
```

`append2.cfg` (Test 2): the same three lines plus `config_save_on_exit = "false"`.

## Command line (all tests)

```
flatpak run --filesystem=/tmp/ra-append-test org.libretro.RetroArch --verbose \
  -L /tmp/ra-append-test/core.so --config /tmp/ra-append-test/base.cfg \
  --appendconfig <append file>
```

Log confirms: `Loading config: base.cfg` then `Appending config: appendN.cfg`.

## Evidence

| Test | Exit method | Base hash before | Base hash after | Outcome |
|---|---|---|---|---|
| 1 | `QUIT` over network cmd (graceful) | `3aa86726…` | `b1a546a5…` | changed |
| 2 | `QUIT` (graceful) | `3aa86726…` | `3aa86726…` | byte-identical |
| 3 | `flatpak kill` (SIGKILL) | `3aa86726…` | `b1a546a5…` | changed |

Test 1 and 3 produced the same file hash.

Changed original keys (Tests 1 and 3):

| Key | Before | After |
|---|---|---|
| `system_directory` | `…/sys_base` | `…/sys` (append value) |
| `savefile_directory` | `…/saves_base` | `…/saves` (append value) |
| `cheat_database_path` | `…/cheats_base` | `…/cheats` (append value) |
| `sentinel_key` and the other 9 original keys | unchanged | unchanged |

RetroArch also rewrote the file as a full dump: 13 keys became about 3,378
(every default setting). So a stale or minimal base config gets fully
populated as well as receiving the override values. A raw byte diff is not
useful; compare by key.

Test 2: no keys changed and no `Saved config` log line.

### Test 4: runtime effective values

`GET_CONFIG_PARAM` while running, in both Test 1 and Test 2:

- `savefile_directory` -> `/tmp/ra-append-test/saves`
- `system_directory` -> `/tmp/ra-append-test/sys`
- `cheat_database_path` -> `unsupported` (not exposed by this command, so not
  confirmed at runtime; its persistence to the base file is confirmed above)

The core wrote its SRAM to `/tmp/ra-append-test/saves/2048/2048.srm`, i.e. the
append path was really used, in both runs.

## Phase 1 interpretation (limited to those runs)

- Passing per-launch paths (or cheat settings) only through `--appendconfig`
  will silently rewrite the user's base `retroarch.cfg` unless the append file
  also sets `config_save_on_exit = "false"`.
- With that key set, the base file was untouched in this test, including the
  early save. This was tested with one core, no content, on 1.22.2 Flatpak.
  The continuation below covers content, overrides and explicit configuration
  saves; it disproves treating this option alone as a universal safeguard.
- A crash does not protect the base file when saving is enabled, because the
  write also happens shortly after startup.
- `config_save_on_exit = "false"` also stops the user's own in-session settings
  changes from being saved on exit for that launch. That is a behaviour
  trade-off for the design to accept explicitly.

## Phase 1 side effects outside the disposable tree (disclosed)

RetroArch's own Flatpak profile was touched by the runs, despite `--config`:

- `~/.var/app/org.libretro.RetroArch/config/retroarch/config/2048/2048.opt`
  (core options file, created by these runs): **deleted afterwards** together
  with its empty directory.
- `~/.var/app/org.libretro.RetroArch/config/retroarch/cores/core_info.cache`
  was refreshed (a regenerable cache). Left in place.
- The real `retroarch.cfg` files were not modified
  (`~/.var/app/org.libretro.RetroArch/config/retroarch/retroarch.cfg` and
  `~/.config/retroarch/retroarch.cfg`; hashes checked against the pre-test
  values).

These are historical Phase 1 effects. The continuation did not repeat them
in the real profile. Its tested isolation recipe and limits follow.

## Phase 2: content-loaded safety experiments

### Scope and reproducibility

Continued on `research/retroarch-appendconfig-test`, worktree
`/home/davedap/emuwiz-ra-appendconfig`, starting commit
`846e2ece5e26a2691439e2638c0b7bf91bebb7b7`. Production Rust, main, real
ROMs/saves and installed Flatpak overrides were not changed. No push.

The installed RetroArch remains 1.22.2, Git `69a4f0ea1e`; Flatpak is 1.14.6.
App deployment commit:
`9c51e2bcb6f7f29ecb327ee057b273c5b59efc22d35026e90aef601bc0052752`.
The copied distro core identifies itself through `retro_get_system_info` as
**Mesen 0.9.9**. The distro `fceumm_libretro.so` filename is a symlink to Mesen;
we used the actual Mesen identity for override paths. Copied core SHA-256:
`552f8ab6ac1fd08bd555f589eb999be73a469c79ccfa929f884adb2cf3366b43`.

Content was an entirely generated mapper-0 battery-backed iNES fixture, no
commercial content, copyrighted game assets or user collection access:
`/tmp/ra-append-test/safety/fixtures/SafetyFixture.nes`, 24,592 bytes, SHA-256
`097980cba31c6776591e5b0519f8d94656f9440d0c2a7947bcaf7e342620309a`.
To recreate its bytes:

```python
header = b'NES\x1a' + bytes([1, 1, 2, 0, 1]) + bytes(7)
prg = bytearray([0xEA]) * 16384
prg[:3] = bytes([0x4C, 0x00, 0x80])  # loop at $8000
prg[-6:] = bytes([0x00, 0x80]) * 3   # interrupt/reset vectors
fixture = header + prg + bytes(8192)
```

`GET_STATUS` confirmed `PLAYING nes,SafetyFixture,crc32=84c8dfa1` before each
case was recorded. A private Xvfb display (`:197`, manual probe `:198`),
software GL, null audio and copied core/info data kept runs disposable. The
initial X11 setup failure was retained as `setup-video-failure`, excluded from
results; the corrected launcher supplies the private display to both Flatpak
and its child. No EmuWiz GUI validation was performed. Only the requested
RetroArch explicit-save action used menu navigation.

Evidence remains under `/tmp/ra-append-test/safety` (temporary, not committed):
`run_cases.py`, `cheat_path_probe.py`, `protected_cheat_probe.py`,
`manual_probe.py`, and each named case's `command.json`, `run.log`,
`loaded.status`, `base.before.cfg`, `base.cfg`, `append.cfg`, `result.json` and
`profile/mountinfo.txt`. `real-profile-compare-*.json` records checks before and
after each group. Pinned upstream source copies are under `source/`.
The case builders refuse to overwrite existing case directories; choose fresh
names when rerunning. Do not rerun the historical Phase 1 broad-profile command.

The continuation's exact argument arrays are in `command.json`; their common
shape was:

```sh
# CASE/FIXTURE_ROOT/PROFILE represent fresh directories under the disposable tree.
# DISPLAY is also supplied to the host flatpak process for the private X11 socket.
DISPLAY=:197 flatpak run --sandbox --die-with-parent \
  --no-session-bus --no-documents-portal \
  --share=network --share=ipc --socket=x11 \
  --filesystem="$CASE" --filesystem="$FIXTURE_ROOT":ro \
  --env=DISPLAY=:197 --env=LIBGL_ALWAYS_SOFTWARE=1 \
  --env=XDG_CONFIG_HOME="$PROFILE/config" \
  --env=XDG_DATA_HOME="$PROFILE/data" \
  --env=XDG_CACHE_HOME="$PROFILE/cache" \
  --env=XDG_STATE_HOME="$PROFILE/state" \
  --command=sh org.libretro.RetroArch -c '<isolation guard>; exec /app/bin/retroarch "$@"' \
  research-guard --verbose -L "$PROFILE/config/retroarch/cores/mesen_libretro.so" \
  --config "$CASE/base.cfg" --appendconfig "$CASE/append.cfg" \
  "$FIXTURE_ROOT/SafetyFixture.nes"
```

Before executing RetroArch, the shell guard required the real Flatpak
`retroarch.cfg` to be absent and its real profile config directory to be
unwritable inside the sandbox, printed all four XDG values and saved mount
information. `--sandbox` dropped the installed app's broad `filesystems=host`
permission; only each case tree was granted writable and fixtures read-only.
Network sharing was solely for the research command interface. No persistent
`flatpak override`, host spawn or application-wide `flatpak kill` was used.
Only the disposable instance received `QUIT`; cleanup addressed its own
launcher and private Xvfb process.

### Content-loaded A/B results — runtime verified

Base settings included automatic config saving true. Append paths differed
from base paths for all four tested directory settings. B added
`config_save_on_exit = "false"`; A omitted that append key. Automatic states,
save sorting, shaders and file logging were disabled for these probes.

| Case | Base SHA-256 before / after (prefix) | Base at content load | Save/state output |
|---|---|---|---|
| A `case-A-default` | `2c155191a358` / `14cb8f65404a` | unchanged | append directories |
| B `case-B-protected` | `f73f4cb9244d` / `f73f4cb9244d` | unchanged | append directories |

All full hashes are in `result.json`. A rewrote the disposable base on graceful
quit, serializing append values. B stayed byte-identical through load, explicit
`SAVE_FILES`/`SAVE_STATE` and quit, with no config-save log. Neither content-loaded
case reproduced Phase 1's early write; that does not invalidate its earlier
runtime evidence or establish that other lifecycle paths cannot save early.

`GET_CONFIG_PARAM` confirmed append `savefile_directory`,
`savestate_directory` and `system_directory`. Explicit `SAVE_FILES` and
`SAVE_STATE`, followed by a wait and `QUIT`, produced
`append/saves/SafetyFixture.srm` (8,192 bytes) and
`append/states/SafetyFixture.state` (compressed, about 702–707 bytes across the
successful automatic-exit cases). Every successful A–F/H–K run exited zero.
The manual-action G run also exited zero; it is not a state-save probe.

Both A and B independently generated these private-profile files:

| File relative to `profile/config/retroarch` | Observed role / size |
|---|---|
| `config/Mesen/Mesen.opt` | core options, 799 bytes |
| `cores/core_info.cache` | core-info cache, 822 bytes |
| `playlists/builtin/content_history.lpl` | loaded content history |
| `playlists/builtin/content_{image,music,video}_history.lpl` | empty history files, 226 bytes each |
| `playlists/logs/Mesen/SafetyFixture.lrtl` | runtime history, 129 bytes |

Thus `config_save_on_exit=false` does not suppress these auxiliary writes.
Across **all continuation groups**, the real Flatpak base, native base and
real `core_info.cache` kept their before-run hashes **and mtimes**;
`config/2048/2048.opt` remained absent. These checks cover those named files;
the sandbox guard/mount evidence supplies the broader filesystem separation,
not a claim that a hash check traced every possible core write. The real ROM
and save directories were neither loaded nor inspected.

### Supported overrides and effective precedence — runtime verified

Used normal auto-loaded `.cfg` overrides, not an additional append argument:

```text
$XDG_CONFIG_HOME/retroarch/config/Mesen/Mesen.cfg          # core override
$XDG_CONFIG_HOME/retroarch/config/Mesen/SafetyFixture.cfg  # game override
```

Every override specified **all five** tested settings. Each layer had separate
save/state/cheat/system directories. RetroArch logged the supported override
paths as found. Override configurations were entirely synthetic and private.

| Case | Append / core / game save flag | Effective directory layer | Base identical after quit? |
|---|---|---|---|
| C `case-C-core` | false / true / absent | core | **no** |
| D `case-D-game` | false / absent / true | game | **no** |
| E `case-E-stacked` | false / false / true | game | **no** |
| F `case-F-disabled` | false / true / true, append `auto_overrides_enable=false` | append | **yes** |

Savefile/state/system values were queried live; actual SRAM and state outputs
confirmed the selected save/state layer. These establish that core overrides
beat append values and game overrides beat both append and core values.
F establishes a tested automatic-override control, not a universal prohibition
on later user actions or other configuration mechanisms.

The boolean and cheat-path getters return `unsupported`; that is not an
observed effective value. To close the cheat-path gap, H/I/J repeated C/D/E
with a one-entry **disabled** cheat marker in every candidate directory:

```ini
cheats = "1"
cheat0_desc = "Disabled <layer> marker"
cheat0_code = "AAAAAA"
cheat0_enable = "false"
```

H logged loading `core/cheats/Mesen/SafetyFixture.cht`; I and J logged loading
`game/cheats/Mesen/SafetyFixture.cht`. No cheat was enabled. Thus
`cheat_database_path` has the same observed layer precedence. H/I/J also
reproduced config rewrites and the save/state redirects.

**Launch-safety blocker:** a later supported game override can change false to
true and cause a base rewrite. The quit logs show overrides unloading before
`[Config] Saved config to ".../base.cfg"`. The resulting C/D/E/H/I/J config
dumps even contain append `config_save_on_exit=false` and append directories:
inspecting that final flag alone would misleadingly suggest safety. The
write happened nonetheless.

Additional runtime finding: H/I/J rewrote both the selected override-layer
cheat and the append-layer cheat during shutdown. K
(`case-K-protected-cheat`, no overrides, append save flag false) stayed
base-identical but rewrote its disabled append cheat from 99 to 600 bytes.
The log records game-specific cheat saves. **Original `.cht` sources must
therefore be kept out of RetroArch's writable cheat projection**; use a
generated derivative. This experiment itself only rewrote disposable markers.

### Explicit configuration save — runtime verified

G (`case-G-manual`) loaded content with append `config_save_on_exit=false`, no
overrides, and a separate pre-existing default-profile config sentinel. Its
private RGUI menu exposes distinct **Save Current Configuration** and
**Save Main Configuration** entries. Both were selected through the normal
menu in private Xvfb, with hashes recorded immediately after each action.

| Moment | `--config` base SHA-256 prefix | Default-profile config SHA-256 prefix |
|---|---|---|
| Before actions | `7357d7031ee4` | `b13bce6761fb` |
| After Save Current Configuration | `0b6bf9590007` | `b13bce6761fb` |
| After Save Main Configuration | `0b6bf9590007` | `3e0588f9e920` |

The log separately confirms saving `case-G-manual/base.cfg`, then looking for
and saving `profile/config/retroarch/retroarch.cfg`. Screenshots and
`after-save-current.json`, `after-save-main.json`, `manual-result.json` retain
evidence. Both dumps contain the false save flag. The real profile remained
inaccessible and the checked real files unchanged.

Therefore **automatic save disabled does not mean explicit save impossible**.
Save Current writes the selected disposable base in this tested no-override
case; Save Main writes the default profile config instead. Source says the
current-config action refuses while overrides are active; that variant was
not exercised manually. No further menu exploration is needed for the safety
conclusion.

## Source explanation, separate from runtime evidence

Sources below are pinned to the installed RetroArch Git identity
`69a4f0ea1e`; source reading explains observed behavior and is not additional
runtime coverage.

- `config_load_file` merges base, append, then override configurations.
  `config_load_override` builds core, content-directory, then game overrides
  and reloads path settings. Folder precedence is **source inference only**
  here; core/game precedence was tested above.
  [Configuration load and override source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/configuration.c#L3774).
- `main_exit` captures `config_save_on_exit` before unloading overrides, then
  uses that captured value to request the config save after unloading. This
  explains a write even though the restored settings and final dump say
  false. Separate lifecycle branches can also request saves before exit.
  [Exit source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/retroarch.c#L8578).
- Save Current calls `command_event_save_current_config`; Save Main first
  calls `open_default_config_file`, replacing the selected config path.
  `config_save_file` does not enforce `config_save_on_exit` for an explicit
  save. Current-config saving refuses while overrides remain active.
  [Menu event source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/retroarch.c#L4645),
  [save command source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/command.c#L2053),
  [default-file and serialization source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/configuration.c#L3530).
- Core options choose game/folder `.opt` when present, otherwise per-core
  `<config-directory>/<core>/<core>.opt` if `global_core_options=false`.
  `core_options_path` controls the global fallback/seed, **not every per-core
  or game option output**. Options are saved separately from the base config.
  [Core-options source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/runloop.c#L1071).
- The application config directory uses `rgui_config_directory`, falling back
  to the selected config's directory only when that setting is empty. Unix
  frontend defaults populate many resource directories from
  `$XDG_CONFIG_HOME/retroarch`, or `$HOME/.config/retroarch` as fallback.
  This explains Phase 1's `2048.opt` in the real profile despite `--config`.
  [Special-directory source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/file_path_special.c#L196),
  [Unix defaults](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/frontend/drivers/platform_unix.c#L1786).
- Core-info cache is `core_info.cache` in the info directory selected by
  `libretro_info_path`; `core_info_cache_enable` governs its use/refresh.
  It is not controlled by `config_save_on_exit`. The Unix default can put
  info/cache alongside cores, as both phases observed.
  [Core-info cache source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/core_info.c#L1998).
- Game-specific cheat load/save paths join `cheat_database_path`, core name
  and content cheat filename. The save function serializes that cheat file.
  K provides the runtime proof that disabling automatic base-config saving
  does not protect a loaded cheat source from rewriting.
  [Cheat path/save source](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/cheat_manager.c#L695).

## Writable resource classification for a future adapter

This is a proposed control policy, not a claim every category wrote in these
experiments. Only the files listed above have continuation runtime evidence.
Settings names are taken from the pinned
[configuration registry](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/configuration.c#L1641)
and [reference config](https://github.com/libretro/RetroArch/blob/69a4f0ea1e/retroarch.cfg).
Existing read-only assets need not all be copied or isolated as writable data.

| Resource | Controls / relevant behavior | Classification and control |
|---|---|---|
| Base/default config, override configs | `--config`, XDG profile, `rgui_config_directory`, `auto_overrides_enable`; explicit saves remain possible | **must isolate** writable configs; merge only approved read-only settings into generated files; control core/folder/game and subsequent reload layers |
| Core options | `core_options_path`, `global_core_options`, `game_specific_options`, config directory; `.opt` auto writes observed | **must isolate** writable option outputs; approved existing options may seed private copies |
| Cheat files | `cheat_database_path`; loaded `.cht` rewrite observed with false save flag | **must isolate** generated derivative; original source read-only/outside writable tree |
| Input remaps/controller profiles | `input_remapping_directory`, `auto_remaps_enable`, `remap_save_on_exit`, `joypad_autoconfig_dir`; remaps and manual controller-profile saves are separate mechanisms | **must isolate** writable remaps; approved user mappings may seed copies; not runtime exercised |
| SRAM, memory cards and selected states | `savefile_directory`, `savestate_directory`, sorting and auto-state settings; selected paths verified | **should point to real user state** only through explicitly selected narrow save/state grants or a defined copy/commit policy; never discard existing saves merely to isolate configs |
| System/BIOS | `system_directory`; `LIBRETRO_SYSTEM_DIRECTORY` can override the setting in source | **should point to real user state** through approved read-only BIOS/system projection, not the whole warehouse; core-specific writable system requirements remain **unknown** until proven |
| Core-info/general caches | `libretro_info_path`, `core_info_cache_enable`, `cache_directory` | **regenerable cache**; private/session or dedicated adapter-owned cache, not incidental writes into user's profile; read-only core info can be projected separately |
| Playlists/favorites/history | `playlist_directory`, `history_list_enable`, `content_history_directory`, individual content/image/music/video history paths | **must isolate** temporary-path history, or suppress it with verified controls; deliberate canonical history updates **may persist normally** under explicit product policy |
| Runtime history | `runtime_log_directory`, `content_runtime_log`, `content_runtime_log_aggregate`; `.lrtl` observed despite `log_to_file=false` | **must isolate** session-path runtime logs or disable separately; deliberate permanent runtime accounting **may persist normally** |
| Diagnostic logs | `log_dir`, `log_to_file` | **may persist normally** in an approved output location; otherwise private/disabled; file-log writes not exercised |
| Screenshots/recordings | `screenshot_directory`, `recording_output_directory`, `recording_config_directory`; state thumbnails are separate | **may persist normally** to selected user output; default-profile destinations must still be controlled; not runtime exercised |
| Shader presets | `video_shader_dir`, config directory, automatic core/game preset and manual-save behavior | **must isolate** writable presets; approved shader resources can remain read-only; not runtime exercised |
| Thumbnails | `thumbnails_directory` and thumbnail download settings | **regenerable cache**; existing assets may remain read-only, deliberate cache updates **may persist normally**; downloads not exercised |
| Content databases/assets/core downloads | `content_database_path`, `assets_directory`, `core_assets_directory`, `libretro_directory`; updater actions differ from read-only lookup | Read-only installed assets can remain shared; updater outputs **must isolate** or remain unavailable during a session; no download work performed |
| Replays, achievements screenshots, core-specific VFS/direct filesystem writes | Separate features and core implementation paths, potentially outside frontend save settings | **unknown** until enabled-feature/core-specific audit; default to OS confinement and narrow approved writable grants |

Do not equate the selected base config with a profile root. Environment values
(such as `LIBRETRO_SYSTEM_DIRECTORY`), copied configs with absolute paths,
symlinks and later reloads can escape a mere path-normalization plan. A future
adapter must prove its resolved paths and filesystem grants together.

## Flatpak profile selection and per-launch isolation

A normal Flatpak invocation supplies app-specific XDG directories under
`~/.var/app/org.libretro.RetroArch`: `config`, `data`, `cache` and `.local/state`.
RetroArch's Linux frontend primarily derives defaults from the config root;
its config/profile therefore normally resides in `config/retroarch`.
Host-side XDG assignments alone are not reliable because Flatpak supplies its
own defaults. [Flatpak XDG conventions](https://docs.flatpak.org/en/latest/conventions.html#xdg-base-directories).

Per-invocation `--env=XDG_CONFIG_HOME=...` (plus data/cache/state) selected the
private profile successfully here. `--sandbox` starts without the installed
app's ordinary permissions, allowing only the additional narrow grants.
This combination hid the real profile even though `HOME` still named the host
user: that name did not expose the host home tree. Probe the actual mount view,
not just the environment string.
[Flatpak run options](https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-run).

| Mechanism | Research conclusion |
|---|---|
| RetroArch `--config` / `--appendconfig` | Selects/merges settings; **does not isolate all profile writes** |
| Explicit RetroArch directory settings | Supported controls for specific resources; useful, but defaults, overrides and separate paths must all be accounted for |
| Flatpak `run --env=...` XDG roots | **runtime verified** profile relocation with this installation; pair with private directories and confinement |
| Flatpak `run --sandbox` plus narrow `--filesystem` grants | **runtime verified** real profile absent/unwritable in the continuation; do not re-grant `host`, `home` or the real app profile |
| `--nofilesystem=host` alone / environment-only relocation | **not proven sufficient**; dropping a broad grant must not be assumed to hide Flatpak's own persistent app data |
| Persistent `flatpak override` / changing actual user's profile | Neither required nor used; unsuitable as per-launch isolation |
| Native RetroArch environment relocation | Source supports XDG config-root selection; **not runtime tested** here and lacks the tested Flatpak filesystem boundary |

The tested command is a research recipe, not a portable adapter implementation.
It used copied core/info resources and a private X11 server; real audio,
controllers, GPU access and normal desktop integration will need individually
justified grants. Restoring normal desktop/portal permissions must not restore
broad filesystem or profile access. No claim that every installation, core or
Flatpak version behaves identically; unsupported isolation must fail closed.

## Universal launch-safety answers

| Question | Answer |
|---|---|
| A. Is `config_save_on_exit=false` necessary? | **Yes for the proposed safety policy:** defaults demonstrably serialize session values. Require false as defense in depth, although OS isolation/disposable configs provide the actual write boundary. |
| B. Is it sufficient? | **No.** Later overrides, explicit saves, core options, cheat files, caches and histories defeat that assumption. |
| C. Can later overrides defeat it? | **Yes, runtime verified** for core and game overrides, including game true after core false. This is a launch-safety blocker unless later layers are prevented or constrained and filesystem writes confined. |
| D. Can `--config` alone isolate RetroArch? | **No.** Phase 1 wrote auxiliary files in the real profile; Phase 2 demonstrated independent default-profile and auxiliary write paths. |
| E. Which additional writable resources require control? | Default/profile configs and overrides, core/game options, cheat derivatives, remaps, histories/runtime logs, caches, enabled preset/media outputs and core-specific writes; approved save/state persistence needs its own narrow grant. |
| F. Is using the real base config ever acceptable for an EmuWiz cheat launch? | **Do not select a writable real base as the launch config.** Read approved values from it, then use a generated/disposable copy. Merely making that one file read-only still leaves independent real-profile writes unless separately confined. |
| G. Minimum proof for adapter READY? | See the concrete gate below. This research alone does not mark an adapter READY. |

Minimum READY evidence must identify the actual binary/package, core, content
and feature shape, then demonstrate:

1. A disposable selected config **and default profile**, with the real profile
   inaccessible to writes; read-only input projections and narrow selected
   save/state grants are verified inside the actual launch environment.
2. Effective safety values after core/content load, including hostile synthetic
   core/folder/game overrides and any supported later reload layers. Disable
   unapproved overrides or build an allowlisted projection; do not trust a
   final serialized false flag as proof of no earlier write.
3. No unintended persistence at load, content changes/unload, graceful quit,
   forced termination or explicit Save Current/Main/New actions. Account for
   writes that happen before termination; prove destinations, not exit code.
4. Separate control/evidence for core options, cheat-source preservation,
   remaps, core-info/cache, histories/runtime logs and enabled output features,
   plus selected-core system/VFS requirements. Untested core paths remain
   unsupported/unknown rather than silently promoted.
5. Actual SRAM/state persistence reaches the intended selected user state,
   approved settings/input mappings remain usable, and symlinks, inherited
   path overrides, package permissions and portal grants cannot restore broad
   writable user access. Revalidate relevant assumptions on package/core changes.

Intentionally deferred: production adapter implementation; native/other package
runtime validation; folder override runtime validation; manually saving while
an override is active and Save New/As actions; exhaustive core-specific
filesystem tracing and all optional feature write paths. The blocker and confinement requirements are
resolved sufficiently for design review, not universal compatibility proof.

Validation for this documentation continuation: `git diff --check`. No Cargo
builds, Rust tests, workspace suites, release builds or EmuWiz GUI tests.
Only these research documents are committed; disposable evidence stays in `/tmp`.
