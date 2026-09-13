use super::*;
use crate::operation::OperationRecoveryStatus;

#[test]
fn attention_receipt_time_round_trips_without_inventing_timestamps() {
    for timestamp in [0, -1, 1_700_000_000, 1_772_323_200, 253_402_300_799] {
        assert_eq!(
            receipt_utc_seconds(&crate::format_unix_timestamp_utc(timestamp)),
            Some(timestamp)
        );
    }
    assert_eq!(receipt_utc_seconds("2026-02-30T00:00:00Z"), None);
    assert_eq!(receipt_utc_seconds("unavailable"), None);
}

fn record(kind: OperationKind, state: OperationState) -> OperationRecord {
    OperationRecord {
        schema_version: 1,
        operation_id: "receipt-1".into(),
        kind,
        state,
        created_at_unix: 100,
        started_at_unix: None,
        completed_at_unix: Some(101),
        input: Default::default(),
        destination: Some("/disposable/output".into()),
        output: Default::default(),
        recovery: OperationRecoveryStatus {
            classification: RecoveryClassification::RequiresReview,
            explanation: "Review saved evidence; recovery will revalidate".into(),
            actions: Default::default(),
        },
        error: None,
    }
}

#[test]
fn attention_database_recovery_failure_blocks_without_inventing_backup_safety() {
    let item = operation_attention(&record(
        OperationKind::DatabaseRecovery,
        OperationState::Failed,
    ))
    .unwrap();
    assert_eq!(item.severity, AttentionSeverity::Blocking);
    assert_eq!(item.destination, AttentionDestination::History);
    assert!(!item.summary.contains("intact"));
}

#[test]
fn attention_failed_operation_with_stale_recovery_blocks() {
    let mut record = record(OperationKind::RepairApply, OperationState::Failed);
    record.recovery.classification = RecoveryClassification::Stale;
    assert_eq!(
        operation_attention(&record).unwrap().severity,
        AttentionSeverity::Blocking
    );
}

#[test]
fn attention_operation_routes_are_typed_and_stale_publications_need_action() {
    for (kind, route) in [
        (OperationKind::RommPublish, AttentionDestination::Romm),
        (OperationKind::EsDePublish, AttentionDestination::EsDe),
        (
            OperationKind::LibraryViewPublish,
            AttentionDestination::LibraryOrganisation,
        ),
        (
            OperationKind::DuplicateQuarantine,
            AttentionDestination::Problems,
        ),
        (OperationKind::CheatApply, AttentionDestination::CheatsMods),
        (OperationKind::ModApply, AttentionDestination::CheatsMods),
        (OperationKind::DatRename, AttentionDestination::DatReview),
    ] {
        let item = operation_attention(&record(kind, OperationState::Stale)).unwrap();
        assert_eq!(item.destination, route);
        assert_eq!(item.severity, AttentionSeverity::ActionNeeded);
    }
}

#[test]
fn attention_registry_deduplicates_and_resolves_from_newer_receipt() {
    let failed = record(OperationKind::RepairApply, OperationState::Partial);
    let mut snapshot = AttentionSnapshot::default();
    snapshot.insert(operation_attention(&failed).unwrap());
    snapshot.insert(operation_attention(&failed).unwrap());
    assert_eq!(snapshot.page(&AttentionFilters::default()).total, 1);
    let mut recovered = failed.clone();
    recovered.state = OperationState::RolledBack;
    recovered.completed_at_unix = Some(102);
    snapshot.insert(operation_attention(&recovered).unwrap());
    snapshot.insert(operation_attention(&failed).unwrap());
    assert_eq!(snapshot.page(&AttentionFilters::default()).total, 0);
    assert_eq!(snapshot.counts(), [0; 4]);
    assert_eq!(
        snapshot
            .page(&AttentionFilters {
                state: AttentionStateFilter::Resolved,
                ..Default::default()
            })
            .total,
        1
    );
}

#[test]
fn attention_normal_planned_and_running_operations_are_not_problems() {
    for state in [
        OperationState::Planned,
        OperationState::Ready,
        OperationState::Running,
    ] {
        assert!(operation_attention(&record(OperationKind::RepairApply, state)).is_none());
    }
}

#[test]
fn attention_filters_sort_and_paginate_without_unbounded_objects() {
    let mut snapshot = AttentionSnapshot::default();
    for index in 0..120 {
        let mut item = AttentionItem::new(
            format!("item:{index}"),
            AttentionCategory::Dat,
            AttentionSeverity::Warning,
            "DAT evidence".into(),
            AttentionDestination::DatReview,
        );
        item.last_observed = Some(index);
        item.platform = Some(if index < 60 { "ps2" } else { "nes" }.into());
        snapshot.insert(item);
    }
    let mut filters = AttentionFilters {
        newest_first: true,
        platform: Some("ps2".into()),
        ..Default::default()
    };
    let first = snapshot.page(&filters);
    assert_eq!(first.total, 60);
    assert_eq!(first.items.len(), ATTENTION_PAGE_SIZE);
    assert_eq!(first.items[0].last_observed, Some(59));
    filters.page = usize::MAX;
    assert_eq!(snapshot.page(&filters).items.len(), 10);
    filters.workflow = Some("not present".into());
    assert_eq!(snapshot.page(&filters).total, 0);
    snapshot.insert(
        operation_attention(&record(
            OperationKind::DatabaseRecovery,
            OperationState::Failed,
        ))
        .unwrap(),
    );
    assert_eq!(
        snapshot.page(&AttentionFilters::default()).items[0].severity,
        AttentionSeverity::Blocking
    );
}

#[test]
fn attention_summary_limit_is_explicit_and_preserves_blockers() {
    let mut snapshot = AttentionSnapshot::default();
    for index in 0..100_000 {
        snapshot.insert(AttentionItem::new(
            index.to_string(),
            AttentionCategory::Unsupported,
            AttentionSeverity::Warning,
            "Unsupported".into(),
            AttentionDestination::Discovery,
        ));
    }
    snapshot.insert(
        operation_attention(&record(
            OperationKind::DatabaseRecovery,
            OperationState::Failed,
        ))
        .unwrap(),
    );
    assert_eq!(snapshot.items().count(), ATTENTION_GROUP_LIMIT);
    assert!(snapshot.limited);
    assert_eq!(snapshot.counts()[0], 1);
    assert_eq!(
        AttentionSnapshot::default()
            .page(&AttentionFilters::default())
            .total,
        0
    );
}
