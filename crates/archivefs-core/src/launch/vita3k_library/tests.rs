use super::*;
use crate::launch::vita3k_installed::tests::{Fixture, fixture, install, sfo, wait};
use std::io::Write;

fn write_vpk(path: &Path, members: &[(&str, Vec<u8>)]) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in members {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

fn good_members(title_id: &str) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("eboot.bin", b"not a real eboot".to_vec()),
        (
            "sce_sys/param.sfo",
            sfo(&[("TITLE_ID", title_id), ("TITLE", "Synthetic")]),
        ),
    ]
}

fn vpk_in(f: &Fixture, title_id: &str) -> PathBuf {
    let path = f.dir.path().join("Game.vpk");
    write_vpk(&path, &good_members(title_id));
    path
}

fn licensed_install(f: &Fixture, id: &str) {
    install(&f.config_root, id, id, "Synthetic", "gd");
    fs::create_dir_all(f.config_root.join("ux0/license/app").join(id)).unwrap();
}

fn request(f: &Fixture, vpk: &Path, id: &str) -> Vita3kLibraryLaunchRequest {
    let evidence = inspect_vpk(vpk).unwrap();
    match resolve_vpk(&evidence, &f.roots) {
        Vita3kLibraryResolution::Installed { entry, .. } => Vita3kLibraryLaunchRequest {
            vpk: evidence,
            entry: *entry,
        },
        other => panic!("{id}: {other:?}"),
    }
}

#[test]
fn a_package_reads_only_its_title_id_and_hash() {
    let f = fixture();
    let evidence = inspect_vpk(&vpk_in(&f, "PCSE00001")).unwrap();
    assert_eq!(evidence.title_id, "PCSE00001");
    assert_eq!(evidence.display_title.as_deref(), Some("Synthetic"));
    assert_eq!(evidence.param_sha256.len(), 64);
}

#[test]
fn malformed_packages_are_refused() {
    let f = fixture();
    let p = f.dir.path().join("a.vpk");
    type Case<'a> = (&'a str, Vec<(&'a str, Vec<u8>)>, VpkRefusalKind);
    let cases: Vec<Case> = vec![
        (
            "no param",
            vec![("eboot.bin", vec![1])],
            VpkRefusalKind::MissingParam,
        ),
        (
            "no eboot",
            vec![("sce_sys/param.sfo", sfo(&[("TITLE_ID", "PCSE00001")]))],
            VpkRefusalKind::NotAPackage,
        ),
        (
            "bad sfo",
            vec![
                ("eboot.bin", vec![1]),
                ("sce_sys/param.sfo", b"junk".to_vec()),
            ],
            VpkRefusalKind::InvalidParam,
        ),
        (
            "option-like id",
            vec![
                ("eboot.bin", vec![1]),
                ("sce_sys/param.sfo", sfo(&[("TITLE_ID", "--evil")])),
            ],
            VpkRefusalKind::InvalidParam,
        ),
        (
            "nested only",
            vec![
                ("x/eboot.bin", vec![1]),
                ("x/sce_sys/param.sfo", sfo(&[("TITLE_ID", "PCSE00001")])),
            ],
            VpkRefusalKind::NotAPackage,
        ),
    ];
    for (label, members, kind) in cases {
        write_vpk(&p, &members);
        assert_eq!(inspect_vpk(&p).unwrap_err().kind, kind, "{label}");
    }
    fs::write(&p, b"not a zip").unwrap();
    assert_eq!(
        inspect_vpk(&p).unwrap_err().kind,
        VpkRefusalKind::NotAPackage
    );
    let other = f.dir.path().join("a.zip");
    fs::copy(&p, &other).unwrap();
    assert_eq!(
        inspect_vpk(&other).unwrap_err().kind,
        VpkRefusalKind::NotVpkExtension
    );
}

#[test]
fn symlinked_packages_are_refused() {
    let f = fixture();
    let real = vpk_in(&f, "PCSE00001");
    let link = f.dir.path().join("link.vpk");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(
        inspect_vpk(&link).unwrap_err().kind,
        VpkRefusalKind::NotRegularFile
    );
}

#[test]
fn resolution_reports_each_installation_state() {
    let f = fixture();
    licensed_install(&f, "PCSE00002");
    let vpk = inspect_vpk(&vpk_in(&f, "PCSE00001")).unwrap();
    assert_eq!(
        resolve_vpk(&vpk, &f.roots),
        Vita3kLibraryResolution::NotInstalled
    );
    install(&f.config_root, "PCSE00001", "PCSE00001", "Synthetic", "gd");
    assert!(matches!(
        resolve_vpk(&vpk, &f.roots),
        Vita3kLibraryResolution::Blocked(_)
    ));
    licensed_install(&f, "PCSE00001");
    assert!(matches!(
        resolve_vpk(&vpk, &f.roots),
        Vita3kLibraryResolution::Installed { .. }
    ));
}

#[test]
fn a_different_installed_param_is_reported_not_refused() {
    let f = fixture();
    let path = f.dir.path().join("u.vpk");
    write_vpk(
        &path,
        &[
            ("eboot.bin", vec![1]),
            (
                "sce_sys/param.sfo",
                sfo(&[("TITLE_ID", "PCSE00001"), ("APP_VER", "01.00")]),
            ),
        ],
    );
    licensed_install(&f, "PCSE00001");
    let evidence = inspect_vpk(&path).unwrap();
    assert!(matches!(
        resolve_vpk(&evidence, &f.roots),
        Vita3kLibraryResolution::Installed {
            param_differs: true,
            ..
        }
    ));
}

#[test]
fn a_package_for_another_title_never_selects_an_installed_one() {
    let f = fixture();
    licensed_install(&f, "PCSE00002");
    let vpk = inspect_vpk(&vpk_in(&f, "PCSE00001")).unwrap();
    assert_eq!(
        resolve_vpk(&vpk, &f.roots),
        Vita3kLibraryResolution::NotInstalled
    );
}

#[test]
fn no_installation_is_reported() {
    let f = fixture();
    let vpk = inspect_vpk(&vpk_in(&f, "PCSE00001")).unwrap();
    let mut roots = f.roots.clone();
    roots.explicit_configuration_roots.clear();
    roots.explicit_executables.clear();
    assert!(matches!(
        resolve_vpk(&vpk, &roots),
        Vita3kLibraryResolution::NoInstallation(_)
    ));
}

#[test]
fn play_starts_the_installed_title() {
    let f = fixture();
    licensed_install(&f, "PCSE00001");
    let vpk = vpk_in(&f, "PCSE00001");
    let req = request(&f, &vpk, "PCSE00001");
    assert!(wait(
        preflight_and_launch_vita3k_library(&req, &f.roots).unwrap()
    ));
}

#[test]
fn a_changed_or_replaced_package_is_refused_at_click() {
    let f = fixture();
    licensed_install(&f, "PCSE00001");
    let vpk = vpk_in(&f, "PCSE00001");
    let req = request(&f, &vpk, "PCSE00001");
    write_vpk(&vpk, &good_members("PCSE00002"));
    assert!(matches!(
        preflight_and_launch_vita3k_library(&req, &f.roots),
        Err(Vita3kLibraryLaunchError::PackageChanged(_))
    ));
    fs::remove_file(&vpk).unwrap();
    assert!(matches!(
        preflight_and_launch_vita3k_library(&req, &f.roots),
        Err(Vita3kLibraryLaunchError::Package(_))
    ));
}

#[test]
fn a_title_uninstalled_or_changed_since_the_offer_is_refused() {
    let f = fixture();
    licensed_install(&f, "PCSE00001");
    let vpk = vpk_in(&f, "PCSE00001");
    let req = request(&f, &vpk, "PCSE00001");
    fs::write(
        f.config_root.join("ux0/app/PCSE00001/sce_sys/param.sfo"),
        sfo(&[("TITLE_ID", "PCSE00001"), ("TITLE", "Other")]),
    )
    .unwrap();
    assert!(matches!(
        preflight_and_launch_vita3k_library(&req, &f.roots),
        Err(Vita3kLibraryLaunchError::Installed(
            Vita3kInstalledLaunchError::Changed(_)
        ))
    ));
    fs::remove_dir_all(f.config_root.join("ux0/app/PCSE00001")).unwrap();
    assert!(matches!(
        preflight_and_launch_vita3k_library(&req, &f.roots),
        Err(Vita3kLibraryLaunchError::Installed(
            Vita3kInstalledLaunchError::Missing(_)
        ))
    ));
}

#[test]
fn a_replaced_executable_is_refused_at_click() {
    let f = fixture();
    licensed_install(&f, "PCSE00001");
    let vpk = vpk_in(&f, "PCSE00001");
    let req = request(&f, &vpk, "PCSE00001");
    fs::remove_file(&f.executable).unwrap();
    assert!(preflight_and_launch_vita3k_library(&req, &f.roots).is_err());
}

#[test]
fn a_title_in_two_installations_is_ambiguous_and_refused_at_click() {
    let f = fixture();
    licensed_install(&f, "PCSE00001");
    let vpk = vpk_in(&f, "PCSE00001");
    let req = request(&f, &vpk, "PCSE00001");
    let second = f.dir.path().join("profile-b");
    fs::create_dir_all(second.join("vs0/sys/external")).unwrap();
    install(&second, "PCSE00001", "PCSE00001", "Synthetic", "gd");
    fs::create_dir_all(second.join("ux0/license/app/PCSE00001")).unwrap();
    let mut roots = f.roots.clone();
    roots.explicit_configuration_roots.push(second);
    let evidence = inspect_vpk(&vpk).unwrap();
    assert_eq!(
        resolve_vpk(&evidence, &roots),
        Vita3kLibraryResolution::Ambiguous
    );
    assert!(matches!(
        preflight_and_launch_vita3k_library(&req, &roots),
        Err(Vita3kLibraryLaunchError::Installed(
            Vita3kInstalledLaunchError::Ambiguous(_)
        ))
    ));
}
