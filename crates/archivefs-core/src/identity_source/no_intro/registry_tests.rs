use std::path::Path;

use tempfile::tempdir;

use crate::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};

use super::registry::{
    NoIntroSourceSelection, no_intro_selection_fingerprint, select_no_intro_source,
};

const GB_NO_INTRO_XML: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Nintendo - Game Boy</name>
        <version>20250101-120000</version>
        <author>No-Intro</author>
    </header>
    <game name="Alleyway (World)">
        <rom name="Alleyway (World).gb" size="32768" crc="9F73FA30" sha1="ed8070e011713527bdc03e2b9cec9f9c4a7e3aaa"/>
    </game>
</datafile>"#;

const GB_NO_INTRO_XML_OTHER: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Nintendo - Game Boy (Rebuild)</name>
        <version>20250601-000000</version>
        <author>No-Intro</author>
    </header>
    <game name="Tetris (World)">
        <rom name="Tetris (World).gb" size="32768" crc="AAAAAAAA" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"/>
    </game>
</datafile>"#;

const TOSEC_XML: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Some TOSEC set</name>
        <version>2025-01-01</version>
        <author>TOSEC</author>
    </header>
    <game name="Not No-Intro">
        <rom name="Not No-Intro.gb" size="1" crc="00000000"/>
    </game>
</datafile>"#;

fn write_dat(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn file_entry(id: &str, path: std::path::PathBuf, platform: Option<&str>) -> DatSourceEntry {
    let mut entry = DatSourceEntry::new(id.to_string(), id.to_string(), path, DatSourceKind::File);
    entry.platform = platform.map(str::to_string);
    entry
}

#[test]
fn no_configured_source_is_not_imported() {
    let registry = DatSourceRegistry::new();
    let selection = select_no_intro_source(&registry, Some("Game Boy"));
    assert!(matches!(selection, NoIntroSourceSelection::NotImported));
}

#[test]
fn non_no_intro_source_stays_not_imported() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "tosec.dat", TOSEC_XML);
    let mut registry = DatSourceRegistry::new();
    registry
        .add(file_entry("tosec", path, Some("Game Boy")))
        .unwrap();

    let selection = select_no_intro_source(&registry, Some("Game Boy"));
    assert!(matches!(selection, NoIntroSourceSelection::NotImported));
}

#[test]
fn configured_no_intro_source_reaches_the_real_lookup_path() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
    let mut registry = DatSourceRegistry::new();
    registry
        .add(file_entry("gb-no-intro", path, Some("Game Boy")))
        .unwrap();

    let selection = select_no_intro_source(&registry, Some("Game Boy"));
    match selection {
        NoIntroSourceSelection::Selected(imported) => {
            assert_eq!(imported.system_name, "Nintendo - Game Boy");
            assert_eq!(imported.entry_count, 1);
        }
        other => panic!("expected a single selected No-Intro source, got {other:?}"),
    }
}

#[test]
fn disabled_no_intro_source_is_not_a_candidate() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
    let mut registry = DatSourceRegistry::new();
    let mut entry = file_entry("gb-no-intro", path, Some("Game Boy"));
    entry.enabled = false;
    registry.add(entry).unwrap();

    let selection = select_no_intro_source(&registry, Some("Game Boy"));
    assert!(matches!(selection, NoIntroSourceSelection::NotImported));
}

#[test]
fn source_assigned_to_a_different_platform_is_not_a_candidate() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
    let mut registry = DatSourceRegistry::new();
    registry
        .add(file_entry("gb-no-intro", path, Some("NES")))
        .unwrap();

    let selection = select_no_intro_source(&registry, Some("Game Boy"));
    assert!(matches!(selection, NoIntroSourceSelection::NotImported));
}

#[test]
fn multiple_matching_no_intro_sources_are_reported_as_ambiguous_not_picked() {
    let dir = tempdir().unwrap();
    let path_a = write_dat(dir.path(), "gb-a.dat", GB_NO_INTRO_XML);
    let path_b = write_dat(dir.path(), "gb-b.dat", GB_NO_INTRO_XML_OTHER);
    let mut registry = DatSourceRegistry::new();
    registry
        .add(file_entry("gb-a", path_a.clone(), Some("Game Boy")))
        .unwrap();
    registry
        .add(file_entry("gb-b", path_b.clone(), Some("Game Boy")))
        .unwrap();

    let selection = select_no_intro_source(&registry, Some("Game Boy"));
    match &selection {
        NoIntroSourceSelection::Ambiguous(labels) => {
            assert_eq!(labels.len(), 2);
            let ids: Vec<&str> = labels
                .iter()
                .map(|label| label.source_id.as_str())
                .collect();
            assert_eq!(ids, ["gb-a", "gb-b"]);
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }

    // Deterministic: asking again returns the same ambiguous set in the same
    // order, never an arbitrary first pick.
    let selection_again = select_no_intro_source(&registry, Some("Game Boy"));
    match (selection, selection_again) {
        (NoIntroSourceSelection::Ambiguous(a), NoIntroSourceSelection::Ambiguous(b)) => {
            assert_eq!(a, b);
        }
        _ => panic!("expected Ambiguous both times"),
    }
}

#[test]
fn fingerprint_changes_when_a_relevant_source_file_changes_on_disk() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
    let mut registry = DatSourceRegistry::new();
    registry
        .add(file_entry("gb-no-intro", path.clone(), Some("Game Boy")))
        .unwrap();

    let before = no_intro_selection_fingerprint(&registry, Some("Game Boy"));
    let before_selection = select_no_intro_source(&registry, Some("Game Boy"));
    assert!(matches!(
        before_selection,
        NoIntroSourceSelection::Selected(_)
    ));

    // Simulate the DAT file being replaced with a newer one on disk, without
    // the registry entry itself changing - a "stale evidence" case.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&path, GB_NO_INTRO_XML_OTHER).unwrap();

    let after = no_intro_selection_fingerprint(&registry, Some("Game Boy"));
    assert_ne!(
        before, after,
        "fingerprint must change when the source file changes"
    );

    let after_selection = select_no_intro_source(&registry, Some("Game Boy"));
    match after_selection {
        NoIntroSourceSelection::Selected(imported) => {
            assert_eq!(imported.system_name, "Nintendo - Game Boy (Rebuild)");
        }
        other => panic!("expected a re-resolved single source, got {other:?}"),
    }
}

#[test]
fn fingerprint_changes_when_a_source_is_disabled() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
    let mut registry = DatSourceRegistry::new();
    registry
        .add(file_entry("gb-no-intro", path, Some("Game Boy")))
        .unwrap();

    let before = no_intro_selection_fingerprint(&registry, Some("Game Boy"));
    registry.get_mut("gb-no-intro").unwrap().enabled = false;
    let after = no_intro_selection_fingerprint(&registry, Some("Game Boy"));

    assert_ne!(before, after);
}

#[test]
fn unchanged_registry_yields_a_stable_fingerprint() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
    let mut registry = DatSourceRegistry::new();
    registry
        .add(file_entry("gb-no-intro", path, Some("Game Boy")))
        .unwrap();

    let first = no_intro_selection_fingerprint(&registry, Some("Game Boy"));
    let second = no_intro_selection_fingerprint(&registry, Some("Game Boy"));
    assert_eq!(first, second);
}
