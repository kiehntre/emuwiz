# Job #24, Stage 1: bounded Blue VICE Play

Stage 1 implements the missing Blue launch seam identified in
[the original inventory](JOB24_C64_VICE_TRAINER_BLOCKER.md). It is based on
`origin/main` at `d10374baa87593d632b88b78ec36eeab567c7c30`.
[Issue #111](https://github.com/kiehntre/emuwiz/issues/111) remains open:
trainer ingestion, selection and monitor application are Stage 2, which must
wait for this stage to be merged and validated.

## Supported configuration

- Native `x64sc` 3.7.1 with SHA-256
  `e3fafd35e65ed271011cd694790998bfabe20a91b64acf9efe61d50795d5cc1c`.
  This is the installed Ubuntu/Debian `vice` package `3.7.1+dfsg1-2build3`.
  The exact binary must be rediscoverable through Gold's ordinary PATH
  discovery; symlink wrappers, `x64`, and other binary digests are withheld.
- A direct, regular, single-link `.crt` file with current exact library DAT
  evidence. Gold must still consider every identity-carrying saved claim
  current and verified, with the same audited hashes and a catalogue header
  resolving to Commodore 64. Missing, stale, ambiguous, different-system,
  archive-member and filename-only evidence cannot authorize Play.
- Gold's existing CRT inspector must validate a v1.0, 64-byte-header,
  hardware-type-0 standard ROM cartridge: EXROM=0, GAME=1, exactly one ROM
  CHIP in bank 0, 8192 payload bytes, loaded at `$8000`, and no extra bytes.
  This is a qualification of the existing structural result, not a new parser.
- VICE must already have its system ROMs configured. EmuWiz does not copy ROMs,
  change its settings, configure additional hardware, or prove that the game
  itself will run under those settings.

Other CRT hardware, RAM/flash cartridges, other banks, 16K/Ultimax cartridges,
tapes, disks, archives, alternative builds and libretro are outside this stage.
The new evidence projection does not authorize another emulator lane.

## User journey and launch boundary

Game Details and Launch show a selection-bound VICE readiness worker, its
refusals and a refresh action. A current exact cartridge identity contributes
to the shared launch plan without replacing a conflicting prior identity.
Supported native profiles remain explicit candidates in that planner.

Blue provides an exact executable/argv preview and an explicit Play action.
Rendering, checking readiness and refreshing never spawn VICE. Gamer Play
uses the same typed request. Existing acknowledgement and candidate gates
remain in place; nothing is enabled automatically.

At Play, a worker reloads the source-aware Gold/Blue DAT projection, re-hashes
the cartridge against its recorded cryptographic hashes and checks its CRT
structure again. It compares that evidence and the configured catalogue
digests with the preview and rechecks the exact executable bytes. Gold's
`preflight_vice_launch` then repeats executable discovery and file/identity
checks and constructs the command passed to `spawn_vice`:

```text
/path/to/x64sc +saveres +cart -cartcrt /absolute/path/to/game.crt
```

There is no shell, imported command text, `-moncommands`, trainer or memory-write
operation in this stage. `+saveres` preserves saved resources. The cartridge
remains the original input; EmuWiz opens it read-only and creates no modified
game file. Gold's VICE implementation is unchanged.

Blue retains the watched process, polls it without blocking the UI, and reports
PID, exit status and bounded stderr using the existing standalone lifecycle.
It explicitly distinguishes process start and normal exit from successful
gameplay. A second launch requires another explicit action and fresh checks.

## Validation evidence

The synthetic fixture uses the real DAT audit/persistence path in a temporary
database, then the shared launch planner and Blue's typed request. A fake native
executable records argv and exits; tests verify the preview/preflight/spawn argv,
explicit-click dispatch, fresh-source refusal, process tracking, source
preservation, changed executable, lost execute bit, identity/candidate mismatch,
unsupported binaries/hardware, malformed files, links, special files, ambiguous
DATs and late results from another selection.

Focused regression command (test processes use private HOME/XDG/TMP roots):

```text
scripts/cargo-iso test -p archivefs-gui --lib -- launch_readiness_page:: gui_v2::vice_evidence:: --test-threads=2
```

Result: 226 passed, 0 failed, 1 existing ignored test. Workspace formatting
check, file-scope guard, GUI-root boundary check and `git diff --check` passed.

On 2026-10-25 an installed-backend probe also launched `/usr/bin/x64sc` with
exactly the four arguments shown above and a synthetic standard 8K cartridge.
It used a private Xvfb, HOME/XDG directories and copies of the already-installed
system ROMs. The cartridge's reset program produced its distinctive green
display. Before/after SHA-256 checks of the cartridge, private `vicerc`, and
the three ROM copies matched. The harness terminated its own process after
capturing the display; that termination is not a normal-game-exit claim.
Local evidence is in `/tmp/emuwiz-job24-native-vice-26d2vk4j/`
(`result.json`, `vice.log`, `screen.png`). The temporary probe did not touch
the user's VICE configuration or source ROMs.

The first synthetic DAT fixture published only SHA-256, which the existing
loose-file audit does not compute. Its positive test correctly failed with no
saved exact identity. The fixture now supplies SHA-1 through the same real audit
path; production checks were retained. Independent review also identified an
early preview refusal hiding historical process feedback when the executable
disappeared. A new regression failed before the correction; the panel now keeps
that feedback visible and disables launch controls until preview succeeds again.
Normal CI and an independent launch-safety
review of the final candidate are required before promotion. Stage 2 remains
gated on Stage 1 merge and validation.
