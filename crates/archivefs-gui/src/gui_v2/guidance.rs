//! Deterministic, local Mr Wiz guidance for native v2.
//!
//! Guidance is a projection of evidence already held by the page. It never
//! probes the network, invents a state, or opens a modal interaction.
//!
//! There is exactly one engine, in the `guidance/` submodules:
//!
//! * [`model`]: the typed vocabulary (categories, levels, topics, the six mascot
//!   states, pages, offered actions, typed facts and evidence);
//! * [`script`]: the authored-script schema and its message templates;
//! * [`catalogue`]: the authored scripts (the 42 designed scripts, and the 35
//!   messages the pre-engine implementation shipped, unchanged);
//! * [`select`]: pure, deterministic selection and provenance;
//! * [`exposure`]: the in-memory repeat/suppression model.
//!
//! This file keeps the page-facing surface (`GuidanceContext`, `GuidancePage`,
//! `GuidanceState`, [`show`]) and the egui rendering. It does not choose
//! anything itself. Phase 1 is backend-only: placement, layout, actions and
//! exposure are not wired into any page yet, so what a page shows is exactly what
//! it showed before, now selected by the engine.

use crate::ui::theme;
use eframe::egui;

#[cfg(test)]
mod audit;
mod catalogue;
mod exposure;
mod model;
mod script;
mod select;
#[cfg(test)]
mod tests;

pub(super) use model::{GuidanceCategory, GuidanceContext, GuidancePage, MascotState};

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

/// The guidance a page currently shows: the selected script's Quick message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GuidanceTip {
    pub(super) key: &'static str,
    pub(super) category: GuidanceCategory,
    pub(super) mascot: MascotState,
    pub(super) message: String,
}

/// Session state for guidance. Selection itself is stateless; the exposure model
/// (repeat suppression) is held here for the Phase 2 wiring and is not consulted
/// yet, so pages behave exactly as before.
#[derive(Debug, Default)]
pub(super) struct GuidanceState {
    #[allow(dead_code)]
    exposure: exposure::ExposureState,
}

impl GuidanceState {
    /// The one message for this context, or `None` when no script applies (a hub,
    /// or a page with nothing to say, shows nothing). Pure: the same context gives
    /// the same answer regardless of what was shown before.
    pub(super) fn select(&mut self, context: &GuidanceContext) -> Option<GuidanceTip> {
        let facts = context.evidence.facts();
        let selection = select::select(context.page, &facts)?;
        let item = selection.item(model::GuidanceLevel::Quick);
        Some(GuidanceTip {
            key: item.id,
            category: item.category,
            mascot: item.mascot,
            message: item.message,
        })
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut GuidanceState, context: GuidanceContext) {
    let Some(selected) = state.select(&context) else {
        return;
    };
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
