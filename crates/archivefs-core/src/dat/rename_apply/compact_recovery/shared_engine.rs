//! Shared executor/Undo dispatch for an explicitly admitted FD transaction.
//! The runtime switch defaults OFF. Enrollment never permits legacy fallback.
use super::*;
use crate::dat::rename_apply::identity::fd_publication_probe::contract::{self, Phase};

pub(crate) fn selected(tx: &RenameTransaction) -> bool {
    tx.entries.iter().any(|e| {
        contract::enrolled(e)
            && e.unknown.get("r2_fd_shared_engine_v1") == Some(&serde_json::json!(true))
    })
}
fn active(dir: &Path, tx: &RenameTransaction) -> io::Result<Rc<Bridge>> {
    let bridge = ACTIVE.with(|a| a.borrow().clone()).ok_or_else(|| {
        fail("FD engine requires an admitted operation controller; legacy fallback refused")
    })?;
    if !bridge.engine_enabled.get() {
        return Err(fail(
            "FD runtime switch is OFF; enrolled transaction cannot fall back to legacy",
        ));
    }
    if bridge.store.ticket.root != dir || bridge.base.transaction_id() != tx.transaction_id {
        return Err(fail(
            "FD controller belongs to a different transaction/root",
        ));
    }
    bridge.guard.borrow().verify(&bridge.base)?;
    Ok(bridge)
}
pub(crate) fn prepare_apply_entry(
    dir: &Path,
    tx: &mut RenameTransaction,
    index: usize,
) -> Option<io::Result<()>> {
    if !selected(tx) {
        return None;
    }
    Some((|| {
        active(dir, tx)?;
        let entry = &mut tx.entries[index];
        if entry.unknown.contains_key(contract::KEY) {
            return Err(fail("fresh Apply cannot replace existing FD evidence"));
        }
        entry.unknown.insert(
            contract::KEY.into(),
            serde_json::to_value(contract::Evidence {
                action: contract::Action::Apply,
                phase: Phase::CapturePending,
                held_identity: None,
                salvage_identity: None,
                detail: "Apply capture pending / Apply incomplete".into(),
            })
            .map_err(fail)?,
        );
        Ok(())
    })())
}
pub(crate) fn apply_entry(
    dir: &Path,
    tx: &mut RenameTransaction,
    index: usize,
) -> Option<Result<(), (EntryState, String)>> {
    if !selected(tx) {
        return None;
    }
    Some(
        (|| -> io::Result<()> {
            let bridge = active(dir, tx)?;
            // The shared executor has just persisted CapturePending before
            // entering this mutation window.
            super::fd_contract_bridge::test_external_action(contract::Edge::Intent)?;
            #[cfg(test)]
            if std::env::var("EMUWIZ_CONTRACT_EXIT").ok().as_deref() == Some("Intent") {
                unsafe { libc::_exit(82) }
            }
            contract::apply_entry(
                dir,
                tx,
                index,
                &mut |tx| bridge.checkpoint(dir, tx),
                &mut |edge| {
                    super::fd_contract_bridge::test_external_action(edge)?;
                    #[cfg(test)]
                    if std::env::var("EMUWIZ_CONTRACT_EXIT").ok().as_deref()
                        == Some(&format!("{edge:?}"))
                    {
                        unsafe { libc::_exit(82) }
                    }
                    Ok(())
                },
            )?;
            if tx.entries[index].state != EntryState::Applied
                || contract::evidence(tx, index)?.is_none_or(|e| e.phase != Phase::Complete)
            {
                return Err(fail(
                    tx.entries[index]
                        .failure_reason
                        .as_deref()
                        .unwrap_or("Apply incomplete"),
                ));
            }
            Ok(())
        })()
        .map_err(|e| (EntryState::ApplyFailed, e.to_string())),
    )
}
pub(crate) fn undo(
    dir: &Path,
    tx: &mut RenameTransaction,
    cancel: &AtomicBool,
) -> Option<Result<rollback::RollbackOutcome, String>> {
    if !selected(tx) {
        return None;
    }
    Some(
        (|| -> io::Result<rollback::RollbackOutcome> {
            let bridge = active(dir, tx)?;
            let receipt = bridge.load()?;
            if receipt.transaction != *tx {
                return Err(fail("stale complete FD transaction"));
            }
            let result = bridge.fd_undo(&receipt, cancel)?;
            *tx = result.transaction.clone();
            Ok(result)
        })()
        .map_err(|e| e.to_string()),
    )
}
