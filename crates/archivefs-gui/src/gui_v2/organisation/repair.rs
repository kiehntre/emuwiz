//! Native Organisation host for the established repair controllers.
use super::*;

pub(super) fn show(app: &mut App, ui: &mut egui::Ui, history: bool) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Back to Organisation").clicked() {
            app.organisation.view = OrganisationView::Landing;
        }
        if ui.selectable_label(!history, "Repair Review").clicked() {
            app.organisation.view = OrganisationView::RepairReview;
        }
        if ui.selectable_label(history, "Repair History").clicked() {
            app.organisation.view = OrganisationView::RepairHistory;
        }
    });
    ui.label("Preview is read-only; Apply and Undo require confirmation. Leaving this page does not cancel accepted work.");
    let workflows = app.native_workflows.get_or_insert_with(|| {
        super::super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
    });
    if history {
        workflows.show_native_repair_history(ui);
    } else {
        workflows.show_native_repair_review(ui);
    }
}
