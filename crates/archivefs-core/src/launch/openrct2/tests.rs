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

fn checked(dirs: &[PathBuf], version: Option<&'static str>) -> OpenRct2Install {
    let candidate = discover_openrct2(dirs).remove(0);
    assert!(
        candidate.version.is_none(),
        "discovery alone knows no version"
    );
    check_openrct2(&candidate, dirs, &|_| version.map(String::from)).unwrap_or(candidate)
}

fn request(bin: &Path, root: &Path) -> (OpenRct2LaunchRequest, Vec<PathBuf>) {
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let install = checked(&dirs, Some("0.4.14"));
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
    let command = preflight_openrct2_launch(&req, &dirs).unwrap();
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
    let install = checked(&[bin.parent().unwrap().into()], Some("0.4.14"));
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
        let install = checked(&dirs, version);
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
    let install = checked(&[bin.parent().unwrap().into()], Some("0.4.14"));
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
    let found = discover_openrct2(&[linked, plain, "rel".into(), real.parent().unwrap().into()]);
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
        preflight_openrct2_launch(&req, &dirs),
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
        preflight_openrct2_launch(&req, &dirs),
        Err(OpenRct2LaunchError::Changed(m)) if m.contains("data")
    ));

    // Executable replaced.
    let (req, dirs) = request(&bin, &rct2_data(t.path()));
    fs::remove_file(&bin).unwrap();
    script(bin.parent().unwrap(), "echo replaced; exit 0");
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs),
        Err(OpenRct2LaunchError::Changed(m)) if m.contains("executable")
    ));

    // Executable removed.
    let (req, dirs) = request(&bin, &rct2_data(t.path()));
    fs::remove_file(&bin).unwrap();
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs),
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
    let mut process = preflight_and_launch_openrct2(&req, &dirs).unwrap();
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
    assert!(preflight_and_launch_openrct2(&req, &dirs).is_err());
    assert!(!marker.exists());
}

fn marker_script(dir: &Path, marker: &Path) -> PathBuf {
    script(
        dir,
        &format!("touch '{}'\necho 'OpenRCT2, v0.4.14'", marker.display()),
    )
}

#[test]
fn passive_discovery_and_planning_never_execute_anything() {
    let t = tempfile::tempdir().unwrap();
    let marker = t.path().join("ran");
    let bin = marker_script(&t.path().join("bin"), &marker);
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let found = discover_openrct2(&dirs);
    assert_eq!(found.len(), 1);
    assert!(found[0].version.is_none());
    let data = inspect_rct2_data(&rct2_data(t.path()));
    let plan = plan_openrct2(Some(&found[0]), Some(&data));
    assert_eq!(
        plan.readiness,
        AlternativeLaunchReadiness::RequirementsUnknown
    );
    let _ = default_search_dirs();
    assert!(!marker.exists(), "nothing may run before an explicit check");
}

#[test]
fn explicit_check_executes_only_the_selected_candidate_and_records_the_version() {
    let t = tempfile::tempdir().unwrap();
    let (marker_a, marker_b) = (t.path().join("ran-a"), t.path().join("ran-b"));
    let a = marker_script(&t.path().join("a"), &marker_a);
    let b = marker_script(&t.path().join("b"), &marker_b);
    let dirs = vec![a.parent().unwrap().into(), b.parent().unwrap().into()];
    let found = discover_openrct2(&dirs);
    assert_eq!(found.len(), 2);
    let want = fs::canonicalize(&b).unwrap();
    let selected = found.iter().find(|f| f.executable == want).unwrap();
    let done = check_openrct2(selected, &dirs, &probe_openrct2_version).unwrap();
    assert_eq!(done.version.as_deref(), Some("0.4.14"));
    assert!(marker_b.exists(), "the selected candidate ran");
    assert!(!marker_a.exists(), "no other candidate may run");
}

#[test]
fn check_refuses_changed_symlinked_missing_and_versionless_candidates() {
    let t = tempfile::tempdir().unwrap();
    let marker = t.path().join("ran");
    let bin = marker_script(&t.path().join("bin"), &marker);
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let listed = discover_openrct2(&dirs).remove(0);

    fs::remove_file(&bin).unwrap();
    marker_script(bin.parent().unwrap(), &marker);
    assert!(matches!(
        check_openrct2(&listed, &dirs, &probe_openrct2_version),
        Err(OpenRct2CheckError::Refused(m)) if m.contains("changed")
    ));
    fs::remove_file(&bin).unwrap();
    let other = t.path().join("other");
    marker_script(&other, &marker);
    symlink(other.join("openrct2"), &bin).unwrap();
    assert!(matches!(
        check_openrct2(&listed, &dirs, &probe_openrct2_version),
        Err(OpenRct2CheckError::Refused(_))
    ));
    fs::remove_file(&bin).unwrap();
    assert!(matches!(
        check_openrct2(&listed, &dirs, &probe_openrct2_version),
        Err(OpenRct2CheckError::Refused(_))
    ));
    assert!(
        !marker.exists(),
        "a refused check must not execute anything"
    );

    let silent = script(&t.path().join("silent"), "exit 0");
    let dirs = vec![silent.parent().unwrap().to_path_buf()];
    let candidate = discover_openrct2(&dirs).remove(0);
    assert_eq!(
        check_openrct2(&candidate, &dirs, &probe_openrct2_version),
        Err(OpenRct2CheckError::NoVersion)
    );
}

#[test]
fn directory_aliases_do_not_create_ambiguous_duplicates() {
    let t = tempfile::tempdir().unwrap();
    let bin = script(&t.path().join("real"), "exit 0");
    let alias = t.path().join("alias");
    symlink(bin.parent().unwrap(), &alias).unwrap();
    let found = discover_openrct2(&[bin.parent().unwrap().into(), alias]);
    assert_eq!(found.len(), 1);
}

#[test]
fn launch_preflight_does_not_rerun_the_version_check() {
    let t = tempfile::tempdir().unwrap();
    let marker = t.path().join("ran");
    let bin = marker_script(&t.path().join("bin"), &marker);
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let candidate = discover_openrct2(&dirs).remove(0);
    let install = check_openrct2(&candidate, &dirs, &probe_openrct2_version).unwrap();
    fs::remove_file(&marker).unwrap();
    let data = inspect_rct2_data(&rct2_data(t.path()));
    let req = OpenRct2LaunchRequest { install, data };
    preflight_openrct2_launch(&req, &dirs).unwrap();
    assert!(
        !marker.exists(),
        "preflight is passive; only the launch executes"
    );
}
