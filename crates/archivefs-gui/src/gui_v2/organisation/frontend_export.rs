//! Blue host for the existing Publisher Profiles controller.
use super::*;
use crate::publisher_profile_page::{PublisherProfilePageAction, show_publisher_profile_page};

pub(super) fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Back to Organisation").clicked() {
            app.organisation.view = OrganisationView::Landing;
        }
        if ui.button("ES-DE gamelist export").clicked() {
            app.playing_library
                .set_destination(PlayingLibraryDestination::EsDe);
            app.organisation.view = OrganisationView::PlayingLibrary;
        }
    });
    let page = &mut app.organisation.frontend_export;
    page.sync_source_plan(ui, app.playing_library.plan());
    if show_publisher_profile_page(ui, page)
        == Some(PublisherProfilePageAction::OpenLibraryOrganisation)
    {
        app.organisation.view = OrganisationView::PlayingLibrary;
    }
}
