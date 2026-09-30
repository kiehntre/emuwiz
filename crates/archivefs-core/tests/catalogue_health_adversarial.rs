use archivefs_core::catalogue_health::{CatalogueHealth, MoveEvidence, preview_catalogue_health};
use archivefs_core::{Archive, Config, Database, scan_and_persist};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

struct F {
    t: TempDir,
    db: Database,
    roots: Vec<PathBuf>,
}
impl F {
    fn new(names: &[&str]) -> Self {
        let t = tempfile::tempdir().unwrap();
        let roots: Vec<_> = names.iter().map(|n| t.path().join(n)).collect();
        for root in &roots {
            fs::create_dir_all(root).unwrap();
        }
        let mut db = Database::open_or_create(t.path().join("copy.sqlite3")).unwrap();
        db.register_source_folders(&roots).unwrap();
        Self { t, db, roots }
    }
    fn sql(&self) -> Connection {
        Connection::open(self.db.path()).unwrap()
    }
    fn add(&mut self, source: usize, rel: &str) -> (i64, PathBuf) {
        let p = self.roots[source].join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"legal fixture").unwrap();
        let a = Archive::from_path_in_root(&p, &self.roots[source]).unwrap();
        let source_id = self
            .db
            .list_source_folders()
            .unwrap()
            .into_iter()
            .find(|s| s.path == self.roots[source])
            .unwrap()
            .id;
        let id = self
            .db
            .upsert_archive(source_id, &self.roots[source], &a)
            .unwrap()
            .archive_id;
        (id, p)
    }
    fn scan(&mut self) -> archivefs_core::ScanPersistSummary {
        scan_and_persist(
            &mut self.db,
            &Config {
                source_folders: self.roots.clone(),
                mount_root: self.t.path().join("mounts"),
                ratarmount_bin: "ratarmount".into(),
                master_rom_root: None,
            },
            "independent-adversarial-fixture",
        )
        .unwrap()
    }
    fn flag(&self, id: i64) -> Option<String> {
        self.sql()
            .query_row(
                "SELECT last_verified_missing_at FROM archives WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn proof(&mut self, state: &str, source: usize) -> i64 {
        let run = self
            .db
            .start_scan_run("injected-coverage-proof", None)
            .unwrap();
        let folder = self
            .db
            .list_source_folders()
            .unwrap()
            .into_iter()
            .find(|s| s.path == self.roots[source])
            .unwrap();
        let m = fs::symlink_metadata(&folder.path).unwrap();
        self.sql().execute("INSERT INTO scan_source_coverage(scan_run_id,source_folder_id,state,excluded_roots_json,root_identity_json) VALUES(?1,?2,?3,'[]',?4)",params![run,folder.id,serde_json::to_string(state).unwrap(),serde_json::to_string(&(m.dev(),m.ino())).unwrap()]).unwrap();
        run
    }
}

#[test]
fn ancestor_symlink_outside_source_must_not_authorize_missing() {
    let mut f = F::new(&["games"]);
    let (id, _) = f.add(0, "SNES/game.zip");
    let old = f.roots[0].join("SNES");
    let saved = f.t.path().join("preserved-original-SNES");
    fs::rename(&old, &saved).unwrap();
    let outside = f.t.path().join("outside-empty");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, &old).unwrap();
    let scan = f.scan();
    eprintln!(
        "ancestor symlink: missing={}, coverage={:?}, preserved={}",
        scan.counts.archives_missing,
        f.db.scan_coverage(scan.scan_run_id).unwrap(),
        saved.join("game.zip").is_file()
    );
    assert_eq!(
        f.flag(id),
        None,
        "a symlink ancestor is not proof of absence in the owned source"
    );
}

#[test]
fn specialist_must_not_discover_a_configured_child_owned_namespace() {
    let mut f = F::new(&["games", "games/arcade/ownedset"]);
    fs::write(f.roots[1].join("chip.u1"), b"one").unwrap();
    fs::write(f.roots[1].join("chip.u2"), b"two").unwrap();
    f.add(0, "SNES/game.zip");
    f.add(0, "PS2/game.zip");
    let scan = f.scan();
    let rows = f.db.load_archives().unwrap();
    eprintln!(
        "specialist ownership: seen={}, rows={:?}",
        scan.counts.archives_seen,
        rows.iter()
            .map(|r| (&r.absolute_path, r.source_folder_id))
            .collect::<Vec<_>>()
    );
    let parent =
        f.db.list_source_folders()
            .unwrap()
            .into_iter()
            .find(|s| s.path == f.roots[0])
            .unwrap()
            .id;
    assert!(
        !rows
            .iter()
            .any(|r| r.source_folder_id == parent && r.absolute_path.starts_with(&f.roots[1])),
        "specialist bypassed explicit child exclusion"
    );
}

#[test]
fn ancestor_symlink_must_block_presence_reconciliation() {
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "SNES/game.zip");
    f.sql()
        .execute(
            "UPDATE archives SET last_verified_missing_at='old' WHERE id=?1",
            [id],
        )
        .unwrap();
    let original = p.parent().unwrap().to_path_buf();
    let outside = f.t.path().join("outside-original");
    fs::rename(&original, &outside).unwrap();
    symlink(&outside, &original).unwrap();
    let report = preview_catalogue_health(&f.db, &f.roots).unwrap();
    eprintln!(
        "ancestor symlink apply: health={:?}, changes={}",
        report.rows[0].health, report.counts.rows_would_change
    );
    assert_eq!(
        report.counts.rows_would_change, 0,
        "out-of-source traversal cannot prove a current owned file"
    );
}

#[test]
fn coverage_states_fail_closed_except_complete() {
    for state in [
        "partial",
        "unavailable",
        "failed",
        "skipped",
        "removed",
        "not_attempted",
        "complete",
    ] {
        let mut f = F::new(&["games"]);
        let (id, p) = f.add(0, "game.zip");
        fs::remove_file(p).unwrap();
        let run = f.proof(state, 0);
        let folder = f.db.list_source_folders().unwrap()[0].id;
        assert_eq!(
            f.db.mark_unseen_archives_missing(run, folder, &[]).unwrap(),
            i64::from(state == "complete")
        );
        assert_eq!(f.flag(id).is_some(), state == "complete");
    }
}

#[test]
fn legitimate_root_replacement_is_reprovable_on_new_scan() {
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "game.zip");
    fs::remove_file(p).unwrap();
    let old_run = f.proof("complete", 0);
    let folder = f.db.list_source_folders().unwrap()[0].id;
    let saved = f.t.path().join("old-root");
    fs::rename(&f.roots[0], &saved).unwrap();
    fs::create_dir(&f.roots[0]).unwrap();
    assert!(
        f.db.mark_unseen_archives_missing(old_run, folder, &[])
            .is_err()
    );
    assert_eq!(f.flag(id), None);
    assert_eq!(
        f.scan().counts.archives_missing,
        1,
        "new scan must establish new identity instead of permanent mismatch"
    );
}

#[test]
fn wrong_root_already_in_place_before_fresh_scan_must_not_poison_prior_membership() {
    let mut f = F::new(&["games"]);
    let (id, _) = f.add(0, "SNES/original.zip");
    f.scan();
    let saved = f.t.path().join("original-device-preserved");
    fs::rename(&f.roots[0], &saved).unwrap();
    fs::create_dir(&f.roots[0]).unwrap();
    fs::write(
        f.roots[0].join("unrelated.zip"),
        b"different disk namespace",
    )
    .unwrap();
    let s = f.scan();
    eprintln!(
        "new root before scan: prior file still exists={},missing={},coverage={:?}",
        saved.join("SNES/original.zip").is_file(),
        s.counts.archives_missing,
        f.db.scan_coverage(s.scan_run_id).unwrap()
    );
    assert_eq!(
        f.flag(id),
        None,
        "capturing a replacement's identity at scan start does not establish continuity with the catalogue's device"
    );
}

#[test]
fn unreadable_owned_child_is_partial_and_preserves_unseen() {
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "PS2/game.zip");
    fs::remove_file(p).unwrap();
    let child = f.roots[0].join("PS2");
    fs::set_permissions(&child, fs::Permissions::from_mode(0)).unwrap();
    let s = f.scan();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(s.counts.archives_missing, 0);
    assert_eq!(f.flag(id), None);
    assert!(
        f.db.scan_coverage(s.scan_run_id)
            .unwrap()
            .iter()
            .any(|c| format!("{:?}", c.state) == "Partial")
    );
}

#[test]
fn child_unavailable_does_not_poison_parent_owned_snes_ps2() {
    let mut f = F::new(&["games", "games/nested"]);
    let (a, _) = f.add(0, "SNES/game.zip");
    let (b, _) = f.add(0, "PS2/game.zip");
    let (c, p) = f.add(1, "game.zip");
    fs::remove_file(p).unwrap();
    fs::remove_dir(&f.roots[1]).unwrap();
    let s = f.scan();
    assert_eq!(s.counts.archives_missing, 0);
    assert_eq!(f.flag(a), None);
    assert_eq!(f.flag(b), None);
    assert_eq!(f.flag(c), None);
}

#[test]
fn duplicate_nested_roots_are_rejected_before_scan_mutation() {
    let mut f = F::new(&["games", "games/nested"]);
    f.roots.push(f.roots[1].clone());
    let before = f.db.load_archives().unwrap();
    let config = Config {
        source_folders: f.roots.clone(),
        mount_root: f.t.path().join("mounts"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    assert!(scan_and_persist(&mut f.db, &config, "duplicate").is_err());
    assert_eq!(f.db.load_archives().unwrap(), before);
}

#[test]
fn hostile_same_name_platforms_never_become_identity() {
    let mut f = F::new(&["games"]);
    let (_, p) = f.add(0, "SNES/Game.zip");
    let (_, _) = f.add(0, "PS2/Game.zip");
    let (_, _) = f.add(0, "arcade/game.zip");
    fs::remove_file(p).unwrap();
    let r = preview_catalogue_health(&f.db, &f.roots).unwrap();
    let missing = r
        .rows
        .iter()
        .find(|x| x.archive.relative_path == Path::new("SNES/Game.zip"))
        .unwrap();
    assert_eq!(missing.health, CatalogueHealth::PossiblyMoved);
    assert_eq!(missing.move_candidates.len(), 2);
    assert!(
        missing
            .move_candidates
            .iter()
            .all(|m| m.evidence != MoveEvidence::VerifiedSha256 && !m.same_platform)
    );
    assert!(missing.archive.identity_report.is_none());
}

#[test]
fn changed_source_membership_between_preview_apply_must_be_refused() {
    let mut f = F::new(&["games", "different-source"]);
    let (id, _) = f.add(0, "game.zip");
    f.sql()
        .execute(
            "UPDATE archives SET last_verified_missing_at='old' WHERE id=?1",
            [id],
        )
        .unwrap();
    let report = preview_catalogue_health(&f.db, &f.roots).unwrap();
    let other =
        f.db.list_source_folders()
            .unwrap()
            .into_iter()
            .find(|s| s.path == f.roots[1])
            .unwrap()
            .id;
    f.sql()
        .execute(
            "UPDATE archives SET source_folder_id=?2 WHERE id=?1",
            params![id, other],
        )
        .unwrap();
    assert!(
        f.db.apply_presence_reconciliation(&report).is_err(),
        "source ownership drift must invalidate preview"
    );
    assert!(f.flag(id).is_some());
}

#[test]
fn changed_archive_kind_between_preview_apply_must_be_refused() {
    let mut f = F::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    f.sql()
        .execute(
            "UPDATE archives SET last_verified_missing_at='old' WHERE id=?1",
            [id],
        )
        .unwrap();
    let report = preview_catalogue_health(&f.db, &f.roots).unwrap();
    f.sql()
        .execute(
            "UPDATE archives SET archive_kind='arcade_set_directory' WHERE id=?1",
            [id],
        )
        .unwrap();
    let current = preview_catalogue_health(&f.db, &f.roots).unwrap();
    assert_eq!(current.rows[0].health, CatalogueHealth::NotChecked);
    assert_eq!(current.counts.rows_would_change, 0);
    assert!(
        f.db.apply_presence_reconciliation(&report).is_err(),
        "a file cannot prove presence after this catalogue row becomes a directory representation"
    );
    assert!(f.flag(id).is_some());
}

#[test]
#[ignore = "requires a process-local LD_PRELOAD fault shim; run separately"]
fn source_disappearing_after_root_preflight_must_preserve_evidence() {
    if std::env::var("EMUWIZ_INJECT_FAULTS").as_deref() != Ok("1") {
        return;
    }
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "game.zip");
    let run = f.proof("complete", 0);
    let folder = f.db.list_source_folders().unwrap()[0].id;
    let original = fs::metadata(&f.roots[0]).unwrap();
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_PATH", &p);
        std::env::set_var("EMUWIZ_FAULT_ROOT", &f.roots[0]);
    }
    let result = f.db.mark_unseen_archives_missing(run, folder, &[]);
    unsafe {
        std::env::remove_var("EMUWIZ_FAULT_PATH");
        std::env::remove_var("EMUWIZ_FAULT_ROOT");
    }
    let current = fs::metadata(&f.roots[0]).unwrap();
    assert_ne!(
        (original.dev(), original.ino()),
        (current.dev(), current.ino()),
        "fault did not fire"
    );
    eprintln!(
        "root changed AFTER preflight: result={result:?}, missing_flag={:?}",
        f.flag(id)
    );
    assert_eq!(
        f.flag(id),
        None,
        "a vanished/replaced mount after preflight must fail closed"
    );
}

#[test]
#[ignore = "requires a process-local LD_PRELOAD fault shim; run separately"]
fn io_failure_halfway_through_enumeration_is_partial() {
    if std::env::var("EMUWIZ_INJECT_FAULTS").as_deref() != Ok("1") {
        return;
    }
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "removed.zip");
    fs::remove_file(p).unwrap();
    for n in 0..12 {
        fs::write(f.roots[0].join(format!("payload-{n}.zip")), b"game").unwrap();
    }
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_ENUM", &f.roots[0]);
    }
    let s = f.scan();
    unsafe {
        std::env::remove_var("EMUWIZ_FAULT_ENUM");
    }
    eprintln!(
        "mid-enumeration I/O: errors={},coverage={:?}",
        s.counts.errors_count,
        f.db.scan_coverage(s.scan_run_id).unwrap()
    );
    assert!(s.counts.errors_count > 0, "fault did not fire");
    assert_eq!(f.flag(id), None);
}

#[test]
#[ignore = "requires a process-local LD_PRELOAD fault shim; run separately"]
fn specialist_io_failure_then_parent_success_is_partial() {
    if std::env::var("EMUWIZ_INJECT_FAULTS").as_deref() != Ok("1") {
        return;
    }
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "SNES/removed.zip");
    fs::remove_file(p).unwrap();
    for n in 0..5 {
        let d = f.roots[0].join(format!("arcade/set{n}"));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("chip.u1"), b"one").unwrap();
        fs::write(d.join("chip.u2"), b"two").unwrap();
    }
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_ENUM", f.roots[0].join("arcade"));
    }
    let s = f.scan();
    unsafe {
        std::env::remove_var("EMUWIZ_FAULT_ENUM");
    }
    eprintln!(
        "specialist failed parent succeeded: errors={},missing={}",
        s.counts.errors_count, s.counts.archives_missing
    );
    assert!(s.counts.errors_count > 0, "fault did not fire");
    assert_eq!(f.flag(id), None);
}

#[test]
#[ignore = "requires a process-local LD_PRELOAD fault shim; run separately"]
fn parent_io_failure_after_specialist_success_is_partial() {
    if std::env::var("EMUWIZ_INJECT_FAULTS").as_deref() != Ok("1") {
        return;
    }
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "SNES/removed.zip");
    fs::remove_file(p).unwrap();
    let d = f.roots[0].join("arcade/set0");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("chip.u1"), b"one").unwrap();
    fs::write(d.join("chip.u2"), b"two").unwrap();
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_ENUM", &f.roots[0]);
    }
    let s = f.scan();
    unsafe {
        std::env::remove_var("EMUWIZ_FAULT_ENUM");
    }
    eprintln!(
        "parent failed after specialist success: errors={},missing={}",
        s.counts.errors_count, s.counts.archives_missing
    );
    assert!(s.counts.errors_count > 0, "fault did not fire");
    assert_eq!(f.flag(id), None);
}

#[test]
#[ignore = "requires a process-local LD_PRELOAD fault shim; run separately"]
fn file_metadata_io_failure_must_make_coverage_partial() {
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "gone.zip");
    fs::remove_file(p).unwrap();
    let (_, unreadable) = f.add(0, "metadata-error.zip");
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_STAT_EIO", &unreadable);
    }
    assert!(fs::metadata(&unreadable).is_err(), "fault did not fire");
    let scan = f.scan();
    unsafe {
        std::env::remove_var("EMUWIZ_FAULT_STAT_EIO");
    }
    eprintln!(
        "file metadata EIO: errors={},missing={},coverage={:?}",
        scan.counts.errors_count,
        scan.counts.archives_missing,
        f.db.scan_coverage(scan.scan_run_id).unwrap()
    );
    assert_eq!(
        f.flag(id),
        None,
        "enumeration cannot be Complete after a metadata I/O failure"
    );
}

#[test]
fn size_only_is_not_a_move_and_strong_hash_is_separate_from_weak_duplicates() {
    let mut f = F::new(&["games", "another-source"]);
    let (old, p) = f.add(0, "SNES/old.zip");
    let (renamed, _) = f.add(1, "PS2/renamed.zip");
    let hash: String = Sha256::digest(fs::read(&p).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    fs::remove_file(p).unwrap();
    let r = preview_catalogue_health(&f.db, &f.roots).unwrap();
    let gone = r.rows.iter().find(|r| r.archive.id == old).unwrap();
    assert_eq!(gone.health, CatalogueHealth::Missing);
    assert!(
        gone.move_candidates.is_empty(),
        "size alone is not identity"
    );
    f.sql()
        .execute(
            "UPDATE archives SET archive_hash=?2 WHERE id IN(?1,?3)",
            params![old, hash, renamed],
        )
        .unwrap();
    let (weak, w) = f.add(1, "arcade/old.zip");
    fs::write(w, b"other payload").unwrap();
    for (id, platform) in [(old, "SNES"), (renamed, "PS2"), (weak, "Arcade")] {
        f.sql().execute("INSERT INTO platform_assignments(archive_id,platform,source,assigned_at) VALUES(?1,?2,'fixture','now')",params![id,platform]).unwrap();
    }
    let before = f.db.load_archives().unwrap();
    let r = preview_catalogue_health(&f.db, &f.roots).unwrap();
    let gone = r.rows.iter().find(|r| r.archive.id == old).unwrap();
    assert_eq!(gone.health, CatalogueHealth::PossiblyMoved);
    assert_eq!(gone.move_candidates.len(), 2);
    assert!(gone.move_candidates.iter().any(|c| c.archive_id == renamed
        && c.evidence == MoveEvidence::VerifiedSha256
        && !c.basename_exact
        && !c.same_platform
        && !c.same_source));
    assert!(
        gone.move_candidates
            .iter()
            .any(|c| c.archive_id == weak && c.evidence != MoveEvidence::VerifiedSha256)
    );
    assert_eq!(r.counts.ambiguous_move_candidates, 1);
    assert_eq!(f.db.load_archives().unwrap(), before);
}

#[test]
fn present_identity_mismatch_is_neutral_and_old_verified_absence_is_not_present_verified() {
    use archivefs_core::game_identity::{
        GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
        IdentityKind, IdentityPlatform, IdentityProvenance, IdentityStatus,
    };
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "PS2/game.iso");
    let identity = GameIdentityReport {
        archive_path: p.clone(),
        platform: IdentityPlatform::PlayStation2,
        format: IdentityImageFormat::Iso,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::Ps2Serial,
            status: IdentityStatus::Verified,
            value: Some("SLUS-12345".into()),
            confidence: IdentityConfidence::ExactBytes,
            provenance: IdentityProvenance {
                archive_path: p.clone(),
                member_path: None,
                member_index: None,
                method: "fixture exact bytes".into(),
            },
            diagnostic: "synthetic".into(),
        }],
        warnings: vec![],
        bytes_read: 13,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    f.sql().execute("UPDATE archives SET identity_report_json=?2,identity_report_size_bytes=size_bytes,identity_report_modified_time_unix_seconds=modified_time_unix_seconds,last_verified_missing_at='stale' WHERE id=?1",params![id,serde_json::to_vec(&identity).unwrap()]).unwrap();
    assert_eq!(
        preview_catalogue_health(&f.db, &f.roots).unwrap().rows[0].health,
        CatalogueHealth::PresentVerified
    );
    fs::write(&p, b"a replacement with a different byte length").unwrap();
    assert_eq!(
        preview_catalogue_health(&f.db, &f.roots).unwrap().rows[0].health,
        CatalogueHealth::PresentNotVerified
    );
    fs::remove_file(&p).unwrap();
    assert_eq!(
        preview_catalogue_health(&f.db, &f.roots).unwrap().rows[0].health,
        CatalogueHealth::Missing
    );
}

#[test]
fn orphan_has_precedence_over_present_stale_and_move_hint() {
    let mut f = F::new(&["removed", "games"]);
    let (id, p) = f.add(0, "game.zip");
    f.add(1, "game.zip");
    f.sql()
        .execute(
            "UPDATE archives SET last_verified_missing_at='stale' WHERE id=?1",
            [id],
        )
        .unwrap();
    let r = preview_catalogue_health(&f.db, &f.roots[1..]).unwrap();
    let old = r.rows.iter().find(|r| r.archive.id == id).unwrap();
    assert_eq!(old.health, CatalogueHealth::OrphanedSource);
    assert!(old.observation.is_present());
    fs::remove_file(p).unwrap();
    let r = preview_catalogue_health(&f.db, &f.roots[1..]).unwrap();
    let old = r.rows.iter().find(|r| r.archive.id == id).unwrap();
    assert_eq!(old.health, CatalogueHealth::OrphanedSource);
    assert_eq!(old.move_candidates.len(), 1);
    assert!(f.flag(id).is_some());
}

/// Public scan path. The fault is armed by the two hardened observe_owned
/// probes and fires at the next resolution of the file: catalogue revalidation.
fn post_probe_scan_attack(mode: &str) {
    if std::env::var("EMUWIZ_INJECT_FAULTS").as_deref() != Ok("1") {
        return;
    }
    let mut f = F::new(&["games"]);
    let (id, file) = f.add(0, "SNES/game.zip");
    let parent = f.roots[0].join("SNES");
    let moved = if mode == "recreate" {
        f.roots[0].join("SNES.moved")
    } else {
        f.t.path().join("preserved-original")
    };
    f.sql()
        .execute(
            "UPDATE archives SET last_verified_missing_at='must-preserve' WHERE id=?1",
            [id],
        )
        .unwrap();
    let observations = |f: &F| -> i64 {
        f.sql()
            .query_row(
                "SELECT count(*) FROM archive_scan_observations WHERE archive_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = observations(&f);
    let (trigger, moved_file) = match mode {
        "leaf" => (file.clone(), moved.clone()),
        _ => (file.clone(), moved.join("game.zip")),
    };
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_PATH", &trigger);
        std::env::set_var("EMUWIZ_FAULT_ROOT", &f.roots[0]);
        std::env::set_var("EMUWIZ_FAULT_ARM_OPENAT2", "2");
        match mode {
            "symlink" => {
                std::env::set_var("EMUWIZ_FAULT_PARENT_LINK", &parent);
                std::env::set_var("EMUWIZ_FAULT_LINK_TARGET", &moved);
            }
            "recreate" => std::env::set_var("EMUWIZ_FAULT_PARENT_LINK", &parent),
            _ => std::env::set_var("EMUWIZ_FAULT_LEAF_LINK", &moved),
        }
        if mode == "recreate" {
            std::env::set_var("EMUWIZ_FAULT_RECREATE", "1");
        }
    }
    let result = scan_and_persist(
        &mut f.db,
        &Config {
            source_folders: f.roots.clone(),
            mount_root: f.t.path().join("mounts"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        },
        "post-probe-attack",
    );
    for k in [
        "EMUWIZ_FAULT_PATH",
        "EMUWIZ_FAULT_ROOT",
        "EMUWIZ_FAULT_ARM_OPENAT2",
        "EMUWIZ_FAULT_PARENT_LINK",
        "EMUWIZ_FAULT_LINK_TARGET",
        "EMUWIZ_FAULT_LEAF_LINK",
        "EMUWIZ_FAULT_RECREATE",
    ] {
        unsafe { std::env::remove_var(k) };
    }
    let changed = match mode {
        "leaf" => fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink(),
        _ => fs::symlink_metadata(&parent).is_ok() && moved.exists(),
    };
    assert!(changed, "fault did not fire");
    assert!(moved_file.exists(), "original content must be preserved");
    assert_eq!(
        f.flag(id).as_deref(),
        Some("must-preserve"),
        "post-probe replacement must not clear Missing evidence"
    );
    assert_eq!(observations(&f), before, "no restoration observation");
    match result {
        Ok(summary) => {
            assert!(summary.counts.errors_count > 0 && !summary.folder_errors.is_empty());
            assert_eq!(summary.counts.archives_restored, 0);
        }
        Err(_) => {}
    }
}

#[test]
#[ignore = "requires a process-local LD_PRELOAD fault shim; run separately"]
fn ancestor_symlink_after_probe_cannot_restore_missing_via_scan_persistence() {
    post_probe_scan_attack("symlink");
}

#[test]
#[ignore = "requires a process-local LD_PRELOAD fault shim; run separately"]
fn ancestor_recreated_after_probe_cannot_restore_missing_via_scan_persistence() {
    post_probe_scan_attack("recreate");
}

#[test]
#[ignore = "requires a process-local LD_PRELOAD fault shim; run separately"]
fn leaf_symlink_after_probe_cannot_restore_missing_via_scan_persistence() {
    post_probe_scan_attack("leaf");
}
