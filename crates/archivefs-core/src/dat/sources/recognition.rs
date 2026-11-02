//! Recognised systems for the registered catalogues, and the one explicit
//! step that applies the safe ones.
//!
//! Validation records each file catalogue's header (see
//! [`super::RecordedDatHeader`]). Nothing here changes a setting on its own:
//! [`recognition_rows`] and [`safe_matches`] only describe, and
//! [`apply_safe_matches`] is the single write. It writes nothing unless the
//! settings file is byte-for-byte what the preview was made from and every
//! catalogue in the preview is still exactly the file whose header was read.
//! It assigns systems only; it never enables a catalogue, reads a game file,
//! starts a scan or verification, or touches saved match records.

use std::path::Path;

use super::config::{
    DatSourcesRevision, DatSourcesSaveError, load_dat_sources_config_with_revision,
    save_dat_sources_config_if_unchanged,
};
use super::validation::observation_text;
use super::{
    DatHealthState, DatSourceEntry, DatSourceKind, DatSourceRegistry, PlatformProvenance,
    RecognitionReceipt,
};
use crate::dat::catalogue_recognition::{CatalogueRecognition, recognise_catalogue, same_platform};

/// How far recognition got for one catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecognitionState {
    /// A folder holds several catalogues; it is not recognised as a whole.
    Folder,
    /// Not checked by this version yet, or its last check failed.
    NotChecked,
    /// The file changed after its last check.
    Changed,
    Recognised(CatalogueRecognition),
}

/// What the recognition means for one catalogue, in the order a summary
/// lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecognitionGroup {
    /// An ordinary game list with no system set yet. "Apply all safe
    /// matches" would set it.
    SafeMatch,
    /// A system is set and the current header does not back it: it names
    /// another system, is extras or several systems for a system that was
    /// set automatically, or names no supported game list. Never changed
    /// automatically.
    Conflict,
    /// The right system, but extras. Only set if a person chooses to.
    Extras,
    /// The header names more than one system.
    Ambiguous,
    /// Not checked yet, or changed since it was checked.
    NeedsCheck,
    /// A system is set and the current header backs it: an ordinary game
    /// list for that system, or extras / one of several systems the person
    /// chose themselves. Also a manual choice not checked yet.
    AlreadySet,
    /// Turned off by the person; left alone.
    TurnedOff,
    /// For several systems, not a game list, or a system EmuWiz does not
    /// support.
    CannotUse,
}

/// One catalogue, as the recognition summary shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionRow {
    pub source_id: String,
    pub display_name: String,
    pub state: RecognitionState,
    /// The system already set for it, as a display name.
    pub assigned: Option<String>,
    /// The set system was applied from a recognised match rather than
    /// chosen by the person. Whether the header still backs it is `group`.
    pub assigned_by_recognition: bool,
    /// Who set the system, as saved.
    pub origin: PlatformProvenance,
    pub group: RecognitionGroup,
}

/// Recognition for one catalogue from its last validation, without reading
/// the catalogue again (only its size and modification time are looked at).
pub fn recognition_state(entry: &DatSourceEntry) -> RecognitionState {
    if entry.kind == DatSourceKind::Folder {
        return RecognitionState::Folder;
    }
    if !matches!(
        entry.health.state(),
        DatHealthState::Valid | DatHealthState::ValidWithWarnings
    ) {
        return RecognitionState::NotChecked;
    }
    let Some(header) = entry.health.current_header() else {
        return RecognitionState::NotChecked;
    };
    if observation_text(&entry.path).as_deref() != Some(header.observed.as_str()) {
        return RecognitionState::Changed;
    }
    RecognitionState::Recognised(recognise_catalogue(
        header.name.as_deref(),
        header.ecosystem,
    ))
}

/// Whether the entry's system was applied from a recognised match, not
/// chosen by the person.
fn assigned_by_recognition(entry: &DatSourceEntry) -> bool {
    entry.platform_provenance() == PlatformProvenance::AppliedFromRecognition
}

fn group(entry: &DatSourceEntry, state: &RecognitionState) -> RecognitionGroup {
    if let Some(assigned) = entry.platform.as_deref() {
        return assigned_group(entry, assigned, state);
    }
    if !entry.enabled {
        return RecognitionGroup::TurnedOff;
    }
    match state {
        RecognitionState::Folder => RecognitionGroup::CannotUse,
        RecognitionState::NotChecked | RecognitionState::Changed => RecognitionGroup::NeedsCheck,
        RecognitionState::Recognised(recognition) => match recognition {
            CatalogueRecognition::Games { .. } => RecognitionGroup::SafeMatch,
            CatalogueRecognition::Extras { .. } => RecognitionGroup::Extras,
            CatalogueRecognition::Ambiguous { .. } => RecognitionGroup::Ambiguous,
            CatalogueRecognition::SeveralSystems
            | CatalogueRecognition::NotGames { .. }
            | CatalogueRecognition::NotRecognised { .. }
            | CatalogueRecognition::NoName => RecognitionGroup::CannotUse,
        },
    }
}

/// A catalogue that already has a system. Nothing here changes the setting;
/// it only says whether the catalogue's current header backs it.
///
/// A header that names nothing usable (not a game list, several systems, an
/// unsupported system, no name) never counts as agreeing. Extras, or one
/// system of several the header names, back only a system the person chose;
/// a system set automatically must still match an ordinary game list.
fn assigned_group(
    entry: &DatSourceEntry,
    assigned: &str,
    state: &RecognitionState,
) -> RecognitionGroup {
    let by_recognition = assigned_by_recognition(entry);
    // Only a person's own choice may stand for extras or for one system of
    // several; a recognised or unrecorded system must match a game list.
    let chosen = entry.platform_provenance() == PlatformProvenance::ChosenByPerson;
    let set = crate::canonical_platform_for_alias(assigned);
    let names = |platform: &str| set.is_some_and(|set| same_platform(set, platform));
    match state {
        RecognitionState::Recognised(recognition) => {
            let backed = match recognition {
                CatalogueRecognition::Games { platform } => names(platform),
                CatalogueRecognition::Extras { platform, .. } => chosen && names(platform),
                CatalogueRecognition::Ambiguous { platforms } => {
                    chosen && platforms.iter().any(|platform| names(platform))
                }
                CatalogueRecognition::SeveralSystems
                | CatalogueRecognition::NotGames { .. }
                | CatalogueRecognition::NotRecognised { .. }
                | CatalogueRecognition::NoName => false,
            };
            if backed {
                RecognitionGroup::AlreadySet
            } else {
                RecognitionGroup::Conflict
            }
        }
        // Set automatically, but the header it came from is no longer
        // current: check the catalogue again before trusting it.
        RecognitionState::NotChecked | RecognitionState::Changed if by_recognition => {
            RecognitionGroup::NeedsCheck
        }
        RecognitionState::NotChecked | RecognitionState::Changed | RecognitionState::Folder => {
            RecognitionGroup::AlreadySet
        }
    }
}

/// Every registered catalogue with its recognition, sorted by group, then
/// name.
pub fn recognition_rows(registry: &DatSourceRegistry) -> Vec<RecognitionRow> {
    let mut rows: Vec<RecognitionRow> = registry
        .entries()
        .iter()
        .map(|entry| {
            let state = recognition_state(entry);
            RecognitionRow {
                source_id: entry.id.clone(),
                display_name: entry.display_name.clone(),
                assigned: entry.platform_display(),
                assigned_by_recognition: assigned_by_recognition(entry),
                origin: entry.platform_provenance(),
                group: group(entry, &state),
                state,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        a.group
            .cmp(&b.group)
            .then_with(|| a.display_name.cmp(&b.display_name))
            .then_with(|| a.source_id.cmp(&b.source_id))
    });
    rows
}

/// One system "Apply all safe matches" would set, with the exact evidence it
/// was decided from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeMatch {
    pub source_id: String,
    pub display_name: String,
    pub platform: &'static str,
    pub header_name: String,
    /// The `size:seconds:nanoseconds` of the file whose header was read.
    pub observed: String,
}

/// The safe matches in `registry`: enabled file catalogues with no system
/// set, whose current header is an ordinary game list for one system.
pub fn safe_matches(registry: &DatSourceRegistry) -> Vec<SafeMatch> {
    let mut matches: Vec<SafeMatch> = registry.entries().iter().filter_map(safe_match).collect();
    matches.sort_by(|a, b| {
        a.display_name
            .cmp(&b.display_name)
            .then_with(|| a.source_id.cmp(&b.source_id))
    });
    matches
}

fn safe_match(entry: &DatSourceEntry) -> Option<SafeMatch> {
    let state = recognition_state(entry);
    if group(entry, &state) != RecognitionGroup::SafeMatch {
        return None;
    }
    let RecognitionState::Recognised(CatalogueRecognition::Games { platform }) = state else {
        return None;
    };
    let header = entry.health.current_header()?;
    Some(SafeMatch {
        source_id: entry.id.clone(),
        display_name: entry.display_name.clone(),
        platform,
        header_name: header.name.clone()?,
        observed: header.observed.clone(),
    })
}

/// Why nothing was applied.
#[derive(Debug)]
pub enum ApplyRefusal {
    /// The preview was empty.
    NothingToApply,
    /// The settings file could not be read, changed, or could not be saved.
    Settings(DatSourcesSaveError),
    /// A catalogue in the preview no longer matches it.
    Stale {
        display_name: String,
        reason: &'static str,
    },
}

impl std::fmt::Display for ApplyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingToApply => f.write_str("There was nothing to apply."),
            Self::Settings(error) => write!(f, "Nothing was changed: {error}."),
            Self::Stale {
                display_name,
                reason,
            } => write!(
                f,
                "Nothing was changed: {display_name} {reason}. Check your catalogues again, \
                 then review the new list."
            ),
        }
    }
}

/// The result of an apply.
#[derive(Debug, Clone)]
pub struct AppliedMatches {
    pub registry: DatSourceRegistry,
    pub revision: DatSourcesRevision,
    pub applied: Vec<SafeMatch>,
    /// The Undo receipt batch every applied catalogue now carries.
    pub batch: String,
}

/// Sets the system of every catalogue in `preview`, all or nothing.
///
/// `checked` is the registry the preview was made from. Its validation
/// records may be newer than the saved ones (a check the person has not
/// saved yet); for each previewed catalogue that record is saved too, so
/// the system is never saved without the header it was recognised from.
///
/// Refuses, writing nothing, unless the settings file still has
/// `expected` (so the person saw the current setup) and each previewed
/// catalogue is, in the saved settings, still the same file, enabled and
/// with no system set; is still recognised from `checked` as the same
/// ordinary game list; and is on disk still exactly the file (size and
/// modification time) whose header was read.
pub fn apply_safe_matches(
    config_path: &Path,
    expected: &DatSourcesRevision,
    checked: &DatSourceRegistry,
    preview: &[SafeMatch],
) -> Result<AppliedMatches, ApplyRefusal> {
    if preview.is_empty() {
        return Err(ApplyRefusal::NothingToApply);
    }
    let (config, revision) = load_dat_sources_config_with_revision(config_path)
        .map_err(|error| ApplyRefusal::Settings(DatSourcesSaveError::Failed(error)))?;
    if &revision != expected {
        return Err(ApplyRefusal::Settings(
            DatSourcesSaveError::ChangedElsewhere,
        ));
    }
    let (mut registry, _) = DatSourceRegistry::from_config(&config);
    for item in preview {
        let stale = |reason| ApplyRefusal::Stale {
            display_name: item.display_name.clone(),
            reason,
        };
        let saved = registry
            .get(&item.source_id)
            .ok_or_else(|| stale("is no longer in your setup"))?;
        let checked_entry = checked
            .get(&item.source_id)
            .ok_or_else(|| stale("is no longer in your setup"))?;
        if saved.path != checked_entry.path || saved.kind != checked_entry.kind {
            return Err(stale("now points at a different file"));
        }
        if saved.platform.is_some() || checked_entry.platform.is_some() {
            return Err(stale("now has a system set"));
        }
        if !saved.enabled || !checked_entry.enabled {
            return Err(stale("has been turned off"));
        }
        // Recomputed from the checked record, including a fresh look at the
        // file's size and modification time.
        if safe_match(checked_entry).as_ref() != Some(item) {
            return Err(stale("changed after it was checked"));
        }
    }
    // The Undo receipt goes into the same atomic write as the systems, so
    // there is no moment at which a system is saved without its receipt.
    let batch = format!("{}-{}", now_unix_nanos(), expected.label());
    for item in preview {
        let health = checked
            .get(&item.source_id)
            .map(|entry| entry.health.clone());
        if let (Some(entry), Some(health)) = (registry.get_mut(&item.source_id), health) {
            entry.health = health;
            entry.platform = Some(item.platform.to_string());
            entry.platform_recognised_from = Some(item.header_name.clone());
            entry.platform_chosen_by_person = false;
            entry.recognition_receipt = Some(RecognitionReceipt {
                batch: batch.clone(),
                platform: item.platform.to_string(),
            });
        }
    }
    let revision =
        save_dat_sources_config_if_unchanged(config_path, expected, &registry.to_config())
            .map_err(ApplyRefusal::Settings)?;
    Ok(AppliedMatches {
        registry,
        revision,
        applied: preview.to_vec(),
        batch,
    })
}

fn now_unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

/// Whether Undo may clear this catalogue's system: it still carries the
/// receipt and the exact system that apply wrote, and nobody chose a system
/// for it since (choosing one by hand drops the receipt).
fn still_as_applied(entry: &DatSourceEntry, receipt: &RecognitionReceipt) -> bool {
    entry.platform.as_deref() == Some(receipt.platform.as_str())
        && entry.platform_recognised_from.is_some()
}

/// The most recent apply that Undo can still reverse, read from the saved
/// receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoableApply {
    pub batch: String,
    /// Catalogues whose system Undo would clear, by display name.
    pub catalogues: Vec<String>,
}

/// Orders batches by when they were applied; an unreadable time sorts first.
fn batch_order(batch: &str) -> (u128, &str) {
    let nanos = batch
        .split_once('-')
        .and_then(|(nanos, _)| nanos.parse().ok())
        .unwrap_or_default();
    (nanos, batch)
}

/// The latest apply with at least one catalogue still exactly as it set it.
pub fn last_undoable_apply(registry: &DatSourceRegistry) -> Option<UndoableApply> {
    let batch = registry
        .entries()
        .iter()
        .filter_map(|entry| {
            let receipt = entry.recognition_receipt.as_ref()?;
            still_as_applied(entry, receipt).then_some(receipt.batch.as_str())
        })
        .max_by(|a, b| batch_order(a).cmp(&batch_order(b)))?
        .to_string();
    let catalogues = registry
        .entries()
        .iter()
        .filter(|entry| {
            entry
                .recognition_receipt
                .as_ref()
                .is_some_and(|receipt| receipt.batch == batch && still_as_applied(entry, receipt))
        })
        .map(|entry| entry.display_name.clone())
        .collect();
    Some(UndoableApply { batch, catalogues })
}

/// The result of an Undo.
#[derive(Debug, Clone)]
pub struct UndoneApply {
    pub registry: DatSourceRegistry,
    pub revision: DatSourcesRevision,
    /// Source ids whose system was cleared.
    pub cleared: Vec<String>,
    /// Display names of catalogues from that apply that were changed later
    /// and so were left exactly as they are.
    pub left_alone: Vec<String>,
}

/// Undoes the apply `batch` from its saved receipts. Works after a restart
/// or reload because nothing is kept in memory.
///
/// Clears the system of each catalogue that still holds exactly what the
/// apply wrote, and leaves every other setting and every later change alone.
/// The receipts of that apply are removed in the same atomic, conditional
/// write, so repeating Undo finds nothing to do and changes nothing.
pub fn undo_apply(
    config_path: &Path,
    expected: &DatSourcesRevision,
    batch: &str,
) -> Result<UndoneApply, ApplyRefusal> {
    let (config, revision) = load_dat_sources_config_with_revision(config_path)
        .map_err(|error| ApplyRefusal::Settings(DatSourcesSaveError::Failed(error)))?;
    if &revision != expected {
        return Err(ApplyRefusal::Settings(
            DatSourcesSaveError::ChangedElsewhere,
        ));
    }
    let (mut registry, _) = DatSourceRegistry::from_config(&config);
    let ids: Vec<String> = registry
        .entries()
        .iter()
        .filter(|entry| {
            entry
                .recognition_receipt
                .as_ref()
                .is_some_and(|receipt| receipt.batch == batch)
        })
        .map(|entry| entry.id.clone())
        .collect();
    let mut cleared = Vec::new();
    let mut left_alone = Vec::new();
    for id in &ids {
        let Some(entry) = registry.get_mut(id) else {
            continue;
        };
        let Some(receipt) = entry.recognition_receipt.take() else {
            continue;
        };
        if still_as_applied(entry, &receipt) {
            entry.platform = None;
            entry.platform_recognised_from = None;
            cleared.push(id.clone());
        } else {
            left_alone.push(entry.display_name.clone());
        }
    }
    if cleared.is_empty() {
        return Err(ApplyRefusal::NothingToApply);
    }
    let revision =
        save_dat_sources_config_if_unchanged(config_path, expected, &registry.to_config())
            .map_err(ApplyRefusal::Settings)?;
    Ok(UndoneApply {
        registry,
        revision,
        cleared,
        left_alone,
    })
}

#[cfg(test)]
#[path = "recognition/tests.rs"]
mod tests;
