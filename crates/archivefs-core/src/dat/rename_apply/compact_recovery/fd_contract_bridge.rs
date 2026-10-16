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
                            detail: detail.into(),
                        })
                        .map_err(fail)?,
                    );
                    self.checkpoint(&self.store.ticket.root, &mut tx)?;
                }
                break;
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
