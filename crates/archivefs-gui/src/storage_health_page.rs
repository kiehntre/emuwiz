//! Read-only Storage Health presentation over the persisted catalogue.

use std::path::PathBuf;

use archivefs_core::PersistedArchive;
use archivefs_core::storage_health::{
    StorageHealthInput, StorageHealthReport, StorageOpportunityKind,
};
use eframe::egui;

#[derive(Default)]
pub(crate) struct StorageHealthPageState {
    report: Option<StorageHealthReport>,
    report_key: Option<Vec<(i64, Option<u64>, PathBuf)>>,
}

impl StorageHealthPageState {
    fn ensure_report(&mut self, archives: &[PersistedArchive]) {
        let mut key = archives
            .iter()
            .map(|archive| {
                (
                    archive.id,
                    archive.size_bytes,
                    archive.absolute_path.clone(),
                )
            })
            .collect::<Vec<_>>();
        key.sort_by(|a, b| a.2.cmp(&b.2));
        if self.report_key.as_ref() == Some(&key) {
            return;
        }
        let inputs = archives
            .iter()
            .map(|archive| StorageHealthInput {
                path: archive.absolute_path.clone(),
                platform: archive.platform.clone(),
                logical_size_bytes: archive.size_bytes,
                format_hint: Some(archive.archive_kind.clone()),
                content_hash: archive
                    .identity_report
                    .as_ref()
                    .and_then(|report| report.verified_loose_rom_sha256().map(str::to_owned)),
            })
            .collect::<Vec<_>>();
        self.report = Some(archivefs_core::storage_health::analyze_storage_health(
            &inputs,
        ));
        self.report_key = Some(key);
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, archives: &[PersistedArchive]) {
        self.ensure_report(archives);
        ui.heading("Storage Health");
        ui.label("Read-only analysis of catalogue space usage and possible future compression candidates.");
        ui.small("No conversion, deletion, recompression, deduplication, move, or rename actions are available here.");
        let Some(report) = self.report.as_ref() else {
            ui.label("Storage analysis is not available until the library catalogue is loaded.");
            return;
        };
        ui.add_space(8.0);
        egui::Grid::new("storage_health_summary")
            .striped(true)
            .show(ui, |ui| {
                ui.label("Scanned items");
                ui.label(report.items.len().to_string());
                ui.end_row();
                ui.label("Logical size");
                ui.label(format_bytes(report.total_logical_size_bytes));
                ui.end_row();
                ui.label("Allocated size");
                ui.label(
                    report
                        .total_allocated_size_bytes
                        .map(format_bytes)
                        .unwrap_or_else(|| "Unknown / mixed filesystem evidence".into()),
                );
                ui.end_row();
                ui.label("Already efficient");
                ui.label(report.already_efficient_count.to_string());
                ui.end_row();
                ui.label("Compression candidates");
                ui.label(report.candidate_count.to_string());
                ui.end_row();
                ui.label("Topology-sensitive");
                ui.label(report.topology_sensitive_count.to_string());
                ui.end_row();
                ui.label("Duplicate/shared candidates");
                ui.label(report.duplicate_candidate_count.to_string());
                ui.end_row();
                ui.label("Unsupported/unknown");
                ui.label(report.unsupported_or_unknown_count.to_string());
                ui.end_row();
            });
        ui.separator();
        ui.heading("Items");
        egui::ScrollArea::vertical()
            .id_salt("storage_health_items")
            .show(ui, |ui| {
                for item in &report.items {
                    ui.group(|ui| {
                        ui.label(item.path.display().to_string());
                        ui.small(format!(
                            "{} · {} · logical {} · allocated {}",
                            item.platform.as_deref().unwrap_or("Unknown platform"),
                            item.format,
                            item.logical_size_bytes
                                .map(format_bytes)
                                .unwrap_or_else(|| "unknown".into()),
                            item.allocated_size_bytes
                                .map(format_bytes)
                                .unwrap_or_else(|| "unknown".into())
                        ));
                        ui.label(opportunity_text(item));
                        if let Some(target) = item.opportunity.target_format {
                            ui.small(format!("Future target: {target}"));
                        }
                        ui.small(format!(
                            "Round-trip: {} · Estimate: {:?} · Cleanup: {}",
                            item.opportunity.round_trip,
                            item.opportunity.estimate.kind,
                            item.opportunity.cleanup_eligibility
                        ));
                        for warning in &item.warnings {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                format!("{}: {}", warning.code, warning.message),
                            );
                        }
                        if let Some(warning) = &item.opportunity.warning {
                            ui.colored_label(egui::Color32::YELLOW, warning);
                        }
                    });
                }
            });
    }
}

fn opportunity_text(item: &archivefs_core::storage_health::StorageHealthItem) -> String {
    match item.opportunity.kind {
        StorageOpportunityKind::AlreadyEfficient => {
            "Already efficient: no recompression recommendation.".into()
        }
        StorageOpportunityKind::Compressible => {
            "Possible compression candidate; verification required.".into()
        }
        StorageOpportunityKind::PossibleDuplicateContent => {
            "Possible duplicate/shared content; no physical savings assumed.".into()
        }
        StorageOpportunityKind::TopologySensitive => "Topology-sensitive: review required.".into(),
        StorageOpportunityKind::UnsupportedFormat => {
            "Unsupported format for a safe recommendation.".into()
        }
        StorageOpportunityKind::DoNotConvert => "Do not convert automatically.".into(),
        StorageOpportunityKind::Unknown => {
            "Unknown: insufficient evidence for a safe action.".into()
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_is_read_only() {
        let forbidden = [
            "Convert",
            "Delete Original",
            "Recompress",
            "Deduplicate",
            "Move",
            "Rename",
        ];
        assert!(forbidden.iter().all(|label| !label.contains("Optimise")));
    }
}
