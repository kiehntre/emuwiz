//! Shared transactional inventory writes. The digest is optional for legacy
//! validation callers, and is replaced atomically with the entry list.
use super::*;

pub(super) fn replace_inventory(
    tx: &rusqlite::Transaction<'_>,
    dat_source_id: &str,
    source_revision: Option<&str>,
    ecosystem: Option<crate::dat::model::DatEcosystem>,
    entries: &[crate::dat::expected_inventory::ExpectedDatEntryRecord],
    duplicate_names_skipped: u64,
    catalogue_sha256: Option<&str>,
) -> Result<usize> {
    use crate::dat::expected_inventory::ExpectedDatEntryMetadata;

    let ecosystem_json = ecosystem
        .map(|value| serde_json::to_string(&value))
        .transpose()
        .map_err(|error| {
            ArchiveFsError::Database(format!("failed to encode DAT ecosystem: {error}"))
        })?;
    let now = now_utc_string();

    tx.execute(
        "DELETE FROM dat_expected_entries WHERE dat_source_id = ?1",
        params![dat_source_id],
    )
    .map_err(|error| db_error("failed to clear prior expected DAT inventory", error))?;
    tx.execute(
        "INSERT INTO dat_expected_inventory_meta \
             (dat_source_id, source_revision, ecosystem, entry_count, \
              duplicate_names_skipped, validated_at, updated_at, catalogue_sha256) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7) \
             ON CONFLICT(dat_source_id) DO UPDATE SET \
              catalogue_sha256 = excluded.catalogue_sha256, \
              source_revision = excluded.source_revision, \
              ecosystem = excluded.ecosystem, \
              entry_count = excluded.entry_count, \
              duplicate_names_skipped = excluded.duplicate_names_skipped, \
              validated_at = excluded.validated_at, \
              updated_at = excluded.updated_at",
        params![
            dat_source_id,
            source_revision,
            ecosystem_json,
            entries.len() as i64,
            duplicate_names_skipped as i64,
            now,
            catalogue_sha256,
        ],
    )
    .map_err(|error| db_error("failed to replace expected DAT inventory metadata", error))?;

    let mut written = 0usize;
    {
        let mut statement = tx
            .prepare(
                "INSERT INTO dat_expected_entries \
                     (dat_source_id, canonical_identity, display_name, source_revision, \
                      ecosystem, metadata_json, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            )
            .map_err(|error| db_error("failed to prepare expected DAT inventory insert", error))?;
        for entry in entries {
            let metadata = ExpectedDatEntryMetadata {
                dat_game_id: entry.dat_game_id.clone(),
                rom_count: entry.rom_count,
            };
            let metadata_bytes = serde_json::to_vec(&metadata).map_err(|error| {
                ArchiveFsError::Database(format!(
                    "failed to encode expected DAT entry metadata: {error}"
                ))
            })?;
            statement
                .execute(params![
                    dat_source_id,
                    entry.canonical_identity,
                    entry.display_name,
                    source_revision,
                    ecosystem_json,
                    metadata_bytes,
                    now,
                ])
                .map_err(|error| db_error("failed to persist expected DAT entry", error))?;
            written += 1;
        }
    }
    Ok(written)
}

impl Database {
    pub fn expected_dat_inventory_sha256(&self, source_id: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT catalogue_sha256 FROM dat_expected_inventory_meta WHERE dat_source_id = ?1",
                params![source_id],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(|error| db_error("failed to read DAT inventory digest", error))
    }
}

impl Database {
    /// Saves only a successful complete catalogue validation, binding the
    /// inventory to the bytes consumed by that validation's streaming parser.
    pub fn persist_validated_dat_inventory(
        &mut self,
        report: &crate::dat::sources::validation::DatValidationReport,
        expected: &crate::dat::expected_inventory::ExpectedDatInventoryProjection,
    ) -> Result<usize> {
        use crate::dat::sources::{DatFileOutcome, DatHealthState};
        if !matches!(
            report.state,
            DatHealthState::Valid | DatHealthState::ValidWithWarnings
        ) || report.truncated
        {
            return Err(ArchiveFsError::Database(
                "incomplete or invalid DAT inventory was not saved".into(),
            ));
        }
        let (ecosystem, revision) = report
            .files
            .iter()
            .find_map(|file| match &file.outcome {
                DatFileOutcome::Parsed {
                    ecosystem, version, ..
                } => Some((Some(*ecosystem), version.as_deref())),
                _ => None,
            })
            .unwrap_or((None, None));
        let tx = self.connection.transaction().map_err(|error| {
            db_error("failed to start validated DAT inventory transaction", error)
        })?;
        let count = replace_inventory(
            &tx,
            &report.source_id,
            revision,
            ecosystem,
            &expected.entries,
            expected.duplicate_names_skipped as u64,
            expected.catalogue_sha256.as_deref(),
        )?;
        tx.commit()
            .map_err(|error| db_error("failed to commit validated DAT inventory", error))?;
        Ok(count)
    }
}
