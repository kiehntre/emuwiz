//! An explicit re-check of one CUE/BIN disc whose saved whole-disc
//! verification is no longer current - typically after a network or FUSE
//! drive was remounted and its files got new device/inode numbers.
//!
//! Nothing here runs by itself. The existing audit re-hashes the sheet and
//! every file it references (the hash cache is off, so nothing is reused on
//! the strength of a name, size or time) against the catalogue the disc was
//! verified with. Verification is renewed only when that same catalogue
//! version again verifies the whole disc as the same entry; otherwise the
//! saved evidence is left exactly as it was and the reason is returned.
use super::*;
use crate::dat::sources::DatSourceKind;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_targets};
use crate::dat::sources::audit_targets::{AuditTargets, catalogue_audit_targets};
use crate::safe_read::TrustedRoots;
use std::sync::atomic::AtomicBool;

/// The catalogue a disc is re-checked against: the configured source the
/// saved verification names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueDiscCatalogue {
    pub source_id: String,
    pub display_name: String,
    pub dat_path: PathBuf,
    pub kind: DatSourceKind,
}

/// What an explicit re-check concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueDiscRevalidation {
    /// Every file re-hashed to the same catalogue entry and version; the
    /// verification was saved again with fresh file fingerprints.
    Verified { entry: String },
    /// The re-check ran but did not verify the same entry; nothing was saved.
    NotVerified { reason: String },
    /// The re-check could not run; nothing was saved.
    Refused { reason: String },
}

impl Database {
    /// Re-checks one catalogued CUE disc against the catalogue its saved
    /// verification names. Bounded to the sheet and the files it references.
    pub fn revalidate_cue_disc(
        &mut self,
        archive_id: i64,
        catalogue: &CueDiscCatalogue,
        trusted: &TrustedRoots,
        cancel: &AtomicBool,
    ) -> Result<CueDiscRevalidation> {
        let refused = |reason: &str| {
            Ok(CueDiscRevalidation::Refused {
                reason: reason.to_string(),
            })
        };
        let Some(saved) = self.library_dat_identity_for_item(archive_id, &catalogue.source_id)?
        else {
            return refused("this disc has no saved verification from that catalogue");
        };
        let Some(entry) = saved.canonical.canonical_dat_name.clone() else {
            return refused("the saved verification names no catalogue entry");
        };
        if saved.source.catalogue_sha256.is_none() {
            return refused(
                "the saved verification has no exact catalogue digest; run a normal audit",
            );
        }
        if saved.disc_members.is_empty()
            || !matches!(
                saved.verification_state,
                crate::dat::library_identity_summary::DatVerificationState::VerifiedSingleMatch { .. }
            )
        {
            return refused("there is no saved whole-disc verification to re-check; run an audit");
        }
        let Some(archive) = self
            .load_archives()?
            .into_iter()
            .find(|archive| archive.id == archive_id)
        else {
            return refused("the disc is no longer in the library");
        };
        let cue = archive.absolute_path;
        if let Err(refusal) = crate::ingestion::ps1_cue_disc::inspect_ps1_cue_disc(&cue) {
            return Ok(CueDiscRevalidation::Refused {
                reason: refusal.to_string(),
            });
        }
        let Some(folder) = cue.parent().map(Path::to_path_buf) else {
            return refused("the disc has no folder");
        };
        let request = DatAuditRequest {
            source_id: catalogue.source_id.clone(),
            source_display_name: catalogue.display_name.clone(),
            dat_path: catalogue.dat_path.clone(),
            dat_kind: catalogue.kind,
            scan_root: folder.clone(),
            limits: crate::dat::limits::DatLimits::default(),
            policy: None,
            platform: archive.platform.clone(),
        };
        let targets = AuditTargets::Catalogue(catalogue_audit_targets([cue.clone()], &folder));
        let outcome = match run_dat_audit_with_targets(
            &request,
            trusted,
            cancel,
            &|_| {},
            AuditCacheConfig::Disabled,
            &targets,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                return Ok(CueDiscRevalidation::Refused {
                    reason: format!("the re-check could not complete: {error:?}"),
                });
            }
        };
        if saved.source.catalogue_sha256.is_some()
            && saved.source.catalogue_sha256 != outcome.catalogue_sha256
        {
            return refused(
                "the installed catalogue has different content from the one that verified this disc; run a normal audit",
            );
        }
        if outcome.catalogue_version != saved.source.source_revision {
            return refused(
                "the installed catalogue is a different version from the one that verified this disc; run a normal audit",
            );
        }
        let cue_key = cue.to_string_lossy();
        let Some(disc) = outcome
            .loose_discs
            .iter()
            .find(|disc| disc.cue_path == cue_key)
        else {
            return refused("the disc's files were not all re-checked");
        };
        if disc.verified_game.as_deref() != Some(entry.as_str()) {
            return Ok(CueDiscRevalidation::NotVerified {
                reason: disc
                    .detail
                    .clone()
                    .unwrap_or_else(|| format!("the disc no longer verifies as {entry}")),
            });
        }
        let persisted = self.persist_library_dat_identities_from_audit(&outcome)?;
        if persisted.refused.is_some() || persisted.inserted + persisted.updated != 1 {
            return refused("the renewed verification could not be saved");
        }
        Ok(CueDiscRevalidation::Verified { entry })
    }
}
