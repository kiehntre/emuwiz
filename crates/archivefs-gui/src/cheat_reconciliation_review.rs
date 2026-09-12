//! Read-only review of a core-produced cheat reconciliation report.
//!
//! The core reconciliation service remains the authority. This page only
//! loads its JSON result, presents provenance and records review choices
//! locally; it never installs, enables, deletes, or selects a winner.

use std::collections::BTreeMap;
use std::path::PathBuf;

use archivefs_core::patch_manager::{
    CheatOperation, CheatReconciliationGroup, CheatReconciliationResult, CheatRelationship,
    CheatReviewChoice, ResolvedCheatApplyEligibility, ResolvedCheatPlan, ResolvedCheatPlanRequest,
    resolve_reviewed_cheat_plan,
};
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::ui::{components as widgets, theme};

mod persistence;
use persistence::{ReviewStore, read_report};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    report_sha256: Option<String>,
    saved_choices: BTreeMap<usize, ReviewChoice>,
    choices_loaded: bool,
    persistence_warning: Option<String>,
    resolved_plan: Option<ResolvedCheatPlan>,
    emulator: String,
    profile: String,
    // Resolve lazily on explicit report open. Default/test construction must
    // never read the user's state directory or start a background operation.
    store: Option<ReviewStore>,
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
                && widgets::action_button(ui, "Close report", widgets::ActionStyle::Secondary, true)
                    .clicked()
            {
                self.close_report();
            }
        });

        if let Some(warning) = &self.persistence_warning {
            widgets::banner(
                ui,
                "Review choices not saved or restored",
                warning,
                widgets::StatusTone::Warning,
            );
            if ui.button("Retry saving choices").clicked() {
                self.save_choices();
            }
        }

        match self.report.as_ref() {
            None => {
                ui.label("Load a core-generated reconciliation JSON report to review it here.");
                ui.weak("Review is read-only: no source is changed and no automatic winner is selected.");
                ui.weak("Reopen the same report to restore saved review choices.");
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
        self.open_report(path);
    }

    fn close_report(&mut self) {
        let store = self.store.take();
        *self = Self {
            store,
            ..Self::default()
        };
    }

    fn open_report(&mut self, path: PathBuf) {
        self.close_report();
        self.source_path = Some(path.clone());
        match read_report(&path) {
            Err(error) => self.report = Some(Err(error)),
            Ok((report, digest)) => {
                if self.store.is_none() {
                    match archivefs_core::app_dirs::data_path("cheat-reconciliation-reviews") {
                        Ok(root) => self.store = Some(ReviewStore::new(root)),
                        Err(error) => self.persistence_warning = Some(error.to_string()),
                    }
                }
                if let Some(store) = &self.store {
                    match store.load(&digest, &report) {
                        Ok(choices) => {
                            self.choices = choices.clone();
                            self.saved_choices = choices;
                            self.choices_loaded = true;
                        }
                        Err(error) => self.persistence_warning = Some(error),
                    }
                }
                self.report_sha256 = Some(digest);
                self.report = Some(Ok(report));
            }
        }
    }

    fn save_choices(&mut self) {
        let result = (|| {
            let report = self
                .report
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .ok_or("Open a valid report before saving review choices.")?;
            let path = self.source_path.as_ref().ok_or("No report is open.")?;
            let digest = self
                .report_sha256
                .as_deref()
                .ok_or("No report identity is available.")?;
            // A saved choice describes only the report the user actually saw.
            // A changed/missing source is never silently rebound by title/path.
            let (_, current_digest) = read_report(path)?;
            if current_digest != digest {
                return Err("The report changed on disk. Reopen it before reviewing; previous choices belong to the earlier report.".to_string());
            }
            self.store
                .as_ref()
                .ok_or("The local review storage is unavailable.")?
                .save(digest, report, &self.saved_choices, &self.choices)
        })();
        match result {
            Ok(()) => {
                self.saved_choices = self.choices.clone();
                self.persistence_warning = None;
            }
            Err(error) => self.persistence_warning = Some(error),
        }
    }

    fn choose(&mut self, group: usize, choice: ReviewChoice) {
        let Some(Ok(report)) = &self.report else {
            return;
        };
        if !choice_allowed(report, group, choice) {
            return;
        }
        self.choices.insert(group, choice);
        self.resolved_plan = None;
        // This choice is fresh, not a restored one - the "Saved review
        // choices loaded" message must not keep claiming otherwise once the
        // user has actively decided something this session.
        self.choices_loaded = false;
        self.save_choices();
    }

    fn build_resolved_plan(&mut self, report: &CheatReconciliationResult) {
        let Some(source_report_digest) = self.report_sha256.clone() else {
            return;
        };
        let choices = self
            .choices
            .iter()
            .map(|(index, choice)| {
                (
                    *index,
                    match choice {
                        ReviewChoice::KeepA => CheatReviewChoice::KeepA,
                        ReviewChoice::KeepB => CheatReviewChoice::KeepB,
                        ReviewChoice::KeepBoth => CheatReviewChoice::KeepBoth,
                        ReviewChoice::Skip => CheatReviewChoice::Skip,
                        ReviewChoice::IgnoreConflict => CheatReviewChoice::IgnoreConflict,
                    },
                )
            })
            .collect();
        let request = ResolvedCheatPlanRequest {
            source_report_digest,
            emulator: self.emulator.trim().to_string(),
            profile: self.profile.trim().to_string(),
            target_file: None,
            existing_file_digest: None,
            destination_changed: false,
        };
        self.resolved_plan = Some(resolve_reviewed_cheat_plan(report, &choices, &request));
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
        let unresolved = conflicts.saturating_sub(self.choices.len());
        let ready = result
            .entries
            .iter()
            .filter(|entry| entry.identity_verified && entry.document.issues.is_empty())
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
        ui.strong("Cheat review");
        ui.label(format!(
            "{} entries found · {ready} ready · {conflicts} conflicts · {duplicates} duplicates · {malformed} malformed/unsupported",
            result.entries.len()
        ));
        ui.label(if unresolved == 0 {
            "All conflicts have a saved choice or no conflict needs review."
        } else {
            "Some conflicts still need a decision before the preview is complete."
        });
        ui.weak(format!(
            "Game: {} · Platform: {:?}",
            result.game_identity, result.platform
        ));
        if let Some(path) = &self.source_path {
            ui.weak(format!("Report: {}", path.display()));
        }
        ui.checkbox(&mut self.show_all, "Show unique entries");
        if self.persistence_warning.is_none() {
            if self.choices_loaded {
                ui.weak("Saved review choices loaded. They apply only to this unchanged report.");
            } else {
                ui.weak(format!("{} review choices saved locally. Reopen this unchanged report to continue after restarting.", self.saved_choices.len()));
            }
        }
        ui.weak("Review choices never install or enable cheats.");

        ui.separator();
        ui.strong("Read-only resolved preview");
        let previous_target = (self.emulator.clone(), self.profile.clone());
        ui.horizontal_wrapped(|ui| {
            ui.label("Emulator/profile:");
            ui.add(egui::TextEdit::singleline(&mut self.emulator).desired_width(150.0));
            ui.add(egui::TextEdit::singleline(&mut self.profile).desired_width(150.0));
            if ui
                .add_enabled(
                    !self.emulator.trim().is_empty() && !self.profile.trim().is_empty(),
                    egui::Button::new("Build exact preview"),
                )
                .clicked()
            {
                self.build_resolved_plan(result);
            }
        });
        if previous_target != (self.emulator.clone(), self.profile.clone()) {
            self.resolved_plan = None;
        }
        if let Some(plan) = &self.resolved_plan {
            show_resolved_plan_preview(ui, plan);
        } else {
            ui.weak("Build a preview after reviewing conflicts. A preview never writes source or emulator files.");
        }

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
            .get(group.entry_indices.first().copied().unwrap_or(usize::MAX))
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
                    self.choice_button(ui, result, group_index, ReviewChoice::KeepA, "Keep A");
                    self.choice_button(ui, result, group_index, ReviewChoice::KeepB, "Keep B");
                    self.choice_button(
                        ui,
                        result,
                        group_index,
                        ReviewChoice::KeepBoth,
                        "Keep both",
                    );
                    self.choice_button(ui, result, group_index, ReviewChoice::Skip, "Skip");
                    self.choice_button(
                        ui,
                        result,
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
        result: &CheatReconciliationResult,
        group: usize,
        choice: ReviewChoice,
        label: &str,
    ) {
        if ui
            .add_enabled_ui(choice_allowed(result, group, choice), |ui| {
                ui.selectable_label(self.choices.get(&group) == Some(&choice), label)
            })
            .inner
            .clicked()
        {
            self.choose(group, choice);
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

fn show_resolved_plan_preview(ui: &mut egui::Ui, plan: &ResolvedCheatPlan) {
    widgets::card(ui, |ui| {
        ui.strong("Resolved cheat plan");
        ui.label(format!("Target game: {}", plan.game_identity));
        ui.label(format!(
            "Emulator/profile: {}/{}",
            plan.emulator, plan.profile
        ));
        ui.label(format!(
            "Included: {} · skipped: {} · ignored: {} · unresolved: {}",
            plan.selected_entries.len(),
            plan.skipped_entries.len(),
            plan.ignored_entries.len(),
            plan.unresolved_conflicts.len()
        ));
        ui.label(format!(
            "Duplicate collapses: {}",
            plan.selected_entries
                .iter()
                .filter(|entry| entry.duplicate_entry_indices.len() > 1)
                .count()
        ));
        ui.label(format!(
            "Malformed: {} · unsupported: {}",
            plan.malformed_entries.len(),
            plan.unsupported_entries.len()
        ));
        widgets::technical_details(ui, "resolved-cheat-plan-details", |ui| {
            ui.label(format!("Report revision: {}", plan.source_report_digest));
            ui.label(format!(
                "Review choices revision: {}",
                plan.review_choices_digest
            ));
            ui.label(format!("Destination: {:?}", plan.destination.target_file));
            for entry in &plan.selected_entries {
                ui.label(format!(
                    "Included: {} · provenance: {}",
                    entry.title,
                    entry.provenance.join(", ")
                ));
            }
            for diagnostic in plan
                .skipped_entries
                .iter()
                .chain(plan.ignored_entries.iter())
                .chain(plan.unresolved_conflicts.iter())
                .chain(plan.malformed_entries.iter())
                .chain(plan.unsupported_entries.iter())
            {
                ui.label(format!(
                    "Excluded: {} — {}",
                    diagnostic.title, diagnostic.reason
                ));
            }
        });
        match &plan.apply_eligibility {
            ResolvedCheatApplyEligibility::PreviewOnly { reason } => {
                widgets::status_badge(ui, "Preview only", widgets::StatusTone::Info);
                ui.label(format!("Automatic installation is unavailable: {reason}"));
            }
            ResolvedCheatApplyEligibility::Blocked { reasons } => {
                widgets::status_badge(ui, "Apply unavailable", widgets::StatusTone::Blocked);
                for reason in reasons {
                    ui.label(format!("Reason: {reason}"));
                }
            }
        }
    });
}

fn choice_allowed(report: &CheatReconciliationResult, index: usize, choice: ReviewChoice) -> bool {
    let Some(group) = report.groups.get(index) else {
        return false;
    };
    if group.relationship != CheatRelationship::SameTitleDifferentCode {
        return false;
    }
    if matches!(choice, ReviewChoice::Skip | ReviewChoice::IgnoreConflict) {
        return true;
    }
    // A/B only names an unambiguous pair. Retaining a review decision must
    // never turn malformed/unsupported evidence into an installable selection.
    group.entry_indices.len() == 2
        && group.entry_indices.iter().all(|index| {
            report.entries.get(*index).is_some_and(|entry| {
                entry.document.issues.is_empty()
                    && !entry.document.operations.is_empty()
                    && !entry
                        .document
                        .operations
                        .iter()
                        .any(|op| matches!(op, CheatOperation::UnsupportedRaw { .. }))
            })
        })
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
