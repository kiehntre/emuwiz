use super::*;
use archivefs_core::dat::sources::audit_cache::AuditCacheConfig;
use archivefs_core::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
use archivefs_core::safe_read::TrustedRoots;
use std::{fs, os::unix::ffi::OsStrExt, sync::atomic::AtomicBool};

pub(crate) struct Lab {
    _dir: tempfile::TempDir,
    pub path: std::path::PathBuf,
    pub database: std::path::PathBuf,
    pub dat: std::path::PathBuf,
}

pub(crate) fn cartridge() -> Vec<u8> {
    let mut bytes = vec![0; 64 + 16 + 8192];
    bytes[..16].copy_from_slice(b"C64 CARTRIDGE   ");
    bytes[0x10..0x14].copy_from_slice(&64u32.to_be_bytes());
    bytes[0x14..0x16].copy_from_slice(&0x100u16.to_be_bytes());
    bytes[0x19] = 1;
    bytes[64..68].copy_from_slice(b"CHIP");
    bytes[68..72].copy_from_slice(&(16u32 + 8192).to_be_bytes());
    bytes[76..78].copy_from_slice(&0x8000u16.to_be_bytes());
    bytes[78..80].copy_from_slice(&8192u16.to_be_bytes());
    // Synthetic reset/warm vectors and CBM80 signature; green border loop.
    bytes[80..89].copy_from_slice(&[0x09, 0x80, 0x09, 0x80, 0xc3, 0xc2, 0xcd, 0x38, 0x30]);
    bytes[89..103].copy_from_slice(&[
        0x78, 0xa9, 0x05, 0x8d, 0x20, 0xd0, 0xa9, 0x00, 0x8d, 0x11, 0xd0, 0x4c, 0x14, 0x80,
    ]);
    bytes
}

impl Lab {
    pub(crate) fn new() -> Self {
        Self::with_bytes(&cartridge(), false)
    }
    fn with_bytes(bytes: &[u8], ambiguous: bool) -> Self {
        Self::with_header(bytes, ambiguous, "Commodore 64")
    }
    fn with_header(bytes: &[u8], ambiguous: bool, header: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.crt");
        fs::write(&path, bytes).unwrap();
        let sha1 = crate::gui_v2::vice_evidence::hex(&sha1::Sha1::digest(bytes));
        let entry = format!(
            "<game name=\"Synthetic\"><rom name=\"synthetic.crt\" size=\"{}\" sha1=\"{sha1}\"/></game>",
            bytes.len()
        );
        let second = if ambiguous {
            entry.replace("Synthetic", "Other")
        } else {
            String::new()
        };
        let dat = dir.path().join("c64.dat");
        fs::write(&dat, format!("<datafile><header><name>{header}</name><description>{header}</description><version>1</version><author>No-Intro</author><homepage>No-Intro</homepage></header>{entry}{second}</datafile>")).unwrap();
        let mut registry = DatSourceRegistry::new();
        registry
            .add(DatSourceEntry::new(
                "c64-test".into(),
                "Synthetic C64".into(),
                dat.clone(),
                DatSourceKind::File,
            ))
            .unwrap();
        archivefs_core::dat::sources::save_dat_sources_config_to(
            dir.path().join("dat_sources.toml"),
            &registry.to_config(),
        )
        .unwrap();
        let database = dir.path().join("library.sqlite3");
        archivefs_core::Database::open_or_create(&database)
            .unwrap()
            .close()
            .unwrap();
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute("INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')", [dir.path().as_os_str().as_bytes()]).unwrap();
        db.execute("INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,size_bytes,first_seen_at,last_seen_at,created_at,updated_at) VALUES(1,1,?1,?2,?1,'direct_game_image','Synthetic','Synthetic',?3,'now','now','now','now')", rusqlite::params![b"synthetic.crt".as_slice(), path.as_os_str().as_bytes(), bytes.len() as i64]).unwrap();
        db.execute("INSERT INTO platform_assignments(archive_id,platform,source,assigned_at,is_current) VALUES(1,'Commodore 64','dat','now',1)", []).unwrap();
        drop(db);
        let outcome = run_dat_audit_with_cache(
            &DatAuditRequest {
                source_id: "c64-test".into(),
                source_display_name: "Synthetic C64".into(),
                dat_path: dat.clone(),
                dat_kind: DatSourceKind::File,
                scan_root: dir.path().into(),
                limits: Default::default(),
                policy: None,
                platform: Some("Commodore 64".into()),
            },
            &TrustedRoots::none(),
            &AtomicBool::new(false),
            &|_| {},
            AuditCacheConfig::Disabled,
        )
        .unwrap();
        archivefs_core::Database::open_or_create(&database)
            .unwrap()
            .persist_library_dat_identities_from_audit(&outcome)
            .unwrap();
        Self {
            _dir: dir,
            path,
            database,
            dat,
        }
    }
    pub(crate) fn evidence(&self) -> Result<Evidence, String> {
        let catalogues = super::super::dat_exact_tests::catalogues(&self.database)?;
        inspect(&self.path, &self.database, &catalogues)
    }
}

#[test]
fn current_dat_match_requires_exact_cartridge_bytes_and_current_source() {
    let lab = Lab::new();
    let original = lab.evidence().unwrap();
    assert_eq!(original.file.size, 8272);
    let bytes = fs::read(&lab.path).unwrap();
    let mut changed = bytes.clone();
    changed[100] ^= 1;
    fs::write(&lab.path, changed).unwrap();
    assert!(lab.evidence().is_err());
    fs::write(&lab.path, &bytes).unwrap();
    // Even restored content needs fresh saved audit observations.
    assert!(lab.evidence().is_err());

    let lab = Lab::new();
    fs::write(&lab.dat, b"<datafile/>").unwrap();
    assert!(lab.evidence().is_err());
}

#[test]
fn ambiguous_dat_and_missing_evidence_never_authorize_play() {
    let lab = Lab::with_bytes(&cartridge(), true);
    assert!(lab.evidence().is_err());
    let lab = Lab::new();
    assert!(inspect(&lab.path, &lab.database, &HashMap::new()).is_err());
    fs::remove_file(&lab.path).unwrap();
    assert!(lab.evidence().is_err());
}

#[test]
fn assigned_c64_label_cannot_override_a_different_catalogue_system() {
    let lab = Lab::with_header(&cartridge(), false, "Commodore VIC-20");
    assert!(lab.evidence().is_err());
}

#[test]
fn other_cartridge_hardware_ram_banks_and_bad_structure_are_refused() {
    for offset in [0x16, 0x18, 0x19, 0x48, 0x4a, 0x4c, 0x4e, 0] {
        let mut bytes = cartridge();
        bytes[offset] ^= 1;
        let lab = Lab::with_bytes(&bytes, false);
        assert!(lab.evidence().is_err(), "offset {offset:x} must refuse");
    }
}

#[test]
fn links_and_special_files_are_refused_without_opening_them() {
    use std::os::unix::fs::symlink;
    let lab = Lab::new();
    let link = lab.path.with_extension("link.crt");
    symlink(&lab.path, &link).unwrap();
    assert!(read_regular(&link, MAX_CARTRIDGE_BYTES).is_err());
    let hard = lab.path.with_extension("hard.crt");
    fs::hard_link(&lab.path, &hard).unwrap();
    assert!(read_regular(&lab.path, MAX_CARTRIDGE_BYTES).is_err());
    let fifo = lab.path.with_extension("fifo.crt");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(read_regular(&fifo, MAX_CARTRIDGE_BYTES).is_err());
    assert!(read_regular(Path::new("/dev/zero"), MAX_CARTRIDGE_BYTES).is_err());
}
