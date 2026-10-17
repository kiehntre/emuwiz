//! One Save Vault job per GUI context. Plans/results stay owned by the worker;
//! the UI never substitutes its current selection into an accepted operation.
use super::*;
use std::sync::{Arc, Mutex, mpsc};

pub(super) enum Outcome {
    Restore(PsuRestoreDialogState),
    PsuExport(PsuExportDialogState),
    FileExport(ExportDialogState),
    History(restore_recovery::History),
}

#[derive(Clone)]
struct Pending {
    label: &'static str,
    receiver: Arc<Mutex<mpsc::Receiver<Outcome>>>,
}

fn id() -> egui::Id {
    egui::Id::new("save_vault_background_job")
}

pub(super) fn busy(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Pending>(id()).is_some())
}

pub(super) fn start(
    ctx: &egui::Context,
    label: &'static str,
    job: impl FnOnce() -> Outcome + Send + 'static,
) -> bool {
    if busy(ctx) {
        return false;
    }
    let (sender, receiver) = mpsc::channel();
    ctx.data_mut(|d| {
        d.insert_temp(
            id(),
            Pending {
                label,
                receiver: Arc::new(Mutex::new(receiver)),
            },
        )
    });
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        // A panic disconnects the channel. Poll reports uncertainty, never success.
        let _ = sender.send(job());
        ctx.request_repaint();
    });
    true
}

pub(super) fn poll(ctx: &egui::Context) {
    let Some(pending) = ctx.data(|d| d.get_temp::<Pending>(id())) else {
        return;
    };
    let result = pending.receiver.lock().unwrap().try_recv();
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
            return;
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            Outcome::Restore(PsuRestoreDialogState::Attention(format!(
                "The Save Vault worker stopped during {}. The outcome is unknown. Preserve the card, outputs, backups and restore records; inspect them before retrying.",
                pending.label
            )))
        }
    };
    ctx.data_mut(|d| d.remove::<Pending>(id()));
    match outcome {
        Outcome::Restore(state) => {
            if !matches!(
                state,
                PsuRestoreDialogState::UndoReviewed { .. }
                    | PsuRestoreDialogState::Confirm(_)
                    | PsuRestoreDialogState::Conflict { .. }
            ) {
                restore_recovery::invalidate_context(ctx);
            }
            ctx.data_mut(|d| d.insert_temp(psu_restore_dialog_id(), state));
        }
        Outcome::PsuExport(state) => {
            ctx.data_mut(|d| d.insert_temp(psu_export_dialog_id(), state));
        }
        Outcome::FileExport(state) => {
            ctx.data_mut(|d| d.insert_temp(export_dialog_id(), state));
        }
        Outcome::History(history) => restore_recovery::cache(ctx, history),
    }
}

pub(super) fn status(ui: &mut egui::Ui) {
    poll(ui.ctx());
    if let Some(pending) = ui.data(|d| d.get_temp::<Pending>(id())) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(pending.label);
        });
        ui.label("Save Vault is working in the background. Do not use the card in an emulator. An accepted write cannot be cancelled by closing this page; review its result or recovery record.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn finish(ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while busy(ctx) {
            poll(ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    #[test]
    fn a_blocked_job_leaves_gui_responsive_and_refuses_duplicate_submission() {
        let ctx = egui::Context::default();
        let ui_thread = std::thread::current().id();
        let (release, wait) = mpsc::channel();
        assert!(start(&ctx, "synthetic preview", move || {
            assert_ne!(std::thread::current().id(), ui_thread);
            wait.recv().unwrap();
            Outcome::Restore(PsuRestoreDialogState::Refused(
                "synthetic source changed".into(),
            ))
        }));
        assert!(!start(&ctx, "duplicate", || panic!(
            "duplicate must not run"
        )));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, status);
        });
        assert!(busy(&ctx));
        release.send(()).unwrap();
        finish(&ctx);
        assert!(
            matches!(ctx.data(|d| d.get_temp::<PsuRestoreDialogState>(psu_restore_dialog_id())), Some(PsuRestoreDialogState::Refused(message)) if message.contains("changed"))
        );
    }
    #[test]
    fn a_panicked_worker_is_unknown_never_a_success() {
        let ctx = egui::Context::default();
        assert!(start(&ctx, "synthetic restore", || panic!(
            "injected worker interruption"
        )));
        finish(&ctx);
        assert!(
            matches!(ctx.data(|d| d.get_temp::<PsuRestoreDialogState>(psu_restore_dialog_id())), Some(PsuRestoreDialogState::Attention(message)) if message.contains("unknown") && message.contains("Preserve"))
        );
    }
    #[test]
    fn navigating_away_keeps_the_accepted_operation_and_its_result() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("synthetic-output");
        let owned = output.clone();
        assert!(start(&ctx, "synthetic export", move || {
            std::fs::write(&owned, b"synthetic output").unwrap();
            Outcome::Restore(PsuRestoreDialogState::Attention(
                "inspect synthetic output".into(),
            ))
        }));
        // The page is not rendered while the accepted worker runs.
        finish(&ctx);
        assert_eq!(std::fs::read(output).unwrap(), b"synthetic output");
        assert!(
            ctx.data(|d| d.get_temp::<PsuRestoreDialogState>(psu_restore_dialog_id()))
                .is_some()
        );
    }
}

#[cfg(test)]
mod save_fixture_tests;
