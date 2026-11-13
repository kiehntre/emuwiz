//! `dat recheck`: lists saved DAT results that are no longer backed by
//! current evidence and, with `--confirm`, checks those games again.
//!
//! Without `--confirm` nothing is hashed and nothing is written. With it,
//! game files are read (never changed) and the library database is updated
//! one bounded set at a time; Ctrl-C stops after the current set and the
//! finished sets stay saved. Running it again continues with the rest.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use archivefs_core::dat::reaudit::{
    ReauditBlock, ReauditCatalogues, ReauditLimits, ReauditPlan, ReauditProgress, ReauditReport,
    ReauditStop, plan_reaudit, run_reaudit,
};
use archivefs_core::dat::sources::audit_run::DatAuditProgress;
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::{Config, SavedDatEvidenceStatus};
use serde::Serialize;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn status_text(status: SavedDatEvidenceStatus) -> &'static str {
    match status {
        SavedDatEvidenceStatus::Current => "current",
        SavedDatEvidenceStatus::MissingProof => "saved before proof was recorded",
        SavedDatEvidenceStatus::FileChanged => "file changed since it was checked",
        SavedDatEvidenceStatus::FileUnavailable => "file missing or storage not connected",
        SavedDatEvidenceStatus::CatalogueChanged => "catalogue changed since it was checked",
        SavedDatEvidenceStatus::CatalogueUnavailable => "catalogue not available",
        SavedDatEvidenceStatus::DoesNotApply => "saved result does not apply",
    }
}

fn block_text(block: ReauditBlock) -> &'static str {
    match block {
        ReauditBlock::CatalogueUnavailable => {
            "catalogue disabled, removed, a folder of several files, or unreadable"
        }
        ReauditBlock::SystemUnknown => "the game has no known system",
        ReauditBlock::FileUnavailable => "file missing or storage not connected",
        ReauditBlock::DoesNotApply => "saved result does not apply to this game",
    }
}

#[derive(Serialize)]
struct PlanJson {
    saved_rows: usize,
    current: usize,
    to_check: usize,
    bytes: u64,
    by_reason: BTreeMap<&'static str, usize>,
    groups: Vec<GroupJson>,
    cannot_check: BTreeMap<&'static str, usize>,
}

#[derive(Serialize)]
struct GroupJson {
    source_id: String,
    catalogue: String,
    system: String,
    games: usize,
}

#[derive(Serialize)]
struct ReportJson {
    planned: usize,
    verified: usize,
    no_match: usize,
    ambiguous: usize,
    saved: usize,
    sets_saved: usize,
    not_checked: Vec<NoteJson>,
    no_longer_matching: Vec<NoteJson>,
    not_reached: usize,
    stopped: Option<String>,
    seconds: f64,
}

#[derive(Serialize)]
struct NoteJson {
    archive_id: i64,
    source_id: String,
    path: String,
    reason: String,
}

fn plan_json(plan: &ReauditPlan) -> PlanJson {
    let mut cannot_check = BTreeMap::new();
    for blocked in &plan.blocked {
        *cannot_check.entry(block_text(blocked.block)).or_insert(0) += 1;
    }
    PlanJson {
        saved_rows: plan.saved_rows,
        current: plan.current,
        to_check: plan.games(),
        bytes: plan.bytes(),
        by_reason: plan
            .by_status()
            .into_iter()
            .map(|(status, count)| (status_text(status), count))
            .collect(),
        groups: plan
            .groups
            .iter()
            .map(|group| GroupJson {
                source_id: group.source_id.clone(),
                catalogue: group.catalogue_name.clone(),
                system: group.platform.clone(),
                games: group.items.len(),
            })
            .collect(),
        cannot_check,
    }
}

fn print_plan(plan: &PlanJson) {
    println!("Saved DAT results: {}", plan.saved_rows);
    println!("  backed by current evidence: {}", plan.current);
    println!(
        "  to check again: {} ({:.1} GiB by recorded size)",
        plan.to_check,
        plan.bytes as f64 / (1u64 << 30) as f64
    );
    for (reason, count) in &plan.by_reason {
        println!("    {count:>7}  {reason}");
    }
    for group in &plan.groups {
        println!(
            "    {:>7}  {} against {} ({})",
            group.games, group.system, group.catalogue, group.source_id
        );
    }
    let cannot: usize = plan.cannot_check.values().sum();
    println!("  cannot be checked now: {cannot}");
    for (reason, count) in &plan.cannot_check {
        println!("    {count:>7}  {reason}");
    }
}

fn report_json(report: &ReauditReport, seconds: f64) -> ReportJson {
    ReportJson {
        planned: report.planned,
        verified: report.verified,
        no_match: report.no_match,
        ambiguous: report.ambiguous,
        saved: report.saved,
        sets_saved: report.sets_saved,
        not_checked: report
            .not_checked
            .iter()
            .map(|note| NoteJson {
                archive_id: note.archive_id,
                source_id: note.source_id.clone(),
                path: note.path.display().to_string(),
                reason: note.reason.clone(),
            })
            .collect(),
        no_longer_matching: report
            .contradictions
            .iter()
            .map(|c| NoteJson {
                archive_id: c.archive_id,
                source_id: c.source_id.clone(),
                path: c.path.display().to_string(),
                reason: format!("saved as {}, now {:?}", c.saved_verdict, c.now),
            })
            .collect(),
        not_reached: report.not_reached,
        stopped: report.stopped.as_ref().map(|stop| match stop {
            ReauditStop::Cancelled => "stopped on request".to_string(),
            ReauditStop::Interrupted(reason) => reason.clone(),
        }),
        seconds,
    }
}

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut confirm = false;
    let mut json = false;
    let mut limit = None;
    let mut database = None;
    let mut games_per_set = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--confirm" => confirm = true,
            "--json" => json = true,
            "--limit" => {
                limit = Some(
                    args.next()
                        .and_then(|value| value.parse::<usize>().ok())
                        .ok_or("dat recheck: --limit needs a number of games")?,
                );
            }
            "--games-per-set" => {
                games_per_set = Some(
                    args.next()
                        .and_then(|value| value.parse::<usize>().ok())
                        .filter(|value| *value > 0)
                        .ok_or("dat recheck: --games-per-set needs a number above zero")?,
                );
            }
            "--database" => {
                database = Some(PathBuf::from(
                    args.next().ok_or("dat recheck: --database needs a path")?,
                ));
            }
            other => {
                return Err(format!(
                    "dat recheck: unknown argument {other}. Nothing was checked.\n\
                     usage: dat recheck [--confirm] [--limit <n>] [--games-per-set <n>] \
                     [--database <path>] [--json]\n\
                     Without --confirm this only lists what would be checked."
                )
                .into());
            }
        }
    }
    let database = match database {
        Some(path) => path,
        None => archivefs_core::default_database_path()?,
    };
    let catalogues = ReauditCatalogues::load_default()?;
    let mut plan = plan_reaudit(&database, &catalogues.configured())?;
    if let Some(limit) = limit {
        plan = plan.truncated(limit);
    }
    let summary = plan_json(&plan);
    if !confirm {
        if json {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        } else {
            print_plan(&summary);
            println!(
                "Nothing was read or changed. Add --confirm to check these games again; \
                 that reads the game files and updates the library database."
            );
        }
        return Ok(());
    }
    if !json {
        print_plan(&summary);
    }
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_interrupt as libc::sighandler_t);
    }
    let trusted = Config::load_default()
        .ok()
        .as_ref()
        .map(TrustedRoots::from_config)
        .unwrap_or_else(TrustedRoots::none);
    let mut limits = ReauditLimits::default();
    if let Some(games) = games_per_set {
        limits.max_games_per_set = games;
    }
    let started = std::time::Instant::now();
    let report = run_reaudit(
        &database,
        &plan,
        &|source_id| catalogues.resolve(source_id),
        &trusted,
        &STOP,
        limits,
        &|progress| match progress {
            ReauditProgress::SetStarted {
                platform,
                games,
                done,
                planned,
                ..
            } => eprintln!("[{done}/{planned}] checking {games} {platform} games"),
            ReauditProgress::SetSaved {
                verified,
                no_match,
                ambiguous,
                not_checked,
            } => eprintln!(
                "  saved: {verified} verified, {no_match} no match, {ambiguous} need review, {not_checked} not checked"
            ),
            ReauditProgress::SetNotSaved { reason, retrying } => eprintln!(
                "  not saved{}: {reason}",
                if retrying { " (trying again)" } else { "" }
            ),
            ReauditProgress::Audit(DatAuditProgress::Hashing { .. })
            | ReauditProgress::Audit(_) => {}
        },
    )?;
    let seconds = started.elapsed().as_secs_f64();
    let output = report_json(&report, seconds);
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    println!(
        "Checked again in {seconds:.1} s: {} verified, {} no match, {} need review; {} rows saved in {} sets.",
        output.verified, output.no_match, output.ambiguous, output.saved, output.sets_saved
    );
    if !output.no_longer_matching.is_empty() {
        println!(
            "{} saved matches no longer match their file (the earlier results are kept for the record):",
            output.no_longer_matching.len()
        );
        for note in output.no_longer_matching.iter().take(20) {
            println!("  {}: {}", note.path, note.reason);
        }
    }
    if !output.not_checked.is_empty() {
        println!("{} games were not checked:", output.not_checked.len());
        for note in output.not_checked.iter().take(20) {
            println!("  {}: {}", note.path, note.reason);
        }
        if output.not_checked.len() > 20 {
            println!(
                "  ... and {} more (use --json for all)",
                output.not_checked.len() - 20
            );
        }
    }
    if let Some(stopped) = &output.stopped {
        println!(
            "Stopped early ({stopped}). {} games were not reached. Finished sets are saved; \
             run the command again to continue.",
            output.not_reached
        );
    }
    println!(
        "A verified file matches its catalogue entry. That says nothing about whether an emulator will run it."
    );
    Ok(())
}
