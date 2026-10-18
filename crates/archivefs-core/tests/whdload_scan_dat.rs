//! Real native scan path, with HOME/application roots isolated in a subprocess.
use archivefs_core::{Config, Database, scan_and_persist};
use sha1::{Digest, Sha1};
use std::{path::PathBuf, process::Command};

fn slave() -> Vec<u8> {
    let mut code = vec![0; 96];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    code[12..14].copy_from_slice(&1u16.to_be_bytes());
    code[14..16].copy_from_slice(&3u16.to_be_bytes());
    code[16..20].copy_from_slice(&524288u32.to_be_bytes());
    code[24..26].copy_from_slice(&70u16.to_be_bytes());
    let mut bytes = Vec::new();
    for word in [0x3f3u32, 0, 1, 0, 0, 24, 0x3e9, 24] {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    bytes.extend(code);
    bytes.extend(0x3f2u32.to_be_bytes());
    bytes
}

#[test]
fn scan_supplies_registered_dat_context_without_creating_verified_results() {
    let Ok(root) = std::env::var("EMUWIZ_WHDLOAD_FIXTURE_CHILD") else {
        let temp = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "scan_supplies_registered_dat_context_without_creating_verified_results",
                "--nocapture",
            ])
            .env("EMUWIZ_WHDLOAD_FIXTURE_CHILD", temp.path())
            .env("HOME", temp.path())
            .env("EMUWIZ_CONFIG_HOME", temp.path().join("config"))
            .env("EMUWIZ_DATA_HOME", temp.path().join("data"))
            .env("XDG_CONFIG_HOME", temp.path().join("xdg-config"))
            .env("XDG_DATA_HOME", temp.path().join("xdg-data"))
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    let root = PathBuf::from(root);
    let source = root.join("games");
    let install = source.join("Misleading filename");
    std::fs::create_dir_all(&install).unwrap();
    let bytes = slave();
    archivefs_core::identity_source::whdload::parse_whdload_slave(&bytes).unwrap();
    let slave_path = install.join("Wrong title.slave");
    std::fs::write(&slave_path, &bytes).unwrap();
    let sha1: String = Sha1::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let dat = root.join("catalogue.dat");
    std::fs::write(&dat, format!(r#"<datafile><header><name>Commodore - Amiga - WHDLoad</name><version>2026-test</version></header><game name="Canonical Release"><rom name="Canonical.slave" size="{}" sha1="{sha1}"/></game></datafile>"#,bytes.len())).unwrap();
    let registry = archivefs_core::dat::sources::default_dat_sources_config_path().unwrap();
    assert!(registry.starts_with(&root));
    std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
    std::fs::write(&registry, format!("[[sources]]\nid='fixture'\ndisplay_name='Reviewed local catalogue'\npath='{}'\nkind='file'\nenabled=true\nplatform='Amiga'\nhealth_state='valid'\n",dat.display())).unwrap();
    let db_path = root.join("library.sqlite3");
    let mut db = Database::open_or_create(&db_path).unwrap();
    let config = Config {
        source_folders: vec![source],
        mount_root: root.join("mount"),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    };

    let mut preserved = vec![(slave_path.clone(), bytes.clone())];
    if archivefs_core::dat::archive::lha::LhaProvider::discover(std::time::Duration::from_secs(5))
        .is_ok()
    {
        for extension in ["lha", "lzh"] {
            let path = config.source_folders[0].join(format!("Wrong package title.{extension}"));
            let package = stored_lha(&[("Wrong/Title.slave", &bytes)]);
            std::fs::write(&path, &package).unwrap();
            preserved.push((path, package));
        }
    } else {
        eprintln!("LHA backend unavailable: archive-specific assertions not executed");
    }
    let report = scan_and_persist(&mut db, &config, "whdload-fixture").unwrap();
    db.close().unwrap();
    let explanation = detail(&db_path, report.scan_run_id, &install);
    assert!(
        explanation.contains("Exact catalogue match: Canonical Release."),
        "{explanation}"
    );
    assert!(explanation.contains("2026-test"), "{explanation}");
    assert!(explanation.contains("local:fixture"), "{explanation}");
    let catalogue_sha256: String = sha2::Sha256::digest(std::fs::read(&dat).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(explanation.contains(&catalogue_sha256), "{explanation}");
    println!(
        "Checked WHDLoad folder and {} stored LHA/LZH packages",
        preserved.len() - 1
    );
    assert!(explanation.contains("not verified"), "{explanation}");
    for (path, _) in preserved.iter().skip(1) {
        let note = detail(&db_path, report.scan_run_id, path);
        assert!(
            note.contains("Exact catalogue match: Canonical Release."),
            "{note}"
        );
        assert!(note.contains("not verified"), "{note}");
    }
    let c = rusqlite::Connection::open(&db_path).unwrap();
    for table in [
        "library_dat_identities",
        "dat_set_audit_results",
        "verified_identity_facts",
    ] {
        assert_eq!(
            c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    drop(c);
    let original_registry = std::fs::read_to_string(&registry).unwrap();
    for (case, body) in [
        (
            "disabled",
            original_registry.replace("enabled=true", "enabled=false"),
        ),
        (
            "stale",
            format!("{original_registry}health_observed_size_bytes=0\n"),
        ),
    ] {
        std::fs::write(&registry, body).unwrap();
        let mut db = Database::open_or_create(&db_path).unwrap();
        let run = scan_and_persist(&mut db, &config, case).unwrap();
        db.close().unwrap();
        let note = detail(&db_path, run.scan_run_id, &install);
        assert!(!note.contains("Exact catalogue match:"), "{case}: {note}");
        assert!(note.contains("not verified"), "{case}: {note}");
    }
    std::fs::write(&registry, original_registry).unwrap();
    let old_dat = std::fs::read_to_string(&dat).unwrap();
    std::fs::write(
        &dat,
        old_dat
            .replace("2026-test", "replacement-revision")
            .replace(&sha1, &"0".repeat(40)),
    )
    .unwrap();
    let mut db = Database::open_or_create(&db_path).unwrap();
    let run = scan_and_persist(&mut db, &config, "replaced-catalogue").unwrap();
    db.close().unwrap();
    assert!(!detail(&db_path, run.scan_run_id, &install).contains("Exact catalogue match:"));
    std::fs::remove_file(&dat).unwrap();
    let mut db = Database::open_or_create(&db_path).unwrap();
    let run = scan_and_persist(&mut db, &config, "unavailable-catalogue").unwrap();
    db.close().unwrap();
    assert!(!detail(&db_path, run.scan_run_id, &install).contains("Exact catalogue match:"));
    for (path, expected) in preserved {
        assert_eq!(std::fs::read(path).unwrap(), expected);
    }
}
fn detail(database: &std::path::Path, run: i64, path: &std::path::Path) -> String {
    Database::open_read_only(database)
        .unwrap()
        .query_discovery_details(run, 0, 100, archivefs_core::DiscoveryDetailFilter::All)
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.path == path)
        .unwrap_or_else(|| panic!("No discovery detail for {} in run {run}", path.display()))
        .explanation
}
fn stored_lha(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut result = Vec::new();
    for (name, payload) in entries {
        let mut crc = 0u16;
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
        header.extend(b"-lh0-");
        header.extend((payload.len() as u32).to_le_bytes());
        header.extend((payload.len() as u32).to_le_bytes());
        header.extend(0u32.to_le_bytes());
        header.extend([0x20, 0, name.len() as u8]);
        header.extend(name.as_bytes());
        header.extend(crc.to_le_bytes());
        header.push(0);
        header[1] = header[2..].iter().fold(0u8, |sum, b| sum.wrapping_add(*b));
        header.extend_from_slice(payload);
        result.extend(header);
    }
    result.push(0);
    result
}
