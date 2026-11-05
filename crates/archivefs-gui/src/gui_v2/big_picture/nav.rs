//! Device-independent Big Picture navigation. Nothing here draws or reads a
//! device, so every rule (repeat, gating, focus movement) is tested directly.

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::gui_v2) enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Named by physical position, which is what the kernel reports: South is
/// Xbox A, PlayStation Cross and the lower Nintendo button.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::gui_v2) enum Button {
    South,
    East,
    West,
    North,
    LeftBumper,
    RightBumper,
    Start,
    /// Right stick pressed in (R3).
    RightStick,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::gui_v2) enum Control {
    Button(Button),
    /// D-pad and analogue stick both arrive as a direction.
    Direction(Direction),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::gui_v2) enum PadEvent {
    Pressed {
        device: u32,
        control: Control,
    },
    Released {
        device: u32,
        control: Control,
    },
    /// The device left, or the kernel dropped events: what it holds is unknown.
    Reset {
        device: u32,
    },
    /// The right stick, each axis -1.0 to 1.0 with up/left negative; (0, 0)
    /// when at rest. Only sent while a consumer asked for it.
    RightStick {
        device: u32,
        x: f32,
        y: f32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::gui_v2) enum Command {
    Move(Direction),
    PageUp,
    PageDown,
    First,
    Last,
    Select,
    Back,
    Launch,
    Search,
    Platforms,
    /// R3: the controller's virtual mouse (ordinary pages only).
    Pointer,
}

const REPEAT_DELAY: f64 = 0.35;
const REPEAT_SLOW: f64 = 0.11;
const REPEAT_FAST: f64 = 0.04;
/// Holding this long speeds up, so a long shelf is quick to cross.
const REPEAT_FAST_AFTER: f64 = 1.5;

fn command_for(control: Control) -> Command {
    match control {
        Control::Direction(direction) => Command::Move(direction),
        Control::Button(Button::South) => Command::Select,
        Control::Button(Button::East) => Command::Back,
        Control::Button(Button::Start) => Command::Launch,
        Control::Button(Button::West) => Command::Search,
        Control::Button(Button::North) => Command::Platforms,
        Control::Button(Button::LeftBumper) => Command::PageUp,
        Control::Button(Button::RightBumper) => Command::PageDown,
        Control::Button(Button::RightStick) => Command::Pointer,
    }
}

fn repeats(control: Control) -> bool {
    matches!(
        control,
        Control::Direction(_)
            | Control::Button(Button::LeftBumper)
            | Control::Button(Button::RightBumper)
    )
}

struct Repeat {
    device: u32,
    control: Control,
    since: f64,
    next: f64,
}

/// Turns pad events into commands. A press only counts when it happens while
/// the gate is open: anything already held when input resumes (the button that
/// quit an emulator, say) does nothing until it is released and pressed again.
#[derive(Default)]
pub(in crate::gui_v2) struct Navigator {
    held: HashSet<(u32, Control)>,
    repeat: Option<Repeat>,
}

impl Navigator {
    pub(in crate::gui_v2) fn feed(
        &mut self,
        event: PadEvent,
        now: f64,
        open: bool,
    ) -> Option<Command> {
        match event {
            PadEvent::Pressed { device, control } => {
                if !self.held.insert((device, control)) || !open {
                    return None;
                }
                self.repeat = repeats(control).then_some(Repeat {
                    device,
                    control,
                    since: now,
                    next: now + REPEAT_DELAY,
                });
                Some(command_for(control))
            }
            PadEvent::Released { device, control } => {
                self.held.remove(&(device, control));
                if self
                    .repeat
                    .as_ref()
                    .is_some_and(|r| r.device == device && r.control == control)
                {
                    self.repeat = None;
                }
                None
            }
            PadEvent::RightStick { .. } => None,
            PadEvent::Reset { device } => {
                self.held.retain(|(held, _)| *held != device);
                if self.repeat.as_ref().is_some_and(|r| r.device == device) {
                    self.repeat = None;
                }
                None
            }
        }
    }

    /// The repeat due at `now`, at most one per frame.
    pub(in crate::gui_v2) fn tick(&mut self, now: f64, open: bool) -> Option<Command> {
        if !open {
            self.repeat = None;
            return None;
        }
        let repeat = self.repeat.as_mut()?;
        if now < repeat.next {
            return None;
        }
        let gap = if now - repeat.since >= REPEAT_FAST_AFTER {
            REPEAT_FAST
        } else {
            REPEAT_SLOW
        };
        repeat.next = now + gap;
        Some(command_for(repeat.control))
    }

    pub(in crate::gui_v2) fn repeating(&self) -> bool {
        self.repeat.is_some()
    }
}

/// A focused position in a virtualised grid and the first row on screen.
/// Moves are O(1), so the size of the library does not matter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::gui_v2) struct Grid {
    pub(in crate::gui_v2) focus: usize,
    pub(in crate::gui_v2) top_row: usize,
}

impl Grid {
    /// Whether the focus moved. Edges stop rather than wrap: wrapping from
    /// the end of a 133,000-game shelf to its start loses your place.
    pub(in crate::gui_v2) fn apply(
        &mut self,
        command: Command,
        len: usize,
        columns: usize,
        rows: usize,
    ) -> bool {
        if len == 0 {
            *self = Self::default();
            return false;
        }
        let columns = columns.max(1);
        let rows = rows.max(1);
        let before = self.focus.min(len - 1);
        let last_row = (len - 1) / columns;
        let row = before / columns;
        let target = match command {
            Command::Move(Direction::Left) if !before.is_multiple_of(columns) => before - 1,
            Command::Move(Direction::Right) if before % columns + 1 < columns => {
                (before + 1).min(len - 1)
            }
            Command::Move(Direction::Up) if row > 0 => before - columns,
            Command::Move(Direction::Down) if row < last_row => (before + columns).min(len - 1),
            Command::PageUp => before.saturating_sub(columns * rows),
            Command::PageDown => (before + columns * rows).min(len - 1),
            Command::First => 0,
            Command::Last => len - 1,
            _ => before,
        };
        self.focus = target;
        self.reveal(len, columns, rows);
        target != before
    }

    /// Bring the focus on screen without moving it.
    pub(in crate::gui_v2) fn reveal(&mut self, len: usize, columns: usize, rows: usize) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        let columns = columns.max(1);
        let rows = rows.max(1);
        self.focus = self.focus.min(len - 1);
        let row = self.focus / columns;
        if row < self.top_row {
            self.top_row = row;
        } else if row >= self.top_row + rows {
            self.top_row = row + 1 - rows;
        }
        self.top_row = self
            .top_row
            .min(((len - 1) / columns).saturating_sub(rows - 1));
    }

    /// Scroll the view (mouse wheel); the focus stays on screen with it.
    pub(in crate::gui_v2) fn scroll(
        &mut self,
        delta_rows: isize,
        len: usize,
        columns: usize,
        rows: usize,
    ) {
        if len == 0 {
            return;
        }
        let columns = columns.max(1);
        let rows = rows.max(1);
        let last_top = ((len - 1) / columns).saturating_sub(rows - 1);
        self.top_row = self.top_row.saturating_add_signed(delta_rows).min(last_top);
        let row = (self.focus / columns).clamp(self.top_row, self.top_row + rows - 1);
        self.focus = (row * columns + self.focus % columns).min(len - 1);
    }
}

/// One key of the controller's on-screen keyboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::gui_v2) enum SearchKey {
    Letter(char),
    Space,
    Delete,
    Clear,
    Done,
}

pub(in crate::gui_v2) const SEARCH_COLUMNS: usize = 10;

pub(in crate::gui_v2) fn search_keys() -> Vec<SearchKey> {
    let mut keys: Vec<SearchKey> = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-'.&"
        .chars()
        .map(SearchKey::Letter)
        .collect();
    keys.extend([
        SearchKey::Space,
        SearchKey::Delete,
        SearchKey::Clear,
        SearchKey::Done,
    ]);
    keys
}

impl SearchKey {
    pub(in crate::gui_v2) fn label(self) -> String {
        match self {
            Self::Letter(letter) => letter.to_string(),
            Self::Space => "Space".into(),
            Self::Delete => "Delete".into(),
            Self::Clear => "Clear".into(),
            Self::Done => "Done".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOWN: Control = Control::Direction(Direction::Down);

    fn press(control: Control) -> PadEvent {
        PadEvent::Pressed { device: 1, control }
    }
    fn release(control: Control) -> PadEvent {
        PadEvent::Released { device: 1, control }
    }

    #[test]
    fn default_buttons_map_to_select_back_launch_search_and_platforms() {
        for (button, command) in [
            (Button::South, Command::Select),
            (Button::East, Command::Back),
            (Button::Start, Command::Launch),
            (Button::West, Command::Search),
            (Button::North, Command::Platforms),
            (Button::LeftBumper, Command::PageUp),
            (Button::RightBumper, Command::PageDown),
        ] {
            let mut nav = Navigator::default();
            assert_eq!(
                nav.feed(press(Control::Button(button)), 0.0, true),
                Some(command)
            );
        }
    }

    #[test]
    fn a_held_direction_repeats_after_the_delay_then_speeds_up_and_stops_on_release() {
        let mut nav = Navigator::default();
        assert_eq!(
            nav.feed(press(DOWN), 0.0, true),
            Some(Command::Move(Direction::Down))
        );
        assert_eq!(nav.tick(0.2, true), None, "no repeat inside the delay");
        assert!(nav.tick(0.36, true).is_some());
        assert_eq!(nav.tick(0.40, true), None, "slow repeat gap");
        assert!(nav.tick(0.48, true).is_some());
        assert!(nav.tick(2.0, true).is_some());
        assert!(nav.tick(2.05, true).is_some(), "fast repeat after a hold");
        nav.feed(release(DOWN), 2.06, true);
        assert_eq!(nav.tick(3.0, true), None);
    }

    #[test]
    fn select_back_and_launch_never_repeat() {
        let mut nav = Navigator::default();
        for button in [Button::South, Button::East, Button::Start] {
            nav.feed(press(Control::Button(button)), 0.0, true);
            assert_eq!(nav.tick(5.0, true), None, "{button:?}");
        }
    }

    #[test]
    fn a_press_while_the_gate_is_closed_does_nothing_even_after_it_opens() {
        let mut nav = Navigator::default();
        let select = Control::Button(Button::South);
        assert_eq!(nav.feed(press(select), 0.0, false), None);
        assert_eq!(
            nav.feed(press(select), 1.0, true),
            None,
            "still the same hold"
        );
        assert_eq!(nav.tick(9.0, true), None);
        nav.feed(release(select), 2.0, true);
        assert_eq!(nav.feed(press(select), 3.0, true), Some(Command::Select));
    }

    #[test]
    fn closing_the_gate_cancels_a_repeat() {
        let mut nav = Navigator::default();
        nav.feed(press(DOWN), 0.0, true);
        assert_eq!(nav.tick(1.0, false), None);
        assert_eq!(nav.tick(2.0, true), None);
    }

    #[test]
    fn an_unplugged_pad_cannot_leave_a_direction_repeating() {
        let mut nav = Navigator::default();
        nav.feed(press(DOWN), 0.0, true);
        nav.feed(PadEvent::Reset { device: 1 }, 0.1, true);
        assert_eq!(nav.tick(1.0, true), None);
        assert_eq!(
            nav.feed(press(DOWN), 2.0, true),
            Some(Command::Move(Direction::Down))
        );
    }

    #[test]
    fn grid_moves_stop_at_edges_and_a_short_last_row_is_reachable() {
        let mut grid = Grid::default();
        let (len, columns, rows) = (10, 4, 2);
        let mv =
            |grid: &mut Grid, direction| grid.apply(Command::Move(direction), len, columns, rows);
        assert!(!mv(&mut grid, Direction::Left));
        assert!(!mv(&mut grid, Direction::Up));
        assert!(mv(&mut grid, Direction::Right));
        assert_eq!(grid.focus, 1);
        grid.focus = 3;
        assert!(!mv(&mut grid, Direction::Right), "end of row stops");
        assert!(mv(&mut grid, Direction::Down));
        assert_eq!(grid.focus, 7);
        assert!(mv(&mut grid, Direction::Down), "column past the short row");
        assert_eq!(grid.focus, 9);
        assert!(!mv(&mut grid, Direction::Down));
        assert!(!mv(&mut grid, Direction::Right));
        assert!(mv(&mut grid, Direction::Up));
        assert_eq!(grid.focus, 5);
    }

    #[test]
    fn the_focused_row_is_always_on_screen_across_a_large_library() {
        let (len, columns, rows) = (133_000, 7, 3);
        let mut grid = Grid::default();
        grid.apply(Command::Last, len, columns, rows);
        assert_eq!(grid.focus, len - 1);
        assert!(grid.focus / columns < grid.top_row + rows);
        assert_eq!(grid.top_row, (len - 1) / columns - (rows - 1));
        grid.apply(Command::PageUp, len, columns, rows);
        assert_eq!(grid.focus, len - 1 - columns * rows);
        assert!(grid.focus / columns >= grid.top_row);
        grid.apply(Command::First, len, columns, rows);
        assert_eq!(grid, Grid::default());
    }

    #[test]
    fn a_narrower_window_keeps_the_same_game_focused_and_on_screen() {
        let mut grid = Grid {
            focus: 100,
            top_row: 14,
        };
        grid.reveal(200, 3, 2);
        assert_eq!(grid.focus, 100);
        assert!((grid.top_row..grid.top_row + 2).contains(&(100 / 3)));
    }

    #[test]
    fn a_shrunken_list_clamps_focus_and_an_empty_one_resets() {
        let mut grid = Grid {
            focus: 50,
            top_row: 10,
        };
        grid.reveal(5, 2, 2);
        assert_eq!(grid.focus, 4);
        assert_eq!(grid.top_row, 1);
        grid.reveal(0, 2, 2);
        assert_eq!(grid, Grid::default());
    }

    #[test]
    fn wheel_scrolling_stops_at_the_ends_and_keeps_focus_on_screen() {
        let mut grid = Grid::default();
        grid.scroll(5, 100, 4, 2);
        assert_eq!(grid.top_row, 5);
        assert_eq!(grid.focus / 4, 5);
        grid.scroll(1000, 100, 4, 2);
        assert_eq!(grid.top_row, 23);
        grid.scroll(-1000, 100, 4, 2);
        assert_eq!(grid.top_row, 0);
    }

    #[test]
    fn every_on_screen_key_is_reachable_with_the_d_pad() {
        let keys = search_keys();
        let mut grid = Grid::default();
        for _ in 0..10 {
            grid.apply(
                Command::Move(Direction::Down),
                keys.len(),
                SEARCH_COLUMNS,
                9,
            );
        }
        let right = Command::Move(Direction::Right);
        for key in [SearchKey::Space, SearchKey::Delete, SearchKey::Clear] {
            assert_eq!(keys[grid.focus], key);
            grid.apply(right, keys.len(), SEARCH_COLUMNS, 9);
        }
        assert_eq!(keys[grid.focus], SearchKey::Done);
    }
}
