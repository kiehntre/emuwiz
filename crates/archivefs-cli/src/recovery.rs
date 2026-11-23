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
  storage                                    where prepared copies are kept, and the connected ext4 disks
  storage --set <folder> --filesystem <UUID> keep them in this folder, on the disk with that filesystem UUID
  storage --reset                            keep them in EmuWiz's own data folder again

Options:
  --system <system>     the system the library records for the file, for example AmigaCD32
  --system-source <s>   how it was recorded (folder_alias, manual); a folder is never proof
  --store <folder> --store-filesystem <UUID>
                        use this existing prepared-media folder instead of the profile's (never created),
                        only while it is on the ext4 filesystem with that UUID
  --filesystem <UUID>   with storage --set: the disk the folder must be on (`lsblk -f` shows each disk's UUID)
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
    let mut set_filesystem = None;
    let mut store_filesystem = None;
    let mut positional = Vec::new();
    let mut args = args.into_iter();
    let mut only_files = false;
    while let Some(arg) = args.next() {
        // After `--` everything is a file name, whatever it looks like.
        if only_files {
            positional.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => only_files = true,
            // Asking for help must only ever print help.
            "--help" | "-h" | "help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--json" => json = true,
            "--yes" => yes = true,
            "--set" => set_folder = Some(args.next().ok_or("--set needs a folder")?),
            "--reset" => reset_folder = true,
            "--filesystem" => {
                set_filesystem = Some(args.next().ok_or("--filesystem needs a UUID")?);
            }
            "--store-filesystem" => {
                store_filesystem = Some(args.next().ok_or("--store-filesystem needs a UUID")?);
            }
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
    if set_folder.is_some() && reset_folder {
        return Err("give either --set <folder> or --reset, not both".into());
    }
    // Each UUID belongs to exactly one folder option; alone it means
    // nothing and is refused rather than ignored.
    if set_filesystem.is_some() && set_folder.is_none() {
        return Err("--filesystem is used with `storage --set <folder>`".into());
    }
    if store_filesystem.is_some() && store_override.is_none() {
        return Err("--store-filesystem is used with --store <folder>".into());
    }
    // Commands that do not use the store must work even when the saved
    // folder for prepared copies can no longer be used.
    if matches!(command.as_str(), "capabilities" | "storage") {
        return run_without_store(&command, set_folder, set_filesystem, reset_folder);
    }
    let store = match store_override {
        // A named folder must already exist on the disk named with it: it
        // is never created here, and never accepted for merely being there.
        Some(folder) => PreparedMediaStore::named(
            PathBuf::from(folder),
            store_filesystem.as_deref(),
            "--store",
        )?,
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
            recovery::moves::adopt_remembered_choice(
                archivefs_core::launch::remembered_launch_system::default_remembered_launch_systems_path()
                    .ok()
                    .as_deref(),
                &outcome,
                &path,
            );
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
                    recovery::BulkItemPlan::Reuse { .. } => {
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
        other => return Err(format!("unknown recovery command {other:?}\n\n{USAGE}").into()),
    }
    Ok(())
}

fn run_without_store(
    command: &str,
    set_folder: Option<String>,
    set_filesystem: Option<String>,
    reset_folder: bool,
) -> Outcome {
    if command == "capabilities" {
        print!("{}", recovery::capability_matrix_markdown());
        return Ok(());
    }
    if reset_folder {
        recovery::storage::set_folder(None, &[], None)?;
        println!(
            "New prepared copies will be kept in EmuWiz's own data folder again. Games already prepared keep working from the folder their copies are in."
        );
    } else if let Some(folder) = set_folder {
        // Without the list of game folders the check cannot be made.
        let library = recovery::storage::library_folders()
            .map_err(|detail| format!("The folder was not changed: {detail}"))?;
        let check = recovery::storage::set_folder(
            Some(Path::new(&folder)),
            &library,
            set_filesystem.as_deref(),
        )
        .map_err(|detail| format!("The folder was not changed: {detail}"))?;
        println!(
            "New prepared copies will be kept in {folder}.{} Games already prepared keep working from the folder their copies are in.",
            check
                .and_then(|check| check.available_bytes)
                .map(|bytes| format!(" {bytes} bytes free there."))
                .unwrap_or_default()
        );
    }
    match recovery::storage::describe_current() {
        Ok((current, chosen)) => println!(
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
        ),
        Err(detail) => println!(
            "The folder chosen for prepared copies cannot be used: {detail}. Choose another with `storage --set <folder> --filesystem <UUID>` or go back to the default with `storage --reset`."
        ),
    }
    // The disks a folder can be on, named by what they are (read-only).
    let disks = recovery::disk_identity::mounted_ext4_disks();
    if !disks.is_empty() {
        println!("Connected ext4 disks (for `storage --set <folder> --filesystem <UUID>`):");
    }
    for disk in disks {
        println!(
            "  {}  mounted at {}{}",
            disk.uuid,
            disk.mount_point.display(),
            disk.label
                .map(|label| format!("  (\"{label}\")"))
                .unwrap_or_default()
        );
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
    let queue = recovery::bulk::run_queue(
        store,
        archivefs_core::launch::remembered_launch_system::default_remembered_launch_systems_path()
            .ok()
            .as_deref(),
        trusted,
        &DiscSetLimits::default(),
        cancel,
        &|step| {
            if let recovery::bulk::BulkProgress::Starting {
                index,
                count,
                title,
            } = step
            {
                eprintln!("game {}/{count}: {title}", index + 1);
            }
        },
    )
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

    /// Whether the temporary folder is on ext4. EmuWiz accepts a store
    /// folder outside its own data folder only on ext4, so a test that needs
    /// an accepted folder cannot run on overlayfs, tmpfs and the like: it
    /// says so and is skipped. CI points TMPDIR at an ext4 image and sets
    /// EMUWIZ_R2_REQUIRE_QUALIFIED=1, with which a skip is a failure, so
    /// these tests cannot go green there without having run.
    fn temp_on_ext4_or_skip(test: &str) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let folder = std::env::temp_dir();
        let name = std::ffi::CString::new(folder.as_os_str().as_bytes()).unwrap();
        // SAFETY: zeroed is a valid initial value for this plain C struct,
        // `name` is NUL-terminated and `stats` is writable and sized.
        let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
        let on_ext4 = unsafe { libc::statfs(name.as_ptr(), &mut stats) } == 0
            && stats.f_type == libc::EXT4_SUPER_MAGIC;
        if !on_ext4 {
            assert!(
                std::env::var_os("EMUWIZ_R2_REQUIRE_QUALIFIED").is_none_or(|value| value != "1"),
                "{test}: the temporary folder {} is not on ext4, and this run requires it",
                folder.display()
            );
            eprintln!(
                "SKIPPED {test}: the temporary folder {} is not on ext4, so there is no folder EmuWiz would accept",
                folder.display()
            );
        }
        on_ext4
    }

    /// Exactly these arguments.
    fn exact(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    /// TEST SUPPORT THAT WORKS BACKWARDS. A test cannot format or mount a
    /// disk, so it has no real ext4 UUID to give. This reads the identity of
    /// the filesystem the folder (or the nearest existing folder above it)
    /// is on and builds a UUID that has that identity: the very thing the
    /// application must never do. It only gets the tests past the check.
    /// They show that refusals work and that nothing is written when one
    /// applies; they do not show that a real disk's real UUID is accepted
    /// (see the acceptance run in docs/GAME_RECOVERY_ENGINE.md). The temp
    /// folder must be on ext4 for the accepting tests to pass.
    fn disk_of(path: &Path) -> String {
        use std::os::unix::ffi::OsStrExt;
        let mut probe = path;
        while std::fs::symlink_metadata(probe).is_err() {
            match probe.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => probe = parent,
                // Not a path at all (a test of malformed arguments): some
                // UUID, of no disk in particular.
                _ => return "6f1d2c3b-4a59-4e68-9778-a6b5c4d3e2f1".to_string(),
            }
        }
        let name = std::ffi::CString::new(probe.as_os_str().as_bytes()).unwrap();
        // SAFETY: zeroed is a valid initial value for this plain C struct.
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `name` is NUL-terminated and `stats` is writable and sized.
        assert_eq!(unsafe { libc::statvfs(name.as_ptr(), &mut stats) }, 0);
        let hex: String = (stats.f_fsid as u64)
            .to_le_bytes()
            .iter()
            .chain([0_u8; 8].iter())
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The arguments, with `--store-filesystem <UUID>` added after every
    /// `--store <folder>` that has none: the UUID is the one [`disk_of`]
    /// builds backwards for that folder. Tests about the UUID itself use
    /// [`exact`].
    fn strings(args: &[&str]) -> Vec<String> {
        let mut out = exact(args);
        if !args.contains(&"--store-filesystem")
            && let Some(at) = args.iter().position(|arg| *arg == "--store")
            && let Some(folder) = args.get(at + 1)
        {
            out.push("--store-filesystem".to_string());
            out.push(disk_of(Path::new(folder)));
        }
        out
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
        (zip, existing_store(dir))
    }

    /// `--store` never creates its folder: the tests make it first.
    fn existing_store(dir: &Path) -> PathBuf {
        use std::os::unix::fs::DirBuilderExt;
        let store = dir.join("store");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&store)
            .unwrap();
        store
    }

    /// Nothing at all was created in the (existing) store folder.
    fn untouched(store: &Path) -> bool {
        std::fs::read_dir(store).unwrap().next().is_none()
    }

    /// Everything inside a folder, for "nothing was written" checks.
    fn contents(folder: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = Vec::new();
        let mut todo = vec![folder.to_path_buf()];
        while let Some(folder) = todo.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    todo.push(path.clone());
                }
                found.push(path);
            }
        }
        found.sort();
        found
    }

    /// A `--store` folder that is not there is refused by every command and
    /// never created (it may be inside a disk that is not connected).
    #[test]
    fn a_store_folder_that_is_missing_is_refused_and_never_created() {
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.path()).unwrap();
        let (zip, _existing) = lab(&dir);
        let zip = zip.to_str().unwrap();
        let mount_point = dir.join("mnt");
        std::fs::create_dir(&mount_point).unwrap();
        let missing = mount_point.join("prepared");
        let missing_arg = missing.to_str().unwrap();
        let error = |args: &[&str]| run(strings(args)).unwrap_err().to_string();
        for command in [
            vec![
                "prepare",
                zip,
                "--system",
                "AmigaCD32",
                "--store",
                missing_arg,
            ],
            vec![
                "prepare-all",
                zip,
                "--system",
                "AmigaCD32",
                "--yes",
                "--store",
                missing_arg,
            ],
            vec!["list", "--store", missing_arg],
            vec!["clean", "--store", missing_arg],
            vec!["queue", "--store", missing_arg],
            vec!["resume", "--store", missing_arg],
        ] {
            let detail = error(&command);
            assert!(
                detail.contains("--store") && detail.contains("is not there"),
                "{command:?}: {detail}"
            );
        }
        assert!(
            error(&[
                "list",
                "--store",
                "relative/folder",
                "--store-filesystem",
                "x"
            ])
            .contains("absolute path")
        );
        assert!(
            std::fs::read_dir(&mount_point).unwrap().next().is_none(),
            "the missing --store folder was created"
        );
    }

    /// A `--store` folder being there proves nothing about which disk is: a
    /// folder of the same name underneath an absent disk looks exactly like
    /// this one (plain, private 0700, holding a store's folders and a
    /// binding file left by an earlier build). Without the disk's UUID, or
    /// with the UUID of the disk that was meant, every command refuses it
    /// and nothing is written into it.
    #[test]
    fn a_store_folder_is_refused_without_its_disk_or_on_another_disk() {
        if !temp_on_ext4_or_skip("a_store_folder_is_refused_without_its_disk_or_on_another_disk") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.path()).unwrap();
        let (zip, _existing) = lab(&dir);
        let zip = zip.to_str().unwrap();
        let underneath = dir.join("mnt").join("prepared");
        for inner in ["v1/sets", "v1/staging", "v1/sources"] {
            std::fs::create_dir_all(underneath.join(inner)).unwrap();
        }
        std::fs::set_permissions(
            &underneath,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(underneath.join("emuwiz-storage-binding.json"), b"{}").unwrap();
        let store = underneath.to_str().unwrap();
        let before = contents(&underneath);
        assert_eq!(before.len(), 5);
        // The disk that was meant: any UUID but the one of the disk this
        // folder is really on.
        let intended = "6f1d2c3b-4a59-4e68-9778-a6b5c4d3e2f1";
        assert_ne!(disk_of(&underneath), intended);
        let commands = [
            vec!["prepare", zip, "--system", "AmigaCD32", "--store", store],
            vec![
                "prepare-all",
                zip,
                "--system",
                "AmigaCD32",
                "--yes",
                "--store",
                store,
            ],
            vec!["list", "--store", store],
            vec!["clean", "--store", store],
            vec!["queue", "--store", store],
            vec!["resume", "--store", store],
            vec!["forget-queue", "--store", store],
        ];
        for command in &commands {
            let detail = run(exact(command)).unwrap_err().to_string();
            assert!(detail.contains("say which disk"), "{command:?}: {detail}");
            assert_eq!(contents(&underneath), before, "{command:?} wrote");

            let mut named = command.clone();
            named.extend(["--store-filesystem", intended]);
            let detail = run(exact(&named)).unwrap_err().to_string();
            assert!(
                detail.contains("is not on the disk with filesystem UUID"),
                "{named:?}: {detail}"
            );
            assert_eq!(contents(&underneath), before, "{named:?} wrote");

            let mut garbled = command.clone();
            garbled.extend(["--store-filesystem", "not-a-uuid"]);
            let detail = run(exact(&garbled)).unwrap_err().to_string();
            assert!(
                detail.contains("is not a filesystem UUID"),
                "{garbled:?}: {detail}"
            );
            assert_eq!(contents(&underneath), before, "{garbled:?} wrote");
        }
        // A UUID belongs to exactly one folder option; alone it is refused,
        // not ignored. The rejected binding command no longer exists.
        for odd in [
            vec!["list", "--store-filesystem", intended],
            vec!["list", "--filesystem", intended],
            vec!["storage", "--filesystem", intended],
            vec!["storage", "--bind", store],
        ] {
            assert!(run(exact(&odd)).is_err(), "{odd:?}");
        }
        assert_eq!(contents(&underneath), before);
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
            assert!(untouched(&store), "{help} prepared a copy");
        }
        run(Vec::new()).unwrap();
    }

    #[test]
    fn a_system_is_never_guessed_and_unknown_input_is_refused() {
        if !temp_on_ext4_or_skip("a_system_is_never_guessed_and_unknown_input_is_refused") {
            return;
        }
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
        assert!(untouched(&store));
    }

    #[test]
    fn diagnose_prepare_verify_and_remove_work_headlessly_and_leave_the_game_file_alone() {
        if !temp_on_ext4_or_skip(
            "diagnose_prepare_verify_and_remove_work_headlessly_and_leave_the_game_file_alone",
        ) {
            return;
        }
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
        assert!(untouched(&store));

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
        if !temp_on_ext4_or_skip(
            "a_batch_is_only_reviewed_without_yes_and_runs_one_game_at_a_time_with_it",
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.path()).unwrap();
        let store = existing_store(&dir);
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
        assert!(untouched(&store));
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
