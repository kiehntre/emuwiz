//! `emuwiz-cli collection reconcile`: what happened to library records whose
//! file is not where the library last saw it.
//!
//! Read-only. It opens the library database read-only, lists the library
//! folders with metadata calls only, and never changes a record, a "file not
//! found" flag, a link or a game file. It writes nothing and keeps nothing
//! between runs.

use archivefs_core::collection_intelligence::{
    EvidenceClass, EvidenceTier, LinkGrade, Progress, ReconcileRequest, ReconciliationReport,
    RecordSet, reconcile,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const USAGE: &str = "\
emuwiz-cli collection reconcile [options]

Explains library records whose file is not at its recorded path: rows flagged
\"file not found\", rows that are not flagged but have no file, and rows removed
by Forget Missing. Also checks every symbolic link in the library folders.
Nothing is changed.

  --json                    the whole report as JSON (docs/json-api.md)
  --records                 include every record (JSON), or list them (text)
  --links                   include every link (JSON), or list broken ones (text)
  --class <level>           only records at this level: confirmed, strong,
                            metadata, weak, present, candidate, unresolved
  --limit <n>               at most n records or links in the text listing
  --verify-content          also read files that have an independently recorded
                            hash for their old path, to confirm their contents
  --forget-receipt <file>   also read this Forget Missing export (repeatable)
  --database <file>         read this library database instead of the usual one

A matching name, size and date is reported as a metadata match. It is not
proof that two files hold the same data.";

type Outcome = Result<(), Box<dyn std::error::Error>>;

fn absolute(path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|dir| dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn class_from(text: &str) -> Option<EvidenceClass> {
    EvidenceClass::ALL
        .into_iter()
        .find(|class| format!("{class:?}").eq_ignore_ascii_case(text))
}

pub fn run(args: Vec<String>) -> Outcome {
    let mut args = args.into_iter();
    let command = args.next();
    // Asking for help must only ever print help.
    if matches!(command.as_deref(), None | Some("--help" | "-h" | "help")) {
        println!("{USAGE}");
        return Ok(());
    }
    if command.as_deref() != Some("reconcile") {
        return Err(format!(
            "unknown collection command {:?}\n\n{USAGE}",
            command.unwrap_or_default()
        )
        .into());
    }
    let (mut json, mut records, mut links) = (false, false, false);
    let mut class = None;
    let mut limit = 40usize;
    let mut database = None;
    let mut verify = false;
    let mut receipts = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg} needs {what}"));
        match arg.as_str() {
            "--help" | "-h" | "help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--json" => json = true,
            "--records" => records = true,
            "--links" => links = true,
            "--verify-content" => verify = true,
            "--class" => {
                let text = value("a level")?;
                class =
                    Some(class_from(&text).ok_or(format!("unknown level {text:?}\n\n{USAGE}"))?);
            }
            "--limit" => {
                limit = value("a number")?
                    .parse()
                    .map_err(|_| "--limit needs a number")?
            }
            "--forget-receipt" => receipts.push(absolute(&value("a file")?)),
            "--database" => database = Some(absolute(&value("a file")?)),
            other => return Err(format!("unknown option {other:?}\n\n{USAGE}").into()),
        }
    }
    let database = match database {
        Some(path) => path,
        None => archivefs_core::default_database_path()?,
    };
    let mut request = ReconcileRequest::new(database);
    request.forget_receipts = receipts;
    request.verify_content = verify;

    let cancel = AtomicBool::new(false);
    let mut last_phase = "";
    let mut report = reconcile(&request, &cancel, &mut |progress: Progress| {
        if !json && progress.phase != last_phase {
            last_phase = progress.phase;
            eprintln!("{}…", progress.phase);
        }
    })?;

    if json {
        if !records {
            report.records.clear();
        } else if let Some(class) = class {
            report.records.retain(|record| record.class == class);
        }
        if !links {
            report.link_findings.clear();
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    print_report(&report, records, links, class, limit);
    Ok(())
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

fn print_report(
    report: &ReconciliationReport,
    records: bool,
    links: bool,
    class: Option<EvidenceClass>,
    limit: usize,
) {
    let m = &report.measures;
    println!("Library records (these counts are separate; do not add them together)");
    println!(
        "  rows in the library                    {}",
        m.rows_in_library
    );
    println!(
        "  file present exactly as recorded       {}",
        m.rows_present_as_recorded
    );
    println!(
        "  file present but changed since scanned {}",
        m.rows_present_but_changed
    );
    println!(
        "  game folders present                   {}",
        m.rows_present_folders
    );
    println!(
        "  flagged \"file not found\"               {}",
        m.rows_flagged_missing
    );
    println!(
        "  not flagged, but no file               {}",
        m.rows_absent_never_flagged
    );
    if m.rows_not_checked > 0 {
        println!(
            "  not checked (see the notes below)      {}",
            m.rows_not_checked
        );
    }
    if m.forgotten_records_not_checked > 0 {
        println!(
            "  removed earlier, not checked           {}",
            m.forgotten_records_not_checked
        );
    }
    println!(
        "  removed earlier by Forget Missing      {}",
        m.records_removed_by_forget_missing
    );
    println!(
        "  distinct files behind present rows     {} ({})",
        m.distinct_files_behind_present_rows,
        gib(m.bytes_of_distinct_present_files)
    );
    println!();
    println!(
        "What became of the {} records without a file at their path",
        m.records_examined
    );
    println!(
        "  {:<5} {:>9} {:>9} {:>9}  evidence",
        "", "flagged", "no flag", "forgotten"
    );
    for tier in EvidenceTier::ALL {
        let count = |set| {
            report
                .record_sets
                .get(&set)
                .and_then(|tiers| tiers.get(tier.code()))
                .map_or(0, |count| count.records)
        };
        let row = [
            count(RecordSet::FlaggedMissing),
            count(RecordSet::AbsentNeverFlagged),
            count(RecordSet::Forgotten),
        ];
        if row.iter().sum::<u64>() == 0 {
            continue;
        }
        println!(
            "  {:<5} {:>9} {:>9} {:>9}  {}: {}",
            tier.code(),
            row[0],
            row[1],
            row[2],
            tier.class().label(),
            tier.explanation()
        );
    }
    println!(
        "  tied to a file that exists now: {} records, {} distinct files",
        m.records_tied_to_a_file, m.distinct_files_reached
    );
    println!();
    let l = &report.links;
    println!("Symbolic links in the library folders (not library records)");
    println!(
        "  links {}, working {}, broken {}",
        l.total, l.resolve, l.broken
    );
    println!(
        "  broken because the storage is not there {}, because the file is gone {}",
        l.storage_unavailable, l.target_missing
    );
    if m.links_that_are_library_rows > 0 {
        println!(
            "  {} links are also library rows and are counted above as records, not twice",
            m.links_that_are_library_rows
        );
    }
    println!(
        "  broken: target identified {}, plausible candidate {}, unresolved {}",
        l.target_identified, l.plausible_candidate, l.unresolved
    );
    for group in l
        .groups
        .iter()
        .filter(|group| group.grade != LinkGrade::Resolves)
        .take(8)
    {
        println!(
            "    {:>6}  {} -> {}  ({}; {})",
            group.links,
            group.link_folder.display(),
            group.target_root.display(),
            group.grade.label(),
            group.cause.map_or("", |cause| cause.label())
        );
    }
    if report.content_check.requested {
        let c = &report.content_check;
        println!();
        println!(
            "Content check: {} compared, {} match, {} differ, {} not comparable ({} bytes read)",
            c.compared, c.matched, c.differed, c.not_comparable, c.bytes_read
        );
    }
    println!();
    let i = &report.inventory;
    println!(
        "Folders listed: {} entries in {} folders, in {} s",
        i.entries, i.directories_listed, i.seconds
    );
    for note in &report.notes {
        println!("Note: {note}");
    }
    if records {
        println!();
        let chosen: Vec<_> = report
            .records
            .iter()
            .filter(|record| class.is_none_or(|class| record.class == class))
            .collect();
        for record in chosen.iter().take(limit) {
            println!(
                "{} [{}] {}",
                record.code,
                record.class.label(),
                record.old_path.display()
            );
            if let Some(target) = &record.target {
                println!("    -> {}", target.display());
            }
            for line in &record.evidence {
                println!("    {line}");
            }
        }
        if chosen.len() > limit {
            println!(
                "… {} more (use --limit, or --json --records)",
                chosen.len() - limit
            );
        }
    }
    if links {
        println!();
        let broken: Vec<_> = report
            .link_findings
            .iter()
            .filter(|link| link.grade != LinkGrade::Resolves)
            .collect();
        for link in broken.iter().take(limit) {
            println!("{} {}", link.grade.label(), link.link.display());
            println!("    points at {}", link.resolved_target.display());
            if let Some(found) = &link.found {
                println!("    found     {}", found.display());
            }
        }
        if broken.len() > limit {
            println!(
                "… {} more (use --limit, or --json --links)",
                broken.len() - limit
            );
        }
    }
    println!();
    println!("Nothing was changed.");
}
