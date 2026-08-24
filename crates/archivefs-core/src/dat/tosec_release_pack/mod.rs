//! Inventory, human-friendly classification, selection and registration of
//! user-supplied official TOSEC release packs.
//!
//! A release pack is a directory tree the user selected locally (no network,
//! no extraction): EmuWiz inventories its DAT files read-only, projects each
//! one onto understandable System / Category / Media dimensions, lets the user
//! enable only what they want (nothing is enabled by default), persists those
//! choices across restarts, and registers enabled DATs into the existing
//! [`crate::dat::sources`] registry so they feed the ordinary DAT parser,
//! evidence and audit pipeline.
//!
//! # The raw naming is never thrown away
//!
//! Friendly categories are an *additional projection*. Every entry keeps the
//! original relative path, the original raw catalogue name, and the original
//! raw category segment(s); the advanced view is simply the raw projection.
//! A catalogue whose naming does not confidently match a known category stays
//! in "Everything Else" instead of being guessed.
//!
//! # Safety
//!
//! The pack is untrusted input: discovery is bounded in depth/file counts,
//! symbolic links are never followed (a link cannot smuggle the walk outside
//! the chosen root), only regular `.dat` files are candidates, hashing is
//! size-bounded, there is no shell, no execution, and EmuWiz never writes
//! inside the pack.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::sources::config::{
    DatSourceConfigEntry, DatSourcesConfig, load_dat_sources_config_from,
    save_dat_sources_config_to,
};
use super::sources::{DEFAULT_DAT_PRIORITY, DatSourceKind, DatSourceOwnership};
use crate::ArchiveFsError;
use crate::identity_source::tosec::import_tosec_dat;

/// How deep below the chosen pack root discovery may descend.
const MAX_PACK_WALK_DEPTH: usize = 6;
/// Total directory entries examined before the scan declares itself partial.
const MAX_PACK_ENTRIES_EXAMINED: usize = 100_000;
/// Maximum inventoried DAT candidates; beyond this the scan is partial.
const MAX_PACK_DATS: usize = 2_000;
/// Largest single DAT hashed during inventory. Keep this aligned with the
/// existing default parser ceiling: a candidate too large to parse must not be
/// hashed here and then fail later through an unbounded import hash.
const MAX_INVENTORY_HASH_BYTES: u64 = crate::dat::limits::DEFAULT_MAX_FILE_SIZE;
/// The persisted projection is a convenience view, not an unbounded input
/// channel. It is deliberately much smaller than a release pack itself.
const MAX_PACKS_CONFIG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PERSISTED_PACKS: usize = 256;

/// A friendly top-level category. Deliberately coarse: uncertain catalogues
/// land in [`TosecFriendlyCategory::EverythingElse`] rather than being guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TosecFriendlyCategory {
    Games,
    EducationalProductivity,
    FirmwareSystemSoftware,
    ManualsCoversPrintedMedia,
    MusicAudio,
    DemosScene,
    PreservationVerificationData,
    EverythingElse,
}

impl TosecFriendlyCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Games => "Games",
            Self::EducationalProductivity => "Educational & Productivity Software",
            Self::FirmwareSystemSoftware => "Firmware & System Software",
            Self::ManualsCoversPrintedMedia => "Manuals, Covers & Printed Media",
            Self::MusicAudio => "Music & Audio",
            Self::DemosScene => "Demos & Scene",
            Self::PreservationVerificationData => "Preservation / Verification Data",
            Self::EverythingElse => "Everything Else",
        }
    }
}

/// A media dimension recognised from TOSEC's own naming evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TosecMediaType {
    Tape,
    FloppyDisk,
    HardDisk,
    Cartridge,
    CdOpticalDisc,
    Snapshot,
    Rom,
    Firmware,
    Audio,
    PrintedMedia,
    Other,
}

impl TosecMediaType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tape => "Tape",
            Self::FloppyDisk => "Floppy / Disk",
            Self::HardDisk => "Hard Disk",
            Self::Cartridge => "Cartridge",
            Self::CdOpticalDisc => "CD / Optical Disc",
            Self::Snapshot => "Snapshot",
            Self::Rom => "ROM",
            Self::Firmware => "Firmware",
            Self::Audio => "Audio",
            Self::PrintedMedia => "Printed Media",
            Self::Other => "Other",
        }
    }
}

/// One inventoried DAT inside a release pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TosecPackDat {
    /// The DAT's path relative to the pack root. Never discarded.
    pub relative_path: PathBuf,
    /// The original raw TOSEC catalogue name (filename without extension).
    pub raw_catalogue_name: String,
    /// Raw system projection from the catalogue name's leading segment(s).
    pub system: String,
    pub category: TosecFriendlyCategory,
    pub media: TosecMediaType,
    /// The original raw category segment(s), verbatim - even when they were
    /// not recognised and the friendly category fell back to Everything Else.
    pub raw_category_label: String,
    /// Whether the friendly category came from a recognised keyword rather
    /// than the Everything Else fallback.
    pub classification_confident: bool,
    /// Exact content digest of the DAT file, when it was within the
    /// inventory hashing bound.
    pub content_sha256: Option<String>,
}

impl TosecPackDat {
    /// The selection group this DAT belongs to.
    pub fn selection_key(&self) -> TosecSelectionKey {
        TosecSelectionKey {
            system: self.system.clone(),
            category: self.category,
            media: self.media,
        }
    }
}

/// A SYSTEM + CATEGORY + MEDIA selection group.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TosecSelectionKey {
    pub system: String,
    pub category: TosecFriendlyCategory,
    pub media: TosecMediaType,
}

impl TosecSelectionKey {
    pub fn label(&self) -> String {
        format!(
            "{} / {} / {}",
            self.system,
            self.category.label(),
            self.media.label()
        )
    }
}

/// One skipped walk entry, with the reason it was skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPackEntry {
    pub relative_path: String,
    pub reason: String,
}

/// The result of inventorying one release pack directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TosecPackInventory {
    pub pack_root: PathBuf,
    pub pack_id: String,
    pub dats: Vec<TosecPackDat>,
    pub skipped: Vec<SkippedPackEntry>,
    /// Whether discovery reached the end of every directory. When false the
    /// inventory is partial and must be presented as such.
    pub scan_complete: bool,
}

// ---------------------------------------------------------------------------
// Friendly classification of raw TOSEC catalogue names
// ---------------------------------------------------------------------------

const CATEGORY_KEYWORDS: &[(&[&str], TosecFriendlyCategory)] = &[
    (&["game", "games"], TosecFriendlyCategory::Games),
    (
        &[
            "educational",
            "productivity",
            "application",
            "applications",
            "utility",
            "utilities",
        ],
        TosecFriendlyCategory::EducationalProductivity,
    ),
    (
        &["firmware", "bios", "system", "systems"],
        TosecFriendlyCategory::FirmwareSystemSoftware,
    ),
    (
        &[
            "manual",
            "manuals",
            "cover",
            "covers",
            "printed",
            "magazine",
            "magazines",
            "book",
            "books",
        ],
        TosecFriendlyCategory::ManualsCoversPrintedMedia,
    ),
    (
        &["music", "audio", "sound", "soundtrack"],
        TosecFriendlyCategory::MusicAudio,
    ),
    (
        &["demo", "demos", "scene", "demoscene", "intro", "intros"],
        TosecFriendlyCategory::DemosScene,
    ),
    (
        &["verification", "verify", "preservation"],
        TosecFriendlyCategory::PreservationVerificationData,
    ),
];

const MEDIA_KEYWORDS: &[(&[&str], TosecMediaType)] = &[
    (
        &[
            "tape",
            "tapes",
            "cassette",
            "cassettes",
            "tap",
            "tzx",
            "cas",
        ],
        TosecMediaType::Tape,
    ),
    (
        &[
            "floppy", "floppies", "disk", "disks", "adf", "d88", "imd", "msa", "stx",
        ],
        TosecMediaType::FloppyDisk,
    ),
    (
        &["hard disk", "harddisk", "hdd", "hdf"],
        TosecMediaType::HardDisk,
    ),
    (
        &["cartridge", "cartridges", "cart", "carts"],
        TosecMediaType::Cartridge,
    ),
    (
        &["cd", "cd-rom", "cdrom", "optical", "iso"],
        TosecMediaType::CdOpticalDisc,
    ),
    (
        &["snapshot", "snapshots", "savestate", "savestates"],
        TosecMediaType::Snapshot,
    ),
    (&["rom", "roms"], TosecMediaType::Rom),
    (&["firmware", "bios"], TosecMediaType::Firmware),
    (&["audio"], TosecMediaType::Audio),
    (
        &[
            "manual", "manuals", "cover", "covers", "printed", "magazine",
        ],
        TosecMediaType::PrintedMedia,
    ),
];

fn keyword_match(segment: &str, table: &[(&[&str], TosecMediaType)]) -> Option<TosecMediaType> {
    let lowered = segment.to_ascii_lowercase();
    table
        .iter()
        .find(|(keywords, _)| keywords.iter().any(|keyword| lowered == *keyword))
        .map(|(_, media)| *media)
}

fn category_match(segment: &str) -> Option<TosecFriendlyCategory> {
    let lowered = segment.to_ascii_lowercase();
    CATEGORY_KEYWORDS
        .iter()
        .find(|(keywords, _)| keywords.iter().any(|keyword| lowered == *keyword))
        .map(|(_, category)| *category)
}

/// Strips a trailing `(TOSEC ...)` version parenthetical from a raw catalogue
/// name. Everything stripped is retained verbatim by the caller in
/// [`TosecPackDat::raw_catalogue_name`]; this only cleans the classification
/// input.
fn strip_tosec_version_marker(name: &str) -> String {
    let trimmed = name.trim();
    match trimmed.rfind("(") {
        Some(open) if trimmed[open..].to_ascii_lowercase().contains("tosec") => {
            trimmed[..open].trim_end().to_string()
        }
        _ => trimmed.to_string(),
    }
}

/// Projects one raw TOSEC catalogue name onto (system, category, media).
///
/// The classic TOSEC packaging convention is `System - Category - Media`, so
/// the projection finds the last segment matching a known category keyword;
/// segments before it are the system, segments after it carry the media.
/// Anything unrecognised is classified as Everything Else with the raw text
/// preserved - never guessed.
pub fn classify_tosec_catalogue_name(raw_catalogue_name: &str) -> TosecClassification {
    let cleaned = strip_tosec_version_marker(raw_catalogue_name);
    let segments: Vec<&str> = cleaned.split(" - ").map(str::trim).collect();

    let category_index = segments
        .iter()
        .rposition(|segment| category_match(segment).is_some());

    let Some(index) = category_index else {
        // No recognised category anywhere: uncertain, keep everything raw.
        return TosecClassification {
            system: segments.first().copied().unwrap_or("").to_string(),
            category: TosecFriendlyCategory::EverythingElse,
            media: TosecMediaType::Other,
            raw_category_label: segments
                .get(1..)
                .map(|rest| rest.join(" - "))
                .unwrap_or_else(|| cleaned.clone()),
            confident: false,
        };
    };

    let system = segments[..index].join(" - ");
    let raw_category_label = segments[index].to_string();
    let category = category_match(segments[index]).unwrap_or(TosecFriendlyCategory::EverythingElse);
    // Media normally comes from a segment after the category ("Games - Tape").
    // When there is none, the category token itself may carry media meaning
    // ("... - Firmware"), which is then used directly.
    let media = segments
        .get((index + 1)..)
        .and_then(|rest| keyword_match(&rest.join(" - "), MEDIA_KEYWORDS))
        .or_else(|| keyword_match(segments[index], MEDIA_KEYWORDS))
        .unwrap_or(TosecMediaType::Other);

    TosecClassification {
        system,
        category,
        media,
        raw_category_label,
        confident: true,
    }
}

/// The friendly projection of one raw TOSEC catalogue name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TosecClassification {
    pub system: String,
    pub category: TosecFriendlyCategory,
    pub media: TosecMediaType,
    pub raw_category_label: String,
    pub confident: bool,
}

// ---------------------------------------------------------------------------
// Bounded, read-only pack discovery
// ---------------------------------------------------------------------------

/// Why a path could not be treated as a release-pack root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TosecPackError {
    NotADirectory(PathBuf),
    Unreadable(String),
}

impl std::fmt::Display for TosecPackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotADirectory(path) => write!(f, "{} is not a directory", path.display()),
            Self::Unreadable(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for TosecPackError {}

fn sha256_file_bounded(path: &Path) -> std::io::Result<Option<String>> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_INVENTORY_HASH_BYTES {
        return Ok(None);
    }
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    ))
}

/// A deterministic local identity for one pack: its folder name plus a short
/// digest of the canonical root path. Purely a label; never authority.
fn pack_identity(canonical_root: &Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(canonical_root.to_string_lossy().as_bytes());
    let short: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
    let leaf = canonical_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "pack".to_string());
    format!("{leaf}-{short}")
}

struct WalkState {
    dats: Vec<TosecPackDat>,
    skipped: Vec<SkippedPackEntry>,
    examined: usize,
    complete: bool,
}

impl WalkState {
    fn push_skipped(&mut self, relative: String, reason: &str) {
        if self.skipped.len() < MAX_PACK_DATS * 4 {
            self.skipped.push(SkippedPackEntry {
                relative_path: relative,
                reason: reason.to_string(),
            });
        }
    }
}

fn walk_pack_directory(
    root: &Path,
    directory: &Path,
    depth: usize,
    state: &mut WalkState,
) -> Result<(), String> {
    if depth > MAX_PACK_WALK_DEPTH {
        state.complete = false;
        return Ok(());
    }
    let read_dir = std::fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in read_dir {
        state.examined += 1;
        if state.examined > MAX_PACK_ENTRIES_EXAMINED || state.dats.len() >= MAX_PACK_DATS {
            state.complete = false;
            return Ok(());
        }
        let Ok(entry) = entry else {
            continue;
        };
        // DirEntry::file_type does not follow symlinks: a link is recorded and
        // never traversed, so nothing outside the chosen root can be reached.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let relative = entry
            .path()
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| entry.path());
        let relative_display = relative.to_string_lossy().into_owned();
        if file_type.is_symlink() {
            state.push_skipped(relative_display, "symbolic link; never followed");
            continue;
        }
        if file_type.is_dir() {
            walk_pack_directory(root, &entry.path(), depth + 1, state)?;
            continue;
        }
        if !file_type.is_file() {
            state.push_skipped(relative_display, "not a regular file");
            continue;
        }
        let is_dat = entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dat"));
        if !is_dat {
            // Non-DAT junk is simply not part of the inventory; it is not an
            // error and not individually listed (a pack contains many extras).
            continue;
        }
        let raw_catalogue_name = entry
            .path()
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let classification = classify_tosec_catalogue_name(&raw_catalogue_name);
        let content_sha256 = match sha256_file_bounded(&entry.path()) {
            Ok(digest) => digest,
            Err(error) => {
                state.push_skipped(relative_display, &format!("could not be read: {error}"));
                continue;
            }
        };
        state.dats.push(TosecPackDat {
            relative_path: relative,
            raw_catalogue_name,
            system: classification.system,
            category: classification.category,
            media: classification.media,
            raw_category_label: classification.raw_category_label,
            classification_confident: classification.confident,
            content_sha256,
        });
    }
    Ok(())
}

/// Inventories a user-selected extracted TOSEC release-pack directory.
/// Strictly read-only; the pack is never modified.
pub fn inventory_release_pack(root: &Path) -> Result<TosecPackInventory, TosecPackError> {
    if !root.is_dir() {
        return Err(TosecPackError::NotADirectory(root.to_path_buf()));
    }
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| TosecPackError::Unreadable(format!("{}: {error}", root.display())))?;
    let mut state = WalkState {
        dats: Vec::new(),
        skipped: Vec::new(),
        examined: 0,
        complete: true,
    };
    walk_pack_directory(&canonical_root, &canonical_root, 0, &mut state)
        .map_err(TosecPackError::Unreadable)?;
    state
        .dats
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(TosecPackInventory {
        pack_id: pack_identity(&canonical_root),
        pack_root: canonical_root,
        dats: state.dats,
        skipped: state.skipped,
        scan_complete: state.complete,
    })
}

// ---------------------------------------------------------------------------
// Persistence: imported packs, inventory projection and user selections
// ---------------------------------------------------------------------------

/// Leaf name of the TOSEC release-pack registry inside the EmuWiz config dir.
pub const TOSEC_PACKS_FILE: &str = "tosec_release_packs.json";

/// The default persistence path for imported packs and their selections.
pub fn default_tosec_packs_path() -> Result<PathBuf, ArchiveFsError> {
    Ok(crate::app_dirs::config_dir()?.join(TOSEC_PACKS_FILE))
}

/// One imported pack as persisted across restarts. Selections default to an
/// empty set: importing a pack never enables anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedTosecPack {
    pub pack_id: String,
    pub root_path: PathBuf,
    pub imported_unix_seconds: u64,
    /// The user's enabled SYSTEM+CATEGORY+MEDIA groups. Empty by default.
    pub selections: BTreeSet<TosecSelectionKey>,
    /// The inventory projection needed to reopen the selection view without a
    /// rescan (raw names and relative paths are preserved verbatim).
    pub dats: Vec<TosecPackDat>,
}

/// Whether the underlying pack folder still exists. A missing pack is
/// reported honestly; the configuration is never silently deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackAvailability {
    Available,
    Missing,
}

impl PersistedTosecPack {
    pub fn availability(&self) -> PackAvailability {
        if self.root_path.is_dir() {
            PackAvailability::Available
        } else {
            PackAvailability::Missing
        }
    }

    /// The DATs whose selection group the user has enabled.
    pub fn selected_dats(&self) -> impl Iterator<Item = &TosecPackDat> {
        self.dats
            .iter()
            .filter(|dat| self.selections.contains(&dat.selection_key()))
    }
}

fn is_normal_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn validate_persisted_pack(pack: &PersistedTosecPack) -> Result<(), ArchiveFsError> {
    if pack.pack_id.is_empty() || pack.pack_id.len() > 512 {
        return Err(ArchiveFsError::Config(
            "TOSEC release pack has an invalid pack ID".to_string(),
        ));
    }
    if !pack.root_path.is_absolute()
        || !pack
            .root_path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(ArchiveFsError::Config(format!(
            "TOSEC release pack root must be a normalized absolute path: {}",
            pack.root_path.display()
        )));
    }
    if pack.dats.len() > MAX_PACK_DATS {
        return Err(ArchiveFsError::Config(format!(
            "TOSEC release pack has too many persisted DATs (limit {MAX_PACK_DATS})"
        )));
    }
    let mut relative_paths = BTreeSet::new();
    for dat in &pack.dats {
        if !is_normal_relative_path(&dat.relative_path)
            || !dat
                .relative_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dat"))
        {
            return Err(ArchiveFsError::Config(format!(
                "TOSEC release pack has an unsafe DAT path: {}",
                dat.relative_path.display()
            )));
        }
        if !relative_paths.insert(dat.relative_path.clone()) {
            return Err(ArchiveFsError::Config(format!(
                "TOSEC release pack contains the same DAT path more than once: {}",
                dat.relative_path.display()
            )));
        }
    }
    Ok(())
}

/// Loads every persisted pack. A missing file means no imported packs yet;
/// malformed or unsafe persisted state is reported rather than silently
/// discarded, because losing selections would hide a real configuration issue.
pub fn load_tosec_packs(path: &Path) -> Result<Vec<PersistedTosecPack>, ArchiveFsError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ArchiveFsError::io(path.to_path_buf(), error)),
    };
    if metadata.len() > MAX_PACKS_CONFIG_BYTES {
        return Err(ArchiveFsError::Config(format!(
            "TOSEC release-pack config exceeds {MAX_PACKS_CONFIG_BYTES} bytes: {}",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
    let packs: Vec<PersistedTosecPack> = serde_json::from_str(&text).map_err(|error| {
        ArchiveFsError::Config(format!(
            "failed to parse TOSEC release-pack config {}: {error}",
            path.display()
        ))
    })?;
    if packs.len() > MAX_PERSISTED_PACKS {
        return Err(ArchiveFsError::Config(format!(
            "TOSEC release-pack config has too many packs (limit {MAX_PERSISTED_PACKS})"
        )));
    }
    for pack in &packs {
        validate_persisted_pack(pack)?;
    }
    Ok(packs)
}

/// Durably persists the pack registry.
pub fn save_tosec_packs(path: &Path, packs: &[PersistedTosecPack]) -> Result<(), ArchiveFsError> {
    if packs.len() > MAX_PERSISTED_PACKS {
        return Err(ArchiveFsError::Config(format!(
            "TOSEC release-pack config has too many packs (limit {MAX_PERSISTED_PACKS})"
        )));
    }
    for pack in packs {
        validate_persisted_pack(pack)?;
    }
    let text = serde_json::to_string_pretty(packs).map_err(|error| {
        ArchiveFsError::Config(format!("could not serialise TOSEC packs: {error}"))
    })?;
    if text.len() as u64 > MAX_PACKS_CONFIG_BYTES {
        return Err(ArchiveFsError::Config(format!(
            "TOSEC release-pack config exceeds {MAX_PACKS_CONFIG_BYTES} bytes"
        )));
    }
    crate::atomic_write_text(path, &text)
}

// ---------------------------------------------------------------------------
// Registration into the existing DAT source registry
// ---------------------------------------------------------------------------

/// Exact provenance retained for one registered TOSEC DAT. Answers "why does
/// this DAT apply?" without flattening TOSEC into an anonymous XML file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TosecDatProvenance {
    pub pack_id: String,
    pub relative_path: PathBuf,
    pub content_sha256: Option<String>,
    /// The authoritative system name parsed from the DAT header.
    pub tosec_header_name: String,
    pub tosec_version: Option<String>,
}

impl TosecDatProvenance {
    pub fn summary(&self) -> String {
        format!(
            "TOSEC release pack {} ({}); relative path {}; sha256 {}; header '{}' version {}",
            self.pack_id,
            "user-supplied local pack",
            self.relative_path.display(),
            self.content_sha256.as_deref().unwrap_or("not hashed"),
            self.tosec_header_name,
            self.tosec_version.as_deref().unwrap_or("unknown"),
        )
    }
}

/// One successfully registered selected DAT.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredTosecDat {
    pub entry: DatSourceConfigEntry,
    pub provenance: TosecDatProvenance,
}

/// The outcome of applying a pack selection to the DAT source registry.
/// Every selected DAT is reported individually; nothing is silently dropped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TosecRegistrationOutcome {
    pub registered: Vec<RegisteredTosecDat>,
    /// Entries this exact pack previously created but whose group is no
    /// longer selected. User-local and other-provider entries never appear
    /// here.
    pub removed: Vec<DatSourceConfigEntry>,
    pub failed: Vec<(PathBuf, String)>,
}

fn registration_id(pack: &PersistedTosecPack, dat: &TosecPackDat) -> String {
    use sha2::{Digest, Sha256};

    // A registry identity belongs to this pack location and DAT path, rather
    // than to the inventory-time digest. The latter can legitimately change
    // before registration; using it here would retain a stale source entry
    // instead of replacing it with the freshly validated artifact.
    let mut hasher = Sha256::new();
    hasher.update(pack.pack_id.as_bytes());
    hasher.update([0]);
    hasher.update(dat.relative_path.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let short: String = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("tosec-pack-{short}")
}

fn resolve_selected_dat_path(
    pack: &PersistedTosecPack,
    dat: &TosecPackDat,
) -> Result<PathBuf, String> {
    if !is_normal_relative_path(&dat.relative_path)
        || !dat
            .relative_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dat"))
    {
        return Err("the persisted DAT path is not a safe relative path".to_string());
    }
    let root_metadata = std::fs::symlink_metadata(&pack.root_path)
        .map_err(|error| format!("cannot inspect release-pack root: {error}"))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("the release-pack root is no longer a real directory".to_string());
    }
    let canonical_root = std::fs::canonicalize(&pack.root_path)
        .map_err(|error| format!("cannot resolve release-pack root: {error}"))?;
    if canonical_root != pack.root_path {
        return Err("the release-pack root changed; rescan it before registering DATs".to_string());
    }

    let mut current = canonical_root.clone();
    for component in dat.relative_path.components() {
        let Component::Normal(component) = component else {
            return Err("the persisted DAT path is not a safe relative path".to_string());
        };
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|error| format!("cannot inspect selected DAT path: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("selected DAT path contains a symbolic link".to_string());
        }
    }
    let metadata = std::fs::symlink_metadata(&current)
        .map_err(|error| format!("cannot inspect selected DAT: {error}"))?;
    if !metadata.is_file() {
        return Err("selected DAT is no longer a regular file".to_string());
    }
    let parser_limit = crate::dat::limits::DatLimits::default().max_file_size;
    if metadata.len() > parser_limit {
        return Err(format!(
            "selected DAT exceeds the bounded parser/hash limit of {parser_limit} bytes"
        ));
    }
    let canonical_dat = std::fs::canonicalize(&current)
        .map_err(|error| format!("cannot resolve selected DAT: {error}"))?;
    if !canonical_dat.starts_with(&canonical_root) {
        return Err("selected DAT escapes the release-pack root".to_string());
    }
    Ok(canonical_dat)
}

/// Registers every selected DAT of one pack into `sources`, validating each
/// through the existing classic-TOSEC import path (bounded parser, internal
/// ecosystem gate, artifact digest, collision-preserving index). Re-registering
/// replaces this pack's earlier entry for the same DAT; entries from other
/// sources are never touched. The pack itself is only ever read.
pub fn register_selected_tosec_dats(
    pack: &PersistedTosecPack,
    sources: &mut DatSourcesConfig,
    now_unix_seconds: u64,
) -> TosecRegistrationOutcome {
    let mut outcome = TosecRegistrationOutcome::default();
    if pack.availability() == PackAvailability::Missing {
        // Honest failure: the folder disappeared. Nothing is registered and
        // nothing is deleted from the configuration either.
        for dat in pack.selected_dats() {
            outcome.failed.push((
                dat.relative_path.clone(),
                "the release pack folder is no longer available".to_string(),
            ));
        }
        return outcome;
    }
    let selected_paths: BTreeSet<PathBuf> = pack
        .selected_dats()
        .map(|dat| dat.relative_path.clone())
        .collect();
    for dat in pack.selected_dats() {
        let absolute = match resolve_selected_dat_path(pack, dat) {
            Ok(path) => path,
            Err(error) => {
                outcome.failed.push((dat.relative_path.clone(), error));
                continue;
            }
        };
        match import_tosec_dat(&absolute) {
            Ok(imported) => {
                let provenance = TosecDatProvenance {
                    pack_id: pack.pack_id.clone(),
                    relative_path: dat.relative_path.clone(),
                    content_sha256: Some(imported.artifact_sha256.clone()),
                    tosec_header_name: imported.system_name.clone(),
                    tosec_version: imported.upstream_version.clone(),
                };
                let id = registration_id(pack, dat);
                let absolute_text = absolute.to_string_lossy().into_owned();
                let ownership = DatSourceOwnership::ImportedTosecReleasePack {
                    pack_id: pack.pack_id.clone(),
                    relative_path: dat.relative_path.clone(),
                };
                if sources.sources.iter().flatten().any(|existing| {
                    existing.id == id
                        && (existing.path != absolute_text || existing.ownership != ownership)
                }) {
                    outcome.failed.push((
                        dat.relative_path.clone(),
                        "TOSEC registration ID conflicts with a different existing local DAT source"
                            .to_string(),
                    ));
                    continue;
                }
                let entry = DatSourceConfigEntry {
                    id: id.clone(),
                    display_name: dat.raw_catalogue_name.clone(),
                    path: absolute_text,
                    kind: DatSourceKind::File,
                    ownership,
                    enabled: Some(true),
                    priority: Some(DEFAULT_DAT_PRIORITY),
                    platform: None,
                    origin: Some(provenance.summary()),
                    added_unix_seconds: Some(now_unix_seconds),
                    health_state: None,
                    health_last_validated_unix_seconds: None,
                    health_detail: None,
                    health_entry_count: None,
                    health_rom_count: None,
                    health_file_count: None,
                    health_formats: None,
                    health_observed_size_bytes: None,
                    health_observed_modified_unix_seconds: None,
                    unknown_fields: toml::Table::new(),
                };
                let sources_list = sources.sources.get_or_insert_with(Vec::new);
                sources_list.retain(|existing| existing.id != id);
                sources_list.push(entry.clone());
                outcome
                    .registered
                    .push(RegisteredTosecDat { entry, provenance });
            }
            Err(error) => {
                outcome
                    .failed
                    .push((dat.relative_path.clone(), error.to_string()));
            }
        }
    }
    if let Some(sources_list) = sources.sources.as_mut() {
        sources_list.retain(|entry| {
            let Some((pack_id, relative_path)) = entry.ownership.imported_tosec_release_pack()
            else {
                return true;
            };
            if pack_id != pack.pack_id || selected_paths.contains(relative_path) {
                return true;
            }
            outcome.removed.push(entry.clone());
            false
        });
    }
    outcome
}

/// Convenience: loads the on-disk DAT source registry, applies one pack's
/// selection, saves it back durably, and returns the outcome.
pub fn apply_selection_to_registry(
    pack: &PersistedTosecPack,
    registry_path: &Path,
    now_unix_seconds: u64,
) -> Result<TosecRegistrationOutcome, ArchiveFsError> {
    let mut sources = load_dat_sources_config_from(registry_path)?;
    let outcome = register_selected_tosec_dats(pack, &mut sources, now_unix_seconds);
    save_dat_sources_config_to(registry_path, &sources)?;
    Ok(outcome)
}

#[cfg(test)]
mod tests;
