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
    FilenameOnly(&'a str, &'a str, usize),
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
            Dat::FilenameOnly(game, rom, size) => {
                format!(r#"<game name="{game}"><rom name="{rom}" size="{size}"/></game>"#)
            }
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
    Lha,
    Rar,
    RarEncrypted,
}

impl Format {
    fn file_name(self) -> &'static str {
        match self {
            Self::Zip => "Game.zip",
            Self::SevenZ => "Game.7z",
            Self::Lha => "Game.lha",
            Self::Rar => "Game.rar",
            Self::RarEncrypted => "Encrypted.rar",
        }
    }
    fn write(self, path: &Path, members: &[(&str, &[u8])]) {
        match self {
            Self::Zip => write_zip(path, members),
            Self::SevenZ => write_7z(path, members),
            Self::Lha => write_lha(path, members),
            Self::Rar | Self::RarEncrypted => {
                let fixture = if matches!(self, Self::RarEncrypted) {
                    "test_read_format_rar5_encrypted.rar"
                } else {
                    "test_read_format_rar5_stored.rar"
                };
                std::fs::copy(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/rar")
                        .join(fixture),
                    path,
                )
                .unwrap();
            }
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
            .find_archive_id_by_absolute_path(&self.archive)
            .unwrap()
            .and_then(|id| {
                self.database
                    .library_dat_identity_for_item(id, SOURCE_ID)
                    .unwrap()
            });
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

fn assert_verified_rar_parent(row: &PersistedLibraryDatIdentity, game: &str, rom: &str) {
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
    assert_eq!(row.hash_evidence.matched_value, Some(sha1_hex(RAR_BYTES)));
    let member = row.archive_member.as_ref().expect("RAR member provenance");
    assert_eq!(member.member_name, "helloworld.txt");
    assert_eq!(member.archive_format, "rar");
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

const RAR_BYTES: &[u8] = b"hello libarchive test suite!\n";

#[test]
fn rar_exact_checksum_with_matching_name_verifies_parent() {
    let mut fx = Fixture::new(
        Format::Rar,
        &[],
        &[Dat::Full("RAR Game", "helloworld.txt", RAR_BYTES)],
    );
    let before = std::fs::read(&fx.archive).unwrap();
    let (outcome, row) = fx.run();
    let row = row.expect("RAR parent identity");
    assert_verified_rar_parent(&row, "RAR Game", "helloworld.txt");
    assert_eq!(
        row.audited_hashes.sha1.as_deref(),
        Some(sha1_hex(&before).as_str())
    );
    assert_eq!(std::fs::read(&fx.archive).unwrap(), before);
    assert!(outcome.archives[0].outer_identity.is_some());
}

#[test]
fn rar_exact_checksum_outranks_different_member_name() {
    let mut fx = Fixture::new(
        Format::Rar,
        &[],
        &[Dat::Full("RAR Game", "Different DAT Name.rom", RAR_BYTES)],
    );
    let (_, row) = fx.run();
    assert_verified_rar_parent(
        &row.expect("checksum is authoritative"),
        "RAR Game",
        "Different DAT Name.rom",
    );
}

#[test]
fn rar_filename_match_with_wrong_checksum_does_not_verify() {
    let mut fx = Fixture::new(
        Format::Rar,
        &[],
        &[Dat::Full("RAR Game", "helloworld.txt", b"different bytes")],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

#[test]
fn rar_filename_only_dat_entry_does_not_verify() {
    let mut fx = Fixture::new(
        Format::Rar,
        &[],
        &[Dat::FilenameOnly(
            "RAR Game",
            "helloworld.txt",
            RAR_BYTES.len(),
        )],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

#[test]
fn rar_probable_and_no_match_evidence_do_not_verify() {
    for dat in [
        Dat::CrcOnly("RAR Game", "helloworld.txt", RAR_BYTES),
        Dat::Full("Other Game", "other.rom", b"unrelated"),
    ] {
        let mut fx = Fixture::new(Format::Rar, &[], &[dat]);
        let (_, row) = fx.run();
        assert!(!is_verified(&row));
    }
}

#[test]
fn rar_conflicting_exact_identities_do_not_verify() {
    let mut fx = Fixture::new(
        Format::Rar,
        &[],
        &[
            Dat::Full("RAR Game A", "helloworld.txt", RAR_BYTES),
            Dat::Full("RAR Game B", "helloworld.txt", RAR_BYTES),
        ],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

#[test]
fn rar_encrypted_member_never_verifies_parent() {
    let mut fx = Fixture::new(
        Format::RarEncrypted,
        &[],
        &[Dat::Full("RAR Game", "helloworld.txt", RAR_BYTES)],
    );
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
}

#[test]
fn rar_corrupt_backend_read_never_verifies_parent() {
    let mut fx = Fixture::new(
        Format::Rar,
        &[],
        &[Dat::Full("RAR Game", "helloworld.txt", RAR_BYTES)],
    );
    std::fs::write(&fx.archive, b"not a RAR archive").unwrap();
    Fixture::scan(&mut fx.database, &fx.source.clone(), &fx.root.clone());
    let (_, row) = fx.run();
    assert!(!is_verified(&row));
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
    let (members, dat) = if matches!(format, Format::Rar) {
        (
            &[][..],
            vec![Dat::Full("RAR Game", "helloworld.txt", RAR_BYTES)],
        )
    } else {
        (
            &[(&"Game A.sfc"[..], GAME)][..],
            vec![Dat::Full("Game A (USA)", "Game A.sfc", GAME)],
        )
    };
    let mut fx = Fixture::new(format, members, &dat);
    let (first, row) = fx.run();
    let row = row.expect("verified");
    assert!(is_verified(&Some(row.clone())));
    let id = fx.archive_id();

    // Replace the container's contents; the library now knows new hashes.
    if matches!(format, Format::Rar) {
        let mut bytes = std::fs::read(&fx.archive).unwrap();
        let offset = bytes
            .windows(RAR_BYTES.len())
            .position(|window| window == RAR_BYTES)
            .unwrap();
        bytes[offset] ^= 1;
        std::fs::write(&fx.archive, bytes).unwrap();
    } else {
        format.write(&fx.archive, &[("Game A.sfc", b"tampered bytes")]);
    }
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

    // A failed member re-read need not erase historical evidence, but the
    // changed outer hash makes that stored verification stale at query time.
    let refreshed = fx
        .database
        .library_dat_identity_summary_for_item(id, SOURCE_ID, Some(&current(&after)), None, true)
        .unwrap()
        .unwrap();
    assert_eq!(
        refreshed.provenance_freshness,
        DatProvenanceFreshness::Stale
    );
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
fn rar_changed_archive_does_not_stay_current() {
    changed_archive_is_stale(Format::Rar);
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

// Stored level-0 fixture writer; no new production parser or extraction path.
fn write_lha(path: &Path, members: &[(&str, &[u8])]) {
    let mut archive = Vec::new();
    for (name, payload) in members {
        assert!(name.len() + 23 <= 255);
        let mut crc = 0_u16;
        for byte in *payload {
            crc ^= u16::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xa001
                } else {
                    crc >> 1
                };
            }
        }
        let mut header = vec![(name.len() + 23) as u8, 0];
        header.extend_from_slice(b"-lh0-");
        header.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        header.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        header.extend_from_slice(&0_u32.to_le_bytes());
        header.extend_from_slice(&[0x20, 0, name.len() as u8]);
        header.extend_from_slice(name.as_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        header.push(0);
        header[1] = header[2..]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        archive.extend_from_slice(&header);
        archive.extend_from_slice(payload);
    }
    archive.push(0);
    std::fs::write(path, archive).unwrap();
}

#[test]
fn rar_parent_projection_and_lha_disabled_production_baseline() {
    if !external_readers_available() {
        return;
    }
    // The checked-in stored RAR fixture contains these exact deterministic bytes.
    let rar_bytes = b"hello libarchive test suite!\n";
    for (format, name, bytes) in [
        (Format::Rar, "helloworld.txt", rar_bytes.as_slice()),
        (Format::Lha, "Game.rom", GAME),
    ] {
        let mut fx = Fixture::new(
            format,
            &[(name, bytes)],
            &[Dat::Full("Game A", name, bytes)],
        );
        let before = std::fs::read(&fx.archive).unwrap();
        let (outcome, row) = fx.run();
        eprintln!(
            "BASELINE {} outer={:?} member={:?} completion={:?} parent_id={:?} identity={:?}",
            fx.archive.display(),
            outcome.report.entries.first().map(|entry| &entry.verdict),
            outcome.archives[0].members,
            outcome.archives[0].completion,
            fx.database
                .find_archive_id_by_absolute_path(&fx.archive)
                .unwrap(),
            row
        );
        assert!(matches!(
            outcome.archives[0].members[0].verdict,
            Some(crate::dat::audit::AuditVerdict::Exact { .. })
        ));
        if matches!(format, Format::Rar) {
            assert_verified_rar_parent(row.as_ref().unwrap(), "Game A", name);
            assert!(outcome.archives[0].outer_identity.is_some());
        } else {
            assert!(!is_verified(&row), "LHA projection remains disabled");
        }
        assert_eq!(std::fs::read(&fx.archive).unwrap(), before);
        assert_eq!(std::fs::read_dir(&fx.source).unwrap().count(), 1);
    }
}

#[test]
fn rar_checksum_match_with_different_name_reaches_parent_projection() {
    if !external_readers_available() {
        return;
    }
    let bytes = b"hello libarchive test suite!\n";
    let mut fx = Fixture::new(
        Format::Rar,
        &[],
        &[Dat::Full("Game A", "renamed.rom", bytes)],
    );
    let (outcome, row) = fx.run();
    let member = &outcome.archives[0].members[0];
    assert_eq!(member.evidence.member_name_display, "helloworld.txt");
    assert_eq!(
        member.evidence.status,
        crate::dat::archive::ArchiveMemberStatus::HashComplete
    );
    assert!(member.evidence.hashes.is_some());
    assert!(matches!(
        member.verdict,
        Some(crate::dat::audit::AuditVerdict::Exact { .. })
    ));
    assert_verified_rar_parent(row.as_ref().unwrap(), "Game A", "renamed.rom");
}

#[test]
fn lha_unix_symlink_is_refused_as_authoritative_member_evidence() {
    if !external_readers_available() {
        return;
    }
    // Level-0 Unix extended header: host U, minor version, mtime, mode,
    // uid, gid. LHA symlink names use "name|target". Independent read-only
    // libarchive inspection of this fixture reports S_IFLNK, target "target";
    // 7-Zip 23.01 instead reports Folder=-, Host OS=MS-DOS, no link/type facts
    // and streams the link text. The raw-header classifier now proves the
    // type, so the member must never become authoritative ROM evidence.
    let name = "Game.rom|target";
    let mut fx = Fixture::new(
        Format::Lha,
        &[(name, GAME)],
        &[Dat::Full("Game A", name, GAME)],
    );
    let bytes = std::fs::read(&fx.archive).unwrap();
    let header_end = 2 + usize::from(bytes[0]);
    let mut header = bytes[..header_end].to_vec();
    *header.last_mut().unwrap() = b'U';
    header.push(0); // Unix minor version
    header.extend_from_slice(&0_u32.to_le_bytes());
    header.extend_from_slice(&0o120777_u16.to_le_bytes());
    header.extend_from_slice(&0_u16.to_le_bytes());
    header.extend_from_slice(&0_u16.to_le_bytes());
    header[0] = (header.len() - 2) as u8;
    header[1] = header[2..]
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
    header.extend_from_slice(&bytes[header_end..]);
    std::fs::write(&fx.archive, &header).unwrap();
    let (outcome, row) = fx.run();
    eprintln!(
        "LHA SPECIAL ENTRY member={:?} completion={:?} parent_id={:?} identity={:?}",
        outcome.archives[0].members,
        outcome.archives[0].completion,
        fx.database
            .find_archive_id_by_absolute_path(&fx.archive)
            .unwrap(),
        row
    );
    let member = &outcome.archives[0].members[0];
    assert!(matches!(
        member.evidence.status,
        crate::dat::archive::ArchiveMemberStatus::NotVerified {
            reason: "LHA symbolic-link member"
        }
    ));
    assert!(member.evidence.hashes.is_none());
    assert!(member.verdict.is_none());
    assert!(member.matched_refs.is_empty());
    assert!(!is_verified(&row));
    assert!(row.is_none(), "no parent row, no verified_single_match");
    assert_eq!(std::fs::read(&fx.archive).unwrap(), header);
    assert_eq!(std::fs::read_dir(&fx.source).unwrap().count(), 1);
}

/// Amiga Lha 40.x writes links as `-lhd-` + extended header 0x60/0x61 (see
/// `dat::archive::lha_header`).  Even a link whose stored bytes equal the DAT
/// ROM must never become verified identity.
#[test]
fn lha_amiga_links_are_never_authoritative_rom_evidence() {
    use crate::dat::archive::lha_header::fixtures::{Entry, archive};
    if !external_readers_available() {
        return;
    }
    for marker in [0x60_u8, 0x61] {
        for disguised in [false, true] {
            let mut fx = Fixture::new(
                Format::Lha,
                &[("Game.rom", GAME)],
                &[Dat::Full("Game A", "Game.rom", GAME)],
            );
            let mut link = Entry::amiga_link("Game.rom", marker, "Other.rom").level(1);
            if disguised {
                // Ordinary method carrying exactly the DAT's ROM bytes.
                link.method = *b"-lh0-";
                link.payload = GAME.to_vec();
            }
            let bytes = archive(&[link]);
            std::fs::write(&fx.archive, &bytes).unwrap();
            let (outcome, row) = fx.run();
            let member = &outcome.archives[0].members[0];
            assert!(
                matches!(
                    member.evidence.status,
                    crate::dat::archive::ArchiveMemberStatus::NotVerified { .. }
                ),
                "marker {marker:#x} disguised={disguised}: {:?}",
                member.evidence.status
            );
            assert!(member.evidence.hashes.is_none());
            assert!(member.verdict.is_none());
            assert!(!is_verified(&row));
            assert!(row.is_none());
            assert_eq!(std::fs::read(&fx.archive).unwrap(), bytes);
            assert_eq!(std::fs::read_dir(&fx.source).unwrap().count(), 1);
        }
    }
}

fn external_readers_available() -> bool {
    let timeout = std::time::Duration::from_secs(10);
    let available = crate::dat::archive::rar::RarProvider::discover(timeout).is_ok()
        && crate::dat::archive::lha::LhaProvider::discover(timeout).is_ok();
    if !available {
        eprintln!("RAR/LHA production audit proof skipped: capable optional 7-Zip unavailable");
    }
    available
}
