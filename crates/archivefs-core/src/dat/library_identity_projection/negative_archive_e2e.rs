//! Same-system negatives must describe examined content, never a failed reader.
use super::*;
use crate::LibraryDatState;
use crate::attention::AttentionCategory;

fn same_system(members: &[(&str, &[u8])]) -> Fixture {
    let mut fx = Fixture::new(
        Format::Zip,
        members,
        &[Dat::Full("Listed", "listed.gb", GAME)],
    );
    let xml = std::fs::read_to_string(&fx.dat_path).unwrap();
    std::fs::write(
        &fx.dat_path,
        xml.replace("<name>Fixture</name>", "<name>Nintendo - Game Boy</name>"),
    )
    .unwrap();
    fx.database
        .assign_platform(fx.archive_id(), Some("Game Boy"), "test")
        .unwrap();
    fx
}

fn save(fx: &mut Fixture, outcome: &DatAuditOutcome) {
    fx.database
        .persist_library_dat_identities_from_audit(outcome)
        .unwrap();
}

fn assert_not_checked(fx: &Fixture) {
    assert_eq!(
        fx.database
            .library_dat_states()
            .unwrap()
            .get(&fx.archive_id()),
        None
    );
    let snapshot = fx.database.attention_snapshot().unwrap();
    assert!(
        snapshot
            .items()
            .filter(|i| i.category == AttentionCategory::Dat)
            .all(|i| !i.summary.contains("No DAT entry matched")),
        "{snapshot:?}"
    );
}

#[test]
fn corrupt_zip_has_no_negative_dat_identity() {
    let mut fx = same_system(&[("renamed.gb", GAME)]);
    std::fs::write(&fx.archive, b"PK\x03\x04unreadable archive control").unwrap();
    let outcome = fx.audit();
    assert!(matches!(
        outcome.archives[0].completion,
        ArchivePassCompletion::Incomplete { .. }
    ));
    assert_eq!(
        outcome.report.entries[0].verdict,
        AuditVerdict::NoUsableEvidence
    );
    save(&mut fx, &outcome);
    assert_not_checked(&fx);
}

#[test]
fn combined_corrupt_zip_report_cannot_claim_no_catalogue_match() {
    use crate::dat::sources::audit_run::{
        CombinedDatAuditRequest, CombinedDatAuditSource, run_combined_dat_audit_with_cache,
    };
    let fx = same_system(&[("renamed.gb", GAME)]);
    std::fs::write(&fx.archive, b"PK\x03\x04unreadable archive control").unwrap();
    let outcome = run_combined_dat_audit_with_cache(
        &CombinedDatAuditRequest {
            sources: vec![CombinedDatAuditSource {
                source_id: SOURCE_ID.into(),
                source_display_name: "No-Intro Test".into(),
                dat_path: fx.dat_path.clone(),
                dat_kind: DatSourceKind::File,
                platform: Some("Game Boy".into()),
            }],
            scan_root: fx.source.clone(),
            limits: DatLimits::default(),
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    assert!(matches!(
        outcome.archives[0].completion,
        ArchivePassCompletion::Incomplete { .. }
    ));
    assert_eq!(
        outcome.report.entries[0].verdict,
        AuditVerdict::NoUsableEvidence
    );
    assert_eq!(outcome.report.summary.not_in_dat, 0);
}

#[test]
fn successfully_examined_zip_keeps_same_system_nonmatch() {
    let mut fx = same_system(&[("unlisted.gb", OTHER)]);
    let outcome = fx.audit();
    assert_eq!(outcome.report.entries[0].verdict, AuditVerdict::NotInDat);
    assert_eq!(
        outcome.archives[0].members[0].verdict,
        Some(AuditVerdict::NotInDat)
    );
    save(&mut fx, &outcome);
    assert_eq!(
        fx.database
            .library_dat_states()
            .unwrap()
            .get(&fx.archive_id()),
        Some(&LibraryDatState::NoMatch)
    );
}

#[test]
fn unsupported_zip_member_is_not_a_catalogue_negative_even_when_pass_completes() {
    let mut fx = same_system(&[("unlisted.gb", OTHER)]);
    let mut bytes = std::fs::read(&fx.archive).unwrap();
    for (signature, offset) in [(b"PK\x03\x04", 8), (b"PK\x01\x02", 10)] {
        let at = bytes.windows(4).position(|part| part == signature).unwrap() + offset;
        // BZip2 is a recognized ZIP method that this bounded reader refuses;
        // unlike method 99 it does not require an AES extra-data field.
        bytes[at..at + 2].copy_from_slice(&12_u16.to_le_bytes());
    }
    std::fs::write(&fx.archive, bytes).unwrap();
    let outcome = fx.audit();
    assert_eq!(
        outcome.archives[0].completion,
        ArchivePassCompletion::Complete
    );
    assert!(matches!(
        outcome.archives[0].members[0].evidence.status,
        ArchiveMemberStatus::UnsupportedCodec { .. }
    ));
    assert_eq!(
        outcome.report.entries[0].verdict,
        AuditVerdict::NoUsableEvidence
    );
    save(&mut fx, &outcome);
    assert_not_checked(&fx);
}

#[test]
fn fileonly_keeps_outer_byte_negative_without_inspecting_members() {
    let mut fx = same_system(&[("unlisted.gb", OTHER)]);
    let xml = std::fs::read_to_string(&fx.dat_path).unwrap();
    std::fs::write(
        &fx.dat_path,
        xml.replace("<header>", "<header><romvault forcepacking=\"fileonly\"/>"),
    )
    .unwrap();
    std::fs::write(
        &fx.archive,
        b"PK\x03\x04not a readable zip, intentionally outer-only",
    )
    .unwrap();
    let outcome = fx.audit();
    assert!(outcome.archives.is_empty());
    assert_eq!(outcome.report.entries[0].verdict, AuditVerdict::NotInDat);
    save(&mut fx, &outcome);
    assert_eq!(
        fx.database
            .library_dat_states()
            .unwrap()
            .get(&fx.archive_id()),
        Some(&LibraryDatState::NoMatch)
    );
}

#[test]
fn valid_zip_still_verifies_renamed_member_by_hash() {
    let mut fx = same_system(&[("renamed.gb", GAME)]);
    let outcome = fx.audit();
    save(&mut fx, &outcome);
    assert_eq!(
        fx.database
            .library_dat_states()
            .unwrap()
            .get(&fx.archive_id()),
        Some(&LibraryDatState::Verified)
    );
    assert!(
        fx.database
            .library_dat_identity_for_item(fx.archive_id(), SOURCE_ID)
            .unwrap()
            .unwrap()
            .archive_member
            .is_some()
    );
}

#[test]
fn incomplete_or_unexamined_members_cannot_persist_outer_negative() {
    use crate::dat::archive::ArchivePassStopReason;
    let cases = [
        ArchiveMemberStatus::Encrypted,
        ArchiveMemberStatus::UnsupportedCodec {
            method: "future codec".into(),
        },
        ArchiveMemberStatus::Corrupt {
            detail: "CRC mismatch".into(),
        },
        ArchiveMemberStatus::RefusedLimits {
            reason: "member size",
        },
        ArchiveMemberStatus::NestedArchive,
        ArchiveMemberStatus::NotVerified {
            reason: "unexamined",
        },
        // Also exercise complete metadata with the claimed hash missing.
        ArchiveMemberStatus::HashComplete,
    ];
    for status in cases {
        let mut fx = same_system(&[("unlisted.gb", OTHER)]);
        let mut outcome = fx.audit();
        outcome.archives[0].members[0].evidence.status = status;
        outcome.archives[0].members[0].evidence.hashes = None;
        // A previously built outcome can reach persistence directly. Do not
        // rely solely on the audit runner having normalized its flat report.
        assert_eq!(outcome.report.entries[0].verdict, AuditVerdict::NotInDat);
        save(&mut fx, &outcome);
        assert_not_checked(&fx);
    }
    for missing_outer in [false, true] {
        let mut fx = same_system(&[("unlisted.gb", OTHER)]);
        let mut outcome = fx.audit();
        if missing_outer {
            outcome.archives[0].outer_identity = None;
        } else {
            outcome.archives[0].completion = ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::RunLogicalBudget,
            };
        }
        save(&mut fx, &outcome);
        assert_not_checked(&fx);
    }
}

#[test]
fn corrupt_archive_does_not_suppress_successfully_examined_siblings() {
    let mut fx = same_system(&[("unlisted.gb", OTHER)]);
    let loose_match = fx.source.join("listed.gb");
    let loose_negative = fx.source.join("unlisted.gb");
    std::fs::write(&loose_match, GAME).unwrap();
    std::fs::write(&loose_negative, OTHER).unwrap();
    let corrupt = fx.source.join("broken.zip");
    std::fs::write(&corrupt, b"PK\x03\x04unreadable archive control").unwrap();
    Fixture::scan(&mut fx.database, &fx.source.clone(), &fx.root.clone());
    for row in fx.database.load_archives().unwrap() {
        fx.database
            .assign_platform(row.id, Some("Game Boy"), "test")
            .unwrap();
    }
    let outcome = fx.audit();
    save(&mut fx, &outcome);
    let states = fx.database.library_dat_states().unwrap();
    for (path, expected) in [
        (&fx.archive, Some(LibraryDatState::NoMatch)),
        (&loose_match, Some(LibraryDatState::Verified)),
        (&loose_negative, Some(LibraryDatState::NoMatch)),
        (&corrupt, None),
    ] {
        let id = fx
            .database
            .find_archive_id_by_absolute_path(path)
            .unwrap()
            .unwrap();
        assert_eq!(states.get(&id), expected.as_ref(), "{}", path.display());
    }
}

#[test]
fn unavailable_catalogue_row_remains_not_checked() {
    use crate::dat::sources::audit_run::run_dat_audit_with_targets;
    use crate::dat::sources::audit_targets::{AuditTargets, catalogue_audit_targets};
    let fx = same_system(&[("unlisted.gb", OTHER)]);
    std::fs::remove_file(&fx.archive).unwrap();
    let targets =
        AuditTargets::Catalogue(catalogue_audit_targets([fx.archive.clone()], &fx.source));
    let outcome = run_dat_audit_with_targets(
        &DatAuditRequest {
            source_id: SOURCE_ID.into(),
            source_display_name: "No-Intro Test".into(),
            dat_path: fx.dat_path.clone(),
            dat_kind: DatSourceKind::File,
            scan_root: fx.source.clone(),
            limits: DatLimits::default(),
            policy: None,
            platform: Some("Game Boy".into()),
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
        &targets,
    );
    assert!(matches!(
        outcome,
        Err(crate::dat::sources::audit_run::DatAuditError::NothingToAudit(_))
    ));
    assert_not_checked(&fx);
}

#[test]
fn incomplete_archive_cannot_overwrite_protected_exact_or_other_source_history() {
    let mut fx = same_system(&[("renamed.gb", GAME)]);
    let exact = fx.audit();
    save(&mut fx, &exact);
    let before = fx
        .database
        .library_dat_identity_for_item(fx.archive_id(), SOURCE_ID)
        .unwrap();
    let mut historical = exact.clone();
    historical.source_id = "historical-catalogue".into();
    save(&mut fx, &historical);
    let history = fx
        .database
        .library_dat_identity_for_item(fx.archive_id(), "historical-catalogue")
        .unwrap();
    std::fs::write(&fx.archive, b"PK\x03\x04broken").unwrap();
    let failed = fx.audit();
    save(&mut fx, &failed);
    assert_eq!(
        fx.database
            .library_dat_identity_for_item(fx.archive_id(), SOURCE_ID)
            .unwrap(),
        before
    );
    assert_eq!(
        fx.database
            .library_dat_identity_for_item(fx.archive_id(), "historical-catalogue")
            .unwrap(),
        history
    );
    assert_eq!(
        fx.database.load_archives().unwrap()[0].platform.as_deref(),
        Some("Game Boy")
    );
}
