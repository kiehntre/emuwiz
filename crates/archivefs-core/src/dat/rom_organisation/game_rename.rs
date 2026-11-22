//! Rename ONE library game to a file name the person types (Blue "Rename…").
//!
//! This is not a new rename engine. It feeds a single candidate, carrying the
//! typed name, to the organiser in [`OrganisationMode::RenameInPlace`], so the
//! existing safeguards all apply unchanged: review-time source identity,
//! collision and case-only collision refusal, apply-time revalidation, the
//! journalled no-clobber executor, catalogue rebinding (the same game row
//! keeps its id, metadata, favourites and collections) and History Undo.
//!
//! What this module adds is only what a typed name needs:
//! - the typed name is normalised and kept to the file's own extension;
//! - files that reference the game by name (`.cue`/`.m3u`/`.m3u8`/`.gdi` in
//!   its folder or any folder above it inside the library folder) would
//!   break, so such a rename is refused rather than silently breaking launch;
//! - RetroArch finds saves and save states by the game's file name, so they
//!   are renamed with the game in the same journalled transaction (one Undo
//!   puts both back). Nothing is guessed: a save another library game could
//!   own, a save name already taken, an unreadable save folder or a running
//!   RetroArch refuses the whole rename;
//! - the preview lists the consequences it cannot carry out yet.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::model::{OrganisationMode, OrganisationPlan, OrganisationStatus};
use super::plan::{OrganisationCandidate, OrganisationPlanRequest, build_organisation_plan};
use super::transaction::{
    apply_organisation_transaction, build_organisation_transaction, live_resolution_for,
    prepare_organisation_catalogue, revalidate_organisation_plan,
};
use crate::dat::classification::ContentSelectionPolicy;
use crate::dat::rename_apply::{ApplyOutcome, RenameTransaction, TransactionState};
use crate::safe_read::TrustedRoots;

/// Longest file name accepted (bytes), well under common filesystem limits.
const MAX_NAME_BYTES: usize = 200;

/// Sidecar types whose text names other game files.
pub(crate) const REFERENCING_EXTENSIONS: [&str; 4] = ["cue", "m3u", "m3u8", "gdi"];

/// A reviewed one-game rename, ready to be confirmed.
#[derive(Debug, Clone)]
pub struct GameRenamePreview {
    pub plan: OrganisationPlan,
    pub approved: BTreeSet<String>,
    pub source: PathBuf,
    pub destination: PathBuf,
    /// The library source folder holding the game (the rename's trusted root).
    pub source_root: PathBuf,
    /// The full new file name (with extension) the apply must still produce.
    pub new_file_name: String,
    /// Plain-language consequences shown before confirming.
    pub notes: Vec<String>,
    /// Where RetroArch saves were looked for (re-checked at apply).
    pub save_locations: RetroArchSaveLocations,
    /// RetroArch saves and save states renamed with the game.
    pub saves: Vec<SaveRename>,
}

/// One RetroArch save or save state renamed with its game, in the same
/// folder, with the identity the preview saw (a save changed since refuses).
#[derive(Debug, Clone)]
pub struct SaveRename {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub identity: crate::dat::rename_apply::model::ObjectIdentity,
}

/// Folders where RetroArch may keep this game's saves and save states, from
/// RetroArch's own configuration (`savefile_directory`, `savestate_directory`
/// and per-core overrides). RetroArch also writes beside the game file when a
/// configured folder is unset or missing; that folder is always searched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetroArchSaveLocations {
    pub directories: Vec<PathBuf>,
    /// Configured folders that could not be examined reliably. Any entry
    /// refuses the rename: saves there could not be ruled out.
    pub uncheckable: Vec<String>,
    /// RetroArch was running when this was read: it may be playing the game
    /// and write its save under the old name, so saves are not renamed then.
    pub retroarch_running: bool,
}

impl RetroArchSaveLocations {
    /// Reads the installed RetroArch configuration (read-only). No RetroArch
    /// profile means no configured folders; the game's own folder is still
    /// searched.
    pub fn discover() -> Self {
        let mut locations = Self::discover_in(
            &crate::emulator_environment::retroarch::DiscoveryEnvironment::from_process_environment(
            ),
        );
        locations.retroarch_running = crate::retroarch_playlist_relink::retroarch_is_running();
        locations
    }

    /// [`Self::discover`] for a given environment (tests use a scratch home).
    pub fn discover_in(
        environment: &crate::emulator_environment::retroarch::DiscoveryEnvironment,
    ) -> Self {
        use crate::emulator_environment::HostReadOnlyFilesystem;
        use crate::emulator_environment::retroarch::{
            SaveDirectoryStatus, discover_retroarch_environment,
        };
        let report = match discover_retroarch_environment(&HostReadOnlyFilesystem, environment) {
            Ok(report) => report,
            Err(error) => {
                return Self {
                    directories: Vec::new(),
                    uncheckable: vec![format!("RetroArch settings could not be read ({error})")],
                    retroarch_running: false,
                };
            }
        };
        let mut locations = Self::default();
        for profile in &report.profiles {
            // A settings file that exists but was not read completely could
            // name a save folder this check never sees.
            use crate::emulator_environment::FsProbe;
            use crate::emulator_environment::retroarch::ConfigReadOutcome;
            let read_fully = matches!(
                profile.config_file.read,
                ConfigReadOutcome::Parsed { complete: true, .. }
            );
            if !read_fully && profile.config_file.probe != FsProbe::Missing {
                locations.uncheckable.push(format!(
                    "{} (RetroArch settings not fully readable)",
                    profile.config_file.path.display
                ));
            }
        }
        for finding in report
            .profiles
            .iter()
            .flat_map(|profile| &profile.save_directories)
        {
            match (&finding.directory, finding.status) {
                // Nothing there: RetroArch writes beside the game instead.
                (Some(_), SaveDirectoryStatus::Missing | SaveDirectoryStatus::NotADirectory) => {}
                (
                    Some(directory),
                    SaveDirectoryStatus::Usable | SaveDirectoryStatus::NotWritable,
                ) if !directory.lossy => {
                    let path = PathBuf::from(&directory.display);
                    if !locations.directories.contains(&path) {
                        locations.directories.push(path);
                    }
                }
                // Unresolvable values, unreadable per-core settings, links and
                // anything else: saves there cannot be ruled out.
                (directory, _) => locations.uncheckable.push(format!(
                    "{} ({} in {})",
                    directory
                        .as_ref()
                        .map_or("an unresolved folder", |directory| directory
                            .display
                            .as_str()),
                    finding.config_key,
                    finding.settings_file.display
                )),
            }
        }
        locations
    }
}

/// Turns what the person typed into the full new file name. The file keeps its
/// own extension: typing it is optional, typing a different one is refused,
/// because the extension decides which emulator can open the game.
pub fn normalise_new_file_name(source: &Path, typed: &str) -> Result<String, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("Type a new name first.".into());
    }
    if typed.contains('/') || typed.contains('\\') || typed.contains('\0') {
        return Err("A name cannot contain / or \\.".into());
    }
    if typed == "." || typed == ".." || typed.starts_with('.') {
        return Err("A name cannot start with a dot.".into());
    }
    if typed.chars().any(char::is_control) {
        return Err("A name cannot contain control characters.".into());
    }
    let extension = source
        .extension()
        .map(|ext| ext.to_string_lossy().into_owned());
    let name = match &extension {
        Some(ext) => {
            let suffix = format!(".{ext}");
            let tail = typed
                .len()
                .checked_sub(suffix.len())
                .filter(|&start| start > 0)
                .and_then(|start| typed.get(start..).map(|tail| (start, tail)));
            match tail {
                Some((start, tail)) if tail.eq_ignore_ascii_case(&suffix) => {
                    format!("{}{suffix}", &typed[..start])
                }
                _ => match typed.rsplit_once('.') {
                    Some((_, other)) if looks_like_extension(other) => {
                        return Err(format!(
                            "The file must keep its .{ext} extension (you typed .{other})."
                        ));
                    }
                    _ => format!("{typed}{suffix}"),
                },
            }
        }
        None => typed.to_string(),
    };
    if name.len() > MAX_NAME_BYTES {
        return Err(format!(
            "That name is too long (max {MAX_NAME_BYTES} bytes)."
        ));
    }
    Ok(name)
}

/// "zip", "iso", "nes": 2-4 ASCII letters/digits with at least one letter.
/// "Game v1.2" or "Vol.2" end in a number, not an extension.
fn looks_like_extension(text: &str) -> bool {
    (2..=4).contains(&text.len())
        && text.chars().all(|c| c.is_ascii_alphanumeric())
        && text.chars().any(|c| c.is_ascii_alphabetic())
}

/// Builds the read-only preview for renaming `source` to `typed`.
pub fn preview_game_rename(
    database: &crate::Database,
    source: &Path,
    typed: &str,
    generation: u64,
    save_locations: &RetroArchSaveLocations,
) -> Result<GameRenamePreview, String> {
    let new_file_name = normalise_new_file_name(source, typed)?;
    let current = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or("This game has no file name.")?;
    if current == new_file_name {
        return Err("That is already the game's file name.".into());
    }
    database
        .find_archive_id_by_absolute_path(source)
        .map_err(|error| format!("could not read the library: {error}"))?
        .ok_or("This file is not in the library any more. Rescan, then try again.")?;
    let source_root = owning_source_root(database, source)?;
    refuse_arcade_set(database, source, &source_root, generation)?;
    if let Some(referrer) = referencing_sidecar(source, &source_root)? {
        return Err(format!(
            "{} names this file, so renaming it alone would stop the game launching. Nothing was changed.",
            referrer.display()
        ));
    }
    let found = find_retroarch_saves(source, save_locations)?;
    let saves = plan_save_renames(
        database,
        source,
        &new_file_name,
        &found,
        save_locations.retroarch_running,
    )?;
    let candidate = OrganisationCandidate {
        source_path: source.to_path_buf(),
        resolution: live_resolution_for(database, source, generation),
        canonical_name: Some(new_file_name.clone()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let mut plan = build_organisation_plan(&OrganisationPlanRequest {
        master_root: &source_root,
        mode: OrganisationMode::RenameInPlace,
        content_policy: ContentSelectionPolicy::AllEntries,
        candidates: std::slice::from_ref(&candidate),
        generation,
        unknown_folder: None,
    });
    let entry = plan
        .entries
        .first()
        .ok_or("The rename could not be planned. Nothing was changed.")?;
    if entry.status != OrganisationStatus::Suggested {
        let reason = entry
            .reason
            .clone()
            .unwrap_or_else(|| format!("{:?}", entry.status));
        return Err(format!(
            "This rename cannot be done: {reason}. Nothing was changed."
        ));
    }
    let destination = entry.destination_path.clone();
    if destination.parent() != source.parent() {
        return Err("A rename must stay in the same folder. Nothing was changed.".into());
    }
    prepare_organisation_catalogue(&mut plan, database)?;
    let approved: BTreeSet<String> = [source.to_string_lossy().into_owned()].into();
    let mut notes = vec![
        format!("The file is renamed in place: {} to {}", current, new_file_name),
        "The game keeps its EmuWiz library entry: favourites, collections, play history and EmuWiz artwork stay with it.".into(),
        "RetroArch playlists and ES-DE gamelists that list the old file name are not updated yet.".into(),
        if saves.is_empty() {
            "Undo is available in History.".into()
        } else {
            "Undo (in History) puts the game and its saves back together.".into()
        },
        "A catalogue verification is tied to the file's name, so the game is re-checked against its catalogue straight after the rename (and after an Undo). It stays verified only if the file still matches.".into(),
        "RomM is not told about the rename. It catches up at its next scan of this system (its scheduled scan, or Scan in RomM), where RomM 5.3 matches the renamed file to its existing entry by file hash if hashing is on. For games RomM never identified, its displayed title stays the old one.".into(),
    ];
    if saves.is_empty() {
        notes.push("No RetroArch saves or save states were found for this game.".into());
    }
    for save in &saves {
        notes.push(format!(
            "Its RetroArch save {} is renamed with it to {}",
            save.source.display(),
            file_name_of(&save.destination)
        ));
    }
    notes.push("Saves kept by standalone emulators are not checked or renamed.".into());
    Ok(GameRenamePreview {
        plan,
        approved,
        source: source.to_path_buf(),
        destination,
        source_root,
        new_file_name,
        notes,
        save_locations: save_locations.clone(),
        saves,
    })
}

/// Applies a confirmed preview with the journalled executor. Everything is
/// re-checked against the live library and disk first; any change since the
/// preview refuses with nothing renamed.
/// `save_locations` is read again just before confirming: RetroArch may
/// have run, or its settings changed, since the preview.
pub fn apply_game_rename(
    preview: &GameRenamePreview,
    database: &crate::Database,
    journal_dir: &Path,
    cancel: &AtomicBool,
    save_locations: &RetroArchSaveLocations,
) -> Result<ApplyOutcome, String> {
    refuse_if_unfinished_rename(preview, journal_dir)?;
    if let Some(referrer) = referencing_sidecar(&preview.source, &preview.source_root)? {
        return Err(format!(
            "{} now names this file, so the rename was refused. Nothing was changed.",
            referrer.display()
        ));
    }
    // The library folder must still be the same one the preview trusted.
    if owning_source_root(database, &preview.source)? != preview.source_root {
        return Err(
            "This game's library folder changed since the preview. Nothing was changed.".into(),
        );
    }
    refuse_arcade_set(
        database,
        &preview.source,
        &preview.source_root,
        preview.plan.generation,
    )?;
    // The saves must be exactly the ones the preview showed: one made since
    // (the game was played) or a folder that changed refuses.
    let mut now = find_retroarch_saves(&preview.source, &preview.save_locations)?;
    now.extend(find_retroarch_saves(&preview.source, save_locations)?);
    now.sort();
    now.dedup();
    let previewed: Vec<&Path> = preview
        .saves
        .iter()
        .map(|save| save.source.as_path())
        .collect();
    if now.iter().map(PathBuf::as_path).collect::<Vec<_>>() != previewed {
        return Err(
            "This game's RetroArch saves changed since the preview. Preview again. Nothing was changed."
                .into(),
        );
    }
    if !preview.saves.is_empty() && save_locations.retroarch_running {
        return Err(RETROARCH_RUNNING.into());
    }
    for save in &preview.saves {
        if std::fs::symlink_metadata(&save.destination).is_ok() {
            return Err(format!(
                "{} appeared after the preview, so this rename was refused: no save is overwritten. Nothing was changed.",
                save.destination.display()
            ));
        }
    }
    // A file that took the new name since the preview is named plainly here;
    // the plan check below would refuse too, with a less direct reason.
    if preview.destination != preview.source
        && std::fs::symlink_metadata(&preview.destination).is_ok()
    {
        return Err(format!(
            "{} appeared after the preview, so this rename was refused: nothing is overwritten. Nothing was changed.",
            preview.destination.display()
        ));
    }
    let wanted = preview.new_file_name.clone();
    let source = preview.source.clone();
    let name_for = move |path: &Path| (path == source).then(|| wanted.clone());
    revalidate_organisation_plan(&preview.plan, database.path(), &name_for)?;
    let mut transaction: RenameTransaction =
        build_organisation_transaction(&preview.plan, &preview.approved, preview.plan.generation)?;
    if transaction.entries.len() != 1 {
        return Err("The game changed since the preview. Nothing was changed.".into());
    }
    let mut approved = preview.approved.clone();
    let mut roots = vec![preview.source_root.clone()];
    for save in &preview.saves {
        transaction.entries.push(save_entry(&preview.source, save));
        approved.insert(save.source.to_string_lossy().into_owned());
        if let Some(folder) = save.source.parent()
            && !roots.iter().any(|root| folder.starts_with(root))
        {
            roots.push(folder.to_path_buf());
        }
    }
    std::fs::create_dir_all(journal_dir)
        .map_err(|error| format!("could not create the History folder: {error}"))?;
    let result = apply_organisation_transaction(
        &mut transaction,
        &approved,
        preview.plan.generation,
        TrustedRoots::from_paths(roots),
        journal_dir,
        cancel,
        OrganisationMode::RenameInPlace,
        &preview.source_root,
        database,
    );
    // The game and its saves are renamed together or not at all: anything
    // this run already renamed is put back.
    let complete =
        matches!(&result, Ok(outcome) if outcome.transaction.state == TransactionState::Applied);
    if !complete && transaction.applied_count() > 0 {
        let _ = super::rollback_organisation_transaction(
            &mut transaction,
            journal_dir,
            &AtomicBool::new(false),
            &preview.source_root,
        );
    }
    let outcome = result.map_err(|error| error.to_string())?;
    // The executor records a failed rename in the journal and still returns
    // Ok: success is only the Applied state.
    if outcome.transaction.state != TransactionState::Applied {
        let reason = outcome
            .transaction
            .entries
            .iter()
            .find_map(|entry| {
                entry
                    .failure_reason
                    .clone()
                    .or_else(|| entry.preflight_failures.first().cloned())
            })
            .unwrap_or_else(|| format!("{:?}", outcome.transaction.state));
        return Err(format!(
            "The rename did not complete ({reason}). Check History before trying again."
        ));
    }
    Ok(outcome)
}

/// RetroArch save and save-state names beside the game: `<file stem>.<one
/// of these>`, with numbered and auto states (`.state1`, `.state.auto`),
/// their thumbnails, and numbered memory cards (`.1.mcr`).
const RETROARCH_SAVE_SUFFIXES: [&str; 15] = [
    "srm", "sav", "rtc", "ram", "eep", "fla", "mpk", "ldci", "state", "bsv", "dsv", "bkr", "bcr",
    "smpc", "mcr",
];
/// Bound on folders examined in one save folder; more refuses.
const MAX_SAVE_SUBFOLDERS: usize = 4096;

/// The earlier paths Rename History proves the game now at `path` had,
/// newest first: each step is one applied, not undone, rename or move of a
/// game (never a save) whose destination was the previous path. `Err` (with
/// why) when History cannot settle it: an unreadable journal, or two entries
/// that both produced the same path.
pub fn earlier_paths_from_history(journal_dir: &Path, path: &Path) -> Result<Vec<PathBuf>, String> {
    use crate::dat::rename_apply::model::{EntryState, TransactionOperation};
    let (transactions, problems) = crate::dat::rename_apply::journal::list_journals(journal_dir);
    if !problems.is_empty() {
        return Err("part of Rename History could not be read".into());
    }
    let mut earlier: Vec<PathBuf> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        let mut sources: Vec<&Path> = transactions
            .iter()
            .filter(|transaction| {
                transaction.state == TransactionState::Applied
                    && super::transaction::is_organisation_transaction(transaction)
            })
            .flat_map(|transaction| &transaction.entries)
            .filter(|entry| {
                entry.state == EntryState::Applied
                    && entry.operation == TransactionOperation::RenameMove
                    && !entry
                        .unknown
                        .contains_key(crate::database::organisation_paths::SAVE_KEY)
                    && entry.destination_path == current
            })
            .map(|entry| entry.source_path.as_path())
            .collect();
        sources.sort();
        sources.dedup();
        let source = match sources.as_slice() {
            [] => break,
            [source] => source.to_path_buf(),
            _ => {
                return Err(format!(
                    "Rename History has more than one rename that produced {}",
                    current.display()
                ));
            }
        };
        // Renamed back and forth: every name is already known.
        if source == path || earlier.contains(&source) {
            break;
        }
        earlier.push(source.clone());
        current = source;
    }
    Ok(earlier)
}

/// Whether `transaction` is a rename that stopped part-way: interrupted
/// (a crash, a failure), or settled by recovery with the game renamed but
/// one of its saves not (recovery proved that save's rename never ran).
/// Undo of that History entry puts the game and its saves back.
pub fn is_unfinished_rename(transaction: &RenameTransaction) -> bool {
    use crate::dat::rename_apply::model::EntryState;
    if transaction.state.needs_recovery() {
        return true;
    }
    let has_saves = transaction.entries.iter().any(|entry| {
        entry
            .unknown
            .contains_key(crate::database::organisation_paths::SAVE_KEY)
    });
    let applied = transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .count();
    has_saves && applied > 0 && applied < transaction.entries.len()
}

/// Refuses while an earlier rename of this game (or of one of its saves)
/// is unfinished: renaming again on top of it could split the game from its
/// saves for good. The message names the History entry whose Undo fixes it.
/// Read-only. Blue also runs it for the preview, so this shows before
/// anything is confirmed.
pub fn refuse_if_unfinished_rename(
    preview: &GameRenamePreview,
    journal_dir: &Path,
) -> Result<(), String> {
    let mut paths = vec![preview.source.as_path(), preview.destination.as_path()];
    for save in &preview.saves {
        paths.push(&save.source);
        paths.push(&save.destination);
    }
    let (journals, problems) = crate::dat::rename_apply::journal::list_journals(journal_dir);
    if let Some(transaction) = journals.iter().find(|transaction| {
        is_unfinished_rename(transaction)
            && transaction.entries.iter().any(|entry| {
                paths.contains(&entry.source_path.as_path())
                    || paths.contains(&entry.destination_path.as_path())
            })
    }) {
        return Err(format!(
            "An earlier rename of this game did not finish (History: {}). Open History and Undo it first; it puts the game and its saves back. Nothing was changed.",
            transaction.transaction_id
        ));
    }
    if let Some(problem) = problems.first() {
        return Err(format!(
            "History could not be read completely ({problem}), so an unfinished rename of this game cannot be ruled out. Nothing was changed."
        ));
    }
    Ok(())
}

const RETROARCH_RUNNING: &str = "RetroArch is running, so this game's saves cannot be renamed safely (it could write a save under the old name). Close RetroArch, then preview again. Nothing was changed.";

/// Every RetroArch save or state for `source`: beside the game or one folder
/// below it (sorted by core), or anything named after the game in a
/// configured save folder and its by-core / by-folder subfolders. Sorted.
/// A folder that cannot be checked refuses rather than being skipped.
pub(crate) fn find_retroarch_saves(
    source: &Path,
    locations: &RetroArchSaveLocations,
) -> Result<Vec<PathBuf>, String> {
    find_retroarch_saves_with(source, locations, &mut SaveSearchIndex::default())
}

/// [`find_retroarch_saves`] reading folders through `index`, so a bulk check
/// lists each shared folder once instead of once per game. The index belongs
/// to one check: build a new one for every preview and every Apply.
pub(crate) fn find_retroarch_saves_with(
    source: &Path,
    locations: &RetroArchSaveLocations,
    index: &mut SaveSearchIndex,
) -> Result<Vec<PathBuf>, String> {
    if let Some(folder) = locations.uncheckable.first() {
        return Err(format!(
            "RetroArch's save setting {folder} could not be checked, so saves for this game cannot be ruled out. Nothing was changed."
        ));
    }
    let Some(stem) = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
    else {
        return Ok(Vec::new());
    };
    // A sub-folder that is a mount boundary (a mounted archive, for example)
    // is never searched below its top level: see `refuse_if_mount_is_relevant`.
    let mut saves = Vec::new();
    if let Some(game_folder) = source.parent() {
        saves.extend(index.saves_named_after(game_folder, &stem, source, false)?);
        let children = index.subdirectories(game_folder)?;
        for child in &children.plain {
            saves.extend(index.saves_named_after(child, &stem, source, false)?);
        }
        for mount in &children.mounts {
            index.refuse_if_mount_is_relevant(mount, &stem, source, false, false)?;
        }
    }
    for root in &locations.directories {
        saves.extend(index.saves_named_after(root, &stem, source, true)?);
        let children = index.subdirectories(root)?;
        for child in &children.plain {
            saves.extend(index.saves_named_after(child, &stem, source, true)?);
            let grandchildren = index.subdirectories(child)?;
            for grandchild in &grandchildren.plain {
                saves.extend(index.saves_named_after(grandchild, &stem, source, true)?);
            }
            for mount in &grandchildren.mounts {
                index.refuse_if_mount_is_relevant(mount, &stem, source, true, false)?;
            }
        }
        for mount in &children.mounts {
            index.refuse_if_mount_is_relevant(mount, &stem, source, true, true)?;
        }
    }
    saves.sort();
    saves.dedup();
    Ok(saves)
}

/// Where each found save goes: `<old stem>.<rest>` becomes `<new stem>.<rest>`
/// in the same folder. Refuses (nothing guessed) when RetroArch is running,
/// when another library game could own a save under the old or the new name
/// (the same name, or `Gradius Vol.2` beside `Gradius Vol`), when a save's
/// name could be another game's, when a save is not a plain file or is itself
/// a library file, or when a new save name is already taken. A longer name
/// that only starts the same way (`Super Mario Bros. 3` beside
/// `Super Mario Bros`) is a different game and does not refuse.
fn plan_save_renames(
    database: &crate::Database,
    source: &Path,
    new_file_name: &str,
    found: &[PathBuf],
    retroarch_running: bool,
) -> Result<Vec<SaveRename>, String> {
    if found.is_empty() {
        return Ok(Vec::new());
    }
    if retroarch_running {
        return Err(RETROARCH_RUNNING.into());
    }
    let old_stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new_stem = Path::new(new_file_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .ok_or("The new name has no file name.")?;
    let old_others = other_library_stems(database, source, &old_stem)?;
    // The new name must not be another game's either: its saves would then
    // share RetroArch's save names, and either game could load or overwrite
    // the other's progress.
    let new_others = other_library_stems(database, source, &new_stem)?;
    let mut renames = Vec::new();
    for save in found {
        let metadata = std::fs::symlink_metadata(save).map_err(|error| unreadable(save, error))?;
        if !metadata.is_file() {
            return Err(format!(
                "{} is not a plain file, so it is not renamed. Rename refused. Nothing was changed.",
                save.display()
            ));
        }
        if database
            .find_archive_id_by_absolute_path(save)
            .map_err(|error| format!("could not read the library: {error}"))?
            .is_some()
        {
            return Err(format!(
                "{} is itself a library file, so it is not treated as a save. Rename refused. Nothing was changed.",
                save.display()
            ));
        }
        let rest = file_name_of(save)
            .strip_prefix(&format!("{old_stem}."))
            .map(str::to_owned)
            .ok_or_else(|| format!("{} is not named after the game.", save.display()))?;
        // A save named for this game that another library game's saves
        // could also be called: whose it is cannot be told.
        if let Some(other) = claimed_by_another_game(&old_others, &old_stem, &file_name_of(save)) {
            return Err(format!(
                "{} could be the save of {} as well, so RetroArch's saves for the two games cannot be told apart. Rename refused. Nothing was changed.",
                save.display(),
                other.display()
            ));
        }
        // Only a recognised RetroArch save name is this game's. Anything else
        // named after it in a save folder (`Zelda.zip`, or `Zelda.Quest.srm`,
        // which may be a game called `Zelda.Quest`) is not proof of a save,
        // so nothing is guessed.
        if !is_save_suffix(&rest.to_ascii_lowercase()) {
            return Err(format!(
                "{} could belong to another game, so this game's saves cannot be told apart. Rename refused. Nothing was changed.",
                save.display()
            ));
        }
        let destination = save.with_file_name(format!("{new_stem}.{rest}"));
        if let Some(other) =
            claimed_by_another_game(&new_others, &new_stem, &file_name_of(&destination))
        {
            return Err(format!(
                "{} is a name {} would use for its saves too, so the renamed save would be shared with it. Rename refused. Nothing was changed.",
                destination.display(),
                other.display()
            ));
        }
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(format!(
                "{} already exists, so renaming the save {} would overwrite it. Rename refused. Nothing was changed.",
                destination.display(),
                save.display()
            ));
        }
        let identity = crate::dat::rename_apply::identity::capture_identity(save)
            .map_err(|error| unreadable(save, error))?;
        renames.push(SaveRename {
            source: save.clone(),
            destination,
            identity,
        });
    }
    Ok(renames)
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The journal entry renaming one save with its game. It has no library row
/// and no catalogue binding: the executor renames the file (no-clobber,
/// identity re-checked) and Undo renames it back.
fn save_entry(game: &Path, save: &SaveRename) -> crate::dat::rename_apply::model::TransactionEntry {
    use crate::dat::rename_apply::model::{EntryState, TransactionEntry, TransactionOperation};
    TransactionEntry {
        source_path: save.source.clone(),
        destination_path: save.destination.clone(),
        original_basename: file_name_of(&save.source),
        proposed_basename: file_name_of(&save.destination),
        identity: save.identity.clone(),
        operation: TransactionOperation::RenameMove,
        preflight_passed: false,
        preflight_failures: Vec::new(),
        state: EntryState::Planned,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: std::collections::BTreeMap::from([(
            crate::database::organisation_paths::SAVE_KEY.to_string(),
            serde_json::json!({"game": game.to_string_lossy()}),
        )]),
    }
}

fn unreadable(folder: &Path, error: impl std::fmt::Display) -> String {
    format!(
        "{} could not be read ({error}), so saves for this game cannot be ruled out. Nothing was changed.",
        folder.display()
    )
}

/// Counts of the real folder reads a [`SaveSearchIndex`] made.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SaveSearchStats {
    /// `read_dir` calls: one per distinct folder, however many games ask.
    pub folder_reads: u64,
    /// Directory entries read from disk across those folders.
    pub entries_read: u64,
    /// Entries compared while answering games (binary search keeps this
    /// close to the number of matches, not the folder size).
    pub entries_compared: u64,
    /// Distinct mount boundaries met (each listed at its top level only).
    pub mounts_seen: u64,
}

/// A folder's real sub-folders, split at mount boundaries.
#[derive(Debug, Clone, Default)]
pub(crate) struct Subfolders {
    pub(crate) plain: Vec<PathBuf>,
    /// On another filesystem than the folder, or a mount point: listed at
    /// their top level only, never searched below it.
    pub(crate) mounts: Vec<PathBuf>,
}

/// One folder as read once: its entries sorted by name, plus each entry's
/// kind as `read_dir` reported it (or the error reading that entry).
struct FolderListing {
    names: Vec<(String, PathBuf)>,
    kinds: Vec<(PathBuf, Result<std::fs::FileType, String>)>,
}

/// Read-only cache of the folders a save search lists. Every folder is read
/// at most once per index, and so is its sub-folder check; an error is kept
/// and returned to every game that asks, exactly as if each had read it. It
/// holds no state between checks: a preview and an Apply each build their own.
#[derive(Default)]
pub struct SaveSearchIndex {
    listings: std::collections::HashMap<PathBuf, Result<Option<FolderListing>, String>>,
    subfolders: std::collections::HashMap<PathBuf, Result<Subfolders, String>>,
    /// Mount points from `/proc/self/mountinfo`, read once when first needed
    /// (catches bind mounts that share the parent's device number).
    mount_points: Option<Result<std::collections::HashSet<PathBuf>, String>>,
    pub stats: SaveSearchStats,
}

impl SaveSearchIndex {
    /// An index that takes these as the system's mount points instead of
    /// reading `/proc/self/mountinfo` (tests cannot mount without root).
    #[cfg(test)]
    pub(crate) fn with_mount_points(points: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            mount_points: Some(Ok(points.into_iter().collect())),
            ..Self::default()
        }
    }

    fn mount_points(&mut self) -> Result<&std::collections::HashSet<PathBuf>, String> {
        let loaded = self.mount_points.get_or_insert_with(read_mount_points);
        loaded.as_ref().map_err(Clone::clone)
    }

    /// A mount sub-folder, met where saves may be: it is listed at its top
    /// level (one read per check, shared by every game), never below it.
    /// - nothing there named after this game: unrelated, skipped;
    /// - a save named after this game: refused (it cannot move with the game);
    /// - unreadable: refused (it cannot be ruled out);
    /// - `searched_deeper` (the search would also look one level inside it)
    ///   and it holds folders: refused, as relevance cannot be shown without
    ///   entering the mount.
    fn refuse_if_mount_is_relevant(
        &mut self,
        mount: &Path,
        stem: &str,
        source: &Path,
        any_suffix: bool,
        searched_deeper: bool,
    ) -> Result<(), String> {
        let found = self.saves_named_after(mount, stem, source, any_suffix)?;
        if !found.is_empty() {
            return Err(format!(
                "{} is a separate mounted filesystem (a mounted archive, for example) holding {} named after this game; it cannot be renamed with the game. Nothing was changed.",
                mount.display(),
                if found.len() == 1 {
                    "a save".to_string()
                } else {
                    format!("{} saves", found.len())
                }
            ));
        }
        if searched_deeper && let Some(listing) = self.listing(mount)? {
            for (path, kind) in &listing.kinds {
                if kind.clone()?.is_dir() {
                    return Err(format!(
                        "{} is a separate mounted filesystem inside RetroArch's save folder with folders of its own ({}); saves in them cannot be ruled out without searching the mount. Nothing was changed.",
                        mount.display(),
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }

    /// `Ok(None)`: the folder does not exist (no saves there).
    fn listing(&mut self, folder: &Path) -> Result<Option<&FolderListing>, String> {
        if !self.listings.contains_key(folder) {
            let read = Self::read(folder, &mut self.stats);
            self.listings.insert(folder.to_path_buf(), read);
        }
        match &self.listings[folder] {
            Ok(listing) => Ok(listing.as_ref()),
            Err(error) => Err(error.clone()),
        }
    }

    fn read(folder: &Path, stats: &mut SaveSearchStats) -> Result<Option<FolderListing>, String> {
        stats.folder_reads += 1;
        let entries = match std::fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(folder, error)),
        };
        let mut names = Vec::new();
        let mut kinds = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| unreadable(folder, error))?;
            stats.entries_read += 1;
            let path = entry.path();
            let kind = entry.file_type().map_err(|error| unreadable(&path, error));
            names.push((
                entry.file_name().to_string_lossy().into_owned(),
                path.clone(),
            ));
            kinds.push((path, kind));
        }
        names.sort();
        Ok(Some(FolderListing { names, kinds }))
    }

    /// Real sub-folders of `folder`. A linked sub-folder, a sub-folder on
    /// another filesystem (a mounted archive, for example), an unreadable
    /// entry or more than [`MAX_SAVE_SUBFOLDERS`] refuses rather than being
    /// skipped or searched.
    fn subdirectories(&mut self, folder: &Path) -> Result<Subfolders, String> {
        if let Some(known) = self.subfolders.get(folder) {
            return known.clone();
        }
        let found = self.find_subdirectories(folder);
        self.subfolders.insert(folder.to_path_buf(), found.clone());
        found
    }

    fn find_subdirectories(&mut self, folder: &Path) -> Result<Subfolders, String> {
        let Some(listing) = self.listing(folder)? else {
            return Ok(Subfolders::default());
        };
        let mut candidates = Vec::new();
        for (path, kind) in &listing.kinds {
            let kind = kind.clone()?;
            if kind.is_symlink() && path.is_dir() {
                return Err(format!(
                    "{} is a linked folder, so saves inside it cannot be ruled out. Nothing was changed.",
                    path.display()
                ));
            }
            if kind.is_dir() {
                candidates.push(path.clone());
                if candidates.len() > MAX_SAVE_SUBFOLDERS {
                    return Err(format!(
                        "{} holds too many folders to check for saves. Nothing was changed.",
                        folder.display()
                    ));
                }
            }
        }
        let mut found = Subfolders::default();
        if candidates.is_empty() {
            return Ok(found);
        }
        use std::os::unix::fs::MetadataExt;
        // The device of the folder as listed (followed: a configured save
        // folder may itself be a link to another disk).
        let device = std::fs::metadata(folder)
            .map_err(|error| unreadable(folder, error))?
            .dev();
        for path in candidates {
            let child =
                std::fs::symlink_metadata(&path).map_err(|error| unreadable(&path, error))?;
            let canonical =
                std::fs::canonicalize(&path).map_err(|error| unreadable(&path, error))?;
            let mount_point = self.mount_points()?.contains(&canonical);
            if child.dev() != device || mount_point {
                self.stats.mounts_seen += 1;
                found.mounts.push(path);
            } else {
                found.plain.push(path);
            }
        }
        Ok(found)
    }

    /// Entries in `folder` named `<stem>.` plus a save suffix (or, in a
    /// configured save folder, any suffix: nothing else lives there).
    fn saves_named_after(
        &mut self,
        folder: &Path,
        stem: &str,
        source: &Path,
        any_suffix: bool,
    ) -> Result<Vec<PathBuf>, String> {
        let prefix = format!("{stem}.");
        let mut compared = 0u64;
        let found = {
            let Some(listing) = self.listing(folder)? else {
                return Ok(Vec::new());
            };
            let start = listing
                .names
                .partition_point(|(name, _)| name.as_str() < prefix.as_str());
            let mut found = Vec::new();
            for (name, path) in &listing.names[start..] {
                compared += 1;
                let Some(rest) = name.strip_prefix(&prefix) else {
                    break; // sorted: no later name starts with the prefix
                };
                if path == source {
                    continue;
                }
                // "Super Mario Bros. 3.srm" starts with "Super Mario Bros." too, but
                // its remainder (" 3.srm") is not a plain extension: it is named
                // after another game and is left alone.
                if is_extension_chain(rest)
                    && (any_suffix || is_save_suffix(&rest.to_ascii_lowercase()))
                {
                    found.push(path.clone());
                }
            }
            found
        };
        self.stats.entries_compared += compared;
        Ok(found)
    }
}

/// Mount points of this process's mount namespace. Unreadable means mount
/// boundaries cannot be told apart, so every save check that needs it refuses.
fn read_mount_points() -> Result<std::collections::HashSet<PathBuf>, String> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").map_err(|error| {
        format!("the system's mount list could not be read ({error}), so mounted folders cannot be recognised and saves cannot be ruled out. Nothing was changed.")
    })?;
    Ok(text
        .lines()
        .filter_map(|line| line.split(' ').nth(4))
        .map(|field| PathBuf::from(unescape_mount_field(field)))
        .collect())
}

/// `/proc/self/mountinfo` writes space, tab, newline and backslash as
/// three-digit octal escapes (`\040`).
fn unescape_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && (b'0'..=b'3').contains(&bytes[index + 1])
            && bytes[index + 2..index + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            // At most \377: always fits a byte.
            let value = (bytes[index + 1] - b'0') * 64
                + (bytes[index + 2] - b'0') * 8
                + (bytes[index + 3] - b'0');
            out.push(value);
            index += 4;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `srm`, `state1.png`, `1.mcr`: dot-separated parts of letters, digits,
/// `_` or `-`. A space or bracket means the file is named after a longer name.
fn is_extension_chain(rest: &str) -> bool {
    rest.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// The stems of the other library files whose names start with `<stem>.`:
/// the only games whose RetroArch saves could share this game's names.
fn other_library_stems(
    database: &crate::Database,
    source: &Path,
    stem: &str,
) -> Result<Vec<(PathBuf, String)>, String> {
    Ok(database
        .present_archive_paths_named_after(stem)
        .map_err(|error| format!("could not read the library: {error}"))?
        .into_iter()
        .filter(|path| path != source)
        .filter_map(|path| {
            let other = path.file_stem()?.to_string_lossy().into_owned();
            Some((path, other))
        })
        .collect())
}

/// The other library game that could also own the save named `save_name`:
/// one with exactly this stem, or a longer one (`Gradius Vol.2` beside
/// `Gradius Vol`) whose own save names include it.
fn claimed_by_another_game<'a>(
    others: &'a [(PathBuf, String)],
    stem: &str,
    save_name: &str,
) -> Option<&'a Path> {
    others
        .iter()
        .find(|(_, other)| {
            other == stem
                || save_name
                    .strip_prefix(&format!("{other}."))
                    .is_some_and(is_extension_chain)
        })
        .map(|(path, _)| path.as_path())
}

fn is_save_suffix(rest: &str) -> bool {
    // `.1.mcr`: a numbered memory card.
    let rest = match rest.split_once('.') {
        Some((number, more))
            if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            more
        }
        _ => rest,
    };
    RETROARCH_SAVE_SUFFIXES
        .iter()
        .any(|suffix| match rest.strip_prefix(suffix) {
            Some("") => true,
            Some(more) => more.starts_with(|c: char| c.is_ascii_digit() || c == '.'),
            None => false,
        })
}

/// Arcade romsets must keep their set names: MAME and FBNeo find a game by
/// its zip name, so renaming one stops it launching.
pub(crate) fn refuse_arcade_set(
    database: &crate::Database,
    source: &Path,
    source_root: &Path,
    generation: u64,
) -> Result<(), String> {
    let folders = database
        .list_source_folders()
        .map_err(|error| format!("could not read the library folders: {error}"))?;
    refuse_arcade_set_in(&folders, database, source, source_root, generation)
}

/// [`refuse_arcade_set`] with the library folders already read (a bulk check
/// reads them once per run instead of once per game).
pub(crate) fn refuse_arcade_set_in(
    folders: &[crate::SourceFolderRecord],
    database: &crate::Database,
    source: &Path,
    source_root: &Path,
    generation: u64,
) -> Result<(), String> {
    let arcade_folder = folders
        .iter()
        .any(|folder| folder.path == source_root && folder.role == crate::SourceRole::ArcadeRomset);
    let arcade_platform = matches!(
        live_resolution_for(database, source, generation),
        crate::platform::identity::PlatformIdentityResolution::Resolved { ref platform, .. }
            if platform.eq_ignore_ascii_case("arcade")
    );
    if arcade_folder || arcade_platform {
        return Err("Arcade games must keep their set name (MAME and FBNeo find them by it), so this one cannot be renamed. Nothing was changed.".into());
    }
    Ok(())
}

/// The enabled library source folder that holds `source` (longest match).
pub(crate) fn owning_source_root(
    database: &crate::Database,
    source: &Path,
) -> Result<PathBuf, String> {
    let folders = database
        .list_source_folders()
        .map_err(|error| format!("could not read the library folders: {error}"))?;
    owning_source_root_in(&folders, source)
}

/// [`owning_source_root`] with the library folders already read.
pub(crate) fn owning_source_root_in(
    folders: &[crate::SourceFolderRecord],
    source: &Path,
) -> Result<PathBuf, String> {
    folders
        .iter()
        .map(|folder| &folder.path)
        .filter(|root| source.starts_with(root) && source != root.as_path())
        .max_by_key(|root| root.components().count())
        .cloned()
        .ok_or_else(|| "This game is not inside a library folder. Nothing was changed.".into())
}

/// A `.cue`/`.m3u`/`.m3u8`/`.gdi` in the game's folder, or in any folder
/// above it up to and including the library folder, whose text names it.
fn referencing_sidecar(source: &Path, source_root: &Path) -> Result<Option<PathBuf>, String> {
    let Some(name) = source.file_name() else {
        return Ok(None);
    };
    let name = name.to_string_lossy().to_lowercase();
    let mut folder = source.parent();
    while let Some(dir) = folder {
        if let Some(found) = sidecar_in(dir, source, &name)? {
            return Ok(Some(found));
        }
        if dir == source_root || !dir.starts_with(source_root) {
            break;
        }
        folder = dir.parent();
    }
    Ok(None)
}

fn sidecar_in(folder: &Path, source: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    Ok(folder_sidecars(folder)?
        .into_iter()
        .find(|(path, text)| path != source && text.contains(name))
        .map(|(path, _)| path))
}

/// Every `.cue`/`.m3u`/`.m3u8`/`.gdi` directly in `folder`, with its text in
/// lower case. Sidecars are small text files; one over 1 MiB is not a
/// sidecar and is left out. A sidecar that cannot be read refuses: it may
/// name the file. Shared by the one-game Rename and the bulk rename guard,
/// which reads each folder once.
pub(crate) fn folder_sidecars(folder: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let entries = std::fs::read_dir(folder)
        .map_err(|error| format!("could not read {}: {error}", folder.display()))?;
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let referencing = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .is_some_and(|ext| REFERENCING_EXTENSIONS.contains(&ext.as_str()));
        if !referencing || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if entry.metadata().is_ok_and(|meta| meta.len() > 1 << 20) {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "{} could not be read ({error}), so it may name this file. Nothing was changed.",
                path.display()
            )
        })?;
        found.push((path, String::from_utf8_lossy(&bytes).to_lowercase()));
    }
    Ok(found)
}

#[cfg(test)]
mod mount_list_tests {
    use super::{read_mount_points, unescape_mount_field};
    use std::path::Path;

    #[test]
    fn mountinfo_escapes_are_decoded() {
        assert_eq!(
            unescape_mount_field(r"/mnt/usb\040drive/a\134b"),
            r"/mnt/usb drive/a\b"
        );
        assert_eq!(
            unescape_mount_field(r"/x\0"),
            r"/x\0",
            "short escape left as is"
        );
        assert_eq!(
            unescape_mount_field(r"/x\777y"),
            r"/x\777y",
            "out of range left as is"
        );
    }

    #[test]
    fn the_real_mount_list_is_read_and_holds_the_root() {
        let points = read_mount_points().expect("/proc/self/mountinfo is readable");
        assert!(points.contains(Path::new("/")), "{points:?}");
    }
}
