//! PlayStation memory-card manager: find PS1 and PS2 cards, show what is on
//! them, back them up, and move single saves in and out.
//!
//! Three rules hold for everything here:
//!
//! 1. **A card that is opened is never written.** Reading, backing up and
//!    exporting only read it. Importing a save writes a *new* card file (the
//!    card plus the save) and leaves the card it started from untouched; the
//!    result is refused if that file already exists. Changing a live card in
//!    place stays with the guarded, journaled restore in
//!    [`crate::memory_card_inventory::restore_guard`].
//! 2. **Two readers must agree.** A card is read by EmuWiz's own reader
//!    ([`crate::memory_card_inventory`]) and by the `ps1-memcard` /
//!    `ps2-memcard` crates. If they list different saves, the card is shown
//!    with a warning and nothing is exported from or imported into it.
//! 3. **A product code is evidence about the save, not a verified game.**
//!    `BASLUS-00594…` names the save; a multi-disc game may use one code for
//!    every disc. It is shown as written and never promoted to an identity.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub mod backup;
pub mod discovery;
pub mod external_tools;
mod ps1;
mod ps2;
#[cfg(test)]
mod tests;

/// Largest file read as a card (a 64 MB PS2 card with ECC is ~66 MiB).
pub const MAX_CARD_BYTES: u64 = crate::memory_card_inventory::PS2_MAX_CARD_BYTES as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardPlatform {
    PlayStation,
    PlayStation2,
}

impl CardPlatform {
    pub fn label(self) -> &'static str {
        match self {
            Self::PlayStation => "PlayStation",
            Self::PlayStation2 => "PlayStation 2",
        }
    }
}

/// What kind of card file this is, decided from its content, never from its
/// extension (a 128 KB Game Boy Advance `.srm` is not a PlayStation card).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardFormat {
    /// Plain 131,072-byte image: DuckStation `.mcd`, ePSXe `.mcr`,
    /// RetroArch `.srm`, PCSX2's PS1 cards.
    Ps1Raw,
    /// DexDrive `.gme`: a 3,904-byte header in front of the card.
    Ps1DexDrive,
    /// Connectix VGS `.vgs` / `.mem`: a 64-byte header in front of the card.
    Ps1Vgs,
    /// PSP / PS3 / RPCS3 `.vmp`: a signed 128-byte header in front of the card.
    Ps1Vmp,
    /// PS2 card with 16 spare (ECC) bytes after every 512-byte page: what
    /// PCSX2 writes (`.ps2`, 8,650,752 bytes for an 8 MB card).
    Ps2WithEcc,
    /// PS2 card of plain 512-byte pages (`.mcd`, `.vmc`, `.bin`, `.mc2`).
    Ps2DataOnly,
    /// PCSX2 folder memory card: a directory, one sub-folder per save.
    Pcsx2Folder,
    NotACard,
}

impl CardFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ps1Raw => "PS1 card (plain 128 KB image)",
            Self::Ps1DexDrive => "PS1 card (DexDrive .gme)",
            Self::Ps1Vgs => "PS1 card (VGS .vgs/.mem)",
            Self::Ps1Vmp => "PS1 card (PSP/PS3 .vmp, signed)",
            Self::Ps2WithEcc => "PS2 card (with ECC, PCSX2 .ps2)",
            Self::Ps2DataOnly => "PS2 card (no ECC)",
            Self::Pcsx2Folder => "PS2 folder card (PCSX2)",
            Self::NotACard => "Not a PlayStation memory card",
        }
    }

    pub fn platform(self) -> Option<CardPlatform> {
        match self {
            Self::Ps1Raw | Self::Ps1DexDrive | Self::Ps1Vgs | Self::Ps1Vmp => {
                Some(CardPlatform::PlayStation)
            }
            Self::Ps2WithEcc | Self::Ps2DataOnly | Self::Pcsx2Folder => {
                Some(CardPlatform::PlayStation2)
            }
            Self::NotACard => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardHealth {
    /// Both readers read the card and list the same saves.
    Healthy,
    /// Readable, but something is off; see the notes. Nothing is exported
    /// from or imported into a card in this state.
    Warnings,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardSave {
    /// The name the card stores (PS1 file name / PS2 folder name). This is
    /// what selects a save for export.
    pub name: String,
    /// Product code written into the save name, for example `SLUS-00594`.
    pub product_code: Option<String>,
    pub region: Option<&'static str>,
    /// The title the game wrote into the save.
    pub title: Option<String>,
    pub size_bytes: u64,
    /// PS1: blocks used (of 15).
    pub blocks: Option<u32>,
    /// PS2: files in the save folder.
    pub file_count: Option<usize>,
    /// PS2: last-modified time stored on the card.
    pub modified: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardReport {
    pub path: PathBuf,
    pub format: CardFormat,
    pub size_bytes: u64,
    /// SHA-256 of the file as read; `None` for a folder card.
    pub sha256: Option<String>,
    pub health: CardHealth,
    pub saves: Vec<CardSave>,
    pub free_space: Option<String>,
    /// Whether EmuWiz's reader and the library reader list the same saves.
    pub readers_agree: Option<bool>,
    /// Single-save format EmuWiz can export from this card, if any.
    pub export_format: Option<&'static str>,
    /// Single-save formats EmuWiz can import into a copy of this card.
    pub import_formats: Vec<&'static str>,
    pub notes: Vec<String>,
    /// Plain statements of what EmuWiz cannot do with this card.
    pub unsupported: Vec<String>,
}

impl CardReport {
    pub fn save(&self, name: &str) -> Option<&CardSave> {
        self.saves.iter().find(|save| save.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardError {
    /// Not a regular file (or, for a folder card, not a plain directory).
    NotAFile(PathBuf),
    TooLarge(u64),
    NotACard(String),
    Unsupported(String),
    /// EmuWiz will not do this to this card; the text says why.
    Refused(String),
    SaveNotFound(String),
    DestinationExists(PathBuf),
    /// Something was written and did not check out; it has been removed.
    VerificationFailed(String),
    Io(String),
}

impl std::fmt::Display for CardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAFile(path) => write!(
                formatter,
                "{} is not a regular file (links are not followed)",
                path.display()
            ),
            Self::TooLarge(bytes) => write!(
                formatter,
                "the file is {bytes} bytes, larger than any supported memory card"
            ),
            Self::NotACard(why) => write!(formatter, "not a PlayStation memory card: {why}"),
            Self::Unsupported(what) => write!(formatter, "not supported: {what}"),
            Self::Refused(why) => write!(formatter, "refused: {why}"),
            Self::SaveNotFound(name) => write!(formatter, "no save named {name:?} on this card"),
            Self::DestinationExists(path) => write!(
                formatter,
                "{} already exists; nothing is ever overwritten",
                path.display()
            ),
            Self::VerificationFailed(why) => write!(formatter, "verification failed: {why}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CardError {}

pub(crate) fn io_error(context: &str, path: &Path, error: std::io::Error) -> CardError {
    CardError::Io(format!("{context} {}: {error}", path.display()))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Reads a card file's bytes without following links. Read-only.
pub(crate) fn read_card_bytes(path: &Path) -> Result<Vec<u8>, CardError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| io_error("cannot read", path, error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CardError::NotAFile(path.to_path_buf()));
    }
    if metadata.len() > MAX_CARD_BYTES {
        return Err(CardError::TooLarge(metadata.len()));
    }
    fs::read(path).map_err(|error| io_error("cannot read", path, error))
}

const PS2_MAGIC: &[u8] = b"Sony PS2 Memory Card Format ";

/// Opens a card read-only and reports what is on it.
///
/// A file that is not a PlayStation card is an `Ok` report with
/// [`CardFormat::NotACard`], so a folder listing can show it plainly.
pub fn read_card(path: &Path) -> Result<CardReport, CardError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| io_error("cannot read", path, error))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return ps2::folder_report(path);
    }
    let bytes = read_card_bytes(path)?;
    Ok(report_bytes(path, &bytes))
}

pub(crate) fn report_bytes(path: &Path, bytes: &[u8]) -> CardReport {
    let sha256 = sha256_hex(bytes);
    if let Some(format) = ps1::detect(bytes) {
        return ps1::report(path, bytes, format, sha256);
    }
    if bytes.starts_with(PS2_MAGIC) {
        return ps2::report(path, bytes, sha256);
    }
    let why = if bytes.len() == ps1_memcard::CAPACITY {
        "128 KB like a PS1 card, but it does not start with the card header `MC` \
         (another system's save file of the same size)"
    } else {
        "neither a PS1 card (128 KB, optionally behind a DexDrive, VGS or PSP header) \
         nor a PS2 card (no `Sony PS2 Memory Card Format` header)"
    };
    CardReport {
        path: path.to_path_buf(),
        format: CardFormat::NotACard,
        size_bytes: bytes.len() as u64,
        sha256: Some(sha256),
        health: CardHealth::Unreadable,
        saves: Vec::new(),
        free_space: None,
        readers_agree: None,
        export_format: None,
        import_formats: Vec::new(),
        notes: vec![why.to_string()],
        unsupported: Vec::new(),
    }
}

/// `BASLUS-00594DRAX00` -> (`SLUS-00594`, `America`).
///
/// Evidence about the save only: the code is the one the game chose to put
/// in the save name, which is not always the disc that wrote it.
pub fn product_code_and_region(save_name: &str) -> (Option<String>, Option<&'static str>) {
    let bytes = save_name.as_bytes();
    if bytes.len() < 12 || bytes[0] != b'B' {
        return (None, None);
    }
    let serial = &bytes[2..12];
    let shaped = serial[..4].iter().all(u8::is_ascii_uppercase)
        && serial[4] == b'-'
        && serial[5..].iter().all(u8::is_ascii_digit);
    if !shaped {
        return (None, None);
    }
    let region = match bytes[1] {
        b'A' => Some("America"),
        b'E' => Some("Europe"),
        b'I' => Some("Japan"),
        b'K' => Some("Korea"),
        b'C' => Some("China"),
        _ => None,
    };
    (Some(String::from_utf8_lossy(serial).into_owned()), region)
}

/// Decodes a Shift-JIS title field (NUL-terminated) for display: full-width
/// Latin letters and digits become ordinary ones, runs of spaces collapse.
pub(crate) fn decode_title(field: &[u8]) -> Option<String> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    if end == 0 {
        return None;
    }
    let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&field[..end]);
    let mapped: String = text
        .chars()
        .map(|character| match character {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => {
                char::from_u32(character as u32 - 0xFEE0).unwrap_or(character)
            }
            other => other,
        })
        .filter(|character| !character.is_control() && *character != '\u{FFFD}')
        .collect();
    let title = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveExport {
    pub save_name: String,
    pub destination: PathBuf,
    /// `mcs` (PS1) or `psu` (PS2).
    pub format: &'static str,
    pub bytes: u64,
    pub sha256: String,
    pub source_card: PathBuf,
    pub source_card_sha256: String,
}

/// Exports one save from a card to a new single-save file (`.mcs` for PS1,
/// `.psu` for PS2). The card is only read; `destination` must not exist.
pub fn export_save(
    card: &Path,
    save_name: &str,
    destination: &Path,
) -> Result<SaveExport, CardError> {
    let bytes = read_card_bytes(card)?;
    let report = report_bytes(card, &bytes);
    let source_card_sha256 = sha256_hex(&bytes);
    require_writable_state(&report, "exported from")?;
    if report.save(save_name).is_none() {
        return Err(CardError::SaveNotFound(save_name.to_string()));
    }
    match report.format.platform() {
        Some(CardPlatform::PlayStation) => {
            let mcs = ps1::export_mcs(&bytes, save_name)?;
            write_new_file(destination, &mcs)?;
            Ok(SaveExport {
                save_name: save_name.to_string(),
                destination: destination.to_path_buf(),
                format: "mcs",
                bytes: mcs.len() as u64,
                sha256: sha256_hex(&mcs),
                source_card: card.to_path_buf(),
                source_card_sha256,
            })
        }
        Some(CardPlatform::PlayStation2) => {
            ps2::export_psu(card, &bytes, save_name, destination, source_card_sha256)
        }
        None => Err(CardError::NotACard(report.notes.join("; "))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveImport {
    pub save_name: String,
    pub source_card: PathBuf,
    pub source_card_sha256: String,
    pub new_card: PathBuf,
    pub new_card_sha256: String,
    pub new_card_format: CardFormat,
    /// What was checked on the new card before it was kept.
    pub checks: Vec<String>,
}

/// Writes `new_card`: a copy of `card` with the save in `save_file` added.
///
/// `card` is only read, and is re-read afterwards to prove it did not change.
/// `new_card` must not exist. The new card is kept only if both readers
/// read it, every save that was on the card is still there byte for byte,
/// and the imported save reads back identical to the file it came from.
pub fn import_save_into_copy(
    card: &Path,
    save_file: &Path,
    new_card: &Path,
) -> Result<SaveImport, CardError> {
    if new_card == card || new_card == save_file {
        return Err(CardError::Refused(
            "the new card must be a different file from the card and the save".into(),
        ));
    }
    if fs::symlink_metadata(new_card).is_ok() {
        return Err(CardError::DestinationExists(new_card.to_path_buf()));
    }
    let bytes = read_card_bytes(card)?;
    let source_card_sha256 = sha256_hex(&bytes);
    let report = report_bytes(card, &bytes);
    require_writable_state(&report, "imported into a copy of")?;
    let outcome = match report.format {
        CardFormat::Ps1Raw | CardFormat::Ps1DexDrive | CardFormat::Ps1Vgs | CardFormat::Ps1Vmp => {
            let save_bytes = read_card_bytes(save_file)?;
            ps1::import_into_copy(card, &bytes, &save_bytes)?
        }
        CardFormat::Ps2WithEcc | CardFormat::Ps2DataOnly => {
            ps2::import_into_copy(card, &bytes, save_file, new_card)?
        }
        CardFormat::Pcsx2Folder | CardFormat::NotACard => {
            return Err(CardError::Unsupported(
                "saves can only be imported into a PS1 or PS2 card file".into(),
            ));
        }
    };
    write_new_file(new_card, &outcome.image)?;
    let keep = (|| {
        let written = read_card_bytes(new_card)?;
        if written != outcome.image {
            return Err(CardError::VerificationFailed(
                "the new card does not read back as written".into(),
            ));
        }
        let after = read_card_bytes(card)?;
        if sha256_hex(&after) != source_card_sha256 {
            return Err(CardError::VerificationFailed(
                "the original card changed while the copy was being made".into(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = keep {
        let _ = fs::remove_file(new_card);
        return Err(error);
    }
    let mut checks = outcome.checks;
    checks.push("the new card reads back exactly as written".into());
    checks.push("the original card is unchanged (same SHA-256 before and after)".into());
    Ok(SaveImport {
        save_name: outcome.save_name,
        source_card: card.to_path_buf(),
        source_card_sha256,
        new_card: new_card.to_path_buf(),
        new_card_sha256: sha256_hex(&outcome.image),
        new_card_format: outcome.format,
        checks,
    })
}

/// A verified new card image, ready to be written to a new file.
pub(crate) struct ImportOutcome {
    pub image: Vec<u8>,
    pub format: CardFormat,
    pub save_name: String,
    pub checks: Vec<String>,
}

fn require_writable_state(report: &CardReport, action: &str) -> Result<(), CardError> {
    if report.format == CardFormat::NotACard {
        return Err(CardError::NotACard(report.notes.join("; ")));
    }
    if report.format == CardFormat::Pcsx2Folder {
        return Err(CardError::Unsupported(
            "PCSX2 folder cards are listed only; saves cannot be exported or imported".into(),
        ));
    }
    if report.health != CardHealth::Healthy {
        return Err(CardError::Refused(format!(
            "saves are only {action} a card both readers read without warnings: {}",
            report.notes.join("; ")
        )));
    }
    Ok(())
}

/// Creates `path` (which must not exist) with exactly `bytes`, flushed to
/// disk. A file that could not be completed is removed again.
pub(crate) fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), CardError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                CardError::DestinationExists(path.to_path_buf())
            } else {
                io_error("cannot create", path, error)
            }
        })?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if let Err(error) = written {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(io_error("cannot write", path, error));
    }
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}
