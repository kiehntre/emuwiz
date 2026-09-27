# EmuWiz release portability and host compatibility test matrix

This is a QA specification, not a support claim. It defines the evidence needed
before a release can claim compatibility with a host class. No host in this
document is marked PASS unless a release artifact was actually exercised on that
host with the required evidence.

Audit basis: `33ecab378c54cfdfa4dd0303e1008b114b920878`.

## SUPPORTED-CLAIM PRINCIPLE

EmuWiz may claim support only for the intersection of:

1. a declared OS/distribution, desktop/session, GPU/driver, and install lane;
2. a specific release artifact whose SHA-256 and layout were verified;
3. a Tier 0 pass on the canonical release gate; and
4. a Tier 1 pass for every host dimension being claimed.

Source inspection, a successful build, an Xvfb run, a historical packaging note,
or a developer-machine launch is not equivalent to a host support result.

The current tree contains an in-progress release-contract reconciliation:
`scripts/build-release.sh` still assembles the historical
`archivefs-v<version>-...tar.gz` shape, while newer verifier/test helpers refer
to a `.tar.xz` contract. This matrix treats the release artifact contract as
**intended/in-progress**, not proven, until one clean artifact passes the same
contract end to end.

Status vocabulary:

- **Proven** — directly exercised with retained evidence for the current release
  and exact host class.
- **Historical** — documented or tested previously, but not evidence for this
  release/host claim.
- **Intended** — represented by source/configuration or an explicit test seam,
  but not executed on the target host.
- **Untested** — no adequate evidence found.
- **Blocked** — the test cannot be meaningfully claimed because the artifact or
  required host fixture is unavailable.

## MATRIX DIMENSIONS

Every matrix row must name all dimensions, even when some are “not applicable”.

| Dimension | Required values |
| --- | --- |
| OS/distribution | Ubuntu 22.04 LTS; Ubuntu 24.04 LTS; Fedora current stable; Nobara current stable |
| Desktop/session | KDE Plasma Wayland; KDE Plasma X11 where available; GNOME Wayland; GNOME X11 where practical |
| GPU/driver | NVIDIA proprietary; AMD Mesa; Intel Mesa |
| Install mode | Direct release archive; user install via `install.sh`; AppImage where supported; DEB/RPM only in active package lanes |
| Network | Normal; offline/no network access |
| State | Pristine; existing current EmuWiz state; legacy ArchiveFS roots; upgrade from previous supported release |
| Input/display extras | No controller; controller present; normal desktop; optional Sunshine/Moonlight stream |

### Compact execution matrix

Rows are required test targets, not pass results. All status entries are
currently **Untested** or **Historical** unless explicitly stated.

| Host | Session | GPU | Install mode | Tier | Required tests | Status evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Ubuntu 22.04 | KDE Wayland | NVIDIA | archive + user install | T0/T1 | artifact, first frame, graphics, paths, scan, discovery, dialogs | Untested |
| Ubuntu 22.04 | GNOME Wayland | AMD Mesa | archive + user install | T0/T1 | same; Mesa/Wayland | Untested |
| Ubuntu 22.04 | GNOME X11 | Intel Mesa | archive + user install | T0/T1 | same; X11 | Untested |
| Ubuntu 24.04 | KDE Wayland | NVIDIA | archive + AppImage | T0/T1 | AppImage fallback, graphics, dialogs, fullscreen | Historical package dependency notes only |
| Ubuntu 24.04 | GNOME Wayland | AMD Mesa | archive + AppImage | T0/T1 | same; offline | Historical package dependency notes only |
| Ubuntu 24.04 | GNOME X11 | Intel Mesa | archive + user install | T0/T1 | same; X11 | Untested current release |
| Fedora stable | KDE Wayland | NVIDIA | archive + RPM/AppImage | T0/T1 | graphics, native/Flatpak discovery | Untested current release |
| Fedora stable | GNOME Wayland | AMD Mesa | archive + RPM | T0/T1 | same; offline | Untested current release |
| Fedora stable | GNOME X11 | Intel Mesa | archive + RPM | T0/T1 | same; X11 | Untested |
| Nobara stable | KDE Wayland | NVIDIA | archive + AppImage | T0/T1 | Gamescope present/absent, streaming, controller | Manual QA procedure exists; no current PASS evidence |
| Nobara stable | KDE Wayland | AMD Mesa | archive + AppImage | T0/T1 | same; Mesa | Manual QA procedure exists; no current PASS evidence |
| Nobara stable | GNOME Wayland | Intel Mesa | archive + AppImage | T0/T1 | same; Wayland | Untested |
| Any declared host | declared session | declared GPU | offline archive | T0 | local startup and source workflow | Must be run per release |
| Any declared host | declared session | declared GPU | AppImage | T0/T1 | AppImage extraction/FUSE fallback and GUI | Must be run per release |
| Any declared host | declared session | declared GPU | any | T2 | Sunshine/Moonlight | Optional/manual |

No row may be marked PASS by copying a result from another row. A result from
Ubuntu 24.04 GNOME Wayland does not cover Ubuntu 22.04 KDE Wayland or Nobara
KDE Wayland.

## TIER 0 TESTS

Tier 0 is release-blocking on every canonical test host and every canonical
install lane that is published.

### Artifact and process gate

1. Verify the release artifact filename, sidecar SHA-256, archive format, one
   safe top-level directory, path traversal refusal, ownership/modes, and exact
   expected payload.
2. Extract into a disposable directory and confirm all expected executables are
   present and executable.
3. Run GUI `--version`; confirm it reports the release version and native GUI-v2
   identity without requiring a display when that is the documented behavior.
4. Run CLI `--version` and `--help`.
5. Start the GUI under the host's declared session and prove a first frame for
   at least the bounded smoke interval.
6. Close the GUI and confirm clean exit with no fatal log markers.

The current source has relevant automation seams in
`scripts/verify-release-artifact.sh`, `scripts/test-release-artifact-verifier.sh`,
`scripts/qa/release-smoke.sh`, and
`scripts/release/packaged_gui_smoke.py`. The packaged GUI helper exercises
empty, existing-profile, legacy-only, and both-root fixtures under Xvfb, but an
Xvfb pass is only a process/first-frame smoke test, not a Wayland/GPU claim.

### Fresh local workflow gate

7. With no config or database, confirm startup does not crash.
8. Confirm effective config and data paths resolve inside disposable XDG roots.
9. Confirm no config/database is created merely by version reporting.
10. Confirm first-run guidance explains the next action.
11. Add a synthetic/legal source folder.
12. Start a scan and confirm it reaches a terminal result.
13. Open the emulator discovery/setup page.
14. Confirm no installed emulator is non-fatal and produces actionable wording.
15. Confirm absent optional archive tools are non-fatal to core startup.
16. Open Game Details/library pages with an empty or synthetic catalogue.
17. Exit cleanly.

The CLI smoke fixture is intentionally local and synthetic; it must not require
copyrighted games, BIOS, emulator binaries, or network access.

### Offline gate

18. Repeat version, first frame, config/data resolution, source add, scan start,
    library view, Game Details, emulator discovery, and exit with network access
    disabled.
19. Confirm provider/DAT/artwork refresh failures are isolated and labelled as
    optional or unavailable, not as a broken EmuWiz installation.

## TIER 1 TESTS

Tier 1 is required before claiming support for a specific host/session/GPU or
install lane.

- X11 and Wayland window creation on the declared session.
- OpenGL/EGL initialization and stable first-frame rendering.
- File picker opens, navigates a disposable fixture, cancels, and returns to
  EmuWiz without blocking the UI.
- Desktop integration: `.desktop` entry, icon lookup, and `xdg-open` for a
  harmless local text/PDF fixture where that workflow is claimed.
- DPI/scaling at 100%, 125%/150%, and a high-DPI setting appropriate to the
  host; text remains readable and controls remain reachable.
- Fullscreen transition and return to windowed mode.
- AppImage normal execution and `APPIMAGE_EXTRACT_AND_RUN=1` fallback where the
  AppImage lane is published.
- Native emulator discovery with executable on PATH and with explicit path.
- Flatpak emulator discovery only where the host/package lane claims it.
- Missing emulator, missing `ratarmount`, missing FUSE, and missing 7z/RAR are
  reported as feature-specific degraded states.
- Controller enumeration and basic navigation when a real controller is
  present; absence of a controller must not block mouse/keyboard use.
- One known-safe emulator launch fixture or user-owned legal test target where
  the host claim includes launch support. Never require copyrighted media or
  proprietary firmware for the base release gate.

## TIER 2 TESTS

Tier 2 is informational/best-effort and cannot expand a public support claim by
itself:

- additional GPU driver versions;
- multiple monitor arrangements, HDR, VRR, fractional scaling, suspend/resume;
- Gamescope, Steam Game Mode, remote desktop, unusual compositors;
- Sunshine/Moonlight streaming;
- uncommon controllers, Steam Input, hotplug races;
- distro point releases outside the declared matrix;
- performance and long-running scan stress.

Failures must still be recorded, but classified rather than silently ignored.

## UBUNTU

Ubuntu 22.04 and 24.04 are separate host classes. Test both rather than
assuming the newer release is representative. Required permutations should
cover at least one KDE/Wayland, GNOME/Wayland, and X11 session where available,
plus NVIDIA, AMD Mesa, and Intel Mesa across the total matrix.

Existing packaging documentation contains historical Ubuntu 24.04 dependency
availability observations for FUSE, 7z, unrar, xdg-utils, and ratarmount. Those
observations are useful dependency evidence, but they are not a current GUI
first-frame or GPU support result.

## FEDORA

Test current stable Fedora separately from Nobara. Verify native package
dependencies, Wayland/X11 behavior, KDE and GNOME where available, and Flatpak
metadata visibility. Fedora's package names and default security/graphics
configuration must be recorded rather than inferred from Ubuntu.

Existing RPM packaging files and historical Fedora 41 notes are intended/package
evidence only until the current release artifact passes the full host matrix.

## NOBARA

Nobara is a first-class target because it is a common emulation desktop, not an
automatic Fedora substitute. Required checks:

- KDE/Wayland first frame and clean exit;
- NVIDIA variant where available, plus AMD Mesa where available;
- native emulator discovery;
- AppImage execution and fallback;
- Flatpak emulator discovery;
- Gamescope installed and absent;
- Sunshine/Moonlight optional streaming row;
- controller enumeration and GUI navigation.

`scripts/test-on-nobara.sh` is a remote/manual QA procedure. It builds and
copies a test bundle, runs CLI checks, backs up an existing database, installs,
and explicitly leaves the GUI test manual. This is **historical/procedural
evidence**, not a current PASS for the release or every Nobara variant.

## KDE

KDE Plasma Wayland and KDE Plasma X11 are separate rows. Test window creation,
file dialogs, clipboard/path selection, icon/desktop launch, fullscreen, DPI,
and return from an external viewer. Record compositor, Plasma version, display
scale, GPU, and whether Gamescope is active. A KDE result cannot be generalized
to GNOME.

## GNOME

GNOME Wayland is the primary modern Linux session row. GNOME X11 is tested where
the distro still provides it. Include file picker behavior, window close,
fullscreen, scaling, xdg-open, and emulator discovery. Record GNOME Shell and
Mutter versions. Do not infer GNOME behavior from Xvfb.

## WAYLAND

Wayland validation must run on a real compositor. Xvfb cannot establish
Wayland, EGL, clipboard, file-dialog, fullscreen, or GPU behavior. Record:

- `XDG_SESSION_TYPE=wayland`;
- compositor/desktop and version;
- `WAYLAND_DISPLAY` and relevant runtime environment;
- GPU and driver;
- scale factor and monitor topology;
- whether the run is native, AppImage, Flatpak, or package.

Check startup, file dialogs, external opening, fullscreen, focus return, and
clean exit. Do not require DRM/KMS, HDR, VRR, or Gamescope for ordinary desktop
startup.

## X11

Run on a real X11 desktop and retain `DISPLAY`, X server version, compositor,
GPU/driver, and scale. Xvfb is useful for automation but must be labelled as a
headless smoke environment. Real X11 Tier 1 still needs file dialogs,
fullscreen, icons, external opening, and emulator discovery.

## NVIDIA

NVIDIA support requires an actual proprietary-driver host test. Check:

- normal GUI startup and first frame;
- NVIDIA driver version and GPU model;
- Wayland startup;
- X11 startup where available;
- OpenGL/EGL capability reporting and rendering;
- fullscreen and focus return;
- Gamescope installed and absent do not affect ordinary GUI startup;
- no hidden HDR/VRR or DRM/KMS requirement.

Do not claim NVIDIA support from `eframe` feature flags, source inspection,
or an AMD/Intel result.

## AMD

On AMD Mesa, check real Wayland and X11 startup, OpenGL/EGL rendering,
fullscreen, file dialogs, scaling, and clean exit. Record Mesa and kernel
versions. Confirm no AMD-specific assumptions are needed for normal GUI use.

## INTEL

On Intel Mesa, run the same rendering/session checks, including integrated-GPU
resource constraints and scaling. A successful software or Xvfb run is not an
Intel Mesa result.

## APPIMAGE

If AppImage is published, Tier 0 requires:

- artifact and sidecar checksum verification;
- normal execution;
- `APPIMAGE_EXTRACT_AND_RUN=1` fallback;
- GUI `--version` in both modes;
- fresh isolated HOME/XDG roots;
- no writes outside the disposable roots;
- first-frame and clean exit on each claimed host class.

The AppImage builder pins appimagetool/runtime hashes in
`packaging/appimage/tooling.lock`. The AppImage intentionally does not bundle
graphics libraries, FUSE, ratarmount, archive tools, or emulators. Missing
AppImage FUSE must be a clear fallback/degraded result, not an unexplained
application failure.

## DIRECT TAR.XZ

The intended repaired direct archive lane must be tested as its own artifact:

- verify exact archive format/name/layout;
- extract without elevated privileges;
- run directly from the extracted root;
- run the bundled user installer if that is the declared contract;
- verify installed binaries, desktop entry, icons, and ownership safeguards;
- confirm no source-tree or developer-home path is required;
- run fresh and existing-state tests.

Until the concurrent release-contract repair settles the canonical format,
tests must record whether the observed artifact is the historical gzip bundle or
the intended tar.xz bundle. Do not merge results across them.

## OFFLINE

Offline means network access is actually disabled, not merely that no provider
button was clicked. Use an isolated network namespace or equivalent host policy
and record the method. The base gate must cover local source add, scan start,
catalogue/library view, Game Details, setup/diagnostics, and exit. Online
provider refreshes must fail with bounded, actionable messages while cached/local
data remains usable.

## UPGRADE

For every supported previous release, test:

| From | To | Required state |
| --- | --- | --- |
| Previous supported EmuWiz | Current release | Current XDG config/data, database, cache, journals, backups |
| Previous ArchiveFS-compatible release | Current release | Legacy `~/.config/archivefs` and `~/.local/share/archivefs` roots |
| Current EmuWiz | Current release | Existing config, database, caches, user-selected source roots |

Verify schema migration, config parsing, cache reuse/invalidation, backup
creation, no user-data deletion, and no path move without explicit policy.
Verify newer unsupported schema refusal is clear. Do not perform or promise
destructive downgrade support.

The read-only `scripts/qa/upgrade_preflight.py` and its self-test are useful
preflight automation; they do not replace a real writable upgrade test with
disposable copies.

## SUNSHINE/MOONLIGHT

This is a separate optional streaming row. It does not establish base host
support. Test only on declared streaming hosts:

```text
Desktop session → launch EmuWiz → Sunshine capture
→ Moonlight connection → mouse/controller reaches GUI
→ launch a synthetic/legal emulator fixture where permitted
→ return to EmuWiz → exit stream → desktop remains healthy
```

Record input method, display session, GPU/driver, compositor, resolution/scale,
fullscreen behavior, Sunshine/Moonlight versions, Gamescope state, emulator
fixture, and return-to-frontend behavior. Do not modify Sunshine configuration
automatically. A streaming failure is Tier 2 unless streaming is explicitly
included in the release claim.

## FAILURE CLASSIFICATION

| Class | Definition | Examples |
| --- | --- | --- |
| BLOCKER | Release cannot launch or complete a core declared workflow on a required host/lane. | Artifact cannot verify; GUI never starts; fresh local setup crashes; config/data path escapes or cannot be safely resolved. |
| DEGRADED | Core works, but an optional/feature-specific workflow fails with clear bounded guidance. | No ratarmount; no emulator installed; provider offline; AppImage normal FUSE path unavailable but extract-and-run works. |
| COSMETIC | Visual or polish defect without loss of core operation. | Minor scaling/layout issue that does not hide controls. |
| UNSUPPORTED ENVIRONMENT | Host/session/GPU/install mode is outside the declared matrix. | Unlisted distro, unusual compositor, unsupported architecture. |

Do not convert a DEGRADED optional-tool result into a release blocker. Do not
use UNSUPPORTED ENVIRONMENT to hide a failure on a host that was advertised.

## TEST EVIDENCE FORMAT

Each run must retain a machine-readable record containing:

```json
{
  "distro": "Ubuntu 24.04.1 LTS",
  "kernel": "...",
  "desktop": "GNOME 46",
  "session": "wayland",
  "gpu": "AMD ...",
  "driver": "Mesa ...",
  "artifact_sha256": "...",
  "emuwiz_version": "...",
  "emuwiz_commit": "...",
  "install_mode": "direct-tar-xz",
  "network": "offline",
  "state_fixture": "pristine",
  "tests": {"gui_first_frame": "PASS", "source_add": "PASS"},
  "classification": "PASS",
  "logs": ["stdout.log", "stderr.log", "environment.json"]
}
```

Also record artifact layout, command exit codes, timestamps, display/session
variables, package-manager versions, optional dependency presence, and a clear
failure classification. Retain screenshots/strace only when policy permits and
redact user paths or credentials.

## AUTOMATION CANDIDATES

Automatable now or later:

- artifact checksum, extraction, path traversal, mode, manifest, version, and
  payload checks;
- CLI version/help and isolated CLI smoke;
- fresh config/data path checks under disposable HOME/XDG roots;
- offline CLI smoke with controlled network isolation;
- packaged GUI first-frame smoke through Xvfb, including empty/current/legacy/
  both-root fixtures;
- release manifest/SBOM/checksum/signature validation;
- missing optional-tool classification;
- synthetic source add/scan and deterministic rescan;
- read-only upgrade preflight and SQLite integrity checks.

Automation must use the extracted release artifact, not a build-tree GUI,
whenever it is intended to validate packaging.

## MANUAL TESTS

Real host/manual testing is required for:

- Wayland window/input/focus behavior;
- NVIDIA proprietary graphics and EGL/OpenGL;
- KDE integration, GNOME integration, and real file pickers;
- DPI/fractional scaling and fullscreen;
- controller enumeration/hotplug/navigation;
- AppImage FUSE behavior on hosts with/without `/dev/fuse`;
- native emulator and Flatpak discovery on the declared host;
- Sunshine/Moonlight streaming and return-to-frontend behavior;
- real user-owned legal emulator fixture launch.

## RELEASE-GATE POLICY

1. Resolve the artifact-contract repair before assigning portability status.
2. Require Tier 0 on every canonical OS/session/GPU/install lane published.
3. Require Tier 1 before naming a host class in support documentation.
4. Require offline Tier 0 on every canonical lane.
5. Treat AppImage, direct archive, and distro packages as separate lanes.
6. Keep historical/procedural evidence visibly separate from current PASS.
7. Block release for any unclassified first-frame crash, core startup failure,
   unsafe path write, artifact verification failure, or data-loss upgrade result.
8. Publish a bounded support statement listing tested host classes and release
   artifact SHA-256; do not claim “Linux desktop” generically.
9. Re-run the required matrix for every release artifact/toolchain change that
   can affect packaging, graphics startup, paths, or migrations.

## MINIMUM CLAIMS FOR NEXT RELEASE

Until the matrix is executed, the minimum defensible claim is:

> EmuWiz is a Linux desktop application with intended X11 and Wayland support.
> The next release is not yet claiming Ubuntu, Fedora, Nobara, KDE, GNOME,
> NVIDIA, AMD, Intel, AppImage, or streaming compatibility beyond the exact
> host/artifact rows documented by retained Tier 0/Tier 1 evidence.

The next release may claim a specific host only after its row passes. A missing
optional archive tool, emulator, provider, Sunshine/Moonlight, or controller is
not a blocker for the base claim when the core startup and local workflow pass;
it must be reported as degraded with actionable wording.

