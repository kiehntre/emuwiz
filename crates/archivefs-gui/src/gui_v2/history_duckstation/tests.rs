//! GUI-v2 History for DuckStation cheat operations. Temp directories only:
//! the two-file change is really applied (and undone) in a scratch profile.

use std::collections::BTreeMap;
use std::fs;

use archivefs_core::PersistedArchive;
use archivefs_core::patch_manager::{
    DuckStationDiscTopology, DuckStationNativeApplyOptions, DuckStationNativeBatchCheat,
    DuckStationNativeCheat, DuckStationNativeOperation, DuckStationNativeReceipt,
    DuckStationNativeRequest, apply_duckstation_native_plan, plan_duckstation_native,
    undo_duckstation_native,
};

use super::*;

const SERIAL: &str = "SCUS-94601";
const APPLIED_AT: u64 = 1_790_000_000;

struct World {
    dir: tempfile::TempDir,
    profile: PathBuf,
    game: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("duckstation");
        fs::create_dir_all(profile.join("cheats")).unwrap();
        fs::create_dir_all(profile.join("gamesettings")).unwrap();
        fs::write(
            profile.join("settings.ini"),
            "[Main]\nSetupWizardIncomplete = false\n",
        )
        .unwrap();
        let game = dir.path().join("Game.chd");
        fs::write(&game, b"disc").unwrap();
        Self { dir, profile, game }
    }

    fn history(&self) -> PathBuf {
        self.dir.path().join("history")
    }

    fn backups(&self) -> PathBuf {
        self.dir.path().join("backups")
    }

    fn ini(&self) -> PathBuf {
        self.profile
            .join("gamesettings")
            .join(format!("{SERIAL}.ini"))
    }

    fn options(&self, id: &str) -> DuckStationNativeApplyOptions {
        DuckStationNativeApplyOptions {
            general_approved: true,
            replacement_approved: true,
            operation_id: id.into(),
            timestamp_unix_seconds: APPLIED_AT,
            history_root: self.history(),
            backup_root: self.backups(),
        }
    }

    fn apply(&self, id: &str) -> DuckStationNativeReceipt {
        let cheat = |name: &str, code: &str| DuckStationNativeBatchCheat {
            cheat: DuckStationNativeCheat {
                name: name.into(),
                metadata: BTreeMap::new(),
                comments: Vec::new(),
                code_lines: vec![code.into()],
            },
            enable: true,
        };
        let request = DuckStationNativeRequest {
            profile_root: self.profile.clone(),
            selected_game: self.game.clone(),
            verified_serials: vec![SERIAL.into()],
            disc_topology: DuckStationDiscTopology::SingleDisc,
            operation: DuckStationNativeOperation::AddMany {
                cheats: vec![
                    cheat("Infinite Lives", "80010000 0001"),
                    cheat("Unlock All Levels", "80020000 0002"),
                ],
            },
            acknowledge_database_shadowing: true,
        };
        let plan = plan_duckstation_native(&request).unwrap();
        apply_duckstation_native_plan(&plan, &self.options(id)).unwrap()
    }

    fn operations(&self) -> Vec<Operation> {
        load(&self.history(), &self.backups())
    }
}

fn library_with(world: &World) -> Library {
    Library::new(vec![PersistedArchive {
        id: 7,
        source_folder_id: 1,
        relative_path: "Game.chd".into(),
        absolute_path: world.game.clone(),
        archive_kind: "direct_game_image".into(),
        display_name: "Game".into(),
        normalized_name: "game".into(),
        size_bytes: Some(4),
        modified_time_unix_seconds: Some(1),
        platform: Some("PlayStation".into()),
        platform_source: Some("manual".into()),
        last_known_health: "pending".into(),
        last_seen_at: "2026-10-07".into(),
        last_verified_missing_at: None,
        identity_report: None,
    }])
}

fn render(operations: &[Operation], library: &Library) -> (egui::FullOutput, Option<Route>) {
    let ctx = egui::Context::default();
    ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
    let mut route = None;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut refresh = false;
            route = show(ui, operations, library, false, &mut refresh);
        });
    });
    (output, route)
}

fn text_of(output: &egui::FullOutput, needle: &str) -> bool {
    fn contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| contains(&clipped.shape, needle))
}

#[test]
fn one_batch_is_one_history_operation_with_its_cheats_and_real_apply_time() {
    let world = World::new();
    world.apply("op-two-cheats");
    let operations = world.operations();
    assert_eq!(
        operations.len(),
        1,
        "two journals, ONE operation: {operations:?}"
    );
    let operation = &operations[0];
    assert_eq!(operation.cheats, ["Infinite Lives", "Unlock All Levels"]);
    assert_eq!(operation.applied_at, Some(APPLIED_AT));
    assert_eq!(operation.outcome, Outcome::Completed);
    assert_eq!(operation.undo, UndoStatus::Available);
    assert_eq!(operation.archive.as_deref(), Some(world.game.as_path()));
    assert_eq!(operation.files.len(), 2);
    assert!(!operation.review_only);
}

#[test]
fn history_paints_one_row_and_a_working_deep_link_to_the_owning_workflow() {
    let world = World::new();
    world.apply("op-paint");
    let library = library_with(&world);
    let (output, route) = render(&world.operations(), &library);
    for expected in [
        "DuckStation cheats · SCUS-94601 (2 cheats)",
        "Infinite Lives",
        "Unlock All Levels",
        "Undo available",
        "Completed",
        "Open in Cheats to undo",
        "UTC",
    ] {
        assert!(text_of(&output, expected), "missing: {expected}");
    }
    assert!(route.is_none(), "painting never navigates");
    // The deep link goes to the Cheats workflow for exactly this game.
    assert_eq!(
        owning_workflow_route(&world.operations()[0], &library),
        Some(Route::Task {
            section: Section::Mods,
            game: 7
        })
    );
}

#[test]
fn an_operation_for_a_game_that_is_not_loaded_has_no_link_and_says_so() {
    let world = World::new();
    world.apply("op-unloaded");
    let empty = Library::new(Vec::new());
    assert_eq!(owning_workflow_route(&world.operations()[0], &empty), None);
    let (output, _) = render(&world.operations(), &empty);
    assert!(text_of(&output, "not in the loaded library"));
    assert!(!text_of(&output, "Open in Cheats to undo"));
}

#[test]
fn undone_state_is_reconstructed_from_disk_after_a_restart() {
    let world = World::new();
    let receipt = world.apply("op-restart");
    // A fresh load - nothing in memory - is what a restarted app does.
    assert_eq!(world.operations()[0].undo, UndoStatus::Available);
    undo_duckstation_native(&receipt, &world.options("op-restart-undo")).unwrap();
    let after = world.operations();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].undo, UndoStatus::AlreadyUndone);
    assert_eq!(after[0].outcome, Outcome::Undone);
    let (output, _) = render(&after, &library_with(&world));
    assert!(text_of(&output, "Already undone"));
    assert!(!text_of(&output, "Open in Cheats to undo"));
}

#[test]
fn a_changed_output_makes_the_row_needs_review_not_undoable() {
    let world = World::new();
    world.apply("op-changed");
    fs::write(world.ini(), "[Cheats]\nEnable = SomethingElse\n").unwrap();
    let operations = world.operations();
    assert!(
        matches!(&operations[0].undo, UndoStatus::NeedsReview(_)),
        "{:?}",
        operations[0].undo
    );
    let (output, _) = render(&operations, &library_with(&world));
    assert!(text_of(&output, "Undo needs review"));
    assert!(text_of(&output, "Open in Cheats to review"));
    assert!(!text_of(&output, "Open in Cheats to undo"));
}

#[test]
fn a_missing_parent_receipt_is_review_only_and_never_suggests_undo() {
    let world = World::new();
    let receipt = world.apply("op-orphan");
    fs::remove_file(receipt.receipt_path.as_ref().unwrap()).unwrap();
    let operations = world.operations();
    assert_eq!(operations.len(), 2, "two orphaned halves, each review-only");
    assert!(operations.iter().all(|operation| operation.review_only));
    assert!(
        operations
            .iter()
            .all(|operation| matches!(operation.undo, UndoStatus::NeedsReview(_)))
    );
    let (output, route) = render(&operations, &library_with(&world));
    assert!(text_of(&output, "incomplete record"));
    assert!(text_of(&output, "will not undo half of it"));
    assert!(!text_of(&output, "Open in Cheats"));
    assert!(route.is_none());
}

#[test]
fn a_damaged_receipt_is_review_only_too() {
    let world = World::new();
    let receipt = world.apply("op-damaged");
    fs::write(receipt.receipt_path.as_ref().unwrap(), b"{ not json").unwrap();
    let operations = world.operations();
    assert!(
        operations.iter().all(|operation| operation.review_only),
        "{operations:?}"
    );
    assert!(
        operations
            .iter()
            .all(|operation| !matches!(operation.undo, UndoStatus::Available))
    );
}

#[test]
fn a_failed_apply_that_rolled_itself_back_is_a_failure_with_nothing_to_undo() {
    let world = World::new();
    let receipt = world.apply("op-rolled");
    undo_duckstation_native(&receipt, &world.options("op-rolled-undo")).unwrap();
    fs::remove_file(receipt.receipt_path.as_ref().unwrap()).unwrap();
    let operations = world.operations();
    assert!(
        operations
            .iter()
            .all(|operation| operation.outcome == Outcome::Failed)
    );
    assert!(operations
        .iter()
        .all(|operation| matches!(&operation.undo, UndoStatus::Unavailable(reason) if reason.contains("rolled back"))));
}

#[test]
fn newest_operations_come_first() {
    let world = World::new();
    world.apply("op-a");
    // A second batch for another serial, applied later.
    let second_game = world.dir.path().join("Other.chd");
    fs::write(&second_game, b"disc").unwrap();
    let cheat = DuckStationNativeBatchCheat {
        cheat: DuckStationNativeCheat {
            name: "Other".into(),
            metadata: BTreeMap::new(),
            comments: Vec::new(),
            code_lines: vec!["80030000 0003".into()],
        },
        enable: true,
    };
    let request = DuckStationNativeRequest {
        profile_root: world.profile.clone(),
        selected_game: second_game,
        verified_serials: vec!["SLUS-00067".into()],
        disc_topology: DuckStationDiscTopology::SingleDisc,
        operation: DuckStationNativeOperation::AddMany {
            cheats: vec![cheat],
        },
        acknowledge_database_shadowing: true,
    };
    let plan = plan_duckstation_native(&request).unwrap();
    let mut options = world.options("op-b");
    options.timestamp_unix_seconds = APPLIED_AT + 100;
    apply_duckstation_native_plan(&plan, &options).unwrap();
    let operations = world.operations();
    assert_eq!(operations.len(), 2);
    assert_eq!(operations[0].serial, "SLUS-00067");
}

#[test]
fn the_mods_page_never_lists_the_child_journals_at_all() {
    // `ModHistory` is what the Mods page paints. Cheat adapters are not mods:
    // neither half of a DuckStation batch can appear there as "Ready to undo".
    let world = World::new();
    world.apply("op-mods");
    let history = archivefs_core::mod_history::ModHistory::from_shared_history(
        &world.history(),
        &world.backups(),
    );
    assert!(history.receipts.is_empty(), "{:?}", history.receipts);
}
