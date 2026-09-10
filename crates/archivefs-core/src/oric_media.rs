//! Read-only Oric media evidence and freshness binding; no launch authority.
//! Reuses safe reads, hash streaming and the existing filesystem identity
//! primitive. No emulator/profile or release database is added.

use crate::content_detector::{ContentDetectionOutcome, ContentDetector, ContentDiagnostic};
use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};
use crate::dat::archive::{ArchiveMemberHashes, hash::hash_member_stream};
use crate::disk_format::oric::{OricMfmLayout, parse_oric_mfm};
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::oric_tape::{OricTapeObservation, has_oric_leader, parse_oric_tap};
use crate::safe_read::{TrustedRoots, open_bounded_read};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_ORIC_MEDIA_BYTES: usize = crate::tape_analysis::MAX_ANALYSIS_BYTES;
pub const ORIC_TAP_EVIDENCE: &str = "Oric standard TAP";
pub const ORIC_DSK_EVIDENCE: &str = "Oric MFM_DISK geometry 1";

/// Candidate dispatch only; bytes must still pass the complete observer.
pub fn candidate_extension(extension: &str) -> bool {
    extension.eq_ignore_ascii_case("tap") || extension.eq_ignore_ascii_case("dsk")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OricMachineCompatibility {
    /// These structures cannot prove Oric-1, Atmos or Telestrat compatibility.
    Undetermined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OricMediaObservation {
    Tape(OricTapeObservation),
    Disk(OricMfmLayout),
}

impl OricMediaObservation {
    pub fn machine_compatibility(&self) -> OricMachineCompatibility {
        OricMachineCompatibility::Undetermined
    }
    pub fn evidence(&self) -> Vec<ContentEvidence> {
        let (kind, value, detail) = match self {
            Self::Tape(tape) => (
                ContentEvidenceKind::TapeFormat,
                ORIC_TAP_EVIDENCE,
                format!(
                    "{} complete Oric TAP segment(s); sync, bounded headers, names and inclusive address extents checked through EOF; no stored checksum or release identity",
                    tape.blocks.len()
                ),
            ),
            Self::Disk(disk) => (
                ContentEvidenceKind::DiskFormat,
                ORIC_DSK_EVIDENCE,
                format!(
                    "MFM_DISK geometry 1, {} side(s), {} tracks/side, {} ordinary 256-byte sectors/track; complete extents and ID/data CRCs checked; no filesystem, machine or release claim",
                    disk.sides, disk.tracks_per_side, disk.sectors_per_track
                ),
            ),
        };
        vec![ContentEvidence::new(
            kind,
            value,
            ContentEvidenceConfidence::Strong,
            detail,
        )]
    }
}

pub fn is_oric_candidate(bytes: &[u8]) -> bool {
    has_oric_leader(bytes) || bytes.starts_with(b"MFM_DISK")
}

pub fn observe_oric_media(bytes: &[u8]) -> Result<OricMediaObservation, String> {
    if bytes.len() > MAX_ORIC_MEDIA_BYTES {
        return Err("Oric media exceeds inspection limit".into());
    }
    if bytes.starts_with(b"MFM_DISK") {
        parse_oric_mfm(bytes)
            .map(OricMediaObservation::Disk)
            .map_err(|e| e.detail())
    } else {
        parse_oric_tap(bytes)
            .map(OricMediaObservation::Tape)
            .map_err(|e| format!("{e:?}"))
    }
}

pub struct OricMediaDetector;
impl ContentDetector for OricMediaDetector {
    fn id(&self) -> &'static str {
        "oric_media"
    }
    fn requires_complete_input(&self) -> bool {
        true
    }
    fn complete_input_limit(&self, prefix: &[u8]) -> Option<usize> {
        is_oric_candidate(prefix).then_some(MAX_ORIC_MEDIA_BYTES)
    }
    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        if !is_oric_candidate(data) {
            return ContentDetectionOutcome::NotRecognized;
        }
        match observe_oric_media(data) {
            Ok(observation) => ContentDetectionOutcome::Recognized {
                evidence: observation.evidence(),
            },
            Err(message) => ContentDetectionOutcome::Malformed {
                evidence: Vec::new(),
                diagnostic: ContentDiagnostic {
                    detector_id: self.id(),
                    category: "unsupported_or_malformed",
                    message,
                },
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OricMediaInspection {
    path: PathBuf,
    identity: CapturedFileIdentity,
    hashes: ArchiveMemberHashes,
    observation: OricMediaObservation,
}

impl OricMediaInspection {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn hashes(&self) -> &ArchiveMemberHashes {
        &self.hashes
    }
    pub fn observation(&self) -> &OricMediaObservation {
        &self.observation
    }
    pub fn revalidate(&self, trusted: &TrustedRoots, cancel: &AtomicBool) -> Result<(), String> {
        let now = inspect_oric_file(&self.path, trusted, cancel)?;
        if now != *self {
            return Err("Oric media source changed; discard previous evidence".into());
        }
        Ok(())
    }
}

/// Explicit bounded inspection, independent of suffix and platform hints.
/// Hashes cover the complete parsed bytes. Revalidation recomputes all hashes;
/// unchanged metadata is never sufficient to reuse an observation.
pub fn inspect_oric_file(
    path: &Path,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
) -> Result<OricMediaInspection, String> {
    let safe = open_bounded_read(path, trusted).map_err(|e| e.detail())?;
    if safe.len() > MAX_ORIC_MEDIA_BYTES as u64 {
        return Err("Oric media exceeds inspection limit".into());
    }
    let mut file = safe.into_file();
    let identity = CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?);
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Oric inspection cancelled".into());
        }
        let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > MAX_ORIC_MEDIA_BYTES {
            return Err("Oric media exceeds inspection limit".into());
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let observation = observe_oric_media(&bytes)?;
    let hashes = hash_member_stream(bytes.as_slice(), MAX_ORIC_MEDIA_BYTES as u64, cancel)
        .map_err(|e| format!("Oric hashing failed: {e:?}"))?
        .hashes;
    let after = CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?);
    let current = open_bounded_read(path, trusted)
        .map_err(|e| e.detail())?
        .into_file();
    let current = CapturedFileIdentity::capture(&current.metadata().map_err(|e| e.to_string())?);
    if identity != after || identity != current || bytes.len() as u64 != identity.size {
        return Err("Oric media changed during inspection".into());
    }
    Ok(OricMediaInspection {
        path: path.to_path_buf(),
        identity,
        hashes,
        observation,
    })
}
