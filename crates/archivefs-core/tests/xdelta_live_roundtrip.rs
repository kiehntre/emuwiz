//! Live xdelta3 round-trip through the supervised EmuWiz apply path.
//!
//! Synthetic data only: the base and target are generated here, and the
//! patch is produced by `xdelta3 -e`. The decode step under test is always
//! EmuWiz's own `apply_standalone_patch`, never a direct `xdelta3 -d`.
//!
//! The test skips when no `/usr/bin/xdelta3` exists, matching how the other
//! external-tool suites in this crate behave on a machine without the
//! backend installed.

archivefs_core::install_test_environment!();

use std::{fs, path::Path, process::Command};

use archivefs_core::standalone_patch::{
    PatchInspectionState, StandalonePatchError, StandalonePatchFormat, apply_standalone_patch,
    build_standalone_patch_apply_plan, inspect_standalone_patch,
};
use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn xdelta3() -> Option<&'static Path> {
    let candidate = Path::new("/usr/bin/xdelta3");
    candidate.is_file().then_some(candidate)
}

/// A deterministic, compressible-but-not-trivial synthetic payload.
fn synthetic(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| {
            let index = index as u64;
            (seed as u64)
                .wrapping_add(index)
                .wrapping_mul(31)
                .wrapping_add(index / 97)
                .to_le_bytes()[0]
        })
        .collect()
}

#[test]
fn live_xdelta3_round_trip_applies_through_the_supervised_path() {
    let Some(tool) = xdelta3() else {
        eprintln!("skipping: no /usr/bin/xdelta3 on this machine");
        return;
    };

    let temp = tempfile::tempdir().expect("temp dir");
    let base_path = temp.path().join("base.bin");
    let target_path = temp.path().join("target.bin");
    let patch_path = temp.path().join("patch.xdelta");
    let output_path = temp.path().join("derived.bin");

    // Target shares a long prefix with base so the delta is a real delta.
    let base = synthetic(7, 64 * 1024);
    let mut target = base.clone();
    target[1024..2048].copy_from_slice(&synthetic(200, 1024));
    target.extend_from_slice(&synthetic(99, 4096));
    fs::write(&base_path, &base).expect("write base");
    fs::write(&target_path, &target).expect("write target");

    let base_sha_before = sha256(&base);
    let target_sha = sha256(&target);

    // Encoding with the real tool is fine; only the decode must be EmuWiz's.
    let encode = Command::new(tool)
        .arg("-e")
        .arg("-s")
        .arg(&base_path)
        .arg(&target_path)
        .arg(&patch_path)
        .status()
        .expect("run xdelta3 -e");
    assert!(encode.success(), "xdelta3 encode failed: {encode:?}");
    let patch_bytes = fs::read(&patch_path).expect("read patch");
    assert!(!patch_bytes.is_empty(), "encoder produced an empty patch");

    let inspection = inspect_standalone_patch(&patch_path).expect("inspect patch");
    assert_eq!(inspection.format, StandalonePatchFormat::XdeltaVcdiff);
    assert_eq!(inspection.state, PatchInspectionState::Valid);

    let plan =
        build_standalone_patch_apply_plan(&inspection, &base_path, &output_path, temp.path())
            .expect("build apply plan");

    let before: Vec<_> = fs::read_dir(temp.path())
        .expect("list before")
        .map(|entry| entry.expect("entry").file_name())
        .collect();

    let result = apply_standalone_patch(&plan).expect("supervised xdelta apply");

    // 1. Output bytes match the target exactly.
    let produced = fs::read(&output_path).expect("read output");
    assert_eq!(sha256(&produced), target_sha, "output differs from target");
    assert_eq!(result.output_sha256, target_sha, "reported hash differs");
    assert_eq!(result.provenance.application, "xdelta3 external applier");

    // 2. The source is untouched.
    assert_eq!(
        sha256(&fs::read(&base_path).expect("re-read base")),
        base_sha_before,
        "base file was modified"
    );

    // 3. No staging residue. The apply path now deliberately leaves two
    //    records beside the output: the provenance sidecar and the terminal
    //    (Completed) recovery journal that makes rollback possible. Anything
    //    else would be a leaked staging file, and no recovery may be pending.
    let mut after: Vec<_> = fs::read_dir(temp.path())
        .expect("list after")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    after.retain(|name| {
        let name = name.to_string_lossy();
        let is_output = name == "derived.bin";
        let is_provenance = name == "derived.bin.emuwiz-patch.json";
        let is_terminal_journal =
            name.starts_with(".emuwiz-patch-output-") && name.ends_with(".json");
        !(is_output || is_provenance || is_terminal_journal)
    });
    after.sort();
    let mut expected = before;
    expected.sort();
    assert_eq!(after, expected, "a staged temp file was left behind");
    assert!(
        temp.path().join("derived.bin.emuwiz-patch.json").is_file(),
        "provenance sidecar missing"
    );
    let (pending, problems) =
        archivefs_core::patch_output_recovery::discover_pending_patch_outputs(temp.path());
    assert!(pending.is_empty(), "apply left pending output recovery");
    assert!(
        problems.is_empty(),
        "unreadable recovery journal: {problems:?}"
    );

    // 4. A second apply over the existing output is refused, and the first
    //    result is left intact.
    let second =
        build_standalone_patch_apply_plan(&inspection, &base_path, &output_path, temp.path());
    match second {
        Err(StandalonePatchError::UnsafeOutput(reason)) => {
            assert!(reason.contains("already exists"), "{reason}");
        }
        Err(other) => panic!("collision must be refused as unsafe output, got {other:?}"),
        Ok(_) => panic!("planning over an existing output must be refused"),
    }
    assert_eq!(
        sha256(&fs::read(&output_path).expect("output still readable")),
        target_sha,
        "refused collision damaged the existing output"
    );
}
