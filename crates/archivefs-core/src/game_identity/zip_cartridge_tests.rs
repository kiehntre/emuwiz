//! Zipped cartridge ROM identity: a ZIP with exactly one supported ROM member.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zip::CompressionMethod;
use zip::unstable::write::FileOptionsExt as _;
use zip::write::{SimpleFileOptions, ZipWriter};

use super::*;

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn opts(method: CompressionMethod) -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(method)
}

fn write_zip(dir: &Path, name: &str, members: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join(name);
    let mut writer = ZipWriter::new(fs::File::create(&path).unwrap());
    for (member, bytes) in members {
        writer
            .start_file(*member, opts(CompressionMethod::Deflated))
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    path
}

fn gb_rom(cgb_flag: u8) -> Vec<u8> {
    use crate::gb_header_evidence::compute_header_checksum;
    const LOGO: [u8; 48] = [
        0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00,
        0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD,
        0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB,
        0xB9, 0x33, 0x3E,
    ];
    let mut bytes = vec![0u8; 0x8000];
    bytes[0x104..0x104 + 48].copy_from_slice(&LOGO);
    bytes[0x143] = cgb_flag;
    bytes[0x14D] = compute_header_checksum(&bytes).unwrap();
    bytes
}

fn snes_rom(seed: u8) -> Vec<u8> {
    (0..0x8000_usize)
        .map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed))
        .collect()
}

fn inspect(path: &Path, platform: &str) -> GameIdentityReport {
    inspect_catalogued_game_identity(path, Some(platform))
}

fn refused(report: &GameIdentityReport) -> bool {
    report
        .verified_value(IdentityKind::LooseRomSha256)
        .is_none()
        && !report.complete
        && report
            .evidence
            .iter()
            .any(|e| e.kind == IdentityKind::LooseRomSha256 && e.status != IdentityStatus::Verified)
}

fn status_of(report: &GameIdentityReport) -> IdentityStatus {
    report
        .evidence
        .iter()
        .find(|e| e.kind == IdentityKind::LooseRomSha256)
        .unwrap()
        .status
}

#[test]
fn single_snes_rom_is_hashed_as_member_not_container() {
    let dir = tempfile::tempdir().unwrap();
    let rom = snes_rom(1);
    let zip = write_zip(
        dir.path(),
        "Game (USA).zip",
        &[("Game (USA).sfc", &rom), ("readme.txt", b"hello")],
    );
    let report = inspect(&zip, "SNES");
    assert_eq!(
        report.format,
        IdentityImageFormat::ZipContainingCartridgeRom
    );
    assert_eq!(
        report.verified_value(IdentityKind::LooseRomSha256),
        Some(sha(&rom).as_str())
    );
    assert_ne!(
        report.verified_value(IdentityKind::LooseRomSha256),
        Some(sha(&fs::read(&zip).unwrap()).as_str())
    );
    assert!(report.complete);
    assert!(!report.is_verified_loose_rom(), "not a loose file");
    assert_eq!(report.verified_loose_rom_sha256(), None);
    let item = report
        .evidence
        .iter()
        .find(|e| e.kind == IdentityKind::LooseRomSha256)
        .unwrap();
    assert_eq!(
        item.provenance.member_path.as_deref(),
        Some(&b"Game (USA).sfc"[..])
    );
    assert_eq!(item.provenance.member_index, Some(0));
    assert!(item.diagnostic.contains("hash is of the member"));
}

#[test]
fn single_gba_and_header_valid_gb_zips_are_verified() {
    let dir = tempfile::tempdir().unwrap();
    let gba = b"synthetic-gba-cartridge".to_vec();
    let zip = write_zip(dir.path(), "a.zip", &[("a.gba", &gba)]);
    let r = inspect(&zip, "Game Boy Advance");
    assert_eq!(
        r.verified_value(IdentityKind::LooseRomSha256),
        Some(sha(&gba).as_str())
    );
    let gb = gb_rom(0x00);
    let zip = write_zip(dir.path(), "b.zip", &[("b.gb", &gb)]);
    let r = inspect(&zip, "Game Boy");
    assert_eq!(
        r.verified_value(IdentityKind::LooseRomSha256),
        Some(sha(&gb).as_str())
    );
}

#[test]
fn header_invalid_gb_inside_zip_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let zip = write_zip(dir.path(), "bad.zip", &[("bad.gb", &vec![0u8; 0x8000])]);
    let r = inspect(&zip, "Game Boy");
    assert!(refused(&r));
    assert_eq!(status_of(&r), IdentityStatus::Invalid);
}

#[test]
fn two_candidate_roms_are_ambiguous_and_never_picked() {
    let dir = tempfile::tempdir().unwrap();
    let zip = write_zip(
        dir.path(),
        "two.zip",
        &[("a.sfc", &snes_rom(1)), ("b.smc", &snes_rom(2))],
    );
    let r = inspect(&zip, "SNES");
    assert!(refused(&r));
    assert_eq!(status_of(&r), IdentityStatus::Ambiguous);
}

#[test]
fn no_candidate_and_wrong_platform_extension_are_missing() {
    let dir = tempfile::tempdir().unwrap();
    let zip = write_zip(dir.path(), "n.zip", &[("a.gba", b"x")]);
    let r = inspect(&zip, "SNES");
    assert!(refused(&r));
    assert_eq!(status_of(&r), IdentityStatus::Missing);
}

#[test]
fn untrusted_platform_hint_gets_no_identity() {
    let dir = tempfile::tempdir().unwrap();
    let zip = write_zip(dir.path(), "u.zip", &[("a.sfc", &snes_rom(1))]);
    let r = inspect_game_identity(&zip, Some("SNES"));
    assert!(refused(&r));
    assert_eq!(status_of(&r), IdentityStatus::Ambiguous);
}

#[test]
fn traversal_absolute_and_duplicate_names_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    for (i, name) in ["../evil.sfc", "dir/../../evil.sfc", "dir\\evil.sfc"]
        .into_iter()
        .enumerate()
    {
        let zip = write_zip(dir.path(), &format!("t{i}.zip"), &[(name, &snes_rom(1))]);
        let r = inspect(&zip, "SNES");
        assert!(refused(&r), "{name}");
        assert_eq!(status_of(&r), IdentityStatus::Invalid, "{name}");
    }
    let zip = write_zip(
        dir.path(),
        "dup.zip",
        &[("a.sfc", &snes_rom(1)), ("A.SFC", &snes_rom(2))],
    );
    let r = inspect(&zip, "SNES");
    assert!(refused(&r));
    assert_eq!(status_of(&r), IdentityStatus::Ambiguous);
}

#[test]
fn encrypted_member_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("enc.zip");
    let mut writer = ZipWriter::new(fs::File::create(&path).unwrap());
    writer
        .start_file(
            "a.sfc",
            opts(CompressionMethod::Stored)
                .with_deprecated_encryption(b"pw")
                .unwrap(),
        )
        .unwrap();
    writer.write_all(&snes_rom(1)).unwrap();
    writer.finish().unwrap();
    let r = inspect(&path, "SNES");
    assert!(refused(&r));
    assert_eq!(status_of(&r), IdentityStatus::Unsupported);
}

#[test]
fn oversized_member_is_refused_before_inflation() {
    let dir = tempfile::tempdir().unwrap();
    let big = vec![0xAA_u8; (MAX_LOOSE_ROM_BYTES + 1) as usize];
    let path = dir.path().join("big.zip");
    let mut writer = ZipWriter::new(fs::File::create(&path).unwrap());
    writer
        .start_file("big.sfc", opts(CompressionMethod::Stored))
        .unwrap();
    writer.write_all(&big).unwrap();
    writer.finish().unwrap();
    let r = inspect(&path, "SNES");
    assert!(refused(&r));
    assert_eq!(status_of(&r), IdentityStatus::ResourceLimitReached);
    assert_eq!(r.bytes_read, 0);
}

#[test]
fn truncated_and_corrupt_zips_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let rom = snes_rom(3);
    let zip = write_zip(dir.path(), "ok.zip", &[("a.sfc", &rom)]);
    let bytes = fs::read(&zip).unwrap();
    let cut = dir.path().join("cut.zip");
    fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    let r = inspect(&cut, "SNES");
    assert!(refused(&r));
    assert_eq!(status_of(&r), IdentityStatus::Invalid);

    // Flip a payload byte: CRC mismatch must not yield an identity.
    let mut corrupt = bytes.clone();
    corrupt[40] ^= 0xFF;
    let bad = dir.path().join("bad.zip");
    fs::write(&bad, &corrupt).unwrap();
    let r = inspect(&bad, "SNES");
    assert!(refused(&r));

    let junk = dir.path().join("junk.zip");
    fs::write(&junk, b"not a zip at all").unwrap();
    assert!(refused(&inspect(&junk, "SNES")));
}

#[test]
fn replaced_zip_yields_a_different_identity() {
    let dir = tempfile::tempdir().unwrap();
    let first = write_zip(dir.path(), "g.zip", &[("g.sfc", &snes_rom(1))]);
    let a = inspect(&first, "SNES");
    let second = write_zip(dir.path(), "g.zip", &[("g.sfc", &snes_rom(2))]);
    assert_eq!(first, second);
    let b = inspect(&second, "SNES");
    assert_ne!(
        a.verified_value(IdentityKind::LooseRomSha256),
        b.verified_value(IdentityKind::LooseRomSha256)
    );
    assert_eq!(
        b.verified_value(IdentityKind::LooseRomSha256),
        Some(sha(&snes_rom(2)).as_str())
    );
}

#[test]
fn loose_and_disc_zip_behaviour_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let rom = snes_rom(4);
    let loose = dir.path().join("l.sfc");
    fs::write(&loose, &rom).unwrap();
    let r = inspect(&loose, "SNES");
    assert_eq!(r.format, IdentityImageFormat::LooseCartridgeRom);
    assert!(r.is_verified_loose_rom());
    // A non-cartridge platform's ZIP still takes the disc route.
    let zip = write_zip(dir.path(), "d.zip", &[("disc.iso", b"nope")]);
    let r = inspect(&zip, "PlayStation 2");
    assert_eq!(r.format, IdentityImageFormat::ZipContainingIso);
}

/// Runs `check` on a worker and fails (instead of hanging the suite) if it
/// has not returned within a bounded time.
#[cfg(unix)]
fn within_deadline(check: impl FnOnce() -> bool + Send + 'static) -> bool {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(check());
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the ZIP freshness recheck blocked on a special file")
}

/// Creates a FIFO at `path`; `false` where the filesystem cannot host one.
#[cfg(unix)]
fn make_fifo(path: &Path) -> bool {
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `name` is a valid NUL-terminated path inside a fresh temporary
    // directory; `mkfifo` only creates the FIFO this test then refuses.
    unsafe { libc::mkfifo(name.as_ptr(), 0o600) == 0 }
}

/// Codex review on #333: the post-read freshness check reopened the ZIP with
/// a plain `File::open`, which blocks forever if the path was swapped for a
/// FIFO. Identity inspection and launch staging now share `still_same_file`,
/// which only reopens through the non-blocking safe-read policy.
#[cfg(unix)]
#[test]
fn freshness_recheck_refuses_a_zip_swapped_for_a_fifo_or_special_file_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let zip = write_zip(dir.path(), "game.zip", &[("Game.sfc", &snes_rom(1))]);
    let before = StableFileMetadata::from_file(&fs::File::open(&zip).unwrap()).unwrap();
    let trusted = TrustedRoots::from_paths([dir.path()]);
    assert!(still_same_file(&zip, &TrustedRoots::none(), &before));
    // An external or multi-drive library reached through a trusted symlink
    // still passes when the target is the same file.
    let link = dir.path().join("linked.zip");
    std::os::unix::fs::symlink(&zip, &link).unwrap();
    assert!(still_same_file(&link, &trusted, &before));

    fs::rename(&zip, dir.path().join("moved.zip")).unwrap();
    if !make_fifo(&zip) {
        return;
    }
    let (path, roots) = (zip.clone(), TrustedRoots::none());
    let b = before.clone();
    assert!(!within_deadline(move || still_same_file(&path, &roots, &b)));
    // A trusted symlink now pointing at the FIFO is refused too.
    let (path, roots, b) = (link.clone(), trusted.clone(), before.clone());
    assert!(!within_deadline(move || still_same_file(&path, &roots, &b)));

    // A directory or device in its place is refused as well.
    fs::remove_file(&zip).unwrap();
    fs::create_dir(&zip).unwrap();
    assert!(!still_same_file(&zip, &TrustedRoots::none(), &before));
    let b = before.clone();
    assert!(!within_deadline(move || {
        still_same_file(Path::new("/dev/zero"), &TrustedRoots::none(), &b)
    }));
}
