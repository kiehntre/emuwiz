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

#[test]
fn metadata_only_cue_edits_after_preview_are_stale_before_the_converter() {
    use std::os::unix::fs::PermissionsExt;
    const WITH_METADATA: &str = "REM GENRE Data\nREM COMMENT \"ripped by hand\"\nTITLE \"Disc ü\"\nFILE \"Track space ü.bin\" BINARY\nTRACK 01 MODE1/2048\n  TITLE \"Track\"\nINDEX 01 00:00:00\n";
    for change in ["title", "comment", "removed", "added"] {
        let dir = tempdir().unwrap();
        let (cue, _bin) = source(dir.path());
        std::fs::write(&cue, WITH_METADATA).unwrap();
        let tool = dir.path().join("converter");
        let marker = dir.path().join("executed");
        std::fs::write(
            &tool,
            format!("#!/bin/sh\ntouch '{}'\nexit 99\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let target = dir.path().join("output.chd");
        let plan = build_chd_conversion_plan(
            &cue,
            &target,
            ChdConversionSourceMode::KeepSource,
            Some(&tool),
        )
        .unwrap();
        let changed = match change {
            "title" => WITH_METADATA.replace("Disc ü", "Other"),
            "comment" => WITH_METADATA.replace("ripped by hand", "edited"),
            "removed" => WITH_METADATA.replace("REM GENRE Data\n", ""),
            _ => format!("REM COMMENT late\n{WITH_METADATA}"),
        };
        std::fs::write(&cue, changed).unwrap();
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
    }
}

// --- Transaction-state discipline -------------------------------------------
//
// The shared apply engine returns `Ok` with the journaled transaction in
// `ApplyFailed` when an entry fails after the whole-batch preflight. These
// tests force that inner failure through the executor's pre-rename seam and
// prove conversion never treats the outer `Ok` as publication or quarantine.

use crate::dat::rename_apply::executor::set_test_before_rename_hook;

const BLOCKED_TARGET_NAME: &str = "converted.chd";
const FOREIGN_BYTES: &[u8] = b"foreign object that appeared during apply";

fn staging_leftovers(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(DISC_CONVERSION_STAGING_PREFIX)
        })
        .collect()
}

/// Occupies the reviewed target between the whole-batch preflight and the
/// no-clobber rename of the staged CHD (`<dir>/.emuwiz-chd-*/output.chd`).
fn occupy_target_before_output_rename(staged: &Path) {
    let dir = staged.parent().unwrap().parent().unwrap();
    std::fs::write(dir.join(BLOCKED_TARGET_NAME), FOREIGN_BYTES).unwrap();
}

#[test]
fn failed_output_publication_is_an_error_and_never_quarantines_the_source() {
    if !chdman_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (cue, bin) = source(dir.path());
    let target = dir.path().join(BLOCKED_TARGET_NAME);
    let plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::QuarantineSource,
        Some(std::path::Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    let before_cue = std::fs::read(&cue).unwrap();
    let before_bin = std::fs::read(&bin).unwrap();

    set_test_before_rename_hook(Some(occupy_target_before_output_rename));
    let outcome = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    );
    set_test_before_rename_hook(None);

    let error = outcome.expect_err("an unpublished CHD must never be reported as a conversion");
    assert!(
        matches!(error, ChdConversionError::Transaction(_)),
        "{error:?}"
    );
    // The original media is exactly where and what it was.
    assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
    assert_eq!(std::fs::read(&bin).unwrap(), before_bin);
    assert!(
        !dir.path().join(DISC_CONVERSION_QUARANTINE_SUBDIR).exists(),
        "no source quarantine may be attempted after a failed publication"
    );
    // The object at the target is the foreign one, untouched; it is not ours.
    assert_eq!(std::fs::read(&target).unwrap(), FOREIGN_BYTES);
    assert_eq!(staging_leftovers(dir.path()), Vec::<PathBuf>::new());
}

#[test]
fn failed_output_publication_is_not_completed_by_the_safe_lane() {
    if !chdman_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (cue, bin) = source(dir.path());
    let target = dir.path().join(BLOCKED_TARGET_NAME);
    let plan = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(std::path::Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    let before_cue = std::fs::read(&cue).unwrap();
    let before_bin = std::fs::read(&bin).unwrap();

    set_test_before_rename_hook(Some(occupy_target_before_output_rename));
    let outcome = execute_safe_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    );
    set_test_before_rename_hook(None);

    // A foreign file at the destination must never be measured and reported
    // as the `Completed` conversion output.
    let error = outcome.expect_err("a foreign destination is not a completed conversion");
    assert!(
        matches!(error, ChdConversionError::Transaction(_)),
        "{error:?}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), FOREIGN_BYTES);
    assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
    assert_eq!(std::fs::read(&bin).unwrap(), before_bin);
    assert_eq!(staging_leftovers(dir.path()), Vec::<PathBuf>::new());
}

thread_local! {
    static SOURCE_QUARANTINE_MOVES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Lets the staged output and the first source move through, then occupies
/// the second source's quarantine destination so that move fails after the
/// first source has already left the library. The seam is one-shot, so it
/// re-arms itself until then.
fn occupy_second_source_quarantine_destination(moving: &Path) {
    let parent = moving.parent().unwrap();
    let is_staged_output = parent
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(DISC_CONVERSION_STAGING_PREFIX);
    if !is_staged_output {
        let seen = SOURCE_QUARANTINE_MOVES.with(|moves| {
            moves.set(moves.get() + 1);
            moves.get()
        });
        if seen == 2 {
            let bucket = std::fs::read_dir(parent.join(DISC_CONVERSION_QUARANTINE_SUBDIR))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            std::fs::write(bucket.join(moving.file_name().unwrap()), FOREIGN_BYTES).unwrap();
            return;
        }
    }
    set_test_before_rename_hook(Some(occupy_second_source_quarantine_destination));
}

#[test]
fn failed_source_quarantine_restores_sources_keeps_the_chd_and_needs_attention() {
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
    let before_cue = std::fs::read(&cue).unwrap();
    let before_bin = std::fs::read(&bin).unwrap();

    SOURCE_QUARANTINE_MOVES.with(|moves| moves.set(0));
    set_test_before_rename_hook(Some(occupy_second_source_quarantine_destination));
    let outcome = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &journal,
        dir.path(),
        &AtomicBool::new(false),
    );
    set_test_before_rename_hook(None);

    // The scenario really happened: one source moved, the second was refused.
    assert_eq!(SOURCE_QUARANTINE_MOVES.with(std::cell::Cell::get), 2);
    let (result, mut transaction) =
        outcome.expect("a verified, published CHD is kept when source cleanup fails");

    // Typed partial success: never an ordinary completed replacement.
    assert!(result.needs_attention());
    assert!(!result.source_quarantined);
    let ChdSourceCleanup::QuarantineFailed {
        detail,
        sources_restored,
    } = &result.source_cleanup
    else {
        panic!("{:?}", result.source_cleanup);
    };
    assert!(*sources_restored, "{detail}");
    assert!(detail.contains("fully rolled back"), "{detail}");
    assert!(transaction.source_quarantine.is_none());

    // The source that had already moved is back; both are byte-identical.
    assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
    assert_eq!(std::fs::read(&bin).unwrap(), before_bin);
    // Nothing of the original pair is left behind in quarantine.
    let bucket = std::fs::read_dir(dir.path().join(DISC_CONVERSION_QUARANTINE_SUBDIR))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let quarantined: Vec<Vec<u8>> = std::fs::read_dir(&bucket)
        .unwrap()
        .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
        .collect();
    assert_eq!(quarantined, vec![FOREIGN_BYTES.to_vec()]);

    // The verified CHD stays published and is the reviewed conversion.
    assert_eq!(result.target_path, target);
    assert_eq!(
        compare_optical_fingerprints(
            &result.source_fingerprint,
            &fingerprint_chd(&target).unwrap()
        ),
        OpticalFingerprintComparison::Equivalent
    );
    assert_eq!(
        transaction.output.transaction.state,
        crate::dat::rename_apply::model::TransactionState::Applied
    );
    assert_eq!(
        result.transaction_id,
        transaction.output.transaction.transaction_id
    );

    // It remains manageable afterwards through the ordinary Undo path.
    rollback_chd_conversion(&mut transaction, &journal, &AtomicBool::new(false)).unwrap();
    assert!(!target.exists());
    assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
    assert_eq!(std::fs::read(&bin).unwrap(), before_bin);
}

#[test]
fn completed_modes_report_their_source_cleanup_without_needing_attention() {
    if !chdman_available() {
        return;
    }
    for (mode, expected) in [
        (
            ChdConversionSourceMode::KeepSource,
            ChdSourceCleanup::NotRequested,
        ),
        (
            ChdConversionSourceMode::QuarantineSource,
            ChdSourceCleanup::Quarantined,
        ),
    ] {
        let dir = tempdir().unwrap();
        let (cue, _) = source(dir.path());
        let plan = build_chd_conversion_plan(
            &cue,
            &dir.path().join("converted.chd"),
            mode,
            Some(std::path::Path::new("/usr/bin/chdman")),
        )
        .unwrap();
        let (result, _) = execute_chd_conversion(
            &plan,
            TrustedRoots::from_paths([dir.path()]),
            &dir.path().join("journal"),
            dir.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.source_cleanup, expected);
        assert!(!result.needs_attention());
        assert_eq!(
            result.source_quarantined,
            mode == ChdConversionSourceMode::QuarantineSource
        );
    }
}

fn interrupt_source_journal_after_bin_move(moving: &Path) {
    if moving.extension().is_some_and(|ext| ext == "bin") {
        let root = moving.parent().unwrap();
        std::fs::rename(root.join("journal"), root.join("journal-saved")).unwrap();
        std::fs::write(root.join("journal"), FOREIGN_BYTES).unwrap();
    } else {
        set_test_before_rename_hook(Some(interrupt_source_journal_after_bin_move));
    }
}

fn obstruct_source_move_and_restoration(moving: &Path) {
    if moving.extension().is_some_and(|ext| ext == "cue") {
        let root = moving.parent().unwrap();
        let bucket = std::fs::read_dir(root.join(DISC_CONVERSION_QUARANTINE_SUBDIR))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(bucket.join(moving.file_name().unwrap()), FOREIGN_BYTES).unwrap();
        std::fs::write(root.join("Track space ü.bin"), FOREIGN_BYTES).unwrap();
    } else {
        set_test_before_rename_hook(Some(obstruct_source_move_and_restoration));
    }
}

fn recovery_fixture(
    hook: fn(&Path),
) -> (
    tempfile::TempDir,
    ChdConversionResult,
    ChdConversionTransaction,
) {
    let dir = tempdir().unwrap();
    let (cue, _) = source(dir.path());
    let plan = build_chd_conversion_plan(
        &cue,
        &dir.path().join("converted.chd"),
        ChdConversionSourceMode::QuarantineSource,
        Some(Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    set_test_before_rename_hook(Some(hook));
    let outcome = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    );
    set_test_before_rename_hook(None);
    let (result, transaction) = outcome.unwrap();
    assert!(matches!(
        result.source_cleanup,
        ChdSourceCleanup::QuarantineFailed {
            sources_restored: false,
            ..
        }
    ));
    assert!(result.needs_attention());
    assert!(!result.source_quarantined);
    assert_eq!(
        compare_optical_fingerprints(
            &fingerprint_chd(&result.target_path).unwrap(),
            &result.output_fingerprint
        ),
        OpticalFingerprintComparison::Equivalent
    );
    (dir, result, transaction)
}

#[test]
fn outer_source_apply_error_retains_the_unresolved_transaction() {
    if !chdman_available() {
        return;
    }
    let (dir, result, transaction) = recovery_fixture(interrupt_source_journal_after_bin_move);
    assert!(!dir.path().join("Track space ü.bin").exists());
    assert!(dir.path().join("Disc space ü.cue").exists());
    assert!(result.target_path.exists());
    let source = transaction
        .source_quarantine
        .expect("the outer Apply error must retain recovery identity");
    let persisted = crate::dat::rename_apply::read_journal(
        &crate::dat::rename_apply::journal_path(
            &dir.path().join("journal-saved"),
            &source.transaction.transaction_id,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(persisted.transaction_id, source.transaction.transaction_id);
    assert_eq!(persisted.state, TransactionState::Applying);
    let moved = persisted
        .entries
        .iter()
        .find(|entry| {
            entry
                .source_path
                .extension()
                .is_some_and(|ext| ext == "bin")
        })
        .unwrap();
    assert_eq!(
        capture_identity(&moved.destination_path).unwrap(),
        moved.identity
    );
}

#[test]
fn incomplete_source_undo_never_withdraws_the_verified_chd() {
    if !chdman_available() {
        return;
    }
    let (dir, result, mut transaction) = recovery_fixture(obstruct_source_move_and_restoration);
    let chd_bytes = std::fs::read(&result.target_path).unwrap();
    let outcome = rollback_chd_conversion(
        &mut transaction,
        &dir.path().join("journal"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_ne!(
        outcome,
        RollbackResult::FullyRolledBack,
        "source rollback failure is not complete recovery"
    );
    assert_eq!(std::fs::read(&result.target_path).unwrap(), chd_bytes);
    assert_eq!(
        std::fs::read(dir.path().join("Track space ü.bin")).unwrap(),
        FOREIGN_BYTES
    );
    let source = transaction.source_quarantine.as_ref().unwrap();
    assert_eq!(source.transaction.state, TransactionState::RollbackFailed);
    let moved = source
        .transaction
        .entries
        .iter()
        .find(|entry| {
            entry
                .source_path
                .extension()
                .is_some_and(|ext| ext == "bin")
        })
        .unwrap();
    assert_eq!(
        capture_identity(&moved.destination_path).unwrap(),
        moved.identity
    );
    assert_eq!(
        transaction.output.transaction.state,
        TransactionState::Applied
    );
}

#[test]
fn outer_source_apply_error_recovers_from_paired_journals_after_restart() {
    if !chdman_available() {
        return;
    }
    let (dir, result, transaction) = recovery_fixture(interrupt_source_journal_after_bin_move);
    let source_id = transaction
        .source_quarantine
        .as_ref()
        .unwrap()
        .transaction
        .transaction_id
        .clone();
    let output_id = result.transaction_id;
    let journal = dir.path().join("journal");
    let saved = dir.path().join("journal-saved");
    let chd_bytes = std::fs::read(&result.target_path).unwrap();
    drop(transaction);
    // The failed journal path cannot authorize Undo or remove the CHD.
    assert!(load_chd_conversion_transaction(&journal, &output_id).is_err());
    assert_eq!(std::fs::read(&result.target_path).unwrap(), chd_bytes);
    std::fs::remove_file(&journal).unwrap();
    std::fs::rename(&saved, &journal).unwrap();
    let mut recovered = load_chd_conversion_transaction(&journal, &output_id).unwrap();
    let source_transaction = recovered.source_quarantine.as_ref().unwrap();
    assert_eq!(source_transaction.transaction.transaction_id, source_id);
    assert_eq!(
        source_transaction.transaction.state,
        TransactionState::Applying
    );
    assert_eq!(
        rollback_chd_conversion(&mut recovered, &journal, &AtomicBool::new(false)).unwrap(),
        RollbackResult::FullyRolledBack
    );
    let expected_dir = tempdir().unwrap();
    let (expected_cue, expected_bin) = source(expected_dir.path());
    assert_eq!(
        std::fs::read(dir.path().join("Disc space ü.cue")).unwrap(),
        std::fs::read(expected_cue).unwrap()
    );
    assert_eq!(
        std::fs::read(dir.path().join("Track space ü.bin")).unwrap(),
        std::fs::read(expected_bin).unwrap()
    );
    assert!(!result.target_path.exists());
    let (journals, problems) = crate::dat::rename_apply::list_journals(&journal);
    assert!(problems.is_empty());
    assert_eq!(journals.len(), 2);
    assert!(journals.iter().all(RenameTransaction::is_fully_rolled_back));
}

#[test]
fn incomplete_source_rollback_is_preserved_after_restart_and_repeat_undo() {
    if !chdman_available() {
        return;
    }
    let (dir, result, transaction) = recovery_fixture(obstruct_source_move_and_restoration);
    let journal = dir.path().join("journal");
    let source_id = transaction
        .source_quarantine
        .as_ref()
        .unwrap()
        .transaction
        .transaction_id
        .clone();
    let before = std::fs::read(&result.target_path).unwrap();
    drop(transaction);
    for _ in 0..2 {
        let mut recovered =
            load_chd_conversion_transaction(&journal, &result.transaction_id).unwrap();
        assert_eq!(
            recovered
                .source_quarantine
                .as_ref()
                .unwrap()
                .transaction
                .transaction_id,
            source_id
        );
        assert!(matches!(
            rollback_chd_conversion(&mut recovered, &journal, &AtomicBool::new(false)).unwrap(),
            RollbackResult::RollbackFailed { .. }
        ));
        assert_eq!(std::fs::read(&result.target_path).unwrap(), before);
        assert_eq!(
            recovered.output.transaction.state,
            TransactionState::Applied
        );
        assert_eq!(
            recovered
                .source_quarantine
                .as_ref()
                .unwrap()
                .transaction
                .state,
            TransactionState::RollbackFailed
        );
    }
}

fn completed_fixture(
    mode: ChdConversionSourceMode,
) -> (
    tempfile::TempDir,
    ChdConversionResult,
    ChdConversionTransaction,
) {
    let dir = tempdir().unwrap();
    let (cue, _) = source(dir.path());
    let plan = build_chd_conversion_plan(
        &cue,
        &dir.path().join("converted.chd"),
        mode,
        Some(Path::new("/usr/bin/chdman")),
    )
    .unwrap();
    let (result, transaction) = execute_chd_conversion(
        &plan,
        TrustedRoots::from_paths([dir.path()]),
        &dir.path().join("journal"),
        dir.path(),
        &AtomicBool::new(false),
    )
    .unwrap();
    (dir, result, transaction)
}

#[test]
fn typed_partial_source_undo_keeps_chd_and_both_journals() {
    if !chdman_available() {
        return;
    }
    let (dir, result, mut transaction) =
        completed_fixture(ChdConversionSourceMode::QuarantineSource);
    let before = std::fs::read(&result.target_path).unwrap();
    std::fs::write(dir.path().join("Track space ü.bin"), FOREIGN_BYTES).unwrap();
    let outcome = rollback_chd_conversion(
        &mut transaction,
        &dir.path().join("journal"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let RollbackResult::PartiallyRolledBack {
        rolled_back,
        failed,
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    assert_eq!(rolled_back, vec![dir.path().join("Disc space ü.cue")]);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, dir.path().join("Track space ü.bin"));
    assert_eq!(std::fs::read(&result.target_path).unwrap(), before);
    assert_eq!(
        std::fs::read(dir.path().join("Track space ü.bin")).unwrap(),
        FOREIGN_BYTES
    );
    assert!(dir.path().join("Disc space ü.cue").is_file());
    let source = transaction.source_quarantine.as_ref().unwrap();
    let bin = source
        .transaction
        .entries
        .iter()
        .find(|entry| {
            entry
                .source_path
                .extension()
                .is_some_and(|ext| ext == "bin")
        })
        .unwrap();
    assert_eq!(
        capture_identity(&bin.destination_path).unwrap(),
        bin.identity
    );
    let (journals, problems) = crate::dat::rename_apply::list_journals(&dir.path().join("journal"));
    assert!(problems.is_empty());
    assert_eq!(journals.len(), 2);
    assert_eq!(
        transaction.output.transaction.state,
        TransactionState::Applied
    );
}

#[test]
fn cancelled_source_undo_retains_chd_and_can_be_explicitly_retried() {
    if !chdman_available() {
        return;
    }
    let (dir, result, mut transaction) =
        completed_fixture(ChdConversionSourceMode::QuarantineSource);
    let journal = dir.path().join("journal");
    let before = std::fs::read(&result.target_path).unwrap();
    assert_ne!(
        rollback_chd_conversion(&mut transaction, &journal, &AtomicBool::new(true)).unwrap(),
        RollbackResult::FullyRolledBack
    );
    assert_eq!(std::fs::read(&result.target_path).unwrap(), before);
    assert!(!dir.path().join("Track space ü.bin").exists());
    drop(transaction);
    let mut recovered = load_chd_conversion_transaction(&journal, &result.transaction_id).unwrap();
    assert_eq!(
        rollback_chd_conversion(&mut recovered, &journal, &AtomicBool::new(false)).unwrap(),
        RollbackResult::FullyRolledBack
    );
    assert!(!result.target_path.exists());
    assert!(dir.path().join("Track space ü.bin").is_file());
}

#[test]
fn normal_undo_survives_restart_in_both_source_modes() {
    if !chdman_available() {
        return;
    }
    for mode in [
        ChdConversionSourceMode::KeepSource,
        ChdConversionSourceMode::QuarantineSource,
    ] {
        let (dir, result, transaction) = completed_fixture(mode);
        drop(transaction);
        let journal = dir.path().join("journal");
        let mut recovered =
            load_chd_conversion_transaction(&journal, &result.transaction_id).unwrap();
        assert_eq!(
            rollback_chd_conversion(&mut recovered, &journal, &AtomicBool::new(false)).unwrap(),
            RollbackResult::FullyRolledBack
        );
        assert!(!result.target_path.exists());
        assert!(dir.path().join("Track space ü.bin").is_file());
        assert!(dir.path().join("Disc space ü.cue").is_file());
    }
}

#[test]
fn missing_corrupt_or_contradictory_source_journal_cannot_authorize_output_undo() {
    if !chdman_available() {
        return;
    }
    let (dir, result, transaction) = completed_fixture(ChdConversionSourceMode::QuarantineSource);
    let journal = dir.path().join("journal");
    let source = &transaction.source_quarantine.as_ref().unwrap().transaction;
    let source_path = journal_path(&journal, &source.transaction_id).unwrap();
    let before = std::fs::read(&result.target_path).unwrap();
    std::fs::remove_file(&source_path).unwrap();
    assert!(load_chd_conversion_transaction(&journal, &result.transaction_id).is_err());
    std::fs::write(&source_path, b"invalid json").unwrap();
    assert!(load_chd_conversion_transaction(&journal, &result.transaction_id).is_err());
    let mut contradictory = source.clone();
    contradictory
        .unknown
        .insert(CHD_OUTPUT_ID_KEY.into(), "different-output".into());
    write_journal(&journal, &contradictory).unwrap();
    assert!(load_chd_conversion_transaction(&journal, &result.transaction_id).is_err());
    write_journal(&journal, source).unwrap();
    assert!(load_chd_conversion_transaction(&journal, &result.transaction_id).is_ok());
    assert_eq!(std::fs::read(&result.target_path).unwrap(), before);
    assert!(!dir.path().join("Track space ü.bin").exists());
}

#[test]
fn original_bytes_must_be_proven_even_when_the_source_journal_says_rolled_back() {
    if !chdman_available() {
        return;
    }
    let (dir, result, transaction) = completed_fixture(ChdConversionSourceMode::QuarantineSource);
    let journal = dir.path().join("journal");
    let mut source = transaction.source_quarantine.unwrap().transaction;
    assert_eq!(
        rollback_repair_transaction(&mut source, &journal, &AtomicBool::new(false)).unwrap(),
        RollbackResult::FullyRolledBack
    );
    std::fs::write(dir.path().join("Track space ü.bin"), FOREIGN_BYTES).unwrap();
    let before = std::fs::read(&result.target_path).unwrap();
    let mut recovered = load_chd_conversion_transaction(&journal, &result.transaction_id).unwrap();
    assert!(rollback_chd_conversion(&mut recovered, &journal, &AtomicBool::new(false)).is_err());
    assert_eq!(std::fs::read(&result.target_path).unwrap(), before);
    assert_eq!(
        recovered.output.transaction.state,
        TransactionState::Applied
    );
}
