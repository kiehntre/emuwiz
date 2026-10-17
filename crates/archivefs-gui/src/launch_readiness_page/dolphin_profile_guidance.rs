//! Advisory UI over the existing candidate and profile. This module cannot
//! select an emulator, modify a launch request or write Dolphin configuration.
use super::*;
use archivefs_core::launch::dolphin_profile_composition::{
    DolphinGuidanceInspection, inspect_dolphin_profile_guidance,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct GuidanceState {
    path: String,
    pending: Option<Receiver<Result<DolphinGuidanceInspection, String>>>,
    result: Option<Result<DolphinGuidanceInspection, String>>,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    context: &DolphinLaunchContext,
) {
    let LaunchTarget::Standalone {
        adapter_id: "dolphin",
        profile_id,
        ..
    } = &candidate.target
    else {
        return;
    };
    let Some(profile) = context
        .discovery
        .profiles
        .iter()
        .find(|p| &p.profile_id == profile_id)
    else {
        return;
    };
    // Every result is scoped to the already-selected game, content, candidate
    // and discovery snapshot. A new selection never inherits another's advice.
    let key = egui::Id::new((
        "dolphin-profile-guidance",
        format!(
            "{:?}/{:?}/{:?}/{:?}/{:?}",
            plan.platform_id, plan.game_key, candidate, profile, context.roots
        ),
    ));
    let state = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<GuidanceState>>>(key)
            .clone()
    });
    let Ok(mut state) = state.try_lock() else {
        return;
    };
    if let Some(receiver) = &state.pending {
        match receiver.try_recv() {
            Ok(result) => {
                state.result = Some(result);
                state.pending = None;
            }
            Err(TryRecvError::Disconnected) => {
                state.result = Some(Err("Guidance worker stopped; recheck the document.".into()));
                state.pending = None;
            }
            Err(TryRecvError::Empty) => {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
    ui.push_id(key, |ui| {
        ui.collapsing(format!("Dolphin per-game guidance · {profile_id}"), |ui| {
            ui.label("Advisory only · No settings are applied. This does not establish game compatibility or change launch readiness.");
            ui.label("Local reviewed recommendation document (JSON):");
            if ui.text_edit_singleline(&mut state.path).changed() {
                state.result = None;
                state.pending = None;
            }
            ui.small("Uses existing GameProfileSetting evidence records. See docs/DOLPHIN_PROFILE_GUIDANCE.md for an example. No downloads or automatic trust.");
            let busy = state.pending.is_some();
            if ui.add_enabled(!busy && !state.path.trim().is_empty(), egui::Button::new("Inspect / recheck guidance")).clicked() {
                let path = PathBuf::from(state.path.trim());
                let identity = preview_identity(plan);
                let candidate = candidate.clone();
                let profile = profile.clone();
                let roots = context.roots.clone();
                let (sender, receiver) = mpsc::channel();
                let repaint = ui.ctx().clone();
                state.result = None;
                state.pending = Some(receiver);
                thread::spawn(move || {
                    let result = inspect_dolphin_profile_guidance(&identity, &candidate, &profile, &roots, &path);
                    let _ = sender.send(result);
                    repaint.request_repaint();
                });
            }
            if busy { ui.label("Inspecting local evidence…"); }
            if let Some(result) = &state.result {
                match result {
                    Err(error) => { ui.colored_label(theme::warning(ui), format!("Guidance unavailable: {error}")); }
                    Ok(report) => show_report(ui, report),
                }
            } else if !busy {
                ui.label("No profile evidence loaded. Existing Dolphin launch checks remain authoritative.");
            }
        });
    });
}

pub(super) fn show_report(ui: &mut egui::Ui, report: &DolphinGuidanceInspection) {
    let guidance = &report.plan;
    let status = match guidance.status {
        archivefs_core::launch::dolphin_profile_composition::DolphinProfileLaunchStatus::Blocked => "Blocked by existing launch checks",
        archivefs_core::launch::dolphin_profile_composition::DolphinProfileLaunchStatus::NeedsReview => "Needs review / applicability unproven",
        archivefs_core::launch::dolphin_profile_composition::DolphinProfileLaunchStatus::ReadyWithRecommendations => "Advisory recommendations (matching supplied evidence)",
        archivefs_core::launch::dolphin_profile_composition::DolphinProfileLaunchStatus::Ready => "No applicable recommendation",
        archivefs_core::launch::dolphin_profile_composition::DolphinProfileLaunchStatus::Unsupported => "Unsupported context",
    };
    ui.label(format!(
        "Guidance review: {} · installed version: {}",
        status,
        guidance
            .provenance
            .current_emulator_version
            .as_deref()
            .unwrap_or("Unknown")
    ));
    ui.label("Matching supplied version text is not independent proof that a setting or value is supported.");
    for recommendation in guidance
        .portable_recommendations
        .iter()
        .chain(&guidance.host_specific_recommendations)
        .chain(&guidance.personal_settings_excluded)
        .chain(&guidance.dangerous_settings_excluded)
        .chain(&guidance.other_settings)
    {
        ui.label(format!(
            "{} = {} · {:?} / {:?}",
            recommendation.key,
            recommendation
                .proposed_value
                .as_deref()
                .unwrap_or("conflicting values"),
            recommendation.classification,
            recommendation.applicability
        ));
        ui.small(&recommendation.reason);
        for evidence in &recommendation.evidence {
            ui.small(format!(
                "Source claim: {} · version {} · {}",
                evidence.provider,
                evidence.emulator_version.as_deref().unwrap_or("Unknown"),
                evidence.reason
            ));
        }
    }
    for conflict in &guidance.local_conflicts {
        ui.label(format!(
            "Local conflict: {} · observed {} · recommended {}",
            conflict.key, conflict.local_value, conflict.recommended_value
        ));
    }
    for warning in guidance.warnings.iter().chain(&report.notes) {
        ui.small(warning);
    }
    ui.collapsing("Guidance snapshot evidence", |ui| {
        ui.label(format!(
            "Document: {}\nSHA-256: {}",
            report.document_path.display(),
            report.document_sha256
        ));
        ui.label(format!(
            "Exact-ID settings: {}\nSHA-256: {}",
            report.local_path.display(),
            report
                .local_sha256
                .as_deref()
                .unwrap_or("Unavailable / not inspected")
        ));
    });
    ui.add_enabled(false, egui::Button::new("Apply profile (unsupported)"));
}
