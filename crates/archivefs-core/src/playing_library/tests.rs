//! Focused tests for the playing-library planner.
//!
//! All grouping evidence is synthetic but shape-faithful: real `ParsedDat`
//! /`DatGameEntry` values whose only populated fields are the ones the
//! trusted flow actually uses (`name`, `clone_of`). Every test that builds a
//! plan asserts the planning run itself wrote nothing to disk.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::dat::model::{DatEcosystem, DatFormat, DatGameEntry, DatSource};

fn game(name: &str, clone_of: Option<&str>) -> DatGameEntry {
    DatGameEntry {
        name: name.to_string(),
        clone_of: clone_of.map(str::to_string),
        ..DatGameEntry::default()
    }
}

fn synthetic_dat(games: Vec<DatGameEntry>) -> ParsedDat {
    ParsedDat {
        source: DatSource {
            format: DatFormat::Logiqx,
            ecosystem: DatEcosystem::NoIntro,
            file_path: "synthetic (No-Intro).dat".to_string(),
            name: Some("Synthetic".to_string()),
            description: None,
            version: None,
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: games.len(),
            rom_count: 0,
            parse_warnings: Vec::new(),
            packing_policy: crate::dat::model::DatPackingPolicy::Standard,
        },
        games,
    }
}

/// One verified archive per DAT entry, named after it, under `source`.
fn auto_matches(dat_games: &[DatGameEntry], source: &Path) -> Vec<DatArchiveMatch> {
    dat_games
        .iter()
        .enumerate()
        .map(|(index, game)| DatArchiveMatch {
            archive_path: source.join(format!("{}.zip", game.name)),
            dat_entry_index: index,
            companion_paths: Vec::new(),
        })
        .collect()
}

fn default_policy() -> PlayingLibraryPolicy {
    PlayingLibraryPolicy {
        preferred_regions: vec!["Europe".into(), "USA".into(), "Japan".into()],
        prefer_newest_revision: true,
        excluded_release_classes: ReleaseClass::all().to_vec(),
        ..PlayingLibraryPolicy::default()
    }
}

/// Recursive snapshot of every path under a directory - the "planning wrote
/// nothing" oracle.
fn tree_snapshot(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        if current.is_dir()
            && let Ok(entries) = fs::read_dir(&current)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                paths.push(path.clone());
                if path.is_dir() {
                    stack.push(path);
                }
            }
        }
    }
    paths.sort();
    paths
}

fn elected_names(plan: &PlayingLibraryPlan) -> Vec<&str> {
    plan.elected_games
        .iter()
        .map(|game| game.dat_entry_name.as_str())
        .collect()
}

#[test]
fn europe_wins_over_usa_when_configured_first() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Sonic the Hedgehog (Europe)", None),
        game(
            "Sonic the Hedgehog (USA)",
            Some("Sonic the Hedgehog (Europe)"),
        ),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(elected_names(&plan), vec!["Sonic the Hedgehog (Europe)"]);
    assert!(plan.unresolved_groups.is_empty());
    assert_eq!(plan.elected_games[0].explanation.steps.len(), 1);
    assert!(plan.elected_games[0].explanation.steps[0].contains("preferred region \"Europe\""));
    assert_eq!(plan.elected_games[0].explanation.rejected.len(), 1);
    assert_eq!(
        plan.elected_games[0].explanation.rejected[0].dat_entry_name,
        "Sonic the Hedgehog (USA)"
    );
    assert_eq!(
        plan.elected_games[0].explanation.rejected[0].reasons,
        vec!["lower preferred-region rank".to_string()]
    );
}

#[test]
fn usa_wins_when_policy_order_is_reversed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Sonic the Hedgehog (Europe)", None),
        game(
            "Sonic the Hedgehog (USA)",
            Some("Sonic the Hedgehog (Europe)"),
        ),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: PlayingLibraryPolicy {
            preferred_regions: vec!["USA".into(), "Europe".into()],
            ..default_policy()
        },
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(elected_names(&plan), vec!["Sonic the Hedgehog (USA)"]);
}

#[test]
fn newest_verified_revision_wins_when_enabled() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Metal Gear (Japan)", None),
        game("Metal Gear (Japan) (Rev 1A)", Some("Metal Gear (Japan)")),
        game("Metal Gear (Japan) (Rev 1)", Some("Metal Gear (Japan)")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: PlayingLibraryPolicy {
            prefer_newest_revision: true,
            ..PlayingLibraryPolicy::default()
        },
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(elected_names(&plan), vec!["Metal Gear (Japan) (Rev 1A)"]);
    let rejected = &plan.elected_games[0].explanation.rejected;
    assert!(
        rejected
            .iter()
            .all(|candidate| candidate.reasons[0].contains("older or absent verified revision"))
    );
}

#[test]
fn revision_preference_can_be_disabled() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Metal Gear (Japan)", None),
        game("Metal Gear (Japan) (Rev 1A)", Some("Metal Gear (Japan)")),
        game("Metal Gear (Japan) (Rev 1)", Some("Metal Gear (Japan)")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        // No preferences at all, and revisions disabled: nothing may elect.
        policy: PlayingLibraryPolicy::default(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert!(plan.elected_games.is_empty());
    // The root plus its two clones all tie: no preferred regions configured,
    // revisions disabled, identical trusted fields everywhere.
    assert_eq!(plan.unresolved_groups[0].tied_candidates.len(), 3);
}

#[test]
fn beta_proto_demo_sample_excluded_only_when_metadata_says_so() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Protector (USA)", None), // title word, never evidence by itself
        game("Sonic the Hedgehog (Beta)", Some("Protector (USA)")),
        game("Sonic the Hedgehog 2 (Proto)", Some("Protector (USA)")),
        game(
            "Sonic the Hedgehog Spinball (Demo)",
            Some("Protector (USA)"),
        ),
        game("Sonic the Hedgehog (Sample)", Some("Protector (USA)")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(plan.exclusions.len(), 4);
    let excluded_names: Vec<&str> = plan
        .exclusions
        .iter()
        .map(|candidate| candidate.dat_entry_name.as_str())
        .collect();
    assert!(!excluded_names.contains(&"Protector (USA)"));
    assert_eq!(elected_names(&plan), vec!["Protector (USA)"]);
}

#[test]
fn unknown_release_status_is_not_treated_as_bad() {
    let temp = tempfile::tempdir().expect("temp dir");
    // No status token anywhere; nothing may be excluded for it.
    let dat = synthetic_dat(vec![game("Super Tennis (Europe)", None)]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert!(plan.exclusions.is_empty());
    assert_eq!(elected_names(&plan), vec!["Super Tennis (Europe)"]);
}

#[test]
fn ambiguous_grouping_remains_unresolved() {
    let temp = tempfile::tempdir().expect("temp dir");
    // Both clones have identical trusted fields under this policy: equal
    // region rank (no preferred region matched), no languages, no revisions.
    let dat = synthetic_dat(vec![
        game("Tetris (Japan)", None),
        game("Tetris (Asia)", Some("Tetris (Japan)")),
        game("Tetris (World)", Some("Tetris (Japan)")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: PlayingLibraryPolicy {
            preferred_regions: vec!["Europe".into()],
            ..default_policy()
        },
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert!(plan.elected_games.is_empty());
    assert_eq!(plan.unresolved_groups.len(), 1);
    // Root + both clones tie under this policy: no preferred-region match,
    // no language evidence, revisions not compared, prefer_parent disabled.
    assert_eq!(plan.unresolved_groups[0].tied_candidates.len(), 3);
    assert!(plan.operations.is_empty());
}

#[test]
fn no_filename_only_family_creation() {
    let temp = tempfile::tempdir().expect("temp dir");
    // Similar titles, no cloneof anywhere: they must stay separate families,
    // each electing itself, never merged by name similarity.
    let dat = synthetic_dat(vec![
        game("Mega Man (Europe)", None),
        game("Mega Man X (Europe)", None),
        game("Mega Man Soccer (USA)", None),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(plan.families_examined, 3);
    assert_eq!(plan.singleton_families, 3);
    assert_eq!(plan.elected_games.len(), 3);
    assert!(plan.unresolved_groups.is_empty());
}

#[test]
fn planning_is_deterministic() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("F-Zero (Japan)", None),
        game("F-Zero (Europe)", Some("F-Zero (Japan)")),
        game("F-Zero (USA)", Some("F-Zero (Japan)")),
        game("F-Zero (Europe) (Rev 1)", Some("F-Zero (Japan)")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let build = || {
        build_playing_library_plan(&PlayingLibraryRequest {
            dat: &dat,
            matches: matches.clone(),
            destination_root: temp.path().join("playing"),
            policy: default_policy(),
        })
        .expect("plan")
    };

    let first = build();
    let second = build();
    assert_eq!(first, second);
    // The tie between the two Europe entries is broken by revision.
    assert_eq!(elected_names(&first), vec!["F-Zero (Europe) (Rev 1)"]);
}

#[test]
fn plan_points_at_originals_and_touches_nothing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let source = temp.path().join("archive collection");
    fs::create_dir_all(&source).expect("source dir");
    fs::write(source.join("Sonic the Hedgehog (Europe).zip"), b"europe").expect("write source");
    fs::write(source.join("Sonic the Hedgehog (USA).zip"), b"usa").expect("write source");
    let destination_root = temp.path().join("playing library");

    let before_source_tree = tree_snapshot(temp.path());

    let dat = synthetic_dat(vec![
        game("Sonic the Hedgehog (Europe)", None),
        game(
            "Sonic the Hedgehog (USA)",
            Some("Sonic the Hedgehog (Europe)"),
        ),
    ]);
    let matches = vec![
        DatArchiveMatch {
            archive_path: source.join("Sonic the Hedgehog (Europe).zip"),
            dat_entry_index: 0,
            companion_paths: Vec::new(),
        },
        DatArchiveMatch {
            archive_path: source.join("Sonic the Hedgehog (USA).zip"),
            dat_entry_index: 1,
            companion_paths: Vec::new(),
        },
    ];
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: destination_root.clone(),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    // The one proposed link points at the untouched original file.
    assert_eq!(plan.operations.len(), 1);
    let operation = &plan.operations[0];
    assert_eq!(
        operation.source_path,
        source.join("Sonic the Hedgehog (Europe).zip")
    );
    assert_eq!(
        operation.destination_path,
        destination_root.join("Sonic the Hedgehog (Europe).zip")
    );
    assert_eq!(
        operation.source_path,
        plan.elected_games[0].launcher_operation.source_path
    );

    // Planning created nothing: not even the destination root.
    assert!(!destination_root.exists());
    let after_source_tree = tree_snapshot(temp.path());
    assert_eq!(before_source_tree, after_source_tree);
    // Source files' bytes are intact.
    assert_eq!(
        fs::read(source.join("Sonic the Hedgehog (Europe).zip")).expect("read back"),
        b"europe"
    );
}

#[test]
fn duplicate_destination_names_become_conflicts_never_overwritten() {
    let temp = tempfile::tempdir().expect("temp dir");
    // Two unrelated families whose elected archives share one file name.
    let dat = synthetic_dat(vec![
        game("Family One (Europe)", None),
        game("Family Two (Japan)", None),
    ]);
    let matches = vec![
        DatArchiveMatch {
            archive_path: temp.path().join("a").join("game.zip"),
            dat_entry_index: 0,
            companion_paths: Vec::new(),
        },
        DatArchiveMatch {
            archive_path: temp.path().join("b").join("game.zip"),
            dat_entry_index: 1,
            companion_paths: Vec::new(),
        },
    ];
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(plan.elected_games.len(), 2);
    assert_eq!(plan.conflicts.len(), 1);
    assert_eq!(plan.conflicts[0].contenders.len(), 2);
    // No operation may remain that would claim the clashing name.
    assert!(plan.operations.is_empty());
}

#[test]
fn evidence_tokens_are_strict() {
    use super::evidence::{DatReleaseEvidence, dat_release_evidence};

    let parsed = dat_release_evidence("Super Mario (Europe) (En,Fr) (Rev 1A)");
    assert_eq!(parsed.regions, vec!["Europe"]);
    assert_eq!(parsed.languages, vec!["En", "Fr"]);
    assert_eq!(parsed.revision.expect("revision").major, 1);
    assert_eq!(parsed.revision.expect("revision").letter, 'A');
    assert!(parsed.release_classes.is_empty());

    // A title word is never a status token.
    let parsed: DatReleaseEvidence = dat_release_evidence("Prototype Tournament (USA)");
    assert_eq!(parsed.regions, vec!["USA"]);
    assert!(parsed.release_classes.is_empty());

    // A delimited token is the only class evidence.
    let parsed = dat_release_evidence("Sonic (Beta)");
    assert_eq!(parsed.release_classes, vec![ReleaseClass::Beta]);
}

#[test]
fn separate_token_groups_preserve_declaration_order() {
    use super::evidence::dat_release_evidence;

    // Two separate trailing groups, not a single comma list: declaration
    // order must survive even though the scan itself walks right-to-left.
    let parsed = dat_release_evidence("Pokemon Crystal (En) (Ja)");
    assert_eq!(parsed.languages, vec!["En", "Ja"]);

    let parsed = dat_release_evidence("Pokemon Crystal (Ja) (En)");
    assert_eq!(parsed.languages, vec!["Ja", "En"]);

    // A single comma list was never affected either way, but re-assert it
    // alongside the fix so a future regression in either path is caught.
    let parsed = dat_release_evidence("Pokemon Crystal (En,Ja)");
    assert_eq!(parsed.languages, vec!["En", "Ja"]);

    // Three separate groups spanning every evidence kind: full left-to-right
    // declaration order must come back exactly as written.
    let parsed = dat_release_evidence("Game (Europe) (En) (Rev 2)");
    assert_eq!(parsed.regions, vec!["Europe"]);
    assert_eq!(parsed.languages, vec!["En"]);
    assert_eq!(parsed.revision.expect("revision").major, 2);
}

#[test]
fn preferred_language_wins_end_to_end() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Pokemon Red (En)", None),
        game("Pokemon Red (Ja)", Some("Pokemon Red (En)")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: PlayingLibraryPolicy {
            preferred_languages: vec!["Ja".into(), "En".into()],
            ..PlayingLibraryPolicy::default()
        },
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(elected_names(&plan), vec!["Pokemon Red (Ja)"]);
    assert!(
        plan.elected_games[0].explanation.steps[0].contains("preferred language \"Ja\""),
        "steps: {:?}",
        plan.elected_games[0].explanation.steps
    );
}

#[test]
fn prefer_parent_wins_when_every_other_field_ties() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Chrono Trigger", None),
        game("Chrono Trigger (Reprint)", Some("Chrono Trigger")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: PlayingLibraryPolicy {
            prefer_parent: true,
            ..PlayingLibraryPolicy::default()
        },
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(elected_names(&plan), vec!["Chrono Trigger"]);
    assert!(
        plan.elected_games[0]
            .explanation
            .steps
            .iter()
            .any(|step| step.contains("declared parent")),
        "steps: {:?}",
        plan.elected_games[0].explanation.steps
    );
}

#[test]
fn without_prefer_parent_the_same_tie_stays_unresolved() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Chrono Trigger", None),
        game("Chrono Trigger (Reprint)", Some("Chrono Trigger")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: PlayingLibraryPolicy::default(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert!(plan.elected_games.is_empty());
    assert_eq!(plan.unresolved_groups[0].tied_candidates.len(), 2);
}

#[test]
fn clone_cycle_fails_closed_never_merges() {
    let temp = tempfile::tempdir().expect("temp dir");
    // Two entries whose cloneof chains point at each other: an authoring
    // error, not a real catalogue shape. Neither may be merged into the
    // other's family - each must stand alone.
    let dat = synthetic_dat(vec![
        game("Ring A", Some("Ring B")),
        game("Ring B", Some("Ring A")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(
        plan.families_examined, 2,
        "a cycle must never merge into one family"
    );
    assert_eq!(plan.singleton_families, 2);
    assert_eq!(plan.elected_games.len(), 2);
    assert!(plan.unresolved_groups.is_empty());
}

#[test]
fn duplicate_parent_name_fails_closed_never_merges() {
    let temp = tempfile::tempdir().expect("temp dir");
    // Two entries share the name "Foo" (an authoring error some real DATs do
    // contain); a third clones "Foo". The clone target is ambiguous, so the
    // clone must stand alone rather than guess which "Foo" it means - and
    // the two "Foo" entries themselves, having no cloneof of their own, are
    // never touched by the ambiguity either.
    let dat = synthetic_dat(vec![
        game("Foo", None),
        game("Foo", None),
        game("Bar", Some("Foo")),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(
        plan.families_examined, 3,
        "an ambiguous clone target must never be silently resolved to either duplicate"
    );
    assert_eq!(plan.singleton_families, 3);
    assert_eq!(plan.elected_games.len(), 3);
}

#[test]
fn out_of_range_dat_entry_index_is_an_error() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![game("Solo Game (Europe)", None)]);
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches: vec![DatArchiveMatch {
            archive_path: temp.path().join("solo.zip"),
            dat_entry_index: 7,
            companion_paths: Vec::new(),
        }],
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let error = build_playing_library_plan(&request).expect_err("must refuse an invalid match");
    assert!(
        error.contains('7'),
        "error should name the bad index: {error}"
    );
}

#[test]
fn multi_region_evidence_ranks_by_best_preferred_match() {
    let temp = tempfile::tempdir().expect("temp dir");
    // The clone carries BOTH "Europe" and "USA" region tokens on one entry.
    // Under a policy preferring Europe first, it must win purely because it
    // has a Europe token among its several regions - not because it is
    // somehow "more" of a match than a single-region entry would be.
    let dat = synthetic_dat(vec![
        game("Game (Japan)", None),
        game("Game (Europe) (USA)", Some("Game (Japan)")),
    ]);
    let evidence = super::evidence::dat_release_evidence("Game (Europe) (USA)");
    assert_eq!(evidence.regions, vec!["Europe", "USA"]);

    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: PlayingLibraryPolicy {
            preferred_regions: vec!["Europe".into(), "USA".into(), "Japan".into()],
            ..PlayingLibraryPolicy::default()
        },
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(elected_names(&plan), vec!["Game (Europe) (USA)"]);
}

// --- multi-file releases (CUE/GDI companions) ----------------------------

#[test]
fn a_multi_file_match_produces_one_election_with_launcher_and_companions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![game("Game (Europe)", None)]);
    let cue = temp.path().join("Game (Europe).cue");
    let track1 = temp.path().join("track1.bin");
    let track2 = temp.path().join("track2.bin");
    let matches = vec![DatArchiveMatch {
        archive_path: cue.clone(),
        dat_entry_index: 0,
        companion_paths: vec![track1.clone(), track2.clone()],
    }];
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(plan.elected_games.len(), 1);
    let elected = &plan.elected_games[0];
    assert_eq!(elected.launcher_operation.source_path, cue);
    assert_eq!(elected.companion_operations.len(), 2);
    assert!(
        elected
            .companion_operations
            .iter()
            .any(|op| op.source_path == track1)
    );
    assert!(
        elected
            .companion_operations
            .iter()
            .any(|op| op.source_path == track2)
    );
    // Requirement 14: the explanation states that companions are included.
    assert!(
        elected
            .explanation
            .steps
            .iter()
            .any(|step| step.contains("2 companion file"))
    );
    // Requirement 8: the transaction-facing operations list carries every
    // required file, launcher and companions alike.
    assert_eq!(plan.operations.len(), 3);
    assert!(plan.operations.contains(&elected.launcher_operation));
    for companion in &elected.companion_operations {
        assert!(plan.operations.contains(companion));
    }
}

#[test]
fn elected_gamecube_two_disc_release_retains_every_disc() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![game("Game Foo (Europe)", None)]);
    let disc1 = temp.path().join("Game Foo (Disc 1).iso");
    let disc2 = temp.path().join("Game Foo (Disc 2).iso");
    let launcher = temp.path().join("Game Foo (Europe).m3u");
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches: vec![DatArchiveMatch {
            archive_path: launcher.clone(),
            dat_entry_index: 0,
            companion_paths: vec![disc1.clone(), disc2.clone()],
        }],
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };

    let plan = build_playing_library_plan(&request).expect("plan");
    let elected = &plan.elected_games[0];

    assert_eq!(elected.dat_entry_name, "Game Foo (Europe)");
    assert_eq!(elected.companion_operations.len(), 2);
    assert!(
        elected
            .companion_operations
            .iter()
            .any(|operation| operation.source_path == disc1)
    );
    assert!(
        elected
            .companion_operations
            .iter()
            .any(|operation| operation.source_path == disc2)
    );
    assert_eq!(plan.operations.len(), 3);
}

#[test]
fn losing_multidisc_region_has_no_disc_materialized() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Game Foo (Europe)", None),
        game("Game Foo (USA)", Some("Game Foo (Europe)")),
    ]);
    let europe_launcher = temp.path().join("Game Foo (Europe).m3u");
    let europe_disc1 = temp.path().join("Game Foo Europe (Disc 1).iso");
    let europe_disc2 = temp.path().join("Game Foo Europe (Disc 2).iso");
    let usa_launcher = temp.path().join("Game Foo (USA).m3u");
    let usa_disc1 = temp.path().join("Game Foo USA (Disc 1).iso");
    let usa_disc2 = temp.path().join("Game Foo USA (Disc 2).iso");
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches: vec![
            DatArchiveMatch {
                archive_path: europe_launcher,
                dat_entry_index: 0,
                companion_paths: vec![europe_disc1.clone(), europe_disc2.clone()],
            },
            DatArchiveMatch {
                archive_path: usa_launcher,
                dat_entry_index: 1,
                companion_paths: vec![usa_disc1.clone(), usa_disc2.clone()],
            },
        ],
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };

    let plan = build_playing_library_plan(&request).expect("plan");
    assert_eq!(elected_names(&plan), vec!["Game Foo (Europe)"]);
    assert_eq!(plan.operations.len(), 3);
    assert!(
        plan.operations
            .iter()
            .any(|operation| operation.source_path == europe_disc1)
    );
    assert!(
        plan.operations
            .iter()
            .any(|operation| operation.source_path == europe_disc2)
    );
    assert!(
        plan.operations
            .iter()
            .all(|operation| operation.source_path != usa_disc1
                && operation.source_path != usa_disc2)
    );
}

#[test]
fn elected_three_disc_release_has_stable_complete_materialization() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![game("Game Foo (Europe)", None)]);
    let launcher = temp.path().join("Game Foo (Europe).m3u");
    let discs = (1..=3)
        .map(|number| temp.path().join(format!("Game Foo (Disc {number}).iso")))
        .collect::<Vec<_>>();
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches: vec![DatArchiveMatch {
            archive_path: launcher,
            dat_entry_index: 0,
            companion_paths: discs.clone(),
        }],
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };

    let first = build_playing_library_plan(&request).expect("first plan");
    let second = build_playing_library_plan(&request).expect("second plan");

    assert_eq!(first, second);
    assert_eq!(first.elected_games[0].companion_operations.len(), 3);
    assert_eq!(first.operations.len(), 4);
}

#[test]
fn a_companion_destination_collision_excludes_the_whole_release_not_just_the_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Game One (Europe)", None),
        game("Game Two (Europe)", None),
    ]);
    let destination_root = temp.path().join("playing");
    // Game One is a CUE release whose companion happens to share a
    // basename with Game Two's own (single-file) destination.
    let matches = vec![
        DatArchiveMatch {
            archive_path: temp.path().join("one.cue"),
            dat_entry_index: 0,
            companion_paths: vec![temp.path().join("shared.bin")],
        },
        DatArchiveMatch {
            archive_path: temp.path().join("shared.bin"),
            dat_entry_index: 1,
            companion_paths: Vec::new(),
        },
    ];
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: destination_root.clone(),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(plan.elected_games.len(), 2);
    assert_eq!(plan.conflicts.len(), 1);
    // Neither election's operations appear in the applicable set - Game
    // One's own launcher (one.cue) is excluded too, even though its own
    // destination never collided with anything, because collision
    // handling covers the whole release atomically.
    assert!(plan.operations.is_empty(), "{:?}", plan.operations);
}

#[test]
fn stable_tie_break_is_independent_of_input_order() {
    // Same family, same policy, but the caller's match list arrives in a
    // different order each time (as a real filesystem scan might return
    // them). The result - including which candidate wins and every
    // narrated reason - must not depend on that order at all.
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("F-Zero (Japan)", None),
        game("F-Zero (Europe)", Some("F-Zero (Japan)")),
        game("F-Zero (USA)", Some("F-Zero (Japan)")),
        game("F-Zero (Europe) (Rev 1)", Some("F-Zero (Japan)")),
    ]);
    let forward = auto_matches(&dat.games, temp.path());
    let mut reversed = forward.clone();
    reversed.reverse();
    let mut shuffled = forward.clone();
    shuffled.swap(0, 3);
    shuffled.swap(1, 2);

    let build = |matches: Vec<DatArchiveMatch>| {
        build_playing_library_plan(&PlayingLibraryRequest {
            dat: &dat,
            matches,
            destination_root: temp.path().join("playing"),
            policy: default_policy(),
        })
        .expect("plan")
    };

    let from_forward = build(forward);
    let from_reversed = build(reversed);
    let from_shuffled = build(shuffled);

    assert_eq!(from_forward, from_reversed);
    assert_eq!(from_forward, from_shuffled);
    assert_eq!(
        elected_names(&from_forward),
        vec!["F-Zero (Europe) (Rev 1)"]
    );
}

#[test]
fn winner_and_loser_explanations_come_from_the_actual_election_values() {
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![
        game("Sonic the Hedgehog (Europe)", None),
        game(
            "Sonic the Hedgehog (USA)",
            Some("Sonic the Hedgehog (Europe)"),
        ),
    ]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(plan.elected_games.len(), 1);
    let elected = &plan.elected_games[0];
    assert_eq!(elected.dat_entry_name, "Sonic the Hedgehog (Europe)");
    // The winner's structured evidence is the real, verified region this
    // candidate's own catalogue name carries - not a placeholder.
    assert_eq!(elected.explanation.winner_evidence.regions, vec!["Europe"]);
    assert!(elected.explanation.winner_evidence.is_declared_parent);
    assert!(!elected.explanation.winner_evidence.is_declared_clone);

    assert_eq!(elected.explanation.rejected.len(), 1);
    let loser = &elected.explanation.rejected[0];
    assert_eq!(loser.dat_entry_name, "Sonic the Hedgehog (USA)");
    assert_eq!(loser.evidence.regions, vec!["USA"]);
    assert!(!loser.evidence.is_declared_parent);
    assert!(loser.evidence.is_declared_clone);
    assert!(!loser.reasons.is_empty());
}

#[test]
fn unknown_metadata_is_reported_as_unknown_not_inferred() {
    // Neither entry carries any recognised region/language/revision token
    // at all - the winner's own evidence must say so explicitly rather
    // than defaulting to some inferred value.
    let temp = tempfile::tempdir().expect("temp dir");
    let dat = synthetic_dat(vec![game("Untitled Game", None)]);
    let matches = auto_matches(&dat.games, temp.path());
    let request = PlayingLibraryRequest {
        dat: &dat,
        matches,
        destination_root: temp.path().join("playing"),
        policy: default_policy(),
    };
    let plan = build_playing_library_plan(&request).expect("plan");

    assert_eq!(plan.elected_games.len(), 1);
    let evidence = &plan.elected_games[0].explanation.winner_evidence;
    assert!(evidence.regions.is_empty());
    assert!(evidence.languages.is_empty());
    assert_eq!(evidence.revision, None);
}
