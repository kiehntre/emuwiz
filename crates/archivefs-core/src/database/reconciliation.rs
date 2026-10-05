//! Read-only queries feeding catalogue reconciliation. Nothing here writes.
use super::*;
use crate::catalogue_reconciliation::{StrongHash, StrongHashAlgorithm};

/// One registered source folder, as reconciliation needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconciliationSource {
    pub id: i64,
    pub path: PathBuf,
    pub removed_from_config: bool,
    pub games_role: bool,
}

impl Database {
    pub(crate) fn reconciliation_sources(&self) -> Result<Vec<ReconciliationSource>> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT id, path, removed_from_config_at, source_role \
                 FROM source_folders ORDER BY id",
            )
            .map_err(|e| db_error("prepare reconciliation sources", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| db_error("read reconciliation sources", e))?;
        let mut sources = Vec::new();
        for row in rows {
            let (id, path, removed, role) =
                row.map_err(|e| db_error("decode reconciliation source", e))?;
            sources.push(ReconciliationSource {
                id,
                path: PathBuf::from(OsString::from_vec(path)),
                removed_from_config: removed.is_some(),
                games_role: role.eq_ignore_ascii_case("games"),
            });
        }
        Ok(sources)
    }

    /// Strong hashes the catalogue already holds per row: `archive_hash`, and
    /// the SHA-1/SHA-256 an earlier DAT audit compared. Nothing is hashed.
    pub(crate) fn reconciliation_row_hashes(&self) -> Result<HashMap<i64, Vec<StrongHash>>> {
        let mut hashes: HashMap<i64, Vec<StrongHash>> = HashMap::new();
        let mut add = |id: i64, algorithm, value: Option<&str>| {
            if let Some(hash) = value.and_then(|v| StrongHash::new(algorithm, v)) {
                let list = hashes.entry(id).or_default();
                if !list.contains(&hash) {
                    list.push(hash);
                }
            }
        };
        let mut stmt = self
            .connection
            .prepare("SELECT id, archive_hash FROM archives WHERE archive_hash IS NOT NULL")
            .map_err(|e| db_error("prepare row hash evidence", e))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| db_error("read row hash evidence", e))?;
        for row in rows {
            let (id, hash) = row.map_err(|e| db_error("decode row hash evidence", e))?;
            add(id, StrongHashAlgorithm::Sha256, Some(&hash));
        }
        drop(stmt);
        let mut stmt = self
            .connection
            .prepare("SELECT archive_id, facts_json FROM library_dat_identities")
            .map_err(|e| db_error("prepare audited hash evidence", e))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
            .map_err(|e| db_error("read audited hash evidence", e))?;
        for row in rows {
            let (id, bytes) = row.map_err(|e| db_error("decode audited hash evidence", e))?;
            // An unreadable record contributes no evidence rather than failing.
            if let Ok(identity) = serde_json::from_slice::<
                crate::dat::library_identity_summary::PersistedLibraryDatIdentity,
            >(&bytes)
            {
                add(
                    id,
                    StrongHashAlgorithm::Sha1,
                    identity.audited_hashes.sha1.as_deref(),
                );
                add(
                    id,
                    StrongHashAlgorithm::Sha256,
                    identity.audited_hashes.sha256.as_deref(),
                );
            }
        }
        Ok(hashes)
    }
}
