//! Dedicated source-action boundary for the Sources workflow.
//!
//! The existing source worker protocol and `ArchiveFsApp` start/poll methods
//! live in `platform_source_actions` because that module also owns the
//! platform-assignment and alias writers. This facade keeps the source
//! vocabulary available as one focused seam and owns the Sources-specific
//! dialog state. A future `SetSourceRole` action belongs here alongside the
//! existing source actions; it must continue to return typed effects to the
//! app-wide history/refresh coordination rather than owning `ArchiveFsApp`.

use std::path::PathBuf;

#[allow(unused_imports)]
pub(crate) use crate::platform_source_actions::{
    RunningSourceAction, SourceAction, SourceActionOutcome, SourcesLastScan, SourcesScanScope,
    gamer_first_scan_after_add, run_source_action, source_action_log_category, source_action_path,
    source_action_started_message, source_action_success_message,
};

/// The Sources page's "Add Folder" dialog state.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourcesAddDialogState {
    pub(crate) path_text: String,
    pub(crate) validation_message: Option<String>,
}

/// The Sources page's remove confirmation state. The path is re-resolved by
/// the existing source action at commit time; the copied count is display
/// context only.
#[derive(Clone, Debug)]
pub(crate) struct SourcesRemoveDialogState {
    pub(crate) path: PathBuf,
    pub(crate) last_archive_count: Option<i64>,
    pub(crate) keep_catalogue: bool,
}
