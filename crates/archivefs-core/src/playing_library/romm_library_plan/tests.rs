use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::dat::identity::DatPlatformConfidence;
use crate::dat::rename_apply::executor::{ApplyExecution, HardConflictMode, apply_transaction};
use crate::dat::rename_apply::journal::write_journal;
use crate::dat::rename_apply::preflight::DirectoryPolicy;
use crate::dat::rename_apply::rollback::rollback_transaction;
use crate::playing_library::{
    CandidateEvidenceSummary, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPolicy,
};

fn strong(platform: &str) -> DatPlatformIdentity {
    DatPlatformIdentity::Resolved {
        platform: platform.into(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: vec![],
    }
}

/// A minimal one-game Playing Library plan: `root` is the linked-library
/// destination root, `source` is the real archive file the launcher
/// symlink would point at.
fn plan(root: &Path, source: &Path, launcher_name: &str) -> PlayingLibraryPlan {
    let op = LinkedLibraryOperation {
        source_path: source.to_path_buf(),
        destination_path: root.join(launcher_name),
    };
    PlayingLibraryPlan {
        destination_root: root.to_path_buf(),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: 1,
        families_examined: 1,
        elected_games: vec![ElectedGame {
            dat_entry_name: "Game".into(),
            family_root_name: "Game".into(),
            explanation: ElectionExplanation {
                steps: vec![],
                rejected: vec![],
                winner_evidence: CandidateEvidenceSummary::unknown(),
            },
            launcher_operation: op,
            companion_operations: vec![],
        }],
        unresolved_groups: vec![],
        exclusions: vec![],
        singleton_families: 1,
        conflicts: vec![],
        operations: vec![],
        rejected_launchers: vec![],
    }
}

fn input(
    label: &str,
    root: &Path,
    source: &Path,
    launcher_name: &str,
    platform: &str,
) -> RommLibraryPlatformInput {
    RommLibraryPlatformInput {
        label: label.to_string(),
        plan: plan(root, source, launcher_name),
        identity: strong(platform),
    }
}

// --- basic mapping -----------------------------------------------------

#[test]
fn one_normal_game_maps_to_the_correct_romm_destination() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.gba");
    std::fs::write(&source, b"game").unwrap();
    let inputs = vec![input(
        "GBA",
        &temp.path().join("playing-gba"),
        &source,
        "game.gba",
        "Game Boy Advance",
    )];
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();

    assert!(plan.blocked_platforms.is_empty());
    assert_eq!(plan.entries.len(), 1);
    let entry = &plan.entries[0];
    assert_eq!(entry.dat_entry_name, "Game");
    assert_eq!(entry.romm_platform_slug, "gba");
    assert_eq!(entry.source_path, source);
    assert_eq!(
        entry.destination_path,
        temp.path().join("romm/roms/gba/game.gba")
    );
    assert_eq!(entry.operation, RommLibraryOperationKind::Symlink);
    assert!(entry.is_launcher);
    assert!(entry.blocked.is_none());
    assert_eq!(plan.ready_count(), 1);
    assert_eq!(plan.blocked_count(), 0);
}

#[test]
fn multiple_platforms_are_combined_into_one_plan() {
    let temp = tempfile::tempdir().unwrap();
    let gba_source = temp.path().join("gba/game.gba");
    std::fs::create_dir_all(gba_source.parent().unwrap()).unwrap();
    std::fs::write(&gba_source, b"gba game").unwrap();
    let ps3_source = temp.path().join("ps3/EBOOT.BIN");
    std::fs::create_dir_all(ps3_source.parent().unwrap()).unwrap();
    std::fs::write(&ps3_source, b"ps3 game").unwrap();

    let inputs = vec![
        input(
            "GBA library",
            &temp.path().join("playing-gba"),
            &gba_source,
            "game.gba",
            "Game Boy Advance",
        ),
        input(
            "PS3 library",
            &temp.path().join("playing-ps3"),
            &ps3_source,
            "EBOOT.BIN",
            "PS3",
        ),
    ];
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();

    assert!(plan.blocked_platforms.is_empty());
    assert_eq!(plan.entries.len(), 2);
    let slugs: Vec<&str> = plan
        .entries
        .iter()
        .map(|entry| entry.romm_platform_slug.as_str())
        .collect();
    assert!(slugs.contains(&"gba"));
    assert!(slugs.contains(&"ps3"));
    let platform_labels: Vec<&str> = plan
        .entries
        .iter()
        .map(|entry| entry.platform_label.as_str())
        .collect();
    assert!(platform_labels.contains(&"GBA library"));
    assert!(platform_labels.contains(&"PS3 library"));
}

#[test]
fn the_elected_1g1r_game_name_is_reused_not_rederived() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.gba");
    std::fs::write(&source, b"game").unwrap();
    let mut one_input = input(
        "GBA",
        &temp.path().join("playing"),
        &source,
        "game.gba",
        "Game Boy Advance",
    );
    // A distinctive elected name a filename/path guess could never produce -
    // proves the entry is carrying the real 1G1R election result through,
    // not re-deriving anything from the path.
    one_input.plan.elected_games[0].dat_entry_name =
        "Definitely Not A Filename Guess (Rev 2)".to_string();
    let plan = build_romm_library_plan(
        &[one_input],
        &temp.path().join("romm"),
        &TrustedRoots::none(),
    )
    .unwrap();
    assert_eq!(
        plan.entries[0].dat_entry_name,
        "Definitely Not A Filename Guess (Rev 2)"
    );
}

// --- fail-closed platform identity --------------------------------------

#[test]
fn unsupported_platform_fails_closed_as_a_blocked_platform() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.bin");
    std::fs::write(&source, b"game").unwrap();
    let inputs = vec![input(
        "Mystery console",
        &temp.path().join("playing"),
        &source,
        "game.bin",
        "Some Platform EmuWiz Has Never Heard Of",
    )];
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();
    assert!(plan.entries.is_empty());
    assert_eq!(plan.blocked_platforms.len(), 1);
    assert_eq!(plan.blocked_platforms[0].label, "Mystery console");
    assert_eq!(plan.blocked_count(), 1);
}

#[test]
fn ambiguous_platform_fails_closed_as_a_blocked_platform() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.bin");
    std::fs::write(&source, b"game").unwrap();
    let inputs = vec![RommLibraryPlatformInput {
        label: "Ambiguous source".to_string(),
        plan: plan(&temp.path().join("playing"), &source, "game.bin"),
        identity: DatPlatformIdentity::Ambiguous {
            candidates: Vec::new(),
        },
    }];
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();
    assert!(plan.entries.is_empty());
    assert_eq!(plan.blocked_platforms.len(), 1);
    assert_eq!(plan.blocked_platforms[0].label, "Ambiguous source");
}

#[test]
fn one_blocked_platform_does_not_abort_the_rest_of_the_collection() {
    let temp = tempfile::tempdir().unwrap();
    let good_source = temp.path().join("game.gba");
    std::fs::write(&good_source, b"game").unwrap();
    let bad_source = temp.path().join("mystery.bin");
    std::fs::write(&bad_source, b"mystery").unwrap();
    let inputs = vec![
        input(
            "GBA",
            &temp.path().join("playing-gba"),
            &good_source,
            "game.gba",
            "Game Boy Advance",
        ),
        input(
            "Mystery",
            &temp.path().join("playing-mystery"),
            &bad_source,
            "mystery.bin",
            "Totally Unknown Platform",
        ),
    ];
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].platform_label, "GBA");
    assert_eq!(plan.blocked_platforms.len(), 1);
    assert_eq!(plan.blocked_platforms[0].label, "Mystery");
}

// --- collisions -----------------------------------------------------

#[test]
fn two_platforms_mapped_to_the_same_slug_collide_as_a_duplicate_destination() {
    let temp = tempfile::tempdir().unwrap();
    let source_a = temp.path().join("a/game.gba");
    std::fs::create_dir_all(source_a.parent().unwrap()).unwrap();
    std::fs::write(&source_a, b"a").unwrap();
    let source_b = temp.path().join("b/game.gba");
    std::fs::create_dir_all(source_b.parent().unwrap()).unwrap();
    std::fs::write(&source_b, b"b").unwrap();

    // Two different source libraries both (mistakenly) mapped to the same
    // platform - a plausible novice mistake - produce the same launcher
    // filename under the same RomM slug.
    let inputs = vec![
        input(
            "GBA - drive A",
            &temp.path().join("playing-a"),
            &source_a,
            "game.gba",
            "Game Boy Advance",
        ),
        input(
            "GBA - drive B",
            &temp.path().join("playing-b"),
            &source_b,
            "game.gba",
            "Game Boy Advance",
        ),
    ];
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();
    assert_eq!(plan.entries.len(), 2);
    let blocked = plan
        .entries
        .iter()
        .find(|entry| entry.platform_label == "GBA - drive B")
        .unwrap();
    assert!(matches!(
        blocked.blocked,
        Some(RommLibraryBlockReason::DuplicateDestination { .. })
    ));
    let first = plan
        .entries
        .iter()
        .find(|entry| entry.platform_label == "GBA - drive A")
        .unwrap();
    assert!(first.blocked.is_none());
}

#[test]
fn an_existing_file_at_the_destination_is_reported_and_never_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.gba");
    std::fs::write(&source, b"game").unwrap();
    let romm_root = temp.path().join("romm");
    let occupied = romm_root.join("roms/gba/game.gba");
    std::fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    std::fs::write(&occupied, b"already here, unrelated").unwrap();

    let inputs = vec![input(
        "GBA",
        &temp.path().join("playing"),
        &source,
        "game.gba",
        "Game Boy Advance",
    )];
    let plan = build_romm_library_plan(&inputs, &romm_root, &TrustedRoots::none()).unwrap();
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(
        plan.entries[0].blocked,
        Some(RommLibraryBlockReason::DestinationOccupied)
    );
    assert_eq!(
        std::fs::read(&occupied).unwrap(),
        b"already here, unrelated"
    );
}

// --- source safety -----------------------------------------------------

#[test]
fn a_missing_source_is_reported_as_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("gone.gba");
    // Deliberately never created.
    let inputs = vec![input(
        "GBA",
        &temp.path().join("playing"),
        &source,
        "gone.gba",
        "Game Boy Advance",
    )];
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();
    assert_eq!(
        plan.entries[0].blocked,
        Some(RommLibraryBlockReason::MissingSource)
    );
}

#[cfg(unix)]
#[test]
fn an_untrusted_symlinked_source_is_reported_as_unsafe() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real.gba");
    std::fs::write(&real, b"game").unwrap();
    let link = temp.path().join("link.gba");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let inputs = vec![input(
        "GBA",
        &temp.path().join("playing"),
        &link,
        "link.gba",
        "Game Boy Advance",
    )];
    // No trusted roots configured - the fail-closed default.
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();
    assert!(matches!(
        plan.entries[0].blocked,
        Some(RommLibraryBlockReason::UnsafeSource { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_symlinked_source_inside_a_trusted_root_is_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real.gba");
    std::fs::write(&real, b"game").unwrap();
    let link = temp.path().join("link.gba");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let inputs = vec![input(
        "GBA",
        &temp.path().join("playing"),
        &link,
        "link.gba",
        "Game Boy Advance",
    )];
    let trusted = TrustedRoots::from_paths([temp.path()]);
    let plan = build_romm_library_plan(&inputs, &temp.path().join("romm"), &trusted).unwrap();
    assert!(plan.entries[0].blocked.is_none());
}

// --- determinism and preview safety --------------------------------------

#[test]
fn repeated_planning_over_the_same_inputs_is_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    let gba_source = temp.path().join("gba/game.gba");
    std::fs::create_dir_all(gba_source.parent().unwrap()).unwrap();
    std::fs::write(&gba_source, b"gba").unwrap();
    let ps3_source = temp.path().join("ps3/EBOOT.BIN");
    std::fs::create_dir_all(ps3_source.parent().unwrap()).unwrap();
    std::fs::write(&ps3_source, b"ps3").unwrap();
    let inputs = vec![
        input(
            "GBA",
            &temp.path().join("playing-gba"),
            &gba_source,
            "game.gba",
            "Game Boy Advance",
        ),
        input(
            "PS3",
            &temp.path().join("playing-ps3"),
            &ps3_source,
            "EBOOT.BIN",
            "PS3",
        ),
    ];
    let destination = temp.path().join("romm");
    let first = build_romm_library_plan(&inputs, &destination, &TrustedRoots::none()).unwrap();
    let second = build_romm_library_plan(&inputs, &destination, &TrustedRoots::none()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn planning_never_touches_the_filesystem() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.gba");
    std::fs::write(&source, b"game").unwrap();
    let romm_root = temp.path().join("romm");
    let inputs = vec![input(
        "GBA",
        &temp.path().join("playing"),
        &source,
        "game.gba",
        "Game Boy Advance",
    )];
    let _plan = build_romm_library_plan(&inputs, &romm_root, &TrustedRoots::none()).unwrap();
    assert!(
        !romm_root.exists(),
        "planning must never create the destination tree"
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"game");
    assert!(!source.is_symlink());
}

// --- apply reuses the existing transaction machinery ----------------------

#[test]
fn apply_reuses_the_existing_transaction_journal_and_rollback_and_leaves_originals_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.gba");
    std::fs::write(&source, b"original bytes").unwrap();
    let romm_root = temp.path().join("romm");
    let inputs = vec![input(
        "GBA",
        &temp.path().join("playing"),
        &source,
        "game.gba",
        "Game Boy Advance",
    )];
    let plan = build_romm_library_plan(&inputs, &romm_root, &TrustedRoots::none()).unwrap();
    assert_eq!(plan.ready_count(), 1);

    let visibility = RommVisibility::verified_same_path_bind(temp.path().to_path_buf()).unwrap();
    let mut transactions = build_romm_library_apply_transactions(&plan, &inputs, &visibility, 1);
    assert_eq!(transactions.len(), 1);
    let (label, result) = transactions.remove(0);
    assert_eq!(label, "GBA");
    let mut transaction = result.expect("the single ready platform must build a transaction");

    let journal_dir = temp.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    std::fs::create_dir_all(&romm_root.join("roms/gba")).unwrap();
    write_journal(&journal_dir, &transaction).unwrap();
    let approved_paths = transaction
        .entries
        .iter()
        .map(|entry| entry.source_path.to_string_lossy().into_owned())
        .collect();
    apply_transaction(&mut ApplyExecution {
        transaction: &mut transaction,
        approved_paths,
        current_generation: 1,
        trusted: TrustedRoots::from_paths([temp.path()]),
        journal_dir: journal_dir.clone(),
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &AtomicBool::new(false),
        directory_policy: DirectoryPolicy::SameFilesystem,
        allow_symlink_source: false,
    })
    .unwrap();

    let destination = romm_root.join("roms/gba/game.gba");
    assert!(
        destination.is_symlink(),
        "apply must create the planned symlink"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"original bytes");
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"original bytes",
        "the original archive must never be modified"
    );
    assert!(!source.is_symlink(), "the original must remain a real file");

    rollback_transaction(&mut transaction, &journal_dir, &AtomicBool::new(false)).unwrap();
    assert!(
        !destination.exists(),
        "rollback must restore the pre-apply state"
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"original bytes",
        "the original archive must still be untouched after rollback"
    );
}

#[test]
fn a_platform_already_blocked_at_plan_time_produces_no_apply_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.bin");
    std::fs::write(&source, b"mystery").unwrap();
    let inputs = vec![input(
        "Mystery",
        &temp.path().join("playing"),
        &source,
        "game.bin",
        "Totally Unknown Platform",
    )];
    let plan =
        build_romm_library_plan(&inputs, &temp.path().join("romm"), &TrustedRoots::none()).unwrap();
    assert_eq!(plan.blocked_platforms.len(), 1);
    let visibility = RommVisibility::unverified(None, None);
    let transactions = build_romm_library_apply_transactions(&plan, &inputs, &visibility, 1);
    assert!(
        transactions.is_empty(),
        "a platform already blocked at plan time must not be retried at apply time"
    );
}

// --- platform-specific layouts are unchanged ------------------------------

#[test]
fn ps3_nested_multi_file_layout_through_the_aggregator_matches_the_single_platform_projection() {
    let temp = tempfile::tempdir().unwrap();
    let eboot = temp.path().join("PS3_GAME/EBOOT.BIN");
    std::fs::create_dir_all(eboot.parent().unwrap()).unwrap();
    std::fs::write(&eboot, b"eboot").unwrap();
    let param = temp.path().join("PS3_GAME/PARAM.SFO");
    std::fs::write(&param, b"param").unwrap();

    let playing_root = temp.path().join("playing");
    let mut source_plan = plan(&playing_root, &eboot, "PS3_GAME/EBOOT.BIN");
    source_plan.elected_games[0]
        .companion_operations
        .push(LinkedLibraryOperation {
            source_path: param.clone(),
            destination_path: playing_root.join("PS3_GAME/PARAM.SFO"),
        });

    let direct = crate::playing_library::build_romm_projection(
        &source_plan,
        &strong("PS3"),
        temp.path().join("romm"),
    )
    .unwrap();

    let via_aggregator = build_romm_library_plan(
        &[RommLibraryPlatformInput {
            label: "PS3".to_string(),
            plan: source_plan,
            identity: strong("PS3"),
        }],
        &temp.path().join("romm"),
        &TrustedRoots::none(),
    )
    .unwrap();

    assert_eq!(via_aggregator.entries.len(), 2);
    let launcher = via_aggregator
        .entries
        .iter()
        .find(|entry| entry.is_launcher)
        .unwrap();
    let companion = via_aggregator
        .entries
        .iter()
        .find(|entry| !entry.is_launcher)
        .unwrap();
    assert_eq!(
        launcher.destination_path,
        direct.games[0].launcher.destination_path
    );
    assert_eq!(
        companion.destination_path,
        direct.games[0].companions[0].destination_path
    );
    assert!(launcher.blocked.is_none());
    assert!(companion.blocked.is_none());
}
