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
//! - files that reference the game by name (`.cue`/`.m3u`/`.gdi` beside it)
//!   would break, so such a rename is refused rather than silently breaking
//!   launch;
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
use crate::dat::rename_apply::{ApplyOutcome, RenameTransaction};
use crate::safe_read::TrustedRoots;

/// Longest file name accepted (bytes), well under common filesystem limits.
const MAX_NAME_BYTES: usize = 200;

/// Sidecar types whose text names other game files.
const REFERENCING_EXTENSIONS: [&str; 3] = ["cue", "m3u", "gdi"];

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
            if typed.len() > suffix.len() && typed.to_lowercase().ends_with(&suffix.to_lowercase())
            {
                format!("{}{}", &typed[..typed.len() - suffix.len()], suffix)
            } else if let Some((_, other)) = typed.rsplit_once('.')
                && is_known_game_extension(other)
            {
                return Err(format!(
                    "The file must keep its .{ext} extension (you typed .{other})."
                ));
            } else {
                format!("{typed}{suffix}")
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

fn is_known_game_extension(extension: &str) -> bool {
    let ext = extension.to_ascii_lowercase();
    ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric()) && ext != "1"
}

/// Builds the read-only preview for renaming `source` to `typed`.
pub fn preview_game_rename(
    database: &crate::Database,
    source: &Path,
    typed: &str,
    generation: u64,
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
    if let Some(referrer) = referencing_sidecar(source)? {
        return Err(format!(
            "{} names this file, so renaming it alone would stop the game launching. Nothing was changed.",
            referrer.display()
        ));
    }
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
        format!("The file is renamed in place: {} → {}", current, new_file_name),
        "The game keeps its library entry: favourites, collections, play history and artwork stay with it.".into(),
        "Undo is available in History.".into(),
        "Emulator saves named after the old file (for example RetroArch .srm and save states) are not renamed yet.".into(),
        "RomM keeps its entry for this game when it next scans this platform (it matches the file by hash).".into(),
    ];
    Ok(GameRenamePreview {
        plan,
        approved,
        source: source.to_path_buf(),
        destination,
        source_root,
        new_file_name,
        notes,
    })
}

/// Applies a confirmed preview with the journalled executor. Everything is
/// re-checked against the live library and disk first; any change since the
/// preview refuses with nothing renamed.
pub fn apply_game_rename(
    preview: &GameRenamePreview,
    database: &crate::Database,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<ApplyOutcome, String> {
    if let Some(referrer) = referencing_sidecar(&preview.source)? {
        return Err(format!(
            "{} now names this file, so the rename was refused. Nothing was changed.",
            referrer.display()
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
    apply_organisation_transaction(
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
    .map_err(|error| error.to_string())
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

/// A `.cue`/`.m3u`/`.gdi` beside `source` whose text mentions its file name.
fn referencing_sidecar(source: &Path) -> Result<Option<PathBuf>, String> {
    let (Some(parent), Some(name)) = (source.parent(), source.file_name()) else {
        return Ok(None);
    };
    let name = name.to_string_lossy().to_lowercase();
    let entries = std::fs::read_dir(parent)
        .map_err(|error| format!("could not read {}: {error}", parent.display()))?;
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
        if let Ok(bytes) = std::fs::read(&path)
            && String::from_utf8_lossy(&bytes)
                .to_lowercase()
                .contains(&name)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}
