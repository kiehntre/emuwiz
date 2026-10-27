//! Blue's read-only view of the existing DAT planner. No action dispatcher.
use super::*;

pub(crate) fn show(ui: &mut egui::Ui, view: &DatSourcesPageView) {
    ui.heading("DAT name preview");
    ui.label(PLAN_ONLY_PROMISE);
    ui.label("Names below come from the last DAT audit, not RomM titles or filename guesses. This is an audit snapshot; files and catalogues have not been rechecked here. It grants no approval to rename.");
    let Some(plan) = &view.rename_plan else {
        ui.label("No DAT rename plan is available. Open DATs & Verification and run its existing catalogue audit first, then return here.");
        return;
    };
    ui.label(format!(
        "Source authority: {} · audit generation {}",
        plan.source_display_name, plan.generation
    ));
    ui.label(format!("Audited scope: {}", plan.scan_root_short));
    ui.label("A source label identifies the selected catalogue; it does not authenticate its publisher. Weak or filename-only matches cannot propose names.");
    if let Some(error) = &plan.error {
        ui.colored_label(theme::WARNING, error);
        return;
    }
    if plan.truncated {
        ui.colored_label(
            theme::WARNING,
            "Partial audit: this preview does not cover the whole collection.",
        );
    }
    ui.label(format!(
        "{} audited · {} with hash evidence · {} proposals",
        plan.audited_total,
        plan.verified_total,
        plan.rows.len()
    ));
    if plan.rows.is_empty() {
        ui.label("No canonical names could be proposed from this audit's evidence.");
    }
    // Existing planner states, explanations and provenance stay authoritative.
    // Reuse the row details but expose no session-review or execution actions.
    const PAGE_SIZE: usize = 25;
    let id = ui.make_persistent_id("dat-canonical-preview-page");
    let pages = plan.rows.len().div_ceil(PAGE_SIZE).max(1);
    let mut page = ui
        .data_mut(|data| data.get_temp::<usize>(id).unwrap_or(0))
        .min(pages - 1);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(page > 0, egui::Button::new("Previous names"))
            .clicked()
        {
            page -= 1;
        }
        ui.label(format!("Page {} of {pages}", page + 1));
        if ui
            .add_enabled(page + 1 < pages, egui::Button::new("Next names"))
            .clicked()
        {
            page += 1;
        }
    });
    ui.data_mut(|data| data.insert_temp(id, page));
    let start = (page * PAGE_SIZE).min(plan.rows.len());
    for row in &plan.rows[start..(start + PAGE_SIZE).min(plan.rows.len())] {
        widgets::card(ui, |ui| super::show_rename_plan_row_details(ui, row));
    }
}
