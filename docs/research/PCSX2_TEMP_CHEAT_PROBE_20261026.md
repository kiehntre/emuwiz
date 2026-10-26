# PCSX2 temporary-cheat integration: refused pending preservation proof

Base: `origin/main` at `bd3b859fecb10c6232286a9a881fbed1f0f8f2e5`.
This job does **not** deliver production temporary-cheat integration.
The disposable runtime can consume and enable a selected PNACH. Safe
production staging, cleanup/recovery and Blue handoff remain unproven, so
activation is withheld under the job's explicit refusal condition.

## Actual Blue call path

`gui_v2/native_workflows.rs::readiness_projection` builds the launch input
through `emulator_setup/controller.rs::build_launch_readiness_input`.
`launch_readiness_page.rs::gamer_play_action` selects a typed request;
`typed_launch_request` calls `pcsx2_launch_request`, producing
`TypedLaunchRequest::Pcsx2`. Its `start` dispatch calls
`Pcsx2LaunchState::start`, then Core's `preflight_and_launch_pcsx2`.
The explicit Launch page also reaches that state through
`show_pcsx2_launch_action`.

Neither path calls `plan_pcsx2_temporary_cheat_launch`, carries selected PNACH
material, nor stages or enables cheats. `Pcsx2LaunchRequest` binds content,
serial, profile, executable, directory mode and installation, but contains no
cheat selection. Ordinary launch may use cheats already enabled by the user;
it is not evidence that EmuWiz activated a temporary selection.

Blue's separate Cheats & Mods path (`NativeWorkflows::show_cheats`) uses the
existing `LocalPcsx2InstallContext` and guarded install preview/apply/Undo
pipeline in `user_cheat_import_page.rs`. It installs a file only after its
own approval; it does not hand temporary material to Play. CodeBreaker
direct-write conversion remains the existing preview, not a second converter.

## Disposable runtime observations

Installed Flatpak `net.pcsx2.PCSX2`, v2.8.2, deployment
`f8162e3d6b0ce75f54d0e10605d368b3678fda969bb1e806277500156c85fa10`.
The AppImage beside `portable.ini` is refused for temporary redirection; no
production AppImage launch was attempted. Native capability was not tested.

`scripts/probe_pcsx2_temp_cheats.py` uses a fresh private `/tmp` tree, child
HOME/XDG overrides, a generated original MIPS branch-loop ELF, a copied
local QA BIOS and a single named PNACH write to otherwise-unused memory.
Memory cards and shutdown save-state writing are disabled. It reads no
production INI, saves or cheat files. No BIOS or game bytes are in this PR.

The display-assisted run reported:

| Observation | Result |
| --- | --- |
| `-testconfig` | exited 0 |
| `-datapath <probe>/stage` | data root `<probe>/stage/PCSX2` |
| Executable CRC | `76E7FB84`, matched PCSX2's log |
| PNACH | `Found 1 cheats in <probe>/stage/PCSX2/cheats/76E7FB84.pnach.` |
| Selection | `Enabled patch: EMUWIZ Probe` |
| Emulator overlay (separate private Xvfb run) | `1 cheat patch is active.` |
| In-game effect | **NOT CONFIRMED**; branch loop has no gameplay |
| Normal exit / crash / interrupted launch recovery | **NOT VALIDATED** |
| Production support | **UNSUPPORTED_PENDING_PRESERVATION_PROOF** |

The probe is stopped after a bounded observation period. A stopped synthetic
run is not a normal-exit test. Evidence directories are deliberately retained;
this probe does not claim canonical-runtime cleanup or recovery coverage.
The network-isolated offscreen run reaches BIOS/ELF inspection but fails to
create a render window (`Unknown PNI platform 'offscreen'`); it correctly
reports no PNACH consumption or named activation.

Reproduce with an authorized local BIOS (exit 2 deliberately means production
support has not been established):

```sh
python3 scripts/test_probe_pcsx2_temp_cheats.py
xvfb-run -a python3 scripts/probe_pcsx2_temp_cheats.py \
  --bios /path/to/local/authorized-bios.bin \
  --approve-disposable-activation --use-display
```

Omit `--use-display` for the network-isolated offscreen check. A display run
uses the caller's X11 transport; use a disposable X server as above.
The private directory, raw logs and JSON report are printed. The report
always withholds production support, including when loading and enablement
are observed. It is evidence for a human review, never a proof token accepted
by Core or the GUI.

## Preservation defect found and corrected

The reviewed planner omitted six configurable folders from its routing
classification: `Patches`, `Covers`, `GameSettings`, `UserResources`,
`DebuggerLayouts`, `DebuggerSettings`. It could accept a relative `Patches`
entry, then plan a redirected launch which would see different patches.
The correction refuses relative/parent paths for these keys and records
missing defaults. Game settings retain the existing copy requirement;
they must not be pinned writable to production. Debugger defaults are
relative to the settings root (`inis`), not the data root.

These details match the installed behavior and versioned upstream
[folder routing](https://github.com/PCSX2/pcsx2/blob/v2.8.2/pcsx2/Pcsx2Config.cpp#L2165)
and [data-root resolution](https://github.com/PCSX2/pcsx2/blob/v2.8.2/pcsx2/Pcsx2Config.cpp#L2068).
The `-datapath` argument must not be confused with the resulting data root.

## Why no production staging executor or Blue activation action is added

The existing planner intentionally has no supported verdict. The experiment
does not establish all its required proof items: real user-folder visibility,
memory-card/save preservation, production-root before/after integrity, or
preservation of ordinary per-game patch behavior. A homebrew test without
memory cards cannot prove those facts. Passing read-only `-testconfig` is
also insufficient to authorize cheat activation.

Consequently no proof flag is introduced, no existing filesystem/transaction
guard is relaxed, and the experiment is not promoted to a launch executor.
The missing production handoff is an explicit refusal, not planning success
or completed integration. The existing canonical per-launch runtime remains
the required staging/recovery path if support can later be established.

Focused gates: Core `pcsx2_temp_cheat_readiness`, existing GUI
`launch_readiness_page::tests` PCSX2 filters, probe safety tests, formatting
and repository scope checks. Exact results belong to the candidate PR.
