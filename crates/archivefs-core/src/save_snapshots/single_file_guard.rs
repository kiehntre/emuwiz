//! Guarded single-file restore and Undo.
//!
//! The generic executor in [`super`] takes the emulator's closure as an input.
//! This module is the only place that produces it for a live save file, and
//! it keeps the proof alive across the write:
//!
//! 1. A kernel read lease on the exact target file is taken (granted only
//!    while no process at all has it open, readers included) and the process
//!    scan runs under it. Without a usable lease (unsupported filesystem,
//!    target missing, other owner) the strict scan alone decides, so any
//!    unreadable process or missing target is `Unknown`.
//! 2. The restore is refused unless that observation is a proved `Closed` and
//!    the fresh preflight still matches the one the person confirmed.
//! 3. The proof is re-checked after the journal is durable and immediately
//!    before the atomic publication (refused as `SaveInUse` if it no longer
//!    holds), and once more after the write against the new file (a fresh
//!    lease, since the write replaces the inode) which must still say
//!    `Closed`. If not, the
//!    result is reported as `opened_during_restore` and nothing is hidden.
//!    The write is not reversed automatically: reversing under a program that
//!    may be writing the same file would be a second unsafe write. The receipt
//!    stays, so Undo remains available once the program is closed.
//!
//! Process names are only ever used to refuse. A missing name never clears.

use super::{
    SaveQuiescenceRequirement, SaveRestoreApplyOptions, SaveRestoreError, SaveRestorePreflight,
    SaveRestoreReceipt, SaveRestoreRefusal, SaveRestoreResult, SaveSnapshot,
    apply_single_file_restore_gated, build_single_file_restore_preflight, history::closure_binding,
    history::snapshot_closure_supported, undo_single_file_restore_gated,
};
use crate::memory_card_inventory::restore_guard::{CardLeaseAttempt, CardWriteLease};
use crate::persistent_state_inventory::StateEmulator;
use crate::save_snapshots::tree_restore::safety::{EmulatorQuiescence, QuiescenceProvider};
use std::path::Path;

#[derive(Debug)]
pub struct GuardedRestore {
    pub result: SaveRestoreResult,
    /// A program may have opened the save while it was being replaced.
    pub opened_during_restore: bool,
}

#[derive(Debug)]
pub struct GuardedUndo {
    pub opened_during_undo: bool,
}

fn requirement(state: EmulatorQuiescence) -> SaveQuiescenceRequirement {
    match state {
        EmulatorQuiescence::Closed => SaveQuiescenceRequirement::ConfirmedClosed,
        EmulatorQuiescence::Running => SaveQuiescenceRequirement::Running,
        EmulatorQuiescence::CardInUse => SaveQuiescenceRequirement::InUse,
        EmulatorQuiescence::Unknown => SaveQuiescenceRequirement::Unknown,
    }
}

/// Observation held open for one operation.
struct Window<'a> {
    observer: &'a dyn QuiescenceProvider,
    binding: crate::save_snapshots::tree_restore::safety::DirectorySaveBinding,
    lease: Option<CardWriteLease>,
    before: SaveQuiescenceRequirement,
    target: &'a Path,
}

impl<'a> Window<'a> {
    fn open(
        emulator: StateEmulator,
        target: &'a Path,
        observer: &'a dyn QuiescenceProvider,
    ) -> Self {
        let binding = closure_binding(emulator, target);
        if !snapshot_closure_supported(emulator) {
            return Self {
                observer,
                binding,
                lease: None,
                before: SaveQuiescenceRequirement::Unknown,
                target,
            };
        }
        let (lease, state) = match CardWriteLease::acquire(target, None) {
            CardLeaseAttempt::Held(lease) => {
                let state = observer.observe_under_card_lease(&binding, &lease);
                // Broken while observing: something opened the file for
                // writing, whatever the scan concluded (unless it saw the
                // emulator itself).
                let state = if state != EmulatorQuiescence::Running && !lease.intact() {
                    EmulatorQuiescence::CardInUse
                } else {
                    state
                };
                (Some(lease), state)
            }
            CardLeaseAttempt::Writers => (None, EmulatorQuiescence::CardInUse),
            CardLeaseAttempt::Changed | CardLeaseAttempt::Unavailable => {
                (None, observer.observe(&binding))
            }
        };
        Self {
            observer,
            binding,
            lease,
            before: requirement(state),
            target,
        }
    }

    /// Pre-publication gate: the original lease is intact AND a fresh scan
    /// made under it still says exactly `Closed` (not merely "not Running").
    /// Without a lease the strict scan alone must say `Closed`.
    fn still_closed(&self) -> bool {
        match &self.lease {
            Some(lease) => {
                lease.intact()
                    && self.observer.observe_under_card_lease(&self.binding, lease)
                        == EmulatorQuiescence::Closed
                    && lease.intact()
            }
            None => self.observer.observe(&self.binding) == EmulatorQuiescence::Closed,
        }
    }

    /// Post-write check. The write replaced the file with a new inode, so the
    /// original lease says nothing about it: take a fresh lease on the file
    /// now at the path and scan under it (or scan strictly where no lease can
    /// be had). Anything but a proved `Closed` is reported to the person.
    fn closed_after_write(&self) -> bool {
        match CardWriteLease::acquire(self.target, None) {
            CardLeaseAttempt::Held(lease) => {
                self.observer
                    .observe_under_card_lease(&self.binding, &lease)
                    == EmulatorQuiescence::Closed
                    && lease.intact()
            }
            CardLeaseAttempt::Writers => false,
            CardLeaseAttempt::Changed | CardLeaseAttempt::Unavailable => {
                self.observer.observe(&self.binding) == EmulatorQuiescence::Closed
            }
        }
    }
}

/// Restore `snapshot` over the exact file the person previewed. `confirmed`
/// is the preflight shown at confirmation; the restore refuses if anything it
/// described has changed.
pub fn restore_single_file_guarded(
    snapshot: &SaveSnapshot,
    confirmed: &SaveRestorePreflight,
    emulator: StateEmulator,
    observer: &dyn QuiescenceProvider,
    options: &SaveRestoreApplyOptions,
) -> Result<GuardedRestore, SaveRestoreError> {
    let target = &confirmed.target_binding.target_path;
    let window = Window::open(emulator, target, observer);
    let fresh =
        build_single_file_restore_preflight(snapshot, &confirmed.target_binding, window.before);
    if fresh.snapshot_sha256 != confirmed.snapshot_sha256 {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::SnapshotHashChanged,
        ));
    }
    if fresh.target_fingerprint != confirmed.target_fingerprint
        || fresh.target_exists != confirmed.target_exists
    {
        return Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::DestinationChanged,
        ));
    }
    // Re-proved after the journal is durable, immediately before publication.
    let result =
        apply_single_file_restore_gated(snapshot, &fresh, options, &|| window.still_closed())?;
    // A write that landed after publication shows as different bytes: report
    // it too, whatever the scan says.
    let bytes_intact = super::hash_and_metadata(target)
        .is_ok_and(|(_, hash, _)| hash == result.receipt.snapshot_sha256);
    let opened_during_restore = !window.closed_after_write() || !bytes_intact;
    Ok(GuardedRestore {
        result,
        opened_during_restore,
    })
}

/// Undo one restore, under the same proof of closure.
pub fn undo_single_file_guarded(
    receipt: &SaveRestoreReceipt,
    emulator: StateEmulator,
    observer: &dyn QuiescenceProvider,
) -> Result<GuardedUndo, SaveRestoreError> {
    let window = Window::open(emulator, &receipt.target_binding.target_path, observer);
    // Re-proved after the previous save is staged, immediately before it is
    // published over the restored file, exactly as apply does.
    undo_single_file_restore_gated(receipt, window.before, &|| window.still_closed())?;
    Ok(GuardedUndo {
        opened_during_undo: !window.closed_after_write(),
    })
}

#[cfg(test)]
mod tests;
