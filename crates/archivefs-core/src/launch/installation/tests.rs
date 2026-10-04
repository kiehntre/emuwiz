use std::ffi::OsString;
use std::path::Path;

use super::*;

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

#[test]
fn native_arguments_are_byte_for_byte_unchanged() {
    let args = os(&["-datapath", "/a b/c", "/games/Game One.iso"]);
    let wrapped = LaunchInstallation::Native
        .wrap_arguments(&VisibilityPlan::new(), args.clone())
        .unwrap();
    assert_eq!(wrapped, args);
}

#[test]
fn an_appimage_is_the_executable_itself_with_unchanged_arguments() {
    let args = os(&["/games/with space/Game.iso"]);
    let wrapped = LaunchInstallation::AppImage {
        extract_and_run: false,
    }
    .wrap_arguments(&VisibilityPlan::new(), args.clone())
    .unwrap();
    assert_eq!(wrapped, args);
}

#[test]
fn extract_and_run_is_explicit_and_only_a_leading_flag() {
    let wrapped = LaunchInstallation::AppImage {
        extract_and_run: true,
    }
    .wrap_arguments(&VisibilityPlan::new(), os(&["game.iso"]))
    .unwrap();
    assert_eq!(wrapped, os(&["--appimage-extract-and-run", "game.iso"]));
}

#[test]
fn flatpak_orders_run_options_app_id_then_emulator_arguments() {
    let mut visibility = VisibilityPlan::new();
    visibility.content(Path::new("/mnt/usbdrive/games/PS2/Game One.iso"));
    let wrapped = LaunchInstallation::flatpak("net.pcsx2.PCSX2")
        .unwrap()
        .wrap_arguments(
            &visibility,
            os(&["-batch", "/mnt/usbdrive/games/PS2/Game One.iso"]),
        )
        .unwrap();
    assert_eq!(
        wrapped,
        os(&[
            "run",
            "--filesystem=/mnt/usbdrive/games/PS2:ro",
            "net.pcsx2.PCSX2",
            "-batch",
            "/mnt/usbdrive/games/PS2/Game One.iso",
        ])
    );
}

#[test]
fn flatpak_content_is_read_only_by_default_and_write_must_be_asked_for() {
    let mut visibility = VisibilityPlan::new();
    visibility.content(Path::new("/mnt/usbdrive/games/a/b.cue"));
    visibility.read_write(Path::new("/srv/saves/ps1"), "explicit save folder");
    let options = visibility.flatpak_options().unwrap();
    assert_eq!(
        options,
        os(&[
            "--filesystem=/mnt/usbdrive/games/a:ro",
            "--filesystem=/srv/saves/ps1",
        ])
    );
}

#[test]
fn a_read_write_request_for_the_same_path_upgrades_it_once() {
    let mut visibility = VisibilityPlan::new();
    visibility.read_only(Path::new("/srv/x/y"), "bios");
    visibility.read_write(Path::new("/srv/x/y"), "saves");
    assert_eq!(visibility.grants().len(), 1);
    assert_eq!(
        visibility.flatpak_options().unwrap(),
        os(&["--filesystem=/srv/x/y"])
    );
}

#[test]
fn broad_or_misparsable_grants_are_refused() {
    for bad in [
        "/",
        "/mnt",
        "/home",
        "/tmp",
        "relative/dir",
        "/srv/a:b",
        "/srv/../etc",
    ] {
        let mut visibility = VisibilityPlan::new();
        visibility.read_only(Path::new(bad), "test");
        assert!(
            matches!(
                visibility.flatpak_options(),
                Err(InstallationError::UnsafeGrant { .. })
            ),
            "{bad} must be refused"
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        let mut visibility = VisibilityPlan::new();
        visibility.read_only(Path::new(&home), "test");
        assert!(
            visibility.flatpak_options().is_err(),
            "HOME must be refused"
        );
    }
}

#[test]
fn no_blanket_host_or_persistent_override_options_are_ever_produced() {
    let mut visibility = VisibilityPlan::new();
    visibility.content(Path::new("/mnt/usbdrive/games/x.iso"));
    let wrapped = LaunchInstallation::flatpak("org.ppsspp.PPSSPP")
        .unwrap()
        .wrap_arguments(&visibility, os(&["/mnt/usbdrive/games/x.iso"]))
        .unwrap();
    let text: Vec<String> = wrapped
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        text.iter()
            .all(|a| a != "override" && !a.contains("host") && !a.starts_with("--persist"))
    );
    assert_eq!(text[0], "run");
}

#[test]
fn app_ids_are_validated() {
    for good in [
        "net.pcsx2.PCSX2",
        "org.ppsspp.PPSSPP",
        "io.github.some_app-1.X",
    ] {
        assert!(valid_flatpak_app_id(good), "{good}");
    }
    for bad in [
        "",
        "pcsx2",
        "--help",
        "a..b",
        "net.pcsx2.",
        ".net.pcsx2",
        "1net.pcsx2.A",
        "a.b c",
        "a.b;ls",
    ] {
        assert!(!valid_flatpak_app_id(bad), "{bad:?}");
        assert!(LaunchInstallation::flatpak(bad).is_err());
    }
    // A hand-built invalid id still cannot be wrapped.
    let hand_built = LaunchInstallation::Flatpak {
        app_id: "--evil".into(),
    };
    assert!(
        hand_built
            .wrap_arguments(&VisibilityPlan::new(), vec![])
            .is_err()
    );
}

#[test]
fn kinds_are_reported_truthfully() {
    assert_eq!(LaunchInstallation::Native.kind(), InstallationKind::Native);
    assert_eq!(
        LaunchInstallation::AppImage {
            extract_and_run: false
        }
        .kind(),
        InstallationKind::AppImage
    );
    assert_eq!(
        LaunchInstallation::flatpak("a.b").unwrap().kind(),
        InstallationKind::Flatpak
    );
    assert_eq!(LaunchInstallation::default(), LaunchInstallation::Native);
}
