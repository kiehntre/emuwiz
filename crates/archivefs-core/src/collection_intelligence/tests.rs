//! Synthetic libraries in a temporary folder. Nothing here touches a real
//! library, and every test asserts on what a person would be told.

use super::*;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

const OLD: &str = "2026-01-01T00:00:00Z";
const LATER: &str = "2099-01-01T00:00:00Z";
/// `LATER` as seconds since the epoch, and a moment after it.
const LATER_UNIX: i64 = 4_070_908_800;
const AFTER_LATER: i64 = 4_200_000_000;
/// The scan that last saw a record whose last sighting is `LATER`: completed,
/// from before the library recorded which folders a scan covered.
fn scan_at_later() -> ScanRun {
    ScanRun {
        started: LATER_UNIX - 60,
        finished: LATER_UNIX + 600,
        completed: true,
        coverage: None,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn set_mtime(path: &Path, seconds: u64) {
    fs::File::open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

fn list(root: &Path) -> Inventory {
    take_inventory(
        &InventoryRequest {
            roots: &[root.to_path_buf()],
            max_entries: DEFAULT_MAX_ENTRIES,
        },
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap()
}

fn record(root: &Path, path: &str, size: u64, mtime: i64) -> OldRecord {
    OldRecord {
        set: RecordSet::FlaggedMissing,
        archive_id: 1,
        path: root.join(path),
        archive_kind: "direct_game_image".into(),
        size: Some(size),
        mtime: Some(mtime),
        first_seen_at: Some(OLD.into()),
        last_seen_at: Some(LATER.into()),
        flagged_missing_at: Some(LATER.into()),
        platform: None,
        source_device: Some(fs::metadata(root).unwrap().dev()),
        source_root: Some(root.to_path_buf()),
        source_folder_id: Some(1),
    }
}

fn classify(inputs: &ReconcileInputs, inventory: &Inventory) -> Vec<RecordFinding> {
    let (links, _) = audit_links(inventory, &AtomicBool::new(false));
    classify_records(
        inputs,
        inventory,
        &links,
        None,
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
}

fn one(inputs: ReconcileInputs, inventory: &Inventory) -> RecordFinding {
    classify(&inputs, inventory).pop().unwrap()
}

fn inputs(record: OldRecord) -> ReconcileInputs {
    ReconcileInputs {
        old: vec![record],
        ..ReconcileInputs::default()
    }
}

#[test]
fn a_moved_file_with_the_same_name_size_and_date_is_a_metadata_match_not_a_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let new = root.join("new-name/Example Game (World).rom");
    write(&new, &[7; 9000]);
    set_mtime(&new, 800_000_000);
    let finding = one(
        inputs(record(
            root,
            "old-name/Example Game (World).rom",
            9000,
            800_000_000,
        )),
        &list(root),
    );
    // No library row for the new path, so the object test cannot hold.
    assert_eq!(finding.tier, EvidenceTier::MetadataMatch);
    assert_eq!(finding.class, EvidenceClass::Metadata);
    assert_eq!(finding.target.as_deref(), Some(new.as_path()));
    assert!(finding.tier.tied_to_file());
}

#[test]
fn the_same_object_tier_needs_an_older_inode_a_later_row_and_the_same_disk() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let new = root.join("new-name/Example Game (World).rom");
    write(&new, &[7; 9000]);
    set_mtime(&new, 800_000_000);
    let inventory = list(root);
    let row = |first_seen: &str, folder: i64| CurrentRow {
        archive_id: 2,
        path: new.clone(),
        archive_kind: "direct_game_image".into(),
        first_seen_at: first_seen.into(),
        source_folder_id: folder,
    };
    let old = record(root, "old-name/Example Game (World).rom", 9000, 800_000_000);
    let case = |old: &OldRecord, first_seen: &str, folder: i64, scans: &[ScanRun]| {
        let mut all = inputs(old.clone());
        all.current = vec![row(first_seen, folder)];
        all.scan_runs = scans.to_vec();
        one(all, &inventory)
    };
    const AFTER: &str = "2099-06-01T00:00:00Z";
    // The file exists now (created before LATER). A completed scan of its
    // library folder last saw the old path, and its row first appears later.
    assert_eq!(
        case(&old, AFTER, 1, &[scan_at_later()]).tier,
        EvidenceTier::SameFileObject
    );
    // Stamped five minutes later but in the same scan: both files existed then.
    let together = case(&old, "2099-01-01T00:05:00Z", 1, &[scan_at_later()]);
    assert_eq!(together.tier, EvidenceTier::MetadataMatch);
    assert!(
        together
            .evidence
            .iter()
            .any(|line| line.contains("side by side"))
    );
    // Both rows existed long before.
    assert_eq!(
        case(&old, OLD, 1, &[scan_at_later()]).tier,
        EvidenceTier::MetadataMatch
    );
    // No scan is known to have seen the old path then: nothing can be said.
    assert_eq!(case(&old, AFTER, 1, &[]).tier, EvidenceTier::MetadataMatch);
    // The scan that last saw the old path was cut short: it may never have
    // reached the folder this file is in.
    let cut_short = ScanRun {
        completed: false,
        ..scan_at_later()
    };
    let not_covered = case(&old, AFTER, 1, &[cut_short]);
    assert_eq!(not_covered.tier, EvidenceTier::MetadataMatch);
    assert!(
        not_covered
            .evidence
            .iter()
            .any(|line| line.contains("not known to have completed"))
    );
    // The file is in another library folder, which that scan is not known to
    // have walked: it can be a copy that was there all along.
    assert_eq!(
        case(&old, AFTER, 2, &[scan_at_later()]).tier,
        EvidenceTier::MetadataMatch
    );
    // Where the library recorded what a scan covered, that record decides.
    let recorded = |complete: bool| ScanRun {
        coverage: Some(HashMap::from([(2, complete)])),
        ..scan_at_later()
    };
    assert_eq!(
        case(&old, AFTER, 2, &[recorded(true)]).tier,
        EvidenceTier::SameFileObject
    );
    assert_eq!(
        case(&old, AFTER, 2, &[recorded(false)]).tier,
        EvidenceTier::MetadataMatch
    );
    assert_eq!(
        case(&old, AFTER, 1, &[recorded(true)]).tier,
        EvidenceTier::MetadataMatch,
        "a folder the scan's coverage record does not name was not covered"
    );
    // Two scans share the second of the sighting: the later one is the one
    // that counts, and the row is not later than it.
    let next = ScanRun {
        started: LATER_UNIX,
        finished: LATER_UNIX + 4_000_000_000,
        ..scan_at_later()
    };
    assert_eq!(
        case(&old, AFTER, 1, &[scan_at_later(), next]).tier,
        EvidenceTier::MetadataMatch
    );
    // Another disk: a copy, never the same object.
    let mut other_disk = old.clone();
    other_disk.source_device = Some(u64::MAX);
    assert_eq!(
        case(&other_disk, AFTER, 1, &[scan_at_later()]).tier,
        EvidenceTier::MetadataMatch
    );
}

#[test]
fn a_different_date_or_several_copies_is_only_weak_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(&root.join("a/Game.bin"), &[1; 9000]);
    set_mtime(&root.join("a/Game.bin"), 1000);
    let finding = one(
        inputs(record(root, "old/Game.bin", 9000, 2000)),
        &list(root),
    );
    assert_eq!(finding.tier, EvidenceTier::NameAndSize);
    write(&root.join("b/Game.bin"), &[2; 9000]);
    set_mtime(&root.join("b/Game.bin"), 2000);
    let finding = one(
        inputs(record(root, "old/Game.bin", 9000, 2000)),
        &list(root),
    );
    assert_eq!(finding.tier, EvidenceTier::NameAndSize);
    assert_eq!(finding.class, EvidenceClass::Weak);
}

#[test]
fn a_shared_title_on_another_system_never_ties_a_record_to_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // Two systems can each have a game of the same name.
    write(
        &root.join("system-b/Shared Title (Japan).zip"),
        &[3; 20_000],
    );
    let finding = one(
        inputs(record(root, "system-a/Shared Title (Japan).zip", 25_000, 1)),
        &list(root),
    );
    assert_eq!(finding.tier, EvidenceTier::NameOnlyLead);
    assert_eq!(finding.class, EvidenceClass::Candidate);
    assert!(!finding.tier.tied_to_file());
    assert!(
        finding
            .evidence
            .iter()
            .any(|line| line.contains("a name alone is not evidence"))
    );
}

#[test]
fn cover_art_with_the_game_s_name_is_not_a_lead() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        &root.join("system-a/assets/covers/Rare Game.png"),
        &[4; 5000],
    );
    write(&root.join("system-a/Rare Game.txt"), b"notes");
    let finding = one(
        inputs(record(root, "old/Rare Game.zip", 5000, 1)),
        &list(root),
    );
    assert_eq!(finding.tier, EvidenceTier::NoTrace);
    assert_eq!(finding.target, None);
}

#[test]
fn an_identical_byte_size_under_another_name_is_a_candidate_only_when_unique_and_not_tiny() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        &root.join("system-b/Multi Game (Asia) (Pirate).zip"),
        &[5; 110_000],
    );
    let finding = one(
        inputs(record(
            root,
            "system-a/Multi Game (Asia) (Unl).zip",
            110_000,
            1,
        )),
        &list(root),
    );
    assert_eq!(finding.tier, EvidenceTier::SizeOnlyCandidate);
    assert!(!finding.tier.tied_to_file());
    // A second file of that size makes it meaningless.
    write(&root.join("system-b/Other.zip"), &[6; 110_000]);
    let finding = one(
        inputs(record(
            root,
            "system-a/Multi Game (Asia) (Unl).zip",
            110_000,
            1,
        )),
        &list(root),
    );
    assert_eq!(finding.tier, EvidenceTier::NoTrace);
    // Tiny files share sizes all the time.
    write(&root.join("system-b/Small.zip"), &[7; 300]);
    let finding = one(
        inputs(record(root, "system-a/Tiny.zip", 300, 1)),
        &list(root),
    );
    assert_eq!(finding.tier, EvidenceTier::NoTrace);
}

#[test]
fn emuwiz_s_own_move_record_plus_the_same_inode_confirms_and_a_hash_confirms_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let now = root.join("carts/Example Cart.zip");
    write(&now, &[8; 3000]);
    set_mtime(&now, 5);
    let metadata = fs::metadata(&now).unwrap();
    let old = record(root, "carts/Example Cart (USA).zip", 3000, 5);
    let mut all = inputs(old.clone());
    all.moves = vec![JournalMove {
        from: old.path.clone(),
        to: now.clone(),
        inode: metadata.ino(),
        device: metadata.dev() + 16,
        size: 3000,
        modified: 5,
        applied_at: Some(AFTER_LATER),
    }];
    let inventory = list(root);
    let finding = one(all.clone(), &inventory);
    assert_eq!(finding.tier, EvidenceTier::ConfirmedObject);
    // A renumbered disk does not break the match, and it is said.
    assert!(
        finding
            .evidence
            .iter()
            .any(|line| line.contains("renumbered"))
    );

    struct Fixed(ContentVerdict);
    impl ContentVerifier for Fixed {
        fn compare(&mut self, _: &Path, _: &RecordedHashes) -> ContentVerdict {
            self.0
        }
    }
    all.recorded_hashes.insert(
        old.path.clone(),
        RecordedHashes {
            source: "RomM".into(),
            ..RecordedHashes::default()
        },
    );
    let (links, _) = audit_links(&inventory, &AtomicBool::new(false));
    let run = |verdict| {
        classify_records(
            &all,
            &inventory,
            &links,
            Some(&mut Fixed(verdict)),
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .pop()
        .unwrap()
    };
    assert_eq!(
        run(ContentVerdict::Match).tier,
        EvidenceTier::ConfirmedContent
    );
    // Contents that differ from the recorded hash contradict the move record:
    // that is a conflict to look at, never a confirmation.
    let differs = run(ContentVerdict::Differs);
    assert_eq!(differs.tier, EvidenceTier::MetadataMatch);
    assert_ne!(differs.class, EvidenceClass::Confirmed);
    assert!(
        differs
            .evidence
            .iter()
            .any(|line| line.starts_with("WARNING"))
    );

    // The journal's file renamed again later: found by inode, not by name.
    let later = root.join("carts/Example Cart (1982).zip");
    fs::rename(&now, &later).unwrap();
    all.recorded_hashes.clear();
    let finding = one(all, &list(root));
    assert_eq!(finding.tier, EvidenceTier::ConfirmedObject);
    assert_eq!(finding.target.as_deref(), Some(later.as_path()));
}

#[test]
fn a_file_at_its_recorded_path_is_present_and_nothing_at_all_is_no_trace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(&root.join("arcade/set/disk.img"), &[9; 5000]);
    let present = one(
        inputs(record(root, "arcade/set/disk.img", 5000, 1)),
        &list(root),
    );
    assert_eq!(present.tier, EvidenceTier::PresentAtRecordedPath);
    assert_eq!(present.class, EvidenceClass::Present);
    let gone = one(
        inputs(record(root, "arcade/set/other.img", 6000, 1)),
        &list(root),
    );
    assert_eq!(gone.tier, EvidenceTier::NoTrace);
    assert_eq!(gone.class, EvidenceClass::Unresolved);
}

#[test]
fn a_file_that_changes_after_the_listing_is_not_reported_as_found() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let new = root.join("new-name/Example Game.rom");
    write(&new, &[7; 9000]);
    set_mtime(&new, 1000);
    let inventory = list(root);
    fs::write(&new, [7; 9001]).unwrap();
    let finding = one(
        inputs(record(root, "old/Example Game.rom", 9000, 1000)),
        &inventory,
    );
    assert_eq!(finding.tier, EvidenceTier::NoTrace);
    assert!(finding.evidence[0].contains("changed while it was being examined"));
}

#[test]
fn results_do_not_depend_on_input_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(&root.join("gb/A.gb"), &[1; 9000]);
    write(&root.join("gb/B.gb"), &[2; 9500]);
    let inventory = list(root);
    let mut a = record(root, "old/A.gb", 9000, 1);
    a.archive_id = 10;
    let mut b = record(root, "old/B.gb", 9500, 1);
    b.archive_id = 5;
    let forward = classify(
        &ReconcileInputs {
            old: vec![a.clone(), b.clone()],
            ..Default::default()
        },
        &inventory,
    );
    let backward = classify(
        &ReconcileInputs {
            old: vec![b, a],
            ..Default::default()
        },
        &inventory,
    );
    assert_eq!(forward, backward);
    assert_eq!(forward[0].archive_id, 5);
}

#[test]
fn links_are_listed_followed_only_at_the_top_and_never_entered_deeper() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    let elsewhere = temp.path().join("second-disk/big-games");
    write(&elsewhere.join("Game.pkg"), &[1; 10]);
    write(&root.join("gb/A.gb"), &[1; 10]);
    symlink(&elsewhere, root.join("big-games")).unwrap();
    fs::create_dir_all(root.join("gb/deep")).unwrap();
    symlink(&elsewhere, root.join("gb/deep/link")).unwrap();
    let inventory = list(&root);
    assert!(inventory.summary.complete);
    assert!(
        inventory.get(&root.join("big-games/Game.pkg")).is_some(),
        "top-level linked folder is part of the library"
    );
    assert!(
        inventory.get(&root.join("gb/deep/link/Game.pkg")).is_none(),
        "deeper links are not followed"
    );
    assert_eq!(
        inventory.get(&root.join("big-games")).unwrap().kind,
        EntryKind::Link
    );
    assert_eq!(inventory.summary.roots[0].links, 2);
}

#[test]
fn an_unreadable_folder_makes_the_listing_incomplete() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(&root.join("gb/A.gb"), &[1; 10]);
    let locked = root.join("locked");
    write(&locked.join("B.gb"), &[1; 10]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let inventory = list(root);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if fs::read_dir(&locked).is_ok() && inventory.summary.complete {
        return; // running as a user that ignores permissions
    }
    assert!(!inventory.summary.complete);
    assert!(!inventory.summary.roots[0].errors.is_empty());
}

fn link_tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let games = temp.path().join("games");
    let downloads = temp.path().join("downloads");
    fs::create_dir_all(games.join("collection")).unwrap();
    (temp, games, downloads)
}

#[test]
fn a_link_broken_by_a_folder_moved_up_is_identified_when_the_file_is_older_than_the_link() {
    let (_temp, games, downloads) = link_tree();
    let old_place = downloads.join("incoming/staging/Collection/Disks/Game.img");
    write(&old_place, &[1; 5000]);
    std::thread::sleep(Duration::from_millis(1100));
    symlink(&old_place, games.join("collection/Game.img")).unwrap();
    // The download folder is moved up one level; the old place is left empty.
    fs::rename(
        downloads.join("incoming/staging/Collection"),
        downloads.join("Collection"),
    )
    .unwrap();
    let inventory = list(&games);
    let (findings, summary) = audit_links(&inventory, &AtomicBool::new(false));
    assert_eq!(
        (summary.total, summary.broken, summary.target_identified),
        (1, 1, 1)
    );
    assert_eq!(
        summary.storage_unavailable, 1,
        "an emptied folder is storage that is not there"
    );
    let finding = &findings[0];
    assert_eq!(finding.cause, Some(LinkCause::StorageEmpty));
    assert_eq!(finding.grade, LinkGrade::TargetIdentified);
    assert_eq!(
        finding.found.as_deref(),
        Some(downloads.join("Collection/Disks/Game.img").as_path())
    );
    assert_eq!(finding.stored_target, old_place);

    // A record of the same name with the target's size is tied to it, weakly.
    let tied = classify(
        &inputs(record(&games, "old-collection/Game.img", 5000, 1)),
        &inventory,
    )
    .pop()
    .unwrap();
    assert_eq!(tied.tier, EvidenceTier::NameAndSizeViaLink);
    assert_eq!(tied.class, EvidenceClass::Weak);
    // A different size is not.
    let not = classify(
        &inputs(record(&games, "old-collection/Game.img", 4999, 1)),
        &inventory,
    )
    .pop()
    .unwrap();
    assert_eq!(not.tier, EvidenceTier::NoTrace);
}

#[test]
fn a_later_copy_beside_the_link_is_only_a_plausible_candidate() {
    let (_temp, games, _) = link_tree();
    let folder = games.join("other-collection");
    fs::create_dir_all(&folder).unwrap();
    symlink(
        "/nonexistent-old-storage/Collection/Game (Publisher)/Game [Disk 1].zip",
        folder.join("Game [Disk 1].zip"),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    write(
        &folder.join("Game (Publisher)/Game [Disk 1].zip"),
        &[1; 5000],
    );
    let (findings, summary) = audit_links(&list(&games), &AtomicBool::new(false));
    assert_eq!(summary.plausible_candidate, 1);
    assert_eq!(findings[0].cause, Some(LinkCause::LocationGone));
    assert_eq!(findings[0].grade, LinkGrade::PlausibleCandidate);
    assert!(
        findings[0]
            .evidence
            .iter()
            .any(|line| line.contains("later copy"))
    );
}

#[test]
fn a_dead_link_with_nothing_behind_it_is_unresolved_and_marks_a_record_as_link_only() {
    let (_temp, games, _) = link_tree();
    symlink(
        "/nonexistent-old-storage/Bonus Disk.zip",
        games.join("collection/Bonus Disk.zip"),
    )
    .unwrap();
    write(&games.join("collection/Real.img"), &[1; 10]);
    symlink(
        games.join("collection/Real.img"),
        games.join("collection/Works.img"),
    )
    .unwrap();
    let inventory = list(&games);
    let (_, summary) = audit_links(&inventory, &AtomicBool::new(false));
    assert_eq!(
        (summary.total, summary.resolve, summary.unresolved),
        (2, 1, 1)
    );
    assert_eq!(
        (summary.storage_unavailable, summary.target_missing),
        (1, 0)
    );
    let finding = classify(
        &inputs(record(&games, "old/Bonus Disk.zip", 7777, 1)),
        &inventory,
    )
    .pop()
    .unwrap();
    assert_eq!(finding.tier, EvidenceTier::BrokenLinkOnly);
    assert_eq!(finding.class, EvidenceClass::Unresolved);
}

#[test]
fn every_tier_has_one_class_and_a_distinct_code() {
    let codes: std::collections::HashSet<_> =
        EvidenceTier::ALL.iter().map(|tier| tier.code()).collect();
    assert_eq!(codes.len(), EvidenceTier::ALL.len());
    // Only the two confirmed tiers may be called confirmed.
    let confirmed: Vec<_> = EvidenceTier::ALL
        .iter()
        .filter(|t| t.class() == EvidenceClass::Confirmed)
        .collect();
    assert_eq!(
        confirmed,
        [
            &EvidenceTier::ConfirmedContent,
            &EvidenceTier::ConfirmedObject
        ]
    );
    assert!(!EvidenceTier::SizeOnlyCandidate.tied_to_file());
    assert!(!EvidenceTier::NameOnlyLead.tied_to_file());
}

#[test]
fn a_link_loop_is_named_as_a_loop_and_resolves_nothing() {
    let (_temp, games, _) = link_tree();
    symlink(
        games.join("collection/b.img"),
        games.join("collection/a.img"),
    )
    .unwrap();
    symlink(
        games.join("collection/a.img"),
        games.join("collection/b.img"),
    )
    .unwrap();
    let (findings, summary) = audit_links(&list(&games), &AtomicBool::new(false));
    assert_eq!((summary.total, summary.unresolved), (2, 2));
    assert!(
        findings
            .iter()
            .all(|finding| finding.cause == Some(LinkCause::LinkLoop))
    );
}

// ---- through a real (synthetic) library database -----------------------------

mod end_to_end {
    use super::*;
    use crate::Database;
    use rusqlite::{Connection, params};
    use sha1::{Digest, Sha1};
    use std::os::unix::ffi::OsStrExt;

    const SEEN: &str = "2090-01-01T00:00:00Z";
    const FLAGGED: &str = "2090-01-02T00:00:00Z";
    const NEW_ROW: &str = "2090-06-01T00:00:00Z";

    struct World {
        temp: tempfile::TempDir,
        db: PathBuf,
        games: PathBuf,
        raw: Connection,
    }

    fn world() -> World {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let db = data.join("library.sqlite3");
        drop(Database::open_or_create(&db).unwrap());
        let games = temp.path().join("games");
        fs::create_dir_all(&games).unwrap();
        let raw = Connection::open(&db).unwrap();
        World {
            temp,
            db,
            games,
            raw,
        }
    }

    impl World {
        fn source(&self, path: &Path, configured: bool) -> i64 {
            self.raw
                .execute(
                    "INSERT INTO source_folders(path, first_seen_at, last_seen_in_config_at, removed_from_config_at) \
                     VALUES (?1, ?2, ?2, ?3)",
                    params![path.as_os_str().as_bytes(), OLD, (!configured).then_some(SEEN)],
                )
                .unwrap();
            self.raw.last_insert_rowid()
        }

        #[allow(clippy::too_many_arguments)]
        fn row(
            &self,
            source: i64,
            root: &Path,
            relative: &str,
            size: u64,
            mtime: i64,
            first: &str,
            last: &str,
            flagged: Option<&str>,
        ) -> i64 {
            let absolute = root.join(relative);
            let name = absolute.file_name().unwrap();
            self.raw
                .execute(
                    "INSERT INTO archives(source_folder_id, relative_path, absolute_path_cached, file_name_cached, \
                     archive_kind, display_name, normalized_name, size_bytes, modified_time_unix_seconds, \
                     first_seen_at, last_seen_at, last_verified_missing_at, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, 'direct_game_image', ?5, ?5, ?6, ?7, ?8, ?9, ?10, ?8, ?9)",
                    params![
                        source,
                        relative.as_bytes(),
                        absolute.as_os_str().as_bytes(),
                        name.as_bytes(),
                        relative,
                        size as i64,
                        mtime,
                        first,
                        last,
                        flagged
                    ],
                )
                .unwrap();
            self.raw.last_insert_rowid()
        }

        /// Records a finished library scan.
        fn scan(&self, started: &str, finished: &str) {
            self.raw
                .execute(
                    "INSERT INTO scan_runs(started_at, finished_at, triggered_by, status) \
                     VALUES (?1, ?2, 'test', 'completed')",
                    params![started, finished],
                )
                .unwrap();
        }

        fn file(&self, relative: &str, bytes: &[u8], mtime: u64) -> PathBuf {
            let path = self.games.join(relative);
            write(&path, bytes);
            set_mtime(&path, mtime);
            path
        }

        fn run(&self, request: &ReconcileRequest) -> Result<ReconciliationReport, ReconcileError> {
            reconcile(request, &AtomicBool::new(false), &mut |_| {})
        }

        fn request(&self) -> ReconcileRequest {
            ReconcileRequest::new(&self.db)
        }
    }

    /// Names of everything in the library's own folder.
    fn beside(db: &Path) -> Vec<std::ffi::OsString> {
        let mut names: Vec<_> = fs::read_dir(db.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    fn tier_of(report: &ReconciliationReport, id: i64, set: RecordSet) -> EvidenceTier {
        report
            .records
            .iter()
            .find(|record| record.archive_id == id && record.set == set)
            .unwrap_or_else(|| panic!("no record {id} in {set:?}"))
            .tier
    }

    #[test]
    fn three_sets_stay_apart_and_the_library_is_not_changed() {
        let world = world();
        let source = world.source(&world.games, true);
        // The scan that last saw the old paths, and a later one.
        world.scan("2089-12-31T23:59:00Z", "2090-01-01T00:10:00Z");
        world.scan("2090-05-31T23:59:00Z", "2090-06-01T00:10:00Z");
        // A folder renamed outside EmuWiz: old-name/ became new-name/.
        world.file("new-name/Example Game.rom", &[1; 9000], 1000);
        let old = world.row(
            source,
            &world.games,
            "old-name/Example Game.rom",
            9000,
            1000,
            OLD,
            SEEN,
            Some(FLAGGED),
        );
        let new = world.row(
            source,
            &world.games,
            "new-name/Example Game.rom",
            9000,
            1000,
            NEW_ROW,
            NEW_ROW,
            None,
        );
        // A row that is not flagged although its file is gone.
        let gone = world.row(
            source,
            &world.games,
            "new-name/Gone.rom",
            7000,
            5,
            OLD,
            SEEN,
            None,
        );
        // A Forget Missing export: one row that moved, one that is back in the library.
        world.file("new-name/Second Game.rom", &[2; 9500], 2000);
        let export = world
            .db
            .with_file_name("library.sqlite3.forget-missing-20900103T000000Z-abc.json");
        let hex = |path: &Path| {
            path.as_os_str()
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        fs::write(
            &export,
            serde_json::json!({
                "version": 1, "state": "applied", "database_path": world.db, "created_at": "2090-01-03T00:00:00Z",
                "tables": [{
                    "table": "archives",
                    "columns": ["id", "source_folder_id", "absolute_path_cached", "archive_kind", "size_bytes",
                                "modified_time_unix_seconds", "first_seen_at", "last_seen_at", "last_verified_missing_at"],
                    "rows": [
                        [900, source, {"blob": hex(&world.games.join("old-name/Second Game.rom"))}, "direct_game_image", 9500, 2000, OLD, SEEN, FLAGGED],
                        [new, source, {"blob": hex(&world.games.join("new-name/Example Game.rom"))}, "direct_game_image", 9000, 1000, OLD, SEEN, FLAGGED]
                    ]
                }, {"table": "platform_assignments", "columns": ["archive_id", "platform", "is_current"],
                    "rows": [[900, "Game Boy", 1]]}]
            })
            .to_string(),
        )
        .unwrap();
        // An export written for some other library is not this library's history.
        let foreign = world
            .db
            .with_file_name("library.sqlite3.forget-missing-20900104T000000Z-def.json");
        fs::write(
            &foreign,
            fs::read_to_string(&export)
                .unwrap()
                .replace("\"database_path\":\"", "\"database_path\":\"/elsewhere"),
        )
        .unwrap();

        let before = fs::read(&world.db).unwrap();
        let beside_before = beside(&world.db);
        let report = world.run(&world.request()).unwrap();
        assert_eq!(
            fs::read(&world.db).unwrap(),
            before,
            "the library database must not change"
        );

        assert_eq!(
            tier_of(&report, old, RecordSet::FlaggedMissing),
            EvidenceTier::SameFileObject
        );
        assert_eq!(
            tier_of(&report, gone, RecordSet::AbsentNeverFlagged),
            EvidenceTier::NoTrace
        );
        assert_eq!(
            tier_of(&report, 900, RecordSet::Forgotten),
            EvidenceTier::MetadataMatch
        );
        let forgotten = report.records.iter().find(|r| r.archive_id == 900).unwrap();
        assert_eq!(forgotten.platform.as_deref(), Some("Game Boy"));
        assert_eq!(
            report.records.len(),
            3,
            "a forgotten row that is back in the library is not counted twice"
        );
        let m = &report.measures;
        assert_eq!(
            (
                m.rows_in_library,
                m.rows_flagged_missing,
                m.rows_absent_never_flagged,
                m.rows_present_as_recorded
            ),
            (3, 1, 1, 1)
        );
        assert_eq!(
            (
                m.records_removed_by_forget_missing,
                m.records_examined,
                m.records_tied_to_a_file
            ),
            (1, 3, 2)
        );
        assert_eq!(report.receipts.len(), 1);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("written for another database"))
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("not counted again"))
        );
        // Nothing is written anywhere, and nothing is kept between runs.
        assert!(!world.temp.path().join("cache").exists());
        assert_eq!(
            beside(&world.db),
            beside_before,
            "nothing new beside the library"
        );

        // Named explicitly, the foreign export is read, and its rows are still not counted twice.
        let mut named = world.request();
        named.forget_receipts = vec![foreign];
        let again = world.run(&named).unwrap();
        assert_eq!(again.receipts.len(), 2);
        assert_eq!(again.records.len(), 3);
        // Same inputs, same answer.
        assert_eq!(again.records, report.records);
    }

    #[test]
    fn an_unattached_drive_is_not_judged_and_an_unreadable_folder_stops_the_report() {
        let world = world();
        let source = world.source(&world.games, true);
        world.file("new-name/Here.rom", &[1; 10], 1);
        world.row(
            source,
            &world.games,
            "new-name/Here.rom",
            10,
            1,
            OLD,
            SEEN,
            None,
        );
        // A retired library folder that exists but is empty: its drive is not attached.
        let other = world.temp.path().join("other-drive");
        fs::create_dir_all(&other).unwrap();
        let retired = world.source(&other, false);
        world.row(retired, &other, "system-a/A.rom", 10, 1, OLD, SEEN, None);
        world.row(retired, &other, "system-a/B.rom", 10, 1, OLD, SEEN, None);
        let report = world.run(&world.request()).unwrap();
        assert_eq!(report.measures.rows_not_checked, 2);
        assert_eq!(
            report.measures.rows_absent_never_flagged, 0,
            "an empty drive is not a library of lost games"
        );
        assert!(report.records.is_empty());
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("were not checked"))
        );

        let locked = world.games.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let outcome = world.run(&world.request());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if fs::read_dir(&locked).is_ok() && outcome.is_ok() {
            return; // running as a user that ignores permissions
        }
        assert!(
            matches!(outcome, Err(ReconcileError::IncompleteListing(_))),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_journalled_rename_is_confirmed_and_a_recorded_hash_confirms_the_contents() {
        let world = world();
        let source = world.source(&world.games, true);
        let now = world.file("carts/Example Cart.bin", b"example rom bytes", 3000);
        let metadata = fs::metadata(&now).unwrap();
        let old_path = world.games.join("carts/Example Cart (USA).bin");
        let old = world.row(
            source,
            &world.games,
            "carts/Example Cart (USA).bin",
            17,
            3000,
            OLD,
            SEEN,
            Some(FLAGGED),
        );
        world.row(
            source,
            &world.games,
            "carts/Example Cart.bin",
            17,
            3000,
            NEW_ROW,
            NEW_ROW,
            None,
        );
        let journals = world.db.with_file_name("rename-transactions");
        fs::create_dir_all(&journals).unwrap();
        fs::write(
            journals.join("1-0.json"),
            serde_json::json!({
                "transaction_id": "1-0", "plan_generation": 1, "created_at_unix": 1, "source_scan_root": world.games,
                "state": "applied",
                "entries": [{
                    "source_path": old_path, "destination_path": now, "original_basename": "Example Cart (USA).bin",
                    "proposed_basename": "Example Cart.bin",
                    "identity": {"size_bytes": 17, "modified_unix": 3000, "kind": "regular_file", "ino": metadata.ino(), "dev": metadata.dev()},
                    "operation": {"kind": "rename_move"}, "preflight_passed": true, "state": "applied", "applied_at_unix": 3_900_000_000u64
                }]
            })
            .to_string(),
        )
        .unwrap();
        let report = world.run(&world.request()).unwrap();
        assert_eq!(
            tier_of(&report, old, RecordSet::FlaggedMissing),
            EvidenceTier::ConfirmedObject,
            "notes: {:?} records: {:?}",
            report.notes,
            report.records
        );
        assert!(!report.content_check.requested);

        // RomM hashed the file under its old name. Asked to, the check reads the file and compares.
        let sha1: String = Sha1::digest(b"example rom bytes")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let cache = world.db.with_file_name("identity").join("romm");
        fs::create_dir_all(&cache).unwrap();
        let write_cache = |sha1: &str| {
            fs::write(
                cache.join("identity-cache.json"),
                serde_json::json!({"format_version": 1, "records": [
                    {"archivefs_path": old_path, "hashes": [{"algorithm": "sha1", "value": sha1}], "title": "Example Cart"}
                ]})
                .to_string(),
            )
            .unwrap();
        };
        write_cache(&sha1);
        let mut request = world.request();
        request.verify_content = true;
        let report = world.run(&request).unwrap();
        assert_eq!(
            tier_of(&report, old, RecordSet::FlaggedMissing),
            EvidenceTier::ConfirmedContent
        );
        assert_eq!(
            (report.content_check.compared, report.content_check.matched),
            (1, 1)
        );
        assert_eq!(
            report.content_check.bytes_read,
            b"example rom bytes".len() as u64
        );

        // A hash that does not match is said loudly and the record is no longer confirmed.
        write_cache(&"0".repeat(40));
        let report = world.run(&request).unwrap();
        assert_eq!(
            tier_of(&report, old, RecordSet::FlaggedMissing),
            EvidenceTier::MetadataMatch
        );
        assert_eq!(report.content_check.differed, 1);
        let record = report.records.iter().find(|r| r.archive_id == old).unwrap();
        assert!(
            record
                .evidence
                .iter()
                .any(|line| line.starts_with("WARNING"))
        );
    }
    #[test]
    fn rows_under_an_empty_folder_are_not_judged_even_when_flagged() {
        let world = world();
        let source = world.source(&world.games, true);
        world.file("system-a/Here.rom", &[1; 10], 1);
        world.row(
            source,
            &world.games,
            "system-a/Here.rom",
            10,
            1,
            OLD,
            SEEN,
            None,
        );
        // A folder inside the library that is there but empty: a drive that is
        // mounted here and not attached looks exactly like this.
        fs::create_dir_all(world.games.join("mounted-here")).unwrap();
        let flagged = world.row(
            source,
            &world.games,
            "mounted-here/A.rom",
            10,
            1,
            OLD,
            SEEN,
            Some(FLAGGED),
        );
        world.row(
            source,
            &world.games,
            "mounted-here/B.rom",
            10,
            1,
            OLD,
            SEEN,
            None,
        );
        let report = world.run(&world.request()).unwrap();
        assert_eq!(report.measures.rows_not_checked, 2, "{:?}", report.notes);
        assert_eq!(report.measures.rows_absent_never_flagged, 0);
        assert!(
            report
                .records
                .iter()
                .all(|record| record.archive_id != flagged),
            "a flagged row that cannot be judged gets no finding, not 'no trace'"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("were not checked"))
        );
    }

    #[test]
    fn a_forgotten_path_that_is_catalogued_again_is_counted_once() {
        let world = world();
        let source = world.source(&world.games, true);
        world.file("system-a/Here.rom", &[1; 10], 1);
        world.row(
            source,
            &world.games,
            "system-a/Here.rom",
            10,
            1,
            OLD,
            SEEN,
            None,
        );
        // The path was forgotten under one id, then catalogued again under
        // another and flagged again. It is one missing record, not two.
        let again = world.row(
            source,
            &world.games,
            "system-a/Gone.rom",
            10,
            1,
            OLD,
            SEEN,
            Some(FLAGGED),
        );
        let export = world
            .db
            .with_file_name("library.sqlite3.forget-missing-20900103T000000Z-abc.json");
        let hex: String = world
            .games
            .join("system-a/Gone.rom")
            .as_os_str()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        fs::write(
            &export,
            serde_json::json!({
                "version": 1, "state": "applied", "database_path": world.db,
                "tables": [{"table": "archives",
                    "columns": ["id", "source_folder_id", "absolute_path_cached", "archive_kind", "size_bytes"],
                    "rows": [[again + 500, source, {"blob": hex}, "direct_game_image", 10]]}]
            })
            .to_string(),
        )
        .unwrap();
        let report = world.run(&world.request()).unwrap();
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].set, RecordSet::FlaggedMissing);
        assert_eq!(report.measures.records_removed_by_forget_missing, 0);
    }

    #[test]
    fn a_link_at_an_unflagged_rows_path_is_not_counted_as_the_file() {
        let world = world();
        let source = world.source(&world.games, true);
        world.file("system-a/Here.rom", &[1; 10], 1);
        world.row(
            source,
            &world.games,
            "system-a/Here.rom",
            10,
            1,
            OLD,
            SEEN,
            None,
        );
        // The row's file was replaced by a link to a copy kept elsewhere.
        let elsewhere = world.temp.path().join("elsewhere/Linked.rom");
        write(&elsewhere, &[2; 9000]);
        symlink(&elsewhere, world.games.join("system-a/Linked.rom")).unwrap();
        let linked = world.row(
            source,
            &world.games,
            "system-a/Linked.rom",
            9000,
            1,
            OLD,
            SEEN,
            None,
        );
        // And a whole folder was replaced by a link to another place.
        let moved = world.temp.path().join("elsewhere/moved");
        write(&moved.join("Game.rom"), &[3; 8000]);
        symlink(&moved, world.games.join("moved")).unwrap();
        let through = world.row(
            source,
            &world.games,
            "moved/Game.rom",
            8000,
            1,
            OLD,
            SEEN,
            None,
        );

        let report = world.run(&world.request()).unwrap();
        assert_eq!(
            report.measures.rows_present_as_recorded, 1,
            "only the real file"
        );
        assert_eq!(report.measures.rows_absent_never_flagged, 2);
        for id in [linked, through] {
            let record = report.records.iter().find(|r| r.archive_id == id).unwrap();
            assert_ne!(
                record.tier,
                EvidenceTier::PresentAtRecordedPath,
                "{record:?}"
            );
            assert!(
                record.evidence.iter().any(|line| line.contains("link")),
                "the reason must be stated: {:?}",
                record.evidence
            );
        }
        assert_eq!(report.measures.links_that_are_library_rows, 1);
    }

    #[test]
    fn forgotten_rows_under_an_empty_folder_are_not_judged() {
        let world = world();
        let source = world.source(&world.games, true);
        world.file("system-a/Here.rom", &[1; 10], 1);
        world.row(
            source,
            &world.games,
            "system-a/Here.rom",
            10,
            1,
            OLD,
            SEEN,
            None,
        );
        fs::create_dir_all(world.games.join("mounted-here")).unwrap();
        let export = world
            .db
            .with_file_name("library.sqlite3.forget-missing-20900103T000000Z-abc.json");
        let hex: String = world
            .games
            .join("mounted-here/Gone.rom")
            .as_os_str()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        fs::write(
            &export,
            serde_json::json!({
                "version": 1, "state": "applied", "database_path": world.db,
                "tables": [{"table": "archives",
                    "columns": ["id", "source_folder_id", "absolute_path_cached", "archive_kind", "size_bytes"],
                    "rows": [[700, source, {"blob": hex}, "direct_game_image", 10]]}]
            })
            .to_string(),
        )
        .unwrap();
        let report = world.run(&world.request()).unwrap();
        assert!(report.records.is_empty(), "{:?}", report.records);
        assert_eq!(report.measures.forgotten_records_not_checked, 1);
        assert_eq!(report.measures.records_removed_by_forget_missing, 0);
    }

    #[test]
    fn a_listing_cut_short_refuses_the_whole_report() {
        let world = world();
        let source = world.source(&world.games, true);
        world.file("system-a/Here.rom", &[1; 10], 1);
        world.row(
            source,
            &world.games,
            "system-a/Here.rom",
            10,
            1,
            OLD,
            SEEN,
            None,
        );
        let mut request = world.request();
        request.max_entries = 0;
        assert!(matches!(
            world.run(&request),
            Err(ReconcileError::IncompleteListing(_))
        ));
    }
}

// ---- how the library scan treats links today ---------------------------------
//
// Characterisation, not a wish: both walkers skip every symbolic link without
// counting it. Symlink-aware discovery (docs/COLLECTION_RECONCILIATION.md,
// section 8) will change this deliberately; until then these tests pin what a
// person actually gets, with the usual shapes a link can take.

mod scanner_today {
    use super::*;
    use crate::ingestion::discovery::discover_source;
    use crate::{ArchiveScanner, Config};

    /// A library folder with one real game and every kind of link.
    fn library() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let games = temp.path().join("games");
        let remote = temp.path().join("remote-storage");
        let second_disk = temp.path().join("second-disk/big-games");
        write(&games.join("megadrive/Real.smd"), &[1; 4096]);
        write(&remote.join("Linked.smd"), &[2; 4096]);
        write(&second_disk.join("Inside.smd"), &[3; 4096]);
        // A working link to a game kept on other storage.
        symlink(
            remote.join("Linked.smd"),
            games.join("megadrive/Linked.smd"),
        )
        .unwrap();
        // A linked folder directly inside the library.
        symlink(&second_disk, games.join("big-games")).unwrap();
        // A link whose target is gone, and a loop.
        symlink(
            temp.path().join("vanished/Gone.smd"),
            games.join("megadrive/Broken.smd"),
        )
        .unwrap();
        symlink(
            games.join("megadrive/LoopB.smd"),
            games.join("megadrive/LoopA.smd"),
        )
        .unwrap();
        symlink(
            games.join("megadrive/LoopA.smd"),
            games.join("megadrive/LoopB.smd"),
        )
        .unwrap();
        (temp, games)
    }

    #[test]
    fn the_library_scan_catalogues_no_link_and_does_not_mention_any() {
        let (_temp, games) = library();
        let config = Config {
            configured_source_roots: Some(vec![games.clone()]),
            source_folders: vec![games.clone()],
            mount_root: games.join("../mount"),
            ratarmount_bin: String::new(),
            master_rom_root: None,
        };
        let discovery = ArchiveScanner::new(&config)
            .scan_archives_with_summary()
            .unwrap();
        let names: Vec<_> = discovery
            .archives
            .iter()
            .map(|archive| {
                archive
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["Real.smd"], "only the real file is catalogued");
        assert!(
            discovery
                .skipped_files
                .iter()
                .all(|skipped| !skipped.path.starts_with(games.join("megadrive"))
                    || skipped.path.ends_with("Real.smd")),
            "links are dropped without a skip record: {:?}",
            discovery.skipped_files
        );
        // The reconciliation listing, by contrast, sees and explains all five.
        let (findings, summary) = audit_links(&list(&games), &AtomicBool::new(false));
        assert_eq!((summary.total, summary.resolve, summary.broken), (5, 2, 3));
        assert_eq!(
            findings
                .iter()
                .filter(|f| f.cause == Some(LinkCause::LinkLoop))
                .count(),
            2
        );
    }

    #[test]
    fn discovery_preview_skips_links_too() {
        let (_temp, games) = library();
        let report = discover_source(&games).unwrap();
        let paths: Vec<_> = report.items.iter().map(|item| item.path.clone()).collect();
        assert!(!paths.is_empty(), "the real game's folder is reported");
        assert!(
            paths.iter().all(|path| !path.ends_with("big-games")),
            "a linked folder is not discovered: {paths:?}"
        );
        assert!(
            paths
                .iter()
                .all(|path| !path.to_string_lossy().contains("Linked")
                    && !path.to_string_lossy().contains("Inside")
                    && !path.to_string_lossy().contains("Broken")
                    && !path.to_string_lossy().contains("Loop")),
            "no link, working or broken, is discovered: {paths:?}"
        );
    }
}

// ---- evidence that must not be overstated ------------------------------------
//
// Each case is written from the rule, not from the code: the expected outcome
// is what a careful person would conclude from the facts in the fixture.

mod evidence_is_not_overstated {
    use super::*;

    /// A file EmuWiz renamed itself, the journal entry for it, and a record
    /// at the old path that really is about that file.
    fn renamed(root: &Path) -> (ReconcileInputs, PathBuf) {
        let now = root.join("carts/Example Cart.zip");
        write(&now, &[8; 9000]);
        set_mtime(&now, 5);
        let metadata = fs::metadata(&now).unwrap();
        let old = record(root, "carts/Example Cart (USA).zip", 9000, 5);
        let mut all = inputs(old.clone());
        all.moves = vec![JournalMove {
            from: old.path,
            to: now.clone(),
            inode: metadata.ino(),
            device: metadata.dev(),
            size: 9000,
            modified: 5,
            applied_at: Some(AFTER_LATER),
        }];
        (all, now)
    }

    #[test]
    fn rename_history_alone_never_confirms_a_record_about_another_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (genuine, now) = renamed(root);
        let inventory = list(root);
        let confirmed = one(genuine.clone(), &inventory);
        assert_eq!(confirmed.tier, EvidenceTier::ConfirmedObject);
        assert_eq!(confirmed.target.as_deref(), Some(now.as_path()));

        // In every case below the journal's destination exists and is the very
        // file the journal moved. Only the record differs.
        type Change = fn(&mut ReconcileInputs);
        let cases: [(&str, &str, Change); 5] = [
            (
                "a different file of another size later had the old path",
                "about another file",
                |i| i.old[0].size = Some(9001),
            ),
            (
                "same name and size, another date",
                "about another file",
                |i| i.old[0].mtime = Some(6),
            ),
            // The hardest one: same name, same size, same date, but the library
            // saw a file at the old path again after EmuWiz had moved it away.
            (
                "same name, size and date, seen after the move",
                "moved an earlier file at this path",
                |i| i.old[0].last_seen_at = Some("2110-01-01T00:00:00Z".into()),
            ),
            (
                "the journal does not say when the move happened",
                "does not say when the move was applied",
                |i| i.moves[0].applied_at = None,
            ),
            ("the record carries no size", "about another file", |i| {
                i.old[0].size = None
            }),
        ];
        for (what, reason, change) in cases {
            let mut inputs = genuine.clone();
            change(&mut inputs);
            let finding = one(inputs, &inventory);
            assert_ne!(
                finding.class,
                EvidenceClass::Confirmed,
                "{what}: {finding:?}"
            );
            assert!(
                finding.evidence.iter().any(|line| line.contains(reason)),
                "{what}: the reason must be stated: {:?}",
                finding.evidence
            );
            // What is left is at most a lead to the file under its new name.
            assert!(
                !finding.tier.tied_to_file(),
                "{what}: a record that is not shown to be about the file is not tied to it"
            );
        }
    }

    #[test]
    fn the_same_inode_number_on_another_disk_or_with_another_date_never_confirms() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (genuine, _) = renamed(root);
        let inventory = list(root);
        // The record's library folder is on some other disk: an equal inode
        // number there says nothing about this file.
        let mut other_disk = genuine.clone();
        other_disk.old[0].source_device = Some(u64::MAX);
        assert_ne!(one(other_disk, &inventory).class, EvidenceClass::Confirmed);
        // The inode was reused by a file of the same size with another date.
        let mut reused = genuine;
        reused.moves[0].modified = 7;
        reused.old[0].mtime = Some(7);
        assert_ne!(one(reused, &inventory).class, EvidenceClass::Confirmed);
    }

    #[test]
    fn a_link_left_at_the_recorded_place_is_not_the_recorded_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("elsewhere/Real.rom"), &[1; 9000]);
        fs::create_dir_all(root.join("system-a")).unwrap();
        symlink(
            root.join("elsewhere/Real.rom"),
            root.join("system-a/Works.rom"),
        )
        .unwrap();
        symlink(
            root.join("nowhere/Gone.rom"),
            root.join("system-a/Dangles.rom"),
        )
        .unwrap();
        let inventory = list(root);
        for name in ["system-a/Works.rom", "system-a/Dangles.rom"] {
            let finding = one(inputs(record(root, name, 9000, 1)), &inventory);
            assert_ne!(finding.tier, EvidenceTier::PresentAtRecordedPath, "{name}");
            assert!(
                finding
                    .evidence
                    .iter()
                    .any(|line| line.contains("a link now sits")),
                "{name}: {:?}",
                finding.evidence
            );
        }
    }

    #[test]
    fn a_file_created_after_the_old_path_was_last_seen_is_not_the_same_object() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let new = root.join("new-name/Example Game.rom");
        write(&new, &[7; 9000]);
        set_mtime(&new, 800_000_000);
        let mut old = record(root, "old-name/Example Game.rom", 9000, 800_000_000);
        // Last seen long before this file existed: it is a later copy.
        old.last_seen_at = Some(OLD.into());
        let mut all = inputs(old);
        all.current = vec![CurrentRow {
            archive_id: 2,
            path: new,
            archive_kind: "direct_game_image".into(),
            first_seen_at: "2026-06-01T00:00:00Z".into(),
            source_folder_id: 1,
        }];
        // The scan that last saw the old path is known, and the new row is from a later one.
        all.scan_runs = vec![ScanRun {
            started: 1_767_225_540,
            finished: 1_767_226_200,
            completed: true,
            coverage: None,
        }];
        assert_eq!(one(all, &list(root)).tier, EvidenceTier::MetadataMatch);
    }

    #[test]
    fn several_matching_files_name_no_single_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for folder in ["a", "b"] {
            write(&root.join(folder).join("Game.rom"), &[1; 9000]);
        }
        let finding = one(inputs(record(root, "old/Game.rom", 9000, 1)), &list(root));
        assert_eq!(finding.tier, EvidenceTier::NameAndSize);
        assert_eq!(finding.target, None, "neither copy is called the file");
        assert!(
            finding
                .evidence
                .iter()
                .any(|line| line.starts_with("2 files have this name and size"))
        );
    }

    #[test]
    fn a_path_that_is_not_valid_text_never_stops_a_report() {
        use std::os::unix::ffi::OsStringExt;
        let odd = PathBuf::from(std::ffi::OsString::from_vec(
            b"/library/Game \xff.rom".to_vec(),
        ));
        let finding = RecordFinding {
            set: RecordSet::FlaggedMissing,
            archive_id: 1,
            old_path: odd.clone(),
            platform: None,
            archive_kind: "direct_game_image".into(),
            size_bytes: None,
            modified_unix_seconds: None,
            first_seen_at: None,
            last_seen_at: None,
            flagged_missing_at: None,
            tier: EvidenceTier::NoTrace,
            class: EvidenceClass::Unresolved,
            code: "T9",
            target: Some(odd),
            target_archive_id: None,
            evidence: Vec::new(),
        };
        let json = serde_json::to_string(&finding).expect("a report must always serialise");
        assert!(json.contains("/library/Game "), "{json}");
    }
}

mod two_records_one_file {
    use super::*;

    const AFTER: &str = "2099-06-01T00:00:00Z";
    /// A day after `LATER`, and the scan that ran then.
    const NEXT_DAY: &str = "2099-01-02T00:00:00Z";
    fn scan_next_day() -> ScanRun {
        ScanRun {
            started: LATER_UNIX + 86_400 - 60,
            finished: LATER_UNIX + 86_400 + 600,
            completed: true,
            coverage: None,
        }
    }

    /// One file on disk, and two library records with its name, size and date
    /// under two other folders. `second` shapes the second record.
    fn two_records(second: impl Fn(&mut OldRecord)) -> Vec<RecordFinding> {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let now = root.join("system/Example Game (World).rom");
        write(&now, &[7; 9000]);
        set_mtime(&now, 800_000_000);
        let first = record(root, "old-a/Example Game (World).rom", 9000, 800_000_000);
        let mut other = record(root, "old-b/Example Game (World).rom", 9000, 800_000_000);
        other.archive_id = 2;
        second(&mut other);
        let inputs = ReconcileInputs {
            old: vec![first, other],
            current: vec![CurrentRow {
                archive_id: 3,
                path: now,
                archive_kind: "direct_game_image".into(),
                first_seen_at: AFTER.into(),
                source_folder_id: 1,
            }],
            scan_runs: vec![scan_at_later(), scan_next_day()],
            ..ReconcileInputs::default()
        };
        classify(&inputs, &list(root))
    }

    #[test]
    fn two_records_that_were_in_the_library_together_cannot_both_be_the_same_file() {
        // Both records were last seen by the same scan: two files, side by side.
        let findings = two_records(|_| {});
        assert_eq!(findings.len(), 2);
        for finding in &findings {
            assert_eq!(finding.tier, EvidenceTier::MetadataMatch, "{finding:?}");
            assert_eq!(finding.class, EvidenceClass::Metadata);
            assert_eq!(finding.code, "T4");
            assert!(finding.target.is_some(), "the record keeps its file");
            assert!(
                finding.evidence[0].contains("at most one of them is this file"),
                "{:?}",
                finding.evidence
            );
        }
    }

    #[test]
    fn a_record_alone_for_its_file_stays_strong() {
        // The control for the test above: the same record, with no twin.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let now = root.join("system/Example Game (World).rom");
        write(&now, &[7; 9000]);
        set_mtime(&now, 800_000_000);
        let mut all = inputs(record(
            root,
            "old-a/Example Game (World).rom",
            9000,
            800_000_000,
        ));
        all.current = vec![CurrentRow {
            archive_id: 3,
            path: now,
            archive_kind: "direct_game_image".into(),
            first_seen_at: AFTER.into(),
            source_folder_id: 1,
        }];
        all.scan_runs = vec![scan_at_later()];
        assert_eq!(one(all, &list(root)).tier, EvidenceTier::SameFileObject);
    }

    #[test]
    fn one_file_moved_twice_keeps_both_of_its_earlier_names_strong() {
        // The second record appears only in the next day's scan, after the
        // first was last seen: one file under two names, one after the other.
        let findings = two_records(|other| {
            other.first_seen_at = Some(NEXT_DAY.into());
            other.last_seen_at = Some(NEXT_DAY.into());
            other.flagged_missing_at = Some(AFTER.into());
        });
        for finding in &findings {
            assert_eq!(finding.tier, EvidenceTier::SameFileObject, "{finding:?}");
        }
    }

    #[test]
    fn records_seen_at_different_seconds_of_one_scan_were_together() {
        // The second record is first and last seen ten minutes into the scan
        // that last saw the first one. Their stamps do not touch, but one scan
        // saw both, so they existed side by side.
        let findings = two_records(|other| {
            other.first_seen_at = Some("2099-01-01T00:09:00Z".into());
            other.last_seen_at = Some("2099-01-01T00:09:00Z".into());
        });
        for finding in &findings {
            assert_eq!(finding.tier, EvidenceTier::MetadataMatch, "{finding:?}");
        }
    }

    #[test]
    fn a_record_with_no_readable_first_sighting_counts_as_overlapping() {
        let findings = two_records(|other| {
            other.first_seen_at = None;
            other.last_seen_at = Some(NEXT_DAY.into());
        });
        for finding in &findings {
            assert_eq!(finding.tier, EvidenceTier::MetadataMatch, "{finding:?}");
        }
    }
}

mod rename_history {
    use super::*;

    /// A real file at `name` with the given date, and the journal entry that
    /// moved a file from `from` to it at `applied`.
    fn moved(root: &Path, from: &str, name: &str, applied: i64) -> (PathBuf, JournalMove) {
        let now = root.join(name);
        write(&now, &[8; 9000]);
        set_mtime(&now, 5);
        let metadata = fs::metadata(&now).unwrap();
        let entry = JournalMove {
            from: root.join(from),
            to: now.clone(),
            inode: metadata.ino(),
            device: metadata.dev(),
            size: 9000,
            modified: 5,
            applied_at: Some(applied),
        };
        (now, entry)
    }

    fn finding(root: &Path, record_at: &str, moves: Vec<JournalMove>) -> RecordFinding {
        let mut all = inputs(record(root, record_at, 9000, 5));
        all.moves = moves;
        one(all, &list(root))
    }

    #[test]
    fn a_different_file_at_the_place_emuwiz_moved_it_to_is_not_the_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (now, mut entry) = moved(root, "carts/Old.zip", "carts/New.zip", AFTER_LATER);
        // Same name, size and date at the destination, but not the inode the
        // journal recorded: it is some other file.
        entry.inode += 1_000_000;
        let found = finding(root, "carts/Old.zip", vec![entry]);
        assert_eq!(found.tier, EvidenceTier::SizeOnlyCandidate);
        assert_eq!(found.target.as_deref(), Some(now.as_path()));
        assert!(
            found
                .evidence
                .iter()
                .any(|line| line.contains("not the same file object"))
        );
    }

    #[test]
    fn a_file_created_after_the_move_has_a_reused_inode_number() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // The journal says the move happened in 2001. The file with that inode
        // number, size and date was created today: it cannot be the file moved.
        let (_, entry) = moved(root, "carts/Old.zip", "carts/New.zip", 1_000_000_000);
        let mut all = inputs(record(root, "carts/Old.zip", 9000, 5));
        all.old[0].last_seen_at = Some("2000-01-01T00:00:00Z".into());
        all.moves = vec![entry];
        let inventory = list(root);
        assert!(
            inventory.entries.iter().any(|entry| entry.born.is_some()),
            "this test needs a filesystem that records creation times"
        );
        let found = one(all, &inventory);
        assert_ne!(found.class, EvidenceClass::Confirmed, "{found:?}");
        assert!(
            found
                .evidence
                .iter()
                .any(|line| line.contains("created after the move"))
        );
    }

    #[test]
    fn the_move_that_counts_is_the_first_one_after_the_record_was_last_seen() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // Two moves from the same path. The first happened before this record
        // was last seen, so it moved an earlier file; the second moved this one.
        let (_, earlier) = moved(
            root,
            "carts/Old.zip",
            "carts/Earlier.zip",
            LATER_UNIX - 1000,
        );
        let (now, later) = moved(root, "carts/Old.zip", "carts/New.zip", AFTER_LATER);
        let both = finding(root, "carts/Old.zip", vec![earlier.clone(), later.clone()]);
        assert_eq!(both.tier, EvidenceTier::ConfirmedObject);
        assert_eq!(both.target.as_deref(), Some(now.as_path()));
        // Whatever order the journals are read in.
        let reversed = finding(root, "carts/Old.zip", vec![later, earlier.clone()]);
        assert_eq!(reversed.target, both.target);
        // With only the earlier move, nothing in the history is about this record.
        let only_earlier = finding(root, "carts/Old.zip", vec![earlier]);
        assert_ne!(only_earlier.class, EvidenceClass::Confirmed);
        assert!(
            only_earlier
                .evidence
                .iter()
                .any(|line| line.contains("moved an earlier file at this path"))
        );
    }

    #[test]
    fn later_moves_are_followed_only_for_the_same_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // Old -> Middle -> Final, all the same file: confirmed at Final.
        let (now, last_hop) = moved(
            root,
            "carts/Middle.zip",
            "carts/Final.zip",
            AFTER_LATER + 100,
        );
        let first_hop = JournalMove {
            from: root.join("carts/Old.zip"),
            to: root.join("carts/Middle.zip"),
            applied_at: Some(AFTER_LATER),
            ..last_hop.clone()
        };
        let chain = finding(
            root,
            "carts/Old.zip",
            vec![first_hop.clone(), last_hop.clone()],
        );
        assert_eq!(chain.tier, EvidenceTier::ConfirmedObject);
        assert_eq!(chain.target.as_deref(), Some(now.as_path()));
        assert!(chain.evidence.iter().any(|line| line.contains("2 move(s)")));
        // A later move from Middle of some OTHER file is not followed, and does
        // not spoil the first hop: the file is still found, by its inode.
        let other = JournalMove {
            inode: last_hop.inode + 1_000_000,
            to: root.join("carts/Unrelated.zip"),
            ..last_hop
        };
        let unrelated = finding(root, "carts/Old.zip", vec![first_hop, other]);
        assert_eq!(unrelated.tier, EvidenceTier::ConfirmedObject);
        assert_eq!(unrelated.target.as_deref(), Some(now.as_path()));
        assert!(
            unrelated
                .evidence
                .iter()
                .any(|line| line.contains("1 move(s)")),
            "the other file's move is not counted as this file's: {:?}",
            unrelated.evidence
        );
        assert!(
            unrelated
                .evidence
                .iter()
                .any(|line| line.contains("found under a later name"))
        );
    }

    #[test]
    fn of_two_moves_after_the_last_sighting_the_earlier_one_is_this_records() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // The path was reused: EmuWiz moved this record's file away, a new
        // file took the path and was moved away later. Only the first move
        // after the record was last seen is about the record.
        let (first_now, first) = moved(root, "carts/Old.zip", "carts/First.zip", AFTER_LATER);
        let (_, second) = moved(root, "carts/Old.zip", "carts/Second.zip", AFTER_LATER + 500);
        for order in [vec![first.clone(), second.clone()], vec![second, first]] {
            let found = finding(root, "carts/Old.zip", order);
            assert_eq!(found.tier, EvidenceTier::ConfirmedObject);
            assert_eq!(found.target.as_deref(), Some(first_now.as_path()));
        }
    }

    #[test]
    fn without_a_creation_time_a_reused_inode_cannot_be_ruled_out() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (now, entry) = moved(root, "carts/Old.zip", "carts/New.zip", AFTER_LATER);
        // The same listing, as a filesystem that records no creation times gives it.
        let listed = list(root);
        let entries = listed
            .entries
            .iter()
            .cloned()
            .map(|mut entry| {
                entry.born = None;
                entry
            })
            .collect();
        let inventory = Inventory::from_entries(entries, listed.summary.roots.clone());
        let mut all = inputs(record(root, "carts/Old.zip", 9000, 5));
        all.moves = vec![entry];
        let found = one(all, &inventory);
        assert_eq!(found.tier, EvidenceTier::MetadataMatch, "{found:?}");
        assert_eq!(found.target.as_deref(), Some(now.as_path()));
        assert!(
            found
                .evidence
                .iter()
                .any(|line| line.contains("does not record when a file was created"))
        );
    }

    #[test]
    fn a_folder_where_a_file_was_recorded_is_not_the_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("carts/Game.rom")).unwrap();
        write(&root.join("carts/Game.rom/inside.bin"), &[1; 10]);
        let found = one(inputs(record(root, "carts/Game.rom", 9000, 5)), &list(root));
        assert_ne!(found.tier, EvidenceTier::PresentAtRecordedPath, "{found:?}");
        assert!(
            found
                .evidence
                .iter()
                .any(|line| line.contains("something else now sits at the recorded place"))
        );
    }

    #[test]
    fn a_record_whose_last_sighting_cannot_be_read_is_never_confirmed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (_, entry) = moved(root, "carts/Old.zip", "carts/New.zip", AFTER_LATER);
        for last_seen in [None, Some("not a date".to_string()), Some(String::new())] {
            let mut all = inputs(record(root, "carts/Old.zip", 9000, 5));
            all.old[0].last_seen_at = last_seen.clone();
            all.moves = vec![entry.clone()];
            let found = one(all, &list(root));
            assert_ne!(found.class, EvidenceClass::Confirmed, "{last_seen:?}");
            assert!(
                found
                    .evidence
                    .iter()
                    .any(|line| line.contains("when this record was last seen cannot be read")),
                "{last_seen:?}: {:?}",
                found.evidence
            );
        }
    }

    #[test]
    fn a_path_reached_through_a_link_is_not_the_recorded_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        let elsewhere = temp.path().join("elsewhere/system-a");
        write(&elsewhere.join("Game.rom"), &[1; 9000]);
        fs::create_dir_all(&root).unwrap();
        symlink(&elsewhere, root.join("system-a")).unwrap();
        // The path resolves to a regular file, but only through a link.
        assert!(
            fs::symlink_metadata(root.join("system-a/Game.rom"))
                .unwrap()
                .is_file()
        );
        let found = one(
            inputs(record(&root, "system-a/Game.rom", 9000, 1)),
            &list(&root),
        );
        assert_ne!(found.tier, EvidenceTier::PresentAtRecordedPath);
        assert!(
            found
                .evidence
                .iter()
                .any(|line| line.contains("reached through a link"))
        );
    }

    #[test]
    fn two_broken_links_of_one_name_single_out_no_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let a = root.join("storage-a/Game.img");
        let b = root.join("storage-b/Game.img");
        write(&a, &[1; 9000]);
        write(&b, &[2; 9000]);
        write(&root.join("library/Other.rom"), &[3; 10]);
        let link = |folder: &str, found: &Path| LinkFinding {
            link: root.join(folder).join("Game.img"),
            stored_target: PathBuf::from("/gone/Game.img"),
            resolved_target: PathBuf::from("/gone/Game.img"),
            grade: LinkGrade::TargetIdentified,
            cause: Some(LinkCause::LocationGone),
            deepest_existing_ancestor: None,
            found: Some(found.to_path_buf()),
            found_inode: None,
            evidence: Vec::new(),
        };
        let inventory = list(&root.join("library"));
        let run = |links: &[LinkFinding]| {
            classify_records(
                &inputs(record(&root.join("library"), "old/Game.img", 9000, 1)),
                &inventory,
                links,
                None,
                &AtomicBool::new(false),
                &mut |_, _| {},
            )
            .pop()
            .unwrap()
        };
        // One identified target of the record's size: a weak tie to it.
        let one_link = run(&[link("x", &a)]);
        assert_eq!(one_link.tier, EvidenceTier::NameAndSizeViaLink);
        assert_eq!(one_link.target.as_deref(), Some(a.as_path()));
        // Two different targets: neither is called the file.
        let two_links = run(&[link("x", &a), link("y", &b)]);
        assert_eq!(two_links.tier, EvidenceTier::NoTrace);
        assert_eq!(two_links.target, None);
        assert!(
            two_links
                .evidence
                .iter()
                .any(|line| line.contains("none is singled out"))
        );
    }
}

mod links_are_graded_on_what_was_seen {
    use super::*;

    #[test]
    fn a_file_that_already_sat_beside_the_link_is_not_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let games = temp.path().join("games");
        let folder = games.join("collection");
        // The file is there first; the link, pointing somewhere else, is made later.
        write(
            &folder.join("Game (Publisher)/Game [Disk 1].zip"),
            &[1; 5000],
        );
        std::thread::sleep(Duration::from_millis(1100));
        symlink(
            "/nonexistent-old-storage/Collection/Game (Publisher)/Game [Disk 1].zip",
            folder.join("Game [Disk 1].zip"),
        )
        .unwrap();
        let (findings, summary) = audit_links(&list(&games), &AtomicBool::new(false));
        assert_eq!(
            (summary.target_identified, summary.plausible_candidate),
            (0, 1)
        );
        assert!(
            findings[0]
                .evidence
                .iter()
                .any(|line| line.contains("does not show that the link pointed at it")),
            "{:?}",
            findings[0].evidence
        );
        assert!(
            findings[0]
                .evidence
                .iter()
                .all(|line| !line.contains("later copy")
                    && !line.contains("not what the link pointed at")),
            "nothing is claimed about where the file used to be: {:?}",
            findings[0].evidence
        );
    }

    #[test]
    fn in_a_moved_folder_a_file_newer_than_the_link_is_only_a_later_copy() {
        let temp = tempfile::tempdir().unwrap();
        let games = temp.path().join("games");
        let storage = temp.path().join("storage");
        // The link names storage/incoming/staging/Collection/Game.zip. Only
        // storage/incoming/staging is left, empty. The missing part of the
        // path, Collection/Game.zip, exists two folders up: the shape a moved
        // folder leaves. But that file was written after the link was made.
        fs::create_dir_all(&games).unwrap();
        fs::create_dir_all(storage.join("incoming/staging")).unwrap();
        symlink(
            storage.join("incoming/staging/Collection/Game.zip"),
            games.join("Game.zip"),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        write(&storage.join("Collection/Game.zip"), &[1; 5000]);
        let (findings, summary) = audit_links(&list(&games), &AtomicBool::new(false));
        assert_eq!(
            (summary.target_identified, summary.plausible_candidate),
            (0, 1),
            "{findings:?}"
        );
        assert!(
            findings[0]
                .evidence
                .iter()
                .any(|line| line.contains("created after the link was made"))
        );
    }
}

mod the_listing {
    use super::*;

    #[test]
    fn a_limit_makes_it_incomplete_and_cancel_stops_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for n in 0..6 {
            write(&root.join(format!("d{n}/Game.rom")), &[1; 10]);
        }
        let limited = take_inventory(
            &InventoryRequest {
                roots: &[root.to_path_buf()],
                max_entries: 3,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert!(!limited.summary.complete, "a listing cut short must say so");
        assert!(
            limited.summary.roots[0]
                .errors
                .iter()
                .any(|error| error.contains("limit"))
        );
        let cancelled = take_inventory(
            &InventoryRequest {
                roots: &[root.to_path_buf()],
                max_entries: DEFAULT_MAX_ENTRIES,
            },
            &AtomicBool::new(true),
            &mut |_| {},
        );
        assert_eq!(cancelled.unwrap_err(), InventoryError::Cancelled);
    }

    #[test]
    fn a_file_rewritten_in_place_is_seen_by_the_next_listing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        let game = root.join("system-a/Game.rom");
        write(&game, &[1; 100]);
        assert_eq!(list(&root).get(&game).unwrap().size, 100);
        // Growing a file does not touch its folder. Nothing is kept between
        // listings, so the new size is seen all the same.
        let folder_before = fs::metadata(root.join("system-a")).unwrap().mtime();
        fs::OpenOptions::new()
            .append(true)
            .open(&game)
            .unwrap()
            .set_len(250)
            .unwrap();
        assert_eq!(
            fs::metadata(root.join("system-a")).unwrap().mtime(),
            folder_before
        );
        assert_eq!(list(&root).get(&game).unwrap().size, 250);
    }

    #[test]
    fn a_link_back_into_the_library_never_lists_its_games_twice_or_instead() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        write(&root.join("megadrive/Game.smd"), &[1; 10]);
        // An alias for a real folder, a link to the library itself and one to its parent.
        symlink(root.join("megadrive"), root.join("alias")).unwrap();
        symlink(&root, root.join("self")).unwrap();
        symlink(temp.path(), root.join("parent")).unwrap();
        write(&temp.path().join("outside/Other.smd"), &[1; 10]);
        let inventory = list(&root);
        assert!(inventory.summary.complete);
        assert!(
            inventory.get(&root.join("megadrive/Game.smd")).is_some(),
            "the real path is listed"
        );
        assert!(
            inventory.get(&root.join("alias/Game.smd")).is_none(),
            "and only once"
        );
        assert!(
            inventory
                .get(&root.join("self/megadrive/Game.smd"))
                .is_none()
        );
        assert!(
            inventory
                .get(&root.join("parent/games/megadrive/Game.smd"))
                .is_none()
        );
        assert!(
            inventory
                .get(&root.join("parent/outside/Other.smd"))
                .is_none(),
            "a link to a folder above the library is not followed"
        );
        assert_eq!(inventory.summary.roots[0].files, 1);
        assert_eq!(inventory.summary.roots[0].links, 3);
    }

    #[test]
    fn emuwiz_s_private_recovery_folders_are_not_game_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("system-a/Game.rom"), &[1; 10]);
        write(&root.join(".emuwiz-r2-holding/tx/Game.rom"), &[1; 10]);
        let inventory = list(root);
        assert!(inventory.get(&root.join("system-a/Game.rom")).is_some());
        assert!(
            inventory
                .entries
                .iter()
                .all(|entry| !entry.path.to_string_lossy().contains(".emuwiz-r2-holding")),
            "a held original must never be offered as where a game went"
        );
    }
}
