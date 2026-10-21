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
                super::super::simple_shell::show_page_scroll(ui, Section::Launch, false, |ui| {
                    super::super::native_workflows::launch_scroll(ui, 7, |ui| {
                        seen.borrow_mut().clip = Some(ui.clip_rect());
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
