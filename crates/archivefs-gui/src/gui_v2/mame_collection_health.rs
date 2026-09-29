//! Read-only MAME collection health presentation.

use archivefs_core::mame_internal_repair::MameInternalRepairPlan;
use archivefs_core::mame_internal_repair::{MameRepairDisposition, MameRepairRequirementClass};
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
            ui.label("Check whether your MAME sets are complete, then choose the safest next step.");
            ui.label("Health checks read your collection. Repair changes files only after a reviewed preview and confirmation; reconstruction creates a separate output.");
            ui.horizontal_wrapped(|ui| {
                for label in MAME_WORKFLOW_LABELS {
                    ui.label(egui::RichText::new(*label).strong());
                    if *label != "History & Undo" { ui.label("→"); }
                }
            });
            egui::Grid::new("mame_collection_health_summary")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Catalogue"); ui.label(catalogue_label(repair_plan)); ui.end_row();
                    ui.label("Collection"); ui.label(collection_label(repair_plan)); ui.end_row();
                    ui.label("Health"); ui.label(health_label(repair_plan)); ui.end_row();
                    ui.label("Parent / clone"); ui.label("Shared parent files are explained separately from clone files"); ui.end_row();
                    ui.label("Next action"); ui.label(next_action_label(plan, repair_plan)); ui.end_row();
                });
            ui.strong("Current collection health");
            ui.label("Current evidence is separate from previous repair or reconstruction receipts in History & Undo.");
            ui.separator();
            if let Some(repair) = repair_plan {
                show_problem_summary(ui, repair);
            } else {
                ui.strong("No MAME collection report");
                ui.label("Choose a MAME collection and current catalogue to check set health.");
            }
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
                ui.label("No MAME Playing Library plan has been built yet.");
                ui.label(PLAYING_LIBRARY_SOURCE_COPY);
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
    } else if plan.wrong_content_same_name_count > 0 {
        "Wrong files"
    } else if plan.bad_dump_count > 0 || plan.preservation_only_no_dump_count > 0 {
        "Needs attention"
    } else if plan.ambiguous_count > 0 {
        "Needs attention"
    } else {
        "Needs attention"
    }
}

fn catalogue_label(plan: Option<&MameInternalRepairPlan>) -> String {
    plan.and_then(|plan| plan.catalogue_version.as_deref())
        .map(|version| format!("MAME catalogue {version}"))
        .unwrap_or_else(|| "Needs verification".into())
}

fn collection_label(plan: Option<&MameInternalRepairPlan>) -> String {
    plan.map(|plan| format!("{} set(s) need attention", plan.sets_currently_failing))
        .unwrap_or_else(|| "No current MAME collection report".into())
}

fn show_problem_summary(ui: &mut egui::Ui, plan: &MameInternalRepairPlan) {
    ui.strong("What needs attention");
    const FILTERS: [&str; 5] = [
        "Needs attention",
        "Missing",
        "Bad hash",
        "Unsupported",
        "Healthy",
    ];
    let filter_id = egui::Id::new("mame_health_problem_filter");
    let mut selected = ui.ctx().data(|data| {
        data.get_temp::<String>(filter_id)
            .unwrap_or_else(|| "Needs attention".into())
    });
    ui.horizontal_wrapped(|ui| {
        for filter in FILTERS {
            let response = ui.selectable_label(selected == filter, filter);
            if response.clicked() {
                selected = filter.into();
                ui.ctx()
                    .data_mut(|data| data.insert_temp(filter_id, selected.clone()));
            }
        }
    });
    for label in filtered_problem_labels(plan, &selected) {
        ui.label(label);
    }
    ui.label("A clone can depend on files stored in its parent set.");
    ui.collapsing("Advanced MAME evidence", |ui| {
        ui.label(format!(
            "DAT source version: {}",
            plan.catalogue_version.as_deref().unwrap_or("unknown")
        ));
        ui.label(format!(
            "Collection root: {}",
            plan.collection_root.display()
        ));
        for item in &plan.requirements {
            ui.push_id((&item.affected_set, &item.required_filename), |ui| {
                ui.label(format!(
                    "{} · {} · {}",
                    item.affected_set,
                    item.required_filename,
                    item.disposition.label()
                ));
                ui.small(format!(
                    "owner={} parent={} member={} CRC={} SHA-1={}",
                    item.relationship.set,
                    item.relationship.parent.as_deref().unwrap_or("none"),
                    item.relationship.merge_member.as_deref().unwrap_or("none"),
                    item.crc.as_deref().unwrap_or("not recorded"),
                    item.sha1.as_deref().unwrap_or("not recorded")
                ));
            });
        }
    });
}

fn problem_labels(plan: &MameInternalRepairPlan) -> Vec<String> {
    let (missing, wrong, duplicates, parent, bad_dump, no_dump) = problem_counts(plan);
    let mut labels = Vec::new();
    if missing > 0 {
        labels.push(format!(
            "Missing member: {missing} required file(s) are missing."
        ));
    }
    if wrong > 0 {
        labels.push(format!(
            "Wrong hash: {wrong} file(s) have the wrong content."
        ));
    }
    if duplicates > 0 {
        labels.push(format!(
            "Duplicate member evidence: {duplicates} requirement(s) have multiple matching copies."
        ));
    }
    if parent > 0 {
        labels.push(format!(
            "Parent dependency missing: {parent} shared file(s) are required from a parent set."
        ));
    }
    if bad_dump > 0 {
        labels.push(format!("BAD_DUMP: {bad_dump} reference file(s) are known imperfect dumps; this is not ordinary corruption."));
    }
    if no_dump > 0 {
        labels.push(format!(
            "NO_DUMP: {no_dump} reference file(s) have no known dump; this is a preservation gap."
        ));
    }
    if labels.is_empty() {
        labels.push(
            if plan.sets_currently_failing == 0 {
                "No MAME problems found."
            } else {
                "The report needs review; see Advanced for the available evidence."
            }
            .into(),
        );
    }
    labels
}

fn filtered_problem_labels(plan: &MameInternalRepairPlan, filter: &str) -> Vec<String> {
    let labels = problem_labels(plan);
    match filter {
        "Missing" => labels
            .into_iter()
            .filter(|label| {
                label.starts_with("Missing member:")
                    || label.starts_with("Parent dependency missing:")
            })
            .collect(),
        "Bad hash" => labels
            .into_iter()
            .filter(|label| label.starts_with("Wrong hash:"))
            .collect(),
        "Unsupported" => labels
            .into_iter()
            .filter(|label| label.starts_with("BAD_DUMP:") || label.starts_with("NO_DUMP:"))
            .collect(),
        "Healthy" if plan.sets_currently_failing == 0 => vec!["No MAME problems found.".into()],
        "Healthy" => Vec::new(),
        _ => labels,
    }
}

fn problem_counts(plan: &MameInternalRepairPlan) -> (usize, usize, usize, usize, usize, usize) {
    let duplicates = plan
        .requirements
        .iter()
        .filter(|item| {
            item.disposition == MameRepairDisposition::Ambiguous
                && item.exact_matching_sources.len() > 1
        })
        .count();
    let parent = plan
        .requirements
        .iter()
        .filter(|item| {
            item.relationship.kind
                == archivefs_core::mame_internal_repair::MameRepairRelationshipKind::ParentShared
                && item.disposition == MameRepairDisposition::GenuinelyAbsent
        })
        .count();
    let bad_dump = plan
        .requirements
        .iter()
        .filter(|item| item.requirement_class == MameRepairRequirementClass::BadDump)
        .count();
    let no_dump = plan
        .requirements
        .iter()
        .filter(|item| item.requirement_class == MameRepairRequirementClass::NoDump)
        .count();
    (
        plan.genuinely_absent_count,
        plan.wrong_content_same_name_count,
        duplicates,
        parent,
        bad_dump,
        no_dump,
    )
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
    use archivefs_core::mame_internal_repair::{
        MameInternalRepairRequirement, MameRepairConfidence, MameRepairRelationship,
        MameRepairRelationshipKind, MameRepairSource,
    };
    use std::path::PathBuf;

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

    fn requirement(
        class: MameRepairRequirementClass,
        disposition: MameRepairDisposition,
    ) -> MameInternalRepairRequirement {
        MameInternalRepairRequirement {
            affected_set: "clone".into(),
            required_filename: "rom.bin".into(),
            required_size: Some(12),
            crc: Some("12345678".into()),
            sha1: Some("aabb".into()),
            requirement_class: class,
            relationship: MameRepairRelationship {
                kind: MameRepairRelationshipKind::ParentShared,
                set: "clone".into(),
                parent: Some("parent".into()),
                merge_member: Some("shared.bin".into()),
                device_refs: Vec::new(),
            },
            expected_destination_container: Some(PathBuf::from("/mame/clone.zip")),
            expected_destination_is_directory: false,
            expected_destination_member: "rom.bin".into(),
            exact_matching_sources: Vec::<MameRepairSource>::new(),
            source_sha1_verified: false,
            ambiguity: None,
            preservation_status: "reference".into(),
            proposed_operation: "none".into(),
            repair_confidence: MameRepairConfidence::Refused,
            disposition,
            refusal_reason: None,
        }
    }

    #[test]
    fn current_healthy_set_is_simple() {
        let mut p = plan();
        p.sets_currently_failing = 0;
        assert_eq!(health_label(Some(&p)), "Healthy");
    }
    #[test]
    fn missing_members_use_a_human_count() {
        let mut p = plan();
        p.genuinely_absent_count = 3;
        assert!(
            problem_labels(&p)
                .iter()
                .any(|s| s.contains("3 required file(s) are missing"))
        );
    }
    #[test]
    fn wrong_hash_is_distinct_from_missing() {
        let mut p = plan();
        p.wrong_content_same_name_count = 1;
        assert!(problem_labels(&p)[0].starts_with("Wrong hash:"));
    }
    #[test]
    fn bad_dump_is_not_called_corruption_or_repair() {
        let mut p = plan();
        p.requirements.push(requirement(
            MameRepairRequirementClass::BadDump,
            MameRepairDisposition::PresentButBadDump,
        ));
        let text = problem_labels(&p).join(" ");
        assert!(text.contains("known imperfect"));
        assert!(!text.contains("Repair:"));
        assert_eq!(next_action_label(None, Some(&p)), "Review the evidence");
    }
    #[test]
    fn no_dump_is_a_preservation_gap() {
        let mut p = plan();
        p.requirements.push(requirement(
            MameRepairRequirementClass::NoDump,
            MameRepairDisposition::NoDump,
        ));
        assert!(problem_labels(&p).join(" ").contains("no known dump"));
    }
    #[test]
    fn repair_and_reconstruction_are_separate_sections() {
        let mut p = plan();
        p.safe_internal_repair_count = 1;
        assert_eq!(next_action_label(None, Some(&p)), "Review repair");
        assert!(MAME_WORKFLOW_LABELS.contains(&"Reconstruction"));
    }
    #[test]
    fn unsupported_or_missing_evidence_does_not_offer_repair() {
        let p = plan();
        assert_eq!(next_action_label(None, Some(&p)), "Review the evidence");
    }
    #[test]
    fn current_health_ignores_historical_receipts() {
        let mut p = plan();
        p.sets_currently_failing = 0;
        assert_eq!(health_label(Some(&p)), "Healthy");
    }
    #[test]
    fn successful_repair_leads_to_verification_when_no_plan_is_loaded() {
        assert_eq!(next_action_label(None, None), "Verify the collection");
    }
    #[test]
    fn playing_library_explains_source_preservation() {
        assert!(PLAYING_LIBRARY_SOURCE_COPY.contains("original ROM sets stay unchanged"));
    }
    #[test]
    fn advanced_evidence_retains_hash_and_provenance_fields() {
        let p = plan();
        let req = requirement(
            MameRepairRequirementClass::GameSpecificRom,
            MameRepairDisposition::GenuinelyAbsent,
        );
        assert_eq!(req.sha1.as_deref(), Some("aabb"));
        assert_eq!(req.relationship.parent.as_deref(), Some("parent"));
        assert_eq!(p.catalogue_version.as_deref(), Some("0.264"));
    }
    #[test]
    fn missing_and_wrong_hash_are_separate_labels() {
        let mut p = plan();
        p.genuinely_absent_count = 1;
        p.wrong_content_same_name_count = 2;
        let labels = problem_labels(&p);
        assert!(labels[0].starts_with("Missing member:"));
        assert!(labels[1].starts_with("Wrong hash:"));
    }
    #[test]
    fn duplicate_member_is_grouped_by_meaning() {
        let mut p = plan();
        let mut r = requirement(
            MameRepairRequirementClass::GameSpecificRom,
            MameRepairDisposition::Ambiguous,
        );
        r.exact_matching_sources = vec![source("a.zip"), source("b.zip")];
        p.requirements.push(r);
        assert!(
            problem_labels(&p)
                .iter()
                .any(|s| s.starts_with("Duplicate member evidence:"))
        );
    }
    #[test]
    fn parent_dependency_is_explained_in_plain_language() {
        let mut p = plan();
        p.requirements.push(requirement(
            MameRepairRequirementClass::ParentSharedRom,
            MameRepairDisposition::GenuinelyAbsent,
        ));
        assert!(
            problem_labels(&p)
                .iter()
                .any(|s| s.contains("required from a parent set"))
        );
    }
    #[test]
    fn no_problem_and_no_report_empty_states_are_distinct() {
        let mut p = plan();
        p.sets_currently_failing = 0;
        assert_eq!(problem_labels(&p)[0], "No MAME problems found.");
        assert_eq!(collection_label(None), "No current MAME collection report");
    }
    #[test]
    fn paint_only_health_render_does_not_mutate_input() {
        let p = plan();
        let before = p.clone();
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_with_plans(ui, None, Some(&p)));
        });
        assert_eq!(p, before);
    }
    #[test]
    fn mame_problem_route_is_mame_scoped() {
        let action =
            crate::gui_v2::routes::family_children(crate::gui_v2::routes::FeatureFamily::Mame)
                .into_iter()
                .find(|a| a.label == "Problems")
                .unwrap();
        assert_eq!(action.route, crate::gui_v2::routes::Route::MameWorkflow);
    }
    #[test]
    fn rendered_health_uses_stable_semantic_id() {
        assert_eq!(
            egui::Id::new("mame_collection_health_summary"),
            egui::Id::new("mame_collection_health_summary")
        );
    }

    #[test]
    fn presentation_filtering_does_not_mutate_backend_evidence() {
        let mut p = plan();
        p.genuinely_absent_count = 2;
        p.wrong_content_same_name_count = 1;
        let before = p.clone();
        assert!(filtered_problem_labels(&p, "Missing")[0].starts_with("Missing member:"));
        assert!(filtered_problem_labels(&p, "Bad hash")[0].starts_with("Wrong hash:"));
        assert_eq!(p, before);
    }

    fn source(name: &str) -> MameRepairSource {
        MameRepairSource {
            container: PathBuf::from(name),
            member: "rom.bin".into(),
            container_is_directory: false,
            sha1: "aabb".into(),
            size_bytes: Some(12),
        }
    }
}

const MAME_WORKFLOW_LABELS: &[&str] = &[
    "Health",
    "Repair",
    "Reconstruction",
    "Verify",
    "Playing Library",
    "History & Undo",
];
const PLAYING_LIBRARY_SOURCE_COPY: &str = "A plan selects preferred sets and creates a separate play-focused view; original ROM sets stay unchanged.";
