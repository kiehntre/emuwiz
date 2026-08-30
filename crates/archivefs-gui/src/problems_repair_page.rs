//! The consolidated "Problems & Repair" destination's shared chrome and
//! Overview tab.
//!
//! # Why this exists
//!
//! Before this module, a user had to choose between three separate sidebar
//! destinations - "Doctor", "Repair Review", "Repair History" - before
//! knowing which one applied to their situation. This module presents all
//! three as tabs of one page (`Overview` / `Diagnostics` / `Repair /
//! Recovery`), reached through exactly one sidebar entry
//! (`nav_view(MainView::Problems, "Problems & Repair")` in
//! `navigation::ADVANCED_NAV_GROUPS`).
//!
//! # This does not merge any engine
//!
//! `MainView::Doctor`, `MainView::RepairReview`, and `MainView::RepairHistory`
//! still exist, are still individually reachable by deep-link, and still
//! render through their own completely unchanged code:
//! [`crate::doctor_page::show_doctor_page`] for Diagnostics,
//! [`crate::repair_review_page`]/[`crate::repair_history_page`] for Repair /
//! Recovery. `ProblemsRepairTab` (defined in `main.rs`, alongside its
//! `LibraryTab` sibling) is a *derived* projection of `ArchiveFsApp::view`,
//! not a new state machine - see that type's own doc comment and
//! `ArchiveFsApp::show_problems_repair_page`, which owns the actual
//! per-tab dispatch (it needs `&mut self` field access no free function in
//! this module could have without an unreasonably large parameter list).
//!
//! This module owns only the two genuinely page-local, stateless pieces:
//! the shared tab row every visit renders identically
//! ([`show_problems_repair_tabs`]), and the Overview tab's own short
//! summary/quick-links ([`show_problems_repair_overview`]) - the one tab
//! with no pre-existing renderer of its own to delegate to.

use super::*;

/// The consolidated destination's shared heading and tab selector, rendered
/// identically regardless of which tab is active - mirrors
/// `show_library_shell_header`'s role for the unified Library shell.
/// Returns the newly clicked tab, if any; the caller
/// (`ArchiveFsApp::show_problems_repair_page`) applies it via
/// `navigate_to_problems_repair_tab`.
pub(crate) fn show_problems_repair_tabs(
    ui: &mut egui::Ui,
    current: ProblemsRepairTab,
) -> Option<ProblemsRepairTab> {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::CHECK,
        "Problems & Repair",
        "See what needs attention and fix it, without switching between separate diagnosis and repair screens.",
    );
    let tab_options: [(ProblemsRepairTab, &str); 3] = [
        (ProblemsRepairTab::Overview, "Overview"),
        (ProblemsRepairTab::Diagnostics, "Diagnostics"),
        (ProblemsRepairTab::Repair, "Repair / Recovery"),
    ];
    let clicked = widgets::tab_row(ui, &tab_options, current);
    ui.add_space(8.0);
    clicked
}

/// The Overview tab: a one-glance "is anything wrong" summary drawn from
/// the same `DoctorScanState` the Diagnostics tab renders in full, plus
/// quick links into Diagnostics and Repair / Recovery. Deliberately thin -
/// it never re-derives evidence or duplicates the finding list; "actionable
/// problems first" here means "tell me whether to go look", not a second
/// copy of what Diagnostics already shows.
///
/// Returns the tab a quick-link button asked to switch to, if any.
pub(crate) fn show_problems_repair_overview(
    ui: &mut egui::Ui,
    doctor_scan: &DoctorScanState,
) -> Option<ProblemsRepairTab> {
    widgets::card(ui, |ui| match doctor_scan.displayed() {
        Some(outcome) if outcome.scan.is_healthy() => {
            widgets::status_badge(ui, "Healthy", widgets::StatusTone::Success);
            ui.label("No problems were found the last time Diagnostics ran.");
        }
        Some(outcome) => {
            let severity = outcome.scan.overall_severity();
            widgets::status_badge(
                ui,
                severity.label(),
                doctor_page::doctor_severity_tone(severity),
            );
            ui.label(
                "Open Diagnostics to review what was found and, where a repair exists, review it.",
            );
        }
        None => {
            widgets::status_badge(ui, "Not checked yet", widgets::StatusTone::Pending);
            ui.label(
                "Nothing has been checked yet. Open Diagnostics and run a scan to see if anything needs attention.",
            );
        }
    });
    ui.add_space(theme::SECTION_GAP);
    ui.label(
        "Diagnostics finds problems and, where available, offers to review a fix. Repair / \
         Recovery reviews and applies whole-library repair plans, and shows past repairs with \
         undo where the core proves it is safe.",
    );
    ui.add_space(theme::SECTION_GAP);
    let mut go_to = None;
    ui.horizontal_wrapped(|ui| {
        if widgets::action_button(ui, "Open Diagnostics", widgets::ActionStyle::Primary, true)
            .clicked()
        {
            go_to = Some(ProblemsRepairTab::Diagnostics);
        }
        if widgets::action_button(
            ui,
            "Open Repair / Recovery",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked()
        {
            go_to = Some(ProblemsRepairTab::Repair);
        }
    });
    go_to
}

/// Shows the safe, navigation-only front door for catalogue entries that were
/// not seen by the latest successful scan.  The actual review and removal
/// controls remain on Library -> Archives, where the existing selection,
/// validation, and confirmation flow owns them.
pub(crate) fn show_stale_library_review_entry(
    ui: &mut egui::Ui,
    doctor_scan: &DoctorScanState,
) -> bool {
    let count = stale_library_entry_count(doctor_scan);
    if count == 0 {
        return false;
    }

    let mut clicked = false;
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{count} stale library entr{}",
                    if count == 1 { "y" } else { "ies" }
                ))
                .strong(),
            );
            if widgets::action_button(
                ui,
                format!("Review stale library entries ({count})"),
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                clicked = true;
            }
        });
        ui.add(egui::Label::new(
            "These catalogue entries were not seen during the latest successful scan. Review them before removing stale entries from EmuWiz. Original ROM and archive files are never deleted by this action.",
        ).wrap());
    });
    clicked
}

pub(crate) fn stale_library_entry_count(doctor_scan: &DoctorScanState) -> usize {
    doctor_scan
        .displayed()
        .map(|outcome| {
            outcome
                .scan
                .findings
                .iter()
                .filter(|finding| finding.id == "library.archive_missing")
                .count()
        })
        .unwrap_or(0)
}
