//! The Check Games "saved results" card: what it shows in each state, what
//! its actions do, and what it never claims. Worker jobs are injected, so no
//! library, catalogue or game file is touched.
use super::*;
use archivefs_core::dat::reaudit::{
    ReauditBlocked, ReauditContradiction, ReauditGroup, ReauditItem, ReauditNote,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn item(id: i64, status: SavedDatEvidenceStatus) -> ReauditItem {
    ReauditItem {
        archive_id: id,
        path: PathBuf::from(format!("/games/gba/game-{id}.gba")),
        size_bytes: Some(8 * 1024 * 1024),
        saved_verdict: "verified_single_match".into(),
        status,
    }
}

fn plan(games: usize, blocked: usize, current: usize) -> ReauditPlan {
    ReauditPlan {
        groups: if games == 0 {
            vec![]
        } else {
            vec![ReauditGroup {
                source_id: "158".into(),
                catalogue_name: "No-Intro: Nintendo - Game Boy Advance".into(),
                platform: "Game Boy Advance".into(),
                items: (0..games)
                    .map(|n| {
                        item(
                            n as i64 + 1,
                            if n == 0 {
                                SavedDatEvidenceStatus::FileChanged
                            } else {
                                SavedDatEvidenceStatus::MissingProof
                            },
                        )
                    })
                    .collect(),
            }]
        },
        blocked: (0..blocked)
            .map(|n| ReauditBlocked {
                archive_id: 900 + n as i64,
                source_id: "165".into(),
                path: PathBuf::from(format!("/games/gbc/game-{n}.gbc")),
                saved_verdict: "verified_single_match".into(),
                status: SavedDatEvidenceStatus::CatalogueUnavailable,
                block: ReauditBlock::CatalogueUnavailable,
            })
            .collect(),
        current,
        saved_rows: games + blocked + current,
    }
}

/// `plan` plus `n` historical results that do not apply to their game.
fn plan_with_not_applicable(games: usize, blocked: usize, current: usize, n: usize) -> ReauditPlan {
    let mut plan = plan(games, blocked, current);
    plan.blocked.extend((0..n).map(|i| ReauditBlocked {
        archive_id: 5000 + i as i64,
        source_id: "158".into(),
        path: PathBuf::from(format!("/games/pc88/disk-{i}.d88")),
        saved_verdict: "no_match".into(),
        status: SavedDatEvidenceStatus::DoesNotApply,
        block: ReauditBlock::DoesNotApply,
    }));
    plan.saved_rows += n;
    plan
}

fn settle(state: &mut State) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.busy() && Instant::now() < deadline {
        state.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
    state.poll();
    assert!(!state.busy(), "the worker never finished");
}

fn rendered(state: &mut State) -> String {
    let context = egui::Context::default();
    crate::gui_v2::readable_style(&context);
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| state.show(ui));
    });
    fn gather(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(text) => {
                out.push_str(text.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| gather(s, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| gather(&clipped.shape, &mut out));
    out
}

fn listed(games: usize, blocked: usize, current: usize) -> State {
    let mut state = State::default();
    state.list_with(move || Ok(plan(games, blocked, current)));
    settle(&mut state);
    state
}

#[test]
fn a_library_with_only_current_results_shows_no_card() {
    let mut state = listed(0, 0, 120);
    assert!(matches!(state.stage, Stage::Listed(_)));
    assert!(!rendered(&mut state).contains(TITLE));
}

#[test]
fn the_list_says_how_many_why_and_that_they_count_as_not_verified() {
    let mut state = listed(3, 2, 10);
    let text = rendered(&mut state);
    assert!(text.contains(TITLE), "{text}");
    assert!(
        text.contains("• 3 can be checked again (they lost their proof; the game has to be read)"),
        "{text}"
    );
    assert!(
        text.contains("Until checked again, EmuWiz treats those games as not verified."),
        "{text}"
    );
    assert!(
        text.contains("• 2: saved before EmuWiz recorded which file and catalogue were checked"),
        "{text}"
    );
    assert!(
        text.contains("• 1: the file changed after it was checked"),
        "{text}"
    );
    assert!(
        text.contains("3 Game Boy Advance against No-Intro: Nintendo - Game Boy Advance"),
        "{text}"
    );
    assert!(text.contains("• 2 cannot be checked now"), "{text}");
    assert!(
        text.contains("switched off, removed or a folder of several files"),
        "{text}"
    );
    assert!(
        text.contains("• 10 already backed by a current check (not read again)"),
        "{text}"
    );
    assert!(text.contains("Check 3 games again"), "{text}");
    // Listing alone starts nothing and saves nothing.
    assert!(!state.busy() && !state.take_saved());
}

#[test]
fn only_blocked_results_offer_no_check() {
    let mut state = listed(0, 4, 0);
    let text = rendered(&mut state);
    assert!(text.contains("• 4 cannot be checked now"), "{text}");
    assert!(
        !text.contains("again\n") || !text.contains("Check "),
        "{text}"
    );
}

#[test]
fn the_confirmation_states_what_is_read_what_is_saved_and_what_it_does_not_promise() {
    let mut state = listed(3, 0, 0);
    let Stage::Listed(plan) = &state.stage else {
        panic!()
    };
    state.stage = Stage::Confirm(plan.clone());
    let text = rendered(&mut state);
    assert!(
        text.contains("This reads 3 games (24 MiB by recorded size)"),
        "{text}"
    );
    assert!(
        text.contains("Game files are only read, never changed."),
        "{text}"
    );
    assert!(
        text.contains("every finished set stays saved, the set being read is given up"),
        "{text}"
    );
    assert!(
        text.contains("the earlier result is kept for the record"),
        "{text}"
    );
    assert!(text.contains(NOT_A_PROMISE), "{text}");
    assert!(
        text.contains("Start checking") && text.contains("Not now"),
        "{text}"
    );
    assert!(
        !state.busy(),
        "showing the confirmation must not start the check"
    );
}

#[test]
fn progress_counts_each_saved_set_and_stop_reaches_the_worker() {
    let mut state = listed(4, 0, 0);
    let Stage::Listed(plan) = &state.stage else {
        panic!()
    };
    let (go, wait) = std::sync::mpsc::channel::<()>();
    state.run_with(plan.clone(), move |plan, cancel, progress| {
        progress(ReauditProgress::SetStarted {
            source_id: "158".into(),
            platform: "Game Boy Advance".into(),
            games: 2,
            done: 0,
            planned: plan.games(),
        });
        progress(ReauditProgress::SetSaved {
            verified: 1,
            no_match: 1,
            ambiguous: 0,
            not_checked: 0,
        });
        wait.recv().unwrap();
        assert!(
            cancel.load(Ordering::Relaxed),
            "Stop did not reach the worker"
        );
        Ok(ReauditReport {
            planned: 4,
            verified: 1,
            no_match: 1,
            saved: 2,
            sets_saved: 1,
            not_reached: 2,
            stopped: Some(ReauditStop::Cancelled),
            ..Default::default()
        })
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        state.poll();
        if matches!(&state.stage, Stage::Running(r) if r.done == 2) || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let text = rendered(&mut state);
    assert!(text.contains("Working… 2 of 4 checked"), "{text}");
    assert!(
        text.contains("1 verified · 1 no match · 0 need your choice · 0 not checked"),
        "{text}"
    );
    assert!(text.contains("Stop"), "{text}");
    state.stop();
    let text = rendered(&mut state);
    assert!(
        text.contains("Stopping. The set being read is not saved; every finished set is."),
        "{text}"
    );
    go.send(()).unwrap();
    settle(&mut state);
    let text = rendered(&mut state);
    assert!(text.contains("Partly completed"), "{text}");
    assert!(
        text.contains("You stopped the check. 2 games not reached. Every finished set is saved."),
        "{text}"
    );
    assert!(text.contains("Continue"), "{text}");
    assert!(text.contains(NOT_A_PROMISE), "{text}");
    // Saved sets mean the library must be read again, exactly once.
    assert!(state.take_saved() && !state.take_saved());
}

#[test]
fn a_finished_check_tells_matches_no_matches_choices_and_failures_apart() {
    let mut state = listed(6, 0, 0);
    let Stage::Listed(plan) = &state.stage else {
        panic!()
    };
    state.run_with(plan.clone(), |_, _, _| {
        Ok(ReauditReport {
            planned: 6,
            verified: 2,
            no_match: 1,
            ambiguous: 1,
            saved: 4,
            sets_saved: 1,
            not_checked: vec![
                ReauditNote {
                    archive_id: 5,
                    source_id: "158".into(),
                    path: PathBuf::from("/games/gba/game-5.gba"),
                    reason: "The file changed while it was being checked".into(),
                },
                ReauditNote {
                    archive_id: 6,
                    source_id: "158".into(),
                    path: PathBuf::from("/games/gba/game-6.gba"),
                    reason: "Unavailable file".into(),
                },
            ],
            contradictions: vec![ReauditContradiction {
                archive_id: 3,
                source_id: "158".into(),
                path: PathBuf::from("/games/gba/game-3.gba"),
                saved_verdict: "verified_single_match".into(),
                now: VerificationFindingKind::NoMatch,
            }],
            ..Default::default()
        })
    });
    settle(&mut state);
    let text = rendered(&mut state);
    assert!(text.contains("Checked again"), "{text}");
    assert!(
        text.contains("Of 6 that could be checked: 4 completed (2 verified · 1 no match · 1 need your choice) · 2 not checked · 0 not reached"),
        "{text}"
    );
    assert!(
        text.contains("1 saved match is no longer the file that is there now."),
        "{text}"
    );
    assert!(
        text.contains("• game-3.gba: now no match in the catalogue"),
        "{text}"
    );
    assert!(
        text.contains("2 games not checked. They stay as they were and will be listed again:"),
        "{text}"
    );
    assert!(
        text.contains("• 1: The file changed while it was being checked (for example game-5.gba)"),
        "{text}"
    );
    assert!(
        text.contains("• 1: Unavailable file (for example game-6.gba)"),
        "{text}"
    );
    assert!(text.contains(NOT_A_PROMISE), "{text}");
    assert!(!text.contains("Failed"), "{text}");
    assert!(
        !text.to_lowercase().contains("will run") || text.contains("does not promise"),
        "{text}"
    );
}

#[test]
fn problems_are_reported_as_could_not_complete_with_what_was_and_was_not_done() {
    let mut state = State::default();
    state.list_with(|| Err("the library database is busy.".into()));
    settle(&mut state);
    let text = rendered(&mut state);
    assert!(text.contains("Could not complete. The saved results could not be listed: the library database is busy. Nothing was checked."), "{text}");
    assert!(text.contains("Look again"), "{text}");

    // A check that could not start says so and saves nothing.
    let mut state = listed(2, 0, 0);
    let Stage::Listed(plan) = &state.stage else {
        panic!()
    };
    state.run_with(plan.clone(), |_, _, _| {
        Err("Another re-check of this library is already running. Nothing was checked.".into())
    });
    settle(&mut state);
    let text = rendered(&mut state);
    assert!(text.contains("Could not complete. Another re-check of this library is already running. Nothing was checked."), "{text}");
    assert!(!state.take_saved());

    // A worker that dies is not reported as a finished check.
    let mut state = listed(2, 0, 0);
    let Stage::Listed(plan) = &state.stage else {
        panic!()
    };
    state.run_with(plan.clone(), |_, _, _| panic!("worker died"));
    settle(&mut state);
    let text = rendered(&mut state);
    assert!(text.contains("The check stopped unexpectedly."), "{text}");
    assert!(!text.contains("Checked again"), "{text}");
}

#[test]
fn nothing_can_be_started_twice() {
    let mut state = listed(2, 0, 0);
    let Stage::Listed(plan) = &state.stage else {
        panic!()
    };
    let plan = plan.clone();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    state.run_with(plan.clone(), move |_, _, _| {
        wait.recv().unwrap();
        Ok(ReauditReport::default())
    });
    let started = std::sync::Arc::new(AtomicBool::new(false));
    let flag = started.clone();
    state.run_with(plan, move |_, _, _| {
        flag.store(true, Ordering::Relaxed);
        Ok(ReauditReport::default())
    });
    state.list_with(|| panic!("listing must not start while a check runs"));
    go.send(()).unwrap();
    settle(&mut state);
    assert!(!started.load(Ordering::Relaxed));
}

#[test]
fn old_results_that_do_not_apply_are_counted_apart_and_never_offered_for_checking() {
    // The owner's real shape: results worth checking, plus thousands of old
    // "no match" results from another system's catalogue.
    let mut state = State::default();
    state.list_with(|| Ok(plan_with_not_applicable(3, 1, 10, 5101)));
    settle(&mut state);
    let text = rendered(&mut state);
    assert!(
        text.contains("5,115 saved catalogue results in this library:"),
        "{text}"
    );
    assert!(text.contains("• 3 can be checked again"), "{text}");
    assert!(
        text.contains("• 10 already backed by a current check"),
        "{text}"
    );
    assert!(text.contains("• 1 cannot be checked now"), "{text}");
    assert!(
        text.contains("• 5,101 do not apply to their game (old results from another system's catalogue; kept as they are, nothing to check)"),
        "{text}"
    );
    assert!(text.contains("Check 3 games again"), "{text}");
    let Stage::Listed(plan) = &state.stage else {
        panic!()
    };
    assert!(plan.tally().is_consistent());

    // After the check the same groups are still told apart.
    let plan = plan.clone();
    state.run_with(plan, |_, _, _| {
        Ok(ReauditReport {
            planned: 3,
            verified: 2,
            saved: 2,
            sets_saved: 1,
            not_checked: vec![ReauditNote {
                archive_id: 3,
                source_id: "158".into(),
                path: PathBuf::from("/games/gba/game-3.gba"),
                reason: "Unavailable file".into(),
            }],
            ..Default::default()
        })
    });
    settle(&mut state);
    let text = rendered(&mut state);
    assert!(
        text.contains("Of 3 that could be checked: 2 completed (2 verified · 0 no match · 0 need your choice) · 1 not checked · 0 not reached"),
        "{text}"
    );
    assert!(
        text.contains("Not part of this check: 10 already current · 1 cannot be checked now · 5,101 do not apply to their game (kept as they are)"),
        "{text}"
    );
}

#[test]
fn when_only_inapplicable_old_results_remain_there_is_one_line_and_no_task() {
    let mut state = State::default();
    state.list_with(|| Ok(plan_with_not_applicable(0, 0, 6502, 5101)));
    settle(&mut state);
    let text = rendered(&mut state);
    assert!(!text.contains(TITLE), "{text}");
    assert!(
        text.contains("Every saved catalogue result that can be checked is backed by a current check. 5,101 older results do not apply to their game"),
        "{text}"
    );
    assert!(!text.contains("Check "), "{text}");
}
