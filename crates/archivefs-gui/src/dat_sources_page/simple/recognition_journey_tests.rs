//! The catalogue recognition journey on the Check My Games page, on
//! disposable settings and catalogue files only.
use super::*;
use archivefs_core::dat::sources::{
    DatSourceEntry, DatSourceKind, load_dat_sources_config_from, save_dat_sources_config_to,
};

fn dat(header: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><datafile><header><name>{header}</name><version>1</version></header>\
         <game name=\"G\"><rom name=\"g.bin\" size=\"3\" sha1=\"a9993e364706816aba3e25717850c26c9cd0d89d\"/></game></datafile>"
    )
}

/// Registers catalogues that have never been checked, as an older version
/// left them, and returns the settings path.
fn setup(root: &Path, catalogues: &[(&str, &str)]) -> PathBuf {
    let mut registry = DatSourceRegistry::new();
    for (id, header) in catalogues {
        let path = root.join(format!("{id}.dat"));
        std::fs::write(&path, dat(header)).unwrap();
        registry
            .add(DatSourceEntry::new(
                id.to_string(),
                format!("No-Intro: {header}"),
                path,
                DatSourceKind::File,
            ))
            .unwrap();
    }
    let config = root.join("dat_sources.toml");
    save_dat_sources_config_to(&config, &registry.to_config()).unwrap();
    config
}

fn saved_platforms(config: &Path) -> Vec<(String, Option<String>, bool)> {
    let (registry, _) =
        DatSourceRegistry::from_config(&load_dat_sources_config_from(config).unwrap());
    registry
        .entries()
        .iter()
        .map(|entry| (entry.id.clone(), entry.platform.clone(), entry.enabled))
        .collect()
}

fn edit_elsewhere(config: &Path) {
    let (mut registry, _) =
        DatSourceRegistry::from_config(&load_dat_sources_config_from(config).unwrap());
    let id = registry.entries()[0].id.clone();
    registry.get_mut(&id).unwrap().display_name = "Renamed in another window".into();
    save_dat_sources_config_to(config, &registry.to_config()).unwrap();
}

const CATALOGUES: &[(&str, &str)] = &[
    ("src-165", "Nintendo - Game Boy Color"),
    ("src-170", "Nintendo - Game Boy Advance (e-Reader)"),
    ("src-901", "Source Code - Nintendo - Game Boy Color"),
];

#[test]
fn opening_the_page_checks_nothing_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), CATALOGUES);
    let before = std::fs::read(&config).unwrap();
    let mut state = page(dir.path());
    let text = render(&state.view(), &mut DatSourcesPageUi::default()).join("\n");
    assert!(!state.is_busy(), "opening must not start a check");
    state.poll();
    assert!(
        text.contains("3 catalogues haven\u{2019}t had their system checked yet"),
        "{text}"
    );
    assert!(text.contains("Check catalogues"), "{text}");
    assert!(!text.contains("Apply all safe matches"), "{text}");
    assert_eq!(std::fs::read(&config).unwrap(), before);
    assert!(
        state
            .draft
            .entries()
            .iter()
            .all(|entry| entry.health.header.is_none() && entry.platform.is_none())
    );
}

#[test]
fn checking_then_applying_sets_only_the_safe_match_and_undo_reverts_it() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), CATALOGUES);
    let mut state = page(dir.path());
    let mut ui = DatSourcesPageUi::default();

    let action = click(&state.view(), &mut ui, "Check catalogues");
    assert_eq!(action, DatSourcesPageAction::ValidateAll);
    state.apply(action);
    finish(&mut state);
    // Checking alone changes no setting.
    assert!(
        saved_platforms(&config)
            .iter()
            .all(|(_, platform, _)| platform.is_none())
    );

    let view = state.view();
    let text = render(&view, &mut ui).join("\n");
    assert!(
        text.contains("1 ordinary game list is ready to set"),
        "{text}"
    );
    assert!(
        text.contains("1 catalogue has extras for a system"),
        "{text}"
    );
    assert!(
        text.contains("1 catalogue can\u{2019}t be used to check single games"),
        "{text}"
    );
    assert!(text.contains("Review 1 safe match"), "{text}");
    // Plain English: no internal source IDs.
    assert!(
        !text.contains("src-165") && !text.contains("src-170"),
        "{text}"
    );

    ui.simple.recognition.preview_open = true;
    let text = render(&view, &mut ui).join("\n");
    assert!(
        text.contains("No-Intro: Nintendo - Game Boy Color \u{2192} Nintendo Game Boy Color"),
        "{text}"
    );
    assert!(
        !text.contains("e-Reader) \u{2192}"),
        "extras are never previewed: {text}"
    );
    let action = click(&view, &mut ui, "Apply all safe matches (1)");
    assert!(matches!(
        &action,
        DatSourcesPageAction::ApplyRecognisedSystems { preview } if preview.len() == 1
    ));
    state.apply(action);
    assert_eq!(
        saved_platforms(&config),
        vec![
            ("src-165".into(), Some("Game Boy Color".into()), true),
            ("src-170".into(), None, true),
            ("src-901".into(), None, true),
        ]
    );
    let text = render(&state.view(), &mut ui).join("\n");
    assert!(text.contains("Set the system for 1 catalogue."), "{text}");
    assert!(
        text.contains("1 catalogue already has its system"),
        "{text}"
    );

    let action = click(&state.view(), &mut ui, "Undo");
    assert_eq!(action, DatSourcesPageAction::UndoRecognisedSystems);
    state.apply(action);
    assert!(
        saved_platforms(&config)
            .iter()
            .all(|(_, platform, _)| platform.is_none())
    );
}

#[test]
fn apply_is_refused_when_the_settings_changed_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), CATALOGUES);
    let mut state = page(dir.path());
    state.apply(DatSourcesPageAction::ValidateAll);
    finish(&mut state);
    edit_elsewhere(&config);
    let after_edit = std::fs::read(&config).unwrap();
    let preview = state.view().recognition.safe;
    assert_eq!(preview.len(), 1);
    state.apply(DatSourcesPageAction::ApplyRecognisedSystems { preview });
    let message = state.view().recognition.message.unwrap().unwrap_err();
    assert!(message.contains("changed somewhere else"), "{message}");
    assert_eq!(std::fs::read(&config).unwrap(), after_edit);
    // Reload shows the other window's edit; checking again then applies.
    state.apply(DatSourcesPageAction::ReloadSetup);
    assert_eq!(
        state.saved.entries()[0].display_name,
        "Renamed in another window"
    );
    state.apply(DatSourcesPageAction::ValidateAll);
    finish(&mut state);
    let preview = state.view().recognition.safe;
    state.apply(DatSourcesPageAction::ApplyRecognisedSystems { preview });
    assert!(state.view().recognition.message.unwrap().is_ok());
    assert_eq!(
        saved_platforms(&config)[0].1.as_deref(),
        Some("Game Boy Color")
    );
}

#[test]
fn save_setup_never_overwrites_an_edit_made_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), CATALOGUES);
    let mut state = page(dir.path());
    state.apply(DatSourcesPageAction::SetEnabled {
        id: "src-901".into(),
        enabled: false,
    });
    edit_elsewhere(&config);
    let after_edit = std::fs::read(&config).unwrap();
    state.apply(DatSourcesPageAction::Save);
    match &state.save_state {
        DatSaveState::Failed(error) => assert!(error.contains("changed somewhere else"), "{error}"),
        other => panic!("save must be refused, got {other:?}"),
    }
    assert_eq!(std::fs::read(&config).unwrap(), after_edit);
}

#[test]
fn a_single_assignment_never_overwrites_an_edit_made_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), CATALOGUES);
    let mut state = page(dir.path());
    edit_elsewhere(&config);
    let after_edit = std::fs::read(&config).unwrap();
    state.apply(DatSourcesPageAction::AssignPlatformNow {
        id: "src-170".into(),
        platform: "Game Boy Advance".into(),
    });
    let result = state.view().platform_assignment.unwrap();
    assert!(
        matches!(&result, Err(error) if error.contains("changed somewhere else")),
        "{result:?}"
    );
    assert_eq!(std::fs::read(&config).unwrap(), after_edit);
}

#[test]
fn choosing_extras_is_an_individual_explicit_choice() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), CATALOGUES);
    let mut state = page(dir.path());
    state.apply(DatSourcesPageAction::ValidateAll);
    finish(&mut state);
    state.apply(DatSourcesPageAction::AssignPlatformNow {
        id: "src-170".into(),
        platform: "Game Boy Advance".into(),
    });
    assert!(state.view().platform_assignment.unwrap().is_ok());
    let saved = saved_platforms(&config);
    assert_eq!(saved[1].1.as_deref(), Some("Game Boy Advance"));
    // The ordinary list was not touched by that choice.
    assert_eq!(saved[0].1, None);
    let row = state
        .view()
        .recognition
        .rows
        .into_iter()
        .find(|row| row.source_id == "src-170")
        .unwrap();
    assert_eq!(
        super::recognition::row_text(&row),
        "Used for Nintendo Game Boy Advance (your choice)"
    );
}
