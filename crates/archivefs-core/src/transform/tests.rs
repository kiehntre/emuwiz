//! Adversarial tests of the transformation engine on real filesystems.
//!
//! Every fixture is disposable: two library roots (on two genuinely separate
//! ext4 filesystems for the cross-filesystem tests, see `second_filesystem`;
//! without one those tests say so and are skipped), a fresh catalogue and a
//! private journal root. Nothing here touches a real library or a real
//! mount: storage changes (read-only remounts, vanished mounts, changed
//! options) are simulated through `fscompat::test_override`.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::engine::*;
use super::fault::{Action, FaultPlan, Point};
use super::model::*;
use super::*;

const SMALL: usize = 64 * 1024;

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32).wrapping_mul(2_654_435_761).to_le_bytes()[seed as usize % 4] ^ seed)
        .collect()
}

fn read(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn template_catalogue() -> &'static Path {
    static TEMPLATE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        let path = dir.join("template.sqlite3");
        drop(crate::Database::open_or_create(&path).unwrap());
        for suffix in ["-wal", "-shm", "-journal"] {
            let side = dir.join(format!("template.sqlite3{suffix}"));
            assert!(!side.exists(), "template catalogue left {}", side.display());
        }
        path
    })
}

/// A folder on a second, genuinely separate *supported* (ext4) filesystem
/// for the cross-filesystem fixtures: `EMUWIZ_OUROBOROS_SECOND_FS` if set,
/// else a folder beside this test binary (the Cargo target directory),
/// when that is on a different ext4 filesystem from the temporary directory.
/// tmpfs is never used: it is not a supported filesystem.
fn second_filesystem() -> Option<&'static Path> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let candidate = match std::env::var_os("EMUWIZ_OUROBOROS_SECOND_FS") {
            Some(dir) => PathBuf::from(dir),
            None => std::env::current_exe()
                .ok()?
                .parent()?
                .parent()?
                .join("ouroboros-fixtures"),
        };
        std::fs::create_dir_all(&candidate).ok()?;
        let candidate = std::fs::canonicalize(candidate).ok()?;
        let tmp = std::fs::canonicalize(std::env::temp_dir()).ok()?;
        let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
        if dev(&candidate)? == dev(&tmp)? {
            return None;
        }
        let policy = super::fscompat::FsPolicy::TestScratch;
        for p in [&candidate, &tmp] {
            let facts = super::fscompat::inspect(p).ok()?;
            super::fscompat::supported(&facts, policy).ok()?;
        }
        Some(candidate)
    })
    .as_deref()
}

fn cross_available() -> bool {
    second_filesystem().is_some()
}

struct Fx {
    _a: tempfile::TempDir,
    _b: tempfile::TempDir,
    root_a: PathBuf,
    root_b: PathBuf,
    /// Test-only execution authority; it created `root_a` and `root_b`.
    auth: super::authority::ScratchAuthority,
    source: PathBuf,
    dest_dir: PathBuf,
    db: crate::Database,
    journal: PathBuf,
    archive_id: i64,
    content: Vec<u8>,
}

impl Fx {
    fn new(cross: bool) -> Self {
        Self::with(cross, pattern(SMALL, 7))
    }

    fn with(cross: bool, content: Vec<u8>) -> Self {
        let a = tempfile::tempdir().unwrap();
        let b = if cross {
            tempfile::Builder::new()
                .prefix("ouroboros-")
                .tempdir_in(second_filesystem().expect("second ext4 filesystem"))
                .unwrap()
        } else {
            tempfile::tempdir().unwrap()
        };
        Self::in_dirs(a, b, cross, content)
    }

    fn in_dirs(a: tempfile::TempDir, b: tempfile::TempDir, cross: bool, content: Vec<u8>) -> Self {
        Self::in_dirs_with(
            a,
            b,
            cross,
            content,
            super::authority::ScratchAuthority::new(),
        )
    }

    fn in_dirs_with(
        a: tempfile::TempDir,
        b: tempfile::TempDir,
        cross: bool,
        content: Vec<u8>,
        mut auth: super::authority::ScratchAuthority,
    ) -> Self {
        let base_a = std::fs::canonicalize(a.path()).unwrap();
        let base_b = std::fs::canonicalize(b.path()).unwrap();
        let root_a = auth.provision_root(&base_a, "lib-a").unwrap();
        let root_b = auth.provision_root(&base_b, "lib-b").unwrap();
        std::fs::create_dir_all(root_a.join("nes")).unwrap();
        std::fs::create_dir_all(root_b.join("nes")).unwrap();
        let dev = |p: &Path| std::fs::metadata(p).unwrap().dev();
        assert_eq!(dev(&root_a) != dev(&root_b), cross, "fixture filesystems");
        let source = root_a.join("nes").join("Game (USA).nes");
        std::fs::write(&source, &content).unwrap();
        // Every fixture starts from a copy of one migrated empty catalogue
        // (migrating a fresh one per fixture dominated the run time).
        let catalogue = base_a.join("catalogue.sqlite3");
        std::fs::copy(template_catalogue(), &catalogue).unwrap();
        let mut db = crate::Database::open_or_create(&catalogue).unwrap();
        db.register_source_folders(&[root_a.clone(), root_b.clone()])
            .unwrap();
        let folder = db
            .list_source_folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == root_a)
            .unwrap()
            .id;
        let archive = crate::Archive::from_path_in_root(&source, &root_a).unwrap();
        let archive_id = db
            .upsert_archive(folder, &root_a, &archive)
            .unwrap()
            .archive_id;
        Self {
            journal: base_a.join("journals"),
            dest_dir: root_b.join("nes"),
            _a: a,
            _b: b,
            root_a,
            root_b,
            auth,
            source,
            db,
            archive_id,
            content,
        }
    }

    fn plan(&self) -> (Plan, String) {
        let p = self.preview(&self.source, &self.dest_dir);
        assert!(p.blockers.is_empty(), "blockers: {:?}", p.blockers);
        (p.plan.unwrap(), p.digest.unwrap())
    }

    /// A scratch-policy preview (the authority created both roots).
    fn preview(&self, source: &Path, destination_dir: &Path) -> Preview {
        plan_relocation_scratch(&self.db, source, destination_dir, &self.auth)
    }

    fn ctx(&self) -> Context<'_> {
        Context::new(&self.journal).with_authority(&self.auth)
    }

    fn ctx_f(&self, fault: &FaultPlan) -> Context<'_> {
        Context {
            journal_root: self.journal.clone(),
            fault: Some(fault.clone()),
            cancel: None,
            authority: Some(&self.auth),
        }
    }

    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.db.path()).unwrap()
    }

    fn catalogue_path(&self) -> PathBuf {
        use std::os::unix::ffi::OsStringExt;
        let bytes: Vec<u8> = self
            .sql()
            .query_row(
                "SELECT absolute_path_cached FROM archives WHERE id=?1",
                [self.archive_id],
                |r| r.get(0),
            )
            .unwrap();
        PathBuf::from(std::ffi::OsString::from_vec(bytes))
    }

    fn records(&self, txn: &str) -> usize {
        journal::load(&self.journal, txn).unwrap().records.len()
    }

    /// Holds in every state, terminal or not.
    fn assert_safe(&self, plan: &Plan, context: &str) {
        let mut places = vec![plan.source_path.clone(), plan.destination_path.clone()];
        places.extend(plan.retired_path.clone());
        for step in &plan.steps {
            if let Step::CopyToStaging { staged } = step {
                places.push(staged.clone());
            }
        }
        assert!(
            places
                .iter()
                .any(|p| read(p).as_deref() == Some(&self.content[..])),
            "{context}: original content lost"
        );
        let named = self.catalogue_path();
        if plan.strategy == Strategy::AtomicRename {
            // One rename and one SQLite commit cannot be a single atomic
            // action: between them the row names the other end of the move.
            // The journal shows the transaction as unfinished and recovery
            // completes the commit; the content is at one of the two ends.
            assert!(
                named == plan.source_path || named == plan.destination_path,
                "{context}: the catalogue names {}",
                named.display()
            );
            assert!(
                [&plan.source_path, &plan.destination_path]
                    .iter()
                    .any(|p| read(p).as_deref() == Some(&self.content[..])),
                "{context}: content is at neither end of the move"
            );
            return;
        }
        assert_eq!(
            read(&named).as_deref(),
            Some(&self.content[..]),
            "{context}: the catalogue names {} which does not hold the content",
            named.display()
        );
    }

    fn assert_complete(&self, plan: &Plan, context: &str) {
        assert_eq!(
            read(&plan.destination_path).as_deref(),
            Some(&self.content[..]),
            "{context}"
        );
        assert!(
            !exists(&plan.source_path),
            "{context}: source still present"
        );
        assert_eq!(self.catalogue_path(), plan.destination_path, "{context}");
        if let Some(r) = &plan.retired_path {
            assert_eq!(
                read(r).as_deref(),
                Some(&self.content[..]),
                "{context}: retained original"
            );
            let retired = std::fs::metadata(r).unwrap();
            assert_eq!(
                (retired.dev(), retired.ino()),
                (plan.source_identity.dev, plan.source_identity.ino)
            );
        } else {
            let dest = std::fs::metadata(&plan.destination_path).unwrap();
            assert_eq!(
                dest.ino(),
                plan.source_identity.ino,
                "{context}: atomic rename keeps the inode"
            );
        }
        for step in &plan.steps {
            if let Step::RemovePrivateDir { dir, .. } = step {
                assert!(!exists(dir), "{context}: {} left behind", dir.display());
            }
        }
    }

    fn assert_compensated(&self, plan: &Plan, context: &str) {
        assert_eq!(
            read(&plan.source_path).as_deref(),
            Some(&self.content[..]),
            "{context}"
        );
        assert!(
            !exists(&plan.destination_path),
            "{context}: destination left behind"
        );
        assert_eq!(self.catalogue_path(), plan.source_path, "{context}");
        for step in &plan.steps {
            if let Step::CreatePrivateDir { dir, .. } = step {
                assert!(!exists(dir), "{context}: {} left behind", dir.display());
            }
        }
    }
}

fn trace_points(cross: bool) -> Vec<Point> {
    let fx = Fx::new(cross);
    let (plan, digest) = fx.plan();
    let t = FaultPlan::tracing();
    assert_eq!(
        apply(&fx.ctx_f(&t), &plan, &digest).unwrap(),
        TxnState::Complete
    );
    let mut points = t.trace();
    let torn: Vec<Point> = points
        .iter()
        .filter_map(|p| match p {
            Point::AfterRecord(seq) => Some(Point::TornRecord(*seq)),
            _ => None,
        })
        .collect();
    points.retain(|p| !matches!(p, Point::TornRecord(_)));
    points.extend(torn);
    points
}

// ---- end-to-end ------------------------------------------------------------

fn complete_journey(cross: bool) {
    let fx = Fx::new(cross);
    let preview = fx.preview(&fx.source, &fx.dest_dir);
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
    assert_eq!(
        preview.strategy,
        Some(if cross {
            Strategy::CopyVerifyRetire
        } else {
            Strategy::AtomicRename
        })
    );
    assert!(
        preview.effects.iter().any(|e| e.contains("re-checked")),
        "{:?}",
        preview.effects
    );
    let (plan, digest) = (preview.plan.unwrap(), preview.digest.unwrap());
    assert_eq!(
        apply(&fx.ctx(), &plan, &digest).unwrap(),
        TxnState::Complete
    );
    fx.assert_complete(&plan, "apply");
    fx.assert_safe(&plan, "apply");
    let n = fx.records(&plan.txn_id);
    // I3: recovering a Complete transaction appends and changes nothing.
    assert_eq!(
        recover(&fx.ctx(), &plan.txn_id).unwrap(),
        TxnState::Complete
    );
    assert_eq!(fx.records(&plan.txn_id), n);
    assert_eq!(status(&fx.ctx(), &plan.txn_id).0, TxnState::Complete);
    // A replay of the same approved plan cannot apply twice.
    assert!(matches!(
        apply(&fx.ctx(), &plan, &digest),
        Err(EngineError::Refused(_))
    ));
    fx.assert_complete(&plan, "replay");

    // Undo.
    let undo = plan_undo(&fx.ctx(), &plan.txn_id).unwrap();
    let ud = plan_digest(&undo);
    assert_eq!(
        apply_undo(&fx.ctx(), &undo, &ud).unwrap(),
        TxnState::Complete
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    let restored = std::fs::metadata(&fx.source).unwrap();
    assert_eq!(
        restored.ino(),
        plan.source_identity.ino,
        "Undo restores the original inode, not a copy"
    );
    assert!(!exists(&plan.destination_path));
    assert_eq!(fx.catalogue_path(), fx.source);
    if let Some(r) = &plan.retired_path {
        assert!(
            !exists(r.parent().unwrap()),
            "retirement directory removed after restore"
        );
    }
    assert_eq!(
        undone_by(&fx.ctx(), &plan.txn_id),
        Some(undo.txn_id.clone())
    );
    let again = undo_blocker(&fx.ctx(), &plan.txn_id).expect("Undo must not be offered twice");
    assert!(again.contains("already undone"), "{again}");
}

#[test]
fn cross_filesystem_relocation_completes_verifies_and_undoes() {
    if !cross_available() {
        eprintln!(
            "SKIPPED: no second ext4 filesystem (set EMUWIZ_OUROBOROS_SECOND_FS); cross-filesystem behaviour NOT proven on this host"
        );
        return;
    }
    complete_journey(true);
}

#[test]
fn same_filesystem_relocation_completes_verifies_and_undoes() {
    complete_journey(false);
}

// ---- crash at every durable transition --------------------------------------

fn crash_everywhere(cross: bool) {
    let points = trace_points(cross);
    assert!(points.len() > 20, "trace too short: {points:?}");
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    for point in &points {
        let fx = Fx::new(cross);
        let (plan, digest) = fx.plan();
        let f = FaultPlan::at(point.clone(), Action::Crash);
        let r = apply(&fx.ctx_f(&f), &plan, &digest);
        assert!(f.fired(), "{point:?} never reached");
        assert!(
            matches!(r, Err(EngineError::Crashed(_))),
            "{point:?}: {r:?}"
        );
        fx.assert_safe(&plan, &format!("after crash at {point:?}"));
        let (mid, _) = status(&fx.ctx(), &plan.txn_id);
        // Only a crash after the outcome record may already read Complete.
        assert!(
            mid == TxnState::Complete || matches!(mid, TxnState::Interrupted { .. }),
            "{point:?}: status {mid:?}"
        );
        let s1 = recover(&fx.ctx(), &plan.txn_id).unwrap();
        assert_eq!(
            s1,
            TxnState::Complete,
            "{point:?}: a crash alone must resume to Complete"
        );
        fx.assert_complete(&plan, &format!("{point:?}"));
        let n = fx.records(&plan.txn_id);
        assert_eq!(
            recover(&fx.ctx(), &plan.txn_id).unwrap(),
            s1,
            "{point:?}: second recovery"
        );
        assert_eq!(
            fx.records(&plan.txn_id),
            n,
            "{point:?}: second recovery appended"
        );
        *outcomes.entry(format!("{s1:?}")).or_default() += 1;
    }
    eprintln!(
        "crash at {} points ({}): {outcomes:?}",
        points.len(),
        if cross { "cross-fs" } else { "same-fs" }
    );
}

#[test]
fn cross_filesystem_crash_at_every_point_recovers() {
    if !cross_available() {
        eprintln!("SKIPPED: no second ext4 filesystem (set EMUWIZ_OUROBOROS_SECOND_FS)");
        return;
    }
    crash_everywhere(true);
}

#[test]
fn same_filesystem_crash_at_every_point_recovers() {
    crash_everywhere(false);
}

/// Crash during apply, then crash again at every point of the first
/// recovery, then recover.
#[test]
fn repeated_crashes_during_recovery_converge() {
    let cross = cross_available();
    let apply_points = trace_points(cross);
    let mut runs = 0;
    for first in apply_points.iter().step_by(3) {
        // Trace the recovery that follows this crash.
        let recovery_points = {
            let fx = Fx::new(cross);
            let (plan, digest) = fx.plan();
            let _ = apply(
                &fx.ctx_f(&FaultPlan::at(first.clone(), Action::Crash)),
                &plan,
                &digest,
            );
            let t = FaultPlan::tracing();
            let _ = recover(&fx.ctx_f(&t), &plan.txn_id).unwrap();
            t.trace()
        };
        for second in recovery_points
            .iter()
            .filter(|p| !matches!(p, Point::TornRecord(_)))
        {
            let fx = Fx::new(cross);
            let (plan, digest) = fx.plan();
            let _ = apply(
                &fx.ctx_f(&FaultPlan::at(first.clone(), Action::Crash)),
                &plan,
                &digest,
            );
            let f = FaultPlan::at(second.clone(), Action::Crash);
            let r = recover(&fx.ctx_f(&f), &plan.txn_id);
            assert!(
                f.fired() && matches!(r, Err(EngineError::Crashed(_))),
                "{first:?}/{second:?}: {r:?}"
            );
            fx.assert_safe(&plan, &format!("{first:?} then {second:?}"));
            assert_eq!(
                recover(&fx.ctx(), &plan.txn_id).unwrap(),
                TxnState::Complete,
                "{first:?}/{second:?}"
            );
            fx.assert_complete(&plan, &format!("{first:?}/{second:?}"));
            runs += 1;
        }
    }
    eprintln!("repeated-crash runs: {runs}");
    assert!(runs > 20);
}

// ---- injected I/O errors (disk full, permission, EIO) -----------------------

#[test]
fn injected_errors_compensate_before_and_block_after_point_of_no_return() {
    let cross = cross_available();
    let points = trace_points(cross);
    let mut seen = BTreeMap::<&str, usize>::new();
    for point in points.iter().filter(|p| matches!(p, Point::Step { .. })) {
        for errno in [libc::ENOSPC, libc::EACCES, libc::EIO] {
            let fx = Fx::new(cross);
            let (plan, digest) = fx.plan();
            let Point::Step { step, .. } = point else {
                unreachable!()
            };
            let f = FaultPlan::at(point.clone(), Action::Error(errno));
            let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
            fx.assert_safe(&plan, &format!("{point:?} errno {errno}"));
            match &state {
                TxnState::Compensated { residue } => {
                    assert!(
                        *step <= plan.point_of_no_return,
                        "{point:?}: compensated after the point of no return"
                    );
                    assert!(residue.is_empty(), "{point:?}: {residue:?}");
                    fx.assert_compensated(&plan, &format!("{point:?}"));
                    *seen.entry("compensated").or_default() += 1;
                }
                TxnState::Blocked { .. } => {
                    assert!(
                        *step > plan.point_of_no_return,
                        "{point:?}: blocked before the point of no return"
                    );
                    // Fixing the cause and recovering finishes the move.
                    assert_eq!(
                        recover(&fx.ctx(), &plan.txn_id).unwrap(),
                        TxnState::Complete,
                        "{point:?}"
                    );
                    fx.assert_complete(&plan, &format!("{point:?}"));
                    *seen.entry("blocked-then-complete").or_default() += 1;
                }
                // An error at a point after the mutation's own checks (for
                // example after a directory fsync) may still complete.
                TxnState::Complete => {
                    fx.assert_complete(&plan, &format!("{point:?}"));
                    *seen.entry("complete").or_default() += 1;
                }
                other => panic!("{point:?}: {other:?}"),
            }
            assert!(!state.is_success() || state == TxnState::Complete);
        }
    }
    eprintln!("injected errors: {seen:?}");
    assert!(seen.contains_key("compensated") && seen.contains_key("blocked-then-complete"));
}

// ---- stale plans and source replacement (I4, I6) ------------------------------

#[test]
fn tampered_or_unapproved_plans_are_refused() {
    let fx = Fx::new(false);
    let (mut plan, digest) = fx.plan();
    assert!(matches!(
        apply(&fx.ctx(), &plan, "0000"),
        Err(EngineError::Refused(_))
    ));
    plan.destination_path = fx.root_b.join("elsewhere.nes");
    assert!(matches!(
        apply(&fx.ctx(), &plan, &digest),
        Err(EngineError::Refused(_))
    ));
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    assert!(!fx.journal.join(&plan.txn_id).exists());
}

#[test]
fn source_replaced_after_planning_is_refused() {
    let fx = Fx::new(cross_available());
    let (plan, digest) = fx.plan();
    // Same size, same modification time, different bytes: only the content
    // proof can tell.
    let mtime = std::fs::metadata(&fx.source).unwrap().modified().unwrap();
    let mut other = fx.content.clone();
    other[100] ^= 0xff;
    std::fs::write(&fx.source, &other).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&fx.source)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let state = apply(&fx.ctx(), &plan, &digest).unwrap();
    assert!(matches!(state, TxnState::Compensated { .. }), "{state:?}");
    assert_eq!(
        read(&fx.source),
        Some(other),
        "the user's file is untouched"
    );
    assert!(!exists(&plan.destination_path));
    assert_eq!(fx.catalogue_path(), fx.source);
}

#[test]
fn source_modified_during_copy_is_never_published() {
    if !cross_available() {
        return;
    }
    let fx = Fx::with(true, pattern(3 << 20, 3)); // several copy chunks
    let (plan, digest) = fx.plan();
    let src = fx.source.clone();
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: 3,
            at: "partial",
        },
        move || {
            let mut bytes = std::fs::read(&src).unwrap();
            let last = bytes.len() - 1;
            bytes[last] ^= 0x55;
            std::fs::write(&src, bytes).unwrap();
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
    let TxnState::Compensated { residue } = &state else {
        panic!("{state:?}")
    };
    let edited = read(&fx.source).unwrap();
    assert_ne!(edited, fx.content, "the user's edit stays in place");
    assert!(!exists(&plan.destination_path));
    assert_eq!(fx.catalogue_path(), fx.source);
    // The partial copy could not be proven disposable (the original changed),
    // so it is retained and reported, never silently removed.
    assert!(!residue.is_empty(), "{residue:?}");
}

#[test]
fn source_modified_after_commit_before_retirement_is_preserved() {
    if !cross_available() {
        return;
    }
    let fx = Fx::new(true);
    let (plan, digest) = fx.plan();
    let retire = plan
        .steps
        .iter()
        .position(|s| {
            matches!(
                s,
                Step::Rename {
                    expect: Expect::Original,
                    ..
                }
            )
        })
        .unwrap();
    let src = fx.source.clone();
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: retire,
            at: "begin",
        },
        move || {
            let mut bytes = std::fs::read(&src).unwrap();
            bytes[0] ^= 1;
            std::fs::write(&src, bytes).unwrap();
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
    let TxnState::Compensated { residue } = &state else {
        panic!("{state:?}")
    };
    let edited = read(&fx.source).unwrap();
    assert_ne!(edited, fx.content);
    // The catalogue names the user's (edited) file again, and the verified
    // copy of the earlier content is retained, not deleted.
    assert_eq!(fx.catalogue_path(), fx.source);
    assert!(!exists(&plan.destination_path));
    assert!(
        residue
            .iter()
            .any(|p| read(p).as_deref() == Some(&fx.content[..])),
        "{residue:?}"
    );
}

// ---- destination, symlinks, storage ------------------------------------------

#[test]
fn destination_appearing_is_never_overwritten() {
    let cross = cross_available();
    let fx = Fx::new(cross);
    let (plan, digest) = fx.plan();
    let publish = plan
        .steps
        .iter()
        .position(|s| matches!(s, Step::Rename { to, .. } if *to == plan.destination_path))
        .unwrap();
    let dest = plan.destination_path.clone();
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: publish,
            at: "before_rename",
        },
        move || {
            std::fs::write(&dest, b"user file").unwrap();
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
    assert!(matches!(state, TxnState::Compensated { .. }), "{state:?}");
    assert_eq!(
        read(&plan.destination_path).as_deref(),
        Some(&b"user file"[..])
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    assert_eq!(fx.catalogue_path(), fx.source);
}

#[test]
fn symlink_substituted_for_destination_folder_is_refused() {
    let cross = cross_available();
    let fx = Fx::new(cross);
    let (plan, digest) = fx.plan();
    let outside = tempfile::tempdir().unwrap();
    let dest_dir = fx.dest_dir.clone();
    let target = outside.path().to_path_buf();
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: 0,
            at: "begin",
        },
        move || {
            let moved = dest_dir.with_file_name("nes-moved");
            std::fs::rename(&dest_dir, &moved).unwrap();
            std::os::unix::fs::symlink(&target, &dest_dir).unwrap();
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
    assert!(!state.is_success(), "{state:?}");
    assert_eq!(
        std::fs::read_dir(outside.path()).unwrap().count(),
        0,
        "nothing written through the symlink"
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    assert_eq!(fx.catalogue_path(), fx.source);
}

#[test]
fn destination_storage_disappearing_fails_closed_then_recovers() {
    let cross = cross_available();
    let fx = Fx::new(cross);
    let (plan, digest) = fx.plan();
    let commit = plan
        .steps
        .iter()
        .position(|s| matches!(s, Step::CommitCatalogue { .. }))
        .unwrap();
    let root = fx.root_b.clone();
    let away = fx.root_b.with_file_name("lib-b-unmounted");
    let (r1, a1) = (root.clone(), away.clone());
    // The root vanishes (as on unmount) just before the catalogue commit.
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: commit,
            at: "begin",
        },
        move || {
            std::fs::rename(&r1, &a1).unwrap();
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
    assert!(!state.is_success(), "{state:?}");
    assert_eq!(
        fx.catalogue_path(),
        fx.source,
        "the catalogue never names vanished storage"
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    // Storage comes back; recovery finishes the compensation.
    std::fs::rename(&away, &root).unwrap();
    let after = recover(&fx.ctx(), &plan.txn_id).unwrap();
    assert!(after.is_terminal(), "{after:?}");
    fx.assert_safe(&plan, "after storage returned");
}

// ---- catalogue concurrency ------------------------------------------------------

#[test]
fn concurrent_catalogue_change_is_detected_and_compensated() {
    let fx = Fx::new(cross_available());
    let (plan, digest) = fx.plan();
    // A scan or another writer moves the row after planning.
    fx.sql()
        .execute(
            "UPDATE archives SET relative_path=X'00', absolute_path_cached=X'2f78' WHERE id=?1",
            [fx.archive_id],
        )
        .unwrap();
    let state = apply(&fx.ctx(), &plan, &digest).unwrap();
    assert!(!state.is_success(), "{state:?}");
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    assert!(
        !exists(&plan.destination_path),
        "no duplicate published for a diverged catalogue"
    );
}

#[test]
fn scan_inserting_the_destination_row_blocks_commit() {
    let fx = Fx::new(cross_available());
    let (plan, digest) = fx.plan();
    let commit = plan
        .steps
        .iter()
        .position(|s| matches!(s, Step::CommitCatalogue { .. }))
        .unwrap();
    let (db_path, root_b, dest) = (
        fx.db.path().to_path_buf(),
        fx.root_b.clone(),
        plan.destination_path.clone(),
    );
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: commit,
            at: "begin",
        },
        move || {
            // A concurrent scan finds the published file and catalogues it.
            let mut db = crate::Database::open_existing(&db_path).unwrap();
            let folder = db
                .list_source_folders()
                .unwrap()
                .into_iter()
                .find(|f| f.path == root_b)
                .unwrap()
                .id;
            let archive = crate::Archive::from_path_in_root(&dest, &root_b).unwrap();
            db.upsert_archive(folder, &root_b, &archive).unwrap();
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
    assert!(matches!(state, TxnState::Compensated { .. }), "{state:?}");
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    assert_eq!(fx.catalogue_path(), fx.source);
}

#[test]
fn overlapping_transformations_are_refused() {
    let fx = Fx::new(false);
    let (a, da) = fx.plan();
    let (b, db_) = fx.plan();
    assert_ne!(a.txn_id, b.txn_id);
    let held = lock::acquire(&fx.journal, &resources(&a)).unwrap();
    let err = apply(&fx.ctx(), &b, &db_).unwrap_err();
    assert!(
        matches!(err, EngineError::Refused(ref m) if m.contains("in use")),
        "{err}"
    );
    drop(held);
    assert_eq!(apply(&fx.ctx(), &a, &da).unwrap(), TxnState::Complete);
    // The second plan is now stale: its source moved.
    let late = apply(&fx.ctx(), &b, &db_).unwrap();
    assert!(matches!(late, TxnState::Compensated { .. }), "{late:?}");
    fx.assert_complete(&a, "first plan");
}

#[test]
fn two_threads_racing_on_one_file_move_it_once() {
    let fx = Fx::new(cross_available());
    let (a, da) = fx.plan();
    let (b, db_) = fx.plan();
    let journal = fx.journal.clone();
    let auth = &fx.auth;
    let results = std::thread::scope(|s| {
        let ja = journal.clone();
        let ta = s.spawn(move || {
            apply(&Context::new(ja).with_authority(auth), &a, &da).map_err(|e| e.to_string())
        });
        let tb = s.spawn(move || {
            apply(&Context::new(journal).with_authority(auth), &b, &db_).map_err(|e| e.to_string())
        });
        [ta.join().unwrap(), tb.join().unwrap()]
    });
    let completes = results
        .iter()
        .filter(|r| matches!(r, Ok(TxnState::Complete)))
        .count();
    assert_eq!(completes, 1, "{results:?}");
    assert_eq!(fx.catalogue_path().parent().unwrap(), fx.dest_dir);
}

// ---- journals --------------------------------------------------------------------

#[test]
fn corrupted_or_missing_journals_are_never_acted_on() {
    let cross = cross_available();
    for damage in [
        "flip",
        "truncate-middle",
        "delete-log",
        "edit-plan",
        "delete-plan",
    ] {
        let fx = Fx::new(cross);
        let (plan, digest) = fx.plan();
        let commit = plan
            .steps
            .iter()
            .position(|s| matches!(s, Step::CommitCatalogue { .. }))
            .unwrap();
        let f = FaultPlan::at(
            Point::Step {
                step: commit,
                at: "after_commit",
            },
            Action::Crash,
        );
        let _ = apply(&fx.ctx_f(&f), &plan, &digest);
        let dir = fx.journal.join(&plan.txn_id);
        let log = dir.join("log.jsonl");
        let perms = |p: &Path| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        match damage {
            "flip" => {
                let mut bytes = std::fs::read(&log).unwrap();
                let at = bytes.len() / 3;
                bytes[at] ^= 0x01;
                std::fs::write(&log, bytes).unwrap();
            }
            "truncate-middle" => {
                let bytes = std::fs::read(&log).unwrap();
                let lines: Vec<&[u8]> = bytes.split_inclusive(|b| *b == b'\n').collect();
                let mut out = Vec::new();
                for (i, l) in lines.iter().enumerate() {
                    if i != 2 {
                        out.extend_from_slice(l);
                    }
                }
                std::fs::write(&log, out).unwrap();
            }
            "delete-log" => std::fs::remove_file(&log).unwrap(),
            "edit-plan" => {
                let p = dir.join("plan.json");
                perms(&p);
                let text = std::fs::read_to_string(&p)
                    .unwrap()
                    .replace("Game (USA)", "Game (EUR)");
                std::fs::write(&p, text).unwrap();
            }
            "delete-plan" => std::fs::remove_file(dir.join("plan.json")).unwrap(),
            _ => unreachable!(),
        }
        let before = (
            read(&fx.source),
            read(&plan.destination_path),
            fx.catalogue_path(),
        );
        let state = recover(&fx.ctx(), &plan.txn_id).unwrap();
        assert!(
            matches!(state, TxnState::Corrupt { .. }),
            "{damage}: {state:?}"
        );
        assert_eq!(
            before,
            (
                read(&fx.source),
                read(&plan.destination_path),
                fx.catalogue_path()
            ),
            "{damage}"
        );
        assert!(matches!(
            status(&fx.ctx(), &plan.txn_id).0,
            TxnState::Corrupt { .. }
        ));
        fx.assert_safe(&plan, damage);
    }
}

#[test]
fn torn_final_record_is_recovered_with_evidence_kept() {
    let fx = Fx::new(false);
    let (plan, digest) = fx.plan();
    let f = FaultPlan::at(Point::TornRecord(4), Action::Crash);
    assert!(matches!(
        apply(&fx.ctx_f(&f), &plan, &digest),
        Err(EngineError::Crashed(_))
    ));
    let loaded = journal::load(&fx.journal, &plan.txn_id).unwrap();
    assert!(loaded.torn_tail.is_some());
    assert_eq!(
        recover(&fx.ctx(), &plan.txn_id).unwrap(),
        TxnState::Complete
    );
    let after = journal::load(&fx.journal, &plan.txn_id).unwrap();
    assert!(after.torn_tail.is_none());
    assert!(after.events().any(|e| matches!(
        e,
        Event::RecoveryStarted {
            torn_tail_bytes: Some(_),
            ..
        }
    )));
}

// ---- permissions, cancellation ----------------------------------------------------

#[test]
fn permission_failure_on_the_source_side_compensates() {
    if unsafe { libc::geteuid() } == 0 || !cross_available() {
        eprintln!("SKIPPED: needs a non-root user and a second ext4 filesystem");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new(true);
    let (plan, digest) = fx.plan();
    let folder = fx.source.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let state = apply(&fx.ctx(), &plan, &digest).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(state, TxnState::Compensated { .. }), "{state:?}");
    fx.assert_compensated(&plan, "read-only source folder");
}

#[test]
fn cancellation_compensates_before_and_completes_after_the_point_of_no_return() {
    let cross = cross_available();
    for (at, expect_complete) in [(2usize, false), (usize::MAX, true)] {
        let fx = Fx::new(cross);
        let (plan, digest) = fx.plan();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let step = if at == usize::MAX {
            plan.point_of_no_return + 1
        } else {
            at
        };
        let flag: &'static std::sync::atomic::AtomicBool = Box::leak(Box::new(cancel));
        let f = FaultPlan::tracing().with_hook(
            Point::Step {
                step: step - 1,
                at: "begin",
            },
            move || flag.store(true, std::sync::atomic::Ordering::SeqCst),
        );
        let ctx = Context {
            journal_root: fx.journal.clone(),
            fault: Some(f),
            cancel: Some(flag),
            authority: Some(&fx.auth),
        };
        let state = apply(&ctx, &plan, &digest).unwrap();
        if expect_complete {
            assert_eq!(state, TxnState::Complete);
            fx.assert_complete(&plan, "late cancel");
        } else {
            assert!(matches!(state, TxnState::Compensated { .. }), "{state:?}");
            fx.assert_compensated(&plan, "early cancel");
        }
    }
}

// ---- Undo preconditions and interruption (I8) -------------------------------------

#[test]
fn undo_is_withheld_when_its_preconditions_fail() {
    let fx = Fx::new(cross_available());
    let (plan, digest) = fx.plan();
    assert_eq!(
        apply(&fx.ctx(), &plan, &digest).unwrap(),
        TxnState::Complete
    );
    // The user edits the relocated game: Undo would discard the edit.
    let mut edited = fx.content.clone();
    edited[1] ^= 9;
    std::fs::write(&plan.destination_path, &edited).unwrap();
    let why = undo_blocker(&fx.ctx(), &plan.txn_id).expect("blocked");
    assert!(why.contains("changed since it was relocated"), "{why}");
    // A plan prepared before the edit is refused at apply too.
    let fx2 = Fx::new(cross_available());
    let (p2, d2) = fx2.plan();
    assert_eq!(apply(&fx2.ctx(), &p2, &d2).unwrap(), TxnState::Complete);
    let undo = plan_undo(&fx2.ctx(), &p2.txn_id).unwrap();
    std::fs::write(&fx2.source, b"new file at the old name").unwrap();
    let state = apply_undo(&fx2.ctx(), &undo, &plan_digest(&undo)).unwrap();
    assert!(matches!(state, TxnState::Compensated { .. }), "{state:?}");
    assert_eq!(
        read(&fx2.source).as_deref(),
        Some(&b"new file at the old name"[..])
    );
    assert_eq!(
        read(&p2.destination_path).as_deref(),
        Some(&fx2.content[..])
    );
    assert_eq!(fx2.catalogue_path(), p2.destination_path);
}

#[test]
fn interrupted_undo_recovers_at_every_point() {
    let cross = cross_available();
    let points = {
        let fx = Fx::new(cross);
        let (plan, digest) = fx.plan();
        apply(&fx.ctx(), &plan, &digest).unwrap();
        let undo = plan_undo(&fx.ctx(), &plan.txn_id).unwrap();
        let t = FaultPlan::tracing();
        assert_eq!(
            apply_undo(&fx.ctx_f(&t), &undo, &plan_digest(&undo)).unwrap(),
            TxnState::Complete
        );
        t.trace()
    };
    for point in points.iter().filter(|p| !matches!(p, Point::TornRecord(_))) {
        let fx = Fx::new(cross);
        let (plan, digest) = fx.plan();
        apply(&fx.ctx(), &plan, &digest).unwrap();
        let undo = plan_undo(&fx.ctx(), &plan.txn_id).unwrap();
        let f = FaultPlan::at(point.clone(), Action::Crash);
        assert!(matches!(
            apply_undo(&fx.ctx_f(&f), &undo, &plan_digest(&undo)),
            Err(EngineError::Crashed(_))
        ));
        // Original content is present somewhere at every crash point.
        let places = [&plan.source_path, &plan.destination_path];
        let mut any = places
            .iter()
            .any(|p| read(p).as_deref() == Some(&fx.content[..]));
        if let Some(r) = &plan.retired_path {
            any |= read(r).as_deref() == Some(&fx.content[..]);
        }
        assert!(any, "{point:?}: content lost during Undo");
        assert_eq!(
            recover(&fx.ctx(), &undo.txn_id).unwrap(),
            TxnState::Complete,
            "{point:?}"
        );
        assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
        assert_eq!(fx.catalogue_path(), fx.source, "{point:?}");
        assert!(!exists(&plan.destination_path), "{point:?}");
    }
}

// ---- dependencies and ownership (I5, I9) ---------------------------------------------

#[test]
fn set_members_descriptors_and_unreadable_descriptors_block_relocation() {
    let fx = Fx::new(false);
    let folder = fx.source.parent().unwrap();
    let bin = folder.join("Disc.bin");
    std::fs::write(&bin, vec![0u8; 2352]).unwrap();
    std::fs::write(
        folder.join("Disc.cue"),
        "FILE \"Disc.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let blockers = fx.preview(&bin, &fx.dest_dir).blockers;
    assert!(
        blockers.iter().any(|b| b.contains("named by")),
        "{blockers:?}"
    );
    let blockers = fx.preview(&folder.join("Disc.cue"), &fx.dest_dir).blockers;
    assert!(
        blockers.iter().any(|b| b.contains("descriptor")),
        "{blockers:?}"
    );
    // An unreadable descriptor next to an unrelated file blocks too: whether
    // it uses the file is unknown, and unknown never authorises a move.
    std::fs::write(folder.join("Broken.cue"), "FILE \"Missing.bin\" BINARY\n").unwrap();
    let blockers = fx.preview(&fx.source, &fx.dest_dir).blockers;
    assert!(
        blockers.iter().any(|b| b.contains("could not be read")),
        "{blockers:?}"
    );
}

#[test]
fn dependency_change_after_planning_is_refused() {
    let fx = Fx::new(false);
    let (plan, digest) = fx.plan();
    let folder = fx.source.parent().unwrap();
    std::fs::write(folder.join("Game.m3u"), "Game (USA).nes\n").unwrap();
    let state = apply(&fx.ctx(), &plan, &digest).unwrap();
    assert!(matches!(state, TxnState::Compensated { .. }), "{state:?}");
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
}

#[test]
fn identical_content_with_a_different_owner_is_untouched() {
    let fx = Fx::new(cross_available());
    let twin = fx.source.with_file_name("Twin (USA).nes");
    std::fs::write(&twin, &fx.content).unwrap();
    let (plan, digest) = fx.plan();
    apply(&fx.ctx(), &plan, &digest).unwrap();
    let undo = plan_undo(&fx.ctx(), &plan.txn_id).unwrap();
    apply_undo(&fx.ctx(), &undo, &plan_digest(&undo)).unwrap();
    assert_eq!(
        read(&twin).as_deref(),
        Some(&fx.content[..]),
        "a byte-identical file owned by another entry survives"
    );
}

#[test]
fn graph_analyses_find_cycles_ambiguity_duplicates_and_missing() {
    use super::graph::*;
    let mut g = MediaGraph::default();
    let a = g.file(Path::new("/x/a.m3u"));
    let b = g.file(Path::new("/x/b.m3u"));
    let t = g.file(Path::new("/x/t.bin"));
    let s = || Verification::Structural { by: "test".into() };
    g.edge(a, b, EdgeKind::Names, s());
    g.edge(b, a, EdgeKind::Names, s());
    g.edge(a, t, EdgeKind::Names, s());
    g.edge(a, t, EdgeKind::Names, s());
    let c = g.file(Path::new("/x/c.cue"));
    g.edge(c, t, EdgeKind::Names, s());
    assert_eq!(g.cycles(), vec![vec![a, b]]);
    assert_eq!(g.duplicate_references, vec![(a, t)]);
    assert!(
        g.ambiguous_owners()
            .iter()
            .any(|(m, o)| *m == t && o.len() == 2)
    );
    assert_eq!(g.missing().len(), 4, "none of these paths exist");
}

#[test]
fn execution_without_the_test_authority_is_refused_even_with_a_marker_file() {
    let fx = Fx::new(false);
    let (plan, digest) = fx.plan();
    // The old fixture marker, copied into both roots, grants nothing.
    for root in [&fx.root_a, &fx.root_b] {
        std::fs::write(
            root.join(".emuwiz-disposable-fixture"),
            b"EmuWiz disposable fixture: transformation tests may modify this tree.\n",
        )
        .unwrap();
    }
    let production = Context::new(&fx.journal);
    let err = apply(&production, &plan, &digest).unwrap_err();
    assert!(
        matches!(err, EngineError::Refused(ref m) if m == super::authority::PRODUCTION_UNAVAILABLE),
        "{err}"
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    assert!(
        !fx.journal.join(&plan.txn_id).exists(),
        "nothing journalled"
    );
    // The production planner is read-only and still previews.
    let preview = plan_relocation(&fx.db, &fx.source, &fx.dest_dir);
    assert!(!preview.effects.is_empty() || !preview.blockers.is_empty());
    if let Some(p) = &preview.plan {
        assert_eq!(p.filesystem_policy, super::fscompat::FsPolicy::Production);
        let d = preview.digest.clone().unwrap();
        assert!(
            apply(&fx.ctx(), p, &d).is_err(),
            "the test authority never executes a production-policy plan"
        );
    }
}

// ---- real process death -----------------------------------------------------------
//
// The engine runs in a child process (this test binary re-executed with a
// filter) that is killed for real: `abort()` at an injected point, SIGKILL
// at a timed delay during a large copy, or a kernel file-size limit that
// makes writes fail (EFBIG, the closest disk-full analogue available without
// root). The parent then recovers from whatever the dead process left.

const CHILD_ENV: &str = "EMUWIZ_OUROBOROS_CHILD";
const FIXTURE_TAG: &str = "OUROBOROS-CHILD-FIXTURE ";

fn child_spec() -> Option<BTreeMap<String, String>> {
    let raw = std::env::var(CHILD_ENV).ok()?;
    Some(split_spec(&raw))
}

fn split_spec(raw: &str) -> BTreeMap<String, String> {
    raw.split('\u{1f}')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn parse_point(text: &str) -> Point {
    let parts: Vec<&str> = text.split(':').collect();
    match parts.as_slice() {
        ["record", seq] => Point::AfterRecord(seq.parse().unwrap()),
        ["torn", seq] => Point::TornRecord(seq.parse().unwrap()),
        ["step", step, at] => Point::Step {
            step: step.parse().unwrap(),
            at: Box::leak(at.to_string().into_boxed_str()),
        },
        _ => panic!("bad point {text}"),
    }
}

fn point_text(p: &Point) -> String {
    match p {
        Point::AfterRecord(s) => format!("record:{s}"),
        Point::TornRecord(s) => format!("torn:{s}"),
        Point::Step { step, at } => format!("step:{step}:{at}"),
    }
}

/// Entry point of the child process; a no-op in the normal test run.
///
/// Trust flows one way only. The environment names nothing but two parent
/// folders and the scenario. The child creates a **fresh** fixture inside
/// them (new private roots, a new game file, a new catalogue) with its own
/// new `ScratchAuthority`, so whatever the environment says, the child can
/// only ever move a file it has just created. No root, identity, plan or
/// journal is read from the environment or adopted here.
#[test]
fn ouroboros_child_entry() {
    let Some(spec) = child_spec() else { return };
    let a = tempfile::tempdir_in(&spec["a"]).unwrap();
    let b = tempfile::tempdir_in(&spec["b"]).unwrap();
    let cross = spec["cross"] == "true";
    let content = pattern(spec["size"].parse().unwrap(), spec["seed"].parse().unwrap());
    let fx = Fx::in_dirs_with(
        a,
        b,
        cross,
        content,
        super::authority::ScratchAuthority::new(),
    );
    let (plan, digest) = fx.plan();
    let plan_file = fx
        .journal
        .with_file_name(format!("plan-{}.json", plan.txn_id));
    std::fs::write(&plan_file, engine::plan_bytes(&plan)).unwrap();
    let fields = [
        ("root_a", fx.root_a.display().to_string()),
        ("root_b", fx.root_b.display().to_string()),
        ("source", fx.source.display().to_string()),
        ("dest_dir", fx.dest_dir.display().to_string()),
        ("catalogue", fx.db.path().display().to_string()),
        ("journal", fx.journal.display().to_string()),
        ("archive_id", fx.archive_id.to_string()),
        ("plan", plan_file.display().to_string()),
        ("digest", digest.clone()),
    ];
    {
        use std::io::Write;
        let line: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let mut out = std::io::stdout().lock();
        writeln!(out, "{FIXTURE_TAG}{}", line.join("\u{1f}")).unwrap();
        out.flush().unwrap();
    }
    let mut ctx = fx.ctx();
    if let Some(point) = spec.get("abort") {
        let mut f = FaultPlan::at(parse_point(point), Action::Crash);
        f.abort_process = true;
        ctx.fault = Some(f);
    }
    if let Some(limit) = spec.get("fsize") {
        // SAFETY: plain libc calls in a single-threaded child.
        unsafe {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            let l = libc::rlimit {
                rlim_cur: limit.parse().unwrap(),
                rlim_max: limit.parse().unwrap(),
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &l), 0);
        }
    }
    let code = match apply(&ctx, &plan, &digest) {
        Ok(TxnState::Complete) => 10,
        Ok(TxnState::Compensated { .. }) => 11,
        Ok(TxnState::Blocked { .. }) => 12,
        other => {
            eprintln!("child: {other:?}");
            13
        }
    };
    std::process::exit(code);
}

/// A child process running one scenario on a fixture it created, and the
/// parent's view of that fixture.
struct ChildRun {
    fx: Fx,
    plan: Plan,
    child: std::process::Child,
    _stdout: std::io::BufReader<std::process::ChildStdout>,
}

/// Spawns the child and waits until it has built its fixture. The parent
/// adopts the child's roots from the child's own stdout pipe, and only as
/// directories inside the folders the parent has just created for it.
fn spawn_child(cross: bool, content: (usize, u8), extra: &[(&str, String)]) -> ChildRun {
    use std::io::BufRead;
    let a = tempfile::tempdir().unwrap();
    let b = if cross {
        tempfile::Builder::new()
            .prefix("ouroboros-")
            .tempdir_in(second_filesystem().expect("second ext4 filesystem"))
            .unwrap()
    } else {
        tempfile::tempdir().unwrap()
    };
    let mut spec = vec![
        format!("a={}", a.path().display()),
        format!("b={}", b.path().display()),
        format!("cross={cross}"),
        format!("size={}", content.0),
        format!("seed={}", content.1),
    ];
    spec.extend(extra.iter().map(|(k, v)| format!("{k}={v}")));
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "transform::tests::ouroboros_child_entry",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_ENV, spec.join("\u{1f}"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let d = loop {
        let mut line = String::new();
        assert!(
            stdout.read_line(&mut line).unwrap() > 0,
            "the child exited before building its fixture"
        );
        // libtest may print "test <name> ... " on the same line first.
        if let Some(rest) = line
            .trim_end()
            .find(FIXTURE_TAG)
            .map(|at| &line.trim_end()[at + FIXTURE_TAG.len()..])
        {
            break split_spec(rest);
        }
    };
    let mut auth = super::authority::ScratchAuthority::new();
    for (nursery, key) in [(a.path(), "root_a"), (b.path(), "root_b")] {
        auth.adopt_from_child(nursery, Path::new(&d[key])).unwrap();
    }
    let plan = parse_plan(&std::fs::read(&d["plan"]).unwrap()).unwrap();
    assert_eq!(engine::plan_digest(&plan), d["digest"]);
    let fx = Fx {
        _a: a,
        _b: b,
        root_a: PathBuf::from(&d["root_a"]),
        root_b: PathBuf::from(&d["root_b"]),
        auth,
        source: PathBuf::from(&d["source"]),
        dest_dir: PathBuf::from(&d["dest_dir"]),
        db: crate::Database::open_or_create(&d["catalogue"]).unwrap(),
        journal: PathBuf::from(&d["journal"]),
        archive_id: d["archive_id"].parse().unwrap(),
        content: pattern(content.0, content.1),
    };
    ChildRun {
        fx,
        plan,
        child,
        _stdout: stdout,
    }
}

#[test]
fn child_process_aborted_at_every_point_recovers() {
    use std::os::unix::process::ExitStatusExt;
    let cross = cross_available();
    let points = trace_points(cross);
    let mut aborted = 0;
    for point in &points {
        let mut run = spawn_child(cross, (SMALL, 7), &[("abort", point_text(point))]);
        let status = run.child.wait().unwrap();
        assert_eq!(
            status.signal(),
            Some(libc::SIGABRT),
            "{point:?}: {status:?}"
        );
        aborted += 1;
        let (fx, plan) = (&run.fx, &run.plan);
        fx.assert_safe(plan, &format!("child aborted at {point:?}"));
        // Recovery after the process died, by another process (this one).
        assert_eq!(
            recover(&fx.ctx(), &plan.txn_id).unwrap(),
            TxnState::Complete,
            "{point:?}"
        );
        fx.assert_complete(plan, &format!("{point:?}"));
    }
    eprintln!("child processes aborted: {aborted}");
}

#[test]
fn child_process_killed_during_large_copy_recovers() {
    if !cross_available() {
        eprintln!(
            "SKIPPED: no second ext4 filesystem; SIGKILL during a cross-filesystem copy NOT proven"
        );
        return;
    }
    let mut outcomes = BTreeMap::<String, usize>::new();
    for delay_ms in [0u64, 2, 5, 10, 20, 40, 80, 120, 200, 400] {
        let mut run = spawn_child(true, (48 << 20, 5), &[]);
        // The child has built its fixture and is starting the move.
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let _ = run.child.kill();
        let status = run.child.wait().unwrap();
        let (fx, plan) = (&run.fx, &run.plan);
        fx.assert_safe(plan, &format!("SIGKILL after {delay_ms} ms"));
        let state = if journal::load(&fx.journal, &plan.txn_id).is_ok() {
            recover(&fx.ctx(), &plan.txn_id).unwrap()
        } else {
            // Killed before the journal existed: nothing may have changed.
            assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
            assert_eq!(fx.catalogue_path(), fx.source);
            TxnState::Compensated { residue: vec![] }
        };
        assert!(state.is_terminal(), "{delay_ms} ms: {state:?}");
        if state == TxnState::Complete {
            fx.assert_complete(plan, &format!("{delay_ms} ms"));
        }
        let label = if state.is_success() {
            "complete"
        } else {
            "compensated"
        };
        *outcomes
            .entry(format!("{:?}/{label}", status.code()))
            .or_default() += 1;
    }
    eprintln!("SIGKILL outcomes: {outcomes:?}");
}

#[test]
fn kernel_file_size_limit_during_copy_compensates() {
    if !cross_available() {
        eprintln!("SKIPPED: no second ext4 filesystem (set EMUWIZ_OUROBOROS_SECOND_FS)");
        return;
    }
    let mut run = spawn_child(true, (8 << 20, 9), &[("fsize", (4u64 << 20).to_string())]);
    let status = run.child.wait().unwrap();
    assert_eq!(
        status.code(),
        Some(11),
        "a real write failure must compensate: {status:?}"
    );
    run.fx.assert_compensated(&run.plan, "RLIMIT_FSIZE");
}

#[test]
fn a_child_given_existing_folders_only_ever_moves_its_own_new_file() {
    // The child's environment names parent folders, never roots: pointed at
    // a folder holding someone's game, it builds a separate fresh fixture
    // inside and leaves the game alone.
    let elsewhere = tempfile::tempdir().unwrap();
    let game = elsewhere.path().join("Somebody's Game (USA).nes");
    std::fs::write(&game, b"not a fixture").unwrap();
    let before: Vec<_> = std::fs::read_dir(elsewhere.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let spec = [
        format!("a={}", elsewhere.path().display()),
        format!("b={}", elsewhere.path().display()),
        "cross=false".to_string(),
        "size=64".to_string(),
        "seed=1".to_string(),
    ];
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "transform::tests::ouroboros_child_entry",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_ENV, spec.join("\u{1f}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(10), "the child moved its own new file");
    assert_eq!(
        std::fs::read(&game).unwrap(),
        b"not a fixture",
        "the existing game is untouched"
    );
    let after: Vec<_> = std::fs::read_dir(elsewhere.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(
        before.iter().all(|n| after.contains(n)),
        "nothing that existed was moved or removed"
    );
    // And the parent-side adoption refuses anything outside the folder it made.
    let nursery = tempfile::tempdir().unwrap();
    let mut auth = super::authority::ScratchAuthority::new();
    assert!(
        auth.adopt_from_child(nursery.path(), elsewhere.path())
            .is_err()
    );
    assert!(
        auth.adopt_from_child(nursery.path(), nursery.path())
            .is_err()
    );
}

// ---- filesystem gate -----------------------------------------------------------

struct OverrideGuard;
impl Drop for OverrideGuard {
    fn drop(&mut self) {
        super::fscompat::test_override::clear();
    }
}

/// Simulates `root`'s mount changing to `change(facts)` from now on.
fn simulate_mount(
    root: PathBuf,
    change: impl Fn(&super::fscompat::MountFacts) -> Result<super::fscompat::MountFacts, String>
    + 'static,
) {
    super::fscompat::test_override::set(move |path, facts| {
        path.starts_with(&root).then(|| change(facts))
    });
}

fn remounted_read_only(
    f: &super::fscompat::MountFacts,
) -> Result<super::fscompat::MountFacts, String> {
    let mut f = f.clone();
    f.mount_options.retain(|o| o != "rw");
    f.mount_options.push("ro".into());
    Ok(f)
}

#[test]
fn production_policy_refuses_this_hosts_filesystems_unless_remount_ro() {
    // The ordinary planner uses the production policy. This host's
    // ext4 mounts use errors=continue, so planning must still preview but
    // never produce an applicable plan; on a host with errors=remount-ro the
    // filesystem is not a blocker.
    let fx = Fx::new(false);
    let preview = plan_relocation(&fx.db, &fx.source, &fx.dest_dir);
    let facts = super::fscompat::inspect(&fx.root_a).unwrap();
    let strict = super::fscompat::supported(&facts, super::fscompat::FsPolicy::Production);
    match strict {
        Err(reason) => {
            assert!(
                preview.plan.is_none() && preview.digest.is_none(),
                "{preview:?}"
            );
            assert!(
                preview.blockers.contains(&reason),
                "{:?} vs {reason}",
                preview.blockers
            );
            assert!(
                !preview.effects.is_empty(),
                "the read-only preview is still produced"
            );
            assert_eq!(preview.strategy, Some(Strategy::AtomicRename));
        }
        Ok(_) => assert!(
            !preview.blockers.iter().any(|b| b.contains("errors=")),
            "{:?}",
            preview.blockers
        ),
    }
}

#[test]
fn scratch_policy_is_recorded_and_said() {
    let fx = Fx::new(false);
    let (plan, _) = fx.plan();
    assert_eq!(
        plan.filesystem_policy,
        super::fscompat::FsPolicy::TestScratch
    );
    // Source root, destination root and the catalogue's folder.
    assert_eq!(plan.filesystems.len(), 3);
    assert!(
        plan.effects.iter().any(|e| e.starts_with("filesystem: ")),
        "{:?}",
        plan.effects
    );
    if plan.filesystems.iter().any(|f| {
        f.journal_options
            .as_ref()
            .is_some_and(|j| j.iter().any(|o| o == "errors=continue"))
    }) {
        assert!(
            plan.effects
                .iter()
                .any(|e| e.contains("test scratch folders only")),
            "{:?}",
            plan.effects
        );
    }
}

/// Replaces `root` with a different directory at the same path (what a
/// bind mount or a substituted directory looks like to the engine: same
/// path, different device/inode). Returns where the original went.
fn substitute_root(root: &Path) -> PathBuf {
    let aside = root.with_extension("aside");
    std::fs::rename(root, &aside).unwrap();
    std::fs::create_dir(root).unwrap();
    std::fs::create_dir(root.join("nes")).unwrap();
    aside
}

fn restore_root(root: &Path, aside: &Path) {
    std::fs::remove_dir_all(root).unwrap();
    std::fs::rename(aside, root).unwrap();
}

#[test]
fn a_root_replaced_or_substituted_after_planning_is_refused() {
    let fx = Fx::new(false);
    let (plan, digest) = fx.plan();
    let aside = substitute_root(&fx.root_b);
    let err = apply(&fx.ctx(), &plan, &digest).unwrap_err();
    assert!(
        err.to_string()
            .contains("replaced, remounted or substituted"),
        "{err}"
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    restore_root(&fx.root_b, &aside);
    // A symlink to an authorised root is not that root.
    let link = fx.root_a.with_extension("link");
    std::os::unix::fs::symlink(&fx.root_a, &link).unwrap();
    assert!(fx.auth.admits(&link).is_err());
    // Another test's authority does not cover these roots.
    let mut other = super::authority::ScratchAuthority::new();
    let _elsewhere = other
        .provision_root(fx.root_a.parent().unwrap(), "other")
        .unwrap();
    let ctx = Context::new(&fx.journal).with_authority(&other);
    assert!(
        apply(&ctx, &plan, &digest)
            .unwrap_err()
            .to_string()
            .contains("not created by this test")
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
}

#[test]
fn a_root_substituted_mid_operation_stops_before_retirement_and_recovers() {
    if !cross_available() {
        eprintln!("SKIPPED: no second ext4 filesystem");
        return;
    }
    let fx = Fx::new(true);
    let (plan, digest) = fx.plan();
    let retire = step_index(&plan, |s| {
        matches!(
            s,
            Step::Rename {
                expect: Expect::Original,
                ..
            }
        )
    });
    let root = fx.root_a.clone();
    let aside = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = aside.clone();
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: retire,
            at: "begin",
        },
        move || {
            *slot.borrow_mut() = Some(substitute_root(&root));
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
    assert!(matches!(state, TxnState::Blocked { .. }), "{state:?}");
    let aside = aside.borrow_mut().take().unwrap();
    let kept = aside.join("nes").join("Game (USA).nes");
    assert_eq!(
        read(&kept).as_deref(),
        Some(&fx.content[..]),
        "the original is kept"
    );
    restore_root(&fx.root_a, &aside);
    fx.assert_safe(&plan, "after substitution");
    let state = recover(&fx.ctx(), &plan.txn_id).unwrap();
    assert!(state.is_terminal(), "{state:?}");
    fx.assert_safe(&plan, "recovered");
}

#[test]
fn lower_level_entry_points_refuse_without_the_test_authority() {
    let fx = Fx::new(cross_available());
    let (plan, digest) = fx.plan();
    let crash = FaultPlan::at(
        Point::Step {
            step: 1,
            at: "begin",
        },
        Action::Crash,
    );
    assert!(matches!(
        apply(&fx.ctx_f(&crash), &plan, &digest),
        Err(EngineError::Crashed(_))
    ));
    let production = Context::new(&fx.journal);
    let before = fx.records(&plan.txn_id);
    assert!(
        recover(&production, &plan.txn_id)
            .unwrap_err()
            .to_string()
            .contains("cannot run in this build")
    );
    assert!(recover_all(&production).iter().all(|(_, r)| r.is_err()));
    assert_eq!(
        fx.records(&plan.txn_id),
        before,
        "a refused recovery appends nothing"
    );
    // Undo of a completed relocation is refused the same way.
    assert_eq!(
        recover(&fx.ctx(), &plan.txn_id).unwrap(),
        TxnState::Complete
    );
    let undo = plan_undo(&fx.ctx(), &plan.txn_id).unwrap();
    let d = plan_digest(&undo);
    assert!(
        apply_undo(&production, &undo, &d)
            .unwrap_err()
            .to_string()
            .contains("cannot run in this build")
    );
    fx.assert_complete(&plan, "undo refused");
    // A plan whose policy was edited to Production (and re-approved) is not
    // executable by the test authority either.
    let fx2 = Fx::new(false);
    let (mut p2, _) = fx2.plan();
    p2.filesystem_policy = super::fscompat::FsPolicy::Production;
    let strict_edit = {
        let mut p = p2.clone();
        p.filesystem_policy = super::fscompat::FsPolicy::TestStrict;
        p
    };
    // A scratch authority does not execute a plan relabelled strict either.
    let ds = plan_digest(&strict_edit);
    assert!(
        apply(&fx2.ctx(), &strict_edit, &ds)
            .unwrap_err()
            .to_string()
            .contains("cannot run in this build")
    );
    let d2 = plan_digest(&p2);
    assert!(
        apply(&fx2.ctx(), &p2, &d2)
            .unwrap_err()
            .to_string()
            .contains("cannot run in this build")
    );
}

fn step_index(plan: &Plan, f: impl Fn(&Step) -> bool) -> usize {
    plan.steps.iter().position(f).unwrap()
}

#[test]
fn read_only_remount_before_retirement_stops_and_recovers_after_restore() {
    if !cross_available() {
        eprintln!("SKIPPED: no second ext4 filesystem");
        return;
    }
    let _guard = OverrideGuard;
    let fx = Fx::new(true);
    let (plan, digest) = fx.plan();
    let retire = step_index(&plan, |s| {
        matches!(
            s,
            Step::Rename {
                expect: Expect::Original,
                ..
            }
        )
    });
    let root = fx.root_a.clone();
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: retire,
            at: "begin",
        },
        move || {
            simulate_mount(root.clone(), remounted_read_only);
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
    assert!(
        matches!(state, TxnState::Blocked { .. }),
        "a read-only source drive must stop the move: {state:?}"
    );
    fx.assert_safe(&plan, "after the read-only remount");
    assert_eq!(
        read(&fx.source).as_deref(),
        Some(&fx.content[..]),
        "the original was not retired"
    );
    // Recovery while the drive is still read-only stays blocked...
    assert!(matches!(
        recover(&fx.ctx(), &plan.txn_id).unwrap(),
        TxnState::Blocked { .. }
    ));
    // ...and finishes once the administrator has restored it.
    super::fscompat::test_override::clear();
    let state = recover(&fx.ctx(), &plan.txn_id).unwrap();
    assert!(state.is_terminal(), "{state:?}");
    fx.assert_safe(&plan, "after recovery");
    if state.is_success() {
        fx.assert_complete(&plan, "recovered");
    } else {
        fx.assert_compensated(&plan, "recovered");
    }
}

#[test]
fn storage_vanishing_or_changing_options_mid_copy_never_completes() {
    if !cross_available() {
        eprintln!("SKIPPED: no second ext4 filesystem");
        return;
    }
    type Change = fn(&super::fscompat::MountFacts) -> Result<super::fscompat::MountFacts, String>;
    let changes: [(&str, Change); 3] = [
        ("vanished", |_| Err("no such mount".into())),
        ("options changed", |f| {
            let mut f = f.clone();
            f.journal_options = Some(vec![
                "barrier".into(),
                "data=journal".into(),
                "errors=remount-ro".into(),
            ]);
            Ok(f)
        }),
        ("barriers off", |f| {
            let mut f = f.clone();
            if let Some(j) = &mut f.journal_options {
                j.retain(|o| o != "barrier");
                j.push("nobarrier".into());
            }
            Ok(f)
        }),
    ];
    for (what, change) in changes {
        let _guard = OverrideGuard;
        let fx = Fx::new(true);
        let (plan, digest) = fx.plan();
        let verify = step_index(&plan, |s| matches!(s, Step::VerifyStaged { .. }));
        let root = fx.root_b.clone();
        let f = FaultPlan::tracing().with_hook(
            Point::Step {
                step: verify,
                at: "begin",
            },
            move || {
                simulate_mount(root.clone(), change);
            },
        );
        let state = apply(&fx.ctx_f(&f), &plan, &digest).unwrap();
        assert!(!state.is_success(), "{what}: {state:?}");
        fx.assert_safe(&plan, what);
        assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]), "{what}");
        super::fscompat::test_override::clear();
        let state = recover(&fx.ctx(), &plan.txn_id).unwrap();
        assert!(state.is_terminal(), "{what}: {state:?}");
        fx.assert_safe(&plan, &format!("{what}, recovered"));
    }
}

/// End to end on administrator-provided ext4 folders mounted with
/// errors=remount-ro (the production configuration). Set
/// `EMUWIZ_OUROBOROS_STRICT_FS=/first/dir:/second/dir`, two folders on two
/// different such filesystems.
/// `EMUWIZ_OUROBOROS_STRICT_FS=/a:/b`: two different disposable ext4
/// filesystems mounted `errors=remount-ro` (see the design document, 4.3).
fn strict_fixtures() -> Option<(PathBuf, PathBuf)> {
    let spec = std::env::var_os("EMUWIZ_OUROBOROS_STRICT_FS")?;
    let spec = spec.to_string_lossy().into_owned();
    let (a, b) = spec
        .split_once(':')
        .expect("EMUWIZ_OUROBOROS_STRICT_FS=/a:/b");
    Some((PathBuf::from(a), PathBuf::from(b)))
}

const STRICT_SKIP: &str = "SKIPPED: EMUWIZ_OUROBOROS_STRICT_FS unset; the production errors=remount-ro \
    configuration is NOT exercised on this host (its ext4 mounts use errors=continue)";

#[test]
fn strict_filesystems_end_to_end() {
    let Some((first, second)) = strict_fixtures() else {
        eprintln!("{STRICT_SKIP}");
        return;
    };
    for dir in [&first, &second] {
        let facts = super::fscompat::inspect(dir).unwrap();
        super::fscompat::supported(&facts, super::fscompat::FsPolicy::Production).unwrap_or_else(
            |e| {
                panic!(
                    "{} is not a production-grade filesystem: {e}",
                    dir.display()
                )
            },
        );
    }
    // Cross-filesystem (copy, verify, publish, commit, retain) and same
    // filesystem (atomic rename), each under the full production policy.
    for (b_parent, cross) in [(&second, true), (&first, false)] {
        let a = tempfile::tempdir_in(&first).unwrap();
        let b = tempfile::tempdir_in(b_parent).unwrap();
        let strict = super::authority::ScratchAuthority::strict();
        let fx = Fx::in_dirs_with(a, b, cross, pattern(SMALL, 21), strict);
        let (plan, digest) = fx.plan();
        assert_eq!(
            plan.filesystem_policy,
            super::fscompat::FsPolicy::TestStrict
        );
        assert!(
            plan.effects.iter().all(|e| !e.contains("errors=continue")),
            "{:?}",
            plan.effects
        );
        assert_eq!(
            apply(&fx.ctx(), &plan, &digest).unwrap(),
            TxnState::Complete
        );
        fx.assert_complete(
            &plan,
            if cross {
                "strict, cross filesystem"
            } else {
                "strict, same filesystem"
            },
        );
    }
}

/// `EMUWIZ_OUROBOROS_CONTINUE_FS=/c`: a disposable ext4 fixture identical
/// to the strict ones except `errors=continue`.
#[test]
fn strict_policy_accepts_the_remount_ro_fixture_and_refuses_its_continue_twin() {
    let (Some((strict, _)), Some(cont)) = (
        strict_fixtures(),
        std::env::var_os("EMUWIZ_OUROBOROS_CONTINUE_FS").map(PathBuf::from),
    ) else {
        eprintln!("{STRICT_SKIP} (also needs EMUWIZ_OUROBOROS_CONTINUE_FS)");
        return;
    };
    use super::fscompat::{FsPolicy::*, inspect, supported};
    let (s, c) = (inspect(&strict).unwrap(), inspect(&cont).unwrap());
    // Otherwise equivalent: same type and options apart from errors=.
    let without_errors = |o: &Option<Vec<String>>| -> Vec<String> {
        o.iter()
            .flatten()
            .filter(|x| !x.starts_with("errors="))
            .cloned()
            .collect()
    };
    assert_eq!(s.fstype, c.fstype);
    assert_eq!(s.mount_options, c.mount_options);
    assert_eq!(
        without_errors(&s.journal_options),
        without_errors(&c.journal_options)
    );
    assert!(
        s.journal_options
            .iter()
            .flatten()
            .any(|o| o == "errors=remount-ro"),
        "{s:?}"
    );
    assert!(
        c.journal_options
            .iter()
            .flatten()
            .any(|o| o == "errors=continue"),
        "{c:?}"
    );
    // The strict policy (production's, and the strict test authority's)
    // accepts the first and refuses the twin; only the scratch policy
    // accepts the twin, with its note.
    assert!(supported(&s, Production).unwrap().is_empty());
    assert!(supported(&s, TestStrict).unwrap().is_empty());
    for policy in [Production, TestStrict] {
        let err = supported(&c, policy).unwrap_err();
        assert!(err.contains("errors=continue"), "{err}");
    }
    assert!(
        supported(&c, TestScratch)
            .unwrap()
            .iter()
            .any(|n| n.contains("test scratch folders only"))
    );

    // End to end: a strict authority cannot plan a move onto the twin.
    let a = tempfile::tempdir_in(&strict).unwrap();
    let b = tempfile::tempdir_in(&cont).unwrap();
    let fx = Fx::in_dirs_with(
        a,
        b,
        true,
        pattern(SMALL, 22),
        super::authority::ScratchAuthority::strict(),
    );
    let preview = plan_relocation_scratch(&fx.db, &fx.source, &fx.dest_dir, &fx.auth);
    assert!(
        preview.plan.is_none(),
        "no plan onto an errors=continue drive"
    );
    assert!(
        preview
            .blockers
            .iter()
            .any(|b| b.contains("errors=continue")),
        "{:?}",
        preview.blockers
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));

    // The production planner on the fully eligible strict fixture yields an
    // applicable-looking Production plan, and still nothing executes it:
    // not the ordinary context, not the strict test authority.
    let a = tempfile::tempdir_in(&strict).unwrap();
    let b = tempfile::tempdir_in(strict_fixtures().unwrap().1).unwrap();
    let fx = Fx::in_dirs_with(
        a,
        b,
        true,
        pattern(SMALL, 23),
        super::authority::ScratchAuthority::strict(),
    );
    let production = plan_relocation(&fx.db, &fx.source, &fx.dest_dir);
    assert!(
        production.blockers.iter().all(|b| !b.contains("errors=")),
        "the strict fixture meets the production filesystem policy: {:?}",
        production.blockers
    );
    if let (Some(p), Some(d)) = (&production.plan, &production.digest) {
        assert_eq!(p.filesystem_policy, Production);
        for ctx in [Context::new(&fx.journal), fx.ctx()] {
            let err = apply(&ctx, p, d).unwrap_err().to_string();
            assert!(err.contains("cannot run in this build"), "{err}");
        }
    }
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    let journalled = super::journal::list(&fx.journal)
        .map(|(ids, _)| ids)
        .unwrap_or_default();
    assert!(
        journalled.is_empty(),
        "nothing was journalled: {journalled:?}"
    );
}

#[test]
fn catalogue_commits_use_a_durable_connection() {
    // D5 is established, not assumed: the transformation's catalogue commit
    // runs with synchronous=EXTRA, so in rollback-journal mode the journal's
    // unlink (the commit point) is directory-fsynced.
    let fx = Fx::new(false);
    let c = crate::database::organisation_paths::open_durable_connection(fx.db.path()).unwrap();
    let level: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
    assert_eq!(level, 3, "synchronous=EXTRA");
    let mode: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert!(
        matches!(mode.as_str(), "delete" | "truncate" | "persist" | "wal"),
        "{mode}"
    );
}

/// Defence in depth for the compile-time boundary (the boundary itself is
/// `cfg(test)` plus crate privacy, proven by the compile_fail doctests in
/// `authority.rs`, the external-caller integration test
/// `tests/ouroboros_executor_boundary.rs` and the symbol checks recorded in
/// the design document). This keeps the boundary from being widened later:
/// no Cargo feature may gate the authority, and no production source may
/// name it.
#[test]
fn scratch_authority_is_unreachable_from_production_builds() {
    let core = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = core.parent().unwrap().parent().unwrap();
    for krate in ["archivefs-core", "archivefs-cli", "archivefs-gui"] {
        let manifest =
            std::fs::read_to_string(repo.join("crates").join(krate).join("Cargo.toml")).unwrap();
        let mut section = String::new();
        for line in manifest.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                section = t.to_string();
                continue;
            }
            if t.starts_with('#') {
                continue;
            }
            let core = krate == "archivefs-core";
            assert!(
                !t.contains("scratch-executor"),
                "{krate}/Cargo.toml: the executor has no feature: {t}"
            );
            if !t.contains("test-support") {
                continue;
            }
            let allowed = section == "[dev-dependencies]"
                || (core && section == "[features]" && t == "test-support = []")
                || (core
                    && (section == "[[example]]" || section == "[[test]]")
                    && t.starts_with("required-features"));
            assert!(
                allowed,
                "{krate}/Cargo.toml enables test-support in {section}: {t}"
            );
        }
    }
    for script in ["scripts/build-release.sh", ".github/workflows/release.yml"] {
        let text = std::fs::read_to_string(repo.join(script)).unwrap();
        for flag in [
            "--all-features",
            "--features",
            "test-support",
            "scratch-executor",
            "--all-targets",
            "--tests",
            "--examples",
        ] {
            let used = text
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .any(|l| l.contains(flag));
            assert!(!used, "{script} builds release binaries with {flag}");
        }
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    for krate in ["archivefs-core", "archivefs-cli", "archivefs-gui"] {
        walk(&repo.join("crates").join(krate).join("src"), &mut files);
    }
    // The authority's constructors are gated by cfg(test) alone, never by a
    // feature that another crate or --all-features could switch on.
    let authority = std::fs::read_to_string(core.join("src/transform/authority.rs")).unwrap();
    for line in authority
        .lines()
        .filter(|l| l.trim_start().starts_with("#[cfg"))
    {
        assert_eq!(
            line.trim(),
            "#[cfg(test)]",
            "authority.rs gate widened: {line}"
        );
    }
    assert!(
        authority.contains("pub(crate) struct ScratchAuthority"),
        "the authority type stays crate-private"
    );
    let transform_dir = core.join("src").join("transform");
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let test_file =
            name == "tests.rs" || name.ends_with("_tests.rs") || name == "model_check.rs";
        if f.starts_with(&transform_dir) || test_file {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for word in [
            "ScratchAuthority",
            "with_authority",
            "provision_root",
            "plan_relocation_scratch",
        ] {
            assert!(
                !text.contains(word),
                "{} refers to {word}; only tests may hold the executor authority",
                f.display()
            );
        }
    }
}

#[test]
fn a_bind_mount_appearing_over_the_game_folder_stops_the_move() {
    // A bind mount of the same filesystem over the destination folder keeps
    // the device number; only the mount table shows it. Simulated through
    // the mount-table seam (no real mount is made).
    let fx = Fx::new(false);
    let (plan, digest) = fx.plan();
    let _guard = OverrideGuard;
    let folder = plan.destination_path.parent().unwrap().to_path_buf();
    let f = FaultPlan::tracing().with_hook(
        Point::Step {
            step: 1,
            at: "begin",
        },
        move || {
            let folder = folder.clone();
            super::fscompat::test_override::set(move |path, facts| {
                path.starts_with(&folder).then(|| {
                    let mut f = facts.clone();
                    f.mount_point = folder.clone();
                    Ok(f)
                })
            });
        },
    );
    let state = apply(&fx.ctx_f(&f), &plan, &digest);
    assert!(!matches!(state, Ok(TxnState::Complete)), "{state:?}");
    fx.assert_safe(&plan, "bind mount over the folder");
    super::fscompat::test_override::clear();
    let state = recover(&fx.ctx(), &plan.txn_id).unwrap();
    assert!(state.is_terminal(), "{state:?}");
    fx.assert_safe(&plan, "recovered after the bind mount went away");
}

#[test]
fn the_published_copy_keeps_the_originals_permissions() {
    use std::os::unix::fs::PermissionsExt;
    if !cross_available() {
        eprintln!("SKIPPED: no second ext4 filesystem");
        return;
    }
    let fx = Fx::new(true);
    std::fs::set_permissions(&fx.source, std::fs::Permissions::from_mode(0o640)).unwrap();
    let (plan, digest) = fx.plan();
    assert_eq!(
        apply(&fx.ctx(), &plan, &digest).unwrap(),
        TxnState::Complete
    );
    let mode = std::fs::metadata(&plan.destination_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode, 0o640, "the copy has the original's permission bits");
}

#[test]
fn edited_or_forged_plan_shapes_are_refused_even_with_the_authority() {
    let fx = Fx::new(false);
    let (plan, _) = fx.plan();
    let outside = fx.root_a.parent().unwrap().join("outside.nes");
    let mut variants: Vec<(&str, Plan)> = Vec::new();
    let mut p = plan.clone();
    if let Some(Step::Rename { to, .. }) = p
        .steps
        .iter_mut()
        .find(|s| matches!(s, Step::Rename { .. }))
    {
        *to = outside.clone();
    }
    variants.push(("rename target outside the roots", p));
    let mut p = plan.clone();
    if let Some(Step::Rename { roots, .. }) = p
        .steps
        .iter_mut()
        .find(|s| matches!(s, Step::Rename { .. }))
    {
        *roots = vec![PathBuf::from("/")];
    }
    variants.push(("rename trusted roots widened", p));
    let mut p = plan.clone();
    p.point_of_no_return = 0;
    variants.push(("point of no return moved", p));
    let mut p = plan.clone();
    p.steps.insert(
        1,
        Step::DiscardDuplicate {
            path: fx.source.clone(),
            keeper: fx.source.clone(),
            root: fx.root_a.clone(),
        },
    );
    p.point_of_no_return += 1;
    variants.push(("an unlink step added", p));
    let mut p = plan.clone();
    p.txn_id = "../../../../tmp/escape-000000000".into();
    variants.push(("malformed transaction id", p));
    let mut p = plan.clone();
    p.source_path = outside.clone();
    variants.push(("source outside its root", p));
    for (what, p) in &variants {
        let d = plan_digest(p);
        let err = apply(&fx.ctx(), p, &d).unwrap_err().to_string();
        assert!(
            err.contains("not one the planner produces"),
            "{what}: {err}"
        );
        assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]), "{what}");
        assert!(!exists(&outside), "{what}");
    }
    // The same edit read back from a journal is refused by recovery, which
    // appends nothing.
    let (_, edited) = &variants[0];
    journal::ensure_root(&fx.journal).unwrap();
    let mut w = journal::JournalWriter::create(
        &fx.journal,
        &edited.txn_id,
        &engine::plan_bytes(edited),
        None,
    )
    .unwrap();
    w.append(Event::Begun {
        pid: std::process::id(),
        engine: super::model::ENGINE.into(),
    })
    .unwrap();
    drop(w);
    let before = fx.records(&edited.txn_id);
    let err = recover(&fx.ctx(), &edited.txn_id).unwrap_err().to_string();
    assert!(err.contains("not one the planner produces"), "{err}");
    assert_eq!(fx.records(&edited.txn_id), before, "nothing appended");
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    // An Undo plan with an edited step is refused too.
    let fx = Fx::new(false);
    let (plan, digest) = fx.plan();
    assert_eq!(
        apply(&fx.ctx(), &plan, &digest).unwrap(),
        TxnState::Complete
    );
    let mut undo = plan_undo(&fx.ctx(), &plan.txn_id).unwrap();
    if let Some(Step::Rename { to, .. }) = undo
        .steps
        .iter_mut()
        .find(|s| matches!(s, Step::Rename { .. }))
    {
        *to = outside.clone();
    }
    let d = plan_digest(&undo);
    let err = apply_undo(&fx.ctx(), &undo, &d).unwrap_err().to_string();
    assert!(err.contains("not one the planner produces"), "{err}");
    fx.assert_complete(&plan, "edited undo refused");
}

/// Review 2026-11-14 (N1, N3): an Undo plan is bound to the relocation it
/// reverses, field by field, and every plan's catalogue is the database its
/// binding writes. Edits that keep a valid step shape are still refused, so
/// an Undo can never discard a file its relocation did not publish.
#[test]
fn an_undo_plan_must_match_its_relocation_and_its_catalogue_binding() {
    let fx = Fx::new(false);
    let (plan, digest) = fx.plan();
    // A forward plan naming a different catalogue than its binding writes.
    let mut p = plan.clone();
    p.catalogue = fx.root_a.parent().unwrap().join("other.sqlite3");
    let err = apply(&fx.ctx(), &p, &plan_digest(&p))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not the database its binding writes"), "{err}");
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
    assert_eq!(
        apply(&fx.ctx(), &plan, &digest).unwrap(),
        TxnState::Complete
    );
    let undo = plan_undo(&fx.ctx(), &plan.txn_id).unwrap();
    let mut variants: Vec<(&str, Plan)> = Vec::new();
    let mut u = undo.clone();
    if let PlanKind::Undo {
        of_complete_sum, ..
    } = &mut u.kind
    {
        of_complete_sum.clear();
    }
    variants.push(("recorded outcome", u));
    let mut u = undo.clone();
    if let PlanKind::Undo { of_plan_digest, .. } = &mut u.kind {
        *of_plan_digest = "0".repeat(64);
    }
    variants.push(("plan digest", u));
    let mut u = undo.clone();
    u.published_identity = None;
    variants.push(("published identity", u));
    let mut u = undo.clone();
    u.source_identity.size_bytes += 1;
    variants.push(("original identity", u));
    for (what, u) in &variants {
        let err = apply_undo(&fx.ctx(), u, &plan_digest(u))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("does not match the relocation it reverses") && err.contains(what),
            "{what}: {err}"
        );
        fx.assert_complete(&plan, what);
    }
    // The genuine Undo still applies.
    assert_eq!(
        apply_undo(&fx.ctx(), &undo, &plan_digest(&undo)).unwrap(),
        TxnState::Complete
    );
    assert_eq!(read(&fx.source).as_deref(), Some(&fx.content[..]));
}
