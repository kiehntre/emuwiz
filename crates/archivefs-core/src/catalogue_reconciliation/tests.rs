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

// ---- referenced companions (CUE / GDI) --------------------------------------

#[test]
fn a_stale_or_unknown_parent_claims_no_companions() {
    let links = |id| {
        vec![CompanionLink {
            parent_archive_id: id,
            basis: CompanionBasis::CueFileReference,
            files: vec!["/lib/t1.bin".into()],
        }]
    };
    let files = [file("/lib/t1.bin", 4, vec![])];
    // Parent row exists but its file is gone: it cannot vouch for anything.
    let stale = reconcile_with_companions(
        &[row(
            1,
            "/lib/Game.cue",
            Some(1),
            RowPresence::Missing,
            vec![],
        )],
        &files,
        &links(1),
    );
    assert_eq!(*file_state(&stale, "/lib/t1.bin"), FileState::Uncatalogued);
    // A link naming no catalogue row at all is ignored.
    let none = reconcile_with_companions(&[], &files, &links(9));
    assert_eq!(*file_state(&none, "/lib/t1.bin"), FileState::Uncatalogued);
    // A present parent does claim it.
    let present = reconcile_with_companions(
        &[row(
            1,
            "/lib/Game.cue",
            Some(1),
            RowPresence::Present,
            vec![],
        )],
        &files,
        &links(1),
    );
    assert_eq!(present.counts.referenced_companions, 1);
    assert_eq!(present.counts.uncatalogued_files, 0);
}

#[test]
fn a_companion_is_never_a_move_target_even_for_a_matching_hash() {
    let report = reconcile_with_companions(
        &[
            row(1, "/lib/Game.cue", Some(1), RowPresence::Present, vec![]),
            row(
                2,
                "/old/lost.bin",
                Some(4),
                RowPresence::Missing,
                vec![h1(3)],
            ),
        ],
        &[file("/lib/t1.bin", 4, vec![h1(3)])],
        &[CompanionLink {
            parent_archive_id: 1,
            basis: CompanionBasis::CueFileReference,
            files: vec!["/lib/t1.bin".into()],
        }],
    );
    assert_eq!(*state(&report, 2), RowState::CatalogueFileMissing);
    assert!(matches!(
        file_state(&report, "/lib/t1.bin"),
        FileState::ReferencedCompanion {
            parent_archive_id: 1,
            ..
        }
    ));
}

mod companions {
    use super::*;
    use crate::Config;
    use crate::database::{Database, scan_and_persist};
    use std::os::unix::ffi::OsStrExt;

    struct World {
        _dir: tempfile::TempDir,
        root: PathBuf,
        db: Database,
        config: Config,
    }

    fn world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("games");
        fs::create_dir_all(&root).unwrap();
        let config = Config {
            source_folders: vec![root.clone()],
            mount_root: dir.path().join("mounts"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        };
        let mut db = Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
        // Registers and binds the source; the catalogue rows are added directly
        // so the test does not depend on what the scanner chooses to accept.
        scan_and_persist(&mut db, &config, "test").unwrap();
        World {
            _dir: dir,
            root,
            db,
            config,
        }
    }

    impl World {
        fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            path
        }
        fn catalogue(&self, rel: &str, kind: &str, hash: Option<&str>) -> i64 {
            let path = self.root.join(rel);
            let size = fs::metadata(&path).map(|m| m.len() as i64).ok();
            let connection = rusqlite::Connection::open(self.db.path()).unwrap();
            let source: i64 = connection
                .query_row("SELECT id FROM source_folders LIMIT 1", [], |r| r.get(0))
                .unwrap();
            connection
                .execute(
                    "INSERT INTO archives (source_folder_id, relative_path, absolute_path_cached, \
                     file_name_cached, archive_kind, display_name, normalized_name, size_bytes, \
                     archive_hash, first_seen_at, last_seen_at, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, 'now', 'now', 'now', 'now')",
                    rusqlite::params![
                        source,
                        rel.as_bytes(),
                        path.as_os_str().as_bytes(),
                        path.file_name().unwrap().as_bytes(),
                        kind,
                        rel,
                        size,
                        hash,
                    ],
                )
                .unwrap();
            connection.last_insert_rowid()
        }
        fn run(&self) -> super::super::db::LibraryReconciliation {
            match reconcile_library(
                &self.db,
                &self.config.source_folders,
                &BTreeMap::new(),
                &AtomicBool::new(false),
                &default_mount_of,
            )
            .unwrap()
            {
                ReconciliationOutcome::Complete(done) => *done,
                ReconciliationOutcome::Cancelled => panic!("cancelled"),
            }
        }
        fn at(&self, rel: &str, out: &super::super::db::LibraryReconciliation) -> FileState {
            let path = self.root.join(rel);
            out.report
                .files
                .iter()
                .find(|f| f.path == path)
                .unwrap()
                .state
                .clone()
        }
    }

    fn is_companion(state: &FileState, parent: i64) -> bool {
        matches!(state, FileState::ReferencedCompanion { parent_archive_id, .. } if *parent_archive_id == parent)
    }

    const CUE_ONE: &str =
        "FILE \"track01.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n";

    #[test]
    fn single_cue_and_single_bin_the_bin_is_a_companion() {
        let w = world();
        w.write("Game.cue", CUE_ONE.as_bytes());
        w.write("track01.bin", b"data");
        let parent = w.catalogue("Game.cue", "direct_game_image", None);
        let out = w.run();
        assert!(is_companion(&w.at("track01.bin", &out), parent));
        assert_eq!(out.report.counts.uncatalogued_files, 0);
        assert_eq!(out.report.counts.referenced_companions, 1);
    }

    #[test]
    fn multiple_tracks_relative_and_subdirectory_references_are_all_companions() {
        let w = world();
        w.write(
            "Game.cue",
            b"FILE \"track01.bin\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n\
              FILE \"./track02.bin\" BINARY\n TRACK 02 AUDIO\n  INDEX 01 00:00:00\n\
              FILE \"audio/track03.wav\" WAVE\n TRACK 03 AUDIO\n  INDEX 01 00:00:00\n",
        );
        for f in ["track01.bin", "track02.bin", "audio/track03.wav"] {
            w.write(f, b"x");
        }
        let parent = w.catalogue("Game.cue", "direct_game_image", None);
        let out = w.run();
        for f in ["track01.bin", "track02.bin", "audio/track03.wav"] {
            assert!(is_companion(&w.at(f, &out), parent), "{f}");
        }
        assert_eq!(out.report.counts.uncatalogued_files, 0);
    }

    #[test]
    fn an_unrelated_bin_beside_a_cue_stays_uncatalogued() {
        let w = world();
        w.write("Game.cue", CUE_ONE.as_bytes());
        w.write("track01.bin", b"data");
        w.write("orphan.bin", b"zzzz");
        w.catalogue("Game.cue", "direct_game_image", None);
        let out = w.run();
        assert_eq!(w.at("orphan.bin", &out), FileState::Uncatalogued);
        assert_eq!(out.report.counts.uncatalogued_files, 1);
    }

    #[test]
    fn a_missing_referenced_bin_does_not_hide_the_others() {
        let w = world();
        w.write(
            "Game.cue",
            b"FILE \"track01.bin\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n\
              FILE \"gone.bin\" BINARY\n TRACK 02 AUDIO\n  INDEX 01 00:00:00\n",
        );
        w.write("track01.bin", b"x");
        let parent = w.catalogue("Game.cue", "direct_game_image", None);
        let out = w.run();
        assert!(is_companion(&w.at("track01.bin", &out), parent));
    }

    #[test]
    fn references_that_escape_the_descriptor_directory_are_refused() {
        let w = world();
        w.write("outside.bin", b"secret");
        w.write(
            "sub/Game.cue",
            b"FILE \"../outside.bin\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n",
        );
        w.write(
            "sub/abs.cue",
            format!(
                "FILE \"{}\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n",
                w.root.join("outside.bin").display()
            )
            .as_bytes(),
        );
        w.catalogue("sub/Game.cue", "direct_game_image", None);
        w.catalogue("sub/abs.cue", "direct_game_image", None);
        let out = w.run();
        assert_eq!(w.at("outside.bin", &out), FileState::Uncatalogued);
    }

    #[test]
    fn a_malformed_cue_claims_nothing_and_does_not_fail_the_run() {
        let w = world();
        w.write("Bad.cue", b"FILE track01.bin BINARY\n");
        w.write("track01.bin", b"x");
        w.catalogue("Bad.cue", "direct_game_image", None);
        let out = w.run();
        assert_eq!(w.at("track01.bin", &out), FileState::Uncatalogued);
    }

    #[test]
    fn reference_case_follows_the_existing_resolver_not_a_guess() {
        let w = world();
        w.write(
            "Game.cue",
            b"FILE \"TRACK01.BIN\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n",
        );
        w.write("track01.bin", b"x");
        w.catalogue("Game.cue", "direct_game_image", None);
        let out = w.run();
        // On a case-sensitive filesystem the resolver cannot find TRACK01.BIN;
        // reconciliation must not paper over that with a case-insensitive match.
        assert_eq!(w.at("track01.bin", &out), FileState::Uncatalogued);
    }

    #[test]
    fn one_bin_referenced_by_two_catalogued_cues_is_contended_not_uncatalogued() {
        let w = world();
        w.write("A.cue", CUE_ONE.as_bytes());
        w.write("B.cue", CUE_ONE.as_bytes());
        w.write("track01.bin", b"x");
        let a = w.catalogue("A.cue", "direct_game_image", None);
        let b = w.catalogue("B.cue", "direct_game_image", None);
        let out = w.run();
        assert_eq!(
            w.at("track01.bin", &out),
            FileState::ContendedCompanion {
                parent_archive_ids: vec![a, b]
            }
        );
        assert_eq!(out.report.counts.contended_companions, 1);
        assert_eq!(out.report.counts.uncatalogued_files, 0);
    }

    #[test]
    fn a_stale_cue_row_does_not_claim_companions_of_a_relocated_cue() {
        let w = world();
        // The catalogued CUE is gone; a same-content CUE now lives elsewhere,
        // uncatalogued, beside its own bin.
        w.write("old/Game.cue", CUE_ONE.as_bytes());
        w.write("old/track01.bin", b"x");
        w.catalogue("old/Game.cue", "direct_game_image", None);
        fs::remove_dir_all(w.root.join("old")).unwrap();
        w.write("new/Game.cue", CUE_ONE.as_bytes());
        w.write("new/track01.bin", b"x");
        let out = w.run();
        assert_eq!(w.at("new/track01.bin", &out), FileState::Uncatalogued);
        assert_eq!(w.at("new/Game.cue", &out), FileState::Uncatalogued);
        assert_eq!(out.report.counts.referenced_companions, 0);
    }

    #[test]
    fn a_gdi_descriptors_tracks_are_companions() {
        let w = world();
        w.write(
            "Disc.gdi",
            b"3\n1 0 4 2352 track01.bin 0\n2 450 0 2352 track02.raw 0\n3 45000 4 2352 track03.bin 0\n",
        );
        for f in ["track01.bin", "track02.raw", "track03.bin"] {
            w.write(f, b"x");
        }
        let parent = w.catalogue("Disc.gdi", "direct_game_image", None);
        let out = w.run();
        for f in ["track01.bin", "track02.raw", "track03.bin"] {
            assert!(
                matches!(w.at(f, &out), FileState::ReferencedCompanion { parent_archive_id, basis: CompanionBasis::GdiTrackReference } if parent_archive_id == parent),
                "{f}: {:?}",
                w.at(f, &out)
            );
        }
    }

    /// The production-style probe: every property the foundation promises, in
    /// one temporary tree, with no write to any real database or game file.
    #[test]
    fn regression_probe_companions_orphans_strong_moves_and_no_folder_platform() {
        let w = world();
        w.write(
            "atari2600/Game.cue",
            CUE_ONE.replace("track01", "t1").as_bytes(),
        );
        w.write("atari2600/t1.bin", b"disc");
        w.write("atari2600/orphan.bin", b"orphan");
        let cue = w.catalogue("atari2600/Game.cue", "direct_game_image", None);
        // A primary item that was renamed on disk, with a persisted SHA-256.
        w.write("atari2600/Pitfall.xyz", b"pitfall");
        let sha = {
            use sha2::{Digest, Sha256};
            Sha256::digest(b"pitfall")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let stale = w.catalogue("atari2600/Pitfall.xyz", "direct_game_image", Some(&sha));
        fs::rename(
            w.root.join("atari2600/Pitfall.xyz"),
            w.root.join("atari2600/Pitfall (1982).xyz"),
        )
        .unwrap();
        let before = fs::read_dir(w.root.join("atari2600")).unwrap().count();

        let first = w.run();
        // A. companions are not uncatalogued.   B. the orphan is.
        assert!(is_companion(&w.at("atari2600/t1.bin", &first), cue));
        assert_eq!(
            w.at("atari2600/orphan.bin", &first),
            FileState::Uncatalogued
        );
        // Not hashed yet: the renamed file is only a possible move.
        assert_eq!(first.report.counts.strong_move_candidates, 0);
        let ask = first.report.hash_requests.clone();
        let hashed = hash_files(
            &w.config.source_folders,
            &ask,
            1 << 20,
            &AtomicBool::new(false),
        );
        let second = match reconcile_library(
            &w.db,
            &w.config.source_folders,
            &hashed.hashes,
            &AtomicBool::new(false),
            &default_mount_of,
        )
        .unwrap()
        {
            ReconciliationOutcome::Complete(done) => *done,
            ReconciliationOutcome::Cancelled => panic!(),
        };
        // C. the stale row is strongly paired with the renamed primary item.
        let moved = second
            .report
            .rows
            .iter()
            .find(|r| r.archive_id == stale)
            .unwrap();
        assert!(
            matches!(&moved.state, RowState::StrongMoveCandidate { to } if to.ends_with("Pitfall (1982).xyz"))
        );
        // ... and the companion is still not independent of its parent.
        assert!(is_companion(&w.at("atari2600/t1.bin", &second), cue));
        // D. no platform was invented anywhere.
        let platforms: i64 = rusqlite::Connection::open(w.db.path())
            .unwrap()
            .query_row("SELECT count(*) FROM platform_assignments", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(platforms, 0);
        assert!(second.report.rows.iter().all(|r| r.platform.is_none()));
        assert_eq!(
            fs::read_dir(w.root.join("atari2600")).unwrap().count(),
            before
        );
    }
}
