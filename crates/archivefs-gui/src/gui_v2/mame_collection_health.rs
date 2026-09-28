//! Read-only MAME collection health presentation.

use archivefs_core::mame_internal_repair::MameInternalRepairPlan;
use archivefs_core::mame_internal_repair_apply::{
    MameInternalRepairApplyOptions, MameInternalRepairApplyPlan, apply_mame_internal_repair_plan,
};
use archivefs_core::mame_playing_library::MamePlayingLibraryPlan;
use eframe::egui;

pub(super) fn show(ui: &mut egui::Ui) {
    show_with_plans(ui, None, None);
}

/// Renders the read-only MAME playing-library projection when an inspection
/// workflow supplies one. `None` is the normal current state until a MAME
/// catalogue/inventory report has been loaded; it never invents metrics.
pub(super) fn show_with_playing_library_plan(
    ui: &mut egui::Ui,
    plan: Option<&MamePlayingLibraryPlan>,
) {
    show_with_plans(ui, plan, None);
}

pub(super) fn show_with_plans(
    ui: &mut egui::Ui,
    plan: Option<&MamePlayingLibraryPlan>,
    repair_plan: Option<&MameInternalRepairPlan>,
) {
    show_with_apply_plan(ui, plan, repair_plan, None);
}

/// Renders the apply controls only when a caller supplies a planner-bound
/// apply projection. The ordinary health route supplies no projection, so it
/// cannot accidentally expose a mutation control for an unreviewed report.
pub(super) fn show_with_apply_plan(
    ui: &mut egui::Ui,
    plan: Option<&MamePlayingLibraryPlan>,
    repair_plan: Option<&MameInternalRepairPlan>,
    apply_plan: Option<&mut MameInternalRepairApplyPlan>,
) {
    egui::CollapsingHeader::new("MAME Collection Health")
        .default_open(true)
        .show(ui, |ui| {
            ui.label("MAME Health");
            ui.label("Review the current collection, then move through Repair, Reconstruction, Verify, Playing Library, and History & Undo.");
            ui.label("This health view is read-only. Any changing workflow still requires its own preview and safety confirmation.");
            ui.horizontal_wrapped(|ui| {
                for label in ["Health", "Repair", "Reconstruction", "Verify", "Playing Library", "History & Undo"] {
                    ui.label(egui::RichText::new(label).strong());
                    if label != "History & Undo" { ui.label("→"); }
                }
            });
            egui::Grid::new("mame_collection_health_summary")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Catalogue"); ui.label("Needs verification"); ui.end_row();
                    ui.label("Collection"); ui.label("Load a MAME root and current catalogue to inspect it"); ui.end_row();
                    ui.label("Health"); ui.label(health_label(repair_plan)); ui.end_row();
                    ui.label("Parent / clone"); ui.label("Shared parent files are explained separately from clone files"); ui.end_row();
                    ui.label("Next action"); ui.label(next_action_label(plan, repair_plan)); ui.end_row();
                });
            ui.strong("Current collection health");
            ui.label("Current evidence is separate from previous repair or reconstruction receipts in History & Undo.");
            ui.separator();
            ui.strong("Top shared problems");
            ui.label("Missing member · Wrong hash · Duplicate member · Unexpected member · Parent dependency");
            ui.label("A parent/shared file may be used by several clones; it is not a generic game-file problem.");
            ui.label("BAD_DUMP means a known imperfect reference dump, not ordinary repairable corruption.");
            ui.label("NO_DUMP means no verified reference dump is known; it is a preservation gap, not a missing file to repair.");
            if ui.button("Export report").clicked() {
                ui.ctx().copy_text("MAME collection health export is available after an inspection report is loaded.".into());
            }
            ui.separator();
            ui.strong("Playing Library / 1G1R preview");
            ui.label("Creates a clean play-focused view without modifying the original ROM collection.");
            if let Some(plan) = plan {
                egui::Grid::new("mame_playing_library_preview")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("Archival collection"); ui.label(format!("{} sets", plan.archival_set_count)); ui.end_row();
                        ui.label("Proposed playing set"); ui.label(plan.projected_set_count.to_string()); ui.end_row();
                        ui.label("Estimated storage"); ui.label(format_bytes(plan.projected_storage_bytes)); ui.end_row();
                        ui.label("Estimated savings"); ui.label(format_bytes(plan.projected_savings_bytes)); ui.end_row();
                        ui.label("Ambiguities"); ui.label(plan.unresolved_cases.len().to_string()); ui.end_row();
                    });
                ui.label(format!("Preference rules selected {} sets; {} alternatives excluded; {} BIOS/device support sets retained.", plan.selected_sets.len(), plan.excluded_sets.len(), plan.required_support_sets.len()));
                ui.label("Source preservation: the archival collection is never mutated by this planner.");
            } else {
                ui.strong("No Playing Library plan");
                ui.label("Load a MAME catalogue and complete collection report to preview selected sets, storage, savings, exclusions, dependencies, and ambiguities.");
                ui.label("No apply, copy, delete, rename, or source-update action is available here.");
            }
            ui.separator();
            ui.strong("Repair from your own collection");
            ui.label("Repair uses exact SHA-1 evidence already present elsewhere; it does not download or rewrite archival originals.");
            if let Some(repair) = repair_plan {
                egui::Grid::new("mame_internal_repair_preview")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("Exact matches already available"); ui.label(repair.safe_internal_repair_count.to_string()); ui.end_row();
                        ui.label("Affected sets"); ui.label(repair.affected_sets.len().to_string()); ui.end_row();
                        ui.label("Projected sets repaired"); ui.label(repair.projected_sets_repairable.to_string()); ui.end_row();
                        ui.label("No-download-needed"); ui.label(repair.no_download_needed_count.to_string()); ui.end_row();
                        ui.label("Genuinely absent"); ui.label(repair.genuinely_absent_count.to_string()); ui.end_row();
                        ui.label("Preservation-only / NO_DUMP"); ui.label(repair.preservation_only_no_dump_count.to_string()); ui.end_row();
                        ui.label("Present but BAD_DUMP"); ui.label(repair.bad_dump_count.to_string()); ui.end_row();
                        ui.label("Ambiguous"); ui.label(repair.ambiguous_count.to_string()); ui.end_row();
                });
                ui.label(format!("{} unique source identities · {} preview operation(s) · {} same-name content mismatch(es)", repair.unique_source_identities_needed, repair.filesystem_operations_required, repair.wrong_content_same_name_count));
                ui.label("Source selection is deterministic and preserves duplicate copies as visible evidence.");
                if let Some(apply_plan) = apply_plan {
                    ui.separator();
                    ui.strong("Apply safe internal repairs");
                    ui.label("EmuWiz already found the exact required bytes elsewhere in your library. This does not download anything or replace archival originals.");
                    let planned = apply_plan.operations.iter().filter(|item| item.state == archivefs_core::mame_internal_repair_apply::MameRepairOperationState::Planned).count();
                    ui.label(format!("{planned} destination write(s); {} conflict(s); {} unsupported archive destination(s).", apply_plan.refused_count, apply_plan.unsupported_count));
                    for operation in apply_plan.operations.iter().filter(|item| item.state == archivefs_core::mame_internal_repair_apply::MameRepairOperationState::Planned).take(8) {
                        ui.label(format!("{} ← {} ({:?})", operation.destination_path.display(), operation.selected_source.display(), operation.strategy));
                    }
                    let phrase = format!("REPAIR {planned} MAME FILES");
                    let mut confirmation = ui.ctx().data_mut(|data| data.get_temp::<String>(egui::Id::new("mame_repair_confirmation")).unwrap_or_default());
                    ui.label(format!("Type {phrase} to confirm."));
                    ui.text_edit_singleline(&mut confirmation);
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("mame_repair_confirmation"), confirmation.clone()));
                    let enabled = planned > 0 && confirmation == phrase;
                    if ui.add_enabled(enabled, egui::Button::new("Apply")).clicked() {
                        let result = apply_mame_internal_repair_plan(apply_plan, &MameInternalRepairApplyOptions::default(), &std::sync::atomic::AtomicBool::new(false));
                        match result {
                            Ok(result) => ui.label(format!("Applied transaction {}. History and undo are available.", result.outcome.transaction.transaction_id)),
                            Err(error) => ui.label(format!("Apply refused: {error}")),
                        };
                    }
                    ui.label("Undo removes only destinations created by this transaction, and refuses if a created file changed.");
                } else {
                    ui.label("No Apply button is available: this health projection is read-only.");
                }
            } else {
                ui.strong("No repairable set report");
                ui.label("Load a MAME catalogue and complete collection report to preview exact internal repair matches, absent identities, preservation gaps, and ambiguities.");
                ui.label("No Apply button is available: this health projection is read-only.");
            }
        });
}

fn health_label(repair_plan: Option<&MameInternalRepairPlan>) -> &'static str {
    let Some(plan) = repair_plan else {
        return "Needs verification";
    };
    if plan.sets_currently_failing == 0 {
        "Healthy"
    } else if plan.genuinely_absent_count > 0 {
        "Missing files"
    } else if plan.wrong_content_same_name_count > 0 || plan.bad_dump_count > 0 {
        "Wrong files"
    } else if plan.ambiguous_count > 0 {
        "Needs attention"
    } else {
        "Needs attention"
    }
}

fn next_action_label(
    plan: Option<&MamePlayingLibraryPlan>,
    repair_plan: Option<&MameInternalRepairPlan>,
) -> &'static str {
    if repair_plan.is_none() {
        "Verify the collection"
    } else if repair_plan.is_some_and(|plan| plan.safe_internal_repair_count > 0) {
        "Review repair"
    } else if plan.is_some() {
        "Open Playing Library"
    } else {
        "Review the evidence"
    }
}

fn format_bytes(value: Option<u64>) -> String {
    let Some(value) = value else {
        return "unknown".into();
    };
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit + 1 < UNITS.len() {
        amount /= 1024.0;
        unit += 1;
    }
    format!("{amount:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> MameInternalRepairPlan {
        MameInternalRepairPlan {
            schema_version: 1,
            collection_root: "/mame".into(),
            catalogue_version: Some("0.264".into()),
            sets_currently_failing: 1,
            affected_sets: vec!["pacman".into()],
            requirements: Vec::new(),
            safe_internal_repair_count: 0,
            no_download_needed_count: 0,
            genuinely_absent_count: 0,
            preservation_only_no_dump_count: 0,
            bad_dump_count: 0,
            ambiguous_count: 0,
            wrong_content_same_name_count: 0,
            unique_source_identities_needed: 0,
            filesystem_operations_required: 0,
            projected_sets_repairable: 0,
            top_repairs_by_impact: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn health_language_uses_current_evidence_not_history() {
        assert_eq!(health_label(None), "Needs verification");
        let mut healthy = plan();
        healthy.sets_currently_failing = 0;
        assert_eq!(health_label(Some(&healthy)), "Healthy");
        let mut missing = plan();
        missing.genuinely_absent_count = 2;
        assert_eq!(health_label(Some(&missing)), "Missing files");
        let mut wrong = plan();
        wrong.wrong_content_same_name_count = 1;
        assert_eq!(health_label(Some(&wrong)), "Wrong files");
    }

    #[test]
    fn next_action_prioritizes_review_before_playing_library() {
        let mut repair = plan();
        repair.safe_internal_repair_count = 1;
        assert_eq!(next_action_label(None, Some(&repair)), "Review repair");
        repair.safe_internal_repair_count = 0;
        assert_eq!(
            next_action_label(None, Some(&repair)),
            "Review the evidence"
        );
    }
}
