use super::*;
use crate::catalogue_health::{ScanCoverageState, SourceScanCoverage};

fn config(roots: Vec<PathBuf>, temp: &Path) -> Config {
    Config {
        source_folders: roots,
        mount_root: temp.join("mounts"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    }
}

#[test]
fn targeted_platform_scan_preserves_unattempted_sources() {
    let temp = tempfile::tempdir().unwrap();
    let roots: Vec<_> = ["arcade", "snes", "ps2"]
        .into_iter()
        .map(|n| temp.path().join(n))
        .collect();
    let mut db = Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
    for root in &roots {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("game.zip"), b"game").unwrap();
    }
    scan_and_persist(&mut db, &config(roots.clone(), temp.path()), "initial").unwrap();
    let before = db.load_archives().unwrap();
    for root in &roots[1..] {
        fs::remove_file(root.join("game.zip")).unwrap();
    }
    let folders = db.register_source_folders(&roots).unwrap();
    let scan = scan_and_persist_folders(&mut db, &folders[..1], "targeted").unwrap();
    let coverage = db.scan_coverage(scan.scan_run_id).unwrap();
    assert_eq!(
        coverage
            .iter()
            .filter(|c| c.state == ScanCoverageState::NotAttempted)
            .count(),
        2
    );
    assert_eq!(
        db.discovery_run_status(scan.scan_run_id).unwrap(),
        Some(DiscoveryRunStatus::Partial)
    );
    assert_eq!(db.load_archives().unwrap()[1..], before[1..]);
}

#[test]
fn missing_authority_reservation_serializes_source_removal_until_commit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    fs::create_dir(&root).unwrap();
    let mut db = Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
    let source = db.register_source_folders(&[root]).unwrap().remove(0);
    let path = db.path().to_path_buf();
    let tx = db.connection.savepoint().unwrap();
    catalogue_health::lock_missing_authority(&tx).unwrap();

    let other = Connection::open(path).unwrap();
    other.busy_timeout(Duration::from_millis(25)).unwrap();
    let blocked = other.execute(
        "UPDATE source_folders SET removed_from_config_at='race' WHERE id=?1",
        [source.id],
    );
    assert!(
        blocked.is_err(),
        "another connection changed source authority while reserved"
    );
    assert_eq!(
        other
            .query_row(
                "SELECT removed_from_config_at IS NOT NULL FROM source_folders WHERE id=?1",
                [source.id],
                |r| r.get::<_, bool>(0),
            )
            .unwrap(),
        false,
    );
    tx.commit().unwrap();
    other
        .execute(
            "UPDATE source_folders SET removed_from_config_at='after-commit' WHERE id=?1",
            [source.id],
        )
        .unwrap();
}

#[test]
fn missing_authority_is_rechecked_when_source_is_removed_after_preflight() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    fs::create_dir(&root).unwrap();
    let archive_path = root.join("game.zip");
    fs::write(&archive_path, b"fixture").unwrap();
    let mut db = Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
    let source = db
        .register_source_folders(std::slice::from_ref(&root))
        .unwrap()
        .remove(0);
    db.upsert_archive(
        source.id,
        &root,
        &Archive::from_path(&archive_path).unwrap(),
    )
    .unwrap();
    let run = scan_and_persist(
        &mut db,
        &config(vec![root.clone()], temp.path()),
        "complete",
    )
    .unwrap()
    .scan_run_id;
    fs::remove_file(&archive_path).unwrap();
    let binding = crate::catalogue_health::SourceRootBinding::inspect(&root).unwrap();

    // This is the public path's early authority check, before filesystem
    // probes. A concurrent configuration write lands before its write boundary.
    catalogue_health::assert_missing_authority(
        &db.connection,
        source.id,
        run,
        root.as_os_str().as_bytes(),
        &binding,
    )
    .unwrap();
    let other = Connection::open(db.path()).unwrap();
    other
        .execute(
            "UPDATE source_folders SET removed_from_config_at='raced' WHERE id=?1",
            [source.id],
        )
        .unwrap();

    let tx = db.connection.savepoint().unwrap();
    catalogue_health::lock_missing_authority(&tx).unwrap();
    assert!(
        catalogue_health::assert_missing_authority(
            &tx,
            source.id,
            run,
            root.as_os_str().as_bytes(),
            &binding,
        )
        .is_err()
    );
    assert_eq!(
        tx.query_row(
            "SELECT last_verified_missing_at IS NOT NULL FROM archives WHERE source_folder_id=?1",
            [source.id],
            |r| r.get::<_, bool>(0),
        )
        .unwrap(),
        false,
    );
}

#[test]
fn partial_platform_proof_cannot_authorize_missing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    fs::create_dir(&root).unwrap();
    let path = root.join("game.zip");
    fs::write(&path, b"game").unwrap();
    let mut db = Database::open_or_create(temp.path().join("db")).unwrap();
    let source = db
        .register_source_folders(std::slice::from_ref(&root))
        .unwrap()
        .remove(0);
    let id = db
        .upsert_archive(source.id, &root, &Archive::from_path(&path).unwrap())
        .unwrap()
        .archive_id;
    fs::remove_file(path).unwrap();
    let run = db.start_scan_run("partial-platform", None).unwrap();
    db.record_scan_coverage(
        run,
        &SourceScanCoverage {
            source_id: source.id,
            root_identity: crate::catalogue_health::source_root_identity(&root),
            root,
            state: ScanCoverageState::Partial,
            excluded_roots: vec![],
            diagnostic: Some("only Arcade was enumerated".into()),
        },
    )
    .unwrap();
    assert_eq!(
        db.mark_unseen_archives_missing(run, source.id, &[])
            .unwrap(),
        0
    );
    assert!(
        db.load_archives()
            .unwrap()
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .last_verified_missing_at
            .is_none()
    );
}

#[test]
fn complete_scan_does_not_call_an_existing_unseen_file_missing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    fs::create_dir(&root).unwrap();
    let path = root.join("game.zip");
    fs::write(&path, b"game").unwrap();
    let mut db = Database::open_or_create(temp.path().join("db")).unwrap();
    let source = db
        .register_source_folders(std::slice::from_ref(&root))
        .unwrap()
        .remove(0);
    let binding = crate::catalogue_health::SourceRootBinding::inspect(&root).unwrap();
    // Model the normal first-scan capture before catalogue rows establish history.
    assert!(
        db.bind_scan_source(source.id, (binding.device, binding.inode))
            .unwrap()
    );
    db.upsert_archive(source.id, &root, &Archive::from_path(&path).unwrap())
        .unwrap();
    let run = db.start_scan_run("complete", None).unwrap();
    db.record_scan_coverage(
        run,
        &SourceScanCoverage {
            source_id: source.id,
            root_identity: crate::catalogue_health::source_root_identity(&root),
            root,
            state: ScanCoverageState::Complete,
            excluded_roots: vec![],
            diagnostic: None,
        },
    )
    .unwrap();
    assert_eq!(
        db.mark_unseen_archives_missing(run, source.id, &[])
            .unwrap(),
        0
    );
    assert!(
        db.load_archives().unwrap()[0]
            .last_verified_missing_at
            .is_none()
    );
}

#[test]
fn excluded_nested_source_is_not_reconciled_by_parent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    let child = root.join("snes");
    fs::create_dir_all(&child).unwrap();
    let path = child.join("game.zip");
    fs::write(&path, b"game").unwrap();
    let mut db = Database::open_or_create(temp.path().join("db")).unwrap();
    let folders = db
        .register_source_folders(&[root.clone(), child.clone()])
        .unwrap();
    db.upsert_archive(folders[0].id, &root, &Archive::from_path(&path).unwrap())
        .unwrap();
    fs::remove_file(path).unwrap();
    let scan = scan_and_persist_folders(&mut db, &folders[..1], "parent-only").unwrap();
    assert_eq!(scan.counts.archives_missing, 0);
    assert!(
        db.load_archives().unwrap()[0]
            .last_verified_missing_at
            .is_none()
    );
}

#[test]
fn recoverable_walk_error_withholds_missing_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("gone.zip"), b"game").unwrap();
    let mut db = Database::open_or_create(temp.path().join("db")).unwrap();
    let config = config(vec![root.clone()], temp.path());
    scan_and_persist(&mut db, &config, "initial").unwrap();
    fs::remove_file(root.join("gone.zip")).unwrap();
    let mut deep = root.clone();
    for _ in 0..130 {
        deep = deep.join("a");
        fs::create_dir(&deep).unwrap();
    }
    let scan = scan_and_persist(&mut db, &config, "depth-bound").unwrap();
    assert_eq!(
        db.scan_coverage(scan.scan_run_id).unwrap()[0].state,
        ScanCoverageState::Partial
    );
    assert!(
        db.load_archives().unwrap()[0]
            .last_verified_missing_at
            .is_none()
    );
}

#[test]
fn replaced_source_root_invalidates_coverage_proof() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    fs::create_dir(&root).unwrap();
    let path = root.join("game.zip");
    fs::write(&path, b"game").unwrap();
    let mut db = Database::open_or_create(temp.path().join("db")).unwrap();
    let source = db
        .register_source_folders(std::slice::from_ref(&root))
        .unwrap()
        .remove(0);
    db.upsert_archive(source.id, &root, &Archive::from_path(&path).unwrap())
        .unwrap();
    let run = db.start_scan_run("complete", None).unwrap();
    db.record_scan_coverage(
        run,
        &SourceScanCoverage {
            source_id: source.id,
            root_identity: crate::catalogue_health::source_root_identity(&root),
            root: root.clone(),
            state: ScanCoverageState::Complete,
            excluded_roots: vec![],
            diagnostic: None,
        },
    )
    .unwrap();
    fs::rename(&root, temp.path().join("previous-root")).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(
        db.mark_unseen_archives_missing(run, source.id, &[])
            .is_err()
    );
    assert!(
        db.load_archives().unwrap()[0]
            .last_verified_missing_at
            .is_none()
    );
}

#[test]
fn failed_run_cannot_use_a_prior_complete_coverage_proof() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    fs::create_dir(&root).unwrap();
    let path = root.join("game.zip");
    fs::write(&path, b"game").unwrap();
    let mut db = Database::open_or_create(temp.path().join("db")).unwrap();
    let source = db
        .register_source_folders(std::slice::from_ref(&root))
        .unwrap()
        .remove(0);
    db.upsert_archive(source.id, &root, &Archive::from_path(&path).unwrap())
        .unwrap();
    let run = db.start_scan_run("complete", None).unwrap();
    db.record_scan_coverage(
        run,
        &SourceScanCoverage {
            source_id: source.id,
            root_identity: crate::catalogue_health::source_root_identity(&root),
            root,
            state: ScanCoverageState::Complete,
            excluded_roots: vec![],
            diagnostic: None,
        },
    )
    .unwrap();
    fs::remove_file(path).unwrap();
    db.fail_scan_run(run, "failed before completion").unwrap();
    assert_eq!(
        db.mark_unseen_archives_missing(run, source.id, &[])
            .unwrap(),
        0
    );
    assert!(
        db.load_archives().unwrap()[0]
            .last_verified_missing_at
            .is_none()
    );
}

#[test]
fn migration_safety_bookkeeping_is_additive_idempotent_and_atomic() {
    let temp = tempfile::tempdir().unwrap();
    for version in [16, 21, 22] {
        let path = temp.path().join(format!("schema-{version}"));
        let mut connection = open_connection(&path).unwrap();
        apply_migrations(&mut connection, &MIGRATIONS[..version]).unwrap();
        connection.execute("INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'old','old')",[b"/historical/source".as_slice()]).unwrap();
        connection.execute("INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,last_verified_missing_at,created_at,updated_at) VALUES(1,1,X'61',X'62',X'61','zip','a','a','old','old','missing','old','old')",[]).unwrap();
        if version == 22 {
            connection.execute_batch("CREATE TRIGGER reject_23 BEFORE INSERT ON schema_migrations WHEN NEW.version=23 BEGIN SELECT RAISE(ABORT,'injected bookkeeping failure'); END;").unwrap();
            assert!(apply_migrations(&mut connection, MIGRATIONS).is_err());
            assert_eq!(
                connection
                    .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                22
            );
            assert_eq!(connection.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('source_scan_bindings','catalogue_health_epoch')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
            connection.execute_batch("DROP TRIGGER reject_23").unwrap();
        }
        apply_migrations(&mut connection, MIGRATIONS).unwrap();
        apply_migrations(&mut connection, MIGRATIONS).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            23
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT last_verified_missing_at FROM archives WHERE id=1",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "missing"
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM source_scan_bindings", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT revision FROM catalogue_health_epoch", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(
            connection
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query([])
                .unwrap()
                .next()
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn root_changed_after_folder_persistence_refuses_the_outer_commit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    fs::create_dir(&root).unwrap();
    let path = root.join("game.zip");
    fs::write(&path, b"fixture").unwrap();
    let mut db = Database::open_or_create(temp.path().join("db")).unwrap();
    let source = db
        .register_source_folders(std::slice::from_ref(&root))
        .unwrap()
        .remove(0);
    let id = db
        .upsert_archive(source.id, &root, &Archive::from_path(&path).unwrap())
        .unwrap()
        .archive_id;
    db.begin_catalogue_refresh().unwrap();
    let run = db.start_scan_run("late root change", None).unwrap();
    let identity = crate::catalogue_health::source_root_identity(&root).unwrap();
    assert!(db.bind_scan_source(source.id, identity).unwrap());
    db.record_scan_coverage(
        run,
        &SourceScanCoverage {
            source_id: source.id,
            root_identity: Some(identity),
            root: root.clone(),
            state: ScanCoverageState::Complete,
            excluded_roots: vec![],
            diagnostic: None,
        },
    )
    .unwrap();
    db.connection
        .execute(
            "UPDATE archives SET last_verified_missing_at='staged-before-outer-commit' WHERE id=?1",
            [id],
        )
        .unwrap();
    fs::rename(&root, temp.path().join("original")).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(db.validate_scan_source_commit(run).is_err());
    db.rollback_catalogue_refresh();
    assert_eq!(
        db.load_archives().unwrap()[0].last_verified_missing_at,
        None
    );
    assert_eq!(
        fs::read(temp.path().join("original/game.zip")).unwrap(),
        b"fixture"
    );
}
