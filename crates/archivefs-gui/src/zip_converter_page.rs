//! Small Converter surface for verified ZIP creation and extraction.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use archivefs_core::zip_converter::reviewed_apply::{
    ZipError, ZipOperationResult, ZipPhase, ZipPreview, ZipProgress, apply_reviewed_with_progress,
    preview_compress, preview_extract, publication_supported,
};
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZipMode {
    Compress,
    Extract,
}

#[derive(Clone, PartialEq, Eq)]
struct ReviewedInputs {
    mode: ZipMode,
    source: String,
    destination: String,
}

/// What a worker runs. The default applies the reviewed preview through the
/// core; tests substitute slow or failing runners to exercise the page.
type Runner = Arc<
    dyn Fn(&ZipPreview, &AtomicBool, &ZipProgress) -> Result<ZipOperationResult, ZipError>
        + Send
        + Sync,
>;

fn real_runner() -> Runner {
    Arc::new(apply_reviewed_with_progress)
}

/// What the running worker was given, kept for truthful reporting. Results are
/// described from this record, never from the (possibly edited) input fields.
struct JobDescription {
    mode: ZipMode,
    destination: PathBuf,
}

/// One background conversion. The worker owns the exact reviewed `ZipPreview`
/// moved into it; this handle can only observe it, cancel it, or read its result.
struct ActiveJob {
    receiver: Receiver<Result<ZipOperationResult, ZipError>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<ZipProgress>,
    description: JobDescription,
    started: Instant,
    cancel_requested: bool,
}

impl Drop for ActiveJob {
    /// An abandoned worker (page state dropped) stops at its next safe point.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

pub(crate) struct ZipConverterPageState {
    mode: ZipMode,
    source: String,
    destination: String,
    preview: Option<ZipPreview>,
    reviewed_inputs: Option<ReviewedInputs>,
    result: Option<ZipOperationResult>,
    /// Mode of the operation that produced `result`.
    finished_mode: ZipMode,
    error: Option<String>,
    /// Neutral outcome text, e.g. a cancellation (not an error).
    notice: Option<String>,
    job: Option<ActiveJob>,
    runner: Runner,
}

impl Default for ZipConverterPageState {
    fn default() -> Self {
        Self {
            mode: ZipMode::Compress,
            source: String::new(),
            destination: String::new(),
            preview: None,
            reviewed_inputs: None,
            result: None,
            finished_mode: ZipMode::Compress,
            error: None,
            notice: None,
            job: None,
            runner: real_runner(),
        }
    }
}

impl ZipConverterPageState {
    /// A worker is converting; the reviewed operation must not be disturbed.
    fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn choose_source(&mut self) {
        if self.busy() {
            return;
        }
        let path = match self.mode {
            ZipMode::Compress => rfd::FileDialog::new().pick_file(),
            ZipMode::Extract => rfd::FileDialog::new()
                .add_filter("ZIP archive", &["zip"])
                .pick_file(),
        };
        if let Some(path) = path {
            self.source = path.display().to_string();
            self.invalidate_if_changed();
            self.result = None;
        }
    }

    fn choose_folder_source(&mut self) {
        if self.busy() {
            return;
        }
        if self.mode == ZipMode::Compress
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            self.source = path.display().to_string();
            self.invalidate_if_changed();
            self.result = None;
        }
    }

    fn choose_destination(&mut self) {
        if self.busy() {
            return;
        }
        let path = match self.mode {
            ZipMode::Compress => rfd::FileDialog::new()
                .add_filter("ZIP archive", &["zip"])
                .save_file(),
            ZipMode::Extract => rfd::FileDialog::new().save_file(),
        };
        if let Some(path) = path {
            self.destination = path.display().to_string();
            self.invalidate_if_changed();
            self.result = None;
        }
    }

    fn set_mode(&mut self, mode: ZipMode) {
        if self.busy() {
            return;
        }
        if self.mode != mode {
            self.clear_review("operation changed");
            self.mode = mode;
            self.source.clear();
            self.destination.clear();
            self.invalidate_if_changed();
            self.result = None;
        }
    }

    fn inputs(&self) -> ReviewedInputs {
        ReviewedInputs {
            mode: self.mode,
            source: self.source.clone(),
            destination: self.destination.clone(),
        }
    }

    fn clear_review(&mut self, reason: &str) {
        if self.preview.take().is_some() {
            self.error = Some(format!(
                "The reviewed {reason}. Preview again before applying."
            ));
        }
        self.reviewed_inputs = None;
        self.result = None;
        self.notice = None;
    }

    fn invalidate_if_changed(&mut self) {
        if let Some(inputs) = &self.reviewed_inputs {
            let reason = if inputs.mode != self.mode {
                Some("operation changed")
            } else if inputs.source != self.source {
                Some("source field changed")
            } else if inputs.destination != self.destination {
                Some("destination field changed")
            } else {
                None
            };
            if let Some(reason) = reason {
                self.clear_review(reason);
            }
        }
    }

    fn preview(&mut self) {
        if self.busy() {
            return;
        }
        self.notice = None;
        self.error = None;
        self.result = None;
        self.preview = None;
        self.reviewed_inputs = None;
        // Whitespace is part of a filename. Never trim reviewed paths.
        let source = PathBuf::from(&self.source);
        let destination = PathBuf::from(&self.destination);
        if self.source.is_empty() || self.destination.is_empty() {
            self.error = Some("Choose a source and destination first.".into());
            return;
        }
        let result = match self.mode {
            ZipMode::Compress => preview_compress(&source, &destination),
            ZipMode::Extract => preview_extract(&source, &destination),
        };
        match result {
            Ok(preview) => {
                self.reviewed_inputs = Some(self.inputs());
                self.preview = Some(preview);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Starts the reviewed operation on a worker thread. The worker receives the
    /// retained `ZipPreview` itself (moved, not rebuilt from the fields), so what
    /// runs is exactly what was reviewed. A second request while one is running
    /// is ignored.
    fn execute(&mut self, context: &egui::Context) {
        if self.busy() {
            return;
        }
        self.result = None;
        self.notice = None;
        self.invalidate_if_changed();
        let Some(preview) = self.preview.take() else {
            if self.error.is_none() {
                self.error = Some("Preview this operation before applying.".into());
            }
            return;
        };
        if self.reviewed_inputs.take().as_ref() != Some(&self.inputs()) {
            self.error = Some("The reviewed inputs changed. Preview again before applying.".into());
            return;
        }
        if let Err(error) = preview.apply_support() {
            self.error = Some(error.to_string());
            return;
        }
        self.error = None;
        let description = JobDescription {
            mode: self.mode,
            destination: preview.destination.clone(),
        };
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(ZipProgress::default());
        let runner = Arc::clone(&self.runner);
        let (worker_cancel, worker_progress) = (Arc::clone(&cancel), Arc::clone(&progress));
        let repaint = context.clone();
        let spawned = std::thread::Builder::new()
            .name("zip-converter".into())
            .spawn(move || {
                let outcome = runner(&preview, &worker_cancel, &worker_progress);
                // The page may be gone; nobody left to tell is not an error.
                let _ = sender.send(outcome);
                repaint.request_repaint();
            });
        if let Err(error) = spawned {
            self.error = Some(format!(
                "The ZIP worker could not be started ({error}). Nothing was changed; Preview again to retry."
            ));
            return;
        }
        self.job = Some(ActiveJob {
            receiver,
            cancel,
            progress,
            description,
            started: Instant::now(),
            cancel_requested: false,
        });
    }

    /// Asks the worker to stop at its next safe checkpoint. It cannot remove
    /// anything that existed before the operation: only the worker's own private
    /// staging object is ever deleted.
    fn cancel_job(&mut self) {
        if let Some(job) = &mut self.job
            && job.progress.snapshot().phase != ZipPhase::Publishing
        {
            job.cancel_requested = true;
            job.cancel.store(true, Ordering::Release);
        }
    }

    /// Collects a finished worker's outcome. The outcome only ever produces a
    /// message: it never restores a preview or an approval, so a stale result
    /// cannot re-arm anything.
    fn poll(&mut self, context: &egui::Context) {
        let Some(job) = &self.job else {
            return;
        };
        let outcome = match job.receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => {
                context.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(ZipError::Io(format!(
                "the ZIP worker stopped unexpectedly, so EmuWiz cannot confirm whether {} was published. Check that folder before trying again",
                job.description.destination.display()
            ))),
        };
        let mode = job.description.mode;
        self.job = None;
        self.preview = None;
        self.reviewed_inputs = None;
        match outcome {
            // Finished work is reported as finished even if Cancel was pressed a
            // moment too late.
            Ok(result) => {
                self.finished_mode = mode;
                self.result = Some(result);
            }
            Err(ZipError::Cancelled) => {
                self.notice = Some(
                    "Cancelled. Nothing was published and no existing file was changed. Preview again to start over."
                        .into(),
                );
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
}

/// The explanation shown on systems that cannot publish ZIP output safely.
fn platform_notice(support: Result<(), ZipError>) -> Option<String> {
    support.err().map(|error| error.to_string())
}

fn phase_label(phase: ZipPhase) -> &'static str {
    match phase {
        ZipPhase::Waiting => "starting",
        ZipPhase::Checking => "checking that the reviewed files are unchanged",
        ZipPhase::Staging => "copying into a private working area",
        ZipPhase::Verifying => "verifying the staged output",
        ZipPhase::Publishing => "publishing verified file (no overwrite)",
        ZipPhase::Done => "finishing",
    }
}

pub(crate) fn show(ui: &mut egui::Ui, state: &mut ZipConverterPageState) {
    state.poll(ui.ctx());
    ui.heading("ZIP Converter");
    ui.label("Create or unpack ZIP files with safe paths and verification.");
    if let Some(notice) = platform_notice(publication_supported()) {
        ui.colored_label(egui::Color32::from_rgb(220, 150, 80), notice);
    }
    // While a worker runs, the reviewed operation is fixed: inputs are locked and
    // only Cancel is available.
    let editable = !state.busy();
    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(state.mode == ZipMode::Compress, "Compress")
                .clicked()
            {
                state.set_mode(ZipMode::Compress);
            }
            if ui
                .selectable_label(state.mode == ZipMode::Extract, "Extract one file")
                .clicked()
            {
                state.set_mode(ZipMode::Extract);
            }
        });
    });
    ui.add_enabled_ui(editable, |ui| show_inputs(ui, state));
    state.invalidate_if_changed();
    ui.add_enabled_ui(editable, |ui| {
        if ui.button("Preview").clicked() {
            state.preview();
        }
    });
    show_job(ui, state);
    show_review_and_outcome(ui, state);
}

fn show_inputs(ui: &mut egui::Ui, state: &mut ZipConverterPageState) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Source:");
        if ui
            .add_sized(
                [ui.available_width().clamp(220.0, 560.0), 24.0],
                egui::TextEdit::singleline(&mut state.source),
            )
            .changed()
        {
            state.clear_review("source field changed");
        }
        if ui.button("Choose file").clicked() {
            state.choose_source();
        }
        if state.mode == ZipMode::Compress && ui.button("Choose folder").clicked() {
            state.choose_folder_source();
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(if state.mode == ZipMode::Compress {
            "ZIP destination:"
        } else {
            "New output file (single-file ZIP):"
        });
        if ui
            .add_sized(
                [ui.available_width().clamp(220.0, 560.0), 24.0],
                egui::TextEdit::singleline(&mut state.destination),
            )
            .changed()
        {
            state.clear_review("destination field changed");
        }
        if ui.button("Choose destination").clicked() {
            state.choose_destination();
        }
    });
}

/// Truthful progress and the only control available while a worker runs.
fn show_job(ui: &mut egui::Ui, state: &mut ZipConverterPageState) {
    let mut cancel_clicked = false;
    if let Some(job) = &state.job {
        let snapshot = job.progress.snapshot();
        ui.separator();
        ui.label(format!(
            "{}: {}",
            if job.description.mode == ZipMode::Compress {
                "Creating ZIP"
            } else {
                "Extracting ZIP"
            },
            phase_label(snapshot.phase)
        ));
        if snapshot.phase == ZipPhase::Staging && snapshot.bytes_total > 0 {
            let fraction = (snapshot.bytes_done as f32 / snapshot.bytes_total as f32).min(1.0);
            ui.add(egui::ProgressBar::new(fraction).text(format!(
                "{} of {} bytes · {} of {} files",
                snapshot.bytes_done,
                snapshot.bytes_total,
                snapshot.files_done,
                snapshot.files_total
            )));
        }
        ui.small(format!(
            "Running for {} s. The window stays usable; the source is not modified.",
            job.started.elapsed().as_secs()
        ));
        if snapshot.phase == ZipPhase::Publishing {
            ui.label("Publishing now; this can no longer be cancelled.");
        } else if job.cancel_requested {
            ui.add_enabled(false, egui::Button::new("Cancel requested"));
            ui.small("Cancellation requested. The final result will confirm whether publication had already begun.");
        } else {
            cancel_clicked = ui.button("Cancel").clicked();
            ui.small(
                "Cancel requests a stop before publication. The final result confirms the outcome; existing files are never removed.",
            );
        }
    }
    if cancel_clicked {
        state.cancel_job();
    }
}

fn show_review_and_outcome(ui: &mut egui::Ui, state: &mut ZipConverterPageState) {
    if let Some(preview) = &state.preview {
        ui.separator();
        ui.label("Review the output before creating it. The source is unchanged and existing destinations are refused.");
        ui.label(format!(
            "{} files · {} bytes",
            preview
                .entries
                .iter()
                .filter(|entry| !entry.directory)
                .count(),
            preview.total_size
        ));
        ui.label(format!("Reviewed source: {}", preview.source.display()));
        ui.label(format!("Destination: {}", preview.destination.display()));
        if let Some(digest) = preview.archive_sha256() {
            ui.monospace(format!("Reviewed ZIP SHA-256: {digest}"));
        }
        if let Err(error) = preview.apply_support() {
            ui.colored_label(egui::Color32::YELLOW, error.to_string());
        }
        ui.small("Apply requires local ext4, anonymous staging and /proc/self/fd. Source media is never modified. Interrupted publication may need manual inspection; no automatic Undo or retry.");
        egui::ScrollArea::vertical()
            .max_height(140.0)
            .show(ui, |ui| {
                for entry in preview.entries.iter().take(200) {
                    ui.label(format!(
                        "{}{} · {} bytes",
                        entry.name,
                        if entry.directory { "/" } else { "" },
                        entry.size
                    ));
                }
                if preview.entries.len() > 200 {
                    ui.label("Preview truncated; all entries are still verified.");
                }
            });
        if ui
            .add_enabled(
                preview.can_apply(),
                egui::Button::new(if state.mode == ZipMode::Compress {
                    "Confirm and create ZIP"
                } else {
                    "Confirm and extract file"
                }),
            )
            .clicked()
        {
            let context = ui.ctx().clone();
            state.execute(&context);
        }
    }
    if let Some(notice) = &state.notice {
        ui.label(notice);
    }
    if let Some(result) = &state.result {
        ui.colored_label(egui::Color32::from_rgb(90, 190, 110), "Verified");
        ui.label(if state.finished_mode == ZipMode::Compress {
            "ZIP created and verified."
        } else {
            "Single file extracted and verified."
        });
        ui.label(&result.verification);
    }
    if let Some(error) = &state.error {
        let collision = error.contains("Destination already exists");
        ui.colored_label(
            egui::Color32::from_rgb(220, 150, 80),
            if collision {
                "Blocked"
            } else {
                "Needs attention"
            },
        );
        if collision {
            ui.label("Blocked: this output already exists. EmuWiz will not replace it; choose another destination and Preview again.");
        } else {
            ui.label(error);
        }
        crate::ui::components::technical_details(ui, ("zip-conversion-error",), |ui| {
            ui.label(error);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    /// Collects any finished worker outcome, waiting for the worker if needed.
    fn settle(state: &mut ZipConverterPageState, context: &egui::Context) {
        wait_until("the worker to finish", || {
            state.poll(context);
            !state.busy()
        });
    }

    /// Starts the reviewed operation and waits for the worker to finish.
    fn run(state: &mut ZipConverterPageState) {
        let context = egui::Context::default();
        state.execute(&context);
        settle(state, &context);
    }

    /// A controllable stand-in for the core: blocks until released or cancelled.
    #[derive(Default)]
    struct Probe {
        calls: AtomicUsize,
        released: AtomicBool,
        cancelled: AtomicBool,
        /// After seeing Cancel, keep running until released (so a test can look
        /// at the "cancel requested" state before the worker settles).
        linger_after_cancel: AtomicBool,
        worker: Mutex<Option<std::thread::ThreadId>>,
        seen: Mutex<Vec<(PathBuf, PathBuf)>>,
    }

    fn synthetic_result(preview: &ZipPreview) -> ZipOperationResult {
        ZipOperationResult {
            source: preview.source.clone(),
            destination: preview.destination.clone(),
            files: 1,
            total_size: 8,
            verification: "synthetic worker finished".into(),
        }
    }

    /// `finish_on_cancel`: complete successfully when cancelled (a cancel that
    /// arrives too late) instead of reporting a cancellation.
    fn blocking_runner(probe: &Arc<Probe>, finish_on_cancel: bool) -> Runner {
        let probe = Arc::clone(probe);
        Arc::new(move |preview, cancel, _progress| {
            probe.calls.fetch_add(1, Ordering::SeqCst);
            *probe.worker.lock().unwrap() = Some(std::thread::current().id());
            probe
                .seen
                .lock()
                .unwrap()
                .push((preview.source.clone(), preview.destination.clone()));
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                if cancel.load(Ordering::Acquire) {
                    probe.cancelled.store(true, Ordering::SeqCst);
                    while probe.linger_after_cancel.load(Ordering::SeqCst)
                        && !probe.released.load(Ordering::SeqCst)
                    {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    return if finish_on_cancel {
                        Ok(synthetic_result(preview))
                    } else {
                        Err(ZipError::Cancelled)
                    };
                }
                if probe.released.load(Ordering::SeqCst) {
                    return Ok(synthetic_result(preview));
                }
                if Instant::now() > deadline {
                    return Err(ZipError::Io("test worker timed out".into()));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    }

    fn frame(context: &egui::Context, state: &mut ZipConverterPageState) -> String {
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, state));
        });
        rendered_text(&output)
    }

    #[test]
    fn edited_destination_refuses_apply_of_old_preview() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.bin");
        let a = root.path().join("reviewed.zip");
        let b = root.path().join("unreviewed.zip");
        std::fs::write(&source, b"reviewed").unwrap();
        let mut state = ZipConverterPageState {
            source: source.to_str().unwrap().into(),
            destination: a.to_str().unwrap().into(),
            ..Default::default()
        };
        state.preview();
        assert_eq!(state.preview.as_ref().unwrap().destination, a);
        state.destination = b.to_str().unwrap().into();
        run(&mut state);
        assert!(!b.exists(), "unreviewed destination B must not be created");
        assert!(state.preview.is_none());
        assert!(
            state
                .error
                .as_deref()
                .unwrap()
                .contains("destination field changed")
        );
        assert!(!a.exists());
    }

    #[test]
    fn changed_source_contents_refuse_apply_of_old_preview() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.bin");
        let destination = root.path().join("reviewed.zip");
        std::fs::write(&source, b"reviewed").unwrap();
        let mut state = ZipConverterPageState {
            source: source.to_str().unwrap().into(),
            destination: destination.to_str().unwrap().into(),
            ..Default::default()
        };
        state.preview();
        std::fs::write(&source, b"unapproved contents").unwrap();
        run(&mut state);
        assert!(
            !destination.exists(),
            "changed contents must not be published"
        );
        assert!(state.result.is_none());
        assert!(state.error.as_deref().unwrap().contains("source"));
        assert!(state.error.as_deref().unwrap().contains("Preview again"));
    }

    fn rendered_text(output: &egui::FullOutput) -> String {
        fn collect(shape: &egui::Shape, output: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    output.push_str(text.galley.text());
                    output.push('\n');
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, output);
                    }
                }
                _ => {}
            }
        }
        let mut text = String::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
    }

    fn reviewed_state() -> (tempfile::TempDir, ZipConverterPageState) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.bin");
        std::fs::write(&source, b"reviewed").unwrap();
        let mut state = ZipConverterPageState {
            source: source.to_str().unwrap().into(),
            destination: root.path().join("out.zip").to_str().unwrap().into(),
            ..Default::default()
        };
        state.preview();
        assert!(state.preview.is_some());
        (root, state)
    }

    #[test]
    fn mode_switch_clears_previous_preview() {
        let (_root, mut state) = reviewed_state();
        state.set_mode(ZipMode::Extract);
        assert!(state.preview.is_none());
        assert!(state.reviewed_inputs.is_none());
        assert!(state.source.is_empty());
    }

    #[test]
    fn zip_preview_explains_copy_safety_before_create() {
        let (_root, mut state) = reviewed_state();
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        let text = rendered_text(&output);
        assert!(text.contains("Review the output before creating it"));
        assert!(text.contains("source is unchanged"));
        assert!(text.contains(&format!("Destination: {}", state.destination)));
        assert!(text.contains("Confirm and create ZIP"));
        assert!(state.result.is_none());
    }

    #[test]
    fn rendered_input_change_removes_apply_confirmation() {
        let (_root, mut state) = reviewed_state();
        state.source.push_str("-changed");
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        let text = rendered_text(&output);
        assert!(state.preview.is_none());
        assert!(!text.contains("Confirm and create ZIP"));
        assert!(text.contains("source field changed"));
    }

    #[test]
    fn failed_new_preview_does_not_retain_old_approval() {
        let (_root, mut state) = reviewed_state();
        state.source.clear();
        state.preview();
        assert!(state.preview.is_none());
        assert!(state.reviewed_inputs.is_none());
        run(&mut state);
        assert!(state.result.is_none());
    }

    #[test]
    fn successful_apply_consumes_review_and_preserves_whitespace() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join(" source Ω ");
        std::fs::write(&source, b"reviewed").unwrap();
        let destination = root.path().join(" out.zip ");
        let mut state = ZipConverterPageState {
            source: source.to_str().unwrap().into(),
            destination: destination.to_str().unwrap().into(),
            ..Default::default()
        };
        state.preview();
        run(&mut state);
        assert!(state.result.is_some(), "{:?}", state.error);
        assert!(destination.exists());
        assert!(state.preview.is_none());
        run(&mut state);
        assert!(state.result.is_none());
        assert!(state.error.as_deref().unwrap().contains("Preview"));
    }

    #[test]
    fn extraction_edit_requires_a_new_preview() {
        let (root, mut state) = reviewed_state();
        run(&mut state);
        let zip = state.result.as_ref().unwrap().destination.clone();
        state.set_mode(ZipMode::Extract);
        state.source = zip.to_str().unwrap().into();
        state.destination = root.path().join("reviewed-output").to_str().unwrap().into();
        state.preview();
        let reviewed = state.preview.as_ref().unwrap().destination.clone();
        let unreviewed = root.path().join("edited-output");
        state.destination = unreviewed.to_str().unwrap().into();
        run(&mut state);
        assert!(!reviewed.exists());
        assert!(!unreviewed.exists());
        assert!(
            state
                .error
                .as_deref()
                .unwrap()
                .contains("destination field changed")
        );
        state.preview();
        run(&mut state);
        assert!(state.result.is_some(), "{:?}", state.error);
        assert!(unreviewed.exists());
        assert!(!reviewed.exists());
    }

    #[test]
    fn operation_edit_cannot_execute_compression_approval_as_extraction() {
        let (_root, mut state) = reviewed_state();
        state.mode = ZipMode::Extract;
        run(&mut state);
        assert!(state.preview.is_none());
        assert!(state.result.is_none());
        assert!(
            state
                .error
                .as_deref()
                .unwrap()
                .contains("operation changed")
        );
    }

    #[test]
    fn zip_destination_conflict_is_a_blocker_not_an_overwrite_action() {
        let mut state = ZipConverterPageState {
            error: Some("Destination already exists.".into()),
            ..Default::default()
        };
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        let text = rendered_text(&output);
        assert!(text.contains("Blocked"));
        assert!(text.contains("EmuWiz will not replace it"));
        assert!(!text.contains("Force"));
        assert!(state.result.is_none());
    }

    #[test]
    fn apply_runs_off_the_gui_thread_and_the_window_stays_responsive() {
        let (_root, mut state) = reviewed_state();
        let probe = Arc::new(Probe::default());
        state.runner = blocking_runner(&probe, false);
        let context = egui::Context::default();
        let started = Instant::now();
        state.execute(&context);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "execute must hand off to a worker, not run the conversion"
        );
        assert!(state.busy());
        wait_until("the worker to start", || {
            probe.calls.load(Ordering::SeqCst) == 1
        });
        // The worker is blocked, yet every frame renders promptly and shows
        // honest progress with a working Cancel and no way to apply again.
        for _ in 0..5 {
            let frame_started = Instant::now();
            let text = frame(&context, &mut state);
            assert!(frame_started.elapsed() < Duration::from_secs(2));
            assert!(state.busy());
            assert!(text.contains("Creating ZIP"), "{text}");
            assert!(text.contains("Cancel"), "{text}");
            assert!(!text.contains("Confirm and create ZIP"), "{text}");
            assert!(!text.contains('%'), "no invented percentage: {text}");
        }
        let worker = probe.worker.lock().unwrap().unwrap();
        assert_ne!(worker, std::thread::current().id());
        probe.released.store(true, Ordering::SeqCst);
        settle(&mut state, &context);
        assert!(state.result.is_some());
        assert!(frame(&context, &mut state).contains("ZIP created and verified."));
    }

    #[test]
    fn duplicate_apply_submissions_are_ignored() {
        let (_root, mut state) = reviewed_state();
        let probe = Arc::new(Probe::default());
        state.runner = blocking_runner(&probe, false);
        let context = egui::Context::default();
        state.execute(&context);
        state.execute(&context);
        state.execute(&context);
        assert!(state.error.is_none() && state.notice.is_none());
        probe.released.store(true, Ordering::SeqCst);
        settle(&mut state, &context);
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        // The approval was consumed by the first run; another needs a new preview.
        state.execute(&context);
        assert!(!state.busy());
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        assert!(state.error.as_deref().unwrap().contains("Preview"));
    }

    #[test]
    fn cancel_reaches_the_worker_and_never_restores_the_approval() {
        let (_root, mut state) = reviewed_state();
        let probe = Arc::new(Probe::default());
        state.runner = blocking_runner(&probe, false);
        let context = egui::Context::default();
        state.execute(&context);
        wait_until("the worker to start", || {
            probe.calls.load(Ordering::SeqCst) == 1
        });
        probe.linger_after_cancel.store(true, Ordering::SeqCst);
        state.cancel_job();
        assert!(state.job.as_ref().unwrap().cancel_requested);
        wait_until("the worker to see Cancel", || {
            probe.cancelled.load(Ordering::SeqCst)
        });
        let text = frame(&context, &mut state);
        assert!(text.contains("Cancel requested"), "{text}");
        assert!(text.contains("Cancellation requested"), "{text}");
        probe.released.store(true, Ordering::SeqCst);
        settle(&mut state, &context);
        assert!(probe.cancelled.load(Ordering::SeqCst));
        assert!(state.result.is_none() && state.error.is_none());
        assert!(state.notice.as_deref().unwrap().contains("Cancelled"));
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .contains("Nothing was published")
        );
        // A cancelled run does not leave an approval behind to re-apply.
        assert!(state.preview.is_none() && state.reviewed_inputs.is_none());
        state.execute(&context);
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn work_that_finished_anyway_is_reported_as_finished_not_cancelled() {
        let (_root, mut state) = reviewed_state();
        let probe = Arc::new(Probe::default());
        state.runner = blocking_runner(&probe, true);
        let context = egui::Context::default();
        state.execute(&context);
        wait_until("the worker to start", || {
            probe.calls.load(Ordering::SeqCst) == 1
        });
        state.cancel_job();
        settle(&mut state, &context);
        assert!(state.result.is_some(), "completed work must be reported");
        assert!(state.notice.is_none() && state.error.is_none());
    }

    #[test]
    fn selection_changes_while_a_worker_runs_cannot_alter_it_or_rearm_approval() {
        let (root, mut state) = reviewed_state();
        let reviewed_source = state.source.clone();
        let reviewed_destination = state.destination.clone();
        let probe = Arc::new(Probe::default());
        state.runner = blocking_runner(&probe, false);
        let context = egui::Context::default();
        state.execute(&context);
        wait_until("the worker to start", || {
            probe.calls.load(Ordering::SeqCst) == 1
        });
        // Every way of changing the selection is refused or inert while active.
        state.set_mode(ZipMode::Extract);
        assert_eq!(state.mode, ZipMode::Compress);
        state.source = root.path().join("other.bin").display().to_string();
        state.destination = root.path().join("other.zip").display().to_string();
        state.preview();
        assert!(state.preview.is_none() && state.reviewed_inputs.is_none());
        state.choose_source();
        state.choose_destination();
        assert!(frame(&context, &mut state).contains("Cancel"));
        probe.released.store(true, Ordering::SeqCst);
        settle(&mut state, &context);
        // The worker ran the reviewed operation, not the edited fields.
        let seen = probe.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, PathBuf::from(&reviewed_source));
        assert_eq!(seen[0].1, PathBuf::from(&reviewed_destination));
        assert_eq!(
            state.result.as_ref().unwrap().destination,
            PathBuf::from(&reviewed_destination)
        );
        // Its result did not bring back any approval for the edited fields.
        assert!(state.preview.is_none() && state.reviewed_inputs.is_none());
        state.execute(&context);
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        assert!(!root.path().join("other.zip").exists());
    }

    #[test]
    fn real_background_conversion_publishes_exactly_the_reviewed_output() {
        let (root, mut state) = reviewed_state();
        let archive = PathBuf::from(&state.destination);
        let context = egui::Context::default();
        state.execute(&context);
        assert!(state.busy() || state.result.is_some() || state.error.is_some());
        settle(&mut state, &context);
        assert!(state.result.is_some(), "{:?}", state.error);
        assert!(archive.is_file());
        // And back again, through the same background path.
        state.set_mode(ZipMode::Extract);
        state.source = archive.display().to_string();
        state.destination = root.path().join("unpacked").display().to_string();
        state.preview();
        run(&mut state);
        assert!(state.result.is_some(), "{:?}", state.error);
        assert_eq!(
            std::fs::read(root.path().join("unpacked")).unwrap(),
            b"reviewed"
        );
        assert!(std::fs::read_dir(root.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".emuwiz-zip")
        }));
    }

    #[test]
    fn destination_collision_during_background_work_is_never_clobbered() {
        let (_root, mut state) = reviewed_state();
        let destination = PathBuf::from(&state.destination);
        let racing = destination.clone();
        // The user (or another program) creates the output while the worker runs.
        state.runner = Arc::new(move |preview, cancel, progress| {
            std::fs::write(&racing, b"someone else's file").unwrap();
            apply_reviewed_with_progress(preview, cancel, progress)
        });
        run(&mut state);
        assert!(state.result.is_none());
        assert_eq!(std::fs::read(&destination).unwrap(), b"someone else's file");
        let error = state.error.clone().unwrap();
        assert!(error.contains("Destination already exists"), "{error}");
        let context = egui::Context::default();
        let text = frame(&context, &mut state);
        assert!(text.contains("Blocked"), "{text}");
        assert!(text.contains("Preview again"), "{text}");
    }

    #[test]
    fn cancel_before_the_worker_starts_publishes_nothing_and_removes_nothing() {
        let (root, mut state) = reviewed_state();
        let before: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let cancelled = Arc::new(AtomicBool::new(false));
        let gate = Arc::clone(&cancelled);
        // Holds the real core back until Cancel was pressed, then runs it.
        state.runner = Arc::new(move |preview, cancel, progress| {
            wait_until("cancel", || cancel.load(Ordering::Acquire));
            gate.store(true, Ordering::SeqCst);
            apply_reviewed_with_progress(preview, cancel, progress)
        });
        let context = egui::Context::default();
        state.execute(&context);
        state.cancel_job();
        settle(&mut state, &context);
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(state.notice.as_deref().unwrap().contains("Cancelled"));
        let after: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let mut before = before;
        let mut after = after;
        before.sort();
        after.sort();
        assert_eq!(before, after);
    }

    #[test]
    fn a_worker_that_dies_is_not_reported_as_success() {
        let (_root, mut state) = reviewed_state();
        let destination = state.destination.clone();
        state.runner = Arc::new(|_, _, _| panic!("synthetic worker crash"));
        run(&mut state);
        assert!(state.result.is_none());
        let error = state.error.clone().unwrap();
        assert!(error.contains("cannot confirm"), "{error}");
        assert!(error.contains(&destination), "{error}");
        assert!(state.preview.is_none() && !state.busy());
    }

    #[test]
    fn dropping_the_page_cancels_an_abandoned_worker() {
        let (_root, mut state) = reviewed_state();
        let probe = Arc::new(Probe::default());
        state.runner = blocking_runner(&probe, false);
        let context = egui::Context::default();
        state.execute(&context);
        wait_until("the worker to start", || {
            probe.calls.load(Ordering::SeqCst) == 1
        });
        drop(state);
        wait_until("the abandoned worker to stop", || {
            probe.cancelled.load(Ordering::SeqCst)
        });
    }

    #[test]
    fn unsupported_systems_get_an_accurate_explanation_during_preview() {
        let text = platform_notice(Err(ZipError::UnsupportedPlatform)).unwrap();
        assert!(text.contains("unavailable on this system"), "{text}");
        assert!(text.contains("no-replace"), "{text}");
        assert!(text.contains("Nothing was read or changed"), "{text}");
        assert!(platform_notice(Ok(())).is_none());
        // On the supported platform nothing misleading is shown.
        let context = egui::Context::default();
        let mut state = ZipConverterPageState::default();
        assert!(!frame(&context, &mut state).contains("unavailable on this system"));
    }

    #[cfg(unix)]
    #[test]
    fn staging_errors_name_the_chosen_folder_and_no_internal_names() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.bin");
        std::fs::write(&source, b"reviewed").unwrap();
        let locked = root.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let mut state = ZipConverterPageState {
            source: source.display().to_string(),
            destination: locked.join("out.zip").display().to_string(),
            ..Default::default()
        };
        state.preview();
        assert!(state.preview.is_some(), "{:?}", state.error);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        // Permission bits do not bind a privileged user; nothing to show then.
        if std::fs::File::create(locked.join(".writable-probe")).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }
        run(&mut state);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        let context = egui::Context::default();
        let text = frame(&context, &mut state);
        let folder = locked.display().to_string();
        assert!(text.contains(&folder), "{text}");
        let outside = text.replace(&folder, "<folder>");
        for internal in ["/proc", "fd/", ".emuwiz-zip"] {
            assert!(!outside.contains(internal), "{internal} leaked: {text}");
        }
    }

    #[test]
    fn multi_file_extraction_stays_preview_only_and_cannot_start_a_worker() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("input");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("one"), b"A").unwrap();
        std::fs::write(folder.join("two"), b"B").unwrap();
        let archive = root.path().join("input.zip");
        let reviewed = preview_compress(&folder, &archive).unwrap();
        archivefs_core::zip_converter::reviewed_apply::apply_reviewed(
            &reviewed,
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut state = ZipConverterPageState {
            mode: ZipMode::Extract,
            source: archive.display().to_string(),
            destination: root.path().join("output").display().to_string(),
            ..Default::default()
        };
        state.preview();
        assert!(!state.preview.as_ref().unwrap().can_apply());
        let context = egui::Context::default();
        let text = frame(&context, &mut state);
        assert!(text.contains("preview-only"), "{text}");
        state.execute(&context);
        assert!(state.job.is_none() && state.result.is_none());
        assert!(!root.path().join("output").exists());
    }
    #[test]
    fn uncertain_publication_never_shows_verified_or_rearms_review() {
        let (root, mut state) = reviewed_state();
        let output = PathBuf::from(&state.destination);
        state.runner = Arc::new(|preview, _, _| {
            std::fs::write(&preview.destination, b"retained output").unwrap();
            Err(ZipError::PublicationUncertain {
                destination: preview.destination.clone(),
                detail: "injected acknowledgement loss".into(),
            })
        });
        run(&mut state);
        assert!(state.result.is_none() && state.preview.is_none());
        assert!(
            state
                .error
                .as_ref()
                .unwrap()
                .contains("may have been published")
        );
        assert_eq!(std::fs::read(output).unwrap(), b"retained output");
        assert_eq!(
            std::fs::read(root.path().join("source.bin")).unwrap(),
            b"reviewed"
        );
        let text = frame(&egui::Context::default(), &mut state);
        assert!(!text.contains("ZIP created and verified."));
    }
}
