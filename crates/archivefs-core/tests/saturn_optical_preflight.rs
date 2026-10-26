//! Authored, tiny CUE/BIN fixtures only; no downloaded/production disc images.
use archivefs_core::{
    saturn_disc_manifest::{
        SaturnDiscManifest, SaturnManifestIssue, inspect_saturn_disc, verify_saturn_manifest,
    },
    saturn_patch_apply::{SaturnPatchBinding, review_saturn_patch},
    saturn_patch_readiness::{optical::assess, *},
};
use std::{fs, path::Path};

const CUE: &str = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 00 00:00:00\nINDEX 01 00:00:01\nPOSTGAP 00:00:02\n";
fn fixture(root: &Path) -> SaturnDiscManifest {
    let mut data = vec![b' '; 2048 * 3];
    data[..16].copy_from_slice(b"SEGA SEGASATURN ");
    data[0x38..0x40].copy_from_slice(b"CD-1/1  ");
    fs::write(root.join("data.bin"), data).unwrap();
    fs::write(root.join("audio.bin"), vec![0x55; 2352 * 3]).unwrap();
    fs::write(root.join("disc.cue"), CUE).unwrap();
    inspect_saturn_disc(&root.join("disc.cue")).unwrap()
}
fn readiness(root: &Path, manifest: SaturnDiscManifest) -> SaturnPatchReadiness {
    let patch = root.join("patch.ips");
    fs::write(&patch, b"PATCHEOF").unwrap();
    let component = &manifest.components[0];
    assess_saturn_patch_readiness(&SaturnPatchReadinessRequest {
        target: SaturnPatchTarget::ComponentBin {
            component: component.path.clone(),
            sha256: component.sha256.clone(),
        },
        manifest,
        expected_manifest_fingerprint: None,
        patch_path: patch,
        provenance: Default::default(),
        expected_output: None,
        external_evidence: vec![],
    })
}
fn refused(root: &Path, manifest: SaturnDiscManifest, reason: &str) {
    let result = readiness(root, manifest);
    assert_eq!(result.state, SaturnPatchReadinessState::NotReady);
    assert!(!result.audio_expected_untouched && !result.topology_expected_untouched);
    assert!(
        result
            .reasons
            .iter()
            .any(|r| r.to_string().contains(reason)),
        "{reason}: {:?}",
        result.reasons
    );
}
#[test]
fn valid_source_is_current_but_does_not_claim_disc_equivalence_or_patch_safety() {
    let root = tempfile::tempdir().unwrap();
    let manifest = fixture(root.path());
    let before = fs::read(&manifest.source_descriptor).unwrap();
    let optical = assess(&manifest);
    assert!(optical.refusals.is_empty(), "{:?}", optical.refusals);
    assert!(optical.limits[0].contains("no patch output, full disc equivalence"));
    let result = readiness(root.path(), manifest.clone());
    // IPS does not declare source/output sizes, so structural success cannot
    // establish that patching will preserve the track length.
    assert!(
        result
            .reasons
            .contains(&SaturnPatchReadinessReason::TrackTopologyImpactUnknown)
    );
    assert_eq!(result.state, SaturnPatchReadinessState::NotReady);
    assert_eq!(fs::read(&manifest.source_descriptor).unwrap(), before);
}
#[test]
fn changed_same_size_audio_and_stored_gap_bytes_are_stale() {
    for offset in [0, 2352] {
        let root = tempfile::tempdir().unwrap();
        let manifest = fixture(root.path());
        let path = root.path().join("audio.bin");
        let mut bytes = fs::read(&path).unwrap();
        bytes[offset] ^= 1;
        fs::write(&path, bytes).unwrap();
        refused(root.path(), manifest, "ComponentHashMismatch");
    }
    let root = tempfile::tempdir().unwrap();
    let manifest = fixture(root.path());
    let mut bytes = fs::read(root.path().join("audio.bin")).unwrap();
    bytes[2352] ^= 1;
    fs::write(root.path().join("audio.bin"), bytes).unwrap();
    refused(root.path(), manifest, "AudioHashMismatch");
}
#[test]
fn missing_and_case_ambiguous_companions_are_never_guessed() {
    for names in [vec![], vec!["Audio.bin", "AUDIO.BIN"]] {
        let root = tempfile::tempdir().unwrap();
        let manifest = fixture(root.path());
        fs::remove_file(root.path().join("audio.bin")).unwrap();
        for name in &names {
            fs::write(root.path().join(name), vec![0; 2352 * 3]).unwrap();
        }
        refused(
            root.path(),
            manifest,
            if names.is_empty() {
                "Missing"
            } else {
                "Ambiguous"
            },
        );
    }
}
#[test]
fn malformed_sheet_and_truncated_companion_have_specific_refusals() {
    let root = tempfile::tempdir().unwrap();
    let manifest = fixture(root.path());
    fs::write(
        root.path().join("disc.cue"),
        CUE.replace("00:00:01", "00:60:00"),
    )
    .unwrap();
    refused(root.path(), manifest, "malformed CUE");
    let root = tempfile::tempdir().unwrap();
    let manifest = fixture(root.path());
    fs::write(root.path().join("audio.bin"), vec![0; 2353]).unwrap();
    refused(root.path(), manifest, "Optical layout");
}
#[test]
fn fresh_dual_gap_manifest_cannot_authorize_readiness_or_patch_review() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let cue = root.path().join("disc.cue");
    fs::write(
        &cue,
        CUE.replace("POSTGAP 00:00:02", "PREGAP 00:00:01\nPOSTGAP 00:00:02"),
    )
    .unwrap();
    let manifest = inspect_saturn_disc(&cue).unwrap();
    refused(root.path(), manifest.clone(), "Ambiguous");
    // Exact fresh hashes and explicit target do not clear ambiguous semantics.
    let patch_dir = tempfile::tempdir().unwrap();
    let patch = patch_dir.path().join("patch.ips");
    fs::write(&patch, b"PATCHEOF").unwrap();
    let inspection = archivefs_core::standalone_patch::inspect_standalone_patch(&patch).unwrap();
    let component = &manifest.components[0];
    let binding = SaturnPatchBinding {
        target: SaturnPatchTarget::ComponentBin {
            component: component.path.clone(),
            sha256: component.sha256.clone(),
        },
        manifest,
        patch_sha256: inspection.patch_sha256,
        track_number: 1,
        disc_ordinal: 1,
        patch_disc_ordinal: 1,
    };
    let destination = patch_dir.path().join("must-not-exist");
    let error = review_saturn_patch(&patch, &destination, &binding).unwrap_err();
    assert!(error.to_string().contains("Ambiguous"), "{error}");
    assert!(!destination.exists());
}
#[test]
fn extra_indexes_and_unrepresented_directives_refuse_fresh_evidence() {
    for (needle, replacement, reason) in [
        ("POSTGAP 00:00:02", "INDEX 02 00:00:02", "INDEX"),
        ("TRACK 02 AUDIO", "TRACK 02 AUDIO\nFLAGS PRE", "FLAGS"),
        ("TRACK 02 AUDIO", "TRACK 03 AUDIO", "Optical layout"),
    ] {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path());
        let cue = root.path().join("disc.cue");
        fs::write(&cue, CUE.replace(needle, replacement)).unwrap();
        let manifest = inspect_saturn_disc(&cue).unwrap();
        refused(root.path(), manifest, reason);
    }
}
#[test]
fn postgap_index_pregap_mode_and_order_changes_are_reported() {
    for (needle, replacement, reason) in [
        ("POSTGAP 00:00:02", "POSTGAP 00:00:03", "POSTGAP changed"),
        (
            "INDEX 01 00:00:01",
            "INDEX 01 00:00:02",
            "UnexpectedIndexChange",
        ),
        (
            "INDEX 00 00:00:00\nINDEX 01 00:00:01",
            "PREGAP 00:00:01\nINDEX 01 00:00:00",
            "UnexpectedPregapChange",
        ),
        (
            "TRACK 02 AUDIO",
            "TRACK 02 MODE1/2352",
            "raw sector layout is not verified",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let manifest = fixture(root.path());
        fs::write(
            root.path().join("disc.cue"),
            CUE.replace(needle, replacement),
        )
        .unwrap();
        refused(root.path(), manifest, reason);
    }
    let root = tempfile::tempdir().unwrap();
    let mut manifest = fixture(root.path());
    manifest.tracks.swap(0, 1);
    assert!(
        verify_saturn_manifest(&manifest)
            .issues
            .iter()
            .any(|i| matches!(i, SaturnManifestIssue::TrackTopologyMismatch { .. }))
    );
}
#[test]
fn an_unused_file_reference_is_not_omitted_from_patch_admission() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let cue = root.path().join("disc.cue");
    fs::write(&cue, format!("{CUE}FILE \"unused.bin\" BINARY\n")).unwrap();
    fs::write(root.path().join("unused.bin"), [0; 2352]).unwrap();
    let manifest = inspect_saturn_disc(&cue).unwrap();
    refused(root.path(), manifest, "absent from the reviewed manifest");
}
#[cfg(unix)]
#[test]
fn symlink_companion_replacement_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let manifest = fixture(root.path());
    fs::rename(root.path().join("audio.bin"), root.path().join("other.bin")).unwrap();
    std::os::unix::fs::symlink("other.bin", root.path().join("audio.bin")).unwrap();
    refused(root.path(), manifest, "unsafe");
}

#[test]
fn synthetic_pregap_and_postgap_are_distinct_supported_source_evidence() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let cue = root.path().join("disc.cue");
    fs::write(
        &cue,
        CUE.replace(
            "INDEX 00 00:00:00\nINDEX 01 00:00:01",
            "PREGAP 00:00:01\nINDEX 01 00:00:00",
        ),
    )
    .unwrap();
    let manifest = inspect_saturn_disc(&cue).unwrap();
    assert_eq!(manifest.tracks[1].declared_pregap_frames, Some(1));
    assert_eq!(manifest.tracks[1].postgap_frames, Some(2));
    assert_eq!(manifest.tracks[1].in_file_pregap_frames, None);
    assert!(assess(&manifest).refusals.is_empty());
}
#[cfg(unix)]
#[test]
fn replaced_nonregular_descriptor_is_refused_before_parsing() {
    let root = tempfile::tempdir().unwrap();
    let manifest = fixture(root.path());
    fs::remove_file(&manifest.source_descriptor).unwrap();
    // A directory is nonregular, like a FIFO, without a potentially hanging
    // regression or requiring permission to create a socket in the test sandbox.
    fs::create_dir(&manifest.source_descriptor).unwrap();
    let report = assess(&manifest);
    assert!(report.refusals.iter().any(|r| r.contains("regular CUE")));
}
