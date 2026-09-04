//! Pure presentation classification for Repair History.
//!
//! This module deliberately contains no filesystem access and no recovery
//! implementation.  The caller supplies the core's already-computed cleanup
//! classification; this layer only decides how that fact should be presented.

use archivefs_core::dat::rename_apply::{
    EntryState, ExactResumeState, RecoveryCleanupClassification, RecoveryResolution,
    RenameTransaction, TransactionState, exact_resume_state,
};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    NeedsAttention,
    RecentChanges,
    History,
    TechnicalInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Warning,
    Info,
    Muted,
    Technical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Actions {
    pub(crate) undo: bool,
    pub(crate) resume: bool,
    pub(crate) reverify: bool,
    pub(crate) technical_details: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransactionPresentation {
    pub(crate) transaction_id: String,
    pub(crate) tier: Tier,
    pub(crate) tone: Tone,
    pub(crate) headline: &'static str,
    pub(crate) detail: &'static str,
    pub(crate) actions: Actions,
    pub(crate) cleanup: RecoveryCleanupClassification,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) needs_attention: usize,
    pub(crate) undo_available: usize,
    pub(crate) historical: usize,
    pub(crate) technical_issues: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) rows: Vec<TransactionPresentation>,
    pub(crate) summary: Summary,
}

fn technically_invalid(transaction: &RenameTransaction) -> bool {
    transaction.state == TransactionState::RolledBack
        && transaction
            .entries
            .iter()
            .any(|entry| entry.state != EntryState::RolledBack)
}

fn exact_resume_pending(transaction: &RenameTransaction) -> bool {
    matches!(
        exact_resume_state(transaction),
        Ok(Some(
            ExactResumeState::Pending | ExactResumeState::Failed | ExactResumeState::Interrupted
        ))
    )
}

/// Classifies one transaction using only typed core facts supplied by the
/// caller.  In particular, `cleanup` must come from
/// `classify_recovery_cleanup`; this function never repeats filesystem checks.
pub(crate) fn classify(
    transaction: &RenameTransaction,
    cleanup: RecoveryCleanupClassification,
) -> TransactionPresentation {
    let invalid = technically_invalid(transaction);
    let resolved = transaction.recovery_resolution == Some(RecoveryResolution::LeaveUntouched);
    let resume = exact_resume_pending(transaction);
    let recovery = !resolved
        && (resume
            || matches!(cleanup, RecoveryCleanupClassification::Uncertain)
                && (transaction.needs_attention()
                    || transaction.state == TransactionState::Applied)
            || matches!(cleanup, RecoveryCleanupClassification::Actionable)
                && matches!(
                    transaction.state,
                    TransactionState::Applying
                        | TransactionState::RollingBack
                        | TransactionState::RollbackFailed
                )
                && transaction.is_rollbackable());
    let rollback_available = transaction.is_rollbackable()
        && matches!(cleanup, RecoveryCleanupClassification::Actionable)
        && !invalid;

    let (tier, tone, headline, detail) = if invalid {
        (
            Tier::TechnicalInvalid,
            Tone::Technical,
            "Technical journal issue",
            "This journal's summary does not match its recorded entry states.",
        )
    } else if recovery {
        (
            Tier::NeedsAttention,
            Tone::Warning,
            "Action needed to finish a previous change",
            if resume {
                "Recovery is required before this transaction can be considered complete."
            } else {
                "A previous change still has unresolved recovery work."
            },
        )
    } else if transaction.state == TransactionState::Applied {
        (
            Tier::RecentChanges,
            Tone::Info,
            "Completed change",
            if rollback_available {
                "Undo available."
            } else {
                "The change is complete; no undo is currently available."
            },
        )
    } else {
        (
            Tier::History,
            Tone::Muted,
            "Historical transaction",
            if exact_resume_state(transaction).is_ok_and(|state| state.is_none()) {
                "This older transaction cannot be resumed."
            } else if matches!(cleanup, RecoveryCleanupClassification::Uncertain) {
                "Resume is not safe for this transaction."
            } else {
                "No action is currently required."
            },
        )
    };

    TransactionPresentation {
        transaction_id: transaction.transaction_id.clone(),
        tier,
        tone,
        headline,
        detail,
        actions: Actions {
            undo: rollback_available,
            resume: recovery && resume && !invalid,
            reverify: !invalid,
            technical_details: invalid,
        },
        cleanup,
    }
}

impl Snapshot {
    pub(crate) fn build(
        transactions: &[RenameTransaction],
        cleanups: &[(String, RecoveryCleanupClassification)],
    ) -> Self {
        let cleanups: HashMap<&str, RecoveryCleanupClassification> = cleanups
            .iter()
            .map(|(id, cleanup)| (id.as_str(), *cleanup))
            .collect();
        let rows = transactions
            .iter()
            .map(|transaction| {
                let cleanup = cleanups
                    .get(transaction.transaction_id.as_str())
                    .copied()
                    .unwrap_or(RecoveryCleanupClassification::Uncertain);
                classify(transaction, cleanup)
            })
            .collect::<Vec<_>>();
        let mut summary = Summary::default();
        for row in &rows {
            if row.actions.undo {
                summary.undo_available += 1;
            }
            match row.tier {
                Tier::NeedsAttention => summary.needs_attention += 1,
                Tier::RecentChanges => {}
                Tier::History => summary.historical += 1,
                Tier::TechnicalInvalid => summary.technical_issues += 1,
            }
        }
        Self { rows, summary }
    }

    pub(crate) fn for_id(&self, id: &str) -> Option<&TransactionPresentation> {
        self.rows.iter().find(|row| row.transaction_id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::dat::rename_apply::{ObjectIdentity, ObjectKind, TransactionEntry};
    use std::path::PathBuf;

    fn tx(state: TransactionState, entry_states: &[EntryState]) -> RenameTransaction {
        RenameTransaction {
            transaction_id: "fixture".to_string(),
            plan_generation: 1,
            classifier_version: None,
            created_at_unix: 1,
            source_scan_root: "/missing/old/root".to_string(),
            state,
            entries: entry_states
                .iter()
                .map(|state| TransactionEntry {
                    source_path: PathBuf::from("/missing/old/root/a.bin"),
                    destination_path: PathBuf::from("/missing/old/root/A.bin"),
                    original_basename: "a.bin".to_string(),
                    proposed_basename: "A.bin".to_string(),
                    identity: ObjectIdentity {
                        size_bytes: 1,
                        modified_unix: 1,
                        kind: ObjectKind::RegularFile,
                        #[cfg(unix)]
                        ino: 1,
                        #[cfg(unix)]
                        dev: 1,
                    },
                    operation: Default::default(),
                    preflight_passed: true,
                    preflight_failures: Vec::new(),
                    state: *state,
                    failure_reason: None,
                    applied_at_unix: None,
                    rolled_back_at_unix: None,
                    unknown: Default::default(),
                })
                .collect(),
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    fn assert_tier(
        transaction: RenameTransaction,
        cleanup: RecoveryCleanupClassification,
        tier: Tier,
    ) {
        assert_eq!(classify(&transaction, cleanup).tier, tier);
    }

    #[test]
    fn applied_is_recent_and_optional_undo_is_not_urgent() {
        let transaction = tx(TransactionState::Applied, &[EntryState::Applied]);
        let row = classify(&transaction, RecoveryCleanupClassification::Actionable);
        assert_eq!(row.tier, Tier::RecentChanges);
        assert!(row.actions.undo);
    }

    #[test]
    fn pending_exact_resume_is_needs_attention() {
        let mut transaction = tx(TransactionState::Applying, &[EntryState::Applying]);
        transaction.unknown.insert(
            "emuwiz_exact_resume_state".to_string(),
            serde_json::json!("pending"),
        );
        let row = classify(&transaction, RecoveryCleanupClassification::Uncertain);
        assert_eq!(row.tier, Tier::NeedsAttention);
        assert!(row.actions.resume);
        assert!(!row.actions.undo);
    }

    #[test]
    fn consistent_rolled_back_is_history() {
        assert_tier(
            tx(TransactionState::RolledBack, &[EntryState::RolledBack]),
            RecoveryCleanupClassification::Stale(
                archivefs_core::dat::rename_apply::StaleRecoveryReason::NoActionableChanges,
            ),
            Tier::History,
        );
    }

    #[test]
    fn contradictory_rolled_back_is_technical_not_success() {
        let row = classify(
            &tx(
                TransactionState::RolledBack,
                &[
                    EntryState::RollbackFailed,
                    EntryState::ApplyFailed,
                    EntryState::Planned,
                ],
            ),
            RecoveryCleanupClassification::Stale(
                archivefs_core::dat::rename_apply::StaleRecoveryReason::NoActionableChanges,
            ),
        );
        assert_eq!(row.tier, Tier::TechnicalInvalid);
        assert!(!row.actions.undo && !row.actions.resume);
        assert!(row.actions.technical_details);
    }

    #[test]
    fn legacy_non_resumable_is_history_even_with_missing_root() {
        assert_tier(
            tx(TransactionState::ApplyFailed, &[EntryState::ApplyFailed]),
            RecoveryCleanupClassification::Stale(
                archivefs_core::dat::rename_apply::StaleRecoveryReason::LegacyWithoutExactResume,
            ),
            Tier::History,
        );
    }

    #[test]
    fn leave_untouched_is_neutral_but_keeps_safe_undo() {
        let mut transaction = tx(TransactionState::ApplyFailed, &[EntryState::Applied]);
        transaction.recovery_resolution = Some(RecoveryResolution::LeaveUntouched);
        let row = classify(&transaction, RecoveryCleanupClassification::Actionable);
        assert_eq!(row.tier, Tier::History);
        assert!(row.actions.undo);
    }
}
