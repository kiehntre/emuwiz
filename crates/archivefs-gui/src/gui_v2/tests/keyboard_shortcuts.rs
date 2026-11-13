//! Blue's shared keyboard shortcuts: the pure precedence, and the real App
//! frame with injected `egui::RawInput` events.
use super::*;
use crate::gui_v2::routes::{Route, Section};
use crate::gui_v2::tests::{archive, fixture};
use eframe::egui;
use std::sync::Arc;

const SIZE: [f32; 2] = [1280.0, 820.0];

fn search() -> Option<egui::Id> {
    Some(egui::Id::new("search"))
}

fn every_gate_that_owns_the_keys() -> [Gate; 5] {
    [
        Gate {
            typing: true,
            ..Gate::default()
        },
        Gate {
            confirmation: true,
            ..Gate::default()
        },
        Gate {
            popup: true,
            ..Gate::default()
        },
        Gate {
            window: true,
            ..Gate::default()
        },
        Gate {
            overlay: true,
            ..Gate::default()
        },
    ]
}

#[test]
fn shortcuts_act_on_a_free_page() {
    let keys = |focus_search, backspace| Keys {
        focus_search,
        backspace,
        escape: false,
    };
    assert_eq!(
        resolve(keys(true, false), Gate::default(), search(), false),
        vec![Shortcut::FocusSearch(search().unwrap())]
    );
    assert_eq!(
        resolve(keys(false, true), Gate::default(), search(), false),
        vec![Shortcut::Back]
    );
    // A page without a search field: `/` does nothing.
    assert!(resolve(keys(true, false), Gate::default(), None, false).is_empty());
}

#[test]
fn text_focus_modals_menus_and_overlays_own_the_keys() {
    let keys = Keys {
        focus_search: true,
        backspace: true,
        escape: true,
    };
    for gate in every_gate_that_owns_the_keys() {
        assert!(!gate.free());
        assert!(
            resolve(keys, gate, search(), true).is_empty(),
            "{gate:?} must keep its keys"
        );
    }
}

#[test]
fn escape_is_back_unless_a_page_claimed_it() {
    let escape = Keys {
        escape: true,
        ..Keys::default()
    };
    // Unclaimed Escape stays with App::show's existing precedence (Back).
    assert!(resolve(escape, Gate::default(), search(), false).is_empty());
    assert_eq!(
        resolve(escape, Gate::default(), search(), true),
        vec![Shortcut::ClaimedEscape]
    );
}

fn key(key: egui::Key, modifiers: egui::Modifiers, repeat: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat,
        modifiers,
    }
}

fn ctrl() -> egui::Modifiers {
    egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    }
}

fn keys_of(events: Vec<egui::Event>, modifiers: egui::Modifiers) -> Keys {
    let context = egui::Context::default();
    let mut keys = Keys::default();
    let _ = context.run(
        egui::RawInput {
            events,
            modifiers,
            ..Default::default()
        },
        |context| keys = context.input(Keys::read),
    );
    keys
}

#[test]
fn keys_are_read_exactly() {
    let none = egui::Modifiers::NONE;
    assert!(keys_of(vec![egui::Event::Text("/".into())], none).focus_search);
    assert!(keys_of(vec![key(egui::Key::Slash, none, false)], none).focus_search);
    assert!(keys_of(vec![key(egui::Key::F, ctrl(), false)], ctrl()).focus_search);
    assert!(!keys_of(vec![key(egui::Key::F, none, false)], none).focus_search);
    assert!(keys_of(vec![key(egui::Key::Backspace, none, false)], none).backspace);
    // Ctrl+Backspace deletes a word; it is never Back.
    assert!(!keys_of(vec![key(egui::Key::Backspace, ctrl(), false)], ctrl()).backspace);
    // Enter and Space are never a shortcut.
    for activation in [egui::Key::Enter, egui::Key::Space] {
        assert_eq!(
            keys_of(vec![key(activation, none, false)], none),
            Keys::default()
        );
    }
}

fn release(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// The key goes down before the button has focus and is never released;
/// once the button holds focus, one more press arrives: an auto-repeat
/// (egui marks it so), or, with `released`, a fresh press after a release.
fn button_clicked_by_held(key_held: egui::Key, released: bool, filtered: bool) -> bool {
    let context = egui::Context::default();
    let mut clicked = false;
    let mut focused = false;
    let press = key(key_held, egui::Modifiers::NONE, false);
    let last = if released {
        vec![release(key_held), press.clone()]
    } else {
        vec![press.clone()]
    };
    for (frame, events) in [vec![press], Vec::new(), Vec::new(), last]
        .into_iter()
        .enumerate()
    {
        let _ = context.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |context| {
                if filtered {
                    context.input_mut(drop_held_activation);
                }
                egui::CentralPanel::default().show(context, |ui| {
                    let response = ui.add(egui::Button::new("Play"));
                    if frame == 0 {
                        response.request_focus();
                    }
                    focused = response.has_focus();
                    clicked |= response.clicked();
                });
            },
        );
    }
    assert!(focused, "the button should hold keyboard focus");
    clicked
}

#[test]
fn a_held_enter_never_activates_the_focused_control() {
    use egui::Key::{Enter, Space};
    // egui alone clicks on an auto-repeat.
    assert!(button_clicked_by_held(Enter, false, false));
    assert!(!button_clicked_by_held(Enter, false, true));
    assert!(!button_clicked_by_held(Space, false, true));
    // A deliberate press still activates the focused control.
    assert!(button_clicked_by_held(Enter, true, true));
    assert!(button_clicked_by_held(Space, true, true));
}

fn run(
    context: &egui::Context,
    app: &mut App,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(SIZE[0], SIZE[1]),
            )),
            events,
            modifiers,
            ..Default::default()
        },
        |context| app.show_with_shortcuts(context),
    );
}

fn idle(context: &egui::Context, app: &mut App) {
    run(context, app, Vec::new(), egui::Modifiers::NONE);
}

fn press(context: &egui::Context, app: &mut App, event: egui::Event) {
    let modifiers = match &event {
        egui::Event::Key { modifiers, .. } => *modifiers,
        _ => egui::Modifiers::NONE,
    };
    run(context, app, vec![event], modifiers);
}

fn slash() -> Vec<egui::Event> {
    vec![
        key(egui::Key::Slash, egui::Modifiers::NONE, false),
        egui::Event::Text("/".into()),
    ]
}

/// The production view: EmuWiz Blue, with its header library search.
fn blue(context: &egui::Context) -> App {
    let mut app = fixture(context);
    app.simple = crate::gui_v2::simple_shell::ShellState::default();
    app
}

/// Two pages deep, so Back has somewhere to go.
fn app_on_problems(context: &egui::Context) -> App {
    let mut app = blue(context);
    app.go(Route::Section(Section::Games));
    app.go(Route::Section(Section::Problems));
    idle(context, &mut app);
    app
}

#[test]
fn backspace_goes_back_only_when_nothing_else_owns_it() {
    let context = egui::Context::default();
    let mut app = app_on_problems(&context);
    let backspace = || key(egui::Key::Backspace, egui::Modifiers::NONE, false);

    // A confirmation is open: Backspace never goes Back underneath it.
    app.confirm_scan = true;
    idle(&context, &mut app);
    press(&context, &mut app, backspace());
    assert_eq!(app.router.current, Route::Section(Section::Problems));
    assert!(app.confirm_scan, "Backspace must not answer the dialog");
    app.confirm_scan = false;
    idle(&context, &mut app);

    // Typing in the search field: Backspace edits the text.
    run(&context, &mut app, slash(), egui::Modifiers::NONE);
    idle(&context, &mut app);
    assert!(context.text_edit_focused(), "`/` should focus the search");
    app.filter.search = "ab".into();
    press(&context, &mut app, backspace());
    assert_eq!(app.router.current, Route::Section(Section::Problems));

    // Leave the field (Escape), then Backspace is Back.
    press(
        &context,
        &mut app,
        key(egui::Key::Escape, egui::Modifiers::NONE, false),
    );
    idle(&context, &mut app);
    assert!(!context.text_edit_focused());
    assert_eq!(app.router.current, Route::Section(Section::Problems));
    press(&context, &mut app, backspace());
    assert_eq!(app.router.current, Route::Section(Section::Games));
}

#[test]
fn slash_and_ctrl_f_focus_the_search_without_typing_into_it() {
    for events in [slash(), vec![key(egui::Key::F, ctrl(), false)]] {
        let context = egui::Context::default();
        let mut app = app_on_problems(&context);
        assert!(!context.text_edit_focused());
        let modifiers = match &events[0] {
            egui::Event::Key { modifiers, .. } => *modifiers,
            _ => egui::Modifiers::NONE,
        };
        run(&context, &mut app, events, modifiers);
        idle(&context, &mut app);
        assert!(
            context.text_edit_focused(),
            "the search field should have focus"
        );
        assert_eq!(app.filter.search, "", "the key must not be typed");
        assert_eq!(app.router.current, Route::Section(Section::Problems));
    }
}

#[test]
fn slash_does_nothing_while_a_confirmation_is_open() {
    let context = egui::Context::default();
    let mut app = app_on_problems(&context);
    app.confirm_scan = true;
    idle(&context, &mut app);
    run(&context, &mut app, slash(), egui::Modifiers::NONE);
    idle(&context, &mut app);
    assert!(!context.text_edit_focused());
    assert!(app.confirm_scan);
}

#[test]
fn a_page_that_claims_escape_keeps_back_from_running() {
    let context = egui::Context::default();
    let mut app = app_on_problems(&context);
    claim_escape(&context);
    let mut seen = false;
    let _ = context.run(
        egui::RawInput {
            events: vec![key(egui::Key::Escape, egui::Modifiers::NONE, false)],
            ..Default::default()
        },
        |context| {
            app.show_with_shortcuts(context);
            seen = claimed_escape_pressed(context);
        },
    );
    assert!(seen);
    assert_eq!(app.router.current, Route::Section(Section::Problems));
    // Unclaimed, the next Escape is the ordinary Back.
    press(
        &context,
        &mut app,
        key(egui::Key::Escape, egui::Modifiers::NONE, false),
    );
    assert_eq!(app.router.current, Route::Section(Section::Games));
}

/// Tab until the Library card named `title` has keyboard focus.
fn tab_to_card(context: &egui::Context, app: &mut App, title: &str) {
    for _ in 0..200 {
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(SIZE[0], SIZE[1]),
                )),
                events: vec![key(egui::Key::Tab, egui::Modifiers::NONE, false)],
                ..Default::default()
            },
            |context| app.show_with_shortcuts(context),
        );
        let reached = output.platform_output.events.iter().any(|event| {
            matches!(event, egui::output::OutputEvent::FocusGained(info)
                if info.label.as_deref() == Some(title))
        });
        if reached {
            return;
        }
    }
    panic!("Tab never reached the {title} card");
}

#[test]
fn enter_on_a_library_card_opens_game_details_and_a_held_enter_goes_no_further() {
    let context = egui::Context::default();
    let mut app = blue(&context);
    app.library = Arc::new(crate::gui_v2::library::Library::new(vec![archive(
        1,
        "Mario",
        Some("SNES"),
    )]));
    app.go(Route::BrowsePlay);
    idle(&context, &mut app);
    tab_to_card(&context, &mut app, "Mario");
    press(
        &context,
        &mut app,
        key(egui::Key::Enter, egui::Modifiers::NONE, false),
    );
    assert_eq!(app.router.current, Route::BrowsePlayGame(1));
    // Holding Enter: only auto-repeats follow. None of them may reach Play.
    for _ in 0..12 {
        press(
            &context,
            &mut app,
            key(egui::Key::Enter, egui::Modifiers::NONE, true),
        );
    }
    assert_eq!(app.router.current, Route::BrowsePlayGame(1));
}
