//! Safety regressions for the ten historical `known_gap_*` scenarios in
//! `data_integrity_invariants.rs`.
//!
//! The historical file is kept verbatim: its `known_gap_*` tests assert the
//! *defective* behaviour, so a pass there means the defect reproduces. Every
//! test here asserts the required safe behaviour for the same scenario and
//! must fail on an implementation that still has the defect.
//!
//! All data is synthetic and lives in per-test temporary directories.
#![cfg(target_os = "linux")]

archivefs_core::install_test_environment!();

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use archivefs_core::dat::rename_apply::*;
use archivefs_core::identity_source::hashing::{LocalHashCache, hash_file};
use archivefs_core::playing_library::*;
use archivefs_core::safe_read::TrustedRoots;

fn stamp(path: &Path, time: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(time))
        .unwrap();
}

fn transaction(source: &Path, destination: &Path) -> RenameTransaction {
    serde_json::from_value(serde_json::json!({
        "transaction_id": "integrity-test", "plan_generation": 7,
        "classifier_version": archivefs_core::dat::classification::CLASSIFIER_VERSION,
        "created_at_unix": 1, "source_scan_root": source.parent().unwrap(),
        "entries": [{
            "source_path": source, "destination_path": destination,
            "original_basename": source.file_name().unwrap().to_str().unwrap(),
            "proposed_basename": destination.file_name().unwrap().to_str().unwrap(),
            "identity": capture_identity(source).unwrap()
        }]
    }))
    .unwrap()
}

fn apply_with(
    tx: &mut RenameTransaction,
    root: &Path,
    trusted: TrustedRoots,
) -> Result<ApplyOutcome, ApplyError> {
    apply_transaction(&mut ApplyExecution {
        approved_paths: tx
            .entries
            .iter()
            .map(|e| e.source_path.to_string_lossy().into_owned())
            .collect(),
        current_generation: 7,
        trusted,
        journal_dir: root.join("journal"),
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &AtomicBool::new(false),
        directory_policy: DirectoryPolicy::SameFilesystem,
        allow_symlink_source: false,
        transaction: tx,
    })
}

fn apply(tx: &mut RenameTransaction, root: &Path) -> Result<ApplyOutcome, ApplyError> {
    apply_with(tx, root, TrustedRoots::from_paths([root]))
}

fn fixture() -> (tempfile::TempDir, RenameTransaction) {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("journal")).unwrap();
    let source = temp.path().join("original.bin");
    fs::write(&source, b"original").unwrap();
    stamp(&source, UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    let tx = transaction(&source, &temp.path().join("renamed.bin"));
    (temp, tx)
}

fn journal_on_disk(root: &Path, tx: &RenameTransaction) -> RenameTransaction {
    read_journal(&journal_path(&root.join("journal"), &tx.transaction_id).unwrap()).unwrap()
}

fn link_transaction(source: &Path, destination: &Path, root: &Path) -> RenameTransaction {
    let mut tx = transaction(source, destination);
    tx.entries[0].operation = TransactionOperation::CreateSymlink {
        expected_target: source.to_path_buf(),
        destination_root: root.to_path_buf(),
    };
    tx
}

// Scenario 1 (historical: known_gap_base_apply_replays_a_rolled_back_transaction)

#[test]
fn a_rolled_back_transaction_is_never_reapplied_by_the_shared_executor() {
    let (temp, mut tx) = fixture();
    apply(&mut tx, temp.path()).unwrap();
    let undo = rollback_transaction(
        &mut tx,
        &temp.path().join("journal"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(undo.result, RollbackResult::FullyRolledBack);
    assert_eq!(tx.state, TransactionState::RolledBack);

    let replay = apply(&mut tx, temp.path());
    assert!(
        matches!(replay, Err(ApplyError::AlreadySettled { .. })),
        "a rolled-back transaction must be refused, got {replay:?}"
    );
    assert_eq!(fs::read(&tx.entries[0].source_path).unwrap(), b"original");
    assert!(!tx.entries[0].destination_path.exists());
    assert_eq!(tx.state, TransactionState::RolledBack);
    assert_eq!(
        journal_on_disk(temp.path(), &tx).state,
        TransactionState::RolledBack,
        "the refused replay must not rewrite the settled journal"
    );
}

#[test]
fn rollback_in_progress_and_failed_rollback_transactions_are_refused_too() {
    for state in [
        TransactionState::RollingBack,
        TransactionState::RollbackFailed,
    ] {
        let (temp, mut tx) = fixture();
        tx.state = state;
        let result = apply(&mut tx, temp.path());
        assert!(
            matches!(result, Err(ApplyError::AlreadySettled { .. })),
            "{state:?} must be refused, got {result:?}"
        );
        assert_eq!(fs::read(&tx.entries[0].source_path).unwrap(), b"original");
        assert!(!tx.entries[0].destination_path.exists());
    }
}

// Scenario 2 (historical: known_gap_retry_of_failed_rollback_reports_success_without_restoring)

#[test]
fn retrying_a_failed_rollback_never_reports_success_without_restoring() {
    let (temp, mut tx) = fixture();
    apply(&mut tx, temp.path()).unwrap();
    fs::write(&tx.entries[0].source_path, b"collision").unwrap();
    let journal = temp.path().join("journal");
    let first = rollback_transaction(&mut tx, &journal, &AtomicBool::new(false)).unwrap();
    assert!(matches!(
        first.result,
        RollbackResult::RollbackFailed { .. }
    ));

    let retry = rollback_transaction(&mut tx, &journal, &AtomicBool::new(false)).unwrap();
    assert!(
        matches!(retry.result, RollbackResult::RollbackFailed { .. }),
        "a retry that restored nothing must not report success, got {:?}",
        retry.result
    );
    assert_eq!(tx.state, TransactionState::RollbackFailed);
    assert_eq!(tx.entries[0].state, EntryState::RollbackFailed);
    assert_eq!(
        journal_on_disk(temp.path(), &tx).state,
        TransactionState::RollbackFailed
    );
    assert_eq!(
        fs::read(&tx.entries[0].destination_path).unwrap(),
        b"original"
    );
    assert_eq!(fs::read(&tx.entries[0].source_path).unwrap(), b"collision");
}

// Scenario 3 (historical: known_gap_concurrent_same_transaction_id_loses_one_operation_from_history)

#[test]
#[ignore = "blocked: shared journal publication is owned by the unpromoted R2 journal-ownership \
            work (fix/r2-journal-ownership-20261013, ac0c9d2d); run with --ignored for evidence"]
fn concurrent_applies_sharing_a_transaction_id_never_lose_a_mutation_from_history() {
    let (temp, original) = fixture();
    let root = temp.path();
    let a = link_transaction(&original.entries[0].source_path, &root.join("a.bin"), root);
    let b = link_transaction(&original.entries[0].source_path, &root.join("b.bin"), root);
    let barrier = Arc::new(Barrier::new(2));
    let (first, second) = std::thread::scope(|scope| {
        let run = |mut tx: RenameTransaction, barrier: Arc<Barrier>| {
            barrier.wait();
            apply(&mut tx, root).map(|outcome| outcome.transaction.state)
        };
        let guard = Arc::clone(&barrier);
        let first = scope.spawn(move || run(a, guard));
        let second = scope.spawn(move || run(b, barrier));
        (first.join().unwrap(), second.join().unwrap())
    });
    assert!(
        first.is_ok() || second.is_ok(),
        "at least one apply must succeed"
    );

    let (journals, problems) = list_journals(&root.join("journal"));
    assert!(problems.is_empty());
    let recorded: Vec<PathBuf> = journals
        .iter()
        .flat_map(|tx| tx.entries.iter())
        .filter(|entry| entry.state == EntryState::Applied)
        .map(|entry| entry.destination_path.clone())
        .collect();
    for link in [root.join("a.bin"), root.join("b.bin")] {
        if link.is_symlink() {
            assert!(
                recorded.contains(&link),
                "{} exists on disk but no journal records it, so Undo can never remove it",
                link.display()
            );
        }
    }
}

// Scenario 4 (historical: known_gap_duplicate_link_retry_adopts_and_removes_a_preexisting_link)

#[test]
fn undo_never_removes_a_link_the_transaction_did_not_create() {
    let (temp, original) = fixture();
    let source = &original.entries[0].source_path;
    let destination = temp.path().join("preexisting.bin");
    std::os::unix::fs::symlink(source, &destination).unwrap();
    let mut tx = link_transaction(source, &destination, temp.path());
    // Adopting an already-correct link is the accepted idempotent-reapply
    // contract (playing_library::apply_adapter); a repeat on the same
    // transaction may be accepted or refused.
    apply(&mut tx, temp.path()).unwrap();
    let _ = apply(&mut tx, temp.path());
    rollback_transaction(
        &mut tx,
        &temp.path().join("journal"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        destination.is_symlink(),
        "rollback removed a link this transaction never created"
    );
    assert_eq!(fs::read_link(&destination).unwrap(), *source);
    assert_eq!(fs::read(source).unwrap(), b"original");
}

#[test]
fn undo_still_removes_a_link_the_transaction_created() {
    let (temp, original) = fixture();
    let source = &original.entries[0].source_path;
    let destination = temp.path().join("created.bin");
    let mut tx = link_transaction(source, &destination, temp.path());
    apply(&mut tx, temp.path()).unwrap();
    assert!(destination.is_symlink());
    let undo = rollback_transaction(
        &mut tx,
        &temp.path().join("journal"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(undo.result, RollbackResult::FullyRolledBack);
    assert!(!destination.is_symlink() && !destination.exists());
    assert_eq!(fs::read(source).unwrap(), b"original");
}

// Scenario 5 (historical: known_gap_hash_cache_accepts_same_size_replacement_with_same_second)

#[test]
fn the_hash_cache_never_returns_a_digest_for_a_same_size_same_second_replacement() {
    let (temp, tx) = fixture();
    let source = &tx.entries[0].source_path;
    let time = fs::metadata(source).unwrap().modified().unwrap();
    let trusted = TrustedRoots::from_paths([temp.path()]);
    let old = hash_file(source, &trusted, None).unwrap();
    let mut cache = LocalHashCache::new();
    cache.insert(old.clone());
    assert_eq!(cache.get(source).map(|h| &h.sha1), Some(&old.sha1));

    fs::rename(source, temp.path().join("old-inode")).unwrap();
    fs::write(source, b"replaced").unwrap();
    stamp(source, time);
    let current = hash_file(source, &trusted, None).unwrap();
    assert_ne!(current.sha1, old.sha1);
    assert!(
        cache
            .get(source)
            .is_none_or(|cached| cached.sha1 == current.sha1),
        "the cache returned the replaced object's digest for the new object"
    );
}

fn in_place_rewrite_with_restored_mtime(gap: Duration) -> bool {
    let (temp, tx) = fixture();
    let source = &tx.entries[0].source_path;
    let time = fs::metadata(source).unwrap().modified().unwrap();
    let trusted = TrustedRoots::from_paths([temp.path()]);
    let old = hash_file(source, &trusted, None).unwrap();
    let mut cache = LocalHashCache::new();
    cache.insert(old);

    std::thread::sleep(gap);
    fs::write(source, b"replaced").unwrap();
    stamp(source, time);
    let current = hash_file(source, &trusted, None).unwrap();
    cache
        .get(source)
        .is_none_or(|cached| cached.sha1 == current.sha1)
}

#[test]
fn the_hash_cache_never_returns_a_digest_for_an_in_place_same_size_rewrite() {
    // The rewrite lands in a later kernel timestamp tick than the hash, so its
    // change time differs even though size and mtime were restored.
    assert!(
        in_place_rewrite_with_restored_mtime(Duration::from_millis(50)),
        "the cache returned a stale digest after an in-place rewrite"
    );
}

#[test]
#[ignore = "residual limit, not historical scenario 5: a same-inode, same-size rewrite within \
            one kernel timestamp tick (~4 ms here) with a restored mtime leaves every metadata \
            field unchanged; only re-reading content can detect it. Timing-dependent: it fails \
            only when hash and rewrite share a tick, so a pass is not evidence of safety"]
fn residual_same_tick_in_place_rewrite_is_not_detectable_from_metadata() {
    assert!(
        in_place_rewrite_with_restored_mtime(Duration::ZERO),
        "the cache returned a stale digest after a same-tick in-place rewrite"
    );
}

#[test]
fn a_legacy_cache_record_without_an_object_stamp_is_never_trusted() {
    let (temp, tx) = fixture();
    let source = &tx.entries[0].source_path;
    let trusted = TrustedRoots::from_paths([temp.path()]);
    let mut legacy = serde_json::to_value(hash_file(source, &trusted, None).unwrap()).unwrap();
    let fingerprint = legacy["fingerprint"].as_object_mut().unwrap();
    fingerprint.retain(|key, _| {
        matches!(
            key.as_str(),
            "path" | "size_bytes" | "modified_unix_seconds"
        )
    });
    let legacy = serde_json::from_value(legacy).unwrap();
    let mut cache = LocalHashCache::new();
    cache.insert(legacy);
    assert!(cache.get(source).is_none());
}

// Scenario 6 (historical: known_gap_hashing_returns_old_digest_bound_to_a_replaced_path)

#[test]
fn hashing_never_binds_a_digest_to_a_path_replaced_while_reading() {
    use archivefs_core::identity_source::hashing::hash_file_reporting;
    let (temp, tx) = fixture();
    let source = &tx.entries[0].source_path;
    let trusted = TrustedRoots::from_paths([temp.path()]);
    let time = fs::metadata(source).unwrap().modified().unwrap();
    let swapped = std::cell::Cell::new(false);
    let result = hash_file_reporting(source, &trusted, None, &|progress| {
        if progress.bytes_read > 0 && !swapped.replace(true) {
            fs::rename(source, temp.path().join("retained-hashed-object")).unwrap();
            fs::write(source, b"replaced").unwrap();
            stamp(source, time);
        }
    });
    assert!(swapped.get(), "the swap must happen mid-read");
    let current = hash_file(source, &trusted, None).unwrap();
    match result {
        Err(_) => {}
        Ok(hashes) => assert!(
            hashes.sha1 == current.sha1 && hashes.fingerprint.still_current(),
            "returned the replaced object's digest as current evidence for the path"
        ),
    }
}

#[test]
fn hashing_an_unchanged_file_still_succeeds_and_is_cacheable() {
    let (temp, tx) = fixture();
    let source = &tx.entries[0].source_path;
    let trusted = TrustedRoots::from_paths([temp.path()]);
    let hashes = hash_file(source, &trusted, None).unwrap();
    assert!(hashes.fingerprint.still_current());
    let mut cache = LocalHashCache::new();
    cache.insert(hashes.clone());
    assert_eq!(cache.get(source), Some(&hashes));
}

// Scenarios 7 and 8 (Playing Library)

fn playing_plan(temp: &Path, source: &Path) -> (PlayingLibraryPlan, PathBuf) {
    let hashes = hash_file(source, &TrustedRoots::from_paths([temp]), None).unwrap();
    let dat_path = temp.join("catalogue.dat");
    fs::write(&dat_path, format!("<datafile><header><name>Fixture</name><version>1</version></header><game name=\"Verified Game\"><rom name=\"game.bin\" size=\"8\" sha1=\"{}\"/></game></datafile>", hashes.sha1)).unwrap();
    let parsed = archivefs_core::dat::parsers::parse_dat_file(
        &dat_path,
        archivefs_core::dat::limits::DatLimits::default(),
    )
    .unwrap();
    let matches = match_loose_files_against_dat(
        &parsed.dat,
        &[source.to_path_buf()],
        &TrustedRoots::from_paths([temp]),
        &AtomicBool::new(false),
    );
    assert_eq!(matches.matches.len(), 1);
    let plan = build_playing_library_plan(&PlayingLibraryRequest {
        dat: &parsed.dat,
        matches: matches.matches,
        destination_root: temp.join("playing"),
        policy: PlayingLibraryPolicy::default(),
    })
    .unwrap();
    fs::create_dir(&plan.destination_root).unwrap();
    (plan, dat_path)
}

/// Builds and applies the plan's transaction; returns whether a link was
/// published, and asserts nothing was published unless it succeeded.
fn publish_playing_plan(temp: &Path, plan: &PlayingLibraryPlan) -> bool {
    let destination = plan.operations[0].destination_path.clone();
    let Ok(mut tx) = build_playing_library_transaction(plan, 7) else {
        assert!(!destination.is_symlink());
        return false;
    };
    match apply(&mut tx, temp) {
        Ok(outcome) if outcome.transaction.state == TransactionState::Applied => true,
        _ => {
            assert!(!destination.is_symlink());
            false
        }
    }
}

#[test]
#[ignore = "confirmed, not repaired: the election carries no object identity to bind to; repair \
            needs an election-evidence contract across playing_library, publisher_profile, \
            es_de_publish and GUI fixtures; run with --ignored for evidence"]
fn playing_library_never_publishes_an_object_replaced_after_election() {
    let (temp, original) = fixture();
    let source = &original.entries[0].source_path;
    let (plan, _) = playing_plan(temp.path(), source);
    fs::rename(source, temp.path().join("old-object")).unwrap();
    fs::write(source, b"different game entirely").unwrap();
    assert!(
        !publish_playing_plan(temp.path(), &plan),
        "a link to an unverified replacement was published"
    );
}

#[test]
#[ignore = "inconclusive: no stated contract binds a Playing Library plan to a DAT revision \
            (the plan holds no DAT identity); needs an owner contract decision; run with \
            --ignored for evidence"]
fn playing_library_refuses_a_plan_whose_dat_revision_changed() {
    let (temp, original) = fixture();
    let (plan, dat_path) = playing_plan(temp.path(), &original.entries[0].source_path);
    fs::write(
        dat_path,
        "<datafile><header><version>2</version></header></datafile>",
    )
    .unwrap();
    assert!(
        !publish_playing_plan(temp.path(), &plan),
        "a plan elected from a superseded DAT revision was published"
    );
}

// Scenario 9 (historical: known_gap_empty_trusted_roots_allow_core_media_mutation)

#[test]
fn an_empty_trusted_root_set_refuses_every_mutation() {
    let (temp, mut tx) = fixture();
    let result = apply_with(&mut tx, temp.path(), TrustedRoots::none());
    assert!(
        result.is_err(),
        "empty trusted roots must refuse, got {result:?}"
    );
    assert_eq!(fs::read(&tx.entries[0].source_path).unwrap(), b"original");
    assert!(!tx.entries[0].destination_path.exists());
}

#[test]
fn trusted_roots_that_all_fail_to_resolve_refuse_every_mutation() {
    // `from_paths` silently drops unresolvable roots, so a misconfigured or
    // unmounted library degrades to an empty set; that must still refuse.
    let (temp, mut tx) = fixture();
    let trusted = TrustedRoots::from_paths([temp.path().join("unmounted-library")]);
    assert!(trusted.is_empty());
    let result = apply_with(&mut tx, temp.path(), trusted);
    assert!(
        result.is_err(),
        "unresolvable roots must refuse, got {result:?}"
    );
    assert_eq!(fs::read(&tx.entries[0].source_path).unwrap(), b"original");
    assert!(!tx.entries[0].destination_path.exists());
}

// Scenario 10 (historical: known_gap_library_view_overwrites_a_destination_created_after_planning)

fn library_view_fixture(
    temp: &Path,
    source: &Path,
    action: archivefs_core::LibraryViewPlanAction,
    manifest_entries: Vec<archivefs_core::LibraryViewManifestEntry>,
) -> (
    archivefs_core::LibraryViewConfig,
    archivefs_core::LibraryViewPlan,
    archivefs_core::LibraryViewManifest,
    PathBuf,
) {
    use archivefs_core::*;
    let destination_root = temp.join("view");
    fs::create_dir_all(&destination_root).unwrap();
    let destination = destination_root.join("Game.bin");
    let view = LibraryViewConfig {
        id: "audit-view".into(),
        name: "Audit".into(),
        destination_root: destination_root.clone(),
        enabled: true,
        source_folders: vec![temp.into()],
        platforms: vec![],
        layout_template: LibraryViewLayoutTemplate::PlatformFilename,
        profile: FrontendProfile::default(),
    };
    let manifest = LibraryViewManifest {
        view_id: view.id.clone(),
        destination_root: destination_root.clone(),
        entries: manifest_entries,
        view_fingerprint: None,
        profile_version: 0,
        created_directories: vec![],
    };
    let counts = match action {
        LibraryViewPlanAction::Repair => LibraryViewPlanCounts {
            repair: 1,
            ..Default::default()
        },
        _ => LibraryViewPlanCounts {
            create: 1,
            ..Default::default()
        },
    };
    let plan = LibraryViewPlan {
        view_id: view.id.clone(),
        destination_root,
        counts,
        entries: vec![LibraryViewPlanEntry {
            action,
            archive_path: Some(source.to_path_buf()),
            relative_link_path: Some("Game.bin".into()),
            destination_path: Some(destination.clone()),
            platform: Some("NES".into()),
            reason: None,
            colliding_with: None,
            source_folder_path: Some(temp.into()),
            archive_identity: Some("8:1700000000".into()),
        }],
        unsafe_root_error: None,
        profile_fingerprint: compute_view_profile_fingerprint(&view),
        fingerprint_conflict: None,
        profile_error: None,
    };
    (view, plan, manifest, destination)
}

#[test]
fn library_view_create_never_overwrites_a_destination_created_after_planning() {
    let (temp, original) = fixture();
    let source = original.entries[0].source_path.clone();
    let (view, plan, manifest, destination) = library_view_fixture(
        temp.path(),
        &source,
        archivefs_core::LibraryViewPlanAction::Create,
        vec![],
    );
    // The reviewed destination was absent; user data arrives while the plan
    // is open, before apply starts.
    fs::write(&destination, b"irreplaceable destination data").unwrap();
    let report = archivefs_core::apply_library_view(
        &view,
        &plan,
        &manifest,
        &temp.path().join("view-history"),
    )
    .unwrap();
    assert_eq!(report.created, 0);
    assert_eq!(report.failed, 1);
    assert!(!destination.is_symlink());
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"irreplaceable destination data",
        "previous destination contents were lost"
    );
    assert_eq!(fs::read(&source).unwrap(), b"original");
}

fn managed_entry(target: &Path, temp: &Path) -> archivefs_core::LibraryViewManifestEntry {
    archivefs_core::LibraryViewManifestEntry {
        relative_link_path: "Game.bin".into(),
        target_path: target.to_path_buf(),
        archive_identity: Some("8:1700000000".into()),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        platform: "NES".into(),
        source_folder_path: temp.into(),
        object_kind: archivefs_core::LibraryViewObjectKind::Symlink,
        content_hash: None,
        rendering_version: None,
    }
}

#[test]
fn library_view_repair_never_overwrites_a_real_file_that_replaced_the_managed_link() {
    let (temp, original) = fixture();
    let source = original.entries[0].source_path.clone();
    let stale = temp.path().join("stale-target.bin");
    let (view, plan, manifest, destination) = library_view_fixture(
        temp.path(),
        &source,
        archivefs_core::LibraryViewPlanAction::Repair,
        vec![managed_entry(&stale, temp.path())],
    );
    // Planned as Repair over a managed symlink; the user replaced the link
    // with real data before apply.
    fs::write(&destination, b"irreplaceable destination data").unwrap();
    let report = archivefs_core::apply_library_view(
        &view,
        &plan,
        &manifest,
        &temp.path().join("view-history"),
    )
    .unwrap();
    assert_eq!(report.repaired, 0);
    assert_eq!(report.failed, 1);
    assert!(!destination.is_symlink());
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"irreplaceable destination data"
    );
    assert!(
        fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")),
        "no temporary link may be left behind"
    );
}

#[test]
fn library_view_repair_still_replaces_a_managed_link_that_points_elsewhere() {
    let (temp, original) = fixture();
    let source = original.entries[0].source_path.clone();
    let stale = temp.path().join("stale-target.bin");
    let (view, plan, manifest, destination) = library_view_fixture(
        temp.path(),
        &source,
        archivefs_core::LibraryViewPlanAction::Repair,
        vec![managed_entry(&stale, temp.path())],
    );
    std::os::unix::fs::symlink(&stale, &destination).unwrap();
    let report = archivefs_core::apply_library_view(
        &view,
        &plan,
        &manifest,
        &temp.path().join("view-history"),
    )
    .unwrap();
    assert_eq!(report.repaired, 1, "{:?}", report.results);
    assert_eq!(report.failed, 0);
    assert_eq!(fs::read_link(&destination).unwrap(), source);
}

#[test]
fn library_view_create_still_creates_into_an_absent_destination() {
    let (temp, original) = fixture();
    let source = original.entries[0].source_path.clone();
    let (view, plan, manifest, destination) = library_view_fixture(
        temp.path(),
        &source,
        archivefs_core::LibraryViewPlanAction::Create,
        vec![],
    );
    let report = archivefs_core::apply_library_view(
        &view,
        &plan,
        &manifest,
        &temp.path().join("view-history"),
    )
    .unwrap();
    assert_eq!(report.created, 1, "{:?}", report.results);
    assert_eq!(fs::read_link(&destination).unwrap(), source);
}

// Independent review (cee57b84) follow-ups.

fn multi_transaction(root: &Path, names: &[(&str, &str)]) -> RenameTransaction {
    let entries: Vec<_> = names
        .iter()
        .map(|(source, destination)| {
            let source = root.join(source);
            serde_json::json!({
                "source_path": source, "destination_path": root.join(destination),
                "original_basename": source.file_name().unwrap().to_str().unwrap(),
                "proposed_basename": destination,
                "identity": capture_identity(&source).unwrap()
            })
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "transaction_id": "integrity-resume", "plan_generation": 7,
        "classifier_version": archivefs_core::dat::classification::CLASSIFIER_VERSION,
        "created_at_unix": 1, "source_scan_root": root,
        "entries": entries
    }))
    .unwrap()
}

#[test]
fn exact_resume_never_replays_entries_reversed_by_a_failed_rollback() {
    use archivefs_core::dat::rename_apply::model::{ExactResumeEnvelope, ExactResumeOperation};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("roms");
    let journal = temp.path().join("journal");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&journal).unwrap();
    for name in ["a.bin", "b.bin", "c.bin"] {
        fs::write(root.join(name), name.as_bytes()).unwrap();
    }
    let mut tx = multi_transaction(
        &root,
        &[("a.bin", "A.bin"), ("b.bin", "B.bin"), ("c.bin", "C.bin")],
    );
    let envelope = ExactResumeEnvelope {
        format_version: exact_resume::EXACT_RESUME_FORMAT_VERSION,
        transaction_id: tx.transaction_id.clone(),
        approved_generation: 7,
        classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.into(),
        plan_digest: "digest".into(),
        source_scan_root: root.to_string_lossy().into_owned(),
        approved_source_paths: tx
            .entries
            .iter()
            .map(|e| e.source_path.to_string_lossy().into_owned())
            .collect(),
        operations: tx
            .entries
            .iter()
            .enumerate()
            .map(|(index, e)| ExactResumeOperation {
                index,
                source_path: e.source_path.clone(),
                destination_path: e.destination_path.clone(),
                operation: e.operation.clone(),
                identity: e.identity.clone(),
                original_basename: e.original_basename.clone(),
                proposed_basename: e.proposed_basename.clone(),
            })
            .collect(),
        created_at_unix: 1,
    };
    tx.unknown.insert(
        "emuwiz_exact_resume_envelope".into(),
        serde_json::to_value(envelope).unwrap(),
    );
    // An exact apply that failed at entry 2 after applying entries 0 and 1.
    tx.unknown.insert(
        "emuwiz_exact_resume_state".into(),
        serde_json::json!("failed"),
    );
    for i in 0..2 {
        fs::rename(&tx.entries[i].source_path, &tx.entries[i].destination_path).unwrap();
        tx.entries[i].state = EntryState::Applied;
    }
    tx.entries[2].state = EntryState::ApplyFailed;
    tx.state = TransactionState::ApplyFailed;
    write_journal(&journal, &tx).unwrap();

    // Undo reverses entry 1, then fails on entry 0 (its source name is taken).
    fs::write(&tx.entries[0].source_path, b"blocker").unwrap();
    rollback_transaction(&mut tx, &journal, &AtomicBool::new(false)).unwrap();
    assert_eq!(tx.state, TransactionState::RollbackFailed);
    assert_eq!(tx.entries[1].state, EntryState::RolledBack);
    fs::remove_file(&tx.entries[0].source_path).unwrap();

    assert!(
        !matches!(
            inspect_exact_resume(&tx, 7, "digest"),
            ExactResumeInspection::Available { .. }
        ),
        "resume must not be offered for a rollback-state transaction"
    );
    let resumed = resume_exact_transaction(&mut ExactResumeExecution {
        transaction: &mut tx,
        current_generation: 7,
        current_plan_digest: "digest".into(),
        trusted: TrustedRoots::from_paths([&root]),
        journal_dir: journal.clone(),
        cancel: &AtomicBool::new(false),
    });
    assert!(resumed.is_err(), "resume must refuse, got {resumed:?}");
    assert!(
        root.join("b.bin").exists() && !root.join("B.bin").exists(),
        "the entry reversed by Undo was replayed"
    );
}

#[test]
fn an_interrupted_link_entry_is_recognised_as_its_own_link_and_undo_removes_it() {
    let (temp, original) = fixture();
    let root = temp.path();
    let source = &original.entries[0].source_path;
    let destination = root.join("link.bin");
    let mut tx = link_transaction(source, &destination, root);
    apply(&mut tx, root).unwrap();
    // A crash between symlink(2) and the Applied checkpoint.
    tx.entries[0].state = EntryState::Applying;
    tx.entries[0].applied_at_unix = None;
    tx.state = TransactionState::Applying;
    write_journal(&root.join("journal"), &tx).unwrap();

    let again = apply(&mut tx, root).unwrap();
    assert_eq!(again.transaction.entries[0].state, EntryState::Applied);
    let undo =
        rollback_transaction(&mut tx, &root.join("journal"), &AtomicBool::new(false)).unwrap();
    assert_eq!(undo.result, RollbackResult::FullyRolledBack);
    assert!(
        !destination.is_symlink(),
        "the link this transaction created was left behind"
    );
}

#[test]
fn an_interrupted_link_entry_with_no_link_yet_is_created() {
    let (temp, original) = fixture();
    let root = temp.path();
    let source = &original.entries[0].source_path;
    let destination = root.join("link.bin");
    let mut tx = link_transaction(source, &destination, root);
    apply(&mut tx, root).unwrap();
    // A crash after the Applying checkpoint but before symlink(2).
    fs::remove_file(&destination).unwrap();
    tx.entries[0].state = EntryState::Applying;
    tx.state = TransactionState::Applying;
    write_journal(&root.join("journal"), &tx).unwrap();

    assert_eq!(
        apply(&mut tx, root).unwrap().transaction.entries[0].state,
        EntryState::Applied
    );
    assert_eq!(fs::read_link(&destination).unwrap(), *source);
}

#[test]
fn library_view_never_removes_an_existing_file_at_a_temporary_link_name() {
    let (temp, original) = fixture();
    let source = original.entries[0].source_path.clone();
    let (view, plan, manifest, destination) = library_view_fixture(
        temp.path(),
        &source,
        archivefs_core::LibraryViewPlanAction::Create,
        vec![],
    );
    // Names a previous interrupted Repair could have left holding user data.
    let parent = destination.parent().unwrap();
    let preserved: Vec<PathBuf> = (0..32)
        .map(|seq| {
            parent.join(format!(
                ".archivefs-link-{:x}-{seq:x}.tmp",
                std::process::id()
            ))
        })
        .collect();
    for path in &preserved {
        fs::write(path, b"preserved user data").unwrap();
    }
    let report = archivefs_core::apply_library_view(
        &view,
        &plan,
        &manifest,
        &temp.path().join("view-history"),
    )
    .unwrap();
    for path in &preserved {
        assert_eq!(fs::read(path).unwrap(), b"preserved user data");
    }
    assert_eq!(report.created, 1, "{:?}", report.results);
    assert_eq!(fs::read_link(&destination).unwrap(), source);
}
