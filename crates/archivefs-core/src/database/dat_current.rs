//! Source-aware read projections shared by library consumers. Raw saved rows
//! remain available for history; these projections require current evidence.
use super::*;
use crate::dat::configured_catalogues::ConfiguredCatalogue;
use crate::dat::library_identity_summary::{
    DatProvenanceFreshness, DatVerificationState, DiscFileState,
};

#[derive(Debug, Default)]
pub struct CurrentLibraryDatEvidence {
    pub exact: HashMap<i64, Option<&'static str>>,
    pub states: HashMap<i64, LibraryDatState>,
    /// The audited hashes behind each exact match in `exact`, taken from the
    /// same current, unchanged-file evidence. Absent when two verified rows
    /// for one item disagree, so a caller never picks one of them.
    pub exact_hashes: HashMap<i64, crate::dat::library_identity_summary::LibraryItemHashes>,
    /// The audited hashes from every current row for an item, whatever its
    /// verdict (the same unchanged-file and current-source rules as above).
    /// Absent when two current rows disagree. Hashes only: never proof.
    pub current_hashes: HashMap<i64, crate::dat::library_identity_summary::LibraryItemHashes>,
}

/// Why one saved DAT row does, or does not, count as current evidence.
///
/// Only [`SavedDatEvidenceStatus::Current`] rows feed
/// [`CurrentLibraryDatEvidence`]. Every other value names what a re-check
/// would have to establish; none of them is ever treated as verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SavedDatEvidenceStatus {
    /// Audit-bound proof is present and the file and catalogue are unchanged.
    Current,
    /// Saved before the audited file or the catalogue digest was recorded.
    MissingProof,
    /// The audited file was rewritten, replaced or can no longer be tied to
    /// the object that was hashed.
    FileChanged,
    /// The file is gone, unreadable or marked missing in the library.
    FileUnavailable,
    /// The catalogue bytes or revision differ from the audited catalogue.
    CatalogueChanged,
    /// The catalogue is not configured now, or has no content digest.
    CatalogueUnavailable,
    /// The saved result does not apply to this row (another source id, or a
    /// negative from a catalogue that does not cover the game's system).
    DoesNotApply,
}

/// One saved DAT row with the reason it is or is not current. Metadata only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedDatEvidenceRow {
    pub archive_id: i64,
    pub dat_source_id: String,
    pub path: PathBuf,
    pub platform: Option<String>,
    pub size_bytes: Option<u64>,
    /// The saved verdict column (`verified_single_match`, `no_match`, ...).
    pub saved_verdict: String,
    pub audited_at: String,
    pub status: SavedDatEvidenceStatus,
}

struct SavedRowInputs<'a> {
    source_id: &'a str,
    source: Option<&'a ConfiguredCatalogue>,
    path: &'a std::path::Path,
    size_bytes: Option<i64>,
    platform: Option<&'a str>,
    marked_stale: bool,
    library_missing: bool,
}

/// The single rule for whether a saved row is current. The summary is
/// returned only for a current row.
fn judge_saved_row(
    saved: &crate::dat::library_identity_summary::PersistedLibraryDatIdentity,
    row: &SavedRowInputs<'_>,
) -> (
    SavedDatEvidenceStatus,
    Option<crate::dat::library_identity_summary::LibraryDatIdentitySummary>,
) {
    use SavedDatEvidenceStatus as S;
    if row.library_missing {
        return (S::FileUnavailable, None);
    }
    let Some(source) = row.source else {
        return (S::CatalogueUnavailable, None);
    };
    if saved.source.source_id != row.source_id {
        return (S::DoesNotApply, None);
    }
    // A "not in this catalogue" result from a catalogue that does not cover
    // the game's system says nothing about the game, with or without proof,
    // so there is nothing to check again.
    if !saved.carries_identity()
        && !crate::dat::identity::negative_dat_result_applies(
            &saved.source.catalogue_names,
            saved.source.ecosystem,
            row.platform,
        )
    {
        return (S::DoesNotApply, None);
    }
    match saved.file_state_for_path(row.path) {
        DiscFileState::Unchanged => {}
        DiscFileState::NotRecorded => return (S::MissingProof, None),
        DiscFileState::Missing(_) => return (S::FileUnavailable, None),
        DiscFileState::Changed(_) | DiscFileState::IdentityUncertain(_) => {
            return (S::FileChanged, None);
        }
    }
    if row.size_bytes.and_then(|size| u64::try_from(size).ok()) != saved.audited_hashes.size_bytes {
        return (S::FileChanged, None);
    }
    if row.marked_stale {
        return (S::CatalogueChanged, None);
    }
    let mut summary = saved.reconstruct_summary(
        Some(&saved.audited_hashes),
        crate::dat::library_identity_summary::SourceFreshnessContext {
            current_source_revision: source.revision.as_deref(),
            source_available: true,
            revision_marked_stale: false,
        },
    );
    source.check_saved_identity(&mut summary);
    match summary.provenance_freshness {
        DatProvenanceFreshness::Current => (S::Current, Some(summary)),
        DatProvenanceFreshness::Stale => (S::CatalogueChanged, None),
        DatProvenanceFreshness::Unknown if saved.source.catalogue_sha256.is_none() => {
            (S::MissingProof, None)
        }
        DatProvenanceFreshness::Unknown => (S::CatalogueUnavailable, None),
    }
}

impl Database {
    /// Every saved DAT row with why it is or is not current. Reads saved rows
    /// and file metadata only: no audit, matching or content hashing.
    pub fn saved_dat_evidence_status(
        &self,
        catalogues: &HashMap<String, ConfiguredCatalogue>,
    ) -> Result<Vec<SavedDatEvidenceRow>> {
        let mut statement = self.connection.prepare(
            "SELECT d.archive_id, d.dat_source_id, d.facts_json, a.size_bytes, p.platform, a.absolute_path_cached,                     d.revision_marked_stale, a.last_verified_missing_at IS NOT NULL, d.verification_state, d.audited_at              FROM library_dat_identities d JOIN archives a ON a.id = d.archive_id              LEFT JOIN platform_assignments p ON p.archive_id = a.id AND p.is_current = 1              ORDER BY d.dat_source_id, d.archive_id",
        ).map_err(|error| db_error("failed to prepare saved DAT evidence status", error))?;
        let mut rows = statement
            .query([])
            .map_err(|error| db_error("failed to query saved DAT evidence status", error))?;
        let mut result = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|error| db_error("failed to read saved DAT evidence status", error))?
        {
            let read = |error| db_error("failed to read saved DAT evidence status", error);
            let archive_id: i64 = row.get(0).map_err(read)?;
            let dat_source_id: String = row.get(1).map_err(read)?;
            let bytes: Vec<u8> = row.get(2).map_err(read)?;
            let size_bytes: Option<i64> = row.get(3).map_err(read)?;
            let platform: Option<String> = row.get(4).map_err(read)?;
            let path_bytes: Vec<u8> = row.get(5).map_err(read)?;
            let marked_stale: bool = row.get(6).map_err(read)?;
            let library_missing: bool = row.get(7).map_err(read)?;
            let saved_verdict: String = row.get(8).map_err(read)?;
            let audited_at: String = row.get(9).map_err(read)?;
            let saved: crate::dat::library_identity_summary::PersistedLibraryDatIdentity =
                serde_json::from_slice(&bytes).map_err(|error| {
                    ArchiveFsError::Database(format!("stored DAT evidence is unreadable: {error}"))
                })?;
            let path = PathBuf::from(std::ffi::OsString::from_vec(path_bytes));
            let (status, _) = judge_saved_row(
                &saved,
                &SavedRowInputs {
                    source_id: &dat_source_id,
                    source: catalogues.get(&dat_source_id),
                    path: &path,
                    size_bytes,
                    platform: platform.as_deref(),
                    marked_stale,
                    library_missing,
                },
            );
            result.push(SavedDatEvidenceRow {
                archive_id,
                dat_source_id,
                path,
                platform,
                size_bytes: size_bytes.and_then(|size| u64::try_from(size).ok()),
                saved_verdict,
                audited_at,
                status,
            });
        }
        Ok(result)
    }

    /// Uses configured local/managed content identities plus the ordinary item
    /// freshness rules. No audit, matching or ROM hashing is performed.
    pub fn current_library_dat_evidence(
        &self,
        catalogues: &HashMap<String, ConfiguredCatalogue>,
    ) -> Result<CurrentLibraryDatEvidence> {
        let mut result = CurrentLibraryDatEvidence::default();
        let mut conflicting_hashes = std::collections::HashSet::new();
        let mut conflicting_current = std::collections::HashSet::new();
        let mut statement = self.connection.prepare(
            "SELECT d.archive_id, d.dat_source_id, d.facts_json, a.size_bytes, p.platform, d.dat_ecosystem, a.absolute_path_cached \
             FROM library_dat_identities d JOIN archives a ON a.id = d.archive_id \
             LEFT JOIN platform_assignments p ON p.archive_id = a.id AND p.is_current = 1 \
             WHERE d.revision_marked_stale = 0 AND a.last_verified_missing_at IS NULL",
        ).map_err(|error| db_error("failed to prepare current DAT evidence", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                ))
            })
            .map_err(|error| db_error("failed to query current DAT evidence", error))?;
        for row in rows {
            let (archive_id, source_id, bytes, size_bytes, platform, ecosystem, path_bytes) =
                row.map_err(|error| db_error("failed to read current DAT evidence", error))?;
            let saved: crate::dat::library_identity_summary::PersistedLibraryDatIdentity =
                serde_json::from_slice(&bytes).map_err(|error| {
                    ArchiveFsError::Database(format!("stored DAT evidence is unreadable: {error}"))
                })?;
            let path = PathBuf::from(std::ffi::OsString::from_vec(path_bytes));
            // The query already excludes rows marked stale or missing.
            let (SavedDatEvidenceStatus::Current, Some(summary)) = judge_saved_row(
                &saved,
                &SavedRowInputs {
                    source_id: &source_id,
                    source: catalogues.get(&source_id),
                    path: &path,
                    size_bytes,
                    platform: platform.as_deref(),
                    marked_stale: false,
                    library_missing: false,
                },
            ) else {
                continue;
            };
            let state = match summary.verification_state {
                DatVerificationState::VerifiedSingleMatch { .. } => LibraryDatState::Verified,
                DatVerificationState::Probable => LibraryDatState::Probable,
                DatVerificationState::AmbiguousMultipleCandidates { .. }
                | DatVerificationState::Conflicting { .. } => LibraryDatState::Ambiguous,
                DatVerificationState::NoMatch => LibraryDatState::NoMatch,
                _ => continue,
            };
            if !conflicting_current.contains(&archive_id) {
                match result.current_hashes.get(&archive_id) {
                    None => {
                        result
                            .current_hashes
                            .insert(archive_id, saved.audited_hashes.clone());
                    }
                    Some(existing) if *existing != saved.audited_hashes => {
                        result.current_hashes.remove(&archive_id);
                        conflicting_current.insert(archive_id);
                    }
                    _ => {}
                }
            }
            result
                .states
                .entry(archive_id)
                .and_modify(|current| *current = (*current).min(state))
                .or_insert(state);
            if state == LibraryDatState::Verified {
                let label = ecosystem
                    .and_then(|text| {
                        serde_json::from_value::<crate::dat::model::DatEcosystem>(text.into()).ok()
                    })
                    .map(crate::dat::model::DatEcosystem::label);
                result
                    .exact
                    .entry(archive_id)
                    .and_modify(|current| {
                        if *current != label {
                            *current = None;
                        }
                    })
                    .or_insert(label);
                let hashes = &saved.audited_hashes;
                match result.exact_hashes.get(&archive_id) {
                    None if !conflicting_hashes.contains(&archive_id) => {
                        result.exact_hashes.insert(archive_id, hashes.clone());
                    }
                    Some(existing) if existing != hashes => {
                        result.exact_hashes.remove(&archive_id);
                        conflicting_hashes.insert(archive_id);
                    }
                    _ => {}
                }
            }
        }
        Ok(result)
    }
}

/// Why the latest re-check of one saved row saved nothing. An explanation
/// only: never evidence, and never read by anything that decides identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatRecheckAttempt {
    pub archive_id: i64,
    pub dat_source_id: String,
    pub path: PathBuf,
    pub attempted_at: String,
    pub reason: String,
    pub catalogue_sha256: Option<String>,
}

impl Database {
    /// Records, in one transaction, why a re-check saved nothing for each
    /// row. A row that has current evidence by then is left without a note.
    pub fn record_dat_recheck_attempts(
        &mut self,
        dat_source_id: &str,
        catalogue_sha256: Option<&str>,
        notes: &[(i64, PathBuf, String)],
    ) -> Result<()> {
        if notes.is_empty() {
            return Ok(());
        }
        let now = now_utc_string();
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| db_error("failed to start DAT re-check note transaction", error))?;
        for (archive_id, path, reason) in notes {
            tx.execute(
                "INSERT INTO dat_recheck_attempts \
                 (archive_id, dat_source_id, archive_path, attempted_at, reason, catalogue_sha256) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(archive_id, dat_source_id) DO UPDATE SET \
                  archive_path = excluded.archive_path, attempted_at = excluded.attempted_at, \
                  reason = excluded.reason, catalogue_sha256 = excluded.catalogue_sha256",
                params![
                    archive_id,
                    dat_source_id,
                    path.as_os_str().as_bytes(),
                    now,
                    reason,
                    catalogue_sha256
                ],
            )
            .map_err(|error| db_error("failed to record a DAT re-check note", error))?;
        }
        tx.commit()
            .map_err(|error| db_error("failed to commit DAT re-check notes", error))
    }

    /// Every recorded reason a re-check saved nothing. A note is returned only
    /// while the library row it names still has the path it had then.
    pub fn dat_recheck_attempts(&self) -> Result<Vec<DatRecheckAttempt>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT n.archive_id, n.dat_source_id, n.archive_path, n.attempted_at, n.reason, n.catalogue_sha256 \
                 FROM dat_recheck_attempts n JOIN archives a \
                   ON a.id = n.archive_id AND a.absolute_path_cached = n.archive_path \
                 ORDER BY n.dat_source_id, n.archive_id",
            )
            .map_err(|error| db_error("failed to prepare DAT re-check notes", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok(DatRecheckAttempt {
                    archive_id: row.get(0)?,
                    dat_source_id: row.get(1)?,
                    path: PathBuf::from(std::ffi::OsString::from_vec(row.get::<_, Vec<u8>>(2)?)),
                    attempted_at: row.get(3)?,
                    reason: row.get(4)?,
                    catalogue_sha256: row.get(5)?,
                })
            })
            .map_err(|error| db_error("failed to query DAT re-check notes", error))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| db_error("failed to read DAT re-check notes", error))
    }

    /// How many replaced evidence rows are kept for the record.
    pub fn replaced_dat_evidence_count(&self) -> Result<u64> {
        self.connection
            .query_row(
                "SELECT COUNT(*) FROM library_dat_identity_history",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as u64)
            .map_err(|error| db_error("failed to count replaced DAT evidence", error))
    }
}
