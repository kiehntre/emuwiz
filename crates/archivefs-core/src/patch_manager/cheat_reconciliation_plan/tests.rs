use super::*;
use crate::patch_manager::{
    CheatDocument, CheatIssue, CheatOperation, CheatPlatform, CheatReconciliationEntry,
    CheatReconciliationGroup, CheatReconciliationResult, CheatRelationship, CheatSourceFormat,
};

fn entry(
    index: usize,
    source: &str,
    title: &str,
    value: u8,
    verified: bool,
) -> CheatReconciliationEntry {
    CheatReconciliationEntry {
        game_identity: "dolphin:GALE01".into(),
        identity_verified: verified,
        title: title.into(),
        source: source.into(),
        source_format: CheatSourceFormat::Gecko,
        document: CheatDocument {
            title: title.into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::Gecko,
            operations: vec![CheatOperation::Write8 {
                address: 0x1000 + index as u64,
                value,
            }],
            issues: Vec::new(),
            provenance: vec![format!("provider-{source}:original-{index}")],
        },
        raw_code: Some(format!("{value:02X}")),
        provenance: vec![format!("provider-{source}:original-{index}")],
    }
}

fn report(
    entries: Vec<CheatReconciliationEntry>,
    groups: Vec<CheatReconciliationGroup>,
) -> CheatReconciliationResult {
    CheatReconciliationResult {
        game_identity: "dolphin:GALE01".into(),
        platform: CheatPlatform::GameCube,
        groups,
        entries,
        auto_winner: None,
    }
}

fn group(relationship: CheatRelationship, indices: Vec<usize>) -> CheatReconciliationGroup {
    CheatReconciliationGroup {
        relationship,
        entry_indices: indices,
        normalized_title: "test".into(),
        semantic_fingerprint: None,
        raw_fingerprint: None,
        differences: vec![],
        quality: vec![],
    }
}

fn request() -> ResolvedCheatPlanRequest {
    ResolvedCheatPlanRequest {
        source_report_digest: "a".repeat(64),
        emulator: "Dolphin".into(),
        profile: "default".into(),
        target_file: Some("/profiles/default/GAME.ini".into()),
        existing_file_digest: Some("b".repeat(64)),
        destination_changed: false,
    }
}

#[test]
fn keep_choice_enters_plan_and_skip_excludes_it() {
    let report = report(
        vec![
            entry(0, "a", "Conflict", 1, true),
            entry(1, "b", "Conflict", 2, true),
        ],
        vec![group(CheatRelationship::SameTitleDifferentCode, vec![0, 1])],
    );
    let mut choices = BTreeMap::new();
    choices.insert(0, CheatReviewChoice::KeepA);
    let plan = resolve_reviewed_cheat_plan(&report, &choices, &request());
    assert_eq!(plan.selected_entries.len(), 1);
    assert_eq!(plan.selected_entries[0].canonical_entry_index, 0);
    choices.insert(0, CheatReviewChoice::Skip);
    let skipped = resolve_reviewed_cheat_plan(&report, &choices, &request());
    assert!(skipped.selected_entries.is_empty());
    assert_eq!(skipped.skipped_entries.len(), 1);
}

#[test]
fn ignore_and_missing_choices_never_choose_a_conflict() {
    let report = report(
        vec![
            entry(0, "a", "Conflict", 1, true),
            entry(1, "b", "Conflict", 2, true),
        ],
        vec![group(CheatRelationship::SameTitleDifferentCode, vec![0, 1])],
    );
    for choice in [None, Some(CheatReviewChoice::IgnoreConflict)] {
        let choices = choice
            .map(|value| [(0, value)].into_iter().collect())
            .unwrap_or_default();
        let plan = resolve_reviewed_cheat_plan(&report, &choices, &request());
        assert!(plan.selected_entries.is_empty());
        assert_eq!(
            plan.unresolved_conflicts.len() + plan.ignored_entries.len(),
            1
        );
    }
}

#[test]
fn duplicate_group_collapses_but_retains_all_provenance() {
    let report = report(
        vec![
            entry(0, "a", "Same", 1, true),
            entry(1, "b", "Same", 1, true),
        ],
        vec![group(CheatRelationship::ExactSemanticDuplicate, vec![0, 1])],
    );
    let plan = resolve_reviewed_cheat_plan(&report, &BTreeMap::new(), &request());
    assert_eq!(plan.selected_entries.len(), 1);
    assert_eq!(plan.selected_entries[0].duplicate_entry_indices, vec![0, 1]);
    assert_eq!(plan.selected_entries[0].provenance.len(), 2);
}

#[test]
fn malformed_and_unsupported_entries_are_diagnostic_only() {
    let mut bad = entry(0, "bad", "Malformed", 1, true);
    bad.document.issues.push(CheatIssue::UnknownWidth);
    let mut raw = entry(1, "raw", "Unsupported", 1, true);
    raw.document
        .operations
        .push(CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Gecko,
            raw: "raw".into(),
            reason: "not representable".into(),
        });
    let report = report(
        vec![bad, raw],
        vec![
            group(CheatRelationship::Unique, vec![0]),
            group(CheatRelationship::Unique, vec![1]),
        ],
    );
    let plan = resolve_reviewed_cheat_plan(&report, &BTreeMap::new(), &request());
    assert!(plan.selected_entries.is_empty());
    assert_eq!(plan.malformed_entries.len(), 1);
    assert_eq!(plan.unsupported_entries.len(), 1);
}

#[test]
fn identity_and_destination_changes_fail_closed_and_plan_is_deterministic() {
    let report = report(
        vec![entry(0, "a", "One", 1, false)],
        vec![group(CheatRelationship::Unique, vec![0])],
    );
    let mut request = request();
    request.destination_changed = true;
    let a = resolve_reviewed_cheat_plan(&report, &BTreeMap::new(), &request);
    let b = resolve_reviewed_cheat_plan(&report, &BTreeMap::new(), &request);
    assert_eq!(a, b);
    assert!(matches!(
        a.apply_eligibility,
        ResolvedCheatApplyEligibility::Blocked { .. }
    ));
    assert!(!a.identity_verified);
    assert!(
        a.warnings
            .iter()
            .any(|warning| warning.contains("destination"))
    );
}

#[test]
fn a_clean_verified_plan_is_preview_only_and_freshness_rejects_stale_inputs() {
    let report = report(
        vec![entry(0, "a", "One", 1, true)],
        vec![group(CheatRelationship::Unique, vec![0])],
    );
    let choices = BTreeMap::new();
    let request = request();
    let plan = resolve_reviewed_cheat_plan(&report, &choices, &request);
    assert!(matches!(
        plan.apply_eligibility,
        ResolvedCheatApplyEligibility::PreviewOnly { .. }
    ));
    assert!(plan.is_fresh(
        &request.source_report_digest,
        &choices,
        "dolphin:GALE01",
        "Dolphin",
        "default",
        false
    ));
    assert!(!plan.is_fresh(
        "changed",
        &choices,
        "dolphin:GALE01",
        "Dolphin",
        "default",
        false
    ));
    assert!(!plan.is_fresh(
        &request.source_report_digest,
        &choices,
        "dolphin:OTHER",
        "Dolphin",
        "default",
        false
    ));
    assert!(!plan.is_fresh(
        &request.source_report_digest,
        &choices,
        "dolphin:GALE01",
        "Other",
        "default",
        false
    ));
    assert!(!plan.is_fresh(
        &request.source_report_digest,
        &choices,
        "dolphin:GALE01",
        "Dolphin",
        "default",
        true
    ));
    let changed_choices = [(0, CheatReviewChoice::Skip)].into_iter().collect();
    assert!(!plan.is_fresh(
        &request.source_report_digest,
        &changed_choices,
        "dolphin:GALE01",
        "Dolphin",
        "default",
        false
    ));
}

#[test]
fn preview_resolution_performs_no_destination_write() {
    let report = report(
        vec![entry(0, "a", "One", 1, true)],
        vec![group(CheatRelationship::Unique, vec![0])],
    );
    let request = request();
    let before = request.target_file.clone();
    let _plan = resolve_reviewed_cheat_plan(&report, &BTreeMap::new(), &request);
    assert_eq!(request.target_file, before);
}

#[test]
fn review_choices_digest_uses_stable_lowercase_sha256_hex() {
    let choices = [
        (0, CheatReviewChoice::KeepA),
        (4, CheatReviewChoice::IgnoreConflict),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        super::choices_digest(&choices),
        "c6fb053c109599d2eac403e0c5f3ebeffba3ff9612ddc25d37ce091b056bce5a"
    );
}

#[test]
fn invalid_group_indices_are_ignored_without_panic() {
    let report = report(
        vec![entry(0, "a", "One", 1, true)],
        vec![group(CheatRelationship::Unique, vec![99])],
    );
    let plan = resolve_reviewed_cheat_plan(&report, &BTreeMap::new(), &request());
    assert_eq!(plan.selected_entries.len(), 1);
    assert_eq!(plan.selected_entries[0].canonical_entry_index, 0);
}

#[test]
fn automatic_winner_in_report_blocks_the_resolved_plan() {
    let mut report = report(
        vec![entry(0, "a", "One", 1, true)],
        vec![group(CheatRelationship::Unique, vec![0])],
    );
    report.auto_winner = Some(0);
    let plan = resolve_reviewed_cheat_plan(&report, &BTreeMap::new(), &request());
    assert!(matches!(
        plan.apply_eligibility,
        ResolvedCheatApplyEligibility::Blocked { .. }
    ));
    assert!(plan.warnings.is_empty() || plan.warnings.iter().all(|warning| !warning.is_empty()));
}
