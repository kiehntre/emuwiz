use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::launch::planning::{
    CandidatePreference, CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind,
    LaunchContentRef, ResolvedIdentity, build_launch_plan,
};
use crate::patch_manager::discover_dolphin_local_profiles;

fn executable(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"#!/bin/false\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A private HOME holding a Dolphin AppImage with its own `User/` root, and
/// a native XDG `Dolphin.ini` left behind with no native Dolphin program.
fn appimage_plus_orphaned_native(home: &Path) -> (DolphinLocalDiscoveryRoots, PathBuf) {
    let config = home.join(".config");
    let data = home.join(".local/share");
    fs::create_dir_all(config.join("dolphin-emu")).unwrap();
    fs::create_dir_all(data.join("dolphin-emu")).unwrap();
    fs::write(config.join("dolphin-emu/Dolphin.ini"), b"[Core]\n").unwrap();
    let image = home.join("Applications/Dolphin/Dolphin.AppImage");
    executable(&image);
    fs::write(image.parent().unwrap().join("portable.txt"), b"").unwrap();
    fs::create_dir_all(image.parent().unwrap().join("User/Config")).unwrap();
    fs::write(
        image.parent().unwrap().join("User/Config/Dolphin.ini"),
        b"[Core]\n",
    )
    .unwrap();
    let roots = DolphinLocalDiscoveryRoots {
        home: home.to_path_buf(),
        xdg_config_home: config,
        xdg_data_home: data,
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
        dolphin_emu_userpath_override: None,
        path_override: Some(Vec::new()),
    };
    (roots, image)
}

fn plan(inputs: &[crate::launch::planning::StandaloneProfileInput]) -> LaunchPlan {
    build_launch_plan(
        &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "GameCube".into(),
            game_key: "GZCE51".into(),
        }),
        &LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some("/games/ZooCube (USA).rvz".into()),
            requires_mount: false,
            provenance: "synthetic fixture".into(),
        },
        inputs,
        &RetroArchEnvironmentReport {
            format_version: 1,
            profiles: Vec::new(),
            diagnostics: Vec::new(),
        },
        &[],
    )
}

#[test]
fn an_orphaned_native_dolphin_ini_used_to_tie_with_the_working_appimage() {
    let home = tempfile::tempdir().unwrap();
    let (roots, _) = appimage_plus_orphaned_native(home.path());
    let discovery = discover_dolphin_local_profiles(&roots);
    // Both profiles are "eligible" by discovery alone: the old projection.
    let naive: Vec<_> = discovery
        .profiles
        .iter()
        .map(|profile| crate::launch::planning::StandaloneProfileInput {
            adapter_id: "dolphin",
            profile_id: profile.profile_id.clone(),
            profile_path: Some(profile.configuration_root.clone()),
            eligible: profile.eligible,
            firmware: FirmwareReadiness::NotRequired,
        })
        .collect();
    assert_eq!(naive.iter().filter(|input| input.eligible).count(), 2);
    let tied = plan(&naive);
    // (The absent Flatpak profile is a third, already-blocked candidate.)
    let unblocked: Vec<_> = tied
        .candidates
        .iter()
        .filter(|c| c.readiness != LaunchReadiness::Blocked)
        .collect();
    assert_eq!(unblocked.len(), 2);
    assert!(
        unblocked
            .iter()
            .all(|c| c.readiness == LaunchReadiness::ReadyWithWarnings),
        "the unusable native profile hid the working AppImage's Launch action"
    );
}

#[test]
fn a_profile_without_a_program_is_blocked_with_its_real_reason_and_never_ties() {
    let home = tempfile::tempdir().unwrap();
    let (roots, _) = appimage_plus_orphaned_native(home.path());
    let discovery = discover_dolphin_local_profiles(&roots);
    let profiles = dolphin_launch_profiles(&discovery, &roots);
    let native_id = format!(
        "dolphin:{}",
        roots.xdg_config_home.join("dolphin-emu").display()
    );
    assert_eq!(profiles.unavailable.len(), 1);
    assert_eq!(profiles.unavailable[0].0, native_id);
    assert_eq!(
        profiles.unavailable[0].1.kind,
        DolphinLaunchBlockerKind::ExecutableMissing
    );

    let mut plan = plan(&profiles.inputs);
    apply(&mut plan, &profiles);
    let native = plan
        .candidates
        .iter()
        .find(|c| matches!(&c.target, LaunchTarget::Standalone { profile_id, .. } if *profile_id == native_id))
        .unwrap();
    assert_eq!(native.readiness, LaunchReadiness::Blocked);
    assert_eq!(native.blockers.len(), 1, "{:?}", native.blockers);
    assert_eq!(
        native.blockers[0].kind,
        LaunchBlockerKind::DolphinBindingUnavailable
    );
    assert!(
        native.blockers[0]
            .detail
            .starts_with("No Dolphin program was found for this Dolphin settings folder."),
        "{}",
        native.blockers[0].detail
    );

    let appimage = plan
        .candidates
        .iter()
        .find(|c| matches!(&c.target, LaunchTarget::Standalone { profile_id, .. } if profile_id.starts_with("dolphin:appimage:")))
        .unwrap();
    assert_eq!(appimage.readiness, LaunchReadiness::Ready);
    assert_eq!(appimage.preference, CandidatePreference::SoleEligible);
    assert!(appimage.warnings.is_empty() && appimage.blockers.is_empty());
    // Ready: the AppImage. Blocked: the program-less native profile and the
    // absent Flatpak profile.
    assert_eq!((plan.summary.ready, plan.summary.blocked), (1, 2));
}

#[test]
fn a_profile_ineligible_by_discovery_keeps_its_own_blocker() {
    let home = tempfile::tempdir().unwrap();
    let (roots, image) = appimage_plus_orphaned_native(home.path());
    fs::remove_file(roots.xdg_config_home.join("dolphin-emu/Dolphin.ini")).unwrap();
    fs::remove_file(image).unwrap();
    let discovery = discover_dolphin_local_profiles(&roots);
    let profiles = dolphin_launch_profiles(&discovery, &roots);
    assert!(profiles.unavailable.is_empty());
    let mut plan = plan(&profiles.inputs);
    let before = plan.clone();
    apply(&mut plan, &profiles);
    assert_eq!(plan, before);
    assert!(plan.candidates.iter().all(|c| {
        c.readiness == LaunchReadiness::Blocked
            && c.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::ProfileIneligible)
    }));
}
