use super::*;
use crate::attention::{AttentionFilters, AttentionState};

fn history(
    success: bool,
    timestamp: &str,
    kind: crate::FrontendProfileKind,
) -> LibraryViewHistoryRecord {
    LibraryViewHistoryRecord {
        schema_version: 1,
        timestamp: timestamp.into(),
        operation: LibraryViewHistoryOperation::Apply,
        view_id: "view-1".into(),
        view_name: "Test view".into(),
        profile_kind: kind,
        destination_root: "/absent/destination".into(),
        manifest_path: "/absent/manifest".into(),
        planned_count: 3,
        created: 0,
        repaired: 0,
        removed: 0,
        unchanged: if success { 3 } else { 0 },
        failed: if success { 0 } else { 3 },
        skipped_or_collision: Some(0),
        success,
        warnings: Vec::new(),
    }
}

#[test]
fn attention_receipts_newer_publication_resolves_old_failure_without_destination_probe() {
    for kind in [
        crate::FrontendProfileKind::Generic,
        crate::FrontendProfileKind::Romm,
        crate::FrontendProfileKind::EsDe,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let paths = AttentionReceiptPaths {
            library_views: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        std::fs::write(
            dir.path().join("a.json"),
            serde_json::to_vec(&history(false, "2026-01-01T00:00:00Z", kind)).unwrap(),
        )
        .unwrap();
        let failed = attention_receipt_snapshot(&paths);
        assert_eq!(failed.page(&AttentionFilters::default()).total, 1);
        std::fs::write(
            dir.path().join("invalid-time.json"),
            serde_json::to_vec(&history(true, "zz-invalid-date", kind)).unwrap(),
        )
        .unwrap();
        let invalid_success = attention_receipt_snapshot(&paths);
        assert_eq!(invalid_success.page(&AttentionFilters::default()).total, 1);
        assert!(
            invalid_success
                .coverage_notes
                .iter()
                .any(|note| note.contains("timestamp"))
        );
        std::fs::write(
            dir.path().join("b.json"),
            serde_json::to_vec(&history(true, "2026-01-02T00:00:00Z", kind)).unwrap(),
        )
        .unwrap();
        let before: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|p| {
                let path = p.unwrap().path();
                (path.clone(), std::fs::read(path).unwrap())
            })
            .collect();
        let resolved = attention_receipt_snapshot(&paths);
        assert_eq!(resolved.page(&AttentionFilters::default()).total, 0);
        assert_eq!(resolved.items().count(), 1);
        assert_eq!(
            resolved.items().next().unwrap().state,
            AttentionState::Resolved
        );
        for (path, bytes) in before {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn attention_receipts_malformed_and_unknown_schema_never_look_healthy() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AttentionReceiptPaths {
        library_views: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    std::fs::write(dir.path().join("malformed.json"), b"{}").unwrap();
    let mut future = history(
        true,
        "2026-01-01T00:00:00Z",
        crate::FrontendProfileKind::Generic,
    );
    future.schema_version = 999;
    std::fs::write(
        dir.path().join("future.json"),
        serde_json::to_vec(&future).unwrap(),
    )
    .unwrap();
    let snapshot = attention_receipt_snapshot(&paths);
    assert!(snapshot.limited);
    assert!(
        snapshot
            .coverage_notes
            .iter()
            .any(|s| s.contains("malformed"))
    );
    assert!(
        snapshot
            .coverage_notes
            .iter()
            .any(|s| s.contains("version"))
    );
    assert_eq!(snapshot.items().count(), 0);
}

#[test]
fn attention_receipt_size_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("oversized.json"))
        .unwrap()
        .set_len(MAX_RECEIPT_BYTES + 1)
        .unwrap();
    let snapshot = attention_receipt_snapshot(&AttentionReceiptPaths {
        rename: Some(dir.path().to_path_buf()),
        ..Default::default()
    });
    assert!(snapshot.limited);
    assert_eq!(snapshot.items().count(), 0);
    assert!(
        snapshot
            .coverage_notes
            .iter()
            .any(|s| s.contains("oversized"))
    );
}

#[test]
fn attention_shared_successful_rollback_resolves_only_the_matching_operation() {
    let original = super::super::tests::shared_journal(
        PreviewAdapter::RetroArch,
        SharedApplyStatus::PartialFailure,
    );
    let mut record = shared_apply_operation(&original, None, None);
    let mut receipt = SharedRollbackPreview {
        schema_version: 1,
        preview_id: "preview".into(),
        journal_path: crate::patch_manager::SharedTransactionPath::from_path(Path::new("/journal")),
        original_operation_id: record.operation_id.clone(),
        destination_root: original.destination_root.clone(),
        entries: Vec::new(),
        available: false,
    };
    let mut wrong = record.clone();
    project_completed_shared_rollback(&mut wrong, &receipt);
    assert_eq!(
        wrong.state,
        OperationState::Partial,
        "an empty marker cannot cover a changed output"
    );
    receipt
        .entries
        .push(crate::patch_manager::SharedRollbackEntry {
            destination: Some(crate::patch_manager::SharedTransactionPath::from_path(
                Path::new(&record.output.outputs[0].path),
            )),
            backup: None,
            expected_installed_digest: None,
            observed_destination_digest: None,
            observed_backup_digest: None,
            outcome: SharedRollbackOutcome::RemovedInstalledFile,
            failure: None,
        });
    receipt.original_operation_id = "wrong".into();
    project_completed_shared_rollback(&mut wrong, &receipt);
    assert_eq!(wrong.state, OperationState::Partial);
    receipt
        .original_operation_id
        .clone_from(&record.operation_id);
    project_completed_shared_rollback(&mut record, &receipt);
    assert_eq!(record.state, OperationState::RolledBack);
}
