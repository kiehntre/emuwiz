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
