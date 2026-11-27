//! "What to include" for Standard Bulk Rename: games selected in the Library,
//! whole platforms, or folders inside the configured sources. Choosing a scope
//! never changes the configuration; it only says what the next preview covers.
use std::collections::BTreeSet;

use archivefs_core::dat::rom_organisation::OrganisationMode;
use eframe::egui;

use super::App;
use super::library::Library;
use super::routes::{Route, Section};
use crate::rom_organisation_page::scope::{OrganisationScope, ScopedGame};
use crate::ui::{components as widgets, theme};

/// Library games as scope entries.
pub(super) fn scoped_games<'a>(
    games: impl Iterator<Item = &'a super::library::Game>,
) -> Vec<ScopedGame> {
    games
        .map(|game| ScopedGame {
            archive_id: game.archive.id,
            path: game.archive.absolute_path.clone(),
            known_missing: game.archive.last_verified_missing_at.is_some(),
        })
        .collect()
}

/// Every Library game of these platforms.
pub(super) fn platform_scope(library: &Library, names: &BTreeSet<String>) -> OrganisationScope {
    let games = names
        .iter()
        .filter_map(|name| library.by_platform.get(name))
        .flatten()
        .filter_map(|index| library.games.get(*index));
    OrganisationScope::Platforms {
        names: names.clone(),
        games: scoped_games(games),
    }
}

impl App {
    /// The one check every Blue entry point makes before changing the
    /// Organise & Export mode or scope: while a preview, apply or other
    /// organisation job runs, the page keeps the mode and scope that job
    /// started with, and the job card says why.
    pub(super) fn organisation_may_change(&mut self) -> bool {
        if self.canonical_organisation_job.is_none() {
            self.organisation.busy_notice = None;
            return true;
        }
        self.organisation.busy_notice =
            Some(crate::rom_organisation_page::ORGANISATION_BUSY_NOTICE.into());
        false
    }

    /// The landing page's "Rename verified games".
    pub(super) fn open_rename_verified_games(&mut self) {
        if self.organisation_may_change() {
            self.canonical_organisation
                .set_mode(OrganisationMode::RenameInPlace);
        }
        self.organisation.view = super::organisation::OrganisationView::VerifiedGames;
    }

    /// Opens Rename verified games with this scope.
    pub(super) fn open_bulk_rename(&mut self, scope: OrganisationScope) {
        if !self.organisation_may_change() {
            self.library_selection.notice = Some(
                "Organise & Export is busy. Rename the selection when the current job has finished."
                    .into(),
            );
            return;
        }
        self.canonical_organisation
            .ensure_mode(OrganisationMode::RenameInPlace);
        self.canonical_organisation.set_scope(Some(scope));
        self.organisation.view = super::organisation::OrganisationView::VerifiedGames;
        self.go(Route::Section(Section::Build));
    }

    /// The "What to include" card above the rename preview.
    pub(super) fn rename_scope_card(&mut self, ui: &mut egui::Ui, busy: bool) {
        let library = self.library.clone();
        let sources = self.canonical_organisation.source_roots().to_vec();
        let mut chosen = None;
        widgets::card(ui, |ui| {
            // The active scope, first and unmistakable (Stage B, owner
            // 2026-11-26).
            widgets::status_badge(
                ui,
                crate::rom_organisation_page::scope::OrganisationScope::banner(
                    self.canonical_organisation.scope(),
                ),
                if self.canonical_organisation.scope().is_some() {
                    widgets::StatusTone::Info
                } else {
                    widgets::StatusTone::Pending
                },
            );
            ui.strong("What to include");
            ui.label(
                egui::RichText::new(
                    "To rename particular games, select them in the Library and choose Rename selected.",
                )
                .color(theme::muted(ui)),
            );
            let picker = &mut self.canonical_organisation.scope_picker;
            ui.collapsing("Platforms", |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rename-scope-platforms")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (name, count) in &library.platforms {
                            let mut on = picker.platforms.contains(name);
                            if ui.checkbox(&mut on, format!("{name} ({count})")).changed() {
                                if on {
                                    picker.platforms.insert(name.clone());
                                } else {
                                    picker.platforms.remove(name);
                                }
                            }
                        }
                    });
                if ui
                    .add_enabled(
                        !busy && !picker.platforms.is_empty(),
                        egui::Button::new("Use these platforms"),
                    )
                    .clicked()
                {
                    chosen = Some(platform_scope(&library, &picker.platforms));
                }
            });
            ui.collapsing("Folders", |ui| {
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut picker.folder_draft);
                    if ui.button("Add folder").clicked() {
                        picker.add_folder(&sources);
                    }
                });
                if let Some(error) = &picker.folder_error {
                    ui.label(
                        egui::RichText::new(error).color(widgets::StatusTone::Blocked.color(ui)),
                    );
                }
                let mut remove = None;
                for (index, folder) in picker.folders.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(folder.display().to_string());
                        if ui.small_button("Remove").clicked() {
                            remove = Some(index);
                        }
                    });
                }
                if let Some(index) = remove {
                    picker.folders.remove(index);
                }
                ui.label(
                    egui::RichText::new(
                        "A folder is read completely, but never inside a mounted archive or another disk.",
                    )
                    .color(theme::muted(ui)),
                );
                if ui
                    .add_enabled(
                        !busy && !picker.folders.is_empty(),
                        egui::Button::new("Use these folders"),
                    )
                    .clicked()
                {
                    chosen = Some(OrganisationScope::Folders(picker.folders.clone()));
                }
            });
        });
        if let Some(scope) = chosen
            && self.organisation_may_change()
        {
            self.canonical_organisation.set_scope(Some(scope));
        }
    }
}
