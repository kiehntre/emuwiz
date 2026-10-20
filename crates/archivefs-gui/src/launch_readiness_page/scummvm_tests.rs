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
                "#!/bin/sh\nif [ \"$1\" = \"-p\" ] || [ \"$1\" = \"-c\" ]; then\n  printf '%s\\n' \"$@\" > \"$0.argv\"\n  if [ \"$1\" = \"-c\" ]; then cp \"$2\" \"$0.config\"; printf 'rewritten=1\\n' >> \"$2\"; fi\n  echo oops >&2\n  exit {exit_code}\nfi\nprintf 'Game ID: %s\\n' '{detected}'\n"
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

// ---- optional saved trainer profile ------------------------------------

use super::scummvm_profile::Environment;
use archivefs_core::launch::TrainerProfileState;

impl Lab {
    /// A saved profile plus the user's own config and saves, all private.
    fn with_profile(&self, extra: &str) -> Environment {
        let saves = self.dir.path().join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("slot1.sav"), b"valuable save fixture").unwrap();
        std::fs::write(
            &self.user_config,
            format!("[scummvm]\nsavepath={}\nfullscreen=true\n", saves.display()),
        )
        .unwrap();
        let profile = self.dir.path().join("owned-profile.ini");
        std::fs::write(
            &profile,
            format!(
                "[emuwiz-trainer]\nengineid=hypno\ngameid=demo\npath={}\ninfiniteHealth=true\n{extra}",
                self.folder.display()
            ),
        )
        .unwrap();
        Environment {
            profile,
            original_configuration: Some(self.user_config.clone()),
            default_savepath: None,
            scratch_root: self.dir.path().join("scratch"),
        }
    }

    fn state(&self, env: Environment) -> StandaloneLaunchState {
        let mut state = StandaloneLaunchState::default();
        state.scummvm_profile.env_override = Some(env);
        state
    }

    fn plain(&self) -> archivefs_core::launch::ScummVmLaunchRequest {
        let Some(StandaloneLaunchRequest::ScummVm(r)) = self.request() else {
            panic!("expected a ScummVM request");
        };
        r
    }
}

fn finish(state: &mut StandaloneLaunchState) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while state.is_active() {
        assert!(Instant::now() < deadline, "launch never finished");
        state.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    state.poll();
}

fn exited_trainer_line(state: &mut StandaloneLaunchState) -> (String, Option<Result<(), String>>) {
    match state.tracked.take() {
        Some((_, _, StandaloneLaunchStage::Exited(StandaloneProcess::ScummVmTrainer(mut p)))) => {
            let line = exit_line("ScummVM", p.poll().unwrap());
            (line, p.cleanup_result().cloned())
        }
        Some((_, _, StandaloneLaunchStage::Failed(e))) => (e, None),
        _ => panic!("unexpected launch state"),
    }
}

#[test]
fn profile_is_never_used_unless_checked_and_explicitly_ticked() {
    let lab = Lab::new(KEY, 0);
    let mut state = lab.state(lab.with_profile(""));
    // Not checked: a plain launch.
    state.start(lab.request().unwrap());
    finish(&mut state);
    assert!(matches!(
        state.tracked.take(),
        Some((
            _,
            _,
            StandaloneLaunchStage::Exited(StandaloneProcess::ScummVm(_))
        ))
    ));
    assert!(
        std::fs::read_to_string(lab.argv())
            .unwrap()
            .starts_with("-p\n")
    );
    // Checked and Ready but not ticked: still plain.
    state.scummvm_profile.check(&lab.plain());
    state.start(lab.request().unwrap());
    finish(&mut state);
    assert!(matches!(
        state.tracked.take(),
        Some((
            _,
            _,
            StandaloneLaunchStage::Exited(StandaloneProcess::ScummVm(_))
        ))
    ));
}

#[test]
fn ticked_profile_launches_on_a_private_copy_and_preserves_everything() {
    let lab = Lab::new(KEY, 0);
    let env = lab.with_profile("");
    let (profile_before, ini_before) = (
        std::fs::read(&env.profile).unwrap(),
        std::fs::read(&lab.user_config).unwrap(),
    );
    let mut state = lab.state(env.clone());
    state.scummvm_profile.check(&lab.plain());
    state.scummvm_profile.set_use_profile(true);
    state.start(lab.request().unwrap());
    finish(&mut state);
    let (line, cleanup) = exited_trainer_line(&mut state);
    assert_eq!(
        line,
        "ScummVM exited normally; this does not prove the game ran."
    );
    assert_eq!(cleanup, Some(Ok(())));
    let argv = std::fs::read_to_string(lab.argv()).unwrap();
    assert!(argv.starts_with("-c\n"), "{argv}");
    assert!(argv.contains(&env.scratch_root.display().to_string()));
    assert!(!argv.contains(&env.profile.display().to_string()));
    assert!(!argv.contains(&lab.user_config.display().to_string()));
    let seen = std::fs::read_to_string(format!("{}.config", lab.exe.display())).unwrap();
    assert!(seen.contains("infiniteHealth=true") && seen.contains("savepath="));
    assert_eq!(std::fs::read(&env.profile).unwrap(), profile_before);
    assert_eq!(std::fs::read(&lab.user_config).unwrap(), ini_before);
    assert_eq!(
        std::fs::read(lab.dir.path().join("saves/slot1.sav")).unwrap(),
        b"valuable save fixture"
    );
    assert_eq!(std::fs::read_dir(&env.scratch_root).unwrap().count(), 0);
    // The tick is spent: the next launch is plain again until re-checked.
    state.start(lab.request().unwrap());
    finish(&mut state);
    assert!(
        std::fs::read_to_string(lab.argv())
            .unwrap()
            .starts_with("-p\n")
    );
}

#[test]
fn missing_unsupported_and_stale_profiles_are_reported_and_cannot_be_ticked() {
    let lab = Lab::new(KEY, 0);
    let env = lab.with_profile("scalerMode=hq\n");
    let mut state = lab.state(env.clone());
    state.scummvm_profile.check(&lab.plain());
    let TrainerProfileState::Unsupported(items) = inspect_state(&state, &lab) else {
        panic!("expected unsupported");
    };
    assert!(items.iter().any(|i| i.contains("scalerMode")));
    state.scummvm_profile.set_use_profile(true);
    state.start(lab.request().unwrap());
    finish(&mut state);
    assert!(
        std::fs::read_to_string(lab.argv())
            .unwrap()
            .starts_with("-p\n")
    );
    // Stale: bound to another folder.
    let text = std::fs::read_to_string(&env.profile)
        .unwrap()
        .replace("scalerMode=hq\n", "");
    std::fs::write(
        &env.profile,
        text.replace(&lab.folder.display().to_string(), "/elsewhere"),
    )
    .unwrap();
    state.scummvm_profile.check(&lab.plain());
    assert!(matches!(
        inspect_state(&state, &lab),
        TrainerProfileState::Mismatch(_)
    ));
    // Missing.
    std::fs::remove_file(&env.profile).unwrap();
    state.scummvm_profile.check(&lab.plain());
    assert_eq!(inspect_state(&state, &lab), TrainerProfileState::Missing);
}

fn inspect_state(state: &StandaloneLaunchState, lab: &Lab) -> TrainerProfileState {
    state
        .scummvm_profile
        .checked_state(&lab.plain())
        .expect("a check for this game")
}

#[test]
fn profile_changed_after_review_is_refused_before_scummvm_starts() {
    let lab = Lab::new(KEY, 0);
    let env = lab.with_profile("");
    let mut state = lab.state(env.clone());
    state.scummvm_profile.check(&lab.plain());
    state.scummvm_profile.set_use_profile(true);
    let text = std::fs::read_to_string(&env.profile)
        .unwrap()
        .replace("true", "false");
    std::fs::write(&env.profile, text).unwrap();
    state.start(lab.request().unwrap());
    finish(&mut state);
    let (message, _) = exited_trainer_line(&mut state);
    assert!(message.contains("changed since you reviewed"), "{message}");
    assert!(!lab.argv().exists());
}

#[test]
fn identity_drift_and_missing_save_location_refuse_a_ticked_launch() {
    let lab = Lab::new(KEY, 0);
    let env = lab.with_profile("");
    let mut state = lab.state(env.clone());
    state.scummvm_profile.check(&lab.plain());
    state.scummvm_profile.set_use_profile(true);
    lab.write_exe("hypno:other", 0);
    state.start(lab.request().unwrap());
    finish(&mut state);
    let (message, _) = exited_trainer_line(&mut state);
    assert!(message.contains("IdentityMismatch"), "{message}");
    assert_eq!(
        std::fs::read_dir(&env.scratch_root).map_or(0, Iterator::count),
        0
    );

    lab.write_exe(KEY, 0);
    std::fs::write(&lab.user_config, "[scummvm]\nfullscreen=true\n").unwrap();
    state.scummvm_profile.check(&lab.plain());
    state.scummvm_profile.set_use_profile(true);
    state.start(lab.request().unwrap());
    finish(&mut state);
    let (message, _) = exited_trainer_line(&mut state);
    assert!(message.contains("save location"), "{message}");
    assert!(!lab.argv().exists());
}

#[test]
fn ticked_launch_failure_exit_is_reported_as_failure() {
    let lab = Lab::new(KEY, 3);
    let mut state = lab.state(lab.with_profile(""));
    state.scummvm_profile.check(&lab.plain());
    state.scummvm_profile.set_use_profile(true);
    state.start(lab.request().unwrap());
    finish(&mut state);
    let (line, _) = exited_trainer_line(&mut state);
    assert!(line.starts_with("ScummVM failed:"), "{line}");
}

#[test]
fn a_check_for_another_game_is_not_carried_over() {
    let lab = Lab::new(KEY, 0);
    let mut state = lab.state(lab.with_profile(""));
    state.scummvm_profile.check(&lab.plain());
    state.scummvm_profile.set_use_profile(true);
    let mut other = lab.plain();
    other.selected_game_folder = lab.dir.path().join("another folder");
    assert!(state.scummvm_profile.checked_state(&other).is_none());
}
