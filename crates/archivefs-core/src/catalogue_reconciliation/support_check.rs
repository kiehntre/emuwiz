//! Explicit, single-file content check for a file Library Files Check set
//! aside as support material.
//!
//! A person picks one support file and asks EmuWiz to compare its content with
//! the installed DAT catalogues. Only that file is read, through the same
//! pinned, symlink-refusing reader as a move confirmation, bound to the
//! identity recorded when the file was listed, so a replaced file, or one that
//! changes while it is read, yields no checksum. Matching is the
//! existing [`audit_files`] comparison. Nothing is reclassified, imported,
//! renamed or written: the result is information for the person to review.
use super::hashing::{Digest3, digest};
use super::{FileIdentity, StrongHash, StrongHashAlgorithm};
use crate::catalogue_health::BoundRoot;
use crate::dat::audit::{AuditVerdict, KnownFileEvidence, audit_files};
use crate::dat::index::DatIndex;
use crate::emulator_environment::FsProbe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

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
/// catalogue, but only if it is still the very file Library Files Check
/// listed: same device, inode, size and modification time. The file is
/// pinned through the bound root; the opened descriptor must carry the listed
/// identity, and the path must still carry it after the last byte is read.
/// Anything else is [`SupportCheckOutcome::ChangedSinceListed`] and no
/// checksum is reported.
pub fn check_support_file_content(
    roots: &[PathBuf],
    path: &Path,
    listed: &FileIdentity,
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
    if cancel.load(Ordering::Relaxed) {
        check.outcome = SupportCheckOutcome::Cancelled;
        return check;
    }
    // The most specific configured root containing the file, opened without
    // following symlinks anywhere beneath it.
    let Some(root) = roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .and_then(|root| BoundRoot::open(root))
    else {
        return check;
    };
    let observed = root.probe(path, false);
    match observed.probe {
        FsProbe::PresentFile => {}
        // A symlink or a different kind of object now at the path.
        FsProbe::Symlink | FsProbe::WrongType => {
            check.outcome = SupportCheckOutcome::ChangedSinceListed;
            return check;
        }
        _ => return check,
    }
    let same = observed.binding.last() == Some(&(listed.device, listed.inode))
        && observed.size == Some(listed.size)
        && observed.modified == Some(listed.modified)
        && observed.modified_ns == Some(listed.modified_ns);
    if !same {
        // Not demonstrably the listed file: not read at all.
        check.outcome = SupportCheckOutcome::ChangedSinceListed;
        return check;
    }
    let hashes = match digest(&root, path, &observed, listed.size, cancel) {
        Digest3::Done(hashes) => hashes,
        Digest3::Cancelled => {
            check.outcome = SupportCheckOutcome::Cancelled;
            return check;
        }
        // The descriptor no longer matched the listing, or the path changed
        // while it was read, or reading failed.
        Digest3::Failed => {
            check.outcome = if root.probe(path, false) == observed {
                SupportCheckOutcome::Unreadable
            } else {
                SupportCheckOutcome::ChangedSinceListed
            };
            return check;
        }
    };
    check.bytes_read = listed.size;
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
    .with_size(listed.size);
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
