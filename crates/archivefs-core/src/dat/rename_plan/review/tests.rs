use super::*;
use crate::dat::sources::{
    DatSourceKind,
    audit_cache::AuditCacheConfig,
    audit_run::{DatAuditRequest, run_dat_audit_with_cache},
};
use std::fs;
use tempfile::TempDir;

fn fixture(game: &str) -> (TempDir, DatAuditOutcome) {
    let root = TempDir::new().unwrap();
    let roms = root.path().join("roms");
    fs::create_dir(&roms).unwrap();
    fs::write(roms.join("old.nes"), b"abc").unwrap();
    let dat = root.path().join("catalogue.dat");
    fs::write(&dat, format!(r#"<datafile><header><name>Test</name><version>2026-10-27</version></header><game name="{game}"><rom name="Canonical.nes" size="3" sha1="a9993e364706816aba3e25717850c26c9cd0d89d"/></game></datafile>"#)).unwrap();
    let request = DatAuditRequest {
        source_id: "test".into(),
        source_display_name: "Selected local DAT".into(),
        dat_path: dat,
        dat_kind: DatSourceKind::File,
        scan_root: roms,
        limits: Default::default(),
        policy: None,
        platform: Some("nes".into()),
    };
    let outcome = run_dat_audit_with_cache(
        &request,
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    (root, outcome)
}
fn review(outcome: &DatAuditOutcome) -> CanonicalReview {
    build_review(outcome, &AtomicBool::new(false)).unwrap()
}

#[test]
fn exact_match_has_working_proposal_provenance_and_no_mutation() {
    let (root, outcome) = fixture("Example (USA)");
    let before_dat = fs::read(&outcome.dat_path).unwrap();
    let result = review(&outcome);
    assert!(
        result.catalogue_problem.is_none(),
        "{:?}",
        result.catalogue_problem
    );
    assert_eq!(result.version.as_deref(), Some("2026-10-27"));
    assert_eq!(result.rows.len(), 1);
    let row = &result.rows[0];
    assert_eq!(row.disposition, ReviewDisposition::Eligible, "{:?}", row);
    assert_eq!(
        row.proposal.as_ref().unwrap().proposed_basename.as_deref(),
        Some("Canonical.nes")
    );
    assert!(row.identity.iter().any(|s| s.starts_with("SHA-1: a9993")));
    assert_eq!(fs::read(&row.path).unwrap(), b"abc");
    assert!(!root.path().join("roms/Canonical.nes").exists());
    assert_eq!(fs::read(&outcome.dat_path).unwrap(), before_dat);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}
#[test]
fn stale_catalogue_and_source_are_held() {
    let (_root, outcome) = fixture("Example");
    fs::write(&outcome.dat_path, b"different").unwrap();
    let result = review(&outcome);
    assert!(result.catalogue_problem.as_ref().unwrap().contains("Stale"));
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Held);
    let (_root, outcome) = fixture("Example");
    fs::write(&outcome.report.entries[0].local_path, b"changed").unwrap();
    let result = review(&outcome);
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Held);
    assert!(
        result.rows[0]
            .reasons
            .iter()
            .any(|s| s.contains("fingerprint"))
    );
}
#[test]
fn new_non_audited_directory_case_collision_is_held() {
    let (root, outcome) = fixture("Example");
    fs::create_dir(root.path().join("roms/CANONICAL.NES")).unwrap();
    let result = review(&outcome);
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Held);
    assert!(
        result.rows[0]
            .reasons
            .iter()
            .any(|s| s.contains("collision"))
    );
}
#[test]
fn duplicate_hash_identity_across_paths_is_held() {
    let (root, mut outcome) = fixture("Example");
    let other = root.path().join("other");
    fs::create_dir(&other).unwrap();
    let entry = outcome.report.entries[0].clone();
    let copy = other.join("copy.nes");
    fs::write(&copy, b"abc").unwrap();
    let mut hashes = outcome.known_hashes.get(&entry.local_path).unwrap().clone();
    hashes.fingerprint = FileFingerprint::observe(&copy);
    outcome
        .known_hashes
        .insert(copy.to_string_lossy().into_owned(), hashes);
    let mut copied_entry = entry;
    copied_entry.local_path = copy.to_string_lossy().into_owned();
    copied_entry.local_filename = "copy.nes".into();
    outcome.report.entries.push(copied_entry);
    outcome.report.summary.total += 1;
    let result = review(&outcome);
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Held);
    assert!(
        result
            .rows
            .iter()
            .all(|row| row.disposition == ReviewDisposition::Held
                && row.reasons.iter().any(|s| s.contains("Duplicate")))
    );
}
#[test]
fn multidisc_related_media_are_held_together_without_name_authority() {
    let (_root, outcome) = fixture("Example (Disc 1 of 2)");
    let result = review(&outcome);
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Held);
    assert!(
        result.rows[0]
            .reasons
            .iter()
            .any(|s| s.contains("part 1 of 2"))
    );
}
#[test]
fn weak_and_conflicting_evidence_are_visible_without_authoritative_names() {
    let (_root, mut outcome) = fixture("Example");
    outcome.report.entries[0].verdict = AuditVerdict::FilenameOnly {
        game_name: "Example".into(),
        rom_name: "Canonical.nes".into(),
    };
    let result = review(&outcome);
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Refused);
    assert!(result.rows[0].proposal.is_none());
    outcome.report.entries[0].verdict = AuditVerdict::Ambiguous {
        detail: "Catalogue records disagree".into(),
    };
    let result = review(&outcome);
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Held);
    assert!(result.rows[0].reasons[0].contains("Conflicting DAT"));
}
#[test]
fn unassigned_authority_and_partial_audits_cannot_be_eligible() {
    let (_root, mut outcome) = fixture("Example");
    outcome.platform = None;
    assert_eq!(
        review(&outcome).rows[0].disposition,
        ReviewDisposition::Held
    );
    outcome.platform = Some("nes".into());
    outcome.truncated = true;
    assert_eq!(
        review(&outcome).rows[0].disposition,
        ReviewDisposition::Held
    );
}
#[cfg(unix)]
#[test]
fn new_broken_symlink_destination_is_preserved_and_held() {
    let (root, outcome) = fixture("Example");
    let destination = root.path().join("roms/Canonical.nes");
    std::os::unix::fs::symlink("missing", &destination).unwrap();
    assert_eq!(
        review(&outcome).rows[0].disposition,
        ReviewDisposition::Held
    );
    assert!(
        fs::symlink_metadata(destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn multiple_exact_records_and_unsupported_extensions_remain_distinct() {
    let (_root, mut outcome) = fixture("Example");
    outcome.report.entries[0].verdict = AuditVerdict::ExactMultipleCandidates {
        algorithm: "sha1",
        count: 2,
        game_names: vec!["Example".into(), "Different".into()],
    };
    let result = review(&outcome);
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Held);
    assert!(
        result.rows[0]
            .proposal
            .as_ref()
            .unwrap()
            .proposed_basename
            .is_none()
    );
    outcome.report.entries[0].verdict = AuditVerdict::Exact {
        game_name: "Example".into(),
        rom_name: "Canonical.zip".into(),
        algorithm: "sha1",
    };
    let result = review(&outcome);
    assert_eq!(result.rows[0].disposition, ReviewDisposition::Refused);
    assert_eq!(
        result.rows[0].proposal.as_ref().unwrap().state,
        ProposalState::Unsupported
    );
}
