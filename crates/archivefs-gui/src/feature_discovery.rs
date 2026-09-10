//! "What you can do with this item" - a compact, deterministic panel that
//! surfaces existing capabilities for the currently selected file instead of
//! requiring a user to know Selected Evidence contains tape analysis, or
//! that Disc Conversion exists as a separate destination.
//!
//! Every fact here is read from [`SelectedEvidenceReport`], which
//! `selected_evidence_page` already computed for the current selection -
//! this module performs no I/O, no hashing, and no re-analysis of its own,
//! and it is deliberately a plain top-level module (not a submodule of
//! `selected_evidence_page`) so it never needs to reach into that page's own
//! in-flux internal structure - only the report's already-`pub` fields.
//!
//! State from other pages is accepted only as an explicit, already-loaded
//! projection. This module never opens a cache, starts a scan, or guesses a
//! capability from a filename.

use super::*;
use crate::selected_evidence_page::SelectedEvidenceReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FeatureDiscoveryAction {
    /// Navigate to the existing Disc Conversion destination - never a
    /// second converter.
    OpenDiscConversion,
    OpenCheats,
    OpenRomm,
    OpenEmulatorSetup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FeatureStatus {
    Available {
        label: String,
        action_label: Option<&'static str>,
        action: Option<FeatureDiscoveryAction>,
    },
    NeedsAttention {
        label: String,
        action_label: Option<&'static str>,
        action: Option<FeatureDiscoveryAction>,
    },
    Unavailable {
        label: String,
        reason: String,
    },
}

/// Read-only state prepared by the application from caches and page-owned
/// state. `None` means the relevant subsystem has not been loaded yet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FeatureDiscoveryContext {
    pub(crate) cheats: Option<FeatureStatus>,
    pub(crate) romm: Option<FeatureStatus>,
    pub(crate) emulator: Option<FeatureStatus>,
    pub(crate) cover_available: Option<bool>,
    pub(crate) screenshot_count: Option<usize>,
    pub(crate) video_available: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeatureDiscoveryItem {
    pub(crate) label: String,
    pub(crate) action_label: Option<&'static str>,
    pub(crate) action: Option<FeatureDiscoveryAction>,
}

/// A feature that is not available right now, with the plain-language
/// reason - shown so a useful capability's *absence* is explained rather
/// than silently omitted (task requirement: empty/unavailable states must
/// say why).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeatureUnavailable {
    pub(crate) label: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FeatureDiscoveryView {
    /// Healthy/ready features - rendered with a checkmark. Deterministic
    /// order: DAT identity, then tape analysis, then format conversion -
    /// the same fixed order every time, never dependent on iteration order.
    pub(crate) available: Vec<FeatureDiscoveryItem>,
    /// Present but incomplete/needs review - rendered with a warning glyph.
    pub(crate) needs_attention: Vec<FeatureDiscoveryItem>,
    /// Not applicable to this item at all, with why.
    pub(crate) unavailable: Vec<FeatureUnavailable>,
}

/// Pure projection from an already-computed evidence report. Deterministic:
/// the same report always produces the same view, in the same order.
pub(crate) fn build_feature_discovery(report: &SelectedEvidenceReport) -> FeatureDiscoveryView {
    build_feature_discovery_with_context(report, &FeatureDiscoveryContext::default())
}

pub(crate) fn build_feature_discovery_with_context(
    report: &SelectedEvidenceReport,
    context: &FeatureDiscoveryContext,
) -> FeatureDiscoveryView {
    let mut view = FeatureDiscoveryView::default();

    // --- DAT identity -----------------------------------------------------
    use archivefs_core::platform_evidence_fusion::identity_presentation::IdentityStatus;
    match report.identity.status {
        IdentityStatus::VerifiedByDat | IdentityStatus::ContentAndDatAgree => {
            view.available.push(FeatureDiscoveryItem {
                label: "DAT verified".to_string(),
                action_label: None,
                action: None,
            });
        }
        IdentityStatus::ContentOnly | IdentityStatus::DatOnly => {
            view.needs_attention.push(FeatureDiscoveryItem {
                label: "Partial identity evidence only".to_string(),
                action_label: None,
                action: None,
            });
        }
        IdentityStatus::Ambiguous => {
            view.needs_attention.push(FeatureDiscoveryItem {
                label: "Identity is ambiguous - more than one candidate".to_string(),
                action_label: None,
                action: None,
            });
        }
        IdentityStatus::Conflict => {
            view.needs_attention.push(FeatureDiscoveryItem {
                label: "Identity evidence conflicts".to_string(),
                action_label: None,
                action: None,
            });
        }
        IdentityStatus::Unknown => {
            view.unavailable.push(FeatureUnavailable {
                label: "DAT verification".to_string(),
                reason: "This file has not been identified against a DAT catalogue yet."
                    .to_string(),
            });
        }
    }

    // --- Tape analysis ------------------------------------------------------
    //
    // No click action: the tape analysis section already renders further
    // down this same page (see `show_ready_report`), so this row is a
    // status highlight, not a second navigation target - it exists so a
    // user sees "tape analysis available" without needing to already know
    // Selected Evidence contains it.
    match &report.tape_analysis {
        Some(Ok(_)) => view.available.push(FeatureDiscoveryItem {
            label: "Tape analysis available (see below)".to_string(),
            action_label: None,
            action: None,
        }),
        Some(Err(error)) => view.needs_attention.push(FeatureDiscoveryItem {
            label: format!("Tape analysis could not complete: {error}"),
            action_label: None,
            action: None,
        }),
        None => view.unavailable.push(FeatureUnavailable {
            label: "Tape analysis".to_string(),
            reason: "No tape analysis for this file type.".to_string(),
        }),
    }

    // --- Format conversion --------------------------------------------------
    // Conservative discovery only: the real eligibility/space/target check
    // happens on the existing Disc Conversion page
    // (`optical_conversion_page::show_optical_conversion_page`) - this only
    // recognises the one source format that page documents converting
    // (CUE/BIN -> CHD) from the extension already on the evidence report,
    // never inventing a broader "universal converter" claim.
    let is_cue = report
        .path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"));
    if is_cue {
        view.available.push(FeatureDiscoveryItem {
            label: "Convertible to CHD (fingerprint-verified)".to_string(),
            action_label: Some("Convert format"),
            action: Some(FeatureDiscoveryAction::OpenDiscConversion),
        });
    } else {
        view.unavailable.push(FeatureUnavailable {
            label: "Format conversion".to_string(),
            reason: "No safe conversion is currently available for this format.".to_string(),
        });
    }

    for status in [
        context.cheats.clone(),
        context.romm.clone(),
        context.emulator.clone(),
    ] {
        match status {
            Some(FeatureStatus::Available {
                label,
                action_label,
                action,
            }) => view.available.push(FeatureDiscoveryItem {
                label,
                action_label,
                action,
            }),
            Some(FeatureStatus::NeedsAttention {
                label,
                action_label,
                action,
            }) => view.needs_attention.push(FeatureDiscoveryItem {
                label,
                action_label,
                action,
            }),
            Some(FeatureStatus::Unavailable { label, reason }) => {
                view.unavailable.push(FeatureUnavailable { label, reason });
            }
            None => {}
        }
    }

    if let Some(cover_available) = context.cover_available {
        if cover_available {
            view.available.push(FeatureDiscoveryItem {
                label: "Cover available".to_string(),
                action_label: None,
                action: None,
            });
        } else {
            view.unavailable.push(FeatureUnavailable {
                label: "Cover art".to_string(),
                reason: "No cached cover is available for this item.".to_string(),
            });
        }
    }
    if let Some(count) = context.screenshot_count {
        if count > 0 {
            view.available.push(FeatureDiscoveryItem {
                label: format!("Screenshots available ({count})"),
                action_label: None,
                action: None,
            });
        } else {
            view.unavailable.push(FeatureUnavailable {
                label: "Screenshots".to_string(),
                reason: "No screenshots are available in the loaded artwork cache.".to_string(),
            });
        }
    }
    if let Some(video_available) = context.video_available {
        if video_available {
            view.available.push(FeatureDiscoveryItem {
                label: "Video evidence available".to_string(),
                action_label: None,
                action: None,
            });
        } else {
            view.unavailable.push(FeatureUnavailable {
                label: "Video".to_string(),
                reason: "No video is available in the loaded metadata/evidence.".to_string(),
            });
        }
    }

    view
}

/// Renders the panel. Returns the one action a user picked, if any -
/// callers dispatch it exactly like every other page's own action enum
/// (see `MuseumAction`, `SelectedEvidenceAction`).
pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &FeatureDiscoveryView,
) -> Option<FeatureDiscoveryAction> {
    let mut action = None;
    widgets::section_header(ui, "What you can do", None);
    widgets::card(ui, |ui| {
        if !view.available.is_empty() {
            ui.label(egui::RichText::new("Available").strong());
            for item in &view.available {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("\u{2713} {}", item.label));
                    if let (Some(label), Some(item_action)) = (item.action_label, item.action)
                        && widgets::action_button(ui, label, widgets::ActionStyle::Secondary, true)
                            .clicked()
                    {
                        action = Some(item_action);
                    }
                });
            }
        }
        if !view.needs_attention.is_empty() {
            ui.add_space(theme::SPACE_XS);
            ui.label(egui::RichText::new("Needs attention").strong());
            for item in &view.needs_attention {
                ui.label(format!("\u{26A0} {}", item.label));
            }
        }
        if !view.unavailable.is_empty() {
            ui.add_space(theme::SPACE_XS);
            widgets::technical_details(ui, ("feature-discovery", "unavailable"), |ui| {
                for item in &view.unavailable {
                    ui.label(format!("{}: {}", item.label, item.reason));
                }
            });
        }
    });
    action
}

#[cfg(test)]
mod tests;
