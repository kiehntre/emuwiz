//! The shared typed-count confirmation gate.
//!
//! Every bulk confirmation dialog in the app calls
//! [`show_bulk_action_typed_count_gate`], so the threshold and the
//! comparison can never drift between them.

use eframe::egui;

/// Renders the ">25 items" typed-count input when required (decisions
/// 1-3, docs/GUI_NAVIGATION_RESET_DESIGN.md §9) and returns whether the
/// confirm button should be enabled - the one shared gate every bulk
/// confirmation dialog in the app calls, so the threshold and comparison
/// can never drift between them.
pub(crate) fn show_bulk_action_typed_count_gate(
    ui: &mut egui::Ui,
    count: usize,
    typed: &mut String,
    otherwise_available: bool,
) -> bool {
    if bulk_action_requires_typed_count(count) {
        ui.label(format!(
            "This affects more than {BULK_ACTION_TYPED_CONFIRMATION_THRESHOLD} items. Type the \
             exact count ({count}) to confirm."
        ));
        ui.add(
            egui::TextEdit::singleline(typed)
                .desired_width(80.0)
                .hint_text(count.to_string()),
        );
    }
    bulk_action_confirm_enabled(count, typed, otherwise_available)
}

/// Decisions 1-3 (docs/GUI_NAVIGATION_RESET_DESIGN.md §9): every bulk
/// action shows a preview and exact item count; 1-25 items use a normal
/// confirmation; more than this threshold requires typing the exact
/// count. One shared threshold and one shared comparison function - every
/// bulk-action confirmation dialog in the app (Mount All, Unmount All,
/// Mount Queue, Mount Selected, bulk platform assignment, missing-entry
/// removal) calls these two functions rather than each re-implementing
/// its own gate, so the rule cannot drift between call sites.
pub(crate) const BULK_ACTION_TYPED_CONFIRMATION_THRESHOLD: usize = 25;

pub(crate) fn bulk_action_requires_typed_count(count: usize) -> bool {
    count > BULK_ACTION_TYPED_CONFIRMATION_THRESHOLD
}

/// Exact match only - no leading/trailing whitespace tolerance beyond a
/// plain `trim`, no partial/prefix match, no sign, no thousands
/// separator. A count that hasn't been typed, or was typed wrong, must
/// never satisfy this.
pub(crate) fn bulk_action_typed_count_matches(typed: &str, count: usize) -> bool {
    let trimmed = typed.trim();
    !trimmed.is_empty() && trimmed == count.to_string()
}

/// Whether a bulk-action confirmation's primary button should be enabled:
/// the ordinary busy/eligibility gate, *and*, only once the count exceeds
/// the threshold, an exact typed match.
pub(crate) fn bulk_action_confirm_enabled(
    count: usize,
    typed: &str,
    otherwise_available: bool,
) -> bool {
    otherwise_available
        && (!bulk_action_requires_typed_count(count)
            || bulk_action_typed_count_matches(typed, count))
}
