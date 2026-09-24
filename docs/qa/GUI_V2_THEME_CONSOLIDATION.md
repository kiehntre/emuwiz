# GUI v2 theme consolidation

Date: 2026-09-22  
Starting SHA: `8c19bd0378699a2c47b03f16b4ccd7de57698814`  
Branch: `fix/gui-v2-theme-consolidation`  
Worktree: `/home/davedap/emuwiz-gui-v2-theme-consolidation`

## Inventory and visual evidence

The normal native-v2 routes are all represented by the shared sidebar:

Home, Setup & Doctor, Games, Saves & States, Duplicates, Platforms, Check
Games, Problems & Repair, Organisation, Launch, Emulator Setup, Mods &
Cheats, Artwork & Metadata, Sources, DAT Management, Activity, History,
Settings, and Advanced.

Before the change, native-v2 landing pages used blue primary buttons, while
embedded workflow pages such as DAT Management used the shared legacy
`theme::ACCENT` amber role. This produced the observed mixed/legacy-orange
appearance. The old 1280-wide desktop evidence is retained at
`/tmp/emuwiz-gui-v2-existing-1280.png`.

After the change, the rebuilt `emuwiz 0.9.0 · GUI v2 (native-v2)` was run in
the authenticated `DISPLAY=:0` XFCE session. DAT Management showed blue
primary controls and the shared dark-v2 surfaces. Evidence:

- `/tmp/emuwiz-gui-v2-consolidated-1280.png`
- `/tmp/emuwiz-gui-v2-consolidated-1920.png`

The actual desktop geometry was 1920×1080. The changed binary was also
checked at its default 1280-wide window size. No legacy orange page chrome
was visible in the inspected native-v2/embedded workflow surface.

## Root cause and fix

The inconsistency was not a global GUI-mode selection problem:

1. Native-v2 pages owned blue primary-button literals locally.
2. Shared workflow widgets used `ui::theme::ACCENT`, which was amber.
3. `gui_v2::readable_style` reset the visual palette to egui's stock dark
   visuals after embedded workflows had applied the application theme.

The fix keeps one token source in `ui::theme`:

- `PRIMARY_ACTION` and `PRIMARY_ACTION_HOVER` are the shared blue v2 action
  roles.
- `ACCENT` remains the compatibility name for the primary action role.
- `AMBER`/`WARNING` remain semantic attention colors and were not repurposed.
- `readable_style` now starts from `theme::apply` and only adjusts v2
  typography, spacing, text override, and selection emphasis.
- Native-v2 page, organisation, and onboarding button literals now consume
  `theme::PRIMARY_ACTION`.

Files changed:

- `crates/archivefs-gui/src/ui/theme.rs`
- `crates/archivefs-gui/src/gui_v2/mod.rs`
- `crates/archivefs-gui/src/gui_v2/pages.rs`
- `crates/archivefs-gui/src/gui_v2/organisation.rs`
- `crates/archivefs-gui/src/gui_v2/onboarding.rs`

## Validation

- Focused theme tests: **PASS**, 2 passed.
- Full GUI library suite: **2,826 passed, 8 failed**. The failures were
  unrelated DAT save/audit background-job timeout tests while the shared
  environment was concurrently running other GUI test loops; GUI-v2 page
  tests, including the all-major-pages responsive route test, passed.
- Clippy: **PASS** for the GUI package with `-D warnings`.
- Release GUI build: **PASS** (`cargo build -p archivefs-gui --release --bin emuwiz`).
- `git diff --check`: **PASS**.
- `scripts/task-postcheck.sh`: not run because this task has no supplied
  baseline/allow-list arguments.

Intentional orange remains limited to semantic warning/attention elements
using `WARNING`/`AMBER`; it is no longer used for ordinary page primary
actions or page chrome.
