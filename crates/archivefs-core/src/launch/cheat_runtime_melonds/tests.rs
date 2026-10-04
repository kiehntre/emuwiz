//! Fake-melonDS tests: real temp directories, a shell script as the emulator
//! that records its environment/argv/config and blocks until released.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use super::*;
use crate::launch::cheat_runtime::{CheatRuntimeSession, CleanupOutcome};
use crate::launch::planning::ResolvedIdentity;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::{discover_melonds_profiles, parse_melonds_cheat_file};

const MCH: &str = "CAT 0 Gameplay\nCODE 0 Free Play\n02000000 00000001\nCODE 0 Other\n02000004 00000002\n\nCAT 1 Exclusive\nCODE 0 Alpha\n02000008 00000003\nCODE 0 Beta\n0200000C 00000004\n";
const GAME_CODE: &str = "EWZP";

struct Fx {
    dir: tempfile::TempDir,
    exe: PathBuf,
    config: PathBuf,
    rom: PathBuf,
    roots: MelonDsProfileDiscoveryRoots,
    request: MelonDsLaunchRequest,
}

fn sha(path: &Path) -> String {
    sha256_file(path).unwrap()
}

impl Fx {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("realcfg");
        fs::create_dir_all(&config_dir).unwrap();
        let config = config_dir.join("melonDS.toml");
        fs::write(
            &config,
            "[Emu]\nDirectBoot = true\n\n[Instance0]\nSaveFilePath = \"\"\nEnableCheats = false\n",
        )
        .unwrap();
        let exe = dir.path().join("melonDS");
        fs::write(
            &exe,
            "#!/bin/sh\n\
             printf '%s' \"${XDG_CONFIG_HOME-unset}\" > \"$0.xdg\"\n\
             printf '%s\\n' \"$@\" > \"$0.argv\"\n\
             if [ -n \"$XDG_CONFIG_HOME\" ]; then\n\
               cp \"$XDG_CONFIG_HOME/melonDS/melonDS.toml\" \"$0.toml\"\n\
               d=$(sed -n 's/^CheatFilePath = \"\\(.*\\)\"$/\\1/p' \"$XDG_CONFIG_HOME/melonDS/melonDS.toml\")\n\
               ls \"$d\" > \"$0.mchlist\"; cat \"$d\"/*.mch > \"$0.mch\"\n\
             fi\n\
             : > \"$0.started\"\n\
             while [ ! -e \"$0.release\" ]; do sleep 0.02; done\n\
             if [ -n \"$XDG_CONFIG_HOME\" ]; then echo x > \"$XDG_CONFIG_HOME/melonDS/rtc.bin\"; echo '# rewritten' >> \"$XDG_CONFIG_HOME/melonDS/melonDS.toml\"; fi\n\
             exit 0\n",
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let rom = dir.path().join("A DS Game.nds");
        let mut bytes = vec![0u8; 0x400];
        bytes[0x0C..0x10].copy_from_slice(GAME_CODE.as_bytes());
        fs::write(&rom, bytes).unwrap();
        let roots = MelonDsProfileDiscoveryRoots {
            home: dir.path().to_path_buf(),
            xdg_config_home: dir.path().join("unused"),
            explicit_configuration_roots: vec![config_dir],
            portable_configuration_roots: vec![],
            explicit_executables: vec![exe.clone()],
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        };
        let profile = &discover_melonds_profiles(&roots).profiles[0];
        let capture = |p: &Path| CapturedFileIdentity::capture(&fs::symlink_metadata(p).unwrap());
        let request = MelonDsLaunchRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "Nintendo DS".into(),
            expected_game_key: "DS-TEST".into(),
            profile_id: profile.profile_id.clone(),
            expected_executable: exe.clone(),
            content_identity: capture(&rom),
            executable_identity: capture(&exe),
            config_identity: Some(capture(&config)),
            expected_installation: crate::launch::installation::LaunchInstallation::Native,
        };
        Self {
            dir,
            exe,
            config,
            rom,
            roots,
            request,
        }
    }

    fn identity() -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Nintendo DS".into(),
            game_key: "DS-TEST".into(),
        })
    }

    fn selection(&self, names: &[&str]) -> MelonDsCheatSelection {
        MelonDsCheatSelection {
            identity: MelonDsRomIdentity {
                rom_path: self.rom.clone(),
                rom_sha256: Some(sha(&self.rom)),
                game_code: Some(GAME_CODE.into()),
                verified: true,
                title_hint: None,
            },
            file: parse_melonds_cheat_file(MCH),
            selected: names.iter().map(|n| n.to_string()).collect(),
            real_config: Some(self.config.clone()),
        }
    }

    fn runtime(&self, name: &str) -> CheatRuntimeRoots {
        CheatRuntimeRoots {
            approved_root: self.dir.path().join(name),
        }
    }

    fn launch(
        &self,
        cheat: Option<&MelonDsCheatSelection>,
        id: &str,
        runtime: &CheatRuntimeRoots,
    ) -> Result<MelonDsCheatLaunch, MelonDsCheatLaunchError> {
        preflight_and_launch_melonds_with_cheats(
            &self.request,
            &self.roots,
            &Self::identity(),
            Some("DS-TEST"),
            cheat,
            id,
            runtime,
        )
    }

    fn file(&self, ext: &str) -> PathBuf {
        PathBuf::from(format!("{}.{ext}", self.exe.display()))
    }

    fn wait_started(&self) {
        let start = Instant::now();
        while !self.file("started").exists() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "fake melonDS never started"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn release(&self) {
        fs::write(self.file("release"), b"").unwrap();
    }
}

fn finish(session: &mut CheatRuntimeSession<WatchedProcess>) {
    let start = Instant::now();
    while !session.is_finished() {
        session.poll();
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "melonDS never exited"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn with_cheats(
    launch: MelonDsCheatLaunch,
) -> (
    Box<CheatRuntimeSession<WatchedProcess>>,
    MelonDsCheatActivation,
    Vec<String>,
) {
    match launch {
        MelonDsCheatLaunch::WithCheats {
            session,
            activation,
            notes,
        } => (session, activation, notes),
        MelonDsCheatLaunch::Plain(_) => panic!("expected a cheat launch"),
    }
}

fn refusal(result: Result<MelonDsCheatLaunch, MelonDsCheatLaunchError>) -> MelonDsCheatError {
    match result.err().expect("expected a refusal") {
        MelonDsCheatLaunchError::Cheats(e) => e,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_launch_without_cheats_is_unchanged_no_env_no_profile() {
    let fx = Fx::new();
    let before = sha(&fx.config);
    let runtime = fx.runtime("ws");
    let MelonDsCheatLaunch::Plain(mut process) = fx.launch(None, "plain1", &runtime).unwrap()
    else {
        panic!("plain expected")
    };
    fx.wait_started();
    let expected_xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| "unset".into());
    assert_eq!(
        fs::read_to_string(fx.file("xdg")).unwrap(),
        expected_xdg,
        "no environment override"
    );
    let argv = fs::read_to_string(fx.file("argv")).unwrap();
    assert_eq!(argv.lines().collect::<Vec<_>>(), [fx.rom.to_str().unwrap()]);
    assert!(!runtime.approved_root.exists());
    assert_eq!(sha(&fx.config), before);
    fx.release();
    while process.poll().is_none() {
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_cheat_launch_stages_a_private_profile_and_keeps_it_for_the_process_lifetime() {
    let fx = Fx::new();
    let config_before = fs::read(&fx.config).unwrap();
    let runtime = fx.runtime("ws with space");
    let (mut session, activation, notes) = with_cheats(
        fx.launch(Some(&fx.selection(&["Free Play"])), "ds1", &runtime)
            .unwrap(),
    );
    fx.wait_started();
    assert!(notes.is_empty());
    let workspace = runtime.approved_root.join(workspace_name("ds1"));
    // Child-only env points at the workspace; argv is still just the ROM.
    assert_eq!(
        fs::read_to_string(fx.file("xdg")).unwrap(),
        workspace.join("config").to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(fx.file("argv")).unwrap().lines().count(),
        1
    );
    // The seeded config keeps the user's settings and forces the two cheat keys.
    let toml = fs::read_to_string(fx.file("toml")).unwrap();
    assert!(toml.contains("DirectBoot = true"), "{toml}");
    assert!(toml.contains("EnableCheats = true"), "{toml}");
    assert!(
        toml.contains(&format!(
            "CheatFilePath = \"{}\"",
            workspace.join("cheats").display()
        )),
        "{toml}"
    );
    // melonDS names the file after the ROM stem and sees only the chosen cheat, switched on.
    assert_eq!(
        fs::read_to_string(fx.file("mchlist")).unwrap().trim(),
        "A DS Game.mch"
    );
    let mch = fs::read_to_string(fx.file("mch")).unwrap();
    assert!(
        mch.contains("CODE 1 Free Play") && !mch.contains("Other") && !mch.contains("Alpha"),
        "{mch}"
    );
    // Alive while running; real config and ROM folder untouched.
    assert_eq!(
        session.cleanup_now(),
        CleanupOutcome::DeferredEmulatorRunning
    );
    assert!(workspace.join("cheats/A DS Game.mch").exists());
    assert_eq!(fs::read(&fx.config).unwrap(), config_before);
    assert!(!fx.rom.with_extension("mch").exists());
    fx.release();
    finish(&mut session);
    assert!(
        !workspace.exists(),
        "cleaned after exit, including files melonDS wrote"
    );
    let receipt = session.receipt();
    assert_eq!(receipt.adapter_id, MELONDS_ADAPTER_ID);
    assert_eq!(receipt.cheats.len(), 1);
    assert_eq!(receipt.cheats[0].provider, "melonds-mch");
    assert!(!receipt.has_violation(), "{:?}", receipt.errors);
    assert_eq!(fs::read(&fx.config).unwrap(), config_before);
    assert_eq!(
        activation,
        MelonDsCheatActivation::MaterialStagedEmulatorPointedAtItEffectNotProven
    );
    let label = activation.label().to_lowercase();
    assert!(label.contains("has not seen") && !label.contains("activated"));
}

#[test]
fn identity_and_material_problems_refuse_before_anything_starts() {
    let fx = Fx::new();
    let runtime = fx.runtime("ws");
    let mut cases: Vec<(MelonDsCheatSelection, &str)> = Vec::new();
    let mut unverified = fx.selection(&["Free Play"]);
    unverified.identity.verified = false;
    cases.push((unverified, "unverified"));
    let mut no_ids = fx.selection(&["Free Play"]);
    no_ids.identity.rom_sha256 = None;
    no_ids.identity.game_code = None;
    cases.push((no_ids, "no ids"));
    let mut wrong_code = fx.selection(&["Free Play"]);
    wrong_code.identity.game_code = Some("ZZZZ".into());
    cases.push((wrong_code, "wrong code"));
    let mut wrong_hash = fx.selection(&["Free Play"]);
    wrong_hash.identity.rom_sha256 = Some("0".repeat(64));
    cases.push((wrong_hash, "wrong hash"));
    let mut wrong_path = fx.selection(&["Free Play"]);
    wrong_path.identity.rom_path = fx.dir.path().join("Other.nds");
    cases.push((wrong_path, "wrong path"));
    for (selection, label) in &cases {
        let e = refusal(fx.launch(Some(selection), "r1", &runtime));
        assert!(
            matches!(
                e,
                MelonDsCheatError::IdentityNotVerified(_) | MelonDsCheatError::IdentityMismatch(_)
            ),
            "{label}: {e:?}"
        );
    }
    assert_eq!(
        refusal(fx.launch(Some(&fx.selection(&[])), "r2", &runtime)),
        MelonDsCheatError::NoCheatSelected
    );
    assert!(matches!(
        refusal(fx.launch(Some(&fx.selection(&["Nope"])), "r3", &runtime)),
        MelonDsCheatError::UnknownCheat(_)
    ));
    assert!(matches!(
        refusal(fx.launch(Some(&fx.selection(&["Alpha", "Beta"])), "r4", &runtime)),
        MelonDsCheatError::Material(_)
    ));
    let mut broken = fx.selection(&["Free Play"]);
    broken.file = parse_melonds_cheat_file("CODE 1 Broken\nnot words\n");
    assert!(matches!(
        refusal(fx.launch(Some(&broken), "r5", &runtime)),
        MelonDsCheatError::Material(_)
    ));
    assert!(matches!(
        refusal(fx.launch(Some(&fx.selection(&["Free Play"])), "../escape", &runtime)),
        MelonDsCheatError::Runtime(_)
    ));
    assert!(!fx.file("started").exists());
    assert!(!runtime.approved_root.exists());
}

#[test]
fn an_unreadable_real_config_is_reported_and_a_minimal_profile_is_used() {
    let fx = Fx::new();
    let mut selection = fx.selection(&["Free Play"]);
    fs::write(&fx.config, "this is = not [toml").unwrap();
    selection.real_config = Some(fx.config.clone());
    let runtime = fx.runtime("ws");
    // Preflight notices the changed config, so build the plan directly.
    let (plan, environment, notes) =
        melonds_runtime_plan(&fx.rom, &selection, "n1", &runtime.approved_root).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("not inherited"));
    assert_eq!(environment.len(), 1);
    let config = plan
        .files()
        .iter()
        .find(|f| f.path.ends_with("melonDS.toml"))
        .unwrap();
    let text = String::from_utf8(config.bytes.clone()).unwrap();
    assert!(text.contains("EnableCheats = true") && !text.contains("not [toml"));
}

#[test]
fn a_spawn_failure_cleans_the_workspace() {
    let fx = Fx::new();
    // Same size/identity class is irrelevant: rewrite and re-authorise the executable.
    fs::write(&fx.exe, "#!/nonexistent/interpreter\n").unwrap();
    let mut fx = fx;
    fx.request.executable_identity =
        CapturedFileIdentity::capture(&fs::symlink_metadata(&fx.exe).unwrap());
    let runtime = fx.runtime("ws");
    let e = refusal(fx.launch(Some(&fx.selection(&["Free Play"])), "fail1", &runtime));
    assert!(matches!(e, MelonDsCheatError::Runtime(_)));
    assert!(!runtime.approved_root.join(workspace_name("fail1")).exists());
}

#[test]
fn two_simultaneous_sessions_are_isolated() {
    let fx = Fx::new();
    let runtime = fx.runtime("ws");
    let (mut a, _, _) = with_cheats(
        fx.launch(Some(&fx.selection(&["Free Play"])), "a1", &runtime)
            .unwrap(),
    );
    fx.wait_started();
    let (mut b, _, _) = with_cheats(
        fx.launch(Some(&fx.selection(&["Other"])), "b1", &runtime)
            .unwrap(),
    );
    let (dir_a, dir_b) = (
        runtime.approved_root.join(workspace_name("a1")),
        runtime.approved_root.join(workspace_name("b1")),
    );
    let mch = |d: &Path| fs::read_to_string(d.join("cheats/A DS Game.mch")).unwrap();
    assert!(mch(&dir_a).contains("Free Play") && !mch(&dir_a).contains("Other"));
    assert!(mch(&dir_b).contains("Other") && !mch(&dir_b).contains("Free Play"));
    // A duplicate id is refused and does not disturb the live workspace.
    assert!(
        fx.launch(Some(&fx.selection(&["Free Play"])), "a1", &runtime)
            .is_err()
    );
    assert!(dir_a.exists());
    fx.release();
    finish(&mut a);
    finish(&mut b);
    assert!(!dir_a.exists() && !dir_b.exists());
}

/// Real-melonDS probe: `EMUWIZ_REAL_MELONDS=<AppImage> cargo test ... -- --ignored real_melonds`.
/// Needs `xvfb-run` and `strace`. A wrapper only adds a virtual display, a
/// 15-second limit and an open(2) trace; everything melonDS sees (env, argv,
/// staged config, cheat file) comes from the adapter. The ROM is a synthetic
/// header-only fixture, so no real ROM or save is involved. The user's real
/// melonDS config is only used as the seed, and its hash must not change.
#[test]
#[ignore = "needs a real melonDS AppImage, xvfb-run and strace"]
fn real_melonds_opens_the_staged_cheat_file_from_the_private_profile() {
    let Some(appimage) = std::env::var_os("EMUWIZ_REAL_MELONDS") else {
        return;
    };
    let fx = Fx::new();
    let trace = fx.dir.path().join("strace.txt");
    fs::write(
        &fx.exe,
        format!(
            "#!/bin/sh\nexec timeout 15 xvfb-run -a strace -f -e trace=openat -o '{}' '{}' --appimage-extract-and-run \"$@\" >/dev/null 2>&1\n",
            trace.display(),
            Path::new(&appimage).display()
        ),
    )
    .unwrap();
    let mut fx = fx;
    fx.request.executable_identity =
        CapturedFileIdentity::capture(&fs::symlink_metadata(&fx.exe).unwrap());
    let real = dirs_real_config();
    let real_hash = real.as_ref().map(|p| sha(p));
    let mut selection = fx.selection(&["Free Play"]);
    selection.real_config = real.clone();
    let runtime = fx.runtime("ws");
    let (mut session, _, _) = with_cheats(fx.launch(Some(&selection), "real1", &runtime).unwrap());
    let workspace = runtime.approved_root.join(workspace_name("real1"));
    let start = Instant::now();
    while !session.is_finished() {
        session.poll();
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "melonDS wrapper never exited"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let log = fs::read_to_string(&trace).unwrap();
    let staged = workspace.join("cheats/A DS Game.mch");
    assert!(
        log.lines().any(|l| l.contains(&format!(
            "openat(AT_FDCWD, \"{}\", O_RDONLY",
            staged.display()
        )) && !l.contains("= -1")),
        "melonDS never opened the staged cheat file:\n{}",
        log.lines()
            .filter(|l| l.contains(".mch") || l.contains("melonDS.toml"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        log.contains(&format!(
            "{}/config/melonDS/melonDS.toml",
            workspace.display()
        )),
        "melonDS did not read the private profile"
    );
    assert!(!workspace.exists());
    assert_eq!(
        real.as_ref().map(|p| sha(p)),
        real_hash,
        "the real melonDS config must not change"
    );
    assert!(!fx.rom.with_extension("mch").exists());
}

fn dirs_real_config() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".config/melonDS/melonDS.toml");
    path.is_file().then_some(path)
}
