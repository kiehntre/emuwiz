//! Native Organisation landing and flow coordinator.
//!
//! This module owns presentation state only. Canonical organisation delegates
//! to `rom_organisation_page`; Playing Library, RomM, ES-DE and RetroDECK all
//! delegate to `playing_library_page` and their existing core planners.

use super::{
    App,
    routes::{Route, Section},
};
use crate::ui::theme;
use crate::{
    playing_library_page::PlayingLibraryDestination,
    rom_organisation_page::{self, RomOrganisationPageAction},
};
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum OrganisationView {
    #[default]
    Landing,
    VerifiedGames,
    PlayingLibrary,
}

#[derive(Default)]
pub(super) struct OrganisationState {
    pub(super) view: OrganisationView,
}

#[derive(Clone, Copy)]
struct ActionCard {
    title: &'static str,
    description: &'static str,
    semantics: &'static str,
    destination: Option<PlayingLibraryDestination>,
}

const ACTIONS: [ActionCard; 5] = [
    ActionCard {
        title: "Organise verified games",
        description: "Rename or arrange verified files into clean platform folders.",
        semantics: "Moves or renames original files · Preview required · Undo available",
        destination: None,
    },
    ActionCard {
        title: "Build a clean playing library",
        description: "Create a tidy linked library without moving your original games.",
        semantics: "Source untouched · Creates links · Preview required · Undo available",
        destination: Some(PlayingLibraryDestination::Generic),
    },
    ActionCard {
        title: "Organise for RomM",
        description: "Create a RomM-ready library using reviewed platform folders and safe links.",
        semantics: "Source untouched · Creates links · Visibility check required",
        destination: Some(PlayingLibraryDestination::Romm),
    },
    ActionCard {
        title: "Export to ES-DE",
        description: "Build a clean linked library, then update gamelist.xml safely.",
        semantics: "Source untouched · Creates links · Metadata updated separately",
        destination: Some(PlayingLibraryDestination::EsDe),
    },
    ActionCard {
        title: "Prepare for RetroDECK",
        description: "Create the linked layout RetroDECK expects and publish its ES-DE metadata.",
        semantics: "Source untouched · Creates links · Sandbox visibility required",
        destination: Some(PlayingLibraryDestination::RetroDeck),
    },
];

fn primary(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(text).strong())
            .fill(theme::PRIMARY_ACTION)
            .min_size(egui::vec2(180.0, 44.0)),
    )
    .clicked()
}

impl App {
    pub(super) fn organisation_page(&mut self, ui: &mut egui::Ui) {
        let mut selected = None;
        let mut advanced = false;
        let mut back = false;
        let mut canonical_action = None;
        let mut playing_action = None;
        let mut review_pending = false;
        let mut open_history = false;

        egui::ScrollArea::vertical()
            .id_salt("v2_organisation")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.organisation.view {
                OrganisationView::Landing => {
                    ui.heading("Choose what you want to organise");
                    ui.label("Choose how you want EmuWiz to arrange or publish your verified games. Nothing changes until you preview and confirm.");
                    ui.label("Every option explains whether original files move, links are created, or only frontend metadata changes.");
                    if self.playing_library.attention_snapshot().items().next().is_some() {
                        crate::ui::components::banner(
                            ui,
                            "An earlier organisation operation needs attention",
                            "Review its saved recovery state before starting another publication.",
                            crate::ui::components::StatusTone::Warning,
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Review").clicked() {
                                review_pending = true;
                            }
                            if ui.button("History").clicked() {
                                open_history = true;
                            }
                        });
                    }
                    ui.add_space(8.0);
                    for card in ACTIONS {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.heading(card.title);
                            ui.label(card.description);
                            ui.label(RichText::new(card.semantics).strong());
                            ui.label("Ready when its required source, destination and verification data are available.");
                            if primary(ui, card.title) {
                                selected = Some(card.destination);
                            }
                        });
                        ui.add_space(8.0);
                    }
                    if ui.button("Advanced organisation tools").clicked() {
                        advanced = true;
                    }
                }
                OrganisationView::VerifiedGames => {
                    self.invalidate_changed_canonical_organisation_plan();
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("← Organisation").clicked() {
                            back = true;
                        }
                        ui.strong("1 Choose  ·  2 Preview  ·  3 Confirm  ·  4 Apply");
                    });
                    ui.label("MOVE changes the original file's folder. RENAME changes its name in place. LINK leaves the original untouched and creates a shortcut in a new library.");
                    canonical_action = rom_organisation_page::show_rom_organisation_page_with_busy(
                        ui,
                        &mut self.canonical_organisation,
                        self.canonical_organisation_job.is_some(),
                        true,
                    );
                }
                OrganisationView::PlayingLibrary => {
                    self.invalidate_changed_playing_library_plan();
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("← Organisation").clicked() {
                            back = true;
                        }
                        ui.strong("1 Choose  ·  2 Preview  ·  3 Confirm  ·  4 Apply");
                    });
                    ui.label("Original files stay untouched. EmuWiz creates reviewed links in the destination, then publishes metadata separately when required.");
                    if self.playing_library.destination == PlayingLibraryDestination::Romm {
                        ui.label("This creates files for RomM to scan. It does not edit your RomM server.");
                    }
                    if self.playing_library.destination == PlayingLibraryDestination::RetroDeck {
                        ui.label("RetroDECK uses ES-DE internally and also needs both linked paths to be visible inside its sandbox.");
                    }
                    playing_action = crate::playing_library_page::show_playing_library_page_with_busy(
                        ui,
                        &mut self.playing_library,
                        self.playing_library_job.is_some(),
                    );
                }
            });

        if back {
            self.organisation.view = OrganisationView::Landing;
        }
        if let Some(destination) = selected {
            match destination {
                None => self.organisation.view = OrganisationView::VerifiedGames,
                Some(destination) => {
                    self.playing_library.set_destination(destination);
                    self.organisation.view = OrganisationView::PlayingLibrary;
                }
            }
        }
        if advanced {
            self.legacy(Section::Build);
        }
        if review_pending {
            self.organisation.view = OrganisationView::PlayingLibrary;
        }
        if open_history {
            self.go(Route::Section(Section::History));
        }
        if let Some(action) = canonical_action {
            match action {
                RomOrganisationPageAction::Preview => self
                    .start_canonical_organisation_job(super::CanonicalOrganisationJobKind::Preview),
                RomOrganisationPageAction::Apply => self
                    .start_canonical_organisation_job(super::CanonicalOrganisationJobKind::Apply),
                RomOrganisationPageAction::Rollback => self.start_canonical_organisation_job(
                    super::CanonicalOrganisationJobKind::Rollback,
                ),
            }
        }
        if let Some(action) = playing_action {
            self.handle_playing_library_action(action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_has_exactly_the_five_backed_normal_actions() {
        assert_eq!(ACTIONS.len(), 5);
        assert_eq!(ACTIONS[0].title, "Organise verified games");
        assert_eq!(
            ACTIONS[1].destination,
            Some(PlayingLibraryDestination::Generic)
        );
        assert_eq!(
            ACTIONS[2].destination,
            Some(PlayingLibraryDestination::Romm)
        );
        assert_eq!(
            ACTIONS[3].destination,
            Some(PlayingLibraryDestination::EsDe)
        );
        assert_eq!(
            ACTIONS[4].destination,
            Some(PlayingLibraryDestination::RetroDeck)
        );
        assert!(ACTIONS.iter().all(|card| card.semantics.contains("Preview")
            || card.semantics.contains("required")
            || card.semantics.contains("separately")));
    }
}
