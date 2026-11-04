//! Controller input for ordinary Blue pages (Controller Everywhere, slice 1).
//!
//! One layer, not per-page hacks: the Big Picture reader and `Navigator` feed
//! commands that become the keyboard actions every page already handles, so
//! egui's own focus (and the shell's focus ring and focus-reveal scrolling)
//! stay the single focus system, and the mouse and keyboard keep working.
//!
//! Safety: Select is only ever Enter on the control that visibly has focus.
//! Nothing here confirms, applies or deletes by itself, and Back is Escape,
//! which closes a confirmation without running it.

use super::big_picture::{
    Reader,
    nav::{Command, Navigator, PadEvent},
    page_command,
};
use super::osk::{self, Key as OskKey, Osk, Outcome};
use super::simple_shell::{DESTINATIONS, destination_for};
use super::*;

#[derive(Default)]
pub(super) struct PadShell {
    reader: Option<Reader>,
    nav: Navigator,
    started: bool,
    /// The on-screen keyboard, while a text field is being edited.
    osk: Option<Osk>,
    /// A field whose text is being read before the keyboard opens.
    pending: Option<(egui::Id, u8)>,
    /// The field to hand focus back to once the keyboard has closed.
    refocus: Option<(egui::Id, u8)>,
    #[cfg(test)]
    injected: Vec<PadEvent>,
}

/// The destination `step` places away from `current` (wrapping), or the first
/// when the page belongs to none.
pub(super) fn neighbour_destination(
    current: Option<simple_shell::Destination>,
    step: isize,
) -> simple_shell::Destination {
    let len = DESTINATIONS.len() as isize;
    let at = current
        .and_then(|c| DESTINATIONS.iter().position(|d| *d == c))
        .map_or(if step > 0 { -1 } else { 0 }, |i| i as isize);
    DESTINATIONS[(at + step).rem_euclid(len) as usize]
}

impl App {
    /// Called once per frame by the ordinary shell. Big Picture owns its own
    /// reader, so this one lives only while Big Picture is closed.
    pub(super) fn drive_pad_shell(&mut self, context: &egui::Context) {
        if self.big_picture.is_some() {
            self.pad_shell = None;
            return;
        }
        let shell = self.pad_shell.get_or_insert_with(Default::default);
        if !shell.started {
            shell.started = true;
            register_capture(context);
            shell.reader = (!cfg!(test)).then(|| Reader::start(context.clone()));
        }
        let (now, focused) = context.input(|i| (i.time, i.focused));
        // Unfocused (an emulator is running, say): read nothing and repaint
        // nothing, so a running game's input never reaches EmuWiz.
        if let Some(reader) = &shell.reader {
            reader.set_paused(!focused);
        }
        #[allow(unused_mut)]
        let mut events: Vec<PadEvent> = shell
            .reader
            .as_ref()
            .map(|r| r.drain().collect())
            .unwrap_or_default();
        #[cfg(test)]
        events.append(&mut shell.injected);
        let mut commands: Vec<Command> = events
            .into_iter()
            .filter_map(|e| shell.nav.feed(e, now, focused))
            .collect();
        commands.extend(shell.nav.tick(now, focused));
        if shell.nav.repeating() {
            context.request_repaint_after(std::time::Duration::from_millis(20));
        }
        if let Some(shell) = self.pad_shell.as_mut()
            && let Some((id, tries)) = shell.refocus
        {
            let held = context.memory(|m| m.focused()) == Some(id);
            shell.refocus = (!held && tries < 5).then_some((id, tries + 1));
            if !held {
                context.memory_mut(|m| m.request_focus(id));
                context.request_repaint();
            }
        }
        self.pad_shell_open_pending(context);
        let typing = self.pad_shell.as_ref().is_some_and(|s| s.osk.is_some());
        if typing {
            self.pad_shell_keyboard_guard(context);
        }
        for command in commands {
            if self.pad_shell.as_ref().is_some_and(|s| s.osk.is_some()) {
                self.pad_shell_osk_command(context, command);
            } else {
                self.pad_shell_command(context, command);
            }
        }
        self.pad_shell_draw_osk(context);
    }

    /// Once the field's text has been read, open the keyboard on it.
    fn pad_shell_open_pending(&mut self, context: &egui::Context) {
        let Some(shell) = self.pad_shell.as_mut() else {
            return;
        };
        let Some((target, waited)) = shell.pending else {
            return;
        };
        let Some(text) = context.data_mut(|d| d.get_temp::<String>(snapshot_slot())) else {
            // The read never came back: give up rather than open a keyboard
            // that could replace text it never saw.
            shell.pending = (waited < 3).then_some((target, waited + 1));
            context.request_repaint();
            return;
        };
        context.data_mut(|d| {
            d.remove::<String>(snapshot_slot());
            d.insert_temp(capture_slot(), false);
        });
        shell.pending = None;
        // The read selected everything; put the caret back at the end.
        if let Some(mut state) = egui::widgets::text_edit::TextEditState::load(context, target) {
            let end = egui::text::CCursor::new(text.chars().count());
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(context, target);
        }
        shell.osk = Some(Osk::new(target, text, false));
    }

    /// While the keyboard is open the page behind gets none of the keyboard's
    /// navigation keys; Escape cancels, Enter is Done, typing on a physical
    /// keyboard closes the on-screen one and goes straight to the field.
    fn pad_shell_keyboard_guard(&mut self, context: &egui::Context) {
        use egui::{Key, Modifiers};
        let (cancel, done, typed) = context.input_mut(|i| {
            let cancel = i.consume_key(Modifiers::NONE, Key::Escape);
            let done = i.consume_key(Modifiers::NONE, Key::Enter);
            for key in [
                Key::Tab,
                Key::ArrowUp,
                Key::ArrowDown,
                Key::ArrowLeft,
                Key::ArrowRight,
            ] {
                while i.consume_key(Modifiers::NONE, key) || i.consume_key(Modifiers::SHIFT, key) {}
            }
            let typed = i.events.iter().any(|e| matches!(e, egui::Event::Text(_)));
            (cancel, done, typed)
        });
        if typed {
            if let Some(shell) = self.pad_shell.as_mut() {
                shell.osk = None;
            }
        } else if cancel {
            self.pad_shell_osk_command(context, Command::Back);
        } else if done {
            self.pad_shell_osk_command(context, Command::Launch);
        }
    }

    fn pad_shell_osk_command(&mut self, context: &egui::Context, command: Command) {
        let Some(shell) = self.pad_shell.as_mut() else {
            return;
        };
        let Some(osk) = shell.osk.as_mut() else {
            return;
        };
        let target = osk.target;
        match osk.command(command) {
            Outcome::Open => {}
            Outcome::Cancelled => {
                shell.osk = None;
                shell.refocus = Some((target, 0));
            }
            Outcome::Done(text) => {
                shell.osk = None;
                shell.refocus = Some((target, 0));
                apply_text(context, target, &text);
            }
        }
    }

    fn pad_shell_draw_osk(&mut self, context: &egui::Context) {
        use crate::ui::theme::blue;
        let Some(osk) = self.pad_shell.as_mut().and_then(|s| s.osk.as_mut()) else {
            return;
        };
        let mut clicked = None;
        let modal = egui::Modal::new(egui::Id::new("pad_shell_osk")).show(context, |ui| {
            ui.set_width(760.0);
            ui.label(
                egui::RichText::new("Type with the controller")
                    .strong()
                    .size(20.0),
            );
            let mut text = osk.shown();
            text.push('_');
            ui.label(
                egui::RichText::new(text)
                    .size(24.0)
                    .color(blue::PRIMARY_TEXT),
            );
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            let all = osk::keys(osk.layer);
            for (row, chunk) in all.chunks(osk::COLUMNS).enumerate() {
                ui.horizontal(|ui| {
                    for (column, key) in chunk.iter().enumerate() {
                        let index = row * osk::COLUMNS + column;
                        if *key == OskKey::Blank {
                            ui.add_space(52.0);
                            continue;
                        }
                        let wide = row >= 4;
                        let mut button =
                            egui::Button::new(egui::RichText::new(key.label()).size(20.0).strong())
                                .min_size(egui::vec2(if wide { 100.0 } else { 52.0 }, 48.0))
                                .sense(egui::Sense::CLICK);
                        if index == osk.grid.focus {
                            button = button
                                .fill(blue::PRIMARY_ACTION)
                                .stroke(egui::Stroke::new(3.0_f32, blue::ACCENT));
                        }
                        if ui.add(button).clicked() {
                            clicked = Some(index);
                        }
                    }
                });
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("A press key   X delete   Y shift   Start done   B cancel")
                    .color(blue::SECONDARY_TEXT),
            );
        });
        let backdrop = modal.should_close();
        let outcome = match clicked {
            Some(index) => osk.click(index),
            None if backdrop => Outcome::Cancelled,
            None => Outcome::Open,
        };
        let target = osk.target;
        match outcome {
            Outcome::Open => {}
            Outcome::Cancelled => {
                let shell = self.pad_shell.as_mut().unwrap();
                shell.osk = None;
                shell.refocus = Some((target, 0));
            }
            Outcome::Done(text) => {
                let shell = self.pad_shell.as_mut().unwrap();
                shell.osk = None;
                shell.refocus = Some((target, 0));
                apply_text(context, target, &text);
            }
        }
    }

    /// Select on a focused text field opens the keyboard; anywhere else it
    /// presses the focused control.
    fn pad_shell_select(&mut self, context: &egui::Context) {
        use egui::widgets::text_edit::TextEditState;
        let field = context
            .memory(|m| m.focused())
            .filter(|id| TextEditState::load(context, *id).is_some());
        let Some(id) = field else {
            page_command(context, Command::Select);
            return;
        };
        if is_secret(context, id) {
            if let Some(shell) = self.pad_shell.as_mut() {
                shell.osk = Some(Osk::new(id, String::new(), true));
            }
            return;
        }
        context.data_mut(|d| {
            d.remove::<String>(snapshot_slot());
            d.insert_temp(capture_slot(), true);
        });
        // Select all, then a copy that the end-of-pass hook reads and strips
        // before it can reach the clipboard.
        context.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            });
            i.events.push(egui::Event::Copy);
        });
        if let Some(shell) = self.pad_shell.as_mut() {
            shell.pending = Some((id, 0));
        }
        context.request_repaint();
    }

    fn pad_shell_command(&mut self, context: &egui::Context, command: Command) {
        match command {
            Command::Select => self.pad_shell_select(context),
            Command::Move(_) => page_command(context, command),
            Command::Back => key_tap(context, egui::Key::Escape),
            Command::PageUp => key_tap(context, egui::Key::PageUp),
            Command::PageDown => key_tap(context, egui::Key::PageDown),
            Command::First => key_tap(context, egui::Key::Home),
            Command::Last => key_tap(context, egui::Key::End),
            Command::Platforms => self.pad_shell_section(1),
            Command::Search => self.pad_shell_section(-1),
            Command::Launch => {
                self.enter_big_picture(context);
                // Big Picture starts its own reader; release this one now.
                self.pad_shell = None;
            }
        }
    }

    /// Previous or next Blue destination. Tools opens the All Tools browser,
    /// which is where its tiles live.
    fn pad_shell_section(&mut self, step: isize) {
        let at = if self.simple.all_tools_open() {
            Some(simple_shell::Destination::Tools)
        } else {
            destination_for(&self.router.current)
        };
        let next = neighbour_destination(at, step);
        match next.route() {
            Some(route) => self.go(route),
            None => self.open_all_tools(),
        }
    }
}

fn snapshot_slot() -> egui::Id {
    egui::Id::new("pad_shell_snapshot")
}
fn capture_slot() -> egui::Id {
    egui::Id::new("pad_shell_capture")
}

/// Mark a text field as a secret (a password or token): the controller
/// keyboard masks it, starts it empty and never reads it back. Call every
/// frame the field is drawn.
pub(crate) fn mark_secret(response: &egui::Response) {
    response
        .ctx
        .data_mut(|d| d.insert_temp(egui::Id::new("pad_secret").with(response.id), true));
}

fn is_secret(context: &egui::Context, id: egui::Id) -> bool {
    context
        .data(|d| d.get_temp::<bool>(egui::Id::new("pad_secret").with(id)))
        .unwrap_or(false)
}

/// Once per context: at the end of a pass that asked to read a field, take the
/// copied text and remove the copy command so the clipboard is never touched.
fn register_capture(context: &egui::Context) {
    let slot = egui::Id::new("pad_shell_hook");
    if context.data(|d| d.get_temp::<bool>(slot)).unwrap_or(false) {
        return;
    }
    context.data_mut(|d| d.insert_temp(slot, true));
    context.on_end_pass(
        "pad_shell_capture",
        std::sync::Arc::new(|ui: &mut egui::Ui| {
            let context = &ui.ctx().clone();
            if !context
                .data(|d| d.get_temp::<bool>(capture_slot()))
                .unwrap_or(false)
            {
                return;
            }
            let mut copied = None;
            context.output_mut(|o| {
                o.commands.retain(|c| match c {
                    egui::OutputCommand::CopyText(text) => {
                        copied = Some(text.clone());
                        false
                    }
                    _ => true,
                });
            });
            // Nothing copied means the field is empty (or not copyable).
            let text = copied.unwrap_or_default();
            context.data_mut(|d| d.insert_temp(snapshot_slot(), text));
        }),
    );
}

/// Replace the whole field with `text`, exactly as if it had been typed.
fn apply_text(context: &egui::Context, target: egui::Id, text: &str) {
    context.memory_mut(|m| m.request_focus(target));
    context.input_mut(|i| {
        i.events.push(egui::Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::A),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        if text.is_empty() {
            for pressed in [true, false] {
                i.events.push(egui::Event::Key {
                    key: egui::Key::Delete,
                    physical_key: Some(egui::Key::Delete),
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        } else {
            i.events.push(egui::Event::Text(text.to_owned()));
        }
    });
}

fn key_tap(context: &egui::Context, key: egui::Key) {
    context.input_mut(|input| {
        for pressed in [true, false] {
            input.events.push(egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
    });
}

#[cfg(test)]
impl PadShell {
    pub(super) fn osk_open(&self) -> bool {
        self.osk.is_some()
    }
    pub(super) fn inject(&mut self, event: PadEvent) {
        self.injected.push(event);
    }
}
