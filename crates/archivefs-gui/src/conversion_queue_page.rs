//! GUI-v2 presentation for the generic conversion queue.
//!
//! This page is planning-only until a converter-specific executor can be
//! safely adapted. It never creates, deletes, or replaces files.

use archivefs_core::conversion_queue::{
    ConversionQueue, ConversionQueueState, ConversionReadiness,
};
use eframe::egui;

pub(crate) fn show(ui: &mut egui::Ui, queue: &mut ConversionQueue) {
    ui.heading("Conversion plan list");
    ui.label("This list organizes reviewed plans only; it does not run conversions.");
    ui.small("Apply each conversion from its supported workflow. This panel never creates, replaces or deletes files.");

    let summary = queue.summary();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{} ready", summary.ready));
        ui.label(format!("{} refused", summary.refused));
        ui.label(format!("{} waiting", summary.waiting));
        ui.label(format!("Input: {} B", summary.total_input_size));
        ui.label(format!("Output: {}", summary.estimated_output.display()));
        ui.label(format!(
            "Temporary: {}",
            summary.estimated_temporary_space.display()
        ));
        ui.label(format!(
            "Free: {}",
            summary
                .available_space
                .map_or_else(|| "Unknown".into(), |bytes| format!("{bytes} B"))
        ));
        ui.label(format!(
            "Likely saved: {}",
            summary.likely_space_saved.display()
        ));
    });

    if queue.items.is_empty() {
        ui.label(
            "No conversion plans are listed. Add a reviewed plan from a workflow that supports it.",
        );
        return;
    }

    let mut remove = None;
    egui::Grid::new("conversion_queue_items")
        .striped(true)
        .num_columns(7)
        .show(ui, |ui| {
            for heading in [
                "Platform",
                "Source → target",
                "Ready",
                "Input",
                "Output",
                "Temporary",
                "State",
            ] {
                ui.strong(heading);
            }
            ui.end_row();
            for item in &queue.items {
                ui.push_id(("conversion-plan", item.id), |ui| {
                    ui.label(item.platform.as_deref().unwrap_or("Unknown platform"));
                    ui.label(format!(
                        "{} → {}",
                        item.source_format, item.destination_format
                    ));
                    ui.label(match item.readiness {
                        ConversionReadiness::Ready => "Ready to review",
                        ConversionReadiness::Waiting => "Waiting for review",
                        ConversionReadiness::Refused => "Blocked",
                    });
                    ui.label(format!("{} B", item.estimate.source_size));
                    ui.label(item.estimate.destination_size.display());
                    ui.label(item.estimate.temporary_space.display());
                    ui.label(format_state(item.state));
                    ui.end_row();
                    if let Some(reason) = &item.refusal_or_warning {
                        ui.label(egui::RichText::new(reason).small());
                        ui.end_row();
                    }
                    ui.collapsing("Advanced details", |ui| {
                        ui.label(format!("Source: {}", item.source_path.display()));
                        ui.label(format!("Destination: {}", item.destination_path.display()));
                        ui.label(format!("Converter: {}", item.converter));
                        ui.label(format!("Verification: {}", item.verification_plan));
                        ui.label(format!("Provenance: {}", item.provenance));
                        ui.label(format!(
                            "Atomic publication duplicate space: {}",
                            if item.estimate.atomic_publication_requires_duplicate {
                                "yes"
                            } else {
                                "no or unknown"
                            }
                        ));
                        if let Some(compression) = &item.compression {
                            ui.label(format!("Compression: {}", compression.expected_type));
                            ui.label(format!(
                                "Space saving: {}",
                                compression.space_saving.display()
                            ));
                            ui.label(if compression.preservation_equivalent {
                                "Preservation-equivalent: yes"
                            } else {
                                "Convenience/playback representation: yes"
                            });
                            ui.label(if compression.retain_original {
                                "Original: retain"
                            } else {
                                "Original: review before any later deletion"
                            });
                        }
                    });
                    if matches!(
                        item.state,
                        ConversionQueueState::Ready | ConversionQueueState::Waiting
                    ) && ui.small_button("Remove plan").clicked()
                    {
                        remove = Some(item.id);
                    }
                });
            }
        });
    ui.horizontal(|ui| {
        if ui.button("Cancel pending items").clicked() {
            queue.cancel_pending();
        }
        if ui.button("Clear completed/cancelled").clicked() {
            queue.items.retain(|item| {
                !matches!(
                    item.state,
                    ConversionQueueState::Completed | ConversionQueueState::Cancelled
                )
            });
        }
    });
    if let Some(id) = remove {
        queue.remove(id);
    }
}

fn format_state(state: ConversionQueueState) -> &'static str {
    match state {
        ConversionQueueState::Planned => "Planned",
        ConversionQueueState::Ready => "Ready",
        ConversionQueueState::Waiting => "Waiting",
        ConversionQueueState::Running => "Running",
        ConversionQueueState::Verifying => "Verifying",
        ConversionQueueState::Completed => "Completed · verification shown by workflow",
        ConversionQueueState::Failed => "Failed",
        ConversionQueueState::Refused => "Refused",
        ConversionQueueState::Cancelled => "Cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::conversion_queue::{ConversionPlanningInput, SpaceEstimate};
    use std::path::PathBuf;

    fn text(output: &egui::FullOutput) -> String {
        fn collect(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(children) => {
                    for child in children {
                        collect(child, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut out);
        }
        out
    }

    #[test]
    fn empty_queue_has_plain_language_preview() {
        let mut queue = ConversionQueue::default();
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut queue));
        });
        assert!(output.platform_output.commands.is_empty());
        let rendered = text(&output);
        assert!(rendered.contains("Conversion plan list"));
        assert!(rendered.contains("does not run conversions"));
        assert!(rendered.contains("No conversion plans are listed"));
    }

    #[allow(dead_code)]
    fn _fixture_input() -> ConversionPlanningInput {
        ConversionPlanningInput {
            source_path: PathBuf::from("fixture.iso"),
            source_format: "ISO".into(),
            destination_path: PathBuf::from("fixture.cso"),
            destination_format: "CSO".into(),
            platform: Some("PSP".into()),
            source_size: 1,
            destination_size: SpaceEstimate::Unknown,
            temporary_space: SpaceEstimate::Unknown,
            reclaimable_space: SpaceEstimate::Exact(1),
            converter: "existing PSP workflow".into(),
            verification_plan: "existing verifier".into(),
            provenance: "test".into(),
            readiness_reason: Some("Direct workflow only".into()),
            available_space_override: Some(1),
        }
    }

    #[test]
    fn completed_process_state_does_not_claim_output_verification() {
        let label = format_state(ConversionQueueState::Completed);
        assert!(label.contains("verification shown by workflow"));
        assert!(!label.contains("Verified"));
    }
}
