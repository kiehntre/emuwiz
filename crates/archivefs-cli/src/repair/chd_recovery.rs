//! CLI observations and guidance, never an alternative rollback/recovery engine.
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use archivefs_core::dat::rename_apply::{
    ObjectIdentity, RenameTransaction, RollbackOutcome, capture_identity, identity_matches,
};
use archivefs_core::repair::optical_conversion::{
    load_chd_conversion_transaction, rollback_chd_conversion,
};
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct RecoveryReport {
    state: &'static str,
    published_chd: Vec<Location>,
    staged_chd: Vec<Location>,
    originals: Vec<Location>,
    quarantine: Vec<Location>,
    locations_complete: bool,
    guidance: String,
}

#[derive(Serialize)]
struct Location {
    path: PathBuf,
    status: &'static str,
    detail: String,
}

impl Location {
    fn inspect(path: PathBuf, expected: Option<&ObjectIdentity>) -> Self {
        // Absence is an observation, never a deduction from a journal state.
        let (status, detail) = match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ("absent", "Not present at inspection.".into())
            }
            Err(error) => ("unverified", format!("Cannot inspect this path: {error}")),
            Ok(_) => match expected.filter(|proof| identity_matches(proof, proof)) {
                None => (
                    "unverified",
                    "Present, but no usable recorded identity is available.".into(),
                ),
                Some(expected) => match capture_identity(&path) {
                    Ok(current) if identity_matches(expected, &current) => (
                        "matches_recorded_identity",
                        "Object and content match the recorded identity at inspection.".into(),
                    ),
                    Ok(_) => (
                        "identity_mismatch",
                        "Present, but does not match the recorded object/content identity.".into(),
                    ),
                    Err(error) => (
                        "unverified",
                        format!("Cannot establish a stable object/content identity: {error}"),
                    ),
                },
            },
        };
        Self {
            path,
            status,
            detail,
        }
    }
}

impl RecoveryReport {
    fn inspect(
        output: &RenameTransaction,
        source: Option<&RenameTransaction>,
        usable_pairing: bool,
        journal_dir: &Path,
    ) -> Self {
        let published_chd = output
            .entries
            .iter()
            .map(|entry| Location::inspect(entry.destination_path.clone(), Some(&entry.identity)))
            .collect();
        let staged_chd = output
            .entries
            .iter()
            .map(|entry| Location::inspect(entry.source_path.clone(), Some(&entry.identity)))
            .collect();
        // The optical core owns link validation. Reading paths for display
        // neither upgrades an invalid/legacy link nor authorizes any action.
        let originals: Vec<_> = output
            .unknown
            .get("chd_conversion_recovery")
            .and_then(|link| link.get("originals"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|original| {
                let path = PathBuf::from(original.get("path")?.as_str()?);
                let identity: Option<ObjectIdentity> = if usable_pairing {
                    original
                        .get("identity")
                        .and_then(|value| serde_json::from_value(value.clone()).ok())
                } else {
                    None
                };
                Some(Location::inspect(path, identity.as_ref()))
            })
            .collect();
        let quarantine = source
            .into_iter()
            .flat_map(|tx| &tx.entries)
            .map(|entry| Location::inspect(entry.destination_path.clone(), Some(&entry.identity)))
            .collect();
        let locations_complete =
            usable_pairing && output.entries.len() == 1 && originals.len() == 2;
        let guidance = format!(
            "Run `repair history --journal-dir <path>` with journal directory {:?} to inspect \
             the output and original-source journals. Review the published/staged CHD and \
             original/quarantine paths below. Fix journal/storage errors before explicitly \
             retrying a recoverable Undo. Entries recorded as RollbackFailed are not retried \
             automatically; identity mismatches and missing/invalid pairing need manual review. \
             Restore and verify the original CUE/BIN before any manual CHD cleanup. Do not \
             delete or overwrite remaining copies while recovery is incomplete.",
            journal_dir,
        );
        Self {
            state: "incomplete_or_uncertain",
            published_chd,
            staged_chd,
            originals,
            quarantine,
            locations_complete,
            guidance,
        }
    }

    pub(super) fn text(&self) -> String {
        let mut text = String::from(
            "CHD Undo did not fully complete. Files may already have moved and journal state \
             may be uncertain. These are read-only observations at inspection, not a persisted \
             completion or a guarantee that files cannot change.\n",
        );
        for (label, locations) in [
            ("Published CHD", &self.published_chd),
            ("Staged CHD", &self.staged_chd),
            ("Original CUE/BIN", &self.originals),
            ("Quarantine CUE/BIN", &self.quarantine),
        ] {
            for location in locations {
                let _ = writeln!(
                    text,
                    "  {label}: {:?} [{}] {}",
                    location.path, location.status, location.detail
                );
            }
        }
        if !self.locations_complete {
            text.push_str("  Some original/quarantine locations are unavailable from a usable journal pairing; inspect repair history for manual recovery.\n");
        }
        let _ = writeln!(text, "  Recovery: {}", self.guidance);
        text
    }
}

pub(super) struct ChdFailure {
    pub(super) reason: String,
    pub(super) phase: &'static str,
    pub(super) recovery: RecoveryReport,
}

impl ChdFailure {
    pub(super) fn text(&self, id: &str) -> String {
        let description = if self.phase == "before_rollback" {
            "cannot be undone automatically; refused before rollback"
        } else {
            "did not complete"
        };
        format!(
            "CHD conversion '{id}' {description}: {}.\n{}",
            self.reason,
            self.recovery.text()
        )
    }
}

// Two optical-core refusal reasons historically carry this assertion. Keep
// their diagnostic reason and manual-recovery requirement; current locations
// are established separately by the observations, never by that suffix.
fn location_neutral_reason(reason: String) -> String {
    reason
        .strip_suffix("; verified CHD retained, manual recovery required")
        .map(|reason| format!("{reason}; manual recovery required"))
        .unwrap_or(reason)
}

pub(super) fn rollback_output(
    journal_dir: &Path,
    requested_id: &str,
    output: &RenameTransaction,
    cancel: &AtomicBool,
) -> Result<RollbackOutcome, ChdFailure> {
    // Preserve the caller's identity boundary: never redirect a request using
    // an ID found inside a differently named journal.
    let mut conversion =
        load_chd_conversion_transaction(journal_dir, requested_id).map_err(|reason| {
            ChdFailure {
                reason: location_neutral_reason(reason),
                phase: "before_rollback",
                recovery: RecoveryReport::inspect(output, None, false, journal_dir),
            }
        })?;
    let result =
        rollback_chd_conversion(&mut conversion, journal_dir, cancel).map_err(|reason| {
            ChdFailure {
                reason: location_neutral_reason(reason),
                phase: "during_rollback",
                recovery: RecoveryReport::inspect(
                    &conversion.output.transaction,
                    conversion
                        .source_quarantine
                        .as_ref()
                        .map(|source| &source.transaction),
                    true,
                    journal_dir,
                ),
            }
        })?;
    Ok(RollbackOutcome {
        result,
        transaction: conversion.output.transaction,
    })
}

/// The same observations accompany typed incomplete results. The existing
/// optical loader alone decides whether a paired source journal is usable.
pub(super) fn inspect_incomplete(
    journal_dir: &Path,
    requested_id: &str,
    output: &RenameTransaction,
) -> RecoveryReport {
    match load_chd_conversion_transaction(journal_dir, requested_id) {
        Ok(conversion) => RecoveryReport::inspect(
            &conversion.output.transaction,
            conversion
                .source_quarantine
                .as_ref()
                .map(|source| &source.transaction),
            true,
            journal_dir,
        ),
        Err(_) => RecoveryReport::inspect(output, None, false, journal_dir),
    }
}
