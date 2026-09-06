# AppImage Fresh-Install QA

## Environment

- Authoritative HEAD inspected: `48140d054f24b7ea346a031dac9463ae84835687`
  ("feat(es-de): resolve final safe platform gaps"), branch
  `feature/archivefs-unified-platform`, working tree clean at the start of
  this QA.
- Host: shared, actively-used Linux desktop (X11, NVIDIA driver) with other
  concurrent automated sessions (other agent sandboxes were observed
  building/running `archivefs-gui` from unrelated worktrees during this QA
  window) and the real user's own established EmuWiz/ArchiveFS state. This
  is **not** an isolated CI box, which materially affects one finding below
  (see "Real-HOME leak check").
- Disposable QA root: `/tmp/emuwiz-fresh-appimage-qa/` with `home/`,
  `xdg-config/`, `xdg-data/`, `xdg-cache/`, `test-games/`, `test-mnt/`
  subdirectories, created fresh for this QA and never pointed at the real
  user's directories.
- Synthetic fixtures only: `test-games/nes/qa-fixture.nes` (19-byte iNES
  header stub) and `test-games/nes/empty-fixture.a26` (0 bytes). No
  copyrighted ROM content was used or downloaded.

## Artifact

- `dist/EmuWiz-x86_64.AppImage`: 83,687,928 bytes, mtime
  2026-09-06 00:47:15 +0100, mode `-rwxr-xr-x`.
- SHA-256: `c2bef3b7f3603e03d09e0d4a2a55f3f1a808854afb9992135503b4837a01f5fe`,
  matches `dist/EmuWiz-x86_64.AppImage.sha256` (`sha256sum -c`: OK).
- `--version` (normal mode) and `APPIMAGE_EXTRACT_AND_RUN=1 --version` both
  print `emuwiz 0.8.1-alpha` and exit 0.
- **Limitation:** this artifact was built from commit `c16f486`
  ("feat(packaging): add EmuWiz AppImage build"), which **predates** the
  VICE adapter (`3eb033d`) and the final ES-DE gap fixes (`48140d0`). A
  Cargo release build was active on this shared host for the entire QA
  window (`cargo build --release -p archivefs-gui`, then later
  `cargo check -p archivefs-gui` from a concurrent session), so per the
  "never run heavy Cargo jobs concurrently" instruction this artifact was
  **not** rebuilt. Findings below reflect the packaging/runtime mechanism
  (AppRun, environment handling, path access, onboarding, general adapter
  policy) rather than VICE/Stella-specific behavior, which this artifact
  does not contain.

## Checks performed and results

| # | Check | Result |
| - | - | - |
| 1 | Artifact/checksum/exec-bit verification | PASS |
| 2 | `--version` (normal + extract-and-run) | PASS |
| 3 | Disposable HOME/XDG root creation | PASS |
| 4 | Synthetic fixture creation (no copyrighted ROMs) | PASS |
| 5 | First launch: starts, no missing-library crash, EmuWiz branding | PASS |
| 6 | Onboarding steps 1-5 (Welcome/Source/DAT/Emulator/Verify), Continue/Skip, resume, completion | PASS |
| 7 | Sources/Discovery: add folder by typed path, scan, status wording | PASS |
| 8 | Arbitrary path access (`/tmp` source, native file-picker showing unrestricted host paths) | PASS |
| 9 | `/mnt`-style mount-destination path field | PASS (selection UI itself is a native GTK dialog; see note) |
| 10 | Host PATH tool discovery (ratarmount/fusermount3/7z/retroarch) not masked | PASS |
| 11 | Emulator discovery smoke (RetroArch, Mesen 2, mGBA, Snes9x, Hatari all surfaced) | PASS |
| 12 | AppImage emulator policy (no generic `*.AppImage` execution) | PASS |
| 13 | Restart/persistence (relaunch, same disposable env) | PASS, with one BLOCKER caveat (see below) |
| 14 | Read-only source principle (byte-identical before/after scan) | PASS |
| 15 | Failure-case wording (RetroArch "needs setup", DuckStation/RPCS3/xemu "not installed") | PASS |
| 16 | `APPIMAGE_EXTRACT_AND_RUN=1` full smoke | PASS |
| 17 | Log/stderr review | PASS (see classification) |
| 18 | Real-HOME leak check | INCONCLUSIVE (see below) |
| 19 | Second-launch navigation (Home/Library/Sources/Emulator Setup) | PASS |
| 20 | Automated harness script | PASS |

## First launch

Launched with:

```
env -i HOME=/tmp/emuwiz-fresh-appimage-qa/home \
  XDG_CONFIG_HOME=/tmp/emuwiz-fresh-appimage-qa/xdg-config \
  XDG_DATA_HOME=/tmp/emuwiz-fresh-appimage-qa/xdg-data \
  XDG_CACHE_HOME=/tmp/emuwiz-fresh-appimage-qa/xdg-cache \
  DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/1000 PATH=/usr/bin:/bin \
  ./dist/EmuWiz-x86_64.AppImage
```

The window opened immediately, titled "EmuWiz" (no stale "ArchiveFS"
branding anywhere in the UI), and went straight to first-time onboarding.
stderr showed only one benign warning (`XRandR reported that the display's
0mm in size`) plus clean `INFO:` lines; no missing-library errors, no panic.

**Important, unrequested-but-load-bearing finding:** the app does **not**
honor `XDG_CONFIG_HOME`/`XDG_DATA_HOME`. Files were created at
`$HOME/.config/emuwiz/{onboarding_state.txt,config.toml}` and
`$HOME/.local/share/emuwiz/library.sqlite3` regardless of the exported
`XDG_CONFIG_HOME`/`XDG_DATA_HOME` values. This is confirmed **intentional**,
not a bug: `crates/archivefs-core/src/app_dirs.rs`'s own module doc states
"the XDG base-directory layout mirrors what EmuWiz already used
(`~/.config/archivefs`, `~/.local/share/archivefs`) rather than adopting the
XDG environment variables, so an existing user's data is found at the exact
same place it has always been." `XDG_CACHE_HOME` *was* honored — but only by
the NVIDIA driver's own GL shader cache, unrelated to EmuWiz. Classified
**BENIGN** (by design) — but any future fresh-install documentation or QA
script must sandbox via `HOME` alone, not `XDG_CONFIG_HOME`/`XDG_DATA_HOME`.

## Onboarding QA

All five steps were exercised end-to-end:

1. **Welcome/principles** — lists the four hard guarantees (never
   rename/move/delete a ROM, never download anything, never configure an
   emulator, never require a DAT account) plainly. `Continue`.
2. **Add a source** — typed `/tmp/emuwiz-fresh-appimage-qa/test-games`
   directly into the "Add Folder" path field (no native picker needed for
   this step); `Add`; source registered; `Scan now` found "2 archive(s)
   found, 0 skipped, Game folders: 1"; status wording read "Not scanned
   yet" before the scan and "1 last scan succeeded" after — matches the
   requested wording check. A "Recent activity" timeline recorded each
   step with exact timestamps and the exact disposable path.
   `Continue`.
3. **Optional DAT** — "No DATs added... Nothing is downloaded and nothing
   is changed." Managed MAME/Redump DAT rows all showed "Not installed" /
   "Not configured" with explicit "Check"/"Update" buttons that were **not**
   clicked (avoiding any real network call in this QA). Used `Skip for
   now`, which correctly advanced `onboarding_state.txt` from
   `in_progress:dat_setup` to `in_progress:emulator_setup`.
4. **Emulator setup** — surfaced RetroArch ("Needs setup" — "cannot find
   usable game-support files") alongside distinct standalone candidates
   (Mesen 2, mGBA, Snes9x, Hatari, all "Not checked" — no auto-probe without
   explicit action) for the platforms this artifact ships. RPCS3,
   DuckStation and xemu all read "Not installed by EmuWiz. Download the
   official `<name>` AppImage from `<exact upstream GitHub URL>`." —
   confirms the reviewed, per-adapter AppImage policy (see below).
   `Continue`.
5. **Verify & finish** — "No DAT catalogue was added, so there is nothing
   to verify yet - that's fine." `Finish` advanced
   `onboarding_state.txt` to `completed`.

Back/Skip/Continue all behaved correctly; no dead-end was found; state
persisted to disk after every single step (verified by reading
`onboarding_state.txt` directly after each click, not only via the GUI).

## Sources/Discovery QA

- Empty state before adding a source: "0 configured source folders."
- `Add game folder` opened a small in-app dialog (path text field + Browse
  button), not immediately a native file picker — typing the full
  `/tmp/...` path directly and clicking `Add` worked without needing the
  native picker at all.
- The added source's path is displayed in full (`/tmp/emuwiz-fresh-appimage-qa/test-games`)
  with no truncation that would make it unreadable, and a hover tooltip
  echoes the exact full path.
- `Scan now` completed instantly against the two tiny fixtures with a
  clear, honest result summary.
- Read-only principle: `find ... -exec stat` before and after the scan
  produced byte-identical output (same file sizes, same mtimes) — no
  rename, move, delete, or in-place rewrite, and no index/catalogue file
  was written inside the source folder itself.

## Arbitrary path access

- The in-app "Add Folder" dialog accepted the `/tmp` path directly with no
  restriction.
- The "Mount destination" → `Choose folder` control opens the host's
  **native** GTK file chooser (via the `rfd` crate), which listed real
  `/mnt/*`, `/home`, and `/tmp` locations without any evidence of
  sandboxing or masking — confirming the packaged AppImage does not
  restrict filesystem visibility. (I did not confirm a selection in this
  native dialog: synthetic `xdotool` clicks on its "Select" button were
  unreliable — a host GTK-dialog automation limitation, not an EmuWiz
  defect — and mid-attempt a stray keyboard-navigation selection highlighted
  a real `/mnt/usbdrive/games` row; I cancelled immediately rather than
  risk confirming a real path. "Mount root: unknown" was unchanged
  afterward — cancel worked correctly and nothing was written.)

## /mnt result

Real `/mnt/*` entries (e.g. `/mnt/usbdrive`, `/mnt/games`,
`/mnt/nvme2/remote-decypharr-mnt`) were visible and selectable in the
native folder picker, proving no `/mnt` masking. A dedicated
`/tmp/emuwiz-fresh-appimage-qa/test-mnt` disposable directory was created
for this purpose and used as the typed-path target; no real `/mnt` path was
ever confirmed/selected (see above). Root was not required or used to
create a new `/mnt` entry.

## Host PATH / tool-discovery result

Host has `ratarmount`, `fusermount3`, `7z`, and `retroarch` all on `PATH`.
`packaging/appimage/AppRun` is a 6-line script that execs
`$APPDIR/usr/bin/emuwiz` directly with **no** `HOME`, `XDG_*`, or `PATH`
reassignment — confirmed by direct inspection, not inference. The
RetroArch candidate's "Technical details" in Emulator Setup showed
"Reviewed core candidate: mgba", proving RetroArch's own installed core
metadata was read successfully from the host installation — host tools are
not masked by the AppImage runtime.

## Emulator-discovery smoke result

RetroArch, Mesen 2, mGBA, Snes9x, and Hatari all appeared as genuinely
distinct candidates for their respective platforms (this build predates
VICE/Stella — see Limitation above). RetroArch correctly read "Needs
setup" (no profile/core folder configured under the disposable HOME) with
technical details naming the exact reviewed core it found. No adapter was
fabricated as "available" when it was not configured.

## AppImage emulator policy result

Confirmed directly from the GUI: RPCS3, DuckStation, and xemu (all
adapters that are officially distributed upstream as AppImages) each read
"Not installed by EmuWiz. Download the official `<Name>` AppImage from
`<exact GitHub URL>`." — i.e. EmuWiz points the user at the specific,
reviewed upstream release location per adapter; it does not grant a
generic "any `*.AppImage` is executable" capability. This matches the
already-audited adapter policies for RMG, Mesen, Snes9x, Stella (native/
explicit executable discovery only, no AppImage seam) and VICE (same),
none of which were touched by this QA pass.

## Restart/persistence result — BLOCKER found

- **First session**, after clicking `Finish` on step 5, the Gamer/Home view
  showed a spinner and "Loading your games..." **indefinitely** — confirmed
  stuck for over 10 minutes of real wall-clock time with the process at
  0.7-1.1% CPU (i.e., not busy-computing) and zero new log lines.
  `/proc/<pid>/task/*/wchan` showed both existing threads parked in
  `do_poll` (the normal idle GUI event-loop wait) — there was **no third
  worker thread still running** `load_data()`, meaning the background load
  had already finished (or its result was dropped) without the UI ever
  transitioning `LoadState::Loading` → `LoadState::Ready`. This points at
  a `refresh_generation`-matching bug in `crates/archivefs-gui/src/main.rs`'s
  `poll_load`/`start_load` state machine, most likely specific to the
  onboarding-finish → first-load transition, not at a slow computation.
- **Second session** (fresh process, identical disposable HOME/XDG,
  `SIGTERM` on the first, clean relaunch): the Home/Library view loaded
  **instantly** and correctly, listing both fixtures ("empty-fixture",
  "qa-fixture", both "NES · Game found"). This rules out the tiny/empty
  fixture files as the cause (the exact same on-disk data loaded fine on
  restart) and narrows the defect specifically to the in-process
  onboarding-completion transition.
- `onboarding_state.txt` correctly read `completed` on the second launch;
  no repeat onboarding was shown.
- `APPIMAGE_EXTRACT_AND_RUN=1` against the same disposable state (third
  session) also loaded instantly and correctly, with an "Archive snapshot
  refreshed" activity entry.

**Classification: BLOCKER.** A genuinely fresh user who completes
onboarding in one sitting may see their own game library hang
indefinitely on first load, with no error, no crash, and no way to
proceed short of restarting the app. This is application/GUI logic (not a
packaging defect) and was **not fixed** here per this task's scope (only
small packaging/runtime defects were in scope for a direct fix); it is
reported precisely for a backend/GUI follow-up, including the exact
repro (finish onboarding → stuck; kill and relaunch → fine).

## Real-HOME leak check

`~/.config/emuwiz` and `~/.local/share/emuwiz` do not exist on this host at
all (this host's own real EmuWiz usage lives under the documented legacy
`~/.config/archivefs` / `~/.local/share/archivefs` names — a large, clearly
long-lived real install with real DAT/cheat/library data). Since every
disposable-HOME launch in this QA exported an explicit `HOME=` override,
and `archivefs-core::app_dirs::home()` reads `env::var_os("HOME")`
directly, none of my launches could have resolved to the real
`/home/davedap` directories for `config.toml`/`onboarding_state.txt`/
`library.sqlite3` (confirmed directly: every launch's own stderr logged
the disposable path being read).

One file under the **real** `~/.config/archivefs/gui_mode.txt` changed
during the QA window (content `advanced`, mtime matching the moment I
clicked "Advanced View" in my second disposable session). I could **not**
confirm this was caused by my sandboxed launches: this is a shared,
actively-used host, and `ps` showed a **separate, concurrent, unrelated
agent session** independently building and (plausibly) running
`archivefs-gui` from a different worktree (`/tmp/archivefs-firmware-bios-gui-p0`)
during the exact same window, under its own real `HOME`. Given every
launch I made showed correct disposable-path resolution in its own log
output at the time, and a clearly plausible unrelated concurrent actor
exists, I am **not** treating this as a confirmed AppImage leak — but it
is recorded here honestly as **INCONCLUSIVE**, should not be dismissed
without a clean single-tenant re-run, and I did not delete or alter the
real file to hide or "resolve" the ambiguity.

## Failure-case findings

- Missing/unconfigured RetroArch: "Needs setup" with a clear, specific
  reason ("cannot find usable game-support files in the folder it is
  currently using") and a remediation button (`Choose game-support
  folder`) — no panic, no crash.
- Emulators not installed by EmuWiz (RPCS3/DuckStation/xemu): explicit
  "Not installed by EmuWiz" plus the official upstream download URL — no
  panic, no silent failure.
- DAT sources not configured: explicit, calm "nothing to verify yet"
  messaging, never blocking progress.
- A source becoming unavailable, a read-only source directory, and a
  nonexistent-emulator-path case were not separately manufactured (per the
  instruction to avoid manufacturing destructive filesystem errors); the
  "Needs setup"/"Not installed" patterns observed above are the same code
  path family and give confidence this class of failure is handled, but
  this is not itself direct evidence for those specific three scenarios.

## Log / stderr review

Full logs saved during this QA (not committed; ephemeral, scratch
`/tmp/emuwiz-fresh-appimage-qa/launch*-std{out,err}.log`).

| Finding | Classification |
| - | - |
| `WARN: XRandR reported that the display's 0mm in size, which is certifiably insane` | BENIGN (X11/virtual-display quirk, unrelated to EmuWiz) |
| `INFO: Guessed window scale factor: 1` | BENIGN |
| `INFO: loading config from ...` / `INFO: loaded config: ...` / `INFO: starting archive scan ...` / `INFO: archive scan complete: ...` | BENIGN (expected, informative) |
| `XDG_CONFIG_HOME`/`XDG_DATA_HOME` not honored | BENIGN (documented, intentional legacy-compat design) |
| "Loading your games..." hang after onboarding completion (first session only) | **BLOCKER** |
| Real `~/.config/archivefs/gui_mode.txt` changed during QA window | **P1** (inconclusive attribution; needs a clean single-tenant re-run to confirm or rule out) |
| Stale artifact predates VICE/Stella/latest ES-DE work | P1 (should rebuild and re-run once a Cargo slot is free) |

No missing-library errors, no permission failures, no stale ArchiveFS
resource-path references, and no FUSE confusion were observed anywhere in
any captured log.

## Files created/changed by this QA

- `packaging/appimage/test-fresh-home.sh` (new) — automated harness.
- `docs/APPIMAGE_FRESH_INSTALL_QA.md` (new, this file).
- No other repository files were touched. `dist/EmuWiz-x86_64.AppImage`
  and its `.sha256` were only read, never modified.

## Automated QA harness result

`packaging/appimage/test-fresh-home.sh [artifact-path]`:

- Creates its own disposable `HOME`/`XDG_*` directories under a fresh
  `mktemp -d`, verifies the checksum, runs `--version` in both normal and
  `APPIMAGE_EXTRACT_AND_RUN=1` modes, asserts the two outputs match,
  asserts the disposable directories remain empty after a bare
  `--version` (no surprise writes), checks host-tool visibility
  (best-effort, missing tools are not a failure), asserts `AppRun` itself
  never assigns `HOME`/`XDG_*`/`PATH`, and removes its own temp directory
  on exit (including on error, via `trap ... EXIT INT TERM`).
- Ran successfully end-to-end against the current artifact; left no
  temporary files behind afterward (`ls /tmp | grep fresh-home-qa` empty).
- Deliberately does **not** attempt full GUI automation (onboarding
  click-through, native file-picker interaction) — those were exercised
  manually in this QA pass instead, per the instruction to avoid brittle
  automation in the committed script.

## Reproduction commands

```sh
# Checksum + version smoke (both modes), fully disposable:
packaging/appimage/test-fresh-home.sh dist/EmuWiz-x86_64.AppImage

# Manual fresh-user session (what this QA pass drove interactively):
mkdir -p /tmp/emuwiz-fresh-appimage-qa/{home,xdg-config,xdg-data,xdg-cache}
env -i HOME=/tmp/emuwiz-fresh-appimage-qa/home \
  XDG_CONFIG_HOME=/tmp/emuwiz-fresh-appimage-qa/xdg-config \
  XDG_DATA_HOME=/tmp/emuwiz-fresh-appimage-qa/xdg-data \
  XDG_CACHE_HOME=/tmp/emuwiz-fresh-appimage-qa/xdg-cache \
  DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/1000 PATH=/usr/bin:/bin \
  ./dist/EmuWiz-x86_64.AppImage

# Reproduce the onboarding-completion hang:
#   1. Run the fresh session above.
#   2. Add a source folder, Skip DAT, Skip/Continue emulator setup, Finish.
#   3. Observe "Loading your games..." never resolves.
#   4. Kill the process and relaunch identically -> loads instantly.
```

## Definition of Done

This QA pass is complete: the artifact was verified, exercised end-to-end
across first launch, onboarding, sources/discovery, arbitrary-path access,
host-tool discovery, emulator-discovery, AppImage policy, restart/
persistence (both normal and extract-and-run), the read-only principle,
and failure-case wording, using only disposable state and synthetic
fixtures; one BLOCKER and one inconclusive P1 were found and documented
without being "fixed away"; a reusable, self-cleaning QA harness script was
added; and no real user state, unrelated adapter work, ES-DE work, or LBC
was modified.

## Current-artifact rebuild attempt — 2026-09-06

At `6f5b95657871a4ca483d2d2f3057d39fcd0f91c5`, the current source still has
the documented same-session onboarding completion → Home “Loading your games…”
P0: no corrective commit or regression test was present. This blocks release
readiness independently of packaging.

`cargo check -p archivefs-gui` and the current release `emuwiz` build passed.
The current AppImage was **not** rebuilt because this host has neither an
approved `appimagetool` executable nor the required pinned type-2 runtime.
The build script correctly fails closed for those explicit host dependencies;
the historical artifact was not reused. Therefore normal/extract AppImage
smokes and the fresh-home harness were not run against a falsely-current
artifact. Status: **RELEASE READINESS BLOCKED BY KNOWN HOME P0** (and the
missing approved packaging-tool inputs).
