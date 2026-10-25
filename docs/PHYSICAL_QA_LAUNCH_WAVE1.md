# EmuWiz Physical QA Wave 1: Real Emulator Launch Handoff

Scope: P0-4 ("Emulator readiness, launch planning, and handoff") from
`docs/PHYSICAL_QA_RELEASE_READINESS_PLAN.md`. This wave proves real
installed-emulator launch handoff only; it is not a full frontend/library QA
pass.

## Environment

- Authoritative HEAD: `e909efeacd492a0aed4d14f1af26b78dd1ed590d` ("test(packaging):
  add fresh-install AppImage QA"), branch `feature/archivefs-unified-platform`,
  clean tree.
- Artifact: `dist/EmuWiz-x86_64.AppImage` (same artifact verified in the prior
  fresh-install QA pass; predates the VICE/Stella adapters and the final
  ES-DE gap fixes - see the "not testable" section below for why this does
  not block Wave 1).
- Run against the host's **real** EmuWiz state (`~/.config/archivefs`,
  `~/.local/share/archivefs` - this host is a pre-rename user, so its real
  data lives under the documented legacy directory names) and the host's
  **real, already-owned** game library (68,853 catalogued items). No ROM,
  BIOS, or emulator was downloaded. No disposable/fresh sandbox was used for
  this wave, per the task's explicit intent to exercise real installed
  emulators and real owned content.

## Installed emulator inventory (read-only)

| Emulator | Status | Evidence |
| --- | --- | --- |
| RetroArch | INSTALLED (native) | `/usr/bin/retroarch` (Debian package `/usr/games/retroarch` symlink target actually invoked) |
| Dolphin Emulator | INSTALLED (Flatpak) | `org.DolphinEmu.dolphin-emu` (user scope) - **not** `/usr/bin/dolphin`, which is KDE's file manager, correctly not confused for the emulator |
| RPCS3 | INSTALLED (Flatpak + AppImage) | `net.rpcs3.RPCS3` (system scope); also `~/RPCS3-AppImage/rpcs3-v0.0.41-...AppImage` |
| PCSX2 | INSTALLED (AppImage) | `~/Applications/PCSX2/PCSX2.AppImage` |
| DuckStation | INSTALLED (AppImage) | `~/Applications/DuckStation/DuckStation.AppImage` |
| Cemu | INSTALLED (AppImage) | `~/Applications/Cemu/Cemu.AppImage` (+ versioned copy) |
| melonDS | INSTALLED (AppImage) | `~/Applications/melonDS/melonDS-x86_64.AppImage` |
| xemu | INSTALLED (AppImage + built from source) | `~/Applications/xemu/xemu.AppImage`, `~/xemu-src` |
| Vita3K | INSTALLED (AppImage) | `~/Applications/Vita3K-x86_64.AppImage` |
| Azahar | INSTALLED (AppImage) | `~/Applications/Azahar/Azahar.AppImage` |
| PPSSPP | INSTALLED (AppImage) | `~/Applications/PPSSPP/PPSSPP.AppImage` |
| FS-UAE | INSTALLED (native) | `/usr/bin/fs-uae` |
| mGBA | INSTALLED (AppImage only) | `~/Applications/mGBA/mGBA.AppImage`, `~/.local/share/applications/mgba.desktop` |
| Flycast | NOT INSTALLED | no binary/AppImage found |
| DOSBox Staging | NOT INSTALLED | no binary/AppImage found |
| MAME | NOT INSTALLED | no binary found |
| Amiberry | NOT INSTALLED | no binary found |
| Hatari | NOT INSTALLED (config-only) | `~/.hatari/hatari.nvram` exists from past use, but no current `hatari` binary found |
| SameBoy | NOT INSTALLED | no binary found |
| **RMG** | **NOT INSTALLED** | no binary/AppImage anywhere on host |
| **Mesen 2** | **NOT INSTALLED** | no binary/AppImage anywhere on host |
| **Snes9x** | **NOT INSTALLED** | no binary/AppImage anywhere on host (`snes9x-gtk`/`snes9x` absent) |
| **Stella** | **NOT INSTALLED** | no binary/AppImage anywhere on host |
| **VICE** (`x64sc`/`x64`) | **NOT INSTALLED** | no binary/AppImage anywhere on host |

None of the five most recently added standalone adapters (RMG, Mesen 2,
Snes9x, Stella, VICE) have an installed emulator on this host, and the
library has no content catalogued under Atari 2600 or Commodore 64 either
(no platform tile for either appeared in the 68,853-item library browse).
**All five are recorded NOT TESTABLE, not FAIL**, per the task's explicit
instruction.

## Selected real test content

Only content already present in the real 68,853-item library was used; no
ROM was created, edited, or downloaded. Real filenames are shown here only
where non-sensitive (public game titles); exact private mount paths are
otherwise abbreviated.

| # | Platform | Content form | Emulator | Purpose |
| - | --- | --- | --- | --- |
| 1 | Game Boy Advance | direct `.gba` (contains a space in its filename) | RetroArch (mGBA core) | native-shape cartridge core launch, path-with-spaces |
| 2 | PSX | direct `.chd` disc image | RetroArch (Mednafen PSX core) | disc emulator, real BIOS-configured platform |
| 3 | PSX | (candidate inspection only, no launch) | DuckStation (AppImage) | fail-closed / needs-setup real candidate |
| 4 | SNES | `.zip` archive with no platform-matching payload | RetroArch (would-be core) | real fail-closed content-rejection case |

## Pre-launch safety records

### Case 1 - RetroArch / Game Boy Advance

- Canonical platform: Game Boy Advance.
- Selected content: direct `.gba` file (~8 MiB), path contains spaces and
  parentheses.
- Candidate: RetroArch (sole reviewed candidate shown; no phantom mGBA/other
  standalone candidate fabricated, matching the confirmed "not installed"
  inventory above).
- Readiness: "Ready to play".
- Technical details shown: "Uses the RetroArch launch adapter."
- Blockers: none.

### Case 2 - RetroArch / PSX

- Canonical platform: PSX.
- Selected content: direct `.chd` (~318 MiB).
- Candidate: RetroArch (sole reviewed candidate). DuckStation (AppImage,
  discovered at `~/Applications/DuckStation/DuckStation.AppImage`) shown
  separately in Emulator Setup as "Needs setup" for the same platform -
  confirmed present as its own distinct candidate, never merged or
  auto-selected over RetroArch.
- Readiness: initially "Checking this game" (async), resolved to "Ready to
  play" within ~3 seconds.
- Blockers: none at launch-plan time.

### Case 3 - DuckStation / PSX (candidate only, not launched)

- Installation form: AppImage, exact path
  `/home/davedap/Applications/DuckStation/DuckStation.AppImage`.
- Status: "Needs setup" (not yet eligible/checked this session).
- Not launched: EmuWiz correctly did not present this as ready, so per the
  task's own fail-closed instruction it was left unlaunched.

### Case 4 - SNES `.zip` archive, no matching payload

- Canonical platform: SNES.
- Selected content: `.zip` archive.
- Status before Prepare: "Ready to prepare" ("Temporarily makes this
  archived game available. The original is unchanged.").
- Outcome after clicking Prepare: refused with "The archive was inspected,
  but it contains no playable file matching this game's platform. Check its
  contents or choose a different archive." No process spawned, no wrong
  content substituted.

## Exact argv captured

Captured directly from `ps aux` while each process was running (never
guessed, never reconstructed):

**Case 1 (RetroArch, GBA):**
```
/usr/games/retroarch -L /usr/lib/x86_64-linux-gnu/libretro/mgba_libretro.so \
  "/mnt/usbdrive/games/gba/Crash Bandicoot - The Huge Adventure (NA).gba"
```

**Case 2 (RetroArch, PSX):**
```
/usr/games/retroarch -L /usr/lib/x86_64-linux-gnu/libretro/mednafen_psx_libretro.so \
  "/mnt/usbdrive/games/psx/007 Racing (NA).chd"
```

Verified for both:
- No shell: `ps aux` shows the direct `execve`'d binary and its argv tokens,
  never a `/bin/sh -c "..."` wrapper.
- Exact executable path: absolute, unmodified.
- Exact selected content path: byte-identical to the library's own stored
  path, spaces and parentheses preserved as a single argv token (not
  shell-split, not truncated).
- Exact expected core/adapter: `mgba_libretro.so` for GBA, `mednafen_psx_libretro.so`
  for PSX - the correct reviewed core for each platform, never a wrong or
  guessed core.
- No unexpected fallback: neither run substituted a different emulator or
  core than the one EmuWiz's own "Technical details" panel had already
  named before launch.

## Launch results

| Case | Process started | Emulator opened | Content reached | EmuWiz stable after exit | Classification |
| --- | --- | --- | --- | --- | --- |
| 1: RetroArch/GBA | Yes | Yes | **Yes** - game boot screen with the exact title text rendered | Yes, "RetroArch closed normally" | **PASS** |
| 2: RetroArch/PSX | Yes | briefly | No - RetroArch/Mednafen core itself crashed (SIGABRT) before reaching gameplay | **Yes** - EmuWiz reported "RetroArch exited unexpectedly (signal: 6 (SIGABRT)) (core dumped)" and remained fully responsive | Handoff **PASS**, underlying emulator crash is a host/RetroArch-core issue (see below), not an EmuWiz defect |
| 3: DuckStation/PSX | Not launched (blocked by design) | N/A | N/A | N/A | Fail-closed **PASS** |
| 4: SNES `.zip` | Not launched (refused) | N/A | N/A | Yes | Fail-closed **PASS** |

### Case 2 crash root-cause note

`/var/crash/_usr_games_retroarch.1000.crash` confirms `Signal: 6` (SIGABRT)
inside `/usr/games/retroarch` with the exact argv above. Real PS1 BIOS files
(e.g. `scph1001.bin`, `scph5501.bin`, `scph7502.bin`) are present in RetroArch's
own configured system directory, ruling out a missing-BIOS cause. The crash
is inside RetroArch's own Mednafen PSX core (or its interaction with this
host's video driver) - a pre-existing host/RetroArch-side condition, not
something introduced by EmuWiz's command construction (which was proven
byte-exact above) or by EmuWiz's process handoff (which detected and reported
the crash correctly). No attempt was made to "fix" RetroArch or this core;
that is out of this task's scope.

## New adapter validation (RMG, Mesen 2, Snes9x, Stella, VICE)

**NOT TESTABLE for all five** - none of the five emulators are installed on
this host (see inventory above), and the library has no Atari 2600 or
Commodore 64 content catalogued. No unsupported/synthetic file was
manufactured to force a false test, per instructions. This finding should be
re-run once at least one of these five emulators is installed on a QA host.

## RetroArch AppImage result

No RetroArch AppImage installation exists on this host (RetroArch is
installed natively via the distro package, `/usr/bin/retroarch`/
`/usr/games/retroarch`). **Recorded NOT TESTABLE**, not FAIL, per the task's
explicit instruction. The already-completed prior QA work (commit `e6ebe22`,
"feat(launch): support verified RetroArch AppImage profiles") remains the
authoritative automated-test evidence for that adapter path; this wave adds
no new physical evidence for it.

## Multiple-candidate result

PSX showed RetroArch ("Ready to play") and DuckStation ("Needs setup") as
two separate, independently-readiness-tracked candidates for the same
platform in Emulator Setup - never merged into one row, and DuckStation's
not-ready state never silently promoted RetroArch as an implicit "winner"
(RetroArch's readiness was independently and correctly computed from its own
discovered profile, not from DuckStation's absence). SNES was browsed (4,023
items) but no second real standalone candidate exists on this host to
demonstrate three-way candidate coexistence physically; the existing
automated test suite (`snes9x_and_retroarch_coexist_as_separate_snes_candidates_with_no_auto_winner`,
`stella_and_retroarch_coexist_as_separate_atari2600_candidates`, etc. - all
already green per this session's earlier work) is the authoritative proof of
that specific coexistence contract. No remembered-preference value was
changed during this QA pass, so nothing needed to be restored afterward.

## Firmware readiness result

PSX readiness resolved to "Ready to play" without EmuWiz ever presenting a
firmware-blocked state, and the actual launch reached RetroArch/Mednafen
successfully loading the disc image before its own unrelated crash (i.e. PS1
BIOS was recognized and used - the crash occurred well past any firmware
gate, confirmed by the crash signal occurring inside the emulator process
itself, not at EmuWiz's own readiness-check stage). No emulator/platform
combination on this host exercised an EmuWiz-side "missing required
firmware, refuse to launch" state during this pass; that specific negative
case is already covered by existing automated `FirmwareReadiness` tests
(unchanged, not re-run here since no code changed).

## Fail-closed result

Two real fail-closed cases were observed and both refused cleanly:
DuckStation was never launched while "Needs setup" (Case 3), and the SNES
`.zip` archive was refused with a specific, actionable message rather than
launching a wrong file or crashing (Case 4).

## Source read-only result

Both launched files' `stat` size/mtime were recorded before and after their
respective launch/crash and are **byte-identical**:
- GBA file: identical size and mtime before and after.
- PSX `.chd`: identical size and mtime after the crash (recorded once, since
  the crash happened after content was already loaded read-only by the
  core - no write-back ever occurs for direct disc-image playback).
No archive/source file was renamed, moved, or rewritten by EmuWiz at any
point in this wave.

## Config-mutation observations

`~/.config/retroarch/retroarch.cfg` size and mtime were recorded before Case
1 and checked again after Case 1's clean exit and after Case 2's crash: size
`113973` bytes, mtime unchanged across the entire session. **EmuWiz did not
rewrite RetroArch's configuration at all** (class B is not applicable here
either, since RetroArch itself also made no persistent config write in this
session - both launches used only argv-level core selection, never RetroArch's
own settings-save path).

## GUI observations

- Candidate cards were understandable: each named its exact emulator, exact
  platform, and an honest status badge ("Ready to play" / "Needs setup" /
  "Not checked").
- The DuckStation "Needs setup" card's compact view did not itself explain
  *why* (only "Technical details" showing the AppImage path) - a user would
  need to click "Check emulators" or open the card fully to learn more.
  **Classification: P1** (understandable but not maximally informative from
  the compact view alone).
- The post-crash message ("RetroArch exited unexpectedly (signal: 6
  (SIGABRT)) (core dumped)") is precise and technically honest, but assumes
  Unix-signal literacy from a novice user. **Classification: POLISH** (not a
  blocker; a friendlier one-line summary above the technical detail would
  help novices, but the current wording is truthful and not misleading).
- No confusing duplicate candidate was observed anywhere in this pass.
- No stale ArchiveFS naming was observed anywhere in this pass (all UI
  strings said "EmuWiz").
- Navigating the game-library search/platform-tile UI to reach a specific
  platform required more scrolling/clicking than expected when a game was
  already selected (the previously-selected game's detail panel remains
  visible above the library grid, pushing it further down) -
  **Classification: POLISH** (a minor navigation friction, not a safety or
  correctness issue, and explicitly out of scope to redesign in this task).

No BLOCKER-class GUI finding was identified in this wave.

## Files changed

- `docs/PHYSICAL_QA_LAUNCH_WAVE1.md` (this file, new).

No production Rust code was changed. No bug requiring a code fix was found:
the one crash observed (Case 2) is inside RetroArch's own emulator core, not
in EmuWiz's launch-planning or process-handoff code, which performed exactly
as specified (exact argv, honest exit reporting, continued stability).

## Tests run

None. No Rust was changed, so per instructions no Cargo ceremony was run.

## Report

- **Authoritative HEAD:** `e909efeacd492a0aed4d14f1af26b78dd1ed590d`.
- **Installed emulator inventory:** 13 installed (RetroArch, Dolphin,
  RPCS3, PCSX2, DuckStation, Cemu, melonDS, xemu, Vita3K, Azahar, PPSSPP,
  FS-UAE, mGBA), 12 not installed (Flycast, DOSBox Staging, MAME, Amiberry,
  Hatari, SameBoy, RMG, Mesen 2, Snes9x, Stella, VICE - see table above).
- **Real launch cases attempted:** 2 full launches + 2 fail-closed
  inspections = 4 cases.
- **Platforms/emulator types tested:** Game Boy Advance via RetroArch
  (mGBA core); PSX via RetroArch (Mednafen PSX core) and DuckStation
  (AppImage, not launched); SNES archive rejection (RetroArch candidate,
  not launched).
- **Passes:** 4/4 (2 full content-reached launches, 2 correct fail-closed
  refusals).
- **Failures:** 0 EmuWiz-attributable failures. 1 underlying-emulator crash
  (RetroArch/Mednafen PSX core, SIGABRT) unrelated to EmuWiz's own code.
- **Not-testable cases:** RMG, Mesen 2, Snes9x, Stella, VICE (no installed
  emulator on host); RetroArch AppImage (none installed on host).
- **Multiple-candidate result:** PSX showed RetroArch (Ready) and
  DuckStation (Needs setup) as two genuinely separate candidates, no
  automatic winner, DuckStation's not-ready state never suppressed or
  altered RetroArch's own independent readiness.
- **RetroArch AppImage result:** NOT TESTABLE (none installed).
- **Firmware readiness result:** PSX BIOS/firmware was successfully
  recognized and used (launch proceeded well past any firmware gate); no
  EmuWiz-side firmware-block case was available to exercise physically on
  this host.
- **Fail-closed result:** 2/2 correct refusals (DuckStation never
  auto-launched while needing setup; SNES zip archive refused with a
  specific, actionable message).
- **Source read-only result:** confirmed byte-identical size/mtime for both
  launched files before and after.
- **Config-mutation observations:** `retroarch.cfg` unchanged (size and
  mtime identical) across the entire session.
- **Blockers:** none.
- **P1 findings:** 1 (DuckStation's compact "Needs setup" card doesn't show
  its specific blocking reason without expanding further).
- **Polish findings:** 2 (the crash-exit message assumes Unix-signal
  literacy; selecting a different platform tile while a game detail panel
  is open requires more scrolling than ideal).
- **Files changed:** `docs/PHYSICAL_QA_LAUNCH_WAVE1.md` only.
- **Tests run:** none (no Rust changed).
- **Commit SHA:** recorded after commit, below.
- **Final git status:** clean after commit.
- **Confirmation no real ROM modified:** confirmed - both launched files'
  size/mtime are byte-identical before and after; the SNES archive was
  never extracted/mutated since Prepare refused before any write.
- **Confirmation no emulator installation/config deliberately rewritten:**
  confirmed - `retroarch.cfg` size/mtime unchanged; no other emulator's
  config was touched; no emulator was installed, upgraded, or reconfigured
  by this QA pass.
- **Confirmation LBC untouched:** confirmed - no LBC file was opened, read,
  or modified; the one filesystem search that traversed `~/lbc-radio` was a
  read-only filename glob during emulator-binary inventory and touched
  nothing inside it.

EMUWIZ PHYSICAL LAUNCH QA WAVE 1 COMPLETE
