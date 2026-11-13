//! Read-only scan history: recent `scan_runs` rows exactly as recorded, with
//! each run's per-source coverage. No writes, no filesystem probes, and no
//! new state model: the run state is `scan_runs.status` verbatim.
use super::{Database, DiscoveryRunStatus, db_error};
use crate::catalogue_health::SourceScanCoverage;
use rusqlite::{OptionalExtension, params};

/// How many recent runs [`Database::recent_scan_runs`] returns at most.
pub const MAX_SCAN_HISTORY_RUNS: i64 = 50;

/// The counters a finished run wrote. Only `complete_scan_run` writes them
/// (a partial run is completed first, then relabelled), so they exist only
/// for `completed`/`partial` rows; every other row still holds the column
/// defaults, which are not measurements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedScanCounts {
    pub source_folders_scanned: i64,
    pub archives_seen: i64,
    pub archives_added: i64,
    pub archives_updated: i64,
    pub archives_missing: i64,
    pub errors_count: i64,
    /// `None` when the run started before migration 4 added the column (or
    /// that migration's time is unknown): the stored 0 is a default there.
    pub archives_unchanged: Option<i64>,
    pub skipped_unsupported_extension: Option<i64>,
    pub skipped_ambiguous_platform: Option<i64>,
}

/// What a run recorded about the sources it covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanRunCoverage {
    /// One entry per source the run recorded (migration 22 onwards).
    Recorded(Vec<SourceScanCoverage>),
    /// The run has no per-source record (older runs, or the database
    /// predates the coverage table).
    NotRecorded,
    /// Rows exist but could not be decoded; the reason is kept.
    Unreadable(String),
}

/// One `scan_runs` row as recorded. Nothing here is inferred: a missing
/// value stays `None`, and `status` is the stored text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRunRecord {
    pub scan_run_id: i64,
    pub started_at: String,
    /// For an `interrupted` row this is when a later scan noticed the
    /// unfinished run, not when that run stopped.
    pub finished_at: Option<String>,
    /// Whole seconds between `started_at` and `finished_at`, when both
    /// parse. Meaningless for `interrupted` rows; callers decide.
    pub recorded_seconds: Option<i64>,
    pub triggered_by: String,
    /// `scan_runs.status` verbatim.
    pub status: String,
    pub error_message: Option<String>,
    pub counts: Option<RecordedScanCounts>,
    pub coverage: ScanRunCoverage,
}

impl ScanRunRecord {
    /// The stored status in the existing run vocabulary, or `None` for a
    /// value this build does not know.
    pub fn state(&self) -> Option<DiscoveryRunStatus> {
        DiscoveryRunStatus::from_db_str(&self.status)
    }
}

impl Database {
    /// The newest `limit` runs (at most [`MAX_SCAN_HISTORY_RUNS`]), newest
    /// first, read in one snapshot. Read-only; safe on a read-only open.
    pub fn recent_scan_runs(&self, limit: i64) -> crate::Result<Vec<ScanRunRecord>> {
        let limit = limit.clamp(0, MAX_SCAN_HISTORY_RUNS);
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|e| db_error("start scan history read", e))?;
        let applied_at = |version: i64| -> crate::Result<Option<String>> {
            transaction
                .query_row(
                    "SELECT applied_at FROM schema_migrations WHERE version = ?1",
                    [version],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| db_error("read migration time", e))
        };
        // Columns added by later migrations hold a default for older runs.
        let skip_counts_since = applied_at(4)?;
        let has_coverage = self.schema_version()? >= 22;
        let mut statement = transaction
            .prepare(
                "SELECT id, started_at, finished_at, triggered_by, status, error_message, \
                 source_folders_scanned, archives_seen, archives_added, archives_updated, \
                 archives_missing, errors_count, archives_unchanged, \
                 skipped_unsupported_extension, skipped_ambiguous_platform, \
                 CAST(strftime('%s', finished_at) AS INTEGER) - CAST(strftime('%s', started_at) AS INTEGER) \
                 FROM scan_runs ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| db_error("prepare scan history", e))?;
        let rows = statement
            .query_map(params![limit], |row| {
                let started_at: String = row.get(1)?;
                let status: String = row.get(4)?;
                let finished = matches!(status.as_str(), "completed" | "partial");
                let since_skip_counts = skip_counts_since
                    .as_deref()
                    .is_some_and(|since| started_at.as_str() >= since);
                let later = |value: i64| since_skip_counts.then_some(value);
                let counts = if finished {
                    Some(RecordedScanCounts {
                        source_folders_scanned: row.get(6)?,
                        archives_seen: row.get(7)?,
                        archives_added: row.get(8)?,
                        archives_updated: row.get(9)?,
                        archives_missing: row.get(10)?,
                        errors_count: row.get(11)?,
                        archives_unchanged: later(row.get(12)?),
                        skipped_unsupported_extension: later(row.get(13)?),
                        skipped_ambiguous_platform: later(row.get(14)?),
                    })
                } else {
                    None
                };
                Ok(ScanRunRecord {
                    scan_run_id: row.get(0)?,
                    started_at,
                    finished_at: row.get(2)?,
                    recorded_seconds: row.get::<_, Option<i64>>(15)?.filter(|s| *s >= 0),
                    triggered_by: row.get(3)?,
                    status,
                    error_message: row.get(5)?,
                    counts,
                    coverage: ScanRunCoverage::NotRecorded,
                })
            })
            .map_err(|e| db_error("read scan history", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| db_error("decode scan history", e))?;
        drop(statement);
        let mut runs = rows;
        if has_coverage {
            for run in &mut runs {
                run.coverage = match self.scan_coverage(run.scan_run_id) {
                    Ok(coverage) if coverage.is_empty() => ScanRunCoverage::NotRecorded,
                    Ok(coverage) => ScanRunCoverage::Recorded(coverage),
                    Err(error) => ScanRunCoverage::Unreadable(error.to_string()),
                };
            }
        }
        drop(transaction);
        Ok(runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue_health::ScanCoverageState;

    fn database() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
        (dir, database)
    }

    #[test]
    fn recent_runs_keep_recorded_values_and_withhold_defaults() {
        let (_dir, database) = database();
        database
            .connection
            .execute_batch(
                "UPDATE schema_migrations SET applied_at = '2026-07-23T11:37:25Z' WHERE version = 4;
                 INSERT INTO source_folders(id, path, first_seen_at, last_seen_in_config_at)
                   VALUES (22, CAST('/games' AS BLOB), '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                 INSERT INTO scan_runs(id, started_at, finished_at, triggered_by, status, archives_seen, archives_unchanged)
                   VALUES (1, '2026-07-19T23:31:57Z', '2026-07-19T23:54:30Z', 'cli-scan-all-sources', 'completed', 98, 0);
                 INSERT INTO scan_runs(id, started_at, finished_at, triggered_by, status)
                   VALUES (2, '2026-08-01T00:00:00Z', '2026-08-01T00:10:00Z', 'cli-scan-all-sources', 'interrupted');
                 INSERT INTO scan_runs(id, started_at, finished_at, triggered_by, status, error_message)
                   VALUES (3, '2026-08-02T00:00:00Z', '2026-08-02T00:00:05Z', 'gui-scan-library', 'failed', 'disk full');
                 INSERT INTO scan_runs(id, started_at, triggered_by, status)
                   VALUES (4, '2026-08-03T00:00:00Z', 'gui-scan-library', 'running');
                 INSERT INTO scan_runs(id, started_at, finished_at, triggered_by, status, source_folders_scanned,
                   archives_seen, archives_added, archives_unchanged, skipped_unsupported_extension)
                   VALUES (5, '2026-11-13T01:13:27Z', '2026-11-13T01:41:14Z', 'cli-source-scan', 'completed', 1,
                   130466, 29776, 100690, 277261);
                 INSERT INTO scan_source_coverage(scan_run_id, source_folder_id, state, excluded_roots_json, diagnostic, root_identity_json)
                   VALUES (5, 22, '\"complete\"', '[]', NULL, 'null');",
            )
            .unwrap();
        let runs = database.recent_scan_runs(10).unwrap();
        assert_eq!(
            runs.iter().map(|r| r.scan_run_id).collect::<Vec<_>>(),
            [5, 4, 3, 2, 1]
        );
        let newest = &runs[0];
        let counts = newest.counts.as_ref().unwrap();
        assert_eq!(counts.archives_added, 29776);
        assert_eq!(counts.archives_unchanged, Some(100690));
        assert_eq!(newest.recorded_seconds, Some(27 * 60 + 47));
        match &newest.coverage {
            ScanRunCoverage::Recorded(coverage) => {
                assert_eq!(coverage.len(), 1);
                assert_eq!(coverage[0].state, ScanCoverageState::Complete);
                assert_eq!(coverage[0].root, std::path::PathBuf::from("/games"));
            }
            other => panic!("{other:?}"),
        }
        // Unfinished, failed and interrupted rows never present defaults.
        for run in &runs[1..4] {
            assert!(run.counts.is_none(), "{run:?}");
            assert_eq!(run.coverage, ScanRunCoverage::NotRecorded);
        }
        assert_eq!(runs[1].finished_at, None);
        assert_eq!(runs[1].state(), Some(DiscoveryRunStatus::Running));
        assert_eq!(runs[2].error_message.as_deref(), Some("disk full"));
        // Before migration 4, the newer columns are not measurements.
        let oldest = runs[4].counts.as_ref().unwrap();
        assert_eq!(oldest.archives_seen, 98);
        assert_eq!(oldest.archives_unchanged, None);
        assert_eq!(oldest.skipped_unsupported_extension, None);
        assert_eq!(database.recent_scan_runs(2).unwrap().len(), 2);
    }

    #[test]
    fn unknown_status_text_is_kept_verbatim() {
        let (_dir, database) = database();
        database
            .connection
            .execute_batch(
                "INSERT INTO scan_runs(started_at, triggered_by, status) VALUES ('2026-01-01T00:00:00Z', 'x', 'paused');",
            )
            .unwrap();
        let runs = database.recent_scan_runs(5).unwrap();
        assert_eq!(runs[0].status, "paused");
        assert_eq!(runs[0].state(), None);
        assert!(runs[0].counts.is_none());
        assert_eq!(runs[0].recorded_seconds, None);
    }
}
