//! Operation status wording.
//!
//! How a completed scan and a database upgrade are phrased for the user.

use archivefs_core::{DatabaseUpgradeReport, ScanPersistSummary};

pub(crate) fn format_scan_completion(summary: &ScanPersistSummary) -> String {
    format!(
        "Scan completed\nSeen: {}\nAdded: {}\nUpdated: {} (including {} restored)\nNewly missing: {}\nUnchanged: {}\nSkipped unsupported: {}\nSkipped ambiguous: {}\nErrors: {}",
        summary.counts.archives_seen,
        summary.counts.archives_added,
        summary.counts.archives_updated,
        summary.counts.archives_restored,
        summary.counts.archives_missing,
        summary.counts.archives_unchanged,
        summary.counts.skipped_unsupported_extension,
        summary.counts.skipped_ambiguous_platform,
        summary.counts.errors_count,
    )
}

pub(crate) fn format_scan_activity(summary: &ScanPersistSummary) -> String {
    format!(
        "Scan completed: seen {}, added {}, updated {} (including {} restored), newly missing {}, unchanged {}, skipped {}, errors {}.",
        summary.counts.archives_seen,
        summary.counts.archives_added,
        summary.counts.archives_updated,
        summary.counts.archives_restored,
        summary.counts.archives_missing,
        summary.counts.archives_unchanged,
        summary.counts.skipped_unsupported_extension + summary.counts.skipped_ambiguous_platform,
        summary.counts.errors_count,
    )
}

pub(crate) fn format_database_upgrade_success(
    report: &DatabaseUpgradeReport,
    summary: &ScanPersistSummary,
) -> String {
    let migration_chain = report
        .applied_versions
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(" → ");
    format!(
        "Library database upgraded safely from schema {} to schema {} using migrations {}. The \
         original database is recoverable from {}. {}",
        report.from_version,
        report.to_version,
        migration_chain,
        report.backup_path.display(),
        format_scan_activity(summary)
    )
}
