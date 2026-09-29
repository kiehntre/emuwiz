use std::fs;
use std::path::{Path, PathBuf};

use archivefs_core::dat::policy::{DatPlatformPolicyConfig, DatPolicyConfig};
use archivefs_core::dat::sources::{DatSourceKind, DatSourceOwnership};
use archivefs_core::emulator_inventory::InventoryEmulator;
use archivefs_core::setup_portability::*;
use archivefs_core::source_root_migration::MigrationClassification;
use tempfile::TempDir;

fn roots() -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new().unwrap();
    let config = temp.path().join("config");
    let identity = temp.path().join("identity");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(identity.join("romm")).unwrap();
    (temp, config, identity)
}

fn library_manifest(path: &Path) -> SetupManifest {
    SetupManifest {
        library: SetupLibrary {
            sources: vec![SetupSource {
                path: path.to_path_buf(),
                enabled: false,
            }],
            ..Default::default()
        },
        ..Default::default()
    }
}

fn remap(id: &str, path: &Path) -> SetupPathRemaps {
    [(id.into(), path.to_path_buf())].into_iter().collect()
}

#[test]
fn collects_enabled_disabled_sources_and_all_library_roots_without_reading_roms() {
    let (_temp, config, identity) = roots();
    fs::write(
        config.join("config.toml"),
        r#"mount_root = "/missing/mounts"
master_rom_root = "/missing/organised"
ratarmount_bin = "ratarmount"
[[source]]
path = "/missing/games"
enabled = true
[[source]]
path = "/missing/offline"
enabled = false
"#,
    )
    .unwrap();
    let manifest = collect_setup(&config, &identity).unwrap();
    assert_eq!(manifest.library.sources.len(), 2);
    assert!(manifest.library.sources[0].enabled);
    assert!(!manifest.library.sources[1].enabled);
    assert_eq!(manifest.library.mount_root, Some("/missing/mounts".into()));
    assert_eq!(
        manifest.library.master_rom_root,
        Some("/missing/organised".into())
    );
    assert_eq!(
        manifest.library.ratarmount_bin.as_deref(),
        Some("ratarmount")
    );
}

#[test]
fn dat_registrations_policy_and_platform_override_survive_round_trip() {
    let (temp, config, identity) = roots();
    fs::write(config.join("dat_sources.toml"), "[[sources]]\nid = 'ps2'\ndisplay_name = 'PS2 catalogue'\npath = '/missing/ps2.dat'\nkind = 'file'\nenabled = false\npriority = 9\nplatform = 'ps2'\n[policy]\nregion_preferences = ['europe', 'usa']\nrevision_policy = 'ask_when_ambiguous'\n[policy.platforms.ps2]\nlanguage_preferences = ['en']\n").unwrap();
    let manifest = collect_setup(&config, &identity).unwrap();
    assert_eq!(manifest.dat_sources.len(), 1);
    assert_eq!(manifest.dat_sources[0].enabled, Some(false));
    assert_eq!(manifest.dat_sources[0].priority, Some(9));
    assert_eq!(
        manifest.dat_policy.as_ref().unwrap().region_preferences,
        Some(vec!["europe".into(), "usa".into()])
    );
    let file = temp.path().join("setup.json");
    export_setup_new(&file, &manifest).unwrap();
    assert_eq!(read_setup_manifest(&file).unwrap(), manifest);
}

#[test]
fn supported_emulator_overrides_and_retroarch_core_selection_are_collected() {
    let (_temp, config, identity) = roots();
    for (file, path) in [
        ("ppsspp_executable_override.txt", "/missing/ppsspp"),
        ("ppsspp_configuration_folder_override.txt", "/missing/PSP"),
        ("pcsx2_executable_override.txt", "/missing/pcsx2"),
        (
            "dolphin_configuration_folder_override.txt",
            "/missing/dolphin",
        ),
        ("retroarch_core_directory_override.txt", "/missing/cores"),
    ] {
        fs::write(config.join(file), path).unwrap();
    }
    let manifest = collect_setup(&config, &identity).unwrap();
    assert_eq!(manifest.emulators.len(), 3);
    let ppsspp = manifest
        .emulators
        .iter()
        .find(|e| e.emulator == InventoryEmulator::Ppsspp)
        .unwrap();
    assert_eq!(ppsspp.executable, Some("/missing/ppsspp".into()));
    assert_eq!(ppsspp.configuration_folder, Some("/missing/PSP".into()));
    assert_eq!(
        manifest.retroarch_core_directory,
        Some("/missing/cores".into())
    );
    assert!(
        manifest
            .emulators
            .iter()
            .find(|e| e.emulator == InventoryEmulator::Dolphin)
            .unwrap()
            .executable
            .is_none()
    );
}

#[test]
fn romm_projection_removes_all_url_credentials_and_token_file_reference() {
    let (temp, config, identity) = roots();
    let secret = temp.path().join("secret-token");
    fs::write(&secret, "NEVER_EXPORT_THE_TOKEN").unwrap();
    let settings = serde_json::json!({
        "enabled": true,
        "url": "https://username:password@romm.example:8443/secret-base-path?api_key=SECRET#PRIVATE",
        "token_path": secret,
        "mappings": [{"provider_prefix": "roms", "archivefs_prefix": "/missing/games"}],
        "provider_path_kind": "provider_relative", "page_size": 50,
        "import_timeout_seconds": 600,
    });
    fs::write(identity.join("romm/config.json"), settings.to_string()).unwrap();
    let manifest = collect_setup(&config, &identity).unwrap();
    let romm = manifest.romm.as_ref().unwrap();
    assert_eq!(
        romm.server_origin.as_deref(),
        Some("https://romm.example:8443")
    );
    assert_eq!(
        romm.mappings[0].archivefs_prefix,
        PathBuf::from("/missing/games")
    );
    assert_eq!(romm.page_size, Some(50));
    let json = serde_json::to_string(&manifest).unwrap();
    for forbidden in [
        "username",
        "password",
        "secret-base-path",
        "SECRET",
        "PRIVATE",
        "token_path",
        "secret-token",
        "NEVER_EXPORT_THE_TOKEN",
    ] {
        assert!(!json.contains(forbidden), "leaked {forbidden}");
    }
    assert_eq!(
        fs::read_to_string(secret).unwrap(),
        "NEVER_EXPORT_THE_TOKEN"
    );
    let preview = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
    assert!(
        preview
            .attention
            .iter()
            .any(|text| text.contains("credentials"))
    );
}

#[test]
fn unknown_dat_fields_and_health_evidence_are_not_exported() {
    let (_temp, config, identity) = roots();
    fs::write(config.join("dat_sources.toml"), "secret = 'TOP_SECRET'\n[[sources]]\nid = 'dat'\ndisplay_name = 'DAT'\npath = '/missing/dat'\nkind = 'file'\norigin = 'SECRET_ORIGIN'\nhealth_detail = 'SECRET_HEALTH'\nfuture = 'SOURCE_SECRET'\n[policy]\nfuture = 'POLICY_SECRET'\n[policy.platforms.ps2]\nfuture = 'PLATFORM_SECRET'\n").unwrap();
    let manifest = collect_setup(&config, &identity).unwrap();
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(!json.contains("SECRET"));
    assert!(
        manifest
            .notices
            .iter()
            .any(|n| n.message.contains("Unknown DAT"))
    );
    manifest.validate().unwrap();
}

#[test]
fn malformed_configuration_is_reported_without_echoing_input_or_inventing_settings() {
    let (_temp, config, identity) = roots();
    fs::write(
        config.join("config.toml"),
        "mount_root = 'SECRET_BAD_INPUT\n",
    )
    .unwrap();
    fs::write(identity.join("romm/config.json"), "SECRET_MALFORMED_JSON").unwrap();
    let manifest = collect_setup(&config, &identity).unwrap();
    assert!(manifest.library.sources.is_empty());
    assert!(manifest.romm.is_none());
    assert!(
        manifest
            .notices
            .iter()
            .any(|n| n.message.contains("could not be parsed"))
    );
    assert!(!serde_json::to_string(&manifest).unwrap().contains("SECRET"));
}

#[test]
fn missing_configuration_is_explicit_and_has_no_inferred_emulators() {
    let (_temp, config, identity) = roots();
    let manifest = collect_setup(&config, &identity).unwrap();
    assert!(manifest.emulators.is_empty());
    assert!(manifest.dat_sources.is_empty());
    assert!(manifest.library.mount_root.is_none());
    assert!(
        manifest
            .notices
            .iter()
            .any(|n| n.area == SetupArea::Library && n.coverage == SetupCoverage::NotIncluded)
    );
}

#[test]
fn export_never_copies_unknown_files_or_optional_rom_save_config_state() {
    let (_temp, config, identity) = roots();
    for file in [
        "game.rom",
        "save.srm",
        "emulator.ini",
        "cheat_sources.toml",
        "recovery.json",
        "gui-v2.json",
        "extra.json",
    ] {
        fs::write(config.join(file), "DO_NOT_READ_OR_EXPORT").unwrap();
    }
    let manifest = collect_setup(&config, &identity).unwrap();
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(!json.contains("DO_NOT_READ_OR_EXPORT"));
    for area in [
        SetupArea::Artwork,
        SetupArea::Controllers,
        SetupArea::Launch,
        SetupArea::Conversion,
        SetupArea::CheatsMods,
        SetupArea::SavesConfigs,
    ] {
        assert!(
            manifest
                .notices
                .iter()
                .any(|n| n.area == area && n.coverage == SetupCoverage::NotIncluded)
        );
    }
}

#[test]
fn deterministic_collection_and_export_preserve_existing_config_bytes() {
    let (temp, config, identity) = roots();
    let original = br#"source_folders = ["/missing/roms"]
mount_root = "/missing/mount"
ratarmount_bin = "ratarmount"
"#;
    fs::write(config.join("config.toml"), original).unwrap();
    let first = collect_setup(&config, &identity).unwrap();
    assert_eq!(first.library.sources.len(), 1);
    let second = collect_setup(&config, &identity).unwrap();
    assert_eq!(first, second);
    export_setup_new(&temp.path().join("a.json"), &first).unwrap();
    export_setup_new(&temp.path().join("b.json"), &second).unwrap();
    assert_eq!(
        fs::read(temp.path().join("a.json")).unwrap(),
        fs::read(temp.path().join("b.json")).unwrap()
    );
    assert_eq!(fs::read(config.join("config.toml")).unwrap(), original);
}

#[test]
fn export_refuses_to_overwrite_an_existing_file() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("setup.json");
    fs::write(&file, "EXISTING").unwrap();
    assert!(
        export_setup_new(&file, &SetupManifest::default())
            .unwrap_err()
            .contains("already exists")
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "EXISTING");
}

#[test]
fn bounded_manifest_read_rejects_oversized_or_unsupported_files_without_echoing_them() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("setup.json");
    fs::write(&file, vec![b'X'; MAX_SETUP_BYTES as usize + 1]).unwrap();
    assert!(read_setup_manifest(&file).unwrap_err().contains("limit"));
    let mut future = SetupManifest::default();
    future.format_version += 1;
    fs::write(&file, serde_json::to_vec(&future).unwrap()).unwrap();
    assert!(
        read_setup_manifest(&file)
            .unwrap_err()
            .contains("unsupported format")
    );
    fs::write(&file, "{\"SECRET\": \"TOKEN\"}").unwrap();
    let error = read_setup_manifest(&file).unwrap_err();
    assert!(!error.contains("SECRET"));
    assert!(!error.contains("TOKEN"));
}

#[test]
fn unconfirmed_paths_are_not_probed_or_assumed_portable() {
    let temp = TempDir::new().unwrap();
    let manifest = library_manifest(temp.path());
    let preview = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
    assert_eq!(
        preview.paths[0].proposal.classification,
        MigrationClassification::ManualReview
    );
    assert!(preview.paths[0].proposal.candidate_path.is_none());
    assert!(preview.read_only);
}

#[test]
fn exact_field_remapping_preserves_original_source_and_checks_destination() {
    let temp = TempDir::new().unwrap();
    let original = Path::new("/original/device/games");
    let manifest = library_manifest(original);
    let preview = preview_setup_import(&manifest, &remap("library.source.0", temp.path())).unwrap();
    assert_eq!(
        preview.paths[0].proposal.classification,
        MigrationClassification::AlreadyCurrent
    );
    assert_eq!(preview.paths[0].proposal.old_path, original);
    assert_eq!(
        preview.paths[0].proposal.candidate_path.as_deref(),
        Some(temp.path())
    );
    assert_eq!(manifest.library.sources[0].path, original);
    assert!(preview.reusable_settings[0].contains("enable/disable"));
}

#[test]
fn missing_emulator_file_is_reported_after_explicit_location_confirmation() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing-ppsspp");
    let manifest = SetupManifest {
        emulators: vec![SetupEmulator {
            emulator: InventoryEmulator::Ppsspp,
            executable: Some(missing.clone()),
            configuration_folder: None,
        }],
        ..Default::default()
    };
    let unconfirmed = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
    assert!(unconfirmed.missing_emulators.is_empty());
    let preview =
        preview_setup_import(&manifest, &remap("emulator.Ppsspp.executable", &missing)).unwrap();
    assert_eq!(
        preview.paths[0].proposal.classification,
        MigrationClassification::TargetMissing
    );
    assert!(preview.missing_emulators[0].contains("PPSSPP"));
}

#[test]
fn stale_remap_identity_is_rejected_instead_of_reused_for_another_file() {
    let manifest = library_manifest(Path::new("/source"));
    assert!(preview_setup_import(&manifest, &remap("wrong.field", Path::new("/local"))).is_err());
}

#[test]
fn foreign_paths_can_be_remapped_but_cannot_be_assumed_local() {
    let temp = TempDir::new().unwrap();
    let original = Path::new("C:\\Games\\ROMs");
    let mut manifest = library_manifest(original);
    manifest.source_os = "other-os".into();
    let preview = preview_setup_import(&manifest, &remap("library.source.0", temp.path())).unwrap();
    assert_eq!(
        preview.paths[0].proposal.classification,
        MigrationClassification::AlreadyCurrent
    );
    assert!(
        preview
            .attention
            .iter()
            .any(|s| s.contains("another operating system"))
    );
    assert_eq!(preview.paths[0].proposal.old_path, original);
}

#[test]
fn relative_parent_traversal_and_wrong_file_types_need_review() {
    let temp = TempDir::new().unwrap();
    let manifest = library_manifest(Path::new("/source"));
    let file = temp.path().join("not-a-directory");
    fs::write(&file, "contents").unwrap();
    for path in [
        PathBuf::from("relative"),
        temp.path().join("../outside"),
        file,
    ] {
        let preview = preview_setup_import(&manifest, &remap("library.source.0", &path)).unwrap();
        assert_eq!(
            preview.paths[0].proposal.classification,
            MigrationClassification::ManualReview
        );
    }
}

#[test]
fn duplicate_emulators_and_dat_identities_are_rejected() {
    let entry = SetupEmulator {
        emulator: InventoryEmulator::Xemu,
        executable: None,
        configuration_folder: None,
    };
    let manifest = SetupManifest {
        emulators: vec![entry.clone(), entry],
        ..Default::default()
    };
    assert!(manifest.validate().is_err());
    let dat = SetupDatSource {
        id: "same".into(),
        display_name: "DAT".into(),
        path: "/dat".into(),
        kind: DatSourceKind::File,
        ownership: DatSourceOwnership::UserLocal,
        enabled: None,
        priority: None,
        platform: None,
    };
    let manifest = SetupManifest {
        dat_sources: vec![dat.clone(), dat],
        ..Default::default()
    };
    assert!(manifest.validate().is_err());
}

#[test]
fn unknown_policy_values_need_attention_and_unknown_fields_are_rejected() {
    let mut manifest = SetupManifest {
        dat_policy: Some(DatPolicyConfig {
            revision_policy: Some("future_policy".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let preview = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
    assert!(
        preview
            .attention
            .iter()
            .any(|s| s.contains("not recognised"))
    );
    let mut platform = DatPlatformPolicyConfig::default();
    platform
        .unknown_fields
        .insert("future".into(), toml::Value::String("secret".into()));
    manifest.dat_policy.as_mut().unwrap().platforms =
        Some([("ps2".into(), platform)].into_iter().collect());
    assert!(manifest.validate().is_err());
}

#[test]
fn managed_dat_ownership_survives_but_does_not_grant_destination_update_authority() {
    let manifest = SetupManifest {
        dat_sources: vec![SetupDatSource {
            id: "managed".into(),
            display_name: "DAT".into(),
            path: "/dat".into(),
            kind: DatSourceKind::File,
            ownership: DatSourceOwnership::EmuWizManaged,
            enabled: None,
            priority: None,
            platform: None,
        }],
        ..Default::default()
    };
    let roundtrip: SetupManifest =
        serde_json::from_slice(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(roundtrip, manifest);
    let preview = preview_setup_import(&roundtrip, &SetupPathRemaps::new()).unwrap();
    assert!(
        preview
            .attention
            .iter()
            .any(|s| s.contains("does not authorise"))
    );
}

#[cfg(unix)]
#[test]
fn symlink_settings_exports_and_intermediate_location_links_are_refused() {
    use std::os::unix::fs::symlink;
    let (temp, config, identity) = roots();
    let secret = temp.path().join("secret.json");
    fs::write(&secret, "SECRET").unwrap();
    symlink(&secret, config.join("config.toml")).unwrap();
    let manifest = collect_setup(&config, &identity).unwrap();
    assert!(manifest.library.sources.is_empty());
    assert!(!serde_json::to_string(&manifest).unwrap().contains("SECRET"));
    let link = temp.path().join("setup.json");
    symlink(&secret, &link).unwrap();
    assert!(export_setup_new(&link, &manifest).is_err());
    assert!(read_setup_manifest(&link).is_err());
    assert_eq!(fs::read_to_string(&secret).unwrap(), "SECRET");
    let directory_link = temp.path().join("directory-link");
    symlink(&config, &directory_link).unwrap();
    let selected = directory_link.join("child");
    fs::create_dir(config.join("child")).unwrap();
    let manifest = library_manifest(Path::new("/source"));
    let preview = preview_setup_import(&manifest, &remap("library.source.0", &selected)).unwrap();
    assert_eq!(
        preview.paths[0].proposal.classification,
        MigrationClassification::ManualReview
    );
}

#[cfg(unix)]
#[test]
fn executable_location_requires_permission_but_is_never_executed() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let executable = temp.path().join("fake-emulator");
    let marker = temp.path().join("executed-marker");
    let script = format!(
        "#!/bin/sh\nprintf executed > '{}'\nexit 99\n",
        marker.display()
    );
    fs::write(&executable, &script).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o600)).unwrap();
    let manifest = SetupManifest {
        emulators: vec![SetupEmulator {
            emulator: InventoryEmulator::Xemu,
            executable: Some(executable.clone()),
            configuration_folder: None,
        }],
        ..Default::default()
    };
    let remaps = remap("emulator.Xemu.executable", &executable);
    assert_eq!(
        preview_setup_import(&manifest, &remaps).unwrap().paths[0]
            .proposal
            .classification,
        MigrationClassification::ManualReview
    );
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        preview_setup_import(&manifest, &remaps).unwrap().paths[0]
            .proposal
            .classification,
        MigrationClassification::AlreadyCurrent
    );
    assert_eq!(fs::read_to_string(executable).unwrap(), script);
    assert!(!marker.exists(), "an imported executable was run");
}

#[cfg(unix)]
#[test]
fn exported_locations_have_private_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("setup.json");
    export_setup_new(&file, &SetupManifest::default()).unwrap();
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn imported_server_origin_cannot_carry_credentials_or_a_url_path() {
    let (_temp, config, identity) = roots();
    fs::write(
        identity.join("romm/config.json"),
        r#"{"enabled":false,"url":"https://romm.example","mappings":[],"token_path":null}"#,
    )
    .unwrap();
    let mut manifest = collect_setup(&config, &identity).unwrap();
    for origin in [
        "https://user:SECRET@romm.example",
        "https://romm.example/SECRET",
        "https://romm.example?token=SECRET",
        "https://romm.example#SECRET",
        "file:///SECRET",
    ] {
        manifest.romm.as_mut().unwrap().server_origin = Some(origin.into());
        let error = manifest.validate().unwrap_err();
        assert!(!error.contains("SECRET"));
    }
}

#[test]
fn oversized_known_configuration_is_omitted_with_an_explicit_notice() {
    let (_temp, config, identity) = roots();
    fs::write(
        config.join("ppsspp_executable_override.txt"),
        vec![b'X'; MAX_SETUP_BYTES as usize + 1],
    )
    .unwrap();
    let manifest = collect_setup(&config, &identity).unwrap();
    assert!(manifest.emulators.is_empty());
    assert!(manifest.notices.iter().any(|notice| {
        notice
            .message
            .contains("ppsspp_executable_override.txt could not be read safely")
    }));
}

#[test]
fn provenance_of_paths_and_order_are_stable_across_preview_and_serialization() {
    let temp = TempDir::new().unwrap();
    let mut manifest = library_manifest(temp.path());
    manifest.library.mount_root = Some(temp.path().join("missing-mounts"));
    manifest.retroarch_core_directory = Some(temp.path().join("missing-cores"));
    let original = manifest.clone();
    let paths = manifest.paths();
    assert!(paths.windows(2).all(|pair| pair[0].id < pair[1].id));
    let first = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
    let second = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
    assert_eq!(first, second);
    let parsed: SetupManifest =
        serde_json::from_slice(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(parsed.paths(), paths);
    assert_eq!(manifest, original);
}
