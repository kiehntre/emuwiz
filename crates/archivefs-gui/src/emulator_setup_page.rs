//! Candidate-first Emulator Setup presentation.
//!
//! The candidate list is deliberately projected from core's reviewed launch
//! compatibility table. This keeps setup honest: a GUI card is not created
//! merely because an executable name sounds plausible, and the final adapter
//! preflight remains the authority for launching a particular game.

use archivefs_core::diagnostics::{DoctorCategory, DoctorSeverity, Finding, Measurement};
use archivefs_core::launch::{LAUNCH_COMPATIBILITY, LaunchCompatibility};
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
        "desmume" => "DeSmuME",
        "hatari" => "Hatari",
        "mgba" => "mGBA",
        "mesen" => "Mesen 2",
        "snes9x" => "Snes9x",
        "rmg" => "RMG",
        "mame" => "MAME",
        "fbneo" => "FBNeo",
        "cemu" => "Cemu",
        "scummvm" => "ScummVM",
        "sameboy" => "SameBoy",
        "dosbox" => "DOSBox",
        "dosbox-staging" => "DOSBox Staging",
        "stella" => "Stella",
        "vice" => "VICE",
        "openmsx" => "openMSX",
        "fuse" => "Fuse",
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

/// Read the readiness verdict produced by Doctor's existing adapter-specific
/// inspection.  Older findings expressed the same fact in their title; keep
/// that compatibility while preferring the typed measurement so standalone
/// adapters are not dependent on wording chosen for their prose.
fn finding_is_ready(finding: &Finding) -> bool {
    finding.severity == DoctorSeverity::Info
        && (matches!(
            finding.measurements.get("ready"),
            Some(Measurement::Flag(true))
        ) || finding
            .title
            .to_ascii_lowercase()
            .contains("ready to launch"))
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
    } else if finding_is_ready(finding) {
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
    candidates
}

const CANDIDATE_CARD_MIN_WIDTH: f32 = 320.0;
const CANDIDATE_CARD_MAX_WIDTH: f32 = 360.0;
const CANDIDATE_CARD_GAP: f32 = 16.0;
const CANDIDATE_CARD_MAX_COLUMNS: usize = 3;

/// The candidate grid deliberately has a modest maximum card width.  A setup
/// card is explanatory copy rather than a dashboard tile, so stretching it
/// across a very wide desktop makes the page harder to scan and leaves the
/// final row looking accidental.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CandidateGridLayout {
    columns: usize,
    card_width: f32,
    row_width: f32,
}

fn candidate_grid_layout(available_width: f32, candidate_count: usize) -> CandidateGridLayout {
    debug_assert!(candidate_count > 0);

    let available_width = available_width.max(0.0);
    let columns_that_fit = ((available_width + CANDIDATE_CARD_GAP)
        / (CANDIDATE_CARD_MIN_WIDTH + CANDIDATE_CARD_GAP))
        .floor() as usize;
    let columns = columns_that_fit
        .clamp(1, CANDIDATE_CARD_MAX_COLUMNS)
        .min(candidate_count);
    let card_width = ((available_width - CANDIDATE_CARD_GAP * columns.saturating_sub(1) as f32)
        / columns as f32)
        .min(CANDIDATE_CARD_MAX_WIDTH)
        .max(0.0);

    CandidateGridLayout {
        columns,
        card_width,
        row_width: card_width * columns as f32
            + CANDIDATE_CARD_GAP * columns.saturating_sub(1) as f32,
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
    let grid = candidate_grid_layout(ui.available_width(), candidates.len());
    for row in candidates.chunks(grid.columns) {
        ui.horizontal(|ui| {
            // Centre each row, including a short final row, without forcing a
            // single candidate to claim the entire content width.
            let row_width = grid.card_width * row.len() as f32
                + CANDIDATE_CARD_GAP * row.len().saturating_sub(1) as f32;
            ui.add_space(((ui.available_width() - row_width) / 2.0).max(0.0));
            for (index, candidate) in row.iter().enumerate() {
                ui.allocate_ui_with_layout(
                    egui::vec2(grid.card_width, 0.0),
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
                if index + 1 < row.len() {
                    ui.add_space(CANDIDATE_CARD_GAP);
                }
            }
        });
        ui.add_space(CANDIDATE_CARD_GAP);
    }
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
    fn native_game_boy_candidates_include_sameboy_from_shared_registration() {
        let candidates = build_candidates(None, RetroArchSetupStatus::NotChecked, None, "");
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Game Boy" && c.adapter_id == "mgba")
        );
        let sameboy = candidates
            .iter()
            .find(|c| c.platform_id == "Game Boy" && c.adapter_id == "sameboy")
            .expect("Game Boy SameBoy candidate");
        assert_eq!(sameboy.name, "SameBoy");
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Game Boy Color" && c.adapter_id == "sameboy")
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|c| c.platform_id == "Game Boy" && c.adapter_id == "sameboy")
                .count(),
            1,
            "SameBoy is derived once from the shared compatibility registration"
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Game Boy" && c.adapter_id == "mesen")
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Game Boy Color" && c.adapter_id == "mesen")
        );
    }

    #[test]
    fn snes9x_and_retroarch_are_distinct_snes_candidates_with_no_auto_winner() {
        let candidates = build_candidates(None, RetroArchSetupStatus::NotChecked, Some("SNES"), "");
        let snes9x = candidates
            .iter()
            .find(|c| c.platform_id == "SNES" && c.adapter_id == "snes9x")
            .expect("standalone Snes9x candidate");
        let retroarch = candidates
            .iter()
            .find(|c| c.platform_id == "SNES" && c.adapter_id == "retroarch")
            .expect("RetroArch candidate");
        // Two separate rows, neither collapsed into the other and neither
        // marked as the chosen one.
        assert_eq!(snes9x.name, "Snes9x");
        assert_eq!(retroarch.name, "RetroArch");
        assert_ne!(snes9x.adapter_id, retroarch.adapter_id);
    }

    #[test]
    fn rmg_candidate_uses_its_own_name_not_the_generic_fallback() {
        // Regression: `adapter_name()` had no `"rmg"` arm, so the N64 RMG row
        // rendered the generic "Supported emulator" fallback instead of "RMG".
        let candidates = build_candidates(None, RetroArchSetupStatus::NotChecked, Some("N64"), "");
        let rmg = candidates
            .iter()
            .find(|c| c.platform_id == "N64" && c.adapter_id == "rmg")
            .expect("standalone RMG candidate for N64");
        assert_eq!(rmg.name, "RMG");
        assert_ne!(rmg.name, "Supported emulator");
    }

    #[test]
    fn dosbox_staging_candidate_uses_canonical_name() {
        let candidates = build_candidates(None, RetroArchSetupStatus::NotChecked, Some("DOS"), "");
        let dosbox = candidates
            .iter()
            .find(|candidate| {
                candidate.platform_id == "DOS" && candidate.adapter_id == "dosbox-staging"
            })
            .expect("DOSBox Staging candidate for DOS");
        assert_eq!(dosbox.name, "DOSBox Staging");
        assert_eq!(dosbox.state, CandidateState::NotChecked);
    }

    #[test]
    fn atari2600_stella_and_retroarch_candidates_coexist_with_no_automatic_winner() {
        let candidates = build_candidates(None, RetroArchSetupStatus::NotChecked, None, "");
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Atari2600" && c.adapter_id == "stella"),
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Atari2600" && c.adapter_id == "retroarch"),
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|c| c.platform_id == "Atari2600")
                .count(),
            2,
            "Stella and RetroArch must both surface as separate candidates, never merged"
        );
        assert_eq!(
            candidates
                .iter()
                .find(|c| c.platform_id == "Atari2600" && c.adapter_id == "stella")
                .map(|c| c.name),
            Some("Stella")
        );
    }

    #[test]
    fn c64_vice_and_retroarch_candidates_coexist_with_no_automatic_winner() {
        let candidates = build_candidates(None, RetroArchSetupStatus::NotChecked, None, "");
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Commodore 64" && c.adapter_id == "vice"),
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.platform_id == "Commodore 64" && c.adapter_id == "retroarch"),
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|c| c.platform_id == "Commodore 64")
                .count(),
            2,
            "VICE and RetroArch must both surface as separate candidates, never merged"
        );
        assert_eq!(
            candidates
                .iter()
                .find(|c| c.platform_id == "Commodore 64" && c.adapter_id == "vice")
                .map(|c| c.name),
            Some("VICE")
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
    fn typed_ready_measurement_promotes_standalone_without_title_wording() {
        let mut ready = finding("PPSSPP", DoctorSeverity::Info, "PPSSPP profile inspected");
        ready
            .measurements
            .insert("ready".to_string(), Measurement::Flag(true));
        let candidates = build_candidates(
            Some(&[ready]),
            RetroArchSetupStatus::NotChecked,
            Some("PSP"),
            "",
        );
        let ppsspp = candidates
            .iter()
            .find(|candidate| candidate.adapter_id == "ppsspp")
            .expect("PPSSPP candidate");
        assert_eq!(ppsspp.state, CandidateState::Ready);
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
    }

    #[test]
    fn candidate_grid_is_responsive_without_stretching_cards() {
        let narrow = candidate_grid_layout(600.0, 12);
        assert_eq!(narrow.columns, 1);
        assert!(narrow.row_width <= 600.0);

        let compact_desktop = candidate_grid_layout(1_024.0, 12);
        assert_eq!(compact_desktop.columns, 3);
        assert!(compact_desktop.row_width <= 1_024.0);

        let desktop = candidate_grid_layout(1_100.0, 12);
        assert_eq!(desktop.columns, 3);
        assert!(desktop.card_width <= CANDIDATE_CARD_MAX_WIDTH);
        assert!(desktop.row_width <= 1_100.0);

        let wide_desktop = candidate_grid_layout(1_440.0, 12);
        assert_eq!(wide_desktop.columns, 3);
        assert_eq!(wide_desktop.card_width, CANDIDATE_CARD_MAX_WIDTH);
        assert!(wide_desktop.row_width < 1_440.0);

        let wide = candidate_grid_layout(1_920.0, 12);
        assert_eq!(wide.columns, 3);
        assert_eq!(wide.card_width, CANDIDATE_CARD_MAX_WIDTH);
        assert!(wide.row_width < 1_920.0);
    }

    #[test]
    fn candidate_grid_keeps_small_result_sets_compact_and_overflow_free() {
        let single = candidate_grid_layout(1_920.0, 1);
        assert_eq!(single.columns, 1);
        assert_eq!(single.card_width, CANDIDATE_CARD_MAX_WIDTH);
        assert!(single.row_width <= 1_920.0);

        let pair = candidate_grid_layout(1_440.0, 2);
        assert_eq!(pair.columns, 2);
        assert_eq!(pair.card_width, CANDIDATE_CARD_MAX_WIDTH);
        assert!(pair.row_width <= 1_440.0);
    }
}
