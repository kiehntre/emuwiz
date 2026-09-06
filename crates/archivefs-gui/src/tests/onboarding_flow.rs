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
    let context = egui::Context::default();
    app.onboarding_advance_from(&context, onboarding::OnboardingStep::AddSource);
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
    let context = egui::Context::default();
    app.onboarding_advance_from(&context, onboarding::OnboardingStep::Verify);
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
    let context = egui::Context::default();
    app.onboarding_skip_entirely(&context);
    assert_eq!(app.onboarding_state, onboarding::OnboardingState::Skipped);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
}

// --- P0 regression: same-session Home load after onboarding completion ----
//
// Reproduces the exact real defect (see docs/APPIMAGE_FRESH_INSTALL_QA.md's
// "restart/persistence result" finding): `ArchiveFsApp::new()`'s very first
// archive-snapshot load runs before onboarding ever writes a config file, so
// it resolves - once, terminally - before the user finishes onboarding.
// Nothing else in the onboarding flow ever reloads that snapshot (adding a
// source only reloads the separate `database_state`, used by Advanced
// View's Library tab, never Gamer View's own snapshot), so the stale
// terminal result stayed in place forever, and Gamer/Home View cannot tell
// a terminal `Error` apart from one still in flight - both render the
// permanent "Loading your games..." spinner. A fresh process relaunch
// against the identical on-disk state loaded instantly because `new()`'s
// *own* first load then succeeded (the config now exists) - this is what
// made it a same-session-only, no-restart-required defect.
//
// These tests seed exactly that pre-condition (a stale, already-resolved
// `LoadState`, never a `Loading` in flight) and call the real
// `onboarding_advance_from`/`onboarding_skip_entirely` production methods -
// no restart is simulated and no state is manually forced to `Ready`.

#[test]
fn finishing_onboarding_in_the_same_session_retries_the_stale_archive_load() {
    let mut app = app_for_operation_tests();
    // The exact real pre-condition: the first, pre-onboarding load already
    // finished (its worker thread has already exited) with an error, because
    // no config file existed yet at that moment.
    app.state = LoadState::Error("configuration file is missing".to_string());
    app.onboarding_state =
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::Verify);
    app.tools_overlay = ToolsOverlay::Onboarding;
    let generation_before = app.refresh_generation;

    let context = egui::Context::default();
    app.onboarding_advance_from(&context, onboarding::OnboardingStep::Verify);

    assert_eq!(app.onboarding_state, onboarding::OnboardingState::Completed);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
    match &app.state {
        LoadState::Loading { generation, .. } => {
            assert_ne!(
                *generation, generation_before,
                "finishing onboarding must start a genuinely new load, not \
                 leave the stale pre-onboarding attempt in place"
            );
            assert_eq!(
                *generation, app.refresh_generation,
                "the freshly-started load's generation must be the one \
                 poll_load will actually accept when its result arrives"
            );
        }
        LoadState::Ready(_) => panic!(
            "expected a fresh Loading state (a new worker was just spawned), \
             not an already-resolved Ready value"
        ),
        LoadState::Error(message) => panic!(
            "onboarding completion left the stale terminal Error in place \
             instead of retrying the load: {message}"
        ),
    }
}

#[test]
fn skipping_onboarding_entirely_also_retries_the_stale_archive_load() {
    let mut app = app_for_operation_tests();
    app.state = LoadState::Error("configuration file is missing".to_string());
    app.onboarding_state =
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::Welcome);
    app.tools_overlay = ToolsOverlay::Onboarding;
    let generation_before = app.refresh_generation;

    let context = egui::Context::default();
    app.onboarding_skip_entirely(&context);

    assert_eq!(app.onboarding_state, onboarding::OnboardingState::Skipped);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
    match &app.state {
        LoadState::Loading { generation, .. } => {
            assert_ne!(*generation, generation_before);
            assert_eq!(*generation, app.refresh_generation);
        }
        _ => panic!("skipping onboarding entirely must also retry the stale load"),
    }
}

#[test]
fn advancing_through_a_non_final_onboarding_step_does_not_reload_the_archive_snapshot() {
    // The fix must be scoped to the two terminal transitions only - every
    // intermediate "Continue"/"Skip for now" must behave exactly as before,
    // with no duplicate/extra worker spawned while the user is still
    // browsing the wizard.
    let mut app = app_for_operation_tests();
    app.state = LoadState::Ready(Box::new(empty_loaded_data("/mount")));
    app.onboarding_state =
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::Welcome);
    let generation_before = app.refresh_generation;

    let context = egui::Context::default();
    app.onboarding_advance_from(&context, onboarding::OnboardingStep::Welcome);

    assert_eq!(
        app.onboarding_state,
        onboarding::OnboardingState::InProgress(onboarding::OnboardingStep::AddSource)
    );
    assert_eq!(
        app.refresh_generation, generation_before,
        "an intermediate step change must never trigger a reload"
    );
    assert!(
        matches!(&app.state, LoadState::Ready(_)),
        "an intermediate step change must never disturb the current snapshot"
    );
}
