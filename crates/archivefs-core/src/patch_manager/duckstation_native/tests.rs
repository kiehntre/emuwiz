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

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
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

#[test]
fn a_failure_while_publishing_the_second_file_rolls_the_first_back() {
    if is_root() {
        return; // permission-based fault injection does not apply to root
    }
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    World::write(&world.cht(), EXISTING_CHT);
    fs::create_dir_all(world.game_settings()).unwrap();
    let plan = plan_duckstation_native(&world.request(add("Infinite Health", "30123456 00000063")))
        .unwrap();
    let cht_before = sha(&world.cht());
    // The INI folder becomes read-only after the preview.
    fs::set_permissions(world.game_settings(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = apply_duckstation_native_plan(&plan, &world.options("op-half"));
    fs::set_permissions(world.game_settings(), fs::Permissions::from_mode(0o755)).unwrap();
    match result {
        Err(DuckStationNativeApplyFailure::PublishFailedRolledBack { stage, .. }) => {
            assert_eq!(stage, "publish settings file");
        }
        other => panic!("expected a rolled-back failure, got {other:?}"),
    }
    // The .cht is exactly as before; no INI was created; no success receipt exists.
    assert_eq!(sha(&world.cht()), cht_before);
    assert!(!world.ini().exists());
    assert!(
        !world
            .dir
            .path()
            .join("history/duckstation-native-op-half.receipt.json")
            .exists()
    );
}

#[test]
fn a_failure_while_publishing_the_first_file_leaves_both_untouched() {
    if is_root() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    World::write(&world.ini(), EXISTING_INI);
    fs::create_dir_all(world.cheats()).unwrap();
    let plan = plan_duckstation_native(&world.request(add("A", "30123456 00000001"))).unwrap();
    let ini_before = sha(&world.ini());
    fs::set_permissions(world.cheats(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = apply_duckstation_native_plan(&plan, &world.options("op-first-fails"));
    fs::set_permissions(world.cheats(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err(), "{result:?}");
    assert_eq!(sha(&world.ini()), ini_before);
    assert!(!world.cht().exists());
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
