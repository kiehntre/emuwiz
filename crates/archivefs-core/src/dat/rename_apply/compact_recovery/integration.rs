//! Bounded adapter behind the default-OFF evaluation controller. Restart
//! requires an out-of-band issuer, identity ticket and trusted committed prefix.
//! Serialized records alone never grant permission to call the executor.
use super::*;
use crate::dat::rename_apply::{executor, model::*, preflight::DirectoryPolicy, rollback};
use crate::safe_read::TrustedRoots;
use std::cell::{Cell, RefCell};
#[path = "integration_recovery.rs"]
mod recovery;
#[cfg(test)]
use recovery::PublicationFault;
#[path = "admission.rs"]
mod admission;
#[path = "approval.rs"]
mod approval;
#[path = "controller.rs"]
pub mod controller;
#[path = "deployment.rs"]
pub mod deployment;
#[path = "fd_contract_bridge.rs"]
mod fd_contract_bridge;
#[path = "restart.rs"]
mod restart;
#[path = "shared_engine.rs"]
pub(crate) mod shared_engine;
use restart::{Confirmation, RequestedAction};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

thread_local! { static ACTIVE: RefCell<Option<Rc<Bridge>>> = const { RefCell::new(None) }; }

pub(crate) fn checkpoint(dir: &Path, tx: &mut RenameTransaction) -> Option<io::Result<()>> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .clone()
            .filter(|b| b.store.ticket.root == dir && b.base.transaction_id() == tx.transaction_id)
            .map(|b| b.checkpoint(dir, tx))
    })
}
struct Installed;
impl Drop for Installed {
    fn drop(&mut self) {
        ACTIVE.with(|a| {
            a.borrow_mut().take();
        });
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    transaction: RenameTransaction,
    prefix: Vec<u8>,
}
struct Bridge {
    restarted: bool,
    engine_enabled: Cell<bool>,
    issuer: RefCell<Option<approval::Verifier>>,
    confirmation: RefCell<Option<Confirmation>>,
    base: Authority,
    store: Store,
    guard: RefCell<admission::Guard>,
    // In this experiment the supervisor retains this anchor independently.
    // A production restart trust root remains an activation prerequisite.
    prefix: RefCell<Vec<u8>>,
    #[cfg(test)]
    fault: RefCell<Option<(EntryState, usize, bool)>>,
    #[cfg(test)]
    checkpoint_count: Cell<usize>,
    #[cfg(test)]
    checkpoint_fault: RefCell<Option<(usize, bool)>>,
    #[cfg(test)]
    publication_fault: RefCell<Option<(Fact, PublicationFault)>>,
}
impl Bridge {
    fn create(
        root: &Path,
        base: Authority,
        authority: &admission::Configuration,
    ) -> io::Result<Rc<Self>> {
        Self::supported(&base)?;
        Self::confined(root, &base)?;
        if base
            .transaction
            .entries
            .iter()
            .any(crate::dat::rename_apply::identity::fd_publication_probe::contract::enrolled)
        {
            // This is the bounded store's root policy, not a requirement of
            // procfd publication itself. Check before claims or artifacts.
            private(&open_dir(root)?).map_err(|_| {
                fail("this bounded controller requires a private owned collection root")
            })?;
            crate::dat::rename_apply::identity::fd_publication_probe::contract::capability::probe(
                root,
            )?;
        }
        let mut guard = authority.acquire()?;
        guard.admit(&base)?;
        let anchor_setup = crate::dat::rename_apply::identity::fd_publication_probe::contract::anchor::Setup::prepare(root, &base.transaction)?;
        let store = Store::reserve(root, &base, CAP, &mut Real)?;
        store.publish_base(&base, &mut Real)?;
        publish_descriptor(root, &base, &store.ticket)?;
        edge("before-anchor-publication");
        anchor_setup.publish()?;
        edge("after-anchor-publication");
        Ok(Rc::new(Self {
            restarted: false,
            engine_enabled: Cell::new(false),
            issuer: RefCell::new(None),
            confirmation: RefCell::new(None),
            base,
            store,
            guard: RefCell::new(guard),
            prefix: RefCell::new(Vec::new()),
            #[cfg(test)]
            fault: RefCell::new(None),
            #[cfg(test)]
            checkpoint_count: Cell::new(0),
            #[cfg(test)]
            checkpoint_fault: RefCell::new(None),
            #[cfg(test)]
            publication_fault: RefCell::new(None),
        }))
    }
    fn reopen_trusted(
        trusted: &approval::Verifier,
        context: &approval::Context,
        base: Authority,
        authority: &admission::Configuration,
    ) -> io::Result<Rc<Self>> {
        let ticket = context.ticket(trusted)?;
        Self::supported(&base)?;
        Self::confined(&ticket.root, &base)?;
        let mut guard = authority.acquire()?;
        guard.resume(&base)?;
        let store = Store::reopen(&ticket, &base, CAP)?;
        context.check(trusted, &store, &base)?;
        let prefix = store.scan(&base)?.cursor;
        context.check(trusted, &store, &base)?;
        Ok(Rc::new(Self {
            restarted: true,
            engine_enabled: Cell::new(false),
            issuer: RefCell::new(Some(trusted.clone())),
            confirmation: RefCell::new(None),
            base,
            store,
            guard: RefCell::new(guard),
            prefix: RefCell::new(prefix),
            #[cfg(test)]
            fault: RefCell::new(None),
            #[cfg(test)]
            checkpoint_count: Cell::new(0),
            #[cfg(test)]
            checkpoint_fault: RefCell::new(None),
            #[cfg(test)]
            publication_fault: RefCell::new(None),
        }))
    }
    fn supported(base: &Authority) -> io::Result<()> {
        let tx = &base.transaction;
        use crate::dat::rename_apply::identity::fd_publication_probe::contract;
        if tx.entries.iter().any(contract::enrolled)
            && tx
                .entries
                .iter()
                .any(|e| !contract::enrolled(e) || e.unknown.contains_key(contract::KEY))
        {
            return Err(fail("mixed or already-started FD enrollment refused"));
        }
        if shared_engine::selected(tx) {
            let mut objects = std::collections::BTreeSet::new();
            if tx.entries.iter().any(|e| {
                !contract::enrolled(e)
                    || e.unknown.get("r2_fd_shared_engine_v1") != Some(&serde_json::json!(true))
                    || e.unknown.get("r2_fd_anchor_v1") != Some(&serde_json::json!(true))
                    || !objects.insert((e.identity.dev, e.identity.ino))
            }) {
                return Err(fail(
                    "shared FD batches require complete anchored enrollment and distinct objects",
                ));
            }
        }
        if tx.state != TransactionState::Planned
            || tx.entries.is_empty()
            || tx.entries.len() > 8
            || !tx.created_directories.is_empty()
            || tx.recovery_resolution.is_some()
            || crate::dat::rename_apply::exact_resume::has_envelope(tx)
            || tx.entries.iter().any(|e| {
                e.state != EntryState::Planned
                    || e.operation != TransactionOperation::RenameMove
                    || e.identity.kind != ObjectKind::RegularFile
            })
        {
            return Err(fail(
                "synthetic adapter supports new regular-file RenameMove only; legacy inspection grants no authority",
            ));
        }
        Ok(())
    }
    fn confined(root: &Path, base: &Authority) -> io::Result<()> {
        if base.transaction.source_scan_root != root.to_string_lossy()
            || base.transaction.entries.iter().any(|e| {
                e.source_path.parent() != Some(root)
                    || e.destination_path.parent() != Some(root)
                    || e.source_path == e.destination_path
                    || e.identity.freshness.is_none()
            })
        {
            return Err(fail(
                "operation outside private synthetic root or missing reviewed identity",
            ));
        }
        Ok(())
    }
    fn snapshot(&self) -> io::Result<Snapshot> {
        self.store.bound()?;
        let s = self.store.scan(&self.base)?;
        if s.pending.is_some() {
            return Err(fail(
                "incomplete checkpoint: exact publication retry required",
            ));
        }
        if !s.cursor.starts_with(&self.prefix.borrow()) {
            return Err(fail(
                "committed evidence lost or replaced since trusted checkpoint",
            ));
        }
        *self.prefix.borrow_mut() = s.cursor.clone();
        Ok(s)
    }
    fn load(&self) -> io::Result<Receipt> {
        let s = self.snapshot()?;
        let tx = s.runtime.restore(&self.base, &s.stream)?;
        crate::dat::rename_apply::identity::fd_publication_probe::contract::verify_records(&tx)?;
        Ok(Receipt {
            transaction: tx,
            prefix: s.cursor,
        })
    }
    fn verify(&self, receipt: &Receipt) -> io::Result<()> {
        let now = self.load()?;
        if receipt.prefix != now.prefix || receipt.transaction != now.transaction {
            return Err(fail(
                "foreign or stale complete receipt refused before mutation",
            ));
        }
        Ok(())
    }
    fn installed(self: &Rc<Self>) -> io::Result<Installed> {
        ACTIVE.with(|a| {
            if a.borrow().is_some() {
                return Err(fail("nested synthetic backend refused"));
            }
            *a.borrow_mut() = Some(self.clone());
            Ok(Installed)
        })
    }
    #[cfg(test)]
    fn apply(self: &Rc<Self>, receipt: &Receipt) -> io::Result<executor::ApplyOutcome> {
        self.apply_approved(
            receipt,
            &AtomicBool::new(false),
            receipt
                .transaction
                .entries
                .iter()
                .map(|e| e.source_path.to_string_lossy().into_owned())
                .collect(),
            receipt.transaction.plan_generation,
        )
    }
    fn apply_approved(
        self: &Rc<Self>,
        receipt: &Receipt,
        cancel: &AtomicBool,
        approved_paths: std::collections::BTreeSet<String>,
        generation: u64,
    ) -> io::Result<executor::ApplyOutcome> {
        if shared_engine::selected(&self.base.transaction) && !self.engine_enabled.get() {
            return Err(fail(
                "FD runtime switch is OFF; enrolled transaction cannot fall back to legacy",
            ));
        }
        self.consume_confirmation(receipt, RequestedAction::Apply)?;
        self.verify(receipt)?;
        if receipt.transaction.state != TransactionState::Planned || !receipt.prefix.is_empty() {
            return Err(fail("only fresh explicitly approved Apply is supported"));
        }
        let _installed = self.installed()?;
        let mut tx = receipt.transaction.clone();
        crate::dat::rename_apply::identity::fd_publication_probe::contract::anchor::verify_before_apply(&self.store.ticket.root, &tx)?;
        executor::apply_transaction(&mut executor::ApplyExecution {
            transaction: &mut tx,
            approved_paths,
            current_generation: generation,
            trusted: TrustedRoots::from_paths([&self.store.ticket.root]),
            journal_dir: self.store.ticket.root.clone(),
            hard_conflict_mode: executor::HardConflictMode::AbortAll,
            cancel,
            directory_policy: DirectoryPolicy::SameDirectory,
            allow_symlink_source: false,
        })
        .map_err(fail)
    }
    fn undo(
        self: &Rc<Self>,
        receipt: &Receipt,
        cancel: &AtomicBool,
    ) -> io::Result<rollback::RollbackOutcome> {
        if shared_engine::selected(&self.base.transaction) && !self.engine_enabled.get() {
            return Err(fail(
                "FD runtime switch is OFF; enrolled transaction cannot fall back to legacy",
            ));
        }
        self.consume_confirmation(receipt, RequestedAction::Undo)?;
        self.verify(receipt)?; // Must precede the core's already-rolled-back fast path.
        if self
            .base
            .transaction
            .entries
            .iter()
            .any(crate::dat::rename_apply::identity::fd_publication_probe::contract::enrolled)
        {
            if shared_engine::selected(&self.base.transaction) {
                let _installed = self.installed()?;
                let mut tx = receipt.transaction.clone();
                return rollback::rollback_transaction_confined(
                    &mut tx,
                    &self.store.ticket.root,
                    cancel,
                    &TrustedRoots::from_paths([&self.store.ticket.root]),
                )
                .map_err(fail);
            }
            return self.fd_undo(receipt, cancel);
        }
        let _installed = self.installed()?;
        let mut tx = receipt.transaction.clone();
        rollback::rollback_transaction_confined(
            &mut tx,
            &self.store.ticket.root,
            cancel,
            &TrustedRoots::from_paths([&self.store.ticket.root]),
        )
        .map_err(fail)
    }
    fn checkpoint(&self, dir: &Path, tx: &mut RenameTransaction) -> io::Result<()> {
        self.guard.borrow().verify(&self.base)?;
        if dir != self.store.ticket.root {
            return Err(fail("foreign journal directory"));
        }
        // Compare every authority-bearing field; only runtime observations may
        // change. In particular unknown fields and created directories remain bound.
        let mut fixed = tx.clone();
        fixed.state = self.base.transaction.state;
        for (e, original) in fixed.entries.iter_mut().zip(&self.base.transaction.entries) {
            fd_contract_bridge::normalize_runtime(e, original)?;
            e.state = original.state;
            e.preflight_passed = original.preflight_passed;
            e.preflight_failures = original.preflight_failures.clone();
            e.failure_reason = original.failure_reason.clone();
            e.applied_at_unix = original.applied_at_unix;
            e.rolled_back_at_unix = original.rolled_back_at_unix;
        }
        if fixed != self.base.transaction {
            return Err(fail("immutable operation authority changed"));
        }
        self.checkpoint_edge(false);
        #[cfg(test)]
        let fault = *self.fault.borrow();
        #[cfg(test)]
        let trigger = fault.is_some_and(|(state, index, _)| tx.entries[index].state == state);
        #[cfg(test)]
        if trigger && !fault.unwrap().2 {
            unsafe { libc::_exit(73) }
        }
        let s = self.snapshot()?;
        let obs = materialize(&self.base, &s.stream, usize::MAX).map_err(protocol)?;
        let mut facts = Vec::new();
        for (i, (e, old)) in tx.entries.iter().zip(&obs.entries).enumerate() {
            let index = i as u64;
            let fact = match (old, e.state) {
                (ObservedEntry::Pending, EntryState::Applying) => Some(Fact::ApplyIntent),
                (ObservedEntry::ApplyUnresolved, EntryState::Applied) => Some(Fact::ApplyComplete),
                (ObservedEntry::Pending | ObservedEntry::ApplyUnresolved, EntryState::Skipped) => {
                    Some(Fact::NoMutation)
                }
                (ObservedEntry::Pending, EntryState::ApplyFailed) => Some(Fact::NoMutation),
                (ObservedEntry::Unchanged, EntryState::ApplyFailed) => None,
                (ObservedEntry::Applied, EntryState::RollingBack) => Some(Fact::UndoIntent),
                (ObservedEntry::UndoUnresolved, EntryState::RolledBack) => Some(Fact::UndoComplete),
                (ObservedEntry::ApplyUnresolved, EntryState::ApplyFailed) => {
                    Some(Fact::ApplyFailure)
                }
                (ObservedEntry::UndoUnresolved, EntryState::RollbackFailed) => {
                    Some(Fact::UndoFailure)
                }
                // Reconciliation may observe that a durable undo intent did
                // not execute. Keep that intent, reusing core retry semantics.
                (ObservedEntry::UndoUnresolved, EntryState::Applied | EntryState::RollingBack) => {
                    None
                }
                (ObservedEntry::Pending, EntryState::Planned | EntryState::PreflightPassed)
                | (ObservedEntry::ApplyUnresolved, EntryState::Applying)
                | (ObservedEntry::Applied, EntryState::Applied)
                | (ObservedEntry::Undone, EntryState::RolledBack)
                | (ObservedEntry::Unchanged, EntryState::Skipped) => None,
                _ => {
                    return Err(fail(format!(
                        "unrepresentable checkpoint {old:?} -> {:?}",
                        e.state
                    )));
                }
            };
            if let Some(f) = fact
                && !s.facts.contains_key(&(index, f as u8))
            {
                facts.push((index, f));
            }
        }
        if tx.state == TransactionState::ApplyFailed && !obs.cancelled {
            facts.push((u64::MAX, Fact::Cancel));
        }
        if tx.state == TransactionState::RolledBack && !obs.rollback_recorded {
            for (i, e) in tx.entries.iter_mut().enumerate() {
                if matches!(e.state, EntryState::Planned | EntryState::PreflightPassed) {
                    facts.push((i as u64, Fact::NoMutation));
                }
            }
            facts.push((u64::MAX, Fact::RollbackSeal));
        }
        self.publish_checkpoint(&facts, tx)?;
        self.snapshot()?;
        self.checkpoint_edge(true);
        #[cfg(test)]
        if trigger && fault.unwrap().2 {
            unsafe { libc::_exit(73) }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "enrolled_tests.rs"]
mod tests;

impl Bridge {
    fn release(&self) -> io::Result<()> {
        self.guard.borrow().verify(&self.base)?;
        let s = self.snapshot()?;
        let tx = s.runtime.restore(&self.base, &s.stream)?;
        if tx.state != TransactionState::RolledBack
            || !materialize(&self.base, &s.stream, usize::MAX)
                .map_err(protocol)?
                .rollback_recorded
        {
            return Err(fail("release requires verified terminal rollback"));
        }
        crate::dat::rename_apply::identity::fd_publication_probe::contract::verify_records(&tx)?;
        for e in &tx.entries {
            if crate::dat::rename_apply::identity::capture_identity(&e.source_path)? != e.identity {
                return Err(fail("release source identity mismatch"));
            }
            match std::fs::symlink_metadata(&e.destination_path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                _ => return Err(fail("release destination not absent")),
            }
        }
        self.store.bound()?;
        edge("before-release");
        self.guard.borrow_mut().release(&self.base)?;
        edge("after-release");
        Ok(())
    }
}
#[cfg(test)]
fn edge(name: &str) {
    if std::env::var("EMUWIZ_R2_EXIT").as_deref() == Ok(name) {
        unsafe { libc::_exit(79) }
    }
}

pub(super) fn active_read(path: &Path) -> Option<io::Result<RenameTransaction>> {
    ACTIVE
        .with(|a| a.borrow().clone())
        .filter(|b| {
            Some(b.store.ticket.root.as_path()) == path.parent()
                && Some(std::ffi::OsStr::new(b.base.transaction_id())) == path.file_stem()
        })
        .map(|b| b.load().map(|r| r.transaction))
}

#[cfg(not(test))]
fn edge(_: &str) {}
