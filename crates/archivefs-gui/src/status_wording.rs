//! Operation status wording.
//!
//! How a completed scan and a database upgrade are phrased for the user.

use archivefs_core::{DatabaseUpgradeReport, ScanPersistSummary, ScummVmScanReport};

fn folder_names(paths: &[std::path::PathBuf]) -> String {
    let mut names: Vec<String> = paths
        .iter()
        .take(3)
        .map(|path| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        })
        .collect();
    if paths.len() > 3 {
        names.push(format!("and {} more", paths.len() - 3));
    }
    names.join(", ")
}

/// Plain-language lines about the ScummVM folder check in a scan: what was
/// found, what was deliberately left out and why. Empty when no scan ran.
pub(crate) fn scummvm_scan_lines(report: &ScummVmScanReport) -> Vec<String> {
    let mut lines = Vec::new();
    if report.sources_considered == 0 {
        return lines;
    }
    if !report.detector_available && report.errors.is_empty() {
        lines.push("ScummVM game folders: not checked, because ScummVM is not installed.".into());
        return lines;
    }
    if report.detector_available {
        let reuse = if report.cache_hits > 0 && report.detector_runs == 0 {
            " (nothing changed since the last check, so ScummVM was not asked again)"
        } else {
            ""
        };
        lines.push(format!(
            "ScummVM game folders: {} recognised{reuse}.",
            report.games_found
        ));
    }
    if !report.ambiguous.is_empty() {
        lines.push(format!(
            "Left out because ScummVM matched more than one game (EmuWiz will not guess): {}.",
            folder_names(&report.ambiguous)
        ));
    }
    if !report.rejected.is_empty() {
        lines.push(format!(
            "Left out because they did not pass EmuWiz's own folder check: {}.",
            folder_names(&report.rejected)
        ));
    }
    if !report.unknown_variants.is_empty() {
        lines.push(format!(
            "ScummVM only guessed these folders from file names (\"unknown game variant\"), so \
             they were not stored as ScummVM games; files inside them with a recognised \
             extension were catalogued on their own: {}.",
            folder_names(&report.unknown_variants)
        ));
    }
    if !report.contained_media.is_empty() {
        lines.push(format!(
            "{} recognised game file(s) sit inside confirmed ScummVM game folders and were kept \
             as part of those games rather than listed separately.",
            report.contained_media.len()
        ));
    }
    for (source, error) in &report.errors {
        lines.push(format!(
            "ScummVM could not check {}: {error}.",
            source.display()
        ));
    }
    lines
}

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
    ) + &scummvm_scan_lines(&summary.scummvm)
        .into_iter()
        .map(|line| format!("\n{line}"))
        .collect::<String>()
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
    ) + &if summary.scummvm.detector_available {
        format!(
            " ScummVM game folders: {}{}.",
            summary.scummvm.games_found,
            match summary.scummvm.ambiguous.len() + summary.scummvm.rejected.len() {
                0 => String::new(),
                left_out => format!(" ({left_out} left out; see the scan details)"),
            }
        )
    } else {
        String::new()
    }
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
        .join(" -> ");
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

#[cfg(test)]
mod scummvm_wording_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn scan_lines_explain_found_left_out_and_unavailable() {
        assert!(scummvm_scan_lines(&ScummVmScanReport::default()).is_empty());
        let missing = ScummVmScanReport {
            sources_considered: 1,
            ..Default::default()
        };
        assert!(scummvm_scan_lines(&missing)[0].contains("not installed"));
        let report = ScummVmScanReport {
            sources_considered: 1,
            detector_available: true,
            games_found: 2,
            ambiguous: vec![PathBuf::from("/g/Mixed")],
            rejected: vec![PathBuf::from("/g/Odd")],
            unknown_variants: vec![PathBuf::from("/g/Tapes")],
            contained_media: vec![PathBuf::from("/g/Queen/bonus.gba")],
            errors: vec![(PathBuf::from("/g"), "timed out".into())],
            cache_hits: 1,
            detector_runs: 0,
        };
        let text = scummvm_scan_lines(&report).join("\n");
        assert!(text.contains("2 recognised"), "{text}");
        assert!(text.contains("was not asked again"));
        assert!(text.contains("more than one game") && text.contains("Mixed"));
        assert!(text.contains("own folder check") && text.contains("Odd"));
        assert!(text.contains("only guessed these folders") && text.contains("Tapes"));
        assert!(text.contains("1 recognised game file(s) sit inside"));
        assert!(text.contains("could not check /g: timed out"));
    }
}
