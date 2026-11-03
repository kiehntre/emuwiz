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
    preview_with(header, assigned, false, platform)
}

fn preview_with(
    header: &str,
    assigned: Option<&str>,
    recognised: bool,
    platform: &str,
) -> Result<(), String> {
    preview_full(header, assigned, recognised, false, platform)
}

/// As [`preview_for`], with the assignment marked as applied from a
/// recognised header (`recognised`), or saved with no record of who set it
/// (`unrecorded`), rather than chosen by the person.
fn preview_full(
    header: &str,
    assigned: Option<&str>,
    recognised: bool,
    unrecorded: bool,
    platform: &str,
) -> Result<(), String> {
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
    if recognised {
        entry.platform = assigned.map(str::to_string);
        entry.platform_recognised_from = Some(header.to_string());
    } else {
        entry.set_platform_by_person(assigned.map(str::to_string));
    }
    if unrecorded {
        // As an older build saved it: a system with no record of who set it.
        entry.platform_chosen_by_person = false;
    }
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

#[test]
fn a_system_applied_from_recognition_never_vouches_for_extras_or_ambiguity() {
    // The catalogue was replaced after its system was applied automatically.
    for (header, platform) in [
        ("Nintendo - Game Boy (Private)", "Game Boy"),
        ("Nintendo - Game Boy Advance (e-Reader)", "Game Boy Advance"),
        ("IBM - PC and Compatibles (Flux)", "DOS"),
        ("Source Code - Nintendo - Game Boy", "Game Boy"),
    ] {
        assert!(
            preview_with(header, Some(platform), true, platform).is_err(),
            "{header}"
        );
    }
    // An ordinary game list still counts, whoever set the system.
    assert!(preview_with("Nintendo - Game Boy", Some("Game Boy"), true, "Game Boy").is_ok());
    // The person choosing the same extras themselves is still accepted.
    assert!(
        preview_with(
            "Nintendo - Game Boy Advance (e-Reader)",
            Some("Game Boy Advance"),
            false,
            "Game Boy Advance"
        )
        .is_ok()
    );
}

#[test]
fn an_unrecognised_name_segment_is_not_an_ordinary_list_for_verification() {
    assert!(preview_for("Nintendo - Nintendo 3DS - Digital", None, "Nintendo 3DS").is_err());
    assert!(
        preview_for(
            "Nintendo - Nintendo 3DS - Digital",
            Some("Nintendo 3DS"),
            "Nintendo 3DS"
        )
        .is_ok()
    );
}

#[test]
fn a_system_with_no_record_of_who_set_it_is_treated_strictly() {
    // An ordinary game list is still accepted, as before.
    assert!(
        preview_full(
            "Nintendo - Game Boy",
            Some("Game Boy"),
            false,
            true,
            "Game Boy"
        )
        .is_ok()
    );
    // But it never vouches for extras or for one of several systems.
    for (header, platform) in [
        ("Nintendo - Game Boy Advance (e-Reader)", "Game Boy Advance"),
        ("IBM - PC and Compatibles (Flux)", "DOS"),
    ] {
        assert!(
            preview_full(header, Some(platform), false, true, platform).is_err(),
            "{header}"
        );
    }
}

#[test]
fn non_redump_discs_are_accepted_only_for_the_system_the_person_chose() {
    let header = "Non-Redump - Sega - Dreamcast";
    // Nobody chose it: refused.
    assert!(refused_for_platform(preview_for(header, None, "Dreamcast")));
    // Set automatically, or with no record of who set it: refused.
    assert!(preview_with(header, Some("Dreamcast"), true, "Dreamcast").is_err());
    assert!(preview_full(header, Some("Dreamcast"), false, true, "Dreamcast").is_err());
    // The person's own choice, for the system the header names: accepted.
    assert!(!refused_for_platform(preview_for(
        header,
        Some("Dreamcast"),
        "Dreamcast"
    )));
    // A system the header does not name is refused even if chosen.
    assert!(refused_for_platform(preview_for(
        header,
        Some("PSX"),
        "PSX"
    )));
    // Non-game Non-Redump discs are refused even if chosen.
    assert!(refused_for_platform(preview_for(
        "Non-Redump - Audio CD",
        Some("Dreamcast"),
        "Dreamcast"
    )));
}
