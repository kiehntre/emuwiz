use std::sync::atomic::AtomicBool;

use tempfile::tempdir;

use super::*;
use crate::safe_read::TrustedRoots;

fn source(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let cue = dir.join("Disc space ü.cue");
    let bin = dir.join("Track space ü.bin");
    let mut bytes = Vec::with_capacity(2048 * 16);
    for sector in 0..16u8 {
        bytes.extend(std::iter::repeat_n(sector, 2048));
    }
    std::fs::write(&bin, bytes).unwrap();
    std::fs::write(
        &cue,
        "FILE \"Track space ü.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    (cue, bin)
}

fn chdman_available() -> bool {
    std::fs::metadata("/usr/bin/chdman")
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

#[test]
fn real_chdman_output_is_fingerprint_verified_before_finalization() {
    if !chdman_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (cue, bin) = source(dir.path());
    let target = dir.path().join("Disc result.chd");
    let journal = dir.path().join("journal");
    let plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(std::path::Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    let before_cue = std::fs::read(&cue).unwrap();
    let before_bin = std::fs::read(&bin).unwrap();
    let trusted = TrustedRoots::from_paths([dir.path()]);
    let (result, mut transaction) = execute_chd_conversion(
        &plan,
        trusted,
        &journal,
        dir.path(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        compare_optical_fingerprints(&result.source_fingerprint, &result.output_fingerprint),
        OpticalFingerprintComparison::Equivalent
    );
    assert!(target.is_file());
    assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
    assert_eq!(std::fs::read(&bin).unwrap(), before_bin);

    rollback_chd_conversion(&mut transaction, &journal, &AtomicBool::new(false)).unwrap();
    assert!(!target.exists());
    assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
    assert_eq!(std::fs::read(&bin).unwrap(), before_bin);
}

#[test]
fn unsupported_source_layout_is_rejected_before_running_chdman() {
    let dir = tempdir().unwrap();
    let cue = dir.path().join("bad.cue");
    std::fs::write(
        &cue,
        "FILE \"track.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("track.bin"), [0u8; 2352]).unwrap();
    let error = build_chd_conversion_plan(
        &cue,
        &dir.path().join("bad.chd"),
        ChdConversionSourceMode::KeepSource,
        Some(std::path::Path::new("/usr/bin/chdman")),
    )
    .unwrap_err();
    assert!(matches!(error, ChdConversionError::InvalidSource(_)));
}

#[test]
fn existing_output_is_refused_without_touching_source() {
    let dir = tempdir().unwrap();
    let (cue, _) = source(dir.path());
    let target = dir.path().join("Disc.chd");
    std::fs::write(&target, b"existing").unwrap();
    let error = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(std::path::Path::new("/usr/bin/chdman")),
    )
    .unwrap_err();
    assert!(matches!(error, ChdConversionError::InvalidTarget(_)));
}

#[test]
fn quarantine_mode_moves_the_pair_only_after_verified_output_and_rolls_back() {
    if !chdman_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (cue, bin) = source(dir.path());
    let target = dir.path().join("converted.chd");
    let journal = dir.path().join("journal");
    let plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::QuarantineSource,
        Some(std::path::Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    let (result, mut transaction) = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &journal,
        dir.path(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(result.source_quarantined);
    assert!(target.is_file());
    assert!(!cue.exists());
    assert!(!bin.exists());
    rollback_chd_conversion(&mut transaction, &journal, &AtomicBool::new(false)).unwrap();
    assert!(!target.exists());
    assert!(cue.is_file());
    assert!(bin.is_file());
}

#[test]
fn source_drift_is_refused_before_chdman_runs() {
    let dir = tempdir().unwrap();
    let (cue, bin) = source(dir.path());
    let target = dir.path().join("converted.chd");
    let plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(std::path::Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    std::fs::write(&bin, [0x55u8; 2048 * 16]).unwrap();
    let error = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(error, ChdConversionError::StaleSource(_)));
    assert!(!target.exists());
}
