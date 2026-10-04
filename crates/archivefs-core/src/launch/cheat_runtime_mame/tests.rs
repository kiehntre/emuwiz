//! Fake-MAME tests: every fixture is a real temp directory and a shell script
//! stands in for the emulator, blocking until released so lifetime is observable.

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use super::*;
use crate::dat::dependency::{DependencyState, SetDependencyReport};
use crate::dat::set::{SetIdentity, SetResolution, SetState};
use crate::launch::cheat_runtime::CleanupOutcome;
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use crate::launch::process_spawn::CapturedFileIdentity;

const CHEAT_XML: &str = r#"<?xml version="1.0"?><mamecheat version="1"><cheat desc="Free Play"><script state="on"><action>0x2000 = 1</action></script></cheat><cheat desc="Other"><script state="on"><action>0x2001 = 1</action></script></cheat></mamecheat>"#;

struct Fx {
    dir: tempfile::TempDir,
    exe: PathBuf,
}

impl Fx {
    /// `body` runs after argv/cheat evidence is recorded; the default waits for
    /// `<exe>.release`.
    fn new(set: &str) -> (Self, MameLaunchRequest) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fake-mame");
        std::fs::write(
            &exe,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.argv\"\n\
             while [ $# -gt 1 ]; do [ \"$1\" = -cheatpath ] && cp \"$2\"/*.xml \"$0.seen\" 2>/dev/null; shift; done\n\
             : > \"$0.started\"\n\
             while [ ! -e \"$0.release\" ]; do sleep 0.02; done\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let content = dir.path().join("rom.zip");
        std::fs::write(&content, b"verified set placeholder").unwrap();
        let request = MameLaunchRequest {
            identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "Arcade".into(),
                game_key: set.into(),
            }),
            set_resolutions: vec![SetResolution {
                identity: SetIdentity {
                    source_id: "mame".into(),
                    game_name: set.into(),
                },
                archive_path: content.clone(),
                state: SetState::Complete,
                members_required: Vec::new(),
                members_verified: Vec::new(),
                members_bad: Vec::new(),
                members_optional: Vec::new(),
                members_borrowed: Vec::new(),
                disks_required: Vec::new(),
                disks_verified: Vec::new(),
                disks_parent_required: Vec::new(),
                dependencies: SetDependencyReport {
                    state: DependencyState::NotApplicable,
                    requirements: Vec::new(),
                },
            }],
            expected_executable: exe.clone(),
            expected_content_identity: Some(CapturedFileIdentity::capture(
                &std::fs::symlink_metadata(&content).unwrap(),
            )),
            rom_search_path: dir.path().to_path_buf(),
            selected_content: content,
        };
        (Self { dir, exe }, request)
    }

    fn roots(&self, name: &str) -> CheatRuntimeRoots {
        CheatRuntimeRoots {
            approved_root: self.dir.path().join(name),
        }
    }

    fn cheat(&self, set: &str, xml: &str) -> MameCheatSelection {
        let staged = self.dir.path().join("staging");
        std::fs::create_dir_all(&staged).unwrap();
        let file = staged.join(format!("{set}.xml"));
        std::fs::write(&file, xml).unwrap();
        MameCheatSelection {
            staged_file: file,
            selected: vec!["Free Play".into()],
        }
    }

    fn argv(&self) -> Vec<String> {
        wait_for(&self.exe.with_extension("started").clone(), &self.exe);
        std::fs::read_to_string(format!("{}.argv", self.exe.display()))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn release(&self) {
        std::fs::write(format!("{}.release", self.exe.display()), b"").unwrap();
    }
}

fn wait_for(_unused: &Path, exe: &Path) {
    let marker = PathBuf::from(format!("{}.started", exe.display()));
    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "fake MAME never started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn finish(session: &mut CheatRuntimeSession<WatchedProcess>) {
    let start = Instant::now();
    while !session.is_finished() {
        session.poll();
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "MAME never exited"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn with_cheats(
    launch: MameCheatLaunch,
) -> (
    Box<CheatRuntimeSession<WatchedProcess>>,
    MameCheatActivation,
) {
    match launch {
        MameCheatLaunch::WithCheats {
            session,
            activation,
        } => (session, activation),
        MameCheatLaunch::Plain(_) => panic!("expected a cheat launch"),
    }
}

#[test]
fn a_launch_without_cheats_is_the_unchanged_plain_launch() {
    let (fx, request) = Fx::new("testset");
    let launch =
        preflight_and_launch_mame_with_cheats(&request, None, "plain1", &fx.roots("ws")).unwrap();
    let MameCheatLaunch::Plain(mut process) = launch else {
        panic!("plain expected")
    };
    let argv = fx.argv();
    assert_eq!(argv.len(), 3);
    assert!(!argv.contains(&"-cheat".to_string()));
    assert!(!fx.dir.path().join("ws").exists());
    fx.release();
    while process.poll().is_none() {
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_cheat_launch_adds_cheat_once_with_the_staged_path_and_keeps_the_workspace_alive() {
    let (fx, request) = Fx::new("testset");
    let cheat = fx.cheat("testset", CHEAT_XML);
    let source_before = std::fs::read(&cheat.staged_file).unwrap();
    let roots = fx.roots("ws with space");
    let (mut session, activation) = with_cheats(
        preflight_and_launch_mame_with_cheats(&request, Some(&cheat), "mame1", &roots).unwrap(),
    );
    let argv = fx.argv();
    let workspace = roots.approved_root.join(workspace_name("mame1"));
    let cheat_dir = workspace.join("cheat");
    assert_eq!(argv.iter().filter(|a| *a == "-cheat").count(), 1);
    assert_eq!(argv[argv.len() - 1], "testset");
    let at = argv.iter().position(|a| a == "-cheatpath").unwrap();
    assert_eq!(argv[at - 1], "-cheat");
    assert_eq!(
        argv[at + 1],
        cheat_dir.to_str().unwrap(),
        "a path with a space is one argument"
    );
    // MAME could read the derivative while running, and it holds only the chosen cheat.
    let seen = std::fs::read_to_string(format!("{}.seen", fx.exe.display())).unwrap();
    assert!(
        seen.contains("Free Play") && !seen.contains("Other"),
        "{seen}"
    );
    // The workspace outlives the launch until the emulator exits.
    assert!(cheat_dir.join("testset.xml").exists());
    assert_eq!(
        session.cleanup_now(),
        CleanupOutcome::DeferredEmulatorRunning
    );
    assert!(workspace.exists());
    fx.release();
    finish(&mut session);
    assert!(!workspace.exists(), "cleaned after exit");
    let receipt = session.receipt();
    assert_eq!(receipt.adapter_id, MAME_ADAPTER_ID);
    assert_eq!(receipt.game_identity, "testset");
    assert_eq!(receipt.cheats.len(), 1);
    assert_eq!(receipt.cheats[0].provider, "mame-xml");
    assert!(!receipt.has_violation(), "{:?}", receipt.errors);
    // Activation is never claimed.
    assert_eq!(
        activation,
        MameCheatActivation::CheatSystemEnabledMaterialAvailableActivationNotProven
    );
    assert!(activation.label().contains("starts switched off"));
    assert!(!activation.label().to_lowercase().contains("activated"));
    assert_eq!(std::fs::read(&cheat.staged_file).unwrap(), source_before);
}

#[test]
fn unresolved_or_wrong_identity_refuses_before_anything_is_created() {
    let (fx, request) = Fx::new("test;set");
    let cheat = fx.cheat("test;set", CHEAT_XML);
    let error =
        preflight_and_launch_mame_with_cheats(&request, Some(&cheat), "bad1", &fx.roots("ws"))
            .err()
            .unwrap();
    assert!(matches!(
        error,
        MameCheatLaunchError::Cheats(MameCheatError::UnresolvedMachine(_))
    ));

    let (fx, request) = Fx::new("testset");
    let other = fx.cheat("galaga", CHEAT_XML);
    let error =
        preflight_and_launch_mame_with_cheats(&request, Some(&other), "bad2", &fx.roots("ws"))
            .err()
            .unwrap();
    assert!(matches!(
        error,
        MameCheatLaunchError::Cheats(MameCheatError::WrongMachine { .. })
    ));
    assert!(!fx.dir.path().join("ws").exists());
    assert!(!PathBuf::from(format!("{}.started", fx.exe.display())).exists());
}

#[test]
fn invalid_material_and_selections_refuse() {
    let (fx, request) = Fx::new("testset");
    let roots = fx.roots("ws");
    let run = |cheat: &MameCheatSelection, id| {
        preflight_and_launch_mame_with_cheats(&request, Some(cheat), id, &roots)
            .err()
            .unwrap()
    };
    let broken = fx.cheat("testset", "<mamecheat><cheat desc=");
    assert!(matches!(
        run(&broken, "m1"),
        MameCheatLaunchError::Cheats(MameCheatError::Material(_))
    ));
    let mut unknown = fx.cheat("testset", CHEAT_XML);
    unknown.selected = vec!["Nope".into()];
    assert!(matches!(
        run(&unknown, "m2"),
        MameCheatLaunchError::Cheats(MameCheatError::UnknownCheat(_))
    ));
    let mut none = fx.cheat("testset", CHEAT_XML);
    none.selected.clear();
    assert!(matches!(
        run(&none, "m3"),
        MameCheatLaunchError::Cheats(MameCheatError::NoCheatSelected)
    ));
    let mut missing = fx.cheat("testset", CHEAT_XML);
    missing.staged_file = fx.dir.path().join("staging/absent/testset.xml");
    assert!(matches!(
        run(&missing, "m4"),
        MameCheatLaunchError::Cheats(MameCheatError::Material(_))
    ));
    let bad_id = fx.cheat("testset", CHEAT_XML);
    assert!(matches!(
        run(&bad_id, "../escape"),
        MameCheatLaunchError::Cheats(MameCheatError::Runtime(_))
    ));
    assert!(!PathBuf::from(format!("{}.started", fx.exe.display())).exists());
    assert!(!roots.approved_root.exists());
}

#[test]
fn a_spawn_failure_cleans_the_workspace_and_reports_no_launch() {
    let (fx, request) = Fx::new("testset");
    // Passes preflight (regular, executable) but cannot be exec'd.
    std::fs::write(&fx.exe, "#!/nonexistent/interpreter\n").unwrap();
    let cheat = fx.cheat("testset", CHEAT_XML);
    let roots = fx.roots("ws");
    let error = preflight_and_launch_mame_with_cheats(&request, Some(&cheat), "fail1", &roots)
        .err()
        .unwrap();
    assert!(matches!(
        error,
        MameCheatLaunchError::Cheats(MameCheatError::Runtime(_))
    ));
    assert!(!roots.approved_root.join(workspace_name("fail1")).exists());
}

#[test]
fn two_simultaneous_launches_keep_separate_workspaces() {
    let (fx, request) = Fx::new("testset");
    let cheat = fx.cheat("testset", CHEAT_XML);
    let roots = fx.roots("ws");
    let (mut a, _) = with_cheats(
        preflight_and_launch_mame_with_cheats(&request, Some(&cheat), "a1", &roots).unwrap(),
    );
    fx.argv();
    std::fs::remove_file(format!("{}.started", fx.exe.display())).unwrap();
    let (mut b, _) = with_cheats(
        preflight_and_launch_mame_with_cheats(&request, Some(&cheat), "b1", &roots).unwrap(),
    );
    let dir_a = roots.approved_root.join(workspace_name("a1"));
    let dir_b = roots.approved_root.join(workspace_name("b1"));
    assert_ne!(dir_a, dir_b);
    assert!(dir_a.join("cheat/testset.xml").exists() && dir_b.join("cheat/testset.xml").exists());
    // Reusing a live id is refused rather than shared.
    assert!(preflight_and_launch_mame_with_cheats(&request, Some(&cheat), "a1", &roots).is_err());
    assert!(
        dir_a.exists(),
        "the refused duplicate did not delete the live workspace"
    );
    fx.release();
    finish(&mut a);
    finish(&mut b);
    assert!(!dir_a.exists() && !dir_b.exists());
}

/// Real-MAME probe: `EMUWIZ_REAL_MAME=/path/to/mame cargo test ... -- --ignored real_mame`.
/// A wrapper only adds isolation flags (scratch ini/cfg/nvram, no video/sound,
/// 3 seconds); the cheat argv comes from the adapter. `pong` is a discrete
/// machine that needs no ROM, so no real ROM or user config is touched.
#[test]
#[ignore = "needs a real MAME: set EMUWIZ_REAL_MAME"]
fn real_mame_loads_the_staged_cheat_file_and_the_workspace_is_cleaned() {
    let Some(mame) = std::env::var_os("EMUWIZ_REAL_MAME") else {
        return;
    };
    let (fx, request) = Fx::new("pong");
    let scratch = fx.dir.path().join("mame-scratch");
    for d in ["ini", "cfg", "nv", "snap", "diff", "comment"] {
        std::fs::create_dir_all(scratch.join(d)).unwrap();
    }
    let log = fx.dir.path().join("mame.log");
    std::fs::write(
        &fx.exe,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.argv\"\nexec '{}' -inipath '{s}/ini' -cfg_directory '{s}/cfg' -nvram_directory '{s}/nv' -snapshot_directory '{s}/snap' -diff_directory '{s}/diff' -comment_directory '{s}/comment' -video none -sound none -seconds_to_run 3 -nothrottle -verbose \"$@\" > '{}' 2>&1\n",
            Path::new(&mame).display(),
            log.display(),
            s = scratch.display()
        ),
    )
    .unwrap();
    let mut cheat = fx.cheat("pong", CHEAT_XML);
    cheat.selected = vec!["Free Play".into()];
    let roots = fx.roots("ws");
    let (mut session, _) = with_cheats(
        preflight_and_launch_mame_with_cheats(&request, Some(&cheat), "real1", &roots).unwrap(),
    );
    finish(&mut session);
    let output = std::fs::read_to_string(&log).unwrap();
    let workspace = roots.approved_root.join(workspace_name("real1"));
    let wanted = format!(
        "Loading cheats file from {}/cheat/pong.xml",
        workspace.display()
    );
    assert!(
        output.contains(&wanted),
        "MAME did not report loading the staged file:\n{output}"
    );
    let exit = session
        .receipt()
        .process
        .as_ref()
        .unwrap()
        .exit
        .as_ref()
        .unwrap();
    assert!(exit.success, "{exit:?}");
    assert!(
        !workspace.exists(),
        "workspace (including MAME's output.xml) is cleaned"
    );
    assert!(
        !session.receipt().has_violation(),
        "{:?}",
        session.receipt().errors
    );
}
