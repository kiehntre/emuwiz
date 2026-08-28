use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;

use tempfile::tempdir;

use super::*;

fn canonical() -> Vec<u8> {
    let mut bytes = N64ByteOrder::Z64.magic().to_vec();
    bytes.extend_from_slice(&[0, 1, 2, 3, 0xde, 0xad, 0xbe, 0xef]);
    bytes
}

fn physical(canonical: &[u8], order: N64ByteOrder) -> Vec<u8> {
    crate::n64_byte_order::denormalize_from_z64(canonical, order).unwrap()
}

fn roots(dir: &std::path::Path) -> TrustedRoots {
    TrustedRoots::from_paths(vec![dir.to_path_buf()])
}

#[test]
fn all_three_orders_form_one_group_with_distinct_physical_hashes() {
    let dir = tempdir().unwrap();
    let z = dir.path().join("game.z64");
    let v = dir.path().join("game.v64");
    let n = dir.path().join("game.n64");
    let bytes = canonical();
    std::fs::write(&z, physical(&bytes, N64ByteOrder::Z64)).unwrap();
    std::fs::write(&v, physical(&bytes, N64ByteOrder::V64)).unwrap();
    std::fs::write(&n, physical(&bytes, N64ByteOrder::N64)).unwrap();

    let report = scan_n64_equivalent_duplicates(
        &[z.clone(), v.clone(), n.clone()],
        &roots(dir.path()),
        None,
    );
    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.preferred, z);
    assert_eq!(group.quarantine_candidates, vec![v, n]);
    assert_eq!(group.projected_savings, bytes.len() as u64 * 2);
    assert_eq!(
        group
            .members
            .iter()
            .map(|member| member.physical_sha256.clone())
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    assert_eq!(
        group
            .members
            .iter()
            .map(|member| member.canonical_sha256.clone())
            .collect::<BTreeSet<_>>()
            .len(),
        1
    );
}

#[test]
fn invalid_and_misaligned_candidates_are_excluded() {
    let dir = tempdir().unwrap();
    let bad = dir.path().join("bad.z64");
    let short = dir.path().join("short.n64");
    let other = dir.path().join("other.v64");
    std::fs::write(&bad, b"not an n64").unwrap();
    std::fs::write(&short, [0x40, 0x12, 0x37, 0x80, 1, 2]).unwrap();
    std::fs::write(&other, physical(&canonical(), N64ByteOrder::V64)).unwrap();
    let report = scan_n64_equivalent_duplicates(
        &[bad.clone(), short.clone(), other],
        &roots(dir.path()),
        None,
    );
    assert!(report.groups.is_empty());
    assert!(
        report
            .excluded
            .iter()
            .any(|item| item.path == bad && item.reason.contains("unrecognized"))
    );
    assert!(
        report
            .excluded
            .iter()
            .any(|item| item.path == short && item.reason.contains("multiple of 4"))
    );
}

#[test]
fn same_physical_copy_is_left_to_exact_review() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.z64");
    let b = dir.path().join("b.z64");
    std::fs::write(&a, canonical()).unwrap();
    std::fs::write(&b, canonical()).unwrap();
    let report = scan_n64_equivalent_duplicates(&[a, b.clone()], &roots(dir.path()), None);
    assert!(report.groups.is_empty());
    assert!(
        report
            .excluded
            .iter()
            .any(|item| item.path == b && item.reason.contains("Exact Duplicate"))
    );
}

#[test]
fn unrelated_files_are_excluded_without_filename_authority() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("looks-like.n64");
    std::fs::write(&path, b"ordinary content").unwrap();
    let report = scan_n64_equivalent_duplicates(&[path.clone()], &roots(dir.path()), None);
    assert!(report.groups.is_empty());
    assert_eq!(report.excluded.len(), 1);
}

#[test]
fn matching_names_do_not_group_different_canonical_content() {
    let dir = tempdir().unwrap();
    let left_dir = dir.path().join("left");
    let right_dir = dir.path().join("right");
    std::fs::create_dir_all(&left_dir).unwrap();
    std::fs::create_dir_all(&right_dir).unwrap();
    let left = left_dir.join("same-name.z64");
    let right = right_dir.join("same-name.z64");
    let mut different = canonical();
    different[7] ^= 0xff;
    std::fs::write(&left, canonical()).unwrap();
    std::fs::write(&right, different).unwrap();
    let report = scan_n64_equivalent_duplicates(&[left, right], &roots(dir.path()), None);
    assert!(report.groups.is_empty());
}

#[test]
fn symlink_candidates_and_paths_outside_trusted_root_are_excluded() {
    let dir = tempdir().unwrap();
    let trusted_dir = dir.path().join("trusted");
    let outside_dir = dir.path().join("outside");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    std::fs::create_dir_all(&outside_dir).unwrap();
    let outside = outside_dir.join("real.z64");
    std::fs::write(&outside, canonical()).unwrap();
    let link = trusted_dir.join("link.z64");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let report =
        scan_n64_equivalent_duplicates(&[link.clone(), outside], &roots(&trusted_dir), None);
    assert_eq!(report.groups.len(), 0);
    assert!(
        report
            .excluded
            .iter()
            .any(|item| item.path == link && item.reason.contains("regular"))
    );
}

#[test]
fn apply_and_rollback_use_shared_journal_transaction() {
    let dir = tempdir().unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir(&journal).unwrap();
    let preferred = dir.path().join("keep.z64");
    let redundant = dir.path().join("redundant.v64");
    let bytes = canonical();
    std::fs::write(&preferred, physical(&bytes, N64ByteOrder::Z64)).unwrap();
    std::fs::write(&redundant, physical(&bytes, N64ByteOrder::V64)).unwrap();
    let report = scan_n64_equivalent_duplicates(
        &[preferred.clone(), redundant.clone()],
        &roots(dir.path()),
        None,
    );
    let group = report.groups.first().unwrap().clone();
    let cancel = AtomicBool::new(false);
    let mut result =
        apply_n64_equivalent_group(&group, dir.path(), roots(dir.path()), &journal, &cancel)
            .unwrap();
    assert!(preferred.exists());
    assert!(!redundant.exists());
    assert!(result.summary.applied > 0);
    rollback_repair_transaction(&mut result.transaction, &journal, &cancel).unwrap();
    assert!(redundant.exists());
    assert_eq!(
        std::fs::read(&redundant).unwrap(),
        physical(&bytes, N64ByteOrder::V64)
    );
    let after_rollback =
        scan_n64_equivalent_duplicates(&[preferred, redundant], &roots(dir.path()), None);
    assert_eq!(after_rollback.groups.len(), 1);
}

#[test]
fn changed_redundant_copy_is_refused_before_quarantine() {
    let dir = tempdir().unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir(&journal).unwrap();
    let preferred = dir.path().join("keep.z64");
    let redundant = dir.path().join("redundant.v64");
    let bytes = canonical();
    std::fs::write(&preferred, physical(&bytes, N64ByteOrder::Z64)).unwrap();
    std::fs::write(&redundant, physical(&bytes, N64ByteOrder::V64)).unwrap();
    let group = scan_n64_equivalent_duplicates(
        &[preferred.clone(), redundant.clone()],
        &roots(dir.path()),
        None,
    )
    .groups
    .first()
    .unwrap()
    .clone();
    std::fs::write(&redundant, b"changed after preview").unwrap();
    let error = apply_n64_equivalent_group(
        &group,
        dir.path(),
        roots(dir.path()),
        &journal,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        crate::repair::RepairExecutionError::StaleSource { .. }
    ));
    assert!(redundant.exists());
}

#[test]
fn disappearing_preferred_copy_is_refused() {
    let dir = tempdir().unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir(&journal).unwrap();
    let preferred = dir.path().join("keep.z64");
    let redundant = dir.path().join("redundant.v64");
    let bytes = canonical();
    std::fs::write(&preferred, physical(&bytes, N64ByteOrder::Z64)).unwrap();
    std::fs::write(&redundant, physical(&bytes, N64ByteOrder::V64)).unwrap();
    let group =
        scan_n64_equivalent_duplicates(&[preferred.clone(), redundant], &roots(dir.path()), None)
            .groups
            .first()
            .unwrap()
            .clone();
    std::fs::remove_file(&preferred).unwrap();
    let error = apply_n64_equivalent_group(
        &group,
        dir.path(),
        roots(dir.path()),
        &journal,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        crate::repair::RepairExecutionError::Build { .. }
    ));
}
