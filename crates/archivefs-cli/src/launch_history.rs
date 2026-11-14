//! `launch-history`: look at, remove and switch off the record EmuWiz keeps
//! of launches that went wrong.
//!
//! A thin front end over `archivefs_core::daedalus::history`. Listing and
//! showing change nothing. Deleting, clearing and switching touch only
//! EmuWiz's own `launch-diagnostics` folder: no database, no emulator
//! settings, no game file. The command starts as a reader whatever it is
//! asked (`is_read_only_invocation` in `main.rs`), so it never opens the Undo
//! seal store or the library database on the way in either.
use std::time::SystemTime;

use archivefs_core::daedalus::history::{FailureRecord, HistoryError, HistoryStore, readable_time};

pub const HELP: &str = "  launch-history [list] [--json]
                             Show the launches EmuWiz kept a record of: failed launches, and
                             launches whose emulator said a save could not be written. Read-only.
                             A record holds structured facts only; no emulator output is stored.
  launch-history show <id> [--json]
                             Show one record in full. Read-only.
  launch-history status      Say whether recording is on, and the limits. Read-only.
  launch-history delete <id> Remove one record.
  launch-history clear       Remove every record. The on/off switch is kept.
  launch-history off | on    Stop or resume recording. Switching off keeps what is stored.
  launch-history --help      Show this help.";

#[derive(Debug, PartialEq, Eq)]
enum Request {
    Help,
    List { json: bool },
    Show { id: String, json: bool },
    Status,
    Delete { id: String },
    Clear,
    Switch { on: bool },
}

fn usage(problem: impl std::fmt::Display) -> String {
    format!("{problem}\n\n{HELP}")
}

fn parse(args: &[String]) -> Result<Request, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let (json, words): (Vec<&str>, Vec<&str>) =
        words.into_iter().partition(|word| *word == "--json");
    let json = match json.len() {
        0 => false,
        1 => true,
        _ => return Err(usage("--json was given more than once")),
    };
    let no_json = |request: Request| {
        if json {
            Err(usage("--json applies to list and show only"))
        } else {
            Ok(request)
        }
    };
    match words.as_slice() {
        ["--help" | "-h" | "help"] => no_json(Request::Help),
        [] | ["list"] => Ok(Request::List { json }),
        ["show", id] => Ok(Request::Show {
            id: (*id).to_string(),
            json,
        }),
        ["status"] => no_json(Request::Status),
        ["delete", id] => no_json(Request::Delete {
            id: (*id).to_string(),
        }),
        ["clear"] => no_json(Request::Clear),
        ["on"] => no_json(Request::Switch { on: true }),
        ["off"] => no_json(Request::Switch { on: false }),
        ["show" | "delete"] => Err(usage(
            "a record id is needed; `launch-history list` shows them",
        )),
        _ => Err(usage("not a launch-history command")),
    }
}

fn headline(record: &FailureRecord) -> String {
    let saves = record.saves.assessment();
    // The launch outcome is always said; a save warning is added beside it.
    let launch = format!("launch: {}", record.launch.describe());
    let what = match saves.problem() {
        Some(problem) if saves.is_write_failure() => {
            format!("{launch}; save warning: {}", problem.headline())
        }
        _ => launch,
    };
    let repeats = match record.occurrences {
        0 | 1 => String::new(),
        times => format!(" ({times} times)"),
    };
    format!(
        "{}  {}  {} on {}: {what}{repeats}",
        record.id,
        readable_time(record.recorded_at_unix),
        record.game_label(),
        record.emulator_label()
    )
}

fn format_list(store: &HistoryStore, now: SystemTime) -> String {
    let listing = store.view(now);
    let mut out =
        String::from("EmuWiz launch history (structured records; no emulator output is kept)\n");
    out.push_str(if store.is_enabled() {
        "Recording is on.\n"
    } else {
        "Recording is off. Nothing new is kept.\n"
    });
    if listing.records.is_empty() {
        out.push_str("\nNo launch is on record.\n");
    } else {
        out.push('\n');
        for record in &listing.records {
            out.push_str(&headline(record));
            out.push('\n');
        }
        out.push_str("\n`launch-history show <id>` shows one in full.\n");
    }
    if listing.unreadable > 0 {
        out.push_str(&format!(
            "{} file(s) in the history folder are not readable records and are not shown.\n",
            listing.unreadable
        ));
    }
    out
}

fn format_status(store: &HistoryStore, now: SystemTime) -> String {
    let limits = store.limits();
    format!(
        "Recording is {}.\nKept: {} record(s).\nLimits: the newest {} records, none older than {} days, {} KiB in total.\n\
         Records hold structured facts only: no emulator output, paths or addresses.",
        if store.is_enabled() { "on" } else { "off" },
        store.view(now).records.len(),
        limits.max_records,
        limits.max_age.as_secs() / 86_400,
        limits.quota_bytes / 1024
    )
}

fn describe_error(error: &HistoryError) -> String {
    match error {
        HistoryError::NotFound => "no record has that id".to_string(),
        HistoryError::InvalidId => "that is not a record id".to_string(),
        other => other.to_string(),
    }
}

fn execute(request: Request, store: &HistoryStore, now: SystemTime) -> Result<String, String> {
    Ok(match request {
        Request::Help => HELP.to_string(),
        Request::List { json: false } => format_list(store, now),
        Request::List { json: true } => {
            let listing = store.view(now);
            serde_json::to_string_pretty(&serde_json::json!({
                "recording": store.is_enabled(),
                "records": listing.records,
                "unreadable": listing.unreadable,
            }))
            .map_err(|error| error.to_string())?
        }
        Request::Show { id, json } => {
            let record = store
                .load(&id, now)
                .map_err(|error| describe_error(&error))?;
            if json {
                serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?
            } else {
                record.describe().join("\n")
            }
        }
        Request::Status => format_status(store, now),
        Request::Delete { id } => {
            if store.delete(&id).map_err(|error| describe_error(&error))? {
                "Removed.".to_string()
            } else {
                return Err("no record has that id".to_string());
            }
        }
        Request::Clear => {
            let removed = store.clear().map_err(|error| describe_error(&error))?;
            format!("Removed {removed} record(s).")
        }
        Request::Switch { on } => {
            store
                .set_enabled(on)
                .map_err(|error| describe_error(&error))?;
            if on {
                "Recording is on.".to_string()
            } else {
                "Recording is off. What is already stored is kept; `launch-history clear` \
                 removes it."
                    .to_string()
            }
        }
    })
}

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let request = parse(&args)?;
    if request == Request::Help {
        println!("{HELP}");
        return Ok(());
    }
    let store = HistoryStore::open_default().map_err(|error| describe_error(&error))?;
    println!("{}", execute(request, &store, SystemTime::now())?);
    Ok(())
}

#[cfg(test)]
#[path = "launch_history/tests.rs"]
mod tests;
