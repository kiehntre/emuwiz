use std::path::PathBuf;

use tempfile::tempdir;

use super::*;
use crate::emulator_environment::HostReadOnlyFilesystem;
use crate::emulator_environment::es_de::{
    DiscoveryEnvironment, EsDeProfile, ExplicitRoot, discover_es_de_environment,
};
use crate::playing_library::model::{
    DestinationConflict, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPlan, PlayingLibraryPolicy,
};

const MINIMAL_PSX_SYSTEMS_XML: &str = r#"<systemList>
    <system>
        <name>psx</name>
        <fullname>Sony PlayStation</fullname>
        <path>%ROMPATH%/psx</path>
        <extension>.chd .cue</extension>
        <command>retroarch %ROM%</command>
        <platform>psx</platform>
        <theme>psx</theme>
    </system>
</systemList>"#;

/// Builds a real `EsDeProfile` (via the actual discovery pipeline, not a
/// hand-built literal) over a temporary `~/ES-DE`-shaped home directory
/// declaring one `psx` system through `custom_systems/es_systems.xml`.
fn profile_with_psx_system(home: &std::path::Path) -> EsDeProfile {
    std::fs::create_dir_all(home.join("custom_systems")).unwrap();
    std::fs::write(
        home.join("custom_systems/es_systems.xml"),
        MINIMAL_PSX_SYSTEMS_XML,
    )
    .unwrap();

    let environment = DiscoveryEnvironment {
        home: Some(std::ffi::OsString::from(
            "/nonexistent-home-not-used-by-explicit-profile",
        )),
        path: Some(std::ffi::OsString::from("")),
        explicit_bundled_systems_files: Vec::new(),
        appimage_search_roots: Vec::new(),
        explicit_root: Some(ExplicitRoot {
            home_directory: home.to_path_buf(),
            executable_path: None,
        }),
        explicit_appimages: Vec::new(),
        explicit_portables: Vec::new(),
    };
    let report = discover_es_de_environment(&HostReadOnlyFilesystem, &environment).unwrap();
    report
        .profiles
        .into_iter()
        .find(|profile| {
            profile
                .systems
                .iter()
                .any(|s| s.name.as_deref() == Some("psx"))
        })
        .expect("a profile with the psx system must be discovered")
}

fn plan_with_operations(
    destination_root: PathBuf,
    entries: &[(&str, PathBuf)],
) -> PlayingLibraryPlan {
    let operations: Vec<LinkedLibraryOperation> = entries
        .iter()
        .map(|(_, destination_path)| LinkedLibraryOperation {
            source_path: destination_path.clone(), // unused by this bridge
            destination_path: destination_path.clone(),
        })
        .collect();
    let elected_games = entries
        .iter()
        .zip(operations.iter())
        .map(|((name, _), operation)| ElectedGame {
            dat_entry_name: name.to_string(),
            family_root_name: name.to_string(),
            explanation: ElectionExplanation {
                steps: Vec::new(),
                rejected: Vec::new(),
            },
            launcher_operation: operation.clone(),
            companion_operations: Vec::new(),
        })
        .collect();
    PlayingLibraryPlan {
        destination_root,
        policy: PlayingLibraryPolicy::default(),
        archives_examined: entries.len(),
        families_examined: entries.len(),
        elected_games,
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: entries.len(),
        conflicts: Vec::new(),
        operations,
        rejected_launchers: Vec::new(),
    }
}

#[test]
fn verified_single_file_game_publishes_as_a_new_gamelist_entry() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );

    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert_eq!(publication.added.len(), 1);
    assert!(publication.already_present.is_empty());
    assert!(publication.previous_content.is_none());
    assert!(publication.new_content.contains("Game (Europe).chd"));
    assert!(!publication.is_unchanged());

    apply_es_de_gamelist_publication(&publication).unwrap();
    let on_disk = std::fs::read_to_string(&publication.gamelist_path).unwrap();
    assert_eq!(on_disk, publication.new_content);
    assert!(
        on_disk.contains(
            &destination_root
                .join("Game (Europe).chd")
                .to_string_lossy()
                .into_owned()
        )
    );
}

#[test]
fn unmapped_platform_is_refused_not_guessed() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/unknown");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game", destination_root.join("Game.zip"))],
    );

    let error = plan_es_de_gamelist_publication(&plan, "SomeUnknownPlatform", &profile)
        .expect_err("an unmapped platform must be refused");
    assert!(matches!(
        error,
        EsDePublicationError::PlatformUnmapped { .. }
    ));
}

#[test]
fn system_not_configured_in_the_profile_is_refused() {
    let home = tempdir().unwrap();
    // No custom_systems/es_systems.xml at all - "snes" is never discovered.
    std::fs::create_dir_all(home.path().join("custom_systems")).unwrap();
    let environment = DiscoveryEnvironment {
        home: Some(std::ffi::OsString::from(
            "/nonexistent-home-not-used-by-explicit-profile",
        )),
        path: Some(std::ffi::OsString::from("")),
        explicit_bundled_systems_files: Vec::new(),
        appimage_search_roots: Vec::new(),
        explicit_root: Some(ExplicitRoot {
            home_directory: home.path().to_path_buf(),
            executable_path: None,
        }),
        explicit_appimages: Vec::new(),
        explicit_portables: Vec::new(),
    };
    let report = discover_es_de_environment(&HostReadOnlyFilesystem, &environment).unwrap();
    let profile = report
        .profiles
        .into_iter()
        .find(|profile| {
            matches!(
                profile.profile_kind,
                crate::emulator_environment::es_de::ProfileKind::Explicit
            )
        })
        .unwrap();

    let destination_root = home.path().join("playing/snes");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game", destination_root.join("Game.zip"))],
    );
    let error = plan_es_de_gamelist_publication(&plan, "SNES", &profile)
        .expect_err("a system absent from the profile must be refused");
    assert!(matches!(
        error,
        EsDePublicationError::SystemNotConfigured { .. }
    ));
}

#[test]
fn existing_gamelist_entries_are_preserved_byte_for_byte() {
    let home = tempdir().unwrap();
    let gamelist_dir = home.path().join("gamelists/psx");
    std::fs::create_dir_all(&gamelist_dir).unwrap();
    let existing = "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>/library/Other Game.chd</path>\n\t\t<name>Other Game</name>\n\t\t<desc>A user-written description that must survive untouched.</desc>\n\t</game>\n</gameList>\n";
    std::fs::write(gamelist_dir.join("gamelist.xml"), existing).unwrap();
    let profile = profile_with_psx_system(home.path());

    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert_eq!(publication.previous_content.as_deref(), Some(existing));
    assert!(publication.new_content.starts_with(
        "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>/library/Other Game.chd</path>"
    ));
    assert!(
        publication
            .new_content
            .contains("A user-written description that must survive untouched.")
    );
    assert!(publication.new_content.contains("Game (Europe).chd"));
    assert!(publication.new_content.ends_with("</gameList>\n"));

    apply_es_de_gamelist_publication(&publication).unwrap();
    let on_disk = std::fs::read_to_string(gamelist_dir.join("gamelist.xml")).unwrap();
    assert!(on_disk.contains("A user-written description that must survive untouched."));
}

#[test]
fn reapplying_the_same_plan_is_unchanged() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );

    let first = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    apply_es_de_gamelist_publication(&first).unwrap();

    let second = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert!(second.is_unchanged());
    assert_eq!(second.already_present.len(), 1);
    assert!(second.added.is_empty());
    assert_eq!(second.new_content, second.previous_content.clone().unwrap());
}

#[test]
fn rollback_restores_the_previous_content_exactly() {
    let home = tempdir().unwrap();
    let gamelist_dir = home.path().join("gamelists/psx");
    std::fs::create_dir_all(&gamelist_dir).unwrap();
    let existing = "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>/library/Other Game.chd</path>\n\t\t<name>Other Game</name>\n\t</game>\n</gameList>\n";
    std::fs::write(gamelist_dir.join("gamelist.xml"), existing).unwrap();
    let profile = profile_with_psx_system(home.path());

    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    apply_es_de_gamelist_publication(&publication).unwrap();
    assert!(
        std::fs::read_to_string(gamelist_dir.join("gamelist.xml"))
            .unwrap()
            .contains("Game (Europe).chd")
    );

    rollback_es_de_gamelist_publication(&publication).unwrap();
    let restored = std::fs::read_to_string(gamelist_dir.join("gamelist.xml")).unwrap();
    assert_eq!(restored, existing);
}

#[test]
fn rollback_of_a_freshly_created_gamelist_removes_only_that_file() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    apply_es_de_gamelist_publication(&publication).unwrap();
    assert!(publication.gamelist_path.is_file());

    rollback_es_de_gamelist_publication(&publication).unwrap();
    assert!(!publication.gamelist_path.exists());
}

#[test]
fn malformed_existing_gamelist_fails_closed() {
    let home = tempdir().unwrap();
    let gamelist_dir = home.path().join("gamelists/psx");
    std::fs::create_dir_all(&gamelist_dir).unwrap();
    std::fs::write(
        gamelist_dir.join("gamelist.xml"),
        "<gameList><game><name>no closing tag at all",
    )
    .unwrap();
    let profile = profile_with_psx_system(home.path());

    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let error = plan_es_de_gamelist_publication(&plan, "PSX", &profile)
        .expect_err("a gamelist with no closing tag must fail closed");
    assert!(matches!(
        error,
        EsDePublicationError::MalformedGamelist { .. }
    ));
}

#[test]
fn destination_not_in_operations_is_never_published() {
    // An elected game whose operation was dropped by conflict filtering
    // (present in `elected_games` but absent from `operations`) must never
    // be published - mirrors the same rule the symlink apply already
    // enforces. A second, unconflicted election in the same plan must
    // still be published normally.
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let conflicted_operation = LinkedLibraryOperation {
        source_path: destination_root.join("Conflicted Game.chd"),
        destination_path: destination_root.join("Conflicted Game.chd"),
    };
    let clean_operation = LinkedLibraryOperation {
        source_path: destination_root.join("Clean Game.chd"),
        destination_path: destination_root.join("Clean Game.chd"),
    };
    let plan = PlayingLibraryPlan {
        destination_root: destination_root.clone(),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: 2,
        families_examined: 2,
        elected_games: vec![
            ElectedGame {
                dat_entry_name: "Conflicted Game".to_string(),
                family_root_name: "Conflicted Game".to_string(),
                explanation: ElectionExplanation {
                    steps: Vec::new(),
                    rejected: Vec::new(),
                },
                launcher_operation: conflicted_operation.clone(),
                companion_operations: Vec::new(),
            },
            ElectedGame {
                dat_entry_name: "Clean Game".to_string(),
                family_root_name: "Clean Game".to_string(),
                explanation: ElectionExplanation {
                    steps: Vec::new(),
                    rejected: Vec::new(),
                },
                launcher_operation: clean_operation.clone(),
                companion_operations: Vec::new(),
            },
        ],
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: 1,
        conflicts: vec![DestinationConflict {
            destination_basename: "conflicted game.chd".to_string(),
            contenders: vec!["Conflicted Game".to_string()],
            destinations: vec![conflicted_operation.destination_path.clone()],
        }],
        operations: vec![clean_operation],
        rejected_launchers: Vec::new(),
    };

    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert_eq!(publication.added.len(), 1);
    assert_eq!(publication.added[0].dat_entry_name, "Clean Game");
    assert!(!publication.new_content.contains("Conflicted Game.chd"));
}

#[test]
fn recovery_record_is_written_before_and_removed_after_a_successful_apply() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    let recovery_path = es_de_gamelist_recovery_path(&publication.gamelist_path);
    assert!(!recovery_path.is_file());

    apply_es_de_gamelist_publication(&publication).unwrap();

    // Requirement: successful finalization must not leave ambiguous stale
    // recovery state.
    assert!(!recovery_path.is_file());
    assert!(!has_unresolved_es_de_gamelist_recovery(
        &publication.gamelist_path
    ));
}

#[test]
fn simulated_restart_recovers_the_exact_prior_bytes_from_nothing_but_the_path() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let gamelist_dir = home.path().join("gamelists/psx");
    std::fs::create_dir_all(&gamelist_dir).unwrap();
    let existing = "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>/library/Other Game.chd</path>\n\t\t<name>Other Game</name>\n\t</game>\n</gameList>\n";
    std::fs::write(gamelist_dir.join("gamelist.xml"), existing).unwrap();

    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    let gamelist_path = publication.gamelist_path.clone();

    // Manually reproduce exactly what `apply_es_de_gamelist_publication`
    // does up to (but not including) its own final cleanup step - this is
    // the "crashed after the real write, before recovery-record removal"
    // case, the hardest one to resolve correctly.
    write_recovery_record(
        &es_de_gamelist_recovery_path(&gamelist_path),
        &gamelist_path,
        &publication.previous_content,
    )
    .unwrap();
    std::fs::write(&gamelist_path, &publication.new_content).unwrap();
    assert!(
        std::fs::read_to_string(&gamelist_path)
            .unwrap()
            .contains("Game (Europe).chd")
    );

    // Simulate a full process restart: drop every in-memory value (`plan`,
    // `publication`, `profile`) and recover from nothing but the path.
    drop(publication);
    drop(profile);
    drop(plan);

    assert!(has_unresolved_es_de_gamelist_recovery(&gamelist_path));
    recover_es_de_gamelist_publication(&gamelist_path).unwrap();

    let restored = std::fs::read_to_string(&gamelist_path).unwrap();
    assert_eq!(restored, existing);
    assert!(!has_unresolved_es_de_gamelist_recovery(&gamelist_path));
}

#[test]
fn simulated_restart_recovers_a_freshly_created_gamelist_by_removing_it() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    let gamelist_path = publication.gamelist_path.clone();
    assert!(publication.previous_content.is_none());

    write_recovery_record(
        &es_de_gamelist_recovery_path(&gamelist_path),
        &gamelist_path,
        &publication.previous_content,
    )
    .unwrap();
    std::fs::write(&gamelist_path, &publication.new_content).unwrap();
    drop(publication);

    recover_es_de_gamelist_publication(&gamelist_path).unwrap();
    assert!(!gamelist_path.exists());
    assert!(!has_unresolved_es_de_gamelist_recovery(&gamelist_path));
}

#[test]
fn an_unresolved_recovery_record_refuses_a_new_publication() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    let gamelist_path = publication.gamelist_path.clone();
    write_recovery_record(
        &es_de_gamelist_recovery_path(&gamelist_path),
        &gamelist_path,
        &publication.previous_content,
    )
    .unwrap();

    let error = plan_es_de_gamelist_publication(&plan, "PSX", &profile)
        .expect_err("planning must refuse while a recovery record is unresolved");
    assert!(matches!(
        error,
        EsDePublicationError::UnresolvedRecovery { .. }
    ));

    let error = apply_es_de_gamelist_publication(&publication)
        .expect_err("apply must also refuse while a recovery record is unresolved");
    assert!(matches!(
        error,
        EsDePublicationError::UnresolvedRecovery { .. }
    ));
}

#[test]
fn recovering_with_no_record_present_fails_closed() {
    let home = tempdir().unwrap();
    let gamelist_path = home.path().join("gamelists/psx/gamelist.xml");
    let error = recover_es_de_gamelist_publication(&gamelist_path)
        .expect_err("recovering with no record must fail closed");
    assert!(matches!(
        error,
        EsDePublicationError::NoRecoveryRecord { .. }
    ));
}

#[test]
fn a_corrupt_recovery_record_fails_closed_rather_than_being_discarded() {
    let home = tempdir().unwrap();
    let gamelist_path = home.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(gamelist_path.parent().unwrap()).unwrap();
    std::fs::write(
        es_de_gamelist_recovery_path(&gamelist_path),
        "not valid json at all",
    )
    .unwrap();

    let error = recover_es_de_gamelist_publication(&gamelist_path)
        .expect_err("a corrupt recovery record must fail closed");
    assert!(matches!(
        error,
        EsDePublicationError::RecoveryCorrupt { .. }
    ));
    // Refusing must not silently delete the one record able to restore
    // the gamelist's true prior state.
    assert!(es_de_gamelist_recovery_path(&gamelist_path).is_file());
}

#[test]
fn generated_xml_values_are_escaped() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let tricky_name = "Bill & Ted's <Excellent> \"Adventure\"";
    let tricky_path = destination_root.join("Bill & Ted's <Excellent> \"Adventure\".chd");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[(tricky_name, tricky_path.clone())],
    );

    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert_eq!(publication.added.len(), 1);
    // The raw, unescaped characters must never appear verbatim next to
    // the XML markup they would otherwise break.
    assert!(!publication.new_content.contains("<Excellent>"));
    assert!(publication.new_content.contains("&amp;"));
    assert!(publication.new_content.contains("&lt;Excellent&gt;"));
    assert!(publication.new_content.contains("&quot;Adventure&quot;"));

    // The escaped document must still be well-formed XML: quick_xml's own
    // reader must walk it without error from start to finish.
    let mut reader = quick_xml::Reader::from_str(&publication.new_content);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event().unwrap() {
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }

    // And it must decode back to exactly the original values - extracted
    // by exact tag delimiters (this content has no nested markup) rather
    // than a second hand-rolled event loop.
    let extract = |open: &str, close: &str| -> String {
        let start = publication.new_content.find(open).unwrap() + open.len();
        let end = publication.new_content[start..].find(close).unwrap() + start;
        quick_xml::escape::unescape(&publication.new_content[start..end])
            .unwrap()
            .into_owned()
    };
    assert_eq!(extract("<path>", "</path>"), tricky_path.to_string_lossy());
    assert_eq!(extract("<name>", "</name>"), tricky_name);
}

#[test]
fn duplicate_existing_path_entries_are_treated_as_already_present_not_duplicated_again() {
    let home = tempdir().unwrap();
    let gamelist_dir = home.path().join("gamelists/psx");
    std::fs::create_dir_all(&gamelist_dir).unwrap();
    let destination_root = home.path().join("playing/psx");
    let target = destination_root.join("Game (Europe).chd");
    let escaped = quick_xml::escape::escape(target.to_string_lossy().as_ref()).into_owned();
    // A malformed/duplicated pre-existing gamelist: the same <path> is
    // already listed twice.
    let existing = format!(
        "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>{escaped}</path>\n\t\t<name>Dup One</name>\n\t</game>\n\t<game>\n\t\t<path>{escaped}</path>\n\t\t<name>Dup Two</name>\n\t</game>\n</gameList>\n"
    );
    std::fs::write(gamelist_dir.join("gamelist.xml"), &existing).unwrap();
    let profile = profile_with_psx_system(home.path());

    let plan = plan_with_operations(destination_root.clone(), &[("Game (Europe)", target)]);
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();

    assert!(publication.added.is_empty());
    assert_eq!(publication.already_present.len(), 1);
    assert!(publication.is_unchanged());
    // The existing duplicate pair is preserved exactly; no third entry is
    // ever added.
    assert_eq!(publication.new_content, existing);
    assert_eq!(publication.new_content.matches("<game>").count(), 2);
}

#[test]
fn mismatched_recovery_record_path_fails_closed_and_cannot_touch_another_file() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    let gamelist_path = publication.gamelist_path.clone();

    // A completely unrelated, important-looking file elsewhere on disk.
    let victim = home.path().join("important_unrelated_file.txt");
    std::fs::write(&victim, "do not touch me").unwrap();

    // Write a recovery record AT the real gamelist's recovery path, but
    // whose own `gamelist_path` field names the victim instead - the
    // shape a tampered or corrupt record would have.
    write_recovery_record(
        &es_de_gamelist_recovery_path(&gamelist_path),
        &victim,
        &Some("some other content".to_string()),
    )
    .unwrap();

    let error = recover_es_de_gamelist_publication(&gamelist_path)
        .expect_err("a mismatched recovery record must fail closed");
    assert!(matches!(
        error,
        EsDePublicationError::RecoveryPathMismatch { .. }
    ));

    // Neither the victim file nor the gamelist itself was touched.
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "do not touch me");
    assert!(!gamelist_path.exists());
    // The record is still present - refusing must not discard the only
    // record able to restore the true prior state.
    assert!(has_unresolved_es_de_gamelist_recovery(&gamelist_path));
}

#[test]
fn oversized_recovery_record_is_refused_before_reading() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    let gamelist_path = publication.gamelist_path.clone();
    let recovery_path = es_de_gamelist_recovery_path(&gamelist_path);

    // A sparse file reporting a size past the bound - proves the bound is
    // enforced from metadata alone, never by attempting to allocate and
    // read a buffer this large.
    std::fs::create_dir_all(recovery_path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&recovery_path).unwrap();
    file.set_len(MAX_RECOVERY_RECORD_BYTES + 1).unwrap();
    drop(file);

    let error = recover_es_de_gamelist_publication(&gamelist_path)
        .expect_err("an oversized recovery record must be refused");
    assert!(matches!(
        error,
        EsDePublicationError::RecoveryTooLarge { .. }
    ));
}

#[test]
fn oversized_existing_gamelist_is_refused_before_reading() {
    let home = tempdir().unwrap();
    let gamelist_dir = home.path().join("gamelists/psx");
    std::fs::create_dir_all(&gamelist_dir).unwrap();
    let file = std::fs::File::create(gamelist_dir.join("gamelist.xml")).unwrap();
    file.set_len(MAX_GAMELIST_BYTES + 1).unwrap();
    drop(file);
    let profile = profile_with_psx_system(home.path());

    let destination_root = home.path().join("playing/psx");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Game (Europe)", destination_root.join("Game (Europe).chd"))],
    );
    let error = plan_es_de_gamelist_publication(&plan, "PSX", &profile)
        .expect_err("an oversized existing gamelist must be refused");
    assert!(matches!(
        error,
        EsDePublicationError::GamelistTooLarge { .. }
    ));
}

#[cfg(unix)]
#[test]
fn writing_the_recovery_record_through_a_symlinked_path_never_touches_its_target() {
    let home = tempdir().unwrap();
    let gamelist_path = home.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(gamelist_path.parent().unwrap()).unwrap();
    let recovery_path = es_de_gamelist_recovery_path(&gamelist_path);

    // An attacker-planted symlink sitting exactly at the recovery path,
    // pointing at a file well outside this system's own directory.
    let victim = home.path().join("victim.txt");
    std::fs::write(&victim, "original victim content").unwrap();
    std::os::unix::fs::symlink(&victim, &recovery_path).unwrap();
    assert!(recovery_path.is_symlink());

    write_recovery_record(
        &recovery_path,
        &gamelist_path,
        &Some("captured".to_string()),
    )
    .unwrap();

    // The victim file must be completely untouched - a write-through
    // would have overwritten it with the recovery JSON instead.
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "original victim content"
    );
    // The symlink itself was replaced by a real regular file: the write
    // landed on the directory entry, never followed through to the
    // target, per POSIX `rename(2)` semantics.
    let metadata = std::fs::symlink_metadata(&recovery_path).unwrap();
    assert!(!metadata.file_type().is_symlink());
    assert!(metadata.is_file());
    let record = read_recovery_record(&recovery_path, &gamelist_path).unwrap();
    assert_eq!(record.previous_content.as_deref(), Some("captured"));
}

#[test]
fn publication_points_at_the_launcher_file_never_a_companion() {
    let home = tempdir().unwrap();
    let profile = profile_with_psx_system(home.path());
    let destination_root = home.path().join("playing/psx");
    let launcher_operation = LinkedLibraryOperation {
        source_path: destination_root.join("Game (Europe).cue"),
        destination_path: destination_root.join("Game (Europe).cue"),
    };
    let companion_operations = vec![
        LinkedLibraryOperation {
            source_path: destination_root.join("track1.bin"),
            destination_path: destination_root.join("track1.bin"),
        },
        LinkedLibraryOperation {
            source_path: destination_root.join("track2.bin"),
            destination_path: destination_root.join("track2.bin"),
        },
    ];
    let mut operations = vec![launcher_operation.clone()];
    operations.extend(companion_operations.iter().cloned());
    let plan = PlayingLibraryPlan {
        destination_root: destination_root.clone(),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: 1,
        families_examined: 1,
        elected_games: vec![ElectedGame {
            dat_entry_name: "Game (Europe)".to_string(),
            family_root_name: "Game (Europe)".to_string(),
            explanation: ElectionExplanation {
                steps: Vec::new(),
                rejected: Vec::new(),
            },
            launcher_operation,
            companion_operations,
        }],
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: 1,
        conflicts: Vec::new(),
        operations,
        rejected_launchers: Vec::new(),
    };

    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert_eq!(publication.added.len(), 1);
    assert_eq!(
        publication.added[0].destination_path,
        destination_root.join("Game (Europe).cue")
    );
    assert!(publication.new_content.contains("Game (Europe).cue"));
    assert!(!publication.new_content.contains("track1.bin"));
    assert!(!publication.new_content.contains("track2.bin"));
}
