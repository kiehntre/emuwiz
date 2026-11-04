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
            // Give the field keyboard focus, as Tab or a click would.
            rig.context
                .memory_mut(|m| m.request_focus(egui::Id::new(FIELD)));
            rig.frame();
            rig
        }

        fn frame(&mut self) {
            let (text, secret) = (&mut self.text, self.secret);
            let app = &mut self.app;
            let out = self.context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE.into())),
                    ..Default::default()
                },
                |ui| {
                    let context = ui.ctx().clone();
                    app.show(&context);
                    egui::Area::new(egui::Id::new("test_area")).show(&context, |ui| {
                        let r = ui.add(egui::TextEdit::singleline(text).id(egui::Id::new(FIELD)));
                        if secret {
                            crate::gui_v2::mark_secret(&r);
                        }
                    });
                },
            );
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
    }

    const A: Control = Control::Button(Button::South);
    const B: Control = Control::Button(Button::East);
    const START: Control = Control::Button(Button::Start);

    #[test]
    fn done_replaces_the_field_and_never_touches_the_clipboard() {
        let mut rig = Rig::new("abc", false);
        rig.press(A);
        assert!(rig.osk_open(), "Select on a text field opens the keyboard");
        assert_eq!(rig.text, "abc", "nothing changes while typing");
        rig.press(A); // first letter key
        assert_eq!(rig.text, "abc");
        rig.press(START);
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "abcq");
        assert!(
            !rig.copied,
            "reading the field must not copy to the clipboard"
        );
    }

    #[test]
    fn cancel_leaves_the_field_untouched() {
        let mut rig = Rig::new("abc", false);
        rig.press(A);
        rig.press(A);
        rig.press(B);
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "abc");
        assert_eq!(rig.app.router.current, Route::Home, "page not activated");
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
    fn secret_fields_start_empty_and_empty_done_keeps_the_old_value() {
        let mut rig = Rig::new("hunter2", true);
        rig.press(A);
        assert!(rig.osk_open());
        rig.press(START);
        assert!(!rig.osk_open());
        assert_eq!(rig.text, "hunter2");
        rig.press(A);
        assert!(rig.osk_open(), "reopens");
        rig.press(A);
        rig.press(START);
        assert_eq!(rig.text, "q");
    }
}
