//! Disposable fixtures only: every catalogue and settings file lives in a
//! temporary directory.

use std::path::{Path, PathBuf};

use super::*;
use crate::dat::limits::DatLimits;
use crate::dat::sources::config::{
    load_dat_sources_config_from, load_dat_sources_config_with_revision, save_dat_sources_config_to,
};
use crate::dat::sources::validation::validate_dat_source;
use crate::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};

fn dat(header: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<datafile>
    <header>
        <name>{header}</name>
        <version>20260101-000000</version>
        <author>No-Intro</author>
    </header>
    <game name="Game (World)">
        <rom name="game.bin" size="4" crc="0c7e7fd8" md5="098f6bcd4621d373cade4e832627b4f6" sha1="a94a8fe5ccb19ba61c4c0873d391e987982fbbd3"/>
    </game>
</datafile>"#
    )
}

struct Setup {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Setup {
    /// Registers one file catalogue per `(id, file name, header)` and
    /// validates each, exactly as the DAT page does, then saves.
    fn new(catalogues: &[(&str, &str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = DatSourceRegistry::new();
        for (id, file, header) in catalogues {
            let path = dir.path().join(file);
            std::fs::write(&path, dat(header)).unwrap();
            let mut entry =
                DatSourceEntry::new(id.to_string(), file.to_string(), path, DatSourceKind::File);
            validate_into(&mut entry);
            registry.add(entry).unwrap();
        }
        let config = dir.path().join("dat_sources.toml");
        save_dat_sources_config_to(&config, &registry.to_config()).unwrap();
        Self { dir, config }
    }

    fn registry(&self) -> DatSourceRegistry {
        DatSourceRegistry::from_config(&load_dat_sources_config_from(&self.config).unwrap()).0
    }

    fn revision(&self) -> DatSourcesRevision {
        load_dat_sources_config_with_revision(&self.config)
            .unwrap()
            .1
    }

    fn edit(&self, change: impl FnOnce(&mut DatSourceRegistry)) {
        let mut registry = self.registry();
        change(&mut registry);
        save_dat_sources_config_to(&self.config, &registry.to_config()).unwrap();
    }

    fn path(&self, file: &str) -> PathBuf {
        self.dir.path().join(file)
    }
}

fn validate_into(entry: &mut DatSourceEntry) {
    let (report, _) = validate_dat_source(entry, DatLimits::default());
    entry.health = report.to_health(&entry.path, entry.kind);
}

fn platforms(registry: &DatSourceRegistry) -> Vec<(String, Option<String>)> {
    registry
        .entries()
        .iter()
        .map(|entry| (entry.id.clone(), entry.platform.clone()))
        .collect()
}

fn group_of(registry: &DatSourceRegistry, id: &str) -> RecognitionGroup {
    recognition_rows(registry)
        .into_iter()
        .find(|row| row.source_id == id)
        .unwrap()
        .group
}

fn touch(path: &Path) {
    let text = std::fs::read_to_string(path).unwrap();
    std::fs::write(path, text.replace("Game (World)", "Other Game (World)")).unwrap();
}

#[test]
fn validation_records_the_header_and_saving_keeps_it() {
    let setup = Setup::new(&[("gbc", "renamed.dat", "Nintendo - Game Boy Color")]);
    let registry = setup.registry();
    let header = registry.entries()[0].health.current_header().unwrap();
    assert_eq!(header.name.as_deref(), Some("Nintendo - Game Boy Color"));
    assert_eq!(
        recognition_state(&registry.entries()[0]),
        RecognitionState::Recognised(CatalogueRecognition::Games {
            platform: "Game Boy Color"
        })
    );
}

#[test]
fn validation_alone_never_assigns_a_system() {
    let setup = Setup::new(&[
        ("gbc", "gbc.dat", "Nintendo - Game Boy Color"),
        ("gba", "gba.dat", "Nintendo - Game Boy Advance"),
    ]);
    assert!(
        platforms(&setup.registry())
            .iter()
            .all(|(_, platform)| platform.is_none())
    );
}

#[test]
fn the_file_name_is_never_used() {
    // The file is named like a Game Boy Color list but its header is not.
    let setup = Setup::new(&[(
        "x",
        "Nintendo - Game Boy Color (20260101).dat",
        "Source Code - Nintendo - Game Boy Color",
    )]);
    let registry = setup.registry();
    assert!(safe_matches(&registry).is_empty());
    assert_eq!(group_of(&registry, "x"), RecognitionGroup::CannotUse);
}

#[test]
fn the_summary_groups_every_kind_of_catalogue() {
    let setup = Setup::new(&[
        ("safe", "a.dat", "Nintendo - Game Boy Color"),
        ("extra", "b.dat", "Nintendo - Game Boy Advance (e-Reader)"),
        ("ambiguous", "c.dat", "IBM - PC and Compatibles"),
        ("source", "d.dat", "Source Code - Nintendo - Game Boy Color"),
        ("unsupported", "e.dat", "Casio - Loopy"),
        ("chosen", "f.dat", "Nintendo - Game Boy"),
        ("conflict", "g.dat", "Sega - Game Gear"),
        ("off", "h.dat", "Nintendo - Virtual Boy"),
    ]);
    setup.edit(|registry| {
        registry.get_mut("chosen").unwrap().platform = Some("Game Boy".into());
        registry.get_mut("conflict").unwrap().platform = Some("MasterSystem".into());
        registry.get_mut("off").unwrap().enabled = false;
    });
    let registry = setup.registry();
    for (id, group) in [
        ("safe", RecognitionGroup::SafeMatch),
        ("extra", RecognitionGroup::Extras),
        ("ambiguous", RecognitionGroup::Ambiguous),
        ("source", RecognitionGroup::CannotUse),
        ("unsupported", RecognitionGroup::CannotUse),
        ("chosen", RecognitionGroup::AlreadySet),
        ("conflict", RecognitionGroup::Conflict),
        ("off", RecognitionGroup::TurnedOff),
    ] {
        assert_eq!(group_of(&registry, id), group, "{id}");
    }
    let matches = safe_matches(&registry);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].source_id, "safe");
    assert_eq!(matches[0].platform, "Game Boy Color");
}

#[test]
fn apply_sets_only_the_previewed_safe_matches() {
    let setup = Setup::new(&[
        ("gbc", "gbc.dat", "Nintendo - Game Boy Color"),
        ("gba", "gba.dat", "Nintendo - Game Boy Advance"),
        ("extra", "e.dat", "Nintendo - Game Boy Advance (e-Reader)"),
        ("conflict", "c.dat", "Sega - Game Gear"),
    ]);
    setup.edit(|registry| {
        registry.get_mut("conflict").unwrap().platform = Some("MasterSystem".into());
    });
    let before = setup.registry();
    let preview = safe_matches(&before);
    assert_eq!(preview.len(), 2);
    let applied = apply_safe_matches(
        &setup.config,
        &setup.revision(),
        &setup.registry(),
        &preview,
    )
    .unwrap();
    assert_eq!(applied.applied, preview);
    let after = setup.registry();
    assert_eq!(
        platforms(&after),
        vec![
            ("conflict".into(), Some("MasterSystem".into())),
            ("extra".into(), None),
            ("gba".into(), Some("Game Boy Advance".into())),
            ("gbc".into(), Some("Game Boy Color".into())),
        ]
    );
    assert_eq!(applied.revision, setup.revision());
    // Nothing else about any catalogue changed: no catalogue was enabled or
    // disabled and the validation records are untouched.
    assert_eq!(before.entries().len(), after.entries().len());
    for (old, new) in before.entries().iter().zip(after.entries()) {
        assert_eq!(old.id, new.id);
        assert_eq!(old.enabled, new.enabled);
        assert_eq!(old.health, new.health);
        assert_eq!(old.path, new.path);
    }
    let row = recognition_rows(&after)
        .into_iter()
        .find(|row| row.source_id == "gbc")
        .unwrap();
    assert!(row.assigned_by_recognition);
    assert_eq!(row.group, RecognitionGroup::AlreadySet);
}

#[test]
fn apply_refuses_when_the_settings_changed_after_the_preview() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    let revision = setup.revision();
    let preview = safe_matches(&setup.registry());
    // Another window renames a catalogue in the meantime.
    setup.edit(|registry| registry.get_mut("gbc").unwrap().display_name = "Renamed".into());
    let saved = std::fs::read(&setup.config).unwrap();
    let refusal =
        apply_safe_matches(&setup.config, &revision, &setup.registry(), &preview).unwrap_err();
    assert!(matches!(
        refusal,
        ApplyRefusal::Settings(DatSourcesSaveError::ChangedElsewhere)
    ));
    assert_eq!(std::fs::read(&setup.config).unwrap(), saved);
}

#[test]
fn apply_refuses_when_a_catalogue_file_changed_after_its_check() {
    let setup = Setup::new(&[
        ("gbc", "gbc.dat", "Nintendo - Game Boy Color"),
        ("gba", "gba.dat", "Nintendo - Game Boy Advance"),
    ]);
    let preview = safe_matches(&setup.registry());
    std::thread::sleep(std::time::Duration::from_millis(20));
    touch(&setup.path("gba.dat"));
    let saved = std::fs::read(&setup.config).unwrap();
    let refusal = apply_safe_matches(
        &setup.config,
        &setup.revision(),
        &setup.registry(),
        &preview,
    )
    .unwrap_err();
    assert!(
        matches!(&refusal, ApplyRefusal::Stale { display_name, .. } if display_name == "gba.dat"),
        "{refusal:?}"
    );
    // All or nothing: the unchanged catalogue was not set either.
    assert_eq!(std::fs::read(&setup.config).unwrap(), saved);
}

#[test]
fn apply_refuses_when_a_catalogue_file_went_missing() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    let preview = safe_matches(&setup.registry());
    std::fs::remove_file(setup.path("gbc.dat")).unwrap();
    assert!(matches!(
        apply_safe_matches(
            &setup.config,
            &setup.revision(),
            &setup.registry(),
            &preview
        )
        .unwrap_err(),
        ApplyRefusal::Stale { .. }
    ));
    assert!(platforms(&setup.registry())[0].1.is_none());
}

#[test]
fn apply_never_overrides_a_system_someone_chose_after_the_preview() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    let preview = safe_matches(&setup.registry());
    setup.edit(|registry| registry.get_mut("gbc").unwrap().platform = Some("Game Boy".into()));
    // Even with the new revision, the previewed catalogue is no longer
    // unassigned, so nothing is applied.
    let refusal = apply_safe_matches(
        &setup.config,
        &setup.revision(),
        &setup.registry(),
        &preview,
    )
    .unwrap_err();
    assert!(matches!(refusal, ApplyRefusal::Stale { .. }), "{refusal:?}");
    assert_eq!(
        platforms(&setup.registry())[0].1.as_deref(),
        Some("Game Boy")
    );
}

#[test]
fn apply_refuses_a_preview_entry_that_is_not_a_safe_match() {
    let setup = Setup::new(&[("extra", "e.dat", "Nintendo - Game Boy Advance (e-Reader)")]);
    let registry = setup.registry();
    let header = registry.entries()[0].health.current_header().unwrap();
    let forged = SafeMatch {
        source_id: "extra".into(),
        display_name: "e.dat".into(),
        platform: "Game Boy Advance",
        header_name: header.name.clone().unwrap(),
        observed: header.observed.clone(),
    };
    assert!(matches!(
        apply_safe_matches(
            &setup.config,
            &setup.revision(),
            &setup.registry(),
            &[forged]
        )
        .unwrap_err(),
        ApplyRefusal::Stale { .. }
    ));
    assert!(platforms(&setup.registry())[0].1.is_none());
}

#[test]
fn an_unsaved_check_is_saved_together_with_the_system() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    // The saved setup has never been checked by this version.
    setup.edit(|registry| registry.get_mut("gbc").unwrap().health.header = None);
    let saved = setup.registry();
    assert!(safe_matches(&saved).is_empty());
    // The page checks the catalogue but has not saved the result yet.
    let mut checked = saved.clone();
    validate_into(checked.get_mut("gbc").unwrap());
    let preview = safe_matches(&checked);
    assert_eq!(preview.len(), 1);
    apply_safe_matches(&setup.config, &setup.revision(), &checked, &preview).unwrap();
    let after = setup.registry();
    assert_eq!(
        after.entries()[0].platform.as_deref(),
        Some("Game Boy Color")
    );
    assert!(after.entries()[0].health.current_header().is_some());
}

#[test]
fn a_check_of_a_different_file_is_refused() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    let mut checked = setup.registry();
    let other = setup.path("other.dat");
    std::fs::write(&other, dat("Nintendo - Game Boy Color")).unwrap();
    let entry = checked.get_mut("gbc").unwrap();
    entry.path = other;
    validate_into(entry);
    let preview = safe_matches(&checked);
    assert!(matches!(
        apply_safe_matches(&setup.config, &setup.revision(), &checked, &preview).unwrap_err(),
        ApplyRefusal::Stale { .. }
    ));
}

#[test]
fn an_empty_preview_applies_nothing() {
    let setup = Setup::new(&[("x", "x.dat", "Casio - Loopy")]);
    let saved = std::fs::read(&setup.config).unwrap();
    assert!(matches!(
        apply_safe_matches(&setup.config, &setup.revision(), &setup.registry(), &[]).unwrap_err(),
        ApplyRefusal::NothingToApply
    ));
    assert_eq!(std::fs::read(&setup.config).unwrap(), saved);
}

#[test]
fn a_changed_catalogue_needs_a_new_check_and_keeps_its_applied_system() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    let preview = safe_matches(&setup.registry());
    apply_safe_matches(
        &setup.config,
        &setup.revision(),
        &setup.registry(),
        &preview,
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    touch(&setup.path("gbc.dat"));
    let registry = setup.registry();
    let row = &recognition_rows(&registry)[0];
    assert_eq!(row.state, RecognitionState::Changed);
    // The setting is not erased, but it is no longer shown as backed by the
    // header until the catalogue is checked again.
    assert_eq!(row.assigned.as_deref(), Some("Nintendo Game Boy Color"));
    assert!(!row.assigned_by_recognition);
}

#[test]
fn a_new_header_that_disagrees_becomes_a_conflict_and_is_not_changed() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    let preview = safe_matches(&setup.registry());
    apply_safe_matches(
        &setup.config,
        &setup.revision(),
        &setup.registry(),
        &preview,
    )
    .unwrap();
    std::fs::write(setup.path("gbc.dat"), dat("Sega - Game Gear")).unwrap();
    setup.edit(|registry| validate_into(registry.get_mut("gbc").unwrap()));
    let registry = setup.registry();
    assert_eq!(group_of(&registry, "gbc"), RecognitionGroup::Conflict);
    assert!(safe_matches(&registry).is_empty());
    assert_eq!(platforms(&registry)[0].1.as_deref(), Some("Game Boy Color"));
}

#[test]
fn an_older_build_revalidation_cannot_pass_off_an_old_header() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    // An older build keeps the header keys but rewrites the observation.
    setup.edit(|registry| {
        registry.get_mut("gbc").unwrap().health.observed_size_bytes = Some(1);
    });
    let registry = setup.registry();
    assert!(registry.entries()[0].health.current_header().is_none());
    assert_eq!(group_of(&registry, "gbc"), RecognitionGroup::NeedsCheck);
}

#[test]
fn folders_and_failed_checks_are_never_matches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.dat"), dat("Nintendo - Game Boy Color")).unwrap();
    let mut folder = DatSourceEntry::new(
        "folder".into(),
        "folder".into(),
        dir.path().to_path_buf(),
        DatSourceKind::Folder,
    );
    validate_into(&mut folder);
    assert!(folder.health.header.is_none());
    let broken_path = dir.path().join("broken.dat");
    std::fs::write(&broken_path, "not a dat").unwrap();
    let mut broken = DatSourceEntry::new(
        "broken".into(),
        "broken".into(),
        broken_path,
        DatSourceKind::File,
    );
    validate_into(&mut broken);
    let mut registry = DatSourceRegistry::new();
    registry.add(folder).unwrap();
    registry.add(broken).unwrap();
    assert!(safe_matches(&registry).is_empty());
    assert_eq!(group_of(&registry, "folder"), RecognitionGroup::CannotUse);
    assert_eq!(group_of(&registry, "broken"), RecognitionGroup::NeedsCheck);
}

#[test]
fn undo_clears_only_what_the_apply_set() {
    let setup = Setup::new(&[
        ("gbc", "gbc.dat", "Nintendo - Game Boy Color"),
        ("gba", "gba.dat", "Nintendo - Game Boy Advance"),
    ]);
    let preview = safe_matches(&setup.registry());
    let applied = apply_safe_matches(
        &setup.config,
        &setup.revision(),
        &setup.registry(),
        &preview,
    )
    .unwrap();
    // The person then chooses a different system for one of them.
    setup.edit(|registry| {
        let entry = registry.get_mut("gba").unwrap();
        entry.platform = Some("Game Boy".into());
        entry.platform_recognised_from = None;
    });
    undo_applied_matches(&setup.config, &setup.revision(), &applied.applied).unwrap();
    assert_eq!(
        platforms(&setup.registry()),
        vec![
            ("gba".into(), Some("Game Boy".into())),
            ("gbc".into(), None),
        ]
    );
}

#[test]
fn unknown_settings_keys_survive_an_apply() {
    let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
    let text = std::fs::read_to_string(&setup.config).unwrap();
    std::fs::write(
        &setup.config,
        text.replacen("[[sources]]\n", "[[sources]]\nfuture_key = \"kept\"\n", 1),
    )
    .unwrap();
    let preview = safe_matches(&setup.registry());
    apply_safe_matches(
        &setup.config,
        &setup.revision(),
        &setup.registry(),
        &preview,
    )
    .unwrap();
    assert!(
        std::fs::read_to_string(&setup.config)
            .unwrap()
            .contains("future_key = \"kept\"")
    );
}

mod conditional_save {
    use super::*;
    use crate::dat::sources::config::{DatSourcesConfig, save_dat_sources_config_if_unchanged};

    #[test]
    fn a_save_from_a_stale_read_is_refused_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dat_sources.toml");
        let (config, first) = load_dat_sources_config_with_revision(&path).unwrap();
        let second = save_dat_sources_config_if_unchanged(&path, &first, &config).unwrap();
        assert_ne!(first, second);
        let written = std::fs::read(&path).unwrap();
        // A page still holding the first revision cannot overwrite.
        assert!(matches!(
            save_dat_sources_config_if_unchanged(&path, &first, &DatSourcesConfig::default()),
            Err(DatSourcesSaveError::ChangedElsewhere)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), written);
        // The current revision saves.
        save_dat_sources_config_if_unchanged(&path, &second, &config).unwrap();
    }

    #[test]
    fn two_saves_from_the_same_read_cannot_both_win() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dat_sources.toml");
        save_dat_sources_config_to(&path, &DatSourcesConfig::default()).unwrap();
        let (_, revision) = load_dat_sources_config_with_revision(&path).unwrap();
        for index in 0..8 {
            std::fs::write(
                dir.path().join(format!("{index}.dat")),
                dat("Nintendo - Game Boy"),
            )
            .unwrap();
        }
        let results: Vec<bool> = (0..8)
            .map(|index| {
                let path = path.clone();
                let revision = revision.clone();
                let file = dir.path().join(format!("{index}.dat"));
                std::thread::spawn(move || {
                    let mut registry = DatSourceRegistry::new();
                    registry
                        .add(DatSourceEntry::new(
                            format!("s{index}"),
                            format!("Source {index}"),
                            file,
                            DatSourceKind::File,
                        ))
                        .unwrap();
                    save_dat_sources_config_if_unchanged(&path, &revision, &registry.to_config())
                        .is_ok()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|won| **won).count(), 1);
        let (config, _) = load_dat_sources_config_with_revision(&path).unwrap();
        assert_eq!(config.sources.unwrap_or_default().len(), 1);
    }
}

mod verification_compatibility {
    use super::*;
    use crate::dat::catalogue_selection::{
        CatalogueInventoryInputs, EvidenceValue, list_installed_catalogues,
    };

    fn platform_offered(registry: &DatSourceRegistry, root: &Path) -> EvidenceValue<String> {
        list_installed_catalogues(CatalogueInventoryInputs {
            local_registry: registry,
            managed_sources: &Default::default(),
            managed_root: root,
            limits: DatLimits::default(),
        })
        .into_iter()
        .find(|row| row.display_name == "gbc.dat")
        .unwrap()
        .platform
    }

    #[test]
    fn a_recognised_system_is_not_used_until_it_is_applied() {
        let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
        assert_eq!(
            platform_offered(&setup.registry(), setup.dir.path()),
            EvidenceValue::Unknown
        );
        let preview = safe_matches(&setup.registry());
        apply_safe_matches(
            &setup.config,
            &setup.revision(),
            &setup.registry(),
            &preview,
        )
        .unwrap();
        assert!(matches!(
            platform_offered(&setup.registry(), setup.dir.path()),
            EvidenceValue::Assigned(_)
        ));
    }

    #[test]
    fn applying_touches_only_the_settings_file() {
        let setup = Setup::new(&[("gbc", "gbc.dat", "Nintendo - Game Boy Color")]);
        let catalogue = std::fs::read(setup.path("gbc.dat")).unwrap();
        let before: Vec<_> = std::fs::read_dir(setup.dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let preview = safe_matches(&setup.registry());
        apply_safe_matches(
            &setup.config,
            &setup.revision(),
            &setup.registry(),
            &preview,
        )
        .unwrap();
        assert_eq!(std::fs::read(setup.path("gbc.dat")).unwrap(), catalogue);
        let mut after: Vec<_> = std::fs::read_dir(setup.dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        // The only new file is the settings lock; no database or anything else.
        after.retain(|name| !before.contains(name));
        assert_eq!(
            after,
            vec![std::ffi::OsString::from("dat_sources.toml.lock")]
        );
    }
}
