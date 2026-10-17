//! Exact mutable fields carried atomically with semantic facts in the same
//! preallocated container record. Immutable/unknown fields stay in the base.
use super::*;
use crate::dat::rename_apply::identity::fd_publication_probe::contract;
use crate::dat::rename_apply::model::TransactionState;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Runtime {
    state: TransactionState,
    entries: Vec<Entry>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    state: EntryState,
    preflight_passed: bool,
    preflight_failures: Vec<String>,
    failure_reason: Option<String>,
    applied_at_unix: Option<u64>,
    rolled_back_at_unix: Option<u64>,
    // Versioned FD schema extension. Old readers deny this unknown field rather
    // than silently treating an FD operation as a legacy completed rename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fd_runtime: Option<serde_json::Value>,
}
impl Runtime {
    pub(super) fn capture(tx: &RenameTransaction) -> Self {
        Self {
            state: tx.state,
            entries: tx
                .entries
                .iter()
                .map(|e| Entry {
                    state: e.state,
                    preflight_passed: e.preflight_passed,
                    preflight_failures: e.preflight_failures.clone(),
                    failure_reason: e.failure_reason.clone(),
                    applied_at_unix: e.applied_at_unix,
                    rolled_back_at_unix: e.rolled_back_at_unix,
                    fd_runtime: if contract::enrolled(e) {
                        e.unknown.get(contract::KEY).cloned()
                    } else {
                        None
                    },
                })
                .collect(),
        }
    }
    pub(super) fn restore(&self, base: &Authority, stream: &[u8]) -> io::Result<RenameTransaction> {
        let obs = materialize(base, stream, usize::MAX).map_err(protocol)?;
        if self.entries.len() != obs.entries.len() {
            return Err(fail("runtime entry count differs"));
        }
        let shared_fd = super::integration::shared_engine::selected(&base.transaction);
        for (e, state) in self.entries.iter().zip(&obs.entries) {
            use ObservedEntry::*;
            // ApplyFailed can mean either a harmless preflight refusal or an
            // unresolved mutation. The authoritative semantic stream tells
            // these apart; only the latter requires capture/holding evidence.
            if shared_fd && matches!(state, ApplyUnresolved) && e.fd_runtime.is_none() {
                return Err(fail(
                    "FD runtime missing after Apply intent; operator review required",
                ));
            }
            let valid = match state {
                Pending => matches!(e.state, EntryState::Planned | EntryState::PreflightPassed),
                ApplyUnresolved => {
                    matches!(e.state, EntryState::Applying | EntryState::ApplyFailed)
                }
                Applied => e.state == EntryState::Applied,
                // The core reconciler may confirm a durable UndoIntent has not
                // run and set Applied for a retry. The original intent remains.
                UndoUnresolved => matches!(
                    e.state,
                    EntryState::Applied | EntryState::RollingBack | EntryState::RollbackFailed
                ),
                Undone => e.state == EntryState::RolledBack,
                Unchanged => matches!(
                    e.state,
                    EntryState::Planned
                        | EntryState::PreflightPassed
                        | EntryState::Skipped
                        | EntryState::ApplyFailed
                ),
            };
            if !valid {
                return Err(fail("runtime contradicts semantic evidence"));
            }
        }
        if (self.state == TransactionState::Applied
            && obs
                .entries
                .iter()
                .any(|s| !matches!(s, ObservedEntry::Applied | ObservedEntry::Unchanged)))
            || (self.state == TransactionState::Planned && obs.cancelled)
            || (self.state == TransactionState::RolledBack) != obs.rollback_recorded
            || (self.state == TransactionState::Applied
                && self
                    .entries
                    .iter()
                    .any(|e| !matches!(e.state, EntryState::Applied | EntryState::Skipped)))
            || (self.state == TransactionState::Planned
                && obs
                    .entries
                    .iter()
                    .any(|s| !matches!(s, ObservedEntry::Pending | ObservedEntry::Unchanged)))
        {
            return Err(fail("overall state contradicts semantic evidence"));
        }
        let mut tx = base.transaction.clone();
        tx.state = self.state;
        for (e, r) in tx.entries.iter_mut().zip(&self.entries) {
            e.state = r.state;
            e.preflight_passed = r.preflight_passed;
            e.preflight_failures = r.preflight_failures.clone();
            e.failure_reason = r.failure_reason.clone();
            e.applied_at_unix = r.applied_at_unix;
            e.rolled_back_at_unix = r.rolled_back_at_unix;
            if crate::dat::rename_apply::identity::fd_publication_probe::contract::requires_runtime(
                e,
            ) && r.fd_runtime.is_none()
            {
                return Err(fail("FD runtime missing for Undo state"));
            }
            if let Some(value) = &r.fd_runtime {
                use crate::dat::rename_apply::identity::fd_publication_probe::contract::{
                    Evidence, KEY, phase_matches_entry,
                };
                if e.unknown.get("r2_fd_enrolled") != Some(&serde_json::json!(1)) {
                    return Err(fail("FD runtime on unenrolled authority"));
                }
                let evidence: Evidence = serde_json::from_value(value.clone()).map_err(fail)?;
                if !phase_matches_entry(&evidence, e) {
                    return Err(fail("FD completion contradicts entry state"));
                }
                e.unknown.insert(KEY.into(), value.clone());
            }
        }
        Ok(tx)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    binding: [u8; 32],
    previous: [u8; 32],
    facts: Vec<(u64, Fact)>,
    runtime: Runtime,
}
impl Store {
    fn empty_snapshot(&self, base: &Authority) -> Snapshot {
        Snapshot {
            stream: vec![],
            facts: BTreeMap::new(),
            pending: None,
            cursor: vec![],
            runtime: Runtime::capture(&base.transaction),
            count: 0,
        }
    }
    fn record_payload(data: &[u8]) -> io::Result<(Vec<u8>, usize)> {
        if data.len() < 16 {
            return Err(fail("truncated checkpoint header"));
        }
        let length = u64::from_le_bytes(decoded(&data[..16])?.try_into().unwrap());
        let length = usize::try_from(length).map_err(fail)?;
        if length < 32 || length > (SLOT as usize - 17) / 2 {
            return Err(fail("checkpoint size limit"));
        }
        let end = 16 + length * 2;
        let decoded = decoded(&data[16..end])?;
        let split = decoded.len() - 32;
        if Sha256::digest(&decoded[..split]).as_slice() != &decoded[split..] {
            return Err(fail("checkpoint digest mismatch"));
        }
        Ok((decoded, end))
    }
    fn extend(base: &Authority, s: &mut Snapshot, bytes: &[u8]) -> io::Result<()> {
        let split = bytes.len() - 32;
        let record: Record = serde_json::from_slice(&bytes[..split]).map_err(fail)?;
        let previous: [u8; 32] = s.cursor.last_chunk().copied().unwrap_or([0; 32]);
        if record.binding != base.binding || record.previous != previous {
            return Err(fail("checkpoint binding or predecessor mismatch"));
        }
        let mut prev: [u8; 32] = s.stream.last_chunk().copied().unwrap_or([0; 32]);
        for (index, fact) in record.facts {
            if s.facts.contains_key(&(index, fact as u8)) {
                return Err(fail("duplicate checkpoint fact"));
            }
            let f = frame(base, index, fact, prev);
            prev = f[79..].try_into().unwrap();
            s.stream.extend(&f);
            s.facts.insert((index, fact as u8), f);
        }
        record.runtime.restore(base, &s.stream)?;
        s.runtime = record.runtime;
        s.cursor.extend(&bytes[split..]);
        s.count += 1;
        Ok(())
    }
    pub(super) fn scan(&self, base: &Authority) -> io::Result<Snapshot> {
        self.bound()?;
        self.allocation()?;
        if base.binding != self.ticket.binding {
            return Err(fail("foreign authority"));
        }
        let b = self.read(0, self.layout.base_end as usize)?;
        let expected = base_payload(base, &self.layout);
        if b[expected.len()] != COMMITTED
            || b[..expected.len()] != expected
            || b[expected.len() + 1..].iter().any(|b| *b != 0)
        {
            return Err(fail("base incomplete or conflicting"));
        }
        let mut s = self.empty_snapshot(base);
        let mut ended = false;
        let mut data = vec![0; SLOT as usize];
        for position in 0..self.layout.slots {
            self.file
                .read_exact_at(&mut data, self.layout.base_end + position * SLOT)?;
            if data.iter().all(|b| *b == 0) {
                ended = true;
                continue;
            }
            if ended || s.pending.is_some() {
                return Err(fail("checkpoint gap or conflicting suffix"));
            }
            // A partial payload never yields a reconstructed success. A fully
            // written uncommitted payload may be verified and repaired after a
            // fresh signed approval; missing runtime bytes cannot be invented.
            match Self::record_payload(&data) {
                Ok((bytes, end)) if data[end] == COMMITTED => {
                    if data[end + 1..].iter().any(|b| *b != 0) {
                        return Err(fail("corrupt checkpoint padding"));
                    }
                    Self::extend(base, &mut s, &bytes)?;
                }
                Ok((_, end)) if data[end] == 0 && data[end + 1..].iter().all(|b| *b == 0) => {
                    s.pending = Some((position, data.clone()))
                }
                _ => {
                    let prefix = data.iter().position(|b| *b == 0).unwrap_or(data.len());
                    if data[..prefix].iter().any(|b| !(1..=16).contains(b))
                        || data[prefix..].iter().any(|b| *b != 0)
                    {
                        return Err(fail("corrupt checkpoint evidence"));
                    }
                    s.pending = Some((position, data.clone()));
                }
            }
        }
        Ok(s)
    }
    pub(super) fn publish(
        &self,
        base: &Authority,
        tx: &RenameTransaction,
        prepare: impl FnOnce(&Snapshot) -> io::Result<Vec<(u64, Fact)>>,
        io: &mut impl Io,
    ) -> io::Result<()> {
        let mut s = self.scan(base)?;
        if s.pending.is_some() {
            return Err(fail("incomplete checkpoint requires signed recovery"));
        }
        // Derive the next facts from this exact fresh scan, without a second
        // container read between pure planning and publication. Never cache a
        // snapshot across calls, a write, or a recovery boundary.
        let actions = prepare(&s)?;
        let runtime = Runtime::capture(tx);
        if actions.is_empty() && runtime == s.runtime {
            return Ok(());
        }
        if s.count >= self.layout.slots {
            return Err(fail("checkpoint reservation exhausted; ownership retained"));
        }
        let record = Record {
            binding: base.binding,
            previous: s.cursor.last_chunk().copied().unwrap_or([0; 32]),
            facts: actions,
            runtime,
        };
        let mut bytes = serde_json::to_vec(&record).map_err(fail)?;
        bytes.extend(Sha256::digest(&bytes).as_slice());
        Self::extend(base, &mut s, &bytes)?;
        if bytes.len() > (SLOT as usize - 17) / 2 {
            return Err(fail(
                "checkpoint payload exceeds reservation; ownership retained",
            ));
        }
        let mut payload = encoded(&(bytes.len() as u64).to_le_bytes());
        payload.extend(encoded(&bytes));
        self.publish_payload(
            self.layout.base_end + (s.count - 1) * SLOT,
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
    pub(super) fn pending_transaction(
        &self,
        base: &Authority,
    ) -> io::Result<Option<(u64, Vec<u8>, RenameTransaction)>> {
        let mut s = self.scan(base)?;
        let Some((position, data)) = s.pending.take() else {
            return Ok(None);
        };
        let (bytes, end) = Self::record_payload(&data).map_err(|_| {
            fail("truncated checkpoint: exact runtime values unavailable; evidence retained")
        })?;
        if data[end] != 0 || data[end + 1..].iter().any(|b| *b != 0) {
            return Err(fail("conflicting pending checkpoint"));
        }
        Self::extend(base, &mut s, &bytes)?;
        Ok(Some((
            position,
            data[..end].to_vec(),
            s.runtime.restore(base, &s.stream)?,
        )))
    }
}
