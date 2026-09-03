//! The "Collection coverage" panel on the Verify Games (DAT Sources) page.
//!
//! This is a **presentation layer only**. Every number it shows is produced
//! by the authoritative core aggregation
//! ([`archivefs_core::dat::coverage`]) and read once, on demand, by
//! [`crate::dat_sources_page`]; nothing here recomputes coverage, parses a
//! DAT, inspects a ROM, hashes a file, or starts an audit. The projection
//! functions below only turn the core structs into display strings and a
//! small set of view enums.
//!
//! # The explicit-platform rule is the core's, shown honestly here
//!
//! When [`archivefs_core::dat::coverage::ExpectedInventoryStatus`] is not
//! `Available` (source unassigned, assigned elsewhere, never validated, or
//! its recorded inventory is stale), Expected / Missing / Completion render
//! as an em dash with a plain-language reason and Full Set reads "cannot be
//! determined" - never `0`, never `0%`, never "Incomplete".

use std::collections::BTreeSet;

use archivefs_core::dat::coverage::{
    ArcadeDatSetCoverage, CompleteSetVerdict, ExpectedInventoryStatus, PlatformDatCoverage,
};
use archivefs_core::dat::model::DatEcosystem;
use eframe::egui;

use crate::ui::components as widgets;
use crate::ui::components::{ActionStyle, StatusTone};
use crate::ui::theme;

/// The initial (and page) size of the missing-identities drill-down list.
pub(crate) const MISSING_PAGE_SIZE: u32 = 100;

/// One catalogue source's coverage row in the panel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourceCoverageEntry {
    pub(crate) source_id: String,
    /// The source's own display name (No-Intro set name, Redump title, …).
    pub(crate) source_label: String,
    /// The source's explicit platform assignment, for the closed-row
    /// summary. `None` renders as "not assigned to a platform".
    pub(crate) platform: Option<String>,
    /// Whether the source is a configured, enabled entry. A disabled source
    /// still lists but its row says so.
    pub(crate) enabled: bool,
    pub(crate) load: CoverageLoad,
}

/// Where a source's coverage read stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CoverageLoad {
    /// The user has not expanded this source's coverage yet - nothing has
    /// been read.
    NotOpened,
    /// The coverage read failed (a database error). The message is shown;
    /// verification never falls back to a guess.
    Failed(String),
    /// A completed read, projected for display.
    Ready(CoverageUnitView),
}

/// The projected, display-ready coverage for one source, split by unit so
/// the Arcade set vocabulary never leaks into a canonical-entry card and
/// vice versa.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CoverageUnitView {
    Canonical(Box<CanonicalCoverageView>),
    Arcade(Box<ArcadeCoverageView>),
}

/// Whether Expected / Missing / Completion have a trustworthy denominator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExpectedStatusView {
    /// A current, durable expected inventory is available.
    Available { unique_count: u64 },
    /// No denominator. `headline` is a short label ("Not assigned to a
    /// platform"); `reason` is the plain-language explanation; `offer_help`
    /// is a first-line suggestion; `offer_validate` asks the panel to show
    /// a Validate action (reusing the page's existing one).
    Unavailable {
        headline: String,
        reason: String,
        help: Option<String>,
        offer_validate: bool,
    },
}

impl ExpectedStatusView {
    pub(crate) fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// The Full-Set verdict, mapped one-to-one from the core so it can never be
/// silently turned into "Incomplete".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FullSetView {
    Complete { extra_duplicate_archives: u64 },
    Incomplete { missing_count: u64 },
    NotProvable { reason: String },
}

/// A bounded page of missing canonical identities plus whether more exist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MissingListView {
    pub(crate) names: Vec<String>,
    pub(crate) has_more: bool,
}

/// Canonical-entry (No-Intro / Redump / TOSEC / generic Logiqx) coverage.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CanonicalCoverageView {
    pub(crate) platform: String,
    pub(crate) ecosystem_label: Option<String>,

    // ---- verification metrics (always populated) ----
    pub(crate) owned: usize,
    pub(crate) checked: usize,
    pub(crate) verified_current: usize,
    pub(crate) verified_stale: usize,
    pub(crate) probable: usize,
    pub(crate) unmatched: usize,
    pub(crate) ambiguous: usize,
    pub(crate) unknown: usize,
    pub(crate) duplicate_identities: usize,
    pub(crate) duplicate_extras: usize,

    // ---- expected-set metrics (gated) ----
    pub(crate) expected: ExpectedStatusView,
    pub(crate) missing_count: Option<u64>,
    pub(crate) completion_percent: Option<f64>,
    pub(crate) full_set: FullSetView,
    pub(crate) missing_list: MissingListView,
}

/// Arcade / FinalBurn Neo set-level coverage. Set completeness is the
/// core's dependency-aware verdict; this view never labels an incomplete
/// dependency set "verified".
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ArcadeCoverageView {
    pub(crate) platform: String,
    pub(crate) ecosystem_label: Option<String>,

    pub(crate) checked_sets: usize,
    pub(crate) complete_sets: usize,
    pub(crate) incomplete_sets: usize,
    pub(crate) needs_review_sets: usize,
    pub(crate) bad_metadata_sets: usize,
    pub(crate) stale_sets: usize,

    pub(crate) expected: ExpectedStatusView,
    pub(crate) missing_sets: Option<u64>,
    pub(crate) completion_percent: Option<f64>,
    pub(crate) full_set: FullSetView,
}

/// What the panel is asking the page to do. The page maps these onto its
/// own `DatSourcesPageAction`s - `Validate` is the page's existing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoveragePanelRequest {
    /// The user expanded this source and its coverage has not been read.
    Load { source_id: String },
    /// Re-read this source's coverage (after a re-validation, say).
    Refresh { source_id: String },
    /// Fetch the next page of missing identities for this source.
    LoadMoreMissing { source_id: String },
    /// Reuse the page's existing per-source Validate action.
    Validate { source_id: String },
}

// ---------------------------------------------------------------------------
// Pure projections: core structs -> display view. No I/O, no recomputation.
// ---------------------------------------------------------------------------

fn ecosystem_label(ecosystem: Option<DatEcosystem>) -> Option<String> {
    ecosystem.map(|value| value.label().to_string())
}

/// Whether this ecosystem's coverage uses the Arcade multi-member set unit.
pub(crate) fn is_arcade_ecosystem(ecosystem: DatEcosystem) -> bool {
    matches!(
        ecosystem,
        DatEcosystem::MAMEArcade | DatEcosystem::MAMESoftwareList | DatEcosystem::FBNeo
    )
}

fn expected_status_view(
    status: &ExpectedInventoryStatus,
    expected_unique_count: Option<u64>,
) -> ExpectedStatusView {
    match status {
        ExpectedInventoryStatus::Available { .. } => ExpectedStatusView::Available {
            unique_count: expected_unique_count.unwrap_or(0),
        },
        ExpectedInventoryStatus::PlatformUnassigned => ExpectedStatusView::Unavailable {
            headline: "Not assigned to a platform".to_string(),
            reason: "This catalogue has no platform assignment, so it can't say how many \
                     games a platform's collection should contain."
                .to_string(),
            help: Some(
                "Assign this catalogue to a platform to calculate collection coverage.".to_string(),
            ),
            offer_validate: false,
        },
        ExpectedInventoryStatus::PlatformMismatch { source_platform } => {
            ExpectedStatusView::Unavailable {
                headline: format!("Assigned to {source_platform}"),
                reason: format!(
                    "This catalogue is assigned to {source_platform}, so it does not provide \
                     an expected count for this platform."
                ),
                help: None,
                offer_validate: false,
            }
        }
        ExpectedInventoryStatus::InventoryMissing => ExpectedStatusView::Unavailable {
            headline: "Not checked yet".to_string(),
            reason: "Coverage is not available yet. Validate this catalogue to record what it \
                     expects."
                .to_string(),
            help: Some(
                "Use Validate on this catalogue to record its expected contents.".to_string(),
            ),
            offer_validate: true,
        },
        ExpectedInventoryStatus::InventoryStale { .. } => ExpectedStatusView::Unavailable {
            headline: "Needs re-validating".to_string(),
            reason: "The catalogue changed after its expected contents were recorded. Validate \
                     it again to refresh coverage."
                .to_string(),
            help: Some("Use Validate on this catalogue to refresh coverage.".to_string()),
            offer_validate: true,
        },
        ExpectedInventoryStatus::SourceUnconfigured => ExpectedStatusView::Unavailable {
            headline: "Not configured".to_string(),
            reason: "This catalogue is no longer a configured, enabled source.".to_string(),
            help: None,
            offer_validate: false,
        },
    }
}

fn full_set_view(verdict: &CompleteSetVerdict) -> FullSetView {
    match verdict {
        CompleteSetVerdict::Complete {
            extra_duplicate_archives,
        } => FullSetView::Complete {
            extra_duplicate_archives: *extra_duplicate_archives,
        },
        CompleteSetVerdict::Incomplete { missing_count } => FullSetView::Incomplete {
            missing_count: *missing_count,
        },
        CompleteSetVerdict::NotProvable { reason } => FullSetView::NotProvable {
            reason: reason.clone(),
        },
    }
}

/// Projects one canonical-entry [`PlatformDatCoverage`] plus the
/// already-fetched missing-identities page into a display view.
pub(crate) fn project_canonical(
    coverage: &PlatformDatCoverage,
    missing_list: MissingListView,
) -> CanonicalCoverageView {
    CanonicalCoverageView {
        platform: coverage.platform.clone(),
        ecosystem_label: ecosystem_label(coverage.ecosystem),
        owned: coverage.owned_applicable,
        checked: coverage.checked,
        verified_current: coverage.verified_current,
        verified_stale: coverage.verified_stale,
        probable: coverage.probable,
        unmatched: coverage.unmatched,
        ambiguous: coverage.ambiguous,
        unknown: coverage.unknown,
        duplicate_identities: coverage.duplicate_canonical_identities,
        duplicate_extras: coverage.duplicate_extra_archives,
        expected: expected_status_view(
            &coverage.expected_inventory,
            coverage.expected_unique_count,
        ),
        missing_count: coverage.missing_count,
        completion_percent: coverage.completion_percent,
        full_set: full_set_view(&coverage.complete_set),
        missing_list,
    }
}

/// Projects one [`ArcadeDatSetCoverage`] into a display view.
pub(crate) fn project_arcade(coverage: &ArcadeDatSetCoverage) -> ArcadeCoverageView {
    ArcadeCoverageView {
        platform: coverage.platform.clone(),
        ecosystem_label: ecosystem_label(coverage.ecosystem),
        checked_sets: coverage.checked_sets,
        complete_sets: coverage.complete_sets,
        incomplete_sets: coverage.incomplete_sets,
        needs_review_sets: coverage.needs_review_sets,
        bad_metadata_sets: coverage.bad_metadata_sets,
        stale_sets: coverage.stale_sets,
        expected: expected_status_view(&coverage.expected_inventory, coverage.expected_sets),
        missing_sets: coverage.missing_sets,
        completion_percent: coverage.completion_percent,
        full_set: full_set_view(&coverage.complete_set),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn thousands(value: usize) -> String {
    group_thousands(value as u64)
}

fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let bytes = digits.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    out
}

fn metric_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [150.0, 0.0],
            egui::Label::new(egui::RichText::new(label).color(theme::muted(ui))),
        );
        ui.label(egui::RichText::new(value).strong());
    });
}

fn dash_row(ui: &mut egui::Ui, label: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [150.0, 0.0],
            egui::Label::new(egui::RichText::new(label).color(theme::muted(ui))),
        );
        ui.label(egui::RichText::new("—").color(theme::muted(ui)));
    });
}

fn full_set_badge(ui: &mut egui::Ui, view: &FullSetView) {
    match view {
        FullSetView::Complete {
            extra_duplicate_archives,
        } => {
            ui.horizontal_wrapped(|ui| {
                widgets::status_badge(ui, "Full set ✓", StatusTone::Success);
                if *extra_duplicate_archives > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} duplicate extra{}",
                            group_thousands(*extra_duplicate_archives),
                            if *extra_duplicate_archives == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ))
                        .color(theme::muted(ui)),
                    );
                }
            });
        }
        FullSetView::Incomplete { missing_count } => {
            widgets::status_badge(
                ui,
                format!("Incomplete — {} missing", group_thousands(*missing_count)),
                StatusTone::Warning,
            );
        }
        FullSetView::NotProvable { reason } => {
            widgets::status_badge(ui, "Full set cannot be determined", StatusTone::Pending);
            widgets::technical_details(ui, "coverage_full_set_reason", |ui| {
                ui.label(egui::RichText::new(reason).color(theme::muted(ui)));
            });
        }
    }
}

fn completion_bar(ui: &mut egui::Ui, percent: Option<f64>) {
    match percent {
        Some(value) => {
            let fraction = (value / 100.0).clamp(0.0, 1.0) as f32;
            ui.add(
                egui::ProgressBar::new(fraction)
                    .text(format!("{value:.1}% complete"))
                    .desired_width(260.0),
            );
        }
        None => {
            ui.label(egui::RichText::new("Coverage unavailable").color(theme::muted(ui)));
        }
    }
}

fn render_expected_block(
    ui: &mut egui::Ui,
    source_id: &str,
    expected: &ExpectedStatusView,
) -> Option<CoveragePanelRequest> {
    let mut request = None;
    match expected {
        ExpectedStatusView::Available { unique_count } => {
            metric_row(ui, "Expected by catalogue", &group_thousands(*unique_count));
        }
        ExpectedStatusView::Unavailable {
            headline,
            reason,
            help,
            offer_validate,
        } => {
            dash_row(ui, "Expected by catalogue");
            dash_row(ui, "Missing");
            dash_row(ui, "Completion");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(headline)
                    .italics()
                    .color(theme::muted(ui)),
            );
            ui.label(egui::RichText::new(reason).color(theme::muted(ui)));
            if let Some(help) = help {
                ui.label(egui::RichText::new(help).color(theme::muted(ui)));
            }
            if *offer_validate
                && widgets::action_button(ui, "Validate catalogue", ActionStyle::Secondary, true)
                    .clicked()
            {
                request = Some(CoveragePanelRequest::Validate {
                    source_id: source_id.to_string(),
                });
            }
        }
    }
    request
}

fn render_canonical(
    ui: &mut egui::Ui,
    source_id: &str,
    view: &CanonicalCoverageView,
    missing_open: &mut BTreeSet<String>,
) -> Option<CoveragePanelRequest> {
    let mut request = None;

    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(&view.platform).strong());
        if let Some(ecosystem) = &view.ecosystem_label {
            widgets::info_chip(ui, ecosystem);
        }
    });
    ui.add_space(6.0);

    // Headline: completion + full set.
    completion_bar(ui, view.completion_percent);
    ui.add_space(4.0);
    full_set_badge(ui, &view.full_set);
    ui.add_space(10.0);

    // Expected / Missing (gated).
    if let Some(inner) = render_expected_block(ui, source_id, &view.expected) {
        request = Some(inner);
    }
    if view.expected.is_available() {
        match view.missing_count {
            Some(missing) => {
                metric_row(ui, "Missing", &group_thousands(missing));
                if missing > 0 {
                    let is_open = missing_open.contains(source_id);
                    if widgets::action_button(
                        ui,
                        if is_open {
                            "Hide missing games"
                        } else {
                            "View missing games"
                        },
                        ActionStyle::Quiet,
                        true,
                    )
                    .clicked()
                    {
                        if is_open {
                            missing_open.remove(source_id);
                        } else {
                            missing_open.insert(source_id.to_string());
                        }
                    }
                    if missing_open.contains(source_id) {
                        render_missing_list(ui, source_id, &view.missing_list, &mut request);
                    }
                }
            }
            None => dash_row(ui, "Missing"),
        }
    }

    ui.add_space(10.0);
    ui.separator();
    ui.label(egui::RichText::new("Your games").color(theme::muted(ui)));
    metric_row(ui, "Owned", &thousands(view.owned));
    metric_row(ui, "Checked", &thousands(view.checked));
    metric_row(ui, "Verified", &thousands(view.verified_current));
    if view.verified_stale > 0 {
        metric_row(ui, "Verified (stale)", &thousands(view.verified_stale));
    }
    if view.probable > 0 {
        metric_row(ui, "Probable (CRC only)", &thousands(view.probable));
    }
    if view.unmatched > 0 {
        metric_row(ui, "Not in catalogue", &thousands(view.unmatched));
    }
    if view.ambiguous > 0 {
        metric_row(ui, "Ambiguous", &thousands(view.ambiguous));
    }
    if view.unknown > 0 {
        metric_row(ui, "Unknown", &thousands(view.unknown));
    }
    if view.duplicate_identities > 0 {
        metric_row(
            ui,
            "Duplicate copies",
            &format!(
                "{} identit{} · {} extra archive{}",
                thousands(view.duplicate_identities),
                if view.duplicate_identities == 1 {
                    "y"
                } else {
                    "ies"
                },
                thousands(view.duplicate_extras),
                if view.duplicate_extras == 1 { "" } else { "s" },
            ),
        );
    }

    request
}

fn render_missing_list(
    ui: &mut egui::Ui,
    source_id: &str,
    list: &MissingListView,
    request: &mut Option<CoveragePanelRequest>,
) {
    widgets::card(ui, |ui| {
        if list.names.is_empty() {
            ui.label(egui::RichText::new("Loading missing games…").color(theme::muted(ui)));
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt(("coverage_missing_list", source_id))
            .max_height(240.0)
            .show(ui, |ui| {
                for name in &list.names {
                    ui.label(name);
                }
            });
        if list.has_more
            && widgets::action_button(ui, "Load more", ActionStyle::Quiet, true).clicked()
        {
            *request = Some(CoveragePanelRequest::LoadMoreMissing {
                source_id: source_id.to_string(),
            });
        }
    });
}

fn render_arcade(
    ui: &mut egui::Ui,
    source_id: &str,
    view: &ArcadeCoverageView,
) -> Option<CoveragePanelRequest> {
    let mut request = None;

    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(&view.platform).strong());
        if let Some(ecosystem) = &view.ecosystem_label {
            widgets::info_chip(ui, ecosystem);
        }
        widgets::info_chip(ui, "Arcade sets");
    });
    ui.add_space(6.0);

    completion_bar(ui, view.completion_percent);
    ui.add_space(4.0);
    full_set_badge(ui, &view.full_set);
    ui.add_space(10.0);

    match &view.expected {
        ExpectedStatusView::Available { unique_count } => {
            metric_row(ui, "Expected sets", &group_thousands(*unique_count));
            match view.missing_sets {
                Some(missing) => metric_row(ui, "Missing sets", &group_thousands(missing)),
                None => dash_row(ui, "Missing sets"),
            }
        }
        other => {
            if let Some(inner) = render_expected_block_arcade(ui, source_id, other) {
                request = Some(inner);
            }
        }
    }

    ui.add_space(10.0);
    ui.separator();
    ui.label(egui::RichText::new("Your sets").color(theme::muted(ui)));
    metric_row(ui, "Checked sets", &thousands(view.checked_sets));
    metric_row(ui, "Complete sets", &thousands(view.complete_sets));
    if view.incomplete_sets > 0 {
        metric_row(ui, "Incomplete sets", &thousands(view.incomplete_sets));
    }
    if view.needs_review_sets > 0 {
        metric_row(ui, "Needs review", &thousands(view.needs_review_sets));
    }
    if view.bad_metadata_sets > 0 {
        metric_row(ui, "Bad-dump entries", &thousands(view.bad_metadata_sets));
    }
    if view.stale_sets > 0 {
        metric_row(ui, "Stale sets", &thousands(view.stale_sets));
    }

    request
}

fn render_expected_block_arcade(
    ui: &mut egui::Ui,
    source_id: &str,
    expected: &ExpectedStatusView,
) -> Option<CoveragePanelRequest> {
    let mut request = None;
    if let ExpectedStatusView::Unavailable {
        headline,
        reason,
        help,
        offer_validate,
    } = expected
    {
        dash_row(ui, "Expected sets");
        dash_row(ui, "Missing sets");
        dash_row(ui, "Completion");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(headline)
                .italics()
                .color(theme::muted(ui)),
        );
        ui.label(egui::RichText::new(reason).color(theme::muted(ui)));
        if let Some(help) = help {
            ui.label(egui::RichText::new(help).color(theme::muted(ui)));
        }
        if *offer_validate
            && widgets::action_button(ui, "Validate catalogue", ActionStyle::Secondary, true)
                .clicked()
        {
            request = Some(CoveragePanelRequest::Validate {
                source_id: source_id.to_string(),
            });
        }
    }
    request
}

/// Draws the whole "Collection coverage" section and returns at most one
/// request for the page to act on.
pub(crate) fn show_coverage_section(
    ui: &mut egui::Ui,
    entries: &[SourceCoverageEntry],
    open_source: &mut Option<String>,
    missing_open: &mut BTreeSet<String>,
) -> Option<CoveragePanelRequest> {
    let mut request = None;

    widgets::section_header(
        ui,
        "Collection coverage",
        Some("How much of each platform's catalogue your library covers, per source."),
    );

    if entries.is_empty() {
        widgets::empty_state(
            ui,
            "No catalogues configured",
            "Add a game catalogue above to see how complete your collection is.",
            None,
        );
        return None;
    }

    for entry in entries {
        widgets::card(ui, |ui| {
            let is_open = open_source.as_deref() == Some(entry.source_id.as_str());
            ui.horizontal_wrapped(|ui| {
                let toggle =
                    ui.add(egui::Button::new(if is_open { "▾" } else { "▸" }).frame(false));
                ui.label(egui::RichText::new(&entry.source_label).strong());
                match &entry.platform {
                    Some(platform) => widgets::info_chip(ui, platform),
                    None => {
                        ui.label(
                            egui::RichText::new("not assigned to a platform")
                                .color(theme::muted(ui)),
                        );
                    }
                }
                if !entry.enabled {
                    ui.label(egui::RichText::new("(disabled)").color(theme::muted(ui)));
                }
                if toggle.clicked() || ui.add(egui::Button::new("Coverage").frame(false)).clicked()
                {
                    if is_open {
                        *open_source = None;
                    } else {
                        *open_source = Some(entry.source_id.clone());
                        if matches!(entry.load, CoverageLoad::NotOpened) {
                            request = Some(CoveragePanelRequest::Load {
                                source_id: entry.source_id.clone(),
                            });
                        }
                    }
                }
            });

            if !is_open {
                return;
            }
            ui.add_space(8.0);

            match &entry.load {
                CoverageLoad::NotOpened => {
                    ui.label(egui::RichText::new("Reading coverage…").color(theme::muted(ui)));
                    if request.is_none() {
                        request = Some(CoveragePanelRequest::Load {
                            source_id: entry.source_id.clone(),
                        });
                    }
                }
                CoverageLoad::Failed(message) => {
                    widgets::banner(
                        ui,
                        "Coverage could not be read",
                        message,
                        StatusTone::Blocked,
                    );
                }
                CoverageLoad::Ready(CoverageUnitView::Canonical(view)) => {
                    if let Some(inner) = render_canonical(ui, &entry.source_id, view, missing_open)
                    {
                        request = Some(inner);
                    }
                    ui.add_space(6.0);
                    if widgets::action_button(ui, "Refresh", ActionStyle::Quiet, true).clicked() {
                        request = Some(CoveragePanelRequest::Refresh {
                            source_id: entry.source_id.clone(),
                        });
                    }
                }
                CoverageLoad::Ready(CoverageUnitView::Arcade(view)) => {
                    if let Some(inner) = render_arcade(ui, &entry.source_id, view) {
                        request = Some(inner);
                    }
                    ui.add_space(6.0);
                    if widgets::action_button(ui, "Refresh", ActionStyle::Quiet, true).clicked() {
                        request = Some(CoveragePanelRequest::Refresh {
                            source_id: entry.source_id.clone(),
                        });
                    }
                }
            }
        });
        ui.add_space(6.0);
    }

    request
}

#[cfg(test)]
mod tests;
