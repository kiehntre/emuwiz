//! Read-only review of a core-produced cheat reconciliation report.
//!
//! The core reconciliation service remains the authority. This page only
//! loads its JSON result, presents provenance and records review choices
//! locally; it never installs, enables, deletes, or selects a winner.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use archivefs_core::patch_manager::{
    CheatOperation, CheatReconciliationGroup, CheatReconciliationResult, CheatRelationship,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewChoice {
    KeepA,
    KeepB,
    KeepBoth,
    Skip,
    IgnoreConflict,
}

#[derive(Default)]
pub(crate) struct CheatReconciliationReviewState {
    report: Option<Result<CheatReconciliationResult, String>>,
    source_path: Option<PathBuf>,
    choices: BTreeMap<usize, ReviewChoice>,
    show_all: bool,
}

impl CheatReconciliationReviewState {
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        widgets::section_header(
            ui,
            "Reconciliation / Review conflicts",
            Some(
                "Review duplicate and conflicting cheats before any separate preview or apply step.",
            ),
        );
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Open reconciliation report",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                self.load_report();
            }
            if self.report.is_some()
                && widgets::action_button(ui, "Clear", widgets::ActionStyle::Secondary, true)
                    .clicked()
            {
                self.report = None;
                self.source_path = None;
                self.choices.clear();
            }
        });

        match self.report.as_ref() {
            None => {
                ui.label("Load a core-generated reconciliation JSON report to review it here.");
                ui.weak("Review is read-only: no source is changed and no automatic winner is selected.");
            }
            Some(Err(error)) => widgets::banner(
                ui,
                "Reconciliation report unavailable",
                error,
                widgets::StatusTone::Warning,
            ),
            Some(Ok(result)) => {
                let result = result.clone();
                self.show_report(ui, &result);
            }
        }
        ui.add_space(theme::SECTION_GAP);
    }

    fn load_report(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Reconciliation JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let report = fs::read(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))
            .and_then(|bytes| {
                serde_json::from_slice(&bytes)
                    .map_err(|error| format!("invalid reconciliation report: {error}"))
            });
        self.report = Some(report);
        self.source_path = Some(path);
        self.choices.clear();
    }

    fn show_report(&mut self, ui: &mut egui::Ui, result: &CheatReconciliationResult) {
        let duplicates = result
            .groups
            .iter()
            .filter(|group| {
                matches!(
                    group.relationship,
                    CheatRelationship::ExactSemanticDuplicate
                        | CheatRelationship::ExactRawDuplicate
                )
            })
            .count();
        let conflicts = result
            .groups
            .iter()
            .filter(|group| {
                matches!(
                    group.relationship,
                    CheatRelationship::SameTitleDifferentCode
                )
            })
            .count();
        let malformed = result
            .groups
            .iter()
            .filter(|group| self.group_is_unsupported(group, result))
            .count();
        widgets::status_strip(
            ui,
            &[
                (
                    "Conflicts",
                    if conflicts > 0 {
                        widgets::StatusTone::Warning
                    } else {
                        widgets::StatusTone::Success
                    },
                ),
                ("Duplicates", widgets::StatusTone::Info),
                (
                    "Malformed/unsupported",
                    if malformed > 0 {
                        widgets::StatusTone::Warning
                    } else {
                        widgets::StatusTone::Pending
                    },
                ),
            ],
        );
        ui.label(format!("{conflicts} conflicts · {duplicates} duplicate groups · {malformed} malformed/unsupported"));
        ui.weak(format!(
            "Game: {} · Platform: {:?}",
            result.game_identity, result.platform
        ));
        if let Some(path) = &self.source_path {
            ui.weak(format!("Report: {}", path.display()));
        }
        ui.checkbox(&mut self.show_all, "Show unique entries");

        for (group_index, group) in result.groups.iter().enumerate() {
            let important = !matches!(group.relationship, CheatRelationship::Unique)
                || self.group_is_unsupported(group, result);
            if !self.show_all && !important {
                continue;
            }
            self.show_group(ui, result, group_index, group);
        }
        if result
            .groups
            .iter()
            .all(|group| matches!(group.relationship, CheatRelationship::Unique))
            && !self.show_all
        {
            ui.label("No duplicate, conflict, malformed, or unsupported groups were found.");
        }
    }

    fn show_group(
        &mut self,
        ui: &mut egui::Ui,
        result: &CheatReconciliationResult,
        group_index: usize,
        group: &CheatReconciliationGroup,
    ) {
        let title = result
            .entries
            .get(group.entry_indices[0])
            .map(|entry| entry.title.as_str())
            .unwrap_or(&group.normalized_title);
        let heading = match group.relationship {
            CheatRelationship::ExactSemanticDuplicate | CheatRelationship::ExactRawDuplicate => {
                "These cheats do the same thing"
            }
            CheatRelationship::SameTitleDifferentCode => "These cheats conflict",
            CheatRelationship::RelatedUnproven => "EmuWiz cannot safely compare these",
            CheatRelationship::Unique => "Unique cheat",
        };
        widgets::card(ui, |ui| {
            ui.strong(title);
            ui.label(heading);
            for difference in &group.differences {
                ui.weak(format!("Difference: {difference}"));
            }
            for (position, index) in group.entry_indices.iter().enumerate() {
                let Some(entry) = result.entries.get(*index) else {
                    continue;
                };
                ui.separator();
                ui.label(format!("Source {}: {}", position + 1, entry.source));
                ui.label(format!(
                    "Game: {} · Format: {:?}",
                    entry.title, entry.source_format
                ));
                for provenance in &entry.provenance {
                    ui.weak(format!("Provenance: {provenance}"));
                }
                for issue in &entry.document.issues {
                    ui.colored_label(theme::WARNING, format!("Issue: {issue:?}"));
                }
                for operation in &entry.document.operations {
                    if let CheatOperation::UnsupportedRaw { reason, .. } = operation {
                        ui.colored_label(theme::WARNING, format!("Unsupported: {reason}"));
                    }
                }
            }
            if matches!(
                group.relationship,
                CheatRelationship::SameTitleDifferentCode
            ) {
                ui.horizontal_wrapped(|ui| {
                    self.choice_button(ui, group_index, ReviewChoice::KeepA, "Keep A");
                    self.choice_button(ui, group_index, ReviewChoice::KeepB, "Keep B");
                    self.choice_button(ui, group_index, ReviewChoice::KeepBoth, "Keep both");
                    self.choice_button(ui, group_index, ReviewChoice::Skip, "Skip");
                    self.choice_button(
                        ui,
                        group_index,
                        ReviewChoice::IgnoreConflict,
                        "Ignore conflict",
                    );
                });
            }
        });
    }

    fn choice_button(
        &mut self,
        ui: &mut egui::Ui,
        group: usize,
        choice: ReviewChoice,
        label: &str,
    ) {
        if ui
            .selectable_label(self.choices.get(&group) == Some(&choice), label)
            .clicked()
        {
            self.choices.insert(group, choice);
        }
    }

    fn group_is_unsupported(
        &self,
        group: &CheatReconciliationGroup,
        result: &CheatReconciliationResult,
    ) -> bool {
        group.entry_indices.iter().any(|index| {
            result.entries.get(*index).is_some_and(|entry| {
                !entry.document.issues.is_empty()
                    || entry
                        .document
                        .operations
                        .iter()
                        .any(|operation| matches!(operation, CheatOperation::UnsupportedRaw { .. }))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_state_starts_compact_without_report_or_auto_winner() {
        let state = CheatReconciliationReviewState::default();
        assert!(state.report.is_none());
        assert!(state.choices.is_empty());
        assert!(!state.show_all);
    }
}
