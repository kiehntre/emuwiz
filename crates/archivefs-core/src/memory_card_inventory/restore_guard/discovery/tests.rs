//! Discovery honesty: unreadable storage, partial enumeration, the 1,024 bound,
//! symlinks and malformed journals. Synthetic temporary directories only.

use super::*;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;

fn name(i: usize) -> String {
    format!("ps2-psu-restore-{i:05}.json")
}

fn dir_with(count: usize) -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let journals = root.path().join("journals");
    fs::create_dir(&journals).unwrap();
    for i in 0..count {
        fs::write(journals.join(name(i)), b"corrupt synthetic journal").unwrap();
    }
    (root, journals)
}

fn running_as_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .is_ok_and(|s| s.lines().any(|l| l.starts_with("Uid:\t0\t")))
}

#[test]
fn a_directory_that_does_not_exist_is_a_complete_empty_inventory() {
    let root = tempfile::tempdir().unwrap();
    let found = discover_ps2_restore_journals(&root.path().join("never-created"));
    assert!(found.is_empty() && found.is_complete());
    assert!(!root.path().join("never-created").exists());
}

#[test]
fn an_empty_existing_directory_is_a_complete_empty_inventory() {
    let (_root, journals) = dir_with(0);
    assert!(discover_ps2_restore_journals(&journals).is_complete());
}

#[test]
fn an_unreadable_directory_is_a_listing_failure_with_its_error_kind_and_path() {
    if running_as_root() {
        return;
    }
    let (_root, journals) = dir_with(1);
    fs::set_permissions(&journals, fs::Permissions::from_mode(0o000)).unwrap();
    let found = discover_ps2_restore_journals(&journals);
    let strict = recover_all_interrupted_ps2_restores(&journals, 1);
    fs::set_permissions(&journals, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(found.is_empty());
    assert!(!found.is_complete());
    match &found.problems[..] {
        [Ps2DiscoveryProblem::ListingFailed { kind, detail }] => {
            assert_eq!(*kind, ErrorKind::PermissionDenied);
            assert!(
                detail.contains("journals") && detail.contains("os error 13"),
                "{detail}"
            );
        }
        other => panic!("expected one ListingFailed, got {other:?}"),
    }
    assert!(strict.outcomes.is_empty() && strict.problems == found.problems);
}

#[test]
fn a_symlink_loop_is_a_listing_failure_not_an_empty_inventory() {
    let root = tempfile::tempdir().unwrap();
    let (a, b) = (root.path().join("a"), root.path().join("b"));
    std::os::unix::fs::symlink(&b, &a).unwrap();
    std::os::unix::fs::symlink(&a, &b).unwrap();
    let found = discover_ps2_restore_journals(&a);
    assert!(found.is_empty() && !found.is_complete());
    assert!(
        matches!(&found.problems[..], [Ps2DiscoveryProblem::ListingFailed { detail, .. }] if detail.contains("os error 40"))
    );
}

#[test]
fn a_dangling_symlink_or_a_plain_file_is_uncertain_not_absent() {
    let root = tempfile::tempdir().unwrap();
    let dangling = root.path().join("dangling");
    std::os::unix::fs::symlink(root.path().join("gone"), &dangling).unwrap();
    let file = root.path().join("file");
    fs::write(&file, b"not a directory").unwrap();
    for path in [dangling, file] {
        let found = discover_ps2_restore_journals(&path);
        assert!(!found.is_complete(), "{}", path.display());
        assert!(matches!(
            &found.problems[..],
            [Ps2DiscoveryProblem::ListingFailed { .. }]
        ));
    }
}

#[test]
fn an_enumeration_error_midway_is_reported_and_partial_records_are_not_complete() {
    let mut problems = Vec::new();
    let entries = vec![
        Ok(PathBuf::from("/synthetic").join(name(2))),
        Ok(PathBuf::from("/synthetic").join(name(1))),
        Err(std::io::Error::from_raw_os_error(5)),
        Ok(PathBuf::from("/synthetic").join(name(0))),
    ];
    let listed = collect_smallest(Path::new("/synthetic"), entries.into_iter(), &mut problems);
    assert_eq!(
        listed,
        [
            PathBuf::from("/synthetic").join(name(1)),
            PathBuf::from("/synthetic").join(name(2))
        ]
    );
    assert!(
        matches!(&problems[..], [Ps2DiscoveryProblem::EnumerationInterrupted { detail }] if detail.contains("/synthetic") && detail.contains("os error 5"))
    );
}

#[test]
fn exactly_1024_records_are_complete_and_sorted() {
    let (_root, journals) = dir_with(PS2_DISCOVERY_LIMIT);
    let found = discover_ps2_restore_journals(&journals);
    assert_eq!(found.len(), 1024);
    assert!(found.is_complete(), "{:?}", found.problems);
    assert!(found.windows(2).all(|pair| pair[0].path < pair[1].path));
}

#[test]
fn record_1025_is_reported_as_truncation_and_the_listed_set_is_deterministic() {
    let (_root, journals) = dir_with(PS2_DISCOVERY_LIMIT + 1);
    let first = discover_ps2_restore_journals(&journals);
    let second = discover_ps2_restore_journals(&journals);
    assert_eq!(first, second);
    assert_eq!(first.len(), 1024);
    assert!(!first.is_complete());
    assert_eq!(
        first.problems,
        [Ps2DiscoveryProblem::Truncated {
            limit: 1024,
            omitted: 1
        }]
    );
    // The smallest names are listed, whatever order the filesystem returns.
    assert_eq!(
        first[0].path.file_name().unwrap().to_str().unwrap(),
        name(0)
    );
    assert_eq!(
        first[1023].path.file_name().unwrap().to_str().unwrap(),
        name(1023)
    );
    assert!(first.iter().all(|r| !r.path.ends_with(name(1024))));
    assert!(first.problems[0].to_string().contains("1 more"));
}

#[test]
fn truncation_counts_every_omitted_record_in_bounded_memory() {
    let mut problems = Vec::new();
    let entries = (0..50_000)
        .rev()
        .map(|i| Ok(PathBuf::from("/s").join(name(i))));
    let listed = collect_smallest(Path::new("/s"), entries, &mut problems);
    assert_eq!(listed.len(), 1024);
    assert_eq!(listed[0], PathBuf::from("/s").join(name(0)));
    assert_eq!(
        problems,
        [Ps2DiscoveryProblem::Truncated {
            limit: 1024,
            omitted: 50_000 - 1024
        }]
    );
}

#[test]
fn unrelated_entries_do_not_count_toward_the_bound() {
    let (_root, journals) = dir_with(PS2_DISCOVERY_LIMIT);
    for i in 0..200 {
        fs::write(journals.join(format!("unrelated-{i}.txt")), b"x").unwrap();
    }
    assert!(discover_ps2_restore_journals(&journals).is_complete());
}

#[test]
fn symlinked_and_directory_entries_with_journal_names_surface_as_blocked_records() {
    let (root, journals) = dir_with(0);
    let real = root.path().join("elsewhere.json");
    fs::write(&real, b"x").unwrap();
    std::os::unix::fs::symlink(&real, journals.join(name(1))).unwrap();
    std::os::unix::fs::symlink(root.path().join("gone"), journals.join(name(2))).unwrap();
    fs::create_dir(journals.join(name(3))).unwrap();
    fs::write(
        journals.join(name(4)),
        b"EMUWIZ-PS2-RESTORE-JOURNAL v1 sha256=00\n{}",
    )
    .unwrap();
    let found = discover_ps2_restore_journals(&journals);
    assert!(found.is_complete());
    assert_eq!(found.len(), 4);
    for row in found.iter() {
        assert!(row.error.is_some() && row.needs_recovery && row.needs_attention);
        assert!(!row.undo_available, "{}", row.path.display());
    }
    assert_eq!(fs::read(&real).unwrap(), b"x");
}

#[test]
fn a_non_utf8_journal_name_is_listed_not_silently_dropped() {
    let (_root, journals) = dir_with(0);
    let mut raw = b"ps2-psu-restore-".to_vec();
    raw.extend([0xff, 0xfe]);
    raw.extend(b".json");
    fs::write(journals.join(std::ffi::OsString::from_vec(raw)), b"corrupt").unwrap();
    let found = discover_ps2_restore_journals(&journals);
    assert_eq!(found.len(), 1);
    assert!(found[0].error.is_some());
}

#[test]
fn batch_recovery_reports_truncation_and_judges_only_listed_records() {
    let (_root, journals) = dir_with(PS2_DISCOVERY_LIMIT + 1);
    let run = recover_all_interrupted_ps2_restores(&journals, 1);
    assert_eq!(run.outcomes.len(), 1024);
    assert!(run.outcomes.iter().all(|(_, outcome)| outcome.is_err()));
    assert_eq!(
        run.problems,
        [Ps2DiscoveryProblem::Truncated {
            limit: 1024,
            omitted: 1
        }]
    );
    assert!(
        journals.join(name(1024)).exists(),
        "an omitted record is left untouched"
    );
}
