//! Candidate-first Emulator Setup presentation.
//!
//! The candidate list is deliberately projected from core's reviewed launch
//! compatibility table. This keeps setup honest: a GUI card is not created
//! merely because an executable name sounds plausible, and the final adapter
//! preflight remains the authority for launching a particular game.

use archivefs_core::diagnostics::{DoctorCategory, DoctorSeverity, Finding};
use archivefs_core::launch::{
    DOSBOX_SUPPORTED_PLATFORM_ID, LAUNCH_COMPATIBILITY, LaunchCompatibility,
    SAMEBOY_SUPPORTED_PLATFORM_IDS,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateState {
    Ready,
    Warnings,
    NeedsSetup,
    Blocked,
    NotChecked,
}

impl CandidateState {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Warnings => "Warnings",
            Self::NeedsSetup => "Needs setup",
            Self::Blocked => "Blocked",
            Self::NotChecked => "Not checked",
        }
    }

    fn tone(self) -> widgets::StatusTone {
        match self {
            Self::Ready => widgets::StatusTone::Success,
            Self::Warnings => widgets::StatusTone::Warning,
            Self::NeedsSetup | Self::NotChecked => widgets::StatusTone::Pending,
            Self::Blocked => widgets::StatusTone::Blocked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmulatorSetupCandidate {
    pub(crate) platform_id: &'static str,
    pub(crate) adapter_id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) state: CandidateState,
    pub(crate) reason: String,
    pub(crate) evidence: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct EmulatorSetupPageState {
    pub(crate) platform_filter: String,
    pub(crate) search: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetroArchSetupStatus {
    NotChecked,
    Checking,
    Ready,
    NeedsSetup,
    Blocked,
}

impl RetroArchSetupStatus {
    fn candidate_state(self) -> CandidateState {
        match self {
            Self::NotChecked | Self::Checking => CandidateState::NotChecked,
            Self::Ready => CandidateState::Ready,
            Self::NeedsSetup => CandidateState::NeedsSetup,
            Self::Blocked => CandidateState::Blocked,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::NotChecked => "RetroArch profiles have not been checked yet.",
            Self::Checking => "RetroArch profile discovery is in progress.",
            Self::Ready => "An eligible RetroArch profile was discovered.",
            Self::NeedsSetup => "RetroArch needs a usable profile or core folder.",
            Self::Blocked => "RetroArch profile discovery needs attention.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmulatorSetupAction {
    CheckEmulators,
}

fn adapter_name(adapter_id: &str) -> &'static str {
    match adapter_id {
        "amiga_whdload" => "Amiga WHDLoad",
        "amiberry" => "Amiberry",
        "fsuae" => "FS-UAE",
        "duckstation" => "DuckStation",
        "pcsx2" => "PCSX2",
        "ppsspp" => "PPSSPP",
        "rpcs3" => "RPCS3",
        "vita3k" => "Vita3K",
        "azahar" => "Azahar",
        "xemu" => "xemu",
        "xenia" => "Xenia",
        "dolphin" => "Dolphin",
        "flycast" => "Flycast",
        "melonds" => "melonDS",
        "hatari" => "Hatari",
        "mgba" => "mGBA",
        "mame" => "MAME",
        "fbneo" => "FBNeo",
        "cemu" => "Cemu",
        "scummvm" => "ScummVM",
        "sameboy" => "SameBoy",
        "dosbox" => "DOSBox",
        _ => "Supported emulator",
    }
}

fn finding_for<'a>(findings: &'a [Finding], adapter_id: &str, name: &str) -> Option<&'a Finding> {
    findings.iter().find(|finding| {
        matches!(
            finding.category,
            DoctorCategory::Emulators | DoctorCategory::EmulatorProfiles
        ) && [name, adapter_id].iter().any(|needle| {
            finding.title.contains(needle)
                || finding.explanation.contains(needle)
                || finding.evidence.iter().any(|line| line.contains(needle))
        })
    })
}

fn candidate_from_mapping(
    mapping: &LaunchCompatibility,
    adapter_id: &'static str,
    findings: Option<&[Finding]>,
) -> EmulatorSetupCandidate {
    let name = adapter_name(adapter_id);
    let Some(findings) = findings else {
        return EmulatorSetupCandidate {
            platform_id: mapping.platform_id,
            adapter_id,
            name,
            state: CandidateState::NotChecked,
            reason: "Run an emulator check to inspect this candidate.".to_string(),
            evidence: Vec::new(),
        };
    };
    let Some(finding) = finding_for(findings, adapter_id, name) else {
        return EmulatorSetupCandidate {
            platform_id: mapping.platform_id,
            adapter_id,
            name,
            state: CandidateState::NeedsSetup,
            reason: "No installation or readiness evidence was found.".to_string(),
            evidence: Vec::new(),
        };
    };
    let state = if finding.severity.is_blocking() {
        CandidateState::Blocked
    } else if finding.severity == DoctorSeverity::Warning {
        CandidateState::Warnings
    } else if finding.severity == DoctorSeverity::Info
        && finding
            .title
            .to_ascii_lowercase()
            .contains("ready to launch")
    {
        CandidateState::Ready
    } else {
        CandidateState::NeedsSetup
    };
    EmulatorSetupCandidate {
        platform_id: mapping.platform_id,
        adapter_id,
        name,
        state,
        reason: finding.explanation.clone(),
        evidence: finding.evidence.clone(),
    }
}

fn unintegrated_candidate(
    platform_id: &'static str,
    adapter_id: &'static str,
    name: &'static str,
    findings: Option<&[Finding]>,
) -> EmulatorSetupCandidate {
    let Some(findings) = findings else {
        return EmulatorSetupCandidate {
            platform_id,
            adapter_id,
            name,
            state: CandidateState::NotChecked,
            reason: "Run an emulator check to inspect this candidate.".to_string(),
            evidence: Vec::new(),
        };
    };
    let Some(finding) = finding_for(findings, adapter_id, name) else {
        return EmulatorSetupCandidate {
            platform_id,
            adapter_id,
            name,
            state: CandidateState::NeedsSetup,
            reason: "The adapter is supported, but no setup evidence was found yet.".to_string(),
            evidence: vec![
                "This adapter is not yet part of the shared selected-game candidate plan."
                    .to_string(),
            ],
        };
    };
    let state = if finding.severity.is_blocking() {
        CandidateState::Blocked
    } else if finding.severity == DoctorSeverity::Warning {
        CandidateState::Warnings
    } else if finding.severity == DoctorSeverity::Info
        && finding
            .title
            .to_ascii_lowercase()
            .contains("ready to launch")
    {
        CandidateState::Ready
    } else {
        CandidateState::NeedsSetup
    };
    EmulatorSetupCandidate {
        platform_id,
        adapter_id,
        name,
        state,
        reason: finding.explanation.clone(),
        evidence: finding.evidence.clone(),
    }
}

pub(crate) fn build_candidates(
    findings: Option<&[Finding]>,
    retroarch: RetroArchSetupStatus,
    platform_filter: Option<&str>,
    search: &str,
) -> Vec<EmulatorSetupCandidate> {
    let search = search.trim().to_ascii_lowercase();
    let matches_filter = |platform_id: &str| {
        platform_filter.is_none_or(|filter| filter.is_empty() || filter == platform_id)
    };
    let matches_search = |candidate: &EmulatorSetupCandidate| {
        search.is_empty()
            || candidate.name.to_ascii_lowercase().contains(&search)
            || candidate.platform_id.to_ascii_lowercase().contains(&search)
            || candidate.adapter_id.to_ascii_lowercase().contains(&search)
    };
    let mut candidates = Vec::new();
    for mapping in LAUNCH_COMPATIBILITY {
        if !matches_filter(mapping.platform_id) {
            continue;
        }
        for &adapter_id in mapping.standalone_adapters {
            let candidate = candidate_from_mapping(mapping, adapter_id, findings);
            if matches_search(&candidate) {
                candidates.push(candidate);
            }
        }
        if !mapping.retroarch_core_hints.is_empty() {
            let candidate = EmulatorSetupCandidate {
                platform_id: mapping.platform_id,
                adapter_id: "retroarch",
                name: "RetroArch",
                state: retroarch.candidate_state(),
                reason: retroarch.reason().to_string(),
                evidence: mapping
                    .retroarch_core_hints
                    .iter()
                    .map(|hint| format!("Reviewed core candidate: {hint}"))
                    .collect(),
            };
            if matches_search(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    // These adapters have complete native command/readiness modules but are
    // intentionally not yet wired into core's shared selected-game matrix.
    // Showing them as explicit setup candidates is useful and honest: they
    // remain Needs setup until that shared authorization seam exists.
    for &platform_id in SAMEBOY_SUPPORTED_PLATFORM_IDS {
        if matches_filter(platform_id) {
            let candidate = unintegrated_candidate(platform_id, "sameboy", "SameBoy", findings);
            if matches_search(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    if matches_filter(DOSBOX_SUPPORTED_PLATFORM_ID) {
        let candidate =
            unintegrated_candidate(DOSBOX_SUPPORTED_PLATFORM_ID, "dosbox", "DOSBox", findings);
        if matches_search(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

pub(crate) fn candidate_columns(available_width: f32) -> usize {
    if available_width >= 1_100.0 {
        3
    } else if available_width >= 680.0 {
        2
    } else {
        1
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut EmulatorSetupPageState,
    findings: Option<&[Finding]>,
    checking: bool,
    retroarch: RetroArchSetupStatus,
    focused_emulator: Option<&str>,
) -> Option<EmulatorSetupAction> {
    widgets::section_header(
        ui,
        "Emulator candidates",
        Some(
            "Choose a platform to see every reviewed emulator candidate and why it is ready or needs setup.",
        ),
    );
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.label("Platform");
        egui::ComboBox::from_id_salt("emulator-setup-platform")
            .selected_text(if state.platform_filter.is_empty() {
                "All platforms"
            } else {
                &state.platform_filter
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.platform_filter, String::new(), "All platforms");
                for mapping in LAUNCH_COMPATIBILITY {
                    ui.selectable_value(
                        &mut state.platform_filter,
                        mapping.platform_id.to_string(),
                        mapping.platform_id,
                    );
                }
            });
        ui.label("Search");
        ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .hint_text("Search emulators…")
                .desired_width((ui.available_width() - 60.0).clamp(180.0, 360.0)),
        );
        if widgets::action_button(
            ui,
            if checking {
                "Checking…"
            } else {
                "Check emulators"
            },
            widgets::ActionStyle::Secondary,
            !checking,
        )
        .clicked()
        {
            action = Some(EmulatorSetupAction::CheckEmulators);
        }
    });
    ui.add_space(theme::SPACE_MD);

    let candidates = build_candidates(
        findings,
        retroarch,
        (!state.platform_filter.is_empty()).then_some(state.platform_filter.as_str()),
        &state.search,
    );
    if candidates.is_empty() {
        widgets::empty_state(
            ui,
            "No emulator candidates match",
            "Try another platform or search, or run the emulator check.",
            None,
        );
        return action;
    }
    let columns = candidate_columns(ui.available_width());
    let width = ((ui.available_width() - (columns.saturating_sub(1) as f32 * 12.0))
        / columns as f32)
        .max(180.0);
    ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
    ui.horizontal_wrapped(|ui| {
        for candidate in &candidates {
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let focused = focused_emulator.is_some_and(|focus| {
                        focus.eq_ignore_ascii_case(candidate.name)
                            || focus.eq_ignore_ascii_case(candidate.adapter_id)
                    });
                    if focused {
                        ui.scroll_to_cursor(Some(egui::Align::Center));
                    }
                    widgets::card(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(candidate.name).strong());
                            widgets::status_badge(ui, candidate.state.label(), candidate.state.tone());
                        });
                        ui.label(egui::RichText::new(candidate.platform_id).color(theme::muted(ui)));
                        ui.label(&candidate.reason);
                        if candidate.state == CandidateState::Ready {
                            ui.label(egui::RichText::new("Eligible evidence was found; final launch checks still run when you play.").small().color(theme::muted(ui)));
                        }
                        if !candidate.evidence.is_empty() {
                            widgets::technical_details(ui, ("emulator-candidate", candidate.adapter_id, candidate.platform_id), |ui| {
                                for line in &candidate.evidence {
                                    ui.label(line);
                                }
                            });
                        }
                    });
                },
            );
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(name: &str, severity: DoctorSeverity, title: &str) -> Finding {
        Finding {
            id: format!("test.{name}"),
            category: DoctorCategory::EmulatorProfiles,
            subsystem: archivefs_core::diagnostics::DoctorSubsystem::EmulatorProfiles,
            severity,
            title: title.to_string(),
            explanation: format!("Evidence for {name}"),
            why_it_matters: None,
            next_step: None,
            evidence: vec![format!("{name} executable")],
            affected: None,
            recovery: None,
            repair: None,
            measurements: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn multiple_arcade_and_amiga_candidates_remain_separate() {
        let candidates = build_candidates(None, RetroArchSetupStatus::NotChecked, None, "");
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Arcade" && c.adapter_id == "mame")
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Arcade" && c.adapter_id == "fbneo")
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Amiga" && c.adapter_id == "amiberry")
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Amiga" && c.adapter_id == "fsuae")
        );
    }

    #[test]
    fn native_sameboy_and_mgba_candidates_are_distinct() {
        let candidates = build_candidates(None, RetroArchSetupStatus::NotChecked, None, "");
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Game Boy" && c.adapter_id == "mgba")
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Game Boy" && c.adapter_id == "sameboy")
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Game Boy Color" && c.adapter_id == "sameboy")
        );
    }

    #[test]
    fn state_projection_is_conservative_and_explains_missing_installation() {
        let findings = vec![finding(
            "Dolphin",
            DoctorSeverity::Info,
            "Dolphin ready to launch",
        )];
        let candidates = build_candidates(
            Some(&findings),
            RetroArchSetupStatus::NotChecked,
            Some("GameCube"),
            "",
        );
        assert_eq!(candidates[0].state, CandidateState::Ready);
        let missing = candidates
            .iter()
            .find(|c| c.adapter_id == "dolphin")
            .unwrap();
        assert!(missing.reason.contains("Evidence"));
    }

    #[test]
    fn warnings_and_blockers_are_not_promoted_to_ready() {
        let findings = vec![
            finding("MAME", DoctorSeverity::Warning, "MAME setup warning"),
            finding("FBNeo", DoctorSeverity::Error, "FBNeo blocked"),
        ];
        let candidates = build_candidates(
            Some(&findings),
            RetroArchSetupStatus::NotChecked,
            Some("Arcade"),
            "",
        );
        assert_eq!(
            candidates
                .iter()
                .find(|c| c.adapter_id == "mame")
                .unwrap()
                .state,
            CandidateState::Warnings
        );
        assert_eq!(
            candidates
                .iter()
                .find(|c| c.adapter_id == "fbneo")
                .unwrap()
                .state,
            CandidateState::Blocked
        );
    }

    #[test]
    fn isolation_and_filtering_are_deterministic() {
        let vita = build_candidates(
            None,
            RetroArchSetupStatus::NotChecked,
            Some("PlayStation Vita"),
            "",
        );
        assert!(
            vita.iter()
                .all(|candidate| candidate.adapter_id == "vita3k")
        );
        let azahar = build_candidates(
            None,
            RetroArchSetupStatus::NotChecked,
            Some("Nintendo 3DS"),
            "azahar",
        );
        assert_eq!(azahar.len(), 1);
        assert_eq!(candidate_columns(1_200.0), 3);
        assert_eq!(candidate_columns(800.0), 2);
        assert_eq!(candidate_columns(600.0), 1);
    }
}
