use super::*;
use archivefs_core::{
    launch::cemu_readiness::inspect_cemu_readiness,
    patch_manager::CemuProfileDiscoveryRoots,
    wiiu_content_readiness::{
        WiiUContentRepresentation as Form, WiiUEncryptionState as Encryption,
    },
};

fn inspection(dir: &std::path::Path) -> CemuReadinessInspection {
    let roots = CemuProfileDiscoveryRoots {
        home: dir.join("home"),
        xdg_config_home: dir.join("config"),
        explicit_configuration_roots: vec![],
        portable_configuration_roots: vec![],
        explicit_executables: vec![],
        known_version_outputs: Default::default(),
        appimage_directory: None,
    };
    inspect_cemu_readiness(&dir.join("absent.wud"), None, &roots)
}

fn render(state: &mut State, inspection: &CemuReadinessInspection) -> String {
    let ctx = egui::Context::default();
    let output = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| state.show(ui, inspection));
    });
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                text.push_str(t.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in output.shapes {
        collect(&shape.shape, &mut text);
    }
    text
}

#[test]
fn non_disc_requirements_do_not_start_key_checks_or_show_disc_actions() {
    let dir = tempfile::tempdir().unwrap();
    let mut inspection = inspection(dir.path());
    let mut state = State::default();
    for (form, encryption, complete, reason) in [
        (
            Form::LoadiineExtracted,
            Encryption::Decrypted,
            true,
            "not required",
        ),
        (Form::Nus, Encryption::Encrypted, true, "title/ticket"),
        (Form::Wua, Encryption::Unknown, true, "unknown"),
        (Form::Wud, Encryption::Encrypted, false, "unknown"),
    ] {
        inspection.content.representation = form;
        inspection.content.encryption_state = encryption;
        inspection.content.structural_complete = complete;
        state.begin(
            &inspection,
            Some(dir.path().join("keys.txt")),
            egui::Context::default(),
        );
        assert!(state.receiver.is_none());
        let text = render(&mut state, &inspection);
        assert!(text.contains(reason), "{text}");
        assert!(!text.contains("Choose existing keys.txt"));
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn encrypted_panel_is_truthful_with_no_profile_and_well_formed_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut inspection = inspection(dir.path());
    inspection.content.representation = Form::Wud;
    inspection.content.encryption_state = Encryption::Encrypted;
    inspection.content.structural_complete = true;
    let path = dir.path().join("keys.txt");
    // Deliberate zero placeholder, never a real key.
    let placeholder = "0".repeat(32);
    std::fs::write(&path, format!("{placeholder} # PRIVATE COMMENT\n")).unwrap();
    let mut state = State {
        report: Some(Report {
            selected: Some(diagnostic::inspect_store(&path)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let text = render(&mut state, &inspection);
    for expected in [
        "No configured Cemu profile",
        "Applicability UNKNOWN",
        "Launch remains unsupported",
        "not a runtime profile binding",
        "Choose existing keys.txt",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(!text.contains(&placeholder));
    assert!(!text.contains("PRIVATE COMMENT"));
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
}

#[test]
fn reset_discards_completed_and_late_results_and_worker_disconnect_is_unknown() {
    let ctx = egui::Context::default();
    let (sender, receiver) = mpsc::channel();
    let mut state = State {
        receiver: Some(receiver),
        report: Some(Report::default()),
        error: true,
    };
    state.reset();
    assert!(sender.send(Report::default()).is_err());
    assert!(state.report.is_none() && state.receiver.is_none() && !state.error);
    let (sender, receiver) = mpsc::channel();
    state.receiver = Some(receiver);
    drop(sender);
    state.poll(&ctx);
    assert!(state.error);
    assert!(state.report.is_none());
}

#[test]
fn every_typed_result_has_an_explicit_reason_without_key_material() {
    for state in [
        StoreState::Missing,
        StoreState::Empty,
        StoreState::Malformed { line: 3 },
        StoreState::LineTooLong { line: 3 },
        StoreState::TooLarge,
        StoreState::UnsafePath,
        StoreState::Unreadable,
        StoreState::Changed,
    ] {
        assert!(!store_text(&state).is_empty());
        assert!(!store_text(&state).contains("Verified"));
    }
    assert!(store_text(&StoreState::Malformed { line: 3 }).contains("line 3"));
}

#[test]
fn explicit_worker_returns_only_a_typed_standalone_report() {
    let dir = tempfile::tempdir().unwrap();
    let mut inspection = inspection(dir.path());
    inspection.content.representation = Form::Wux;
    inspection.content.encryption_state = Encryption::Encrypted;
    inspection.content.structural_complete = true;
    let path = dir.path().join("keys.txt");
    std::fs::write(&path, vec![b'0'; 32]).unwrap();
    let before = format!("{inspection:?}");
    let mut state = State::default();
    state.begin(&inspection, Some(path.clone()), egui::Context::default());
    let report = state
        .receiver
        .take()
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(report.profiles.is_empty());
    assert_eq!(
        report.selected.unwrap(),
        StoreDiagnostic {
            path,
            state: StoreState::WellFormedUnverified { entries: 1 }
        }
    );
    assert_eq!(format!("{inspection:?}"), before);
}
