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

// --- stale-evidence regressions ------------------------------------------
//
// The stored mtime has whole-second resolution, so an edit in the same second
// as the stored mtime is invisible to the comparison. A fact is only Current
// when its mtime is comfortably older than the moment it was observed.

const OBSERVED: &str = "2026-01-01T00:00:05Z";
const OBSERVED_UNIX: i64 = 1_767_225_605;

fn observed_fact(
    device: u64,
    inode: u64,
    size: u64,
    modified_unix: Option<i64>,
    observed_at: &str,
) -> PersistedIdentityFact {
    PersistedIdentityFact {
        observed_at: observed_at.to_string(),
        ..stored_fact(device, inode, size, modified_unix)
    }
}

fn captured_at(device: u64, inode: u64, size: u64, modified: Duration) -> CapturedFileIdentity {
    CapturedFileIdentity {
        device,
        inode,
        size,
        modified: Some(UNIX_EPOCH + modified),
    }
}

#[test]
fn a_same_size_edit_in_the_observation_second_is_not_current() {
    // Stored mtime == observation second; the file is then rewritten, same
    // size, 400 ms later within that very second: whole seconds still match.
    let fact = observed_fact(66, 1234, 700_000, Some(OBSERVED_UNIX), OBSERVED);
    let edited = captured_at(
        66,
        1234,
        700_000,
        Duration::from_secs(OBSERVED_UNIX as u64) + Duration::from_millis(400),
    );
    assert_ne!(
        fact.freshness(Some(&edited)),
        IdentityFactFreshness::Current
    );
}

#[test]
fn an_mtime_inside_the_safety_margin_before_observation_is_not_current() {
    for offset in [3600, 2, 1, 0, -1, -3600] {
        let mtime = OBSERVED_UNIX - offset;
        let fact = observed_fact(66, 1234, 700_000, Some(mtime), OBSERVED);
        let now = captured(66, 1234, 700_000, Some(mtime as u64));
        // offset 1 and 0 and a future mtime are racy / unprovable.
        let expected = if offset >= 2 {
            IdentityFactFreshness::Current
        } else {
            IdentityFactFreshness::Unknown
        };
        assert_eq!(fact.freshness(Some(&now)), expected, "offset {offset}");
    }
}

#[test]
fn an_mtime_comfortably_before_observation_stays_current() {
    let fact = observed_fact(66, 1234, 700_000, Some(OBSERVED_UNIX - 60), OBSERVED);
    let now = captured(66, 1234, 700_000, Some((OBSERVED_UNIX - 60) as u64));
    assert_eq!(fact.freshness(Some(&now)), IdentityFactFreshness::Current);
}

#[test]
fn an_unparseable_observation_time_cannot_prove_currency() {
    for observed in [
        "",
        "garbage",
        "2026-01-01 00:00:00",
        "2026-13-01T00:00:00Z",
        "2026-02-30T00:00:00Z",
        "2026-01-01T24:00:00Z",
        "2026-01-01T00:00:00+01:00",
    ] {
        let fact = observed_fact(66, 1234, 700_000, Some(1_700_000_000), observed);
        let now = captured(66, 1234, 700_000, Some(1_700_000_000));
        assert_eq!(
            fact.freshness(Some(&now)),
            IdentityFactFreshness::Unknown,
            "{observed:?}"
        );
    }
}

#[test]
fn an_unknown_file_object_identity_cannot_be_current() {
    // Rows persisted without a (device, inode), or a current capture without
    // one, cannot show it is the same object even if size and mtime agree.
    let stored_unknown = stored_fact(0, 0, 700_000, Some(1_700_000_000));
    let current_known = captured(66, 1234, 700_000, Some(1_700_000_000));
    assert_eq!(
        stored_unknown.freshness(Some(&current_known)),
        IdentityFactFreshness::Unknown
    );
    let stored_known = stored_fact(66, 1234, 700_000, Some(1_700_000_000));
    let current_unknown = captured(0, 0, 700_000, Some(1_700_000_000));
    assert_eq!(
        stored_known.freshness(Some(&current_unknown)),
        IdentityFactFreshness::Unknown
    );
    let both_unknown = captured(0, 0, 700_000, Some(1_700_000_000));
    assert_eq!(
        stored_unknown.freshness(Some(&both_unknown)),
        IdentityFactFreshness::Unknown
    );
    // Positive evidence of change still wins over "unknown".
    let resized = captured(0, 0, 700_001, Some(1_700_000_000));
    assert_eq!(
        stored_unknown.freshness(Some(&resized)),
        IdentityFactFreshness::Stale
    );
}

fn fact_from_real_file(path: &std::path::Path, observed_unix: i64) -> PersistedIdentityFact {
    let (_, identity) = crate::launch::process_spawn::capture_file_identity(path).unwrap();
    PersistedIdentityFact {
        observed_at: crate::database::format_unix_timestamp_utc(observed_unix),
        file_device: identity.device,
        file_inode: identity.inode,
        file_size_bytes: identity.size,
        file_modified_unix_seconds: identity.modified.and_then(system_time_unix_seconds),
        ..stored_fact(0, 0, 0, None)
    }
}

#[test]
fn a_real_same_size_rewrite_right_after_observation_is_not_current() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.iso");
    std::fs::write(&path, b"original-bytes!").unwrap();
    let now = system_time_unix_seconds(std::time::SystemTime::now()).unwrap();
    let fact = fact_from_real_file(&path, now);
    std::fs::write(&path, b"replaced-bytes!").unwrap();
    let (_, current) = crate::launch::process_spawn::capture_file_identity(&path).unwrap();
    assert_eq!(current.size, fact.file_size_bytes);
    assert_ne!(
        fact.freshness(Some(&current)),
        IdentityFactFreshness::Current
    );
}

#[test]
fn a_real_replacement_file_with_the_same_size_and_mtime_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.iso");
    std::fs::write(&path, b"original-bytes!").unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let now = system_time_unix_seconds(std::time::SystemTime::now()).unwrap();
    let fact = fact_from_real_file(&path, now);
    // A different file is moved into place carrying the same size and mtime.
    let replacement = dir.path().join("incoming.iso");
    std::fs::write(&replacement, b"replaced-bytes!").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(old)
        .unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    let (_, current) = crate::launch::process_spawn::capture_file_identity(&path).unwrap();
    assert_eq!(fact.freshness(Some(&current)), IdentityFactFreshness::Stale);
}

#[test]
fn the_observation_time_parser_round_trips_the_database_formatter() {
    for unix in [
        0,
        86_399,
        951_782_400,   // 2000-02-29
        1_078_012_800, // 2004-02-29
        1_700_000_000,
        1_767_225_605,
        4_107_542_400, // 2100-03-01 (2100 is not a leap year)
        4_102_444_799,
    ] {
        let text = crate::database::format_unix_timestamp_utc(unix);
        assert_eq!(parse_utc_timestamp(&text), Some(unix), "{text}");
    }
    assert_eq!(parse_utc_timestamp("2100-02-29T00:00:00Z"), None);
}
