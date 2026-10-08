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
