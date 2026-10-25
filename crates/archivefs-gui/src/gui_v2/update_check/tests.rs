//! Reuses the historical adapter's state/worker cases, with actual egui
//! action wiring and bounded waits. No GitHub requests or browser processes.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

fn check(status: EmuWizUpdateStatus) -> EmuWizUpdateCheck {
    EmuWizUpdateCheck {
        current_version: current_emuwiz_version().into(),
        latest_version: Some("v99.0.0".into()),
        release_url: Some("https://github.com/kiehntre/emuwiz/releases/tag/v99.0.0".into()),
        release_notes: Some("Synthetic notes".into()),
        status,
        detail: None,
        checked_at_unix_seconds: unix_seconds(),
    }
}
fn settle(state: &mut UpdateCheckState) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while state.task.is_some() && Instant::now() < deadline {
        state.poll(&egui::Context::default());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        state.task.is_none(),
        "worker did not settle within test deadline"
    );
}
fn frame<F>(
    ctx: &egui::Context,
    state: &mut UpdateCheckState,
    size: egui::Vec2,
    events: Vec<egui::Event>,
    checker: F,
) -> (egui::Response, egui::FullOutput)
where
    F: FnOnce() -> EmuWizUpdateCheck + Send + 'static,
{
    let checker = Arc::new(std::sync::Mutex::new(Some(checker)));
    let mut response = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // egui can request multiple layout passes; only an explicit click
                // consumes the injected worker closure, not an idle layout pass.
                let checker = checker.clone();
                response = Some(show_with_checker(ui, state, move || {
                    checker
                        .lock()
                        .unwrap()
                        .take()
                        .expect("one worker action per frame")()
                }));
            });
        },
    );
    (response.unwrap(), output)
}
fn text(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect(s, out)
                }
            }
            _ => (),
        }
    }
    let mut out = Vec::new();
    for s in &output.shapes {
        collect(&s.shape, &mut out);
    }
    out.join("\n")
}

#[test]
fn current_version_is_the_existing_workspace_version() {
    assert_eq!(current_emuwiz_version(), env!("CARGO_PKG_VERSION"));
}
#[test]
fn blue_settings_exposes_about_and_the_explicit_check_without_starting_it() {
    let ctx = egui::Context::default();
    let mut app = crate::gui_v2::tests::fixture(&ctx);
    app.router.current =
        crate::gui_v2::routes::Route::Section(crate::gui_v2::routes::Section::Settings);
    let out = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 900.),
            )),
            ..Default::default()
        },
        |ctx| app.show(ctx),
    );
    let rendered = text(&out);
    assert!(rendered.contains("About EmuWiz"));
    assert!(rendered.contains("Check for Updates"));
    assert!(app.update_check.task.is_none());
    assert!(app.update_check.last_result.is_none());
    assert!(app.preferences_dirty.is_none());
}
#[test]
fn idle_render_never_starts_a_startup_or_offline_network_check() {
    let mut state = UpdateCheckState::default();
    let ctx = egui::Context::default();
    for _ in 0..3 {
        let (_, out) = frame(&ctx, &mut state, egui::vec2(1024., 600.), vec![], || {
            panic!("idle UI must not check")
        });
        assert!(text(&out).contains("Check for Updates"));
    }
    assert!(state.task.is_none());
    assert!(state.last_result.is_none());
}
#[test]
fn check_is_single_flight_and_worker_execution_is_off_the_gui_thread() {
    let mut state = UpdateCheckState::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let gui_thread = std::thread::current().id();
    let (release, gate) = mpsc::channel();
    state.start_check_with(egui::Context::default(), move || {
        assert_ne!(std::thread::current().id(), gui_thread);
        count.fetch_add(1, Ordering::SeqCst);
        gate.recv_timeout(Duration::from_secs(2)).unwrap();
        check(EmuWizUpdateStatus::UpdateAvailable)
    });
    assert!(state.task.is_some());
    state.start_check_with(egui::Context::default(), || {
        panic!("second worker must not start")
    });
    release.send(()).unwrap();
    settle(&mut state);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.last_result.unwrap().status,
        EmuWizUpdateStatus::UpdateAvailable
    );
}
#[test]
fn running_check_disables_action_and_keeps_rendering_responsive() {
    let mut state = UpdateCheckState::default();
    let ctx = egui::Context::default();
    let (release, gate) = mpsc::channel();
    state.start_check_with(ctx.clone(), move || {
        gate.recv_timeout(Duration::from_secs(2)).unwrap();
        check(EmuWizUpdateStatus::UpToDate)
    });
    let (button, out) = frame(&ctx, &mut state, egui::vec2(1024., 600.), vec![], || {
        panic!("disabled action")
    });
    assert!(!button.enabled());
    assert!(text(&out).contains("Checking…"));
    assert!(state.task.is_some());
    release.send(()).unwrap();
    settle(&mut state);
}
#[test]
fn button_click_dispatches_the_injected_check() {
    let mut state = UpdateCheckState::default();
    let ctx = egui::Context::default();
    let (button, _) = frame(&ctx, &mut state, egui::vec2(1024., 600.), vec![], || {
        panic!("no click")
    });
    let pos = button.rect.center();
    frame(
        &ctx,
        &mut state,
        egui::vec2(1024., 600.),
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
        || panic!("press is not release"),
    );
    frame(
        &ctx,
        &mut state,
        egui::vec2(1024., 600.),
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
        || check(EmuWizUpdateStatus::UpdateAvailable),
    );
    settle(&mut state);
    assert_eq!(
        state.last_result.unwrap().status,
        EmuWizUpdateStatus::UpdateAvailable
    );
}
#[test]
fn keyboard_activation_uses_the_same_explicit_action() {
    let mut state = UpdateCheckState::default();
    let ctx = egui::Context::default();
    let (button, _) = frame(&ctx, &mut state, egui::vec2(1024., 600.), vec![], || {
        panic!("idle")
    });
    ctx.memory_mut(|m| m.request_focus(button.id));
    frame(
        &ctx,
        &mut state,
        egui::vec2(1024., 600.),
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        || check(EmuWizUpdateStatus::UpToDate),
    );
    settle(&mut state);
    assert_eq!(
        state.last_result.unwrap().status,
        EmuWizUpdateStatus::UpToDate
    );
}
#[test]
fn disconnected_worker_replaces_an_old_success_with_a_truthful_failure() {
    let mut state = UpdateCheckState::default();
    state.last_result = Some(check(EmuWizUpdateStatus::UpToDate));
    let (sender, receiver) = mpsc::channel();
    drop(sender);
    state.task = Some(receiver);
    state.poll(&egui::Context::default());
    let r = state.last_result.unwrap();
    assert_eq!(r.status, EmuWizUpdateStatus::CheckFailed);
    assert!(r.detail.unwrap().contains("without a result"));
    assert!(r.release_url.is_none());
}
#[test]
fn retry_after_failure_can_succeed_without_losing_current_version() {
    let mut state = UpdateCheckState::default();
    for status in [
        EmuWizUpdateStatus::NetworkFailure,
        EmuWizUpdateStatus::UpdateAvailable,
    ] {
        state.start_check_with(egui::Context::default(), move || check(status));
        settle(&mut state);
        assert_eq!(state.last_result.as_ref().unwrap().status, status);
        assert_eq!(
            state.last_result.as_ref().unwrap().current_version,
            current_emuwiz_version()
        );
    }
}
#[test]
fn status_messages_keep_all_failure_and_success_states_distinct() {
    let states = [
        EmuWizUpdateStatus::UpToDate,
        EmuWizUpdateStatus::UpdateAvailable,
        EmuWizUpdateStatus::DevelopmentBuild,
        EmuWizUpdateStatus::CurrentVersionUnknown,
        EmuWizUpdateStatus::LatestVersionUnknown,
        EmuWizUpdateStatus::NetworkFailure,
        EmuWizUpdateStatus::Unavailable,
        EmuWizUpdateStatus::MalformedResponse,
        EmuWizUpdateStatus::CheckFailed,
    ];
    let messages = states.map(status_message);
    let unique = messages
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), states.len());
    assert!(status_message(EmuWizUpdateStatus::NetworkFailure).contains("possibly offline"));
}
#[test]
fn forged_untrusted_or_mismatched_release_links_are_not_exposed() {
    let mut r = check(EmuWizUpdateStatus::UpdateAvailable);
    assert!(release_link(&r).is_some());
    for link in [
        "https://evil.test/",
        "https://github.com/kiehntre/emuwiz/issues/1",
        "https://github.com/kiehntre/emuwiz/releases/tag/v1.0.0",
    ] {
        r.release_url = Some(link.into());
        assert!(release_link(&r).is_none());
    }
}
#[test]
fn notes_are_plain_text_and_idle_render_never_opens_a_browser() {
    let mut state = UpdateCheckState::default();
    state.last_result = Some(check(EmuWizUpdateStatus::UpdateAvailable));
    let ctx = egui::Context::default();
    let (_, out) = frame(&ctx, &mut state, egui::vec2(1024., 600.), vec![], || {
        panic!("no explicit click")
    });
    assert!(text(&out).contains("Open official release notes"));
    assert!(out.platform_output.commands.is_empty());
    assert!(state.task.is_none());
}
#[test]
fn check_action_remains_visible_in_supported_narrow_viewports() {
    for size in [
        egui::vec2(1920., 1080.),
        egui::vec2(1440., 900.),
        egui::vec2(1024., 600.),
    ] {
        let mut state = UpdateCheckState::default();
        let ctx = egui::Context::default();
        let (button, out) = frame(&ctx, &mut state, size, vec![], || {
            panic!("no check on render")
        });
        assert!(button.enabled());
        assert!(button.rect.min.x >= 0. && button.rect.max.x <= size.x);
        assert!(button.rect.min.y >= 0. && button.rect.max.y <= size.y);
        assert!(text(&out).contains("About EmuWiz"));
    }
}
#[test]
fn all_results_render_the_current_available_and_last_checked_information() {
    for status in [
        EmuWizUpdateStatus::UpToDate,
        EmuWizUpdateStatus::UpdateAvailable,
        EmuWizUpdateStatus::NetworkFailure,
        EmuWizUpdateStatus::Unavailable,
        EmuWizUpdateStatus::MalformedResponse,
    ] {
        let mut state = UpdateCheckState::default();
        state.last_result = Some(check(status));
        let ctx = egui::Context::default();
        let (_, out) = frame(&ctx, &mut state, egui::vec2(1024., 600.), vec![], || {
            panic!("render")
        });
        let text = text(&out);
        assert!(text.contains(current_emuwiz_version()));
        assert!(text.contains("v99.0.0"));
        assert!(text.contains("seconds ago"));
        assert!(text.contains(status_message(status)));
    }
}
