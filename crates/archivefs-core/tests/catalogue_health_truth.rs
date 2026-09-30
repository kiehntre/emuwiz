use archivefs_core::catalogue_health::{
    CatalogueHealth, MoveEvidence, ScanCoverageState, preview_catalogue_health,
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
    f.sql().execute_batch("DROP TABLE scan_source_coverage; DELETE FROM schema_migrations WHERE version=22; PRAGMA user_version=21;").unwrap();
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
