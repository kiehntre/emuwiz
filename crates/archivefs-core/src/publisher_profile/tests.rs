//! Synthetic test matrix for Publisher Profile Phase 1 - task section 22
//! (a representative, feasible subset; see
//! `docs/research/PUBLISHER_PROFILES_PHASE1.md` for which letters this
//! covers and which are covered instead by inherited 1G1R election
//! guarantees).

use std::path::PathBuf;

use super::es_de::{es_de_profile, resolve_es_de_platform_mapping};
use super::model::*;
use super::planner::{PublisherPlanRequest, build_publisher_plan};
use super::romm::{resolve_romm_platform_mapping, romm_profile};
use crate::platform_evidence_fusion::romm_platform_mapping::FrontendPlatformMapping;
use crate::playing_library::{
    CandidateEvidenceSummary, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPlan, PlayingLibraryPolicy,
};

fn elected(name: &str, source_file: &str, companions: Vec<&str>) -> ElectedGame {
    ElectedGame {
        dat_entry_name: name.to_string(),
        family_root_name: name.to_string(),
        explanation: ElectionExplanation {
            steps: vec!["the only election-eligible release in its family".to_string()],
            rejected: Vec::new(),
            winner_evidence: CandidateEvidenceSummary::unknown(),
        },
        launcher_operation: LinkedLibraryOperation {
            source_path: PathBuf::from(source_file),
            destination_path: PathBuf::from("/source-library").join(source_file),
        },
        companion_operations: companions
            .into_iter()
            .map(|companion| LinkedLibraryOperation {
                source_path: PathBuf::from(companion),
                destination_path: PathBuf::from("/source-library").join(companion),
            })
            .collect(),
    }
}

fn plan_with(games: Vec<ElectedGame>) -> PlayingLibraryPlan {
    PlayingLibraryPlan {
        destination_root: PathBuf::from("/source-library"),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: games.len(),
        families_examined: games.len(),
        elected_games: games,
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: 0,
        conflicts: Vec::new(),
        operations: Vec::new(),
        rejected_launchers: Vec::new(),
    }
}

fn mapped_romm(canonical: &str) -> PublisherPlatformMapping {
    resolve_romm_platform_mapping(canonical, &FrontendPlatformMapping::default(), None)
}

// --- A. simple single-file ROM -------------------------------------------

#[test]
fn a_simple_single_file_rom_is_safe_to_act() {
    let plan = plan_with(vec![elected(
        "Sensible Soccer (Europe)",
        "sensible-soccer.adf",
        vec![],
    )]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert_eq!(result.items.len(), 1);
    let item = &result.items[0];
    assert_eq!(item.safety, PublisherActionSafety::SafeToAct);
    assert_eq!(
        item.planned_destination,
        Some(PathBuf::from("/library/roms/amiga/sensible-soccer.adf"))
    );
    assert_eq!(item.planned_action.kind, PublisherActionKind::Symlink);
}

// --- B. CHD optical game (passthrough representation) --------------------

#[test]
fn b_chd_optical_game_representation_is_passed_through_unchanged() {
    let plan = plan_with(vec![elected(
        "Ridge Racer Type 4 (Japan)",
        "rr4.chd",
        vec![],
    )]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert!(
        result.items[0]
            .planned_destination
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .ends_with("rr4.chd")
    );
}

// --- C. multi-disc complete set ------------------------------------------

#[test]
fn c_multi_disc_complete_set_publishes_launcher_and_every_companion() {
    let plan = plan_with(vec![elected(
        "Some Game (Disc 1-3)",
        "game.m3u",
        vec!["disc1.chd", "disc2.chd", "disc3.chd"],
    )]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert_eq!(result.items[0].companions.len(), 3);
    assert_eq!(result.items[0].safety, PublisherActionSafety::SafeToAct);
}

// --- D. incomplete media set ----------------------------------------------

#[test]
fn d_incomplete_media_set_is_review_required_with_a_warning() {
    // A companion whose source path has no file name (e.g. `..`) cannot be
    // safely projected - the planner must not drop it silently.
    let mut game = elected("Broken Multi-Disc", "game.m3u", vec![]);
    game.companion_operations.push(LinkedLibraryOperation {
        source_path: PathBuf::from(".."),
        destination_path: PathBuf::from("/source-library/.."),
    });
    let plan = plan_with(vec![game]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert!(
        result.items[0]
            .warnings
            .iter()
            .any(|warning| matches!(warning, PublisherWarning::IncompleteMediaSet { .. }))
    );
}

// --- F/G/H. 1G1R election, region, and revision evidence pass through ----

#[test]
fn fgh_elected_release_identity_flows_through_unchanged() {
    // Region/revision selection is the existing 1G1R planner's job; this
    // planner only ever receives its output as one `ElectedGame` per
    // family, and must never re-derive or second-guess that election.
    let plan = plan_with(vec![elected(
        "Super Game (Europe) (Rev 1)",
        "super-game.rom",
        vec![],
    )]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(
        result.items[0].dat_entry_name,
        "Super Game (Europe) (Rev 1)"
    );
}

// --- I. same destination collision ----------------------------------------

#[test]
fn i_same_destination_collision_blocks_both_contenders() {
    let plan = plan_with(vec![
        elected("Game A (USA)", "duplicate-name.rom", vec![]),
        elected("Game A (Prototype)", "duplicate-name.rom", vec![]),
    ]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert!(
        result
            .items
            .iter()
            .all(|item| item.safety == PublisherActionSafety::Blocked)
    );
    assert!(
        result.items[0]
            .conflicts
            .iter()
            .any(|conflict| matches!(conflict, PublisherConflict::DestinationPlanCollision { .. }))
    );
}

// --- J. case-fold collision -------------------------------------------

#[test]
fn j_case_fold_collision_is_reported_distinctly_from_an_exact_collision() {
    let plan = plan_with(vec![
        elected("Game A", "Game.rom", vec![]),
        elected("Game B", "GAME.rom", vec![]),
    ]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert!(
        result
            .items
            .iter()
            .all(|item| item.safety == PublisherActionSafety::Blocked)
    );
    assert!(
        result.items[0]
            .conflicts
            .iter()
            .any(|conflict| matches!(conflict, PublisherConflict::CaseFoldCollision { .. }))
    );
}

// --- K. unsupported platform -----------------------------------------------

#[test]
fn k_unsupported_platform_is_marked_unsupported_never_guessed() {
    let plan = plan_with(vec![elected("Some Game", "game.rom", vec![])]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: PublisherPlatformMapping::Unsupported {
            canonical_platform_id: "Totally Fictional Platform".to_string(),
        },
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert_eq!(result.items[0].safety, PublisherActionSafety::Unsupported);
    assert!(result.items[0].planned_destination.is_none());
}

// --- L. unknown/unmapped platform -------------------------------------------

#[test]
fn l_unmapped_platform_is_review_required_never_guessed() {
    let plan = plan_with(vec![elected("Some Game", "game.rom", vec![])]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: PublisherPlatformMapping::Unmapped {
            canonical_platform_id: "Nonexistent Platform".to_string(),
        },
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert_eq!(
        result.items[0].safety,
        PublisherActionSafety::ReviewRequired
    );
    assert!(result.items[0].planned_destination.is_none());
}

// --- M/N. existing destination inspection (read-only) ----------------------

#[test]
fn m_existing_correct_destination_is_already_present() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("game.rom");
    std::fs::write(&source, b"data").unwrap();
    let library_root = dir.path().join("library");
    std::fs::create_dir_all(library_root.join("roms/amiga")).unwrap();
    let destination = library_root.join("roms/amiga/game.rom");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &destination).unwrap();

    let mut game = elected("Some Game", "placeholder.rom", vec![]);
    game.launcher_operation.source_path = source.clone();
    let plan = plan_with(vec![game]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: library_root.clone(),
        existing_destination_root: Some(&library_root),
    };
    let result = build_publisher_plan(&request).unwrap();
    #[cfg(unix)]
    assert_eq!(
        result.items[0].destination_state,
        DestinationState::AlreadyCorrect
    );
    // Read-only: nothing was created beyond what the test itself set up.
    assert!(destination.exists());
}

#[test]
fn n_existing_conflicting_destination_is_blocked_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("game.rom");
    std::fs::write(&source, b"data").unwrap();
    let library_root = dir.path().join("library");
    std::fs::create_dir_all(library_root.join("roms/amiga")).unwrap();
    let destination = library_root.join("roms/amiga/game.rom");
    std::fs::write(&destination, b"unrelated pre-existing content").unwrap();

    let mut game = elected("Some Game", "placeholder.rom", vec![]);
    game.launcher_operation.source_path = source;
    let plan = plan_with(vec![game]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: library_root.clone(),
        existing_destination_root: Some(&library_root),
    };
    let result = build_publisher_plan(&request).unwrap();
    assert_eq!(
        result.items[0].destination_state,
        DestinationState::Conflicting
    );
    assert_eq!(result.items[0].safety, PublisherActionSafety::Blocked);
    // Read-only: the pre-existing file's content is untouched.
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"unrelated pre-existing content"
    );
}

// --- O. BIOS-dependent system (Phase 1 honest limitation) -------------------

#[test]
fn o_bios_requirements_are_honestly_empty_in_phase_1() {
    let plan = plan_with(vec![elected("BIOS-dependent Game", "game.rom", vec![])]);
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapped_romm("Amiga"),
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    // Phase 1 does not yet consume any BIOS-detection input - this is a
    // documented limitation (see PUBLISHER_PROFILES_PHASE1.md), never a
    // silent, fabricated "no BIOS needed" claim.
    assert!(result.bios_requirements.is_empty());
}

// --- P. RomM mapping ---------------------------------------------------------

#[test]
fn p_romm_mapping_reuses_the_existing_production_table() {
    let mapping = mapped_romm("Amiga");
    assert_eq!(
        mapping,
        PublisherPlatformMapping::Mapped {
            canonical_platform_id: "Amiga".to_string(),
            folder: "amiga".to_string(),
        }
    );
}

// --- Q. ES-DE mapping ---------------------------------------------------------

#[test]
fn q_es_de_mapping_reuses_the_existing_reviewed_table() {
    let mapping = resolve_es_de_platform_mapping("TurboGrafx-16");
    assert_eq!(
        mapping,
        PublisherPlatformMapping::Mapped {
            canonical_platform_id: "TurboGrafx-16".to_string(),
            folder: "tg16".to_string(),
        }
    );
    let plan = plan_with(vec![elected("Bonk's Adventure", "bonk.pce", vec![])]);
    let request = PublisherPlanRequest {
        profile: &es_de_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapping,
        destination_root: PathBuf::from("/rompath"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).unwrap();
    assert_eq!(
        result.items[0].planned_destination,
        Some(PathBuf::from("/rompath/tg16/bonk.pce"))
    );
}

// --- Determinism (task section 23) ------------------------------------------

#[test]
fn determinism_same_input_produces_identical_hash_regardless_of_insertion_order() {
    let forward = plan_with(vec![
        elected("Game A", "a.rom", vec![]),
        elected("Game B", "b.rom", vec![]),
    ]);
    let backward = plan_with(vec![
        elected("Game B", "b.rom", vec![]),
        elected("Game A", "a.rom", vec![]),
    ]);
    let build = |plan: &PlayingLibraryPlan| {
        build_publisher_plan(&PublisherPlanRequest {
            profile: &romm_profile(),
            playing_library_plan: plan,
            platform_mapping: mapped_romm("Amiga"),
            destination_root: PathBuf::from("/library"),
            existing_destination_root: None,
        })
        .unwrap()
    };
    let first = build(&forward);
    let second = build(&backward);
    assert_eq!(first.plan_hash, second.plan_hash);
}

#[test]
fn determinism_changing_a_source_path_changes_the_hash() {
    let a = plan_with(vec![elected("Game A", "a.rom", vec![])]);
    let b = plan_with(vec![elected("Game A", "a-different.rom", vec![])]);
    let build = |plan: &PlayingLibraryPlan| {
        build_publisher_plan(&PublisherPlanRequest {
            profile: &romm_profile(),
            playing_library_plan: plan,
            platform_mapping: mapped_romm("Amiga"),
            destination_root: PathBuf::from("/library"),
            existing_destination_root: None,
        })
        .unwrap()
    };
    assert_ne!(build(&a).plan_hash, build(&b).plan_hash);
}

// --- Zero side effects (task section 25) ------------------------------------

mod zero_side_effects {
    use super::*;

    #[test]
    fn planning_never_creates_a_destination() {
        let dir = tempfile::tempdir().unwrap();
        let library_root = dir.path().join("library");
        // Deliberately do NOT create library_root - planning must not
        // create it either, even though every item's destination lives
        // under it.
        let plan = plan_with(vec![
            elected("Game A", "a.rom", vec![]),
            elected(
                "Multi Disc Game",
                "multi.m3u",
                vec!["disc1.chd", "disc2.chd"],
            ),
        ]);
        let request = PublisherPlanRequest {
            profile: &romm_profile(),
            playing_library_plan: &plan,
            platform_mapping: mapped_romm("Amiga"),
            destination_root: library_root.clone(),
            existing_destination_root: Some(&library_root),
        };
        let result = build_publisher_plan(&request).unwrap();
        assert!(!result.items.is_empty());
        assert!(
            !library_root.exists(),
            "Phase 1 planning must never create the destination root"
        );
    }

    #[test]
    fn no_source_module_references_a_filesystem_write_call() {
        // Structural proof, not just a runtime observation: none of this
        // module's own planning/profile source files may call a
        // filesystem write primitive. `destination_inspection.rs` is
        // exempt only inside its own `#[cfg(test)]` fixture setup, which
        // this check accounts for by only scanning non-test lines.
        let forbidden = [
            "fs::write(",
            "fs::create_dir",
            "fs::remove_",
            "fs::rename(",
            "fs::hard_link(",
            "fs::copy(",
            "::unix::fs::symlink(",
            "os::windows::fs::symlink",
        ];
        for file in [
            "model.rs",
            "planner.rs",
            "romm.rs",
            "es_de.rs",
            "destination_inspection.rs",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/publisher_profile")
                .join(file);
            let source = std::fs::read_to_string(&path).unwrap();
            let production_source = source.split("#[cfg(test)]").next().unwrap_or(&source);
            for token in forbidden {
                assert!(
                    !production_source.contains(token),
                    "{file} contains a forbidden write-shaped call: {token}"
                );
            }
        }
    }
}
