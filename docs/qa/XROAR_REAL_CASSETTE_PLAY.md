# XRoar real cassette playback — Job #21

Validated 2026-10-25 on Linux, starting from fetched `origin/main`
`fdff2a86c8746e079113d7a9cf7319269e2fd1e0`. Inspected merged
[PR #100](https://github.com/kiehntre/emuwiz/pull/100), merge `5194e15f`,
and its head `06c1df97`. That PR's stub/synthetic tests did not establish
playback. This run used the unchanged production Blue executable built from
the starting main and a real native XRoar 1.12.1 build (SDL2 UI).

**Result: genuine Dragon 32 cassette gameplay succeeds through Blue.**
Cross Trex loaded, accepted Space to jump, cleared an obstacle and advanced
its score to `0001`. Blue reported Running and then Process exited after
XRoar's normal Ctrl+Q quit. No automated cassette loader or production
launch-boundary change is needed for this supported journey.

## Reproducible media and evidence

Use the author's [Cross-Lib trexBETA release](https://github.com/Fabrizio-Caruso/CROSS-LIB/releases/tag/trexBETA),
asset [Xtrex_coco_dragon_mono.cas](https://github.com/Fabrizio-Caruso/CROSS-LIB/releases/download/trexBETA/Xtrex_coco_dragon_mono.cas).
The filename in the release differs from the TOSEC title; renaming does not
establish identity. The complete downloaded bytes matched:

| Field | Observed value |
| --- | --- |
| TOSEC title | `Cross Trex (2024-09-25)(Caruso, Fabrizio)[mono]` |
| Size | 12348 bytes |
| CRC32 | `607b4f1d` |
| MD5 | `5f721d732f3c690a6583d593229995c6` |
| SHA-1 | `1b69517465788eed19838d7b4e3f1cc759200fe4` |
| DAT | `Dragon Data Dragon - Games - [CAS] (TOSEC-v2025-01-15_CM).dat` |
| DAT SHA-256 | `8429cdcac1e3bfd7b45b39f119e0f9153ed835abe315ee35664b794f05c085bf` |

The [release's licence](https://github.com/Fabrizio-Caruso/CROSS-LIB/blob/trexBETA/docs/LICENCE.txt)
permits noncommercial use with attribution/origin, modification-marking and
notice-preservation restrictions. Credit Fabrizio Caruso, preserve that
notice with any test-media package, and use the original binary. This is an
author-distributed test game, not an inferred permission from a DAT entry.
No cassette or BIOS binary is added to this repository.

Copied and enabled all 36 preserved classic CAS DATs examined for PR #100,
including both Dragon and CoCo catalogues. Cross Trex matched only the Dragon
Games DAT on all four fields. Blue's own cassette panel displayed that exact
match and the DAT digest. DAT headers remain local trust anchors; Blue does
not authenticate official provenance. This run does not establish absence
of collisions in catalogues outside that registered corpus.

## Exact supported journey

1. Obtain [XRoar 1.12.1](https://www.6809.org.uk/xroar/), the test cassette
   and the matching DAT above. Register the DAT as an enabled **file** source
   in DAT Management. Keep any other relevant catalogues enabled so ambiguity
   checks can consult them. Folder sources or unreadable enabled DATs refuse.
2. Use a disposable HOME and XDG config/data/cache/state/runtime/TMPDIR, plus
   explicit `EMUWIZ_CONFIG_HOME` and `EMUWIZ_DATA_HOME`. Point each at private
   directories. Put a regular native `xroar` executable first on PATH.
   Give only the private collection copy to the emulator.
3. Before starting Blue, set `EMUWIZ_XROAR_MACHINE=dragon32` and
   `EMUWIZ_XROAR_FIRMWARE_DIRECTORY` to a private directory containing
   `d32.rom` (16384 bytes). This run's actual ROM CRC32 was `e3879310`, SHA-1
   `f2dab125673e653995a83bf6b793e3390ec7f65a`; it booted Dragon's BASIC
   Interpreter 1.0. The private validation copy came from the
   [upstream online emulator](https://www.6809.org.uk/xroar/online/)'s
   `d32.rom` resource. Firmware is excluded from the committed artifacts.
   EmuWiz checks readable canonical files and sizes, not BIOS authenticity;
   its firmware state is **present, unverified**.
4. In Blue, add the private game folder, review and confirm its scan, select
   the cassette in Library, and choose Play. The scan can still show
   **Unknown system / Not checked**: launch-time cassette verification is
   separate from a persisted library DAT audit. Expand **Dragon / CoCo
   cassette** to inspect the exact-match evidence and manual loading advice.
5. Choose **Launch xroar**. Actual child arguments were:

   ```text
   xroar -no-c -machine dragon32 -no-machine-cart \
     -load-tape "/private/collection/Cross Trex (2024-09-25)(Caruso, Fabrizio)[mono].cas" \
     -extbas /private/firmware/d32.rom
   ```

   All actual paths were absolute, including the executable; the working
   directory was its installation's `bin` directory. `-no-c` bypasses user
   XRoar configuration, and `-no-machine-cart` prevents automatic disk-cart
   selection. There was no output tape, disk or startup script argument.
6. Focus XRoar. At the BASIC `OK` prompt type **`CLOADM`**, then Enter.
   Fast tape loading is enabled by XRoar's defaults. This particular cassette
   starts its own loader/game; **EXEC was not required**. At **PRESS A KEY**,
   press Space to start. Press Space again at an approaching cactus to jump.
   Observed the dinosaur on the ground, airborne after input, then score
   `0001` after clearing the obstacle. Do not treat a title screen as proof
   of interactive gameplay.
7. Quit XRoar with **Ctrl+Q**. Blue changed from **Running** to **Process
   exited**, its active launch job ended, and the recorded child PID was gone.
   These labels describe process lifecycle; Blue does not detect tape-load
   success or observe gameplay.

The run used private Xvfb display `:121`, software rendering and dummy audio,
with keyboard events sent to the focused emulator. Sound quality and a real
desktop/controller were not validated. Short synthetic key pulses sometimes
missed the emulated keyboard scan; deliberate key holds were used for the
successful run. No key injection is added to EmuWiz.

## Machine revisions and loading limits

| Blue machine selection | XRoar profile | Required canonical BIOS files | Playback evidence in this job |
| --- | --- | --- | --- |
| `dragon32` | `dragon32` | `d32.rom`, 16384 bytes | Cross Trex: load, start, jump, score, normal exit |
| `dragon64` | `dragon64` | `d64_1.rom` and `d64_2.rom`, 16384 bytes each | Not tested |
| `coco2` | `coco` | `bas13.rom` and `extbas11.rom`, 8192 bytes each | Not tested; the selected built-in profile is PAL CoCo |
| `coco3` | `coco3` | `coco3.rom`, 32768 bytes | Not tested |

TOSEC family evidence does not establish required RAM, video standard, exact
hardware revision or bootability. Do not select a CoCo profile merely because
this release's filename says `coco_dragon`: Blue uses the registered DAT
evidence, which is Dragon-only for these bytes. A CoCo claim needs its own
unambiguous compatible cassette and real playback check.

Only direct `.cas` media is in this Blue launch contract. For other tapes the
[XRoar manual](https://www.6809.org.uk/xroar/doc/xroar.shtml) distinguishes
attaching input from running it: BASIC commonly needs `CLOAD` then `RUN`;
machine code commonly needs `CLOADM` then `EXEC`. Some tapes self-start,
have multiple parts, or use special loaders. Follow the title's instructions;
there is no universal load command or timing guarantee.

Other observed limits:

- Author-distributed Blockdown 1.0 did not match these DATs. No fake catalogue
  record was created to make it eligible.
- Cross Bomber's 2020 Dragon asset exactly matches both Dragon and CoCo Games
  DATs (5999 bytes, SHA-1 `45dea0491993368e23e279e2ae06d388d195f0c0`).
  That is ambiguous under the current contract; do not disable a relevant
  catalogue just to obtain Play.
- Cross Chase's 2020 Dragon asset matched unambiguously and loaded through
  Blue into its game field, but the displayed I/J/K/L controls did not produce
  confirmed movement in this setup. It is not counted as a playback pass.
- Once the selected XRoar launch exits, this UI displays the historical result
  without an XRoar relaunch/recheck button. Restarting the disposable Blue
  instance allowed a retry. The smallest retry improvement would reuse
  `StandaloneLaunchState::start` with a freshly rechecked XRoar request, as
  other standalone adapters do; it needs its own focused test and independent
  launch-boundary review. It is not necessary for the successful first launch
  and was not implemented here.

## Pathname race assessment

Traced the actual boundary: Blue's `Request::execute_with` rehashes the tape
and reimports registered catalogues, then core's `preflight_xroar_launch`
rechecks the selected file identity, machine profile, executable path and
readable ROMs. `spawn_xroar` passes paths to `spawn_watched_process`, which
uses `Command::new` and separate argument values, without a shell.

Those are point-in-time checks, **not atomic consumption guarantees**. The
kernel subsequently resolves the executable pathname, and XRoar subsequently
opens the tape/ROM pathnames. A concurrent rename, symlink swap or in-place
write after the final check can invalidate earlier evidence. The executable
check also does not authenticate executable bytes. This known limitation
remains; a successful run in stable private directories does not disprove it.

For the specific preservation question, inspected XRoar 1.12.1's
`tape_open_reading` and CAS open path: `-load-tape` opens the input as `rb`.
Writing requires a separate tape-output operation. The observed argv used
`-no-c`, no automatic cartridge and no output tape. No destructive-write
reproduction or new privilege-boundary violation was established. Accordingly
this job does not redesign executable/ROM/media handoff or alter the shared
process spawner. Such hardening would require a separately specified threat
model and independent review; an extra pathname stat alone would not close
the final race.

## Evidence, preservation and validation

Host artifacts: `/home/davedap/emuwiz-qa-artifacts/job21-xroar-play/`.
`evidence/environment.json` and `start-session.py` preserve the isolated
launch setup. `cassette-catalogue-evidence.json` records exact media matches
across the 36 DATs. `trex-blue-child.json` records the real executable, argv,
cwd and PID; `trex-exit-observation.json` records child disappearance.

Key screenshots: `trex-loaded.png`, `trex-running-before.png`, `trex-jump.png`,
`trex-running-after.png`, `trex-blue-running.png`, `trex-blue-exited.png`.
The corresponding first-cassette `blue-catalogue.png` shows the catalogue
digest and the distinction between launch evidence and library audit status.
No screenshot generated by a stub is used as playback evidence.

Downloads, working copies, firmware, DATs, profiles, database, display and test
roots all stayed outside the repository. Original media and source DAT bytes
are preserved; final comparisons are recorded in `preservation-check.json`.
The real user's collection was never passed to the emulator and their HOME,
XDG settings, library database, saves and emulator profiles were not selected.

Focused and final local validation results are recorded under `evidence/`.
This change is documentation-only; no regression tests or launch-boundary
implementation were added, so an independent launch-boundary review is not
required. GitHub CI on a new branch requires an authorized push; local checks
must not be described as a hosted CI run. Cargo uses one target for the one
Job #21 worktree, with the lifecycle release performed after all builds and
the disposable GUI have stopped.
