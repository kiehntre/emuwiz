use super::*;
use crate::save_snapshots::{
    SaveArtifactType, SaveProvenance, SaveSnapshotRequest, create_snapshot,
};
use std::time::{Duration, SystemTime};
use tempfile::tempdir;

fn location(path: &Path) -> SaveLocation {
    SaveLocation {
        path: path.to_path_buf(),
        emulator: Some("DuckStation".into()),
        profile: Some("profile-a".into()),
        artifact_type: SaveArtifactType::PsMemoryCard,
        provenance: SaveProvenance::ConfiguredPath,
    }
}

fn snapshot(source: &Path, root: &Path, id: &str, at: u64) -> SaveSnapshot {
    create_snapshot(&SaveSnapshotRequest {
        location: location(source),
        game_identity: Some("ps1_serial:SLUS-00001".into()),
        platform: Some("PlayStation".into()),
        storage_root: root.to_path_buf(),
        available_space_bytes: None,
        snapshot_id: Some(id.into()),
        now_unix_seconds: Some(at),
        emulator_use: EmulatorUseStatus::NotDetected,
    })
    .unwrap()
}

fn set_mtime(path: &Path, seconds_ahead: u64) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(seconds_ahead))
        .unwrap();
}

#[test]
fn missing_root_is_an_empty_history_and_partial_staging_is_never_listed() {
    let dir = tempdir().unwrap();
    assert_eq!(
        list_snapshots(&dir.path().join("absent")),
        SnapshotListing::default()
    );
    let game = dir.path().join("root/game");
    fs::create_dir_all(game.join(".snap.partial-1")).unwrap();
    fs::write(game.join(".snap.partial-1/manifest.json"), b"{}").unwrap();
    let listing = list_snapshots(&dir.path().join("root"));
    assert!(listing.snapshots.is_empty());
    assert!(listing.warnings.is_empty());
}

#[test]
fn history_is_bound_to_exact_path_and_game_identity_newest_first() {
    let dir = tempdir().unwrap();
    let card = dir.path().join("cards/shared.mcd");
    fs::create_dir_all(card.parent().unwrap()).unwrap();
    fs::write(&card, b"card-v1").unwrap();
    let root = dir.path().join("snapshots");
    snapshot(&card, &root, "older", 10);
    snapshot(&card, &root, "newer", 20);
    // Same filename elsewhere: never associated by name.
    let other = dir.path().join("elsewhere/shared.mcd");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, b"card-v1").unwrap();
    snapshot(&other, &root, "other-path", 30);

    let listing = list_snapshots(&root);
    assert_eq!(listing.snapshots.len(), 3);
    let history = history_for_location(&listing, &location(&card), "ps1_serial:SLUS-00001");
    let ids: Vec<_> = history
        .iter()
        .map(|entry| entry.snapshot.manifest.snapshot_id.as_str())
        .collect();
    assert_eq!(ids, ["newer", "older"]);
    assert!(history.iter().all(SnapshotHistoryEntry::compatible));
    assert!(
        history
            .iter()
            .all(|entry| entry.freshness == SnapshotFreshness::MatchesCurrentSave)
    );
    assert!(history_for_location(&listing, &location(&card), "ps1_serial:SLUS-99999").is_empty());
}

#[test]
fn stale_snapshot_reports_newer_live_save_and_missing_live_save() {
    let dir = tempdir().unwrap();
    let card = dir.path().join("card.mcd");
    fs::write(&card, b"card-v1").unwrap();
    let root = dir.path().join("snapshots");
    snapshot(&card, &root, "s1", 10);
    fs::write(&card, b"card-v2-longer").unwrap();
    set_mtime(&card, 120);
    let listing = list_snapshots(&root);
    let entry = &history_for_location(&listing, &location(&card), "ps1_serial:SLUS-00001")[0];
    assert_eq!(entry.freshness, SnapshotFreshness::CurrentSaveNewer);
    assert!(entry.compatible(), "staleness is not an incompatibility");

    fs::remove_file(&card).unwrap();
    let entry = &history_for_location(&listing, &location(&card), "ps1_serial:SLUS-00001")[0];
    assert_eq!(entry.freshness, SnapshotFreshness::CurrentSaveMissing);
}

#[test]
fn incompatible_profile_and_tampered_snapshot_are_flagged() {
    let dir = tempdir().unwrap();
    let card = dir.path().join("card.mcd");
    fs::write(&card, b"card-v1").unwrap();
    let root = dir.path().join("snapshots");
    let made = snapshot(&card, &root, "s1", 10);
    let listing = list_snapshots(&root);
    let mut other_profile = location(&card);
    other_profile.profile = Some("profile-b".into());
    let entry = &history_for_location(&listing, &other_profile, "ps1_serial:SLUS-00001")[0];
    assert_eq!(
        entry.compatibility_conflicts,
        [SaveRestoreConflict::ProfileMismatch]
    );
    assert!(!entry.compatible());

    fs::write(made.storage_path.join("files/card.mcd"), b"tampered").unwrap();
    let entry = &history_for_location(&listing, &location(&card), "ps1_serial:SLUS-00001")[0];
    assert!(matches!(entry.integrity, SnapshotIntegrity::Failed(_)));
    assert!(!entry.compatible());
    assert_eq!(fs::read(&card).unwrap(), b"card-v1", "live save untouched");
}

#[test]
fn malformed_manifest_is_a_warning_not_a_snapshot() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("game/broken");
    fs::create_dir_all(&snap).unwrap();
    fs::write(snap.join("manifest.json"), b"not json").unwrap();
    let listing = list_snapshots(dir.path());
    assert!(listing.snapshots.is_empty());
    assert_eq!(listing.warnings.len(), 1);
}

struct Observed(EmulatorQuiescence);
impl QuiescenceProvider for Observed {
    fn observe(
        &self,
        _: &crate::save_snapshots::tree_restore::safety::DirectorySaveBinding,
    ) -> EmulatorQuiescence {
        self.0
    }
}

#[test]
fn only_reviewed_closed_evidence_allows_snapshot_creation() {
    let card = Path::new("/synthetic/Mcd001.ps2");
    for (observed, expected) in [
        (EmulatorQuiescence::Closed, EmulatorUseStatus::NotDetected),
        (EmulatorQuiescence::Running, EmulatorUseStatus::Running),
        (EmulatorQuiescence::Unknown, EmulatorUseStatus::Unknown),
    ] {
        assert_eq!(
            snapshot_emulator_use(StateEmulator::Pcsx2, card, &Observed(observed)),
            expected
        );
    }
    // No reviewed closure observer: even a "closed" observation is ignored.
    for emulator in [
        StateEmulator::DuckStation,
        StateEmulator::Dolphin,
        StateEmulator::Unknown,
    ] {
        assert!(!snapshot_closure_supported(emulator));
        assert_eq!(
            snapshot_emulator_use(emulator, card, &Observed(EmulatorQuiescence::Closed)),
            EmulatorUseStatus::Unknown
        );
    }
    // RetroArch now has a reviewed observer (see single_file_guard): only a
    // proved Closed clears it.
    let save = Path::new("/synthetic/game.srm");
    assert!(snapshot_closure_supported(StateEmulator::RetroArch));
    for (observed, expected) in [
        (EmulatorQuiescence::Closed, EmulatorUseStatus::NotDetected),
        (EmulatorQuiescence::Running, EmulatorUseStatus::Running),
        (EmulatorQuiescence::Unknown, EmulatorUseStatus::Unknown),
    ] {
        assert_eq!(
            snapshot_emulator_use(StateEmulator::RetroArch, save, &Observed(observed)),
            expected
        );
    }
}

fn fake_proc(dir: &Path, processes: &[(&str, &str, &[u8])]) -> PathBuf {
    let proc_root = dir.join("proc");
    fs::create_dir_all(&proc_root).unwrap();
    for (pid, comm, cmdline) in processes {
        let base = proc_root.join(pid);
        fs::create_dir_all(base.join("fd")).unwrap();
        fs::write(base.join("comm"), format!("{comm}\n")).unwrap();
        fs::write(base.join("cmdline"), cmdline).unwrap();
        fs::write(base.join("status"), "State:\tS (sleeping)\n").unwrap();
    }
    proc_root
}

#[test]
fn reviewed_proc_scan_drives_pcsx2_closure_through_a_synthetic_proc() {
    let dir = tempdir().unwrap();
    let card = dir.path().join("Mcd001.ps2");
    fs::write(&card, b"card").unwrap();
    let closed = ProcScanQuiescence::with_root(fake_proc(
        &dir.path().join("a"),
        &[("10", "bash", b"/bin/bash\0")],
    ));
    assert_eq!(
        snapshot_emulator_use(StateEmulator::Pcsx2, &card, &closed),
        EmulatorUseStatus::NotDetected
    );
    let running = ProcScanQuiescence::with_root(fake_proc(
        &dir.path().join("b"),
        &[("11", "pcsx2-qt", b"/usr/bin/pcsx2-qt\0")],
    ));
    assert_eq!(
        snapshot_emulator_use(StateEmulator::Pcsx2, &card, &running),
        EmulatorUseStatus::Running
    );
    // Ambiguous: a live process whose identity cannot be read.
    let ambiguous_root = fake_proc(&dir.path().join("c"), &[("12", "bash", b"/bin/bash\0")]);
    fs::remove_file(ambiguous_root.join("12/comm")).unwrap();
    fs::create_dir(ambiguous_root.join("12/comm")).unwrap();
    assert_eq!(
        snapshot_emulator_use(
            StateEmulator::Pcsx2,
            &card,
            &ProcScanQuiescence::with_root(ambiguous_root)
        ),
        EmulatorUseStatus::Unknown
    );
    // Missing /proc listing is unknown, never closed.
    assert_eq!(
        snapshot_emulator_use(
            StateEmulator::Pcsx2,
            &card,
            &ProcScanQuiescence::with_root(dir.path().join("no-proc"))
        ),
        EmulatorUseStatus::Unknown
    );
}

// ------------------------------------------- PCSX2 card snapshot under lease

fn card_request(card: &Path, root: &Path) -> SaveSnapshotRequest {
    SaveSnapshotRequest {
        location: location(card),
        game_identity: None,
        platform: Some("PS2".into()),
        storage_root: root.to_path_buf(),
        available_space_bytes: None,
        snapshot_id: Some("leased".into()),
        now_unix_seconds: Some(1),
        emulator_use: EmulatorUseStatus::NotDetected, // replaced by the check
    }
}

fn kept_snapshots(root: &Path) -> usize {
    fs::read_dir(root.join("unknown-game"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name() == "leased")
                .count()
        })
        .unwrap_or(0)
}

/// Closed, but starts a program opening the card for writing while the
/// snapshot's lease is held, and returns once the kernel shows the break.
struct WriterDuringCopy(std::cell::RefCell<Option<std::thread::JoinHandle<()>>>);
impl QuiescenceProvider for WriterDuringCopy {
    fn observe(
        &self,
        _: &crate::save_snapshots::tree_restore::safety::DirectorySaveBinding,
    ) -> EmulatorQuiescence {
        EmulatorQuiescence::Closed
    }
    fn observe_under_card_lease(
        &self,
        binding: &crate::save_snapshots::tree_restore::safety::DirectorySaveBinding,
        _: &crate::memory_card_inventory::restore_guard::CardWriteLease,
    ) -> EmulatorQuiescence {
        use std::os::unix::fs::MetadataExt;
        let card = PathBuf::from(&binding.profile);
        let needle = format!(":{} ", fs::metadata(&card).unwrap().ino());
        *self.0.borrow_mut() = Some(std::thread::spawn(move || {
            drop(fs::OpenOptions::new().write(true).open(card).unwrap());
        }));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !fs::read_to_string("/proc/locks")
            .unwrap_or_default()
            .lines()
            .any(|line| line.contains("BREAKING") && line.contains(&needle))
        {
            assert!(std::time::Instant::now() < deadline, "lease never broke");
            std::thread::sleep(Duration::from_millis(5));
        }
        EmulatorQuiescence::Closed
    }
}

#[test]
fn a_card_snapshot_is_kept_only_if_no_writer_opened_the_card_during_the_copy() {
    let dir = tempdir().unwrap();
    let card = dir.path().join("Mcd001.ps2");
    fs::write(&card, vec![0x5a; 64 * 1024]).unwrap();
    let root = dir.path().join("snapshots");

    // A writer arriving during the copy: whatever the timestamps say, the
    // copy is discarded.
    let observer = WriterDuringCopy(std::cell::RefCell::new(None));
    let result =
        create_snapshot_observed(StateEmulator::Pcsx2, card_request(&card, &root), &observer);
    observer.0.take().unwrap().join().unwrap();
    assert!(
        matches!(result, Err(SaveSnapshotError::CardInUse)),
        "{result:?}"
    );
    assert_eq!(kept_snapshots(&root), 0, "no torn snapshot is kept");

    // Another descriptor on the card, writer or reader, refuses up front.
    for write in [true, false] {
        let holder = fs::OpenOptions::new()
            .read(true)
            .write(write)
            .open(&card)
            .unwrap();
        let result = create_snapshot_observed(
            StateEmulator::Pcsx2,
            card_request(&card, &root),
            &Observed(EmulatorQuiescence::Closed),
        );
        assert!(
            matches!(result, Err(SaveSnapshotError::CardInUse)),
            "{result:?}"
        );
        drop(holder);
    }
    assert_eq!(kept_snapshots(&root), 0);

    // Undisturbed: kept.
    create_snapshot_observed(
        StateEmulator::Pcsx2,
        card_request(&card, &root),
        &Observed(EmulatorQuiescence::Closed),
    )
    .unwrap();
    assert_eq!(kept_snapshots(&root), 1);
}
