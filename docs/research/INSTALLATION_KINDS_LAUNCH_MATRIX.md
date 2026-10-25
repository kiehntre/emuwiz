# Native, AppImage and Flatpak installs as launch targets

EmuWiz used to detect installs it could not launch: the PCSX2, DuckStation,
PPSSPP and melonDS launch bindings accepted native executables only, so
Doctor could say "installed" while the launcher said "unsupported" (or a
package kind simply was not looked for). This change makes the three package
kinds first-class for those four adapters. Dolphin and RPCS3 now use the same
resolver and launch wrapper while retaining their existing content planners.

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
  an `installation` field. Existing adapters retain their marked-portable
  versus default AppImage rules. Dolphin uses its proven adjacent `User` root
  when `portable.txt` and `Config/Dolphin.ini` exist; RPCS3 uses its standard
  XDG configuration path. Flatpaks bind to their exact `~/.var/app/<id>`
  profile. Requests carry `expected_installation`, so drift between kinds is
  refused.
- Dolphin and RPCS3 command plans carry that same `LaunchInstallation` value.
  Their resolver rechecks the exact profile and executable before spawn; a
  Flatpak plan adds only a transient read-only content grant and exact app id.
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

### Dolphin and RPCS3 (saltbox26)

`flatpak list` confirms the exact installed IDs below. The PATH entries
`dolphin-emu` and `rpcs3` are Flatpak forwarding scripts, so neither is
counted as a native binary. Dolphin's desktop entry passes `-u` with the
AppImage's adjacent `User` root; EmuWiz binds it only when the root contains a
regular `Config/Dolphin.ini`. RPCS3 AppImages use only the established XDG
configuration path; no unverified per-AppImage portable layout is assumed.

| Emulator | Kind | Installed form | Resolver result | Safe probe | Readiness |
|---|---|---|---|---|---|
| Dolphin | Native | no native executable observed; PATH `dolphin-emu` is a Flatpak forwarding script | - | - | not installed |
| Dolphin | AppImage | `/home/davedap/Applications/Dolphin/Dolphin.AppImage`; existing `User/` portable profile | AppImage, `User/Config/Dolphin.ini` | watched `--version`, `Dolphin 2606a`, exit 0 | ready |
| Dolphin | Flatpak | `org.DolphinEmu.dolphin-emu` (user deployment, version `2606a`) | `/usr/bin/flatpak run org.DolphinEmu.dolphin-emu`; profile `.var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu` | watched `--version`, `Dolphin 2606a`, exit 0 | ready |
| RPCS3 | Native | PATH `rpcs3` is a Flatpak forwarding script; no native binary observed | - | - | not installed |
| RPCS3 | AppImage | none found in bounded known locations | - | - | not installed |
| RPCS3 | Flatpak | `net.rpcs3.RPCS3` (user + system deployments; selected version `0.0.42-19980-028d1e8f`) | `/usr/bin/flatpak run net.rpcs3.RPCS3`; profile `.var/app/net.rpcs3.RPCS3/config/rpcs3` | watched `--version`, `RPCS3 0.0.42-19980-028d1e8f Alpha`, exit 0 | ready |

Both Flatpak wrappers remained alive through their applications in the watched
process test and returned the harmless child process's exit code 23. The
selected RPCS3 Flatpak command uses the canonical app ID; Flatpak reports both
user and system deployments on this host.

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

Cheat runtimes (PCSX2/DuckStation/PPSSPP stay as documented), the other
emulators' adapters, and a cheat launch for a Flatpak melonDS (refused: the
child environment does not reach the sandbox).
