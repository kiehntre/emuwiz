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
//! - RetroArch finds saves and save states by the game's file name, so a game
//!   that has any is refused (they are never renamed, moved or guessed at);
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
const REFERENCING_EXTENSIONS: [&str; 4] = ["cue", "m3u", "m3u8", "gdi"];

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
}

impl RetroArchSaveLocations {
    /// Reads the installed RetroArch configuration (read-only). No RetroArch
    /// profile means no configured folders; the game's own folder is still
    /// searched.
    pub fn discover() -> Self {
        Self::discover_in(
            &crate::emulator_environment::retroarch::DiscoveryEnvironment::from_process_environment(
            ),
        )
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
    refuse_if_retroarch_saves(source, save_locations)?;
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
    let notes = vec![
        format!("The file is renamed in place: {} to {}", current, new_file_name),
        "The game keeps its EmuWiz library entry: favourites, collections, play history and EmuWiz artwork stay with it.".into(),
        "RetroArch playlists and ES-DE gamelists that list the old file name are not updated yet.".into(),
        "Undo is available in History.".into(),
        "A catalogue verification is tied to the file's name, so the game is re-checked against its catalogue straight after the rename (and after an Undo). It stays verified only if the file still matches.".into(),
        "No RetroArch saves or save states were found for this game. Saves kept by standalone emulators are not checked.".into(),
        "RomM is not told about the rename. It catches up at its next scan of this system (its scheduled scan, or Scan in RomM), where RomM 5.3 matches the renamed file to its existing entry by file hash if hashing is on. For games RomM never identified, its displayed title stays the old one.".into(),
    ];
    Ok(GameRenamePreview {
        plan,
        approved,
        source: source.to_path_buf(),
        destination,
        source_root,
        new_file_name,
        notes,
        save_locations: save_locations.clone(),
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
    // A save made since the preview (the game was played) also refuses.
    refuse_if_retroarch_saves(&preview.source, &preview.save_locations)?;
    refuse_if_retroarch_saves(&preview.source, save_locations)?;
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
    std::fs::create_dir_all(journal_dir)
        .map_err(|error| format!("could not create the History folder: {error}"))?;
    let outcome = apply_organisation_transaction(
        &mut transaction,
        &preview.approved,
        preview.plan.generation,
        TrustedRoots::from_paths([preview.source_root.clone()]),
        journal_dir,
        cancel,
        OrganisationMode::RenameInPlace,
        &preview.source_root,
        database,
    )
    .map_err(|error| error.to_string())?;
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

/// Refuses when any RetroArch save or state for `source` may exist: beside
/// the game or one folder below it (sorted by core), or anything named after
/// the game in a configured save folder and its by-core / by-folder
/// subfolders.
fn refuse_if_retroarch_saves(
    source: &Path,
    locations: &RetroArchSaveLocations,
) -> Result<(), String> {
    if let Some(folder) = locations.uncheckable.first() {
        return Err(format!(
            "RetroArch's save setting {folder} could not be checked, so saves for this game cannot be ruled out. Nothing was changed."
        ));
    }
    let Some(stem) = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
    else {
        return Ok(());
    };
    let found = |folder: &Path, any_suffix: bool| -> Result<(), String> {
        match save_named_after(folder, &stem, source, any_suffix)? {
            Some(found) => Err(format!(
                "RetroArch keeps a save for this game at {}. Renaming the game would disconnect it, so this rename is refused. Nothing was changed.",
                found.display()
            )),
            None => Ok(()),
        }
    };
    if let Some(game_folder) = source.parent() {
        found(game_folder, false)?;
        for child in subdirectories(game_folder)? {
            found(&child, false)?;
        }
    }
    for root in &locations.directories {
        found(root, true)?;
        for child in subdirectories(root)? {
            found(&child, true)?;
            for grandchild in subdirectories(&child)? {
                found(&grandchild, true)?;
            }
        }
    }
    Ok(())
}

fn unreadable(folder: &Path, error: impl std::fmt::Display) -> String {
    format!(
        "{} could not be read ({error}), so saves for this game cannot be ruled out. Nothing was changed.",
        folder.display()
    )
}

/// Real sub-folders of `folder`. A linked sub-folder, an unreadable entry or
/// more than [`MAX_SAVE_SUBFOLDERS`] refuses rather than being skipped.
fn subdirectories(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unreadable(folder, error)),
    };
    let mut folders = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| unreadable(folder, error))?;
        let kind = entry
            .file_type()
            .map_err(|error| unreadable(&entry.path(), error))?;
        if kind.is_symlink() && entry.path().is_dir() {
            return Err(format!(
                "{} is a linked folder, so saves inside it cannot be ruled out. Nothing was changed.",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            folders.push(entry.path());
            if folders.len() > MAX_SAVE_SUBFOLDERS {
                return Err(format!(
                    "{} holds too many folders to check for saves. Nothing was changed.",
                    folder.display()
                ));
            }
        }
    }
    Ok(folders)
}

/// A file in `folder` named `<stem>.` plus a save suffix (or, in a
/// configured save folder, any suffix: nothing else lives there).
fn save_named_after(
    folder: &Path,
    stem: &str,
    source: &Path,
    any_suffix: bool,
) -> Result<Option<PathBuf>, String> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(folder, error)),
    };
    let prefix = format!("{stem}.");
    for entry in entries {
        let entry = entry.map_err(|error| unreadable(folder, error))?;
        let path = entry.path();
        if path == source {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if any_suffix || is_save_suffix(&rest.to_ascii_lowercase()) {
            return Ok(Some(path));
        }
    }
    Ok(None)
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
fn refuse_arcade_set(
    database: &crate::Database,
    source: &Path,
    source_root: &Path,
    generation: u64,
) -> Result<(), String> {
    let arcade_folder = database
        .list_source_folders()
        .map_err(|error| format!("could not read the library folders: {error}"))?
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
fn owning_source_root(database: &crate::Database, source: &Path) -> Result<PathBuf, String> {
    let folders = database
        .list_source_folders()
        .map_err(|error| format!("could not read the library folders: {error}"))?;
    folders
        .into_iter()
        .map(|folder| folder.path)
        .filter(|root| source.starts_with(root) && source != root)
        .max_by_key(|root| root.components().count())
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
    let entries = std::fs::read_dir(folder)
        .map_err(|error| format!("could not read {}: {error}", folder.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path == source {
            continue;
        }
        let referencing = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .is_some_and(|ext| REFERENCING_EXTENSIONS.contains(&ext.as_str()));
        if !referencing || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        // Sidecars are small text files; a huge one is not a sidecar.
        if entry.metadata().is_ok_and(|meta| meta.len() > 1 << 20) {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "{} could not be read ({error}), so it may name this file. Nothing was changed.",
                path.display()
            )
        })?;
        if String::from_utf8_lossy(&bytes)
            .to_lowercase()
            .contains(name)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}
