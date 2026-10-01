use std::sync::atomic::AtomicBool;

use tempfile::tempdir;

use super::*;
use crate::optical_fingerprint::fingerprint_cue_bin;
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
    assert_eq!(plan.source_layout.tracks.len(), 1);
    let track = &plan.source_layout.tracks[0];
    assert_eq!(track.number, 1);
    assert_eq!(track.path, plan.bin_path);
    assert_eq!(track.index_01.unwrap().frames, 0);
    assert!(track.index_00.is_none());
    assert!(track.pregap.is_none());
    assert!(track.postgap.is_none());
    assert!(plan.cue_identity.freshness.is_some());
    assert!(plan.bin_identity.freshness.is_some());
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
    layout_contract::verify_output_layout(&target, 16).unwrap();
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

#[test]
fn unsupported_layout_is_rechecked_when_a_public_plan_is_modified() {
    if !chdman_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (cue, _) = source(dir.path());
    let target = dir.path().join("converted.chd");
    let mut plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    let text = std::fs::read_to_string(&cue).unwrap() + "INDEX 02 00:00:04\n";
    std::fs::write(&cue, text).unwrap();
    // Updating source identity cannot turn a data-payload proof into a proof
    // of this added index. Public plans must pass admission again at execution.
    plan.cue_identity = capture_identity(&cue).unwrap();
    let error = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(error, ChdConversionError::InvalidSource(_)));
    assert!(!target.exists());
    assert!(!dir.path().join("journal").exists());
}

#[cfg(unix)]
#[test]
fn matching_payload_with_changed_postgap_is_not_finalized() {
    if !chdman_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (cue, bin) = source(dir.path());
    let fixture = dir.path().join("fixture.chd");
    let output = std::process::Command::new("/usr/bin/chdman")
        .arg("createcd")
        .arg("--input")
        .arg(&cue)
        .arg("--output")
        .arg(&fixture)
        .args(["-np", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut bytes = std::fs::read(&fixture).unwrap();
    let offset = bytes
        .windows(b"POSTGAP:0".len())
        .position(|part| part == b"POSTGAP:0")
        .unwrap();
    bytes[offset + b"POSTGAP:".len()] = b'2';
    std::fs::write(&fixture, bytes).unwrap();
    // This is the concrete failure of the old payload-only proof.
    assert_eq!(
        compare_optical_fingerprints(
            &fingerprint_cue_bin(&cue).unwrap(),
            &fingerprint_chd(&fixture).unwrap()
        ),
        OpticalFingerprintComparison::Equivalent
    );
    let converter = fixture_converter(dir.path(), "cp -- \"$d/fixture.chd\" \"$5\"");
    let before_cue = std::fs::read(&cue).unwrap();
    let before_bin = std::fs::read(&bin).unwrap();
    let target = dir.path().join("output.chd");
    let plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(&converter),
    )
    .unwrap();
    let error = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        matches!(error, ChdConversionError::VerificationFailed(_)),
        "{error:?}"
    );
    assert!(!target.exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(DISC_CONVERSION_STAGING_PREFIX)
    }));
    assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
    assert_eq!(std::fs::read(&bin).unwrap(), before_bin);
}

#[cfg(unix)]
fn fixture_converter(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let source = dir.join("converter-template.txt");
    let converter = dir.join("fixture-converter.sh");
    std::fs::write(
        &source,
        format!("#!/bin/sh\nset -eu\nd=$(dirname -- \"$0\")\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o700)).unwrap();
    // A sibling test's fork must not inherit a writable handle to the executable.
    let copied = std::process::Command::new("cp")
        .arg("--")
        .arg(&source)
        .arg(&converter)
        .output()
        .unwrap();
    assert!(copied.status.success(), "{copied:?}");
    converter
}

fn recorded_chd(cue: &Path, output: &Path, compression: Option<&str>) {
    let mut command = std::process::Command::new("/usr/bin/chdman");
    command
        .arg("createcd")
        .arg("--input")
        .arg(cue)
        .arg("--output")
        .arg(output)
        .args(["-np", "1"]);
    if let Some(compression) = compression {
        command.args(["-c", compression]);
    }
    let result = command.output().unwrap();
    assert!(result.status.success(), "{result:?}");
}

#[test]
fn recorded_chdman_partial_tracks_and_multiple_hunks_verify() {
    if !chdman_available() {
        return;
    }
    for frames in [1, 3, 4, 9, 16, 32] {
        for compression in [None, Some("none")] {
            // The recorded one-hunk compressed-map refusal has its own test.
            if compression.is_none() && frames < 8 {
                continue;
            }
            let dir = tempdir().unwrap();
            let (cue, bin) = source(dir.path());
            let payload = vec![0x42; 2048 * frames];
            std::fs::write(&bin, &payload).unwrap();
            let output = dir.path().join("recorded.chd");
            recorded_chd(&cue, &output, compression);
            layout_contract::verify_output_layout(&output, frames as u64).unwrap();
            layout_contract::verify_output_storage(&output, frames as u64).unwrap_or_else(
                |error| panic!("frames={frames}, compression={compression:?}: {error}"),
            );
            assert_eq!(
                compare_optical_fingerprints(
                    &fingerprint_cue_bin(&cue).unwrap(),
                    &fingerprint_chd(&output).unwrap()
                ),
                OpticalFingerprintComparison::Equivalent
            );
            assert_eq!(std::fs::read(&bin).unwrap(), payload);
        }
    }
}

#[cfg(unix)]
#[test]
fn source_content_and_inode_drift_cannot_hide_behind_size_and_mtime() {
    for change in ["cue", "bin", "inode", "plan_layout"] {
        let dir = tempdir().unwrap();
        let (cue, bin) = source(dir.path());
        let converter = fixture_converter(dir.path(), "touch \"$d/ran\"; exit 1");
        let target = dir.path().join("output.chd");
        let mut plan = build_chd_conversion_plan(
            &cue,
            &target,
            ChdConversionSourceMode::KeepSource,
            Some(&converter),
        )
        .unwrap();
        if change == "plan_layout" {
            plan.source_layout.tracks[0].postgap =
                Some(crate::ingestion::cue_bin::CueTimestamp { frames: 1 });
        } else {
            let path = if change == "cue" { &cue } else { &bin };
            let before = std::fs::metadata(path).unwrap();
            let mut bytes = std::fs::read(path).unwrap();
            if change == "inode" {
                let replacement = dir.path().join("replacement.bin");
                std::fs::write(&replacement, &bytes).unwrap();
                std::fs::rename(replacement, path).unwrap();
            } else {
                // Case-only CUE change retains exactly the same interpreted layout.
                bytes[0] ^= 0x20;
                std::fs::write(path, &bytes).unwrap();
            }
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
                .unwrap();
            assert_eq!(std::fs::metadata(path).unwrap().len(), before.len());
        }
        let error = execute_chd_conversion(
            &plan,
            TrustedRoots::from_paths([dir.path()]),
            &dir.path().join("journal"),
            dir.path(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            matches!(error, ChdConversionError::StaleSource(_)),
            "{change}: {error:?}"
        );
        assert!(!dir.path().join("ran").exists());
        assert!(!dir.path().join("journal").exists());
        assert!(!target.exists());
    }
}

#[cfg(unix)]
#[test]
fn component_mapping_is_rechecked_before_and_after_conversion() {
    if !chdman_available() {
        return;
    }
    for during_conversion in [false, true] {
        let dir = tempdir().unwrap();
        let (cue, bin) = source(dir.path());
        let original = std::fs::read(&bin).unwrap();
        let other = dir.path().join("other.bin");
        std::fs::write(&other, &original).unwrap();
        let alias = dir.path().join("alias.bin");
        std::os::unix::fs::symlink(&bin, &alias).unwrap();
        let text = "FILE \"alias.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
        std::fs::write(&cue, text).unwrap();
        recorded_chd(&cue, &dir.path().join("fixture.chd"), None);
        let converter = fixture_converter(
            dir.path(),
            "touch \"$d/ran\"\ncp -- \"$d/fixture.chd\" \"$5\"\nrm -- \"$d/alias.bin\"\nln -s -- \"$d/other.bin\" \"$d/alias.bin\"",
        );
        let target = dir.path().join("output.chd");
        let plan = build_chd_conversion_plan(
            &cue,
            &target,
            ChdConversionSourceMode::KeepSource,
            Some(&converter),
        )
        .unwrap();
        if !during_conversion {
            std::fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(&other, &alias).unwrap();
        }
        let error = execute_chd_conversion(
            &plan,
            TrustedRoots::from_paths([dir.path()]),
            &dir.path().join("journal"),
            dir.path(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            matches!(error, ChdConversionError::StaleSource(_)),
            "{error:?}"
        );
        assert_eq!(dir.path().join("ran").exists(), during_conversion);
        assert!(!target.exists());
        assert_eq!(std::fs::read(&cue).unwrap(), text.as_bytes());
        assert_eq!(std::fs::read(&bin).unwrap(), original);
        assert_eq!(std::fs::read(&other).unwrap(), original);
    }
}

#[cfg(unix)]
#[test]
fn source_modified_during_conversion_is_not_published() {
    if !chdman_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (cue, _) = source(dir.path());
    recorded_chd(&cue, &dir.path().join("fixture.chd"), None);
    let converter = fixture_converter(
        dir.path(),
        "cp -- \"$d/fixture.chd\" \"$5\"\nprintf changed > \"$d/Track space ü.bin\"",
    );
    let target = dir.path().join("output.chd");
    let plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(&converter),
    )
    .unwrap();
    let error = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        matches!(error, ChdConversionError::StaleSource(_)),
        "{error:?}"
    );
    assert!(!target.exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(DISC_CONVERSION_STAGING_PREFIX)
    }));
}

#[cfg(unix)]
#[test]
fn a_chd_filename_or_symlink_cannot_authorize_publication() {
    for body in [
        "printf not-a-CHD > \"$5\"",
        "ln -s -- \"$d/Track space ü.bin\" \"$5\"",
    ] {
        let dir = tempdir().unwrap();
        let (cue, bin) = source(dir.path());
        let before_cue = std::fs::read(&cue).unwrap();
        let before_bin = std::fs::read(&bin).unwrap();
        let converter = fixture_converter(dir.path(), body);
        let target = dir.path().join("output.chd");
        let plan = build_chd_conversion_plan(
            &cue,
            &target,
            ChdConversionSourceMode::KeepSource,
            Some(&converter),
        )
        .unwrap();
        let error = execute_chd_conversion(
            &plan,
            TrustedRoots::from_paths([dir.path()]),
            &dir.path().join("journal"),
            dir.path(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            matches!(error, ChdConversionError::VerificationFailed(_)),
            "{error:?}"
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
        assert_eq!(std::fs::read(&bin).unwrap(), before_bin);
    }
}

#[test]
fn quarantine_proposals_retain_the_reviewed_source_identity() {
    let dir = tempdir().unwrap();
    let (cue, _) = source(dir.path());
    let reviewed = capture_identity(&cue).unwrap();
    std::fs::write(&cue, b"foreign replacement").unwrap();
    let proposal = proposal(
        "reviewed-cue",
        &cue,
        &dir.path().join("quarantined.cue"),
        "test".into(),
        &reviewed,
    )
    .unwrap();
    assert_eq!(proposal.expected_source_identity.as_ref(), Some(&reviewed));
    assert_ne!(capture_identity(&cue).unwrap(), reviewed);
    let plan = plan_for_moves("reviewed-source", vec![proposal]).unwrap();
    assert!(matches!(
        build_repair_transaction(&plan),
        Err(RepairExecutionError::StaleSource { .. })
    ));
    assert_eq!(std::fs::read(&cue).unwrap(), b"foreign replacement");
    assert!(!dir.path().join("quarantined.cue").exists());
}

#[cfg(unix)]
#[test]
fn stale_layout_matrix_never_starts_converter_or_creates_staging() {
    use std::os::unix::fs::PermissionsExt;
    for change in [
        "pregap",
        "index00",
        "mode",
        "reorder",
        "component",
        "missing",
        "bytes",
        "plan",
        "title",
    ] {
        let dir = tempdir().unwrap();
        let (cue, bin) = source(dir.path());
        let original = std::fs::read_to_string(&cue).unwrap();
        let tool = dir.path().join("converter");
        let marker = dir.path().join("executed");
        std::fs::write(
            &tool,
            format!("#!/bin/sh\ntouch '{}'\nexit 99\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let target = dir.path().join("output.chd");
        let mut plan = build_chd_conversion_plan(
            &cue,
            &target,
            ChdConversionSourceMode::KeepSource,
            Some(&tool),
        )
        .unwrap();
        match change {
            "title" => {
                std::fs::write(&cue, original.clone() + "TITLE \"Changed annotation\"\n").unwrap()
            }
            "pregap" => std::fs::write(
                &cue,
                original.replace("INDEX 01", "PREGAP 00:00:02\nINDEX 01"),
            )
            .unwrap(),
            "index00" => std::fs::write(
                &cue,
                original.replace("INDEX 01 00:00:00", "INDEX 00 00:00:00\nINDEX 01 00:00:02"),
            )
            .unwrap(),
            "mode" => std::fs::write(&cue, original.replace("MODE1/2048", "MODE1/2352")).unwrap(),
            // A simple admitted disc cannot already have two tracks; adding a
            // reordered sequence must not turn its old preview into authority.
            "reorder" => std::fs::write(
                &cue,
                original.replace("TRACK 01", "TRACK 02") + "TRACK 01 AUDIO\nINDEX 01 00:00:04\n",
            )
            .unwrap(),
            "component" => {
                std::fs::copy(&bin, dir.path().join("other.bin")).unwrap();
                std::fs::write(
                    &cue,
                    original.replace(bin.file_name().unwrap().to_str().unwrap(), "other.bin"),
                )
                .unwrap();
            }
            "missing" => std::fs::remove_file(&bin).unwrap(),
            "bytes" => std::fs::write(&bin, vec![0x99; 2048 * 16]).unwrap(),
            "plan" => plan.source_layout.tracks[0].number = 2,
            _ => unreachable!(),
        }
        let result = execute_chd_conversion(
            &plan,
            TrustedRoots::from_paths([dir.path()]),
            &dir.path().join("journal"),
            dir.path(),
            &AtomicBool::new(false),
        );
        assert!(
            matches!(result, Err(ChdConversionError::StaleSource(_))),
            "{change}: {result:?}"
        );
        assert!(!marker.exists(), "{change}");
        assert!(!target.exists(), "{change}");
        assert!(!dir.path().join("journal").exists(), "{change}");
        assert!(
            !std::fs::read_dir(dir.path()).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(DISC_CONVERSION_STAGING_PREFIX)),
            "{change}"
        );
    }
}

#[test]
fn real_chdman_accepts_benign_annotations_without_weakening_verification() {
    if !chdman_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (cue, bin) = source(dir.path());
    let text = format!(
        "REM COMMENT original dump\nREM GENRE Game\nTITLE \"Example disc\"\n{}",
        std::fs::read_to_string(&cue).unwrap()
    );
    std::fs::write(&cue, &text).unwrap();
    let bytes = std::fs::read(&bin).unwrap();
    let target = dir.path().join("output.chd");
    let plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    )
    .unwrap();
    layout_contract::verify_output_layout(&target, 16).unwrap();
    layout_contract::verify_output_storage(&target, 16).unwrap();
    assert_eq!(std::fs::read_to_string(cue).unwrap(), text);
    assert_eq!(std::fs::read(bin).unwrap(), bytes);
}
