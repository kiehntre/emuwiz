//! Big Picture: a full-screen, controller-friendly way to browse the library.
//!
//! This is a presentation over what already exists. It owns no catalogue,
//! filter, artwork, readiness or launch logic:
//!
//! * the games and their order come from the Library's own filtered list, so
//!   search and platform filters use the same asynchronous worker;
//! * covers come from the same bounded artwork pipeline, and only the tiles on
//!   screen are drawn, so the size of the library does not change a frame;
//! * Play opens the unchanged Launch page. An emulator starts only when its
//!   Launch button is activated there, with every existing check and refusal.
//!
//! Controller input is ignored while an emulator runs or the window is not
//! focused, because evdev delivers it to every reader regardless of focus.

mod gamepad;
pub(super) mod nav;

use super::{
    App,
    browse_play::{PRESERVATION_ONLY_SENTENCE, thousands},
    launch_readiness_summary::{self as readiness, Attempt, CompactReadiness, ReadinessAction},
    library::{Game, Library, media_kind_label},
    media_sources::Kind,
    routes::Route,
};
use crate::ui::theme::blue;
use eframe::egui::{self, Color32, RichText};
use gamepad::PadStatus;
pub(super) use gamepad::Reader;
#[cfg(test)]
pub(super) use gamepad::{codes, decode_raw};
use nav::{
    Button, Command, Control, Direction, Grid, Navigator, PadEvent, SEARCH_COLUMNS, SearchKey,
    search_keys,
};
use std::sync::Arc;

const MARGIN: f32 = 24.0;
const GAP: f32 = 16.0;
const TILE_MIN_WIDTH: f32 = 230.0;
const TILE_MAX_WIDTH: f32 = 340.0;
const TILE_PAD: f32 = 10.0;
const TITLE_SIZE: f32 = 22.0;
const META_SIZE: f32 = 17.0;
const TILE_TEXT_HEIGHT: f32 = 92.0;
const MIN_COVER_HEIGHT: f32 = 90.0;
const COVER_RATIO: f32 = 1.05;
const COVER_MIN_RATIO: f32 = 0.7;
const FOCUS_WIDTH: f32 = 5.0;
const PLATFORM_ROW_HEIGHT: f32 = 52.0;
/// Launch page text is scaled so it can be read from across a room.
const PAGE_TEXT_SCALE: f32 = 1.3;
/// After an emulator exits, input waits this long: the press that closed the
/// game must not also act on the browser.
const RESUME_GRACE_SECS: f64 = 0.75;
/// How long the first Back at the top level waits for a second.
const LEAVE_CONFIRM_SECS: f64 = 4.0;
/// Two controller X presses this close together on the same game are Play.
const DOUBLE_X_SECS: f64 = 0.35;

/// What a controller X press does.
#[derive(Debug, PartialEq)]
enum XPress {
    /// Not on a game: X keeps its old meaning.
    Pass(Command),
    /// The first tap: nothing happens yet.
    Armed,
    /// The second tap in time on the same game: Play it.
    Play,
}

/// The first X of a possible double tap: where it was pressed, and when.
#[derive(Clone, Copy, Debug, PartialEq)]
struct XTap {
    step: Step,
    game: i64,
    at: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Step {
    Browse,
    Game(i64),
    /// The existing Launch page for this game.
    Launch(i64),
}

#[derive(Debug, Eq, PartialEq)]
enum Overlay {
    Search(Grid),
    Platforms {
        grid: Grid,
        /// The system name, empty for all, and its game count.
        rows: Vec<(String, usize)>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GameAction {
    Play,
    Fix(ReadinessAction),
    Details,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tone {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Notice {
    text: String,
    tone: Tone,
}

impl Notice {
    fn new(tone: Tone, text: impl Into<String>) -> Self {
        Self {
            tone,
            text: text.into(),
        }
    }
}

pub(super) struct BigPicture {
    step: Step,
    overlay: Option<Overlay>,
    grid: Grid,
    /// The focused game: survives filtering and list rebuilds.
    focused: Option<i64>,
    /// The list `grid` indexes; held so a new list is noticed by identity.
    shown: Arc<Vec<usize>>,
    columns: usize,
    rows: usize,
    wheel: f32,
    nav: Navigator,
    reader: Option<Reader>,
    pad_status: PadStatus,
    /// Footer and tile clicks, applied at the start of the next frame.
    clicked: Vec<Command>,
    /// What the Game step offers, in order, from its last drawn readiness.
    actions: Vec<GameAction>,
    action: usize,
    play_refusal: Option<String>,
    notice: Option<Notice>,
    leave_armed_until: Option<f64>,
    /// The last controller X, while a second one would still be a double tap.
    x_tap: Option<XTap>,
    fullscreen: bool,
    /// The game Launch was opened for while an emulator is running.
    running: Option<Option<i64>>,
    resume_at: f64,
    #[cfg(test)]
    injected: Vec<PadEvent>,
    #[cfg(test)]
    launch_active: bool,
}

impl BigPicture {
    fn new(reader: Option<Reader>, focused: Option<i64>) -> Self {
        let pad_status = reader
            .as_ref()
            .map_or(PadStatus::Unsupported, Reader::status);
        // Without a controller this is a window on a desktop, not a sofa:
        // stay out of the way until asked (F11).
        let fullscreen = pad_status.connected();
        Self {
            step: Step::Browse,
            overlay: None,
            grid: Grid::default(),
            focused,
            shown: Arc::default(),
            columns: 1,
            rows: 1,
            wheel: 0.0,
            nav: Navigator::default(),
            reader,
            pad_status,
            clicked: Vec::new(),
            actions: Vec::new(),
            action: 0,
            play_refusal: None,
            notice: None,
            leave_armed_until: None,
            x_tap: None,
            fullscreen,
            running: None,
            resume_at: 0.0,
            #[cfg(test)]
            injected: Vec::new(),
            #[cfg(test)]
            launch_active: false,
        }
    }

    fn pad_events(&mut self) -> Vec<PadEvent> {
        #[allow(unused_mut)]
        let mut events: Vec<PadEvent> = self
            .reader
            .as_ref()
            .map(|reader| reader.drain().collect())
            .unwrap_or_default();
        #[cfg(test)]
        events.append(&mut self.injected);
        events
    }

    /// `open`: whether controller input may act now. The flag marks a fresh
    /// controller X press (the navigator already drops held repeats).
    fn pad_commands(&mut self, now: f64, open: bool) -> Vec<(Command, bool)> {
        let mut commands = Vec::new();
        for event in self.pad_events() {
            let x = matches!(
                event,
                PadEvent::Pressed {
                    control: Control::Button(Button::West),
                    ..
                }
            );
            if let Some(command) = self.nav.feed(event, now, open) {
                commands.push((command, x && command == Command::Search));
            }
        }
        commands.extend(self.nav.tick(now, open).map(|command| (command, false)));
        commands
    }

    /// The game an X press is on: a focused tile, or the open game, with no
    /// overlay over it. Anywhere else X is never the start of a double tap.
    fn x_target(&self) -> Option<(Step, i64)> {
        if self.overlay.is_some() {
            return None;
        }
        match self.step {
            Step::Browse => self.focused_position().and(self.focused),
            Step::Game(id) => Some(id),
            Step::Launch(_) => None,
        }
        .map(|game| (self.step, game))
    }

    /// #437: X on a game arms a double tap and does nothing else; a second X
    /// on the same game in time is Play. Any other command in between has
    /// already cleared the first tap. Off a game (Search open, the Launch
    /// page) X keeps its old meaning.
    fn x_press(&mut self, tap: Option<XTap>, command: Command, now: f64) -> XPress {
        let Some((step, game)) = self.x_target() else {
            return XPress::Pass(command);
        };
        // Like any other input, X disarms a pending "Back again to leave".
        if self.leave_armed_until.take().is_some() {
            self.notice = None;
        }
        if tap.is_some_and(|tap| {
            tap.step == step && tap.game == game && now - tap.at <= DOUBLE_X_SECS
        }) {
            return XPress::Play;
        }
        self.x_tap = Some(XTap {
            step,
            game,
            at: now,
        });
        XPress::Armed
    }

    fn focused_position(&self) -> Option<usize> {
        (!self.shown.is_empty()).then(|| self.grid.focus.min(self.shown.len() - 1))
    }

    /// Follow the Library's current list, keeping the same game focused when
    /// it is still there. Only a new list costs a search of it.
    fn sync_list(&mut self, library: &Library, indices: &Arc<Vec<usize>>) {
        if !Arc::ptr_eq(&self.shown, indices) {
            self.shown = indices.clone();
            if indices.is_empty() {
                return;
            }
            let id = |index: &usize| library.games.get(*index).map(|game| game.archive.id);
            match self
                .focused
                .and_then(|focused| indices.iter().position(|index| id(index) == Some(focused)))
            {
                Some(position) => self.grid.focus = position,
                None => self.grid = Grid::default(),
            }
        }
        self.grid.reveal(self.shown.len(), self.columns, self.rows);
        self.focused = self
            .focused_position()
            .and_then(|position| library.games.get(self.shown[position]))
            .map(|game| game.archive.id)
            .or(self.focused);
    }

    fn enter_step(&mut self, step: Step) {
        self.step = step;
        self.action = 0;
        self.actions.clear();
        self.play_refusal = None;
    }
}

/// The controller on an ordinary page: it moves real keyboard focus, and Select
/// is egui's own Enter on the focused control, so nothing bypasses the page.
pub(super) fn page_command(context: &egui::Context, command: Command) {
    match command {
        Command::Move(Direction::Up | Direction::Left) => {
            context.memory_mut(|m| m.move_focus(egui::FocusDirection::Previous));
        }
        Command::Move(Direction::Down | Direction::Right) => {
            context.memory_mut(|m| m.move_focus(egui::FocusDirection::Next));
        }
        Command::Select => context.input_mut(|input| {
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: Some(egui::Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }),
        _ => {}
    }
}

/// Why the mode ended and where the app should be left.
#[derive(Debug, Eq, PartialEq)]
enum Next {
    Stay,
    Leave,
}

/// The sentence the browser shows after an emulator session ends.
fn session_notice(title: &str, attempt: Option<&Attempt>) -> Notice {
    match attempt {
        Some(Attempt::CouldNotStart { technical, .. }) => {
            Notice::new(Tone::Error, format!("{title} could not start. {technical}"))
        }
        Some(Attempt::EndedQuickly { seconds }) => Notice::new(
            Tone::Warning,
            format!(
                "{title} closed after {seconds} seconds. Open it again to see what the launch check says."
            ),
        ),
        None => Notice::new(Tone::Info, format!("{title} has closed.")),
    }
}

/// The one word a tile needs when a game cannot simply be played.
fn tile_badge(game: &Game) -> Option<(&'static str, Color32)> {
    if game.is_preservation_only() {
        Some(("Not playable", blue::SECONDARY_TEXT))
    } else if game.archive.last_verified_missing_at.is_some()
        || game.archive.last_known_health == "missing"
    {
        Some(("Missing file", blue::BAD_TEXT))
    } else if game.attention {
        Some(("Needs attention", blue::WARN))
    } else {
        None
    }
}

fn game_actions(view: &CompactReadiness) -> Vec<GameAction> {
    let fix = view.fix.map(GameAction::Fix);
    let mut actions = Vec::with_capacity(3);
    if !view.pending {
        actions.extend(fix);
    }
    if view.play_enabled {
        actions.push(GameAction::Play);
    }
    if view.pending {
        actions.extend(fix);
    }
    actions.push(GameAction::Details);
    actions
}

fn action_label(action: GameAction) -> &'static str {
    match action {
        GameAction::Play => "Play",
        GameAction::Fix(fix) => readiness::action_label(fix),
        GameAction::Details => "Game details",
    }
}

fn keyboard_commands(
    context: &egui::Context,
    step: Step,
    overlay: Option<&Overlay>,
) -> Vec<Command> {
    use egui::{Key, Modifiers};
    let none = Modifiers::NONE;
    let mut commands = Vec::new();
    context.input_mut(|input| {
        if press(input, Key::Escape) {
            commands.push(Command::Back);
        }
        // Launch keeps ordinary keyboard use: Tab, arrows, Enter and Space.
        if matches!(step, Step::Launch(_)) && overlay.is_none() {
            return;
        }
        for (key, command) in [
            (Key::ArrowUp, Command::Move(Direction::Up)),
            (Key::ArrowDown, Command::Move(Direction::Down)),
            (Key::ArrowLeft, Command::Move(Direction::Left)),
            (Key::ArrowRight, Command::Move(Direction::Right)),
            (Key::PageUp, Command::PageUp),
            (Key::PageDown, Command::PageDown),
            (Key::Home, Command::First),
            (Key::End, Command::Last),
        ] {
            if input.consume_key(none, key) {
                commands.push(command);
            }
        }
        if matches!(overlay, Some(Overlay::Search(_))) {
            // Typing goes to the search; Enter finishes it. Handled in `type_search`.
            return;
        }
        if press(input, Key::Enter) {
            commands.push(Command::Select);
        }
        if press(input, Key::Backspace) {
            commands.push(Command::Back);
        }
        for (key, command) in [
            (Key::L, Command::Launch),
            (Key::Slash, Command::Search),
            (Key::S, Command::Search),
            (Key::F, Command::Platforms),
        ] {
            if press(input, key) {
                commands.push(command);
            }
        }
        if commands.contains(&Command::Search) {
            // The letter that opened the search arrives as text in the same
            // frame; it must not become the first character searched for.
            input
                .events
                .retain(|event| !matches!(event, egui::Event::Text(_)));
        }
    });
    commands
}

/// Consumes every press of `key` and says whether one was a new press. Only
/// movement repeats while held, as on the controller: an autorepeating Esc or
/// Enter (for example the key that just closed an emulator) does not act.
fn press(input: &mut egui::InputState, key: egui::Key) -> bool {
    let mut fresh = false;
    input.events.retain(|event| match event {
        egui::Event::Key {
            key: pressed_key,
            modifiers,
            pressed: true,
            repeat,
            ..
        } if *pressed_key == key && modifiers.matches_logically(egui::Modifiers::NONE) => {
            fresh |= !repeat;
            false
        }
        _ => true,
    });
    fresh
}

/// What the keyboard typed into an open search.
fn type_search(context: &egui::Context, search: &mut String) -> bool {
    context.input_mut(|input| {
        for event in &input.events {
            if let egui::Event::Text(text) = event {
                search.push_str(text);
            }
        }
        if input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace) {
            search.pop();
        }
        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
    })
}

impl App {
    /// Open Big Picture over the current Library filter.
    pub(super) fn enter_big_picture(&mut self, context: &egui::Context) {
        if self.big_picture.is_some() {
            return;
        }
        let selected = match self.router.current {
            Route::BrowsePlayGame(id) => Some(id),
            _ => None,
        };
        // Tests drive the pure input path and never open a real device.
        let reader = (!cfg!(test)).then(|| Reader::start(context.clone()));
        let state = BigPicture::new(reader, selected);
        if state.fullscreen {
            context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        self.big_picture = Some(Box::new(state));
    }

    fn leave_big_picture(context: &egui::Context, state: &BigPicture) {
        if state.fullscreen {
            context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    fn big_picture_launch_active(&self, state: &BigPicture) -> bool {
        #[cfg(test)]
        if state.launch_active {
            return true;
        }
        let _ = state;
        self.native_workflows
            .as_ref()
            .is_some_and(|workflows| workflows.launch_in_progress())
    }

    /// Stays out of the emulator's way, then comes back to the same game.
    fn observe_emulator(
        &mut self,
        state: &mut BigPicture,
        context: &egui::Context,
        active: bool,
        now: f64,
    ) {
        if active && state.running.is_none() {
            let game = match state.step {
                Step::Launch(id) => Some(id),
                _ => None,
            };
            state.running = Some(game);
            state.notice = None;
            if state.fullscreen {
                // A full-screen window can hide the emulator's own window.
                context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        } else if !active && let Some(game) = state.running.take() {
            state.resume_at = now + RESUME_GRACE_SECS;
            if state.fullscreen {
                context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            }
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            context.request_repaint_after(std::time::Duration::from_secs_f64(RESUME_GRACE_SECS));
            if let Some(id) = game {
                let title = self
                    .library
                    .game(id)
                    .map_or("The game".to_string(), |game| game.title.clone());
                let attempt = self
                    .native_workflows
                    .as_ref()
                    .and_then(|workflows| workflows.launch_attempt(id));
                state.notice = Some(session_notice(&title, attempt));
                state.focused = Some(id);
                state.shown = Arc::default();
                state.overlay = None;
                state.enter_step(Step::Browse);
            }
        }
    }

    /// Draws Big Picture instead of the normal shell. False when it is closed.
    pub(super) fn show_big_picture(&mut self, context: &egui::Context) -> bool {
        let Some(mut state) = self.big_picture.take() else {
            return false;
        };
        let now = context.input(|input| input.time);
        if let Some(reader) = &state.reader {
            state.pad_status = reader.status();
        }
        let active = self.big_picture_launch_active(&state);
        self.observe_emulator(&mut state, context, active, now);
        let focused = context.input(|input| input.focused);
        let open = !active && focused && now >= state.resume_at;
        if let Some(reader) = &state.reader {
            reader.set_paused(!open);
        }
        if context.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F11)) {
            state.fullscreen = !state.fullscreen;
            context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(state.fullscreen));
        }
        let mut commands: Vec<(Command, bool)> = std::mem::take(&mut state.clicked)
            .into_iter()
            .map(|command| (command, false))
            .collect();
        commands.extend(state.pad_commands(now, open));
        if focused {
            // Taken even when closed, so a key held while a game runs or in
            // the grace period after it does not act on Big Picture. The Launch
            // step still passes Tab, Enter and Space to its page, whose own
            // "launch in progress" guard refuses a second start.
            let keys = keyboard_commands(context, state.step, state.overlay.as_ref());
            if open {
                commands.extend(keys.into_iter().map(|command| (command, false)));
            }
        }
        if state.nav.repeating() {
            context.request_repaint_after(std::time::Duration::from_millis(20));
        }
        if let Some(until) = state.leave_armed_until {
            if now >= until {
                state.leave_armed_until = None;
                state.notice = None;
            } else {
                context.request_repaint_after(std::time::Duration::from_secs_f64(until - now));
            }
        }
        let indices = self.browse_play_indices();
        let library = self.library.clone();
        state.sync_list(&library, &indices);
        let route = self.router.current.clone();
        let mut next = Next::Stay;
        for (command, pad_x) in commands {
            // Every command ends a pending double tap; only X can complete one.
            let tap = state.x_tap.take();
            let command = match command {
                _ if pad_x => match state.x_press(tap, command, now) {
                    XPress::Pass(command) => command,
                    XPress::Armed => continue,
                    XPress::Play => {
                        self.big_picture_play(&mut state, context);
                        state.sync_list(&library, &indices);
                        continue;
                    }
                },
                // R3 is Search here, so X is free for the double tap.
                Command::Pointer => Command::Search,
                other => other,
            };
            if self.big_picture_command(&mut state, context, command, now) == Next::Leave {
                next = Next::Leave;
            }
            state.sync_list(&library, &indices);
        }
        if next == Next::Stay {
            self.big_picture_frame(&mut state, context, &library);
        }
        // Anything that moved the app to another page has left the mode.
        if next == Next::Leave || self.router.current != route {
            Self::leave_big_picture(context, &state);
        } else {
            self.big_picture = Some(state);
        }
        true
    }

    fn big_picture_command(
        &mut self,
        state: &mut BigPicture,
        context: &egui::Context,
        command: Command,
        now: f64,
    ) -> Next {
        match state.overlay.take() {
            Some(Overlay::Search(grid)) => {
                state.overlay = self.search_command(grid, command);
                return Next::Stay;
            }
            Some(Overlay::Platforms { grid, rows }) => {
                state.overlay = self.platform_command(grid, rows, command);
                return Next::Stay;
            }
            None => {}
        }
        if command != Command::Back && state.leave_armed_until.take().is_some() {
            state.notice = None;
        }
        match state.step {
            Step::Browse => self.browse_command(state, command, now),
            Step::Game(id) => self.game_command(state, context, id, command),
            Step::Launch(id) => {
                if command == Command::Back {
                    state.enter_step(Step::Game(id));
                } else {
                    page_command(context, command);
                }
                Next::Stay
            }
        }
    }

    fn search_command(&mut self, mut grid: Grid, command: Command) -> Option<Overlay> {
        let keys = search_keys();
        match command {
            Command::Back | Command::Launch | Command::Search => return None,
            Command::Select => match keys[grid.focus.min(keys.len() - 1)] {
                SearchKey::Letter(letter) => self.filter.search.push(letter),
                SearchKey::Space => self.filter.search.push(' '),
                SearchKey::Delete => {
                    self.filter.search.pop();
                }
                SearchKey::Clear => self.filter.search.clear(),
                SearchKey::Done => return None,
            },
            other => {
                grid.apply(other, keys.len(), SEARCH_COLUMNS, keys.len());
            }
        }
        Some(Overlay::Search(grid))
    }

    fn platform_command(
        &mut self,
        mut grid: Grid,
        rows: Vec<(String, usize)>,
        command: Command,
    ) -> Option<Overlay> {
        match command {
            Command::Back | Command::Platforms => return None,
            Command::Select => {
                if let Some((name, _)) = rows.get(grid.focus) {
                    // The Library's own rule: this also clears the search.
                    self.filter.select_platform(name.clone());
                }
                return None;
            }
            other => {
                grid.apply(other, rows.len(), 1, 10);
            }
        }
        Some(Overlay::Platforms { grid, rows })
    }

    fn platform_rows(&self) -> Vec<(String, usize)> {
        let mut rows: Vec<(String, usize)> = self
            .library
            .platforms
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let total = rows.iter().map(|(_, count)| count).sum();
        rows.insert(0, (String::new(), total));
        rows
    }

    fn browse_command(&mut self, state: &mut BigPicture, command: Command, now: f64) -> Next {
        let focused = state
            .focused_position()
            .and_then(|position| self.library.games.get(state.shown[position]));
        let id = focused.map(|game| game.archive.id);
        match command {
            Command::Back => {
                if state.leave_armed_until.is_some() {
                    return Next::Leave;
                }
                state.leave_armed_until = Some(now + LEAVE_CONFIRM_SECS);
                state.notice = Some(Notice::new(
                    Tone::Info,
                    "Press Back again to leave Big Picture.",
                ));
            }
            Command::Select => {
                if let Some(id) = id {
                    state.notice = None;
                    state.enter_step(Step::Game(id));
                }
            }
            Command::Launch => match focused {
                Some(game) if game.is_preservation_only() => {
                    state.notice = Some(Notice::new(Tone::Warning, PRESERVATION_ONLY_SENTENCE));
                }
                Some(game) => {
                    state.notice = None;
                    state.enter_step(Step::Launch(game.archive.id));
                }
                None => {}
            },
            Command::Search => state.overlay = Some(Overlay::Search(Grid::default())),
            Command::Platforms => {
                let rows = self.platform_rows();
                let at = rows
                    .iter()
                    .position(|(name, _)| *name == self.filter.platform)
                    .unwrap_or(0);
                let mut grid = Grid {
                    focus: at,
                    top_row: 0,
                };
                grid.reveal(rows.len(), 1, 10);
                state.overlay = Some(Overlay::Platforms { grid, rows });
            }
            movement => {
                if state
                    .grid
                    .apply(movement, state.shown.len(), state.columns, state.rows)
                {
                    state.notice = None;
                }
            }
        }
        Next::Stay
    }

    fn game_command(
        &mut self,
        state: &mut BigPicture,
        context: &egui::Context,
        id: i64,
        command: Command,
    ) -> Next {
        match command {
            Command::Back => state.enter_step(Step::Browse),
            Command::Move(Direction::Left | Direction::Up) => {
                state.action = state.action.saturating_sub(1);
            }
            Command::Move(Direction::Right | Direction::Down) => {
                state.action = (state.action + 1).min(state.actions.len().saturating_sub(1));
            }
            Command::Select => {
                if let Some(action) = state.actions.get(state.action).copied() {
                    return self.game_action(state, context, id, action);
                }
            }
            Command::Launch => {
                if state.actions.contains(&GameAction::Play) {
                    state.enter_step(Step::Launch(id));
                } else if let Some(reason) = state.play_refusal.clone() {
                    state.notice = Some(Notice::new(Tone::Warning, reason));
                }
            }
            _ => {}
        }
        Next::Stay
    }

    /// X twice: the game card's own Play (`ReadinessAction::Play`), which
    /// starts the planner's single safe request when there is one and
    /// otherwise waits on, or refuses on, the Launch page. The Launch page is
    /// shown either way, so a running game, a refusal or a needed choice is
    /// always on screen. A game Play is not offered for is refused here, with
    /// the reason Start gives.
    fn big_picture_play(&mut self, state: &mut BigPicture, context: &egui::Context) {
        let Some((step, id)) = state.x_target() else {
            return;
        };
        match step {
            Step::Browse => match self.library.game(id) {
                Some(game) if game.is_preservation_only() => {
                    state.notice = Some(Notice::new(Tone::Warning, PRESERVATION_ONLY_SENTENCE));
                    return;
                }
                Some(_) => {}
                None => return,
            },
            _ if !state.actions.contains(&GameAction::Play) => {
                if let Some(reason) = state.play_refusal.clone() {
                    state.notice = Some(Notice::new(Tone::Warning, reason));
                }
                return;
            }
            _ => {}
        }
        state.notice = None;
        // The Launch page creates these on first draw; Play needs them now so
        // a game never checked before still has its Play waiting for it.
        let workflows = self
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(context.clone()));
        let _ = workflows.readiness_action_route(context, ReadinessAction::Play, id);
        state.enter_step(Step::Launch(id));
    }

    fn game_action(
        &mut self,
        state: &mut BigPicture,
        context: &egui::Context,
        id: i64,
        action: GameAction,
    ) -> Next {
        match action {
            GameAction::Play => state.enter_step(Step::Launch(id)),
            GameAction::Details => self.go(Route::Game(id)),
            GameAction::Fix(fix) => {
                let route = self
                    .native_workflows
                    .as_mut()
                    .and_then(|workflows| workflows.readiness_action_route(context, fix, id));
                match route {
                    // Choosing an emulator is on the Launch page itself.
                    Some(Route::Task {
                        section: super::routes::Section::Launch,
                        game,
                    }) => state.enter_step(Step::Launch(game)),
                    // Repair work needs the full desktop tools.
                    Some(route) => self.go(route),
                    None => {}
                }
            }
        }
        Next::Stay
    }

    fn big_picture_frame(
        &mut self,
        state: &mut BigPicture,
        context: &egui::Context,
        library: &Arc<Library>,
    ) {
        let frame = egui::Frame::new()
            .fill(blue::DEEP_BACKGROUND)
            .inner_margin(egui::Margin::same(MARGIN as i8));
        egui::TopBottomPanel::top("big_picture_header")
            .frame(frame)
            .show_separator_line(false)
            .show(context, |ui| self.big_picture_header(ui, state));
        egui::TopBottomPanel::bottom("big_picture_footer")
            .frame(frame)
            .show_separator_line(false)
            .show(context, |ui| big_picture_footer(ui, state));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(blue::DEEP_BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(MARGIN as i8, 0)),
            )
            .show(context, |ui| match state.step {
                Step::Browse => self.big_picture_browse(ui, state, library),
                Step::Game(id) => self.big_picture_game(ui, state, id, library),
                Step::Launch(id) => {
                    let style = ui.style_mut();
                    for font in style.text_styles.values_mut() {
                        font.size *= PAGE_TEXT_SCALE;
                    }
                    self.launch(ui, id);
                }
            });
        if let Some(overlay) = state.overlay.take() {
            state.overlay = Some(self.big_picture_overlay(context, state, overlay));
        }
    }

    fn big_picture_header(&mut self, ui: &mut egui::Ui, state: &BigPicture) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Big Picture")
                    .size(32.0)
                    .strong()
                    .color(blue::PRIMARY_TEXT),
            );
            ui.add_space(GAP);
            let mut facts = Vec::new();
            if self.loaded {
                let count = state.shown.len();
                let noun = if count == 1 { "game" } else { "games" };
                facts.push(format!("{} {noun}", thousands(count)));
            }
            if !self.filter.platform.is_empty() {
                facts.push(self.filter.platform.clone());
            }
            let search = self.filter.search.trim();
            if !search.is_empty() {
                facts.push(format!("\"{search}\""));
            }
            if self.filter.readiness != super::library::ReadinessFilter::All {
                facts.push(self.filter.readiness.label().to_string());
            }
            ui.label(
                RichText::new(facts.join("  ·  "))
                    .size(20.0)
                    .color(blue::BODY_TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(state.pad_status.describe())
                        .size(META_SIZE)
                        .color(blue::SECONDARY_TEXT),
                );
            });
        });
        if state.running.is_some() {
            notice_strip(
                ui,
                &Notice::new(
                    Tone::Info,
                    "A game is running. EmuWiz ignores the controller until it closes.",
                ),
            );
        } else if let Some(notice) = &state.notice {
            notice_strip(ui, notice);
        }
    }

    fn big_picture_browse(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut BigPicture,
        library: &Arc<Library>,
    ) {
        if library.games.is_empty() {
            let (title, body) = if self.loaded {
                (
                    "No games have been added yet",
                    "Add a game folder in the desktop view, then scan it into your library.",
                )
            } else {
                ("Loading your game list…", "Nothing is being changed.")
            };
            centred_message(ui, title, body, !self.loaded);
            return;
        }
        if library.games.len() > super::browse_play::IMMEDIATE_FILTER_LIMIT
            && (self.filter_dirty.is_some() || self.filter_inflight)
        {
            centred_message(ui, "Updating results…", "", true);
            return;
        }
        if state.shown.is_empty() {
            centred_message(
                ui,
                "No games match",
                "Try a different search or system.",
                false,
            );
            return;
        }
        let area = ui.available_size();
        let columns = (((area.x + GAP) / (TILE_MIN_WIDTH + GAP)).floor() as usize).max(1);
        let width = ((area.x - GAP * (columns as f32 - 1.0)) / columns as f32).min(TILE_MAX_WIDTH);
        let text = TILE_TEXT_HEIGHT + 2.0 * TILE_PAD;
        // As many rows as fit with a slightly shorter cover, not a half-empty screen.
        let tightest = width * COVER_MIN_RATIO + text;
        let rows = (((area.y + GAP) / (tightest + GAP)).floor() as usize).max(1);
        let height = ((area.y - GAP * (rows as f32 - 1.0)) / rows as f32)
            .min(width * COVER_RATIO + text)
            .max(MIN_COVER_HEIGHT + text);
        if (state.columns, state.rows) != (columns, rows) {
            state.columns = columns;
            state.rows = rows;
            state.grid.reveal(state.shown.len(), columns, rows);
        }
        let origin = ui.max_rect().min;
        if ui.rect_contains_pointer(ui.max_rect()) {
            state.wheel += ui.input(|input| input.smooth_scroll_delta.y);
            let step = height + GAP;
            let whole = (state.wheel / step).trunc();
            if whole != 0.0 {
                state.wheel -= whole * step;
                state
                    .grid
                    .scroll(-(whole as isize), state.shown.len(), columns, rows);
            }
        }
        let top = state.grid.top_row;
        for row in top..top + rows {
            for column in 0..columns {
                let position = row * columns + column;
                let Some(game) = state
                    .shown
                    .get(position)
                    .and_then(|index| library.games.get(*index))
                else {
                    return;
                };
                let rect = egui::Rect::from_min_size(
                    origin
                        + egui::vec2(
                            column as f32 * (width + GAP),
                            (row - top) as f32 * (height + GAP),
                        ),
                    egui::vec2(width, height),
                );
                let focused = position == state.grid.focus;
                let response = ui.interact(
                    rect,
                    egui::Id::new(("big_picture_tile", game.archive.id)),
                    egui::Sense::CLICK,
                );
                self.big_picture_tile(ui, game, rect, focused);
                if response.clicked() {
                    if focused {
                        state.clicked.push(Command::Select);
                        ui.ctx().request_repaint();
                    } else {
                        state.grid.focus = position;
                    }
                }
            }
        }
    }

    fn big_picture_tile(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        rect: egui::Rect,
        focused: bool,
    ) {
        let painter = ui.painter_at(rect.expand(FOCUS_WIDTH));
        let fill = if focused {
            blue::SELECTED_FILL
        } else {
            blue::CARD_SURFACE
        };
        painter.rect_filled(rect, 12.0, fill);
        let inner = rect.shrink(TILE_PAD);
        let cover = egui::vec2(inner.width(), inner.height() - TILE_TEXT_HEIGHT);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        self.picture_tile(&mut child, game, Kind::Cover, cover);
        let text_top = inner.min.y + cover.y + 6.0;
        let mut title = egui::text::LayoutJob::single_section(
            game.title.clone(),
            egui::TextFormat {
                font_id: egui::FontId::proportional(TITLE_SIZE),
                color: blue::PRIMARY_TEXT,
                ..Default::default()
            },
        );
        title.wrap = egui::text::TextWrapping {
            max_width: inner.width(),
            max_rows: 2,
            break_anywhere: false,
            overflow_character: Some('…'),
        };
        let title = painter.layout_job(title);
        painter.galley(egui::pos2(inner.min.x, text_top), title, blue::PRIMARY_TEXT);
        let (meta, colour) = match tile_badge(game) {
            Some((badge, colour)) => (format!("{badge} · {}", game.platform), colour),
            None => (game.platform.clone(), blue::SECONDARY_TEXT),
        };
        let mut meta = egui::text::LayoutJob::simple(
            meta,
            egui::FontId::proportional(META_SIZE),
            colour,
            inner.width(),
        );
        meta.wrap.max_rows = 1;
        meta.wrap.overflow_character = Some('…');
        let meta = painter.layout_job(meta);
        painter.galley(
            egui::pos2(inner.min.x, inner.max.y - meta.size().y),
            meta,
            colour,
        );
        if focused {
            // Outline width as well as colour, so focus survives a washed-out TV.
            painter.rect_stroke(
                rect,
                12.0,
                egui::Stroke::new(FOCUS_WIDTH, blue::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn big_picture_game(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut BigPicture,
        id: i64,
        library: &Arc<Library>,
    ) {
        let Some(game) = library.game(id).cloned() else {
            state.notice = Some(Notice::new(
                Tone::Warning,
                "This game is no longer in the library.",
            ));
            state.enter_step(Step::Browse);
            return;
        };
        let view = (!game.is_preservation_only()).then(|| self.big_picture_readiness(ui, &game));
        state.actions = view
            .as_ref()
            .map_or_else(|| vec![GameAction::Details], game_actions);
        state.action = state.action.min(state.actions.len() - 1);
        state.play_refusal = view
            .as_ref()
            .filter(|view| !view.play_enabled)
            .map(|view| view.headline.clone());
        ui.horizontal_top(|ui| {
            let height = ui.available_height().clamp(240.0, 560.0);
            self.picture(ui, &game, Kind::Cover, egui::vec2(height * 0.75, height));
            ui.add_space(GAP * 2.0);
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(&game.title)
                            .size(38.0)
                            .strong()
                            .color(blue::PRIMARY_TEXT),
                    )
                    .wrap(),
                );
                ui.label(
                    RichText::new(format!(
                        "{}  ·  {}",
                        game.platform,
                        media_kind_label(&game.archive.archive_kind)
                    ))
                    .size(20.0)
                    .color(blue::BODY_TEXT),
                );
                if let Some((badge, colour)) = tile_badge(&game) {
                    ui.label(RichText::new(badge).size(22.0).strong().color(colour));
                }
                ui.add_space(GAP);
                match &view {
                    Some(view) => {
                        ui.horizontal(|ui| {
                            crate::ui::components::status_badge(
                                ui,
                                view.badge,
                                readiness::blue_status_tone(view.tone),
                            );
                            if view.spinner {
                                ui.spinner();
                            }
                        });
                        ui.add(
                            egui::Label::new(
                                RichText::new(&view.reason)
                                    .size(20.0)
                                    .color(blue::BODY_TEXT),
                            )
                            .wrap(),
                        );
                    }
                    None => {
                        ui.add(
                            egui::Label::new(
                                RichText::new(PRESERVATION_ONLY_SENTENCE)
                                    .size(20.0)
                                    .color(blue::BODY_TEXT),
                            )
                            .wrap(),
                        );
                    }
                }
                ui.add_space(GAP);
                ui.horizontal_wrapped(|ui| {
                    for (index, action) in state.actions.iter().enumerate() {
                        let focused = index == state.action;
                        let mut button = egui::Button::new(
                            RichText::new(action_label(*action)).size(24.0).strong(),
                        )
                        .min_size(egui::vec2(220.0, 72.0))
                        .sense(egui::Sense::CLICK);
                        button = if focused {
                            button
                                .fill(blue::PRIMARY_ACTION)
                                .stroke(egui::Stroke::new(FOCUS_WIDTH, blue::ACCENT))
                        } else {
                            button.fill(blue::CARD_SURFACE)
                        };
                        if ui.add(button).clicked() {
                            state.action = index;
                            state.clicked.push(Command::Select);
                            ui.ctx().request_repaint();
                        }
                    }
                });
            });
        });
    }

    /// The same read-only compact projection as the Library's selected-game
    /// panel. The Game step was opened on purpose, so it needs no settle delay.
    fn big_picture_readiness(&mut self, ui: &egui::Ui, game: &Game) -> CompactReadiness {
        let workflows = self
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(ui.ctx().clone()));
        workflows.note_catalogue_row(&game.archive);
        let view = workflows.compact_readiness(
            ui.ctx(),
            game.archive.id,
            &game.archive.absolute_path,
            game.archive.identity_report.as_ref(),
            game.dat_exact.is_some(),
            &mut self.activity,
        );
        self.note_checked_readiness(game, &view);
        view
    }

    fn big_picture_overlay(
        &mut self,
        context: &egui::Context,
        state: &mut BigPicture,
        mut overlay: Overlay,
    ) -> Overlay {
        let screen = context.content_rect();
        let width = (screen.width() - 80.0).min(960.0);
        let height = (screen.height() - 200.0).max(240.0);
        egui::Area::new(egui::Id::new("big_picture_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.center() - egui::vec2(width, height) / 2.0)
            .show(context, |ui| {
                egui::Frame::new()
                    .fill(blue::CARD_SURFACE_TOP)
                    .stroke(egui::Stroke::new(2.0_f32, blue::BORDER_STRONG))
                    .corner_radius(14.0)
                    .inner_margin(egui::Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_width(width - 40.0);
                        ui.set_max_height(height - 40.0);
                        match &mut overlay {
                            Overlay::Search(grid) => {
                                if type_search(context, &mut self.filter.search) {
                                    state.clicked.push(Command::Back);
                                    context.request_repaint();
                                }
                                self.search_overlay(ui, state, grid);
                            }
                            Overlay::Platforms { grid, rows } => {
                                platform_overlay(ui, state, grid, rows);
                            }
                        }
                    });
            });
        overlay
    }

    fn search_overlay(&mut self, ui: &mut egui::Ui, state: &mut BigPicture, grid: &mut Grid) {
        let text = if self.filter.search.is_empty() {
            RichText::new("Search by title or system").color(blue::SECONDARY_TEXT)
        } else {
            RichText::new(format!("{}_", self.filter.search)).color(blue::PRIMARY_TEXT)
        };
        ui.label(text.size(30.0));
        ui.add_space(GAP);
        let keys = search_keys();
        let spacing = 6.0;
        let cell = ((ui.available_width() - spacing * 9.0) / SEARCH_COLUMNS as f32).floor();
        ui.spacing_mut().item_spacing = egui::vec2(spacing, spacing);
        for (row, keys) in keys.chunks(SEARCH_COLUMNS).enumerate() {
            ui.horizontal(|ui| {
                for (column, key) in keys.iter().enumerate() {
                    let index = row * SEARCH_COLUMNS + column;
                    let focused = index == grid.focus;
                    let mut button =
                        egui::Button::new(RichText::new(key.label()).size(22.0).strong())
                            .min_size(egui::vec2(cell, 58.0))
                            .sense(egui::Sense::CLICK)
                            .fill(blue::CARD_SURFACE);
                    if focused {
                        button = button
                            .fill(blue::PRIMARY_ACTION)
                            .stroke(egui::Stroke::new(FOCUS_WIDTH, blue::ACCENT));
                    }
                    if ui.add(button).clicked() {
                        grid.focus = index;
                        state.clicked.push(Command::Select);
                        ui.ctx().request_repaint();
                    }
                }
            });
        }
    }
}

fn platform_overlay(
    ui: &mut egui::Ui,
    state: &mut BigPicture,
    grid: &mut Grid,
    rows: &[(String, usize)],
) {
    ui.label(
        RichText::new("Choose a system")
            .size(30.0)
            .strong()
            .color(blue::PRIMARY_TEXT),
    );
    ui.add_space(GAP / 2.0);
    let shown = ((ui.max_rect().height() - 60.0) / PLATFORM_ROW_HEIGHT)
        .floor()
        .max(3.0) as usize;
    grid.reveal(rows.len(), 1, shown);
    for (offset, (name, count)) in rows.iter().skip(grid.top_row).take(shown).enumerate() {
        let index = grid.top_row + offset;
        let focused = index == grid.focus;
        let label = if name.is_empty() { "All systems" } else { name };
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), PLATFORM_ROW_HEIGHT - 6.0),
            egui::Sense::CLICK,
        );
        let fill = if focused {
            blue::PRIMARY_ACTION
        } else {
            blue::CARD_SURFACE
        };
        ui.painter().rect_filled(rect, 8.0, fill);
        if focused {
            ui.painter().rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(FOCUS_WIDTH, blue::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        let middle = rect.left_center() + egui::vec2(16.0, 0.0);
        ui.painter().text(
            middle,
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(22.0),
            blue::PRIMARY_TEXT,
        );
        ui.painter().text(
            rect.right_center() - egui::vec2(16.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            thousands(*count),
            egui::FontId::proportional(META_SIZE),
            blue::BODY_TEXT,
        );
        if response.clicked() {
            grid.focus = index;
            state.clicked.push(Command::Select);
            ui.ctx().request_repaint();
        }
    }
}

fn notice_strip(ui: &mut egui::Ui, notice: &Notice) {
    let colour = match notice.tone {
        Tone::Info => blue::ACCENT,
        Tone::Warning => blue::WARN,
        Tone::Error => blue::BAD_TEXT,
    };
    ui.add_space(GAP / 2.0);
    egui::Frame::new()
        .fill(blue::CARD_SURFACE)
        .stroke(egui::Stroke::new(2.0_f32, colour))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(&notice.text)
                        .size(20.0)
                        .color(blue::PRIMARY_TEXT),
                )
                .wrap(),
            );
        });
}

fn centred_message(ui: &mut egui::Ui, title: &str, body: &str, spinner: bool) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        if spinner {
            ui.spinner();
        }
        ui.label(
            RichText::new(title)
                .size(32.0)
                .strong()
                .color(blue::PRIMARY_TEXT),
        );
        if !body.is_empty() {
            ui.label(RichText::new(body).size(20.0).color(blue::BODY_TEXT));
        }
    });
}

/// The controls for this screen, each naming its controller button and key.
/// Clicking one does the same thing, so a mouse needs nothing else.
fn big_picture_footer(ui: &mut egui::Ui, state: &mut BigPicture) {
    let legend: &[(&str, Option<Command>)] = match (&state.overlay, state.step) {
        (Some(Overlay::Search(_)), _) => &[
            ("A  Press key", Some(Command::Select)),
            ("Start / Enter  Done", Some(Command::Launch)),
            ("B / Esc  Close", Some(Command::Back)),
        ],
        (Some(Overlay::Platforms { .. }), _) => &[
            ("A / Enter  Choose", Some(Command::Select)),
            ("B / Esc  Close", Some(Command::Back)),
        ],
        (None, Step::Browse) => &[
            ("A / Enter  Select", Some(Command::Select)),
            ("Start / L  Launch", Some(Command::Launch)),
            ("B / Esc  Back", Some(Command::Back)),
            ("R3 / S  Search", Some(Command::Search)),
            ("Y / F  System", Some(Command::Platforms)),
            ("LB RB / PgUp PgDn  Page", None),
            ("F11  Full screen", None),
        ],
        (None, Step::Game(_)) => &[
            ("A / Enter  Select", Some(Command::Select)),
            ("Start / L  Launch", Some(Command::Launch)),
            ("B / Esc  Back", Some(Command::Back)),
        ],
        (None, Step::Launch(_)) => &[
            ("D-pad / Tab  Move", None),
            ("A / Enter  Activate", None),
            ("B / Esc  Back", Some(Command::Back)),
        ],
    };
    // Only with a controller: the double tap is a controller shortcut.
    let x_twice = state.pad_status.connected()
        && state.x_target().is_some()
        && (state.step == Step::Browse || state.actions.contains(&GameAction::Play));
    let x_twice: &[(&str, Option<Command>)] = if x_twice {
        &[("X twice  Play", None)]
    } else {
        &[]
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 28.0;
        for (text, command) in legend.iter().chain(x_twice) {
            let text = RichText::new(*text).size(19.0).color(blue::BODY_TEXT);
            let mut label = egui::Label::new(text).selectable(false);
            if command.is_some() {
                // Not focusable: Tab on the Launch page must stay on its controls.
                label = label.sense(egui::Sense::CLICK);
            }
            let response = ui.add(label);
            if let Some(command) = command
                && response.clicked()
            {
                state.clicked.push(*command);
                ui.ctx().request_repaint();
            }
        }
    });
}

#[cfg(test)]
impl BigPicture {
    pub(super) fn step(&self) -> Step {
        self.step
    }
    pub(super) fn focus(&self) -> usize {
        self.grid.focus
    }
    pub(super) fn focused_game(&self) -> Option<i64> {
        self.focused
    }
    pub(super) fn set_focus(&mut self, focus: usize) {
        self.grid.focus = focus;
    }
    pub(super) fn columns(&self) -> usize {
        self.columns
    }
    pub(super) fn notice_text(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_str())
    }
    pub(super) fn search_open(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Search(_)))
    }
    pub(super) fn platforms_open(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Platforms { .. }))
    }
    pub(super) fn fullscreen(&self) -> bool {
        self.fullscreen
    }
    pub(super) fn inject(&mut self, event: PadEvent) {
        self.injected.push(event);
    }
    pub(super) fn set_launch_active(&mut self, active: bool) {
        self.launch_active = active;
    }
    pub(super) fn set_connected(&mut self) {
        self.pad_status = PadStatus::Connected(vec!["Test pad".into()]);
        self.fullscreen = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_v2::launch_readiness_summary::ReadinessPresentationState as State;

    fn view(play_enabled: bool, fix: Option<ReadinessAction>, pending: bool) -> CompactReadiness {
        CompactReadiness {
            badge: "x",
            tone: State::Checking,
            spinner: false,
            reason: String::new(),
            headline: String::new(),
            play_enabled,
            fix,
            pending,
            more_findings: 0,
        }
    }

    #[test]
    fn a_refused_game_offers_its_fix_first_and_never_play() {
        assert_eq!(
            game_actions(&view(false, Some(ReadinessAction::SetUpEmulator), false)),
            [
                GameAction::Fix(ReadinessAction::SetUpEmulator),
                GameAction::Details
            ]
        );
    }

    #[test]
    fn a_ready_or_unchecked_game_offers_play_first() {
        assert_eq!(
            game_actions(&view(true, None, false)),
            [GameAction::Play, GameAction::Details]
        );
        assert_eq!(
            game_actions(&view(true, Some(ReadinessAction::ReviewIdentity), true)),
            [
                GameAction::Play,
                GameAction::Fix(ReadinessAction::ReviewIdentity),
                GameAction::Details
            ]
        );
    }

    #[test]
    fn a_session_ending_says_what_happened_without_claiming_more() {
        assert_eq!(session_notice("Doom", None).text, "Doom has closed.");
        let quick = session_notice("Doom", Some(&Attempt::EndedQuickly { seconds: 2 }));
        assert_eq!(quick.tone, Tone::Warning);
        assert!(quick.text.contains("closed after 2 seconds"));
    }
}
