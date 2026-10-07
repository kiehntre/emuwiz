//! Read-only presentation of the existing planner and matcher; no election or I/O.
use super::*;
use archivefs_core::playing_library::matching::MatchOutcome;
use std::collections::HashSet;

// Match the existing GUI's 200-row presentation bound; totals use the whole plan.
pub(super) const ROW_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PreviewFilter {
    #[default]
    All,
    Selected,
    NeedsReview,
    Conflicts,
    Excluded,
}
impl PreviewFilter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Selected,
        Self::NeedsReview,
        Self::Conflicts,
        Self::Excluded,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Selected => "Selected",
            Self::NeedsReview => "Needs review",
            Self::Conflicts => "Conflicts",
            Self::Excluded => "Excluded",
        }
    }
    fn includes(self, kind: Self) -> bool {
        self == Self::All || self == kind
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ReviewCounts {
    pub selected: usize,
    pub excluded: usize,
    pub needs_review: usize,
    pub conflicts: usize,
}
impl ReviewCounts {
    pub fn from_state(plan: &PlayingLibraryPlan, state: &PlayingLibraryPageState) -> Self {
        Self {
            selected: plan.elected_games.len(),
            excluded: plan.exclusions.len()
                + plan
                    .elected_games
                    .iter()
                    .map(|g| g.explanation.rejected.len())
                    .sum::<usize>(),
            needs_review: plan.unresolved_groups.len()
                + plan.rejected_launchers.len()
                + state.review_paths.len(),
            conflicts: plan.conflicts.len(),
        }
    }
    pub fn visible(&self, filter: PreviewFilter) -> usize {
        match filter {
            PreviewFilter::All => {
                self.selected + self.excluded + self.needs_review + self.conflicts
            }
            PreviewFilter::Selected => self.selected,
            PreviewFilter::Excluded => self.excluded,
            PreviewFilter::NeedsReview => self.needs_review,
            PreviewFilter::Conflicts => self.conflicts,
        }
    }
}

/// Candidate files not represented by a match or a reported rejected launcher.
/// This does not distinguish ambiguity, absent checksums, read failures or no match.
/// Companions of accepted releases are already represented by their launcher.
pub(super) fn unrepresented_paths(candidates: &[PathBuf], outcome: &MatchOutcome) -> Vec<PathBuf> {
    let represented: HashSet<&PathBuf> = outcome
        .matches
        .iter()
        .flat_map(|m| std::iter::once(&m.archive_path).chain(m.companion_paths.iter()))
        .chain(outcome.rejected_launchers.iter().map(|r| &r.launcher_path))
        .collect();
    candidates
        .iter()
        .filter(|p| !represented.contains(p))
        .cloned()
        .collect()
}

/// A bounded slice of the existing projection, with a presentation-only cursor.
fn window<'a, T>(rows: &'a [T], skip: &mut usize, remaining: &mut usize) -> &'a [T] {
    let start = (*skip).min(rows.len());
    *skip -= start;
    let length = (rows.len() - start).min(*remaining);
    *remaining -= length;
    &rows[start..start + length]
}

fn filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

pub(super) fn show_preview(
    ui: &mut egui::Ui,
    plan: &PlayingLibraryPlan,
    state: &PlayingLibraryPageState,
    action: &mut Option<PlayingLibraryPageAction>,
) {
    let counts = ReviewCounts::from_state(plan, state);
    let filter = state.preview_filter;
    widgets::card(ui, |ui| {
        ui.strong("Preview summary");
        ui.label(format!(
            "Selected: {} · Excluded releases: {} · Needs review items: {} · Conflicts: {}",
            counts.selected, counts.excluded, counts.needs_review, counts.conflicts
        ));
        ui.label(format!("Destination: {}", plan.destination_root.display()));
        ui.label("Output: links to originals, not copies. Preview reads only; original ROMs remain unchanged. Undo is available for supported applied operations.");
        ui.label("Only unique checksum matches enter selection. Filename-only, unknown or ambiguous identity is not trusted. Needs review items can be files or unresolved groups.");
        if plan.elected_games.is_empty() {
            ui.label(if state.source_files_examined == 0 {
                "No games available to organise in the selected source folder."
            } else {
                "No verified games eligible for this preview. Review identity or the existing preferences."
            });
        }
        if filter == PreviewFilter::Conflicts && counts.conflicts == 0 {
            ui.label("No conflicts found in this preview.");
        } else if counts.visible(filter) == 0 && filter != PreviewFilter::All {
            ui.label("Filters hide all results. Choose All to see the complete preview.");
        }
        if counts.visible(filter) > ROW_LIMIT {
            let start = state
                .preview_page
                .min(counts.visible(filter).div_ceil(ROW_LIMIT) - 1)
                * ROW_LIMIT;
            ui.label(format!("Showing review items {}–{} of {}. Counts include the whole preview; use the review page controls or filters.", start + 1, (start + ROW_LIMIT).min(counts.visible(filter)), counts.visible(filter)));
        }
        let pages = counts.visible(filter).div_ceil(ROW_LIMIT).max(1);
        let mut skip = state.preview_page.min(pages - 1) * ROW_LIMIT;
        let mut remaining = ROW_LIMIT;
        if filter.includes(PreviewFilter::Selected) {
            for elected in window(&plan.elected_games, &mut skip, &mut remaining) {
                ui.push_id(("playing-family", &elected.dat_entry_name), |ui| {
                    ui.strong(format!(
                        "{} · Selected · Verified checksum identity",
                        elected.dat_entry_name
                    ));
                    if elected.explanation.steps.is_empty() {
                        ui.label(
                            "Selected by the existing planner; its evidence is available below.",
                        );
                    } else {
                        for step in elected.explanation.steps.iter().take(ROW_LIMIT) {
                            ui.label(format!("EmuWiz prefers this release: {step}"));
                        }
                    }
                    let selected = state.selected_family() == Some(elected.dat_entry_name.as_str());
                    if widgets::action_button(
                        ui,
                        if selected { "Hide" } else { "Why this one?" },
                        widgets::ActionStyle::Quiet,
                        true,
                    )
                    .clicked()
                    {
                        *action = Some(PlayingLibraryPageAction::SelectFamily(if selected {
                            None
                        } else {
                            Some(elected.dat_entry_name.clone())
                        }));
                    }
                    if selected {
                        ui.label(evidence_summary_line(&elected.explanation.winner_evidence));
                        for rejected in elected.explanation.rejected.iter().take(ROW_LIMIT) {
                            ui.label(format!(
                                "Not selected: {} — {}",
                                rejected.dat_entry_name,
                                rejected.reasons.join("; ")
                            ));
                        }
                    }
                    widgets::technical_details(
                        ui,
                        ("playing-library-evidence", &elected.dat_entry_name),
                        |ui| {
                            ui.label(format!(
                                "DAT entry: {} · Family: {}",
                                elected.dat_entry_name, elected.family_root_name
                            ));
                            ui.label(format!(
                                "Evidence: {:?}",
                                elected.explanation.winner_evidence
                            ));
                            for operation in elected.all_operations().take(ROW_LIMIT) {
                                ui.label(format!(
                                    "Source: {} → Output: {}",
                                    operation.source_path.display(),
                                    operation.destination_path.display()
                                ));
                            }
                            for rejected in elected.explanation.rejected.iter().take(ROW_LIMIT) {
                                ui.label(format!(
                                    "Not selected: {} · {:?} · {}",
                                    rejected.dat_entry_name,
                                    rejected.evidence,
                                    rejected.reasons.join("; ")
                                ));
                            }
                        },
                    );
                });
            }
        }
        if filter.includes(PreviewFilter::NeedsReview) {
            for group in window(&plan.unresolved_groups, &mut skip, &mut remaining) {
                ui.push_id(("unresolved-family", &group.family_root_name), |ui| {
                    ui.colored_label(theme::WARNING, format!("{} · Needs review", group.family_root_name));
                    ui.label("Identity or preference evidence is not strong enough to choose automatically.");
                    widgets::technical_details(ui, "unresolved-evidence", |ui| { ui.label(&group.reason); ui.label(group.tied_candidates.join(", ")); });
                });
            }
            for rejected in window(&plan.rejected_launchers, &mut skip, &mut remaining) {
                ui.push_id(("rejected-launcher", &rejected.launcher_path), |ui| {
                    ui.colored_label(
                        theme::WARNING,
                        format!("{} · Needs review", filename(&rejected.launcher_path)),
                    );
                    ui.label(&rejected.reason);
                    widgets::technical_details(ui, "launcher-path", |ui| {
                        ui.label(rejected.launcher_path.display().to_string());
                    });
                });
            }
            for path in window(&state.review_paths, &mut skip, &mut remaining) {
                ui.push_id(("unverified-file", path), |ui| {
                    ui.colored_label(theme::WARNING, format!("{} · Needs review", filename(path)));
                    ui.label(
                        "This preview did not establish a unique checksum identity. Not selected.",
                    );
                    widgets::technical_details(ui, "unverified-path", |ui| {
                        ui.label(path.display().to_string());
                    });
                });
            }
        }
        if filter.includes(PreviewFilter::Conflicts) {
            for conflict in window(&plan.conflicts, &mut skip, &mut remaining) {
                ui.push_id(("destination-conflict", &conflict.destination_basename), |ui| {
                    ui.colored_label(theme::WARNING, format!("{} · Conflict", conflict.destination_basename));
                    ui.label("Multiple selected releases target the same destination. Apply must wait for review.");
                    widgets::technical_details(ui, "conflict-evidence", |ui| { ui.label(format!("{conflict:?}")); });
                });
            }
        }
        if filter.includes(PreviewFilter::Excluded) {
            for excluded in window(&plan.exclusions, &mut skip, &mut remaining) {
                ui.push_id(
                    (
                        "excluded-release",
                        &excluded.dat_entry_name,
                        &excluded.source_path,
                    ),
                    |ui| {
                        ui.label(format!(
                            "{} · Excluded: {}",
                            excluded.dat_entry_name,
                            excluded.excluded_classes.join(", ")
                        ));
                        widgets::technical_details(ui, "excluded-source", |ui| {
                            ui.label(excluded.source_path.display().to_string());
                        });
                    },
                );
            }
            for elected in &plan.elected_games {
                for rejected in window(&elected.explanation.rejected, &mut skip, &mut remaining) {
                    ui.push_id(
                        (
                            "alternative-release",
                            &elected.dat_entry_name,
                            &rejected.dat_entry_name,
                        ),
                        |ui| {
                            ui.label(format!(
                                "{} · Excluded alternative: {}",
                                rejected.dat_entry_name,
                                rejected.reasons.join("; ")
                            ));
                            widgets::technical_details(ui, "alternative-evidence", |ui| {
                                ui.label(format!("{:?}", rejected.evidence));
                            });
                        },
                    );
                }
                if remaining == 0 {
                    break;
                }
            }
        }
    });
}
