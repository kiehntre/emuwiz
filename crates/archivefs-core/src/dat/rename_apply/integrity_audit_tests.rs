//! Deterministic interleaving at the preflight/mutation boundary.
#![cfg(target_os = "linux")]

use super::*;
use crate::dat::rename_apply::executor::set_test_before_rename_hook;
use crate::safe_read::TrustedRoots;
use std::collections::BTreeSet;

struct Fixture {
    _temp: tempfile::TempDir,
    source: std::path::PathBuf,
    destination: std::path::PathBuf,
    entry: TransactionEntry,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.bin");
    let destination = temp.path().join("destination.bin");
    std::fs::write(&source, b"reviewed game").unwrap();
    let entry: TransactionEntry = serde_json::from_value(serde_json::json!({
        "source_path": source, "destination_path": destination,
        "original_basename": "source.bin", "proposed_basename": "destination.bin",
        "identity": capture_identity(&source).unwrap()
    }))
    .unwrap();
    let approved = BTreeSet::from([source.to_string_lossy().into_owned()]);
    let trusted = TrustedRoots::from_paths([temp.path()]);
    run_preflight(
        &entry,
        &PreflightOptions {
            plan_generation: 1,
            current_generation: 1,
            approved_paths: &approved,
            trusted: &trusted,
            batch_destinations: &BTreeSet::new(),
            directory_policy: DirectoryPolicy::SameDirectory,
            allow_symlink_source: false,
        },
    )
    .unwrap();
    Fixture {
        _temp: temp,
        source,
        destination,
        entry,
    }
}

fn swap_source(source: &std::path::Path) {
    std::fs::rename(source, source.with_file_name("retained-original")).unwrap();
    std::fs::write(source, b"another game's only copy").unwrap();
}

#[test]
fn source_swap_after_preflight_is_refused_without_moving_the_replacement() {
    let f = fixture();
    swap_source(&f.source);
    let failure = executor::apply_mutation(&f.entry).unwrap_err();
    assert_eq!(failure.0, EntryState::ApplyFailed);
    assert!(!f.destination.exists(), "the replacement was not moved");
    assert_eq!(
        std::fs::read(&f.source).unwrap(),
        b"another game's only copy"
    );
    assert_eq!(
        std::fs::read(f.source.with_file_name("retained-original")).unwrap(),
        b"reviewed game"
    );
}

#[test]
fn swap_inside_the_final_rename_window_is_restored_not_reported_applied() {
    let f = fixture();
    set_test_before_rename_hook(Some(swap_source));
    let failure = executor::apply_mutation(&f.entry).unwrap_err();
    assert_eq!(failure.0, EntryState::ApplyFailed);
    assert!(failure.1.contains("restored"), "{}", failure.1);
    assert!(!f.destination.exists(), "a refused move is not published");
    assert_eq!(
        std::fs::read(&f.source).unwrap(),
        b"another game's only copy"
    );
    assert_eq!(
        std::fs::read(f.source.with_file_name("retained-original")).unwrap(),
        b"reviewed game"
    );
}

#[test]
fn source_content_change_after_preflight_is_refused_in_place() {
    let f = fixture();
    std::fs::write(&f.source, b"changed after review, longer").unwrap();
    let failure = executor::apply_mutation(&f.entry).unwrap_err();
    assert_eq!(failure.0, EntryState::ApplyFailed);
    assert!(!f.destination.exists());
    assert_eq!(
        std::fs::read(&f.source).unwrap(),
        b"changed after review, longer"
    );
}

#[test]
fn unchanged_source_still_renames() {
    let f = fixture();
    executor::apply_mutation(&f.entry).unwrap();
    assert!(!f.source.exists());
    assert_eq!(std::fs::read(&f.destination).unwrap(), b"reviewed game");
}

#[test]
fn an_occupied_destination_is_never_overwritten() {
    let f = fixture();
    std::fs::write(&f.destination, b"someone else").unwrap();
    let failure = executor::apply_mutation(&f.entry).unwrap_err();
    assert_eq!(failure.0, EntryState::ApplyFailed);
    assert_eq!(std::fs::read(&f.destination).unwrap(), b"someone else");
    assert_eq!(std::fs::read(&f.source).unwrap(), b"reviewed game");
}
