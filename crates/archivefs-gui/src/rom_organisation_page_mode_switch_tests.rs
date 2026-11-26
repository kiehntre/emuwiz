//! Mode switches (Library Makeover follow-up): a scoped rename's file list,
//! preview, approvals and confirmation never carry into Move or a link mode,
//! and the chosen scope applies to Rename only.
use std::path::Path;

use super::*;

const UNSCOPED: [OrganisationMode; 3] = [
    OrganisationMode::MoveRealFile,
    OrganisationMode::BuildLinkedLibrary,
    OrganisationMode::OrganiseSymlinkOnly,
];

/// A reviewed scoped rename over the gbc folder of the configured source
/// folder, whose preview kept one of its three files: what a scoped preview leaves behind, with an approved
/// preview and an open confirmation. Private folders only.
fn scoped_rename(dir: &Path) -> RomOrganisationPageState {
    let roms = dir.join("roms");
    std::fs::create_dir_all(roms.join("gbc")).unwrap();
    for name in ["a.gbc", "b.gbc", "c.gbc"] {
        std::fs::write(roms.join("gbc").join(name), b"x").unwrap();
    }
    let mut state =
        RomOrganisationPageState::default().with_deferred_source_roots(vec![roms.clone()]);
    state.configure_for_test(roms.clone(), dir.join("journal"));
    state.set_scope(Some(scope::OrganisationScope::Folders(vec![
        roms.join("gbc"),
    ])));
    state.sources = vec![roms.join("gbc").join("a.gbc")];
    state.sources_scanned = true;
    state.hold_plan_for_test(&roms, &roms.join("gbc").join("a.gbc").to_string_lossy());
    state.pending_apply = Some(1);
    state.confirm_text = "RENAME 1 FILES".into();
    state.include_hidden_rows = true;
    state
}

fn assert_nothing_carried_over(state: &RomOrganisationPageState, mode: OrganisationMode) {
    assert!(state.plan.is_none(), "{mode:?}: preview kept");
    assert!(state.approved.is_empty(), "{mode:?}: approvals kept");
    assert!(state.pending_apply.is_none(), "{mode:?}: confirmation kept");
    assert!(state.confirm_text.is_empty(), "{mode:?}: typed phrase kept");
    assert!(
        !state.include_hidden_rows,
        "{mode:?}: hidden-rows choice kept"
    );
    assert!(state.sources.is_empty(), "{mode:?}: file list kept");
    assert!(
        !state.sources_scanned,
        "{mode:?}: file list still counted as scanned"
    );
    assert!(state.saved_checks.is_empty() && state.name_choices.is_empty());
}

#[test]
fn leaving_a_scoped_rename_discards_its_files_preview_and_approvals() {
    for mode in UNSCOPED {
        let dir = tempfile::tempdir().unwrap();
        let mut state = scoped_rename(dir.path());
        state.set_mode(mode);
        assert_nothing_carried_over(&state, mode);
        assert!(!state.uses_scope());
        assert!(
            state.scope().is_some(),
            "{mode:?}: the chosen scope is kept for Rename"
        );
    }
}

#[test]
fn after_a_scoped_rename_other_modes_walk_every_configured_source() {
    for mode in UNSCOPED {
        let dir = tempfile::tempdir().unwrap();
        let mut state = scoped_rename(dir.path());
        state.set_mode(mode);
        // The next preview walks the configured folders because nothing
        // counts as scanned; the walk finds all three files, not the scope's one.
        assert!(!state.sources_scanned());
        state.scan_sources();
        let mut found: Vec<_> = state
            .sources
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        found.sort();
        assert_eq!(found, ["a.gbc", "b.gbc", "c.gbc"], "{mode:?}");
    }
}

#[test]
fn with_the_sources_unavailable_no_file_from_the_old_scope_is_previewed() {
    for mode in UNSCOPED {
        let dir = tempfile::tempdir().unwrap();
        let mut state = scoped_rename(dir.path());
        std::fs::remove_dir_all(dir.path().join("roms")).unwrap();
        state.set_mode(mode);
        state.generate_plan();
        assert!(state.plan.is_none(), "{mode:?}: planned from the old scope");
        assert!(
            state
                .error()
                .unwrap_or_default()
                .contains("may not be mounted"),
            "{mode:?}: {:?}",
            state.error()
        );
    }
}

#[test]
fn returning_to_rename_keeps_the_chosen_scope_and_starts_afresh() {
    for mode in UNSCOPED {
        let dir = tempfile::tempdir().unwrap();
        let mut state = scoped_rename(dir.path());
        let chosen = state.scope().cloned();
        state.set_mode(mode);
        state.set_mode(OrganisationMode::RenameInPlace);
        assert!(state.uses_scope());
        assert_eq!(state.scope().cloned(), chosen, "{mode:?}");
        assert_nothing_carried_over(&state, mode);
    }
}

#[test]
fn switching_between_move_and_link_modes_discards_each_preview() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scoped_rename(dir.path());
    state.set_mode(OrganisationMode::MoveRealFile);
    for mode in [
        OrganisationMode::BuildLinkedLibrary,
        OrganisationMode::OrganiseSymlinkOnly,
        OrganisationMode::MoveRealFile,
    ] {
        let roms = dir.path().join("roms");
        state.sources = vec![roms.join("gbc").join("b.gbc")];
        state.sources_scanned = true;
        state.hold_plan_for_test(&roms, &roms.join("gbc").join("b.gbc").to_string_lossy());
        state.pending_apply = Some(1);
        state.set_mode(mode);
        assert_nothing_carried_over(&state, mode);
    }
}

#[test]
fn choosing_the_current_mode_again_keeps_the_file_list() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scoped_rename(dir.path());
    state.set_mode(OrganisationMode::RenameInPlace);
    assert_eq!(state.sources.len(), 1);
    assert!(state.sources_scanned);
    assert!(state.scope().is_some());
}
