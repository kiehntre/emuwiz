//! Gamer shows the shared DuckStation resolver's fallback and refusal.
//! Synthetic fixtures in a private HOME/XDG; no launch request is executed.
use super::*;
use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority;
use archivefs_core::emulator_inventory::InventoryEmulator;
use archivefs_core::emulator_profile_resolver::{
    CandidateIdentity, EmulatorSelectionMode, remember_selection_to,
};
use archivefs_core::game_identity::{
    IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityKind, IdentityPlatform,
    IdentityProvenance, IdentityStatus,
};
use archivefs_core::launch::{FirmwareReadiness, LaunchReadiness, LaunchTarget};
use std::fs;

struct EnvGuard(Vec<(String, Option<std::ffi::OsString>)>);
impl EnvGuard {
    fn set(values: &[(&str, &Path)]) -> Self {
        let old = values
            .iter()
            .map(|(key, value)| {
                let old = std::env::var_os(key);
                // These tests run alone, serially, in a private process/profile.
                unsafe {
                    std::env::set_var(key, value);
                }
                (key.to_string(), old)
            })
            .collect();
        Self(old)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}
struct Machine {
    _dir: tempfile::TempDir,
    _env: EnvGuard,
    executable: PathBuf,
    root: PathBuf,
    selection: PathBuf,
    content: PathBuf,
}
impl Machine {
    fn new() -> Self {
        Self::with_names(false)
    }
    fn with_names(long: bool) -> Self {
        let pad = if long {
            "-".to_string() + &"deeply-nested-long-name-".repeat(6)
        } else {
            String::new()
        };
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let config = dir.path().join(format!("xdg-config{pad}"));
        let data = dir.path().join("xdg-data");
        let appconfig = dir.path().join("app-config");
        for p in [&home, &config, &data, &appconfig] {
            fs::create_dir_all(p).unwrap();
        }
        let guard = EnvGuard::set(&[
            ("HOME", &home),
            ("XDG_CONFIG_HOME", &config),
            ("XDG_DATA_HOME", &data),
            ("EMUWIZ_CONFIG_HOME", &appconfig),
            ("PATH", Path::new("")),
        ]);
        let executable = home.join(format!(
            "Applications{pad}/Healthy{pad}/DuckStation.AppImage"
        ));
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let root = config.join("duckstation");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("settings.ini"),
            b"[Main]\nSetupWizardIncomplete = false\n",
        )
        .unwrap();
        fs::write(root.join("playtime.dat"), b"SCUS-94601 341 1783636844\n").unwrap();
        let selection =
            archivefs_core::emulator_profile_resolver::persist::default_selection_path().unwrap();
        let content = dir.path().join("Reviewer PS1.iso");
        fs::write(&content, b"synthetic selected content; not an actual game").unwrap();
        Self {
            _dir: dir,
            _env: guard,
            executable,
            root,
            selection,
            content,
        }
    }
    fn remember(&self, forced: bool) {
        let gone = self._dir.path().join("Gone");
        let identity = CandidateIdentity {
            emulator: InventoryEmulator::DuckStation,
            executable: Some(gone.join("DuckStation.AppImage")),
            profile_root: gone,
        };
        let mode = if forced {
            EmulatorSelectionMode::Forced(identity)
        } else {
            EmulatorSelectionMode::Preferred(identity)
        };
        remember_selection_to(&self.selection, InventoryEmulator::DuckStation, &mode).unwrap();
    }
    fn app(&self) -> ArchiveFsApp {
        let mut app = app_for_operation_tests();
        let mut game = record(self.content.to_str().unwrap(), MountState::NotMountable);
        game.metadata.title = Some("Reviewer PS1".into());
        game.metadata.platform = Some("PS1".into());
        app.state = LoadState::Ready(Box::new(loaded_data_with_records(
            self._dir.path().to_str().unwrap(),
            vec![game],
        )));
        app.archive_context.select_only(self.content.clone());
        app.emulator_readiness
            .emulator_setup_overrides
            .duckstation_executable = Some(self.executable.clone());
        app.selected_evidence_ui.selected_evidence = ready_selected_evidence_state(&self.content);
        if let selected_evidence_page::SelectedEvidenceState::Ready { report, .. } =
            &mut app.selected_evidence_ui.selected_evidence
        {
            let identity = &mut report.game_identity_report;
            identity.platform = IdentityPlatform::PlayStation;
            identity.format = IdentityImageFormat::Iso;
            identity.evidence = vec![IdentityEvidence {
                kind: IdentityKind::Ps1Serial,
                status: IdentityStatus::Verified,
                value: Some("SCUS-94601".into()),
                confidence: IdentityConfidence::StructuredMetadata,
                provenance: IdentityProvenance {
                    archive_path: self.content.clone(),
                    member_path: None,
                    member_index: None,
                    method: "synthetic reviewer verified-identity fixture".into(),
                },
                diagnostic: "synthetic, no real game data".into(),
            }];
            identity.complete = true;
        }
        app
    }
    fn input(&self, app: &ArchiveFsApp) -> launch_readiness_page::LaunchReadinessInput {
        let LoadState::Ready(data) = &app.state else {
            unreachable!()
        };
        app.build_launch_readiness_input(Some(data))
    }
}
fn texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect, egui::Rect)> {
    fn walk(
        shape: &egui::Shape,
        clip: egui::Rect,
        out: &mut Vec<(String, egui::Rect, egui::Rect)>,
    ) {
        match shape {
            egui::Shape::Text(t) => out.push((
                t.galley.text().into(),
                egui::Rect::from_min_size(t.pos, t.galley.size()),
                clip,
            )),
            egui::Shape::Vec(v) => {
                for s in v {
                    walk(s, clip, out)
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for s in &output.shapes {
        walk(&s.shape, s.clip_rect, &mut out)
    }
    out
}
fn gamer_frame(
    ctx: &egui::Context,
    app: &mut ArchiveFsApp,
    play: &launch_readiness_page::GamerPlayAction,
    notice: Option<&launch_readiness_page::GamerResolverNotice>,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<GamerViewAction>) {
    let mut action = None;
    let mut cover_requests = vec![];
    let mut screenshot_requests = vec![];
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024., 600.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let LoadState::Ready(data) = &app.state else {
                    unreachable!()
                };
                action = show_gamer_view(
                    ui,
                    Some(data),
                    GamerViewViewState {
                        filter: &mut app.library_ui.filter,
                        library_filters: &mut app.library_ui.library_filters,
                        archive_context: &mut app.archive_context,
                        screen: &mut app.gamer_view_screen,
                        busy: false,
                        block_reason: None,
                        cleanup_after_unmount: false,
                        cheat_workflow: None,
                        feedback: None,
                        scan_review_available: false,
                        artwork_directory: None,
                        artwork_cache: app.artwork_media.platform_artwork.render_assets().cache,
                        covers: &mut app.artwork_media.gamer_covers,
                        screenshots: &mut app.artwork_media.gamer_screenshots,
                        cover_requests: &mut cover_requests,
                        screenshot_requests: &mut screenshot_requests,
                        game_metadata: None,
                        identity_status: Some(GamerIdentityStatus::Identified),
                        prepared_member: false,
                        member_choices: None,
                        preparation_message: None,
                        play_action: play,
                        resolver_notice: notice,
                        retroarch_launch_state: &mut app.launch_retroarch,
                        dolphin_launch_state: &mut app.launch_dolphin,
                        pcsx2_launch_state: &mut app.launch_pcsx2,
                        standalone_launch_state: &mut app.launch_standalone,
                        alpha_jump: &mut app.artwork_media.gamer_alpha_jump,
                    },
                );
            });
        },
    );
    (output, action)
}

fn settled(
    app: &mut ArchiveFsApp,
    play: &launch_readiness_page::GamerPlayAction,
    notice: Option<&launch_readiness_page::GamerResolverNotice>,
) -> (egui::Context, Vec<(String, egui::Rect, egui::Rect)>) {
    let ctx = egui::Context::default();
    ctx.style_mut(|s| s.animation_time = 0.);
    let mut output = gamer_frame(&ctx, app, play, notice, vec![]).0;
    for _ in 0..4 {
        output = gamer_frame(&ctx, app, play, notice, vec![]).0;
    }
    (ctx, texts(&output))
}

fn visible(drawn: &[(String, egui::Rect, egui::Rect)], needle: &str) -> bool {
    drawn
        .iter()
        .any(|(s, r, clip)| s.contains(needle) && clip.contains_rect(*r))
}

fn click(
    ctx: &egui::Context,
    app: &mut ArchiveFsApp,
    play: &launch_readiness_page::GamerPlayAction,
    notice: Option<&launch_readiness_page::GamerResolverNotice>,
    pos: egui::Pos2,
) -> Option<GamerViewAction> {
    gamer_frame(
        ctx,
        app,
        play,
        notice,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
    );
    gamer_frame(
        ctx,
        app,
        play,
        notice,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    )
    .1
}

/// The real controller reads HOME/XDG/PATH, so each fixture runs in its own
/// child process with private values: nothing mutates this process's
/// environment, and parallel tests are unaffected.
fn in_isolated_process(name: &str, body: impl FnOnce()) {
    if std::env::var_os("EMUWIZ_DUCKSTATION_NOTICE_CHILD").is_some() {
        body();
        return;
    }
    let module = module_path!().split_once("::").map_or("", |(_, rest)| rest);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("{module}::{name}"),
            "--test-threads=1",
            "--nocapture",
        ])
        .env("EMUWIZ_DUCKSTATION_NOTICE_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{name} failed in its isolated process:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{name} did not run in the isolated process"
    );
}

#[test]
fn forced_unavailable_shows_the_resolver_reason_and_keeps_the_setup_handoff() {
    in_isolated_process(
        "forced_unavailable_shows_the_resolver_reason_and_keeps_the_setup_handoff",
        || {
            let m = Machine::new();
            m.remember(true);
            let saved = fs::read(&m.selection).unwrap();
            let mut app = m.app();
            let input = m.input(&app);
            let launch_readiness_page::LaunchReadinessInput::Plan {
                plan,
                duckstation: Some(context),
                ..
            } = &input
            else {
                panic!("controller did not build a real plan")
            };
            assert!(
                matches!(&context.authority, DuckStationLaunchAuthority::Refused { reason } if reason.contains("FORCED") && reason.contains("Gone")),
                "{:?}",
                context.authority
            );
            let duck: Vec<_> = plan
                .candidates
                .iter()
                .filter(|c| {
                    matches!(
                        c.target,
                        LaunchTarget::Standalone {
                            adapter_id: "duckstation",
                            ..
                        }
                    )
                })
                .collect();
            assert_eq!(duck.len(), 1);
            assert_eq!(duck[0].readiness, LaunchReadiness::Blocked);
            let play = launch_readiness_page::gamer_play_action(&input);
            let launch_readiness_page::GamerPlayAction::BlockedTyped(blocker) = &play else {
                panic!("must stay blocked")
            };
            assert!(
                blocker.detail.contains("FORCED") && blocker.detail.contains("Gone"),
                "technical detail keeps the resolver reason: {}",
                blocker.detail
            );
            let notice = launch_readiness_page::gamer_resolver_notice(&input, &play)
                .expect("a refusal is a notice");
            assert_eq!(
                notice.kind,
                launch_readiness_page::GamerResolverNoticeKind::Blocked
            );
            let (ctx, drawn) = settled(&mut app, &play, Some(&notice));
            assert!(visible(&drawn, "FORCED"), "pinned choice named: {drawn:#?}");
            assert!(
                visible(&drawn, "Gone"),
                "unavailable target named: {drawn:#?}"
            );
            assert!(
                visible(&drawn, "will not use any other DuckStation installation"),
                "{drawn:#?}"
            );
            assert!(
                !drawn.iter().any(|(s, ..)| s.starts_with("Play with")),
                "launch stays blocked"
            );
            let pos = drawn
                .iter()
                .find(|(s, r, clip)| s == "Open Emulator Setup" && clip.contains_rect(*r))
                .expect("Emulator Setup handoff visible")
                .1
                .center();
            let action = click(&ctx, &mut app, &play, Some(&notice), pos);
            assert!(
                matches!(action, Some(GamerViewAction::OpenEmulatorSetup(..))),
                "Open Emulator Setup must hand off"
            );
            assert_eq!(
                fs::read(&m.selection).unwrap(),
                saved,
                "saved preference untouched"
            );
        },
    );
}

fn fallback_case(long: bool) {
    let m = Machine::with_names(long);
    m.remember(false);
    let saved = fs::read(&m.selection).unwrap();
    let mut app = m.app();
    let mut input = m.input(&app);
    let launch_readiness_page::LaunchReadinessInput::Plan {
        plan,
        duckstation: Some(context),
        ..
    } = &mut input
    else {
        panic!("controller did not build a real plan")
    };
    assert!(
        matches!(&context.authority, DuckStationLaunchAuthority::Authorised { executable, profile_root, fallback: true, .. } if executable == &m.executable && profile_root == &m.root),
        "{:?}",
        context.authority
    );
    let c = plan
        .candidates
        .iter_mut()
        .find(|c| {
            matches!(
                c.target,
                LaunchTarget::Standalone {
                    adapter_id: "duckstation",
                    ..
                }
            )
        })
        .expect("real controller candidate");
    // The synthetic profile has no BIOS: model a passed firmware preflight so
    // the ready-plan branch renders, keeping the real resolver authority.
    c.readiness = LaunchReadiness::Ready;
    c.firmware = FirmwareReadiness::NotRequired;
    c.blockers.clear();
    c.warnings.clear();
    c.preference = archivefs_core::launch::CandidatePreference::SoleEligible;
    let play = launch_readiness_page::gamer_play_action(&input);
    assert!(matches!(
        &play,
        launch_readiness_page::GamerPlayAction::Launch(_)
    ));
    let notice = launch_readiness_page::gamer_resolver_notice(&input, &play)
        .expect("a fallback is a notice");
    assert_eq!(
        notice.kind,
        launch_readiness_page::GamerResolverNoticeKind::PreferredFallback
    );
    let (ctx, drawn) = settled(&mut app, &play, Some(&notice));
    let exe = m.executable.display().to_string();
    const REASON: &str = "It is the only installation with a usable configuration.";
    assert!(
        visible(&drawn, "preferred DuckStation is unavailable"),
        "bypass stated: {drawn:#?}"
    );
    assert!(
        visible(&drawn, "DuckStation.AppImage"),
        "chosen installation named compactly: {drawn:#?}"
    );
    assert!(
        visible(&drawn, "Your saved preference was not changed."),
        "{drawn:#?}"
    );
    // Full paths and the exact reason are behind one disclosure, not
    // in the way of Play.
    assert!(
        !visible(&drawn, &exe),
        "full path is not forced into the stage"
    );
    let header = drawn
        .iter()
        .find(|(s, r, clip)| s == "Full paths and reason" && clip.contains_rect(*r))
        .expect("disclosure visible")
        .1
        .center();
    let pos = drawn
        .iter()
        .find(|(s, r, clip)| s == "Play with DuckStation" && clip.contains_rect(*r))
        .expect("Play visible at 1024x600 without scrolling")
        .1
        .center();
    let action = click(&ctx, &mut app, &play, Some(&notice), pos);
    let Some(GamerViewAction::Play(request)) = action else {
        panic!("Play must return the launch request")
    };
    assert_eq!(request.adapter_name(), "DuckStation");
    // Open the disclosure: the whole executable, profile and reason.
    click(&ctx, &mut app, &play, Some(&notice), header);
    let (_, opened) = {
        let out = gamer_frame(&ctx, &mut app, &play, Some(&notice), vec![]).0;
        (ctx.clone(), texts(&out))
    };
    assert!(visible(&opened, &exe), "full executable path: {opened:#?}");
    assert!(
        visible(&opened, &m.root.display().to_string()),
        "full profile path: {opened:#?}"
    );
    assert!(visible(&opened, REASON), "exact reason: {opened:#?}");
    assert_eq!(
        fs::read(&m.selection).unwrap(),
        saved,
        "saved preference untouched"
    );
}

#[test]
fn preferred_fallback_shows_selection_reason_and_bypass_before_play() {
    in_isolated_process(
        "preferred_fallback_shows_selection_reason_and_bypass_before_play",
        || fallback_case(false),
    );
}

#[test]
fn preferred_fallback_with_long_paths_keeps_play_visible_and_launchable_at_1024x600() {
    in_isolated_process(
        "preferred_fallback_with_long_paths_keeps_play_visible_and_launchable_at_1024x600",
        || fallback_case(true),
    );
}

#[test]
fn healthy_preferred_and_auto_add_no_resolver_notice() {
    in_isolated_process("healthy_preferred_and_auto_add_no_resolver_notice", || {
        // PREFERRED pointing at the healthy installation: nothing was bypassed.
        let m = Machine::new();
        let identity = CandidateIdentity {
            emulator: InventoryEmulator::DuckStation,
            executable: Some(m.executable.clone()),
            profile_root: m.root.clone(),
        };
        remember_selection_to(
            &m.selection,
            InventoryEmulator::DuckStation,
            &EmulatorSelectionMode::Preferred(identity),
        )
        .unwrap();
        let mut app = m.app();
        let input = m.input(&app);
        let play = launch_readiness_page::gamer_play_action(&input);
        assert!(launch_readiness_page::gamer_resolver_notice(&input, &play).is_none());
        let (_, drawn) = settled(&mut app, &play, None);
        assert!(
            !drawn
                .iter()
                .any(|(s, ..)| s.contains("preferred DuckStation is unavailable")
                    || s.contains("blocked by the shared resolver"))
        );

        // AUTO: no remembered selection at all.
        drop(app);
        drop(m);
        let m = Machine::new();
        let app = m.app();
        let input = m.input(&app);
        let play = launch_readiness_page::gamer_play_action(&input);
        assert!(launch_readiness_page::gamer_resolver_notice(&input, &play).is_none());
    });
}
