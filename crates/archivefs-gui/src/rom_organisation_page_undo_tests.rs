//! Quick Rename after Apply: a used preview cannot be applied again, Undo asks
//! first and names what it reverses, and confirm cards and outcomes are
//! brought into view.

use super::*;
use archivefs_core::dat::rename_apply::model::{
    EntryState, ObjectIdentity, ObjectKind, TransactionEntry, TransactionState,
};
use archivefs_core::dat::rename_apply::{ApplyOutcome, RenameTransaction, TransactionSummary};

fn entry(name: &str, renamed: &str, state: EntryState) -> TransactionEntry {
    TransactionEntry {
        source_path: PathBuf::from(format!("/roms/nes/{name}")),
        destination_path: PathBuf::from(format!("/roms/nes/{renamed}")),
        original_basename: name.into(),
        proposed_basename: renamed.into(),
        identity: ObjectIdentity {
            size_bytes: 1,
            modified_unix: 1,
            kind: ObjectKind::RegularFile,
            ino: 1,
            dev: 1,
            freshness: None,
        },
        operation: Default::default(),
        preflight_passed: true,
        preflight_failures: Vec::new(),
        state,
        failure_reason: None,
        applied_at_unix: Some(1),
        rolled_back_at_unix: None,
        unknown: Default::default(),
    }
}

fn applied_transaction() -> RenameTransaction {
    RenameTransaction {
        transaction_id: "1795351772-1".into(),
        plan_generation: 7,
        classifier_version: None,
        created_at_unix: 0,
        source_scan_root: "/roms".into(),
        state: TransactionState::Applied,
        entries: vec![
            entry("ugly_one.nes", "Alpha Quest (USA).nes", EntryState::Applied),
            entry("plain.nes", "Plain Sailing (USA).nes", EntryState::Applied),
            entry("left.nes", "Left Alone (USA).nes", EntryState::Skipped),
        ],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    }
}

fn outcome(summary: TransactionSummary) -> ApplyOutcome {
    ApplyOutcome {
        transaction: applied_transaction(),
        summary,
    }
}

fn input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 900.0),
        )),
        ..Default::default()
    }
}

fn frame(
    ctx: &egui::Context,
    state: &mut RomOrganisationPageState,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<RomOrganisationPageAction>) {
    let mut action = None;
    let output = ctx.run(egui::RawInput { events, ..input() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = show_quick_rename_page_with_busy(ui, state, false);
        });
    });
    (output, action)
}

fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
    fn find(shape: &egui::Shape, needle: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle => {
                Some(text.galley.rect.translate(text.pos.to_vec2()))
            }
            egui::Shape::Vec(nested) => nested.iter().find_map(|s| find(s, needle)),
            _ => None,
        }
    }
    output.shapes.iter().find_map(|c| find(&c.shape, needle))
}

fn shows(output: &egui::FullOutput, needle: &str) -> bool {
    fn find(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| find(s, needle)),
            _ => false,
        }
    }
    output.shapes.iter().any(|c| find(&c.shape, needle))
}

/// Clicks the button labelled `label` (move, press, release over frames) and
/// returns every page action those frames produced, plus the last output.
fn click(
    ctx: &egui::Context,
    state: &mut RomOrganisationPageState,
    label: &str,
) -> (Vec<RomOrganisationPageAction>, egui::FullOutput) {
    let (output, _) = frame(ctx, state, Vec::new());
    let at = text_rect(&output, label)
        .unwrap_or_else(|| panic!("no '{label}' button on the page"))
        .center();
    let mut actions = Vec::new();
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }],
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    ] {
        let (_, action) = frame(ctx, state, events);
        actions.extend(action);
    }
    let (output, action) = frame(ctx, state, Vec::new());
    actions.extend(action);
    (actions, output)
}

fn undo_ready() -> RomOrganisationPageState {
    let mut state = RomOrganisationPageState::default();
    state.applied = Some(applied_transaction());
    state.result_message = Some("Applied 2 organisation(s).".into());
    state.result_complete = true;
    state
}

fn is_rollback(action: &RomOrganisationPageAction) -> bool {
    matches!(action, RomOrganisationPageAction::Rollback)
}

#[test]
fn undo_asks_first_and_names_the_rename_operation_it_reverses() {
    let ctx = egui::Context::default();
    let mut state = undo_ready();
    let (actions, output) = click(&ctx, &mut state, "Undo");
    assert!(
        !actions.iter().any(is_rollback),
        "the Undo button alone must not undo"
    );
    assert!(state.pending_undo);
    assert!(shows(&output, "Undo rename operation 1795351772-1?"));
    assert!(shows(&output, "It puts 2 file(s) back"));
    assert!(shows(
        &output,
        "Alpha Quest (USA).nes \u{2192} ugly_one.nes (in /roms/nes)"
    ));
    assert!(shows(&output, "Plain Sailing (USA).nes \u{2192} plain.nes"));
    assert!(
        !shows(&output, "Left Alone"),
        "a skipped entry is not offered as something Undo puts back"
    );

    let (actions, _) = click(&ctx, &mut state, "Confirm undo");
    assert_eq!(actions.iter().filter(|a| is_rollback(a)).count(), 1);
    assert!(!state.pending_undo);
}

#[test]
fn cancelling_undo_changes_nothing_and_closes_the_card() {
    let ctx = egui::Context::default();
    let mut state = undo_ready();
    click(&ctx, &mut state, "Undo");
    let (actions, output) = click(&ctx, &mut state, "Cancel");
    assert!(actions.is_empty(), "{:?}", actions.len());
    assert!(!state.pending_undo);
    assert!(!shows(&output, "Undo rename operation"));
    assert!(state.applied.is_some(), "Undo stays available");
}

#[test]
fn a_new_outcome_closes_an_open_undo_confirmation() {
    let ctx = egui::Context::default();
    let mut state = undo_ready();
    click(&ctx, &mut state, "Undo");
    assert!(state.pending_undo);
    state.begin_outcome();
    assert!(!state.pending_undo);
}

#[test]
fn the_undo_list_is_bounded_and_counts_the_rest() {
    let mut transaction = applied_transaction();
    transaction.entries = (0..14)
        .map(|i| {
            entry(
                &format!("g{i}.nes"),
                &format!("G{i}.nes"),
                EntryState::Applied,
            )
        })
        .collect();
    let lines = undo_confirmation_lines(&transaction, 10);
    assert_eq!(lines.len(), 11);
    assert_eq!(lines[10], "and 4 more");
}

#[test]
fn a_complete_apply_retires_the_preview_so_it_cannot_be_applied_again() {
    let mut state = RomOrganisationPageState::default();
    state.hold_plan_for_test(Path::new("/roms"), "/roms/nes/ugly_one.nes");
    state.record_apply_result_for_test(
        Ok(outcome(TransactionSummary {
            requested: 2,
            applied: 2,
            ..Default::default()
        })),
        applied_transaction(),
    );
    assert!(state.result_complete());
    assert!(!state.has_plan_for_test(), "the used preview is cleared");
    assert!(state.approved.is_empty(), "its approvals are cleared");
    assert!(state.applied().is_some(), "Undo stays available");

    let ctx = egui::Context::default();
    let (output, _) = frame(&ctx, &mut state, Vec::new());
    assert!(shows(
        &output,
        "Preview again before renaming anything else"
    ));
    assert!(!shows(&output, "Apply approved organisation"));
    // Apply itself refuses without a fresh preview and keeps the Undo handle.
    state.apply();
    assert!(state.error.is_some());
    assert_eq!(
        state.applied().map(|t| t.transaction_id.as_str()),
        Some("1795351772-1")
    );
}

#[test]
fn an_incomplete_apply_keeps_the_reviewed_plan_for_review() {
    let mut state = RomOrganisationPageState::default();
    state.hold_plan_for_test(Path::new("/roms"), "/roms/nes/ugly_one.nes");
    state.record_apply_result_for_test(
        Ok(outcome(TransactionSummary {
            requested: 2,
            applied: 1,
            failed: 1,
            ..Default::default()
        })),
        applied_transaction(),
    );
    assert!(!state.result_complete());
    assert!(state.has_plan_for_test());
    assert!(!state.approved.is_empty());
}

/// Draws the page inside a short scroll area under a tall block, and returns
/// the vertical scroll offset after the frames settle.
fn scroll_offset_after(state: &mut RomOrganisationPageState) -> f32 {
    let ctx = egui::Context::default();
    let mut offset = 0.0;
    // Scrolling is animated: let time pass between frames.
    for i in 0..60 {
        let raw = egui::RawInput {
            time: Some(f64::from(i) * 0.05),
            predicted_dt: 0.05,
            ..input()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let out = egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .id_salt("reveal-test")
                    .show(ui, |ui| {
                        ui.add_space(2000.0);
                        show_quick_rename_page_with_busy(ui, state, false);
                    });
                offset = out.state.offset.y;
            });
        });
    }
    offset
}

#[test]
fn an_apply_outcome_is_scrolled_into_view_once() {
    let mut quiet = undo_ready();
    assert_eq!(scroll_offset_after(&mut quiet), 0.0, "no reveal, no jump");

    let mut state = undo_ready();
    state.reveal = true;
    assert!(scroll_offset_after(&mut state) > 1500.0);
    assert!(!state.reveal, "the reveal is used once");
}

#[test]
fn the_undo_confirmation_is_scrolled_into_view() {
    let mut state = undo_ready();
    state.pending_undo = true;
    state.reveal = true;
    assert!(scroll_offset_after(&mut state) > 1500.0);
}

#[test]
fn the_apply_confirmation_is_scrolled_into_view() {
    let mut state = RomOrganisationPageState::default();
    // Field setters only: no folder is created or read.
    state.configure_for_test(PathBuf::from("/roms"), PathBuf::from("/no-journal"));
    state.hold_plan_for_test(Path::new("/roms"), "/roms/nes/ugly_one.nes");
    state
        .plan
        .as_mut()
        .unwrap()
        .entries
        .push(OrganisationPlanEntry {
            source_identity: None,
            catalogue_binding: None,
            source_path: PathBuf::from("/roms/nes/ugly_one.nes"),
            destination_path: PathBuf::from("/roms/nes/Alpha Quest (USA).nes"),
            platform: Some("NES".to_string()),
            platform_display_name: "Nintendo Entertainment System".to_string(),
            platform_source: "Manual".to_string(),
            slug: None,
            layout_folder: None,
            mode: OrganisationMode::RenameInPlace,
            content_classification: None,
            original_metadata: Default::default(),
            status: OrganisationStatus::Suggested,
            reason: None,
        });
    state.pending_apply = Some(1);
    state.reveal = true;
    assert!(scroll_offset_after(&mut state) > 1500.0);
}
