use super::*;
use crate::cheat_reconciliation_review::CheatReconciliationReviewState;
use archivefs_core::patch_manager::{
    CheatDocument, CheatIssue, CheatOperation, CheatPlatform, CheatReconciliationEntry,
    CheatSourceFormat,
};

fn rendered_text_contains(output: &eframe::egui::FullOutput, needle: &str) -> bool {
    fn contains(shape: &eframe::egui::Shape, needle: &str) -> bool {
        match shape {
            eframe::egui::Shape::Text(text) => text.galley.text().contains(needle),
            eframe::egui::Shape::Vec(shapes) => shapes.iter().any(|s| contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|shape| contains(&shape.shape, needle))
}

fn report() -> CheatReconciliationResult {
    let entries = (1..=2)
        .map(|value| CheatReconciliationEntry {
            game_identity: "GAFE01-r1".into(),
            identity_verified: true,
            title: "Lives".into(),
            source: format!("Local source {value}"),
            source_format: CheatSourceFormat::Gecko,
            document: CheatDocument {
                title: "Lives".into(),
                platform: CheatPlatform::GameCube,
                source_format: CheatSourceFormat::Gecko,
                operations: vec![CheatOperation::Write32 {
                    address: 0x8000_1000,
                    value,
                }],
                issues: vec![],
                provenance: vec!["/private/source.ini".into()],
            },
            raw_code: None,
            provenance: vec!["https://private.example/report?token=secret".into()],
        })
        .collect();
    match reconcile_cheats_for_game(entries) {
        CheatReconciliationOutcome::Ready(report) => report,
        _ => panic!("valid fixture"),
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    path: PathBuf,
    report: CheatReconciliationResult,
    digest: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        let report = report();
        fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
        let (_, digest) = read_report(&path).unwrap();
        Self {
            dir,
            path,
            report,
            digest,
        }
    }
    fn store(&self) -> ReviewStore {
        ReviewStore::new(self.dir.path().join("reviews"))
    }
    fn state(&self) -> CheatReconciliationReviewState {
        let mut state = CheatReconciliationReviewState {
            store: Some(self.store()),
            ..Default::default()
        };
        state.open_report(self.path.clone());
        state
    }
    fn saved_path(&self) -> PathBuf {
        self.store().path(&self.digest).unwrap()
    }
}

#[test]
fn initial_read_does_not_create_state_or_select_a_winner() {
    let f = Fixture::new();
    let state = f.state();
    assert!(state.choices.is_empty());
    assert!(state.persistence_warning.is_none());
    assert!(!state.show_all);
    assert!(!f.store().root.exists());
    assert!(f.report.auto_winner.is_none());
}

#[test]
fn every_explicit_review_choice_survives_restart_and_page_recreation() {
    for choice in [
        ReviewChoice::KeepA,
        ReviewChoice::KeepB,
        ReviewChoice::KeepBoth,
        ReviewChoice::Skip,
        ReviewChoice::IgnoreConflict,
    ] {
        let f = Fixture::new();
        let mut first = f.state();
        first.choose(0, choice);
        assert!(
            first.persistence_warning.is_none(),
            "{:?}",
            first.persistence_warning
        );
        assert_eq!(first.saved_choices.get(&0), Some(&choice));
        drop(first); // No shutdown save: the explicit action was already durable.
        let reloaded = f.state();
        assert_eq!(reloaded.choices.get(&0), Some(&choice));
        assert!(reloaded.persistence_warning.is_none());
    }
}

#[test]
fn close_keeps_saved_choices_and_reopen_restores_them() {
    let f = Fixture::new();
    let mut state = f.state();
    state.choose(0, ReviewChoice::Skip);
    state.close_report();
    assert!(state.report.is_none());
    assert!(state.choices.is_empty());
    state.open_report(f.path.clone());
    assert_eq!(state.choices.get(&0), Some(&ReviewChoice::Skip));
}

#[test]
fn navigation_frames_retain_choices_and_do_not_rewrite_state() {
    let f = Fixture::new();
    let mut state = f.state();
    state.choose(0, ReviewChoice::KeepA);
    let before = fs::metadata(f.saved_path()).unwrap().modified().unwrap();
    let context = eframe::egui::Context::default();
    let _ = context.run(Default::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            state.show(ui);
        });
    });
    assert_eq!(state.choices.get(&0), Some(&ReviewChoice::KeepA));
    assert_eq!(
        before,
        fs::metadata(f.saved_path()).unwrap().modified().unwrap()
    );
}

#[test]
fn corrupt_json_is_visible_preserved_and_never_overwritten_on_retry() {
    let f = Fixture::new();
    f.state().choose(0, ReviewChoice::KeepA);
    fs::write(f.saved_path(), b"{ truncated").unwrap();
    let mut state = f.state();
    assert!(state.choices.is_empty());
    assert!(
        state
            .persistence_warning
            .as_ref()
            .unwrap()
            .contains("damaged")
    );
    state.choose(0, ReviewChoice::KeepB);
    assert!(state.persistence_warning.is_some());
    assert_eq!(fs::read(f.saved_path()).unwrap(), b"{ truncated");
}

#[test]
fn incompatible_version_is_visible_and_preserved() {
    let f = Fixture::new();
    f.state().choose(0, ReviewChoice::KeepA);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(f.saved_path()).unwrap()).unwrap();
    value["version"] = 999.into();
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(f.saved_path(), &bytes).unwrap();
    let mut state = f.state();
    assert!(state.choices.is_empty());
    assert!(
        state
            .persistence_warning
            .as_ref()
            .unwrap()
            .contains("unsupported version 999")
    );
    state.choose(0, ReviewChoice::KeepB);
    assert_eq!(fs::read(f.saved_path()).unwrap(), bytes);
}

#[test]
fn deleted_state_does_not_resurrect_old_decisions() {
    let f = Fixture::new();
    let mut state = f.state();
    state.choose(0, ReviewChoice::KeepA);
    fs::remove_file(f.saved_path()).unwrap();
    state.choose(0, ReviewChoice::KeepB);
    assert!(
        state
            .persistence_warning
            .as_ref()
            .unwrap()
            .contains("removed")
    );
    assert!(!f.saved_path().exists());
    assert!(f.state().choices.is_empty());
}

#[test]
fn changed_report_invalidates_choices_even_at_same_path() {
    let f = Fixture::new();
    let mut state = f.state();
    state.choose(0, ReviewChoice::KeepA);
    let saved = fs::read(f.saved_path()).unwrap();
    // Even harmless reformatting conservatively makes a new report identity.
    fs::write(&f.path, serde_json::to_vec_pretty(&f.report).unwrap()).unwrap();
    state.choose(0, ReviewChoice::KeepB);
    assert!(
        state
            .persistence_warning
            .as_ref()
            .unwrap()
            .contains("changed on disk")
    );
    assert_eq!(fs::read(f.saved_path()).unwrap(), saved);
    let reopened = f.state();
    assert!(reopened.choices.is_empty());
    assert_ne!(reopened.report_sha256.as_deref(), Some(f.digest.as_str()));
}

#[test]
fn identical_report_moved_to_another_path_restores_choices_without_storing_paths() {
    let f = Fixture::new();
    f.state().choose(0, ReviewChoice::KeepB);
    let moved = f.dir.path().join("renamed.json");
    fs::rename(&f.path, &moved).unwrap();
    let mut state = f.state();
    assert!(matches!(state.report, Some(Err(_))));
    state.open_report(moved);
    assert_eq!(state.choices.get(&0), Some(&ReviewChoice::KeepB));
}

#[test]
fn missing_report_blocks_save_without_touching_saved_choices() {
    let f = Fixture::new();
    let mut state = f.state();
    state.choose(0, ReviewChoice::Skip);
    let saved = fs::read(f.saved_path()).unwrap();
    fs::remove_file(&f.path).unwrap();
    state.choose(0, ReviewChoice::KeepBoth);
    assert!(state.persistence_warning.is_some());
    assert_eq!(fs::read(f.saved_path()).unwrap(), saved);
}

#[test]
fn mismatched_digest_and_invalid_group_never_restore() {
    let f = Fixture::new();
    f.state().choose(0, ReviewChoice::Skip);
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(f.saved_path()).unwrap()).unwrap();
    for (field, bad) in [
        ("report_sha256", serde_json::json!("0".repeat(64))),
        ("choices", serde_json::json!({"5000":"KeepA"})),
    ] {
        let mut value = original.clone();
        value[field] = bad;
        fs::write(f.saved_path(), serde_json::to_vec(&value).unwrap()).unwrap();
        let state = f.state();
        assert!(state.choices.is_empty());
        assert!(state.persistence_warning.is_some());
    }
}

#[test]
fn report_schema_and_broken_references_fail_explicitly() {
    let f = Fixture::new();
    for bad in [serde_json::json!({}), {
        let mut value = serde_json::to_value(&f.report).unwrap();
        value["groups"][0]["entry_indices"] = serde_json::json!([]);
        value
    }] {
        fs::write(&f.path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(matches!(f.state().report, Some(Err(_))));
    }
    assert!(!f.store().root.exists());
}

#[test]
fn existing_cli_filtered_report_contract_still_restores_choices() {
    let f = Fixture::new();
    let mut entries = f.report.entries.clone();
    let mut more = entries.clone();
    for entry in &mut more {
        entry.title = "Ammo".into();
        entry.document.title = "Ammo".into();
        if let CheatOperation::Write32 { address, .. } = &mut entry.document.operations[0] {
            *address += 4;
        }
    }
    entries.extend(more);
    let CheatReconciliationOutcome::Ready(mut full) = reconcile_cheats_for_game(entries) else {
        panic!("valid fixture")
    };
    assert_eq!(full.groups.len(), 2);
    full.groups.remove(0); // Same subset operation as the CLI JSON filter.
    fs::write(&f.path, serde_json::to_vec(&full).unwrap()).unwrap();
    let mut state = f.state();
    assert!(matches!(state.report, Some(Ok(_))));
    state.choose(0, ReviewChoice::KeepB);
    assert!(state.persistence_warning.is_none());
    assert_eq!(f.state().choices.get(&0), Some(&ReviewChoice::KeepB));
}

#[test]
fn malformed_and_unsupported_keep_choices_are_refused() {
    let f = Fixture::new();
    for unsupported in [false, true] {
        let mut report = f.report.clone();
        if unsupported {
            report.entries[0].document.operations = vec![CheatOperation::UnsupportedRaw {
                source_format: CheatSourceFormat::Gecko,
                raw: "bad".into(),
                reason: "Unsupported".into(),
            }];
        } else {
            report.entries[0]
                .document
                .issues
                .push(CheatIssue::RawPreserved);
        }
        for choice in [
            ReviewChoice::KeepA,
            ReviewChoice::KeepB,
            ReviewChoice::KeepBoth,
        ] {
            assert!(!choice_allowed(&report, 0, choice));
            assert!(
                f.store()
                    .save(
                        &f.digest,
                        &report,
                        &BTreeMap::new(),
                        &BTreeMap::from([(0, choice)])
                    )
                    .is_err()
            );
        }
    }
    assert!(!f.store().root.exists());
}

#[test]
fn concurrent_windows_refuse_overwriting_another_review() {
    let f = Fixture::new();
    let mut first = f.state();
    let mut second = f.state();
    first.choose(0, ReviewChoice::KeepA);
    second.choose(0, ReviewChoice::KeepB);
    assert!(
        second
            .persistence_warning
            .as_ref()
            .unwrap()
            .contains("another window")
    );
    assert_eq!(f.state().choices.get(&0), Some(&ReviewChoice::KeepA));
}

#[test]
fn busy_lock_retains_unsaved_choice_and_retry_succeeds_after_release() {
    let f = Fixture::new();
    let mut state = f.state();
    let lock = f.store().lock().unwrap();
    state.choose(0, ReviewChoice::Skip);
    assert!(state.persistence_warning.as_ref().unwrap().contains("busy"));
    assert_eq!(state.choices.get(&0), Some(&ReviewChoice::Skip));
    assert!(state.saved_choices.is_empty());
    drop(lock);
    state.save_choices();
    assert!(state.persistence_warning.is_none());
    assert_eq!(f.state().choices.get(&0), Some(&ReviewChoice::Skip));
}

#[test]
fn interrupted_temp_output_is_ignored_and_previous_state_survives() {
    let f = Fixture::new();
    f.state().choose(0, ReviewChoice::Skip);
    let interrupted = f.store().root.join(".review-crashed.tmp");
    fs::write(&interrupted, b"{partially written").unwrap();
    assert_eq!(f.state().choices.get(&0), Some(&ReviewChoice::Skip));
    f.state().choose(0, ReviewChoice::KeepA);
    assert_eq!(f.state().choices.get(&0), Some(&ReviewChoice::KeepA));
    assert!(interrupted.exists()); // No blind stale-file deletion.
}

#[test]
fn process_exit_without_cleanup_preserves_choices_and_releases_lock() {
    let f = Fixture::new();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cheat_reconciliation_review::persistence::tests::crash_writer_helper",
            "--nocapture",
        ])
        .env("EMUWIZ_REVIEW_CRASH_FIXTURE", f.dir.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(23));
    let mut state = f.state();
    assert_eq!(state.choices.get(&0), Some(&ReviewChoice::Skip));
    state.choose(0, ReviewChoice::KeepA);
    assert!(state.persistence_warning.is_none());
    assert_eq!(f.state().choices.get(&0), Some(&ReviewChoice::KeepA));
}

#[test]
fn crash_writer_helper() {
    let Some(root) = std::env::var_os("EMUWIZ_REVIEW_CRASH_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let mut state = CheatReconciliationReviewState {
        store: Some(ReviewStore::new(root.join("reviews"))),
        ..Default::default()
    };
    state.open_report(root.join("report.json"));
    state.choose(0, ReviewChoice::Skip);
    assert!(state.persistence_warning.is_none());
    let store = state.store.as_ref().unwrap();
    let _lock = store.lock().unwrap();
    fs::write(store.root.join(".review-interrupted.tmp"), b"{unfinished").unwrap();
    // No Rust destructors or application exit hooks; OS owns lock recovery.
    std::process::exit(23);
}

#[test]
fn oversized_files_and_invalid_storage_identity_are_refused() {
    let f = Fixture::new();
    assert!(f.store().path("../../outside").is_err());
    f.state().choose(0, ReviewChoice::Skip);
    File::create(f.saved_path())
        .unwrap()
        .set_len(MAX_STATE_BYTES + 1)
        .unwrap();
    assert!(
        f.state()
            .persistence_warning
            .unwrap()
            .contains("size limit")
    );
    File::create(&f.path)
        .unwrap()
        .set_len(MAX_REPORT_BYTES + 1)
        .unwrap();
    assert!(read_report(&f.path).unwrap_err().contains("size limit"));
}

#[test]
fn failed_publication_preserves_complete_prior_state() {
    let f = Fixture::new();
    f.state().choose(0, ReviewChoice::Skip);
    let previous = fs::read(f.saved_path()).unwrap();
    let impossible_target = f.store().root.join("directory");
    fs::create_dir(&impossible_target).unwrap();
    assert!(f.store().publish(&impossible_target, b"new").is_err());
    assert_eq!(fs::read(f.saved_path()).unwrap(), previous);
    assert!(!fs::read_dir(f.store().root).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|e| e == "tmp")
    }));
}

#[test]
fn saved_payload_contains_only_digest_version_and_choices_and_no_media_mutation() {
    let f = Fixture::new();
    let media = f.dir.path().join("source.rom");
    fs::write(&media, b"preservation media").unwrap();
    let source_before = fs::read(&f.path).unwrap();
    f.state().choose(0, ReviewChoice::KeepBoth);
    let raw = fs::read_to_string(f.saved_path()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 3);
    for private in [
        "source.ini",
        "private.example",
        "secret",
        "GAFE01",
        "Lives",
        "source.rom",
    ] {
        assert!(!raw.contains(private));
    }
    assert_eq!(fs::read(&f.path).unwrap(), source_before);
    assert_eq!(fs::read(media).unwrap(), b"preservation media");
}

#[cfg(unix)]
#[test]
fn private_permissions_and_symlink_refusals() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    f.state().choose(0, ReviewChoice::Skip);
    assert_eq!(
        fs::metadata(f.saved_path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(f.store().root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let backup = f.dir.path().join("saved.json");
    fs::rename(f.saved_path(), &backup).unwrap();
    symlink(&backup, f.saved_path()).unwrap();
    let mut state = f.state();
    assert!(state.persistence_warning.is_some());
    let original = fs::read(&backup).unwrap();
    state.choose(0, ReviewChoice::KeepA);
    assert_eq!(fs::read(backup).unwrap(), original);
}

#[test]
fn rendered_saved_and_failed_feedback_is_explicit() {
    let f = Fixture::new();
    let mut state = f.state();
    state.choose(0, ReviewChoice::KeepA);
    let ctx = eframe::egui::Context::default();
    let output = ctx.run(Default::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            state.show(ui);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "review choices saved locally"
    ));
    state.persistence_warning = Some("Synthetic storage failure".into());
    let output = ctx.run(Default::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            state.show(ui);
        });
    });
    assert!(rendered_text_contains(&output, "Synthetic storage failure"));
    assert!(rendered_text_contains(&output, "Retry saving choices"));
}
