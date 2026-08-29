use super::container::{ContainerKind, FolderRole};
use super::content_registry::ContentKind;
use super::content_registry::recognized_extensions;
use super::discovery::{SkipReason, ValidationState, discover_source};
use std::path::Path;
use tempfile::tempdir;

fn source_dir(name: &str) -> tempfile::TempDir {
    tempdir().unwrap_or_else(|error| panic!("failed to create temp source dir {name}: {error}"))
}

/// The standard z64 (big-endian) N64 dump signature.
const N64_Z64_MAGIC: [u8; 4] = [0x80, 0x37, 0x12, 0x40];

/// Minimal valid Game Boy header bytes are not required for detection -
/// `.gb` is filename-strong evidence on its own - so loose ROM fixtures
/// below use arbitrary content unless a format needs a real signature to
/// be read at all (Amiga HDF, WHDLoad slave).
fn put16(v: &mut [u8], at: usize, n: u16) {
    v[at..at + 2].copy_from_slice(&n.to_be_bytes());
}
fn put32(v: &mut [u8], at: usize, n: u32) {
    v[at..at + 4].copy_from_slice(&n.to_be_bytes());
}

/// A minimal, valid WHDLoad `.slave` HUNK binary - mirrors the fixture in
/// `identity_source::whdload::tests`.
fn minimal_whdload_slave() -> Vec<u8> {
    let size: usize = 30;
    let mut code = vec![0; (size + 64).next_multiple_of(4)];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    put16(&mut code, 12, 1); // runtime_version
    put16(&mut code, 14, 3);
    put32(&mut code, 16, 524288);
    put32(&mut code, 20, 1);
    put32(&mut code, 24, 2);
    put16(&mut code, 28, 0);
    code[size..size + 5].copy_from_slice(b"Game\0");
    code[size + 8..size + 13].copy_from_slice(b"Copy\0");
    code[size + 16..size + 21].copy_from_slice(b"Info\0");
    code[size + 24..size + 29].copy_from_slice(b"Kick\0");
    code[size + 32..size + 39].copy_from_slice(b"Config\0");
    let mut out = Vec::new();
    for n in [
        0x3f3_u32,
        0,
        1,
        0,
        0,
        (code.len() / 4) as u32,
        0x3e9,
        (code.len() / 4) as u32,
    ] {
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend_from_slice(&code);
    out.extend_from_slice(&0x3f2_u32.to_be_bytes());
    out
}

/// A minimal valid RDB/HDF image with no partitions - just enough for
/// `amiga_disk::inspect_hdf` to succeed, mirroring `amiga_disk::tests`.
fn minimal_amiga_hdf() -> Vec<u8> {
    let mut b = vec![0u8; 512 * 4];
    b[..4].copy_from_slice(b"RDSK");
    put32(&mut b, 16, 512); // block size
    put32(&mut b, 28, 0xffff_ffff); // partition head: NONE
    put32(&mut b, 64, 20);
    put32(&mut b, 68, 10);
    put32(&mut b, 72, 2);
    b
}

fn write_zip_containing(dir: &Path, zip_name: &str, member_name: &str, member_bytes: &[u8]) {
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(dir.join(zip_name)).unwrap();
    let mut writer = ZipWriter::new(file);
    writer
        .start_file(member_name, SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut writer, member_bytes).unwrap();
    writer.finish().unwrap();
}

#[test]
fn loose_n64_rom_is_discovered_as_a_rom_cartridge_candidate() {
    let dir = source_dir("n64");
    std::fs::write(dir.path().join("Mario Kart 64.z64"), N64_Z64_MAGIC).unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1, "{:?}", report.items);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::RomCartridge));
    assert_eq!(item.container, ContainerKind::DirectFile);
    assert!(!item.explanation.is_empty());
    assert_eq!(report.stats.loose_roms, 1);
}

#[test]
fn loose_game_boy_rom_is_discovered_as_a_rom_cartridge_candidate() {
    let dir = source_dir("gb");
    std::fs::write(dir.path().join("Pokemon.gb"), b"not a real rom, just bytes").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].content, Some(ContentKind::RomCartridge));
    assert_eq!(report.stats.loose_roms, 1);
}

#[test]
fn zip_containing_a_game_boy_rom_is_discovered_with_the_same_content_kind_as_loose() {
    let dir = source_dir("zip-gb");
    write_zip_containing(dir.path(), "Pokemon.zip", "Pokemon.gb", b"gb rom bytes");

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert!(matches!(item.container, ContainerKind::Archive(_)));
    assert_eq!(item.content, Some(ContentKind::RomCartridge));
    assert_eq!(report.stats.archives, 1);
}

#[test]
fn cue_bin_pair_becomes_one_disc_image_candidate() {
    let dir = source_dir("cue-bin");
    std::fs::write(
        dir.path().join("Final Fantasy VII Disc1.bin"),
        b"disc bytes",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Final Fantasy VII.cue"),
        "FILE \"Final Fantasy VII Disc1.bin\" BINARY\n",
    )
    .unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(
        report.items.len(),
        1,
        "the .bin must be consumed by the .cue, not listed separately: {:?}",
        report.items
    );
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::DiscImage));
    assert_eq!(item.path.extension().unwrap(), "cue");
    assert_eq!(report.stats.disc_images, 1);
}

#[test]
fn a_lone_bin_without_a_cue_is_flagged_missing_paired_file_not_silently_dropped() {
    let dir = source_dir("lone-bin");
    std::fs::write(dir.path().join("orphan.bin"), b"disc bytes").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    assert_eq!(
        report.items[0].skip_reason,
        Some(SkipReason::MissingPairedFile)
    );
    assert!(!report.items[0].explanation.is_empty());
}

#[test]
fn chd_is_discovered_as_a_disc_image_candidate() {
    let dir = source_dir("chd");
    std::fs::write(dir.path().join("Arcade Game.chd"), b"not a real chd").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::DiscImage));
    // CHD is deliberately never strong platform evidence on its own -
    // it is fine for identity to be unresolved, but it must still be a
    // visible, explained candidate, never a silent drop.
    if let Some(reason) = &item.skip_reason {
        assert_eq!(reason, &SkipReason::RecognizedContentNoIdentityMatch);
    }
    assert_eq!(report.stats.disc_images, 1);
}

#[test]
fn dsk_and_cdt_are_recognised_media_without_platform_guessing() {
    for (name, expected) in [
        ("unknown-platform.dsk", ContentKind::ComputerDisk),
        ("unknown-platform.cdt", ContentKind::TapeImage),
    ] {
        let dir = source_dir(name);
        std::fs::write(dir.path().join(name), b"fixture bytes").unwrap();
        let report = discover_source(dir.path()).unwrap();
        assert_eq!(report.items.len(), 1);
        let item = &report.items[0];
        assert_eq!(item.content, Some(expected));
        assert!(
            item.platform_hint.is_none(),
            "{name} must not infer a platform"
        );
        assert_eq!(
            item.skip_reason,
            Some(SkipReason::RecognizedContentNoIdentityMatch)
        );
    }
}

#[test]
fn amiga_hdf_is_discovered_and_validated() {
    let dir = source_dir("hdf");
    std::fs::write(dir.path().join("Workbench.hdf"), minimal_amiga_hdf()).unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::AmigaImage));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert!(item.explanation.contains("partition"));
    assert_eq!(report.stats.amiga_images, 1);
}

/// A real-world WHDLoad CD32 pack shape: the whole file is one AmigaDOS
/// filesystem starting at byte 0, no RDB wrapper - mirrors
/// `amiga_disk::tests::flat_amigados_image_is_recognised_as_one_whole_image_partition`.
fn minimal_flat_amiga_hdf() -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    b[..4].copy_from_slice(&0x444f_5301u32.to_be_bytes());
    b
}

#[test]
fn a_real_world_flat_cd32_hdf_is_discovered_as_amiga_not_just_a_valid_rdb_one() {
    let dir = source_dir("cd32-flat-hdf");
    std::fs::write(
        dir.path().join("JungleStrike_v1.2_CD32.hdf"),
        minimal_flat_amiga_hdf(),
    )
    .unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::AmigaImage));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(report.stats.amiga_images, 1);
}

/// Real X68000 collections ship hard-disk images as `.hdf` too - a genuine
/// extension collision discovered against a real 3,376-file X68000
/// collection during validation, where these were previously mislabelled
/// `AmigaImage`. A `.hdf` that is not a readable Amiga image, but whose
/// platform is otherwise known (here via folder alias), must never be
/// reported as Amiga content.
#[test]
fn a_non_amiga_hdf_with_known_platform_is_not_mislabelled_amiga() {
    let dir = source_dir("x68000-hdf");
    let platform_dir = dir.path().join("x68000");
    std::fs::create_dir_all(&platform_dir).unwrap();
    std::fs::write(platform_dir.join("Daimakaimura.hdf"), b"not an amiga image").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_ne!(item.content, Some(ContentKind::AmigaImage));
    assert_eq!(item.content, Some(ContentKind::ComputerDisk));
    assert_eq!(item.platform_hint.as_deref(), Some("Sharp X68000"));
    assert_eq!(item.validation_state, ValidationState::Accepted);
}

#[test]
fn a_non_amiga_hdf_with_no_platform_evidence_is_flagged_ambiguous_not_amiga() {
    let dir = source_dir("unknown-hdf");
    std::fs::write(dir.path().join("mystery.hdf"), b"not an amiga image").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_ne!(item.content, Some(ContentKind::AmigaImage));
    assert_eq!(item.skip_reason, Some(SkipReason::AmbiguousPlatform));
}

#[test]
fn whdload_folder_is_discovered_as_one_install_candidate() {
    let dir = source_dir("whdload");
    let install_dir = dir.path().join("Turrican II");
    std::fs::create_dir_all(&install_dir).unwrap();
    std::fs::write(install_dir.join("Turrican2.slave"), minimal_whdload_slave()).unwrap();
    std::fs::write(install_dir.join("Turrican2"), b"the game binary").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(
        report.items.len(),
        1,
        "the folder is one item, not recursed into: {:?}",
        report.items
    );
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::WhdloadInstall));
    assert_eq!(
        item.container,
        ContainerKind::Folder(FolderRole::WhdloadInstall)
    );
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(report.stats.game_folders, 1);
}

#[test]
fn unsupported_file_is_visible_with_an_explained_skip_reason() {
    let dir = source_dir("unknown");
    std::fs::write(dir.path().join("Notes.xyz"), b"not a game").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, None);
    assert_eq!(item.skip_reason, Some(SkipReason::UnsupportedExtension));
    assert!(!item.explanation.is_empty());
    assert!(
        !item
            .skip_reason
            .as_ref()
            .unwrap()
            .suggested_action()
            .is_empty()
    );
    assert_eq!(report.stats.unknown, 1);
}

/// Regression test for a live-QA bug: a real collection's incidental,
/// never-game-content sidecar files (box art, readmes, saves, frontend
/// metadata) were each counted as an `UnsupportedExtension` "needs
/// attention" item, inflating a real scan's "needs attention" total into
/// the hundreds of thousands and swamping any real signal in it.
#[test]
fn known_non_game_sidecar_files_are_not_reported_as_needing_attention() {
    let dir = source_dir("sidecars");
    let root = dir.path();
    std::fs::write(root.join("Mario Kart 64.z64"), N64_Z64_MAGIC).unwrap();
    for name in [
        "Mario Kart 64.jpg",
        "Mario Kart 64.png",
        "Mario Kart 64.nfo",
        "Mario Kart 64.txt",
        "gamelist.xml",
        "Thumbs.db",
        "trailer.mp4",
    ] {
        std::fs::write(root.join(name), b"not game content").unwrap();
    }

    let report = discover_source(root).unwrap();
    assert_eq!(
        report.items.len(),
        1,
        "only the ROM should be a discovery item at all, sidecars must not \
         appear as items: {:?}",
        report.items
    );
    assert_eq!(report.stats.unknown, 0);
    assert_eq!(report.skip_reasons.total(), 0);
}

#[test]
fn a_mixed_folder_produces_a_candidate_for_every_kind_with_no_unexplained_disappearance() {
    let dir = source_dir("mixed");
    let root = dir.path();

    std::fs::write(root.join("Mario Kart 64.z64"), N64_Z64_MAGIC).unwrap();
    std::fs::write(root.join("Pokemon Red.gb"), b"gb bytes").unwrap();
    write_zip_containing(root, "Game.zip", "Game.gba", b"gba bytes");
    std::fs::write(root.join("Final Fantasy VII Disc1.bin"), b"disc bytes").unwrap();
    std::fs::write(
        root.join("Final Fantasy VII.cue"),
        "FILE \"Final Fantasy VII Disc1.bin\" BINARY\n",
    )
    .unwrap();
    std::fs::write(root.join("Arcade.chd"), b"chd bytes").unwrap();
    std::fs::write(root.join("Workbench.hdf"), minimal_amiga_hdf()).unwrap();
    let whdload_dir = root.join("WHDLoad Game");
    std::fs::create_dir_all(&whdload_dir).unwrap();
    std::fs::write(whdload_dir.join("Game.slave"), minimal_whdload_slave()).unwrap();
    std::fs::write(root.join("Unknown.xyz"), b"mystery").unwrap();

    let report = discover_source(root).unwrap();

    // Every item must have a non-empty explanation, whether accepted or
    // skipped - the "no valid content disappears silently" guarantee.
    for item in &report.items {
        assert!(
            !item.explanation.is_empty(),
            "item without an explanation: {item:?}"
        );
        if item.validation_state == ValidationState::Skipped {
            assert!(item.skip_reason.is_some());
        }
    }

    assert_eq!(report.stats.loose_roms, 2, "{:?}", report.stats); // z64 + gb
    assert_eq!(report.stats.archives, 1); // Game.zip
    assert_eq!(report.stats.disc_images, 2); // cue+bin pair + chd
    assert_eq!(report.stats.amiga_images, 1); // hdf
    assert_eq!(report.stats.game_folders, 1); // WHDLoad folder
    assert_eq!(report.stats.unknown, 1); // Unknown.xyz

    // The paired .bin must never appear as its own item.
    assert!(
        !report
            .items
            .iter()
            .any(|item| item.path.extension().and_then(|e| e.to_str()) == Some("bin")),
        "{:?}",
        report.items
    );
}

/// Every extension the content registry recognises must actually be
/// picked up end-to-end by discovery when placed as a loose file - keeps
/// the registry table and the discovery wiring from silently drifting
/// apart, mirroring `crate::media_registry`'s own scan/watch parity test.
#[test]
fn every_registered_content_extension_is_discovered_end_to_end() {
    for (index, extension) in recognized_extensions().enumerate() {
        // hdf/hdfx/rdb require a real RDB image to be read at all -
        // covered individually by `amiga_hdf_is_discovered_and_validated`
        // and by the Amiga integration; a garbage file for those would
        // correctly produce an `InvalidContent` skip, which is exactly
        // right, not a registry drift bug - so skip them here.
        if matches!(extension, "hdf" | "hdfx" | "rdb") {
            continue;
        }
        let dir = source_dir(&format!("parity-{index}"));
        let file_name = format!("fixture.{extension}");
        std::fs::write(dir.path().join(&file_name), b"fixture bytes").unwrap();

        let report = discover_source(dir.path()).unwrap();
        assert_eq!(
            report.items.len(),
            1,
            "extension {extension} did not produce exactly one discovery item"
        );
        assert!(
            report.items[0].content.is_some(),
            "extension {extension} was registered but discovery reported no content kind"
        );
    }
}
