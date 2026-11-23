//! The MAME probe is only ever asked about a catalogued Arcade set folder. On
//! Blue's v2 routes the saved-library snapshot is not loaded, so the page's own
//! catalogue row must be enough (found live: the probe never ran for 1941).
use archivefs_core::ArchiveKind;
use std::path::PathBuf;

#[test]
fn the_pages_own_arcade_row_is_enough_without_a_loaded_snapshot() {
    let mut app = crate::tests::app_for_operation_tests();
    assert!(app.database_state.snapshot().is_none());
    let set = PathBuf::from("/games/arcade/1941");
    assert!(!app.is_catalogued_arcade_set_directory(&set));

    app.catalogued_loose_content = Some((set.clone(), ArchiveKind::ArcadeSetDirectory));
    assert!(app.is_catalogued_arcade_set_directory(&set));
}

#[test]
fn another_path_or_another_kind_is_never_treated_as_an_arcade_set() {
    let mut app = crate::tests::app_for_operation_tests();
    let set = PathBuf::from("/games/arcade/1941");
    app.catalogued_loose_content = Some((set.clone(), ArchiveKind::ArcadeSetDirectory));
    assert!(!app.is_catalogued_arcade_set_directory(&PathBuf::from("/games/arcade/1942")));
    assert!(!app.is_catalogued_arcade_set_directory(&PathBuf::from("/games/arcade")));

    app.catalogued_loose_content = Some((set.clone(), ArchiveKind::DirectGameImage));
    assert!(!app.is_catalogued_arcade_set_directory(&set));
    app.catalogued_loose_content = None;
    assert!(!app.is_catalogued_arcade_set_directory(&set));
}
