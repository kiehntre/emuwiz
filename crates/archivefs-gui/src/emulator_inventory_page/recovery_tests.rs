// Included inside this page's existing tests. No system inventory or real /proc.
fn gui_fixture_update(
    root: &std::path::Path,
) -> (EmulatorInstallation, UpdateResult, UpdateJournal) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    use archivefs_core::emulator_inventory::{
        BuildChannel, SaveStateRisk, UpdateCapability, VersionConfidence, VersionSource,
    };
    use archivefs_core::emulator_update::{UpdateDownloader, UpdateMetadataSource};
    std::fs::write(root.join("synthetic-emulator"), b"old emulator").unwrap();
    let install = EmulatorInstallation {
        emulator: InventoryEmulator::Dolphin,
        executable_path: root.join("synthetic-emulator"),
        installation_root: root.into(),
        version: Some("1.0".into()),
        version_confidence: VersionConfidence::ProfileEvidence,
        version_source: VersionSource::Profile,
        channel: BuildChannel::Stable,
        installation_type: InstallationType::Portable,
        update_capability: UpdateCapability::PortableManaged,
        preferred: None,
        save_state_risk: SaveStateRisk::Unknown,
        warnings: vec![],
    };
    let update = UpdateResult {
        emulator: install.emulator,
        executable_path: install.executable_path.clone(),
        installed_version: install.version.clone(),
        installed_channel: BuildChannel::Stable,
        available_version: Some("2.0".into()),
        available_channel: BuildChannel::Stable,
        status: UpdateStatus::UpdateAvailable,
        source: UpdateMetadataSource::OfficialReleaseApi,
        provenance: "synthetic".into(),
        checked_unix_seconds: 1,
        warning: None,
        save_state_warning: false,
    };
    let artifact = UpdateArtifact {
        version: "2.0".into(),
        channel: BuildChannel::Stable,
        url: "https://example.invalid/synthetic".into(),
        sha256: Some("09bd991b6e746a27e5b2305e09b52553dd10a6becc9dfc054566a8d5b09daedf".into()),
        source: UpdateMetadataSource::OfficialReleaseApi,
        provenance: "synthetic".into(),
    };
    struct Bytes;
    impl UpdateDownloader for Bytes {
        fn download(
            &mut self,
            _: &str,
            out: &mut std::fs::File,
        ) -> Result<(), UpdateExecutionError> {
            use std::io::Write;
            out.write_all(b"new emulator")
                .map_err(|e| UpdateExecutionError::Io(e.to_string()))
        }
    }
    let plan = plan_staged_update(&install, &update, artifact, QuiescenceEvidence::Stopped);
    let journal = execute_staged_update(
        &plan,
        &install,
        &update,
        || QuiescenceEvidence::Stopped,
        &mut Bytes,
    )
    .unwrap();
    (install, update, journal)
}
fn gui_interrupted_missing_target(j: &mut UpdateJournal) {
    let stage = j
        .target_path
        .with_file_name(format!(".emuwiz-update-staging-{}", j.transaction_id));
    std::fs::rename(&j.target_path, &stage).unwrap();
    j.staged_path = Some(stage);
    j.state = UpdateTransactionState::Applying;
    std::fs::write(j.record_path().unwrap(), serde_json::to_vec(j).unwrap()).unwrap();
}
#[test]
fn restart_discovery_retains_records_when_inventory_has_no_executable() {
    let root = tempfile::tempdir().unwrap();
    let (_, _, mut j) = gui_fixture_update(root.path());
    gui_interrupted_missing_target(&mut j);
    let roots = remembered_installation_roots().unwrap();
    assert!(roots.contains(&root.path().to_path_buf()));
    let mut page = EmulatorInventoryPageState::default();
    page.refresh_with_inventory(EmulatorInventory::default(), vec![root.path().into()]);
    assert_eq!(page.records.len(), 1);
    assert!(page.records[0].needs_attention());
    assert!(page.review_journal.is_none());
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| page.show(ui, false));
    });
    fn text(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    text(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut rendered = String::new();
    for shape in &output.shapes {
        text(&shape.shape, &mut rendered);
    }
    assert!(
        rendered.contains("Interrupted or unreadable update records"),
        "{rendered}"
    );
    assert!(rendered.contains("Recover (conservative)"), "{rendered}");
    assert!(
        rendered.contains(&j.target_path.display().to_string()),
        "{rendered}"
    );
    assert_eq!(page.records.len(), 1);
}
#[test]
fn restart_undo_is_explicit_and_associated_with_the_correct_installation() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let (i1, _, j1) = gui_fixture_update(first.path());
    let (i2, _, j2) = gui_fixture_update(second.path());
    let mut page = EmulatorInventoryPageState::default();
    page.refresh_with_inventory(
        EmulatorInventory {
            installations: vec![i1.clone(), i2.clone()],
            ..Default::default()
        },
        vec![],
    );
    assert!(page.review_journal.is_none());
    page.records.reverse();
    page.rollback_confirmation = "ROLL BACK DOLPHIN".into();
    page.review_undo_for(&i1.executable_path);
    assert_eq!(page.review_journal, Some(j1));
    assert!(page.rollback_confirmation.is_empty());
    page.review_undo_for(&i2.executable_path);
    assert_eq!(page.review_journal, Some(j2));
    page.review_undo_for(&first.path().join("unknown-installation"));
    assert!(page.review_journal.is_none());
}
#[test]
fn restart_recovery_restores_verified_original_and_refreshes_saved_state() {
    let root = tempfile::tempdir().unwrap();
    let (_, _, mut j) = gui_fixture_update(root.path());
    gui_interrupted_missing_target(&mut j);
    let mut page = EmulatorInventoryPageState::default();
    page.refresh_with_inventory(EmulatorInventory::default(), vec![root.path().into()]);
    page.recover_record_with(&j.record_path().unwrap(), || QuiescenceEvidence::Stopped);
    assert!(page.error.is_none());
    assert!(
        page.feedback
            .as_ref()
            .unwrap()
            .contains("Recovery reconciled")
    );
    assert_eq!(std::fs::read(&j.target_path).unwrap(), b"old emulator");
    assert!(!page.records[0].needs_attention());
}
#[test]
fn restart_recovery_refusal_clears_old_success_and_preserves_external_executable() {
    let root = tempfile::tempdir().unwrap();
    let (_, _, mut j) = gui_fixture_update(root.path());
    gui_interrupted_missing_target(&mut j);
    std::fs::write(&j.target_path, b"external executable").unwrap();
    let mut page = EmulatorInventoryPageState::default();
    page.refresh_with_inventory(EmulatorInventory::default(), vec![root.path().into()]);
    page.feedback = Some("old successful restoration".into());
    page.recover_record_with(&j.record_path().unwrap(), || QuiescenceEvidence::Stopped);
    assert!(page.feedback.is_none());
    assert!(page.error.as_ref().unwrap().contains("did not complete"));
    assert_eq!(
        std::fs::read(&j.target_path).unwrap(),
        b"external executable"
    );
}
#[test]
fn duplicate_or_legacy_ordering_never_selects_an_arbitrary_undo() {
    let root = tempfile::tempdir().unwrap();
    let (_, _, mut j) = gui_fixture_update(root.path());
    j.sequence = None;
    std::fs::write(j.record_path().unwrap(), serde_json::to_vec(&j).unwrap()).unwrap();
    let mut records = discover_update_records(root.path());
    records.push(records[0].clone());
    assert!(actionable_undo(&records, &j.target_path).is_none());
}
#[test]
fn terminal_disk_conflict_is_visible_after_restart_and_not_offered_as_undo() {
    let root = tempfile::tempdir().unwrap();
    let (_, _, j) = gui_fixture_update(root.path());
    std::fs::write(&j.target_path, b"user custom build").unwrap();
    let mut page = EmulatorInventoryPageState::default();
    page.refresh_with_inventory(EmulatorInventory::default(), vec![root.path().into()]);
    assert!(page.records[0].needs_attention());
    assert!(actionable_undo(&page.records, &j.target_path).is_none());
}

fn rendered(page: &mut EmulatorInventoryPageState) -> String {
    fn text(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| text(s, out)),
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| page.show(ui, false));
    });
    let mut out = String::new();
    for shape in &output.shapes {
        text(&shape.shape, &mut out);
    }
    out
}

#[test]
fn undo_that_stops_after_the_executable_moved_requires_recovery_and_drops_the_stale_offer() {
    let root = tempfile::tempdir().unwrap();
    let (install, _, journal) = gui_fixture_update(root.path());
    let mut page = EmulatorInventoryPageState::default();
    let inventory = EmulatorInventory {
        installations: vec![install.clone()],
        ..Default::default()
    };
    page.refresh_with_inventory(inventory.clone(), vec![]);
    page.review_undo_for(&install.executable_path);
    assert_eq!(page.review_journal.as_ref(), Some(&journal));
    page.rollback_confirmation = "ROLL BACK DOLPHIN".into();
    // The emulator becomes unverifiable AFTER the published executable moved.
    let calls = std::cell::Cell::new(0);
    page.rollback_review_with(|| {
        calls.set(calls.get() + 1);
        if calls.get() >= 3 {
            QuiescenceEvidence::Unknown
        } else {
            QuiescenceEvidence::Stopped
        }
    });
    let message = page.error.clone().unwrap();
    assert!(message.contains("RECOVERY REQUIRED"), "{message}");
    assert!(message.contains("already moved"), "{message}");
    assert!(
        !message.contains("No executable was replaced"),
        "a partial operation must not claim nothing changed: {message}"
    );
    // Stale snapshots and confirmations are gone; saved state was re-read.
    assert!(page.review_journal.is_none() && page.review.is_none());
    assert!(page.rollback_confirmation.is_empty() && page.confirmation.is_empty());
    assert!(!journal.target_path.exists());
    assert_eq!(page.records.len(), 1);
    assert!(page.records[0].needs_attention());
    assert!(actionable_undo(&page.records, &journal.target_path).is_none());
    let shown = rendered(&mut page);
    assert!(shown.contains("Recover (conservative)"), "{shown}");
    assert!(!shown.contains("Review Undo"), "{shown}");

    // Restart (a fresh page) shows the same obligation; recovery completes once
    // and a repeat is a harmless no-op.
    let record = journal.record_path().unwrap();
    let mut restarted = EmulatorInventoryPageState::default();
    restarted.refresh_with_inventory(inventory, vec![root.path().into()]);
    assert!(restarted.records[0].needs_attention());
    restarted.recover_record_with(&record, || QuiescenceEvidence::Stopped);
    assert!(restarted.error.is_none(), "{:?}", restarted.error);
    assert_eq!(
        std::fs::read(&journal.target_path).unwrap(),
        b"new emulator"
    );
    assert!(!restarted.records[0].needs_attention());
    let saved = std::fs::read(&record).unwrap();
    restarted.recover_record_with(&record, || QuiescenceEvidence::Stopped);
    assert!(restarted.error.is_none());
    assert_eq!(std::fs::read(&record).unwrap(), saved);
    assert!(actionable_undo(&restarted.records, &journal.target_path).is_some());
}

#[test]
fn legacy_records_show_no_undo_control_and_say_why() {
    let root = tempfile::tempdir().unwrap();
    let (install, _, journal) = gui_fixture_update(root.path());
    let mut old = serde_json::to_value(&journal).unwrap();
    for key in [
        "sequence",
        "root_binding",
        "target_parent_binding",
        "original_identity",
        "staged_identity",
    ] {
        old.as_object_mut().unwrap().remove(key);
    }
    std::fs::write(
        journal.record_path().unwrap(),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let mut page = EmulatorInventoryPageState::default();
    page.refresh_with_inventory(
        EmulatorInventory {
            installations: vec![install.clone()],
            ..Default::default()
        },
        vec![],
    );
    assert!(actionable_undo(&page.records, &install.executable_path).is_none());
    page.review_undo_for(&install.executable_path);
    assert!(page.review_journal.is_none());
    let shown = rendered(&mut page);
    assert!(!shown.contains("Review Undo"), "{shown}");
    assert!(shown.contains("Undo unavailable"), "{shown}");
    assert_eq!(
        std::fs::read(&install.executable_path).unwrap(),
        b"new emulator"
    );
}

#[test]
fn unknown_process_visibility_is_labelled_differently_from_a_running_emulator() {
    assert_ne!(
        eligibility_label(UpdateExecutionEligibility::QuiescenceUnknown),
        eligibility_label(UpdateExecutionEligibility::RunningBlocked)
    );
    assert!(
        eligibility_label(UpdateExecutionEligibility::QuiescenceUnknown).contains("cannot verify")
    );
    use archivefs_core::emulator_inventory::BuildChannel;
    use archivefs_core::emulator_update::UpdateMetadataSource;
    let root = tempfile::tempdir().unwrap();
    let (install, update, _) = gui_fixture_update(root.path());
    let artifact = UpdateArtifact {
        version: "2.0".into(),
        channel: BuildChannel::Stable,
        url: "https://example.invalid/synthetic".into(),
        sha256: Some("09bd991b6e746a27e5b2305e09b52553dd10a6becc9dfc054566a8d5b09daedf".into()),
        source: UpdateMetadataSource::OfficialReleaseApi,
        provenance: "synthetic".into(),
    };
    let label = |evidence| {
        eligibility_label(
            plan_staged_update(&install, &update, artifact.clone(), evidence).eligibility,
        )
    };
    assert_eq!(
        label(QuiescenceEvidence::Running),
        "Blocked: emulator is running"
    );
    assert!(label(QuiescenceEvidence::Unknown).contains("cannot verify"));
    assert_ne!(
        label(QuiescenceEvidence::Unknown),
        label(QuiescenceEvidence::Running)
    );
}
