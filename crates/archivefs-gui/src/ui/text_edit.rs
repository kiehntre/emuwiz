//! Single-line text fields with a right-click Cut/Copy/Paste/Select-all menu.
//!
//! egui has no synchronous clipboard hook a context-menu item can call, so
//! these helpers apply the edit themselves against the field's own char
//! ranges, using the same `TextBuffer` trait egui's built-in Ctrl+X/C/V uses
//! - they can never disagree with it about UTF-8 boundaries. The clipboard
//! itself stays behind `ClipboardBackend`, so tests drive the whole menu
//! without an OS clipboard.

use std::ops::Range;

use eframe::egui;
// Brings `String`'s char-index-safe insert/delete/slice methods into
// scope - see `show_text_edit_with_context_menu` and its helpers, the
// only place these are used. The same trait egui's own `TextEdit`
// editing uses internally, so this can never disagree with it about
// UTF-8/char-boundary handling.
use eframe::egui::TextBuffer;

use crate::{ClipboardBackend, ClipboardTextStatus, clipboard_status_label};

/// One item in the shared text-field context menu - see
/// `show_text_edit_with_context_menu`. Deliberately just these four:
/// exactly what a normal desktop text field's right-click menu offers,
/// nothing more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextEditContextMenuAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl TextEditContextMenuAction {
    pub(crate) const ALL: [Self; 4] = [Self::Cut, Self::Copy, Self::Paste, Self::SelectAll];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cut => "Cut",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select all",
        }
    }
}

pub(crate) fn text_edit_context_menu_action_available(
    action: TextEditContextMenuAction,
    has_selection: bool,
    is_empty: bool,
    has_clipboard_text: bool,
) -> bool {
    match action {
        TextEditContextMenuAction::Cut | TextEditContextMenuAction::Copy => has_selection,
        TextEditContextMenuAction::Paste => has_clipboard_text,
        TextEditContextMenuAction::SelectAll => !is_empty,
    }
}

pub(crate) fn text_edit_selected_char_range(
    ctx: &egui::Context,
    id: egui::Id,
) -> Option<Range<usize>> {
    let range = egui::widgets::text_edit::TextEditState::load(ctx, id)?
        .cursor
        .char_range()?;
    (!range.is_empty()).then(|| range.as_sorted_char_range())
}

/// The field's current cursor position as a character range - a
/// selection if one exists, otherwise an empty range at the caret. Unlike
/// `text_edit_selected_char_range`, this always returns *something*:
/// Paste needs an insertion point even with no selection, falling back to
/// the end of `text` if the field has no persisted cursor state at all
/// (never out of bounds, and the only reasonable default with zero other
/// information).
pub(crate) fn text_edit_cursor_char_range(
    ctx: &egui::Context,
    id: egui::Id,
    text: &str,
) -> Range<usize> {
    egui::widgets::text_edit::TextEditState::load(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.as_sorted_char_range())
        .unwrap_or_else(|| {
            let end = text.chars().count();
            end..end
        })
}

/// Moves the field's cursor to a single position (no selection) and gives
/// it keyboard focus, so the edit this always follows is visible
/// immediately and further typing continues from the right place.
pub(crate) fn set_text_edit_caret(ctx: &egui::Context, id: egui::Id, char_index: usize) {
    let mut state = egui::widgets::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(char_index),
        )));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
}

pub(crate) fn apply_select_all(ctx: &egui::Context, id: egui::Id, text: &str) {
    let mut state = egui::widgets::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let full_range = egui::text::CCursorRange::two(
        egui::text::CCursor::new(0),
        egui::text::CCursor::new(text.chars().count()),
    );
    state.cursor.set_char_range(Some(full_range));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
}

/// Copy - writes exactly the selected substring (char-index sliced via
/// `TextBuffer::char_range`, so a selection ending mid-multi-byte
/// character is impossible) to the clipboard. A no-op if nothing is
/// selected; never clears or overwrites the clipboard in that case. A
/// clipboard write failure is safely discarded here (already logged by
/// `ClipboardBackend::set_text`); there is no field mutation to roll
/// back for Copy.
pub(crate) fn apply_copy(
    ctx: &egui::Context,
    id: egui::Id,
    text: &str,
    clipboard: &mut dyn ClipboardBackend,
) {
    if let Some(range) = text_edit_selected_char_range(ctx, id) {
        let _ = clipboard.set_text(text.char_range(range).to_string());
    }
}

/// Cut - copies exactly the selected substring, then removes exactly
/// that same range from `text` (via `TextBuffer::delete_char_range`, the
/// identical method `TextEdit`'s own Ctrl+X handling uses), leaving the
/// caret where the removed text started. A no-op if nothing is selected.
/// If the clipboard write fails, `text` is left completely untouched -
/// removing text the user could not actually cut anywhere would be a
/// silent data loss, not a safe degradation.
pub(crate) fn apply_cut(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    clipboard: &mut dyn ClipboardBackend,
) {
    let Some(range) = text_edit_selected_char_range(ctx, id) else {
        return;
    };
    if clipboard
        .set_text(text.char_range(range.clone()).to_string())
        .is_err()
    {
        return;
    }
    text.delete_char_range(range.clone());
    set_text_edit_caret(ctx, id, range.start);
}

/// Paste - inserts the clipboard's text at the caret (no selection), or
/// replaces exactly the selected range (a selection, partial or the
/// entire field) via `TextBuffer::insert_text`/`delete_char_range`, the
/// same char-index-safe methods `TextEdit`'s own Ctrl+V handling uses. A
/// no-op if the clipboard is empty *or* unavailable - both cases already
/// disable the Paste menu item (see `text_edit_context_menu_action_available`),
/// but this defends against being called anyway (e.g. a stale click).
pub(crate) fn apply_paste(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    clipboard: &mut dyn ClipboardBackend,
) {
    let clip_text = match clipboard.get_text_status() {
        ClipboardTextStatus::Ready(text) => text,
        ClipboardTextStatus::Empty | ClipboardTextStatus::Unavailable(_) => return,
    };
    let range = text_edit_cursor_char_range(ctx, id, text);
    if !range.is_empty() {
        text.delete_char_range(range.clone());
    }
    let inserted = text.insert_text(&clip_text, range.start);
    set_text_edit_caret(ctx, id, range.start + inserted);
}

pub(crate) fn show_text_edit_with_context_menu(
    ui: &mut egui::Ui,
    text: &mut String,
    clipboard: &mut dyn ClipboardBackend,
    configure: impl FnOnce(egui::TextEdit<'_>) -> egui::TextEdit<'_>,
) -> egui::Response {
    let is_empty = text.is_empty();
    let text_edit = configure(egui::TextEdit::singleline(text));
    let output = text_edit.show(ui);
    let response = output.response.response;
    let id = response.id;
    let has_selection = output.cursor_range.is_some_and(|range| !range.is_empty());

    // Right-clicking to open this field's context menu also gives it
    // keyboard focus, exactly like a real desktop text field - so the
    // field visibly looks active even before any menu item is clicked.
    // This is cosmetic only now: every action below reaches the correct
    // field via its `id`, captured once above, regardless of whether
    // focus is still there by the time the user actually clicks.
    if response.secondary_clicked() {
        ui.memory_mut(|memory| memory.request_focus(id));
    }

    response.context_menu(|ui| {
        // One read per menu-open frame, shared by the status line and
        // Paste's enabled state - never a second clipboard read.
        let clipboard_status = clipboard.get_text_status();
        ui.small(clipboard_status_label(&clipboard_status));
        ui.separator();
        let has_clipboard_text = matches!(clipboard_status, ClipboardTextStatus::Ready(_));
        for action in TextEditContextMenuAction::ALL {
            let enabled = text_edit_context_menu_action_available(
                action,
                has_selection,
                is_empty,
                has_clipboard_text,
            );
            if ui
                .add_enabled(enabled, egui::Button::new(action.label()))
                .clicked()
            {
                match action {
                    TextEditContextMenuAction::Cut => apply_cut(ui.ctx(), id, text, clipboard),
                    TextEditContextMenuAction::Copy => apply_copy(ui.ctx(), id, text, clipboard),
                    TextEditContextMenuAction::Paste => apply_paste(ui.ctx(), id, text, clipboard),
                    TextEditContextMenuAction::SelectAll => apply_select_all(ui.ctx(), id, text),
                }
                ui.close();
            }
        }
    });

    response
}
