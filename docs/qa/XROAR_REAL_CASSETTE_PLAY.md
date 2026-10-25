# XRoar real cassette playback — Job #21

The first report below records PR #136. The [follow-up](#follow-up-retry-machine-matrix-and-launch-boundary) supersedes its retry, untested-machine and pathname-handoff limitations.

## Initial validation (PR #136)

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


## Follow-up: retry, machine matrix and launch boundary

Started from fetched `origin/main` `b2bb4dc0c597c3bf3adf2b42548afadf3e1e70b4`,
the merge of [PR #136](https://github.com/kiehntre/emuwiz/pull/136), and inspected
PR #100 again. Reused the original XRoar 1.12.1 native SDL2 build, Trex bytes
and 36 registered CAS catalogues. Corrected production tree tested here:
`399562200f77c44cdd878fd964259901b387c7a2`. Later test-initializer/documentation
changes do not alter that production tree.

### Retry and explicit loading

The exit watcher was working. Blue's shared launch renderer returned after
showing the selected XRoar attempt's historical Exited/Failed state, with no
XRoar retry action. **Launch XRoar again (recheck)** now starts the existing
request worker again. It reimports catalogues, rehashes the current media and
rechecks machine/firmware/executable readiness; it does not replay a cached
command. The egui regression first failed on the absent button, then passed,
including refusal when cassette bytes change before the retry click.

Real Dragon 32: one Blue PID `1774239`, first XRoar PID `1779475`, second PID
`1787118`. Each launch loaded Trex with `CLOADM`, accepted Space to start and
jump, and reached score `0001`. Ctrl+Q produced **Process exited**, then the
new button produced **Running** without restarting Blue. Both emulator PIDs
and both private launch aliases disappeared after their normal exits.

The [XRoar manual's Running programs section](https://www.6809.org.uk/xroar/doc/xroar.shtml#Running-programs)
already documents File → Run / Ctrl+Shift+L and `-run`: XRoar can detect the
first tape program's type and issue its loading commands. This is distinct
from attaching input with `-load-tape`. Gold has no control channel to an
already running XRoar; Blue now supplies **conditional guidance** for XRoar
builds with a file chooser, plus the existing manual commands. It does not
silently run tapes, send keys, add `-run`, or invent a loader. The tested
SDL2 build has no GTK file chooser; that optional menu journey was not
validated. Use the already inserted tape and the BASIC prompt in this build.
Choosing another file inside XRoar is an explicit user operation outside
Blue's prepared initial-asset binding.

### Real machine results

All rows below passed actual loading, input response and normal Ctrl+Q exit
through Blue on the corrected production tree. All required BIOS bytes were
observed through the child's bound descriptors. **Present, unverified** is
still the BIOS readiness state; neither filename nor canonical size proves
BIOS authenticity. No BIOS binary is committed or licensed by this report.

| Blue selection | Actual XRoar 1.12.1 built-in configuration | Private ROMs (bytes; CRC32) | Cassette, commands and observed response |
| --- | --- | --- | --- |
| `dragon32` | `dragon32`, PAL, 32 KiB | `d32.rom` (16384; `e3879310`) | Trex above; `CLOADM` self-starts; Space start/jump, score `0001`; same-session retry repeats gameplay |
| `dragon64` | `dragon64`, PAL, 64 KiB RAM; standard 32K-compatible BASIC boot | `d64_1.rom` (16384; `84f68bf9`), `d64_2.rom` (16384; `17893a42`) | Same Dragon-only Trex bytes; `CLOADM` self-starts; Space jump, score `0001` |
| `coco2` | **`coco`**, PAL, 64 KiB, original 6847; not the separate CoCo 2B/T1 profile | `bas13.rom` (8192; `d8f4d15e`), `extbas11.rom` (8192; `a82a6254`) | Cross Snake v1.0; `CLOADM` self-starts; Space starts/confirms level; L/I/J visibly change snake heading right/up/left |
| `coco3` | `coco3`, NTSC, GIME 1986, **512 KiB** | `coco3.rom` (32768; `b4c88d6c`) | Cross Bomber 2023 CoCo3; `CLOADM` self-starts; Space starts and drops bombs; visible bomb, building count `23` → `22`, score `0000` → `0010` |

These defaults are in the tested XRoar source `src/xroar.c`'s built-in machine
configurations. No machine defaults or Dragon 32 recipe were changed.
Dragon 64 alternate 64K BASIC mode, CoCo 2B/T1, NTSC CoCo 1/2, PAL CoCo 3,
6309 variants, real hardware, controllers and sound quality were not tested.
Passing a Dragon title in Dragon 64's compatible boot mode does not establish
that title's use of alternate RAM or the second BASIC bank. Neither family
identity nor this matrix promises every cassette will load.

The two additional originals come from Fabrizio Caruso's author releases:
[Cross Snake XSnake1.0](https://github.com/Fabrizio-Caruso/CROSS-LIB/releases/download/XSnake1.0/Xsnake_coco.cas)
and [Cross Bomber cmoc](https://github.com/Fabrizio-Caruso/CROSS-LIB/releases/download/cmoc/Xbomber_coco3.cas).
Their respective tagged [Snake licence](https://github.com/Fabrizio-Caruso/CROSS-LIB/blob/XSnake1.0/docs/LICENCE.txt)
and [Bomber licence](https://github.com/Fabrizio-Caruso/CROSS-LIB/blob/cmoc/docs/LICENCE.txt)
permit noncommercial use subject to origin/notice/modification conditions.
Original binaries and notices are preserved in the private evidence, with
credit to Fabrizio Caruso; no game binaries are committed.

| Exact catalogue identity | Size | CRC32 | MD5 | SHA-1 |
| --- | --- | --- | --- | --- |
| `Cross Snake v1.0 (2020-12-12)(Caruso, Fabrizio)` | 18946 | `6e299624` | `47943fe0cfe841c9a66f2511721c9085` | `04c4f3913273d4a728246b77613dec15ede6de84` |
| `Cross Bomber (2023-10-26)(Caruso, Fabrizio)(CoCo 3)` | 6153 | `c134d82c` | `33a1a6d783f86dd97368b5cad02e023f` | `b074dd7191d7ff9961601a237a8ba868b98bb86f` |

Both match only the Tandy Games CAS DAT across the 36 registered catalogues:
`Tandy Radio Shack TRS-80 Color Computer - Games - [CAS]`, artifact SHA-256
`f7bd1493d8b770d104bbdc7b708f889141138a2503c0f214c47062fc77dea313`.
Blue displayed that exact whole-file match in each session. Trex remains
Dragon-only under the registered evidence; it was not forced into CoCo by its
release filename. Existing ambiguity, unreadable/enabled DAT, disabled/folder
source, machine-family, changed identity and missing/wrong BIOS refusals remain.

For reproduction, follow the initial journey with a separate disposable
profile per row, select the row's `EMUWIZ_XROAR_MACHINE`, provide all its
canonical ROM files through `EMUWIZ_XROAR_FIRMWARE_DIRECTORY`, and use its
exact cassette/DAT. The runtime still uses `-no-c`, `-no-machine-cart` and
`-load-tape`. Type `CLOADM` then Enter for these three self-starting test
programs; no `EXEC` was needed. Other machine-code tapes may need `EXEC`;
BASIC tapes commonly need `CLOAD` then `RUN`. Follow the game's instructions.

### Reproduced race and bounded correction

A deterministic regression against the former real Rust preflight/spawn
boundary showed both pathname replacement and in-place mutation could make
the child consume replacement executable/tape/ROM bytes. Independent native
XRoar instrumentation also observed a swapped cassette opened for reading.
This establishes a validation-to-consumption integrity defect even without
claiming a reproduced destructive write.

XRoar now spawns only a prepared launch. On Linux, it opens regular non-symlink
sources, checks bounded stable capture and the cassette's exact size/SHA-1/
MD5/CRC32, then seals anonymous copies of executable, cassette and canonical
ROMs. The child consumes inherited descriptors. A private mode-0700 retained
directory provides `input.cas` through its directory descriptor: XRoar needs
the `.cas` suffix to select its tape parser. Bare `/proc/self/fd/N` does not
preserve that parser selection. Its cwd is bound to the same private directory.
The watcher retains assets until child exit even if Blue drops its handle;
cleanup is nonrecursive and identity checked.

Source execution permission is checked with descriptor-bound effective
`faccessat2(X_OK)`, including ACL/noexec policy, before/after capture and
immediately before spawn. Setuid/setgid sources refuse. Parent descriptors
remain CLOEXEC; only required descriptors above stdin/stdout/stderr are
inherited by the child. Shared-spawner callers retain their existing behavior.
There is no path-only fallback and no permission/sysctl override.

This handoff requires Linux with usable procfs, memfd sealing/execution and
`faccessat2` support. Other platforms or denied kernel capabilities fail closed.
Executable capture is limited to **64 MiB**, cassette to **8 MiB**; ROMs retain
canonical exact-size checks and an explicit firmware directory is required.
Native XRoar 1.12.1 is the tested executable. Arbitrary wrappers, children that
outlive the watched process, executable self-location/`$ORIGIN` assumptions
and other emulator versions are not established by this result.

This is initial launch-asset binding, not an OS sandbox or executable/BIOS
authentication. Host interpreters and shared libraries, deliberate same-UID
tampering with private alias contents and permission changes after the last
check remain outside its guarantee. Metadata stamps do not authenticate
executable/ROM content or prove every concurrent write is detected; cassette
hashes validate the actual captured bytes. Explicit later media changes inside
XRoar likewise belong to the emulator's user interface.

### Follow-up evidence and gates

Artifacts are retained separately at
`/home/davedap/emuwiz-qa-artifacts/job21-xroar-followup/`; the PR #136 evidence
is preserved. `sessions/{dragon32,dragon64,coco2,coco3}/` contains isolated
environments, process/argument/asset hashes, exact-match screenshots, real
loading/gameplay and Blue exit screens. `evidence/playback-matrix.json`
records outcomes; Dragon 32 `08-trex-jump.png`, `09-trex-score.png`,
`14-retry-jump.png`, `15-retry-score.png` prove both attempts. Dragon 64
`07-trex-jump.png`/`08-trex-score.png`, CoCo `10-snake-right.png`/
`11-snake-up.png`, and CoCo 3 `play-02.png`/`play-09.png` show input response.
Keyboard driving is external test instrumentation only, with deliberate
150–200 ms holds for the emulated scan; it is not an EmuWiz feature.

`evidence/preservation-final.json` records 57 passing comparisons/isolation/
exit/alias checks: all original/collection cassette hashes and six private ROM
hashes are unchanged; the 36 catalogue paths/digests are retained. User HOME,
XDG roots, collections, saves and emulator configuration were not selected.
Each real session had its own private roots and Xvfb display `:181`; only owned
Blue/XRoar/Xvfb processes were ended. No media, ROM, generated database or
screenshot is added to the repository.

The pre-fix retry regression and two pre-fix asset-race failures are preserved.
The corrected combined focused gate passed **49 core tests and 18 GUI tests**,
with one existing ignored test. These include source replacement/in-place
mutation, all cassette hash refusals, permission/noexec/setid checks, closed
stdio, sealed snapshots, alias preservation/cleanup and watcher lifetime.
Independent read-only safety review of `39956220` found no blocking issue,
subject to native playback, normal CI, these limits and exact final-SHA review.
Final hosted CI/review/integration and Cargo handoff results belong in the PR
and retained completion evidence; they must not be inferred from local tests.
