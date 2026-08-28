//! Focused tests for the Exact Duplicate Review and Quarantine page.
//!
//! Every fixture is a real temporary folder; `scan()`, `confirm_apply()`,
//! and `rollback_last()` all run the real core engine and the real shared
//! quarantine transaction/journal/rollback machinery - nothing here is a
//! render-only mock.

use std::path::PathBuf;
use std::time::Duration;

use archivefs_core::dat::rename_apply::journal::write_journal;
use archivefs_core::dat::rename_apply::model::TransactionState;

use super::*;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

fn base_state(fixture: &Fixture) -> ExactDuplicateReviewPageState {
    let journal_dir = fixture.path("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    ExactDuplicateReviewPageState::with_journal_dir(journal_dir)
}

/// Runs a scan to completion by polling, bounded so a genuine bug (a scan
/// that never finishes) fails the test instead of hanging the suite.
fn run_scan_to_completion(state: &mut ExactDuplicateReviewPageState) {
    state.scan();
    for _ in 0..2000 {
        if state.poll_scan() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("scan did not complete in time");
}

// --- real-widget render/interaction helpers, same idiom as
// playing_library_page/tests.rs -------------------------------------------

fn screen() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 2400.0))
}

fn render(
    ctx: &egui::Context,
    state: &mut ExactDuplicateReviewPageState,
    input: egui::RawInput,
) -> egui::FullOutput {
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_exact_duplicate_review_page(ui, state);
        });
    })
}

fn base_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(screen()),
        ..Default::default()
    }
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|shape| shape_contains(shape, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

fn find_exact_text_center(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn find_in_shape(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text_shape) => (text_shape.galley.text() == needle)
                .then(|| text_shape.pos + text_shape.galley.size() / 2.0),
            egui::Shape::Vec(nested) => nested.iter().find_map(|s| find_in_shape(s, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|clipped| find_in_shape(&clipped.shape, needle))
}

fn click_event(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ]
}

fn click_text(
    ctx: &egui::Context,
    state: &mut ExactDuplicateReviewPageState,
    needle: &str,
) -> egui::FullOutput {
    let before = render(ctx, state, base_input());
    let pos = find_exact_text_center(&before, needle)
        .unwrap_or_else(|| panic!("expected to find rendered text {needle:?} to click"));
    render(
        ctx,
        state,
        egui::RawInput {
            screen_rect: Some(screen()),
            events: click_event(pos),
            ..Default::default()
        },
    )
}

// --- 1: scan renders exact groups and reclaimable bytes --------------------

#[test]
fn scan_renders_exact_groups_and_reclaimable_bytes() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    let trusted_dir = fixture.path("trusted");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&trusted_dir).unwrap();
    std::fs::write(source.join("a.bin"), b"identical bytes here").unwrap();
    std::fs::write(trusted_dir.join("b.bin"), b"identical bytes here").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();

    run_scan_to_completion(&mut state);

    let report = state.report().expect("report");
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].reclaimable_bytes, 20);

    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(&output, "Exact copies"));
    assert!(rendered_text_contains(
        &output,
        "2 identical copies, 20 B each - 20 B reclaimable"
    ));
}

// --- 2: different-byte files never display as duplicates -------------------

#[test]
fn different_byte_files_never_display_as_duplicates() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("a.bin"), b"aaaaaaaaaaaaaaaaaaaa").unwrap();
    std::fs::write(source.join("b.bin"), b"bbbbbbbbbbbbbbbbbbbb").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();

    run_scan_to_completion(&mut state);

    assert!(state.report().unwrap().groups.is_empty());
    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(!rendered_text_contains(&output, "Exact copies"));
}

// --- 3: trusted-root recommendation is shown --------------------------------

#[test]
fn trusted_root_recommendation_is_shown() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    let trusted_dir = fixture.path("trusted");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let trusted_copy = trusted_dir.join("game.bin");
    let other_copy = source.join("game.bin");
    std::fs::write(&trusted_copy, b"same bytes everywhere").unwrap();
    std::fs::write(&other_copy, b"same bytes everywhere").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();

    run_scan_to_completion(&mut state);

    let group = &state.report().unwrap().groups[0];
    assert_eq!(
        group.recommendation,
        CanonicalRecommendation::TrustedRoot(trusted_copy)
    );
    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(
        &output,
        "is the only copy inside a user-designated trusted root"
    ));
}

// --- 4: elected-library recommendation is shown -----------------------------

#[test]
fn elected_library_recommendation_is_shown() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    let elected_dir = fixture.path("elected");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&elected_dir).unwrap();
    let elected_copy = elected_dir.join("game.bin");
    let other_copy = source.join("game.bin");
    std::fs::write(&elected_copy, b"same bytes everywhere").unwrap();
    std::fs::write(&other_copy, b"same bytes everywhere").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.elected_library_draft = elected_dir.display().to_string();

    run_scan_to_completion(&mut state);

    let group = &state.report().unwrap().groups[0];
    assert_eq!(
        group.recommendation,
        CanonicalRecommendation::ElectedLibrary(elected_copy)
    );
    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(
        &output,
        "is the only copy already used by a published/elected library"
    ));
}

// --- 5: undecided group requires user choice --------------------------------

#[test]
fn undecided_group_requires_user_choice() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("aaa.bin"), b"tied, no evidence at all").unwrap();
    std::fs::write(source.join("zzz.bin"), b"tied, no evidence at all").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();

    run_scan_to_completion(&mut state);

    assert_eq!(
        state.report().unwrap().groups[0].recommendation,
        CanonicalRecommendation::RequiresUserChoice
    );
    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(
        &output,
        "Needs your choice: which copy should be kept?"
    ));
    // Every member is offered as a manual choice - never a pre-selected
    // automatic winner (no "Move N copies to quarantine" button exists
    // until a choice is made).
    assert!(rendered_text_contains(&output, "Keep this copy: "));
    assert!(!rendered_text_contains(&output, "to quarantine"));
}

// --- 6: selecting retained copy updates preview -----------------------------

#[test]
fn selecting_retained_copy_updates_preview() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    std::fs::create_dir_all(&source).unwrap();
    let a = source.join("aaa.bin");
    let z = source.join("zzz.bin");
    std::fs::write(&a, b"tied, no evidence at all").unwrap();
    std::fs::write(&z, b"tied, no evidence at all").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    run_scan_to_completion(&mut state);
    assert!(
        state.effective_group(0).unwrap().recommendation
            == CanonicalRecommendation::RequiresUserChoice
    );

    state.choose_retained(0, z.clone());

    let effective = state.effective_group(0).unwrap();
    assert_eq!(
        effective.recommendation,
        CanonicalRecommendation::UserChosen(z)
    );
    assert_eq!(effective.redundant_paths, vec![a]);
    assert_eq!(effective.readiness, GroupQuarantineReadiness::Safe);

    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(&output, "Move 1 copy to quarantine"));
}

// --- 7: protected CUE companion cannot be selected independently -----------

#[test]
fn protected_cue_companion_cannot_be_selected_independently() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    std::fs::create_dir_all(&source).unwrap();
    let shared = source.join("shared_track.bin");
    std::fs::write(&shared, b"shared-track-bytes").unwrap();
    let cue1 = source.join("one.cue");
    std::fs::write(&cue1, "FILE \"shared_track.bin\" BINARY\n").unwrap();
    let cue2 = source.join("two.cue");
    std::fs::write(&cue2, "REM two\nFILE \"shared_track.bin\" BINARY\n").unwrap();
    let trusted_dir = fixture.path("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let shared_dup = trusted_dir.join("shared_track_copy.bin");
    std::fs::write(&shared_dup, b"shared-track-bytes").unwrap();

    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();
    run_scan_to_completion(&mut state);

    let group_index = state
        .report()
        .unwrap()
        .groups
        .iter()
        .position(|g| g.members.iter().any(|m| m.path == shared))
        .expect("shared-track group present");
    assert!(matches!(
        state.report().unwrap().groups[group_index].readiness,
        GroupQuarantineReadiness::Blocked(_)
    ));

    // Attempting to choose it manually is a no-op - blocked groups never
    // accept a selection.
    let before = state.report().unwrap().groups[group_index]
        .recommendation
        .clone();
    state.choose_retained(group_index, shared_dup.clone());
    let effective = state.effective_group(group_index).unwrap();
    // The attempted manual choice changed nothing - the group's own
    // automatic recommendation (whatever it was) is untouched, and
    // readiness is still Blocked regardless of what recommendation says.
    assert_eq!(effective.recommendation, before);
    assert!(matches!(
        effective.readiness,
        GroupQuarantineReadiness::Blocked(_)
    ));

    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(!rendered_text_contains(&output, "Keep this copy: "));
    assert!(rendered_text_contains(&output, "Protected:"));
}

// --- 8: technical details are collapsed by default --------------------------

#[test]
fn technical_details_are_collapsed_by_default() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("a.bin"), b"identical bytes here").unwrap();
    std::fs::write(source.join("b.bin"), b"identical bytes here").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    run_scan_to_completion(&mut state);
    let sha256 = state.report().unwrap().groups[0].sha256.clone();

    let ctx = egui::Context::default();
    let collapsed = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(&collapsed, "Technical details"));
    assert!(!rendered_text_contains(
        &collapsed,
        &format!("SHA-256: {sha256}")
    ));

    let expanded = click_text(&ctx, &mut state, "Technical details");
    assert!(rendered_text_contains(
        &expanded,
        &format!("SHA-256: {sha256}")
    ));
}

// --- 9: apply requires explicit confirmation --------------------------------

#[test]
fn apply_requires_explicit_confirmation() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    let trusted_dir = fixture.path("trusted");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let a = trusted_dir.join("a.bin");
    let b = source.join("b.bin");
    std::fs::write(&a, b"identical bytes here").unwrap();
    std::fs::write(&b, b"identical bytes here").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();
    run_scan_to_completion(&mut state);

    state.open_apply_confirmation(0);
    assert!(state.apply_confirm().is_some());
    // Nothing moved yet - opening a confirmation is not applying.
    assert!(a.exists());
    assert!(b.exists());

    let ctx = egui::Context::default();
    // Two frames: egui's `Window` positions itself on the first pass and
    // paints settled content on the second - the same reason
    // `click_text` elsewhere in this suite always renders once before
    // interacting.
    let _ = render(&ctx, &mut state, base_input());
    let output = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(&output, "Keeping: "));

    state.cancel_apply_confirmation();
    assert!(state.apply_confirm().is_none());
    assert!(a.exists());
    assert!(b.exists());
}

// --- 10: changed-since-preview error is plain and blocks mutation ----------

#[test]
fn changed_since_preview_error_is_plain_and_blocks_mutation() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    std::fs::create_dir_all(&source).unwrap();
    let trusted_dir = fixture.path("trusted");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let retained = trusted_dir.join("game.bin");
    let redundant = source.join("game.bin");
    std::fs::write(&retained, b"kept content, byte for byte").unwrap();
    std::fs::write(&redundant, b"kept content, byte for byte").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();
    run_scan_to_completion(&mut state);

    state.open_apply_confirmation(0);
    // The redundant copy changes after the scan/preview, before apply.
    std::fs::write(&redundant, b"mutated after preview, different bytes now").unwrap();

    state.confirm_apply(&trusted_dir);

    let error = state.apply_error().expect("plain error").to_string();
    assert!(error.contains("changed since the scan"));
    assert!(redundant.exists(), "nothing was moved");
    assert_eq!(
        std::fs::read(&redundant).unwrap(),
        b"mutated after preview, different bytes now"
    );
    assert!(state.applied().is_none());
}

// --- 11/12: successful quarantine updates UI state; rollback restores ------

fn live_pair(fixture: &Fixture) -> (PathBuf, PathBuf, PathBuf) {
    let source = fixture.path("source");
    let trusted_dir = fixture.path("trusted");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&trusted_dir).unwrap();
    let retained = trusted_dir.join("game.bin");
    let redundant = source.join("game.bin");
    std::fs::write(&retained, b"kept content, byte for byte").unwrap();
    std::fs::write(&redundant, b"kept content, byte for byte").unwrap();
    (source, trusted_dir, redundant)
}

#[test]
fn successful_quarantine_updates_ui_state() {
    let fixture = Fixture::new();
    let (source, trusted_dir, redundant) = live_pair(&fixture);
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();
    run_scan_to_completion(&mut state);

    state.open_apply_confirmation(0);
    state.confirm_apply(&trusted_dir);

    assert!(state.apply_error().is_none(), "{:?}", state.apply_error());
    assert!(state.applied().is_some());
    assert!(!redundant.exists(), "redundant copy was moved");

    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(&output, "You can undo this"));
}

#[test]
fn rollback_restores_state() {
    let fixture = Fixture::new();
    let (source, trusted_dir, redundant) = live_pair(&fixture);
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();
    run_scan_to_completion(&mut state);
    state.open_apply_confirmation(0);
    state.confirm_apply(&trusted_dir);
    assert!(!redundant.exists());

    state.rollback_last(&trusted_dir);

    assert!(
        state.rollback_error().is_none(),
        "{:?}",
        state.rollback_error()
    );
    assert!(
        redundant.exists(),
        "rollback restored the exact original path"
    );
    assert_eq!(
        std::fs::read(&redundant).unwrap(),
        b"kept content, byte for byte"
    );
    assert!(state.applied().is_none());
}

// --- 13: recovery state appears after interrupted transaction --------------

#[test]
fn recovery_state_appears_after_interrupted_transaction() {
    let fixture = Fixture::new();
    let (source, trusted_dir, _redundant) = live_pair(&fixture);
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();
    run_scan_to_completion(&mut state);
    state.open_apply_confirmation(0);
    state.confirm_apply(&trusted_dir);
    let mut transaction = state.applied().expect("applied").clone();

    // Simulate an interruption: the journal is left mid-flight.
    transaction.state = TransactionState::Applying;
    write_journal(&state_journal_dir(&state), &transaction).expect("write journal");

    // A fresh scan (as if the app were restarted) picks the interrupted
    // journal back up.
    run_scan_to_completion(&mut state);

    let recovery = state.recovery().expect("recovery report");
    assert_eq!(recovery.recoverable.len(), 1);

    let ctx = egui::Context::default();
    let output = render(&ctx, &mut state, base_input());
    assert!(rendered_text_contains(
        &output,
        "were interrupted before this program closed"
    ));
}

fn state_journal_dir(state: &ExactDuplicateReviewPageState) -> PathBuf {
    state.journal_dir.clone()
}

// --- 14: cancellation leaves files untouched --------------------------------

#[test]
fn cancellation_leaves_files_untouched() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("a.bin"), b"identical bytes here").unwrap();
    std::fs::write(source.join("b.bin"), b"identical bytes here").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();

    state.scan();
    state.cancel_scan();
    for _ in 0..2000 {
        if state.poll_scan() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    assert!(matches!(
        state.scan_status(),
        Some(ScanStatus::Cancelled) | Some(ScanStatus::Completed)
    ));
    assert!(source.join("a.bin").exists());
    assert!(source.join("b.bin").exists());
    assert_eq!(
        std::fs::read(source.join("a.bin")).unwrap(),
        b"identical bytes here"
    );
    assert_eq!(
        std::fs::read(source.join("b.bin")).unwrap(),
        b"identical bytes here"
    );
}

// --- unrelated files remain untouched throughout ---------------------------

#[test]
fn unrelated_source_files_remain_untouched() {
    let fixture = Fixture::new();
    let (source, trusted_dir, _redundant) = live_pair(&fixture);
    let unrelated = source.join("unrelated.bin");
    std::fs::write(&unrelated, b"not part of any duplicate group").unwrap();
    let mut state = base_state(&fixture);
    state.source_root_draft = source.display().to_string();
    state.trusted_root_draft = trusted_dir.display().to_string();
    run_scan_to_completion(&mut state);

    state.open_apply_confirmation(0);
    state.confirm_apply(&trusted_dir);

    assert!(unrelated.exists());
    assert_eq!(
        std::fs::read(&unrelated).unwrap(),
        b"not part of any duplicate group"
    );
}

#[test]
fn scanning_with_no_source_folder_shows_a_plain_error() {
    let fixture = Fixture::new();
    let mut state = base_state(&fixture);

    state.scan();

    assert!(state.error().unwrap().contains("source folder"));
    assert!(state.report().is_none());
}
