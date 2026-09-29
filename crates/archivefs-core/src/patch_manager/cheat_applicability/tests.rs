use super::super::cht_document::parse_cht_text;
use super::*;
use crate::game_identity::{IdentityConfidence, IdentityProvenance};
use crate::patch_manager::{
    CheatOperation, CheatPlatform, CheatReconciliationEntry, CheatReconciliationOutcome,
    CheatRouteBasis, reconcile_cheats_for_game,
};

fn fact(kind: IdentityKind, value: &str, status: IdentityStatus) -> IdentityEvidence {
    IdentityEvidence {
        kind,
        value: Some(value.into()),
        status,
        confidence: IdentityConfidence::StructuredMetadata,
        provenance: IdentityProvenance {
            archive_path: "/tmp/emuwiz-applicability-game.iso".into(),
            member_path: None,
            member_index: None,
            method: "fixture".into(),
        },
        diagnostic: String::new(),
    }
}

fn verified(value: &str) -> CheatReleaseEvidence {
    CheatReleaseEvidence {
        value: value.into(),
        status: IdentityStatus::Verified,
    }
}

fn fixture() -> CheatApplicabilityInput {
    CheatApplicabilityInput {
        game: CheatSelectedGame {
            title: Some("Example Game".into()),
            filename: Some("Example".into()),
            platform: Some(verified("GameCube")),
            region: Some(verified("NTSC-U")),
            revision: Some(verified("1.0")),
            facts: vec![fact(
                IdentityKind::DolphinGameId,
                "GEXE01",
                IdentityStatus::Verified,
            )],
        },
        association: CheatGameAssociation {
            title: Some("Example Game".into()),
            filename: None,
            platform: Some("GameCube".into()),
            region: Some("NTSC-U".into()),
            revision: Some("1.0".into()),
            identities: vec![CheatIdentityRequirement {
                kind: IdentityKind::DolphinGameId,
                value: "GEXE01".into(),
            }],
            manually_associated: false,
        },
        document: CheatDocument {
            title: "Infinite lives".into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::Gecko,
            operations: vec![CheatOperation::Write32 {
                address: 0x1234,
                value: 9,
            }],
            issues: vec![],
            provenance: vec!["fixture:A".into()],
        },
        parsing: CheatParseEvidence::Valid,
        native_cht: None,
        route: Some(CheatRoute {
            platform_id: "GameCube".into(),
            target: CheatRouteTarget::standalone("dolphin"),
            basis: CheatRouteBasis::ExplicitSelection,
            apply_support: CheatApplySupport::Supported,
            native_format: "Dolphin GameSettings .ini",
            alternatives: vec![],
        }),
        reconciliation: None,
    }
}

fn title_only(input: &mut CheatApplicabilityInput) {
    input.association.identities.clear();
    input.association.platform = None;
    input.association.region = None;
    input.association.revision = None;
}

#[test]
fn exact_verified_game_match() {
    let report = assess_cheat_applicability(&fixture());
    assert_eq!(report.state, CheatApplicabilityState::Ready);
    assert_eq!(
        report.identity_match,
        CheatApplicabilityMatch::VerifiedIdentifier
    );
    assert!(report.blockers.is_empty());
}

#[test]
fn exact_hash_identity_is_stronger_than_identifier() {
    let mut input = fixture();
    input.game.facts.push(fact(
        IdentityKind::LooseRomSha256,
        "abc123",
        IdentityStatus::Verified,
    ));
    input.association.identities.push(CheatIdentityRequirement {
        kind: IdentityKind::LooseRomSha256,
        value: "abc123".into(),
    });
    input.association.revision = None;
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.identity_match, CheatApplicabilityMatch::ExactHash);
    assert_eq!(report.state, CheatApplicabilityState::Ready);
    assert!(
        report
            .warnings
            .contains(&CheatApplicabilityIssue::RevisionUnknown)
    );
    assert!(
        report
            .presentation()
            .summary
            .contains("game data matches exactly")
    );
}

#[test]
fn serial_match_does_not_prove_revision() {
    let mut input = fixture();
    input.game.facts = vec![fact(
        IdentityKind::Ps2Serial,
        "SLUS-12345",
        IdentityStatus::Verified,
    )];
    input.association.identities = vec![CheatIdentityRequirement {
        kind: IdentityKind::Ps2Serial,
        value: "SLUS-12345".into(),
    }];
    input.association.revision = None;
    let report = assess_cheat_applicability(&input);
    assert_eq!(
        report.identity_match,
        CheatApplicabilityMatch::VerifiedIdentifier
    );
    assert_eq!(report.state, CheatApplicabilityState::ExactGameMatch);
}

#[test]
fn title_only_match_is_possible() {
    let mut input = fixture();
    title_only(&mut input);
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::PossibleMatch
    );
}

#[test]
fn filename_only_association_never_becomes_exact() {
    let mut input = fixture();
    title_only(&mut input);
    input.association.title = None;
    input.association.filename = input.game.filename.clone();
    let report = assess_cheat_applicability(&input);
    assert_eq!(
        report.identity_match,
        CheatApplicabilityMatch::FilenameAssociation
    );
    assert_eq!(report.state, CheatApplicabilityState::PossibleMatch);
}

#[test]
fn exact_region_is_retained_as_evidence() {
    let report = assess_cheat_applicability(&fixture());
    assert!(
        report
            .evidence
            .iter()
            .any(|e| e.kind == CheatApplicabilityFindingKind::RegionMatch && e.detail == "NTSC-U")
    );
}

#[test]
fn wrong_region_blocks_matching_title_and_identity() {
    let mut input = fixture();
    input.association.region = Some("PAL".into());
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::WrongRegion);
    assert_eq!(
        report.presentation().summary,
        "This cheat is for the PAL release. Your selected game is NTSC-U."
    );
}

#[test]
fn exact_revision_is_retained_as_evidence() {
    assert!(
        assess_cheat_applicability(&fixture())
            .evidence
            .iter()
            .any(|e| e.kind == CheatApplicabilityFindingKind::RevisionMatch && e.detail == "1.0")
    );
}

#[test]
fn wrong_revision_blocks_readiness() {
    let mut input = fixture();
    input.game.revision = Some(verified("1.1"));
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::WrongRevision);
    assert_eq!(
        report.presentation().summary,
        "This cheat is for revision 1.0. Your selected game is revision 1.1."
    );
}

#[test]
fn unknown_revision_is_not_assumed_compatible() {
    let mut input = fixture();
    input.game.revision = None;
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::ExactGameMatch);
    assert!(
        report
            .warnings
            .contains(&CheatApplicabilityIssue::RevisionUnknown)
    );
    assert!(report.presentation().summary.contains("could not verify"));
}

#[test]
fn unsupported_format_is_not_supported_by_route_alone() {
    let mut input = fixture();
    input.document.source_format = CheatSourceFormat::Other("Game Genie".into());
    input.document.operations = vec![CheatOperation::UnsupportedRaw {
        source_format: input.document.source_format.clone(),
        raw: "SXIOPO".into(),
        reason: "No reviewed decoder for this target".into(),
    }];
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::UnsupportedFormat
    );
}

#[test]
fn unsupported_emulator_blocks_readiness() {
    let mut input = fixture();
    input.route.as_mut().unwrap().apply_support = CheatApplySupport::Unsupported;
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::UnsupportedEmulator
    );
}

#[test]
fn unknown_emulator_capability_requires_review() {
    let mut input = fixture();
    input.route = None;
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::NeedsReview);
    assert_eq!(report.support.emulator, CheatSupportState::Unknown);
}

fn entry(input: &CheatApplicabilityInput, source: &str) -> CheatReconciliationEntry {
    CheatReconciliationEntry {
        game_identity: "GEXE01".into(),
        identity_verified: true,
        title: input.document.title.clone(),
        source: source.into(),
        source_format: input.document.source_format.clone(),
        document: input.document.clone(),
        raw_code: None,
        provenance: vec![source.into()],
    }
}

fn reconcile(entries: Vec<CheatReconciliationEntry>) -> CheatReconciliationResult {
    let CheatReconciliationOutcome::Ready(result) = reconcile_cheats_for_game(entries) else {
        panic!("fixture identity is verified")
    };
    result
}

#[test]
fn conflicting_source_variants_preserve_every_code() {
    let mut input = fixture();
    let a = entry(&input, "A");
    let mut b = entry(&input, "B");
    b.document.operations = vec![CheatOperation::Write32 {
        address: 0x1234,
        value: 10,
    }];
    input.reconciliation = Some(reconcile(vec![a, b]));
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::ConflictingVariants);
    assert_eq!(report.presentation().label, "Needs review");
    assert_eq!(report.reconciliation.unwrap().entries.len(), 2);
}

#[test]
fn malformed_cheat_uses_parser_evidence() {
    let mut input = fixture();
    let parsed = parse_cht_text("cheats = 1\ncheat0_code = \"bad\"quote\"\n").unwrap();
    input.parsing = CheatParseEvidence::from_cht_entry(&parsed.entries[0]);
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::Malformed
    );
}

#[test]
fn missing_code_uses_parser_evidence() {
    let mut input = fixture();
    let parsed = parse_cht_text("cheats = 1\ncheat0_desc = \"Infinite lives\"\n").unwrap();
    input.parsing = CheatParseEvidence::from_cht_entry(&parsed.entries[0]);
    let report = assess_cheat_applicability(&input);
    assert_eq!(
        report.state,
        CheatApplicabilityState::MissingRequiredEvidence
    );
    assert_eq!(
        report.presentation().summary,
        "This cheat has no usable code."
    );
}

#[test]
fn duplicate_corroborated_cheat_is_ready_without_claiming_independence() {
    let mut input = fixture();
    input.reconciliation = Some(reconcile(vec![entry(&input, "A"), entry(&input, "B")]));
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::Ready);
    assert!(report.evidence.iter().any(|e| e.kind
        == CheatApplicabilityFindingKind::CorroboratedDuplicate
        && e.detail.contains("independence is not established")));
    assert!(report.reconciliation.unwrap().auto_winner.is_none());
}

#[test]
fn manual_user_association_is_possible_not_exact() {
    let mut input = fixture();
    title_only(&mut input);
    input.association.title = None;
    input.association.manually_associated = true;
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::PossibleMatch);
    assert_eq!(
        report.identity_match,
        CheatApplicabilityMatch::ManualAssociation
    );
    assert!(
        report
            .presentation()
            .summary
            .contains("has not verified compatibility")
    );
}

#[test]
fn unrelated_game_is_rejected_even_with_same_title_or_manual_association() {
    let mut input = fixture();
    input.association.identities[0].value = "GOTHER".into();
    input.association.manually_associated = true;
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::DifferentGame
    );
}

#[test]
fn deterministic_status_wording() {
    let input = fixture();
    let first = assess_cheat_applicability(&input);
    assert_eq!(
        first.presentation(),
        assess_cheat_applicability(&input).presentation()
    );
    assert_eq!(
        first.presentation(),
        CheatApplicabilityPresentation {
            label: "Ready to use",
            summary: "Exact match for this game, region, and revision.".into()
        }
    );
    let mut title = input;
    title_only(&mut title);
    assert_eq!(
        assess_cheat_applicability(&title).presentation().summary,
        "The title matches, but EmuWiz could not verify the exact game revision."
    );
}

#[test]
fn evidence_ordering_is_stable() {
    let mut input = fixture();
    input.game.facts.push(fact(
        IdentityKind::LooseRomSha256,
        "abc",
        IdentityStatus::Verified,
    ));
    input.association.identities.push(CheatIdentityRequirement {
        kind: IdentityKind::LooseRomSha256,
        value: "abc".into(),
    });
    let a = assess_cheat_applicability(&input);
    input.game.facts.reverse();
    input.association.identities.reverse();
    let b = assess_cheat_applicability(&input);
    assert_eq!(a.evidence, b.evidence);
    assert_eq!(a.blockers, b.blockers);
    assert_eq!(a.warnings, b.warnings);
    assert!(a.evidence.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn applicability_never_enables_or_selects_cheats() {
    let input = fixture();
    let original = input.document.clone();
    let parsed =
        parse_cht_text("cheats = 1\ncheat0_code = \"04001234 00000009\"\ncheat0_enable = false\n")
            .unwrap();
    let selection = crate::patch_manager::CheatSelection::from_document(&parsed);
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::Ready
    );
    assert_eq!(input.document, original);
    assert!(!parsed.entries[0].enabled_by_default);
    assert!(
        selection
            .entries
            .iter()
            .all(|entry| !entry.selected && !entry.enabled)
    );
}

#[test]
fn candidate_identifier_never_proves_exact_match() {
    let mut input = fixture();
    title_only(&mut input);
    input.association.identities = vec![CheatIdentityRequirement {
        kind: IdentityKind::DolphinGameId,
        value: "GEXE01".into(),
    }];
    input.game.facts[0].status = IdentityStatus::Candidate;
    assert_eq!(
        assess_cheat_applicability(&input).identity_match,
        CheatApplicabilityMatch::TitleOnly
    );
}

#[test]
fn platform_evidence_cannot_be_used_as_game_identifier() {
    let mut input = fixture();
    title_only(&mut input);
    input.association.title = None;
    input.game.facts = vec![fact(
        IdentityKind::Platform,
        "GameCube",
        IdentityStatus::Verified,
    )];
    input.association.identities = vec![CheatIdentityRequirement {
        kind: IdentityKind::Platform,
        value: "GameCube".into(),
    }];
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::MissingRequiredEvidence
    );
}

#[test]
fn conflicting_verified_identity_requires_review_regardless_of_order() {
    let mut input = fixture();
    input.game.facts.push(fact(
        IdentityKind::DolphinGameId,
        "GOTHER",
        IdentityStatus::Verified,
    ));
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::NeedsReview
    );
    input.game.facts.reverse();
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::NeedsReview
    );
}

#[test]
fn all_region_and_revision_blockers_survive_summary_priority() {
    let mut input = fixture();
    input.association.region = Some("PAL".into());
    input.association.revision = Some("1.1".into());
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::WrongRegion);
    assert!(
        report
            .blockers
            .contains(&CheatApplicabilityIssue::WrongRevision)
    );
}

#[test]
fn unreviewed_format_for_supported_emulator_is_unknown() {
    let mut input = fixture();
    input.route.as_mut().unwrap().target = CheatRouteTarget::standalone("ppsspp");
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::NeedsReview);
    assert_eq!(report.support.format, CheatSupportState::Unknown);
}

#[test]
fn unknown_retroarch_core_does_not_prove_support() {
    let mut input = fixture();
    input.route.as_mut().unwrap().target = CheatRouteTarget::retroarch(None);
    assert_eq!(
        assess_cheat_applicability(&input).support.emulator,
        CheatSupportState::Unknown
    );
}

#[test]
fn native_retroarch_format_is_supported_but_opaque_engine_is_unknown() {
    let mut input = fixture();
    input.document.source_format = CheatSourceFormat::RetroArch;
    input.document.operations.clear();
    input.native_cht = Some(
        parse_cht_text("cheats = 1\ncheat0_code = \"SXIOPO\"\n")
            .unwrap()
            .entries
            .remove(0),
    );
    input.route.as_mut().unwrap().target = CheatRouteTarget::retroarch(Some("example_core"));
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.support.format, CheatSupportState::Supported);
    assert_eq!(report.support.engine, CheatSupportState::Unknown);
    assert_eq!(report.state, CheatApplicabilityState::NeedsReview);
    assert!(
        report
            .blockers
            .contains(&CheatApplicabilityIssue::EngineCapabilityUnknown)
    );
    assert_eq!(report.native_cht.unwrap().code.as_deref(), Some("SXIOPO"));
}

#[test]
fn native_parser_evidence_cannot_be_overridden_by_valid_claim() {
    let mut input = fixture();
    input.native_cht = Some(
        parse_cht_text("cheats = 1\ncheat0_desc = \"Missing code\"\n")
            .unwrap()
            .entries
            .remove(0),
    );
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::MissingRequiredEvidence
    );
}

#[test]
fn declared_hash_requires_verification_even_when_title_and_release_match() {
    let mut input = fixture();
    input.association.identities.push(CheatIdentityRequirement {
        kind: IdentityKind::LooseRomSha256,
        value: "required_hash".into(),
    });
    let report = assess_cheat_applicability(&input);
    assert_eq!(
        report.state,
        CheatApplicabilityState::MissingRequiredEvidence
    );
    assert!(
        report
            .blockers
            .contains(&CheatApplicabilityIssue::RequiredIdentityUnknown)
    );
}

#[test]
fn executable_crc_does_not_claim_whole_game_hash_identity() {
    let mut input = fixture();
    input.game.facts = vec![fact(
        IdentityKind::Pcsx2ExecutableCrc,
        "AABBCCDD",
        IdentityStatus::Verified,
    )];
    input.association.identities = vec![CheatIdentityRequirement {
        kind: IdentityKind::Pcsx2ExecutableCrc,
        value: "AABBCCDD".into(),
    }];
    input.association.revision = None;
    let report = assess_cheat_applicability(&input);
    assert_eq!(
        report.identity_match,
        CheatApplicabilityMatch::VerifiedIdentifier
    );
    assert_eq!(report.state, CheatApplicabilityState::ExactGameMatch);
}

#[test]
fn title_platform_and_unknown_release_are_a_strong_match() {
    let mut input = fixture();
    input.association.identities.clear();
    input.association.revision = None;
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::StrongMatch
    );
}

#[test]
fn candidate_region_and_revision_are_unknown_not_exact() {
    let mut input = fixture();
    input.game.region.as_mut().unwrap().status = IdentityStatus::Candidate;
    input.game.revision.as_mut().unwrap().status = IdentityStatus::Candidate;
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::ExactGameMatch);
    assert!(
        report
            .warnings
            .contains(&CheatApplicabilityIssue::RegionUnknown)
    );
    assert!(
        report
            .warnings
            .contains(&CheatApplicabilityIssue::RevisionUnknown)
    );
}

#[test]
fn platform_aliases_reuse_canonical_registry() {
    let mut input = fixture();
    input.association.platform = Some("Nintendo GameCube".into());
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::Ready
    );
}

#[test]
fn source_document_platform_mismatch_is_always_blocking() {
    let mut input = fixture();
    input.document.platform = CheatPlatform::Ps2;
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::DifferentGame
    );
}

#[test]
fn hash_identity_never_overrides_known_wrong_region() {
    let mut input = fixture();
    input.game.facts.push(fact(
        IdentityKind::LooseRomSha256,
        "abc",
        IdentityStatus::Verified,
    ));
    input.association.identities.push(CheatIdentityRequirement {
        kind: IdentityKind::LooseRomSha256,
        value: "abc".into(),
    });
    input.association.region = Some("PAL".into());
    assert_eq!(
        assess_cheat_applicability(&input).state,
        CheatApplicabilityState::WrongRegion
    );
}

#[test]
fn report_adapter_preserves_inspector_candidates_and_decodes_dolphin_region() {
    use crate::game_identity::{IdentityImageFormat, IdentityPlatform};
    let report = GameIdentityReport {
        archive_path: "/tmp/emuwiz-misleading-PAL-rev-7.iso".into(),
        platform: IdentityPlatform::GameCube,
        format: IdentityImageFormat::Iso,
        evidence: vec![
            fact(IdentityKind::Platform, "GameCube", IdentityStatus::Verified),
            fact(
                IdentityKind::DolphinGameId,
                "GEXE01",
                IdentityStatus::Verified,
            ),
            fact(IdentityKind::DolphinRegion, "E", IdentityStatus::Verified),
            fact(
                IdentityKind::DolphinRevision,
                "0",
                IdentityStatus::Candidate,
            ),
        ],
        warnings: vec![],
        bytes_read: 0,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    let selected = CheatSelectedGame::from_identity_report(&report);
    assert_eq!(selected.region, Some(verified("USA")));
    assert!(selected.revision.is_none());
    assert_eq!(selected.facts, report.evidence);
    assert!(selected.filename.unwrap().contains("PAL-rev-7"));
}

#[test]
fn title_with_matching_region_and_revision_never_claims_exact_game() {
    let mut input = fixture();
    input.association.identities.clear();
    let report = assess_cheat_applicability(&input);
    assert_eq!(report.state, CheatApplicabilityState::StrongMatch);
    assert_eq!(report.identity_match, CheatApplicabilityMatch::Strong);
}
