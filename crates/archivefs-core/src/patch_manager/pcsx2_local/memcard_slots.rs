//! Read-only PCSX2 memory-card slot assignment.
//!
//! PCSX2 stores its two console slots in `PCSX2.ini`:
//!
//! ```ini
//! [Folders]
//! MemoryCards = memcards        ; relative to the profile root, or absolute
//! [MemoryCards]
//! Slot1_Enable = true
//! Slot1_Filename = Mcd001.ps2
//! Slot2_Enable = true
//! Slot2_Filename = Mcd002.ps2
//! ```
//!
//! This module only reads those settings and stats the named card. It never
//! creates, opens for writing, renames or repairs anything. Any setting it
//! cannot interpret with certainty is reported as `Unsupported` with the
//! reason, never resolved by guessing.

use super::{Pcsx2Config, Pcsx2Settings, parse_ini_bool};
use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// PCSX2 has two console memory-card ports (multitap slots are reported as
/// a note only).
pub const PCSX2_MEMCARD_SLOT_COUNT: u8 = 2;
const DEFAULT_MEMCARDS_FOLDER: &str = "memcards";
/// Bound on retained slot-related settings per INI file.
pub(super) const MAX_RETAINED_MEMCARD_VALUES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2MemcardSlotState {
    /// The slot is enabled and the named card exists.
    Enabled,
    /// The slot is switched off in PCSX2's settings.
    Disabled,
    /// The slot is enabled but no card exists at the configured path.
    Missing,
    /// The slot is enabled but has no card filename.
    Unassigned,
    /// The configuration cannot be interpreted with certainty.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2MemcardSlotSource {
    /// Values were read from PCSX2's settings file.
    Settings,
    /// PCSX2's settings file was readable but did not name this slot, so
    /// PCSX2's built-in default (`Mcd00N.ps2`, enabled) applies.
    Default,
    /// No assignment could be determined.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pcsx2MemcardSlot {
    /// 1 or 2, as PCSX2 labels them.
    pub slot: u8,
    pub state: Pcsx2MemcardSlotState,
    pub source: Pcsx2MemcardSlotSource,
    pub filename: Option<String>,
    pub path: Option<PathBuf>,
    /// PCSX2 "folder memory card": the configured path is a directory.
    /// Its contents are not a card image and are not inspected.
    pub folder_card: bool,
    /// Plain-language reason for `Unsupported`, or an extra note.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Pcsx2MemcardSlots {
    /// The memory-card folder PCSX2 resolves card filenames against.
    pub folder: Option<PathBuf>,
    pub slots: Vec<Pcsx2MemcardSlot>,
    pub notes: Vec<String>,
}

impl Pcsx2MemcardSlots {
    /// Paths of slots that are enabled and point at an existing card image
    /// file (not a folder card), in slot order.
    pub fn assigned_card_files(&self) -> impl Iterator<Item = &Path> {
        self.slots
            .iter()
            .filter(|slot| slot.state == Pcsx2MemcardSlotState::Enabled && !slot.folder_card)
            .filter_map(|slot| slot.path.as_deref())
    }
}

/// Whether a `[section] key` pair is one this module reads. Section and
/// key comparison is case-insensitive, matching PCSX2's own INI reader.
pub(super) fn is_memcard_setting(section: &str, key: &str) -> bool {
    let section = section.to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    match section.as_str() {
        "folders" => key == "memorycards",
        "memorycards" => {
            (1..=PCSX2_MEMCARD_SLOT_COUNT)
                .any(|n| key == format!("slot{n}_enable") || key == format!("slot{n}_filename"))
                || key.starts_with("multitap")
        }
        "emucore" => key.starts_with("multitapport") && key.ends_with("_enabled"),
        _ => false,
    }
}

/// One setting from `[section] key`, or an error when the file names it
/// more than once with different values (PCSX2's choice is then unknown).
fn setting<'a>(
    settings: &'a Pcsx2Settings,
    section: &str,
    key: &str,
) -> Result<Option<&'a str>, String> {
    let mut found: Option<&str> = None;
    for (s, k, value) in &settings.memcard_values {
        if s.eq_ignore_ascii_case(section) && k.eq_ignore_ascii_case(key) {
            match found {
                Some(previous) if previous != value => {
                    return Err(format!(
                        "PCSX2's settings name [{section}] {key} more than once with different values."
                    ));
                }
                _ => found = Some(value.as_str()),
            }
        }
    }
    Ok(found)
}

fn unsupported(slot: u8, detail: impl Into<String>) -> Pcsx2MemcardSlot {
    Pcsx2MemcardSlot {
        slot,
        state: Pcsx2MemcardSlotState::Unsupported,
        source: Pcsx2MemcardSlotSource::Unknown,
        filename: None,
        path: None,
        folder_card: false,
        detail: Some(detail.into()),
    }
}

/// Resolves the memory-card folder from `[Folders] MemoryCards`.
fn memcards_folder(configuration_path: &Path, settings: &Pcsx2Settings) -> Result<PathBuf, String> {
    let value = setting(settings, "Folders", "MemoryCards")?.unwrap_or("");
    if value.is_empty() {
        return Ok(configuration_path.join(DEFAULT_MEMCARDS_FOLDER));
    }
    if value.chars().any(char::is_control) {
        return Err("PCSX2's memory-card folder setting contains control characters.".into());
    }
    let path = Path::new(value);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        configuration_path.join(path)
    })
}

/// A card filename PCSX2 resolves inside the memory-card folder. Anything
/// with a folder component, an absolute path or control characters is
/// refused rather than resolved by assumption.
fn plain_filename(value: &str) -> Result<&str, String> {
    let mut components = Path::new(value).components();
    let plain = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !value.contains(['/', '\\'])
        && !value.chars().any(char::is_control);
    if plain {
        Ok(value)
    } else {
        Err(format!(
            "The card filename \"{value}\" includes a folder or unusual characters; EmuWiz does not guess where PCSX2 looks for it."
        ))
    }
}

fn resolve_slot(slot: u8, folder: &Path, settings: &Pcsx2Settings) -> Pcsx2MemcardSlot {
    let enable_key = format!("Slot{slot}_Enable");
    let filename_key = format!("Slot{slot}_Filename");
    let enable = match setting(settings, "MemoryCards", &enable_key) {
        Ok(value) => value,
        Err(detail) => return unsupported(slot, detail),
    };
    let filename = match setting(settings, "MemoryCards", &filename_key) {
        Ok(value) => value,
        Err(detail) => return unsupported(slot, detail),
    };
    let source = if enable.is_none() && filename.is_none() {
        Pcsx2MemcardSlotSource::Default
    } else {
        Pcsx2MemcardSlotSource::Settings
    };
    let enabled = match enable {
        None => true,
        Some(value) => match parse_ini_bool(value) {
            Some(enabled) => enabled,
            None => {
                return unsupported(
                    slot,
                    format!("PCSX2's {enable_key} value \"{value}\" is not on/off."),
                );
            }
        },
    };
    let default_name = format!("Mcd{slot:03}.ps2");
    let filename = filename.unwrap_or(&default_name);
    let base = Pcsx2MemcardSlot {
        slot,
        state: Pcsx2MemcardSlotState::Unassigned,
        source,
        filename: None,
        path: None,
        folder_card: false,
        detail: None,
    };
    if filename.is_empty() {
        return Pcsx2MemcardSlot {
            state: if enabled {
                Pcsx2MemcardSlotState::Unassigned
            } else {
                Pcsx2MemcardSlotState::Disabled
            },
            ..base
        };
    }
    let filename = match plain_filename(filename) {
        Ok(filename) => filename,
        Err(detail) => return unsupported(slot, detail),
    };
    let path = folder.join(filename);
    let refuse = |path: PathBuf, detail: String| Pcsx2MemcardSlot {
        filename: Some(filename.to_string()),
        path: Some(path),
        ..unsupported(slot, detail)
    };
    let (exists, folder_card) = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return refuse(
                path,
                "The card path is a symbolic link; EmuWiz does not follow it.".into(),
            );
        }
        Ok(meta) if meta.is_dir() => (true, true),
        Ok(meta) if meta.is_file() => (true, false),
        Ok(_) => {
            return refuse(
                path,
                "The card path is not a regular file or folder.".into(),
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, false),
        Err(error) => return refuse(path, format!("The card could not be checked: {error}")),
    };
    let state = match (enabled, exists) {
        (false, _) => Pcsx2MemcardSlotState::Disabled,
        (true, true) => Pcsx2MemcardSlotState::Enabled,
        (true, false) => Pcsx2MemcardSlotState::Missing,
    };
    Pcsx2MemcardSlot {
        state,
        filename: Some(filename.to_string()),
        path: Some(path),
        folder_card,
        detail: (!enabled && !exists)
            .then(|| "The named card file does not exist either.".to_string()),
        ..base
    }
}

/// Which cards PCSX2 assigns to Slot 1 and Slot 2, from the global settings
/// file (and, when given, a per-game settings file). Read-only.
pub fn resolve_pcsx2_memcard_slots(
    configuration_path: &Path,
    global: &Pcsx2Config,
    per_game: Option<&Pcsx2Config>,
) -> Pcsx2MemcardSlots {
    if !global.readable {
        let reason = if global.exists {
            "PCSX2's settings file could not be read, so its memory-card slots are unknown."
        } else {
            "PCSX2's settings file was not found, so its memory-card slots are unknown."
        };
        return Pcsx2MemcardSlots {
            folder: None,
            slots: (1..=PCSX2_MEMCARD_SLOT_COUNT)
                .map(|slot| unsupported(slot, reason))
                .collect(),
            notes: Vec::new(),
        };
    }
    let settings = &global.settings;
    let folder = match memcards_folder(configuration_path, settings) {
        Ok(folder) => folder,
        Err(detail) => {
            return Pcsx2MemcardSlots {
                folder: None,
                slots: (1..=PCSX2_MEMCARD_SLOT_COUNT)
                    .map(|slot| unsupported(slot, detail.clone()))
                    .collect(),
                notes: Vec::new(),
            };
        }
    };
    let per_game_names_slots = |slot: u8| {
        per_game.is_some_and(|config| {
            config.readable
                && config.settings.memcard_values.iter().any(|(s, k, _)| {
                    s.eq_ignore_ascii_case("MemoryCards")
                        && k.to_ascii_lowercase().starts_with(&format!("slot{slot}_"))
                })
        })
    };
    let slots = (1..=PCSX2_MEMCARD_SLOT_COUNT)
        .map(|slot| {
            if per_game_names_slots(slot) {
                Pcsx2MemcardSlot {
                    source: Pcsx2MemcardSlotSource::Unknown,
                    ..unsupported(
                        slot,
                        "This game's own PCSX2 settings also name a card for this slot; EmuWiz cannot confirm which one PCSX2 uses.",
                    )
                }
            } else {
                resolve_slot(slot, &folder, settings)
            }
        })
        .collect();
    let mut notes = Vec::new();
    let multitap_on = settings.memcard_values.iter().any(|(s, k, v)| {
        s.eq_ignore_ascii_case("EmuCore")
            && k.to_ascii_lowercase().starts_with("multitapport")
            && parse_ini_bool(v) == Some(true)
    });
    if multitap_on {
        notes.push(
            "A PCSX2 multitap is enabled; cards in its extra slots are not shown here.".into(),
        );
    }
    Pcsx2MemcardSlots {
        folder: Some(folder),
        slots,
        notes,
    }
}

#[cfg(test)]
mod tests;
