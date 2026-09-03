use super::*;
use crate::dat::audit::{AuditEntry, AuditReport, AuditSummary, AuditVerdict};
use crate::dat::model::DatEcosystem;
use crate::dat::sources::audit_run::{AuditedFileHashes, DatAuditContentOutcome};

const AUDITED_AT: &str = "2026-09-03T00:00:00Z";

fn base_outcome() -> DatAuditOutcome {
    DatAuditOutcome {
        source_id: "no-intro-nes".to_string(),
        source_display_name: "No-Intro - Nintendo Entertainment System".to_string(),
        dat_path: "/dats/nes.dat".to_string(),
        scan_root: "/roms/nes".to_string(),
        catalogue_names: vec!["Nintendo - Nintendo Entertainment System".to_string()],
        catalogue_entries: 1,
        catalogue_roms: 1,
        catalogue_version: Some("20240501-000000".to_string()),
        catalogue_author: Some("No-Intro".to_string()),
        catalogue_homepage: Some("No-Intro".to_string()),
        catalogue_ecosystem: Some(DatEcosystem::NoIntro),
        unreadable_catalogues: Vec::new(),
        report: AuditReport {
            entries: Vec::new(),
            summary: AuditSummary::default(),
        },
        evidence_sources: Vec::new(),
        archives: Vec::new(),
        sets: Vec::new(),
        unhashed: Vec::new(),
        files_scanned: 0,
        bytes_hashed: 0,
        archive_bytes_hashed: 0,
        truncated: false,
        policy: None,
        content: DatAuditContentOutcome::default(),
        platform: Some("NES".to_string()),
        cache: Default::default(),
        known_hashes: Default::default(),
    }
}

fn entry(local_path: &str, verdict: AuditVerdict) -> AuditEntry {
    AuditEntry {
        local_path: local_path.to_string(),
        local_filename: std::path::Path::new(local_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        verdict,
    }
}

fn exact_verdict() -> AuditVerdict {
    AuditVerdict::Exact {
        game_name: "Mystery Game (USA)".to_string(),
        rom_name: "Mystery Game (USA).nes".to_string(),
        algorithm: "SHA-1",
    }
}

fn hashes(sha1: &str) -> AuditedFileHashes {
    AuditedFileHashes {
        size_bytes: Some(131_072),
        crc32: Some("deadbeef".to_string()),
        md5: None,
        sha1: Some(sha1.to_string()),
        sha256: None,
    }
}

// --- happy path: one verified entry ----------------------------------------

#[test]
fn an_exact_match_is_projected_with_its_hashes_and_source_intact() {
    let mut outcome = base_outcome();
    let path = "/roms/nes/Mystery Game (USA).nes";
    outcome
        .known_hashes
        .insert(path.to_string(), hashes("aaaa1111"));
    outcome.report.entries.push(entry(path, exact_verdict()));

    let projection = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap();

    assert_eq!(projection.skipped.len(), 0);
    assert_eq!(projection.completeness, DatAuditCompleteness::Exhaustive);
    assert_eq!(projection.items.len(), 1);
    let item = &projection.items[0];
    assert_eq!(item.local_path, path);
    assert_eq!(
        item.identity.verification_state,
        crate::dat::library_identity_summary::DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".to_string()
        }
    );
    assert_eq!(item.identity.source.source_id, "no-intro-nes");
    assert_eq!(item.identity.source.ecosystem, Some(DatEcosystem::NoIntro));
    assert_eq!(
        item.identity.source.source_revision.as_deref(),
        Some("20240501-000000")
    );
    assert_eq!(
        item.identity.canonical.canonical_dat_name.as_deref(),
        Some("Mystery Game (USA)")
    );
    assert_eq!(
        item.identity.audited_hashes.sha1.as_deref(),
        Some("aaaa1111")
    );
    assert_eq!(item.identity.audited_at, AUDITED_AT);
    assert_eq!(item.identity.completeness, DatAuditCompleteness::Exhaustive);
}

// --- combined-audit refusal -------------------------------------------------

#[test]
fn a_combined_audit_outcome_is_refused_outright() {
    let mut outcome = base_outcome();
    outcome.source_id = COMBINED_AUDIT_SOURCE_ID.to_string();
    outcome
        .report
        .entries
        .push(entry("/roms/nes/game.nes", exact_verdict()));

    let refusal = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap_err();
    assert_eq!(refusal, LibraryDatIdentityProjectionRefusal::CombinedAudit);
}

// --- completeness / negative-verdict safety --------------------------------

#[test]
fn an_exhaustive_run_persists_a_negative_verdict() {
    let mut outcome = base_outcome();
    outcome
        .report
        .entries
        .push(entry("/roms/nes/unknown.nes", AuditVerdict::NotInDat));

    let projection = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap();

    assert_eq!(projection.skipped.len(), 0);
    assert_eq!(projection.items.len(), 1);
    assert_eq!(
        projection.items[0].identity.verification_state,
        crate::dat::library_identity_summary::DatVerificationState::NoMatch
    );
}

#[test]
fn a_truncated_run_withholds_a_negative_verdict_but_keeps_a_positive_one() {
    let mut outcome = base_outcome();
    outcome.truncated = true;
    outcome
        .report
        .entries
        .push(entry("/roms/nes/unknown.nes", AuditVerdict::NotInDat));
    outcome
        .report
        .entries
        .push(entry("/roms/nes/known.nes", exact_verdict()));

    let projection = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap();

    assert_eq!(projection.completeness, DatAuditCompleteness::Partial);
    assert_eq!(
        projection.items.len(),
        1,
        "only the positive match survives"
    );
    assert_eq!(projection.items[0].local_path, "/roms/nes/known.nes");
    assert_eq!(projection.skipped.len(), 1);
    assert_eq!(projection.skipped[0].0, "/roms/nes/unknown.nes");
    assert_eq!(
        projection.skipped[0].1,
        LibraryDatIdentitySkipReason::NegativeConclusionUnsafe
    );
}

#[test]
fn an_unreadable_catalogue_withholds_negative_verdicts_the_same_way_as_truncation() {
    let mut outcome = base_outcome();
    outcome.unreadable_catalogues = vec!["second.dat: malformed XML".to_string()];
    outcome.report.entries.push(entry(
        "/roms/nes/maybe-in-the-unreadable-file.nes",
        AuditVerdict::NoUsableEvidence,
    ));

    let projection = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap();

    assert_eq!(projection.completeness, DatAuditCompleteness::Partial);
    assert_eq!(projection.items.len(), 0);
    assert_eq!(projection.skipped.len(), 1);
}

#[test]
fn a_partial_run_still_persists_ambiguous_and_filename_only_positive_verdicts() {
    let mut outcome = base_outcome();
    outcome.truncated = true;
    outcome.report.entries.push(entry(
        "/roms/nes/ambiguous.nes",
        AuditVerdict::Ambiguous {
            detail: "conflicting evidence".to_string(),
        },
    ));
    outcome.report.entries.push(entry(
        "/roms/nes/filename-only.nes",
        AuditVerdict::FilenameOnly {
            game_name: "Filename Only Game".to_string(),
            rom_name: "Filename Only Game.nes".to_string(),
        },
    ));

    let projection = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap();

    assert_eq!(
        projection.items.len(),
        2,
        "Ambiguous and FilenameOnly both carry identity and are not negative conclusions"
    );
    assert!(projection.skipped.is_empty());
}

// --- multi-entry / no fabrication -------------------------------------------

#[test]
fn each_entry_produces_at_most_one_projected_row_keyed_by_its_own_path() {
    let mut outcome = base_outcome();
    outcome
        .report
        .entries
        .push(entry("/roms/nes/a.nes", exact_verdict()));
    outcome
        .report
        .entries
        .push(entry("/roms/nes/b.nes", AuditVerdict::NotInDat));

    let projection = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap();

    let mut paths: Vec<&str> = projection
        .items
        .iter()
        .map(|item| item.local_path.as_str())
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["/roms/nes/a.nes", "/roms/nes/b.nes"]);
}

#[test]
fn a_file_with_no_computed_hash_still_projects_with_empty_audited_hashes() {
    // `known_hashes` deliberately has nothing for this path (e.g. a
    // filename-only match with no hash evidence at all) - the projection
    // must not fabricate a hash, only leave the snapshot empty.
    let mut outcome = base_outcome();
    outcome.report.entries.push(entry(
        "/roms/nes/no-hash.nes",
        AuditVerdict::FilenameOnly {
            game_name: "No Hash Game".to_string(),
            rom_name: "No Hash Game.nes".to_string(),
        },
    ));

    let projection = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap();

    assert_eq!(projection.items.len(), 1);
    let hashes = &projection.items[0].identity.audited_hashes;
    assert_eq!(hashes.sha1, None);
    assert_eq!(hashes.crc32, None);
    assert_eq!(hashes.size_bytes, None);
}

#[test]
fn no_second_hashing_pass_occurs_hashes_come_only_from_the_outcome_already_given() {
    // There is no filesystem, transport, or hasher reachable from this
    // module at all - a projection can only ever echo `outcome.known_hashes`
    // verbatim. This test pins that by using a hash value that could not
    // exist on any real file (`known_hashes` is trusted verbatim).
    let mut outcome = base_outcome();
    let path = "/roms/nes/impossible.nes";
    outcome.known_hashes.insert(
        path.to_string(),
        hashes("not-a-real-sha1-but-trusted-verbatim"),
    );
    outcome.report.entries.push(entry(path, exact_verdict()));

    let projection = project_dat_audit_for_library_identity(&outcome, AUDITED_AT).unwrap();

    assert_eq!(
        projection.items[0].identity.audited_hashes.sha1.as_deref(),
        Some("not-a-real-sha1-but-trusted-verbatim")
    );
}
