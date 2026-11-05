//! Controller input on ordinary Blue pages, through real frames of the App.
//! Input enters at the same `PadEvent` level the evdev reader produces.
use super::big_picture::nav::{Button, Control, Direction, PadEvent};
use super::pad_shell::neighbour_destination;
use super::simple_shell::{DESTINATIONS, Destination};
use super::*;

const SIZE: [f32; 2] = [1400.0, 900.0];

fn app(context: &egui::Context) -> App {
    let mut app = fixture(context);
    app.library = Arc::new(Library::new(vec![archive(1, "Game 1", Some("SNES"))]));
    app.loaded = true;
    app.router.current = Route::Home;
    let _ = frame(context, &mut app, SIZE);
    let _ = frame(context, &mut app, SIZE);
    app
}

fn press(context: &egui::Context, app: &mut App, control: Control) {
    let shell = app.pad_shell.as_mut().expect("shell exists after a frame");
    shell.inject(PadEvent::Pressed { device: 1, control });
    shell.inject(PadEvent::Released { device: 1, control });
    let _ = frame(context, app, SIZE);
    let _ = frame(context, app, SIZE);
}

#[test]
fn destinations_step_and_wrap() {
    assert_eq!(
        neighbour_destination(Some(Destination::Home), 1),
        Destination::Library
    );
    assert_eq!(
        neighbour_destination(Some(Destination::Home), -1),
        Destination::Settings
    );
    assert_eq!(
        neighbour_destination(Some(Destination::Settings), 1),
        Destination::Home
    );
    assert_eq!(neighbour_destination(None, 1), DESTINATIONS[0]);
}

#[test]
fn directions_move_focus_and_select_acts_only_on_the_focused_control() {
    let context = egui::Context::default();
    let mut app = app(&context);
    assert_eq!(context.memory(|m| m.focused()), None);
    press(&context, &mut app, Control::Direction(Direction::Down));
    assert!(
        context.memory(|m| m.focused()).is_some(),
        "Down focuses a control"
    );
    // Select with nothing deliberately chosen must not navigate away.
    let before = app.router.current.clone();
    press(&context, &mut app, Control::Direction(Direction::Down));
    assert_eq!(app.router.current, before, "moving never activates");
}

#[test]
fn y_button_moves_to_the_next_section_and_start_opens_big_picture() {
    let context = egui::Context::default();
    let mut app = app(&context);
    press(&context, &mut app, Control::Button(Button::North));
    assert_eq!(
        super::simple_shell::destination_for(&app.router.current),
        Some(Destination::Library)
    );
    press(&context, &mut app, Control::Button(Button::Start));
    assert!(app.big_picture.is_some());
    assert!(app.pad_shell.is_none(), "Big Picture owns the reader");
}

#[test]
fn x_and_y_walk_every_destination_both_ways_including_tools() {
    let context = egui::Context::default();
    let mut app = app(&context);
    app.simple.enabled = true;
    let _ = frame(&context, &mut app, SIZE);
    let mut seen = Vec::new();
    for _ in 0..DESTINATIONS.len() {
        press(&context, &mut app, Control::Button(Button::North));
        seen.push(if app.simple.all_tools_open() {
            Some(Destination::Tools)
        } else {
            super::simple_shell::destination_for(&app.router.current)
        });
    }
    let expected: Vec<_> = DESTINATIONS[1..]
        .iter()
        .chain(&DESTINATIONS[..1])
        .map(|d| Some(*d))
        .collect();
    assert_eq!(
        seen, expected,
        "Y visits all eight, Tools included, and wraps"
    );
    press(&context, &mut app, Control::Button(Button::West));
    assert_eq!(
        super::simple_shell::destination_for(&app.router.current),
        Some(Destination::Settings),
        "X goes back"
    );
    assert!(!app.simple.all_tools_open());
}

#[test]
fn unfocused_window_ignores_the_controller() {
    let context = egui::Context::default();
    let mut app = app(&context);
    app.pad_shell.as_mut().unwrap().inject(PadEvent::Pressed {
        device: 1,
        control: Control::Button(Button::North),
    });
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE.into())),
            focused: false,
            ..Default::default()
        },
        |context| app.show(context),
    );
    assert_eq!(app.router.current, Route::Home);
}

/// The one way controller text-entry tests draw a frame. It uses `run_ui`
/// (egui runs end-of-pass hooks only there, as the real app does) and fails
/// explicitly if the hook did not run, so a test can never silently exercise
/// a keyboard that cannot read its field. Returns the frame's output.
fn run_ui_frame(
    context: &egui::Context,
    events: Vec<egui::Event>,
    body: impl FnMut(&egui::Context),
) -> egui::FullOutput {
    run_ui_frame_at(context, None, events, body)
}

fn run_ui_frame_at(
    context: &egui::Context,
    time: Option<f64>,
    events: Vec<egui::Event>,
    mut body: impl FnMut(&egui::Context),
) -> egui::FullOutput {
    let before = super::super::pad_shell::hook_runs(context);
    let out = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE.into())),
            events,
            time,
            ..Default::default()
        },
        |ui| body(&ui.ctx().clone()),
    );
    assert!(
        super::super::pad_shell::hook_runs(context) > before,
        "the end-of-pass hook did not run: controller text-entry tests must draw frames through run_ui"
    );
    out
}

mod text_entry {
    use super::*;

    struct Rig {
        context: egui::Context,
        app: App,
        text: String,
        secret: bool,
        copied: bool,
    }

    const FIELD: &str = "pad_shell_test_field";

    impl Rig {
        fn new(text: &str, secret: bool) -> Self {
            let context = egui::Context::default();
            let app = super::app(&context);
            let mut rig = Self {
                context,
                app,
                text: text.into(),
                secret,
                copied: false,
            };
            rig.frame();
            rig.focus_field();
            rig
        }

        fn focus_field(&mut self) {
            // As Tab or a click would.
            self.context
                .memory_mut(|m| m.request_focus(egui::Id::new(FIELD)));
            self.frame();
        }

        fn frame(&mut self) {
            self.frame_with(Vec::new());
        }

        fn frame_with(&mut self, events: Vec<egui::Event>) {
            let (text, secret) = (&mut self.text, self.secret);
            let app = &mut self.app;
            let out = run_ui_frame(&self.context, events, |context| {
                app.show(context);
                egui::Area::new(egui::Id::new("test_area")).show(context, |ui| {
                    let r = ui.add(egui::TextEdit::singleline(text).id(egui::Id::new(FIELD)));
                    if secret {
                        crate::gui_v2::mark_secret(&r);
                    }
                });
            });
            self.copied |= out
                .platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::CopyText(_)));
        }

        fn press(&mut self, control: Control) {
            let shell = self.app.pad_shell.as_mut().unwrap();
            shell.inject(PadEvent::Pressed { device: 1, control });
            shell.inject(PadEvent::Released { device: 1, control });
            for _ in 0..3 {
                self.frame();
            }
        }

        fn osk_open(&self) -> bool {
            self.app.pad_shell.as_ref().unwrap().osk_open()
        }

        fn buffer(&self) -> Option<String> {
            self.app.pad_shell.as_ref().unwrap().buffer()
        }

        fn focused_is_field(&self) -> bool {
            self.context.memory(|m| m.focused()) == Some(egui::Id::new(FIELD))
        }
    }

    const A: Control = Control::Button(Button::South);
    const B: Control = Control::Button(Button::East);
    const X: Control = Control::Button(Button::West);
    const START: Control = Control::Button(Button::Start);

    #[test]
    fn prefilled_text_is_read_into_the_keyboard_and_done_replaces_the_field() {
        let mut rig = Rig::new("abc", false);
        rig.press(A);
        assert!(rig.osk_open(), "Select on a text field opens the keyboard");
        assert_eq!(
            rig.buffer().as_deref(),
            Some("abc"),
            "existing text is read"
        );
        assert_eq!(rig.text, "abc", "nothing changes while typing");
        rig.press(A); // first letter key
        assert_eq!(rig.text, "abc", "typing is staged, not applied");
        assert_eq!(rig.buffer().as_deref(), Some("abcq"));
        rig.press(START);
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "abcq");
        assert!(rig.focused_is_field(), "focus returns to the field");
    }

    #[test]
    fn empty_field_opens_empty_and_done_applies_what_was_typed() {
        let mut rig = Rig::new("", false);
        rig.press(A);
        assert!(rig.osk_open());
        assert_eq!(rig.buffer().as_deref(), Some(""));
        rig.press(A);
        rig.press(START);
        assert_eq!(rig.text, "q");
    }

    #[test]
    fn deleting_everything_then_done_empties_the_field() {
        let mut rig = Rig::new("ab", false);
        rig.press(A);
        rig.press(X);
        rig.press(X);
        assert_eq!(rig.buffer().as_deref(), Some(""));
        assert_eq!(rig.text, "ab");
        rig.press(START);
        assert_eq!(rig.text, "");
    }

    #[test]
    fn cancel_leaves_the_field_untouched_and_returns_focus() {
        let mut rig = Rig::new("abc", false);
        rig.press(A);
        rig.press(A);
        assert_eq!(rig.buffer().as_deref(), Some("abcq"));
        rig.press(B);
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "abc");
        assert!(rig.focused_is_field());
        assert_eq!(rig.app.router.current, Route::Home, "page not activated");
    }

    #[test]
    fn escape_on_a_physical_keyboard_also_cancels() {
        let mut rig = Rig::new("abc", false);
        rig.press(A);
        rig.press(A);
        rig.frame_with(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        rig.frame();
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "abc");
    }

    #[test]
    fn keys_do_not_reach_the_page_while_the_keyboard_is_open() {
        let mut rig = Rig::new("abc", false);
        rig.press(A);
        let route = rig.app.router.current.clone();
        rig.press(Control::Button(Button::North)); // shift, not next section
        rig.press(Control::Direction(Direction::Down));
        assert_eq!(rig.app.router.current, route);
        assert!(rig.osk_open());
    }

    #[test]
    fn focus_lost_while_typing_is_restored_on_done_and_on_cancel() {
        for finish in [START, B] {
            let mut rig = Rig::new("abc", false);
            rig.press(A);
            rig.context
                .memory_mut(|m| m.surrender_focus(egui::Id::new(FIELD)));
            rig.frame();
            assert!(rig.osk_open(), "losing focus does not close the keyboard");
            rig.press(A);
            rig.press(finish);
            assert!(rig.focused_is_field(), "focus back on the field");
            assert_eq!(rig.text, if finish == START { "abcq" } else { "abc" });
        }
    }

    #[test]
    fn physical_typing_closes_the_keyboard_and_reaches_the_field() {
        let mut rig = Rig::new("abc", false);
        rig.press(A);
        rig.frame_with(vec![egui::Event::Text("z".into())]);
        rig.frame();
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "abcz");
    }

    #[test]
    fn secret_fields_start_empty_and_empty_done_keeps_the_old_value() {
        let mut rig = Rig::new("hunter2", true);
        rig.press(A);
        assert!(rig.osk_open());
        assert_eq!(
            rig.buffer().as_deref(),
            Some(""),
            "a secret is never read back"
        );
        rig.press(START);
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "hunter2");
        rig.press(A);
        assert!(rig.osk_open(), "reopens");
        rig.press(A);
        rig.press(START);
        assert_eq!(rig.text, "q");
        assert!(!rig.copied);
    }

    #[test]
    fn reading_the_field_never_reaches_the_clipboard_but_real_copies_still_do() {
        let mut rig = Rig::new("secret-ish path", false);
        rig.press(A);
        rig.press(B);
        rig.press(A);
        rig.press(START);
        assert!(
            !rig.copied,
            "the capture copy command must be consumed before it reaches the clipboard"
        );
        // A deliberate Ctrl+C by the user is untouched by the capture hook.
        rig.focus_field();
        rig.frame_with(vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
            egui::Event::Copy,
        ]);
        assert!(rig.copied, "a user's own copy still reaches the clipboard");
    }

    #[test]
    fn without_the_end_of_pass_hook_the_keyboard_fails_closed() {
        // Plain `run` skips egui's end-of-pass hooks. The field's text can't be
        // read, so the keyboard must not open on text it never saw.
        let mut rig = Rig::new("abc", false);
        rig.app
            .pad_shell
            .as_mut()
            .unwrap()
            .inject(PadEvent::Pressed {
                device: 1,
                control: A,
            });
        for _ in 0..6 {
            let (text, app) = (&mut rig.text, &mut rig.app);
            let _ = rig.context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE.into())),
                    ..Default::default()
                },
                |context| {
                    app.show(context);
                    egui::Area::new(egui::Id::new("test_area")).show(context, |ui| {
                        ui.add(egui::TextEdit::singleline(text).id(egui::Id::new(FIELD)));
                    });
                },
            );
        }
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "abc");
    }
}

mod picker_journey {
    use super::*;
    use crate::ui::picker::{self, PickRequest};

    struct Rig {
        context: egui::Context,
        app: App,
        taken: Vec<std::path::PathBuf>,
        behind_clicks: u32,
    }

    fn slot() -> egui::Id {
        egui::Id::new("picker_test_slot")
    }

    impl Rig {
        fn new() -> Self {
            let context = egui::Context::default();
            let app = super::app(&context);
            let mut rig = Self {
                context,
                app,
                taken: Vec::new(),
                behind_clicks: 0,
            };
            rig.frame();
            picker::set_controller_in_use(&rig.context, true);
            rig
        }

        fn frame(&mut self) {
            let (app, taken, clicks) = (&mut self.app, &mut self.taken, &mut self.behind_clicks);
            run_ui_frame(&self.context, Vec::new(), |context| {
                app.show(context);
                egui::Area::new(egui::Id::new("behind")).show(context, |ui| {
                    if ui.button("Behind the picker").clicked() {
                        *clicks += 1;
                    }
                });
                if let Some(path) = picker::take(context, slot()) {
                    taken.push(path);
                }
            });
        }

        fn press(&mut self, control: Control) {
            let shell = self.app.pad_shell.as_mut().unwrap();
            shell.inject(PadEvent::Pressed { device: 1, control });
            shell.inject(PadEvent::Released { device: 1, control });
            for _ in 0..3 {
                self.frame();
            }
        }

        fn open(&mut self, request: PickRequest) {
            picker::request(&self.context, slot(), request);
            for _ in 0..3 {
                self.frame();
            }
        }
    }

    const A: Control = Control::Button(Button::South);
    const B: Control = Control::Button(Button::East);
    const DOWN: Control = Control::Direction(Direction::Down);

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Games")).unwrap();
        std::fs::write(dir.path().join("Games").join("a.iso"), b"x").unwrap();
        std::fs::write(dir.path().join("Games").join("b.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("Games").join(".hidden"), b"x").unwrap();
        dir
    }

    fn request_at(dir: &std::path::Path, folder: bool) -> PickRequest {
        let mut request = if folder {
            PickRequest::folder("Pick")
        } else {
            PickRequest::file("Pick", &["iso"])
        };
        request.start = Some(dir.to_path_buf());
        request
    }

    #[test]
    fn controller_can_choose_the_current_folder_without_touching_the_page_behind() {
        let dir = tree();
        let mut rig = Rig::new();
        rig.open(request_at(dir.path(), true));
        assert!(picker::open(&rig.context));
        // Walk the whole list: focus never leaves the picker.
        for _ in 0..12 {
            rig.press(DOWN);
            assert!(rig.context.memory(|m| m.focused()).is_some());
        }
        assert_eq!(rig.behind_clicks, 0, "moving never presses the page behind");
        // Go back to the first row, "Use this folder", and choose it.
        for _ in 0..12 {
            rig.press(Control::Direction(Direction::Up));
        }
        rig.press(A);
        assert!(!picker::open(&rig.context));
        assert_eq!(rig.taken, vec![dir.path().to_path_buf()]);
        assert_eq!(rig.behind_clicks, 0);
    }

    #[test]
    fn controller_can_open_a_folder_and_pick_a_filtered_file() {
        let dir = tree();
        let mut rig = Rig::new();
        rig.open(request_at(dir.path(), false));
        // First row after the shortcuts: "Up one folder", then "Games".
        rig.press(DOWN);
        rig.press(A);
        assert!(
            picker::open(&rig.context),
            "entering a folder keeps the picker open"
        );
        // Focus restarts on the first row ("Up one folder"); the files follow.
        rig.press(DOWN);
        rig.press(A);
        assert_eq!(rig.taken, vec![dir.path().join("Games").join("a.iso")]);
    }

    #[test]
    fn back_cancels_and_answers_nothing() {
        let dir = tree();
        let mut rig = Rig::new();
        rig.open(request_at(dir.path(), true));
        rig.press(B);
        assert!(!picker::open(&rig.context));
        assert!(rig.taken.is_empty());
        assert_eq!(rig.behind_clicks, 0);
    }

    #[test]
    fn section_buttons_do_nothing_while_the_picker_is_open() {
        let dir = tree();
        let mut rig = Rig::new();
        let route = rig.app.router.current.clone();
        rig.open(request_at(dir.path(), true));
        for button in [Button::North, Button::West, Button::Start] {
            rig.press(Control::Button(button));
        }
        assert_eq!(rig.app.router.current, route);
        assert!(picker::open(&rig.context));
        assert!(rig.app.big_picture.is_none());
    }
}

mod dialog_safety {
    use super::*;

    struct Rig {
        context: egui::Context,
        app: App,
        time: f64,
        cancel: u32,
        destroy: u32,
        page: u32,
    }

    const A: Control = Control::Button(Button::South);

    impl Rig {
        fn new() -> Self {
            let context = egui::Context::default();
            let app = super::app(&context);
            let mut rig = Self {
                context,
                app,
                time: 1.0,
                cancel: 0,
                destroy: 0,
                page: 0,
            };
            rig.frame();
            rig
        }

        fn frame(&mut self) {
            self.time += 0.1;
            let (app, cancel, destroy, page) = (
                &mut self.app,
                &mut self.cancel,
                &mut self.destroy,
                &mut self.page,
            );
            run_ui_frame_at(&self.context, Some(self.time), Vec::new(), |context| {
                app.show(context);
                egui::Area::new(egui::Id::new("page_area"))
                    .order(egui::Order::Background)
                    .show(context, |ui| {
                        if ui.button("Delete page thing").clicked() {
                            *page += 1;
                        }
                    });
                egui::Window::new("Are you sure?").show(context, |ui| {
                    if ui.button("Cancel").clicked() {
                        *cancel += 1;
                    }
                    if ui.button("Delete everything").clicked() {
                        *destroy += 1;
                    }
                });
            });
        }

        fn focus(&mut self, label: &str) {
            // Walk with the controller's own Down until the label is focused.
            for _ in 0..120 {
                self.step(Control::Direction(Direction::Down));
                if self.focused_label().as_deref() == Some(label) {
                    return;
                }
            }
            panic!("never focused {label}");
        }

        fn focused_label(&self) -> Option<String> {
            self.context
                .data(|d| {
                    d.get_temp::<(egui::Id, String, bool)>(egui::Id::new("pad_shell_focus_label"))
                })
                .map(|(_, label, _)| label)
        }

        fn step(&mut self, control: Control) {
            let shell = self.app.pad_shell.as_mut().unwrap();
            shell.inject(PadEvent::Pressed { device: 1, control });
            shell.inject(PadEvent::Released { device: 1, control });
            for _ in 0..3 {
                self.frame();
            }
        }

        fn hold_a(&mut self, frames: usize) {
            let shell = self.app.pad_shell.as_mut().unwrap();
            shell.inject(PadEvent::Pressed {
                device: 1,
                control: A,
            });
            for _ in 0..frames {
                self.frame();
            }
        }

        fn release_a(&mut self) {
            let shell = self.app.pad_shell.as_mut().unwrap();
            shell.inject(PadEvent::Released {
                device: 1,
                control: A,
            });
            for _ in 0..3 {
                self.frame();
            }
        }
    }

    #[test]
    fn a_tap_never_presses_an_unrecognised_dialog_button() {
        let mut rig = Rig::new();
        rig.focus("Delete everything");
        rig.step(A);
        assert_eq!(rig.destroy, 0, "a tap must not confirm");
    }

    #[test]
    fn a_tap_presses_cancel() {
        let mut rig = Rig::new();
        rig.focus("Cancel");
        rig.step(A);
        assert_eq!(rig.cancel, 1);
        assert_eq!(rig.destroy, 0);
    }

    #[test]
    fn holding_a_long_enough_confirms_and_releasing_early_does_not() {
        let mut rig = Rig::new();
        rig.focus("Delete everything");
        rig.hold_a(3); // ~0.3 s
        rig.release_a();
        assert_eq!(rig.destroy, 0, "released early");
        rig.hold_a(9); // ~0.9 s
        assert_eq!(rig.destroy, 1, "held long enough");
        rig.release_a();
        assert_eq!(rig.destroy, 1, "fires once");
    }

    #[test]
    fn moving_focus_during_a_hold_cancels_it() {
        let mut rig = Rig::new();
        rig.focus("Delete everything");
        rig.hold_a(2);
        rig.context
            .memory_mut(|m| m.request_focus(egui::Id::new("nowhere")));
        rig.frame();
        rig.hold_a(9);
        assert_eq!(rig.destroy, 0);
    }

    #[test]
    fn buttons_on_the_page_itself_are_not_slowed_down() {
        let mut rig = Rig::new();
        rig.focus("Delete page thing");
        rig.step(A);
        assert_eq!(rig.page, 1, "the dialog guard is for dialogs only");
    }
}

/// The acceptance journey, driven only by controller events: Home → Library →
/// a game's details → Verify Games → back → Setup → Enhance (saves) → back to
/// Library → Big Picture. (Emulator configuration and real devices are covered
/// by the Nobara acceptance run; this proves the navigation path needs no mouse.)
mod journey {
    use super::*;

    struct Pad {
        context: egui::Context,
        app: App,
        seen: Vec<String>,
    }

    impl Pad {
        fn frames(&mut self) {
            for _ in 0..3 {
                let app = &mut self.app;
                run_ui_frame(&self.context, Vec::new(), |ctx| app.show(ctx));
            }
        }
        fn tap(&mut self, control: Control) {
            let shell = self
                .app
                .pad_shell
                .as_mut()
                .expect("controller layer active");
            shell.inject(PadEvent::Pressed { device: 1, control });
            shell.inject(PadEvent::Released { device: 1, control });
            self.frames();
        }
        fn label(&self) -> Option<String> {
            self.context
                .data(|d| {
                    d.get_temp::<(egui::Id, String, bool)>(egui::Id::new("pad_shell_focus_label"))
                })
                .map(|x| x.1)
        }
        /// Walk focus with Down until a control whose label satisfies `wanted`.
        fn focus(&mut self, wanted: impl Fn(&str) -> bool, what: &str) {
            for _ in 0..200 {
                self.tap(Control::Direction(Direction::Down));
                if let Some(l) = self.label() { self.seen.push(l); }
                if self.label().is_some_and(|l| wanted(&l)) {
                    return;
                }
            }
            panic!(
                "controller could not reach {what}; last focus {:?}; seen {:?}",
                self.label(),
                self.seen
            );
        }
        fn destination(&self) -> Option<super::super::simple_shell::Destination> {
            super::super::simple_shell::destination_for(&self.app.router.current)
        }
    }

    #[test]
    fn launch_browse_details_verify_setup_saves_and_big_picture_without_a_mouse() {
        let context = egui::Context::default();
        let mut app = super::app(&context);
        app.simple.enabled = true;
        app.artwork.index = Some(Arc::new(MediaIndex::default()));
        let mut pad = Pad { context, app, seen: Vec::new() };
        pad.frames();
        let north = Control::Button(Button::North);
        let south = Control::Button(Button::South);
        let east = Control::Button(Button::East);

        pad.tap(north); // Home → Library
        assert_eq!(pad.destination(), Some(Destination::Library));

        // Open a game's details from the list.
        pad.focus(|l| l.contains("Game 1"), "the game in the Library");
        pad.tap(south);
        assert!(
            matches!(
                pad.app.router.current,
                Route::BrowsePlayGame(_) | Route::Game(_)
            ),
            "A opens the game's details, got {:?}",
            pad.app.router.current
        );
        pad.tap(east); // Back to the Library
        assert_eq!(pad.destination(), Some(Destination::Library));
        assert!(matches!(pad.app.router.current, Route::BrowsePlay));

        // Verify Games is reachable and opens its page.
        pad.focus(|l| l == "Verify Games", "Verify Games");
        pad.tap(south);
        assert_ne!(
            pad.app.router.current,
            Route::BrowsePlay,
            "Verify Games opened"
        );
        pad.tap(east);

        // Setup, then Enhance (cheats, mods, saves), with the section buttons.
        pad.tap(north);
        assert_eq!(pad.destination(), Some(Destination::Setup));
        for _ in 0..3 {
            pad.tap(north);
        }
        assert_eq!(pad.destination(), Some(Destination::Enhance));

        // Back to the Library by section buttons, then into Big Picture.
        for _ in 0..4 {
            pad.tap(Control::Button(Button::West));
        }
        assert_eq!(pad.destination(), Some(Destination::Library));
        pad.focus(|l| l == "Big Picture", "Big Picture");
        pad.tap(south);
        assert!(
            pad.app.big_picture.is_some(),
            "Big Picture opened from the pad"
        );
    }
}
