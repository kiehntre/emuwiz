//! Synthetic cards only. Every card here is built in a temporary folder.

use super::backup::{SourceState, backup_card, list_backups, verify_backup};
use super::discovery::{find_cards_in, sniff_format};
use super::external_tools::{find_external_tools_in, launch};
use super::*;
use crate::memory_card_inventory::tests::ps2_inventory_fixture;
use ps1_memcard::{BLOCK, CAPACITY, CardBuilder, FRAME, MemoryCard, Save};
use std::os::unix::fs::PermissionsExt;

/// Full-width Shift-JIS, the way games write save titles.
fn sjis_fullwidth(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for character in text.chars() {
        let code: u16 = match character {
            ' ' => 0x8140,
            '0'..='9' => 0x824F + (character as u16 - '0' as u16),
            'A'..='Z' => 0x8260 + (character as u16 - 'A' as u16),
            'a'..='z' => 0x8281 + (character as u16 - 'a' as u16),
            other => panic!("no full-width form for {other:?} in this helper"),
        };
        out.extend_from_slice(&code.to_be_bytes());
    }
    out
}

fn ps1_save(name: &str, blocks: usize, title: &str) -> Save {
    let mut data = vec![0u8; blocks * BLOCK];
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = ((index * 31 + name.len() * 7) & 0xff) as u8;
    }
    data[..4].copy_from_slice(&[b'S', b'C', 0x11, blocks as u8]);
    data[4..68].fill(0);
    let title = sjis_fullwidth(title);
    data[4..4 + title.len()].copy_from_slice(&title);
    Save {
        slot: 0,
        name: name.to_string(),
        dirent: Vec::new(),
        data,
    }
}

fn ps1_card(saves: &[Save]) -> Vec<u8> {
    let mut builder = CardBuilder::new();
    for save in saves {
        builder.add(save.clone());
    }
    builder.build().unwrap()
}

fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn ps1_card_lists_saves_with_title_code_and_region() {
    let dir = tempfile::tempdir().unwrap();
    let card = ps1_card(&[
        ps1_save("BASLUS-00594GAME", 1, "Crash 2 Save"),
        ps1_save("BESCES-01237DATA", 2, "Tomb File"),
    ]);
    let path = write(&dir, "shared_card_1.mcd", &card);
    let report = read_card(&path).unwrap();

    assert_eq!(report.format, CardFormat::Ps1Raw);
    assert_eq!(report.health, CardHealth::Healthy, "{:?}", report.notes);
    assert_eq!(report.readers_agree, Some(true));
    assert_eq!(report.free_space.as_deref(), Some("12 of 15 blocks free"));
    assert_eq!(report.export_format, Some("mcs"));
    assert_eq!(report.saves.len(), 2);
    let first = &report.saves[0];
    assert_eq!(first.title.as_deref(), Some("Crash 2 Save"));
    assert_eq!(first.product_code.as_deref(), Some("SLUS-00594"));
    assert_eq!(first.region, Some("America"));
    assert_eq!(first.blocks, Some(1));
    let second = &report.saves[1];
    assert_eq!(second.product_code.as_deref(), Some("SCES-01237"));
    assert_eq!(second.region, Some("Europe"));
    assert_eq!((second.blocks, second.size_bytes), (Some(2), 16_384));
    assert_eq!(
        fs::read(&path).unwrap(),
        card,
        "reading never changes a card"
    );
}

#[test]
fn cards_are_recognised_by_content_not_by_extension() {
    let dir = tempfile::tempdir().unwrap();
    let card = ps1_card(&[ps1_save("BASLUS-00594GAME", 1, "Save")]);
    write(&dir, "plain.srm", &card);
    let mut dexdrive = vec![0u8; 3904];
    dexdrive[..11].copy_from_slice(b"123-456-STD");
    dexdrive.extend_from_slice(&card);
    write(&dir, "dex.gme", &dexdrive);
    let mut vgs = vec![0u8; 64];
    vgs[..4].copy_from_slice(b"VgsM");
    vgs.extend_from_slice(&card);
    write(&dir, "vgs.mem", &vgs);
    let mut vmp = vec![0u8; 128];
    vmp[1..4].copy_from_slice(b"PMV");
    vmp.extend_from_slice(&card);
    let vmp_path = write(&dir, "SCEVMC0.VMP", &vmp);
    // A Game Boy Advance flash save is also exactly 128 KB.
    let gba = write(&dir, "gba-game.srm", &vec![0xFFu8; CAPACITY]);
    write(&dir, "notes.txt", b"not a card");
    write(&dir, "card.ps2", &ps2_inventory_fixture());
    let folder_card = dir.path().join("Folder.ps2");
    fs::create_dir_all(folder_card.join("BESLES-50009SAVE")).unwrap();
    fs::write(folder_card.join("_pcsx2_superblock"), b"x").unwrap();
    fs::write(folder_card.join("BESLES-50009SAVE/data.bin"), [1u8; 300]).unwrap();

    let found: Vec<(String, CardFormat)> = find_cards_in(dir.path(), Some("Test"))
        .into_iter()
        .map(|card| {
            (
                card.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                card.format,
            )
        })
        .collect();
    assert_eq!(
        found,
        vec![
            ("Folder.ps2".to_string(), CardFormat::Pcsx2Folder),
            ("SCEVMC0.VMP".to_string(), CardFormat::Ps1Vmp),
            ("card.ps2".to_string(), CardFormat::Ps2DataOnly),
            ("dex.gme".to_string(), CardFormat::Ps1DexDrive),
            ("plain.srm".to_string(), CardFormat::Ps1Raw),
            ("vgs.mem".to_string(), CardFormat::Ps1Vgs),
        ]
    );

    let not_a_card = read_card(&gba).unwrap();
    assert_eq!(not_a_card.format, CardFormat::NotACard);
    assert!(not_a_card.notes[0].contains("another system's save file"));
    assert_eq!(sniff_format(&gba), CardFormat::NotACard);

    // A signed PSP/PS3 card is read, and says plainly that it is never written.
    let report = read_card(&vmp_path).unwrap();
    assert_eq!(report.format, CardFormat::Ps1Vmp);
    assert_eq!(report.saves.len(), 1);
    assert!(
        report
            .unsupported
            .iter()
            .any(|line| line.contains("never writes one"))
    );

    let folder = read_card(&folder_card).unwrap();
    assert_eq!(folder.format, CardFormat::Pcsx2Folder);
    assert_eq!(folder.saves[0].name, "BESLES-50009SAVE");
    assert_eq!(folder.saves[0].product_code.as_deref(), Some("SLES-50009"));
    assert_eq!(
        (folder.export_format, folder.import_formats.len()),
        (None, 0)
    );
    assert!(matches!(
        export_save(&folder_card, "BESLES-50009SAVE", &dir.path().join("x.psu")),
        Err(CardError::NotAFile(_))
    ));
}

#[test]
fn rpcs3_ps1_classic_cards_are_found_inside_its_storage_folder() {
    let dir = tempfile::tempdir().unwrap();
    let saves = dir.path().join("dev_hdd0/game/NPEF00036/USRDIR/SAVEDATA");
    fs::create_dir_all(&saves).unwrap();
    let mut vmp = vec![0u8; 128];
    vmp[1..4].copy_from_slice(b"PMV");
    vmp.extend_from_slice(&ps1_card(&[ps1_save("BESCES-01237DATA", 1, "Classic")]));
    fs::write(saves.join("SCEVMC0.VMP"), &vmp).unwrap();
    fs::write(saves.join("ICON0.PNG"), b"not a card").unwrap();
    // Elsewhere in the storage folder: not looked at.
    fs::create_dir_all(dir.path().join("dev_hdd0/home/00000001")).unwrap();
    fs::write(dir.path().join("dev_hdd0/home/00000001/stray.VMP"), &vmp).unwrap();

    let found = super::discovery::find_rpcs3_cards(&dir.path().join("dev_hdd0"));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, saves.join("SCEVMC0.VMP"));
    assert_eq!(found[0].format, CardFormat::Ps1Vmp);
    assert_eq!(found[0].emulator.as_deref(), Some("RPCS3"));
    assert!(super::discovery::find_rpcs3_cards(&dir.path().join("missing")).is_empty());
}

#[test]
fn standard_folders_of_native_and_flatpak_installs_are_both_searched() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (config, data) = (home.join(".config"), home.join(".local/share"));
    let ps1 = ps1_card(&[ps1_save("BASLUS-00594GAME", 1, "Save")]);
    let ps2 = ps2_inventory_fixture();
    let mut vmp = vec![0u8; 128];
    vmp[1..4].copy_from_slice(b"PMV");
    vmp.extend_from_slice(&ps1);
    let files: [(&str, &[u8]); 11] = [
        (".config/PCSX2/memcards/Mcd001.ps2", &ps2),
        (
            ".var/app/net.pcsx2.PCSX2/config/PCSX2/memcards/Mcd001.ps2",
            &ps2,
        ),
        (".local/share/duckstation/memcards/shared_card_1.mcd", &ps1),
        (
            ".var/app/org.duckstation.DuckStation/config/duckstation/memcards/a.mcd",
            &ps1,
        ),
        (".config/retroarch/saves/Game (USA).srm", &ps1),
        (
            ".config/rpcs3/dev_hdd0/game/NPEF00036/USRDIR/SAVEDATA/SCEVMC0.VMP",
            &vmp,
        ),
        // Not cards, or not in a card folder: never listed.
        (".config/retroarch/saves/GBA Game.srm", &[0xFF; 131_072]),
        (".config/PCSX2/inis/PCSX2.ini", b"[UI]"),
        (".config/other/elsewhere.mcd", &ps1),
        // RetroArch sorting saves by core: one level down is searched...
        (".config/retroarch/saves/PCSX-ReARMed/Other (USA).srm", &ps1),
        // ...and no deeper.
        (".config/retroarch/saves/a/b/Deep (USA).srm", &ps1),
    ];
    for (path, bytes) in files {
        let path = home.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let before: Vec<_> = files
        .iter()
        .map(|(path, _)| fs::read(home.join(path)).unwrap())
        .collect();

    let mut found: Vec<(String, String)> =
        super::discovery::find_cards_in_standard_folders(home, &config, &data)
            .into_iter()
            .map(|card| {
                (
                    card.emulator.unwrap_or_default(),
                    card.path
                        .strip_prefix(home)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect();
    found.sort();
    let expected: Vec<(String, String)> = [
        ("DuckStation", files[2].0),
        ("DuckStation", files[3].0),
        ("PCSX2", files[0].0),
        ("PCSX2", files[1].0),
        ("RPCS3", files[5].0),
        ("RetroArch", files[4].0),
        ("RetroArch", files[9].0),
    ]
    .iter()
    .map(|(emulator, path)| (emulator.to_string(), path.to_string()))
    .collect();
    assert_eq!(found, expected);
    let after: Vec<_> = files
        .iter()
        .map(|(path, _)| fs::read(home.join(path)).unwrap())
        .collect();
    assert_eq!(before, after, "searching only reads");

    // Two installs sharing one card folder through a link: each card once.
    let shared = home.join(".var/app/org.libretro.RetroArch/config/retroarch");
    fs::create_dir_all(&shared).unwrap();
    std::os::unix::fs::symlink(home.join(".config/retroarch/saves"), shared.join("saves")).unwrap();
    let all = super::discovery::find_cards_in_standard_folders(home, &config, &data);
    let unique = super::discovery::without_duplicates(all.clone());
    assert_eq!(
        all.len(),
        9,
        "the linked folder is seen twice before de-duplication"
    );
    assert_eq!(unique.len(), 7);
    assert!(
        unique.iter().all(|card| !card.path.starts_with(&shared)),
        "first path wins"
    );
}

#[test]
fn ps1_save_round_trips_between_cards_and_no_card_is_modified() {
    let dir = tempfile::tempdir().unwrap();
    let wanted = ps1_save("BASLUS-00594GAME", 2, "Two Blocks");
    let source_bytes = ps1_card(&[ps1_save("BASLUS-01111AAAA", 1, "Other"), wanted.clone()]);
    let target_bytes = ps1_card(&[ps1_save("BISLPS-00001KEEP", 3, "Keep Me")]);
    let source = write(&dir, "source.mcd", &source_bytes);
    let target = write(&dir, "target.mcd", &target_bytes);

    let exported = dir.path().join("save.mcs");
    let export = export_save(&source, "BASLUS-00594GAME", &exported).unwrap();
    assert_eq!(export.format, "mcs");
    assert_eq!(export.bytes as usize, FRAME + 2 * BLOCK);
    assert!(matches!(
        export_save(&source, "BASLUS-00594GAME", &exported),
        Err(CardError::DestinationExists(_))
    ));

    let new_card = dir.path().join("target-with-save.mcd");
    let import = import_save_into_copy(&target, &exported, &new_card).unwrap();
    assert_eq!(import.save_name, "BASLUS-00594GAME");
    assert_eq!(import.new_card_format, CardFormat::Ps1Raw);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert_eq!(fs::read(&target).unwrap(), target_bytes);

    let report = read_card(&new_card).unwrap();
    assert_eq!(report.health, CardHealth::Healthy, "{:?}", report.notes);
    let names: Vec<&str> = report.saves.iter().map(|save| save.name.as_str()).collect();
    assert_eq!(names, ["BISLPS-00001KEEP", "BASLUS-00594GAME"]);
    assert_eq!(
        report.save("BASLUS-00594GAME").unwrap().title.as_deref(),
        Some("Two Blocks")
    );

    // The save's bytes survive the whole trip: card -> .mcs -> card -> .mcs.
    let new_bytes = fs::read(&new_card).unwrap();
    let parsed = MemoryCard::parse(&new_bytes).unwrap();
    let landed = parsed
        .saves()
        .iter()
        .find(|save| save.name == wanted.name)
        .unwrap();
    assert_eq!(landed.data, wanted.data);
    let again = dir.path().join("again.mcs");
    export_save(&new_card, "BASLUS-00594GAME", &again).unwrap();
    let (first, second) = (fs::read(&exported).unwrap(), fs::read(&again).unwrap());
    assert_eq!(first[FRAME..], second[FRAME..], "save blocks identical");
    assert_eq!(first[10..30], second[10..30], "save name identical");

    // Only the new save's two blocks (4 and 5) and their frames differ.
    for (offset, (old, new)) in target_bytes.iter().zip(&new_bytes).enumerate() {
        let touched =
            (4 * BLOCK..6 * BLOCK).contains(&offset) || (4 * FRAME..6 * FRAME).contains(&offset);
        assert!(
            old == new || touched,
            "byte {offset} changed outside the new save"
        );
    }
    // The same two saves filed by the library's own writer read identically.
    let reference = ps1_card(&[ps1_save("BISLPS-00001KEEP", 3, "Keep Me"), wanted]);
    let reference = MemoryCard::parse(&reference).unwrap();
    let listing = |card: &MemoryCard| -> Vec<(String, Vec<u8>)> {
        card.saves()
            .iter()
            .map(|save| (save.name.clone(), save.data.clone()))
            .collect()
    };
    assert_eq!(listing(&parsed), listing(&reference));
}

#[test]
fn ps1_psv_save_imports_and_unknown_formats_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let card = write(&dir, "card.mcd", &ps1_card(&[]));
    let save = ps1_save("BASCUS-94163FF7-S01", 1, "Psv Save");
    let mut psv = vec![0u8; 0x84];
    psv[1..4].copy_from_slice(b"VSP");
    psv[0x3C..0x40].copy_from_slice(&1u32.to_le_bytes());
    psv[0x40..0x44].copy_from_slice(&(BLOCK as u32).to_le_bytes());
    psv[0x44..0x48].copy_from_slice(&0x84u32.to_le_bytes());
    psv[0x64..0x64 + save.name.len()].copy_from_slice(save.name.as_bytes());
    psv.extend_from_slice(&save.data);
    let psv_path = write(&dir, "BASCUS-94163.PSV", &psv);

    let new_card = dir.path().join("new.mcd");
    import_save_into_copy(&card, &psv_path, &new_card).unwrap();
    let report = read_card(&new_card).unwrap();
    assert_eq!(report.health, CardHealth::Healthy, "{:?}", report.notes);
    assert_eq!(report.saves[0].name, "BASCUS-94163FF7-S01");
    assert_eq!(report.saves[0].title.as_deref(), Some("Psv Save"));

    // A raw block dump has no name inside it: refused, and said so.
    let raw = write(&dir, "raw.bin", &save.data);
    let error = import_save_into_copy(&card, &raw, &dir.path().join("raw.mcd")).unwrap_err();
    assert!(
        matches!(&error, CardError::Unsupported(text) if text.contains(".mcs")),
        "{error}"
    );
    assert!(!dir.path().join("raw.mcd").exists());
    // Same name twice, and a card without room.
    let error =
        import_save_into_copy(&new_card, &psv_path, &dir.path().join("dup.mcd")).unwrap_err();
    assert!(matches!(&error, CardError::Refused(text) if text.contains("already has a save")));
    let full = write(
        &dir,
        "full.mcd",
        &ps1_card(&[ps1_save("BASLUS-00001FULL", 15, "Full")]),
    );
    let error = import_save_into_copy(&full, &psv_path, &dir.path().join("full2.mcd")).unwrap_err();
    assert!(matches!(&error, CardError::Refused(text) if text.contains("free block")));
    // The result is never written over an existing file.
    assert!(matches!(
        import_save_into_copy(&card, &psv_path, &new_card),
        Err(CardError::DestinationExists(_))
    ));
}

#[test]
fn damaged_ps1_card_is_shown_with_a_warning_and_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let mut card = ps1_card(&[ps1_save("BASLUS-00594GAME", 2, "Looped")]);
    // Point the second block's link back at the first: a chain that loops.
    card[2 * FRAME + 8..2 * FRAME + 10].copy_from_slice(&0u16.to_le_bytes());
    card[3 * FRAME - 1] = ps1_memcard::frame_checksum(&card[2 * FRAME..3 * FRAME]);
    let path = write(&dir, "damaged.mcd", &card);

    let report = read_card(&path).unwrap();
    assert_eq!(report.health, CardHealth::Warnings);
    assert_eq!(report.readers_agree, Some(false));
    assert_eq!(report.saves.len(), 1, "still listed by EmuWiz's own reader");
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("ps1-memcard reader refused"))
    );
    assert_eq!(
        (report.export_format, report.import_formats.len()),
        (None, 0)
    );
    assert!(matches!(
        export_save(&path, "BASLUS-00594GAME", &dir.path().join("out.mcs")),
        Err(CardError::Refused(_))
    ));
    assert!(!dir.path().join("out.mcs").exists());
}

fn icon_sys(title: &str, second_line_at: usize) -> Vec<u8> {
    let mut icon = vec![0u8; 964];
    icon[..4].copy_from_slice(b"PS2D");
    icon[6..8].copy_from_slice(&(second_line_at as u16 * 2).to_le_bytes());
    let title = sjis_fullwidth(title);
    icon[0xC0..0xC0 + title.len()].copy_from_slice(&title);
    icon
}

fn add_ecc(data_only: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data_only.len() / 512 * 528);
    for page in data_only.chunks_exact(512) {
        let mut spare = [0u8; 16];
        ps2_memcard::ecc::fill_spare(page, &mut spare);
        out.extend_from_slice(page);
        out.extend_from_slice(&spare);
    }
    out
}

/// A `.psu` holding `icon.sys` and one data file, as uLaunchELF writes it.
fn psu(save_name: &str, title: &str, payload: &[u8]) -> Vec<u8> {
    const STAMP: [u8; 8] = [0, 6, 5, 4, 3, 2, 0xD1, 0x07];
    fn entry(mode: u32, length: u32, name: &str) -> Vec<u8> {
        let mut entry = vec![0u8; 512];
        entry[..4].copy_from_slice(&mode.to_le_bytes());
        entry[4..8].copy_from_slice(&length.to_le_bytes());
        entry[8..16].copy_from_slice(&STAMP);
        entry[24..32].copy_from_slice(&STAMP);
        entry[0x40..0x40 + name.len()].copy_from_slice(name.as_bytes());
        entry
    }
    let files = [
        ("icon.sys", icon_sys(title, 0)),
        ("data.bin", payload.to_vec()),
    ];
    let mut out = entry(0x8427, files.len() as u32 + 2, save_name);
    out.extend(entry(0x8427, 0, "."));
    out.extend(entry(0x8427, 0, ".."));
    for (name, data) in files {
        out.extend(entry(0x8497, data.len() as u32, name));
        out.extend_from_slice(&data);
        out.resize(out.len() + (1024 - data.len() % 1024) % 1024, 0);
    }
    out
}

#[test]
fn icon_sys_title_is_decoded_with_its_line_break() {
    assert_eq!(
        ps2::icon_sys_title(&icon_sys("GranTurismo 3", 4)).as_deref(),
        Some("Gran Turismo 3")
    );
    assert_eq!(
        ps2::icon_sys_title(&icon_sys("One Line", 0)).as_deref(),
        Some("One Line")
    );
    assert_eq!(ps2::icon_sys_title(b"not an icon"), None);
}

#[test]
fn ps2_cards_with_and_without_ecc_are_listed_by_both_readers() {
    let dir = tempfile::tempdir().unwrap();
    let plain = ps2_inventory_fixture();
    for (name, bytes, format) in [
        ("Mcd-plain.ps2", plain.clone(), CardFormat::Ps2DataOnly),
        ("Mcd001.ps2", add_ecc(&plain), CardFormat::Ps2WithEcc),
    ] {
        let path = write(&dir, name, &bytes);
        let report = read_card(&path).unwrap();
        assert_eq!(report.format, format);
        assert_eq!(
            report.health,
            CardHealth::Healthy,
            "{name}: {:?}",
            report.notes
        );
        assert_eq!(report.readers_agree, Some(true), "{name}");
        assert_eq!(report.saves.len(), 1);
        let save = &report.saves[0];
        assert_eq!(save.name, "BASLUS-00001TEST");
        assert_eq!(save.product_code.as_deref(), Some("SLUS-00001"));
        assert_eq!((save.file_count, save.size_bytes), (Some(1), 100));
        assert_eq!(save.modified.as_deref(), Some("2026-06-24 12:44"));
        assert!(
            report
                .free_space
                .as_deref()
                .is_some_and(|free| free.ends_with("KB free"))
        );
        assert_eq!(report.import_formats, ["psu"]);
        assert_eq!(sniff_format(&path), format);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn ps2_save_round_trips_into_copies_of_plain_and_ecc_cards() {
    let dir = tempfile::tempdir().unwrap();
    let payload: Vec<u8> = (0..5000u32).map(|value| (value % 251) as u8).collect();
    let save_file = write(
        &dir,
        "save.psu",
        &psu("BESLES-50009GAME", "Imported Save", &payload),
    );
    let plain = ps2_inventory_fixture();
    for (name, bytes) in [("plain.ps2", plain.clone()), ("ecc.ps2", add_ecc(&plain))] {
        let card = write(&dir, name, &bytes);
        let new_card = dir.path().join(format!("new-{name}"));
        let import = import_save_into_copy(&card, &save_file, &new_card).unwrap();
        assert_eq!(import.save_name, "BESLES-50009GAME");
        assert_eq!(
            fs::read(&card).unwrap(),
            bytes,
            "{name}: the card is untouched"
        );
        assert_eq!(fs::metadata(&new_card).unwrap().len(), bytes.len() as u64);

        let report = read_card(&new_card).unwrap();
        assert_eq!(
            report.health,
            CardHealth::Healthy,
            "{name}: {:?}",
            report.notes
        );
        assert_eq!(report.readers_agree, Some(true));
        let imported = report.save("BESLES-50009GAME").unwrap();
        assert_eq!(imported.title.as_deref(), Some("Imported Save"));
        assert_eq!(imported.region, Some("Europe"));
        assert_eq!(imported.file_count, Some(2));
        assert!(
            report.save("BASLUS-00001TEST").is_some(),
            "the old save is kept"
        );

        // Out again as a .psu: the same two files, byte for byte.
        let again = dir.path().join(format!("again-{name}.psu"));
        let export = export_save(&new_card, "BESLES-50009GAME", &again).unwrap();
        assert_eq!(export.format, "psu");
        // The standard layout other tools read: each file's record is followed
        // by its bytes. (EmuWiz's older export put every record first.)
        let again_bytes = fs::read(&again).unwrap();
        let back = ps2::parse_psu_standard(&again_bytes).expect("standard .psu layout");
        let original = ps2::parse_psu_standard(&fs::read(&save_file).unwrap()).unwrap();
        let contents = |psu: &ps2::Psu| -> Vec<Vec<u8>> {
            let mut files: Vec<Vec<u8>> = psu.files.iter().map(|(_, data)| data.clone()).collect();
            files.sort();
            files
        };
        assert_eq!(contents(&back), contents(&original));
        assert!(ps2::parse_psu_records_first(&again_bytes).is_none());
        assert_eq!(
            again_bytes.len(),
            fs::metadata(&save_file).unwrap().len() as usize
        );
        let library = ps2_memcard::MemoryCard::parse(&fs::read(&new_card).unwrap())
            .unwrap()
            .saves()
            .unwrap();
        let landed = library
            .iter()
            .find(|save| save.name == "BESLES-50009GAME")
            .unwrap();
        let data = landed
            .files
            .iter()
            .find(|file| file.name == "data.bin")
            .unwrap();
        assert_eq!(data.data, payload);
        // No working files are left beside the new card.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".emuwiz"))
            .collect();
        assert!(leftovers.is_empty());
    }
}

#[test]
fn ps2_import_also_reads_emuwiz_older_psu_layout_and_refuses_other_formats() {
    let dir = tempfile::tempdir().unwrap();
    let card = write(&dir, "card.ps2", &ps2_inventory_fixture());
    let standard = psu("BESLES-50009GAME", "Older Layout", b"some save data");
    let parsed = ps2::parse_psu_standard(&standard).unwrap();
    let records_first = ps2::write_psu_records_first(&parsed);
    assert_ne!(records_first, standard, "two files: the layouts differ");
    let older = write(&dir, "older.psu", &records_first);

    let new_card = dir.path().join("new.ps2");
    import_save_into_copy(&card, &older, &new_card).unwrap();
    let report = read_card(&new_card).unwrap();
    assert_eq!(
        report.save("BESLES-50009GAME").unwrap().title.as_deref(),
        Some("Older Layout")
    );

    // Not a .psu at all (an Action Replay .max starts "Ps2PowerSave"): refused in words.
    let max = write(&dir, "save.max", b"Ps2PowerSave and then compressed data");
    let error = import_save_into_copy(&card, &max, &dir.path().join("max.ps2")).unwrap_err();
    assert!(
        matches!(&error, CardError::Unsupported(text) if text.contains(".max")),
        "{error}"
    );
    // The same save a second time is refused; replacing is not offered.
    let error =
        import_save_into_copy(&new_card, &older, &dir.path().join("twice.ps2")).unwrap_err();
    assert!(matches!(error, CardError::Refused(_)), "{error}");
    assert!(!dir.path().join("max.ps2").exists() && !dir.path().join("twice.ps2").exists());
}

/// `(existing save bytes, imported payload)` as the second reader sees a card.
fn ps2_contents(card: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let saves = ps2_memcard::MemoryCard::parse(card)
        .unwrap()
        .saves()
        .unwrap();
    let file = |save: &str, file: &str| {
        saves
            .iter()
            .find(|candidate| candidate.name == save)
            .and_then(|save| save.files.iter().find(|candidate| candidate.name == file))
            .map(|file| file.data.clone())
            .unwrap_or_default()
    };
    (
        file("BASLUS-00001TEST", "icon.sys"),
        file("BESLES-50009GAME", "data.bin"),
    )
}

#[test]
fn pcsx2_change_marker_is_updated_only_on_a_confirmed_pcsx2_card() {
    let dir = tempfile::tempdir().unwrap();
    let save_file = write(
        &dir,
        "save.psu",
        &psu("BESLES-50009GAME", "Marked", b"payload"),
    );
    // As PCSX2 leaves a card: the console wrote the second page blank (so its
    // ECC is the blank-page code), and PCSX2 later put its 8-byte marker at
    // 0x210 without touching that code.
    let mut plain = ps2_inventory_fixture();
    plain[512..1024].fill(0xFF);
    let mut bytes = add_ecc(&plain);
    let marker = 0x2781_286F_1BC0_C994u64;
    bytes[0x210..0x218].copy_from_slice(&marker.to_le_bytes());
    let card = write(&dir, "Mcd001.ps2", &bytes);

    let report = read_card(&card).unwrap();
    assert_eq!(report.health, CardHealth::Healthy, "{:?}", report.notes);
    assert_eq!(report.import_formats, ["psu"], "{:?}", report.unsupported);

    let new_card = dir.path().join("new.ps2");
    let import = import_save_into_copy(&card, &save_file, &new_card).unwrap();
    assert!(
        import
            .checks
            .iter()
            .any(|check| check.contains("plus PCSX2's 8-byte"))
    );
    let new_bytes = fs::read(&new_card).unwrap();
    let new_marker = u64::from_le_bytes(new_bytes[0x210..0x218].try_into().unwrap());
    assert_ne!(new_marker, marker, "PCSX2 must see a card that changed");
    assert_eq!(
        new_bytes[0x218..0x420],
        bytes[0x218..0x420],
        "rest of that page and its ECC"
    );
    assert_eq!(read_card(&new_card).unwrap().health, CardHealth::Healthy);
    assert_eq!(
        ps2_contents(&new_bytes),
        (vec![0x5a; 100], b"payload".to_vec())
    );
    assert_eq!(fs::read(&card).unwrap(), bytes);
}

#[test]
fn bytes_at_the_marker_offset_are_kept_when_a_pcsx2_marker_is_not_proven() {
    let dir = tempfile::tempdir().unwrap();
    let save_file = write(
        &dir,
        "save.psu",
        &psu("BESLES-50009GAME", "Kept", b"payload"),
    );
    let blank_second_page = {
        let mut plain = ps2_inventory_fixture();
        plain[512..1024].fill(0xFF);
        plain
    };
    // Real data in the second page, written by the console with its own ECC.
    let mut real_data = ps2_inventory_fixture();
    for (index, byte) in real_data[512..1024].iter_mut().enumerate() {
        *byte = (index * 7 + 3) as u8;
    }
    // Real data whose ECC happens to equal the blank-page code (eight equal
    // bytes cancel out): the ECC alone must not be taken as proof.
    let mut ecc_blind = blank_second_page.clone();
    ecc_blind[512..520].fill(0x3C);
    let mut ecc_blind = add_ecc(&ecc_blind);
    let mut blank_code = [0u8; 16];
    ps2_memcard::ecc::fill_spare(&[0xFF; 512], &mut blank_code);
    assert_eq!(
        ecc_blind[1040..1052],
        blank_code[..12],
        "the code cannot tell them apart"
    );
    ecc_blind[1052..1056].fill(0);
    // A second page that was never written at all.
    let mut never_written = add_ecc(&blank_second_page);
    never_written[528..1056].fill(0xFF);

    for (name, bytes) in [
        ("console-data.ps2", add_ecc(&real_data)),
        ("ecc-blind.ps2", ecc_blind),
        ("never-written.ps2", never_written),
    ] {
        let card = write(&dir, name, &bytes);
        let new_card = dir.path().join(format!("new-{name}"));
        let import = import_save_into_copy(&card, &save_file, &new_card)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let new_bytes = fs::read(&new_card).unwrap();
        assert_eq!(
            new_bytes[528..1056],
            bytes[528..1056],
            "{name}: second page kept"
        );
        assert!(
            import
                .checks
                .iter()
                .any(|check| check.contains("No PCSX2 change marker was found")),
            "{name}: {:?}",
            import.checks
        );
        assert_eq!(
            read_card(&new_card).unwrap().health,
            CardHealth::Healthy,
            "{name}"
        );
        assert_eq!(
            ps2_contents(&new_bytes),
            (vec![0x5a; 100], b"payload".to_vec()),
            "{name}"
        );
        assert_eq!(
            fs::read(&card).unwrap(),
            bytes,
            "{name}: the card is untouched"
        );
    }
}

#[test]
fn console_settings_folder_is_shown_but_not_exported() {
    let dir = tempfile::tempdir().unwrap();
    // EmuWiz's own reader leaves BEDATA-SYSTEM closed; the second reader lists it.
    let mut bytes = ps2_inventory_fixture();
    let name_at = (41 + 9) * 1024 + 0x40;
    bytes[name_at..name_at + 16].copy_from_slice(b"BEDATA-SYSTEM\0\0\0");
    let card = write(&dir, "card.ps2", &bytes);

    let report = read_card(&card).unwrap();
    assert_eq!(report.health, CardHealth::Healthy, "{:?}", report.notes);
    assert_eq!(report.readers_agree, Some(true));
    assert_eq!(report.saves.len(), 1);
    assert_eq!(report.saves[0].name, "BEDATA-SYSTEM");
    assert!(report.saves[0].notes[0].contains("console's own settings"));
    let error = export_save(&card, "BEDATA-SYSTEM", &dir.path().join("out.psu")).unwrap_err();
    assert!(matches!(&error, CardError::Refused(text) if text.contains("settings folder")));
    assert!(!dir.path().join("out.psu").exists());

    // It survives an import into a copy untouched.
    let save_file = write(
        &dir,
        "save.psu",
        &psu("BESLES-50009GAME", "Game", b"payload"),
    );
    let new_card = dir.path().join("new.ps2");
    import_save_into_copy(&card, &save_file, &new_card).unwrap();
    let names: Vec<String> = read_card(&new_card)
        .unwrap()
        .saves
        .into_iter()
        .map(|save| save.name)
        .collect();
    assert_eq!(names, ["BESLES-50009GAME", "BEDATA-SYSTEM"]);
}

#[test]
fn pcsx2_page_export_of_a_multi_file_save_is_standard_and_restores_both_layouts() {
    use crate::memory_card_inventory::{
        apply_ps2_psu_export, inspect_memory_card, plan_ps2_psu_export, plan_ps2_psu_restore,
    };
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    // A card holding a two-file save (icon.sys + data.bin).
    let base = write(&dir, "base.ps2", &ps2_inventory_fixture());
    let payload: Vec<u8> = (0..3000u32).map(|value| (value % 199) as u8).collect();
    let incoming = psu("BESLES-50009GAME", "Two Files", &payload);
    let source = write(&dir, "in.psu", &incoming);
    let card = root.join("card.ps2");
    import_save_into_copy(&base, &source, &card).unwrap();

    // The export the PCSX2 page uses: standard layout, each record followed
    // by its bytes, exactly what an established tool writes for these files.
    let inventory = inspect_memory_card(&card).unwrap();
    let save = inventory
        .ps2_inventory
        .as_ref()
        .unwrap()
        .save_directories
        .iter()
        .find(|save| save.entry.display_name == "BESLES-50009GAME")
        .unwrap();
    let exported = root.join("exported.psu");
    apply_ps2_psu_export(&plan_ps2_psu_export(&inventory, save, &exported).unwrap()).unwrap();
    let bytes = fs::read(&exported).unwrap();
    let standard = ps2::parse_psu_standard(&bytes).expect("standard layout");
    assert!(ps2::parse_psu_records_first(&bytes).is_none());
    let original = ps2::parse_psu_standard(&incoming).unwrap();
    let files = |psu: &ps2::Psu| -> Vec<Vec<u8>> {
        let mut files: Vec<Vec<u8>> = psu.files.iter().map(|(_, data)| data.clone()).collect();
        files.sort();
        files
    };
    assert_eq!(files(&standard), files(&original));
    assert_eq!(bytes.len(), incoming.len());
    // An existing export is never overwritten.
    assert!(plan_ps2_psu_export(&inventory, save, &exported).is_err());
    assert_eq!(fs::read(&exported).unwrap(), bytes);

    // Its restore reads the standard file, a file from another tool, and an
    // export made by an earlier EmuWiz (every record first).
    let target = inspect_memory_card(&base).unwrap();
    let older = root.join("older.psu");
    fs::write(&older, ps2::write_psu_records_first(&standard)).unwrap();
    assert_ne!(fs::read(&older).unwrap(), bytes);
    for file in [&exported, &fs::canonicalize(&source).unwrap(), &older] {
        let plan = plan_ps2_psu_restore(&target, file, &root.join("backup.ps2"), false)
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        assert_eq!(plan.file_count, 2);
        let restored =
            crate::memory_card_inventory::build_restored_card(&plan, &fs::read(&base).unwrap())
                .unwrap();
        let library = ps2_memcard::MemoryCard::parse(&restored)
            .unwrap()
            .saves()
            .unwrap();
        let landed = library
            .iter()
            .find(|save| save.name == "BESLES-50009GAME")
            .unwrap();
        let data = landed
            .files
            .iter()
            .find(|file| file.name == "data.bin")
            .unwrap();
        assert_eq!(data.data, payload, "{}", file.display());
    }
    assert_eq!(
        fs::read(&base).unwrap(),
        ps2_inventory_fixture(),
        "no card was written"
    );
    assert!(!root.join("backup.ps2").exists());
}

#[test]
fn ps2_card_whose_ecc_emuwiz_cannot_reproduce_is_read_but_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = add_ecc(&ps2_inventory_fixture());
    bytes[512] ^= 0x55; // first page's stored code is no longer the computed one
    let card = write(&dir, "foreign-ecc.ps2", &bytes);
    let save_file = write(
        &dir,
        "save.psu",
        &psu("BESLES-50009GAME", "Save", b"payload"),
    );

    let report = read_card(&card).unwrap();
    assert_eq!(report.format, CardFormat::Ps2WithEcc);
    assert_eq!(report.export_format, Some("psu"));
    assert!(report.import_formats.is_empty());
    assert!(
        report
            .unsupported
            .iter()
            .any(|line| line.contains("error-correction"))
    );
    let new_card = dir.path().join("new.ps2");
    assert!(matches!(
        import_save_into_copy(&card, &save_file, &new_card),
        Err(CardError::Refused(_))
    ));
    assert!(!new_card.exists());
    export_save(&card, "BASLUS-00001TEST", &dir.path().join("out.psu")).unwrap();
}

#[test]
fn backup_is_a_verified_separate_copy() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = ps1_card(&[ps1_save("BASLUS-00594GAME", 1, "Backed Up")]);
    let card = write(&dir, "card.mcd", &bytes);
    let root = dir.path().join("backups");

    let backup = backup_card(&card, &root).unwrap();
    assert_eq!(
        fs::read(&card).unwrap(),
        bytes,
        "a backup only reads the card"
    );
    assert_eq!(fs::read(&backup.backup_file).unwrap(), bytes);
    assert_ne!(backup.backup_file, card);
    assert_eq!(backup.sha256, sha256_hex(&bytes));
    assert_eq!(backup.save_names, ["BASLUS-00594GAME"]);
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let check = verify_backup(&backup.record_file()).unwrap();
    assert!(check.backup_intact);
    assert_eq!(check.source, SourceState::Unchanged);
    assert_eq!(list_backups(&root), vec![backup.clone()]);

    // The (synthetic) card is played on: the backup still holds the old card.
    let mut played = bytes.clone();
    played[BLOCK + 200] ^= 0xFF;
    fs::write(&card, &played).unwrap();
    let check = verify_backup(&backup.record_file()).unwrap();
    assert!(check.backup_intact);
    assert_eq!(check.source, SourceState::ChangedSinceBackup);

    // A damaged backup is reported, never passed off as good.
    fs::write(&backup.backup_file, &played).unwrap();
    assert!(!verify_backup(&backup.record_file()).unwrap().backup_intact);

    let not_a_card = write(&dir, "notes.txt", b"hello");
    assert!(matches!(
        backup_card(&not_a_card, &root),
        Err(CardError::NotACard(_))
    ));
    let link = dir.path().join("link.mcd");
    std::os::unix::fs::symlink(&card, &link).unwrap();
    assert!(matches!(
        backup_card(&link, &root),
        Err(CardError::NotAFile(_))
    ));
}

#[test]
fn external_editors_are_found_by_name_and_started_without_a_card() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("started");
    let script = format!("#!/bin/sh\necho \"args=$#\" > '{}'\n", marker.display());
    let tool = dir.path().join("myMCpp-v0.3.3-linux-x64-clang.AppImage");
    fs::write(&tool, script).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    // Present but not executable, and an unrelated program: neither is offered.
    fs::write(dir.path().join("PSMemoryManager"), b"data").unwrap();
    fs::write(dir.path().join("mymc-notes.txt"), b"data").unwrap();

    let tools = find_external_tools_in(&[dir.path().to_path_buf()]);
    assert_eq!(tools.len(), 1);
    assert_eq!((tools[0].name, tools[0].licence), ("myMCpp", "GPL-3.0"));
    assert_eq!(tools[0].executable, tool);

    launch(&tools[0]).unwrap();
    for _ in 0..200 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(&marker).unwrap().trim(), "args=0");
}
