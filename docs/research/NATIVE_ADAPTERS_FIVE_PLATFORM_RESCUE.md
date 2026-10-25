# Native adapters: Atari800, b-em, Caprice32, NP2kai, Oricutron (current-main rescue)

Base: `bdeb86f1087ed4d2db49a232f40a38380130d91b`. Source material (read-only, never
modified): the uncommitted workspace `/home/davedap/emuwiz-082-batch3-launch`
(base `684916d2`, 202 commits behind main).

## Final classification

| Adapter | Classification | Why |
| --- | --- | --- |
| Atari800 | **VERIFIED LAUNCHABLE** | Atari800 5.0.0 run headless against the prepared command; containment observed with `strace`. |
| Caprice32 | **VERIFIED LAUNCHABLE** | Caprice32 4.6.0 (built from source, source read) run headless; every write site accounted for. |
| b-em | **PREVIEW / READINESS ONLY** | Not installed; upstream README and `main.c` disagree on flags; config and CMOS are saved on exit to an unproven path. |
| NP2kai | **PREVIEW / READINESS ONLY** | Not installed; config selection, positional routing and CMOS/NVRAM locations unproven; the audit says refuse. |
| Oricutron | **PREVIEW / READINESS ONLY** | Not installed; `oricutron.cfg` and ROM paths resolve beside the executable, so no env/cwd isolation can apply. |

Preview-only adapters validate and bind their inputs (executable policy, verified
evidence on the same bytes, main's structural parsers, a hash-bound scratch plan) and
expose `NativePreview`: readiness is always `Blocked`, `is_launchable()` is `false`, and
`unproven()` names exactly what stands in the way. They have no `prepare`, `spawn` or argv.

## Dirty source inventory

| Item | Disposition |
| --- | --- |
| `safe_launch_sandbox.rs`, `fs.rs`, tests | **Still needed, adapted.** Main had only the V1 design doc. Reviewed in full and kept (descriptor-relative workspace, ownership marker + lease, hash revalidation, bounded retention, spawn-intent recovery); its 28 tests pass unmodified on main. Adapted: `MediaRole::ConfigReferenced`, `MediaKind::Firmware`, source-path-in-config refusal. |
| `process_spawn.rs` isolated seam | **Kept, minimal.** Crate-private, Linux-only: per-child env, inherited workspace lease, start/exit callbacks. Applies cleanly; existing callers are byte-for-byte unaffected. |
| Per-adapter executable checks (5 copies, refused ANY symlink component) | **Replaced** by one policy in `native_support` (below). |
| Atari800 | **Ported**, simplified onto shared helpers. |
| Caprice32 | **Rewritten**: the seed syntax was wrong and containment unproven. |
| b-em, NP2kai, Oricutron prepare/spawn paths | **Not ported** (cannot be proven); validation kept as preview. |
| NP2kai weaker D88 validator | **Discarded** for main's `disk_format` D88 parser. |
| Oricutron length-only seed check, argv-attached ROMs | **Discarded.** |
| Everything else in the dirty tree (platform detection, Oric/Thomson media, Gamate, PDA, N-Gage, Virtual Boy, NGP, Pokémon Mini, Enterprise, Supervision profiles, `lib.rs`/`mod.rs` registrations, other launch edits) | **Unrelated / out of scope**, already on main in part. Not touched. |

## One shared scratch lifecycle (`safe_launch_sandbox`)

Owned private workspace (`media/ config/ data/ cache/ state/`), bounded physical copies
with source SHA-256 + identity revalidation before and after copying, ownership marker
and lease, lifetime tied to the watched child, deterministic cleanup (bounded retention
after a failed exit), HOME/XDG/TMPDIR isolation, and cleanup after spawn failure.
Also added: a member may be `ConfigReferenced`; `spawn` then verifies the validated
scratch config names `media/<scratch name>`, forbids passing it in argv too, and refuses
any config that contains an original path.

## Executable symlink policy (all five adapters)

Existing EmuWiz policy (`emulator_inventory`, `emulator_lifecycle`, every
`*_execution`): the LEAF must be a regular, non-symlink file with an execute bit. The
dirty adapters were stricter (any symlink component, which would refuse `/bin` on a
merged-usr system). The shared policy matches main: parents may be symlinks, the leaf
may not. The file is bound by identity AND SHA-256 (metadata alone missed a same-size
inode-reusing replacement within one timestamp tick, found in testing).

A PATH symlink (here `~/.local/bin/caprice32 -> ~/Applications/emulators/caprice32/cap32`)
is **not followed**. Discovery reports the link and, when the resolved target is itself an
eligible executable, that target, so a person can select the real file explicitly.
Trust was not widened to make Caprice32 pass.

## Input / state routing

| | Read-only inputs | Writable media | Config | NVRAM/CMOS | Save/state | Log/cache |
| --- | --- | --- | --- | --- | --- | --- |
| Atari800 | system ROMs (scratch copies), BASIC | scratch ATR/XFD (opened O_RDWR) | exact seed via `-config`, `-no-autosave-config` | none | not routed (disposable) | HOME/XDG/TMPDIR in workspace |
| Caprice32 | `<exe dir>/rom/*.rom` (hash-bound, read-only), resources | scratch DSK/CDT | exact INI seed via `--cfg_file` | none | snap/dsk/tape/cart/printer/screenshots -> `state/` (seed) | cwd = workspace (`./debug.txt`), TMPDIR in workspace |
| b-em / NP2kai / Oricutron | validated only | n/a | **unproven** | **unproven** | **unproven** | n/a |

## Evidence

### Atari800 5.0.0 (installed, real run, `strace`)
- `-help` run with the REAL HOME **created `~/.atari800.cfg`** (a probe side effect of
  this investigation; it did not exist before and was removed). Consequence: no probe
  of this emulator may run without an isolated HOME. `-v` does not write.
- Real default config keys confirm every key in the seed (`HD_READ_ONLY`,
  `ENABLE_H/P/R/SIO_PATCH`, `DISABLE_BASIC`); the first line is a comment/"created by".
- Headless (SDL dummy), prepared argv: the process stayed up until killed; it opened
  the explicit config (O_RDONLY), the ROM from scratch (O_RDONLY) and the scratch ATR
  (O_RDONLY then **O_RDWR**: the emulator writes disks, hence scratch copies); no other
  write-capable open, no mkdir/rename/unlink; HOME empty; original ATR byte-identical.

### Caprice32 4.6.0 (installed, built from source; source read)
- Config is INI (`[system] model=<0..3>`, `[file] snap_path=` ...). The prototype's
  dotted keys and string model were not valid.
- `--cfg_file` is searched first; an unreadable file silently falls back to
  `<exe dir>/cap32.cfg` (here the user's live config), `$XDG_CONFIG_HOME/cap32.cfg`,
  `~/.cap32.cfg`. The scratch config must therefore exist (checked at plan and spawn).
- Default writable locations are under the EXECUTABLE directory, not HOME/XDG, so
  isolation by environment alone does not contain them. The seed names `state/` for all.
  `saveConfiguration` is reachable only from the options GUI.
- Headless real run with the corrected seed: announced `Using configuration file:
  <workspace>/config/caprice32.cfg`, read `cpc6128.rom` from the install, read the
  scratch DSK, no write-capable open at all, nothing in HOME, install dir unchanged.

### b-em (not installed)
Upstream `src/main.c`: `-cfg`, `-disc`, `-disc1`, `-tape`, `-m<n>`, `-t<n>`,
positional `.uef/.csw/.snp`/disc; no `-c`, no `-u` (the BBC audit, from the README, lists
both). `main_close()` calls `config_save()` and `cmos_save()`.

### NP2kai (not installed)
Config dirs `~/.config/np2kai` (SDL2) and `~/.config/xnp2kai` (X11); no usable
documentation of a config argument, CMOS/NVRAM location or write protection.

### Oricutron (not installed)
Upstream `main.c`: options `-m/-d/-t`; `oricutron.cfg` opened via `add_fileprefix()` =
executable directory on Linux; ROMs only via config keys with the same prefix; disks
autosave to the image's own path. A scratch copy of the whole installation would be
required to isolate any of it.

## Tests performed
Shared sandbox 36, native support 11 (policy, hash binding, merged-/usr paths,
discovery, registry, preview-only guards), Atari800 32 (including a real-emulator
test), Caprice32 22 (including a real-emulator test), b-em 8, NP2kai 7, Oricutron 7:
123 new tests. Real-emulator tests skip when the emulator or a headless video driver
is unavailable.

## Final focused safety review (rebased onto `e128180e`)

The rebase was mechanical: all 18 files byte-identical to the reviewed tip, no overlap
with main's intervening changes. Changes made by the review:

- **`NativePreview::scratch_plan()` removed; `bound_sources()` added.** Handing a
  `SandboxPlan` to generic code would let it `prepare` and `spawn` a preview-only adapter
  with a hand-built argv. Only the hash-bound provenance is exposed now. A guard test scans
  b-em, NP2kai and Oricutron for any launch token, and fails (verified) if one is added;
  a second guard asserts planning, platform_map, readiness and integration do not name the
  native modules.
- **Sandbox attack tests** (after `prepare`, before `spawn`): same-size scratch edit,
  scratch config edit, scratch media replaced by a symlink to the source, source edited,
  source removed, ownership marker removed, marker rewritten for another transaction. All
  refuse before the child starts; owned workspaces are cleaned, and workspaces whose
  ownership cannot be proven are left in place. Look-alike, unmarked and symlinked
  directories in the root are never deleted by the startup sweep. The child holds the
  workspace lease while it runs, a startup sweep leaves it alone, cleanup happens once
  after exit, concurrent children clean only their own workspace, the parent environment
  is not mutated, and a hostile source name never reaches argv or a shell.
- **Atari800 / Caprice32:** a missing, unreadable, symlinked or edited scratch config
  refuses before spawn (both emulators fall back to a user/system config otherwise, so a
  planted live config is checked to be untouched); changed, removed or symlinked firmware
  refuses.
- **Merged-/usr paths:** `/bin -> usr/bin` style parents stay usable and hash-bound; a
  leaf symlink inside a symlinked parent is still refused; this host's `/bin/sh`
  (a symlink to dash) is refused as a leaf while `/bin/dash` is accepted.
- **Real-emulator evidence through the adapters' own prepared commands** (per-process
  `strace`): Atari800 opened the scratch config and ROM read-only and the scratch ATR
  read-write, and nothing else write-capable; Caprice32 opened the scratch config, its
  install ROM and the scratch DSK, all read-only, with no write-capable open or mutation.
  Both processes also connect to the user's D-Bus sockets (SDL); that is IPC, not file
  writes, and not something this sandbox isolates.
- A real-emulator assertion that counted entries in the live HOME was removed: it failed
  intermittently because other activity changes that directory. It now checks the specific
  files Caprice32 would create (`~/.cap32.cfg`, `~/.config/cap32.cfg`).
- **`launch::topology::...one_hundred_thousand_projections...`** (a 10 s wall-clock budget
  for 100k projections, 2.7 s alone) exceeds its budget on BOTH pristine main and the
  candidate under the same heavy load (`topology.rs` and `media_set` are unchanged); it is
  a pre-existing, load-sensitive test.

## Remaining gaps
- b-em, NP2kai, Oricutron need an installed binary and a scratch-installation (or
  proven state-redirect) design before any launch is offered.
- Atari800/Caprice32 firmware is hash-bound but unverified (no trust catalogue).
- No routing/GUI wiring: `platform_map` `standalone_adapters` feeds the planner and
  cheat routing, so changing it is not a tiny isolated edit. `NATIVE_ADAPTERS` is the
  read-only registry a later change can consume.
