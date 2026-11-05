use super::*;
use std::os::unix::fs::symlink;

fn script(dir: &Path, body: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("openrct2");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn rct2_data(dir: &Path) -> PathBuf {
    let root = dir.join("rct2");
    fs::create_dir_all(root.join("Data")).unwrap();
    fs::write(root.join("Data/g1.dat"), b"synthetic, not game data").unwrap();
    root
}

fn probe_ok(_: &Path) -> Option<String> {
    Some("0.4.14".into())
}

fn request(bin: &Path, root: &Path) -> (OpenRct2LaunchRequest, Vec<PathBuf>) {
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let install = discover_openrct2(&dirs, &probe_ok).remove(0);
    let data = inspect_rct2_data(root);
    (OpenRct2LaunchRequest { install, data }, dirs)
}

#[test]
fn ready_plan_and_exact_argv() {
    let t = tempfile::tempdir().unwrap();
    let bin = script(&t.path().join("bin"), "exit 0");
    let root = rct2_data(t.path());
    let (req, dirs) = request(&bin, &root);
    let plan = plan_openrct2(Some(&req.install), Some(&req.data));
    assert_eq!(plan.readiness, AlternativeLaunchReadiness::Ready);
    let command = preflight_openrct2_launch(&req, &dirs, &probe_ok).unwrap();
    assert_eq!(command.executable, bin);
    let mut expected = OsString::from("--rct2-data-path=");
    expected.push(&root);
    assert_eq!(command.arguments, vec![expected]);
}

#[test]
fn game_and_engine_identity_are_separate_inputs() {
    let t = tempfile::tempdir().unwrap();
    let root = rct2_data(t.path());
    let data = inspect_rct2_data(&root);
    // Data alone: engine missing, game evidence still reported.
    let plan = plan_openrct2(None, Some(&data));
    assert_eq!(plan.readiness, AlternativeLaunchReadiness::ProviderMissing);
    // Engine alone: no game declared.
    let bin = script(&t.path().join("bin"), "exit 0");
    let install = discover_openrct2(&[bin.parent().unwrap().into()], &probe_ok).remove(0);
    let plan = plan_openrct2(Some(&install), None);
    assert_eq!(
        plan.readiness,
        AlternativeLaunchReadiness::IdentityInsufficient
    );
}

#[test]
fn unknown_or_old_versions_are_never_ready() {
    let t = tempfile::tempdir().unwrap();
    let bin = script(&t.path().join("bin"), "exit 0");
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let data = inspect_rct2_data(&rct2_data(t.path()));
    for (version, expected) in [
        (None, AlternativeLaunchReadiness::RequirementsUnknown),
        (
            Some("0.3.5"),
            AlternativeLaunchReadiness::VersionUnsupported,
        ),
    ] {
        let install = discover_openrct2(&dirs, &|_| version.map(String::from)).remove(0);
        assert_eq!(
            plan_openrct2(Some(&install), Some(&data)).readiness,
            expected
        );
    }
    assert!(version_supported("0.4.0") && version_supported("1.0.0"));
    assert!(!version_supported("garbage") && !version_supported("0.3.9"));
}

#[test]
fn data_folder_refusals_are_specific() {
    let t = tempfile::tempdir().unwrap();
    let empty = t.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    assert!(
        inspect_rct2_data(&empty)
            .blocker
            .unwrap()
            .contains("not found")
    );
    assert!(
        inspect_rct2_data(&t.path().join("missing"))
            .blocker
            .is_some()
    );
    assert!(inspect_rct2_data(Path::new("relative")).blocker.is_some());

    let wrong = t.path().join("wrong");
    fs::create_dir_all(wrong.join("data")).unwrap();
    fs::write(wrong.join("data/G1.DAT"), b"x").unwrap();
    let report = inspect_rct2_data(&wrong);
    assert!(report.blocker.unwrap().contains("capitalised"));
    assert!(wrong.join("data/G1.DAT").exists(), "never renamed");

    let zero = t.path().join("zero");
    fs::create_dir_all(zero.join("Data")).unwrap();
    fs::write(zero.join("Data/g1.dat"), b"").unwrap();
    assert!(inspect_rct2_data(&zero).blocker.is_some());

    let real = rct2_data(t.path());
    let link = t.path().join("link");
    symlink(&real, &link).unwrap();
    assert!(
        inspect_rct2_data(&link)
            .blocker
            .unwrap()
            .contains("real directory")
    );

    let linked_g1 = t.path().join("lg");
    fs::create_dir_all(linked_g1.join("Data")).unwrap();
    symlink(real.join("Data/g1.dat"), linked_g1.join("Data/g1.dat")).unwrap();
    assert!(inspect_rct2_data(&linked_g1).blocker.is_some());

    // A refused folder yields a non-ready plan.
    let t2 = tempfile::tempdir().unwrap();
    let bin = script(&t2.path().join("bin"), "exit 0");
    let install = discover_openrct2(&[bin.parent().unwrap().into()], &probe_ok).remove(0);
    assert_eq!(
        plan_openrct2(Some(&install), Some(&inspect_rct2_data(&empty))).readiness,
        AlternativeLaunchReadiness::OriginalAssetsMissing
    );
}

#[test]
fn discovery_skips_symlinks_non_executables_and_relative_dirs() {
    let t = tempfile::tempdir().unwrap();
    let real = script(&t.path().join("real"), "exit 0");
    let linked = t.path().join("linked");
    fs::create_dir_all(&linked).unwrap();
    symlink(&real, linked.join("openrct2")).unwrap();
    let plain = t.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("openrct2"), "not executable").unwrap();
    let found = discover_openrct2(
        &[linked, plain, "rel".into(), real.parent().unwrap().into()],
        &probe_ok,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].executable, real);
}

#[test]
fn click_time_drift_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let bin = script(&t.path().join("bin"), "exit 0");
    let root = rct2_data(t.path());
    let (req, dirs) = request(&bin, &root);

    // Data replaced after the user saw it.
    fs::remove_file(root.join("Data/g1.dat")).unwrap();
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs, &probe_ok),
        Err(OpenRct2LaunchError::NotReady(
            AlternativeLaunchReadiness::OriginalAssetsMissing,
            _
        ))
    ));
    fs::write(
        root.join("Data/g1.dat"),
        b"a different, longer synthetic file",
    )
    .unwrap();
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs, &probe_ok),
        Err(OpenRct2LaunchError::Changed(m)) if m.contains("data")
    ));

    // Executable replaced.
    let (req, dirs) = request(&bin, &rct2_data(t.path()));
    fs::remove_file(&bin).unwrap();
    script(bin.parent().unwrap(), "echo replaced; exit 0");
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs, &probe_ok),
        Err(OpenRct2LaunchError::Changed(m)) if m.contains("executable")
    ));

    // Version changed, and executable removed.
    let (req, dirs) = request(&bin, &rct2_data(t.path()));
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs, &|_| Some("0.4.15".into())),
        Err(OpenRct2LaunchError::Changed(m)) if m.contains("version")
    ));
    fs::remove_file(&bin).unwrap();
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs, &probe_ok),
        Err(OpenRct2LaunchError::NotReady(
            AlternativeLaunchReadiness::ProviderMissing,
            _
        ))
    ));
}

#[test]
fn launch_spawns_only_the_validated_command_and_reports_exit() {
    let t = tempfile::tempdir().unwrap();
    let log = t.path().join("args.log");
    let bin = script(
        &t.path().join("bin"),
        &format!("printf '%s\\n' \"$@\" > '{}'\nexit 3", log.display()),
    );
    let root = rct2_data(t.path());
    let (req, dirs) = request(&bin, &root);
    let mut process = preflight_and_launch_openrct2(&req, &dirs, &probe_ok).unwrap();
    let started = Instant::now();
    while process.poll().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no exit report"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let logged = fs::read_to_string(&log).unwrap();
    assert_eq!(
        logged.trim_end(),
        format!("--rct2-data-path={}", root.display())
    );
    assert_eq!(
        process
            .exit_report()
            .unwrap()
            .status
            .as_ref()
            .unwrap()
            .code(),
        Some(3)
    );
    // Game data untouched.
    assert_eq!(
        fs::read(root.join("Data/g1.dat")).unwrap(),
        b"synthetic, not game data"
    );
}

#[test]
fn refused_launch_spawns_nothing() {
    let t = tempfile::tempdir().unwrap();
    let marker = t.path().join("ran");
    let bin = script(
        &t.path().join("bin"),
        &format!("touch '{}'", marker.display()),
    );
    let root = rct2_data(t.path());
    let (req, dirs) = request(&bin, &root);
    fs::remove_dir_all(root.join("Data")).unwrap();
    assert!(preflight_and_launch_openrct2(&req, &dirs, &probe_ok).is_err());
    assert!(!marker.exists());
}
