//! Read-only row loader for collection reconciliation.
//!
//! Reconciliation needs three things the general archive loader does not
//! carry: when a row was first seen, the platform of a row whose assignment
//! was retired when it went missing, and every source folder including the
//! ones no longer configured. Nothing here writes.

use super::*;

/// One library row, as much of it as reconciliation reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionRecordRow {
    pub id: i64,
    pub source_folder_id: i64,
    pub absolute_path: PathBuf,
    pub archive_kind: String,
    pub size_bytes: Option<u64>,
    pub modified_time_unix_seconds: Option<i64>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub last_verified_missing_at: Option<String>,
    /// The current platform, or the most recent one if none is current.
    pub platform: Option<String>,
}

/// One library scan, as the library recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionScanRun {
    pub started_at: String,
    pub finished_at: String,
    /// `completed`, `interrupted`, and so on.
    pub status: String,
    /// Per library folder, the coverage state the scan recorded (`complete`,
    /// `removed`, ...). `None` for scans from before coverage was recorded.
    pub coverage: Option<Vec<(i64, String)>>,
}

/// A source folder, configured or retired, and how many rows it still owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionSourceFolder {
    pub id: i64,
    pub path: PathBuf,
    pub configured: bool,
    pub rows: u64,
}

impl Database {
    /// Every library row, ordered by id.
    pub fn collection_record_rows(&self) -> Result<Vec<CollectionRecordRow>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT a.id, a.source_folder_id, a.absolute_path_cached, a.archive_kind, \
                        a.size_bytes, a.modified_time_unix_seconds, a.first_seen_at, \
                        a.last_seen_at, a.last_verified_missing_at, \
                        (SELECT p.platform FROM platform_assignments p \
                          WHERE p.archive_id = a.id \
                          ORDER BY p.is_current DESC, p.id DESC LIMIT 1) \
                 FROM archives a ORDER BY a.id",
            )
            .map_err(|error| db_error("failed to prepare collection rows", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok(CollectionRecordRow {
                    id: row.get(0)?,
                    source_folder_id: row.get(1)?,
                    absolute_path: PathBuf::from(OsString::from_vec(row.get::<_, Vec<u8>>(2)?)),
                    archive_kind: row.get(3)?,
                    size_bytes: row
                        .get::<_, Option<i64>>(4)?
                        .map(|value| value.max(0) as u64),
                    modified_time_unix_seconds: row.get(5)?,
                    first_seen_at: row.get(6)?,
                    last_seen_at: row.get(7)?,
                    last_verified_missing_at: row.get(8)?,
                    platform: row.get(9)?,
                })
            })
            .map_err(|error| db_error("failed to read collection rows", error))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| db_error("failed to read a collection row", error))
    }

    /// Every scan run that has a finish time, oldest first, with what the
    /// library recorded about the library folders it covered.
    pub fn collection_scan_runs(&self) -> Result<Vec<CollectionScanRun>> {
        let mut coverage: std::collections::HashMap<i64, Vec<(i64, String)>> =
            std::collections::HashMap::new();
        {
            let mut statement = self
                .connection
                .prepare("SELECT scan_run_id, source_folder_id, state FROM scan_source_coverage")
                .map_err(|error| db_error("failed to prepare collection scan coverage", error))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| db_error("failed to read collection scan coverage", error))?;
            for row in rows {
                let (run, source, state) = row.map_err(|error| {
                    db_error("failed to read a collection scan coverage row", error)
                })?;
                // The state is stored as a JSON string.
                let state = state.trim_matches('"').to_string();
                coverage.entry(run).or_default().push((source, state));
            }
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, started_at, finished_at, status FROM scan_runs \
                 WHERE finished_at IS NOT NULL ORDER BY id",
            )
            .map_err(|error| db_error("failed to prepare collection scan runs", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|error| db_error("failed to read collection scan runs", error))?;
        let mut runs = Vec::new();
        for row in rows {
            let (id, started_at, finished_at, status) =
                row.map_err(|error| db_error("failed to read a collection scan run", error))?;
            runs.push(CollectionScanRun {
                started_at,
                finished_at,
                status,
                coverage: coverage.remove(&id),
            });
        }
        Ok(runs)
    }

    /// Every source folder the library has ever had, with its row count.
    pub fn collection_source_folders(&self) -> Result<Vec<CollectionSourceFolder>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT s.id, s.path, s.removed_from_config_at IS NULL, \
                        (SELECT COUNT(*) FROM archives a WHERE a.source_folder_id = s.id) \
                 FROM source_folders s ORDER BY s.id",
            )
            .map_err(|error| db_error("failed to prepare collection sources", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok(CollectionSourceFolder {
                    id: row.get(0)?,
                    path: PathBuf::from(OsString::from_vec(row.get::<_, Vec<u8>>(1)?)),
                    configured: row.get(2)?,
                    rows: row.get::<_, i64>(3)?.max(0) as u64,
                })
            })
            .map_err(|error| db_error("failed to read collection sources", error))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| db_error("failed to read a collection source", error))
    }
}
