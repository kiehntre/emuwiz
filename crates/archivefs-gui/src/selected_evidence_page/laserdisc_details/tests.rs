use super::*;
use crate::selected_evidence_page::{self, StructuralMediaDetails};
use archivefs_core::laserdisc_set::{FrameMapping, LaserdiscFrameRangeEvidence};
use std::path::PathBuf;

fn fixture() -> LaserdiscSetEvidence {
    let root = PathBuf::from("/nonexistent/synthetic-laserdisc");
    LaserdiscSetEvidence {
        set_root: root.clone(),
        detected_family: LaserdiscFamily::Daphne,
        framefile_path: Some(root.join("test.framefile")),
        mappings: vec![FrameMapping {
            start_frame: 0,
            media_name: "video.m2v".into(),
            line: 2,
        }],
        referenced_media: vec!["video.m2v".into()],
        present_media: vec![root.join("video.m2v")],
        missing_media: vec![],
        video_assets: vec![VideoAssetEvidence {
            media_name: "video.m2v".into(),
            path: root.join("video.m2v"),
            exists: true,
            readable: true,
            size_bytes: Some(1234),
            metadata_note: Some("bounded ffprobe summary; no full decode".into()),
            metadata: Some(LaserdiscMediaMetadata {
                container_format: Some("mpeg".into()),
                video_codec: Some("mpeg2video".into()),
                width: Some(640),
                height: Some(480),
                frame_rate: Some("30000/1001".into()),
                duration_millis: Some(12_500),
                reported_frame_count: Some(375),
                audio_stream_count: 1,
                video_stream_count: 1,
                probe_status: LaserdiscProbeStatus::Available,
                warnings: vec![],
            }),
        }],
        frame_ranges: vec![LaserdiscFrameRangeEvidence {
            media_name: "video.m2v".into(),
            first_referenced_frame: 0,
            last_referenced_frame: 374,
            status: LaserdiscFrameRangeStatus::RangeValid,
        }],
        frame_range_status: LaserdiscFrameRangeStatus::RangeValid,
        rom_components: vec![root.join("test.rom")],
        script_components: vec![],
        config_components: vec![],
        warnings: vec![],
        readiness: LaserdiscReadiness::Ready,
    }
}

fn render(evidence: &LaserdiscSetEvidence, expanded: bool, width: f32) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);
    let draw = |events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 3000.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    selected_evidence_page::show_structural_media_details(
                        ui,
                        &StructuralMediaDetails::LaserDisc(evidence.clone()),
                    );
                });
            },
        )
    };
    let collapsed = draw(Vec::new());
    if !expanded {
        return collapsed;
    }
    // Exercise the real disclosure with a pointer click. Its internal child
    // Ui id is intentionally not part of this test's contract.
    fn disclosure_position(shape: &egui::Shape) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == "Technical details" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(disclosure_position),
            _ => None,
        }
    }
    let pos = collapsed
        .shapes
        .iter()
        .find_map(|shape| disclosure_position(&shape.shape))
        .expect("Technical details must be reachable");
    let _ = draw(vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        },
    ]);
    let _ = draw(vec![egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    }]);
    draw(Vec::new())
}

fn text(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::Shape, result: &mut String) {
        match shape {
            egui::Shape::Text(shape) => {
                result.push_str(shape.galley.text());
                result.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, result);
                }
            }
            _ => {}
        }
    }
    let mut result = String::new();
    for shape in &output.shapes {
        collect(&shape.shape, &mut result);
    }
    result
}

#[test]
fn every_range_state_has_plain_wording_and_an_honest_tone() {
    for (status, expected, tone) in [
        (
            LaserdiscFrameRangeStatus::RangeValid,
            "Within reported frame count",
            StatusTone::Success,
        ),
        (
            LaserdiscFrameRangeStatus::RangeExceedsMedia,
            "Exceeds reported media length",
            StatusTone::Blocked,
        ),
        (
            LaserdiscFrameRangeStatus::RangeUnverified,
            "Unverified",
            StatusTone::Pending,
        ),
        (
            LaserdiscFrameRangeStatus::MetadataUnavailable,
            "Metadata unavailable",
            StatusTone::Pending,
        ),
        (
            LaserdiscFrameRangeStatus::MalformedMapping,
            "Malformed mapping",
            StatusTone::Blocked,
        ),
    ] {
        assert_eq!(frame_range(status), (expected, tone));
        let mut evidence = fixture();
        evidence.frame_range_status = status;
        assert!(text(&render(&evidence, false, 560.0)).contains(expected));
    }
}

#[test]
fn readiness_is_projected_not_recomputed_from_metadata() {
    for (status, label, tone) in [
        (LaserdiscReadiness::Ready, "Ready", StatusTone::Success),
        (LaserdiscReadiness::Partial, "Partial", StatusTone::Warning),
        (LaserdiscReadiness::Broken, "Broken", StatusTone::Blocked),
        (LaserdiscReadiness::Unknown, "Unknown", StatusTone::Pending),
    ] {
        assert_eq!(readiness(status), (label, tone));
    }
    let mut evidence = fixture();
    evidence.frame_range_status = LaserdiscFrameRangeStatus::MetadataUnavailable;
    evidence.video_assets[0].metadata = None;
    let output = text(&render(&evidence, false, 560.0));
    assert!(output.contains("Ready"));
    assert!(output.contains("Metadata unavailable"));
    assert!(output.contains("Frame coverage is not proven"));
    assert!(!output.contains("Broken"));
}

#[test]
fn compact_card_shows_resolution_duration_range_and_limits() {
    let evidence = fixture();
    let output = text(&render(&evidence, false, 420.0));
    for expected in [
        "LaserDisc set",
        "Daphne",
        "Ready",
        "1 present, 0 missing",
        "640×480",
        "0:00:12.500",
        "Within reported frame count",
        "not an emulator launch test",
    ] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    assert!(
        !output.contains("mpeg2video"),
        "advanced fields start collapsed"
    );
}

#[test]
fn technical_disclosure_shows_backend_metadata_and_provenance() {
    let output = text(&render(&fixture(), true, 900.0));
    for expected in [
        "Source set:",
        "Framefile:",
        "test.framefile",
        "mpeg2video",
        "30000/1001",
        "Reported frame count: 375",
        "Streams: 1 video, 1 audio",
        "0–374",
        "File size: 1234 bytes",
        "no extra probe runs",
    ] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
}

#[test]
fn broken_range_and_warnings_are_visible_without_expanding() {
    let mut evidence = fixture();
    evidence.readiness = LaserdiscReadiness::Broken;
    evidence.frame_range_status = LaserdiscFrameRangeStatus::RangeExceedsMedia;
    evidence
        .warnings
        .push("video.m2v: mapping exceeds reported frames".into());
    let output = text(&render(&evidence, false, 560.0));
    assert!(output.contains("Broken"));
    assert!(output.contains("Exceeds reported media length"));
    assert!(output.contains("video.m2v: mapping exceeds reported frames"));
}

#[test]
fn unavailable_probe_does_not_display_default_zero_stream_counts_as_facts() {
    for status in [
        LaserdiscProbeStatus::Unavailable,
        LaserdiscProbeStatus::Failed,
    ] {
        let mut evidence = fixture();
        let metadata = evidence.video_assets[0].metadata.as_mut().unwrap();
        metadata.probe_status = status;
        metadata.video_stream_count = 0;
        metadata.audio_stream_count = 0;
        metadata.warnings.push("synthetic probe limitation".into());
        let output = text(&render(&evidence, true, 900.0));
        assert!(output.contains(probe_label(status)));
        assert!(output.contains("synthetic probe limitation"));
        assert!(!output.contains("Streams:"));
        assert!(!output.contains("640×480"));
    }
}

#[test]
fn missing_frame_count_is_never_estimated_and_zero_observations_are_preserved() {
    let mut evidence = fixture();
    let metadata = evidence.video_assets[0].metadata.as_mut().unwrap();
    metadata.reported_frame_count = None;
    metadata.duration_millis = Some(0);
    metadata.video_stream_count = 0;
    let output = text(&render(&evidence, true, 900.0));
    assert!(output.contains("Reported frame count: unknown (not estimated)"));
    assert!(output.contains("0:00:00.000"));
    assert!(output.contains("Streams: 0 video, 1 audio"));
}

#[test]
fn each_media_keeps_its_own_range_and_metadata() {
    let mut evidence = fixture();
    let mut second = evidence.video_assets[0].clone();
    second.media_name = "second.m2v".into();
    second.path = evidence.set_root.join(&second.media_name);
    second.metadata.as_mut().unwrap().width = Some(720);
    evidence.video_assets.push(second);
    evidence.frame_ranges.push(LaserdiscFrameRangeEvidence {
        media_name: "second.m2v".into(),
        first_referenced_frame: 400,
        last_referenced_frame: 800,
        status: LaserdiscFrameRangeStatus::RangeUnverified,
    });
    let output = text(&render(&evidence, true, 900.0));
    assert!(
        output.contains("video.m2v: referenced mapping frames 0–374 · Within reported frame count")
    );
    assert!(output.contains("second.m2v: referenced mapping frames 400–800 · Unverified"));
    assert!(output.contains("second.m2v: 720×480"));
}

#[test]
fn extra_warnings_and_missing_media_remain_accessible() {
    let mut evidence = fixture();
    evidence.warnings = (0..5).map(|i| format!("synthetic warning {i}")).collect();
    evidence.missing_media = (0..5).map(|i| format!("missing{i}.m2v")).collect();
    let compact = text(&render(&evidence, false, 700.0));
    assert!(compact.contains("More missing media or warnings"));
    assert!(!compact.contains("synthetic warning 4"));
    let advanced = text(&render(&evidence, true, 900.0));
    assert!(advanced.contains("synthetic warning 4"));
    assert!(advanced.contains("missing4.m2v"));
}

#[test]
fn all_families_have_readable_names_without_guessing() {
    for (family, name) in [
        (LaserdiscFamily::Daphne, "Daphne"),
        (LaserdiscFamily::HypseusSinge, "Hypseus / Singe"),
        (LaserdiscFamily::Mame, "MAME"),
        (LaserdiscFamily::Unknown, "Not determined"),
    ] {
        let mut evidence = fixture();
        evidence.detected_family = family;
        assert!(text(&render(&evidence, false, 560.0)).contains(name));
    }
}

#[test]
fn selected_directory_retains_verifier_result_and_renders_missing_media_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let framefile = dir.path().join("synthetic.framefile");
    let rom = dir.path().join("synthetic.rom");
    std::fs::write(&framefile, b"0 missing.m2v\n").unwrap();
    std::fs::write(&rom, b"synthetic companion, not a ROM dump").unwrap();
    let before = (
        std::fs::read(&framefile).unwrap(),
        std::fs::read(&rom).unwrap(),
    );
    // No video exists: this invokes the real verifier without ffprobe or
    // reliance on any host emulator/media installation.
    let expected = archivefs_core::laserdisc_set::verify_laserdisc_set(dir.path()).unwrap();
    let report = selected_evidence_page::gather_selected_evidence_fast(dir.path(), None).unwrap();
    let Some(StructuralMediaDetails::LaserDisc(evidence)) = report.structural_media else {
        panic!("selected-directory path did not retain LaserDisc evidence");
    };
    assert_eq!(
        evidence, expected,
        "no fields may be discarded by projection"
    );
    let output = text(&render(&evidence, false, 700.0));
    assert!(output.contains("Broken"));
    assert!(output.contains("Missing or empty media: missing.m2v"));
    assert_eq!(
        before,
        (
            std::fs::read(&framefile).unwrap(),
            std::fs::read(&rom).unwrap()
        )
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn rendering_does_not_require_source_files_or_mutate_the_result() {
    let evidence = fixture();
    let before = evidence.clone();
    let _ = render(&evidence, true, 900.0);
    let _ = render(&evidence, false, 420.0);
    assert_eq!(evidence, before);
}
