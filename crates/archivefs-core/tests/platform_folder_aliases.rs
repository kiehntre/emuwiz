//! Synthetic, bounded scanner regressions. Never scans a configured real source.
archivefs_core::install_test_environment!();

use archivefs_core::platform::detect::{
    DetectionRequest, DetectionSource, detect_platform_report, platform_for_folder_name,
};
use archivefs_core::{
    Archive, ArchiveHealth, ArchiveIdentity, ArchiveKind, Config, Database, PlatformProvenance,
    detect_platform_with_details, scan_and_persist,
};
use std::path::Path;

const ALIASES: [(&str, &str, &str); 4] = [
    ("pc-8800-series", "NEC PC-8801", "pc88"),
    ("acpc", "Amstrad CPC", "amstradcpc"),
    ("sega32", "Sega 32X", "sega32x"),
    ("dc", "Dreamcast", "dreamcast"),
];

#[test]
fn recovered_folder_aliases_are_exact_supported_platform_evidence() {
    for (folder, expected, frontend) in ALIASES {
        for variant in [
            folder.to_string(),
            folder.to_uppercase(),
            folder.replace('-', "_"),
        ] {
            assert_eq!(
                platform_for_folder_name(&variant).map(|p| p.id),
                Some(expected),
                "{variant}"
            );
            let path = Path::new("/synthetic").join(&variant).join("Game.zip");
            let details = detect_platform_with_details(&path, "/synthetic").unwrap();
            assert_eq!(details.platform, expected);
            assert_eq!(details.provenance, PlatformProvenance::FolderAlias);
            let report =
                detect_platform_report(&DetectionRequest::new(&path, Path::new("/synthetic")));
            assert_eq!(report.deciding_source, Some(DetectionSource::FolderAlias));
            assert!(
                report.requires_confirmation,
                "a folder is not verified game identity"
            );
        }
        assert_eq!(
            archivefs_core::launch::es_de_export::es_de_system_for_platform(expected)
                .unwrap()
                .es_de_system,
            frontend
        );
        assert!(platform_for_folder_name(&format!("{folder}-unrelated")).is_none());
        let filename = Path::new("/synthetic").join(format!("{folder}.zip"));
        assert!(detect_platform_with_details(&filename, "/synthetic").is_none());
    }
}

#[test]
fn unsupported_hardware_and_media_names_do_not_become_folder_aliases() {
    for name in [
        "msx-turbo",
        "vc-4000",
        "daphne",
        "beena",
        "sega-pico",
        "pico",
        "ipf",
        "roms",
        "preservation",
    ] {
        assert!(platform_for_folder_name(name).is_none(), "{name}");
    }
    assert!(
        detect_platform_with_details("/synthetic/preservation/ipf/Game.ipf", "/synthetic")
            .is_none(),
        "an IPF extension plus a media folder is not Amiga evidence"
    );
    let path = Path::new("/pc-8800-series/private/Game.zip");
    assert!(detect_platform_with_details(path, "/pc-8800-series/private").is_none());
    let nested =
        detect_platform_with_details("/synthetic/pc-8800-series/pc98/Game.zip", "/synthetic")
            .unwrap();
    assert_eq!(
        nested.platform, "PC-98",
        "nearest specific folder still wins"
    );
    let path = Path::new("/synthetic/sega32/Game.zip");
    let manual = detect_platform_report(
        &DetectionRequest::new(path, Path::new("/synthetic")).with_manual_platform(Some("Amiga")),
    );
    assert_eq!(manual.platform, Some("Amiga"));
}

#[test]
fn bounded_synthetic_rescan_preserves_ids_paths_hashes_dat_and_user_choices() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let db_path = dir.path().join("copy.sqlite3");
    let mut database = Database::open_or_create(&db_path).unwrap();
    let folder_id = database.register_source_folders(&[source.clone()]).unwrap()[0].id;
    let mut expected = Vec::new();
    let mut empty_zip = vec![0; 22];
    empty_zip[..4].copy_from_slice(b"PK\x05\x06");
    for (alias, platform, _) in ALIASES {
        for kind in ["automatic", "manual", "dat", "custom"] {
            let path = source.join(alias).join(if kind == "custom" {
                "custom/Game.zip"
            } else {
                kind
            });
            let path = if kind == "custom" {
                path
            } else {
                path.with_extension("zip")
            };
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &empty_zip).unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            let mut identity = ArchiveIdentity::from_path(&path, &source, Some(&metadata));
            // Simulate an older catalogue row, without clearing any live record.
            identity.platform = None;
            identity.platform_provenance = None;
            let archive = Archive {
                path: path.clone(),
                kind: ArchiveKind::Zip,
                identity,
                health: ArchiveHealth::Pending,
            };
            let id = database
                .upsert_archive(folder_id, &source, &archive)
                .unwrap()
                .archive_id;
            let target = match kind {
                "manual" => {
                    database.set_manual_platform(id, "Amiga").unwrap();
                    "Amiga"
                }
                "dat" => {
                    database
                        .assign_platform(
                            id,
                            Some("AtariST"),
                            archivefs_core::VERIFIED_DAT_PLATFORM_SOURCE,
                        )
                        .unwrap();
                    "AtariST"
                }
                "custom" => "PC",
                _ => platform,
            };
            expected.push((id, path, target));
        }
    }
    database.add_platform_alias("custom", "PC").unwrap();
    // Seed a persisted fingerprint in the disposable database only.
    let evidence = rusqlite::Connection::open(&db_path).unwrap();
    evidence
        .execute(
            "UPDATE archives SET content_hash='synthetic-preserved-content-hash'",
            [],
        )
        .unwrap();
    let config = Config {
        source_folders: vec![source],
        mount_root: dir.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    for _ in 0..2 {
        scan_and_persist(
            &mut database,
            &config,
            "synthetic-platform-alias-regression",
        )
        .unwrap();
        let rows = database.load_archives().unwrap();
        assert_eq!(rows.len(), expected.len());
        for (id, path, platform) in &expected {
            let row = rows.iter().find(|row| row.id == *id).unwrap();
            assert_eq!(&row.absolute_path, path);
            assert_eq!(row.platform.as_deref(), Some(*platform));
            let hash: String = evidence
                .query_row(
                    "SELECT content_hash FROM archives WHERE id=?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(hash, "synthetic-preserved-content-hash");
            assert!(row.last_verified_missing_at.is_none());
            assert_eq!(std::fs::read(path).unwrap(), empty_zip);
        }
    }
}

/// Explicit opt-in audit of an owner's DATABASE COPY, opened read-only.
/// Applies the bounded proposal to a second, test-owned copy only. No input
/// database writes, signatures, archive extraction or directory traversal.
/// Normal tests skip it; this is never a production-rescan entry point.
#[test]
#[ignore = "requires EMUWIZ_PLATFORM_AUDIT_COPY pointing at a disposable SQLite backup"]
fn audit_database_copy_read_only() {
    use rusqlite::{Connection, OpenFlags};
    use std::collections::{BTreeMap, BTreeSet};
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;
    let path =
        std::env::var_os("EMUWIZ_PLATFORM_AUDIT_COPY").expect("explicit database copy required");
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection.pragma_update(None, "query_only", true).unwrap();
    let owned = tempfile::tempdir().unwrap();
    let validation_path = owned.path().join("validation.sqlite3");
    connection
        .backup(rusqlite::MAIN_DB, &validation_path, None)
        .unwrap();
    let mut validation = Database::open_or_create(&validation_path).unwrap();
    let evidence =
        Connection::open_with_flags(&validation_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let protected = protected_table_hashes(&evidence);
    let mut proposed = Vec::new();
    let mut alias_query = connection
        .prepare("SELECT normalized_alias FROM platform_aliases")
        .unwrap();
    let custom_aliases = alias_query
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<BTreeSet<_>, _>>()
        .unwrap();
    let names = [
        "pc-8800-series",
        "acpc",
        "msx-turbo",
        "vc-4000",
        "sega32",
        "daphne",
        "beena",
        "dc",
        "ipf",
        "sega-pico",
    ];
    let mut counts: BTreeMap<&str, serde_json::Value> = names.into_iter().map(|name| (name, serde_json::json!({"rows":0,"unknown_before":0,"present":0,"recognised_present_unknown":0,"unknown_after_projection":0}))).collect();
    let mut statement = connection.prepare("SELECT a.absolute_path_cached, s.path, p.platform, a.id,
    (s.assigned_platform IS NOT NULL OR s.source_role != 'games'
     OR EXISTS(SELECT 1 FROM verified_identity_facts f WHERE f.archive_id=a.id)
     OR EXISTS(SELECT 1 FROM library_dat_identities d WHERE d.archive_id=a.id AND d.verification_state NOT IN ('no_match','no_usable_evidence')))
    FROM archives a JOIN source_folders s ON s.id=a.source_folder_id LEFT JOIN platform_assignments p ON p.archive_id=a.id AND p.is_current=1").unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (path, source_root, current, id, protected_context) = row.unwrap();
        let path = PathBuf::from(std::ffi::OsString::from_vec(path));
        let source_root = PathBuf::from(std::ffi::OsString::from_vec(source_root));
        let Some(name) = names.iter().find(|name| {
            path.parent()
                .is_some_and(|p| p.components().any(|part| part.as_os_str() == **name))
        }) else {
            continue;
        };
        let count = counts.get_mut(name).unwrap();
        let present = path.is_file(); // Exact catalogue paths only; no collection scan.
        let unknown = current.is_none();
        let report = detect_platform_report(&DetectionRequest::new(&path, &source_root));
        let corrected = present
            && unknown
            && !protected_context
            && !audit_has_custom_alias(&path, &source_root, &custom_aliases)
            && report.deciding_source == Some(DetectionSource::FolderAlias)
            && ALIASES
                .iter()
                .any(|(alias, platform, _)| alias == name && Some(*platform) == report.platform);
        if corrected {
            proposed.push((id, path.clone(), report.platform.unwrap().to_string()));
        }
        for (key, increment) in [
            ("rows", true),
            ("unknown_before", unknown),
            ("present", present),
            ("recognised_present_unknown", corrected),
            ("unknown_after_projection", unknown && !corrected),
        ] {
            count[key] = (count[key].as_u64().unwrap() + u64::from(increment)).into();
        }
    }
    let mut applied = 0;
    for (id, _path, platform) in &proposed {
        validation
            .assign_platform(
                *id,
                Some(platform),
                PlatformProvenance::FolderAlias.as_source_str(),
            )
            .unwrap();
        let actual: String = evidence
            .query_row(
                "SELECT platform FROM platform_assignments WHERE archive_id=?1 AND is_current=1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(&actual, platform);
        applied += 1;
    }
    assert_eq!(
        protected_table_hashes(&evidence),
        protected,
        "assignment must preserve archives, DATs, overrides, source state and scan cache"
    );
    println!("COPY_VALIDATED_ASSIGNMENTS={applied}; PROTECTED_TABLES_UNCHANGED=true");
    println!(
        "FOLDER_AUDIT_JSON={}",
        serde_json::to_string_pretty(&counts).unwrap()
    );
}

#[test]
fn old_cached_unknown_requires_fresh_per_record_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let path = source.join("acpc/Game.zip");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = vec![0; 22];
    bytes[..4].copy_from_slice(b"PK\x05\x06");
    std::fs::write(&path, &bytes).unwrap();
    let db_path = dir.path().join("synthetic.sqlite3");
    let mut database = Database::open_or_create(&db_path).unwrap();
    let config = Config {
        source_folders: vec![source.clone()],
        mount_root: dir.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    scan_and_persist(&mut database, &config, "synthetic-first").unwrap();
    let id = database.load_archives().unwrap()[0].id;
    // Simulate the old fingerprint and Unknown assignment in this fixture only.
    let fixture = rusqlite::Connection::open(&db_path).unwrap();
    fixture
        .execute("DELETE FROM platform_assignments WHERE archive_id=?1", [id])
        .unwrap();
    fixture
        .execute(
            "UPDATE scan_fingerprints SET platform=NULL,platform_provenance=NULL",
            [],
        )
        .unwrap();
    scan_and_persist(&mut database, &config, "synthetic-cached").unwrap();
    assert!(database.load_archives().unwrap()[0].platform.is_none());
    let fresh = Archive::from_path_in_root(&path, &source).unwrap();
    assert_eq!(fresh.identity.platform.as_deref(), Some("Amstrad CPC"));
    assert_eq!(
        fresh.identity.platform_provenance,
        Some(PlatformProvenance::FolderAlias)
    );
    // Canonical assignment API, same id/path; fresh identification is not a
    // filesystem traversal and does not require rewriting the media or DATs.
    database
        .assign_platform(
            id,
            fresh.identity.platform.as_deref(),
            fresh.identity.platform_provenance.unwrap().as_source_str(),
        )
        .unwrap();
    let row = database.load_archives().unwrap().remove(0);
    assert_eq!(row.id, id);
    assert_eq!(row.absolute_path, path);
    assert_eq!(row.platform.as_deref(), Some("Amstrad CPC"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

// Hash every value deterministically; the copy's assignment-only validation
// must leave all existing identity, paths, DAT and configuration data intact.
fn protected_table_hashes(connection: &rusqlite::Connection) -> Vec<Vec<u8>> {
    use sha2::{Digest, Sha256};
    [
        "archives",
        "verified_identity_facts",
        "library_dat_identities",
        "platform_aliases",
        "source_folders",
        "scan_fingerprints",
    ]
    .iter()
    .map(|table| {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
            .unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        let mut hash = Sha256::new();
        while let Some(row) = rows.next().unwrap() {
            for column in 0..columns {
                use rusqlite::types::ValueRef;
                let value = row.get_ref(column).unwrap();
                match value {
                    ValueRef::Null => hash.update(b"N"),
                    ValueRef::Integer(value) => {
                        hash.update(b"I");
                        hash.update(value.to_le_bytes());
                    }
                    ValueRef::Real(value) => {
                        hash.update(b"R");
                        hash.update(value.to_bits().to_le_bytes());
                    }
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                        hash.update(if matches!(value, ValueRef::Text(_)) {
                            b"T"
                        } else {
                            b"B"
                        });
                        hash.update((bytes.len() as u64).to_le_bytes());
                        hash.update(bytes);
                    }
                }
            }
        }
        hash.finalize().to_vec()
    })
    .collect()
}

// Match the same bounded, normalised whole components as the scanner's custom
// alias resolver. Unresolved custom authority is excluded, never overwritten.
fn audit_has_custom_alias(
    path: &Path,
    source_root: &Path,
    aliases: &std::collections::BTreeSet<String>,
) -> bool {
    let Some(relative) = path.strip_prefix(source_root).ok().and_then(Path::parent) else {
        return false;
    };
    relative
        .components()
        .map(|part| part.as_os_str())
        .chain(source_root.file_name())
        .any(|part| {
            aliases.contains(&archivefs_core::platform::normalize_alias(
                &part.to_string_lossy(),
            ))
        })
}

#[test]
fn copy_audit_excludes_custom_alias_authority_within_the_source_boundary() {
    let custom = std::collections::BTreeSet::from(["acpc".to_string(), "private".to_string()]);
    assert!(audit_has_custom_alias(
        Path::new("/source/acpc/Game.zip"),
        Path::new("/source"),
        &custom
    ));
    assert!(audit_has_custom_alias(
        Path::new("/source/AC_PC/Game.zip"),
        Path::new("/source"),
        &custom
    ));
    assert!(audit_has_custom_alias(
        Path::new("/source/acpc/Game.zip"),
        Path::new("/source/acpc"),
        &custom
    ));
    assert!(!audit_has_custom_alias(
        Path::new("/private/source/sega32/Game.zip"),
        Path::new("/private/source"),
        &custom
    ));
    assert!(!audit_has_custom_alias(
        Path::new("/source/sega32/acpc.zip"),
        Path::new("/source"),
        &custom
    ));
}
