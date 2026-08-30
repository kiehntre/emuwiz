use super::*;
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use crate::launch::readiness::LaunchReadiness;
use std::path::PathBuf;

// --- fixtures ----------------------------------------------------------

fn dos_identity() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "DOS".into(),
        game_key: "dat:some-hash".into(),
    })
}

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "archivefs-dosbox-cmd-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Writes an executable-bit file at `path`.
fn write_executable(path: &Path) {
    std::fs::write(path, b"#!/bin/sh\nexit 0\n").expect("write exe");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

fn ok_binding(variant: DosBoxVariant) -> Result<DosBoxNativeLaunchBinding, DosBoxBindingRefusal> {
    Ok(DosBoxNativeLaunchBinding {
        executable: PathBuf::from(match variant {
            DosBoxVariant::Classic => "/usr/games/dosbox",
            DosBoxVariant::Staging => "/usr/bin/dosbox-staging",
        }),
        variant,
    })
}

fn verified_config(path: &str) -> DosBoxConfigStatus {
    DosBoxConfigStatus::Verified {
        config_path: PathBuf::from(path),
        autoexec_command_lines: 3,
    }
}

// --- positive (section 11) -------------------------------------------------

#[test]
fn classic_dosbox_executable_is_discovered_through_the_seam() {
    let dir = scratch_dir("classic-exe");
    let exe = dir.join("dosbox");
    write_executable(&exe);
    let binding = resolve_dosbox_native_launch_binding_at(&exe, DosBoxVariant::Classic).unwrap();
    assert_eq!(binding.variant, DosBoxVariant::Classic);
    assert_eq!(binding.executable, exe);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dosbox_staging_executable_is_discovered_through_the_seam() {
    let dir = scratch_dir("staging-exe");
    let exe = dir.join("dosbox-staging");
    write_executable(&exe);
    let binding = resolve_dosbox_native_launch_binding_at(&exe, DosBoxVariant::Staging).unwrap();
    assert_eq!(binding.variant, DosBoxVariant::Staging);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_verified_dosbox_conf_is_discovered_in_a_game_directory() {
    let dir = scratch_dir("cfg-verified");
    std::fs::write(
        dir.join("dosbox.conf"),
        b"[dosbox]\nmemsize=16\n[autoexec]\nmount c .\nc:\ngame.exe\n",
    )
    .unwrap();
    let status = discover_dosbox_config(&dir, &crate::safe_read::TrustedRoots::none());
    match status {
        DosBoxConfigStatus::Verified {
            autoexec_command_lines,
            ..
        } => assert_eq!(autoexec_command_lines, 3),
        other => panic!("expected Verified, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ready_when_identity_dos_plus_verified_config_plus_binding() {
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        Path::new("/games/Keen"),
        &verified_config("/games/Keen/dosbox.conf"),
        &ok_binding(DosBoxVariant::Staging),
    );
    assert!(plan.blockers.is_empty());
    assert_eq!(plan.readiness(), LaunchReadiness::Ready);
    let command = plan.command.expect("command");
    assert_eq!(command.executable, PathBuf::from("/usr/bin/dosbox-staging"));
    assert_eq!(command.arguments, vec!["-conf", "/games/Keen/dosbox.conf"]);
    assert_eq!(
        command.working_directory,
        Some(PathBuf::from("/games/Keen"))
    );
    assert_eq!(command.selection.platform_id, "DOS");
    assert_eq!(command.selection.variant, DosBoxVariant::Staging);
}

#[test]
fn argv_construction_is_deterministic_and_uses_separate_components() {
    let build = || {
        build_dosbox_command_plan(
            &dos_identity(),
            Path::new("/g/x"),
            &verified_config("/g/x/dosbox.conf"),
            &ok_binding(DosBoxVariant::Classic),
        )
    };
    assert_eq!(build(), build());
    let command = build().command.unwrap();
    assert_eq!(command.arguments.len(), 2);
    assert_eq!(command.arguments[0], "-conf");
    assert_eq!(command.arguments[1], "/g/x/dosbox.conf");
}

#[test]
fn a_config_path_with_spaces_is_one_argv_component() {
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        Path::new("/games/Some DOS Game (1993)"),
        &verified_config("/games/Some DOS Game (1993)/dosbox.conf"),
        &ok_binding(DosBoxVariant::Classic),
    );
    let command = plan.command.unwrap();
    assert_eq!(
        command.arguments[1],
        "/games/Some DOS Game (1993)/dosbox.conf"
    );
    assert_eq!(command.arguments.len(), 2);
}

#[test]
fn variant_ids_map_only_to_supported_families() {
    assert_eq!(
        dosbox_variant_from_id("dosbox"),
        Some(DosBoxVariant::Classic)
    );
    assert_eq!(
        dosbox_variant_from_id("DOSBox-Staging"),
        Some(DosBoxVariant::Staging)
    );
    assert_eq!(
        dosbox_variant_from_id("dosbox_staging"),
        Some(DosBoxVariant::Staging)
    );
    assert_eq!(dosbox_variant_from_id("dosbox-x"), None);
    assert_eq!(dosbox_variant_from_id("wine"), None);
}

// --- negative (section 12) ---------------------------------------------

#[test]
fn a_missing_executable_fails_closed() {
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        Path::new("/g/x"),
        &verified_config("/g/x/dosbox.conf"),
        &Err(DosBoxBindingRefusal::ExecutableUnavailable(
            "no DOSBox executable found".into(),
        )),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.kind == LaunchBlockerKind::DosBoxBindingUnavailable)
    );
    assert_eq!(plan.readiness(), LaunchReadiness::Blocked);
}

#[test]
fn a_fake_executable_path_is_refused_by_the_seam() {
    let refusal = resolve_dosbox_native_launch_binding_at(
        Path::new("/nope/not/dosbox"),
        DosBoxVariant::Classic,
    )
    .unwrap_err();
    assert!(matches!(
        refusal,
        DosBoxBindingRefusal::ExecutableUnavailable(_)
    ));
}

#[test]
fn a_non_executable_regular_file_is_refused_by_the_seam() {
    let dir = scratch_dir("not-exe");
    let path = dir.join("dosbox");
    std::fs::write(&path, b"just text").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(resolve_dosbox_native_launch_binding_at(&path, DosBoxVariant::Classic).is_err());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_filename_only_dosbox_conf_is_malformed_not_usable() {
    let dir = scratch_dir("cfg-nameonly");
    std::fs::write(
        dir.join("dosbox.conf"),
        b"this is just a note, not a config, no sections\n",
    )
    .unwrap();
    let status = discover_dosbox_config(&dir, &crate::safe_read::TrustedRoots::none());
    assert!(matches!(status, DosBoxConfigStatus::Malformed(_)));
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        &dir,
        &status,
        &ok_binding(DosBoxVariant::Classic),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::DosBoxConfigMalformed)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_dosbox_conf_is_malformed() {
    let dir = scratch_dir("cfg-empty");
    std::fs::write(dir.join("dosbox.conf"), b"").unwrap();
    let status = discover_dosbox_config(&dir, &crate::safe_read::TrustedRoots::none());
    assert!(matches!(status, DosBoxConfigStatus::Malformed(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_malformed_section_header_is_malformed() {
    let dir = scratch_dir("cfg-badsection");
    std::fs::write(
        dir.join("dosbox.conf"),
        b"[dosbox]\ncore=auto\n[autoexec\nmount c .\n",
    )
    .unwrap();
    let status = discover_dosbox_config(&dir, &crate::safe_read::TrustedRoots::none());
    assert!(matches!(status, DosBoxConfigStatus::Malformed(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_config_without_autoexec_blocks_with_no_autoexec() {
    let dir = scratch_dir("cfg-noauto");
    std::fs::write(
        dir.join("dosbox.conf"),
        b"[dosbox]\nmemsize=32\n[cpu]\ncycles=max\n",
    )
    .unwrap();
    let status = discover_dosbox_config(&dir, &crate::safe_read::TrustedRoots::none());
    assert_eq!(status, DosBoxConfigStatus::ValidNoAutoexec);
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        &dir,
        &status,
        &ok_binding(DosBoxVariant::Classic),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::DosBoxConfigNoAutoexec)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_config_blocks_with_config_missing() {
    let dir = scratch_dir("cfg-missing");
    let status = discover_dosbox_config(&dir, &crate::safe_read::TrustedRoots::none());
    assert_eq!(status, DosBoxConfigStatus::Missing);
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        &dir,
        &status,
        &ok_binding(DosBoxVariant::Classic),
    );
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::DosBoxConfigMissing)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mz_alone_unknown_identity_does_not_authorize_a_dosbox_launch() {
    // A bare Weak MZ signature never produces a Resolved DOS identity; it
    // reaches this planner as Unknown and must fail closed.
    let plan = build_dosbox_command_plan(
        &CanonicalIdentityStatus::Unknown,
        Path::new("/g/x"),
        &verified_config("/g/x/dosbox.conf"),
        &ok_binding(DosBoxVariant::Classic),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::IdentityUnresolved)
    );
}

#[test]
fn an_ambiguous_platform_does_not_authorize_a_dosbox_launch() {
    let plan = build_dosbox_command_plan(
        &CanonicalIdentityStatus::Conflicting,
        Path::new("/g/x"),
        &verified_config("/g/x/dosbox.conf"),
        &ok_binding(DosBoxVariant::Classic),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::IdentityConflict)
    );
}

#[test]
fn a_non_dos_resolved_identity_is_a_platform_mismatch() {
    let plan = build_dosbox_command_plan(
        &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PC".into(),
            game_key: "k".into(),
        }),
        Path::new("/g/x"),
        &verified_config("/g/x/dosbox.conf"),
        &ok_binding(DosBoxVariant::Classic),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::DosBoxPlatformMismatch)
    );
}

#[test]
fn an_executable_name_inside_autoexec_never_reaches_the_command() {
    let dir = scratch_dir("cfg-secret");
    std::fs::write(
        dir.join("dosbox.conf"),
        b"[autoexec]\nmount c .\nc:\nSECRET_LAUNCHER.EXE\ncall setup.bat\n",
    )
    .unwrap();
    let status = discover_dosbox_config(&dir, &crate::safe_read::TrustedRoots::none());
    assert!(status.is_verified());
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        &dir,
        &status,
        &ok_binding(DosBoxVariant::Classic),
    );
    let command = plan.command.expect("command");
    // The only args are `-conf <the dosbox.conf path>`; nothing from inside it.
    assert_eq!(command.arguments.len(), 2);
    assert_eq!(command.arguments[0], "-conf");
    let joined = format!("{command:?}");
    assert!(!joined.to_lowercase().contains("secret_launcher"));
    assert!(!joined.to_lowercase().contains("setup.bat"));
    assert!(!joined.contains("mount"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shell_metacharacters_in_paths_stay_literal_argv_data() {
    let evil = "/games/x; rm -rf ~ && $(touch pwned)/dosbox.conf";
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        Path::new("/games/x; rm -rf ~ && $(touch pwned)"),
        &verified_config(evil),
        &ok_binding(DosBoxVariant::Classic),
    );
    let command = plan.command.expect("command");
    // Exactly one argv slot holds the whole string, unsplit and un-evaluated.
    assert_eq!(command.arguments[1], std::ffi::OsString::from(evil));
    assert_eq!(command.selection.config_path, PathBuf::from(evil));
}

#[test]
fn an_unsupported_dosbox_variant_fails_closed() {
    let refusal = resolve_dosbox_native_launch_binding_from_id("dosbox-x").unwrap_err();
    assert_eq!(
        refusal,
        DosBoxBindingRefusal::VariantUnsupported("dosbox-x".into())
    );
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        Path::new("/g/x"),
        &verified_config("/g/x/dosbox.conf"),
        &Err(refusal),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::DosBoxVariantUnsupported)
    );
}

#[test]
fn a_relative_game_directory_is_unsupported() {
    let plan = build_dosbox_command_plan(
        &dos_identity(),
        Path::new("relative/dir"),
        &verified_config("relative/dir/dosbox.conf"),
        &ok_binding(DosBoxVariant::Classic),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::DosBoxContentUnsupported)
    );
}
