//! Read-only presentation of the core Ready-to-Play projection.
//!
//! This module owns no discovery or launch policy.  The caller supplies the
//! result projected from evidence that has already been gathered elsewhere.

use archivefs_core::diagnostics::DoctorSeverity;
use archivefs_core::mame_input_requirements::{
    ArcadeInputRequirement, ArcadeInputRequirementFamily, ArcadeInputRequirements,
};
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
    original_controls: Vec<OriginalControlsView>,
}

struct OriginalControlsView {
    item_identity: String,
    requirements: ArcadeInputRequirements,
}

impl ReadyToPlayPageState {
    pub(crate) fn set_results(&mut self, results: Vec<ReadyToPlayResult>) {
        self.results = results;
    }

    /// Supplies already-normalized static MAME metadata for display. This is
    /// intentionally separate from readiness results and performs no probing.
    pub(crate) fn set_original_controls(
        &mut self,
        controls: Vec<(String, ArcadeInputRequirements)>,
    ) {
        self.original_controls = controls
            .into_iter()
            .map(|(item_identity, requirements)| OriginalControlsView {
                item_identity,
                requirements,
            })
            .collect();
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
            let controls = self
                .original_controls
                .iter()
                .find(|controls| controls.item_identity == result.item_identity)
                .map(|controls| &controls.requirements);
            show_result(ui, result, controls);
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

fn show_result(
    ui: &mut egui::Ui,
    result: &ReadyToPlayResult,
    controls: Option<&ArcadeInputRequirements>,
) {
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
        if let Some(controls) = controls {
            show_original_controls(ui, controls);
        }
    });
}

fn show_original_controls(ui: &mut egui::Ui, controls: &ArcadeInputRequirements) {
    ui.separator();
    ui.collapsing("Original Controls", |ui| {
        ui.label(
            "Static metadata describing the original machine; not a hardware compatibility check.",
        );
        if let Some(players) = controls.supported_players.as_deref() {
            ui.label(supported_players_label(players));
        }
        if controls.requirements.is_empty() {
            ui.label("Input metadata not available.");
            return;
        }
        for requirement in &controls.requirements {
            ui.collapsing(control_summary(requirement), |ui| {
                ui.label(format!(
                    "Original machine uses {}.",
                    family_label_input(requirement.family)
                ));
                show_input_technical_details(ui, requirement);
            });
        }
        ui.label(format!("Source: {}", controls.provenance));
    });
}

fn supported_players_label(players: &str) -> String {
    format!("Supports up to {players} players")
}

fn control_summary(requirement: &ArcadeInputRequirement) -> String {
    match requirement.family {
        ArcadeInputRequirementFamily::Buttons => requirement
            .buttons
            .as_deref()
            .map(|buttons| format!("{buttons} buttons"))
            .unwrap_or_else(|| "Buttons".into()),
        ArcadeInputRequirementFamily::Digital2Way => "2-way directional controls".into(),
        ArcadeInputRequirementFamily::Digital4Way => "4-way joystick / directional controls".into(),
        ArcadeInputRequirementFamily::Digital8Way => "8-way joystick / directional controls".into(),
        family => family_label_input(family).into(),
    }
}

fn family_label_input(family: ArcadeInputRequirementFamily) -> &'static str {
    match family {
        ArcadeInputRequirementFamily::DigitalDirections => "directional controls",
        ArcadeInputRequirementFamily::Digital2Way => "2-way directional controls",
        ArcadeInputRequirementFamily::Digital4Way => "4-way joystick / directional controls",
        ArcadeInputRequirementFamily::Digital8Way => "8-way joystick / directional controls",
        ArcadeInputRequirementFamily::DirectionalOther => "other directional controls",
        ArcadeInputRequirementFamily::Buttons => "buttons",
        ArcadeInputRequirementFamily::DualStick => "dual-stick controls",
        ArcadeInputRequirementFamily::AnalogAxis => "analog control",
        ArcadeInputRequirementFamily::Pedal => "pedal",
        ArcadeInputRequirementFamily::RelativePointer => "relative pointer control",
        ArcadeInputRequirementFamily::AbsolutePointer => "absolute pointer control",
        ArcadeInputRequirementFamily::LightGun => "light gun",
        ArcadeInputRequirementFamily::Keyboard => "keyboard",
        ArcadeInputRequirementFamily::Mouse => "mouse",
        ArcadeInputRequirementFamily::Trackball => "trackball",
        ArcadeInputRequirementFamily::DialSpinner => "dial / spinner",
        ArcadeInputRequirementFamily::PositionalControl => "positional control",
        ArcadeInputRequirementFamily::SpecialPanel => "special control panel",
        ArcadeInputRequirementFamily::Unknown => "unknown control type",
    }
}

fn show_input_technical_details(ui: &mut egui::Ui, requirement: &ArcadeInputRequirement) {
    ui.collapsing("Technical details", |ui| {
        if let Some(value) = requirement.raw_control_type.as_deref() {
            ui.label(format!("Original control type: {value}"));
        }
        if let Some(value) = requirement.player.as_deref() {
            ui.label(format!("Player: {value}"));
        }
        for (label, value) in [
            ("Buttons", requirement.buttons.as_deref()),
            ("Required buttons", requirement.reqbuttons.as_deref()),
            ("Ways", requirement.ways.as_deref()),
            ("Ways 2", requirement.ways2.as_deref()),
            ("Ways 3", requirement.ways3.as_deref()),
            ("Minimum", requirement.minimum.as_deref()),
            ("Maximum", requirement.maximum.as_deref()),
            ("Sensitivity", requirement.sensitivity.as_deref()),
            ("Key delta", requirement.keydelta.as_deref()),
            ("Reverse", requirement.reverse.as_deref()),
        ] {
            if let Some(value) = value {
                ui.label(format!("{label}: {value}"));
            }
        }
        ui.label(format!("Evidence: {:?}", requirement.evidence_strength));
        ui.label(format!("Source: {}", requirement.provenance));
        for (key, value) in &requirement.raw_attributes {
            ui.label(format!("Raw {key}: {value}"));
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

    #[test]
    fn static_input_labels_preserve_original_machine_context() {
        assert_eq!(supported_players_label("4"), "Supports up to 4 players");
        assert_eq!(
            family_label_input(ArcadeInputRequirementFamily::Trackball),
            "trackball"
        );
        assert_eq!(
            control_summary(&ArcadeInputRequirement {
                family: ArcadeInputRequirementFamily::Buttons,
                buttons: Some("3".into()),
                ..Default::default()
            }),
            "3 buttons"
        );
        assert!(!family_label_input(ArcadeInputRequirementFamily::LightGun).contains("need"));
        assert!(!family_label_input(ArcadeInputRequirementFamily::LightGun).contains("require"));
    }

    #[test]
    fn unknown_controls_are_shown_as_unknown_not_missing() {
        assert_eq!(
            family_label_input(ArcadeInputRequirementFamily::Unknown),
            "unknown control type"
        );
        assert!(!family_label_input(ArcadeInputRequirementFamily::Unknown).contains("missing"));
    }
}
