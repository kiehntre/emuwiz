//! Finding PlayStation memory cards. Read-only: a card is recognised from
//! its size and first bytes, never from its extension, and nothing is opened
//! for writing.

use super::{CardFormat, MAX_CARD_BYTES, PS2_MAGIC};
use crate::save_state_orchestration::inventory_configured_state;
use serde::Serialize;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundCard {
    pub path: PathBuf,
    /// The emulator whose save folder the card was found in, when known.
    pub emulator: Option<String>,
    pub format: CardFormat,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CardSearch {
    pub cards: Vec<FoundCard>,
    /// Save folders that are configured but could not be read.
    pub unavailable: Vec<String>,
}

const PS1_CARD: u64 = ps1_memcard::CAPACITY as u64;

/// What kind of card `path` is, from its size and header. Reads at most a
/// few kilobytes; the full structure is checked when the card is opened.
pub fn sniff_format(path: &Path) -> CardFormat {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return CardFormat::NotACard;
    };
    if metadata.file_type().is_symlink() {
        return CardFormat::NotACard;
    }
    if metadata.is_dir() {
        let marker = path.join("_pcsx2_superblock");
        return if fs::symlink_metadata(marker).is_ok_and(|marker| marker.is_file()) {
            CardFormat::Pcsx2Folder
        } else {
            CardFormat::NotACard
        };
    }
    let length = metadata.len();
    if !metadata.is_file() || length > MAX_CARD_BYTES {
        return CardFormat::NotACard;
    }
    let ps1 = match length.checked_sub(PS1_CARD) {
        Some(0) => Some((0, CardFormat::Ps1Raw, &b""[..])),
        Some(3904) => Some((3904, CardFormat::Ps1DexDrive, &b"123-456-STD"[..])),
        Some(64) => Some((64, CardFormat::Ps1Vgs, &b"VgsM"[..])),
        Some(128) => Some((128, CardFormat::Ps1Vmp, &b"\0PMV"[..])),
        _ => None,
    };
    let Ok(mut file) = fs::File::open(path) else {
        return CardFormat::NotACard;
    };
    let mut head = [0u8; 32];
    if file.read_exact(&mut head).is_err() {
        return CardFormat::NotACard;
    }
    if let Some((offset, format, magic)) = ps1 {
        let mut card_magic = [0u8; 2];
        let positioned =
            file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut card_magic).is_ok();
        return if positioned && head.starts_with(magic) && &card_magic == b"MC" {
            format
        } else {
            CardFormat::NotACard
        };
    }
    if head.starts_with(PS2_MAGIC) {
        // 528-byte pages when the size only divides that way, or when it
        // divides both ways and the page count is the power of two a card has.
        let raw_pages = length / 528;
        let with_ecc = length.is_multiple_of(528)
            && (!length.is_multiple_of(512) || raw_pages.is_power_of_two());
        return if with_ecc {
            CardFormat::Ps2WithEcc
        } else {
            CardFormat::Ps2DataOnly
        };
    }
    CardFormat::NotACard
}

/// The cards directly inside `folder` (not its sub-folders, except that a
/// PCSX2 folder card is itself a sub-folder). Sorted by path.
pub fn find_cards_in(folder: &Path, emulator: Option<&str>) -> Vec<FoundCard> {
    let mut cards: Vec<FoundCard> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| found(&entry.path(), emulator))
        .collect();
    cards.sort_by(|left, right| left.path.cmp(&right.path));
    cards
}

fn found(path: &Path, emulator: Option<&str>) -> Option<FoundCard> {
    let format = sniff_format(path);
    (format != CardFormat::NotACard).then(|| FoundCard {
        path: path.to_path_buf(),
        emulator: emulator.map(str::to_string),
        format,
        size_bytes: fs::symlink_metadata(path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map_or(0, |metadata| metadata.len()),
    })
}

/// The cards in the save folders of the emulators EmuWiz already knows
/// (the same read-only inventory the Saves page uses): PCSX2 and DuckStation
/// memory-card folders, RetroArch saves, RPCS3 PS1 Classics.
pub fn find_configured_cards() -> CardSearch {
    let configured = inventory_configured_state();
    let mut cards: Vec<FoundCard> = configured
        .inventory
        .records
        .iter()
        .filter_map(|record| found(&record.path, Some(record.emulator.as_str())))
        .collect();
    cards.sort_by(|left, right| left.path.cmp(&right.path));
    cards.dedup_by(|left, right| left.path == right.path);
    CardSearch {
        cards,
        unavailable: configured
            .unavailable
            .iter()
            .map(|root| {
                format!(
                    "{}: {}",
                    root.emulator.as_str(),
                    root.path
                        .as_ref()
                        .map(|path| format!("{} ({})", path.display(), root.reason))
                        .unwrap_or_else(|| root.reason.clone())
                )
            })
            .collect(),
    }
}
