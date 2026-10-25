//! Opt-in real approved RetroArch/Fuse run through Blue's actual readiness,
//! chooser clicks and launch workers. The driver supplies private HOME/XDG
//! and an original legally distributable snapshot. No fake backend or authority.
use super::*;
use crate::launch_readiness_page::RetroArchLaunchDisplay;
use archivefs_core::launch::cheat_runtime::RuntimeEffect;
use archivefs_core::launch::libretro_probe::{ProbeLauncher, run_probe_child};
use sha2::{Digest, Sha256};

struct RealProbe;
impl ProbeLauncher for RealProbe {
    fn child(&self) -> std::io::Result<(PathBuf, Vec<String>, Vec<(String, String)>)> {
        Ok((
            std::env::current_exe()?,
            vec![
                "--exact".into(),
                "gui_v2::retroarch_journey_tests::fuse_snapshot::probe_child_entry".into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
            ],
            vec![("EMUWIZ_JOB20_PROBE_CHILD".into(), "1".into())],
        ))
    }
}
#[test]
fn probe_child_entry() {
    if std::env::var_os("EMUWIZ_JOB20_PROBE_CHILD").is_none() {
        return;
    }
    let args = std::env::args()
        .rev()
        .take(9)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    run_probe_child(&args);
}
fn digest(path: &Path) -> String {
    Sha256::digest(fs::read(path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn seed(root: &Path) {
    use archivefs_core::dat::sources::{
        DatSourceKind,
        audit_cache::AuditCacheConfig,
        audit_run::{DatAuditRequest, run_dat_audit_with_cache},
    };
    let config = archivefs_core::default_config_path().unwrap();
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        format!(
            "source_folders = [\"{}\"]\nmount_root = \"{}\"\nratarmount_bin = \"ratarmount\"\n",
            root.join("content").display(),
            root.join("mount").display()
        ),
    )
    .unwrap();
    let dbpath = archivefs_core::default_database_path().unwrap();
    fs::create_dir_all(dbpath.parent().unwrap()).unwrap();
    let mut db = archivefs_core::Database::open_or_create(dbpath).unwrap();
    archivefs_core::scan_and_persist(
        &mut db,
        &archivefs_core::Config::load_default().unwrap(),
        "job20-real-blue",
    )
    .unwrap();
    let path = root.join("content/activation.z80");
    let id = db.find_archive_id_by_absolute_path(&path).unwrap().unwrap();
    db.set_manual_platform_for_archives(&[id], "ZX Spectrum")
        .unwrap();
    let sha1 = sha1::Sha1::digest(fs::read(&path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let dat = root.join("blue-fixture.dat");
    fs::write(&dat,format!(r#"<datafile><header><name>Sinclair ZX Spectrum - Games - [Z80]</name><version>20261025</version></header><game name="Original Job20 Activation"><rom name="activation.z80" size="49182" sha1="{sha1}"/></game></datafile>"#)).unwrap();
    let outcome = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: "job20-zx".into(),
            source_display_name: "Original Job20 ZX fixture".into(),
            dat_path: dat.clone(),
            dat_kind: DatSourceKind::File,
            scan_root: root.join("content"),
            limits: Default::default(),
            policy: None,
            platform: None,
        },
        &archivefs_core::safe_read::TrustedRoots::none(),
        &std::sync::atomic::AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    db.persist_library_dat_identities_from_audit(&outcome)
        .unwrap();
    let sources = archivefs_core::dat::sources::default_dat_sources_config_path().unwrap();
    fs::write(sources,format!("[[sources]]\nid = \"job20-zx\"\ndisplay_name = \"Original Job20 ZX fixture\"\npath = \"{}\"\nkind = \"file\"\nenabled = true\n",dat.display())).unwrap();
    // Create the trainer association through Gold, using the actual freshly
    // loaded DAT identities (not a caller-supplied verification boolean).
    let (game, roots) = crate::gui_v2::classic_trainers::current_launch_evidence(&path).unwrap();
    let pin = archivefs_core::classic_trainer_evidence::pinning::preview_pin(&game, &path, &roots)
        .unwrap();
    archivefs_core::classic_trainer_evidence::pinning::create_pin(&pin, &game, &roots).unwrap();
}
fn finish_run(
    blue: &mut Blue,
    request: &archivefs_core::launch::RetroArchLaunchRequest,
    root: &Path,
    mode: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let pid = loop {
        blue.paint();
        match blue.state().display_for(request) {
            RetroArchLaunchDisplay::Running { pid } => break pid,
            RetroArchLaunchDisplay::Failed { message, detail } => {
                panic!("{mode}: {message}: {detail}")
            }
            _ => {}
        }
        assert!(
            Instant::now() < deadline,
            "{mode}: did not start; {:?}",
            blue.last
        );
        std::thread::sleep(Duration::from_millis(30));
    };
    fs::write(
        root.join(format!("{mode}-process.txt")),
        fs::read(format!("/proc/{pid}/cmdline")).unwrap(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        Command::new("/usr/bin/import")
            .args(["-window", "root"])
            .arg(root.join(format!("{mode}.png")))
            .status()
            .unwrap()
            .success()
    );
    if let Some(receipt) = blue.state().smoke_receipt() {
        assert_eq!(receipt.runtime_effect, RuntimeEffect::Unknown);
        fs::write(
            root.join(format!("{mode}-started.json")),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let workspace = receipt.workspace_root.unwrap();
        fn copy(from: &Path, to: &Path) {
            fs::create_dir_all(to).unwrap();
            for entry in fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                if entry.path().is_dir() {
                    copy(&entry.path(), &to.join(entry.file_name()));
                } else {
                    fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
                }
            }
        }
        copy(&workspace, &root.join(format!("{mode}-workspace-copy")));
    }
    Command::new("/usr/bin/xdotool")
        .args(["key", "Escape"])
        .status()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        blue.paint();
        if let RetroArchLaunchDisplay::Exited { success, .. } = blue.state().display_for(request) {
            assert!(success);
            break;
        }
        if Instant::now() > deadline {
            Command::new("/usr/bin/kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .unwrap();
            panic!("{mode}: emulator exit timeout");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    if let Some(receipt) = blue.state().smoke_receipt() {
        assert!(!receipt.has_violation());
        fs::write(
            root.join(format!("{mode}-receipt.json")),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
    } else {
        fs::write(
            root.join(format!("{mode}-plain.txt")),
            "Blue canonical plain launch: no cheats or cheat receipt",
        )
        .unwrap();
    }
    fs::write(
        root.join(format!("{mode}-blue-ui.txt")),
        blue.last.join("\n"),
    )
    .unwrap();
}
#[test]
#[ignore = "real approved RetroArch/Fuse, private display and original CC0 snapshot; run only with disposable driver"]
fn installed_fuse_activation_through_blue() {
    let root = PathBuf::from(
        std::env::var_os("EMUWIZ_JOB20_SMOKE_ROOT").expect("use the disposable smoke driver"),
    );
    assert!(
        root.starts_with("/tmp")
            && root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("emuwiz-job20-runtime-")
    );
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_DATA_DIRS",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
        "EMUWIZ_CONFIG_HOME",
        "EMUWIZ_DATA_HOME",
    ] {
        assert!(
            PathBuf::from(std::env::var_os(key).unwrap()).starts_with(&root),
            "private {key} required"
        );
    }
    assert_eq!(
        digest(Path::new("/usr/games/retroarch")),
        archivefs_core::classic_trainer_evidence::launch::VERIFIED_RETROARCH_SHA256
    );
    assert_eq!(
        digest(Path::new(
            "/usr/lib/x86_64-linux-gnu/libretro/fuse_libretro.so"
        )),
        archivefs_core::classic_trainer_evidence::launch::VERIFIED_FUSE_CORE_SHA256
    );
    seed(&root);
    let context = eframe::egui::Context::default();
    context.style_mut(|style| style.animation_time = 0.0);
    let mut app = super::super::tests::fixture(&context);
    app.simple.enabled = true;
    app.library = Arc::new(
        super::super::backend::load_library(&archivefs_core::default_database_path().unwrap())
            .unwrap(),
    );
    let game = app
        .library
        .games
        .iter()
        .find(|g| g.archive.absolute_path == root.join("content/activation.z80"))
        .unwrap();
    assert!(game.dat_exact.is_some());
    let id = game.archive.id;
    app.router.current = Route::Task {
        section: Section::Launch,
        game: id,
    };
    let mut blue = Blue {
        context,
        app,
        game: id,
        last: vec![],
        seen: vec![],
    };
    blue.paint();
    blue.state().use_probe(Arc::new(RealProbe));
    blue.wait_for_readiness();
    let request = current_retroarch_request(&blue);
    assert_eq!(request.expected_platform_id, "ZX Spectrum");
    assert_eq!(request.core_stem, "fuse");
    let protected = [
        root.join("content/activation.z80"),
        root.join("content/activation.pok"),
        root.join("content/activation.pok.target"),
        root.join("config/retroarch/retroarch.cfg"),
    ];
    let before = protected
        .iter()
        .map(|p| (p.clone(), digest(p)))
        .collect::<Vec<_>>();
    fs::write(
        root.join("blue-hashes-before.json"),
        serde_json::to_vec_pretty(&before).unwrap(),
    )
    .unwrap();
    // Guard the ACTUAL canonical selection, including baseline Play. This
    // performs the real preflight and never constructs an alternate command.
    let command=archivefs_core::launch::preflight_retroarch_launch(&request,&archivefs_core::emulator_environment::HostReadOnlyFilesystem,&archivefs_core::emulator_environment::retroarch::DiscoveryEnvironment::from_process_environment()).unwrap();
    assert_eq!(command.executable, Path::new("/usr/games/retroarch"));
    assert_eq!(
        command.selection.core_library,
        root.join("cores/fuse_libretro.so")
    );
    assert_eq!(
        digest(&command.executable),
        archivefs_core::classic_trainer_evidence::launch::VERIFIED_RETROARCH_SHA256
    );
    assert_eq!(
        digest(&command.selection.core_library),
        archivefs_core::classic_trainer_evidence::launch::VERIFIED_FUSE_CORE_SHA256
    );
    blue.click(PLAY);
    finish_run(&mut blue, &request, &root, "baseline");
    blue.click("POKE trainers (optional, default OFF)");
    blue.click("Use explicitly selected POKE trainers for this launch");
    blue.click("Load / refresh verified POKE entries");
    blue.wait_until(|b| b.shown("1: Green upper and border"), 20);
    blue.click("1: Green upper and border");
    blue.click("Launch with selected POKE trainers");
    assert!(!matches!(
        blue.state().display_for(&request),
        RetroArchLaunchDisplay::Running { .. }
    ));
    blue.click("I reviewed these POKE trainers for this game");
    blue.click("Launch with selected POKE trainers");
    finish_run(&mut blue, &request, &root, "selected");
    blue.click(PLAY);
    finish_run(&mut blue, &request, &root, "after-off");
    let after = protected
        .iter()
        .map(|p| (p.clone(), digest(p)))
        .collect::<Vec<_>>();
    assert_eq!(before, after);
    fs::write(
        root.join("blue-hashes-after.json"),
        serde_json::to_vec_pretty(&after).unwrap(),
    )
    .unwrap();
    println!(
        "Real Blue baseline / explicit reviewed trainer / OFF again complete. External screenshots must establish activation; receipts remain UNKNOWN."
    );
}
