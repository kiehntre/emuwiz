//! Plain native ScummVM launch from the Playing Library: request building from
//! a real launch plan, then the core's fresh preflight and spawn, against a
//! fake ScummVM script and private temp roots. No real ScummVM, game or user
//! configuration is involved.

use super::*;
use archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport;
use archivefs_core::launch::{
    CanonicalIdentityStatus, LaunchContentRef, ResolvedIdentity, StandaloneProfileInput,
    build_launch_plan,
};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

const KEY: &str = "hypno:demo";

struct Lab {
    dir: tempfile::TempDir,
    folder: PathBuf,
    exe: PathBuf,
    user_config: PathBuf,
}

impl Lab {
    fn new(detected: &str, exit_code: i32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("A Game Folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("resource.dat"), b"synthetic game").unwrap();
        let user_config = dir.path().join("scummvm.ini");
        std::fs::write(&user_config, "[scummvm]\nsavepath=/saves\n").unwrap();
        let exe = dir.path().join("scummvm");
        let lab = Self {
            dir,
            folder,
            exe,
            user_config,
        };
        lab.write_exe(detected, exit_code);
        lab
    }

    /// Detection (`--detect`-style calls) prints the game ID; a launch (first
    /// argument `-p`) records its argv, writes to stderr and exits.
    fn write_exe(&self, detected: &str, exit_code: i32) {
        std::fs::write(
            &self.exe,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-p\" ]; then\n  printf '%s\\n' \"$@\" > \"$0.argv\"\n  echo oops >&2\n  exit {exit_code}\nfi\nprintf 'Game ID: %s\\n' '{detected}'\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&self.exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The detector really execs this file; wait out a transient ETXTBSY.
        for _ in 0..10_000 {
            match std::process::Command::new(&self.exe)
                .arg("--detect")
                .output()
            {
                Ok(_) => break,
                Err(e) if e.raw_os_error() == Some(26) => std::thread::yield_now(),
                Err(e) => panic!("fixture not spawnable: {e}"),
            }
        }
    }

    fn argv(&self) -> PathBuf {
        PathBuf::from(format!("{}.argv", self.exe.display()))
    }

    fn plan(&self, with_profile: bool) -> LaunchPlan {
        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "ScummVM".into(),
            game_key: KEY.into(),
        });
        let content = LaunchContentRef {
            kind: None,
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(self.folder.clone()),
            requires_mount: false,
            provenance: "test".into(),
        };
        let profiles = if with_profile {
            vec![StandaloneProfileInput {
                adapter_id: "scummvm",
                profile_id: format!("scummvm:{}", self.exe.display()),
                profile_path: Some(self.exe.clone()),
                eligible: true,
                firmware: FirmwareReadiness::NotRequired,
            }]
        } else {
            Vec::new()
        };
        let env = RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![],
            diagnostics: vec![],
        };
        build_launch_plan(&identity, &content, &profiles, &env, &[])
    }

    fn request(&self) -> Option<StandaloneLaunchRequest> {
        let plan = self.plan(true);
        standalone_launch_request(
            &plan,
            &plan.candidates[0],
            None,
            None,
            None,
            None,
            None,
            None,
        )
    }
}

fn started(
    request: StandaloneLaunchRequest,
) -> Result<archivefs_core::launch::LaunchedScummVmProcess, String> {
    match request.execute()? {
        StandaloneProcess::ScummVm(process) => Ok(process),
        _ => panic!("a ScummVM request must start a ScummVM process"),
    }
}

fn refused(request: StandaloneLaunchRequest) -> String {
    match started(request) {
        Err(error) => error,
        Ok(_) => panic!("the launch must be refused"),
    }
}

fn wait_line(mut process: archivefs_core::launch::LaunchedScummVmProcess) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(report) = process.poll() {
            return exit_line("ScummVM", report);
        }
        assert!(Instant::now() < deadline, "fake ScummVM did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_verified_folder_and_ready_executable_offer_launch() {
    let lab = Lab::new(KEY, 0);
    let plan = lab.plan(true);
    assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
    let Some(StandaloneLaunchRequest::ScummVm(request)) = lab.request() else {
        panic!("expected a ScummVM request");
    };
    assert_eq!(request.selected_game_folder, lab.folder);
    assert_eq!(request.expected_game_key, KEY);
    assert_eq!(request.expected_executable, lab.exe);
    assert!(request.trainer.is_none());
}

#[test]
fn no_executable_wrong_platform_file_or_link_content_offer_no_launch() {
    let lab = Lab::new(KEY, 0);
    // No installed ScummVM: the planner has no candidate to run.
    let plan = lab.plan(false);
    assert!(
        standalone_launch_request(
            &plan,
            &plan.candidates[0],
            None,
            None,
            None,
            None,
            None,
            None
        )
        .is_none()
    );
    // Wrong platform on an otherwise ready candidate.
    let mut plan = lab.plan(true);
    plan.platform_id = Some("DOS".into());
    assert!(
        standalone_launch_request(
            &plan,
            &plan.candidates[0],
            None,
            None,
            None,
            None,
            None,
            None
        )
        .is_none()
    );
    // A file where a game folder is required.
    let file = lab.dir.path().join("game.zip");
    std::fs::write(&file, b"zip").unwrap();
    let mut plan = lab.plan(true);
    plan.candidates[0].content.resolved_path = Some(file);
    assert!(
        standalone_launch_request(
            &plan,
            &plan.candidates[0],
            None,
            None,
            None,
            None,
            None,
            None
        )
        .is_none()
    );
    // A symlinked folder.
    let link = lab.dir.path().join("link");
    std::os::unix::fs::symlink(&lab.folder, &link).unwrap();
    let mut plan = lab.plan(true);
    plan.candidates[0].content.resolved_path = Some(link);
    assert!(
        standalone_launch_request(
            &plan,
            &plan.candidates[0],
            None,
            None,
            None,
            None,
            None,
            None
        )
        .is_none()
    );
    // A blocked candidate is never turned into a request.
    let mut plan = lab.plan(true);
    plan.candidates[0].readiness = LaunchReadiness::Blocked;
    assert!(
        standalone_launch_request(
            &plan,
            &plan.candidates[0],
            None,
            None,
            None,
            None,
            None,
            None
        )
        .is_none()
    );
}

#[test]
fn launch_runs_fresh_detection_then_starts_with_exact_argv_and_reports_clean_exit() {
    let lab = Lab::new(KEY, 0);
    let before = std::fs::read(&lab.user_config).unwrap();
    let mut state = StandaloneLaunchState::default();
    state.start(lab.request().unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    while state.is_active() {
        assert!(Instant::now() < deadline, "launch never finished");
        state.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    state.poll();
    let Some((_, adapter, StandaloneLaunchStage::Exited(StandaloneProcess::ScummVm(mut process)))) =
        state.tracked.take()
    else {
        panic!("expected an exited ScummVM process");
    };
    assert_eq!(adapter, "scummvm");
    let line = exit_line("ScummVM", process.poll().unwrap());
    assert_eq!(
        line,
        "ScummVM exited normally; this does not prove the game ran."
    );
    let argv = std::fs::read_to_string(lab.argv()).unwrap();
    assert_eq!(argv, format!("-p\n{}\n{KEY}\n", lab.folder.display()));
    // Nothing else is passed: no configuration or save path is named.
    assert!(!argv.contains("-c") && !argv.contains("savepath"));
    assert_eq!(std::fs::read(&lab.user_config).unwrap(), before);
    assert_eq!(
        std::fs::read(lab.folder.join("resource.dat")).unwrap(),
        b"synthetic game"
    );
}

#[test]
fn nonzero_exit_is_reported_as_failure() {
    let lab = Lab::new(KEY, 3);
    let line = wait_line(started(lab.request().unwrap()).unwrap());
    assert!(line.starts_with("ScummVM failed:"), "{line}");
    assert!(line.contains("oops"), "{line}");
    assert!(!line.contains("exited normally"), "{line}");
}

#[test]
fn missing_or_replaced_executable_is_refused_before_anything_starts() {
    let lab = Lab::new(KEY, 0);
    let request = lab.request().unwrap();
    std::fs::remove_file(&lab.exe).unwrap();
    assert!(refused(request.clone()).contains("BindingUnavailable"));
    // Swapped for a symlink to a real executable.
    let real = lab.dir.path().join("real-scummvm");
    std::fs::write(&real, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&real, &lab.exe).unwrap();
    assert!(refused(request.clone()).contains("BindingUnavailable"));
    // Replaced by a non-executable file.
    std::fs::remove_file(&lab.exe).unwrap();
    std::fs::write(&lab.exe, "not executable").unwrap();
    assert!(refused(request).contains("BindingUnavailable"));
    assert!(!lab.argv().exists());
}

#[test]
fn identity_that_no_longer_matches_is_refused() {
    let lab = Lab::new(KEY, 0);
    let request = lab.request().unwrap();
    lab.write_exe("hypno:other", 0);
    assert!(refused(request.clone()).contains("IdentityMismatch"));
    // Detection that no longer yields an ID at all.
    lab.write_exe("", 0);
    assert!(!refused(request).is_empty());
    assert!(!lab.argv().exists());
}

#[test]
fn folder_removed_or_swapped_for_a_link_after_planning_is_refused() {
    let lab = Lab::new(KEY, 0);
    let request = lab.request().unwrap();
    let moved = lab.dir.path().join("moved");
    std::fs::rename(&lab.folder, &moved).unwrap();
    assert!(refused(request.clone()).contains("ContentNotFound"));
    std::os::unix::fs::symlink(&moved, &lab.folder).unwrap();
    assert!(refused(request).contains("ContentIsSymlink"));
    assert!(!lab.argv().exists());
}
