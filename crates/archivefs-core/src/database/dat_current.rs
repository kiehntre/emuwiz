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
    /// Library games that have at least one saved DAT result, from any
    /// catalogue and whether or not it is current. Reads saved rows only.
    pub fn archives_with_saved_dat_results(&self) -> Result<std::collections::HashSet<i64>> {
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT archive_id FROM library_dat_identities")
            .map_err(|error| db_error("failed to prepare saved DAT result list", error))?;
        let ids = statement
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|error| db_error("failed to list saved DAT results", error))?
            .collect::<std::result::Result<_, _>>()
            .map_err(|error| db_error("failed to read saved DAT result list", error))?;
        Ok(ids)
    }

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
            // A row of a catalogue that is not configured can never count,
            // so it is skipped unread: one unreadable row of a removed
            // catalogue must not take the whole library's evidence with it.
            if !catalogues.contains_key(&source_id) {
                continue;
            }
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
    /// row. Nothing is written for a row that is no longer in the library at
    /// that path, or for which this very check saved a proof-bearing result.
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
            // A check that did save a proof-bearing row against these very
            // catalogue bytes (a probable match, say) saved something: the
            // row itself is the record, and a note would contradict it.
            let saved_by_this_check =
                library_dat_identity_for_item_conn(&tx, *archive_id, dat_source_id)?.is_some_and(
                    |row| {
                        (row.audit_file.is_some() || !row.disc_members.is_empty())
                            && catalogue_sha256.is_some()
                            && row.source.catalogue_sha256.as_deref() == catalogue_sha256
                    },
                );
            if saved_by_this_check {
                continue;
            }
            // Only for the library row the check was about: the same id at
            // the same path. A plan made before the row moved or was
            // forgotten writes nothing.
            tx.execute(
                "INSERT INTO dat_recheck_attempts \
                 (archive_id, dat_source_id, archive_path, attempted_at, reason, catalogue_sha256) \
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6 \
                 WHERE EXISTS (SELECT 1 FROM archives a \
                               WHERE a.id = ?1 AND a.absolute_path_cached = ?3) \
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

/// One saved row reduced to what the rename name rule needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedDatNameRow {
    pub dat_source_id: String,
    pub status: SavedDatEvidenceStatus,
    /// The saved verdict column (`verified_single_match`, `no_match`, ...).
    pub saved_verdict: String,
    pub canonical_rom_name: Option<String>,
    pub canonical_dat_name: Option<String>,
    /// The match was made through one member of an outer archive.
    pub through_archive_member: bool,
}

/// Whether a library item has one verified catalogue name that a rename may
/// use. Only current, unique, single-match evidence qualifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedDatName {
    /// No saved check names this item.
    None,
    /// A trustworthy name: the DAT `<rom>` name (with its own extension).
    Usable {
        rom_name: String,
        game_name: String,
        dat_source_id: String,
        /// The name belongs to a file inside this archive, not the archive.
        through_archive_member: bool,
    },
    /// Saved evidence exists but must not be used; the text says why.
    NotUsable(String),
}

fn status_reason(status: SavedDatEvidenceStatus) -> &'static str {
    use SavedDatEvidenceStatus as S;
    match status {
        S::Current => "the saved check is current",
        S::MissingProof => {
            "the saved check is from before checks were recorded in full; check the game again"
        }
        S::FileChanged => "the file has changed since it was checked; check the game again",
        S::FileUnavailable => "the file is missing or unreadable",
        S::CatalogueChanged => {
            "the catalogue has changed since the game was checked; check the game again"
        }
        S::CatalogueUnavailable => "the catalogue that checked this game is not available now",
        S::DoesNotApply => "the saved check does not apply to this game",
    }
}

/// The single rule for a verified rename name, on already-judged rows.
pub fn decide_verified_dat_name(rows: &[SavedDatNameRow]) -> VerifiedDatName {
    use SavedDatEvidenceStatus as S;
    let current: Vec<&SavedDatNameRow> = rows.iter().filter(|r| r.status == S::Current).collect();
    if current.iter().any(|r| {
        matches!(
            r.saved_verdict.as_str(),
            "conflicting" | "ambiguous_multiple_candidates"
        )
    }) {
        return VerifiedDatName::NotUsable(
            "a catalogue check found more than one possible match".into(),
        );
    }
    let verified: Vec<&&SavedDatNameRow> = current
        .iter()
        .filter(|r| r.saved_verdict == "verified_single_match")
        .collect();
    if verified.is_empty() {
        return match rows
            .iter()
            .find(|r| r.saved_verdict == "verified_single_match")
        {
            Some(stale) => VerifiedDatName::NotUsable(status_reason(stale.status).into()),
            None => VerifiedDatName::None,
        };
    }
    let mut names: Vec<(&str, &str, &str, bool)> = Vec::new();
    for row in &verified {
        let (Some(rom), Some(game)) = (
            row.canonical_rom_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty()),
            row.canonical_dat_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty()),
        ) else {
            return VerifiedDatName::NotUsable(
                "the saved match does not record a single catalogue file name".into(),
            );
        };
        match names.iter_mut().find(|(r, ..)| *r == rom) {
            Some(existing) => existing.3 |= row.through_archive_member,
            None => names.push((
                rom,
                game,
                row.dat_source_id.as_str(),
                row.through_archive_member,
            )),
        }
    }
    if names.len() > 1 {
        return VerifiedDatName::NotUsable(format!(
            "catalogues disagree on the name: {}",
            names
                .iter()
                .map(|(rom, _, source, _)| format!("'{rom}' ({source})"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let (rom, game, source, member) = names[0];
    VerifiedDatName::Usable {
        rom_name: rom.to_string(),
        game_name: game.to_string(),
        dat_source_id: source.to_string(),
        through_archive_member: member,
    }
}

impl Database {
    /// The verified catalogue name for one library item, judged by the same
    /// currency rule as every other consumer (live file state, catalogue
    /// digest, revision marks). Read-only; no matching or hashing of ROMs.
    pub fn verified_dat_name_for_archive(
        &self,
        archive_id: i64,
        catalogues: &HashMap<String, ConfiguredCatalogue>,
    ) -> Result<VerifiedDatName> {
        let mut statement = self.connection.prepare(
            "SELECT d.dat_source_id, d.facts_json, a.size_bytes, p.platform, a.absolute_path_cached, \
                    d.revision_marked_stale, a.last_verified_missing_at IS NOT NULL, d.verification_state \
             FROM library_dat_identities d JOIN archives a ON a.id = d.archive_id \
             LEFT JOIN platform_assignments p ON p.archive_id = a.id AND p.is_current = 1 \
             WHERE d.archive_id = ?1 ORDER BY d.dat_source_id",
        ).map_err(|error| db_error("failed to prepare verified DAT name query", error))?;
        let mut rows = statement
            .query(params![archive_id])
            .map_err(|error| db_error("failed to query verified DAT name", error))?;
        let mut judged = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|error| db_error("failed to read verified DAT name", error))?
        {
            let read = |error| db_error("failed to read verified DAT name", error);
            let dat_source_id: String = row.get(0).map_err(read)?;
            let bytes: Vec<u8> = row.get(1).map_err(read)?;
            let size_bytes: Option<i64> = row.get(2).map_err(read)?;
            let platform: Option<String> = row.get(3).map_err(read)?;
            let path_bytes: Vec<u8> = row.get(4).map_err(read)?;
            let marked_stale: bool = row.get(5).map_err(read)?;
            let library_missing: bool = row.get(6).map_err(read)?;
            let saved_verdict: String = row.get(7).map_err(read)?;
            // Unreadable saved evidence is never a name; it is reported.
            let Ok(saved) = serde_json::from_slice::<
                crate::dat::library_identity_summary::PersistedLibraryDatIdentity,
            >(&bytes) else {
                return Ok(VerifiedDatName::NotUsable(
                    "the saved check for this game is unreadable".into(),
                ));
            };
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
            judged.push(SavedDatNameRow {
                dat_source_id,
                status,
                saved_verdict,
                canonical_rom_name: saved.canonical.canonical_rom_name.clone(),
                canonical_dat_name: saved.canonical.canonical_dat_name.clone(),
                through_archive_member: saved.archive_member.is_some(),
            });
        }
        Ok(decide_verified_dat_name(&judged))
    }
}

impl Database {
    /// Which archives have any saved DAT row, and every catalogue ID those
    /// rows name. Lets a caller skip items with nothing saved and read only
    /// the catalogues saved rows can refer to. Metadata only.
    pub fn saved_dat_row_index(
        &self,
    ) -> Result<(
        std::collections::HashSet<i64>,
        std::collections::HashSet<String>,
    )> {
        let mut statement = self
            .connection
            .prepare("SELECT archive_id, dat_source_id FROM library_dat_identities")
            .map_err(|error| db_error("failed to prepare saved DAT row index", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| db_error("failed to query saved DAT row index", error))?;
        let mut archives = std::collections::HashSet::new();
        let mut sources = std::collections::HashSet::new();
        for row in rows {
            let (archive, source) =
                row.map_err(|error| db_error("failed to read saved DAT row index", error))?;
            archives.insert(archive);
            sources.insert(source);
        }
        Ok((archives, sources))
    }
}
