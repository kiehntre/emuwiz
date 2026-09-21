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

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::sources::config::{
    DatSourceConfigEntry, DatSourcesConfig, load_dat_sources_config_from,
    save_dat_sources_config_to,
};
use super::sources::{DEFAULT_DAT_PRIORITY, DatSourceKind, DatSourceOwnership};
use crate::ArchiveFsError;
use crate::identity_source::managed_snapshot::{
    ActivationResult, ManagedSourceDescriptor, ManagedSourceKind, ManagedSourceMetadata,
    ManagedSourceReference, ManagedSourceStore, ManagedSourceTrust, ValidatedCandidate,
    VerificationFreshness,
};
use crate::identity_source::tosec::import_tosec_dat;

/// The aggregate bound for one immutable TOSEC snapshot. Individual DATs are
/// already bounded by the parser's 256 MiB limit; the smaller aggregate bound
/// prevents a selected release from becoming an accidental second full pack.
pub const MAX_TOSEC_SNAPSHOT_DAT_BYTES: u64 = 512 * 1024 * 1024;
/// Official TOSEC release index for the browser-assisted acquisition flow.
/// No request is made by the core pack/lifecycle code.
pub const TOSEC_OFFICIAL_DOWNLOADS_PAGE: &str = "https://tosecdev.org/downloads";
const MAX_TOSEC_SNAPSHOT_MEMBERS: usize = MAX_PACK_DATS;
const TOSEC_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const TOSEC_MANAGED_PROVIDER_ID: &str = "tosec-release-pack";
const TOSEC_MANAGED_MEDIA_TYPE: &str = "application/vnd.emuwiz.tosec-snapshot";
const TOSEC_MANAGED_PARSER_SCHEMA: &str = "tosec-managed-snapshot-v1";
const TOSEC_MANAGED_SOURCE_SENTINEL: &str = "/emuwiz/tosec-local-release-pack";

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
// Immutable managed snapshots
// ---------------------------------------------------------------------------

/// Metadata retained for one DAT in a managed snapshot. The DAT bytes are
/// stored beside this record in the immutable object; this manifest is the
/// inspectable, self-contained catalogue projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TosecManagedDat {
    pub relative_path: PathBuf,
    pub raw_catalogue_name: String,
    pub system: String,
    pub category: TosecFriendlyCategory,
    pub media: TosecMediaType,
    pub raw_category_label: String,
    pub classification_confident: bool,
    pub content_sha256: String,
    pub tosec_header_name: String,
    pub tosec_version: Option<String>,
    pub entry_count: usize,
    pub rom_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TosecSnapshotManifest {
    schema_version: u32,
    pack_id: String,
    release_version: Option<String>,
    selected_groups: BTreeSet<TosecSelectionKey>,
    imported_unix_seconds: u64,
    source_path: PathBuf,
    dats: Vec<TosecManagedDat>,
}

/// A decoded active TOSEC snapshot. `dats` contains the actual DAT bytes, so
/// callers can continue offline after the original extracted directory is
/// removed. No executable content is represented by this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TosecManagedSnapshot {
    pub pack_id: String,
    pub release_version: Option<String>,
    pub selected_groups: BTreeSet<TosecSelectionKey>,
    pub imported_unix_seconds: u64,
    pub source_path: PathBuf,
    pub dats: Vec<(TosecManagedDat, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TosecSnapshotPreview {
    pub release_identifier: String,
    pub release_version: Option<String>,
    pub dat_count: usize,
    pub selected_group_count: usize,
    pub total_snapshot_bytes: u64,
    pub categories: BTreeSet<TosecFriendlyCategory>,
    pub media: BTreeSet<TosecMediaType>,
    pub current_active_release: Option<String>,
    pub differs_from_active: bool,
    pub revision_comparison: TosecRevisionComparison,
    pub validation_warnings: Vec<String>,
}

/// Conservative comparison of a staged TOSEC release against the active
/// managed release. Unknown means the release labels are absent or not in the
/// documented date-shaped form; content hashes establish identity only, not
/// ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TosecRevisionComparison {
    NoActiveRelease,
    Newer,
    Same,
    Older,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TosecManagedCandidate {
    pub validated: ValidatedCandidate,
    pub snapshot: TosecManagedSnapshot,
}

/// Adapter over the provider-neutral managed snapshot spine. The source
/// sentinel is deliberately not the user's pack path: that path is provenance
/// in the manifest only, and activation therefore never depends on it.
#[derive(Debug, Clone)]
pub struct TosecManagedSnapshotStore {
    root: PathBuf,
    store: ManagedSourceStore,
}

impl TosecManagedSnapshotStore {
    pub fn new(root: PathBuf) -> Result<Self, ArchiveFsError> {
        let descriptor = ManagedSourceDescriptor {
            provider_id: TOSEC_MANAGED_PROVIDER_ID.to_string(),
            display_name: "TOSEC local release snapshots".to_string(),
            source_kind: ManagedSourceKind::Local,
            source: ManagedSourceReference::LocalPath(PathBuf::from(TOSEC_MANAGED_SOURCE_SENTINEL)),
            expected_media_type: TOSEC_MANAGED_MEDIA_TYPE.to_string(),
            maximum_size_bytes: MAX_TOSEC_SNAPSHOT_DAT_BYTES,
            attribution_url: None,
            parser_schema_version: TOSEC_MANAGED_PARSER_SCHEMA.to_string(),
            trust: ManagedSourceTrust::UserProvided,
        };
        Ok(Self {
            root: root.clone(),
            store: ManagedSourceStore::new(root, descriptor)?,
        })
    }

    pub fn store(&self) -> &ManagedSourceStore {
        &self.store
    }

    /// Read, validate, hash and stage selected DATs. This never activates the
    /// candidate and never writes into the source release directory.
    pub fn stage_release_pack(
        &self,
        inventory: &TosecPackInventory,
        selections: &BTreeSet<TosecSelectionKey>,
        imported_unix_seconds: u64,
    ) -> Result<TosecManagedCandidate, ArchiveFsError> {
        if !inventory.scan_complete {
            return Err(ArchiveFsError::Config(
                "cannot stage a partial TOSEC release-pack inventory".to_string(),
            ));
        }
        if selections.is_empty() {
            return Err(ArchiveFsError::Config(
                "cannot stage a TOSEC snapshot without selected groups".to_string(),
            ));
        }
        let selected: Vec<_> = inventory
            .dats
            .iter()
            .filter(|dat| selections.contains(&dat.selection_key()))
            .collect();
        if selected.is_empty() {
            return Err(ArchiveFsError::Config(
                "TOSEC selections do not match any inventoried DAT".to_string(),
            ));
        }
        if selected.len() > MAX_TOSEC_SNAPSHOT_MEMBERS {
            return Err(ArchiveFsError::Config(
                "TOSEC snapshot contains too many selected DATs".to_string(),
            ));
        }

        let mut seen_names = BTreeMap::<String, String>::new();
        let mut total_bytes = 0_u64;
        let mut dats = Vec::with_capacity(selected.len());
        for dat in selected {
            let path = resolve_inventory_dat_path(&inventory.pack_root, &dat.relative_path)?;
            let imported = import_tosec_dat(&path).map_err(|error| {
                ArchiveFsError::Config(format!(
                    "selected TOSEC DAT {} failed validation: {error}",
                    dat.relative_path.display()
                ))
            })?;
            if dat.content_sha256.as_deref() != Some(imported.artifact_sha256.as_str()) {
                return Err(ArchiveFsError::Config(format!(
                    "selected TOSEC DAT {} changed since inventory; rescan before staging",
                    dat.relative_path.display()
                )));
            }
            if let Some(previous_hash) = seen_names.insert(
                dat.raw_catalogue_name.clone(),
                imported.artifact_sha256.clone(),
            ) && previous_hash != imported.artifact_sha256
            {
                return Err(ArchiveFsError::Config(format!(
                    "conflicting duplicate TOSEC catalogue name: {}",
                    dat.raw_catalogue_name
                )));
            }
            let bytes =
                std::fs::read(&path).map_err(|error| ArchiveFsError::io(path.clone(), error))?;
            total_bytes = total_bytes.checked_add(bytes.len() as u64).ok_or_else(|| {
                ArchiveFsError::Config("TOSEC snapshot size overflow".to_string())
            })?;
            if total_bytes > MAX_TOSEC_SNAPSHOT_DAT_BYTES {
                return Err(ArchiveFsError::Config(format!(
                    "TOSEC snapshot exceeds {} byte aggregate DAT limit",
                    MAX_TOSEC_SNAPSHOT_DAT_BYTES
                )));
            }
            dats.push((
                TosecManagedDat {
                    relative_path: dat.relative_path.clone(),
                    raw_catalogue_name: dat.raw_catalogue_name.clone(),
                    system: dat.system.clone(),
                    category: dat.category,
                    media: dat.media,
                    raw_category_label: dat.raw_category_label.clone(),
                    classification_confident: dat.classification_confident,
                    content_sha256: imported.artifact_sha256,
                    tosec_header_name: imported.system_name,
                    tosec_version: imported.upstream_version,
                    entry_count: imported.entry_count,
                    rom_count: imported.rom_count,
                },
                bytes,
            ));
        }
        dats.sort_by(|left, right| left.0.relative_path.cmp(&right.0.relative_path));
        let release_version = common_release_version(&dats);
        let manifest = TosecSnapshotManifest {
            schema_version: TOSEC_SNAPSHOT_SCHEMA_VERSION,
            pack_id: inventory.pack_id.clone(),
            release_version: release_version.clone(),
            selected_groups: selections.clone(),
            imported_unix_seconds,
            source_path: inventory.pack_root.clone(),
            dats: dats.iter().map(|(metadata, _)| metadata.clone()).collect(),
        };
        let bytes = encode_snapshot(&manifest, &dats)?;
        let staged = self.store.stage_bytes(
            &bytes,
            ManagedSourceMetadata {
                provider_version: release_version,
                content_length: Some(bytes.len() as u64),
                ..ManagedSourceMetadata::default()
            },
        )?;
        let validated = self.store.validate_candidate(
            staged,
            crate::identity_source::managed_snapshot::ValidationReport {
                valid: true,
                summary: format!("validated {} selected TOSEC DAT files", dats.len()),
                record_count: Some(
                    dats.iter()
                        .map(|(metadata, _)| metadata.entry_count as u64)
                        .sum(),
                ),
                warnings: Vec::new(),
            },
        )?;
        Ok(TosecManagedCandidate {
            validated,
            snapshot: TosecManagedSnapshot {
                pack_id: manifest.pack_id,
                release_version: manifest.release_version,
                selected_groups: manifest.selected_groups,
                imported_unix_seconds: manifest.imported_unix_seconds,
                source_path: manifest.source_path,
                dats,
            },
        })
    }

    pub fn preview_activation(
        &self,
        candidate: &TosecManagedCandidate,
    ) -> Result<TosecSnapshotPreview, ArchiveFsError> {
        let preview = self.store.preview_activation(&candidate.validated)?;
        let categories = candidate
            .snapshot
            .dats
            .iter()
            .map(|(dat, _)| dat.category)
            .collect();
        let media = candidate
            .snapshot
            .dats
            .iter()
            .map(|(dat, _)| dat.media)
            .collect();
        let active = self.active_snapshot()?;
        let current_active_release = active
            .as_ref()
            .and_then(|snapshot| snapshot.release_version.clone());
        let revision_comparison = compare_tosec_release_versions(
            candidate.snapshot.release_version.as_deref(),
            current_active_release.as_deref(),
        );
        Ok(TosecSnapshotPreview {
            release_identifier: candidate.snapshot.pack_id.clone(),
            release_version: candidate.snapshot.release_version.clone(),
            dat_count: candidate.snapshot.dats.len(),
            selected_group_count: candidate.snapshot.selected_groups.len(),
            total_snapshot_bytes: candidate.validated.snapshot.size_bytes,
            categories,
            media,
            current_active_release,
            differs_from_active: preview.changed,
            revision_comparison,
            validation_warnings: preview.warnings,
        })
    }

    pub fn activate(
        &self,
        candidate: &TosecManagedCandidate,
        expected_active: Option<&str>,
    ) -> Result<ActivationResult, ArchiveFsError> {
        self.store
            .activate_snapshot(&candidate.validated, expected_active)
    }

    pub fn rollback(&self, hash: &str) -> Result<ActivationResult, ArchiveFsError> {
        self.store.rollback_snapshot(hash)
    }

    pub fn active_snapshot(&self) -> Result<Option<TosecManagedSnapshot>, ArchiveFsError> {
        let Some(bytes) = self.store.active_snapshot_bytes()? else {
            return Ok(None);
        };
        decode_snapshot(&bytes).map(Some)
    }

    pub fn active_record(
        &self,
    ) -> Result<
        Option<crate::identity_source::managed_snapshot::ManagedSourceSnapshot>,
        ArchiveFsError,
    > {
        self.store.active_snapshot()
    }

    /// Materializes the active snapshot's DAT members under the managed
    /// storage root so the ordinary read-only DAT verifier can consume them
    /// without reaching back into the user's original release directory.
    /// Existing materialized files are hash-checked and never overwritten.
    pub fn materialize_active_dat_sources(
        &self,
    ) -> Result<Vec<(TosecManagedDat, PathBuf)>, ArchiveFsError> {
        let record = self
            .store
            .active_snapshot()?
            .ok_or_else(|| ArchiveFsError::Config("no active TOSEC snapshot".to_string()))?;
        let snapshot = decode_snapshot(&self.store.snapshot_bytes(&record)?)?;
        let root = self.root.join("materialized").join(&record.sha256);
        std::fs::create_dir_all(&root).map_err(|error| ArchiveFsError::io(root.clone(), error))?;
        reject_materialized_symlinks(&root, &root)?;
        let mut paths = Vec::with_capacity(snapshot.dats.len());
        for (metadata, bytes) in snapshot.dats {
            if !is_normal_relative_path(&metadata.relative_path) {
                return Err(ArchiveFsError::Config(format!(
                    "unsafe TOSEC materialized DAT path: {}",
                    metadata.relative_path.display()
                )));
            }
            let path = root.join(&metadata.relative_path);
            let parent = path
                .parent()
                .ok_or_else(|| ArchiveFsError::Config("TOSEC DAT has no parent".to_string()))?;
            std::fs::create_dir_all(parent)
                .map_err(|error| ArchiveFsError::io(parent.to_path_buf(), error))?;
            reject_materialized_symlinks(&root, parent)?;
            if let Ok(existing) = std::fs::symlink_metadata(&path) {
                if existing.file_type().is_symlink() || !existing.is_file() {
                    return Err(ArchiveFsError::Config(format!(
                        "TOSEC materialized DAT is not a regular file: {}",
                        path.display()
                    )));
                }
                let existing_bytes = std::fs::read(&path)
                    .map_err(|error| ArchiveFsError::io(path.clone(), error))?;
                if sha256_bytes(&existing_bytes) != metadata.content_sha256 {
                    return Err(ArchiveFsError::Config(format!(
                        "TOSEC materialized DAT hash mismatch: {}",
                        path.display()
                    )));
                }
            } else {
                let temporary = tempfile::NamedTempFile::new_in(parent)
                    .map_err(|error| ArchiveFsError::io(parent.to_path_buf(), error))?;
                let temporary_path = temporary.path().to_path_buf();
                let mut file = temporary
                    .reopen()
                    .map_err(|error| ArchiveFsError::io(temporary_path.clone(), error))?;
                file.write_all(&bytes)
                    .map_err(|error| ArchiveFsError::io(temporary_path.clone(), error))?;
                file.sync_all()
                    .map_err(|error| ArchiveFsError::io(temporary_path.clone(), error))?;
                std::fs::rename(&temporary_path, &path)
                    .map_err(|error| ArchiveFsError::io(path.clone(), error))?;
            }
            paths.push((metadata, path));
        }
        Ok(paths)
    }

    /// Registers the active immutable snapshot's materialized DAT members in
    /// the ordinary local DAT registry. The original release-pack path is
    /// retained only in snapshot provenance and is never registered here.
    pub fn register_active_snapshot_to_registry(
        &self,
        registry_path: &Path,
        now_unix_seconds: u64,
    ) -> Result<TosecRegistrationOutcome, ArchiveFsError> {
        let active = self.active_snapshot()?.ok_or_else(|| {
            ArchiveFsError::Config("cannot register without an active TOSEC snapshot".to_string())
        })?;
        let paths = self.materialize_active_dat_sources()?;
        let root_path = self.root.join("materialized").join(
            self.store
                .active_snapshot()?
                .ok_or_else(|| {
                    ArchiveFsError::Config("active TOSEC snapshot disappeared".to_string())
                })?
                .sha256,
        );
        let pack = PersistedTosecPack {
            pack_id: active.pack_id,
            root_path,
            imported_unix_seconds: active.imported_unix_seconds,
            selections: active.selected_groups,
            dats: paths
                .into_iter()
                .map(|(dat, _)| TosecPackDat {
                    relative_path: dat.relative_path,
                    raw_catalogue_name: dat.raw_catalogue_name,
                    system: dat.system,
                    category: dat.category,
                    media: dat.media,
                    raw_category_label: dat.raw_category_label,
                    classification_confident: dat.classification_confident,
                    content_sha256: Some(dat.content_sha256),
                })
                .collect(),
        };
        apply_selection_to_registry(&pack, registry_path, now_unix_seconds)
    }

    /// Existing verification tied to another immutable snapshot must be
    /// rechecked after activation. Reusing the generic signal avoids making
    /// "active" imply "verified" or "current".
    pub fn freshness_for(
        &self,
        recorded_snapshot_sha256: Option<&str>,
    ) -> Result<VerificationFreshness, ArchiveFsError> {
        let Some(active) = self.store.active_snapshot()? else {
            return Ok(VerificationFreshness::NeedsRecheck);
        };
        Ok(
            if recorded_snapshot_sha256 == Some(active.sha256.as_str()) {
                VerificationFreshness::Current
            } else {
                VerificationFreshness::NeedsRecheck
            },
        )
    }
}

fn common_release_version(dats: &[(TosecManagedDat, Vec<u8>)]) -> Option<String> {
    let first = dats.first()?.0.tosec_version.clone()?;
    dats.iter()
        .all(|(dat, _)| dat.tosec_version.as_deref() == Some(first.as_str()))
        .then_some(first)
}

fn compare_tosec_release_versions(
    candidate: Option<&str>,
    active: Option<&str>,
) -> TosecRevisionComparison {
    let (Some(candidate), Some(active)) = (candidate, active) else {
        return if active.is_none() {
            TosecRevisionComparison::NoActiveRelease
        } else {
            TosecRevisionComparison::Unknown
        };
    };
    if candidate == active {
        return TosecRevisionComparison::Same;
    }
    let is_date = |value: &str| {
        value.len() == 10
            && value.as_bytes()[4] == b'-'
            && value.as_bytes()[7] == b'-'
            && value
                .bytes()
                .enumerate()
                .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    };
    if !is_date(candidate) || !is_date(active) {
        return TosecRevisionComparison::Unknown;
    }
    match candidate.cmp(active) {
        std::cmp::Ordering::Greater => TosecRevisionComparison::Newer,
        std::cmp::Ordering::Equal => TosecRevisionComparison::Same,
        std::cmp::Ordering::Less => TosecRevisionComparison::Older,
    }
}

fn reject_materialized_symlinks(root: &Path, path: &Path) -> Result<(), ArchiveFsError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        ArchiveFsError::Config("TOSEC materialized path escaped its managed root".to_string())
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(ArchiveFsError::Config(
                "unsafe TOSEC materialized path".to_string(),
            ));
        };
        current.push(component);
        if let Ok(metadata) = std::fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            return Err(ArchiveFsError::Config(format!(
                "TOSEC materialized path contains a symbolic link: {}",
                current.display()
            )));
        }
    }
    Ok(())
}

fn resolve_inventory_dat_path(root: &Path, relative: &Path) -> Result<PathBuf, ArchiveFsError> {
    if !is_normal_relative_path(relative)
        || relative
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("dat"))
    {
        return Err(ArchiveFsError::Config(format!(
            "unsafe TOSEC DAT path: {}",
            relative.display()
        )));
    }
    let root_metadata = std::fs::symlink_metadata(root)
        .map_err(|error| ArchiveFsError::io(root.to_path_buf(), error))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ArchiveFsError::Config(
            "TOSEC release-pack root is not a real directory".to_string(),
        ));
    }
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| ArchiveFsError::io(root.to_path_buf(), error))?;
    let mut current = canonical_root.clone();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(ArchiveFsError::Config("unsafe TOSEC DAT path".to_string()));
        };
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|error| ArchiveFsError::io(current.clone(), error))?;
        if metadata.file_type().is_symlink() {
            return Err(ArchiveFsError::Config(
                "TOSEC DAT path contains a symbolic link".to_string(),
            ));
        }
    }
    let metadata = std::fs::symlink_metadata(&current)
        .map_err(|error| ArchiveFsError::io(current.clone(), error))?;
    if !metadata.is_file() || metadata.len() > crate::dat::limits::DEFAULT_MAX_FILE_SIZE {
        return Err(ArchiveFsError::Config(
            "TOSEC DAT is missing, non-regular, or oversized".to_string(),
        ));
    }
    let canonical = std::fs::canonicalize(&current)
        .map_err(|error| ArchiveFsError::io(current.clone(), error))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(ArchiveFsError::Config(
            "TOSEC DAT escapes the release-pack root".to_string(),
        ));
    }
    Ok(canonical)
}

fn encode_snapshot(
    manifest: &TosecSnapshotManifest,
    dats: &[(TosecManagedDat, Vec<u8>)],
) -> Result<Vec<u8>, ArchiveFsError> {
    let manifest_bytes = serde_json::to_vec(manifest).map_err(|error| {
        ArchiveFsError::Config(format!("could not encode TOSEC manifest: {error}"))
    })?;
    let mut output = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut output);
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();
        builder
            .append_data(&mut header, "manifest.json", manifest_bytes.as_slice())
            .map_err(|error| {
                ArchiveFsError::Config(format!("could not encode TOSEC snapshot: {error}"))
            })?;
        for (metadata, bytes) in dats {
            let archive_path = PathBuf::from("dats").join(&metadata.relative_path);
            let archive_path = archive_path.to_string_lossy();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            builder
                .append_data(&mut header, archive_path.as_ref(), bytes.as_slice())
                .map_err(|error| {
                    ArchiveFsError::Config(format!("could not encode TOSEC DAT: {error}"))
                })?;
        }
        builder.finish().map_err(|error| {
            ArchiveFsError::Config(format!("could not finish TOSEC snapshot: {error}"))
        })?;
    }
    Ok(output)
}

fn decode_snapshot(bytes: &[u8]) -> Result<TosecManagedSnapshot, ArchiveFsError> {
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut manifest_bytes = None;
    let mut members = BTreeMap::<PathBuf, Vec<u8>>::new();
    let mut examined = 0usize;
    for entry in archive.entries().map_err(|error| {
        ArchiveFsError::Config(format!("invalid TOSEC snapshot archive: {error}"))
    })? {
        examined += 1;
        if examined > MAX_TOSEC_SNAPSHOT_MEMBERS + 1 {
            return Err(ArchiveFsError::Config(
                "TOSEC snapshot has too many members".to_string(),
            ));
        }
        let mut entry = entry.map_err(|error| {
            ArchiveFsError::Config(format!("invalid TOSEC snapshot member: {error}"))
        })?;
        if !entry.header().entry_type().is_file() {
            return Err(ArchiveFsError::Config(
                "TOSEC snapshot contains a non-file member".to_string(),
            ));
        }
        let path = entry
            .path()
            .map_err(|error| {
                ArchiveFsError::Config(format!("invalid TOSEC snapshot path: {error}"))
            })?
            .into_owned();
        if path.is_absolute()
            || !path
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(ArchiveFsError::Config(
                "TOSEC snapshot contains an unsafe path".to_string(),
            ));
        }
        let size = entry.size();
        if size > MAX_TOSEC_SNAPSHOT_DAT_BYTES {
            return Err(ArchiveFsError::Config(
                "TOSEC snapshot member is oversized".to_string(),
            ));
        }
        let mut member = Vec::with_capacity(size as usize);
        entry.read_to_end(&mut member).map_err(|error| {
            ArchiveFsError::Config(format!("could not read TOSEC snapshot member: {error}"))
        })?;
        if member.len() as u64 != size {
            return Err(ArchiveFsError::Config(
                "TOSEC snapshot member was truncated".to_string(),
            ));
        }
        if path == Path::new("manifest.json") {
            if manifest_bytes.replace(member).is_some() {
                return Err(ArchiveFsError::Config(
                    "TOSEC snapshot has duplicate manifest".to_string(),
                ));
            }
        } else if path.components().next() == Some(Component::Normal("dats".as_ref())) {
            let mut components = path.components();
            components.next();
            let dat_path: PathBuf = components.map(|component| component.as_os_str()).collect();
            if !is_normal_relative_path(&dat_path) || members.insert(dat_path, member).is_some() {
                return Err(ArchiveFsError::Config(
                    "TOSEC snapshot has duplicate or unsafe DAT member".to_string(),
                ));
            }
        } else {
            return Err(ArchiveFsError::Config(
                "TOSEC snapshot contains an unexpected member".to_string(),
            ));
        }
    }
    let manifest: TosecSnapshotManifest = serde_json::from_slice(
        &manifest_bytes
            .ok_or_else(|| ArchiveFsError::Config("TOSEC snapshot has no manifest".to_string()))?,
    )
    .map_err(|error| ArchiveFsError::Config(format!("invalid TOSEC snapshot manifest: {error}")))?;
    if manifest.schema_version != TOSEC_SNAPSHOT_SCHEMA_VERSION
        || manifest.dats.len() > MAX_TOSEC_SNAPSHOT_MEMBERS
    {
        return Err(ArchiveFsError::Config(
            "unsupported TOSEC snapshot schema or size".to_string(),
        ));
    }
    if manifest.dats.len() != members.len() {
        return Err(ArchiveFsError::Config(
            "TOSEC snapshot manifest/member mismatch".to_string(),
        ));
    }
    let mut dats = Vec::with_capacity(manifest.dats.len());
    for metadata in manifest.dats {
        let bytes = members.remove(&metadata.relative_path).ok_or_else(|| {
            ArchiveFsError::Config(format!(
                "missing TOSEC DAT member: {}",
                metadata.relative_path.display()
            ))
        })?;
        let digest = sha256_bytes(&bytes);
        if digest != metadata.content_sha256 {
            return Err(ArchiveFsError::Config(format!(
                "TOSEC DAT digest mismatch: {}",
                metadata.relative_path.display()
            )));
        }
        dats.push((metadata, bytes));
    }
    Ok(TosecManagedSnapshot {
        pack_id: manifest.pack_id,
        release_version: manifest.release_version,
        selected_groups: manifest.selected_groups,
        imported_unix_seconds: manifest.imported_unix_seconds,
        source_path: manifest.source_path,
        dats,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
    /// Selected DATs whose current bytes are already represented by an
    /// existing source.  They are satisfied without replacing or duplicating
    /// the user's source.
    pub already_registered: Vec<PathBuf>,
    /// Entries this exact pack previously created but whose group is no
    /// longer selected. User-local and other-provider entries never appear
    /// here.
    pub removed: Vec<DatSourceConfigEntry>,
    /// TOSEC-ISO/PIX entries remain visible in the inventory but are outside
    /// the classic-media authority boundary of this adapter.
    pub deferred: Vec<(PathBuf, String)>,
    /// An existing source was deliberately preserved because it conflicts
    /// with the selected pack entry.
    pub conflicts: Vec<(PathBuf, String)>,
    /// Actual registration errors (bad path, parse failure, vanished file,
    /// and similar failures), distinct from intentional deferral/conflict.
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
                let existing = sources
                    .sources
                    .iter()
                    .flatten()
                    .find(|existing| existing.path == absolute_text || existing.id == id);
                if let Some(existing) = existing {
                    if existing.path == absolute_text {
                        if existing.ownership.is_user_local() {
                            outcome.already_registered.push(dat.relative_path.clone());
                            continue;
                        }
                        if existing.id == id
                            && existing.ownership == ownership
                            && existing
                                .origin
                                .as_deref()
                                .is_some_and(|origin| origin.contains(&imported.artifact_sha256))
                        {
                            outcome.already_registered.push(dat.relative_path.clone());
                            continue;
                        }
                        if existing.id == id && existing.ownership == ownership {
                            // This pack owns the entry and the bytes changed;
                            // replace it below so provenance follows the
                            // artifact actually parsed now.
                        } else {
                            outcome.conflicts.push((
                                dat.relative_path.clone(),
                                "the selected DAT path is already owned by another source; the existing entry was preserved".to_string(),
                            ));
                            continue;
                        }
                    } else {
                        outcome.conflicts.push((
                            dat.relative_path.clone(),
                            "the generated TOSEC source ID is already used by a different local DAT source; the existing entry was preserved".to_string(),
                        ));
                        continue;
                    }
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
                    health_arcade_catalogue_revisions: None,
                    unknown_fields: toml::Table::new(),
                };
                let sources_list = sources.sources.get_or_insert_with(Vec::new);
                sources_list.retain(|existing| existing.id != id);
                sources_list.push(entry.clone());
                outcome
                    .registered
                    .push(RegisteredTosecDat { entry, provenance });
            }
            Err(error) => match error {
                crate::identity_source::tosec::TosecImportError::OutOfScope {
                    catalogue_kind,
                    ..
                } => outcome.deferred.push((
                    dat.relative_path.clone(),
                    format!("deferred TOSEC {catalogue_kind} catalogue"),
                )),
                error => outcome
                    .failed
                    .push((dat.relative_path.clone(), error.to_string())),
            },
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
