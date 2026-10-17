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
    retention: Option<
        crate::dat::rename_apply::identity::fd_publication_probe::contract::RetentionApproval,
    >,
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
        let guard = authority.acquire()?;
        match std::fs::symlink_metadata(root.join(NAMESPACE)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
            Ok(_) => {
                return Err(fail(
                    "existing recovery root requires verified explicit succession; no enrollment claims created",
                ));
            }
        }
        Self::enroll_locked(root, transaction, guard, switch, false)
    }
    pub(super) fn enroll_locked(
        root: &Path,
        transaction: &RenameTransaction,
        guard: admission::Guard,
        switch: RuntimeSwitch,
        generation: bool,
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
        let bridge = Bridge::create_locked(root, base, guard, generation)?;
        bridge.engine_enabled.set(true);
        Ok(Self {
            bridge,
            retention: None,
        })
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
        Self::reopen_locked(
            original,
            authority.acquire()?,
            trusted,
            grant,
            action,
            switch,
        )
    }
    pub(super) fn reopen_locked(
        original: &[u8],
        guard: admission::Guard,
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
        let bridge = Bridge::reopen_locked(trusted, &grant.context, base, guard)?;
        bridge.engine_enabled.set(true);
        bridge.accept_approval(grant, action)?;
        let retention = if action == RecoveryAction::RetainForeign {
            Some(crate::dat::rename_apply::identity::fd_publication_probe::contract::RetentionApproval {
                nonce: grant.nonce.clone(), observations: bridge.observations()?,
            })
        } else {
            None
        };
        Ok(Self { bridge, retention })
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
    /// Explicit retention never substitutes for ordinary signed Undo or its ownership release.
    pub fn retain_foreign(&self, cancel: &AtomicBool) -> io::Result<RenameTransaction> {
        let proof = self
            .retention
            .as_ref()
            .ok_or_else(|| fail("separate retained-file authority required"))?;
        self.bridge
            .retain_foreign(&self.bridge.load()?, proof, cancel)
    }
    pub fn holding_inventory(&self) -> io::Result<serde_json::Value> {
        crate::dat::rename_apply::identity::fd_publication_probe::contract::capability::inventory(
            &self.bridge.store.ticket.root,
            &self.transaction()?,
        )
    }
}
