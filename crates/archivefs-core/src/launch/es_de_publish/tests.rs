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

fn minimal_systems_xml(system: &str, fullname: &str) -> String {
    format!(
        "<systemList>\n    <system>\n        <name>{system}</name>\n        <fullname>{fullname}</fullname>\n        <path>%ROMPATH%/{system}</path>\n        <extension>.zip .chd</extension>\n        <command>retroarch %ROM%</command>\n        <platform>{system}</platform>\n        <theme>{system}</theme>\n    </system>\n</systemList>"
    )
}

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
    profile_with_system(home, "psx", "Sony PlayStation", MINIMAL_PSX_SYSTEMS_XML)
}

fn profile_with_system(
    home: &std::path::Path,
    system: &str,
    fullname: &str,
    systems_xml: &str,
) -> EsDeProfile {
    std::fs::create_dir_all(home.join("custom_systems")).unwrap();
    std::fs::write(home.join("custom_systems/es_systems.xml"), systems_xml).unwrap();

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
            profile.systems.iter().any(|s| {
                s.name.as_deref() == Some(system) && s.fullname.as_deref() == Some(fullname)
            })
        })
        .expect("a profile with the requested system must be discovered")
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
                winner_evidence: crate::playing_library::CandidateEvidenceSummary::unknown(),
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
fn batch_two_publication_preview_uses_the_reviewed_arcade_target() {
    let home = tempdir().unwrap();
    let arcade_xml = minimal_systems_xml("arcade", "Arcade");
    let profile = profile_with_system(home.path(), "arcade", "Arcade", &arcade_xml);
    let destination_root = home.path().join("playing/arcade");
    let plan = plan_with_operations(
        destination_root.clone(),
        &[("Pac-Man", destination_root.join("pacman.zip"))],
    );

    let publication = plan_es_de_gamelist_publication(&plan, "Arcade", &profile).unwrap();
    assert_eq!(publication.es_de_system, "arcade");
    assert_eq!(
        publication.gamelist_path,
        home.path().join("gamelists/arcade/gamelist.xml")
    );
    assert_eq!(publication.added.len(), 1);
}

#[test]
fn batch_five_publication_previews_use_the_exact_reviewed_system_targets() {
    for (platform_id, system, fullname) in [
        ("VIC-20", "vic20", "Commodore VIC-20"),
        ("Neo Geo CD", "neogeocd", "SNK Neo Geo CD"),
        ("Switch", "switch", "Nintendo Switch"),
        ("PC-FX", "pcfx", "NEC PC-FX"),
        ("Philips CD-i", "cdimono1", "Philips CD-i"),
        ("PS4", "ps4", "Sony PlayStation 4"),
        ("Sharp X68000", "x68000", "Sharp X68000"),
    ] {
        let home = tempdir().unwrap();
        let systems_xml = minimal_systems_xml(system, fullname);
        let profile = profile_with_system(home.path(), system, fullname, &systems_xml);
        let destination_root = home.path().join("playing").join(system);
        let plan = plan_with_operations(
            destination_root.clone(),
            &[("Verified Game", destination_root.join("Verified Game.chd"))],
        );

        let publication = plan_es_de_gamelist_publication(&plan, platform_id, &profile)
            .unwrap_or_else(|error| panic!("{platform_id} preview was refused: {error}"));
        assert_eq!(publication.es_de_system, system);
        assert_eq!(
            publication.gamelist_path,
            home.path()
                .join("gamelists")
                .join(system)
                .join("gamelist.xml")
        );
        assert!(!publication.is_unchanged());
    }
}

#[test]
fn final_safe_gap_publication_previews_preserve_distinct_targets_and_pc98_equivalence() {
    for (platform_id, system, fullname) in [
        ("TurboGrafx-16", "tg16", "NEC TurboGrafx-16"),
        ("PC-98", "pc98", "NEC PC-9800 Series"),
        ("NEC PC-9801", "pc98", "NEC PC-9800 Series"),
    ] {
        let home = tempdir().unwrap();
        let systems_xml = minimal_systems_xml(system, fullname);
        let profile = profile_with_system(home.path(), system, fullname, &systems_xml);
        let destination_root = home.path().join("playing").join(platform_id);
        let plan = plan_with_operations(
            destination_root.clone(),
            &[("Verified Game", destination_root.join("Verified Game.chd"))],
        );

        let publication = plan_es_de_gamelist_publication(&plan, platform_id, &profile)
            .unwrap_or_else(|error| panic!("{platform_id} preview was refused: {error}"));
        assert_eq!(publication.es_de_system, system);
        assert_eq!(
            publication.gamelist_path,
            home.path()
                .join("gamelists")
                .join(system)
                .join("gamelist.xml")
        );
    }
}

#[test]
fn batch_4_publication_previews_use_each_verified_system_target() {
    for (platform_id, system, fullname) in [
        ("3DO", "3do", "3DO Interactive Multiplayer"),
        ("Acorn Archimedes", "archimedes", "Acorn Archimedes"),
        ("Acorn Electron", "electron", "Acorn Electron"),
        ("Amstrad CPC", "amstradcpc", "Amstrad CPC"),
        ("Apple II", "apple2", "Apple II"),
        ("BBC Micro", "bbcmicro", "Acorn Computers BBC Micro"),
        ("FM Towns", "fmtowns", "Fujitsu FM Towns"),
        ("Macintosh", "macintosh", "Apple Macintosh"),
        ("NEC PC-8801", "pc88", "NEC PC-8800 Series"),
        ("NGage", "ngage", "Nokia N-Gage"),
        ("PC Engine", "pcengine", "NEC PC Engine"),
        ("PC Engine CD", "pcenginecd", "NEC PC Engine CD"),
    ] {
        let home = tempdir().unwrap();
        let systems_xml = minimal_systems_xml(system, fullname);
        let profile = profile_with_system(home.path(), system, fullname, &systems_xml);
        let destination_root = home.path().join("playing").join(system);
        let plan = plan_with_operations(
            destination_root.clone(),
            &[("Game", destination_root.join("game.rom"))],
        );

        let publication = plan_es_de_gamelist_publication(&plan, platform_id, &profile)
            .unwrap_or_else(|error| panic!("{platform_id} preview failed: {error}"));
        assert_eq!(publication.es_de_system, system);
        assert_eq!(
            publication.gamelist_path,
            home.path()
                .join("gamelists")
                .join(system)
                .join("gamelist.xml")
        );
        assert_eq!(publication.added.len(), 1);
    }
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
                    winner_evidence: crate::playing_library::CandidateEvidenceSummary::unknown(),
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
                    winner_evidence: crate::playing_library::CandidateEvidenceSummary::unknown(),
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
    persist_record(&publication_record(&publication).unwrap()).unwrap();
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

    persist_record(&publication_record(&publication).unwrap()).unwrap();
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
        if let quick_xml::events::Event::Eof = reader.read_event().unwrap() {
            break;
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
    let recovery_dir = gamelist_path.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&recovery_dir).unwrap();
    let recovery_path = es_de_gamelist_recovery_path(&gamelist_path);

    // An attacker-planted symlink sitting exactly at the recovery path,
    // pointing at a file well outside this system's own directory.
    let victim = home.path().join("victim.txt");
    std::fs::write(&victim, "original victim content").unwrap();
    std::os::unix::fs::symlink(&victim, &recovery_path).unwrap();
    assert!(recovery_path.is_symlink());

    // The invariant: a symlink at the recovery-record path is *refused*,
    // fail-closed, before any mutation. `crate::atomic_write_text` reads the
    // final path component with `fs::symlink_metadata` (never followed) and
    // returns an error before it creates a temporary file or renames
    // anything - see the `es_de_publish` module doc comment's
    // "Recovery-record safety" section.
    let error = write_recovery_record(
        &recovery_path,
        &gamelist_path,
        &Some("captured".to_string()),
    )
    .expect_err("a symlinked recovery path must be refused, never written through");
    assert!(matches!(error, EsDePublicationError::Io { .. }));

    // The victim's bytes are byte-for-byte unchanged - nothing was written
    // through, or renamed over, the link.
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "original victim content"
    );
    // The planted symlink is left exactly as found: not removed, not
    // replaced, not overwritten. EmuWiz never disturbs an unsafe path it
    // refuses.
    let metadata = std::fs::symlink_metadata(&recovery_path).unwrap();
    assert!(metadata.file_type().is_symlink());
    assert_eq!(std::fs::read_link(&recovery_path).unwrap(), victim);

    // No orphan temporary or sidecar file is left in the parent directory by
    // the refused write - the only entry is the planted symlink itself.
    let leftovers: Vec<_> = std::fs::read_dir(&recovery_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        vec![recovery_path.file_name().unwrap().to_os_string()]
    );

    // The symlink is never read *through* either: the read path also refuses
    // it (`RecoveryCorrupt`, via `symlink_metadata`), so a planted link can
    // neither redirect a write nor smuggle content into a restore.
    let read_error = read_recovery_record(&recovery_path, &gamelist_path)
        .expect_err("a symlinked recovery path must never be read through");
    assert!(matches!(
        read_error,
        EsDePublicationError::RecoveryCorrupt { .. }
    ));
}

/// Pins the shared atomic writer's leaf-only boundary via the test-only legacy
/// receipt writer. Production publication never trusts profile discovery to
/// freeze ancestors: DestinationEvidence separately rejects parent symlinks.
#[cfg(unix)]
#[test]
fn a_symlinked_parent_directory_of_the_recovery_path_is_out_of_this_helper_s_scope() {
    let home = tempdir().unwrap();
    let real_dir = home.path().join("real/gamelists/psx");
    std::fs::create_dir_all(&real_dir).unwrap();
    let linked_parent = home.path().join("gamelists/psx");
    std::fs::create_dir_all(linked_parent.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real_dir, &linked_parent).unwrap();

    let gamelist_path = linked_parent.join("gamelist.xml");
    let recovery_path = es_de_gamelist_recovery_path(&gamelist_path);
    // This fixture writer bypasses production evidence checks deliberately.
    write_recovery_record(&recovery_path, &gamelist_path, &Some("x".to_string())).unwrap();
    assert!(real_dir.join(recovery_path.file_name().unwrap()).is_file());
}

#[test]
fn an_owned_recovery_record_round_trips_and_cannot_be_replaced() {
    let (_root, p) = preservation_fixture(Some("<gameList></gameList>"));
    let record = publication_record(&p).unwrap();
    persist_record(&record).unwrap();
    let path = es_de_gamelist_recovery_path(&p.gamelist_path);
    assert_eq!(
        read_recovery_record(&path, &p.gamelist_path).unwrap(),
        record
    );
    let bytes = std::fs::read(&path).unwrap();
    assert!(persist_record(&record).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn a_directory_planted_at_the_recovery_path_is_refused_without_mutation() {
    let home = tempdir().unwrap();
    let gamelist_path = home.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(gamelist_path.parent().unwrap()).unwrap();
    let recovery_path = es_de_gamelist_recovery_path(&gamelist_path);
    std::fs::create_dir(&recovery_path).unwrap();
    std::fs::write(recovery_path.join("planted"), b"attacker file").unwrap();

    let error = write_recovery_record(&recovery_path, &gamelist_path, &Some("x".to_string()))
        .expect_err("a non-regular-file destination must be refused");
    assert!(matches!(error, EsDePublicationError::Io { .. }));

    // The directory and its contents are untouched.
    assert!(recovery_path.is_dir());
    assert_eq!(
        std::fs::read_to_string(recovery_path.join("planted")).unwrap(),
        "attacker file"
    );
    let entries: Vec<_> = std::fs::read_dir(&recovery_path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("planted")]);
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
                winner_evidence: crate::playing_library::CandidateEvidenceSummary::unknown(),
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

// Preservation regressions: every path is inside this owned temporary fixture.
fn preservation_fixture(existing: Option<&str>) -> (tempfile::TempDir, EsDeGamelistPublication) {
    let root = tempdir().unwrap();
    let path = root.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    if let Some(text) = existing {
        std::fs::write(&path, text).unwrap();
    }
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("Game", root.path().join("playing/Game.chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    (root, publication)
}

#[test]
fn preservation_external_edit_after_preview_is_refused() {
    let (_root, p) = preservation_fixture(Some("<gameList><!-- before --></gameList>"));
    let edited = "<gameList><!-- user's later edit --></gameList>";
    std::fs::write(&p.gamelist_path, edited).unwrap();
    assert!(apply_es_de_gamelist_publication(&p).is_err());
    assert_eq!(std::fs::read_to_string(&p.gamelist_path).unwrap(), edited);
}

#[test]
fn preservation_file_created_after_absence_preview_is_refused() {
    let (_root, p) = preservation_fixture(None);
    let edited = "<gameList><!-- externally created --></gameList>";
    std::fs::write(&p.gamelist_path, edited).unwrap();
    assert!(apply_es_de_gamelist_publication(&p).is_err());
    assert_eq!(std::fs::read_to_string(&p.gamelist_path).unwrap(), edited);
}

#[test]
fn preservation_undo_refuses_modified_existing_and_new_output() {
    for before in [None, Some("<gameList><!-- before --></gameList>")] {
        let (_root, p) = preservation_fixture(before);
        apply_es_de_gamelist_publication(&p).unwrap();
        let edited = "<gameList><!-- edit after publish --></gameList>";
        std::fs::write(&p.gamelist_path, edited).unwrap();
        assert!(rollback_es_de_gamelist_publication(&p).is_err());
        assert_eq!(std::fs::read_to_string(&p.gamelist_path).unwrap(), edited);
    }
}

#[test]
fn preservation_two_stale_plans_cannot_replace_each_other() {
    let (_root, first) = preservation_fixture(Some("<gameList></gameList>"));
    let second = first.clone();
    apply_es_de_gamelist_publication(&first).unwrap();
    let edited = first
        .new_content
        .replace("<name>Game</name>", "<name>Changed</name>");
    std::fs::write(&first.gamelist_path, &edited).unwrap();
    assert!(apply_es_de_gamelist_publication(&second).is_err());
    assert_eq!(
        std::fs::read_to_string(&first.gamelist_path).unwrap(),
        edited
    );
}

#[test]
fn preservation_malformed_xml_with_closing_tag_is_refused() {
    let root = tempdir().unwrap();
    let path = root.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("Game", root.path().join("playing/Game.chd"))],
    );
    for text in [
        "<gameList><game></gameList>",
        "<wrong></gameList>",
        "<gameList><!-- </gameList> -->",
        "<gameList></gameList><other/>",
        "<gameList><1bad/></gameList>",
        "<gameList custom='&unknown;'></gameList>",
        "<gameList><game><path>&#0;</path></game></gameList>",
        "<!DOCTYPE gameList><gameList/>",
        "<?xml version='1.0'?><?xml version='1.0'?><gameList/>",
    ] {
        std::fs::write(&path, text).unwrap();
        assert!(
            plan_es_de_gamelist_publication(&plan, "PSX", &profile).is_err(),
            "accepted {text}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}

#[test]
fn preservation_recovery_refuses_external_changes_and_keeps_evidence() {
    for before in [None, Some("<gameList><!-- before --></gameList>")] {
        let (_root, p) = preservation_fixture(before);
        PUBLICATION_FAULT.with(|fault| *fault.borrow_mut() = Some("after_write"));
        let result = apply_es_de_gamelist_publication(&p);
        PUBLICATION_FAULT.with(|fault| *fault.borrow_mut() = None);
        assert!(result.is_err());
        let record_path = es_de_gamelist_recovery_path(&p.gamelist_path);
        let receipt = std::fs::read(&record_path).unwrap();
        let edited = "<gameList><!-- externally changed after interruption --></gameList>";
        std::fs::write(&p.gamelist_path, edited).unwrap();
        assert!(recover_es_de_gamelist_publication(&p.gamelist_path).is_err());
        assert_eq!(std::fs::read_to_string(&p.gamelist_path).unwrap(), edited);
        assert_eq!(std::fs::read(record_path).unwrap(), receipt);
    }
}

#[test]
fn preservation_legacy_receipt_requires_review_even_when_original_absent() {
    for before in [None, Some("<gameList></gameList>".to_string())] {
        let (_root, p) = preservation_fixture(None);
        let current = "<gameList><!-- user data --></gameList>";
        std::fs::write(&p.gamelist_path, current).unwrap();
        write_recovery_record(
            &es_de_gamelist_recovery_path(&p.gamelist_path),
            &p.gamelist_path,
            &before,
        )
        .unwrap();
        assert!(recover_es_de_gamelist_publication(&p.gamelist_path).is_err());
        assert_eq!(std::fs::read_to_string(&p.gamelist_path).unwrap(), current);
        assert!(has_unresolved_es_de_gamelist_recovery(&p.gamelist_path));
    }
}

#[test]
fn preservation_fault_stages_recover_only_known_before_or_after_images() {
    for stage in ["before_write", "after_write", "finalize"] {
        for before in [None, Some("<gameList><!-- before --></gameList>")] {
            let (_root, p) = preservation_fixture(before);
            PUBLICATION_FAULT.with(|fault| *fault.borrow_mut() = Some(stage));
            let result = apply_es_de_gamelist_publication(&p);
            PUBLICATION_FAULT.with(|fault| *fault.borrow_mut() = None);
            assert!(result.is_err());
            let expected = if stage == "before_write" {
                EsDeRestoreOutcome::AlreadyRestored
            } else {
                EsDeRestoreOutcome::Restored
            };
            assert_eq!(
                recover_es_de_gamelist_publication_with_outcome(&p.gamelist_path).unwrap(),
                expected
            );
            assert_eq!(read_current(&p.gamelist_path).unwrap().as_deref(), before);
            assert!(!has_unresolved_es_de_gamelist_recovery(&p.gamelist_path));
        }
    }
}

#[test]
fn preservation_modified_v2_receipt_is_not_restoration_authority() {
    let (_root, p) = preservation_fixture(Some("<gameList></gameList>"));
    PUBLICATION_FAULT.with(|fault| *fault.borrow_mut() = Some("after_write"));
    assert!(apply_es_de_gamelist_publication(&p).is_err());
    PUBLICATION_FAULT.with(|fault| *fault.borrow_mut() = None);
    let path = es_de_gamelist_recovery_path(&p.gamelist_path);
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["previous_content"] = serde_json::Value::String("foreign data".into());
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(recover_es_de_gamelist_publication(&p.gamelist_path).is_err());
    assert_eq!(
        std::fs::read_to_string(&p.gamelist_path).unwrap(),
        p.new_content
    );
    assert!(path.exists());
}

#[test]
fn preservation_replaced_file_same_bytes_is_stale() {
    let (_root, p) = preservation_fixture(Some("<gameList></gameList>"));
    let replacement = p.gamelist_path.with_extension("replacement");
    std::fs::write(&replacement, p.previous_content.as_ref().unwrap()).unwrap();
    std::fs::rename(replacement, &p.gamelist_path).unwrap();
    assert!(apply_es_de_gamelist_publication(&p).is_err());
    assert_eq!(read_current(&p.gamelist_path).unwrap(), p.previous_content);
}

#[test]
fn preservation_replaced_or_unavailable_parent_refuses_without_recreation() {
    for replace in [false, true] {
        let (_root, p) = preservation_fixture(Some("<gameList></gameList>"));
        let parent = p.gamelist_path.parent().unwrap();
        let saved = parent.with_extension("old");
        std::fs::rename(parent, &saved).unwrap();
        if replace {
            std::fs::create_dir(parent).unwrap();
        }
        assert!(apply_es_de_gamelist_publication(&p).is_err());
        assert!(!p.gamelist_path.exists());
        assert_eq!(
            std::fs::read_to_string(saved.join("gamelist.xml")).unwrap(),
            p.previous_content.unwrap()
        );
        if !replace {
            assert!(!parent.exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn preservation_parent_and_leaf_symlink_substitution_refuse() {
    for parent_swap in [false, true] {
        let (root, p) = preservation_fixture(Some("<gameList></gameList>"));
        let target = root.path().join("foreign");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("gamelist.xml"), "foreign bytes").unwrap();
        if parent_swap {
            let parent = p.gamelist_path.parent().unwrap();
            std::fs::rename(parent, parent.with_extension("old")).unwrap();
            std::os::unix::fs::symlink(&target, parent).unwrap();
        } else {
            std::fs::remove_file(&p.gamelist_path).unwrap();
            std::os::unix::fs::symlink(target.join("gamelist.xml"), &p.gamelist_path).unwrap();
        }
        assert!(apply_es_de_gamelist_publication(&p).is_err());
        assert_eq!(
            std::fs::read_to_string(target.join("gamelist.xml")).unwrap(),
            "foreign bytes"
        );
    }
}

#[test]
fn preservation_semantic_paths_unknown_metadata_and_comment_text() {
    let root = tempdir().unwrap();
    let path = root.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let game = root.path().join("playing/Game&One.chd");
    let encoded = game.to_str().unwrap().replace('&', "&#38;");
    let existing = format!(
        "<?xml version=\"1.0\"?><gameList custom=\"yes\"><!-- </gameList> --><game foreign=\"keep\"><path> {encoded} </path><custom>hello &amp; goodbye</custom></game></gameList>"
    );
    std::fs::write(&path, &existing).unwrap();
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(root.path().join("playing"), &[("Game", game)]);
    let p = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert!(p.is_unchanged());
    assert_eq!(p.new_content, existing);
    apply_es_de_gamelist_publication(&p).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
    assert_eq!(
        rollback_es_de_gamelist_publication_with_outcome(&p).unwrap(),
        EsDeRestoreOutcome::AlreadyRestored
    );
    std::fs::write(&path, "<gameList><!-- external --></gameList>").unwrap();
    assert!(rollback_es_de_gamelist_publication_with_outcome(&p).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "<gameList><!-- external --></gameList>"
    );
}

#[test]
fn preservation_matching_path_in_comment_is_not_an_entry() {
    let (root, p) = preservation_fixture(None);
    let existing = format!(
        "<gameList><!-- <path>{}</path> --></gameList>",
        p.added[0].destination_path.display()
    );
    std::fs::write(&p.gamelist_path, &existing).unwrap();
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("Game", p.added[0].destination_path.clone())],
    );
    let fresh = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert_eq!(fresh.added.len(), 1);
    assert!(fresh.new_content.starts_with("<gameList><!-- <path>"));
}

#[test]
fn preservation_repeat_apply_and_repeat_undo_are_explicit_noops() {
    let (root, p) = preservation_fixture(Some("<gameList></gameList>"));
    apply_es_de_gamelist_publication(&p).unwrap();
    let identity = crate::dat::rename_apply::capture_identity(&p.gamelist_path).unwrap();
    assert!(apply_es_de_gamelist_publication(&p).is_err());
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("Game", p.added[0].destination_path.clone())],
    );
    let fresh = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert!(fresh.is_unchanged());
    apply_es_de_gamelist_publication(&fresh).unwrap();
    assert_eq!(
        crate::dat::rename_apply::capture_identity(&p.gamelist_path).unwrap(),
        identity
    );
    assert_eq!(
        rollback_es_de_gamelist_publication_with_outcome(&p).unwrap(),
        EsDeRestoreOutcome::Restored
    );
    assert_eq!(
        rollback_es_de_gamelist_publication_with_outcome(&p).unwrap(),
        EsDeRestoreOutcome::AlreadyRestored
    );
}

#[test]
fn preservation_empty_root_preserves_attributes_and_surrounding_bytes() {
    let (_root, p) = preservation_fixture(Some(
        "<?xml version=\"1.0\"?>\n<gameList custom=\"keep\"/>\n<!-- tail -->",
    ));
    apply_es_de_gamelist_publication(&p).unwrap();
    assert!(
        p.new_content
            .starts_with("<?xml version=\"1.0\"?>\n<gameList custom=\"keep\">")
    );
    assert!(p.new_content.ends_with("</gameList>\n<!-- tail -->"));
    rollback_es_de_gamelist_publication(&p).unwrap();
    assert_eq!(read_current(&p.gamelist_path).unwrap(), p.previous_content);
}

#[cfg(unix)]
#[test]
fn preservation_competing_reservation_fails_while_owner_holds_lock() {
    let (_root, p) = preservation_fixture(None);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let release = std::sync::Arc::new(std::sync::Barrier::new(2));
    let path = p.gamelist_path.clone();
    let b = barrier.clone();
    let r = release.clone();
    let owner = std::thread::spawn(move || {
        let _lock = PublicationLock::acquire(&path).unwrap();
        b.wait();
        r.wait();
    });
    barrier.wait();
    let result = apply_es_de_gamelist_publication(&p);
    release.wait();
    owner.join().unwrap();
    assert!(result.is_err());
    assert!(!p.gamelist_path.exists());
    apply_es_de_gamelist_publication(&p).unwrap();
}

#[cfg(unix)]
#[test]
fn preservation_two_actual_publications_are_excluded_at_the_write_barrier() {
    let (_root, p) = preservation_fixture(Some("<gameList></gameList>"));
    let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
    let release = std::sync::Arc::new(std::sync::Barrier::new(2));
    let first = p.clone();
    let e = entered.clone();
    let r = release.clone();
    let worker = std::thread::spawn(move || {
        PUBLICATION_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |stage, _| {
                if stage == "before_write" {
                    e.wait();
                    r.wait();
                }
            }))
        });
        let result = apply_es_de_gamelist_publication(&first);
        PUBLICATION_HOOK.with(|hook| *hook.borrow_mut() = None);
        result
    });
    entered.wait();
    let second = apply_es_de_gamelist_publication(&p);
    release.wait();
    assert!(worker.join().unwrap().is_ok());
    assert!(second.is_err());
    assert_eq!(read_current(&p.gamelist_path).unwrap(), Some(p.new_content));
}

#[test]
fn preservation_external_edit_after_receipt_is_refused_without_blind_recovery() {
    let (_root, p) = preservation_fixture(Some("<gameList></gameList>"));
    let external = "<gameList><!-- late external edit --></gameList>";
    PUBLICATION_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |stage, path| {
            if stage == "before_write" {
                std::fs::write(path, external).unwrap();
            }
        }))
    });
    let result = apply_es_de_gamelist_publication(&p);
    PUBLICATION_HOOK.with(|hook| *hook.borrow_mut() = None);
    assert!(result.is_err());
    assert_eq!(
        read_current(&p.gamelist_path).unwrap().as_deref(),
        Some(external)
    );
    assert!(has_unresolved_es_de_gamelist_recovery(&p.gamelist_path));
    assert!(recover_es_de_gamelist_publication(&p.gamelist_path).is_err());
    assert_eq!(
        read_current(&p.gamelist_path).unwrap().as_deref(),
        Some(external)
    );
}

#[test]
fn preservation_distinct_stale_plans_preserve_the_first_publication() {
    let (root, first) = preservation_fixture(Some("<gameList></gameList>"));
    let profile = profile_with_psx_system(root.path());
    let second_plan = plan_with_operations(
        root.path().join("playing"),
        &[("Other", root.path().join("playing/Other.chd"))],
    );
    let second = plan_es_de_gamelist_publication(&second_plan, "PSX", &profile).unwrap();
    apply_es_de_gamelist_publication(&first).unwrap();
    assert!(apply_es_de_gamelist_publication(&second).is_err());
    assert_eq!(
        read_current(&first.gamelist_path).unwrap(),
        Some(first.new_content)
    );
}

#[test]
fn preservation_external_creation_matching_proposed_bytes_is_still_stale() {
    let (_root, p) = preservation_fixture(None);
    std::fs::write(&p.gamelist_path, &p.new_content).unwrap();
    assert!(apply_es_de_gamelist_publication(&p).is_err());
    assert_eq!(read_current(&p.gamelist_path).unwrap(), Some(p.new_content));
}

// ---------------------------------------------------------------------------
// Gamelist matching and preservation (2026-10-16 completion).
// ES-DE resolves `./` paths against the system ROM directory, here
// `%ROMPATH%/psx` with ROMDirectory from settings/es_settings.xml.
// ---------------------------------------------------------------------------

struct Matching {
    root: tempfile::TempDir,
    path: PathBuf,
    profile: EsDeProfile,
}

impl Matching {
    /// `rom_directory`: Some(value) writes es_settings.xml with that ROMDirectory.
    fn new(existing_games: &[&str], rom_directory: Option<&str>) -> Self {
        let root = tempdir().unwrap();
        let roms = root.path().join("roms");
        std::fs::create_dir_all(roms.join("psx")).unwrap();
        std::fs::create_dir_all(root.path().join("settings")).unwrap();
        if let Some(value) = rom_directory {
            let value = value.replace("{ROMS}", roms.to_str().unwrap());
            std::fs::write(
                root.path().join("settings/es_settings.xml"),
                format!("<?xml version=\"1.0\"?>\n<bool name=\"LegacyGamelistFileLocation\" value=\"false\" />\n<string name=\"ROMDirectory\" value=\"{value}\" />\n"),
            )
            .unwrap();
        }
        let mut text = String::from("<?xml version=\"1.0\"?>\n<gameList>\n");
        for game in existing_games {
            text.push_str(&format!(
                "\t<game>\n\t\t<path>{game}</path>\n\t\t<name>User Name</name>\n\t\t<favorite>true</favorite>\n\t\t<playcount>3</playcount>\n\t</game>\n"
            ));
        }
        text.push_str("</gameList>\n");
        let path = root.path().join("gamelists/psx/gamelist.xml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        let profile = profile_with_psx_system(root.path());
        Self {
            root,
            path,
            profile,
        }
    }
    fn p(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }
    fn file(&self, relative: &str) -> PathBuf {
        let path = self.p(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, relative.as_bytes()).unwrap();
        path
    }
    /// Publishes `(name, destination, source)` triples.
    fn plan(&self, games: &[(&str, PathBuf, PathBuf)]) -> EsDeGamelistPublication {
        let entries: Vec<(&str, PathBuf)> = games.iter().map(|(n, d, _)| (*n, d.clone())).collect();
        let mut plan = plan_with_operations(self.p("playing"), &entries);
        for (index, (_, _, source)) in games.iter().enumerate() {
            plan.operations[index].source_path = source.clone();
            plan.elected_games[index].launcher_operation.source_path = source.clone();
        }
        plan_es_de_gamelist_publication(&plan, "PSX", &self.profile).unwrap()
    }
    fn refused_names(publication: &EsDeGamelistPublication) -> Vec<&str> {
        publication
            .refused
            .iter()
            .map(|r| r.entry.dat_entry_name.as_str())
            .collect()
    }
}

fn game_count(text: &str) -> usize {
    text.matches("<game>").count() + text.matches("<game ").count()
}

#[test]
fn gamelist_dot_relative_entry_for_the_same_file_is_already_present() {
    // The playing library is ES-DE's own ROM directory: ./Game.chd *is* it.
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    let destination = m.file("roms/psx/Game.chd");
    let publication = m.plan(&[("Game", destination, m.file("master/Game.chd"))]);
    assert!(publication.is_unchanged(), "{:?}", publication.refused);
    assert_eq!(publication.already_present.len(), 1);
    assert_eq!(
        publication.new_content,
        std::fs::read_to_string(&m.path).unwrap()
    );
}

#[test]
fn gamelist_nested_dot_relative_entry_for_the_same_file_is_already_present() {
    let m = Matching::new(&["./Disc Games/Game.chd"], Some("{ROMS}/"));
    let destination = m.file("roms/psx/Disc Games/Game.chd");
    let publication = m.plan(&[("Game", destination, m.file("master/Game.chd"))]);
    assert!(publication.is_unchanged(), "{:?}", publication.refused);
}

#[test]
fn gamelist_dot_relative_entry_for_a_different_file_with_the_same_name_is_added_beside_it() {
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    m.file("roms/psx/Game.chd");
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), m.file("master/Game.chd"))]);
    assert_eq!(publication.added.len(), 1, "{:?}", publication.refused);
    assert_eq!(game_count(&publication.new_content), 2);
}

#[test]
fn gamelist_entry_for_the_source_rom_is_already_present_by_identity() {
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    let source = m.file("roms/psx/Game.chd");
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), source)]);
    assert!(publication.is_unchanged());
    assert!(publication.refused.is_empty());
}

#[cfg(unix)]
#[test]
fn gamelist_same_file_through_a_symlinked_ancestor_is_already_present_by_identity() {
    let m = Matching::new(&[], None);
    let destination = m.file("playing/Game.chd");
    std::os::unix::fs::symlink(m.p("playing"), m.p("alias")).unwrap();
    let alias = m.p("alias/Game.chd");
    let m = Matching::new(&[alias.to_str().unwrap()], None);
    let publication = m.plan(&[("Game", destination.clone(), destination)]);
    assert!(publication.is_unchanged(), "{:?}", publication.added);
}

#[test]
fn gamelist_absolute_and_lexically_equivalent_paths_are_already_present() {
    let m = Matching::new(&[], None);
    let destination = m.p("playing/Game.chd");
    let equivalent = format!("{}//./Game.chd", m.p("playing").display());
    let m = Matching::new(&[equivalent.as_str()], None);
    let publication = m.plan(&[("Game", destination, m.p("master/Game.chd"))]);
    assert!(publication.is_unchanged(), "{:?}", publication.refused);
}

#[test]
fn gamelist_moved_game_is_refused_per_entry_and_the_rest_still_publishes() {
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}")); // roms/psx/Game.chd is gone
    let publication = m.plan(&[
        ("Game", m.p("playing/Game.chd"), m.file("master/Game.chd")),
        (
            "Other",
            m.p("playing/Other.chd"),
            m.file("master/Other.chd"),
        ),
    ]);
    assert_eq!(Matching::refused_names(&publication), ["Game"]);
    assert!(publication.refused[0].reason.contains("moved"));
    assert_eq!(publication.refused[0].existing_path, "./Game.chd");
    assert_eq!(publication.added.len(), 1);
    assert_eq!(publication.added[0].dat_entry_name, "Other");
    apply_es_de_gamelist_publication(&publication).unwrap();
    let on_disk = std::fs::read_to_string(&m.path).unwrap();
    assert_eq!(game_count(&on_disk), 2);
    assert!(on_disk.contains(
        "<path>./Game.chd</path>\n\t\t<name>User Name</name>\n\t\t<favorite>true</favorite>"
    ));
}

#[test]
fn gamelist_unverified_rom_root_refuses_only_same_name_relative_entries() {
    for setting in [None, Some(""), Some("~/ROMs"), Some("relative/roms")] {
        let m = Matching::new(&["./Game.chd"], setting);
        let publication = m.plan(&[
            ("Game", m.p("playing/Game.chd"), m.file("master/Game.chd")),
            (
                "Other",
                m.p("playing/Other.chd"),
                m.file("master/Other.chd"),
            ),
        ]);
        assert_eq!(
            Matching::refused_names(&publication),
            ["Game"],
            "{setting:?}"
        );
        assert_eq!(publication.added.len(), 1, "{setting:?}");
    }
}

#[test]
fn gamelist_missing_rom_directory_is_an_unverified_root() {
    let m = Matching::new(&["./Game.chd"], Some("/nonexistent-emuwiz-roms"));
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), m.file("master/Game.chd"))]);
    assert_eq!(Matching::refused_names(&publication), ["Game"]);
}

#[test]
fn gamelist_traversal_home_bare_and_padded_paths_refuse_same_name_entries() {
    for existing in [
        "./../psx/Game.chd",
        "~/ROMs/psx/Game.chd",
        "Game.chd",
        " ./Game.chd",
    ] {
        let m = Matching::new(&[existing], Some("{ROMS}"));
        let destination = m.file("roms/psx/Game.chd");
        let publication = m.plan(&[("Game", destination, m.file("master/Game.chd"))]);
        assert_eq!(
            Matching::refused_names(&publication),
            ["Game"],
            "{existing:?}"
        );
        assert!(publication.is_unchanged(), "{existing:?}");
    }
}

#[test]
fn gamelist_whitespace_padded_absolute_path_is_not_already_present() {
    let m = Matching::new(&[], None);
    let destination = m.p("playing/Game.chd");
    let padded = format!(" {}", destination.display());
    let m = Matching::new(&[padded.as_str()], None);
    let publication = m.plan(&[("Game", destination, m.p("master/Game.chd"))]);
    assert_eq!(Matching::refused_names(&publication), ["Game"]);
}

#[cfg(unix)]
#[test]
fn gamelist_dot_relative_entry_escaping_the_rom_directory_through_a_symlink_is_refused() {
    let m = Matching::new(&["./escape/Game.chd"], Some("{ROMS}"));
    m.file("outside/Game.chd");
    std::os::unix::fs::symlink(m.p("outside"), m.p("roms/psx/escape")).unwrap();
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), m.file("master/Game.chd"))]);
    assert_eq!(Matching::refused_names(&publication), ["Game"]);
    assert!(publication.refused[0].reason.contains("outside"));
}

#[test]
fn gamelist_rom_directory_set_twice_is_an_unverified_root() {
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    let settings = m.p("settings/es_settings.xml");
    let text = std::fs::read_to_string(&settings).unwrap();
    std::fs::write(
        &settings,
        format!("{text}<string name=\"ROMDirectory\" value=\"/other\" />\n"),
    )
    .unwrap();
    m.file("roms/psx/Game.chd");
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), m.file("master/Game.chd"))]);
    assert_eq!(Matching::refused_names(&publication), ["Game"]);
}

#[test]
fn gamelist_duplicate_elections_for_one_destination_are_added_once() {
    let m = Matching::new(&[], None);
    let destination = m.p("playing/Game.chd");
    let publication = m.plan(&[
        ("Game", destination.clone(), m.file("master/Game.chd")),
        ("Game (Alt)", destination, m.file("master/Game (Alt).chd")),
    ]);
    assert_eq!(publication.added.len(), 1);
    assert_eq!(game_count(&publication.new_content), 1);
}

#[test]
fn gamelist_utf8_bom_is_preserved_or_refused_never_rewritten() {
    let root = tempdir().unwrap();
    let existing = "\u{feff}<?xml version=\"1.0\"?>\n<gameList>\n</gameList>\n";
    let path = root.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, existing).unwrap();
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("Game", root.path().join("playing/Game.chd"))],
    );
    match plan_es_de_gamelist_publication(&plan, "PSX", &profile) {
        Ok(publication) => {
            assert!(
                publication
                    .new_content
                    .starts_with("\u{feff}<?xml version=\"1.0\"?>\n<gameList>\n")
            );
            apply_es_de_gamelist_publication(&publication).unwrap();
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .starts_with('\u{feff}')
            );
        }
        Err(error) => {
            assert!(
                matches!(error, EsDePublicationError::MalformedGamelist { .. }),
                "{error:?}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
        }
    }
}

#[test]
fn gamelist_customisations_folders_and_unknown_markup_survive_as_a_pure_insertion() {
    let root = tempdir().unwrap();
    let existing = "<?xml version=\"1.0\"?>\n<!-- user notes -->\n<gameList custom=\"keep\">\n\t<alternativeEmulator>\n\t\t<label>Mednafen</label>\n\t</alternativeEmulator>\n\t<folder>\n\t\t<path>./Collections</path>\n\t\t<name>My Folder</name>\n\t\t<image>./media/folder.png</image>\n\t</folder>\n\t<game id=\"42\" source=\"ScreenScraper.fr\">\n\t\t<path>/nowhere/Moved Away.chd</path>\n\t\t<name>Renamed By User</name>\n\t\t<desc>Custom &amp; edited</desc>\n\t\t<image>~/media/a.png</image>\n\t\t<rating>0.9</rating>\n\t\t<favorite>true</favorite>\n\t\t<hidden>true</hidden>\n\t\t<playcount>7</playcount>\n\t\t<lastplayed>20260101T120000</lastplayed>\n\t\t<x-unknown attr=\"1\"><nested/></x-unknown>\n\t</game>\n</gameList>\n";
    let path = root.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, existing).unwrap();
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("New Game", root.path().join("playing/New Game.chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    let insertion_at = existing.rfind("</gameList>").unwrap();
    assert_eq!(
        &publication.new_content[..insertion_at],
        &existing[..insertion_at]
    );
    assert!(publication.new_content.ends_with("</gameList>\n"));
    let inserted = &publication.new_content
        [insertion_at..publication.new_content.len() - "</gameList>\n".len()];
    assert_eq!(game_count(inserted), 1);
    assert_eq!(
        publication.preserved_summary().unwrap(),
        Some(EsDePreservedSummary {
            game_entries: 1,
            folder_entries: 1,
            other_elements: 1,
            preserved_bytes: existing.len(),
        })
    );
    apply_es_de_gamelist_publication(&publication).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        publication.new_content
    );
}

#[test]
fn gamelist_preserved_summary_is_none_for_a_new_file() {
    let root = tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("gamelists/psx")).unwrap();
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("Game", root.path().join("playing/Game.chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    assert_eq!(publication.preserved_summary().unwrap(), None);
}

#[test]
fn gamelist_preserved_bytes_exclude_the_rewritten_empty_root_terminator() {
    let root = tempdir().unwrap();
    let existing = "<?xml version=\"1.0\"?>\n<gameList/>\n";
    let path = root.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, existing).unwrap();
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("Game", root.path().join("playing/Game.chd"))],
    );
    let publication = plan_es_de_gamelist_publication(&plan, "PSX", &profile).unwrap();
    let summary = publication.preserved_summary().unwrap().unwrap();
    assert_eq!(summary.preserved_bytes, existing.len() - 2);
}

#[test]
fn gamelist_apply_refuses_content_that_is_not_an_insertion() {
    let existing = "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>/x/Other.chd</path>\n\t\t<name>Mine</name>\n\t</game>\n</gameList>\n";
    let (root, mut publication) = preservation_fixture(Some(existing));
    publication.new_content = "<?xml version=\"1.0\"?>\n<gameList>\n\t<game>\n\t\t<path>/x/Other.chd</path>\n\t\t<name>Replaced</name>\n\t</game>\n</gameList>\n".into();
    let path = root.path().join("gamelists/psx/gamelist.xml");
    let result = apply_es_de_gamelist_publication(&publication);
    assert!(result.is_err(), "{result:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
    assert!(!has_unresolved_es_de_gamelist_recovery(&path));
}

// Review round 2 (2026-10-16) regressions.

fn publish_one(
    existing: &str,
) -> (
    tempfile::TempDir,
    PathBuf,
    Result<EsDeGamelistPublication, EsDePublicationError>,
) {
    let root = tempdir().unwrap();
    let path = root.path().join("gamelists/psx/gamelist.xml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, existing).unwrap();
    let profile = profile_with_psx_system(root.path());
    let plan = plan_with_operations(
        root.path().join("playing"),
        &[("Game", root.path().join("playing/Game.chd"))],
    );
    let result = plan_es_de_gamelist_publication(&plan, "PSX", &profile);
    (root, path, result)
}

/// A publication must either refuse, or insert exactly one real entry right
/// before the final `</gameList>` while keeping every original byte.
fn assert_inserted_before_close_or_refused(existing: &str) {
    let (_root, path, result) = publish_one(existing);
    let Ok(publication) = result else {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
        return;
    };
    let new = &publication.new_content;
    let close = new.rfind("</gameList>").expect("closing tag");
    let inserted_at = new.find("\t<game>\n\t\t<path>").expect("inserted entry");
    assert!(inserted_at < close, "{new:?}");
    assert!(!new[..inserted_at].ends_with("<!--"), "{new:?}");
    assert!(
        new.contains("<path>") && new.matches("Game.chd").count() == 1,
        "{new:?}"
    );
    apply_es_de_gamelist_publication(&publication).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), *new);
}

#[test]
fn gamelist_bom_with_a_trailing_comment_inserts_outside_the_comment() {
    let existing = "\u{feff}<?xml version=\"1.0\"?>\n<gameList>\n<!--note--></gameList>\n";
    assert_inserted_before_close_or_refused(existing);
    let (_r, _p, result) = publish_one(existing);
    if let Ok(publication) = result {
        assert!(publication.new_content.contains("<!--note-->"));
        assert!(
            publication
                .new_content
                .starts_with("\u{feff}<?xml version=\"1.0\"?>\n<gameList>\n<!--note-->")
        );
    }
}

#[test]
fn gamelist_bom_with_an_empty_root_keeps_its_attributes() {
    let existing = "\u{feff}<gameList a=\"xy\"/>";
    let (_r, _p, result) = publish_one(existing);
    if let Ok(publication) = result {
        assert!(
            publication
                .new_content
                .starts_with("\u{feff}<gameList a=\"xy\">"),
            "{:?}",
            publication.new_content
        );
        assert!(publication.new_content.ends_with("</gameList>"));
    }
    assert_inserted_before_close_or_refused(existing);
}

#[test]
fn gamelist_multibyte_text_before_the_closing_tag_never_panics() {
    for existing in [
        "<gameList><!--é--></gameList>",
        "\u{feff}<gameList>日本語</gameList>",
        "\u{feff}<gameList><game><path>/x/é.chd</path></game>é</gameList>",
    ] {
        let (_r, _p, result) = publish_one(existing);
        if let Ok(publication) = result {
            assert!(
                publication.new_content.ends_with("</gameList>"),
                "{existing:?}"
            );
        }
    }
}

#[test]
fn gamelist_apply_refuses_a_publication_whose_entry_lands_inside_a_comment() {
    let existing = "<?xml version=\"1.0\"?>\n<gameList>\n<!--note--></gameList>\n";
    let (root, mut publication) = preservation_fixture(Some(existing));
    // Splice three bytes early, as a wrong offset would.
    let close = existing.rfind("</gameList>").unwrap() - 3;
    publication.new_content = format!(
        "{}\t<game>\n\t\t<path>{}</path>\n\t</game>\n{}",
        &existing[..close],
        root.path().join("playing/Game.chd").display(),
        &existing[close..]
    );
    let path = root.path().join("gamelists/psx/gamelist.xml");
    assert!(apply_es_de_gamelist_publication(&publication).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
}

#[cfg(unix)]
#[test]
fn gamelist_dot_relative_leaf_symlink_to_the_source_rom_is_already_present() {
    // RetroDECK's ROM root populated by EmuWiz links: ./Game.chd is a link
    // whose target (outside the root) is this game's source ROM.
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    let source = m.file("master/Game.chd");
    std::os::unix::fs::symlink(&source, m.p("roms/psx/Game.chd")).unwrap();
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), source)]);
    assert!(publication.is_unchanged(), "{:?}", publication.refused);
    assert!(publication.refused.is_empty());
}

// Owner-approved symlink exception (2026-10-16): a final-component file
// symlink may resolve outside the ROM root only for read-only identity.

#[cfg(unix)]
#[test]
fn gamelist_external_rom_symlink_to_a_different_file_is_added_beside_it() {
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    let elsewhere = m.file("elsewhere/Game.chd");
    std::os::unix::fs::symlink(&elsewhere, m.p("roms/psx/Game.chd")).unwrap();
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), m.file("master/Game.chd"))]);
    assert_eq!(publication.added.len(), 1, "{:?}", publication.refused);
}

#[cfg(unix)]
#[test]
fn gamelist_broken_rom_symlink_establishes_no_identity() {
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    std::os::unix::fs::symlink(m.p("gone/Game.chd"), m.p("roms/psx/Game.chd")).unwrap();
    let source = m.file("master/Game.chd");
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), source)]);
    assert_eq!(Matching::refused_names(&publication), ["Game"]);
    assert!(publication.added.is_empty());
}

#[cfg(unix)]
#[test]
fn gamelist_rom_symlink_to_a_directory_establishes_no_identity() {
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    std::fs::create_dir_all(m.p("outside/dir")).unwrap();
    std::os::unix::fs::symlink(m.p("outside/dir"), m.p("roms/psx/Game.chd")).unwrap();
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), m.file("master/Game.chd"))]);
    assert_eq!(Matching::refused_names(&publication), ["Game"]);
}

#[cfg(unix)]
#[test]
fn gamelist_rom_symlink_retargeted_after_preview_makes_apply_stale() {
    // Preview: ./Game.chd links to a different file, so Game is added.
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    let link = m.p("roms/psx/Game.chd");
    std::os::unix::fs::symlink(m.file("elsewhere/Game.chd"), &link).unwrap();
    let source = m.file("master/Game.chd");
    let publication = m.plan(&[("Game", m.p("playing/Game.chd"), source.clone())]);
    assert_eq!(publication.added.len(), 1);
    // Before Apply the link is retargeted at this game's own ROM: adding now
    // would duplicate it.
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let before = std::fs::read_to_string(&m.path).unwrap();
    let error = apply_es_de_gamelist_publication(&publication).unwrap_err();
    assert!(error.to_string().contains("preview is stale"), "{error}");
    assert_eq!(std::fs::read_to_string(&m.path).unwrap(), before);
    assert!(!has_unresolved_es_de_gamelist_recovery(&m.path));
}

#[cfg(unix)]
#[test]
fn gamelist_present_by_link_then_broken_before_apply_is_stale() {
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    let link = m.p("roms/psx/Game.chd");
    let source = m.file("master/Game.chd");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let publication = m.plan(&[
        ("Game", m.p("playing/Game.chd"), source),
        (
            "Other",
            m.p("playing/Other.chd"),
            m.file("master/Other.chd"),
        ),
    ]);
    assert_eq!(publication.already_present.len(), 1);
    assert_eq!(publication.added.len(), 1);
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(m.p("gone.chd"), &link).unwrap();
    let error = apply_es_de_gamelist_publication(&publication).unwrap_err();
    assert!(error.to_string().contains("preview is stale"), "{error}");
}

#[cfg(unix)]
#[test]
fn gamelist_links_created_by_this_plan_after_preview_are_not_stale() {
    // The RetroDECK flow creates the playing-library links between preview
    // and publication; the ROM root is the destination directory.
    let m = Matching::new(&["./Old.chd"], Some("{ROMS}"));
    let source = m.file("master/Game.chd");
    let destination = m.p("roms/psx/Game.chd");
    let publication = m.plan(&[("Game", destination.clone(), source.clone())]);
    assert_eq!(publication.added.len(), 1);
    std::os::unix::fs::symlink(&source, &destination).unwrap();
    apply_es_de_gamelist_publication(&publication).unwrap();
    assert!(
        std::fs::read_to_string(&m.path)
            .unwrap()
            .contains("Game.chd")
    );
}

#[cfg(unix)]
#[test]
fn gamelist_link_to_a_foreign_file_appearing_after_preview_is_stale() {
    let m = Matching::new(&[], None);
    let destination = m.p("playing/Game.chd");
    let publication = m.plan(&[("Game", destination.clone(), m.file("master/Game.chd"))]);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(m.file("foreign/Game.chd"), &destination).unwrap();
    let error = apply_es_de_gamelist_publication(&publication).unwrap_err();
    assert!(error.to_string().contains("preview is stale"), "{error}");
}

// Review round 3: the link exception is bound to each destination's own source.

#[cfg(unix)]
#[test]
fn gamelist_destination_linked_to_another_planned_games_source_is_stale() {
    let m = Matching::new(&[], None);
    let first = m.file("master/Game.chd");
    let second = m.file("master/Other.chd");
    let destination = m.p("playing/Game.chd");
    let publication = m.plan(&[
        ("Game", destination.clone(), first),
        ("Other", m.p("playing/Other.chd"), second.clone()),
    ]);
    assert_eq!(publication.added.len(), 2);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&second, &destination).unwrap();
    let error = apply_es_de_gamelist_publication(&publication).unwrap_err();
    assert!(error.to_string().contains("preview is stale"), "{error}");
}

#[cfg(unix)]
#[test]
fn gamelist_refused_entry_that_becomes_a_link_to_another_planned_source_is_stale() {
    // ./Game.chd is gone at preview (Game refused as moved); then something
    // creates it as a link to Other's source, which Other's new entry would
    // duplicate.
    let m = Matching::new(&["./Game.chd"], Some("{ROMS}"));
    let other_source = m.file("master/Other.chd");
    let publication = m.plan(&[
        ("Game", m.p("playing/Game.chd"), m.file("master/Game.chd")),
        ("Other", m.p("playing/Other.chd"), other_source.clone()),
    ]);
    assert_eq!(Matching::refused_names(&publication), ["Game"]);
    std::os::unix::fs::symlink(&other_source, m.p("roms/psx/Game.chd")).unwrap();
    let error = apply_es_de_gamelist_publication(&publication).unwrap_err();
    assert!(error.to_string().contains("preview is stale"), "{error}");
}

#[cfg(unix)]
#[test]
fn gamelist_republication_after_links_were_undone_is_not_stale() {
    // The gamelist kept Game's entry, but its playing-library link was undone;
    // re-applying recreates the link before publishing a new game beside it.
    let m = Matching::new(&[], Some("{ROMS}"));
    let destination = m.p("roms/psx/Game.chd");
    let m = Matching::new(&[destination.to_str().unwrap()], Some("{ROMS}"));
    let source = m.file("master/Game.chd");
    let publication = m.plan(&[
        ("Game", destination.clone(), source.clone()),
        (
            "Other",
            m.p("roms/psx/Other.chd"),
            m.file("master/Other.chd"),
        ),
    ]);
    assert_eq!(publication.already_present.len(), 1);
    assert_eq!(publication.added.len(), 1);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&source, &destination).unwrap();
    apply_es_de_gamelist_publication(&publication).unwrap();
}
