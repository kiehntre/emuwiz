use super::*;
use crate::emulator_environment::HostReadOnlyFilesystem;
use crate::patch_manager::*;
use crate::platform_evidence_fusion::evidence_lineage::ClaimStrength;
use std::collections::BTreeMap;
use std::fs;

fn cht_fixture(kind: CheatSourceKind) -> (tempfile::TempDir, CheatCatalogueSnapshot) {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Game (USA) (Rev 1).cht"),
        "cheats = 1\ncheat7_desc = \" Infinite Lives \"\ncheat7_code = \" abCD + 0010 \"\n",
    )
    .unwrap();
    let snapshot = RetroarchChtDirectorySource::new("fixture-provider")
        .with_source_kind(kind)
        .load(&HostReadOnlyFilesystem, root.path());
    (root, snapshot)
}
fn entry(label: &str, value: u8, evidence: CheatRecordProvenance) -> CheatReconciliationEntry {
    CheatReconciliationEntry {
        game_identity: "dolphin:GALE01".into(),
        identity_verified: true,
        applicability: Default::default(),
        source_path: None,
        source_index: None,
        source_fields: vec![],
        title: label.into(),
        source: label.into(),
        source_format: CheatSourceFormat::Gecko,
        document: CheatDocument {
            title: label.into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::Gecko,
            operations: vec![CheatOperation::Write8 {
                address: 0x1000,
                value,
            }],
            issues: vec![],
            provenance: vec![],
            source_evidence: vec![evidence],
        },
        raw_code: Some(format!("{value:02x}")),
        provenance: vec![],
    }
}
fn local(id: &str) -> CheatRecordProvenance {
    let mut e = CheatRecordProvenance::local(Path::new(id), id.as_bytes(), "fixture");
    e.original_description = Some("Infinite Lives".into());
    e.original_code = Some("original code".into());
    e.record_index = Some(7);
    e
}
fn ready(entries: Vec<CheatReconciliationEntry>) -> CheatReconciliationResult {
    match reconcile_cheats_for_game(entries) {
        CheatReconciliationOutcome::Ready(report) => report,
        other => panic!("{other:?}"),
    }
}
fn plan(report: &CheatReconciliationResult, choice: CheatReviewChoice) -> ResolvedCheatPlan {
    ResolvedCheatPlan::build(
        report,
        &BTreeMap::from([(0, choice)]),
        &ResolvedCheatPlanRequest {
            source_report_digest: "a".repeat(64),
            emulator: "Dolphin".into(),
            profile: "default".into(),
            target_file: None,
            existing_file_digest: None,
            destination_changed: false,
        },
    )
}
#[test]
fn local_file_provenance_is_retained() {
    let (_root, snapshot) = cht_fixture(CheatSourceKind::LocalFile);
    let e = &snapshot.games[0].cheats[0].source_evidence[0];
    assert_eq!(e.source_kind, CheatSourceKind::LocalFile);
    assert_eq!(e.source_quality, CheatSourceQuality::LocalKnown);
    assert_eq!(e.provider_name.as_deref(), Some("fixture-provider"));
}
#[test]
fn retroarch_pack_has_explicit_origin_and_format() {
    let (_root, snapshot) = cht_fixture(CheatSourceKind::RetroArchPack);
    let e = &snapshot.games[0].cheats[0].source_evidence[0];
    assert_eq!(e.source_kind, CheatSourceKind::RetroArchPack);
    assert_eq!(e.source_format.as_deref(), Some("retroarch_cht"));
    assert_eq!(e.source_quality, CheatSourceQuality::ImportedUnverified);
}
#[test]
fn bundled_origin_does_not_invent_verification() {
    let (_root, snapshot) = cht_fixture(CheatSourceKind::Bundled);
    let e = &snapshot.games[0].cheats[0].source_evidence[0];
    assert_eq!(e.source_kind, CheatSourceKind::Bundled);
    assert_ne!(e.source_quality, CheatSourceQuality::BundledVerifiedSource);
}
#[test]
fn source_filename_path_and_hash_are_retained_internally() {
    let (root, snapshot) = cht_fixture(CheatSourceKind::LocalFile);
    let e = &snapshot.games[0].cheats[0].source_evidence[0];
    assert_eq!(
        e.source_path,
        Some(root.path().join("Game (USA) (Rev 1).cht"))
    );
    assert_eq!(
        e.artifact.as_ref().unwrap().artifact_sha256,
        snapshot.games[0].source_file_hash
    );
    assert_eq!(e.display_filename(), Some("Game (USA) (Rev 1).cht"));
    assert!(!e.display_filename().unwrap().contains('/'));
}
#[test]
fn source_declared_index_survives_noncontiguous_import() {
    let (_root, snapshot) = cht_fixture(CheatSourceKind::LocalFile);
    assert_eq!(
        snapshot.games[0].cheats[0].source_evidence[0].record_index,
        Some(7)
    );
}
#[test]
fn original_description_is_verbatim() {
    let (_root, snapshot) = cht_fixture(CheatSourceKind::LocalFile);
    assert_eq!(
        snapshot.games[0].cheats[0].source_evidence[0]
            .original_description
            .as_deref(),
        Some("\" Infinite Lives \"")
    );
}
#[test]
fn original_code_is_verbatim() {
    let (_root, snapshot) = cht_fixture(CheatSourceKind::LocalFile);
    assert_eq!(
        snapshot.games[0].cheats[0].source_evidence[0]
            .original_code
            .as_deref(),
        Some("\" abCD + 0010 \"")
    );
}
#[test]
fn comparison_does_not_rewrite_source() {
    let mut e = CheatRecordProvenance::original(Some(" Lives ".into()), Some(" ab + CD ".into()));
    e.note_comparison(Some("lives".into()), Some("AB+CD".into()));
    assert_eq!(e.original_description.as_deref(), Some(" Lives "));
    assert_eq!(e.original_code.as_deref(), Some(" ab + CD "));
    assert_eq!(e.normalization, CheatNormalizationStatus::ComparisonOnly);
}
#[test]
fn identical_cheats_keep_two_sources() {
    let r = ready(vec![
        entry("Lives", 1, local("a")),
        entry("Lives", 1, local("b")),
    ]);
    assert_eq!(
        r.groups[0].relationship,
        CheatRelationship::ExactSemanticDuplicate
    );
    let evidence = r.groups[0].source_evidence(&r);
    assert_eq!(evidence.len(), 2);
    assert_eq!(cheat_evidence_source_count(&evidence), 2);
    assert_eq!(r.entries.len(), 2);
}
#[test]
fn three_sources_remain_three_observations() {
    let r = ready(vec![
        entry("Lives", 1, local("a")),
        entry("Lives", 1, local("b")),
        entry("Lives", 1, local("c")),
    ]);
    let evidence = r.groups[0].source_evidence(&r);
    assert_eq!(evidence.len(), 3);
    assert_eq!(cheat_evidence_source_count(&evidence), 3);
    assert!(
        evidence
            .iter()
            .all(|e| e.lineage == LineageRelation::Unknown)
    );
}
#[test]
fn mirrors_are_not_independent_corroboration() {
    let a = local("a");
    let mut b = a.clone();
    b.source_path = Some("b".into());
    assert_eq!(cheat_evidence_source_count(&[a, b]), 1);
    let mut a = local("a");
    let mut b = local("b");
    a.upstream_evidence_key = Some("original:7".into());
    b.upstream_evidence_key = a.upstream_evidence_key.clone();
    b.lineage = LineageRelation::Relay;
    assert_eq!(cheat_evidence_source_count(&[a, b]), 1);
}
#[test]
fn conflict_preserves_both_originals_and_classification() {
    let mut a = local("a");
    a.original_code = Some("code A".into());
    let mut b = local("b");
    b.original_code = Some("code B".into());
    let r = ready(vec![entry("Lives", 1, a), entry("Lives", 2, b)]);
    assert_eq!(
        r.groups[0].relationship,
        CheatRelationship::SameTitleDifferentCode
    );
    let evidence = r.groups[0].source_evidence(&r);
    assert_eq!(evidence[0].original_code.as_deref(), Some("code A"));
    assert_eq!(evidence[1].original_code.as_deref(), Some("code B"));
    let p = plan(&r, CheatReviewChoice::KeepBoth);
    assert_eq!(p.selected_entries.len(), 2);
}
#[test]
fn canonical_display_precedence_does_not_delete_evidence() {
    let r = ready(vec![
        entry("Lives", 1, local("b")),
        entry("Lives", 1, local("a")),
    ]);
    let p = plan(&r, CheatReviewChoice::KeepB);
    assert_eq!(p.selected_entries.len(), 1);
    // Canonical choice is independent of input order: the entry whose
    // provenance sorts first ("a", input position 1) is displayed.
    assert_eq!(p.selected_entries[0].canonical_entry_index, 1);
    assert_eq!(p.selected_entries[0].document.source_evidence.len(), 2);
}
#[test]
fn exact_identity_is_stronger_than_filename_association() {
    let mut e = local("a");
    e.applicability.push(CheatApplicabilityEvidence {
        kind: CheatApplicabilityKind::FilenameAssociation,
        value: "Game".into(),
        strength: ClaimStrength::Weak,
    });
    let e = entry("Lives", 1, e).audit_evidence().remove(0);
    assert_eq!(e.applicability[0].strength, ClaimStrength::Weak);
    assert!(
        e.applicability
            .iter()
            .any(|e| e.kind == CheatApplicabilityKind::VerifiedGameIdentity
                && e.strength == ClaimStrength::Strong)
    );
}
fn manifest() -> CheatCatalogueSnapshot {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manifest.json");
    fs::write(&path, r#"{"games":[{"game_name":"Game","region":"USA","revision":"Rev 2","serial":"ABC","content_hash":"1234","cheats":[{"description":"Lives","code":"raw-code"}]}]}"#).unwrap();
    JsonManifestSource::new("provider").load(&HostReadOnlyFilesystem, &path)
}
#[test]
fn region_declaration_is_retained_without_upgrade() {
    let snapshot = manifest();
    let e = &snapshot.games[0].cheats[0].source_evidence[0];
    assert!(
        e.applicability
            .iter()
            .any(|e| e.kind == CheatApplicabilityKind::Region
                && e.value == "USA"
                && e.strength == ClaimStrength::Weak)
    );
}
#[test]
fn revision_declaration_is_retained_without_upgrade() {
    let snapshot = manifest();
    let e = &snapshot.games[0].cheats[0].source_evidence[0];
    assert!(
        e.applicability
            .iter()
            .any(|e| e.kind == CheatApplicabilityKind::Revision
                && e.value == "Rev 2"
                && e.strength == ClaimStrength::Weak)
    );
}
#[test]
fn manifest_record_key_and_original_code_are_preserved() {
    let snapshot = manifest();
    let e = &snapshot.games[0].cheats[0].source_evidence[0];
    assert_eq!(e.record_key.as_deref(), Some("games/0/cheats/0"));
    assert_eq!(e.original_code.as_deref(), Some("raw-code"));
    assert_eq!(e.source_kind, CheatSourceKind::ImportedDatabase);
}
#[test]
fn unknown_source_is_explicit_and_partial_json_does_not_panic() {
    let e: CheatRecordProvenance = serde_json::from_str("{}").unwrap();
    assert_eq!(e, CheatRecordProvenance::default());
    assert_eq!(e.source_kind, CheatSourceKind::Unknown);
    assert_eq!(e.source_quality, CheatSourceQuality::Unknown);
    assert_eq!(cheat_evidence_source_count(&[e]), 0);
}
#[test]
fn manual_entry_is_distinct_and_not_promoted_by_equivalence() {
    let e = CheatRecordProvenance::manual("Lives", "01", "GALE01");
    let r = ready(vec![
        entry("Lives", 1, e.clone()),
        entry("Lives", 1, local("a")),
    ]);
    assert_eq!(r.entries[0].document.source_evidence[0], e);
    assert_eq!(
        r.entries[0].audit_evidence()[0].source_quality,
        CheatSourceQuality::UserAuthored
    );
    assert_eq!(e.original_code.as_deref(), Some("01"));
    assert_eq!(
        e.applicability[0].kind,
        CheatApplicabilityKind::ManualAssociation
    );
}
#[test]
fn audit_order_is_deterministic_without_dropping_duplicates() {
    let mut a = vec![local("b"), local("a"), local("a")];
    let mut b = a.iter().rev().cloned().collect::<Vec<_>>();
    order_cheat_provenance(&mut a);
    order_cheat_provenance(&mut b);
    assert_eq!(a, b);
    assert_eq!(a.len(), 3);
}
#[test]
fn report_and_resolved_domain_roundtrips_preserve_provenance() {
    let r = ready(vec![
        entry("Lives", 1, local("a")),
        entry("Lives", 1, local("b")),
    ]);
    let bytes = serde_json::to_vec(&r).unwrap();
    assert_eq!(
        serde_json::from_slice::<CheatReconciliationResult>(&bytes).unwrap(),
        r
    );
    let p = plan(&r, CheatReviewChoice::KeepA);
    assert_eq!(
        serde_json::from_slice::<ResolvedCheatPlan>(&serde_json::to_vec(&p).unwrap()).unwrap(),
        p
    );
}
#[test]
fn legacy_documents_deserialize_with_unknown_audit_evidence() {
    let mut e = entry("Lives", 1, local("a"));
    let mut json = serde_json::to_value(&e.document).unwrap();
    json.as_object_mut().unwrap().remove("source_evidence");
    e.document = serde_json::from_value(json).unwrap();
    assert_eq!(e.audit_evidence()[0].source_kind, CheatSourceKind::Unknown);
    assert_eq!(e.audit_evidence()[0].original_code, e.raw_code);
}
#[test]
fn malformed_input_does_not_invent_source_records() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("Bad.cht"), "garbage").unwrap();
    let snapshot =
        RetroarchChtDirectorySource::new("provider").load(&HostReadOnlyFilesystem, root.path());
    assert!(snapshot.games.is_empty());
    assert!(parse_cht_bytes(b"garbage").is_err());
}
#[test]
fn full_fidelity_parser_keeps_pre_normalization_values() {
    let doc =
        parse_cht_bytes(b"cheats=1\ncheat3_desc=\"Lives\"\ncheat3_code=\"ab\\\"cd\"\n").unwrap();
    assert_eq!(
        doc.entries[0].original_code.as_deref(),
        Some("\"ab\\\"cd\"")
    );
    // The hardened parser has no backslash escape language: it stays literal.
    assert_eq!(doc.entries[0].code.as_deref(), Some("ab\\\"cd"));
    assert!(!doc.entries[0].is_selectable());
}
#[test]
fn source_metadata_does_not_change_semantic_identity() {
    let a = entry("Lives", 1, local("a"));
    let mut b = a.clone();
    b.document.source_evidence = vec![local("b")];
    assert_ne!(a.document, b.document);
    assert_eq!(
        ready(vec![a, b]).groups[0].relationship,
        CheatRelationship::ExactSemanticDuplicate
    );
}
#[test]
fn conversion_preview_retains_original_evidence() {
    let d = entry("Lives", 1, local("a")).document;
    let p = assess_document_conversion(&d, CheatTargetFormat::Gecko);
    assert_eq!(p.source_evidence, d.source_evidence);
}
#[test]
fn user_supplied_file_is_local_not_automatically_user_authored() {
    let (root, _) = cht_fixture(CheatSourceKind::LocalFile);
    let report = scan_user_cheat_file(root.path().join("Game (USA) (Rev 1).cht"), &[]).unwrap();
    let e = &report.candidates[0].source_evidence[0];
    assert_eq!(e.source_kind, CheatSourceKind::LocalFile);
    assert_eq!(e.source_quality, CheatSourceQuality::LocalKnown);
    assert_eq!(e.record_index, Some(7));
    assert_eq!(e.original_code.as_deref(), Some("\" abCD + 0010 \""));
    assert_eq!(
        serde_json::from_slice::<UserCheatImportReport>(&serde_json::to_vec(&report).unwrap())
            .unwrap(),
        report
    );
}

#[test]
fn choosing_one_conflicting_source_retains_the_underlying_report() {
    let r = ready(vec![
        entry("Lives", 1, local("a")),
        entry("Lives", 2, local("b")),
    ]);
    let p = plan(&r, CheatReviewChoice::KeepA);
    assert_eq!(p.selected_entries.len(), 1);
    assert_eq!(p.source_report.as_deref(), Some(&r));
    assert_eq!(
        p.source_report.as_ref().unwrap().groups[0].relationship,
        CheatRelationship::SameTitleDifferentCode
    );
}

#[test]
fn missing_group_indexes_do_not_panic_or_fabricate_evidence() {
    let mut r = ready(vec![entry("Lives", 1, local("a"))]);
    r.groups[0].entry_indices = vec![usize::MAX];
    assert!(r.groups[0].source_evidence(&r).is_empty());
}

#[test]
fn n64_domain_conversion_preserves_region_and_revision_evidence() {
    let result = decode_n64_gameshark(
        "Lives",
        "80100000 0001",
        N64CheatRevisionEvidence::HeaderIdentity {
            game_code: "NSME".into(),
            revision: 2,
        },
        N64CheatRegion::Usa,
    );
    let d = result.to_document();
    let e = &d.source_evidence[0];
    assert_eq!(e.source_quality, CheatSourceQuality::Unknown);
    assert!(
        e.applicability
            .iter()
            .any(|a| a.kind == CheatApplicabilityKind::Revision
                && a.value == "2"
                && a.strength == ClaimStrength::Weak)
    );
    assert!(
        e.applicability
            .iter()
            .any(|a| a.kind == CheatApplicabilityKind::Region && a.value == "Usa")
    );
}

#[test]
fn n64_hash_identity_does_not_upgrade_unknown_source_quality() {
    let result = decode_n64_gameshark(
        "Lives",
        "80100000 0001",
        N64CheatRevisionEvidence::ExactRomHash {
            hash: "abcd".into(),
        },
        N64CheatRegion::Unknown,
    );
    let d = result.to_document();
    let e = &d.source_evidence[0];
    assert_eq!(e.source_quality, CheatSourceQuality::Unknown);
    assert!(
        e.applicability
            .iter()
            .any(|a| a.kind == CheatApplicabilityKind::ContentHashMatch
                && a.strength == ClaimStrength::Strong)
    );
}

#[test]
fn native_decoder_keeps_original_spacing_in_record_evidence() {
    let result = decode_n64_gameshark(
        "Lives",
        "  80100000 0001  ",
        N64CheatRevisionEvidence::Unknown,
        N64CheatRegion::Unknown,
    );
    assert_eq!(
        result.to_document().source_evidence[0]
            .original_code
            .as_deref(),
        Some("  80100000 0001  ")
    );
}

#[test]
fn original_text_alone_does_not_claim_source_provenance() {
    let mut e = entry(
        "Lives",
        1,
        CheatRecordProvenance::original(Some("Lives".into()), Some("01".into())),
    );
    e.source.clear();
    let r = ready(vec![e]);
    assert!(!r.groups[0].quality[0].provenance_present);
}

#[test]
fn provider_revision_and_record_key_survive_roundtrip() {
    let mut e = local("source.db");
    e.source_kind = CheatSourceKind::ImportedDatabase;
    e.source_quality = CheatSourceQuality::ImportedUnverified;
    e.provider_id = Some("future-provider".into());
    e.provider_name = Some("Curated archive".into());
    e.record_key = Some("game:42/cheat:17".into());
    e.artifact.as_mut().unwrap().upstream_version = Some("revision-123".into());
    let restored: CheatRecordProvenance =
        serde_json::from_slice(&serde_json::to_vec(&e).unwrap()).unwrap();
    assert_eq!(restored, e);
    assert_eq!(
        restored.artifact.unwrap().upstream_version.as_deref(),
        Some("revision-123")
    );
}
