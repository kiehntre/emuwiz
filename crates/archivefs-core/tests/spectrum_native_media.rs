//! PZX / NEX / UDI / MDR / RZX through the production scanner and preview:
//! accepted only after structural proof, recordings and unsupported variants
//! never catalogued, and originals never modified.

use archivefs_core::ingestion::preview::{MediaDisposition, preview_media};
use archivefs_core::{ArchiveScanner, Config, SkipReason};
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn block(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}
fn pzx() -> Vec<u8> {
    let mut data = 16u32.to_le_bytes().to_vec();
    data.extend_from_slice(&[0, 0, 1, 1, 0x57, 0x03, 0xAE, 0x06, 0xFF, 0x00]);
    let mut out = block(b"PZXT", &[1, 0]);
    out.extend(block(b"DATA", &data));
    out
}
fn nex(version: &[u8; 4]) -> Vec<u8> {
    let mut h = vec![0u8; 512];
    h[0..4].copy_from_slice(b"Next");
    h[4..8].copy_from_slice(version);
    h[9] = 1;
    h[18 + 5] = 1;
    h.extend(std::iter::repeat_n(0, 16384));
    h
}
fn udi() -> Vec<u8> {
    let mut b = b"UDI!\0\0\0\0\0\0\0\0\0\0\0\0".to_vec();
    b.extend_from_slice(&[0, 16, 0]);
    b.extend_from_slice(&[0x4E; 16]);
    b.extend_from_slice(&[0, 0]);
    let declared = b.len() as u32;
    b[4..8].copy_from_slice(&declared.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b
}
fn sum255(bytes: &[u8]) -> u8 {
    (bytes.iter().map(|b| u32::from(*b)).sum::<u32>() % 255) as u8
}
fn mdr() -> Vec<u8> {
    let mut b = Vec::new();
    for n in [2u8, 1] {
        let mut sector = vec![0u8; 543];
        sector[0] = 1;
        sector[1] = n;
        sector[14] = sum255(&sector[..14]);
        sector[29] = sum255(&sector[15..29]);
        sector[542] = sum255(&sector[30..542]);
        b.extend(sector);
    }
    b.push(0);
    b
}
/// The right Microdrive size with no valid checksum anywhere.
fn sized_like_mdr() -> Vec<u8> {
    let mut b = vec![0u8; 543 * 2 + 1];
    b[0] = 1;
    b[1] = 1;
    b[14] = 77;
    b
}
fn rzx() -> Vec<u8> {
    let mut b = b"RZX!\0\x0d\0\0\0\0".to_vec();
    b.push(0x80);
    b.extend_from_slice(&10u32.to_le_bytes());
    b.extend_from_slice(&[0; 5]);
    b
}

fn config(root: &Path) -> Config {
    Config {
        source_folders: vec![root.into()],
        mount_root: root.join("mount"),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    }
}

#[test]
fn native_spectrum_media_is_catalogued_only_after_structural_proof() {
    let t = tempfile::tempdir().unwrap();
    // A neutral folder: no alias supplies the platform.
    let root = t.path().join("incoming");
    std::fs::create_dir(&root).unwrap();
    let mut good = pzx();
    good.truncate(good.len() - 1);
    let fixtures: Vec<(&str, Vec<u8>)> = vec![
        ("tape.pzx", pzx()),
        ("next.nex", nex(b"V1.2")),
        ("disk.udi", udi()),
        ("cart.mdr", mdr()),
        ("session.rzx", rzx()),
        ("cut.pzx", good),
        ("future.nex", nex(b"V9.9")),
        ("sized.mdr", sized_like_mdr()),
    ];
    for (name, bytes) in &fixtures {
        std::fs::write(root.join(name), bytes).unwrap();
    }
    let c = config(&root);
    let scan = ArchiveScanner::new(&c)
        .scan_archives_with_summary()
        .unwrap();
    let mut names: Vec<_> = scan
        .archives
        .iter()
        .map(|a| a.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "cart.mdr",
            "disk.udi",
            "next.nex",
            "session.rzx",
            "tape.pzx"
        ]
    );
    assert!(
        scan.skipped_files
            .iter()
            .any(|f| f.path.ends_with("cut.pzx") && f.reason == SkipReason::InvalidContent)
    );
    assert_eq!(
        scan.archives
            .iter()
            .find(|a| a.path.ends_with("session.rzx"))
            .unwrap()
            .kind,
        archivefs_core::ArchiveKind::PreservationRecording
    );
    let reason = |name: &str| {
        scan.skipped_files
            .iter()
            .find(|f| f.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} not skipped"))
            .reason
    };
    assert_eq!(reason("future.nex"), SkipReason::UnsupportedVariant);
    assert_eq!(reason("sized.mdr"), SkipReason::AmbiguousPlatform);

    let preview =
        preview_media(&c, &[], &[], &[], &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(preview.count(MediaDisposition::Eligible), 5);
    let explain = |name: &str| {
        preview
            .items
            .iter()
            .find(|i| i.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} missing from preview"))
            .explanation
            .clone()
    };
    assert!(explain("session.rzx").contains("preservation-only gameplay recording"));
    assert!(explain("tape.pzx").contains("no game identity"));
    assert!(explain("future.nex").contains("Unsupported variant"));
    assert!(explain("cut.pzx").contains("truncated"));
    assert!(explain("sized.mdr").contains("AmbiguousPlatform"));
    assert!(explain("sized.mdr").contains("size alone is not Spectrum evidence"));
    for (name, bytes) in fixtures {
        assert_eq!(
            std::fs::read(root.join(name)).unwrap(),
            bytes,
            "{name} changed"
        );
    }
}

#[test]
fn next_requirement_and_partial_status_reach_the_discovery_explanation() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for (name, bytes, expect) in [
        ("n.nex", nex(b"V1.1"), "requires ZX Spectrum Next"),
        ("d.udi", udi(), "partially understood"),
        ("m.mdr", mdr(), "structurally validated"),
    ] {
        let path = root.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let item =
            archivefs_core::ingestion::discovery::inspect_spectrum_media(&path, root).unwrap();
        assert_eq!(
            item.validation_state,
            archivefs_core::ingestion::ValidationState::Accepted
        );
        assert_eq!(item.platform_hint.as_deref(), Some("ZX Spectrum"));
        assert!(item.explanation.contains(expect), "{}", item.explanation);
        assert!(item.explanation.contains("no emulator is implied"));
    }
}
