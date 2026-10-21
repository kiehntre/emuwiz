//! Keyboard focus on the Launch page's controls must be brought into view at
//! the smallest supported window (760x560), where the pinned header and banner
//! leave only a short body. Mouse and wheel scrolling must stay the user's.
use super::*;
use std::cell::RefCell;

const SIZE: [f32; 2] = [760.0, 560.0];
/// What the pinned header, banner and status bar take from the 560 px window.
const PINNED: f32 = 330.0;
const LABELS: [&str; 5] = [
    "Check saved trainer profile",
    "Use this saved profile for the next launch",
    "Launch scummvm",
    "Technical details",
    "Retry readiness check",
];

#[derive(Default)]
struct Seen {
    /// The Library page's Play button, which still holds focus when Enter opens Launch.
    library_play: Option<egui::Id>,
    /// False while the page has not yet produced its controls (readiness pending).
    pending: bool,
    clip: Option<egui::Rect>,
    controls: Vec<(String, egui::Id, egui::Rect)>,
}

fn tab(shift: bool) -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: Some(egui::Key::Tab),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers {
                shift,
                ..Default::default()
            },
        })
        .collect()
}

fn wheel(delta: f32) -> Vec<egui::Event> {
    vec![egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, delta),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    }]
}

/// One real frame of the production nesting: the shared page scroller around
/// the Launch scroller, with launch-like controls separated by tall panels.
fn render(context: &egui::Context, seen: &RefCell<Seen>, events: Vec<egui::Event>, shift: bool) {
    render_page(context, seen, events, shift, true);
}

/// `launch` false renders some other page, as the Library is while Play is chosen.
fn render_page(
    context: &egui::Context,
    seen: &RefCell<Seen>,
    events: Vec<egui::Event>,
    shift: bool,
    launch: bool,
) {
    seen.borrow_mut().controls.clear();
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(SIZE[0], SIZE[1]),
            )),
            modifiers: egui::Modifiers {
                shift,
                ..Default::default()
            },
            events,
            ..Default::default()
        },
        |context| {
            super::super::simple_shell::prepare_keyboard_reveal(context);
            egui::CentralPanel::default().show(context, |ui| {
                ui.allocate_space(egui::vec2(100.0, PINNED));
                if !launch {
                    ui.label("Library");
                    seen.borrow_mut().library_play = Some(ui.button("Play").id);
                    return;
                }
                super::super::simple_shell::show_page_scroll(ui, Section::Launch, false, |ui| {
                    super::super::native_workflows::launch_scroll(ui, 7, |ui| {
                        seen.borrow_mut().clip = Some(ui.clip_rect());
                        if seen.borrow().pending {
                            ui.label("Preparing launch");
                            return;
                        }
                        for label in LABELS {
                            ui.allocate_space(egui::vec2(100.0, 260.0));
                            let response = if label.starts_with("Use") {
                                let mut on = false;
                                ui.checkbox(&mut on, label)
                            } else {
                                ui.button(label)
                            };
                            seen.borrow_mut().controls.push((
                                label.into(),
                                response.id,
                                response.rect,
                            ));
                        }
                    });
                });
            });
        },
    );
}

fn settle(context: &egui::Context, seen: &RefCell<Seen>) {
    for _ in 0..40 {
        render(context, seen, Vec::new(), false);
    }
}

/// The focused control and whether its whole rect is inside the visible clip.
fn focused(context: &egui::Context, seen: &RefCell<Seen>) -> (String, bool, egui::Rect) {
    let id = context
        .memory(|m| m.focused())
        .expect("a control has focus");
    let seen = seen.borrow();
    let (label, _, rect) = seen
        .controls
        .iter()
        .find(|(_, candidate, _)| *candidate == id)
        .expect("focus is on a launch control");
    let clip = seen.clip.expect("content rendered");
    let inside = rect.min.y >= clip.min.y - 1.5 && rect.max.y <= clip.max.y + 1.5;
    (label.clone(), inside, *rect)
}

fn start() -> (egui::Context, RefCell<Seen>) {
    let context = egui::Context::default();
    let seen = RefCell::new(Seen::default());
    settle(&context, &seen);
    (context, seen)
}

#[test]
fn forward_tab_keeps_every_launch_control_inside_the_visible_clip() {
    let (context, seen) = start();
    let mut visited = Vec::new();
    for step in 0..LABELS.len() {
        render(&context, &seen, tab(false), false);
        settle(&context, &seen);
        let (label, inside, rect) = focused(&context, &seen);
        assert!(
            inside,
            "step {step}: {label} at {rect:?} is outside {:?}",
            seen.borrow().clip
        );
        visited.push(label);
    }
    for label in LABELS {
        assert!(
            visited.iter().any(|v| v == label),
            "{label} never focused: {visited:?}"
        );
    }
}

#[test]
fn reverse_shift_tab_keeps_every_launch_control_inside_the_visible_clip() {
    let (context, seen) = start();
    for step in 0..LABELS.len() {
        render(&context, &seen, tab(true), true);
        settle(&context, &seen);
        let (label, inside, rect) = focused(&context, &seen);
        assert!(
            inside,
            "step {step}: {label} at {rect:?} is outside {:?}",
            seen.borrow().clip
        );
    }
}

#[test]
fn wheel_scrolling_after_a_reveal_is_not_snapped_back() {
    let (context, seen) = start();
    render(&context, &seen, tab(false), false);
    settle(&context, &seen);
    let (_, inside, _) = focused(&context, &seen);
    assert!(inside);
    // The user scrolls away on purpose; focus stays where it is.
    for _ in 0..3 {
        render(&context, &seen, wheel(240.0), false);
    }
    settle(&context, &seen);
    let after_wheel = focused(&context, &seen).2.min.y;
    for _ in 0..60 {
        render(&context, &seen, Vec::new(), false);
    }
    let later = focused(&context, &seen).2.min.y;
    assert!(
        (later - after_wheel).abs() < 0.5,
        "the page snapped back from {after_wheel} to {later}"
    );
}

/// After an app restart nothing has loaded the saved-library snapshot, yet the
/// Launch page already holds the game's catalogue row. The planner must use it,
/// or a catalogued ScummVM folder reads "Identity unresolved" until a scan.
#[test]
fn launch_planner_knows_a_catalogued_scummvm_folder_without_the_saved_snapshot() {
    let context = egui::Context::default();
    let mut native = native_workflows::NativeWorkflows::new(context);
    let mut row = super::archive(9, "Wetlands", Some("ScummVM"));
    row.archive_kind = "scummvm_directory".into();
    row.absolute_path = "/games/Wetlands".into();
    assert!(native.app.database_state.snapshot().is_none());
    assert!(
        native
            .app
            .persisted_scummvm_folder(Path::new("/games/Wetlands"))
            .is_none()
    );
    native.note_catalogue_row(&row);
    assert!(
        native
            .app
            .persisted_scummvm_folder(Path::new("/games/Wetlands"))
            .is_some()
    );
    // Another path, or an ordinary game, never inherits it.
    assert!(
        native
            .app
            .persisted_scummvm_folder(Path::new("/games/Other"))
            .is_none()
    );
    native.note_catalogue_row(&super::archive(10, "Plain", Some("GameCube")));
    assert!(
        native
            .app
            .persisted_scummvm_folder(Path::new("/games/Wetlands"))
            .is_none()
    );
}

/// The Library Play flow probes the installed ScummVM. That probe used to leave a
/// `scummvm.ini` in the working directory (the source tree under `cargo test`)
/// because ScummVM saves its default configuration even for `--version`.
#[cfg(unix)]
#[test]
fn scummvm_probe_leaves_no_stray_config_in_the_source_tree() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("scummvm");
    fs::write(
        &exe,
        "#!/bin/sh\ncase \"$*\" in *--config=*) ;; *) echo x > scummvm.ini;; esac\necho 'ScummVM 2.8.0 (test)'\n",
    )
    .unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let stray = std::env::current_dir().unwrap().join("scummvm.ini");
    assert!(!stray.exists(), "{} already exists", stray.display());
    assert!(archivefs_core::scummvm_detection::scummvm_version(&exe).is_some());
    assert!(!stray.exists(), "the probe created {}", stray.display());
}

fn enter() -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        })
        .collect()
}

fn click() -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| egui::Event::PointerButton {
            pos: egui::pos2(300.0, 100.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        })
        .collect()
}

fn on_library(context: &egui::Context, seen: &RefCell<Seen>) {
    for _ in 0..5 {
        render_page(context, seen, Vec::new(), false, false);
    }
}

/// Opening the Launch page with Enter from the Library must leave focus on its
/// first control, not on nothing: egui's focus order starts at the status bar and
/// sidebar, so otherwise a keyboard user tabs through every other control first.
#[test]
fn keyboard_entry_to_the_launch_page_focuses_its_first_control() {
    let context = egui::Context::default();
    let seen = RefCell::new(Seen::default());
    on_library(&context, &seen);
    render_page(&context, &seen, enter(), false, false);
    render_page(&context, &seen, Vec::new(), false, true);
    settle(&context, &seen);
    let (label, inside, rect) = focused(&context, &seen);
    assert_eq!(label, LABELS[0], "first control is focused");
    assert!(inside, "and fully visible at {rect:?}");
    // Tab then moves on to the next control in order.
    render(&context, &seen, tab(false), false);
    settle(&context, &seen);
    assert_eq!(focused(&context, &seen).0, LABELS[1]);
}

#[test]
fn mouse_entry_to_the_launch_page_does_not_take_focus() {
    let context = egui::Context::default();
    let seen = RefCell::new(Seen::default());
    on_library(&context, &seen);
    render_page(&context, &seen, click(), false, false);
    render_page(&context, &seen, Vec::new(), false, true);
    settle(&context, &seen);
    assert_eq!(context.memory(|m| m.focused()), None);
}

#[test]
fn a_page_that_stays_open_is_not_refocused_after_the_user_moves_focus() {
    let context = egui::Context::default();
    let seen = RefCell::new(Seen::default());
    on_library(&context, &seen);
    render_page(&context, &seen, enter(), false, false);
    render_page(&context, &seen, Vec::new(), false, true);
    settle(&context, &seen);
    for _ in 0..3 {
        render(&context, &seen, tab(false), false);
    }
    settle(&context, &seen);
    let moved = focused(&context, &seen).0;
    for _ in 0..30 {
        render(&context, &seen, Vec::new(), false);
    }
    assert_eq!(focused(&context, &seen).0, moved);
}

/// The launch panel appears after the page opens; focus waits for it.
#[test]
fn entry_focus_waits_for_controls_that_arrive_after_the_page_opens() {
    let context = egui::Context::default();
    let seen = RefCell::new(Seen::default());
    on_library(&context, &seen);
    render_page(&context, &seen, enter(), false, false);
    seen.borrow_mut().pending = true;
    for _ in 0..6 {
        render_page(&context, &seen, Vec::new(), false, true);
    }
    assert_eq!(
        context.memory(|m| m.focused()),
        None,
        "nothing to focus yet"
    );
    seen.borrow_mut().pending = false;
    settle(&context, &seen);
    assert_eq!(focused(&context, &seen).0, LABELS[0]);
}

#[test]
fn a_click_while_the_panel_is_loading_cancels_the_entry_focus() {
    let context = egui::Context::default();
    let seen = RefCell::new(Seen::default());
    on_library(&context, &seen);
    render_page(&context, &seen, enter(), false, false);
    seen.borrow_mut().pending = true;
    render_page(&context, &seen, Vec::new(), false, true);
    render_page(&context, &seen, click(), false, true);
    seen.borrow_mut().pending = false;
    settle(&context, &seen);
    assert_eq!(context.memory(|m| m.focused()), None);
}

/// The real flow: Play holds focus when Enter opens the page, so the old page's
/// focus is stale and must not stop the new page taking focus.
#[test]
fn entry_focus_replaces_the_stale_focus_of_the_control_that_opened_the_page() {
    let context = egui::Context::default();
    let seen = RefCell::new(Seen::default());
    on_library(&context, &seen);
    let play = seen.borrow().library_play.expect("Play was drawn");
    context.memory_mut(|m| m.request_focus(play));
    render_page(&context, &seen, Vec::new(), false, false);
    assert_eq!(context.memory(|m| m.focused()), Some(play));
    render_page(&context, &seen, enter(), false, false);
    render_page(&context, &seen, Vec::new(), false, true);
    settle(&context, &seen);
    let (label, inside, _) = focused(&context, &seen);
    assert_eq!(label, LABELS[0]);
    assert!(inside);
}
