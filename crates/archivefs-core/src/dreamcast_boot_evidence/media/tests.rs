use super::*;
use crate::dreamcast_boot_evidence::ip_bin::{IpBinStatus, product_crc16};
use crate::dreamcast_patch_readiness::{DreamcastPatchReadinessState, DreamcastPatchTargetMatch};
use std::io::Write;

// Synthetic header and opaque payload; no retail bootstrap or patch bytes.
fn boot() -> Vec<u8> {
    let mut b = vec![0; IP_BIN_BYTES];
    b[..256].fill(b' ');
    for (offset, value) in [
        (0, b"SEGA SEGAKATANA ".as_slice()),
        (16, b"SEGA ENTERPRISES"),
        (32, b"0000 GD-ROM1/1  "),
        (48, b"JUE     "),
        (56, b"0000010 "),
        (64, b"T-1234M   "),
        (74, b"V1.000"),
        (80, b"20000229"),
        (96, b"1ST_READ.BIN"),
        (128, b"SYNTHETIC"),
    ] {
        b[offset..offset + value.len()].copy_from_slice(value);
    }
    let crc = format!("{:04X}", product_crc16(&b[64..80]));
    b[32..36].copy_from_slice(crc.as_bytes());
    b
}
fn iso(root: &Path) -> PathBuf {
    let path = root.join("synthetic.iso");
    fs::write(&path, boot()).unwrap();
    path
}
fn package(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    zip.start_file("1ST_READ.BIN", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"freely created synthetic replacement")
        .unwrap();
    zip.finish().unwrap();
    path
}
fn gdi(root: &Path, offset: i64) -> PathBuf {
    let path = root.join("synthetic.gdi");
    fs::write(root.join("low.iso"), vec![0u8; 2048]).unwrap();
    fs::write(root.join("audio.raw"), vec![0u8; 2352]).unwrap();
    fs::write(root.join("game.iso"), boot()).unwrap();
    fs::write(
        &path,
        format!(
            "3\n1 0 4 2048 low.iso 0\n2 450 0 2352 audio.raw 0\n3 45000 4 2048 game.iso {offset}\n"
        ),
    )
    .unwrap();
    path
}
#[test]
fn dreamcast_media_cooked_metadata_and_source_bytes_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let path = iso(root.path());
    let image = inspect_image(&path).unwrap();
    let meta = &image.inspection.ip_bin.unwrap().metadata;
    assert_eq!(meta.product_number.value, "T-1234M");
    assert_eq!(meta.area_symbols.value, "JUE");
    assert_eq!(meta.boot_filename.value, "1ST_READ.BIN");
    assert_eq!(fs::read(path).unwrap(), boot());
}
#[test]
fn dreamcast_media_malformed_truncated_and_unsupported_are_truthful() {
    let root = tempfile::tempdir().unwrap();
    let path = iso(root.path());
    fs::write(&path, vec![0; 2048]).unwrap();
    assert!(inspect_image(&path).is_err());
    let mut bad = boot();
    bad[96] = 1;
    fs::write(&path, bad).unwrap();
    assert_eq!(
        inspect_image(&path).unwrap().inspection.status,
        IpBinStatus::Malformed
    );
    for ext in ["cdi", "chd", "cue", "bin"] {
        let p = root.path().join(format!("unsupported.{ext}"));
        fs::write(&p, boot()).unwrap();
        assert!(
            inspect_image(&p)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
    }
}
#[test]
fn dreamcast_media_gdi_preserves_all_tracks_and_refuses_unproven_offsets() {
    let root = tempfile::tempdir().unwrap();
    let path = gdi(root.path(), 0);
    let image = inspect_image(&path).unwrap();
    assert_eq!(image.files.len(), 4);
    let desc = image.topology.unwrap();
    assert_eq!(desc.tracks[2].start_lba, 45000);
    assert_eq!(desc.tracks[1].track_type, gdi::GdiTrackType::Audio);
    gdi(root.path(), 2048);
    assert!(
        inspect_image(&path)
            .unwrap_err()
            .to_string()
            .contains("offset")
    );
}
#[test]
fn dreamcast_media_missing_audio_and_escaping_track_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let path = gdi(root.path(), 0);
    fs::remove_file(root.path().join("audio.raw")).unwrap();
    assert!(inspect_image(&path).is_err());
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("audio.raw", "../outside.raw");
    fs::write(&path, text).unwrap();
    assert!(inspect_image(&path).is_err());
}
#[test]
fn dreamcast_media_dcp_preview_never_infers_target_or_writes_output() {
    let root = tempfile::tempdir().unwrap();
    let path = iso(root.path());
    let patch = package(root.path(), "T-1234M.dcp");
    let original = fs::read(&patch).unwrap();
    let image = inspect_image(&path).unwrap();
    let review = review_patch(&image, &patch).unwrap();
    assert_eq!(review.state, DreamcastPatchReadinessState::NotReady);
    assert_eq!(review.target_match, DreamcastPatchTargetMatch::NoExactMatch);
    assert_eq!(review.package.entries.len(), 1);
    assert!(!review.package.source_hashes_embedded);
    assert_eq!(fs::read(path).unwrap(), boot());
    assert_eq!(fs::read(patch).unwrap(), original);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}
#[test]
fn dreamcast_media_changed_source_evidence_invalidates_package_preview() {
    let root = tempfile::tempdir().unwrap();
    let path = gdi(root.path(), 0);
    let patch = package(root.path(), "p.dcp");
    let image = inspect_image(&path).unwrap();
    let mut bytes = boot();
    bytes[128] = b'X';
    fs::write(root.path().join("game.iso"), bytes).unwrap();
    assert!(
        review_patch(&image, &patch)
            .unwrap_err()
            .to_string()
            .contains("STALE SOURCE")
    );
    let image = inspect_image(&path).unwrap();
    fs::write(root.path().join("audio.raw"), vec![1; 2352 * 2]).unwrap();
    assert!(review_patch(&image, &patch).is_err());
}
#[test]
fn dreamcast_media_package_reinspection_hashes_changed_bytes_and_refuses_bad_zip() {
    let root = tempfile::tempdir().unwrap();
    let path = iso(root.path());
    let patch = package(root.path(), "p.dcp");
    let image = inspect_image(&path).unwrap();
    let first = review_patch(&image, &patch).unwrap();
    let second = package(root.path(), "other.dcp");
    fs::write(&patch, b"not a ZIP").unwrap();
    assert!(review_patch(&image, &patch).is_err());
    assert_eq!(
        first.package.package_sha256,
        review_patch(&image, &second)
            .unwrap()
            .package
            .package_sha256
    );
}
#[cfg(unix)]
#[test]
fn dreamcast_media_replaced_inode_and_symlink_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let path = iso(root.path());
    let patch = package(root.path(), "p.dcp");
    let image = inspect_image(&path).unwrap();
    let replacement = root.path().join("new.iso");
    fs::write(&replacement, boot()).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert!(review_patch(&image, &patch).is_err());
    let link = root.path().join("link.iso");
    std::os::unix::fs::symlink(path, &link).unwrap();
    assert!(inspect_image(&link).is_err());
}

#[test]
fn dreamcast_media_raw2352_gdi_maps_boot_payload_and_refuses_corrupt_sectors() {
    let root = tempfile::tempdir().unwrap();
    let path = gdi(root.path(), 0);
    let mut raw = Vec::new();
    for payload in boot().chunks_exact(2048) {
        let mut sector = vec![0; 2352];
        sector[1..11].fill(0xff);
        sector[15] = 1;
        sector[16..2064].copy_from_slice(payload);
        raw.extend(sector);
    }
    let descriptor = fs::read_to_string(&path)
        .unwrap()
        .replace("4 2048 game.iso", "4 2352 game.iso");
    fs::write(&path, descriptor).unwrap();
    fs::write(root.path().join("game.iso"), &raw).unwrap();
    let image = inspect_image(&path).unwrap();
    assert_eq!(image.boot_sha256, hash(&boot()));
    assert_eq!(
        image
            .inspection
            .ip_bin
            .unwrap()
            .metadata
            .product_number
            .value,
        "T-1234M"
    );
    raw[2352 * 10 + 1] = 0;
    fs::write(root.path().join("game.iso"), raw).unwrap();
    assert!(inspect_image(&path).is_err());
}
