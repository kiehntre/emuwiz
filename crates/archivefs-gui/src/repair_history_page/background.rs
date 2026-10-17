//! Native history I/O: move the existing controller to one worker, never
//! duplicate its recovery decisions. While away, retain results for polling.
use super::*;

impl RepairHistoryPageState {
    pub(crate) fn load_background(journal_dir: PathBuf) -> Self {
        let mut state = Self::empty(journal_dir);
        state.background_ui = true;
        state.refresh();
        state
    }

    pub(super) fn start_io(&mut self, action: impl FnOnce(&mut Self) + Send + 'static) {
        if self.io_job.is_some() || self.undo_running || self.io_error.is_some() {
            return;
        }
        let mut placeholder = Self::empty(self.journal_dir.clone());
        placeholder.background_ui = true;
        let mut working = std::mem::replace(self, placeholder);
        working.background_ui = false;
        let (sender, receiver) = std::sync::mpsc::channel();
        self.io_job = Some(receiver);
        std::thread::spawn(move || {
            action(&mut working);
            let _ = sender.send(working);
        });
    }

    pub(crate) fn poll_background(&mut self) -> bool {
        let Some(job) = &self.io_job else {
            return false;
        };
        match job.try_recv() {
            Ok(mut result) => {
                result.background_ui = true;
                *self = result;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.io_job = None;
                self.io_error = Some("History worker stopped without a result. Outcome is unknown. Preserve files and journals; inspect recovery before retrying. Restart the page only after reviewing the records.".into());
                true
            }
        }
    }

    pub(crate) fn background_busy(&self) -> bool {
        self.io_job.is_some()
    }

    pub(super) fn show_background_status(&mut self, ui: &mut egui::Ui) -> bool {
        self.poll_background();
        if let Some(error) = &self.io_error {
            widgets::banner(
                ui,
                "Recovery review required",
                error,
                widgets::StatusTone::Blocked,
            );
            return true;
        }
        if self.background_busy() {
            ui.spinner();
            ui.label(
                "Checking repair records in the background. No completed result is available yet.",
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(40));
            return true;
        }
        false
    }
}
