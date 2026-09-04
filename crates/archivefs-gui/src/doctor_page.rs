//! The Doctor page: the read-only manual-scan diagnostics dashboard and its
//! own per-finding repair review/confirm flow.
//!
//! Extracted verbatim from `main.rs` (GUI extraction: Doctor/Repair
//! consolidation) - pure rendering only. State ownership, the worker-thread
//! scan gathering (`gather_doctor_inputs`/`start_doctor_scan`/
//! `poll_doctor_scan`), and the actions that actually mutate
//! (`review_doctor_repair*`/`confirm_doctor_repair*`/`cancel_doctor_repair`,
//! all of which need `&mut ArchiveFsApp` for `self.history`, `Config`, and
//! thread spawning) stay in `main.rs`, exactly per that extraction's own
//! "keep engines distinct, only move page rendering" rule - see
//! `problems_repair_page`'s module doc for how this page now reaches the
//! screen (as the "Diagnostics" tab of the consolidated "Problems & Repair"
//! destination, `MainView::Doctor` still being the actual render target).
//!
//! This is one of the two independent Doctor implementations in this GUI
//! (see `navigation::ADVANCED_NAV_GROUPS`'s own doc comment): the manual,
//! user-triggered deep scan (`DoctorScanState`, defined in `main.rs`) with
//! its own immediate per-finding repair. The other - the automatic
//! `DoctorReport` health overlay (`ToolsOverlay::DoctorChecks`) - remains
//! entirely separate and unmoved, per that same prior finding that the two
//! cannot be merged without losing capability.
//!
//! `use super::*` reaches `main.rs`'s own type definitions
//! (`DoctorScanState`, `DoctorGathered`, `DoctorRepairReview`,
//! `DoctorScanOutcome`, plus every `archivefs_core::diagnostics::*` type
//! `main.rs` already imports) - the same pattern `sources_page.rs` and
//! `navigation.rs` already use, appropriate here because this rendering
//! code is this tightly coupled to those shared type definitions.

use super::*;

/// The only thing the Doctor page can ask for in Stage 1A. There is no
/// repair action here by design: findings are read-only, and no finding
/// renders a clickable fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DoctorPageAction {
    RunScan,
    /// Open the confirmation screen. Never executes anything.
    ReviewRepair {
        action: DoctorRepairAction,
        finding_id: String,
        affected: Option<String>,
    },
    /// The only action that mutates, and only from the confirmation screen.
    ConfirmRepair,
    CancelRepair,
}

/// The compact, pure projection used by the Doctor hero. Findings and
/// coverage are counted separately: an unavailable or deferred check is
/// unknown evidence, never a zero-valued healthy result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DoctorHealthSummary {
    pub(crate) blocking: usize,
    pub(crate) warnings: usize,
    pub(crate) informational: usize,
    pub(crate) unknown: usize,
}

impl DoctorHealthSummary {
    pub(crate) fn from_scan(scan: &DoctorScan) -> Self {
        Self {
            blocking: scan.blocking_count(),
            warnings: scan.count(DoctorSeverity::Warning),
            informational: scan.count(DoctorSeverity::Info),
            unknown: scan.unavailable_subsystems().len() + scan.deferred.len(),
        }
    }
}

/// The read-only Doctor dashboard.
///
/// Shows severity counts, findings grouped by category, and an evidence
/// panel for the selected finding. Where a repair already exists elsewhere
/// in EmuWiz the finding *says so in words* and stops there - Stage 1A
/// exposes no repair control at all.
/// Draws the whole Doctor page. The parameter list is long because the page is
/// one cohesive screen; the mode flag (Gamer vs Advanced) is the only thing
/// this PR's cleanup adds to the existing seven.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_doctor_page(
    ui: &mut egui::Ui,
    state: &DoctorScanState,
    selected: &mut Option<String>,
    review: Option<&DoctorRepairReview>,
    repair_result: Option<&DoctorRepairOutcome>,
    repair_finished_at_unix_seconds: Option<i64>,
    clipboard: &mut dyn ClipboardBackend,
    gamer_view: bool,
) -> Option<DoctorPageAction> {
    let mut action = None;
    // The confirmation screen replaces the finding list while it is open, so
    // there is no way to trigger a second repair from behind it.
    if let Some(review) = review {
        return show_doctor_repair_review(ui, review);
    }
    let running = state.is_running();
    let displayed = state.displayed();

    let health = displayed
        .map(|outcome| DoctorHealthSummary::from_scan(&outcome.scan))
        .unwrap_or(DoctorHealthSummary {
            unknown: 1,
            ..Default::default()
        });

    widgets::hero_card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("System Health").size(24.0).strong());
            match displayed {
                Some(outcome) if outcome.scan.is_healthy() => {
                    widgets::status_badge(ui, "No problems found", widgets::StatusTone::Success)
                }
                Some(outcome) => widgets::status_badge(
                    ui,
                    outcome.scan.overall_severity().label(),
                    doctor_severity_tone(outcome.scan.overall_severity()),
                ),
                None => widgets::status_badge(ui, "Not checked yet", widgets::StatusTone::Pending),
            }
        });
        ui.add_space(8.0);
        let health_grid = doctor_health_grid_layout(ui.available_width());
        let metrics = [
            ("Blocking", health.blocking, widgets::StatusTone::Blocked),
            ("Warnings", health.warnings, widgets::StatusTone::Warning),
            (
                "Informational",
                health.informational,
                widgets::StatusTone::Info,
            ),
            ("Unknown", health.unknown, widgets::StatusTone::Pending),
        ];
        ui.spacing_mut().item_spacing.x = health_grid.gap;
        ui.spacing_mut().item_spacing.y = health_grid.gap;
        ui.horizontal_wrapped(|ui| {
            for (label, count, tone) in metrics {
                ui.allocate_ui_with_layout(
                    egui::vec2(health_grid.card_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| doctor_health_metric(ui, health_grid.card_width, label, count, tone),
                );
            }
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Check for problems",
                widgets::ActionStyle::Primary,
                !running,
            )
            .clicked()
            {
                action = Some(DoctorPageAction::RunScan);
            }
            if let Some(outcome) = displayed
                && widgets::action_button(ui, "Copy report", widgets::ActionStyle::Secondary, true)
                    .clicked()
            {
                let _ = clipboard.set_text(doctor_scan_report_text(outcome));
            }
        });
        ui.label(
            egui::RichText::new("Checking is read-only and will not change your files.")
                .color(theme::muted(ui))
                .small(),
        );
        widgets::technical_details(ui, "doctor-scan-safety", |ui| {
            ui.label(DOCTOR_WHAT_IT_CHECKS_NOTICE);
            ui.label(DOCTOR_READ_ONLY_NOTICE);
        });
        match displayed {
            Some(outcome) => ui.weak(format!(
                "Last run: {}",
                format_unix_timestamp_utc(outcome.finished_at_unix_seconds)
            )),
            None => ui.weak("Last run: never"),
        };
        // The scan timestamp is retained; the repair timestamp is additional.
        if let Some(repaired_at) = repair_finished_at_unix_seconds {
            ui.weak(format!(
                "Last repair: {}",
                format_unix_timestamp_utc(repaired_at)
            ));
        }
        if running {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(if displayed.is_some() {
                    "Re-checking… the previous result stays on screen until this finishes."
                } else {
                    "Checking…"
                });
            });
        }
    });

    let Some(outcome) = displayed else {
        ui.add_space(theme::SECTION_GAP);
        widgets::empty_state(
            ui,
            "No check has run yet",
            "Check for problems to review your game folders, library, storage, and emulator setup. Nothing will be changed.",
            None,
        );
        return action;
    };

    let scan = &outcome.scan;
    ui.add_space(theme::SECTION_GAP);
    if !scan.is_healthy() {
        ui.add_space(theme::SECTION_GAP);
        ui.label(egui::RichText::new("Needs Attention").size(20.0).strong());
        ui.label(
            egui::RichText::new(format!(
                "{} blocking, {} warning{} require review.",
                health.blocking,
                health.warnings,
                if health.warnings == 1 { "" } else { "s" }
            ))
            .color(theme::muted(ui)),
        );
        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            for (severity, count) in scan.counts() {
                widgets::status_badge(
                    ui,
                    format!("{}: {count}", severity.label()),
                    if count == 0 {
                        widgets::StatusTone::Pending
                    } else {
                        doctor_severity_tone(severity)
                    },
                );
            }
        });
    }
    if scan.merged_duplicate_count > 0 {
        ui.weak(format!(
            "{} duplicate finding(s) reported by more than one check were merged.",
            scan.merged_duplicate_count
        ));
    }

    if let Some(outcome) = repair_result {
        ui.add_space(theme::SECTION_GAP);
        show_doctor_repair_result(ui, outcome);
    }

    // Deliberately *not* a running draw-order counter: a compact group's
    // cards only exist on frames where that group is expanded, so a draw
    // counter would renumber every later card the moment one group opened
    // and hand it another card's expansion state. A finding's index in the
    // scan is fixed for as long as the scan is.
    let ordinals = DoctorFindingOrdinals::of(scan);

    if gamer_view {
        // Gamer View foregrounds what needs attention and summarises the rest.
        // A real scan can produce hundreds of informational findings; those are
        // counted exactly, summarised in one line, and kept fully reachable
        // behind "Technical details" - never silently dropped and never allowed
        // to bury the actionable findings.
        let mut info_count = 0usize;
        for (category, findings) in scan.by_category() {
            let actionable: Vec<&Finding> = findings
                .iter()
                .filter(|finding| finding.severity != DoctorSeverity::Info)
                .copied()
                .collect();
            info_count += findings.len() - actionable.len();
            if actionable.is_empty() {
                continue;
            }
            show_doctor_category_group(ui, category, &actionable, selected, &mut action, &ordinals);
        }
        if info_count > 0 {
            ui.add_space(theme::SECTION_GAP);
            widgets::banner(
                ui,
                &format!("{info_count} checks are informational or healthy"),
                "None of these need attention. The exported report keeps every detail, and the \
                 full list is one disclosure away.",
                widgets::StatusTone::Info,
            );
            widgets::technical_details(ui, "doctor-info-findings", |ui| {
                for (category, findings) in scan.by_category() {
                    let informational: Vec<&Finding> = findings
                        .iter()
                        .filter(|finding| finding.severity == DoctorSeverity::Info)
                        .copied()
                        .collect();
                    if informational.is_empty() {
                        continue;
                    }
                    show_doctor_category_group(
                        ui,
                        category,
                        &informational,
                        selected,
                        &mut action,
                        &ordinals,
                    );
                }
            });
        }
    } else {
        for (category, findings) in scan.by_category() {
            show_doctor_category_group(ui, category, &findings, selected, &mut action, &ordinals);
        }
    }

    ui.add_space(theme::SECTION_GAP);
    show_doctor_coverage(ui, scan);
    action
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DoctorHealthGridLayout {
    columns: usize,
    card_width: f32,
    gap: f32,
}

const DOCTOR_HEALTH_CARD_MIN_WIDTH: f32 = 148.0;
const DOCTOR_HEALTH_GRID_GAP: f32 = 8.0;

fn doctor_health_grid_layout(available_width: f32) -> DoctorHealthGridLayout {
    let width = available_width.max(0.0);
    let columns = if width >= DOCTOR_HEALTH_CARD_MIN_WIDTH * 4.0 + DOCTOR_HEALTH_GRID_GAP * 3.0 {
        4
    } else if width >= DOCTOR_HEALTH_CARD_MIN_WIDTH * 2.0 + DOCTOR_HEALTH_GRID_GAP {
        2
    } else {
        1
    };
    let card_width =
        ((width - DOCTOR_HEALTH_GRID_GAP * (columns - 1) as f32) / columns as f32).max(0.0);
    DoctorHealthGridLayout {
        columns,
        card_width,
        gap: DOCTOR_HEALTH_GRID_GAP,
    }
}

fn doctor_health_metric(
    ui: &mut egui::Ui,
    card_width: f32,
    label: &str,
    count: usize,
    tone: widgets::StatusTone,
) {
    widgets::card(ui, |ui| {
        // `card` sizes its frame from its child. Reserve the target content
        // width so the visible outer frame follows the grid allocation too.
        ui.set_min_width((card_width - 28.0).max(0.0));
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(count.to_string()).size(22.0).strong());
            widgets::status_badge(ui, label, tone);
        });
    });
}

#[cfg(test)]
mod health_grid_tests {
    use super::*;

    #[test]
    fn health_grid_uses_all_four_categories_when_they_fit() {
        let layout = doctor_health_grid_layout(900.0);
        assert_eq!(layout.columns, 4);
        assert!(layout.card_width >= DOCTOR_HEALTH_CARD_MIN_WIDTH);
        assert_eq!(
            layout.card_width,
            doctor_health_grid_layout(900.0).card_width
        );
    }

    #[test]
    fn health_grid_balances_medium_width_into_two_columns() {
        let layout = doctor_health_grid_layout(500.0);
        assert_eq!(layout.columns, 2);
        assert_eq!(layout.card_width, (500.0 - DOCTOR_HEALTH_GRID_GAP) / 2.0);
    }

    #[test]
    fn health_grid_stacks_narrow_width_without_overflow() {
        let layout = doctor_health_grid_layout(240.0);
        assert_eq!(layout.columns, 1);
        assert_eq!(layout.card_width, 240.0);
        assert!(layout.card_width >= 0.0);
    }

    #[test]
    fn four_categories_are_preserved_in_the_grid_model() {
        let labels = ["Blocking", "Warnings", "Informational", "Unknown"];
        let layout = doctor_health_grid_layout(500.0);
        let rows = labels.len().div_ceil(layout.columns);
        assert_eq!(rows, 2);
        assert_eq!(labels.last(), Some(&"Unknown"));
    }
}

/// One category's findings inside a collapsible section, shared by both view
/// modes so the card rendering is identical wherever it appears.
fn show_doctor_category_group(
    ui: &mut egui::Ui,
    category: DoctorCategory,
    findings: &[&Finding],
    selected: &mut Option<String>,
    action: &mut Option<DoctorPageAction>,
    ordinals: &DoctorFindingOrdinals,
) {
    ui.add_space(theme::SECTION_GAP);
    egui::CollapsingHeader::new(format!("{} ({})", category.label(), findings.len()))
        .id_salt(("doctor-category-v2", category.label()))
        // Keep large warning/info inventories compact. Core setup failures
        // remain immediately visible because they are actionable.
        .default_open(
            cfg!(test)
                || matches!(
                    category,
                    DoctorCategory::Configuration
                        | DoctorCategory::Filesystems
                        | DoctorCategory::MountRoot
                        | DoctorCategory::Mounts
                ),
        )
        .show(ui, |ui| {
            for repeated in doctor_presentation_groups(findings) {
                if repeated_doctor_group_is_compact(&repeated) {
                    show_repeated_doctor_group(ui, &repeated, selected, action, ordinals);
                } else {
                    for finding in repeated {
                        show_doctor_finding_card(ui, finding, selected, action, ordinals);
                        ui.add_space(6.0);
                    }
                }
                ui.add_space(6.0);
            }
        });
}

const DOCTOR_REPEATED_GROUP_EXAMPLES: usize = 10;

fn doctor_presentation_groups<'a>(findings: &[&'a Finding]) -> Vec<Vec<&'a Finding>> {
    let mut positions = HashMap::<&str, usize>::new();
    let mut groups = Vec::<Vec<&Finding>>::new();
    for finding in findings {
        if let Some(position) = positions.get(finding.id.as_str()).copied() {
            groups[position].push(*finding);
        } else {
            positions.insert(finding.id.as_str(), groups.len());
            groups.push(vec![*finding]);
        }
    }
    groups
}

fn repeated_doctor_group_is_compact(findings: &[&Finding]) -> bool {
    findings.len() > DOCTOR_REPEATED_GROUP_EXAMPLES
        && findings.first().is_some_and(|finding| {
            matches!(
                finding.id.as_str(),
                "mounts.historical_failure"
                    | "mounts.not_required"
                    | "mounts.failure_evidence_incomplete"
            )
        })
}

pub(crate) fn repeated_doctor_group_heading(finding: &Finding, count: usize) -> String {
    match finding.id.as_str() {
        "mounts.historical_failure" => format!("Historical mount failures: {count}"),
        "mounts.not_required" => format!("{count} loose ROMs are healthy"),
        "mounts.failure_evidence_incomplete" => {
            format!("Mount results with insufficient evidence: {count}")
        }
        _ => format!("{}: {count}", finding.title),
    }
}

/// The friendly plain-language line shown for a compact repeated group, if it
/// has one. Technical detail stays in the individual findings and the "Show
/// all" expansion.
pub(crate) fn repeated_doctor_group_explanation(finding: &Finding) -> Option<&'static str> {
    match finding.id.as_str() {
        "mounts.not_required" => Some("These games can be used directly. Nothing needs fixing."),
        "mounts.historical_failure" => Some(
            "These were mount problems in the past. They are shown for reference; nothing is \
             broken right now.",
        ),
        "mounts.failure_evidence_incomplete" => Some(
            "Some earlier mount results did not record enough detail to be certain. They are \
             kept rather than guessed at.",
        ),
        _ => None,
    }
}

fn repeated_doctor_group_counts(findings: &[&Finding], key: &str) -> String {
    let mut counts = BTreeMap::<String, usize>::new();
    for finding in findings {
        if let Some(value) = finding.measurements.get(key) {
            *counts.entry(value.to_string()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(value, count)| format!("{value}: {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A key that identifies one *rendered* finding card uniquely and stably.
///
/// `Finding::id` names the finding's **kind**, not the occurrence:
/// `doctor_presentation_groups` exists precisely because a real scan
/// produces hundreds of findings sharing one id (`library.unknown_platform`,
/// `mounts.not_required`, ...). Using that id as the expansion key meant
/// every card of a kind shared one piece of state, so "Details" on one card
/// expanded all of them - and every one of those cards then built its
/// "Measured values" disclosure with the *same* egui widget id. egui
/// resolved that clash by reporting "First use of widget ID …" over the
/// header and letting each colliding widget overwrite the previous one's
/// state within the frame, which is why clicking the triangle looked
/// completely inert.
///
/// The ordinal supplies the missing uniqueness. It is stable for as long as
/// a scan result is on screen, which is exactly as long as the expansion
/// state should survive; a new scan re-numbers, and the selection is
/// re-validated against the new findings when that happens.
pub(crate) fn doctor_finding_key(finding: &Finding, ordinal: usize) -> String {
    format!("{}#{ordinal}", finding.id)
}

/// Each finding's index in the scan that produced it.
///
/// Keyed by address rather than by content because two findings of the same
/// kind can legitimately carry identical content - and because the addresses
/// are those of the scan's own `findings` elements, which live as long as the
/// scan does. Nothing is ever dereferenced through these keys.
pub(crate) struct DoctorFindingOrdinals(HashMap<*const Finding, usize>);

impl DoctorFindingOrdinals {
    pub(crate) fn of(scan: &DoctorScan) -> Self {
        Self(
            scan.findings
                .iter()
                .enumerate()
                .map(|(index, finding)| (std::ptr::from_ref(finding), index))
                .collect(),
        )
    }

    /// A finding not drawn from this scan cannot collide with one that was,
    /// because no real index reaches `usize::MAX`.
    pub(crate) fn ordinal(&self, finding: &Finding) -> usize {
        self.0
            .get(&std::ptr::from_ref(finding))
            .copied()
            .unwrap_or(usize::MAX)
    }
}

/// The `Finding::id` part of a key built by `doctor_finding_key`.
///
/// The stored selection is a key, but the two places that invalidate it
/// (a fresh scan, and a completed repair) reason about finding *kinds*, so
/// they compare on this.
pub(crate) fn doctor_finding_key_id(key: &str) -> &str {
    key.rsplit_once('#').map_or(key, |(id, _)| id)
}

fn show_repeated_doctor_group(
    ui: &mut egui::Ui,
    findings: &[&Finding],
    selected: &mut Option<String>,
    action: &mut Option<DoctorPageAction>,
    ordinals: &DoctorFindingOrdinals,
) {
    let Some(first) = findings.first() else {
        return;
    };
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                first.severity.label(),
                doctor_severity_tone(first.severity),
            );
            ui.label(
                egui::RichText::new(repeated_doctor_group_heading(first, findings.len())).strong(),
            );
        });
        ui.add(
            egui::Label::new(
                repeated_doctor_group_explanation(first).unwrap_or(&first.explanation),
            )
            .wrap(),
        );
        egui::CollapsingHeader::new("Show examples")
            .id_salt(("doctor-repeated-examples", first.id.as_str()))
            .default_open(false)
            .show(ui, |ui| {
                ui.strong(format!(
                    "First {} of {}",
                    DOCTOR_REPEATED_GROUP_EXAMPLES,
                    findings.len()
                ));
                for finding in findings.iter().take(DOCTOR_REPEATED_GROUP_EXAMPLES) {
                    if let Some(affected) = &finding.affected {
                        ui.add(egui::Label::new(format!("• {}", affected.display)).wrap());
                    }
                }
            });
        egui::CollapsingHeader::new("Show details")
            .id_salt(("doctor-repeated-details", first.id.as_str()))
            .default_open(false)
            .show(ui, |ui| {
                for (label, key) in [
                    ("By reason", "reason"),
                    ("By platform", "platform"),
                    ("By media kind", "media_kind"),
                    ("By evidence", "mount_failure_scope"),
                ] {
                    let counts = repeated_doctor_group_counts(findings, key);
                    if !counts.is_empty() {
                        ui.label(format!("{label}: {counts}"));
                    }
                }
                egui::CollapsingHeader::new(format!("Show all {} findings", findings.len()))
                    .id_salt(("doctor-repeated-group", first.id.as_str()))
                    .default_open(false)
                    .show(ui, |ui| {
                        for finding in findings {
                            show_doctor_finding_card(ui, finding, selected, action, ordinals);
                            ui.add_space(6.0);
                        }
                    });
                ui.weak(
                    "Copy report exports every underlying finding; grouping changes presentation only.",
                );
            });
    });
}

fn show_doctor_finding_card(
    ui: &mut egui::Ui,
    finding: &Finding,
    selected: &mut Option<String>,
    action: &mut Option<DoctorPageAction>,
    ordinals: &DoctorFindingOrdinals,
) {
    // This card's own key, not its kind's - two findings sharing an id must
    // open and close independently.
    let key = doctor_finding_key(finding, ordinals.ordinal(finding));
    let is_selected = selected.as_deref() == Some(key.as_str());
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                finding.severity.label(),
                doctor_severity_tone(finding.severity),
            );
            ui.label(egui::RichText::new(&finding.title).strong());
        });
        let explanation_is_technical = finding_explanation_is_technical(&finding.explanation);
        if explanation_is_technical {
            ui.weak("Open Details for guidance and technical information.");
        } else {
            ui.add(egui::Label::new(&finding.explanation).wrap());
        }
        if widgets::action_button(
            ui,
            if is_selected {
                "Hide details"
            } else {
                "Details"
            },
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
        {
            *selected = if is_selected { None } else { Some(key.clone()) };
        }
        if let Some(repair) = finding.offered_repair() {
            ui.horizontal_wrapped(|ui| {
                widgets::status_badge(ui, "Repair available", widgets::StatusTone::Active);
                ui.label(repair.title);
                widgets::status_badge(
                    ui,
                    match repair.risk {
                        archivefs_core::diagnostics::repair::DoctorRepairRisk::Safe => {
                            "Safe · confirmation required"
                        }
                        archivefs_core::diagnostics::repair::DoctorRepairRisk::NeedsConfirmation => {
                            "Changes real state · confirmation required"
                        }
                    },
                    widgets::StatusTone::Warning,
                );
            });
            if widgets::action_button(ui, "Review repair", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                *action = Some(DoctorPageAction::ReviewRepair {
                    action: repair.action,
                    finding_id: finding.id.clone(),
                    affected: finding.affected.as_ref().map(|path| path.display.clone()),
                });
            }
        }
        if is_selected {
            show_doctor_finding_details(ui, finding, &key);
        }
    });
}

/// The confirmation screen. The only route to executing a repair, and it
/// states every fact the milestone requires before a person approves.
fn show_doctor_repair_review(
    ui: &mut egui::Ui,
    review: &DoctorRepairReview,
) -> Option<DoctorPageAction> {
    let mut action = None;
    let spec = review.action.spec();
    widgets::card(ui, |ui| {
        ui.label(
            egui::RichText::new("Review this repair")
                .size(17.0)
                .strong(),
        );
        ui.add_space(6.0);
        ui.add(egui::Label::new(format!("Finding: {}", review.finding_title)).wrap());
        match &review.affected {
            Some(affected) => {
                ui.add(egui::Label::new(format!("Affected resource: {affected}")).wrap());
            }
            None => {
                ui.label("Affected resource: this EmuWiz installation");
            }
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Repair").strong());
        ui.add(egui::Label::new(spec.title).wrap());
        ui.weak(format!("Runs the existing {}", spec.invokes));

        ui.add_space(6.0);
        ui.label(egui::RichText::new("Exactly what will change").strong());
        ui.add(egui::Label::new(spec.expected_mutation).wrap());
        if spec.performs_library_scan {
            widgets::banner(
                ui,
                "This rescans your library",
                "As part of its existing implementation, this repair scans every configured source folder. Nothing in your library is modified, but on a large library it can take a while.",
                widgets::StatusTone::Warning,
            );
        }

        ui.add_space(6.0);
        ui.label(egui::RichText::new("What will not be touched").strong());
        ui.add(egui::Label::new(spec.never_touches).wrap());

        if !review.evidence.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Evidence for this repair").strong());
            for item in &review.evidence {
                ui.add(egui::Label::new(format!("• {item}")).wrap());
            }
        }

        ui.add_space(6.0);
        ui.label(egui::RichText::new("Afterwards").strong());
        ui.add(egui::Label::new(spec.verification).wrap());
        ui.add(egui::Label::new(format!("Undo: {}", spec.undo.label())).wrap());

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(DoctorPageAction::CancelRepair);
            }
            if widgets::action_button(ui, "Confirm repair", widgets::ActionStyle::Primary, true)
                .clicked()
            {
                action = Some(DoctorPageAction::ConfirmRepair);
            }
        });
    });
    action
}

/// What one repair attempt did, including when it did not work.
fn show_doctor_repair_result(ui: &mut egui::Ui, outcome: &DoctorRepairOutcome) {
    let record = &outcome.record;
    let (title, tone) = match record.status {
        DoctorRepairStatus::Succeeded => match record.verification {
            DoctorRepairVerification::Verified => ("Repair verified", widgets::StatusTone::Success),
            DoctorRepairVerification::FindingRemains => (
                "Repair completed but finding remains",
                widgets::StatusTone::Warning,
            ),
            DoctorRepairVerification::CouldNotComplete => (
                "Verification could not complete",
                widgets::StatusTone::Warning,
            ),
            DoctorRepairVerification::NotAttempted => {
                ("Repair completed", widgets::StatusTone::Info)
            }
        },
        DoctorRepairStatus::Failed => (
            "Repair failed and state was preserved",
            widgets::StatusTone::Blocked,
        ),
        DoctorRepairStatus::Rejected => {
            // A refusal because the issue disappeared before repair is not a
            // failure on the user's part. The exact reason still travels in
            // the record's summary, which is kept below and in History & Logs.
            if record.rejection == Some(DoctorRepairRejection::StaleFinding) {
                ("Nothing needed changing", widgets::StatusTone::Info)
            } else {
                (
                    "Repair was refused and nothing was changed",
                    widgets::StatusTone::Blocked,
                )
            }
        }
        DoctorRepairStatus::DryRun => ("Validated only", widgets::StatusTone::Info),
    };
    widgets::banner(ui, title, &record.summary, tone);
    widgets::card(ui, |ui| {
        ui.label(format!("Repair: {}", record.action_title));
        if let Some(affected) = &record.affected {
            ui.add(egui::Label::new(format!("Resource: {}", affected.display)).wrap());
        }
        if record.changed_paths.is_empty() {
            ui.label("Nothing was changed on disk.");
        } else {
            ui.label(egui::RichText::new("Changed on disk").strong());
            for path in &record.changed_paths {
                ui.add(egui::Label::new(format!("• {}", path.display)).wrap());
            }
        }
        ui.label(format!("Undo: {}", record.undo.label()));
        if let Some(error) = &record.error {
            ui.add(egui::Label::new(format!("Error: {error}")).wrap());
        }
        ui.weak("This attempt was recorded in History & Logs.");
    });
}

/// Shown in place of the disclosure when a finding measured nothing.
pub(crate) const DOCTOR_NO_MEASURED_VALUES: &str = "No measured values recorded";

/// The selected finding's evidence and provenance. Everything here is
/// observed fact or existing guidance prose - no invented advice, and no
/// control that could change anything.
fn show_doctor_finding_details(ui: &mut egui::Ui, finding: &Finding, key: &str) {
    ui.add_space(6.0);
    ui.separator();
    if let Some(why) = &finding.why_it_matters {
        ui.label(egui::RichText::new("Why it matters").strong());
        ui.add(egui::Label::new(why).wrap());
    }
    if let Some(next) = &finding.next_step {
        ui.label(egui::RichText::new("Recommended next step").strong());
        ui.add(egui::Label::new(next).wrap());
    } else if finding.offered_repair().is_none()
        && finding.recovery.is_none()
        && !matches!(
            finding.severity,
            DoctorSeverity::Info | DoctorSeverity::Healthy
        )
    {
        ui.label(egui::RichText::new("What to do next").strong());
        ui.label("EmuWiz cannot repair this automatically.");
    }
    if let Some(recovery) = &finding.recovery {
        // Informational only. Stage 1A deliberately renders no button here:
        // exposing these safely needs confirmation and post-repair
        // verification, which is Stage 1B's job.
        ui.add_space(4.0);
        ui.add(egui::Label::new(recovery.notice()).wrap());
    }
    widgets::technical_details(ui, format!("doctor-finding-{key}"), |ui| {
        if finding_explanation_is_technical(&finding.explanation) {
            ui.label(egui::RichText::new("Original finding").strong());
            ui.add(egui::Label::new(&finding.explanation).wrap());
        }
        if let Some(affected) = &finding.affected {
            ui.add(egui::Label::new(format!("Resource: {}", affected.display)).wrap());
            if affected.lossy {
                ui.weak(
                    "This path contains bytes that are not valid text, so it is shown approximately.",
                );
            }
        }
        if !finding.evidence.is_empty() {
            ui.label(egui::RichText::new("Evidence").strong());
            for item in &finding.evidence {
                ui.add(egui::Label::new(format!("• {item}")).wrap());
            }
        }
        if finding.measurements.is_empty() {
            ui.weak(DOCTOR_NO_MEASURED_VALUES);
        } else {
            ui.label(egui::RichText::new("Measured values").strong());
            for (name, value) in &finding.measurements {
                ui.label(format!("{name}: {value}"));
            }
        }
        ui.label(egui::RichText::new("Technical reference").strong());
        ui.label(format!("Check: {}", finding.subsystem.label()));
        ui.label(format!("Diagnostic ID: {}", finding.id));
    });
}

fn finding_explanation_is_technical(explanation: &str) -> bool {
    explanation.split_whitespace().any(|word| {
        word.trim_matches(|character: char| {
            matches!(character, '`' | '(' | ')' | '[' | ']' | ',' | ';')
        })
        .starts_with('/')
            || word.contains(":\\")
            || word.contains("file://")
    })
}

/// What this scan actually covered, what it could not, and what EmuWiz
/// does not check at all yet. Without this a clean result would read as
/// "everything is fine", which would be untrue.
fn show_doctor_coverage(ui: &mut egui::Ui, scan: &DoctorScan) {
    egui::CollapsingHeader::new("What was checked")
        .id_salt("doctor-coverage")
        .default_open(false)
        .show(ui, |ui| {
            let checked = scan.checked_subsystems();
            if checked.is_empty() {
                ui.label("Nothing could be checked in this run.");
            } else {
                for entry in checked {
                    ui.label(format!(
                        "Checked: {} ({})",
                        entry.category.label(),
                        entry.subsystem.label()
                    ));
                }
            }
            let unavailable = scan.unavailable_subsystems();
            if !unavailable.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Not checked in this run").strong());
                for entry in unavailable {
                    let reason = match &entry.status {
                        CoverageStatus::Unavailable { reason } => reason.as_str(),
                        CoverageStatus::Checked => "",
                    };
                    ui.add(
                        egui::Label::new(format!("{}: {reason}", entry.category.label())).wrap(),
                    );
                }
            }
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Not checked by EmuWiz yet").strong());
            ui.weak(
                "These are not covered by the result above, so a healthy result does not mean they are fine.",
            );
            for deferred in scan.deferred {
                ui.add(egui::Label::new(format!("{}: {}", deferred.name, deferred.reason)).wrap());
            }
        });
}

pub(crate) fn doctor_severity_tone(severity: DoctorSeverity) -> widgets::StatusTone {
    match severity {
        DoctorSeverity::Critical | DoctorSeverity::Error => widgets::StatusTone::Blocked,
        DoctorSeverity::Warning => widgets::StatusTone::Warning,
        DoctorSeverity::Info => widgets::StatusTone::Info,
        DoctorSeverity::Healthy => widgets::StatusTone::Success,
    }
}

/// The History detail line for one repair attempt.
///
/// Records everything the milestone requires that `HistoryEntry` itself has
/// no column for - action id, finding id, confirmation, verification, changed
/// paths, undo availability and any error - as one structured line, rather
/// than adding a database migration to store it.
pub(crate) fn doctor_repair_history_detail(outcome: &DoctorRepairOutcome) -> String {
    let record = &outcome.record;
    let mut parts = vec![
        format!("action={}", record.action_id),
        format!("finding={}", record.finding_id),
        format!("confirmed={}", record.confirmed),
        format!("dry_run={}", record.dry_run),
        format!("result={:?}", record.status),
        format!("verification={}", record.verification.label()),
        format!("undo={}", record.undo.label()),
    ];
    if let Some(affected) = &record.affected {
        parts.push(format!("resource={}", affected.display));
    }
    if !record.changed_paths.is_empty() {
        parts.push(format!(
            "changed=[{}]",
            record
                .changed_paths
                .iter()
                .map(|path| path.display.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(rejection) = &record.rejection {
        parts.push(format!("refused={rejection:?}"));
    }
    if let Some(error) = &record.error {
        parts.push(format!("error={error}"));
    }
    format!("{} | {}", record.summary, parts.join(" "))
}

/// The exact statement shown on the Doctor page, kept as one constant so the
/// GUI and its tests cannot drift.
pub(crate) const DOCTOR_READ_ONLY_NOTICE: &str = "This scan is read-only: it inspects configuration, existing files and existing records only. It never creates, mounts, unmounts, repairs, rebuilds or removes anything. Free space and write access are read from the filesystem itself - no test file is ever written, and your emulator profiles, cheats and patches are never modified.";

/// A one-line "what does Run Doctor actually check" summary, shown right
/// beside the read-only safety notice above - not only in the empty
/// "no scan has run yet" state (which disappears after the first run and
/// previously left this explanation nowhere to be found on later visits).
/// Deliberately one sentence: the safety notice already covers what it
/// never does; this covers what it does, without duplicating the full
/// per-check detail available in the results themselves.
pub(crate) const DOCTOR_WHAT_IT_CHECKS_NOTICE: &str = "Checks configuration, source folder availability, the mount destination, library and database health, and emulator or profile prerequisites where applicable.";

/// Plain-text form of a scan, for "Copy report".
pub(crate) fn doctor_scan_report_text(outcome: &DoctorScanOutcome) -> String {
    let scan = &outcome.scan;
    let mut lines = vec![
        "EmuWiz Doctor - read-only diagnostic scan".to_string(),
        format!(
            "Last run: {}",
            format_unix_timestamp_utc(outcome.finished_at_unix_seconds)
        ),
        scan.counts()
            .iter()
            .map(|(severity, count)| format!("{}: {count}", severity.label()))
            .collect::<Vec<_>>()
            .join("  "),
        String::new(),
    ];
    if scan.is_healthy() {
        lines.push("No problems detected by the available read-only checks.".to_string());
        lines.push(String::new());
    }
    for (category, findings) in scan.by_category() {
        lines.push(format!("{} ({})", category.label(), findings.len()));
        for finding in findings {
            lines.push(format!(
                "  [{}] {} - {}",
                finding.severity.label().to_lowercase(),
                finding.id,
                finding.title
            ));
            lines.push(format!("      {}", finding.explanation));
            if let Some(affected) = &finding.affected {
                lines.push(format!("      Resource: {}", affected.display));
            }
            if let Some(why) = &finding.why_it_matters {
                lines.push(format!("      Why it matters: {why}"));
            }
            if let Some(next) = &finding.next_step {
                lines.push(format!("      Next step: {next}"));
            }
            for item in &finding.evidence {
                lines.push(format!("      Evidence: {item}"));
            }
            for (key, value) in &finding.measurements {
                lines.push(format!("      Measured: {key} = {value}"));
            }
            lines.push(format!("      Reported by: {}", finding.subsystem.label()));
        }
        lines.push(String::new());
    }
    lines.push("Checked:".to_string());
    for entry in scan.checked_subsystems() {
        lines.push(format!(
            "  {} ({})",
            entry.category.label(),
            entry.subsystem.label()
        ));
    }
    for entry in scan.unavailable_subsystems() {
        let reason = match &entry.status {
            CoverageStatus::Unavailable { reason } => reason.as_str(),
            CoverageStatus::Checked => "",
        };
        lines.push(format!(
            "  Not checked: {} - {reason}",
            entry.category.label()
        ));
    }
    lines.push(String::new());
    lines.push("Not checked by EmuWiz yet:".to_string());
    for deferred in scan.deferred {
        lines.push(format!("  {} - {}", deferred.name, deferred.reason));
    }
    lines.join("\n")
}
