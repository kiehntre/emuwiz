use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

struct Fixture {
    root: tempfile::TempDir,
    folder: PathBuf,
    profile: PathBuf,
    saves: PathBuf,
    original: PathBuf,
}

const ID: &str = "hypno:demo";
const TARGET: &str = "emuwiz-trainer";

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("game");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("resource.dat"), b"synthetic game").unwrap();
        let saves = root.path().join("saves");
        std::fs::create_dir(&saves).unwrap();
        std::fs::write(saves.join("slot1.sav"), b"valuable save fixture").unwrap();
        let original = root.path().join("scummvm.ini");
        std::fs::write(
            &original,
            format!("[scummvm]\nsavepath={}\nfullscreen=true\n", saves.display()),
        )
        .unwrap();
        let profile = root.path().join("profile.ini");
        std::fs::write(
            &profile,
            format!(
                "[{TARGET}]\ngameid=demo\nengineid=hypno\npath={}\ninfiniteHealth=true\ncheats=false\n",
                folder.display()
            ),
        )
        .unwrap();
        Self {
            root,
            folder,
            profile,
            saves,
            original,
        }
    }

    /// Detector answers `--detect`; a launch (first arg `-c`) records what it
    /// was given, then rewrites its configuration the way ScummVM does.
    fn scummvm(&self, detected: &str, exit_code: i32) -> PathBuf {
        let path = self.root.path().join("scummvm-fixture");
        let record = self.root.path().join("launch-record");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-c\" ]; then\n  printf '%s\\n' \"$*\" > '{r}.args'\n  cp \"$2\" '{r}.config'\n  printf 'rewritten=by-scummvm\\n' >> \"$2\"\n  echo 'fixture stderr' >&2\n  exit {exit_code}\nfi\nprintf 'Game ID: %s\\n' '{detected}'\n",
                r = record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The detector really execs this file; wait out a transient ETXTBSY.
        for _ in 0..10_000 {
            match std::process::Command::new(&path).arg("--detect").output() {
                Ok(_) => break,
                Err(e) if e.raw_os_error() == Some(26) => std::thread::yield_now(),
                Err(e) => panic!("fixture not spawnable: {e}"),
            }
        }
        path
    }

    fn request(&self, executable: PathBuf) -> TrainerLaunchRequest {
        let summary = match inspect_trainer_profile(&self.profile, TARGET, ID, &self.folder) {
            TrainerProfileState::Ready(summary) => summary,
            other => panic!("fixture profile not ready: {other:?}"),
        };
        TrainerLaunchRequest {
            selected_game_folder: self.folder.clone(),
            expected_game_key: ID.into(),
            expected_executable: executable,
            profile: self.profile.clone(),
            target_name: TARGET.into(),
            expected_profile_sha256: summary.sha256,
            original_configuration: Some(self.original.clone()),
            default_savepath: None,
            scratch_root: self.root.path().join("scratch"),
        }
    }

    fn record(&self, suffix: &str) -> PathBuf {
        self.root.path().join(format!("launch-record.{suffix}"))
    }

    fn scratch_entries(&self) -> usize {
        std::fs::read_dir(self.root.path().join("scratch")).map_or(0, Iterator::count)
    }
}

fn wait_for_exit(launched: &mut LaunchedTrainerScummVm) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while launched.poll().is_none() {
        assert!(Instant::now() < deadline, "fixture process never exited");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn inspection_reports_missing_ready_unsupported_and_mismatched_profiles() {
    let f = Fixture::new();
    let missing = f.root.path().join("absent.ini");
    assert_eq!(
        inspect_trainer_profile(&missing, TARGET, ID, &f.folder),
        TrainerProfileState::Missing
    );
    let TrainerProfileState::Ready(summary) =
        inspect_trainer_profile(&f.profile, TARGET, ID, &f.folder)
    else {
        panic!("expected a ready profile");
    };
    assert_eq!(
        summary.options,
        vec![
            ("cheats".to_string(), false),
            ("infiniteHealth".to_string(), true)
        ]
    );
    let good = std::fs::read_to_string(&f.profile).unwrap();
    for (changed, needle) in [
        (format!("{good}scalerMode=hq\n"), "scalerMode"),
        (format!("{good}[other]\nkeep=1\n"), "[other]"),
        (
            good.replace("infiniteHealth=true", "infiniteHealth=maybe"),
            "not true or false",
        ),
        (format!("{good}cheats=true\n"), "duplicate"),
    ] {
        std::fs::write(&f.profile, changed).unwrap();
        let TrainerProfileState::Unsupported(items) =
            inspect_trainer_profile(&f.profile, TARGET, ID, &f.folder)
        else {
            panic!("expected unsupported for {needle}");
        };
        assert!(items.iter().any(|i| i.contains(needle)), "{items:?}");
    }
    std::fs::write(&f.profile, good).unwrap();
    assert!(matches!(
        inspect_trainer_profile(&f.profile, TARGET, ID, Path::new("/some/other/game")),
        TrainerProfileState::Mismatch(_)
    ));
    assert!(matches!(
        inspect_trainer_profile(&f.profile, TARGET, "scumm:monkey", &f.folder),
        TrainerProfileState::Unsupported(_)
    ));
}

#[test]
fn symlinked_profile_is_unreadable_not_followed() {
    let f = Fixture::new();
    let link = f.root.path().join("link.ini");
    std::os::unix::fs::symlink(&f.profile, &link).unwrap();
    assert!(matches!(
        inspect_trainer_profile(&link, TARGET, ID, &f.folder),
        TrainerProfileState::Unreadable(_)
    ));
}

#[test]
fn launch_uses_a_private_copy_and_preserves_profile_config_saves_and_game() {
    let f = Fixture::new();
    let request = f.request(f.scummvm(ID, 0));
    let profile_before = std::fs::read(&f.profile).unwrap();
    let original_before = std::fs::read(&f.original).unwrap();
    let mut launched = preflight_and_launch_with_trainer_profile(&request).unwrap();
    wait_for_exit(&mut launched);
    let report = launched.poll().unwrap();
    assert!(report.status.as_ref().unwrap().success());
    assert!(String::from_utf8_lossy(&report.stderr).contains("fixture stderr"));

    let args = std::fs::read_to_string(f.record("args")).unwrap();
    assert!(
        args.contains(&f.root.path().join("scratch").display().to_string()),
        "{args}"
    );
    assert!(!args.contains(&f.profile.display().to_string()));
    assert!(!args.contains(&f.original.display().to_string()));
    let seen = std::fs::read_to_string(f.record("config")).unwrap();
    assert!(seen.contains("infiniteHealth=true"));
    assert!(seen.contains(&format!("savepath={}", f.saves.display())));

    // ScummVM rewrote only its private copy, which is gone after exit.
    assert_eq!(launched.cleanup_result(), Some(&Ok(())));
    assert_eq!(f.scratch_entries(), 0);
    assert_eq!(std::fs::read(&f.profile).unwrap(), profile_before);
    assert_eq!(std::fs::read(&f.original).unwrap(), original_before);
    assert_eq!(
        std::fs::read(f.saves.join("slot1.sav")).unwrap(),
        b"valuable save fixture"
    );
    assert_eq!(
        std::fs::read(f.folder.join("resource.dat")).unwrap(),
        b"synthetic game"
    );
}

#[test]
fn nonzero_exit_is_reported_as_a_failure_not_success() {
    let f = Fixture::new();
    let mut launched =
        preflight_and_launch_with_trainer_profile(&f.request(f.scummvm(ID, 3))).unwrap();
    wait_for_exit(&mut launched);
    let status = launched.poll().unwrap().status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(3));
}

#[test]
fn profile_changed_since_review_is_refused_before_anything_runs() {
    let f = Fixture::new();
    let request = f.request(f.scummvm(ID, 0));
    let changed = std::fs::read_to_string(&f.profile)
        .unwrap()
        .replace("cheats=false", "cheats=true");
    std::fs::write(&f.profile, changed).unwrap();
    let error = preflight_and_launch_with_trainer_profile(&request)
        .err()
        .unwrap();
    assert!(
        matches!(error, TrainerLaunchError::ProfileChanged { .. }),
        "{error}"
    );
    assert!(!f.record("args").exists());
    assert_eq!(f.scratch_entries(), 0);
}

#[test]
fn missing_unsupported_and_mismatched_profiles_are_refused_with_distinct_errors() {
    let f = Fixture::new();
    let request = f.request(f.scummvm(ID, 0));
    let good = std::fs::read_to_string(&f.profile).unwrap();
    std::fs::write(&f.profile, format!("{good}scalerMode=hq\n")).unwrap();
    assert!(matches!(
        preflight_and_launch_with_trainer_profile(&request)
            .err()
            .unwrap(),
        TrainerLaunchError::ProfileUnsupported(_)
    ));
    std::fs::write(
        &f.profile,
        good.replace(&f.folder.display().to_string(), "/elsewhere"),
    )
    .unwrap();
    assert!(matches!(
        preflight_and_launch_with_trainer_profile(&request)
            .err()
            .unwrap(),
        TrainerLaunchError::ProfileMismatch(_)
    ));
    std::fs::remove_file(&f.profile).unwrap();
    assert!(matches!(
        preflight_and_launch_with_trainer_profile(&request)
            .err()
            .unwrap(),
        TrainerLaunchError::ProfileMissing
    ));
    assert!(!f.record("args").exists());
}

#[test]
fn game_identity_drift_is_refused_and_leaves_no_private_copy() {
    let f = Fixture::new();
    let request = f.request(f.scummvm("hypno:other", 0));
    let error = preflight_and_launch_with_trainer_profile(&request)
        .err()
        .unwrap();
    assert!(
        matches!(&error, TrainerLaunchError::Preflight(e)
            if e.kind == super::super::ScummVmLaunchPreflightErrorKind::IdentityMismatch),
        "{error}"
    );
    assert!(!f.record("args").exists());
    assert_eq!(f.scratch_entries(), 0);
}

#[test]
fn unknown_save_location_is_refused_rather_than_guessed() {
    let f = Fixture::new();
    let mut request = f.request(f.scummvm(ID, 0));
    std::fs::write(&f.original, "[scummvm]\nfullscreen=true\n").unwrap();
    assert!(matches!(
        preflight_and_launch_with_trainer_profile(&request)
            .err()
            .unwrap(),
        TrainerLaunchError::SaveLocationUnavailable(_)
    ));
    request.default_savepath = Some(f.root.path().join("no-such-saves"));
    assert!(matches!(
        preflight_and_launch_with_trainer_profile(&request)
            .err()
            .unwrap(),
        TrainerLaunchError::SaveLocationUnavailable(_)
    ));
    request.default_savepath = Some(f.saves.clone());
    let mut launched = preflight_and_launch_with_trainer_profile(&request).unwrap();
    wait_for_exit(&mut launched);
    assert_eq!(launched.savepath, f.saves);
}

#[test]
fn missing_executable_is_a_binding_refusal() {
    let f = Fixture::new();
    let request = f.request(f.root.path().join("no-scummvm"));
    assert!(matches!(
        preflight_and_launch_with_trainer_profile(&request)
            .err()
            .unwrap(),
        TrainerLaunchError::Preflight(_)
    ));
    assert_eq!(f.scratch_entries(), 0);
}
