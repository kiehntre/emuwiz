# Native, AppImage and Flatpak installs as launch targets

EmuWiz used to detect installs it could not launch: the PCSX2, DuckStation,
PPSSPP and melonDS launch bindings accepted native executables only, so
Doctor could say "installed" while the launcher said "unsupported" (or a
package kind simply was not looked for). This change makes the three package
kinds first-class for those four adapters.

## Model (nothing new beside two small modules)

- `launch/installation.rs`: `LaunchInstallation` = `Native | AppImage
  {extract_and_run} | Flatpak {app_id}` and `VisibilityPlan`. Emulator adapters
  still build only emulator arguments; `wrap_arguments` turns them into
  `<args>` (Native, unchanged), `<args>` run by the AppImage itself, or
  `run [--filesystem=<dir>:ro ...] <app-id> <args>` for Flatpak. No shell,
  no persistent override, no `host` grant; grants are validated (absolute,
  normalised, no `:`, never a broad root or `$HOME`). Content is read-only;
  write access has to be requested with a reason.
- `launch/installation_known.rs`: bounded discovery from explicit
  definitions (known AppImage name prefixes in `~/Applications`,
  `~/Applications/emulators` and a same-named subfolder; exact Flatpak ids in
  Flatpak's metadata directories). Nothing is executed or crawled. A script
  that only runs `flatpak run <known app>` (for example `~/.local/bin/PPSSPPSDL`)
  is not a native emulator; the real Flatpak is bound instead.
- Existing per-emulator bindings (`resolve_*_native_launch_binding`) gained
  an `installation` field and now bind: a portable AppImage to the profile in
  its own directory (portable marker recorded, never touched), a plain
  AppImage to the default profile, a Flatpak to its `~/.var/app/<id>` profile.
  Requests carry `expected_installation`, so drift between kinds is refused.
- `launch/installation_support.rs` runs those same bindings to answer
  "launchable?" for Doctor. `EmulatorLifecycleInstallation.launch_support` is
  `Launchable{kind}`, `NotLaunchableYet{reason}` or `NotAssessed`; AppImages in
  known locations now appear in the lifecycle list (version not probed, so
  "unknown", never guessed).

## Real machine (saltbox26, 2026-10-04)

No game was started and no emulator setting written. Config/data trees of all
four emulators, the Flatpak override directory and the AppImage folders were
fingerprinted (path, size, mtime) before and after: identical.

| Emulator | Kind | Detected | Binding | Real probe | Launchable |
|---|---|---|---|---|---|
| DuckStation | AppImage, portable (`~/Applications/DuckStation`, `portable.txt`) | yes | AppImage, data root beside it | watched process, `-version`: `0.1-11894-gc66b2694d` (exit 1, upstream behaviour) | yes |
| DuckStation | AppImage (`~/Applications/emulators`) | yes | AppImage, default `~/.local/share/duckstation` | same | yes |
| PCSX2 | AppImage v2.9.9, portable (`portable.ini`) | yes | AppImage, data root beside it | `-version`: `PCSX2 v2.9.9` (exit 1) | yes |
| PCSX2 | Flatpak v2.8.2 | yes | `flatpak run net.pcsx2.PCSX2`, sandbox profile | starts in the sandbox (Qt portal warning, exit 1); lifecycle below | yes |
| PCSX2 | native | no install | none: "no discovered executable ..." | - | n/a |
| PPSSPP | Flatpak 1.20.4 (+ wrapper `PPSSPPSDL`) | yes | `flatpak run org.ppsspp.PPSSPP`; wrapper not reported as native | `--version`: `v1.20.4`, exit 0 | yes |
| PPSSPP | AppImage (`~/Applications/PPSSPP`) | yes | none: no `~/.config/ppsspp` yet | - | **not yet**: "start the emulator once so it creates a settings folder" |
| melonDS | AppImage 1.1 | yes | AppImage, default `~/.config/melonDS` | `--help`: `melonDS 1.1`, exit 0 | yes |
| RetroArch | Flatpak 1.22.2 + wrapper | yes | existing RetroArch Flatpak logic, unchanged | its tests pass | unchanged |

Each probe is the exact wrapped argv run through `spawn_watched_process`
(pid and exit status captured) and again to capture text; Qt AppImages and the
Qt Flatpak ran under `xvfb-run` because even `--version` needs a display.

### Flatpak transient visibility (Flatpak 1.14.6)

Fixture outside the library (`/tmp/emuwiz-fp-probe/sub/fixture file.txt`):

- `flatpak run --nofilesystem=host --command=ls org.ppsspp.PPSSPP <dir>`:
  `No such file or directory`.
- same with `--filesystem=<dir>:ro --command=cat ...`: file content read.
- write attempt inside the `:ro` grant: `Read-only file system`.
- a later run without the option: not visible again; the Flatpak override
  directory is byte-identical (no persistent override).

(Every installed emulator Flatpak here already has `filesystems=host` in its
own manifest, so the grants are redundant on this host; EmuWiz itself never
asks for `host`.)

### Lifecycle

`flatpak run --command=sh <id> -c "sleep 3; exit 7"` through the watched
process: still running after 1.5 s and exit code 7 after 3.45 s, so the
`flatpak run` wrapper lives as long as the app and hands back its status.

## Not covered

Cheat runtimes (PCSX2/DuckStation/PPSSPP stay as documented), Dolphin, RPCS3
and the other emulators' adapters, and a cheat launch for a Flatpak melonDS
(refused: the child environment does not reach the sandbox).
