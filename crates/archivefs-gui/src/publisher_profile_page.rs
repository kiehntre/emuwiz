//! Publisher / Frontend Library planning view - task section 19.
//!
//! A thin, read-only GUI over `archivefs_core::publisher_profile`. This
//! page never scans, elects, or mutates anything: it takes an
//! already-built `PlayingLibraryPlan` (produced by the existing "Build
//! Playing Library" flow), a chosen target profile, and a destination
//! root, and shows what `build_publisher_plan` reports. **There is no
//! Apply/Publish button anywhere in this file** - Phase 1 is preview only.
//!
//! # Wiring status (honest limitation)
//!
//! This module is complete and unit-tested on its own, but is not yet
//! wired into `MainView`/the app sidebar - see
//! `docs/research/PUBLISHER_PROFILES_PHASE1.md`'s GUI section for why that
//! last step was left as a small, mechanical follow-up rather than risking
//! an unreviewed change to the main navigation file in this pass.

// Not yet reachable from `MainView`/the sidebar (see this module's own doc
// comment) - allowed here rather than papered over with an unused `_`
// prefix on every public item, since this module's own tests already
// exercise the real code paths below.
#![allow(dead_code)]

use archivefs_core::platform_evidence_fusion::romm_platform_mapping::FrontendPlatformMapping;
use archivefs_core::playing_library::PlayingLibraryPlan;
use archivefs_core::publisher_profile::es_de::{es_de_profile, resolve_es_de_platform_mapping};
use archivefs_core::publisher_profile::romm::{resolve_romm_platform_mapping, romm_profile};
use archivefs_core::publisher_profile::{
    PublisherActionSafety, PublisherFrontend, PublisherPlan, PublisherPlanItem,
    PublisherPlanRequest, build_publisher_plan,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// Which result bucket the user is currently filtering to - task section
/// 19's exact filter list, plus "All".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PublisherResultFilter {
    #[default]
    All,
    Ready,
    AlreadyPresent,
    ReviewRequired,
    Blocked,
    Unsupported,
}

impl PublisherResultFilter {
    const ALL: [Self; 6] = [
        Self::All,
        Self::Ready,
        Self::AlreadyPresent,
        Self::ReviewRequired,
        Self::Blocked,
        Self::Unsupported,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Ready => "Ready",
            Self::AlreadyPresent => "Already present",
            Self::ReviewRequired => "Review required",
            Self::Blocked => "Blocked",
            Self::Unsupported => "Unsupported",
        }
    }

    fn matches(self, item: &PublisherPlanItem) -> bool {
        match self {
            Self::All => true,
            Self::Ready => {
                item.safety == PublisherActionSafety::SafeToAct
                    && item.destination_state
                        != archivefs_core::publisher_profile::DestinationState::AlreadyCorrect
            }
            Self::AlreadyPresent => {
                item.destination_state
                    == archivefs_core::publisher_profile::DestinationState::AlreadyCorrect
            }
            Self::ReviewRequired => item.safety == PublisherActionSafety::ReviewRequired,
            Self::Blocked => item.safety == PublisherActionSafety::Blocked,
            Self::Unsupported => item.safety == PublisherActionSafety::Unsupported,
        }
    }
}

#[derive(Default)]
pub(crate) struct PublisherProfilePageState {
    pub(crate) target: Option<PublisherFrontend>,
    pub(crate) destination_root: String,
    /// Supplied by the existing Playing Library flow - this page never
    /// builds one itself.
    pub(crate) source_plan: Option<PlayingLibraryPlan>,
    pub(crate) canonical_platform_id: String,
    pub(crate) result: Option<PublisherPlan>,
    pub(crate) error: Option<String>,
    pub(crate) filter: PublisherResultFilter,
    pub(crate) advanced: bool,
    pub(crate) selected_item: Option<usize>,
}

impl PublisherProfilePageState {
    /// Builds the read-only preview. Never called automatically - only
    /// from an explicit "Preview plan" button.
    pub(crate) fn preview(&mut self) {
        self.error = None;
        self.result = None;
        let Some(target) = self.target else {
            self.error = Some("Choose a target profile first.".to_string());
            return;
        };
        let Some(plan) = &self.source_plan else {
            self.error =
                Some("Choose a source (a Playing Library plan) to preview first.".to_string());
            return;
        };
        let destination_root = std::path::PathBuf::from(self.destination_root.trim());
        if !destination_root.is_absolute() {
            self.error = Some("The destination root must be an absolute path.".to_string());
            return;
        }
        let (profile, mapping) = match target {
            PublisherFrontend::RomM => (
                romm_profile(),
                resolve_romm_platform_mapping(
                    &self.canonical_platform_id,
                    &FrontendPlatformMapping::default(),
                    None,
                ),
            ),
            PublisherFrontend::EsDe => (
                es_de_profile(),
                resolve_es_de_platform_mapping(&self.canonical_platform_id),
            ),
        };
        let request = PublisherPlanRequest {
            profile: &profile,
            playing_library_plan: plan,
            platform_mapping: mapping,
            destination_root,
            existing_destination_root: None,
        };
        match build_publisher_plan(&request) {
            Ok(result) => self.result = Some(result),
            Err(message) => self.error = Some(message),
        }
    }
}

/// Renders the Publisher / Frontend Library planning view. Returns
/// nothing actionable - there is deliberately no "apply" outcome type,
/// unlike every other page in this crate that wraps a real mutation.
pub(crate) fn show_publisher_profile_page(
    ui: &mut egui::Ui,
    state: &mut PublisherProfilePageState,
) {
    widgets::section_header(
        ui,
        "Publisher / Frontend Library",
        Some("Preview how your library would look for another app. Nothing is changed here."),
    );
    ui.label(
        egui::RichText::new("PREVIEW ONLY — nothing will be changed.")
            .strong()
            .color(theme::muted(ui)),
    );
    ui.add_space(8.0);

    widgets::card(ui, |ui| {
        ui.label("Choose what you want to create:");
        ui.horizontal(|ui| {
            if ui
                .selectable_label(
                    state.target == Some(PublisherFrontend::RomM),
                    PublisherFrontend::RomM.plain_language_goal(),
                )
                .clicked()
            {
                state.target = Some(PublisherFrontend::RomM);
            }
            if ui
                .selectable_label(
                    state.target == Some(PublisherFrontend::EsDe),
                    PublisherFrontend::EsDe.plain_language_goal(),
                )
                .clicked()
            {
                state.target = Some(PublisherFrontend::EsDe);
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Destination folder:");
            ui.text_edit_singleline(&mut state.destination_root);
        });
        ui.horizontal(|ui| {
            ui.label("Platform (canonical id):");
            ui.text_edit_singleline(&mut state.canonical_platform_id);
        });
        if state.source_plan.is_none() {
            ui.label(
                egui::RichText::new(
                    "No Playing Library plan selected yet - build one first on the Playing \
                     Library page.",
                )
                .color(theme::muted(ui)),
            );
        }
        if ui.button("Preview plan").clicked() {
            state.preview();
        }
        if let Some(error) = &state.error {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
        }
    });

    let Some(result) = &state.result else {
        return;
    };

    ui.add_space(10.0);
    let summary = result.summary();
    widgets::card(ui, |ui| {
        ui.label(format!("Target: {}", result.frontend.label()));
        ui.label(format!("Source items: {}", summary.source_items));
        ui.label(format!("Will publish: {}", summary.will_publish));
        ui.label(format!("Already present: {}", summary.already_present));
        ui.label(format!("Review required: {}", summary.review_required));
        ui.label(format!("Blocked: {}", summary.blocked));
        ui.label(format!("Unsupported: {}", summary.unsupported));
        ui.label(format!(
            "Planned future actions: {} symlinks, {} hardlinks, {} copies",
            summary.planned_symlinks, summary.planned_hardlinks, summary.planned_copies
        ));
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for candidate in PublisherResultFilter::ALL {
            if ui
                .selectable_label(state.filter == candidate, candidate.label())
                .clicked()
            {
                state.filter = candidate;
            }
        }
        ui.checkbox(&mut state.advanced, "Advanced");
    });

    ui.add_space(6.0);
    let filtered: Vec<(usize, &PublisherPlanItem)> = result
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| state.filter.matches(item))
        .collect();
    for (index, item) in &filtered {
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new(&item.dat_entry_name).strong());
            ui.label(&item.reason);
            if state.advanced {
                ui.label(format!(
                    "canonical platform: {}",
                    item.platform_mapping.canonical_platform_id()
                ));
                if let Some(folder) = item.platform_mapping.folder() {
                    ui.label(format!("target folder: {folder}"));
                }
                ui.label(format!("action: {:?}", item.planned_action.kind));
                ui.label(format!("safety: {:?}", item.safety));
                if let Some(destination) = &item.planned_destination {
                    ui.label(format!("destination path: {}", destination.display()));
                }
            }
            if ui
                .selectable_label(state.selected_item == Some(*index), "Details")
                .clicked()
            {
                state.selected_item = Some(*index);
            }
        });
    }
    if filtered.is_empty() {
        ui.label(egui::RichText::new("No items match this filter.").color(theme::muted(ui)));
    }
}

#[cfg(test)]
mod tests;
