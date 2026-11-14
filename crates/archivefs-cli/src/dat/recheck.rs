//! `dat recheck`: lists saved DAT results that are no longer backed by
//! current evidence and, with `--confirm`, checks those games again.
//!
//! Without `--confirm` nothing is hashed and nothing is written. With it,
//! game files are read (never changed) and the library database is updated
//! one bounded set at a time. Ctrl-C stops the run: the set being read is
//! given up, every finished set stays saved, and running the command again
//! continues with the rest.
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
        ReauditBlock::LeftOutOfThisRun => "left out of this run by --limit",
    }
}

#[derive(Serialize)]
struct TallyJson {
    saved_results: usize,
    current: usize,
    not_applicable: usize,
    skipped: usize,
    eligible: usize,
    completed: usize,
    verified: usize,
    no_match: usize,
    need_choice: usize,
    failed: usize,
    not_reached: usize,
}

fn tally_json(tally: archivefs_core::dat::reaudit::ReauditTally) -> TallyJson {
    TallyJson {
        saved_results: tally.saved_results,
        current: tally.current,
        not_applicable: tally.not_applicable,
        skipped: tally.skipped,
        eligible: tally.eligible,
        completed: tally.completed,
        verified: tally.verified,
        no_match: tally.no_match,
        need_choice: tally.need_choice,
        failed: tally.failed,
        not_reached: tally.not_reached,
    }
}

fn print_tally(tally: &TallyJson, after_run: bool) {
    println!("Saved DAT results: {}", tally.saved_results);
    println!(
        "  {:>7}  current         already backed by a current check; not read again",
        tally.current
    );
    println!(
        "  {:>7}  not applicable  historical results that say nothing about their game (for example a no-match from another system's catalogue); kept as they are",
        tally.not_applicable
    );
    println!(
        "  {:>7}  skipped         cannot be checked now (catalogue unavailable, system not known, file not reachable, or left out by --limit)",
        tally.skipped
    );
    println!(
        "  {:>7}  eligible        lost their proof and can be checked again by reading the game",
        tally.eligible
    );
    if after_run {
        println!("  of the eligible:");
        println!(
            "  {:>7}  completed       a definitive result was reached and saved",
            tally.completed
        );
        println!(
            "  {:>7}    verified      the file matches one catalogue entry exactly",
            tally.verified
        );
        println!(
            "  {:>7}    no match      the file is not in the catalogue",
            tally.no_match
        );
        println!(
            "  {:>7}    need a choice more than one catalogue entry fits",
            tally.need_choice
        );
        println!(
            "  {:>7}  failed          looked at, no result could be saved (reasons below); unchanged",
            tally.failed
        );
        println!(
            "  {:>7}  not reached     the run stopped before them",
            tally.not_reached
        );
    }
}

#[derive(Serialize)]
struct PlanJson {
    tally: TallyJson,
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
    tally: TallyJson,
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
    warnings: Vec<String>,
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
        tally: tally_json(plan.tally()),
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
    print_tally(&plan.tally, false);
    println!(
        "Eligible: {} ({:.1} GiB by recorded size)",
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
    println!("Not eligible ({cannot}), by reason:");
    for (reason, count) in &plan.cannot_check {
        println!("    {count:>7}  {reason}");
    }
}

fn report_json(report: &ReauditReport, plan: &ReauditPlan, seconds: f64) -> ReportJson {
    ReportJson {
        tally: tally_json(report.tally(plan)),
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
        warnings: report.warnings.clone(),
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
        Some(path) => std::path::absolute(path)?,
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
        libc::signal(
            libc::SIGINT,
            on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
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
    let output = report_json(&report, &plan, seconds);
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    println!(
        "Checked again in {seconds:.1} s; {} rows saved in {} sets.",
        output.saved, output.sets_saved
    );
    print_tally(&output.tally, true);
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
    for warning in &output.warnings {
        println!("Note: {warning}");
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
