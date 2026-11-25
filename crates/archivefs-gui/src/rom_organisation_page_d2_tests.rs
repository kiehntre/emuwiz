//! D2 (#539): an approved change the filter hides is never applied without
//! the person's explicit say-so, and the confirmation lists every change.
use std::path::Path;

use super::*;

fn entry(path: &str, status: OrganisationStatus) -> OrganisationPlanEntry {
    OrganisationPlanEntry {
        source_identity: None,
        catalogue_binding: None,
        disc_set: None,
        source_path: path.into(),
        destination_path: Path::new(path).with_file_name("New (World).gbc"),
        platform: Some("Game Boy Color".into()),
        platform_display_name: "Game Boy Color".into(),
        platform_source: "Verified DAT".into(),
        slug: None,
        layout_folder: None,
        mode: OrganisationMode::RenameInPlace,
        content_classification: None,
        original_metadata: Default::default(),
        status,
        reason: None,
        deferred_capture: None,
    }
}

/// Two approved renames and one already-correct row, with a private journal
/// folder (never the real data folder).
fn state_with_plan() -> (RomOrganisationPageState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = RomOrganisationPageState::default();
    state.configure_for_test(dir.path().join("roms"), dir.path().join("journal"));
    state.hold_plan_for_test(Path::new("/roms"), "/roms/gbc/a.gbc");
    state.approved.insert("/roms/gbc/b.gbc".into());
    let plan = state.plan.as_mut().unwrap();
    plan.entries = vec![
        entry("/roms/gbc/a.gbc", OrganisationStatus::Suggested),
        entry("/roms/gbc/b.gbc", OrganisationStatus::Suggested),
        entry("/roms/gbc/c.gbc", OrganisationStatus::AlreadyOrganised),
    ];
    (state, dir)
}

#[test]
fn a_filter_that_hides_approved_changes_is_counted() {
    let (mut state, _dir) = state_with_plan();
    assert_eq!(state.hidden_approved(), 0, "All shows everything");
    state.set_filter(Some(OrganisationStatus::Suggested));
    assert_eq!(state.hidden_approved(), 0);
    state.set_filter(Some(OrganisationStatus::AlreadyOrganised));
    assert_eq!(state.hidden_approved(), 2);
}

#[test]
fn the_confirmation_lists_every_approved_change() {
    let (state, _dir) = state_with_plan();
    let changes = approved_changes(state.plan.as_ref().unwrap(), &state.approved);
    let lines: Vec<&str> = changes.iter().map(|(_, line)| line.as_str()).collect();
    assert_eq!(
        lines,
        [
            "/roms/gbc/a.gbc  →  New (World).gbc",
            "/roms/gbc/b.gbc  →  New (World).gbc"
        ]
    );
}

#[test]
fn apply_refuses_hidden_changes_that_were_not_confirmed() {
    let (mut state, _dir) = state_with_plan();
    state.set_filter(Some(OrganisationStatus::AlreadyOrganised));
    state.apply();
    let error = state.error().unwrap_or_default().to_string();
    assert!(error.contains("hidden by the filter"), "{error}");
    assert!(state.applied().is_none(), "nothing was applied");
}

#[test]
fn approving_only_what_the_filter_shows_drops_the_hidden_changes() {
    let (mut state, _dir) = state_with_plan();
    state.set_filter(Some(OrganisationStatus::AlreadyOrganised));
    state.pending_apply = Some(2);
    state.drop_hidden_approvals();
    assert!(state.approved.is_empty());
    assert_eq!(state.pending_apply, Some(0));
    assert_eq!(state.hidden_approved(), 0);
}

#[test]
fn a_new_scope_discards_the_plan_and_its_approvals() {
    let (mut state, _dir) = state_with_plan();
    state.set_scope(Some(scope::OrganisationScope::Folders(vec![
        "/roms/gbc".into(),
    ])));
    assert!(state.plan.is_none() && state.approved.is_empty());
    assert!(!state.sources_scanned);
}

#[test]
fn a_scoped_rename_preview_needs_a_scope_and_reads_nothing_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = RomOrganisationPageState::default();
    state.configure_for_test(dir.path().join("roms"), dir.path().join("journal"));
    state.generate_plan();
    assert!(state.plan.is_none());
    assert!(
        state
            .error()
            .unwrap_or_default()
            .contains("Choose what to include")
    );
}
