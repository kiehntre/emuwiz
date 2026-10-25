//! Blue game-page save snapshots: selection, missing saves, incompatible
//! profiles, stale snapshots, closure refusal and the supported create action.
use super::super::save_snapshots::{
    RESTORE_UNAVAILABLE, RecordSnapshots, SnapshotReport, SnapshotTarget, closure_message, create,
    freshness_label, load_history, snapshot_target,
};
use super::*;
use archivefs_core::game_identity::{GameIdentityReport, IdentityImageFormat, IdentityPlatform};
use archivefs_core::save_snapshots::{
    EmulatorUseStatus,
    history::SnapshotFreshness,
    tree_restore::safety::{DirectorySaveBinding, EmulatorQuiescence, QuiescenceProvider},
};

struct Observed(EmulatorQuiescence);
impl QuiescenceProvider for Observed {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        self.0
    }
}

fn serial(value: &str) -> IdentityEvidence {
    IdentityEvidence {
        kind: IdentityKind::Ps2Serial,
        status: IdentityStatus::Verified,
        value: Some(value.into()),
        confidence: IdentityConfidence::StructuredMetadata,
        provenance: IdentityProvenance {
            archive_path: "synthetic".into(),
            member_path: None,
            member_index: None,
            method: "fixture".into(),
        },
        diagnostic: "fixture".into(),
    }
}

fn game_with(serials: &[&str]) -> Game {
    let mut archive = archive(301, "Synthetic PS2 Game", Some("PS2"));
    archive.identity_report = Some(GameIdentityReport {
        archive_path: archive.absolute_path.clone(),
        platform: IdentityPlatform::PlayStation2,
        format: IdentityImageFormat::Iso,
        evidence: serials.iter().map(|value| serial(value)).collect(),
        warnings: vec![],
        bytes_read: 0,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    });
    Game::from_archive(archive)
}

fn card_record(path: &Path, emulator: StateEmulator) -> PersistentStateRecord {
    let mut record = state_record(PersistentStateType::MemoryCard, emulator);
    record.path = path.to_path_buf();
    record.game_identity = vec![serial("SLUS-20001")];
    record
}

fn target_for(path: &Path, emulator: StateEmulator) -> SnapshotTarget {
    snapshot_target(&card_record(path, emulator), &game_with(&["SLUS-20001"])).unwrap()
}

#[test]
fn snapshot_target_requires_shared_verified_identity_and_a_game_save_type() {
    let game = game_with(&["SLUS-20001"]);
    let record = card_record(Path::new("/fixture/Mcd001.ps2"), StateEmulator::Pcsx2);
    let target = snapshot_target(&record, &game).unwrap();
    assert_eq!(target.game_identity, "Ps2Serial:SLUS-20001");
    assert_eq!(target.platform, game.platform);

    // Another game's serial: never inferred from the path or filename.
    assert!(snapshot_target(&record, &game_with(&["SLUS-99999"])).is_err());
    let mut unidentified = record.clone();
    unidentified.game_identity.clear();
    assert!(snapshot_target(&unidentified, &game).is_err());
    let mut system = record.clone();
    system.state_type = PersistentStateType::NandOrVirtualDisk;
    assert!(snapshot_target(&system, &game).is_err());
    let mut warned = record;
    warned.warnings.push("unreadable".into());
    assert!(snapshot_target(&warned, &game).is_err());
}

#[test]
fn supported_closure_creates_a_verified_snapshot_and_history_shows_it() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("Mcd001.ps2");
    fs::write(&card, b"card-v1").unwrap();
    let root = dir.path().join("snapshots");
    let target = target_for(&card, StateEmulator::Pcsx2);

    let id = create(&root, &target, &Observed(EmulatorQuiescence::Closed)).unwrap();
    assert_eq!(fs::read(&card).unwrap(), b"card-v1", "live save only read");
    let report = load_history(
        &root,
        std::slice::from_ref(&target),
        &Observed(EmulatorQuiescence::Closed),
    );
    let snapshots = &report.records[&card];
    assert_eq!(snapshots.emulator_use, Some(EmulatorUseStatus::NotDetected));
    assert_eq!(snapshots.entries.len(), 1);
    let entry = &snapshots.entries[0];
    assert_eq!(entry.snapshot.manifest.snapshot_id, id);
    assert_eq!(entry.snapshot.manifest.original_save_path, card);
    assert!(entry.compatible());
    assert_eq!(entry.freshness, SnapshotFreshness::MatchesCurrentSave);
}

#[test]
fn active_ambiguous_or_unsupported_closure_refuses_without_copying() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("Mcd001.ps2");
    fs::write(&card, b"card").unwrap();
    let root = dir.path().join("snapshots");
    for (emulator, observed) in [
        (StateEmulator::Pcsx2, EmulatorQuiescence::Running),
        (StateEmulator::Pcsx2, EmulatorQuiescence::Unknown),
        // No reviewed closure observer for DuckStation: "closed" is ignored.
        (StateEmulator::DuckStation, EmulatorQuiescence::Closed),
    ] {
        let target = target_for(&card, emulator);
        assert!(create(&root, &target, &Observed(observed)).is_err());
        assert!(
            !root.exists(),
            "nothing written for {emulator:?}/{observed:?}"
        );
    }
    assert!(closure_message(StateEmulator::Dolphin, None).contains("cannot yet confirm"));
    assert!(
        closure_message(StateEmulator::Pcsx2, Some(EmulatorUseStatus::Running))
            .contains("Close it")
    );
}

#[test]
fn stale_snapshot_and_incompatible_profile_are_explained() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("Mcd001.ps2");
    fs::write(&card, b"card-v1").unwrap();
    let root = dir.path().join("snapshots");
    let target = target_for(&card, StateEmulator::Pcsx2);
    create(&root, &target, &Observed(EmulatorQuiescence::Closed)).unwrap();
    fs::write(&card, b"card-v2-after-play").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&card)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(120))
        .unwrap();
    let mut other_profile = target.clone();
    other_profile.location.profile = Some("another-profile".into());
    let report = load_history(
        &root,
        &[other_profile],
        &Observed(EmulatorQuiescence::Closed),
    );
    let entry = &report.records[&card].entries[0];
    assert_eq!(entry.freshness, SnapshotFreshness::CurrentSaveNewer);
    assert!(freshness_label(entry.freshness).contains("played since"));
    assert!(!entry.compatible());
}

fn render_section(
    context: &egui::Context,
    app: &mut App,
    record: &PersistentStateRecord,
    target: Result<&SnapshotTarget, &'static str>,
    click: Option<egui::Pos2>,
) -> egui::FullOutput {
    let mut events = Vec::new();
    if let Some(point) = click {
        events.push(egui::Event::PointerMoved(point));
        events.push(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        events.push(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
    }
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::super::save_snapshots::show(ui, app, record, target);
            });
        },
    )
}

fn report_for(path: &Path, emulator_use: EmulatorUseStatus) -> SnapshotReport {
    let mut report = SnapshotReport::default();
    report.records.insert(
        path.to_path_buf(),
        RecordSnapshots {
            entries: Vec::new(),
            emulator_use: Some(emulator_use),
        },
    );
    report
}

fn click_create(
    context: &egui::Context,
    app: &mut App,
    record: &PersistentStateRecord,
    target: &SnapshotTarget,
) -> Vec<String> {
    let first = render_section(context, app, record, Ok(target), None);
    let point = text_bounds(&first, "Create snapshot")
        .first()
        .expect("create action is visible")
        .center();
    // Press and release over two frames, as a real click is delivered.
    render_section(context, app, record, Ok(target), Some(point));
    render_section(context, app, record, Ok(target), None);
    text(&first)
}

#[test]
fn blue_section_offers_create_only_with_proven_closure_and_never_restore() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let dir = tempfile::tempdir().unwrap();
    app.saves_states.snapshots.root = Some(dir.path().join("snapshots"));
    app.saves_states.snapshots.root_resolved = true;
    let card = dir.path().join("Mcd001.ps2");
    let record = card_record(&card, StateEmulator::Pcsx2);
    let target = target_for(&card, StateEmulator::Pcsx2);

    app.saves_states.snapshots.report = Some(report_for(&card, EmulatorUseStatus::Running));
    let shown = click_create(&context, &mut app, &record, &target);
    assert!(shown.iter().any(|s| s == "Memory card snapshots"));
    assert!(
        shown
            .iter()
            .any(|s| s.contains("No snapshots of this save yet"))
    );
    assert!(shown.iter().any(|s| s.contains("Close it, then refresh")));
    assert!(shown.iter().any(|s| s == "Restore"));
    assert!(
        app.saves_states.snapshots.create_job.is_none(),
        "running emulator refuses"
    );

    app.saves_states.snapshots.report = Some(report_for(&card, EmulatorUseStatus::NotDetected));
    click_create(&context, &mut app, &record, &target);
    assert!(
        app.saves_states.snapshots.create_job.is_some(),
        "closed emulator may snapshot"
    );
    assert!(RESTORE_UNAVAILABLE.contains("not available"));
}

#[test]
fn blue_section_explains_missing_saves_and_unsupported_emulators() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.saves_states.snapshots.root = Some("/fixture/snapshots".into());
    app.saves_states.snapshots.root_resolved = true;
    let record = card_record(Path::new("/fixture/duck.mcd"), StateEmulator::DuckStation);

    let shown = text(&render_section(
        &context,
        &mut app,
        &record,
        Err("This game has no verified identity yet, so EmuWiz cannot tie a snapshot to it."),
        None,
    ));
    assert!(shown.iter().any(|s| s.contains("Mr Wiz")));
    assert!(shown.iter().any(|s| s.contains("no verified identity")));

    let target = target_for(&record.path, StateEmulator::DuckStation);
    app.saves_states.snapshots.report = Some(report_for(&record.path, EmulatorUseStatus::Unknown));
    let shown = click_create(&context, &mut app, &record, &target);
    assert!(
        shown
            .iter()
            .any(|s| s.contains("cannot yet confirm this emulator is closed"))
    );
    assert!(app.saves_states.snapshots.create_job.is_none());
}
