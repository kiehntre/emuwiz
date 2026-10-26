use super::*;
use archivefs_core::patch_manager::SharedTransactionPath;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fixture {
    _root: tempfile::TempDir,
    game: PathBuf,
    detector: PathBuf,
    roots: Roots,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("original.data"), b"synthetic original bytes").unwrap();
        let detector = root.path().join("detector");
        let roots = Roots {
            profiles: root.path().join("owned-profiles"),
            history: root.path().join("history"),
            backups: root.path().join("backups"),
        };
        let fixture = Self {
            _root: root,
            game,
            detector,
            roots,
        };
        fixture.output("Game: hypno:demo (Synthetic Game)");
        fixture
    }

    fn output(&self, output: &str) {
        fs::write(
            &self.detector,
            format!("#!/bin/sh\nprintf '%s\\n' '{output}'\n"),
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&self.detector, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn prepare(&self) -> Result<Prepared, String> {
        backend::prepare_with(
            &self.game,
            Some("hypno:demo"),
            vec![ScummVmTrainerSelection {
                key: "infiniteHealth".into(),
                value: Some(ScummVmTrainerValue::Boolean(true)),
            }],
            self.detector.clone(),
            self.roots.clone(),
        )
    }

    fn profile(&self) -> PathBuf {
        self.roots.profile(&self.game, "hypno:demo")
    }

    fn assert_game_untouched(&self) {
        assert_eq!(
            fs::read(self.game.join("original.data")).unwrap(),
            b"synthetic original bytes"
        );
        assert_eq!(fs::read_dir(&self.game).unwrap().count(), 1);
    }
}

#[test]
#[cfg(unix)]
fn scummvm_preview_apply_restart_history_and_confirmed_undo() {
    let f = Fixture::new();
    let prepared = f.prepare().unwrap();
    assert!(!f.profile().exists());
    assert!(!f.roots.history.exists());
    let result = backend::apply(prepared).unwrap();
    assert_eq!(
        result.journal.status,
        SharedApplyStatus::Success,
        "{:?}",
        result.journal
    );
    assert!(result.journal_path.unwrap().is_file());
    assert!(
        fs::read_to_string(f.profile())
            .unwrap()
            .contains("infiniteHealth=true")
    );
    // Reload from durable receipts, without retaining the Apply result or state.
    let (receipts, problems) = backend::history(&f.roots);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(receipts.len(), 1);
    let undo = backend::undo_preview(&receipts[0], &f.roots).unwrap();
    assert!(undo.available);
    assert!(f.profile().exists()); // Preview is read-only.
    let result = backend::undo(undo, f.roots.clone());
    assert_eq!(result.status, SharedApplyStatus::Success);
    assert!(!f.profile().exists());
    f.assert_game_untouched();
}

#[test]
#[cfg(unix)]
fn scummvm_replacement_preserves_exact_backup_and_restores_it() {
    let f = Fixture::new();
    fs::create_dir(&f.roots.profiles).unwrap();
    let original = b"[scummvm]\nsavepath=/synthetic/existing-saves\n[unrelated]\nkeep=true\n";
    fs::write(f.profile(), original).unwrap();
    let result = backend::apply(f.prepare().unwrap()).unwrap();
    assert_eq!(result.journal.status, SharedApplyStatus::Success);
    let backup = result.journal.entries[0]
        .backup_path
        .as_ref()
        .unwrap()
        .to_path_buf()
        .unwrap();
    assert_eq!(fs::read(backup).unwrap(), original);
    let (receipts, _) = backend::history(&f.roots);
    let preview = backend::undo_preview(&receipts[0], &f.roots).unwrap();
    assert!(preview.available);
    assert_eq!(
        backend::undo(preview, f.roots.clone()).status,
        SharedApplyStatus::Success
    );
    assert_eq!(fs::read(f.profile()).unwrap(), original);
    f.assert_game_untouched();
}

#[test]
#[cfg(unix)]
fn scummvm_missing_or_ambiguous_identity_and_unsupported_engine_refuse() {
    let f = Fixture::new();
    for output in [
        "No games found",
        "Game: hypno:demo (One)\nGame: hypno:other (Two)",
        "Game: scumm:monkey (Unsupported)",
    ] {
        f.output(output);
        assert!(
            backend::prepare_with(&f.game, None, vec![], f.detector.clone(), f.roots.clone())
                .is_err()
        );
    }
    assert!(!f.roots.profiles.exists());
    f.assert_game_untouched();
}

#[test]
#[cfg(unix)]
fn scummvm_fresh_detection_and_directory_identity_refuse_stale_approval() {
    let f = Fixture::new();
    let prepared = f.prepare().unwrap();
    f.output("Game: hypno:other (Changed game)");
    assert!(backend::apply(prepared).is_err());
    f.output("Game: hypno:demo (Synthetic Game)");
    let prepared = f.prepare().unwrap();
    let moved = f.game.with_extension("previous");
    fs::rename(&f.game, &moved).unwrap();
    fs::create_dir(&f.game).unwrap();
    assert!(backend::apply(prepared).is_err());
    assert!(!f.profile().exists());
    assert_eq!(
        fs::read(moved.join("original.data")).unwrap(),
        b"synthetic original bytes"
    );
}

#[test]
#[cfg(unix)]
fn scummvm_new_destination_or_changed_existing_config_refuse_apply() {
    let f = Fixture::new();
    let prepared = f.prepare().unwrap();
    fs::create_dir(&f.roots.profiles).unwrap();
    fs::write(f.profile(), b"[other]\nuntouched=true\n").unwrap();
    let result = backend::apply(prepared).unwrap();
    assert_ne!(result.journal.status, SharedApplyStatus::Success);
    let prepared = f.prepare().unwrap();
    fs::write(f.profile(), b"[other]\nchanged=true\n").unwrap();
    assert_ne!(
        backend::apply(prepared).unwrap().journal.status,
        SharedApplyStatus::Success
    );
    assert_eq!(fs::read(f.profile()).unwrap(), b"[other]\nchanged=true\n");
    f.assert_game_untouched();
}

#[test]
#[cfg(unix)]
fn scummvm_changed_destination_after_undo_preview_refuses() {
    let f = Fixture::new();
    backend::apply(f.prepare().unwrap()).unwrap();
    let (receipts, _) = backend::history(&f.roots);
    let preview = backend::undo_preview(&receipts[0], &f.roots).unwrap();
    fs::write(f.profile(), b"changed after approval").unwrap();
    assert_ne!(
        backend::undo(preview, f.roots.clone()).status,
        SharedApplyStatus::Success
    );
    assert_eq!(fs::read(f.profile()).unwrap(), b"changed after approval");
    assert!(
        !backend::undo_preview(&receipts[0], &f.roots)
            .unwrap()
            .available
    );
}

#[test]
#[cfg(unix)]
fn scummvm_missing_backup_and_wrong_receipt_scope_refuse_undo() {
    let f = Fixture::new();
    fs::create_dir(&f.roots.profiles).unwrap();
    fs::write(f.profile(), b"[original]\nkeep=true\n").unwrap();
    let result = backend::apply(f.prepare().unwrap()).unwrap();
    let (mut receipts, _) = backend::history(&f.roots);
    let backup = result.journal.entries[0]
        .backup_path
        .as_ref()
        .unwrap()
        .to_path_buf()
        .unwrap();
    fs::remove_file(backup).unwrap();
    assert!(
        !backend::undo_preview(&receipts[0], &f.roots)
            .unwrap()
            .available
    );
    receipts[0].journal.entries[0]
        .plan_entry
        .destination_relative_path = SharedTransactionPath::from_path(Path::new("foreign.ini"));
    assert!(backend::undo_preview(&receipts[0], &f.roots).is_err());
    assert!(f.profile().is_file());
}

#[test]
#[cfg(unix)]
fn scummvm_symlink_source_or_destination_and_source_owned_profile_refuse() {
    let f = Fixture::new();
    let alias = f.game.with_extension("alias");
    symlink(&f.game, &alias).unwrap();
    assert!(
        backend::prepare_with(&alias, None, vec![], f.detector.clone(), f.roots.clone()).is_err()
    );
    fs::create_dir(&f.roots.profiles).unwrap();
    symlink(f.game.join("original.data"), f.profile()).unwrap();
    assert!(f.prepare().is_err());
    let mut roots = f.roots.clone();
    roots.profiles = f.game.join("profiles");
    assert!(backend::prepare_with(&f.game, None, vec![], f.detector.clone(), roots).is_err());
    f.assert_game_untouched();
}

#[test]
#[cfg(unix)]
fn scummvm_cancel_draft_change_and_game_switch_discard_preview() {
    let f = Fixture::new();
    let mut state = State {
        folder: Some(f.game.clone()),
        identity: Some("hypno:demo".into()),
        prepared: Some(f.prepare().unwrap()),
        confirmation: Some(Confirmation::Apply),
        ..Default::default()
    };
    state.invalidate_preview();
    assert!(state.prepared.is_none());
    assert!(state.confirmation.is_none());
    state.prepared = Some(f.prepare().unwrap());
    state.confirmation = Some(Confirmation::Apply);
    let context = egui::Context::default();
    let _ = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            state.show(
                ui,
                &f.game.with_extension("different"),
                "Different selection",
            );
        });
    });
    assert!(state.prepared.is_none());
    assert!(state.confirmation.is_none());
    assert!(!f.profile().exists());
}

#[test]
fn scummvm_profile_names_bind_exact_folder_and_identity() {
    let f = Fixture::new();
    assert_ne!(
        f.profile(),
        f.roots
            .profile(&f.game.with_extension("other"), "hypno:demo")
    );
    assert_ne!(f.profile(), f.roots.profile(&f.game, "hypno:other"));
    assert_eq!(f.profile().parent(), Some(f.roots.profiles.as_path()));
    assert!(LAUNCH_NOTE.contains("choose this saved profile"));
}

fn modal_frame(
    ctx: &egui::Context,
    state: &mut State,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show_confirmation(ui));
        },
    )
}

fn click_modal(ctx: &egui::Context, state: &mut State, label: &str) {
    let output = modal_frame(ctx, state, vec![]);
    let output = if output.shapes.is_empty() {
        modal_frame(ctx, state, vec![])
    } else {
        output
    };
    fn position(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == label => {
                Some(t.pos + t.galley.rect.size() / 2.0)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| position(s, label)),
            _ => None,
        }
    }
    let point = output
        .shapes
        .iter()
        .find_map(|s| position(&s.shape, label))
        .expect("confirmation control visible");
    for pressed in [true, false] {
        modal_frame(
            ctx,
            state,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
    }
}

fn wait_for_worker(state: &mut State) {
    inspect_worker(state, |_| {});
}

fn inspect_worker(state: &mut State, inspect: impl FnOnce(&Result<Completed, String>)) {
    if let Some(receiver) = state.pending.take() {
        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("bounded synthetic worker must complete");
        inspect(&result);
        let (tx, rx) = mpsc::channel();
        tx.send(result)
            .unwrap_or_else(|_| panic!("poll channel closed"));
        state.pending = Some(rx);
        state.poll();
    }
}

#[test]
#[cfg(unix)]
fn scummvm_real_confirmation_cancel_apply_and_undo_are_explicit() {
    let f = Fixture::new();
    let ctx = egui::Context::default();
    let mut state = State {
        prepared: Some(f.prepare().unwrap()),
        confirmation: Some(Confirmation::Apply),
        focus_cancel: true,
        ..Default::default()
    };
    modal_frame(&ctx, &mut state, vec![]);
    modal_frame(&ctx, &mut state, vec![]);
    assert!(!f.profile().exists());
    click_modal(&ctx, &mut state, "Cancel");
    assert!(state.confirmation.is_none());
    assert!(state.pending.is_none());
    assert!(!f.profile().exists());
    state.confirmation = Some(Confirmation::Apply);
    state.focus_cancel = true;
    modal_frame(&ctx, &mut state, vec![]);
    click_modal(&ctx, &mut state, "Confirm");
    wait_for_worker(&mut state);
    assert!(f.profile().exists(), "{:?}", state.status);
    let (receipts, problems) = backend::history(&f.roots);
    assert!(problems.is_empty());
    state.undo = Some((
        f.roots.clone(),
        backend::undo_preview(&receipts[0], &f.roots).unwrap(),
    ));
    state.confirmation = Some(Confirmation::Undo);
    state.focus_cancel = true;
    modal_frame(&ctx, &mut state, vec![]);
    click_modal(&ctx, &mut state, "Cancel");
    assert!(f.profile().exists());
    state.confirmation = Some(Confirmation::Undo);
    state.focus_cancel = true;
    modal_frame(&ctx, &mut state, vec![]);
    click_modal(&ctx, &mut state, "Confirm");
    wait_for_worker(&mut state);
    assert!(!f.profile().exists(), "{:?}", state.status);
    f.assert_game_untouched();
}

#[test]
fn scummvm_confirmation_keeps_cancel_default_but_allows_keyboard_focus() {
    let ctx = egui::Context::default();
    let mut state = State {
        confirmation: Some(Confirmation::Apply),
        focus_cancel: true,
        ..Default::default()
    };
    modal_frame(&ctx, &mut state, vec![]);
    modal_frame(&ctx, &mut state, vec![]);
    let default = ctx
        .memory(|m| m.focused())
        .expect("Cancel gets initial focus");
    modal_frame(
        &ctx,
        &mut state,
        vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }],
    );
    modal_frame(&ctx, &mut state, vec![]);
    assert_ne!(
        ctx.memory(|m| m.focused()),
        Some(default),
        "Cancel must not steal keyboard focus every frame"
    );
}

#[test]
#[cfg(unix)]
fn scummvm_gui_workers_keep_history_and_backups_in_test_machine() {
    const CHILD: &str = "EMUWIZ_SCUMMVM_HISTORY_WORKER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let f = Fixture::new();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cheats_mods::scummvm_trainer::tests::scummvm_gui_workers_keep_history_and_backups_in_test_machine",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", f._root.path())
            .env("TMPDIR", f._root.path())
            .env("EMUWIZ_DATA_HOME", f._root.path().join("app-data"))
            .env(archivefs_core::scummvm_detection::TEST_EXECUTABLE_ENV, &f.detector)
            .env(CHILD, f._root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // Environment overrides are child-only. The TestMachine remains thread-local:
    // worker resolution would choose the distinct process-default stores.
    let f = Fixture::new();
    let _machine = crate::cheats_mods::duckstation_environment::test_environment::install(
        crate::cheats_mods::duckstation_environment::test_environment::TestMachine {
            duckstation: None,
            history_root: f.roots.history.clone(),
            backup_root: f.roots.backups.clone(),
        },
    );
    let roots = Roots::managed().unwrap();
    let profile = roots.profile(&f.game, "hypno:demo");
    let original = b"[emuwiz-trainer]\ninfiniteHealth=false\n";
    fs::create_dir_all(&roots.profiles).unwrap();
    fs::write(&profile, original).unwrap();
    let ctx = egui::Context::default();
    let mut state = State::default();

    fn frame(
        ctx: &egui::Context,
        state: &mut State,
        folder: &Path,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 2400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.show(ui, folder, "Synthetic game");
                });
            },
        )
    }
    fn click(ctx: &egui::Context, state: &mut State, folder: &Path, label: &str) {
        fn position(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.pos + t.galley.rect.size() / 2.0)
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| position(s, label)),
                _ => None,
            }
        }
        frame(ctx, state, folder, vec![]);
        let output = frame(ctx, state, folder, vec![]);
        let point = output
            .shapes
            .iter()
            .find_map(|s| position(&s.shape, label))
            .expect(label);
        for pressed in [true, false] {
            frame(
                ctx,
                state,
                folder,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
    }
    // The first frame starts the automatic History load, which keeps Check
    // disabled until it finishes. Settle it first, or a load finishing
    // mid-click silently swallows the Check click.
    frame(&ctx, &mut state, &f.game, vec![]);
    assert!(state.pending.is_some(), "the first frame loads History");
    inspect_worker(&mut state, |result| {
        assert!(
            matches!(result, Ok(Completed::History(..))),
            "the first worker must be the History load"
        );
    });
    click(&ctx, &mut state, &f.game, "Check trainer compatibility");
    assert!(state.pending.is_some(), "Check must start its worker");
    inspect_worker(&mut state, |result| {
        let Ok(Completed::Checked(prepared)) = result else {
            panic!("Check worker must return a prepared compatibility result");
        };
        assert_eq!(prepared.roots.history, f.roots.history);
        assert_eq!(prepared.roots.backups, f.roots.backups);
    });
    assert_eq!(
        state.identity.as_deref(),
        Some("hypno:demo"),
        "{:?}",
        state.status
    );
    click(&ctx, &mut state, &f.game, "On");
    click(&ctx, &mut state, &f.game, "Preview trainer profile");
    wait_for_worker(&mut state);
    let prepared = state.prepared.as_ref().expect("GUI preview completed");
    assert_eq!(
        prepared.roots.history, f.roots.history,
        "Preview must retain caller's History root"
    );
    assert_eq!(prepared.roots.backups, f.roots.backups);
    assert_eq!(
        fs::read(&profile).unwrap(),
        original,
        "Check/Preview are read-only"
    );
    click(&ctx, &mut state, &f.game, "Review Apply…");
    click_modal(&ctx, &mut state, "Confirm");
    wait_for_worker(&mut state);
    let (receipts, problems) = backend::history(&roots);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(receipts.len(), 1, "{:?}", state.status);
    let receipt = &receipts[0];
    assert_eq!(receipt.journal.status, SharedApplyStatus::Success);
    assert!(receipt.journal_path.starts_with(&f.roots.history));
    let backup = receipt.journal.entries[0]
        .backup_path
        .as_ref()
        .unwrap()
        .to_path_buf()
        .unwrap();
    assert!(backup.starts_with(&f.roots.backups));
    assert_eq!(fs::read(backup).unwrap(), original);
    assert!(
        !archivefs_core::patch_manager::default_shared_history_root()
            .unwrap()
            .exists()
    );
    assert!(
        !archivefs_core::patch_manager::default_shared_backup_root()
            .unwrap()
            .exists()
    );
    let mut fresh = State::default();
    assert!(fresh.prepare_history(&ctx, Some(&f.game)));
    wait_for_worker(&mut fresh);
    assert_eq!(fresh.receipts.len(), 1);
    assert_eq!(
        fresh.receipts[0].journal.operation_id,
        receipt.journal.operation_id
    );
    f.assert_game_untouched();
}
