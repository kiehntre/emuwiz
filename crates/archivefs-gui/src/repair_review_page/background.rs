//! Read-only native preview loading; Apply continues through the existing worker.
use super::*;

pub(super) enum Input {
    Plan(
        PathBuf,
        Result<(Box<PreparedPreview>, CountsAvailability), String>,
    ),
    Setup(Vec<PathBuf>),
}

impl RepairReviewPageState {
    pub(crate) fn input_busy(&self) -> bool {
        self.input_job.is_some()
    }

    pub(super) fn queue_plan(&mut self, path: PathBuf) {
        if self.input_busy() || self.is_apply_running() || self.is_scan_running() {
            return;
        }
        // A previous confirmation cannot survive an explicit preview reload.
        self.apply_confirm = None;
        let (sender, receiver) = std::sync::mpsc::channel();
        self.input_job = Some(receiver);
        std::thread::spawn(move || {
            let result = std::fs::read_to_string(&path)
                .map_err(|e| format!("could not read '{}': {e}", path.display()))
                .and_then(|text| {
                    serde_json::from_str::<LibraryRepairPlan>(&text)
                        .map(|plan| {
                            (
                                Box::new(PreparedPreview::new(plan)),
                                CountsAvailability::from_raw_json(&text),
                            )
                        })
                        .map_err(|e| format!("could not parse repair plan: {e}"))
                });
            let _ = sender.send(Input::Plan(path, result));
        });
    }

    pub(super) fn queue_setup(&mut self) {
        if self.input_busy() || self.is_scan_running() || self.is_apply_running() {
            return;
        }
        self.catalogue_picker.ensure_loaded();
        let (sender, receiver) = std::sync::mpsc::channel();
        self.input_job = Some(receiver);
        std::thread::spawn(move || {
            let folders = Config::load_default()
                .map(|config| config.source_folders)
                .unwrap_or_default();
            let _ = sender.send(Input::Setup(folders));
        });
    }

    pub(crate) fn poll_input(&mut self) -> bool {
        let Some(receiver) = &self.input_job else {
            return false;
        };
        match receiver.try_recv() {
            Ok(input) => {
                self.input_job = None;
                match input {
                    Input::Plan(path, Ok((plan, counts))) => {
                        self.adopt_prepared_plan(*plan, Some(path), counts)
                    }
                    Input::Plan(_, Err(error)) => self.error = Some(error),
                    Input::Setup(library_folders) => {
                        self.scan_setup = Some(ScanSetupState {
                            library_folders,
                            ..Default::default()
                        });
                        self.scan_status = None;
                    }
                }
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.input_job = None;
                self.error = Some("Preview input worker stopped. Nothing was applied by this read-only load; reload the preview.".into());
                true
            }
        }
    }
}
