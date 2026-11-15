use super::*;
use std::os::unix::fs::symlink;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A real native ELF stand-in: a copy of the system `true` (exit 0) or
/// `false` (exit 1), named `openrct2`. No real OpenRCT2 is ever run.
fn elf(dir: &Path, which: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let source = ["/usr/bin", "/bin"]
        .iter()
        .map(|d| Path::new(d).join(which))
        .find(|p| p.exists())
        .expect("system true/false");
    let path = dir.join("openrct2");
    let _ = fs::remove_file(&path);
    fs::copy(source, &path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

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

/// Explicit check with the real, descriptor-pinned probe (the coreutils
/// stand-in reports a version such as 9.4, which is >= the 0.4 floor).
fn checked(dirs: &[PathBuf]) -> OpenRct2Install {
    let candidate = discover_openrct2(dirs).remove(0);
    assert!(candidate.version.is_none() && candidate.content_sha256.is_none());
    check_openrct2(&candidate, dirs, &probe_openrct2_version).unwrap()
}

fn request(bin: &Path, root: &Path) -> (OpenRct2LaunchRequest, Vec<PathBuf>) {
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let install = checked(&dirs);
    let data = inspect_rct2_data(root);
    (OpenRct2LaunchRequest { install, data }, dirs)
}

fn wait_exit(process: &mut WatchedProcess) -> Option<i32> {
    let started = Instant::now();
    while process.poll().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no exit report"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    process
        .exit_report()
        .unwrap()
        .status
        .as_ref()
        .unwrap()
        .code()
}

#[test]
fn ready_plan_and_exact_argv() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let root = rct2_data(t.path());
    let (req, dirs) = request(&bin, &root);
    assert!(req.install.version.is_some() && req.install.content_sha256.is_some());
    let plan = plan_openrct2(Some(&req.install), Some(&req.data));
    assert_eq!(plan.readiness, AlternativeLaunchReadiness::Ready);
    let prepared = preflight_openrct2_launch(&req, &dirs).unwrap();
    assert_eq!(prepared.command.executable, fs::canonicalize(&bin).unwrap());
    let mut expected = OsString::from("--rct2-data-path=");
    expected.push(&root);
    assert_eq!(prepared.command.arguments, vec![expected]);
}

#[test]
fn game_and_engine_identity_are_separate_inputs() {
    let t = tempfile::tempdir().unwrap();
    let root = rct2_data(t.path());
    let data = inspect_rct2_data(&root);
    let plan = plan_openrct2(None, Some(&data));
    assert_eq!(plan.readiness, AlternativeLaunchReadiness::ProviderMissing);
    let bin = elf(&t.path().join("bin"), "true");
    let install = checked(&[bin.parent().unwrap().into()]);
    let plan = plan_openrct2(Some(&install), None);
    assert_eq!(
        plan.readiness,
        AlternativeLaunchReadiness::IdentityInsufficient
    );
}

#[test]
fn unknown_or_old_versions_are_never_ready() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let data = inspect_rct2_data(&rct2_data(t.path()));
    let candidate = discover_openrct2(&dirs).remove(0);
    // Unchecked: discovery alone is never Ready.
    assert_eq!(
        plan_openrct2(Some(&candidate), Some(&data)).readiness,
        AlternativeLaunchReadiness::RequirementsUnknown
    );
    // Ran but no version.
    assert_eq!(
        check_openrct2(&candidate, &dirs, &|_| None),
        Err(OpenRct2CheckError::NoVersion)
    );
    // Old version.
    let old = check_openrct2(&candidate, &dirs, &|_| Some("0.3.5".into())).unwrap();
    assert_eq!(
        plan_openrct2(Some(&old), Some(&data)).readiness,
        AlternativeLaunchReadiness::VersionUnsupported
    );
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
    let bin = elf(&t2.path().join("bin"), "true");
    let install = checked(&[bin.parent().unwrap().into()]);
    assert_eq!(
        plan_openrct2(Some(&install), Some(&inspect_rct2_data(&empty))).readiness,
        AlternativeLaunchReadiness::OriginalAssetsMissing
    );
}

#[test]
fn discovery_skips_symlinks_non_executables_and_relative_dirs() {
    let t = tempfile::tempdir().unwrap();
    let real = elf(&t.path().join("real"), "true");
    let linked = t.path().join("linked");
    fs::create_dir_all(&linked).unwrap();
    symlink(&real, linked.join("openrct2")).unwrap();
    let plain = t.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("openrct2"), "not executable").unwrap();
    let found = discover_openrct2(&[linked, plain, "rel".into(), real.parent().unwrap().into()]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].executable, fs::canonicalize(&real).unwrap());
}

#[test]
fn directory_aliases_do_not_create_ambiguous_duplicates() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("real"), "true");
    let alias = t.path().join("alias");
    symlink(bin.parent().unwrap(), &alias).unwrap();
    assert_eq!(
        discover_openrct2(&[bin.parent().unwrap().into(), alias]).len(),
        1
    );
}

#[test]
fn launch_executes_the_verified_file_even_if_the_path_is_replaced_afterwards() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let root = rct2_data(t.path());
    let (req, dirs) = request(&bin, &root);
    let prepared = preflight_openrct2_launch(&req, &dirs).unwrap();
    // After preflight an attacker renames a different program over the path.
    let decoy = elf(&t.path().join("decoy"), "false");
    fs::rename(&decoy, &bin).unwrap();
    let mut process = spawn_prepared_openrct2(&prepared).unwrap();
    assert_eq!(
        wait_exit(&mut process),
        Some(0),
        "the verified `true` must run, not the decoy `false`"
    );
    // And the replacement is not accepted by a fresh preflight.
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs),
        Err(OpenRct2LaunchError::Changed(m)) if m.contains("changed since it was checked")
    ));
    assert_eq!(
        fs::read(root.join("Data/g1.dat")).unwrap(),
        b"synthetic, not game data"
    );
}

#[test]
fn replacement_during_the_version_check_cannot_reuse_that_version() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let candidate = discover_openrct2(&dirs).remove(0);
    let swap = |_: &PinnedOpenRct2| {
        let decoy = elf(&t.path().join("decoy"), "false");
        fs::rename(decoy, &bin).unwrap();
        Some("0.4.14".to_string())
    };
    // The probe ran on the pinned original; its evidence is bound to it.
    let install = check_openrct2(&candidate, &dirs, &swap).unwrap();
    let data = inspect_rct2_data(&rct2_data(t.path()));
    let req = OpenRct2LaunchRequest { install, data };
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs),
        Err(OpenRct2LaunchError::Changed(_))
    ));
}

#[test]
fn symlink_substitution_is_refused_at_check_and_launch() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let root = rct2_data(t.path());
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let listed = discover_openrct2(&dirs).remove(0);
    let (req, _) = request(&bin, &root);
    let other = elf(&t.path().join("other"), "true");
    fs::remove_file(&bin).unwrap();
    symlink(&other, &bin).unwrap();
    assert!(matches!(
        check_openrct2(&listed, &dirs, &probe_openrct2_version),
        Err(OpenRct2CheckError::Refused(_))
    ));
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs),
        Err(OpenRct2LaunchError::NotReady(
            AlternativeLaunchReadiness::ProviderMissing,
            _
        ))
    ));
    // The pinning open itself never follows a final symlink either.
    assert!(PinnedOpenRct2::open(&bin).is_err());
}

#[test]
fn same_size_in_place_edit_with_restored_mtime_makes_the_checked_version_stale() {
    use std::io::{Seek, SeekFrom, Write};
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let root = rct2_data(t.path());
    let (req, dirs) = request(&bin, &root);
    let before = fs::metadata(&bin).unwrap();
    let mut file = fs::OpenOptions::new().write(true).open(&bin).unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(&[0xA5]).unwrap();
    file.set_modified(before.modified().unwrap()).unwrap();
    drop(file);
    let after = fs::metadata(&bin).unwrap();
    // Device, inode, size and mtime cannot see this edit (ctime may, when
    // the kernel's timestamp tick has moved); the hash always does.
    let old_check = |m: &fs::Metadata| {
        let i = CapturedFileIdentity::capture(m);
        (i.device, i.inode, i.size, i.modified)
    };
    assert_eq!(old_check(&before), old_check(&after));
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs),
        Err(OpenRct2LaunchError::Changed(m)) if m.contains("changed since it was checked")
    ));
}

#[test]
fn unchecked_scripts_oversized_and_ambiguous_candidates_are_refused() {
    let t = tempfile::tempdir().unwrap();
    // An unchecked install can never be launched.
    let bin = elf(&t.path().join("bin"), "true");
    let dirs = vec![bin.parent().unwrap().to_path_buf()];
    let data = inspect_rct2_data(&rct2_data(t.path()));
    let req = OpenRct2LaunchRequest {
        install: discover_openrct2(&dirs).remove(0),
        data,
    };
    assert!(matches!(
        preflight_openrct2_launch(&req, &dirs),
        Err(OpenRct2LaunchError::NotReady(
            AlternativeLaunchReadiness::RequirementsUnknown,
            _
        ))
    ));
    // Wrappers are not pinned and are refused without running.
    let marker = t.path().join("ran");
    let wrapper = script(
        &t.path().join("w"),
        &format!("touch '{}'\necho v0.4.14", marker.display()),
    );
    let wdirs = vec![wrapper.parent().unwrap().to_path_buf()];
    let calls = AtomicUsize::new(0);
    let counting = |_: &PinnedOpenRct2| {
        calls.fetch_add(1, Ordering::SeqCst);
        Some("0.4.14".into())
    };
    let candidate = discover_openrct2(&wdirs).remove(0);
    assert!(matches!(
        check_openrct2(&candidate, &wdirs, &counting),
        Err(OpenRct2CheckError::Refused(m)) if m.contains("ELF")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!marker.exists());
    // Oversized files are refused by size, not trusted by metadata.
    let big = t.path().join("big");
    fs::create_dir_all(&big).unwrap();
    let big_path = big.join("openrct2");
    let f = fs::File::create(&big_path).unwrap();
    f.set_len(MAX_EXECUTABLE_BYTES + 1).unwrap();
    fs::set_permissions(&big_path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        PinnedOpenRct2::open(&big_path)
            .unwrap_err()
            .contains("128 MiB")
    );
    // A check for a candidate that is not exactly one current match is refused.
    let ghost = OpenRct2Install {
        executable: t.path().join("nowhere/openrct2"),
        ..candidate
    };
    assert!(matches!(
        check_openrct2(&ghost, &dirs, &counting),
        Err(OpenRct2CheckError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn only_the_explicit_check_runs_the_probe_and_only_for_the_selected_candidate() {
    let t = tempfile::tempdir().unwrap();
    let a = elf(&t.path().join("a"), "true");
    let b = elf(&t.path().join("b"), "true");
    let dirs = vec![a.parent().unwrap().into(), b.parent().unwrap().into()];
    let calls = AtomicUsize::new(0);
    let seen = std::sync::Mutex::new(Vec::new());
    let counting = |p: &PinnedOpenRct2| {
        calls.fetch_add(1, Ordering::SeqCst);
        seen.lock().unwrap().push(p.path.clone());
        Some("0.4.14".into())
    };
    let found = discover_openrct2(&dirs);
    let data = inspect_rct2_data(&rct2_data(t.path()));
    let _ = (
        plan_openrct2(Some(&found[0]), Some(&data)),
        default_search_dirs(),
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "passive work never probes");
    let want = fs::canonicalize(&b).unwrap();
    let selected = found.iter().find(|f| f.executable == want).unwrap();
    check_openrct2(selected, &dirs, &counting).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*seen.lock().unwrap(), vec![want]);
}

#[test]
fn real_probe_runs_the_pinned_descriptor_and_limits_output() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let pinned = PinnedOpenRct2::open(&fs::canonicalize(&bin).unwrap()).unwrap();
    let decoy = elf(&t.path().join("decoy"), "false");
    fs::rename(decoy, &bin).unwrap();
    // The path now names a different file, but the probe still runs the pin.
    assert!(probe_openrct2_version(&pinned).is_some());
    assert!(pinned.confirm_unchanged().is_ok());
}

/// Renaming over the path moves only the pinned inode's ctime (its last
/// name is gone); the bytes still hash the same through the descriptor, so
/// the pin stays valid. A forged ctime with changed bytes is refused.
#[test]
fn a_ctime_only_change_is_revalidated_by_hash_and_changed_bytes_are_refused() {
    use std::io::{Seek, SeekFrom, Write};
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let mut pinned = PinnedOpenRct2::open(&fs::canonicalize(&bin).unwrap()).unwrap();
    let checked = pinned.identity;
    pinned.identity.changed = checked
        .changed
        .map(|ctime| ctime - std::time::Duration::from_nanos(1));
    assert!(
        pinned.confirm_unchanged().is_ok(),
        "same bytes: revalidated"
    );
    let mut file = fs::OpenOptions::new().write(true).open(&bin).unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(&[0xA5]).unwrap();
    file.set_modified(checked.modified.unwrap()).unwrap();
    drop(file);
    pinned.identity.changed = fs::metadata(&bin)
        .map(|m| CapturedFileIdentity::capture(&m).changed)
        .unwrap()
        .map(|ctime| ctime - std::time::Duration::from_nanos(1));
    assert!(
        pinned.confirm_unchanged().is_err(),
        "changed bytes: refused"
    );
}

#[test]
fn files_that_are_writable_or_setuid_are_not_pinned() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let path = fs::canonicalize(&bin).unwrap();
    assert!(PinnedOpenRct2::open(&path).is_ok());
    for (mode, why) in [
        (0o775, "writable"),
        (0o757, "writable"),
        (0o4755, "setuid"),
        (0o2755, "setuid"),
    ] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let error = PinnedOpenRct2::open(&path).unwrap_err();
        assert!(error.contains(why), "{mode:o}: {error}");
    }
    // Refusals reach the check as Refused, without running anything.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o775)).unwrap();
    let dirs = vec![path.parent().unwrap().to_path_buf()];
    let candidate = discover_openrct2(&dirs).remove(0);
    let calls = AtomicUsize::new(0);
    let counting = |_: &PinnedOpenRct2| {
        calls.fetch_add(1, Ordering::SeqCst);
        Some("0.4.14".into())
    };
    assert!(matches!(
        check_openrct2(&candidate, &dirs, &counting),
        Err(OpenRct2CheckError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

fn sh(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(script);
    command
}

#[test]
fn version_probe_reads_a_version_from_a_well_behaved_program() {
    assert_eq!(
        run_version_command(sh("echo 'OpenRCT2, v0.4.14'")).as_deref(),
        Some("0.4.14")
    );
    // Non-UTF8 bytes do not hide a valid version.
    assert_eq!(
        run_version_command(sh("printf '\\377\\376 v0.4.2\\n'")).as_deref(),
        Some("0.4.2")
    );
}

#[test]
fn version_probe_times_out_and_kills_the_whole_group() {
    let marker = tempfile::tempdir().unwrap();
    let pidfile = marker.path().join("pid");
    let started = Instant::now();
    let script = format!("sleep 30 & echo $! > '{}'; sleep 30", pidfile.display());
    assert_eq!(run_version_command(sh(&script)), None);
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "bounded by the timeout"
    );
    let pid: i32 = fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // SAFETY: signal 0 only tests for existence.
    let alive = unsafe { libc::kill(pid, 0) } == 0
        && fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z"));
    assert!(!alive, "the descendant must be killed with the group");
}

#[test]
fn version_probe_is_not_blocked_by_a_descendant_holding_the_pipe_or_by_a_flood() {
    let started = Instant::now();
    // The shell exits at once; a background child keeps stdout open for 30 s.
    assert_eq!(
        run_version_command(sh("(sleep 30) & echo v0.4.1")).as_deref(),
        Some("0.4.1")
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "not held by the pipe holder"
    );
    // A flood is capped; the version at the start is still read, memory is bounded.
    let started = Instant::now();
    let flood = run_version_command(sh("echo v0.4.9; head -c 50000000 /dev/zero | tr '\\0' 'a'"));
    assert!(started.elapsed() < Duration::from_secs(6));
    assert_eq!(flood.as_deref(), Some("0.4.9"));
}

#[test]
fn owner_and_mode_rules_cover_foreign_owners_without_needing_root() {
    assert!(check_owner_and_mode(1000, 0o755, 1000).is_ok());
    assert!(check_owner_and_mode(0, 0o755, 1000).is_ok());
    assert!(
        check_owner_and_mode(1001, 0o755, 1000)
            .unwrap_err()
            .contains("another user")
    );
    assert!(
        check_owner_and_mode(1000, 0o775, 1000)
            .unwrap_err()
            .contains("writable")
    );
    assert!(
        check_owner_and_mode(1000, 0o4755, 1000)
            .unwrap_err()
            .contains("setuid")
    );
}

#[test]
fn a_descendant_left_in_the_group_is_killed_after_a_successful_check() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let script = format!(
        "(sleep 30) & echo $! > '{}'; echo v0.4.1",
        pidfile.display()
    );
    assert_eq!(run_version_command(sh(&script)).as_deref(), Some("0.4.1"));
    let pid: i32 = fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let alive = fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z"));
    assert!(
        !alive,
        "the leftover descendant must be killed with its group"
    );
}

#[test]
fn a_symlinked_data_folder_is_refused_and_its_comment_matches() {
    let t = tempfile::tempdir().unwrap();
    let elsewhere = t.path().join("elsewhere/Data");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("g1.dat"), b"synthetic").unwrap();
    let root = t.path().join("rct2");
    fs::create_dir_all(&root).unwrap();
    symlink(&elsewhere, root.join("Data")).unwrap();
    let report = inspect_rct2_data(&root);
    let blocker = report.blocker.expect("a symlinked Data folder is refused");
    assert!(blocker.contains("symbolic link"), "{blocker}");
    assert!(report.g1_identity.is_none());
    // Inspection is read-only: nothing was created inside the chosen folder.
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn checked_evidence_goes_stale_when_the_file_or_folder_changes() {
    let t = tempfile::tempdir().unwrap();
    let bin = elf(&t.path().join("bin"), "true");
    let root = rct2_data(t.path());
    let (req, _dirs) = request(&bin, &root);
    assert!(req.install.file_is_current() && req.data.evidence_is_current());
    // Replace the executable (new inode): the old verification no longer applies.
    let replacement = elf(&t.path().join("other"), "false");
    fs::rename(&replacement, &bin).unwrap();
    assert!(!req.install.file_is_current());
    // Swap it for a symlink: also not current.
    fs::remove_file(&bin).unwrap();
    symlink("/usr/bin/true", &bin).unwrap();
    assert!(!req.install.file_is_current());
    // Changing g1.dat makes the folder evidence stale.
    fs::write(
        root.join("Data/g1.dat"),
        b"a different, longer synthetic file",
    )
    .unwrap();
    assert!(!req.data.evidence_is_current());
}
