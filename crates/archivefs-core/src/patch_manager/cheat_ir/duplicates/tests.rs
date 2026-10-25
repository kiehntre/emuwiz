use super::*;
use crate::patch_manager::{
    CheatReviewChoice, CheatSelection, ResolvedCheatApplyEligibility, ResolvedCheatPlanRequest,
    parse_cht_text, resolve_reviewed_cheat_plan,
};
fn entry(name: &str, source: &str, value: u8) -> CheatReconciliationEntry {
    CheatReconciliationEntry {
        game_identity: "verified:GALE01".into(),
        identity_verified: true,
        title: name.into(),
        source: source.into(),
        source_format: CheatSourceFormat::Gecko,
        applicability: Default::default(),
        source_path: None,
        source_index: None,
        source_fields: vec![],
        document: CheatDocument {
            source_evidence: Vec::new(),
            title: name.into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::Gecko,
            operations: vec![CheatOperation::Write8 {
                address: 100,
                value,
            }],
            issues: vec![],
            provenance: vec![format!("{source}:original")],
        },
        raw_code: None,
        provenance: vec![format!("{source}:entry")],
    }
}
fn ready(entries: Vec<CheatReconciliationEntry>) -> CheatReconciliationResult {
    match reconcile(entries) {
        CheatReconciliationOutcome::Ready(report) => report,
        other => panic!("{other:?}"),
    }
}
fn kinds(entries: Vec<CheatReconciliationEntry>) -> Vec<CheatDuplicateKind> {
    ready(entries).groups[0].classifications.clone()
}
fn request() -> ResolvedCheatPlanRequest {
    ResolvedCheatPlanRequest {
        source_report_digest: "digest".into(),
        emulator: "Dolphin".into(),
        profile: "test".into(),
        target_file: None,
        existing_file_digest: None,
        destination_changed: false,
    }
}
#[test]
fn unique() {
    assert_eq!(
        kinds(vec![entry("Lives", "local", 1)]),
        vec![CheatDuplicateKind::Unique]
    );
}
#[test]
fn identical_same_source() {
    let a = entry("Lives", "local", 1);
    let r = ready(vec![a.clone(), a]);
    assert_eq!(r.entries.len(), 2);
    assert_eq!(
        r.groups[0].classifications,
        vec![CheatDuplicateKind::ExactDuplicate]
    );
}
#[test]
fn identical_two_sources_and_duplicate_provenance() {
    let r = ready(vec![entry("Lives", "local", 1), entry("Lives", "pack", 1)]);
    assert_eq!(r.entries.len(), 2);
    assert_eq!(
        r.groups[0].classifications,
        vec![CheatDuplicateKind::ExactDuplicate]
    );
    let p = resolve_reviewed_cheat_plan(&r, &BTreeMap::new(), &request());
    assert_eq!(p.selected_entries.len(), 1);
    assert_eq!(p.selected_entries[0].duplicate_entry_indices.len(), 2);
    for s in ["local", "pack"] {
        assert!(
            p.selected_entries[0]
                .provenance
                .iter()
                .any(|v| v.contains(&format!("{s}:original")))
        );
    }
}
#[test]
fn same_name_same_code() {
    assert_eq!(
        kinds(vec![entry("Lives", "a", 1), entry("Lives", "b", 1)]),
        vec![CheatDuplicateKind::ExactDuplicate]
    );
}
#[test]
fn same_name_different_code() {
    assert_eq!(
        kinds(vec![entry("Lives", "a", 1), entry("Lives", "b", 2)]),
        vec![CheatDuplicateKind::NameConflict]
    );
}
#[test]
fn same_code_different_names() {
    let r = ready(vec![
        entry("Health", "a", 1),
        entry("Invincibility", "b", 1),
    ]);
    assert_eq!(
        r.groups[0].classifications,
        vec![CheatDuplicateKind::EquivalentDuplicate]
    );
    assert_eq!(r.entries[0].title, "Health");
    assert_eq!(r.entries[1].title, "Invincibility");
}
#[test]
fn source_index_conflicting_descriptions() {
    let d = parse_cht_text("cheat4_desc = Ammo\ncheat4_code = A\ncheat4_desc = Lives\n").unwrap();
    assert!(!d.entries[0].is_selectable());
    let r =
        ready(d.reconciliation_entries("game", true, CheatPlatform::NintendoDs, "local", "x.cht"));
    assert!(
        r.groups[0]
            .classifications
            .contains(&CheatDuplicateKind::SourceIndexConflict)
    );
    assert_eq!(r.entries[0].source_fields[0].value, "Ammo");
    assert_eq!(r.entries[0].source_fields[2].value, "Lives");
}
#[test]
fn source_index_conflicting_codes_survive_roundtrip_and_projection() {
    let d = parse_cht_text("cheat4_desc = Ammo\ncheat4_code = A\ncheat4_code = B\n").unwrap();
    assert!(!d.entries[0].is_selectable());
    let r =
        ready(d.reconciliation_entries("game", true, CheatPlatform::NintendoDs, "local", "x.cht"));
    let restored: CheatReconciliationResult =
        serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
    assert_eq!(r, restored);
    assert_eq!(
        restored.entries[0].source_fields[2].raw_source,
        "cheat4_code = B"
    );
    assert!(
        restored.groups[0]
            .classifications
            .contains(&CheatDuplicateKind::SourceIndexConflict)
    );
    assert!(
        resolve_reviewed_cheat_plan(&restored, &BTreeMap::new(), &request())
            .selected_entries
            .is_empty()
    );
}
fn regions(same_name: bool, same_code: bool) -> CheatReconciliationResult {
    let mut a = entry("Lives", "a", 1);
    let mut b = entry(
        if same_name { "Lives" } else { "Ammo" },
        "b",
        if same_code { 1 } else { 2 },
    );
    a.applicability.region = Some("NTSC-U".into());
    b.applicability.region = Some("PAL".into());
    ready(vec![a, b])
}
#[test]
fn same_code_different_regions() {
    assert_eq!(
        regions(false, true).groups[0].classifications,
        vec![CheatDuplicateKind::RegionVariant]
    );
}
#[test]
fn same_name_different_regions() {
    let r = regions(true, false);
    assert_eq!(
        r.groups[0].classifications,
        vec![CheatDuplicateKind::RegionVariant]
    );
    assert!(
        resolve_reviewed_cheat_plan(&r, &BTreeMap::new(), &request())
            .selected_entries
            .is_empty()
    );
}
#[test]
fn same_code_different_games() {
    let a = entry("Lives", "a", 1);
    let mut b = a.clone();
    b.game_identity = "unrelated".into();
    assert!(matches!(
        reconcile(vec![a, b]),
        CheatReconciliationOutcome::Unavailable { .. }
    ));
}
#[test]
fn same_name_code_different_revisions() {
    let mut a = entry("Lives", "a", 1);
    let mut b = a.clone();
    a.applicability.revision = Some("1.0".into());
    b.applicability.revision = Some("1.1".into());
    assert_eq!(kinds(vec![a, b]), vec![CheatDuplicateKind::VersionVariant]);
}
#[test]
fn different_verified_binaries() {
    let mut a = entry("Lives", "a", 1);
    let mut b = a.clone();
    a.applicability.verified_binary_identity = Some("sha256:a".into());
    b.applicability.verified_binary_identity = Some("sha256:b".into());
    assert_eq!(kinds(vec![a, b]), vec![CheatDuplicateKind::VersionVariant]);
}
#[test]
fn whitespace_only_description() {
    let r = ready(vec![
        entry("  Infinite  Lives ", "a", 1),
        entry("infinite lives", "b", 1),
    ]);
    assert_eq!(
        r.groups[0].classifications,
        vec![CheatDuplicateKind::ExactDuplicate]
    );
    assert_eq!(r.entries[0].title, "  Infinite  Lives ");
}
#[test]
fn harmless_proven_hex_formatting() {
    let mut a = entry("Lives", "a", 1);
    let mut b = entry("Lives", "b", 1);
    a.document.operations = vec![dolphin_line_to_ir(
        "042318ac   3b8003e7",
        CheatSourceFormat::Gecko,
    )];
    b.document.operations = vec![dolphin_line_to_ir(
        "042318AC 3B8003E7",
        CheatSourceFormat::Gecko,
    )];
    a.raw_code = Some("042318ac   3b8003e7".into());
    b.raw_code = Some("042318AC 3B8003E7".into());
    assert_eq!(kinds(vec![a, b]), vec![CheatDuplicateKind::ExactDuplicate]);
}
#[test]
fn meaningful_code_differences() {
    let a = entry("Lives", "a", 1);
    for op in [
        CheatOperation::Write8 {
            address: 100,
            value: 2,
        },
        CheatOperation::Write16 {
            address: 100,
            value: 1,
        },
        CheatOperation::OnFrameWrite8 {
            address: 100,
            value: 1,
        },
    ] {
        let mut b = a.clone();
        b.source = "b".into();
        b.document.operations = vec![op];
        assert_eq!(
            kinds(vec![a.clone(), b]),
            vec![CheatDuplicateKind::NameConflict]
        );
    }
}
#[test]
fn deterministic_order_across_input_permutations() {
    let e = vec![
        entry("Zelda", "b", 1),
        entry("Mario", "c", 2),
        entry("Zelda", "a", 1),
    ];
    let a = ready(e.clone());
    let b = ready(e.into_iter().rev().collect());
    let view = |r: &CheatReconciliationResult| {
        r.groups
            .iter()
            .map(|g| {
                (
                    g.classifications.clone(),
                    g.entry_indices
                        .iter()
                        .map(|i| r.entries[*i].clone())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(view(&a), view(&b));
}
#[test]
fn duplicate_subset_never_hides_conflict_or_auto_launches_both() {
    let r = ready(vec![
        entry("Lives", "a", 1),
        entry("Lives", "b", 1),
        entry("Lives", "c", 2),
    ]);
    assert_eq!(r.groups.len(), 1);
    assert!(
        r.groups[0]
            .classifications
            .contains(&CheatDuplicateKind::NameConflict)
    );
    let p = resolve_reviewed_cheat_plan(&r, &BTreeMap::new(), &request());
    assert!(p.selected_entries.is_empty());
    assert!(matches!(
        p.apply_eligibility,
        ResolvedCheatApplyEligibility::Blocked { .. }
    ));
}
#[test]
fn explicit_unambiguous_choice_respected() {
    let r = ready(vec![entry("Lives", "a", 1), entry("Lives", "b", 2)]);
    let p = resolve_reviewed_cheat_plan(
        &r,
        &[(0, CheatReviewChoice::KeepB)].into_iter().collect(),
        &request(),
    );
    assert_eq!(p.selected_entries.len(), 1);
    assert_eq!(p.selected_entries[0].canonical_entry_index, 1);
}
#[test]
fn heavily_duplicated_input() {
    let r = ready(vec![entry("Lives", "a", 1); 10_000]);
    assert_eq!(r.entries.len(), 10_000);
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].entry_indices.len(), 10_000);
}
#[test]
fn unknown_region_metadata_and_engine_require_review() {
    let a = entry("Lives", "a", 1);
    let mut b = a.clone();
    b.applicability.region = Some("PAL".into());
    assert_eq!(
        kinds(vec![a.clone(), b]),
        vec![CheatDuplicateKind::AmbiguousPossibleDuplicate]
    );
    let mut b = a.clone();
    b.applicability.metadata.insert("width".into(), "16".into());
    assert_eq!(
        kinds(vec![a.clone(), b]),
        vec![CheatDuplicateKind::AmbiguousPossibleDuplicate]
    );
    let mut b = a.clone();
    b.applicability.engine = Some("engine-a".into());
    let mut c = a;
    c.applicability.engine = Some("engine-b".into());
    assert_eq!(kinds(vec![b, c]), vec![CheatDuplicateKind::SyntaxVariant]);
}
#[test]
fn source_index_scope_includes_path() {
    let mut a = entry("Lives", "a", 1);
    a.source_index = Some(4);
    a.source_path = Some("a.cht".into());
    let mut b = a.clone();
    b.title = "Ammo".into();
    b.document.title = "Ammo".into();
    b.document.operations = vec![CheatOperation::Write8 {
        address: 200,
        value: 1,
    }];
    b.source_path = Some("b.cht".into());
    assert_eq!(ready(vec![a.clone(), b.clone()]).groups.len(), 2);
    b.source_path = a.source_path.clone();
    assert!(kinds(vec![a, b]).contains(&CheatDuplicateKind::SourceIndexConflict));
}
#[test]
fn identical_repeated_fields_preserved_and_nonblocking() {
    let d=parse_cht_text("cheat0_desc = Lives\ncheat0_code = A\ncheat0_code = A\ncheat0_enable = true\ncheat0_enable = true\ncheat0_handler = 0\ncheat0_handler = 0\n").unwrap();
    assert!(d.entries[0].is_selectable());
    assert_eq!(d.entries[0].source_fields.len(), 7);
}
#[test]
fn enable_and_extra_field_conflicts_block() {
    for fields in [
        "cheat0_enable = true\ncheat0_enable = false\n",
        "cheat0_handler = 0\ncheat0_handler = 1\n",
    ] {
        let d = parse_cht_text(&format!("cheat0_code = A\n{fields}")).unwrap();
        assert!(!d.entries[0].is_selectable());
    }
}
#[test]
fn retroarch_defaults_disable_conflicts_and_resolve_refuses_both() {
    let d=parse_cht_text("cheat0_desc = Lives\ncheat0_code = A\ncheat0_enable = true\ncheat1_desc = Lives\ncheat1_code = B\ncheat1_enable = true\n").unwrap();
    let mut s = CheatSelection::from_document(&d);
    assert!(d.entries.iter().all(|e| e.enabled_by_default));
    assert!(s.entries.iter().all(|e| !e.enabled));
    s.select_all();
    assert!(s.resolve(&d).is_ok());
    s.set_enabled(0, true);
    s.set_enabled(1, true);
    assert!(s.resolve(&d).is_err());
    s.set_enabled(1, false);
    assert!(s.resolve(&d).is_ok());
}

#[test]
fn global_conflicts_are_preserved_blocking_and_projected() {
    let d = parse_cht_text(
        "cheats = 1\ncheats = 2\ncheat_delay = 1\ncheat_delay = 2\ncheat0_code = A\n",
    )
    .unwrap();
    assert_eq!(d.declared_count, Some(1));
    assert_eq!(d.source_fields.len(), 4);
    // Hardened-parser policy: ignored file-wide metadata never rewrites or
    // blocks the selected entry; the conflict is review evidence instead.
    assert!(d.entries[0].is_selectable());
    let r =
        ready(d.reconciliation_entries("game", true, CheatPlatform::NintendoDs, "local", "x.cht"));
    assert!(
        r.groups[0]
            .classifications
            .contains(&CheatDuplicateKind::SourceMetadataConflict)
    );
    let p = resolve_reviewed_cheat_plan(&r, &BTreeMap::new(), &request());
    assert!(p.selected_entries.is_empty());
    assert!(
        p.unresolved_conflicts[0]
            .classifications
            .contains(&CheatDuplicateKind::SourceMetadataConflict)
    );
}
#[test]
fn opaque_syntax_and_internal_separators_are_not_assumed_equivalent() {
    let mut a = entry("Lives", "a", 1);
    let mut b = entry("Lives", "b", 1);
    for e in [&mut a, &mut b] {
        e.document.operations = vec![CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Gecko,
            raw: "ABCD-1234".into(),
            reason: "opaque".into(),
        }];
        e.raw_code = Some("ABCD-1234".into());
    }
    b.source_format = CheatSourceFormat::RetroArch;
    b.document.source_format = CheatSourceFormat::RetroArch;
    assert!(kinds(vec![a.clone(), b.clone()]).contains(&CheatDuplicateKind::SyntaxVariant));
    b.source_format = CheatSourceFormat::Gecko;
    b.document.source_format = CheatSourceFormat::Gecko;
    b.raw_code = Some("ABCD 1234".into());
    assert_eq!(kinds(vec![a, b]), vec![CheatDuplicateKind::NameConflict]);
}
#[test]
fn raw_fallback_does_not_drop_known_operations_in_mixed_documents() {
    let mut a = entry("Lives", "a", 1);
    let mut b = entry("Lives", "b", 2);
    for e in [&mut a, &mut b] {
        e.document.operations.push(CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Gecko,
            raw: "opaque".into(),
            reason: "unknown".into(),
        });
    }
    let r = ready(vec![a, b]);
    assert!(r.groups[0].raw_fingerprint.is_none());
    assert!(
        !r.groups[0]
            .classifications
            .contains(&CheatDuplicateKind::ExactDuplicate)
    );
}
#[test]
fn contradictory_decoded_operations_with_identical_raw_codes_are_code_conflicts() {
    let mut a = entry("Health", "a", 1);
    let mut b = entry("Ammo", "b", 2);
    a.raw_code = Some("same".into());
    b.raw_code = a.raw_code.clone();
    assert_eq!(kinds(vec![a, b]), vec![CheatDuplicateKind::CodeConflict]);
}
#[test]
fn logical_projection_preserves_original_aliases() {
    let r = ready(vec![
        entry("Health", "a", 1),
        entry("Invincibility", "b", 1),
    ]);
    let p = resolve_reviewed_cheat_plan(&r, &BTreeMap::new(), &request());
    assert_eq!(
        p.selected_entries[0].aliases,
        vec!["Health", "Invincibility"]
    );
}
#[test]
fn a_b_choices_cannot_silently_choose_from_three_conflicting_records() {
    let r = ready(vec![
        entry("Lives", "a", 1),
        entry("Lives", "b", 2),
        entry("Lives", "c", 3),
    ]);
    let p = resolve_reviewed_cheat_plan(
        &r,
        &[(0, CheatReviewChoice::KeepA)].into_iter().collect(),
        &request(),
    );
    assert!(p.selected_entries.is_empty());
}

#[test]
fn filtered_or_flattened_conflict_reports_do_not_autoselect_variants() {
    let r = ready(vec![entry("Lives", "a", 1), entry("Lives", "b", 2)]);
    for flatten in [false, true] {
        let mut report = r.clone();
        if flatten {
            report.groups[0].relationship = CheatRelationship::ExactSemanticDuplicate;
            report.groups[0].classifications.clear();
        } else {
            report.groups.clear();
        }
        let p = resolve_reviewed_cheat_plan(&report, &BTreeMap::new(), &request());
        assert!(p.selected_entries.is_empty());
        assert!(!p.unresolved_conflicts.is_empty());
    }
}

#[test]
fn mixed_decoded_disagreement_with_supplied_raw_is_ambiguous() {
    let mut a = entry("Lives", "a", 1);
    let mut b = entry("Lives", "b", 2);
    for e in [&mut a, &mut b] {
        e.raw_code = Some("complete source body".into());
        e.document.operations.push(CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Gecko,
            raw: "opaque tail".into(),
            reason: "unknown".into(),
        });
    }
    assert_eq!(
        kinds(vec![a, b]),
        vec![CheatDuplicateKind::AmbiguousPossibleDuplicate]
    );
}

#[test]
fn harmless_source_index_formatting_is_not_a_conflict() {
    let mut a = entry(" Infinite Lives ", "local", 1);
    a.source_index = Some(4);
    a.source_path = Some("x.cht".into());
    a.raw_code = Some(" A ".into());
    let mut b = a.clone();
    b.title = "infinite   lives".into();
    b.document.title = b.title.clone();
    b.raw_code = Some("A".into());
    assert_eq!(kinds(vec![a, b]), vec![CheatDuplicateKind::ExactDuplicate]);
    let d=parse_cht_text("cheat0_desc = \" Infinite Lives \"\ncheat0_desc = \"infinite   lives\"\ncheat0_code = \"A\"\ncheat0_code = \"A\"\ncheat0_enable = true\ncheat0_enable = TRUE\n").unwrap();
    assert!(d.entries[0].is_selectable());
    assert_eq!(
        d.entries[0].description.as_deref(),
        Some(" Infinite Lives ")
    );
    assert_eq!(d.entries[0].source_fields.len(), 6);
}

#[test]
fn malformed_or_truncated_code_evidence_cannot_prove_exact_duplicates() {
    let mut a = entry("Lives", "a", 1);
    let mut b = entry("Lives", "b", 1);
    for e in [&mut a, &mut b] {
        e.raw_code = Some("retained prefix".into());
        e.document.issues.push(CheatIssue::UnsupportedOperation(
            "oversized source field".into(),
        ));
    }
    let r = ready(vec![a, b]);
    assert_eq!(
        r.groups[0].classifications,
        vec![CheatDuplicateKind::AmbiguousPossibleDuplicate]
    );
    let p = resolve_reviewed_cheat_plan(&r, &BTreeMap::new(), &request());
    assert!(p.selected_entries.is_empty());
    assert_eq!(p.malformed_entries.len(), 2);
}

#[test]
fn unknown_code_padding_is_only_possible_duplicate_evidence() {
    let d = parse_cht_text(
        "cheat0_desc = Health\ncheat0_code = \"ABCD-1234\"\ncheat1_desc = Invincibility\ncheat1_code = \" ABCD-1234 \"\n",
    )
    .unwrap();
    let r =
        ready(d.reconciliation_entries("game", true, CheatPlatform::NintendoDs, "local", "x.cht"));
    assert_eq!(r.groups.len(), 1);
    assert_eq!(
        r.groups[0].classifications,
        vec![CheatDuplicateKind::AmbiguousPossibleDuplicate]
    );
    assert_eq!(r.entries[0].raw_code.as_deref(), Some("ABCD-1234"));
    assert_eq!(r.entries[1].raw_code.as_deref(), Some(" ABCD-1234 "));
    assert!(
        resolve_reviewed_cheat_plan(&r, &BTreeMap::new(), &request())
            .selected_entries
            .is_empty()
    );
}

#[test]
fn unknown_code_padding_cannot_enable_both_native_records() {
    let d = parse_cht_text(
        "cheat0_desc = Health\ncheat0_code = \"ABCD-1234\"\ncheat0_enable = true\ncheat1_desc = Invincibility\ncheat1_code = \" ABCD-1234 \"\ncheat1_enable = true\n",
    )
    .unwrap();
    assert!(d.entries.iter().all(|e| e.enabled_by_default));
    let mut selection = CheatSelection::from_document(&d);
    assert!(selection.entries.iter().all(|e| !e.enabled));
    selection.select_all();
    selection.set_enabled(0, true);
    selection.set_enabled(1, true);
    assert!(selection.resolve(&d).is_err());
    selection.set_enabled(1, false);
    assert!(selection.resolve(&d).is_ok());
}

#[test]
fn unknown_code_padding_at_same_source_index_is_a_conflict() {
    let d = parse_cht_text("cheat0_code = \"ABCD-1234\"\ncheat0_code = \" ABCD-1234 \"\n").unwrap();
    assert!(!d.entries[0].is_selectable());
    assert!(
        d.entries[0].warnings.iter().any(|warning| warning.kind
            == crate::patch_manager::ChtEntryWarningKind::ConflictingDuplicate)
    );
    assert_eq!(d.entries[0].source_fields.len(), 2);
}

#[test]
fn source_index_conflict_beyond_retained_field_bound_is_still_observable() {
    let prefix = "A".repeat(crate::patch_manager::MAX_CHT_FIELD_BYTES);
    let d = parse_cht_text(&format!(
        "cheat0_code = {prefix}\ncheat0_code = {prefix}B\n"
    ))
    .unwrap();
    assert!(!d.entries[0].is_selectable());
    assert!(
        d.entries[0].warnings.iter().any(|warning| warning.kind
            == crate::patch_manager::ChtEntryWarningKind::ConflictingDuplicate)
    );
    assert!(d.entries[0].source_fields[1].raw_source.ends_with('B'));
}

#[test]
fn global_source_evidence_is_retained_once_instead_of_copied_into_every_record() {
    let d = parse_cht_text("cheat_delay = 1\ncheat_delay = 2\ncheat0_code = A\ncheat1_code = B\n")
        .unwrap();
    let entries =
        d.reconciliation_entries("game", true, CheatPlatform::NintendoDs, "local", "x.cht");
    assert_eq!(
        entries
            .iter()
            .flat_map(|entry| &entry.source_fields)
            .filter(|field| field.field == "cheat_delay")
            .count(),
        2
    );
    // Hardened-parser policy: ignored file-wide metadata does not make a
    // selected entry uninstallable, but the conflict is document evidence and
    // every projected record carries it as a review-requiring issue.
    assert!(d.warnings.iter().any(|warning| warning.kind
        == crate::patch_manager::ChtDocumentWarningKind::ConflictingDuplicate));
    assert!(entries.iter().all(|entry| {
        entry
            .document
            .issues
            .contains(&crate::patch_manager::CheatIssue::SourceMetadataConflict)
    }));
}

#[test]
fn logical_plan_presentation_is_stable_across_source_insertion_order() {
    let entries = vec![entry("Zelda", "b", 1), entry("Mario", "a", 2)];
    let reports = [
        ready(entries.clone()),
        ready(entries.into_iter().rev().collect()),
    ];
    let views: Vec<_> = reports
        .iter()
        .map(|report| {
            resolve_reviewed_cheat_plan(report, &BTreeMap::new(), &request())
                .selected_entries
                .into_iter()
                .map(|entry| (entry.title, entry.document, entry.provenance))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(views[0], views[1]);
    assert_eq!(views[0][0].0, "Mario");
}
#[test]
fn long_global_keys_do_not_multiply_full_diagnostics_into_every_entry() {
    let key = "x".repeat(8_000);
    let d = parse_cht_text(&format!(
        "{key} = 1\n{key} = 2\ncheat0_code = A\ncheat1_code = B\n"
    ))
    .unwrap();
    assert!(
        d.entries
            .iter()
            .flat_map(|entry| &entry.warnings)
            .all(|warning| warning.detail.len() < 256)
    );
    assert_eq!(d.source_fields[0].field, key);
}

#[test]
fn informational_raw_preservation_does_not_turn_known_identical_codes_into_conflicts() {
    let mut a = entry("Lives", "a", 1);
    let mut b = entry("Lives", "b", 1);
    for e in [&mut a, &mut b] {
        e.document.issues.push(CheatIssue::RawPreserved);
    }
    let r = ready(vec![a, b]);
    assert_eq!(
        r.groups[0].classifications,
        vec![CheatDuplicateKind::ExactDuplicate]
    );
    assert!(r.groups[0].semantic_fingerprint.is_some());
    assert!(
        r.entries
            .iter()
            .all(|entry| entry.document.issues == vec![CheatIssue::RawPreserved])
    );
}

#[test]
fn metadata_bounds_cannot_hide_differences_and_prove_exact_duplicates() {
    let fields: String = (0..=crate::patch_manager::MAX_CHT_EXTRA_FIELDS_PER_ENTRY)
        .map(|i| format!("cheat0_custom{i} = A\n"))
        .collect();
    let d = parse_cht_text(&format!("cheat0_code = A\n{fields}")).unwrap();
    assert!(!d.entries[0].is_selectable());
    assert_eq!(
        d.entries[0].source_fields.len(),
        crate::patch_manager::MAX_CHT_EXTRA_FIELDS_PER_ENTRY + 2
    );
    let entries =
        d.reconciliation_entries("game", true, CheatPlatform::NintendoDs, "local", "x.cht");
    let r = ready(vec![entries[0].clone(), entries[0].clone()]);
    assert!(
        !r.groups[0]
            .classifications
            .contains(&CheatDuplicateKind::ExactDuplicate)
    );
}
#[test]
fn global_field_limit_blocks_lossy_projection_but_identical_repeats_do_not() {
    let fields: String = (0..crate::patch_manager::MAX_CHT_GLOBAL_FIELDS)
        .map(|i| format!("custom{i} = A\n"))
        .collect();
    let d = parse_cht_text(&format!("{fields}custom0 = A\ncheat0_code = A\n")).unwrap();
    assert!(d.entries[0].is_selectable());
    let d = parse_cht_text(&format!("{fields}another = B\ncheat0_code = A\n")).unwrap();
    assert!(
        d.warnings
            .iter()
            .any(|warning| warning.kind
                == crate::patch_manager::ChtDocumentWarningKind::LimitReached)
    );
    let projected =
        d.reconciliation_entries("game", true, CheatPlatform::NintendoDs, "local", "x.cht");
    assert!(projected[0].document.issues.iter().any(|issue| matches!(
        issue,
        crate::patch_manager::CheatIssue::UnsupportedOperation(_)
    )));
    assert!(
        d.source_fields
            .iter()
            .any(|field| field.field == "another" && field.value == "B")
    );
}
