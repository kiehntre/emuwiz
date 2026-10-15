//! Synthetic confirmation boundary. These values cannot be deserialized.
//! Reuses the executor's approved-source set and exact-resume operation model;
//! this is a freshness seal around caller approval, not a new operation policy.
use super::*;
use crate::dat::rename_apply::identity::capture_identity;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum RequestedAction {
    Apply,
    Undo,
}

pub(super) struct Confirmation {
    pub(super) process: u32,
    action: RequestedAction,
    receipt: Receipt,
    observations: Vec<(Option<ObjectIdentity>, Option<ObjectIdentity>)>,
}

impl Bridge {
    pub(super) fn observations(
        &self,
    ) -> io::Result<Vec<(Option<ObjectIdentity>, Option<ObjectIdentity>)>> {
        self.store.bound()?;
        let capture = |p: &Path| match capture_identity(p) {
            Ok(i) => Ok(Some(i)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        };
        let values = self
            .base
            .transaction
            .entries
            .iter()
            .map(|e| Ok((capture(&e.source_path)?, capture(&e.destination_path)?)))
            .collect::<io::Result<Vec<_>>>()?;
        self.store.bound()?;
        Ok(values)
    }

    pub(super) fn accept_approval(
        &self,
        grant: &approval::Approval,
        action: RequestedAction,
    ) -> io::Result<()> {
        self.accept_approval_with(grant, action, &mut Real, || Ok(()))
    }
    pub(super) fn accept_approval_with(
        &self,
        grant: &approval::Approval,
        action: RequestedAction,
        io: &mut impl Io,
        after_consumption: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.confirmation.borrow_mut().take();
        if self.store.process != std::process::id() {
            return Err(fail("inherited issuer handoff refused"));
        }
        let trusted = self
            .issuer
            .borrow()
            .clone()
            .ok_or_else(|| fail("no independently configured issuer"))?;
        grant.context.require_issuer(&trusted)?;
        let observed = self.observations();
        let bytes = observed
            .as_ref()
            .ok()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(fail)?
            .unwrap_or_default();
        self.guard.borrow().verify(&self.base)?;
        edge("before-consumption");
        grant.consume(&trusted, action, &bytes)?; // fsync record AND directory before any repair/mutation
        let observations = observed?;
        self.store.bound()?;
        self.guard.borrow().verify(&self.base)?;
        edge("after-consumption");
        after_consumption()?;
        grant.context.check(&trusted, &self.store, &self.base)?;
        if observations != self.observations()? {
            return Err(fail("filesystem changed while consuming approval"));
        }
        self.repair_pending(io)?;
        let receipt = self.load()?;
        if observations != self.observations()? {
            return Err(fail("filesystem changed during checkpoint repair"));
        }
        if action == RequestedAction::Apply
            && (!receipt.prefix.is_empty()
                || receipt.transaction.state != TransactionState::Planned)
        {
            return Err(fail("spent transaction cannot resume Apply"));
        }
        for e in &receipt.transaction.entries {
            if e.state == EntryState::RolledBack && capture_identity(&e.source_path)? != e.identity
            {
                return Err(fail("completed rollback no longer matches original object"));
            }
        }
        *self.confirmation.borrow_mut() = Some(Confirmation {
            process: std::process::id(),
            action,
            receipt,
            observations,
        });
        Ok(())
    }

    pub(super) fn consume_confirmation(
        &self,
        receipt: &Receipt,
        action: RequestedAction,
    ) -> io::Result<()> {
        let approval = self.confirmation.borrow_mut().take();
        self.store.bound()?;
        self.guard.borrow().verify(&self.base)?;
        if !self.restarted {
            return Ok(());
        }
        let approval = approval
            .ok_or_else(|| fail("restart requires a newly issued, durably consumed grant"))?;
        if approval.process != std::process::id()
            || approval.action != action
            || approval.receipt.transaction != receipt.transaction
            || approval.receipt.prefix != receipt.prefix
            || approval.observations != self.observations()?
        {
            return Err(fail(
                "consumed execution handoff is stale or foreign; issuer must review again",
            ));
        }
        self.verify(receipt)?;
        Ok(())
    }
}

impl Bridge {
    pub(super) fn runtime(&self) -> runtime::RuntimeReport {
        use crate::dat::rename_apply::reconcile::{RecoveryIssueKind, classify_entry};
        use runtime::*;
        let Ok(snapshot) = self.snapshot() else {
            return RuntimeReport::blocked(RuntimeState::EvidenceUnknownOrConflicting);
        };
        let mut report = project(&self.base, &snapshot.stream, usize::MAX);
        if matches!(
            report.state,
            RuntimeState::EvidenceUnknownOrConflicting
                | RuntimeState::UnsupportedOperation
                | RuntimeState::LegacyMigrationRequired
                | RuntimeState::RecoveryBlocked
        ) {
            return report;
        }
        let Ok(receipt) = self.load() else {
            return RuntimeReport::blocked(RuntimeState::RecoveryBlocked);
        };
        // Overall state is checkpointed exactly, including cancellation before
        // any entry acquires UndoIntent. Facts alone cannot express that case.
        report.state = match receipt.transaction.state {
            TransactionState::Planned => RuntimeState::AwaitingApproval,
            TransactionState::Applying => RuntimeState::PartiallyApplied,
            TransactionState::Applied => RuntimeState::Applied,
            TransactionState::ApplyFailed => RuntimeState::ApplyFailed,
            TransactionState::RollingBack => RuntimeState::UndoPartiallyCompleted,
            TransactionState::RollbackFailed => RuntimeState::RollbackFailed,
            TransactionState::RolledBack => RuntimeState::FullyRolledBack,
        };
        if receipt.transaction.state == TransactionState::RollbackFailed {
            report.next_action = NextAction::InspectEvidence;
            report.undo = UndoAvailability::Blocked;
        }
        for (i, e) in receipt.transaction.entries.iter().enumerate() {
            let kind = classify_entry(e, i).kind;
            let valid = match e.state {
                EntryState::Applied => kind == RecoveryIssueKind::RenameConfirmed,
                EntryState::Planned | EntryState::Skipped | EntryState::RolledBack => {
                    kind == RecoveryIssueKind::RenameDidNotHappen
                }
                EntryState::Applying | EntryState::RollingBack => matches!(
                    kind,
                    RecoveryIssueKind::RenameConfirmed | RecoveryIssueKind::RenameDidNotHappen
                ),
                _ => false,
            };
            if !valid {
                return RuntimeReport::blocked(RuntimeState::RecoveryBlocked);
            }
        }
        if self.store.bound().is_err() {
            return RuntimeReport::blocked(RuntimeState::RecoveryBlocked);
        }
        if report.undo == UndoAvailability::RequiresFilesystemInspection {
            report.undo = UndoAvailability::AvailableAfterConfirmation;
        }
        report
    }
}
