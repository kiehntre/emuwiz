use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::Archive;
use crate::dat::library_identity_summary::{
    DatCanonicalIdentity, DatHashEvidenceSummary, DatSourceProvenance, DurableDatEntryRef,
    LibraryItemHashes,
};
use crate::dat::model::{DatEcosystem, DatFormat, DatGameEntry, DatRomEntry, DatSource};
use crate::database::Database;
use crate::safe_read::TrustedRoots;

use super::*;

/// SHA-1 of `b"test"` (4 bytes) - mirrors `playing_library::matching::tests`.
const SHA1_TEST: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
/// SHA-1 of `b"other"` (5 bytes) - a different file's content, used to prove
/// hash drift is caught.
const SHA1_OTHER: &str = "07e11eec7bfbdd63a92100e793ecf87071ba7f28";

struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    database: Database,
    source_folder_id: i64,
}

impl Fixture {
    fn new() -> Self {
        let root_dir = tempfile::tempdir().expect("temp dir");
        let root = root_dir.path().to_path_buf();
        let db_path = root.join("library.sqlite3");
        let mut database = Database::open_or_create(&db_path).expect("open database");
        let registered = database
            .register_source_folders(&[root.clone()])
            .expect("register source folder");
        let source_folder_id = registered[0].id;
        Self {
            _root: root_dir,
            root,
            database,
            source_folder_id,
        }
    }

    /// Writes a real file, registers it as an archive, and returns
    /// (absolute path, archive_id).
    fn seed_file(&mut self, name: &str, content: &[u8]) -> (PathBuf, i64) {
        let path = self.root.join(name);
        std::fs::write(&path, content).unwrap();
        let archive =
            Archive::from_path_in_root(&path, self.root.clone()).expect("recognised archive kind");
        let outcome = self
            .database
            .upsert_archive(self.source_folder_id, &self.root, &archive)
            .expect("upsert archive");
        (path, outcome.archive_id)
    }

    fn persist_verified(&mut self, archive_id: i64, source_id: &str, sha1: &str, game_name: &str) {
        let persisted = PersistedLibraryDatIdentity {
            verification_state: DatVerificationState::VerifiedSingleMatch {
                algorithm: "SHA-1".into(),
            },
            source: DatSourceProvenance {
                source_id: source_id.into(),
                source_name: format!("{source_id} display"),
                ecosystem: Some(DatEcosystem::NoIntro),
                variant: None,
                source_revision: Some("v1".into()),
                author: None,
                catalogue_names: vec![format!("{source_id} catalogue")],
                dat_path: format!("/dats/{source_id}.dat"),
            },
            canonical: DatCanonicalIdentity {
                canonical_dat_name: Some(game_name.into()),
                canonical_rom_name: Some(format!("{game_name}.rom")),
                region: None,
                revision: None,
            },
            hash_evidence: DatHashEvidenceSummary {
                matched_algorithm: Some("SHA-1".into()),
                matched_value: Some(sha1.into()),
                available_algorithms: vec!["SHA-1".into()],
            },
            ambiguous_candidates: Vec::new(),
            candidate_provenance: Vec::new(),
            matched_entries: vec![DurableDatEntryRef {
                source_id: source_id.into(),
                game_name: game_name.into(),
                rom_name: Some(format!("{game_name}.rom")),
                checksums: vec![("SHA-1".into(), sha1.into())],
            }],
            audited_hashes: LibraryItemHashes {
                size_bytes: None,
                crc32: None,
                md5: None,
                sha1: Some(sha1.into()),
                sha256: None,
            },
            audited_at: "2026-06-01T00:00:00Z".into(),
            completeness: crate::dat::library_identity_summary::DatAuditCompleteness::Exhaustive,
        };
        self.database
            .persist_library_dat_identity(archive_id, &persisted)
            .expect("persist identity");
    }
}

fn dat_with_one_rom(sha1: &str, game_name: &str) -> ParsedDat {
    ParsedDat {
        source: DatSource {
            format: DatFormat::Logiqx,
            ecosystem: DatEcosystem::NoIntro,
            file_path: "synthetic.dat".to_string(),
            name: Some("Synthetic".to_string()),
            description: None,
            version: None,
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: 1,
            rom_count: 1,
            parse_warnings: Vec::new(),
            packing_policy: crate::dat::model::DatPackingPolicy::Standard,
        },
        games: vec![DatGameEntry {
            name: game_name.to_string(),
            roms: vec![DatRomEntry {
                name: format!("{game_name}.rom"),
                size_bytes: Some(4),
                sha1: Some(sha1.to_string()),
                ..DatRomEntry::default()
            }],
            ..DatGameEntry::default()
        }],
    }
}

#[test]
fn a_fresh_verified_match_is_bridged_without_any_fallback_hashing_decision() {
    let mut fixture = Fixture::new();
    let (path, archive_id) = fixture.seed_file("game.iso", b"test");
    fixture.persist_verified(archive_id, "no-intro-test", SHA1_TEST, "Game (World)");
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1);
    assert_eq!(outcome.matches[0].archive_path, path);
    assert_eq!(outcome.matches[0].dat_entry_index, 0);
    assert!(outcome.matches[0].companion_paths.is_empty());
    assert!(outcome.needs_fallback.is_empty());
    assert!(outcome.rejected_launchers.is_empty());
}

#[test]
fn a_file_never_scanned_needs_fallback() {
    let fixture = Fixture::new();
    let missing = fixture.root.join("never-scanned.bin");
    std::fs::write(&missing, b"test").unwrap();
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[missing.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(outcome.needs_fallback.len(), 1);
    assert_eq!(outcome.needs_fallback[0].0, missing);
    assert_eq!(
        outcome.needs_fallback[0].1,
        BridgeSkipReason::NoArchiveRecord
    );
}

#[test]
fn a_scanned_file_with_no_persisted_identity_needs_fallback() {
    let mut fixture = Fixture::new();
    let (path, _archive_id) = fixture.seed_file("game.iso", b"test");
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(
        outcome.needs_fallback,
        vec![(path, BridgeSkipReason::NoPersistedIdentity)]
    );
}

#[test]
fn source_file_mutation_invalidates_the_persisted_match() {
    let mut fixture = Fixture::new();
    let (path, archive_id) = fixture.seed_file("game.iso", b"test");
    fixture.persist_verified(archive_id, "no-intro-test", SHA1_TEST, "Game (World)");
    // The file on disk now differs from what was audited.
    std::fs::write(&path, b"other").unwrap();
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert!(
        outcome.matches.is_empty(),
        "a mutated source file must never be silently trusted"
    );
    assert_eq!(
        outcome.needs_fallback,
        vec![(path, BridgeSkipReason::HashDrifted)]
    );
    // Sanity: the "other" content really does hash differently, proving this
    // is a genuine drift detection and not an accidental match.
    assert_ne!(SHA1_TEST, SHA1_OTHER);
}

#[test]
fn same_path_replaced_with_different_content_is_treated_as_drift_not_reuse() {
    let mut fixture = Fixture::new();
    let (path, archive_id) = fixture.seed_file("game.iso", b"test");
    fixture.persist_verified(archive_id, "no-intro-test", SHA1_TEST, "Game (World)");
    // Simulate the same path now pointing at an entirely different object.
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"other").unwrap();
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(
        outcome.needs_fallback,
        vec![(path, BridgeSkipReason::HashDrifted)]
    );
}

#[test]
fn a_catalogue_no_longer_containing_the_hash_needs_fallback() {
    let mut fixture = Fixture::new();
    let (path, archive_id) = fixture.seed_file("game.iso", b"test");
    fixture.persist_verified(archive_id, "no-intro-test", SHA1_TEST, "Game (World)");
    // The freshly loaded catalogue no longer has this entry at all (e.g. a
    // new DAT revision removed/renamed it).
    let dat = dat_with_one_rom(SHA1_OTHER, "A Different Game");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(
        outcome.needs_fallback,
        vec![(path, BridgeSkipReason::NoLongerUniqueInCatalogue)]
    );
}

#[test]
fn ambiguous_persisted_evidence_is_never_silently_resolved() {
    let mut fixture = Fixture::new();
    let (path, archive_id) = fixture.seed_file("game.iso", b"test");
    let mut persisted_source = DatSourceProvenance {
        source_id: "no-intro-test".into(),
        source_name: "display".into(),
        ecosystem: Some(DatEcosystem::NoIntro),
        variant: None,
        source_revision: Some("v1".into()),
        author: None,
        catalogue_names: vec!["catalogue".into()],
        dat_path: "/dats/no-intro-test.dat".into(),
    };
    persisted_source.source_id = "no-intro-test".into();
    // A defensive-consistency case: the verdict itself claims a strong,
    // single cryptographic match, but the row *also* still carries
    // competing candidate names (e.g. from stale data, or a future writer
    // that populates both fields together). The bridge must never trust
    // `VerifiedSingleMatch` alone - a non-empty `ambiguous_candidates` list
    // is its own independent refusal.
    let persisted = PersistedLibraryDatIdentity {
        verification_state: DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        },
        source: persisted_source,
        canonical: DatCanonicalIdentity::default(),
        hash_evidence: DatHashEvidenceSummary {
            matched_algorithm: Some("SHA-1".into()),
            matched_value: Some(SHA1_TEST.into()),
            available_algorithms: vec!["SHA-1".into()],
        },
        ambiguous_candidates: vec!["Game (USA)".into(), "Game (Europe)".into()],
        candidate_provenance: Vec::new(),
        matched_entries: Vec::new(),
        audited_hashes: LibraryItemHashes {
            size_bytes: None,
            crc32: None,
            md5: None,
            sha1: Some(SHA1_TEST.into()),
            sha256: None,
        },
        audited_at: "2026-06-01T00:00:00Z".into(),
        completeness: crate::dat::library_identity_summary::DatAuditCompleteness::Exhaustive,
    };
    fixture
        .database
        .persist_library_dat_identity(archive_id, &persisted)
        .expect("persist ambiguous identity");
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(
        outcome.needs_fallback,
        vec![(path, BridgeSkipReason::AmbiguousCandidates)]
    );
}

#[test]
fn a_probable_only_verdict_is_not_strong_enough_to_bridge() {
    let mut fixture = Fixture::new();
    let (path, archive_id) = fixture.seed_file("game.iso", b"test");
    let persisted = PersistedLibraryDatIdentity {
        verification_state: DatVerificationState::Probable,
        source: DatSourceProvenance {
            source_id: "no-intro-test".into(),
            source_name: "display".into(),
            ecosystem: Some(DatEcosystem::NoIntro),
            variant: None,
            source_revision: Some("v1".into()),
            author: None,
            catalogue_names: vec!["catalogue".into()],
            dat_path: "/dats/no-intro-test.dat".into(),
        },
        canonical: DatCanonicalIdentity {
            canonical_dat_name: Some("Game (World)".into()),
            canonical_rom_name: Some("Game (World).rom".into()),
            region: None,
            revision: None,
        },
        hash_evidence: DatHashEvidenceSummary {
            matched_algorithm: Some("CRC32".into()),
            matched_value: Some("deadbeef".into()),
            available_algorithms: vec!["CRC32".into()],
        },
        ambiguous_candidates: Vec::new(),
        candidate_provenance: Vec::new(),
        matched_entries: Vec::new(),
        audited_hashes: LibraryItemHashes {
            size_bytes: None,
            crc32: Some("deadbeef".into()),
            md5: None,
            sha1: Some(SHA1_TEST.into()),
            sha256: None,
        },
        audited_at: "2026-06-01T00:00:00Z".into(),
        completeness: crate::dat::library_identity_summary::DatAuditCompleteness::Exhaustive,
    };
    fixture
        .database
        .persist_library_dat_identity(archive_id, &persisted)
        .expect("persist probable identity");
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(
        outcome.needs_fallback,
        vec![(path, BridgeSkipReason::NotStronglyVerified)]
    );
}

#[test]
fn cue_bin_grouping_survives_persistence_and_reconstructs_the_combined_match() {
    let mut fixture = Fixture::new();
    let (bin, archive_id) = fixture.seed_file("game.iso", b"test");
    fixture.persist_verified(archive_id, "no-intro-test", SHA1_TEST, "Game (World)");
    let cue_path = fixture.root.join("game.cue");
    std::fs::write(&cue_path, "FILE \"game.iso\" BINARY\n").unwrap();
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[bin.clone(), cue_path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1, "{:?}", outcome.matches);
    assert_eq!(outcome.matches[0].archive_path, cue_path);
    assert_eq!(outcome.matches[0].companion_paths, vec![bin]);
    assert!(outcome.needs_fallback.is_empty());
}

#[test]
fn a_dat_source_id_mismatch_is_treated_as_no_persisted_identity() {
    let mut fixture = Fixture::new();
    let (path, archive_id) = fixture.seed_file("game.iso", b"test");
    // Persisted against a different DAT source than the one now selected.
    fixture.persist_verified(archive_id, "redump-test", SHA1_TEST, "Game (World)");
    let dat = dat_with_one_rom(SHA1_TEST, "Game (World)");

    let outcome = bridge_verified_evidence(
        &fixture.database,
        "no-intro-test",
        &dat,
        &[path.clone()],
        &TrustedRoots::from_paths([fixture.root.as_path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(
        outcome.needs_fallback,
        vec![(path, BridgeSkipReason::NoPersistedIdentity)]
    );
}
