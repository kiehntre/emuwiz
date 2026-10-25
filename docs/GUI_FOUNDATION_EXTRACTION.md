# GUI Foundation Extraction

This milestone added UI-independent presentation and safety models for the
approved Gamer View / Advanced View redesign. It did not implement either
view's screens, navigation, or actions.

> **Outcome (2026-09-16): none of the five models was adopted.** The redesign
> shipped its own live implementations instead, and the preparatory code sat
> unreferenced for roughly seven weeks. It was removed once the GUI became a
> library target, which is what made the dead code visible: `pub` at a binary
> crate root exempts items from dead-code analysis and `pub(crate)` in a
> library does not. Each section below records what replaced it. The two
> modules that survive - `bulk_confirmation` and `game_presentation` - kept
> only the parts the shipped GUI actually calls, which were moved into them
> later and are unrelated to this milestone.

## Modules and responsibilities

### `view_mode`

This milestone added a preparatory `ViewMode::{Gamer, Advanced}` model with
`label()`, `persisted()`/`from_persisted()`, `ALL`, `Display` and `FromStr`,
and deliberately no storage wiring.

**Superseded.** The navigation reset that followed shipped `GuiMode` -
`GamerView`/`AdvancedView` - with the real mode switching and a real
`gui_mode.txt` preference file, and nothing ever adopted `ViewMode`. The two
sat side by side in this module until `ViewMode` was removed; `GuiMode` is now
the single mode identity. The persisted values are unchanged (`gamer` /
`advanced`), and the display labels live with the controls that show them
(`navigation::GAMER_MENU_ADVANCED_LABEL`).

### `status_wording`

`StatusContext`, `PlainStatus` and `plain_status()` were the beginner-wording
mapper. **Removed.** Its only caller was `SelectedGamePresentation`, itself
never called. The live beginner wording is
`cheats_mods::controller::mount_validation_label`, pinned by
`mount_validation_labels_distinguish_ready_mounted_and_collision`. The module
now holds only the scan/upgrade wording moved into it later.

### `game_presentation`

`SelectedGamePresentation`, `PathAvailability`, `GameTechnicalStatus` and the
kind/label helpers were the selected-game model. **Removed** - and already
recorded as dead by `docs/reviews/CHEATS_MODS_COMPLETION_AUDIT.md`. The live
selected-game surfaces are `selected_game_panel::show_selected_page` for
Advanced View and `gamer_view/stage.rs` for Gamer View. The module now holds
only the platform identity wording moved into it later.

### `bulk_confirmation`

`BulkConfirmation`, `ConfirmationState`, `ConfirmationValidation` and
`TYPED_CONFIRMATION_THRESHOLD` were the confirmation state machine.
**Removed.** The shipped gate is `show_bulk_action_typed_count_gate` with
`bulk_action_requires_typed_count` / `_typed_count_matches` /
`_confirm_enabled` - same threshold of 25, five call sites in `library_view`,
and a superset of the removed tests' assertions. The module now holds only
that gate.

### `selection_guard`

`SelectionGuard`, `SelectionToken` and `SelectionBound`. **Module deleted** -
it had zero references anywhere in the repository, production or test. As this
section already said, the helpers only *complemented* protections the GUI
already had: `RefreshGeneration`, `DatabaseGeneration`, the inspector and
preparation generations, and the stale-generation check every `poll_*` performs
before installing a result. Those remain and are extensively tested.

## Intended integration

The only shared-file integration is five public module declarations near the
top of the GUI crate root. (Those declarations are now `pub(crate)` in
`crates/archivefs-gui/src/lib.rs`: the GUI became a library with thin
`src/bin/` launchers, and a library's `pub` is real public API, so the four
surviving modules are crate-internal and marked `#[allow(dead_code)]` while
they remain unadopted.) Claude Code can consume the models
incrementally from the new modules while implementing screens. No current
rendering function calls them, so cherry-picking does not change visible
behaviour.

For selected-game presentation, resolve identity availability, Cheats & Mods
availability, and Undo availability through the existing workflow first, then
pass those booleans/status values into `SelectedGamePresentation`. For async
work, retain the existing request key or provider generation and add a
`SelectionToken` at the presentation boundary.

## Conflict risk

Risk against `feature/gui-navigation-reset` is low. New implementation is in
new files. The one likely textual conflict is the small `mod` declaration block
near the imports in `main.rs` if navigation work added modules at the same
location; resolution is to retain both sets of declarations. No screen
composition, navigation enum, `ArchiveFsApp` field, rendering function,
provider, adapter, database, or core enum was edited.

## Cherry-pick order

The commits are intentionally ordered and should be cherry-picked as follows:

1. `f8904d7` — view mode and beginner status wording
2. `332a41a` — selected-game presentation model
3. `d1860d0` — bulk confirmation model
4. `d015df4` — selection generation guards
5. Documentation/validation commit containing this file

The first two are dependent because the selected-game model uses
`status_wording`. The bulk and selection commits are otherwise independent,
but preserving this order gives the tested branch state.

## Deliberately untouched

- Gamer View and Advanced View screen/layout implementation
- navigation, sidebar, gear menu, Settings, Mount, and Cheats & Mods rendering
- action-panel and mode-switch UI wiring
- Mount All and all destructive/backend action wiring
- provider, adapter, identity resolution, and async provider implementations
- archivefs-core enums and backend transaction logic
- databases, migrations, ROMs, and emulator profiles
- broad `main.rs` extraction or unrelated rendering functions
