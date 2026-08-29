use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::dat::rename_apply::model::TransactionState;
use crate::repair::quarantine::{
    apply_quarantine_transaction, build_quarantine_transaction, rollback_quarantine_transaction,
};

fn trusted_for(dir: &std::path::Path) -> TrustedRoots {
    TrustedRoots::from_paths([dir])
}

// --- 1: identical size+hash form one group ---------------------------------

#[test]
fn two_files_with_identical_size_and_hash_form_one_exact_group() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a.bin");
    let b = temp.path().join("b.bin");
    std::fs::write(&a, b"identical content").unwrap();
    std::fs::write(&b, b"identical content").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        &[a.clone(), b.clone()],
        &trusted,
        &[],
        &BTreeSet::new(),
        None,
    );

    assert_eq!(report.groups.len(), 1, "{:?}", report.groups);
    assert_eq!(report.groups[0].members.len(), 2);
    assert_eq!(report.groups[0].size_bytes, 17);
    assert!(report.excluded.is_empty());
}

// --- 2: same size, different hash never groups ------------------------------

#[test]
fn same_size_different_hash_does_not_group() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a.bin");
    let b = temp.path().join("b.bin");
    std::fs::write(&a, b"aaaaaaaaaaaaaaaaaa").unwrap();
    std::fs::write(&b, b"bbbbbbbbbbbbbbbbbb").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(&[a, b], &trusted, &[], &BTreeSet::new(), None);

    assert!(report.groups.is_empty(), "{:?}", report.groups);
}

// --- 3: same DAT identity, different bytes never groups ---------------------
//
// This module never even looks at DAT identity - grouping is purely by
// full-file hash - so two files that a DAT audit would call the "same
// game/rom" but whose bytes differ can never land in one exact group
// regardless of what any catalogue says.

#[test]
fn same_dat_identity_different_bytes_does_not_group() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("Sonic (Europe) [corrupt-copy-1].bin");
    let b = temp.path().join("Sonic (Europe) [corrupt-copy-2].bin");
    std::fs::write(&a, b"corrupt bytes one!").unwrap();
    std::fs::write(&b, b"corrupt bytes two!").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(&[a, b], &trusted, &[], &BTreeSet::new(), None);

    assert!(report.groups.is_empty(), "{:?}", report.groups);
}

// --- 4: ZIP and its matching loose member never group -----------------------

#[test]
fn a_zip_and_its_matching_loose_member_do_not_group_as_exact_files() {
    let temp = tempfile::tempdir().unwrap();
    let loose = temp.path().join("game.bin");
    std::fs::write(&loose, b"the-rom-bytes").unwrap();
    // A trivial stand-in "zip": in reality this would be a real ZIP
    // container, but the point under test is purely that the *outer
    // file's own bytes* (whatever they are) never equal the inner
    // member's bytes byte-for-byte, so this module's pure full-file hash
    // comparison can never conflate them - no ZIP-specific code path
    // exists in this module at all.
    let zip = temp.path().join("game.zip");
    std::fs::write(&zip, b"PK\x03\x04-not-the-same-bytes-as-the-loose-member").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(&[loose, zip], &trusted, &[], &BTreeSet::new(), None);

    assert!(report.groups.is_empty(), "{:?}", report.groups);
}

// --- 5: N64 byte order variants never group ---------------------------------

#[test]
fn n64_alternate_byte_orders_are_not_exact_duplicates() {
    let temp = tempfile::tempdir().unwrap();
    let big_endian = temp.path().join("game.z64");
    let byte_swapped = temp.path().join("game.v64");
    std::fs::write(&big_endian, [0x80, 0x37, 0x12, 0x40]).unwrap();
    std::fs::write(&byte_swapped, [0x37, 0x80, 0x40, 0x12]).unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        &[big_endian, byte_swapped],
        &trusted,
        &[],
        &BTreeSet::new(),
        None,
    );

    assert!(report.groups.is_empty(), "{:?}", report.groups);
}

// --- 6: NES headered/unheadered never group ---------------------------------

#[test]
fn headered_and_unheadered_nes_are_not_exact_duplicates() {
    let temp = tempfile::tempdir().unwrap();
    let headered = temp.path().join("game.nes");
    let unheadered = temp.path().join("game.unheadered.nes");
    let mut headered_bytes = vec![0x4E, 0x45, 0x53, 0x1A, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let body = vec![0xAA; 32];
    headered_bytes.extend_from_slice(&body);
    std::fs::write(&headered, &headered_bytes).unwrap();
    std::fs::write(&unheadered, &body).unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        &[headered, unheadered],
        &trusted,
        &[],
        &BTreeSet::new(),
        None,
    );

    assert!(report.groups.is_empty(), "{:?}", report.groups);
}

// --- 7: deterministic retained-copy recommendation uses trusted-root evidence

#[test]
fn deterministic_retained_copy_uses_trusted_root_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let trusted_copy = temp.path().join("trusted").join("game.bin");
    let other_copy = temp.path().join("elsewhere").join("game.bin");
    std::fs::create_dir_all(trusted_copy.parent().unwrap()).unwrap();
    std::fs::create_dir_all(other_copy.parent().unwrap()).unwrap();
    std::fs::write(&trusted_copy, b"same bytes everywhere").unwrap();
    std::fs::write(&other_copy, b"same bytes everywhere").unwrap();
    let trusted = trusted_for(temp.path());
    let trusted_root = temp.path().join("trusted");

    let report = scan_exact_duplicates(
        &[trusted_copy.clone(), other_copy.clone()],
        &trusted,
        &[trusted_root],
        &BTreeSet::new(),
        None,
    );

    assert_eq!(report.groups.len(), 1);
    assert_eq!(
        report.groups[0].recommendation,
        CanonicalRecommendation::TrustedRoot(trusted_copy.clone())
    );
    assert_eq!(report.groups[0].redundant_paths, vec![other_copy]);
    assert_eq!(report.groups[0].readiness, GroupQuarantineReadiness::Safe);
}

// --- 8: no trusted/elected distinction requires user choice -----------------

#[test]
fn no_trusted_or_elected_distinction_requires_user_choice_not_path_sorting() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("aaa.bin");
    let z = temp.path().join("zzz.bin");
    std::fs::write(&a, b"tied bytes, no evidence").unwrap();
    std::fs::write(&z, b"tied bytes, no evidence").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(&[a, z], &trusted, &[], &BTreeSet::new(), None);

    assert_eq!(report.groups.len(), 1);
    assert_eq!(
        report.groups[0].recommendation,
        CanonicalRecommendation::RequiresUserChoice
    );
    assert!(report.groups[0].redundant_paths.is_empty());
    assert!(matches!(
        report.groups[0].readiness,
        GroupQuarantineReadiness::NeedsReview(_)
    ));
}

// --- 9/10/11: multi-file companion protection -------------------------------

#[test]
fn a_cue_companion_cannot_be_quarantined_independently() {
    let temp = tempfile::tempdir().unwrap();
    // Release A: cue + bin, kept in place (not deduplicated away).
    let bin_a = temp.path().join("a.bin");
    std::fs::write(&bin_a, b"disc-bytes").unwrap();
    let cue_a = temp.path().join("a.cue");
    std::fs::write(&cue_a, "FILE \"a.bin\" BINARY\n").unwrap();
    // A trusted-root duplicate of just the BIN's bytes, unowned by any
    // launcher - trusted-root evidence would otherwise make this the
    // recommended retained copy, which would pick `bin_a` (the CUE's own
    // companion) as redundant. That must be blocked, not silently allowed.
    let trusted_dir = temp.path().join("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let bin_dup = trusted_dir.join("standalone-copy.bin");
    std::fs::write(&bin_dup, b"disc-bytes").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        &[bin_a.clone(), cue_a, bin_dup],
        &trusted,
        &[trusted_dir],
        &BTreeSet::new(),
        None,
    );

    let group = report
        .groups
        .iter()
        .find(|g| g.members.iter().any(|m| m.path == bin_a))
        .expect("bin group present");
    assert!(group.redundant_paths.contains(&bin_a));
    assert!(
        matches!(group.readiness, GroupQuarantineReadiness::Blocked(_)),
        "{:?}",
        group.readiness
    );
}

#[test]
fn a_gdi_track_cannot_be_quarantined_independently() {
    let temp = tempfile::tempdir().unwrap();
    let track = temp.path().join("track01.bin");
    std::fs::write(&track, b"gdi-track-bytes").unwrap();
    let gdi = temp.path().join("game.gdi");
    std::fs::write(&gdi, "1\n1 0 4 2352 track01.bin 0\n").unwrap();
    let trusted_dir = temp.path().join("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let track_dup = trusted_dir.join("track01-copy.bin");
    std::fs::write(&track_dup, b"gdi-track-bytes").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        &[track.clone(), gdi, track_dup],
        &trusted,
        &[trusted_dir],
        &BTreeSet::new(),
        None,
    );

    let group = report
        .groups
        .iter()
        .find(|g| g.members.iter().any(|m| m.path == track))
        .expect("track group present");
    assert!(group.redundant_paths.contains(&track));
    assert!(
        matches!(group.readiness, GroupQuarantineReadiness::Blocked(_)),
        "{:?}",
        group.readiness
    );
}

#[test]
fn an_m3u_member_cannot_be_quarantined_independently() {
    let temp = tempfile::tempdir().unwrap();
    let disc1 = temp.path().join("disc1.chd");
    std::fs::write(&disc1, b"disc-one-bytes").unwrap();
    let m3u = temp.path().join("game.m3u");
    std::fs::write(&m3u, "disc1.chd\n").unwrap();
    let trusted_dir = temp.path().join("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let disc1_dup = trusted_dir.join("disc1-elsewhere.chd");
    std::fs::write(&disc1_dup, b"disc-one-bytes").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        &[disc1.clone(), m3u, disc1_dup],
        &trusted,
        &[trusted_dir],
        &BTreeSet::new(),
        None,
    );

    let group = report
        .groups
        .iter()
        .find(|g| g.members.iter().any(|m| m.path == disc1))
        .expect("disc group present");
    assert!(group.redundant_paths.contains(&disc1));
    assert!(
        matches!(group.readiness, GroupQuarantineReadiness::Blocked(_)),
        "{:?}",
        group.readiness
    );
}

// --- 12: ambiguous shared companion blocks automatic quarantine -------------

#[test]
fn an_ambiguous_shared_companion_blocks_automatic_quarantine() {
    let temp = tempfile::tempdir().unwrap();
    let shared = temp.path().join("shared_track.bin");
    std::fs::write(&shared, b"shared-track-bytes").unwrap();
    let cue1 = temp.path().join("one.cue");
    std::fs::write(&cue1, "FILE \"shared_track.bin\" BINARY\n").unwrap();
    let cue2 = temp.path().join("two.cue");
    std::fs::write(&cue2, "FILE \"shared_track.bin\" BINARY\n").unwrap();
    let trusted_dir = temp.path().join("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let shared_dup = trusted_dir.join("shared_track_copy.bin");
    std::fs::write(&shared_dup, b"shared-track-bytes").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        &[shared.clone(), cue1, cue2, shared_dup],
        &trusted,
        &[trusted_dir],
        &BTreeSet::new(),
        None,
    );

    let group = report
        .groups
        .iter()
        .find(|g| g.members.iter().any(|m| m.path == shared))
        .expect("shared-track group present");
    assert!(group.redundant_paths.contains(&shared));
    assert!(
        matches!(group.readiness, GroupQuarantineReadiness::Blocked(_)),
        "{:?}",
        group.readiness
    );
}

// --- 13: preview reports exact reclaimable bytes without double counting ---

#[test]
fn reclaimable_bytes_are_exact_and_never_double_counted() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a.bin");
    let b = temp.path().join("b.bin");
    let c = temp.path().join("c.bin");
    std::fs::write(&a, [1u8; 100]).unwrap();
    std::fs::write(&b, [1u8; 100]).unwrap();
    std::fs::write(&c, [1u8; 100]).unwrap();
    let trusted = trusted_for(temp.path());
    let trusted_root = temp.path().to_path_buf();

    let report = scan_exact_duplicates(
        &[a, b, c],
        &trusted,
        &[trusted_root],
        &BTreeSet::new(),
        None,
    );

    assert_eq!(report.groups.len(), 1);
    // All three are inside the same trusted root, so trusted-root evidence
    // does not distinguish a unique one - user choice required, zero
    // redundant paths, zero reclaimable bytes counted for this group.
    assert_eq!(report.groups[0].reclaimable_bytes, 0);
    assert_eq!(report.total_reclaimable_bytes(), 0);
}

#[test]
fn reclaimable_bytes_count_each_redundant_file_exactly_once_when_resolved() {
    let temp = tempfile::tempdir().unwrap();
    let trusted_copy = temp.path().join("trusted").join("a.bin");
    let other1 = temp.path().join("other1.bin");
    let other2 = temp.path().join("other2.bin");
    std::fs::create_dir_all(trusted_copy.parent().unwrap()).unwrap();
    std::fs::write(&trusted_copy, [7u8; 50]).unwrap();
    std::fs::write(&other1, [7u8; 50]).unwrap();
    std::fs::write(&other2, [7u8; 50]).unwrap();
    let trusted = trusted_for(temp.path());
    let trusted_root = temp.path().join("trusted");

    let report = scan_exact_duplicates(
        &[trusted_copy, other1, other2],
        &trusted,
        &[trusted_root],
        &BTreeSet::new(),
        None,
    );

    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].reclaimable_bytes, 100);
    assert_eq!(report.total_reclaimable_bytes(), 100);
}

// --- 14/15/16/17/18: apply/collision/rollback/idempotent reapply, via the
// existing quarantine transaction engine unchanged ---------------------------

struct LiveGroup {
    temp: tempfile::TempDir,
    trusted_root: std::path::PathBuf,
    retained: std::path::PathBuf,
    redundant: std::path::PathBuf,
    unrelated: std::path::PathBuf,
}

fn live_two_copy_group() -> LiveGroup {
    let temp = tempfile::tempdir().unwrap();
    let trusted_dir = temp.path().join("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let retained = trusted_dir.join("game.bin");
    let redundant = temp.path().join("elsewhere").join("game.bin");
    std::fs::create_dir_all(redundant.parent().unwrap()).unwrap();
    std::fs::write(&retained, b"kept content, byte for byte").unwrap();
    std::fs::write(&redundant, b"kept content, byte for byte").unwrap();
    let unrelated = temp.path().join("elsewhere").join("unrelated.bin");
    std::fs::write(&unrelated, b"totally different file").unwrap();
    LiveGroup {
        temp,
        trusted_root: trusted_dir,
        retained,
        redundant,
        unrelated,
    }
}

#[test]
fn apply_moves_only_approved_redundant_copies() {
    let live = live_two_copy_group();
    let trusted_roots = TrustedRoots::from_paths([live.temp.path()]);
    let mut cache = DuplicateHashCache::new();
    let report = scan_exact_duplicates(
        &[live.retained.clone(), live.redundant.clone()],
        &trusted_roots,
        std::slice::from_ref(&live.trusted_root),
        &BTreeSet::new(),
        None,
    );
    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.readiness, GroupQuarantineReadiness::Safe);

    let proposals = build_exact_duplicate_group_proposals(
        group,
        &live.trusted_root,
        &mut cache,
        &trusted_roots,
        None,
    )
    .expect("proposals");
    assert_eq!(proposals.len(), 1);

    let mut transaction = build_quarantine_transaction(
        &proposals,
        &live.retained,
        &live.trusted_root,
        0,
        &mut cache,
        &trusted_roots,
        None,
    )
    .expect("transaction");

    let journal_dir = live.temp.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let cancel = AtomicBool::new(false);
    apply_quarantine_transaction(
        &mut transaction,
        &live.retained,
        &live.trusted_root,
        0,
        trusted_roots.clone(),
        &journal_dir,
        &cancel,
        &mut cache,
    )
    .expect("apply");

    assert!(live.retained.exists(), "retained copy stays in place");
    assert!(!live.redundant.exists(), "redundant copy was moved");
    assert!(live.unrelated.exists(), "unrelated file untouched");
    assert_eq!(transaction.state, TransactionState::Applied);
}

#[test]
fn an_existing_destination_collision_blocks_the_transaction() {
    let live = live_two_copy_group();
    let trusted_roots = TrustedRoots::from_paths([live.temp.path()]);
    let mut cache = DuplicateHashCache::new();
    let report = scan_exact_duplicates(
        &[live.retained.clone(), live.redundant.clone()],
        &trusted_roots,
        std::slice::from_ref(&live.trusted_root),
        &BTreeSet::new(),
        None,
    );
    let group = &report.groups[0];
    let proposals = build_exact_duplicate_group_proposals(
        group,
        &live.trusted_root,
        &mut cache,
        &trusted_roots,
        None,
    )
    .unwrap();
    let mut transaction = build_quarantine_transaction(
        &proposals,
        &live.retained,
        &live.trusted_root,
        0,
        &mut cache,
        &trusted_roots,
        None,
    )
    .unwrap();

    // Pre-create the destination so the move collides.
    let destination = transaction.entries[0].destination_path.clone();
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(&destination, b"already occupied").unwrap();

    let journal_dir = live.temp.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let cancel = AtomicBool::new(false);
    let result = apply_quarantine_transaction(
        &mut transaction,
        &live.retained,
        &live.trusted_root,
        0,
        trusted_roots.clone(),
        &journal_dir,
        &cancel,
        &mut cache,
    );

    assert!(result.is_err(), "collision must refuse the transaction");
    assert!(
        live.redundant.exists(),
        "source must remain untouched on refusal"
    );
}

#[test]
fn an_induced_failure_rolls_back_every_move_in_the_batch() {
    let temp = tempfile::tempdir().unwrap();
    let trusted_dir = temp.path().join("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let retained = trusted_dir.join("game.bin");
    std::fs::write(&retained, b"content shared by three copies!").unwrap();
    let redundant1 = temp.path().join("r1").join("game.bin");
    let redundant2 = temp.path().join("r2").join("game.bin");
    std::fs::create_dir_all(redundant1.parent().unwrap()).unwrap();
    std::fs::create_dir_all(redundant2.parent().unwrap()).unwrap();
    std::fs::write(&redundant1, b"content shared by three copies!").unwrap();
    std::fs::write(&redundant2, b"content shared by three copies!").unwrap();
    let trusted_roots = TrustedRoots::from_paths([temp.path()]);
    let mut cache = DuplicateHashCache::new();

    let report = scan_exact_duplicates(
        &[retained.clone(), redundant1.clone(), redundant2.clone()],
        &trusted_roots,
        std::slice::from_ref(&trusted_dir),
        &BTreeSet::new(),
        None,
    );
    let group = &report.groups[0];
    let proposals = build_exact_duplicate_group_proposals(
        group,
        &trusted_dir,
        &mut cache,
        &trusted_roots,
        None,
    )
    .unwrap();
    let mut transaction = build_quarantine_transaction(
        &proposals,
        &retained,
        &trusted_dir,
        0,
        &mut cache,
        &trusted_roots,
        None,
    )
    .unwrap();

    // Induce a failure on the second entry by removing its source before
    // apply reaches it - the second entry's own live re-proof will refuse.
    let second_entry_source = transaction.entries[1].source_path.clone();
    std::fs::remove_file(&second_entry_source).unwrap();

    let journal_dir = temp.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let cancel = AtomicBool::new(false);
    let apply_result = apply_quarantine_transaction(
        &mut transaction,
        &retained,
        &trusted_dir,
        0,
        trusted_roots.clone(),
        &journal_dir,
        &cancel,
        &mut cache,
    );
    // The apply loop stops at the failing entry rather than erroring the
    // whole call; the first entry may already be Applied.
    let _ = apply_result;

    let rollback =
        rollback_quarantine_transaction(&mut transaction, &journal_dir, &cancel, &trusted_dir)
            .expect("rollback");
    let _ = rollback;

    assert!(retained.exists(), "retained copy untouched throughout");
    assert!(
        redundant1.exists(),
        "first entry restored to its original path"
    );
}

#[test]
fn reapplying_an_identical_plan_is_idempotent() {
    let live = live_two_copy_group();
    let trusted_roots = TrustedRoots::from_paths([live.temp.path()]);
    let mut cache = DuplicateHashCache::new();
    let report = scan_exact_duplicates(
        &[live.retained.clone(), live.redundant.clone()],
        &trusted_roots,
        std::slice::from_ref(&live.trusted_root),
        &BTreeSet::new(),
        None,
    );
    let group = &report.groups[0];
    let proposals = build_exact_duplicate_group_proposals(
        group,
        &live.trusted_root,
        &mut cache,
        &trusted_roots,
        None,
    )
    .unwrap();
    let mut transaction = build_quarantine_transaction(
        &proposals,
        &live.retained,
        &live.trusted_root,
        0,
        &mut cache,
        &trusted_roots,
        None,
    )
    .unwrap();
    let journal_dir = live.temp.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let cancel = AtomicBool::new(false);
    apply_quarantine_transaction(
        &mut transaction,
        &live.retained,
        &live.trusted_root,
        0,
        trusted_roots.clone(),
        &journal_dir,
        &cancel,
        &mut cache,
    )
    .unwrap();
    let after_first = std::fs::read(&transaction.entries[0].destination_path).unwrap();

    // Re-running the exact same scan now (source already moved) must not
    // find a new redundant copy, and must not disturb the quarantined file.
    let rescanned = scan_exact_duplicates(
        std::slice::from_ref(&live.retained),
        &trusted_roots,
        std::slice::from_ref(&live.trusted_root),
        &BTreeSet::new(),
        None,
    );
    assert!(
        rescanned.groups.is_empty(),
        "only one copy remains; nothing to group"
    );
    let after_rescan = std::fs::read(&transaction.entries[0].destination_path).unwrap();
    assert_eq!(after_first, after_rescan);
}

// --- 20: existing unrelated source files remain untouched -------------------

#[test]
fn unrelated_source_files_are_never_touched_by_a_scan() {
    let live = live_two_copy_group();
    let trusted_roots = TrustedRoots::from_paths([live.temp.path()]);
    let before = std::fs::read(&live.unrelated).unwrap();

    let _report = scan_exact_duplicates(
        &[
            live.retained.clone(),
            live.redundant.clone(),
            live.unrelated.clone(),
        ],
        &trusted_roots,
        std::slice::from_ref(&live.trusted_root),
        &BTreeSet::new(),
        None,
    );

    let after = std::fs::read(&live.unrelated).unwrap();
    assert_eq!(before, after);
    assert!(live.unrelated.exists());
}

// --- excluded candidates are reported, never silently dropped ---------------

#[test]
fn a_missing_candidate_is_reported_as_excluded_not_silently_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("does-not-exist.bin");
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        std::slice::from_ref(&missing),
        &trusted,
        &[],
        &BTreeSet::new(),
        None,
    );

    assert!(report.groups.is_empty());
    assert_eq!(report.excluded.len(), 1);
    assert_eq!(report.excluded[0].path, missing);
}

// --- elected-library evidence, when trusted-root evidence does not decide --

#[test]
fn elected_library_membership_recommends_a_retained_copy_when_no_trusted_root_wins() {
    let temp = tempfile::tempdir().unwrap();
    let elected = temp.path().join("elected.bin");
    let other = temp.path().join("other.bin");
    std::fs::write(&elected, b"same bytes for both").unwrap();
    std::fs::write(&other, b"same bytes for both").unwrap();
    let trusted = trusted_for(temp.path());
    let mut elected_paths = BTreeSet::new();
    elected_paths.insert(elected.clone());

    let report = scan_exact_duplicates(
        &[elected.clone(), other.clone()],
        &trusted,
        &[],
        &elected_paths,
        None,
    );

    assert_eq!(report.groups.len(), 1);
    assert_eq!(
        report.groups[0].recommendation,
        CanonicalRecommendation::ElectedLibrary(elected)
    );
    assert_eq!(report.groups[0].redundant_paths, vec![other]);
}

// --- Live-shape end-to-end: scan -> preview -> quarantine apply -> rollback
//
// A temporary directory tree shaped like the real chaotic collection: a
// flattened loose ROM, a nested per-game ZIP-shaped duplicate (a distinct
// outer file whose own bytes never equal the loose ROM's - no real ZIP
// container is needed to prove that distinction, since it holds for any
// two files with different bytes), a byte-identical duplicate of the
// loose ROM inside a designated trusted root, one CUE/BIN release, and one
// completely unrelated same-directory file that must never be touched.

#[test]
fn live_shape_scan_preview_apply_and_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let flat_dir = temp.path().join("flat");
    let nested_dir = temp.path().join("nested").join("SomeGame");
    let trusted_dir = temp.path().join("trusted-root");
    std::fs::create_dir_all(&flat_dir).unwrap();
    std::fs::create_dir_all(&nested_dir).unwrap();
    std::fs::create_dir_all(&trusted_dir).unwrap();

    // Flattened loose file, and a byte-identical redundant copy sitting in
    // the designated trusted root (so the scan has real evidence to pick a
    // canonical copy from - never alphabetical).
    let loose = flat_dir.join("Some Game (USA).bin");
    std::fs::write(&loose, b"the-actual-rom-bytes-of-some-game").unwrap();
    let trusted_copy = trusted_dir.join("Some Game (USA).bin");
    std::fs::write(&trusted_copy, b"the-actual-rom-bytes-of-some-game").unwrap();

    // A nested "per-game archive" representation - a different container
    // whose own bytes are never equal to the loose ROM's, exactly the
    // "ZIP vs loose ROM" case that must never be called an exact
    // duplicate.
    let archive_alternate = nested_dir.join("Some Game (USA).zip");
    std::fs::write(&archive_alternate, b"PK\x03\x04-different-container-bytes").unwrap();

    // One CUE/BIN release, self-contained and untouched by any duplicate.
    let cue_bin = flat_dir.join("Another Game.bin");
    std::fs::write(&cue_bin, b"another-games-disc-bytes").unwrap();
    let cue = flat_dir.join("Another Game.cue");
    std::fs::write(&cue, "FILE \"Another Game.bin\" BINARY\n").unwrap();

    // One completely unrelated file sharing a directory with candidates -
    // must never be scanned into any group or moved.
    let unrelated = flat_dir.join("readme.txt");
    std::fs::write(&unrelated, b"not a rom at all").unwrap();

    let candidates = vec![
        loose.clone(),
        trusted_copy.clone(),
        archive_alternate.clone(),
        cue_bin.clone(),
        cue.clone(),
        unrelated.clone(),
    ];
    let trusted_roots_evidence = vec![trusted_dir.clone()];
    let trusted = TrustedRoots::from_paths([temp.path()]);
    let mut cache = DuplicateHashCache::new();

    // --- Scan / report ------------------------------------------------
    let report = scan_exact_duplicates(
        &candidates,
        &trusted,
        &trusted_roots_evidence,
        &BTreeSet::new(),
        None,
    );

    assert_eq!(report.groups.len(), 1, "{:?}", report.groups);
    let group = &report.groups[0];
    assert_eq!(
        group.recommendation,
        CanonicalRecommendation::TrustedRoot(trusted_copy.clone())
    );
    assert_eq!(group.redundant_paths, vec![loose.clone()]);
    assert_eq!(group.readiness, GroupQuarantineReadiness::Safe);
    assert_eq!(group.multi_file, MultiFileProtection::NotMultiFile);
    assert_eq!(
        group.reclaimable_bytes,
        b"the-actual-rom-bytes-of-some-game".len() as u64
    );
    assert!(report.excluded.is_empty());

    // --- Preview: build proposals + transaction, mutating nothing ------
    let proposals =
        build_exact_duplicate_group_proposals(group, &trusted_dir, &mut cache, &trusted, None)
            .expect("proposals");
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].source_path, loose);

    let mut transaction = build_quarantine_transaction(
        &proposals,
        &trusted_copy,
        &trusted_dir,
        0,
        &mut cache,
        &trusted,
        None,
    )
    .expect("transaction");
    assert!(loose.exists(), "preview must not move anything yet");

    // --- Explicit apply -------------------------------------------------
    let journal_dir = temp.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let cancel = AtomicBool::new(false);
    apply_quarantine_transaction(
        &mut transaction,
        &trusted_copy,
        &trusted_dir,
        0,
        trusted.clone(),
        &journal_dir,
        &cancel,
        &mut cache,
    )
    .expect("apply");

    assert!(!loose.exists(), "redundant loose copy was quarantined");
    assert!(trusted_copy.exists(), "trusted-root copy retained");
    assert!(archive_alternate.exists(), "alternate container untouched");
    assert!(cue_bin.exists(), "CUE/BIN release untouched");
    assert!(cue.exists(), "CUE launcher untouched");
    assert!(unrelated.exists(), "unrelated file untouched");
    assert_eq!(
        std::fs::read(&unrelated).unwrap(),
        b"not a rom at all",
        "unrelated file content untouched"
    );

    // --- Rollback restores the exact original path ----------------------
    let rollback =
        rollback_quarantine_transaction(&mut transaction, &journal_dir, &cancel, &trusted_dir)
            .expect("rollback");
    let _ = rollback;
    assert!(loose.exists(), "rollback restored the exact original path");
    assert_eq!(
        std::fs::read(&loose).unwrap(),
        b"the-actual-rom-bytes-of-some-game"
    );
    assert!(
        trusted_copy.exists(),
        "retained copy still present after rollback"
    );
}

// --- corrected evidence model: SHA-256 is the sole authority --------------

#[test]
fn triple_legacy_hashes_alone_do_not_authorize_quarantine_without_sha256() {
    // A real hash collision can't be constructed for a test, so this proves
    // the authorization boundary directly on the one pure predicate that
    // decides group membership: two pieces of evidence that agree on every
    // legacy field (crc32/md5/sha1 are not even part of `FullFileIdentity`)
    // but disagree on SHA-256 must never be treated as a match.
    let a = FullFileIdentity {
        size_bytes: 4096,
        sha256: "a".repeat(64),
    };
    let b = FullFileIdentity {
        size_bytes: 4096,
        sha256: "b".repeat(64),
    };
    assert!(!exact_bytes_match(&a, &b));

    // And the same size with a differing SHA-256, from real files whose
    // legacy triple happens to already have narrowed them together, must
    // not be able to reach the same group: forcing a legacy-hash collision
    // is infeasible, so this instead proves the scan pipeline never even
    // looks at the legacy triple to decide `sha256_buckets` membership -
    // only `(size_bytes, sha256)` from `hash_full_file_sha256` does.
    let temp = tempfile::tempdir().unwrap();
    let one = temp.path().join("one.bin");
    let two = temp.path().join("two.bin");
    std::fs::write(&one, b"aaaaaaaaaaaaaaaa").unwrap();
    std::fs::write(&two, b"bbbbbbbbbbbbbbbb").unwrap();
    let trusted = trusted_for(temp.path());
    let identity_one = hash_full_file_sha256(&one, &trusted, None).unwrap();
    let identity_two = hash_full_file_sha256(&two, &trusted, None).unwrap();
    assert_eq!(identity_one.size_bytes, identity_two.size_bytes);
    assert_ne!(identity_one.sha256, identity_two.sha256);
    assert!(!exact_bytes_match(&identity_one, &identity_two));
}

#[test]
fn matching_sha256_and_size_authorizes_an_exact_group() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a.bin");
    let b = temp.path().join("b.bin");
    std::fs::write(&a, b"byte-for-byte identical payload").unwrap();
    std::fs::write(&b, b"byte-for-byte identical payload").unwrap();
    let trusted = trusted_for(temp.path());

    let identity_a = hash_full_file_sha256(&a, &trusted, None).unwrap();
    let identity_b = hash_full_file_sha256(&b, &trusted, None).unwrap();
    assert!(exact_bytes_match(&identity_a, &identity_b));
    assert_eq!(
        identity_a.sha256.len(),
        64,
        "SHA-256 is 32 bytes hex-encoded"
    );

    let report = scan_exact_duplicates(&[a, b], &trusted, &[], &BTreeSet::new(), None);
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].sha256, identity_a.sha256);
    assert_eq!(report.groups[0].size_bytes, identity_a.size_bytes);
}

#[test]
fn archive_member_hashes_never_substitute_for_outer_file_sha256() {
    // hash_full_file_sha256 must hash the physical ZIP itself, never look
    // inside it - a ZIP containing an identical inner member as some loose
    // file must never be treated as an "exact duplicate" of that member,
    // because their outer-file SHA-256 values are never equal (the ZIP
    // carries container framing bytes the loose member does not).
    let temp = tempfile::tempdir().unwrap();
    let loose = temp.path().join("game.bin");
    std::fs::write(&loose, b"the-actual-rom-bytes").unwrap();

    let zip_path = temp.path().join("game.zip");
    {
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("game.bin", SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, b"the-actual-rom-bytes").unwrap();
        writer.finish().unwrap();
    }
    let trusted = trusted_for(temp.path());

    let loose_identity = hash_full_file_sha256(&loose, &trusted, None).unwrap();
    let zip_identity = hash_full_file_sha256(&zip_path, &trusted, None).unwrap();
    assert_ne!(
        loose_identity.sha256, zip_identity.sha256,
        "the ZIP's own outer-file SHA-256 must never equal its member's SHA-256"
    );
    assert!(!exact_bytes_match(&loose_identity, &zip_identity));

    let report = scan_exact_duplicates(&[loose, zip_path], &trusted, &[], &BTreeSet::new(), None);
    assert!(report.groups.is_empty(), "{:?}", report.groups);
}

#[test]
fn a_file_mutated_after_preview_blocks_apply() {
    let live = live_two_copy_group();
    let trusted_roots = TrustedRoots::from_paths([live.temp.path()]);
    let mut cache = DuplicateHashCache::new();
    let report = scan_exact_duplicates(
        &[live.retained.clone(), live.redundant.clone()],
        &trusted_roots,
        std::slice::from_ref(&live.trusted_root),
        &BTreeSet::new(),
        None,
    );
    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.readiness, GroupQuarantineReadiness::Safe);

    // The redundant copy is mutated after the scan/preview but before the
    // proposal/apply step - simulating a file changing between preview and
    // apply.
    std::fs::write(
        &live.redundant,
        b"mutated after preview, not the same bytes",
    )
    .unwrap();

    let outcome = build_exact_duplicate_group_proposals(
        group,
        &live.trusted_root,
        &mut cache,
        &trusted_roots,
        None,
    );

    assert!(
        outcome.is_err(),
        "a file that changed since preview must block the whole group, not just itself"
    );
    assert!(live.redundant.exists(), "nothing was moved");
    assert!(live.retained.exists(), "nothing was moved");
    assert_eq!(
        std::fs::read(&live.redundant).unwrap(),
        b"mutated after preview, not the same bytes"
    );
}

#[test]
fn a_retained_copy_mutated_after_preview_also_blocks_apply() {
    let live = live_two_copy_group();
    let trusted_roots = TrustedRoots::from_paths([live.temp.path()]);
    let mut cache = DuplicateHashCache::new();
    let report = scan_exact_duplicates(
        &[live.retained.clone(), live.redundant.clone()],
        &trusted_roots,
        std::slice::from_ref(&live.trusted_root),
        &BTreeSet::new(),
        None,
    );
    let group = &report.groups[0];

    // This time the *retained* ("keeper") copy is the one that changes -
    // just as unsafe to build a move plan against as the redundant side.
    std::fs::write(&live.retained, b"the keeper itself changed too").unwrap();

    let outcome = build_exact_duplicate_group_proposals(
        group,
        &live.trusted_root,
        &mut cache,
        &trusted_roots,
        None,
    );

    assert!(outcome.is_err());
    assert!(live.redundant.exists());
    assert!(live.retained.exists());
}

// --- apply_user_choice: the GUI's honest path out of RequiresUserChoice ----

#[test]
fn a_user_choice_on_an_undecided_group_becomes_safe_and_labelled_as_chosen() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("aaa.bin");
    let z = temp.path().join("zzz.bin");
    std::fs::write(&a, b"tied bytes, no evidence").unwrap();
    std::fs::write(&z, b"tied bytes, no evidence").unwrap();
    let trusted = trusted_for(temp.path());
    let report = scan_exact_duplicates(
        &[a.clone(), z.clone()],
        &trusted,
        &[],
        &BTreeSet::new(),
        None,
    );
    assert_eq!(
        report.groups[0].recommendation,
        CanonicalRecommendation::RequiresUserChoice
    );

    let chosen = apply_user_choice(&report.groups[0], &z).expect("valid choice");
    assert_eq!(
        chosen.recommendation,
        CanonicalRecommendation::UserChosen(z.clone())
    );
    assert!(chosen.recommendation.reason().contains("chosen"));
    assert_eq!(chosen.redundant_paths, vec![a]);
    assert_eq!(chosen.readiness, GroupQuarantineReadiness::Safe);
}

#[test]
fn a_user_choice_naming_a_path_outside_the_group_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("aaa.bin");
    let z = temp.path().join("zzz.bin");
    let outsider = temp.path().join("not_in_group.bin");
    std::fs::write(&a, b"tied bytes, no evidence").unwrap();
    std::fs::write(&z, b"tied bytes, no evidence").unwrap();
    let trusted = trusted_for(temp.path());
    let report = scan_exact_duplicates(&[a, z], &trusted, &[], &BTreeSet::new(), None);

    let result = apply_user_choice(&report.groups[0], &outsider);
    assert!(result.is_err());
}

#[test]
fn a_user_choice_on_a_blocked_multi_file_group_stays_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let shared = temp.path().join("shared_track.bin");
    std::fs::write(&shared, b"shared-track-bytes").unwrap();
    let cue1 = temp.path().join("one.cue");
    std::fs::write(&cue1, "FILE \"shared_track.bin\" BINARY\n").unwrap();
    let cue2 = temp.path().join("two.cue");
    std::fs::write(&cue2, "FILE \"shared_track.bin\" BINARY\n").unwrap();
    let trusted_dir = temp.path().join("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let shared_dup = trusted_dir.join("shared_track_copy.bin");
    std::fs::write(&shared_dup, b"shared-track-bytes").unwrap();
    let trusted = trusted_for(temp.path());

    let report = scan_exact_duplicates(
        &[shared.clone(), cue1, cue2, shared_dup.clone()],
        &trusted,
        &[trusted_dir],
        &BTreeSet::new(),
        None,
    );
    let group = report
        .groups
        .iter()
        .find(|g| g.members.iter().any(|m| m.path == shared))
        .expect("shared-track group present");
    // This group is already Blocked by multi-file protection, not merely
    // NeedsReview - a manual choice must never bypass that safety check.
    assert!(matches!(
        group.readiness,
        GroupQuarantineReadiness::Blocked(_)
    ));

    let chosen = apply_user_choice(group, &shared_dup).expect("valid choice");
    assert!(
        matches!(chosen.readiness, GroupQuarantineReadiness::Blocked(_)),
        "{:?}",
        chosen.readiness
    );
}
