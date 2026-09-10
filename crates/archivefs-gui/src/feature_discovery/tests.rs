use std::path::PathBuf;

use archivefs_core::platform_evidence_fusion::identity_orchestrator::{
    IdentityInspectionInput, inspect_identity,
};
use archivefs_core::platform_evidence_fusion::identity_presentation::{
    IdentityStatus, present_identity,
};

use crate::selected_evidence_page::{NoIntroLookupResult, SelectedEvidenceEnrichmentStatus};

use super::*;

fn base_report(path: &str) -> SelectedEvidenceReport {
    let identity_result = inspect_identity(IdentityInspectionInput::default());
    SelectedEvidenceReport {
        path: PathBuf::from(path),
        tape_analysis: None,
        structural_facts: Vec::new(),
        identity: present_identity(&identity_result),
        identity_result,
        game_identity_report: archivefs_core::game_identity::GameIdentityReport {
            archive_path: PathBuf::from(path),
            platform: archivefs_core::game_identity::IdentityPlatform::Other,
            format: archivefs_core::game_identity::IdentityImageFormat::Unsupported,
            evidence: Vec::new(),
            warnings: Vec::new(),
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: false,
        },
        hashes: None,
        no_intro: NoIntroLookupResult::NotImported,
        enrichment: SelectedEvidenceEnrichmentStatus::Complete,
        base_observations: Vec::new(),
        structural_media: None,
    }
}

fn with_status(
    mut report: SelectedEvidenceReport,
    status: IdentityStatus,
) -> SelectedEvidenceReport {
    report.identity.status = status;
    report
}

fn synthetic_tape_analysis() -> archivefs_core::tape_analysis::TapeAnalysis {
    archivefs_core::tape_analysis::TapeAnalysis {
        format: archivefs_core::tape_analysis::TapeFormat::BbcUef,
        platform: Some("BBC Micro"),
        block_count: 0,
        entries: Vec::new(),
        metadata: Vec::new(),
        loader: None,
        checksum: archivefs_core::tape_analysis::ChecksumState::NotApplicable,
        warnings: Vec::new(),
        semantic_blocks: Vec::new(),
        logical_segments: 0,
        unsupported_blocks: 0,
    }
}

#[test]
fn unknown_identity_is_reported_unavailable_with_a_reason() {
    let report = base_report("game.gba");
    let view = build_feature_discovery(&report);
    assert!(
        view.available
            .iter()
            .all(|item| item.label != "DAT verified")
    );
    let entry = view
        .unavailable
        .iter()
        .find(|item| item.label == "DAT verification")
        .expect("unknown identity must be explained, not hidden");
    assert!(entry.reason.contains("has not been identified"));
}

#[test]
fn verified_by_dat_is_reported_available_in_plain_language() {
    let report = with_status(base_report("game.gba"), IdentityStatus::VerifiedByDat);
    let view = build_feature_discovery(&report);
    assert!(
        view.available
            .iter()
            .any(|item| item.label == "DAT verified"),
        "must use plain language, not the internal VerifiedSingleMatch-style term"
    );
}

#[test]
fn content_and_dat_agree_is_also_reported_available() {
    let report = with_status(base_report("game.gba"), IdentityStatus::ContentAndDatAgree);
    let view = build_feature_discovery(&report);
    assert!(
        view.available
            .iter()
            .any(|item| item.label == "DAT verified")
    );
}

#[test]
fn ambiguous_identity_needs_attention_not_a_silent_available() {
    let report = with_status(base_report("game.gba"), IdentityStatus::Ambiguous);
    let view = build_feature_discovery(&report);
    assert!(
        view.available
            .iter()
            .all(|item| item.label != "DAT verified")
    );
    assert!(
        view.needs_attention
            .iter()
            .any(|item| item.label.contains("ambiguous"))
    );
}

#[test]
fn tape_analysis_available_is_surfaced_as_a_status_highlight() {
    let mut report = base_report("game.uef");
    report.tape_analysis = Some(Ok(synthetic_tape_analysis()));
    let view = build_feature_discovery(&report);
    let item = view
        .available
        .iter()
        .find(|item| item.label.starts_with("Tape analysis available"))
        .expect("tape analysis available must be surfaced");
    // No click action: the real tape evidence already renders further down
    // the same page - see the module doc comment.
    assert_eq!(item.action_label, None);
    assert_eq!(item.action, None);
}

#[test]
fn tape_analysis_absent_explains_why_rather_than_hiding_the_row() {
    let report = base_report("game.gba");
    let view = build_feature_discovery(&report);
    let entry = view
        .unavailable
        .iter()
        .find(|item| item.label == "Tape analysis")
        .expect("absence must be explained");
    assert_eq!(entry.reason, "No tape analysis for this file type.");
}

#[test]
fn tape_analysis_failure_is_needs_attention_not_available_or_silently_dropped() {
    let mut report = base_report("game.uef");
    report.tape_analysis = Some(Err("truncated chunk".to_string()));
    let view = build_feature_discovery(&report);
    assert!(
        view.available
            .iter()
            .all(|item| !item.label.starts_with("Tape"))
    );
    assert!(
        view.needs_attention
            .iter()
            .any(|item| item.label.contains("truncated chunk"))
    );
}

#[test]
fn a_cue_file_offers_convert_format() {
    let report = base_report("game.cue");
    let view = build_feature_discovery(&report);
    let item = view
        .available
        .iter()
        .find(|item| item.action == Some(FeatureDiscoveryAction::OpenDiscConversion))
        .expect("a CUE file must offer format conversion");
    assert_eq!(item.action_label, Some("Convert format"));
}

#[test]
fn a_non_cue_file_explains_no_safe_conversion_rather_than_pretending_universality() {
    let report = base_report("game.gba");
    let view = build_feature_discovery(&report);
    let entry = view
        .unavailable
        .iter()
        .find(|item| item.label == "Format conversion")
        .expect("must explain the absence");
    assert_eq!(
        entry.reason,
        "No safe conversion is currently available for this format."
    );
}

#[test]
fn ordering_is_deterministic_across_repeated_builds() {
    let mut report = base_report("game.cue");
    report.tape_analysis = Some(Ok(synthetic_tape_analysis()));
    report = with_status(report, IdentityStatus::VerifiedByDat);
    let first = build_feature_discovery(&report);
    let second = build_feature_discovery(&report);
    assert_eq!(first, second);
    // Fixed order: DAT identity, then tape, then conversion.
    let labels: Vec<&str> = first
        .available
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(
        labels,
        vec![
            "DAT verified",
            "Tape analysis available (see below)",
            "Convertible to CHD (fingerprint-verified)",
        ]
    );
}

#[test]
fn rendering_never_panics_and_reports_no_action_without_a_click() {
    let report = base_report("game.gba");
    let view = build_feature_discovery(&report);
    let context = egui::Context::default();
    let mut action = None;
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(700.0, 500.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                action = show(ui, &view);
            });
        },
    );
    assert!(action.is_none());
}

#[test]
fn cross_feature_context_surfaces_only_loaded_capabilities() {
    let report = base_report("game.gba");
    let context = FeatureDiscoveryContext {
        cheats: Some(FeatureStatus::NeedsAttention {
            label: "Cheat conflicts need review".to_string(),
            action_label: Some("Review cheats"),
            action: Some(FeatureDiscoveryAction::OpenCheats),
        }),
        romm: Some(FeatureStatus::Available {
            label: "RomM is up to date".to_string(),
            action_label: Some("Open RomM"),
            action: Some(FeatureDiscoveryAction::OpenRomm),
        }),
        emulator: Some(FeatureStatus::Unavailable {
            label: "Emulator readiness".to_string(),
            reason: "Emulator setup has not completed a usable check yet.".to_string(),
        }),
        cover_available: Some(false),
        screenshot_count: Some(2),
        video_available: Some(false),
    };
    let view = build_feature_discovery_with_context(&report, &context);
    assert!(
        view.needs_attention
            .iter()
            .any(|item| item.label == "Cheat conflicts need review")
    );
    assert!(
        view.available
            .iter()
            .any(|item| item.label == "RomM is up to date")
    );
    assert!(
        view.available
            .iter()
            .any(|item| item.label == "Screenshots available (2)")
    );
    assert!(view.unavailable.iter().any(|item| item.label == "Video"));
}

#[test]
fn context_actions_are_explicit_and_never_apply_changes() {
    let report = base_report("game.gba");
    let context = FeatureDiscoveryContext {
        cheats: Some(FeatureStatus::Available {
            label: "Cheat workflow available".to_string(),
            action_label: Some("Review cheats"),
            action: Some(FeatureDiscoveryAction::OpenCheats),
        }),
        ..FeatureDiscoveryContext::default()
    };
    let view = build_feature_discovery_with_context(&report, &context);
    let item = view
        .available
        .iter()
        .find(|item| item.label == "Cheat workflow available")
        .unwrap();
    assert_eq!(item.action, Some(FeatureDiscoveryAction::OpenCheats));
}
