# Native Emulator Adapter Coverage Audit V1

Date: 2026-09-07  
Authority: `feature/archivefs-unified-platform` at `7c349a7ab2093c9649fba9dc56d0b8e8d2ca37ad`

This is a source and history audit only. No emulator adapter was implemented.
The working tree already contains active, unrelated dirty work in the launch
and GUI files; those files were not edited or staged.

## Method and completion rule

The audit checked the committed files at `HEAD`, the reachable history, branch
tips, registered worktrees, and locally discoverable executables. A native
adapter is considered complete only when the committed path has the relevant
discovery/profile seam, platform and content gate, readiness, typed command
plan, process execution/preflight, generic GUI candidate projection, and
focused tests. A command module by itself is recorded as `PARTIAL`.

`GENERIC_RETROARCH_ONLY` means the platform may receive a RetroArch candidate
through the reviewed core-information resolver, but no native standalone
adapter is registered in `LAUNCH_COMPATIBILITY`. It does not mean a core is
installed or that a core is auto-selected.

## Mainline native coverage

The following are present at current authority in `crates/archivefs-core/src/launch/`,
registered through `platform_map.rs`/`integration.rs`, and named by the generic
Emulator Setup projection unless noted.

| Platform | Native emulator / adapter | Status | Discovery | Readiness | Command + execution | GUI / tests |
|---|---|---|---|---|---|---|
| PSX | DuckStation | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Yes / focused tests |
| PS2 | PCSX2 | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Yes / focused tests |
| PS3 | RPCS3 | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Yes / focused tests |
| PSP | PPSSPP | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Yes / focused tests |
| PlayStation Vita | Vita3K | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Generic candidate / tests |
| GameCube, Wii | Dolphin | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Generic candidate / tests |
| Dreamcast | Flycast | `IMPLEMENTED_MAIN` | Yes | BIOS-aware | Yes | Generic candidate / tests |
| Xbox | xemu | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Yes / focused tests |
| Xbox 360 | Xenia | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Yes / focused tests |
| Wii U | Cemu | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Generic candidate / tests |
| Nintendo 3DS | Azahar | `IMPLEMENTED_MAIN` | Yes | Yes | Yes | Generic candidate / tests |
| Nintendo Switch | Ryujinx | `ABSENT` | No EmuWiz profile seam | No | No | No native candidate; Flatpak presence is external to EmuWiz |
| Nintendo DS | melonDS | `IMPLEMENTED_MAIN` | Yes | Firmware-aware | Yes | Generic candidate / tests |
| Nintendo DS | DeSmuME | `IMPLEMENTED_MAIN` | Yes, including profile discovery | Yes | Yes | Emulator Setup / tests |
| Game Boy, Game Boy Color | mGBA, SameBoy, Mesen 2 | `IMPLEMENTED_MAIN` | Yes | Not required or adapter-specific | Yes | Generic candidates / tests |
| Game Boy Advance | mGBA, Mesen 2 | `IMPLEMENTED_MAIN` | Yes | Not required | Yes | Generic candidates / tests |
| NES, PC Engine, WonderSwan families | Mesen 2 | `IMPLEMENTED_MAIN` | Yes | Not required | Yes | Generic candidate / tests |
| SNES | Mesen 2, Snes9x | `IMPLEMENTED_MAIN` | Yes | Not required | Yes | Separate candidates / tests |
| Commodore 64 | VICE | `IMPLEMENTED_MAIN` | Yes | Not required | Yes, fresh preflight | Emulator Setup / launch tests |
| Atari ST | Hatari | `IMPLEMENTED_MAIN` | Yes | TOS-aware | Yes | Generic candidate / tests |
| Amiga WHDLoad | Amiberry, FS-UAE | `IMPLEMENTED_MAIN` | Existing profile/discovery | Kickstart/profile-aware | Yes, plus WHDLoad execution | GUI WHDLoad seam / tests |
| Amiga CD32 / CDTV | Amiberry | `IMPLEMENTED_MAIN` | Yes | Kickstart/profile-aware | Yes | Generic candidate / tests |
| N64 | RMG | `IMPLEMENTED_MAIN` | Yes | Not required | Yes | Generic candidate / tests |
| Atari 2600 | Stella | `IMPLEMENTED_MAIN` | Yes | Not required | Yes | Generic candidate / tests |
| Arcade | MAME, FBNeo | `IMPLEMENTED_MAIN` | Yes / DAT or set-aware where required | Adapter-specific | Yes | Generic candidates / tests |
| ScummVM | ScummVM | `IMPLEMENTED_MAIN` | Detector-verified game identity | Not required | Yes | Selected-game candidate / tests |
| DOS | DOSBox Staging | `IMPLEMENTED_MAIN` | Verified DOSBox configuration | Config-aware | Yes | Generic candidate / tests |
| MSX2 cartridge path | openMSX | `PARTIAL` | Yes | `NotRequired` | Command planning only | Setup/candidate projection / planner tests |

The openMSX classification is intentionally conservative. `openmsx_command.rs`
and `openmsx_local` provide the reviewed `.mx1`/`.mx2` content gate, executable
binding, machine selection, and exact argv, but `HEAD` has no
`openmsx_execution.rs` or equivalent native process-preflight seam. It is not
counted as a complete adapter under the rule above. The same command planner
must be reused if execution is completed later.

### RetroArch

RetroArch itself is `IMPLEMENTED_MAIN` as the generic downstream adapter. It
uses discovered core metadata and strict platform identity already resolved by
the identity layer. It is not a native replacement for the platform-specific
rows above and does not auto-select a core.

## Branch and worktree reconciliation

No adapter was found that exists only on a side branch while being absent from
the authority. Several side tips are older source/reconcile lanes or carry
follow-up work:

| Side lane | Relation to authority | What it proves |
|---|---|---|
| `feature/desmume-adapter` at `7722ca7` | Tip not ancestor | The side branch contains the original DeSmuME implementation; current authority already has DeSmuME through `38aefe1` and `ad18fb`. Do not promote the old branch wholesale. |
| `feature/fsuae-adapter` at `260ea4c` | Tip is ancestor; worktree has uncommitted files | FS-UAE is already mainline. The worktree's dirty follow-up is not authority and is not needed for this audit. |
| `feature/vice-c64-adapter` at `9bfa5aa` | Tip is ancestor; worktree has uncommitted files | VICE is already mainline. No duplicate VICE adapter should be built. |
| `integration/scummvm-plus-gamerview` at `eec3595` | Tip not ancestor; worktree has dirty Gamer View files | Mainline ScummVM launch integration is present through `bb2e166`; the worktree is a separate GUI/enrichment lane, not a missing native adapter. |
| `feature/rmg-n64-adapter` at `963dd0` | Tip not ancestor / registered locked worktree metadata | Mainline RMG exists through `070715e`; do not promote the stale branch. |
| `feature/stella-atari2600-adapter` at `4538b1` | Tip not ancestor / registered locked worktree metadata | Mainline Stella exists through `e66dcea`; Spectrum/Fuse work remains out of scope. |
| Hatari, Cemu, Azahar, SameBoy, Flycast, Mesen, mGBA, Snes9x, MAME/FBNeo, Vita3K branches | Mixed stale or reconcile tips | Current authority already contains the relevant native files and registrations where shown in the mainline table. Branch-tip non-ancestry is not evidence that the adapter is absent. |

The current authority also has active dirty Fuse/Spectrum-related launch and
GUI changes (`fuse_command.rs` and related shared launch files). That lane is
owned elsewhere and is explicitly excluded from the recommendations below.

## Platforms with no native adapter in current authority

These platforms are supported by identity/media evidence to varying degrees,
but have no native standalone row in `LAUNCH_COMPATIBILITY` at `HEAD`.

| Platform | Candidate native emulator | Current status | RetroArch alternative | Native value assessment |
|---|---|---|---|---|
| Atari 8-bit | Atari800 | `GENERIC_RETROARCH_ONLY` / native absent | Atari800-family libretro core where installed and metadata-resolved | Strong identity/media evidence and a mature Linux emulator make this a credible native target. |
| Amstrad CPC | Caprice32 | `GENERIC_RETROARCH_ONLY` / native absent | Caprice32/libretro or MAME | Useful, but native CLI/profile contract and modern Linux packaging need verification. |
| BBC Micro / Acorn Electron | BeebEm or b-em | `GENERIC_RETROARCH_ONLY` / native absent | MAME or an available libretro core | Identity exists, but native Linux distribution and deterministic CLI/profile contract are less uniform. |
| Dragon / CoCo | XRoar | `GENERIC_RETROARCH_ONLY` / native absent | MAME/libretro where available | XRoar is a plausible native target with useful cassette/media semantics; contract research is still required. |
| PC-98 / NEC PC-9801 | Neko Project II / NP2kai | `GENERIC_RETROARCH_ONLY` / native absent | MAME/NP2kai core where available | Strong disk evidence exists, but BIOS/config and executable naming need a bounded Linux audit. |
| Sharp X68000 | XM6 TypeG or px68k | `GENERIC_RETROARCH_ONLY` / native absent | MAME/libretro where available | Media identity is improving, but native Linux practicality and deterministic target contract are uncertain. |
| FM Towns | Tsugaru | `GENERIC_RETROARCH_ONLY` / native absent | MAME/libretro where available | Identity evidence exists, but Linux distribution, firmware, and command contract require research. |
| Sega CD | Native standalone target not reviewed | `GENERIC_RETROARCH_ONLY` | Genesis Plus GX | Keep RetroArch-only until a native adapter adds a concrete readiness or compatibility benefit. |
| Mega Drive | Native standalone target not reviewed | `GENERIC_RETROARCH_ONLY` | Genesis Plus GX | Keep RetroArch-only; the native value case is weak compared with the mature core. |
| Virtual Boy | Native standalone target not reviewed | `GENERIC_RETROARCH_ONLY` | Beetle VB / Mednafen VB | Keep RetroArch-only unless a deterministic native target is identified. |
| Atari 5200 | Native standalone target not reviewed | `GENERIC_RETROARCH_ONLY` | Atari 5200 core | Low native priority; no verified executable/profile seam in this repository. |
| Atari Lynx | Native standalone target not reviewed | `GENERIC_RETROARCH_ONLY` | Handy/libretro | Low native priority; no verified executable/profile seam in this repository. |

ZX Spectrum/Fuse is deliberately recorded as `ACTIVE / EXCLUDED`, not as a
free task. The current dirty worktree owns Fuse-related launch files. This
audit did not inspect, modify, stage, or recommend Spectrum/Fuse work.

## Adapter capability findings

For the committed mainline adapters, the usual pattern is a typed command
planner plus a dedicated execution module using the shared process-spawn
preflight. The generic GUI candidate grid is driven from
`LAUNCH_COMPATIBILITY` and names adapters through
`crates/archivefs-gui/src/emulator_setup_page.rs`.

The two meaningful completion gaps found in the committed table are:

1. **openMSX execution seam** — discovery, profile binding, content rules,
   readiness, mapping, GUI candidate, and command planning exist; native
   execution/preflight is absent.
2. **Native adapters for the platforms in the preceding table** — there is no
   reviewed executable/profile/readiness/command/execution contract to audit
   in the authority, so RetroArch must remain the only generic launch option.

No automatic emulator winner is implied by any row. Multiple native adapters
remain separate candidates, and an absent native row is not filled by a
filename guess.

## Local installation discovery

Read-only `command -v` checks on this host found:

- `retroarch` — `/usr/bin/retroarch`
- `dolphin` — `/usr/bin/dolphin`
- `fs-uae` — `/usr/bin/fs-uae`
- `scummvm` — `/usr/games/scummvm`

No direct command was found for the audited candidates or for the other
standalone adapters checked (including `openmsx`, `vice`, `hatari`, `rmg`,
`stella`, `desmume`, `melonds`, `atari800`, `caprice32`, `beebem`, `b-em`,
`xroar`, `dosbox`, `dosbox-staging`, `np2`, `np2kai`, `tsugaru`, `xm6`,
`px68k`, `fuse`, and `zesarux`). Flatpak inspection found Dolphin, RPCS3,
Ryujinx, RetroDECK, and unrelated desktop applications; it did not establish
an installed native adapter for the missing-platform candidates. This is an
installation observation, not a capability claim about the emulators.

## Priorities and recommendation

### P1 — finish openMSX native execution

This is the safest next launch task because the identity, `.mx1`/`.mx2` scope,
profile discovery, `FirmwareReadiness::NotRequired`, command plan, and GUI
candidate already exist. The missing work is a narrowly bounded native
preflight/spawn seam, not a new platform model. It should remain cartridge-only
and reuse the existing process executor. This is a **completion task**, not a
reason to redesign launch planning.

### P2 — Atari 8-bit / Atari800 native adapter audit and implementation

Atari 8-bit has strong structural media evidence (`ATR`, `ATX`, `XFD` and
related reviewed forms) and a plausible mature Linux target. Before coding,
verify executable names, exact media argv, profile/config behavior, firmware
requirements, and no-save/config-isolation semantics. If that contract is not
deterministic, retain RetroArch-only status.

### P3 — Dragon/CoCo / XRoar or Amstrad CPC / Caprice32, after a contract audit

Choose one only after authoritative CLI and profile research. Both have useful
native value, but neither is sufficiently proven by the current repository to
start implementation now. BBC, PC-98, X68000, and FM Towns remain lower until
their Linux target contracts and firmware/profile semantics are equally clear.

### Do not build next

- Spectrum/Fuse: active elsewhere; excluded from this lane.
- VICE, FS-UAE, Amiberry, Hatari, RMG, Stella: already represented in
  authority; do not duplicate side branches.
- Sega CD, Mega Drive, Virtual Boy, Atari 5200, Atari Lynx: RetroArch is the
  appropriate current path unless a concrete native benefit is demonstrated.
- DeSmuME, ScummVM, DOSBox Staging, SameBoy, Azahar, Cemu, Flycast, MAME,
  FBNeo, mGBA, Mesen 2, Snes9x, Vita3K: current authority already has the
  corresponding reviewed native seams.

## Audit conclusion

The authoritative next task is **finish the openMSX execution/preflight seam**.
If product policy requires only entirely new adapters rather than completion
work, the next new-adapter candidate is **Atari800 for Atari 8-bit**, subject to
the CLI audit above. No adapter implementation is included in this document.
