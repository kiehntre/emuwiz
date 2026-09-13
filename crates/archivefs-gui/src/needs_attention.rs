//! Read-only projection of existing problem and recovery state.
//!
//! This module deliberately owns no persistence and no resolution flags. An
//! item is present only while its source-of-truth signal is present.

use std::collections::HashSet;
use std::path::PathBuf;

use archivefs_core::operation::{
    OperationKind, OperationRecord, OperationState, RecoveryClassification,
};
use archivefs_core::{
    CatalogueDuplicateReport, HealthCategory, HealthIssue, SetupDiagnosticStatus, SetupDiagnostics,
    SourceAvailability, SourceHealthIssue,
};
use eframe::egui;

use crate::ui::components as widgets;

use super::MainView;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AttentionSeverity {
    Blocking,
    ActionNeeded,
    Warning,
    Info,
}

impl AttentionSeverity {
    fn label(self) -> &'static str {
        match self {
            Self::Blocking => "Blocking",
            Self::ActionNeeded => "Action needed",
            Self::Warning => "Warning",
            Self::Info => "Info",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Blocking => 0,
            Self::ActionNeeded => 1,
            Self::Warning => 2,
            Self::Info => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AttentionCategory {
    Sources,
    Identity,
    Duplicates,
    Repair,
    Emulator,
    Recovery,
    Unsupported,
    Operations,
}

impl AttentionCategory {
    const ALL: [Self; 8] = [
        Self::Sources,
        Self::Identity,
        Self::Duplicates,
        Self::Repair,
        Self::Emulator,
        Self::Recovery,
        Self::Unsupported,
        Self::Operations,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Sources => "Sources",
            Self::Identity => "Identity",
            Self::Duplicates => "Duplicates",
            Self::Repair => "Repair",
            Self::Emulator => "Emulator readiness",
            Self::Recovery => "Database recovery",
            Self::Unsupported => "Unsupported files",
            Self::Operations => "Operations",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AttentionItem {
    pub(super) id: String,
    pub(super) category: AttentionCategory,
    pub(super) severity: AttentionSeverity,
    pub(super) title: String,
    pub(super) summary: String,
    pub(super) affected: Option<PathBuf>,
    pub(super) source_workflow: String,
    pub(super) detected_at: Option<String>,
    pub(super) recommended_action: String,
    pub(super) destination: MainView,
    pub(super) recoverability: String,
    pub(super) resolved: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AttentionFilters {
    pub(super) search: String,
    pub(super) severity: Option<AttentionSeverity>,
    pub(super) category: Option<AttentionCategory>,
    pub(super) show_resolved: bool,
}

fn health_item(issue: &HealthIssue) -> AttentionItem {
    let (severity, category, destination, action) = match issue.category {
        HealthCategory::UnknownPlatform => (
            AttentionSeverity::ActionNeeded,
            AttentionCategory::Identity,
            MainView::DatSources,
            "Review platform identity in Sources → DAT Sources".to_string(),
        ),
        HealthCategory::TerminalFailure => (
            AttentionSeverity::Blocking,
            if issue.reason.to_lowercase().contains("unsupported") {
                AttentionCategory::Unsupported
            } else {
                AttentionCategory::Repair
            },
            MainView::Problems,
            "Open Problems & Repair to inspect the failure".to_string(),
        ),
        HealthCategory::RetryableFailure | HealthCategory::RecoveryAvailable => (
            AttentionSeverity::ActionNeeded,
            AttentionCategory::Repair,
            MainView::Problems,
            "Open Problems & Repair to review the available recovery".to_string(),
        ),
        HealthCategory::Missing | HealthCategory::CachedOnly => (
            AttentionSeverity::ActionNeeded,
            AttentionCategory::Sources,
            MainView::Sources,
            "Check the source folder and rescan when it is available".to_string(),
        ),
        HealthCategory::AwaitingValidation
        | HealthCategory::HistoricalMountFailure
        | HealthCategory::MountFailureEvidenceInsufficient => (
            AttentionSeverity::Warning,
            AttentionCategory::Repair,
            MainView::Problems,
            "Review the stored health evidence in Problems & Repair".to_string(),
        ),
        HealthCategory::MountNotRequired => (
            AttentionSeverity::Info,
            AttentionCategory::Operations,
            MainView::Library,
            "No mount action is required for this item".to_string(),
        ),
    };
    AttentionItem {
        id: format!("health:{}", issue.path.display()),
        category,
        severity,
        title: issue.category.label().to_string(),
        summary: issue.reason.clone(),
        affected: Some(issue.path.clone()),
        source_workflow: "Library health".to_string(),
        detected_at: issue.last_seen_at.clone(),
        recommended_action: action,
        destination,
        recoverability: if issue.recovery_available() {
            "Existing recovery action available".to_string()
        } else {
            "Review required".to_string()
        },
        resolved: false,
    }
}

fn source_item(issue: &SourceHealthIssue) -> AttentionItem {
    let (severity, summary) = match issue.availability {
        SourceAvailability::PermissionDenied | SourceAvailability::ScanFailed => {
            (AttentionSeverity::Blocking, issue.reason.clone())
        }
        SourceAvailability::Unavailable => (AttentionSeverity::ActionNeeded, issue.reason.clone()),
        SourceAvailability::Available | SourceAvailability::Disabled => {
            (AttentionSeverity::Info, issue.reason.clone())
        }
    };
    AttentionItem {
        id: format!("source:{}", issue.path.display()),
        category: AttentionCategory::Sources,
        severity,
        title: "Source folder needs attention".to_string(),
        summary: format!(
            "{summary} {} catalogue entrie(s) are preserved.",
            issue.archives_preserved
        ),
        affected: Some(issue.path.clone()),
        source_workflow: "Source folders".to_string(),
        detected_at: None,
        recommended_action: "Open Sources to inspect this folder".to_string(),
        destination: MainView::Sources,
        recoverability: "Catalogue entries remain recoverable".to_string(),
        resolved: false,
    }
}

fn operation_item(record: &OperationRecord) -> Option<AttentionItem> {
    let resolved = matches!(
        record.state,
        OperationState::Completed | OperationState::RolledBack
    );
    let (severity, category, destination) = match record.kind {
        OperationKind::DatabaseRecovery => (
            AttentionSeverity::Blocking,
            AttentionCategory::Recovery,
            MainView::HistoryLogs,
        ),
        OperationKind::DuplicateQuarantine | OperationKind::RepairApply => (
            AttentionSeverity::ActionNeeded,
            AttentionCategory::Repair,
            MainView::Problems,
        ),
        _ => (
            AttentionSeverity::ActionNeeded,
            AttentionCategory::Operations,
            MainView::HistoryLogs,
        ),
    };
    let recovery = match record.recovery.classification {
        RecoveryClassification::SafeToRollback => "Rollback is available after review",
        RecoveryClassification::RequiresReview
        | RecoveryClassification::Stale
        | RecoveryClassification::Unrecoverable => "Review required before continuing",
        RecoveryClassification::SafeToResume => "This operation has a reviewed continuation",
    };
    Some(AttentionItem {
        id: format!("operation:{}", record.operation_id),
        category,
        severity,
        title: format!("{} operation needs attention", record.kind.label()),
        summary: record.output.summary.clone(),
        affected: record.destination.as_ref().map(PathBuf::from),
        source_workflow: record.kind.label().to_string(),
        detected_at: Some(record.created_at_unix.to_string()),
        recommended_action: "Open History & Logs to inspect the receipt".to_string(),
        destination,
        recoverability: recovery.to_string(),
        resolved,
    })
}

/// Builds the unified list only from already-loaded state and durable operation
/// receipts. It never scans collection roots, probes emulators, or writes.
pub(super) fn build_attention_items(
    health: &[HealthIssue],
    sources: &[SourceHealthIssue],
    operations: &[OperationRecord],
    duplicates: Option<&CatalogueDuplicateReport>,
    diagnostics: Option<&SetupDiagnostics>,
) -> Vec<AttentionItem> {
    let mut items = health.iter().map(health_item).collect::<Vec<_>>();
    items.extend(sources.iter().map(source_item));
    let mut operation_ids = HashSet::new();
    items.extend(operations.iter().filter_map(|record| {
        operation_ids
            .insert(record.operation_id.clone())
            .then(|| operation_item(record))
            .flatten()
    }));
    if let Some(report) = duplicates {
        for group in &report.groups {
            items.push(AttentionItem {
                id: format!("duplicate:{}:{}", group.platform, group.normalized_title),
                category: AttentionCategory::Duplicates,
                severity: AttentionSeverity::ActionNeeded,
                title: "Duplicate files are waiting for review".to_string(),
                summary: format!(
                    "{} files share the normalized name {} on {}.",
                    group.entries.len(),
                    group.title,
                    group.platform
                ),
                affected: group.entries.first().map(|entry| entry.path.clone()),
                source_workflow: "Duplicate Finder".to_string(),
                detected_at: None,
                recommended_action: "Open Duplicate Finder to review the group".to_string(),
                destination: MainView::ExactDuplicateReview,
                recoverability: "Quarantine uses the existing reversible workflow".to_string(),
                resolved: false,
            });
        }
    }
    if let Some(report) = diagnostics {
        for check in &report.checks {
            let (severity, category, destination) = match check.status {
                SetupDiagnosticStatus::Error => (
                    AttentionSeverity::Blocking,
                    AttentionCategory::Emulator,
                    MainView::EmulatorSetup,
                ),
                SetupDiagnosticStatus::Warning => (
                    AttentionSeverity::Warning,
                    AttentionCategory::Emulator,
                    MainView::EmulatorSetup,
                ),
                SetupDiagnosticStatus::Ready
                | SetupDiagnosticStatus::NotChecked
                | SetupDiagnosticStatus::NotConfigured => continue,
            };
            items.push(AttentionItem {
                id: format!("diagnostic:{}", check.name),
                category,
                severity,
                title: check.name.clone(),
                summary: check.detail.clone(),
                affected: report.mount_root.clone(),
                source_workflow: "Setup diagnostics".to_string(),
                detected_at: None,
                recommended_action: check.next_step.clone(),
                destination,
                recoverability: check.why_it_matters.clone(),
                resolved: false,
            });
        }
    }
    items.sort_by(|left, right| {
        left.severity
            .rank()
            .cmp(&right.severity.rank())
            .then_with(|| left.category.label().cmp(right.category.label()))
            .then_with(|| left.id.cmp(&right.id))
    });
    items
}

fn filtered_indices(items: &[AttentionItem], filters: &AttentionFilters) -> Vec<usize> {
    let needle = filters.search.trim().to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            filters.severity.is_none_or(|value| value == item.severity)
                && filters.category.is_none_or(|value| value == item.category)
                && (filters.show_resolved || !item.resolved)
                && (needle.is_empty()
                    || item.title.to_lowercase().contains(&needle)
                    || item.summary.to_lowercase().contains(&needle)
                    || item.affected.as_ref().is_some_and(|path| {
                        path.display().to_string().to_lowercase().contains(&needle)
                    }))
        })
        .map(|(index, _)| index)
        .collect()
}

pub(super) fn show_needs_attention_page(
    ui: &mut egui::Ui,
    items: &[AttentionItem],
    filters: &mut AttentionFilters,
) -> Option<MainView> {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::CHECK,
        "Needs Attention",
        "One read-only workspace for unresolved identity, setup, source, duplicate, and recovery signals.",
    );
    let blocking = items
        .iter()
        .filter(|item| item.severity == AttentionSeverity::Blocking)
        .count();
    let action_needed = items
        .iter()
        .filter(|item| item.severity == AttentionSeverity::ActionNeeded)
        .count();
    let warnings = items
        .iter()
        .filter(|item| item.severity == AttentionSeverity::Warning)
        .count();
    ui.horizontal_wrapped(|ui| {
        widgets::status_badge(
            ui,
            format!("{} total", items.len()),
            widgets::StatusTone::Info,
        );
        widgets::status_badge(
            ui,
            format!("{blocking} blocking"),
            widgets::StatusTone::Blocked,
        );
        widgets::status_badge(
            ui,
            format!("{action_needed} action needed"),
            widgets::StatusTone::Warning,
        );
        widgets::status_badge(
            ui,
            format!("{warnings} warnings"),
            widgets::StatusTone::Warning,
        );
    });
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("Filter:");
        ui.add(egui::TextEdit::singleline(&mut filters.search).hint_text("Search issues"));
        egui::ComboBox::from_id_salt("needs_attention_severity")
            .selected_text(
                filters
                    .severity
                    .map_or("All severities", AttentionSeverity::label),
            )
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(filters.severity.is_none(), "All severities")
                    .clicked()
                {
                    filters.severity = None;
                }
                for severity in [
                    AttentionSeverity::Blocking,
                    AttentionSeverity::ActionNeeded,
                    AttentionSeverity::Warning,
                    AttentionSeverity::Info,
                ] {
                    if ui
                        .selectable_label(filters.severity == Some(severity), severity.label())
                        .clicked()
                    {
                        filters.severity = Some(severity);
                    }
                }
            });
        egui::ComboBox::from_id_salt("needs_attention_category")
            .selected_text(
                filters
                    .category
                    .map_or("All workflows", AttentionCategory::label),
            )
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(filters.category.is_none(), "All workflows")
                    .clicked()
                {
                    filters.category = None;
                }
                for category in AttentionCategory::ALL {
                    if ui
                        .selectable_label(filters.category == Some(category), category.label())
                        .clicked()
                    {
                        filters.category = Some(category);
                    }
                }
            });
        ui.checkbox(&mut filters.show_resolved, "Show resolved");
    });
    let indices = filtered_indices(items, filters);
    if indices.is_empty() {
        widgets::empty_state(
            ui,
            "Nothing needs attention",
            "All indexed problem sources are clear, or no catalogue/diagnostic snapshot is ready yet.",
            None,
        );
        return None;
    }
    ui.label(format!("Showing {} unresolved indexed item(s). Opening this page does not scan or change anything.", indices.len()));
    let mut destination = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for index in indices.into_iter().take(200) {
                let item = &items[index];
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        widgets::status_badge(
                            ui,
                            item.severity.label(),
                            match item.severity {
                                AttentionSeverity::Blocking => widgets::StatusTone::Blocked,
                                AttentionSeverity::ActionNeeded | AttentionSeverity::Warning => {
                                    widgets::StatusTone::Warning
                                }
                                AttentionSeverity::Info => widgets::StatusTone::Info,
                            },
                        );
                        widgets::status_badge(ui, item.category.label(), widgets::StatusTone::Info);
                        ui.strong(&item.title);
                    });
                    ui.label(&item.summary);
                    if let Some(path) = &item.affected {
                        widgets::path_value(ui, "Affected", path);
                    }
                    ui.label(format!(
                        "Source: {} · {}",
                        item.source_workflow, item.recoverability
                    ));
                    ui.label(format!("Next: {}", item.recommended_action));
                    if ui
                        .button(format!("Open {}", item.destination_title()))
                        .clicked()
                    {
                        destination = Some(item.destination);
                    }
                });
                ui.add_space(5.0);
            }
        });
    destination
}

trait DestinationTitle {
    fn destination_title(&self) -> &'static str;
}
impl DestinationTitle for AttentionItem {
    fn destination_title(&self) -> &'static str {
        match self.destination {
            MainView::Problems => "Problems & Repair",
            MainView::DatSources => "DAT Sources",
            MainView::Sources => "Sources",
            MainView::ExactDuplicateReview => "Duplicate Finder",
            MainView::EmulatorSetup => "Emulator Setup",
            MainView::HistoryLogs => "History & Logs",
            MainView::Library => "Library",
            _ => "workflow",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn issue(category: HealthCategory, name: &str) -> HealthIssue {
        HealthIssue {
            path: PathBuf::from(format!("/library/{name}.zip")),
            platform: None,
            present: true,
            mount_state: None,
            category,
            reason: "test reason".into(),
            retryable: false,
            recovery_action: None,
            last_seen_at: None,
            size_bytes: None,
            modified_time_unix_seconds: None,
        }
    }

    #[test]
    fn health_items_are_routed_and_stably_identified() {
        let items = build_attention_items(
            &[issue(HealthCategory::UnknownPlatform, "game")],
            &[],
            &[],
            None,
            None,
        );
        assert_eq!(items[0].id, "health:/library/game.zip");
        assert_eq!(items[0].destination, MainView::DatSources);
        assert_eq!(items[0].severity, AttentionSeverity::ActionNeeded);
    }

    #[test]
    fn duplicate_operation_ids_are_deduplicated() {
        let record = OperationRecord {
            schema_version: 1,
            operation_id: "same".into(),
            kind: OperationKind::RepairApply,
            state: OperationState::Failed,
            created_at_unix: 1,
            started_at_unix: Some(1),
            completed_at_unix: None,
            input: Default::default(),
            destination: None,
            output: Default::default(),
            recovery: archivefs_core::operation::OperationRecoveryStatus {
                classification: RecoveryClassification::RequiresReview,
                explanation: "review".into(),
                actions: Default::default(),
            },
            error: Some("failed".into()),
        };
        let items = build_attention_items(&[], &[], &[record.clone(), record], None, None);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn resolved_operations_disappear_without_a_manual_flag() {
        let record = OperationRecord {
            schema_version: 1,
            operation_id: "done".into(),
            kind: OperationKind::RepairApply,
            state: OperationState::Completed,
            created_at_unix: 1,
            started_at_unix: Some(1),
            completed_at_unix: Some(2),
            input: Default::default(),
            destination: None,
            output: Default::default(),
            recovery: archivefs_core::operation::OperationRecoveryStatus {
                classification: RecoveryClassification::Unrecoverable,
                explanation: "done".into(),
                actions: Default::default(),
            },
            error: None,
        };
        let items = build_attention_items(&[], &[], &[record], None, None);
        assert!(filtered_indices(&items, &AttentionFilters::default()).is_empty());
    }

    #[test]
    fn filters_match_search_and_category() {
        let items = vec![issue(HealthCategory::UnknownPlatform, "game")];
        let projected = build_attention_items(&items, &[], &[], None, None);
        let mut filters = AttentionFilters {
            search: "game".into(),
            category: Some(AttentionCategory::Identity),
            ..Default::default()
        };
        assert_eq!(filtered_indices(&projected, &filters), vec![0]);
        filters.category = Some(AttentionCategory::Duplicates);
        assert!(filtered_indices(&projected, &filters).is_empty());
    }
}
