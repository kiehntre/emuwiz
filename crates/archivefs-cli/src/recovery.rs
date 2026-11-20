//! Headless game recovery: why a game cannot start, and verified preparation
//! of a playable copy.
//!
//! `diagnose`, `list`, `verify` and `capabilities` only read. `prepare`
//! creates new files in EmuWiz's prepared-media folder and never changes the
//! game file. `remove` and `clean` delete only files this engine created.
//! None of them opens the library database.

use archivefs_core::recovery::{
    self, ActionEffect, ActionPerformer, DiscSetLimits, HostFacts, PrepareProgress,
    PreparedMediaStore, RecoveryContext, RecoveryReport, RecoverySubject, prepared_media::SetState,
};
use archivefs_core::safe_read::TrustedRoots;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const USAGE: &str = "\
emuwiz-cli recovery <command>

  capabilities                               what the engine can and cannot do, per system and format
  diagnose <game-file> --system <system>     why it cannot start and what would fix it (read-only)
  prepare  <game-file> --system <system>     extract a checked copy the emulator can open (new files only)
  list                                       prepared copies and unfinished preparations (read-only)
  verify <copy-id>                           re-read a prepared copy against its receipt (read-only)
  remove <copy-id>                           delete a prepared copy EmuWiz created
  clean                                      delete the leftovers of unfinished preparations
  prepare-all <game-file>... --system <s>    review several games; with --yes, prepare them one at a time
  queue                                      the saved batch and what is left of it (read-only)
  resume                                     continue an unfinished batch
  forget-queue                               forget the saved batch (prepared copies are kept)
  storage [--set <folder> | --reset]         where prepared copies are kept; choose another folder

Options:
  --system <system>     the system the library records for the file, for example AmigaCD32
  --system-source <s>   how it was recorded (folder_alias, manual); a folder is never proof
  --store <folder>      use this prepared-media folder (absolute path) instead of the profile's
  --json                machine-readable output for diagnose and list
  --yes                 with prepare-all: start the reviewed batch (without it, only the review is shown)

The game file is only ever read. `prepare` never overwrites: an existing verified
copy is reused, a damaged one is refused until you remove it. Nothing is downloaded.";

type Outcome = Result<(), Box<dyn std::error::Error>>;

pub fn run(args: Vec<String>) -> Outcome {
    let mut json = false;
    let mut system = None;
    let mut system_source = None;
    let mut store_override = None;
    let mut yes = false;
    let mut set_folder = None;
    let mut reset_folder = false;
    let mut positional = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // Asking for help must only ever print help.
            "--help" | "-h" | "help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--json" => json = true,
            "--yes" => yes = true,
            "--set" => set_folder = Some(args.next().ok_or("--set needs a folder")?),
            "--reset" => reset_folder = true,
            "--system" => system = Some(args.next().ok_or("--system needs a system name")?),
            "--system-source" => {
                system_source = Some(args.next().ok_or("--system-source needs a value")?);
            }
            "--store" => store_override = Some(args.next().ok_or("--store needs a folder")?),
            other if other.starts_with("--") => {
                return Err(format!("unknown recovery option {other:?}\n\n{USAGE}").into());
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
    let store = match store_override {
        Some(folder) => {
            let folder = PathBuf::from(folder);
            if !folder.is_absolute() {
                return Err("--store needs an absolute path".into());
            }
            PreparedMediaStore::at(folder)
        }
        None => PreparedMediaStore::for_profile()?,
    };
    let game = |what: &str| -> Result<(PathBuf, String), String> {
        let path = rest
            .first()
            .map(|value| absolute(Path::new(value)))
            .ok_or_else(|| format!("recovery {command} needs {what}\n\n{USAGE}"))?;
        let system = system
            .as_deref()
            .ok_or("recovery needs --system <system>: the engine never guesses a system")?;
        let canonical = archivefs_core::canonical_platform_for_alias(system)
            .ok_or_else(|| format!("{system:?} is not a system EmuWiz knows"))?;
        Ok((path, canonical.to_string()))
    };
    let cancel = AtomicBool::new(false);
    let trusted = TrustedRoots::none();

    match command.as_str() {
        "capabilities" => {
            print!("{}", recovery::capability_matrix_markdown());
        }
        "diagnose" => {
            let (path, system) = game("a game file")?;
            let host = HostFacts::probe();
            let report = recovery::diagnose(
                &RecoverySubject::new(&path, Some(&system))
                    .with_platform_source(system_source.as_deref()),
                &RecoveryContext {
                    store: &store,
                    trusted: &trusted,
                    limits: DiscSetLimits::default(),
                    host: &host,
                    cancel: &cancel,
                },
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
        }
        "prepare" => {
            let (path, system) = game("a game file")?;
            if !recovery::preparation_may_apply(&path, Some(&system)) {
                return Err(format!(
                    "No validated preparation exists for this kind of file under {system}. \
                     Run `emuwiz-cli recovery diagnose` for the reason. Nothing was changed."
                )
                .into());
            }
            let outcome = store
                .prepare(
                    &path,
                    &system,
                    &trusted,
                    &DiscSetLimits::default(),
                    &cancel,
                    &|progress| match progress {
                        PrepareProgress::Extracting { name, index, count } => {
                            eprintln!("extracting {}/{count}: {name}", index + 1);
                        }
                        PrepareProgress::Verifying { name, index, count } => {
                            eprintln!("verifying  {}/{count}: {name}", index + 1);
                        }
                        PrepareProgress::Publishing => eprintln!("publishing"),
                    },
                )
                .map_err(|error| format!("{error}. The game file was only read."))?;
            if let Some(warning) = &outcome.link_warning {
                eprintln!(
                    "warning: the copy is in place but could not be linked to this ZIP ({warning}); run prepare again"
                );
            }
            let summary = outcome.summary;
            println!(
                "{}\n  copy id:    {}\n  folder:     {}\n  start file: {}\n  files:      {} ({} bytes)\n\
                 The game file was only read. Undo: emuwiz-cli recovery remove {}",
                if outcome.already_prepared {
                    "This ZIP holds exactly the files of an existing prepared copy (compared by SHA-256); nothing was extracted."
                } else {
                    "Prepared, and checked against the ZIP's own checksums."
                },
                summary.set_id,
                summary.directory.display(),
                summary.launch_path.display(),
                summary.file_count,
                summary.total_bytes,
                summary.set_id,
            );
        }
        "list" => {
            let sets = store.list();
            let interrupted = store.interrupted();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "store": store.root(),
                        "prepared": sets,
                        "unfinished": interrupted,
                    }))?
                );
                return Ok(());
            }
            println!("Prepared-media folder: {}", store.root().display());
            for set in &sets {
                match (&set.state, &set.summary) {
                    (SetState::Ready, Some(summary)) => println!(
                        "{}  ready    {} file(s), {} bytes\n    from  {}\n    start {}",
                        set.set_id,
                        summary.file_count,
                        summary.total_bytes,
                        summary.source_path.display(),
                        summary.launch_path.display()
                    ),
                    (SetState::Damaged(detail), _) => {
                        println!("{}  DAMAGED  {detail}", set.set_id);
                    }
                    (SetState::Ready, None) => println!("{}  DAMAGED", set.set_id),
                }
            }
            for entry in &interrupted {
                println!("unfinished: {}", entry.directory.display());
            }
            println!(
                "{} prepared copy/copies, {} unfinished. Nothing was changed.",
                sets.len(),
                interrupted.len()
            );
        }
        "verify" => {
            let id = rest.first().ok_or("recovery verify needs a copy id")?;
            let summary = store
                .verify_full(id, &cancel)
                .map_err(|detail| format!("Not verified: {detail}. Nothing was changed."))?;
            println!(
                "Verified: all {} file(s) match their recorded SHA-256.\n  start file: {}",
                summary.file_count,
                summary.launch_path.display()
            );
        }
        "remove" => {
            let id = rest.first().ok_or("recovery remove needs a copy id")?;
            let report = store.remove(id).map_err(|error| error.to_string())?;
            print_removal(&report);
        }
        "clean" => {
            let reports = store
                .clean_interrupted()
                .map_err(|error| error.to_string())?;
            if reports.is_empty() {
                println!("No unfinished preparation to clean up. Nothing was changed.");
            }
            for report in &reports {
                print_removal(report);
            }
        }
        "prepare-all" => {
            let system = system
                .as_deref()
                .ok_or("recovery needs --system <system>: the engine never guesses a system")?;
            let canonical = archivefs_core::canonical_platform_for_alias(system)
                .ok_or_else(|| format!("{system:?} is not a system EmuWiz knows"))?;
            if rest.is_empty() {
                return Err(format!("recovery prepare-all needs game files\n\n{USAGE}").into());
            }
            let candidates: Vec<recovery::BulkCandidate> = rest
                .iter()
                .map(|value| {
                    let path = absolute(Path::new(value));
                    recovery::BulkCandidate {
                        title: path
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        path,
                        platform_id: canonical.to_string(),
                    }
                })
                .collect();
            let plan = recovery::bulk::plan_bulk(
                &store,
                &candidates,
                &trusted,
                &DiscSetLimits::default(),
                &cancel,
            )?;
            for item in &plan.items {
                let what = match &item.plan {
                    recovery::BulkItemPlan::Extract { files, bytes } => {
                        format!("prepare: {files} new files, {bytes} bytes")
                    }
                    recovery::BulkItemPlan::Reuse => {
                        "matches an existing copy: compare in full, extract nothing".to_string()
                    }
                    recovery::BulkItemPlan::AlreadyPrepared => "already prepared".to_string(),
                    recovery::BulkItemPlan::Refused(why) => format!("cannot be prepared: {why}"),
                };
                println!("{}\n    {what}", item.candidate.path.display());
            }
            println!(
                "\n{} to do, {} bytes of new files; needs {} bytes free in {}; {} free.",
                plan.actionable().count(),
                plan.extract_bytes,
                plan.required_free_bytes,
                plan.store.display(),
                plan.available_bytes
                    .map_or("unknown".to_string(), |bytes| bytes.to_string())
            );
            if !yes {
                println!("Review only: nothing was changed. Add --yes to prepare these games.");
                return Ok(());
            }
            recovery::bulk::start_queue(&store, &plan, &|_| true)
                .map_err(|error| format!("{error}. Nothing was changed."))?;
            run_batch(&store, &trusted, &cancel)?;
        }
        "queue" => match recovery::bulk::load_queue(&store) {
            Some(queue) => print_queue(&queue),
            None => println!("There is no saved batch. Nothing was changed."),
        },
        "resume" => run_batch(&store, &trusted, &cancel)?,
        "forget-queue" => {
            recovery::bulk::clear_queue(&store).map_err(|error| error.to_string())?;
            println!("The saved batch was forgotten. Prepared copies were not touched.");
        }
        "storage" => {
            let library: Vec<PathBuf> = archivefs_core::default_config_path()
                .ok()
                .and_then(|config| archivefs_core::load_source_folder_configs_from(config).ok())
                .map(|sources| sources.into_iter().map(|source| source.path).collect())
                .unwrap_or_default();
            if reset_folder {
                recovery::storage::set_folder(None, &library)?;
                println!("Prepared copies will be kept in EmuWiz's own data folder again.");
            } else if let Some(folder) = set_folder {
                let check = recovery::storage::set_folder(Some(Path::new(&folder)), &library)
                    .map_err(|detail| format!("The folder was not changed: {detail}"))?;
                println!(
                    "Prepared copies will now be kept in {folder}.{} Copies already prepared elsewhere are not moved.",
                    check
                        .and_then(|check| check.available_bytes)
                        .map(|bytes| format!(" {bytes} bytes free there."))
                        .unwrap_or_default()
                );
            }
            let (current, chosen) = recovery::storage::describe_current()?;
            println!(
                "Prepared copies are kept in {}{}; {} free.",
                current.root().display(),
                if chosen {
                    " (your choice)"
                } else {
                    " (default)"
                },
                current
                    .available_bytes()
                    .map_or("unknown space".to_string(), |bytes| format!(
                        "{bytes} bytes"
                    ))
            );
        }
        other => return Err(format!("unknown recovery command {other:?}\n\n{USAGE}").into()),
    }
    Ok(())
}

fn print_queue(queue: &recovery::Queue) {
    for item in &queue.items {
        let state = match &item.state {
            recovery::QueueState::Pending => "to do".to_string(),
            recovery::QueueState::Prepared => "prepared".to_string(),
            recovery::QueueState::Reused => "matched an existing copy".to_string(),
            recovery::QueueState::Failed(why) => format!("not prepared: {why}"),
        };
        println!("{}\n    {state}", item.candidate.path.display());
    }
    if let Some(why) = &queue.stopped {
        println!("Stopped early: {why}");
    }
    println!("{:?}", queue.counts());
}

fn run_batch(
    store: &PreparedMediaStore,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
) -> Result<(), Box<dyn std::error::Error>> {
    let queue =
        recovery::bulk::run_queue(store, trusted, &DiscSetLimits::default(), cancel, &|step| {
            if let recovery::bulk::BulkProgress::Starting {
                index,
                count,
                title,
            } = step
            {
                eprintln!("game {}/{count}: {title}", index + 1);
            }
        })
        .map_err(|error| error.to_string())?;
    print_queue(&queue);
    println!("The game files were only read.");
    Ok(())
}

fn print_removal(report: &recovery::RemovalReport) {
    println!(
        "Removed {} file(s), {} bytes freed.",
        report.removed_files.len(),
        report.freed_bytes
    );
    for kept in &report.kept {
        println!(
            "  left in place (not exactly what EmuWiz created): {}",
            kept.display()
        );
    }
}

fn effect_label(effect: ActionEffect) -> &'static str {
    match effect {
        ActionEffect::ReadOnly => "read-only",
        ActionEffect::CreatesNewFiles => "creates new files",
        ActionEffect::ChangesExistingState => "changes existing state",
    }
}

fn print_report(report: &RecoveryReport) {
    println!("Game:    {}", report.subject.path.display());
    println!(
        "System:  {} ({})",
        report
            .subject
            .platform_id
            .as_deref()
            .unwrap_or("not recorded"),
        report
            .subject
            .platform_source
            .as_deref()
            .unwrap_or("source not given")
    );
    println!("Verdict: {:?}\n", report.verdict);
    for finding in &report.findings {
        println!(
            "[{:?}] {}  (confidence: {:?}; blocks: {:?})",
            finding.code, finding.title, finding.confidence, finding.blocking
        );
        println!("    {}", finding.explanation);
        for evidence in &finding.evidence {
            println!(
                "    evidence ({:?}): {}",
                evidence.source, evidence.statement
            );
        }
        for action in &finding.actions {
            println!(
                "    action [{}; {}]: {}\n        {}",
                effect_label(action.effect),
                match action.performer {
                    ActionPerformer::Engine => "EmuWiz can do this",
                    ActionPerformer::Person => "you do this",
                },
                action.title,
                action.detail
            );
        }
        println!();
    }
    if let Some(plan) = &report.plan {
        println!(
            "Preparation plan: {} file(s), {} bytes, into {}\n  needs {} bytes free",
            plan.files.len(),
            plan.total_bytes,
            plan.destination.display(),
            plan.required_free_bytes
        );
        for step in &plan.steps {
            println!("  - {step}");
        }
    }
    if let Some(prepared) = &report.prepared {
        println!("Prepared copy: {}", prepared.launch_path.display());
    }
    println!("Nothing was changed.");
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::synthetic_media::{cd_disc_set, stored_zip};

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    /// A synthetic zipped disc and an empty store folder, both in `dir`.
    fn lab(dir: &Path) -> (PathBuf, PathBuf) {
        let members = cd_disc_set("Synthetic Disc (1994)(Test)[!]");
        let borrowed: Vec<(&str, &[u8])> = members
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        let zip = dir.join("Synthetic Disc (1994)(Test)[!].zip");
        std::fs::write(&zip, stored_zip(&borrowed)).unwrap();
        (zip, dir.join("store"))
    }

    #[test]
    fn asking_for_help_never_runs_a_command() {
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.path()).unwrap();
        let (zip, store) = lab(&dir);
        for help in ["--help", "-h", "help"] {
            run(strings(&[
                "prepare",
                zip.to_str().unwrap(),
                "--system",
                "AmigaCD32",
                "--store",
                store.to_str().unwrap(),
                help,
            ]))
            .unwrap();
            assert!(!store.exists(), "{help} prepared a copy");
        }
        run(Vec::new()).unwrap();
    }

    #[test]
    fn a_system_is_never_guessed_and_unknown_input_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.path()).unwrap();
        let (zip, store) = lab(&dir);
        let zip = zip.to_str().unwrap();
        let store_arg = store.to_str().unwrap();
        let error = |args: &[&str]| run(strings(args)).unwrap_err().to_string();
        assert!(error(&["prepare", zip, "--store", store_arg]).contains("never guesses"));
        assert!(
            error(&[
                "prepare",
                zip,
                "--system",
                "NotASystem",
                "--store",
                store_arg
            ])
            .contains("not a system")
        );
        assert!(
            error(&["prepare", zip, "--system", "Xbox", "--store", store_arg])
                .contains("No validated preparation")
        );
        assert!(
            error(&[
                "prepare",
                zip,
                "--system",
                "AmigaCD32",
                "--store",
                "relative"
            ])
            .contains("absolute")
        );
        assert!(error(&["frobnicate"]).contains("unknown recovery command"));
        assert!(error(&["list", "--frobnicate"]).contains("unknown recovery option"));
        assert!(!store.exists());
    }

    #[test]
    fn diagnose_prepare_verify_and_remove_work_headlessly_and_leave_the_game_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.path()).unwrap();
        let (zip, store) = lab(&dir);
        let before = std::fs::read(&zip).unwrap();
        let common = |command: &str| {
            strings(&[
                command,
                zip.to_str().unwrap(),
                "--system",
                "cd32",
                "--system-source",
                "folder_alias",
                "--store",
                store.to_str().unwrap(),
            ])
        };
        // Read-only commands create nothing, not even the store folder.
        run(common("diagnose")).unwrap();
        let mut json = common("diagnose");
        json.push("--json".into());
        run(json).unwrap();
        run(strings(&["list", "--store", store.to_str().unwrap()])).unwrap();
        run(strings(&["capabilities"])).unwrap();
        assert!(!store.exists());

        run(common("prepare")).unwrap();
        let prepared = PreparedMediaStore::at(&store)
            .prepared_for_source(&zip)
            .expect("prepared and linked");
        assert_eq!(prepared.platform_id, "AmigaCD32");
        // A second prepare reuses the copy.
        run(common("prepare")).unwrap();
        assert_eq!(PreparedMediaStore::at(&store).list().len(), 1);
        let store_arg = store.to_str().unwrap();
        run(strings(&["verify", &prepared.set_id, "--store", store_arg])).unwrap();
        run(strings(&["clean", "--store", store_arg])).unwrap();
        run(strings(&["remove", &prepared.set_id, "--store", store_arg])).unwrap();
        assert!(PreparedMediaStore::at(&store).list().is_empty());
        assert!(run(strings(&["remove", &prepared.set_id, "--store", store_arg])).is_err());
        assert!(run(strings(&["verify", &prepared.set_id, "--store", store_arg])).is_err());
        assert_eq!(std::fs::read(&zip).unwrap(), before);
    }

    #[test]
    fn a_batch_is_only_reviewed_without_yes_and_runs_one_game_at_a_time_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.path()).unwrap();
        let store = dir.join("store");
        let store_arg = store.to_str().unwrap();
        let mut zips = Vec::new();
        for title in ["First Disc", "Second Disc"] {
            let members = cd_disc_set(title);
            let borrowed: Vec<(&str, &[u8])> = members
                .iter()
                .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
                .collect();
            let zip = dir.join(format!("{title}.zip"));
            std::fs::write(&zip, stored_zip(&borrowed)).unwrap();
            zips.push(zip);
        }
        let broken = dir.join("Broken.zip");
        std::fs::write(&broken, b"not a zip").unwrap();
        let batch = |extra: &[&str]| {
            let mut args = strings(&["prepare-all", "--system", "AmigaCD32", "--store", store_arg]);
            args.extend(zips.iter().map(|zip| zip.to_str().unwrap().to_string()));
            args.push(broken.to_str().unwrap().to_string());
            args.extend(strings(extra));
            run(args)
        };
        // Review only.
        batch(&[]).unwrap();
        assert!(!store.exists());
        run(strings(&["queue", "--store", store_arg])).unwrap();
        assert!(run(strings(&["resume", "--store", store_arg])).is_err());

        batch(&["--yes"]).unwrap();
        let prepared = PreparedMediaStore::at(&store);
        assert_eq!(prepared.list().len(), 2);
        for zip in &zips {
            assert!(prepared.prepared_for_source(zip).is_some());
        }
        let queue = recovery::bulk::load_queue(&prepared).unwrap();
        assert_eq!(queue.items.len(), 2, "the unreadable file was never queued");
        assert_eq!(queue.pending(), 0);
        // Nothing left to do: a second batch is refused, nothing changes.
        assert!(batch(&["--yes"]).is_err());
        run(strings(&["forget-queue", "--store", store_arg])).unwrap();
        assert!(recovery::bulk::load_queue(&prepared).is_none());
        assert_eq!(prepared.list().len(), 2);
    }
}
