use archivefs_core::catalogue_health::{
    CatalogueHealth, MoveEvidence, ScanCoverageState, SourceRootBinding, preview_catalogue_health,
};
use archivefs_core::{Archive, Config, Database, scan_and_persist};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    db: Database,
    roots: Vec<PathBuf>,
}
impl Fixture {
    fn new(names: &[&str]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let roots: Vec<_> = names.iter().map(|n| temp.path().join(n)).collect();
        for root in &roots {
            fs::create_dir_all(root).unwrap();
        }
        let mut db = Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
        db.register_source_folders(&roots).unwrap();
        Self { temp, db, roots }
    }
    fn sql(&self) -> Connection {
        Connection::open(self.db.path()).unwrap()
    }
    fn add(&mut self, source: usize, relative: &str) -> (i64, PathBuf) {
        let path = self.roots[source].join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"fixture bytes").unwrap();
        let archive = Archive::from_path_in_root(&path, &self.roots[source]).unwrap();
        let source_id = self
            .db
            .list_source_folders()
            .unwrap()
            .iter()
            .find(|s| s.path == self.roots[source])
            .unwrap()
            .id;
        let id = self
            .db
            .upsert_archive(source_id, &self.roots[source], &archive)
            .unwrap()
            .archive_id;
        (id, path)
    }
    fn stale(&self, id: i64) {
        self.sql()
            .execute(
                "UPDATE archives SET last_verified_missing_at='old-evidence' WHERE id=?1",
                [id],
            )
            .unwrap();
    }
    fn scan(&mut self) -> archivefs_core::ScanPersistSummary {
        scan_and_persist(
            &mut self.db,
            &Config {
                source_folders: self.roots.clone(),
                mount_root: self.temp.path().join("mounts"),
                ratarmount_bin: "ratarmount".into(),
                master_rom_root: None,
            },
            "fixture",
        )
        .unwrap()
    }
    fn preview(&self) -> archivefs_core::catalogue_health::CatalogueHealthReport {
        preview_catalogue_health(&self.db, &self.roots).unwrap()
    }
    fn flag(&self, id: i64) -> Option<String> {
        self.db
            .load_archives()
            .unwrap()
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
            .last_verified_missing_at
    }
    fn status(&self, run: i64) -> String {
        self.sql()
            .query_row("SELECT status FROM scan_runs WHERE id=?1", [run], |r| {
                r.get(0)
            })
            .unwrap()
    }
}

#[test]
fn complete_successful_scan_records_complete_coverage() {
    let mut f = Fixture::new(&["snes"]);
    f.add(0, "game.zip");
    let s = f.scan();
    assert_eq!(f.status(s.scan_run_id), "completed");
    assert_eq!(
        f.db.scan_coverage(s.scan_run_id).unwrap()[0].state,
        ScanCoverageState::Complete
    );
}

#[test]
fn only_arcade_succeeds_without_poisoning_snes_or_ps2() {
    let mut f = Fixture::new(&["arcade", "snes", "ps2"]);
    let (a, ap) = f.add(0, "gone.zip");
    let (s, _) = f.add(1, "game.zip");
    let (p, _) = f.add(2, "game.zip");
    fs::remove_file(ap).unwrap();
    fs::rename(&f.roots[1], f.temp.path().join("offline-snes")).unwrap();
    fs::rename(&f.roots[2], f.temp.path().join("offline-ps2")).unwrap();
    let scan = f.scan();
    assert!(f.flag(a).is_some());
    assert_eq!(f.flag(s), None);
    assert_eq!(f.flag(p), None);
    assert_eq!(f.status(scan.scan_run_id), "partial");
    assert_eq!(
        f.db.scan_coverage(scan.scan_run_id)
            .unwrap()
            .iter()
            .filter(|s| s.state == ScanCoverageState::Unavailable)
            .count(),
        2
    );
}

#[test]
fn nested_arcade_does_not_replace_mixed_source_walk() {
    let mut f = Fixture::new(&["games"]);
    let (s, _) = f.add(0, "snes/game.zip");
    let (p, _) = f.add(0, "ps2/game.zip");
    let set = f.roots[0].join("arcade/testset");
    fs::create_dir_all(&set).unwrap();
    fs::write(set.join("chip1"), b"a").unwrap();
    fs::write(set.join("chip2"), b"b").unwrap();
    let scan = f.scan();
    assert_eq!(scan.counts.archives_seen, 3);
    assert_eq!(f.flag(s), None);
    assert_eq!(f.flag(p), None);
    assert_eq!(f.status(scan.scan_run_id), "completed");
}

#[test]
fn source_error_cannot_stamp_its_rows_missing() {
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    fs::rename(&f.roots[0], f.temp.path().join("old-games")).unwrap();
    fs::write(&f.roots[0], b"not a directory").unwrap();
    let s = f.scan();
    assert_eq!(f.flag(id), None);
    assert_eq!(f.status(s.scan_run_id), "partial");
    assert_eq!(
        f.db.scan_coverage(s.scan_run_id).unwrap()[0].state,
        ScanCoverageState::Failed
    );
}

#[test]
fn stale_present_flag_is_previewed_without_writes() {
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    f.stale(id);
    let before = fs::read(f.db.path()).unwrap();
    let report = f.preview();
    assert_eq!(report.counts.present_stale_flag, 1);
    assert_eq!(report.counts.rows_would_change, 1);
    assert_eq!(report.rows[0].health, CatalogueHealth::PresentNotVerified);
    assert_eq!(fs::read(f.db.path()).unwrap(), before);
    assert!(f.flag(id).is_some());
}

#[test]
fn apply_only_clears_proven_presence_and_preserves_history() {
    let mut f = Fixture::new(&["games"]);
    let (p, _) = f.add(0, "present.zip");
    let (m, mp) = f.add(0, "missing.zip");
    f.stale(p);
    f.stale(m);
    fs::remove_file(mp).unwrap();
    let before = f.db.load_archives().unwrap();
    let report = f.preview();
    assert_eq!(f.db.apply_presence_reconciliation(&report).unwrap(), 1);
    assert_eq!(f.flag(p), None);
    assert_eq!(f.db.load_archives().unwrap()[1], before[1]);
    assert_eq!(f.sql().query_row("SELECT count(*) FROM archive_scan_observations WHERE archive_id=?1 AND observation='restored'",[p],|r|r.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn gone_path_is_marked_only_under_complete_source() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    fs::remove_file(path).unwrap();
    assert_eq!(f.scan().counts.archives_missing, 1);
    assert!(f.flag(id).is_some());
}

#[test]
fn unproven_scan_cannot_call_missing_mutation() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    fs::remove_file(path).unwrap();
    let run = f.db.start_scan_run("unproven", None).unwrap();
    assert_eq!(f.db.mark_unseen_archives_missing(run, 1, &[]).unwrap(), 0);
    assert_eq!(f.flag(id), None);
}

#[test]
fn basename_match_is_review_evidence_and_never_relinks() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "old/game.zip");
    f.add(0, "new/game.zip");
    fs::remove_file(&path).unwrap();
    let r = f.preview();
    assert_eq!(r.rows[0].health, CatalogueHealth::PossiblyMoved);
    assert_eq!(
        r.rows[0].move_candidates[0].evidence,
        MoveEvidence::BasenameAndSize
    );
    assert_eq!(f.db.load_archives().unwrap()[0].absolute_path, path);
    assert_eq!(f.flag(id), None);
}

#[test]
fn verified_file_hash_strengthens_move_evidence() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "old/game.zip");
    f.add(0, "new/game.zip");
    let hash: String = Sha256::digest(fs::read(&path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    f.sql()
        .execute(
            "UPDATE archives SET archive_hash=?2 WHERE id=?1",
            params![id, hash],
        )
        .unwrap();
    fs::remove_file(path).unwrap();
    let r = f.preview();
    assert_eq!(
        r.rows[0].move_candidates[0].evidence,
        MoveEvidence::VerifiedSha256
    );
    assert_eq!(r.counts.strong_move_candidates, 1);
    f.sql()
        .execute("UPDATE archives SET size_bytes=NULL WHERE id=?1", [id])
        .unwrap();
    assert_eq!(
        f.preview().rows[0].move_candidates[0].evidence,
        MoveEvidence::VerifiedSha256,
        "missing optional size metadata must not suppress a proven SHA-256 match"
    );
}

#[test]
fn removed_source_is_explicit_and_rows_are_preserved() {
    let mut f = Fixture::new(&["old", "games"]);
    let (id, _) = f.add(0, "game.zip");
    f.stale(id);
    f.db.register_source_folders(&f.roots[1..]).unwrap();
    let before = f.db.load_archives().unwrap().remove(0);
    let r = preview_catalogue_health(&f.db, &f.roots[1..]).unwrap();
    assert_eq!(r.rows[0].health, CatalogueHealth::OrphanedSource);
    assert_eq!(r.counts.orphaned_source, 1);
    assert_eq!(f.db.apply_presence_reconciliation(&r).unwrap(), 1);
    let after = f.db.load_archives().unwrap().remove(0);
    assert_eq!(after.source_folder_id, before.source_folder_id);
    assert_eq!(after.absolute_path, before.absolute_path);
    assert!(after.last_verified_missing_at.is_none());
    assert_eq!(
        preview_catalogue_health(&f.db, &f.roots[1..]).unwrap().rows[0].health,
        CatalogueHealth::OrphanedSource
    );
    assert_eq!(f.db.load_archives().unwrap().len(), 1);
}

#[test]
fn multiple_move_candidates_are_ambiguous() {
    let mut f = Fixture::new(&["games"]);
    let (_, p) = f.add(0, "old/game.zip");
    f.add(0, "new/game.zip");
    f.add(0, "other/game.zip");
    fs::remove_file(p).unwrap();
    let r = f.preview();
    assert_eq!(r.rows[0].move_candidates.len(), 2);
    assert_eq!(r.counts.ambiguous_move_candidates, 1);
}

#[test]
fn unavailable_root_is_not_verified_missing() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    fs::rename(&f.roots[0], f.temp.path().join("offline")).unwrap();
    assert_eq!(f.preview().rows[0].health, CatalogueHealth::NotChecked);
}

#[test]
fn recovery_after_partial_scan_restores_normal_coverage() {
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    let offline = f.temp.path().join("offline");
    fs::rename(&f.roots[0], &offline).unwrap();
    let partial = f.scan();
    assert_eq!(f.status(partial.scan_run_id), "partial");
    fs::rename(offline, &f.roots[0]).unwrap();
    let s = f.scan();
    assert_eq!(f.status(s.scan_run_id), "completed");
    assert_eq!(f.flag(id), None);
}

#[test]
fn repeated_preview_is_deterministic() {
    let mut f = Fixture::new(&["games"]);
    let (_, p) = f.add(0, "old/game.zip");
    f.add(0, "new/game.zip");
    fs::remove_file(p).unwrap();
    assert_eq!(f.preview(), f.preview());
}

#[test]
fn dry_run_counts_reconcile_exactly_with_apply() {
    let mut f = Fixture::new(&["games"]);
    for i in 0..4 {
        let (id, _) = f.add(0, &format!("game{i}.zip"));
        if i % 2 == 0 {
            f.stale(id);
        }
    }
    let r = f.preview();
    assert_eq!(
        r.counts.total,
        r.counts.rows_would_change + r.counts.rows_left_untouched
    );
    assert_eq!(
        f.db.apply_presence_reconciliation(&r).unwrap(),
        r.counts.rows_would_change
    );
    assert_eq!(f.preview().counts.rows_would_change, 0);
}

#[test]
fn changed_preview_is_rejected_atomically() {
    let mut f = Fixture::new(&["games"]);
    let (a, _) = f.add(0, "a.zip");
    let (b, bp) = f.add(0, "b.zip");
    f.stale(a);
    f.stale(b);
    let r = f.preview();
    fs::remove_file(bp).unwrap();
    assert!(f.db.apply_presence_reconciliation(&r).is_err());
    assert!(f.flag(a).is_some());
    assert!(f.flag(b).is_some());
}

#[test]
fn leaf_symlink_is_not_presence_proof() {
    let mut f = Fixture::new(&["games"]);
    let (id, p) = f.add(0, "game.zip");
    f.stale(id);
    let target = f.temp.path().join("target");
    fs::rename(&p, &target).unwrap();
    std::os::unix::fs::symlink(target, &p).unwrap();
    let r = f.preview();
    assert_eq!(r.counts.rows_would_change, 0);
    assert_eq!(r.rows[0].health, CatalogueHealth::NotChecked);
}

#[test]
fn arcade_directory_is_present_not_a_missing_regular_file() {
    let mut f = Fixture::new(&["arcade"]);
    let set = f.roots[0].join("testset");
    fs::create_dir_all(&set).unwrap();
    fs::write(set.join("a"), b"1").unwrap();
    fs::write(set.join("b"), b"2").unwrap();
    f.scan();
    let r = f.preview();
    assert_eq!(r.counts.present_clean, 1);
    assert_eq!(r.rows[0].health, CatalogueHealth::PresentNotVerified);
}

#[test]
fn repeated_complete_scans_do_not_invent_missing_evidence() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    let first = f.scan();
    let second = f.scan();
    assert_eq!(first.counts.archives_missing, 0);
    assert_eq!(second.counts.archives_missing, 0);
    assert_eq!(second.counts.archives_unchanged, 1);
    assert_eq!(f.status(second.scan_run_id), "completed");
}

#[test]
fn case_folded_basename_is_explicitly_weak_review_evidence() {
    let mut f = Fixture::new(&["games"]);
    let (_, old) = f.add(0, "old/GAME.zip");
    f.add(0, "new/game.zip");
    fs::remove_file(old).unwrap();
    let report = f.preview();
    assert_eq!(report.rows[0].health, CatalogueHealth::PossiblyMoved);
    assert!(!report.rows[0].move_candidates[0].basename_exact);
    assert_ne!(
        report.rows[0].move_candidates[0].evidence,
        MoveEvidence::VerifiedSha256
    );
}

#[test]
fn present_verified_requires_current_fingerprint_and_existing_verified_identity() {
    use archivefs_core::game_identity::{
        GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
        IdentityKind, IdentityPlatform, IdentityProvenance, IdentityStatus,
    };
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.iso");
    let report = GameIdentityReport {
        archive_path: path.clone(),
        platform: IdentityPlatform::PlayStation2,
        format: IdentityImageFormat::Iso,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::Ps2Serial,
            status: IdentityStatus::Verified,
            value: Some("SLUS-12345".into()),
            confidence: IdentityConfidence::ExactBytes,
            provenance: IdentityProvenance {
                archive_path: path.clone(),
                member_path: None,
                member_index: None,
                method: "fixture".into(),
            },
            diagnostic: "fixture".into(),
        }],
        warnings: vec![],
        bytes_read: 13,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    f.sql().execute("UPDATE archives SET identity_report_json=?2,identity_report_size_bytes=size_bytes,identity_report_modified_time_unix_seconds=modified_time_unix_seconds WHERE id=?1",params![id,serde_json::to_vec(&report).unwrap()]).unwrap();
    assert_eq!(f.preview().rows[0].health, CatalogueHealth::PresentVerified);
    fs::write(path, b"changed identity contents").unwrap();
    assert_eq!(
        f.preview().rows[0].health,
        CatalogueHealth::PresentNotVerified
    );
}

#[test]
fn historical_hash_mismatch_cannot_strengthen_a_name_match() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "old/game.zip");
    let (_, other) = f.add(0, "new/game.zip");
    let hash: String = Sha256::digest(fs::read(&path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    f.sql()
        .execute(
            "UPDATE archives SET archive_hash=?2 WHERE id=?1",
            params![id, hash],
        )
        .unwrap();
    fs::remove_file(path).unwrap();
    fs::write(other, b"wrong-content").unwrap();
    assert_ne!(
        f.preview().rows[0].move_candidates[0].evidence,
        MoveEvidence::VerifiedSha256
    );
}

#[test]
fn preceding_schema_preview_does_not_migrate_or_write() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    let sql = f.sql();
    let triggers: Vec<String> = sql
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'catalogue_epoch_%'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for trigger in triggers {
        sql.execute_batch(&format!("DROP TRIGGER {trigger}"))
            .unwrap();
    }
    sql.execute_batch("DROP TABLE source_scan_bindings; DROP TABLE catalogue_health_epoch; DELETE FROM schema_migrations WHERE version=23; DROP TABLE scan_source_coverage; DELETE FROM schema_migrations WHERE version=22; PRAGMA user_version=21;").unwrap();
    let before = fs::read(f.db.path()).unwrap();
    let read_only = Database::open_catalogue_health_read_only(f.db.path()).unwrap();
    assert_eq!(
        preview_catalogue_health(&read_only, &f.roots)
            .unwrap()
            .counts
            .present_clean,
        1
    );
    assert_eq!(read_only.schema_version().unwrap(), 21);
    assert_eq!(fs::read(f.db.path()).unwrap(), before);
}

#[test]
fn every_symlink_position_and_loop_refuses_presence() {
    use std::os::unix::fs::symlink;
    for position in ["root", "parent", "middle", "leaf", "loop"] {
        let mut f = Fixture::new(&["games"]);
        let (id, path) = f.add(0, "SNES/deeper/game.zip");
        f.stale(id);
        let target = match position {
            "root" => f.roots[0].clone(),
            "parent" => f.roots[0].join("SNES"),
            "middle" | "loop" => path.parent().unwrap().to_path_buf(),
            _ => path.clone(),
        };
        let outside = f.temp.path().join("saved");
        fs::rename(&target, &outside).unwrap();
        symlink(
            if position == "loop" {
                &target
            } else {
                &outside
            },
            &target,
        )
        .unwrap();
        assert_eq!(f.preview().counts.rows_would_change, 0, "{position}");
        assert!(f.flag(id).is_some());
    }
}

#[test]
fn introduced_ancestor_symlink_invalidates_an_existing_preview() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "SNES/deeper/game.zip");
    f.stale(id);
    let preview = f.preview();
    let parent = path.parent().unwrap();
    let saved = f.temp.path().join("saved");
    fs::rename(parent, &saved).unwrap();
    symlink(&saved, parent).unwrap();
    assert!(f.db.apply_presence_reconciliation(&preview).is_err());
    assert!(f.flag(id).is_some());
}

#[test]
fn stale_preview_refuses_database_changes_including_aba() {
    for change in [
        "path",
        "missing",
        "removed",
        "reattached",
        "delete_recreate",
        "new_scan",
        "identity",
    ] {
        let mut f = Fixture::new(&["games"]);
        let (id, _) = f.add(0, "game.zip");
        f.stale(id);
        let preview = f.preview();
        let sql = f.sql();
        match change {
            "path" => {
                sql.execute(
                    "UPDATE archives SET absolute_path_cached=?1 WHERE id=?2",
                    params![
                        f.roots[0].join("other.zip").as_os_str().as_encoded_bytes(),
                        id
                    ],
                )
                .unwrap();
            }
            "missing" => {
                sql.execute(
                    "UPDATE archives SET last_verified_missing_at='new-evidence' WHERE id=?1",
                    [id],
                )
                .unwrap();
            }
            "removed" => {
                sql.execute(
                    "UPDATE source_folders SET removed_from_config_at='removed'",
                    [],
                )
                .unwrap();
            }
            "reattached" => {
                sql.execute_batch("UPDATE source_folders SET removed_from_config_at='removed'; UPDATE source_folders SET removed_from_config_at=NULL;").unwrap();
            }
            "delete_recreate" => {
                sql.execute_batch("CREATE TEMP TABLE saved AS SELECT * FROM archives; DELETE FROM archives; INSERT INTO archives SELECT * FROM saved;").unwrap();
            }
            "new_scan" => {
                f.db.start_scan_run("new scan", None).unwrap();
            }
            _ => {
                sql.execute(
                    "UPDATE archives SET archive_hash=?1 WHERE id=?2",
                    params!["f".repeat(64), id],
                )
                .unwrap();
            }
        }
        assert!(
            f.db.apply_presence_reconciliation(&preview).is_err(),
            "{change}"
        );
        if change != "path" {
            assert_eq!(
                f.preview().counts.rows_would_change,
                1,
                "fresh preview: {change}"
            );
        }
    }
}

#[test]
fn known_root_replacement_requires_reviewed_generation_and_new_scan() {
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "SNES/game.zip");
    let scan = f.scan();
    let source = f.db.load_archives().unwrap()[0].source_folder_id;
    fs::rename(&f.roots[0], f.temp.path().join("preserved")).unwrap();
    fs::create_dir(&f.roots[0]).unwrap();
    let blocked = f.scan();
    assert_eq!(blocked.counts.archives_missing, 0);
    assert_eq!(f.flag(id), None);
    let review = f.db.review_source_rebind(source).unwrap();
    f.db.rebind_source_after_review(&review).unwrap();
    assert!(
        f.db.mark_unseen_archives_missing(scan.scan_run_id, source, &[])
            .is_err()
    );
    assert_eq!(f.flag(id), None);
    assert_eq!(f.scan().counts.archives_missing, 1);
}

#[test]
fn unavailable_then_same_source_restored_keeps_its_generation() {
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    f.scan();
    let saved = f.temp.path().join("disconnected");
    fs::rename(&f.roots[0], &saved).unwrap();
    assert_eq!(f.scan().counts.archives_missing, 0);
    assert_eq!(f.flag(id), None);
    fs::rename(saved, &f.roots[0]).unwrap();
    drop(f.db);
    f.db = Database::open_or_create(f.temp.path().join("library.sqlite3")).unwrap();
    assert!(f.scan().folder_errors.is_empty());
    assert_eq!(
        f.sql()
            .query_row("SELECT generation FROM source_scan_bindings", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn ownership_is_most_specific_across_three_levels_and_specialist_leaf() {
    let mut f = Fixture::new(&[
        "games",
        "games/arcade/ownedset",
        "games/extra",
        "games/extra/inner",
    ]);
    fs::write(f.roots[1].join("chip.u1"), b"one").unwrap();
    fs::write(f.roots[1].join("chip.u2"), b"two").unwrap();
    let parentset = f.roots[0].join("arcade/parentset");
    fs::create_dir_all(&parentset).unwrap();
    fs::write(parentset.join("chip.u1"), b"one").unwrap();
    fs::write(parentset.join("chip.u2"), b"two").unwrap();
    f.add(0, "SNES/game.zip");
    f.add(0, "PS2/game.zip");
    f.add(2, "own.zip");
    f.add(3, "deep.zip");
    let summary = f.scan();
    assert!(
        summary.folder_errors.is_empty(),
        "{:?}",
        summary.folder_errors
    );
    let rows = f.db.load_archives().unwrap();
    let sources = f.db.list_source_folders().unwrap();
    for row in &rows {
        let owner = sources
            .iter()
            .filter(|s| row.absolute_path.starts_with(&s.path))
            .max_by_key(|s| s.path.components().count())
            .unwrap();
        assert_eq!(
            row.source_folder_id,
            owner.id,
            "{}",
            row.absolute_path.display()
        );
        assert_eq!(
            rows.iter()
                .filter(|r| r.absolute_path == row.absolute_path)
                .count(),
            1
        );
    }
    assert!(
        rows.iter()
            .any(|r| r.absolute_path == f.roots[1] && r.archive_kind == "arcade_set_directory")
    );
    assert!(rows.iter().any(|r| r.absolute_path == parentset));
    assert_eq!(rows.len(), 6);
}

#[test]
fn duplicate_basenames_have_bounded_deterministic_review_details() {
    let mut f = Fixture::new(&["games"]);
    for i in 0..300 {
        f.add(0, &format!("present/{i}/game.zip"));
    }
    for i in 0..300 {
        let (_, p) = f.add(0, &format!("absent/{i}/game.zip"));
        fs::remove_file(p).unwrap();
    }
    let preview = f.preview();
    assert_eq!(preview.counts.possibly_moved, 300);
    assert_eq!(preview.counts.ambiguous_move_candidates, 300);
    for row in preview
        .rows
        .iter()
        .filter(|r| r.health == CatalogueHealth::PossiblyMoved)
    {
        assert_eq!(row.move_candidates.len(), 256);
        assert!(row.move_candidates_truncated);
    }
    assert_eq!(preview, f.preview());
}

#[test]
fn historical_unbound_source_requires_explicit_initial_review() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    fs::remove_file(path).unwrap();
    f.sql()
        .execute(
            "UPDATE source_folders SET last_successful_scan_at='legacy-scan'",
            [],
        )
        .unwrap();
    let before = f.db.load_archives().unwrap();
    let blocked = f.scan();
    assert_eq!(blocked.counts.archives_missing, 0);
    assert_eq!(f.db.load_archives().unwrap(), before);
    let source = before[0].source_folder_id;
    let review = f.db.review_source_rebind(source).unwrap();
    f.db.rebind_source_after_review(&review).unwrap();
    assert_eq!(f.scan().counts.archives_missing, 1);
    assert!(f.flag(id).is_some());
}

#[test]
#[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
fn mount_bind_and_wrong_filesystem_fail_closed_without_remount_trap() {
    use std::ffi::CString;
    assert_eq!(std::env::var("EMUWIZ_TEST_MOUNTS").as_deref(), Ok("1"));
    struct Mounted(CString);
    impl Drop for Mounted {
        fn drop(&mut self) {
            assert_eq!(
                unsafe { libc::umount2(self.0.as_ptr(), libc::MNT_DETACH) },
                0
            );
        }
    }
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    f.scan();
    let target = CString::new(f.roots[0].as_os_str().as_encoded_bytes()).unwrap();
    // Same tree through a bind mount is a legitimate reopening of the source.
    assert_eq!(
        unsafe {
            libc::mount(
                target.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        },
        0
    );
    let bind = Mounted(target.clone());
    assert!(f.scan().folder_errors.is_empty());
    drop(bind);
    // An unrelated filesystem at that path is not the accepted generation.
    let tmpfs = CString::new("tmpfs").unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                tmpfs.as_ptr(),
                target.as_ptr(),
                tmpfs.as_ptr(),
                0,
                std::ptr::null(),
            )
        },
        0
    );
    let other = Mounted(target);
    let blocked = f.scan();
    assert_eq!(blocked.counts.archives_missing, 0);
    assert_eq!(f.flag(id), None);
    drop(other);
    assert!(f.scan().folder_errors.is_empty());
    // A mount under the source must never import outside content or restore
    // stale evidence, even a bind mount with the same st_dev.
    let (id, path) = f.add(0, "SNES/game.zip");
    f.stale(id);
    let outside = f.temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("game.zip"), b"fixture bytes").unwrap();
    let src = CString::new(outside.as_os_str().as_encoded_bytes()).unwrap();
    let dst = CString::new(path.parent().unwrap().as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                src.as_ptr(),
                dst.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        },
        0
    );
    let nested = Mounted(dst);
    assert_eq!(f.preview().counts.rows_would_change, 0);
    drop(nested);
    assert_eq!(f.preview().counts.rows_would_change, 1);
}

#[test]
#[ignore = "requires process-local fault shim; run separately"]
fn root_replaced_during_enumeration_preserves_catalogue_evidence() {
    assert_eq!(std::env::var("EMUWIZ_INJECT_FAULTS").as_deref(), Ok("1"));
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "SNES/game.zip");
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_PATH", &path);
        std::env::set_var("EMUWIZ_FAULT_ROOT", &f.roots[0]);
    }
    let summary = f.scan();
    assert_eq!(summary.counts.archives_missing, 0);
    assert_eq!(f.flag(id), None);
    assert_eq!(
        f.db.scan_coverage(summary.scan_run_id).unwrap()[0].state,
        ScanCoverageState::Failed
    );
    assert!(
        f.roots[0]
            .with_extension("saved-original")
            .join("SNES/game.zip")
            .exists()
    );
}

#[test]
#[ignore = "requires isolated mount namespace and process-local fault shim"]
fn mount_disappearing_at_candidate_probe_preserves_missing_evidence() {
    use std::ffi::CString;
    use std::os::unix::fs::MetadataExt;
    assert_eq!(std::env::var("EMUWIZ_TEST_MOUNTS").as_deref(), Ok("1"));
    assert_eq!(std::env::var("EMUWIZ_INJECT_FAULTS").as_deref(), Ok("1"));
    let mut f = Fixture::new(&["games"]);
    let target = CString::new(f.roots[0].as_os_str().as_encoded_bytes()).unwrap();
    let tmpfs = CString::new("tmpfs").unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                tmpfs.as_ptr(),
                target.as_ptr(),
                tmpfs.as_ptr(),
                0,
                std::ptr::null(),
            )
        },
        0
    );
    let (id, path) = f.add(0, "game.zip");
    let scan = f.scan();
    let source = f.db.load_archives().unwrap()[0].source_folder_id;
    let old = fs::metadata(&f.roots[0]).unwrap();
    fs::remove_file(&path).unwrap();
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_PATH", &path);
        std::env::set_var("EMUWIZ_FAULT_ROOT", &f.roots[0]);
        std::env::set_var("EMUWIZ_FAULT_UNMOUNT", "1");
    }
    assert!(
        f.db.mark_unseen_archives_missing(scan.scan_run_id, source, &[])
            .is_err()
    );
    assert_ne!(
        fs::metadata(&f.roots[0]).unwrap().dev(),
        old.dev(),
        "fault must detach the mount"
    );
    assert_eq!(f.flag(id), None);
}

#[test]
fn filesystem_id_collision_with_same_device_inode_is_not_accepted() {
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    let scan = f.scan();
    f.stale(id);
    let source = f.db.load_archives().unwrap()[0].source_folder_id;
    let current = SourceRootBinding::inspect(&f.roots[0]).unwrap();
    let mut other = current.clone();
    other.filesystem_id[0] ^= 0xff;
    f.sql()
        .execute(
            "UPDATE source_scan_bindings SET root_identity_json=?1",
            [serde_json::to_string(&other).unwrap()],
        )
        .unwrap();
    assert_eq!(f.preview().counts.rows_would_change, 0);
    assert!(
        f.db.mark_unseen_archives_missing(scan.scan_run_id, source, &[])
            .is_err()
    );
    assert_eq!(f.scan().counts.archives_missing, 0);
    assert!(f.flag(id).is_some());
    let review = f.db.review_source_rebind(source).unwrap();
    assert_eq!(review.current, current);
    f.db.rebind_source_after_review(&review).unwrap();
    assert_eq!(f.scan().counts.archives_restored, 1);
    assert_eq!(f.flag(id), None);
}

#[test]
#[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
fn duplicate_configured_bind_aliases_are_diagnosed_without_catalogue_writes() {
    use std::ffi::CString;
    assert_eq!(std::env::var("EMUWIZ_TEST_MOUNTS").as_deref(), Ok("1"));
    let mut f = Fixture::new(&["games", "alias"]);
    f.add(0, "game.zip");
    let before = f.db.load_archives().unwrap();
    let src = CString::new(f.roots[0].as_os_str().as_encoded_bytes()).unwrap();
    let dst = CString::new(f.roots[1].as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                src.as_ptr(),
                dst.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        },
        0
    );
    let config = Config {
        source_folders: f.roots.clone(),
        mount_root: f.temp.path().join("mounts"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    let result = scan_and_persist(&mut f.db, &config, "duplicate bind alias");
    assert_eq!(unsafe { libc::umount2(dst.as_ptr(), libc::MNT_DETACH) }, 0);
    assert!(result.is_err());
    assert_eq!(f.db.load_archives().unwrap(), before);
}

fn inject_parent_link_between_target_probes(apply: bool) {
    assert_eq!(std::env::var("EMUWIZ_INJECT_FAULTS").as_deref(), Ok("1"));
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "SNES/deeper/game.zip");
    f.stale(id);
    let preview = f.preview();
    let parent = path.parent().unwrap();
    let outside = f.temp.path().join("outside-original");
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_PATH", &path);
        std::env::set_var("EMUWIZ_FAULT_ROOT", &f.roots[0]);
        std::env::set_var("EMUWIZ_FAULT_PROBE_NUMBER", "2");
        std::env::set_var("EMUWIZ_FAULT_PARENT_LINK", parent);
        std::env::set_var("EMUWIZ_FAULT_LINK_TARGET", &outside);
    }
    if apply {
        assert!(f.db.apply_presence_reconciliation(&preview).is_err());
    } else {
        assert_eq!(f.preview().counts.rows_would_change, 0);
    }
    assert!(
        fs::symlink_metadata(parent)
            .unwrap()
            .file_type()
            .is_symlink(),
        "injection must happen between the target probes"
    );
    assert_eq!(
        fs::read(outside.join("game.zip")).unwrap(),
        b"fixture bytes"
    );
    assert!(f.flag(id).is_some());
}

#[test]
#[ignore = "requires process-local fault shim; run separately"]
fn ancestor_symlink_inserted_between_presence_probe_phases_is_refused() {
    inject_parent_link_between_target_probes(false);
}

#[test]
#[ignore = "requires process-local fault shim; run separately"]
fn ancestor_symlink_inserted_between_apply_probe_phases_is_refused() {
    inject_parent_link_between_target_probes(true);
}

#[test]
#[ignore = "requires process-local fault shim; run separately"]
fn root_replaced_between_preflight_and_descriptor_binding_is_refused() {
    use std::os::unix::fs::MetadataExt;
    assert_eq!(std::env::var("EMUWIZ_INJECT_FAULTS").as_deref(), Ok("1"));
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    let source = f.db.load_archives().unwrap()[0].source_folder_id;
    let old = fs::metadata(&f.roots[0]).unwrap();
    let run = f.db.start_scan_run("preflight swap", None).unwrap();
    f.sql().execute("INSERT INTO scan_source_coverage(scan_run_id,source_folder_id,state,excluded_roots_json,root_identity_json) VALUES(?1,?2,'\"complete\"','[]',?3)",params![run,source,serde_json::to_string(&(old.dev(),old.ino())).unwrap()]).unwrap();
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_PATH", &f.roots[0]);
        std::env::set_var("EMUWIZ_FAULT_ROOT", &f.roots[0]);
        std::env::set_var("EMUWIZ_FAULT_PROBE_NUMBER", "3");
    }
    assert!(f.db.mark_unseen_archives_missing(run, source, &[]).is_err());
    assert_ne!(
        fs::metadata(&f.roots[0]).unwrap().ino(),
        old.ino(),
        "fault must replace the root after the two preflight probes"
    );
    assert_eq!(f.flag(id), None);
    assert!(
        f.roots[0]
            .with_extension("saved-original")
            .join("game.zip")
            .exists()
    );
}

#[test]
fn empty_preview_is_refused_after_missing_evidence_changes() {
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    let empty = f.preview();
    assert_eq!(empty.counts.rows_would_change, 0);
    f.stale(id);
    assert!(f.db.apply_presence_reconciliation(&empty).is_err());
    let fresh = f.preview();
    assert_eq!(f.db.apply_presence_reconciliation(&fresh).unwrap(), 1);
    let next = f.preview();
    assert_eq!(f.db.apply_presence_reconciliation(&next).unwrap(), 0);
}

mod nested_mounts {
    use super::*;
    use std::ffi::CString;

    fn cstr(path: &std::path::Path) -> CString {
        CString::new(path.as_os_str().as_encoded_bytes()).unwrap()
    }
    fn mount_tmpfs(target: &std::path::Path) {
        let fs_type = CString::new("tmpfs").unwrap();
        assert_eq!(
            unsafe {
                libc::mount(
                    fs_type.as_ptr(),
                    cstr(target).as_ptr(),
                    fs_type.as_ptr(),
                    0,
                    std::ptr::null(),
                )
            },
            0
        );
    }
    fn bind(source: &std::path::Path, target: &std::path::Path) {
        assert_eq!(
            unsafe {
                libc::mount(
                    cstr(source).as_ptr(),
                    cstr(target).as_ptr(),
                    std::ptr::null(),
                    libc::MS_BIND,
                    std::ptr::null(),
                )
            },
            0
        );
    }
    fn detach(target: &std::path::Path) {
        assert_eq!(
            unsafe { libc::umount2(cstr(target).as_ptr(), libc::MNT_DETACH) },
            0
        );
    }
    fn setup() -> (Fixture, PathBuf, i64, PathBuf) {
        assert_eq!(std::env::var("EMUWIZ_TEST_MOUNTS").as_deref(), Ok("1"));
        let mut f = Fixture::new(&["games"]);
        let sub = f.roots[0].join("sub");
        fs::create_dir(&sub).unwrap();
        mount_tmpfs(&sub);
        let (id, inner) = f.add(0, "sub/inner.zip");
        (f, sub, id, inner)
    }
    fn assert_boundary_withheld(f: &mut Fixture, id: i64) {
        let s = f.scan();
        assert_eq!(s.counts.archives_missing, 0);
        assert_eq!(f.flag(id), None);
        assert_eq!(
            f.db.scan_coverage(s.scan_run_id).unwrap()[0].state,
            ScanCoverageState::Partial
        );
        assert!(
            s.folder_errors
                .iter()
                .any(|(_, m)| m.contains("nested filesystem boundary")),
            "{:?}",
            s.folder_errors
        );
    }

    #[test]
    #[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
    fn vanished_nested_mount_does_not_mark_files_beneath_missing() {
        let (mut f, sub, id, _) = setup();
        f.scan();
        assert_eq!(f.flag(id), None);
        detach(&sub);
        assert!(!sub.join("inner.zip").exists(), "empty mountpoint remains");
        assert_boundary_withheld(&mut f, id);
        // Still preserved on every later scan while the mountpoint stays empty.
        assert_boundary_withheld(&mut f, id);
        let source = f.db.load_archives().unwrap()[0].source_folder_id;
        let run = f.scan().scan_run_id;
        // Partial coverage is non-authoritative, so the write boundary refuses.
        assert_eq!(
            f.db.mark_unseen_archives_missing(run, source, &[]).ok(),
            Some(0)
        );
        assert_eq!(f.flag(id), None);
    }

    #[test]
    #[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
    fn unchanged_nested_mount_never_writes_missing_evidence() {
        let (mut f, _sub, id, _) = setup();
        for _ in 0..2 {
            let s = f.scan();
            assert_eq!(s.counts.archives_missing, 0);
            assert_eq!(f.flag(id), None);
            assert!(
                !s.folder_errors
                    .iter()
                    .any(|(_, m)| m.contains("nested filesystem boundary")),
                "an intact boundary must not be reported: {:?}",
                s.folder_errors
            );
        }
    }

    #[test]
    #[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
    fn different_filesystem_replacing_nested_mount_is_refused() {
        let (mut f, sub, id, _) = setup();
        f.scan();
        detach(&sub);
        mount_tmpfs(&sub);
        assert!(!sub.join("inner.zip").exists());
        assert_boundary_withheld(&mut f, id);
    }

    fn boundary_rows(f: &Fixture) -> (i64, i64) {
        f.sql()
            .query_row(
                "SELECT COUNT(*), COUNT(binding_json) FROM source_nested_boundaries",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }
    fn inject_unmount(trigger: &std::path::Path, root: &std::path::Path, skip: Option<&str>) {
        unsafe {
            std::env::set_var("EMUWIZ_FAULT_PATH", trigger);
            std::env::set_var("EMUWIZ_FAULT_ROOT", root);
            std::env::set_var("EMUWIZ_FAULT_UNMOUNT", "1");
            if let Some(skip) = skip {
                std::env::set_var("EMUWIZ_FAULT_PROBE_NUMBER", skip);
            }
        }
    }

    // Blocker 1: the mount vanishes after the walker saw it but before its
    // identity is accepted. The exposed parent must never become the boundary.
    // Hits of the mountpoint path: four walker/statx observations, then the
    // hardened opens of the pre-record check (5th) and post-record check (6th).
    #[test]
    #[ignore = "requires isolated mount namespace and process-local fault shim"]
    fn first_capture_race_before_recording_never_accepts_the_exposed_parent() {
        first_capture_race("5");
    }
    #[test]
    #[ignore = "requires isolated mount namespace and process-local fault shim"]
    fn first_capture_race_after_recording_never_accepts_the_exposed_parent() {
        first_capture_race("6");
    }
    fn first_capture_race(skip: &str) {
        assert_eq!(std::env::var("EMUWIZ_INJECT_FAULTS").as_deref(), Ok("1"));
        let (mut f, sub, id, _) = setup();
        inject_unmount(&sub, &sub, Some(skip));
        let s = f.scan();
        assert!(
            !sub.join("inner.zip").exists(),
            "fault must detach the mount"
        );
        assert_eq!(s.counts.archives_missing, 0);
        assert_eq!(f.flag(id), None);
        // Non-authoritative: the seen file is refused persistence (Failed) or the
        // boundary withholds reconciliation (Partial); never Complete.
        assert!(matches!(
            f.db.scan_coverage(s.scan_run_id).unwrap()[0].state,
            ScanCoverageState::Partial | ScanCoverageState::Failed
        ));
        assert!(
            s.folder_errors.iter().any(|(_, m)| m.contains("identity")),
            "{:?}",
            s.folder_errors
        );
        assert_eq!(boundary_rows(&f).1, 0, "no accepted identity may be stored");
        // The unproven boundary keeps protecting later scans.
        assert_boundary_withheld(&mut f, id);
        assert_eq!(boundary_rows(&f).1, 0);
    }

    // Blocker 3: the mount vanishes after continuity validation, before the
    // Missing write. Zero facts may be written.
    #[test]
    #[ignore = "requires isolated mount namespace and process-local fault shim"]
    fn write_time_disappearance_writes_no_missing_facts() {
        assert_eq!(std::env::var("EMUWIZ_INJECT_FAULTS").as_deref(), Ok("1"));
        assert_eq!(std::env::var("EMUWIZ_TEST_MOUNTS").as_deref(), Ok("1"));
        let mut f = Fixture::new(&["games"]);
        let sub = f.roots[0].join("sub");
        fs::create_dir(&sub).unwrap();
        mount_tmpfs(&sub);
        let scan = f.scan();
        assert_eq!(boundary_rows(&f), (1, 1));
        let (id, inner) = f.add(0, "sub/inner.zip");
        let source = f.db.load_archives().unwrap()[0].source_folder_id;
        inject_unmount(&inner, &sub, None);
        let result =
            f.db.mark_unseen_archives_missing(scan.scan_run_id, source, &[]);
        assert!(
            !sub.join("inner.zip").exists(),
            "fault must detach the mount"
        );
        assert!(result.is_err(), "{result:?}");
        assert_eq!(f.flag(id), None);
        let observations: i64 = f
            .sql()
            .query_row(
                "SELECT COUNT(*) FROM archive_scan_observations WHERE observation='missing'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(observations, 0);
    }

    // Blocker 4: preview agrees with persistence.
    #[test]
    #[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
    fn preview_under_a_discontinuous_boundary_is_not_authoritative_missing() {
        let (mut f, sub, id, _) = setup();
        f.scan();
        let (stale, _) = f.add(0, "stale.zip");
        f.stale(stale);
        let live = f.preview();
        assert!(live.diagnostics.is_empty());
        detach(&sub);
        let preview = f.preview();
        let row = preview.rows.iter().find(|r| r.archive.id == id).unwrap();
        assert_eq!(row.health, CatalogueHealth::NotChecked);
        assert_eq!(preview.counts.missing, 0);
        assert!(
            preview
                .diagnostics
                .iter()
                .any(|d| d.contains("nested filesystem boundary")),
            "{:?}",
            preview.diagnostics
        );
        // The independent safe repair is unaffected.
        assert_eq!(preview.counts.rows_would_change, 1);
        assert_eq!(f.flag(id), None);
    }

    fn bind_setup() -> (Fixture, PathBuf, PathBuf, i64) {
        assert_eq!(std::env::var("EMUWIZ_TEST_MOUNTS").as_deref(), Ok("1"));
        let mut f = Fixture::new(&["games"]);
        let store = f.temp.path().join("store");
        fs::create_dir(&store).unwrap();
        let sub = f.roots[0].join("sub");
        fs::create_dir(&sub).unwrap();
        bind(&store, &sub);
        let (id, _) = f.add(0, "sub/inner.zip");
        (f, store, sub, id)
    }

    // Blocker 2: a bind mount on the same device shares st_dev.
    #[test]
    #[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
    fn same_device_bind_mount_is_a_remembered_boundary() {
        use std::os::unix::fs::MetadataExt;
        let (mut f, store, sub, id) = bind_setup();
        assert_eq!(
            fs::metadata(&sub).unwrap().dev(),
            fs::metadata(f.roots[0].as_path()).unwrap().dev(),
            "fixture must be same-device"
        );
        // Present: unchanged, safe, recorded.
        let s = f.scan();
        assert_eq!(s.counts.archives_missing, 0);
        assert_eq!(f.flag(id), None);
        assert_eq!(boundary_rows(&f), (1, 1));
        // Detached.
        detach(&sub);
        assert_boundary_withheld(&mut f, id);
        // Replaced by a different directory on the same device.
        let other = f.temp.path().join("other");
        fs::create_dir(&other).unwrap();
        bind(&other, &sub);
        assert_boundary_withheld(&mut f, id);
        detach(&sub);
        // The original mount returning is safe.
        bind(&store, &sub);
        assert_eq!(f.scan().counts.archives_missing, 0);
        assert_eq!(f.flag(id), None);
    }

    #[test]
    #[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
    fn ordinary_directories_are_not_boundaries() {
        assert_eq!(std::env::var("EMUWIZ_TEST_MOUNTS").as_deref(), Ok("1"));
        let mut f = Fixture::new(&["games"]);
        let (id, path) = f.add(0, "plain/dir/inner.zip");
        assert_eq!(f.scan().counts.archives_missing, 0);
        assert_eq!(boundary_rows(&f), (0, 0));
        fs::remove_file(path).unwrap();
        let s = f.scan();
        assert_eq!(s.counts.archives_missing, 1);
        assert!(f.flag(id).is_some());
    }

    #[test]
    #[ignore = "run in an isolated user/mount namespace with EMUWIZ_TEST_MOUNTS=1"]
    fn remounting_the_same_filesystem_recovers_authority_safely() {
        assert_eq!(std::env::var("EMUWIZ_TEST_MOUNTS").as_deref(), Ok("1"));
        let mut f = Fixture::new(&["games"]);
        let store = f.temp.path().join("store");
        fs::create_dir(&store).unwrap();
        mount_tmpfs(&store);
        let sub = f.roots[0].join("sub");
        fs::create_dir(&sub).unwrap();
        bind(&store, &sub);
        let (keep, _) = f.add(0, "keep.zip");
        f.scan();
        // Removing a file outside the boundary is still legitimate evidence
        // while the nested filesystem is provably the same one.
        let (gone, gone_path) = f.add(0, "gone.zip");
        fs::remove_file(gone_path).unwrap();
        assert_eq!(f.scan().counts.archives_missing, 1);
        assert!(f.flag(gone).is_some());
        detach(&sub);
        let (later, later_path) = f.add(0, "later.zip");
        fs::remove_file(later_path).unwrap();
        assert_boundary_withheld(&mut f, later);
        bind(&store, &sub);
        let s = f.scan();
        assert_eq!(s.counts.archives_missing, 1, "{:?}", s.folder_errors);
        assert!(f.flag(later).is_some());
        assert_eq!(f.flag(keep), None);
    }
}

#[test]
fn ordinary_local_deletion_still_produces_missing_evidence() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "SNES/game.zip");
    f.scan();
    fs::remove_file(path).unwrap();
    let s = f.scan();
    assert_eq!(s.counts.archives_missing, 1);
    assert!(f.flag(id).is_some());
    assert_eq!(
        f.db.scan_coverage(s.scan_run_id).unwrap()[0].state,
        ScanCoverageState::Complete
    );
}
