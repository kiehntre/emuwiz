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
    nav::{Button, Command, Control, Direction, Navigator, PadEvent},
    page_command,
};
use super::osk::{self, Key as OskKey, Osk, Outcome};
use super::pad_mouse;
use super::simple_shell::{DESTINATIONS, destination_for};
use super::suggest;
use super::*;
use std::sync::{Arc, Mutex};

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
    /// Whether A (South) is physically down, for hold-to-confirm.
    south_down: bool,
    /// The last input was the controller, not the mouse: only then is an open
    /// dialog made modal for focus, so mouse use behind it is unchanged.
    pad_mode: bool,
    /// Test oracle: scan controls in reading order, the only moves egui names.
    #[cfg(test)]
    pub(super) reading_order: bool,
    /// The last focused control in the page content (not the sidebar), so
    /// Right from the sidebar can return to it.
    last_content: Option<egui::Id>,
    /// An Up/Down that may have found nothing spatially: (direction, focus
    /// before, frames waited). Falls back to reading order so lists that
    /// scroll past the visible controls still move.
    pending_move: Option<(Direction, Option<egui::Id>, u8, bool)>,
    /// The sidebar row of the page being shown: where Left lands when nothing
    /// in the content is further left.
    current_nav: Option<egui::Id>,
    /// B was pressed over this dialog layer: if it is still open after a few
    /// frames, Back did not close it (a window EmuWiz has no close rule for).
    back_pending: Option<(egui::LayerId, u8)>,
    /// A dialog the controller was let out of with B, so it can never trap.
    released: Option<egui::LayerId>,
    /// A hold-to-confirm in progress: the control, when it began, its label.
    hold: Option<(egui::Id, f64, String)>,
    /// The R3 virtual mouse.
    mouse: pad_mouse::VirtualMouse,
    #[cfg(test)]
    injected: Vec<PadEvent>,
}

impl PadShell {
    /// The virtual mouse's share of this frame's pad events: R3 toggles it, the
    /// right stick steers it, and while it is on A, X and B belong to it and
    /// never reach the navigator (so a click is not also a press). Everything
    /// else, and everything when it is off, passes through untouched.
    fn mouse_filter(
        &mut self,
        context: &egui::Context,
        events: Vec<PadEvent>,
        now: f64,
        focused: bool,
    ) -> Vec<PadEvent> {
        let mut kept = Vec::new();
        for event in events {
            match event {
                PadEvent::RightStick { x, y, .. } => self.mouse.set_stick(x, y),
                PadEvent::Pressed {
                    control: Control::Button(Button::RightStick),
                    ..
                } => self.toggle_mouse(context),
                PadEvent::Released {
                    control: Control::Button(Button::RightStick),
                    ..
                } => {}
                PadEvent::Pressed {
                    control: Control::Button(button @ (Button::South | Button::West | Button::East)),
                    ..
                } if self.mouse.on => {
                    self.mouse.swallowed.push(button);
                    match button {
                        Button::South => {
                            self.mouse_button(context, egui::PointerButton::Primary, true)
                        }
                        Button::West => {
                            self.mouse_button(context, egui::PointerButton::Secondary, true)
                        }
                        _ => self.exit_mouse(context),
                    }
                }
                PadEvent::Released {
                    control: Control::Button(button),
                    ..
                } if self.mouse.swallowed.contains(&button) => {
                    self.mouse.swallowed.retain(|b| *b != button);
                    match button {
                        Button::South => {
                            self.mouse_button(context, egui::PointerButton::Primary, false)
                        }
                        Button::West => {
                            self.mouse_button(context, egui::PointerButton::Secondary, false)
                        }
                        _ => {}
                    }
                }
                PadEvent::Reset { .. } => {
                    self.exit_mouse(context);
                    kept.push(event);
                }
                other => kept.push(other),
            }
        }
        if self.mouse.on && !focused {
            // An emulator took the screen: leave the mode rather than hold a
            // pointer (or a button) across it.
            self.exit_mouse(context);
        }
        if self.mouse.on {
            if let Some(pos) = self.mouse.step(now, context.content_rect()) {
                pad_mouse::queue(context, [egui::Event::PointerMoved(pos)]);
            }
            pad_mouse::paint(context, self.mouse.pos);
            show_mouse_hint(context);
            if self.mouse.moving() {
                context.request_repaint_after(std::time::Duration::from_millis(8));
            }
        } else {
            // Keep the clock honest for the next time it is switched on.
            let _ = self.mouse.step(now, context.content_rect());
        }
        if let Some(reader) = &self.reader {
            reader.set_analog(self.mouse.on);
        }
        kept
    }

    fn toggle_mouse(&mut self, context: &egui::Context) {
        if self.mouse.on {
            self.exit_mouse(context);
        } else {
            self.mouse.on = true;
            self.mouse.reset_motion();
            self.mouse.pos = context
                .pointer_latest_pos()
                .unwrap_or_else(|| context.content_rect().center());
            pad_mouse::queue(context, [egui::Event::PointerMoved(self.mouse.pos)]);
            context.request_repaint();
        }
    }

    fn exit_mouse(&mut self, context: &egui::Context) {
        if !self.mouse.on {
            return;
        }
        self.mouse_button(context, egui::PointerButton::Primary, false);
        self.mouse_button(context, egui::PointerButton::Secondary, false);
        self.mouse.on = false;
        self.mouse.reset_motion();
        context.request_repaint();
    }

    fn mouse_button(
        &mut self,
        context: &egui::Context,
        button: egui::PointerButton,
        pressed: bool,
    ) {
        let held = match button {
            egui::PointerButton::Secondary => &mut self.mouse.secondary_down,
            _ => &mut self.mouse.primary_down,
        };
        if *held == pressed {
            return; // never a release without a press, or a double press
        }
        *held = pressed;
        pad_mouse::queue(
            context,
            [
                egui::Event::PointerMoved(self.mouse.pos),
                egui::Event::PointerButton {
                    pos: self.mouse.pos,
                    button,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        context.request_repaint();
    }
}

fn show_mouse_hint(context: &egui::Context) {
    egui::Area::new(egui::Id::new("pad_mouse_hint"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(16.0, -16.0))
        .show(context, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label("Mouse mode: right stick moves · A click · X right-click · B exit");
            });
        });
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
        let current_nav = destination_for(&self.router.current).map(simple_shell::nav_id);
        let shell = self.pad_shell.get_or_insert_with(Default::default);
        shell.current_nav = current_nav;
        if !shell.started {
            shell.started = true;
            register_capture(context);
            pad_mouse::register_injector(context);
            shell.reader = (!cfg!(test)).then(|| Reader::start(context.clone()));
        }
        let (now, focused) = context.input(|i| (i.time, i.focused));
        if let Some(reader) = &shell.reader {
            crate::ui::picker::set_controller_in_use(context, reader.status().connected());
        }
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
        let events = shell.mouse_filter(context, events, now, focused);
        let pointer_used = context.input(|i| {
            i.pointer.is_moving()
                || i.pointer.any_pressed()
                || i.smooth_scroll_delta != egui::Vec2::ZERO
        });
        if pointer_used {
            shell.pad_mode = false;
        }
        if events
            .iter()
            .any(|e| !matches!(e, PadEvent::Reset { .. } | PadEvent::Released { .. }))
        {
            shell.pad_mode = true;
        }
        track_focus_region(context, shell);
        if shell.pad_mode {
            confine_to_dialog(context, shell);
        }
        for event in &events {
            match event {
                PadEvent::Pressed {
                    control: Control::Button(Button::South),
                    ..
                } => shell.south_down = true,
                PadEvent::Released {
                    control: Control::Button(Button::South),
                    ..
                }
                | PadEvent::Reset { .. } => shell.south_down = false,
                _ => {}
            }
        }
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
        self.pad_shell_hold_tick(context, now);
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
        let mut osk = Osk::new(target, text, false);
        osk.suggest = wants_suggestions(context, target);
        shell.osk = Some(osk);
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
            if let Some(shell) = self.pad_shell.as_mut()
                && let Some(osk) = shell.osk.take()
            {
                // The typed text is still in this frame's input: give the
                // field focus before it is drawn so it lands there.
                context.memory_mut(|m| m.request_focus(osk.target));
                shell.refocus = Some((osk.target, 0));
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

    /// Keep the keyboard's suggestions in step with its text. The title index
    /// is built once per library on a worker thread; a keystroke only queries it.
    fn pad_shell_refresh_suggestions(&mut self, context: &egui::Context) {
        let wanted = self
            .pad_shell
            .as_ref()
            .and_then(|s| s.osk.as_ref())
            .filter(|o| o.suggest && !o.secret)
            .map(|o| (o.buffer.clone(), o.suggested_for.clone()));
        let Some((text, last)) = wanted else {
            return;
        };
        let index = self.title_index(context);
        let Some(osk) = self.pad_shell.as_mut().and_then(|s| s.osk.as_mut()) else {
            return;
        };
        osk.loading = index.is_none();
        if let Some(index) = index {
            if last.as_deref() != Some(text.as_str()) {
                osk.suggestions = index.suggest(&text);
                osk.suggested_for = Some(text);
                osk.suggestion_focus = osk
                    .suggestion_focus
                    .filter(|at| *at < osk.suggestions.len());
            }
        } else {
            osk.suggested_for = None;
            context.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The title index for the current library, once built.
    fn title_index(&self, context: &egui::Context) -> Option<Arc<suggest::TitleIndex>> {
        type Slot = Arc<Mutex<(usize, usize, Option<Arc<suggest::TitleIndex>>)>>;
        let slot: Slot = context.data_mut(|d| {
            d.get_temp_mut_or_insert_with::<Slot>(
                egui::Id::new("pad_title_index"),
                Default::default,
            )
            .clone()
        });
        let key = (
            Arc::as_ptr(&self.library) as usize,
            self.library.games.len(),
        );
        {
            let mut state = slot.lock().unwrap_or_else(|e| e.into_inner());
            if (state.0, state.1) == key {
                return state.2.clone();
            }
            *state = (key.0, key.1, None);
        }
        let library = self.library.clone();
        let context = context.clone();
        std::thread::spawn(move || {
            let built = Arc::new(suggest::TitleIndex::build(
                library.games.iter().map(|g| g.title.as_str()),
            ));
            let mut state = slot.lock().unwrap_or_else(|e| e.into_inner());
            if (state.0, state.1) == key {
                state.2 = Some(built);
            }
            drop(state);
            context.request_repaint();
        });
        None
    }

    fn pad_shell_draw_osk(&mut self, context: &egui::Context) {
        use crate::ui::theme::blue;
        self.pad_shell_refresh_suggestions(context);
        let Some(osk) = self.pad_shell.as_mut().and_then(|s| s.osk.as_mut()) else {
            return;
        };
        let mut clicked = None;
        let mut clicked_suggestion = None;
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
            if osk.suggest && !osk.secret {
                if osk.loading {
                    ui.label(
                        egui::RichText::new("Loading title suggestions…")
                            .color(blue::SECONDARY_TEXT),
                    );
                } else if !osk.suggestions.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for (at, title) in osk.suggestions.iter().enumerate() {
                            let shown: String = title.chars().take(34).collect();
                            let mut button =
                                egui::Button::new(egui::RichText::new(shown).size(17.0))
                                    .sense(egui::Sense::CLICK);
                            if osk.suggestion_focus == Some(at) {
                                button = button
                                    .fill(blue::PRIMARY_ACTION)
                                    .stroke(egui::Stroke::new(3.0_f32, blue::ACCENT));
                            }
                            if ui.add(button).on_hover_text(title.as_str()).clicked() {
                                clicked_suggestion = Some(at);
                            }
                        }
                    });
                    ui.label(
                        egui::RichText::new("Up from the top row to pick a suggestion")
                            .small()
                            .color(blue::SECONDARY_TEXT),
                    );
                }
                ui.add_space(4.0);
            }
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
        if let Some(at) = clicked_suggestion {
            osk.click_suggestion(at);
        }
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

    /// Press the focused control, unless it is a button in a dialog that is
    /// not recognisably safe: that must be held (see `HOLD_SECONDS`).
    fn pad_shell_press_focused(&mut self, context: &egui::Context) {
        let now = context.input(|i| i.time);
        let guarded = (!crate::ui::picker::open(context))
            .then(|| guarded_button(context))
            .flatten();
        match (guarded, self.pad_shell.as_mut()) {
            (Some((id, label)), Some(shell)) => {
                if shell.hold.is_none() {
                    shell.hold = Some((id, now, label));
                }
                context.request_repaint();
            }
            _ => page_command(context, Command::Select),
        }
    }

    /// Advance a hold-to-confirm: fires on completion, cancels on release or
    /// when focus moves, and shows its progress.
    fn pad_shell_hold_tick(&mut self, context: &egui::Context, now: f64) {
        let Some(shell) = self.pad_shell.as_mut() else {
            return;
        };
        let Some((id, since, label)) = shell.hold.clone() else {
            return;
        };
        let still = shell.south_down && context.memory(|m| m.focused()) == Some(id);
        if !still {
            shell.hold = None;
            return;
        }
        let progress = ((now - since) / HOLD_SECONDS).clamp(0.0, 1.0) as f32;
        if progress >= 1.0 {
            shell.hold = None;
            page_command(context, Command::Select);
            return;
        }
        egui::Area::new(egui::Id::new("pad_shell_hold"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .show(context, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(format!("Keep holding A to confirm: {label}"));
                    ui.add(egui::ProgressBar::new(progress).desired_width(320.0));
                });
            });
        context.request_repaint_after(std::time::Duration::from_millis(30));
    }

    /// Select on a focused text field opens the keyboard; anywhere else it
    /// presses the focused control.
    fn pad_shell_select(&mut self, context: &egui::Context) {
        use egui::widgets::text_edit::TextEditState;
        let field = context
            .memory(|m| m.focused())
            .filter(|id| TextEditState::load(context, *id).is_some());
        let Some(id) = field else {
            self.pad_shell_press_focused(context);
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
        // A file picker is open: it is driven by focus like any page, and the
        // page behind it must not change.
        if crate::ui::picker::open(context)
            && matches!(
                command,
                Command::Platforms | Command::Search | Command::Launch
            )
        {
            return;
        }
        match command {
            Command::Select => self.pad_shell_select(context),
            Command::Move(direction) => self.pad_shell_move(context, direction),
            Command::Back => {
                // If this dialog is still open a few frames from now, B has no
                // way to close it: let the controller out rather than trap it.
                let top = context.memory(|m| m.areas().top_layer_id(egui::Order::Middle));
                if let (Some(layer), Some(shell)) = (top, self.pad_shell.as_mut()) {
                    shell.back_pending.get_or_insert((layer, 3));
                }
                key_tap(context, egui::Key::Escape);
            }
            Command::PageUp => key_tap(context, egui::Key::PageUp),
            Command::PageDown => key_tap(context, egui::Key::PageDown),
            Command::First => key_tap(context, egui::Key::Home),
            Command::Last => key_tap(context, egui::Key::End),
            Command::Platforms => self.pad_shell_section(1),
            Command::Search => self.pad_shell_section(-1),
            // R3 is taken by the virtual mouse before commands are made.
            Command::Pointer => {}
            Command::Launch => {
                self.enter_big_picture(context);
                // Big Picture starts its own reader; release this one now.
                self.pad_shell = None;
            }
        }
    }

    /// Spatial movement: the nearest control in the direction pressed, so a
    /// grid moves by row and column and Right from the sidebar lands level
    /// with where you were. Nothing focused yet starts from the ends.
    fn pad_shell_move(&mut self, context: &egui::Context, direction: Direction) {
        use egui::FocusDirection as F;
        let focused = context.memory(|m| m.focused());
        let Some(shell) = self.pad_shell.as_mut() else {
            return;
        };
        // In a dialog or the picker, stay in reading order: egui reports a
        // control's name (used by hold-to-confirm) only for those moves, and
        // dialogs are short vertical lists where reading order is the layout.
        #[cfg(test)]
        let forced = shell.reading_order;
        #[cfg(not(test))]
        let forced = false;
        let in_dialog = forced
            || crate::ui::picker::open(context)
            || context.memory(|m| {
                m.areas()
                    .top_layer_id(egui::Order::Middle)
                    .is_some_and(|l| m.areas().is_visible(&l) && shell.released != Some(l))
            });
        if in_dialog {
            let forward = matches!(direction, Direction::Down | Direction::Right);
            context.memory_mut(|m| m.move_focus(if forward { F::Next } else { F::Previous }));
            return;
        }
        let Some(from) = focused else {
            let first = matches!(direction, Direction::Down | Direction::Right);
            context.memory_mut(|m| m.move_focus(if first { F::Next } else { F::Previous }));
            return;
        };
        if direction == Direction::Right
            && is_sidebar(context, from)
            && let Some(back) = shell.last_content.filter(|id| usable(context, *id))
        {
            // Return to where you were in the content, without scrolling.
            context.memory_mut(|m| m.request_focus(back));
            return;
        }
        let spatial = match direction {
            Direction::Up => F::Up,
            Direction::Down => F::Down,
            Direction::Left => F::Left,
            Direction::Right => F::Right,
        };
        context.memory_mut(|m| m.move_focus(spatial));
        if matches!(direction, Direction::Up | Direction::Down | Direction::Left) {
            shell.pending_move = Some((direction, focused, 0, false));
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

/// Whether a control sits in Blue's sidebar: by position, so every control in
/// that column counts, not only the eight destination rows.
fn is_sidebar(context: &egui::Context, id: egui::Id) -> bool {
    let edge = DESTINATIONS
        .iter()
        .filter_map(|d| context.read_response(simple_shell::nav_id(*d)))
        .map(|r| r.rect.right())
        .fold(f32::NEG_INFINITY, f32::max);
    edge.is_finite()
        && context
            .read_response(id)
            .is_some_and(|r| r.rect.right() <= edge + 1.0)
}

/// A remembered control that is still drawn, enabled and on screen.
fn usable(context: &egui::Context, id: egui::Id) -> bool {
    context.read_response(id).is_some_and(|r| {
        r.enabled() && r.rect.is_finite() && context.content_rect().intersects(r.rect)
    })
}

/// Remember the last content-region focus, and retry an Up/Down that found
/// nothing yet (a scrolling grid renders its next row after the scroll).
fn track_focus_region(context: &egui::Context, shell: &mut PadShell) {
    let focused = context.memory(|m| m.focused());
    if let Some(id) = focused
        && !is_sidebar(context, id)
        && context
            .read_response(id)
            .is_some_and(|r| r.layer_id.order == egui::Order::Background)
    {
        shell.last_content = Some(id);
    }
    if let Some((direction, before, waited, retried)) = shell.pending_move {
        if focused != before {
            shell.pending_move = None;
        } else if waited >= 1 {
            // Nothing was in that direction. A grid that scrolls renders its
            // next row only once the focused row is revealed, so look once
            // more; if there is still nothing, stay put (an edge), never
            // wander to an unrelated control.
            shell.pending_move = (!retried).then_some((direction, before, 0, true));
            if direction == Direction::Left {
                // Nothing further left in the content: the sidebar, on the row
                // of the page being shown (the sidebar may not be level).
                if let Some(row) = shell.current_nav.filter(|id| usable(context, *id)) {
                    context.memory_mut(|m| m.request_focus(row));
                }
                shell.pending_move = None;
            } else if !retried {
                context.memory_mut(|m| {
                    m.move_focus(if direction == Direction::Down {
                        egui::FocusDirection::Down
                    } else {
                        egui::FocusDirection::Up
                    })
                });
                context.request_repaint();
            }
        } else {
            shell.pending_move = Some((direction, before, waited + 1, retried));
            context.request_repaint();
        }
    }
}

/// While the controller is in use, an open dialog window is made the modal
/// layer for focus: egui then keeps focus movement and activation inside it
/// (it takes effect from the next frame), so Next/Previous can never step onto
/// the page behind. The picker and keyboard are `egui::Modal`s and do this
/// themselves.
fn confine_to_dialog(context: &egui::Context, shell: &mut PadShell) {
    context.memory_mut(|m| {
        let top = m
            .areas()
            .top_layer_id(egui::Order::Middle)
            .filter(|layer| m.areas().is_visible(layer));
        // A release or pending Back only lasts as long as its dialog does.
        if shell.released.is_some() && shell.released != top {
            shell.released = None;
        }
        if let Some((layer, left)) = shell.back_pending {
            shell.back_pending = if Some(layer) != top {
                None
            } else if left == 0 {
                shell.released = Some(layer);
                None
            } else {
                Some((layer, left - 1))
            };
        }
        if let Some(layer) = top
            && shell.released != Some(layer)
        {
            m.set_modal_layer(layer);
        }
    });
}

/// How long A is held to press an unrecognised button in a dialog.
const HOLD_SECONDS: f64 = 0.6;

/// Labels that only dismiss or go back: a tap is enough.
fn is_safe_label(label: &str) -> bool {
    let l = label.trim().trim_end_matches(['…', '.']).to_lowercase();
    [
        "cancel", "close", "back", "no", "ok", "okay", "done", "dismiss", "not now", "skip",
        "got it",
    ]
    .contains(&l.as_str())
        || ["keep ", "cancel ", "don't ", "do not ", "leave ", "stay "]
            .iter()
            .any(|p| l.starts_with(p))
}

/// The focused control when it is a button inside a dialog (not on the page
/// itself) whose label is not recognisably safe. Anything unknown is guarded:
/// a wrongly guarded button costs a longer press, never a wrong action.
fn guarded_button(context: &egui::Context) -> Option<(egui::Id, String)> {
    let id = context.memory(|m| m.focused())?;
    // A control whose name was never reported is guarded, not trusted.
    let (label, button) =
        match context.data(|d| d.get_temp::<(egui::Id, String, bool)>(focus_label_slot())) {
            Some((focus_id, label, button)) if focus_id == id => (label, button),
            _ => ("this control".to_owned(), true),
        };
    if !button || is_safe_label(&label) {
        return None;
    }
    let layer = context.read_response(id)?.layer_id;
    matches!(layer.order, egui::Order::Middle | egui::Order::Foreground).then_some((id, label))
}

fn focus_label_slot() -> egui::Id {
    egui::Id::new("pad_shell_focus_label")
}

fn snapshot_slot() -> egui::Id {
    egui::Id::new("pad_shell_snapshot")
}
fn hook_runs_slot() -> egui::Id {
    egui::Id::new("pad_shell_hook_runs")
}
/// How many passes the end-of-pass hook has run in. egui only runs it for
/// `run_ui`; tests use this to fail loudly when driven through plain `run`.
#[cfg(test)]
pub(super) fn hook_runs(context: &egui::Context) -> u64 {
    context
        .data(|d| d.get_temp::<u64>(hook_runs_slot()))
        .unwrap_or(0)
}
fn capture_slot() -> egui::Id {
    egui::Id::new("pad_shell_capture")
}

/// Mark a text field as a title search: the controller keyboard offers game
/// title suggestions for it. Call every frame the field is drawn.
pub(super) fn mark_suggestions(response: &egui::Response) {
    response
        .ctx
        .data_mut(|d| d.insert_temp(egui::Id::new("pad_suggest").with(response.id), true));
}

fn wants_suggestions(context: &egui::Context, id: egui::Id) -> bool {
    context
        .data(|d| d.get_temp::<bool>(egui::Id::new("pad_suggest").with(id)))
        .unwrap_or(false)
        && !is_secret(context, id)
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
            context.data_mut(|d| {
                *d.get_temp_mut_or_insert_with(hook_runs_slot(), || 0_u64) += 1;
            });
            let gained = context.output(|o| {
                o.events.iter().rev().find_map(|e| match e {
                    egui::output::OutputEvent::FocusGained(info) => Some((
                        info.label.clone().unwrap_or_default(),
                        info.typ == egui::WidgetType::Button,
                    )),
                    _ => None,
                })
            });
            if let (Some((label, button)), Some(focused)) =
                (gained, context.memory(|m| m.focused()))
            {
                context.data_mut(|d| d.insert_temp(focus_label_slot(), (focused, label, button)));
            }
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
    pub(super) fn buffer(&self) -> Option<String> {
        self.osk.as_ref().map(|o| o.buffer.clone())
    }
    pub(super) fn suggestions(&self) -> Vec<String> {
        self.osk
            .as_ref()
            .map(|o| o.suggestions.clone())
            .unwrap_or_default()
    }
    pub(super) fn suggestion_focus(&self) -> Option<usize> {
        self.osk.as_ref().and_then(|o| o.suggestion_focus)
    }
    pub(super) fn type_for_test(&mut self, text: &str) {
        if let Some(o) = self.osk.as_mut() {
            o.buffer.push_str(text);
        }
    }
    pub(super) fn focus_top_row_for_test(&mut self) {
        if let Some(o) = self.osk.as_mut() {
            o.grid.focus = 0;
        }
    }
    pub(super) fn mouse_on(&self) -> bool {
        self.mouse.on
    }
    pub(super) fn set_pad_mode_for_test(&mut self) {
        self.pad_mode = true;
    }
    pub(super) fn osk_open(&self) -> bool {
        self.osk.is_some()
    }
    pub(super) fn inject(&mut self, event: PadEvent) {
        self.injected.push(event);
    }
}
