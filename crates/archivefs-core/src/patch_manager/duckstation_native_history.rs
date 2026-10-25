//! One history row per DuckStation native cheat operation.
//!
//! The native adapter writes two files (`SERIAL.cht` and the per-game
//! `SERIAL.ini`) and keeps one shared-transaction journal for each. Those two
//! journals are an implementation detail of ONE operation whose undo must run
//! INI-first through [`undo_duckstation_native`]. If the history list showed
//! them as ordinary journals, a person could undo the `.cht` alone and leave
//! an INI that enables a cheat that no longer exists.
//!
//! This module groups the children under their [`DuckStationNativeReceipt`]
//! for display and marks every child journal (even one whose receipt is lost)
//! as *not independently undoable*. Everything is derived from files on disk,
//! so it is as trustworthy after a restart as the receipt and journals are.

use std::fs;
use std::path::{Path, PathBuf};

use super::duckstation_native::{
    DuckStationNativeReceipt, preview_duckstation_native_undo, read_duckstation_native_receipt,
};
use super::shared_transaction::{
    SharedHistoryReport, SharedRollbackOutcome, preview_shared_rollback,
};

/// `SharedApplyContext::source_mode` of every journal the native adapter writes.
pub const DUCKSTATION_NATIVE_SOURCE_MODE: &str = "duckstation_native";

const MAX_RECEIPTS: usize = 512;

/// What a person can do with a grouped operation right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeGroupState {
    /// Both files can be restored together.
    UndoAvailable,
    /// Both files were already restored.
    AlreadyUndone,
    /// An earlier undo stopped partway: `restored` files are back, `remaining`
    /// still hold the change. With `resumable` the rest can be restored now.
    PartiallyUndone {
        restored: Vec<String>,
        remaining: Vec<String>,
        resumable: bool,
        reasons: Vec<String>,
    },
    /// Cannot be undone from here; plain-language reasons.
    Blocked(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeHistoryGroup {
    pub receipt: DuckStationNativeReceipt,
    /// Indices into `SharedHistoryReport::journals` that belong to this
    /// operation (zero to two).
    pub children: Vec<usize>,
    pub state: NativeGroupState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupedHistoryRow {
    /// An ordinary journal, undoable on its own as before.
    Journal(usize),
    /// One DuckStation operation covering its child journals.
    DuckStationNative(Box<NativeHistoryGroup>),
    /// A native-adapter journal whose receipt is missing or unreadable. It is
    /// shown, but never offered for individual undo.
    OrphanNativeChild {
        journal_index: usize,
        /// The child was already rolled back (a failed apply undoing itself, or
        /// a finished undo whose receipt is gone): nothing remains to undo.
        rolled_back: bool,
    },
}

/// Reads every readable receipt in `history_root` (bounded). Unreadable ones
/// are skipped; their child journals then surface as `OrphanNativeChild`.
#[must_use]
pub fn discover_duckstation_native_receipts(history_root: &Path) -> Vec<DuckStationNativeReceipt> {
    let Ok(entries) = fs::read_dir(history_root) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("duckstation-native-") && name.ends_with(".receipt.json")
            })
        })
        .collect();
    paths.sort();
    paths.truncate(MAX_RECEIPTS);
    paths
        .iter()
        .filter_map(|path| read_duckstation_native_receipt(path).ok())
        .collect()
}

/// Groups the report's journals under their receipts.
#[must_use]
pub fn group_shared_history(
    report: &SharedHistoryReport,
    receipts: Vec<DuckStationNativeReceipt>,
    backup_root: &Path,
) -> Vec<GroupedHistoryRow> {
    let journal_paths: Vec<Option<PathBuf>> = report
        .journals
        .iter()
        .map(|(path, _)| path.to_path_buf().ok())
        .collect();
    let find = |wanted: &Path| {
        journal_paths
            .iter()
            .position(|candidate| candidate.as_deref() == Some(wanted))
    };

    let mut claimed = vec![false; report.journals.len()];
    let mut groups: Vec<(usize, NativeHistoryGroup)> = Vec::new();
    for receipt in receipts {
        let mut children = Vec::new();
        for side in [receipt.cht.as_ref(), receipt.game_ini.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(index) = find(&side.journal_path) {
                claimed[index] = true;
                children.push(index);
            }
        }
        let state = group_state(&receipt, backup_root);
        let anchor = children.iter().copied().min().unwrap_or(usize::MAX);
        groups.push((
            anchor,
            NativeHistoryGroup {
                receipt,
                children,
                state,
            },
        ));
    }

    let mut rows = Vec::new();
    for (index, (_, journal)) in report.journals.iter().enumerate() {
        for (anchor, group) in &groups {
            if *anchor == index {
                rows.push(GroupedHistoryRow::DuckStationNative(Box::new(
                    group.clone(),
                )));
            }
        }
        if claimed[index] {
            continue;
        }
        if journal.context.source_mode == DUCKSTATION_NATIVE_SOURCE_MODE {
            let rolled_back = report.journals[index]
                .0
                .to_path_buf()
                .ok()
                .zip(journal.destination_root.to_path_buf().ok())
                .is_some_and(|(path, root)| {
                    let preview = preview_shared_rollback(&path, &root, backup_root);
                    journal.rollback_operation_id.is_some()
                        || (!preview.entries.is_empty()
                            && preview.entries.iter().all(|entry| {
                                entry.outcome == SharedRollbackOutcome::AlreadyRolledBack
                            }))
                });
            rows.push(GroupedHistoryRow::OrphanNativeChild {
                journal_index: index,
                rolled_back,
            });
        } else {
            rows.push(GroupedHistoryRow::Journal(index));
        }
    }
    // Receipts whose journals are gone still deserve a (blocked) row.
    for (anchor, group) in groups {
        if anchor == usize::MAX {
            rows.push(GroupedHistoryRow::DuckStationNative(Box::new(group)));
        }
    }
    rows
}

fn group_state(receipt: &DuckStationNativeReceipt, backup_root: &Path) -> NativeGroupState {
    let preview = preview_duckstation_native_undo(receipt, backup_root);
    let sides = usize::from(receipt.cht.is_some()) + usize::from(receipt.game_ini.is_some());
    if !preview.already_restored.is_empty() && preview.already_restored.len() == sides {
        return NativeGroupState::AlreadyUndone;
    }
    if !preview.already_restored.is_empty() {
        return NativeGroupState::PartiallyUndone {
            restored: preview.already_restored,
            remaining: preview.remaining,
            resumable: preview.available,
            reasons: preview.reasons,
        };
    }
    if preview.available {
        NativeGroupState::UndoAvailable
    } else {
        NativeGroupState::Blocked(preview.reasons)
    }
}
