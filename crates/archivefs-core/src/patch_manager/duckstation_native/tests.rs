//! Tests for the DuckStation native cheat adapter. Every test works inside its
//! own temporary directory; none touches a real DuckStation profile.

use super::*;

const SERIAL: &str = "SLUS-00067";

struct World {
    dir: tempfile::TempDir,
    profile: PathBuf,
    game: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("duckstation");
        fs::create_dir_all(&profile).unwrap();
        let game = dir.path().join("game.cue");
        fs::write(&game, b"FILE \"game.bin\" BINARY\n").unwrap();
        Self { dir, profile, game }
    }

    fn cheats(&self) -> PathBuf {
        self.profile.join("cheats")
    }

    fn game_settings(&self) -> PathBuf {
        self.profile.join("gamesettings")
    }

    fn cht(&self) -> PathBuf {
        self.cheats().join(format!("{SERIAL}.cht"))
    }

    fn ini(&self) -> PathBuf {
        self.game_settings().join(format!("{SERIAL}.ini"))
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn options(&self, id: &str) -> DuckStationNativeApplyOptions {
        DuckStationNativeApplyOptions {
            general_approved: true,
            replacement_approved: true,
            operation_id: id.to_string(),
            timestamp_unix_seconds: 1_700_000_000,
            history_root: self.dir.path().join("history"),
            backup_root: self.dir.path().join("backups"),
        }
    }

    fn request(&self, operation: DuckStationNativeOperation) -> DuckStationNativeRequest {
        DuckStationNativeRequest {
            profile_root: self.profile.clone(),
            selected_game: self.game.clone(),
            verified_serials: vec![SERIAL.to_string()],
            disc_topology: DuckStationDiscTopology::SingleDisc,
            operation,
            acknowledge_database_shadowing: true,
        }
    }
}

fn cheat(name: &str, code: &str) -> DuckStationNativeCheat {
    DuckStationNativeCheat {
        name: name.to_string(),
        metadata: BTreeMap::from([("Description".to_string(), "test".to_string())]),
        comments: vec!["; added by EmuWiz".to_string()],
        code_lines: vec![code.to_string()],
    }
}

fn add(name: &str, code: &str) -> DuckStationNativeOperation {
    DuckStationNativeOperation::Add {
        cheat: cheat(name, code),
        enable: true,
    }
}

fn text(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn sha(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| sha256_hex(&bytes))
}

const EXISTING_CHT: &str = "; my own header\n\n[Keep Me]\n; why I keep this\nType = Gameshark\nActivation = EndFrame\nCustomKey=odd spacing\n80123456 03E7\nA0123456 00000001\n\n[Also Mine]\nType = Gameshark\n30123457 00000002\n";
const EXISTING_INI: &str = "[Main]\nSetting = 1\n\n[Display]\nRenderer = Vulkan\n";

fn apply(world: &World, plan: &DuckStationNativePlan, id: &str) -> DuckStationNativeReceipt {
    apply_duckstation_native_plan(plan, &world.options(id))
        .unwrap_or_else(|failure| panic!("apply failed: {failure:?}"))
}

// ---- identity routing -------------------------------------------------------

#[test]
fn a_verified_serial_routes_to_serial_cht_and_the_game_settings_ini() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("Infinite Health", "30123456 00000063")))
        .unwrap();
    assert_eq!(plan.preview.serial, SERIAL);
    assert_eq!(plan.preview.cht_path, world.cht());
    assert_eq!(plan.preview.game_ini_path, world.ini());
    assert_eq!(plan.preview.cheats_added, ["Infinite Health"]);
    assert!(!plan.preview.no_op && plan.preview.undo_available);
    // Previewing writes nothing.
    assert!(!world.cheats().exists() && !world.game_settings().exists());
}

#[test]
fn lowercase_serials_are_normalised_to_the_canonical_file_name() {
    let world = World::new();
    let mut request = world.request(add("A", "30123456 00000001"));
    request.verified_serials = vec!["slus-00067".into()];
    assert_eq!(
        plan_duckstation_native(&request).unwrap().preview.serial,
        SERIAL
    );
}

#[test]
fn missing_or_unverified_serials_are_refused() {
    let world = World::new();
    for serials in [
        vec![],
        vec![String::new()],
        vec!["title only".to_string()],
        vec!["SLUS-0006".to_string()],
        vec!["../SLUS-00067".to_string()],
        vec!["SLUS-00067*".to_string()],
    ] {
        let mut request = world.request(add("A", "30123456 00000001"));
        request.verified_serials = serials.clone();
        assert_eq!(
            plan_duckstation_native(&request).unwrap_err(),
            DuckStationNativeRefusal::MissingVerifiedSerial,
            "{serials:?}"
        );
    }
}

#[test]
fn an_ambiguous_serial_is_refused_not_guessed() {
    let world = World::new();
    let mut request = world.request(add("A", "30123456 00000001"));
    request.verified_serials = vec!["SLUS-00067".into(), "SCUS-94900".into()];
    assert_eq!(
        plan_duckstation_native(&request).unwrap_err(),
        DuckStationNativeRefusal::AmbiguousSerial {
            serials: vec!["SCUS-94900".into(), "SLUS-00067".into()]
        }
    );
    // The same serial offered twice is not ambiguous.
    request.verified_serials = vec!["SLUS-00067".into(), "slus-00067".into()];
    assert!(plan_duckstation_native(&request).is_ok());
}

#[test]
fn multi_disc_and_unproven_disc_topology_are_refused() {
    let world = World::new();
    let mut request = world.request(add("A", "30123456 00000001"));
    request.disc_topology = DuckStationDiscTopology::MultiDisc;
    assert_eq!(
        plan_duckstation_native(&request).unwrap_err(),
        DuckStationNativeRefusal::UnsupportedMultiDisc
    );
    request.disc_topology = DuckStationDiscTopology::Unknown;
    assert_eq!(
        plan_duckstation_native(&request).unwrap_err(),
        DuckStationNativeRefusal::DiscTopologyUnproven
    );
}

#[test]
fn hash_specific_and_other_serial_prefixed_cht_files_are_refused() {
    for other in [
        "SLUS-00067_0123456789ABCDEF.cht",
        "slus-00067_0123456789abcdef.cht",
        "SLUS-00067-extra.cht",
    ] {
        let world = World::new();
        World::write(&world.cheats().join(other), "[X]\n30123456 00000001\n");
        let error =
            plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap_err();
        assert_eq!(
            error,
            DuckStationNativeRefusal::HashSpecificVariantPresent {
                files: vec![other.to_string()]
            }
        );
    }
    // A different game's file is not a variant.
    let world = World::new();
    World::write(
        &world.cheats().join("SCUS-94900.cht"),
        "[X]\n30123456 00000001\n",
    );
    assert!(plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).is_ok());
}

// ---- cheat file -----------------------------------------------------------

#[test]
fn adding_a_cheat_keeps_every_existing_byte_comments_unknown_fields_and_other_cheats() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let plan =
        plan_duckstation_native(&world.request(add("New One", "90123456 DEADBEEF"))).unwrap();
    apply(&world, &plan, "op-keep");
    let after = text(&world.cht());
    assert!(
        after.starts_with(EXISTING_CHT),
        "existing bytes must be an untouched prefix:\n{after}"
    );
    assert!(after.contains("[New One]\n; added by EmuWiz\nType = Gameshark\nActivation = EndFrame\nDescription = test\n90123456 DEADBEEF\n"));
    // The unsupported A0 line and odd-spacing key survive verbatim.
    assert!(after.contains("A0123456 00000001\n") && after.contains("CustomKey=odd spacing\n"));
    assert!(after.contains("; my own header"));
}

#[test]
fn crlf_files_and_a_missing_final_newline_are_preserved() {
    let world = World::new();
    let crlf = "[Keep]\r\nType = Gameshark\r\n30123456 00000001\r\n";
    World::write(&world.cht(), crlf);
    let plan = plan_duckstation_native(&world.request(add("More", "30123457 00000002"))).unwrap();
    apply(&world, &plan, "op-crlf");
    let after = text(&world.cht());
    assert!(after.starts_with(crlf));
    assert!(after.contains("[More]\r\n") && !after.replace("\r\n", "").contains('\n'));

    let world = World::new();
    World::write(&world.cht(), "[Keep]\n30123456 00000001");
    let plan = plan_duckstation_native(&world.request(add("More", "30123457 00000002"))).unwrap();
    apply(&world, &plan, "op-nonl");
    assert!(text(&world.cht()).starts_with("[Keep]\n30123456 00000001\n\n[More]\n"));
}

#[test]
fn a_same_name_cheat_with_different_content_is_a_typed_conflict() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let error =
        plan_duckstation_native(&world.request(add("Keep Me", "30123456 00000001"))).unwrap_err();
    assert_eq!(
        error,
        DuckStationNativeRefusal::CheatNameConflict {
            name: "Keep Me".into()
        }
    );
    // Nothing was written.
    assert_eq!(text(&world.cht()), EXISTING_CHT);
}

#[test]
fn re_adding_an_identical_cheat_is_a_no_op() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("Same", "30123456 00000063"))).unwrap();
    apply(&world, &plan, "op-first");
    let again = plan_duckstation_native(&world.request(add("Same", "30123456 00000063"))).unwrap();
    assert!(again.preview.no_op && !again.preview.undo_available);
    let receipt = apply(&world, &again, "op-second");
    assert!(receipt.no_op && receipt.cht.is_none() && receipt.game_ini.is_none());
}

#[test]
fn update_replaces_only_the_proven_section_and_leaves_the_rest() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let digest = duckstation_native_section_digest(EXISTING_CHT, "Also Mine").unwrap();
    let updated = cheat("Also Mine", "30123457 00000009");
    let plan = plan_duckstation_native(&world.request(DuckStationNativeOperation::Update {
        cheat: updated,
        expected_existing_digest: digest,
    }))
    .unwrap();
    assert_eq!(plan.preview.cheats_changed, ["Also Mine"]);
    apply(&world, &plan, "op-update");
    let after = text(&world.cht());
    assert!(after.contains("30123457 00000009") && !after.contains("30123457 00000002"));
    assert!(after.starts_with(&EXISTING_CHT[..EXISTING_CHT.find("[Also Mine]").unwrap()]));
}

#[test]
fn update_with_a_stale_digest_or_a_missing_cheat_is_refused() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let stale = plan_duckstation_native(&world.request(DuckStationNativeOperation::Update {
        cheat: cheat("Also Mine", "30123457 00000009"),
        expected_existing_digest: "0".repeat(64),
    }))
    .unwrap_err();
    assert!(matches!(
        stale,
        DuckStationNativeRefusal::ExistingFileChanged { .. }
    ));
    let missing = plan_duckstation_native(&world.request(DuckStationNativeOperation::Update {
        cheat: cheat("Nope", "30123457 00000009"),
        expected_existing_digest: "0".repeat(64),
    }))
    .unwrap_err();
    assert_eq!(
        missing,
        DuckStationNativeRefusal::CheatNotFound {
            name: "Nope".into()
        }
    );
}

#[test]
fn remove_deletes_only_that_section_and_its_enable_entry() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    World::write(
        &world.ini(),
        "[Cheats]\nEnableCheats = true\nEnable = Keep Me\nEnable = Also Mine\n\n[Display]\nRenderer = Vulkan\n",
    );
    let plan = plan_duckstation_native(&world.request(DuckStationNativeOperation::Remove {
        name: "Also Mine".into(),
    }))
    .unwrap();
    assert_eq!(plan.preview.cheats_removed, ["Also Mine"]);
    assert_eq!(plan.preview.enable_entries_removed, ["Also Mine"]);
    apply(&world, &plan, "op-remove");
    let cht = text(&world.cht());
    assert!(!cht.contains("Also Mine") && cht.contains("[Keep Me]") && cht.contains("A0123456"));
    let ini = text(&world.ini());
    assert!(ini.contains("Enable = Keep Me") && !ini.contains("Also Mine"));
    assert!(ini.contains("EnableCheats = true") && ini.contains("Renderer = Vulkan"));
}

#[test]
fn set_enabled_changes_only_the_ini_and_requires_the_cheat_to_exist() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let before = sha(&world.cht());
    let plan = plan_duckstation_native(&world.request(DuckStationNativeOperation::SetEnabled {
        name: "Keep Me".into(),
        enabled: true,
    }))
    .unwrap();
    assert!(!plan.preview.cht_will_change && plan.preview.game_ini_will_change);
    apply(&world, &plan, "op-enable");
    assert_eq!(sha(&world.cht()), before);
    assert!(text(&world.ini()).contains("Enable = Keep Me"));
    let error = plan_duckstation_native(&world.request(DuckStationNativeOperation::SetEnabled {
        name: "Ghost".into(),
        enabled: true,
    }))
    .unwrap_err();
    assert_eq!(
        error,
        DuckStationNativeRefusal::CheatNotFound {
            name: "Ghost".into()
        }
    );
}

#[test]
fn unsupported_or_malformed_incoming_cheats_are_refused_before_anything_is_written() {
    let world = World::new();
    // 0xA0 is a compare, not a write; 0x31 is a bit-set; both stay unsupported.
    for code in [
        "A0123456 00000001",
        "31123456 00000001",
        "not a code",
        "30123456",
    ] {
        let error = plan_duckstation_native(&world.request(add("Bad", code))).unwrap_err();
        assert!(
            matches!(error, DuckStationNativeRefusal::UnsupportedCheatCode { .. }),
            "{code}: {error:?}"
        );
    }
    for name in ["", " lead", "has [bracket]", "new\nline"] {
        let error =
            plan_duckstation_native(&world.request(add(name, "30123456 00000001"))).unwrap_err();
        assert!(
            matches!(error, DuckStationNativeRefusal::InvalidCheat { .. }),
            "{name:?}"
        );
    }
    assert!(!world.cheats().exists());
}

#[test]
fn the_three_direct_write_opcodes_are_accepted_and_uppercased() {
    let world = World::new();
    let mut multi = cheat("Writes", "30123456 63");
    multi.code_lines = vec![
        "30123456 63".into(),
        "80123458 03e7".into(),
        "9012345c deadbeef".into(),
    ];
    let plan = plan_duckstation_native(&world.request(DuckStationNativeOperation::Add {
        cheat: multi,
        enable: false,
    }))
    .unwrap();
    apply(&world, &plan, "op-writes");
    let after = text(&world.cht());
    assert!(after.contains("30123456 63\n80123458 03E7\n9012345C DEADBEEF\n"));
}

// ---- game INI -------------------------------------------------------------

#[test]
fn a_fresh_game_ini_gets_cheats_enablecheats_and_the_enable_entry() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("Infinite Health", "30123456 00000063")))
        .unwrap();
    assert_eq!(plan.preview.enable_cheats, EnableCheatsChange::Created);
    assert!(!plan.preview.game_ini_exists);
    apply(&world, &plan, "op-fresh");
    assert_eq!(
        text(&world.ini()),
        "[Cheats]\nEnableCheats = true\nEnable = Infinite Health\n"
    );
}

#[test]
fn an_ini_without_a_cheats_section_gets_one_and_unrelated_content_is_preserved() {
    let world = World::new();
    World::write(&world.ini(), EXISTING_INI);
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    apply(&world, &plan, "op-append");
    assert_eq!(
        text(&world.ini()),
        format!("{EXISTING_INI}\n[Cheats]\nEnableCheats = true\nEnable = A\n")
    );
}

#[test]
fn an_existing_cheats_section_is_updated_in_place() {
    let world = World::new();
    World::write(
        &world.ini(),
        "[Main]\nSetting = 1\n\n[Cheats]\n; keep this comment\nEnable = Other\nLoadCheatsFromDatabase = false\n\n[Display]\nRenderer = Vulkan\n",
    );
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    assert_eq!(plan.preview.enable_cheats, EnableCheatsChange::Created);
    apply(&world, &plan, "op-inplace");
    assert_eq!(
        text(&world.ini()),
        "[Main]\nSetting = 1\n\n[Cheats]\nEnableCheats = true\n; keep this comment\nEnable = Other\nEnable = A\nLoadCheatsFromDatabase = false\n\n[Display]\nRenderer = Vulkan\n"
    );
}

#[test]
fn enablecheats_false_is_changed_to_true_and_reported() {
    let world = World::new();
    World::write(
        &world.ini(),
        "[Cheats]\nEnable = Other\nEnableCheats = false\n",
    );
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    assert_eq!(
        plan.preview.enable_cheats,
        EnableCheatsChange::ChangedToTrue {
            from: "false".into()
        }
    );
    apply(&world, &plan, "op-false");
    assert_eq!(
        text(&world.ini()),
        "[Cheats]\nEnable = Other\nEnable = A\nEnableCheats = true\n"
    );
}

#[test]
fn an_already_enabled_cheat_leaves_the_ini_untouched() {
    let world = World::new();
    World::write(&world.ini(), "[Cheats]\nEnableCheats = true\nEnable = A\n");
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    assert!(!plan.preview.game_ini_will_change);
    assert_eq!(plan.preview.enable_cheats, EnableCheatsChange::AlreadyTrue);
    apply(&world, &plan, "op-already");
    assert_eq!(
        text(&world.ini()),
        "[Cheats]\nEnableCheats = true\nEnable = A\n"
    );
}

#[test]
fn an_ambiguous_ini_is_a_typed_conflict() {
    for ini in [
        "[Cheats]\nEnable = A\n[Cheats]\nEnableCheats = true\n",
        "[Cheats]\nEnableCheats = true\nEnableCheats = false\n",
    ] {
        let world = World::new();
        World::write(&world.ini(), ini);
        let error =
            plan_duckstation_native(&world.request(add("B", "30123456 00000001"))).unwrap_err();
        assert!(
            matches!(error, DuckStationNativeRefusal::IniUpdateConflict { .. }),
            "{ini}"
        );
    }
}

#[test]
fn enable_keys_that_merely_start_with_enable_are_not_confused() {
    let world = World::new();
    World::write(
        &world.ini(),
        "[Cheats]\nEnableCheats = true\nEnableCheatsExtra = x\nEnable = A\n",
    );
    let plan = plan_duckstation_native(&world.request(DuckStationNativeOperation::SetEnabled {
        name: "A".into(),
        enabled: false,
    }));
    // The cheat file is absent, but disabling never needs it to exist.
    let plan = plan.unwrap();
    apply(&world, &plan, "op-prefix");
    assert_eq!(
        text(&world.ini()),
        "[Cheats]\nEnableCheats = true\nEnableCheatsExtra = x\n"
    );
}

// ---- database shadowing ---------------------------------------------------------

#[test]
fn unseen_database_cheats_must_be_disproved_or_acknowledged() {
    let world = World::new();
    let mut request = world.request(add("A", "30123456 00000001"));
    request.acknowledge_database_shadowing = false;
    assert_eq!(
        plan_duckstation_native(&request).unwrap_err(),
        DuckStationNativeRefusal::DatabaseShadowingUnknown
    );
    // Acknowledging records a warning.
    request.acknowledge_database_shadowing = true;
    assert!(
        plan_duckstation_native(&request)
            .unwrap()
            .preview
            .warnings
            .contains(&DuckStationNativeWarning::DatabaseShadowingAcknowledged)
    );
    // Explicitly turning the database off for this game proves it safe.
    World::write(&world.ini(), "[Cheats]\nLoadCheatsFromDatabase = false\n");
    request.acknowledge_database_shadowing = false;
    let plan = plan_duckstation_native(&request).unwrap();
    assert!(
        !plan
            .preview
            .warnings
            .contains(&DuckStationNativeWarning::DatabaseShadowingAcknowledged)
    );
}

#[test]
fn removing_or_toggling_a_cheat_does_not_need_the_database_question_answered() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let mut request = world.request(DuckStationNativeOperation::Remove {
        name: "Also Mine".into(),
    });
    request.acknowledge_database_shadowing = false;
    assert!(plan_duckstation_native(&request).is_ok());
}

// ---- folders --------------------------------------------------------------------

#[test]
fn custom_relative_and_absolute_folders_are_resolved_and_used() {
    let world = World::new();
    let elsewhere = world.dir.path().join("elsewhere").join("gs");
    World::write(
        &world.profile.join("settings.ini"),
        &format!(
            "[Folders]\nCheats = my cheats\nGameSettings = {}\n",
            elsewhere.display()
        ),
    );
    let folders = resolve_duckstation_folders(&world.profile).unwrap();
    assert_eq!(folders.cheats, world.profile.join("my cheats"));
    assert_eq!(folders.game_settings, elsewhere);
    assert!(folders.cheats_custom && folders.game_settings_custom);

    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    assert_eq!(
        plan.preview.cht_path,
        world.profile.join("my cheats").join("SLUS-00067.cht")
    );
    assert_eq!(plan.preview.game_ini_path, elsewhere.join("SLUS-00067.ini"));
    let receipt = apply(&world, &plan, "op-custom");
    assert!(world.profile.join("my cheats/SLUS-00067.cht").is_file());
    assert!(elsewhere.join("SLUS-00067.ini").is_file());
    // The default folders were not used.
    assert!(!world.cheats().exists() && !world.game_settings().exists());
    // Each file keeps its own transaction root (its folder's parent) in the receipt.
    let cht = receipt.cht.unwrap();
    assert_eq!(cht.destination_root, world.profile);
    assert_eq!(cht.relative_path, "my cheats/SLUS-00067.cht");
    let ini = receipt.game_ini.unwrap();
    assert_eq!(ini.destination_root, elsewhere.parent().unwrap());
    assert_eq!(ini.relative_path, "gs/SLUS-00067.ini");
}

#[test]
fn defaults_apply_without_a_settings_ini_or_with_empty_values() {
    let world = World::new();
    let folders = resolve_duckstation_folders(&world.profile).unwrap();
    assert_eq!(folders.cheats, world.cheats());
    assert!(!folders.cheats_custom);
    World::write(
        &world.profile.join("settings.ini"),
        "[Folders]\nCheats =\nOther = x\n",
    );
    let folders = resolve_duckstation_folders(&world.profile).unwrap();
    assert_eq!(folders.cheats, world.cheats());
    assert_eq!(folders.game_settings, world.game_settings());
}

#[test]
fn unsafe_custom_folders_are_refused() {
    let world = World::new();
    let cases = [
        "Cheats = ../outside",
        "Cheats = /",
        "Cheats = /home",
        "Cheats = a/../../b",
        "GameSettings = /tmp/../etc",
    ];
    for case in cases {
        World::write(
            &world.profile.join("settings.ini"),
            &format!("[Folders]\n{case}\n"),
        );
        let error = resolve_duckstation_folders(&world.profile).unwrap_err();
        assert!(
            matches!(error, DuckStationNativeRefusal::UnsafeCustomFolder { .. }),
            "{case}: {error:?}"
        );
    }
}

#[test]
fn a_symlinked_or_non_directory_folder_is_refused() {
    let world = World::new();
    let real = world.dir.path().join("real");
    fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, world.profile.join("cheats")).unwrap();
    assert!(matches!(
        resolve_duckstation_folders(&world.profile).unwrap_err(),
        DuckStationNativeRefusal::UnsafeCustomFolder { .. }
    ));
    fs::remove_file(world.profile.join("cheats")).unwrap();
    fs::write(world.profile.join("cheats"), b"a file").unwrap();
    assert!(matches!(
        resolve_duckstation_folders(&world.profile).unwrap_err(),
        DuckStationNativeRefusal::UnsafeCustomFolder { .. }
    ));
}

#[test]
fn an_unusable_profile_root_is_refused() {
    let world = World::new();
    let missing = world.dir.path().join("nope");
    assert!(matches!(
        resolve_duckstation_folders(&missing).unwrap_err(),
        DuckStationNativeRefusal::InvalidDuckStationProfile { .. }
    ));
    assert!(matches!(
        resolve_duckstation_folders(Path::new("relative/path")).unwrap_err(),
        DuckStationNativeRefusal::InvalidDuckStationProfile { .. }
    ));
    World::write(
        &world.profile.join("settings.ini"),
        "[Folders]\nCheats = a\nCheats = b\n",
    );
    assert!(matches!(
        resolve_duckstation_folders(&world.profile).unwrap_err(),
        DuckStationNativeRefusal::InvalidDuckStationProfile { .. }
    ));
}

#[test]
fn an_unreadable_existing_cht_is_refused() {
    let world = World::new();
    fs::create_dir_all(world.cheats()).unwrap();
    fs::write(world.cht(), [0xff, 0xfe, 0x00]).unwrap();
    assert!(matches!(
        plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap_err(),
        DuckStationNativeRefusal::ExistingFileChanged { .. }
    ));
    fs::remove_file(world.cht()).unwrap();
    fs::create_dir(world.cht()).unwrap();
    assert!(matches!(
        plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap_err(),
        DuckStationNativeRefusal::ExistingFileChanged { .. }
    ));
}

// ---- transaction ----------------------------------------------------------------

#[test]
fn a_two_file_apply_publishes_both_verifies_and_writes_a_receipt() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    World::write(&world.ini(), EXISTING_INI);
    let (cht_before, ini_before) = (sha(&world.cht()), sha(&world.ini()));
    let plan = plan_duckstation_native(&world.request(add("Infinite Health", "30123456 00000063")))
        .unwrap();
    assert!(plan.preview.backup_will_be_made && plan.preview.cht_exists);
    let receipt = apply(&world, &plan, "op-two");
    let (cht, ini) = (
        receipt.cht.as_ref().unwrap(),
        receipt.game_ini.as_ref().unwrap(),
    );
    // Receipt fingerprints match the real before/after bytes.
    assert_eq!(cht.before_sha256, cht_before);
    assert_eq!(ini.before_sha256, ini_before);
    assert_eq!(Some(cht.after_sha256.clone()), sha(&world.cht()));
    assert_eq!(Some(ini.after_sha256.clone()), sha(&world.ini()));
    assert!(text(&world.cht()).contains("[Infinite Health]"));
    assert!(text(&world.ini()).contains("Enable = Infinite Health"));
    assert!(text(&world.ini()).contains("Renderer = Vulkan"));
    // The receipt is durable and readable.
    let loaded = read_duckstation_native_receipt(receipt.receipt_path.as_ref().unwrap()).unwrap();
    assert_eq!(loaded.cht, receipt.cht);
    assert_eq!(loaded.cheats_added, ["Infinite Health"]);
}

#[test]
fn nothing_is_written_without_confirmation_or_with_an_unsafe_operation_id() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let mut options = world.options("op-no");
    options.general_approved = false;
    assert_eq!(
        apply_duckstation_native_plan(&plan, &options).unwrap_err(),
        DuckStationNativeApplyFailure::Refused(DuckStationNativeRefusal::ConfirmationRequired)
    );
    for id in ["", "../x", "a b", &"x".repeat(80)] {
        let mut options = world.options("op-ok");
        options.operation_id = id.to_string();
        assert_eq!(
            apply_duckstation_native_plan(&plan, &options).unwrap_err(),
            DuckStationNativeApplyFailure::Refused(DuckStationNativeRefusal::InvalidOperationId),
            "{id:?}"
        );
    }
    assert!(!world.cheats().exists() && !world.game_settings().exists());
}

#[test]
fn a_destination_changed_after_preview_publishes_neither_file() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    World::write(&world.ini(), EXISTING_INI);
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    // Someone edits the INI between preview and apply.
    World::write(&world.ini(), "[Main]\nSetting = 2\n");
    let (cht_before, ini_before) = (sha(&world.cht()), sha(&world.ini()));
    let error = apply_duckstation_native_plan(&plan, &world.options("op-drift")).unwrap_err();
    assert_eq!(
        error,
        DuckStationNativeApplyFailure::Refused(
            DuckStationNativeRefusal::DestinationChangedAfterPreview { path: world.ini() }
        )
    );
    assert_eq!(
        (sha(&world.cht()), sha(&world.ini())),
        (cht_before, ini_before)
    );
}

#[test]
fn a_new_file_appearing_after_preview_is_also_a_change() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    World::write(&world.cht(), "[Surprise]\n30123456 00000001\n");
    assert!(matches!(
        apply_duckstation_native_plan(&plan, &world.options("op-new")).unwrap_err(),
        DuckStationNativeApplyFailure::Refused(
            DuckStationNativeRefusal::DestinationChangedAfterPreview { .. }
        )
    ));
    assert_eq!(text(&world.cht()), "[Surprise]\n30123456 00000001\n");
    assert!(!world.ini().exists());
}

// ---- deterministic fault injection ------------------------------------------------
//
// Failures come from the shared transaction layer's own test-only fault points
// and from `#[cfg(test)]` hooks in the apply path, never from file permissions,
// so they behave the same for every user including root.

use super::super::shared_transaction::{FaultPoint, inject_fault};

/// Clears every injected fault when a test ends, pass or fail.
struct Faults;

impl Faults {
    fn shared(point: FaultPoint) -> Self {
        inject_fault(Some(point));
        Self
    }

    fn hook(fault: TestFault) -> Self {
        set_test_fault(Some(fault));
        Self
    }
}

impl Drop for Faults {
    fn drop(&mut self) {
        inject_fault(None);
        set_test_fault(None);
    }
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn receipt_files(world: &World) -> Vec<String> {
    names(&world.dir.path().join("history"))
        .into_iter()
        .filter(|name| name.ends_with(".receipt.json"))
        .collect()
}

fn pending_operations(world: &World) -> usize {
    crate::patch_manager::discover_pending_operations(&world.dir.path().join("history")).len()
}

/// A world holding both files in a known original state.
fn world_with_originals() -> World {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    World::write(&world.ini(), EXISTING_INI);
    world
}

#[test]
fn a_failure_during_the_first_publication_changes_nothing_and_leaves_no_debris() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let (cht_names, ini_names) = (names(&world.cheats()), names(&world.game_settings()));
    let (cht_before, ini_before) = (sha(&world.cht()), sha(&world.ini()));
    let result = {
        let _faults = Faults::shared(FaultPoint::Rename);
        apply_duckstation_native_plan(&plan, &world.options("op-first"))
    };
    match result {
        Err(DuckStationNativeApplyFailure::PublishFailedRolledBack { stage, .. }) => {
            assert_eq!(stage, "publish cheat file");
        }
        other => panic!("expected a clean rolled-back failure, got {other:?}"),
    }
    assert_eq!(
        (sha(&world.cht()), sha(&world.ini())),
        (cht_before, ini_before)
    );
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), EXISTING_INI);
    // No temporary files or journals left behind, and no success receipt.
    assert_eq!(names(&world.cheats()), cht_names);
    assert_eq!(names(&world.game_settings()), ini_names);
    assert!(receipt_files(&world).is_empty());
    assert_eq!(pending_operations(&world), 0);
}

#[test]
fn a_first_publication_failure_on_fresh_files_creates_nothing() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let result = {
        let _faults = Faults::shared(FaultPoint::Rename);
        apply_duckstation_native_plan(&plan, &world.options("op-fresh-first"))
    };
    assert!(
        matches!(
            result,
            Err(DuckStationNativeApplyFailure::PublishFailedRolledBack { .. })
        ),
        "{result:?}"
    );
    assert!(!world.cht().exists() && !world.ini().exists());
    assert!(receipt_files(&world).is_empty());
    assert_eq!(pending_operations(&world), 0);
}

#[test]
fn an_unsafe_second_destination_is_refused_before_the_first_file_is_published() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    // After the preview, the settings folder is swapped for a symlink.
    let real = world.dir.path().join("elsewhere");
    fs::rename(world.game_settings(), &real).unwrap();
    std::os::unix::fs::symlink(&real, world.game_settings()).unwrap();
    let error = apply_duckstation_native_plan(&plan, &world.options("op-unsafe")).unwrap_err();
    assert!(
        matches!(
            error,
            DuckStationNativeApplyFailure::Refused(
                DuckStationNativeRefusal::ExistingFileChanged { .. }
            )
        ),
        "{error:?}"
    );
    // Both shared plans are built and checked before anything is published.
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&real.join("SLUS-00067.ini")), EXISTING_INI);
    assert!(receipt_files(&world).is_empty());
    assert_eq!(pending_operations(&world), 0);
}

#[test]
fn a_failure_after_the_cht_but_before_the_ini_restores_the_cht_exactly() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let cht_names = names(&world.cheats());
    // Someone edits the INI after the .cht is published and before the INI is.
    let concurrent = "[Main]\nSetting = 2\n";
    let result = {
        let _faults = Faults::hook(TestFault::MutateAfterFirstPublish {
            path: world.ini(),
            content: concurrent.to_string(),
        });
        apply_duckstation_native_plan(&plan, &world.options("op-second"))
    };
    match result {
        Err(DuckStationNativeApplyFailure::PublishFailedRolledBack { stage, .. }) => {
            assert_eq!(stage, "publish settings file");
        }
        other => panic!("expected a clean rolled-back failure, got {other:?}"),
    }
    // The .cht is byte-identical to the original; the INI is exactly what the
    // concurrent editor left (we never wrote it) and enables nothing of ours.
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), concurrent);
    assert_eq!(names(&world.cheats()), cht_names);
    assert!(receipt_files(&world).is_empty());
    assert_eq!(pending_operations(&world), 0);
}

#[test]
fn a_failure_after_a_fresh_cht_publication_removes_it_again() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let result = {
        let _faults = Faults::hook(TestFault::MutateAfterFirstPublish {
            path: world.ini(),
            content: "[Main]\nSetting = 2\n".to_string(),
        });
        // The INI does not exist yet, so the hook creates it, which the shared
        // executor must treat as a destination that changed since the preview.
        fs::create_dir_all(world.game_settings()).unwrap();
        apply_duckstation_native_plan(&plan, &world.options("op-fresh-second"))
    };
    assert!(
        matches!(
            result,
            Err(DuckStationNativeApplyFailure::PublishFailedRolledBack { .. })
        ),
        "{result:?}"
    );
    assert!(
        !world.cht().exists(),
        "the published .cht must be removed again"
    );
    assert!(receipt_files(&world).is_empty());
    assert_eq!(pending_operations(&world), 0);
}

#[test]
fn a_verification_failure_after_both_publications_rolls_both_files_back() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let result = {
        let _faults = Faults::hook(TestFault::ForceVerifyFailure);
        apply_duckstation_native_plan(&plan, &world.options("op-verify"))
    };
    match result {
        Err(DuckStationNativeApplyFailure::PublishFailedRolledBack { stage, detail }) => {
            assert_eq!(stage, "verify published files");
            assert!(detail.contains("injected verification failure"), "{detail}");
        }
        other => panic!("expected a clean rolled-back failure, got {other:?}"),
    }
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), EXISTING_INI);
    assert!(receipt_files(&world).is_empty());
    assert_eq!(pending_operations(&world), 0);
}

#[test]
fn a_file_changed_after_publication_is_reported_not_overwritten_and_never_a_success() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let result = {
        let _faults = Faults::hook(TestFault::TamperAfterPublish {
            path: world.ini(),
            content: "[Cheats]\nEnable = Tampered\n".to_string(),
        });
        apply_duckstation_native_plan(&plan, &world.options("op-tamper"))
    };
    match result {
        Err(DuckStationNativeApplyFailure::RollbackIncomplete { stage, detail }) => {
            assert_eq!(stage, "verify published files");
            assert!(detail.contains("rollback"), "{detail}");
        }
        other => panic!("expected an explicit incomplete rollback, got {other:?}"),
    }
    // The other person's change is never clobbered; the .cht we published is
    // still rolled back, so the INI cannot enable a cheat that is now missing.
    assert_eq!(text(&world.ini()), "[Cheats]\nEnable = Tampered\n");
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert!(receipt_files(&world).is_empty());
}

#[test]
fn a_receipt_failure_rolls_both_files_back_and_is_never_a_success() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let result = {
        let _faults = Faults::hook(TestFault::FailReceiptWrite);
        apply_duckstation_native_plan(&plan, &world.options("op-receipt"))
    };
    match result {
        Err(DuckStationNativeApplyFailure::PublishFailedRolledBack { stage, .. }) => {
            assert_eq!(stage, "write receipt");
        }
        other => panic!("expected a clean rolled-back failure, got {other:?}"),
    }
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), EXISTING_INI);
    assert!(receipt_files(&world).is_empty());
    assert_eq!(pending_operations(&world), 0);
}

#[test]
fn the_receipt_exists_only_when_both_files_were_published_and_verified() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let receipt = apply(&world, &plan, "op-both");
    assert_eq!(
        receipt_files(&world),
        ["duckstation-native-op-both.receipt.json"]
    );
    assert!(receipt.cht.is_some() && receipt.game_ini.is_some());
    assert_eq!(pending_operations(&world), 0);
}

#[test]
fn a_destination_changed_after_preview_is_refused_for_either_file() {
    // The .cht changes.
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    World::write(&world.cht(), "[Other]\n30123456 00000009\n");
    let error = apply_duckstation_native_plan(&plan, &world.options("op-cht-drift")).unwrap_err();
    assert_eq!(
        error,
        DuckStationNativeApplyFailure::Refused(
            DuckStationNativeRefusal::DestinationChangedAfterPreview { path: world.cht() }
        )
    );
    assert_eq!(text(&world.ini()), EXISTING_INI);
    assert!(receipt_files(&world).is_empty());
}

#[test]
fn undo_is_refused_when_either_published_file_changed_since_apply() {
    for changed in ["cht", "ini"] {
        let world = world_with_originals();
        let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
        let receipt = apply(&world, &plan, "op-undo-either");
        let target = if changed == "cht" {
            world.cht()
        } else {
            world.ini()
        };
        let edited = format!("{}\n; user edit\n", text(&target));
        World::write(&target, &edited);
        let options = world.options("undo-either");
        assert!(
            !preview_duckstation_native_undo(&receipt, &options.backup_root).available,
            "{changed}"
        );
        assert!(
            undo_duckstation_native(&receipt, &options).is_err(),
            "{changed}"
        );
        // Neither file is touched by the refused undo.
        assert_eq!(text(&target), edited, "{changed}");
    }
}

#[test]
fn an_acknowledged_database_risk_is_recorded_and_the_ini_setting_is_never_changed_silently() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    assert!(
        plan.preview
            .warnings
            .contains(&DuckStationNativeWarning::DatabaseShadowingAcknowledged)
    );
    let receipt = apply(&world, &plan, "op-ack");
    assert!(
        receipt
            .warnings
            .contains(&DuckStationNativeWarning::DatabaseShadowingAcknowledged)
    );
    let loaded = read_duckstation_native_receipt(receipt.receipt_path.as_ref().unwrap()).unwrap();
    assert_eq!(loaded.warnings, receipt.warnings);
    // EmuWiz wrote enablement only; the database setting is untouched.
    assert!(!text(&world.ini()).contains("LoadCheatsFromDatabase"));

    // Without acknowledgement the default stays closed.
    let mut request = world.request(add("B", "30123457 00000002"));
    request.acknowledge_database_shadowing = false;
    assert_eq!(
        plan_duckstation_native(&request).unwrap_err(),
        DuckStationNativeRefusal::DatabaseShadowingUnknown
    );
}

#[test]
fn an_existing_a0_line_is_preserved_and_never_emitted_as_a_write() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let plan = plan_duckstation_native(&world.request(add("New", "90123456 DEADBEEF"))).unwrap();
    apply(&world, &plan, "op-a0");
    let after = text(&world.cht());
    assert!(
        after.contains("A0123456 00000001\n"),
        "existing unsupported line kept"
    );
    assert!(after.contains("90123456 DEADBEEF\n"));
    // A newly requested A0 line is refused rather than written as a write.
    let error =
        plan_duckstation_native(&world.request(add("Bad", "A0123456 DEADBEEF"))).unwrap_err();
    assert!(matches!(
        error,
        DuckStationNativeRefusal::UnsupportedCheatCode { .. }
    ));
}

#[test]
fn published_state_verification_detects_a_lost_or_altered_section() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let plan = plan_duckstation_native(&world.request(add("New", "30123456 00000001"))).unwrap();
    let good = String::from_utf8(plan.cht_final.clone()).unwrap();
    let ini = String::from_utf8(plan.ini_final.clone()).unwrap();
    let check = |cht: &str, ini: &str| {
        verify_state(
            &plan.expectations,
            Some(cht),
            Some(ini),
            &world.cht(),
            &world.ini(),
        )
    };
    assert!(check(&good, &ini).is_ok());
    // The new cheat is missing.
    assert!(check(&good.replace("[New]", "[Renamed]"), &ini).is_err());
    // An unrelated cheat was altered.
    assert!(
        check(
            &good.replace("30123457 00000002", "30123457 00000003"),
            &ini
        )
        .is_err()
    );
    // An unrelated cheat was lost.
    assert!(check(&good.replace("[Also Mine]", "[Gone]"), &ini).is_err());
    // The header comment changed.
    assert!(check(&good.replace("; my own header", "; changed"), &ini).is_err());
    // The INI lost enablement or the entry.
    assert!(
        check(
            &good,
            &ini.replace("EnableCheats = true", "EnableCheats = false")
        )
        .is_err()
    );
    assert!(check(&good, &ini.replace("Enable = New", "Enable = Other")).is_err());
    assert!(check(&good, &format!("{ini}Enable = New\n")).is_err());
}

#[test]
fn published_state_verification_detects_lost_unrelated_ini_lines() {
    let world = World::new();
    World::write(&world.ini(), EXISTING_INI);
    let plan = plan_duckstation_native(&world.request(add("New", "30123456 00000001"))).unwrap();
    let cht = String::from_utf8(plan.cht_final.clone()).unwrap();
    let ini = String::from_utf8(plan.ini_final.clone()).unwrap();
    let broken = ini.replace("Renderer = Vulkan\n", "");
    assert!(
        verify_state(
            &plan.expectations,
            Some(&cht),
            Some(&broken),
            &world.cht(),
            &world.ini()
        )
        .is_err()
    );
}

// ---- undo -----------------------------------------------------------------------

#[test]
fn undo_restores_both_existing_files_byte_for_byte() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    World::write(&world.ini(), EXISTING_INI);
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let receipt = apply(&world, &plan, "op-undo-existing");
    assert_ne!(text(&world.cht()), EXISTING_CHT);
    let options = world.options("undo-existing");
    let preview = preview_duckstation_native_undo(&receipt, &options.backup_root);
    assert!(preview.available, "{:?}", preview.reasons);
    undo_duckstation_native(&receipt, &options).unwrap();
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), EXISTING_INI);
}

#[test]
fn undo_removes_files_that_did_not_exist_before() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let receipt = apply(&world, &plan, "op-undo-fresh");
    assert!(world.cht().is_file() && world.ini().is_file());
    undo_duckstation_native(&receipt, &world.options("undo-fresh")).unwrap();
    assert!(!world.cht().exists() && !world.ini().exists());
}

#[test]
fn undo_is_refused_when_a_file_changed_since_apply_and_never_twice() {
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let receipt = apply(&world, &plan, "op-undo-drift");
    let edited = format!("{}\n; user edit\n", text(&world.cht()));
    World::write(&world.cht(), &edited);
    let options = world.options("undo-drift");
    let preview = preview_duckstation_native_undo(&receipt, &options.backup_root);
    assert!(!preview.available && !preview.reasons.is_empty());
    assert!(undo_duckstation_native(&receipt, &options).is_err());
    // The user's edit and the INI are exactly as they were.
    assert_eq!(text(&world.cht()), edited);
    assert!(text(&world.ini()).contains("Enable = A"));

    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let receipt = apply(&world, &plan, "op-undo-twice");
    undo_duckstation_native(&receipt, &world.options("undo-once")).unwrap();
    assert!(undo_duckstation_native(&receipt, &world.options("undo-twice")).is_err());
}

#[test]
fn a_no_op_has_nothing_to_undo() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    apply(&world, &plan, "op-a");
    let again = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let receipt = apply(&world, &again, "op-b");
    let preview = preview_duckstation_native_undo(&receipt, &world.options("x").backup_root);
    assert!(!preview.available);
}

// ---- explanations ---------------------------------------------------------------

#[test]
fn every_refusal_has_a_plain_language_explanation() {
    let refusals = [
        DuckStationNativeRefusal::MissingVerifiedSerial,
        DuckStationNativeRefusal::AmbiguousSerial {
            serials: vec!["A".into()],
        },
        DuckStationNativeRefusal::UnsupportedMultiDisc,
        DuckStationNativeRefusal::DiscTopologyUnproven,
        DuckStationNativeRefusal::HashSpecificVariantPresent {
            files: vec!["x.cht".into()],
        },
        DuckStationNativeRefusal::CheatNameConflict { name: "N".into() },
        DuckStationNativeRefusal::CheatNotFound { name: "N".into() },
        DuckStationNativeRefusal::DestinationChangedAfterPreview {
            path: PathBuf::from("/p"),
        },
        DuckStationNativeRefusal::InvalidDuckStationProfile { reason: "r".into() },
        DuckStationNativeRefusal::UnsafeCustomFolder {
            setting: "Cheats".into(),
            reason: "r".into(),
        },
        DuckStationNativeRefusal::DatabaseShadowingUnknown,
        DuckStationNativeRefusal::IniUpdateConflict { reason: "r".into() },
        DuckStationNativeRefusal::ExistingFileChanged {
            path: PathBuf::from("/p"),
            reason: "r".into(),
        },
        DuckStationNativeRefusal::UnsupportedCheatCode {
            name: "N".into(),
            line: "L".into(),
        },
        DuckStationNativeRefusal::InvalidCheat { reason: "r".into() },
        DuckStationNativeRefusal::ConfirmationRequired,
        DuckStationNativeRefusal::InvalidOperationId,
    ];
    for refusal in refusals {
        assert!(refusal.explain().len() > 20, "{refusal:?}");
    }
}

// ---- multi-cheat batches: ONE logical operation ----------------------------

fn batch(items: &[(&str, &str, bool)]) -> DuckStationNativeOperation {
    DuckStationNativeOperation::AddMany {
        cheats: items
            .iter()
            .map(|(name, code, enable)| DuckStationNativeBatchCheat {
                cheat: cheat(name, code),
                enable: *enable,
            })
            .collect(),
    }
}

const THREE: &[(&str, &str, bool)] = &[
    ("Infinite health", "80010000 0001", true),
    ("Infinite ammo", "80020000 0002", true),
    ("Max money", "80030000 0003", true),
];

#[test]
fn several_ticked_cheats_are_one_preview_one_apply_one_receipt() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    // ONE preview: three cheats, one .cht edit, one settings edit.
    assert_eq!(
        plan.preview.cheats_added,
        ["Infinite health", "Infinite ammo", "Max money"]
    );
    assert!(plan.preview.cht_will_change && plan.preview.game_ini_will_change);
    assert_eq!(plan.preview.enable_entries_added.len(), 3);

    let receipt = apply(&world, &plan, "op-batch");
    // ONE logical operation with one receipt covering both files.
    assert_eq!(receipt.cheats_added.len(), 3);
    assert!(receipt.cht.is_some() && receipt.game_ini.is_some());
    assert_eq!(
        receipt_files(&world),
        ["duckstation-native-op-batch.receipt.json"]
    );
    let cht = text(&world.cht());
    let ini = text(&world.ini());
    for (name, _, _) in THREE {
        assert!(cht.contains(&format!("[{name}]")), "{cht}");
        assert!(ini.contains(&format!("Enable = {name}")), "{ini}");
    }
    assert!(ini.contains("EnableCheats = true"));
    // What was there before is untouched.
    assert!(cht.starts_with(EXISTING_CHT.trim_end()));
    assert!(ini.contains("Renderer = Vulkan"));
}

#[test]
fn undo_restores_the_entire_batch_as_one_operation() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let receipt = apply(&world, &plan, "op-batch-undo");
    undo_duckstation_native(&receipt, &world.options("op-batch-undo-rollback")).unwrap();
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), EXISTING_INI);
}

#[test]
fn a_batch_is_atomic_across_both_files() {
    // Verification fails after both files were published: neither survives.
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let result = {
        let _faults = Faults::hook(TestFault::ForceVerifyFailure);
        apply_duckstation_native_plan(&plan, &world.options("op-batch-verify"))
    };
    assert!(matches!(
        result,
        Err(DuckStationNativeApplyFailure::PublishFailedRolledBack { .. })
    ));
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), EXISTING_INI);
    assert!(receipt_files(&world).is_empty());
    // And a receipt failure leaves neither file changed either.
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let result = {
        let _faults = Faults::hook(TestFault::FailReceiptWrite);
        apply_duckstation_native_plan(&plan, &world.options("op-batch-receipt"))
    };
    assert!(result.is_err());
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), EXISTING_INI);
}

#[test]
fn a_batch_on_a_fresh_game_creates_both_files_and_undo_removes_them() {
    let world = World::new();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let receipt = apply(&world, &plan, "op-fresh");
    assert!(world.cht().exists() && world.ini().exists());
    undo_duckstation_native(&receipt, &world.options("op-fresh-undo")).unwrap();
    assert!(!world.cht().exists() && !world.ini().exists());
}

#[test]
fn only_enabled_cheats_get_an_enable_entry() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(&[
        ("On", "80010000 0001", true),
        ("Off", "80020000 0002", false),
    ])))
    .unwrap();
    apply(&world, &plan, "op-mixed");
    let ini = text(&world.ini());
    assert!(ini.contains("Enable = On"));
    assert!(!ini.contains("Enable = Off"));
    assert!(text(&world.cht()).contains("[Off]"));
}

#[test]
fn re_planning_an_installed_batch_is_a_no_op() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    apply(&world, &plan, "op-first");
    let again = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    assert!(again.preview.no_op);
    assert!(again.preview.cheats_added.is_empty());
}

#[test]
fn a_partially_installed_batch_adds_only_what_is_missing() {
    let world = world_with_originals();
    let first = plan_duckstation_native(&world.request(batch(&THREE[..1]))).unwrap();
    apply(&world, &first, "op-one");
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    assert_eq!(plan.preview.cheats_added, ["Infinite ammo", "Max money"]);
}

// ---- conflicts are all reported before anything is written -----------------

fn conflicts_of(
    world: &World,
    operation: DuckStationNativeOperation,
) -> Vec<DuckStationBatchConflict> {
    match plan_duckstation_native(&world.request(operation)) {
        Err(DuckStationNativeRefusal::BatchConflicts { conflicts }) => conflicts,
        other => panic!("expected batch conflicts, got {other:?}"),
    }
}

#[test]
fn duplicate_names_and_incompatible_duplicates_are_reported() {
    let world = world_with_originals();
    let conflicts = conflicts_of(
        &world,
        batch(&[
            ("Same", "80010000 0001", true),
            ("Same", "80010000 0001", true),
            ("Clash", "80020000 0002", true),
            ("Clash", "80020000 0009", true),
        ]),
    );
    assert!(
        conflicts.contains(&DuckStationBatchConflict::DuplicateInSelection {
            name: "Same".into()
        })
    );
    assert!(
        conflicts.contains(&DuckStationBatchConflict::IncompatibleDuplicateCodes {
            name: "Clash".into()
        })
    );
    assert_eq!(text(&world.cht()), EXISTING_CHT, "nothing was written");
}

#[test]
fn an_existing_native_cheat_with_a_different_body_is_never_overwritten() {
    let world = world_with_originals();
    let conflicts = conflicts_of(
        &world,
        batch(&[
            ("Keep Me", "80999999 FFFF", true),
            ("Fresh", "80010000 0001", true),
        ]),
    );
    assert_eq!(
        conflicts,
        [DuckStationBatchConflict::ExistingCheatDiffers {
            name: "Keep Me".into()
        }]
    );
    assert_eq!(text(&world.cht()), EXISTING_CHT);
}

#[test]
fn unsupported_codes_and_invalid_cheats_are_listed_together_with_the_good_ones_untouched() {
    let world = world_with_originals();
    let conflicts = conflicts_of(
        &world,
        batch(&[
            ("Good", "80010000 0001", true),
            ("Conditional", "D0010000 0001", true),
            ("[Bad]", "80020000 0002", true),
        ]),
    );
    assert!(conflicts.iter().any(|c| matches!(
        c,
        DuckStationBatchConflict::UnsupportedCode { name, .. } if name == "Conditional"
    )));
    assert!(conflicts.iter().any(|c| matches!(
        c,
        DuckStationBatchConflict::InvalidCheat { name, .. } if name == "[Bad]"
    )));
    assert_eq!(conflicts.len(), 2, "the valid cheat is not blamed");
}

#[test]
fn a_conflicting_ini_enable_state_is_reported() {
    let world = World::new();
    // Install "Hold" switched on...
    let first =
        plan_duckstation_native(&world.request(batch(&[("Hold", "80010000 0001", true)]))).unwrap();
    apply(&world, &first, "op-hold");
    // ...then ask for the very same cheat to stay off while the game's
    // settings already switch it on.
    let conflicts = conflicts_of(&world, batch(&[("Hold", "80010000 0001", false)]));
    assert_eq!(
        conflicts,
        [DuckStationBatchConflict::IniEnableStateConflict {
            name: "Hold".into()
        }]
    );
}

#[test]
fn database_shadowing_still_needs_an_answer_for_a_batch() {
    let world = world_with_originals();
    let mut request = world.request(batch(THREE));
    request.acknowledge_database_shadowing = false;
    assert_eq!(
        plan_duckstation_native(&request).unwrap_err(),
        DuckStationNativeRefusal::DatabaseShadowingUnknown
    );
}

#[test]
fn an_empty_batch_is_refused() {
    let world = world_with_originals();
    let conflicts = conflicts_of(
        &world,
        DuckStationNativeOperation::AddMany { cheats: Vec::new() },
    );
    assert!(matches!(
        &conflicts[0],
        DuckStationBatchConflict::InvalidCheat { .. }
    ));
}

#[test]
fn hash_specific_files_and_multi_disc_still_refuse_batches() {
    let world = world_with_originals();
    World::write(
        &world
            .cheats()
            .join(format!("{SERIAL}_0123456789ABCDEF.cht")),
        "[X]\n",
    );
    assert!(matches!(
        plan_duckstation_native(&world.request(batch(THREE))),
        Err(DuckStationNativeRefusal::HashSpecificVariantPresent { .. })
    ));
    let world = world_with_originals();
    let mut request = world.request(batch(THREE));
    request.disc_topology = DuckStationDiscTopology::MultiDisc;
    assert_eq!(
        plan_duckstation_native(&request).unwrap_err(),
        DuckStationNativeRefusal::UnsupportedMultiDisc
    );
}

// ---- history: ONE row, ONE undo, trustworthy after a restart ----------------

fn grouped(world: &World) -> Vec<crate::patch_manager::GroupedHistoryRow> {
    let history = world.dir.path().join("history");
    let backups = world.dir.path().join("backups");
    let report = crate::patch_manager::discover_shared_apply_history(&history);
    let receipts = crate::patch_manager::discover_duckstation_native_receipts(&history);
    crate::patch_manager::group_shared_history(&report, receipts, &backups)
}

#[test]
fn the_two_child_journals_appear_as_one_grouped_history_row() {
    use crate::patch_manager::{GroupedHistoryRow, NativeGroupState};
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    apply(&world, &plan, "op-hist");
    let history = world.dir.path().join("history");
    let report = crate::patch_manager::discover_shared_apply_history(&history);
    // The receipt is not misread as a broken journal; the two children exist.
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.journals.len(), 2);

    let rows = grouped(&world);
    assert_eq!(
        rows.len(),
        1,
        "two journals, ONE user-visible row: {rows:?}"
    );
    let GroupedHistoryRow::DuckStationNative(group) = &rows[0] else {
        panic!("{rows:?}");
    };
    assert_eq!(group.children.len(), 2);
    assert_eq!(group.state, NativeGroupState::UndoAvailable);
    assert_eq!(group.receipt.cheats_added.len(), 3);
}

#[test]
fn history_is_rebuilt_from_disk_after_a_restart_and_undo_is_remembered() {
    use crate::patch_manager::{GroupedHistoryRow, NativeGroupState};
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let receipt = apply(&world, &plan, "op-restart");
    // "Restart": nothing but files on disk are consulted by `grouped`.
    let GroupedHistoryRow::DuckStationNative(group) = &grouped(&world)[0] else {
        panic!();
    };
    assert_eq!(group.state, NativeGroupState::UndoAvailable);

    undo_duckstation_native(&receipt, &world.options("op-restart-undo")).unwrap();
    let rows = grouped(&world);
    let GroupedHistoryRow::DuckStationNative(group) = &rows[0] else {
        panic!("{rows:?}");
    };
    assert_eq!(group.state, NativeGroupState::AlreadyUndone);
}

#[test]
fn a_child_journal_whose_receipt_is_missing_is_never_individually_undoable() {
    use crate::patch_manager::GroupedHistoryRow;
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let receipt = apply(&world, &plan, "op-orphan");
    fs::remove_file(receipt.receipt_path.as_ref().unwrap()).unwrap();
    let rows = grouped(&world);
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| matches!(row, GroupedHistoryRow::OrphanNativeChild { .. })),
        "{rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row, GroupedHistoryRow::Journal(_))),
        "an orphaned child must not be offered as an ordinary undoable journal"
    );
}

#[test]
fn an_edited_file_blocks_the_grouped_undo_for_both_files() {
    use crate::patch_manager::{GroupedHistoryRow, NativeGroupState};
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    apply(&world, &plan, "op-edited");
    fs::write(world.ini(), "[Cheats]\nEnable = SomethingElse\n").unwrap();
    let GroupedHistoryRow::DuckStationNative(group) = &grouped(&world)[0] else {
        panic!();
    };
    assert!(
        matches!(group.state, NativeGroupState::Blocked(_)),
        "{:?}",
        group.state
    );
}

// ---- partial undo: precise report, recoverable, resumable ------------------

use super::super::shared_transaction::inject_fault_after;

#[test]
fn a_partial_undo_failure_is_reported_precisely_and_leaves_a_recoverable_state() {
    use crate::patch_manager::{GroupedHistoryRow, NativeGroupState};
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let receipt = apply(&world, &plan, "op-partial");
    let batch_cht = text(&world.cht());

    // Settings restore (first) succeeds; the cheat-file restore (second) fails.
    inject_fault_after(FaultPoint::RollbackRestore, 1);
    let result = undo_duckstation_native(&receipt, &world.options("op-partial-undo"));
    inject_fault(None);
    match result {
        Err(DuckStationNativeApplyFailure::RollbackIncomplete { stage, detail }) => {
            assert_eq!(stage, "undo");
            assert!(detail.contains(&format!("cheats/{SERIAL}.cht")), "{detail}");
        }
        other => panic!("expected an incomplete undo, got {other:?}"),
    }
    // Precisely what happened: settings back, cheat file still holds the batch
    // (inert without its enable entries) - never an INI enabling a missing cheat.
    assert_eq!(text(&world.ini()), EXISTING_INI);
    assert_eq!(text(&world.cht()), batch_cht);

    // History does NOT claim a complete undo.
    let rows = grouped(&world);
    let GroupedHistoryRow::DuckStationNative(group) = &rows[0] else {
        panic!("{rows:?}");
    };
    match &group.state {
        NativeGroupState::PartiallyUndone {
            restored,
            remaining,
            resumable,
            ..
        } => {
            assert_eq!(restored.len(), 1);
            assert!(restored[0].ends_with(".ini"));
            assert_eq!(remaining.len(), 1);
            assert!(remaining[0].ends_with(".cht"));
            assert!(*resumable);
        }
        other => panic!("expected PartiallyUndone, got {other:?}"),
    }

    // Retrying finishes only what is left; both files end verified original.
    undo_duckstation_native(&receipt, &world.options("op-partial-retry")).unwrap();
    assert_eq!(text(&world.cht()), EXISTING_CHT);
    assert_eq!(text(&world.ini()), EXISTING_INI);
    let GroupedHistoryRow::DuckStationNative(group) = &grouped(&world)[0] else {
        panic!();
    };
    assert_eq!(group.state, NativeGroupState::AlreadyUndone);
}

#[test]
fn resuming_a_partial_undo_refuses_if_the_remaining_file_was_edited_meanwhile() {
    use crate::patch_manager::{GroupedHistoryRow, NativeGroupState};
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let receipt = apply(&world, &plan, "op-partial-edit");
    inject_fault_after(FaultPoint::RollbackRestore, 1);
    let _ = undo_duckstation_native(&receipt, &world.options("op-partial-edit-undo"));
    inject_fault(None);
    // Someone edits the remaining cheat file afterwards.
    let edited = format!("{}\n; my own edit\n", text(&world.cht()));
    fs::write(world.cht(), &edited).unwrap();
    assert!(undo_duckstation_native(&receipt, &world.options("op-partial-edit-retry")).is_err());
    assert_eq!(
        text(&world.cht()),
        edited,
        "a later user change is never overwritten"
    );
    let GroupedHistoryRow::DuckStationNative(group) = &grouped(&world)[0] else {
        panic!();
    };
    assert!(
        matches!(
            &group.state,
            NativeGroupState::PartiallyUndone {
                resumable: false,
                ..
            }
        ),
        "{:?}",
        group.state
    );
}

#[test]
fn undo_refuses_when_either_output_was_changed_and_overwrites_nothing() {
    let world = world_with_originals();
    let plan = plan_duckstation_native(&world.request(batch(THREE))).unwrap();
    let receipt = apply(&world, &plan, "op-guard");
    for target in [world.cht(), world.ini()] {
        let before = text(&target);
        let edited = format!("{before}\n; edited after apply\n");
        fs::write(&target, &edited).unwrap();
        assert!(undo_duckstation_native(&receipt, &world.options("op-guard-undo")).is_err());
        assert_eq!(text(&target), edited);
        fs::write(&target, before).unwrap();
    }
    undo_duckstation_native(&receipt, &world.options("op-guard-final")).unwrap();
}
