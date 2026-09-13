//! Bounded read-only presentation. Action buttons only navigate.
use crate::ui::components as widgets;
use crate::{ArchiveFsApp, LoadState, MainView, launch_readiness_page::LaunchReadinessInput};
use archivefs_core::attention::*;
use eframe::egui;
use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Projects the DAT authority dashboard into the current unified attention
/// snapshot. DAT remains a read-only producer; deduplication and filtering are
/// owned by `archivefs_core::attention::AttentionSnapshot`.
pub(crate) fn append_dat_attention(
    snapshot: &mut AttentionSnapshot,
    data: &archivefs_core::dat::authority::DatAuthorityDashboard,
) {
    use archivefs_core::dat::authority::{AuthorityFreshness, CompletenessState};

    for row in &data.collections {
        let authority = data
            .authorities
            .iter()
            .find(|a| Some(&a.source.id) == row.source_id.as_ref());
        let freshness_unknown =
            authority.is_some_and(|a| !matches!(a.freshness, AuthorityFreshness::Current));
        if row.state == CompletenessState::Complete && !freshness_unknown {
            continue;
        }
        let severity = if row.counts.bios_missing.is_some_and(|n| n > 0) {
            AttentionSeverity::Blocking
        } else if matches!(
            row.state,
            CompletenessState::Incomplete
                | CompletenessState::Ambiguous
                | CompletenessState::NoAuthority
        ) {
            AttentionSeverity::ActionNeeded
        } else {
            AttentionSeverity::Warning
        };
        let mut item = AttentionItem::new(
            format!(
                "dat-authority:{}:{}",
                row.platform,
                row.source_id.as_deref().unwrap_or("unassigned")
            ),
            AttentionCategory::Dat,
            severity,
            format!("{}: {}", row.platform, row.state.label()),
            AttentionDestination::DatReview,
        );
        item.summary = row.explanations.join(" ");
        if freshness_unknown {
            item.summary.push_str(
                " Publisher freshness is unknown or the authority changed; review source provenance in DAT Sources.",
            );
        }
        item.source_workflow = "DAT authority & completeness".into();
        item.source_records
            .push(row.source_id.clone().unwrap_or_else(|| "unassigned".into()));
        item.platform = Some(row.platform.clone());
        item.recommended_action =
            "Review authority, completeness and BIOS evidence in DAT Sources".into();
        item.recoverability =
            "Read-only evidence; use the existing DAT review and verification workflows.".into();
        item.provenance = "Recorded DAT inventory, catalogue identity and set-audit evidence; no filesystem scan or automatic fix.".into();
        item.affected_count = row.counts.local;
        snapshot.insert(item);
    }
    for authority in data
        .authorities
        .iter()
        .filter(|a| a.source.platform.is_none())
    {
        let mut item = AttentionItem::new(
            format!("dat-authority:unlinked:{}", authority.source.id),
            AttentionCategory::Dat,
            AttentionSeverity::Warning,
            format!("{}: DAT is not linked to a platform", authority.source.name),
            AttentionDestination::DatReview,
        );
        item.summary = authority.preparation.join(" ");
        item.source_workflow = "DAT authority & completeness".into();
        item.source_records.push(authority.source.id.clone());
        item.recommended_action = "Review platform assignment in DAT Sources".into();
        item.recoverability = "No changes made; authority assignment requires review.".into();
        item.provenance =
            "Recorded DAT source configuration and imported inventory metadata.".into();
        snapshot.insert(item);
    }
}

#[derive(Default)]
pub(crate) struct AttentionWorkspace {
    pub(crate) snapshot: AttentionSnapshot,
    stored: AttentionSnapshot,
    receiver: Option<Receiver<AttentionSnapshot>>,
    last_started: Option<Instant>,
    last_session_refresh: Option<Instant>,
    generation: Option<u64>,
    pub(crate) loaded: bool,
    pub(crate) filters: AttentionFilters,
}

impl AttentionWorkspace {
    pub(crate) fn invalidate(&mut self) {
        self.last_session_refresh = None;
    }
}

impl ArchiveFsApp {
    pub(crate) fn invalidate_needs_attention(&mut self) {
        self.needs_attention.invalidate();
    }

    /// Independent of the selected page. The page only renders memory.
    pub(crate) fn poll_needs_attention(&mut self, context: &egui::Context) {
        if let Some(receiver) = &self.needs_attention.receiver {
            match receiver.try_recv() {
                Ok(snapshot) => {
                    self.needs_attention.stored = snapshot;
                    self.needs_attention.loaded = true;
                    self.needs_attention.receiver = None;
                    self.needs_attention.last_session_refresh = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.needs_attention.stored = AttentionSnapshot::default();
                    self.needs_attention.stored.coverage_notes.push(
                        "Attention refresh failed; current catalogue coverage is unavailable."
                            .into(),
                    );
                    self.needs_attention.loaded = true;
                    self.needs_attention.receiver = None;
                    self.needs_attention.last_session_refresh = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let due = self
            .needs_attention
            .last_started
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(30));
        let changed = self.needs_attention.generation != Some(self.database_generation.0);
        if self.needs_attention.receiver.is_none() && (due || changed) {
            self.needs_attention.generation = Some(self.database_generation.0);
            self.needs_attention.last_started = Some(Instant::now());
            let (sender, receiver) = mpsc::channel();
            self.needs_attention.receiver = Some(receiver);
            let context = context.clone();
            std::thread::spawn(move || {
                let mut snapshot = match archivefs_core::default_database_path()
                    .and_then(archivefs_core::Database::open_read_only)
                    .and_then(|db| db.attention_snapshot())
                {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        let mut snapshot = AttentionSnapshot::default();
                        snapshot
                            .coverage_notes
                            .push(format!("Catalogue evidence unavailable: {error}"));
                        snapshot
                    }
                };
                snapshot.merge(archivefs_core::operation::attention_receipt_snapshot(
                    &archivefs_core::operation::AttentionReceiptPaths {
                        rename: archivefs_core::dat::rename_apply::default_rename_transaction_dir()
                            .ok(),
                        library_views: archivefs_core::default_library_view_history_dir().ok(),
                        shared_apply: archivefs_core::patch_manager::default_shared_history_root()
                            .ok(),
                        database: archivefs_core::default_database_path().ok(),
                    },
                ));
                let _ = sender.send(snapshot);
                context.request_repaint();
            });
        }
        if self
            .needs_attention
            .last_session_refresh
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(1))
        {
            let mut snapshot = self.needs_attention.stored.clone();
            if let Some(outcome) = self.doctor_scan.displayed() {
                snapshot.merge(doctor_attention(
                    &outcome.scan,
                    outcome.finished_at_unix_seconds,
                ));
            } else {
                snapshot.coverage_notes.push("Emulator, BIOS, tools and managed-entry diagnostics have not been checked. Open Emulator Setup or Problems & Repair / Diagnostics.".into());
            }
            let live = match &self.state {
                LoadState::Ready(data) => Some(data.as_ref()),
                _ => None,
            };
            snapshot.merge(launch_attention(
                &self.build_launch_readiness_input(live),
                self.archive_context.focused.as_deref(),
            ));
            if let Some(page) = &self.rom_organisation_page {
                snapshot.merge(page.playing_library.attention_snapshot());
            }
            if let Some(page) = &self.exact_duplicate_review_page
                && page.attention_group_count() > 0
            {
                let mut item = AttentionItem::new(
                    format!("exact-duplicate-review:{}", page.source_root_draft),
                    AttentionCategory::Duplicates,
                    AttentionSeverity::ActionNeeded,
                    format!(
                        "{} verified duplicate groups are waiting for review",
                        page.attention_group_count()
                    ),
                    AttentionDestination::ExactDuplicates,
                );
                item.summary = "The existing exact-duplicate scan proved matching file content. Its current review list excludes groups already quarantined; freshness is checked again before any action.".into();
                item.source_workflow = "Exact duplicate review".into();
                item.affected = Some(page.source_root_draft.clone());
                item.affected_count = page.attention_group_count() as u64;
                item.provenance =
                    "Current ExactDuplicateScanReport, not catalogue name similarity".into();
                snapshot.insert(item);
            }
            if let Some(page) = &self.repair_review_page
                && !page.plan_stale
                && let Some(plan) = &page.plan
                && plan.has_safe_repairs()
            {
                let mut item = AttentionItem::new(
                    format!("repair-review:{}", plan.scan_root),
                    AttentionCategory::Repair,
                    AttentionSeverity::ActionNeeded,
                    format!(
                        "{} proposed repairs are waiting for review",
                        plan.safe_repair_count()
                    ),
                    AttentionDestination::Problems,
                );
                item.summary = "The loaded repair plan contains executable proposals. Review and freshness checks remain in Problems & Repair; nothing is applied here.".into();
                item.source_workflow = "Repair review".into();
                item.source_records
                    .push(format!("{}:{}", plan.source_id, plan.generation));
                item.affected = Some(plan.scan_root.clone());
                item.affected_count = plan.safe_repair_count() as u64;
                item.last_observed = Some(plan.created_at_unix as i64);
                item.provenance = "Current non-stale LibraryRepairPlan".into();
                snapshot.insert(item);
            }
            if let Some(Ok(data)) = &self.dat_authority.data {
                append_dat_attention(&mut snapshot, data);
                snapshot.source_rows += (data.collections.len() + data.authorities.len()) as u64;
            }
            self.needs_attention.snapshot = snapshot;
            self.needs_attention.last_session_refresh = Some(Instant::now());
        }
        context.request_repaint_after(Duration::from_secs(1));
    }

    pub(crate) fn navigate_attention(&mut self, destination: AttentionDestination) {
        if matches!(
            destination,
            AttentionDestination::Romm | AttentionDestination::EsDe
        ) {
            let page = self
                .rom_organisation_page
                .get_or_insert_with(crate::rom_organisation_page::RomOrganisationPageState::load);
            page.showing_playing_library = true;
            // Navigation preserves the preview/error under review. The normal
            // destination-switch action clears those, so do not call it here.
            page.playing_library.destination = if destination == AttentionDestination::Romm {
                crate::playing_library_page::PlayingLibraryDestination::Romm
            } else {
                crate::playing_library_page::PlayingLibraryDestination::EsDe
            };
        }
        self.navigate_to_main_view(destination_view(destination));
    }
}

fn launch_attention(
    input: &LaunchReadinessInput,
    selected: Option<&std::path::Path>,
) -> AttentionSnapshot {
    use archivefs_core::launch::{FirmwareReadiness, LaunchBlockerKind};
    let mut snapshot = AttentionSnapshot::default();
    let Some(selected) = selected else {
        snapshot.coverage_notes.push(
            "Launch readiness is known only for games checked in the existing launch workflow."
                .into(),
        );
        return snapshot;
    };
    let (title, summary, category, destination, platform) = match input {
        LaunchReadinessInput::EvidenceNotLoaded | LaunchReadinessInput::RetroArchNotScanned => {
            snapshot.coverage_notes.push("Selected-game launch evidence is not loaded; no launch failure has been inferred.".into());
            return snapshot;
        },
        LaunchReadinessInput::IdentityUnknown | LaunchReadinessInput::IdentityConflicting => (
            "Launch needs verified identity".into(), "The selected game's identity is unresolved or conflicts. Review the existing identity evidence.".into(), AttentionCategory::Launch, AttentionDestination::LaunchReadiness, None),
        LaunchReadinessInput::Plan { plan, retroarch_scanned, standalone_scans_complete, .. } => {
            if plan.summary.ready + plan.summary.ready_with_warnings > 0 { return snapshot; }
            if !retroarch_scanned && !standalone_scans_complete {
                snapshot.coverage_notes.push("Emulator discovery for the selected game has not completed.".into());
                return snapshot;
            }
            if plan.candidates.is_empty() && (!retroarch_scanned || !standalone_scans_complete) {
                snapshot.coverage_notes.push("Some emulator discovery lanes remain unchecked; no missing-emulator failure has been inferred.".into());
                return snapshot;
            }
            let missing_bios = plan.candidates.iter().any(|candidate| candidate.firmware == FirmwareReadiness::Missing || candidate.blockers.iter().any(|blocker| blocker.kind == LaunchBlockerKind::RequiredFirmwareMissing));
            let title = if missing_bios { format!("Required BIOS is missing for {}", plan.platform_id.as_deref().unwrap_or("the selected game")) } else { "The selected game has no ready launch option".into() };
            let summary = plan.candidates.iter().flat_map(|candidate| &candidate.blockers).take(4).map(|blocker| blocker.detail.as_str()).collect::<Vec<_>>().join(" · ");
            (title, if summary.is_empty() { "No compatible emulator is ready in the completed discovery checks.".into() } else { summary }, if missing_bios { AttentionCategory::Emulator } else { AttentionCategory::Launch }, if missing_bios { AttentionDestination::EmulatorSetup } else { AttentionDestination::LaunchReadiness }, plan.platform_id.clone())
        },
    };
    let mut item = AttentionItem::new(
        format!("launch:{selected:?}"),
        category,
        AttentionSeverity::Blocking,
        title,
        destination,
    );
    item.summary = summary;
    item.platform = platform;
    item.affected = Some(selected.display().to_string());
    item.source_workflow = "Launch readiness".into();
    item.provenance =
        "Existing selected-game planner and completed discovery evidence; no new probe".into();
    snapshot.insert(item);
    snapshot
}

pub(crate) fn destination_view(destination: AttentionDestination) -> MainView {
    match destination {
        AttentionDestination::Sources => MainView::Sources,
        AttentionDestination::Discovery => MainView::SourcesDiscovery,
        AttentionDestination::DatReview => MainView::IdentifyRename,
        AttentionDestination::Duplicates => MainView::Duplicates,
        AttentionDestination::Problems => MainView::Problems,
        AttentionDestination::EmulatorSetup => MainView::EmulatorSetup,
        AttentionDestination::LaunchReadiness => MainView::Selected,
        AttentionDestination::History => MainView::HistoryLogs,
        AttentionDestination::Romm | AttentionDestination::EsDe => MainView::CanonicalOrganisation,
        AttentionDestination::LibraryOrganisation => MainView::LibraryViews,
        AttentionDestination::CheatsMods => MainView::CheatsMods,
        AttentionDestination::DiscConversion => MainView::DiscConversion,
        AttentionDestination::ExactDuplicates => MainView::ExactDuplicateReview,
    }
}

pub(crate) fn show_needs_attention_page(
    ui: &mut egui::Ui,
    workspace: &mut AttentionWorkspace,
) -> Option<AttentionDestination> {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::CHECK,
        "Needs Attention",
        "Current saved evidence, with actions in the workflows that own it.",
    );
    if !workspace.loaded {
        ui.spinner();
        ui.label("Loading saved attention evidence in the background…");
    }
    let counts = workspace.snapshot.counts();
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("{} unresolved", counts.iter().sum::<usize>()));
        for (severity, count) in AttentionSeverity::ALL.into_iter().zip(counts) {
            ui.label(format!("{count} {}", severity.label().to_lowercase()));
        }
    });
    if workspace.snapshot.limited {
        ui.colored_label(egui::Color32::YELLOW, "Partial coverage: summary/receipt limits were reached. Counts are a lower bound; review complete details in the source workflows.");
    }
    let before = workspace.filters.clone();
    let filters = &mut workspace.filters;
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(&mut filters.search).hint_text("Search attention items"));
        egui::ComboBox::from_id_salt("attention-severity")
            .selected_text(
                filters
                    .severity
                    .map_or("All severities", AttentionSeverity::label),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filters.severity, None, "All severities");
                for severity in AttentionSeverity::ALL {
                    ui.selectable_value(&mut filters.severity, Some(severity), severity.label());
                }
            });
        egui::ComboBox::from_id_salt("attention-category")
            .selected_text(
                filters
                    .category
                    .map_or("All categories", AttentionCategory::label),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filters.category, None, "All categories");
                for category in AttentionCategory::ALL {
                    ui.selectable_value(&mut filters.category, Some(category), category.label());
                }
            });
    });
    ui.horizontal_wrapped(|ui| {
        let platforms: BTreeSet<_> = workspace
            .snapshot
            .items()
            .filter_map(|i| i.platform.as_ref())
            .collect();
        let workflows: BTreeSet<_> = workspace
            .snapshot
            .items()
            .map(|i| &i.source_workflow)
            .collect();
        egui::ComboBox::from_id_salt("attention-platform")
            .selected_text(filters.platform.as_deref().unwrap_or("All platforms"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filters.platform, None, "All platforms");
                for platform in platforms {
                    ui.selectable_value(&mut filters.platform, Some(platform.clone()), platform);
                }
            });
        egui::ComboBox::from_id_salt("attention-workflow")
            .selected_text(filters.workflow.as_deref().unwrap_or("All workflows"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filters.workflow, None, "All workflows");
                for workflow in workflows {
                    ui.selectable_value(&mut filters.workflow, Some(workflow.clone()), workflow);
                }
            });
        egui::ComboBox::from_id_salt("attention-state")
            .selected_text(match filters.state {
                AttentionStateFilter::Unresolved => "Unresolved",
                AttentionStateFilter::Resolved => "Resolved history",
                AttentionStateFilter::All => "All states",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut filters.state,
                    AttentionStateFilter::Unresolved,
                    "Unresolved",
                );
                ui.selectable_value(
                    &mut filters.state,
                    AttentionStateFilter::Resolved,
                    "Resolved history",
                );
                ui.selectable_value(&mut filters.state, AttentionStateFilter::All, "All states");
            });
        ui.checkbox(&mut filters.newest_first, "Newest first");
    });
    if *filters != before {
        filters.page = 0;
    }
    let page = workspace.snapshot.page(filters);
    filters.page = page.page;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(page.page > 0, egui::Button::new("Previous"))
            .clicked()
        {
            filters.page -= 1;
        }
        ui.label(format!(
            "Page {} · {} matching summaries · up to {ATTENTION_PAGE_SIZE} per page",
            page.page + 1,
            page.total
        ));
        if ui
            .add_enabled(
                (page.page + 1) * ATTENTION_PAGE_SIZE < page.total,
                egui::Button::new("Next"),
            )
            .clicked()
        {
            filters.page += 1;
        }
    });
    if page.total == 0 && workspace.loaded {
        ui.label(if counts.iter().sum::<usize>() == 0 {
            "No unresolved issues need your attention in the checked sources."
        } else {
            "No items match these filters."
        });
    }
    egui::CollapsingHeader::new("Coverage and freshness").default_open(page.total == 0 || workspace.snapshot.limited).show(ui, |ui| {
        ui.label("Saved state refreshes automatically (at most 30 seconds). Diagnostics and launch findings update after their existing checks complete; unchecked sources are not assumed healthy.");
        ui.label(format!("{} source records; {} catalogue queries; {} ms query time. Opening this page performs no mutations, library walk or network calls.", workspace.snapshot.source_rows, workspace.snapshot.query_count, workspace.snapshot.query_millis));
        for note in &workspace.snapshot.coverage_notes { ui.label(note); }
    });
    let mut destination = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for item in page.items {
            ui.push_id(&item.id, |ui| widgets::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(ui, item.severity.label(), match item.severity { AttentionSeverity::Blocking => widgets::StatusTone::Blocked, AttentionSeverity::ActionNeeded | AttentionSeverity::Warning => widgets::StatusTone::Warning, AttentionSeverity::Info => widgets::StatusTone::Info });
                    ui.strong(&item.title);
                });
                ui.label(&item.summary);
                ui.label(&item.recommended_action);
                if ui.button(format!("Open {}", item.destination.label())).clicked() { destination = Some(item.destination); }
                egui::CollapsingHeader::new("Evidence and recovery details").show(ui, |ui| {
                    if let Some(path) = &item.affected { ui.label(path); }
                    ui.label(format!("Workflow: {} · {} affected", item.source_workflow, item.affected_count));
                    ui.label(&item.provenance);
                    ui.label(&item.recoverability);
                    ui.label(format!("First detected: {:?}; last observed: {:?} (Unix seconds; unknown is not inferred)", item.first_detected, item.last_observed));
                    for reference in &item.source_records { ui.label(reference); }
                });
            }));
        }
    });
    destination
}

#[cfg(test)]
mod tests;
