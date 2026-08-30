use super::container::{ContainerKind, FolderRole};
use super::content_registry::ContentKind;
use super::content_registry::recognized_extensions;
use super::discovery::{SkipReason, ValidationState, discover_source};
use std::path::Path;
use tempfile::tempdir;

fn source_dir(name: &str) -> tempfile::TempDir {
    tempdir().unwrap_or_else(|error| panic!("failed to create temp source dir {name}: {error}"))
}

fn minimal_hdi() -> Vec<u8> {
    let mut image = vec![0; 32 + 512];
    image[8..12].copy_from_slice(&32_u32.to_le_bytes());
    image[12..16].copy_from_slice(&512_u32.to_le_bytes());
    image[16..20].copy_from_slice(&512_u32.to_le_bytes());
    image[20..24].copy_from_slice(&1_u32.to_le_bytes());
    image[24..28].copy_from_slice(&1_u32.to_le_bytes());
    image[28..32].copy_from_slice(&1_u32.to_le_bytes());
    image
}

fn minimal_nhd() -> Vec<u8> {
    let mut image = vec![0; 0x200 + 512];
    image[..15].copy_from_slice(b"T98HDDIMAGE.R0\0");
    image[0x110..0x114].copy_from_slice(&0x200_u32.to_le_bytes());
    image[0x114..0x118].copy_from_slice(&1_u32.to_le_bytes());
    image[0x118..0x11a].copy_from_slice(&1_u16.to_le_bytes());
    image[0x11a..0x11c].copy_from_slice(&1_u16.to_le_bytes());
    image[0x11c..0x11e].copy_from_slice(&512_u16.to_le_bytes());
    image
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

fn minimal_valid_dfs_image() -> Vec<u8> {
    let mut image = vec![0_u8; 400 * 256];
    image[0..8].copy_from_slice(b"TEST DFS");
    image[256..260].copy_from_slice(b"    ");
    image[260] = 0x12;
    image[261] = 8;
    image[262] = 1;
    image[263] = 0x90;
    image[8..15].copy_from_slice(b"!BOOT  ");
    image[15] = b'$';
    image[264..266].copy_from_slice(&0x1900_u16.to_le_bytes());
    image[266..268].copy_from_slice(&0x1900_u16.to_le_bytes());
    image[268..270].copy_from_slice(&3_u16.to_le_bytes());
    image[271] = 2;
    image
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
fn loose_wonderswan_rom_is_discovered_with_its_existing_platform_context() {
    let parent = source_dir("wonderswan");
    let platform_dir = parent.path().join("WonderSwan");
    std::fs::create_dir(&platform_dir).unwrap();
    std::fs::write(platform_dir.join("Game.ws"), b"wonderswan rom bytes").unwrap();

    let report = discover_source(&platform_dir).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::RomCartridge));
    assert_eq!(item.platform_hint.as_deref(), Some("WonderSwan"));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(report.stats.loose_roms, 1);
}

#[test]
fn loose_wonderswan_color_rom_is_discovered_with_its_existing_platform_context() {
    let parent = source_dir("wonderswan-color");
    let platform_dir = parent.path().join("WonderSwan Color");
    std::fs::create_dir(&platform_dir).unwrap();
    std::fs::write(platform_dir.join("Game.wsc"), b"wonderswan color rom bytes").unwrap();

    let report = discover_source(&platform_dir).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::RomCartridge));
    assert_eq!(item.platform_hint.as_deref(), Some("WonderSwan Color"));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(report.stats.loose_roms, 1);
}

#[test]
fn loose_msx_cartridges_are_discovered_and_resolved_by_generation() {
    let dir = source_dir("msx-cartridges");
    std::fs::write(dir.path().join("cart.mx1"), b"MSX1 cartridge bytes").unwrap();
    std::fs::write(dir.path().join("cart.mx2"), b"MSX2 cartridge bytes").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 2, "{:?}", report.items);
    for item in &report.items {
        assert_eq!(item.container, ContainerKind::DirectFile);
        assert_eq!(item.content, Some(ContentKind::RomCartridge));
        assert_eq!(item.validation_state, ValidationState::Accepted);
    }
    assert_eq!(
        report
            .items
            .iter()
            .find(|item| item.path.extension().and_then(|ext| ext.to_str()) == Some("mx1"))
            .and_then(|item| item.platform_hint.as_deref()),
        Some("MSX")
    );
    assert_eq!(
        report
            .items
            .iter()
            .find(|item| item.path.extension().and_then(|ext| ext.to_str()) == Some("mx2"))
            .and_then(|item| item.platform_hint.as_deref()),
        Some("MSX2")
    );
    assert_eq!(report.stats.loose_roms, 2);
}

#[test]
fn pc98_hdi_and_nhd_are_discovered_only_with_folder_identity() {
    let parent = source_dir("pc98-hard-disks");
    let pc98 = parent.path().join("pc98");
    std::fs::create_dir(&pc98).unwrap();
    std::fs::write(pc98.join("disk.hdi"), minimal_hdi()).unwrap();
    std::fs::write(pc98.join("disk.nhd"), minimal_nhd()).unwrap();
    let report = discover_source(&pc98).unwrap();
    assert_eq!(report.items.len(), 2, "{:?}", report.items);
    for item in &report.items {
        assert_eq!(item.content, Some(ContentKind::ComputerDisk));
        assert_eq!(item.platform_hint.as_deref(), Some("PC-98"));
        assert_eq!(item.validation_state, ValidationState::Accepted);
    }

    let bare = source_dir("bare-pc98-hard-disks");
    std::fs::write(bare.path().join("disk.hdi"), minimal_hdi()).unwrap();
    std::fs::write(bare.path().join("disk.nhd"), minimal_nhd()).unwrap();
    let report = discover_source(bare.path()).unwrap();
    assert_eq!(report.items.len(), 2);
    assert!(report.items.iter().all(|item| {
        item.platform_hint.is_none()
            && item.validation_state == ValidationState::Skipped
            && item.skip_reason == Some(SkipReason::RecognizedContentNoIdentityMatch)
    }));
}

#[test]
fn ambiguous_msx_rom_and_bin_files_do_not_self_resolve() {
    let dir = source_dir("msx-ambiguous");
    std::fs::write(dir.path().join("cart.rom"), b"shared ROM bytes").unwrap();
    std::fs::write(dir.path().join("cart.bin"), b"shared binary bytes").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 2, "{:?}", report.items);
    assert!(report.items.iter().all(|item| item.platform_hint.is_none()));
    assert!(report.items.iter().all(|item| item.content.is_none()));
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
fn archive_member_ssd_and_dsd_extensions_are_likely_computer_disks() {
    let dir = source_dir("zip-dfs");
    write_zip_containing(dir.path(), "Disks.zip", "Game.ssd", b"not inspected");
    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].content, Some(ContentKind::ComputerDisk));
    assert!(matches!(
        report.items[0].container,
        ContainerKind::Archive(_)
    ));
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
    // `.dsk` now goes through the shared structural disk layer: 13 garbage
    // bytes are not a CPCEMU container at all, so this is an `InvalidContent`
    // skip rather than "recognised, no identity". Both leave the platform
    // unset, which is the property this test is really guarding.
    for (name, expected, expected_reason) in [
        (
            "unknown-platform.dsk",
            ContentKind::ComputerDisk,
            SkipReason::InvalidContent(String::new()),
        ),
        (
            "unknown-platform.cdt",
            ContentKind::TapeImage,
            SkipReason::RecognizedContentNoIdentityMatch,
        ),
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
            std::mem::discriminant(item.skip_reason.as_ref().unwrap()),
            std::mem::discriminant(&expected_reason)
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

/// A minimal, structurally valid flat AmigaDOS floppy image (no RDB
/// wrapper) - the same shape `amiga_disk::tests::flat_adf` builds: a
/// `DOS\x0N` boot block, a root-block pointer, and a valid `ST_ROOT` root
/// block with a volume label. This is what `.adf` content inspection
/// validates through the existing bounded OFS/FFS reader.
fn minimal_flat_adf(dos: u8, volume: &[u8]) -> Vec<u8> {
    const SECTORS: usize = 128;
    const ROOT: usize = SECTORS / 2;
    let mut img = vec![0u8; SECTORS * 512];
    img[..3].copy_from_slice(b"DOS");
    img[3] = dos;
    put32(&mut img, 8, ROOT as u32);
    let mut root = [0u8; 512];
    put32(&mut root, 0, 2); // T_HEADER
    put32(&mut root, 12, 72); // hash-table size
    let name_len = volume.len().min(30);
    root[0x1B0] = name_len as u8;
    root[0x1B1..0x1B1 + name_len].copy_from_slice(&volume[..name_len]);
    put32(&mut root, 508, 1); // ST_ROOT
    let mut sum = 0u32;
    for offset in (0..512).step_by(4) {
        if offset != 20 {
            sum = sum.wrapping_add(u32::from_be_bytes(
                root[offset..offset + 4].try_into().unwrap(),
            ));
        }
    }
    put32(&mut root, 20, (sum as i32).wrapping_neg() as u32);
    img[ROOT * 512..(ROOT + 1) * 512].copy_from_slice(&root);
    img
}

#[test]
fn adf_ofs_dos0_is_discovered_as_amiga_from_its_contents() {
    let dir = source_dir("adf-ofs");
    std::fs::write(
        dir.path().join("Some Puzzle Game (1991).adf"),
        minimal_flat_adf(0, b"PuzzleDisk"),
    )
    .unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::AmigaImage));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(item.platform_hint.as_deref(), Some("Amiga"));
    assert_eq!(item.skip_reason, None);
    assert!(item.explanation.contains("OFS"));
    assert!(item.explanation.contains("DOS\\0"));
    assert_eq!(report.stats.amiga_images, 1);
}

#[test]
fn adf_ffs_dos1_is_discovered_as_amiga_with_the_ffs_family_named() {
    let dir = source_dir("adf-ffs");
    std::fs::write(
        dir.path().join("unrelated title.adf"),
        minimal_flat_adf(1, b"FastFileDisk"),
    )
    .unwrap();

    let report = discover_source(dir.path()).unwrap();
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::AmigaImage));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(item.platform_hint.as_deref(), Some("Amiga"));
    assert!(item.explanation.contains("FFS"));
}

/// Requirement 4: random/unrelated content named like an Amiga game must
/// not gain a strong Amiga structural identity from the `.adf` extension.
#[test]
fn random_content_named_like_an_amiga_game_gets_no_amiga_structural_evidence() {
    let dir = source_dir("adf-lie");
    std::fs::write(
        dir.path().join("Some Amiga Game.adf"),
        b"this is not an amiga disk image, just some bytes with a lying name",
    )
    .unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    // The item stays visible and explained, but is never accepted, and no
    // Amiga platform identity is attached from the extension alone.
    assert_eq!(item.validation_state, ValidationState::Skipped);
    assert_eq!(item.platform_hint, None);
    assert!(matches!(
        item.skip_reason,
        Some(SkipReason::InvalidContent(_))
    ));
    assert!(!item.explanation.contains("OFS"));
    assert!(!item.explanation.contains("FFS"));
    assert_eq!(report.stats.amiga_images, 1); // still categorised as an Amiga-image attempt
}

#[test]
fn zip_and_truncated_files_renamed_adf_fail_closed() {
    let dir = source_dir("adf-fakes");
    let mut zip = vec![0u8; 4096];
    zip[..4].copy_from_slice(b"PK\x03\x04");
    std::fs::write(dir.path().join("Game A.adf"), &zip).unwrap();
    std::fs::write(dir.path().join("Game B.adf"), b"DOS\x00 short").unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 2);
    for item in &report.items {
        assert_eq!(item.validation_state, ValidationState::Skipped, "{item:?}");
        assert_eq!(item.platform_hint, None);
        assert!(matches!(
            item.skip_reason,
            Some(SkipReason::InvalidContent(_))
        ));
    }
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

#[test]
fn fds_is_discovered_as_nes_computer_disk_media() {
    let root = source_dir("fds-discovery");
    std::fs::write(root.path().join("game.fds"), b"raw FDS fixture").unwrap();
    let report = discover_source(root.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::ComputerDisk));
    assert_eq!(item.platform_hint.as_deref(), Some("NES"));
    assert_eq!(item.validation_state, ValidationState::Accepted);
}

#[test]
fn apple_media_is_discovered_with_platform_and_sit_keeps_archive_content() {
    let root = source_dir("apple-media");
    for extension in ["do", "po", "woz", "2mg", "nib"] {
        std::fs::write(
            root.path().join(format!("game.{extension}")),
            b"fixture bytes",
        )
        .unwrap();
    }
    let mac = source_dir("macintosh-media");
    for extension in ["hfv", "dc42", "sit"] {
        std::fs::write(
            mac.path().join(format!("game.{extension}")),
            b"fixture bytes",
        )
        .unwrap();
    }

    let mut items = discover_source(root.path()).unwrap().items;
    items.extend(discover_source(mac.path()).unwrap().items);
    assert_eq!(items.len(), 8);
    for item in &items {
        assert_eq!(item.validation_state, ValidationState::Accepted, "{item:?}");
        if item.path.extension().and_then(|e| e.to_str()) == Some("sit") {
            assert_eq!(item.content, Some(ContentKind::Archive));
            assert_eq!(item.platform_hint.as_deref(), Some("Macintosh"));
        } else if ["hfv", "dc42"].contains(&item.path.extension().unwrap().to_str().unwrap()) {
            assert_eq!(item.content, Some(ContentKind::ComputerDisk));
            assert_eq!(item.platform_hint.as_deref(), Some("Macintosh"));
        } else {
            assert_eq!(item.content, Some(ContentKind::ComputerDisk));
            assert_eq!(item.platform_hint.as_deref(), Some("Apple II"));
        }
    }
}

#[test]
fn loose_commodore_disks_are_discovered_as_computer_disks() {
    let root = source_dir("commodore-disks");
    let dir = root.path().join("c128");
    std::fs::create_dir_all(&dir).unwrap();
    for extension in ["d64", "g64", "d71", "d81"] {
        std::fs::write(dir.join(format!("fixture.{extension}")), b"fixture bytes").unwrap();
    }

    let report = discover_source(&dir).unwrap();
    assert_eq!(
        report.items.len(),
        4,
        "all four Commodore disks must be visible"
    );
    assert!(report.items.iter().all(|item| {
        item.content == Some(ContentKind::ComputerDisk)
            && item.validation_state == ValidationState::Accepted
    }));
    assert_eq!(report.stats.computer_disks, 4);
}

#[test]
fn c128_folder_keeps_d64_and_g64_as_c128_ingestion_items() {
    let root = source_dir("c128-folder");
    let c128 = root.path().join("c128");
    std::fs::create_dir_all(&c128).unwrap();
    for extension in ["d64", "g64"] {
        std::fs::write(c128.join(format!("game.{extension}")), b"fixture bytes").unwrap();
    }

    let report = discover_source(&c128).unwrap();
    assert_eq!(report.items.len(), 2);
    assert!(report.items.iter().all(|item| {
        item.content == Some(ContentKind::ComputerDisk)
            && item.platform_hint.as_deref() == Some("Commodore 128")
            && item.validation_state == ValidationState::Accepted
    }));
}

#[test]
fn valid_dfs_disks_are_accepted_with_bbc_and_electron_folder_context() {
    for (folder, expected) in [
        ("bbcmicro", "BBC Micro"),
        ("bbcmaster", "BBC Micro"),
        ("electron", "Acorn Electron"),
    ] {
        let root = source_dir(&format!("dfs-{folder}"));
        let directory = root.path().join(folder);
        std::fs::create_dir_all(&directory).unwrap();
        for extension in ["ssd", "dsd"] {
            let image = if extension == "ssd" {
                minimal_valid_dfs_image()
            } else {
                let side = minimal_valid_dfs_image();
                let mut dsd = vec![0_u8; 800 * 256];
                dsd[..512].copy_from_slice(&side[..512]);
                dsd[0x0a00..0x0a00 + 512].copy_from_slice(&side[..512]);
                dsd
            };
            std::fs::write(directory.join(format!("game.{extension}")), image).unwrap();
        }
        let report = discover_source(&directory).unwrap();
        assert_eq!(report.items.len(), 2);
        assert!(
            report.items.iter().all(|item| {
                item.content == Some(ContentKind::ComputerDisk)
                    && item.platform_hint.as_deref() == Some(expected)
                    && item.validation_state == ValidationState::Accepted
            }),
            "{report:?}"
        );
    }
}
// --- ZX Spectrum snapshots + +3 disks ---------------------------------

fn z80_v1_uncompressed() -> Vec<u8> {
    let mut file = vec![0u8; 30 + 49152];
    file[7] = 0x80; // PC = 0x8000 (non-zero -> v1)
    file[9] = 0xC0; // SP = 0xC000
    file[27] = 1; // IFF1
    file[28] = 1; // IFF2
    file[29] = 1; // interrupt mode 1
    file
}

fn sna_128k() -> Vec<u8> {
    let mut file = vec![0u8; 27 + 49152 + 4 + (5 * 16384)];
    file[24] = 0x80; // SP = 0x8000
    file[25] = 1; // interrupt mode
    file[26] = 7; // border
    file
}

/// A minimal standard CPCEMU `.dsk`; when `plus3` is set, track 0 sector 1
/// carries a valid +3DOS disk specification.
fn dsk(tracks: u8, sides: u8, plus3: bool) -> Vec<u8> {
    let spt = 9usize;
    let track_size = 256 + spt * 512;
    let mut info = vec![0u8; 256];
    info[..0x22].copy_from_slice(b"MV - CPCEMU Disk-File\r\nDisk-Info\r\n");
    info[0x30] = tracks;
    info[0x31] = sides;
    info[0x32..0x34].copy_from_slice(&(track_size as u16).to_le_bytes());
    let mut image = info;
    for t in 0..(usize::from(tracks) * usize::from(sides)) {
        let mut h = vec![0u8; 256];
        h[..0x0C].copy_from_slice(b"Track-Info\r\n");
        h[0x10] = (t / usize::from(sides)) as u8;
        h[0x11] = (t % usize::from(sides)) as u8;
        h[0x14] = 2;
        h[0x15] = spt as u8;
        for s in 0..spt {
            let b = 0x18 + s * 8;
            h[b + 2] = 0xC1 + s as u8;
            h[b + 3] = 2;
        }
        image.extend_from_slice(&h);
        let mut data = vec![0xE5u8; spt * 512];
        if t == 0 && plus3 {
            data[0] = 0; // disk type: Spectrum +3 SS
            data[2] = tracks;
            data[3] = spt as u8;
            data[4] = 2;
            for byte in data.iter_mut().take(15).skip(10) {
                *byte = 0;
            }
        }
        image.extend_from_slice(&data);
    }
    image
}

fn d88(name: &str) -> Vec<u8> {
    let header_bytes = 0x2b0usize;
    let mut image = vec![0u8; header_bytes + 16 + 128];
    let name_bytes = name.as_bytes();
    image[..name_bytes.len().min(17)].copy_from_slice(&name_bytes[..name_bytes.len().min(17)]);
    image[0x1c..0x20].copy_from_slice(&(header_bytes as u32).to_le_bytes());
    let track = header_bytes;
    image[track + 2] = 1; // R
    image[track + 4..track + 6].copy_from_slice(&1u16.to_le_bytes());
    image[track + 14..track + 16].copy_from_slice(&128u16.to_le_bytes());
    image[track + 16..].fill(0xe5);
    image
}

#[test]
fn d88_discovery_requires_structure_and_preserves_folder_platform_evidence() {
    let root = source_dir("d88-discovery");
    let pc88 = root.path().join("pc88");
    let pc98 = root.path().join("pc98");
    std::fs::create_dir_all(&pc88).unwrap();
    std::fs::create_dir_all(&pc98).unwrap();
    std::fs::write(pc88.join("named.d88"), d88("PC88 DISK")).unwrap();
    std::fs::write(pc98.join("named.d88"), d88("PC98 DISK")).unwrap();
    let pc88_report = discover_source(&pc88).unwrap();
    let pc98_report = discover_source(&pc98).unwrap();
    assert_eq!(pc88_report.items.len(), 1, "{:?}", pc88_report.items);
    assert_eq!(pc98_report.items.len(), 1, "{:?}", pc98_report.items);
    assert_eq!(
        pc88_report.items[0].content,
        Some(ContentKind::ComputerDisk)
    );
    assert_eq!(
        pc98_report.items[0].content,
        Some(ContentKind::ComputerDisk)
    );
    assert_eq!(
        pc88_report.items[0].platform_hint.as_deref(),
        Some("NEC PC-8801")
    );
    assert_eq!(pc98_report.items[0].platform_hint.as_deref(), Some("PC-98"));

    let bare = source_dir("d88-bare");
    std::fs::write(bare.path().join("bare.d88"), d88("BARE DISK")).unwrap();
    std::fs::write(bare.path().join("random.d88"), vec![0x5a; 832]).unwrap();
    let report = discover_source(bare.path()).unwrap();
    assert_eq!(report.items.len(), 2, "{:?}", report.items);
    let item = |name: &str| {
        report
            .items
            .iter()
            .find(|item| item.path.ends_with(name))
            .unwrap_or_else(|| panic!("missing {name}: {:?}", report.items))
    };
    assert_eq!(item("bare.d88").content, Some(ContentKind::ComputerDisk));
    assert_eq!(item("bare.d88").platform_hint, None);
    assert_eq!(item("bare.d88").validation_state, ValidationState::Skipped);
    assert_eq!(
        item("random.d88").validation_state,
        ValidationState::Skipped
    );
    assert!(matches!(
        item("random.d88").skip_reason,
        Some(SkipReason::InvalidContent(_))
    ));
}

#[test]
fn z80_v1_snapshot_is_discovered_as_a_zx_spectrum_machine_snapshot() {
    let dir = source_dir("z80");
    std::fs::write(
        dir.path().join("Manic Miner (1983).z80"),
        z80_v1_uncompressed(),
    )
    .unwrap();
    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::MachineSnapshot));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(item.platform_hint.as_deref(), Some("ZX Spectrum"));
    assert!(item.explanation.contains("48K"));
    assert_eq!(report.stats.snapshots, 1);
}

#[test]
fn sna_128k_snapshot_is_discovered() {
    let dir = source_dir("sna");
    std::fs::write(dir.path().join("unrelated name.sna"), sna_128k()).unwrap();
    let report = discover_source(dir.path()).unwrap();
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::MachineSnapshot));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(item.platform_hint.as_deref(), Some("ZX Spectrum"));
}

// --- ZX Spectrum TR-DOS media (.trd / .scl) --------------------------

fn scl_archive(files: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"SINCLAIR");
    v.push(files.len() as u8);
    for &sectors in files {
        let mut e = [0u8; 14];
        e[..8].copy_from_slice(b"GAME    ");
        e[8] = b'C';
        e[13] = sectors;
        v.extend_from_slice(&e);
    }
    v.extend(std::iter::repeat_n(
        0u8,
        files.iter().map(|&s| usize::from(s) * 256).sum(),
    ));
    v
}

fn trd_disk_40ss() -> Vec<u8> {
    // 40-track single-sided: 40 * 16 * 256 = 163840 bytes.
    let total_sectors = 40u64 * 16;
    let mut v = vec![0u8; (total_sectors * 256) as usize];
    v[0..8].copy_from_slice(b"BOOT    ");
    v[8] = b'B';
    v[13] = 1;
    let d = 0x800usize;
    v[d + 0xE1] = 0; // first free sector
    v[d + 0xE2] = 1; // first free track
    v[d + 0xE3] = 0x19; // 40 SS
    v[d + 0xE4] = 1; // one file
    let free = (total_sectors - 16) as u16;
    v[d + 0xE5..d + 0xE7].copy_from_slice(&free.to_le_bytes());
    v[d + 0xE7] = 0x10; // TR-DOS id
    v[d + 0xF5..d + 0xFD].copy_from_slice(b"SPECCY  ");
    v
}

#[test]
fn scl_archive_is_discovered_as_zx_spectrum_media() {
    let dir = source_dir("scl");
    std::fs::write(dir.path().join("Some Collection.scl"), scl_archive(&[2, 4])).unwrap();
    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::ComputerDisk));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(item.platform_hint.as_deref(), Some("ZX Spectrum"));
    assert!(item.explanation.contains("TR-DOS"));
}

#[test]
fn trd_disk_is_discovered_as_zx_spectrum_media() {
    let dir = source_dir("trd");
    std::fs::write(dir.path().join("unrelated name.trd"), trd_disk_40ss()).unwrap();
    let report = discover_source(dir.path()).unwrap();
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::ComputerDisk));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(item.platform_hint.as_deref(), Some("ZX Spectrum"));
}

#[test]
fn random_bytes_named_like_a_spectrum_snapshot_are_not_accepted() {
    let dir = source_dir("z80-lie");
    std::fs::write(
        dir.path().join("Some Spectrum Game.z80"),
        b"not a snapshot at all",
    )
    .unwrap();
    let report = discover_source(dir.path()).unwrap();
    let item = &report.items[0];
    assert_eq!(item.validation_state, ValidationState::Skipped);
    assert_eq!(item.platform_hint, None);
    assert!(matches!(
        item.skip_reason,
        Some(SkipReason::InvalidContent(_))
    ));
    assert!(!item.explanation.contains("48K"));
}

#[test]
fn plus3_dsk_is_discovered_as_zx_spectrum() {
    let dir = source_dir("plus3-dsk");
    std::fs::write(dir.path().join("Game (1988).dsk"), dsk(40, 1, true)).unwrap();
    let report = discover_source(dir.path()).unwrap();
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::ComputerDisk));
    assert_eq!(item.validation_state, ValidationState::Accepted);
    assert_eq!(item.platform_hint.as_deref(), Some("ZX Spectrum"));
    assert!(item.explanation.contains("+3"));
}

#[test]
fn generic_cpc_dsk_stays_ambiguous_and_is_never_forced_to_spectrum() {
    let dir = source_dir("generic-dsk");
    std::fs::write(dir.path().join("data disk.dsk"), dsk(80, 2, false)).unwrap();
    let report = discover_source(dir.path()).unwrap();
    let item = &report.items[0];
    assert_eq!(item.content, Some(ContentKind::ComputerDisk));
    assert_ne!(item.platform_hint.as_deref(), Some("ZX Spectrum"));
    assert_eq!(item.validation_state, ValidationState::Skipped);
    assert_eq!(
        item.skip_reason,
        Some(SkipReason::RecognizedContentNoIdentityMatch)
    );
}

#[test]
fn truncated_dsk_fails_closed() {
    let dir = source_dir("trunc-dsk");
    let mut image = dsk(40, 1, true);
    image.truncate(image.len() - 2048);
    std::fs::write(dir.path().join("game.dsk"), image).unwrap();
    let report = discover_source(dir.path()).unwrap();
    let item = &report.items[0];
    assert_eq!(item.validation_state, ValidationState::Skipped);
    assert!(matches!(
        item.skip_reason,
        Some(SkipReason::InvalidContent(_))
    ));
    assert_eq!(item.platform_hint, None);
}

#[test]
fn random_bytes_named_trd_or_scl_are_not_spectrum_evidence() {
    let dir = source_dir("trdos-lies");
    // Correctly sized for an 80-track DS TR-DOS disk, but random.
    std::fs::write(
        dir.path().join("Speccy Game.trd"),
        (0..655360u32)
            .map(|i| (i * 91 + 5) as u8)
            .collect::<Vec<u8>>(),
    )
    .unwrap();
    std::fs::write(dir.path().join("Speccy Archive.scl"), vec![0x11u8; 8192]).unwrap();

    let report = discover_source(dir.path()).unwrap();
    assert_eq!(report.items.len(), 2);
    for item in &report.items {
        assert_eq!(item.validation_state, ValidationState::Skipped, "{item:?}");
        assert_eq!(item.platform_hint, None);
        assert!(matches!(
            item.skip_reason,
            Some(SkipReason::InvalidContent(_))
        ));
    }
}

#[test]
fn truncated_trd_fails_closed_in_discovery() {
    let dir = source_dir("trd-trunc");
    let mut image = trd_disk_40ss();
    image.truncate(0x400); // descriptor sector gone
    std::fs::write(dir.path().join("game.trd"), image).unwrap();
    let report = discover_source(dir.path()).unwrap();
    let item = &report.items[0];
    assert_eq!(item.validation_state, ValidationState::Skipped);
    assert!(matches!(
        item.skip_reason,
        Some(SkipReason::InvalidContent(_))
    ));
    assert_eq!(item.platform_hint, None);
}
