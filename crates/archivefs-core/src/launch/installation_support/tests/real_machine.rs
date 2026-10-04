//! Real-machine acceptance matrix: `cargo test ... real_machine -- --ignored --nocapture`.
//!
//! Reads this machine's actual installs through the same assessment Doctor
//! uses, shows the launch argv the launcher would build, and runs one
//! bounded `--version`/`-help` style probe per resolved installation through
//! the watched-process layer. No game is booted and no emulator setting is
//! written.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::emulator_lifecycle::ExactBinding;
use crate::launch::installation::{LaunchInstallation, VisibilityPlan};
use crate::launch::installation_known::{KnownInstallRoots, discover_appimages, discover_flatpaks};
use crate::launch::installation_support::assess_from_environment;
use crate::launch::process_spawn::{PreparedProcessCommand, spawn_watched_process};

fn probe_args(emulator_id: &str) -> Vec<&'static str> {
    match emulator_id {
        "PCSX2" | "DuckStation" => vec!["-version"],
        "melonDS" => vec!["--help"],
        _ => vec!["--version"],
    }
}

#[test]
#[ignore = "reads this machine's real installs and spawns version/help probes"]
fn real_machine_acceptance_matrix() {
    let assessment = assess_from_environment();
    let known = KnownInstallRoots::from_environment().unwrap();
    let scratch = PathBuf::from("/tmp/emuwiz-ik-probe");
    std::fs::create_dir_all(&scratch).unwrap();
    let content = scratch.join("fixture game.bin");
    std::fs::write(&content, b"scratch fixture, not a game").unwrap();

    println!("\n=== DETECTION (known locations) ===");
    for app in &assessment.appimages {
        let binding = ExactBinding::PortableExecutable {
            path: app.path.clone(),
        };
        println!(
            "{:<12} AppImage  {}  portable_marker={}  -> {}",
            app.emulator_id,
            app.path.display(),
            app.portable_marker.is_some(),
            assessment.support_for(app.emulator_id, &binding).label()
        );
    }
    for def in [
        &crate::launch::installation_known::PCSX2,
        &crate::launch::installation_known::DUCKSTATION,
        &crate::launch::installation_known::PPSSPP,
        &crate::launch::installation_known::MELONDS,
    ] {
        for flatpak in discover_flatpaks(def, &known) {
            let binding = ExactBinding::FlatpakApp {
                app_id: flatpak.app_id.clone(),
            };
            println!(
                "{:<12} Flatpak   {} ({:?})  -> {}",
                def.id,
                flatpak.app_id,
                flatpak.scope,
                assessment.support_for(def.id, &binding).label()
            );
        }
        let _ = discover_appimages(def, &known);
    }

    println!("\n=== LAUNCH BINDINGS AND PROBES ===");
    let xvfb = known
        .path_dirs
        .iter()
        .map(|d| d.join("xvfb-run"))
        .find(|p| p.is_file());
    for resolved in &assessment.resolved {
        let mut visibility = VisibilityPlan::new();
        visibility.content(&content);
        let args = probe_args(resolved.emulator_id);
        let argv = resolved
            .installation
            .wrap_arguments(&visibility, args.iter().map(Into::into).collect())
            .unwrap();
        // Qt emulators need a display even for --version/--help, so every
        // probe runs under a virtual one when xvfb-run exists.
        let (exe, full): (PathBuf, Vec<std::ffi::OsString>) = match &xvfb {
            Some(x) => {
                let mut v = vec!["-a".into(), resolved.executable.clone().into_os_string()];
                v.extend(argv.clone());
                (x.clone(), v)
            }
            None => (resolved.executable.clone(), argv.clone()),
        };
        println!(
            "{:<12} {:?}\n    exe={}\n    argv={:?}",
            resolved.emulator_id,
            resolved.installation.kind(),
            resolved.executable.display(),
            argv
        );
        // (1) the exact command through the watched-process lifecycle.
        let started = Instant::now();
        let mut watched = spawn_watched_process(&PreparedProcessCommand {
            executable: exe.clone(),
            arguments: full.clone(),
            working_directory: None,
        })
        .expect("spawn");
        let pid = watched.pid;
        let status = loop {
            if let Some(report) = watched.poll() {
                break format!("{:?}", report.status.as_ref().map(|s| s.code()));
            }
            if started.elapsed() > Duration::from_secs(40) {
                break "timeout".into();
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        println!(
            "    watched pid={pid} exit={status} after {:?}",
            started.elapsed()
        );
        // (2) the same argv again, capturing the printed text.
        let output = Command::new(&exe)
            .args(&full)
            .stdin(Stdio::null())
            .output()
            .expect("probe");
        let text = String::from_utf8_lossy(&[output.stdout, output.stderr].concat())
            .lines()
            .filter(|l| {
                !l.trim().is_empty() && !l.contains("XDG_SESSION") && !l.contains("Wayland")
            })
            .take(2)
            .collect::<Vec<_>>()
            .join(" | ");
        println!("    output: {}", text.chars().take(160).collect::<String>());
    }
    println!("\n=== BLOCKERS (profiles that did not bind) ===");
    for (id, detail) in &assessment.blockers {
        println!("{id:<12} {detail}");
    }
}

/// `flatpak run` must stay alive for as long as the sandboxed app runs and
/// hand back its exit status, or the watched-process layer would treat the
/// wrapper's exit as the emulator's.
#[test]
#[ignore = "runs a harmless shell inside the first installed emulator Flatpak"]
fn real_machine_flatpak_lifecycle_follows_the_app() {
    let assessment = assess_from_environment();
    let Some(flatpak) = assessment
        .resolved
        .iter()
        .find(|r| matches!(r.installation, LaunchInstallation::Flatpak { .. }))
    else {
        println!("no Flatpak emulator installed; nothing to check");
        return;
    };
    let LaunchInstallation::Flatpak { app_id } = &flatpak.installation else {
        unreachable!()
    };
    let argv: Vec<std::ffi::OsString> = vec![
        "run".into(),
        "--command=sh".into(),
        app_id.into(),
        "-c".into(),
        "sleep 3; exit 7".into(),
    ];
    let started = Instant::now();
    let mut watched = spawn_watched_process(&PreparedProcessCommand {
        executable: flatpak.executable.clone(),
        arguments: argv,
        working_directory: None,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        watched.poll().is_none(),
        "the wrapper exited before the sandboxed app did"
    );
    let code = loop {
        if let Some(report) = watched.poll() {
            break report.status.as_ref().ok().and_then(|s| s.code());
        }
        assert!(started.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(50));
    };
    println!(
        "flatpak {app_id}: still running at 1.5s, exit code {code:?} after {:?}",
        started.elapsed()
    );
    assert_eq!(code, Some(7));
}
