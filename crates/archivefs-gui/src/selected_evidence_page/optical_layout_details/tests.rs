use super::*;
use crate::selected_evidence_page::{self, OpticalMediaDetails, StructuralMediaDetails};
use archivefs_core::optical_layout_inspection::inspect_optical_layout;
use std::fs;

fn report_for(cue: &str, bin: Option<usize>) -> OpticalLayoutReport {
    let dir = tempfile::tempdir().unwrap();
    if let Some(sectors) = bin {
        fs::write(dir.path().join("g.bin"), vec![0u8; sectors * 2352]).unwrap();
    }
    let path = dir.path().join("g.cue");
    fs::write(&path, cue).unwrap();
    inspect_optical_layout(&path).unwrap()
}

const SIMPLE: &str = "FILE \"g.bin\" BINARY\n TRACK 01 MODE2/2352\n  INDEX 01 00:00:00\n";

#[test]
fn valid_layout_never_claims_byte_verification() {
    let text = lines(&report_for(SIMPLE, Some(20))).join("\n");
    assert!(text.contains("Structurally valid"));
    assert!(text.contains("Not byte verification"));
    assert!(text.contains("No file content was read or hashed"));
    assert!(!text.to_ascii_lowercase().contains("verified by"));
}

#[test]
fn missing_layout_blocks_and_points_away_from_workflows() {
    let text = lines(&report_for(SIMPLE, None)).join("\n");
    assert!(text.contains("Companion missing"));
    assert!(text.contains("g.bin: missing"));
    assert!(text.contains("re-check before using Converter or Launch"));
}

#[test]
fn gap_semantics_are_explained_when_present() {
    let cue = "FILE \"g.bin\" BINARY\n TRACK 01 MODE2/2352\n  INDEX 01 00:00:00\n TRACK 02 AUDIO\n  INDEX 00 00:04:00\n  INDEX 01 00:06:00\n";
    let text = lines(&report_for(cue, Some(600))).join("\n");
    assert!(text.contains("stored in the file"));
    assert!(text.contains("never summed"));
    assert!(text.contains("Redump"));
}

#[test]
fn every_state_has_wording() {
    use OpticalLayoutState::*;
    for s in [
        Valid,
        Missing,
        Ambiguous,
        Mismatch,
        Unsafe,
        Unsupported,
        Unreadable,
    ] {
        assert!(!state(s).0.is_empty());
    }
}

#[test]
fn renders_inside_the_optical_card_without_panicking() {
    let layout = report_for(SIMPLE, Some(20));
    let details = StructuralMediaDetails::Optical(OpticalMediaDetails {
        platform: "Unknown".into(),
        boot_product_evidence: vec![],
        container: "CUE".into(),
        track_session_summary: None,
        dat_status: "No local DAT imported".into(),
        identity_confidence: "Existing identity evidence".into(),
        limitations: vec![],
        layout: Some(layout),
    });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            selected_evidence_page::show_structural_media_details(ui, &details);
        });
    });
    assert!(!output.shapes.is_empty());
}
