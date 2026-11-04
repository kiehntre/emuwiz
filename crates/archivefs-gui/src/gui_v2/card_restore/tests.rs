//! End-to-end proof of snapshot -> one folder -> PSU -> guarded restore ->
//! Undo. Every card, snapshot, PSU, backup and journal is a disposable
//! synthetic file in a temporary directory; no real save is read or written.

use super::*;
use crate::pcsx2_page::ps2_inventory_fixture;
use archivefs_core::{
    memory_card_inventory::restore_guard::{
        Ps2RecoveryOutcome, Ps2RestorePhase, load_ps2_restore_journal,
        recover_all_interrupted_ps2_restores,
    },
    persistent_state_inventory::{PortabilityClass, StatePathOrigin},
    save_snapshots::tree_restore::safety::{DirectorySaveBinding, EmulatorQuiescence},
};

struct Observed(EmulatorQuiescence);
impl QuiescenceProvider for Observed {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        self.0
    }
}
const CLOSED: Observed = Observed(EmulatorQuiescence::Closed);
const RUNNING: Observed = Observed(EmulatorQuiescence::Running);
const UNKNOWN: Observed = Observed(EmulatorQuiescence::Unknown);

/// Offset of the synthetic save's only file (`icon.sys`, 100 bytes).
const ICON_DATA: std::ops::Range<usize> = (41 + 3) * 1024..(41 + 3) * 1024 + 100;
/// The card's raw 32-byte, zero-padded directory name.
fn raw(name: &[u8]) -> Vec<u8> {
    let mut raw = name.to_vec();
    raw.resize(32, 0);
    raw
}
fn folder() -> Vec<u8> {
    raw(b"BASLUS-00001TEST")
}

pub(super) fn card_record(path: &Path) -> PersistentStateRecord {
    PersistentStateRecord {
        sram_origin: None,
        emulator: StateEmulator::Pcsx2,
        selected_installation: None,
        state_type: PersistentStateType::MemoryCard,
        game_identity: Vec::new(),
        path: path.to_path_buf(),
        container_path: None,
        slot_profile_account: Some("default".into()),
        emulator_version: None,
        firmware_context: None,
        portability_class: PortabilityClass::CopyWithMetadata,
        source_path_origin: StatePathOrigin::Configured,
        provenance: "fixture".into(),
        sha256: None,
        size_bytes: 0,
        warnings: Vec::new(),
    }
}

struct World {
    _dir: tempfile::TempDir,
    card: PathBuf,
    location: SaveLocation,
    snapshots: PathBuf,
    dirs: WorkDirs,
    options: Ps2RestoreGuardOptions,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("Mcd001.ps2");
        fs::write(&card, ps2_inventory_fixture()).unwrap();
        let record = card_record(&card);
        let location = card_location(&record, std::slice::from_ref(&record)).unwrap();
        let journals = dir.path().join("journals");
        fs::create_dir(&journals).unwrap();
        Self {
            card,
            location,
            snapshots: dir.path().join("snapshots"),
            dirs: WorkDirs {
                psu_dir: dir.path().join("work/psu"),
                backup_dir: dir.path().join("work/backups"),
            },
            options: Ps2RestoreGuardOptions {
                journal_dir: journals,
                unix_seconds: 1_800_000_000,
            },
            _dir: dir,
        }
    }

    fn snapshot(&self) -> SaveSnapshot {
        create_card_snapshot(&self.snapshots, &self.location, &CLOSED).unwrap();
        let history = load_card_history(&self.snapshots, &self.location, &CLOSED);
        assert_eq!(history.snapshots.len(), 1);
        assert!(history.snapshots[0].integrity.is_ok());
        history.snapshots[0].snapshot.clone()
    }

    /// Simulate playing after the snapshot: the save's bytes change.
    fn play(&self) -> Vec<u8> {
        let mut bytes = fs::read(&self.card).unwrap();
        bytes[ICON_DATA].fill(0x11);
        fs::write(&self.card, &bytes).unwrap();
        bytes
    }

    fn card_bytes(&self) -> Vec<u8> {
        fs::read(&self.card).unwrap()
    }

    fn files_in(dir: &Path) -> usize {
        fs::read_dir(dir).map(|e| e.count()).unwrap_or(0)
    }
}

fn snapshot_card_bytes(snapshot: &SaveSnapshot) -> Vec<u8> {
    fs::read(
        snapshot
            .storage_path
            .join("files")
            .join(&snapshot.manifest.artifacts[0].relative_path),
    )
    .unwrap()
}

#[test]
fn snapshot_folder_restores_through_guarded_core_path_and_undo_returns_the_card() {
    let world = World::new();
    let snapshot = world.snapshot();
    assert_eq!(
        snapshot.manifest.game_identity, None,
        "never tied to a game"
    );
    let snapshot_bytes = snapshot_card_bytes(&snapshot);
    let played = world.play();

    // Inspect: the folders the snapshot really contains.
    let folders = snapshot_folders(&snapshot, &world.location).unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].raw_name, folder());
    assert_eq!(folders[0].files, vec![("icon.sys".to_string(), 100)]);
    assert_eq!(folders[0].problem, None);

    // Preview: exports one folder to a PSU, reads the live card only.
    let preview = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
    assert_eq!(preview.folder, "BASLUS-00001TEST");
    assert_eq!(preview.plan.source_card_path, world.card);
    assert!(preview.plan.existing_save && preview.plan.replace_existing);
    assert_eq!(world.card_bytes(), played, "preview never writes the card");
    assert!(!preview.plan.backup_path.exists());
    assert_eq!(World::files_in(&world.options.journal_dir), 0);

    // Apply through the unchanged guarded Core restore.
    let done = apply_restore(&preview.plan, &CLOSED, &world.options).unwrap();
    let restored = world.card_bytes();
    assert_ne!(restored, played);
    assert_eq!(&restored[ICON_DATA], &snapshot_bytes[ICON_DATA]);
    assert_eq!(
        fs::read(&done.result.backup_path).unwrap(),
        played,
        "verified backup of the pre-restore card"
    );
    assert_eq!(
        load_ps2_restore_journal(&done.journal_path).unwrap().phase,
        Ps2RestorePhase::Published
    );
    assert_eq!(
        snapshot_card_bytes(&snapshot),
        snapshot_bytes,
        "snapshot only read"
    );
    // Only the chosen folder changed: the card is not the snapshot image.
    assert_eq!(restored.len(), snapshot_bytes.len());

    // Undo returns the exact pre-restore card.
    undo_restore(&done.journal_path, &CLOSED, 2).unwrap();
    assert_eq!(world.card_bytes(), played);
    assert_eq!(
        load_ps2_restore_journal(&done.journal_path).unwrap().phase,
        Ps2RestorePhase::Undone
    );
}

#[test]
fn cancelling_a_preview_changes_nothing_and_removes_its_exported_folder() {
    let world = World::new();
    let snapshot = world.snapshot();
    let played = world.play();
    let preview = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
    assert!(preview.plan.source_psu_path.exists());
    discard_preview(&preview, &world.dirs);
    assert!(!preview.plan.source_psu_path.exists());
    assert_eq!(world.card_bytes(), played);
    assert_eq!(World::files_in(&world.dirs.backup_dir), 0);
    assert_eq!(World::files_in(&world.options.journal_dir), 0);
}

#[test]
fn closure_must_be_proven_before_the_card_is_written() {
    let world = World::new();
    let snapshot = world.snapshot();
    let played = world.play();
    for observer in [RUNNING, UNKNOWN] {
        let preview =
            prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
        assert!(apply_restore(&preview.plan, &observer, &world.options).is_err());
        assert_eq!(world.card_bytes(), played);
        assert!(!preview.plan.backup_path.exists());
        discard_preview(&preview, &world.dirs);
    }
    // Snapshot creation also needs proven closure.
    assert!(create_card_snapshot(&world.snapshots, &world.location, &RUNNING).is_err());
    assert!(create_card_snapshot(&world.snapshots, &world.location, &UNKNOWN).is_err());
}

#[test]
fn changed_snapshot_or_live_card_evidence_is_refused() {
    // Tampered snapshot: integrity, inspection and preview all refuse.
    let world = World::new();
    let snapshot = world.snapshot();
    let card_file = snapshot
        .storage_path
        .join("files")
        .join(&snapshot.manifest.artifacts[0].relative_path);
    let mut tampered = fs::read(&card_file).unwrap();
    tampered[ICON_DATA.start] ^= 0xff;
    fs::write(&card_file, &tampered).unwrap();
    let before = world.card_bytes();
    assert!(
        load_card_history(&world.snapshots, &world.location, &CLOSED).snapshots[0]
            .integrity
            .is_err()
    );
    assert!(snapshot_folders(&snapshot, &world.location).is_err());
    assert!(prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).is_err());
    assert_eq!(World::files_in(&world.dirs.psu_dir), 0);
    assert_eq!(world.card_bytes(), before);

    // Live card changed between preview and confirm: Core refuses.
    let world = World::new();
    let snapshot = world.snapshot();
    world.play();
    let preview = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
    let mut changed = world.card_bytes();
    changed[ICON_DATA.start] = 0x77;
    fs::write(&world.card, &changed).unwrap();
    assert!(apply_restore(&preview.plan, &CLOSED, &world.options).is_err());
    assert_eq!(world.card_bytes(), changed);

    // Card changed after restore: Undo refuses and keeps the newer card.
    let world = World::new();
    let snapshot = world.snapshot();
    world.play();
    let preview = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
    let done = apply_restore(&preview.plan, &CLOSED, &world.options).unwrap();
    let mut after = world.card_bytes();
    after[ICON_DATA.start] = 0x42;
    fs::write(&world.card, &after).unwrap();
    assert!(undo_restore(&done.journal_path, &CLOSED, 2).is_err());
    assert_eq!(world.card_bytes(), after);

    // A folder name that is not in the snapshot is refused.
    let world = World::new();
    let snapshot = world.snapshot();
    assert!(
        prepare_restore(
            &snapshot,
            &world.location,
            &raw(b"BASLUS-99999NONE"),
            &world.dirs,
            1
        )
        .is_err()
    );
}

#[test]
fn wrong_or_ambiguous_destinations_are_refused() {
    let world = World::new();
    let snapshot = world.snapshot();
    // Another card path, or the same card under another profile.
    let other_card = world.card.with_file_name("Mcd002.ps2");
    fs::write(&other_card, ps2_inventory_fixture()).unwrap();
    let other_record = card_record(&other_card);
    let other = card_location(&other_record, std::slice::from_ref(&other_record)).unwrap();
    let mut other_profile = world.location.clone();
    other_profile.profile = Some("second".into());
    for location in [&other, &other_profile] {
        assert!(
            load_card_history(&world.snapshots, location, &CLOSED)
                .snapshots
                .is_empty()
        );
        assert!(snapshot_folders(&snapshot, location).is_err());
        assert!(prepare_restore(&snapshot, location, &folder(), &world.dirs, 1).is_err());
    }
    assert_eq!(fs::read(&other_card).unwrap(), ps2_inventory_fixture());

    // Missing live card.
    fs::remove_file(&world.card).unwrap();
    assert!(prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).is_err());
    assert!(!world.card.exists());

    // The same card listed twice, other emulators, savestates, card entries
    // and cards with warnings are never offered.
    let record = card_record(&world.card);
    assert!(card_location(&record, &[record.clone(), record.clone()]).is_err());
    let mut duck = record.clone();
    duck.emulator = StateEmulator::DuckStation;
    let mut state = record.clone();
    state.state_type = PersistentStateType::SaveState;
    let mut entry = record.clone();
    entry.container_path = Some(world.card.clone());
    let mut warned = record.clone();
    warned.warnings.push("needs review".into());
    for record in [duck, state, entry, warned] {
        assert!(card_location(&record, std::slice::from_ref(&record)).is_err());
    }
}

#[test]
fn an_interrupted_restore_blocks_new_restores_until_the_save_vault_recovers_it() {
    let world = World::new();
    let snapshot = world.snapshot();
    world.play();
    let first = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
    let done = apply_restore(&first.plan, &CLOSED, &world.options).unwrap();

    // Rewrite the real journal, in Core's checksummed format, as if the
    // process died while publishing.
    let mut journal = load_ps2_restore_journal(&done.journal_path).unwrap();
    journal.phase = Ps2RestorePhase::Publishing;
    let body = serde_json::to_vec_pretty(&journal).unwrap();
    let mut bytes = format!(
        "EMUWIZ-PS2-RESTORE-JOURNAL v1 sha256={}\n",
        sha256_hex(&body)
    )
    .into_bytes();
    bytes.extend_from_slice(&body);
    fs::write(&done.journal_path, bytes).unwrap();
    assert!(
        load_ps2_restore_journal(&done.journal_path)
            .unwrap()
            .phase
            .needs_recovery()
    );

    let card = world.card_bytes();
    let second = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 3).unwrap();
    let refused = apply_restore(&second.plan, &CLOSED, &world.options).unwrap_err();
    assert!(refused.contains("interrupted"), "{refused}");
    assert_eq!(world.card_bytes(), card);
    assert!(!second.plan.backup_path.exists());

    // Gold's existing recovery judges it; then restores are allowed again.
    let run = recover_all_interrupted_ps2_restores(&world.options.journal_dir, 4);
    assert!(matches!(
        run.outcomes.as_slice(),
        [(_, Ok(Ps2RecoveryOutcome::ConfirmedPublished))]
    ));
    let third = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 5).unwrap();
    apply_restore(&third.plan, &CLOSED, &world.options).unwrap();

    // An unreadable restore record also blocks writes.
    fs::write(
        world
            .options
            .journal_dir
            .join("ps2-psu-restore-garbage.json"),
        b"x",
    )
    .unwrap();
    let fourth = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 6).unwrap();
    let card = world.card_bytes();
    assert!(apply_restore(&fourth.plan, &CLOSED, &world.options).is_err());
    assert_eq!(world.card_bytes(), card);
}

#[test]
fn savestate_and_game_tied_snapshots_are_not_card_restore_sources() {
    let world = World::new();
    // A game-page snapshot of the same card carries a game identity and is
    // not listed or accepted here.
    create_snapshot(&SaveSnapshotRequest {
        location: world.location.clone(),
        game_identity: Some("Ps2Serial:SLUS-20001".into()),
        platform: None,
        storage_root: world.snapshots.clone(),
        available_space_bytes: None,
        snapshot_id: Some("game-tied".into()),
        now_unix_seconds: Some(1),
        emulator_use: EmulatorUseStatus::NotDetected,
    })
    .unwrap();
    let listing = list_snapshots(&world.snapshots);
    assert_eq!(listing.snapshots.len(), 1);
    assert!(
        load_card_history(&world.snapshots, &world.location, &CLOSED)
            .snapshots
            .is_empty()
    );
    assert!(snapshot_folders(&listing.snapshots[0], &world.location).is_err());
    assert!(SAVESTATE_RESTORE_UNSUPPORTED.contains("cannot be restored"));
}

#[test]
fn an_interrupted_restore_recorded_under_another_path_still_blocks() {
    let world = World::new();
    let snapshot = world.snapshot();
    world.play();
    let first = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
    let done = apply_restore(&first.plan, &CLOSED, &world.options).unwrap();
    // The record names the card by another spelling of its path.
    let mut journal = load_ps2_restore_journal(&done.journal_path).unwrap();
    journal.phase = Ps2RestorePhase::Publishing;
    journal.card_path = world.card.with_file_name("alias-Mcd001.ps2");
    let body = serde_json::to_vec_pretty(&journal).unwrap();
    let mut bytes = format!(
        "EMUWIZ-PS2-RESTORE-JOURNAL v1 sha256={}\n",
        sha256_hex(&body)
    )
    .into_bytes();
    bytes.extend_from_slice(&body);
    fs::write(&done.journal_path, bytes).unwrap();
    let card = world.card_bytes();
    let second = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 2).unwrap();
    assert!(apply_restore(&second.plan, &CLOSED, &world.options).is_err());
    assert_eq!(world.card_bytes(), card);
}

#[test]
fn preview_reads_one_private_verified_copy_and_leaves_only_the_psu() {
    let world = World::new();
    let snapshot = world.snapshot();
    let preview = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
    let left: Vec<_> = fs::read_dir(&world.dirs.psu_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(left, vec![preview.plan.source_psu_path.clone()]);
    // A snapshot whose bytes no longer match the manifest leaves nothing.
    discard_preview(&preview, &world.dirs);
    let image = snapshot
        .storage_path
        .join("files")
        .join(&snapshot.manifest.artifacts[0].relative_path);
    let mut bytes = fs::read(&image).unwrap();
    bytes[ICON_DATA.start] ^= 0x01;
    fs::write(&image, bytes).unwrap();
    assert!(prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 2).is_err());
    assert_eq!(World::files_in(&world.dirs.psu_dir), 0);
}
