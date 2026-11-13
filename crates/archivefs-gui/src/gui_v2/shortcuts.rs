//! Blue's shared keyboard shortcuts: one place that decides what a key means,
//! so pages opt in instead of each reading keys on its own.
//!
//! | Key                          | Meaning                                       |
//! |------------------------------|-----------------------------------------------|
//! | `/`, Ctrl+F                  | Focus this page's search field                |
//! | Escape                       | Close the open dialog (as Cancel), else leave |
//! |                              | the focused field, else Back                  |
//! | Alt+Left                     | Back                                          |
//! | Backspace                    | Back, only while no text field has focus      |
//! | Enter, Space                 | Activate the focused control (never held)     |
//! | Tab, Shift+Tab               | Move focus                                    |
//!
//! Escape and Alt+Left are still read by `App::show` (pages.rs) with the same
//! precedence; the keys here run first and never act while a text field has
//! focus, a Blue confirmation is open, a menu or another window is open, or a
//! full-screen viewer, Big Picture or the controller keyboard owns the keys.
//!
//! No key here launches a game or answers a confirmation. Enter and Space only
//! do what egui gives every focused control (a click), and their auto-repeat is
//! dropped, so holding Enter cannot run on from a card into Play and Launch.
//!
//! Pages opt in:
//! - a search field calls [`register_search`] each frame it is drawn (the title
//!   searches already do, through `pad_shell::mark_suggestions`);
//! - page-level keys (for example a list's Ctrl+A) check
//!   [`page_keys_allowed`] first;
//! - a page with its own Escape meaning (for example clearing a selection)
//!   calls [`claim_escape`] each frame it has something to clear, and reads
//!   [`claimed_escape_pressed`]; Back then waits for the next Escape.

use super::App;

/// The keys of one frame this module resolves. Enter is deliberately absent:
/// it is never a shortcut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Keys {
    pub focus_search: bool,
    pub backspace: bool,
    pub escape: bool,
}

impl Keys {
    pub(super) fn read(input: &egui::InputState) -> Self {
        let plain =
            |modifiers: &egui::Modifiers| !modifiers.ctrl && !modifiers.alt && !modifiers.command;
        let mut keys = Self::default();
        for event in &input.events {
            match event {
                egui::Event::Text(text) if text == "/" && plain(&input.modifiers) => {
                    keys.focus_search = true;
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => match key {
                    egui::Key::Slash if plain(modifiers) && !modifiers.shift => {
                        keys.focus_search = true;
                    }
                    egui::Key::F if modifiers.command_only() => keys.focus_search = true,
                    egui::Key::Backspace if modifiers.is_none() => keys.backspace = true,
                    egui::Key::Escape if modifiers.is_none() => keys.escape = true,
                    _ => {}
                },
                _ => {}
            }
        }
        keys
    }
}

/// Who else may own the keyboard this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Gate {
    /// A text field has focus: its keys are its own (Escape leaves it).
    pub typing: bool,
    /// A Blue confirmation dialog is open: only its own buttons answer it.
    pub confirmation: bool,
    /// An egui menu, combo box or other popup is open.
    pub popup: bool,
    /// Another window (a classic dialog hosted in Blue) is open.
    pub window: bool,
    /// Big Picture, a full-screen viewer or the controller keyboard is shown.
    pub overlay: bool,
}

impl Gate {
    /// True when page-level shortcuts may act.
    pub(super) fn free(self) -> bool {
        !(self.typing || self.confirmation || self.popup || self.window || self.overlay)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shortcut {
    /// Move keyboard focus to the page's search field.
    FocusSearch(egui::Id),
    /// The existing Back route behaviour.
    Back,
    /// A page claimed Escape for its own meaning; Back does not run.
    ClaimedEscape,
}

/// What this frame's keys mean. Pure, so the precedence is unit tested.
pub(super) fn resolve(
    keys: Keys,
    gate: Gate,
    search: Option<egui::Id>,
    escape_claimed: bool,
) -> Vec<Shortcut> {
    let mut out = Vec::new();
    if !gate.free() {
        return out;
    }
    if keys.escape && escape_claimed {
        out.push(Shortcut::ClaimedEscape);
    }
    if keys.focus_search
        && let Some(search) = search
    {
        out.push(Shortcut::FocusSearch(search));
    }
    if keys.backspace {
        out.push(Shortcut::Back);
    }
    out
}

fn search_slot() -> egui::Id {
    egui::Id::new("v2_shortcut_search_field")
}

fn escape_claim_slot() -> egui::Id {
    egui::Id::new("v2_shortcut_escape_claim")
}

fn escape_claimed_slot() -> egui::Id {
    egui::Id::new("v2_shortcut_escape_claimed")
}

fn gate_slot() -> egui::Id {
    egui::Id::new("v2_shortcut_gate")
}

/// Mark `response` as this page's search field, for `/` and Ctrl+F. Call each
/// frame it is drawn. The last one drawn wins, so a page's own search, drawn
/// after the Blue header's library search, is the one focused.
pub(super) fn register_search(response: &egui::Response) {
    let id = response.id;
    response
        .ctx
        .data_mut(|data| data.insert_temp(search_slot(), id));
}

/// True when a page may act on its own keys this frame: no text field has
/// focus, and no confirmation, menu, window or overlay owns the keyboard.
pub(super) fn page_keys_allowed(context: &egui::Context) -> bool {
    let gate = context
        .data(|data| data.get_temp::<Gate>(gate_slot()))
        .unwrap_or_else(|| live_gate(context));
    // A field focused earlier in this frame is typing too.
    gate.free() && !context.text_edit_focused()
}

/// A page with its own Escape meaning calls this each frame it has something
/// for Escape to do; the next Escape is then the page's, not Back.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "hook for the Library multi-select (Batch 3 Job 10); remove once it calls this"
    )
)]
pub(super) fn claim_escape(context: &egui::Context) {
    context.data_mut(|data| data.insert_temp(escape_claim_slot(), true));
}

/// True on the frame Escape was pressed while this page held its claim.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "hook for the Library multi-select (Batch 3 Job 10); remove once it calls this"
    )
)]
pub(super) fn claimed_escape_pressed(context: &egui::Context) -> bool {
    context
        .data(|data| data.get_temp::<bool>(escape_claimed_slot()))
        .unwrap_or(false)
}

/// The gate as egui alone sees it (no Blue confirmation state).
fn live_gate(context: &egui::Context) -> Gate {
    Gate {
        typing: context.text_edit_focused(),
        popup: egui::Popup::is_any_open(context),
        window: other_window_open(context),
        ..Gate::default()
    }
}

/// Any other open window (the classic Sources dialogs are hosted in Blue too).
fn other_window_open(context: &egui::Context) -> bool {
    context.memory(|memory| {
        memory
            .areas()
            .visible_layer_ids()
            .iter()
            .any(|layer| layer.order == egui::Order::Middle)
    })
}

fn drop_held_activation(input: &mut egui::InputState) {
    input.events.retain(|event| {
        !matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Enter | egui::Key::Space,
                repeat: true,
                ..
            }
        )
    });
}

fn drop_search_keys(input: &mut egui::InputState) {
    input.events.retain(|event| match event {
        egui::Event::Text(text) => text != "/",
        egui::Event::Key { key, modifiers, .. } => !matches!(
            (key, modifiers.command_only()),
            (egui::Key::Slash, _) | (egui::Key::F, true)
        ),
        _ => true,
    });
}

impl App {
    /// One Blue frame: the shared shortcuts, then the page.
    pub(super) fn show_with_shortcuts(&mut self, context: &egui::Context) {
        self.apply_shortcuts(context);
        self.show(context);
    }

    /// A Blue confirmation dialog is open. Mirrors the list `App::show` uses
    /// for Escape, so Backspace and page keys never act underneath one.
    pub(super) fn blue_confirmation_open(&self) -> bool {
        self.confirm_scan
            || self.setup_actions.confirm.is_some()
            || self.repair_confirm
            || self.undo_confirm.is_some()
            || self.history_view.confirm_undo.is_some()
            || self.missing.confirm
            || self.equiv.confirm
    }

    fn shortcut_overlay(&self) -> bool {
        self.big_picture.is_some()
            || self.manual_viewer.is_open()
            || self.text_guide_viewer.is_open()
            || self.controller_keyboard_open()
    }

    fn apply_shortcuts(&mut self, context: &egui::Context) {
        // Registrations are per frame: what the last frame drew.
        let search = context.data_mut(|data| data.remove_temp::<egui::Id>(search_slot()));
        let escape_claimed = context
            .data_mut(|data| data.remove_temp::<bool>(escape_claim_slot()))
            .unwrap_or(false);
        context.data_mut(|data| data.remove::<bool>(escape_claimed_slot()));
        let gate = Gate {
            typing: self.text_was_focused || context.text_edit_focused(),
            confirmation: self.blue_confirmation_open(),
            overlay: self.shortcut_overlay(),
            ..live_gate(context)
        };
        context.data_mut(|data| data.insert_temp(gate_slot(), gate));
        if gate.overlay {
            // Big Picture and the viewers keep their own keys untouched.
            return;
        }
        if !gate.typing {
            context.input_mut(drop_held_activation);
        }
        let keys = context.input(Keys::read);
        for shortcut in resolve(keys, gate, search, escape_claimed) {
            match shortcut {
                Shortcut::ClaimedEscape => {
                    context.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                    });
                    context.data_mut(|data| data.insert_temp(escape_claimed_slot(), true));
                }
                Shortcut::FocusSearch(id) => {
                    // The `/` must not be typed into the field it just focused.
                    context.input_mut(drop_search_keys);
                    context.memory_mut(|memory| memory.request_focus(id));
                }
                Shortcut::Back => {
                    context.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace);
                    });
                    self.back();
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/keyboard_shortcuts.rs"]
mod tests;
