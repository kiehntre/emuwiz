//! Contract 1-I adapter, installed only by the explicit evaluation controller. Reuses admission, signatures, consumption,
//! checkpoint storage and runtime projection; no separate journal/authority.
use super::*;
use crate::dat::rename_apply::identity::fd_publication_probe::contract::{
    self, Evidence, Fault, Phase,
};

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
            contract::undo_entry(
                &self.store.ticket.root,
                &mut tx,
                index,
                &mut |tx| self.checkpoint(&self.store.ticket.root, tx),
                &mut |edge| {
                    test_external_action(edge)?;
                    #[cfg(test)]
                    if std::env::var("EMUWIZ_CONTRACT_EXIT").ok().as_deref()
                        == Some(&format!("{edge:?}"))
                    {
                        unsafe { libc::_exit(82) }
                    }
                    Ok(())
                },
                Fault::None,
            )?;
            if tx.entries[index].state != EntryState::RolledBack {
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
                    .filter(|e| e.state != EntryState::RolledBack)
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
