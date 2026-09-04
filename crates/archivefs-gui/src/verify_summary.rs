//! The compact, decision-first front of Verify.
//!
//! This module deliberately consumes only state already held by the DAT page.
//! It does not parse catalogues, walk the library, contact providers, or invent
//! a denominator when the core has not produced one.
//!
//! RomM's `RommDuplicateProviderSummary` and `RommStaleSummary` are deliberately
//! not composed here yet. Their cached, generation-aware state currently belongs
//! to the Sources/RomM `main.rs` seam; duplicating report computation here would
//! make Verify expensive and risk disagreeing with the existing RomM surface.

use archivefs_core::identity_source::no_intro::{
    ManagedNoIntroStatusReport, NoIntroLifecycleHealth,
};
use eframe::egui;

use super::{DatHealthState, DatSourcesPageView};
use crate::dat_coverage_panel::{CoverageLoad, CoverageUnitView, ExpectedStatusView};
use crate::ui::{components as widgets, theme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifyHealthView {
    pub(crate) catalogue_count: usize,
    pub(crate) checked_count: usize,
    pub(crate) healthy_count: usize,
    pub(crate) attention_count: usize,
    pub(crate) coverage_count: usize,
    pub(crate) coverage_loaded: usize,
    pub(crate) no_intro: NoIntroHealthView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoIntroHealthView {
    Current { platforms: usize, rollback: usize },
    Stale,
    Unknown,
    Conflict,
    Invalid,
    NoCurrent,
    Unavailable,
}

pub(crate) fn build(view: &DatSourcesPageView) -> VerifyHealthView {
    let checked_count = view
        .rows
        .iter()
        .filter(|row| row.health_state.is_checked())
        .count();
    let healthy_count = view
        .rows
        .iter()
        .filter(|row| matches!(row.health_state, DatHealthState::Valid))
        .count();
    let attention_count = view
        .rows
        .iter()
        .filter(|row| {
            row.health_stale
                || matches!(
                    row.health_state,
                    DatHealthState::Invalid | DatHealthState::Unreadable
                )
        })
        .count()
        + view.load_problems.len()
        + view.unresolved.len();
    let coverage_loaded = view
        .coverage_sources
        .iter()
        .filter(|source| matches!(source.load, CoverageLoad::Ready(_)))
        .count();
    VerifyHealthView {
        catalogue_count: view.rows.len(),
        checked_count,
        healthy_count,
        attention_count,
        coverage_count: view.coverage_sources.len(),
        coverage_loaded,
        no_intro: no_intro_health(
            view.no_intro_status.as_ref(),
            view.no_intro_status_error.is_some(),
        ),
    }
}

fn no_intro_health(
    report: Option<&ManagedNoIntroStatusReport>,
    unavailable: bool,
) -> NoIntroHealthView {
    let Some(report) = report else {
        return if unavailable {
            NoIntroHealthView::Unavailable
        } else {
            NoIntroHealthView::NoCurrent
        };
    };
    match report.health {
        NoIntroLifecycleHealth::Healthy => NoIntroHealthView::Current {
            platforms: report.summary.platforms_covered,
            rollback: report.summary.rollback_available,
        },
        NoIntroLifecycleHealth::Stale => NoIntroHealthView::Stale,
        NoIntroLifecycleHealth::Unknown => NoIntroHealthView::Unknown,
        NoIntroLifecycleHealth::Conflict => NoIntroHealthView::Conflict,
        NoIntroLifecycleHealth::Invalid => NoIntroHealthView::Invalid,
        NoIntroLifecycleHealth::NoCurrent => NoIntroHealthView::NoCurrent,
    }
}

pub(crate) fn show(ui: &mut egui::Ui, view: &DatSourcesPageView) {
    let health = build(view);
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Verify your collection");
            if health.catalogue_count == 0 {
                widgets::status_badge(ui, "Not set up", widgets::StatusTone::Info);
            } else if health.attention_count == 0 && health.checked_count > 0 {
                widgets::status_badge(ui, "Healthy", widgets::StatusTone::Success);
            } else if health.attention_count > 0 {
                widgets::status_badge(ui, "Needs attention", widgets::StatusTone::Warning);
            } else {
                widgets::status_badge(ui, "Not checked", widgets::StatusTone::Info);
            }
        });
        ui.label(
            "Trusted catalogues help EmuWiz identify games and show what still needs review. Verification is read-only.",
        );
        if health.catalogue_count == 0 {
            ui.strong("No DATs added");
            ui.label("Add trusted catalogue data to check game names, versions, and known-good identities.");
        }
        ui.add_space(theme::SECTION_GAP / 2.0);
        ui.columns(4, |columns| {
            metric(&mut columns[0], "Catalogues", health.catalogue_count, None);
            metric(
                &mut columns[1],
                "Checked",
                health.checked_count,
                Some(health.catalogue_count),
            );
            metric(
                &mut columns[2],
                "Healthy",
                health.healthy_count,
                Some(health.catalogue_count),
            );
            metric(
                &mut columns[3],
                "Needs attention",
                health.attention_count,
                None,
            );
        });
        ui.add_space(theme::SECTION_GAP / 2.0);
        ui.label(
            "Use the catalogue controls below to validate sources, then open a platform row for coverage details.",
        );
    });

    if health.attention_count > 0 {
        widgets::section_header(
            ui,
            "What needs attention",
            Some("Only current catalogue evidence is listed here."),
        );
        widgets::card(ui, |ui| {
            for row in &view.rows {
                if row.health_stale {
                    ui.label(format!(
                        "{} — catalogue changed; validate it again",
                        row.display_name
                    ));
                } else if matches!(
                    row.health_state,
                    DatHealthState::Invalid | DatHealthState::Unreadable
                ) {
                    ui.label(format!(
                        "{} — {}",
                        row.display_name,
                        row.health_state.label()
                    ));
                }
            }
            if !view.load_problems.is_empty() {
                ui.label(format!(
                    "{} catalogue load issue(s) need review",
                    view.load_problems.len()
                ));
            }
            if !view.unresolved.is_empty() {
                ui.label(format!(
                    "{} saved setting(s) are not understood by this build",
                    view.unresolved.len()
                ));
            }
        });
    }

    show_catalogue_status(ui, view);
    show_no_intro(ui, view);
}

fn metric(ui: &mut egui::Ui, label: &str, value: usize, denominator: Option<usize>) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).color(theme::muted(ui)).small());
        let value = denominator
            .filter(|denominator| *denominator > 0)
            .map_or_else(
                || value.to_string(),
                |denominator| format!("{value} / {denominator}"),
            );
        ui.label(egui::RichText::new(value).strong().size(20.0));
    });
}

fn show_catalogue_status(ui: &mut egui::Ui, view: &DatSourcesPageView) {
    widgets::section_header(
        ui,
        "Collection coverage",
        Some(
            "Platform totals appear after the core has read a source; unknown denominators stay explicit.",
        ),
    );
    if view.coverage_sources.is_empty() {
        widgets::card(ui, |ui| {
            ui.strong("No catalogue coverage yet");
            ui.label("Add trusted catalogue data to compare owned games with expected identities.");
        });
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} catalogue source(s)",
            view.coverage_sources.len()
        ));
        ui.label(format!(
            "{} coverage read",
            view.coverage_sources
                .iter()
                .filter(|source| matches!(source.load, CoverageLoad::Ready(_)))
                .count()
        ));
    });
    for source in view
        .coverage_sources
        .iter()
        .filter(|source| matches!(source.load, CoverageLoad::Ready(_)))
    {
        let Some((platform, owned, checked, verified, expected)) = coverage_line(&source.load)
        else {
            continue;
        };
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(platform);
                widgets::status_badge(
                    ui,
                    format!("{verified} verified"),
                    widgets::StatusTone::Success,
                );
            });
            ui.label(format!("Owned {owned} · Checked {checked}"));
            match expected {
                Some(expected) => ui.label(format!(
                    "Expected {expected} · Coverage {}%",
                    percentage(verified, expected)
                )),
                None => ui.label("Expected count unavailable — coverage cannot be determined yet."),
            };
        });
    }
}

fn coverage_line(load: &CoverageLoad) -> Option<(&str, usize, usize, usize, Option<u64>)> {
    let CoverageLoad::Ready(unit) = load else {
        return None;
    };
    match unit {
        CoverageUnitView::Canonical(view) => Some((
            view.platform.as_str(),
            view.owned,
            view.checked,
            view.verified_current,
            match view.expected {
                ExpectedStatusView::Available { unique_count } => Some(unique_count),
                ExpectedStatusView::Unavailable { .. } => None,
            },
        )),
        CoverageUnitView::Arcade(view) => Some((
            view.platform.as_str(),
            view.checked_sets,
            view.checked_sets,
            view.complete_sets,
            match view.expected {
                ExpectedStatusView::Available { unique_count } => Some(unique_count),
                ExpectedStatusView::Unavailable { .. } => None,
            },
        )),
    }
}

fn percentage(numerator: usize, denominator: u64) -> String {
    if denominator == 0 {
        return "—".to_string();
    }
    format!("{:.1}", numerator as f64 * 100.0 / denominator as f64)
}

fn show_no_intro(ui: &mut egui::Ui, view: &DatSourcesPageView) {
    widgets::section_header(
        ui,
        "Managed No-Intro",
        Some("Lifecycle status comes from the core's read-only managed-pack report."),
    );
    widgets::card(ui, |ui| {
        let (label, tone, detail) = match view.no_intro_status.as_ref().map(|report| report.health)
        {
            Some(NoIntroLifecycleHealth::Healthy) => (
                "CURRENT",
                widgets::StatusTone::Success,
                "The managed pack is current for its known platforms.",
            ),
            Some(NoIntroLifecycleHealth::Stale) => (
                "STALE",
                widgets::StatusTone::Warning,
                "A managed pack is no longer the current evidence for at least one platform.",
            ),
            Some(NoIntroLifecycleHealth::Unknown) => (
                "FRESHNESS UNKNOWN",
                widgets::StatusTone::Info,
                "The lifecycle report cannot establish freshness.",
            ),
            Some(NoIntroLifecycleHealth::Conflict) => (
                "CONFLICT",
                widgets::StatusTone::Warning,
                "Managed snapshots disagree about which pack is current.",
            ),
            Some(NoIntroLifecycleHealth::Invalid) => (
                "INVALID",
                widgets::StatusTone::Blocked,
                "The managed lifecycle contains invalid evidence and needs review.",
            ),
            Some(NoIntroLifecycleHealth::NoCurrent) => (
                "NO CURRENT PACK",
                widgets::StatusTone::Info,
                "Add a validated No-Intro pack to check cartridge coverage.",
            ),
            None if view.no_intro_status_error.is_none() => (
                "NO CURRENT PACK",
                widgets::StatusTone::Info,
                "Add a validated No-Intro pack to check cartridge coverage.",
            ),
            None => (
                "UNAVAILABLE",
                widgets::StatusTone::Info,
                "Managed No-Intro status is unavailable; no lifecycle conclusion is shown.",
            ),
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong("No-Intro");
            widgets::status_badge(ui, label, tone);
        });
        ui.label(detail);
        if let Some(report) = &view.no_intro_status {
            ui.label(format!(
                "{} platform(s) covered · {} historical snapshot(s) · {} rollback option(s)",
                report.summary.platforms_covered,
                report.summary.historical_snapshots,
                report.summary.rollback_available
            ));
        }
        ui.label("Snapshot IDs and lifecycle warnings remain available in technical details on the catalogue-management surface.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_current_is_neutral_and_does_not_fabricate_counts() {
        let health = no_intro_health(None, false);
        assert_eq!(health, NoIntroHealthView::NoCurrent);
    }

    #[test]
    fn unavailable_is_distinct_from_no_current() {
        assert_eq!(no_intro_health(None, true), NoIntroHealthView::Unavailable);
    }

    #[test]
    fn zero_denominator_never_becomes_zero_percent() {
        assert_eq!(percentage(0, 0), "—");
    }
}
