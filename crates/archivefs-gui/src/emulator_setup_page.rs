//! Candidate-first Emulator Setup presentation.
//!
//! The candidate list is deliberately projected from core's reviewed launch
//! compatibility table. This keeps setup honest: a GUI card is not created
//! merely because an executable name sounds plausible, and the final adapter
//! preflight remains the authority for launching a particular game.

use archivefs_core::diagnostics::{DoctorCategory, DoctorSeverity, Finding, Measurement};
use archivefs_core::emulator_environment::es_de::{
    self, DiscoveryError, EligibilityBlocker, EsDeEnvironmentReport, ExecutableSearchOutcome,
    ProfileKind,
};
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
    /// Read-only ES-DE discovery result, cached for the lifetime of this
    /// page state so `show` does not re-probe the filesystem every frame.
    /// Cleared only by constructing a fresh `EmulatorSetupPageState`
    /// (e.g. the "Check emulators" flow does not currently refresh this -
    /// see [`show_frontends`]).
    frontend_report: Option<Result<EsDeEnvironmentReport, DiscoveryError>>,
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
        "tsugaru" => "Tsugaru",
        "xroar" => "XRoar",
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
        // `ui.horizontal` centres its children on the cross (vertical) axis
        // by default - with cards of differing heights (a "Ready" card's
        // extra "Technical details" line, a longer reason, ...) that made
        // every row look staggered/masonry-like even though the layout
        // below is genuinely row-major (`chunks(grid.columns)`, one row per
        // `ui.horizontal`). Top-aligning the row is the actual fix: shorter
        // cards keep their own natural height, but every card's *top* edge
        // now lines up with the rest of its row.
        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
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

    ui.add_space(theme::SECTION_GAP);
    show_frontends(ui, state);

    action
}

/// A friendly, non-technical reason ES-DE was found but is not yet ready -
/// covers every [`EligibilityBlocker`] variant explicitly (no wildcard arm)
/// so a new blocker added upstream fails to compile here rather than
/// silently falling back to a generic message.
fn frontend_blocker_reason(blocker: &EligibilityBlocker) -> &'static str {
    match blocker {
        EligibilityBlocker::ExecutableMissing => "the ES-DE executable was not found on your PATH",
        EligibilityBlocker::ExecutableUnsafe => {
            "the ES-DE executable was found, but is not safely usable (not a plain executable file)"
        }
        EligibilityBlocker::ConfigurationRootMissing => {
            "its configuration directory (~/ES-DE) was not found"
        }
        EligibilityBlocker::ConfigurationRootUnsafe => {
            "its configuration directory exists, but is not usable"
        }
        EligibilityBlocker::ConflictingCandidates => {
            "more than one ES-DE install was found pointing at the same configuration, so \
             EmuWiz will not guess which one to use"
        }
    }
}

/// Frontends (currently just ES-DE) are not emulators - they organize and
/// launch games across many emulators - so they are deliberately kept out
/// of the `LAUNCH_COMPATIBILITY`-driven candidate grid above and shown in
/// their own small, clearly-labelled subsection instead. See "ES-DE
/// INTEGRATION VISIBILITY + SETUP FIX V1": this reads
/// `archivefs_core::emulator_environment::es_de` (already-mature,
/// read-only discovery used elsewhere by the Playing Library "Publish to
/// ES-DE" flow) and projects it here for the first time - it performs no
/// writes and does not modify any ES-DE configuration.
fn show_frontends(ui: &mut egui::Ui, state: &mut EmulatorSetupPageState) {
    if state.frontend_report.is_none() {
        state.frontend_report = Some(es_de::discover_es_de_environment_default());
    }

    widgets::section_header(
        ui,
        "Frontends",
        Some(
            "Frontends like ES-DE organize and launch games across many emulators. They are \
             set up separately from the individual emulators above.",
        ),
    );

    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("ES-DE").strong());
            ui.label(egui::RichText::new("Frontend").color(theme::muted(ui)));
        });

        match state
            .frontend_report
            .as_ref()
            .expect("populated immediately above")
        {
            Err(err) => {
                widgets::status_badge(ui, "Not detected", widgets::StatusTone::Pending);
                ui.label(format!("EmuWiz could not check for ES-DE: {err}"));
            }
            Ok(report) => {
                let native = report
                    .profiles
                    .iter()
                    .find(|profile| profile.profile_kind == ProfileKind::Native);
                // A `Native` profile is always produced by discovery (it
                // reflects the documented default `~/ES-DE` home whether or
                // not anything is actually there), so `None` here is a
                // defensive fallback rather than a reachable state today -
                // "detected" is judged by whether the `es-de` executable
                // itself was actually found, not by that profile's mere
                // existence.
                let not_installed = native.is_none_or(|profile| {
                    profile.executable.outcome != ExecutableSearchOutcome::Found
                });
                if not_installed {
                    widgets::status_badge(ui, "Not detected", widgets::StatusTone::Pending);
                    ui.label(
                        "ES-DE was not found. Install it and EmuWiz will detect it here \
                         automatically - nothing is required from you now.",
                    );
                } else {
                    match native.expect("not_installed is false only when native is Some") {
                        profile if profile.eligible => {
                            widgets::status_badge(ui, "Detected", widgets::StatusTone::Success);
                            ui.label("ES-DE is installed.");
                            ui.label(format!(
                                "ROM library home: {}",
                                profile.home_directory.path.display
                            ));
                            ui.label(format!(
                                "EmuWiz found {} configured system{}.",
                                profile.systems.len(),
                                if profile.systems.len() == 1 { "" } else { "s" }
                            ));
                            if profile.systems_may_be_incomplete {
                                ui.label(
                                    egui::RichText::new(
                                        "EmuWiz could not read ES-DE's full bundled systems list, \
                                     so this count may be incomplete.",
                                    )
                                    .small()
                                    .color(theme::muted(ui)),
                                );
                            }
                            ui.label(
                            egui::RichText::new(
                                "Per-system emulator and BIOS readiness is not checked here yet \
                                 - use Playing Library's \"Publish to ES-DE\" flow to review and \
                                 publish games.",
                            )
                            .small()
                            .color(theme::muted(ui)),
                        );
                        }
                        profile => {
                            widgets::status_badge(ui, "Needs setup", widgets::StatusTone::Warning);
                            ui.label("ES-DE was found, but is not ready yet:");
                            for blocker in &profile.blockers {
                                ui.label(format!("• {}", frontend_blocker_reason(blocker)));
                            }
                        }
                    }
                }

                widgets::technical_details(ui, ("frontend-esde", "es-de", "status"), |ui| {
                    ui.label(format!("Discovery complete: {}", report.discovery_complete));
                    for profile in &report.profiles {
                        ui.label(format!(
                            "Profile {:?} ({:?}): eligible={}",
                            profile.profile_kind, profile.provenance, profile.eligible
                        ));
                        ui.label(format!(
                            "  executable: {:?} via {:?}",
                            profile.executable.outcome, profile.executable.provenance
                        ));
                        ui.label(format!(
                            "  home directory probe: {:?}",
                            profile.home_directory.probe
                        ));
                        for finding in &profile.systems_files {
                            ui.label(format!(
                                "  systems file [{:?}] {}: {:?}",
                                finding.role, finding.path.display, finding.read
                            ));
                        }
                    }
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::emulator_environment::HostReadOnlyFilesystem;

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

    // --- physical row layout (PHYSICAL RUNTIME UX REPAIR V2) ---------------
    //
    // The real regression: `ui.horizontal` centres its children on the
    // cross (vertical) axis by default. VICE's card (no extra evidence
    // line) and RetroArch's "Ready" card (an extra "Technical details"
    // line) have different natural heights, so the same genuinely
    // row-major layout (`chunks(grid.columns)`, one `ui.horizontal`/row)
    // rendered the shorter card's top edge lower than the taller card's -
    // looking exactly like an accidental masonry layout even though no
    // masonry algorithm was ever in play.

    fn text_top_y(output: &egui::FullOutput, needle: &str) -> Option<f32> {
        fn walk(shape: &egui::Shape, needle: &str) -> Option<f32> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == needle => Some(text.pos.y),
                egui::Shape::Vec(nested) => nested.iter().find_map(|s| walk(s, needle)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|clipped| walk(&clipped.shape, needle))
    }

    #[test]
    fn candidate_rows_top_align_cards_of_different_heights() {
        let context = egui::Context::default();
        let mut state = EmulatorSetupPageState {
            platform_filter: "Commodore 64".to_string(),
            search: String::new(),
            ..Default::default()
        };
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 900.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let _ = show(
                        ui,
                        &mut state,
                        None,
                        false,
                        RetroArchSetupStatus::Ready,
                        None,
                    );
                });
            },
        );
        let vice_top = text_top_y(&output, "VICE").expect("VICE card must render");
        let retroarch_top = text_top_y(&output, "RetroArch").expect("RetroArch card must render");
        assert!(
            (vice_top - retroarch_top).abs() < 2.0,
            "cards in the same row must have aligned top edges: VICE at {vice_top}, \
             RetroArch at {retroarch_top}"
        );
    }

    // --- ES-DE frontend visibility (ES-DE INTEGRATION VISIBILITY + SETUP V1) ---
    //
    // These tests never touch the real ES-DE discovery machinery's default
    // (`discover_es_de_environment_default`, which reads real process
    // `$HOME`/`$PATH`) - each builds its own bounded temp fixture and an
    // explicit `DiscoveryEnvironment`, exactly mirroring the pattern already
    // used by `playing_library_page.rs`'s own ES-DE tests, so results never
    // depend on whatever is actually installed on the machine running the
    // test.

    struct EsDeFixture {
        root: std::path::PathBuf,
    }

    impl EsDeFixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "archivefs-gui-esde-setup-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        /// The directory ES-DE discovery actually probes for the `Native`
        /// profile - `<HOME>/ES-DE`, not `HOME` itself (see
        /// `discover_es_de_environment`'s `home_dir.join("ES-DE")`).
        fn es_de_home(&self) -> std::path::PathBuf {
            self.root.join("ES-DE")
        }

        fn env(&self) -> es_de::DiscoveryEnvironment {
            es_de::DiscoveryEnvironment {
                home: Some(self.root.clone().into_os_string()),
                path: None,
                explicit_bundled_systems_files: Vec::new(),
                appimage_search_roots: Vec::new(),
                explicit_root: None,
                explicit_appimages: Vec::new(),
                explicit_portables: Vec::new(),
            }
        }

        fn snapshot(&self) -> Vec<std::path::PathBuf> {
            fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    return;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, out);
                    } else {
                        out.push(path);
                    }
                }
            }
            let mut out = Vec::new();
            walk(&self.root, &mut out);
            out.sort();
            out
        }
    }

    impl Drop for EsDeFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn render_frontends(state: &mut EmulatorSetupPageState) -> egui::FullOutput {
        let context = egui::Context::default();
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 900.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    show_frontends(ui, state);
                });
            },
        )
    }

    fn output_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn walk(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains(needle),
                egui::Shape::Vec(nested) => nested.iter().any(|s| walk(s, needle)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| walk(&clipped.shape, needle))
    }

    #[test]
    fn es_de_not_installed_shows_not_detected_and_is_labelled_frontend() {
        let fixture = EsDeFixture::new("not-installed");
        let report = es_de::discover_es_de_environment(&HostReadOnlyFilesystem, &fixture.env())
            .expect("discovery with an explicit HOME never fails");
        let mut state = EmulatorSetupPageState {
            frontend_report: Some(Ok(report)),
            ..Default::default()
        };
        let output = render_frontends(&mut state);
        assert!(output_contains(&output, "ES-DE"), "ES-DE must be visible");
        assert!(
            output_contains(&output, "Frontend"),
            "ES-DE must be labelled as a frontend, not an emulator"
        );
        assert!(output_contains(&output, "Not detected"));
    }

    #[test]
    fn es_de_installed_and_eligible_reports_systems_without_claiming_full_readiness() {
        let fixture = EsDeFixture::new("eligible");
        // A usable home directory plus a real, executable `es-de` on PATH -
        // the two things `EsDeProfile::eligible` requires.
        let bin_dir = fixture.root.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let executable = bin_dir.join("es-de");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::create_dir_all(fixture.es_de_home().join("custom_systems")).unwrap();
        std::fs::write(
            fixture.es_de_home().join("custom_systems/es_systems.xml"),
            r#"<?xml version="1.0"?>
<systemList>
  <system>
    <name>nes</name>
    <fullname>Nintendo Entertainment System</fullname>
    <path>%ROMPATH%/nes</path>
    <extension>.nes .zip</extension>
    <command>%EMULATOR_RETROARCH% %ROM%</command>
    <platform>nes</platform>
    <theme>nes</theme>
  </system>
</systemList>
"#,
        )
        .unwrap();
        let mut env = fixture.env();
        env.path = Some(bin_dir.clone().into_os_string());
        let report = es_de::discover_es_de_environment(&HostReadOnlyFilesystem, &env)
            .expect("discovery with an explicit HOME never fails");
        assert!(
            report
                .profiles
                .iter()
                .any(|profile| profile.profile_kind == ProfileKind::Native && profile.eligible),
            "fixture must actually be eligible for this test to be meaningful"
        );
        let mut state = EmulatorSetupPageState {
            frontend_report: Some(Ok(report)),
            ..Default::default()
        };
        let output = render_frontends(&mut state);
        assert!(output_contains(&output, "Detected"));
        assert!(output_contains(&output, "1 configured system."));
        // V1 deliberately never claims per-system emulator/BIOS readiness -
        // it only reports what ES-DE discovery itself can see.
        assert!(output_contains(&output, "not checked here yet"));
    }

    #[test]
    fn es_de_executable_found_but_config_root_missing_is_reported_as_needs_setup() {
        let fixture = EsDeFixture::new("found-but-blocked");
        // An `es-de` executable really is on PATH here (so this is not the
        // "Not detected" case), but `<HOME>/ES-DE` itself is never created -
        // `EligibilityBlocker::ConfigurationRootMissing` should block
        // eligibility and the specific reason must be shown, not a silent
        // "Ready".
        let bin_dir = fixture.root.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let executable = bin_dir.join("es-de");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = fixture.env();
        env.path = Some(bin_dir.into_os_string());
        let report = es_de::discover_es_de_environment(&HostReadOnlyFilesystem, &env)
            .expect("discovery with an explicit HOME never fails");
        let native = report
            .profiles
            .iter()
            .find(|profile| profile.profile_kind == ProfileKind::Native)
            .expect("a Native profile is always produced");
        assert_eq!(native.executable.outcome, ExecutableSearchOutcome::Found);
        assert!(
            !native.eligible,
            "fixture must actually be blocked for this test to be meaningful"
        );
        let mut state = EmulatorSetupPageState {
            frontend_report: Some(Ok(report)),
            ..Default::default()
        };
        let output = render_frontends(&mut state);
        assert!(output_contains(&output, "Needs setup"));
        assert!(
            output_contains(
                &output,
                "its configuration directory (~/ES-DE) was not found"
            ),
            "the specific blocker reason must be shown, not a generic message"
        );
    }

    #[test]
    fn malformed_systems_file_renders_without_panicking() {
        let fixture = EsDeFixture::new("malformed");
        std::fs::create_dir_all(fixture.es_de_home().join("custom_systems")).unwrap();
        std::fs::write(
            fixture.es_de_home().join("custom_systems/es_systems.xml"),
            "<systemList><system><name>broken</name>",
        )
        .unwrap();
        let report = es_de::discover_es_de_environment(&HostReadOnlyFilesystem, &fixture.env())
            .expect("a malformed file is a soft diagnostic, never a hard discovery error");
        // The fixture is deliberately truncated (an unclosed `<system>`) -
        // core's own parser is expected to fail soft: no fabricated system
        // record, plus a diagnostic explaining why, never a panic and never
        // an invented "successfully parsed" system.
        let native = report
            .profiles
            .iter()
            .find(|profile| profile.profile_kind == ProfileKind::Native)
            .expect("a Native profile is always produced");
        assert!(
            native.systems.is_empty(),
            "an unclosed <system> element must never be fabricated into a parsed system"
        );
        assert!(
            native
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "systems_file_unclosed_element_at_eof"),
            "fixture must actually be malformed for this test to be meaningful"
        );
        let mut state = EmulatorSetupPageState {
            frontend_report: Some(Ok(report)),
            ..Default::default()
        };
        // The real assertion is simply that this does not panic.
        let output = render_frontends(&mut state);
        assert!(output_contains(&output, "ES-DE"));
    }

    #[test]
    fn technical_details_are_collapsed_by_default() {
        let fixture = EsDeFixture::new("collapsed");
        let report = es_de::discover_es_de_environment(&HostReadOnlyFilesystem, &fixture.env())
            .expect("discovery with an explicit HOME never fails");
        let mut state = EmulatorSetupPageState {
            frontend_report: Some(Ok(report)),
            ..Default::default()
        };
        let output = render_frontends(&mut state);
        assert!(
            !output_contains(&output, "Discovery complete:"),
            "technical details must stay collapsed until the user opens them"
        );
        assert!(output_contains(&output, "Technical details"));
    }

    #[test]
    fn discovery_never_writes_to_the_fixture() {
        let fixture = EsDeFixture::new("no-writes");
        std::fs::create_dir_all(fixture.es_de_home().join("custom_systems")).unwrap();
        std::fs::write(
            fixture.es_de_home().join("custom_systems/es_systems.xml"),
            "<systemList/>",
        )
        .unwrap();
        let before = fixture.snapshot();
        let report = es_de::discover_es_de_environment(&HostReadOnlyFilesystem, &fixture.env())
            .expect("discovery with an explicit HOME never fails");
        let mut state = EmulatorSetupPageState {
            frontend_report: Some(Ok(report)),
            ..Default::default()
        };
        let _ = render_frontends(&mut state);
        assert_eq!(
            before,
            fixture.snapshot(),
            "neither discovery nor rendering the setup card may write to ES-DE's directories"
        );
    }

    #[test]
    fn full_setup_page_still_shows_generic_emulator_candidates_alongside_frontends() {
        let context = egui::Context::default();
        let mut state = EmulatorSetupPageState {
            platform_filter: "Commodore 64".to_string(),
            search: String::new(),
            ..Default::default()
        };
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 1200.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let _ = show(
                        ui,
                        &mut state,
                        None,
                        false,
                        RetroArchSetupStatus::Ready,
                        None,
                    );
                });
            },
        );
        assert!(
            output_contains(&output, "VICE"),
            "adding the frontends section must not remove existing emulator candidates"
        );
        assert!(
            output_contains(&output, "ES-DE"),
            "ES-DE must now be visible on the same page"
        );
        assert!(output_contains(&output, "Frontend"));
    }
}
