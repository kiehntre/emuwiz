//! Approved repair of a completely written checkpoint. A torn runtime payload
//! cannot be regenerated from semantic facts: retain it and refuse explicitly.
use super::*;
use crate::dat::rename_apply::reconcile::{RecoveryIssueKind, classify_entry};
#[derive(Clone, Copy)]
pub(super) enum PublicationFault {
    Exit(Event),
    Short(usize),
    Fail(Event),
}
struct FaultIo(PublicationFault, u64, bool);
impl Io for FaultIo {
    fn event(&mut self, event: Event) -> io::Result<()> {
        if self.2 {
            match self.0 {
                PublicationFault::Exit(e) if event == e => unsafe { libc::_exit(77) },
                PublicationFault::Fail(e) if event == e => {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                _ => (),
            }
        }
        Ok(())
    }
    fn write(&mut self, f: &File, offset: u64, bytes: &[u8]) -> io::Result<()> {
        self.2 = offset >= self.1 && offset < self.1 + SLOT;
        if let PublicationFault::Short(limit) = self.0
            && self.2
        {
            f.write_all_at(&bytes[..limit.min(bytes.len())], offset)?;
            f.sync_all()?;
            return Err(io::Error::from_raw_os_error(libc::ENOSPC));
        }
        f.write_all_at(bytes, offset)
    }
}
impl Bridge {
    pub(super) fn repair_pending(&self, io: &mut impl Io) -> io::Result<()> {
        self.guard.borrow().verify(&self.base)?;
        self.store.bound()?;
        let s = self.store.scan(&self.base)?;
        if !s.cursor.starts_with(&self.prefix.borrow()) {
            return Err(fail("restart evidence no longer extends trusted prefix"));
        }
        let Some((position, payload, tx)) = self.store.pending_transaction(&self.base)? else {
            return Ok(());
        };
        for (i, e) in tx.entries.iter().enumerate() {
            let wanted = match e.state {
                EntryState::Applied => Some(RecoveryIssueKind::RenameConfirmed),
                EntryState::RolledBack | EntryState::Skipped => {
                    Some(RecoveryIssueKind::RenameDidNotHappen)
                }
                _ => None,
            };
            if wanted.is_some_and(|wanted| classify_entry(e, i).kind != wanted) {
                return Err(fail(
                    "pending completion contradicts current filesystem; inspection required",
                ));
            }
        }
        self.store.bound()?;
        self.store.publish_payload(
            self.store.layout.base_end + position * SLOT,
            &payload,
            [
                Event::RecordWrite,
                Event::RecordSync,
                Event::RecordCommit,
                Event::RecordCommitSync,
            ],
            io,
        )
    }
    pub(super) fn publish_checkpoint(
        &self,
        facts: &[(u64, Fact)],
        tx: &RenameTransaction,
    ) -> io::Result<()> {
        if let Some((target, fault)) = *self.publication_fault.borrow()
            && facts.iter().any(|(_, fact)| *fact == target)
        {
            let offset = self.store.layout.base_end + self.store.scan(&self.base)?.count * SLOT;
            return self
                .store
                .publish(&self.base, facts, tx, &mut FaultIo(fault, offset, false));
        }
        self.store.publish(&self.base, facts, tx, &mut Real)
    }
    pub(super) fn checkpoint_edge(&self, after: bool) {
        if !after {
            self.checkpoint_count.set(self.checkpoint_count.get() + 1);
        }
        if *self.checkpoint_fault.borrow() == Some((self.checkpoint_count.get(), after)) {
            unsafe { libc::_exit(76) }
        }
    }
}
