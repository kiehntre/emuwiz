use super::*;
use crate::patch_manager::{Pcsx2IdentityState, Pcsx2ProfileScope};
use std::os::unix::fs::symlink;

fn profile(path: &Path) -> Pcsx2Profile {
    Pcsx2Profile {
        profile_id: "test-profile".into(),
        installation_type: Pcsx2InstallationType::FlatpakUser,
        scope: Pcsx2ProfileScope::User,
        configuration_path: path.into(),
        provenance: "test",
        eligible: true,
        blockers: vec![],
        patch_directories: vec![],
        configuration_identity: None,
        executable_candidates: vec![],
    }
}

#[test]
fn overlay_preserves_real_cards_patches_and_copies_game_settings() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    for name in ["memcards", "sstates", "snaps", "videos"] {
        std::fs::create_dir_all(real.join(name)).unwrap();
    }
    let ini = config::Ini::parse("[Folders]\nBios = bios\nMemoryCards = memcards\nPatches = patches\nGameSettings = gamesettings\n[EmuCore]\nEnablePatches = true\nEnableWideScreenPatches = true\nEnableCheats = false\n[Unknown]\nRetain = opaque\n[MemoryCards]\nSlot1_Filename = Mcd001.ps2\n").unwrap();
    let workspace = dir.path().join("cheats-test");
    let result = config::overlay(
        ini,
        &profile(&real),
        &workspace,
        &dir.path().join("games/game.iso"),
    )
    .unwrap();
    assert_eq!(
        result.ini.get("Folders", "MemoryCards"),
        real.join("memcards").to_str()
    );
    assert_eq!(
        result.ini.get("Folders", "Patches"),
        real.join("patches").to_str()
    );
    assert_eq!(
        result.ini.get("Folders", "GameSettings"),
        workspace.join("PCSX2/gamesettings").to_str()
    );
    assert_eq!(
        result.ini.get("Folders", "Cheats"),
        workspace.join("PCSX2/cheats").to_str()
    );
    assert_eq!(
        result.ini.get("Folders", "Bios"),
        workspace.join("PCSX2/bios").to_str()
    );
    assert_eq!(result.ini.get("EmuCore", "EnablePatches"), Some("true"));
    assert_eq!(
        result.ini.get("EmuCore", "EnableWideScreenPatches"),
        Some("true")
    );
    assert_eq!(result.ini.get("Unknown", "Retain"), Some("opaque"));
    assert_eq!(result.ini.get("Cheats", "Enable"), Some(SELECTED_GROUP));
    assert!(!real.join("PCSX2.ini").exists());
    assert!(!workspace.exists()); // planning performs no staging
}

#[test]
fn ambiguous_ini_and_escaping_memory_cards_are_refused() {
    for text in [
        "[Folders]\nBios=a\nBios=b\n",
        "[Folders]\n[folders]\n",
        "[EmuCore]\nEnableCheats=true\nenablecheats=false\n",
    ] {
        assert!(config::Ini::parse(text).is_err());
    }
    for filename in ["/other/card.ps2", "../card.ps2", "nested/card.ps2"] {
        let ini =
            config::Ini::parse(&format!("[MemoryCards]\nSlot1_Filename={filename}\n")).unwrap();
        assert!(ini.validate_memory_cards().is_err());
    }
}

#[test]
fn path_normalization_never_hides_parent_traversal_or_links() {
    let dir = tempfile::tempdir().unwrap();
    assert!(config::folder(Some("../outside"), dir.path(), "bios").is_err());
    assert!(config::folder(Some("bad;grant"), dir.path(), "bios").is_err());
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), dir.path().join("link")).unwrap();
    assert!(config::folder(Some("link"), dir.path(), "bios").is_err());
    let fifo = dir.path().join("fifo");
    use std::ffi::CString;
    let name = CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(config::read_source(&fifo, 512).is_err());
}

#[test]
fn existing_pnach_parser_binds_crc_and_serial_without_a_new_converter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SLUS-00001_12345678.pnach");
    std::fs::write(&path, "patch=1,EE,00110000,word,12345678\n").unwrap();
    let mut identity = Pcsx2GameIdentity {
        archive_path: dir.path().join("game.iso"),
        title: "Homebrew".into(),
        region: None,
        serial: Some("SLUS-00001".into()),
        executable_crc: Some("12345678".into()),
        state: Pcsx2IdentityState::Verified,
        evidence: vec![],
        plain_failure_reason: None,
    };
    let discovered = discover_local_pcsx2_pnach_file(&path, &identity).unwrap();
    let before = discovered.source_sha256;
    identity.serial = Some("SLUS-00002".into());
    assert!(discover_local_pcsx2_pnach_file(&path, &identity).is_err());
    identity.serial = Some("SLUS-00001".into());
    std::fs::write(&path, "patch=1,EE,00110000,word,87654321\n").unwrap();
    assert_ne!(
        before,
        discover_local_pcsx2_pnach_file(&path, &identity)
            .unwrap()
            .source_sha256
    );
}

#[test]
fn staging_is_not_reported_as_emulator_activation_or_in_game_effect() {
    let path = Path::new("/private/cheats/SLUS-00001_12345678.pnach");
    let mut observation = Pcsx2CheatObservation::default();
    assert!(!observation.game_launch_handed_off);
    assert!(observation.label().contains("game has not started"));
    observation.game_launch_handed_off = true;
    flatpak::observe(
        "Found 1 cheats in /other.pnach.\nEnabled patch: Other\n",
        path,
        &mut observation,
    );
    assert!(!observation.file_loaded);
    assert!(!observation.selection_enabled);
    flatpak::observe(
        &format!(
            "Found 1 cheats in {}.\nEnabled patch: {SELECTED_GROUP}\n",
            path.display()
        ),
        path,
        &mut observation,
    );
    assert!(observation.file_loaded && observation.selection_enabled);
    assert!(observation.label().contains("In-game effect NOT CONFIRMED"));
}

#[test]
fn unknown_instance_lifetime_is_never_safe_to_delete() {
    let dir = tempfile::tempdir().unwrap();
    assert!(workspace_may_be_active(dir.path()));
    std::fs::write(dir.path().join(INSTANCE_FILE), "not-an-instance\n").unwrap();
    assert!(workspace_may_be_active(dir.path()));
    let target = dir.path().join("outside");
    std::fs::write(&target, "12345").unwrap();
    std::fs::remove_file(dir.path().join(INSTANCE_FILE)).unwrap();
    symlink(&target, dir.path().join(INSTANCE_FILE)).unwrap();
    assert!(workspace_may_be_active(dir.path()));
}

#[test]
fn bios_and_console_state_are_copied_verified_and_never_pointed_back_at_production() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    for name in ["bios", "memcards", "sstates", "snaps", "videos"] {
        std::fs::create_dir_all(real.join(name)).unwrap();
    }
    let bios = b"original synthetic BIOS fixture";
    std::fs::write(real.join("bios/test.bin"), bios).unwrap();
    std::fs::write(real.join("bios/test.nvm"), b"original console state").unwrap();
    let ini = config::Ini::parse("[Filenames]\nBIOS=test.bin\n").unwrap();
    let workspace = dir.path().join("cheats-bios");
    let mut overlay = config::overlay(
        ini,
        &profile(&real),
        &workspace,
        &dir.path().join("game.iso"),
    )
    .unwrap();
    use md5::Md5;
    use sha1::{Sha1, digest::Digest};
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
    let evidence = FirmwareIdentityRecord {
        system: crate::dat::firmware_evidence::FirmwareSystem::PlayStation2,
        provider: crate::dat::model::DatEcosystem::Redump,
        name: "Synthetic fixture".into(),
        description: None,
        size_bytes: bios.len() as u64,
        crc32: crate::identity_source::hashing::Crc32::of(bios),
        md5: hex(&Md5::digest(bios)),
        sha1: hex(&Sha1::digest(bios)),
        dat_version: None,
    };
    let mut sources = vec![];
    let mut files = vec![];
    assert!(
        config::stage_bios(
            &mut overlay,
            &[],
            &workspace.join("PCSX2"),
            &mut sources,
            &mut files
        )
        .is_err()
    );
    assert!(files.is_empty() && sources.is_empty());
    config::stage_bios(
        &mut overlay,
        &[evidence],
        &workspace.join("PCSX2"),
        &mut sources,
        &mut files,
    )
    .unwrap();
    let image = files
        .iter()
        .find(|f| f.kind == RuntimeFileKind::VerifiedResource)
        .unwrap();
    assert_eq!(image.bytes, bios);
    assert_eq!(image.path, workspace.join("PCSX2/bios/test.bin"));
    assert_eq!(
        sources
            .iter()
            .find(|s| s.expected_sha256 == image.sha256)
            .unwrap()
            .path,
        real.join("bios/test.bin")
    );
    assert!(
        files
            .iter()
            .any(|f| f.path == workspace.join("PCSX2/bios/test.nvm")
                && f.bytes == b"original console state")
    );
    assert_eq!(
        std::fs::read(real.join("bios/test.nvm")).unwrap(),
        b"original console state"
    );
    assert!(!workspace.exists());
}

#[test]
fn hardcore_refusal_respects_the_effective_per_game_settings() {
    let global = config::Ini::parse("[Achievements]\nEnabled=true\nChallengeMode=false\n").unwrap();
    global.validate_hardcore(None).unwrap();
    let game = config::Ini::parse("[Achievements]\nChallengeMode=true\n").unwrap();
    assert!(game.validate_hardcore(Some(&global)).is_err());
    let off = config::Ini::parse("[Achievements]\nEnabled=false\nChallengeMode=true\n").unwrap();
    off.validate_hardcore(None).unwrap();
}

#[test]
fn alternate_case_memory_card_and_hardcore_keys_cannot_bypass_validation() {
    for key in ["Slot1_filename", "SLOT1_FILENAME", "Slot1_FiLeNaMe"] {
        let ini = config::Ini::parse(&format!("[MemoryCards]\n{key}=../outside.ps2\n")).unwrap();
        assert!(ini.validate_memory_cards().is_err(), "{key}");
    }
    for text in [
        "[Achievements]\nenabled=true\nChallengeMode=true\n",
        "[achievements]\nEnabled=true\nchallengemode=true\n",
        "[Achievements]\nEnabled=true\nChallengeMode=YES\n",
    ] {
        match config::Ini::parse(text) {
            Ok(ini) => assert!(ini.validate_hardcore(None).is_err(), "{text}"),
            Err(_) => {}
        }
    }
}

#[test]
fn running_sandbox_must_report_the_tested_application_and_deployment() {
    for (app, commit) in [
        (APP_ID, TESTED_DEPLOYMENT),
        ("other.App", TESTED_DEPLOYMENT),
        (APP_ID, "updated"),
        (APP_ID, ""),
    ] {
        let ini = config::Ini::parse(&format!(
            "[Application]\nname={app}\n[Instance]\napp-commit={commit}\n"
        ))
        .unwrap();
        assert_eq!(
            flatpak::verify_deployment(&ini).is_ok(),
            app == APP_ID && commit == TESTED_DEPLOYMENT
        );
    }
}

#[test]
fn case_insensitive_lookup_detects_per_game_bios_overrides() {
    for key in ["BIOS", "bios", "Bios"] {
        let ini = config::Ini::parse(&format!("[Filenames]\n{key}=/original/bios.bin\n")).unwrap();
        assert_eq!(ini.get("Filenames", "BIOS"), Some("/original/bios.bin"));
    }
}

#[test]
fn spaced_section_aliases_cannot_bypass_protected_ini_settings() {
    for text in [
        "[ Filenames ]\nbios=/original/bios.bin\n",
        "[ MemoryCards ]\nSlot1_Filename=../outside.ps2\n",
        "[ Achievements ]\nEnabled=true\nChallengeMode=true\n",
        "[Folders]\nBios=bios\n[ Folders ]\nBios=other\n",
    ] {
        assert!(config::Ini::parse(text).is_err(), "{text}");
    }
}
