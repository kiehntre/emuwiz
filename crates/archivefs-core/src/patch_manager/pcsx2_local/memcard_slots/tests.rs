//! Representative PCSX2.ini fixtures for slot assignment.

use super::super::{inspect_pcsx2_config, pcsx2_global_config_path};
use super::*;
use std::fs;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn profile(ini: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("PCSX2");
    fs::create_dir_all(root.join("inis")).unwrap();
    fs::create_dir_all(root.join("memcards")).unwrap();
    if let Some(ini) = ini {
        fs::write(root.join("inis/PCSX2.ini"), ini).unwrap();
    }
    Fixture { _dir: dir, root }
}

fn resolve(fixture: &Fixture) -> Pcsx2MemcardSlots {
    let global = inspect_pcsx2_config(&pcsx2_global_config_path(&fixture.root));
    resolve_pcsx2_memcard_slots(&fixture.root, &global, None)
}

fn card(path: &Path) {
    fs::write(path, [0u8; 16]).unwrap();
}

/// Snapshot of every file under the profile, to prove nothing is written.
fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.push((path.clone(), Vec::new()));
                stack.push(path);
            } else {
                out.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

const STOCK: &str = "[UI]\nSettingsVersion = 1\n\n[Folders]\nBios = bios\nMemoryCards = memcards\n\n[MemoryCards]\nSlot1_Enable = true\nSlot1_Filename = Mcd001.ps2\nSlot2_Enable = true\nSlot2_Filename = Mcd002.ps2\nMultitap1_Slot2_Enable = false\nMultitap1_Slot2_Filename = Mcd-Multitap1-Slot02.ps2\n";

#[test]
fn stock_settings_assign_both_slots_and_change_nothing() {
    let fixture = profile(Some(STOCK));
    card(&fixture.root.join("memcards/Mcd001.ps2"));
    card(&fixture.root.join("memcards/Mcd002.ps2"));
    let before = tree(&fixture.root);
    let slots = resolve(&fixture);
    assert_eq!(slots.folder, Some(fixture.root.join("memcards")));
    assert_eq!(slots.slots.len(), 2);
    for (index, slot) in slots.slots.iter().enumerate() {
        let n = index as u8 + 1;
        assert_eq!(slot.slot, n);
        assert_eq!(slot.state, Pcsx2MemcardSlotState::Enabled);
        assert_eq!(slot.source, Pcsx2MemcardSlotSource::Settings);
        assert_eq!(
            slot.path,
            Some(fixture.root.join(format!("memcards/Mcd00{n}.ps2")))
        );
        assert!(!slot.folder_card);
    }
    assert_eq!(slots.assigned_card_files().count(), 2);
    assert!(slots.notes.is_empty());
    assert_eq!(tree(&fixture.root), before, "resolution must be read-only");
}

#[test]
fn custom_folder_and_custom_filenames_are_followed() {
    let fixture = profile(None);
    let elsewhere = fixture.root.parent().unwrap().join("My Cards");
    fs::create_dir_all(&elsewhere).unwrap();
    card(&elsewhere.join("RPG.ps2"));
    let ini = format!(
        "[Folders]\nMemoryCards = {}\n[MemoryCards]\nSlot1_Enable = true\nSlot1_Filename = RPG.ps2\nSlot2_Enable = true\nSlot2_Filename = Racing.ps2\n",
        elsewhere.display()
    );
    fs::write(fixture.root.join("inis/PCSX2.ini"), ini).unwrap();
    let slots = resolve(&fixture);
    assert_eq!(slots.folder.as_deref(), Some(elsewhere.as_path()));
    assert_eq!(slots.slots[0].state, Pcsx2MemcardSlotState::Enabled);
    assert_eq!(slots.slots[0].path, Some(elsewhere.join("RPG.ps2")));
    assert_eq!(slots.slots[1].state, Pcsx2MemcardSlotState::Missing);
    assert_eq!(slots.slots[1].filename.as_deref(), Some("Racing.ps2"));
    assert!(
        !elsewhere.join("Racing.ps2").exists(),
        "a missing card is never created"
    );
}

#[test]
fn relative_custom_folder_resolves_against_profile_root() {
    let fixture = profile(Some(
        "[Folders]\nMemoryCards = cards/ps2\n[MemoryCards]\nSlot1_Filename = A.ps2\n",
    ));
    fs::create_dir_all(fixture.root.join("cards/ps2")).unwrap();
    card(&fixture.root.join("cards/ps2/A.ps2"));
    let slots = resolve(&fixture);
    assert_eq!(
        slots.slots[0].path,
        Some(fixture.root.join("cards/ps2/A.ps2"))
    );
    assert_eq!(slots.slots[0].state, Pcsx2MemcardSlotState::Enabled);
}

#[test]
fn disabled_and_unassigned_slots_are_distinguished() {
    let fixture = profile(Some(
        "[MemoryCards]\nSlot1_Enable = false\nSlot1_Filename = Mcd001.ps2\nSlot2_Enable = true\nSlot2_Filename =\n",
    ));
    card(&fixture.root.join("memcards/Mcd001.ps2"));
    let slots = resolve(&fixture);
    assert_eq!(slots.slots[0].state, Pcsx2MemcardSlotState::Disabled);
    assert_eq!(slots.slots[0].filename.as_deref(), Some("Mcd001.ps2"));
    assert_eq!(slots.slots[1].state, Pcsx2MemcardSlotState::Unassigned);
    assert_eq!(slots.slots[1].path, None);
    assert_eq!(slots.assigned_card_files().count(), 0);
}

#[test]
fn absent_slot_keys_use_pcsx2_defaults_and_say_so() {
    let fixture = profile(Some("[UI]\nSettingsVersion = 1\n"));
    card(&fixture.root.join("memcards/Mcd001.ps2"));
    let slots = resolve(&fixture);
    assert_eq!(slots.slots[0].source, Pcsx2MemcardSlotSource::Default);
    assert_eq!(slots.slots[0].state, Pcsx2MemcardSlotState::Enabled);
    assert_eq!(slots.slots[1].filename.as_deref(), Some("Mcd002.ps2"));
    assert_eq!(slots.slots[1].state, Pcsx2MemcardSlotState::Missing);
}

#[test]
fn folder_memory_card_is_recognised_but_not_offered_as_an_image() {
    let fixture = profile(Some(
        "[MemoryCards]\nSlot1_Enable = true\nSlot1_Filename = Mcd001.ps2\n",
    ));
    fs::create_dir_all(fixture.root.join("memcards/Mcd001.ps2")).unwrap();
    let slots = resolve(&fixture);
    assert_eq!(slots.slots[0].state, Pcsx2MemcardSlotState::Enabled);
    assert!(slots.slots[0].folder_card);
    assert_eq!(slots.assigned_card_files().count(), 0);
}

#[test]
fn missing_or_unreadable_settings_are_unknown_not_defaulted() {
    let fixture = profile(None);
    card(&fixture.root.join("memcards/Mcd001.ps2"));
    let slots = resolve(&fixture);
    assert!(slots.folder.is_none());
    assert!(slots.slots.iter().all(|slot| {
        slot.state == Pcsx2MemcardSlotState::Unsupported
            && slot.source == Pcsx2MemcardSlotSource::Unknown
            && slot
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("not found"))
    }));
}

#[test]
fn ambiguous_values_are_unsupported_not_guessed() {
    for (ini, expect) in [
        ("[MemoryCards]\nSlot1_Enable = maybe\n", "is not on/off"),
        (
            "[MemoryCards]\nSlot1_Filename = ../outside.ps2\n",
            "includes a folder",
        ),
        (
            "[MemoryCards]\nSlot1_Filename = /abs/card.ps2\n",
            "includes a folder",
        ),
        (
            "[MemoryCards]\nSlot1_Filename = sub\\card.ps2\n",
            "includes a folder",
        ),
        (
            "[MemoryCards]\nSlot1_Filename = A.ps2\nslot1_filename = B.ps2\n",
            "more than once",
        ),
    ] {
        let fixture = profile(Some(ini));
        let slots = resolve(&fixture);
        let slot = &slots.slots[0];
        assert_eq!(slot.state, Pcsx2MemcardSlotState::Unsupported, "{ini}");
        assert!(
            slot.detail.as_deref().is_some_and(|d| d.contains(expect)),
            "{ini}: {:?}",
            slot.detail
        );
        // The other slot is unaffected and still resolved from defaults.
        assert_ne!(slots.slots[1].state, Pcsx2MemcardSlotState::Unsupported);
    }
}

#[test]
fn case_insensitive_spelling_matches_pcsx2_reader() {
    let fixture = profile(Some(
        "[memorycards]\nslot2_enable = FALSE\nSLOT2_FILENAME = Other.ps2\n",
    ));
    let slots = resolve(&fixture);
    assert_eq!(slots.slots[1].state, Pcsx2MemcardSlotState::Disabled);
    assert_eq!(slots.slots[1].filename.as_deref(), Some("Other.ps2"));
}

#[test]
fn symlinked_card_is_not_followed() {
    let fixture = profile(Some("[MemoryCards]\nSlot1_Filename = Link.ps2\n"));
    let target = fixture.root.join("real.ps2");
    card(&target);
    std::os::unix::fs::symlink(&target, fixture.root.join("memcards/Link.ps2")).unwrap();
    let slots = resolve(&fixture);
    assert_eq!(slots.slots[0].state, Pcsx2MemcardSlotState::Unsupported);
    assert!(
        slots.slots[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("symbolic link")
    );
}

#[test]
fn per_game_slot_override_is_reported_as_unconfirmed() {
    let fixture = profile(Some(STOCK));
    card(&fixture.root.join("memcards/Mcd001.ps2"));
    let game_ini = fixture.root.join("inis/gamesettings/SLUS-20946.ini");
    fs::create_dir_all(game_ini.parent().unwrap()).unwrap();
    fs::write(&game_ini, "[MemoryCards]\nSlot1_Filename = Game.ps2\n").unwrap();
    let global = inspect_pcsx2_config(&pcsx2_global_config_path(&fixture.root));
    let per_game = inspect_pcsx2_config(&game_ini);
    let slots = resolve_pcsx2_memcard_slots(&fixture.root, &global, Some(&per_game));
    assert_eq!(slots.slots[0].state, Pcsx2MemcardSlotState::Unsupported);
    assert!(
        slots.slots[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("cannot confirm")
    );
    assert_eq!(slots.slots[1].state, Pcsx2MemcardSlotState::Missing);
}

#[test]
fn enabled_multitap_is_noted() {
    let fixture = profile(Some("[EmuCore]\nMultitapPort0_Enabled = true\n"));
    let slots = resolve(&fixture);
    assert_eq!(slots.notes.len(), 1);
    assert!(slots.notes[0].contains("multitap"));
}
