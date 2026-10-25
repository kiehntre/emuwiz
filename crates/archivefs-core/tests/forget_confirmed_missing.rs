//! Synthetic temp databases only. Every refusal must leave rows and files untouched.
archivefs_core::install_test_environment!();

use archivefs_core::catalogue_health::{FORGET_PLAN_STALE, MissingClassification as C};
use archivefs_core::{Archive, Config, Database, scan_and_persist};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct F {
    temp: TempDir,
    db: Database,
    roots: Vec<PathBuf>,
}
impl F {
    fn new(names: &[&str]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let roots: Vec<_> = names.iter().map(|n| temp.path().join(n)).collect();
        for r in &roots {
            fs::create_dir_all(r).unwrap();
        }
        let mut db = Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
        db.register_source_folders(&roots).unwrap();
        Self { temp, db, roots }
    }
    fn sql(&self) -> Connection {
        Connection::open(self.db.path()).unwrap()
    }
    fn add(&mut self, source: usize, rel: &str) -> (i64, PathBuf) {
        let path = self.roots[source].join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"rom bytes").unwrap();
        let archive = Archive::from_path_in_root(&path, &self.roots[source]).unwrap();
        let sid = self
            .db
            .list_source_folders()
            .unwrap()
            .iter()
            .find(|s| s.path == self.roots[source])
            .unwrap()
            .id;
        let id = self
            .db
            .upsert_archive(sid, &self.roots[source], &archive)
            .unwrap()
            .archive_id;
        (id, path)
    }
    fn scan(&mut self) {
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
        .unwrap();
    }
    fn plan(&self) -> archivefs_core::catalogue_health::ForgetMissingPlan {
        self.db
            .preview_forget_confirmed_missing(&self.roots)
            .unwrap()
    }
    fn class(&self, id: i64) -> Option<C> {
        self.plan()
            .entries
            .iter()
            .find(|e| e.archive_id == id)
            .map(|e| e.classification)
    }
    fn count(&self) -> i64 {
        self.sql()
            .query_row("SELECT count(*) FROM archives", [], |r| r.get(0))
            .unwrap()
    }
    /// One source, scanned, then `gone.zip` deleted and scanned again: confirmed missing.
    fn confirmed() -> (Self, i64, i64, PathBuf) {
        let mut f = Self::new(&["games"]);
        let (gone, gp) = f.add(0, "gone.zip");
        let (kept, _) = f.add(0, "kept.zip");
        f.scan();
        fs::remove_file(&gp).unwrap();
        f.scan();
        (f, gone, kept, gp)
    }
    fn files(&self) -> Vec<PathBuf> {
        fn walk(p: &Path, out: &mut Vec<PathBuf>) {
            for e in fs::read_dir(p).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        for r in self.roots.iter().filter(|r| r.is_dir()) {
            walk(r, &mut out);
        }
        out.sort();
        out
    }
    fn receipts(&self) -> Vec<PathBuf> {
        fs::read_dir(self.temp.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains("forget-missing"))
            .collect()
    }
}

#[test]
fn healthy_scan_and_absent_file_is_eligible_only_for_the_missing_one() {
    let (f, gone, kept, _) = F::confirmed();
    let plan = f.plan();
    assert_eq!(plan.counts.confirmed_missing, 1);
    assert_eq!(f.class(gone), Some(C::ConfirmedMissing));
    assert_eq!(f.class(kept), None); // NotMissing rows are counted, not listed
    let e = &plan.entries[0];
    assert!(e.evidence.is_some() && e.undo_restorable);
    assert!(e.removes.scan_observations >= 1);
}

#[test]
fn rediscovered_file_is_not_eligible() {
    let (mut f, gone, _, gp) = F::confirmed();
    fs::write(&gp, b"rom bytes").unwrap();
    assert_eq!(f.class(gone), None);
    f.scan();
    assert_eq!(f.plan().counts.confirmed_missing, 0);
}

#[test]
fn unavailable_source_refuses() {
    let (f, gone, _, _) = F::confirmed();
    fs::rename(&f.roots[0], f.temp.path().join("offline")).unwrap();
    assert_eq!(f.class(gone), Some(C::SourceUnavailable));
    assert_eq!(f.plan().counts.confirmed_missing, 0);
}

#[test]
fn disabled_source_refuses_and_reenable_without_scan_still_refuses() {
    let (mut f, gone, _, _) = F::confirmed();
    f.db.sync_source_enablement(&[(f.roots[0].clone(), false)])
        .unwrap();
    assert_eq!(f.class(gone), Some(C::SourceUnavailable));
    f.db.sync_source_enablement(&[(f.roots[0].clone(), true)])
        .unwrap();
    assert_eq!(f.class(gone), Some(C::ScanIncomplete));
    f.scan();
    assert_eq!(f.class(gone), Some(C::ConfirmedMissing));
}

#[test]
fn partial_and_aborted_scans_refuse() {
    let (mut f, gone, _, _) = F::confirmed();
    let offline = f.temp.path().join("offline");
    fs::rename(&f.roots[0], &offline).unwrap();
    f.scan(); // source unreachable: partial
    fs::rename(&offline, &f.roots[0]).unwrap();
    assert_eq!(f.class(gone), Some(C::ScanIncomplete));
    // aborted: the root is replaced by a file while scanning
    fs::rename(&f.roots[0], &offline).unwrap();
    fs::write(&f.roots[0], b"not a dir").unwrap();
    f.scan();
    fs::remove_file(&f.roots[0]).unwrap();
    fs::rename(&offline, &f.roots[0]).unwrap();
    assert_eq!(f.class(gone), Some(C::ScanIncomplete));
}

#[test]
fn never_scanned_source_refuses() {
    let mut f = F::new(&["games"]);
    let (id, p) = f.add(0, "game.zip");
    fs::remove_file(p).unwrap();
    assert_ne!(f.class(id), Some(C::ConfirmedMissing));
    assert_eq!(f.plan().counts.confirmed_missing, 0);
}

#[test]
fn possible_move_is_never_forgotten() {
    let (mut f, gone, _, _) = F::confirmed();
    fs::create_dir_all(f.roots[0].join("sub")).unwrap();
    fs::write(f.roots[0].join("sub/gone.zip"), b"rom bytes").unwrap();
    f.scan();
    assert_eq!(f.class(gone), Some(C::PossiblyMoved));
    let plan = f.plan();
    let before = f.count();
    assert_eq!(
        f.db.apply_forget_confirmed_missing(&plan)
            .unwrap()
            .forgotten,
        0
    );
    assert_eq!(f.count(), before);
}

#[test]
fn stale_plans_are_refused_and_change_nothing() {
    // after a new scan
    let (mut f, _, _, _) = F::confirmed();
    let plan = f.plan();
    f.scan();
    let err =
        f.db.apply_forget_confirmed_missing(&plan)
            .unwrap_err()
            .to_string();
    assert!(err.contains(FORGET_PLAN_STALE), "{err}");
    // after a source state change (enablement is not covered by the epoch)
    let plan = f.plan();
    f.db.sync_source_enablement(&[(f.roots[0].clone(), false)])
        .unwrap();
    assert!(
        f.db.apply_forget_confirmed_missing(&plan)
            .unwrap_err()
            .to_string()
            .contains(FORGET_PLAN_STALE)
    );
    f.db.sync_source_enablement(&[(f.roots[0].clone(), true)])
        .unwrap();
    f.scan();
    // after the game is rediscovered
    let plan = f.plan();
    assert_eq!(plan.counts.confirmed_missing, 1);
    fs::write(f.roots[0].join("gone.zip"), b"rom bytes").unwrap();
    assert!(
        f.db.apply_forget_confirmed_missing(&plan)
            .unwrap_err()
            .to_string()
            .contains(FORGET_PLAN_STALE)
    );
    assert_eq!(f.count(), 2);
    assert!(f.receipts().is_empty());
}

#[test]
fn bulk_forgets_only_confirmed_rows_and_never_touches_files() {
    let mut f = F::new(&["a", "b"]);
    let mut gone = Vec::new();
    for n in 0..3 {
        let (id, p) = f.add(0, &format!("g{n}.zip"));
        gone.push((id, p));
    }
    let (moved, mp) = f.add(0, "moved.zip");
    let (offline, op) = f.add(1, "offline.zip");
    let (kept, _) = f.add(0, "kept.zip");
    fs::create_dir_all(f.roots[0].join("saves")).unwrap();
    for extra in ["saves/g0.sav", "g0.pdf", "g0.png"] {
        fs::write(f.roots[0].join(extra), b"keep me").unwrap();
    }
    f.scan();
    for (_, p) in &gone {
        fs::remove_file(p).unwrap();
    }
    fs::rename(&mp, f.roots[0].join("saves/moved.zip")).unwrap();
    fs::remove_file(op).unwrap();
    f.scan();
    fs::rename(&f.roots[1], f.temp.path().join("offline")).unwrap();
    let plan = f.plan();
    assert_eq!(plan.counts.confirmed_missing, 3);
    assert_eq!(plan.counts.possibly_moved, 1);
    assert_eq!(plan.counts.source_unavailable, 1);
    assert_eq!(f.class(moved), Some(C::PossiblyMoved));
    assert_eq!(f.class(offline), Some(C::SourceUnavailable));
    let files = f.files();
    let result = f.db.apply_forget_confirmed_missing(&plan).unwrap();
    assert_eq!(result.forgotten, 3);
    assert_eq!(f.files(), files);
    let ids: Vec<i64> = f.db.load_archives().unwrap().iter().map(|a| a.id).collect();
    assert!(gone.iter().all(|(id, _)| !ids.contains(id)));
    assert!(ids.contains(&moved) && ids.contains(&offline) && ids.contains(&kept));
    for extra in ["saves/g0.sav", "g0.pdf", "g0.png", "kept.zip"] {
        assert!(f.roots[0].join(extra).exists());
    }
}

#[test]
fn failed_transaction_rolls_back_everything() {
    let (mut f, gone, _, _) = F::confirmed();
    let plan = f.plan();
    let before = f.db.load_archives().unwrap();
    let obs: i64 = f
        .sql()
        .query_row("SELECT count(*) FROM archive_scan_observations", [], |r| {
            r.get(0)
        })
        .unwrap();
    f.sql()
        .execute_batch(
            "CREATE TRIGGER boom BEFORE DELETE ON archives BEGIN SELECT RAISE(ABORT,'boom'); END;",
        )
        .unwrap();
    assert!(f.db.apply_forget_confirmed_missing(&plan).is_err());
    assert_eq!(f.db.load_archives().unwrap(), before);
    let after: i64 = f
        .sql()
        .query_row("SELECT count(*) FROM archive_scan_observations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(after, obs);
    assert!(f.receipts().is_empty());
    f.sql().execute_batch("DROP TRIGGER boom").unwrap();
    assert_eq!(f.class(gone), Some(C::ConfirmedMissing));
}

#[test]
fn undo_restores_exactly_and_repeat_undo_is_refused() {
    let (mut f, gone, _, _) = F::confirmed();
    let before = f.db.load_archives().unwrap();
    let rows = |f: &F| -> Vec<i64> {
        ["archive_scan_observations", "platform_assignments"]
            .iter()
            .map(|t| {
                f.sql()
                    .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                    .unwrap()
            })
            .collect()
    };
    let child_before = rows(&f);
    let result = f.db.apply_forget_confirmed_missing(&f.plan()).unwrap();
    assert_eq!(f.class(gone), None);
    let receipt = result.receipt_path.unwrap();
    assert_eq!(f.db.undo_forget_missing(&receipt).unwrap(), 1);
    assert_eq!(f.db.load_archives().unwrap(), before);
    assert_eq!(rows(&f), child_before);
    assert!(
        f.db.undo_forget_missing(&receipt)
            .unwrap_err()
            .to_string()
            .contains("already undone")
    );
    assert_eq!(f.count(), 2);
}

#[test]
fn undo_refuses_when_state_changed_and_restores_nothing() {
    let (mut f, gone, _, gp) = F::confirmed();
    let receipt =
        f.db.apply_forget_confirmed_missing(&f.plan())
            .unwrap()
            .receipt_path
            .unwrap();
    // the path is catalogued again by a new scan
    fs::write(&gp, b"rom bytes").unwrap();
    f.scan();
    let count = f.count();
    let err = f.db.undo_forget_missing(&receipt).unwrap_err().to_string();
    assert!(err.contains("occupied"), "{err}");
    assert_eq!(f.count(), count);
    assert!(
        f.db.load_archives()
            .unwrap()
            .iter()
            .filter(|a| a.id == gone)
            .all(|a| a.last_verified_missing_at.is_none())
    );
}

#[test]
fn interrupted_forget_is_recoverable_from_the_receipt() {
    let (mut f, _, _, _) = F::confirmed();
    let before = f.db.load_archives().unwrap();
    let receipt =
        f.db.apply_forget_confirmed_missing(&f.plan())
            .unwrap()
            .receipt_path
            .unwrap();
    // crash after commit but before the receipt was marked: still "pending"
    let text = fs::read_to_string(&receipt)
        .unwrap()
        .replace("\"applied\"", "\"pending\"");
    fs::write(&receipt, text).unwrap();
    assert_eq!(f.db.undo_forget_missing(&receipt).unwrap(), 1);
    assert_eq!(f.db.load_archives().unwrap(), before);
    // crash before the delete: pending receipt, rows still present -> nothing to restore
    let text = fs::read_to_string(&receipt)
        .unwrap()
        .replace("\"undone\"", "\"pending\"");
    fs::write(&receipt, text).unwrap();
    assert!(f.db.undo_forget_missing(&receipt).is_err());
    assert_eq!(f.db.load_archives().unwrap(), before);
}

#[test]
fn preview_performs_no_writes() {
    let (f, _, _, _) = F::confirmed();
    let bytes = fs::read(f.db.path()).unwrap();
    let dir = |f: &F| -> Vec<_> {
        let mut v: Vec<_> = fs::read_dir(f.temp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        v.sort();
        v
    };
    let listing = dir(&f);
    let a = f.plan();
    let ro = Database::open_catalogue_health_read_only(f.db.path()).unwrap();
    let b = ro.preview_forget_confirmed_missing(&f.roots).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.token(), b.token());
    assert_eq!(fs::read(f.db.path()).unwrap(), bytes);
    assert_eq!(dir(&f), listing);
}
