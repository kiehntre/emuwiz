//! The primary archive selection.
//!
//! One owner for "which archive is the user working on" - the focused
//! archive plus the multi-select set - so Library, Selected, Cheats & Mods
//! and Gamer View can never disagree about it. Selecting, clearing and
//! pruning against a rebuilt row list all happen here; every page reads the
//! same two fields rather than keeping its own copy.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ArchiveRow;

/// Authoritative archive context shared by every primary workflow.
///
/// Invariants:
/// - `focused` is the Library/Selected detail identity, never a row index.
/// - `selected` is the exact multi-selection used for highlighting and bulk
///   actions. A single selection always equals `focused`.
/// - the active Cheats & Mods archive is derived from `focused`; it is not
///   stored a second time. Adapter state may be cached for this identity,
///   but may never choose a different archive.
/// - queue membership and mounted records are independent and must never
///   clear or replace this context.
#[derive(Default)]
pub(crate) struct ArchiveContext {
    pub(crate) focused: Option<PathBuf>,
    pub(crate) selected: HashSet<PathBuf>,
}

impl ArchiveContext {
    pub(crate) fn select_only(&mut self, path: PathBuf) {
        self.selected.clear();
        self.selected.insert(path.clone());
        self.focused = Some(path);
    }

    pub(crate) fn clear_selection(&mut self) {
        self.focused = None;
        self.selected.clear();
    }

    pub(crate) fn prune(&mut self, rows: &[ArchiveRow]) {
        self.selected
            .retain(|path| rows.iter().any(|row| &row.path == path));
        if self
            .focused
            .as_ref()
            .is_some_and(|focused| !rows.iter().any(|row| &row.path == focused))
        {
            self.focused = None;
        }
    }

    pub(crate) fn active_cheats(&self) -> Option<&Path> {
        self.focused.as_deref()
    }
}
