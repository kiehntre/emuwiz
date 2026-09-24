# Simple Mode / UX Rescue

Starting main: `6def4bf23e9f673cf3db5eac790ded9af45af5bb`.
Worktree: `/home/davedap/emuwiz-simple-mode-ux-rescue`.
Branch: `feature/simple-mode-ux-rescue`.

## GUI audit and changes

| Area | Before | Simple Mode |
| --- | --- | --- |
| First launch | Gamer View bypassed task pages; setup required changing modes | Home with six explained tasks, plus My Games in the sidebar |
| Fix Problems | Diagnostics/recovery tabs and repair-history explanation before the first check | Review my setup → Check My Setup → review a finding; repair history is secondary |
| DATs & Verification | Library subpage with a catalogue chooser and management sections | Direct Check My Games entry; platform, relevant verification data, folder, Verify |
| Identity Providers | Independent provider snapshot status above verification | Technical disclosure; imported Arcade data does not require the program/provider |
| Managed DAT Sources | Software-list/source controls competed with the normal workflow | Not on Check My Games; retained under Advanced / More tools |
| Collection Coverage | Catalogue-centric counts and assignment terminology | Plain, scoped last-check results; detailed evidence remains expandable |
| Identify & Rename | MAME readiness counted software lists, ignoring local Arcade data | Arcade verification readiness is separate; software lists are an optional technical detail |
| Verify Games | Choose an installed catalogue from an unfiltered inventory | One assigned source is selected automatically; competing sources require a choice |
| Ready to Play | Read-only readiness report without a launch handle | Play explains the next step and offers Choose a game to play; readiness is secondary |
| Mods & Cheats | Empty selection opened system/provider overview | Choose Game, preview/install signpost, details only after selection |
| Playing Library | Technical catalogue inventory and 1G1R rules before preview | Platform-filtered verification-data chooser, Preview playing library, version rules under Advanced; existing approvals retained |
| Sources / Discovery | Folder setup mixed with catalogue, connection, and source-role concepts | Add folder → scan → View games; advanced management remains available |
| Emulator setup | Dense all-platform candidates, technical status labels | Choose platform → Check Emulators → Change setup; one-column candidates and plain statuses |

## Arcade acceptance journey

Before: change to Advanced View → Library → DATs & Verification → distinguish
the MAME provider from local catalogues → assign Arcade → choose a catalogue →
choose a folder → find Verify.

After: Check My Games → Arcade → see `MAME 0.174 Arcade` → accept/select the
games folder → Verify Arcade Collection → read the results. No installed MAME
program is required. An imported but unassigned file with current, structurally
parsed MAME arcade evidence first offers “Use it for Arcade?”; Yes changes the
draft, and Save setup explicitly persists the reviewed changes.

This confirmation is a GUI join onto the existing registry. It does not create
a managed provider, activate a snapshot, infer an ecosystem from a filename,
or treat a MAME software list as arcade machine data. Folder sources and
uncertain data do not receive the automatic suggestion. Multiple matching
catalogues are never silently elected. Removed explicit choices are not
silently replaced. Audits continue to parse/read through the existing worker.

## Doors, handles, signposts, and escape routes

Simple Mode has a visible location trail and Back/Home controls. Task cards
explain their purpose. Check My Games adds platform and result trails, Back to
platforms, View games, Change setup, and Advanced details. Import checks the data
on the existing worker; assignment/save require explicit actions. Cancel setup
discards the draft; Cancel check uses the worker's existing cancellation rules.
An in-progress, non-cancellable parse is labelled and cannot pretend to cancel.

Navigation, filters, disclosures, and browsing do not start repairs or activate
providers. Existing transactional review/confirmation is unchanged. Original
game media is never rewritten by this feature. No live GUI was restarted.

Body text in Simple Mode is 18 px, button text 17 px, primary buttons at least
44 px high, and important tasks use a readable single column. Status includes
words, not just colour. Advanced and Gamer mode preferences remain readable;
existing explicit choices are preserved, and new/unreadable profiles default
to Simple Mode. Advanced View is reachable from every Simple Mode task.

## Scope

GUI presentation, navigation, executable input validation, tests, and this audit
only. No core identity architecture, acquisition ecosystem, emulator adapter,
mod format, launch mechanism, or original media changes.

Verification-file rejection occurs before configuring or running MAME, including
`.dat`/`.xml` files marked executable and renamed XML. The message explains that
the data is safe, MAME is optional for imported-data checks, and where to import
it. The original error remains under Technical details.

Headless tests cover import/confirmation/save/reload/verification, actual button
actions, ambiguity, stale/missing data, software-list separation, safe exploration,
default navigation, retained advanced routes, and executable rejection. Manual
live-window/large-text acceptance remains a follow-up QA task; the GUI was not
restarted or operated against a real collection during this implementation.

## Changed files

All Rust paths below are relative to `crates/archivefs-gui/src/`:

- Task shell and routing: `simple_mode.rs`, `view_mode.rs`, `navigation.rs`,
  `navigation/primary.rs`, `app.rs`, `app_shell.rs`, `app_pages.rs`,
  `app_reactions.rs`, `lib.rs` (one module declaration).
- Verification presentation and shared input validation:
  `dat_sources_page/simple.rs`, `dat_sources_page/simple/tests.rs`,
  `dat_sources_page.rs`, `dat_sources_page/tests.rs`,
  `dat_catalogue_picker.rs`, `identity_providers_page.rs`.
- Task handles and mode-preserving links: `sources_page.rs`,
  `problems_repair_page.rs`, `emulator_setup_page.rs`,
  `rom_organisation_page.rs`, `playing_library_page.rs`,
  `cheats_mods_preview.rs`, `cheats_mods/controller.rs`,
  `selected_game_readiness.rs`.
- Legacy test fixture with an explicit mode: `tests/mod.rs`.
- This audit: `docs/SIMPLE_MODE_UX_RESCUE.md` (repository-relative).

## Validation

All Cargo commands used `CARGO_TARGET_DIR=/home/davedap/.cache/emuwiz-cargo-target`
and `CARGO_INCREMENTAL=0`; no `/dev/shm` target or competing Cargo jobs.

| Check | Final implementation result |
| --- | --- |
| GUI `--lib simple` | 16 passed |
| GUI `--lib dat_sources_page` | 267 passed |
| GUI `--lib mame` | 8 passed |
| GUI `--lib identify_rename` | 3 passed |
| GUI `--lib navigation` | 28 passed, 1 failed (Converter title mismatch) |
| `cargo test -p archivefs-core --lib` | 9,518 passed, 3 ignored |
| `cargo test -p archivefs-gui --lib` | 2,737 passed, 3 failed, 2 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `git diff --check` | Passed |
| `scripts/task-postcheck.sh` with preflight baseline and 25 exact allowed paths | Passed; GUI root growth +1 |

The full GUI failures are:

- `tests::health_and_platform_actions::library_renders_multiple_complete_rows_at_desktop_and_small_viewports`
- `tests::platform_shelf_and_library_shell::every_navigation_destination_has_a_title_and_width_policy`
- `tests::platform_shelf_and_library_shell::major_workflows_are_reachable_from_home_sidebar_and_top_menu`

All three were independently reproduced by running the same tests against a
pristine `git archive` of starting main
`6def4bf23e9f673cf3db5eac790ded9af45af5bb`, extracted into a separate temporary
directory. The Advanced Library failure has identical 1024×600 row geometry;
the other two have the same `Converter` versus `Disc Conversion` mismatch.
They are pre-existing failures, not new Simple Mode regressions. Neither their
assertions nor the unrelated Advanced Library layout/converter naming was
changed to mask them.

An earlier repeated core run encountered `Text file busy` in the existing
managed-AppImage subprocess timeout test; the latest full core run passed.
Compile/test failures found during implementation were fixed before the final
focused run. The legacy GUI fixture explicitly retains Gamer View so its
mode-transition assertions do not accidentally test the new novice default.

## Main integration and live Sunshine handoff

The authoritative main was checked again before integration and was still
`6def4bf23e9f673cf3db5eac790ded9af45af5bb`, exactly the feature's parent.
The integration uses `/home/davedap/emuwiz-simple-mode-main-integration` on
`integration/simple-mode-ux-rescue-main`, created from that checked main.
Source: `f1db44a96ccba5f4b0d0f75bfd1631ff9cd71c52`.

There were no newer commits or conflicts to reconcile. Integration preserves
the source feature's GUI code byte-for-byte; only this handoff documentation
is added. In particular, the shared mods controller changes are mode-preserving
navigation only, not changes to RPCS3, PCSX2, PPSSPP, or Cemu mod operations.
No core, adapter, DAT ecosystem, launch implementation, or original media is
changed. The four pre-existing untracked research documents in main remain
outside the integration.

The fresh integration validation reran every command in the table above with
the required shared target and incremental compilation disabled. Results were
unchanged: Simple Mode 16 passed; DAT Sources 267 passed; MAME 8 passed;
Identify & Rename 3 passed; navigation 28 passed with the known naming failure;
full core 9,518 passed and 3 ignored; full GUI 2,737 passed, the same 3 known
failures, and 2 ignored. Strict workspace/all-target/all-feature Clippy passed.
Staged and unstaged whitespace checks and the exact-path task postcheck passed.
No test assertions or unrelated layout/naming code were changed during integration.

After validation and the local fast-forward, build from authoritative main:

```sh
CARGO_TARGET_DIR=/home/davedap/.cache/emuwiz-cargo-target \
CARGO_INCREMENTAL=0 cargo build --release -p archivefs-gui
```

The primary release GUI executable is
`/home/davedap/.cache/emuwiz-cargo-target/release/emuwiz`.
It launches the native GUI v2 experience. The package also builds
`emuwiz-gui` and the legacy `archivefs-gui` compatibility aliases; all three
resolve to the same v2 application.

Run as `davedap`, using the existing XFCE/Sunshine X11 session (not a new
display or D-Bus session):

```sh
env -u WAYLAND_DISPLAY \
  DISPLAY=:0 \
  XAUTHORITY=/home/davedap/.Xauthority \
  XDG_RUNTIME_DIR=/run/user/1000 \
  DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus \
  /home/davedap/.cache/emuwiz-cargo-target/release/emuwiz
```

The existing live profile has an explicit `advanced` GUI preference. It is
not overwritten by integration: choose **Help → Simple Mode** once after
launch. Fresh profiles default to Simple Mode. Do not use a fresh isolated
profile for this acceptance run: it would hide the existing verification setup.
No GUI is started or stopped by this handoff.

### Required live acceptance (still pending)

- Home → Check My Games → Arcade → see **MAME 0.174 Arcade**.
- If the simple **Use it for Arcade?** confirmation appears, approve it and
  save the setup; no technical source-management screen should be needed.
- Accept/select `/mnt/usbdrive/games/arcade` → **Verify Arcade Collection** →
  understand the results without opening Technical details.
- Visit all six tasks and My Games. Each must make its purpose, primary
  action, next step, and Back/Home route obvious. Check readability through
  Sunshine and ensure Advanced/details remain secondary.
- Explore without approving changes: no media changes, provider activation,
  or accidental repairs should result from navigation or inspection.

Passing automated tests and producing the release binary is an engineering
handoff, not product acceptance. Product completion requires this real-user
Sunshine test and visual review.
