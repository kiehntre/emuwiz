use super::super::ContinuityError;
use super::super::compat::*;
use super::super::discover::*;
use super::super::model::*;
use super::*;
use crate::memory_card_inventory::tests::ps2_inventory_fixture;
use crate::memory_card_manager::import_save_into_copy;
use crate::memory_card_manager::tests::{add_ecc, psu};
use std::sync::atomic::AtomicBool;

/// A PS2 card (with ECC) holding the given saves.
pub(super) fn ps2_card(dir: &Path, name: &str, saves: &[(&str, &[u8])]) -> PathBuf {
    let mut current = dir.join(format!(".{name}.0"));
    fs::write(&current, add_ecc(&ps2_inventory_fixture())).unwrap();
    for (index, (save_name, payload)) in saves.iter().enumerate() {
        let file = dir.join(format!(".{name}.{index}.psu"));
        fs::write(&file, psu(save_name, "Synthetic Save", payload)).unwrap();
        let next = dir.join(format!(".{name}.{}", index + 1));
        import_save_into_copy(&current, &file, &next).unwrap();
        fs::remove_file(&file).unwrap();
        fs::remove_file(&current).unwrap();
        current = next;
    }
    let path = dir.join(name);
    fs::rename(&current, &path).unwrap();
    path
}

fn gci(code: &str, name: &str, blocks: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x40 + usize::from(blocks) * 0x2000];
    bytes[..6].copy_from_slice(code.as_bytes());
    bytes[6] = 0xFF;
    bytes[8..8 + name.len()].copy_from_slice(name.as_bytes());
    bytes[0x38..0x3A].copy_from_slice(&blocks.to_be_bytes());
    bytes[0x40..0x44].copy_from_slice(b"SAVE");
    bytes
}

/// A minimal PARAM.SFO with text keys.
pub(super) fn sfo(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut keys = Vec::new();
    let mut data = Vec::new();
    let mut index = Vec::new();
    for (key, value) in pairs {
        let (key_at, data_at) = (keys.len() as u16, data.len() as u32);
        keys.extend_from_slice(key.as_bytes());
        keys.push(0);
        let mut text = value.as_bytes().to_vec();
        text.push(0);
        let used = text.len() as u32;
        text.resize(text.len().next_multiple_of(4), 0);
        index.extend_from_slice(&key_at.to_le_bytes());
        index.extend_from_slice(&0x0204u16.to_le_bytes());
        index.extend_from_slice(&used.to_le_bytes());
        index.extend_from_slice(&(text.len() as u32).to_le_bytes());
        index.extend_from_slice(&data_at.to_le_bytes());
        data.extend(text);
    }
    keys.resize(keys.len().next_multiple_of(4), 0);
    let key_table = 20 + index.len() as u32;
    let mut out = b"\0PSF".to_vec();
    out.extend_from_slice(&0x0101u32.to_le_bytes());
    out.extend_from_slice(&key_table.to_le_bytes());
    out.extend_from_slice(&(key_table + keys.len() as u32).to_le_bytes());
    out.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
    out.extend(index);
    out.extend(keys);
    out.extend(data);
    out
}

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn path(&self, relative: &str) -> PathBuf {
        let path = self.dir.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    }
    fn put(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        fs::write(&path, bytes).unwrap();
        path
    }
    fn roots(&self) -> DiscoveryRoots {
        DiscoveryRoots::under_home(self.dir.path())
    }
    fn find(&self) -> DiscoveryReport {
        discover(
            &self.roots(),
            DiscoveryOptions {
                observer: Some(&CLOSED),
                ..Default::default()
            },
        )
        .unwrap()
    }
}

fn populated() -> Home {
    let home = Home::new();
    let native = home.path(".config/PCSX2/memcards/x");
    ps2_card(
        native.parent().unwrap(),
        "Mcd001.ps2",
        &[
            ("BASLUS-20827MANHUNT", b"manhunt progress"),
            ("BASCUS-97399GodOfWar", b"kratos"),
        ],
    );
    let flatpak = home.path(".var/app/net.pcsx2.PCSX2/config/PCSX2/memcards/x");
    ps2_card(
        flatpak.parent().unwrap(),
        "Mcd001.ps2",
        &[("BASLUS-20814MaxPay2", b"max")],
    );
    home.put(".var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu/GC/USA/Card A/01-GEDE-Eternal Darkness.gci", &gci("GEDE01", "Eternal Darkness", 2));
    home.put(
        ".var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu/GC/USA/Card A/notes.gci",
        b"not a save",
    );
    home.put(
        ".config/rpcs3/dev_hdd0/home/00000001/savedata/BLUS30750_RERC_SAVE/PARAM.SFO",
        &sfo(&[
            ("CATEGORY", "SD"),
            ("SAVEDATA_DIRECTORY", "BLUS30750_RERC_SAVE"),
            ("TITLE", "Resident Evil"),
        ]),
    );
    home.put(
        ".config/rpcs3/dev_hdd0/home/00000001/savedata/BLUS30750_RERC_SAVE/DATA.BIN",
        &[4u8; 2000],
    );
    let history = home
        .dir
        .path()
        .join(".config/retroarch/content_history.lpl");
    home.put(
        ".config/retroarch/retroarch.cfg",
        format!("savefile_directory = \"default\"\nhistory_list_enable = \"true\"\ncontent_history_path = \"{}\"\n", history.display()).as_bytes(),
    );
    home.put(".config/retroarch/saves/Zelda (USA).srm", &[0x42u8; 8192]);
    home.put(
        ".config/retroarch/saves/Blank Game (World).srm",
        &[0xFFu8; 8192],
    );
    home.put(".config/retroarch/saves/readme.txt", b"not a save");
    home.put(".config/retroarch/states/Zelda (USA).state1", &[1u8; 5000]);
    home
}

#[test]
fn saves_are_found_with_their_evidence_and_nothing_is_written() {
    let home = populated();
    let before = tree(home.dir.path());
    let report = home.find();
    assert_eq!(tree(home.dir.path()), before, "discovery is read-only");
    let by_name = |name: &str| {
        report
            .saves
            .iter()
            .find(|save| {
                save.entry_name.as_deref() == Some(name) || save.source_path.ends_with(name)
            })
            .unwrap_or_else(|| panic!("{name} not found"))
    };

    // A card holds several games; each is its own record, tied by product code.
    let manhunt = by_name("BASLUS-20827MANHUNT");
    assert_eq!(
        (manhunt.emulator.as_str(), manhunt.profile.as_str()),
        ("PCSX2", "native")
    );
    assert_eq!(manhunt.format, SaveFormat::Ps2CardSave);
    assert_eq!(
        manhunt.identity.strength,
        AssociationStrength::ContentProven
    );
    assert_eq!(manhunt.identity.product_code.as_deref(), Some("SLUS-20827"));
    assert_eq!(manhunt.in_use, InUse::NotDetected);
    assert!(manhunt.content_sha256.is_some() && manhunt.container_sha256.is_some());
    let gow = by_name("BASCUS-97399GodOfWar");
    assert_eq!(gow.source_path, manhunt.source_path);
    assert_ne!(gow.content_sha256, manhunt.content_sha256);
    assert_eq!(by_name("BASLUS-20814MaxPay2").profile, "flatpak");

    let gamecube = by_name("Eternal Darkness");
    assert_eq!(gamecube.identity.product_code.as_deref(), Some("GEDE01"));
    assert_eq!(gamecube.identity.region.as_deref(), Some("USA"));
    assert_eq!(
        gamecube.identity.strength,
        AssociationStrength::ContentProven
    );

    let ps3 = by_name("BLUS30750_RERC_SAVE");
    assert_eq!(ps3.identity.product_code.as_deref(), Some("BLUS30750"));
    assert_eq!(ps3.identity.title.as_deref(), Some("Resident Evil"));
    assert!(
        ps3.integrity
            .iter()
            .any(|line| line.contains("NOT verified"))
    );

    // A cartridge save's name is only ever a hint.
    let zelda = by_name("Zelda (USA).srm");
    assert_eq!(zelda.format, SaveFormat::CartridgeBattery);
    assert_eq!(zelda.identity.strength, AssociationStrength::NameOnly);
    assert!(zelda.identity.product_code.is_none() && zelda.identity.evidence.is_empty());
    assert!(
        by_name("Blank Game (World).srm")
            .notes
            .iter()
            .any(|note| note.contains("holds no data"))
    );
    let state = by_name("Zelda (USA).state1");
    assert_eq!(state.format, SaveFormat::SaveState);
    assert!(!state.unsupported.is_empty());

    assert!(
        report.skipped.iter().any(|line| line.contains("notes.gci")),
        "{:?}",
        report.skipped
    );
    assert!(
        !report
            .saves
            .iter()
            .any(|save| save.source_path.ends_with("readme.txt"))
    );
    assert!(!report.not_covered.is_empty());
}

#[test]
fn retroarch_history_upgrades_a_name_to_a_convention_only_when_it_is_unambiguous() {
    let home = populated();
    let playlist = |paths: &[&str]| {
        let items: Vec<String> = paths.iter().map(|path| format!(r#"{{"path":"{path}","label":"","core_path":"/cores/snes9x_libretro.so","core_name":"Snes9x","crc32":"","db_name":""}}"#)).collect();
        format!(r#"{{"version":"1.5","items":[{}]}}"#, items.join(","))
    };
    let zelda = |home: &Home| {
        home.find()
            .saves
            .into_iter()
            .find(|save| save.source_path.ends_with("Zelda (USA).srm"))
            .unwrap()
    };

    home.put(
        ".config/retroarch/content_history.lpl",
        playlist(&["/roms/snes/Zelda (USA).sfc"]).as_bytes(),
    );
    let one = zelda(&home);
    assert_eq!(
        one.identity.strength,
        AssociationStrength::EmulatorConvention
    );
    assert!(
        one.identity
            .ambiguity
            .iter()
            .any(|line| line.contains("do not say which game")),
        "still not proof"
    );

    // Two different game files with that name: back to a hint.
    home.put(
        ".config/retroarch/content_history.lpl",
        playlist(&["/roms/snes/Zelda (USA).sfc", "/roms/gb/Zelda (USA).gb"]).as_bytes(),
    );
    let two = zelda(&home);
    assert_eq!(two.identity.strength, AssociationStrength::NameOnly);
    assert!(two.identity.ambiguity[0].contains("2 different game files"));
}

#[test]
fn links_are_not_followed() {
    let home = populated();
    let secret = home.put("elsewhere/secret.srm", &[9u8; 100]);
    std::os::unix::fs::symlink(&secret, home.path(".config/retroarch/saves/Linked.srm")).unwrap();
    std::os::unix::fs::symlink(
        home.dir.path().join("elsewhere"),
        home.path(".config/retroarch/saves/linked-folder"),
    )
    .unwrap();
    let card = home.dir.path().join(".config/PCSX2/memcards/Mcd001.ps2");
    std::os::unix::fs::symlink(&card, home.path(".config/PCSX2/memcards/Mcd002.ps2")).unwrap();
    let report = home.find();
    assert!(
        !report
            .saves
            .iter()
            .any(|save| save.source_path.to_string_lossy().contains("Linked")
                || save.source_path.to_string_lossy().contains("linked-folder")
                || save.source_path.ends_with("Mcd002.ps2"))
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|line| line.contains("Linked.srm") && line.contains("link")),
        "{:?}",
        report.skipped
    );
}

#[test]
fn unchanged_files_are_not_read_again_and_any_change_is_noticed() {
    let home = populated();
    let mut cache = DiscoveryCache::default();
    let run = |cache: &mut DiscoveryCache| {
        discover(
            &home.roots(),
            DiscoveryOptions {
                cache: Some(cache),
                observer: Some(&CLOSED),
                ..Default::default()
            },
        )
        .unwrap()
    };
    let first = run(&mut cache);
    assert_eq!(first.stats.reused_from_cache, 0);
    assert!(first.stats.bytes_hashed > 0);
    let second = run(&mut cache);
    assert_eq!(second.saves, first.saves, "same answer either way");
    // Only the file that is not a save (10 bytes, never remembered) is looked at again.
    assert_eq!(
        (second.stats.files_read, second.stats.bytes_hashed),
        (1, 10)
    );

    // Same size, same modification time, different content: the change time gives it away.
    let srm = home
        .dir
        .path()
        .join(".config/retroarch/saves/Zelda (USA).srm");
    let mtime = fs::metadata(&srm).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(&srm, [0x43u8; 8192]).unwrap();
    fs::File::options()
        .write(true)
        .open(&srm)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let third = run(&mut cache);
    assert_eq!(third.stats.files_read, 2);
    let changed = |report: &DiscoveryReport| {
        report
            .saves
            .iter()
            .find(|save| save.source_path == srm)
            .unwrap()
            .content_sha256
            .clone()
    };
    assert_ne!(changed(&third), changed(&first));

    // The cache survives being written and read back.
    let file = home.dir.path().join("cache.json");
    cache.save(&file).unwrap();
    let mut loaded = DiscoveryCache::load(&file);
    assert_eq!(run(&mut loaded).stats.files_read, 1);
}

#[test]
fn discovery_can_be_cancelled_and_is_bounded() {
    let home = populated();
    let cancel = AtomicBool::new(true);
    let cancelled = discover(
        &home.roots(),
        DiscoveryOptions {
            cancel: Some(&cancel),
            ..Default::default()
        },
    );
    assert_eq!(cancelled.unwrap_err(), ContinuityError::Cancelled);
    let limited = discover(
        &home.roots(),
        DiscoveryOptions {
            max_files: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(limited.stats.stopped_at_limit);
    assert_eq!(limited.stats.files_examined, 2);
    // Without an observer nothing is ever reported as safe.
    assert!(
        limited
            .saves
            .iter()
            .all(|save| save.in_use == InUse::Unknown)
    );
}

#[test]
fn a_gci_must_be_structurally_complete() {
    assert!(parse_gci(&gci("GEDE01", "Eternal Darkness", 3)).is_some());
    let mut short = gci("GEDE01", "x", 3);
    short.truncate(short.len() - 1);
    assert!(parse_gci(&short).is_none(), "truncated");
    assert!(
        parse_gci(&gci("ge\x01e01", "x", 1)).is_none(),
        "unprintable code"
    );
    assert!(parse_gci(&[0u8; 10]).is_none());
}

fn target(emulator: &str, profile: &str, code: Option<&str>) -> ContinuationTarget {
    ContinuationTarget {
        emulator: emulator.into(),
        profile: profile.into(),
        game_product_code: code.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn compatibility_verdicts_follow_the_evidence() {
    let home = populated();
    let report = home.find();
    let find = |name: &str| {
        report
            .saves
            .iter()
            .find(|save| {
                save.entry_name.as_deref() == Some(name) || save.source_path.ends_with(name)
            })
            .unwrap()
    };
    let manhunt = find("BASLUS-20827MANHUNT");

    assert_eq!(
        assess(manhunt, &target("PCSX2", "native", None)).verdict,
        Compatibility::Verified
    );
    let other_profile = assess(manhunt, &target("PCSX2", "flatpak", Some("SLUS-20827")));
    assert_eq!(
        other_profile.verdict,
        Compatibility::AfterVerifiedConversion
    );
    let plan = other_profile.conversion.unwrap();
    assert!(plan.implemented && !plan.validation.is_empty() && !plan.risks.is_empty());
    // The European release of the same game does not read the American save.
    assert_eq!(
        assess(manhunt, &target("PCSX2", "flatpak", Some("SLES-52023"))).verdict,
        Compatibility::Incompatible
    );
    // Release not established: allowed, with the condition spelled out.
    assert!(
        assess(manhunt, &target("PCSX2", "flatpak", None))
            .unchecked_conditions
            .iter()
            .any(|line| line.contains("same release"))
    );
    assert_eq!(
        assess(manhunt, &target("RetroArch", "native", None)).verdict,
        Compatibility::Conditional
    );
    assert_eq!(
        assess(manhunt, &target("Dolphin", "native", None)).verdict,
        Compatibility::Incompatible
    );

    assert_eq!(
        assess(find("Eternal Darkness"), &target("Dolphin", "native", None)).verdict,
        Compatibility::Conditional
    );
    assert_eq!(
        assess(
            find("BLUS30750_RERC_SAVE"),
            &target("RPCS3", "flatpak", None)
        )
        .verdict,
        Compatibility::Unknown
    );
    assert_eq!(
        assess(
            find("BLUS30750_RERC_SAVE"),
            &target("PCSX2", "native", None)
        )
        .verdict,
        Compatibility::Incompatible
    );

    let srm = assess(
        find("Zelda (USA).srm"),
        &target("RetroArch", "flatpak", None),
    );
    assert_eq!(srm.verdict, Compatibility::Conditional);
    assert!(
        srm.reasons[0].contains("not confirmed"),
        "{:?}",
        srm.reasons
    );
    assert_eq!(
        assess(find("Zelda (USA).srm"), &target("mGBA", "native", None)).verdict,
        Compatibility::Unknown
    );

    // Save states never travel.
    let state = find("Zelda (USA).state1");
    assert_eq!(
        assess(state, &target("DuckStation", "native", None)).verdict,
        Compatibility::Incompatible
    );
    assert_eq!(
        assess(state, &target("RetroArch", "flatpak", None)).verdict,
        Compatibility::Unknown
    );
    for save in &report.saves {
        for emulator in [
            "PCSX2",
            "RetroArch",
            "DuckStation",
            "Dolphin",
            "RPCS3",
            "PPSSPP",
        ] {
            let verdict = assess(save, &target(emulator, "elsewhere", None));
            if save.format == SaveFormat::SaveState {
                assert!(matches!(
                    verdict.verdict,
                    Compatibility::Incompatible | Compatibility::Unknown
                ));
            }
            if verdict.verdict == Compatibility::AfterVerifiedConversion {
                assert!(
                    verdict.conversion.is_some_and(|plan| plan.implemented),
                    "a conversion is only promised where it exists"
                );
            }
        }
    }
}

#[test]
fn a_continuation_copy_is_a_new_card_and_both_originals_are_untouched() {
    let home = populated();
    let source = home.dir.path().join(".config/PCSX2/memcards/Mcd001.ps2");
    let destination = home
        .dir
        .path()
        .join(".var/app/net.pcsx2.PCSX2/config/PCSX2/memcards/Mcd001.ps2");
    let before = tree(home.dir.path());
    let work = home.dir.path().join("continuation");

    let copy =
        prepare_card_continuation(&source, "BASLUS-20827MANHUNT", &destination, &work).unwrap();
    let after: Vec<_> = tree(home.dir.path())
        .into_iter()
        .filter(|(path, _)| !path.starts_with(&work))
        .collect();
    assert_eq!(after, before, "nothing outside the new folder changed");
    assert!(copy.new_card.starts_with(&work) && copy.exported_save.starts_with(&work));

    // The new card holds the destination's save and the one carried over.
    let found = crate::memory_card_manager::read_card(&copy.new_card).unwrap();
    let mut names: Vec<&str> = found.saves.iter().map(|save| save.name.as_str()).collect();
    names.sort();
    assert!(
        names.contains(&"BASLUS-20827MANHUNT") && names.contains(&"BASLUS-20814MaxPay2"),
        "{names:?}"
    );
    assert!(
        copy.checks
            .iter()
            .any(|check| check.contains("same SHA-256 as on the source card"))
    );
    assert!(
        copy.next
            .iter()
            .any(|line| line.contains("did not start the game"))
    );
}

#[test]
fn a_save_already_at_the_destination_is_a_conflict_and_nothing_is_written() {
    let home = Home::new();
    let a_dir = home.path("a/x");
    let b_dir = home.path("b/x");
    let source = ps2_card(
        a_dir.parent().unwrap(),
        "Mcd001.ps2",
        &[("BASLUS-20827MANHUNT", b"desktop progress")],
    );
    let different = ps2_card(
        b_dir.parent().unwrap(),
        "Mcd001.ps2",
        &[("BASLUS-20827MANHUNT", b"laptop progress, longer")],
    );
    let same_dir = home.path("c/x");
    let same = ps2_card(
        same_dir.parent().unwrap(),
        "Mcd001.ps2",
        &[("BASLUS-20827MANHUNT", b"desktop progress")],
    );
    let before = tree(home.dir.path());

    let work = home.dir.path().join("work");
    let Err(ContinuationError::Conflict(conflict)) =
        prepare_card_continuation(&source, "BASLUS-20827MANHUNT", &different, &work)
    else {
        panic!("expected a conflict");
    };
    assert!(!conflict.identical);
    assert!(conflict.explanation.contains("will not replace either"));
    let Err(ContinuationError::Conflict(identical)) =
        prepare_card_continuation(&source, "BASLUS-20827MANHUNT", &same, &work)
    else {
        panic!("expected a conflict");
    };
    assert!(identical.identical);
    assert!(!work.exists());
    assert_eq!(tree(home.dir.path()), before);

    // Other refusals also leave nothing behind.
    assert!(matches!(
        prepare_card_continuation(&source, "NO-SUCH-SAVE", &different, &work),
        Err(ContinuationError::Stopped(ContinuityError::NotFound(_)))
    ));
    assert!(
        prepare_card_continuation(
            &source,
            "BASLUS-20827MANHUNT",
            &different,
            &a_dir.parent().unwrap().join("inside")
        )
        .is_err(),
        "not inside a card folder"
    );
    fs::create_dir(&work).unwrap();
    assert!(
        prepare_card_continuation(&source, "BASLUS-20827MANHUNT", &different, &work).is_err(),
        "existing folder"
    );
    fs::remove_dir(&work).unwrap();
    assert_eq!(tree(home.dir.path()), before);
}

// ---- RetroArch battery-save writing (read-only diagnostic) ----

#[test]
fn the_retroarch_diagnostic_reports_battery_save_writing_and_changes_nothing() {
    use super::super::diagnostics::{BatterySaveWriting, retroarch_installs};
    let home = Home::new();
    let native = home.put(
        ".config/retroarch/retroarch.cfg",
        b"autosave_interval = \"0\"\nsavestate_auto_save = \"false\"\n",
    );
    let flatpak = home.put(
        ".var/app/org.libretro.RetroArch/config/retroarch/retroarch.cfg",
        b"savestate_auto_save = \"true\"\nautosave_interval = \"10\"\n",
    );
    let before = tree(home.dir.path());
    let roots = home.roots();
    let found = retroarch_installs(&roots.home, &roots.config);
    assert_eq!(tree(home.dir.path()), before, "reads only");
    assert_eq!(found.len(), 2);

    let off = &found[0];
    assert_eq!(
        (off.install.as_str(), &off.config_file),
        ("native", &native)
    );
    assert_eq!(off.battery_saves, BatterySaveWriting::PeriodicWritingOff);
    assert_eq!(off.save_state_on_exit, Some(false));
    let text = off.advice.join(" ").to_lowercase();
    // "May", not "always"; what a snapshot cannot do; no promise.
    assert!(text.contains("may then stay only in memory"), "{text}");
    assert!(
        text.contains("cannot recover progress that was never written"),
        "{text}"
    );
    assert!(
        text.contains("not a guarantee for every core or game"),
        "{text}"
    );
    assert!(
        !text.contains("always") && !text.contains("only when it closes"),
        "{text}"
    );
    // Battery saves and save states are kept apart.
    assert!(
        text.contains("save states are a separate feature"),
        "{text}"
    );
    assert!(
        text.contains("emuwiz does not change this setting"),
        "{text}"
    );

    let on = &found[1];
    assert_eq!(
        (on.install.as_str(), &on.config_file),
        ("flatpak", &flatpak)
    );
    assert_eq!(on.battery_saves, BatterySaveWriting::Every { seconds: 10 });
    assert_eq!(on.save_state_on_exit, Some(true));
    assert!(!on.advice.join(" ").contains("may then stay only in memory"));

    // A setting that is missing or unreadable is "not established", not "off".
    let odd = Home::new();
    odd.put(
        ".config/retroarch/retroarch.cfg",
        b"autosave_interval = \"often\"\n",
    );
    let roots = odd.roots();
    assert!(matches!(
        retroarch_installs(&roots.home, &roots.config)[0].battery_saves,
        BatterySaveWriting::NotEstablished { .. }
    ));
    let empty = Home::new();
    empty.put(
        ".config/retroarch/retroarch.cfg",
        b"video_driver = \"gl\"\n",
    );
    let roots = empty.roots();
    assert!(matches!(
        retroarch_installs(&roots.home, &roots.config)[0].battery_saves,
        BatterySaveWriting::NotEstablished { .. }
    ));
    assert!(
        retroarch_installs(
            Path::new("/nonexistent-home"),
            Path::new("/nonexistent-home/.config")
        )
        .is_empty()
    );
}
