//! Deterministic, local Mr Wiz guidance for native v2.
//!
//! Guidance is a projection of evidence already held by the page. It never
//! probes the network, invents a state, or opens a modal interaction.

use crate::ui::theme;
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GuidanceCategory {
    Tip,
    Explain,
    WhyBlocked,
    Success,
    Warning,
    EmptyState,
}

impl GuidanceCategory {
    fn label(self) -> &'static str {
        match self {
            Self::Tip => "Tip",
            Self::Explain => "Explain",
            Self::WhyBlocked => "Why this is blocked",
            Self::Success => "Ready",
            Self::Warning => "Warning",
            Self::EmptyState => "Nothing here yet",
        }
    }

    fn colour(self) -> egui::Color32 {
        match self {
            Self::Tip | Self::Explain => theme::TEAL,
            Self::Success => theme::SUCCESS,
            Self::WhyBlocked | Self::Warning => theme::WARNING,
            Self::EmptyState => theme::SECONDARY_TEXT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MascotState {
    Welcome,
    Explorer,
    Launch,
    Repair,
    Organise,
    Tinker,
    Archive,
    Explain,
    Success,
    Warning,
    Thinking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GuidancePage {
    Home,
    Sources,
    Games,
    Launch,
    ProblemsRepair,
    Organisation,
    CheatsMods,
    Museum,
    TapeInspector,
    ArchiveInspector,
    DatManagement,
    BiosFirmware,
    EmulatorSetup,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct GuidanceEvidence {
    pub(super) has_games: Option<bool>,
    pub(super) source_available: Option<bool>,
    pub(super) source_last_scan: Option<String>,
    pub(super) launch_identity_verified: Option<bool>,
    pub(super) blocker: Option<String>,
    pub(super) tape_format: Option<String>,
    pub(super) tape_blocks: Option<usize>,
    pub(super) dat_name: Option<String>,
    pub(super) operation_succeeded: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GuidanceContext {
    pub(super) page: GuidancePage,
    pub(super) evidence: GuidanceEvidence,
}

impl GuidanceContext {
    pub(super) fn new(page: GuidancePage) -> Self {
        Self {
            page,
            evidence: GuidanceEvidence::default(),
        }
    }

    fn key(&self) -> String {
        format!("{:?}:{:?}", self.page, self.evidence)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GuidanceTip {
    pub(super) key: &'static str,
    pub(super) category: GuidanceCategory,
    pub(super) mascot: MascotState,
    pub(super) message: String,
}

#[derive(Debug, Default)]
pub(super) struct GuidanceState {
    context_key: Option<String>,
    rotation: usize,
}

impl GuidanceState {
    pub(super) fn select(&mut self, context: &GuidanceContext) -> GuidanceTip {
        let key = context.key();
        if self.context_key.as_deref() != Some(key.as_str()) {
            self.context_key = Some(key);
            self.rotation = self.rotation.wrapping_add(1);
        }
        let tips = applicable_tips(context);
        tips[self.rotation % tips.len()].clone()
    }
}

fn applicable_tips(context: &GuidanceContext) -> Vec<GuidanceTip> {
    let e = &context.evidence;
    let mut tips = Vec::new();
    match context.page {
        GuidancePage::Home => {
            if e.has_games == Some(false) {
                tips.push(tip(
                    "home-empty",
                    GuidanceCategory::EmptyState,
                    MascotState::Welcome,
                    "No games are listed yet. Review Sources to choose which existing folders EmuWiz may scan.",
                ));
            } else {
                tips.push(tip(
                    "home-browse",
                    GuidanceCategory::Tip,
                    MascotState::Explorer,
                    "Browse first: EmuWiz keeps inspection separate from actions that change files.",
                ));
            }
        }
        GuidancePage::Sources => {
            if e.source_available == Some(false) {
                if let Some(scan) = &e.source_last_scan {
                    tips.push(tip(
                        "source-unavailable-history",
                        GuidanceCategory::Warning,
                        MascotState::Warning,
                        format!("This source is unavailable, but its catalogue history has been kept. Last scan: {scan}."),
                    ));
                } else {
                    tips.push(tip(
                        "source-unavailable",
                        GuidanceCategory::Warning,
                        MascotState::Warning,
                        "This source is unavailable. Review its configured path before scanning again.",
                    ));
                }
            } else if let Some(scan) = &e.source_last_scan {
                tips.push(tip(
                    "source-last-scan",
                    GuidanceCategory::Explain,
                    MascotState::Explorer,
                    format!("This folder was last scanned at {scan}."),
                ));
            } else {
                tips.push(tip(
                    "source-review",
                    GuidanceCategory::Tip,
                    MascotState::Explorer,
                    "Review a folder before scanning it; browsing does not move or rename source files.",
                ));
            }
        }
        GuidancePage::Launch => {
            if e.launch_identity_verified == Some(false) {
                tips.push(tip(
                    "launch-identity-blocked",
                    GuidanceCategory::WhyBlocked,
                    MascotState::Launch,
                    "This game is blocked because its identity has not been verified.",
                ));
            } else if e.launch_identity_verified == Some(true) {
                tips.push(tip(
                    "launch-verified",
                    GuidanceCategory::Success,
                    MascotState::Launch,
                    "Identity is verified; EmuWiz will still check the emulator, media and firmware before launch.",
                ));
            } else {
                tips.push(tip(
                    "launch-checks",
                    GuidanceCategory::Explain,
                    MascotState::Thinking,
                    "Launch readiness checks the selected game's identity and required emulator setup together.",
                ));
            }
        }
        GuidancePage::ProblemsRepair => {
            if let Some(blocker) = &e.blocker {
                tips.push(tip(
                    "problem-blocker",
                    GuidanceCategory::WhyBlocked,
                    MascotState::Repair,
                    blocker.clone(),
                ));
            } else {
                tips.push(tip(
                    "problem-review",
                    GuidanceCategory::Tip,
                    MascotState::Repair,
                    "Review the evidence and preview a repair before confirming it.",
                ));
            }
        }
        GuidancePage::Organisation => {
            tips.push(tip(
                "organisation-preview",
                GuidanceCategory::Explain,
                MascotState::Organise,
                "Organisation starts with a preview. Originals stay protected until you explicitly confirm a reviewed transaction.",
            ));
        }
        GuidancePage::CheatsMods => tips.push(tip(
            "cheats-mods-review",
            GuidanceCategory::Tip,
            MascotState::Tinker,
            "Preview a cheat or mod change first; the selected game's identity determines what can be applied safely.",
        )),
        GuidancePage::Museum => tips.push(tip(
            "museum-browse",
            GuidanceCategory::Explain,
            MascotState::Archive,
            "Museum views the current catalogue by platform; it does not alter games.",
        )),
        GuidancePage::TapeInspector => {
            if let (Some(format), Some(blocks)) = (&e.tape_format, e.tape_blocks) {
                tips.push(tip(
                    "tape-structure",
                    GuidanceCategory::Explain,
                    MascotState::Explain,
                    format!("This {format} contains {blocks} blocks. TZX can store timing information that a plain TAP cannot."),
                ));
            } else {
                tips.push(tip(
                    "tape-review",
                    GuidanceCategory::Explain,
                    MascotState::Archive,
                    "Tape inspection reports structure from the selected file; no conversion is performed by browsing.",
                ));
            }
        }
        GuidancePage::ArchiveInspector => tips.push(tip(
            "archive-review",
            GuidanceCategory::Explain,
            MascotState::Archive,
            "Archive inspection lists bounded member evidence without extracting or changing the source archive.",
        )),
        GuidancePage::DatManagement => {
            if let Some(name) = &e.dat_name {
                tips.push(tip(
                    "dat-provenance",
                    GuidanceCategory::Success,
                    MascotState::Success,
                    format!("This DAT supplied the identity used for this match: {name}."),
                ));
            } else {
                tips.push(tip(
                    "dat-review",
                    GuidanceCategory::Explain,
                    MascotState::Explain,
                    "DAT Management keeps identity data versioned and reviewable before it is used for matching.",
                ));
            }
        }
        GuidancePage::BiosFirmware => tips.push(tip(
            "firmware-review",
            GuidanceCategory::Explain,
            MascotState::Explain,
            "Review required firmware and detected evidence here; EmuWiz does not provide copyrighted firmware.",
        )),
        GuidancePage::EmulatorSetup => tips.push(tip(
            "emulator-readiness",
            GuidanceCategory::Explain,
            MascotState::Explain,
            "Emulator Setup reports detected installations and readiness; it does not prove that every game can launch.",
        )),
        GuidancePage::Games => tips.push(tip(
            "games-browse",
            GuidanceCategory::Tip,
            MascotState::Explorer,
            "Games is the catalogue view. Select a title to review identity and readiness before launching.",
        )),
    }
    tips
}

fn tip(
    key: &'static str,
    category: GuidanceCategory,
    mascot: MascotState,
    message: impl Into<String>,
) -> GuidanceTip {
    GuidanceTip {
        key,
        category,
        mascot,
        message: message.into(),
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut GuidanceState, context: GuidanceContext) {
    let selected = state.select(&context);
    egui::Frame::new()
        .fill(selected.category.colour().gamma_multiply(0.10))
        .stroke(egui::Stroke::new(
            1.0_f32,
            selected.category.colour().gamma_multiply(0.55),
        ))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("Mr Wiz · {}", selected.category.label()));
                ui.label(selected.message);
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_blocker_uses_only_explicit_identity_evidence() {
        let mut context = GuidanceContext::new(GuidancePage::Launch);
        context.evidence.launch_identity_verified = Some(false);
        let mut state = GuidanceState::default();
        let tip = state.select(&context);
        assert_eq!(tip.category, GuidanceCategory::WhyBlocked);
        assert!(tip.message.contains("identity has not been verified"));
    }

    #[test]
    fn source_tip_does_not_invent_a_scan_time() {
        let context = GuidanceContext::new(GuidancePage::Sources);
        let mut state = GuidanceState::default();
        let tip = state.select(&context);
        assert!(!tip.message.contains("last scanned"));
        assert!(!tip.message.contains("202"));
    }

    #[test]
    fn tape_tip_uses_real_block_count_and_format() {
        let mut context = GuidanceContext::new(GuidancePage::TapeInspector);
        context.evidence.tape_format = Some("TZX".into());
        context.evidence.tape_blocks = Some(12);
        let mut state = GuidanceState::default();
        let tip = state.select(&context);
        assert!(tip.message.contains("TZX contains 12 blocks"));
    }

    #[test]
    fn selection_is_stable_for_a_context_and_changes_after_context_change() {
        let mut state = GuidanceState::default();
        let home = GuidanceContext::new(GuidancePage::Home);
        let first = state.select(&home);
        assert_eq!(state.select(&home), first);
        let mut games = GuidanceContext::new(GuidancePage::Games);
        games.evidence.has_games = Some(true);
        let next = state.select(&games);
        assert_ne!(next.key, first.key);
    }

    #[test]
    fn mascot_states_cover_the_art_direction_hook() {
        let states = [
            MascotState::Welcome,
            MascotState::Explorer,
            MascotState::Launch,
            MascotState::Repair,
            MascotState::Organise,
            MascotState::Tinker,
            MascotState::Archive,
            MascotState::Explain,
            MascotState::Success,
            MascotState::Warning,
            MascotState::Thinking,
        ];
        assert_eq!(states.len(), 11);
    }
}
