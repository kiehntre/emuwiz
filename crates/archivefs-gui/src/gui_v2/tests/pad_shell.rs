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
    mut body: impl FnMut(&egui::Context),
) -> egui::FullOutput {
    let before = super::super::pad_shell::hook_runs(context);
    let out = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE.into())),
            events,
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
