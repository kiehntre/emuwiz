use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use super::*;

fn roots(dir: &Path) -> KnownInstallRoots {
    KnownInstallRoots {
        home: dir.join("home"),
        user_data: dir.join("home/.local/share"),
        system_data: dir.join("var/lib"),
        path_dirs: vec![dir.join("bin")],
    }
}

fn exe(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn known_appimages_are_found_in_the_fixed_places_with_portable_markers_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let r = roots(dir.path());
    let duck_portable = r.home.join("Applications/DuckStation/DuckStation.AppImage");
    let duck_plain = r.home.join("Applications/emulators/DuckStation.AppImage");
    exe(&duck_portable);
    exe(&duck_plain);
    fs::write(duck_portable.with_file_name("portable.txt"), b"").unwrap();
    exe(&r.home.join("Applications/Blender.AppImage"));
    exe(&r.home.join("Applications/melonDS/melonDS-x86_64.AppImage"));
    let found = discover_appimages(&DUCKSTATION, &r);
    assert_eq!(found.len(), 2, "{found:?}");
    let portable = found.iter().find(|f| f.path == duck_portable).unwrap();
    assert!(portable.portable_marker.is_some());
    assert!(
        found
            .iter()
            .find(|f| f.path == duck_plain)
            .unwrap()
            .portable_marker
            .is_none()
    );
    let melon = discover_appimages(&MELONDS, &r);
    assert_eq!(melon.len(), 1);
    assert!(melon[0].path.ends_with("melonDS-x86_64.AppImage"));
}

#[test]
fn unrelated_symlinked_and_non_executable_appimages_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let r = roots(dir.path());
    let real = dir.path().join("elsewhere/PCSX2.AppImage");
    exe(&real);
    fs::create_dir_all(r.home.join("Applications")).unwrap();
    symlink(&real, r.home.join("Applications/PCSX2.AppImage")).unwrap();
    let plain = r.home.join("Applications/PCSX2-v2.AppImage");
    fs::write(&plain, b"x").unwrap();
    assert!(discover_appimages(&PCSX2, &r).is_empty());
}

#[test]
fn flatpaks_are_found_by_exact_id_in_user_and_system_metadata_only() {
    let dir = tempfile::tempdir().unwrap();
    let r = roots(dir.path());
    let meta = |base: &Path, id: &str| {
        let m = base
            .join("flatpak/app")
            .join(id)
            .join("current/active/metadata");
        fs::create_dir_all(m.parent().unwrap()).unwrap();
        fs::write(m, b"[Application]\n").unwrap();
    };
    meta(&r.user_data, "org.ppsspp.PPSSPP");
    meta(&r.system_data, "net.pcsx2.PCSX2");
    meta(&r.user_data, "org.example.NotAnEmulator");
    assert_eq!(
        discover_flatpaks(&PPSSPP, &r),
        vec![FoundFlatpak {
            app_id: "org.ppsspp.PPSSPP".into(),
            scope: FlatpakScope::User
        }]
    );
    assert_eq!(discover_flatpaks(&PCSX2, &r)[0].scope, FlatpakScope::System);
    assert!(discover_flatpaks(&DUCKSTATION, &r).is_empty());
}

#[test]
fn flatpak_launch_needs_both_the_app_and_the_flatpak_program() {
    let dir = tempfile::tempdir().unwrap();
    let r = roots(dir.path());
    assert_eq!(
        resolve_flatpak_launch("org.ppsspp.PPSSPP", &r).unwrap_err(),
        "Flatpak application is not installed"
    );
    let m = r
        .user_data
        .join("flatpak/app/org.ppsspp.PPSSPP/current/active/metadata");
    fs::create_dir_all(m.parent().unwrap()).unwrap();
    fs::write(m, b"x").unwrap();
    assert_eq!(
        resolve_flatpak_launch("org.ppsspp.PPSSPP", &r).unwrap_err(),
        "the flatpak program was not found on PATH"
    );
    exe(&dir.path().join("bin/flatpak"));
    let (binary, installation) = resolve_flatpak_launch("org.ppsspp.PPSSPP", &r).unwrap();
    assert!(binary.ends_with("bin/flatpak"));
    assert_eq!(
        installation,
        LaunchInstallation::flatpak("org.ppsspp.PPSSPP").unwrap()
    );
    // An id EmuWiz does not know is never accepted, even if "installed".
    assert!(resolve_flatpak_launch("org.example.NotAnEmulator", &r).is_err());
}

#[test]
fn appimage_validation_reports_each_failure() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("A B/Good.AppImage");
    exe(&good);
    assert!(validate_appimage(&good).is_ok());
    assert_eq!(
        validate_appimage(&dir.path().join("none.AppImage")),
        Err("AppImage does not exist")
    );
    assert_eq!(
        validate_appimage(Path::new("rel.AppImage")),
        Err("AppImage path is not absolute")
    );
    let plain = dir.path().join("plain.AppImage");
    fs::write(&plain, b"x").unwrap();
    assert_eq!(validate_appimage(&plain), Err("AppImage is not executable"));
    let link = dir.path().join("link.AppImage");
    symlink(&good, &link).unwrap();
    assert_eq!(validate_appimage(&link), Err("AppImage is a symlink"));
    assert_eq!(
        validate_appimage(dir.path()),
        Err("AppImage is not a regular file")
    );
}
