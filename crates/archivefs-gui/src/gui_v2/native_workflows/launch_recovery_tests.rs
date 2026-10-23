//! Regression for a rejected GB snapshot surviving restoration and a rescan.
use super::*;
use crate::selected_evidence_page::{HasheousState, SelectedEvidenceState};

fn frame(
    context: &egui::Context,
    workflows: &mut NativeWorkflows,
    path: &Path,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                workflows.show_launch(ui, 1, path, None, &mut Activity::default());
            });
        },
    )
}

fn text_center(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Pos2> {
    match shape {
        egui::epaint::Shape::Text(text) if text.galley.text() == label => {
            Some(text.pos + text.galley.size() / 2.0)
        }
        egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(|s| text_center(s, label)),
        _ => None,
    }
}

fn valid_cartridge() -> Vec<u8> {
    let mut bytes = vec![0; 0x8000];
    let logo = [
        0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00,
        0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD,
        0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB,
        0xB9, 0x33, 0x3E,
    ];
    bytes[0x104..0x134].copy_from_slice(&logo);
    bytes[0x14D] = archivefs_core::gb_header_evidence::compute_header_checksum(&bytes).unwrap();
    bytes
}

fn retry_current_bytes(repair: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repaired.gb");
    let invalid = vec![0x55; 0x8000];
    std::fs::write(&path, &invalid).unwrap();
    let report =
        selected_evidence_page::gather_selected_evidence_fast(&path, Some("Game Boy")).unwrap();
    assert_eq!(
        report.game_identity_report.verified_loose_rom_sha256(),
        None
    );

    let context = egui::Context::default();
    let mut workflows = NativeWorkflows::new(context.clone());
    workflows.app = crate::tests::app_with_cheat_game(path.to_str().unwrap(), "Game Boy");
    workflows.app.catalogued_platform_hint = Some((path.clone(), "Game Boy".into()));
    workflows
        .app
        .selected_evidence_ui
        .selected_evidence_generation = 7;
    workflows.app.selected_evidence_ui.selected_evidence = SelectedEvidenceState::Ready {
        generation: 7,
        report: Box::new(report),
        hasheous: HasheousState::Idle,
    };
    frame(&context, &mut workflows, &path, Vec::new());
    let output = frame(&context, &mut workflows, &path, Vec::new());
    assert!(matches!(
        workflows.launch_input(),
        launch_readiness_page::LaunchReadinessInput::IdentityUnknown
    ));
    let retry = output
        .shapes
        .iter()
        .find_map(|s| text_center(&s.shape, "Retry readiness check"))
        .expect("a cached unresolved identity must offer the existing readiness retry");

    let current = if repair { valid_cartridge() } else { invalid };
    std::fs::write(&path, &current).unwrap();
    for pressed in [true, false] {
        frame(
            &context,
            &mut workflows,
            &path,
            vec![
                egui::Event::PointerMoved(retry),
                egui::Event::PointerButton {
                    pos: retry,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(
        workflows
            .app
            .selected_evidence_ui
            .selected_evidence_generation
            > 7,
        "the real retry button must dispatch the existing generation-guarded worker"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        workflows.app.poll_selected_evidence();
        if let SelectedEvidenceState::Ready { report, .. } =
            &workflows.app.selected_evidence_ui.selected_evidence
        {
            assert_eq!(
                report
                    .game_identity_report
                    .verified_loose_rom_sha256()
                    .is_some(),
                repair
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "readiness retry did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(path).unwrap(), current);
}

#[test]
fn a_repaired_cartridge_can_be_rechecked_from_unresolved_blue_play() {
    retry_current_bytes(true);
}

#[test]
fn retrying_unresolved_blue_play_never_promotes_invalid_cartridge_bytes() {
    retry_current_bytes(false);
}
