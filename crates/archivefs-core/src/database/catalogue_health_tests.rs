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
