//! Pure-model tests for the verified-identity fact cache.
//!
//! Every fixture builds a real [`GameIdentityReport`] by hand (all fields
//! are public). What is under test is the extraction/filtering rules and the
//! file-identity freshness derivation, not any detector.

use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use super::*;
use crate::game_identity::{IdentityImageFormat, IdentityPlatform, IdentityProvenance};

fn provenance(method: &str, member: Option<&[u8]>) -> IdentityProvenance {
    IdentityProvenance {
        archive_path: PathBuf::from("/library/game.iso"),
        member_path: member.map(<[u8]>::to_vec),
        member_index: None,
        method: method.to_string(),
    }
}

fn evidence(
    kind: IdentityKind,
    status: IdentityStatus,
    value: Option<&str>,
    confidence: IdentityConfidence,
) -> crate::game_identity::IdentityEvidence {
    crate::game_identity::IdentityEvidence {
        kind,
        status,
        value: value.map(str::to_string),
        confidence,
        provenance: provenance("test fixture", None),
        diagnostic: "test fixture evidence".to_string(),
    }
}

fn report(
    platform: IdentityPlatform,
    evidence: Vec<crate::game_identity::IdentityEvidence>,
    complete: bool,
) -> GameIdentityReport {
    GameIdentityReport {
        archive_path: PathBuf::from("/library/game.iso"),
        platform,
        format: IdentityImageFormat::Iso,
        evidence,
        warnings: Vec::new(),
        bytes_read: 4096,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete,
    }
}

fn kinds(facts: &[PersistableVerifiedFact]) -> Vec<IdentityKind> {
    facts.iter().map(|fact| fact.kind).collect()
}

// --- what is persisted ---------------------------------------------------

#[test]
fn a_verified_ps1_serial_is_persistable() {
    let facts = persistable_verified_facts(&report(
        IdentityPlatform::PlayStation,
        vec![evidence(
            IdentityKind::Ps1Serial,
            IdentityStatus::Verified,
            Some("SCUS-94103"),
            IdentityConfidence::ExactBytes,
        )],
        true,
    ));
    assert_eq!(kinds(&facts), vec![IdentityKind::Ps1Serial]);
    assert_eq!(facts[0].value, "SCUS-94103");
    assert_eq!(facts[0].confidence, IdentityConfidence::ExactBytes);
}

#[test]
fn every_sony_xbox_nintendo_and_dreamcast_launch_kind_round_trips() {
    // One report carrying a verified value for each launch/useful kind the
    // task enumerates; all must survive extraction unchanged.
    let cases: Vec<(IdentityKind, &str)> = vec![
        (IdentityKind::Ps1Serial, "SCUS-94103"),
        (IdentityKind::Ps2Serial, "SLUS-20946"),
        (IdentityKind::Pcsx2ExecutableCrc, "0F3E1B2A"),
        (IdentityKind::PspDiscId, "UCUS-98632"),
        (IdentityKind::Ps3TitleId, "BLUS30481"),
        (IdentityKind::XbeTitleId, "4D530064"),
        (IdentityKind::XexTitleId, "4D5307D5"),
        (IdentityKind::XexMediaId, "1E1F2A3B"),
        (IdentityKind::DolphinGameId, "GALE01"),
        (IdentityKind::DolphinRevision, "0"),
        (IdentityKind::DolphinDiscNumber, "0"),
        (IdentityKind::DolphinRegion, "NTSC-U"),
        (IdentityKind::DreamcastProductCode, "HDR-0076"),
    ];
    let evidence_list = cases
        .iter()
        .map(|(kind, value)| {
            evidence(
                *kind,
                IdentityStatus::Verified,
                Some(value),
                IdentityConfidence::StructuredMetadata,
            )
        })
        .collect();
    let facts = persistable_verified_facts(&report(IdentityPlatform::Wii, evidence_list, true));

    for (kind, value) in cases {
        let found = facts
            .iter()
            .find(|fact| fact.kind == kind)
            .unwrap_or_else(|| panic!("{kind:?} must be persistable"));
        assert_eq!(found.value, value, "{kind:?} value preserved");
    }
}

#[test]
fn a_candidate_value_is_never_persisted() {
    let facts = persistable_verified_facts(&report(
        IdentityPlatform::PlayStation2,
        vec![evidence(
            IdentityKind::Ps2Serial,
            IdentityStatus::Candidate,
            Some("SLUS-20946"),
            IdentityConfidence::ExactBytes,
        )],
        true,
    ));
    assert!(
        facts.is_empty(),
        "Candidate status must not be promoted to a stored fact"
    );
}

#[test]
fn a_filename_only_verified_value_is_never_persisted() {
    let facts = persistable_verified_facts(&report(
        IdentityPlatform::PlayStation,
        vec![evidence(
            IdentityKind::Ps1Serial,
            IdentityStatus::Verified,
            Some("SCUS-94103"),
            IdentityConfidence::FilenameOnly,
        )],
        true,
    ));
    assert!(
        facts.is_empty(),
        "a filename-derived value is not a verified byte fact"
    );
}

#[test]
fn non_identity_metadata_kinds_are_excluded_even_when_verified() {
    let facts = persistable_verified_facts(&report(
        IdentityPlatform::N64,
        vec![
            evidence(
                IdentityKind::LooseRomTitle,
                IdentityStatus::Verified,
                Some("Some Title"),
                IdentityConfidence::StructuredMetadata,
            ),
            evidence(
                IdentityKind::LooseRomFormat,
                IdentityStatus::Verified,
                Some("z64"),
                IdentityConfidence::StructuredMetadata,
            ),
            evidence(
                IdentityKind::Platform,
                IdentityStatus::Verified,
                Some("N64"),
                IdentityConfidence::StructuredMetadata,
            ),
        ],
        true,
    ));
    assert!(
        facts.is_empty(),
        "title/format/platform metadata is not an opaque identity fact"
    );
}

#[test]
fn a_conflicting_report_persists_neither_value_for_that_kind() {
    let facts = persistable_verified_facts(&report(
        IdentityPlatform::PlayStation2,
        vec![
            evidence(
                IdentityKind::Ps2Serial,
                IdentityStatus::Verified,
                Some("SLUS-20946"),
                IdentityConfidence::ExactBytes,
            ),
            evidence(
                IdentityKind::Ps2Serial,
                IdentityStatus::Verified,
                Some("SLES-51234"),
                IdentityConfidence::ExactBytes,
            ),
            // A non-conflicting second kind in the same report is kept.
            evidence(
                IdentityKind::Pcsx2ExecutableCrc,
                IdentityStatus::Verified,
                Some("0F3E1B2A"),
                IdentityConfidence::ExactBytes,
            ),
        ],
        true,
    ));
    assert_eq!(
        kinds(&facts),
        vec![IdentityKind::Pcsx2ExecutableCrc],
        "the conflicted serial is dropped, the unambiguous CRC stays"
    );
}

#[test]
fn a_repeated_identical_verified_value_is_not_a_conflict() {
    let facts = persistable_verified_facts(&report(
        IdentityPlatform::PlayStation,
        vec![
            evidence(
                IdentityKind::Ps1Serial,
                IdentityStatus::Verified,
                Some("SCUS-94103"),
                IdentityConfidence::ExactBytes,
            ),
            evidence(
                IdentityKind::Ps1Serial,
                IdentityStatus::Verified,
                Some("SCUS-94103"),
                IdentityConfidence::StructuredMetadata,
            ),
        ],
        true,
    ));
    assert_eq!(kinds(&facts), vec![IdentityKind::Ps1Serial]);
}

#[test]
fn multiple_distinct_fact_kinds_coexist() {
    let facts = persistable_verified_facts(&report(
        IdentityPlatform::Wii,
        vec![
            evidence(
                IdentityKind::DolphinGameId,
                IdentityStatus::Verified,
                Some("RSBE01"),
                IdentityConfidence::StructuredMetadata,
            ),
            evidence(
                IdentityKind::DolphinRegion,
                IdentityStatus::Verified,
                Some("NTSC-U"),
                IdentityConfidence::StructuredMetadata,
            ),
        ],
        true,
    ));
    let mut got = kinds(&facts);
    got.sort_by_key(|kind| format!("{kind:?}"));
    assert_eq!(
        got,
        vec![IdentityKind::DolphinGameId, IdentityKind::DolphinRegion]
    );
}

#[test]
fn provenance_method_and_member_path_are_carried() {
    let mut item = evidence(
        IdentityKind::XbeTitleId,
        IdentityStatus::Verified,
        Some("4D530064"),
        IdentityConfidence::ExactBytes,
    );
    item.provenance = provenance("xbe certificate", Some(b"default.xbe"));
    let facts = persistable_verified_facts(&report(IdentityPlatform::Xbox, vec![item], true));
    assert_eq!(facts[0].method.as_deref(), Some("xbe certificate"));
    assert_eq!(
        facts[0].member_path.as_deref(),
        Some(b"default.xbe".as_slice())
    );
}

// --- freshness ---------------------------------------------------------

fn stored_fact(
    device: u64,
    inode: u64,
    size: u64,
    modified_unix: Option<i64>,
) -> PersistedIdentityFact {
    PersistedIdentityFact {
        archive_id: 1,
        kind: IdentityKind::Ps2Serial,
        value: "SLUS-20946".to_string(),
        confidence: IdentityConfidence::ExactBytes,
        method: None,
        member_path: None,
        observed_at: "2026-01-01T00:00:00Z".to_string(),
        file_device: device,
        file_inode: inode,
        file_size_bytes: size,
        file_modified_unix_seconds: modified_unix,
    }
}

fn captured(
    device: u64,
    inode: u64,
    size: u64,
    modified_unix: Option<u64>,
) -> CapturedFileIdentity {
    CapturedFileIdentity {
        device,
        inode,
        size,
        modified: modified_unix.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)),
    }
}

#[test]
fn an_unchanged_file_identity_is_current() {
    let fact = stored_fact(66, 1234, 700_000, Some(1_700_000_000));
    let now = captured(66, 1234, 700_000, Some(1_700_000_000));
    assert_eq!(fact.freshness(Some(&now)), IdentityFactFreshness::Current);
}

#[test]
fn a_changed_size_is_stale() {
    let fact = stored_fact(66, 1234, 700_000, Some(1_700_000_000));
    let now = captured(66, 1234, 700_001, Some(1_700_000_000));
    assert_eq!(fact.freshness(Some(&now)), IdentityFactFreshness::Stale);
}

#[test]
fn a_changed_mtime_is_stale() {
    let fact = stored_fact(66, 1234, 700_000, Some(1_700_000_000));
    let now = captured(66, 1234, 700_000, Some(1_700_000_999));
    assert_eq!(fact.freshness(Some(&now)), IdentityFactFreshness::Stale);
}

#[test]
fn a_swapped_inode_is_stale_even_if_size_and_mtime_match() {
    let fact = stored_fact(66, 1234, 700_000, Some(1_700_000_000));
    let now = captured(66, 9999, 700_000, Some(1_700_000_000));
    assert_eq!(fact.freshness(Some(&now)), IdentityFactFreshness::Stale);
}

#[test]
fn no_current_identity_is_unknown() {
    let fact = stored_fact(66, 1234, 700_000, Some(1_700_000_000));
    assert_eq!(fact.freshness(None), IdentityFactFreshness::Unknown);
}

#[test]
fn matching_size_but_incomparable_mtime_is_unknown_not_current() {
    let fact = stored_fact(66, 1234, 700_000, None);
    let now = captured(66, 1234, 700_000, Some(1_700_000_000));
    assert_eq!(fact.freshness(Some(&now)), IdentityFactFreshness::Unknown);
}

#[test]
fn snapshot_reconstructs_the_captured_file_identity() {
    let fact = stored_fact(66, 1234, 700_000, Some(1_700_000_000));
    let snap = fact.snapshot();
    assert_eq!(snap.device, 66);
    assert_eq!(snap.inode, 1234);
    assert_eq!(snap.size, 700_000);
    assert_eq!(
        snap.modified
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok()),
        Some(Duration::from_secs(1_700_000_000))
    );
}

// --- enum <-> db name helpers ---------------------------------------------

#[test]
fn identity_kind_db_names_round_trip_for_every_persistable_kind() {
    for kind in [
        IdentityKind::Ps1Serial,
        IdentityKind::Ps2Serial,
        IdentityKind::Pcsx2ExecutableCrc,
        IdentityKind::PspDiscId,
        IdentityKind::Ps3TitleId,
        IdentityKind::DreamcastProductCode,
        IdentityKind::DolphinGameId,
        IdentityKind::DolphinRevision,
        IdentityKind::DolphinDiscNumber,
        IdentityKind::DolphinRegion,
        IdentityKind::XbeTitleId,
        IdentityKind::XexTitleId,
        IdentityKind::XexMediaId,
    ] {
        let name = identity_kind_to_db(kind);
        assert_eq!(
            identity_kind_from_db(&name),
            Some(kind),
            "{kind:?} via {name}"
        );
    }
}

#[test]
fn an_unknown_stored_kind_name_parses_to_none() {
    assert_eq!(identity_kind_from_db("not_a_real_kind"), None);
}

#[test]
fn identity_confidence_db_names_round_trip() {
    for confidence in [
        IdentityConfidence::ExactBytes,
        IdentityConfidence::StructuredMetadata,
        IdentityConfidence::CatalogueContext,
        IdentityConfidence::FilenameOnly,
        IdentityConfidence::Unavailable,
    ] {
        let name = identity_confidence_to_db(confidence);
        assert_eq!(identity_confidence_from_db(&name), Some(confidence));
    }
}

#[test]
fn persisted_fact_serde_round_trips() {
    let fact = stored_fact(66, 1234, 700_000, Some(1_700_000_000));
    let json = serde_json::to_vec(&fact).unwrap();
    let back: PersistedIdentityFact = serde_json::from_slice(&json).unwrap();
    assert_eq!(fact, back);
}

#[test]
fn system_time_before_the_epoch_is_negative_not_lost() {
    let before = UNIX_EPOCH - Duration::from_secs(10);
    assert_eq!(system_time_unix_seconds(before), Some(-10));
    let after = UNIX_EPOCH + Duration::from_secs(10);
    assert_eq!(system_time_unix_seconds(after), Some(10));
}
