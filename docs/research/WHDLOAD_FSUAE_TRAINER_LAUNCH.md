# WHDLoad trainer launch: resolved choices, runtime disabled

Finish of preserved candidate `dc205e9233ac854c12fc39012033767a48f257ed`.
Its original four commits and worktree are preserved; integration reuses the
resolver, installed-directory planner, identity binding and focused tests.

## Runtime evidence (2026-10-17)

Installed binary: `/usr/bin/fs-uae`, Debian package `3.1.66-2build2`.
`fs-uae-launcher` and `amiberry` are not present on PATH; the installed desktop
entry is FS-UAE only. The installed binary's strings include the WHDLoad quit
key but not `whdload_args`. This corroborates the original disposable binary
probe, which observed no consumption of `--x-whdload-args`.

The upstream [WHDLoad support documentation](https://fs-uae.net/docs/whdload-support/)
explicitly assigns automatic WHDLoad setup to **FS-UAE Launcher/Arcade**, not
plain FS-UAE. The [Launcher documentation](https://github.com/FrodeSolheim/fs-uae-launcher/blob/main/docs/whdload-support.md)
describes preparing a boot environment. Merely mounting a host folder or
accepting an `x_*` command-line argument does not execute WHDLoad. Installing or
integrating Launcher, acquiring boot dependencies, rebuilding boot folders and
new emulator adapters are outside this bounded finish. No game/ROM or production
configuration was inspected or modified. No in-game trainer execution is claimed.

RetroArch and PUAE/PUAE2021 cores are also installed (core filenames and strings
only inspected). The upstream [PUAE README](https://github.com/libretro/libretro-uae#whdload)
documents guest-side `custom` / `custom_$SLAVE` files in the game/HDF or WHDSaves,
not a direct CUSTOM command-line option. EmuWiz's existing RetroArch launch seam
does not bind and provision those guest files for this exact installed slave.
Adding save/boot provisioning and core-version validation would be a separate
adapter, not finishing the preserved FS-UAE planner. No such route is enabled
or claimed here; no installed core was run against real content. Existing
manually configured trainers and emulator settings are left untouched.

## Finished behaviour

- The existing GUI editor still previews and saves choices in EmuWiz's per-game
  option file using the existing storage and Undo contract. Save is separate
  from **Enable trainers at launch**, which is disabled.
- The editor shows inspected saved CUSTOM values, runtime support and that no
  CUSTOM trainer values are passed. The launch panel reports support for the
  actual request's emulator (FS-UAE or Amiberry), without changing normal launch.
- `resolve_saved_whdload_launch_trainer` reads only through the existing storage
  callback, binds exact verified identity, selected host slave path/hash and
  option-file hash, and returns `SavedOptions`, not an assertion of runtime Apply.
- `plan_fsuae_whdload_launch_with_saved_trainers` refuses nonempty saved choices
  with an explicit unsupported-runtime blocker. The low-level trainer planner
  independently refuses them. Canonical preflight also rejects caller-created
  trainer commands, even if their hashes and projected argv are valid.
- Missing or zero CUSTOM values preserve the ordinary plan. LHA launches and
  ordinary Amiberry requests retain their existing behaviour. Installed-folder
  plans mount the selected game read-only; no newly enabled GUI folder launch
  route or writeable save location is introduced.

## Saved CUSTOM semantics

Reuse the candidate's typed resolver: CUSTOM1–CUSTOM5 and C1–C5 aliases;
decimal u32 values; independent declared bit flags combine; multiple slots
remain ordered; boolean/list values must be declared. Sparse list choice numbers
are retained. Duplicate/conflicting slots, unknown bits, opaque declarations,
malformed values, changed slave evidence and mismatched game identities refuse.
Zero values mean unset; unrelated tooltypes are preserved by the editor and do
not become launch arguments. Slaves with unsafe argument-splitting names refuse
trainer projection. Option/slave reads reject symlinks, FIFOs and oversized files.

Pure CUSTOM projections in tests are previews, never emulator consumption.
All current emulator versions, unknown versions and executables merely named
`fs-uae-launcher` remain unsupported. Future support needs a verified adapter,
not a filename/version allowlist or a caller-provided capability flag.

## Preservation and retirement

Original worktree: `/home/davedap/emuwiz-whdload-trainer-launch-20261016` remains
at `dc205e92`, unchanged. Its four commits are reachable through
`preservation/whdload-trainer-dc205e92-20261017` and a verified bundle at
`/home/davedap/emuwiz-audit-ledger/2026-10-17-whdload-finish/original-four-commits.bundle`.
The bundle requires `61d21eab`, independently verified as an ancestor of main.
On completion its Cargo task is released for normal 48-hour idle retirement.
Keep the original worktree until `worktree-check` says CLEAR; retain the
preservation ref and bundle even after any later worktree removal. No history
or original worktree is deleted by this finish.
