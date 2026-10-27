//! Native Organisation host for the established repair controllers.
use super::*;

pub(super) fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Back to Organisation").clicked() {
            app.organisation.view = OrganisationView::Landing;
        }
        if ui.button("DAT name preview (read-only)").clicked() {
            app.organisation.view = OrganisationView::DatNamePreview;
        }
        if ui
            .selectable_label(
                app.organisation.view == OrganisationView::RepairReview,
                "Repair Review",
            )
            .clicked()
        {
            app.organisation.view = OrganisationView::RepairReview;
        }
        if ui
            .selectable_label(
                app.organisation.view == OrganisationView::RepairHistory,
                "Repair History",
            )
            .clicked()
        {
            app.organisation.view = OrganisationView::RepairHistory;
        }
        if ui
            .selectable_label(
                app.organisation.view == OrganisationView::ExactDuplicates,
                "Exact duplicates",
            )
            .clicked()
        {
            app.organisation.view = OrganisationView::ExactDuplicates;
        }
    });
    ui.label("Preview is read-only; Apply and Undo require confirmation. Leaving this page does not cancel accepted work.");
    let workflows = app.native_workflows.get_or_insert_with(|| {
        super::super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
    });
    match app.organisation.view {
        OrganisationView::DatNamePreview => workflows.show_dat_name_preview(ui),
        OrganisationView::RepairHistory => workflows.show_native_repair_history(ui),
        OrganisationView::ExactDuplicates => workflows.show_native_exact_duplicates(ui),
        _ => workflows.show_native_repair_review(ui),
    }
}
