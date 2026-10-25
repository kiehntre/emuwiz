use super::*;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    temp: tempfile::TempDir,
    selection: Rpcs3ActivationSelection,
    definition: PathBuf,
}
fn sfo(title: &str, version: &str) -> Vec<u8> {
    let entries = [
        ("TITLE_ID", title),
        ("APP_VER", version),
        ("CATEGORY", "DG"),
    ];
    let mut keys = Vec::new();
    let mut data = Vec::new();
    let mut indexes = Vec::new();
    for (key, value) in entries {
        let k = keys.len() as u16;
        keys.extend_from_slice(key.as_bytes());
        keys.push(0);
        let d = data.len() as u32;
        data.extend_from_slice(value.as_bytes());
        data.push(0);
        indexes.extend_from_slice(&k.to_le_bytes());
        indexes.extend_from_slice(&0x204u16.to_le_bytes());
        indexes.extend_from_slice(&((value.len() + 1) as u32).to_le_bytes());
        indexes.extend_from_slice(&((value.len() + 1) as u32).to_le_bytes());
        indexes.extend_from_slice(&d.to_le_bytes());
    }
    let key_start = 20 + indexes.len();
    let data_start = key_start + keys.len();
    let mut out = b"\0PSF".to_vec();
    out.extend_from_slice(&0x101u32.to_le_bytes());
    out.extend_from_slice(&(key_start as u32).to_le_bytes());
    out.extend_from_slice(&(data_start as u32).to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend(indexes);
    out.extend(keys);
    out.extend(data);
    out
}
fn yaml(version: &str) -> Vec<u8> {
    format!("# local user comment\nVersion: 1.2\n\nPPU-0123456789abcdef0123456789abcdef01234567:\n  \"60 FPS\":\n    Games:\n      Example:\n        BLUS12345:\n          - \"{version}\"\n    Patch:\n      - [ be32, 0x00100000, 0x00000001 ]\n").into_bytes()
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let home = root.join("home");
        let config = home.join("config/rpcs3");
        let game = root.join("game");
        let executable = root.join("bin/rpcs3");
        fs::create_dir_all(config.join("patches")).unwrap();
        fs::create_dir_all(game.join("PS3_GAME/USRDIR")).unwrap();
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(
            &executable,
            b"\x7fELF disposable native executable; never executed",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            config.join("config.yml"),
            b"Core:\n  PPU Decoder: Recompiler (LLVM)\n",
        )
        .unwrap();
        fs::write(game.join("PS3_GAME/PARAM.SFO"), sfo("BLUS12345", "01.03")).unwrap();
        fs::write(
            game.join("PS3_GAME/USRDIR/EBOOT.BIN"),
            b"SCE\0disposable SELF",
        )
        .unwrap();
        let definition = config.join("patches/imported_patch.yml");
        fs::write(&definition, yaml("01.03")).unwrap();
        let roots = Rpcs3ProfileDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home.join("config"),
            xdg_data_home: home.join("data"),
            explicit_configuration_roots: vec![],
            portable_configuration_roots: vec![],
            explicit_executables: vec![executable.clone()],
            known_version_outputs: Default::default(),
            appimage_directory: None,
            selected_executable: None,
            path_override: Some(vec![]),
        };
        let profile = discover_rpcs3_profiles(&roots)
            .profiles
            .into_iter()
            .find(|p| p.installation_type == Rpcs3InstallationType::Native)
            .unwrap();
        Self {
            temp,
            definition,
            selection: Rpcs3ActivationSelection {
                game,
                roots,
                profile_id: profile.profile_id,
                configuration_root: config,
                executable,
            },
        }
    }
    fn request(&self) -> Rpcs3ActivationRequest {
        let c = inspect_rpcs3_activation(&self.selection).unwrap();
        let e = &c.entries[0];
        Rpcs3ActivationRequest {
            selection: self.selection.clone(),
            source: e.source.clone(),
            expected_source_sha256: e.source_sha256.clone(),
            hash: e.hash.clone(),
            description: e.definition.description.clone(),
            expected_title_id: c.title_id,
            expected_app_version: c.app_version,
            enabled: true,
        }
    }
    fn options(&self, p: &Rpcs3ActivationPreview) -> SharedApplyOptions {
        SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: p.plan.plan_id.clone(),
                general_approved: true,
                replacement_approved: false,
            }),
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 1,
            current_context: p.plan.context.clone(),
            history_root: self.temp.path().join("history"),
            backup_root: self.temp.path().join("backups"),
        }
    }
}
fn apply_fixture(f: &Fixture, p: &Rpcs3ActivationPreview) -> Result<SharedApplyResult, String> {
    apply_with_probe(p, &f.selection, &f.options(p), || {
        EmulatorProcessObservation::NotRunning
    })
}
#[test]
fn confirmed_config_only_apply_readback_history_and_guarded_undo() {
    let f = Fixture::new();
    let definition = fs::read(&f.definition).unwrap();
    let game_sfo = fs::read(f.selection.game.join("PS3_GAME/PARAM.SFO")).unwrap();
    let game_self = fs::read(f.selection.game.join("PS3_GAME/USRDIR/EBOOT.BIN")).unwrap();
    let config = fs::read(f.selection.configuration_root.join("config.yml")).unwrap();
    let p = preview_rpcs3_activation(&f.request()).unwrap();
    assert!(!p.destination.exists());
    assert_eq!(p.plan.entries.len(), 1);
    assert_eq!(
        p.plan.entries[0]
            .destination_relative_path
            .to_path_buf()
            .unwrap(),
        PathBuf::from("patch_config.yml")
    );
    let result = apply_fixture(&f, &p).unwrap();
    assert_eq!(
        result.journal.status,
        SharedApplyStatus::Success,
        "{result:?}"
    );
    assert_eq!(fs::read(&p.destination).unwrap(), p.config_bytes);
    assert!(String::from_utf8_lossy(&p.config_bytes).contains("Enabled: true"));
    let options = f.options(&p);
    let rows = discover_shared_apply_history(&options.history_root);
    assert!(rows.complete);
    assert_eq!(rows.journals.len(), 1);
    let journal = result.journal_path.unwrap();
    fs::write(&p.destination, b"unrelated changed user configuration\n").unwrap();
    assert!(
        !preview_rpcs3_patch_rollback(
            &journal,
            &f.selection.configuration_root,
            &options.backup_root
        )
        .available
    );
    fs::write(&p.destination, &p.config_bytes).unwrap();
    let undo = preview_rpcs3_patch_rollback(
        &journal,
        &f.selection.configuration_root,
        &options.backup_root,
    );
    assert!(undo.available);
    let result = rollback_rpcs3_patch(
        &undo,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: undo.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 2,
            history_root: options.history_root,
            backup_root: options.backup_root,
        },
    );
    assert_eq!(result.status, SharedApplyStatus::Success);
    assert!(!p.destination.exists());
    assert_eq!(fs::read(&f.definition).unwrap(), definition);
    assert_eq!(
        fs::read(f.selection.game.join("PS3_GAME/PARAM.SFO")).unwrap(),
        game_sfo
    );
    assert_eq!(
        fs::read(f.selection.game.join("PS3_GAME/USRDIR/EBOOT.BIN")).unwrap(),
        game_self
    );
    assert_eq!(
        fs::read(f.selection.configuration_root.join("config.yml")).unwrap(),
        config
    );
}
#[test]
fn exact_and_all_version_declarations_use_correct_config_key() {
    let f = Fixture::new();
    fs::write(&f.definition, yaml("All")).unwrap();
    let p = preview_rpcs3_activation(&f.request()).unwrap();
    assert!(String::from_utf8_lossy(&p.config_bytes).contains("\"All\":"));
    assert!(!String::from_utf8_lossy(&p.config_bytes).contains("\"01.03\":"));
    let mut r = f.request();
    r.enabled = false;
    assert!(
        String::from_utf8_lossy(&preview_rpcs3_activation(&r).unwrap().config_bytes)
            .contains("Enabled: false")
    );
    r.expected_app_version = "01.99".into();
    assert!(preview_rpcs3_activation(&r).is_err());
    r.expected_app_version = "01.03".into();
    r.expected_title_id = "BLES54321".into();
    assert!(preview_rpcs3_activation(&r).is_err());
}
#[test]
fn incompatible_game_or_version_and_unknown_operations_refuse() {
    let f = Fixture::new();
    for text in [
        String::from_utf8(yaml("01.99")).unwrap(),
        String::from_utf8(yaml("01.03"))
            .unwrap()
            .replace("BLUS12345", "BLES54321"),
    ] {
        fs::write(&f.definition, text).unwrap();
        let c = inspect_rpcs3_activation(&f.selection).unwrap();
        let e = &c.entries[0];
        assert!(!matches!(
            e.readiness,
            Rpcs3PatchReadiness::ExactCompatible | Rpcs3PatchReadiness::AllVersionsCompatible
        ));
        let r = Rpcs3ActivationRequest {
            selection: f.selection.clone(),
            source: e.source.clone(),
            expected_source_sha256: e.source_sha256.clone(),
            hash: e.hash.clone(),
            description: e.definition.description.clone(),
            expected_title_id: c.title_id,
            expected_app_version: c.app_version,
            enabled: true,
        };
        assert!(preview_rpcs3_activation(&r).is_err());
    }
    fs::write(
        &f.definition,
        String::from_utf8(yaml("01.03"))
            .unwrap()
            .replace("be32", "jump"),
    )
    .unwrap();
    assert!(inspect_rpcs3_activation(&f.selection).is_err());
    assert!(
        !f.selection
            .configuration_root
            .join("patch_config.yml")
            .exists()
    );
}
#[test]
fn existing_configuration_and_conflicting_definitions_are_preserved() {
    let f = Fixture::new();
    fs::write(
        f.selection.configuration_root.join("patch_config.yml"),
        b"user-created config",
    )
    .unwrap();
    assert!(inspect_rpcs3_activation(&f.selection).is_err());
    assert_eq!(
        fs::read(f.selection.configuration_root.join("patch_config.yml")).unwrap(),
        b"user-created config"
    );
    let f = Fixture::new();
    fs::write(
        f.selection.configuration_root.join("patches/patch.yml"),
        yaml("01.03"),
    )
    .unwrap();
    assert!(inspect_rpcs3_activation(&f.selection).is_err());
    assert_eq!(fs::read(&f.definition).unwrap(), yaml("01.03"));
}
#[test]
fn selection_definition_target_and_identity_drift_refuse_without_writes() {
    let f = Fixture::new();
    let p = preview_rpcs3_activation(&f.request()).unwrap();
    let mut changed = f.selection.clone();
    changed.profile_id.push_str("other");
    assert!(
        apply_with_probe(&p, &changed, &f.options(&p), || {
            EmulatorProcessObservation::NotRunning
        })
        .is_err()
    );
    assert!(!p.destination.exists());
    fs::write(&f.definition, yaml("01.04")).unwrap();
    assert!(apply_fixture(&f, &p).is_err());
    assert!(!p.destination.exists());
    let f = Fixture::new();
    let p = preview_rpcs3_activation(&f.request()).unwrap();
    fs::write(&p.destination, b"new user file").unwrap();
    assert!(apply_fixture(&f, &p).is_err());
    assert_eq!(fs::read(&p.destination).unwrap(), b"new user file");
    let f = Fixture::new();
    let p = preview_rpcs3_activation(&f.request()).unwrap();
    fs::write(
        f.selection.game.join("PS3_GAME/PARAM.SFO"),
        sfo("BLUS12345", "01.04"),
    )
    .unwrap();
    assert!(apply_fixture(&f, &p).is_err());
    assert!(!p.destination.exists());
}
#[test]
fn missing_sources_portable_vfs_updates_and_symlinks_refuse() {
    for variant in 0..5 {
        let f = Fixture::new();
        let p = preview_rpcs3_activation(&f.request()).unwrap();
        match variant {
            0 => {
                fs::remove_file(&f.definition).unwrap();
            }
            1 => {
                fs::create_dir(f.selection.executable.parent().unwrap().join("portable")).unwrap();
            }
            2 => {
                fs::write(
                    f.selection.configuration_root.join("vfs.yml"),
                    b"custom VFS",
                )
                .unwrap();
            }
            3 => {
                fs::create_dir_all(
                    f.selection
                        .configuration_root
                        .join("dev_hdd0/game/BLUS12345"),
                )
                .unwrap();
            }
            _ => {
                fs::remove_file(&f.definition).unwrap();
                std::os::unix::fs::symlink(
                    f.selection.game.join("PS3_GAME/PARAM.SFO"),
                    &f.definition,
                )
                .unwrap();
            }
        }
        assert!(apply_fixture(&f, &p).is_err());
        assert!(!p.destination.exists());
    }
}
#[test]
fn no_confirmation_and_running_or_unobserved_process_do_not_activate() {
    let f = Fixture::new();
    let p = preview_rpcs3_activation(&f.request()).unwrap();
    let mut options = f.options(&p);
    options.confirmation = None;
    let r = apply_with_probe(&p, &f.selection, &options, || {
        EmulatorProcessObservation::NotRunning
    })
    .unwrap();
    assert_ne!(r.journal.status, SharedApplyStatus::Success);
    assert!(!p.destination.exists());
    for status in [
        EmulatorProcessObservation::Running,
        EmulatorProcessObservation::NotObserved,
    ] {
        assert!(apply_with_probe(&p, &f.selection, &f.options(&p), || status).is_err());
        assert!(!p.destination.exists());
    }
}
#[test]
fn malformed_yaml_extra_operands_and_multiple_title_maps_are_read_only() {
    let f = Fixture::new();
    for bytes in [
        b"Version: 1.2\nA: &alias\n".to_vec(),
        String::from_utf8(yaml("01.03"))
            .unwrap()
            .replace("0x00000001 ]", "0x00000001, 9 ]")
            .into_bytes(),
        String::from_utf8(yaml("01.03"))
            .unwrap()
            .replace(
                "    Patch:",
                "      Other:\n        BLES54321:\n          - \"01.99\"\n    Patch:",
            )
            .into_bytes(),
    ] {
        fs::write(&f.definition, &bytes).unwrap();
        assert!(inspect_rpcs3_activation(&f.selection).is_err());
        assert_eq!(fs::read(&f.definition).unwrap(), bytes);
        assert!(
            !f.selection
                .configuration_root
                .join("patch_config.yml")
                .exists()
        );
    }
}

#[test]
fn exact_approval_cannot_be_replayed_for_another_state_entry_or_reviewed_evidence() {
    let f = Fixture::new();
    let text = String::from_utf8(yaml("01.03")).unwrap();
    let (_, rest) = text.split_once("  \"60 FPS\":").unwrap();
    fs::write(&f.definition, format!("{text}  \"Other patch\":{rest}")).unwrap();
    let mut request = f.request();
    request.enabled = false;
    let disabled = preview_rpcs3_activation(&request).unwrap();
    request.enabled = true;
    let enabled = preview_rpcs3_activation(&request).unwrap();
    assert_ne!(disabled.plan.plan_id, enabled.plan.plan_id);
    request.description = "Other patch".into();
    let other = preview_rpcs3_activation(&request).unwrap();
    assert_ne!(enabled.plan.plan_id, other.plan.plan_id);
    let mut options = f.options(&enabled);
    options.confirmation = f.options(&disabled).confirmation;
    let refused = apply_with_probe(&enabled, &f.selection, &options, || {
        EmulatorProcessObservation::NotRunning
    })
    .unwrap();
    assert_eq!(refused.journal.status, SharedApplyStatus::Failed);
    assert!(refused.journal_path.is_none());
    assert!(!enabled.destination.exists());
    assert!(!options.history_root.exists());

    fs::write(
        &f.selection.executable,
        b"\x7fELF changed disposable native executable",
    )
    .unwrap();
    let changed_executable = preview_rpcs3_activation(&request).unwrap();
    assert_ne!(other.plan.plan_id, changed_executable.plan.plan_id);
    fs::write(
        f.selection.configuration_root.join("config.yml"),
        b"Core:\n  PPU Decoder: Interpreter\n",
    )
    .unwrap();
    let changed_config = preview_rpcs3_activation(&request).unwrap();
    assert_ne!(changed_executable.plan.plan_id, changed_config.plan.plan_id);

    let f = Fixture::new();
    fs::write(&f.definition, yaml("All")).unwrap();
    let first_version = preview_rpcs3_activation(&f.request()).unwrap();
    fs::write(
        f.selection.game.join("PS3_GAME/PARAM.SFO"),
        sfo("BLUS12345", "01.04"),
    )
    .unwrap();
    let next_version = preview_rpcs3_activation(&f.request()).unwrap();
    assert_eq!(first_version.config_bytes, next_version.config_bytes);
    assert_ne!(first_version.plan.plan_id, next_version.plan.plan_id);
}

#[test]
fn history_mode_accepts_only_exact_activation_prefix_and_full_lowercase_digest() {
    assert!(is_rpcs3_activation_source_mode(&format!(
        "{RPCS3_ACTIVATION_SOURCE_MODE}:{}",
        "a0".repeat(32)
    )));
    for mode in [
        RPCS3_ACTIVATION_SOURCE_MODE.to_owned(),
        format!("{RPCS3_ACTIVATION_SOURCE_MODE}:short"),
        format!("{RPCS3_ACTIVATION_SOURCE_MODE}:{}", "A".repeat(64)),
        format!("unrelated:{}", "a".repeat(64)),
    ] {
        assert!(!is_rpcs3_activation_source_mode(&mode));
    }
}
