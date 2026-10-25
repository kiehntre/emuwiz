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
}

impl Database {
    /// Uses configured local/managed content identities plus the ordinary item
    /// freshness rules. No audit, matching or ROM hashing is performed.
    pub fn current_library_dat_evidence(
        &self,
        catalogues: &HashMap<String, ConfiguredCatalogue>,
    ) -> Result<CurrentLibraryDatEvidence> {
        let mut result = CurrentLibraryDatEvidence::default();
        let mut conflicting_hashes = std::collections::HashSet::new();
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
            let Some(source) = catalogues.get(&source_id) else {
                continue;
            };
            let saved: crate::dat::library_identity_summary::PersistedLibraryDatIdentity =
                serde_json::from_slice(&bytes).map_err(|error| {
                    ArchiveFsError::Database(format!("stored DAT evidence is unreadable: {error}"))
                })?;
            let path = PathBuf::from(std::ffi::OsString::from_vec(path_bytes));
            if saved.source.source_id != source_id
                || saved.file_state_for_path(&path) != DiscFileState::Unchanged
                || size_bytes.and_then(|size| u64::try_from(size).ok())
                    != saved.audited_hashes.size_bytes
            {
                continue;
            }
            if !saved.carries_identity()
                && !crate::dat::identity::negative_dat_result_applies(
                    &saved.source.catalogue_names,
                    saved.source.ecosystem,
                    platform.as_deref(),
                )
            {
                continue;
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
            if summary.provenance_freshness != DatProvenanceFreshness::Current {
                continue;
            }
            let state = match summary.verification_state {
                DatVerificationState::VerifiedSingleMatch { .. } => LibraryDatState::Verified,
                DatVerificationState::Probable => LibraryDatState::Probable,
                DatVerificationState::AmbiguousMultipleCandidates { .. }
                | DatVerificationState::Conflicting { .. } => LibraryDatState::Ambiguous,
                DatVerificationState::NoMatch => LibraryDatState::NoMatch,
                _ => continue,
            };
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
