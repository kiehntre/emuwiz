#![cfg(unix)]

use archivefs_core::ingestion::cue_preservation::*;
use archivefs_core::ingestion::cue_timeline::chd_conversion_preservation;
use archivefs_core::optical_fingerprint::fingerprint_cue_bin_exact;
use archivefs_core::repair::optical_conversion::{
    ChdConversionError, ChdConversionSourceMode, build_chd_conversion_plan,
};
use sha2::{Digest, Sha256};
use std::fs::{File, FileTimes};
use std::path::PathBuf;

const SINGLE: &str = "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\n";

fn fixture(text: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cue = dir.path().join("disc.cue");
    let bin = dir.path().join("disc.bin");
    std::fs::write(&bin, vec![1; 3 * 2048]).unwrap();
    std::fs::write(&cue, text).unwrap();
    (dir, cue, bin)
}

fn rewrite_restoring_mtime(path: &std::path::Path, bytes: &[u8]) {
    let modified = std::fs::metadata(path).unwrap().modified().unwrap();
    std::fs::write(path, bytes).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(
        std::fs::metadata(path).unwrap().modified().unwrap(),
        modified
    );
}

fn assert_indeterminate(
    left: &Result<DiscLayoutEvidence, CuePreservationError>,
    right: &Result<DiscLayoutEvidence, CuePreservationError>,
) {
    let comparison = compare_cue_evidence(left, right);
    assert_eq!(comparison.outcome, CueComparisonOutcome::Indeterminate);
    assert_eq!(comparison.programs_match, None);
    assert!(comparison.differences.is_empty());
    assert!(!comparison.refusals.is_empty());
}

#[test]
fn stable_unchanged_source_retains_the_inspected_bytes_and_equivalence() {
    let (_dir, cue, _bin) = fixture(SINGLE);
    let inspection = inspect_cue_preservation(&cue).unwrap();
    let first = read_cue_preservation(&inspection);
    let second = read_cue_preservation(&inspection);
    assert_eq!(first, second);
    assert_eq!(
        first.as_ref().unwrap().sources()[0].physical_sha256,
        Sha256::digest(vec![1; 3 * 2048])
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    assert_eq!(
        compare_cue_evidence(&first, &second).outcome,
        CueComparisonOutcome::Equivalent
    );
}

#[test]
fn same_size_modification_after_inspection_is_changed_source() {
    let (_dir, cue, bin) = fixture(SINGLE);
    let inspection = inspect_cue_preservation(&cue).unwrap();
    std::fs::write(&bin, vec![9; 3 * 2048]).unwrap();
    assert_eq!(
        read_cue_preservation(&inspection).unwrap_err(),
        CuePreservationError::ChangedSource { path: bin }
    );
}

#[test]
fn repeated_restored_mtime_writes_never_publish_later_bytes_as_inspected() {
    let (_dir, cue, bin) = fixture(SINGLE);
    for value in 2..=33 {
        let inspection = inspect_cue_preservation(&cue).unwrap();
        rewrite_restoring_mtime(&bin, &vec![value; 3 * 2048]);
        assert_eq!(
            read_cue_preservation(&inspection).unwrap_err(),
            CuePreservationError::ChangedSource { path: bin.clone() }
        );
    }
}

#[test]
fn evidence_mutation_after_capture_is_indeterminate_even_against_itself() {
    let (_dir, cue, bin) = fixture(SINGLE);
    let captured = collect_cue_preservation(&cue);
    assert!(captured.is_ok());
    rewrite_restoring_mtime(&bin, &vec![9; 3 * 2048]);
    assert_indeterminate(&captured, &captured);
}

#[test]
fn cue_mutation_after_capture_is_indeterminate_even_with_same_layout() {
    let (_dir, cue, _bin) = fixture(SINGLE);
    let captured = collect_cue_preservation(&cue);
    assert!(captured.is_ok());
    // Case changes preserve layout and byte count but change provenance bytes.
    rewrite_restoring_mtime(&cue, SINGLE.replace("BINARY", "binary").as_bytes());
    assert_indeterminate(&captured, &captured);
}

#[test]
fn atomic_replacement_after_capture_cannot_reuse_old_evidence() {
    let (dir, cue, bin) = fixture(SINGLE);
    let captured = collect_cue_preservation(&cue);
    assert!(captured.is_ok());
    let replacement = dir.path().join("replacement.bin");
    std::fs::write(&replacement, vec![1; 3 * 2048]).unwrap();
    std::fs::rename(replacement, bin).unwrap();
    assert_indeterminate(&captured, &captured);
}

#[test]
fn cue_path_replaced_by_a_symlink_is_indeterminate() {
    let (dir, cue, _bin) = fixture(SINGLE);
    let captured = collect_cue_preservation(&cue);
    assert!(captured.is_ok());
    let old = dir.path().join("old.cue");
    std::fs::rename(&cue, &old).unwrap();
    std::os::unix::fs::symlink(old, cue).unwrap();
    assert_indeterminate(&captured, &captured);
}

#[test]
fn symlink_resolution_after_capture_cannot_reuse_old_evidence() {
    let (dir, cue, _bin) = fixture(&SINGLE.replace("disc.bin", "link.bin"));
    std::os::unix::fs::symlink("disc.bin", dir.path().join("link.bin")).unwrap();
    let captured = collect_cue_preservation(&cue);
    assert!(captured.is_ok());
    std::fs::write(dir.path().join("other.bin"), vec![1; 3 * 2048]).unwrap();
    std::os::unix::fs::symlink("other.bin", dir.path().join("new-link.bin")).unwrap();
    std::fs::rename(dir.path().join("new-link.bin"), dir.path().join("link.bin")).unwrap();
    assert_indeterminate(&captured, &captured);
}

#[test]
fn directory_replacement_after_capture_cannot_reuse_old_evidence() {
    let (dir, cue, bin) = fixture(&SINGLE.replace("disc.bin", "source/disc.bin"));
    std::fs::create_dir(dir.path().join("source")).unwrap();
    std::fs::rename(bin, dir.path().join("source/disc.bin")).unwrap();
    let captured = collect_cue_preservation(&cue);
    assert!(captured.is_ok());
    std::fs::rename(dir.path().join("source"), dir.path().join("old-source")).unwrap();
    std::fs::create_dir(dir.path().join("source")).unwrap();
    std::fs::write(dir.path().join("source/disc.bin"), vec![1; 3 * 2048]).unwrap();
    assert_indeterminate(&captured, &captured);
}

#[test]
fn missing_source_after_capture_is_indeterminate() {
    let (_dir, cue, bin) = fixture(SINGLE);
    let captured = collect_cue_preservation(&cue);
    assert!(captured.is_ok());
    std::fs::remove_file(bin).unwrap();
    assert_indeterminate(&captured, &captured);
}

#[test]
fn equivalent_evidence_grants_no_conversion_or_source_removal_permission() {
    let text = SINGLE.replace(
        " INDEX 01 00:00:00",
        " INDEX 00 00:00:00\n INDEX 01 00:00:01",
    );
    let (dir, cue, bin) = fixture(&text);
    let captured = collect_cue_preservation(&cue);
    let evidence = captured.as_ref().unwrap();
    assert_eq!(
        compare_cue_evidence(&captured, &captured).outcome,
        CueComparisonOutcome::Equivalent
    );
    assert!(chd_conversion_preservation(evidence.timeline()).is_err());
    assert!(fingerprint_cue_bin_exact(&cue).is_err());
    for mode in [
        ChdConversionSourceMode::KeepSource,
        ChdConversionSourceMode::QuarantineSource,
    ] {
        assert!(matches!(
            build_chd_conversion_plan(&cue, &dir.path().join("disc.chd"), mode, None),
            Err(ChdConversionError::InvalidSource(_))
        ));
    }
    assert_eq!(std::fs::read(bin).unwrap(), vec![1; 3 * 2048]);
    assert_eq!(std::fs::read_to_string(cue).unwrap(), text);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}
