# openMSX Standalone Adapter Audit

Research / design only. No production Rust, tests, or Cargo runs were touched
while producing this document. Authoritative HEAD inspected:
`52819b02855c57f122063e13e4fc88cc33ea9f52`
(`feat(launch): add Snes9x SNES adapter`) on
`feature/archivefs-unified-platform`.

## Executive summary

openMSX is a mature, still-maintained MSX/MSX2/MSX2+/turboR emulator with a
stable, well-documented command line. A bare media filename is auto-inserted
as the correct media type by registered extension, and it can boot with **no
user setup**: on a fresh install it runs the bundled, freely-licensed
**C-BIOS** machine `C-BIOS_MSX2+`.

The blocking fact for a v1 adapter is machine scope, not the CLI:

- openMSX is genuinely **multi-machine**. Without `-machine` it uses the
  `default_machine` *setting* (initially `C-BIOS_MSX2+`), which EmuWiz cannot
  read and therefore cannot review.
- **C-BIOS runs cartridge ROMs only.** Upstream: *"It does not include support
  for MSX-BASIC or disk drives yet, so software that comes on tape, disk or
  any other media than ROM cartridges will not run."* Disk (`.dsk`) and
  cassette (`.cas`) launches require a real MSX machine definition **plus that
  machine's proprietary system ROMs**, which EmuWiz can neither supply nor
  pick a specific one of.
- EmuWiz's canonical MSX evidence is folder-alias based; its *strong*
  extensions are `.mx1` (MSX) and `.mx2` (MSX2) only, which openMSX treats as
  plain cartridge ROMs.

Recommendation: **DEFER_V1** for a full standalone adapter until a shared
machine-profile seam exists (the same seam the in-flight VICE C64 lane needs).
A deliberately narrow **cartridge-ROM-only, default-C-BIOS-machine** adapter is
technically feasible today and is classified **READY_WITH_MACHINE_PROFILE_SEAM**;
choose it only if the project wants openMSX before the VICE seam lands.

## Current EmuWiz MSX identities

From `crates/archivefs-core/src/platform/mod.rs` (unchanged, do not modify):

| field | `MSX` | `MSX2` |
| --- | --- | --- |
| `id` | `"MSX"` | `"MSX2"` |
| `display_name` | `"MSX"` | `"MSX2"` |
| `folder_aliases` | `msx`, `msx1`, `microsoftmsx`, `msxone` | `msx2`, `microsoftmsx2`, `msx2plus`, `msxtwo` |
| `filename_aliases` | `[]` | `[]` |
| `strong_extensions` | `["mx1"]` | `["mx2"]` |
| `weak_extensions` | `["rom", "dsk", "cas", "zip"]` | `["rom", "dsk", "cas", "zip"]` |
| `magic` | `[]` | `[]` |
| `layout` | `[]` | `[]` |
| `conflicts_with` | `["MSX2"]` | `["MSX"]` |
| `preferred_emulator` | `None` | `None` |

- Identity is essentially **folder-alias only**. No magic bytes, no layout
  evidence, no header/serial identity, no BIOS/firmware identity.
- Registry explanation (MSX): *"`.mx1` is MSX1 specific. `.rom`/`.dsk`/`.cas`
  are shared across the whole MSX range and with other systems."* (MSX2):
  *"`.mx2` is MSX2 specific; every other MSX format is shared with MSX1."*
- `crate::platform::platform_for_alias` resolves `msx`/`microsoftmsx` →
  `MSX` and `msx2`/`microsoftmsx2` → `MSX2` (normalised, exact match). A
  RetroArch core whose `.info` `systemname`/`database` names
  `MSX` / `Microsoft - MSX` / `Microsoft - MSX2` therefore already produces a
  RetroArch launch *candidate* via
  `crate::launch::platform_map::retroarch_platform_candidate`, independent of
  any `LAUNCH_COMPATIBILITY` row.

### Current launch-compatibility rows

`crate::launch::platform_map::LAUNCH_COMPATIBILITY` has **no `MSX` row and no
`MSX2` row**. Consequences today:

- No reviewed standalone adapter, no reviewed RetroArch core hint for either
  platform.
- Emulator Setup (`crates/archivefs-gui/src/emulator_setup_page.rs`) iterates
  `LAUNCH_COMPATIBILITY`, so **MSX/MSX2 do not appear in Emulator Setup at
  all**. The only non-row candidates the GUI synthesises are the hardcoded
  SameBoy and DOSBox "unintegrated" rows.
- The shared planner (`crate::launch::planning::build_launch_plan`) still
  emits a RetroArch-core candidate for a resolved `MSX`/`MSX2` identity when
  an installed `bluemsx`/`fmsx` core's own `.info` resolves to it.

### `VerifiedIdentityFact`

`crate::launch::input_projection::VerifiedIdentityFact` has **no MSX
variant**. This is fine: the mGBA / RMG / Snes9x / Mesen adapters do not use a
`VerifiedIdentityFact` either — they gate purely on
`CanonicalIdentityStatus::Resolved(identity)` with
`identity.platform_id == "<id>"`, carrying the opaque `game_key` through
unchanged. openMSX would follow the same pattern; **no new
`VerifiedIdentityFact` variant is required.**

## Upstream openMSX CLI evidence

Sources (retrieved 2026-09-06):

- openMSX source `master` (≈ release 21.0):
  - `src/CommandLineParser.cc` — option registration and bare-filename
    (file-type) dispatch.
  - `src/memory/MSXRomCLI.cc` — cartridge options + auto extensions.
  - `src/fdc/DiskImageCLI.cc` — disk options + auto extensions.
  - `src/cassette/CassettePlayerCLI.cc` — cassette option + auto extensions.
  - `src/config/SettingsConfig.cc` / `.hh` — settings auto-save on exit.
- openMSX manual: <https://openmsx.org/manual/user.html>,
  <https://openmsx.org/manual/setup.html>,
  <https://openmsx.org/manual/commands.html>.
- openMSX GitHub release `RELEASE_21_0`
  (`openmsx-21.0-linux-x86_64-bin.zip`; no AppImage asset).
- Debian/Ubuntu package metadata: `openmsx 19.1+dfsg-1ubuntu3`,
  `Provides: msx-emulator`, `Depends: openmsx-data (= …), cbios, …`.

### Executable

- Linux executable name: **`openmsx`** (distro package, Flatpak wrapper
  target, and the upstream Linux `.zip` bundle all name it `openmsx`).
- Version reference: Ubuntu 24.04 ships `19.1`; current upstream is `21.0`.
  The command-line surface below is stable across both.

### Options (`src/CommandLineParser.cc`)

Registered top-level options: `-h` / `--help`, `-v` / `--version`, `-bash`,
`-setting <file>`, `-control <type>`, `-script <tcl>`, `-command <tcl>`,
`-testconfig`, `-machine <name>`, `-setup <name>`.

- `--` is **not** a separator/end-of-options marker. The only `--` forms are
  `--help` and `--version`.
- Media (`-cart*`, `-ext*`, `-disk*`, `-hd*`, `-cassetteplayer`, `-cd*`) are
  registered by dedicated CLI helper classes, not in the list above.

### Bare-filename dispatch (auto media type)

`CommandLineParser::getFileTypeHandlerForFileName` looks up the argument's
extension in a registered file-type table and routes it to the matching media
handler. Manual: *"If you run openMSX from the command line, adding a file
name (with path if necessary) as a command-line option, openMSX will insert
the file as the proper type of media."*

| Media handler | Options | Auto-detected extensions |
| --- | --- | --- |
| Cartridge (`src/memory/MSXRomCLI.cc`) | `-cart`, `-carta`, `-cartb`, `-cartc`, `-cartd` (`-cart` == `-carta` == slot A); also `-ips <patch>`, `-romtype <type>` | `ri`, `rom`, `mx1`, `mx2`, `sg`, `col` |
| Disk (`src/fdc/DiskImageCLI.cc`) | `-diska`, `-diskb` | `di1`, `di2`, `dmk`, `dsk`, `xsa`, `fd1`, `fd2` |
| Cassette (`src/cassette/CassettePlayerCLI.cc`) | `-cassetteplayer` | `cas`, `wav`, `tsx` |

Note: `.mx1` and `.mx2` are in the **cartridge** extension set — openMSX loads
them into a cartridge slot exactly like `.rom`; the extension does **not**
select an MSX1 vs MSX2 machine.

### Machine / extension selection

- `-machine <name>` — e.g. `openmsx -machine Panasonic_FS-A1GT`.
- `-ext <name>` / `-exta` / `-extb` — plug an extension (e.g. `-ext fmpac`).
- `-setup <name>` — a saved multi-media setup; overrides `-machine`.
- Without `-machine`/`-setup`: openMSX uses the `default_machine` setting.
  Manual: *"openMSX uses this machine when it is started without the
  `-machine` option and without the `-setup` option and the `default_setup`
  setting is empty …"* Out of the box `default_machine` is `C-BIOS_MSX2+`.

### First-run / config behaviour

- User data root: `~/.openMSX/` (`OPENMSX_HOME`). Settings file:
  `~/.openMSX/share/settings.xml`. System-ROM file pools:
  `~/.openMSX/share/systemroms` (+ system data dir, `OPENMSX_*` env).
- `SettingsConfig::~SettingsConfig` auto-writes `settings.xml` on exit when
  `save_settings_on_exit` is enabled; the manual treats save-on-exit as the
  normal/enabled case (*"If you disabled `save_settings_on_exit`, you can use
  [`save_settings`] …"*). There is **no single command-line flag** to disable
  it (only `set save_settings_on_exit off` via `-command`, which would be
  EmuWiz reconfiguring the emulator — out of scope). openMSX also creates
  `~/.openMSX/` and its `share/` tree on first run.
- Missing system ROM is deterministic and testable:
  `Fatal error: Error in "<machine>" machine: Couldn't find ROM file for
  "<rom name>" (sha1: …)` on stderr, non-zero exit. `C-BIOS` machines never
  hit this — *"The C-BIOS machines come with ROMs installed."*

## Machine selection semantics

This is the decisive question. openMSX emulates dozens of distinct machines.

1. **`openmsx <cartridge>` with no `-machine`** boots the `default_machine`
   setting. On an untouched install that is `C-BIOS_MSX2+`, which upstream
   says runs *"most MSX1, MSX2 and MSX2+ cartridge-based games"*. So a
   cartridge launch is deterministic **given openMSX installed**, but:
   - EmuWiz cannot read `default_machine`, so it cannot state which machine a
     candidate will run on.
   - If the user configured a real machine that needs proprietary ROMs and
     those ROMs are absent, `openmsx <cartridge>` fails with the fatal
     missing-ROM error above — a state EmuWiz cannot detect read-only without
     knowing the machine.
2. **`openmsx -machine C-BIOS_MSX1 <cartridge>`** (or `C-BIOS_MSX2` /
   `C-BIOS_MSX2+`) is fully deterministic and needs no proprietary ROMs.
   `C-BIOS_MSX1` (MSX1, 64 kB), `C-BIOS_MSX2` (MSX2, 512 kB RAM / 128 kB
   VRAM), `C-BIOS_MSX2+` (MSX2+, + MSX-MUSIC) are openMSX's own bundled,
   freely-licensed machine definitions — **not** an invented generic machine.
   The cost: pinning `-machine` overrides a user's `default_machine`
   preference.
3. **Disk / cassette on any C-BIOS machine does not work** (no disk ROM, no
   MSX-BASIC). A working disk/cassette launch needs a real machine
   (`Philips_NMS_8250`, `Panasonic_FS-A1WX`, …) whose proprietary system ROMs
   the user has installed into `systemroms/`. EmuWiz has no verified evidence
   to choose one of those machines and cannot legally supply the ROMs.

**Conclusion:** a *reviewed* openMSX launch is safe **only** for cartridge
content, and even then only if EmuWiz either (a) accepts "the user's default
machine" as the reviewed contract, or (b) pins `-machine C-BIOS_MSX1` /
`-machine C-BIOS_MSX2`. Option (b) is the honest reviewed choice for a v1 that
must not depend on unseen user config, but it needs a place to record "this
platform maps to this C-BIOS machine" and a way for the user to override it —
i.e. a machine-profile seam.

## MSX decision

- **Standalone adapter, v1: `DEFER_V1`.** openMSX can launch MSX cartridge
  ROMs today, but a reviewed adapter needs the machine-profile seam to (a)
  pin/record `C-BIOS_MSX1` deterministically and (b) let a user point at a
  real machine + supplied ROMs. Disk and cassette — the bulk of real MSX1
  software — are out for v1 regardless.
- **If the project wants openMSX before the seam:
  `READY_WITH_MACHINE_PROFILE_SEAM`** for a cartridge-ROM-only adapter that
  pins `-machine C-BIOS_MSX1`, accepting `.mx1` strong content only.
- **`READY_FOR_ADAPTER` is not appropriate**: unlike single-machine mGBA /
  Snes9x, openMSX's machine is a user setting EmuWiz cannot review.

## MSX2 decision

- **Standalone adapter, v1: `DEFER_V1`.** Same reasoning. The natural pin is
  `-machine C-BIOS_MSX2` (`C-BIOS_MSX2+` is the upstream default and also
  acceptable). Disk/cassette out for v1.
- **If the project wants openMSX before the seam:
  `READY_WITH_MACHINE_PROFILE_SEAM`** for a cartridge-ROM-only adapter that
  pins `-machine C-BIOS_MSX2`, accepting `.mx2` strong content only.
- MSX2 has a mild edge over MSX1 here: `C-BIOS_MSX2+` (the default) natively
  covers MSX2 software, so "user default machine" is more often correct for
  MSX2 than for MSX1.

Both platforms would need machine choice to become part of Emulator Setup if a
"pin C-BIOS by default, allow real-machine override" model is adopted.

## Content-format matrix

EmuWiz strong extensions: `MSX` → `.mx1`; `MSX2` → `.mx2`. Weak/ambiguous
(shared): `.rom`, `.dsk`, `.cas`, `.zip`. Following the Snes9x/Mesen
precedent (accept strong extensions only; refuse weak/ambiguous), a v1
openMSX adapter accepts `.mx1`/`.mx2` only.

| Form | openMSX handling | v1 class | Why |
| --- | --- | --- | --- |
| `.mx1` (MSX strong) | auto cartridge slot A | **SAFE_DIRECT** (with default/C-BIOS machine) | unambiguous MSX1 cartridge ROM; C-BIOS_MSX1 runs it, no firmware |
| `.mx2` (MSX2 strong) | auto cartridge slot A | **SAFE_DIRECT** (with default/C-BIOS machine) | unambiguous MSX2 cartridge ROM; C-BIOS_MSX2(+) runs it, no firmware |
| `.rom` (weak) | auto cartridge slot A | **AMBIGUOUS → UNSUPPORTED_FOR_V1** | shared with MAME/SG-1000/ColecoVision/etc.; mapper type also unverified. Same call as Snes9x refusing `.bin` |
| `.ri`, `.sg`, `.col` (openMSX cartridge exts, not EmuWiz MSX exts) | auto cartridge | **UNSUPPORTED_FOR_V1** | not EmuWiz MSX identity forms |
| `.dsk` / `.di1` / `.di2` / `.dmk` / `.xsa` / `.fd1` / `.fd2` | auto disk drive A (`-diska`) | **UNSUPPORTED_FOR_V1** | C-BIOS has no disk ROM; a real machine needs proprietary ROMs EmuWiz cannot supply/verify. Also `.dsk` weak/ambiguous in EmuWiz |
| `.cas` / `.wav` / `.tsx` | auto cassette player (`-cassetteplayer`) | **UNSUPPORTED_FOR_V1** | C-BIOS has no MSX-BASIC; tape autoload needs `RUN"CAS:"` from BASIC. Corroborated by `docs/research/TAPE_FORMAT_SUPPORT_AUDIT.md` ("Launch today: none for any tape format") |
| `.zip` (weak) | not a media type; openMSX can read some archives internally but not as a bare CLI arg reliably | **UNSUPPORTED_FOR_V1** | never a canonical launch form in EmuWiz |
| hard disk (`-hda` + `-ext ide`), laserdisc, `-setup` | n/a | **UNSUPPORTED_FOR_V1** | out of MSX/MSX2 cartridge scope |

## Firmware / system-ROM requirements

- openMSX ships **machine definitions** (`share/machines/*.xml`) but bundles
  **only** the freely-licensed **C-BIOS** ROMs. Every non-C-BIOS machine
  requires the user to place that machine's proprietary system ROMs into a
  `systemroms/` file pool.
- Debian/Ubuntu: `openmsx` `Depends: openmsx-data` (machine/extension defs +
  free software) **and** `cbios` (the C-BIOS ROM package). `+dfsg` in the
  version string = repackaged with non-free content removed. So the packaged
  install boots `C-BIOS_MSX2+` immediately with zero user action.
- Missing-ROM reporting is deterministic and read-only-testable:
  `Fatal error: … Couldn't find ROM file for "<name>" (sha1: <hash>)`,
  non-zero exit. Search order: `~/.openMSX/share/systemroms`, system data
  `share/systemroms`, `OPENMSX_*` env pools.
- **EmuWiz firmware position for a v1 cartridge adapter: `NOT_REQUIRED`** when
  the launch pins a C-BIOS machine (or the user's default is C-BIOS).
  `FirmwareReadiness::Unknown` when the launch relies on the unseen
  `default_machine` setting. A real-machine profile (future) would carry a
  per-machine `systemroms` readiness probe, projected to
  `PresentUnverified` / `Missing` / `Unknown` — never verified, never
  downloaded, never copied.
- Do **not**: download or copy system ROMs, name specific proprietary ROMs,
  or propose redistribution.

## Configuration behaviour

- openMSX creates `~/.openMSX/` and writes `~/.openMSX/share/settings.xml` on
  exit (save-on-exit is the normal enabled state). This is **openMSX-owned
  config in the user's own openMSX area** — harmless, expected, and exactly
  analogous to SameBoy/mGBA/Mesen writing their own prefs. It is **not**
  EmuWiz mutating configuration.
- The v1 adapter therefore launches **bare** (no `-setting`, no `-command`,
  no config file authored by EmuWiz). It must not attempt to suppress
  openMSX's own save-on-exit — that would be "automatic emulator
  configuration", which the shared launch contract forbids.
- Distinguish clearly in the future adapter doc/comments: *openMSX writing
  `settings.xml` = allowed and out of adapter scope; EmuWiz writing any
  openMSX config = forbidden.*
- There is no `--doNotSaveSettings`-style flag (that is Mesen). Do not invent
  one.

## Discovery / install forms

| Form | Recommendation |
| --- | --- |
| PATH `openmsx` (distro package, source build, Flatpak-exported binary) | **SAFE** — primary discovery, mirrors Snes9x/Mesen |
| Explicit user-supplied executable path | **SAFE** — secondary, mirrors every recent adapter |
| Upstream Linux release | a plain `openmsx-<ver>-linux-x86_64-bin.zip` build tree, **not** an AppImage — treat an extracted `openmsx` binary as an explicit path |
| AppImage | **openMSX upstream does not distribute an AppImage** (no `.AppImage` release asset). Even if a third party packaged one, classify it under existing EmuWiz managed-AppImage policy only — **do not** add generic AppImage execution for openMSX. Same stance as Snes9x/Mesen |
| Flatpak (`org.openmsx.openMSX`) | out of scope for v1 discovery (no adapter currently does Flatpak-scoped discovery for a standalone emulator); note only |

## Exact proposed argv

All forms are `[executable, args…]` with **no shell**, **no string
concatenation**, **inherited environment**, and a final pre-spawn
executable + content re-validation. The verified selected ROM path is passed
verbatim; the path is never rewritten.

### Preferred v1 (cartridge-only, pinned C-BIOS machine)

```
MSX   : [ <openmsx>, "-machine", "C-BIOS_MSX1", "-carta", <exact .mx1 path> ]
MSX2  : [ <openmsx>, "-machine", "C-BIOS_MSX2", "-carta", <exact .mx2 path> ]
```

- `-machine C-BIOS_MSX1` / `C-BIOS_MSX2` — pins the deterministic,
  ROM-included, freely-licensed machine that matches EmuWiz's resolved
  platform; removes dependence on the unseen `default_machine` setting.
  `C-BIOS_MSX2+` is an acceptable alternative for `MSX2` (it is the upstream
  default) but `C-BIOS_MSX2` is the faithful match.
- `-carta <path>` — pins the ROM to cartridge slot A explicitly rather than
  relying on bare-filename extension sniffing; equivalent to `-cart`.
- No `-romtype` — EmuWiz has no verified mapper evidence; openMSX
  auto-detects, and forcing a wrong type would be worse.

### Minimal alternative (honour user's default machine)

```
MSX/MSX2 : [ <openmsx>, "-carta", <exact .mx1|.mx2 path> ]
```

- Drops `-machine`; openMSX uses `default_machine`. Simpler, honours user
  config, but the reviewed contract weakens to "cartridge ROM into slot A of
  whatever machine the user configured", and a missing-ROM fatal error is
  possible if the user's machine needs absent proprietary ROMs.

### Explicitly rejected for v1

```
[ <openmsx>, <bare path> ]                         # relies on ext sniffing
[ <openmsx>, "-diska", <disk> ]                    # C-BIOS has no disk ROM
[ <openmsx>, "-cassetteplayer", <tape> ]           # C-BIOS has no MSX-BASIC
[ <openmsx>, "-machine", <real machine>, … ]       # needs proprietary ROMs; no evidence to choose one
[ <openmsx>, "-ext", …, "-hda", … ]                # out of scope
[ <openmsx>, "-command", "set save_settings_on_exit off", … ]  # EmuWiz reconfiguring the emulator
```

## Readiness / blocker model

The future adapter must validate, before offering a candidate:

**BLOCKER (fail closed):**

- executable path not absolute / not found / not a regular file / not
  executable (unix mode `& 0o111`).
- captured executable identity `(device, inode, size, mtime)` no longer
  matches the value captured at authorization (stale verification evidence).
- resolved `CanonicalIdentityStatus` is `Unknown` / `Conflicting`.
- resolved `platform_id` is not `MSX` (for the MSX candidate) / not `MSX2`
  (for the MSX2 candidate).
- content path not absolute / is a symlink / not a regular file.
- content extension is not the platform's strong extension
  (`.mx1` for MSX, `.mx2` for MSX2); any archive / mount-input container.
- captured content identity no longer matches authorization
  (changed-before-spawn).
- candidate `target` is not a standalone `openmsx` target
  (`Snes9x*CandidateRequired`-style guard) — no RetroArch fallback.
- launch binding could not be resolved (no safe executable / ambiguous
  executable).
- **machine-profile mode only:** the pinned/selected machine definition is
  not discoverable, or (real machine) its required `systemroms` are
  `Missing`.

**WARNING / USER SETUP REQUIRED (surface, do not block):**

- machine relies on the unseen `default_machine` setting (minimal-argv mode):
  `FirmwareReadiness::Unknown`, "openMSX will use its configured default
  machine; EmuWiz cannot verify which".
- real-machine profile whose `systemroms` are `PresentUnverified`.
- MSX1 (`.mx1`) content about to run on an MSX2+ default machine (minimal-argv
  mode): informational machine-mismatch warning.
- openMSX will create/update `~/.openMSX/` config on exit (informational;
  openMSX-owned, not EmuWiz).

**Final pre-spawn revalidation** (mirrors Snes9x/Mesen `preflight_*`):
re-`symlink_metadata` the executable and content, re-check regular/executable,
re-compare captured identities, re-resolve the binding, re-confirm identity
platform + game key, then hand a `PreparedProcessCommand` to
`process_spawn::spawn_watched_process`.

## RetroArch coexistence

- MSX/MSX2 have no `LAUNCH_COMPATIBILITY` row today, so there are currently no
  *reviewed* RetroArch core hints — but RetroArch candidates still generate
  from an installed `bluemsx` / `fmsx` core's own `.info`.
- Adding an openMSX row must set `retroarch_core_hints` to the reviewed MSX
  cores (`bluemsx`, and optionally `fmsx`) so the GUI's RetroArch row and the
  planner's hint-ranking keep working. It must **not** remove or narrow any
  existing hint behaviour (there is none to remove).
- The future adapter must leave openMSX-standalone and RetroArch as **two
  separate candidates** for the same platform, exactly like Snes9x + Mesen +
  RetroArch on `SNES`: no automatic winner, no fallback, `apply_preference`
  untouched.

## GUI implications

- With a `LAUNCH_COMPATIBILITY` row added, `emulator_setup_page.rs` shows an
  openMSX candidate and a RetroArch candidate for MSX/MSX2 automatically —
  **no hardcoded GUI row** (do not add one to the SameBoy/DOSBox
  "unintegrated" list).
- `adapter_name` needs `"openmsx" => "openMSX"`.
- `launch_readiness_page.rs` `candidate_emulator_name` (single-adapter
  blocker naming) may gain `"openmsx" => "openMSX"`; the `adapter => adapter`
  fallback already covers it if omitted (matches how RMG/Mesen were left).
- **Machine selection needs a GUI seam only if the pinned-C-BIOS-plus-override
  model is chosen.** Minimum future seam: Emulator Setup lets the user pick,
  per MSX/MSX2, one of `{C-BIOS (default), <a discovered real machine>}` and,
  for a real machine, shows its `systemroms` readiness. Do **not** redesign
  Emulator Setup; do **not** modify any GUI file in this task.

## V1 classification

Per-platform (one exact class each):

| Platform | V1 class |
| --- | --- |
| **MSX** | `DEFER_V1` (full adapter) — or `READY_WITH_MACHINE_PROFILE_SEAM` for a cartridge-only, `-machine C-BIOS_MSX1` adapter |
| **MSX2** | `DEFER_V1` (full adapter) — or `READY_WITH_MACHINE_PROFILE_SEAM` for a cartridge-only, `-machine C-BIOS_MSX2` adapter |

Per-content-type:

| Content | V1 class |
| --- | --- |
| `.mx1` cartridge (MSX strong) | `SAFE_DIRECT` — needs machine-profile seam to be *reviewed* |
| `.mx2` cartridge (MSX2 strong) | `SAFE_DIRECT` — needs machine-profile seam to be *reviewed* |
| `.rom` cartridge (weak/shared) | `AMBIGUOUS` → `UNSUPPORTED_FOR_V1` |
| `.dsk` and other disk images | `UNSUPPORTED_FOR_V1` (C-BIOS has no disk ROM; real machine needs proprietary ROMs) |
| `.cas` / `.wav` / `.tsx` cassette | `UNSUPPORTED_FOR_V1` (C-BIOS has no MSX-BASIC) |
| hard disk / laserdisc / `-setup` | `UNSUPPORTED_FOR_V1` |

## Full matrix

| PLATFORM | CONTENT | OPENMSX MODE | MACHINE SELECTION REQUIRED? | FIRMWARE CONDITION | V1 CLASS | RECOMMENDATION |
| --- | --- | --- | --- | --- | --- | --- |
| MSX | `.mx1` cartridge | `-carta` (slot A) | Yes — pin `-machine C-BIOS_MSX1`, or accept unseen `default_machine` | NOT_REQUIRED (C-BIOS) / UNKNOWN (user default) | SAFE_DIRECT | Adapter only after machine-profile seam; else cartridge-only `-machine C-BIOS_MSX1` adapter |
| MSX | `.rom` cartridge | `-carta` | Yes | as above | UNSUPPORTED_FOR_V1 | Refuse — weak/ambiguous, mapper unverified |
| MSX | `.dsk` disk | `-diska` | Yes — real machine w/ disk ROM | REQUIRED, user-supplied, unverifiable per-machine | UNSUPPORTED_FOR_V1 | Refuse |
| MSX | `.cas` tape | `-cassetteplayer` | Yes — real machine w/ MSX-BASIC | REQUIRED, user-supplied | UNSUPPORTED_FOR_V1 | Refuse |
| MSX2 | `.mx2` cartridge | `-carta` | Yes — pin `-machine C-BIOS_MSX2`, or accept unseen `default_machine` | NOT_REQUIRED (C-BIOS) / UNKNOWN (user default) | SAFE_DIRECT | Adapter only after machine-profile seam; else cartridge-only `-machine C-BIOS_MSX2` adapter |
| MSX2 | `.rom` cartridge | `-carta` | Yes | as above | UNSUPPORTED_FOR_V1 | Refuse |
| MSX2 | `.dsk` disk | `-diska` | Yes — real machine w/ disk ROM | REQUIRED, user-supplied | UNSUPPORTED_FOR_V1 | Refuse |
| MSX2 | `.cas` tape | `-cassetteplayer` | Yes — real machine w/ MSX-BASIC | REQUIRED, user-supplied | UNSUPPORTED_FOR_V1 | Refuse |

## Multi-machine profile design

Options considered:

- **A. One openMSX installation profile + user-selected machine
  configuration** (default = C-BIOS per platform, optional real-machine
  override with its own `systemroms` readiness). Mirrors Hatari
  (`HatariProfile` + `HatariMachineModel` + `--configfile`, TOS path from
  config) and DOSBox (`-conf`).
- B. One standalone candidate per discovered openMSX machine — explodes the
  candidate list; most machines are unusable without user ROMs; rejected.
- C. Default-machine launch only — hides the machine question rather than
  answering it; a `.mx1` silently running on MSX2+ is exactly the kind of
  unreviewed inference EmuWiz avoids; rejected as the *reviewed* model
  (acceptable only as the "minimal alternative" argv, clearly labelled
  Unknown).
- **D. Defer the adapter until a machine-profile seam exists.**

**Chosen: D for the full adapter, with A as the shape the seam should take.**
The in-flight `feature/vice-c64-adapter` lane needs the identical seam (VICE is
multi-machine: C64/C128/VIC-20/PET/Plus4). Building the openMSX adapter on top
of whatever machine-profile seam VICE introduces avoids inventing a
second, divergent one. If openMSX is wanted sooner, ship option A restricted
to `{C-BIOS default}` (no real-machine override yet) — that is the
`READY_WITH_MACHINE_PROFILE_SEAM` path and it is a small, self-contained
seam.

## Future implementation plan

Assuming the machine-profile seam exists (option A), the adapter mirrors
Snes9x/Mesen plus a small machine field:

**Likely files (new):**

- `crates/archivefs-core/src/patch_manager/openmsx_local.rs` — bounded
  read-only discovery: PATH `openmsx` + explicit executables; profile
  eligibility; discovered bundled machine list (parse `share/machines/*.xml`
  names read-only, or a fixed C-BIOS allowlist for the minimal version);
  per-machine `systemroms` probe for real machines; re-verifying launch
  binding resolver. No config written, no binary executed.
- `crates/archivefs-core/src/launch/openmsx_command.rs` —
  `OPENMSX_SUPPORTED_PLATFORM_IDS = &["MSX", "MSX2"]`;
  `direct_openmsx_extension` (`.mx1` for MSX, `.mx2` for MSX2);
  `build_openmsx_command_plan` producing
  `[exe, "-machine", <machine>, "-carta", <rom>]`; blockers as above.
- `crates/archivefs-core/src/launch/openmsx_execution.rs` —
  `OpenMsxLaunchRequest` (+ `expected_machine`), `preflight_openmsx_launch`,
  `spawn_openmsx`.

**Shared-seam edits (sibling additions only):**

- `launch/mod.rs` — `pub mod openmsx_command; pub mod openmsx_execution;` +
  re-exports.
- `patch_manager/mod.rs` — `mod openmsx_local;` + re-exports.
- `launch/readiness.rs` — new `LaunchBlockerKind` variants
  `OpenMsxCandidateRequired`, `OpenMsxPlatformMismatch`,
  `OpenMsxContentFormatUnsupported`, `OpenMsxBindingUnavailable`,
  `OpenMsxMachineUnavailable`, `OpenMsxSystemRomMissing`.
- `launch/platform_map.rs` — new `LAUNCH_COMPATIBILITY` rows:
  `MSX` → `standalone_adapters: &["openmsx"]`,
  `retroarch_core_hints: &["bluemsx"]`, `StronglyKnown`;
  `MSX2` likewise. Update `every_required_platform_is_covered` /
  `classic_console_rows_*` if MSX/MSX2 get added to their lists.
- `launch/integration.rs` — `DiscoveredStandaloneProfile::OpenMsx { profile }`
  variant + `openmsx()` constructor + projection arm gated on
  `identity.platform_id == "MSX" | "MSX2"`, `FirmwareReadiness::NotRequired`
  (C-BIOS) / projected `systemroms` readiness (real machine).
- `crates/archivefs-gui/src/emulator_setup_page.rs` —
  `adapter_name`: `"openmsx" => "openMSX"`; optional coexistence test.
- `crates/archivefs-gui/src/launch_readiness_page.rs` — optional
  `"openmsx" => "openMSX"` in `candidate_emulator_name`.

**Focused tests:** native discovery / no-installation / non-executable;
`MSX`+`.mx1` and `MSX2`+`.mx2` accepted; non-MSX platform refused; `.rom` /
`.dsk` / `.cas` refused; exact argv incl. `-machine` + `-carta`; exact ROM
path preserved; no shell; missing/stale executable refused; no RetroArch
fallback; shared registration (`platforms_for_standalone_adapter("openmsx")`);
openMSX + RetroArch coexist as separate candidates; no auto-winner; C-BIOS
implies `NOT_REQUIRED` firmware; real-machine missing `systemroms` →
`Missing`/blocker.

**Estimated scope:** ~3 new files + ~7 shared-seam files, ≈ 1,100–1,400
inserted lines, ≈ 20–24 focused tests — comparable to Snes9x/Mesen, plus a
small machine field and one `systemroms` probe.

### Conflicts / overlap with current adapter lanes

Active lanes touching the identical shared seams:

- `feature/stella-atari2600-adapter` (locked worktree
  `agent-a282a76954614a6f3`, `4538b13`) — Atari 2600.
- `feature/vice-c64-adapter` (`9bfa5aa`) — VICE C64 (**and** the
  machine-profile seam this audit depends on).

Every one of these edits the same set as RMG/Mesen/Snes9x did:
`launch/integration.rs`, `launch/mod.rs`, `launch/platform_map.rs`
(rows + the `classic_console_*` / `every_required_platform_*` tests),
`launch/readiness.rs`, `patch_manager/mod.rs`,
`crates/archivefs-gui/src/emulator_setup_page.rs`,
`crates/archivefs-gui/src/launch_readiness_page.rs`. Expect **sibling-entry
conflicts** (adjacent enum variants, match arms, re-export lists, table rows,
test branches) with whichever of Stella/VICE lands first. Reconcile narrowly
onto the current authoritative HEAD; never copy a stale shared file wholesale.
Sequence openMSX **after** the VICE machine-profile seam is promoted.

## Definition of Done

The future openMSX adapter is done when:

1. A machine-profile seam exists (shared, from the VICE lane or purpose-built
   as option A) — **or** the project explicitly accepts the cartridge-only,
   pinned-`C-BIOS` `READY_WITH_MACHINE_PROFILE_SEAM` scope.
2. `MSX` and `MSX2` gain reviewed `LAUNCH_COMPATIBILITY` rows
   (`standalone_adapters: &["openmsx"]`, `retroarch_core_hints: &["bluemsx"]`)
   without removing or narrowing any existing behaviour.
3. Discovery recognises PATH `openmsx` + explicit executables only; **no
   generic AppImage execution**.
4. Command plan is exactly
   `[ <verified openmsx>, "-machine", "C-BIOS_MSX1|C-BIOS_MSX2", "-carta",
   <exact .mx1|.mx2 path> ]` — no shell, no string concat, inherited
   environment, no `-command`/`-setting`, exact ROM path preserved, C-BIOS
   pinned (or an explicit user-selected machine + verified-present
   `systemroms`), no automatic emulator configuration.
5. Content is limited to the platform strong extension (`.mx1` / `.mx2`);
   `.rom`, `.dsk`, `.cas`, `.zip`, archives, and every disk/tape/HD form are
   refused with a structured blocker.
6. Readiness fails closed for: missing/non-regular/non-executable/stale
   executable, non-MSX/MSX2 identity, unsupported content, unavailable
   binding, unavailable pinned machine, missing real-machine `systemroms`;
   and only warns for: unseen `default_machine`, `PresentUnverified`
   `systemroms`, MSX1-on-MSX2+ mismatch, openMSX-owned config creation.
7. openMSX standalone and RetroArch remain separate MSX/MSX2 candidates with
   no automatic winner and no fallback.
8. Immediate pre-spawn re-validation of executable + content is preserved.
9. EmuWiz never downloads, copies, names, or redistributes any proprietary
   MSX system ROM, and never writes openMSX configuration.
10. Focused tests (list above) pass, `cargo test -p archivefs-core launch`
    stays green, `cargo check -p archivefs-gui` passes, `cargo fmt --all --
    --check` and `git diff --check` are clean.
