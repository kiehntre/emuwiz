//! Production-path proof that a safe, exact archive-member match reaches the
//! parent library archive's `library_dat_identities` row: real temp
//! ZIP/7z files -> `scan_and_persist` (library rows) -> `run_dat_audit` ->
//! `persist_library_dat_identities_from_audit` -> temp DB query. Nothing here
//! touches a real database or real games.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sha1::{Digest, Sha1};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::dat::library_identity_summary::{
    DatProvenanceFreshness, DatVerificationState, LibraryItemHashes, PersistedLibraryDatIdentity,
    SourceFreshnessContext,
};
use crate::dat::limits::DatLimits;
use crate::dat::sources::DatSourceKind;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{DatAuditOutcome, DatAuditRequest, run_dat_audit_with_cache};
use crate::safe_read::TrustedRoots;
use crate::{Config, Database, scan_and_persist};

const SOURCE_ID: &str = "no-intro-test";

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `(game, rom, bytes)` -> full-hash DAT entry; `crc_only` entries carry no
/// sha1/md5 so the best possible verdict is Probable.
enum Dat<'a> {
    Full(&'a str, &'a str, &'a [u8]),
    CrcOnly(&'a str, &'a str, &'a [u8]),
}

fn crc32_hex(bytes: &[u8]) -> String {
    crate::identity_source::hashing::Crc32::of(bytes).to_string()
}

fn dat_xml(entries: &[Dat<'_>]) -> String {
    let games: String = entries
        .iter()
        .map(|entry| match entry {
            Dat::Full(game, rom, bytes) => format!(
                r#"<game name="{game}"><rom name="{rom}" size="{}" sha1="{}"/></game>"#,
                bytes.len(),
                sha1_hex(bytes)
            ),
            Dat::CrcOnly(game, rom, bytes) => format!(
                r#"<game name="{game}"><rom name="{rom}" size="{}" crc="{}"/></game>"#,
                bytes.len(),
                crc32_hex(bytes)
            ),
        })
        .collect();
    format!(
        r#"<?xml version="1.0"?><datafile><header><name>Fixture</name><version>20240501</version><author>No-Intro</author></header>{games}</datafile>"#
    )
}

fn write_zip(path: &Path, members: &[(&str, &[u8])]) {
    let mut writer = ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, bytes) in members {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

fn write_7z(path: &Path, members: &[(&str, &[u8])]) {
    let mut writer =
        sevenz_rust2::ArchiveWriter::new(std::fs::File::create(path).unwrap()).unwrap();
    for (name, bytes) in members {
        let mut entry = sevenz_rust2::ArchiveEntry::new();
        entry.name = name.to_string();
        entry.has_stream = true;
        entry.size = bytes.len() as u64;
        writer
            .push_archive_entry(entry, Some(std::io::Cursor::new(*bytes)))
            .unwrap();
    }
    writer.finish().unwrap();
}

#[derive(Clone, Copy)]
enum Format {
    Zip,
    SevenZ,
}

impl Format {
    fn file_name(self) -> &'static str {
        match self {
            Self::Zip => "Game.zip",
            Self::SevenZ => "Game.7z",
        }
    }
    fn write(self, path: &Path, members: &[(&str, &[u8])]) {
        match self {
            Self::Zip => write_zip(path, members),
            Self::SevenZ => write_7z(path, members),
        }
    }
}

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    archive: PathBuf,
    database: Database,
    dat_path: PathBuf,
}

impl Fixture {
    fn new(format: Format, members: &[(&str, &[u8])], dat: &[Dat<'_>]) -> Self {
        let root = tempfile::tempdir().unwrap().keep();
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let archive = source.join(format.file_name());
        format.write(&archive, members);
        let dat_path = root.join("fixture.dat");
        std::fs::write(&dat_path, dat_xml(dat)).unwrap();
        let mut database = Database::open_or_create(root.join("library.sqlite3")).unwrap();
        Self::scan(&mut database, &source, &root);
        Self {
            root,
            source,
            archive,
            database,
            dat_path,
        }
    }

    fn scan(database: &mut Database, source: &Path, root: &Path) {
        let config = Config {
            source_folders: vec![source.to_path_buf()],
            mount_root: root.join("mount"),
            ratarmount_bin: "ratarmount".to_string(),
            master_rom_root: None,
        };
        scan_and_persist(database, &config, "test").unwrap();
    }

    fn audit(&self) -> DatAuditOutcome {
        run_dat_audit_with_cache(
            &DatAuditRequest {
                source_id: SOURCE_ID.to_string(),
                source_display_name: "No-Intro Test".to_string(),
                dat_path: self.dat_path.clone(),
                dat_kind: DatSourceKind::File,
                scan_root: self.source.clone(),
                limits: DatLimits::default(),
                policy: None,
                platform: None,
            },
            &TrustedRoots::none(),
            &AtomicBool::new(false),
            &|_| {},
            AuditCacheConfig::Disabled,
        )
        .unwrap()
    }

    fn archive_id(&self) -> i64 {
        self.database
            .find_archive_id_by_absolute_path(&self.archive)
            .unwrap()
            .expect("library row for the outer archive")
    }

    /// Audit + persist, then the parent archive's stored row (if any).
    fn run(&mut self) -> (DatAuditOutcome, Option<PersistedLibraryDatIdentity>) {
        let outcome = self.audit();
        self.database
            .persist_library_dat_identities_from_audit(&outcome)
            .unwrap();
        let row = self
            .database
            .library_dat_identity_for_item(self.archive_id(), SOURCE_ID)
            .unwrap();
        (outcome, row)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn is_verified(row: &Option<PersistedLibraryDatIdentity>) -> bool {
    matches!(
        row.as_ref().map(|row| &row.verification_state),
        Some(DatVerificationState::VerifiedSingleMatch { .. })
    )
}

const GAME: &[u8] = b"game-a rom bytes";
const OTHER: &[u8] = b"game-b rom bytes";

fn assert_verified_parent(row: &PersistedLibraryDatIdentity, game: &str, rom: &str) {
    assert_eq!(
        row.verification_state,
        DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".to_string()
        }
    );
    assert_eq!(row.source.source_id, SOURCE_ID);
    assert_eq!(row.source.source_revision.as_deref(), Some("20240501"));
    assert_eq!(row.canonical.canonical_dat_name.as_deref(), Some(game));
    assert_eq!(row.canonical.canonical_rom_name.as_deref(), Some(rom));
    assert_eq!(row.hash_evidence.matched_value, Some(sha1_hex(GAME)));
    let member = row.archive_member.as_ref().expect("member provenance");
    assert_eq!(member.member_name, rom);
    assert_eq!(member.algorithm, "SHA-1");
}

// ---- one exact member --------------------------------------------------

fn one_exact(format: Format) {
    let mut fx = Fixture::new(
        format,
        &[("Game A.sfc", GAME)],
        &[Dat::Full("Game A (USA)", "Game A.sfc", GAME)],
    );
    let before = std::fs::read(&fx.archive).unwrap();
    let (outcome, row) = fx.run();
    assert_eq!(outcome.archives.len(), 1);
    assert_verified_parent(&row.expect("parent row"), "Game A (USA)", "Game A.sfc");
    // Safety: archive untouched, nothing extracted beside it.
    assert_eq!(std::fs::read(&fx.archive).unwrap(), before);
    assert_eq!(std::fs::read_dir(&fx.source).unwrap().count(), 1);
}

#[test]
fn zip_one_exact_member_verifies_the_parent() {
    one_exact(Format::Zip);
}

#[test]
fn sevenz_one_exact_member_verifies_the_parent() {
    one_exact(Format::SevenZ);
}

// ---- ancillary members -------------------------------------------------

fn ancillary(format: Format) {
    let mut fx = Fixture::new(
        format,
        &[("Game A.sfc", GAME), ("readme.txt", b"hello")],
        &[Dat::Full("Game A (USA)", "Game A.sfc", GAME)],
    );
    let (_, row) = fx.run();
    assert_verified_parent(&row.unwrap(), "Game A (USA)", "Game A.sfc");
}

#[test]
fn zip_ancillary_text_member_is_tolerated() {
    ancillary(Format::Zip);
}

#[test]
fn sevenz_ancillary_text_member_is_tolerated() {
    ancillary(Format::SevenZ);
}

// ---- must NOT verify ---------------------------------------------------

fn two_different_games(format: Format) {
    let mut fx = Fixture::new(
        format,
        &[("Game A.sfc", GAME), ("Game B.sfc", OTHER)],
        &[
            Dat::Full("Game A (USA)", "Game A.sfc", GAME),
            Dat::Full("Game B (USA)", "Game B.sfc", OTHER),
        ],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row), "collection must not verify as one game");
}

#[test]
fn zip_two_different_exact_games_are_not_verified() {
    two_different_games(Format::Zip);
}

#[test]
fn sevenz_two_different_exact_games_are_not_verified() {
    two_different_games(Format::SevenZ);
}

#[test]
fn unknown_second_rom_member_is_not_treated_as_ancillary() {
    let mut fx = Fixture::new(
        Format::Zip,
        &[("Game A.sfc", GAME), ("Mystery.sfc", OTHER)],
        &[Dat::Full("Game A (USA)", "Game A.sfc", GAME)],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

fn weak(format: Format) {
    let mut fx = Fixture::new(
        format,
        &[("Game A.sfc", GAME)],
        &[Dat::CrcOnly("Game A (USA)", "Game A.sfc", GAME)],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row), "CRC-only member match is probable");
}

#[test]
fn zip_probable_member_is_not_verified() {
    weak(Format::Zip);
}

#[test]
fn sevenz_probable_member_is_not_verified() {
    weak(Format::SevenZ);
}

#[test]
fn zip_ambiguous_member_is_not_verified() {
    let mut fx = Fixture::new(
        Format::Zip,
        &[("Game A.sfc", GAME)],
        &[
            Dat::Full("Game A (USA)", "Game A.sfc", GAME),
            Dat::Full("Game A (Europe)", "Game A.sfc", GAME),
        ],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

#[test]
fn zip_no_dat_match_is_not_verified() {
    let mut fx = Fixture::new(
        Format::Zip,
        &[("Game A.sfc", GAME)],
        &[Dat::Full("Other (USA)", "Other.sfc", OTHER)],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

#[test]
fn corrupt_zip_fails_safely() {
    let mut fx = Fixture::new(
        Format::Zip,
        &[("Game A.sfc", GAME)],
        &[Dat::Full("Game A (USA)", "Game A.sfc", GAME)],
    );
    std::fs::write(&fx.archive, b"PK\x03\x04 definitely not a zip").unwrap();
    Fixture::scan(&mut fx.database, &fx.source.clone(), &fx.root.clone());
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

#[test]
fn zip_with_traversal_member_name_is_not_verified() {
    let mut fx = Fixture::new(
        Format::Zip,
        &[("../Game A.sfc", GAME)],
        &[Dat::Full("Game A (USA)", "Game A.sfc", GAME)],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

#[test]
fn nested_archive_member_is_not_verified() {
    let mut fx = Fixture::new(
        Format::Zip,
        &[("Game A.sfc", GAME), ("inner.zip", b"PK nested")],
        &[Dat::Full("Game A (USA)", "Game A.sfc", GAME)],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

// ---- freshness ----------------------------------------------------------

fn changed_archive_is_stale(format: Format) {
    let mut fx = Fixture::new(
        format,
        &[("Game A.sfc", GAME)],
        &[Dat::Full("Game A (USA)", "Game A.sfc", GAME)],
    );
    let (first, row) = fx.run();
    let row = row.expect("verified");
    assert!(is_verified(&Some(row.clone())));
    let id = fx.archive_id();

    // Replace the container's contents; the library now knows new hashes.
    format.write(&fx.archive, &[("Game A.sfc", b"tampered bytes")]);
    let after = fx.audit();
    let current = |outcome: &DatAuditOutcome| {
        let hashes = &outcome.known_hashes[&fx.archive.display().to_string()];
        LibraryItemHashes {
            size_bytes: hashes.size_bytes,
            crc32: hashes.crc32.clone(),
            md5: hashes.md5.clone(),
            sha1: hashes.sha1.clone(),
            sha256: hashes.sha256.clone(),
        }
    };
    // Unchanged container: current. Changed container: the stored identity
    // must not be accepted as current.
    let context = SourceFreshnessContext {
        current_source_revision: Some("20240501"),
        source_available: true,
        revision_marked_stale: false,
    };
    assert_eq!(
        row.reconstruct_summary(Some(&current(&first)), context)
            .provenance_freshness,
        DatProvenanceFreshness::Current
    );
    assert_eq!(
        row.reconstruct_summary(Some(&current(&after)), context)
            .provenance_freshness,
        DatProvenanceFreshness::Stale
    );

    // A DAT revision bump marks the compressed identity stale like any other.
    fx.database
        .mark_library_dat_identity_stale_for_source_revision(SOURCE_ID, Some("20250101"))
        .unwrap();
    let summary = fx
        .database
        .library_dat_identity_summary_for_item(id, SOURCE_ID, Some(&current(&first)), None, true)
        .unwrap()
        .unwrap();
    assert_eq!(summary.provenance_freshness, DatProvenanceFreshness::Stale);

    // Re-auditing the changed container replaces the old verified row.
    fx.database
        .persist_library_dat_identities_from_audit(&after)
        .unwrap();
    let refreshed = fx
        .database
        .library_dat_identity_for_item(id, SOURCE_ID)
        .unwrap();
    assert!(!is_verified(&refreshed));
}

#[test]
fn zip_changed_archive_does_not_stay_current() {
    changed_archive_is_stale(Format::Zip);
}

#[test]
fn sevenz_changed_archive_does_not_stay_current() {
    changed_archive_is_stale(Format::SevenZ);
}

#[test]
fn sevenz_conflicting_and_weak_members_are_not_verified() {
    let mut fx = Fixture::new(
        Format::SevenZ,
        &[("Game A.sfc", GAME), ("Game B.sfc", OTHER)],
        &[
            Dat::Full("Game A (USA)", "Game A.sfc", GAME),
            Dat::CrcOnly("Game B (USA)", "Game B.sfc", OTHER),
        ],
    );
    let (_, row) = fx.run();
    assert!(
        !is_verified(&row),
        "exact + probable members stay unverified"
    );
}
