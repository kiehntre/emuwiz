use super::*;

fn game(name: &str, description: Option<&str>, rom_count: usize) -> DatGameEntry {
    DatGameEntry {
        name: name.to_string(),
        description: description.map(str::to_string),
        roms: vec![Default::default(); rom_count],
        ..Default::default()
    }
}

#[test]
fn projects_one_record_per_named_game() {
    let games = vec![
        game(
            "Super Mario Bros. (USA)",
            Some("Super Mario Bros. (USA)"),
            1,
        ),
        game(
            "Super Mario Bros. (Europe)",
            Some("Super Mario Bros. (Europe)"),
            1,
        ),
    ];
    let projection = project_expected_dat_inventory(&games);
    assert_eq!(projection.entries.len(), 2);
    assert_eq!(projection.duplicate_names_skipped, 0);
    let names: std::collections::BTreeSet<&str> = projection
        .entries
        .iter()
        .map(|entry| entry.canonical_identity.as_str())
        .collect();
    assert_eq!(
        names,
        ["Super Mario Bros. (USA)", "Super Mario Bros. (Europe)"]
            .into_iter()
            .collect()
    );
}

#[test]
fn region_and_revision_distinct_names_stay_distinct_entries() {
    // No-Intro/Redump/TOSEC bake region/revision into the name itself - two
    // different names must never collapse into one expected identity.
    let games = vec![
        game("Game (USA) (Rev 1)", None, 1),
        game("Game (USA) (Rev 2)", None, 1),
        game("Game (Europe)", None, 1),
    ];
    let projection = project_expected_dat_inventory(&games);
    assert_eq!(projection.entries.len(), 3);
    assert_eq!(projection.duplicate_names_skipped, 0);
}

#[test]
fn redump_style_multi_disc_entries_stay_distinct() {
    let games = vec![
        game("Final Fantasy VII (USA) (Disc 1)", None, 1),
        game("Final Fantasy VII (USA) (Disc 2)", None, 1),
        game("Final Fantasy VII (USA) (Disc 3)", None, 1),
    ];
    let projection = project_expected_dat_inventory(&games);
    assert_eq!(projection.entries.len(), 3);
}

#[test]
fn a_pathological_exact_duplicate_name_is_refused_not_merged() {
    let games = vec![
        game("Game", Some("First declaration"), 1),
        game("Game", Some("Second declaration, same name"), 1),
    ];
    let projection = project_expected_dat_inventory(&games);
    assert_eq!(projection.entries.len(), 1);
    assert_eq!(projection.duplicate_names_skipped, 1);
    // The first one seen is the one kept - never silently overwritten by
    // the second.
    assert_eq!(projection.entries[0].display_name, "First declaration");
}

#[test]
fn duplicate_pretty_titles_with_different_canonical_names_do_not_collapse() {
    // Two entries whose *description* happens to read the same, but whose
    // DAT-declared name (the actual match key) differs, are not the same
    // identity.
    let games = vec![
        game("gamea", Some("Game (Alt)"), 1),
        game("gameb", Some("Game (Alt)"), 1),
    ];
    let projection = project_expected_dat_inventory(&games);
    assert_eq!(projection.entries.len(), 2);
    assert_eq!(projection.duplicate_names_skipped, 0);
}

#[test]
fn a_missing_description_falls_back_to_the_name_as_display_name() {
    let games = vec![game("clrmamepro-style-entry", None, 1)];
    let projection = project_expected_dat_inventory(&games);
    assert_eq!(projection.entries[0].display_name, "clrmamepro-style-entry");
}

#[test]
fn extend_from_catches_a_duplicate_spanning_two_files_in_one_folder_source() {
    let mut projection = ExpectedDatInventoryProjection::default();
    projection.extend_from(&[game("Game", None, 1)]);
    projection.extend_from(&[game("Game", None, 1), game("Other", None, 1)]);
    assert_eq!(projection.entries.len(), 2);
    assert_eq!(projection.duplicate_names_skipped, 1);
}

#[test]
fn an_empty_catalogue_projects_no_entries() {
    let projection = project_expected_dat_inventory(&[]);
    assert!(projection.entries.is_empty());
    assert_eq!(projection.duplicate_names_skipped, 0);
}

#[test]
fn dat_game_id_is_preserved_when_present_and_none_when_absent() {
    let mut with_id = game("Game", None, 0);
    with_id.id = Some("12345".to_string());
    let games = vec![with_id, game("Other", None, 0)];
    let projection = project_expected_dat_inventory(&games);
    let by_name: std::collections::HashMap<&str, &ExpectedDatEntryRecord> = projection
        .entries
        .iter()
        .map(|entry| (entry.canonical_identity.as_str(), entry))
        .collect();
    assert_eq!(by_name["Game"].dat_game_id.as_deref(), Some("12345"));
    assert_eq!(by_name["Other"].dat_game_id, None);
}
