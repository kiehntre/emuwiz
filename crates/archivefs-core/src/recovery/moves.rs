//! Keeps a prepared copy and its remembered launch choice attached to a game
//! file that is renamed or moved.
//!
//! Two routes, with different proof:
//!
//! * EmuWiz's own journalled rename or move ([`before`] / [`after`], called by
//!   the shared organisation transaction for apply and for Undo). The link is
//!   captured only if it is fully valid just before the operation, and is
//!   carried only if the file at the new path is the same file object (device
//!   and inode) with the same size and modification time and nothing is left
//!   at the old path. No game data is read, so this is safe to run wherever
//!   the rename itself runs.
//! * Anything else (a rename in a file manager, a move to another disk, an
//!   interrupted EmuWiz operation): [`rediscover`], which reads the ZIP in
//!   full and requires every file to match the prepared copy's recorded
//!   SHA-256. It runs on a worker.
//!
//! Neither route ever accepts a file because of its name, size or timestamps
//! alone, and both leave a modified, ambiguous or unreadable ZIP unlinked.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::disc_zip::DiscSetLimits;
use super::prepared_media::{
    MoveCandidate, PrepareError, PrepareProgress, PreparedMediaStore, RelinkOutcome,
};
use crate::safe_read::TrustedRoots;

/// Links captured before a rename or move, to be carried afterwards.
#[derive(Debug, Default)]
pub struct PendingMoves(Vec<MoveCandidate>);

impl PendingMoves {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Captures the valid prepared-copy links of the files about to be moved.
/// Cheap: for a file with no prepared copy it is one `stat` and one failed
/// `open` of a small index record.
pub fn before<'a>(moves: impl IntoIterator<Item = (&'a Path, &'a Path)>) -> PendingMoves {
    let Ok(store) = PreparedMediaStore::for_profile() else {
        return PendingMoves::default();
    };
    before_in(&store, moves)
}

pub fn before_in<'a>(
    store: &PreparedMediaStore,
    moves: impl IntoIterator<Item = (&'a Path, &'a Path)>,
) -> PendingMoves {
    // Resolved once; a store with no link records answers at once.
    let Some(store) = store.resolved() else {
        return PendingMoves::default();
    };
    PendingMoves(
        moves
            .into_iter()
            .filter(|(old, new)| old != new)
            .filter_map(|(old, new)| store.movable_link(old, new))
            .collect(),
    )
}

/// After a preparation that reused an existing copy for a file that was
/// renamed or moved: moves the remembered launch choice from the one path
/// that is provably gone.
pub fn adopt_remembered_choice(
    remembered_file: Option<&Path>,
    outcome: &super::prepared_media::PrepareOutcome,
    source: &Path,
) {
    if let (Some(file), Some(old)) = (remembered_file, &outcome.moved_from) {
        let _ = crate::launch::remembered_launch_system::move_remembered_launch_system_at(
            file,
            old,
            source,
            &outcome.summary.platform_id,
        );
    }
}

/// Carries each captured link whose move `happened` says took place, and
/// moves the remembered launch choice with it. Best effort by design: a
/// failure here never fails the rename; the game then simply needs
/// [`rediscover`].
pub fn after(pending: PendingMoves, happened: impl Fn(&Path, &Path) -> bool) {
    if pending.is_empty() {
        return;
    }
    let Ok(store) = PreparedMediaStore::for_profile() else {
        return;
    };
    let remembered =
        crate::launch::remembered_launch_system::default_remembered_launch_systems_path().ok();
    after_in(&store, remembered.as_deref(), pending, happened);
}

pub fn after_in(
    store: &PreparedMediaStore,
    remembered_file: Option<&Path>,
    pending: PendingMoves,
    happened: impl Fn(&Path, &Path) -> bool,
) -> usize {
    let mut carried = 0;
    for candidate in &pending.0 {
        if !happened(&candidate.old_path, &candidate.new_path) {
            continue;
        }
        if store.carry_link(candidate).is_ok() {
            carried += 1;
            if let Some(file) = remembered_file {
                let _ = crate::launch::remembered_launch_system::move_remembered_launch_system_at(
                    file,
                    &candidate.old_path,
                    &candidate.new_path,
                    &candidate.platform_id,
                );
            }
        }
    }
    carried
}

/// Finds the prepared copy of a ZIP that EmuWiz did not rename itself, by
/// content, and moves the remembered launch choice when exactly one earlier
/// path of that copy is gone (the file was renamed or moved, not duplicated).
pub fn rediscover(
    store: &PreparedMediaStore,
    remembered_file: Option<&Path>,
    source: &Path,
    trusted: &TrustedRoots,
    limits: &DiscSetLimits,
    cancel: &AtomicBool,
    progress: &dyn Fn(PrepareProgress<'_>),
) -> Result<RelinkOutcome, PrepareError> {
    let outcome = store.relink(source, trusted, limits, cancel, progress)?;
    if let RelinkOutcome::Linked {
        summary,
        moved_from: Some(old),
    } = &outcome
        && let Some(file) = remembered_file
    {
        let _ = crate::launch::remembered_launch_system::move_remembered_launch_system_at(
            file,
            old,
            source,
            &summary.platform_id,
        );
    }
    Ok(outcome)
}
