//! Headless save continuity: find saves, see what ties each to a game, keep
//! verified snapshots, plan a recovery or a move to another emulator, and
//! compare two devices offline.
//!
//! Nothing here writes to a save, a memory card or an emulator folder, and
//! nothing opens the library database. The only things written are new
//! files in EmuWiz's own snapshot folder (`--store`), a new recovery folder,
//! or a new continuation folder, and the last two only with `--confirm`.
//! No command opens a network connection.

use archivefs_core::save_continuity::{
    self as continuity,
    compat::{
        ContinuationError, ContinuationTarget, assess, match_game_file, prepare_card_continuation,
    },
    device::{
        DeviceIdentity, DirectLink, LocalDevice, PeerBook, SignedManifestSet, compare,
        receive_snapshot,
    },
    discover::{
        DiscoveryOptions, DiscoveryReport, DiscoveryRoots, ProcessObserver, SAFE_POINT_EMULATORS,
        discover,
    },
    model::{DiscoveredSave, InUse},
    session::{ProcFs, SessionJournal},
    store::{CaptureOutcome, CaptureReason, SnapshotStore},
    workflow::{capture_request, timelines},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const USAGE: &str = "\
emuwiz-cli continuity <command> [--json]

Looking (reads only):
  discover [--emulator <name>]            saves in the emulators' own folders, with evidence
  inspect <save-id>                       everything known about one save
  assess <save-id> --to <emulator> [--profile native|flatpak] [--game-code <code>]
                                          can this save be continued there, and why
  match <save-id> --game <game-file>      is this the game the save was written by (by content)
  safe-points                             which emulators EmuWiz can prove are closed
  safe-point --emulator <name> <file>     is this one save file at a safe point right now
  timeline                                snapshots, oldest first, per save
  verify <snapshot-id>                    re-read a snapshot and compare with its record
  sessions                                recorded play sessions, newest first
  pruning-proposal [--keep <n>]           what could be removed; removes nothing

Making new files (never changes a save):
  snapshot <save-id> [--best-effort]      verified copy into the snapshot folder
  recover <snapshot-id> --to <new-folder> [--confirm]
                                          without --confirm: show the plan only
  continue <save-id> --onto <card> --work <new-folder> [--confirm]
                                          a new card: <card> plus that save

Two devices, offline (local folders only; no network):
  device show [--name <label>]            this profile's device identity
  device export --out <file>              signed list of this device's snapshots
  device pair <identity-or-list.json> [--confirm]
  device compare <their-list.json>        whose save is ahead; conflicts need a decision
  device receive <their-list.json> <snapshot-id> --from-store <folder> [--confirm]

Options:
  --home <folder>     look in this folder as the home directory (a scratch profile)
  --store <folder>    snapshot folder (default: EmuWiz's data folder, save-continuity)

A snapshot is \"verified\" only when the emulator was found closed and nothing had
the save open, before and after the copy. --best-effort copies anyway when that
cannot be established and records the snapshot as unverified. A running emulator
is always a refusal.";

type Outcome = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Options {
    json: bool,
    confirm: bool,
    best_effort: bool,
    home: Option<PathBuf>,
    store: Option<PathBuf>,
    to: Option<String>,
    onto: Option<PathBuf>,
    work: Option<PathBuf>,
    out: Option<PathBuf>,
    from_store: Option<PathBuf>,
    emulator: Option<String>,
    profile: Option<String>,
    game_code: Option<String>,
    game: Option<PathBuf>,
    name: Option<String>,
    keep: Option<usize>,
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|dir| dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn emit(
    options: &Options,
    command: &str,
    read_only: bool,
    body: Value,
    text: impl FnOnce(),
) -> Outcome {
    if options.json {
        let mut envelope = json!({
            "schema_version": 1,
            "command": format!("continuity {command}"),
            "read_only": read_only,
        });
        if let (Some(envelope), Value::Object(body)) = (envelope.as_object_mut(), body) {
            envelope.extend(body);
        }
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        text();
    }
    Ok(())
}

fn when(unix: i64) -> String {
    // UTC, to the minute; the JSON carries the exact number.
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (mut year, mut day) = (1970i64, days);
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let length = if leap { 366 } else { 365 };
        if day < length {
            break;
        }
        day -= length;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while day >= months[month] {
        day -= months[month];
        month += 1;
    }
    format!(
        "{year:04}-{:02}-{:02} {:02}:{:02} UTC",
        month + 1,
        day + 1,
        secs / 3600,
        secs % 3600 / 60
    )
}

fn roots(options: &Options) -> Result<DiscoveryRoots, String> {
    match &options.home {
        Some(home) => Ok(DiscoveryRoots::under_home(home)),
        None => DiscoveryRoots::current().ok_or_else(|| "HOME is not set".to_string()),
    }
}

fn find(options: &Options) -> Result<DiscoveryReport, Box<dyn std::error::Error>> {
    let observer = ProcessObserver::new();
    let mut report = discover(
        &roots(options)?,
        DiscoveryOptions {
            observer: Some(&observer),
            ..Default::default()
        },
    )?;
    if let Some(emulator) = &options.emulator {
        report
            .saves
            .retain(|save| save.emulator.eq_ignore_ascii_case(emulator));
    }
    Ok(report)
}

fn pick<'a>(report: &'a DiscoveryReport, id: &str) -> Result<&'a DiscoveredSave, String> {
    let matches: Vec<&DiscoveredSave> = report
        .saves
        .iter()
        .filter(|save| save.id.starts_with(id))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "no save with id {id}. Run `continuity discover` for the ids"
        )),
        _ => Err(format!(
            "{id} matches more than one save; give more of the id"
        )),
    }
}

fn store(options: &Options) -> Result<SnapshotStore, Box<dyn std::error::Error>> {
    let root = match &options.store {
        Some(folder) => folder.clone(),
        None => continuity::default_root()?,
    };
    Ok(SnapshotStore::open(&root)?)
}

fn device(
    options: &Options,
    store: &SnapshotStore,
) -> Result<LocalDevice, Box<dyn std::error::Error>> {
    let fallback = std::fs::read_to_string("/etc/hostname")
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|_| "This device".to_string());
    Ok(LocalDevice::open_or_create(
        store.root(),
        options.name.as_deref().unwrap_or(&fallback),
    )?)
}

fn save_line(save: &DiscoveredSave) -> String {
    let what = match (&save.entry_name, &save.identity.title) {
        (Some(name), Some(title)) => format!("{title} [{name}]"),
        (Some(name), None) => name.clone(),
        (None, _) => save
            .source_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    format!(
        "{}  {} ({})  {}\n      {} · {} · {}",
        &save.id[..8],
        save.emulator,
        save.profile,
        what,
        save.format.label(),
        save.identity.strength.label(),
        save.in_use.label()
    )
}

pub fn run(args: Vec<String>) -> Outcome {
    let mut options = Options::default();
    let mut positional = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg} needs {what}"));
        match arg.as_str() {
            // Asking for help must only ever print help.
            "--help" | "-h" | "help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--json" => options.json = true,
            "--confirm" => options.confirm = true,
            "--best-effort" => options.best_effort = true,
            "--home" => options.home = Some(absolute(Path::new(&value("a folder")?))),
            "--store" => options.store = Some(absolute(Path::new(&value("a folder")?))),
            "--to" => options.to = Some(value("a destination")?),
            "--onto" => options.onto = Some(absolute(Path::new(&value("a card")?))),
            "--work" => options.work = Some(absolute(Path::new(&value("a folder")?))),
            "--out" => options.out = Some(absolute(Path::new(&value("a file")?))),
            "--from-store" => options.from_store = Some(absolute(Path::new(&value("a folder")?))),
            "--emulator" => options.emulator = Some(value("a name")?),
            "--profile" => options.profile = Some(value("native or flatpak")?),
            "--game" => options.game = Some(absolute(Path::new(&value("a game file")?))),
            "--game-code" => options.game_code = Some(value("a product code")?),
            "--name" => options.name = Some(value("a label")?),
            "--keep" => {
                options.keep = Some(
                    value("a number")?
                        .parse()
                        .map_err(|_| "--keep needs a number")?,
                )
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown continuity option {other:?}\n\n{USAGE}").into());
            }
            _ => positional.push(arg),
        }
    }
    let mut positional = positional.into_iter();
    let Some(command) = positional.next() else {
        println!("{USAGE}");
        return Ok(());
    };
    let rest: Vec<String> = positional.collect();
    let need = |index: usize, what: &str| -> Result<&String, String> {
        rest.get(index)
            .ok_or_else(|| format!("continuity {command} needs {what}\n\n{USAGE}"))
    };
    match command.as_str() {
        "discover" => {
            let report = find(&options)?;
            let body = json!({ "saves": report.saves, "skipped": report.skipped, "stats": report.stats, "not_covered": report.not_covered });
            emit(&options, "discover", true, body, || {
                for save in &report.saves {
                    println!("{}", save_line(save));
                }
                for line in &report.skipped {
                    println!("not listed: {line}");
                }
                println!(
                    "{} save(s) found; {} file(s) examined, {} bytes read. Nothing was changed.",
                    report.saves.len(),
                    report.stats.files_examined,
                    report.stats.bytes_hashed
                );
                println!("Not looked for: {}.", report.not_covered.join("; "));
            })
        }
        "inspect" => {
            let report = find(&options)?;
            let save = pick(&report, need(0, "a save id")?)?;
            emit(&options, "inspect", true, json!({ "save": save }), || {
                println!("{}", save_line(save));
                println!("  file: {}", save.source_path.display());
                println!("  size: {} bytes", save.size_bytes);
                if let Some(sha) = &save.content_sha256 {
                    println!("  save SHA-256: {sha}");
                }
                if let Some(sha) = &save.container_sha256 {
                    println!("  card SHA-256: {sha}");
                }
                if let Some(code) = &save.identity.product_code {
                    println!(
                        "  game: {code}{}",
                        save.identity
                            .region
                            .as_deref()
                            .map(|r| format!(" ({r})"))
                            .unwrap_or_default()
                    );
                }
                for evidence in &save.identity.evidence {
                    println!(
                        "  evidence: {} ({})",
                        evidence.diagnostic, evidence.provenance.method
                    );
                }
                for line in &save.identity.ambiguity {
                    println!("  not certain: {line}");
                }
                println!(
                    "  emulator version: {}",
                    save.emulator_version
                        .as_deref()
                        .unwrap_or("not established")
                );
                if let Some(time) = save.modified_unix {
                    println!(
                        "  file last changed: {} (says nothing about progress)",
                        when(time)
                    );
                }
                if let Some(time) = &save.internal_modified {
                    println!("  time written inside the save: {time}");
                }
                for line in &save.integrity {
                    println!("  checked: {line}");
                }
                for line in save.notes.iter().chain(&save.unsupported) {
                    println!("  note: {line}");
                }
            })
        }
        "assess" => {
            let report = find(&options)?;
            let save = pick(&report, need(0, "a save id")?)?;
            let target = ContinuationTarget {
                emulator: options.to.clone().ok_or("assess needs --to <emulator>")?,
                profile: options.profile.clone().unwrap_or_default(),
                game_product_code: options.game_code.clone(),
                ..Default::default()
            };
            let result = assess(save, &target);
            emit(
                &options,
                "assess",
                true,
                json!({ "save_id": save.id, "target_emulator": target.emulator, "assessment": result }),
                || {
                    println!("{}: {}", result.verdict.label(), result.summary);
                    for line in &result.reasons {
                        println!("  because: {line}");
                    }
                    for line in &result.unchecked_conditions {
                        println!("  you must confirm: {line}");
                    }
                    if let Some(plan) = &result.conversion {
                        println!(
                            "  from: {}\n  to:   {}",
                            plan.source_format, plan.destination_format
                        );
                        for line in &plan.steps {
                            println!("  step: {line}");
                        }
                        for line in &plan.risks {
                            println!("  risk: {line}");
                        }
                        for line in &plan.validation {
                            println!("  checked by: {line}");
                        }
                    }
                },
            )
        }
        "match" => {
            let report = find(&options)?;
            let save = pick(&report, need(0, "a save id")?)?;
            let game = options
                .game
                .clone()
                .ok_or("match needs --game <game-file>")?;
            let result = match_game_file(save, &game);
            emit(
                &options,
                "match",
                true,
                json!({ "save_id": save.id, "game_file": game, "match": result }),
                || {
                    println!("{result:?}");
                    println!(
                        "The game file and the save were only read. File names were not used."
                    );
                },
            )
        }
        "safe-point" => {
            use archivefs_core::save_continuity::store::InUseObserver;
            let emulator = options
                .emulator
                .clone()
                .ok_or("safe-point needs --emulator <name>")?;
            let file = absolute(Path::new(need(0, "a save file")?));
            let observer = ProcessObserver::new();
            let guard = observer.guard(&emulator, &file);
            let (state, kernel) = (guard.state(), guard.kernel_watched());
            let verdict = match state {
                InUse::NotDetected => "verified_safe",
                InUse::EmulatorRunning | InUse::OpenByAProgram => "active",
                InUse::Unknown => "unknown",
            };
            emit(
                &options,
                "safe-point",
                true,
                json!({ "emulator": emulator, "file": file, "verdict": verdict, "state": state, "kernel_watched": kernel }),
                || {
                    println!(
                        "{verdict}: {}{}",
                        state.label(),
                        if kernel {
                            " (kernel lease held on the file)"
                        } else {
                            " (no kernel lease)"
                        }
                    );
                },
            )
        }
        "safe-points" => {
            let report = find(&options)?;
            let rows: Vec<Value> = SAFE_POINT_EMULATORS
                .iter()
                .map(|emulator| {
                    let saves: Vec<&DiscoveredSave> = report
                        .saves
                        .iter()
                        .filter(|save| save.emulator.eq_ignore_ascii_case(emulator))
                        .collect();
                    let count =
                        |state: InUse| saves.iter().filter(|save| save.in_use == state).count();
                    json!({
                        "emulator": emulator,
                        "saves_found": saves.len(),
                        "verified_safe": count(InUse::NotDetected),
                        "active": count(InUse::EmulatorRunning) + count(InUse::OpenByAProgram),
                        "unknown": count(InUse::Unknown),
                    })
                })
                .collect();
            emit(
                &options,
                "safe-points",
                true,
                json!({ "emulators": rows }),
                || {
                    println!("emulator      saves  verified-safe  active  unknown   (right now)");
                    for row in &rows {
                        let n = |key: &str| row[key].as_u64().unwrap_or(0);
                        println!(
                            "{:<12} {:>6} {:>14} {:>7} {:>8}",
                            row["emulator"].as_str().unwrap_or(""),
                            n("saves_found"),
                            n("verified_safe"),
                            n("active"),
                            n("unknown")
                        );
                    }
                    println!(
                        "Any other emulator is always \"unknown\": EmuWiz has no reviewed check for it."
                    );
                },
            )
        }
        "snapshot" => {
            let report = find(&options)?;
            let save = pick(&report, need(0, "a save id")?)?;
            let store = store(&options)?;
            let device = device(&options, &store)?;
            let request = capture_request(
                save,
                &report.saves,
                &device.identity.device_id,
                CaptureReason::Manual,
                None,
                options.best_effort,
            )?;
            let outcome = store.capture(&request, &ProcessObserver::new())?;
            let manifest = outcome.manifest().clone();
            let created = matches!(outcome, CaptureOutcome::Created { .. });
            emit(
                &options,
                "snapshot",
                false,
                json!({ "created": created, "store": store.root(), "result": outcome }),
                || {
                    if created {
                        println!("Snapshot {} taken.", &manifest.snapshot_id[..12]);
                    } else {
                        println!(
                            "Nothing has changed since snapshot {}; no new snapshot was needed.",
                            &manifest.snapshot_id[..12]
                        );
                    }
                    println!("  {}", manifest.body.boundary.label());
                    for line in &manifest.body.checks {
                        println!("  checked: {line}");
                    }
                    println!(
                        "  kept in {}. The save itself was only read.",
                        store.root().display()
                    );
                },
            )
        }
        "timeline" => {
            let store = store(&options)?;
            let lines = timelines(&store);
            emit(
                &options,
                "timeline",
                true,
                json!({ "store": store.root(), "saves": lines, "usage": store.usage() }),
                || {
                    for timeline in &lines {
                        println!("{} · {}", timeline.emulator, timeline.source.display());
                        for entry in &timeline.entries {
                            println!(
                                "  {}  {}  {}{}",
                                &entry.snapshot_id[..12],
                                when(entry.captured_unix),
                                entry.boundary.label(),
                                entry
                                    .session_id
                                    .as_deref()
                                    .map(|id| format!("  (session {id})"))
                                    .unwrap_or_default()
                            );
                            for change in &entry.changes {
                                println!("      {change}");
                            }
                        }
                    }
                    let usage = store.usage();
                    println!(
                        "{} snapshot(s), {} bytes on disk ({} before sharing identical content). Times are when a snapshot was taken, not a measure of progress.",
                        usage.snapshots, usage.stored_bytes, usage.logical_bytes
                    );
                },
            )
        }
        "verify" => {
            let store = store(&options)?;
            let report = store.verify(need(0, "a snapshot id")?)?;
            let intact = report.intact();
            emit(
                &options,
                "verify",
                true,
                json!({ "intact": intact, "report": report }),
                || {
                    for (name, check) in &report.files {
                        println!("  {name}: {check:?}");
                    }
                    println!(
                        "{}",
                        if intact {
                            format!(
                                "Verified: {} bytes re-read and matched.",
                                report.bytes_checked
                            )
                        } else {
                            "NOT intact: this snapshot must not be recovered from.".to_string()
                        }
                    );
                },
            )?;
            if intact {
                Ok(())
            } else {
                Err("the snapshot does not verify".into())
            }
        }
        "recover" => {
            let store = store(&options)?;
            let id = need(0, "a snapshot id")?;
            let destination = absolute(Path::new(
                options
                    .to
                    .as_deref()
                    .ok_or("recover needs --to <new-folder>")?,
            ));
            let plan = store.plan_recovery(id, &destination)?;
            if !options.confirm || !plan.blockers.is_empty() {
                let blocked = !plan.blockers.is_empty();
                emit(
                    &options,
                    "recover",
                    true,
                    json!({ "dry_run": true, "plan": plan }),
                    || {
                        println!(
                            "Recovery plan for snapshot {} -> {}",
                            &plan.snapshot_id[..12],
                            plan.destination.display()
                        );
                        for file in &plan.files {
                            println!("  would write {} ({} bytes)", file.name, file.size_bytes);
                        }
                        for line in &plan.blockers {
                            println!("  cannot go ahead: {line}");
                        }
                        for line in &plan.notes {
                            println!("  note: {line}");
                        }
                        println!(
                            "Nothing was changed.{}",
                            if blocked {
                                ""
                            } else {
                                " Add --confirm to write the new folder."
                            }
                        );
                    },
                )?;
                return if blocked && options.confirm {
                    Err("the recovery was not carried out".into())
                } else {
                    Ok(())
                };
            }
            let receipt = store.recover_to_new(id, &destination)?;
            emit(
                &options,
                "recover",
                false,
                json!({ "dry_run": false, "receipt": receipt }),
                || {
                    println!("Recovered into {}", receipt.destination.display());
                    for line in &receipt.checks {
                        println!("  checked: {line}");
                    }
                    println!("The original save was not touched.");
                },
            )
        }
        "continue" => {
            let report = find(&options)?;
            let save = pick(&report, need(0, "a save id")?)?;
            let onto = options.onto.clone().ok_or("continue needs --onto <card>")?;
            let work = options
                .work
                .clone()
                .ok_or("continue needs --work <new-folder>")?;
            let name = save
                .entry_name
                .clone()
                .ok_or("only a save on a PlayStation memory card can be continued this way")?;
            if !options.confirm {
                return emit(
                    &options,
                    "continue",
                    true,
                    json!({ "dry_run": true, "save": name, "source_card": save.source_path, "destination_card": onto, "work_folder": work }),
                    || {
                        println!(
                            "Would write a new card in {}:\n  everything on {}\n  plus {} from {}",
                            work.display(),
                            onto.display(),
                            name,
                            save.source_path.display()
                        );
                        println!(
                            "Neither card is changed. Nothing was written. Add --confirm to build it."
                        );
                    },
                );
            }
            match prepare_card_continuation(&save.source_path, &name, &onto, &work) {
                Ok(copy) => emit(
                    &options,
                    "continue",
                    false,
                    json!({ "dry_run": false, "copy": copy }),
                    || {
                        println!("New card: {}", copy.new_card.display());
                        for line in &copy.checks {
                            println!("  checked: {line}");
                        }
                        for line in &copy.next {
                            println!("  {line}");
                        }
                    },
                ),
                Err(ContinuationError::Conflict(conflict)) => {
                    emit(
                        &options,
                        "continue",
                        true,
                        json!({ "dry_run": false, "conflict": conflict }),
                        || {
                            println!("Needs your decision: {}", conflict.explanation);
                        },
                    )?;
                    Err("nothing was written".into())
                }
                Err(ContinuationError::Stopped(error)) => Err(error.into()),
            }
        }
        "sessions" => {
            let store = store(&options)?;
            let device = device(&options, &store)?;
            let journal = SessionJournal::open(store.root(), &device.identity.device_id)?;
            let settled = journal.reconcile(&ProcFs, continuity_now());
            let sessions = journal.recent(200);
            emit(
                &options,
                "sessions",
                false,
                json!({ "sessions": sessions, "settled_now": settled.len() }),
                || {
                    for session in &sessions {
                        println!(
                            "{}  {}  {} ({})  {}",
                            when(session.requested_unix),
                            session.game.title,
                            session.launch.emulator,
                            session.launch.profile,
                            session.status_line()
                        );
                        for change in &session.save_changes {
                            println!("      {change:?}");
                        }
                    }
                    println!("{} session(s).", sessions.len());
                },
            )
        }
        "pruning-proposal" => {
            let store = store(&options)?;
            let proposal = store.pruning_proposal(options.keep.unwrap_or(5));
            emit(
                &options,
                "pruning-proposal",
                true,
                json!({ "proposal": proposal }),
                || {
                    for candidate in &proposal.candidates {
                        println!(
                            "  {}  {}  would free {} bytes  ({})",
                            &candidate.snapshot_id[..12],
                            when(candidate.captured_unix),
                            candidate.would_free_bytes,
                            candidate.why
                        );
                    }
                    println!(
                        "{} snapshot(s) could be removed, freeing {} bytes. Nothing was deleted; EmuWiz never removes snapshots by itself.",
                        proposal.candidates.len(),
                        proposal.would_free_bytes
                    );
                },
            )
        }
        "device" => run_device(&options, &rest),
        other => Err(format!("unknown continuity command {other:?}\n\n{USAGE}").into()),
    }
}

fn continuity_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn read_list(path: &str) -> Result<SignedManifestSet, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    Ok(serde_json::from_slice(&bytes)
        .map_err(|error| format!("{path} is not a device list: {error}"))?)
}

fn run_device(options: &Options, rest: &[String]) -> Outcome {
    let Some(action) = rest.first() else {
        return Err(format!("continuity device needs a command\n\n{USAGE}").into());
    };
    let need = |index: usize, what: &str| -> Result<&String, String> {
        rest.get(index)
            .ok_or_else(|| format!("continuity device {action} needs {what}"))
    };
    let store = store(options)?;
    let device = device(options, &store)?;
    match action.as_str() {
        "show" => emit(
            options,
            "device show",
            false,
            json!({ "device": device.identity, "pairing_code": device.identity.fingerprint() }),
            || {
                println!("{}  id {}", device.identity.name, device.identity.device_id);
                println!(
                    "Pairing code (compare on both devices): {}",
                    device.identity.fingerprint()
                );
            },
        ),
        "export" => {
            let out = options
                .out
                .clone()
                .ok_or("device export needs --out <file>")?;
            if std::fs::symlink_metadata(&out).is_ok() {
                return Err(format!("{} already exists", out.display()).into());
            }
            let list = device.export(&store)?;
            std::fs::write(&out, serde_json::to_vec_pretty(&list)?)?;
            emit(
                options,
                "device export",
                false,
                json!({ "written": out, "sequence": list.body.sequence, "snapshots": list.body.snapshots.len() }),
                || {
                    println!(
                        "Signed list number {} ({} snapshot(s)) written to {}",
                        list.body.sequence,
                        list.body.snapshots.len(),
                        out.display()
                    );
                },
            )
        }
        "pair" => {
            let path = need(1, "the other device's identity or list file")?;
            let bytes =
                std::fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
            let identity: DeviceIdentity = serde_json::from_slice::<SignedManifestSet>(&bytes)
                .map(|list| list.body.device)
                .or_else(|_| {
                    serde_json::from_slice::<Value>(&bytes).and_then(|value| {
                        serde_json::from_value(value.get("device").cloned().unwrap_or(value))
                    })
                })
                .map_err(|error| format!("{path} does not hold a device identity: {error}"))?;
            if !options.confirm {
                return emit(
                    options,
                    "device pair",
                    true,
                    json!({ "dry_run": true, "device": identity, "pairing_code": identity.fingerprint() }),
                    || {
                        println!(
                            "Would pair with \"{}\". Its pairing code is {}.",
                            identity.name,
                            identity.fingerprint()
                        );
                        println!(
                            "Check that the other device shows the same code, then add --confirm. Nothing was changed."
                        );
                    },
                );
            }
            PeerBook::open(store.root()).pair(&identity, &device.identity)?;
            emit(
                options,
                "device pair",
                false,
                json!({ "dry_run": false, "paired": identity }),
                || {
                    println!(
                        "Paired with \"{}\" ({}).",
                        identity.name,
                        identity.fingerprint()
                    );
                },
            )
        }
        "compare" => {
            let theirs = read_list(need(1, "the other device's list file")?)?;
            PeerBook::open(store.root()).accept(&theirs)?;
            let mine = device.export(&store)?;
            let result = compare(&mine.body, &theirs.body);
            emit(
                options,
                "device compare",
                false,
                json!({ "this_device": mine.body.device, "other_device": theirs.body.device, "saves": result }),
                || {
                    println!(
                        "This device: {}   Other device: {}",
                        mine.body.device.name, theirs.body.device.name
                    );
                    for item in &result {
                        println!(
                            "{}{}",
                            item.title
                                .as_deref()
                                .map(|title| format!("{title} "))
                                .unwrap_or_default(),
                            item.name
                        );
                        println!(
                            "  {}{}",
                            if item.needs_decision {
                                "NEEDS YOUR DECISION: "
                            } else {
                                ""
                            },
                            item.proposal
                        );
                    }
                    println!(
                        "Nothing was transferred or changed. Clocks on two devices differ, so times were not compared."
                    );
                },
            )
        }
        "receive" => {
            let theirs = read_list(need(1, "the other device's list file")?)?;
            let id = need(2, "a snapshot id")?;
            let from = SnapshotStore::open(
                options
                    .from_store
                    .as_deref()
                    .ok_or("device receive needs --from-store <folder>")?,
            )?;
            PeerBook::open(store.root()).accept(&theirs)?;
            let Some(manifest) = theirs
                .body
                .snapshots
                .iter()
                .find(|manifest| manifest.snapshot_id.starts_with(id.as_str()))
            else {
                return Err(format!("the other device's list has no snapshot {id}").into());
            };
            if !options.confirm {
                return emit(
                    options,
                    "device receive",
                    true,
                    json!({ "dry_run": true, "snapshot": manifest }),
                    || {
                        println!(
                            "Would receive snapshot {} ({} bytes) from \"{}\" into {}.",
                            &manifest.snapshot_id[..12],
                            manifest.total_bytes(),
                            theirs.body.device.name,
                            store.root().display()
                        );
                        println!(
                            "No save would be changed: a received snapshot is only kept. Nothing was done. Add --confirm to receive it."
                        );
                    },
                );
            }
            // The sender's current list number, read from its own folder: the offline stand-in for asking it.
            let sender_now = LocalDevice::recorded_sequence(from.root());
            let report = receive_snapshot(
                &theirs,
                sender_now,
                &manifest.snapshot_id,
                &from,
                &store,
                &mut DirectLink,
            )?;
            emit(
                options,
                "device receive",
                false,
                json!({ "dry_run": false, "report": report }),
                || {
                    println!(
                        "Received snapshot {} ({} bytes carried).",
                        &report.snapshot_id[..12],
                        report.bytes_carried
                    );
                    for line in &report.checks {
                        println!("  checked: {line}");
                    }
                },
            )
        }
        other => Err(format!("unknown continuity device command {other:?}\n\n{USAGE}").into()),
    }
}
