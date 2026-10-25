# Cheat routing by selected emulator, and post-install loadability

**Branch:** `feature/cheat-routing-loadability` (from `main` @ `f42c88f5`)
**Motivation:** `docs/research/CHEAT_FORMAT_ADAPTER_COVERAGE_AUDIT.md`
found that EmuWiz could report a cheat install as successful when the selected
emulator would never read the file. Two causes:

- Every platform other than PS2, GameCube, Wii and Xbox 360 was routed to
  RetroArch. That included PS3, where no libretro core exists.
- "Files written" and "the emulator will load them" were shown as one success
  state.

This change fixes routing and adds a loadability result. It adds no cheat
catalogue, decoder, provider, conflict analysis, or transaction/history change.

## 1. Routing model

Routing is in core, `crates/archivefs-core/src/patch_manager/cheat_route.rs`.
It is pure (no I/O) and deterministic.

| Type | Meaning |
|---|---|
| `CheatRouteTarget` | `RetroArch { core: Option<String> }` or `Standalone { adapter_id }`. Adapter IDs are the same keys that `LAUNCH_COMPATIBILITY` and remembered profiles use. |
| `CheatRouteRequest` | The caller's observed facts: platform, explicit selection, configured defaults, installed standalone emulators, and RetroArch installed state plus cores for the platform. |
| `CheatRouteDecision` | One of `Routed(CheatRoute)`, `Refused { selected, refusal, alternatives }`, `Ambiguous { candidates }`, or `NoRoute`. |
| `CheatRoute` | Target, `CheatRouteBasis`, `CheatApplySupport`, native format, and the other emulators the user may choose explicitly. |
| `CheatApplySupport` | `Supported` = RetroArch, PCSX2, Dolphin, Xenia, DuckStation, PPSSPP, mGBA, and MAME. `InventoryOnly` = Flycast and RPCS3. `Unsupported` = everything else. |

### Precedence

1. **Explicit selection.** The user chose an emulator for this game. If it
   serves the platform, it is routed. If not, the decision is `Refused` with
   alternatives. It is never replaced silently.
2. **Configured default.** A remembered emulator profile that serves the
   platform. Two different defaults for one platform produce `Ambiguous`.
3. **Platform fallback.** Used only when the fallback emulator can consume the
   format:
   - A writable standalone emulator owns its platform: PS2 → PCSX2,
     GameCube/Wii → Dolphin, Xbox 360 → Xenia.
   - RetroArch is a fallback only for platforms with a reviewed libretro core
     hint, or platforms outside the reviewed table.
   - If a standalone emulator for the same platform is observed installed,
     the decision is `Ambiguous` and the user chooses.
   - Platforms with no RetroArch route (PS3, 3DS, Vita, Xbox, Wii U, ...)
     route to their standalone emulator. That emulator is shown as
     inventory-only or unsupported, with no apply.

RetroArch is never a cheat route for PS2, GameCube, Wii or Xbox 360. EmuWiz's
codes for these systems target the standalone file layouts, and the existing
test `gamecube_route_cannot_select_retroarch` still holds.

### Examples

| Game | Selected | Result |
|---|---|---|
| PS1 | DuckStation | DuckStation native `.cht` apply, with no RetroArch substitute when DuckStation owns the route. |
| PS1 | RetroArch (Beetle PSX HW) | RetroArch with that core. Installable. |
| PS1 | none, DuckStation installed, RetroArch installed | `Ambiguous`. The user chooses. |
| PSP | PPSSPP | PPSSPP native CWCheat apply. |
| PSP | RetroArch | RetroArch. The single installed PSP core is adopted. |
| Dreamcast | Flycast standalone | Flycast standalone. Inventory only. |
| Dreamcast | RetroArch Flycast core | RetroArch (flycast). |
| PS3 | none or RPCS3 | RPCS3. Never RetroArch. Explicitly choosing RetroArch is `Refused`. |
| 3DS | Azahar | Azahar. Format not supported. No RetroArch route is invented. |
| PS2 | DuckStation | `Refused`. The alternative offered is PCSX2. |

## 2. RetroArch core handling

- `RetroArch { core: None }` is a real state. RetroArch alone is never treated
  as sufficient identity.
- A selected RetroArch target without a core adopts a core only when exactly
  one installed core's `.info` metadata declares the platform
  (`retroarch_platform_matches`). With several cores, the core stays unknown
  and the route panel offers one explicit button per core.
- The core is recorded in the route and in the loadability report
  (`retroarch_core`, `CheatLoadabilityEvidence::RetroArchCore`). Technical
  details show it after install.
- A core is not written into the shared transaction journal. That would change
  the journal schema, which is out of scope. The per-core auto-load path
  already encodes the core as a directory name.

### Honest RetroArch finding

The current RetroArch installer writes to
`<cheat_database_path>/<platform>/<name>.cht` (`resolve_cheat_destination`).
That is the folder "Quick Menu → Cheats → Load Cheat File" opens. RetroArch
auto-loads only `<cheat_database_path>/<core>/<content>.cht`, per
`cheat_manager_get_game_specific_filename` and the existing
`retroarch.rs::per_game_cheat_destination`.

Loadability compares the installed path with that auto-load path. Today's
platform-folder installs are therefore reported as `PathNotObserved` with the
reason "RetroArch does not load this file automatically. Open Quick Menu →
Cheats → Load Cheat File." Moving the installer to the per-core path is a
follow-up. It was not done here because it changes an install contract.

## 3. Standalone routes and minimum loadability checks

| Emulator | Route | Apply | Expected directory | File check | Enable setting read | Restart |
|---|---|---|---|---|---|---|
| PCSX2 | yes | yes | profile `cheats/` | `*.pnach` | `[EmuCore] EnableCheats` (existing activation reader) | restart game |
| Dolphin | yes | yes | profile `GameSettings/` | `*.ini` | `[Core] EnableCheats` (existing activation reader) | restart game |
| Xenia | yes | yes | profile `patches/` | `*.patch.toml` | `apply_patches` in `xenia-canary.config.toml` | restart title |
| RetroArch | yes | yes | `<cheat_database_path>/<core>/` | `<content stem>.cht` | `apply_cheats_after_load` in `retroarch.cfg` | reload content |
| DuckStation | yes | native apply | `.cht` | per-cheat / profile state | restart game |
| PPSSPP | yes | native apply | CWCheat `.ini` | per-cheat / global state | reload cheats or game |
| mGBA | yes | native apply | configured cheats directory | `.cheats` | native per-set state | restart game |
| MAME | yes | native apply | configured cheat path | cheat XML | native runtime state | restart machine/game |
| Flycast | yes | inventory only | — | — | — | — |
| RPCS3 | yes | inventory only | — | — | — | — |
| Azahar | yes | unsupported | — | — | — | — |

An inventory-only or unsupported emulator gets a known route and a route panel
that says apply is not supported. No workflow is created, so no apply button
can appear.

When a setting key is absent, it is reported as "could not confirm"
(`LoadableExpected`). EmuWiz does not assume the emulator's default.

## 4. Post-install verification

This is `cheat_loadability.rs` in core, wired in at
`cheats_mods/routing.rs::cheat_install_loadability`. It runs once, when the
apply worker reports `Success` or `PartialFailure`:

1. Take the journal entry that was actually written: `InstalledNew`,
   `ReplacedExisting` or `AlreadyInstalled`.
2. Re-read `destination_root/destination_relative_path` without following a
   final symlink, bounded at 16 MiB, and compare its SHA-256 with the
   journal's `final_destination_digest`.
3. Check the file is directly inside the selected profile's cheat directory
   and matches that emulator's name or suffix convention.
4. Read the emulator's cheat-enable setting read-only. PCSX2 and Dolphin reuse
   the workflow's existing activation readers. RetroArch and Xenia use a
   bounded 1 MiB config reader.
5. Look read-only for a running emulator process in Linux `/proc` `comm`
   names, bounded at 8,192 entries. When unavailable the result is
   `NotObserved`.
6. Produce a `CheatLoadabilityReport`.

The check is read-only. The journal, history and rollback are unchanged, and a
test asserts it.

## 5. Loadability states

| State | When | Headline |
|---|---|---|
| `LoadableVerifiedByConfig` | Path matches and the cheat setting is on | "Installed and ready." |
| `LoadableExpected` | Path matches, setting unreadable or absent | "Installed. The selected emulator should load it; EmuWiz could not confirm its cheat setting." |
| `RestartRequired` | As above, but the emulator process was observed running | "Installed. Restart the emulator to load this cheat." |
| `EmulatorCheatsDisabled` | The setting is explicitly off | "Installed, but cheats are disabled in the selected emulator." |
| `PathNotObserved` | Bytes not verified, wrong folder, wrong name, or RetroArch platform-folder install | "Installed, but the selected emulator is not currently configured to load it." |
| `UnsupportedBySelectedEmulator` | Routed emulator is inventory-only or unsupported | "This cheat format is not supported by your selected emulator." |
| `AmbiguousProfile` | RetroArch core unknown | "EmuWiz cannot verify which RetroArch core this install belongs to." |
| `Unknown` | No expected-path information | "Installed. EmuWiz cannot tell whether the selected emulator will load it." |

No text claims that a cheat executed. The GUI adds: "EmuWiz checks files and
settings only; it cannot confirm a cheat works in-game."

## 6. Restart and enablement

- The restart requirement is per emulator (`CheatRestartRequirement`):
  RetroArch needs the content reloaded; the standalone emulators need the game
  restarted.
- Whenever the result says the cheat will load, the result also shows "If the
  game is already running, restart it (or reload the content)".
- `RestartRequired` is reported as a state only when the emulator process is
  actually observed running.
- A cheat switch that is off is a hard "No", never a success.

## 7. DuckStation counter fix

The previous `inspect_cheats` counted only lines starting with `[Cheat` or
`Cheat`. DuckStation names each section after the cheat itself (for example
`[Infinite Health]` with `Type = Gameshark`, `Activation = EndFrame`), so real
files were undercounted, usually as zero.

The fix counts every non-empty `[Section]` header. An enabled entry is still
counted only from an explicit in-file `Enabled = true` key; newer DuckStation
builds keep enable state in per-game settings, which this inventory does not
read. There is a new test with three realistic sections and one empty `[]`
header. The native DuckStation adapter now supplies preview, merge, apply, and
rollback through the shared transaction path.

## 8. Import quick wins

`user_cheat_import` now recognises two more formats, using the existing parsers
only:

- **Dolphin `.ini`** (`UserCheatFormat::DolphinGameSettingsIni`), read by
  `parse_dolphin_ini`. `.ini` is a generic extension, so a file without any
  `[Gecko]` or `[ActionReplay]` code is reported as an ignored file, not a
  malformed cheat. The Game ID comes from the file name.
- **Xenia `.patch.toml`** (`UserCheatFormat::XeniaPatchToml`), read by
  `parse_xenia_patch_toml`. The title ID and title name are used as match
  evidence.

The import page offers "Preview install for Dolphin/Xenia" on these
candidates. The button hands the file to the existing local-install bridges,
which re-validate the Game ID or Title ID against the game selected in
Cheats & Mods before any preview. The indexer itself still never writes. The
page's stale "never offers an install operation" header was corrected.

## 9. GUI changes

- **Route panel** (`show_cheat_route_panel`), at the top of the selected
  system workflow. It shows:
  - the selected emulator and a badge: "Cheats can be installed", "Apply not
    supported yet", "Format not supported", "Cannot use cheats here" or
    "Choose an emulator";
  - the headline, the cheat format, and why this emulator was chosen;
  - the unknown-RetroArch-core caveat when it applies;
  - one explicit "Use …" button per alternative emulator or core, and "Clear
    my emulator choice".
- Choices are kept per archive for the session
  (`ArchiveFsApp::cheat_emulator_selections`). Choosing rebuilds the workflow
  for the newly routed emulator.
- **Loadability card** (`show_cheat_loadability`), shown in the beginner
  result, the shared preview result and the BSFree result. Separate rows show:
  - Selected emulator
  - Install target
  - **Installed file:** Yes, bytes verified / Not verified
  - **Selected emulator will load it:** Yes, verified from its settings /
    Expected, setting not confirmed / After a restart / No / Cannot tell
  - the headline, each reason, and the restart note
  - technical evidence
- Result badges were renamed from "Installed successfully" / "Installed and
  verified" to "Files installed" / "Files installed and verified". Emulator
  loading is reported separately.
- The old "Unsupported platform" banner now appears only when there is no
  route at all. Otherwise the route panel explains the situation.

## 10. Revision safety

- Exact identity is kept. PCSX2, Dolphin and Xenia installs are classed
  `Exact` (CRC, Game ID, Title ID), and RetroArch installs are `Exact` only
  when every materialized source was `VerifiedExact`.
- RetroArch title/platform matches and all BSFree installs are `TitleOnly`.
  The report then always carries "Game revision not verified: this cheat was
  matched by title and platform only.", even when the state is ready.
- No identity is downgraded, and routing never widens a match.

## 11. Known limitations

- Installed-standalone detection is limited to PS1 (DuckStation), PSP (PPSSPP)
  and Dreamcast (Flycast), using the same bounded discovery the launch
  readiness page runs. mGBA and MAME are valid native route targets when
  explicitly selected or remembered; their broader GUI profile discovery is a
  separate concern.
  - The route panel always lists the standalone alternatives.
- RetroArch installs still land in the platform folder, so they are reported as
  needing a manual load (see §2). Moving the installer to the per-core
  auto-load path is the recommended next task.
- The RetroArch per-core directory name uses the existing `retroarch.rs`
  convention (core stem). It has not been re-verified against RetroArch's
  `library_name` in this task.
- The RetroArch `apply_cheats_after_load` and Xenia `apply_patches` keys are
  read literally. When a key is absent, EmuWiz reports "could not confirm" and
  does not assume the emulator's default.
- Explicit emulator choices last for the session only; they are not written to
  `emulator_profiles.toml`.
- Process observation works on Linux only, matches `comm` names, and never
  controls a process.
- Loadability is computed when the apply completes. A result restored from
  History does not recompute it.

## 12. Tests

- **Core `cheat_route` (20):**
  - PS3 routes to RPCS3 and never to RetroArch, including refusing an explicit
    RetroArch choice.
  - PS1 routes to DuckStation or RetroArch; PSP to PPSSPP or RetroArch;
    Dreamcast to standalone Flycast or the RetroArch Flycast core.
  - A selected emulator that doesn't serve the platform is refused; an
    unsupported one is not offered apply.
  - 3DS never gets a RetroArch route.
  - Ambiguous installed standalone + RetroArch; two defaults are ambiguous.
  - Precedence: explicit selection over configured default over fallback.
  - Several RetroArch cores keep the core unknown.
  - Standalone-owned platforms keep their emulator; RetroArch-only platforms
    still fall back to RetroArch.
  - Unknown platform has no route.
  - Repeated routing is deterministic, including input-order invariance.
- **Core `cheat_loadability` (16):**
  - installed bytes are verified, and a digest mismatch or missing file is
    caught;
  - a symlinked destination is refused;
  - cheats enabled, disabled, and setting unreadable;
  - restart required;
  - wrong directory and wrong suffix;
  - an unverified file never reports loadable;
  - the RetroArch core is recorded;
  - a RetroArch platform-folder install needs a manual load;
  - a RetroArch install without a core is ambiguous;
  - Flycast/RPCS3 inventory-only emulators are unsupported for apply;
  - the title-only revision warning is kept, and exact identity is not
    downgraded;
  - the config parser and the `/proc` probe.
- **Core `duckstation_local`:** a test with realistic named sections.
- **Core `user_cheat_import`:** Dolphin `.ini` recognition, unrelated `.ini`
  ignored, Xenia `.patch.toml` recognition.
- **GUI `tests::cheat_routing_loadability` (11):**
  - workspace routing for PS3, PS1 with DuckStation, PS1 with RetroArch, and a
    refused cross-platform selection;
  - the route panel text;
  - a Dolphin install that is ready, including a check that the journal is
    unchanged;
  - cheats disabled, with rendered "Installed file" / "will load it" rows;
  - restart required;
  - path mismatch;
  - a RetroArch install with no core is ambiguous;
  - the beginner result shows the file-install and loadability rows
    separately.
- **Updated tests:**
  - `adapter_routing_is_platform_authoritative`: PS3 → Unsupported (was
    RetroArch), and a PSX → RetroArch assertion was added.
  - Three result-label assertions now expect "Files installed".
