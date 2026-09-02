//! Approval-bound managed emulator downloads for the Emulator Setup page.
//!
//! This is a thin GUI adapter over the existing
//! [`archivefs_core::emulator_download`] backend. It never downloads on its
//! own: a novice clicks **Download emulator**, EmuWiz resolves the exact
//! stable release and asset **read-only**, shows a confirmation panel, and
//! only after an explicit **Install emulator** click does any byte get
//! fetched or written. Discovery, Doctor findings and launch readiness stay
//! authoritative for whether Play is available - "Installed" here never
//! means "Ready to play".

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use archivefs_core::emulator_download::{
    DownloadError, EMULATOR_DOWNLOAD_CATALOGUE, EmulatorDistribution, EmulatorDownloadCancellation,
    EmulatorDownloadOptions, EmulatorDownloadPlan, EmulatorDownloadProgress,
    EmulatorDownloadProgressPhase, EmulatorDownloadProgressReporter, EmulatorDownloadReceipt,
    EmulatorDownloadSpec, HttpsEmulatorDownloadTransport, download_and_install_resolved,
    emulator_download_spec, managed_appimage_install, resolve_download_plan,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// The install root the managed emulator flow reads and writes under
/// (`<data_dir>/emulators/<id>/…`). Resolved once via the same
/// `app_dirs::data_dir()` every other EmuWiz data path uses.
fn managed_root() -> Result<PathBuf, String> {
    archivefs_core::app_dirs::data_dir().map_err(|error| error.to_string())
}

/// One managed emulator's current, novice-facing state in the download UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmulatorDownloadEntryState {
    /// A successfully EmuWiz-managed AppImage is present (not a claim about
    /// launch readiness).
    Installed(PathBuf),
    /// No managed install; a download is offered.
    NotInstalled,
    /// This emulator has no automated AppImage lane - the user installs it
    /// themselves and the existing manual/system setup paths still apply.
    ManualInstallRequired,
    /// Read-only release resolution is running.
    Checking,
    /// Resolution finished; the confirmation panel is shown. For a plan
    /// that would replace an existing managed binary, the replacement
    /// checkbox must be ticked before Install is enabled.
    ReadyToDownload(Box<EmulatorDownloadPlan>),
    /// A replacement plan whose replacement has been explicitly confirmed;
    /// Install is enabled.
    AwaitingConfirmation(Box<EmulatorDownloadPlan>),
    /// Downloading the asset.
    Downloading(EmulatorDownloadProgress),
    /// Validating the downloaded AppImage (ELF/AppImage + checksum).
    Verifying(EmulatorDownloadProgress),
    /// Atomically installing the validated file.
    Installing(EmulatorDownloadProgress),
    /// Installed in this session; carries the provenance receipt.
    Complete(Box<EmulatorDownloadReceipt>),
    /// The last operation was cancelled while it was still safe to stop.
    Cancelled,
    /// The last operation failed; carries the typed backend error.
    Failed(DownloadError),
    /// Defensive: an entry that is not a resolvable managed emulator.
    Unsupported,
}

impl EmulatorDownloadEntryState {
    fn plan(&self) -> Option<&EmulatorDownloadPlan> {
        match self {
            Self::ReadyToDownload(plan) | Self::AwaitingConfirmation(plan) => Some(plan),
            _ => None,
        }
    }

    fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading(_) | Self::Verifying(_) | Self::Installing(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmulatorDownloadPageAction {
    /// Resolve the exact release/asset for `id`, read-only.
    Resolve(String),
    /// Install the plan currently held for `id` (Install emulator click).
    Install(String),
    /// The user ticked the replacement-confirmation checkbox for `id`.
    ConfirmReplacement(String),
    /// Cancel the in-flight operation for `id`.
    Cancel(String),
    /// Dismiss a terminal (Complete / Failed / Cancelled) card for `id`.
    Dismiss(String),
}

enum TaskResult {
    Resolved(Result<EmulatorDownloadPlan, DownloadError>),
    Installed(Result<EmulatorDownloadReceipt, DownloadError>),
}

/// Shared page state, held once on the app and threaded into Emulator Setup.
#[derive(Default)]
pub(crate) struct EmulatorDownloadPageState {
    entries: BTreeMap<&'static str, EmulatorDownloadEntryState>,
    task: Option<(String, Receiver<TaskResult>)>,
    cancellation: Option<EmulatorDownloadCancellation>,
    progress: Arc<Mutex<Option<EmulatorDownloadProgress>>>,
    /// Set to the emulator id when an install completes, so the app can
    /// refresh discovery / Doctor / readiness exactly once.
    completed_install: Option<String>,
    refreshed: bool,
}

impl EmulatorDownloadPageState {
    /// The managed (automated AppImage lane) emulators, catalogue order.
    fn managed_specs() -> impl Iterator<Item = &'static EmulatorDownloadSpec> {
        EMULATOR_DOWNLOAD_CATALOGUE
            .iter()
            .filter(|spec| spec.distribution == EmulatorDistribution::GithubAppImage)
    }

    /// Recompute every non-transient entry from on-disk discovery. Called
    /// on first show and after an install completes. Never disturbs an
    /// in-flight or terminal-in-this-session entry.
    pub(crate) fn refresh(&mut self) {
        let root = managed_root().ok();
        self.refresh_from_root(root.as_deref());
    }

    /// [`Self::refresh`] against an explicit install root (test seam).
    fn refresh_from_root(&mut self, root: Option<&std::path::Path>) {
        self.refreshed = true;
        for spec in EMULATOR_DOWNLOAD_CATALOGUE {
            let keep = matches!(
                self.entries.get(spec.id),
                Some(state)
                    if state.is_busy()
                        || matches!(
                            state,
                            EmulatorDownloadEntryState::ReadyToDownload(_)
                                | EmulatorDownloadEntryState::AwaitingConfirmation(_)
                                | EmulatorDownloadEntryState::Complete(_)
                                | EmulatorDownloadEntryState::Failed(_)
                                | EmulatorDownloadEntryState::Cancelled
                        )
            );
            if keep {
                continue;
            }
            let state = if spec.distribution != EmulatorDistribution::GithubAppImage {
                EmulatorDownloadEntryState::ManualInstallRequired
            } else if let Some(binary) = root.and_then(|root| managed_appimage_install(root, spec))
            {
                EmulatorDownloadEntryState::Installed(binary)
            } else {
                EmulatorDownloadEntryState::NotInstalled
            };
            self.entries.insert(spec.id, state);
        }
    }

    /// Drain the background task channel and fold the result back in.
    pub(crate) fn poll(&mut self, context: &egui::Context) {
        if !self.refreshed {
            self.refresh();
        }
        // Live progress for whatever operation is running.
        if let Some((id, _)) = &self.task {
            let key = catalogue_id(id);
            let latest = self
                .progress
                .lock()
                .ok()
                .and_then(|progress| progress.clone());
            if !key.is_empty()
                && let Some(progress) = latest
                && self
                    .entries
                    .get(key)
                    .is_some_and(EmulatorDownloadEntryState::is_busy)
            {
                let mapped = match progress.phase {
                    EmulatorDownloadProgressPhase::Downloading => {
                        Some(EmulatorDownloadEntryState::Downloading(progress))
                    }
                    EmulatorDownloadProgressPhase::Validating => {
                        Some(EmulatorDownloadEntryState::Verifying(progress))
                    }
                    EmulatorDownloadProgressPhase::Installing => {
                        Some(EmulatorDownloadEntryState::Installing(progress))
                    }
                    _ => None,
                };
                if let Some(state) = mapped {
                    self.entries.insert(key, state);
                }
            }
        }

        let Some((id, receiver)) = self.task.as_ref() else {
            return;
        };
        let id = id.clone();
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                context.request_repaint_after(Duration::from_millis(120));
                return;
            }
            Err(TryRecvError::Disconnected) => TaskResult::Installed(Err(DownloadError::Io(
                "the emulator download background task stopped unexpectedly".into(),
            ))),
        };
        self.task = None;
        self.cancellation = None;
        if let Ok(mut progress) = self.progress.lock() {
            *progress = None;
        }
        self.apply_task_result(&id, result);
        context.request_repaint();
    }

    /// Fold one finished background result into the entry map. Extracted so
    /// state transitions are testable without threads or a network.
    fn apply_task_result(&mut self, id: &str, result: TaskResult) {
        let key = catalogue_id(id);
        if key.is_empty() {
            return;
        }
        match result {
            TaskResult::Resolved(Ok(plan)) => {
                self.entries.insert(
                    key,
                    EmulatorDownloadEntryState::ReadyToDownload(Box::new(plan)),
                );
            }
            TaskResult::Resolved(Err(error)) => {
                self.entries
                    .insert(key, EmulatorDownloadEntryState::Failed(error));
            }
            TaskResult::Installed(Ok(receipt)) => {
                self.completed_install = Some(key.to_string());
                self.entries
                    .insert(key, EmulatorDownloadEntryState::Complete(Box::new(receipt)));
            }
            TaskResult::Installed(Err(DownloadError::Cancelled)) => {
                self.entries
                    .insert(key, EmulatorDownloadEntryState::Cancelled);
            }
            TaskResult::Installed(Err(error)) => {
                self.entries
                    .insert(key, EmulatorDownloadEntryState::Failed(error));
            }
        }
    }

    /// The emulator id whose install just completed, consumed once. The app
    /// uses this to re-run discovery / Doctor / readiness.
    pub(crate) fn take_completed_install(&mut self) -> Option<String> {
        self.completed_install.take()
    }

    pub(crate) fn handle(&mut self, action: EmulatorDownloadPageAction, context: egui::Context) {
        match action {
            EmulatorDownloadPageAction::Resolve(id) => self.start_resolve(id, context),
            EmulatorDownloadPageAction::Install(id) => self.start_install(id, context),
            EmulatorDownloadPageAction::ConfirmReplacement(id) => {
                let key = catalogue_id(&id);
                if let Some(EmulatorDownloadEntryState::ReadyToDownload(plan)) =
                    self.entries.remove(key)
                {
                    self.entries
                        .insert(key, EmulatorDownloadEntryState::AwaitingConfirmation(plan));
                }
            }
            EmulatorDownloadPageAction::Cancel(_) => {
                if let Some(cancellation) = &self.cancellation {
                    cancellation.cancel();
                }
            }
            EmulatorDownloadPageAction::Dismiss(id) => {
                let key = catalogue_id(&id);
                self.entries.remove(key);
                self.refresh();
            }
        }
    }

    fn start_resolve(&mut self, id: String, context: egui::Context) {
        if self.task.is_some() {
            return;
        }
        let Some(spec) = emulator_download_spec(&id) else {
            self.entries
                .insert(catalogue_id(&id), EmulatorDownloadEntryState::Unsupported);
            return;
        };
        let key = catalogue_id(&id);
        self.entries
            .insert(key, EmulatorDownloadEntryState::Checking);
        let (sender, receiver) = mpsc::channel();
        self.task = Some((id.clone(), receiver));
        let spec_id = spec.id;
        thread::spawn(move || {
            let result = (|| {
                let root = managed_root().map_err(DownloadError::Io)?;
                let spec = emulator_download_spec(spec_id)
                    .ok_or_else(|| DownloadError::Unsupported("unknown emulator".into()))?;
                let transport = HttpsEmulatorDownloadTransport::new();
                resolve_download_plan(&root, spec, &transport, &EmulatorDownloadOptions::default())
            })();
            let _ = sender.send(TaskResult::Resolved(result));
            context.request_repaint();
        });
    }

    fn start_install(&mut self, id: String, context: egui::Context) {
        if self.task.is_some() {
            return;
        }
        let key = catalogue_id(&id);
        let plan = match self
            .entries
            .get(key)
            .and_then(EmulatorDownloadEntryState::plan)
        {
            // A replacement plan must be in AwaitingConfirmation (checkbox
            // ticked) before Install is honoured.
            Some(plan) if !plan.replaces_existing_managed => plan.clone(),
            Some(plan)
                if matches!(
                    self.entries.get(key),
                    Some(EmulatorDownloadEntryState::AwaitingConfirmation(_))
                ) =>
            {
                plan.clone()
            }
            _ => return,
        };
        let cancellation = EmulatorDownloadCancellation::default();
        self.cancellation = Some(cancellation.clone());
        let progress_handle = Arc::clone(&self.progress);
        if let Ok(mut current) = progress_handle.lock() {
            *current = None;
        }
        let reporter = EmulatorDownloadProgressReporter::new(move |event| {
            if let Ok(mut current) = progress_handle.lock() {
                *current = Some(event);
            }
        });
        let (sender, receiver) = mpsc::channel();
        self.task = Some((id.clone(), receiver));
        self.entries.insert(
            key,
            EmulatorDownloadEntryState::Downloading(EmulatorDownloadProgress {
                phase: EmulatorDownloadProgressPhase::Downloading,
                release_tag: Some(plan.release_tag.clone()),
                asset_name: Some(plan.asset_name.clone()),
                bytes_received: 0,
                total_bytes: None,
            }),
        );
        let plan_for_thread = plan.clone();
        thread::spawn(move || {
            let result = (|| {
                let root = managed_root().map_err(DownloadError::Io)?;
                let transport = HttpsEmulatorDownloadTransport::new();
                let options = EmulatorDownloadOptions {
                    cancellation: Some(cancellation),
                    progress: Some(reporter),
                    ..Default::default()
                };
                download_and_install_resolved(&root, &plan_for_thread, &transport, &options)
            })();
            let _ = sender.send(TaskResult::Installed(result));
            context.request_repaint();
        });
    }

    /// Render the managed-emulator download section. Returns at most one
    /// action; the caller passes it back to [`Self::handle`].
    pub(crate) fn show(&self, ui: &mut egui::Ui) -> Option<EmulatorDownloadPageAction> {
        let mut action = None;
        widgets::card(ui, |ui| {
            ui.heading("Download managed emulators");
            ui.label(
                "EmuWiz can download the official Linux AppImage for these emulators. Nothing is \
                 downloaded until you review the exact release and approve it. Installing an \
                 emulator does not by itself make a game ready to play - that is still decided by \
                 the checks above.",
            );
            ui.add_space(theme::SECTION_GAP);
            for spec in Self::managed_specs() {
                let state = self
                    .entries
                    .get(spec.id)
                    .cloned()
                    .unwrap_or(EmulatorDownloadEntryState::NotInstalled);
                ui.group(|ui| {
                    if let Some(row_action) = self.show_entry(ui, spec, &state) {
                        action = Some(row_action);
                    }
                });
                ui.add_space(8.0);
            }
            if Self::managed_specs().next().is_none() {
                widgets::empty_state(
                    ui,
                    "No managed emulators",
                    "No emulator in the catalogue has an automated download lane.",
                    None,
                );
            }
        });
        action
    }

    fn show_entry(
        &self,
        ui: &mut egui::Ui,
        spec: &EmulatorDownloadSpec,
        state: &EmulatorDownloadEntryState,
    ) -> Option<EmulatorDownloadPageAction> {
        let id = spec.id.to_string();
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(spec.display_name).strong());
            let busy = self.task.is_some();
            match state {
                EmulatorDownloadEntryState::Installed(_) => {
                    widgets::status_badge(ui, "Installed", widgets::StatusTone::Success);
                }
                EmulatorDownloadEntryState::NotInstalled => {
                    widgets::status_badge(ui, "Not installed", widgets::StatusTone::Pending);
                    if ui
                        .add_enabled(!busy, egui::Button::new("Download emulator"))
                        .clicked()
                    {
                        action = Some(EmulatorDownloadPageAction::Resolve(id.clone()));
                    }
                }
                EmulatorDownloadEntryState::ManualInstallRequired => {
                    widgets::status_badge(ui, "Manual install", widgets::StatusTone::Info);
                }
                EmulatorDownloadEntryState::Unsupported => {
                    widgets::status_badge(ui, "Unsupported", widgets::StatusTone::Blocked);
                }
                EmulatorDownloadEntryState::Checking => {
                    widgets::status_badge(ui, "Checking…", widgets::StatusTone::Active);
                }
                EmulatorDownloadEntryState::ReadyToDownload(_)
                | EmulatorDownloadEntryState::AwaitingConfirmation(_) => {
                    widgets::status_badge(ui, "Ready to install", widgets::StatusTone::Active);
                }
                EmulatorDownloadEntryState::Downloading(_) => {
                    widgets::status_badge(ui, "Downloading", widgets::StatusTone::Active);
                }
                EmulatorDownloadEntryState::Verifying(_) => {
                    widgets::status_badge(ui, "Verifying", widgets::StatusTone::Active);
                }
                EmulatorDownloadEntryState::Installing(_) => {
                    widgets::status_badge(ui, "Installing", widgets::StatusTone::Active);
                }
                EmulatorDownloadEntryState::Complete(_) => {
                    widgets::status_badge(ui, "Installed just now", widgets::StatusTone::Success);
                }
                EmulatorDownloadEntryState::Cancelled => {
                    widgets::status_badge(ui, "Cancelled", widgets::StatusTone::Warning);
                }
                EmulatorDownloadEntryState::Failed(_) => {
                    widgets::status_badge(ui, "Download failed", widgets::StatusTone::Blocked);
                }
            }
        });

        match state {
            EmulatorDownloadEntryState::Installed(path) => {
                ui.label(
                    "An EmuWiz-managed AppImage is installed. Whether a game will launch is still \
                     decided by the readiness checks above.",
                );
                widgets::technical_details(ui, ("emu-dl-installed", spec.id), |ui| {
                    widgets::detail_row(ui, "Path", &path.to_string_lossy());
                });
            }
            EmulatorDownloadEntryState::NotInstalled => {
                ui.label(format!(
                    "Not installed by EmuWiz. Download the official {} AppImage from {}.",
                    spec.official_project, spec.project_url
                ));
            }
            EmulatorDownloadEntryState::ManualInstallRequired => {
                ui.label(format!(
                    "{} does not have an automated download here. Install it yourself from {} - the \
                     existing setup steps for it still apply.",
                    spec.display_name, spec.project_url
                ));
            }
            EmulatorDownloadEntryState::Unsupported => {
                ui.label("This emulator cannot be downloaded automatically.");
            }
            EmulatorDownloadEntryState::Checking => {
                ui.label(
                    "Looking up the exact stable release and download. Nothing is downloaded yet.",
                );
            }
            EmulatorDownloadEntryState::ReadyToDownload(plan)
            | EmulatorDownloadEntryState::AwaitingConfirmation(plan) => {
                if let Some(row_action) = self.show_confirmation(ui, spec, plan, state) {
                    action = Some(row_action);
                }
            }
            EmulatorDownloadEntryState::Downloading(progress)
            | EmulatorDownloadEntryState::Verifying(progress)
            | EmulatorDownloadEntryState::Installing(progress) => {
                let phase_label = match state {
                    EmulatorDownloadEntryState::Downloading(_) => "Downloading the AppImage",
                    EmulatorDownloadEntryState::Verifying(_) => {
                        "Verifying the download (AppImage and checksum)"
                    }
                    _ => "Installing",
                };
                ui.label(phase_label);
                if let Some(total) = progress.total_bytes.filter(|total| *total > 0) {
                    let fraction = progress.bytes_received as f32 / total as f32;
                    ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
                } else if progress.bytes_received > 0 {
                    ui.label(format!(
                        "{} received",
                        widgets::format_size(Some(progress.bytes_received))
                    ));
                }
                if matches!(state, EmulatorDownloadEntryState::Downloading(_))
                    && ui.button("Cancel").clicked()
                {
                    action = Some(EmulatorDownloadPageAction::Cancel(id.clone()));
                }
            }
            EmulatorDownloadEntryState::Complete(receipt) => {
                ui.label(format!(
                    "Installed {} {}. This does not by itself mean a game is ready to play.",
                    spec.display_name, receipt.release_tag
                ));
                widgets::technical_details(ui, ("emu-dl-receipt", spec.id), |ui| {
                    widgets::detail_row(ui, "Release", &receipt.release_tag);
                    widgets::detail_row(ui, "Asset", &receipt.asset_name);
                    widgets::detail_row(ui, "SHA-256", &receipt.sha256);
                    widgets::detail_row(
                        ui,
                        "Upstream digest verified",
                        if receipt.digest_verified {
                            "yes"
                        } else {
                            "no upstream digest published"
                        },
                    );
                    widgets::detail_row(ui, "Path", &receipt.installed_path.to_string_lossy());
                });
                if ui.button("Dismiss").clicked() {
                    action = Some(EmulatorDownloadPageAction::Dismiss(id.clone()));
                }
            }
            EmulatorDownloadEntryState::Cancelled => {
                ui.label("The download was cancelled. Nothing was installed or changed.");
                if ui.button("Dismiss").clicked() {
                    action = Some(EmulatorDownloadPageAction::Dismiss(id.clone()));
                }
            }
            EmulatorDownloadEntryState::Failed(error) => {
                let (short, technical) = novice_error(error);
                ui.label(short);
                widgets::technical_details(ui, ("emu-dl-error", spec.id), |ui| {
                    ui.label(technical);
                });
                ui.horizontal(|ui| {
                    if self.task.is_none() && ui.button("Try again").clicked() {
                        action = Some(EmulatorDownloadPageAction::Resolve(id.clone()));
                    }
                    if ui.button("Dismiss").clicked() {
                        action = Some(EmulatorDownloadPageAction::Dismiss(id.clone()));
                    }
                });
            }
        }
        action
    }

    fn show_confirmation(
        &self,
        ui: &mut egui::Ui,
        spec: &EmulatorDownloadSpec,
        plan: &EmulatorDownloadPlan,
        state: &EmulatorDownloadEntryState,
    ) -> Option<EmulatorDownloadPageAction> {
        let id = spec.id.to_string();
        let mut action = None;
        ui.label("Review this exact download before it is fetched:");
        widgets::detail_row(ui, "Emulator", &plan.display_name);
        widgets::detail_row(
            ui,
            "Release",
            plan.release_name
                .as_deref()
                .map(|name| format!("{name} ({})", plan.release_tag))
                .as_deref()
                .unwrap_or(&plan.release_tag),
        );
        widgets::detail_row(ui, "File", &plan.asset_name);
        widgets::detail_row(ui, "Source project", &plan.official_project);
        widgets::detail_row(ui, "From", &plan.asset_url);
        widgets::detail_row(ui, "Install to", &plan.destination_path.to_string_lossy());
        widgets::detail_row(
            ui,
            "Checksum",
            plan.expected_sha256.as_deref().unwrap_or(
                "not published by the project (the file is still verified as an AppImage)",
            ),
        );

        let confirmed = matches!(state, EmulatorDownloadEntryState::AwaitingConfirmation(_));
        if plan.replaces_existing_managed {
            widgets::banner(
                ui,
                "This will replace the emulator EmuWiz installed before",
                "An EmuWiz-managed AppImage is already installed here. Installing replaces it. \
                 No previous version is kept.",
                widgets::StatusTone::Warning,
            );
            let mut checkbox = confirmed;
            if ui
                .checkbox(&mut checkbox, "Replace the existing EmuWiz-managed install")
                .changed()
                && checkbox
            {
                action = Some(EmulatorDownloadPageAction::ConfirmReplacement(id.clone()));
            }
        }

        let install_enabled = self.task.is_none() && (!plan.replaces_existing_managed || confirmed);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(install_enabled, egui::Button::new("Install emulator"))
                .clicked()
            {
                action = Some(EmulatorDownloadPageAction::Install(id.clone()));
            }
            if ui.button("Not now").clicked() {
                action = Some(EmulatorDownloadPageAction::Dismiss(id.clone()));
            }
        });
        action
    }
}

/// Resolve a possibly-arbitrary id string to a `'static` catalogue id, or
/// `""` if it is not in the catalogue (never inserted).
fn catalogue_id(id: &str) -> &'static str {
    emulator_download_spec(id).map(|spec| spec.id).unwrap_or("")
}

/// A short, plain-language message plus the raw technical detail for one
/// typed backend failure. The short line is shown first; the raw error goes
/// under "Technical details".
pub(crate) fn novice_error(error: &DownloadError) -> (String, String) {
    let short = match error {
        DownloadError::Cancelled => "The download was cancelled. Nothing was installed.",
        DownloadError::HttpStatus(_)
        | DownloadError::Io(_)
        | DownloadError::TruncatedTransfer { .. }
        | DownloadError::ContentLengthInvalid => {
            "The download did not finish. Check your internet connection and try again."
        }
        DownloadError::ReleaseNotFound => {
            "EmuWiz could not find a stable release to download safely."
        }
        DownloadError::AmbiguousRelease(_) | DownloadError::InvalidRelease(_) => {
            "The project's release list was not something EmuWiz could pick from safely."
        }
        DownloadError::InvalidAsset(_) => {
            "EmuWiz could not find exactly one Linux x86_64 AppImage in that release."
        }
        DownloadError::ChecksumMismatch { .. } => {
            "The downloaded file did not match the checksum the project published. It was not installed."
        }
        DownloadError::InvalidImage | DownloadError::TooSmall => {
            "The downloaded file was not a valid Linux AppImage. It was not installed."
        }
        DownloadError::TooLarge => {
            "The download was larger than EmuWiz's safety limit and was stopped."
        }
        DownloadError::InvalidDigest(_) => {
            "The checksum the project published was not in a form EmuWiz could verify."
        }
        DownloadError::RedirectRejected(_) | DownloadError::RedirectLimit => {
            "The download tried to send EmuWiz somewhere it does not trust. It was stopped."
        }
        DownloadError::Unsupported(_) => "This emulator cannot be downloaded automatically here.",
    };
    (short.to_string(), error.to_string())
}

#[cfg(test)]
impl EmulatorDownloadPageState {
    pub(crate) fn entry(&self, id: &str) -> Option<&EmulatorDownloadEntryState> {
        self.entries.get(catalogue_id(id))
    }

    pub(crate) fn test_refresh_from_root(&mut self, root: &std::path::Path) {
        self.refresh_from_root(Some(root));
    }

    fn test_ingest(&mut self, id: &str, result: TaskResult) {
        self.apply_task_result(id, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::emulator_download::{
        EmulatorDownloadPlan, EmulatorDownloadReceipt, ReleaseSelectionPolicy, install_appimage_at,
    };
    use std::path::PathBuf;

    fn plan(id: &str, replaces: bool, dest: PathBuf) -> EmulatorDownloadPlan {
        EmulatorDownloadPlan {
            emulator_id: id.to_string(),
            display_name: "PCSX2".into(),
            distribution: EmulatorDistribution::GithubAppImage,
            official_project: "PCSX2".into(),
            project_url: "https://github.com/PCSX2/pcsx2".into(),
            release_tag: "v1.2.3".into(),
            release_name: Some("Stable release".into()),
            asset_name: "pcsx2-x86_64.AppImage".into(),
            asset_url:
                "https://github.com/PCSX2/pcsx2/releases/download/v1.2.3/pcsx2-x86_64.AppImage"
                    .into(),
            expected_sha256: None,
            upstream_digest: None,
            destination_path: dest,
            selection_policy: ReleaseSelectionPolicy::default(),
            replaces_existing_managed: replaces,
        }
    }

    fn receipt(id: &str, dest: PathBuf) -> EmulatorDownloadReceipt {
        EmulatorDownloadReceipt {
            emulator_id: id.to_string(),
            release_tag: "v1.2.3".into(),
            asset_name: "pcsx2-x86_64.AppImage".into(),
            installed_path: dest,
            sha256: "0".repeat(64),
            upstream_digest: None,
            digest_verified: false,
        }
    }

    fn image() -> Vec<u8> {
        let mut bytes = vec![0u8; 1_048_576];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes
    }

    #[test]
    fn refresh_classifies_managed_vs_manual_vs_missing() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = EmulatorDownloadPageState::default();
        state.test_refresh_from_root(temp.path());
        // GithubAppImage lane, nothing installed.
        assert_eq!(
            state.entry("pcsx2"),
            Some(&EmulatorDownloadEntryState::NotInstalled)
        );
        // Manual lane emulators are never offered a managed download.
        assert_eq!(
            state.entry("dolphin"),
            Some(&EmulatorDownloadEntryState::ManualInstallRequired)
        );
        assert_eq!(
            state.entry("shadps4"),
            Some(&EmulatorDownloadEntryState::ManualInstallRequired)
        );

        // Lay down a real managed install and re-classify.
        let spec = emulator_download_spec("pcsx2").unwrap();
        let destination = install_appimage_at(temp.path(), spec, &image(), None).unwrap();
        state.test_refresh_from_root(temp.path());
        assert_eq!(
            state.entry("pcsx2"),
            Some(&EmulatorDownloadEntryState::Installed(destination))
        );
    }

    #[test]
    fn manual_lane_emulator_never_exposes_a_managed_download_row() {
        // The section only iterates GithubAppImage-lane specs.
        let ids: Vec<_> = EmulatorDownloadPageState::managed_specs()
            .map(|spec| spec.id)
            .collect();
        assert!(ids.contains(&"pcsx2"));
        assert!(!ids.contains(&"dolphin"));
        assert!(!ids.contains(&"scummvm"));
        assert!(!ids.contains(&"shadps4"));
        assert!(!ids.contains(&"retroarch"));
    }

    #[test]
    fn a_resolved_plan_moves_to_ready_to_download_not_installing() {
        let mut state = EmulatorDownloadPageState::default();
        let dest = PathBuf::from("/data/emulators/pcsx2/pcsx2.AppImage");
        state.test_ingest(
            "pcsx2",
            TaskResult::Resolved(Ok(plan("pcsx2", false, dest.clone()))),
        );
        match state.entry("pcsx2") {
            Some(EmulatorDownloadEntryState::ReadyToDownload(resolved)) => {
                assert_eq!(resolved.release_tag, "v1.2.3");
                assert_eq!(resolved.destination_path, dest);
            }
            other => panic!("expected ReadyToDownload, got {other:?}"),
        }
        // No install started - take_completed_install is still None.
        assert!(state.take_completed_install().is_none());
    }

    #[test]
    fn install_is_not_started_without_confirmation_for_a_replacement_plan() {
        let ctx = egui::Context::default();
        let mut state = EmulatorDownloadPageState::default();
        let dest = PathBuf::from("/data/emulators/pcsx2/pcsx2.AppImage");
        state.test_ingest(
            "pcsx2",
            TaskResult::Resolved(Ok(plan("pcsx2", true, dest.clone()))),
        );
        // Install click on a replacement plan that has NOT been confirmed:
        // nothing happens (no task, still ReadyToDownload).
        state.handle(
            EmulatorDownloadPageAction::Install("pcsx2".into()),
            ctx.clone(),
        );
        assert!(state.task.is_none());
        assert!(matches!(
            state.entry("pcsx2"),
            Some(EmulatorDownloadEntryState::ReadyToDownload(_))
        ));

        // After explicit confirmation it becomes AwaitingConfirmation.
        state.handle(
            EmulatorDownloadPageAction::ConfirmReplacement("pcsx2".into()),
            ctx.clone(),
        );
        assert!(matches!(
            state.entry("pcsx2"),
            Some(EmulatorDownloadEntryState::AwaitingConfirmation(_))
        ));
    }

    #[test]
    fn a_successful_install_flags_a_readiness_refresh_exactly_once() {
        let mut state = EmulatorDownloadPageState::default();
        let dest = PathBuf::from("/data/emulators/pcsx2/pcsx2.AppImage");
        state.test_ingest(
            "pcsx2",
            TaskResult::Installed(Ok(receipt("pcsx2", dest.clone()))),
        );
        assert!(matches!(
            state.entry("pcsx2"),
            Some(EmulatorDownloadEntryState::Complete(_))
        ));
        assert_eq!(state.take_completed_install().as_deref(), Some("pcsx2"));
        // Consumed - not raised again.
        assert!(state.take_completed_install().is_none());
    }

    #[test]
    fn a_failed_install_keeps_the_typed_error_for_technical_details() {
        let mut state = EmulatorDownloadPageState::default();
        state.test_ingest(
            "pcsx2",
            TaskResult::Installed(Err(DownloadError::ChecksumMismatch {
                expected: "aa".into(),
                actual: "bb".into(),
            })),
        );
        match state.entry("pcsx2") {
            Some(EmulatorDownloadEntryState::Failed(error)) => {
                let (short, technical) = novice_error(error);
                assert!(short.to_lowercase().contains("checksum"));
                assert!(technical.contains("aa") && technical.contains("bb"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        // A cancelled install is its own state, never Failed.
        state.test_ingest(
            "pcsx2",
            TaskResult::Installed(Err(DownloadError::Cancelled)),
        );
        assert!(matches!(
            state.entry("pcsx2"),
            Some(EmulatorDownloadEntryState::Cancelled)
        ));
    }

    #[test]
    fn every_typed_download_error_has_a_distinct_plain_language_lead() {
        let errors = [
            DownloadError::Cancelled,
            DownloadError::HttpStatus(503),
            DownloadError::ReleaseNotFound,
            DownloadError::AmbiguousRelease("x".into()),
            DownloadError::InvalidAsset("x".into()),
            DownloadError::ChecksumMismatch {
                expected: "a".into(),
                actual: "b".into(),
            },
            DownloadError::InvalidImage,
            DownloadError::TooLarge,
            DownloadError::RedirectRejected("x".into()),
            DownloadError::Unsupported("x".into()),
        ];
        for error in errors {
            let (short, technical) = novice_error(&error);
            assert!(!short.is_empty());
            assert!(!technical.is_empty());
            // The plain lead never simply echoes the raw Display string.
            assert_ne!(short, technical);
        }
    }

    #[test]
    fn dismiss_returns_a_terminal_entry_to_discovery_state() {
        let ctx = egui::Context::default();
        let temp = tempfile::tempdir().unwrap();
        let mut state = EmulatorDownloadPageState::default();
        state.test_refresh_from_root(temp.path());
        state.test_ingest("pcsx2", TaskResult::Installed(Err(DownloadError::TooLarge)));
        assert!(matches!(
            state.entry("pcsx2"),
            Some(EmulatorDownloadEntryState::Failed(_))
        ));
        state.handle(EmulatorDownloadPageAction::Dismiss("pcsx2".into()), ctx);
        // refresh() inside Dismiss uses the real data dir; the entry is at
        // least no longer a Failed card.
        assert!(!matches!(
            state.entry("pcsx2"),
            Some(EmulatorDownloadEntryState::Failed(_))
        ));
    }
}
