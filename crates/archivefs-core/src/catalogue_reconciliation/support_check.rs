//! Explicit, single-file content check for a file Library Files Check set
//! aside as support material.
//!
//! A person picks one support file and asks EmuWiz to compare its content with
//! the installed DAT catalogues. Only that file is read, through the same
//! pinned, symlink-refusing [`hash_files`] path as a move confirmation, so a
//! file that changes while it is read yields no checksum. Matching is the
//! existing [`audit_files`] comparison. Nothing is reclassified, imported,
//! renamed or written: the result is information for the person to review.
use super::hashing::hash_files;
use super::{StrongHash, StrongHashAlgorithm};
use crate::dat::audit::{AuditVerdict, KnownFileEvidence, audit_files};
use crate::dat::index::DatIndex;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// One installed catalogue the caller resolved and is willing to compare with.
pub struct CatalogueToCheck<'a> {
    /// What the person calls it (the catalogue's display name).
    pub label: String,
    /// Where it came from (user-registered, EmuWiz-managed, ...).
    pub provenance: String,
    pub index: &'a DatIndex,
}

/// One catalogue whose entries the file's content matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueMatch {
    pub catalogue: String,
    pub provenance: String,
    /// `Exact` or `ExactMultipleCandidates`; CRC-only verdicts never appear,
    /// because a support check only computes cryptographic checksums.
    pub verdict: AuditVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportCheckOutcome {
    /// The file could not be read through the pinned root.
    Unreadable,
    /// The file is no longer the one Library Files Check listed (size or
    /// identity changed, or it changed while being read). Run the check again.
    ChangedSinceListed,
    /// The check was cancelled before it finished.
    Cancelled,
    /// No usable catalogue is installed, so nothing was compared.
    NoCatalogueToCompare,
    /// Checked every listed catalogue; none has this content.
    NoMatchInCheckedCatalogues,
    /// Exactly one catalogue entry, in total, has this exact content.
    ExactMatch,
    /// The content matches more than one entry or catalogue.
    AmbiguousMatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportContentCheck {
    pub path: PathBuf,
    pub outcome: SupportCheckOutcome,
    /// The checksums actually computed for this file (empty unless read).
    pub hashes: Vec<StrongHash>,
    pub matches: Vec<CatalogueMatch>,
    /// Every catalogue the content was compared with.
    pub checked: Vec<String>,
    /// File bytes read by this check: only the selected file's.
    pub bytes_read: u64,
}

/// Hashes exactly `path` (beneath one of `roots`) and compares it with each
/// catalogue. `listed_size` is the size Library Files Check recorded: a file
/// whose size differs is reported as changed rather than checked.
pub fn check_support_file_content(
    roots: &[PathBuf],
    path: &Path,
    listed_size: u64,
    catalogues: &[CatalogueToCheck<'_>],
    cancel: &AtomicBool,
) -> SupportContentCheck {
    let mut check = SupportContentCheck {
        path: path.to_path_buf(),
        outcome: SupportCheckOutcome::Unreadable,
        hashes: Vec::new(),
        matches: Vec::new(),
        checked: Vec::new(),
        bytes_read: 0,
    };
    // Display-only snapshot to explain a refusal; the pinned read in
    // `hash_files` is what actually binds the checksum to the file.
    let snapshot = || {
        std::fs::symlink_metadata(path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|m| (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
    };
    let before = snapshot();
    if before.is_some_and(|(_, _, len, _, _)| len != listed_size) {
        // Not the file that was listed: do not read it at all.
        check.outcome = SupportCheckOutcome::ChangedSinceListed;
        return check;
    }
    let hashed = hash_files(roots, &[path.to_path_buf()], u64::MAX, cancel);
    check.bytes_read = hashed.bytes_read;
    if hashed.cancelled {
        check.outcome = SupportCheckOutcome::Cancelled;
        return check;
    }
    let Some(hashes) = hashed.hashes.get(path) else {
        // `hash_files` also refuses a file that moved while it was read.
        let after = snapshot();
        check.outcome = if before.is_some() && after.is_some() && before != after {
            SupportCheckOutcome::ChangedSinceListed
        } else {
            SupportCheckOutcome::Unreadable
        };
        return check;
    };
    if hashed.bytes_read != listed_size {
        check.outcome = SupportCheckOutcome::ChangedSinceListed;
        return check;
    }
    check.hashes = hashes.clone();
    if catalogues.is_empty() {
        check.outcome = SupportCheckOutcome::NoCatalogueToCompare;
        return check;
    }
    let mut evidence = KnownFileEvidence::new(
        path.to_string_lossy().into_owned(),
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
    .with_size(listed_size);
    for hash in hashes {
        evidence = match hash.algorithm {
            StrongHashAlgorithm::Sha1 => evidence.with_sha1(hash.hex.clone()),
            StrongHashAlgorithm::Sha256 => evidence.with_sha256(hash.hex.clone()),
        };
    }
    let mut entries = 0usize;
    for catalogue in catalogues {
        check.checked.push(catalogue.label.clone());
        let report = audit_files(std::slice::from_ref(&evidence), catalogue.index);
        let Some(entry) = report.entries.into_iter().next() else {
            continue;
        };
        match &entry.verdict {
            AuditVerdict::Exact { .. } => entries += 1,
            AuditVerdict::ExactMultipleCandidates { count, .. } => entries += count,
            _ => continue,
        }
        check.matches.push(CatalogueMatch {
            catalogue: catalogue.label.clone(),
            provenance: catalogue.provenance.clone(),
            verdict: entry.verdict,
        });
    }
    check.outcome = match entries {
        0 => SupportCheckOutcome::NoMatchInCheckedCatalogues,
        1 => SupportCheckOutcome::ExactMatch,
        _ => SupportCheckOutcome::AmbiguousMatch,
    };
    check
}

#[cfg(test)]
#[path = "support_check_tests.rs"]
mod tests;
