//! Native route regression: durable trainer receipts must work before Mods is opened.
use super::*;
use archivefs_core::patch_manager::*;
use sha2::{Digest, Sha256};
use std::{fs, sync::Arc};

fn texts(shape: &egui::Shape, output: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            output.push_str(t.galley.text());
            output.push('\n');
        }
        egui::Shape::Vec(v) => {
            for shape in v {
                texts(shape, output);
            }
        }
        _ => {}
    }
}

#[test]
fn scummvm_native_blue_cheats_opens_the_trainer_controls_without_a_generic_adapter() {
    let context = egui::Context::default();
    let mut host = crate::gui_v2::native_workflows::NativeWorkflows::new(context.clone());
    let library = Library::new(vec![super::archive(
        7101,
        "Synthetic Hypno",
        "/fixture/hypno-folder",
        "ScummVM",
    )]);
    let mut activity = crate::gui_v2::activity::Activity::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 1600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(
                    host.show_cheats(ui, library.game(7101), &mut activity)
                        .is_none()
                );
            });
        },
    );
    let mut output_text = String::new();
    for shape in output.shapes {
        texts(&shape.shape, &mut output_text);
    }
    assert!(
        output_text.contains("Check trainer compatibility"),
        "{output_text}"
    );
    assert!(output_text.contains("does not yet load"));
    assert!(
        host.app.cheat_workflow.is_none(),
        "ScummVM uses its focused trainer flow"
    );
}

#[test]
fn scummvm_fresh_blue_history_and_exact_game_scope_reload_receipts() {
    // A real catalogue already owns its application-data directory. The
    // transaction intentionally refuses to invent missing managed-root parents.
    let data = archivefs_core::app_dirs::data_dir().unwrap();
    assert!(data.starts_with(
        archivefs_core::test_environment::isolated_root().expect("private test root")
    ));
    fs::create_dir_all(data).unwrap();
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    fs::create_dir(&game).unwrap();
    let mut hash = Sha256::new();
    hash.update(b"hypno:demo\0");
    hash.update(game.as_os_str().as_encoded_bytes());
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let config = archivefs_core::app_dirs::data_path("scummvm-trainer-options")
        .unwrap()
        .join(format!("{digest}.ini"));
    let staging = tempfile::tempdir().unwrap();
    let preview = build_scummvm_trainer_preview(&ScummVmTrainerRequest {
        identity: ScummVmTrainerIdentity {
            game_id: "hypno:demo".into(),
            game_folder: game.clone(),
            target_name: "emuwiz-trainer".into(),
        },
        configuration_path: config.clone(),
        staging_root: staging.path().to_path_buf(),
        selections: vec![ScummVmTrainerSelection {
            key: "cheats".into(),
            value: Some(ScummVmTrainerValue::Boolean(true)),
        }],
        expected_configuration_sha256: None,
    })
    .unwrap();
    let operation = generate_shared_operation_id();
    let result = apply_scummvm_trainer_preview(
        &preview,
        &ScummVmTrainerApplyOptions {
            general_approved: true,
            replacement_approved: true,
            operation_id: operation.clone(),
            timestamp_unix_seconds: 123,
            history_root: default_shared_history_root().unwrap(),
            backup_root: default_shared_backup_root().unwrap(),
        },
    )
    .unwrap();
    assert_eq!(
        result.journal.status,
        SharedApplyStatus::Success,
        "journal: {:#?}; failure: {:?}",
        result.journal,
        result.journal_failure
    );
    let original = fs::read(&config).unwrap();
    let context = egui::Context::default();
    let mut app = crate::gui_v2::tests::fixture(&context);
    let mut selected = super::archive(7001, "Synthetic Hypno", "/unused", "ScummVM");
    selected.absolute_path = game.clone();
    let other = super::archive(7002, "Other", "/fixture/different-folder", "ScummVM");
    app.library = Arc::new(Library::new(vec![selected, other]));
    assert!(app.native_workflows.is_none());
    app.router.current = Route::Section(Section::History);
    let frame = |app: &mut crate::gui_v2::App| {
        let result = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 2400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.history(ui));
            },
        );
        let mut output = String::new();
        for shape in result.shapes {
            texts(&shape.shape, &mut output);
        }
        output
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut output = frame(&mut app);
    while !output.contains(&operation) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
        output = frame(&mut app);
    }
    assert!(
        app.native_workflows.is_some(),
        "History initializes its receipt reader after restart"
    );
    assert!(
        output.contains(&operation),
        "durable trainer receipt must be visible: {output}"
    );
    assert!(output.contains("Preview Undo"));
    assert!(
        !output.contains("No history yet"),
        "trainer receipts suppress an empty History claim"
    );
    app.router.current = Route::Task {
        section: Section::History,
        game: 7001,
    };
    let output = frame(&mut app);
    assert!(
        output.contains(&operation),
        "exact game History includes its trainer receipt"
    );
    assert!(!output.contains("No history for this game"));
    app.router.current = Route::Task {
        section: Section::History,
        game: 7002,
    };
    let output = frame(&mut app);
    assert!(
        !output.contains(&operation),
        "another game cannot inherit this receipt"
    );
    app.router.current = Route::Task {
        section: Section::History,
        game: 7999,
    };
    let output = frame(&mut app);
    assert!(
        !output.contains(&operation),
        "an unresolved scoped game must not turn into global trainer History"
    );
    assert_eq!(
        fs::read(config).unwrap(),
        original,
        "browsing History does not perform Undo"
    );
}
