# External emulator launch parity audit (Flatpak / AppImage)

Status: research audit only — no code changed. Evidence gathered from the
repository at branch `feature/archivefs-unified-platform`
(commit `76fc2c3`, workspace version tracked in `Cargo.toml`), scoped to
`crates/archivefs-core` and `crates/archivefs-gui`. This audit deliberately
does not touch, and does not evaluate, anything under Cheats & Mods /
`cheat_journey` / `local_cheat_install` / `user_cheat_import_page` — that
area has separate in-flight work.

This is a companion audit to `docs/V1_PACKAGING_FEASIBILITY.md`, which asked
"can EmuWiz *itself* ship as an AppImage/Flatpak". This audit asks a
different question: "can EmuWiz *launch a user's already-installed*
Flatpak/AppImage emulator", which is orthogonal and does not depend on how
EmuWiz itself is packaged.

## 1. Executive summary

EmuWiz already discovers Flatpak and AppImage emulator installations in
detail — for RetroArch as a first-class `ProfileKind::AppImage` /
`ProfileKind::Flatpak`, and for eight other adapters (Dolphin, PCSX2,
DuckStation, PPSSPP, RPCS3, xemu, Cemu, Flycast, MelonDS, Hatari, mGBA,
SameBoy, Vita3K) as `FlatpakUser`/`FlatpakSystem`/`Portable`/`AppImage`
installation-type variants used today only to locate cheat/patch/config
trees. None of these kinds can currently be launched. Every adapter that has
a real launch path (RetroArch, PCSX2, Dolphin, DuckStation, PPSSPP, RPCS3,
xemu) enforces an explicit, intentional, well-documented native-only gate,
both in `archivefs-core`'s launch-binding resolvers and again independently
in the `archivefs-gui` button-eligibility functions. This is not an
oversight; it is a scope boundary that was never widened.

The two non-native kinds are not equally hard to close:

- **AppImage is close to free.** The codebase already spawns a discovered
  AppImage directly via `Command::new(&executable)` with no shell, for the
  managed PPSSPP/PCSX2 first-run bootstrap
  (`crates/archivefs-core/src/managed_appimage_bootstrap.rs:216-221`), and
  RetroArch's own command-plan builder already resolves an exact,
  verified-executable AppImage path
  (`crates/archivefs-core/src/launch/retroarch_command.rs:170-178`) — the
  *only* thing missing for AppImage parity is removing/relaxing the
  profile-kind gates that never let that resolved path reach a spawn call,
  plus the same symlink/executable-bit rechecks the native path already
  does. No new argv shape is needed: an AppImage is invoked with exactly the
  same argv as a native executable.
- **Flatpak is genuinely different work**, not a gate to delete. RetroArch's
  own command-plan builder documents *why* it refuses to build a Flatpak
  command: a Flatpak profile proves an *installed application*, not an
  *exact executable path* — `flatpak run <app-id> ...` is a structurally
  different, two-stage argv (launcher + app id + separator + emulator args)
  that every adapter would need to construct explicitly
  (`crates/archivefs-core/src/launch/retroarch_command.rs:179-183`). It also
  raises real filesystem-sandbox and `PATH`-dependency questions this audit
  answers in Sections 5 and 8.

Recommended smallest-safe slice (Section 9): ship AppImage launch parity for
RetroArch first (highest existing scaffolding), generalize to the other
AppImage-capable adapters, and treat Flatpak launch as a separate, later
slice gated on an explicit filesystem-permission and `flatpak run` argv
design — never bundled into the AppImage work.

## 2. Current discovery matrix

| Profile kind | Discovery exists? | File / line |
|---|---|---|
| RetroArch native | DONE | `crates/archivefs-core/src/emulator_environment/retroarch.rs:876-884` (`discover_profile(..., ProfileKind::Native, ...)`) |
| RetroArch AppImage (distinct config) | DONE | `crates/archivefs-core/src/emulator_environment/retroarch.rs:902-937` (`appimage_candidates`, `partition_by_config_association`, `discover_profile(..., ProfileKind::AppImage, ...)`); candidate shape in `retroarch.rs:663-681` (`AppImageCandidate`) |
| RetroArch Flatpak (user + system) | DONE | `crates/archivefs-core/src/emulator_environment/retroarch.rs:939-963` (`flatpak_metadata_found`, two `discover_profile(..., ProfileKind::Flatpak, ...)` calls for `ProfileScope::User`/`System`) |
| PCSX2 native / `NativeAlternate` / Flatpak (user+system) / Portable | DONE (discovery + eligibility) | `crates/archivefs-core/src/patch_manager/pcsx2_local.rs:50-60` (`Pcsx2InstallationType`), profile scan around `pcsx2_local.rs:352-382` |
| Dolphin native / AppImage / Flatpak (user+system) / Explicit | DONE | `crates/archivefs-core/src/patch_manager/dolphin_local.rs:54-60` (`DolphinInstallationType` — the only adapter with a first-class `AppImage` installation-type variant besides RetroArch), Flatpak app-id constant `dolphin_local.rs:49` |
| DuckStation native / FlatpakUser / Portable / Explicit | DONE | `crates/archivefs-core/src/patch_manager/duckstation_local.rs:44-49` |
| PPSSPP native / FlatpakUser / Portable / Explicit + EmuWiz-managed AppImage | DONE | `crates/archivefs-core/src/patch_manager/ppsspp_local.rs:43-48`; managed AppImage install evidence in `crates/archivefs-core/src/diagnostics/profiles.rs:124-160` |
| RPCS3 native / FlatpakUser / Portable / Explicit | DONE | `crates/archivefs-core/src/patch_manager/rpcs3_local.rs:68-73` |
| xemu native / FlatpakUser / Portable / Explicit | DONE | `crates/archivefs-core/src/patch_manager/xemu_local.rs:33-38` |
| Cemu / MelonDS / Flycast / Hatari / mGBA / SameBoy native / FlatpakUser / Portable / Explicit | DONE | one `*_local.rs` module each, e.g. `cemu_local.rs:32-37`, `melonds_local.rs:17-22`, `flycast_local.rs:37-42` |
| Vita3K | DONE (adapter exists; installation-type shape not enumerated in this pass) | `crates/archivefs-core/src/patch_manager/vita3k_local.rs` |
| Generic PATH/`~/Applications`/Flatpak AppImage sweep (Doctor/profiles) | DONE | `crates/archivefs-core/src/diagnostics/profiles.rs:180-337` (fixed `~/Applications/<Emulator>/<Emulator>.AppImage` roots for Dolphin/PCSX2/PPSSPP/RPCS3/xemu/DuckStation/Xenia, plus Flatpak user/system root sweep at `profiles.rs:197-217`) |
| EmuWiz-managed AppImage (PPSSPP, PCSX2) download + install-marker evidence | DONE | `crates/archivefs-core/src/emulator_download.rs:40-105`, `crates/archivefs-core/src/managed_appimage_bootstrap.rs:147-156` (`exact_managed_install`) |
| Flatpak sandbox config-tree path derivation (`~/.var/app/<app-id>/...`) | DONE | per-adapter `FLATPAK_APP_ID` constants + sandbox path joins, e.g. `dolphin_local.rs:352`, `duckstation_local.rs:37`, `ppsspp_local.rs:36`, `flycast_local.rs:30`, `hatari_local.rs:29`, `retroarch.rs:872-874` (`flatpak_sandbox_home`, `flatpak_config_dir`) |
| GUI surfacing of discovered kind (labels only) | DONE | `crates/archivefs-core/src/diagnostics/profiles.rs:1518-1519,1911-1912,2085-2086` ("Portable/AppImage xemu", "Flatpak PPSSPP", etc.) |

Every profile kind the audit was asked to cover is already discoverable.
Discovery is not the gap.

## 3. Current execution matrix

| Profile kind | Launch implemented? | File / line |
|---|---|---|
| RetroArch Native | DONE | `crates/archivefs-core/src/launch/execution.rs` (full preflight+spawn pipeline); spawn at `process_spawn.rs:157-185` via `execution.rs:601-617` |
| RetroArch AppImage | PARTIAL | Command-plan construction already resolves an exact, confidence-`Exact`, executable-bit-verified AppImage path (`launch/retroarch_command.rs:170-178`), but `preflight_retroarch_launch` refuses the request before that plan is ever reached: gate at `launch/execution.rs:293-302`. GUI never offers the button: gate at `crates/archivefs-gui/src/launch_readiness_page.rs:1362-1364`. |
| RetroArch Flatpak | MISSING | Command-plan construction deliberately returns no executable candidates and cannot produce a plan at all (`launch/retroarch_command.rs:179-183`), independent of and in addition to the same two gates as AppImage. |
| PCSX2 Native | DONE | `crates/archivefs-core/src/launch/pcsx2_execution.rs` (full pipeline); binding resolver at `patch_manager/pcsx2_local.rs:805-830` |
| PCSX2 `NativeAlternate`/Portable/FlatpakUser/FlatpakSystem | MISSING | Explicit blocker in `resolve_pcsx2_native_launch_binding`: `patch_manager/pcsx2_local.rs:815-828` (`Pcsx2LaunchBlockerKind::UnsupportedInstallationType`); no AppImage-specific command-plan path exists for PCSX2 at all (unlike RetroArch) — PCSX2's managed AppImage is only ever spawned by the bootstrap module, never by real game launch. GUI eligibility gate: `launch_readiness_page.rs:1179` region (`Launch PCSX2` eligibility). |
| Dolphin Native | DONE | `crates/archivefs-core/src/launch/dolphin_execution.rs` |
| Dolphin AppImage/FlatpakUser/Portable | MISSING | Blocker in `patch_manager/dolphin_local.rs:2208-2217` (`DolphinLocalInstallationType::FlatpakUser`/`Portable` both `Err(... UnsupportedInstallationType)`; `AppImage` variant is not matched as an acceptable binding source either — only `Native` and `Explicit` resolve). GUI gate documented at `launch_readiness_page.rs:1032` ("excludes Flatpak/AppImage/..."). |
| DuckStation Native | DONE | `crates/archivefs-core/src/launch/duckstation_execution.rs` |
| DuckStation FlatpakUser/Portable/Explicit | MISSING | `patch_manager/duckstation_local.rs:1519-1528` |
| PPSSPP Native | DONE | `crates/archivefs-core/src/launch/ppsspp_execution.rs` |
| PPSSPP FlatpakUser/Portable (Explicit accepted alongside Native) | MISSING | `patch_manager/ppsspp_local.rs:379-388` |
| PPSSPP/PCSX2 EmuWiz-managed AppImage — **first-run bootstrap only** | DONE (bootstrap), MISSING (real game launch) | `crates/archivefs-core/src/managed_appimage_bootstrap.rs:216-221` spawns the managed AppImage directly with `Command::new` + no args, purely to force the emulator to create its own config; it is never used to launch actual game content and has no argv/content-injection path. |
| RPCS3 Native (+ Explicit accepted alongside Native) | DONE | `crates/archivefs-core/src/launch/rpcs3_execution.rs` |
| RPCS3 FlatpakUser/Portable | MISSING | `patch_manager/rpcs3_local.rs:218-224` region |
| xemu Native (+ Explicit accepted alongside Native) | DONE | `crates/archivefs-core/src/launch/xemu_execution.rs` |
| xemu FlatpakUser/Portable | MISSING | `patch_manager/xemu_local.rs:415-430` region |
| Cemu, Flycast, MelonDS, Hatari, mGBA, SameBoy, Vita3K, Xenia (any kind) | DEFER | No `launch/*_execution.rs` module exists for these adapters at all yet (only `cemu_execution.rs`, `flycast_execution.rs`, `melonds_execution.rs` exist and are pre-native-launch scaffolding per their own doc comments — e.g. `cemu_execution.rs:38` "It never adds Wine/Proton, AppImage extraction, or Flatpak sandboxing"). Native launch itself is not yet built for these; Flatpak/AppImage parity is out of scope until native launch exists. |
| Generic spawn primitive (`spawn_watched_process`) | DONE, kind-agnostic | `crates/archivefs-core/src/launch/process_spawn.rs:157-185` — takes only `{executable, arguments, working_directory}`; has no concept of profile kind at all, so it already works unmodified for AppImage argv shape and would work for a pre-built Flatpak `flatpak run ...` argv too. |

## 4. Exact native-only blocker(s)

There is no single global gate; the gate is duplicated per adapter by
design (the codebase's own stated principle: "no silent fallback between
profile kinds"). The two canonical instances audited in full:

**RetroArch — `crates/archivefs-core/src/launch/execution.rs:293-302`:**

```rust
    // --- 4: profile kind gate (Flatpak/AppImage refused outright) ---
    if request.profile.profile_kind != ProfileKind::Native {
        return Err(preflight_error(
            LaunchPreflightErrorKind::UnsupportedProfileKind,
            format!(
                "only native RetroArch profiles are supported in this phase, got {:?}",
                request.profile.profile_kind
            ),
        ));
    }
```

Mirrored independently in the GUI so the button never even appears —
`crates/archivefs-gui/src/launch_readiness_page.rs:1362-1364`:

```rust
    if profile.profile_kind != ProfileKind::Native {
        return None;
    }
```

**PCSX2 — `crates/archivefs-core/src/patch_manager/pcsx2_local.rs:815-828`:**

```rust
    match profile.installation_type {
        Pcsx2InstallationType::Native => resolve_default_native_binding(profile, roots),
        Pcsx2InstallationType::NativeAlternate
        | Pcsx2InstallationType::Portable
        | Pcsx2InstallationType::FlatpakUser
        | Pcsx2InstallationType::FlatpakSystem => Err(launch_blocker(
            Pcsx2LaunchBlockerKind::UnsupportedInstallationType,
            format!(
                "only {:?} PCSX2 installations are supported by this native launch binding, got \
                 {:?}",
                Pcsx2InstallationType::Native,
                profile.installation_type
            ),
        )),
    }
```

The same shape (`match profile.installation_type { Native => Ok(...), other-kinds => Err(UnsupportedInstallationType) }`)
repeats verbatim in `dolphin_local.rs:2208-2217`, `duckstation_local.rs:1519-1528`,
`ppsspp_local.rs:379-388`, `rpcs3_local.rs:218-224`, `xemu_local.rs:415-430`.
**One additional, structurally distinct blocker exists below the gate for
RetroArch specifically:** even if the gate above were deleted, Flatpak
profiles would still fail to produce a command, because
`executable_paths()` returns an empty `Vec` for `ProfileKind::Flatpak`
(`launch/retroarch_command.rs:179-183`) — there is no executable path to
plan around, by design, until Flatpak gets its own argv construction
(Section 5).

## 5. Flatpak launch requirements

**What the stored profile data structures actually contain today** (not
assumed): every adapter stores only a compile-time-constant `FLATPAK_APP_ID`
(e.g. `"org.DolphinEmu.dolphin-emu"`, `"org.duckstation.DuckStation"`,
`"org.ppsspp.PPSSPP"`, `"org.libretro.RetroArch"`, `"org.flycast.Flycast"`,
`"org.tuxfamily.Hatari"`) plus a derived sandbox config root
(`~/.var/app/<app-id>/config/...` or `/data/...`) and, for RetroArch, a
`flatpak_metadata_found: bool` (installed-app-directory evidence only — see
`Evidence::flatpak_metadata_found`, `emulator_environment/retroarch.rs:270-274`).
**No field for branch, no field for extra Flatpak runtime args, no field
for a persisted `flatpak run` argv.** `EmulatorDownloadSpec::flatpak_id`
(`emulator_download.rs:48`) is populated for RetroArch (`Some("org.libretro.RetroArch")`)
but `None` for every AppImage-distributed managed emulator, and it is used
only by the download picker, never by any launch path.

- **Minimum correct argv contract (MISSING today):** `flatpak run <app-id> -- <emulator argv>`.
  The `--` separator matters: without it, `flatpak run` can misinterpret an
  emulator's own flags (e.g. `-L` for a RetroArch core) as `flatpak run`'s
  own options. No code in this repository builds this argv shape anywhere;
  `retroarch_command.rs:179-183` is the only place that even acknowledges
  the difference, and it acknowledges it by refusing rather than building
  it. Building it is real, adapter-visible work, not a one-line change.
- **Branch:** not tracked or stored anywhere. `flatpak run` defaults to the
  app's default branch (usually `stable`) when `--branch` is omitted, so
  omitting it is a safe default, not a silent behavior change from a value
  EmuWiz already knew — but it does mean EmuWiz cannot detect or launch a
  side-installed `beta`/`testing` branch a user deliberately chose. DEFER:
  document the default-branch assumption; do not add branch tracking in the
  first slice.
- **Filesystem sandbox permissions — MISSING, not automatically solved.**
  ROMs/ISOs live under arbitrary user source roots and `/mnt` (per
  `V1_PACKAGING_FEASIBILITY.md` Section 2, `mount_root` defaults to
  `/mnt/archivefs`). A Flatpak-sandboxed emulator's default filesystem
  permissions (from its Flathub manifest) are typically `home` plus maybe
  `xdg-download`; they do **not** include `/mnt` or arbitrary other mount
  points. `flatpak run` does not grant any new permission at invocation
  time — permissions are whatever `flatpak override` (or the app's own
  manifest) already established for that installed app. This repository
  never runs `flatpak override` and has no code path that inspects an
  installed Flatpak's *current* filesystem permission grants (only whether
  the app directory exists — Section 2 discovery). **Concretely: launching
  a Flatpak emulator against content under `/mnt/archivefs` can silently
  fail inside the sandbox (file not found from the emulator's point of
  view) even though the exact same `flatpak run` command succeeds against
  content under `$HOME`.** This is the single biggest correctness risk of
  Flatpak launch and is not visible from anything EmuWiz inspects today.
- **Document portal / URI forwarding:** not currently relevant because
  EmuWiz would invoke `flatpak run <app-id> -- <path>` with a plain
  filesystem path argument (matching every native adapter's own argv
  shape), not a URI or `--file-forwarding` request — that is only needed
  when the *caller* is itself sandboxed and wants the portal to broker
  access to a file outside its own already-declared permissions. EmuWiz
  itself is unsandboxed today (`docs/security.md`, referenced in
  `V1_PACKAGING_FEASIBILITY.md:22`), so it is not a document-portal client;
  the exposure is entirely on the *emulator's* side (previous bullet).
- **Existing filesystem overrides:** none configured or assumed by this
  repository anywhere. There is no `flatpak override --filesystem=...`
  invocation, guidance string, or Doctor diagnostic in the codebase today.
  A Flatpak launch slice would need to either (a) document a manual
  `flatpak override --filesystem=<mount_root>` step for the user (matching
  this repo's existing "Doctor prints plain-language guidance, never runs
  privileged/mutating commands itself" pattern — e.g. the `ratarmount`
  install guidance in `lib.rs:1539-1540`), or (b) add a Doctor diagnostic
  that detects the missing grant before ever attempting to spawn. Actually
  running `flatpak override` on the user's behalf would be a policy change
  (EmuWiz mutating another application's sandbox) well outside "smallest
  safe slice" and should not be attempted without separate explicit design.
- **Emulator config path inside the sandbox — DONE at discovery, but launch
  spawn has no equivalent guard.** Discovery already derives
  `~/.var/app/<app-id>/config/<emulator>` instead of the native
  `~/.config/<emulator>` for every adapter (Section 2). The risk this audit
  was asked to check — "does EmuWiz assume the native config path when
  writing cheat/save-state/config files for a profile discovery says is
  Flatpak" — is **not present** for the *patch/cheat* write paths audited
  (`patch_manager/*_local.rs` already branch on `installation_type`/
  `flatpak_app_id` before choosing a config root, e.g.
  `dolphin_local.rs:423,710`). It *would* become a live risk the moment a
  Flatpak launch path is added, because a newly-written command-plan
  builder must reuse this same already-correct sandbox-path derivation
  rather than re-deriving a native-looking path from scratch — call this
  out explicitly in any implementation PR's review.
- **`flatpak` binary location — PATH lookup, with existing precedent for
  both styles.** `flatpak` itself is never invoked anywhere in this
  codebase today. Precedent exists for two different levels of rigor: (a)
  `xdg-open` is spawned via a bare `Command::new("xdg-open")` relying on
  process `PATH` resolution with no pre-check (`crates/archivefs-gui/src/main.rs:20914`);
  (b) `ratarmount`/`fusermount3` are pre-checked with `command_available()`
  (`crates/archivefs-core/src/lib.rs:8009-8018`) before being relied upon,
  and Doctor reports plain-language guidance if missing. A Flatpak launch
  path should follow pattern (b) — `flatpak` is a hard dependency for that
  entire launch kind (unlike `xdg-open`, which is best-effort UX) — and
  surface "flatpak is not installed" as a distinct, named preflight
  failure, not a generic spawn error.
- **Launch failure diagnostics needed but absent today:** no
  `LaunchPreflightErrorKind`/blocker variant exists anywhere for "Flatpak
  binary missing", "app id not installed", "sandbox permission likely
  denied", or "flatpak run exited non-zero because of a sandbox filesystem
  restriction". `spawn_watched_process` captures bounded stderr for
  diagnostic purposes already (`process_spawn.rs:31-36,94-103`), which
  would surface the emulator's own "file not found" complaint, but nothing
  in the codebase currently distinguishes a sandbox-permission failure from
  any other failure — that message would arrive as opaque emulator-owned
  stderr text a user has no path to act on (they'd need to be told to run
  `flatpak override --filesystem=...` and cannot infer that from a generic
  "file not found").

## 6. AppImage launch requirements

- **Argv shape — no new construction needed.** An AppImage is a directly
  executable ELF (self-mounting SquashFS + ELF stub); it is invoked exactly
  like a native executable: `./Foo.AppImage <same args a native binary
  would take>`. RetroArch's own command-plan builder already proves this by
  resolving an AppImage path through the exact same `executable` field a
  native path fills (`retroarch_command.rs:170-178,272`); the resulting
  `RetroArchCommand.arguments` (`-L`, core, content) does not change based
  on whether the executable happens to be an AppImage. **DONE at the
  planning level for RetroArch; MISSING (no plan builder even attempts it)
  for every other adapter, because their `resolve_*_native_launch_binding`
  functions refuse the installation type before an executable-resolution
  step like RetroArch's `executable_paths()` is ever reached.**
- **Executable-bit and symlink-safety — precedent exists, reuse it
  verbatim.** The exact discipline already used for native executables
  applies unchanged to an AppImage path:
  - `crates/archivefs-core/src/launch/execution.rs:471-495` (`recheck_executable`):
    `fs::symlink_metadata` (never follows a symlink), refuses if
    `file_type().is_symlink()` or not `is_file()`, refuses if the Unix mode
    has no `0o111` bit set.
  - The AppImage discovery layer already carries an `ExecutableState`
    per candidate (`AppImageCandidate.executable: Option<ExecutableState>`,
    `retroarch.rs:670-673`) and RetroArch's plan builder already filters on
    `ExecutableState::Executable` (`retroarch_command.rs:175`).
  - `managed_appimage_bootstrap.rs` independently re-derives the same
    "regular file, not a symlink" check for its own narrower purpose
    (`safe_regular_file`, `managed_appimage_bootstrap.rs:136-139`).
  A generalized AppImage launch path should reuse `recheck_executable`
  (or an equivalent shared helper) rather than re-deriving a third copy of
  this logic — DONE as reusable pattern, MISSING as an actually-shared
  function across all three sites today (mild duplication risk, not a
  correctness gap).
- **No shell, exact argv — already guaranteed by the shared spawn
  primitive.** `spawn_watched_process` (`process_spawn.rs:157-166`) uses
  `Command::new` + `.args()` unconditionally; there is no branch anywhere
  in this function on file type or extension. An AppImage path flowing
  through the existing `RetroArchCommand`/`PreparedProcessCommand` shape
  gets the exact-argv guarantee for free. **DONE, no new code needed.**
- **Environment inheritance — the one real AppImage-specific risk, already
  identified once before.** `V1_PACKAGING_FEASIBILITY.md` Section 9's
  decision table already flagged this from the *other* direction (EmuWiz
  running from inside an AppImage): the AppImage runtime exports
  `APPDIR`, `APPIMAGE`, `ARGV0`, and sometimes `LD_LIBRARY_PATH` into every
  child process. The scenario this audit is about is the mirror image —
  **EmuWiz (running normally, not from an AppImage) spawning a
  user-installed emulator that itself happens to be an AppImage.** In that
  direction the risk is smaller but not zero: when the *user's shell*
  originally launched EmuWiz from inside another AppImage's mount (e.g. a
  terminal opened by an AppImage-packaged file manager) those variables
  could already be present in EmuWiz's own environment and would propagate
  unchanged to the launched emulator AppImage, same as they do to every
  other spawned child today (`spawn_watched_process` never filters env —
  "inherited exactly" is a stated invariant, `execution.rs:592-595`). The
  target AppImage runtime itself unconditionally overwrites `APPDIR`/
  `APPIMAGE`/`ARGV0` for its own mount on exec, so this is self-correcting
  for the *launched* AppImage's own use of those variables; the only
  latent hazard is `LD_LIBRARY_PATH` if a parent AppImage's runtime left
  one pointing at library versions the launched emulator AppImage does not
  expect. **DEFER**: worth a one-line Doctor note ("if EmuWiz was itself
  started from inside another AppImage's mounted environment, launched
  emulators inherit that environment"), not worth new spawn-time env
  filtering — filtering would itself violate the "child inherits exactly,
  no override" principle this audit was told to preserve.
- **Host FUSE runtime assumption.** Most AppImages self-mount via FUSE at
  exec time; on a host without `/dev/fuse` or `fusermount`/`fusermount3`
  available, the AppImage fails at its own startup (not something EmuWiz's
  spawn call can detect in advance — it is opaque to `Command::spawn`,
  which only reports whether `execve` itself succeeded, not whether the
  AppImage's internal SquashFS mount then failed). Two facts make this a
  narrow, already-partially-covered risk rather than a new one: (1) EmuWiz
  already has a hard `fusermount3`/`fusermount` dependency for its own
  archive mounting (`V1_PACKAGING_FEASIBILITY.md` Section 5), so a host
  capable of running EmuWiz's primary feature is already capable of
  supporting a self-mounting AppImage; (2) newer AppImage runtimes fall
  back to `--appimage-extract-and-run` automatically when FUSE is
  unavailable, so *most* AppImages on a no-FUSE host still work, just
  slower on first launch. **DEFER**: do not add a bespoke FUSE preflight
  probe for launched AppImages in the first slice — bounded stderr capture
  (`PROCESS_STDERR_CAPTURE_LIMIT`, already spawned for every launch) will
  surface the AppImage runtime's own "cannot mount AppImage, please check
  your FUSE setup" message verbatim if it happens, which is sufficient
  diagnostic surface for a first slice.
- **Existing "managed AppImage" pattern is the correct one to generalize.**
  `managed_appimage_bootstrap.rs` already establishes: validate an
  install-marker/provenance record immediately before spawn (never trust a
  stale path -- `exact_managed_install`, lines 147-156), spawn with
  `Command::new(&executable)` + explicit empty/known args (never a shell,
  line 216-221), bound the wait, require caller-visible evidence after a
  clean exit. This is a stricter, narrower cousin of exactly the
  `preflight_*_launch` → `spawn_watched_process` shape RetroArch/PCSX2
  native launch already use. **The AppImage launch slice should route
  through the same `preflight_retroarch_launch`/`spawn_retroarch` (and
  equivalent per-adapter) pipeline that native launch uses today, not
  invent a second bootstrap-shaped pipeline** — the bootstrap module solves
  a different, narrower problem (first-run config creation, no game
  content, no args) and its safety machinery does not need to be
  rearchitected, only referenced as precedent for the executable-safety
  checks.

## 7. Adapter-specific exceptions

| Adapter | Shares generic spawn layer? | Needs bespoke command-plan work? | Why |
|---|---|---|---|
| RetroArch | Yes | AppImage: no (already resolves); Flatpak: yes | `executable_paths()` already branches per `ProfileKind` and already produces a correct AppImage executable; only the profile-kind gates need to fall. Flatpak needs a wholly new `flatpak run <app-id> -- ...` argv builder distinct from `executable_paths()`'s executable-path model entirely — it produces an *argv prefix*, not an executable substitute. |
| PCSX2 | Yes (spawn), No (plan) | Yes, both AppImage and Flatpak | `resolve_pcsx2_native_launch_binding` has no AppImage-executable-resolution branch at all (unlike RetroArch's `executable_paths()`) — `NativeAlternate` (PCSX2's AppImage-observed path pattern) is refused outright, not partially supported. AppImage parity for PCSX2 needs a new resolution branch mirroring RetroArch's, not just a gate removal. |
| Dolphin | Yes (spawn), No (plan) | Yes, both AppImage and Flatpak | Same shape as PCSX2: `DolphinLocalInstallationType::AppImage` exists as a *discovery* variant but is not matched as an acceptable binding source in `resolve_dolphin_native_launch_binding`'s `match` (only `Native`/`Explicit` resolve) — closest to RetroArch's situation of the non-RetroArch adapters, since the discovery-side type distinction already exists. |
| DuckStation, PPSSPP, RPCS3, xemu | Yes (spawn), No (plan) | Yes, both AppImage and Flatpak | None of these distinguish an AppImage installation type from `Portable`/`FlatpakUser` at the type level the way Dolphin/RetroArch do — `Portable` is the closest existing bucket an AppImage would fall into, and it is blanket-refused today. Adding AppImage parity here requires first deciding whether an AppImage counts as `Portable` (existing type) or needs its own new variant threaded through discovery, not just execution — more design work than RetroArch/Dolphin. |
| Cemu, Flycast, MelonDS, Hatari, mGBA, SameBoy, Vita3K, Xenia | N/A | DEFER entirely | No real native launch pipeline (`launch/*_execution.rs`) exists yet for these adapters; Flatpak/AppImage parity is meaningless before native launch exists. Explicitly out of scope for this audit's implementation slices. |
| PPSSPP/PCSX2 managed AppImage bootstrap | Its own narrow pipeline (`managed_appimage_bootstrap.rs`), not the general one | No — it already works for its one purpose (first-run config creation) | Solves a different problem (create config, not run a game); should stay separate, but its safety pattern (marker-verified path, `Command::new` no shell, bounded wait) is the reference implementation for the executable-safety half of AppImage launch. |

**Can one generic layer cover both kinds?** The spawn primitive
(`spawn_watched_process`) already does — it is kind-agnostic and needs no
change for either AppImage or Flatpak. The **command-plan construction**
layer cannot be unified: AppImage fits the existing "resolve one exact
executable path, then reuse the native argv shape" model with only a
kind-check relaxation; Flatpak requires a structurally different plan
shape (`executable = "flatpak"`, `arguments = ["run", app_id, "--", ...emulator argv]`)
that every adapter's command-plan builder would need to grow a distinct
branch for, mirroring the `executable_paths()` match RetroArch already has
at `retroarch_command.rs:161-184` but extended with a real Flatpak arm
instead of `Vec::new()`.

## 8. Security/safety implications

- **No profile-kind substitution risk introduced.** Every proposed change
  in Section 9 is additive to existing `match`/gate statements (adding a
  new matched arm), never a change to how a *requested* kind is matched
  against a *discovered* candidate — the existing "never a silent
  substitution" invariant (`RequestedCandidateNotFound` in
  `execution.rs:147-150`, `Pcsx2LaunchBlockerKind::ProfileRootMismatch`,
  etc.) is untouched by adding AppImage/Flatpak arms.
- **No shell introduced.** Both a resolved AppImage path and a constructed
  `flatpak run` argv pass through the same `Command::new` + `.args()`
  primitive that never invokes `sh -c`; `flatpak run <app-id> -- <argv>` is
  itself just another argv list, not a shell string, so it does not weaken
  the "never a shell" invariant even though it is a "launcher launching a
  launcher."
- **Flatpak sandbox permission opacity is a genuine new safety-relevant
  surface, not merely a UX rough edge.** Because EmuWiz cannot read an
  installed Flatpak's actual current filesystem grants (only whether the
  app directory exists), a Flatpak launch could succeed at the process
  level (`Command::spawn` returns `Ok`) while failing at the file-access
  level inside the emulator, in a way indistinguishable from a "this game
  won't boot" emulator bug from the emulator's own stderr alone. This is a
  correctness/diagnostics risk, not a memory-safety or privilege-escalation
  risk — EmuWiz is not granting itself, or the emulator, any new
  permission; it is only invoking `flatpak run`, and the sandbox enforces
  whatever it already enforces regardless of what invoked it.
- **AppImage executable-bit/symlink checks must be applied to the exact
  path being spawned, at spawn time, not at discovery time** — this mirrors
  the existing `recheck_executable` re-check-immediately-before-spawn
  pattern (`execution.rs:385-394`) that already defends against a
  time-of-check/time-of-use swap for native executables; an AppImage launch
  path must reuse this same "recheck immediately before spawn" discipline,
  not just the discovery-time `ExecutableState` snapshot (which can be
  stale by the time Launch is actually clicked, exactly as the existing
  doc comment for native RetroArch explains at `execution.rs:225-241`).
- **`flatpak` PATH lookup does not need path-realpath/symlink hardening
  beyond what `command_available` already gives Doctor-style checks** —
  `flatpak` is a well-known system package manager entry point, not
  user-supplied data; the existing `command_available()` pattern
  (`lib.rs:8009-8018`) is adequate precedent and no stricter check is
  warranted (this differs from the *content*/executable paths, which are
  user-controlled and do warrant symlink/regular-file rechecks).
- **No mutation of another application's sandbox.** This audit explicitly
  recommends against EmuWiz ever invoking `flatpak override` on the user's
  behalf (Section 5) — that would be EmuWiz silently altering another
  installed application's security posture, a much larger safety change
  than "spawn a process the user asked for," and is out of scope for any
  "smallest safe slice."

## 9. Recommended implementation slices (ordered, smallest-safe-first)

1. **RetroArch AppImage launch parity.** Relax the gate at
   `launch/execution.rs:293-302` to accept `ProfileKind::AppImage` (as well
   as `Native`), and relax the matching GUI gate at
   `launch_readiness_page.rs:1362-1364`. No change needed to
   `retroarch_command.rs` — `executable_paths()` already produces a correct
   AppImage executable candidate. Add one new preflight recheck path
   mirroring `recheck_executable`/`recheck_core_library` if the AppImage
   path itself needs an equivalent immediate-before-spawn re-check beyond
   what `build_retroarch_command_plan` already re-derives fresh each time
   (it does re-derive fresh from a freshly rediscovered environment, so
   this may already be covered — verify during implementation, don't
   assume). This is the smallest possible slice: one gate relaxed in core,
   one gate relaxed in GUI, zero new argv-construction code.
2. **Dolphin AppImage launch parity.** Add an `AppImage`-accepting arm to
   `resolve_dolphin_native_launch_binding`'s match
   (`dolphin_local.rs:2208-2217`) that performs the equivalent of
   RetroArch's `executable_paths()` AppImage-candidate resolution (exact
   confidence, executable-bit verified) against Dolphin's own discovered
   AppImage evidence, then relax the corresponding GUI gate
   (`launch_readiness_page.rs:1032` region). Second-smallest because the
   `AppImage` installation-type variant already exists at the discovery
   layer — only the binding resolver needs a new arm, not a new discovery
   type.
3. **PCSX2, DuckStation, PPSSPP, RPCS3, xemu AppImage launch parity.**
   Requires first deciding, per adapter, whether an AppImage-installed copy
   should be represented as a new discovery-level type (matching Dolphin's
   `AppImage` variant) or reuse the existing `Portable`/`NativeAlternate`
   bucket with an added executable-resolution branch. Recommend mirroring
   Dolphin's shape (dedicated variant) for consistency across adapters,
   done one adapter at a time behind its own test suite, each following
   the RetroArch pattern (resolve exact executable → reuse native argv
   shape → recheck immediately before spawn).
4. **EmuWiz-managed AppImage (PPSSPP/PCSX2) real-game launch, not just
   bootstrap.** Once slice 3 lands PPSSPP/PCSX2 AppImage binding
   resolution generally, verify the EmuWiz-managed install marker path
   (`exact_managed_install`) can also feed that same generic binding
   resolver, so a user who let EmuWiz download PPSSPP/PCSX2 as an AppImage
   gets real launch, not just first-run bootstrap. Explicitly reuse
   `managed_appimage_bootstrap.rs`'s marker-verification pattern rather
   than re-deriving install-provenance trust from scratch.
5. **Flatpak launch design spike (no code yet).** Before writing any
   Flatpak command-plan code: (a) decide and document the exact
   `flatpak run <app-id> [--branch=...] -- <argv>` construction per
   adapter; (b) decide the Doctor-diagnostic story for missing filesystem
   permissions (detect-and-warn vs. leave entirely to user documentation);
   (c) decide the new `LaunchBlockerKind`/`LaunchPreflightErrorKind`
   variants needed for "flatpak not installed" / "app id not found
   installed" distinct from a generic spawn failure. This slice produces a
   short design doc, not code, and should be reviewed before slice 6.
6. **RetroArch Flatpak launch parity.** Implement the design from slice 5
   for RetroArch specifically first (it already has the cleanest
   `executable_paths()`-shaped seam to extend at
   `retroarch_command.rs:179-183`), gated behind a Doctor-surfaced
   filesystem-permission warning rather than a silent attempt.
7. **Remaining adapters' Flatpak launch parity**, one at a time, each
   reusing the RetroArch Flatpak argv-construction pattern and each
   requiring its own `resolve_*_native_launch_binding` new arm exactly like
   the AppImage slices above.

Flatpak work (slices 5-7) is a separate, later track gated on the design
spike, and should never ship before AppImage parity (slices 1-4) is
proven, per the executive summary's difficulty ordering.

## 10. Definition of Done

A launch-parity slice for a given `(adapter, profile kind)` pair is done
only when **all** of the following hold, mirroring the bar RetroArch/PCSX2
native launch already meets:

- The profile-kind (or installation-type) gate accepts the new kind in
  exactly one place per layer (core binding resolver, core preflight if
  distinct, GUI eligibility function) — no adapter gains a second,
  inconsistent gate.
- The command-plan builder for that adapter produces the *same* argv shape
  contract as native (exact `OsString` arguments, no shell) for AppImage,
  or an explicitly-reviewed `flatpak run <app-id> -- ...` shape for
  Flatpak — never a shell string in either case.
- The executable/core-library recheck-immediately-before-spawn discipline
  (`recheck_executable`-equivalent) runs against the newly-accepted kind's
  resolved path, not only at discovery time.
- No profile-kind substitution is possible: a request naming an AppImage
  profile can never resolve to a native or Flatpak executable and vice
  versa, verified by a test mirroring
  `RequestedCandidateNotFound`/`ProfileRootMismatch`'s existing coverage.
- For Flatpak specifically: a missing `flatpak` binary and a
  not-actually-installed app id each produce a distinct, named preflight
  error (not a generic spawn failure), and release/Doctor documentation
  states the filesystem-permission assumption in Section 5 explicitly
  (either "you may need `flatpak override --filesystem=...`" guidance, or
  an active Doctor diagnostic — either is acceptable, silence is not).
- Test coverage exists at the same rigor as the existing
  `launch/*_execution/tests.rs` and `patch_manager/*_local.rs` binding
  tests: at minimum, one test proving the new kind launches successfully
  end-to-end against a fake executable/script, and one test proving the
  gate still refuses every kind not yet supported (e.g. Flatpak still
  refused while only AppImage parity has shipped).
- No change to `spawn_watched_process`, `process_spawn.rs`, or the
  "inherits environment exactly, no shell, no timeout" invariants
  documented throughout this crate — every slice in Section 9 is additive
  above that layer, never a modification to it.
