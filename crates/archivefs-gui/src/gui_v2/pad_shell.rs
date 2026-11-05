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
use super::simple_shell::{DESTINATIONS, destination_for};
use super::*;

#[derive(Default)]
pub(super) struct PadShell {
    reader: Option<Reader>,
    nav: Navigator,
    started: bool,
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
        for command in commands {
            self.pad_shell_command(context, command);
        }
    }

    fn pad_shell_command(&mut self, context: &egui::Context, command: Command) {
        match command {
            Command::Move(_) | Command::Select => page_command(context, command),
            Command::Back => key_tap(context, egui::Key::Escape),
            Command::PageUp => key_tap(context, egui::Key::PageUp),
            Command::PageDown => key_tap(context, egui::Key::PageDown),
            Command::First => key_tap(context, egui::Key::Home),
            Command::Last => key_tap(context, egui::Key::End),
            Command::Platforms => self.pad_shell_section(1),
            Command::Launch => {
                self.enter_big_picture(context);
                // Big Picture starts its own reader; release this one now.
                self.pad_shell = None;
            }
            // Text entry (on-screen keyboard) arrives in a later slice.
            Command::Search => {}
        }
    }

    /// Next Blue destination, skipping Tools (it has no page of its own).
    fn pad_shell_section(&mut self, step: isize) {
        let mut at = destination_for(&self.router.current);
        for _ in 0..DESTINATIONS.len() {
            let next = neighbour_destination(at, step);
            if let Some(route) = next.route() {
                self.go(route);
                return;
            }
            at = Some(next);
        }
    }
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
    pub(super) fn inject(&mut self, event: PadEvent) {
        self.injected.push(event);
    }
}
