//! Native / AppImage / Flatpak installs on a fixture that mirrors the layout
//! found on the development machine (portable and plain AppImages, Flatpaks
//! with a `flatpak` program, a Flatpak wrapper script). Everything is a
//! temp directory; nothing is executed.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::installation::{InstallationKind, LaunchInstallation};
use super::installation_support::{AssessmentRoots, LaunchAssessment, LaunchSupport, assess};
use crate::emulator_lifecycle::{
    ExactBinding, FlatpakInstallation, LifecycleContext, LifecycleState,
    inspect_all_emulator_lifecycles,
};
use crate::launch::installation_known::KnownInstallRoots;
use crate::patch_manager::{
    DuckStationInstallationType, DuckStationProfileDiscoveryRoots, MelonDsProfileDiscoveryRoots,
    Pcsx2InstallationType, Pcsx2ProfileDiscoveryRoots, PpssppProfileDiscoveryRoots,
    discover_duckstation_profiles, discover_pcsx2_profiles, discover_ppsspp_profiles,
    resolve_duckstation_native_launch_binding, resolve_pcsx2_native_launch_binding,
    resolve_ppsspp_native_launch_binding,
};

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn exe(path: &Path) {
    write(path, b"#!/bin/sh\n");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn flatpak_app(home: &Path, id: &str) {
    write(
        &home
            .join(".local/share/flatpak/app")
            .join(id)
            .join("current/active/metadata"),
        b"[Application]\n",
    );
}

struct Machine {
    dir: tempfile::TempDir,
    home: PathBuf,
    roots: AssessmentRoots,
}

/// DuckStation: portable AppImage + plain AppImage, no native.
/// PCSX2: portable AppImage + Flatpak. PPSSPP: Flatpak + wrapper script.
/// melonDS: AppImage. A `flatpak` program is on PATH.
fn machine() -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let bin = dir.path().join("bin");
    exe(&bin.join("flatpak"));
    // DuckStation
    exe(&home.join("Applications/DuckStation/DuckStation.AppImage"));
    write(&home.join("Applications/DuckStation/portable.txt"), b"");
    write(
        &home.join("Applications/DuckStation/settings.ini"),
        b"[Main]\n",
    );
    exe(&home.join("Applications/emulators/DuckStation.AppImage"));
    write(
        &home.join(".local/share/duckstation/settings.ini"),
        b"[Main]\n",
    );
    // PCSX2
    exe(&home.join("Applications/PCSX2/PCSX2.AppImage"));
    write(&home.join("Applications/PCSX2/portable.ini"), b"");
    write(&home.join("Applications/PCSX2/inis/PCSX2.ini"), b"[UI]\n");
    flatpak_app(&home, "net.pcsx2.PCSX2");
    write(
        &home.join(".var/app/net.pcsx2.PCSX2/config/PCSX2/inis/PCSX2.ini"),
        b"[UI]\n",
    );
    // PPSSPP
    flatpak_app(&home, "org.ppsspp.PPSSPP");
    write(
        &home.join(".var/app/org.ppsspp.PPSSPP/config/ppsspp/PSP/SYSTEM/ppsspp.ini"),
        b"[General]\n",
    );
    write(
        &home.join(".local/bin/PPSSPPSDL"),
        b"#!/usr/bin/env bash\nexec flatpak run \"org.ppsspp.PPSSPP\" \"$@\"\n",
    );
    fs::set_permissions(
        home.join(".local/bin/PPSSPPSDL"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    // melonDS
    exe(&home.join("Applications/melonDS/melonDS-x86_64.AppImage"));
    write(
        &home.join(".config/melonDS/melonDS.toml"),
        b"Emu.DirectBoot = true\n",
    );
    // Dolphin: AppImage user root and Flatpak config, with no native binary.
    let dolphin = "org.DolphinEmu.dolphin-emu";
    flatpak_app(&home, dolphin);
    write(
        &home.join(format!(".var/app/{dolphin}/config/dolphin-emu/Dolphin.ini")),
        b"[Core]\n",
    );
    exe(&home.join("Applications/Dolphin/Dolphin.AppImage"));
    write(
        &home.join("Applications/Dolphin/User/Config/Dolphin.ini"),
        b"[Core]\n",
    );
    // RPCS3: AppImage and Flatpak share the official XDG config, but retain
    // separate profile identities so the Doctor result names the executable kind.
    let rpcs3 = "net.rpcs3.RPCS3";
    flatpak_app(&home, rpcs3);
    write(
        &home.join(format!(".var/app/{rpcs3}/config/rpcs3/config.yml")),
        b"Core:\n",
    );
    write(&home.join(".config/rpcs3/config.yml"), b"Core:\n");
    fs::create_dir_all(home.join(".config/rpcs3/dev_hdd0")).unwrap();
    exe(&home.join("Applications/RPCS3/RPCS3.AppImage"));

    let user_data = home.join(".local/share");
    let known = KnownInstallRoots {
        home: home.clone(),
        user_data: user_data.clone(),
        system_data: dir.path().join("var/lib"),
        path_dirs: vec![bin.clone(), home.join(".local/bin")],
    };
    let portable = |name: &str| vec![home.join("Applications").join(name)];
    let roots = AssessmentRoots {
        known,
        pcsx2: Pcsx2ProfileDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home.join(".config"),
            xdg_data_home: user_data.clone(),
            documents_home: home.join("Documents"),
            flatpak_system_root: dir.path().join("var/lib/flatpak"),
            appimage_directory: None,
            portable_configuration_roots: portable("PCSX2"),
            explicit_executables: Vec::new(),
            path_override: Some(vec![bin.clone(), home.join(".local/bin")]),
        },
        duckstation: DuckStationProfileDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home.join(".config"),
            xdg_data_home: user_data.clone(),
            xdg_config_home_explicit: false,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: portable("DuckStation"),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            path_override: Some(vec![bin.clone(), home.join(".local/bin")]),
        },
        ppsspp: PpssppProfileDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home.join(".config"),
            xdg_data_home: user_data.clone(),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            path_override: Some(vec![bin.clone(), home.join(".local/bin")]),
        },
        melonds: MelonDsProfileDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home.join(".config"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        },
        dolphin: crate::patch_manager::DolphinLocalDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home.join(".config"),
            xdg_data_home: user_data.clone(),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            dolphin_emu_userpath_override: None,
            path_override: Some(vec![bin.clone(), home.join(".local/bin")]),
        },
        rpcs3: crate::patch_manager::Rpcs3ProfileDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home.join(".config"),
            xdg_data_home: user_data,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            path_override: Some(vec![bin, home.join(".local/bin")]),
        },
    };
    Machine { dir, home, roots }
}

fn appimage(path: PathBuf) -> ExactBinding {
    ExactBinding::PortableExecutable { path }
}

fn flatpak(id: &str) -> ExactBinding {
    ExactBinding::FlatpakApp { app_id: id.into() }
}

fn launchable(kind: InstallationKind) -> LaunchSupport {
    LaunchSupport::Launchable { kind }
}

#[test]
fn every_installed_package_kind_on_the_machine_is_launchable() {
    let m = machine();
    let a = assess(&m.roots);
    let apps = m.home.join("Applications");
    let cases = [
        (
            "DuckStation",
            appimage(apps.join("DuckStation/DuckStation.AppImage")),
            InstallationKind::AppImage,
        ),
        (
            "DuckStation",
            appimage(apps.join("emulators/DuckStation.AppImage")),
            InstallationKind::AppImage,
        ),
        (
            "PCSX2",
            appimage(apps.join("PCSX2/PCSX2.AppImage")),
            InstallationKind::AppImage,
        ),
        (
            "PCSX2",
            flatpak("net.pcsx2.PCSX2"),
            InstallationKind::Flatpak,
        ),
        (
            "PPSSPP",
            flatpak("org.ppsspp.PPSSPP"),
            InstallationKind::Flatpak,
        ),
        (
            "melonDS",
            appimage(apps.join("melonDS/melonDS-x86_64.AppImage")),
            InstallationKind::AppImage,
        ),
        (
            "Dolphin",
            appimage(apps.join("Dolphin/Dolphin.AppImage")),
            InstallationKind::AppImage,
        ),
        (
            "Dolphin",
            flatpak("org.DolphinEmu.dolphin-emu"),
            InstallationKind::Flatpak,
        ),
        (
            "RPCS3",
            appimage(apps.join("RPCS3/RPCS3.AppImage")),
            InstallationKind::AppImage,
        ),
        (
            "RPCS3",
            flatpak("net.rpcs3.RPCS3"),
            InstallationKind::Flatpak,
        ),
    ];
    for (id, binding, kind) in cases {
        assert_eq!(
            a.support_for(id, &binding),
            launchable(kind),
            "{id} {binding:?}"
        );
    }
}

#[test]
fn portable_appimages_bind_to_their_own_directory_and_plain_ones_to_the_default_profile() {
    let m = machine();
    let apps = m.home.join("Applications");
    let roots = &m.roots.duckstation;
    let profiles = discover_duckstation_profiles(roots).profiles;
    let portable = profiles
        .iter()
        .find(|p| p.installation_type == DuckStationInstallationType::Portable && p.eligible)
        .unwrap();
    let binding = resolve_duckstation_native_launch_binding(portable, roots).unwrap();
    assert_eq!(
        binding.executable,
        apps.join("DuckStation/DuckStation.AppImage")
    );
    assert_eq!(
        binding.installation,
        LaunchInstallation::AppImage {
            extract_and_run: false
        }
    );
    let native = profiles
        .iter()
        .find(|p| p.installation_type == DuckStationInstallationType::Native && p.eligible)
        .unwrap();
    let binding = resolve_duckstation_native_launch_binding(native, roots).unwrap();
    assert_eq!(
        binding.executable,
        apps.join("emulators/DuckStation.AppImage")
    );
    assert_eq!(
        binding.installation,
        LaunchInstallation::AppImage {
            extract_and_run: false
        }
    );
}

#[test]
fn pcsx2_flatpak_and_portable_appimage_each_get_their_own_binding() {
    let m = machine();
    let roots = &m.roots.pcsx2;
    let profiles = discover_pcsx2_profiles(roots).unwrap().profiles;
    let flatpak_profile = profiles
        .iter()
        .find(|p| p.installation_type == Pcsx2InstallationType::FlatpakUser && p.eligible)
        .unwrap();
    let binding = resolve_pcsx2_native_launch_binding(flatpak_profile, roots).unwrap();
    assert_eq!(
        binding.installation,
        LaunchInstallation::flatpak("net.pcsx2.PCSX2").unwrap()
    );
    assert!(binding.executable.ends_with("bin/flatpak"));
    let portable = profiles
        .iter()
        .find(|p| p.installation_type == Pcsx2InstallationType::Portable && p.eligible)
        .unwrap();
    let binding = resolve_pcsx2_native_launch_binding(portable, roots).unwrap();
    assert!(
        binding
            .executable
            .ends_with("Applications/PCSX2/PCSX2.AppImage")
    );
    // The portable marker is only recorded: it is still there, untouched.
    assert!(m.home.join("Applications/PCSX2/portable.ini").is_file());
}

#[test]
fn a_flatpak_wrapper_script_is_not_a_native_emulator_and_the_real_flatpak_is_bound() {
    let m = machine();
    let roots = &m.roots.ppsspp;
    let discovery = discover_ppsspp_profiles(roots);
    for profile in &discovery.profiles {
        assert!(
            profile.executable_candidates.iter().all(|c| c
                .path
                .file_name()
                .and_then(|n| n.to_str())
                != Some("PPSSPPSDL")),
            "the wrapper must not be reported as a native executable"
        );
    }
    let profile = discovery
        .profiles
        .iter()
        .find(|p| p.eligible && p.configuration_path.ends_with("config/ppsspp"))
        .unwrap();
    let binding = resolve_ppsspp_native_launch_binding(profile).unwrap();
    assert_eq!(
        binding.installation,
        LaunchInstallation::flatpak("org.ppsspp.PPSSPP").unwrap()
    );
}

#[test]
fn a_detected_but_unbindable_install_is_distinct_from_ready_and_from_missing() {
    let m = machine();
    // A third PCSX2 AppImage with no portable marker and no default profile.
    exe(&m.home.join("Applications/emulators/PCSX2-v9.AppImage"));
    let a = assess(&m.roots);
    let binding = appimage(m.home.join("Applications/emulators/PCSX2-v9.AppImage"));
    match a.support_for("PCSX2", &binding) {
        LaunchSupport::NotLaunchableYet { .. } => {}
        other => panic!("expected NotLaunchableYet, got {other:?}"),
    }
    assert!(
        a.support_for("PCSX2", &binding)
            .label()
            .starts_with("Installed, but this installation type is not launchable yet")
    );
    // An emulator never assessed is neither ready nor claimed missing.
    assert_eq!(
        LaunchAssessment::default().support_for("PCSX2", &binding),
        LaunchSupport::NotAssessed
    );
}

#[test]
fn doctor_lifecycle_lists_the_installs_the_launcher_can_start() {
    let m = machine();
    let assessment = assess(&m.roots);
    let mut context = LifecycleContext::default();
    context.launch_assessment = assessment;
    context.flatpak_installations = vec![
        FlatpakInstallation {
            app_id: "org.ppsspp.PPSSPP".into(),
            installed_ref: None,
            version: Some("1.20.4".into()),
            branch: Some("stable".into()),
            scope: Some("user".into()),
        },
        FlatpakInstallation {
            app_id: "net.pcsx2.PCSX2".into(),
            installed_ref: None,
            version: Some("v2.8.2".into()),
            branch: Some("stable".into()),
            scope: Some("user".into()),
        },
    ];
    let projections = inspect_all_emulator_lifecycles(&context);
    let by = |id: &str| projections.iter().find(|p| p.emulator_id == id).unwrap();
    // melonDS: an AppImage only; installed, version unknown, launchable.
    let melon = by("melonDS");
    assert_eq!(melon.state, LifecycleState::InstalledUnknownVersion);
    assert_eq!(melon.installations.len(), 1);
    assert_eq!(
        melon.installations[0].launch_support,
        launchable(InstallationKind::AppImage)
    );
    // DuckStation: two AppImages, both launchable, none silently chosen.
    let duck = by("DuckStation");
    assert_eq!(duck.state, LifecycleState::MultipleInstallations);
    assert!(
        duck.installations
            .iter()
            .all(|i| i.launch_support == launchable(InstallationKind::AppImage))
    );
    assert!(duck.selected.is_none());
    // PCSX2: portable AppImage + Flatpak.
    let pcsx2 = by("PCSX2");
    assert_eq!(pcsx2.installations.len(), 2);
    assert!(
        pcsx2
            .installations
            .iter()
            .all(|i| matches!(i.launch_support, LaunchSupport::Launchable { .. }))
    );
    // PPSSPP Flatpak is launchable and no longer "Missing".
    let ppsspp = by("PPSSPP");
    assert_ne!(ppsspp.state, LifecycleState::Missing);
    assert_eq!(
        ppsspp.installations[0].launch_support,
        launchable(InstallationKind::Flatpak)
    );
    let _ = m.dir.path();
}

#[test]
fn an_explicit_selection_wins_over_newly_discovered_installs() {
    let m = machine();
    let chosen = appimage(m.home.join("Applications/emulators/DuckStation.AppImage"));
    let mut context = LifecycleContext::default();
    context.launch_assessment = assess(&m.roots);
    context
        .selected_bindings
        .insert("DuckStation".into(), chosen.clone());
    let projections = inspect_all_emulator_lifecycles(&context);
    let duck = projections
        .iter()
        .find(|p| p.emulator_id == "DuckStation")
        .unwrap();
    assert_eq!(duck.selected, Some(chosen));
}
