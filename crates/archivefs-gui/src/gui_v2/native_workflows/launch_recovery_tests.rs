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

fn catalogue_row(path: &Path, platform: Option<&str>) -> archivefs_core::PersistedArchive {
    archivefs_core::PersistedArchive {
        id: 1,
        source_folder_id: 1,
        relative_path: "dual.gb".into(),
        absolute_path: path.to_path_buf(),
        archive_kind: "direct_game_image".into(),
        display_name: "Synthetic dual-mode cartridge".into(),
        normalized_name: "synthetic".into(),
        size_bytes: None,
        modified_time_unix_seconds: None,
        platform: platform.map(str::to_owned),
        platform_source: platform.map(|_| "manual".into()),
        last_known_health: "unknown".into(),
        last_seen_at: String::new(),
        last_verified_missing_at: None,
        identity_report: None,
    }
}

fn cached_dual_cartridge(context: &egui::Context, path: &Path) -> NativeWorkflows {
    let mut bytes = valid_cartridge();
    bytes[0x143] = 0x80;
    bytes[0x14D] = archivefs_core::gb_header_evidence::compute_header_checksum(&bytes).unwrap();
    std::fs::write(path, bytes).unwrap();
    let report =
        selected_evidence_page::gather_selected_evidence_fast(path, Some("Game Boy Color"))
            .unwrap();
    assert!(
        report
            .game_identity_report
            .verified_loose_rom_sha256()
            .is_some()
    );
    let mut workflows = NativeWorkflows::new(context.clone());
    workflows.note_catalogue_row(&catalogue_row(path, Some("Game Boy Color")));
    workflows.select(path);
    workflows
        .app
        .selected_evidence_ui
        .selected_evidence_generation = 7;
    workflows.app.selected_evidence_ui.selected_evidence = SelectedEvidenceState::Ready {
        generation: 7,
        report: Box::new(report),
        hasheous: HasheousState::Idle,
    };
    assert!(matches!(
        workflows.launch_input(),
        launch_readiness_page::LaunchReadinessInput::Plan { .. }
    ));
    workflows
}

fn check_catalogued_platform_change(platform: Option<&str>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dual.gb");
    let context = egui::Context::default();
    let mut workflows = cached_dual_cartridge(&context, &path);
    let original = std::fs::read(&path).unwrap();
    workflows.note_catalogue_row(&catalogue_row(&path, platform));
    assert!(
        !matches!(
            workflows.launch_input(),
            launch_readiness_page::LaunchReadinessInput::Plan { .. }
        ),
        "a changed or cleared system choice cannot reuse the old resolved launch identity"
    );
    assert!(
        workflows
            .app
            .selected_evidence_ui
            .selected_evidence_generation
            > 7
    );
    frame(&context, &mut workflows, &path, Vec::new());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        workflows.app.poll_selected_evidence();
        if let SelectedEvidenceState::Ready { report, .. } =
            &workflows.app.selected_evidence_ui.selected_evidence
        {
            if platform.is_some() {
                assert_eq!(
                    report.game_identity_report.platform,
                    archivefs_core::game_identity::IdentityPlatform::GameBoy
                );
                assert!(
                    report
                        .game_identity_report
                        .verified_loose_rom_sha256()
                        .is_some()
                );
            } else {
                assert_eq!(
                    report.game_identity_report.verified_loose_rom_sha256(),
                    None,
                    "clearing a deliberate choice leaves this ambiguous cartridge unassigned"
                );
            }
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn changing_a_catalogued_platform_invalidates_the_old_launch_authority() {
    check_catalogued_platform_change(Some("Game Boy"));
}

#[test]
fn clearing_a_catalogued_platform_never_keeps_an_ambiguous_cartridge_ready() {
    check_catalogued_platform_change(None);
}

#[test]
fn unchanged_catalogued_platform_does_not_restart_the_evidence_worker() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dual.gb");
    let context = egui::Context::default();
    let mut workflows = cached_dual_cartridge(&context, &path);
    workflows.note_catalogue_row(&catalogue_row(&path, Some("Game Boy Color")));
    assert_eq!(
        workflows
            .app
            .selected_evidence_ui
            .selected_evidence_generation,
        7
    );
    assert!(matches!(
        workflows.launch_input(),
        launch_readiness_page::LaunchReadinessInput::Plan { .. }
    ));
}

#[test]
fn blue_launch_and_details_start_cassette_verification() {
    for details in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.cas");
        std::fs::write(&path, b"synthetic unverified tape").unwrap();
        let context = egui::Context::default();
        let mut workflows = NativeWorkflows::new(context.clone());
        workflows.app = crate::tests::app_with_cheat_game(path.to_str().unwrap(), "Unknown");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let _ = context.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if details {
                        workflows.show_readiness_summary(
                            ui,
                            1,
                            &path,
                            None,
                            false,
                            &mut Activity::default(),
                        );
                    } else {
                        workflows.show_launch(ui, 1, &path, None, &mut Activity::default());
                    }
                });
            });
            if let Some(checked) = workflows
                .app
                .emulator_readiness
                .xroar_cassette
                .context(Some(&path))
            {
                assert!(
                    checked.result.is_err(),
                    "synthetic bytes must remain unverified"
                );
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Blue never started its cassette worker (details={details})"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[test]
fn blue_and_gold_dosbox_folder_review_is_reachable_without_release_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("homebrew.zip");
    std::fs::write(&path, b"synthetic catalogue selection").unwrap();
    std::fs::write(directory.path().join("GAME.EXE"), b"MZ").unwrap();
    std::fs::write(
        directory.path().join("dosbox.conf"),
        "[dosbox]\nmemsize=16\n[autoexec]\nmount c .\nc:\ngame.exe\nexit\n",
    )
    .unwrap();
    let context = egui::Context::default();
    let mut workflows = NativeWorkflows::new(context.clone());
    workflows.app = crate::tests::app_with_cheat_game(path.to_str().unwrap(), "Unknown");
    workflows.select(&path);
    let report = crate::selected_evidence_page::gather_selected_evidence_with_platform(
        &path,
        Some("DOS"),
        None,
    )
    .unwrap();
    assert!(matches!(
        archivefs_core::launch::canonical_identity_from_game_report(&report.game_identity_report).0,
        archivefs_core::launch::CanonicalIdentityStatus::Unknown
    ));
    workflows.app.selected_evidence_ui.selected_evidence = SelectedEvidenceState::Ready {
        generation: 0,
        report: Box::new(report),
        hasheous: HasheousState::Idle,
    };
    assert!(matches!(
        workflows.launch_input(),
        launch_readiness_page::LaunchReadinessInput::DosBoxReview
    ));
}

#[test]
fn changing_blue_details_selection_clears_dosbox_approval_before_returning_to_launch() {
    let directory = tempfile::tempdir().unwrap();
    let a = directory.path().join("a.zip");
    let b = directory.path().join("b.zip");
    std::fs::write(directory.path().join("GAME.EXE"), b"MZ").unwrap();
    std::fs::write(
        directory.path().join("dosbox.conf"),
        "[dosbox]\nmemsize=16\n[autoexec]\nmount c .\nc:\ngame.exe\nexit\n",
    )
    .unwrap();
    let review = archivefs_core::launch::review_dosbox_game_directory(directory.path()).unwrap();
    let target = archivefs_core::launch::UserApprovedDosBoxTarget::approve(
        &review,
        directory.path(),
        &directory.path().join("GAME.EXE"),
    )
    .unwrap();
    let mut workflows = NativeWorkflows::new(egui::Context::default());
    workflows.select(&a);
    assert!(
        workflows
            .app
            .launch_standalone
            .dosbox_approval_for_test(Some(target))
    );
    workflows.select(&b);
    assert!(
        !workflows
            .app
            .launch_standalone
            .dosbox_approval_for_test(None)
    );
    workflows.select(&a);
    assert!(
        !workflows
            .app
            .launch_standalone
            .dosbox_approval_for_test(None)
    );
}
