//! Recovery evidence decoding and observation. The decoder/materializer below
//! are pure; the Linux storage child supplies application readers and an explicit
//! default-OFF controller and explicit operator signing workflow.
//! A valid record describes claimed history; it does not authenticate that history.
use super::model::{EntryState, RenameTransaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const VERSION: u16 = 2;
pub const FRAME_BYTES: usize = 111;
const MAGIC: &[u8; 4] = b"EWR2";

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Limit,
    Overflow,
    Truncated,
    Binding,
    Transition,
    Conflict,
}

/// Immutable, complete original bytes, including unknown nested legacy fields.
/// Content hashes bind evidence, not ownership. This is NOT a transaction lease.
#[derive(Debug)]
pub struct Authority {
    original: Vec<u8>,
    transaction: RenameTransaction,
    binding: [u8; 32],
}
impl Authority {
    /// Inspection limits are caller supplied; this does not impose new legacy
    /// admission limits. No production reader calls this experimental decoder.
    pub fn decode_legacy(bytes: &[u8], max_bytes: usize) -> Result<Self, Error> {
        if bytes.len() > max_bytes {
            return Err(Error::Limit);
        }
        // Strict recursive duplicate-key validation before typed decoding.
        let _: UniqueJson = serde_json::from_slice(bytes).map_err(|_| Error::Malformed)?;
        let transaction: RenameTransaction =
            serde_json::from_slice(bytes).map_err(|_| Error::Malformed)?;
        if transaction.transaction_id.is_empty() {
            return Err(Error::Binding);
        }
        let mut hash = Sha256::new();
        hash.update(b"EmuWiz/read-only-authority/v2\0");
        hash.update(bytes);
        Ok(Self {
            original: bytes.to_vec(),
            transaction,
            binding: hash.finalize().into(),
        })
    }
    pub fn original_bytes(&self) -> &[u8] {
        &self.original
    }
    pub fn binding(&self) -> [u8; 32] {
        self.binding
    }
    pub fn transaction_id(&self) -> &str {
        &self.transaction.transaction_id
    }
}

// A recursive JSON visitor rejects duplicate keys, including unknown metadata.
struct UniqueJson;
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut keys = std::collections::BTreeSet::new();
                while let Some(k) = a.next_key::<String>()? {
                    if !keys.insert(k) {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    a.next_value::<UniqueJson>()?;
                }
                Ok(UniqueJson)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<UniqueJson, A::Error> {
                while a.next_element::<UniqueJson>()?.is_some() {}
                Ok(UniqueJson)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
        }
        d.deserialize_any(Visitor)
    }
}

/// Monotone semantic facts. A retry repeats the identical intent; errors leave
/// an intent unresolved. No per-attempt timestamp or unlimited error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Fact {
    ApplyIntent = 1,
    ApplyComplete = 2,
    NoMutation = 3,
    UndoIntent = 4,
    UndoComplete = 5,
    ApplyFailure = 6,
    UndoFailure = 7,
    DirectoryIntent = 8,
    DirectoryComplete = 9,
    Cancel = 10,
    RollbackSeal = 11,
}
impl Fact {
    fn from_tag(tag: u8) -> Result<Self, Error> {
        Ok(match tag {
            1 => Self::ApplyIntent,
            2 => Self::ApplyComplete,
            3 => Self::NoMutation,
            4 => Self::UndoIntent,
            5 => Self::UndoComplete,
            6 => Self::ApplyFailure,
            7 => Self::UndoFailure,
            8 => Self::DirectoryIntent,
            9 => Self::DirectoryComplete,
            10 => Self::Cancel,
            11 => Self::RollbackSeal,
            _ => return Err(Error::Malformed),
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedEntry {
    Pending,
    ApplyUnresolved,
    Applied,
    UndoUnresolved,
    Undone,
    Unchanged,
}
#[derive(Debug)]
pub struct Observation {
    pub entries: Vec<ObservedEntry>,
    pub directories_cleaned: Vec<bool>,
    pub cancelled: bool,
    /// Recorded completion only; never filesystem verification or permission.
    pub rollback_recorded: bool,
}

/// Fixed frame: magic[4], version LE u16, binding[32], index LE u64,
/// fact u8, predecessor frame digest[32], SHA256(all previous 79 bytes)[32].
/// Global facts use index u64::MAX. Duplicate frames are exact byte duplicates.
/// Any incomplete suffix rejects the entire materialization (no success result).
pub fn materialize(base: &Authority, bytes: &[u8], max_bytes: usize) -> Result<Observation, Error> {
    if bytes.len() > max_bytes {
        return Err(Error::Limit);
    }
    if !bytes.len().is_multiple_of(FRAME_BYTES) {
        return Err(Error::Truncated);
    }
    let mut result = Observation {
        entries: base
            .transaction
            .entries
            .iter()
            .map(|e| match e.state {
                EntryState::Planned | EntryState::PreflightPassed => ObservedEntry::Pending,
                EntryState::Applying | EntryState::ApplyFailed => ObservedEntry::ApplyUnresolved,
                EntryState::Applied => ObservedEntry::Applied,
                EntryState::RollingBack | EntryState::RollbackFailed => {
                    ObservedEntry::UndoUnresolved
                }
                EntryState::RolledBack => ObservedEntry::Undone,
                EntryState::Skipped => ObservedEntry::Unchanged,
            })
            .collect(),
        directories_cleaned: vec![false; base.transaction.created_directories.len()],
        cancelled: false,
        rollback_recorded: false,
    };
    let mut previous = [0; 32];
    let mut seen: BTreeMap<(u64, u8), Vec<u8>> = BTreeMap::new();
    let mut directory_intents = vec![false; result.directories_cleaned.len()];
    for frame in bytes.chunks_exact(FRAME_BYTES) {
        if &frame[..4] != MAGIC || frame[4..6] != VERSION.to_le_bytes() {
            return Err(Error::Malformed);
        }
        if frame[6..38] != base.binding {
            return Err(Error::Binding);
        }
        let digest: [u8; 32] = Sha256::digest(&frame[..79]).into();
        if frame[79..] != digest {
            return Err(Error::Malformed);
        }
        let index = u64::from_le_bytes(frame[38..46].try_into().unwrap());
        let fact = Fact::from_tag(frame[46])?;
        if let Some(old) = seen.get(&(index, frame[46])) {
            if old != frame {
                return Err(Error::Conflict);
            }
            continue;
        }
        if result.rollback_recorded || frame[47..79] != previous {
            return Err(Error::Transition);
        }
        match fact {
            Fact::Cancel | Fact::RollbackSeal => {
                if index != u64::MAX {
                    return Err(Error::Binding);
                }
                if fact == Fact::Cancel {
                    result.cancelled = true;
                } else {
                    if !result
                        .entries
                        .iter()
                        .all(|s| matches!(s, ObservedEntry::Undone | ObservedEntry::Unchanged))
                        || result.directories_cleaned.contains(&false)
                    {
                        return Err(Error::Transition);
                    }
                    result.rollback_recorded = true;
                }
            }
            Fact::DirectoryIntent | Fact::DirectoryComplete => {
                let i = usize::try_from(index).map_err(|_| Error::Limit)?;
                if !result
                    .entries
                    .iter()
                    .all(|s| matches!(s, ObservedEntry::Undone | ObservedEntry::Unchanged))
                {
                    return Err(Error::Transition);
                }
                let intent = directory_intents.get_mut(i).ok_or(Error::Binding)?;
                if fact == Fact::DirectoryIntent {
                    *intent = true;
                } else {
                    if !*intent {
                        return Err(Error::Transition);
                    }
                    result.directories_cleaned[i] = true;
                }
            }
            _ => {
                let i = usize::try_from(index).map_err(|_| Error::Limit)?;
                let state = result.entries.get_mut(i).ok_or(Error::Binding)?;
                use ObservedEntry::*;
                *state = match (*state, fact) {
                    (Pending, Fact::ApplyIntent) if !result.cancelled => ApplyUnresolved,
                    (Pending, Fact::NoMutation) => Unchanged,
                    (ApplyUnresolved, Fact::NoMutation) => Unchanged,
                    (ApplyUnresolved, Fact::ApplyComplete) => Applied,
                    (ApplyUnresolved, Fact::ApplyFailure) => ApplyUnresolved,
                    (Applied, Fact::UndoIntent) => UndoUnresolved,
                    // Undo of an Apply whose completion was never recorded.
                    // The exact runtime record states what was found.
                    (ApplyUnresolved, Fact::UndoIntent) => UndoUnresolved,
                    (UndoUnresolved, Fact::UndoFailure) => UndoUnresolved,
                    (UndoUnresolved, Fact::UndoComplete) => Undone,
                    _ => return Err(Error::Transition),
                };
            }
        }
        previous = digest;
        seen.insert((index, frame[46]), frame.to_vec());
    }
    Ok(result)
}

/// Maximum distinct slots: seven entry facts (including mutually exclusive
/// branches), two per directory, cancellation and final seal. Overestimation
/// is intentional. Duplicate retries reserve no additional slots.
#[derive(Debug, PartialEq, Eq)]
pub struct Budget {
    pub slots: u64,
    pub logical_bytes: u64,
    pub accounted_bytes: u64,
}
pub fn budget(base_bytes: u64, entries: u64, directories: u64, cap: u64) -> Result<Budget, Error> {
    fn add(a: u64, b: u64) -> Result<u64, Error> {
        a.checked_add(b).ok_or(Error::Overflow)
    }
    fn mul(a: u64, b: u64) -> Result<u64, Error> {
        a.checked_mul(b).ok_or(Error::Overflow)
    }
    let slots = add(add(mul(entries, 7)?, mul(directories, 2)?)?, 2)?;
    let logical_bytes = add(base_bytes, mul(slots, FRAME_BYTES as u64)?)?;
    // Proposed single preallocated container: base + fixed slots, one reserve
    // for staging base and four metadata/activation objects. NOT a reservation.
    let rounded = mul(add(logical_bytes, 4095)? / 4096, 4096)?;
    let accounted_bytes = add(add(rounded, base_bytes)?, 64 * 1024)?;
    if accounted_bytes > cap {
        return Err(Error::Limit);
    }
    Ok(Budget {
        slots,
        logical_bytes,
        accounted_bytes,
    })
}
#[cfg(test)]
mod tests;

// Compact readers are always installed. The explicit core evaluation
// controller alone can install an admitted writer; its switch defaults OFF.
#[cfg(all(test, target_os = "linux"))]
mod runtime;
#[cfg(target_os = "linux")]
mod storage;
#[cfg(target_os = "linux")]
pub(super) use storage::integration::checkpoint as enrolled_checkpoint;
#[cfg(target_os = "linux")]
pub(super) use storage::{discovery_path, is_enrolled, read_with_history as enrolled_history_read};

#[cfg(target_os = "linux")]
pub(crate) use storage::{discovery_bounded, read_bounded};

#[cfg(target_os = "linux")]
pub(crate) use storage::integration::shared_engine::{
    apply_entry as fd_apply_entry, prepare_apply_entry as fd_prepare_apply_entry, undo as fd_undo,
};

/// Explicit evaluation controller. Default application callers do not install it.
#[cfg(target_os = "linux")]
pub use storage::integration::{controller, deployment};
