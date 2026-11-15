//! Night Watch S11-04: Undo and recovery commands do not wait behind every
//! ordinary command queued before them, and a Playing Library preview can be
//! stopped without leaving a partial plan.
use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::{Condvar, Mutex};

type Gate = Arc<(Mutex<bool>, Condvar)>;

fn gate(open: bool) -> Gate {
    Arc::new((Mutex::new(open), Condvar::new()))
}

fn release(gate: &Gate) {
    let (open, signal) = &**gate;
    *open.lock().unwrap() = true;
    signal.notify_all();
}

fn finished_order(backend: &super::backend::Backend, expected: usize) -> Vec<u64> {
    let mut order = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while order.len() < expected {
        assert!(
            std::time::Instant::now() < deadline,
            "worker stalled: {order:?}"
        );
        if let super::backend::Event::Finished { id, .. } = backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap()
        {
            order.push(id);
        }
    }
    order
}

fn rollback_command() -> super::backend::Command {
    // A rollback with nothing applied refuses without touching anything.
    super::backend::Command::CanonicalOrganisation {
        state: Box::new(crate::rom_organisation_page::RomOrganisationPageState::default()),
        generation: 1,
        kind: super::CanonicalOrganisationJobKind::Rollback,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn a_queued_undo_runs_before_ordinary_commands_queued_earlier() {
    let context = egui::Context::default();
    let backend = super::backend::Backend::start(context);
    let blocker = gate(false);
    backend
        .send(1, super::backend::Command::Block(Arc::clone(&blocker)))
        .unwrap();
    // Wait until the blocker is actually running, so the next two are queued.
    loop {
        if let super::backend::Event::Started(1) = backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
        {
            break;
        }
    }
    backend
        .send(2, super::backend::Command::Block(gate(true)))
        .unwrap();
    backend.send(3, rollback_command()).unwrap();
    release(&blocker);

    assert_eq!(finished_order(&backend, 3), vec![1, 3, 2]);
}

#[test]
fn ordinary_commands_keep_their_order() {
    let context = egui::Context::default();
    let backend = super::backend::Backend::start(context);
    for id in 1..=3 {
        backend
            .send(id, super::backend::Command::Block(gate(true)))
            .unwrap();
    }
    assert_eq!(finished_order(&backend, 3), vec![1, 2, 3]);
}

#[test]
fn recovery_commands_keep_their_own_order() {
    let context = egui::Context::default();
    let backend = super::backend::Backend::start(context);
    let blocker = gate(false);
    backend
        .send(1, super::backend::Command::Block(Arc::clone(&blocker)))
        .unwrap();
    loop {
        if let super::backend::Event::Started(1) = backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
        {
            break;
        }
    }
    backend.send(2, rollback_command()).unwrap();
    backend.send(3, rollback_command()).unwrap();
    release(&blocker);
    assert_eq!(finished_order(&backend, 3), vec![1, 2, 3]);
}

#[test]
fn a_gone_worker_refuses_the_command_instead_of_queueing_it() {
    let backend = super::backend::Backend::disconnected();
    let error = backend.send(9, rollback_command()).unwrap_err();
    assert!(error.contains("worker is unavailable"), "{error}");
}

#[test]
fn recovery_classification_names_only_undo_and_recovery() {
    use super::backend::Command;
    assert!(rollback_command().is_recovery());
    assert!(
        Command::MissingUndo {
            receipt: "/nowhere".into()
        }
        .is_recovery()
    );
    assert!(!Command::Load { scan: false }.is_recovery());
    assert!(!Command::LoadRepairHistory.is_recovery());
    assert!(
        !Command::CanonicalOrganisation {
            state: Box::new(crate::rom_organisation_page::RomOrganisationPageState::default()),
            generation: 1,
            kind: super::CanonicalOrganisationJobKind::Preview,
            cancel: Arc::new(AtomicBool::new(false)),
        }
        .is_recovery()
    );
}

#[test]
fn a_cancelled_playing_library_preview_keeps_no_partial_plan() {
    let mut state = crate::playing_library_page::PlayingLibraryPageState::default();
    let cancel = AtomicBool::new(true);
    assert!(!state.preview_cancellable(&cancel));
    assert!(state.plan().is_none());
    assert!(
        state.error().is_none(),
        "a stop is not an error: {:?}",
        state.error()
    );
}

#[test]
fn an_uncancelled_playing_library_preview_still_reports_its_refusal() {
    let mut state = crate::playing_library_page::PlayingLibraryPageState::default();
    assert!(state.preview_cancellable(&AtomicBool::new(false)));
    assert!(state.error().is_some(), "no source folder was chosen");
}

#[test]
fn a_blue_playing_library_preview_is_cancellable_and_other_kinds_are_not() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.backend = Backend::disconnected();
    app.start_playing_library_job(super::PlayingLibraryJobKind::Preview);
    let preview = app
        .activity
        .jobs
        .values()
        .find(|job| job.title == "Planning your playing library")
        .expect("preview queued");
    assert!(
        preview.cancel.is_some(),
        "the preview carries a cancel flag"
    );
    app.playing_library_job = None;
    app.start_playing_library_job(super::PlayingLibraryJobKind::Apply);
    let apply = app
        .activity
        .jobs
        .values()
        .find(|job| job.title == "Building your playing library")
        .expect("apply queued");
    assert!(apply.cancel.is_none(), "apply must run to its safe end");
}
