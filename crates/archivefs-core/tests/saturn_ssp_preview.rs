//! Entirely authored synthetic ZIP/CUE/BIN fixtures, no game or patch payloads.
use archivefs_core::saturn_patch_readiness::ssp::*;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn package(root: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = root.join("translation.ssp");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for (name, bytes) in entries {
        zip.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    path
}
fn patch(root: &Path) -> PathBuf {
    package(
        root,
        &[
            ("version.txt", b"1.95.8035.20705\n"),
            (
                "changes.md5",
                b"DEMO.BIN: 11111111111111111111111111111111\n",
            ),
            (
                "DEMO.BIN._DFR",
                b"opaque synthetic payload; deliberately not executable",
            ),
        ],
    )
}
fn system_id() -> [u8; 256] {
    let mut bytes = [b' '; 256];
    bytes[..16].copy_from_slice(b"SEGA SEGASATURN ");
    bytes[16..32].copy_from_slice(b"SEGA ENTERPRISES");
    bytes[32..42].copy_from_slice(b"T-1234G   ");
    bytes[42..48].copy_from_slice(b"V1.000");
    bytes[48..56].copy_from_slice(b"19960101");
    bytes[56..64].copy_from_slice(b"CD-1/1  ");
    bytes[64..74].copy_from_slice(b"JTU       ");
    bytes
}
fn disc(root: &Path) -> PathBuf {
    let cue = root.join("game.cue");
    fs::write(&cue, "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:00:01\nFILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n").unwrap();
    let mut data = vec![0; 2048 * 3];
    data[2048..2304].copy_from_slice(&system_id());
    fs::write(root.join("data.bin"), data).unwrap();
    fs::write(root.join("audio.bin"), vec![0x55; 2352 * 2]).unwrap();
    cue
}

#[test]
fn zip_convention_is_inspected_without_inventing_patch_semantics() {
    let root = tempfile::tempdir().unwrap();
    let path = patch(root.path());
    let report = inspect_package(&path).unwrap();
    assert_eq!(report.members.len(), 3);
    assert_eq!(report.version_text.as_deref(), Some("1.95.8035.20705\n"));
    assert!(
        report
            .members
            .iter()
            .all(|m| m.sha256.as_ref().unwrap().len() == 64)
    );
    assert!(report.changes_md5_text.unwrap().contains("DEMO.BIN"));
}
#[test]
fn ordinary_zip_without_version_is_not_promoted_to_ssp_support() {
    let root = tempfile::tempdir().unwrap();
    let path = package(root.path(), &[("payload", b"unknown")]);
    assert!(inspect_package(&path).unwrap().version_text.is_none());
}
#[test]
fn metadata_named_directories_are_not_declarations() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("directories.ssp");
    let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for name in ["version.txt/", "changes.md5/"] {
        writer
            .add_directory(name, zip::write::SimpleFileOptions::default())
            .unwrap();
    }
    writer.finish().unwrap();
    let report = inspect_package(&path).unwrap();
    assert!(report.version_text.is_none());
    assert!(report.changes_md5_text.is_none());
    assert!(
        report
            .members
            .iter()
            .all(|m| m.directory && m.sha256.is_none())
    );
}
#[test]
fn deflated_metadata_is_crc_checked_and_inspected() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("deflated.ssp");
    let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    writer
        .start_file(
            "version.txt",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer
        .write_all(b"synthetic producer declaration\n")
        .unwrap();
    writer.finish().unwrap();
    assert_eq!(
        inspect_package(&path).unwrap().version_text.as_deref(),
        Some("synthetic producer declaration\n")
    );
}
#[test]
fn inspection_never_relaxes_existing_ssp_application_refusal() {
    use archivefs_core::{
        saturn_patch_apply::{SaturnPatchBinding, review_saturn_patch},
        saturn_patch_readiness::SaturnPatchTarget,
    };
    let root = tempfile::tempdir().unwrap();
    let patch = patch(root.path());
    let cue = disc(root.path());
    let report = preview(&patch, &cue).unwrap();
    let component = &report.manifest.components[0];
    let binding = SaturnPatchBinding {
        target: SaturnPatchTarget::ComponentBin {
            component: component.path.clone(),
            sha256: component.sha256.clone(),
        },
        manifest: report.manifest,
        patch_sha256: report.package.sha256,
        track_number: 1,
        disc_ordinal: 1,
        patch_disc_ordinal: 1,
    };
    let destination = root.path().join("must-not-exist");
    let error = review_saturn_patch(&patch, &destination, &binding).unwrap_err();
    assert_eq!(error.to_string(), "SSP unsupported");
    assert!(!destination.exists());
}
#[test]
fn native_identity_and_full_layout_bind_preview_but_not_compatibility() {
    let root = tempfile::tempdir().unwrap();
    let patch = patch(root.path());
    let cue = disc(root.path());
    let before = [
        fs::read(&cue).unwrap(),
        fs::read(root.path().join("data.bin")).unwrap(),
        fs::read(root.path().join("audio.bin")).unwrap(),
        fs::read(&patch).unwrap(),
    ];
    let report = preview(&patch, &cue).unwrap();
    assert_eq!(report.compatibility, Compatibility::UnknownBaseBinding);
    assert!(!report.can_apply());
    assert_eq!(
        report
            .manifest
            .system_id
            .as_ref()
            .unwrap()
            .fact
            .product_number,
        "T-1234G"
    );
    assert_eq!(report.manifest.tracks[0].index_00_frame, Some(0));
    assert_eq!(report.manifest.tracks[0].in_file_pregap_frames, Some(1));
    assert_eq!(report.manifest.tracks[0].declared_pregap_frames, None);
    assert_eq!(report.manifest.tracks[0].sector_count, 2);
    assert_eq!(report.manifest.components.len(), 2);
    assert_eq!(report.manifest.tracks[1].sector_size, 2352);
    assert_eq!(revalidate(&report).unwrap(), report);
    assert_eq!(
        before,
        [
            fs::read(&cue).unwrap(),
            fs::read(root.path().join("data.bin")).unwrap(),
            fs::read(root.path().join("audio.bin")).unwrap(),
            fs::read(&patch).unwrap()
        ]
    );
    assert!(!root.path().join("output.bin").exists());
    assert!(APPLY_BLOCKER.contains("No output is created or verified"));
}
#[test]
fn shared_bin_index00_and_raw_sector_mode_remain_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let patch = patch(root.path());
    let cue = root.path().join("shared.cue");
    fs::write(&cue, "FILE \"shared.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 00 00:00:02\nINDEX 01 00:00:03\n").unwrap();
    let mut bytes = vec![0x55; 2352 * 4];
    for sector in bytes[..2352 * 2].chunks_mut(2352) {
        sector[..12].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0]);
        sector[15] = 1;
    }
    bytes[16..272].copy_from_slice(&system_id());
    fs::write(root.path().join("shared.bin"), &bytes).unwrap();
    let report = preview(&patch, &cue).unwrap();
    assert_eq!(report.manifest.tracks[0].sector_size, 2352);
    assert_eq!(report.manifest.tracks[0].sector_count, 2);
    assert_eq!(report.manifest.tracks[1].index_00_frame, Some(2));
    assert_eq!(report.manifest.tracks[1].index_01_frame, 3);
    assert_eq!(fs::read(root.path().join("shared.bin")).unwrap(), bytes);
    assert!(!report.can_apply());
}
#[test]
fn titles_and_extensions_cannot_supply_saturn_or_patch_target_identity() {
    let root = tempfile::tempdir().unwrap();
    let patch = patch(root.path());
    let cue = disc(root.path());
    let mut data = fs::read(root.path().join("data.bin")).unwrap();
    data[2048..2064].copy_from_slice(b"NOT A SATURN    ");
    fs::write(root.path().join("data.bin"), data).unwrap();
    assert!(preview(&patch, &cue).is_err());
}
#[test]
fn stale_package_descriptor_audio_and_identity_evidence_refuse_recheck() {
    for change in ["patch", "patch-inode", "cue", "audio", "game", "missing"] {
        let root = tempfile::tempdir().unwrap();
        let patch = patch(root.path());
        let cue = disc(root.path());
        let reviewed = preview(&patch, &cue).unwrap();
        match change {
            "patch" => {
                package(root.path(), &[("version.txt", b"new declaration")]);
            }
            "patch-inode" => {
                let bytes = fs::read(&patch).unwrap();
                fs::rename(&patch, root.path().join("old.ssp")).unwrap();
                fs::write(&patch, bytes).unwrap();
            }
            "cue" => {
                let mut text = fs::read_to_string(&cue).unwrap();
                text.push_str("REM changed\n");
                fs::write(&cue, text).unwrap();
            }
            "audio" => {
                fs::write(root.path().join("audio.bin"), vec![0x66; 2352 * 2]).unwrap();
            }
            "game" => {
                let path = root.path().join("data.bin");
                let mut bytes = fs::read(&path).unwrap();
                bytes[2048 + 32] = b'X';
                fs::write(path, bytes).unwrap();
            }
            "missing" => {
                fs::remove_file(root.path().join("data.bin")).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(revalidate(&reviewed).is_err(), "{change}");
    }
}
#[test]
fn unsupported_optical_representations_never_enable_apply() {
    let root = tempfile::tempdir().unwrap();
    let patch = patch(root.path());
    for name in ["lone.bin", "disc.chd", "disc.gdi"] {
        let p = root.path().join(name);
        fs::write(&p, b"unsupported").unwrap();
        assert!(preview(&patch, &p).is_err());
    }
    let cue = root.path().join("bad.cue");
    fs::write(
        &cue,
        "FILE \"../escape.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    assert!(preview(&patch, &cue).is_err());
}
#[test]
fn malformed_truncated_and_corrupt_zip_payloads_are_not_verified() {
    let root = tempfile::tempdir().unwrap();
    let p = patch(root.path());
    let original = fs::read(&p).unwrap();
    for bytes in [&b"not a ZIP"[..], &original[..original.len() - 10]] {
        fs::write(&p, bytes).unwrap();
        assert!(inspect_package(&p).is_err());
    }
    let mut bytes = original;
    let payload = 30
        + u16::from_le_bytes(bytes[26..28].try_into().unwrap()) as usize
        + u16::from_le_bytes(bytes[28..30].try_into().unwrap()) as usize;
    bytes[payload] ^= 1;
    fs::write(&p, bytes).unwrap();
    assert!(inspect_package(&p).is_err());
}
#[test]
fn traversal_case_alias_and_special_members_are_refused() {
    let root = tempfile::tempdir().unwrap();
    for name in ["../outside", "/absolute", "a\\b", "C:drive", "a//b"] {
        let p = package(root.path(), &[(name, b"x")]);
        assert!(inspect_package(&p).is_err(), "{name}");
    }
    let p = package(root.path(), &[("VERSION.TXT", b"1"), ("version.txt", b"2")]);
    assert!(inspect_package(&p).is_err());
    let p = package(root.path(), &[("dir", b"file"), ("dir/file", b"child")]);
    assert!(inspect_package(&p).is_err());
    let p = root.path().join("link.ssp");
    let mut zip = zip::ZipWriter::new(fs::File::create(&p).unwrap());
    zip.add_symlink("link", "target", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.finish().unwrap();
    assert!(inspect_package(&p).is_err());
}
#[test]
fn bounded_metadata_and_entry_counts_refuse_without_large_allocations() {
    let root = tempfile::tempdir().unwrap();
    let p = package(
        root.path(),
        &[("version.txt", &vec![b'x'; MAX_TEXT_BYTES as usize + 1])],
    );
    assert!(inspect_package(&p).is_err());
    let p = package(root.path(), &[("version.txt", &[0xff, 0xfe])]);
    assert!(inspect_package(&p).is_err());
    let p = patch(root.path());
    let mut bytes = fs::read(&p).unwrap();
    let n = bytes.len();
    bytes[n - 14..n - 10].copy_from_slice(&[255, 255, 255, 255]);
    fs::write(&p, bytes).unwrap();
    assert!(inspect_package(&p).is_err());
}

// Tiny contradictory streams with consistent ZIP header declarations. The
// payload is empty deflate; declared sizes must be refused before decoding.
fn zip_declaring(sizes: &[u64]) -> Vec<u8> {
    fn word(out: &mut Vec<u8>, n: u16) {
        out.extend_from_slice(&n.to_le_bytes());
    }
    fn dword(out: &mut Vec<u8>, n: u32) {
        out.extend_from_slice(&n.to_le_bytes());
    }
    let (mut body, mut central) = (Vec::new(), Vec::new());
    for (i, size) in sizes.iter().enumerate() {
        let name = format!("opaque{i}");
        let offset = body.len() as u32;
        let size32 = (*size).min(u32::MAX as u64) as u32;
        let mut extra = Vec::new();
        if *size >= u32::MAX as u64 {
            word(&mut extra, 1);
            word(&mut extra, 8);
            extra.extend_from_slice(&size.to_le_bytes());
        }
        body.extend_from_slice(b"PK\x03\x04");
        for n in [45, 0, 8, 0, 0] {
            word(&mut body, n);
        }
        for n in [0, 2, size32] {
            dword(&mut body, n);
        }
        word(&mut body, name.len() as u16);
        word(&mut body, extra.len() as u16);
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(&extra);
        body.extend_from_slice(&[3, 0]);
        central.extend_from_slice(b"PK\x01\x02");
        for n in [45, 45, 0, 8, 0, 0] {
            word(&mut central, n);
        }
        for n in [0, 2, size32] {
            dword(&mut central, n);
        }
        for n in [name.len() as u16, extra.len() as u16, 0, 0, 0] {
            word(&mut central, n);
        }
        dword(&mut central, 0);
        dword(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
        central.extend_from_slice(&extra);
    }
    let offset = body.len() as u32;
    body.extend_from_slice(&central);
    body.extend_from_slice(b"PK\x05\x06");
    for n in [0, 0, sizes.len() as u16, sizes.len() as u16] {
        word(&mut body, n);
    }
    dword(&mut body, central.len() as u32);
    dword(&mut body, offset);
    word(&mut body, 0);
    body
}
#[test]
fn declared_zip64_and_aggregate_sizes_refuse_before_decoding() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("oversized.ssp");
    for (sizes, reason) in [
        (vec![u64::MAX], "declared member size"),
        (vec![MAX_PACKAGE_BYTES + 1], "declared member size"),
        (vec![MAX_PACKAGE_BYTES; 3], "declared expanded size"),
    ] {
        let bytes = zip_declaring(&sizes);
        assert!(bytes.len() < 400);
        fs::write(&path, bytes).unwrap();
        let result = std::panic::catch_unwind(|| inspect_package(&path));
        let error = result
            .expect("malformed declarations must not panic")
            .unwrap_err();
        assert!(error.contains(reason), "{error}");
    }
}
#[cfg(unix)]
#[test]
fn symlink_patch_inputs_are_not_followed() {
    let root = tempfile::tempdir().unwrap();
    let p = patch(root.path());
    let alias = root.path().join("alias.ssp");
    std::os::unix::fs::symlink(p, &alias).unwrap();
    assert!(inspect_package(&alias).is_err());
}

#[test]
fn fresh_ssp_context_refuses_ambiguous_gap_evidence_before_apply() {
    let root = tempfile::tempdir().unwrap();
    let patch = patch(root.path());
    let cue = disc(root.path());
    let text = fs::read_to_string(&cue).unwrap();
    fs::write(
        &cue,
        text.replace("INDEX 01 00:00:01", "INDEX 01 00:00:01\nPREGAP 00:00:02"),
    )
    .unwrap();
    let error = preview(&patch, &cue).unwrap_err();
    assert!(error.contains("Ambiguous"), "{error}");
    assert!(!root.path().join("output.bin").exists());
}
