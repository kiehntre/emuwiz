//! Core treats a `gui-v2.json` equal to `FIRST_START_GUI_PREFERENCES_JSON` as
//! a disposable first-start leftover when it chooses between an EmuWiz and a
//! legacy ArchiveFS setup. These tests pin that constant to what Blue really
//! writes on a first start, so a change to the defaults shows up here instead
//! of silently turning every stray first-start folder into a second setup.

use super::*;

/// A first start: the real constructor, then the Restore a missing
/// `gui-v2.json` produces (the defaults), then the snapshot the Save
/// command writes.
#[test]
fn a_first_start_saves_exactly_the_core_first_start_preferences() {
    let context = egui::Context::default();
    let mut app = App::new(context);
    app.restore_preferences(backend::Preferences::default());
    let saved = serde_json::to_value(app.saved_preferences()).unwrap();
    let core: serde_json::Value =
        serde_json::from_str(archivefs_core::app_dirs::FIRST_START_GUI_PREFERENCES_JSON).unwrap();
    assert_eq!(
        saved, core,
        "update FIRST_START_GUI_PREFERENCES_JSON in app_dirs.rs to match Blue's first-start save"
    );
}

#[test]
fn core_first_start_preferences_match_the_default_preferences() {
    let default = serde_json::to_value(backend::Preferences::default()).unwrap();
    let core: serde_json::Value =
        serde_json::from_str(archivefs_core::app_dirs::FIRST_START_GUI_PREFERENCES_JSON).unwrap();
    assert_eq!(default, core);
}
