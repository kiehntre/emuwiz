use super::*;
use crate::dat::audit::{AuditEntry, AuditReport, AuditSummary};
use crate::dat::dependency::SetDependencyReport;
use crate::dat::index::{DatMemberKey, MemberLocation};
use crate::dat::model::DatEcosystem;
use crate::dat::set::{SetIdentity, SetResolution, SetState};
use std::path::PathBuf;

// --- fixtures --------------------------------------------------------------

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
        files_scanned: 1,
        bytes_hashed: 4,
        archive_bytes_hashed: 0,
        truncated: false,
        policy: None,
        content: Default::default(),
        platform: Some("NES".to_string()),
        cache: Default::default(),
    }
}

fn hashes() -> LibraryItemHashes {
    LibraryItemHashes {
        size_bytes: Some(65_536),
        crc32: Some("abcd1234".to_string()),
        md5: Some("d41d8cd98f00b204e9800998ecf8427e".to_string()),
        sha1: Some("da39a3ee5e6b4b0d3255bfef95601890afd80709".to_string()),
        sha256: None,
    }
}

fn query<'a>(
    outcome: &'a DatAuditOutcome,
    verdict: &'a AuditVerdict,
    audited: &'a LibraryItemHashes,
    current: Option<&'a LibraryItemHashes>,
) -> LibraryDatIdentityQuery<'a> {
    LibraryDatIdentityQuery {
        outcome,
        verdict,
        matched_refs: &[],
        audited_hashes: audited,
        current_hashes: current,
    }
}

fn exact(name: &str) -> AuditVerdict {
    AuditVerdict::Exact {
        game_name: name.to_string(),
        rom_name: format!("{name}.nes"),
        algorithm: "SHA-1",
    }
}

// --- verification states -------------------------------------------------

#[test]
fn verified_single_match_is_marked_verified_with_its_algorithm() {
    let outcome = base_outcome();
    let verdict = exact("Super Mario Bros. (World)");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));

    assert!(summary.is_verified());
    assert!(!summary.is_ambiguous());
    assert_eq!(
        summary.verification_state,
        DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".to_string()
        }
    );
    assert_eq!(
        summary.hash_evidence.matched_algorithm.as_deref(),
        Some("SHA-1")
    );
    assert_eq!(
        summary.hash_evidence.matched_value.as_deref(),
        Some("da39a3ee5e6b4b0d3255bfef95601890afd80709")
    );
    assert_eq!(
        summary.hash_evidence.available_algorithms,
        vec!["SHA-1", "MD5", "CRC32"]
    );
    assert!(summary.ambiguous_candidates.is_empty());
}

#[test]
fn no_match_is_reported_without_a_canonical_name_or_hash_evidence() {
    let outcome = base_outcome();
    let audited = hashes();
    let summary =
        summarize_library_dat_identity(&query(&outcome, &AuditVerdict::NotInDat, &audited, None));

    assert_eq!(summary.verification_state, DatVerificationState::NoMatch);
    assert!(summary.is_no_match());
    assert!(!summary.is_verified());
    assert_eq!(summary.canonical.canonical_dat_name, None);
    assert_eq!(summary.hash_evidence.matched_algorithm, None);
    assert_eq!(summary.hash_evidence.matched_value, None);
    // The item's hashes are still surfaced so a person can see what was tried.
    assert_eq!(
        summary.hash_evidence.available_algorithms,
        vec!["SHA-1", "MD5", "CRC32"]
    );
}

#[test]
fn no_usable_evidence_is_distinct_from_no_match() {
    let outcome = base_outcome();
    let empty = LibraryItemHashes::default();
    let summary = summarize_library_dat_identity(&query(
        &outcome,
        &AuditVerdict::NoUsableEvidence,
        &empty,
        None,
    ));
    assert_eq!(
        summary.verification_state,
        DatVerificationState::NoUsableEvidence
    );
    assert!(summary.is_no_match());
    assert!(summary.hash_evidence.available_algorithms.is_empty());
}

#[test]
fn multiple_cryptographic_candidates_are_ambiguous_and_list_the_names() {
    let outcome = base_outcome();
    let verdict = AuditVerdict::ExactMultipleCandidates {
        algorithm: "SHA-1",
        count: 2,
        game_names: vec!["Game (USA)".to_string(), "Game (Europe)".to_string()],
    };
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));

    assert!(summary.is_ambiguous());
    assert!(!summary.is_verified());
    assert_eq!(
        summary.verification_state,
        DatVerificationState::AmbiguousMultipleCandidates {
            algorithm: "SHA-1".to_string(),
            candidate_count: 2
        }
    );
    assert_eq!(
        summary.ambiguous_candidates,
        vec!["Game (USA)".to_string(), "Game (Europe)".to_string()]
    );
    assert_eq!(summary.canonical.canonical_dat_name, None);
}

#[test]
fn conflicting_evidence_is_reported_with_its_detail() {
    let outcome = base_outcome();
    let verdict = AuditVerdict::Ambiguous {
        detail: "CRC32 matches 3 entries but size disagrees with all".to_string(),
    };
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));
    assert!(summary.is_ambiguous());
    match summary.verification_state {
        DatVerificationState::Conflicting { detail } => {
            assert!(detail.contains("size disagrees"))
        }
        other => panic!("expected Conflicting, got {other:?}"),
    }
}

#[test]
fn a_crc32_probable_match_is_probable_not_verified() {
    let outcome = base_outcome();
    let verdict = AuditVerdict::Probable {
        game_name: "Some Game (USA)".to_string(),
        rom_name: "some.nes".to_string(),
    };
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));
    assert_eq!(summary.verification_state, DatVerificationState::Probable);
    assert!(!summary.is_verified());
    assert_eq!(
        summary.hash_evidence.matched_algorithm.as_deref(),
        Some("CRC32+size")
    );
    assert_eq!(
        summary.hash_evidence.matched_value.as_deref(),
        Some("abcd1234")
    );
}

// --- no filename-derived verification ---------------------------------

#[test]
fn a_filename_only_match_is_never_verification() {
    let outcome = base_outcome();
    let verdict = AuditVerdict::FilenameOnly {
        game_name: "Named Game (Europe)".to_string(),
        rom_name: "named.nes".to_string(),
    };
    let audited = LibraryItemHashes::default();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));

    assert_eq!(
        summary.verification_state,
        DatVerificationState::FilenameOnlyNotVerified
    );
    assert!(!summary.is_verified());
    assert!(!summary.is_ambiguous());
    assert_eq!(summary.hash_evidence.matched_algorithm, None);
    assert_eq!(summary.hash_evidence.matched_value, None);
    // The canonical name is still surfaced, just not as verification.
    assert_eq!(
        summary.canonical.canonical_dat_name.as_deref(),
        Some("Named Game (Europe)")
    );
}

// --- source / ecosystem provenance ------------------------------------

#[test]
fn source_and_ecosystem_provenance_come_straight_from_the_outcome() {
    let outcome = base_outcome();
    let verdict = exact("Game");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));

    assert_eq!(summary.source.source_id, "no-intro-nes");
    assert_eq!(
        summary.source.source_name,
        "No-Intro - Nintendo Entertainment System"
    );
    assert_eq!(summary.source.ecosystem, Some(DatEcosystem::NoIntro));
    assert_eq!(
        summary.source.source_revision.as_deref(),
        Some("20240501-000000")
    );
    assert_eq!(summary.source.author.as_deref(), Some("No-Intro"));
    assert_eq!(
        summary.source.catalogue_names,
        vec!["Nintendo - Nintendo Entertainment System".to_string()]
    );
    assert_eq!(summary.source.dat_path, "/dats/nes.dat");
}

// --- stale / current / unknown --------------------------------------

#[test]
fn provenance_is_current_when_the_audited_hash_equals_the_live_hash() {
    let outcome = base_outcome();
    let verdict = exact("Game");
    let audited = hashes();
    let current = hashes();
    let summary =
        summarize_library_dat_identity(&query(&outcome, &verdict, &audited, Some(&current)));
    assert_eq!(
        summary.provenance_freshness,
        DatProvenanceFreshness::Current
    );
}

#[test]
fn provenance_is_stale_when_the_live_hash_differs() {
    let outcome = base_outcome();
    let verdict = exact("Game");
    let audited = hashes();
    let mut current = hashes();
    current.sha1 = Some("0000000000000000000000000000000000000000".to_string());
    let summary =
        summarize_library_dat_identity(&query(&outcome, &verdict, &audited, Some(&current)));
    assert_eq!(summary.provenance_freshness, DatProvenanceFreshness::Stale);
}

#[test]
fn provenance_is_unknown_without_a_live_hash_snapshot() {
    let outcome = base_outcome();
    let verdict = exact("Game");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));
    assert_eq!(
        summary.provenance_freshness,
        DatProvenanceFreshness::Unknown
    );
}

#[test]
fn provenance_is_unknown_when_no_hash_algorithm_overlaps() {
    let outcome = base_outcome();
    let verdict = exact("Game");
    let audited = LibraryItemHashes {
        sha1: Some("da39a3ee5e6b4b0d3255bfef95601890afd80709".to_string()),
        ..Default::default()
    };
    let current = LibraryItemHashes {
        sha256: Some(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        ),
        ..Default::default()
    };
    let summary =
        summarize_library_dat_identity(&query(&outcome, &verdict, &audited, Some(&current)));
    assert_eq!(
        summary.provenance_freshness,
        DatProvenanceFreshness::Unknown
    );
}

// --- canonical name / region / revision ------------------------------

#[test]
fn canonical_name_region_and_revision_come_from_the_matched_entry_name() {
    let outcome = base_outcome();
    let verdict = exact("The Legend of Zelda (USA) (Rev 1)");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));

    assert_eq!(
        summary.canonical.canonical_dat_name.as_deref(),
        Some("The Legend of Zelda (USA) (Rev 1)")
    );
    assert_eq!(
        summary.canonical.canonical_rom_name.as_deref(),
        Some("The Legend of Zelda (USA) (Rev 1).nes")
    );
    assert_eq!(summary.canonical.region.as_deref(), Some("USA"));
    assert_eq!(summary.canonical.revision.as_deref(), Some("Rev 1"));
}

#[test]
fn a_multi_region_token_is_kept_verbatim_and_a_non_region_token_is_ignored() {
    let outcome = base_outcome();
    let verdict = exact("Game (USA, Europe) (Beta)");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));
    assert_eq!(summary.canonical.region.as_deref(), Some("USA, Europe"));
    assert_eq!(summary.canonical.revision, None);
}

#[test]
fn a_region_declared_on_the_dat_rom_metadata_wins_over_the_name_scan() {
    let outcome = base_outcome();
    let verdict = exact("Game (Europe)");
    let audited = hashes();
    let mut reference = DatRomRef {
        game_index: 0,
        game_name: "Game (Europe)".to_string(),
        rom_index: 0,
        member_key: DatMemberKey {
            game_index: 0,
            location: MemberLocation::TopLevel { rom_index: 0 },
        },
        rom_name: "game.nes".to_string(),
        size_bytes: Some(65_536),
        checksums: Vec::new(),
        status: None,
        merge: None,
        content_classification: Default::default(),
        original_metadata: Default::default(),
        clone_of: None,
    };
    reference
        .original_metadata
        .fields
        .insert("region".to_string(), "Japan".to_string());
    let refs = vec![reference];
    let summary = summarize_library_dat_identity(&LibraryDatIdentityQuery {
        outcome: &outcome,
        verdict: &verdict,
        matched_refs: &refs,
        audited_hashes: &audited,
        current_hashes: None,
    });
    assert_eq!(summary.canonical.region.as_deref(), Some("Japan"));
}

// --- set / dependency summary ---------------------------------------

#[test]
fn set_dependency_is_pending_when_the_audit_computed_no_sets() {
    let outcome = base_outcome();
    let verdict = exact("Game");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));
    match summary.set_dependency {
        DatSetDependencySummary::Pending { reason } => {
            assert!(reason.contains("no catalogue set resolutions"))
        }
        other => panic!("expected Pending, got {other:?}"),
    }
}

#[test]
fn set_dependency_is_pending_when_no_resolved_set_matches_the_entry() {
    let mut outcome = base_outcome();
    outcome.sets.push(sample_set("A Different Game"));
    let verdict = exact("Game");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));
    match summary.set_dependency {
        DatSetDependencySummary::Pending { reason } => {
            assert!(reason.contains("not part of a multi-member"))
        }
        other => panic!("expected Pending, got {other:?}"),
    }
}

#[test]
fn a_matching_set_resolution_is_summarised_compactly() {
    let mut outcome = base_outcome();
    outcome.sets.push(sample_set("Big Arcade Game"));
    let verdict = exact("Big Arcade Game");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));
    match summary.set_dependency {
        DatSetDependencySummary::Resolved {
            set_name,
            source_id,
            state,
            members_required,
            members_verified,
            members_missing,
            dependency_state,
            ..
        } => {
            assert_eq!(set_name, "Big Arcade Game");
            assert_eq!(source_id, "no-intro-nes");
            assert_eq!(state, SetState::Incomplete);
            assert_eq!(members_required, 3);
            assert_eq!(members_verified, 2);
            assert_eq!(members_missing, 1);
            assert_eq!(dependency_state, DependencyState::NotApplicable);
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
}

#[test]
fn duplicate_set_names_are_reported_as_pending_never_resolved_to_one() {
    let mut outcome = base_outcome();
    outcome.sets.push(sample_set("Ambiguous Set"));
    outcome.sets.push(sample_set("Ambiguous Set"));
    let verdict = exact("Ambiguous Set");
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));
    assert!(matches!(
        summary.set_dependency,
        DatSetDependencySummary::Pending { .. }
    ));
}

fn sample_set(game_name: &str) -> SetResolution {
    SetResolution {
        identity: SetIdentity {
            source_id: "no-intro-nes".to_string(),
            game_name: game_name.to_string(),
        },
        archive_path: PathBuf::from("/roms/nes/set.zip"),
        state: SetState::Incomplete,
        members_required: vec!["a.rom".into(), "b.rom".into(), "c.rom".into()],
        members_verified: vec!["a.rom".into(), "b.rom".into()],
        members_bad: Vec::new(),
        members_optional: Vec::new(),
        members_borrowed: Vec::new(),
        disks_required: Vec::new(),
        disks_verified: Vec::new(),
        disks_parent_required: Vec::new(),
        dependencies: SetDependencyReport::from_requirements(Vec::new()),
    }
}

// --- optional metadata missing cleanly ------------------------------

#[test]
fn optional_catalogue_metadata_absent_is_all_none_never_fabricated() {
    let mut outcome = base_outcome();
    outcome.catalogue_version = None;
    outcome.catalogue_author = None;
    outcome.catalogue_ecosystem = None;
    outcome.catalogue_names = Vec::new();
    let verdict = exact("Game With No Region Or Revision");
    let audited = LibraryItemHashes {
        crc32: Some("abcd1234".to_string()),
        ..Default::default()
    };
    let summary = summarize_library_dat_identity(&query(&outcome, &verdict, &audited, None));

    assert_eq!(summary.source.source_revision, None);
    assert_eq!(summary.source.author, None);
    assert_eq!(summary.source.ecosystem, None);
    assert!(summary.source.catalogue_names.is_empty());
    assert_eq!(summary.canonical.region, None);
    assert_eq!(summary.canonical.revision, None);
    assert_eq!(
        summary.provenance_freshness,
        DatProvenanceFreshness::Unknown
    );
    assert!(matches!(
        summary.set_dependency,
        DatSetDependencySummary::Pending { .. }
    ));
    // Still a usable summary: it verified on SHA-1 evidence... no - only CRC32
    // was held, and the verdict says Exact/SHA-1, so matched_value is absent
    // because the item holds no SHA-1 to show. The state itself is intact.
    assert!(summary.is_verified());
    assert_eq!(
        summary.hash_evidence.matched_algorithm.as_deref(),
        Some("SHA-1")
    );
    assert_eq!(summary.hash_evidence.matched_value, None);
}

// --- entry with only a physical-file verdict (no matched_refs) --------

#[test]
fn a_flat_physical_file_verdict_without_matched_refs_still_summarises() {
    let outcome = base_outcome();
    let entry = AuditEntry {
        local_path: "/roms/nes/game.nes".to_string(),
        local_filename: "game.nes".to_string(),
        verdict: exact("Kirby's Adventure (USA)"),
    };
    let audited = hashes();
    let summary = summarize_library_dat_identity(&query(&outcome, &entry.verdict, &audited, None));
    assert!(summary.is_verified());
    assert_eq!(
        summary.canonical.canonical_dat_name.as_deref(),
        Some("Kirby's Adventure (USA)")
    );
    assert_eq!(summary.canonical.region.as_deref(), Some("USA"));
}
