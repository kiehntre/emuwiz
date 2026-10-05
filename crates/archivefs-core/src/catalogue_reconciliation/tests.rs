use super::db::{ReconciliationOutcome, reconcile_library};
use super::hashing::hash_files;
use super::walk::{RootWalkState, WalkLimits, WalkRoot, default_mount_of, walk_roots};
use super::*;
use std::fs;
use std::sync::atomic::AtomicBool;

fn h1(n: u8) -> StrongHash {
    StrongHash::new(StrongHashAlgorithm::Sha1, &format!("{n:02x}").repeat(20)).unwrap()
}
fn h256(n: u8) -> StrongHash {
    StrongHash::new(StrongHashAlgorithm::Sha256, &format!("{n:02x}").repeat(32)).unwrap()
}
fn row(
    id: i64,
    path: &str,
    size: Option<u64>,
    presence: RowPresence,
    hashes: Vec<StrongHash>,
) -> RowFacts {
    RowFacts {
        archive_id: id,
        source_id: 1,
        path: path.into(),
        size,
        platform: Some("Atari2600".into()),
        presence,
        hashes,
    }
}
fn file(path: &str, size: u64, hashes: Vec<StrongHash>) -> FileFacts {
    FileFacts {
        path: path.into(),
        source_id: 1,
        size,
        hashes,
    }
}
fn state(report: &ReconciliationReport, id: i64) -> &RowState {
    &report
        .rows
        .iter()
        .find(|r| r.archive_id == id)
        .unwrap()
        .state
}
fn file_state<'a>(report: &'a ReconciliationReport, path: &str) -> &'a FileState {
    &report
        .files
        .iter()
        .find(|f| f.path == Path::new(path))
        .unwrap()
        .state
}

#[test]
fn physical_file_with_a_row_is_present_and_one_without_is_uncatalogued() {
    let report = reconcile(
        &[row(1, "/lib/a.bin", Some(4), RowPresence::Present, vec![])],
        &[
            file("/lib/a.bin", 4, vec![]),
            file("/lib/new.bin", 4, vec![]),
        ],
    );
    assert_eq!(*state(&report, 1), RowState::CataloguedPresent);
    assert_eq!(
        *file_state(&report, "/lib/a.bin"),
        FileState::Catalogued { archive_id: 1 }
    );
    assert_eq!(
        *file_state(&report, "/lib/new.bin"),
        FileState::Uncatalogued
    );
    assert_eq!(report.counts.uncatalogued_files, 1);
}

#[test]
fn a_missing_row_with_nothing_related_is_missing() {
    let report = reconcile(
        &[row(
            1,
            "/lib/gone.bin",
            Some(4),
            RowPresence::Missing,
            vec![h1(1)],
        )],
        &[file("/lib/other.bin", 99, vec![])],
    );
    assert_eq!(*state(&report, 1), RowState::CatalogueFileMissing);
    assert_eq!(report.counts.missing, 1);
}

#[test]
fn unique_exact_hash_is_a_strong_candidate_even_after_a_rename() {
    let report = reconcile(
        &[row(
            1,
            "/lib/old name.bin",
            Some(4),
            RowPresence::Missing,
            vec![h1(7)],
        )],
        &[file("/lib/Renamed (USA).bin", 4, vec![h1(7), h256(9)])],
    );
    assert_eq!(
        *state(&report, 1),
        RowState::StrongMoveCandidate {
            to: "/lib/Renamed (USA).bin".into()
        }
    );
    assert_eq!(
        *file_state(&report, "/lib/Renamed (USA).bin"),
        FileState::StrongMoveTarget { archive_id: 1 }
    );
}

#[test]
fn same_filename_with_a_different_hash_is_not_a_move() {
    let report = reconcile(
        &[row(
            1,
            "/old/Game.bin",
            Some(4),
            RowPresence::Missing,
            vec![h1(1)],
        )],
        &[file("/new/Game.bin", 4, vec![h1(2)])],
    );
    assert_eq!(*state(&report, 1), RowState::CatalogueFileMissing);
}

#[test]
fn name_and_size_alone_is_weak_never_a_proposal() {
    let with_hash = reconcile(
        &[row(
            1,
            "/old/Game.bin",
            Some(4),
            RowPresence::Missing,
            vec![h1(1)],
        )],
        &[file("/new/game.BIN", 4, vec![])],
    );
    assert!(matches!(
        state(&with_hash, 1),
        RowState::PossiblyMoved {
            basis: WeakBasis::NameAndSize,
            proof: MoveProof::NeedsHashing,
            ..
        }
    ));
    assert_eq!(
        with_hash.hash_requests,
        vec![PathBuf::from("/new/game.BIN")]
    );

    let without_hash = reconcile(
        &[row(
            1,
            "/old/Game.bin",
            Some(4),
            RowPresence::Missing,
            vec![],
        )],
        &[file("/new/Game.bin", 4, vec![])],
    );
    assert!(matches!(
        state(&without_hash, 1),
        RowState::PossiblyMoved {
            proof: MoveProof::NoPersistedHash,
            ..
        }
    ));
    assert!(without_hash.hash_requests.is_empty());
    assert_eq!(without_hash.counts.strong_move_candidates, 0);

    // A file name with no size to corroborate it is nothing at all.
    let nameless = reconcile(
        &[row(1, "/old/Game.bin", None, RowPresence::Missing, vec![])],
        &[file("/new/Game.bin", 4, vec![])],
    );
    assert_eq!(*state(&nameless, 1), RowState::CatalogueFileMissing);
}

#[test]
fn one_row_matching_several_files_is_ambiguous() {
    let report = reconcile(
        &[row(
            1,
            "/old/a.bin",
            Some(4),
            RowPresence::Missing,
            vec![h1(5)],
        )],
        &[
            file("/n/a1.bin", 4, vec![h1(5)]),
            file("/n/a2.bin", 4, vec![h1(5)]),
        ],
    );
    match state(&report, 1) {
        RowState::AmbiguousMoveCandidates {
            reason,
            candidates,
            candidates_total,
        } => {
            assert_eq!(*reason, AmbiguityReason::ManyFilesOneRow);
            assert_eq!((candidates.len(), *candidates_total), (2, 2));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(report.counts.strong_move_candidates, 0);
    assert!(matches!(
        file_state(&report, "/n/a1.bin"),
        FileState::ContendedMoveTarget { .. }
    ));
}

#[test]
fn several_rows_matching_one_file_are_all_ambiguous() {
    let report = reconcile(
        &[
            row(1, "/old/a.bin", Some(4), RowPresence::Missing, vec![h1(5)]),
            row(2, "/old/b.bin", Some(4), RowPresence::Missing, vec![h1(5)]),
        ],
        &[file("/n/x.bin", 4, vec![h1(5)])],
    );
    for id in [1, 2] {
        assert!(matches!(
            state(&report, id),
            RowState::AmbiguousMoveCandidates {
                reason: AmbiguityReason::ManyRowsOneFile,
                ..
            }
        ));
    }
    assert_eq!(
        *file_state(&report, "/n/x.bin"),
        FileState::ContendedMoveTarget {
            archive_ids: vec![1, 2]
        }
    );
}

#[test]
fn conflicting_persisted_identities_fail_closed() {
    let report = reconcile(
        &[row(
            1,
            "/old/a.bin",
            Some(4),
            RowPresence::Missing,
            vec![h1(1), h1(2)],
        )],
        &[file("/n/a.bin", 4, vec![h1(1)])],
    );
    assert!(matches!(state(&report, 1), RowState::Conflict { .. }));

    // The same strong hash on a file of a different size contradicts itself.
    let report = reconcile(
        &[row(
            1,
            "/old/a.bin",
            Some(4),
            RowPresence::Missing,
            vec![h1(1)],
        )],
        &[file("/n/a.bin", 5, vec![h1(1)])],
    );
    assert!(matches!(state(&report, 1), RowState::Conflict { .. }));

    // A different SHA-256 alongside an equal SHA-1: not the same file.
    let report = reconcile(
        &[row(
            1,
            "/old/a.bin",
            Some(4),
            RowPresence::Missing,
            vec![h1(1), h256(1)],
        )],
        &[file("/n/a.bin", 4, vec![h1(1), h256(2)])],
    );
    assert!(matches!(state(&report, 1), RowState::Conflict { .. }));
}

#[test]
fn unavailable_storage_is_unknown_never_missing_and_never_pairs() {
    for reason in [
        UnprovableReason::StorageUnavailable,
        UnprovableReason::NestedBoundaryUnproven,
        UnprovableReason::SourceNeedsReview,
        UnprovableReason::Inaccessible,
    ] {
        let report = reconcile(
            &[row(
                1,
                "/old/a.bin",
                Some(4),
                RowPresence::Unprovable(reason),
                vec![h1(1)],
            )],
            &[file("/n/a.bin", 4, vec![h1(1)])],
        );
        assert_eq!(*state(&report, 1), RowState::Unknown { reason });
        assert_eq!(*file_state(&report, "/n/a.bin"), FileState::Uncatalogued);
        assert_eq!(report.counts.missing, 0);
    }
}

#[test]
fn persisted_platform_survives_a_stale_path_and_files_carry_none() {
    let report = reconcile(
        &[row(
            1,
            "/atari2600/gone.bin",
            Some(4),
            RowPresence::Missing,
            vec![],
        )],
        &[file("/atari2600/stray.bin", 4, vec![])],
    );
    assert_eq!(report.rows[0].platform.as_deref(), Some("Atari2600"));
    // A file outcome has no platform at all: a folder name is not identity.
    assert_eq!(
        *file_state(&report, "/atari2600/stray.bin"),
        FileState::Uncatalogued
    );
}

#[test]
fn results_do_not_depend_on_input_order() {
    let rows: Vec<_> = (1..=40)
        .map(|i| {
            row(
                i,
                &format!("/old/{i}.bin"),
                Some(i as u64 % 5),
                if i % 3 == 0 {
                    RowPresence::Present
                } else {
                    RowPresence::Missing
                },
                if i % 2 == 0 {
                    vec![h1(i as u8 % 7)]
                } else {
                    vec![]
                },
            )
        })
        .collect();
    let files: Vec<_> = (1..=60)
        .map(|i| {
            file(
                &format!("/n/{i}.bin"),
                i as u64 % 5,
                if i % 4 == 0 {
                    vec![h1(i as u8 % 7)]
                } else {
                    vec![]
                },
            )
        })
        .collect();
    let forward = reconcile(&rows, &files);
    let (mut r2, mut f2) = (rows.clone(), files.clone());
    r2.reverse();
    f2.reverse();
    f2.rotate_left(7);
    assert_eq!(forward, reconcile(&r2, &f2));
}

#[test]
fn scales_to_tens_of_thousands_without_pairwise_matching() {
    // 20,000 files of just four distinct sizes: a naive all-pairs match over
    // 4,000 missing rows would be 80 million comparisons.
    let files: Vec<_> = (0..20_000)
        .map(|i| file(&format!("/lib/f{i:05}.bin"), 4096 + (i % 4) as u64, vec![]))
        .collect();
    let mut rows = Vec::new();
    for i in 0..4_000 {
        rows.push(row(
            i + 1,
            &format!("/gone/g{i:05}.bin"),
            Some(4096 + (i % 4) as u64),
            RowPresence::Missing,
            vec![h1((i % 200) as u8)],
        ));
    }
    let started = std::time::Instant::now();
    let report = reconcile(&rows, &files);
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    assert_eq!(report.counts.uncatalogued_files, 20_000);
    assert_eq!(report.counts.possibly_moved, 4_000);
    assert!(report.hash_requests.len() <= 4_000 * MAX_LISTED_CANDIDATES);
}

// ---- filesystem walking ----------------------------------------------------

fn tree() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("atari2600");
    fs::create_dir_all(root.join("sub/deeper")).unwrap();
    fs::write(root.join("a.bin"), b"aaaa").unwrap();
    fs::write(root.join("b.xyz"), b"bb").unwrap();
    fs::write(root.join("sub/c.sta"), b"c").unwrap();
    fs::write(root.join("sub/deeper/d"), b"dddd").unwrap();
    std::os::unix::fs::symlink(root.join("a.bin"), root.join("link.bin")).unwrap();
    (dir, root)
}

fn walk(
    root: &Path,
    mount: impl Fn(&Path) -> std::io::Result<u64>,
    cancel: &AtomicBool,
) -> walk::WalkReport {
    walk_roots(
        &[WalkRoot {
            source_id: 1,
            path: root.to_path_buf(),
            excluded: vec![],
        }],
        WalkLimits::default(),
        cancel,
        &mount,
    )
}

#[test]
fn walk_lists_every_regular_file_regardless_of_extension_and_skips_symlinks() {
    let (_dir, root) = tree();
    let report = walk(&root, default_mount_of, &AtomicBool::new(false));
    let names: Vec<_> = report
        .files
        .iter()
        .map(|f| {
            f.path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["a.bin", "b.xyz", "sub/c.sta", "sub/deeper/d"]);
    assert_eq!(report.roots[0].state, RootWalkState::Complete);
}

#[test]
fn walk_does_not_enter_a_nested_filesystem_and_reports_it_partial() {
    let (_dir, root) = tree();
    let nested = root.join("sub");
    let report = walk(
        &root,
        |path| Ok(if path.starts_with(&nested) { 2 } else { 1 }),
        &AtomicBool::new(false),
    );
    assert_eq!(
        report.files.len(),
        2,
        "nothing beneath the boundary is listed"
    );
    assert_eq!(report.roots[0].state, RootWalkState::Partial);
    assert_eq!(report.roots[0].nested_boundaries, vec![nested]);
}

#[test]
fn walk_skips_excluded_child_roots_and_is_cancellable() {
    let (_dir, root) = tree();
    let report = walk_roots(
        &[WalkRoot {
            source_id: 1,
            path: root.clone(),
            excluded: vec![root.join("sub")],
        }],
        WalkLimits::default(),
        &AtomicBool::new(false),
        &default_mount_of,
    );
    assert_eq!(report.files.len(), 2);
    let cancelled = walk(&root, default_mount_of, &AtomicBool::new(true));
    assert!(cancelled.cancelled);
}

#[test]
fn a_missing_root_is_unavailable_not_empty() {
    let dir = tempfile::tempdir().unwrap();
    let report = walk(
        &dir.path().join("nope"),
        default_mount_of,
        &AtomicBool::new(false),
    );
    assert_eq!(report.roots[0].state, RootWalkState::Unavailable);
}

#[test]
fn explicit_hashing_is_bounded_cancellable_and_symlink_safe() {
    let (_dir, root) = tree();
    let a = root.join("a.bin");
    let out = hash_files(
        &[root.clone()],
        &[a.clone(), root.join("link.bin")],
        1 << 20,
        &AtomicBool::new(false),
    );
    let sha1 = out.hashes[&a]
        .iter()
        .find(|h| h.algorithm == StrongHashAlgorithm::Sha1)
        .unwrap();
    assert_eq!(sha1.hex, "70c881d4a26984ddce795f6f71817c9cf4480e79"); // sha1("aaaa")
    assert_eq!(
        out.unreadable,
        vec![root.join("link.bin")],
        "a symlink is never read"
    );
    let tiny = hash_files(&[root.clone()], &[a.clone()], 2, &AtomicBool::new(false));
    assert_eq!(tiny.over_budget, vec![a.clone()]);
    assert_eq!(tiny.bytes_read, 0);
    assert!(hash_files(&[root], &[a], 1 << 20, &AtomicBool::new(true)).cancelled);
}

// ---- catalogue-backed ------------------------------------------------------

mod catalogue {
    use super::*;
    use crate::Config;
    use crate::database::{Database, scan_and_persist};

    struct World {
        _dir: tempfile::TempDir,
        root: PathBuf,
        db: Database,
        config: Config,
    }

    fn world(files: &[(&str, &[u8])]) -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("atari2600");
        fs::create_dir_all(&root).unwrap();
        for (name, bytes) in files {
            fs::write(root.join(name), bytes).unwrap();
        }
        let config = Config {
            source_folders: vec![root.clone()],
            mount_root: dir.path().join("mounts"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        };
        let mut db = Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
        scan_and_persist(&mut db, &config, "test").unwrap();
        World {
            _dir: dir,
            root,
            db,
            config,
        }
    }

    fn run(world: &World, hashes: &BTreeMap<PathBuf, Vec<StrongHash>>) -> LibraryReconciliation {
        match reconcile_library(
            &world.db,
            &world.config.source_folders,
            hashes,
            &AtomicBool::new(false),
            &default_mount_of,
        )
        .unwrap()
        {
            ReconciliationOutcome::Complete(done) => *done,
            ReconciliationOutcome::Cancelled => panic!("cancelled"),
        }
    }

    use super::db::LibraryReconciliation;

    fn set_hash(world: &World, name: &str, hash: &str) {
        let connection = rusqlite::Connection::open(world.db.path()).unwrap();
        let path = world.root.join(name);
        use std::os::unix::ffi::OsStrExt;
        connection
            .execute(
                "UPDATE archives SET archive_hash = ?1 WHERE absolute_path_cached = ?2",
                rusqlite::params![hash, path.as_os_str().as_bytes()],
            )
            .unwrap();
    }

    fn platform_rows(world: &World) -> i64 {
        rusqlite::Connection::open(world.db.path())
            .unwrap()
            .query_row("SELECT count(*) FROM platform_assignments", [], |r| {
                r.get(0)
            })
            .unwrap()
    }

    #[test]
    fn atari_like_scenario_counts_and_no_identity_is_invented() {
        let w = world(&[("Combat.zip", b"combat"), ("Pitfall.zip", b"pitfall")]);
        // Uncatalogued strays in the same folder, in extensions the scan skips.
        fs::write(w.root.join("stray1.xyz"), b"s1").unwrap();
        fs::write(w.root.join("stray2.xyz"), b"s2").unwrap();
        // One catalogued file disappears; another reappears under a new name.
        let sha256 = {
            use sha2::{Digest, Sha256};
            Sha256::digest(b"pitfall")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        set_hash(&w, "Pitfall.zip", &sha256);
        fs::rename(
            w.root.join("Pitfall.zip"),
            w.root.join("Pitfall! (1982).xyz"),
        )
        .unwrap();
        fs::remove_file(w.root.join("Combat.zip")).unwrap();
        let before_platforms = platform_rows(&w);
        let before_rows = w.db.load_archives().unwrap();

        // Without any hashing: nothing is claimed as a move.
        let first = run(&w, &BTreeMap::new());
        assert_eq!(first.report.counts.uncatalogued_files, 3);
        assert_eq!(first.report.counts.strong_move_candidates, 0);
        assert_eq!(
            first.report.counts.missing + first.report.counts.possibly_moved,
            2
        );
        assert!(
            first.report.rows.iter().all(|r| r.platform.is_some()),
            "persisted platform survives"
        );
        let ask = first.report.hash_requests.clone();
        assert!(ask.contains(&w.root.join("Pitfall! (1982).xyz")), "{ask:?}");

        // The explicit, bounded hashing step settles it - and only that one.
        let hashed = hash_files(
            &w.config.source_folders,
            &ask,
            1 << 20,
            &AtomicBool::new(false),
        );
        let second = run(&w, &hashed.hashes);
        assert_eq!(second.report.counts.strong_move_candidates, 1);
        assert_eq!(
            second.report.counts.uncatalogued_files, 3,
            "still uncatalogued: nothing imported"
        );
        let moved = second
            .report
            .rows
            .iter()
            .find(|r| matches!(r.state, RowState::StrongMoveCandidate { .. }))
            .unwrap();
        assert_eq!(moved.platform.as_deref(), Some("Atari2600"));

        // Read-only throughout: the catalogue and the platforms are untouched.
        assert_eq!(w.db.load_archives().unwrap(), before_rows);
        assert_eq!(platform_rows(&w), before_platforms);
    }

    #[test]
    fn uncatalogued_files_in_an_atari_folder_get_no_platform() {
        let w = world(&[("Real.zip", b"real")]);
        fs::write(w.root.join("stray.xyz"), b"stray").unwrap();
        let before = platform_rows(&w);
        let out = run(&w, &BTreeMap::new());
        let stray = out
            .report
            .files
            .iter()
            .find(|f| f.path.ends_with("stray.xyz"))
            .unwrap();
        assert_eq!(stray.state, FileState::Uncatalogued);
        assert_eq!(platform_rows(&w), before);
        assert!(
            w.db.load_archives()
                .unwrap()
                .iter()
                .all(|a| !a.relative_path.ends_with("stray.xyz"))
        );
    }

    #[test]
    fn a_vanished_root_makes_rows_unknown_not_missing() {
        let w = world(&[("A.zip", b"a"), ("B.zip", b"b")]);
        fs::remove_dir_all(&w.root).unwrap();
        let out = run(&w, &BTreeMap::new());
        assert_eq!(out.report.counts.missing, 0);
        assert_eq!(out.report.counts.unknown, 2);
        assert_eq!(out.roots[0].state, RootWalkState::Unavailable);
    }

    #[test]
    fn a_root_no_longer_configured_is_not_reconciled() {
        let w = world(&[("A.zip", b"a")]);
        fs::remove_file(w.root.join("A.zip")).unwrap();
        let out = match reconcile_library(
            &w.db,
            &[],
            &BTreeMap::new(),
            &AtomicBool::new(false),
            &default_mount_of,
        )
        .unwrap()
        {
            ReconciliationOutcome::Complete(done) => *done,
            ReconciliationOutcome::Cancelled => panic!(),
        };
        assert_eq!(out.report.counts.unknown, 1);
        assert_eq!(out.report.counts.missing, 0);
    }

    #[test]
    fn cancellation_yields_no_report() {
        let w = world(&[("A.zip", b"a")]);
        let outcome = reconcile_library(
            &w.db,
            &w.config.source_folders,
            &BTreeMap::new(),
            &AtomicBool::new(true),
            &default_mount_of,
        )
        .unwrap();
        assert_eq!(outcome, ReconciliationOutcome::Cancelled);
    }
}
