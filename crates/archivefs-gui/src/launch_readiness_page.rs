//! The Launch Readiness panel on the Selected page.
//!
//! Read-only rendering of an [`archivefs_core::launch::LaunchPlan`] the
//! caller has already built from existing, already-verified evidence via
//! [`archivefs_core::launch::canonical_identity_from_game_report`]/
//! [`archivefs_core::launch::launch_content_ref_from_archive_record`] and
//! [`archivefs_core::launch::build_launch_plan`]. This module never calls
//! any of those itself - see `main.rs`'s `MainView::Selected` branch for
//! where the plan is assembled and handed in as [`LaunchReadinessInput`].
//!
//! # What this module is not
//!
//! - There is no action enum and no button that launches, mounts, or
//!   changes anything - every value here is display-only. See
//!   `tests::renderer_exposes_no_launch_action_or_button` for the guarantee
//!   this holds mechanically.
//! - It never calls [`archivefs_core::launch::build_retroarch_command_plan`]
//!   or any ES-DE export/write function - it only reads the fields already
//!   present on a [`LaunchCandidate`]/[`LaunchPlan`] the caller built.
//! - It never resolves identity, mounts an archive, or guesses an inner
//!   archive member - see the module doc comment on
//!   [`archivefs_core::launch::evidence_bridge`] for where those honest
//!   fail-closed rules actually live; this module only renders whatever
//!   that bridge already decided.

use archivefs_core::launch::{
    CandidatePreference, FirmwareReadiness, LaunchBlocker, LaunchCandidate, LaunchPlan,
    LaunchReadiness, LaunchTarget, LaunchWarning,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// Everything [`show_launch_readiness_panel`] needs, gathered by the caller
/// from existing App/MainView state before this module ever runs. Every
/// non-[`Self::Plan`] variant is a prerequisite the caller checked *without*
/// calling the planner - see each variant's own doc comment for exactly
/// which check it stands for.
pub(crate) enum LaunchReadinessInput {
    /// `SelectedEvidenceState` is not `Ready` for the focused archive yet.
    /// The planner is never called in this state.
    EvidenceNotLoaded,
    /// `RetroArchProfilesState` is not `Ready` - RetroArch profiles/cores
    /// have never been scanned. The planner is never called in this state,
    /// and this panel never triggers a scan itself.
    RetroArchNotScanned,
    /// `CanonicalIdentityStatus::Unknown` - identity could not be resolved
    /// at all.
    IdentityUnknown,
    /// `CanonicalIdentityStatus::Conflicting` - identity evidence conflicts.
    IdentityConflicting,
    /// Identity was resolved and a real [`LaunchPlan`] was built.
    Plan(LaunchPlan),
}

pub(crate) fn show_launch_readiness_panel(ui: &mut egui::Ui, input: &LaunchReadinessInput) {
    widgets::section_header(
        ui,
        "Launch readiness",
        Some("Ways this game can be played. Planning only — nothing is launched."),
    );

    match input {
        LaunchReadinessInput::EvidenceNotLoaded => {
            widgets::card(ui, |ui| {
                ui.label("Load ROM Identity & Evidence first.");
            });
        }
        LaunchReadinessInput::RetroArchNotScanned => {
            widgets::card(ui, |ui| {
                ui.label("Scan RetroArch profiles to check installed cores.");
            });
        }
        LaunchReadinessInput::IdentityUnknown => {
            widgets::banner(
                ui,
                "Identity unresolved",
                "This game's identity could not be verified, so no launch options can be \
                 safely planned yet.",
                widgets::StatusTone::Pending,
            );
        }
        LaunchReadinessInput::IdentityConflicting => {
            widgets::banner(
                ui,
                "Identity conflicts",
                "Evidence for this game's identity conflicts and needs resolution before \
                 launch options can be planned.",
                widgets::StatusTone::Warning,
            );
        }
        LaunchReadinessInput::Plan(plan) => show_plan(ui, plan),
    }
}

fn show_plan(ui: &mut egui::Ui, plan: &LaunchPlan) {
    if plan.candidates.is_empty() {
        widgets::empty_state(
            ui,
            "No launch options found",
            "No installed RetroArch core is a candidate for this game's platform yet.",
            None,
        );
        return;
    }
    for candidate in &plan.candidates {
        ui.add_space(6.0);
        show_candidate(ui, candidate);
    }
}

fn readiness_label_and_tone(readiness: LaunchReadiness) -> (&'static str, widgets::StatusTone) {
    match readiness {
        LaunchReadiness::Ready => ("Ready", widgets::StatusTone::Success),
        LaunchReadiness::ReadyWithWarnings => ("Ready with warnings", widgets::StatusTone::Warning),
        LaunchReadiness::Blocked => ("Blocked", widgets::StatusTone::Blocked),
    }
}

fn preference_label(preference: CandidatePreference) -> &'static str {
    match preference {
        CandidatePreference::Remembered => "Remembered",
        CandidatePreference::SoleEligible => "Sole eligible",
        CandidatePreference::Undetermined => "Choice needed",
    }
}

fn firmware_label_and_tone(firmware: FirmwareReadiness) -> (&'static str, widgets::StatusTone) {
    match firmware {
        FirmwareReadiness::Verified => ("Verified", widgets::StatusTone::Success),
        FirmwareReadiness::PresentUnverified => {
            ("Present but unverified", widgets::StatusTone::Warning)
        }
        FirmwareReadiness::Missing => ("Missing", widgets::StatusTone::Blocked),
        FirmwareReadiness::Unknown => ("Unknown", widgets::StatusTone::Pending),
        FirmwareReadiness::NotRequired => ("Not required", widgets::StatusTone::Info),
    }
}

/// `(name, profile description)` for one candidate's target - never the
/// command that would launch it, only what it is.
fn target_labels(target: &LaunchTarget) -> (String, String) {
    match target {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } => (adapter_id.to_string(), format!("Profile {profile_id}")),
        LaunchTarget::RetroArchCore {
            profile, core_stem, ..
        } => (
            core_stem.clone(),
            format!(
                "RetroArch ({:?} / {:?})",
                profile.profile_kind, profile.scope
            ),
        ),
    }
}

fn show_candidate(ui: &mut egui::Ui, candidate: &LaunchCandidate) {
    widgets::card(ui, |ui| {
        let (name, profile) = target_labels(&candidate.target);
        let (readiness_label, readiness_tone) = readiness_label_and_tone(candidate.readiness);

        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(ui, readiness_label, readiness_tone);
            ui.label(egui::RichText::new(&name).strong().size(15.0));
        });
        ui.label(
            egui::RichText::new(&profile)
                .small()
                .color(theme::muted(ui)),
        );
        ui.label(
            egui::RichText::new(format!(
                "Preference: {}",
                preference_label(candidate.preference)
            ))
            .small()
            .color(theme::muted(ui)),
        );

        let (firmware_label, firmware_tone) = firmware_label_and_tone(candidate.firmware);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Firmware/BIOS:").small());
            widgets::status_badge(ui, firmware_label, firmware_tone);
        });

        for blocker in &candidate.blockers {
            show_blocker(ui, blocker);
        }
        for warning in &candidate.warnings {
            show_warning(ui, warning);
        }

        widgets::technical_details(ui, (&name, &profile), |ui| {
            detail_label(
                ui,
                "Content resolved",
                &candidate.content.has_runnable_path().to_string(),
            );
            detail_label(
                ui,
                "Requires mount",
                &candidate.content.requires_mount.to_string(),
            );
            detail_label(ui, "Content provenance", &candidate.content.provenance);
        });
    });
}

fn show_blocker(ui: &mut egui::Ui, blocker: &LaunchBlocker) {
    ui.horizontal_wrapped(|ui| {
        widgets::status_badge(ui, "Blocked", widgets::StatusTone::Blocked);
        ui.label(egui::RichText::new(&blocker.detail).small());
    });
}

fn show_warning(ui: &mut egui::Ui, warning: &LaunchWarning) {
    ui.horizontal_wrapped(|ui| {
        widgets::status_badge(ui, "Warning", widgets::StatusTone::Warning);
        ui.label(egui::RichText::new(&warning.detail).small());
    });
}

fn detail_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_sized(
            [140.0, 0.0],
            egui::Label::new(egui::RichText::new(label).strong()),
        );
        ui.add(egui::Label::new(value).wrap());
    });
}

#[cfg(test)]
mod tests;
