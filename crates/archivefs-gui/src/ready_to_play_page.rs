//! Read-only presentation of the core Ready-to-Play projection.
//!
//! This module owns no discovery or launch policy.  The caller supplies the
//! result projected from evidence that has already been gathered elsewhere.

use archivefs_core::diagnostics::DoctorSeverity;
use archivefs_core::ready_to_play::{
    Fixability, ReadinessReason, ReadinessReasonFamily, ReadyToPlayResult, ReadyToPlayState,
};
use eframe::egui;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ReadyToPlayFilter {
    #[default]
    All,
    Ready,
    ReadyWithWarnings,
    NeedsAttention,
    Blocked,
    Unsupported,
    Unknown,
}

#[derive(Default)]
pub(crate) struct ReadyToPlayPageState {
    pub(crate) filter: ReadyToPlayFilter,
    results: Vec<ReadyToPlayResult>,
}

impl ReadyToPlayPageState {
    pub(crate) fn set_results(&mut self, results: Vec<ReadyToPlayResult>) {
        self.results = results;
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("Ready-to-Play");
        ui.label("Read-only readiness summary from evidence already gathered by EmuWiz.");
        ui.label(
            "Changing this view does not rescan files, change emulator settings, or launch a game.",
        );
        ui.add_space(8.0);

        ui.horizontal_wrapped(|ui| {
            for (filter, label) in [
                (ReadyToPlayFilter::All, "All"),
                (ReadyToPlayFilter::Ready, "Ready"),
                (ReadyToPlayFilter::ReadyWithWarnings, "Ready with warnings"),
                (ReadyToPlayFilter::NeedsAttention, "Needs attention"),
                (ReadyToPlayFilter::Blocked, "Blocked"),
                (ReadyToPlayFilter::Unsupported, "Unsupported"),
                (ReadyToPlayFilter::Unknown, "Unknown"),
            ] {
                if ui.selectable_label(self.filter == filter, label).clicked() {
                    self.filter = filter;
                }
            }
        });
        ui.add_space(8.0);

        if self.results.is_empty() {
            ui.label("No readiness projection is available yet.");
            ui.label(
                "Not enough evidence has been gathered yet; this is not a missing or broken game.",
            );
            return;
        }

        show_summary(ui, &self.results);
        if !self
            .results
            .iter()
            .any(|result| filter_matches(self.filter, result.state))
        {
            ui.label("No items match this filter.");
            return;
        }
        for result in self
            .results
            .iter()
            .filter(|result| filter_matches(self.filter, result.state))
        {
            show_result(ui, result);
        }
    }
}

fn show_summary(ui: &mut egui::Ui, results: &[ReadyToPlayResult]) {
    let count = |state| {
        results
            .iter()
            .filter(|result| result.state == state)
            .count()
    };
    ui.label(format!(
        "{} items: {} ready, {} with warnings, {} need attention, {} blocked, {} unsupported, {} unknown.",
        results.len(),
        count(ReadyToPlayState::Ready),
        count(ReadyToPlayState::ReadyWithWarnings),
        count(ReadyToPlayState::NeedsAttention),
        count(ReadyToPlayState::Blocked),
        count(ReadyToPlayState::Unsupported),
        count(ReadyToPlayState::Unknown),
    ));
    ui.add_space(6.0);
}

fn show_result(ui: &mut egui::Ui, result: &ReadyToPlayResult) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.strong(&result.item_identity);
            ui.separator();
            ui.label(state_label(result.state));
        });
        if let Some(platform) = &result.platform_id {
            ui.label(format!("Platform: {platform}"));
        }
        if result.reasons.is_empty() {
            ui.label("No non-blocking conditions were reported.");
        } else {
            ui.separator();
            ui.strong("Readiness reasons");
            for reason in &result.reasons {
                show_reason(ui, reason);
            }
        }
    });
}

fn show_reason(ui: &mut egui::Ui, reason: &ReadinessReason) {
    ui.collapsing(
        format!("{} — {}", family_label(reason.family), reason.summary),
        |ui| {
            ui.label(format!("Severity: {}", reason.severity.label()));
            ui.label(format!(
                "Fixability: {}",
                fixability_label(reason.fixability)
            ));
            ui.label(format!("Source: {}", reason.provenance));
            ui.label(&reason.technical_detail);
            ui.collapsing("Technical details", |ui| {
                ui.label(format!("Reason family: {:?}", reason.family));
                ui.label(format!("Fixability: {:?}", reason.fixability));
                if let Some(blocker) = reason.original_blocker {
                    ui.label(format!("Original evidence kind: {blocker:?}"));
                }
            });
        },
    );
}

pub(crate) fn state_label(state: ReadyToPlayState) -> &'static str {
    match state {
        ReadyToPlayState::Ready => "Ready",
        ReadyToPlayState::ReadyWithWarnings => "Ready with warnings",
        ReadyToPlayState::NeedsAttention => "Needs attention",
        ReadyToPlayState::Blocked => "Blocked",
        ReadyToPlayState::Unsupported => "Unsupported",
        ReadyToPlayState::Unknown => "Unknown — evidence not gathered",
    }
}

pub(crate) fn family_label(family: ReadinessReasonFamily) -> &'static str {
    match family {
        ReadinessReasonFamily::Identity => "Identity",
        ReadinessReasonFamily::Content => "Content",
        ReadinessReasonFamily::MediaTopology => "Media",
        ReadinessReasonFamily::Firmware => "Firmware / BIOS",
        ReadinessReasonFamily::Emulator => "Emulator",
        ReadinessReasonFamily::Configuration => "Configuration",
        ReadinessReasonFamily::Dependency => "Dependencies",
        ReadinessReasonFamily::Arcade => "Arcade",
        ReadinessReasonFamily::DatCompatibility => "DAT compatibility",
        ReadinessReasonFamily::ModOrPatch => "Mods / patches",
        ReadinessReasonFamily::Controller => "Controller",
        ReadinessReasonFamily::LaunchPlan => "Launch plan",
        ReadinessReasonFamily::Unsupported => "Unsupported",
        ReadinessReasonFamily::UnknownEvidence => "Evidence not gathered",
    }
}

fn fixability_label(fixability: Fixability) -> &'static str {
    match fixability {
        Fixability::InformationOnly => "Information only",
        Fixability::UserCanFix => "You can fix this",
        Fixability::EmuwizCanGuide => "EmuWiz can guide you",
        Fixability::EmuwizCanRepairSafely => "EmuWiz can repair this safely",
        Fixability::Unsupported => "Unsupported",
    }
}

pub(crate) fn filter_matches(filter: ReadyToPlayFilter, state: ReadyToPlayState) -> bool {
    match filter {
        ReadyToPlayFilter::All => true,
        ReadyToPlayFilter::Ready => state == ReadyToPlayState::Ready,
        ReadyToPlayFilter::ReadyWithWarnings => state == ReadyToPlayState::ReadyWithWarnings,
        ReadyToPlayFilter::NeedsAttention => state == ReadyToPlayState::NeedsAttention,
        ReadyToPlayFilter::Blocked => state == ReadyToPlayState::Blocked,
        ReadyToPlayFilter::Unsupported => state == ReadyToPlayState::Unsupported,
        ReadyToPlayFilter::Unknown => state == ReadyToPlayState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_plain_language_label() {
        assert_eq!(state_label(ReadyToPlayState::Ready), "Ready");
        assert_eq!(
            state_label(ReadyToPlayState::ReadyWithWarnings),
            "Ready with warnings"
        );
        assert_eq!(
            state_label(ReadyToPlayState::NeedsAttention),
            "Needs attention"
        );
        assert_eq!(state_label(ReadyToPlayState::Blocked), "Blocked");
        assert_eq!(state_label(ReadyToPlayState::Unsupported), "Unsupported");
        assert!(state_label(ReadyToPlayState::Unknown).contains("evidence not gathered"));
        assert!(!state_label(ReadyToPlayState::Unknown).contains("Missing"));
    }

    #[test]
    fn filters_are_independent() {
        assert!(filter_matches(
            ReadyToPlayFilter::Ready,
            ReadyToPlayState::Ready
        ));
        assert!(!filter_matches(
            ReadyToPlayFilter::Ready,
            ReadyToPlayState::ReadyWithWarnings
        ));
        assert!(filter_matches(
            ReadyToPlayFilter::ReadyWithWarnings,
            ReadyToPlayState::ReadyWithWarnings
        ));
        assert!(!filter_matches(
            ReadyToPlayFilter::Blocked,
            ReadyToPlayState::Unknown
        ));
        assert!(filter_matches(
            ReadyToPlayFilter::Unknown,
            ReadyToPlayState::Unknown
        ));
    }

    #[test]
    fn family_and_fixability_labels_are_user_facing() {
        assert_eq!(
            family_label(ReadinessReasonFamily::UnknownEvidence),
            "Evidence not gathered"
        );
        assert_eq!(
            fixability_label(Fixability::EmuwizCanGuide),
            "EmuWiz can guide you"
        );
        assert_eq!(DoctorSeverity::Warning.label(), "Warning");
    }
}
