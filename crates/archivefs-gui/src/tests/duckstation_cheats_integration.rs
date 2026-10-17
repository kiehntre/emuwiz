//! DuckStation cheats inside the Cheats & Mods workflow, on a temporary
//! machine. The installation comes from the shared resolver Launch uses.
//! Nothing here touches the real home, config, history or emulator files.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::cheats_mods::duckstation_environment::test_environment::{Guard, TestMachine, install};
use crate::emulator_setup::profile_resolution::DuckStationResolutionInputs;
use archivefs_core::patch_manager::{
    DuckStationProfileDiscoveryRoots, GroupedHistoryRow, NativeGroupState,
};

const SERIAL: &str = "SCUS-94601";

struct Machine {
    dir: tempfile::TempDir,
    home: PathBuf,
    a_root: PathBuf,
    b_root: PathBuf,
    store_path: PathBuf,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn executable(path: &Path) {
    write(path, "#!/bin/sh\n");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A DuckStation game-database cache in the verified v33 layout.
fn game_database(sets: &[&[&str]], games: &[&str]) -> Vec<u8> {
    fn put(out: &mut Vec<u8>, text: &str) {
        out.extend((text.len() as u32).to_le_bytes());
        out.extend(text.as_bytes());
    }
    let mut out = Vec::new();
    out.extend(b"HLCE");
    out.extend(33_u32.to_le_bytes());
    out.extend([0_u8; 16]);
    out.extend((sets.len() as u32).to_le_bytes());
    out.extend((games.len() as u32).to_le_bytes());
    out.extend(0_u32.to_le_bytes());
    for (index, serials) in sets.iter().enumerate() {
        put(&mut out, &format!("Set {index}"));
        put(&mut out, "");
        put(&mut out, "");
        put(&mut out, &format!("Set {index} (Europe)"));
        out.extend((serials.len() as u32).to_le_bytes());
        for serial in *serials {
            put(&mut out, serial);
        }
    }
    for serial in games {
        put(&mut out, serial);
        put(&mut out, "A Title");
        out.extend([0_u8; 24]);
    }
    out
}

/// A cache of plausible size (the reader refuses stubs): the three-disc set,
/// our game, and filler sets/games.
fn plausible_game_database() -> Vec<u8> {
    let mut sets: Vec<Vec<String>> = vec![vec![
        "SCUS-94163".into(),
        "SCUS-94164".into(),
        "SCUS-94165".into(),
    ]];
    sets.extend((0..120).map(|i| {
        vec![
            format!("SLPS-{:05}", 10_000 + i * 2),
            format!("SLPS-{:05}", 10_001 + i * 2),
        ]
    }));
    let set_refs: Vec<Vec<&str>> = sets
        .iter()
        .map(|s| s.iter().map(String::as_str).collect())
        .collect();
    let set_slices: Vec<&[&str]> = set_refs.iter().map(Vec::as_slice).collect();
    let mut games: Vec<String> = vec![
        SERIAL.into(),
        "SCUS-94163".into(),
        "SCUS-94164".into(),
        "SCUS-94165".into(),
    ];
    games.extend((0..2_100).map(|i| format!("SLUS-{:05}", 20_000 + i)));
    let game_refs: Vec<&str> = games.iter().map(String::as_str).collect();
    game_database(&set_slices, &game_refs)
}

fn machine() -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let a_root = home.join("Applications/DuckStation");
    let a_exe = a_root.join("DuckStation.AppImage");
    executable(&a_exe);
    write(&a_root.join("portable.txt"), "");
    write(
        &a_root.join("settings.ini"),
        "[Main]\nSetupWizardIncomplete = false\n\n[BIOS]\nSearchDirectory = ../../../../does/not/exist/psx\n\n[Cheats]\nLoadCheatsFromDatabase = false\n",
    );
    write(
        &a_root.join("playtime.dat"),
        "SCUS-94601   341   1783636844\n",
    );
    fs::create_dir_all(a_root.join("cheats")).unwrap();
    fs::create_dir_all(a_root.join("gamesettings")).unwrap();
    // This install's own game database: a three-disc set and our game.
    write(&a_root.join("cache/x"), "");
    fs::remove_file(a_root.join("cache/x")).unwrap();
    fs::write(a_root.join("cache/gamedb.cache"), plausible_game_database()).unwrap();
    let b_exe = home.join("Applications/emulators/DuckStation.AppImage");
    executable(&b_exe);
    let b_root = home.join(".local/share/duckstation");
    write(&b_root.join("settings.ini"), "[Main]\nOther = 1\n");
    Machine {
        store_path: dir.path().join("selection.toml"),
        dir,
        home,
        a_root,
        b_root,
    }
}

impl Machine {
    fn duck_roots(&self) -> DuckStationProfileDiscoveryRoots {
        DuckStationProfileDiscoveryRoots {
            home: self.home.clone(),
            xdg_config_home: self.home.join(".config"),
            xdg_data_home: self.home.join(".local/share"),
            xdg_config_home_explicit: false,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            path_override: Some(Vec::new()),
        }
    }

    fn inputs(&self) -> DuckStationResolutionInputs {
        DuckStationResolutionInputs {
            roots: self.duck_roots(),
            desktop_directories: Vec::new(),
            selection_path: Some(self.store_path.clone()),
        }
    }

    fn install(&self) -> Guard {
        install(TestMachine {
            duckstation: Some(self.inputs()),
            history_root: self.dir.path().join("history"),
            backup_root: self.dir.path().join("backups"),
        })
    }
}

// ---------------------------------------------------------------------------
// DuckStation cheats: the whole workflow, inside Cheats & Mods
// ---------------------------------------------------------------------------

fn ps1_identity(
    workflow: &CheatWorkflowState,
    serial: &str,
) -> (GameIdentityRequest, GameIdentityReport) {
    let request = GameIdentityRequest {
        archive_path: workflow.archive_path.clone(),
        platform: workflow.platform.clone(),
        adapter: CheatEmulatorAdapter::DuckStation,
    };
    let report = GameIdentityReport {
        archive_path: workflow.archive_path.clone(),
        platform: archivefs_core::game_identity::IdentityPlatform::PlayStation,
        format: IdentityImageFormat::Iso,
        evidence: vec![archivefs_core::game_identity::IdentityEvidence {
            kind: IdentityKind::Ps1Serial,
            status: IdentityStatus::Verified,
            value: Some(serial.to_string()),
            confidence: archivefs_core::game_identity::IdentityConfidence::ExactBytes,
            provenance: archivefs_core::game_identity::IdentityProvenance {
                archive_path: workflow.archive_path.clone(),
                member_path: None,
                member_index: None,
                method: "test fixture".to_string(),
            },
            diagnostic: "test fixture".to_string(),
        }],
        warnings: Vec::new(),
        bytes_read: 512,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    (request, report)
}

fn app_at_the_picker(machine: &Machine, serial: &str, ticked: &[u32]) -> ArchiveFsApp {
    app_at_the_picker_with(
        machine,
        serial,
        "cheats = 3\n\
         cheat0_desc = \"Infinite health\"\ncheat0_code = \"80010000 0001\"\n\
         cheat1_desc = \"Infinite ammo\"\ncheat1_code = \"80020000 0002\"\n\
         cheat2_desc = \"Max money\"\ncheat2_code = \"80030000 0003\"\n",
        ticked,
    )
}

fn app_at_the_picker_with(
    machine: &Machine,
    serial: &str,
    cht: &str,
    ticked: &[u32],
) -> ArchiveFsApp {
    let game = machine.dir.path().join("game.chd");
    write(&game, "disc image");
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut record = record(game.to_str().unwrap(), MountState::Pending);
        record.identity.platform = Some("PSX".to_string());
        data.records.push(record);
    }
    app.cheat_emulator_selections.insert(
        game.clone(),
        archivefs_core::patch_manager::CheatRouteTarget::standalone("duckstation"),
    );
    assert!(app.prepare_cheats_mods_workspace(game.clone()));
    let workflow = app.cheat_workflow.as_mut().unwrap();
    assert_eq!(workflow.adapter, CheatEmulatorAdapter::DuckStation);
    workflow.identity = CheatStepResource::Ready(ps1_identity(workflow, serial));
    let document = archivefs_core::patch_manager::parse_cht_text(cht).unwrap();
    let mut selection = CheatSelection::from_document(&document);
    for index in ticked {
        assert!(selection.set_selected(*index, true));
        assert!(selection.set_enabled(*index, true));
    }
    workflow.candidate_selection = Some(CheatCandidateSelection {
        candidate: CheatCandidate {
            source_evidence: Vec::new(),
            catalogue_relative_path: "Sony - PlayStation/Game.cht".to_string(),
            display_name: "Game".to_string(),
            platform: Some("PSX".to_string()),
            region: None,
            revision: None,
            classification: CheatCandidateClassification::Strong,
            confidence_score: 700,
            evidence: Vec::new(),
            cheat_count: 3,
            source_file_hash: None,
            auto_selectable: false,
            manually_selectable: true,
        },
        loaded: LoadedCandidate {
            absolute_path: PathBuf::from("/catalogue/Game.cht"),
            digest: "a".repeat(64),
            document,
        },
        selection,
    });
    app
}

fn settle_transaction(app: &mut ArchiveFsApp) {
    for _ in 0..400 {
        app.poll_duckstation_transaction();
        if !matches!(
            app.cheat_workflow.as_ref().unwrap().transaction,
            CheatTransactionState::NativeApplying { .. }
        ) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the apply never finished");
}

fn load_history(app: &mut ArchiveFsApp, ctx: &egui::Context) {
    app.refresh_shared_history(ctx.clone());
    for _ in 0..400 {
        app.poll_shared_history();
        if matches!(app.shared_history, SharedHistoryState::Ready(_)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("history never loaded");
}

#[test]
fn duckstation_cheats_install_a_batch_in_one_operation_and_undo_it_from_one_history_row() {
    let machine = machine();
    let _env = machine.install();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0, 1, 2]);
    let ctx = egui::Context::default();

    // The cheats land in the AUTO-resolved PORTABLE profile.
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(
        workflow.duckstation.resolved.as_ref().unwrap().profile_root,
        machine.a_root
    );

    // Non-DAT proof: the install's own game database vouches for one disc.
    app.poll_duckstation_transaction();
    let proof = &app
        .cheat_workflow
        .as_ref()
        .unwrap()
        .duckstation
        .disc_proof
        .as_ref()
        .unwrap()
        .proof;
    assert!(
        matches!(
            proof,
            archivefs_core::patch_manager::DiscTopologyProof::SingleDisc { .. }
        ),
        "{proof:?}"
    );

    // DuckStation may also load community cheats with these names: until the
    // person answers that, the preview is refused with that exact reason.
    app.start_duckstation_install_preview();
    let CheatStepResource::Ready(first) = &app.cheat_workflow.as_ref().unwrap().preview else {
        panic!();
    };
    assert!(matches!(
        &first.outcome,
        CheatPreviewOutcome::Failed(CheatPreviewFailure::DuckStationNative(
            archivefs_core::patch_manager::DuckStationNativeRefusal::DatabaseShadowingUnknown
        ))
    ));
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .duckstation
        .acknowledge_database_shadowing = true;

    // ONE preview for all three ticked cheats, naming both files.
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!("preview not ready");
    };
    let generated = response
        .duckstation_generated
        .as_ref()
        .expect("a native plan");
    assert_eq!(generated.plan.preview.cheats_added.len(), 3);
    assert!(generated.plan.preview.cht_path.starts_with(&machine.a_root));
    assert!(
        generated
            .plan
            .preview
            .game_ini_path
            .starts_with(&machine.a_root)
    );
    assert!(
        !generated.plan.preview.cht_path.exists(),
        "preview writes nothing"
    );

    // ONE review, ONE apply.
    app.review_duckstation_apply();
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().transaction,
        CheatTransactionState::NativeReview { .. }
    ));
    app.start_duckstation_apply(ctx.clone());
    settle_transaction(&mut app);
    let CheatTransactionState::NativeResult { receipt, .. } =
        &app.cheat_workflow.as_ref().unwrap().transaction
    else {
        panic!("apply did not succeed");
    };
    assert_eq!(receipt.cheats_added.len(), 3);
    let cht = fs::read_to_string(machine.a_root.join(format!("cheats/{SERIAL}.cht"))).unwrap();
    let ini =
        fs::read_to_string(machine.a_root.join(format!("gamesettings/{SERIAL}.ini"))).unwrap();
    assert!(cht.contains("[Infinite health]") && cht.contains("[Max money]"));
    assert!(ini.contains("Enable = Infinite ammo"));
    assert!(
        !machine.b_root.join("cheats").exists(),
        "the unused native profile is untouched"
    );

    // ONE grouped history row; the two child journals are not listed.
    load_history(&mut app, &ctx);
    let SharedHistoryState::Ready(loaded) = &app.shared_history else {
        panic!();
    };
    assert_eq!(loaded.report.journals.len(), 2);
    assert_eq!(loaded.rows.len(), 1);
    let GroupedHistoryRow::DuckStationNative(group) = &loaded.rows[0] else {
        panic!("{:?}", loaded.rows);
    };
    assert_eq!(group.state, NativeGroupState::UndoAvailable);
    let receipt_path = group.receipt.receipt_path.clone().unwrap();

    let mut rollback = SharedRollbackState::Idle;
    let mut history = OperationHistory::default();
    let mut filters = HistoryLogFilters::default();
    let mut clipboard = InMemoryClipboard::default();
    let render_ctx = egui::Context::default();
    render_ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
    let output = render_ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_history_logs_page(
                ui,
                &app.shared_history,
                &mut rollback,
                None,
                &mut history,
                &mut filters,
                &mut clipboard,
                &mut None,
                &mut String::new(),
                &mut None,
                false,
            );
        });
    });
    assert!(rendered_text_contains(
        &output,
        "DuckStation cheats · SCUS-94601 (3 cheats)"
    ));
    assert!(rendered_text_contains(&output, "Preview undo"));
    assert!(rendered_text_contains(
        &output,
        "cannot be undone separately"
    ));

    // ONE undo, through the adapter's own undo (INI first).
    app.start_native_undo_preview(receipt_path);
    assert!(matches!(
        &app.shared_rollback,
        SharedRollbackState::NativeReview { preview, .. } if preview.available
    ));
    app.start_shared_rollback(ctx.clone());
    for _ in 0..400 {
        app.poll_shared_rollback();
        if matches!(app.shared_rollback, SharedRollbackState::NativeDone(_)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(matches!(
        app.shared_rollback,
        SharedRollbackState::NativeDone(_)
    ));
    assert!(!machine.a_root.join(format!("cheats/{SERIAL}.cht")).exists());
    assert!(
        !machine
            .a_root
            .join(format!("gamesettings/{SERIAL}.ini"))
            .exists()
    );
    // History now says so, remembered from disk.
    load_history(&mut app, &ctx);
    let SharedHistoryState::Ready(loaded) = &app.shared_history else {
        panic!();
    };
    let GroupedHistoryRow::DuckStationNative(group) = &loaded.rows[0] else {
        panic!();
    };
    assert_eq!(group.state, NativeGroupState::AlreadyUndone);
}

#[test]
fn a_game_without_a_dat_match_is_allowed_when_the_install_vouches_for_it_and_a_known_set_is_refused()
 {
    let machine = machine();
    let _env = machine.install();
    // Serial in a multi-disc set -> refused with the specific reason.
    let mut app = app_at_the_picker(&machine, "SCUS-94164", &[0]);
    app.poll_duckstation_transaction();
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!();
    };
    assert!(matches!(
        &response.outcome,
        CheatPreviewOutcome::Failed(CheatPreviewFailure::DuckStationNative(
            archivefs_core::patch_manager::DuckStationNativeRefusal::UnsupportedMultiDisc
        ))
    ));
    // A serial the database has never heard of -> refused, naming the missing proof.
    let mut app = app_at_the_picker(&machine, "SLUS-99999", &[0]);
    app.poll_duckstation_transaction();
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!();
    };
    let CheatPreviewOutcome::Failed(CheatPreviewFailure::DuckStationNative(refusal)) =
        &response.outcome
    else {
        panic!();
    };
    let text = refusal.explain();
    assert!(text.contains("does not list this serial"), "{text}");
    assert!(text.contains("no trusted DAT set"), "{text}");
}

#[test]
fn conflicts_in_the_batch_are_all_shown_and_nothing_is_written() {
    let machine = machine();
    let _env = machine.install();
    // A different cheat with the same name already lives in the game's file.
    write(
        &machine.a_root.join(format!("cheats/{SERIAL}.cht")),
        "[Infinite health]\nType = Gameshark\nActivation = EndFrame\n80999999 FFFF\n",
    );
    let before = fs::read_to_string(machine.a_root.join(format!("cheats/{SERIAL}.cht"))).unwrap();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0, 1]);
    app.poll_duckstation_transaction();
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!();
    };
    let CheatPreviewOutcome::Failed(CheatPreviewFailure::DuckStationNative(refusal)) =
        &response.outcome
    else {
        panic!("{:?}", response.outcome);
    };
    assert!(refusal.explain().contains("Infinite health"));
    assert_eq!(
        fs::read_to_string(machine.a_root.join(format!("cheats/{SERIAL}.cht"))).unwrap(),
        before
    );
    assert!(
        !machine
            .a_root
            .join(format!("gamesettings/{SERIAL}.ini"))
            .exists()
    );
}

#[test]
fn the_duckstation_section_is_part_of_the_cheats_page_and_shows_both_files_and_the_proof() {
    let machine = machine();
    let _env = machine.install();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0, 1, 2]);
    app.poll_duckstation_transaction();
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .duckstation
        .acknowledge_database_shadowing = true;
    app.start_duckstation_install_preview();
    let ctx = egui::Context::default();
    ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let facts = duckstation_facts(workflow, None);
            let _ = show_duckstation_workflow(ui, workflow, &facts, false, &mut clipboard);
        });
    });
    for expected in [
        "Layout: Portable (settings beside the program)",
        "What will change",
        &format!("{SERIAL}.cht"),
        &format!("{SERIAL}.ini"),
        "Single disc confirmed",
        "File 1 of 2",
        "File 2 of 2",
        "Review changes",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing: {expected}"
        );
    }
}

#[test]
fn nothing_in_the_integration_repaired_or_created_dead_bios_or_profiles() {
    let machine = machine();
    let _env = machine.install();
    let ini_before = fs::read_to_string(machine.a_root.join("settings.ini")).unwrap();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0]);
    app.poll_duckstation_transaction();
    app.start_duckstation_install_preview();
    assert_eq!(
        fs::read_to_string(machine.a_root.join("settings.ini")).unwrap(),
        ini_before
    );
    assert!(!machine.home.join("does").exists());
    assert!(!machine.b_root.join("cheats").exists());
}

// ---------------------------------------------------------------------------
// The user journey: two cheats, one batch, one History entry, one Undo
// ---------------------------------------------------------------------------

const TWO: &str = "cheats = 2\n\
    cheat0_desc = \"Infinite Lives\"\ncheat0_code = \"80010000 0001\"\n\
    cheat1_desc = \"Unlock All Levels\"\ncheat1_code = \"80020000 0002\"\n";

fn apply_two_cheats(machine: &Machine) -> (ArchiveFsApp, PathBuf) {
    let mut app = app_at_the_picker_with(machine, SERIAL, TWO, &[0, 1]);
    let ctx = egui::Context::default();
    app.poll_duckstation_transaction();
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .duckstation
        .acknowledge_database_shadowing = true;
    app.start_duckstation_install_preview();
    app.review_duckstation_apply();
    app.start_duckstation_apply(ctx);
    settle_transaction(&mut app);
    let CheatTransactionState::NativeResult { receipt, .. } =
        &app.cheat_workflow.as_ref().unwrap().transaction
    else {
        panic!("apply did not succeed");
    };
    assert_eq!(
        receipt.cheats_added,
        ["Infinite Lives", "Unlock All Levels"]
    );
    let path = receipt.receipt_path.clone().unwrap();
    (app, path)
}

fn files(machine: &Machine) -> (PathBuf, PathBuf) {
    (
        machine.a_root.join(format!("cheats/{SERIAL}.cht")),
        machine.a_root.join(format!("gamesettings/{SERIAL}.ini")),
    )
}

fn settle_undo(app: &mut ArchiveFsApp) {
    for _ in 0..400 {
        app.poll_duckstation_transaction();
        if !matches!(
            app.cheat_workflow.as_ref().unwrap().duckstation.undo,
            NativeUndoState::Applying { .. }
        ) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the undo never finished");
}

#[test]
fn a_two_cheat_batch_has_one_result_card_undo_that_restores_both_files() {
    let machine = machine();
    let _env = machine.install();
    let (cht, ini) = files(&machine);
    let (mut app, receipt_path) = apply_two_cheats(&machine);
    assert!(cht.exists() && ini.exists());
    // The result card offers THE undo (not "open history").
    let ctx = egui::Context::default();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let facts = duckstation_facts(workflow, None);
            let _ = show_duckstation_workflow(ui, workflow, &facts, false, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(&output, "Undo this change"));
    assert!(!rendered_text_contains(&output, "Open History to undo"));

    app.start_duckstation_undo_preview(receipt_path);
    assert!(matches!(
        &app.cheat_workflow.as_ref().unwrap().duckstation.undo,
        NativeUndoState::Review { preview, .. } if preview.available
    ));
    app.confirm_duckstation_undo(ctx);
    settle_undo(&mut app);
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().duckstation.undo,
        NativeUndoState::Done(_)
    ));
    assert!(
        !cht.exists() && !ini.exists(),
        "both files are back to their previous (absent) state"
    );
    // The stale result card was cleared and the batch now reads as undone.
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().transaction,
        CheatTransactionState::Idle
    ));
    app.refresh_duckstation_history();
    let earlier = &app.cheat_workflow.as_ref().unwrap().duckstation.earlier;
    assert_eq!(earlier.len(), 1);
    assert_eq!(earlier[0].state, NativeGroupState::AlreadyUndone);
}

#[test]
fn after_an_application_restart_the_grouped_undo_is_rebuilt_from_disk_and_works() {
    let machine = machine();
    let _env = machine.install();
    let (cht, ini) = files(&machine);
    let (first_session, _) = apply_two_cheats(&machine);
    drop(first_session);

    // "Restart": a brand-new app that knows nothing but what is on disk.
    let mut app = app_at_the_picker_with(&machine, SERIAL, TWO, &[0, 1]);
    app.poll_duckstation_transaction();
    let earlier = &app.cheat_workflow.as_ref().unwrap().duckstation.earlier;
    assert_eq!(earlier.len(), 1, "one logical entry, not two journals");
    assert_eq!(earlier[0].state, NativeGroupState::UndoAvailable);
    assert_eq!(earlier[0].receipt.cheats_added.len(), 2);
    let path = earlier[0].receipt.receipt_path.clone().unwrap();
    app.start_duckstation_undo_preview(path);
    app.confirm_duckstation_undo(egui::Context::default());
    settle_undo(&mut app);
    assert!(!cht.exists() && !ini.exists());
}

#[test]
fn undo_refuses_when_an_output_was_changed_and_never_overwrites_it() {
    let machine = machine();
    let _env = machine.install();
    let (cht, ini) = files(&machine);
    let (mut app, path) = apply_two_cheats(&machine);
    let edited = format!("{}\n; my own edit\n", fs::read_to_string(&ini).unwrap());
    fs::write(&ini, &edited).unwrap();
    app.start_duckstation_undo_preview(path);
    let NativeUndoState::Review { preview, .. } =
        &app.cheat_workflow.as_ref().unwrap().duckstation.undo
    else {
        panic!();
    };
    assert!(!preview.available, "the preview already says why");
    app.confirm_duckstation_undo(egui::Context::default());
    // Nothing ran: still reviewing, both files exactly as they were.
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().duckstation.undo,
        NativeUndoState::Review { .. }
    ));
    assert_eq!(fs::read_to_string(&ini).unwrap(), edited);
    assert!(cht.exists());
}

#[test]
fn a_preview_gone_stale_is_rechecked_at_confirm_time_and_blocks_instead_of_running() {
    let machine = machine();
    let _env = machine.install();
    let (cht, _ini) = files(&machine);
    let (mut app, path) = apply_two_cheats(&machine);
    app.start_duckstation_undo_preview(path);
    assert!(matches!(
        &app.cheat_workflow.as_ref().unwrap().duckstation.undo,
        NativeUndoState::Review { preview, .. } if preview.available
    ));
    // The cheat file changes AFTER the person saw an "available" preview.
    let edited = format!(
        "{}\n; edited after the preview\n",
        fs::read_to_string(&cht).unwrap()
    );
    fs::write(&cht, &edited).unwrap();
    app.confirm_duckstation_undo(egui::Context::default());
    let NativeUndoState::Review { preview, .. } =
        &app.cheat_workflow.as_ref().unwrap().duckstation.undo
    else {
        panic!("the stale confirmation must not run");
    };
    assert!(!preview.available);
    assert_eq!(fs::read_to_string(&cht).unwrap(), edited);
}

#[test]
fn a_missing_or_damaged_receipt_leaves_no_workflow_undo_for_half_a_change() {
    let machine = machine();
    let _env = machine.install();
    let (mut app, path) = apply_two_cheats(&machine);
    fs::write(&path, b"{ damaged").unwrap();
    app.cheat_workflow.as_mut().unwrap().duckstation.earlier_for = None;
    app.poll_duckstation_transaction();
    assert!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .duckstation
            .earlier
            .is_empty(),
        "no grouped, undoable entry without its receipt"
    );
    app.start_duckstation_undo_preview(path);
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().duckstation.undo,
        NativeUndoState::Failed(_)
    ));
}

#[test]
fn an_edit_made_between_preview_and_apply_blocks_the_apply_and_writes_nothing() {
    let machine = machine();
    let _env = machine.install();
    let mut app = app_at_the_picker_with(&machine, SERIAL, TWO, &[0, 1]);
    app.poll_duckstation_transaction();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.duckstation.acknowledge_database_shadowing = true;
    app.start_duckstation_install_preview();
    app.review_duckstation_apply();
    // The game's cheat file appears after the person reviewed the preview.
    let (cht, ini) = files(&machine);
    write(
        &cht,
        "[Someone Else's]\nType = Gameshark\nActivation = EndFrame\n80999999 FFFF\n",
    );
    app.start_duckstation_apply(egui::Context::default());
    settle_transaction(&mut app);
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().transaction,
        CheatTransactionState::NativeFailed { .. }
    ));
    assert!(fs::read_to_string(&cht).unwrap().contains("Someone Else's"));
    assert!(!ini.exists());
}

// ---------------------------------------------------------------------------
// Identity: wrong serial, region mismatch
// ---------------------------------------------------------------------------

fn set_candidate_region(app: &mut ArchiveFsApp, region: &str) {
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .candidate_selection
        .as_mut()
        .unwrap()
        .candidate
        .region = Some(region.to_string());
}

#[test]
fn a_cheat_file_for_another_region_is_refused_before_anything_is_planned() {
    let machine = machine();
    let _env = machine.install();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0]);
    set_candidate_region(&mut app, "Europe");
    app.poll_duckstation_transaction();
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .duckstation
        .acknowledge_database_shadowing = true;
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(matches!(workflow.preview, CheatStepResource::NotLoaded));
    let (_, refusal) = workflow.duckstation.region_refusal.as_ref().unwrap();
    assert!(
        refusal.contains("Europe") && refusal.contains("USA"),
        "{refusal}"
    );
    assert!(!machine.a_root.join(format!("cheats/{SERIAL}.cht")).exists());
    assert!(
        !machine
            .a_root
            .join(format!("gamesettings/{SERIAL}.ini"))
            .exists()
    );
}

#[test]
fn a_cheat_file_that_names_the_disc_region_among_others_is_previewed() {
    let machine = machine();
    let _env = machine.install();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0]);
    set_candidate_region(&mut app, "USA, Europe");
    app.poll_duckstation_transaction();
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .duckstation
        .acknowledge_database_shadowing = true;
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(workflow.duckstation.region_refusal.is_none());
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!("no preview");
    };
    assert!(
        response.duckstation_generated.is_some(),
        "{:?}",
        response.outcome
    );
}

#[test]
fn region_rules_follow_the_serial_prefix_and_ignore_unrecognised_text() {
    use crate::cheats_mods::duckstation_region_conflict as conflict;
    assert!(conflict(Some("SLES-00001"), Some("USA")).is_some());
    assert!(conflict(Some("SLPS-00001"), Some("Europe")).is_some());
    assert!(conflict(Some("SCUS-94601"), Some("USA")).is_none());
    assert!(conflict(Some("SLES-00001"), Some("Europe, Australia")).is_none());
    // Nothing recognisable on one side: the region check has no opinion.
    assert!(conflict(Some("SCUS-94601"), Some("World")).is_none());
    assert!(conflict(Some("XXXX-00001"), Some("USA")).is_none());
    assert!(conflict(None, Some("USA")).is_none());
    assert!(conflict(Some("SCUS-94601"), None).is_none());
}

#[test]
fn a_serial_nothing_can_prove_single_disc_is_refused_and_nothing_is_written() {
    let machine = machine();
    let _env = machine.install();
    // A serial the installation's game database does not list, and no DAT.
    let unknown = "SLUS-99999";
    let mut app = app_at_the_picker(&machine, unknown, &[0]);
    app.poll_duckstation_transaction();
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .duckstation
        .acknowledge_database_shadowing = true;
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!("no preview");
    };
    let CheatPreviewOutcome::Failed(CheatPreviewFailure::DuckStationNative(refusal)) =
        &response.outcome
    else {
        panic!("{:?}", response.outcome);
    };
    assert!(
        matches!(
            refusal,
            archivefs_core::patch_manager::DuckStationNativeRefusal::DiscProofMissing { .. }
        ),
        "{refusal:?}"
    );
    assert!(
        fs::read_dir(machine.a_root.join("cheats"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(
        fs::read_dir(machine.a_root.join("gamesettings"))
            .unwrap()
            .next()
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// One resolver: cheats act on exactly what Launch would start
// ---------------------------------------------------------------------------

fn pair(exe: &Path, root: &Path) -> archivefs_core::emulator_profile_resolver::CandidateIdentity {
    archivefs_core::emulator_profile_resolver::CandidateIdentity {
        emulator: archivefs_core::emulator_inventory::InventoryEmulator::DuckStation,
        executable: Some(exe.to_path_buf()),
        profile_root: root.to_path_buf(),
    }
}

fn remember(
    machine: &Machine,
    mode: &archivefs_core::emulator_profile_resolver::EmulatorSelectionMode,
) {
    archivefs_core::emulator_profile_resolver::remember_selection_to(
        &machine.store_path,
        archivefs_core::emulator_inventory::InventoryEmulator::DuckStation,
        mode,
    )
    .unwrap();
}

fn launch_and_cheat_pairs(
    machine: &Machine,
) -> (
    crate::emulator_setup::profile_resolution::DuckStationLaunchSelection,
    crate::cheats_mods::duckstation_environment::DuckStationCheatSelection,
) {
    let inputs = machine.inputs();
    (
        crate::emulator_setup::profile_resolution::select_for_launch(&inputs),
        crate::cheats_mods::duckstation_environment::select_for_cheats(&inputs),
    )
}

#[test]
fn cheats_use_the_same_pair_launch_is_authorised_to_start() {
    use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority;
    let machine = machine();
    let (launch, cheats) = launch_and_cheat_pairs(&machine);
    let DuckStationLaunchAuthority::Authorised {
        executable,
        profile_root,
        ..
    } = &launch.authority
    else {
        panic!("{:?}", launch.authority);
    };
    let profile = cheats.profile.expect("cheats have a profile");
    assert_eq!(&profile.executable, executable);
    assert_eq!(&profile.profile_root, profile_root);
    assert_eq!(cheats.authority, launch.authority);
}

#[test]
fn a_preferred_fallback_is_the_same_for_cheats_and_launch_and_is_announced() {
    use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority;
    let machine = machine();
    let gone = machine.dir.path().join("gone/DuckStation.AppImage");
    remember(
        &machine,
        &archivefs_core::emulator_profile_resolver::EmulatorSelectionMode::Preferred(pair(
            &gone,
            &machine.b_root,
        )),
    );
    let (launch, cheats) = launch_and_cheat_pairs(&machine);
    assert_eq!(cheats.authority, launch.authority);
    let DuckStationLaunchAuthority::Authorised {
        executable,
        fallback,
        ..
    } = &launch.authority
    else {
        panic!("{:?}", launch.authority);
    };
    assert!(*fallback, "the bypassed preference is a fallback");
    assert_eq!(
        cheats.profile.as_ref().map(|p| &p.executable),
        Some(executable)
    );
    assert_ne!(executable, &gone);
    assert!(!cheats.authority.launch_notices().is_empty());
}

#[test]
fn a_forced_pin_that_cannot_be_used_blocks_cheats_exactly_as_it_blocks_launch() {
    use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority;
    let machine = machine();
    let gone = machine.dir.path().join("gone/DuckStation.AppImage");
    remember(
        &machine,
        &archivefs_core::emulator_profile_resolver::EmulatorSelectionMode::Forced(pair(
            &gone,
            &machine.a_root,
        )),
    );
    let (launch, cheats) = launch_and_cheat_pairs(&machine);
    assert!(matches!(
        launch.authority,
        DuckStationLaunchAuthority::Refused { .. }
    ));
    assert_eq!(cheats.authority, launch.authority);
    assert!(
        cheats.profile.is_none(),
        "no other installation is substituted"
    );

    // And the Cheats page refuses to preview, writing nothing anywhere.
    let _env = machine.install();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0]);
    app.poll_duckstation_transaction();
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(workflow.duckstation.resolved.is_none());
    assert!(
        fs::read_dir(machine.a_root.join("cheats"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(!machine.b_root.join("cheats").exists());
}

#[test]
fn installing_cheats_does_not_change_what_launch_starts() {
    let machine = machine();
    let (before, _) = launch_and_cheat_pairs(&machine);
    let _env = machine.install();
    let (_app, _) = apply_two_cheats(&machine);
    assert!(machine.a_root.join(format!("cheats/{SERIAL}.cht")).exists());
    let (after, _) = launch_and_cheat_pairs(&machine);
    assert_eq!(before.authority, after.authority);
    let ids =
        |selection: &crate::emulator_setup::profile_resolution::DuckStationLaunchSelection| {
            selection
                .discovery
                .profiles
                .iter()
                .map(|profile| profile.configuration_path.clone())
                .collect::<Vec<_>>()
        };
    assert_eq!(ids(&before), ids(&after));
}

#[test]
fn an_installation_changed_after_the_preview_blocks_the_apply_and_writes_nothing() {
    let machine = machine();
    let _env = machine.install();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0, 1]);
    app.poll_duckstation_transaction();
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .duckstation
        .acknowledge_database_shadowing = true;
    app.start_duckstation_install_preview();
    app.review_duckstation_apply();
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().transaction,
        CheatTransactionState::NativeReview { .. }
    ));
    // Meanwhile, in Emulator setup, DuckStation is pinned to something unusable.
    let gone = machine.dir.path().join("gone/DuckStation.AppImage");
    remember(
        &machine,
        &archivefs_core::emulator_profile_resolver::EmulatorSelectionMode::Forced(pair(
            &gone,
            &machine.a_root,
        )),
    );
    app.start_duckstation_apply(egui::Context::default());
    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatTransactionState::NativeFailed { message, .. } = &workflow.transaction else {
        panic!("the apply was not refused");
    };
    assert!(message.contains("changed since this preview"), "{message}");
    assert!(workflow.duckstation.resolved.is_none());
    assert!(
        fs::read_dir(machine.a_root.join("cheats"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(
        fs::read_dir(machine.a_root.join("gamesettings"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(!machine.dir.path().join("history").exists());
}

#[test]
fn a_new_preview_uses_the_installation_chosen_since_the_workflow_opened() {
    let machine = machine();
    let _env = machine.install();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0]);
    app.poll_duckstation_transaction();
    assert!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .duckstation
            .resolved
            .is_some()
    );
    let gone = machine.dir.path().join("gone/DuckStation.AppImage");
    remember(
        &machine,
        &archivefs_core::emulator_profile_resolver::EmulatorSelectionMode::Forced(pair(
            &gone,
            &machine.a_root,
        )),
    );
    app.start_duckstation_install_preview();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(workflow.duckstation.resolved.is_none());
    if let CheatStepResource::Ready(response) = &workflow.preview {
        assert!(response.duckstation_generated.is_none());
    }
}

#[test]
fn a_region_refusal_is_not_shown_for_a_different_cheat_file() {
    let machine = machine();
    let _env = machine.install();
    let mut app = app_at_the_picker(&machine, SERIAL, &[0]);
    set_candidate_region(&mut app, "Europe");
    app.poll_duckstation_transaction();
    app.start_duckstation_install_preview();
    let render = |app: &mut ArchiveFsApp| {
        let ctx = egui::Context::default();
        ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
        let workflow = app.cheat_workflow.as_mut().unwrap();
        let mut clipboard = InMemoryClipboard::default();
        ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let facts = duckstation_facts(workflow, None);
                let _ = show_duckstation_workflow(ui, workflow, &facts, false, &mut clipboard);
            });
        })
    };
    let banner = "This cheat file is for another region";
    assert!(rendered_text_contains(&render(&mut app), banner));
    // The user picks a different cheat file: the old refusal is not shown for it.
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .candidate_selection
        .as_mut()
        .unwrap()
        .candidate
        .catalogue_relative_path = "Sony - PlayStation/Game (USA).cht".to_string();
    assert!(!rendered_text_contains(&render(&mut app), banner));
}
