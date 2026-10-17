//! Focused tests for the Publisher / Frontend Library planning view's
//! state logic - task section 27's "GUI tests". These exercise the same
//! `preview()`/filter logic the page itself calls, not simulated egui
//! clicks (this codebase's other GUI pages test the same way - see
//! `playing_library_page/tests.rs`).

use std::path::PathBuf;

use archivefs_core::playing_library::{
    CandidateEvidenceSummary, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPlan, PlayingLibraryPolicy,
};
use archivefs_core::publisher_profile::PublisherActionSafety;

use super::*;

fn elected(name: &str, file: &str) -> ElectedGame {
    ElectedGame {
        dat_entry_name: name.to_string(),
        family_root_name: name.to_string(),
        explanation: ElectionExplanation {
            steps: vec!["the only election-eligible release in its family".to_string()],
            rejected: Vec::new(),
            winner_evidence: CandidateEvidenceSummary::unknown(),
        },
        launcher_operation: LinkedLibraryOperation {
            source_path: PathBuf::from(file),
            destination_path: PathBuf::from("/source-library").join(file),
        },
        companion_operations: Vec::new(),
    }
}

fn plan_with(games: Vec<ElectedGame>) -> PlayingLibraryPlan {
    PlayingLibraryPlan {
        destination_root: PathBuf::from("/source-library"),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: games.len(),
        families_examined: games.len(),
        elected_games: games,
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: 0,
        conflicts: Vec::new(),
        operations: Vec::new(),
        rejected_launchers: Vec::new(),
    }
}

#[test]
fn preview_without_a_target_reports_a_plain_language_error() {
    let mut state = PublisherProfilePageState {
        destination_root: "/library".to_string(),
        source_plan: Some(plan_with(vec![elected("Game", "game.rom")])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    assert!(state.result.is_none());
    assert_eq!(
        state.error.as_deref(),
        Some("Choose a target profile first.")
    );
}

#[test]
fn preview_without_a_source_plan_reports_a_plain_language_error() {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: "/library".to_string(),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    assert!(state.result.is_none());
    assert!(state.error.is_some());
}

#[test]
fn preview_with_a_relative_destination_is_rejected() {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: "relative/path".to_string(),
        source_plan: Some(plan_with(vec![elected("Game", "game.rom")])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    assert!(state.result.is_none());
    assert!(state.error.as_deref().unwrap().contains("absolute"));
}

#[test]
fn preview_with_valid_input_produces_a_result_with_no_apply_path() {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: "/library".to_string(),
        source_plan: Some(plan_with(vec![elected("Sensible Soccer", "soccer.adf")])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    assert!(state.error.is_none());
    let result = state.result.unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].safety, PublisherActionSafety::SafeToAct);
}

#[test]
fn es_de_target_resolves_via_its_own_reviewed_table() {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::EsDe),
        destination_root: "/rompath".to_string(),
        source_plan: Some(plan_with(vec![elected("Bonk's Adventure", "bonk.pce")])),
        canonical_platform_id: "TurboGrafx-16".to_string(),
        ..Default::default()
    };
    state.preview();
    let result = state.result.unwrap();
    assert_eq!(
        result.items[0].planned_destination,
        Some(PathBuf::from("/rompath/tg16/bonk.pce"))
    );
}

#[test]
fn filters_partition_items_by_safety_and_destination_state() {
    let ready = elected("Ready Game", "ready.rom");
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: "/library".to_string(),
        source_plan: Some(plan_with(vec![ready])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    let result = state.result.as_ref().unwrap();
    let ready_count = result
        .items
        .iter()
        .filter(|item| PublisherResultFilter::Ready.matches(item))
        .count();
    assert_eq!(ready_count, 1);
    let blocked_count = result
        .items
        .iter()
        .filter(|item| PublisherResultFilter::Blocked.matches(item))
        .count();
    assert_eq!(blocked_count, 0);
}

fn executable_state(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> PublisherProfilePageState {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: destination.display().to_string(),
        source_plan: Some(plan_with(vec![elected(
            "Game",
            &source.display().to_string(),
        )])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    state
}

#[test]
fn execution_review_requires_explicit_mode_and_exact_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    std::fs::write(&source, b"gui source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);

    state.execution_review();
    assert_eq!(state.execution_stage, PublisherExecutionStage::Review);
    assert!(state.execution_error.is_some());

    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Hardlink);
    state.execution_review();
    assert_eq!(state.execution_stage, PublisherExecutionStage::Confirming);
    assert!(!state.can_apply());
    state.confirmation_text = "PUBLISH 0 ITEMS".to_string();
    assert!(!state.can_apply());
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    assert!(state.can_apply());
}

#[test]
fn gui_hardlink_path_applies_and_rolls_back_without_touching_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"hardlink gui source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Hardlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.apply_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::Applied);
    let transaction = state.execution_transaction.as_ref().unwrap();
    let published = transaction.transaction.entries[0].destination_path.clone();
    assert!(published.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"hardlink gui source");
    assert_eq!(
        state.execution_result.as_ref().unwrap().mode,
        archivefs_core::publisher_profile::PublisherLinkMode::Hardlink
    );

    state.rollback_confirmation_text = state.rollback_confirmation_phrase().unwrap();
    assert!(state.can_rollback());
    state.rollback_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::RolledBack);
    assert!(!published.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"hardlink gui source");
}

#[test]
fn gui_apply_refuses_a_replaced_destination_root_before_any_write() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"root replacement source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();

    std::fs::remove_dir(&destination).unwrap();
    std::fs::create_dir(&destination).unwrap();
    state.apply_with_journal_dir(&journal);

    assert_eq!(state.execution_stage, PublisherExecutionStage::Failed);
    assert!(
        state
            .execution_error
            .as_deref()
            .is_some_and(|error| error.contains("destination root changed"))
    );
    assert!(!destination.join("roms").exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"root replacement source");
}

#[test]
fn gui_symlink_path_applies_and_rolls_back_without_touching_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"symlink gui source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.apply_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::Applied);
    let published = state
        .execution_transaction
        .as_ref()
        .unwrap()
        .transaction
        .entries[0]
        .destination_path
        .clone();
    assert!(published.is_symlink());
    assert_eq!(std::fs::read_link(&published).unwrap(), source);
    assert_eq!(
        state.execution_result.as_ref().unwrap().mode,
        archivefs_core::publisher_profile::PublisherLinkMode::Symlink
    );

    state.rollback_confirmation_text = state.rollback_confirmation_phrase().unwrap();
    state.rollback_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::RolledBack);
    assert!(!published.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"symlink gui source");
}

#[test]
fn changed_destination_blocks_apply_and_requires_a_new_preview() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"stale gui source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.destination_root = temp.path().join("changed").display().to_string();
    state.apply_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::Failed);
    assert!(
        state
            .execution_error
            .as_deref()
            .unwrap()
            .contains("changed since preview")
    );
    assert!(!destination.join("roms").exists());
}

// ---- R3: `Ok(outcome)` from the executor is not "published" unless fully resolved ----

fn publish_summary(
    applied: usize,
    failed: usize,
    unresolved: usize,
    pending: usize,
) -> archivefs_core::dat::rename_apply::TransactionSummary {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionSummary, UnresolvedEntry};
    TransactionSummary {
        requested: applied + failed + unresolved + pending,
        applied,
        failed,
        unresolved,
        pending,
        unresolved_entries: (0..unresolved)
            .map(|i| UnresolvedEntry {
                source_path: std::path::PathBuf::from(format!("/src/a{i}.bin")),
                destination_path: std::path::PathBuf::from(format!("/out/A{i}.bin")),
                state: EntryState::Applying,
                stage: "rename in flight (completion not recorded)".into(),
                detail: None,
                required_action: "Run recovery reconciliation; do not replay.".into(),
            })
            .collect(),
        ..TransactionSummary::default()
    }
}

#[test]
fn publisher_reports_applied_only_for_a_fully_resolved_batch() {
    let done = publish_summary(3, 0, 0, 0);
    assert_eq!(execution_stage_for(&done), PublisherExecutionStage::Applied);
    assert!(incomplete_publish_message(&done).is_none());

    for incomplete in [
        publish_summary(0, 0, 1, 2),
        publish_summary(2, 1, 0, 0),
        publish_summary(1, 0, 0, 3),
    ] {
        assert_eq!(
            execution_stage_for(&incomplete),
            PublisherExecutionStage::NeedsReconciliation
        );
        let message = incomplete_publish_message(&incomplete).expect("explained");
        assert!(message.contains("NOT complete"), "{message}");
    }
    let message = incomplete_publish_message(&publish_summary(0, 0, 1, 0)).unwrap();
    assert!(message.contains("/out/A0.bin"), "{message}");
    assert!(message.contains("do not replay"), "{message}");
}

#[test]
fn export_profiles_preserve_user_frontend_files_through_apply_and_undo() {
    for target in [PublisherFrontend::RomM, PublisherFrontend::EsDe] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.adf");
        let destination = temp.path().join("frontend");
        let journal = temp.path().join("journal");
        std::fs::write(&source, b"synthetic game").unwrap();
        std::fs::create_dir(&destination).unwrap();
        let files = [
            (
                "gamelist.xml",
                b"<gameList><!-- user edit --><game><name>My name</name></game></gameList>"
                    .as_slice(),
            ),
            ("es_systems.xml", b"user configuration".as_slice()),
            ("cover.png", b"synthetic user artwork".as_slice()),
        ];
        for (name, content) in &files {
            std::fs::write(destination.join(name), content).unwrap();
        }
        let gamelist = destination.join("amiga/gamelist.xml");
        let artwork = destination.join("amiga/media/cover.png");
        std::fs::create_dir_all(artwork.parent().unwrap()).unwrap();
        std::fs::write(&gamelist, files[0].1).unwrap();
        std::fs::write(&artwork, files[2].1).unwrap();
        let mut state = executable_state(&source, &destination);
        state.target = Some(target);
        state.preview();
        state.link_mode = Some(PublisherLinkMode::Symlink);
        state.execution_review();
        state.confirmation_text = state.apply_confirmation_phrase().unwrap();
        state.apply_with_journal_dir(&journal);
        assert_eq!(
            state.execution_stage,
            PublisherExecutionStage::Applied,
            "{:?}",
            state.execution_error
        );
        for (name, content) in &files {
            assert_eq!(std::fs::read(destination.join(name)).unwrap(), *content);
        }
        state.rollback_confirmation_text = state.rollback_confirmation_phrase().unwrap();
        state.rollback_with_journal_dir(&journal);
        assert_eq!(state.execution_stage, PublisherExecutionStage::RolledBack);
        for (name, content) in &files {
            assert_eq!(std::fs::read(destination.join(name)).unwrap(), *content);
        }
        assert_eq!(std::fs::read(&gamelist).unwrap(), files[0].1);
        assert_eq!(std::fs::read(&artwork).unwrap(), files[2].1);
        assert_eq!(std::fs::read(&source).unwrap(), b"synthetic game");
    }
}

#[test]
fn export_profile_refuses_to_replace_user_edited_gamelist_in_game_slot() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("gamelist.xml");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic source").unwrap();
    let occupied = destination.join("amiga/gamelist.xml");
    std::fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    let user_edit = b"<gameList><!-- user names, play counts and artwork references --></gameList>";
    std::fs::write(&occupied, user_edit).unwrap();
    let mut state = executable_state(&source, &destination);
    state.target = Some(PublisherFrontend::EsDe);
    state.preview();
    let result = state.result.as_ref().unwrap();
    assert_eq!(result.summary().will_publish, 0);
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    assert!(!state.can_apply());
    state.confirmation_text = "PUBLISH 1 ITEMS".into();
    state.apply_with_journal_dir(&temp.path().join("journal"));
    assert_eq!(std::fs::read(occupied).unwrap(), user_edit);
    assert!(!temp.path().join("journal").exists());
}

#[test]
fn reviewing_again_consumes_previous_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    assert!(state.can_apply());
    state.execution_review();
    assert!(!state.can_apply());
}

#[test]
fn background_preview_is_read_only_and_retains_its_result() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.adf");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    let mut state = executable_state(&source, &destination);
    let context = egui::Context::default();
    state.start(PublisherJob::Preview, context.clone());
    assert!(state.is_busy());
    // A second request cannot replace the accepted job.
    state.start(PublisherJob::Apply, context.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while state.is_busy() {
        state.poll(&context);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(state.result.is_some());
    assert!(!destination.exists());
    assert_eq!(std::fs::read(source).unwrap(), b"synthetic");
}

fn rendered_labels(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
    fn gather(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.text().into(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    gather(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for shape in &output.shapes {
        gather(&shape.shape, &mut out);
    }
    out
}

#[test]
fn worker_failure_never_claims_no_destination_changes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.execution_stage = PublisherExecutionStage::NeedsReconciliation;
    state.execution_error = Some("Synthetic worker failure".into());
    let context = egui::Context::default();
    let output = context.run(Default::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_publisher_profile_page(ui, &mut state);
        });
    });
    let labels = rendered_labels(&output)
        .into_iter()
        .map(|x| x.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(labels.contains("Synthetic worker failure"));
    assert!(!labels.contains("No destination changes were made"));
}

#[test]
fn confirmation_is_keyboard_editable_and_apply_reachable_at_1024_by_600() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    let context = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 600.0));
    let draw = |events, state: &mut PublisherProfilePageState| {
        context.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        show_publisher_profile_page(ui, state);
                    });
                });
            },
        )
    };
    draw(vec![], &mut state);
    let mut found = false;
    // Standard tab navigation must eventually focus the confirmation editor.
    for _ in 0..35 {
        draw(
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            &mut state,
        );
        draw(
            vec![egui::Event::Text("PUBLISH 1 ITEMS".into())],
            &mut state,
        );
        if state.confirmation_text == "PUBLISH 1 ITEMS" {
            found = true;
            break;
        }
        // Typing into the destination/system fields must not invalidate the fixture for subsequent focus steps.
        state.destination_root = destination.display().to_string();
        state.canonical_platform_id = "Amiga".into();
    }
    assert!(found, "confirmation editor was unreachable by Tab");
    assert!(state.can_apply());
    context.style_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
    draw(
        vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        &mut state,
    );
    draw(vec![], &mut state);
    let output = draw(vec![], &mut state);
    assert!(
        rendered_labels(&output)
            .iter()
            .any(|(label, rect)| label == "Apply publisher plan" && screen.contains_rect(*rect)),
        "Apply was not scrolled into view by keyboard focus: focused={:?}, labels={:?}",
        context.memory(|memory| memory.focused()),
        rendered_labels(&output)
    );
    assert!(!destination.join("roms").exists());
}

#[test]
fn unsupported_frontend_platform_has_no_apply_path() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    let mut state = executable_state(&source, &destination);
    state.canonical_platform_id = "Synthetic unsupported platform".into();
    state.preview();
    assert_eq!(state.result.as_ref().unwrap().summary().will_publish, 0);
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    assert!(!state.can_apply());
    state.confirmation_text = "PUBLISH 1 ITEMS".into();
    state.apply_with_journal_dir(&temp.path().join("journal"));
    assert!(!destination.exists());
    assert!(!temp.path().join("journal").exists());
}
