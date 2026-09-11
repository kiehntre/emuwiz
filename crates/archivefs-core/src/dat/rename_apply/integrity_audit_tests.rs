//! Deterministic interleaving at the existing preflight/mutation boundary.
//! Passing `known_gap` tests document an unfixed violation, not safe behavior.
#![cfg(target_os = "linux")]

use super::*;
use crate::safe_read::TrustedRoots;
use std::collections::BTreeSet;

#[test]
fn known_gap_source_swap_after_preflight_moves_the_wrong_object_before_reporting_failure() {
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

    // Exactly the ordering an external writer can take after preflight
    // (including while the executor writes the Applying journal checkpoint).
    std::fs::rename(&source, temp.path().join("retained-original")).unwrap();
    std::fs::write(&source, b"another game's only copy").unwrap();
    let failure = executor::apply_mutation(&entry).unwrap_err();
    assert_eq!(failure.0, EntryState::ApplyFailed);
    assert!(!source.exists());
    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"another game's only copy"
    );
    assert_eq!(
        std::fs::read(temp.path().join("retained-original")).unwrap(),
        b"reviewed game"
    );
}
