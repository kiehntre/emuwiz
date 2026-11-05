//! Disposable-fixture tests for Blue's update front end.  No network, no real
//! process table and no installed emulator is touched: every executable lives in
//! a temporary directory and the backend is a fixture.

use super::*;
use archivefs_core::emulator_inventory::{
    BuildChannel, EmulatorInventory, InstallationType, InventoryEmulator, SaveStateRisk,
    UpdateCapability, VersionConfidence, VersionSource,
};
use archivefs_core::emulator_update::{UpdateMetadataSource, UpdateTransactionState};
use std::os::unix::fs::PermissionsExt;

const NEW_BYTES: &[u8] = b"new emulator";
const NEW_SHA: &str = "09bd991b6e746a27e5b2305e09b52553dd10a6becc9dfc054566a8d5b09daedf";

struct Fixture {
    quiescence: QuiescenceEvidence,
    bytes: &'static [u8],
    sha: Option<&'static str>,
    resolver_error: Option<&'static str>,
}

impl Fixture {
    fn ok() -> Self {
        Self {
            quiescence: QuiescenceEvidence::Stopped,
            bytes: NEW_BYTES,
            sha: Some(NEW_SHA),
            resolver_error: None,
        }
    }
}

struct Bytes(&'static [u8]);
impl UpdateDownloader for Bytes {
    fn download(&mut self, _: &str, out: &mut std::fs::File) -> Result<(), UpdateExecutionError> {
        use std::io::Write;
        out.write_all(self.0)
            .map_err(|e| UpdateExecutionError::Io(e.to_string()))
    }
}

impl UpdateBackend for Fixture {
    fn check(&self, installations: &[EmulatorInstallation]) -> UpdateReport {
        UpdateReport {
            results: installations.iter().map(update_for).collect(),
        }
    }
    fn artifact(
        &self,
        _: &EmulatorInstallation,
        _: &UpdateResult,
    ) -> Result<UpdateArtifact, String> {
        if let Some(error) = self.resolver_error {
            return Err(error.into());
        }
        Ok(UpdateArtifact {
            version: "2.0".into(),
            channel: BuildChannel::Stable,
            url: "https://example.invalid/synthetic".into(),
            sha256: self.sha.map(Into::into),
            source: UpdateMetadataSource::OfficialReleaseApi,
            provenance: "synthetic".into(),
        })
    }
    fn downloader(&self) -> Box<dyn UpdateDownloader + Send> {
        Box::new(Bytes(self.bytes))
    }
    fn apply_quiescence(&self, tracked: bool, _: &Path) -> QuiescenceEvidence {
        QuiescenceEvidence::from_tracked_flag(tracked).combine(self.quiescence)
    }
    fn journal_quiescence(&self, tracked: bool, _: &UpdateJournal) -> QuiescenceEvidence {
        QuiescenceEvidence::from_tracked_flag(tracked).combine(self.quiescence)
    }
}

fn install(root: &Path) -> EmulatorInstallation {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(root.join("synthetic-emulator"), b"old emulator").unwrap();
    EmulatorInstallation {
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
    }
}

fn update_for(i: &EmulatorInstallation) -> UpdateResult {
    UpdateResult {
        emulator: i.emulator,
        executable_path: i.executable_path.clone(),
        installed_version: i.version.clone(),
        installed_channel: BuildChannel::Stable,
        available_version: Some("2.0".into()),
        available_channel: BuildChannel::Stable,
        status: UpdateStatus::UpdateAvailable,
        source: UpdateMetadataSource::OfficialReleaseApi,
        provenance: "synthetic".into(),
        checked_unix_seconds: 1,
        warning: None,
        save_state_warning: false,
    }
}

fn fixture_page(
    root: &Path,
    fixture: Fixture,
) -> (EmulatorInventoryPageState, EmulatorInstallation) {
    let installation = install(root);
    let mut page = EmulatorInventoryPageState::default();
    page.blue.backend = Some(Arc::new(fixture));
    page.refresh_with_inventory(
        EmulatorInventory {
            installations: vec![installation.clone()],
            ..Default::default()
        },
        vec![root.into()],
    );
    (page, installation)
}

fn settle(page: &mut EmulatorInventoryPageState) {
    for _ in 0..2000 {
        if !page.poll_blue() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("worker did not finish");
}

fn render(page: &mut EmulatorInventoryPageState) -> String {
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
        egui::CentralPanel::default().show(ctx, |ui| page.show_blue(ui, false));
    });
    let mut rendered = String::new();
    output
        .shapes
        .iter()
        .for_each(|s| text(&s.shape, &mut rendered));
    rendered
}

/// Drives check → preview → (optional typed confirmation) in the page.
fn review(page: &mut EmulatorInventoryPageState, installation: &EmulatorInstallation) {
    let ctx = egui::Context::default();
    page.start_check(&ctx);
    assert!(page.blue_busy());
    settle(page);
    let result = page.update_report.as_ref().unwrap().results[0].clone();
    assert_eq!(result.status, UpdateStatus::UpdateAvailable);
    page.start_review(&ctx, installation.clone(), result);
    settle(page);
}

fn exe(root: &Path) -> Vec<u8> {
    std::fs::read(root.join("synthetic-emulator")).unwrap()
}

#[test]
fn full_flow_check_preview_confirm_update_and_undo_on_a_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(dir.path(), Fixture::ok());
    review(&mut page, &installation);
    let review = page.review.as_ref().expect("preview opened");
    assert_eq!(review.plan.eligibility, UpdateExecutionEligibility::Ready);
    let shown = render(&mut page);
    assert!(shown.contains("Review Dolphin update"), "{shown}");
    assert!(shown.contains(NEW_SHA), "{shown}");
    assert!(shown.contains("UPDATE DOLPHIN"), "{shown}");
    assert_eq!(exe(dir.path()), b"old emulator", "preview changes nothing");

    let ctx = egui::Context::default();
    // Missing or wrong confirmation: nothing runs.
    page.start_apply(&ctx);
    assert!(!page.blue_busy());
    page.confirmation = "update dolphin".into();
    page.start_apply(&ctx);
    assert!(!page.blue_busy());
    assert!(page.error.as_deref().unwrap().contains("UPDATE DOLPHIN"));
    assert_eq!(exe(dir.path()), b"old emulator");

    page.confirmation = "UPDATE DOLPHIN".into();
    page.start_apply(&ctx);
    assert!(
        page.blue_busy(),
        "apply runs on a worker, not the UI thread"
    );
    // A second operation cannot start while one is running.
    page.start_check(&ctx);
    settle(&mut page);
    assert_eq!(exe(dir.path()), NEW_BYTES);
    assert!(page.review.is_none());
    assert!(
        page.feedback
            .as_deref()
            .unwrap()
            .contains("Updated Dolphin from 1.0 to 2.0")
    );
    let journal = page
        .review_journal
        .clone()
        .expect("Undo offered after update");
    assert_eq!(journal.state, UpdateTransactionState::Published);
    assert_eq!(
        std::fs::read(&journal.rollback_path).unwrap(),
        b"old emulator"
    );
    assert!(render(&mut page).contains("Undo Dolphin update"));

    // Undo needs its own typed phrase.
    page.start_undo(&ctx);
    assert!(!page.blue_busy());
    assert_eq!(exe(dir.path()), NEW_BYTES);
    page.rollback_confirmation = "ROLL BACK DOLPHIN".into();
    page.start_undo(&ctx);
    settle(&mut page);
    assert_eq!(exe(dir.path()), b"old emulator");
    assert!(
        page.feedback
            .as_deref()
            .unwrap()
            .contains("Restored Dolphin 1.0")
    );
    assert!(page.review_journal.is_none());
    // Nothing was deleted: the newer program is kept where Undo reported it.
    let feedback = page.feedback.clone().unwrap();
    let kept = feedback
        .split("was kept at ")
        .nth(1)
        .expect("Undo reports where the replaced program was kept")
        .trim_end_matches('.');
    assert_eq!(std::fs::read(kept).unwrap(), NEW_BYTES);
}

#[test]
fn unknown_or_running_emulator_blocks_update_and_changes_nothing() {
    for evidence in [QuiescenceEvidence::Unknown, QuiescenceEvidence::Running] {
        let dir = tempfile::tempdir().unwrap();
        let (mut page, installation) = fixture_page(
            dir.path(),
            Fixture {
                quiescence: evidence,
                ..Fixture::ok()
            },
        );
        review(&mut page, &installation);
        let plan = &page.review.as_ref().unwrap().plan;
        assert_ne!(plan.eligibility, UpdateExecutionEligibility::Ready);
        let shown = render(&mut page);
        assert!(shown.contains("cannot be applied safely"), "{shown}");
        assert!(!shown.contains("to confirm"), "{shown}");
        assert!(!shown.contains("press Update now"), "{shown}");
        page.confirmation = "UPDATE DOLPHIN".into();
        page.start_apply(&egui::Context::default());
        assert!(!page.blue_busy());
        assert_eq!(exe(dir.path()), b"old emulator");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no staging or backup created"
        );
    }
}

#[test]
fn missing_checksum_or_unresolvable_source_never_opens_an_applicable_review() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(
        dir.path(),
        Fixture {
            sha: None,
            ..Fixture::ok()
        },
    );
    review(&mut page, &installation);
    let eligibility = page.review.as_ref().map(|r| r.plan.eligibility);
    assert_ne!(eligibility, Some(UpdateExecutionEligibility::Ready));
    assert_eq!(exe(dir.path()), b"old emulator");

    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(
        dir.path(),
        Fixture {
            resolver_error: Some("No official artifact resolver is configured for this emulator."),
            ..Fixture::ok()
        },
    );
    review(&mut page, &installation);
    assert!(page.review.is_none());
    assert!(
        page.error
            .as_deref()
            .unwrap()
            .contains("No official artifact resolver")
    );
}

#[test]
fn corrupt_download_is_rejected_and_the_original_survives() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(
        dir.path(),
        Fixture {
            bytes: b"tampered bytes",
            ..Fixture::ok()
        },
    );
    review(&mut page, &installation);
    page.confirmation = "UPDATE DOLPHIN".into();
    page.start_apply(&egui::Context::default());
    settle(&mut page);
    assert_eq!(exe(dir.path()), b"old emulator");
    let error = page.error.as_deref().unwrap();
    assert!(error.contains("did not complete"), "{error}");
    assert!(page.review.is_none() && page.review_journal.is_none());
    assert!(page.confirmation.is_empty());
}

#[test]
fn interrupted_update_is_recovered_through_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(dir.path(), Fixture::ok());
    review(&mut page, &installation);
    page.confirmation = "UPDATE DOLPHIN".into();
    let ctx = egui::Context::default();
    page.start_apply(&ctx);
    settle(&mut page);
    let mut journal = page.review_journal.clone().unwrap();
    // Simulate a crash after the executable moved aside but before publishing.
    let stage = journal
        .target_path
        .with_file_name(format!(".emuwiz-update-staging-{}", journal.transaction_id));
    std::fs::rename(&journal.target_path, &stage).unwrap();
    journal.staged_path = Some(stage);
    journal.state = UpdateTransactionState::Applying;
    std::fs::write(
        journal.record_path().unwrap(),
        serde_json::to_vec(&journal).unwrap(),
    )
    .unwrap();
    page.refresh_with_inventory(
        EmulatorInventory {
            installations: vec![installation],
            ..Default::default()
        },
        vec![dir.path().into()],
    );
    let shown = render(&mut page);
    assert!(shown.contains("Recovery required"), "{shown}");
    let entry = page
        .records
        .iter()
        .find(|e| e.needs_attention())
        .unwrap()
        .clone();
    page.start_recover(&ctx, entry.path.clone(), entry.journal.clone().unwrap());
    assert!(page.blue_busy());
    settle(&mut page);
    assert!(
        page.feedback
            .as_deref()
            .unwrap()
            .contains("Recovery reconciled")
    );
    assert!(!page.records.iter().any(|e| e.needs_attention()));
}

#[test]
fn progress_is_shown_while_a_worker_runs_and_controls_pause() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, _) = fixture_page(dir.path(), Fixture::ok());
    let (release, hold) = mpsc::channel::<()>();
    page.spawn(
        &egui::Context::default(),
        dir.path().into(),
        "Looking up the official download and its published checksum…",
        move |_| {
            let _ = hold.recv();
            JobDone::Checked(UpdateReport::default())
        },
    );
    let shown = render(&mut page);
    assert!(
        shown.contains("Looking up the official download"),
        "{shown}"
    );
    assert!(shown.contains("paused until this finishes"), "{shown}");
    release.send(()).unwrap();
    settle(&mut page);
    assert!(!page.blue_busy());
}

#[test]
fn a_worker_that_dies_is_reported_and_records_are_re_read() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, _) = fixture_page(dir.path(), Fixture::ok());
    page.spawn(
        &egui::Context::default(),
        dir.path().into(),
        "Working…",
        |_| panic!("synthetic worker failure"),
    );
    settle(&mut page);
    assert!(
        page.error
            .as_deref()
            .unwrap()
            .contains("stopped unexpectedly")
    );
    assert!(page.review.is_none() && page.review_journal.is_none());
}

#[test]
fn an_emuwiz_tracked_running_emulator_blocks_update_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(dir.path(), Fixture::ok());
    page.set_tracked(true);
    review(&mut page, &installation);
    assert_eq!(
        page.review.as_ref().unwrap().plan.eligibility,
        UpdateExecutionEligibility::RunningBlocked
    );
    page.confirmation = "UPDATE DOLPHIN".into();
    page.start_apply(&egui::Context::default());
    assert!(!page.blue_busy());
    assert_eq!(exe(dir.path()), b"old emulator");

    // The flag is re-read at apply time: a preview opened while stopped is still
    // refused by the worker if the emulator is running when Apply is pressed.
    page.set_tracked(false);
    review(&mut page, &installation);
    page.confirmation = "UPDATE DOLPHIN".into();
    page.start_apply(&egui::Context::default());
    settle(&mut page);
    assert_eq!(exe(dir.path()), NEW_BYTES);
    page.set_tracked(true);
    page.rollback_confirmation = "ROLL BACK DOLPHIN".into();
    page.start_undo(&egui::Context::default());
    settle(&mut page);
    assert_eq!(exe(dir.path()), NEW_BYTES, "Undo refused while running");
    assert!(page.error.is_some());
}

#[test]
fn a_new_check_discards_the_old_preview_and_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(dir.path(), Fixture::ok());
    review(&mut page, &installation);
    page.confirmation = "UPDATE DOLPHIN".into();
    let ctx = egui::Context::default();
    page.start_check(&ctx);
    settle(&mut page);
    assert!(page.review.is_none());
    assert!(page.confirmation.is_empty());
    page.start_apply(&ctx);
    assert!(!page.blue_busy());
    assert_eq!(exe(dir.path()), b"old emulator");
}

#[test]
fn a_launch_during_the_download_is_seen_by_the_final_check() {
    // The worker reads the live flag when each quiescence check runs, not the
    // value from when Apply was pressed.
    struct LaunchesMidDownload(Arc<AtomicBool>);
    impl UpdateDownloader for LaunchesMidDownload {
        fn download(
            &mut self,
            _: &str,
            out: &mut std::fs::File,
        ) -> Result<(), UpdateExecutionError> {
            use std::io::Write;
            self.0.store(true, Ordering::SeqCst); // the user launches an emulator now
            out.write_all(NEW_BYTES)
                .map_err(|e| UpdateExecutionError::Io(e.to_string()))
        }
    }
    struct Racing(Fixture, Arc<AtomicBool>);
    impl UpdateBackend for Racing {
        fn check(&self, i: &[EmulatorInstallation]) -> UpdateReport {
            self.0.check(i)
        }
        fn artifact(
            &self,
            i: &EmulatorInstallation,
            u: &UpdateResult,
        ) -> Result<UpdateArtifact, String> {
            self.0.artifact(i, u)
        }
        fn downloader(&self) -> Box<dyn UpdateDownloader + Send> {
            Box::new(LaunchesMidDownload(self.1.clone()))
        }
        fn apply_quiescence(&self, t: bool, e: &Path) -> QuiescenceEvidence {
            self.0.apply_quiescence(t, e)
        }
        fn journal_quiescence(&self, t: bool, j: &UpdateJournal) -> QuiescenceEvidence {
            self.0.journal_quiescence(t, j)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(dir.path(), Fixture::ok());
    page.blue.backend = Some(Arc::new(Racing(Fixture::ok(), page.blue.tracked.clone())));
    review(&mut page, &installation);
    page.confirmation = "UPDATE DOLPHIN".into();
    page.start_apply(&egui::Context::default());
    settle(&mut page);
    assert_eq!(
        exe(dir.path()),
        b"old emulator",
        "refused before the executable moved"
    );
    assert!(page.error.is_some());
}

#[test]
fn a_typed_undo_phrase_does_not_survive_a_new_update() {
    let dir = tempfile::tempdir().unwrap();
    let (mut page, installation) = fixture_page(dir.path(), Fixture::ok());
    page.rollback_confirmation = "ROLL BACK DOLPHIN".into();
    review(&mut page, &installation);
    page.confirmation = "UPDATE DOLPHIN".into();
    page.start_apply(&egui::Context::default());
    settle(&mut page);
    assert!(page.review_journal.is_some());
    assert!(page.rollback_confirmation.is_empty());
}

#[test]
fn a_check_worker_that_dies_says_nothing_changed() {
    let (mut page, _) = fixture_page(tempfile::tempdir().unwrap().path(), Fixture::ok());
    page.spawn(
        &egui::Context::default(),
        PathBuf::new(),
        "Checking…",
        |_| panic!("synthetic"),
    );
    settle(&mut page);
    assert!(
        page.error
            .as_deref()
            .unwrap()
            .contains("Nothing was changed")
    );
}
