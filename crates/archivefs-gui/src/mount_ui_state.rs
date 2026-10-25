use std::collections::HashSet;
use std::path::PathBuf;

use crate::{
    MountAllConfirmation, MountAllResult, RunningMountAll, RunningOperation, RunningUnmountAll,
    UnmountAllConfirmation, UnmountAllResult, UnmountSelectedConfirmation,
};

/// UI/session state for mount-page controls and mount-operation presentation.
///
/// Mount execution, worker protocols, and filesystem semantics remain owned by
/// the existing mount controllers. This bundle only consolidates the state
/// that the app shell passes between those controllers and the UI.
#[derive(Default)]
pub(crate) struct MountUiState {
    pub(crate) operation: Option<RunningOperation>,
    pub(crate) mount_all: Option<RunningMountAll>,
    pub(crate) unmount_all: Option<RunningUnmountAll>,
    pub(crate) confirm_mount_all: Option<MountAllConfirmation>,
    pub(crate) focus_mount_all_cancel: bool,
    pub(crate) mount_all_result: Option<MountAllResult>,
    pub(crate) mount_queue: Vec<PathBuf>,
    pub(crate) mount_search: String,
    pub(crate) confirm_mount_queue: bool,
    pub(crate) active_mounts_confirm_unmount: Option<PathBuf>,
    pub(crate) confirm_unmount_all: Option<UnmountAllConfirmation>,
    pub(crate) focus_unmount_all_cancel: bool,
    pub(crate) confirm_unmount_selected: Option<UnmountSelectedConfirmation>,
    pub(crate) focus_unmount_selected_cancel: bool,
    pub(crate) unmount_all_result: Option<UnmountAllResult>,
    pub(crate) confirm_unmount: Option<PathBuf>,
    pub(crate) confirm_lazy_unmount: Option<PathBuf>,
    pub(crate) confirm_lazy_unmount_final: Option<PathBuf>,
    pub(crate) focus_lazy_cancel: bool,
    pub(crate) focus_final_lazy_cancel: bool,
    pub(crate) lazy_unmount_offers: HashSet<PathBuf>,
    pub(crate) remount_offers: HashSet<PathBuf>,
    pub(crate) cleanup_after_unmount: bool,
    pub(crate) mount_all_typed_count: String,
    pub(crate) unmount_all_typed_count: String,
    pub(crate) confirm_mount_selected: Option<Vec<PathBuf>>,
    pub(crate) focus_mount_selected_cancel: bool,
    pub(crate) mount_selected_typed_count: String,
}
