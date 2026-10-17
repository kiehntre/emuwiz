use super::*;
use crate::launch::{
    self, CanonicalIdentityStatus, FirmwareReadiness, LaunchContainerKind, LaunchContentRef,
    ResolvedIdentity, StandaloneProfileInput,
};
use crate::patch_manager::CemuProfileDiscoveryRoots;
use std::{fs, path::Path};
use tempfile::{TempDir, tempdir};
const BASE: &str = "0005000010101A00";
fn title(path: &Path, id: &str, version: &str, code: bool) {
    fs::create_dir_all(path.join("meta")).unwrap();
    fs::create_dir_all(path.join("content")).unwrap();
    fs::write(
        path.join("meta/meta.xml"),
        format!("<menu><title_id>{id}</title_id><title_version>{version}</title_version></menu>"),
    )
    .unwrap();
    if code {
        fs::create_dir_all(path.join("code")).unwrap();
        fs::write(path.join("code/game.rpx"), b"synthetic-rpx").unwrap();
    }
}
fn fixture() -> (TempDir, PathBuf, CemuProfileDiscoveryRoots) {
    let d = tempdir().unwrap();
    let base = d.path().join("base");
    title(&base, BASE, "0", true);
    let profile = d.path().join("profile");
    fs::create_dir(&profile).unwrap();
    let mlc = d.path().join("mlc");
    fs::create_dir(&mlc).unwrap();
    fs::write(
        profile.join("settings.xml"),
        format!("<content><mlc_path>{}</mlc_path></content>", mlc.display()),
    )
    .unwrap();
    let exe = d.path().join("Cemu");
    fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let roots = CemuProfileDiscoveryRoots {
        home: d.path().join("private-home"),
        xdg_config_home: d.path().join("private-xdg"),
        explicit_configuration_roots: vec![profile],
        portable_configuration_roots: vec![],
        explicit_executables: vec![exe],
        known_version_outputs: Default::default(),
        appimage_directory: None,
    };
    (d, base, roots)
}
fn planned(
    inspection: Option<&CemuReadinessInspection>,
    path: &Path,
    id: &str,
    profile_id: &str,
) -> LaunchPlan {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "WiiU".into(),
        game_key: id.into(),
    });
    let content = LaunchContentRef {
        kind: None,
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(path.into()),
        requires_mount: false,
        provenance: "synthetic verified input".into(),
    };
    let profiles = [StandaloneProfileInput {
        adapter_id: "cemu",
        profile_id: profile_id.into(),
        profile_path: None,
        eligible: true,
        firmware: FirmwareReadiness::NotRequired,
    }];
    let env = crate::emulator_environment::retroarch::RetroArchEnvironmentReport {
        format_version: 2,
        profiles: vec![],
        diagnostics: vec![],
    };
    let mut plan = launch::build_launch_plan(&identity, &content, &profiles, &env, &[]);
    apply_cemu_readiness(&mut plan, inspection);
    plan
}
#[test]
fn extracted_base_preview_and_existing_preflight_agree() {
    let (_d, path, roots) = fixture();
    let inspection = inspect_cemu_readiness(&path, Some(BASE), &roots);
    assert!(inspection.refusals.is_empty(), "{:?}", inspection.refusals);
    let profile = inspection
        .profiles
        .iter()
        .find(|p| p.command.is_some())
        .unwrap();
    let command = profile.command.as_ref().unwrap();
    let actual = preflight_cemu_launch(
        &CemuPreflightRequest {
            selected_content_path: path.clone(),
            profile_id: profile.profile_id.clone(),
            expected_executable: command.executable.clone(),
        },
        &roots,
    )
    .unwrap();
    assert_eq!(&actual, command);
    assert_eq!(
        planned(Some(&inspection), &path, BASE, &profile.profile_id)
            .summary
            .ready,
        1
    );
}
#[test]
fn matching_update_and_dlc_are_presence_evidence_only() {
    let (d, path, roots) = fixture();
    for (high, role) in [
        ("0005000e", WiiUContentRole::Update),
        ("0005000c", WiiUContentRole::Dlc),
    ] {
        let addon = d.path().join("mlc/usr/title").join(high).join("10101a00");
        title(&addon, &format!("{high}10101a00"), "32", false);
        let projection = content::project_wiiu_content_readiness(
            &addon,
            WiiUKeyStoreEvidence::NotChecked,
            Some(BASE),
        );
        assert_eq!(projection.role, role);
        assert_eq!(projection.readiness, WiiUContentReadiness::WrongContentRole);
    }
    let inspection = inspect_cemu_readiness(&path, Some(BASE), &roots);
    let profile = inspection
        .profiles
        .iter()
        .find(|p| p.command.is_some())
        .unwrap();
    assert_eq!(profile.related.len(), 2);
    assert!(profile.refusals.is_empty());
}
#[test]
fn mismatched_mlc_title_id_refuses_preview_and_preflight() {
    let (d, path, roots) = fixture();
    title(
        &d.path().join("mlc/usr/title/0005000e/10101a00"),
        "0005000E99999999",
        "16",
        false,
    );
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
    assert!(
        inspection
            .profiles
            .iter()
            .any(|p| p.refusals.iter().any(|r| r.contains("mismatched")))
    );
}
#[test]
fn verified_title_id_mismatch_is_never_overwritten() {
    let (_d, path, roots) = fixture();
    let inspection = inspect_cemu_readiness(&path, Some("0005000099999999"), &roots);
    assert!(inspection.refusals.iter().any(|r| r.contains("TITLE_ID")));
    let profile = &inspection.profiles[0];
    let plan = planned(
        Some(&inspection),
        &path,
        "0005000099999999",
        &profile.profile_id,
    );
    assert_eq!(plan.game_key.as_deref(), Some("0005000099999999"));
    assert_eq!(plan.summary.ready, 0);
}
#[test]
fn absent_content_and_absent_inspection_never_become_ready() {
    let (_d, path, roots) = fixture();
    assert_eq!(
        planned(None, &path, BASE, "cemu:synthetic").summary.blocked,
        1
    );
    let absent = inspect_cemu_readiness(&path.join("absent"), None, &roots);
    assert!(!absent.refusals.is_empty());
    assert!(absent.profiles.iter().all(|p| p.command.is_none()));
}
#[test]
fn conflicting_versions_do_not_select_a_winner() {
    let d = tempdir().unwrap();
    let mut projections = Vec::new();
    for version in ["16", "32"] {
        let path = d.path().join(version);
        title(&path, "0005000E10101A00", version, false);
        projections.push(content::project_wiiu_content_readiness(
            &path,
            WiiUKeyStoreEvidence::NotChecked,
            Some(BASE),
        ));
    }
    assert!(
        related_content_refusals(&projections)
            .iter()
            .any(|s| s.contains("versions"))
    );
}
#[test]
fn malformed_duplicate_and_commented_identity_never_authorize_launch() {
    let (_d, path, roots) = fixture();
    for xml in [
        "<menu><title_id>0005000010101A00</title_id>",
        "<menu><title_id>0005000010101A00</title_id><title_id>0005000099999999</title_id></menu>",
        "<menu><!-- <title_id>0005000010101A00</title_id> --></menu>",
        "<menu><title_id>0005000010101A00</title_id><title_id/></menu>",
        "<menu><title_id>0005000010101A00</title_id></menu><extra/>",
        "<menu><title_id>00050000&unknown;10101A00</title_id></menu>",
    ] {
        fs::write(path.join("meta/meta.xml"), xml).unwrap();
        let inspection = inspect_cemu_readiness(&path, None, &roots);
        assert!(!inspection.refusals.is_empty());
        assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
    }
}
#[test]
fn missing_or_ambiguous_rpx_and_missing_identity_are_refused() {
    let (_d, path, roots) = fixture();
    fs::write(path.join("code/other.rpx"), b"rpx").unwrap();
    assert!(
        inspect_cemu_readiness(&path, None, &roots)
            .profiles
            .iter()
            .all(|p| p.command.is_none())
    );
    fs::remove_file(path.join("code/other.rpx")).unwrap();
    fs::remove_file(path.join("meta/meta.xml")).unwrap();
    assert!(
        inspect_cemu_readiness(&path, None, &roots)
            .profiles
            .iter()
            .all(|p| p.command.is_none())
    );
}
#[test]
fn unusable_or_missing_cemu_cannot_produce_a_preview_command() {
    let (_d, path, roots) = fixture();
    fs::remove_file(&roots.explicit_executables[0]).unwrap();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
}
#[test]
fn changed_nested_file_refuses_preview_execution() {
    let (_d, path, roots) = fixture();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    let id = inspection
        .profiles
        .iter()
        .find(|p| p.command.is_some())
        .unwrap()
        .profile_id
        .clone();
    fs::write(path.join("code/game.rpx"), b"changed-rpx").unwrap();
    assert!(inspection.stamp.is_stale());
    assert!(launch_previewed_cemu(&inspection, &id, &roots).is_err());
}
#[test]
fn changed_mlc_metadata_refuses_preview_execution() {
    let (d, path, roots) = fixture();
    let addon = d.path().join("mlc/usr/title/0005000e/10101a00");
    title(&addon, "0005000E10101A00", "16", false);
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    let id = inspection
        .profiles
        .iter()
        .find(|p| p.command.is_some())
        .unwrap()
        .profile_id
        .clone();
    title(&addon, "0005000E10101A00", "32", false);
    assert!(launch_previewed_cemu(&inspection, &id, &roots).is_err());
}
#[cfg(unix)]
#[test]
fn symlinked_code_and_same_size_replacement_are_refused() {
    let (d, path, roots) = fixture();
    let stamp = WiiUContentStamp::capture(&path);
    let rpx = path.join("code/game.rpx");
    let tmp = d.path().join("replacement");
    fs::write(&tmp, fs::read(&rpx).unwrap()).unwrap();
    fs::rename(&tmp, &rpx).unwrap();
    assert!(stamp.is_stale());
    let code = path.join("code");
    fs::rename(&code, path.join("original-code")).unwrap();
    std::os::unix::fs::symlink(path.join("original-code"), &code).unwrap();
    assert!(
        inspect_cemu_readiness(&path, None, &roots)
            .profiles
            .iter()
            .all(|p| p.command.is_none())
    );
}

#[test]
fn swapped_update_dlc_slot_is_refused() {
    let (d, path, roots) = fixture();
    title(
        &d.path().join("mlc/usr/title/0005000e/10101a00"),
        "0005000C10101A00",
        "16",
        false,
    );
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
}
#[test]
fn encrypted_nus_addon_is_not_treated_as_installed_mlc_content() {
    let (d, path, roots) = fixture();
    let addon = d.path().join("mlc/usr/title/0005000e/10101a00");
    fs::create_dir_all(&addon).unwrap();
    for (name, offset) in [("title.tmd", 0x18c), ("title.tik", 0x1dc)] {
        let mut bytes = vec![0u8; offset + 8];
        bytes[..4].copy_from_slice(&0x00010004u32.to_be_bytes());
        bytes[offset..].copy_from_slice(&0x0005000E10101A00u64.to_be_bytes());
        fs::write(addon.join(name), bytes).unwrap();
    }
    fs::write(addon.join("title.cert"), b"cert").unwrap();
    fs::write(addon.join("00000000.app"), b"encrypted").unwrap();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
}
#[test]
fn changed_addon_payload_refuses_preview_execution() {
    let (d, path, roots) = fixture();
    let addon = d.path().join("mlc/usr/title/0005000e/10101a00");
    title(&addon, "0005000E10101A00", "16", false);
    fs::write(addon.join("content/data.bin"), b"first").unwrap();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    let id = inspection
        .profiles
        .iter()
        .find(|p| p.command.is_some())
        .unwrap()
        .profile_id
        .clone();
    fs::write(addon.join("content/data.bin"), b"replacement").unwrap();
    assert!(launch_previewed_cemu(&inspection, &id, &roots).is_err());
}
#[test]
fn changed_executable_refuses_preview_execution() {
    let (_d, path, roots) = fixture();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    let id = inspection
        .profiles
        .iter()
        .find(|p| p.command.is_some())
        .unwrap()
        .profile_id
        .clone();
    fs::write(&roots.explicit_executables[0], b"#!/bin/sh\nexit 42\n").unwrap();
    assert!(launch_previewed_cemu(&inspection, &id, &roots).is_err());
}
#[test]
fn trimmed_base_title_id_is_safe_and_version_is_validated() {
    let (d, path, roots) = fixture();
    title(
        &d.path().join("mlc/usr/title/0005000e/10101a00"),
        "0005000E10101A00",
        "16",
        false,
    );
    assert_eq!(
        inspect_related_content(&d.path().join("mlc"), "\u{2003}0005000010101A00\u{2003}").len(),
        1
    );
    title(&path, BASE, "not-a-version", true);
    assert!(
        inspect_cemu_readiness(&path, None, &roots)
            .profiles
            .iter()
            .all(|p| p.command.is_none())
    );
}

#[cfg(unix)]
#[test]
fn non_executable_cemu_is_refused_without_mutating_profile() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, path, roots) = fixture();
    let config = roots.explicit_configuration_roots[0].join("settings.xml");
    let before = fs::read(&config).unwrap();
    fs::set_permissions(
        &roots.explicit_executables[0],
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
    assert_eq!(fs::read(config).unwrap(), before);
}
#[test]
fn deep_payload_exceeds_freshness_bound_and_is_refused() {
    let (_d, path, roots) = fixture();
    let mut deep = path.join("content");
    for _ in 0..33 {
        deep = deep.join("nested");
    }
    fs::create_dir_all(deep).unwrap();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    assert!(inspection.stamp.is_stale());
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
}
#[test]
fn setup_refusal_explains_missing_mlc_instead_of_only_counting_blockers() {
    let (_d, path, roots) = fixture();
    fs::write(
        roots.explicit_configuration_roots[0].join("settings.xml"),
        "<content/>",
    )
    .unwrap();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
    assert!(inspection.profiles.iter().any(|p| {
        p.refusals
            .iter()
            .any(|s| s.contains("CemuMlcUnavailable") && s.contains("NotConfigured"))
    }));
}
#[test]
fn competing_metadata_files_are_ambiguous_instead_of_directory_order_dependent() {
    let (_d, path, roots) = fixture();
    fs::write(
        path.join("meta/Meta.xml"),
        "<menu><title_id>0005000099999999</title_id></menu>",
    )
    .unwrap();
    let inspection = inspect_cemu_readiness(&path, None, &roots);
    assert!(
        inspection
            .refusals
            .iter()
            .any(|r| r.contains("ConflictingMetadata"))
    );
    assert!(inspection.profiles.iter().all(|p| p.command.is_none()));
}
