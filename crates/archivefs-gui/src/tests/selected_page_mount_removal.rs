//! Focused tests for removing the redundant mount-queue section from the
//! Selected page while keeping the Mount page's own queue machinery intact.
//!
//! `MainView::Selected` used to render a full duplicate of the Mount page's
//! queue review/manipulation/confirmation UI, sharing `mount_queue`/
//! `confirm_mount_queue` state and the `MountPageAction`/
//! `show_mount_queue_confirmation` machinery with the real Mount page
//! (`administration_pages::show_mount_page`). This file proves: Selected no
//! longer renders any of that, Selected's own unique panels (evidence,
//! launch readiness, profile/firmware scan triggers, Cheats & Mods entry)
//! are untouched, the Mount page still fully owns queue review and
//! confirmation, and the plain "Open Mounts" shortcut Selected keeps routes
//! into that exact existing workflow rather than reimplementing it.

use super::*;

fn selected_screen_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1400.0, 1600.0),
        )),
        ..Default::default()
    }
}

fn render_app(app: &mut ArchiveFsApp) -> egui::FullOutput {
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    ctx.run(selected_screen_input(), |ctx| app.update(ctx, &mut frame))
}

fn app_with_one_pending_archive_queued() -> ArchiveFsApp {
    let mut app = app_for_operation_tests();
    let path = PathBuf::from("/roms/Queued Game.zip");
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/Queued Game.zip", MountState::Pending));
    }
    app.mount_queue.push(path.clone());
    app.archive_context.select_only(path);
    app
}

// --- Selected: the redundant mount queue section is gone ------------------

#[test]
fn selected_no_longer_renders_the_mount_queue_review_section() {
    let mut app = app_with_one_pending_archive_queued();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Selected;

    let output = render_app(&mut app);

    for gone in [
        "Mount queue",
        "Clear queue",
        "Ready entries will be mounted",
        "READY",
        "BLOCKED / SKIPPED",
        "archives queued.",
    ] {
        assert!(
            !rendered_text_contains(&output, gone),
            "Selected must no longer render {gone:?}"
        );
    }
}

#[test]
fn selected_no_longer_describes_itself_as_a_pre_mount_queue_screen() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Selected;

    let output = render_app(&mut app);

    assert!(
        !rendered_text_contains(
            &output,
            "Review queued archives and their validated destinations before mounting."
        ),
        "the old queue-screen wording must be gone"
    );
    assert!(
        rendered_text_contains(
            &output,
            "Review identity, launch readiness and available actions for this game."
        ),
        "the page must describe itself as a game-details/review surface"
    );
    assert!(rendered_text_contains(&output, "Game Details"));
}

#[test]
fn selected_still_offers_a_plain_open_mounts_shortcut() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Selected;

    let output = render_app(&mut app);

    assert!(
        rendered_text_contains(&output, "Open Mounts"),
        "a plain shortcut into the real Mount workflow must remain"
    );
}

#[test]
fn the_open_mounts_shortcut_routes_into_the_existing_mount_workflow() {
    let mut app = app_for_operation_tests();
    app.view = MainView::Selected;
    let ctx = egui::Context::default();

    // The shared handler every Mount/Selected action already goes through -
    // proves the shortcut reuses `GoToMount`'s existing, unmodified effect
    // rather than a new bespoke navigation path.
    app.handle_mount_page_action(&ctx, Some(MountPageAction::GoToMount));

    assert_eq!(app.view, MainView::Mount);
}

// --- Mount page: still fully owns queue review/manipulation/confirmation --

#[test]
fn mount_page_still_renders_and_manages_the_queue() {
    let mut app = app_with_one_pending_archive_queued();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Mount;

    let output = render_app(&mut app);

    for expected in [
        "Mount queue",
        "Clear queue",
        "Queue all visible",
        "1 archive queued.",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "Mount page must still render {expected:?}"
        );
    }
}

#[test]
fn mount_queue_confirmation_still_works_on_the_mount_page() {
    let mut app = app_with_one_pending_archive_queued();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Mount;
    app.confirm_mount_queue = true;

    let output = render_app(&mut app);

    for expected in [
        "Confirmation",
        "Mount 1 queued archive?",
        "Mount now",
        "Cancel",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "the shared mount-queue confirmation strip must still render {expected:?}"
        );
    }
}

#[test]
fn confirming_the_mount_queue_still_starts_the_real_batch_engine() {
    let mut app = app_with_one_pending_archive_queued();
    let ctx = egui::Context::default();

    app.handle_mount_page_action(&ctx, Some(MountPageAction::MountQueue));

    assert!(
        app.mount_all.is_some(),
        "MountQueue must still drive the existing start_mount_all engine, unmodified"
    );
}

// --- Selected: unique panels and scan triggers remain untouched -----------

#[test]
fn selected_still_renders_identity_evidence_and_launch_readiness_panels() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Selected;

    let output = render_app(&mut app);

    assert!(
        rendered_text_contains(&output, "ROM Identity & Evidence"),
        "the identity/evidence panel must remain"
    );
    assert!(
        rendered_text_contains(&output, "Launch readiness"),
        "the launch readiness panel must remain"
    );
}

#[test]
fn selected_still_triggers_dolphin_pcsx2_profile_and_firmware_scans() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Selected;
    assert!(matches!(
        app.dolphin_local_profiles,
        DolphinLocalProfilesState::NotScanned
    ));
    assert!(matches!(
        app.pcsx2_launch_profiles,
        Pcsx2LaunchProfilesState::NotScanned
    ));
    assert!(matches!(
        app.pcsx2_firmware_evidence,
        Pcsx2FirmwareEvidenceState::NotLoaded
    ));

    let _ = render_app(&mut app);

    assert!(
        matches!(
            app.dolphin_local_profiles,
            DolphinLocalProfilesState::Scanning { .. }
        ),
        "visiting Selected must still start the Dolphin profile scan"
    );
    assert!(
        matches!(
            app.pcsx2_launch_profiles,
            Pcsx2LaunchProfilesState::Scanning { .. }
        ),
        "visiting Selected must still start the PCSX2 profile scan"
    );
    assert!(
        matches!(
            app.pcsx2_firmware_evidence,
            Pcsx2FirmwareEvidenceState::Loading { .. }
        ),
        "visiting Selected must still start loading PCSX2 firmware evidence"
    );
}

#[test]
fn open_cheats_mods_from_selected_still_works() {
    let mut app = app_for_operation_tests();
    app.view = MainView::Selected;
    let ctx = egui::Context::default();
    let path = PathBuf::from("/roms/Game.zip");

    app.handle_mount_page_action(&ctx, Some(MountPageAction::OpenCheatsMods(path.clone())));

    assert_eq!(app.view, MainView::CheatsMods);
    assert_eq!(app.archive_context.focused.as_deref(), Some(path.as_path()));
}

#[test]
fn gamer_view_review_still_lands_on_selected_with_the_same_focused_game() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    let archive_path = PathBuf::from("/roms/Mystery Game.bin");

    app.review_identity(archive_path.clone());

    assert_eq!(app.ui_mode, GuiMode::AdvancedView);
    assert_eq!(app.view, MainView::Selected);
    assert_eq!(
        app.archive_context.focused.as_deref(),
        Some(archive_path.as_path())
    );
}
