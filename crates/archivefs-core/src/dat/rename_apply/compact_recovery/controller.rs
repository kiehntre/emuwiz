//! Explicit, default-OFF core evaluation API. No application or GUI caller
//! enables this controller implicitly; the separate operator CLI opts in. Admission and signed restart authority are supplied
//! independently by its caller, never inferred from a recovery journal.
pub use super::admission::Configuration as AdmissionConfiguration;
pub use super::approval::{Approval as SignedApproval, Verifier as TrustedIssuer};
pub use super::restart::RequestedAction as RecoveryAction;
use super::*;

/// Opt-in for the bounded Contract 1-I evaluation; Default performs no enrollment.
#[derive(Clone, Copy, Default)]
pub struct RuntimeSwitch {
    enabled: bool,
}
impl RuntimeSwitch {
    pub fn enabled_for_evaluation() -> Self {
        Self { enabled: true }
    }
}
/// An operation guard. Dropping it retains unresolved durable ownership.
/// This handle is intentionally confined to its creating process/thread.
pub struct Controller {
    pub(super) bridge: Rc<Bridge>,
}
impl Controller {
    /// Enroll a newly approved, supported regular-file batch in a private test
    /// collection. The existing bounded capacity and one-container root limit
    /// remain explicit; a refusal never falls back to another writer.
    pub fn enroll(
        root: &Path,
        transaction: &RenameTransaction,
        authority: &AdmissionConfiguration,
        switch: RuntimeSwitch,
    ) -> io::Result<Self> {
        if !switch.enabled {
            return Err(fail("FD runtime switch is OFF"));
        }
        let mut tx = transaction.clone();
        for e in &mut tx.entries {
            for (key, value) in [
                ("r2_fd_enrolled", serde_json::json!(1)),
                ("r2_fd_anchor_v1", serde_json::json!(true)),
                ("r2_fd_shared_engine_v1", serde_json::json!(true)),
            ] {
                if e.unknown.contains_key(key) {
                    return Err(fail("reserved FD enrollment metadata already present"));
                }
                e.unknown.insert(key.into(), value);
            }
        }
        let base = Authority::decode_legacy(&serde_json::to_vec(&tx).map_err(fail)?, 1024 * 1024)
            .map_err(protocol)?;
        let bridge = Bridge::create(root, base, authority)?;
        bridge.engine_enabled.set(true);
        Ok(Self { bridge })
    }
    /// Reopen only through the existing signed issuer verification and durable
    /// single-use consumption path. A journal alone cannot construct authority.
    pub fn reopen(
        original: &[u8],
        authority: &AdmissionConfiguration,
        trusted: &TrustedIssuer,
        grant: &SignedApproval,
        action: RecoveryAction,
        switch: RuntimeSwitch,
    ) -> io::Result<Self> {
        if !switch.enabled {
            return Err(fail("FD runtime switch is OFF"));
        }
        let base = Authority::decode_legacy(original, 1024 * 1024).map_err(protocol)?;
        if !shared_engine::selected(&base.transaction) {
            return Err(fail("shared FD enrollment required"));
        }
        let bridge = Bridge::reopen_trusted(trusted, &grant.context, base, authority)?;
        bridge.engine_enabled.set(true);
        bridge.accept_approval(grant, action)?;
        Ok(Self { bridge })
    }
    pub fn transaction(&self) -> io::Result<RenameTransaction> {
        Ok(self.bridge.load()?.transaction)
    }
    pub fn apply(
        &self,
        approved_paths: std::collections::BTreeSet<String>,
        generation: u64,
        cancel: &AtomicBool,
    ) -> io::Result<executor::ApplyOutcome> {
        self.bridge
            .apply_approved(&self.bridge.load()?, cancel, approved_paths, generation)
    }
    pub fn undo(&self, cancel: &AtomicBool) -> io::Result<rollback::RollbackOutcome> {
        let result = self.bridge.undo(&self.bridge.load()?, cancel)?;
        if matches!(result.result, RollbackResult::FullyRolledBack) {
            self.bridge.release()?;
        }
        Ok(result)
    }
    pub fn holding_inventory(&self) -> io::Result<serde_json::Value> {
        crate::dat::rename_apply::identity::fd_publication_probe::contract::capability::inventory(
            &self.bridge.store.ticket.root,
            &self.transaction()?,
        )
    }
}
