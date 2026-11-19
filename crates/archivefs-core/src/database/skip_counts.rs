//! Exact skipped-file totals per scan run, source and reason (schema 30).
//!
//! These are the one source of truth for "why was this file not catalogued"
//! totals: the CLI, the scan report, scan history and Attention all read
//! them back from here. The historical `scan_runs.skipped_*` columns stay
//! written for older readers; they are coarser projections of these rows.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};

use super::{Database, db_error};
use crate::{Result, SkipReason, SkipReasonCounts};

impl Database {
    /// Stores one source's exact totals for `scan_run_id`, one row per reason
    /// (zeros included, so "recorded as 0" differs from "not recorded").
    pub(crate) fn record_scan_skip_counts(
        &self,
        scan_run_id: i64,
        source_folder_id: i64,
        counts: &SkipReasonCounts,
    ) -> Result<()> {
        // Plain statements: a scan calls this inside its own open transaction,
        // so these rows commit or roll back together with the scan. A plain
        // INSERT: a source recorded twice in one run is an error, never a
        // silent overwrite that would part the totals from the coarse columns.
        let mut statement = self
            .connection
            .prepare(
                "INSERT INTO scan_run_skip_counts(scan_run_id, source_folder_id, reason, file_count)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(|e| db_error("prepare skip counts", e))?;
        for (reason, count) in counts.entries() {
            statement
                .execute(params![
                    scan_run_id,
                    source_folder_id,
                    reason.storage_key(),
                    i64::try_from(count).unwrap_or(i64::MAX)
                ])
                .map_err(|e| db_error("record skip counts", e))?;
        }
        Ok(())
    }

    /// Exact totals per source for one run. Sources the run did not record a
    /// breakdown for are absent.
    pub fn scan_run_skip_counts_by_source(
        &self,
        scan_run_id: i64,
    ) -> Result<BTreeMap<i64, SkipReasonCounts>> {
        read_skip_counts_by_source(&self.connection, scan_run_id)
    }

    /// Exact totals for one run across its sources, or `None` when the run
    /// recorded no breakdown (for example a scan made before schema 30).
    pub fn scan_run_skip_counts(&self, scan_run_id: i64) -> Result<Option<SkipReasonCounts>> {
        read_skip_counts(&self.connection, scan_run_id)
    }
}

pub(super) fn read_skip_counts_by_source(
    connection: &rusqlite::Connection,
    scan_run_id: i64,
) -> Result<BTreeMap<i64, SkipReasonCounts>> {
    if !table_exists(connection)? {
        return Ok(BTreeMap::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT source_folder_id, reason, file_count FROM scan_run_skip_counts
             WHERE scan_run_id = ?1",
        )
        .map_err(|e| db_error("prepare skip counts", e))?;
    let rows = statement
        .query_map([scan_run_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| db_error("read skip counts", e))?;
    let mut by_source = BTreeMap::<i64, SkipReasonCounts>::new();
    for row in rows {
        let (source, reason, count) = row.map_err(|e| db_error("read skip count", e))?;
        let entry = by_source.entry(source).or_default();
        // The table's CHECK admits only these five reasons, so an unknown key
        // means a damaged file: refused rather than miscounted.
        let reason = SkipReason::from_storage_key(&reason).ok_or_else(|| {
            crate::ArchiveFsError::Database(format!(
                "scan run {scan_run_id} recorded an unknown skip reason {reason:?}"
            ))
        })?;
        entry.set(reason, count.max(0) as u64);
    }
    Ok(by_source)
}

pub(super) fn read_skip_counts(
    connection: &rusqlite::Connection,
    scan_run_id: i64,
) -> Result<Option<SkipReasonCounts>> {
    let by_source = read_skip_counts_by_source(connection, scan_run_id)?;
    if by_source.is_empty() {
        return Ok(None);
    }
    let mut total = SkipReasonCounts::default();
    for counts in by_source.values() {
        total.merge(counts);
    }
    Ok(Some(total))
}

/// Whether the table exists. It does on every database this version upgrades;
/// a read-only look at an older file reports "not recorded" instead of failing.
fn table_exists(connection: &rusqlite::Connection) -> Result<bool> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'scan_run_skip_counts'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map(|found| found.is_some())
        .map_err(|e| db_error("check skip count table", e))
}
