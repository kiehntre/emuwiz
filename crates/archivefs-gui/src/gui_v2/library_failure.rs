//! A failed library load is its own state. It must never read as "still
//! loading" or as "0 games": the catalogue on disk is intact, this process just
//! could not open it.
use super::{App, Section, pages::primary};
use eframe::egui;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LibraryLoadFailureKind {
    /// The catalogue was written by an older schema than this build requires.
    UpgradeRequired {
        found: i64,
        required: i64,
    },
    /// The catalogue was written by a newer EmuWiz than this build.
    NewerThanThisBuild {
        found: i64,
        required: i64,
    },
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LibraryLoadFailure {
    pub kind: LibraryLoadFailureKind,
    pub technical: String,
}

impl LibraryLoadFailure {
    pub fn from_error(error: &str) -> Self {
        let kind = match (
            number_after(error, "schema version "),
            number_after(error, "required current version "),
        ) {
            (Some(found), Some(required)) if found < required => {
                LibraryLoadFailureKind::UpgradeRequired { found, required }
            }
            (Some(found), Some(required)) if found > required => {
                LibraryLoadFailureKind::NewerThanThisBuild { found, required }
            }
            _ => LibraryLoadFailureKind::Other,
        };
        Self {
            kind,
            technical: error.to_string(),
        }
    }

    fn heading(&self) -> &'static str {
        match self.kind {
            LibraryLoadFailureKind::UpgradeRequired { .. } => "Your library needs an upgrade",
            LibraryLoadFailureKind::NewerThanThisBuild { .. } => {
                "This library was made by a newer EmuWiz"
            }
            LibraryLoadFailureKind::Other => "Your game library could not be loaded",
        }
    }

    fn explanation(&self) -> &'static str {
        match self.kind {
            LibraryLoadFailureKind::UpgradeRequired { .. } => {
                "EmuWiz can see the existing library, but this version cannot open it until its database format is upgraded. Nothing has been changed and no games have been removed."
            }
            LibraryLoadFailureKind::NewerThanThisBuild { .. } => {
                "EmuWiz can see the existing library, but it was saved by a newer version than this one. Nothing has been changed and no games have been removed."
            }
            LibraryLoadFailureKind::Other => {
                "EmuWiz could not read its saved game list. This does not mean you have no games. Your game files were not changed."
            }
        }
    }
}

fn number_after(text: &str, marker: &str) -> Option<i64> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Routes whose content is derived from the library list.
pub(super) fn needs_library(route: &super::Route) -> bool {
    use super::Route;
    matches!(
        route,
        Route::BrowsePlay
            | Route::BrowsePlayGame(_)
            | Route::Section(
                Section::Games
                    | Section::Launch
                    | Section::Platforms
                    | Section::Museum
                    | Section::MultiDisc
            )
    )
}

enum Choice {
    Retry,
    OpenUpgradeTools,
}

impl App {
    pub(super) fn library_load_failed(&mut self, error: &str) {
        self.library_failure = Some(LibraryLoadFailure::from_error(error));
    }

    pub(super) fn retry_library_load(&mut self) {
        // `load` already refuses to queue a second job while one is running.
        self.load(false);
    }

    /// Draws the failure panel in place of the page when no library has ever
    /// loaded (returns `true`: the caller must not draw the page), or a slim
    /// banner above the page when a last-known-good library is still shown.
    pub(super) fn show_library_failure(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(failure) = self.library_failure.clone() else {
            return false;
        };
        let retained = self.loaded;
        let mut choice = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            if retained {
                ui.strong("The game list could not be refreshed");
                ui.label(
                    "Showing the last game list that loaded. Your game files were not changed.",
                );
            } else {
                ui.heading(failure.heading());
                ui.label(failure.explanation());
            }
            if let LibraryLoadFailureKind::UpgradeRequired { found, required }
            | LibraryLoadFailureKind::NewerThanThisBuild { found, required } = failure.kind
            {
                ui.label(format!(
                    "Library format: {found} · this version requires: {required}"
                ));
            }
            ui.horizontal_wrapped(|ui| {
                let can_retry = self.load_job.is_none();
                match failure.kind {
                    LibraryLoadFailureKind::UpgradeRequired { .. } => {
                        if primary(ui, "Open upgrade tools") {
                            choice = Some(Choice::OpenUpgradeTools);
                        }
                        if ui
                            .add_enabled(can_retry, egui::Button::new("Check again"))
                            .clicked()
                        {
                            choice = Some(Choice::Retry);
                        }
                    }
                    _ => {
                        if ui
                            .add_enabled(can_retry, egui::Button::new("Retry"))
                            .clicked()
                        {
                            choice = Some(Choice::Retry);
                        }
                    }
                }
            });
            egui::CollapsingHeader::new("Technical details")
                .id_salt("library_load_failure_details")
                .show(ui, |ui| {
                    ui.label(&failure.technical);
                });
        });
        match choice {
            Some(Choice::Retry) => self.retry_library_load(),
            // The existing specialist window holds the backup-first upgrade; this
            // only opens it and never migrates anything itself.
            Some(Choice::OpenUpgradeTools) => self.legacy(Section::Advanced),
            None => {}
        }
        !retained
    }
}
