//! Synthetic ZIPs only; no real ROM or library is read.
use super::*;
use std::io::Write;

const ROM: &[u8] = b"synthetic cartridge rom bytes";

enum Entry<'a> {
    File(&'a str, &'a [u8]),
    Dir(&'a str),
    Link(&'a str, &'a str),
}

fn zip_bytes(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for entry in entries {
        match entry {
            Entry::File(name, bytes) => {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
            Entry::Dir(name) => writer.add_directory(*name, options).unwrap(),
            Entry::Link(name, target) => writer.add_symlink(*name, *target, options).unwrap(),
        }
    }
    writer.finish().unwrap().into_inner()
}

/// Applies `edit(header, flags offset, uncompressed-size offset)` to every
/// local and central header.
fn patch(bytes: &mut [u8], edit: impl Fn(&mut [u8], usize, usize)) {
    for at in 0..bytes.len().saturating_sub(4) {
        match &bytes[at..at + 4] {
            [0x50, 0x4b, 0x03, 0x04] => edit(&mut bytes[at..], 6, 22),
            [0x50, 0x4b, 0x01, 0x02] => edit(&mut bytes[at..], 8, 24),
            _ => {}
        }
    }
}

fn select(bytes: &[u8], platform: IdentityPlatform) -> Result<ZipCartridgeMember, IdentityStatus> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.zip");
    std::fs::write(&path, bytes).unwrap();
    let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
    let mut seen = 0;
    select_zip_cartridge_member(&mut archive, platform, &mut seen).map_err(|(status, _)| status)
}

#[test]
fn the_one_supported_rom_is_selected_beside_other_files_and_folders() {
    let bytes = zip_bytes(&[
        Entry::Dir("docs/"),
        Entry::File("docs/readme.txt", b"notes"),
        Entry::File("Game (Europe).gb", ROM),
    ]);
    let member = select(&bytes, IdentityPlatform::GameBoy).unwrap();
    assert_eq!(member.name, b"Game (Europe).gb");
    assert_eq!(member.index, 2);
    assert_eq!(member.declared, ROM.len() as u64);
    assert_eq!(member.format, "gb");
}

#[test]
fn ambiguity_and_absence_are_refused_never_picked() {
    let two = zip_bytes(&[Entry::File("A.gb", ROM), Entry::File("B.gb", ROM)]);
    assert_eq!(
        select(&two, IdentityPlatform::GameBoy),
        Err(IdentityStatus::Ambiguous)
    );
    let duplicate = zip_bytes(&[Entry::File("Game.gb", ROM), Entry::File("GAME.GB", ROM)]);
    assert_eq!(
        select(&duplicate, IdentityPlatform::GameBoy),
        Err(IdentityStatus::Ambiguous)
    );
    let none = zip_bytes(&[Entry::File("readme.txt", b"notes")]);
    assert_eq!(
        select(&none, IdentityPlatform::GameBoy),
        Err(IdentityStatus::Missing)
    );
    // A ROM for another system is not this system's ROM.
    let other = zip_bytes(&[Entry::File("Game.sfc", ROM)]);
    assert_eq!(
        select(&other, IdentityPlatform::GameBoy),
        Err(IdentityStatus::Missing)
    );
}

#[test]
fn unsafe_names_links_and_encryption_refuse_the_whole_archive() {
    for name in ["../Game.gb", "/abs/Game.gb", "dir\\Game.gb"] {
        let bytes = zip_bytes(&[Entry::File(name, ROM)]);
        assert_eq!(
            select(&bytes, IdentityPlatform::GameBoy),
            Err(IdentityStatus::Invalid),
            "{name}"
        );
    }
    let linked = zip_bytes(&[
        Entry::File("Game.gb", ROM),
        Entry::Link("Other.gb", "Game.gb"),
    ]);
    assert_eq!(
        select(&linked, IdentityPlatform::GameBoy),
        Err(IdentityStatus::Unsupported)
    );
    let mut locked = zip_bytes(&[Entry::File("Game.gb", ROM)]);
    patch(&mut locked, |header, flags, _| header[flags] |= 1);
    assert_eq!(
        select(&locked, IdentityPlatform::GameBoy),
        Err(IdentityStatus::Unsupported)
    );
}

#[test]
fn declared_size_and_ratio_bounds_are_enforced_before_decompression() {
    let mut huge = zip_bytes(&[Entry::File("Game.gb", ROM)]);
    let over = (MAX_LOOSE_ROM_BYTES + 1) as u32;
    patch(&mut huge, |header, _, size| {
        header[size..size + 4].copy_from_slice(&over.to_le_bytes())
    });
    assert_eq!(
        select(&huge, IdentityPlatform::GameBoy),
        Err(IdentityStatus::ResourceLimitReached)
    );
    // Declares far more than its stored bytes could expand to.
    let mut bomb = zip_bytes(&[Entry::File("Game.gb", ROM)]);
    let claimed = (ROM.len() as u64 * MAX_ZIP_CARTRIDGE_RATIO + 4096 + 1) as u32;
    patch(&mut bomb, |header, _, size| {
        header[size..size + 4].copy_from_slice(&claimed.to_le_bytes())
    });
    assert_eq!(
        select(&bomb, IdentityPlatform::GameBoy),
        Err(IdentityStatus::ResourceLimitReached)
    );
}

#[test]
fn only_cartridge_systems_take_the_zipped_route() {
    assert!(is_loose_cartridge_platform(IdentityPlatform::GameBoy));
    assert!(is_loose_cartridge_platform(IdentityPlatform::Nes));
    assert!(!is_loose_cartridge_platform(IdentityPlatform::PlayStation));
    assert!(!is_loose_cartridge_platform(IdentityPlatform::Amiga));
}

/// Two entries stored under the exact same name (a writer refuses to make
/// one, so the second name is patched in afterwards).
fn exact_duplicate() -> Vec<u8> {
    let mut bytes = zip_bytes(&[
        Entry::File("Game.gb", ROM),
        Entry::File("Gxme.gb", b"other"),
    ]);
    for at in 0..bytes.len() - 7 {
        if &bytes[at..at + 7] == b"Gxme.gb" {
            bytes[at..at + 7].copy_from_slice(b"Game.gb");
        }
    }
    bytes
}

#[test]
fn exact_duplicate_names_are_refused_not_collapsed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.zip");
    std::fs::write(&path, exact_duplicate()).unwrap();
    let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
    let (status, message) =
        select_zip_cartridge_member(&mut archive, IdentityPlatform::GameBoy, &mut 0).unwrap_err();
    assert_eq!(status, IdentityStatus::Invalid);
    // Refused for the duplicate itself, not some incidental inconsistency.
    assert!(message.contains("duplicate file names"), "{message}");
}

#[test]
fn drive_prefixed_and_nul_names_are_refused() {
    for name in ["C:/Game.gb", "C:Game.gb", "Ga\0me.gb"] {
        let bytes = zip_bytes(&[Entry::File(name, ROM)]);
        assert_eq!(
            select(&bytes, IdentityPlatform::GameBoy),
            Err(IdentityStatus::Invalid),
            "{name:?}"
        );
    }
}
