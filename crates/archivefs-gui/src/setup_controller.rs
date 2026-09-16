use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::{
    ArchiveFsError, ConfigIdentity, SetupDiagnosticStatus, SetupDiagnostics,
    create_configured_mount_root_default, create_starter_config_default, default_config_path,
    run_setup_diagnostics_default, set_mount_root_default,
};
use eframe::egui;

use super::{
    ActionFeedback, ActivityAction, ActivityOutcome, ArchiveFsApp, HistoryEntry, LoadState,
    RefreshGeneration, ToolsOverlay, open_folder_in_file_manager, sources_page,
};

pub(crate) type DiagnosticsMessage = (RefreshGeneration, SetupDiagnostics);

pub(crate) enum DiagnosticsState {
    Loading {
        generation: RefreshGeneration,
        receiver: Receiver<DiagnosticsMessage>,
    },
    Ready {
        generation: RefreshGeneration,
        report: SetupDiagnostics,
    },
    Error {
        generation: RefreshGeneration,
        message: String,
    },
}

impl DiagnosticsState {
    pub(crate) fn generation(&self) -> RefreshGeneration {
        match self {
            Self::Loading { generation, .. }
            | Self::Ready { generation, .. }
            | Self::Error { generation, .. } => *generation,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SetupAction {
    CreateStarterConfig,
    CreateMountRoot,
    OpenConfigFolder,
    SetMountRoot(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiagnosticsUiAction {
    Refresh,
    Continue,
    ViewLastSnapshot,
    CreateStarterConfig,
    CreateMountRoot,
    OpenConfigFolder,
    CopyConfigPath,
}

pub(crate) struct RunningSetupAction {
    pub(crate) action: SetupAction,
    pub(crate) receiver: Receiver<Result<String, String>>,
}

pub(crate) fn diagnostics_can_continue(report: &SetupDiagnostics) -> bool {
    report.ready_for_scanning
}

pub(crate) fn starter_config_available(report: &SetupDiagnostics) -> bool {
    report.config_path.is_some() && report.config_missing && report.config_path_error.is_none()
}

pub(crate) fn diagnostics_state_can_continue(state: &DiagnosticsState) -> bool {
    matches!(state, DiagnosticsState::Ready { report, .. } if diagnostics_can_continue(report))
}

/// Archive actions are only safe when the snapshot and diagnostics both
/// belong to the current refresh generation *and* were derived from the
/// exact same configuration contents. Matching generations alone is not
/// enough: the config file can change between the snapshot read and the
/// diagnostics read of the same generation, so identities are compared too.
pub(crate) fn latest_generation_actions_safe(
    current: RefreshGeneration,
    snapshot_generation: Option<RefreshGeneration>,
    snapshot_stale: bool,
    snapshot_identity: Option<&ConfigIdentity>,
    diagnostics: &DiagnosticsState,
) -> bool {
    if snapshot_generation != Some(current) || snapshot_stale || diagnostics.generation() != current
    {
        return false;
    }
    let DiagnosticsState::Ready { report, .. } = diagnostics else {
        return false;
    };
    report.ready_for_actions && snapshot_identity == Some(&report.config_identity)
}

/// A concise, human-readable explanation for why archive actions (Mount,
/// Unmount, Lazy Unmount, Remount) are currently blocked for a selected
/// *live* archive. Mirrors `latest_generation_actions_safe`'s checks in the
/// same order rather than re-deriving its own notion of "safe", so the
/// label can never disagree with the actual gate: `Some(_)` here if and
/// only if `busy || !latest_generation_actions_safe(..)` is true for the
/// same inputs (see `archive_action_block_reason_matches_the_safety_gate`).
/// Returns `None` when actions are available.
pub(crate) fn archive_action_block_reason(
    busy: bool,
    current: RefreshGeneration,
    snapshot_generation: Option<RefreshGeneration>,
    snapshot_stale: bool,
    snapshot_identity: Option<&ConfigIdentity>,
    diagnostics: &DiagnosticsState,
) -> Option<&'static str> {
    if busy {
        return Some("Another operation is running.");
    }
    if snapshot_generation != Some(current) || snapshot_stale {
        return Some("Selection is stale. Refresh to continue.");
    }
    if diagnostics.generation() != current {
        return Some("Waiting for diagnostics.");
    }
    let DiagnosticsState::Ready { report, .. } = diagnostics else {
        return Some("Waiting for diagnostics.");
    };
    if !report.ready_for_actions {
        let mount_root_failed = report.checks.iter().any(|check| {
            (check.name == "Mount root exists or can be created safely"
                || check.name == "Mount root is writable")
                && check.status != SetupDiagnosticStatus::Ready
        });
        return Some(if mount_root_failed {
            "Mount root is unavailable."
        } else {
            "Setup needs attention."
        });
    }
    if snapshot_identity != Some(&report.config_identity) {
        return Some("Selection is stale. Refresh to continue.");
    }
    None
}

pub(crate) fn snapshot_identity(state: &LoadState) -> Option<&ConfigIdentity> {
    match state {
        LoadState::Ready(data) => Some(&data.config_identity),
        LoadState::Loading { .. } | LoadState::Error(_) => None,
    }
}

/// A full, line-by-line breakdown of every boolean
/// `latest_generation_actions_safe`/`archive_action_block_reason` reads,
/// plus the raw `SetupDiagnostics.checks` list underneath `ready_for_actions`.
/// Added specifically because the Library page's "Doctor: Ready" summary
/// (from the *separate* `ArchiveSnapshot.doctor`/`DoctorReport`, computed by
/// a different code path than `SetupDiagnostics`) can legitimately disagree
/// with this gate, and previously gave the user no way to see why. Rendered
/// in an always-present, collapsed-by-default section next to the Mount/
/// Unmount button (see `show_selected_archive`); its mere presence in a
/// running build also confirms the deployed binary actually contains this
/// code, not just the terse one-line `archive_action_block_reason` text.
pub(crate) fn action_readiness_debug_lines(
    is_operation_busy: bool,
    current: RefreshGeneration,
    snapshot_generation: Option<RefreshGeneration>,
    snapshot_stale: bool,
    snapshot_identity: Option<&ConfigIdentity>,
    diagnostics: &DiagnosticsState,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "busy (an operation is running): {is_operation_busy}"
    ));
    lines.push(format!("current refresh generation: {}", current.0));
    lines.push(format!(
        "snapshot generation: {}",
        snapshot_generation
            .map_or_else(|| "none".to_string(), |generation| generation.0.to_string())
    ));
    lines.push(format!(
        "snapshot generation matches current: {}",
        snapshot_generation == Some(current)
    ));
    lines.push(format!("snapshot marked stale: {snapshot_stale}"));
    lines.push(format!(
        "diagnostics generation: {}",
        diagnostics.generation().0
    ));
    lines.push(format!(
        "diagnostics generation matches current: {}",
        diagnostics.generation() == current
    ));
    match diagnostics {
        DiagnosticsState::Loading { .. } => {
            lines.push("diagnostics state: Loading".to_string());
        }
        DiagnosticsState::Error { message, .. } => {
            lines.push(format!("diagnostics state: Error ({message})"));
        }
        DiagnosticsState::Ready { report, .. } => {
            lines.push("diagnostics state: Ready".to_string());
            lines.push(format!("ready_for_scanning: {}", report.ready_for_scanning));
            lines.push(format!("ready_for_actions: {}", report.ready_for_actions));
            lines.push(format!(
                "snapshot config identity matches diagnostics config identity: {}",
                snapshot_identity == Some(&report.config_identity)
            ));
            lines.push("setup checks:".to_string());
            for check in &report.checks {
                lines.push(format!(
                    "  [{:?}] {}: {}",
                    check.status, check.name, check.detail
                ));
            }
        }
    }
    lines
}

pub(crate) fn start_diagnostics(
    context: egui::Context,
    generation: RefreshGeneration,
) -> DiagnosticsState {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send((generation, run_setup_diagnostics_default()));
        context.request_repaint();
    });
    DiagnosticsState::Loading {
        generation,
        receiver,
    }
}

pub(crate) fn start_setup_action_worker(
    context: egui::Context,
    action: SetupAction,
) -> RunningSetupAction {
    let (sender, receiver) = mpsc::channel();
    let worker_action = action.clone();
    thread::spawn(move || {
        let result = match worker_action {
            SetupAction::CreateStarterConfig => create_starter_config_default()
                .map(|path| format!("Created starter config at {}.", path.display())),
            SetupAction::CreateMountRoot => create_configured_mount_root_default()
                .map(|path| format!("Created mount root at {}.", path.display())),
            SetupAction::OpenConfigFolder => open_default_config_folder(),
            SetupAction::SetMountRoot(path) => set_mount_root_default(&path).map(|path| {
                format!(
                    "Temporary game preparation folder updated to {}.",
                    path.display()
                )
            }),
        }
        .map_err(|error| error.to_string());
        let _ = sender.send(result);
        context.request_repaint();
    });
    RunningSetupAction { action, receiver }
}

fn open_default_config_folder() -> archivefs_core::Result<String> {
    let config_path = default_config_path()?;
    let folder = config_path.parent().ok_or_else(|| {
        ArchiveFsError::Config(format!(
            "config path has no parent folder: {}",
            config_path.display()
        ))
    })?;
    open_folder_in_file_manager(folder)?;
    Ok(format!("Opened config folder {}.", folder.display()))
}

impl ArchiveFsApp {
    pub(crate) fn refresh_diagnostics(&mut self, context: &egui::Context) {
        self.health_duplicate_ui.diagnostics_refresh_generation = self
            .health_duplicate_ui
            .diagnostics_refresh_generation
            .next();
        self.history.record(HistoryEntry::new(
            ActivityAction::Diagnostics,
            None,
            ActivityOutcome::Started,
            "Refreshing setup diagnostics.",
        ));
        self.doctor_repair.diagnostics =
            start_diagnostics(context.clone(), self.refresh_generation);
    }

    pub(crate) fn poll_diagnostics(&mut self) {
        enum PollResult {
            Completed(DiagnosticsMessage),
            Disconnected(RefreshGeneration),
        }

        let result = match &self.doctor_repair.diagnostics {
            DiagnosticsState::Loading {
                generation,
                receiver,
            } => match receiver.try_recv() {
                Ok(message) => Some(PollResult::Completed(message)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(PollResult::Disconnected(*generation)),
            },
            DiagnosticsState::Ready { .. } | DiagnosticsState::Error { .. } => None,
        };
        match result {
            Some(PollResult::Completed((generation, report)))
                if generation == self.refresh_generation =>
            {
                self.history.record(HistoryEntry::new(
                    ActivityAction::Diagnostics,
                    None,
                    ActivityOutcome::Completed,
                    if report.ready_for_actions {
                        "Diagnostics completed: EmuWiz is ready."
                    } else {
                        "Diagnostics completed: setup needs attention."
                    },
                ));
                if !report.config_missing && report.config_path_error.is_none() {
                    self.doctor_repair.config_previously_confirmed = true;
                }
                self.doctor_repair.diagnostics = DiagnosticsState::Ready { generation, report };
                self.maybe_auto_open_onboarding();
            }
            Some(PollResult::Disconnected(generation)) if generation == self.refresh_generation => {
                let message = "The diagnostics worker stopped unexpectedly. Run diagnostics again."
                    .to_string();
                self.history.record(HistoryEntry::new(
                    ActivityAction::Diagnostics,
                    None,
                    ActivityOutcome::Failed,
                    message.clone(),
                ));
                self.doctor_repair.diagnostics = DiagnosticsState::Error {
                    generation,
                    message,
                };
                self.tools_overlay = ToolsOverlay::Diagnostics;
            }
            Some(PollResult::Completed(_)) | Some(PollResult::Disconnected(_)) | None => {}
        }
    }

    pub(crate) fn start_setup_action(&mut self, context: egui::Context, action: SetupAction) {
        if self.is_busy()
            || (matches!(&action, SetupAction::SetMountRoot(_))
                && self.sources_ui.source_action.is_some())
        {
            return;
        }
        if matches!(&action, SetupAction::SetMountRoot(_)) {
            // The previous outcome is no longer current the moment a new
            // apply begins.
            self.sources_ui.mount_root_feedback = None;
        }
        let started_message = match &action {
            SetupAction::CreateStarterConfig => "Creating starter config.",
            SetupAction::CreateMountRoot => "Creating configured mount root.",
            SetupAction::OpenConfigFolder => "Opening config folder.",
            SetupAction::SetMountRoot(_) => "Updating temporary preparation folder.",
        };
        self.history.record(HistoryEntry::new(
            ActivityAction::Setup,
            None,
            ActivityOutcome::Started,
            started_message,
        ));
        self.setup_action = Some(start_setup_action_worker(context, action));
    }

    pub(crate) fn poll_setup_action(&mut self, context: &egui::Context) {
        let result = self.setup_action.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.action.clone(), result))
        });
        if let Some((action, result)) = result {
            self.setup_action = None;
            match result {
                Ok(message) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Setup,
                        None,
                        ActivityOutcome::Completed,
                        message.clone(),
                    ));
                    let reload_warning = if matches!(&action, SetupAction::SetMountRoot(_)) {
                        self.gui_config.reload_default().err().map(|error| {
                            format!(
                                "The folder changed successfully, but EmuWiz could not reload its configuration: {error}."
                            )
                        })
                    } else {
                        None
                    };
                    self.feedback = Some(ActionFeedback {
                        succeeded: true,
                        message: message.clone(),
                        cleanup: None,
                        warning: reload_warning.clone(),
                        more_information: None,
                    });
                    if matches!(&action, SetupAction::SetMountRoot(_)) {
                        self.sources_ui.mount_root_feedback =
                            Some(sources_page::MountRootFeedback {
                                succeeded: true,
                                summary: message,
                                detail: None,
                                warning: reload_warning,
                            });
                        self.sources_ui.mount_root_draft = None;
                        self.refresh(context);
                    } else if action != SetupAction::OpenConfigFolder {
                        self.refresh_diagnostics(context);
                    }
                }
                Err(message) => {
                    let (feedback_message, more_information) = if matches!(
                        &action,
                        SetupAction::SetMountRoot(_)
                    ) {
                        (
                                "Could not change the temporary game preparation folder. Choose an existing writable folder and try again."
                                    .to_string(),
                                Some(message.clone()),
                            )
                    } else {
                        (message.clone(), None)
                    };
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Setup,
                        None,
                        ActivityOutcome::Failed,
                        message.clone(),
                    ));
                    if matches!(&action, SetupAction::SetMountRoot(_)) {
                        self.sources_ui.mount_root_feedback =
                            Some(sources_page::MountRootFeedback {
                                succeeded: false,
                                summary: feedback_message.clone(),
                                detail: Some(message.clone()),
                                warning: None,
                            });
                    }
                    self.feedback = Some(ActionFeedback {
                        succeeded: false,
                        message: feedback_message,
                        cleanup: None,
                        warning: None,
                        more_information,
                    });
                }
            }
        }
    }
}
