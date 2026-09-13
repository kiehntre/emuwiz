use super::*;
use archivefs_core::launch::*;

fn input(firmware: FirmwareReadiness, ready: bool) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan: LaunchPlan {
            platform_id: Some("ps2".into()),
            game_key: Some("SLUS-TEST".into()),
            candidates: vec![LaunchCandidate {
                target: LaunchTarget::Standalone {
                    adapter_id: "pcsx2",
                    profile_id: "test".into(),
                    profile_path: None,
                },
                content: LaunchContentRef {
                    kind: None,
                    container: None,
                    resolved_path: Some("/disposable/game.iso".into()),
                    requires_mount: false,
                    provenance: "fixture".into(),
                },
                firmware,
                blockers: Vec::new(),
                warnings: Vec::new(),
                readiness: if ready {
                    LaunchReadiness::Ready
                } else {
                    LaunchReadiness::Blocked
                },
                preference: CandidatePreference::SoleEligible,
            }],
            summary: LaunchPlanSummary {
                candidates: 1,
                ready: usize::from(ready),
                ready_with_warnings: 0,
                blocked: usize::from(!ready),
            },
            media_topology: None,
        },
        retroarch: None,
        retroarch_scanned: true,
        standalone_scans_complete: true,
        dolphin: None,
        pcsx2: None,
        duckstation: None,
        ppsspp: None,
        rpcs3: None,
        xemu: None,
        xenia: None,
        amiga_whdload: None,
    }
}

#[test]
fn attention_missing_bios_blocks_and_resolution_uses_current_plan() {
    let path = Some(std::path::Path::new("/disposable/game.iso"));
    let missing = launch_attention(&input(FirmwareReadiness::Missing, false), path);
    let page = missing.page(&AttentionFilters::default());
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].severity, AttentionSeverity::Blocking);
    assert!(page.items[0].title.contains("BIOS"));
    assert_eq!(
        destination_view(page.items[0].destination),
        MainView::EmulatorSetup
    );
    assert_eq!(
        launch_attention(&input(FirmwareReadiness::Verified, true), path)
            .page(&AttentionFilters::default())
            .total,
        0
    );
}

#[test]
fn attention_unchecked_launch_is_not_a_failure() {
    let snapshot = launch_attention(
        &LaunchReadinessInput::EvidenceNotLoaded,
        Some(std::path::Path::new("/absent")),
    );
    assert_eq!(snapshot.items().count(), 0);
    assert!(!snapshot.coverage_notes.is_empty());
}

#[test]
fn attention_partly_scanned_empty_launch_plan_is_not_a_missing_emulator() {
    let mut input = input(FirmwareReadiness::Unknown, false);
    if let LaunchReadinessInput::Plan {
        plan,
        standalone_scans_complete,
        ..
    } = &mut input
    {
        plan.candidates.clear();
        plan.summary = LaunchPlanSummary::default();
        *standalone_scans_complete = false;
    }
    let snapshot = launch_attention(&input, Some(std::path::Path::new("/disposable/game.iso")));
    assert_eq!(snapshot.items().count(), 0);
    assert!(!snapshot.coverage_notes.is_empty());
}

#[test]
fn attention_routes_use_existing_views() {
    assert_eq!(
        destination_view(AttentionDestination::DatReview),
        MainView::IdentifyRename
    );
    assert_eq!(
        destination_view(AttentionDestination::Duplicates),
        MainView::Duplicates
    );
    assert_eq!(
        destination_view(AttentionDestination::Romm),
        MainView::CanonicalOrganisation
    );
    assert_eq!(
        destination_view(AttentionDestination::EsDe),
        MainView::CanonicalOrganisation
    );
    assert_eq!(
        destination_view(AttentionDestination::History),
        MainView::HistoryLogs
    );
    assert_eq!(
        destination_view(AttentionDestination::CheatsMods),
        MainView::CheatsMods
    );
}

#[test]
fn attention_rendering_is_paged_and_mutation_free() {
    let mut workspace = AttentionWorkspace {
        loaded: true,
        ..Default::default()
    };
    for index in 0..120 {
        workspace.snapshot.insert(AttentionItem::new(
            format!("item-{index}"),
            AttentionCategory::Unsupported,
            AttentionSeverity::Warning,
            "Unsupported file".into(),
            AttentionDestination::Discovery,
        ));
    }
    let before: Vec<_> = workspace.snapshot.items().cloned().collect();
    let context = egui::Context::default();
    let started = Instant::now();
    for _ in 0..3 {
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                assert!(show_needs_attention_page(ui, &mut workspace).is_none())
            });
        });
    }
    assert_eq!(
        workspace.snapshot.page(&workspace.filters).items.len(),
        ATTENTION_PAGE_SIZE
    );
    assert_eq!(
        before,
        workspace.snapshot.items().cloned().collect::<Vec<_>>()
    );
    assert!(workspace.receiver.is_none());
    assert!(workspace.last_started.is_none());
    eprintln!(
        "ATTENTION_GUI three_frames_ms={:.3} summaries=120 page_size={ATTENTION_PAGE_SIZE}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

#[test]
fn attention_empty_render_does_not_start_loading_or_checks() {
    let mut workspace = AttentionWorkspace {
        loaded: true,
        ..Default::default()
    };
    let _ = egui::Context::default().run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_needs_attention_page(ui, &mut workspace);
        });
    });
    assert!(workspace.receiver.is_none());
    assert_eq!(workspace.snapshot.counts(), [0; 4]);
}

#[test]
fn attention_background_replacement_resolves_without_navigation_or_manual_flags() {
    let mut app = crate::tests::app_for_operation_tests();
    app.needs_attention.last_started = Some(Instant::now());
    app.needs_attention.generation = Some(app.database_generation.0);
    let mut failed = AttentionSnapshot::default();
    failed.insert(AttentionItem::new(
        "source:1".into(),
        AttentionCategory::Sources,
        AttentionSeverity::ActionNeeded,
        "Scan failed".into(),
        AttentionDestination::Sources,
    ));
    let (sender, receiver) = mpsc::channel();
    app.needs_attention.receiver = Some(receiver);
    sender.send(failed).unwrap();
    app.poll_needs_attention(&egui::Context::default());
    assert!(
        app.needs_attention
            .snapshot
            .items()
            .any(|i| i.id == "source:1")
    );
    let (sender, receiver) = mpsc::channel();
    app.needs_attention.receiver = Some(receiver);
    sender.send(AttentionSnapshot::default()).unwrap();
    app.poll_needs_attention(&egui::Context::default());
    assert!(
        !app.needs_attention
            .snapshot
            .items()
            .any(|i| i.id == "source:1")
    );
    assert!(app.needs_attention.receiver.is_none());
}
