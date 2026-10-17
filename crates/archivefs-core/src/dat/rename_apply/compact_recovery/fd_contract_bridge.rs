//! Contract 1-I adapter, installed only by the explicit evaluation controller. Reuses admission, signatures, consumption,
//! checkpoint storage and runtime projection; no separate journal/authority.
use super::*;
use crate::dat::rename_apply::identity::fd_publication_probe::contract::{self, Evidence, Phase};

pub(super) fn normalize_runtime(
    e: &mut TransactionEntry,
    original: &TransactionEntry,
) -> io::Result<()> {
    if !contract::enrolled(original) {
        return Ok(());
    }
    if contract::requires_runtime(e) && !e.unknown.contains_key(contract::KEY) {
        return Err(fail("FD runtime missing for Undo state"));
    }
    if let Some(runtime) = e.unknown.get(contract::KEY) {
        if original.unknown.get("r2_fd_enrolled") != Some(&serde_json::json!(1))
            || original.unknown.contains_key(contract::KEY)
        {
            return Err(fail("FD runtime requires fresh explicit enrollment"));
        }
        let parsed: Evidence = serde_json::from_value(runtime.clone()).map_err(fail)?;
        if !contract::phase_matches_entry(&parsed, e) {
            return Err(fail("FD phase contradicts completion"));
        }
        e.unknown.remove(contract::KEY);
    }
    Ok(())
}
impl Bridge {
    pub(super) fn fd_undo(
        &self,
        receipt: &Receipt,
        cancel: &AtomicBool,
    ) -> io::Result<rollback::RollbackOutcome> {
        if !self
            .base
            .transaction
            .entries
            .iter()
            .all(|e| e.unknown.get("r2_fd_enrolled") == Some(&serde_json::json!(1)))
        {
            return Err(fail("mixed FD enrollment refused"));
        }
        contract::capability::probe(&self.store.ticket.root)?;
        let mut tx = receipt.transaction.clone();
        let parked = parked_undo_entries(&tx)?;
        if !parked.is_empty()
            && eligible_entries(&self.store.ticket.root, &tx, RequestedAction::Undo, true).is_err()
        {
            return Err(fail(
                "Held for review / Undo incomplete: operator review required; no ordinary Undo entry can progress",
            ));
        }
        for index in (0..tx.entries.len()).rev() {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                // Preserve existing unresolved evidence. When no entry has
                // started Undo, persist a cancelled intent without mutation,
                // so History cannot call an attempted Undo "not requested".
                if tx.state != TransactionState::RolledBack
                    && tx.entries.iter().all(|e| {
                        matches!(
                            e.state,
                            EntryState::Applied | EntryState::RolledBack | EntryState::Skipped
                        )
                    })
                    && let Some(pending) = tx
                        .entries
                        .iter()
                        .rposition(|e| e.state == EntryState::Applied)
                {
                    let detail = "Undo cancelled before capture / Undo incomplete";
                    tx.state = TransactionState::RollingBack;
                    let e = &mut tx.entries[pending];
                    e.state = EntryState::RollingBack;
                    e.failure_reason = Some(detail.into());
                    e.unknown.insert(
                        contract::KEY.into(),
                        serde_json::to_value(Evidence {
                            action: contract::Action::Undo,
                            phase: Phase::CapturePending,
                            held_identity: None,
                            salvage_identity: None,
                            retention: None,
                            detail: detail.into(),
                        })
                        .map_err(fail)?,
                    );
                    self.checkpoint(&self.store.ticket.root, &mut tx)?;
                }
                break;
            }
            if parked.contains(&index)
                || contract::evidence(&tx, index)?
                    .is_some_and(|v| v.phase == Phase::RetainedForeign)
            {
                continue; // Only a FRESH signed Undo can reach this snapshot; never touch parked objects.
            }
            // Entries whose Apply was interrupted or failed are resolved from
            // trusted anchor/holding evidence first (see contract::recovery).
            contract::recovery::undo_entry_recovering(
                &self.store.ticket.root,
                &mut tx,
                index,
                &mut |tx| self.checkpoint(&self.store.ticket.root, tx),
                &mut |edge| {
                    test_external_action(edge)?;
                    test_exit(edge, index);
                    Ok(())
                },
            )?;
            if !matches!(
                tx.entries[index].state,
                EntryState::RolledBack | EntryState::Skipped
            ) {
                break; // Review is required before any further file-changing action.
            }
        }
        let complete = tx.state == TransactionState::RolledBack;
        let result = if complete {
            RollbackResult::FullyRolledBack
        } else {
            RollbackResult::RollbackFailed {
                failed: tx
                    .entries
                    .iter()
                    .filter(|e| !matches!(e.state, EntryState::RolledBack | EntryState::Skipped))
                    .map(|e| {
                        (
                            e.source_path.clone(),
                            e.failure_reason
                                .clone()
                                .unwrap_or_else(|| "Undo incomplete".into()),
                        )
                    })
                    .collect(),
            }
        };
        Ok(rollback::RollbackOutcome {
            result,
            transaction: tx,
        })
    }
}

// Deterministic external writer, deliberately outside admission. Only present
// in the existing test-only bridge; actions are selected by the fixture owner.
#[cfg(test)]
pub(super) fn test_external_action(edge: contract::Edge) -> io::Result<()> {
    let Some(input) = std::env::var_os("EMUWIZ_CONTRACT_ACTION_REQUEST") else {
        return Ok(());
    };
    let mut request: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&input)?).map_err(fail)?;
    if request["fd_action_edge"].as_str() != Some(&format!("{edge:?}")) {
        return Ok(());
    }
    let legacy = request["fd_action"] == "legacy-writer";
    request["mode"] = serde_json::json!(if legacy {
        "current-legacy-writer"
    } else {
        "fd-external-action"
    });
    let home = std::path::Path::new(&input)
        .parent()
        .ok_or_else(|| fail("fixture parent missing"))?;
    let external_input = home.join("fd-external-request.json");
    let output = home.join("fd-external-response.json");
    std::fs::write(&external_input, serde_json::to_vec(&request).map_err(fail)?)?;
    let executable = if legacy {
        request["legacy_executable"]
            .as_str()
            .map(std::path::PathBuf::from)
    } else {
        None
    }
    .unwrap_or(std::env::current_exe()?);
    let status = std::process::Command::new(executable)
        .args([
            "--exact",
            "dat::rename_apply::compact_recovery::storage::integration::tests::worker",
        ])
        .env("EMUWIZ_R2_REQUEST", external_input)
        .env("EMUWIZ_R2_RESPONSE", &output)
        .env_remove("EMUWIZ_CONTRACT_ACTION_REQUEST")
        .env_remove("EMUWIZ_CONTRACT_EXIT")
        .status()?;
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output)?).map_err(fail)?;
    if !status.success() || result["ok"] != true {
        return Err(fail(format!("external fixture failed: {result}")));
    }
    Ok(())
}

#[cfg(not(test))]
pub(super) fn test_external_action(_: contract::Edge) -> io::Result<()> {
    Ok(())
}

// Deterministic process death at a named boundary, optionally only for one
// entry index (EMUWIZ_CONTRACT_EXIT_ENTRY) and only after skipping the first
// n matches (EMUWIZ_CONTRACT_EXIT_SKIP), so a boundary name shared by the
// Apply roll-forward and the Undo of the same entry can be targeted exactly.
#[cfg(test)]
pub(super) fn test_exit(edge: contract::Edge, index: usize) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static MATCHED: AtomicUsize = AtomicUsize::new(0);
    if std::env::var("EMUWIZ_CONTRACT_EXIT").ok().as_deref() != Some(&format!("{edge:?}")) {
        return;
    }
    if std::env::var("EMUWIZ_CONTRACT_EXIT_ENTRY")
        .ok()
        .is_some_and(|v| v != index.to_string())
    {
        return;
    }
    let skip: usize = std::env::var("EMUWIZ_CONTRACT_EXIT_SKIP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if MATCHED.fetch_add(1, Ordering::SeqCst) < skip {
        return;
    }
    unsafe { libc::_exit(82) }
}

#[cfg(not(test))]
pub(super) fn test_exit(_: contract::Edge, _: usize) {}

/// Already parked Undo entries are never retried by ordinary Undo. Newly parked
/// entries still stop that attempt; the operator must review and sign afresh.
pub(super) fn parked_undo_entries(tx: &RenameTransaction) -> io::Result<Vec<usize>> {
    (0..tx.entries.len())
        .filter_map(|i| match contract::evidence(tx, i) {
            Ok(Some(v))
                if v.action == contract::Action::Undo
                    && !matches!(
                        v.phase,
                        Phase::CapturePending
                            | Phase::Captured
                            | Phase::CleanupPending
                            | Phase::Complete
                            | Phase::Unmutated
                            | Phase::RetainedForeign
                    ) =>
            {
                Some(Ok(i))
            }
            Err(e) => Some(Err(e)),
            _ => None,
        })
        .collect()
}
/// Per-action applicability is shared by approval and read-only grant status.
pub(super) fn eligible_entries(
    root: &Path,
    tx: &RenameTransaction,
    action: RequestedAction,
    spent: bool,
) -> io::Result<Vec<usize>> {
    if action == RequestedAction::Apply {
        if tx.state != TransactionState::Planned || spent {
            return Err(fail(
                "Apply is not applicable: transaction is spent; review Undo instead",
            ));
        }
        for entry in &tx.entries {
            if crate::dat::rename_apply::identity::capture_identity(&entry.source_path)?
                != entry.identity
            {
                return Err(fail(
                    "Apply is not applicable: source evidence changed; preserve enrolled authority",
                ));
            }
            match std::fs::symlink_metadata(&entry.destination_path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
                Ok(_) => return Err(fail("Apply is not applicable: destination is occupied")),
            }
        }
        return Ok((0..tx.entries.len()).collect());
    }
    contract::verify_records(tx)?;
    let parked = parked_undo_entries(tx)?;
    let mut eligible = Vec::new();
    for (i, e) in tx.entries.iter().enumerate() {
        let v = contract::evidence(tx, i)?;
        if action == RequestedAction::RetainForeign {
            if v.as_ref().is_some_and(|v| {
                v.action == contract::Action::Undo
                    && matches!(v.phase, Phase::ForeignHeld | Phase::RetentionPending)
            }) {
                contract::retained::verify_candidate(root, tx, i)?;
                eligible.push(i);
            }
        } else if v.as_ref().is_some_and(|v| {
            v.action == contract::Action::Apply
                && !matches!(
                    v.phase,
                    Phase::Complete
                        | Phase::CapturePending
                        | Phase::Captured
                        | Phase::CleanupPending
                )
        }) {
            return Err(fail(
                "interrupted Apply evidence requires operator review; ordinary Undo cannot guess its history",
            ));
        } else if !parked.contains(&i)
            && !matches!(e.state, EntryState::RolledBack | EntryState::Skipped)
        {
            eligible.push(i);
        } else if e.state == EntryState::Skipped {
            match std::fs::symlink_metadata(contract::anchor::path(root, tx, i)?) {
                Ok(_) => eligible.push(i),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
    }
    if eligible.is_empty()
        && !(action == RequestedAction::Undo
            && tx
                .entries
                .iter()
                .all(|e| matches!(e.state, EntryState::RolledBack | EntryState::Skipped)))
    {
        return Err(fail(
            "action is not applicable: no provable entry remains; retain evidence for operator review",
        ));
    }
    Ok(eligible)
}
impl Bridge {
    pub(super) fn verify_retention_receipts(&self, tx: &RenameTransaction) -> io::Result<()> {
        if tx.entries.iter().any(|e| {
            e.unknown
                .get(contract::KEY)
                .is_some_and(|v| v.get("retention").is_some())
        }) {
            let verifier = self
                .issuer
                .borrow()
                .clone()
                .ok_or_else(|| fail("retention issuer unavailable"))?;
            approval::verify_retention_receipts(&verifier, &self.store, &self.base, tx)?;
        }
        Ok(())
    }
    pub(super) fn retain_foreign(
        &self,
        receipt: &Receipt,
        proof: &contract::RetentionApproval,
        cancel: &AtomicBool,
    ) -> io::Result<RenameTransaction> {
        if !self.engine_enabled.get() || !shared_engine::selected(&receipt.transaction) {
            return Err(fail(
                "retention requires explicitly enabled shared FD controller",
            ));
        }
        self.consume_confirmation(receipt, RequestedAction::RetainForeign)?;
        self.verify(receipt)?;
        contract::capability::probe(&self.store.ticket.root)?;
        let mut tx = receipt.transaction.clone();
        let indices = match eligible_entries(
            &self.store.ticket.root,
            &tx,
            RequestedAction::RetainForeign,
            true,
        ) {
            Ok(indices) => indices,
            Err(_)
                if tx.entries.iter().any(|e| {
                    e.unknown
                        .get(contract::KEY)
                        .is_some_and(|v| v["phase"] == "RetainedForeign")
                }) && !tx.entries.iter().enumerate().any(|(i, _)| {
                    contract::evidence(&tx, i).ok().flatten().is_some_and(|v| {
                        matches!(v.phase, Phase::ForeignHeld | Phase::RetentionPending)
                    })
                }) =>
            {
                return Ok(tx);
            } // signed pending-checkpoint repair already recorded the receipt
            Err(error) => return Err(error),
        };
        for index in indices {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            contract::retained::settle(
                &self.store.ticket.root,
                &mut tx,
                index,
                proof,
                &mut |tx| {
                    self.verify_retention_receipts(tx)?;
                    self.checkpoint(&self.store.ticket.root, tx)
                },
                &mut |edge| {
                    test_exit(edge, index);
                    Ok(())
                },
            )?;
            self.verify_retention_receipts(&tx)?;
        }
        Ok(tx)
    }
}
