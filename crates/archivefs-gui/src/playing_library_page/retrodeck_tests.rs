use super::*;

#[test]
fn retrodeck_campaign_undo_survives_discarded_preview() {
    let fixture = Fixture::new("retrodeck-campaign-durable-undo");
    let (mut state, original, _) = preview_a_single_election(&fixture);
    state.dat_platform_identity = Some(DatPlatformIdentity::Resolved {
        platform: "PSX".into(),
        machine_key: None,
        confidence: DatPlatformConfidence::Strong,
        evidence: vec![],
    });
    install_retrodeck_projection(
        &mut state,
        &fixture,
        RetroDeckVisibility::verified_same_path_bind(
            fixture.path("roms"),
            fixture.path("retrodeck-destination"),
        )
        .unwrap(),
    );
    let gamelist = state
        .retrodeck_projection
        .as_ref()
        .unwrap()
        .es_de_publication
        .gamelist_path
        .clone();
    let before = std::fs::read(&gamelist).unwrap();
    state.request_retrodeck_apply();
    state.confirm_retrodeck_apply();
    assert!(
        state.retrodeck_error.is_none(),
        "{:?}",
        state.retrodeck_error
    );
    state.retrodeck_projection = None;
    state.rollback_retrodeck();
    assert_eq!(
        std::fs::read(&gamelist).unwrap(),
        before,
        "Undo must restore metadata without relying on a mutable preview"
    );
    assert_eq!(std::fs::read(original).unwrap(), b"test");
}

#[test]
fn retrodeck_campaign_genuine_preview_block_confirmation_completion_uptodate_and_restart_recovery()
{
    use archivefs_core::launch::es_de_publish::operation::Phase;
    let fixture = Fixture::new("retrodeck-campaign-gui-states");
    let (mut state, source) = reviewed_profile_fixture(&fixture, true);
    let ctx = egui::Context::default();
    let (_, action) = campaign_click(&ctx, &mut state, "Preview RetroDECK Library");
    assert!(matches!(
        action,
        Some(PlayingLibraryPageAction::PreviewRetroDeck)
    ));
    state.preview_retrodeck();
    let (output, _) = render(&ctx, &mut state, campaign_input());
    assert!(rendered_text_contains(&output, "Output profile preview"));
    assert!(rendered_text_contains(&output, "RetroDECK"));
    state.retrodeck_visibility_verified = false;
    state.preview_retrodeck();
    let (output, _) = render(&ctx, &mut state, campaign_input());
    assert!(rendered_text_contains(&output, "Apply is blocked"));
    state.request_retrodeck_apply();
    state.confirm_retrodeck_apply();
    assert!(state.retrodeck_applied.is_none());
    state.retrodeck_visibility_verified = true;
    state.preview_retrodeck();
    let gamelist = state
        .retrodeck_projection
        .as_ref()
        .unwrap()
        .es_de_publication
        .gamelist_path
        .clone();
    let original_xml = std::fs::read(&gamelist).unwrap();
    let (_, action) = campaign_click(&ctx, &mut state, "Apply output profile");
    assert!(matches!(
        action,
        Some(PlayingLibraryPageAction::RequestApply)
    ));
    state.request_retrodeck_apply();
    let (output, _) = render(&ctx, &mut state, campaign_input());
    assert!(rendered_text_contains(&output, "Confirm"));
    assert_eq!(std::fs::read(&gamelist).unwrap(), original_xml);
    let (_, action) = campaign_click(&ctx, &mut state, "Confirm");
    assert!(matches!(
        action,
        Some(PlayingLibraryPageAction::ConfirmApply)
    ));
    state.confirm_retrodeck_apply();
    assert_eq!(
        state.retrodeck_phase,
        Some(Phase::Completed),
        "{:?}",
        state.retrodeck_error
    );
    let (output, _) = render(&ctx, &mut state, campaign_input());
    assert!(rendered_text_contains(&output, "RetroDECK library created"));
    let once = std::fs::read(&gamelist).unwrap();
    state.preview_retrodeck();
    let (output, _) = render(&ctx, &mut state, campaign_input());
    assert!(rendered_text_contains(&output, "already up to date"));
    assert_eq!(std::fs::read(&gamelist).unwrap(), once);
    // A fresh controller obtains its operation from durable disk receipts,
    // with no projection, profile, plan, source draft or application database.
    let mut restart = PlayingLibraryPageState::with_journal_dir(state.journal_dir.clone());
    restart.destination = PlayingLibraryDestination::RetroDeck;
    restart.rediscover_retrodeck_operations();
    assert_eq!(restart.retrodeck_operations.len(), 1);
    let id = restart.retrodeck_operations[0]
        .transaction
        .transaction_id
        .clone();
    let label = format!("{id} · Completed");
    let (_, action) = campaign_click(&ctx, &mut restart, &label);
    assert!(action.is_none());
    assert_eq!(restart.retrodeck_phase, Some(Phase::Completed));
    let (_, action) = campaign_click(&ctx, &mut restart, "Roll back RetroDECK Library");
    assert!(matches!(
        action,
        Some(PlayingLibraryPageAction::RollbackRetroDeck)
    ));
    restart.rollback_retrodeck();
    let (output, _) = render(&ctx, &mut restart, campaign_input());
    assert!(rendered_text_contains(
        &output,
        "RetroDECK library rolled back"
    ));
    assert_eq!(std::fs::read(&gamelist).unwrap(), original_xml);
    assert_eq!(std::fs::read(source).unwrap(), b"test");
}

#[test]
fn retrodeck_campaign_failure_is_honest_and_changed_xml_keeps_recovery_available() {
    use archivefs_core::launch::es_de_publish::operation::Phase;
    let fixture = Fixture::new("retrodeck-campaign-recovery-state");
    let (mut state, source) = reviewed_profile_fixture(&fixture, true);
    apply_reviewed_profile(&mut state, true);
    assert_eq!(state.retrodeck_phase, Some(Phase::Completed));
    let gamelist = state
        .retrodeck_projection
        .as_ref()
        .unwrap()
        .es_de_publication
        .gamelist_path
        .clone();
    std::fs::write(
        &gamelist,
        "<gameList><!-- independent external edit --></gameList>",
    )
    .unwrap();
    state.rollback_retrodeck();
    assert_eq!(state.retrodeck_phase, Some(Phase::RecoveryRequired));
    let ctx = egui::Context::default();
    let (output, _) = render(&ctx, &mut state, campaign_input());
    assert!(rendered_text_contains(
        &output,
        "RetroDECK recovery required"
    ));
    assert!(rendered_text_contains(&output, "Recover RetroDECK Library"));
    assert!(!rendered_text_contains(
        &output,
        "links and game list committed"
    ));
    assert_eq!(std::fs::read(source).unwrap(), b"test");
    assert_eq!(state.retrodeck_operations.len(), 1);
}

#[test]
fn retrodeck_campaign_interrupted_native_apply_renders_recovery_and_restores_after_restart() {
    use archivefs_core::launch::es_de_publish::operation::{self, Phase};
    let fixture = Fixture::new("retrodeck-campaign-native-interruption");
    let (mut state, source) = reviewed_profile_fixture(&fixture, true);
    let gamelist = state
        .retrodeck_projection
        .as_ref()
        .unwrap()
        .es_de_publication
        .gamelist_path
        .clone();
    let before = std::fs::read(&gamelist).unwrap();
    operation::set_test_fault(Some("before_completion"));
    apply_reviewed_profile(&mut state, true);
    operation::set_test_fault(None);
    assert_eq!(state.retrodeck_phase, Some(Phase::MetadataPublishing));
    let ctx = egui::Context::default();
    let (output, _) = render(&ctx, &mut state, campaign_input());
    assert!(rendered_text_contains(
        &output,
        "RetroDECK recovery required"
    ));
    assert!(!rendered_text_contains(
        &output,
        "links and game list committed"
    ));
    let mut restarted = PlayingLibraryPageState::with_journal_dir(state.journal_dir.clone());
    restarted.destination = PlayingLibraryDestination::RetroDeck;
    restarted.rediscover_retrodeck_operations();
    let label = format!(
        "{} · MetadataPublishing",
        restarted.retrodeck_operations[0].transaction.transaction_id
    );
    campaign_click(&ctx, &mut restarted, &label);
    let (_, action) = campaign_click(&ctx, &mut restarted, "Recover RetroDECK Library");
    assert!(matches!(
        action,
        Some(PlayingLibraryPageAction::RollbackRetroDeck)
    ));
    restarted.rollback_retrodeck();
    assert_eq!(restarted.retrodeck_phase, Some(Phase::Restored));
    assert_eq!(std::fs::read(&gamelist).unwrap(), before);
    assert_eq!(std::fs::read(source).unwrap(), b"test");
}

#[test]
fn retrodeck_campaign_post_link_failure_renders_rolled_back_and_preserves_xml() {
    use archivefs_core::launch::es_de_publish::operation::{self, Phase};
    let fixture = Fixture::new("retrodeck-campaign-post-link-failure");
    let (mut state, source) = reviewed_profile_fixture(&fixture, true);
    let projection = state.retrodeck_projection.as_ref().unwrap().clone();
    let before = std::fs::read(&projection.es_de_publication.gamelist_path).unwrap();
    operation::set_test_fault(Some("before_metadata"));
    apply_reviewed_profile(&mut state, true);
    operation::set_test_fault(None);
    assert_eq!(
        state.retrodeck_phase,
        Some(Phase::Restored),
        "{:?}",
        state.retrodeck_error
    );
    assert!(state.retrodeck_error.is_some());
    for op in &projection.playing_library_plan.operations {
        assert!(!op.destination_path.exists());
    }
    assert_eq!(
        std::fs::read(&projection.es_de_publication.gamelist_path).unwrap(),
        before
    );
    assert_eq!(std::fs::read(source).unwrap(), b"test");
    let ctx = egui::Context::default();
    let (output, _) = render(&ctx, &mut state, campaign_input());
    assert!(rendered_text_contains(
        &output,
        "RetroDECK library rolled back"
    ));
    assert!(rendered_text_contains(&output, "injected before_metadata"));
    assert!(!rendered_text_contains(
        &output,
        "links and game list committed"
    ));
}

fn campaign_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 3600.0),
        )),
        ..Default::default()
    }
}
fn campaign_click(
    ctx: &egui::Context,
    state: &mut PlayingLibraryPageState,
    label: &str,
) -> (egui::FullOutput, Option<PlayingLibraryPageAction>) {
    let (output, _) = render(ctx, state, campaign_input());
    let pos = find_exact_text_center(&output, label)
        .unwrap_or_else(|| panic!("missing genuine widget {label}"));
    let mut input = campaign_input();
    input.events = click_event(pos);
    let (_, action) = render(ctx, state, input);
    let (settled, _) = render(ctx, state, campaign_input());
    (settled, action)
}

#[test]
fn retrodeck_campaign_changed_output_refuses_combined_undo_without_touching_foreign_file() {
    use archivefs_core::launch::es_de_publish::operation::Phase;
    let fixture = Fixture::new("retrodeck-campaign-foreign-output");
    let (mut state, source) = reviewed_profile_fixture(&fixture, true);
    apply_reviewed_profile(&mut state, true);
    let output = state.retrodeck_projection.as_ref().unwrap().games[0]
        .launcher
        .destination_path
        .clone();
    std::fs::rename(&output, fixture.path("saved-owned-link")).unwrap();
    std::fs::write(&output, b"foreign output").unwrap();
    state.rollback_retrodeck();
    assert_eq!(state.retrodeck_phase, Some(Phase::RecoveryRequired));
    assert!(state.retrodeck_error.is_some());
    assert_eq!(std::fs::read(output).unwrap(), b"foreign output");
    assert_eq!(std::fs::read(source).unwrap(), b"test");
    let (frame, _) = render(&egui::Context::default(), &mut state, campaign_input());
    assert!(rendered_text_contains(
        &frame,
        "RetroDECK recovery required"
    ));
    assert!(!rendered_text_contains(
        &frame,
        "links and game list committed"
    ));
}
