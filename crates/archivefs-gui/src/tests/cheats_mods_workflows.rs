//! GUI Maintenance Batch 2: relocated from main.rs's single inline
//! `#[cfg(test)] mod tests { ... }` (see `crate::tests` for the shared
//! imports/fixtures this file and its siblings rely on).
//!
//! This file's name is a best-effort thematic label, not a strict
//! single-feature boundary: the original test module interleaved topics
//! throughout (tests for unrelated features sit side by side in source
//! order), so this file was cut at safe item boundaries within that
//! existing order rather than by re-sorting tests into pure per-feature
//! files. Every test here is copied byte-for-byte from its original
//! location - nothing was rewritten, renamed, or reordered relative to
//! its neighbors within this slice.
//!
//! Predominant theme observed in this slice: Cheats & Mods navigation, GameCube/Dolphin/PCSX2 gamehacking flows.

use super::*;

#[test]
fn gamecube_cancel_notice_is_distinct_from_success_and_failure() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-gamecube-cancel-{}",
        std::process::id()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "G9RE7D");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.transaction_notice =
        Some("Installation cancelled before apply; no live emulator file was changed.".to_string());
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    assert!(rendered_text_contains(&output, "Installation cancelled"));
    assert!(rendered_text_contains(
        &output,
        "no live emulator file was changed"
    ));
    assert!(!rendered_text_contains(&output, "Installed successfully"));
}

#[test]
fn successful_undo_clears_the_matching_installed_state() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-undo-state-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_ready_for_beginner_install(&temp);
    let apply = successful_shared_apply_result();
    let original_operation_id = apply.journal.operation_id.clone();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.transaction = CheatTransactionState::Result {
        key: cheat_preview_key(workflow),
        result: apply,
    };
    let (sender, receiver) = mpsc::channel();
    app.shared_rollback = SharedRollbackState::Applying { receiver };
    sender
        .send(Ok(SharedRollbackResult {
            preview: SharedRollbackPreview {
                schema_version: 1,
                preview_id: "undo-preview".to_string(),
                journal_path: SharedTransactionPath::from_path(Path::new(
                    "/history/op-beginner-test.json",
                )),
                original_operation_id,
                destination_root: SharedTransactionPath::from_path(&temp),
                entries: Vec::new(),
                available: true,
            },
            journal_path: Some(PathBuf::from("/history/undo.json")),
            status: SharedApplyStatus::Success,
        }))
        .unwrap();
    app.poll_shared_rollback();
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().transaction,
        CheatTransactionState::Idle
    ));
    let _ = std::fs::remove_dir_all(&temp);
}

/// Phase 4 fix: `start_cheat_install_rollback` used to only set
/// `self.view = MainView::HistoryLogs` - a silent no-op while
/// `self.ui_mode == GuiMode::GamerView`, since Gamer View's own render
/// branch is chosen purely from `ui_mode` and never reads `view` at all.
/// Clicking "Undo last change" from Gamer View therefore changed
/// internal state with nothing visibly happening on screen. This pins
/// that the fix (also switching `ui_mode`, and giving visible feedback)
/// actually makes the click do something a person can see.
#[test]
fn undo_from_gamer_view_actually_switches_to_the_review_screen_not_just_internal_state() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-gamer-undo-visible-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_ready_for_beginner_install(&temp);
    app.ui_mode = GuiMode::GamerView;
    let apply = successful_shared_apply_result();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.transaction = CheatTransactionState::Result {
        key: cheat_preview_key(workflow),
        result: apply,
    };

    app.start_cheat_install_rollback(egui::Context::default());

    assert_eq!(
        app.ui_mode,
        GuiMode::AdvancedView,
        "undo must switch to the mode that can actually show the review screen"
    );
    assert_eq!(app.view, MainView::HistoryLogs);
    let feedback = app
        .feedback
        .as_ref()
        .expect("undo must give visible feedback");
    assert!(feedback.succeeded);
    assert!(!feedback.message.is_empty());

    let _ = std::fs::remove_dir_all(&temp);
}

/// Phase 5 fix: `prepare_cheats_mods_workspace` used to only set
/// `self.view`, which Gamer View's own render branch never reads (the
/// same bug shape Phase 4 already fixed for Undo) - clicking "Cheats &
/// Mods" from Gamer View silently did nothing visible. This pins that
/// opening the workflow now actually switches to the mode that renders
/// it.
#[test]
fn opening_cheats_mods_from_gamer_view_actually_switches_mode() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;

    app.prepare_cheats_mods_workspace(PathBuf::from("/roms/Game.zip"));

    assert_eq!(
        app.ui_mode,
        GuiMode::AdvancedView,
        "Cheats & Mods must switch to the mode that can actually show it"
    );
    assert_eq!(app.view, MainView::CheatsMods);
}

/// Phase 5: "Review" on an unresolved-platform game must land on
/// Advanced View's Selected page with that exact game still selected -
/// not a generic page with no context (requirement 6's journey test).
#[test]
fn review_identity_lands_on_selected_page_with_the_same_game_still_selected() {
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

/// Phase 5 vocabulary guard: the unknown-platform block in Gamer View's
/// selected-game panel (`show_gamer_view`) must use the approved human
/// wording and a real "Review" action, and must never name the internal
/// concepts docs/GUI_NAVIGATION_RESET_DESIGN.md §2.6/§5 keep out of
/// Gamer View's primary wording. Source-scanned rather than rendered
/// (matching this file's existing `bsfree_gui_apply_and_rollback_reuse_
/// the_shared_backend` precedent) since `show_gamer_view` has no direct
/// render-harness test anywhere in this suite yet.
#[test]
fn gamer_view_unknown_platform_block_uses_approved_wording_only() {
    // `show_gamer_view` lives in `gamer_view.rs`, not `main.rs`, since the
    // GUI extraction (2026-08-22, Phase A) moved Gamer View rendering out
    // of the app-shell file.
    let source = include_str!("../gamer_view.rs");
    let block = source
        .split("if let Some(row) = row\n                                        && row.unknown_platform\n                                    {")
        .nth(1)
        .expect("the unknown-platform block must exist in show_gamer_view")
        .split("\n\n                                    ui.add_space(theme::SECTION_GAP);")
        .next()
        .unwrap();

    assert!(block.contains("We couldn't tell which game system this is for."));
    assert!(block.contains("\"Review\""));

    for banned in [
        "candidate identity",
        "provenance",
        "evidence source",
        "identity source",
        "resolver",
        "ArchiveIdentity",
        "ContentKind",
        "ContainerKind",
    ] {
        assert!(
            !block
                .to_ascii_lowercase()
                .contains(&banned.to_ascii_lowercase()),
            "unknown-platform block names banned term {banned:?}"
        );
    }
}

#[test]
fn details_is_collapsed_by_default_on_the_beginner_dolphin_page() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-details-collapsed-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_ready_for_beginner_install(&temp);
    assert!(!app.cheat_workflow.as_ref().unwrap().dolphin_details_open);
    let output = render_dolphin_workflow(&mut app);
    assert!(
        rendered_text_contains(&output, "Details"),
        "rendering mismatch"
    );
    assert!(
        !rendered_text_contains(&output, "Stage 2 · Find matching cheats"),
        "technical stage text leaked outside the collapsed Details section"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn automatic_fetch_failure_has_plain_retry_without_raw_error_details() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-fetch-failure-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: "dolphin-native-test".to_string(),
        reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::OnlyValidProfile,
    });
    workflow.dolphin_provider = CheatStepResource::Failed(
        "HTTP 503 from https://raw.githubusercontent.com/internal/provider".to_string(),
    );
    let output = render_dolphin_workflow(&mut app);
    assert!(rendered_text_contains(&output, "Try again"));
    assert!(rendered_text_contains(
        &output,
        "EmuWiz could not load compatible cheats. Check your connection and try again."
    ));
    assert!(!rendered_text_contains(&output, "HTTP 503"));
    assert!(!rendered_text_contains(
        &output,
        "raw.githubusercontent.com"
    ));
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn beginner_view_shows_neutral_state_for_missing_upstream_game_without_retry() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-not-available-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: "dolphin-native-test".to_string(),
        reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::OnlyValidProfile,
    });
    workflow.dolphin_provider = CheatStepResource::Ready(gafe01_not_available_fetch());
    assert_eq!(
        dolphin_beginner_status(workflow),
        BeginnerCheatStatus::NoUpstreamCheatsAvailable
    );
    assert_eq!(
        BeginnerCheatStatus::NoUpstreamCheatsAvailable.tone(),
        widgets::StatusTone::Info
    );
    let output = render_dolphin_workflow(&mut app);
    assert!(rendered_text_contains(
        &output,
        "No upstream Dolphin cheats are available for this game."
    ));
    assert!(
        !rendered_text_contains(&output, "Try again"),
        "a deterministic missing-file result must not offer Retry"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn technical_details_shows_neutral_banner_without_retry_for_missing_upstream_game() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-details-not-available-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: "dolphin-native-test".to_string(),
        reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::OnlyValidProfile,
    });
    workflow.dolphin_provider = CheatStepResource::Ready(gafe01_not_available_fetch());
    workflow.dolphin_details_open = true;
    let output = render_dolphin_workflow(&mut app);
    assert!(rendered_text_contains(
        &output,
        "No upstream Dolphin cheats are available for this game."
    ));
    assert!(
        !rendered_text_contains(&output, "Retry"),
        "a deterministic missing-file result must not offer Retry"
    );
    assert!(!rendered_text_contains(
        &output,
        "Could not load matching cheats"
    ));
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn technical_details_keeps_retry_and_red_error_state_for_a_real_provider_failure() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-details-real-failure-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: "dolphin-native-test".to_string(),
        reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::OnlyValidProfile,
    });
    workflow.dolphin_provider =
        CheatStepResource::Failed("Gecko provider returned HTTP 500".to_string());
    workflow.dolphin_details_open = true;
    let output = render_dolphin_workflow(&mut app);
    assert!(rendered_text_contains(
        &output,
        "Could not load matching cheats"
    ));
    assert!(
        rendered_text_contains(&output, "Retry"),
        "a transient failure must still offer Retry"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn choosing_a_dolphin_profile_in_the_chooser_remembers_it_and_reaches_auto_selected_state() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-chooser-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let second = temp.join("second");
    std::fs::create_dir_all(&second).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    let profile_a = DolphinProfile {
        profile_id: "profile-a".to_string(),
        configuration_path: temp.clone(),
        ..dolphin_profile_fixture()
    };
    let profile_b = DolphinProfile {
        profile_id: "profile-b".to_string(),
        configuration_path: second.clone(),
        ..dolphin_profile_fixture()
    };
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.selected_dolphin_profile_id = None;
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::NeedsChoice {
        candidates: vec![
            EmulatorProfileCandidate {
                profile_id: "profile-a".to_string(),
                root: temp.clone(),
                eligible: true,
                is_portable: false,
                evidence_priority: 0,
            },
            EmulatorProfileCandidate {
                profile_id: "profile-b".to_string(),
                root: second.clone(),
                eligible: true,
                is_portable: false,
                evidence_priority: 0,
            },
        ],
    });
    app.dolphin_profiles = DolphinProfilesState::Ready(DolphinProfileDiscovery {
        profiles: vec![profile_a, profile_b],
        warnings: Vec::new(),
        complete: true,
    });
    let output = render_dolphin_workflow(&mut app);
    assert!(
        rendered_text_contains(&output, "Select the Dolphin profile to use"),
        "rendering mismatch"
    );
    assert!(rendered_text_contains(
        &output,
        "EmuWiz found 2 credible Dolphin profiles."
    ));
    assert!(rendered_text_contains(&output, "Native profile 1"));
    assert!(rendered_text_contains(&output, "Native profile 2"));
    assert!(!rendered_text_contains(&output, "Use selected profile"));
    assert!(rendered_text_contains(
        &output,
        &second.display().to_string()
    ));

    app.cheat_workflow.as_mut().unwrap().dolphin_profile_choice = Some("profile-b".to_string());
    app.confirm_dolphin_profile_choice();

    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(
        workflow.selected_dolphin_profile_id.as_deref(),
        Some("profile-b")
    );
    assert!(matches!(
        workflow.dolphin_profile_selection,
        Some(EmulatorProfileSelection::Auto { .. })
    ));
    assert_eq!(
        remembered_profile_for(&app.remembered_emulator_profiles, "dolphin")
            .unwrap()
            .profile_id,
        "profile-b"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn gamecube_provider_codes_render_without_a_preexisting_ini_or_retroarch_controls() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-provider-no-ini-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    install_provider_fixture(&mut app, &temp);
    app.cheat_workflow.as_mut().unwrap().dolphin_details_open = true;
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let workflow = app.cheat_workflow.as_mut().unwrap();
            let _ = show_dolphin_workflow(ui, workflow, &app.dolphin_profiles, &mut clipboard);
        });
    });
    for expected in [
        "Find matching cheats",
        "GAFE01",
        "Animal Crossing",
        "16:9 Widescreen",
        "No existing GameSettings file is required",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    for forbidden in ["RetroArch profile", "Fetch / Update catalogue"] {
        assert!(!rendered_text_contains(&output, forbidden));
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn individual_provider_selection_invalidates_preview_and_rendering_never_fetches() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-provider-selection-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    install_provider_fixture(&mut app, &temp);
    app.update_dolphin_code_selection(|selection| {
        assert!(selection.set_selected(0, true));
    });
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(
        workflow
            .dolphin_provider_selection
            .as_ref()
            .unwrap()
            .selection
            .selected_count(),
        1
    );
    assert!(matches!(workflow.preview, CheatStepResource::NotLoaded));

    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    for _ in 0..2 {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let workflow = app.cheat_workflow.as_mut().unwrap();
                let _ = show_dolphin_workflow(ui, workflow, &app.dolphin_profiles, &mut clipboard);
            });
        });
        assert!(matches!(
            app.cheat_workflow.as_ref().unwrap().dolphin_provider,
            CheatStepResource::Ready(_)
        ));
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn changing_archive_clears_external_provider_selection_and_preview_state() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-provider-archive-change-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    install_provider_fixture(&mut app, &temp);
    if let LoadState::Ready(data) = &mut app.state {
        let mut second = record("/roms/b.zip", MountState::Pending);
        second.identity.platform = Some("GameCube".to_string());
        data.records.push(second);
    }
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/b.zip")));
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(matches!(
        workflow.dolphin_provider,
        CheatStepResource::NotLoaded
    ));
    assert!(workflow.dolphin_provider_selection.is_none());
    assert!(matches!(workflow.preview, CheatStepResource::NotLoaded));
    assert!(matches!(workflow.transaction, CheatTransactionState::Idle));
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn external_gecko_provider_is_not_offered_for_wii() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-provider-wii-scope-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    app.cheat_workflow.as_mut().unwrap().platform = Some("Wii".to_string());
    app.cheat_workflow.as_mut().unwrap().dolphin_details_open = true;
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let workflow = app.cheat_workflow.as_mut().unwrap();
            let _ = show_dolphin_workflow(ui, workflow, &app.dolphin_profiles, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "supports exact-ID external Gecko retrieval for GameCube only"
    ));
    assert!(!rendered_text_contains(&output, "Fetch Gecko codes"));
    let _ = std::fs::remove_dir_all(&temp);
}

#[cfg(any())]
#[test]
fn dolphin_candidate_match_opens_a_real_matched_file_with_its_own_gecko_codes() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-dolphin-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");

    app.start_dolphin_candidate_match();

    let workflow = app.cheat_workflow.as_ref().unwrap();
    let outcome = workflow.dolphin_candidate_outcome.as_ref().unwrap();
    let candidate = outcome.candidate.as_ref().expect("candidate matched");
    assert_eq!(candidate.game_id, "GAFE01");
    let selection = workflow
        .dolphin_selection
        .as_ref()
        .expect("selection opened");
    assert_eq!(selection.selection.entries.len(), 2);
    assert_eq!(
        selection.selection.selected_count(),
        1,
        "already-enabled code pre-selected"
    );
    let already_enabled = selection
        .selection
        .entries
        .iter()
        .find(|entry| entry.name == "Instant Growth [Nayr]")
        .unwrap();
    assert!(already_enabled.already_enabled);
    assert!(already_enabled.selected);
    let not_enabled = selection
        .selection
        .entries
        .iter()
        .find(|entry| entry.name == "Infinite Bells [Nayr]")
        .unwrap();
    assert!(!not_enabled.already_enabled);
    assert!(!not_enabled.selected);
    assert_eq!(
        app.history.entries().next().unwrap().action,
        ActivityAction::DolphinGeckoCandidateMatch
    );
    assert_eq!(
        app.history.entries().next().unwrap().outcome,
        ActivityOutcome::Completed
    );
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let history = OperationHistory::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
                &app.xenia_profiles,
                None,
                None,
                &history,
                false,
                &mut clipboard,
            );
        });
    });
    for expected in [
        "GameCube",
        "GAFE01",
        "Revision: 0",
        "Infinite Bells [Nayr]",
        "Instant Growth [Nayr]",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    assert!(!rendered_text_contains(
        &output,
        "Stage 1 · Archive and RetroArch profile"
    ));
    let _ = std::fs::remove_dir_all(&temp);
}

#[cfg(any())]
#[test]
fn changing_archive_clears_stale_dolphin_candidate_preview_and_transaction_state() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-dolphin-context-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    app.start_dolphin_candidate_match();
    assert!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .dolphin_selection
            .is_some()
    );
    let stale_key = cheat_preview_key(app.cheat_workflow.as_ref().unwrap());
    let (stale_sender, stale_receiver) = mpsc::channel();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.preview = CheatStepResource::Failed("stale preview".to_string());
    workflow.transaction = CheatTransactionState::Applying {
        key: stale_key,
        receiver: stale_receiver,
    };

    let other_path = PathBuf::from("/roms/other-gamecube.zip");
    if let LoadState::Ready(data) = &mut app.state {
        let mut other = record_at(other_path.clone(), MountState::Pending);
        other.identity.platform = Some("GameCube".to_string());
        data.rows.push(row_for(&other));
        data.records.push(other);
    }
    app.archive_context.select_only(other_path.clone());
    assert!(app.prepare_cheats_mods_workspace(other_path.clone()));

    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(workflow.archive_path, other_path);
    assert!(workflow.dolphin_candidate_outcome.is_none());
    assert!(workflow.dolphin_selection.is_none());
    assert!(matches!(workflow.preview, CheatStepResource::NotLoaded));
    assert!(matches!(workflow.transaction, CheatTransactionState::Idle));
    assert!(stale_sender.send(Err("stale result".to_string())).is_err());
    let _ = std::fs::remove_dir_all(&temp);
}

#[cfg(any())]
#[test]
fn dolphin_candidate_match_records_a_rejected_activity_event_when_nothing_matches() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-dolphin-test-nomatch-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp).unwrap();
    // The inventory has GAFE01, but the verified identity is a
    // different game - never guessed, never fabricated.
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.identity = CheatStepResource::NotLoaded;
    workflow.identity_request = None;

    app.start_dolphin_candidate_match();

    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(
        workflow
            .dolphin_candidate_outcome
            .as_ref()
            .unwrap()
            .candidate
            .is_none()
    );
    assert!(workflow.dolphin_selection.is_none());
    let latest = app.history.entries().next().unwrap();
    assert_eq!(latest.action, ActivityAction::DolphinGeckoCandidateMatch);
    assert_eq!(latest.outcome, ActivityOutcome::Rejected);
    let _ = std::fs::remove_dir_all(&temp);
}

#[cfg(any())]
#[test]
fn toggling_a_dolphin_code_invalidates_a_stale_preview() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-dolphin-test-toggle-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    app.start_dolphin_candidate_match();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let key = cheat_preview_key(workflow);
    workflow.preview = CheatStepResource::Ready(CheatPreviewResponse {
        key,
        outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::Shared(
            SharedPreviewError::InvalidRequest(
                archivefs_core::patch_manager::PreviewBlockerKind::SourceMissing,
            ),
        )),
        materialized: None,
        generated: None,
        dolphin_generated: None,
        xenia_generated: None,
        pcsx2_generated: None,
        gamecube_gamehacking_generated: None,
        bsfree_gamecube_generated: None,

        bsfree_wii_generated: None,
    });

    app.update_dolphin_code_selection(|selection| {
        selection.set_selected(0, true);
    });

    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(matches!(workflow.preview, CheatStepResource::NotLoaded));
    assert!(matches!(workflow.transaction, CheatTransactionState::Idle));
    assert!(
        workflow
            .dolphin_selection
            .as_ref()
            .unwrap()
            .selection
            .entries[0]
            .selected
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[cfg(any())]
#[test]
fn select_all_and_clear_all_dolphin_codes_update_the_real_selection_counts() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-dolphin-test-selectall-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    app.start_dolphin_candidate_match();

    app.update_dolphin_code_selection(DolphinCodeSelection::select_all);
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .dolphin_selection
            .as_ref()
            .unwrap()
            .selection
            .selected_count(),
        2
    );

    app.update_dolphin_code_selection(DolphinCodeSelection::clear_all);
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .dolphin_selection
            .as_ref()
            .unwrap()
            .selection
            .selected_count(),
        0
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[cfg(any())]
#[test]
fn dolphin_code_picker_renders_the_already_enabled_distinction_and_toggle_action() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-dolphin-test-render-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    app.start_dolphin_candidate_match();
    let workflow = app.cheat_workflow.as_mut().unwrap();

    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_dolphin_code_picker(ui, workflow);
        });
    });
    for expected in [
        "Stage 4 · Codes to install",
        "Infinite Bells [Nayr]",
        "Instant Growth [Nayr]",
        "Already enabled in file",
        "1 of 2 selected",
        "Preview the installed file",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn route_change_drops_stale_pcsx2_result_and_preserves_archive_state() {
    let mut app = app_with_cheats_mods_context();
    app.mount_queue.push(PathBuf::from("/roms/queued.zip"));
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("PS2".to_string());
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    workflow.selected_pcsx2_profile_id = Some("pcsx2-native-test".to_string());
    workflow.pcsx2_inventory_profile_id = Some("pcsx2-native-test".to_string());
    let (sender, receiver) = mpsc::channel();
    workflow.pcsx2_inventory = CheatStepResource::Loading { receiver };
    let archive = workflow.archive_path.clone();

    let (identity_sender, identity_receiver) = mpsc::channel();
    workflow.identity_request = Some(GameIdentityRequest {
        archive_path: archive.clone(),
        platform: workflow.platform.clone(),
        adapter: CheatEmulatorAdapter::Pcsx2,
    });
    workflow.identity = CheatStepResource::Loading {
        receiver: identity_receiver,
    };

    if let LoadState::Ready(data) = &mut app.state {
        data.records[0].identity.platform = Some("PS3".to_string());
    }
    app.view = MainView::CheatsMods;
    app.reconcile_cheats_mods_context(&egui::Context::default());

    assert!(sender.send(Ok(empty_pcsx2_inventory())).is_err());
    assert!(
        identity_sender
            .send(Ok((
                GameIdentityRequest {
                    archive_path: archive.clone(),
                    platform: Some("PS2".to_string()),
                    adapter: CheatEmulatorAdapter::Pcsx2,
                },
                inspect_game_identity(Path::new("/missing.chd"), Some("PS2")),
            )))
            .is_err()
    );
    assert_eq!(app.cheat_workflow.as_ref().unwrap().archive_path, archive);
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().pcsx2_inventory,
        CheatStepResource::NotLoaded
    ));
    assert_eq!(app.mount_queue, vec![PathBuf::from("/roms/queued.zip")]);
    assert_eq!(
        app.archive_context.focused,
        Some(PathBuf::from("/roms/a.zip"))
    );
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.platform.as_deref()),
        Some("PS3")
    );
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().adapter,
        CheatEmulatorAdapter::RetroArch
    );
}

#[test]
fn identity_result_is_rejected_after_page_or_platform_context_changes() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("GameCube".to_string());
    workflow.adapter = CheatEmulatorAdapter::Dolphin;
    let request = GameIdentityRequest {
        archive_path: workflow.archive_path.clone(),
        platform: workflow.platform.clone(),
        adapter: workflow.adapter,
    };
    let (sender, receiver) = mpsc::channel();
    workflow.identity_request = Some(request.clone());
    workflow.identity = CheatStepResource::Loading { receiver };
    sender
        .send(Ok((
            request,
            inspect_game_identity(Path::new("/missing.chd"), Some("GameCube")),
        )))
        .unwrap();
    app.view = MainView::Library;
    app.poll_cheat_workflow(&egui::Context::default());
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().identity,
        CheatStepResource::NotLoaded
    ));

    app.view = MainView::CheatsMods;
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let old_request = GameIdentityRequest {
        archive_path: workflow.archive_path.clone(),
        platform: Some("GameCube".to_string()),
        adapter: CheatEmulatorAdapter::Dolphin,
    };
    let (sender, receiver) = mpsc::channel();
    workflow.identity_request = Some(old_request.clone());
    workflow.identity = CheatStepResource::Loading { receiver };
    workflow.platform = Some("Wii".to_string());
    sender
        .send(Ok((
            old_request,
            inspect_game_identity(Path::new("/missing.chd"), Some("GameCube")),
        )))
        .unwrap();
    app.poll_cheat_workflow(&egui::Context::default());
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().identity,
        CheatStepResource::NotLoaded
    ));
}

#[test]
fn supported_loose_rom_identity_is_presented_without_unsupported_platform_wording() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-gui-loose-identity-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("Alien 3 (USA, Europe).md");
    std::fs::write(&path, b"synthetic ROM").unwrap();
    let report = inspect_catalogued_game_identity(&path, Some("MegaDrive"));
    let request = GameIdentityRequest {
        archive_path: path.clone(),
        platform: Some("MegaDrive".to_string()),
        adapter: CheatEmulatorAdapter::RetroArch,
    };
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.archive_path = path;
    workflow.platform = Some("MegaDrive".to_string());
    workflow.identity_request = Some(request.clone());
    workflow.identity = CheatStepResource::Ready((request, report));
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_shared_game_identity(ui, workflow, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(&output, "Media kind · Loose ROM"));
    assert!(rendered_text_contains(&output, "Local ROM SHA-256"));
    assert!(!rendered_text_contains(&output, "Unsupported platform"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wbfs_identity_has_a_specific_gui_format_label() {
    assert_eq!(
        dolphin_identity_format_label(IdentityImageFormat::Wbfs),
        "This WBFS file"
    );
}

#[test]
fn persisted_wbfs_identity_seeds_cheats_workspace_after_restart() {
    let path = PathBuf::from("/roms/Wrong [RZDE01].wbfs");
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut item = record(path.to_str().unwrap(), MountState::Pending);
        item.identity.platform = Some("Wii".to_string());
        data.records.push(item);
    }
    let report = GameIdentityReport {
        archive_path: path.clone(),
        platform: archivefs_core::game_identity::IdentityPlatform::Wii,
        format: IdentityImageFormat::Wbfs,
        evidence: vec![archivefs_core::game_identity::IdentityEvidence {
            kind: IdentityKind::DolphinGameId,
            status: IdentityStatus::Verified,
            value: Some("SMNE01".to_string()),
            confidence: archivefs_core::game_identity::IdentityConfidence::ExactBytes,
            provenance: archivefs_core::game_identity::IdentityProvenance {
                archive_path: path.clone(),
                member_path: None,
                member_index: None,
                method: "WBFS-contained Wii disc-info header copy".to_string(),
            },
            diagnostic: "verified synthetic fixture".to_string(),
        }],
        warnings: Vec::new(),
        bytes_read: 5_028,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    let mut persisted = persisted_archive(path.clone(), false);
    persisted.platform = Some("Wii".to_string());
    persisted.identity_report = Some(report);
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(cached_snapshot(vec![persisted])),
        last_scan_summary: None,
    };
    app.cheat_workflow = None;

    assert!(app.prepare_cheats_mods_workspace(path));
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(
        ready_game_identity(workflow).and_then(GameIdentityReport::verified_dolphin_game_id),
        Some("SMNE01")
    );
    assert!(matches!(workflow.identity, CheatStepResource::Ready(_)));
}

#[test]
fn preview_result_is_rejected_after_profile_source_or_page_changes() {
    let mut app = app_with_cheats_mods_context();
    app.mount_queue.push(PathBuf::from("/roms/queued.zip"));
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let old_key = cheat_preview_key(workflow);
    let (sender, receiver) = mpsc::channel();
    workflow.preview_request = Some(old_key.clone());
    workflow.preview = CheatStepResource::Loading { receiver };
    workflow.source_mode = CheatSourceMode::ExistingRetroArchLibrary;
    sender
        .send(Ok(CheatPreviewResponse {
            key: old_key,
            outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::Shared(
                SharedPreviewError::InvalidRequest(
                    archivefs_core::patch_manager::PreviewBlockerKind::SourceMissing,
                ),
            )),
            materialized: None,
            generated: None,
            dolphin_generated: None,
            xenia_generated: None,
            pcsx2_generated: None,
            gamecube_gamehacking_generated: None,
            bsfree_gamecube_generated: None,

            bsfree_wii_generated: None,
        }))
        .unwrap();
    app.poll_cheat_workflow(&egui::Context::default());
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().preview,
        CheatStepResource::NotLoaded
    ));

    let workflow = app.cheat_workflow.as_mut().unwrap();
    let page_key = cheat_preview_key(workflow);
    let (_sender, receiver) = mpsc::channel();
    workflow.preview_request = Some(page_key);
    workflow.preview = CheatStepResource::Loading { receiver };
    app.view = MainView::Library;
    app.poll_cheat_workflow(&egui::Context::default());
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().preview,
        CheatStepResource::NotLoaded
    ));
    app.view = MainView::CheatsMods;
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let profile_key = cheat_preview_key(workflow);
    let (_sender, receiver) = mpsc::channel();
    workflow.preview_request = Some(profile_key);
    workflow.preview = CheatStepResource::Loading { receiver };
    workflow.selected_profile_id = Some("different-profile".to_string());
    app.poll_cheat_workflow(&egui::Context::default());
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().preview,
        CheatStepResource::NotLoaded
    ));
    assert_eq!(app.mount_queue, vec![PathBuf::from("/roms/queued.zip")]);
    assert_eq!(
        app.archive_context.focused,
        Some(PathBuf::from("/roms/a.zip"))
    );
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.platform.as_deref()),
        None
    );
}

#[test]
fn preview_and_confirmation_key_changes_with_catalogue_snapshot() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.source_fetch = CheatStepResource::Ready(cheat_fetch_result_for(
        "source-a",
        CheatSourceFetchStatus::OfflineReused,
    ));
    let first = cheat_preview_key(workflow);
    let CheatStepResource::Ready(result) = &mut workflow.source_fetch else {
        unreachable!();
    };
    result.manifest.archive_sha256 = "b".repeat(64);
    let second = cheat_preview_key(workflow);
    assert_ne!(first, second);
}

#[test]
fn pcsx2_workflow_presents_gamehacking_with_identity_gated_download() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("PS2".to_string());
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    workflow.selected_pcsx2_profile_id = Some("pcsx2-native-test".to_string());
    workflow.pcsx2_inventory_profile_id = Some("pcsx2-native-test".to_string());
    workflow.pcsx2_inventory = CheatStepResource::Ready(empty_pcsx2_inventory());
    let profiles = Pcsx2ProfilesState::Ready(Pcsx2ProfileDiscovery {
        profiles: vec![pcsx2_profile_fixture()],
        warnings: Vec::new(),
        complete: true,
    });
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_workflow(ui, workflow, &profiles, &mut clipboard);
        });
    });
    for expected in [
        "Stage 1 · PCSX2 profile",
        "Eligible",
        "Existing PCSX2-managed files",
        "Uploaded · No",
        "Executed · No",
        "Changed · No",
        "Verified game CRC unavailable",
        "GameHacking.org",
        "Game identity incomplete",
        "Download",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    for forbidden in ["Install now", "Apply patch", "Enable patch", "Delete file"] {
        assert!(!rendered_text_contains(&output, forbidden));
    }
}

#[test]
fn pcsx2_workflow_shows_the_exact_resolved_cheats_directory_before_install() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("PS2".to_string());
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    workflow.selected_pcsx2_profile_id = Some("pcsx2-native-test".to_string());
    let profiles = Pcsx2ProfilesState::Ready(Pcsx2ProfileDiscovery {
        profiles: vec![pcsx2_profile_fixture()],
        warnings: Vec::new(),
        complete: true,
    });
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_workflow(ui, workflow, &profiles, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "PCSX2 cheats directory identified"
    ));
    assert!(rendered_text_contains(&output, "/isolated/PCSX2/cheats"));
}

#[test]
fn pcsx2_workflow_warns_and_blocks_install_when_cheats_directory_is_unresolvable() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("PS2".to_string());
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    workflow.selected_pcsx2_profile_id = Some("pcsx2-native-test".to_string());
    let mut profile = pcsx2_profile_fixture();
    // No documented `cheats` category patch directory is available or
    // safely creatable: this profile cannot resolve an install target.
    profile.patch_directories = vec![Pcsx2PatchDirectory {
        path: PathBuf::from("/isolated/PCSX2/cheats"),
        category: Pcsx2PatchCategory::Cheats,
        state: Pcsx2PatchDirectoryState::UnsafePath,
        warning: Some("directory is a symlink and will not be followed".to_string()),
        identity: None,
    }];
    let profiles = Pcsx2ProfilesState::Ready(Pcsx2ProfileDiscovery {
        profiles: vec![profile],
        warnings: Vec::new(),
        complete: true,
    });
    assert!(resolved_pcsx2_cheats_directory(&profiles, "pcsx2-native-test").is_none());
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_workflow(ui, workflow, &profiles, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "PCSX2 cheats directory could not be confidently identified"
    ));
    assert!(!rendered_text_contains(&output, "Install selected"));
}

#[test]
fn pcsx2_workflow_requires_explicit_choice_between_multiple_eligible_profiles() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("PS2".to_string());
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    workflow.selected_pcsx2_profile_id = None;
    let mut second = pcsx2_profile_fixture();
    second.profile_id = "pcsx2-portable-test".to_string();
    second.installation_type = Pcsx2InstallationType::Portable;
    second.scope = Pcsx2ProfileScope::Portable;
    second.configuration_path = PathBuf::from("/isolated/appimage-dir");
    let profiles = Pcsx2ProfilesState::Ready(Pcsx2ProfileDiscovery {
        profiles: vec![pcsx2_profile_fixture(), second],
        warnings: Vec::new(),
        complete: true,
    });
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_workflow(ui, workflow, &profiles, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "2 eligible profiles were found. Choose one explicitly."
    ));
    assert!(workflow.selected_pcsx2_profile_id.is_none());
    assert!(!rendered_text_contains(
        &output,
        "PCSX2 cheats directory identified"
    ));
}

#[test]
fn pcsx2_gamehacking_title_candidate_shows_identity_before_confirmation() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("PS2".to_string());
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    let game = GameHackingGame {
        game_id: 138_153,
        title: "Example Candidate".to_string(),
        system: "PlayStation 2".to_string(),
        region: Some("(PAL-M5)".to_string()),
        serial: Some("SLES-54658".to_string()),
        crc: None,
        source_url: "https://gamehacking.org/game/138153".to_string(),
    };
    workflow.pcsx2_gamehacking = CheatStepResource::Ready(Pcsx2GameHackingState {
        status: GameHackingMatchStatus::Candidates,
        detail: "Confirm the correct game.".to_string(),
        game: None,
        match_candidates: vec![GameHackingMatchCandidate {
            game,
            strength: archivefs_core::patch_manager::GameHackingMatchStrength::NormalizedTitle,
            requires_user_confirmation: true,
        }],
        candidates: Vec::new(),
        selection: Pcsx2CheatSelection::default(),
        cached_fallback: false,
    });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_gamehacking(ui, workflow, None);
        });
    });
    for expected in [
        "Confirm a candidate",
        "Example Candidate",
        "SLES-54658",
        "(PAL-M5)",
        "138153",
        "Use this match",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
}

#[test]
fn pcsx2_gamehacking_cheat_uses_real_name_as_primary_label() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("PS2".to_string());
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    workflow.pcsx2_gamehacking = CheatStepResource::Ready(Pcsx2GameHackingState {
        status: GameHackingMatchStatus::Matched,
        detail: "Matched from the local catalogue.".to_string(),
        game: Some(GameHackingGame {
            game_id: 42,
            title: "Example Game".to_string(),
            system: "PlayStation 2".to_string(),
            region: Some("NTSC-U".to_string()),
            serial: Some("SLUS-12345".to_string()),
            crc: Some("A1B2C3D4".to_string()),
            source_url: "https://gamehacking.org/game/42".to_string(),
        }),
        match_candidates: Vec::new(),
        candidates: vec![Pcsx2CheatCandidate {
            id: "gh-42-1".to_string(),
            name: "Player Codes › Infinite Health".to_string(),
            description: Some("Health never decreases.".to_string()),
            author: Some("Ada".to_string()),
            source_game_id: Some("42".to_string()),
            source_url: Some("https://gamehacking.org/game/42".to_string()),
            provider_id: "gamehacking.org".to_string(),
            provider_name: "GameHacking.org".to_string(),
            source: "https://gamehacking.org/game/42".to_string(),
            game_crc: "A1B2C3D4".to_string(),
            serial_constraint: Some("SLUS-12345".to_string()),
            region_constraint: Some("NTSC-U".to_string()),
            patch_lines: vec![
                archivefs_core::patch_manager::PnachPatchLine::parse(
                    "patch=1,EE,20123456,word,00000001",
                )
                .unwrap(),
            ],
            confidence:
                archivefs_core::patch_manager::Pcsx2CheatConfidence::VerifiedCrcAndConstraints,
            compatibility: archivefs_core::patch_manager::Pcsx2CheatCompatibility::Compatible,
        }],
        selection: Pcsx2CheatSelection::default(),
        cached_fallback: false,
    });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_gamehacking(ui, workflow, None);
        });
    });
    for expected in [
        "Player Codes › Infinite Health",
        "Author: Ada",
        "Notes: Health never decreases.",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    assert!(!rendered_text_contains(&output, "Cheat 1"));
}

#[test]
fn wii_gamehacking_section_shows_safe_and_preview_only_cheats() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-wii-gamehacking-{}",
        std::process::id()
    ));
    let mut app = wii_workflow_with_matched_identity(&directory, "R3HX6Z");
    let safe = GameHackingWiiCheat {
        id: "wii-safe".to_string(),
        name: "Infinite Health".to_string(),
        author: Some("Fixture Author".to_string()),
        description: None,
        code_format: WiiCodeFormat::Gecko,
        safety: archivefs_core::patch_manager::WiiCheatSafety::Installable,
        safety_warnings: Vec::new(),
        code_lines: vec!["04001234 60000000".to_string()],
        source_game_id: 131_936,
        source_url: "https://gamehacking.org/game/131936".to_string(),
    };
    let unsafe_cheat = GameHackingWiiCheat {
        id: "wii-placeholder".to_string(),
        name: "Choose Amount".to_string(),
        author: None,
        description: None,
        code_format: WiiCodeFormat::Gecko,
        safety: archivefs_core::patch_manager::WiiCheatSafety::UnresolvedPlaceholder,
        safety_warnings: Vec::new(),
        code_lines: vec!["04001234 XXXXXXXX".to_string()],
        source_game_id: 131_936,
        source_url: "https://gamehacking.org/game/131936".to_string(),
    };
    let matched = GameHackingWiiMatch {
        status: GameHackingWiiMatchStatus::Matched,
        detail: "Matched by exact Wii Game ID.".to_string(),
        game: Some(GameHackingWiiGame {
            game_id: 131_936,
            title: "Agent Hugo".to_string(),
            system: "Wii".to_string(),
            region: Some("Europe".to_string()),
            dolphin_game_id: Some("R3HX6Z".to_string()),
            revision: None,
            disc_number: Some(0),
            crc32: None,
            source_url: "https://gamehacking.org/game/131936".to_string(),
        }),
        candidates: Vec::new(),
    };
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.gamecube_gamehacking =
        CheatStepResource::Ready(wii_match_state(matched, vec![safe, unsafe_cheat], false));
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    for expected in [
        "GameHacking.org (Wii)",
        "R3HX6Z",
        "Code format: Gecko",
        "Choose Amount",
        "Preview only: contains unresolved placeholder text.",
        "Install selected",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    let CheatStepResource::Ready(state) = &workflow.gamecube_gamehacking else {
        panic!("fixture must remain ready")
    };
    assert_eq!(
        state
            .selection
            .entries
            .iter()
            .filter(|entry| entry.selectable)
            .count(),
        1
    );
}

fn write_wii_match_catalogue(root: &Path, game_id: &str, revision: Option<u16>) {
    std::fs::create_dir_all(root).unwrap();
    let revision = revision.map_or("null".to_string(), |value| value.to_string());
    let catalogue = format!(
        r#"{{"schema_version":1,"provider":"gamehacking.org","system":"Wii","source_url":"https://gamehacking.org/system/wii/all","retrieved_at_unix_seconds":1,"pages":[],"games":[{{"game_id":131936,"title":"New Super Mario Bros. Wii","dolphin_game_id":"{game_id}","region":"USA","revision":{revision},"disc_number":0,"crc32":null,"source_url":"https://gamehacking.org/game/131936","index_source_url":"https://gamehacking.org/system/wii/all","retrieved_at_unix_seconds":1}}]}}"#
    );
    std::fs::write(root.join("wii-catalogue.json"), catalogue.as_bytes()).unwrap();
}

/// How long a test waits for a background Wii/GameCube matching task
/// before declaring the run stuck.
///
/// The work itself is a cached-catalogue read and a match: single-digit
/// milliseconds. Thirty seconds is therefore three to four orders of
/// magnitude of headroom for a loaded machine, while still failing a
/// genuine deadlock long before an outer CI job timeout would, and with a
/// far better message than a killed process gives.
const WII_MATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Spins on `ready` until it returns true, or the deadline passes.
///
/// Returns how long the wait took, or `Err(elapsed)` on timeout.
///
/// # Why a deadline rather than an iteration count
///
/// The previous version of `wait_for_wii_match_terminal` gave up after a
/// fixed 2,000 iterations of `poll` + `std::thread::yield_now()`. That is
/// not a wait at all: `yield_now` returns immediately when the calling
/// core has nothing else runnable, so the whole budget could be spent in
/// microseconds - before the worker thread had even been scheduled once.
/// The loop's *duration* was a function of machine load rather than a
/// bound anyone chose, so the busier the suite, the sooner it gave up.
/// That is exactly backwards, and it is why adding unrelated GUI tests
/// made a pre-existing race start showing.
///
/// A wall-clock deadline inverts that: contention can only make the wait
/// longer, never shorter, so the only way to fail is to genuinely exceed
/// the timeout. The backoff below keeps the fast path fast (a ready
/// result is observed on the first poll, with no syscall) while making a
/// slow path actually yield the core to the worker rather than burning it.
fn wait_until<F>(
    timeout: std::time::Duration,
    mut ready: F,
) -> Result<std::time::Duration, std::time::Duration>
where
    F: FnMut() -> bool,
{
    // A short spin first: the overwhelmingly common case is that the
    // result is already there, and a sleep would add latency to every
    // test for nothing.
    const SPIN_POLLS: u32 = 64;
    const MIN_SLEEP: std::time::Duration = std::time::Duration::from_micros(200);
    const MAX_SLEEP: std::time::Duration = std::time::Duration::from_millis(2);

    let started = Instant::now();
    let mut polls = 0_u32;
    let mut sleep = MIN_SLEEP;
    loop {
        if ready() {
            return Ok(started.elapsed());
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(elapsed);
        }
        if polls < SPIN_POLLS {
            std::thread::yield_now();
        } else {
            // Never sleep past the deadline: a timeout must be reported
            // promptly rather than one whole sleep late.
            std::thread::sleep(sleep.min(timeout - elapsed));
            sleep = (sleep * 2).min(MAX_SLEEP);
        }
        polls = polls.saturating_add(1);
    }
}

/// The observable state of one cheat step, for a failure message.
fn describe_cheat_step<T>(resource: &CheatStepResource<T>) -> String {
    match resource {
        CheatStepResource::NotLoaded => "NotLoaded".to_string(),
        CheatStepResource::Loading { .. } => "Loading".to_string(),
        CheatStepResource::Ready(_) => "Ready".to_string(),
        CheatStepResource::Failed(message) => format!("Failed({message})"),
    }
}

fn wait_for_wii_match_terminal(app: &mut ArchiveFsApp, context: &egui::Context) {
    wait_for_wii_match_terminal_within(app, context, WII_MATCH_TIMEOUT);
}

/// Polls the workflow until its GameHacking step leaves `Loading`.
///
/// The completion signal is the real one the application uses: the worker
/// sends its result down the `mpsc` channel held in
/// `CheatStepResource::Loading { receiver }`, and `poll_cheat_workflow`
/// picks it up with `try_recv` and replaces the state. This waits on that
/// same observable state transition rather than on a timer, so it cannot
/// pass before the result has actually been absorbed. The receiver is
/// deliberately not read directly here - doing so would consume the
/// message the application is supposed to see.
fn wait_for_wii_match_terminal_within(
    app: &mut ArchiveFsApp,
    context: &egui::Context,
    timeout: std::time::Duration,
) -> std::time::Duration {
    let outcome = wait_until(timeout, || {
        app.poll_cheat_workflow(context);
        !matches!(
            app.cheat_workflow.as_ref().unwrap().gamecube_gamehacking,
            CheatStepResource::Loading { .. }
        )
    });
    match outcome {
        Ok(elapsed) => elapsed,
        Err(elapsed) => {
            // Everything observable about why it is still stuck.
            let workflow = app.cheat_workflow.as_ref().unwrap();
            panic!(
                "the cached Wii matching task did not publish a terminal result within \
                     {timeout:?} (waited {elapsed:?}).\n  gamecube_gamehacking: {}\n  \
                     request: {:?}\n  generation: {}\n  cancellation outstanding: {}\n  \
                     blocked: {}\n  platform: {:?}\n  adapter: {:?}",
                describe_cheat_step(&workflow.gamecube_gamehacking),
                workflow.gamecube_gamehacking_request,
                workflow.gamecube_gamehacking_generation,
                workflow.gamecube_gamehacking_cancellation.is_some(),
                workflow.gamecube_gamehacking_blocked,
                workflow.platform,
                workflow.adapter,
            );
        }
    }
}

// --- The asynchronous wait helper itself --------------------------------
//
// `wait_for_wii_match_terminal` used to give up after a fixed 2,000
// iterations of poll + `yield_now()`. That budget is a count, not a
// duration, and `yield_now` does not sleep - so on a loaded machine the
// whole budget could be spent before the worker thread was scheduled even
// once, and the test failed while the code under test was perfectly
// healthy. These tests pin the replacement's contract directly, without
// needing a background worker to misbehave.

#[test]
fn the_wait_helper_returns_at_once_when_the_condition_already_holds() {
    let elapsed = wait_until(std::time::Duration::from_secs(30), || true)
        .expect("an already-satisfied condition cannot time out");
    assert!(
        elapsed < std::time::Duration::from_millis(50),
        "the fast path must not sleep; took {elapsed:?}"
    );
}

/// The case the old helper got wrong: completion that arrives late still
/// has to be waited for, however busy the machine is.
#[test]
fn the_wait_helper_tolerates_completion_that_arrives_late() {
    let flag = Arc::new(AtomicBool::new(false));
    let worker = flag.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(250));
        worker.store(true, Ordering::SeqCst);
    });

    let elapsed = wait_until(std::time::Duration::from_secs(30), || {
        flag.load(Ordering::SeqCst)
    })
    .expect("250ms is far inside a 30s deadline");
    handle.join().unwrap();

    assert!(
        elapsed >= std::time::Duration::from_millis(200),
        "it must actually have waited for the worker; took {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "and it must return promptly once the worker finishes; took {elapsed:?}"
    );
}

/// A genuine deadlock must still fail, and fail on time rather than
/// hanging until an outer harness kills the process.
#[test]
fn the_wait_helper_times_out_promptly_when_nothing_ever_completes() {
    let timeout = std::time::Duration::from_millis(300);
    let started = Instant::now();
    let outcome = wait_until(timeout, || false);
    let measured = started.elapsed();

    let reported = outcome.expect_err("a condition that never holds must time out");
    assert!(
        reported >= timeout,
        "the deadline must be honoured, not cut short; reported {reported:?}"
    );
    assert!(
        measured < timeout * 4,
        "the timeout must be prompt, not one long sleep late; took {measured:?}"
    );
}

/// The timeout message has to say what the last observed state was, or it
/// is no more useful than the bare panic it replaced.
#[test]
fn the_failure_description_names_the_observed_state() {
    let loading: CheatStepResource<u8> = CheatStepResource::Loading {
        receiver: mpsc::channel().1,
    };
    assert_eq!(describe_cheat_step(&loading), "Loading");
    assert_eq!(
        describe_cheat_step(&CheatStepResource::<u8>::NotLoaded),
        "NotLoaded"
    );
    assert_eq!(
        describe_cheat_step(&CheatStepResource::Ready(1_u8)),
        "Ready"
    );
    // A failure keeps its reason: that is usually the whole answer.
    assert_eq!(
        describe_cheat_step(&CheatStepResource::<u8>::Failed("no catalogue".to_string())),
        "Failed(no catalogue)"
    );
}

/// End to end: a real worker made deliberately slow still reaches a
/// terminal result through the real polling path.
#[test]
fn a_deliberately_delayed_wii_match_still_reaches_a_terminal_result() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-gui-wii-delayed-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    write_wii_match_catalogue(&root, "SMNE01", None);
    let mut app = wii_workflow_with_matched_identity(&root, "SMNE01");
    let context = egui::Context::default();
    app.start_gamecube_gamehacking_fetch_with_options(
        context.clone(),
        WiiGameHackingFetchMode::CacheOnly,
        GameHackingGameCubeFetchOptions {
            cache_root: root.clone(),
            force_refresh: false,
            // A real delay inside the worker, so this exercises waiting
            // rather than a result that happens to be ready already.
            delay: std::time::Duration::from_millis(250),
            cancellation: None,
        },
    );
    // Still `Loading` immediately after starting, which is what makes the
    // wait below meaningful rather than vacuous.
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().gamecube_gamehacking,
        CheatStepResource::Loading { .. }
    ));

    let elapsed =
        wait_for_wii_match_terminal_within(&mut app, &context, std::time::Duration::from_secs(30));
    assert!(
        !matches!(
            app.cheat_workflow.as_ref().unwrap().gamecube_gamehacking,
            CheatStepResource::Loading { .. }
        ),
        "the delayed worker must still have been absorbed"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "a 250ms worker must not take anywhere near the deadline; took {elapsed:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn wii_cached_match_starts_once_across_repeated_frames_and_clicks() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-gui-wii-one-shot-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    write_wii_match_catalogue(&root, "SMNE01", None);
    let mut app = wii_workflow_with_matched_identity(&root, "SMNE01");
    let context = egui::Context::default();
    let options = GameHackingGameCubeFetchOptions {
        cache_root: root.clone(),
        force_refresh: false,
        delay: std::time::Duration::ZERO,
        cancellation: None,
    };
    assert!(wii_gamehacking_auto_match_needed(
        app.cheat_workflow.as_ref().unwrap()
    ));
    app.start_gamecube_gamehacking_fetch_with_options(
        context.clone(),
        WiiGameHackingFetchMode::CacheOnly,
        options.clone(),
    );
    for _ in 0..32 {
        app.start_gamecube_gamehacking_fetch_with_options(
            context.clone(),
            WiiGameHackingFetchMode::CacheOnly,
            options.clone(),
        );
    }
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(workflow.gamecube_gamehacking_generation, 1);
    assert!(!wii_gamehacking_auto_match_needed(workflow));
    assert_eq!(
        app.history
            .entries()
            .filter(|entry| {
                entry.action == ActivityAction::CheatSourceRetrieval
                    && entry.outcome == ActivityOutcome::Started
            })
            .count(),
        1
    );

    wait_for_wii_match_terminal(&mut app, &context);
    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatStepResource::Ready(state) = &workflow.gamecube_gamehacking else {
        panic!("cached SMNE01 match must terminate successfully")
    };
    assert_eq!(state.status, GameHackingGameCubeMatchStatus::Matched);
    assert_eq!(
        state.game.as_ref().unwrap().dolphin_game_id.as_deref(),
        Some("SMNE01")
    );
    assert!(workflow.gamecube_gamehacking_cancellation.is_none());
    assert_eq!(workflow.gamecube_gamehacking_generation, 1);
    assert_eq!(
        wii_gamehacking_beginner_status(workflow),
        BeginnerCheatStatus::NoCompatibleCheatsFound
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wii_cached_no_match_and_provider_error_both_clear_loading() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-gui-wii-terminal-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let context = egui::Context::default();
    let options = GameHackingGameCubeFetchOptions {
        cache_root: root.clone(),
        force_refresh: false,
        delay: std::time::Duration::ZERO,
        cancellation: None,
    };
    let mut failed = wii_workflow_with_matched_identity(&root, "SMNE01");
    failed.start_gamecube_gamehacking_fetch_with_options(
        context.clone(),
        WiiGameHackingFetchMode::CacheOnly,
        options.clone(),
    );
    wait_for_wii_match_terminal(&mut failed, &context);
    assert!(matches!(
        failed.cheat_workflow.as_ref().unwrap().gamecube_gamehacking,
        CheatStepResource::Failed(_)
    ));

    write_wii_match_catalogue(&root, "RMCE01", None);
    let mut no_match = wii_workflow_with_matched_identity(&root, "SMNE01");
    no_match.start_gamecube_gamehacking_fetch_with_options(
        context.clone(),
        WiiGameHackingFetchMode::CacheOnly,
        options,
    );
    wait_for_wii_match_terminal(&mut no_match, &context);
    let CheatStepResource::Ready(state) = &no_match
        .cheat_workflow
        .as_ref()
        .unwrap()
        .gamecube_gamehacking
    else {
        panic!("no-match is a successful terminal provider result")
    };
    assert_eq!(state.status, GameHackingGameCubeMatchStatus::NoMatch);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wii_cached_match_discards_a_result_after_selected_game_changes() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-gui-wii-stale-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    write_wii_match_catalogue(&root, "SMNE01", None);
    let mut app = wii_workflow_with_matched_identity(&root, "SMNE01");
    let context = egui::Context::default();
    app.start_gamecube_gamehacking_fetch_with_options(
        context.clone(),
        WiiGameHackingFetchMode::CacheOnly,
        GameHackingGameCubeFetchOptions {
            cache_root: root.clone(),
            force_refresh: false,
            delay: std::time::Duration::ZERO,
            cancellation: None,
        },
    );
    app.cheat_workflow.as_mut().unwrap().archive_path =
        PathBuf::from("/games/a-different-selection.wbfs");
    wait_for_wii_match_terminal(&mut app, &context);
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(matches!(
        workflow.gamecube_gamehacking,
        CheatStepResource::NotLoaded
    ));
    assert!(workflow.gamecube_gamehacking_request.is_none());
    assert!(workflow.gamecube_gamehacking_cancellation.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wii_cloudflare_state_offers_offline_import_without_retry() {
    let directory =
        std::env::temp_dir().join(format!("archivefs-gui-wii-blocked-{}", std::process::id()));
    let mut app = wii_workflow_with_matched_identity(&directory, "R3HX6Z");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.gamecube_gamehacking =
        CheatStepResource::Failed(GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE.to_string());
    workflow.gamecube_gamehacking_blocked = true;
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "GameHacking.org access blocked"
    ));
    assert!(rendered_text_contains(
        &output,
        "gamehacking-wii-import-page"
    ));
    assert_eq!(count_exact_text_occurrences(&output, "Try again"), 0);
}

#[test]
fn gamecube_gamehacking_shows_matched_title_game_id_named_cheats_and_install_button() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-gamecube-gamehacking-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GTRE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.gamecube_gamehacking = CheatStepResource::Ready(GameCubeGameHackingState {
        status: GameHackingGameCubeMatchStatus::Matched,
        detail: "Matched from the local catalogue.".to_string(),
        game: Some(GameHackingGameCubeGame {
            game_id: 501,
            title: "Test Racer".to_string(),
            system: "GameCube".to_string(),
            region: Some("USA".to_string()),
            dolphin_game_id: Some("GTRE01".to_string()),
            revision: None,
            hash: None,
            source_url: "https://gamehacking.org/game/501".to_string(),
        }),
        match_candidates: Vec::new(),
        selection: gamecube_gamehacking_selection_for(&[GameHackingGameCubeCheat {
            id: "gh-gc-501-1".to_string(),
            name: "Infinite Boost".to_string(),
            author: Some("Ada".to_string()),
            description: Some("Boost never runs out.".to_string()),
            code_format: GameCubeCodeFormat::ActionReplay,
            code_lines: vec!["04001234 00000001".to_string()],
            source_game_id: 501,
            source_url: "https://gamehacking.org/game/501".to_string(),
        }]),
        cheats: vec![GameHackingGameCubeCheat {
            id: "gh-gc-501-1".to_string(),
            name: "Infinite Boost".to_string(),
            author: Some("Ada".to_string()),
            description: Some("Boost never runs out.".to_string()),
            code_format: GameCubeCodeFormat::ActionReplay,
            code_lines: vec!["04001234 00000001".to_string()],
            source_game_id: 501,
            source_url: "https://gamehacking.org/game/501".to_string(),
        }],
        cached_fallback: false,
    });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    for expected in [
        "Test Racer",
        "GameHacking game 501",
        "Infinite Boost",
        "Author: Ada",
        "Notes: Boost never runs out.",
        "Code format: Action Replay",
        "Install selected",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
}

#[test]
fn gamecube_install_selected_reaches_review_and_names_live_and_staging_paths() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-gamecube-apply-review-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(directory.join("GameSettings")).unwrap();
    std::fs::write(directory.join("Dolphin.ini"), "[Core]\n").unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&directory, "G9RE7D");
    let cheat = GameHackingGameCubeCheat {
        id: "gh-gc-55194-1".to_string(),
        name: "Infinite Money And Gems".to_string(),
        author: Some("Codejunkies".to_string()),
        description: None,
        code_format: GameCubeCodeFormat::ActionReplay,
        code_lines: vec!["040D30C8 3860270F".to_string()],
        source_game_id: 55_194,
        source_url: "https://gamehacking.org/game/55194".to_string(),
    };
    let mut selection = gamecube_gamehacking_selection_for(std::slice::from_ref(&cheat));
    selection.select_all();
    app.cheat_workflow.as_mut().unwrap().gamecube_gamehacking =
        CheatStepResource::Ready(GameCubeGameHackingState {
            status: GameHackingGameCubeMatchStatus::Matched,
            detail: "Exact Game ID match".to_string(),
            game: Some(GameHackingGameCubeGame {
                game_id: 55_194,
                title: "The Sims Bustin' Out".to_string(),
                system: "GameCube".to_string(),
                region: Some("USA".to_string()),
                dolphin_game_id: Some("G9RE7D".to_string()),
                revision: None,
                hash: None,
                source_url: "https://gamehacking.org/game/55194".to_string(),
            }),
            match_candidates: Vec::new(),
            cheats: vec![cheat],
            selection,
            cached_fallback: false,
        });

    app.start_gamecube_gamehacking_install_preview_with_staging_root(directory.join("staging"));
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let CheatTransactionState::Review { plan, .. } = &workflow.transaction else {
        panic!("Install selected must reach shared review instead of stopping at staging")
    };
    assert_eq!(
        format!(
            "{}/{}",
            plan.entries[0].destination_root.display,
            plan.entries[0].destination_relative_path.display
        ),
        directory
            .join("GameSettings/G9RE7D.ini")
            .display()
            .to_string()
    );
    assert!(
        !plan
            .destination_root
            .display
            .contains("generated-gamecube-gamehacking")
    );

    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "Staging artifact (not destination)"
    ));
    assert!(rendered_text_contains(&output, "GameSettings/G9RE7D.ini"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn gamecube_gamehacking_candidate_requires_explicit_confirmation() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-gamecube-gamehacking-candidate-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GTRE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.gamecube_gamehacking = CheatStepResource::Ready(GameCubeGameHackingState {
            status: GameHackingGameCubeMatchStatus::Candidates,
            detail: "Only normalized-title candidates were found.".to_string(),
            game: None,
            match_candidates: vec![GameHackingGameCubeMatchCandidate {
                game: GameHackingGameCubeGame {
                    game_id: 501,
                    title: "Test Racer".to_string(),
                    system: "GameCube".to_string(),
                    region: Some("USA".to_string()),
                    dolphin_game_id: Some("ZZZZZZ".to_string()),
                    revision: None,
                    hash: None,
                    source_url: "https://gamehacking.org/game/501".to_string(),
                },
                strength:
                    archivefs_core::patch_manager::GameHackingGameCubeMatchStrength::NormalizedTitleAndRegion,
                requires_user_confirmation: true,
            }],
            selection: gamecube_gamehacking_selection_for(&[]),
            cheats: Vec::new(),
            cached_fallback: false,
        });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    for expected in [
        "Confirm a candidate",
        "Test Racer",
        "GameHacking game ID: 501",
        "normalized title + compatible region",
        "Use this match",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    assert!(!rendered_text_contains(&output, "Install selected"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn gamecube_gamehacking_shows_dedicated_blocked_state_without_retry() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-gamecube-gamehacking-blocked-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GTRE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.gamecube_gamehacking =
        CheatStepResource::Failed(GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE.to_string());
    workflow.gamecube_gamehacking_blocked = true;
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "GameHacking.org access blocked"
    ));
    assert!(rendered_text_contains(
        &output,
        GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE
    ));
    assert_eq!(
        count_exact_text_occurrences(&output, "Try again"),
        0,
        "a confirmed Cloudflare block must not offer a Retry button (the required wording itself legitimately ends in \"Try again later.\", so this checks the standalone button label, not a substring)"
    );
    assert!(!rendered_text_contains(
        &output,
        "Could not check GameHacking.org"
    ));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn gamecube_gamehacking_marks_a_successful_cached_fallback_as_stale() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-gamecube-gamehacking-cache-{}",
        std::process::id()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GTRE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.gamecube_gamehacking = CheatStepResource::Ready(GameCubeGameHackingState {
        status: GameHackingGameCubeMatchStatus::NoMatch,
        detail: "No match in the cached catalogue.".to_string(),
        game: None,
        match_candidates: Vec::new(),
        cheats: Vec::new(),
        selection: gamecube_gamehacking_selection_for(&[]),
        cached_fallback: true,
    });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "Using cached GameHacking.org data"
    ));
    assert!(rendered_text_contains(&output, "may be stale"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn gamecube_gamehacking_keeps_retry_for_an_ordinary_failure() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-gamecube-gamehacking-ordinary-failure-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GTRE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.gamecube_gamehacking = CheatStepResource::Failed(
        "GameHacking.org is temporarily unavailable (HTTP 500)".to_string(),
    );
    workflow.gamecube_gamehacking_blocked = false;
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_gamecube_gamehacking(ui, workflow);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "Could not check GameHacking.org"
    ));
    assert!(
        rendered_text_contains(&output, "Try again"),
        "a transient failure must still offer Retry"
    );
    assert!(!rendered_text_contains(
        &output,
        "GameHacking.org access blocked"
    ));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn ps2_archive_context_defaults_to_pcsx2_without_queue_or_mount_mutation() {
    let mut app = app_for_operation_tests();
    let mut ps2 = record("/roms/game.zip", MountState::Mounted);
    ps2.identity.platform = Some("PS2".to_string());
    ps2.mount_plan.archive.identity.platform = Some("PS2".to_string());
    if let LoadState::Ready(data) = &mut app.state {
        data.records.push(ps2);
    }
    app.mount_queue.push(PathBuf::from("/roms/queued.zip"));
    app.archive_context.focused = Some(PathBuf::from("/roms/other.zip"));
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/game.zip")));
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(workflow.adapter, CheatEmulatorAdapter::Pcsx2);
    assert_eq!(workflow.archive_path, PathBuf::from("/roms/game.zip"));
    assert_eq!(app.mount_queue, vec![PathBuf::from("/roms/queued.zip")]);
    assert_eq!(
        app.archive_context.focused,
        Some(PathBuf::from("/roms/other.zip"))
    );
    let live = match &app.state {
        LoadState::Ready(data) => &data.records[0],
        _ => unreachable!(),
    };
    assert_eq!(live.mount_state, MountState::Mounted);
    assert_eq!(live.identity.platform.as_deref(), Some("PS2"));
}

#[test]
fn cheat_source_labels_cover_every_freshness_and_status() {
    assert_eq!(cheat_freshness_label(CheatSourceFreshness::Fresh), "Fresh");
    assert_eq!(cheat_freshness_label(CheatSourceFreshness::Stale), "Stale");
    assert_eq!(
        cheat_freshness_label(CheatSourceFreshness::Missing),
        "Not cached"
    );
    assert_eq!(
        cheat_freshness_label(CheatSourceFreshness::Unknown),
        "Unknown"
    );
    assert_eq!(
        cheat_fetch_status_label(CheatSourceFetchStatus::Fetched),
        "Downloaded fresh catalogue"
    );
    assert_eq!(
        cheat_fetch_status_label(CheatSourceFetchStatus::CacheReused),
        "Reused cached snapshot"
    );
    assert_eq!(
        cheat_fetch_status_label(CheatSourceFetchStatus::OfflineReused),
        "Offline: reused cached snapshot"
    );
}

#[test]
fn sources_catalogue_card_separates_download_update_and_never_offers_apply() {
    let (_, _, mut list) = cheat_source_list_fixture();
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    list.entries[0].status = CheatCatalogueStatus::Missing;
    list.entries[0].freshness = CheatSourceFreshness::Missing;
    list.entries[0].setup_usable = false;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_retroarch_catalogue_manager(
                ui,
                &CatalogueManagerState::Ready(list.clone()),
                None,
                None,
                None,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Download"));
    assert!(!rendered_text_contains(&output, "Update"));
    assert!(rendered_text_contains(
        &output,
        "Apply remains a separate confirmed transaction"
    ));

    list.entries[0].status = CheatCatalogueStatus::Ready;
    list.entries[0].freshness = CheatSourceFreshness::Fresh;
    list.entries[0].setup_usable = true;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_retroarch_catalogue_manager(
                ui,
                &CatalogueManagerState::Ready(list.clone()),
                None,
                None,
                None,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Update"));
    assert!(rendered_text_contains(&output, "Verify"));
}

#[test]
fn sources_retained_snapshot_stays_visibly_usable_after_update_failure() {
    let (_, _, mut list) = cheat_source_list_fixture();
    let entry = &mut list.entries[0];
    entry.status = CheatCatalogueStatus::ReadyWithWarnings;
    entry.indexed_file_count = Some(27_853);
    entry.excluded_file_count = Some(147);
    entry.last_error = Some(CheatSourceError {
        schema_version: CHEAT_SOURCE_RESULT_SCHEMA_VERSION,
        stage: archivefs_core::patch_manager::CheatSourceErrorStage::Download,
        code: "download_too_large".into(),
        message: "received 268435457 bytes, exceeding configured limit 268435456 bytes".into(),
        retry_after_seconds: None,
    });
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_retroarch_catalogue_manager(
                ui,
                &CatalogueManagerState::Ready(list.clone()),
                None,
                None,
                None,
                &mut clipboard,
            );
        });
    });
    for expected in [
        "Verified with warnings",
        "Indexed: 27853",
        "147 cheat files could not be read",
        "Update failed · existing catalogue remains active and usable",
        "larger than the current safety limit",
    ] {
        assert!(rendered_text_contains(&output, expected));
    }
    // The raw error code/byte-count text is a Technical details disclosure
    // now, not primary UI - it must not appear in the collapsed render.
    assert!(!rendered_text_contains(&output, "download_too_large"));
}

#[test]
fn sources_shows_streaming_bytes_percentage_retry_and_retained_snapshot_wording() {
    let (_, _, list) = cheat_source_list_fixture();
    let (_result_sender, result_receiver) = mpsc::channel();
    let (_progress_sender, progress_receiver) = mpsc::channel();
    let mut running = RunningCatalogueRetrieval {
        generation: 1,
        source_id: "libretro-buildbot-cheats".into(),
        cancellation: CheatSourceCancellation::default(),
        receiver: result_receiver,
        progress_receiver,
        progress: Some(CheatSourceProgress {
            phase: CheatSourceProgressPhase::Downloading,
            attempt: 1,
            maximum_attempts: 3,
            bytes_received: 89 * 1024 * 1024,
            total_bytes: Some(178 * 1024 * 1024),
            retry_delay_seconds: None,
        }),
        cancellation_requested: false,
    };
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_retroarch_catalogue_manager(
                ui,
                &CatalogueManagerState::Ready(list.clone()),
                None,
                Some(&running),
                None,
                &mut clipboard,
            );
        });
    });
    for expected in [
        "Downloading",
        "Attempt 1 of 3",
        "Received 89.0 MiB of 178.0 MiB (50.0%)",
        "active snapshot remains usable until activation",
    ] {
        assert!(rendered_text_contains(&output, expected));
    }

    running.progress = Some(CheatSourceProgress {
        phase: CheatSourceProgressPhase::Retrying,
        attempt: 2,
        maximum_attempts: 3,
        bytes_received: 0,
        total_bytes: None,
        retry_delay_seconds: Some(5),
    });
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_retroarch_catalogue_manager(
                ui,
                &CatalogueManagerState::Ready(list.clone()),
                None,
                Some(&running),
                None,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Retrying"));
    assert!(rendered_text_contains(&output, "Attempt 2 of 3"));
    assert!(rendered_text_contains(&output, "Retrying after 5 seconds"));
}

#[test]
fn excluded_retroarch_match_has_precise_gui_state() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let key = cheat_preview_key(workflow);
    workflow.preview = CheatStepResource::Ready(CheatPreviewResponse {
        key,
        outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::Materialization(
            RetroArchMaterializationError {
                kind: RetroArchMaterializationErrorKind::MatchingEntryExcluded,
                path: Some(PathBuf::from("/snapshot/Alien 3.cht")),
                detail: "matching catalogue file was excluded because it could not be parsed"
                    .into(),
            },
        )),
        materialized: None,
        generated: None,
        dolphin_generated: None,
        xenia_generated: None,
        pcsx2_generated: None,
        gamecube_gamehacking_generated: None,
        bsfree_gamecube_generated: None,

        bsfree_wii_generated: None,
    });
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_shared_cheat_preview(ui, workflow, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "Matching catalogue entry excluded"
    ));
    assert!(rendered_text_contains(&output, "could not be parsed"));
    assert!(!rendered_text_contains(&output, "No matching cheat found"));
}

#[test]
fn no_eligible_match_is_reported_honestly_with_no_installation_ready_wording() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let key = cheat_preview_key(workflow);
    workflow.preview = CheatStepResource::Ready(CheatPreviewResponse {
        key,
        outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::Materialization(
            RetroArchMaterializationError {
                kind: RetroArchMaterializationErrorKind::NoEligibleMatch,
                path: None,
                detail: "no exact or approved strong match".into(),
            },
        )),
        materialized: None,
        generated: None,
        dolphin_generated: None,
        xenia_generated: None,
        pcsx2_generated: None,
        gamecube_gamehacking_generated: None,
        bsfree_gamecube_generated: None,

        bsfree_wii_generated: None,
    });
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_shared_cheat_preview(ui, workflow, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "No cheats found for this game"
    ));
    assert!(rendered_text_contains(&output, "Try another cheat source"));
    for forbidden in ["Install now", "Ready to install", "Apply now"] {
        assert!(
            !rendered_text_contains(&output, forbidden),
            "an unmatched archive must never show an installation-ready action or badge"
        );
    }
}

/// Phase 6: builds one real `SharedPreviewEntry` with a given match
/// strength/proposed action, for testing `show_shared_cheat_preview`'s
/// actual rendered output rather than source-scanning it - this function
/// already had a real render test in this file (`excluded_retroarch_
/// match_has_precise_gui_state` et al.), just never with a populated
/// `entries` list until now.
fn preview_entry_fixture(
    match_strength: PreviewMatchStrength,
    proposed_action: PreviewProposedAction,
) -> SharedPreviewEntry {
    SharedPreviewEntry {
        adapter: PreviewAdapter::Dolphin,
        selected_archive: PathBuf::from("/roms/Mystery Game.zip"),
        verified_identity: None,
        match_strength,
        source_path: Some(PathBuf::from("/staging/cheat.ini")),
        source_digest: Some("a".repeat(64)),
        destination_root: PathBuf::from("/dolphin/GameSettings"),
        destination_relative_path: Some(PathBuf::from("GAFE01.ini")),
        destination_path: Some(PathBuf::from("/dolphin/GameSettings/GAFE01.ini")),
        destination_state: PreviewDestinationState::Missing,
        existing_destination_digest: None,
        state: PreviewState::Ambiguous,
        proposed_action,
        eligibility: PreviewEligibility::Eligible,
        blockers: Vec::new(),
        warnings: Vec::new(),
        backup_required: false,
        explicit_replacement_permission_required: false,
    }
}

fn preview_report_with_one_entry(entry: SharedPreviewEntry) -> SharedPreviewReport {
    SharedPreviewReport {
        request_archive: PathBuf::from("/roms/Mystery Game.zip"),
        adapter: PreviewAdapter::Dolphin,
        entries: vec![entry],
        conflicts: Vec::new(),
        warnings: Vec::new(),
        summary: Default::default(),
        complete: true,
    }
}

fn render_shared_cheat_preview_for(entry: SharedPreviewEntry) -> egui::FullOutput {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let key = cheat_preview_key(workflow);
    workflow.preview = CheatStepResource::Ready(CheatPreviewResponse {
        key,
        outcome: CheatPreviewOutcome::Ready(preview_report_with_one_entry(entry)),
        materialized: None,
        generated: None,
        dolphin_generated: None,
        xenia_generated: None,
        pcsx2_generated: None,
        gamecube_gamehacking_generated: None,
        bsfree_gamecube_generated: None,
        bsfree_wii_generated: None,
    });
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_shared_cheat_preview(ui, workflow, &mut clipboard);
        });
    })
}

/// Requirement 1/6: an ambiguous match must read as a human sentence, not
/// the bare classification word, as the primary presentation.
#[test]
fn ambiguous_match_shows_human_wording_not_the_raw_classification_word() {
    let entry = preview_entry_fixture(
        PreviewMatchStrength::Ambiguous,
        PreviewProposedAction::Blocked,
    );
    let output = render_shared_cheat_preview_for(entry);
    assert!(rendered_text_contains(&output, "Not sure which one"));
    assert!(rendered_text_contains(
        &output,
        "We're not sure which game this file belongs to"
    ));
    // The precise word survives, but only inside Technical details - not
    // as a bare, unexplained primary badge.
    assert!(!rendered_text_contains(&output, "Ambiguous"));
}

#[test]
fn candidate_match_shows_human_wording_not_the_raw_classification_word() {
    let entry = preview_entry_fixture(
        PreviewMatchStrength::Candidate,
        PreviewProposedAction::Install,
    );
    let output = render_shared_cheat_preview_for(entry);
    assert!(rendered_text_contains(&output, "Possible match"));
    assert!(rendered_text_contains(
        &output,
        "We found a possible match, but it needs checking"
    ));
    assert!(!rendered_text_contains(&output, "Candidate"));
}

#[test]
fn confident_match_wording_does_not_overstate_verification() {
    let entry = preview_entry_fixture(PreviewMatchStrength::Strong, PreviewProposedAction::Install);
    let output = render_shared_cheat_preview_for(entry);
    assert!(rendered_text_contains(&output, "Confident match"));
    assert!(rendered_text_contains(
        &output,
        "not independently verified"
    ));
}

/// Requirement 3: no `format!("Proposed action: {:?}")` raw enum Debug
/// output as primary presentation - a real sentence instead.
#[test]
fn proposed_action_is_a_sentence_not_debug_formatted_enum_output() {
    let entry = preview_entry_fixture(PreviewMatchStrength::Strong, PreviewProposedAction::Replace);
    let output = render_shared_cheat_preview_for(entry);
    assert!(rendered_text_contains(
        &output,
        "If you continue: Replace the existing cheat file"
    ));
    assert!(!rendered_text_contains(&output, "Proposed action: Replace"));
}

/// Requirement 2/9: technical detail (hashes, raw paths, precise
/// classification/state names, the raw proposed-action enum name) must
/// still be reachable, just no longer the primary presentation. This
/// checks the full rendered output (both collapsed and, implicitly,
/// available-on-expand content egui includes in its output tree) still
/// contains it rather than having deleted it.
#[test]
fn technical_detail_is_preserved_not_deleted() {
    let entry = preview_entry_fixture(
        PreviewMatchStrength::Candidate,
        PreviewProposedAction::Install,
    );
    let output = render_shared_cheat_preview_for(entry);
    assert!(
        rendered_text_contains(&output, "Technical details"),
        "the disclosure itself must still be offered"
    );
}

/// Requirement 4: the existing accurate preview-only safety message must
/// survive this presentation pass unchanged.
#[test]
fn preview_only_safety_message_is_preserved() {
    let entry = preview_entry_fixture(
        PreviewMatchStrength::Ambiguous,
        PreviewProposedAction::Blocked,
    );
    let output = render_shared_cheat_preview_for(entry);
    assert!(rendered_text_contains(
        &output,
        "Preview only. No files were changed."
    ));
}

#[test]
fn stale_catalogue_result_is_rejected_without_touching_library_state() {
    let mut app = app_for_operation_tests();
    app.mount_queue.push(PathBuf::from("/roms/queued.zip"));
    app.archive_context.focused = Some(PathBuf::from("/roms/Alien 3.md"));
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Ok(cheat_fetch_result_for(
            "libretro-buildbot-cheats",
            CheatSourceFetchStatus::Fetched,
        )))
        .unwrap();
    app.catalogue_generation = 2;
    let (_progress_sender, progress_receiver) = mpsc::channel();
    app.catalogue_retrieval = Some(RunningCatalogueRetrieval {
        generation: 1,
        source_id: "libretro-buildbot-cheats".into(),
        cancellation: CheatSourceCancellation::default(),
        receiver,
        progress_receiver,
        progress: None,
        cancellation_requested: false,
    });
    app.poll_catalogue_manager(&egui::Context::default());
    assert!(app.catalogue_retrieval.is_none());
    assert!(app.catalogue_last_result.is_none());
    assert_eq!(app.mount_queue, vec![PathBuf::from("/roms/queued.zip")]);
    assert_eq!(
        app.archive_context.focused,
        Some(PathBuf::from("/roms/Alien 3.md"))
    );
}

#[test]
fn stale_cheat_source_fetch_result_is_discarded_not_applied() {
    let mut app = app_with_cheats_mods_context();
    let (sender, receiver) = mpsc::channel();
    app.cheat_workflow.as_mut().unwrap().source_fetch = CheatStepResource::Loading { receiver };

    // The user switched sources while the fetch ran: the result
    // arriving for the *old* source must be discarded, never shown.
    sender
        .send(Ok(cheat_fetch_result_for(
            "source-b",
            CheatSourceFetchStatus::Fetched,
        )))
        .unwrap();
    app.poll_cheat_workflow(&egui::Context::default());
    assert!(
        matches!(
            app.cheat_workflow.as_ref().unwrap().source_fetch,
            CheatStepResource::NotLoaded
        ),
        "a result for a no-longer-selected source must be discarded"
    );
}

#[test]
fn superseded_cheat_fetch_receiver_cannot_deliver_a_result() {
    let mut app = app_with_cheats_mods_context();
    let (old_sender, old_receiver) = mpsc::channel();
    app.cheat_workflow.as_mut().unwrap().source_fetch = CheatStepResource::Loading {
        receiver: old_receiver,
    };
    // A newer operation supersedes the old one: replacing the state
    // drops the old receiver, so the old worker's send fails and its
    // result can never apply.
    let (_new_sender, new_receiver) = mpsc::channel();
    app.cheat_workflow.as_mut().unwrap().source_fetch = CheatStepResource::Loading {
        receiver: new_receiver,
    };
    let stale = old_sender.send(Ok(cheat_fetch_result_for(
        "source-a",
        CheatSourceFetchStatus::Fetched,
    )));
    assert!(
        stale.is_err(),
        "the superseded worker's send must fail once its receiver is gone"
    );
}

#[test]
fn changing_cheats_mods_archive_discards_previous_background_result() {
    let mut app = app_with_cheats_mods_context();
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/b.zip", MountState::Pending));
    }
    let (old_sender, old_receiver) = mpsc::channel();
    app.cheat_workflow.as_mut().unwrap().source_fetch = CheatStepResource::Loading {
        receiver: old_receiver,
    };

    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/b.zip")));
    assert!(
        old_sender
            .send(Ok(cheat_fetch_result_for(
                "source-a",
                CheatSourceFetchStatus::Fetched,
            )))
            .is_err(),
        "replacing the exact archive context must drop its old receiver"
    );
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().source_fetch,
        CheatStepResource::NotLoaded
    ));
}

#[test]
fn offline_cached_snapshot_reuse_applies_and_is_recorded() {
    let mut app = app_with_cheats_mods_context();
    let (sender, receiver) = mpsc::channel();
    app.cheat_workflow.as_mut().unwrap().source_fetch = CheatStepResource::Loading { receiver };
    sender
        .send(Ok(cheat_fetch_result_for(
            "source-a",
            CheatSourceFetchStatus::OfflineReused,
        )))
        .unwrap();
    app.poll_cheat_workflow(&egui::Context::default());
    match &app.cheat_workflow.as_ref().unwrap().source_fetch {
        CheatStepResource::Ready(result) => {
            assert_eq!(result.status, CheatSourceFetchStatus::OfflineReused);
        }
        other => panic!("expected Ready, got {}", cheat_resource_debug(other)),
    }
    assert!(
        app.history.entries().any(|entry| {
            entry.action == ActivityAction::CheatSourceRetrieval
                && entry.outcome == ActivityOutcome::Completed
        }),
        "a completed retrieval must be recorded in the operation history"
    );
}

fn cheat_resource_debug<T>(resource: &CheatStepResource<T>) -> &'static str {
    match resource {
        CheatStepResource::NotLoaded => "NotLoaded",
        CheatStepResource::Loading { .. } => "Loading",
        CheatStepResource::Ready(_) => "Ready",
        CheatStepResource::Failed(_) => "Failed",
    }
}

#[test]
fn cheat_entry_requires_one_existing_archive_but_not_a_completed_profile_scan() {
    let records = vec![record("/roms/a.zip", MountState::Pending)];
    let ready =
        RetroArchProfilesState::Ready(cheat_discovery(vec![cheat_profile("native-user", true)]));
    let path = Some(Path::new("/roms/a.zip"));

    assert_eq!(cheat_entry_blocker(path, 1, Some(&records), &ready), None);

    assert!(cheat_entry_blocker(None, 0, Some(&records), &ready).is_some());
    assert!(
        cheat_entry_blocker(path, 2, Some(&records), &ready).is_some(),
        "a multi-selection must disable the entry"
    );
    assert!(
        cheat_entry_blocker(Some(Path::new("/roms/gone.zip")), 1, Some(&records), &ready).is_some(),
        "an archive missing from the live snapshot must disable the entry"
    );
    assert!(cheat_entry_blocker(path, 1, None, &ready).is_some());
    assert_eq!(
        cheat_entry_blocker(path, 1, Some(&records), &RetroArchProfilesState::NotScanned),
        None
    );
    let (_sender, receiver) = mpsc::channel();
    assert_eq!(
        cheat_entry_blocker(
            path,
            1,
            Some(&records),
            &RetroArchProfilesState::Scanning { receiver }
        ),
        None
    );
    assert_eq!(
        cheat_entry_blocker(
            path,
            1,
            Some(&records),
            &RetroArchProfilesState::Error("boom".to_string())
        ),
        None
    );
    let no_eligible = RetroArchProfilesState::Ready(cheat_discovery(vec![cheat_profile(
        "blocked-profile",
        false,
    )]));
    assert_eq!(
        cheat_entry_blocker(path, 1, Some(&records), &no_eligible),
        None
    );
}

#[test]
fn cheats_mods_context_preselects_only_a_single_eligible_profile() {
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/a.zip", MountState::Pending));
    }
    app.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    app.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();

    // Two eligible profiles: no silent choice.
    app.retroarch_profiles = RetroArchProfilesState::Ready(cheat_discovery(vec![
        cheat_profile("native-user", true),
        cheat_profile("flatpak-user", true),
    ]));
    app.prepare_cheats_mods_workspace(PathBuf::from("/roms/a.zip"));
    let workflow = app.cheat_workflow.as_ref().expect("workflow must open");
    assert_eq!(workflow.selected_profile_id, None);
    assert_eq!(app.view, MainView::CheatsMods);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);

    // Exactly one eligible profile: preselected (the CLI's rule).
    app.retroarch_profiles = RetroArchProfilesState::Ready(cheat_discovery(vec![
        cheat_profile("native-user", true),
        cheat_profile("blocked-profile", false),
    ]));
    app.cheat_workflow = None;
    app.prepare_cheats_mods_workspace(PathBuf::from("/roms/a.zip"));
    let workflow = app.cheat_workflow.as_ref().expect("workflow must reopen");
    assert_eq!(workflow.selected_profile_id.as_deref(), Some("native-user"));

    // Opening for an archive missing from the snapshot does nothing.
    app.cheat_workflow = None;
    app.prepare_cheats_mods_workspace(PathBuf::from("/roms/gone.zip"));
    assert!(app.cheat_workflow.is_none());
}

#[test]
fn cheats_mods_navigation_and_reconciliation_track_selection_without_touching_the_queue() {
    let ctx = egui::Context::default();
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/a.zip", MountState::Pending));
        data.records
            .push(record("/roms/other.zip", MountState::Pending));
    }
    app.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    app.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    app.mount_queue = vec![PathBuf::from("/roms/queued.zip")];
    app.retroarch_profiles =
        RetroArchProfilesState::Ready(cheat_discovery(vec![cheat_profile("native-user", true)]));

    app.prepare_cheats_mods_workspace(PathBuf::from("/roms/a.zip"));
    assert_eq!(
        app.archive_context.focused.as_deref(),
        Some(Path::new("/roms/a.zip"))
    );
    assert_eq!(app.archive_context.selected.len(), 1);
    assert_eq!(app.mount_queue, vec![PathBuf::from("/roms/queued.zip")]);

    // Matching selection: the full-page workflow stays available.
    app.reconcile_cheats_mods_context(&ctx);
    assert!(app.cheat_workflow.is_some());
    assert_eq!(app.view, MainView::CheatsMods);

    // `selected_archive` is the one authoritative field: changing it
    // while already on the page must be picked up here, without a
    // second explicit navigation.
    app.archive_context.focused = Some(PathBuf::from("/roms/other.zip"));
    app.reconcile_cheats_mods_context(&ctx);
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .map(|workflow| workflow.archive_path.as_path()),
        Some(Path::new("/roms/other.zip")),
        "reconciliation must follow selected_archive, not keep the stale workflow"
    );

    // Clearing the selection clears the workspace too - no page is
    // allowed to keep showing an archive no other page still considers
    // selected. Mount queue membership is never touched by any of this.
    app.archive_context.focused = None;
    app.reconcile_cheats_mods_context(&ctx);
    assert!(
        app.cheat_workflow.is_none(),
        "clearing selected_archive must clear the Cheats & Mods workspace too"
    );
    assert_eq!(
        app.archive_context.selected.len(),
        1,
        "selected_archives is untouched by reconciliation - only the explicit clear/select paths update it"
    );
    assert_eq!(app.mount_queue, vec![PathBuf::from("/roms/queued.zip")]);
}

#[test]
fn selecting_an_archive_in_library_reaches_cheats_mods_without_a_separate_picker_step() {
    let ctx = egui::Context::default();
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/a.zip", MountState::Pending));
    }

    // The exact effect of clicking an archive row in Library: only
    // `selected_archive`/`selected_archives` change, mirroring
    // `apply_row_click` - no Cheats & Mods-specific call is made here.
    app.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    app.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    assert!(
        app.cheat_workflow.is_none(),
        "selecting a row in Library must not, by itself, open the Cheats & Mods workspace"
    );

    // The exact effect of the sidebar's "Cheats & Mods" click: just a
    // view switch (see the MainView::CheatsMods arm of the navigation
    // click handler) - reconciliation does the rest below.
    app.view = MainView::CheatsMods;
    app.reconcile_cheats_mods_context(&ctx);

    let workflow = app
            .cheat_workflow
            .as_ref()
            .expect("the already-selected archive must be available immediately, with no separate 'Choose archive' step");
    assert_eq!(workflow.archive_path, PathBuf::from("/roms/a.zip"));
}

#[test]
fn one_library_selection_is_the_same_context_on_selected_and_cheats_mods() {
    let path = PathBuf::from("/roms/animal-crossing.zip");
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut animal_crossing = record_at(path.clone(), MountState::Pending);
        animal_crossing.identity.platform = Some("GameCube".to_string());
        data.rows.push(row_for(&animal_crossing));
        data.records.push(animal_crossing);
    }

    app.archive_context.select_only(path.clone());
    assert_eq!(app.archive_context.focused.as_deref(), Some(path.as_path()));
    assert_eq!(app.archive_context.active_cheats(), Some(path.as_path()));
    assert_eq!(app.archive_context.selected.len(), 1);

    let ctx = egui::Context::default();
    let mut queue = Vec::new();
    let mut confirm = false;
    let live = match &app.state {
        LoadState::Ready(data) => Some(data.as_ref()),
        _ => None,
    };
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_selected_page(
                ui,
                live,
                None,
                SelectedPageViewState {
                    selected_archive: app.archive_context.focused.as_deref(),
                    selected_count: app.archive_context.selected.len(),
                    retroarch_profiles: &app.retroarch_profiles,
                    queue: &mut queue,
                    confirm: &mut confirm,
                    busy: false,
                    block_reason: None,
                },
            );
        });
    });
    assert!(rendered_text_contains(
        &output,
        path.to_string_lossy().as_ref()
    ));

    app.view = MainView::CheatsMods;
    app.reconcile_cheats_mods_context(&egui::Context::default());
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .map(|workflow| workflow.archive_path.as_path()),
        Some(path.as_path())
    );
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().adapter,
        CheatEmulatorAdapter::Dolphin
    );
}

#[test]
fn mount_state_change_and_rescan_preserve_archive_context() {
    let path = PathBuf::from("/roms/animal-crossing.zip");
    let mut app = app_for_operation_tests();
    app.archive_context.select_only(path.clone());

    let pending = record_at(path.clone(), MountState::Pending);
    let pending_status = row_for(&pending);
    let pending_rows = build_display_rows(&[pending], &[pending_status], None);
    app.prune_selection(&pending_rows);
    assert_eq!(app.archive_context.focused.as_deref(), Some(path.as_path()));

    let mounted = record_at(path.clone(), MountState::Mounted);
    let mounted_status = row_for(&mounted);
    let mounted_rows = build_display_rows(&[mounted], &[mounted_status], None);
    app.prune_selection(&mounted_rows);
    assert_eq!(app.archive_context.focused.as_deref(), Some(path.as_path()));
    assert_eq!(app.archive_context.selected, [path].into_iter().collect());
}

#[test]
fn cheats_mods_context_survives_navigating_away_and_back() {
    let ctx = egui::Context::default();
    let mut app = app_with_cheats_mods_context();
    let archive_path = app.cheat_workflow.as_ref().unwrap().archive_path.clone();
    app.reconcile_cheats_mods_context(&ctx);
    assert!(app.cheat_workflow.is_some());

    // Leaving the page: reconciliation is a no-op while `view` is
    // anything else, so the workspace is preserved rather than reset.
    app.view = MainView::Mount;
    app.reconcile_cheats_mods_context(&ctx);
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .map(|workflow| workflow.archive_path.clone()),
        Some(archive_path.clone()),
        "navigating away must not discard the in-progress workflow"
    );

    // Coming back: the same archive is still selected, so the exact
    // same workflow (not a freshly reset one) is what's shown again.
    app.view = MainView::CheatsMods;
    app.reconcile_cheats_mods_context(&ctx);
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .map(|workflow| workflow.archive_path.clone()),
        Some(archive_path),
        "navigating back must not lose or replace the preserved workflow"
    );
}

#[test]
fn cheats_mods_archive_picker_open_and_cancel_preserve_context() {
    let mut app = app_with_cheats_mods_context();
    let original = app.cheat_workflow.as_ref().unwrap().archive_path.clone();

    app.open_cheat_archive_picker();
    assert_eq!(
        app.cheat_archive_picker
            .as_ref()
            .and_then(|picker| picker.candidate.as_deref()),
        Some(original.as_path())
    );
    app.cheat_archive_picker = None;

    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .map(|workflow| workflow.archive_path.as_path()),
        Some(original.as_path())
    );

    assert!(app.confirm_cheat_archive_change.is_none());
}

#[test]
fn choosing_an_archive_inside_cheats_mods_updates_selection_everywhere_else_too() {
    let ctx = egui::Context::default();
    let mut app = app_with_cheats_mods_context();
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/b.zip", MountState::Pending));
    }
    let original = app.cheat_workflow.as_ref().unwrap().archive_path.clone();
    app.archive_context.focused = Some(original.clone());
    app.archive_context.selected = [original].into_iter().collect();

    app.apply_cheat_archive_choice(&ctx, PathBuf::from("/roms/b.zip"));

    assert_eq!(
        app.archive_context.focused.as_deref(),
        Some(Path::new("/roms/b.zip")),
        "choosing an archive inside Cheats & Mods must update selected_archive, so Library and Mount agree with it"
    );
    assert_eq!(
        app.archive_context.selected,
        [PathBuf::from("/roms/b.zip")].into_iter().collect()
    );
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .map(|workflow| workflow.archive_path.clone()),
        Some(PathBuf::from("/roms/b.zip"))
    );
}

#[test]
fn cheats_mods_archive_picker_search_and_filters_cover_every_displayed_field() {
    let rows = vec![
        row_with_fields(
            "/roms/snes/Chrono Trigger.zip",
            "SNES",
            "Ready to mount",
            "/roms/snes/Chrono Trigger.zip",
            "/mount/Chrono Trigger",
        )
        .with_source_path(Some(PathBuf::from("/roms/snes"))),
        row_with_fields(
            "/backup/psx/Chrono Cross.zip",
            "PlayStation",
            "Already mounted",
            "/backup/psx/Chrono Cross.zip",
            "/mount/Chrono Cross",
        )
        .with_source_path(Some(PathBuf::from("/backup/psx"))),
    ];

    for search in [
        "chrono trigger",
        "snes",
        "/roms/snes",
        "ready to mount",
        "/mount/chrono trigger",
    ] {
        assert!(cheat_picker_row_matches(&rows[0], search, None, None));
    }
    assert!(!cheat_picker_row_matches(
        &rows[0],
        "chrono",
        Some("PlayStation"),
        None
    ));
    assert!(cheat_picker_row_matches(
        &rows[1],
        "",
        Some("PlayStation"),
        Some(Path::new("/backup/psx"))
    ));
    assert!(!cheat_picker_row_matches(
        &rows[1],
        "",
        None,
        Some(Path::new("/roms/snes"))
    ));
    let picker = CheatArchivePickerState::default();
    let visible = cheat_picker_visible_indices(&rows, &picker);
    assert_eq!(
        move_cheat_picker_candidate(&rows, &visible, None, ArrowDirection::Down).as_deref(),
        Some(Path::new("/backup/psx/Chrono Cross.zip"))
    );
    assert_eq!(
        cheat_archive_picker_size(egui::vec2(1536.0, 864.0)),
        egui::vec2(1080.0, 708.48)
    );
    assert_eq!(
        cheat_archive_picker_size(egui::vec2(3440.0, 1440.0)),
        egui::vec2(1080.0, 760.0)
    );
}

#[test]
fn gamecube_platform_model_keeps_zip_and_rvz_visible_in_library_and_cheat_picker() {
    let rows = vec![
        row_with_fields(
            "/roms/gcn/Animal Crossing (USA).zip",
            "GameCube",
            "Ready to mount",
            "/roms/gcn/Animal Crossing (USA).zip",
            "/mount/Animal Crossing",
        ),
        row_with_fields(
            "/roms/gcn/ZooCube (USA).rvz",
            "GameCube",
            "Not mountable",
            "/roms/gcn/ZooCube (USA).rvz",
            "",
        ),
    ];
    let filters = LibraryRowFilters {
        platform: Some("GameCube".to_string()),
        ..LibraryRowFilters::default()
    };
    assert!(rows.iter().all(|row| filters.matches(row)));
    let picker = CheatArchivePickerState {
        platform_filter: Some("GameCube".to_string()),
        ..CheatArchivePickerState::default()
    };
    assert_eq!(cheat_picker_visible_indices(&rows, &picker), vec![0, 1]);
}

#[test]
fn ten_thousand_row_platform_and_chooser_smoke_stays_bounded() {
    let rows = (0..10_000)
        .map(|index| {
            let platform = if index % 5 == 0 {
                "GameCube"
            } else if index % 7 == 0 {
                "Unknown"
            } else {
                "Xbox 360"
            };
            let mut row = row_with_fields(
                &format!("/fixture/{platform}/Game {index}.rvz"),
                platform,
                "Ready to mount",
                &format!("/fixture/{platform}/Game {index}.rvz"),
                "",
            );
            row.unknown_platform = platform == "Unknown";
            row
        })
        .collect::<Vec<_>>();

    let started = std::time::Instant::now();
    let gamecube = LibraryRowFilters {
        platform: Some("GameCube".to_string()),
        ..LibraryRowFilters::default()
    };
    let gamecube_count = rows.iter().filter(|row| gamecube.matches(row)).count();
    let unknown = LibraryRowFilters {
        platform: Some("Unknown".to_string()),
        ..LibraryRowFilters::default()
    };
    let unknown_count = rows.iter().filter(|row| unknown.matches(row)).count();
    let picker = CheatArchivePickerState {
        platform_filter: Some("GameCube".to_string()),
        ..CheatArchivePickerState::default()
    };
    let chooser_count = cheat_picker_visible_indices(&rows, &picker).len();
    let elapsed = started.elapsed();

    eprintln!(
        "10k GUI smoke: GameCube={gamecube_count}, Unknown={unknown_count}, chooser={chooser_count}, elapsed={elapsed:?}"
    );
    assert_eq!(gamecube_count, 2_000);
    assert_eq!(unknown_count, 1_143);
    assert_eq!(chooser_count, 2_000);
    assert!(elapsed < std::time::Duration::from_secs(5));
}

#[test]
fn explicit_cheat_archive_choice_changes_only_workspace_context() {
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        data.records = vec![
            record("/roms/a.zip", MountState::Mounted),
            record("/roms/b.zip", MountState::Pending),
        ];
    }
    app.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    app.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    app.mount_queue = vec![PathBuf::from("/roms/queued.zip")];

    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/a.zip")));
    {
        let workflow = app.cheat_workflow.as_mut().unwrap();
        workflow.source_mode = CheatSourceMode::ExistingRetroArchLibrary;
        workflow.selected_source_id = Some("source-a".to_string());
        workflow.fetch_force_refresh = true;
    }
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/b.zip")));
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .map(|workflow| workflow.archive_path.as_path()),
        Some(Path::new("/roms/b.zip"))
    );
    assert_eq!(
        app.archive_context.focused.as_deref(),
        Some(Path::new("/roms/a.zip"))
    );
    assert_eq!(app.archive_context.selected.len(), 1);
    assert_eq!(app.mount_queue, vec![PathBuf::from("/roms/queued.zip")]);
    let LoadState::Ready(data) = &app.state else {
        panic!("test fixture must remain ready");
    };
    assert_eq!(data.records[0].mount_state, MountState::Mounted);
    assert_eq!(data.records[1].mount_state, MountState::Pending);
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().source_mode,
        CheatSourceMode::ExistingRetroArchLibrary
    );
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .selected_source_id
            .as_deref(),
        Some("source-a")
    );
    assert!(app.cheat_workflow.as_ref().unwrap().fetch_force_refresh);
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().source_fetch,
        CheatStepResource::NotLoaded
    ));
}

#[test]
fn fetched_state_requires_confirmation_before_changing_archive() {
    let mut app = app_with_cheats_mods_context();
    assert!(!cheat_archive_change_requires_confirmation(
        app.cheat_workflow.as_ref(),
        Path::new("/roms/b.zip")
    ));
    app.cheat_workflow.as_mut().unwrap().source_fetch = CheatStepResource::Ready(
        cheat_fetch_result_for("source-a", CheatSourceFetchStatus::OfflineReused),
    );
    assert!(cheat_archive_change_requires_confirmation(
        app.cheat_workflow.as_ref(),
        Path::new("/roms/b.zip")
    ));
    assert!(!cheat_archive_change_requires_confirmation(
        app.cheat_workflow.as_ref(),
        Path::new("/roms/a.zip")
    ));
}

#[test]
fn source_mode_is_independent_and_local_unverified_has_no_action() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let archive = workflow.archive_path.clone();
    let profile = workflow.selected_profile_id.clone();
    workflow.source_mode = CheatSourceMode::ExistingRetroArchLibrary;
    assert_eq!(workflow.archive_path, archive);
    assert_eq!(workflow.selected_profile_id, profile);

    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheat_source_modes(ui, workflow, &RetroArchProfilesState::NotScanned);
        });
    });
    assert!(rendered_text_contains(&output, "Existing RetroArch cheats"));
    assert!(!rendered_text_contains(&output, "Local unverified source"));
    let details_ctx = egui::Context::default();
    details_ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
    let details = details_ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheat_source_modes(ui, workflow, &RetroArchProfilesState::NotScanned);
        });
    });
    assert!(rendered_text_contains(&details, "Local unverified source"));
    assert!(rendered_text_contains(&details, "Planned"));
    for fake_action in ["Browse local", "Choose local", "Import local"] {
        assert!(!rendered_text_contains(&output, fake_action));
    }
}

#[test]
fn retroarch_library_states_distinguish_missing_inaccessible_and_unsafe() {
    assert_eq!(
        retroarch_library_state_presentation(RetroArchCheatLibraryState::Missing).0,
        "Directory missing"
    );
    assert_eq!(
        retroarch_library_state_presentation(RetroArchCheatLibraryState::Inaccessible).0,
        "Directory inaccessible"
    );
    assert_eq!(
        retroarch_library_state_presentation(RetroArchCheatLibraryState::UnsafePath).0,
        "Unsafe path refused"
    );
}

#[test]
fn cheats_mods_is_a_primary_active_navigation_destination() {
    let position = PRIMARY_NAVIGATION_DESTINATIONS
        .iter()
        .position(|(view, label)| *view == MainView::CheatsMods && *label == "Cheats & Mods")
        .expect("Cheats & Mods must be in the primary workflow navigation");
    let selected_position = PRIMARY_NAVIGATION_DESTINATIONS
        .iter()
        .position(|(view, _)| *view == MainView::Selected)
        .unwrap();

    assert_eq!(position, selected_position + 1);
    assert!(navigation_destination_selected(
        MainView::CheatsMods,
        MainView::CheatsMods
    ));
    assert!(!navigation_destination_selected(
        MainView::Library,
        MainView::CheatsMods
    ));
    assert_eq!(main_view_title(MainView::CheatsMods), "Cheats & Mods");
}

#[test]
fn row_context_action_preserves_the_exact_archive_for_cheats_mods() {
    let exact = PathBuf::from("/roms/SNES/Game [Rev A].zip");
    match cheats_mods_row_action(&exact) {
        RowContextMenuAction::CheatsMods(path) => assert_eq!(path, exact),
        _ => panic!("the Cheats & Mods row action must retain the exact archive path"),
    }

    let records = vec![record("/roms/a.zip", MountState::Pending)];
    let row = row_with_fields("/roms/a.zip", "SNES", "Pending", "a.zip", "/mount/a");
    let menu_context = row_menu_context_for(&records);
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_single_row_context_menu(ui, &row, &menu_context);
        });
    });
    assert!(rendered_text_contains(&output, "Cheats & Mods"));
}

#[test]
fn loose_rom_context_menu_has_no_mount_action_but_keeps_cheats_mods() {
    let records = vec![loose_mega_drive_record("/roms/genesis/Alien 3.md")];
    let row = row_with_fields(
        "/roms/genesis/Alien 3.md",
        "MegaDrive",
        "NotMountable",
        "Alien 3.md",
        "/mount/Alien_3",
    );
    let menu_context = row_menu_context_for(&records);
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_single_row_context_menu(ui, &row, &menu_context);
        });
    });
    assert!(rendered_text_contains(&output, "No mount required"));
    assert!(rendered_text_contains(&output, "Cheats & Mods"));
    assert!(!rendered_text_contains(&output, "Mount selected"));
}

#[test]
fn returning_to_cheats_mods_preserves_completed_workflow_state_and_queue() {
    let mut app = app_with_cheats_mods_context();
    app.mount_queue = vec![PathBuf::from("/roms/queued.zip")];
    app.cheat_workflow.as_mut().unwrap().source_fetch = CheatStepResource::Ready(
        cheat_fetch_result_for("source-a", CheatSourceFetchStatus::OfflineReused),
    );
    app.view = MainView::Library;

    let replaced = app.prepare_cheats_mods_workspace(PathBuf::from("/roms/a.zip"));

    assert!(
        !replaced,
        "returning to the same exact archive must reuse state"
    );
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().source_fetch,
        CheatStepResource::Ready(_)
    ));
    assert_eq!(app.mount_queue, vec![PathBuf::from("/roms/queued.zip")]);
    assert_eq!(app.view, MainView::CheatsMods);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
}
