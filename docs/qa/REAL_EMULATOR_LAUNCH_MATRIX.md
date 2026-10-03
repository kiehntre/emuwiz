# Real Emulator Launch Matrix

> **Recovered historical research — status against current main (`b66422c2`).**
> Source: branch `qa/real-emulator-launch-matrix` at `c10c9fcb`. Recovered unchanged below this block except where marked `[refreshed]`.
> - **QA framework and a historical snapshot (2026-09-22, main `41c9a645`) — not launch evidence.** It records *no* completed real launch: every row is "Not tested". No row may be read as a pass, and no untested row has been converted to one here.
> - See "Result taxonomy" at the end for the classifications and the rule for counting a launch.


Date: 2026-09-22

This is a bounded, read-only QA record for the Saltbox host. It deliberately
does not turn a launch plan or a process `--version` probe into proof that a
game reached an emulator. A real launch is counted only when the selected
EmuWiz path starts the selected installed emulator in the user desktop and
the source media remains unchanged.

## Build and host

- QA branch: `qa/real-emulator-launch-matrix`
- Base commit: `41c9a645464a3fec9fa68cad7de78cfadd19cf12`
- Source worktree was clean apart from the pre-existing untracked research
  documents in the authoritative checkout; those files were not copied or
  modified.
- No ROM, BIOS, save, configuration, or database mutation was performed.
- The shell session had no `DISPLAY`, `WAYLAND_DISPLAY`, or desktop session
  environment. The host X server was present on `:0`, but it was root-owned
  and an authenticated user probe could not connect within the bounded probe.

## Inventory evidence

Native executable candidates discovered without launching them:

| Adapter | Selected native path | Result |
|---|---|---|
| RetroArch | `/home/davedap/.local/bin/retroarch` | installed candidate; no game launch attempted |
| DuckStation | `/home/davedap/.local/bin/duckstation` | installed candidate; no game launch attempted |
| RPCS3 | `/home/davedap/.local/bin/rpcs3` | installed candidate; no game launch attempted |
| Dolphin | `/usr/bin/dolphin` | installed candidate; no game launch attempted |
| MAME | `/home/davedap/.local/bin/mame` | installed candidate; no game launch attempted |
| Hatari | `/usr/bin/hatari` | installed candidate; no game launch attempted |
| FS-UAE | `/usr/bin/fs-uae` | installed candidate; no game launch attempted |
| xemu | `/home/davedap/.local/bin/xemu` | installed candidate; no game launch attempted |
| ScummVM | `/usr/games/scummvm` | installed candidate; no game launch attempted |
| DOSBox | `/usr/bin/dosbox` | installed candidate; no game launch attempted |
| VICE x64 | `/usr/bin/x64` | installed candidate; no game launch attempted |
| VICE x64sc | `/home/davedap/.local/bin/x64sc` | installed candidate; no game launch attempted |

Flatpak candidates discovered:

| Adapter | App ID | Version/ref shown by Flatpak |
|---|---|---|
| PCSX2 | `net.pcsx2.PCSX2` | `v2.8.2` |
| PPSSPP | `org.ppsspp.PPSSPP` | `1.20.4` |
| RetroArch | `org.libretro.RetroArch` | `1.22.2` |
| Dolphin | `org.DolphinEmu.dolphin-emu` | `2606a` |
| RPCS3 | `net.rpcs3.RPCS3` | two installed refs were listed |
| xemu | `app.xemu.xemu` | `0.8.136` |
| Ryujinx | `io.github.ryubing.Ryujinx` | `1.3.3` |

The duplicate RPCS3 Flatpak entries are an installation-selection issue, not
permission to silently choose one. No Flatpak game launch was attempted.

## Real catalogue/media evidence

The production catalogue was queried read-only. It contains real entries for
representative launch shapes, including:

- SNES, NES, Game Boy, Game Boy Advance, MegaDrive, Master System, N64,
  PlayStation, Dreamcast, Atari ST, Amiga CD32, PC Engine CD, Arcade, Xbox,
  and Commodore 64.
- direct images, ZIP, 7z, RAR, MegaDrive ROM records, and catalogue evidence
  for multi-file/optical topologies.
- 34 Arcade logical records, so raw chip members were not selected as an
  independent test game.
- real source paths with spaces, apostrophes, Unicode, and nested folders are
  present in the catalogue; no path was renamed or rewritten for this QA.

This evidence establishes suitable test candidates, but not a successful
launch. A bounded desktop launch was not possible in the available session.

## Launch matrix

| Platform/media shape | Adapter candidate | Installation binding | Launch result | Classification |
|---|---|---|---|---|
| SNES single ROM/ZIP | RetroArch | native and Flatpak candidates exist | Not tested | HostDisplay |
| PS1 CUE/BIN or M3U | DuckStation | native candidate | Not tested | HostDisplay |
| PS2 ISO/DVD image | PCSX2 | Flatpak app ID available | Not tested | HostDisplay |
| PSP ISO/CSO | PPSSPP | Flatpak app ID available | Not tested | HostDisplay |
| PS3 game directory | RPCS3 | native and duplicate Flatpak refs | Not tested | HostDisplay / selection review |
| GameCube/Wii ISO | Dolphin | native and Flatpak candidates | Not tested | HostDisplay |
| Dreamcast GDI/CDI | Flycast | no executable candidate confirmed | Not tested | Unknown |
| Atari ST floppy image | Hatari | native candidate | Not tested | HostDisplay |
| Amiga floppy/HDF | FS-UAE | native candidate | Not tested | HostDisplay |
| xemu ISO/XISO | xemu | native and Flatpak candidates | Not tested | HostDisplay |
| MAME extracted logical set | MAME | native candidate | Not tested | HostDisplay |
| FBNeo logical set | FBNeo | no executable candidate confirmed | Not tested | Unknown |
| ScummVM directory | ScummVM | native candidate | Not tested | HostDisplay |
| DOS directory/game | DOSBox | native candidate | Not tested | HostDisplay |
| C64 tape/disk | VICE | native candidates | Not tested | HostDisplay |
| Vita3K title | Vita3K | no executable candidate confirmed | Not tested | Unknown |
| Cemu title | Cemu | no executable candidate confirmed | Not tested | Unknown |
| RMG / Stella / mGBA / melonDS / DeSmuME / SameBoy / Mesen / Snes9x / Amiberry | respective adapter | no native candidate confirmed in bounded PATH inventory | Not tested | Unknown |

No row is marked successful. No generated command was executed because the
required user desktop could not be authenticated. This is intentional: a
headless process start would not prove the requested visual/game acceptance.

## Safety checks

- No source-media pre/post hash was taken because no real launch occurred.
- No source file was opened for writing.
- No temporary projection, rename, mount, patch, conversion, or save/config
  change was performed.
- No adapter bug was fixed; there was no reproducible EmuWiz failure to fix.
- The duplicate RPCS3 Flatpak installation is recorded as a lifecycle
  selection risk. The correct behavior is to preserve explicit selection and
  refuse ambiguity, not silently switch installations.

## Existing automated coverage

The integrated source contains focused launch/readiness and process-spawn
tests for RetroArch, PCSX2, PPSSPP, Dolphin, Hatari, xemu, Xenia, Vita3K,
Amiberry, DOSBox, ScummVM, VICE, and other adapters. Those tests validate
typed command construction, final revalidation, safe spawning, and failure
classification against controlled fixtures. They are not a substitute for
this missing desktop proof.

## Not tested / next required run

The following remain unproven until the QA runner is attached to the actual
Sunshine/XFCE user display:

- every real game launch in the matrix;
- process/window survival and clean exit;
- visual confirmation that the intended game reached the intended emulator;
- Flatpak portal/sandbox media access;
- AppImage/native selected-binding enforcement during a real launch;
- BIOS-missing blocked launch in the GUI;
- Arcade parent/clone and FBNeo launches;
- M3U companion preservation;
- source hash before/after a real launch;
- escaped-write detection during real emulator startup.

The blocker is classified as `HostDisplay`, not `EmuWizBug`. The next run
must start from the Sunshine/XFCE session with its authenticated display
environment and retain per-launch stdout/stderr, process status, selected
binding, source hash comparison, and a bounded screenshot or equivalent
window-observation evidence.


## Result taxonomy (added on recovery)

These definitions describe how future runs of this matrix must classify results. They are not results.

- **HostDisplay** — the run could not reach an authenticated user desktop (as in the snapshot above). A blocker of the QA host, never counted against EmuWiz.
- **EmuWizBug** — a reproducible EmuWiz failure with retained evidence. Only this class gets an adapter fix.
- **NotProven** — evidence is missing, a bounded probe timed out, or the row was not run. This is the default for every row; a launch plan, a command preview or a `--version` probe never upgrades it.
- **Unknown** — no executable or installation candidate was confirmed for the adapter, so a run was not possible (the snapshot's "Unknown" rows).

A launch counts as a pass **only if** the selected EmuWiz path starts the selected installed emulator in the user's desktop session, the intended game is visibly or observably running, the process exits cleanly, and a source-media hash taken before and after is unchanged. Retain per-launch stdout/stderr, process status, the selected binding, and a bounded screenshot or equivalent window observation.

**Duplicate-installation risk:** the snapshot found two installed RPCS3 Flatpak refs. EmuWiz must keep the explicit selection and refuse the ambiguity rather than silently choose one; treat any matrix run that depends on a silently chosen duplicate as `NotProven`.
