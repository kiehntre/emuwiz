//! The explicit, later operation that gives physical files strong hashes.
//!
//! Reconciliation never reads file contents. A caller that wants to settle
//! [`super::MoveProof::NeedsHashing`] candidates passes just those paths here,
//! with a byte budget and a cancel flag. Reads go through the same pinned,
//! symlink-refusing root handle Catalogue Health uses, and a file that changes
//! while it is read yields no hash.
use super::{StrongHash, StrongHashAlgorithm};
use crate::catalogue_health::BoundRoot;
use crate::emulator_environment::FsProbe;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HashOutcome {
    pub hashes: BTreeMap<PathBuf, Vec<StrongHash>>,
    pub unreadable: Vec<PathBuf>,
    /// Not attempted: reading them would exceed the budget.
    pub over_budget: Vec<PathBuf>,
    pub bytes_read: u64,
    pub cancelled: bool,
}

/// Hashes `requests` (each a file beneath one of `roots`), up to `max_bytes` in
/// total. `roots` pairs a configured root path with each request's root.
pub fn hash_files(
    roots: &[PathBuf],
    requests: &[PathBuf],
    max_bytes: u64,
    cancel: &AtomicBool,
) -> HashOutcome {
    let mut outcome = HashOutcome::default();
    let bound: Vec<_> = roots
        .iter()
        .filter_map(|root| BoundRoot::open(root).map(|bound| (root, bound)))
        .collect();
    let mut requests: Vec<&PathBuf> = requests.iter().collect();
    requests.sort();
    requests.dedup();
    for path in requests {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        let Some((_, root)) = bound
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
        else {
            outcome.unreadable.push(path.clone());
            continue;
        };
        let expected = root.probe(path, false);
        let (FsProbe::PresentFile, Some(size)) = (expected.probe, expected.size) else {
            outcome.unreadable.push(path.clone());
            continue;
        };
        if outcome.bytes_read.saturating_add(size) > max_bytes {
            outcome.over_budget.push(path.clone());
            continue;
        }
        match digest(root, path, &expected, size, cancel) {
            Digest3::Done(hashes) => {
                outcome.bytes_read += size;
                outcome.hashes.insert(path.clone(), hashes);
            }
            Digest3::Cancelled => {
                outcome.cancelled = true;
                break;
            }
            Digest3::Failed => outcome.unreadable.push(path.clone()),
        }
    }
    outcome
}

enum Digest3 {
    Done(Vec<StrongHash>),
    Cancelled,
    Failed,
}

fn digest(
    root: &BoundRoot,
    path: &Path,
    expected: &crate::catalogue_health::PathObservation,
    size: u64,
    cancel: &AtomicBool,
) -> Digest3 {
    let Some(mut file) = root.read_file(path, expected) else {
        return Digest3::Failed;
    };
    let (mut sha1, mut sha256) = (Sha1::new(), Sha256::new());
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Digest3::Cancelled;
        }
        let Ok(read) = file.read(&mut buffer) else {
            return Digest3::Failed;
        };
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > size {
            return Digest3::Failed;
        }
        sha1.update(&buffer[..read]);
        sha256.update(&buffer[..read]);
    }
    // A file that changed size or identity while being read has no hash.
    if total != size || root.probe(path, false) != *expected {
        return Digest3::Failed;
    }
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let hashes = [
        StrongHash::new(StrongHashAlgorithm::Sha1, &hex(&sha1.finalize())),
        StrongHash::new(StrongHashAlgorithm::Sha256, &hex(&sha256.finalize())),
    ]
    .into_iter()
    .flatten()
    .collect();
    Digest3::Done(hashes)
}
