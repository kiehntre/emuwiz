use super::*;
use archivefs_core::launch::planning::ResolvedIdentity;
use archivefs_core::launch::{DosBoxLaunchPreflightError, DosBoxVariant};
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    game: PathBuf,
    executable: PathBuf,
    bwrap_ok: PathBuf,
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture(autoexec: &str) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("games").join("Game One");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("GAME.EXE"), b"MZ").unwrap();
    std::fs::write(
        game.join("dosbox.conf"),
        format!("[dosbox]\nmemsize=16\n[autoexec]\n{autoexec}"),
    )
    .unwrap();
    let tools = root.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let dosbox = tools.join("dosbox");
    executable(&dosbox, "#!/bin/sh\nexit 0\n");
    let bwrap_ok = tools.join("bwrap");
    executable(&bwrap_ok, "#!/bin/sh\nexit 0\n");
    Fixture {
        _root: root,
        game,
        executable: dosbox,
        bwrap_ok,
    }
}

fn dos() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: DOS_PLATFORM_ID.into(),
        game_key: "dos:game-one".into(),
    })
}

fn binding(f: &Fixture) -> Result<DosBoxNativeLaunchBinding, DosBoxBindingRefusal> {
    Ok(DosBoxNativeLaunchBinding {
        executable: f.executable.clone(),
        variant: DosBoxVariant::Staging,
    })
}

fn reasons(check: DosBoxCheck) -> Vec<String> {
    match check {
        DosBoxCheck::Refused(reasons) => {
            assert!(!reasons.is_empty());
            reasons
        }
        DosBoxCheck::Ready(_) => panic!("expected a refusal"),
    }
}

const CLEAN: &str = "mount c .\nc:\ngame.exe\nexit\n";

#[test]
fn a_clean_folder_with_a_working_sandbox_is_offered_for_approval_only() {
    let f = fixture(CLEAN);
    let check = check_dosbox_game(
        &f.game.join("GAME.EXE"),
        &dos(),
        binding(&f),
        DosBoxHostAccess::none(),
        &f.bwrap_ok,
    );
    let DosBoxCheck::Ready(ready) = &check else {
        panic!("expected ready, got {check:?}");
    };
    assert_eq!(ready.review.game_directory, f.game);
    assert_eq!(
        ready.review.start.as_ref().unwrap().program_file,
        f.game.join("GAME.EXE")
    );

    // Nothing can be launched until the user approves the review on screen.
    let mut panel = Panel {
        key: Some((f.game.clone(), dos())),
        result: Some(check),
        ..Panel::default()
    };
    assert!(panel.request().is_none());
    assert!(panel.approve());
    let Some(StandaloneLaunchRequest::DosBox(request)) = panel.request() else {
        panic!("expected a DOSBox request after approval");
    };
    assert_eq!(request.game_directory, f.game);
    assert_eq!(request.expected_executable, f.executable);
    assert_eq!(request.expected_config_path, f.game.join("dosbox.conf"));
    assert_eq!(request.approval.start_command, "game.exe");
    assert!(matches!(
        request.identity,
        archivefs_core::launch::DosBoxLaunchIdentity::UserApproved(_)
    ));
}

#[test]
fn an_approval_is_never_minted_from_a_refused_check() {
    let mut panel = Panel {
        key: Some((PathBuf::from("/x"), dos())),
        result: Some(refused("no")),
        ..Panel::default()
    };
    assert!(!panel.approve());
    assert!(panel.request().is_none());
}

#[test]
fn an_unverified_or_non_dos_identity_is_refused_before_anything_is_read() {
    let f = fixture(CLEAN);
    for identity in [
        CanonicalIdentityStatus::Conflicting,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "ScummVM".into(),
            game_key: "x".into(),
        }),
    ] {
        let text = reasons(check_dosbox_game(
            &f.game,
            &identity,
            binding(&f),
            DosBoxHostAccess::none(),
            &f.bwrap_ok,
        ));
        assert!(
            text[0].contains("not verified this game as a DOS game"),
            "{text:?}"
        );
    }
}

#[test]
fn a_folder_without_dosbox_conf_is_refused_in_words() {
    let f = fixture(CLEAN);
    std::fs::remove_file(f.game.join("dosbox.conf")).unwrap();
    let text = reasons(check_dosbox_game(
        &f.game,
        &dos(),
        binding(&f),
        DosBoxHostAccess::none(),
        &f.bwrap_ok,
    ));
    assert!(
        text[0].contains("No folder with a usable dosbox.conf"),
        "{text:?}"
    );
}

#[test]
fn an_unsafe_autoexec_mount_is_refused_with_the_review_explanation() {
    let f = fixture("mount c /\nc:\ngame.exe\n");
    let text = reasons(check_dosbox_game(
        &f.game,
        &dos(),
        binding(&f),
        DosBoxHostAccess::none(),
        &f.bwrap_ok,
    ));
    assert!(
        text.iter()
            .any(|reason| reason.contains("mounts something unsafe")),
        "{text:?}"
    );
}

#[test]
fn a_missing_sandbox_refuses_and_never_falls_back() {
    let f = fixture(CLEAN);
    let missing = f.game.parent().unwrap().join("no-bwrap");
    let text = reasons(check_dosbox_game(
        &f.game,
        &dos(),
        binding(&f),
        DosBoxHostAccess::none(),
        &missing,
    ));
    assert!(
        text.iter()
            .any(|reason| reason.contains("only runs inside the bubblewrap sandbox")),
        "{text:?}"
    );
}

#[test]
fn a_failing_sandbox_probe_is_refused() {
    let f = fixture(CLEAN);
    let failing = f.game.parent().unwrap().join("bwrap-denied");
    executable(
        &failing,
        "#!/bin/sh\necho 'setting up uid map: Permission denied' >&2\nexit 1\n",
    );
    let text = reasons(check_dosbox_game(
        &f.game,
        &dos(),
        binding(&f),
        DosBoxHostAccess::none(),
        &failing,
    ));
    assert!(
        text.iter()
            .any(|reason| reason.contains("Permission denied")
                && reason.contains("never runs DOSBox without the sandbox")),
        "{text:?}"
    );
}

#[test]
fn missing_dosbox_is_explained_once_and_says_what_to_install() {
    let f = fixture(CLEAN);
    let text = reasons(check_dosbox_game(
        &f.game,
        &dos(),
        Err(DosBoxBindingRefusal::ExecutableUnavailable(
            "no DOSBox or DOSBox Staging executable was found".into(),
        )),
        DosBoxHostAccess::none(),
        &f.bwrap_ok,
    ));
    assert_eq!(text.len(), 1, "{text:?}");
    assert!(text[0].contains("Install DOSBox Staging"), "{text:?}");
}

#[test]
fn a_dosbox_inside_the_game_folder_is_refused_by_the_sandbox_grant_check() {
    let f = fixture(CLEAN);
    let inside = f.game.join("dosbox");
    executable(&inside, "#!/bin/sh\nexit 0\n");
    let text = reasons(check_dosbox_game(
        &f.game,
        &dos(),
        Ok(DosBoxNativeLaunchBinding {
            executable: inside,
            variant: DosBoxVariant::Staging,
        }),
        DosBoxHostAccess::none(),
        &f.bwrap_ok,
    ));
    assert!(
        text.iter()
            .any(|reason| reason.starts_with("The sandbox refused")),
        "{text:?}"
    );
}

#[test]
fn launch_refusals_read_as_sentences_without_debug_syntax() {
    let text = dosbox_refusal_text(&DosBoxLaunchExecutionError::Preflight(
        DosBoxLaunchPreflightError {
            kind: DosBoxLaunchPreflightErrorKind::StartCommandNotApproved,
            detail: "dosbox.conf changed since it was approved".into(),
        },
    ));
    assert_eq!(
        text,
        "DOSBox was not started: dosbox.conf changed since it was approved. Something you \
         approved has changed. Check again and review the folder before approving it again."
    );
    let sandbox = dosbox_refusal_text(&DosBoxLaunchExecutionError::Spawn(
        DosBoxLaunchSpawnError::Sandbox(DosBoxSandboxError::Unavailable(
            "/usr/bin/bwrap: bubblewrap is not installed".into(),
        )),
    ));
    assert!(sandbox.starts_with("DOSBox was not started. DOSBox only runs inside"));
    for text in [text, sandbox] {
        assert!(
            !text.contains("{") && !text.contains("Preflight("),
            "{text}"
        );
    }
}

#[test]
fn shared_access_is_described_and_sound_is_never_claimed() {
    assert!(shared_with_dosbox(&DosBoxHostAccess::none()).contains("No display"));
    let access = DosBoxHostAccess {
        read_only_paths: vec![PathBuf::from("/run/user/1000/wayland-0")],
        environment: vec![("WAYLAND_DISPLAY".into(), "wayland-0".into())],
    };
    let text = shared_with_dosbox(&access);
    assert!(text.contains("Wayland display"));
    assert!(text.contains("Not shared: sound, network"));
}

/// Real end-to-end run on a host with `/usr/bin/bwrap` and an installed
/// DOSBox: check, approve, launch through the shared standalone executor and
/// observe the sandboxed process exit. No display is granted, so DOSBox may
/// exit with an error; only the process result is asserted to be observed.
#[test]
#[ignore = "needs bubblewrap and DOSBox installed on the host"]
fn host_end_to_end_check_approve_launch_and_exit() {
    let f = fixture(CLEAN);
    let check = check_dosbox_game(
        &f.game,
        &dos(),
        resolve_dosbox_native_launch_binding(),
        DosBoxHostAccess::none(),
        Path::new(BWRAP_PATH),
    );
    assert!(matches!(check, DosBoxCheck::Ready(_)), "{check:?}");
    let mut panel = Panel {
        key: Some((f.game.clone(), dos())),
        result: Some(check),
        ..Panel::default()
    };
    assert!(panel.approve());
    let request = panel.request().unwrap();
    let mut process = match request.execute() {
        Ok(super::super::StandaloneProcess::DosBox(process)) => process,
        Ok(_) => panic!("not a DOSBox process"),
        Err(error) => panic!("refused: {error}"),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while process.poll().is_none() {
        assert!(std::time::Instant::now() < deadline, "DOSBox did not exit");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let line = super::super::exit_line("DOSBox", process.poll().unwrap());
    eprintln!("{line}");
    assert!(line.starts_with("DOSBox "));
    // The game folder is untouched apart from what the game itself may write.
    assert!(f.game.join("GAME.EXE").is_file());
}

#[test]
fn an_unknown_release_can_be_reviewed_but_needs_explicit_target_approval() {
    let f = fixture(CLEAN);
    let check = check_dosbox_game(
        &f.game,
        &CanonicalIdentityStatus::Unknown,
        binding(&f),
        DosBoxHostAccess::none(),
        &f.bwrap_ok,
    );
    assert!(matches!(check, DosBoxCheck::Ready(_)), "{check:?}");
    let mut panel = Panel {
        focus: Some(f.game.clone()),
        key: Some((f.game.clone(), CanonicalIdentityStatus::Unknown)),
        result: Some(check),
        ..Panel::default()
    };
    assert!(panel.request().is_none());
    assert!(panel.approve());
    let Some(StandaloneLaunchRequest::DosBox(request)) = panel.request() else {
        panic!()
    };
    assert!(matches!(
        request.identity,
        archivefs_core::launch::DosBoxLaunchIdentity::UserApproved(_)
    ));
    panel.focus(Some(&f.game.join("different.exe")));
    assert!(panel.request().is_none());
    panel.focus(Some(&f.game));
    assert!(
        panel.request().is_none(),
        "returning to the game must not revive approval"
    );
}

#[test]
fn a_missing_start_executable_has_a_named_refusal() {
    let f = fixture(CLEAN);
    std::fs::remove_file(f.game.join("GAME.EXE")).unwrap();
    let text = reasons(check_dosbox_game(
        &f.game,
        &CanonicalIdentityStatus::Unknown,
        binding(&f),
        DosBoxHostAccess::none(),
        &f.bwrap_ok,
    ));
    assert!(
        text.iter()
            .any(|r| r.contains("start program that cannot be used")),
        "{text:?}"
    );
}

#[test]
fn a_folder_below_a_linked_parent_is_refused_before_approval_is_offered() {
    let f = fixture(CLEAN);
    let aliases = tempfile::tempdir().unwrap();
    let alias = aliases.path().join("linked-parent");
    std::os::unix::fs::symlink(f.game.parent().unwrap(), &alias).unwrap();
    let folder = alias.join(f.game.file_name().unwrap());
    let text = reasons(check_dosbox_game(
        &folder,
        &CanonicalIdentityStatus::Unknown,
        binding(&f),
        DosBoxHostAccess::none(),
        &f.bwrap_ok,
    ));
    assert!(text.iter().any(|r| r.contains("symbolic link")), "{text:?}");
}
