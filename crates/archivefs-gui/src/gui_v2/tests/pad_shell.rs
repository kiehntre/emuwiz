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
        suggest: bool,
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
                suggest: false,
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
            let (text, secret, suggest) = (&mut self.text, self.secret, self.suggest);
            let app = &mut self.app;
            let out = run_ui_frame(&self.context, events, |context| {
                app.show(context);
                egui::Area::new(egui::Id::new("test_area")).show(context, |ui| {
                    let r = ui.add(egui::TextEdit::singleline(text).id(egui::Id::new(FIELD)));
                    if secret {
                        crate::gui_v2::mark_secret(&r);
                    }
                    if suggest {
                        super::super::super::pad_shell::mark_suggestions(&r);
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

        fn mark_suggest(&mut self, on: bool) {
            self.suggest = on;
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

    fn library_of(titles: &[&str]) -> Arc<Library> {
        Arc::new(Library::new(
            titles
                .iter()
                .enumerate()
                .map(|(i, t)| archive(i as i64 + 1, t, Some("SNES")))
                .collect(),
        ))
    }

    /// Frames until the background title index has answered.
    fn await_suggestions(rig: &mut Rig) {
        for _ in 0..200 {
            rig.frame();
            if !rig.app.pad_shell.as_ref().unwrap().suggestions().is_empty() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("suggestions never arrived");
    }

    #[test]
    fn search_fields_offer_title_suggestions_that_a_picks_and_done_applies() {
        let mut rig = Rig::new("", false);
        rig.mark_suggest(true);
        rig.app.library = library_of(&["Alpha Quest", "Alien Run", "Beta Racer", "Gamma Blaster"]);
        rig.frame();
        rig.press(A);
        assert!(rig.osk_open());
        let shell = rig.app.pad_shell.as_mut().unwrap();
        shell.type_for_test("al");
        shell.focus_top_row_for_test();
        await_suggestions(&mut rig);
        let offered = rig.app.pad_shell.as_ref().unwrap().suggestions();
        assert_eq!(
            offered,
            vec!["Alien Run".to_owned(), "Alpha Quest".to_owned()]
        );
        assert_eq!(rig.text, "", "suggestions do not touch the field");
        rig.press(Control::Direction(Direction::Up));
        assert_eq!(
            rig.app.pad_shell.as_ref().unwrap().suggestion_focus(),
            Some(0)
        );
        rig.press(Control::Direction(Direction::Right));
        rig.press(A); // take "Alpha Quest"
        assert_eq!(rig.buffer().as_deref(), Some("Alpha Quest"));
        assert_eq!(rig.text, "", "still only staged");
        rig.press(START);
        assert_eq!(rig.text, "Alpha Quest", "Done applies the search");
    }

    #[test]
    fn plain_fields_and_secrets_get_no_suggestions_and_cancel_still_changes_nothing() {
        let mut rig = Rig::new("keep", false);
        rig.app.library = library_of(&["Keep Going"]);
        rig.frame();
        rig.press(A);
        rig.app.pad_shell.as_mut().unwrap().type_for_test("ke");
        for _ in 0..20 {
            rig.frame();
        }
        assert!(
            rig.app.pad_shell.as_ref().unwrap().suggestions().is_empty(),
            "unmarked field"
        );
        rig.press(B);
        assert_eq!(rig.text, "keep");

        let mut secret = Rig::new("hunter2", true);
        secret.mark_suggest(true);
        secret.app.library = library_of(&["Hunter Hunted"]);
        secret.frame();
        secret.press(A);
        secret.app.pad_shell.as_mut().unwrap().type_for_test("hun");
        for _ in 0..20 {
            secret.frame();
        }
        assert!(
            secret
                .app
                .pad_shell
                .as_ref()
                .unwrap()
                .suggestions()
                .is_empty(),
            "never for secrets"
        );
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
        page_rect: egui::Rect,
        dialog_open: bool,
        dialog_buttons: bool,
    }

    const A: Control = Control::Button(Button::South);
    const B: Control = Control::Button(Button::East);

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
                page_rect: egui::Rect::NOTHING,
                dialog_open: true,
                dialog_buttons: true,
            };
            rig.frame();
            rig
        }

        fn frame(&mut self) {
            self.frame_with(Vec::new());
        }

        fn frame_with(&mut self, events: Vec<egui::Event>) {
            self.time += 0.1;
            let (app, cancel, destroy, page, page_rect) = (
                &mut self.app,
                &mut self.cancel,
                &mut self.destroy,
                &mut self.page,
                &mut self.page_rect,
            );
            let (dialog_open, dialog_buttons) = (self.dialog_open, self.dialog_buttons);
            run_ui_frame_at(&self.context, Some(self.time), events, |context| {
                app.show(context);
                egui::Area::new(egui::Id::new("page_area"))
                    .order(egui::Order::Background)
                    .show(context, |ui| {
                        let r = ui.button("Delete page thing");
                        *page_rect = r.rect;
                        if r.clicked() {
                            *page += 1;
                        }
                    });
                if dialog_open {
                    egui::Window::new("Are you sure?")
                        .fixed_pos(egui::pos2(800.0, 400.0))
                        .show(context, |ui| {
                            if !dialog_buttons {
                                ui.label("Working… nothing to press here");
                                return;
                            }
                            if ui.button("Cancel").clicked() {
                                *cancel += 1;
                            }
                            if ui.button("Delete everything").clicked() {
                                *destroy += 1;
                            }
                        });
                }
            });
        }

        fn focus(&mut self, label: &str) {
            // Walk with the controller's own Down until the label is focused.
            for _ in 0..120 {
                self.app.pad_shell.as_mut().unwrap().reading_order = true;
                self.step(Control::Direction(Direction::Down));
                self.app.pad_shell.as_mut().unwrap().reading_order = false;
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

    fn focus_is_in_the_dialog(rig: &Rig) -> bool {
        let Some(id) = rig.context.memory(|m| m.focused()) else {
            return true; // nothing focused is not a leak
        };
        rig.context
            .read_response(id)
            .is_some_and(|r| r.layer_id.order == egui::Order::Middle)
    }

    #[test]
    fn controller_focus_cannot_leave_an_open_dialog_in_either_direction() {
        for direction in [Direction::Down, Direction::Up] {
            let mut rig = Rig::new();
            rig.step(Control::Direction(direction)); // controller takes over
            for _ in 0..30 {
                rig.step(Control::Direction(direction));
                assert!(focus_is_in_the_dialog(&rig), "focus leaked {direction:?}");
                rig.step(A); // a tap on whatever has focus
            }
            assert_eq!(rig.page, 0, "the page behind was never pressed");
            assert_eq!(rig.destroy, 0, "a tap never confirms");
        }
    }

    #[test]
    fn a_dialog_with_no_close_button_cannot_trap_the_controller() {
        let mut rig = Rig::new();
        rig.dialog_buttons = false; // a window with only text: nothing to focus
        rig.frame();
        rig.step(Control::Direction(Direction::Down));
        for _ in 0..6 {
            rig.step(Control::Direction(Direction::Down));
            assert!(focus_is_in_the_dialog(&rig), "locked in while it is open");
        }
        assert_eq!(rig.page, 0);
        rig.step(B); // Back: this window has no close rule
        rig.step(Control::Direction(Direction::Down));
        let mut reached_page = false;
        for _ in 0..30 {
            rig.step(Control::Direction(Direction::Down));
            reached_page |= !focus_is_in_the_dialog(&rig);
        }
        assert!(reached_page, "after B the controller can leave the window");
        assert!(rig.dialog_open, "the dialog itself was not touched");
    }

    #[test]
    fn mouse_use_behind_the_dialog_is_unchanged() {
        let mut rig = Rig::new();
        rig.step(Control::Direction(Direction::Down)); // controller mode
        // Mouse takes over: a move, then a click on the page button.
        let point = rig.page_rect.center();
        rig.frame_with(vec![egui::Event::PointerMoved(point)]);
        rig.frame();
        for pressed in [true, false] {
            rig.frame_with(vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert_eq!(rig.page, 1, "mouse can still use the page behind a dialog");
    }

    #[test]
    fn holding_a_while_focus_moves_never_activates_another_control() {
        let mut rig = Rig::new();
        rig.focus("Delete everything");
        rig.hold_a(2); // hold begun on the destructive button
        rig.step(Control::Direction(Direction::Up)); // focus moves, A still down
        rig.step(Control::Direction(Direction::Down));
        for _ in 0..10 {
            rig.frame(); // A is still held well past 0.6 s
        }
        rig.release_a();
        assert_eq!(rig.destroy, 0);
        assert_eq!(rig.cancel, 0);
        assert_eq!(rig.page, 0);
    }

    #[test]
    fn buttons_on_the_page_itself_are_not_slowed_down() {
        // With no dialog open the page is ordinary: a tap presses its button.
        let mut rig = Rig::new();
        rig.dialog_open = false;
        rig.frame();
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
                // Scan in reading order: egui names a control only then.
                self.app.pad_shell.as_mut().unwrap().reading_order = true;
                self.tap(Control::Direction(Direction::Down));
                self.app.pad_shell.as_mut().unwrap().reading_order = false;
                if let Some(l) = self.label() {
                    self.seen.push(l);
                }
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
        let mut pad = Pad {
            context,
            app,
            seen: Vec::new(),
        };
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

/// Spatial movement on Blue's real Library: sidebar on the left, a grid of
/// game cards on the right.
mod spatial {
    use super::super::super::simple_shell::nav_id;
    use super::*;

    struct Lib {
        context: egui::Context,
        app: App,
    }

    const LEFT: Control = Control::Direction(Direction::Left);
    const RIGHT: Control = Control::Direction(Direction::Right);
    const DOWN: Control = Control::Direction(Direction::Down);
    const UP: Control = Control::Direction(Direction::Up);

    impl Lib {
        fn new(games: i64) -> Self {
            Self::with_titles((1..=games).map(|id| format!("Game {id:03}")).collect())
        }

        fn with_titles(titles: Vec<String>) -> Self {
            let context = egui::Context::default();
            let mut app = fixture(&context);
            app.simple.enabled = true;
            app.library = Arc::new(Library::new(
                titles
                    .iter()
                    .enumerate()
                    .map(|(i, t)| archive(i as i64 + 1, t, Some("SNES")))
                    .collect(),
            ));
            app.artwork.index = Some(Arc::new(MediaIndex::default()));
            app.loaded = true;
            app.router.current = Route::BrowsePlay;
            let mut lib = Self { context, app };
            lib.frames();
            lib.frames();
            lib
        }
        fn frames(&mut self) {
            for _ in 0..3 {
                let app = &mut self.app;
                run_ui_frame(&self.context, Vec::new(), |c| app.show(c));
            }
        }
        fn tap(&mut self, control: Control) {
            let shell = self.app.pad_shell.as_mut().unwrap();
            shell.inject(PadEvent::Pressed { device: 1, control });
            shell.inject(PadEvent::Released { device: 1, control });
            self.frames();
        }
        fn focused(&self) -> Option<egui::Id> {
            self.context.memory(|m| m.focused())
        }
        fn rect(&self, id: egui::Id) -> egui::Rect {
            self.context.read_response(id).expect("drawn").rect
        }
        fn on_sidebar(&self) -> bool {
            self.focused().is_some_and(|id| {
                self.rect(id).right() <= self.rect(nav_id(Destination::Library)).right() + 1.0
            })
        }
        fn focus_sidebar(&mut self) {
            let id = nav_id(Destination::Library);
            self.context.memory_mut(|m| m.request_focus(id));
            self.frames();
            assert!(self.on_sidebar());
            // The controller takes over from here.
            self.app.pad_shell.as_mut().unwrap().set_pad_mode_for_test();
        }
        /// A game card is far taller than any chip or button.
        fn is_card(&self, id: egui::Id) -> bool {
            self.rect(id).height() > 100.0
        }
        /// Walk Down from the sidebar's Right until a card has focus.
        fn reach_a_card(&mut self) {
            self.tap(RIGHT);
            for _ in 0..40 {
                if self
                    .focused()
                    .is_some_and(|id| !self.on_sidebar() && self.is_card(id))
                {
                    return;
                }
                self.tap(DOWN);
            }
            panic!("never reached a game card");
        }
    }

    #[test]
    fn right_from_the_sidebar_lands_in_the_content_level_with_the_row_you_left() {
        let mut lib = Lib::new(40);
        lib.focus_sidebar();
        let from = lib.rect(lib.focused().unwrap());
        lib.tap(RIGHT);
        let id = lib.focused().expect("something has focus");
        assert!(!lib.on_sidebar(), "Right left the sidebar");
        let to = lib.rect(id);
        assert!(
            to.left() > from.right() - 1.0,
            "it is to the right of the sidebar"
        );
        assert!(
            (to.center().y - from.center().y).abs() < from.height() * 2.5,
            "landed level with the sidebar row, not at the top of the page: {from:?} → {to:?}"
        );
    }

    #[test]
    fn grid_moves_by_row_and_column() {
        let mut lib = Lib::new(40);
        lib.focus_sidebar();
        lib.reach_a_card();
        let first = lib.rect(lib.focused().unwrap());
        lib.tap(RIGHT);
        let right_id = lib.focused().unwrap();
        let right = lib.rect(right_id);
        assert!(right.left() > first.left() && (right.center().y - first.center().y).abs() < 10.0);
        lib.tap(DOWN);
        let below = lib.rect(lib.focused().unwrap());
        assert!(below.top() > right.top(), "Down goes to the next row");
        assert!(
            (below.center().x - right.center().x).abs() < right.width() / 2.0,
            "same column"
        );
        lib.tap(UP);
        assert_eq!(lib.focused(), Some(right_id), "Up returns to the same card");
    }

    #[test]
    fn returning_from_the_sidebar_restores_the_card_without_scrolling() {
        let mut lib = Lib::new(60);
        lib.focus_sidebar();
        lib.reach_a_card();
        for _ in 0..6 {
            lib.tap(DOWN); // scroll well down the library
        }
        // Walk Left to the sidebar; the last content control passed is the
        // one we are leaving from.
        let mut leaving = lib.focused().unwrap();
        for _ in 0..12 {
            lib.tap(LEFT);
            if lib.on_sidebar() {
                break;
            }
            leaving = lib.focused().unwrap();
        }
        assert!(lib.on_sidebar(), "Left ends in the sidebar");
        let before = lib.rect(leaving);
        assert!(lib.is_card(leaving), "we left from a game card");
        lib.tap(RIGHT);
        assert_eq!(
            lib.focused(),
            Some(leaving),
            "the same card is focused again"
        );
        // Returning reveals the card where it is; it does not jump to the top
        // of the page (which would put a row-6 card far below the screen).
        let now = lib.rect(leaving);
        assert!(
            now.bottom() <= SIZE[1] + 1.0 && now.top() >= 0.0,
            "card on screen: {now:?}"
        );
        assert!(
            now.top() > 300.0,
            "still deep in the library, not at the top: {now:?} (was {before:?})"
        );
    }

    /// Regression: the shelf scrolled to a focused card only for Tab, not for a
    /// controller move, so on a real-sized library Down left the focused card
    /// below the screen and the page did not follow.
    #[test]
    fn moving_down_a_long_grid_keeps_the_focused_card_on_screen() {
        let mut lib = Lib::new(300);
        lib.focus_sidebar();
        lib.reach_a_card();
        let mut deepest = 0.0_f32;
        for step in 0..25 {
            lib.tap(DOWN);
            lib.frames(); // the page follows the focus a frame after it moves
            let id = lib.focused().expect("focus stays on a control");
            let rect = lib.rect(id);
            deepest = deepest.max(rect.top());
            assert!(
                rect.top() >= 0.0 && rect.bottom() <= SIZE[1] + 1.0,
                "step {step}: the focused card is off screen: {rect:?}"
            );
        }
        // And it really travelled down the library (the page followed).
        let id = lib.focused().unwrap();
        assert!(lib.is_card(id));
        for _ in 0..25 {
            lib.tap(UP);
            lib.frames();
            let rect = lib.rect(lib.focused().unwrap());
            assert!(
                rect.top() >= 0.0 && rect.bottom() <= SIZE[1] + 1.0,
                "moving back up: off screen {rect:?}"
            );
        }
    }

    #[test]
    fn a_remembered_card_that_is_gone_falls_back_safely() {
        let mut lib = Lib::new(40);
        lib.focus_sidebar();
        lib.reach_a_card();
        // The library changes under us: the card list is replaced by fewer games.
        lib.app.library = Arc::new(Library::new(
            (1..=3)
                .map(|id| archive(100 + id, &format!("New {id}"), Some("SNES")))
                .collect(),
        ));
        lib.frames();
        lib.focus_sidebar();
        lib.tap(RIGHT);
        assert!(lib.focused().is_some(), "focus is not lost");
        assert!(!lib.on_sidebar(), "and it moved into the content");
    }

    fn lettered() -> Vec<String> {
        let mut titles = Vec::new();
        for letter in ["A", "B", "C", "D", "M", "N", "Z"] {
            for n in 1..=9 {
                titles.push(format!("{letter}rcade Title {n:02}"));
            }
        }
        for n in 0..4 {
            titles.push(format!("19{n}3 Classic"));
        }
        titles
    }

    impl Lib {
        /// Move focus (reading order, whose controls egui names) to the A-Z
        /// button with this label.
        fn focus_letter(&mut self, letter: &str) {
            self.app.pad_shell.as_mut().unwrap().reading_order = true;
            for _ in 0..400 {
                self.tap(DOWN);
                let label = self
                    .context
                    .data(|d| {
                        d.get_temp::<(egui::Id, String, bool)>(egui::Id::new(
                            "pad_shell_focus_label",
                        ))
                    })
                    .map(|x| x.1);
                if label.as_deref() == Some(letter) {
                    self.app.pad_shell.as_mut().unwrap().reading_order = false;
                    return;
                }
            }
            panic!("the controller never reached the {letter} button");
        }

        fn focused_title(&self) -> Option<String> {
            // A card's name is its game title.
            self.context
                .data(|d| {
                    d.get_temp::<(egui::Id, String, bool)>(egui::Id::new("pad_shell_focus_label"))
                })
                .map(|x| x.1)
        }
    }

    #[test]
    fn a_to_z_jumps_to_the_first_game_of_a_letter_and_focuses_it() {
        let mut lib = Lib::with_titles(lettered());
        lib.focus_sidebar();
        lib.focus_letter("N");
        lib.tap(Control::Button(Button::South));
        let id = lib.focused().expect("a game has focus after the jump");
        assert!(lib.is_card(id), "focus moved onto a game card");
        let rect = lib.rect(id);
        assert!(
            rect.top() >= 0.0 && rect.bottom() <= SIZE[1] + 1.0,
            "the jumped-to row is on screen: {rect:?}"
        );
        // Reading order from here is the games from N onward: the card named
        // for the first N title is the one focused.
        lib.app.pad_shell.as_mut().unwrap().reading_order = true;
        let title = lib.focused_title().unwrap_or_default();
        assert_eq!(title, "Nrcade Title 01", "first N game, not a later one");
    }

    #[test]
    fn letters_with_no_games_are_skipped_and_numbers_have_a_hash_button() {
        let mut lib = Lib::with_titles(lettered());
        lib.focus_sidebar();
        lib.app.pad_shell.as_mut().unwrap().reading_order = true;
        let mut seen = Vec::new();
        for _ in 0..40 {
            lib.tap(DOWN);
            if let Some(l) = lib.focused_title() {
                seen.push(l);
            }
        }
        for present in ["#", "A", "B", "C", "D", "M", "N", "Z"] {
            assert!(
                seen.iter().any(|l| l == present),
                "{present} missing from {seen:?}"
            );
        }
        for absent in ["Q", "X", "K"] {
            assert!(
                !seen.iter().any(|l| l == absent),
                "{absent} has no games, so it is not a stop"
            );
        }
    }

    #[test]
    fn the_hash_button_reaches_titles_that_start_with_a_number() {
        let mut lib = Lib::with_titles(lettered());
        lib.focus_sidebar();
        lib.focus_letter("#");
        lib.tap(Control::Button(Button::South));
        lib.app.pad_shell.as_mut().unwrap().reading_order = true;
        assert!(lib.focused_title().unwrap_or_default().starts_with("19"));
    }

    #[test]
    fn the_strip_follows_search_and_a_jump_can_be_left_and_returned_to() {
        let mut lib = Lib::with_titles(lettered());
        lib.app.filter.search = "rcade title 0".to_owned();
        lib.frames();
        lib.frames();
        lib.focus_sidebar();
        lib.app.pad_shell.as_mut().unwrap().reading_order = true;
        let mut seen = Vec::new();
        for _ in 0..40 {
            lib.tap(DOWN);
            if let Some(l) = lib.focused_title() {
                seen.push(l);
            }
        }
        assert!(
            !seen.iter().any(|l| l == "#"),
            "no numbered title matches: {seen:?}"
        );
        assert!(seen.iter().any(|l| l == "Z"));
        lib.app.pad_shell.as_mut().unwrap().reading_order = false;
        lib.focus_letter("Z");
        lib.tap(Control::Button(Button::South));
        let mut leaving = lib.focused().unwrap();
        assert!(lib.is_card(leaving), "the jump landed on a game");
        for _ in 0..8 {
            lib.tap(LEFT);
            if lib.on_sidebar() {
                break;
            }
            leaving = lib.focused().unwrap();
        }
        assert!(lib.on_sidebar());
        lib.tap(RIGHT);
        assert_eq!(
            lib.focused(),
            Some(leaving),
            "Right returns to where the jump left off"
        );
    }

    #[test]
    fn short_libraries_do_not_get_the_strip() {
        let mut lib = Lib::with_titles((1..=10).map(|n| format!("Game {n:02}")).collect());
        lib.focus_sidebar();
        lib.app.pad_shell.as_mut().unwrap().reading_order = true;
        for _ in 0..30 {
            lib.tap(DOWN);
            assert_ne!(lib.focused_title().as_deref(), Some("M"));
        }
    }
}

/// The R3 virtual mouse, through real frames.
mod virtual_mouse {
    use super::*;

    struct Rig {
        context: egui::Context,
        app: App,
        time: f64,
        left_clicks: u32,
        right_clicks: u32,
        focused_activations: u32,
    }

    const R3: Control = Control::Button(Button::RightStick);
    const A: Control = Control::Button(Button::South);
    const B: Control = Control::Button(Button::East);
    const X: Control = Control::Button(Button::West);

    fn centre() -> egui::Pos2 {
        egui::pos2(SIZE[0] / 2.0, SIZE[1] / 2.0)
    }

    impl Rig {
        fn new() -> Self {
            let context = egui::Context::default();
            let app = super::app(&context);
            let mut rig = Self {
                context,
                app,
                time: 1.0,
                left_clicks: 0,
                right_clicks: 0,
                focused_activations: 0,
            };
            rig.frame(true);
            let target = rig
                .context
                .data(|d| d.get_temp::<egui::Id>(egui::Id::new("mouse_target_id")))
                .expect("target drawn");
            rig.context.memory_mut(|m| m.request_focus(target));
            rig.frame(true);
            // The controller is in use from here on.
            rig.app.pad_shell.as_mut().unwrap().set_pad_mode_for_test();
            rig
        }

        fn frame(&mut self, focused: bool) {
            self.time += 0.016;
            let (app, left, right, activated) = (
                &mut self.app,
                &mut self.left_clicks,
                &mut self.right_clicks,
                &mut self.focused_activations,
            );
            let out = self.context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE.into())),
                    time: Some(self.time),
                    focused,
                    ..Default::default()
                },
                |ui| {
                    let context = ui.ctx().clone();
                    app.show(&context);
                    egui::Area::new(egui::Id::new("under_pointer"))
                        .order(egui::Order::Background)
                        .fixed_pos(centre() - egui::vec2(50.0, 20.0))
                        .show(&context, |ui| {
                            let r =
                                ui.add_sized([100.0, 40.0], egui::Button::new("Under the pointer"));
                            if r.clicked() {
                                *left += 1;
                            }
                            if r.secondary_clicked() {
                                *right += 1;
                            }
                        });
                    egui::Area::new(egui::Id::new("target"))
                        .order(egui::Order::Background)
                        .fixed_pos(egui::pos2(40.0, 700.0))
                        .show(&context, |ui| {
                            let r = ui.button("Focused elsewhere");
                            ui.data_mut(|d| d.insert_temp(egui::Id::new("mouse_target_id"), r.id));
                            if r.clicked() {
                                *activated += 1;
                            }
                        });
                },
            );
            let _ = out;
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame(true);
            }
        }

        fn send(&mut self, event: PadEvent) {
            self.app.pad_shell.as_mut().unwrap().inject(event);
        }

        fn tap(&mut self, control: Control) {
            self.send(PadEvent::Pressed { device: 1, control });
            self.frames(2);
            self.send(PadEvent::Released { device: 1, control });
            self.frames(3);
        }

        fn stick(&mut self, x: f32, y: f32) {
            self.send(PadEvent::RightStick { device: 1, x, y });
        }

        fn pointer(&self) -> Option<egui::Pos2> {
            self.context.pointer_latest_pos()
        }

        fn mouse_on(&self) -> bool {
            self.app.pad_shell.as_ref().unwrap().mouse_on()
        }
    }

    #[test]
    fn r3_toggles_a_visible_pointer_and_b_exits() {
        let mut rig = Rig::new();
        assert!(!rig.mouse_on());
        rig.tap(R3);
        assert!(rig.mouse_on());
        rig.tap(B);
        assert!(!rig.mouse_on(), "B leaves mouse mode");
        rig.tap(R3);
        rig.tap(R3);
        assert!(!rig.mouse_on(), "R3 toggles");
    }

    #[test]
    fn the_right_stick_moves_the_pointer_smoothly_and_respects_the_dead_zone() {
        let mut rig = Rig::new();
        rig.tap(R3);
        let start = rig.pointer().expect("pointer placed");
        rig.stick(0.1, 0.1); // inside the dead zone
        rig.frames(20);
        assert_eq!(rig.pointer(), Some(start), "drift does not move it");
        rig.stick(0.5, 0.0);
        rig.frames(30);
        let half = rig.pointer().unwrap();
        rig.stick(0.0, 0.0);
        rig.frames(3);
        let settled = rig.pointer().unwrap();
        rig.frames(10);
        assert_eq!(rig.pointer(), Some(settled), "stops when the stick rests");
        rig.stick(1.0, 0.0);
        rig.frames(30);
        let full = rig.pointer().unwrap();
        assert!(half.x > start.x + 5.0);
        assert!(
            (full.x - settled.x) > (half.x - start.x) * 3.0,
            "a full push is much faster than half: {start:?} {half:?} {full:?}"
        );
    }

    #[test]
    fn a_and_x_click_under_the_pointer_and_never_also_press_the_focused_control() {
        let mut rig = Rig::new();
        rig.tap(R3);
        assert_eq!(rig.pointer(), Some(centre()));
        rig.tap(A);
        assert_eq!(rig.left_clicks, 1, "A is a left click");
        rig.tap(X);
        assert_eq!(rig.right_clicks, 1, "X is a right click");
        assert_eq!(
            rig.focused_activations, 0,
            "the clicks were consumed, so the focused control was not pressed"
        );
        assert_eq!(
            rig.app.router.current,
            Route::Home,
            "and no Search/section command ran"
        );
    }

    #[test]
    fn with_mouse_mode_off_the_stick_does_nothing_and_a_still_presses_the_focus() {
        let mut rig = Rig::new();
        rig.stick(1.0, 1.0);
        rig.frames(20);
        assert_eq!(rig.left_clicks, 0);
        rig.tap(A);
        assert_eq!(
            rig.focused_activations, 1,
            "normal controller A is unchanged"
        );
        rig.tap(R3);
        rig.tap(B);
        rig.tap(A);
        assert_eq!(
            rig.focused_activations, 2,
            "and after leaving mouse mode too"
        );
    }

    #[test]
    fn a_held_click_is_released_when_the_mode_ends_and_a_press_is_never_lost() {
        let mut rig = Rig::new();
        rig.tap(R3);
        rig.send(PadEvent::Pressed {
            device: 1,
            control: A,
        });
        rig.frames(3); // A held down
        rig.tap(R3); // mode off while A is still held
        rig.send(PadEvent::Released {
            device: 1,
            control: A,
        });
        rig.frames(3);
        // The click focused the button under the pointer; put focus back, as the
        // person would, then the next A works as a normal controller A.
        let target = rig
            .context
            .data(|d| d.get_temp::<egui::Id>(egui::Id::new("mouse_target_id")))
            .unwrap();
        rig.context.memory_mut(|m| m.request_focus(target));
        rig.frames(2);
        rig.tap(A);
        assert_eq!(rig.focused_activations, 1);
    }

    #[test]
    fn leaving_the_window_leaves_the_mode() {
        let mut rig = Rig::new();
        rig.tap(R3);
        assert!(rig.mouse_on());
        rig.frame(false); // an emulator took focus
        assert!(!rig.mouse_on());
    }
}
