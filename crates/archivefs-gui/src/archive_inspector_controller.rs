use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::*;

/// The Archive Inspector's column/sort choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InspectorSortField {
    #[default]
    Path,
    Size,
    Classification,
}

impl std::fmt::Display for InspectorSortField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Path => "Path",
            Self::Size => "Size",
            Self::Classification => "Classification",
        })
    }
}

pub(crate) type InspectorMessage = (RefreshGeneration, Result<InspectorReport, String>);

/// What the Archive Inspector overlay is currently showing for its archive.
pub(crate) enum ArchiveInspectorStatus {
    Loading {
        generation: RefreshGeneration,
        receiver: Receiver<InspectorMessage>,
    },
    Ready(InspectorReport),
    Error(String),
}

/// Complete state for the Archive Inspector overlay for one archive.
pub(crate) struct ArchiveInspectorState {
    pub(crate) archive_path: PathBuf,
    pub(crate) status: ArchiveInspectorStatus,
    pub(crate) search: String,
    pub(crate) classification_filter: Option<InspectorEntryClassification>,
    pub(crate) sort_field: InspectorSortField,
    pub(crate) sort_ascending: bool,
    pub(crate) selected_entry: Option<String>,
    pub(crate) path_column_width: f32,
}

pub(crate) type ArchivePreparationMessage = (
    RefreshGeneration,
    Result<archivefs_core::ArchiveMemberResolution, String>,
);

/// Session-only launch preparation. The archive itself is never rewritten or
/// extracted; this retains the exact member selected from the existing mount.
#[derive(Default)]
pub(crate) enum ArchivePreparationState {
    #[default]
    Idle,
    Inspecting {
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        generation: RefreshGeneration,
        receiver: Receiver<ArchivePreparationMessage>,
    },
    Choosing {
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        candidates: Vec<archivefs_core::PreparedMemberCandidate>,
    },
    PendingMount {
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        candidate: archivefs_core::PreparedMemberCandidate,
    },
    Ready {
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        mount_path: PathBuf,
        candidate: archivefs_core::PreparedMemberCandidate,
    },
    Failed {
        archive_path: PathBuf,
        message: String,
    },
}

impl ArchiveInspectorState {
    pub(crate) fn loading(
        archive_path: PathBuf,
        generation: RefreshGeneration,
        receiver: Receiver<InspectorMessage>,
    ) -> Self {
        Self {
            archive_path,
            status: ArchiveInspectorStatus::Loading {
                generation,
                receiver,
            },
            search: String::new(),
            classification_filter: None,
            sort_field: InspectorSortField::default(),
            sort_ascending: true,
            selected_entry: None,
            path_column_width: DEFAULT_INSPECTOR_PATH_COLUMN_WIDTH,
        }
    }
}

pub(crate) const DEFAULT_INSPECTOR_PATH_COLUMN_WIDTH: f32 = 520.0;

impl ArchiveFsApp {
    pub(crate) fn start_archive_inspection(
        &mut self,
        context: egui::Context,
        archive_path: PathBuf,
    ) {
        self.archive_inspector_generation = self.archive_inspector_generation.next();
        let generation = self.archive_inspector_generation;
        let (sender, receiver) = mpsc::channel();
        let job_path = archive_path.clone();
        std::thread::spawn(move || {
            let result = inspect_archive(&job_path).map_err(|error| error.to_string());
            let _ = sender.send((generation, result));
            context.request_repaint();
        });
        self.archive_inspector = Some(ArchiveInspectorState::loading(
            archive_path,
            generation,
            receiver,
        ));
        self.tools_overlay = ToolsOverlay::ArchiveInspector;
    }

    /// Mirrors `poll_load`'s receiver-then-apply shape so the borrow checker
    /// never sees a conflict between reading and updating inspector state.
    pub(crate) fn poll_archive_inspection(&mut self) {
        let result = match self
            .archive_inspector
            .as_ref()
            .map(|inspector| &inspector.status)
        {
            Some(ArchiveInspectorStatus::Loading {
                generation,
                receiver,
            }) => match receiver.try_recv() {
                Ok(message) => Some(message),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    *generation,
                    Err("The inspection worker stopped unexpectedly.".to_string()),
                )),
            },
            Some(ArchiveInspectorStatus::Ready(_) | ArchiveInspectorStatus::Error(_)) | None => {
                None
            }
        };

        let Some((generation, result)) = result else {
            return;
        };
        if generation != self.archive_inspector_generation {
            return;
        }
        if let Some(inspector) = self.archive_inspector.as_mut() {
            inspector.status = match result {
                Ok(report) => ArchiveInspectorStatus::Ready(report),
                Err(message) => ArchiveInspectorStatus::Error(message),
            };
        }
    }

    pub(crate) fn current_archive_record(&self, archive_path: &Path) -> Option<ArchiveRecord> {
        match &self.state {
            LoadState::Ready(data) => data
                .records
                .iter()
                .find(|record| record.mount_plan.archive.path == archive_path)
                .cloned(),
            LoadState::Loading { previous, .. } => previous.as_ref().and_then(|data| {
                data.records
                    .iter()
                    .find(|record| record.mount_plan.archive.path == archive_path)
                    .cloned()
            }),
            LoadState::Error(_) => None,
        }
    }

    pub(crate) fn start_archive_preparation(
        &mut self,
        context: egui::Context,
        archive_path: PathBuf,
    ) {
        let Some(record) = self.current_archive_record(&archive_path) else {
            self.archive_preparation = ArchivePreparationState::Failed {
                archive_path,
                message: "This game isn't in your library any more. Refresh and try again."
                    .to_string(),
            };
            return;
        };
        let platform = record
            .metadata
            .platform
            .clone()
            .or_else(|| record.identity.platform.clone());
        self.archive_preparation_generation = self.archive_preparation_generation.next();
        let generation = self.archive_preparation_generation;
        let (sender, receiver) = mpsc::channel();
        let job_path = archive_path.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                archivefs_core::inspect_archive(&job_path)
                    .map(|report| {
                        archivefs_core::resolve_prepared_members(&report, platform.as_deref())
                    })
                    .map_err(|error| error.to_string())
            }))
            .unwrap_or_else(|_| {
                Err("archive preparation stopped while inspecting this archive".to_string())
            });
            let _ = sender.send((generation, result));
            context.request_repaint();
        });
        self.archive_preparation = ArchivePreparationState::Inspecting {
            archive_path,
            identity: record.identity,
            generation,
            receiver,
        };
    }

    pub(crate) fn poll_archive_preparation(&mut self, context: &egui::Context) {
        let result = match &self.archive_preparation {
            ArchivePreparationState::Inspecting {
                generation,
                receiver,
                ..
            } => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    *generation,
                    Err("archive preparation worker stopped unexpectedly".to_string()),
                )),
            },
            _ => None,
        };
        let Some((generation, result)) = result else {
            return;
        };
        if generation != self.archive_preparation_generation {
            return;
        }
        let ArchivePreparationState::Inspecting { archive_path, .. } = &self.archive_preparation
        else {
            return;
        };
        let archive_path = archive_path.clone();
        match result {
            Ok(archivefs_core::ArchiveMemberResolution::One(candidate)) => {
                self.begin_archive_member_preparation(context, archive_path, candidate);
            }
            Ok(archivefs_core::ArchiveMemberResolution::Multiple(candidates)) => {
                let identity = self
                    .current_archive_record(&archive_path)
                    .map(|record| record.identity);
                if let Some(identity) = identity {
                    self.archive_preparation = ArchivePreparationState::Choosing {
                        archive_path,
                        identity,
                        candidates,
                    };
                } else {
                    self.archive_preparation = ArchivePreparationState::Failed {
                        archive_path,
                        message: "The selected game changed while its archive was being inspected. Refresh and try again.".to_string(),
                    };
                }
            }
            Ok(archivefs_core::ArchiveMemberResolution::None(message)) | Err(message) => {
                self.archive_preparation = ArchivePreparationState::Failed {
                    archive_path,
                    message,
                };
            }
        }
    }

    fn begin_archive_member_preparation(
        &mut self,
        context: &egui::Context,
        archive_path: PathBuf,
        candidate: archivefs_core::PreparedMemberCandidate,
    ) {
        let Some(record) = self.current_archive_record(&archive_path) else {
            self.archive_preparation = ArchivePreparationState::Failed {
                archive_path,
                message: "This game isn't in your library any more. Refresh and try again."
                    .to_string(),
            };
            return;
        };
        let identity = record.identity.clone();
        if record.mount_state == MountState::Mounted {
            match archivefs_core::prepared_member_path(
                &record.mount_plan.mount_path,
                &candidate.member_name,
            ) {
                Ok(_member_path) => {
                    self.archive_preparation = ArchivePreparationState::Ready {
                        archive_path,
                        identity,
                        mount_path: record.mount_plan.mount_path,
                        candidate,
                    };
                }
                Err(message) => {
                    self.archive_preparation = ArchivePreparationState::Failed {
                        archive_path,
                        message,
                    };
                }
            }
            return;
        }
        self.archive_preparation = ArchivePreparationState::PendingMount {
            archive_path: archive_path.clone(),
            identity,
            candidate,
        };
        let started = self.start_operation(
            context.clone(),
            ArchiveAction::Mount,
            archive_path.clone(),
            false,
        );
        if !started {
            self.archive_preparation = ArchivePreparationState::Failed {
                archive_path,
                message: "Another archive operation is already running. Try Prepare game again when it finishes.".to_string(),
            };
        }
    }

    pub(crate) fn select_archive_member(
        &mut self,
        context: &egui::Context,
        archive_path: PathBuf,
        member_name: String,
    ) {
        let candidate = match &self.archive_preparation {
            ArchivePreparationState::Choosing {
                archive_path: state_path,
                candidates,
                ..
            } if *state_path == archive_path => candidates
                .iter()
                .find(|candidate| candidate.member_name == member_name)
                .cloned(),
            _ => None,
        };
        if let Some(candidate) = candidate {
            self.begin_archive_member_preparation(context, archive_path, candidate);
        }
    }

    pub(crate) fn reconcile_archive_preparation(&mut self) {
        let state_path = match &self.archive_preparation {
            ArchivePreparationState::Idle => None,
            ArchivePreparationState::Inspecting { archive_path, .. }
            | ArchivePreparationState::Choosing { archive_path, .. }
            | ArchivePreparationState::PendingMount { archive_path, .. }
            | ArchivePreparationState::Ready { archive_path, .. }
            | ArchivePreparationState::Failed { archive_path, .. } => Some(archive_path),
        };
        if state_path != self.archive_context.focused.as_ref() {
            if state_path.is_some() {
                self.archive_preparation_generation = self.archive_preparation_generation.next();
                self.archive_preparation = ArchivePreparationState::Idle;
            }
            return;
        }
        let Some(path) = state_path.cloned() else {
            return;
        };
        let Some(record) = self.current_archive_record(&path) else {
            self.archive_preparation = ArchivePreparationState::Idle;
            return;
        };
        let state = std::mem::take(&mut self.archive_preparation);
        self.archive_preparation = match state {
            ArchivePreparationState::PendingMount {
                archive_path,
                identity,
                candidate,
            } if record.mount_state == MountState::Mounted => {
                self.finalize_archive_member(archive_path, identity, candidate, &record)
            }
            ArchivePreparationState::Choosing { identity, .. } if identity != record.identity => {
                ArchivePreparationState::Idle
            }
            ArchivePreparationState::Inspecting { identity, .. } if identity != record.identity => {
                ArchivePreparationState::Idle
            }
            ArchivePreparationState::Ready {
                archive_path: _archive_path,
                identity,
                mount_path,
                candidate,
            } if identity != record.identity
                || record.mount_state != MountState::Mounted
                || mount_path != record.mount_plan.mount_path
                || archivefs_core::prepared_member_path(&mount_path, &candidate.member_name)
                    .is_err() =>
            {
                ArchivePreparationState::Idle
            }
            other => other,
        };
    }

    fn finalize_archive_member(
        &self,
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        candidate: archivefs_core::PreparedMemberCandidate,
        record: &ArchiveRecord,
    ) -> ArchivePreparationState {
        match archivefs_core::prepared_member_path(
            &record.mount_plan.mount_path,
            &candidate.member_name,
        ) {
            Ok(_) => ArchivePreparationState::Ready {
                archive_path,
                identity,
                mount_path: record.mount_plan.mount_path.clone(),
                candidate,
            },
            Err(message) => ArchivePreparationState::Failed {
                archive_path,
                message,
            },
        }
    }

    pub(crate) fn archive_preparation_view(
        &self,
        archive_path: &Path,
    ) -> (
        bool,
        Option<Vec<archivefs_core::PreparedMemberCandidate>>,
        Option<String>,
    ) {
        match &self.archive_preparation {
            ArchivePreparationState::Ready {
                archive_path: path, ..
            } if path == archive_path => (true, None, None),
            ArchivePreparationState::Choosing {
                archive_path: path,
                candidates,
                ..
            } if path == archive_path => (false, Some(candidates.clone()), None),
            ArchivePreparationState::Inspecting {
                archive_path: path, ..
            } if path == archive_path => (
                false,
                None,
                Some("Inspecting this archive safely…".to_string()),
            ),
            ArchivePreparationState::PendingMount {
                archive_path: path, ..
            } if path == archive_path => (false, None, Some("Preparing this game…".to_string())),
            ArchivePreparationState::Failed {
                archive_path: path,
                message,
            } if path == archive_path => (false, None, Some(message.clone())),
            _ => (false, None, None),
        }
    }

    pub(crate) fn resolved_archive_member_path(&self, record: &ArchiveRecord) -> Option<PathBuf> {
        let ArchivePreparationState::Ready {
            archive_path,
            identity,
            mount_path,
            candidate,
        } = &self.archive_preparation
        else {
            return None;
        };
        if archive_path != &record.mount_plan.archive.path
            || identity != &record.identity
            || mount_path != &record.mount_plan.mount_path
            || record.mount_state != MountState::Mounted
        {
            return None;
        }
        archivefs_core::prepared_member_path(mount_path, &candidate.member_name).ok()
    }
}
