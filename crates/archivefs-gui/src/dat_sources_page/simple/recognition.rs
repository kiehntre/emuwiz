//! "Which systems your catalogues are for": the recognition summary, its
//! preview and the one "Apply all safe matches" step.
//!
//! Nothing here changes a setting by itself. Checking catalogues reads only
//! their headers; the summary only describes; Apply sets exactly the systems
//! the preview lists, after the core re-checks the settings file and every
//! catalogue. No game file is read and no verification is started.
use super::*;
use archivefs_core::dat::catalogue_recognition::CatalogueRecognition;
use archivefs_core::dat::sources::recognition::{
    RecognitionGroup, RecognitionRow, RecognitionState,
};
use archivefs_core::platform::display_name_for;

#[derive(Default)]
pub(crate) struct RecognitionUi {
    pub(super) preview_open: bool,
}

fn count(rows: &[RecognitionRow], group: RecognitionGroup) -> usize {
    rows.iter().filter(|row| row.group == group).count()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The plain-English line for one catalogue.
pub(super) fn row_text(row: &RecognitionRow) -> String {
    let header = match &row.state {
        RecognitionState::Recognised(recognition) => recognition.describe(),
        RecognitionState::Folder => {
            "A folder of several catalogues; choose a single catalogue file to use it".to_string()
        }
        RecognitionState::NotChecked => "Its system hasn\u{2019}t been checked yet".to_string(),
        RecognitionState::Changed => "The file changed since it was checked".to_string(),
    };
    match (row.group, &row.assigned) {
        (RecognitionGroup::Conflict, Some(assigned)) => {
            format!("You chose {assigned}, but the catalogue says: {header}. Nothing was changed.")
        }
        (RecognitionGroup::AlreadySet, Some(assigned)) if row.assigned_by_recognition => {
            format!("Used for {assigned} (recognised from the catalogue)")
        }
        (RecognitionGroup::AlreadySet, Some(assigned)) => {
            format!("Used for {assigned} (your choice)")
        }
        (RecognitionGroup::TurnedOff, _) => format!("Turned off. {header}"),
        _ => header,
    }
}

/// The summary card. Returns the action a button asked for.
pub(super) fn card(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    state: &mut RecognitionUi,
) -> Option<DatSourcesPageAction> {
    let recognition = &view.recognition;
    let rows = &recognition.rows;
    if rows.is_empty() {
        return None;
    }
    let busy = view.background_busy || view.running.is_some();
    let mut action = None;
    widgets::full_width_card(ui, |ui| {
        ui.strong("Which systems your catalogues are for");
        let needs_check = count(rows, RecognitionGroup::NeedsCheck);
        let safe = recognition.safe.len();
        let lines = [
            (
                safe,
                "ordinary game list is ready to set",
                "ordinary game lists are ready to set",
            ),
            (
                count(rows, RecognitionGroup::AlreadySet),
                "catalogue already has its system",
                "catalogues already have their system",
            ),
            (
                count(rows, RecognitionGroup::Extras),
                "catalogue has extras for a system (only used if you choose it)",
                "catalogues have extras for a system (only used if you choose them)",
            ),
            (
                count(rows, RecognitionGroup::Ambiguous),
                "catalogue could be for more than one system",
                "catalogues could be for more than one system",
            ),
            (
                count(rows, RecognitionGroup::Conflict),
                "catalogue says a different system from the one you chose (not changed)",
                "catalogues say a different system from the one you chose (not changed)",
            ),
            (
                count(rows, RecognitionGroup::CannotUse),
                "catalogue can\u{2019}t be used to check single games (several systems, not a game list, or a system EmuWiz doesn\u{2019}t support yet)",
                "catalogues can\u{2019}t be used to check single games (several systems, not game lists, or systems EmuWiz doesn\u{2019}t support yet)",
            ),
            (
                count(rows, RecognitionGroup::TurnedOff),
                "catalogue is turned off",
                "catalogues are turned off",
            ),
            (
                needs_check,
                "catalogue hasn\u{2019}t had its system checked yet",
                "catalogues haven\u{2019}t had their system checked yet",
            ),
        ];
        for (n, one, many) in lines {
            if n > 0 {
                ui.label(format!("\u{2022} {}", plural(n, one, many)));
            }
        }
        if needs_check > 0 {
            ui.label(
                egui::RichText::new(
                    "Checking reads only the catalogue files, never your games, and changes no setting.",
                )
                .color(theme::muted(ui)),
            );
            if ui
                .add_enabled(!busy, egui::Button::new("Check catalogues"))
                .clicked()
            {
                action = Some(DatSourcesPageAction::ValidateAll);
            }
        }
        match &recognition.message {
            Some(Ok(message)) => {
                ui.colored_label(theme::SUCCESS, message);
                if recognition.can_undo
                    && ui.add_enabled(!busy, egui::Button::new("Undo")).clicked()
                {
                    action = Some(DatSourcesPageAction::UndoRecognisedSystems);
                }
            }
            Some(Err(message)) => {
                ui.colored_label(theme::WARNING, message);
                if ui
                    .add_enabled(!busy, egui::Button::new("Reload setup"))
                    .on_hover_text(
                        "Reads your saved setup again. Unsaved changes on this page are discarded.",
                    )
                    .clicked()
                {
                    action = Some(DatSourcesPageAction::ReloadSetup);
                }
            }
            None => {}
        }
        if safe > 0 {
            if !state.preview_open {
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(format!(
                            "Review {}",
                            plural(safe, "safe match", "safe matches")
                        )),
                    )
                    .clicked()
                {
                    state.preview_open = true;
                }
            } else {
                ui.separator();
                ui.strong("These systems will be set");
                ui.label(
                    "Only these catalogues change. No catalogue is turned on or off, no game is read or checked, and saved check results are kept.",
                );
                egui::ScrollArea::vertical()
                    .id_salt("recognition-preview")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for item in &recognition.safe {
                            ui.label(format!(
                                "{} \u{2192} {}",
                                item.display_name,
                                display_name_for(item.platform)
                            ));
                        }
                    });
                ui.horizontal(|ui| {
                    if primary_button(ui, &format!("Apply all safe matches ({safe})"), !busy)
                        .clicked()
                    {
                        action = Some(DatSourcesPageAction::ApplyRecognisedSystems {
                            preview: recognition.safe.clone(),
                        });
                        state.preview_open = false;
                    }
                    if ui.button("Cancel").clicked() {
                        state.preview_open = false;
                    }
                });
            }
        } else {
            state.preview_open = false;
        }
        widgets::technical_details(ui, "recognition-details", |ui| {
            for row in rows {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{}: {}", row.display_name, row_text(row)));
                    if row.group == RecognitionGroup::Extras
                        && let RecognitionState::Recognised(CatalogueRecognition::Extras {
                            platform,
                            ..
                        }) = &row.state
                        && ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(format!(
                                    "Use for {}",
                                    display_name_for(platform)
                                )),
                            )
                            .clicked()
                    {
                        action = Some(DatSourcesPageAction::AssignPlatformNow {
                            id: row.source_id.clone(),
                            platform: platform.to_string(),
                        });
                    }
                });
            }
        });
    });
    action
}
