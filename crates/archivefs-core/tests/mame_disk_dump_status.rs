//! Disk dump quality is a catalogue fact, not content or launch verification.
//! All XML and observations below are disposable synthetic evidence.
use archivefs_core::arcade_mame_compatibility::*;
use archivefs_core::dat::dependency::{DependencyKind, DependencyOutcome};
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::parsers::mame_listxml::MAME_LISTXML_PARSER_SCHEMA_VERSION;
use archivefs_core::identity_source::mame_listxml::import_mame_listxml;
use archivefs_core::mame_collection_analyser::*;
use archivefs_core::ready_to_play::ReadyToPlayState;

const ROM_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DISK_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const WRONG_SHA: &str = "cccccccccccccccccccccccccccccccccccccccc";
const ROM: &str =
    r#"<rom name="main" size="4" crc="aaaaaaaa" sha1="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"/>"#;
const LAYOUTS: [MameCollectionLayout; 4] = [
    MameCollectionLayout::RuntimeSearch,
    MameCollectionLayout::Split,
    MameCollectionLayout::Merged,
    MameCollectionLayout::NonMerged,
];

struct Fixture {
    _dir: tempfile::TempDir,
    dat: ParsedDat,
    pin: InstalledMameEvidence,
}

impl Fixture {
    fn new(machines: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retained-listxml.xml");
        std::fs::write(&path, format!(r#"<mame build="0.280">{machines}</mame>"#)).unwrap();
        let imported = import_mame_listxml(&path).unwrap();
        let pin = InstalledMameEvidence::from_imported(
            dir.path().join("mame"),
            "0.280",
            &path,
            [],
            &imported,
            Some(123),
        );
        Self {
            _dir: dir,
            dat: imported.dat,
            pin,
        }
    }

    fn disks(disks: &str) -> Self {
        Self::new(&format!(r#"<machine name="game">{ROM}{disks}</machine>"#))
    }

    fn expectation(&self, layout: MameCollectionLayout) -> MameSetExpectation {
        MameExpectationIndex::new(&self.dat)
            .expectation_for_layout("game", layout)
            .unwrap()
    }
}

fn observations(completeness: ObservedEvidenceCompleteness) -> ObservedArcadeSetEvidence {
    let mut e = ObservedArcadeSetEvidence::new("game", completeness);
    e.observed_set_names.push("game".into());
    e.observed_roms.push(MameObservedRom {
        set_name: "game".into(),
        name: "main".into(),
        size_bytes: Some(4),
        crc32: Some("aaaaaaaa".into()),
        sha1: Some(ROM_SHA.into()),
    });
    e
}

fn disk(set: &str, name: &str, sha: Option<&str>) -> MameObservedDisk {
    MameObservedDisk {
        set_name: set.into(),
        name: Some(name.into()),
        sha1: sha.map(str::to_owned),
    }
}

#[test]
fn required_nodump_disks_are_known_preservation_gaps_with_original_provenance() {
    for status in ["nodump", "NoDuMp"] {
        let f = Fixture::disks(&format!(r#"<disk name="undumped" status="{status}"/>"#));
        for layout in LAYOUTS {
            for completeness in [
                ObservedEvidenceCompleteness::Complete,
                ObservedEvidenceCompleteness::Partial,
                ObservedEvidenceCompleteness::Unknown,
                ObservedEvidenceCompleteness::NotGathered,
            ] {
                let expected = f.expectation(layout);
                assert_eq!(expected.disks[0].status.as_deref(), Some(status));
                let result = observations(completeness).audit(&f.pin, expected);
                assert_eq!(
                    result.state,
                    MameSetCompatibilityState::Unsupported,
                    "{result:?}"
                );
                assert!(result.only_known_preservation_gaps());
                assert_eq!(result.ready_to_play_state, Some(ReadyToPlayState::Unknown));
                assert_eq!(result.mismatches.len(), 1);
                let gap = &result.mismatches[0];
                assert_eq!(gap.reason, MameMismatchReason::NoDump);
                assert_eq!(gap.set_name, "game");
                assert_eq!(gap.member_name.as_deref(), Some("undumped"));
                assert!(gap.detail.contains("no known dump"));
                assert_eq!(result.mame, f.pin);
                assert_eq!(
                    result.mame.parser_schema_version,
                    MAME_LISTXML_PARSER_SCHEMA_VERSION
                );
                assert_eq!(
                    result.mame.metadata_build_identity.as_ref().unwrap().len(),
                    64
                );
            }
        }
    }
}

#[test]
fn matched_baddump_disks_have_quality_warnings_in_every_layout() {
    for status in ["baddump", "BaDdUmP"] {
        let f = Fixture::disks(&format!(
            r#"<disk name="disk" sha1="{DISK_SHA}" status="{status}"/>"#
        ));
        let mut e = observations(ObservedEvidenceCompleteness::Complete);
        e.observed_disks.push(disk("game", "disk", Some(DISK_SHA)));
        for layout in LAYOUTS {
            let result = e.audit(&f.pin, f.expectation(layout));
            assert_eq!(
                result.state,
                MameSetCompatibilityState::CompatibleWithWarnings
            );
            assert_eq!(
                result.ready_to_play_state,
                Some(ReadyToPlayState::ReadyWithWarnings)
            );
            assert!(!result.only_known_preservation_gaps());
            assert_eq!(result.mismatches.len(), 1);
            let warning = &result.mismatches[0];
            assert_eq!(warning.reason, MameMismatchReason::BadDump);
            assert_eq!(warning.member_name.as_deref(), Some("disk"));
            assert!(warning.detail.contains("imperfect"));
            assert_eq!(result.expectation.disks[0].status.as_deref(), Some(status));
        }
    }
}

#[test]
fn optional_nodump_and_baddump_disks_do_not_become_launch_requirements() {
    for status in ["nodump", "baddump"] {
        let f = Fixture::disks(&format!(
            r#"<disk name="extra" optional="yes" status="{status}" sha1="{DISK_SHA}"/>"#
        ));
        for layout in LAYOUTS {
            let expected = f.expectation(layout);
            assert!(expected.disks.is_empty());
            let result =
                observations(ObservedEvidenceCompleteness::Complete).audit(&f.pin, expected);
            assert_eq!(result.state, MameSetCompatibilityState::Compatible);
            assert!(result.mismatches.is_empty());
        }
    }
}

#[test]
fn nodump_gaps_never_soften_missing_or_wrong_required_disks() {
    let f = Fixture::disks(&format!(
        r#"<disk name="undumped" status="nodump"/><disk name="required" sha1="{DISK_SHA}"/>"#
    ));
    for layout in LAYOUTS {
        for (observed, reason) in [
            (None, MameMismatchReason::ChdMissing),
            (Some(WRONG_SHA), MameMismatchReason::ChdHashMismatch),
        ] {
            let mut e = observations(ObservedEvidenceCompleteness::Complete);
            if let Some(sha) = observed {
                e.observed_disks.push(disk("game", "required", Some(sha)));
            }
            let result = e.audit(&f.pin, f.expectation(layout));
            assert_eq!(result.state, MameSetCompatibilityState::Incompatible);
            assert_eq!(result.ready_to_play_state, Some(ReadyToPlayState::Blocked));
            assert!(!result.only_known_preservation_gaps());
            assert!(
                result
                    .mismatches
                    .iter()
                    .any(|m| m.reason == MameMismatchReason::NoDump)
            );
            assert!(result.mismatches.iter().any(|m| m.reason == reason));
        }
    }
}

#[test]
fn baddump_quality_does_not_replace_missing_or_contradictory_identity_evidence() {
    for (expected_sha, observed, state, reason) in [
        (
            Some(DISK_SHA),
            None,
            MameSetCompatibilityState::Incompatible,
            MameMismatchReason::ChdMissing,
        ),
        (
            Some(DISK_SHA),
            Some(None),
            MameSetCompatibilityState::Unknown,
            MameMismatchReason::EvidenceUnavailable,
        ),
        (
            None,
            Some(Some(DISK_SHA)),
            MameSetCompatibilityState::Unknown,
            MameMismatchReason::EvidenceUnavailable,
        ),
        (
            Some(DISK_SHA),
            Some(Some(WRONG_SHA)),
            MameSetCompatibilityState::Incompatible,
            MameMismatchReason::ChdHashMismatch,
        ),
    ] {
        let hash = expected_sha
            .map(|sha| format!(r#"sha1="{sha}""#))
            .unwrap_or_default();
        let f = Fixture::disks(&format!(r#"<disk name="disk" status="baddump" {hash}/>"#));
        for layout in LAYOUTS {
            let mut e = observations(ObservedEvidenceCompleteness::Complete);
            if let Some(sha) = observed {
                e.observed_disks.push(disk("game", "disk", sha));
            }
            let result = e.audit(&f.pin, f.expectation(layout));
            assert_eq!(result.state, state);
            assert!(result.mismatches.iter().any(|m| m.reason == reason));
            assert!(
                !result
                    .mismatches
                    .iter()
                    .any(|m| m.reason == MameMismatchReason::BadDump)
            );
        }
    }
}

#[test]
fn inherited_bios_and_device_disks_keep_declarations_and_optional_status() {
    let f = Fixture::new(&format!(
        r#"<machine name="game" romof="bios">{ROM}<device_ref name="device"/></machine>
        <machine name="bios" isbios="yes"><disk name="undumped" status="nodump"/><disk name="optional-bad" status="baddump" optional="yes" sha1="{DISK_SHA}"/></machine>
        <machine name="device" isdevice="yes"><disk name="bad" status="baddump" sha1="{DISK_SHA}"/><disk name="optional-gap" status="nodump" optional="yes"/></machine>"#
    ));
    for layout in LAYOUTS {
        let expected = f.expectation(layout);
        assert_eq!(expected.disks.len(), 2);
        for kind in [DependencyKind::Bios, DependencyKind::Device] {
            assert!(
                expected
                    .dependencies
                    .resolution
                    .references
                    .iter()
                    .any(|r| r.requirement.kind == kind
                        && r.requirement.outcome == DependencyOutcome::Satisfied)
            );
        }
        let mut e = observations(ObservedEvidenceCompleteness::Complete);
        e.observed_disks.push(disk("device", "bad", Some(DISK_SHA)));
        let result = e.audit(&f.pin, expected);
        assert_eq!(
            result.state,
            MameSetCompatibilityState::Unsupported,
            "{result:?}"
        );
        assert!(result.only_known_preservation_gaps());
        assert_eq!(result.ready_to_play_state, Some(ReadyToPlayState::Unknown));
        assert_eq!(result.mismatches.len(), 2);
        assert!(
            result
                .mismatches
                .iter()
                .any(|m| m.reason == MameMismatchReason::NoDump && m.set_name == "bios")
        );
        assert!(
            result
                .mismatches
                .iter()
                .any(|m| m.reason == MameMismatchReason::BadDump && m.set_name == "device")
        );
    }
}

#[test]
fn unresolved_disk_merge_is_not_excused_by_nodump_status() {
    let f = Fixture::disks(r#"<disk name="undumped" merge="provider" status="nodump"/>"#);
    let result = observations(ObservedEvidenceCompleteness::Complete)
        .audit(&f.pin, f.expectation(MameCollectionLayout::RuntimeSearch));
    assert_eq!(result.state, MameSetCompatibilityState::Unknown);
    assert!(!result.only_known_preservation_gaps());
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::EvidenceUnavailable)
    );
}

#[test]
fn stale_metadata_and_unknown_dump_status_remain_unexplained_evidence() {
    let mut f = Fixture::disks(r#"<disk name="undumped" status="nodump"/>"#);
    f.dat.games[0].mame_listxml_schema_version = Some(0);
    let result = observations(ObservedEvidenceCompleteness::Complete)
        .audit(&f.pin, f.expectation(MameCollectionLayout::RuntimeSearch));
    assert_eq!(result.state, MameSetCompatibilityState::Unknown);
    assert!(!result.only_known_preservation_gaps());
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::StaleMetadata)
    );
    let f = Fixture::disks(r#"<disk name="unknown" status="unrecognised"/>"#);
    let result = observations(ObservedEvidenceCompleteness::Complete)
        .audit(&f.pin, f.expectation(MameCollectionLayout::RuntimeSearch));
    assert_eq!(result.state, MameSetCompatibilityState::Unknown);
    assert!(!result.only_known_preservation_gaps());
}

#[test]
fn partial_observations_preserve_baddump_warning_without_claiming_compatibility() {
    let f = Fixture::disks(&format!(
        r#"<disk name="disk" sha1="{DISK_SHA}" status="baddump"/>"#
    ));
    let mut e = observations(ObservedEvidenceCompleteness::Partial);
    e.observed_disks.push(disk("game", "disk", Some(DISK_SHA)));
    let result = e.audit(&f.pin, f.expectation(MameCollectionLayout::RuntimeSearch));
    assert_eq!(result.state, MameSetCompatibilityState::Unknown);
    assert_eq!(result.ready_to_play_state, Some(ReadyToPlayState::Unknown));
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::BadDump)
    );
    assert!(
        result
            .mismatches
            .iter()
            .any(|m| m.reason == MameMismatchReason::EvidenceUnavailable)
    );
}

#[test]
fn a_known_undumped_disk_cannot_authorize_play_now_or_prove_mame_cannot_run() {
    use archivefs_core::arcade_compatibility::{
        ArcadeEmulatorCompatibility, evaluate_arcade_compatibility,
    };
    use archivefs_core::arcade_recommendation::{
        ArcadeRecommendationChoice, ArcadeRecommendationConfidence, ArcadeRecommendationOptions,
        recommend_arcade_emulator,
    };
    let f = Fixture::disks(r#"<disk name="undumped" status="nodump"/>"#);
    let summary = evaluate_arcade_compatibility(
        &observations(ObservedEvidenceCompleteness::Complete),
        Some((
            &f.pin,
            Some(f.expectation(MameCollectionLayout::RuntimeSearch)),
        )),
        None,
    );
    let ArcadeEmulatorCompatibility::Evaluated(result) = &summary.mame else {
        panic!("the pinned MAME source must be evaluated");
    };
    assert!(result.only_known_preservation_gaps());
    let recommendation =
        recommend_arcade_emulator(&summary, ArcadeRecommendationOptions::default());
    assert_eq!(
        recommendation.preferred,
        ArcadeRecommendationChoice::Unknown
    );
    assert_eq!(
        recommendation.confidence,
        ArcadeRecommendationConfidence::InsufficientEvidence
    );
    assert_eq!(recommendation.play_now, None);
}

#[test]
fn collection_preservation_content_verification_and_runnability_stay_separate() {
    for (status, preservation) in [
        ("nodump", MamePreservationStatus::NoVerifiedDumpExists),
        ("baddump", MamePreservationStatus::NeedsRedump),
        ("good", MamePreservationStatus::VerifiedDumpExpected),
    ] {
        let f = Fixture::disks(&format!(
            r#"<disk name="disk" status="{status}" sha1="{DISK_SHA}"/>"#
        ));
        let inventory = MameCollectionInventory {
            inspected_completely: true,
            sets: vec![MameObservedSet {
                name: "game".into(),
                location: "/synthetic/game.zip".into(),
                directory: false,
                members: vec![MameObservedMember {
                    name: "main".into(),
                    size_bytes: Some(4),
                    crc32: Some("aaaaaaaa".into()),
                    md5: None,
                    sha1: Some(ROM_SHA.into()),
                }],
            }],
            ..Default::default()
        };
        let verified = if status == "nodump" {
            vec![]
        } else {
            vec![MameVerifiedChd {
                set_name: "game".into(),
                name: "disk".into(),
                verified_overall_sha1: DISK_SHA.into(),
            }]
        };
        let analysis =
            analyse_collection_with_chds(&f.dat, &inventory, Some("0.280".into()), None, &verified);
        let result = analysis
            .set_results
            .iter()
            .find(|r| r.name == "game")
            .unwrap();
        assert_eq!(result.evidence.runnability, MameRunnability::NotAssessed);
        assert_eq!(result.evidence.verified_complete(), status == "good");
        let requirement = result
            .evidence
            .requirements
            .iter()
            .find(|r| r.disk)
            .unwrap();
        assert_eq!(requirement.preservation, preservation);
        assert_eq!(
            requirement.content == MameMemberEvidence::Verified,
            status != "nodump"
        );
        assert_eq!(result.health == MameSetHealth::Good, status == "good");
    }
}
