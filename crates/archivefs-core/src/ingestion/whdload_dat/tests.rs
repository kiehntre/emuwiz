use super::*;
use crate::dat::{limits::DatLimits, parsers::parse_dat_file};
use crate::identity_source::whdload::inspect_whdload_slave_file;
use sha2::{Digest, Sha256};

fn slave(root: &Path) -> SlaveArtifact {
    let mut code = vec![0; 96];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    code[12..14].copy_from_slice(&1u16.to_be_bytes());
    code[24..26].copy_from_slice(&70u16.to_be_bytes());
    let mut bytes = Vec::new();
    for word in [0x3f3u32, 0, 1, 0, 0, 24, 0x3e9, 24] {
        bytes.extend(word.to_be_bytes());
    }
    bytes.extend(code);
    bytes.extend(0x3f2u32.to_be_bytes());
    let path = root.join("Misleading title.slave");
    std::fs::write(&path, bytes).unwrap();
    inspect_whdload_slave_file(&path).unwrap()
}
fn catalogue(root: &Path, filename: &str, title: &str, sha1: &str) -> SpectrumCatalogue {
    let path = root.join(filename);
    std::fs::write(&path,format!(r#"<datafile><header><name>Commodore - Amiga - WHDLoad</name><version>fixture-revision</version></header><game name="{title}"><rom name="irrelevant.slave" sha1="{sha1}"/></game></datafile>"#)).unwrap();
    SpectrumCatalogue::from_parsed_for_platform(
        &parse_dat_file(&path, DatLimits::default()).unwrap().dat,
    )
    .unwrap()
}
#[test]
fn exact_slave_association_retains_catalogue_digest_and_does_not_verify_package() {
    let temp = tempfile::tempdir().unwrap();
    let slave = slave(temp.path());
    let cat = catalogue(
        temp.path(),
        "retroplay-looking.dat",
        "Canonical Release",
        &slave.hashes.sha1,
    );
    let expected: String = Sha256::digest(std::fs::read(&cat.source.file_path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let entries = ScanCatalogues::load(vec![cat]).unwrap();
    // A suggestive filename never invents a preservation corpus.
    assert_eq!(entries[0].artifact.source_family, SourceFamily::Unknown);
    assert_eq!(
        entries[0].artifact.artifact_sha256.as_deref(),
        Some(expected.as_str())
    );
    let mut scan = ScanCatalogues {
        loaded: Some(Ok(entries)),
    };
    let note = scan.annotate(&[slave]);
    for expected in [
        "Exact catalogue match: Canonical Release.",
        "fixture-revision",
        &expected,
        "whole-.slave SHA-1",
        "content are not verified",
        "metadata and filenames alone",
    ] {
        assert!(note.contains(expected), "{note}");
    }
}
#[test]
fn cross_catalogue_disagreement_never_selects_a_release() {
    let temp = tempfile::tempdir().unwrap();
    let slave = slave(temp.path());
    let entries = ScanCatalogues::load(vec![
        catalogue(temp.path(), "a.dat", "First Release", &slave.hashes.sha1),
        catalogue(temp.path(), "b.dat", "Second Release", &slave.hashes.sha1),
    ])
    .unwrap();
    let mut scan = ScanCatalogues {
        loaded: Some(Ok(entries)),
    };
    let note = scan.annotate(&[slave]);
    assert!(note.contains("Conflicting WHDLoad catalogue associations"));
    assert!(note.contains("First Release") && note.contains("Second Release"));
    assert!(!note.contains("Exact catalogue match:"));
    assert!(note.contains("no release selected"));
}
#[test]
fn unmatched_slave_name_is_never_catalogue_identity() {
    let temp = tempfile::tempdir().unwrap();
    let slave = slave(temp.path());
    let entries = ScanCatalogues::load(vec![catalogue(
        temp.path(),
        "a.dat",
        &slave.name,
        &"0".repeat(40),
    )])
    .unwrap();
    let mut scan = ScanCatalogues {
        loaded: Some(Ok(entries)),
    };
    let note = scan.annotate(&[slave]);
    assert!(note.contains("No exact whole-.slave SHA-1 catalogue match"));
    assert!(!note.contains("Exact catalogue match:"));
}
#[test]
fn changed_catalogue_or_configuration_invalidates_scan_context() {
    let temp = tempfile::tempdir().unwrap();
    let slave = slave(temp.path());
    let mut cat = catalogue(temp.path(), "a.dat", "Release", &slave.hashes.sha1);
    let config = temp.path().join("registry.toml");
    std::fs::write(&config, b"enabled=true").unwrap();
    cat.inputs
        .push((config.clone(), FileFingerprint::observe(&config).unwrap()));
    let scan = ScanCatalogues {
        loaded: Some(Ok(ScanCatalogues::load(vec![cat]).unwrap())),
    };
    scan.recheck().unwrap();
    std::fs::write(config, b"enabled=false").unwrap();
    assert!(scan.recheck().unwrap_err().contains("Scan rolled back"));
    let cat = catalogue(temp.path(), "b.dat", "Release", &slave.hashes.sha1);
    let path = cat.source.file_path.clone();
    let scan = ScanCatalogues {
        loaded: Some(Ok(ScanCatalogues::load(vec![cat]).unwrap())),
    };
    std::fs::write(path, b"replacement").unwrap();
    assert!(scan.recheck().is_err());
}
#[test]
fn missing_and_unavailable_catalogues_are_explicit_structural_only() {
    let temp = tempfile::tempdir().unwrap();
    let slave = slave(temp.path());
    for loaded in [
        Ok(Vec::new()),
        Err("synthetic unavailable catalogue".into()),
    ] {
        let mut scan = ScanCatalogues {
            loaded: Some(loaded),
        };
        let note = scan.annotate(std::slice::from_ref(&slave));
        assert!(!note.contains("Exact catalogue match:"));
        assert!(note.contains("not verified"));
    }
}
#[test]
fn unrelated_discovery_never_loads_dat_configuration() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("readme.txt"), b"notes").unwrap();
    let mut scan = ScanCatalogues::default();
    let report = discover(temp.path(), &[], &mut scan).unwrap();
    assert!(report.items.is_empty());
    assert!(scan.loaded.is_none());
}

#[test]
fn mid_scan_catalogue_change_rolls_back_native_scan_writes_and_preserves_history() {
    let test_name = "ingestion::whdload_dat::tests::mid_scan_catalogue_change_rolls_back_native_scan_writes_and_preserves_history";
    let Ok(_) = std::env::var("EMUWIZ_WHDLOAD_COMMIT_FIXTURE") else {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env("EMUWIZ_WHDLOAD_COMMIT_FIXTURE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    // Core unit binaries replace inherited environment roots before main.
    // Use that existing process isolation rather than trying to override it.
    let root = crate::test_environment::isolated_root()
        .expect("rollback fixture requires native test isolation")
        .to_path_buf();
    let source = root.join("games");
    let install = source.join("Wrong title");
    std::fs::create_dir_all(&install).unwrap();
    let artifact = slave(&install);
    let cat = catalogue(
        &root,
        "current.dat",
        "Canonical Release",
        &artifact.hashes.sha1,
    );
    let dat_path = std::path::PathBuf::from(cat.source.file_path);
    let registry = crate::dat::sources::default_dat_sources_config_path().unwrap();
    assert!(registry.starts_with(&root));
    std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
    std::fs::write(&registry,format!("[[sources]]\nid='fixture'\ndisplay_name='Fixture'\npath='{}'\nkind='file'\nenabled=true\nplatform='Amiga'\nhealth_state='valid'\n",dat_path.display())).unwrap();
    let path = root.join("db");
    let mut db = crate::Database::open_or_create(&path).unwrap();
    let config = crate::Config {
        source_folders: vec![source.clone()],
        mount_root: root.join("mount"),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    };
    let run = crate::scan_and_persist(&mut db, &config, "baseline").unwrap();
    db.close().unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("INSERT INTO discovery_import_operations VALUES('prior','committed','old',NULL,x'7b7d'); INSERT INTO discovery_import_records VALUES('prior',99,1,x'61',x'62',x'7b7d',0);").unwrap();
    let counts = |c: &rusqlite::Connection| -> Vec<i64> {
        ["scan_runs", "discovery_details", "archives"]
            .iter()
            .map(|t| {
                c.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                    .unwrap()
            })
            .collect()
    };
    let before = counts(&c);
    drop(c);
    // New valid content would add another discovery row if the scan committed.
    let extra = source.join("Second install");
    std::fs::create_dir(&extra).unwrap();
    let extra_artifact = slave(&extra);
    let original = std::fs::read(&extra_artifact.path).unwrap();
    BEFORE_COMMIT_FAULT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(dat_path, b"replacement during scan").unwrap();
        }))
    });
    let mut db = crate::Database::open_or_create(&path).unwrap();
    let error = crate::scan_and_persist(&mut db, &config, "must-roll-back").unwrap_err();
    assert!(error.to_string().contains("Scan rolled back"), "{error}");
    db.close().unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(counts(&c), before);
    assert_eq!(
        c.query_row("SELECT max(id) FROM scan_runs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        run.scan_run_id
    );
    assert_eq!(c.query_row("SELECT state||':'||hex(receipt_json) FROM discovery_import_operations WHERE operation_id='prior'",[],|r|r.get::<_,String>(0)).unwrap(),"committed:7B7D");
    assert_eq!(
        c.query_row(
            "SELECT touched FROM discovery_import_records WHERE operation_id='prior'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(std::fs::read(extra_artifact.path).unwrap(), original);
}
