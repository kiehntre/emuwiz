use super::*;
use crate::gui_v2::vice_evidence::tests::Lab;
use archivefs_core::classic_trainer_evidence::pinning::{create_pin, preview_pin};
use archivefs_core::launch::cheat_runtime::{
    CleanupOutcome, HandoffOutcome, MaterialisationOutcome, RuntimeEffect, StateCheckOutcome,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

struct Fixture {
    lab: Lab,
    request: Request,
    trusted: TrustedRoots,
    runtime: CheatRuntimeRoots,
    selection: Selection,
}
fn fixture(text: &str) -> Fixture {
    let lab = Lab::new();
    fs::write(lab.path.with_extension("pok"), text).unwrap();
    let evidence = lab.evidence().unwrap();
    let trusted = TrustedRoots::from_paths([lab.path.parent().unwrap()]);
    let pin = preview_pin(&game(&evidence), &lab.path, &trusted).unwrap();
    create_pin(&pin, &game(&evidence), &trusted).unwrap();
    let exe = lab.path.with_file_name("x64sc");
    let args = lab.path.with_file_name("args");
    let captured = lab.path.with_file_name("captured");
    fs::write(&exe, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nif [ \"$2\" = -moncommands ]; then cat \"$3\" > '{}'; fi\nexit 0\n", args.display(), captured.display())).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let digest = vice_evidence::hex(&sha2::Sha256::digest(fs::read(&exe).unwrap()));
    let profiles = ViceProfileDiscoveryRoots {
        explicit_executables: vec![exe],
        path_env: None,
        ..Default::default()
    };
    let context =
        Context::inspect_with(lab.path.clone(), evidence.clone(), profiles, &digest).unwrap();
    let request = context.profiles[0].1.clone();
    let selection = Selection {
        scope: Scope::of(&request),
        preview: gold::preview(&game(&evidence), &lab.path, &trusted).unwrap(),
        entries: vec![0],
    };
    let runtime = CheatRuntimeRoots {
        approved_root: lab.path.with_file_name("runtime"),
    };
    fs::create_dir(&runtime.approved_root).unwrap();
    Fixture {
        lab,
        request,
        trusted,
        runtime,
        selection,
    }
}
fn finish(session: &mut CheatRuntimeSession<WatchedProcess>) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !session.is_finished() && Instant::now() < deadline {
        let _ = session.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(session.is_finished());
}
fn read(f: &Fixture) -> Result<(Evidence, TrustedRoots), String> {
    Ok((f.lab.evidence()?, f.trusted.clone()))
}

#[test]
fn approved_bundle_uses_gold_preflight_runtime_receipt_and_post_exit_cleanup() {
    let f = fixture("POKE $C000,$2A\nPOKE $CFFF,7");
    let originals: Vec<_> = [
        f.lab.path.clone(),
        f.lab.path.with_extension("pok"),
        archivefs_core::classic_trainer_evidence::trainer_target_path(&f.lab.path),
        f.request.launch.expected_executable.clone(),
    ]
    .into_iter()
    .map(|p| {
        let bytes = fs::read(&p).unwrap();
        (p, bytes)
    })
    .collect();
    let foreign = f.runtime.approved_root.join("foreign");
    fs::write(&foreign, b"keep").unwrap();
    let mut session = f
        .request
        .clone()
        .launch_trainers_checked(&f.selection, &f.runtime, || read(&f))
        .unwrap();
    assert!(matches!(
        session.receipt().materialisation,
        MaterialisationOutcome::Materialised { .. }
    ));
    assert_eq!(session.receipt().handoff, HandoffOutcome::PassedToEmulator);
    assert_eq!(session.receipt().runtime_effect, RuntimeEffect::Unknown);
    assert_eq!(session.receipt().cheats.len(), 1);
    let workspace = session.receipt().workspace_root.clone().unwrap();
    finish(&mut session);
    assert_eq!(session.receipt().cleanup, CleanupOutcome::Completed);
    assert!(!workspace.exists());
    assert_eq!(fs::read(&foreign).unwrap(), b"keep");
    assert_eq!(
        fs::read(f.lab.path.with_file_name("captured")).unwrap(),
        b"radix H\n> C000 2A\n> CFFF 07\nx\n"
    );
    let arguments = fs::read_to_string(f.lab.path.with_file_name("args")).unwrap();
    assert!(arguments.starts_with("+saveres\n-moncommands\n"));
    assert!(arguments.ends_with(&format!("+cart\n-cartcrt\n{}\n", f.lab.path.display())));
    assert!(
        session
            .receipt()
            .state_checks
            .iter()
            .all(|s| !matches!(s.outcome, StateCheckOutcome::Violation { .. }))
    );
    for (path, bytes) in originals {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn ordinary_play_never_inherits_trainer_choices_or_monitor_arguments() {
    let f = fixture("POKE $C000,42");
    let mut process = f
        .request
        .clone()
        .execute_checked(|_| f.lab.evidence())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while process.poll().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(process.poll().is_some());
    assert!(
        !fs::read_to_string(f.lab.path.with_file_name("args"))
            .unwrap()
            .contains("moncommands")
    );
    assert!(!f.lab.path.with_file_name("captured").exists());
}

#[test]
fn stale_trainer_pin_game_dat_and_backend_refuse_without_spawn() {
    for which in ["trainer", "pin", "game", "dat", "exe"] {
        let f = fixture("POKE $C000,42");
        let path = match which {
            "trainer" => f.lab.path.with_extension("pok"),
            "pin" => archivefs_core::classic_trainer_evidence::trainer_target_path(&f.lab.path),
            "game" => f.lab.path.clone(),
            "dat" => f.lab.dat.clone(),
            _ => f.request.launch.expected_executable.clone(),
        };
        fs::write(path, b"changed").unwrap();
        assert!(
            f.request
                .clone()
                .launch_trainers_checked(&f.selection, &f.runtime, || read(&f))
                .is_err(),
            "{which}"
        );
        assert!(!f.lab.path.with_file_name("args").exists());
    }
}

#[test]
fn last_handoff_rechecks_dat_and_retains_refusal_receipt_with_cleanup() {
    let f = fixture("POKE $C000,42");
    let mut calls = 0;
    let session = f
        .request
        .clone()
        .launch_trainers_checked(&f.selection, &f.runtime, || {
            calls += 1;
            if calls == 2 {
                return Err("DAT evidence changed after preparation".into());
            }
            read(&f)
        })
        .unwrap();
    assert_eq!(calls, 2);
    assert!(matches!(
        session.receipt().handoff,
        HandoffOutcome::SpawnFailed { .. }
    ));
    assert_eq!(session.receipt().cleanup, CleanupOutcome::Completed);
    assert_eq!(session.receipt().runtime_effect, RuntimeEffect::Unknown);
    assert!(!f.lab.path.with_file_name("args").exists());
    assert!(
        session
            .receipt()
            .errors
            .iter()
            .any(|s| s.contains("DAT evidence changed"))
    );
    assert_eq!(phase_label(&session), "Trainer launch refused");
}

#[test]
fn unknown_empty_and_unsupported_selections_do_not_prepare_commands() {
    let f = fixture("POKE $C000,42");
    for entries in [vec![], vec![1], vec![0, 0]] {
        let selection = Selection {
            scope: f.selection.scope.clone(),
            preview: f.selection.preview.clone(),
            entries,
        };
        assert!(
            f.request
                .clone()
                .launch_trainers_checked(&selection, &f.runtime, || read(&f))
                .is_err()
        );
    }
    let f = fixture("POKE $D800,42");
    assert!(
        f.request
            .clone()
            .launch_trainers_checked(&f.selection, &f.runtime, || read(&f))
            .is_err()
    );
    assert!(!f.lab.path.with_file_name("args").exists());
}

fn render_panel(input: &LaunchReadinessInput, state: &mut StandaloneLaunchState) {
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_launch_readiness_panel(
                ui,
                input,
                &mut Default::default(),
                &mut Default::default(),
                &mut Default::default(),
                state,
                &mut Default::default(),
            );
        });
    });
}

#[test]
fn multiple_vice_profiles_preserve_gold_ambiguity_refusal_and_clear_approval() {
    let f = fixture("POKE $C000,42");
    let second = f.lab.path.with_file_name("second-bin").join("x64sc");
    fs::create_dir(second.parent().unwrap()).unwrap();
    fs::copy(&f.request.launch.expected_executable, &second).unwrap();
    let roots = ViceProfileDiscoveryRoots {
        explicit_executables: vec![f.request.launch.expected_executable.clone(), second],
        path_env: None,
        ..Default::default()
    };
    let context = Context::inspect_with(
        f.lab.path.clone(),
        f.lab.evidence().unwrap(),
        roots,
        &f.request.executable_sha256,
    )
    .unwrap();
    assert_eq!(context.profiles.len(), 2);
    let mut choice = chooser(&f);
    choice.enabled = true;
    choice.selected.insert(0);
    let mut state = StandaloneLaunchState {
        vice_trainers: choice,
        ..Default::default()
    };
    let input = super::super::tests::input(context);
    let LaunchReadinessInput::Plan {
        plan,
        vice: Some(context),
        ..
    } = &input
    else {
        panic!("expected VICE plan")
    };
    assert!(
        plan.candidates
            .iter()
            .all(|c| context.request(plan, c).is_none())
    );
    assert!(plan.candidates.iter().any(|c| {
        c.warnings
            .iter()
            .any(|w| w.kind == archivefs_core::launch::LaunchWarningKind::MultipleEligibleProfiles)
    }));
    for _ in 0..2 {
        render_panel(&input, &mut state);
    }
    assert!(
        !state.vice_trainers.enabled
            && !state.vice_trainers.approved
            && state.vice_trainers.selected.is_empty()
    );
}

#[test]
fn leaving_the_supported_lane_revokes_trainer_approval() {
    let f = fixture("POKE $C000,42");
    let mut choice = chooser(&f);
    choice.enabled = true;
    choice.approved = true;
    choice.selected.insert(0);
    let mut state = StandaloneLaunchState {
        vice_trainers: choice,
        ..Default::default()
    };
    let context = Context::inspect_with(
        f.lab.path.clone(),
        f.lab.evidence().unwrap(),
        f.request.roots.clone(),
        &f.request.executable_sha256,
    )
    .unwrap();
    let supported = super::super::tests::input(context);
    render_panel(&supported, &mut state);
    assert!(
        state.vice_trainers.enabled
            && state.vice_trainers.approved
            && state.vice_trainers.selected.contains(&0)
    );
    render_panel(&LaunchReadinessInput::IdentityUnknown, &mut state);
    assert!(
        !state.vice_trainers.enabled
            && !state.vice_trainers.approved
            && state.vice_trainers.selected.is_empty()
    );
    render_panel(&supported, &mut state);
    let ctx = egui::Context::default();
    assert!(
        frame(&ctx, &mut state.vice_trainers, &f.request, vec![])
            .1
            .is_none()
    );
    assert!(
        !state.vice_trainers.enabled
            && !state.vice_trainers.approved
            && state.vice_trainers.preview.is_none()
    );
}

fn assert_shared_refusal(session: CheatRuntimeSession<WatchedProcess>, f: &Fixture) {
    let mut state = StandaloneLaunchState {
        tracked: Some((
            f.lab.path.clone(),
            "vice".into(),
            StandaloneLaunchStage::Running(StandaloneProcess::ViceTrainers(Box::new(session))),
        )),
        ..Default::default()
    };
    state.poll();
    assert!(!state.is_active());
    assert!(
        state.failure_detail().is_some_and(|e| !e.is_empty()),
        "canonical refusal must reach Activity/attempt failure reporting"
    );
}

#[test]
fn changing_blue_game_focus_without_drawing_launch_panel_revokes_approval() {
    let f = fixture("POKE $C000,42");
    let mut choice = chooser(&f);
    choice.enabled = true;
    choice.approved = true;
    choice.selected.insert(0);
    let mut state = StandaloneLaunchState {
        vice_trainers: choice,
        ..Default::default()
    };
    state.focus_vice_trainer_game(Some(&f.lab.path));
    assert!(state.vice_trainers.approved);
    let other = f.lab.path.with_file_name("unsupported-game.tap");
    state.focus_vice_trainer_game(Some(&other));
    state.focus_vice_trainer_game(Some(&f.lab.path));
    assert!(
        !state.vice_trainers.enabled
            && !state.vice_trainers.approved
            && state.vice_trainers.selected.is_empty()
            && state.vice_trainers.preview.is_none()
    );
}

fn frame(
    ctx: &egui::Context,
    chooser: &mut Chooser,
    request: &Request,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Selection>) {
    let mut selected = None;
    let time = ctx.input(|i| i.time) + 1.0 / 60.0;
    let out = ctx.run(
        egui::RawInput {
            time: Some(time),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200., 1600.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                selected = chooser.show(ui, request, false);
            });
        },
    );
    (out, selected)
}
fn text_center(shape: &egui::epaint::Shape, text: &str) -> Option<egui::Pos2> {
    match shape {
        egui::epaint::Shape::Text(t) if t.galley.text().contains(text) => {
            Some(t.pos + t.galley.size() / 2.)
        }
        egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(|s| text_center(s, text)),
        _ => None,
    }
}
fn click(
    ctx: &egui::Context,
    chooser: &mut Chooser,
    request: &Request,
    text: &str,
) -> Option<Selection> {
    let (out, _) = frame(ctx, chooser, request, vec![]);
    let pos = out
        .shapes
        .iter()
        .find_map(|s| text_center(&s.shape, text))
        .unwrap_or_else(|| panic!("missing control: {text}"));
    let mut selected = None;
    for pressed in [true, false] {
        let (_, choice) = frame(
            ctx,
            chooser,
            request,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        if choice.is_some() {
            selected = choice;
        }
    }
    selected
}
fn chooser(f: &Fixture) -> Chooser {
    let r = &f.request;
    Chooser {
        scope: Some(Scope::of(r)),
        preview: Some(f.selection.preview.clone()),
        ..Default::default()
    }
}

#[test]
fn chooser_requires_default_off_opt_in_entry_selection_and_distinct_approval() {
    let f = fixture("POKE $C000,42");
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);
    let mut chooser = chooser(&f);
    assert!(!chooser.enabled && !chooser.approved && chooser.selected.is_empty());
    assert!(frame(&ctx, &mut chooser, &f.request, vec![]).1.is_none());
    click(&ctx, &mut chooser, &f.request, "C64 POKE trainers");
    click(
        &ctx,
        &mut chooser,
        &f.request,
        "Enable explicit C64 trainer selection",
    );
    assert!(chooser.enabled);
    click(&ctx, &mut chooser, &f.request, "whole bundle");
    assert_eq!(chooser.selected, BTreeSet::from([0]));
    assert!(!chooser.approved);
    assert!(
        click(
            &ctx,
            &mut chooser,
            &f.request,
            "Launch with selected C64 trainer"
        )
        .is_none()
    );
    click(&ctx, &mut chooser, &f.request, "I approve these selected");
    let selected = click(
        &ctx,
        &mut chooser,
        &f.request,
        "Launch with selected C64 trainer",
    )
    .unwrap();
    assert_eq!(selected.entries, [0]);
    assert!(!chooser.enabled && !chooser.approved && chooser.selected.is_empty());
    assert!(!f.lab.path.with_file_name("args").exists());
}

#[test]
fn scope_change_clears_approvals_and_discards_late_trainer_results() {
    let a = fixture("POKE $C000,42");
    let b = fixture("POKE $C000,7");
    let mut chooser = chooser(&a);
    chooser.enabled = true;
    chooser.approved = true;
    chooser.selected.insert(0);
    let (sender, receiver) = mpsc::channel();
    sender.send(Ok(a.selection.preview.clone())).unwrap();
    chooser.pending = Some((chooser.scope.clone().unwrap(), receiver));
    let ctx = egui::Context::default();
    assert!(frame(&ctx, &mut chooser, &b.request, vec![]).1.is_none());
    assert!(!chooser.enabled && !chooser.approved && chooser.selected.is_empty());
    assert!(chooser.preview.is_none() && chooser.pending.is_none());
}

#[test]
fn refreshed_request_with_changed_catalogue_proof_cannot_reuse_old_approval() {
    let f = fixture("POKE $C000,42");
    let mut changed = f.request.clone();
    changed.evidence.catalogues[0].1 = Some("different-catalogue-proof".into());
    assert!(
        changed
            .clone()
            .launch_trainers_checked(&f.selection, &f.runtime, || read(&f))
            .is_err()
    );
    let mut chooser = chooser(&f);
    chooser.enabled = true;
    chooser.approved = true;
    chooser.selected.insert(0);
    let ctx = egui::Context::default();
    assert!(frame(&ctx, &mut chooser, &changed, vec![]).1.is_none());
    assert!(!chooser.enabled && !chooser.approved && chooser.selected.is_empty());
    assert!(!f.lab.path.with_file_name("args").exists());
}

#[test]
fn ordinary_play_consumes_any_unused_trainer_approval() {
    let f = fixture("POKE $C000,42");
    let mut chooser = chooser(&f);
    chooser.enabled = true;
    chooser.approved = true;
    chooser.selected.insert(0);
    let mut state = StandaloneLaunchState {
        vice_trainers: chooser,
        ..Default::default()
    };
    state.start(StandaloneLaunchRequest::Vice(f.request.clone()));
    assert!(
        !state.vice_trainers.enabled
            && !state.vice_trainers.approved
            && state.vice_trainers.selected.is_empty()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while state.is_active() && Instant::now() < deadline {
        state.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!state.is_active());
    assert!(!f.lab.path.with_file_name("captured").exists());
}

#[test]
fn unsafe_workspace_root_produces_refusal_receipt_and_no_process() {
    use std::os::unix::fs::symlink;
    let f = fixture("POKE $C000,42");
    let link = f.lab.path.with_file_name("unsafe-runtime-link");
    symlink(&f.runtime.approved_root, &link).unwrap();
    let roots = CheatRuntimeRoots {
        approved_root: link,
    };
    let session = f
        .request
        .clone()
        .launch_trainers_checked(&f.selection, &roots, || read(&f))
        .unwrap();
    assert!(matches!(
        session.receipt().materialisation,
        MaterialisationOutcome::Refused { .. }
    ));
    assert!(session.receipt().process.is_none());
    assert_eq!(phase_label(&session), "Trainer launch refused");
    assert_eq!(session.receipt().cleanup, CleanupOutcome::NotNeeded);
    assert!(!f.lab.path.with_file_name("args").exists());
    assert_shared_refusal(session, &f);
}

#[test]
fn trainer_changed_after_materialisation_is_refused_at_final_handoff() {
    let f = fixture("POKE $C000,42");
    let mut calls = 0;
    let session = f
        .request
        .clone()
        .launch_trainers_checked(&f.selection, &f.runtime, || {
            calls += 1;
            if calls == 2 {
                fs::write(f.lab.path.with_extension("pok"), "POKE $C000,7").unwrap();
            }
            read(&f)
        })
        .unwrap();
    assert_eq!(calls, 2);
    assert!(matches!(
        session.receipt().handoff,
        HandoffOutcome::SpawnFailed { .. }
    ));
    assert_eq!(session.receipt().cleanup, CleanupOutcome::Completed);
    assert!(session.receipt().process.is_none());
    assert!(!f.lab.path.with_file_name("args").exists());
    assert_eq!(
        fs::read_to_string(f.lab.path.with_extension("pok")).unwrap(),
        "POKE $C000,7"
    );
    assert_shared_refusal(session, &f);
}
