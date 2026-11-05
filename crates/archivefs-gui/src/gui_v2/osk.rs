//! Controller text entry for ordinary Blue fields (Controller Everywhere,
//! slice 2). The keyboard edits a staged copy: the field itself is untouched
//! until Done, so Cancel can never change it. Pure model here; `pad_shell`
//! owns drawing and applying the result.

use super::big_picture::nav::{Command, Direction, Grid};

pub(super) const COLUMNS: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Layer {
    Lower,
    Upper,
    Symbols,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Key {
    Char(char),
    Blank,
    Shift,
    Symbols,
    Space,
    Delete,
    Clear,
    Cancel,
    Done,
}

impl Key {
    pub(super) fn label(self) -> String {
        match self {
            Self::Char(c) => c.to_string(),
            Self::Blank => String::new(),
            Self::Shift => "Shift".into(),
            Self::Symbols => "123/#+=".into(),
            Self::Space => "Space".into(),
            Self::Delete => "Delete".into(),
            Self::Clear => "Clear".into(),
            Self::Cancel => "Cancel".into(),
            Self::Done => "Done".into(),
        }
    }
}

fn rows(layer: Layer) -> [&'static str; 4] {
    match layer {
        Layer::Lower => ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm,.-"],
        Layer::Upper => ["1234567890", "QWERTYUIOP", "ASDFGHJKL'", "ZXCVBNM,.-"],
        Layer::Symbols => ["!\"#$%&'()*", "+,-./:;<=>", "?@[\\]^_`{|", "}~"],
    }
}

/// Every key of a layer, in grid order: four rows of ten (short rows padded
/// with blanks), then the control row.
pub(super) fn keys(layer: Layer) -> Vec<Key> {
    let mut out = Vec::new();
    for row in rows(layer) {
        let mut count = 0;
        for c in row.chars() {
            out.push(Key::Char(c));
            count += 1;
        }
        out.extend(std::iter::repeat_n(Key::Blank, COLUMNS - count));
    }
    out.extend([
        Key::Shift,
        Key::Symbols,
        Key::Space,
        Key::Delete,
        Key::Clear,
        Key::Cancel,
        Key::Done,
    ]);
    out
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Outcome {
    Open,
    /// Apply this text to the field.
    Done(String),
    /// Leave the field exactly as it was.
    Cancelled,
}

pub(super) struct Osk {
    pub(super) target: eframe::egui::Id,
    /// A secret is shown masked, starts empty and is never read back.
    pub(super) secret: bool,
    pub(super) buffer: String,
    pub(super) layer: Layer,
    pub(super) grid: Grid,
    /// This field offers title suggestions (a Library search).
    pub(super) suggest: bool,
    /// Suggestions for the current text, and the one focused (None = keys).
    pub(super) suggestions: Vec<String>,
    pub(super) suggestion_focus: Option<usize>,
    /// The suggestion list is still being prepared.
    pub(super) loading: bool,
    /// The text the suggestions were made for.
    pub(super) suggested_for: Option<String>,
}

impl Osk {
    pub(super) fn new(target: eframe::egui::Id, initial: String, secret: bool) -> Self {
        Self {
            target,
            secret,
            buffer: if secret { String::new() } else { initial },
            layer: Layer::Lower,
            // Start on Done's neighbour-free home key, the first letter.
            grid: Grid {
                focus: COLUMNS,
                top_row: 0,
            },
            suggest: false,
            suggestions: Vec::new(),
            suggestion_focus: None,
            loading: false,
            suggested_for: None,
        }
    }

    pub(super) fn shown(&self) -> String {
        if self.secret {
            "•".repeat(self.buffer.chars().count())
        } else {
            self.buffer.clone()
        }
    }

    fn press(&mut self, key: Key) -> Outcome {
        match key {
            Key::Char(c) => self.buffer.push(c),
            Key::Blank => {}
            Key::Space => self.buffer.push(' '),
            Key::Delete => {
                self.buffer.pop();
            }
            Key::Clear => self.buffer.clear(),
            Key::Shift => {
                self.layer = if self.layer == Layer::Upper {
                    Layer::Lower
                } else {
                    Layer::Upper
                }
            }
            Key::Symbols => {
                self.layer = if self.layer == Layer::Symbols {
                    Layer::Lower
                } else {
                    Layer::Symbols
                }
            }
            Key::Cancel => return Outcome::Cancelled,
            // An empty secret would wipe a saved password by accident.
            Key::Done if self.secret && self.buffer.is_empty() => return Outcome::Cancelled,
            Key::Done => return Outcome::Done(self.buffer.clone()),
        }
        Outcome::Open
    }

    /// Controller commands. B cancels, X deletes, Y shifts, Start is Done.
    pub(super) fn command(&mut self, command: Command) -> Outcome {
        let all = keys(self.layer);
        // On the suggestion row, left/right choose, down returns to the keys,
        // A takes the suggestion; the other buttons keep their meaning.
        if let Some(at) = self.suggestion_focus {
            match command {
                Command::Move(Direction::Left) => {
                    self.suggestion_focus = Some(at.saturating_sub(1));
                    return Outcome::Open;
                }
                Command::Move(Direction::Right) => {
                    self.suggestion_focus =
                        Some((at + 1).min(self.suggestions.len().saturating_sub(1)));
                    return Outcome::Open;
                }
                Command::Move(Direction::Down) => {
                    self.suggestion_focus = None;
                    return Outcome::Open;
                }
                Command::Move(Direction::Up) => return Outcome::Open,
                Command::Select => {
                    self.take_suggestion(at);
                    return Outcome::Open;
                }
                _ => {}
            }
        }
        match command {
            Command::Move(Direction::Up)
                if self.suggest && self.grid.focus < COLUMNS && !self.suggestions.is_empty() =>
            {
                self.suggestion_focus = Some(0);
                Outcome::Open
            }
            Command::Select => {
                let key = all[self.grid.focus.min(all.len() - 1)];
                self.press(key)
            }
            Command::Back => Outcome::Cancelled,
            Command::Search => self.press(Key::Delete),
            Command::Platforms => self.press(Key::Shift),
            Command::Launch => self.press(Key::Done),
            Command::Move(d) => {
                self.grid
                    .apply(Command::Move(d), all.len(), COLUMNS, all.len());
                // Blank padding is never a resting place.
                let mut guard = 0;
                while all[self.grid.focus] == Key::Blank && guard < COLUMNS {
                    let before = self.grid.focus;
                    let back = match d {
                        Direction::Right => Direction::Left,
                        Direction::Left => Direction::Right,
                        other => other,
                    };
                    self.grid
                        .apply(Command::Move(back), all.len(), COLUMNS, all.len());
                    if self.grid.focus == before {
                        break;
                    }
                    guard += 1;
                }
                Outcome::Open
            }
            _ => Outcome::Open,
        }
    }

    /// Replace the text with a suggested title; the keys take over again.
    pub(super) fn take_suggestion(&mut self, at: usize) {
        if let Some(title) = self.suggestions.get(at).cloned() {
            self.buffer = title;
        }
        self.suggestion_focus = None;
    }

    /// A mouse click on a suggestion.
    pub(super) fn click_suggestion(&mut self, at: usize) {
        self.take_suggestion(at);
    }

    /// A mouse click on a key.
    pub(super) fn click(&mut self, index: usize) -> Outcome {
        self.grid.focus = index;
        self.command(Command::Select)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Id;

    fn osk(initial: &str, secret: bool) -> Osk {
        Osk::new(Id::new("f"), initial.into(), secret)
    }
    fn index_of(layer: Layer, key: Key) -> usize {
        keys(layer).iter().position(|k| *k == key).unwrap()
    }

    #[test]
    fn typing_edits_a_copy_and_cancel_returns_nothing_to_apply() {
        let mut o = osk("abc", false);
        assert_eq!(
            o.click(index_of(Layer::Lower, Key::Char('q'))),
            Outcome::Open
        );
        assert_eq!(o.buffer, "abcq");
        assert_eq!(o.command(Command::Search), Outcome::Open);
        assert_eq!(o.buffer, "abc");
        assert_eq!(o.command(Command::Back), Outcome::Cancelled);
    }

    #[test]
    fn done_returns_staged_text_and_layers_change_keys() {
        let mut o = osk("", false);
        o.command(Command::Platforms);
        assert_eq!(o.layer, Layer::Upper);
        o.click(index_of(Layer::Upper, Key::Char('Q')));
        o.click(index_of(Layer::Upper, Key::Symbols));
        assert_eq!(o.layer, Layer::Symbols);
        o.click(index_of(Layer::Symbols, Key::Char('/')));
        assert_eq!(o.command(Command::Launch), Outcome::Done("Q/".into()));
    }

    #[test]
    fn secrets_start_empty_mask_and_never_wipe_on_empty_done() {
        let mut o = osk("hunter2", true);
        assert_eq!(o.buffer, "");
        o.click(index_of(Layer::Lower, Key::Char('a')));
        assert_eq!(o.shown(), "•");
        let mut empty = osk("old", true);
        assert_eq!(empty.command(Command::Launch), Outcome::Cancelled);
    }

    #[test]
    fn focus_never_rests_on_blank_padding() {
        let mut o = osk("", false);
        o.layer = Layer::Symbols;
        o.grid.focus = 3 * COLUMNS - 1; // end of the third symbol row
        o.command(Command::Move(Direction::Down)); // onto the short row
        assert_ne!(keys(Layer::Symbols)[o.grid.focus], Key::Blank);
    }

    #[test]
    fn up_from_the_top_row_reaches_suggestions_and_a_takes_one() {
        let mut o = osk("al", false);
        o.suggest = true;
        o.suggestions = vec!["Alpha Quest".into(), "Alien Run".into()];
        o.grid.focus = 3; // top row of keys
        assert_eq!(o.command(Command::Move(Direction::Up)), Outcome::Open);
        assert_eq!(o.suggestion_focus, Some(0));
        o.command(Command::Move(Direction::Right));
        o.command(Command::Move(Direction::Right)); // clamps at the last one
        assert_eq!(o.suggestion_focus, Some(1));
        assert_eq!(o.command(Command::Select), Outcome::Open);
        assert_eq!(o.buffer, "Alien Run", "A replaced the text with the title");
        assert_eq!(o.suggestion_focus, None, "keys take over again");
        assert_eq!(
            o.command(Command::Launch),
            Outcome::Done("Alien Run".into())
        );
    }

    #[test]
    fn down_returns_to_the_keys_and_other_buttons_keep_their_meaning() {
        let mut o = osk("ab", false);
        o.suggest = true;
        o.suggestions = vec!["Abc".into()];
        o.grid.focus = 0;
        o.command(Command::Move(Direction::Up));
        o.command(Command::Search); // X still deletes while on suggestions
        assert_eq!(o.buffer, "a");
        assert_eq!(o.command(Command::Move(Direction::Down)), Outcome::Open);
        assert_eq!(o.suggestion_focus, None);
        o.suggestion_focus = Some(0);
        assert_eq!(
            o.command(Command::Back),
            Outcome::Cancelled,
            "B still cancels"
        );
    }

    #[test]
    fn fields_without_suggestions_never_enter_the_row() {
        let mut o = osk("a", false);
        o.suggestions = vec!["Abc".into()];
        o.grid.focus = 0;
        o.command(Command::Move(Direction::Up));
        assert_eq!(o.suggestion_focus, None);
    }
}
