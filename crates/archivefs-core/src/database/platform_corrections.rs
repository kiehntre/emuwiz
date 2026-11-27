//! Platform Corrections: preview, apply and Undo for corrections of systems
//! the scanner guessed (owner decisions 2026-11-26/27).
//!
//! A guessed system (folder, file type, path, archive layout, RomM) that a
//! catalogue for one other system matches exactly (single SHA-1/SHA-256 game,
//! complete check, no conflicting evidence) can be corrected, in bulk, only
//! after a preview the person reviews, and every correction can be undone.
//!
//! * **Preview** (read-only) sorts candidates into Ready (preselected),
//!   Needs review (verified, header, chosen-by-hand or unknown origin; MD5
//!   only; anything that cannot be re-checked) and Ambiguous (never
//!   applicable). Each candidate binds the current assignment row, the file's
//!   on-disk identity and the existing result row; the catalogue is bound by
//!   the SHA-256 of its bytes.
//! * **Apply** re-checks every binding inside one immediate transaction,
//!   makes the changes, writes the complete receipt and fsyncs it (file and
//!   folder) **before** the commit. A committed correction therefore always
//!   has a durable receipt; a receipt left pending by a crash is resolved
//!   against the database when receipts are listed (applied if its rows are
//!   there, void if not).
//! * **Undo** (one game or the batch) refuses a game whose system or result
//!   changed after the correction, so a later change is never overwritten.
//!   It deletes exactly the applied assignment row and makes the previous
//!   one current again (the organisation precedent), restores the previous
//!   result and the scanner's cached system.
//!
//! No game file is ever moved, renamed, written or deleted: the only files
//! written are the receipts beside the database. No schema change: the
//! receipt follows the Forget-missing precedent.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    Database, PlatformProvenanceClass, VERIFIED_DAT_PLATFORM_SOURCE, db_error, now_utc_string,
    persist_library_dat_identity_row,
};
use crate::dat::identity::{CatalogueSystems, catalogue_systems, identifies_system};
use crate::dat::library_identity_summary::{
    DatAuditCompleteness, DatVerificationState, PersistedLibraryDatIdentity,
};
use crate::{ArchiveFsError, Result};

/// A preview older than this must be made again before it is applied.
pub const PLATFORM_CORRECTION_PREVIEW_MAX_AGE: std::time::Duration =
    std::time::Duration::from_secs(60 * 60);

/// Which part of the preview a candidate belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlatformCorrectionGroup {
    /// A strong exact match against a guessed system: preselected, can be
    /// applied in bulk.
    Ready,
    /// Real evidence, but EmuWiz will not change this system by itself
    /// (verified, read from the header, chosen by hand, of unknown origin,
    /// MD5 only, or not re-checkable). The person decides, on the game.
    NeedsReview,
    /// The evidence does not point at one game on one system. Never applied.
    Ambiguous,
}

/// The assignment row a candidate was previewed against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentBinding {
    pub id: i64,
    pub platform: String,
    pub source: String,
}

/// The game file as it was when the preview was made. A file that changed
/// since is not the file the catalogue matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBinding {
    pub size: u64,
    pub modified_ns: i128,
    pub device: u64,
    pub inode: u64,
}

impl FileBinding {
    fn read(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        Some(Self {
            size: meta.len(),
            modified_ns: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
}

/// The catalogue the evidence came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueBinding {
    pub source_id: String,
    pub display_name: String,
    pub dat_path: String,
    pub revision: Option<String>,
    /// SHA-256 of the catalogue file's bytes; `None` for a folder of
    /// catalogues, which cannot be re-checked as one file.
    pub sha256: Option<String>,
}

/// One proposed correction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformCorrectionCandidate {
    pub archive_id: i64,
    pub path: String,
    /// The catalogue game the file matched.
    pub game_name: Option<String>,
    pub current: AssignmentBinding,
    pub provenance: PlatformProvenanceClass,
    pub proposed_platform: String,
    /// The hash algorithm(s) of the match, as recorded.
    pub algorithm: String,
    pub group: PlatformCorrectionGroup,
    /// Why it is in its group, for the person.
    pub reason: String,
    /// Applying this correction ends the Undo of the Discovery Import that
    /// added this game (its record is marked changed for good).
    pub loses_discovery_import_undo: bool,
    pub file: Option<FileBinding>,
    /// Digest of the result row this catalogue already holds for the game.
    pub existing_result: Option<String>,
    pub(crate) identity: PersistedLibraryDatIdentity,
}

/// A read-only preview of corrections from one completed catalogue check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformCorrectionPreview {
    pub database_path: PathBuf,
    pub created_at: String,
    #[serde(skip, default = "std::time::SystemTime::now")]
    pub(crate) made: std::time::SystemTime,
    pub catalogue: CatalogueBinding,
    pub candidates: Vec<PlatformCorrectionCandidate>,
}

impl PlatformCorrectionPreview {
    pub fn group(
        &self,
        group: PlatformCorrectionGroup,
    ) -> impl Iterator<Item = &PlatformCorrectionCandidate> {
        self.candidates.iter().filter(move |c| c.group == group)
    }

    /// The preselected games: every Ready candidate.
    pub fn preselected(&self) -> Vec<i64> {
        self.group(PlatformCorrectionGroup::Ready)
            .map(|c| c.archive_id)
            .collect()
    }

    /// Identifies this preview's bindings.
    pub fn token(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.database_path.as_os_str().as_bytes());
        hash.update(self.catalogue.source_id.as_bytes());
        hash.update(self.catalogue.sha256.as_deref().unwrap_or("").as_bytes());
        for c in &self.candidates {
            hash.update(c.archive_id.to_le_bytes());
            hash.update(c.current.id.to_le_bytes());
            hash.update(c.proposed_platform.as_bytes());
        }
        hex(&hash.finalize())
    }
}

/// What to do when some selected games changed since the preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhenSomeChanged {
    /// Apply nothing (the default).
    #[default]
    RefuseAll,
    /// Apply the games that still match and list the others.
    ProceedWithTheRest,
}

/// A game that was not corrected or not undone, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformCorrectionRefusal {
    pub archive_id: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformCorrectionsApplied {
    pub receipt_path: Option<PathBuf>,
    pub corrected: Vec<i64>,
    pub refused: Vec<PlatformCorrectionRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformCorrectionsUndone {
    pub undone: Vec<i64>,
    pub refused: Vec<PlatformCorrectionRefusal>,
}

/// A receipt on disk, as resolved against the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformCorrectionReceiptSummary {
    pub receipt_path: PathBuf,
    pub created_at: String,
    pub catalogue: String,
    /// Games whose correction is still in place (Undo is offered).
    pub undoable: Vec<PlatformCorrectionRecordSummary>,
    /// Games already undone.
    pub undone: Vec<i64>,
    /// Games changed since the correction (Undo would overwrite something).
    pub changed: Vec<PlatformCorrectionRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformCorrectionRecordSummary {
    pub archive_id: i64,
    pub path: String,
    pub previous_platform: Option<String>,
    pub previous_source: Option<String>,
    pub corrected_platform: String,
}

pub(crate) const PLATFORM_CORRECTION_STALE: &str = "platform correction preview is out of date";

// --- preview ---------------------------------------------------------------------

impl Database {
    /// Builds the Platform Corrections preview from a completed catalogue
    /// check. Read-only: changes nothing in the database or on disk.
    pub fn preview_platform_corrections(
        &self,
        outcome: &crate::dat::sources::audit_run::DatAuditOutcome,
        generation: u64,
    ) -> Result<PlatformCorrectionPreview> {
        let now = now_utc_string();
        let catalogue = CatalogueBinding {
            source_id: outcome.source_id.clone(),
            display_name: outcome.source_display_name.clone(),
            dat_path: outcome.dat_path.clone(),
            revision: outcome.catalogue_version.clone(),
            sha256: outcome.catalogue_sha256.clone(),
        };
        let mut preview = PlatformCorrectionPreview {
            database_path: self.path.clone(),
            created_at: now.clone(),
            made: std::time::SystemTime::now(),
            catalogue,
            candidates: Vec::new(),
        };
        let Ok(projection) =
            crate::dat::library_identity_projection::project_dat_audit_for_library_identity(
                outcome, &now,
            )
        else {
            return Ok(preview);
        };
        let mut seen = std::collections::HashSet::new();
        for item in &projection.items {
            let Ok(Some(archive_id)) =
                self.find_archive_id_by_absolute_path(Path::new(&item.local_path))
            else {
                continue;
            };
            // One candidate per game: a second projected item for the same
            // archive (members, tracks) never adds a second correction.
            if !seen.insert(archive_id) {
                preview.candidates.retain(|c| c.archive_id != archive_id);
                continue;
            }
            if let Some(candidate) =
                self.classify_correction(archive_id, item, &preview.catalogue, generation)?
            {
                preview.candidates.push(candidate);
            }
        }
        Ok(preview)
    }

    fn classify_correction(
        &self,
        archive_id: i64,
        item: &crate::dat::library_identity_projection::ProjectedLibraryDatIdentity,
        catalogue: &CatalogueBinding,
        generation: u64,
    ) -> Result<Option<PlatformCorrectionCandidate>> {
        use PlatformCorrectionGroup as G;
        // A game with no system is identified by the ordinary save; a
        // preservation recording never gets a system from a match.
        let Some(current) = current_assignment(&self.connection, archive_id)? else {
            return Ok(None);
        };
        if self.is_preservation_recording(archive_id)? {
            return Ok(None);
        }
        let identity = &item.identity;
        let DatVerificationState::VerifiedSingleMatch { algorithm } = &identity.verification_state
        else {
            return Ok(match &identity.verification_state {
                DatVerificationState::AmbiguousMultipleCandidates { .. }
                | DatVerificationState::Conflicting { .. } => match catalogue_systems(&identity.source) {
                    CatalogueSystems::One(platform) if !same_system(&current.platform, &platform) => {
                        Some(self.candidate(
                            archive_id, item, current, platform, String::new(), G::Ambiguous,
                            "the catalogue matched several games or conflicting entries, so it cannot say which game this is".into(),
                        )?)
                    }
                    _ => None,
                },
                _ => None,
            });
        };
        let platform = match catalogue_systems(&identity.source) {
            CatalogueSystems::One(platform) => platform,
            CatalogueSystems::Several(_) | CatalogueSystems::Disagreeing => {
                // Only worth showing when the game is not already on one of them.
                return Ok(Some(self.candidate(
                    archive_id,
                    item,
                    current,
                    String::new(),
                    algorithm.clone(),
                    G::Ambiguous,
                    "the catalogue names several systems, or its header contradicts its assignment, so it cannot say which system this game is for".into(),
                )?))
                .map(|c| c.filter(|c| !c.current.platform.is_empty()));
            }
            CatalogueSystems::ArcadeMachines | CatalogueSystems::Unmapped => return Ok(None),
        };
        if same_system(&current.platform, &platform) {
            return Ok(None);
        }
        let strong = identifies_system(identity);
        let md5_only = !strong
            && identity.completeness == DatAuditCompleteness::Exhaustive
            && identity.ambiguous_candidates.is_empty()
            && algorithm.split('/').any(|part| part == "MD5")
            && algorithm
                .split('/')
                .all(|part| matches!(part, "MD5" | "SHA-1" | "SHA-256"));
        if !strong && !md5_only {
            // CRC, filename-only, partial or several-game evidence never
            // proposes a system.
            return Ok(
                if identity.ambiguous_candidates.is_empty()
                    && identity.completeness == DatAuditCompleteness::Exhaustive
                {
                    None
                } else {
                    Some(self.candidate(
                    archive_id, item, current, platform, algorithm.clone(), G::Ambiguous,
                    "the match is incomplete or names several games, so it cannot identify the system".into(),
                )?)
                },
            );
        }
        let provenance = PlatformProvenanceClass::of(&current.source);
        let (group, reason) = if md5_only {
            (
                G::NeedsReview,
                "matched by MD5 only: a suggestion. If it is right, choose the system yourself on the game's page".to_string(),
            )
        } else {
            match provenance {
                PlatformProvenanceClass::Inferred => {
                    if self.resolver_conflicts(archive_id, &platform, generation)? {
                        (
                            G::Ambiguous,
                            "other evidence about this game disagrees with the catalogue".to_string(),
                        )
                    } else if catalogue.sha256.is_none() {
                        (
                            G::NeedsReview,
                            "this catalogue is a folder of files, so it cannot be re-checked just before the correction".to_string(),
                        )
                    } else if FileBinding::read(Path::new(&item.local_path)).is_none() {
                        (
                            G::NeedsReview,
                            "the game file cannot be read now, so it cannot be re-checked just before the correction".to_string(),
                        )
                    } else {
                        (
                            G::Ready,
                            format!(
                                "the system was guessed by the scanner ({}); the catalogue matched this file exactly",
                                current.source
                            ),
                        )
                    }
                }
                PlatformProvenanceClass::Person => (
                    G::NeedsReview,
                    "you chose this system; EmuWiz never changes it. If the catalogue is right, choose the system yourself on the game's page".to_string(),
                ),
                PlatformProvenanceClass::Verified => (
                    G::NeedsReview,
                    "another catalogue already verified this system: investigate before changing anything".to_string(),
                ),
                PlatformProvenanceClass::Header => (
                    G::NeedsReview,
                    "this system was read from the game's own header: investigate before changing anything".to_string(),
                ),
                PlatformProvenanceClass::Unknown => (
                    G::NeedsReview,
                    "EmuWiz cannot tell where this system came from, so it is not treated as a guess".to_string(),
                ),
            }
        };
        Ok(Some(self.candidate(
            archive_id,
            item,
            current,
            platform,
            algorithm.clone(),
            group,
            reason,
        )?))
    }

    #[allow(clippy::too_many_arguments)]
    fn candidate(
        &self,
        archive_id: i64,
        item: &crate::dat::library_identity_projection::ProjectedLibraryDatIdentity,
        current: AssignmentBinding,
        proposed_platform: String,
        algorithm: String,
        group: PlatformCorrectionGroup,
        reason: String,
    ) -> Result<PlatformCorrectionCandidate> {
        let source_id = &item.identity.source.source_id;
        Ok(PlatformCorrectionCandidate {
            archive_id,
            path: item.local_path.clone(),
            game_name: item
                .identity
                .matched_entries
                .first()
                .map(|e| e.game_name.clone()),
            provenance: PlatformProvenanceClass::of(&current.source),
            current,
            proposed_platform,
            algorithm,
            group,
            reason,
            loses_discovery_import_undo: discovery_undo_still_open(&self.connection, archive_id)?,
            file: FileBinding::read(Path::new(&item.local_path)),
            existing_result: result_row(&self.connection, archive_id, source_id)?
                .map(|row| row.digest()),
            identity: item.identity.clone(),
        })
    }

    /// Would the platform resolver call this a conflict, given everything
    /// else known about the game?
    fn resolver_conflicts(&self, archive_id: i64, platform: &str, generation: u64) -> Result<bool> {
        let mut evidence = self.current_platform_identity_evidence(archive_id, generation)?;
        evidence.push(crate::platform::identity::PlatformIdentityEvidence {
            platform: platform.to_string(),
            source: crate::platform::identity::PlatformIdentitySource::VerifiedDat,
            confidence: crate::platform::identity::PlatformIdentityConfidence::Verified,
            generation,
            detail: "preview".into(),
        });
        Ok(matches!(
            crate::platform::identity::resolve_platform_identity(generation, evidence),
            crate::platform::identity::PlatformIdentityResolution::Conflict { .. }
        ))
    }
}

fn same_system(current: &str, platform: &str) -> bool {
    crate::canonical_platform_for_alias(current).is_some_and(|current| {
        current == platform || crate::platform::equivalent_platform_ids(platform).contains(&current)
    })
}

// --- apply -----------------------------------------------------------------------

impl Database {
    /// Applies the selected Ready corrections of `preview`, all in one
    /// transaction, after re-checking the catalogue's bytes and every game's
    /// bindings. Only Ready candidates can be applied; anything else is
    /// refused. No game file is touched.
    pub fn apply_platform_corrections(
        &mut self,
        preview: &PlatformCorrectionPreview,
        selected: &[i64],
        when_changed: WhenSomeChanged,
    ) -> Result<PlatformCorrectionsApplied> {
        if preview.database_path != self.path {
            return Err(stale("the preview belongs to another library"));
        }
        if preview
            .made
            .elapsed()
            .is_ok_and(|age| age > PLATFORM_CORRECTION_PREVIEW_MAX_AGE)
        {
            return Err(stale("the preview is more than an hour old"));
        }
        let mut chosen = Vec::new();
        for id in selected {
            let candidate = preview
                .candidates
                .iter()
                .find(|c| c.archive_id == *id)
                .ok_or_else(|| stale("a selected game is not in the preview"))?;
            if candidate.group != PlatformCorrectionGroup::Ready {
                return Err(ArchiveFsError::Database(format!(
                    "game {id} is not a ready correction ({:?}); only strong exact matches against a guessed system can be applied",
                    candidate.group
                )));
            }
            chosen.push(candidate);
        }
        if chosen.is_empty() {
            return Ok(PlatformCorrectionsApplied {
                receipt_path: None,
                corrected: Vec::new(),
                refused: Vec::new(),
            });
        }
        // The catalogue evidence: its bytes are still the ones checked.
        let expected = preview
            .catalogue
            .sha256
            .as_deref()
            .ok_or_else(|| stale("the catalogue cannot be re-checked"))?;
        match fs::read(&preview.catalogue.dat_path) {
            Ok(bytes) if hex(&Sha256::digest(&bytes)) == expected => {}
            Ok(_) => return Err(stale("the catalogue file changed since the preview")),
            Err(error) => {
                return Err(stale(&format!(
                    "the catalogue file cannot be read ({error})"
                )));
            }
        }

        let receipt_dir = receipt_dir(&self.path);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| db_error("failed to start the platform corrections", error))?;
        let mut refused = Vec::new();
        let mut ready = Vec::new();
        for candidate in &chosen {
            match recheck(&tx, candidate)? {
                Ok(()) => ready.push(*candidate),
                Err(reason) => refused.push(PlatformCorrectionRefusal {
                    archive_id: candidate.archive_id,
                    reason,
                }),
            }
        }
        if !refused.is_empty() && when_changed == WhenSomeChanged::RefuseAll {
            return Ok(PlatformCorrectionsApplied {
                receipt_path: None,
                corrected: Vec::new(),
                refused,
            });
        }
        if ready.is_empty() {
            return Ok(PlatformCorrectionsApplied {
                receipt_path: None,
                corrected: Vec::new(),
                refused,
            });
        }
        let stamp = now_utc_string();
        let mut records = Vec::new();
        for candidate in &ready {
            records.push(apply_one(&tx, candidate, &stamp)?);
        }
        let receipt = Receipt {
            version: 1,
            state: ReceiptState::Pending,
            database_path: self.path.clone(),
            created_at: stamp.clone(),
            preview_token: preview.token(),
            catalogue: preview.catalogue.clone(),
            files_untouched: true,
            records,
        };
        // Durable before the commit: a committed correction always has its
        // receipt. If this fails the transaction is rolled back.
        let receipt_path = write_new_receipt(&receipt_dir, &self.path, &receipt)?;
        #[cfg(test)]
        if seams::stop_at(seams::Point::AfterReceiptBeforeCommit) {
            // Simulated crash: the transaction is dropped (rolled back) and
            // the receipt stays pending.
            drop(tx);
            return Err(ArchiveFsError::Database("simulated termination".into()));
        }
        if let Err(error) = tx.commit() {
            // Nothing was applied: the receipt is void. If it cannot be
            // removed it stays pending and resolves to void when listed.
            let _ = fs::remove_file(&receipt_path);
            return Err(db_error("failed to commit the platform corrections", error));
        }
        #[cfg(test)]
        if seams::stop_at(seams::Point::AfterCommitBeforeMark) {
            return Err(ArchiveFsError::Database("simulated termination".into()));
        }
        // Marking it applied is a convenience; a pending receipt whose rows
        // are present is read as applied.
        let _ = update_receipt(&receipt_path, |r| r.state = ReceiptState::Applied);
        Ok(PlatformCorrectionsApplied {
            receipt_path: Some(receipt_path),
            corrected: ready.iter().map(|c| c.archive_id).collect(),
            refused,
        })
    }
}

/// `Ok(Err(reason))` when the game no longer matches its preview.
fn recheck(
    conn: &Connection,
    candidate: &PlatformCorrectionCandidate,
) -> Result<std::result::Result<(), String>> {
    let id = candidate.archive_id;
    let current = current_assignment(conn, id)?;
    if current.as_ref() != Some(&candidate.current) {
        return Ok(Err("its system changed since the preview".into()));
    }
    if PlatformProvenanceClass::of(&candidate.current.source) != PlatformProvenanceClass::Inferred {
        return Ok(Err("its system is no longer a scanner guess".into()));
    }
    let path: Option<Vec<u8>> = conn
        .query_row(
            "SELECT absolute_path_cached FROM archives WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| db_error("failed to read the game for a correction", error))?;
    if path.as_deref() != Some(candidate.path.as_bytes()) {
        return Ok(Err("the game is no longer at the previewed path".into()));
    }
    if candidate.file.is_none() || FileBinding::read(Path::new(&candidate.path)) != candidate.file {
        return Ok(Err(
            "the game file changed since the preview, so it is not the file the catalogue matched"
                .into(),
        ));
    }
    let recording: bool = conn
        .query_row(
            "SELECT archive_kind = 'preservation_recording' FROM archives WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| db_error("failed to read the game's kind", error))?;
    if recording {
        return Ok(Err("the game is a preservation recording".into()));
    }
    let existing = result_row(conn, id, &candidate.identity.source.source_id)?.map(|r| r.digest());
    if existing != candidate.existing_result {
        return Ok(Err("its saved result changed since the preview".into()));
    }
    if !identifies_system(&candidate.identity)
        || catalogue_systems(&candidate.identity.source)
            != CatalogueSystems::One(candidate.proposed_platform.clone())
    {
        return Ok(Err("the evidence no longer identifies one system".into()));
    }
    Ok(Ok(()))
}

fn apply_one(
    conn: &Connection,
    candidate: &PlatformCorrectionCandidate,
    stamp: &str,
) -> Result<ReceiptRecord> {
    let id = candidate.archive_id;
    let previous_result = result_row(conn, id, &candidate.identity.source.source_id)?;
    // The result save clears this game's re-check note for the catalogue;
    // Undo puts it back.
    let previous_recheck = ResultRow::read_from(
        conn,
        "dat_recheck_attempts",
        "archive_id = ?1 AND dat_source_id = ?2",
        &[&id, &candidate.identity.source.source_id],
    )?;
    let retired = conn
        .execute(
            "UPDATE platform_assignments SET is_current = 0 WHERE id = ?1 AND archive_id = ?2 AND is_current = 1",
            params![candidate.current.id, id],
        )
        .map_err(|error| db_error("failed to retire the guessed system", error))?;
    if retired != 1 {
        return Err(stale("a system changed while the corrections were applied"));
    }
    conn.execute(
        "INSERT INTO platform_assignments(archive_id,platform,source,confidence,is_current,assigned_at) \
         VALUES (?1,?2,?3,NULL,1,?4)",
        params![id, candidate.proposed_platform, VERIFIED_DAT_PLATFORM_SOURCE, stamp],
    )
    .map_err(|error| db_error("failed to record the corrected system", error))?;
    let applied_assignment_id = conn.last_insert_rowid();

    let fingerprint = fingerprint_platform(conn, id)?;
    if let Some((folder, relative, _)) = &fingerprint {
        conn.execute(
            "UPDATE scan_fingerprints SET platform = ?1 WHERE source_folder_id = ?2 AND relative_path = ?3",
            params![candidate.proposed_platform, folder, relative],
        )
        .map_err(|error| db_error("failed to update the scanner's cached system", error))?;
    }

    let written = persist_library_dat_identity_row(conn, id, &candidate.identity, stamp)?;
    if !matches!(
        written,
        super::LibraryDatIdentityPersistOutcome::Inserted
            | super::LibraryDatIdentityPersistOutcome::Updated
    ) {
        return Err(ArchiveFsError::Database(format!(
            "game {id}: the verification result was not saved ({written:?}); nothing was corrected"
        )));
    }
    let applied_result = result_row(conn, id, &candidate.identity.source.source_id)?
        .ok_or_else(|| ArchiveFsError::Database("the saved result cannot be read back".into()))?;
    Ok(ReceiptRecord {
        archive_id: id,
        path: candidate.path.clone(),
        previous: candidate.current.clone(),
        applied_assignment_id,
        applied_platform: candidate.proposed_platform.clone(),
        fingerprint: fingerprint.map(
            |(folder, relative, platform): (i64, Vec<u8>, Option<String>)| FingerprintRecord {
                source_folder_id: folder,
                relative_path_hex: hex(&relative),
                previous_platform: platform,
            },
        ),
        previous_result,
        previous_recheck,
        applied_result_id: applied_result.id,
        applied_result_digest: applied_result.digest(),
        undone: false,
    })
}

// --- undo ------------------------------------------------------------------------

/// How a receipt record stands against the database now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordState {
    InPlace,
    Undone,
    Changed(String),
    /// The correction was never committed (a void pending receipt).
    NeverApplied,
}

impl Database {
    /// Undoes the corrections of one receipt: every game, or only `only`.
    /// A game whose system or result changed after the correction is refused
    /// and left as it is; by default any refusal refuses the whole Undo.
    pub fn undo_platform_corrections(
        &mut self,
        receipt_path: &Path,
        only: Option<&[i64]>,
        when_changed: WhenSomeChanged,
    ) -> Result<PlatformCorrectionsUndone> {
        let receipt = read_receipt(receipt_path)?;
        if receipt.database_path != self.path {
            return Err(ArchiveFsError::Database(
                "the receipt belongs to another library".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| db_error("failed to start the Undo", error))?;
        let mut refused = Vec::new();
        let mut doable = Vec::new();
        for record in &receipt.records {
            if only.is_some_and(|only| !only.contains(&record.archive_id)) {
                continue;
            }
            match record_state(&tx, &receipt, record)? {
                RecordState::InPlace => doable.push(record),
                RecordState::Undone => refused.push(PlatformCorrectionRefusal {
                    archive_id: record.archive_id,
                    reason: "already undone".into(),
                }),
                RecordState::NeverApplied => refused.push(PlatformCorrectionRefusal {
                    archive_id: record.archive_id,
                    reason: "this correction was never applied".into(),
                }),
                RecordState::Changed(reason) => refused.push(PlatformCorrectionRefusal {
                    archive_id: record.archive_id,
                    reason,
                }),
            }
        }
        if let Some(only) = only {
            for id in only {
                if !receipt.records.iter().any(|r| r.archive_id == *id) {
                    refused.push(PlatformCorrectionRefusal {
                        archive_id: *id,
                        reason: "not in this correction".into(),
                    });
                }
            }
        }
        if doable.is_empty() || (!refused.is_empty() && when_changed == WhenSomeChanged::RefuseAll)
        {
            return Ok(PlatformCorrectionsUndone {
                undone: Vec::new(),
                refused,
            });
        }
        for record in &doable {
            undo_one(&tx, record)?;
        }
        #[cfg(test)]
        if seams::stop_at(seams::Point::UndoBeforeCommit) {
            drop(tx);
            return Err(ArchiveFsError::Database("simulated termination".into()));
        }
        tx.commit()
            .map_err(|error| db_error("failed to commit the Undo", error))?;
        let undone: Vec<i64> = doable.iter().map(|r| r.archive_id).collect();
        // The database is the authority (an undone record is recognised from
        // it); the receipt mark is kept in step when it can be.
        let _ = update_receipt(receipt_path, |r| {
            r.state = ReceiptState::Applied;
            for record in &mut r.records {
                if undone.contains(&record.archive_id) {
                    record.undone = true;
                }
            }
        });
        Ok(PlatformCorrectionsUndone { undone, refused })
    }

    /// Every correction receipt of this library, resolved against the
    /// database. A receipt left pending by an interruption is settled here:
    /// applied when its rows are in place, void when they never were.
    pub fn platform_correction_receipts(&self) -> Result<Vec<PlatformCorrectionReceiptSummary>> {
        let mut summaries = Vec::new();
        let dir = receipt_dir(&self.path);
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok(summaries);
        };
        let prefix = receipt_prefix(&self.path);
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".json"))
            })
            .collect();
        paths.sort();
        for path in paths {
            let Ok(receipt) = read_receipt(&path) else {
                continue;
            };
            if receipt.database_path != self.path || receipt.state == ReceiptState::Void {
                continue;
            }
            let mut summary = PlatformCorrectionReceiptSummary {
                receipt_path: path.clone(),
                created_at: receipt.created_at.clone(),
                catalogue: receipt.catalogue.display_name.clone(),
                undoable: Vec::new(),
                undone: Vec::new(),
                changed: Vec::new(),
            };
            let mut never = 0;
            for record in &receipt.records {
                match record_state(&self.connection, &receipt, record)? {
                    RecordState::InPlace => {
                        summary.undoable.push(PlatformCorrectionRecordSummary {
                            archive_id: record.archive_id,
                            path: record.path.clone(),
                            previous_platform: Some(record.previous.platform.clone()),
                            previous_source: Some(record.previous.source.clone()),
                            corrected_platform: record.applied_platform.clone(),
                        })
                    }
                    RecordState::Undone => summary.undone.push(record.archive_id),
                    RecordState::Changed(reason) => {
                        summary.changed.push(PlatformCorrectionRefusal {
                            archive_id: record.archive_id,
                            reason,
                        })
                    }
                    RecordState::NeverApplied => never += 1,
                }
            }
            if receipt.state == ReceiptState::Pending {
                if never == receipt.records.len() {
                    // Interrupted before the commit: nothing was applied.
                    let _ = update_receipt(&path, |r| r.state = ReceiptState::Void);
                    continue;
                }
                let _ = update_receipt(&path, |r| r.state = ReceiptState::Applied);
            }
            summaries.push(summary);
        }
        Ok(summaries)
    }
}

fn record_state(
    conn: &Connection,
    receipt: &Receipt,
    record: &ReceiptRecord,
) -> Result<RecordState> {
    let applied: Option<(i64, String, String, i64, String)> = conn
        .query_row(
            "SELECT archive_id, platform, source, is_current, assigned_at FROM platform_assignments WHERE id = ?1",
            [record.applied_assignment_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(|error| db_error("failed to read a corrected system", error))?;
    let ours = applied
        .as_ref()
        .is_some_and(|(archive, platform, source, _, at)| {
            *archive == record.archive_id
                && *platform == record.applied_platform
                && source == VERIFIED_DAT_PLATFORM_SOURCE
                && *at == receipt.created_at
        });
    let current = current_assignment(conn, record.archive_id)?;
    if !ours {
        // The applied row is gone (or the id belongs to another row).
        if current.as_ref().map(|c| c.id) == Some(record.previous.id) {
            return Ok(if record.undone || receipt.state != ReceiptState::Pending {
                RecordState::Undone
            } else {
                RecordState::NeverApplied
            });
        }
        if receipt.state == ReceiptState::Pending && !record.undone {
            return Ok(RecordState::NeverApplied);
        }
        return Ok(RecordState::Changed(
            "its system was changed after the correction".into(),
        ));
    }
    if applied.as_ref().is_some_and(|a| a.3 != 1)
        || current.as_ref().map(|c| c.id) != Some(record.applied_assignment_id)
    {
        return Ok(RecordState::Changed(
            "its system was changed after the correction; Undo would overwrite that".into(),
        ));
    }
    let previous_exists: bool = conn
        .query_row(
            "SELECT count(*) FROM platform_assignments WHERE id = ?1 AND archive_id = ?2 AND is_current = 0",
            params![record.previous.id, record.archive_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| db_error("failed to read the previous system", error))?
        == 1;
    if !previous_exists {
        return Ok(RecordState::Changed(
            "its previous system is no longer recorded".into(),
        ));
    }
    let result = result_row_by_id(conn, record.applied_result_id)?;
    if result.map(|r| r.digest()) != Some(record.applied_result_digest.clone()) {
        return Ok(RecordState::Changed(
            "its verification result changed after the correction; Undo would overwrite that"
                .into(),
        ));
    }
    Ok(RecordState::InPlace)
}

fn undo_one(conn: &Connection, record: &ReceiptRecord) -> Result<()> {
    let removed = conn
        .execute(
            "DELETE FROM platform_assignments WHERE id = ?1 AND archive_id = ?2 AND is_current = 1 AND source = ?3",
            params![record.applied_assignment_id, record.archive_id, VERIFIED_DAT_PLATFORM_SOURCE],
        )
        .map_err(|error| db_error("failed to remove the corrected system", error))?;
    if removed != 1 {
        return Err(ArchiveFsError::Database(
            "the corrected system changed during the Undo; nothing was undone".into(),
        ));
    }
    let restored = conn
        .execute(
            "UPDATE platform_assignments SET is_current = 1 WHERE id = ?1 AND archive_id = ?2 AND is_current = 0",
            params![record.previous.id, record.archive_id],
        )
        .map_err(|error| db_error("failed to restore the previous system", error))?;
    if restored != 1 {
        return Err(ArchiveFsError::Database(
            "the previous system cannot be restored; nothing was undone".into(),
        ));
    }
    if let Some(fp) = &record.fingerprint {
        let relative = unhex(&fp.relative_path_hex)?;
        // Only the value the correction wrote is put back: a newer scan's
        // cached result is never overwritten.
        conn.execute(
            "UPDATE scan_fingerprints SET platform = ?1 \
             WHERE source_folder_id = ?2 AND relative_path = ?3 AND platform IS ?4",
            params![
                fp.previous_platform,
                fp.source_folder_id,
                relative,
                record.applied_platform
            ],
        )
        .map_err(|error| db_error("failed to restore the scanner's cached system", error))?;
    }
    match &record.previous_result {
        None => {
            let n = conn
                .execute(
                    "DELETE FROM library_dat_identities WHERE id = ?1 AND archive_id = ?2",
                    params![record.applied_result_id, record.archive_id],
                )
                .map_err(|error| db_error("failed to remove the correction's result", error))?;
            if n != 1 {
                return Err(ArchiveFsError::Database(
                    "the correction's result changed during the Undo; nothing was undone".into(),
                ));
            }
        }
        Some(previous) => previous.restore(conn)?,
    }
    if let Some(recheck) = &record.previous_recheck {
        let exists: i64 = conn
            .query_row(
                "SELECT count(*) FROM dat_recheck_attempts WHERE archive_id = ?1 AND dat_source_id = ?2",
                params![record.archive_id, recheck_source(recheck)],
                |row| row.get(0),
            )
            .map_err(|error| db_error("failed to read a re-check note", error))?;
        // A newer note is never overwritten.
        if exists == 0 {
            recheck.insert_into(conn, "dat_recheck_attempts")?;
        }
    }
    Ok(())
}

// --- rows ------------------------------------------------------------------------

fn current_assignment(conn: &Connection, archive_id: i64) -> Result<Option<AssignmentBinding>> {
    conn.query_row(
        "SELECT id, platform, source FROM platform_assignments WHERE archive_id = ?1 AND is_current = 1",
        [archive_id],
        |row| {
            Ok(AssignmentBinding {
                id: row.get(0)?,
                platform: row.get(1)?,
                source: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|error| db_error("failed to read a game's system", error))
}

/// A Discovery Import that added this game can still be undone; a
/// correction marks its record changed for good (migration 0025).
fn discovery_undo_still_open(conn: &Connection, archive_id: i64) -> Result<bool> {
    conn.query_row(
        "SELECT count(*) FROM discovery_import_records r \
         JOIN discovery_import_operations o ON o.operation_id = r.operation_id \
         WHERE r.archive_id = ?1 AND r.touched = 0 AND o.state = 'committed'",
        [archive_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(|error| db_error("failed to read Discovery Import history", error))
}

/// A scanner cache row: source folder, relative path, cached system.
type FingerprintRow = (i64, Vec<u8>, Option<String>);

fn fingerprint_platform(conn: &Connection, archive_id: i64) -> Result<Option<FingerprintRow>> {
    conn.query_row(
        "SELECT f.source_folder_id, f.relative_path, f.platform FROM scan_fingerprints f \
         JOIN archives a ON a.source_folder_id = f.source_folder_id AND a.relative_path = f.relative_path \
         WHERE a.id = ?1",
        [archive_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(|error| db_error("failed to read the scanner's cached system", error))
}

/// One `library_dat_identities` row, every column, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResultRow {
    id: i64,
    columns: Vec<String>,
    /// Each value as `["n"]`, `["i", v]`, `["t", text]` or `["b", hex]`.
    values: Vec<Vec<String>>,
}

impl ResultRow {
    fn digest(&self) -> String {
        let mut hash = Sha256::new();
        for (column, value) in self.columns.iter().zip(&self.values) {
            hash.update(column.as_bytes());
            hash.update([0]);
            for part in value {
                hash.update(part.as_bytes());
                hash.update([1]);
            }
        }
        hex(&hash.finalize())
    }

    fn read(
        conn: &Connection,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Option<Self>> {
        Self::read_from(conn, "library_dat_identities", filter, args)
    }

    fn read_from(
        conn: &Connection,
        table: &str,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Option<Self>> {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} WHERE {filter}"))
            .map_err(|error| db_error("failed to read a verification result", error))?;
        let columns: Vec<String> = stmt.column_names().map(|c| c.to_string()).collect();
        let width = columns.len();
        stmt.query_row(args, |row| {
            let mut values = Vec::with_capacity(width);
            for index in 0..width {
                values.push(match row.get::<_, SqlValue>(index)? {
                    SqlValue::Null => vec!["n".to_string()],
                    SqlValue::Integer(v) => vec!["i".to_string(), v.to_string()],
                    SqlValue::Real(v) => vec!["r".to_string(), v.to_string()],
                    SqlValue::Text(v) => vec!["t".to_string(), v],
                    SqlValue::Blob(v) => vec!["b".to_string(), hex(&v)],
                });
            }
            let id_index = columns.iter().position(|c| c == "id").unwrap_or(0);
            let id = values[id_index]
                .get(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or(-1);
            Ok(Self {
                id,
                columns: columns.clone(),
                values,
            })
        })
        .optional()
        .map_err(|error| db_error("failed to read a verification result", error))
    }

    fn sql_values(&self) -> Result<Vec<SqlValue>> {
        let mut values = Vec::new();
        for value in &self.values {
            values.push(match value.first().map(String::as_str) {
                Some("n") => SqlValue::Null,
                Some("i") => SqlValue::Integer(
                    value
                        .get(1)
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(malformed)?,
                ),
                Some("r") => SqlValue::Real(
                    value
                        .get(1)
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(malformed)?,
                ),
                Some("t") => SqlValue::Text(value.get(1).cloned().ok_or_else(malformed)?),
                Some("b") => SqlValue::Blob(unhex(value.get(1).ok_or_else(malformed)?)?),
                _ => return Err(malformed()),
            });
        }
        Ok(values)
    }

    /// Inserts this row into `table` again, every column as it was.
    fn insert_into(&self, conn: &Connection, table: &str) -> Result<()> {
        let values = self.sql_values()?;
        let columns: Vec<String> = self.columns.iter().map(|c| format!("\"{c}\"")).collect();
        let marks: Vec<String> = (1..=values.len()).map(|i| format!("?{i}")).collect();
        let args: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        conn.execute(
            &format!(
                "INSERT INTO {table}({}) VALUES({})",
                columns.join(","),
                marks.join(",")
            ),
            args.as_slice(),
        )
        .map_err(|error| db_error("failed to restore a re-check note", error))?;
        Ok(())
    }

    /// Puts this row back exactly (same id, every column).
    fn restore(&self, conn: &Connection) -> Result<()> {
        let values = self.sql_values()?;
        let assignments: Vec<String> = self
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| format!("\"{c}\" = ?{}", i + 1))
            .collect();
        let sql = format!(
            "UPDATE library_dat_identities SET {} WHERE id = ?{}",
            assignments.join(", "),
            self.columns.len() + 1
        );
        let mut args: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        args.push(&self.id);
        let n = conn
            .execute(&sql, args.as_slice())
            .map_err(|error| db_error("failed to restore the previous result", error))?;
        if n != 1 {
            return Err(ArchiveFsError::Database(
                "the previous result cannot be restored; nothing was undone".into(),
            ));
        }
        Ok(())
    }
}

fn recheck_source(row: &ResultRow) -> String {
    row.columns
        .iter()
        .position(|c| c == "dat_source_id")
        .and_then(|i| row.values[i].get(1).cloned())
        .unwrap_or_default()
}

fn result_row(conn: &Connection, archive_id: i64, source_id: &str) -> Result<Option<ResultRow>> {
    ResultRow::read(
        conn,
        "archive_id = ?1 AND dat_source_id = ?2",
        &[&archive_id, &source_id],
    )
}

fn result_row_by_id(conn: &Connection, id: i64) -> Result<Option<ResultRow>> {
    ResultRow::read(conn, "id = ?1", &[&id])
}

// --- receipt ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptState {
    /// Written before the commit; settled when listed.
    Pending,
    Applied,
    /// Interrupted before the commit: nothing was applied.
    Void,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FingerprintRecord {
    source_folder_id: i64,
    relative_path_hex: String,
    previous_platform: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReceiptRecord {
    archive_id: i64,
    path: String,
    previous: AssignmentBinding,
    applied_assignment_id: i64,
    applied_platform: String,
    fingerprint: Option<FingerprintRecord>,
    previous_result: Option<ResultRow>,
    #[serde(default)]
    previous_recheck: Option<ResultRow>,
    applied_result_id: i64,
    applied_result_digest: String,
    undone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    version: u32,
    state: ReceiptState,
    database_path: PathBuf,
    created_at: String,
    preview_token: String,
    catalogue: CatalogueBinding,
    /// Always true: corrections change library records only.
    files_untouched: bool,
    records: Vec<ReceiptRecord>,
}

fn receipt_dir(db: &Path) -> PathBuf {
    db.parent().map(Path::to_path_buf).unwrap_or_default()
}

fn receipt_prefix(db: &Path) -> String {
    format!(
        "{}.platform-corrections-",
        db.file_name().unwrap_or_default().to_string_lossy()
    )
}

fn receipt_error(what: &str, error: impl std::fmt::Display) -> ArchiveFsError {
    ArchiveFsError::Database(format!(
        "platform correction receipt: could not {what}: {error}; nothing was corrected"
    ))
}

fn sync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

fn write_new_receipt(dir: &Path, db: &Path, receipt: &Receipt) -> Result<PathBuf> {
    let unix = receipt.created_at.replace([':', '-'], "");
    let path = dir.join(format!(
        "{}{unix}-{}.json",
        receipt_prefix(db),
        &receipt.preview_token[..12]
    ));
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|e| receipt_error("encode", e))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| receipt_error("create", e))?;
    let written = (|| {
        #[cfg(test)]
        seams::fail_write()?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        sync_dir(dir)
    })();
    if let Err(error) = written {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(receipt_error("write", error));
    }
    Ok(path)
}

fn read_receipt(path: &Path) -> Result<Receipt> {
    let bytes = fs::read(path).map_err(|e| receipt_error("read", e))?;
    let receipt: Receipt = serde_json::from_slice(&bytes).map_err(|e| receipt_error("parse", e))?;
    if receipt.version != 1 || !receipt.files_untouched {
        return Err(receipt_error("use", "unknown receipt version"));
    }
    Ok(receipt)
}

/// Rewrites a receipt atomically and durably (temporary file, fsync,
/// rename, folder fsync).
fn update_receipt(path: &Path, change: impl FnOnce(&mut Receipt)) -> Result<()> {
    let mut receipt = read_receipt(path)?;
    change(&mut receipt);
    let bytes = serde_json::to_vec_pretty(&receipt).map_err(|e| receipt_error("encode", e))?;
    let tmp = path.with_extension("json.tmp");
    (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        sync_dir(&receipt_dir(path))
    })()
    .map_err(|e| receipt_error("update", e))
}

fn stale(why: &str) -> ArchiveFsError {
    ArchiveFsError::Database(format!(
        "{PLATFORM_CORRECTION_STALE}: {why}; nothing was corrected. Check the games again and make a new preview"
    ))
}

fn malformed() -> ArchiveFsError {
    ArchiveFsError::Database("platform correction receipt is malformed; nothing was undone".into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return Err(malformed());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| malformed()))
        .collect()
}

#[cfg(test)]
pub(crate) mod seams {
    use std::cell::Cell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Point {
        AfterReceiptBeforeCommit,
        AfterCommitBeforeMark,
        UndoBeforeCommit,
    }

    thread_local! {
        static STOP: Cell<Option<Point>> = const { Cell::new(None) };
        static WRITE_ERROR: Cell<Option<i32>> = const { Cell::new(None) };
        static ABORT: Cell<bool> = const { Cell::new(false) };
    }

    /// Stop (simulate termination) at `point`; with `abort`, really end the
    /// process there.
    pub(crate) fn set_stop(point: Option<Point>, abort: bool) {
        STOP.with(|c| c.set(point));
        ABORT.with(|c| c.set(abort));
    }

    pub(crate) fn stop_at(point: Point) -> bool {
        let hit = STOP.with(|c| c.get()) == Some(point);
        if hit && ABORT.with(|c| c.get()) {
            std::process::abort();
        }
        hit
    }

    /// Make the receipt write fail with this OS error (e.g. ENOSPC).
    pub(crate) fn set_write_error(errno: Option<i32>) {
        WRITE_ERROR.with(|c| c.set(errno));
    }

    pub(crate) fn fail_write() -> std::io::Result<()> {
        match WRITE_ERROR.with(|c| c.get()) {
            Some(errno) => Err(std::io::Error::from_raw_os_error(errno)),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "platform_corrections_tests.rs"]
mod tests;
