use super::super::ContinuityError;
use super::super::model::InUse;
use super::super::store::*;
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[test]
fn a_snapshot_is_verified_and_recovered_into_a_new_folder_without_touching_the_original() {
    let bench = Bench::new();
    let card = bench.file("Mcd001.ps2", &vec![7u8; 70_000]);
    let before = tree(&bench.dir.path().join("saves"));

    let outcome = bench
        .store
        .capture(&request("dev", &[&card], 100), &CLOSED)
        .unwrap();
    let CaptureOutcome::Created {
        manifest,
        new_bytes,
        ..
    } = outcome
    else {
        panic!("first snapshot must be new");
    };
    assert_eq!(new_bytes, 70_000);
    assert_eq!(
        manifest.body.boundary,
        CaptureBoundary::EmulatorClosedObserved
    );
    assert!(bench.store.verify(&manifest.snapshot_id).unwrap().intact());

    let destination = bench.dir.path().join("recovered");
    let plan = bench
        .store
        .plan_recovery(&manifest.snapshot_id, &destination)
        .unwrap();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(!destination.exists(), "a plan writes nothing");
    let receipt = bench
        .store
        .recover_to_new(&manifest.snapshot_id, &destination)
        .unwrap();
    assert_eq!(
        fs::read(destination.join("Mcd001.ps2")).unwrap(),
        vec![7u8; 70_000]
    );
    assert!(destination.join("EMUWIZ-RECOVERY.json").is_file());
    assert_eq!(receipt.files.len(), 1);
    assert_eq!(
        tree(&bench.dir.path().join("saves")),
        before,
        "the original is untouched"
    );
}

#[test]
fn identical_content_is_stored_once_and_an_unchanged_save_makes_no_new_snapshot() {
    let bench = Bench::new();
    let a = bench.file("a.srm", &[1u8; 4096]);
    let first = bench
        .store
        .capture(&request("dev", &[&a], 100), &CLOSED)
        .unwrap();
    let again = bench
        .store
        .capture(&request("dev", &[&a], 200), &CLOSED)
        .unwrap();
    assert!(
        matches!(again, CaptureOutcome::Unchanged { ref existing } if existing.snapshot_id == first.manifest().snapshot_id)
    );

    // The same bytes under another name and location: a new snapshot, no new content.
    fs::create_dir(bench.dir.path().join("other")).unwrap();
    let b = bench.dir.path().join("other/b.srm");
    fs::write(&b, [1u8; 4096]).unwrap();
    let CaptureOutcome::Created {
        new_bytes,
        reused_bytes,
        ..
    } = bench
        .store
        .capture(&request("dev", &[&b], 300), &CLOSED)
        .unwrap()
    else {
        panic!("a different location is a different history");
    };
    assert_eq!((new_bytes, reused_bytes), (0, 4096));
    let usage = bench.store.usage();
    assert_eq!(
        (
            usage.snapshots,
            usage.objects,
            usage.stored_bytes,
            usage.logical_bytes
        ),
        (2, 1, 4096, 8192)
    );
}

#[test]
fn history_links_each_snapshot_to_the_one_before() {
    let bench = Bench::new();
    let a = bench.file("a.srm", b"one");
    let first = bench
        .store
        .capture(&request("dev", &[&a], 100), &CLOSED)
        .unwrap();
    fs::write(&a, b"two").unwrap();
    let second = bench
        .store
        .capture(&request("dev", &[&a], 50), &CLOSED)
        .unwrap();
    assert_eq!(
        second.manifest().body.parent.as_deref(),
        Some(first.manifest().snapshot_id.as_str())
    );
    assert_eq!(
        second.manifest().body.lineage,
        first.manifest().body.lineage
    );
}

#[test]
fn a_damaged_stored_copy_is_reported_never_recovered_from_and_never_silently_replaced() {
    let bench = Bench::new();
    let a = bench.file("a.srm", &[9u8; 1000]);
    let manifest = bench
        .store
        .capture(&request("dev", &[&a], 100), &CLOSED)
        .unwrap()
        .manifest()
        .clone();
    let object = bench
        .store
        .root()
        .join("objects")
        .join(&manifest.body.files[0].sha256[..2])
        .join(&manifest.body.files[0].sha256);
    fs::set_permissions(&object, fs::Permissions::from_mode(0o600)).unwrap();
    let mut bytes = fs::read(&object).unwrap();
    bytes[500] ^= 0x01; // one flipped bit
    fs::write(&object, bytes).unwrap();

    let report = bench.store.verify(&manifest.snapshot_id).unwrap();
    assert!(!report.intact());
    assert!(matches!(report.files[0].1, FileCheck::Corrupt { .. }));
    let refused = bench
        .store
        .recover_to_new(&manifest.snapshot_id, &bench.dir.path().join("out"))
        .unwrap_err();
    assert!(matches!(refused, ContinuityError::Refused(_)), "{refused}");
    assert!(!bench.dir.path().join("out").exists());

    // Taking the snapshot again must not quietly paper over the damage.
    let again = bench
        .store
        .capture(&request("dev", &[&a], 200), &CLOSED)
        .unwrap_err();
    assert!(matches!(again, ContinuityError::Corrupt(_)), "{again}");
}

#[test]
fn a_missing_stored_copy_is_reported_as_missing() {
    let bench = Bench::new();
    let a = bench.file("a.srm", &[9u8; 10]);
    let manifest = bench
        .store
        .capture(&request("dev", &[&a], 100), &CLOSED)
        .unwrap()
        .manifest()
        .clone();
    let sha = &manifest.body.files[0].sha256;
    fs::remove_file(bench.store.root().join("objects").join(&sha[..2]).join(sha)).unwrap();
    assert_eq!(
        bench.store.verify(&manifest.snapshot_id).unwrap().files[0].1,
        FileCheck::Missing
    );
}

#[test]
fn a_crash_before_the_record_is_written_leaves_no_snapshot() {
    for step in ["before-link", "after-object", "before-manifest"] {
        let bench = Bench::new();
        let a = bench.file("a.srm", &[3u8; 2048]);
        CRASH_AT.with(|at| *at.borrow_mut() = Some(step));
        let crashed = bench.store.capture(&request("dev", &[&a], 100), &CLOSED);
        CRASH_AT.with(|at| *at.borrow_mut() = None);
        assert!(crashed.is_err(), "{step}");
        assert!(
            bench.store.list().is_empty(),
            "{step}: nothing half-made is visible"
        );
        // The next attempt starts clean and succeeds; leftovers are only reported.
        let ok = bench
            .store
            .capture(&request("dev", &[&a], 200), &CLOSED)
            .unwrap();
        assert!(
            bench
                .store
                .verify(&ok.manifest().snapshot_id)
                .unwrap()
                .intact(),
            "{step}"
        );
        assert!(bench.store.health().damaged_manifests.is_empty(), "{step}");
    }
}

#[test]
fn links_and_escaping_names_are_refused() {
    let bench = Bench::new();
    let real = bench.file("real.srm", b"data");
    let link = bench.dir.path().join("saves/link.srm");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let refused = bench
        .store
        .capture(&request("dev", &[&link], 1), &CLOSED)
        .unwrap_err();
    assert!(
        matches!(refused, ContinuityError::Refused(ref text) if text.contains("link")),
        "{refused}"
    );

    for name in ["../escape", "/etc/passwd", "a/../../b", "", "a\0b"] {
        let mut bad = request("dev", &[&real], 1);
        bad.files[0].name = name.to_string();
        assert!(
            matches!(
                bench.store.capture(&bad, &CLOSED),
                Err(ContinuityError::Refused(_))
            ),
            "{name:?}"
        );
    }
    assert!(bench.store.list().is_empty());

    // A directory or a FIFO is not a save file either.
    let fifo = bench.dir.path().join("saves/pipe");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        bench.store.capture(&request("dev", &[&fifo], 1), &CLOSED),
        Err(ContinuityError::Refused(_))
    ));
}

#[test]
fn a_shared_hard_link_is_copied_and_pointed_out() {
    let bench = Bench::new();
    let a = bench.file("a.srm", b"shared");
    fs::hard_link(&a, bench.dir.path().join("saves/second-name.srm")).unwrap();
    let manifest = bench
        .store
        .capture(&request("dev", &[&a], 1), &CLOSED)
        .unwrap()
        .manifest()
        .clone();
    assert_eq!(manifest.body.files[0].source_hard_links, 2);
    assert!(
        manifest
            .body
            .checks
            .iter()
            .any(|check| check.contains("hard link"))
    );
}

#[test]
fn a_running_emulator_or_open_save_is_always_refused() {
    let bench = Bench::new();
    let a = bench.file("a.srm", b"data");
    for state in [InUse::EmulatorRunning, InUse::OpenByAProgram] {
        let mut ask = request("dev", &[&a], 1);
        ask.allow_unproven_closure = true; // even when a best-effort copy was asked for
        let refused = bench.store.capture(&ask, &Fixed(state)).unwrap_err();
        assert!(matches!(refused, ContinuityError::InUse(_)), "{refused}");
    }
    assert!(bench.store.list().is_empty());
}

#[test]
fn unknown_is_refused_unless_asked_for_and_is_then_labelled_unverified_for_good() {
    let bench = Bench::new();
    let a = bench.file("a.srm", b"data");
    let unknown = Fixed(InUse::Unknown);
    let refused = bench
        .store
        .capture(&request("dev", &[&a], 1), &unknown)
        .unwrap_err();
    assert!(
        matches!(refused, ContinuityError::ClosureUnproven(_)),
        "{refused}"
    );

    let mut ask = request("dev", &[&a], 2);
    ask.allow_unproven_closure = true;
    let best_effort = bench
        .store
        .capture(&ask, &unknown)
        .unwrap()
        .manifest()
        .clone();
    assert_eq!(best_effort.body.boundary, CaptureBoundary::StableReadOnly);
    assert!(
        best_effort
            .body
            .checks
            .iter()
            .any(|check| check.starts_with("UNVERIFIED"))
    );

    // The same bytes taken later at a verified safe point are a separate
    // snapshot: the unverified copy never stands in for a verified one.
    let verified = bench
        .store
        .capture(&request("dev", &[&a], 3), &CLOSED)
        .unwrap();
    assert!(matches!(verified, CaptureOutcome::Created { .. }));
    assert_eq!(
        verified.manifest().body.boundary,
        CaptureBoundary::EmulatorClosedObserved
    );
    // ...and the reverse: a later unverified look does not get the verified label.
    let mut ask = request("dev", &[&a], 4);
    ask.allow_unproven_closure = true;
    let later = bench.store.capture(&ask, &unknown).unwrap();
    assert_eq!(
        later.manifest().body.boundary,
        CaptureBoundary::StableReadOnly
    );
}

/// Closed when the copy starts, a writer by the time it ends.
struct WriterArrives(AtomicUsize);
impl InUseObserver for WriterArrives {
    fn observe(&self, _: &str, _: &Path) -> InUse {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            InUse::NotDetected
        } else {
            InUse::OpenByAProgram
        }
    }
}

#[test]
fn a_writer_that_appears_during_the_copy_means_nothing_is_kept() {
    let bench = Bench::new();
    let a = bench.file("a.srm", b"data");
    let refused = bench
        .store
        .capture(
            &request("dev", &[&a], 1),
            &WriterArrives(AtomicUsize::new(0)),
        )
        .unwrap_err();
    assert!(
        matches!(refused, ContinuityError::InUse(ref text) if text.contains("during the copy")),
        "{refused}"
    );
    assert!(bench.store.list().is_empty());
    assert_eq!(bench.store.usage().objects, 0);
}

/// A writer rewrites the file in place, in pieces, over and over. Reading
/// the file twice is not enough on its own: a writer that pauses half-way
/// leaves a file that reads the same twice and is still torn (this test
/// caught exactly that when it trusted the double read alone). What makes a
/// snapshot "verified" is the kernel lease: it is only granted while no
/// program has the file open, and it is broken the moment one opens it to
/// write. So a verified snapshot here is always one whole version.
#[test]
fn a_file_being_rewritten_is_never_kept_torn_at_a_verified_safe_point() {
    use super::super::discover::ProcessObserver;
    let bench = Bench::new();
    let size = 2 * 1024 * 1024;
    let (all_a, all_b) = (vec![0xAAu8; size], vec![0xBBu8; size]);
    let path = bench.file("card.ps2", &all_a);
    let empty_proc = tempfile::tempdir().unwrap();
    let observer = ProcessObserver::with_proc_root(empty_proc.path().to_path_buf());
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (path, stop, a, b) = (path.clone(), stop.clone(), all_a.clone(), all_b.clone());
        std::thread::spawn(move || {
            use std::io::Write;
            let mut flip = false;
            while !stop.load(Ordering::Relaxed) {
                let Ok(mut file) = fs::OpenOptions::new().write(true).open(&path) else {
                    continue;
                };
                let source = if flip { &a } else { &b };
                for chunk in source.chunks(64 * 1024) {
                    file.write_all(chunk).unwrap();
                    std::thread::yield_now(); // pause mid-rewrite, as a busy emulator does
                }
                flip = !flip;
            }
        })
    };
    let (mut kept, mut refused) = (0, 0);
    for round in 0..60 {
        let mut ask = request("dev", &[&path], round);
        ask.provenance.emulator = "PCSX2".into();
        match bench.store.capture(&ask, &observer) {
            Ok(outcome) => {
                kept += 1;
                let manifest = outcome.manifest();
                assert_eq!(
                    manifest.body.boundary,
                    CaptureBoundary::EmulatorClosedObserved
                );
                let bytes = bench
                    .store
                    .read_object(&manifest.body.files[0].sha256)
                    .unwrap();
                assert!(
                    bytes == all_a || bytes == all_b,
                    "a torn copy was kept as verified"
                );
            }
            Err(ContinuityError::InUse(_) | ContinuityError::Unstable(_)) => refused += 1,
            Err(other) => panic!("{other}"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    assert!(refused > 0, "the writer was never noticed ({kept} kept)");
}

#[test]
fn the_size_limit_stops_a_snapshot_and_nothing_is_removed_to_make_room() {
    let dir = tempfile::tempdir().unwrap();
    let store = SnapshotStore::open_with_budget(&dir.path().join("store"), 3000).unwrap();
    let a = dir.path().join("a.srm");
    fs::write(&a, [1u8; 2000]).unwrap();
    store.capture(&request("dev", &[&a], 1), &CLOSED).unwrap();
    fs::write(&a, [2u8; 2000]).unwrap();
    let refused = store
        .capture(&request("dev", &[&a], 2), &CLOSED)
        .unwrap_err();
    assert!(
        matches!(refused, ContinuityError::NoSpace(ref text) if text.contains("Nothing is removed automatically")),
        "{refused}"
    );
    assert_eq!(store.list().len(), 1, "the earlier snapshot is still there");
    assert!(store.verify(&store.list()[0].snapshot_id).unwrap().intact());
}

#[test]
fn recovery_never_overwrites_and_an_interrupted_recovery_leaves_no_destination() {
    let bench = Bench::new();
    let a = bench.file("a.srm", b"progress");
    let id = bench
        .store
        .capture(&request("dev", &[&a], 1), &CLOSED)
        .unwrap()
        .manifest()
        .snapshot_id
        .clone();

    let existing = bench.dir.path().join("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("a.srm"), b"do not lose me").unwrap();
    for destination in [
        existing.clone(),
        bench.dir.path().join("saves"),
        bench.dir.path().join("saves/inside"),
        bench.store.root().join("x"),
        PathBuf::from("relative"),
    ] {
        let refused = bench.store.recover_to_new(&id, &destination).unwrap_err();
        assert!(
            matches!(refused, ContinuityError::Refused(_)),
            "{}: {refused}",
            destination.display()
        );
    }
    assert_eq!(fs::read(existing.join("a.srm")).unwrap(), b"do not lose me");
    assert_eq!(fs::read(&a).unwrap(), b"progress");

    let destination = bench.dir.path().join("out");
    CRASH_AT.with(|at| *at.borrow_mut() = Some("recover-after-file"));
    let crashed = bench.store.recover_to_new(&id, &destination);
    CRASH_AT.with(|at| *at.borrow_mut() = None);
    assert!(crashed.is_err());
    assert!(
        !destination.exists(),
        "a half-recovered folder must not look finished"
    );
    // The same destination can be used once the recovery is run again.
    bench.store.recover_to_new(&id, &destination).unwrap();
    assert_eq!(fs::read(destination.join("a.srm")).unwrap(), b"progress");
}

#[test]
fn an_edited_snapshot_record_is_not_believed() {
    let bench = Bench::new();
    let a = bench.file("a.srm", b"data");
    let manifest = bench
        .store
        .capture(&request("dev", &[&a], 1), &CLOSED)
        .unwrap()
        .manifest()
        .clone();
    let record = bench
        .store
        .root()
        .join("manifests")
        .join(&manifest.body.lineage);
    let path = fs::read_dir(&record)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("emulator_closed_observed", "imported");
    fs::write(&path, text).unwrap();
    assert!(
        bench.store.list().is_empty(),
        "the altered record is not listed"
    );
    assert_eq!(bench.store.health().damaged_manifests, vec![path]);
    assert!(matches!(
        bench.store.get(&manifest.snapshot_id),
        Err(ContinuityError::NotFound(_))
    ));
}

#[test]
fn a_record_cannot_name_content_outside_the_store() {
    let bench = Bench::new();
    let a = bench.file("a.srm", b"data");
    let good = bench
        .store
        .capture(&request("dev", &[&a], 1), &CLOSED)
        .unwrap()
        .manifest()
        .clone();
    for (name, sha) in [
        ("../../outside", good.body.files[0].sha256.clone()),
        ("ok", "../../../etc/passwd".to_string()),
    ] {
        let mut body = good.body.clone();
        body.captured_unix = 99;
        body.files[0].name = name.to_string();
        body.files[0].sha256 = sha;
        let forged = SnapshotManifest {
            format_version: 1,
            snapshot_id: body_id(&body),
            body,
        };
        assert!(bench.store.put_manifest(&forged).is_err(), "{name}");
    }
    assert_eq!(bench.store.list().len(), 1);
}

#[test]
fn pruning_is_a_proposal_that_counts_shared_content_once_and_deletes_nothing() {
    let bench = Bench::new();
    let a = bench.file("a.srm", &[0u8; 1000]);
    for (time, fill) in [(1, 1u8), (2, 2), (3, 1), (4, 4)] {
        fs::write(&a, [fill; 1000]).unwrap();
        bench
            .store
            .capture(&request("dev", &[&a], time), &CLOSED)
            .unwrap();
    }
    let before = tree(bench.store.root());
    let proposal = bench.store.pruning_proposal(2);
    // Snapshots 1 and 2 are candidates. Snapshot 1's content is still used
    // by snapshot 3 (kept), so only snapshot 2's bytes would be freed.
    assert_eq!(proposal.candidates.len(), 2);
    assert_eq!(proposal.would_free_bytes, 1000);
    assert!(proposal.nothing_was_deleted);
    assert_eq!(tree(bench.store.root()), before);
    assert_eq!(
        bench.store.pruning_proposal(0).candidates.len(),
        3,
        "at least one is always kept"
    );
}

#[test]
fn snapshot_ids_are_repeatable() {
    let ids: Vec<String> = (0..2)
        .map(|_| {
            let dir = tempfile::tempdir().unwrap();
            // Same path both times, so the same provenance.
            let root = PathBuf::from("/nonexistent-save-root");
            let store = SnapshotStore::open(&dir.path().join("store")).unwrap();
            let file = dir.path().join("a.srm");
            fs::write(&file, b"same").unwrap();
            fs::File::open(&file)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1000))
                .unwrap();
            let mut ask = request("dev", &[&file], 5);
            ask.provenance.source_root = root;
            store
                .capture(&ask, &CLOSED)
                .unwrap()
                .manifest()
                .snapshot_id
                .clone()
        })
        .collect();
    assert_eq!(ids[0], ids[1]);
}
