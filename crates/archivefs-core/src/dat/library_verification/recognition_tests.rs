//! The preview's catalogue check uses the same strict recognition as the
//! catalogue setup. Disposable fixtures only.
use super::*;
use crate::dat::catalogue_selection::{CatalogueInventoryInputs, CatalogueRef, resolve_catalogue};
use crate::dat::sources::{DatSourceEntry, DatSourceRegistry};

const PLATFORM_REFUSAL: &str =
    "Catalogue platform is unknown, ambiguous or incompatible with the selected system.";

/// The preview's answer for a catalogue with `header`, optionally set to
/// `assigned` by the person, checked for `platform`.
fn preview_for(header: &str, assigned: Option<&str>, platform: &str) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite3");
    Database::open_or_create(&db).unwrap();
    let dat = dir.path().join("catalogue.dat");
    std::fs::write(
        &dat,
        format!(
            "<datafile><header><name>{header}</name><version>1</version></header>\
             <game name=\"G\"><rom name=\"g.bin\" size=\"1\" sha1=\"{}\"/></game></datafile>",
            "0".repeat(40)
        ),
    )
    .unwrap();
    let mut entry = DatSourceEntry::new("c".into(), "c".into(), dat, DatSourceKind::File);
    entry.platform = assigned.map(str::to_string);
    let mut registry = DatSourceRegistry::default();
    registry.add(entry).unwrap();
    let resolved = resolve_catalogue(
        &CatalogueRef::local("c"),
        CatalogueInventoryInputs {
            local_registry: &registry,
            managed_sources: &Default::default(),
            managed_root: dir.path(),
            limits: DatLimits::default(),
        },
    )
    .unwrap();
    preview_library_verification(resolved, db, platform.into(), None).map(|_| ())
}

fn refused_for_platform(result: Result<(), String>) -> bool {
    matches!(result, Err(error) if error == PLATFORM_REFUSAL)
}

#[test]
fn an_ordinary_game_list_for_the_system_is_accepted() {
    assert!(!refused_for_platform(preview_for(
        "Nintendo - Game Boy Color",
        None,
        "Game Boy Color"
    )));
    assert!(!refused_for_platform(preview_for(
        "Apple - II (WOZ)",
        Some("Apple II"),
        "Apple II"
    )));
}

#[test]
fn source_code_named_after_a_system_is_refused() {
    // The older resolver read this header as Game Boy Color.
    assert!(refused_for_platform(preview_for(
        "Source Code - Nintendo - Game Boy Color",
        Some("Game Boy Color"),
        "Game Boy Color"
    )));
}

#[test]
fn a_trimmed_name_is_not_a_match() {
    // "PlayStation Mobile" is not the PlayStation.
    assert!(refused_for_platform(preview_for(
        "Sony - PlayStation Mobile (PSN)",
        Some("PSX"),
        "PSX"
    )));
}

#[test]
fn extras_are_only_accepted_when_the_person_chose_the_system() {
    let header = "Nintendo - Game Boy Advance (e-Reader)";
    assert!(refused_for_platform(preview_for(
        header,
        None,
        "Game Boy Advance"
    )));
    assert!(!refused_for_platform(preview_for(
        header,
        Some("Game Boy Advance"),
        "Game Boy Advance"
    )));
}

#[test]
fn a_game_list_for_another_system_is_refused() {
    assert!(refused_for_platform(preview_for(
        "Nintendo - Game Boy Color",
        None,
        "Game Boy"
    )));
}

#[test]
fn an_ambiguous_catalogue_is_accepted_only_for_the_system_the_person_chose() {
    let header = "IBM - PC and Compatibles";
    assert!(refused_for_platform(preview_for(header, None, "PC")));
    assert!(!refused_for_platform(preview_for(header, Some("PC"), "PC")));
    assert!(!refused_for_platform(preview_for(
        header,
        Some("DOS"),
        "DOS"
    )));
    // Chose PC, checking DOS: refused.
    assert!(refused_for_platform(preview_for(header, Some("PC"), "DOS")));
    // A system the header does not name is refused even if chosen.
    assert!(refused_for_platform(preview_for(
        header,
        Some("Amiga"),
        "Amiga"
    )));
}

#[test]
fn multi_system_catalogues_are_refused() {
    assert!(refused_for_platform(preview_for(
        "Arcade - PC-based",
        Some("Arcade"),
        "Arcade"
    )));
}
