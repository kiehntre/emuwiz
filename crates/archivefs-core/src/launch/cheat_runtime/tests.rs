//! Generic backbone tests. Every fixture is a real temp directory; a fake
//! process stands in for the emulator so each exit path can be driven.

use std::cell::Cell;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::launch::cheat_launch_plan::{FingerprintKind, LaunchStateClass};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Env {
    base: PathBuf,
    roots: CheatRuntimeRoots,
}

impl Env {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "emuwiz-cheat-runtime-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let approved = base.join("approved");
        Self {
            roots: CheatRuntimeRoots {
                approved_root: approved,
            },
            base,
        }
    }

    fn user_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.base.join("user").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn root_for(&self, id: &str) -> PathBuf {
        self.roots.approved_root.join(workspace_name(id))
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Restore permissions a test may have removed, then delete the temp tree.
        fn open_up(path: &Path) {
            if let Ok(meta) = fs::symlink_metadata(path) {
                if meta.is_dir() && !meta.file_type().is_symlink() {
                    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                    if let Ok(entries) = fs::read_dir(path) {
                        for entry in entries.flatten() {
                            open_up(&entry.path());
                        }
                    }
                }
            }
        }
        open_up(&self.base);
        let _ = fs::remove_dir_all(&self.base);
    }
}

struct Built {
    plan: CheatRuntimePlan,
    config: PathBuf,
    cheat: PathBuf,
}

fn cheat_info() -> ReceiptCheat {
    ReceiptCheat {
        logical_id: "lives".into(),
        variant_id: "v1".into(),
        title: "Infinite Lives".into(),
        provider: "local".into(),
        source_id: "a".into(),
    }
}

fn build_with(
    env: &Env,
    id: &str,
    expectations: Vec<LaunchStateExpectation>,
    sources: Vec<SourceBinding>,
) -> Built {
    let root = env.root_for(id);
    let sub = root.join("retroarch");
    let config = sub.join("profile.cfg");
    let cheat = sub.join("cheats").join("Core").join("Game.cht");
    let plan = CheatRuntimePlan::new(CheatRuntimePlanParts {
        launch_id: id.into(),
        adapter_id: "test-emulator".into(),
        game_identity: "game-key".into(),
        root: root.clone(),
        directories: vec![
            sub.clone(),
            sub.join("cheats"),
            sub.join("cheats").join("Core"),
        ],
        files: vec![
            RuntimeFile::new(
                config.clone(),
                b"config_save_on_exit = \"false\"\n".to_vec(),
                RuntimeFileKind::Config,
            ),
            RuntimeFile::new(
                cheat.clone(),
                b"cheats = 1\ncheat0_desc = \"Infinite Lives\"\n".to_vec(),
                RuntimeFileKind::CheatMaterial,
            ),
        ],
        extra_arguments: vec!["--config".into(), config.clone().into_os_string()],
        sources,
        expectations,
        cheats: vec![cheat_info()],
    })
    .unwrap();
    Built {
        plan,
        config,
        cheat,
    }
}

fn build(env: &Env, id: &str) -> Built {
    build_with(env, id, Vec::new(), Vec::new())
}

fn binding() -> LiveLaunchBinding {
    LiveLaunchBinding {
        game_identity: "game-key".into(),
        adapter_id: "test-emulator".into(),
    }
}

/// A fake emulator process whose exit the test controls.
#[derive(Clone)]
struct Fake {
    pid: u32,
    exit: Arc<Mutex<Option<ExitSummary>>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            pid: 4242,
            exit: Arc::new(Mutex::new(None)),
        }
    }

    fn exit_with(&self, success: bool) {
        *self.exit.lock().unwrap() = Some(ExitSummary {
            success,
            code: Some(i32::from(!success)),
        });
    }
}

impl RuntimeProcess for Fake {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn poll_exit(&mut self) -> Option<ExitSummary> {
        self.exit.lock().unwrap().clone()
    }
}

type Args = Vec<OsString>;

/// Runs the executor with a spawn that records the argv it was handed and, at
/// spawn time, what the prepared files contain.
fn run(
    env: &Env,
    built: &Built,
    binding: &LiveLaunchBinding,
    fake: &Fake,
) -> (
    CheatRuntimeSession<Fake>,
    Rc<Cell<bool>>,
    Rc<Mutex<Option<(Args, String, String)>>>,
) {
    let spawned = Rc::new(Cell::new(false));
    let seen: Rc<Mutex<Option<(Args, String, String)>>> = Rc::new(Mutex::new(None));
    let (flag, record) = (spawned.clone(), seen.clone());
    let config = built.config.clone();
    let cheat = built.cheat.clone();
    let fake = fake.clone();
    let session = execute_cheat_runtime(
        &built.plan,
        binding,
        &env.roots,
        vec![
            OsString::from("-L"),
            OsString::from("core"),
            OsString::from("game"),
        ],
        |mut command: Args, extra| {
            command.extend(extra.iter().cloned());
            command
        },
        move |command: Args| {
            flag.set(true);
            *record.lock().unwrap() = Some((
                command,
                fs::read_to_string(&config).unwrap_or_default(),
                fs::read_to_string(&cheat).unwrap_or_default(),
            ));
            Ok(fake)
        },
    );
    (session, spawned, seen)
}

fn tree_names(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(path: &Path, base: &Path, out: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                out.push(p.strip_prefix(base).unwrap().display().to_string());
                if p.is_dir() {
                    walk(&p, base, out);
                }
            }
        }
    }
    walk(path, path, &mut out);
    out.sort();
    out
}

// 1 ---------------------------------------------------------------------

#[test]
fn the_workspace_is_created_only_under_the_approved_root() {
    let env = Env::new("owned-root");
    let built = build(&env, "one");
    let fake = Fake::new();
    let (session, spawned, _) = run(&env, &built, &binding(), &fake);
    assert!(spawned.get());
    let root = session.receipt().workspace_root.clone().unwrap();
    assert_eq!(root, env.root_for("one"));
    assert_eq!(root.parent().unwrap(), env.roots.approved_root);
    assert!(root.join(WORKSPACE_MARKER_NAME).is_file());

    // A plan rooted anywhere else is refused before anything is created.
    let outside = env.base.join("elsewhere").join(workspace_name("two"));
    let plan = CheatRuntimePlan::new(CheatRuntimePlanParts {
        root: outside.clone(),
        directories: vec![],
        files: vec![RuntimeFile::new(
            outside.join("c.cfg"),
            b"x".to_vec(),
            RuntimeFileKind::Config,
        )],
        ..parts_for(&env, "two")
    })
    .unwrap();
    assert_eq!(
        CheatRuntimeWorkspace::materialise(&plan, &binding(), &env.roots).unwrap_err(),
        MaterialiseError::Refused(RuntimeRefusal::WorkspaceOutsideApprovedRoot)
    );
    assert!(!outside.exists() && !env.base.join("elsewhere").exists());
}

fn parts_for(env: &Env, id: &str) -> CheatRuntimePlanParts {
    let root = env.root_for(id);
    CheatRuntimePlanParts {
        launch_id: id.into(),
        adapter_id: "test-emulator".into(),
        game_identity: "game-key".into(),
        root: root.clone(),
        directories: vec![],
        files: vec![RuntimeFile::new(
            root.join("c.cfg"),
            b"x".to_vec(),
            RuntimeFileKind::Config,
        )],
        extra_arguments: vec![],
        sources: vec![],
        expectations: vec![],
        cheats: vec![cheat_info()],
    }
}

// 2, 3, 4 ---------------------------------------------------------------

#[test]
fn a_game_mismatch_refuses_and_creates_nothing() {
    let env = Env::new("game");
    let built = build(&env, "g");
    let wrong = LiveLaunchBinding {
        game_identity: "another-game".into(),
        adapter_id: "test-emulator".into(),
    };
    let (session, spawned, _) = run(&env, &built, &wrong, &Fake::new());
    assert!(!spawned.get());
    assert_eq!(
        session.receipt().materialisation,
        MaterialisationOutcome::Refused {
            reason: RuntimeRefusal::GameMismatch
        }
    );
    assert!(!env.root_for("g").exists());
}

#[test]
fn an_emulator_mismatch_refuses_and_creates_nothing() {
    let env = Env::new("emu");
    let built = build(&env, "e");
    let wrong = LiveLaunchBinding {
        game_identity: "game-key".into(),
        adapter_id: "pcsx2".into(),
    };
    let (session, spawned, _) = run(&env, &built, &wrong, &Fake::new());
    assert!(!spawned.get());
    assert_eq!(
        session.receipt().materialisation,
        MaterialisationOutcome::Refused {
            reason: RuntimeRefusal::EmulatorMismatch
        }
    );
    assert!(!env.root_for("e").exists());
}

#[test]
fn a_changed_plan_or_changed_cheat_evidence_refuses() {
    let env = Env::new("stale");
    // The plan was altered after its digest was computed.
    let mut built = build(&env, "s1");
    built.plan.extra_arguments.push("--extra".into());
    let (session, spawned, _) = run(&env, &built, &binding(), &Fake::new());
    assert!(!spawned.get());
    assert_eq!(
        session.receipt().materialisation,
        MaterialisationOutcome::Refused {
            reason: RuntimeRefusal::StalePlan
        }
    );

    // The source cheat file changed between planning and launch.
    let source = env.user_file("lives.cht", b"original cheat file");
    let sha = sha256_hex(b"original cheat file");
    let bound = build_with(
        &env,
        "s2",
        vec![],
        vec![SourceBinding {
            path: source.clone(),
            expected_sha256: sha.clone(),
        }],
    );
    fs::write(&source, b"edited cheat file").unwrap();
    let (session, spawned, _) = run(&env, &bound, &binding(), &Fake::new());
    assert!(!spawned.get());
    assert_eq!(
        session.receipt().materialisation,
        MaterialisationOutcome::Refused {
            reason: RuntimeRefusal::CheatEvidenceChanged {
                source: "lives.cht".into()
            }
        }
    );
    assert!(!env.root_for("s2").exists());

    // And a missing source is a different, equally closed answer.
    fs::remove_file(&source).unwrap();
    let missing = build_with(
        &env,
        "s3",
        vec![],
        vec![SourceBinding {
            path: source,
            expected_sha256: sha,
        }],
    );
    let (session, spawned, _) = run(&env, &missing, &binding(), &Fake::new());
    assert!(!spawned.get());
    assert!(matches!(
        session.receipt().materialisation,
        MaterialisationOutcome::Refused {
            reason: RuntimeRefusal::CheatMaterialMissing { .. }
        }
    ));
}

// 5, 6 ------------------------------------------------------------------

#[test]
fn traversal_is_refused_at_plan_construction() {
    let env = Env::new("traversal");
    let mut parts = parts_for(&env, "t");
    parts.files[0].path = env.root_for("t").join("..").join("escape.cfg");
    assert!(matches!(
        CheatRuntimePlan::new(parts),
        Err(RuntimeRefusal::PathEscapesWorkspace { .. })
    ));

    let mut parts = parts_for(&env, "t");
    parts.directories = vec![env.base.join("user")];
    assert!(matches!(
        CheatRuntimePlan::new(parts),
        Err(RuntimeRefusal::PathEscapesWorkspace { .. })
    ));

    // A root that is not a normal absolute path is not a workspace at all.
    let mut parts = parts_for(&env, "t");
    parts.root = env
        .roots
        .approved_root
        .join("x")
        .join("..")
        .join(workspace_name("t"));
    parts.files[0].path = parts.root.join("c.cfg");
    assert_eq!(
        CheatRuntimePlan::new(parts).unwrap_err(),
        RuntimeRefusal::WorkspaceOutsideApprovedRoot
    );

    // A launch id cannot smuggle a path.
    let mut parts = parts_for(&env, "t");
    parts.launch_id = "../x".into();
    assert_eq!(
        CheatRuntimePlan::new(parts).unwrap_err(),
        RuntimeRefusal::InvalidLaunchId
    );
}

#[test]
fn a_symlink_cannot_stand_in_for_the_workspace_or_the_approved_root() {
    let env = Env::new("symlink");
    let outside = env.base.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("precious.txt"), b"user data").unwrap();
    fs::create_dir_all(&env.roots.approved_root).unwrap();

    // The workspace path is already a symlink to a user directory.
    let built = build(&env, "link");
    symlink(&outside, env.root_for("link")).unwrap();
    let (session, spawned, _) = run(&env, &built, &binding(), &Fake::new());
    assert!(!spawned.get());
    assert_eq!(
        session.receipt().materialisation,
        MaterialisationOutcome::Refused {
            reason: RuntimeRefusal::WorkspaceExists
        }
    );
    assert_eq!(tree_names(&outside), vec!["precious.txt".to_string()]);

    // The approved root itself is a symlink.
    let other = Env::new("symlink-root");
    let link = other.base.join("approved");
    symlink(&outside, &link).unwrap();
    let built = build(&other, "x");
    let (session, spawned, _) = run(&other, &built, &binding(), &Fake::new());
    assert!(!spawned.get());
    assert_eq!(
        session.receipt().materialisation,
        MaterialisationOutcome::Refused {
            reason: RuntimeRefusal::ApprovedRootUnsafe
        }
    );
    assert_eq!(tree_names(&outside), vec!["precious.txt".to_string()]);
}

#[test]
fn an_existing_workspace_is_never_reused() {
    let env = Env::new("exists");
    let built = build(&env, "dup");
    let (first, _, _) = run(&env, &built, &binding(), &Fake::new());
    let before = tree_names(&env.root_for("dup"));
    let (second, spawned, _) = run(&env, &built, &binding(), &Fake::new());
    assert!(!spawned.get());
    assert_eq!(
        second.receipt().materialisation,
        MaterialisationOutcome::Refused {
            reason: RuntimeRefusal::WorkspaceExists
        }
    );
    assert_eq!(
        tree_names(&env.root_for("dup")),
        before,
        "first launch untouched"
    );
    drop(first);
}

// 7 ---------------------------------------------------------------------

#[test]
fn materialisation_is_deterministic() {
    let env = Env::new("deterministic");
    let a = build(&env, "da");
    let b = build(&env, "db");
    let (sa, _, seen_a) = run(&env, &a, &binding(), &Fake::new());
    let (sb, _, seen_b) = run(&env, &b, &binding(), &Fake::new());
    let names = |s: &CheatRuntimeSession<Fake>| match &s.receipt().materialisation {
        MaterialisationOutcome::Materialised { created } => created.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(names(&sa), names(&sb));
    let (_, config_a, cheat_a) = seen_a.lock().unwrap().clone().unwrap();
    let (_, config_b, cheat_b) = seen_b.lock().unwrap().clone().unwrap();
    assert_eq!((config_a, cheat_a), (config_b, cheat_b));
    assert_eq!(
        tree_names(&env.root_for("da")),
        tree_names(&env.root_for("db"))
    );
}

// 8, 9, 10, 22-like -------------------------------------------------------

#[test]
fn real_config_and_source_media_are_fingerprinted_and_stay_unchanged() {
    let env = Env::new("unchanged");
    let real_config = env.user_file("retroarch.cfg", b"video_driver = \"gl\"\n");
    let rom = env.user_file("Game.sfc", b"synthetic rom bytes");
    let cfg_before = fs::metadata(&real_config).unwrap().modified().unwrap();
    let rom_before = fs::metadata(&rom).unwrap().modified().unwrap();
    let expectations = vec![
        LaunchStateExpectation {
            path: real_config.clone(),
            class: LaunchStateClass::ProtectedConfig,
            expectation: StateExpectation::MustRemainUnchanged,
            fingerprint: FingerprintKind::Sha256,
            severity: ViolationSeverity::Warning,
        },
        LaunchStateExpectation {
            path: rom.clone(),
            class: LaunchStateClass::ReadOnlySource,
            expectation: StateExpectation::MustRemainUnchanged,
            fingerprint: FingerprintKind::FileIdentity,
            severity: ViolationSeverity::Corruption,
        },
    ];
    let built = build_with(&env, "keep", expectations, vec![]);
    let fake = Fake::new();
    let (mut session, _, seen) = run(&env, &built, &binding(), &fake);

    // The prepared config and cheat data existed, with the right content, at
    // the moment of handoff (10).
    let (args, config_text, cheat_text) = seen.lock().unwrap().clone().unwrap();
    assert!(config_text.contains("config_save_on_exit = \"false\""));
    assert!(cheat_text.contains("Infinite Lives"));
    assert_eq!(args.last().unwrap(), &built.config.clone().into_os_string());

    fake.exit_with(true);
    let receipt = session.poll().clone();
    assert!(!receipt.has_violation());
    assert!(
        receipt
            .state_checks
            .iter()
            .all(|c| c.outcome == StateCheckOutcome::Unchanged)
    );
    assert_eq!(fs::read(&real_config).unwrap(), b"video_driver = \"gl\"\n");
    assert_eq!(fs::read(&rom).unwrap(), b"synthetic rom bytes");
    assert_eq!(
        fs::metadata(&real_config).unwrap().modified().unwrap(),
        cfg_before
    );
    assert_eq!(fs::metadata(&rom).unwrap().modified().unwrap(), rom_before);
}

#[test]
fn a_change_to_protected_state_is_reported_as_a_violation() {
    let env = Env::new("violation");
    let real_config = env.user_file("retroarch.cfg", b"original\n");
    let expectations = vec![LaunchStateExpectation {
        path: real_config.clone(),
        class: LaunchStateClass::ProtectedConfig,
        expectation: StateExpectation::MustRemainUnchanged,
        fingerprint: FingerprintKind::Sha256,
        severity: ViolationSeverity::LaunchAffecting,
    }];
    let built = build_with(&env, "viol", expectations, vec![]);
    let fake = Fake::new();
    let (mut session, _, _) = run(&env, &built, &binding(), &fake);
    fs::write(&real_config, b"rewritten by the emulator\n").unwrap();
    fake.exit_with(true);
    let receipt = session.poll().clone();
    assert!(receipt.has_violation());
    assert!(receipt.errors.iter().any(|e| e.contains("retroarch.cfg")));
}

// 11, 12, 13 ------------------------------------------------------------

#[test]
fn the_receipt_records_each_stage_truthfully() {
    let env = Env::new("receipt");
    let built = build(&env, "rec");
    let fake = Fake::new();
    let (mut session, _, _) = run(&env, &built, &binding(), &fake);
    {
        let receipt = session.receipt();
        assert!(matches!(
            receipt.materialisation,
            MaterialisationOutcome::Materialised { .. }
        ));
        assert_eq!(receipt.handoff, HandoffOutcome::PassedToEmulator);
        assert_eq!(
            receipt.stages,
            vec![
                CheatRuntimeStage::Planned,
                CheatRuntimeStage::Materialised,
                CheatRuntimeStage::PassedToEmulator,
                CheatRuntimeStage::ProcessStarted,
                CheatRuntimeStage::RuntimeEffectUnknown,
            ]
        );
        assert_eq!(receipt.process.as_ref().unwrap().pid, 4242);
        // 13: nothing here can know whether a cheat took effect.
        assert_eq!(receipt.runtime_effect, RuntimeEffect::Unknown);
        assert_eq!(
            receipt.highest_stage(),
            CheatRuntimeStage::RuntimeEffectUnknown
        );
        assert_eq!(receipt.cleanup, CleanupOutcome::NotAttempted);
    }
    fake.exit_with(true);
    let receipt = session.poll().clone();
    assert_eq!(
        receipt.runtime_effect,
        RuntimeEffect::Unknown,
        "exit is not proof"
    );
    assert_eq!(receipt.highest_stage(), CheatRuntimeStage::CleanedUp);
    assert_eq!(receipt.process.unwrap().exit.unwrap().success, true);
}

#[test]
fn the_receipt_keeps_no_rom_or_source_paths_and_round_trips() {
    let env = Env::new("redaction");
    let rom = env.user_file("Secret Game Title.sfc", b"rom");
    let source = env.user_file("private cheats.cht", b"cheat file");
    let expectations = vec![LaunchStateExpectation {
        path: rom.clone(),
        class: LaunchStateClass::ReadOnlySource,
        expectation: StateExpectation::MustRemainUnchanged,
        fingerprint: FingerprintKind::FileIdentity,
        severity: ViolationSeverity::Corruption,
    }];
    let built = build_with(
        &env,
        "red",
        expectations,
        vec![SourceBinding {
            path: source,
            expected_sha256: sha256_hex(b"cheat file"),
        }],
    );
    let fake = Fake::new();
    let (mut session, _, _) = run(&env, &built, &binding(), &fake);
    fake.exit_with(true);
    let receipt = session.poll().clone();
    let json = serde_json::to_string(&receipt).unwrap();
    assert!(!json.contains("user/"), "{json}");
    assert!(
        json.contains("Secret Game Title.sfc"),
        "only the file name is kept"
    );
    let back: CheatRuntimeReceipt = serde_json::from_str(&json).unwrap();
    assert_eq!(back, receipt);
}

// 14, 15, 16 ------------------------------------------------------------

#[test]
fn cleanup_removes_the_owned_workspace_after_the_emulator_exits() {
    let env = Env::new("cleanup");
    let built = build(&env, "clean");
    let fake = Fake::new();
    let (mut session, _, _) = run(&env, &built, &binding(), &fake);
    assert!(env.root_for("clean").exists());
    // While the emulator runs, nothing is deleted.
    assert_eq!(
        session.cleanup_now(),
        CleanupOutcome::DeferredEmulatorRunning
    );
    assert!(env.root_for("clean").exists());
    assert!(!session.is_finished());
    fake.exit_with(true);
    assert_eq!(session.poll().cleanup, CleanupOutcome::Completed);
    assert!(!env.root_for("clean").exists());
    assert!(session.is_finished());
    assert!(
        session
            .receipt()
            .stages
            .contains(&CheatRuntimeStage::CleanedUp)
    );
}

#[test]
fn cleanup_never_removes_a_sibling_or_a_user_path() {
    let env = Env::new("siblings");
    let user_dir = env.base.join("user-docs");
    fs::create_dir_all(&user_dir).unwrap();
    fs::write(user_dir.join("keep.txt"), b"mine").unwrap();
    fs::create_dir_all(env.roots.approved_root.join("not-a-workspace")).unwrap();
    fs::write(
        env.roots
            .approved_root
            .join("not-a-workspace")
            .join("keep.txt"),
        b"bios projection",
    )
    .unwrap();
    let built = build(&env, "mine");
    let fake = Fake::new();
    let (mut session, _, _) = run(&env, &built, &binding(), &fake);
    // A symlink inside the workspace pointing at user data: removing the tree
    // removes the link, never what it points to.
    symlink(
        &user_dir,
        env.root_for("mine").join("retroarch").join("escape"),
    )
    .unwrap();
    fake.exit_with(true);
    session.poll();
    assert!(!env.root_for("mine").exists());
    assert_eq!(fs::read(user_dir.join("keep.txt")).unwrap(), b"mine");
    assert!(
        env.roots
            .approved_root
            .join("not-a-workspace")
            .join("keep.txt")
            .exists()
    );
}

#[test]
fn a_cleanup_that_cannot_prove_ownership_fails_visibly_and_deletes_nothing() {
    let env = Env::new("cleanup-fails");
    let built = build(&env, "cf");
    let fake = Fake::new();
    let (mut session, _, _) = run(&env, &built, &binding(), &fake);
    // The marker is replaced by something that names another launch.
    let marker = env.root_for("cf").join(WORKSPACE_MARKER_NAME);
    let mut tampered: WorkspaceMarker =
        serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    tampered.launch_id = "someone-else".into();
    fs::write(&marker, serde_json::to_vec(&tampered).unwrap()).unwrap();
    fake.exit_with(true);
    let receipt = session.poll().clone();
    assert!(matches!(receipt.cleanup, CleanupOutcome::Failed { .. }));
    assert!(receipt.errors.iter().any(|e| e.contains("cleanup failed")));
    assert!(
        env.root_for("cf").exists(),
        "nothing is deleted when ownership is unproven"
    );
    assert!(!receipt.stages.contains(&CheatRuntimeStage::CleanedUp));
}

// 26, 27 (generic) --------------------------------------------------------

#[test]
fn a_failed_materialisation_prevents_the_spawn_and_rolls_back() {
    let env = Env::new("rollback");
    let root = env.root_for("rb");
    // A file whose parent directory is not part of the plan cannot be created.
    let mut parts = parts_for(&env, "rb");
    parts.files.push(RuntimeFile::new(
        root.join("missing-dir").join("late.cfg"),
        b"x".to_vec(),
        RuntimeFileKind::Config,
    ));
    let plan = CheatRuntimePlan::new(parts).unwrap();
    let built = Built {
        config: root.join("c.cfg"),
        cheat: root.join("none"),
        plan,
    };
    let (session, spawned, _) = run(&env, &built, &binding(), &Fake::new());
    assert!(
        !spawned.get(),
        "no process may start after a failed materialisation"
    );
    assert!(matches!(
        session.receipt().materialisation,
        MaterialisationOutcome::Failed {
            rolled_back: true,
            ..
        }
    ));
    assert_eq!(session.receipt().handoff, HandoffOutcome::NotReached);
    assert!(!root.exists(), "everything created was rolled back");
    assert!(session.is_finished());
}

#[test]
fn a_spawn_failure_cleans_up_and_is_reported_truthfully() {
    let env = Env::new("spawn-fail");
    let built = build(&env, "sf");
    let session: CheatRuntimeSession<Fake> = execute_cheat_runtime(
        &built.plan,
        &binding(),
        &env.roots,
        Vec::<OsString>::new(),
        |c: Args, _| c,
        |_| Err("exec failed: not found".to_string()),
    );
    let receipt = session.receipt();
    assert!(matches!(
        receipt.handoff,
        HandoffOutcome::SpawnFailed { .. }
    ));
    assert!(
        !receipt
            .stages
            .contains(&CheatRuntimeStage::PassedToEmulator)
    );
    assert!(!receipt.stages.contains(&CheatRuntimeStage::ProcessStarted));
    assert!(receipt.process.is_none());
    assert_eq!(receipt.cleanup, CleanupOutcome::Completed);
    assert!(!env.root_for("sf").exists());
    assert!(session.is_finished());
}

#[test]
fn a_spawn_failure_whose_cleanup_fails_keeps_recovery_state() {
    let env = Env::new("spawn-fail-keep");
    let built = build(&env, "sk");
    let marker = env.root_for("sk").join(WORKSPACE_MARKER_NAME);
    let session: CheatRuntimeSession<Fake> = execute_cheat_runtime(
        &built.plan,
        &binding(),
        &env.roots,
        Vec::<OsString>::new(),
        |c: Args, _| c,
        |_| {
            // Break ownership proof just before cleanup runs.
            let mut tampered: WorkspaceMarker =
                serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
            assert_eq!(tampered.state, MarkerState::LaunchAttempted);
            tampered.created_by = "someone".into();
            fs::write(&marker, serde_json::to_vec(&tampered).unwrap()).unwrap();
            Err("exec failed".to_string())
        },
    );
    assert!(matches!(
        session.receipt().cleanup,
        CleanupOutcome::Failed { .. }
    ));
    assert!(env.root_for("sk").exists(), "left for explicit recovery");
}

#[test]
fn the_marker_tells_the_truth_before_and_after_the_launch() {
    let env = Env::new("marker");
    let built = build(&env, "mk");
    let fake = Fake::new();
    let (session, _, _) = run(&env, &built, &binding(), &fake);
    let marker: WorkspaceMarker =
        serde_json::from_slice(&fs::read(env.root_for("mk").join(WORKSPACE_MARKER_NAME)).unwrap())
            .unwrap();
    assert_eq!(marker.created_by, "emuwiz");
    assert_eq!(marker.launch_id, "mk");
    assert_eq!(marker.game_identity, "game-key");
    assert_eq!(marker.adapter_id, "test-emulator");
    assert_eq!(marker.cheat_ids, vec!["lives:v1".to_string()]);
    assert_eq!(marker.plan_digest, built.plan.digest());
    assert_eq!(marker.state, MarkerState::EmulatorStarted);
    assert_eq!(marker.emulator_pid, Some(4242));
    assert_eq!(marker.owner_pid, std::process::id());
    drop(session);
}

#[test]
fn files_are_private_to_the_owner() {
    let env = Env::new("modes");
    let built = build(&env, "md");
    let (_session, _, _) = run(&env, &built, &binding(), &Fake::new());
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&env.root_for("md")), 0o700);
    assert_eq!(mode(&built.config), 0o600);
    assert_eq!(mode(&built.cheat), 0o600);
    assert_eq!(mode(built.config.parent().unwrap()), 0o700);
}

// 17, 18: stale detection ---------------------------------------------

/// Creates a real workspace and rewrites its marker as if an earlier EmuWiz
/// process (now gone) had made it.
fn orphan(
    env: &Env,
    id: &str,
    age_secs: u64,
    owner_pid: u32,
    emulator_pid: Option<u32>,
) -> PathBuf {
    let built = build(env, id);
    let (workspace, _) =
        CheatRuntimeWorkspace::materialise(&built.plan, &binding(), &env.roots).unwrap();
    let mut marker = workspace.marker().clone();
    marker.created_unix = now_unix().saturating_sub(age_secs);
    marker.owner_pid = owner_pid;
    marker.owner_start_ticks = None;
    marker.emulator_pid = emulator_pid;
    update_marker(workspace.root(), &marker).unwrap();
    workspace.root().to_path_buf()
}

fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn a_stale_owned_workspace_is_recognised_and_only_then_cleaned_explicitly() {
    let env = Env::new("stale");
    let dead = dead_pid();
    let stale = orphan(&env, "old", 3600, dead, Some(dead));
    let young = orphan(&env, "new", 1, dead, None);
    let live_owner = orphan(&env, "live", 3600, std::process::id(), None);
    let live_emulator = orphan(&env, "emu", 3600, dead, Some(std::process::id()));

    let scan = scan_cheat_runtime_workspaces(&env.roots, now_unix(), DEFAULT_STALE_GRACE_SECS);
    let disposition = |p: &Path| scan.iter().find(|e| e.path == p).unwrap().disposition;
    assert_eq!(disposition(&stale), WorkspaceDisposition::OwnedStale);
    assert_eq!(disposition(&young), WorkspaceDisposition::OwnedYoung);
    assert_eq!(disposition(&live_owner), WorkspaceDisposition::OwnedActive);
    assert_eq!(
        disposition(&live_emulator),
        WorkspaceDisposition::OwnedActive
    );
    // Scanning is read-only.
    assert!(stale.exists() && young.exists() && live_owner.exists() && live_emulator.exists());

    // Only the stale one can be cleaned, and only explicitly.
    for kept in [&young, &live_owner, &live_emulator] {
        assert!(
            cleanup_stale_workspace(&env.roots, kept, now_unix(), DEFAULT_STALE_GRACE_SECS)
                .is_err()
        );
        assert!(kept.exists());
    }
    cleanup_stale_workspace(&env.roots, &stale, now_unix(), DEFAULT_STALE_GRACE_SECS).unwrap();
    assert!(!stale.exists());
    assert!(young.exists() && live_owner.exists() && live_emulator.exists());
}

#[test]
fn a_foreign_directory_is_never_deleted_by_the_scan_or_the_cleanup() {
    let env = Env::new("foreign");
    let approved = &env.roots.approved_root;
    fs::create_dir_all(approved).unwrap();
    let no_marker = approved.join("cheats-nomarker");
    fs::create_dir_all(&no_marker).unwrap();
    fs::write(no_marker.join("data.txt"), b"not ours").unwrap();

    let bad_marker = approved.join("cheats-badmarker");
    fs::create_dir_all(&bad_marker).unwrap();
    fs::write(bad_marker.join(WORKSPACE_MARKER_NAME), b"{not json").unwrap();

    let other_owner = approved.join("cheats-other");
    fs::create_dir_all(&other_owner).unwrap();
    let marker = WorkspaceMarker {
        schema_version: MARKER_SCHEMA_VERSION,
        created_by: "some-other-tool".into(),
        launch_id: "other".into(),
        game_identity: String::new(),
        adapter_id: String::new(),
        plan_digest: String::new(),
        cheat_ids: vec![],
        created_unix: 0,
        owner_pid: dead_pid(),
        owner_start_ticks: None,
        state: MarkerState::Materialised,
        emulator_pid: None,
    };
    fs::write(
        other_owner.join(WORKSPACE_MARKER_NAME),
        serde_json::to_vec(&marker).unwrap(),
    )
    .unwrap();

    let wrong_id = approved.join("cheats-mismatch");
    fs::create_dir_all(&wrong_id).unwrap();
    let mut mismatched = marker.clone();
    mismatched.created_by = "emuwiz".into();
    mismatched.launch_id = "not-the-dir-name".into();
    fs::write(
        wrong_id.join(WORKSPACE_MARKER_NAME),
        serde_json::to_vec(&mismatched).unwrap(),
    )
    .unwrap();

    let target = env.base.join("user-docs");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("keep.txt"), b"mine").unwrap();
    let linked = approved.join("cheats-linked");
    symlink(&target, &linked).unwrap();

    let unrelated = approved.join("projection-1");
    fs::create_dir_all(&unrelated).unwrap();

    let scan = scan_cheat_runtime_workspaces(&env.roots, now_unix(), 0);
    assert_eq!(
        scan.len(),
        5,
        "only cheat-workspace-shaped names are listed"
    );
    assert!(
        scan.iter()
            .all(|e| e.disposition == WorkspaceDisposition::Foreign)
    );
    for path in [&no_marker, &bad_marker, &other_owner, &wrong_id, &linked] {
        assert!(cleanup_stale_workspace(&env.roots, path, now_unix(), 0).is_err());
        assert!(path.exists());
    }
    assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"mine");
    assert!(no_marker.join("data.txt").exists() && unrelated.exists());

    // Paths outside the approved root are refused outright.
    assert!(cleanup_stale_workspace(&env.roots, &target, now_unix(), 0).is_err());
    assert!(cleanup_stale_workspace(&env.roots, &env.roots.approved_root, now_unix(), 0).is_err());
    assert!(target.join("keep.txt").exists());
}

#[test]
fn the_plan_digest_changes_when_anything_that_matters_changes() {
    let env = Env::new("digest");
    let a = build(&env, "dg");
    let b = build(&env, "dg");
    assert_eq!(a.plan.digest(), b.plan.digest(), "same inputs, same digest");
    let mut other = parts_for(&env, "dg");
    other.files[0] = RuntimeFile::new(
        env.root_for("dg").join("c.cfg"),
        b"different".to_vec(),
        RuntimeFileKind::Config,
    );
    assert_ne!(
        CheatRuntimePlan::new(other).unwrap().digest(),
        CheatRuntimePlan::new(parts_for(&env, "dg"))
            .unwrap()
            .digest()
    );
}
