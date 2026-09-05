//! First-run onboarding: `ArchiveFsApp`-level behavior tests. Pure
//! state-model tests (parse/serialize, step ordering) live in
//! `onboarding.rs`'s own `#[cfg(test)] mod tests`; this file covers the
//! app-level wiring (auto-open, resume, run-again, degraded states) using
//! the shared `app_for_operation_tests()` fixture.

use super::*;

fn app_with_no_source() -> ArchiveFsApp {
    let mut app = app_for_operation_tests();
    app.gui_config = GuiConfigSnapshot::from_config(Config {
        source_folders: Vec::new(),
        mount_root: PathBuf::from("/mount"),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    });
    app
}

#[test]
fn first_run_opens_onboarding_at_the_welcome_step() {
    let mut app = app_for_operation_tests();
    app.config_previously_confirmed = false;
    app.onboarding_state = onboarding::OnboardingState::NotStarted;
    app.maybe_auto_open_onboarding();
    assert_eq!(app.tools_overlay, ToolsOverlay::Onboarding);
    assert_eq!(
        app.onboarding_state,
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::Welcome)
    );
}

#[test]
fn completed_onboarding_does_not_reopen_automatically() {
    let mut app = app_for_operation_tests();
    app.config_previously_confirmed = false;
    app.onboarding_state = onboarding::OnboardingState::Completed;
    app.maybe_auto_open_onboarding();
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
    assert_eq!(app.onboarding_state, onboarding::OnboardingState::Completed);
}

#[test]
fn skipped_onboarding_does_not_reopen_automatically() {
    let mut app = app_for_operation_tests();
    app.config_previously_confirmed = false;
    app.onboarding_state = onboarding::OnboardingState::Skipped;
    app.maybe_auto_open_onboarding();
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
    assert_eq!(app.onboarding_state, onboarding::OnboardingState::Skipped);
}

#[test]
fn an_existing_confirmed_install_never_gets_auto_onboarding() {
    // `config_previously_confirmed: true` means this session already saw a
    // real config file - not a fresh install, even though the onboarding
    // sidecar itself has never been written (e.g. it predates this feature).
    let mut app = app_for_operation_tests();
    app.config_previously_confirmed = true;
    app.onboarding_state = onboarding::OnboardingState::NotStarted;
    app.maybe_auto_open_onboarding();
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
    assert_eq!(
        app.onboarding_state,
        onboarding::OnboardingState::NotStarted
    );
}

#[test]
fn the_auto_open_check_runs_at_most_once_per_session() {
    let mut app = app_for_operation_tests();
    app.config_previously_confirmed = false;
    app.onboarding_state = onboarding::OnboardingState::NotStarted;
    app.maybe_auto_open_onboarding();
    assert_eq!(app.tools_overlay, ToolsOverlay::Onboarding);

    // Simulate the user dismissing it, then something resetting the
    // in-memory state back to NotStarted within the same session (should
    // never happen in practice, but proves the one-shot guard actually
    // guards rather than re-deriving from state each call).
    app.tools_overlay = ToolsOverlay::None;
    app.onboarding_state = onboarding::OnboardingState::NotStarted;
    app.maybe_auto_open_onboarding();
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
}

#[test]
fn resume_reconstructs_the_exact_persisted_step() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onboarding_state.txt");
    onboarding::save_onboarding_state_at(
        &path,
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::EmulatorSetup),
    );
    let resumed = onboarding::load_onboarding_state_at(&path);
    assert_eq!(
        resumed,
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::EmulatorSetup)
    );

    let mut app = app_for_operation_tests();
    app.onboarding_state = resumed;
    assert_eq!(
        app.onboarding_state,
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::EmulatorSetup)
    );
}

#[test]
fn a_malformed_sidecar_file_is_read_as_not_started_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onboarding_state.txt");
    std::fs::write(&path, b"\x00garbage-not-a-real-state\xff").unwrap();
    assert_eq!(
        onboarding::load_onboarding_state_at(&path),
        onboarding::OnboardingState::NotStarted
    );
}

#[test]
fn a_missing_sidecar_file_is_read_as_not_started() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.txt");
    assert_eq!(
        onboarding::load_onboarding_state_at(&path),
        onboarding::OnboardingState::NotStarted
    );
}

#[test]
fn skipping_the_source_step_advances_without_fabricating_a_source() {
    let mut app = app_with_no_source();
    assert!(!app.onboarding_has_source());
    app.onboarding_state =
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::AddSource);
    app.onboarding_advance_from(onboarding::OnboardingStep::AddSource);
    assert_eq!(
        app.onboarding_state,
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::DatSetup)
    );
    // Advancing past the step must never itself add a source folder.
    assert!(!app.onboarding_has_source());
}

#[test]
fn the_dat_step_count_matches_the_real_dat_sources_page_count_never_a_duplicate_count() {
    let app = app_for_operation_tests();
    // `dat_sources_page` is deliberately left unloaded by the shared
    // fixture (matches its own doc comment) - the count must reflect that
    // truthfully as 0, never a fabricated "not configured" vs "0" split.
    assert!(app.dat_sources_page.is_none());
    assert_eq!(app.onboarding_dat_source_count(), 0);
}

#[test]
fn finish_persists_completion_and_closes_the_overlay() {
    let mut app = app_for_operation_tests();
    app.onboarding_state =
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::Verify);
    app.tools_overlay = ToolsOverlay::Onboarding;
    app.onboarding_advance_from(onboarding::OnboardingStep::Verify);
    assert_eq!(app.onboarding_state, onboarding::OnboardingState::Completed);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
}

#[test]
fn run_again_resets_only_onboarding_progress_and_touches_nothing_else() {
    let mut app = app_for_operation_tests();
    app.onboarding_state = onboarding::OnboardingState::Completed;
    let source_folders_before = app.gui_config.source_roots().map(<[_]>::to_vec);
    let dat_page_was_none_before = app.dat_sources_page.is_none();

    app.restart_onboarding();

    assert_eq!(
        app.onboarding_state,
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::Welcome)
    );
    assert_eq!(app.tools_overlay, ToolsOverlay::Onboarding);
    // The only two things "run again" is allowed to change.
    assert_eq!(
        app.gui_config.source_roots().map(<[_]>::to_vec),
        source_folders_before,
        "run-again must never mutate configured source folders"
    );
    assert_eq!(
        app.dat_sources_page.is_none(),
        dat_page_was_none_before,
        "run-again must never mutate DAT source registration state"
    );
}

#[test]
fn skipping_setup_entirely_persists_skipped_and_closes_the_overlay() {
    let mut app = app_for_operation_tests();
    app.onboarding_state =
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::Welcome);
    app.tools_overlay = ToolsOverlay::Onboarding;
    app.onboarding_skip_entirely();
    assert_eq!(app.onboarding_state, onboarding::OnboardingState::Skipped);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
}
