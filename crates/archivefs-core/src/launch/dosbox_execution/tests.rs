use super::*;
use crate::launch::dosbox_command::DosBoxVariant;
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// --- fixtures --------------------------------------------------------------

struct ReadyFixture {
    game_directory: PathBuf,
    executable: PathBuf,
    config_path: PathBuf,
}

impl Drop for ReadyFixture {
    fn drop(&mut self) {
        if let Some(root) = self.game_directory.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn dos_identity() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "DOS".into(),
        game_key: "dat:some-hash".into(),
    })
}

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write exe");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// Builds a temp tree with an absolute game directory (name = `game_name`),
/// a verified `dosbox.conf` in it, and an executable running `script`.
fn ready_fixture(label: &str, game_name: &str, script: &str) -> ReadyFixture {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    let root = std::env::temp_dir().join(format!(
        "archivefs-dosbox-exec-{label}-{}-{unique}",
        std::process::id(),
    ));
    let game_directory = root.join(game_name);
    std::fs::create_dir_all(&game_directory).expect("game dir");
    let config_path = game_directory.join("dosbox.conf");
    std::fs::write(
        &config_path,
        b"[dosbox]\nmemsize=16\n[autoexec]\nmount c .\nc:\ngame.exe\n",
    )
    .expect("config");
    std::fs::write(game_directory.join("GAME.EXE"), b"MZ-not-run-on-host").expect("game");
    let executable = root.join("dosbox");
    write_executable(&executable, script);
    ReadyFixture {
        game_directory,
        executable,
        config_path,
    }
}

fn approval_for(fixture: &ReadyFixture) -> crate::launch::dosbox_autoexec::DosBoxStartApproval {
    crate::launch::dosbox_autoexec::review_dosbox_game_directory(&fixture.game_directory)
        .expect("review")
        .approve()
        .expect("approvable review")
}

fn dummy_approval() -> crate::launch::dosbox_autoexec::DosBoxStartApproval {
    let identity = crate::launch::process_spawn::CapturedFileIdentity {
        device: 0,
        inode: 0,
        size: 0,
        modified: None,
    };
    crate::launch::dosbox_autoexec::DosBoxStartApproval {
        config_path: PathBuf::from("/x/dosbox.conf"),
        config_sha256: String::new(),
        start_command: "game.exe".into(),
        start_file: PathBuf::from("/x/GAME.EXE"),
        start_file_identity: identity,
        start_file_sha256: String::new(),
        game_directory_identity: identity,
    }
}

fn request_for(fixture: &ReadyFixture) -> DosBoxLaunchRequest {
    DosBoxLaunchRequest {
        approval: approval_for(fixture),
        host_access: DosBoxHostAccess::none(),
        game_directory: fixture.game_directory.clone(),
        identity: dos_identity().into(),
        expected_executable: fixture.executable.clone(),
        expected_variant: DosBoxVariant::Classic,
        expected_config_path: fixture.config_path.clone(),
    }
}

fn wait_for_exit(process: &mut LaunchedDosBoxProcess) -> &DosBoxLaunchExitReport {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if process.poll().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "fake DOSBox script did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    process.poll().unwrap()
}

// --- positive ------------------------------------------------------------

#[test]
fn a_ready_plan_reaches_the_execution_adapter() {
    let fixture = ready_fixture("ready", "Keen", "#!/bin/sh\nexit 0\n");
    let command = preflight_dosbox_launch(&request_for(&fixture)).expect("preflight");
    assert_eq!(command.executable, fixture.executable);
    assert_eq!(command.arguments.len(), 2);
    assert_eq!(command.arguments[0], "-conf");
    assert_eq!(command.arguments[1], fixture.config_path.as_os_str());
    assert_eq!(
        command.working_directory.as_deref(),
        Some(fixture.game_directory.as_path())
    );
}

#[test]
fn exactly_two_argv_components_are_preserved_through_preflight() {
    let fixture = ready_fixture("argv2", "Keen", "#!/bin/sh\nexit 0\n");
    let command = preflight_dosbox_launch(&request_for(&fixture)).unwrap();
    assert_eq!(command.arguments.len(), 2);
    assert_eq!(command.arguments[0], "-conf");
    assert_eq!(command.arguments[1], fixture.config_path.as_os_str());
}

#[test]
fn a_config_path_containing_spaces_stays_one_argv_component() {
    let fixture = ready_fixture("spaces", "Some DOS Game (1993)", "#!/bin/sh\nexit 0\n");
    assert!(fixture.config_path.to_string_lossy().contains(' '));
    let command = preflight_dosbox_launch(&request_for(&fixture)).unwrap();
    assert_eq!(command.arguments.len(), 2);
    assert_eq!(command.arguments[1], fixture.config_path.as_os_str());
}

#[test]
fn the_fake_executable_receives_exactly_conf_and_the_config_path_and_the_right_cwd() {
    let fixture = ready_fixture("capture", "Keen", "placeholder");
    let capture = fixture.game_directory.join("capture.txt");
    write_executable(
        &fixture.executable,
        &format!(
            "#!/bin/sh\n{{ printf 'ARG:%s\\n' \"$@\"; printf 'CWD:%s\\n' \"$(pwd -P)\"; }} > '{}'\nexit 0\n",
            capture.display()
        ),
    );
    let command = preflight_dosbox_launch(&request_for(&fixture)).unwrap();
    let mut process = spawn_dosbox(command, &DosBoxHostAccess::none()).expect("spawn");
    wait_for_exit(&mut process);

    let recorded = std::fs::read_to_string(&capture).unwrap();
    let lines: Vec<&str> = recorded.lines().collect();
    assert_eq!(
        lines,
        vec![
            "ARG:-conf",
            &format!("ARG:{}", fixture.config_path.display()),
            &format!(
                "CWD:{}",
                std::fs::canonicalize(&fixture.game_directory)
                    .unwrap()
                    .display()
            ),
        ]
    );
}

#[test]
fn a_successful_spawn_returns_a_pid_but_is_not_proof_the_game_booted() {
    let fixture = ready_fixture("spawn-ok", "Keen", "#!/bin/sh\nexit 0\n");
    let command = preflight_dosbox_launch(&request_for(&fixture)).unwrap();
    let mut process = spawn_dosbox(command, &DosBoxHostAccess::none()).expect("spawn");
    assert!(process.pid > 0);
    let report = wait_for_exit(&mut process);
    // A clean exit of the fake binary; the adapter makes no "game verified"
    // claim - it only reports that a process ran and how it ended.
    assert!(report.status.as_ref().unwrap().success());
}

#[test]
fn a_non_zero_exit_and_bounded_stderr_are_reported_truthfully() {
    let fixture = ready_fixture(
        "spawn-fail",
        "Keen",
        "#!/bin/sh\necho 'synthetic dosbox failure' 1>&2\nexit 5\n",
    );
    let command = preflight_dosbox_launch(&request_for(&fixture)).unwrap();
    let mut process = spawn_dosbox(command, &DosBoxHostAccess::none()).unwrap();
    let report = wait_for_exit(&mut process);
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(5));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic dosbox failure"));
}

// --- shell safety ------------------------------------------------------

#[test]
fn shell_metacharacters_in_the_path_stay_literal_and_are_never_executed() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-dosbox-exec-shell-marker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&marker);
    let dir_name = format!("game; touch {} #", marker.display());
    let fixture = ready_fixture("shell", &dir_name, "#!/bin/sh\nexit 0\n");
    let command = preflight_dosbox_launch(&request_for(&fixture)).unwrap();
    let mut process = spawn_dosbox(command, &DosBoxHostAccess::none()).unwrap();
    wait_for_exit(&mut process);
    assert!(
        !marker.exists(),
        "shell metacharacters in a path must never be interpreted"
    );
    let _ = std::fs::remove_file(&marker);
}

// --- blocked / refused before spawn ----------------------------------

#[test]
fn a_non_dos_resolved_identity_is_blocked_before_any_spawn() {
    let fixture = ready_fixture("mismatch", "Keen", "#!/bin/sh\nexit 0\n");
    let mut request = request_for(&fixture);
    request.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PC".into(),
        game_key: "k".into(),
    })
    .into();
    let error = preflight_dosbox_launch(&request).unwrap_err();
    assert_eq!(error.kind, DosBoxLaunchPreflightErrorKind::CommandBlocked);
}

#[test]
fn an_unknown_identity_cannot_reach_execution() {
    let fixture = ready_fixture("unknown", "Keen", "#!/bin/sh\nexit 0\n");
    let mut request = request_for(&fixture);
    request.identity = CanonicalIdentityStatus::Unknown.into();
    let error = preflight_dosbox_launch(&request).unwrap_err();
    assert_eq!(error.kind, DosBoxLaunchPreflightErrorKind::CommandBlocked);
}

#[test]
fn a_conflicting_identity_cannot_reach_execution() {
    let fixture = ready_fixture("conflict", "Keen", "#!/bin/sh\nexit 0\n");
    let mut request = request_for(&fixture);
    request.identity = CanonicalIdentityStatus::Conflicting.into();
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::CommandBlocked
    );
}

#[test]
fn a_relative_game_directory_is_refused_before_spawn() {
    let error = preflight_dosbox_launch(&DosBoxLaunchRequest {
        game_directory: PathBuf::from("relative/dir"),
        identity: dos_identity().into(),
        expected_executable: PathBuf::from("/usr/games/dosbox"),
        expected_variant: DosBoxVariant::Classic,
        expected_config_path: PathBuf::from("relative/dir/dosbox.conf"),
        approval: dummy_approval(),
        host_access: DosBoxHostAccess::none(),
    })
    .unwrap_err();
    assert_eq!(
        error.kind,
        DosBoxLaunchPreflightErrorKind::GameDirectoryNotAbsolute
    );
}

#[test]
fn a_missing_game_directory_is_refused() {
    let error = preflight_dosbox_launch(&DosBoxLaunchRequest {
        game_directory: PathBuf::from("/archivefs/does/not/exist/here"),
        identity: dos_identity().into(),
        expected_executable: PathBuf::from("/usr/games/dosbox"),
        expected_variant: DosBoxVariant::Classic,
        expected_config_path: PathBuf::from("/archivefs/does/not/exist/here/dosbox.conf"),
        approval: dummy_approval(),
        host_access: DosBoxHostAccess::none(),
    })
    .unwrap_err();
    assert_eq!(
        error.kind,
        DosBoxLaunchPreflightErrorKind::GameDirectoryNotFound
    );
}

// --- executable / config drift ------------------------------------------

#[test]
fn an_executable_that_disappeared_before_preflight_fails_safely() {
    let fixture = ready_fixture("exe-gone", "Keen", "#!/bin/sh\nexit 0\n");
    let request = request_for(&fixture);
    std::fs::remove_file(&fixture.executable).unwrap();
    let error = preflight_dosbox_launch(&request).unwrap_err();
    assert_eq!(
        error.kind,
        DosBoxLaunchPreflightErrorKind::ExecutableUnavailable
    );
}

#[test]
fn a_non_executable_regular_file_at_the_expected_path_is_refused() {
    let fixture = ready_fixture("exe-noexec", "Keen", "#!/bin/sh\nexit 0\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fixture.executable, std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert_eq!(
            preflight_dosbox_launch(&request_for(&fixture))
                .unwrap_err()
                .kind,
            DosBoxLaunchPreflightErrorKind::ExecutableUnavailable
        );
    }
}

#[test]
fn a_config_that_lost_its_autoexec_since_readiness_is_refused() {
    let fixture = ready_fixture("cfg-degraded", "Keen", "#!/bin/sh\nexit 0\n");
    let request = request_for(&fixture);
    std::fs::write(
        &fixture.config_path,
        b"[dosbox]\nmemsize=16\n[cpu]\ncycles=max\n",
    )
    .unwrap();
    let error = preflight_dosbox_launch(&request).unwrap_err();
    assert_eq!(
        error.kind,
        DosBoxLaunchPreflightErrorKind::ConfigNotVerified
    );
}

#[test]
fn a_config_that_disappeared_since_readiness_is_refused() {
    let fixture = ready_fixture("cfg-gone", "Keen", "#!/bin/sh\nexit 0\n");
    let request = request_for(&fixture);
    std::fs::remove_file(&fixture.config_path).unwrap();
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::ConfigNotVerified
    );
}

#[test]
fn a_config_at_a_different_path_than_authorized_is_refused_as_drift() {
    let fixture = ready_fixture("cfg-drift", "Keen", "#!/bin/sh\nexit 0\n");
    let mut request = request_for(&fixture);
    request.expected_config_path = fixture.game_directory.join("some-other-name.conf");
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::ConfigPathDrift
    );
}

// --- spawn-time failure ----------------------------------------------

#[test]
fn spawn_reports_an_os_error_when_the_executable_is_gone_at_spawn_time() {
    let fixture = ready_fixture("spawn-gone", "Keen", "#!/bin/sh\nexit 0\n");
    let mut command = preflight_dosbox_launch(&request_for(&fixture)).unwrap();
    command.executable = fixture.game_directory.parent().unwrap().join("not-there");
    let result = spawn_dosbox(command, &DosBoxHostAccess::none());
    assert!(matches!(
        result,
        Err(DosBoxLaunchSpawnError::Sandbox(
            DosBoxSandboxError::InvalidGrant(_)
        ))
    ));
}

// --- unsupported variant cannot even form a request -------------------

#[test]
fn an_unsupported_dosbox_variant_never_produces_a_binding_and_so_cannot_execute() {
    use crate::launch::dosbox_command::{
        DosBoxBindingRefusal, resolve_dosbox_native_launch_binding_from_id,
    };
    // `DosBoxLaunchRequest::expected_variant` is a `DosBoxVariant` enum with
    // only Classic/Staging, so an unsupported variant can never be carried
    // into execution. It is rejected at the binding boundary instead:
    assert_eq!(
        resolve_dosbox_native_launch_binding_from_id("dosbox-x").unwrap_err(),
        DosBoxBindingRefusal::VariantUnsupported("dosbox-x".into())
    );
}

// --- approval, autoexec safety and host safety ---------------------------

fn rewrite_config(fixture: &ReadyFixture, autoexec: &str) {
    std::fs::write(
        &fixture.config_path,
        format!("[dosbox]\nmemsize=16\n[autoexec]\n{autoexec}"),
    )
    .unwrap();
}

#[test]
fn a_changed_config_after_approval_is_refused() {
    let fixture = ready_fixture("cfg-changed", "Keen", "#!/bin/sh\nexit 0\n");
    let request = request_for(&fixture);
    rewrite_config(&fixture, "mount c .\nc:\ngame.exe /extra\n");
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::StartCommandNotApproved
    );
}

#[test]
fn a_swapped_start_program_after_approval_is_refused() {
    let fixture = ready_fixture("exe-swapped", "Keen", "#!/bin/sh\nexit 0\n");
    let request = request_for(&fixture);
    std::fs::write(
        fixture.game_directory.join("GAME.EXE"),
        b"MZ-different-and-longer",
    )
    .unwrap();
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::StartCommandNotApproved
    );
}

#[test]
fn a_replaced_game_directory_after_approval_is_refused() {
    let fixture = ready_fixture("dir-replaced", "Keen", "#!/bin/sh\nexit 0\n");
    let request = request_for(&fixture);
    let moved = fixture.game_directory.with_file_name("Keen-old");
    std::fs::rename(&fixture.game_directory, &moved).unwrap();
    std::fs::create_dir(&fixture.game_directory).unwrap();
    std::fs::copy(moved.join("dosbox.conf"), &fixture.config_path).unwrap();
    std::fs::copy(
        moved.join("GAME.EXE"),
        fixture.game_directory.join("GAME.EXE"),
    )
    .unwrap();
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::GameDirectoryChanged
    );
}

#[test]
fn an_unsafe_autoexec_after_approval_is_refused_even_with_a_matching_shape() {
    let fixture = ready_fixture("cfg-unsafe", "Keen", "#!/bin/sh\nexit 0\n");
    let request = request_for(&fixture);
    rewrite_config(&fixture, "mount c /\nc:\ngame.exe\n");
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::AutoexecUnsafe
    );
}

#[test]
fn the_dos_program_is_never_run_on_the_host() {
    let fixture = ready_fixture("no-host-exec", "Keen", "placeholder");
    let capture = fixture.game_directory.join("argv.txt");
    write_executable(
        &fixture.executable,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture.display()
        ),
    );
    // A host-executable script named like the start program: if anything ran
    // it directly, the marker would appear.
    let marker = fixture.game_directory.join("host-ran");
    write_executable(
        &fixture.game_directory.join("GAME.EXE"),
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    let command = preflight_dosbox_launch(&request_for(&fixture)).unwrap();
    assert_eq!(command.executable, fixture.executable);
    assert!(
        command
            .arguments
            .iter()
            .all(|a| a.to_string_lossy() != "GAME.EXE")
    );
    let mut process = spawn_dosbox(command, &DosBoxHostAccess::none()).unwrap();
    wait_for_exit(&mut process);
    assert!(
        !marker.exists(),
        "the DOS program must never run on the host"
    );
    let argv = std::fs::read_to_string(&capture).unwrap();
    assert_eq!(argv.lines().count(), 2);
}

#[test]
fn a_user_approved_target_uses_the_same_preflight_and_detects_changes() {
    use crate::launch::{DosBoxLaunchIdentity, UserApprovedDosBoxTarget};
    let fixture = ready_fixture("approved-target", "Keen", "#!/bin/sh\nexit 0\n");
    let review = review_dosbox_game_directory(&fixture.game_directory).unwrap();
    let start = &review.start.as_ref().unwrap().program_file;
    let target =
        UserApprovedDosBoxTarget::approve(&review, &fixture.game_directory, start).unwrap();
    let mut request = request_for(&fixture);
    request.identity = DosBoxLaunchIdentity::UserApproved(target);
    assert!(preflight_dosbox_launch(&request).is_ok());
    std::fs::write(start, b"MZ-changed-after-approval").unwrap();
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::StartCommandNotApproved
    );
}

#[test]
fn a_user_approved_target_cannot_be_transplanted_to_another_folder() {
    use crate::launch::{DosBoxLaunchIdentity, UserApprovedDosBoxTarget};
    let first = ready_fixture("target-first", "Keen", "#!/bin/sh\nexit 0\n");
    let second = ready_fixture("target-second", "Keen", "#!/bin/sh\nexit 0\n");
    let review = review_dosbox_game_directory(&first.game_directory).unwrap();
    let target = UserApprovedDosBoxTarget::approve(
        &review,
        &first.game_directory,
        &review.start.as_ref().unwrap().program_file,
    )
    .unwrap();
    let mut request = request_for(&second);
    request.identity = DosBoxLaunchIdentity::UserApproved(target);
    assert_eq!(
        preflight_dosbox_launch(&request).unwrap_err().kind,
        DosBoxLaunchPreflightErrorKind::StartCommandNotApproved
    );
}
