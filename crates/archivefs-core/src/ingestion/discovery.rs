//! Central discovery orchestration: walks a source folder, classifies
//! every path's container and content, and produces one [`GameDiscovery`]
//! per item - accepted or skipped, always with an explanation.
//!
//! Everything in this module is read-only. Discovery never renames,
//! moves, deletes, or extracts anything; scanning a source can be run as
//! often as wanted with no side effects on the files it looks at.

use super::container::{
    ArchiveFormat, ContainerKind, FolderRole, detect_container, extension_lowercase,
    find_slave_files, list_archive_entry_names,
};
use super::content_registry::{ContentKind, content_kind_for_extension};
use super::cue_bin::resolve_cue;
use crate::ArchiveIdentity;
use crate::amiga_disk::{self, AmigaDisk, structural_amiga_floppy_observation};
use crate::identity_source::whdload::{
    WhdloadDatContext, inspect_whdload_slave_file, reconcile_whdload_slaves,
};
use crate::platform_evidence_fusion::evidence_lineage::EvidenceObservation;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Bounds mirroring the existing archive scanner's own limits (see
/// `crate::ArchiveScanner`), so a pathological source cannot make
/// discovery consume unbounded memory or time.
pub const MAX_DISCOVERY_ENTRIES: usize = 250_000;
pub const MAX_DISCOVERY_DEPTH: usize = 128;

/// Extensions of files real collections are full of that are never game
/// content in their own right - box art, manuals, metadata, saves, and
/// filesystem/emulator-frontend housekeeping files sitting next to the
/// actual ROMs/discs/images. Live testing against a ~75,000-file collection
/// showed these outnumbering real content by a wide margin, so counting
/// every one of them as a "[`SkipReason::UnsupportedExtension`]" item
/// needing attention produced a "Needs attention: 167,499 unknown items"
/// figure that was really almost entirely box art and readmes - true but
/// useless, since there's nothing a person can or should do about a JPEG.
/// Filtered out at walk time (never becoming a [`GameDiscovery`] item at
/// all) rather than merely re-labelled, so they don't inflate `DiscoveryStats`
/// either. This list is deliberately conservative - only extensions with no
/// plausible game-content meaning anywhere in `content_registry` - so it can
/// never hide a real game file.
const KNOWN_NON_GAME_EXTENSIONS: &[&str] = &[
    // Images (box art, screenshots, fanart).
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tga", "ico",
    // Text/metadata sidecars. Note: "md" is deliberately excluded here - it
    // is a registered Sega Genesis/Mega Drive ROM extension in
    // `content_registry`, not a safe sidecar to filter (a real-world
    // collision the end-to-end extension-coverage test caught).
    "txt", "nfo", "xml", "json", "yml", "yaml", "ini", "cfg", "conf", "log",
    // Frontend/database housekeeping.
    "db", "url", "lnk", "ds_store",
    // Video/audio extras (trailers, theme music) sometimes bundled alongside a game.
    "mp4", "webm", "mp3", "ogg",
];

/// Whether an extension is a known ancillary/supporting-media type rather
/// than game content. Callers which build a *games-only* view should use this
/// same conservative classification instead of treating cover art or manuals
/// as unidentified games.
pub fn is_known_non_game_extension(extension: &str) -> bool {
    KNOWN_NON_GAME_EXTENSIONS.contains(&extension)
}

/// Why a discovered item was not accepted as a confident library
/// candidate. Every variant is a *reason*, not a dead end - each carries
/// (via [`SkipReason::suggested_action`]) what a person could do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// No container/content registry recognises this extension at all.
    UnsupportedExtension,
    /// The content category was recognised (it looks like a ROM, disc
    /// image, etc.) but platform/identity detection could not confirm
    /// what game - or even confidently what platform - it is.
    RecognizedContentNoIdentityMatch,
    /// A lone `.bin` (or similar) file with no `.cue` sheet naming it, or
    /// a `.cue` sheet whose referenced file(s) could not be found.
    MissingPairedFile,
    /// The extension is inherently shared between formats/platforms and
    /// nothing available corroborates which one this is.
    AmbiguousPlatform,
    /// The path matched a recognised extension/container but its content
    /// failed to parse as that format (e.g. an `.hdf` that is not a valid
    /// RDB image).
    InvalidContent(String),
}

impl SkipReason {
    pub fn label(&self) -> &'static str {
        match self {
            Self::UnsupportedExtension => "Unsupported file type",
            Self::RecognizedContentNoIdentityMatch => "No identity match found",
            Self::MissingPairedFile => "Missing paired file",
            Self::AmbiguousPlatform => "Ambiguous platform",
            Self::InvalidContent(_) => "Could not be read as expected",
        }
    }

    pub fn suggested_action(&self) -> &'static str {
        match self {
            Self::UnsupportedExtension => {
                "EmuWiz doesn't recognise this file type yet - it was left alone."
            }
            Self::RecognizedContentNoIdentityMatch => {
                "The content looks recognisable, but nothing matched it to a known game. \
                 It can still be added manually."
            }
            Self::MissingPairedFile => {
                "A disc image needs its matching .cue/.bin pair - check that both files \
                 are present together."
            }
            Self::AmbiguousPlatform => {
                "This file's extension is shared by more than one platform, and nothing \
                 nearby confirms which one."
            }
            Self::InvalidContent(_) => {
                "The file has a recognised extension but could not be read as that format - \
                 it may be corrupt, partial, or a false match."
            }
        }
    }
}

/// How confidently a [`GameDiscovery`] can be treated as ready for the
/// library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationState {
    /// Content and identity both resolved.
    Accepted,
    /// Content resolved but identity/platform did not - still visible,
    /// not silently discarded (see [`SkipReason::RecognizedContentNoIdentityMatch`]).
    Skipped,
}

/// One discovered path: how it is stored, what it appears to be, and -
/// whether accepted or skipped - always a plain-language explanation of
/// why. Designed to be rendered directly by a future UI: every field a
/// person would ask about ("what is this", "why wasn't it picked up",
/// "what should I do") has a home here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameDiscovery {
    pub path: PathBuf,
    pub container: ContainerKind,
    pub content: Option<ContentKind>,
    /// The best platform guess available, from the existing identity
    /// system - never recomputed here.
    pub platform_hint: Option<String>,
    /// Display/normalised-name identity evidence, reused as-is from
    /// [`crate::ArchiveIdentity`].
    pub identity_candidate: Option<IdentitySummary>,
    pub validation_state: ValidationState,
    /// Always populated - a short, plain-language sentence explaining
    /// what this item is (if accepted) or why it wasn't (if skipped).
    pub explanation: String,
    pub skip_reason: Option<SkipReason>,
}

/// A trimmed, UI-friendly projection of [`crate::ArchiveIdentity`] -
/// display name and detected platform only. Discovery never invents its
/// own identity fields; this is a read of the existing system's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySummary {
    pub display_name: String,
    pub platform: Option<String>,
}

impl From<&ArchiveIdentity> for IdentitySummary {
    fn from(identity: &ArchiveIdentity) -> Self {
        Self {
            display_name: identity.display_name.clone(),
            platform: identity.platform.clone(),
        }
    }
}

/// Per-content-category counts for one source, for "here is what your
/// collection actually contains" reporting. Counted by recognised
/// container/content category, independent of identity confidence - see
/// the module docs on why an item can be counted here and still appear
/// with a [`SkipReason`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscoveryStats {
    pub archives: usize,
    pub loose_roms: usize,
    pub disc_images: usize,
    pub amiga_images: usize,
    pub computer_disks: usize,
    /// Machine-state snapshots (`.z80`, `.sna`, `.szx`, ...).
    pub snapshots: usize,
    pub game_folders: usize,
    pub unknown: usize,
}

impl DiscoveryStats {
    /// Folds another source's counts into this one - used to combine
    /// per-source-folder reports into one run-wide total.
    pub fn merge(&mut self, other: &DiscoveryStats) {
        self.archives += other.archives;
        self.loose_roms += other.loose_roms;
        self.disc_images += other.disc_images;
        self.amiga_images += other.amiga_images;
        self.computer_disks += other.computer_disks;
        self.snapshots += other.snapshots;
        self.game_folders += other.game_folders;
        self.unknown += other.unknown;
    }

    fn record(&mut self, discovery: &GameDiscovery) {
        if matches!(discovery.container, ContainerKind::Archive(_)) {
            self.archives += 1;
            return;
        }
        match discovery.content {
            Some(ContentKind::RomCartridge) => self.loose_roms += 1,
            Some(ContentKind::DiscImage) => self.disc_images += 1,
            Some(ContentKind::AmigaImage) => self.amiga_images += 1,
            Some(ContentKind::ComputerDisk) | Some(ContentKind::TapeImage) => {
                self.computer_disks += 1
            }
            Some(ContentKind::MachineSnapshot) => self.snapshots += 1,
            Some(ContentKind::Archive) | Some(ContentKind::Executable) => self.archives += 1,
            Some(ContentKind::WhdloadInstall) | Some(ContentKind::ExtractedGameFolder) => {
                self.game_folders += 1
            }
            None => self.unknown += 1,
        }
    }
}

/// Per-[`SkipReason`] counts for one source, always exact - the "needs
/// attention" breakdown a person actually wants ("120 unknown items", "15
/// missing cue/bin pairs"), computed alongside [`DiscoveryStats`] rather
/// than requiring a caller to filter/count [`SourceDiscoveryReport::items`]
/// itself (which is only ever a bounded sample once aggregated across a
/// multi-folder run - see `database::ScanPersistSummary`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SkipReasonCounts {
    pub unsupported_extension: usize,
    pub no_identity_match: usize,
    pub missing_paired_file: usize,
    pub ambiguous_platform: usize,
    pub invalid_content: usize,
}

impl SkipReasonCounts {
    pub fn merge(&mut self, other: &SkipReasonCounts) {
        self.unsupported_extension += other.unsupported_extension;
        self.no_identity_match += other.no_identity_match;
        self.missing_paired_file += other.missing_paired_file;
        self.ambiguous_platform += other.ambiguous_platform;
        self.invalid_content += other.invalid_content;
    }

    pub fn total(&self) -> usize {
        self.unsupported_extension
            + self.no_identity_match
            + self.missing_paired_file
            + self.ambiguous_platform
            + self.invalid_content
    }

    fn record(&mut self, reason: &SkipReason) {
        match reason {
            SkipReason::UnsupportedExtension => self.unsupported_extension += 1,
            SkipReason::RecognizedContentNoIdentityMatch => self.no_identity_match += 1,
            SkipReason::MissingPairedFile => self.missing_paired_file += 1,
            SkipReason::AmbiguousPlatform => self.ambiguous_platform += 1,
            SkipReason::InvalidContent(_) => self.invalid_content += 1,
        }
    }
}

/// One structural lineage observation produced during discovery, paired
/// with the file it describes.
///
/// This is how a bytes-validated structural fact from discovery enters the
/// existing identity pipeline
/// ([`crate::platform_evidence_fusion::evidence_lineage`]) without discovery
/// itself resolving identity. Currently only
/// [`structural_amiga_floppy_observation`] for a `.adf` whose OFS/FFS
/// structures validated - a `Strong` `Amiga` platform candidate, never an
/// exact release (see that function's own contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredStructuralEvidence {
    pub path: PathBuf,
    pub observation: EvidenceObservation,
}

/// The full, read-only result of discovering one source folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceDiscoveryReport {
    pub items: Vec<GameDiscovery>,
    pub stats: DiscoveryStats,
    pub skip_reasons: SkipReasonCounts,
    /// Structural lineage evidence gathered while classifying items - the
    /// `.adf` content-inspection path's own
    /// [`structural_amiga_floppy_observation`] output, one per validated
    /// Amiga floppy. Empty when no such file was seen.
    pub structural_evidence: Vec<DiscoveredStructuralEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    Io(String),
    NotADirectory,
}

/// Discover every game-content item under `root`, bounded and read-only.
/// `root` is also used as the source root for identity/platform detection
/// (folder-alias matching never looks above it), matching the existing
/// scanner's behaviour.
pub fn discover_source(root: &Path) -> Result<SourceDiscoveryReport, DiscoveryError> {
    discover_source_with_whdload_dat(root, None)
}

/// [`discover_source`] with an optional generic DAT context for reconciling
/// any discovered WHDLoad install's verified slave evidence against a
/// catalogue's own hash index (see
/// [`crate::identity_source::whdload::reconcile`]). When `whdload_dat` is
/// `None` - the current catalogue-scan caller - WHDLoad installs still gain
/// structural Amiga evidence, just no exact catalogue identity.
pub fn discover_source_with_whdload_dat(
    root: &Path,
    whdload_dat: Option<&WhdloadDatContext<'_>>,
) -> Result<SourceDiscoveryReport, DiscoveryError> {
    if !root.is_dir() {
        return Err(DiscoveryError::NotADirectory);
    }

    let files = walk_bounded(root)?;

    // CUE sheets are resolved first so their referenced `.bin`s are known
    // and excluded from independent classification - see module docs.
    let mut consumed: BTreeSet<PathBuf> = BTreeSet::new();
    let mut items = Vec::new();
    let mut structural_evidence: Vec<DiscoveredStructuralEvidence> = Vec::new();

    for path in &files {
        if extension_lowercase(path).as_deref() != Some("cue") {
            continue;
        }
        items.push(discover_cue(path, &mut consumed));
    }

    for path in &files {
        if consumed.contains(path) {
            continue;
        }
        if extension_lowercase(path).as_deref() == Some("cue") {
            continue; // already handled above
        }
        items.push(discover_file(path, root, whdload_dat, &mut structural_evidence));
    }

    let mut stats = DiscoveryStats::default();
    let mut skip_reasons = SkipReasonCounts::default();
    for item in &items {
        stats.record(item);
        if let Some(reason) = &item.skip_reason {
            skip_reasons.record(reason);
        }
    }
    Ok(SourceDiscoveryReport {
        items,
        stats,
        skip_reasons,
        structural_evidence,
    })
}

/// Iterative, symlink-refusing, bounded walk collecting every regular
/// file and every folder that resolves to a leaf [`FolderRole`] (WHDLoad
/// install or extracted game folder) - both stop recursion, matching
/// "a container holds one game" rather than "a container is a nested
/// source". Read-only: only ever `read_dir`/`symlink_metadata`.
fn walk_bounded(root: &Path) -> Result<Vec<PathBuf>, DiscoveryError> {
    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    let mut collected = Vec::new();
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DISCOVERY_DEPTH || collected.len() >= MAX_DISCOVERY_ENTRIES {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            if collected.len() >= MAX_DISCOVERY_ENTRIES {
                break;
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue; // never follow symlinks during discovery
            }
            if metadata.is_dir() {
                match super::container::detect_container(&path, true) {
                    ContainerKind::Folder(FolderRole::Plain) => {
                        stack.push((path, depth + 1));
                    }
                    ContainerKind::Folder(_) => collected.push(path),
                    _ => unreachable!("detect_container(_, true) always returns Folder"),
                }
            } else if metadata.is_file() {
                let is_known_non_game = extension_lowercase(&path)
                    .is_some_and(|extension| is_known_non_game_extension(&extension));
                if !is_known_non_game {
                    collected.push(path);
                }
            }
        }
    }
    Ok(collected)
}

fn discover_cue(cue_path: &Path, consumed: &mut BTreeSet<PathBuf>) -> GameDiscovery {
    match resolve_cue(cue_path) {
        Ok(sheet) => {
            let missing: Vec<&PathBuf> = sheet
                .referenced_paths
                .iter()
                .filter(|referenced| !referenced.is_file())
                .collect();
            if !missing.is_empty() {
                return skipped(
                    cue_path.to_path_buf(),
                    ContainerKind::DirectFile,
                    Some(ContentKind::DiscImage),
                    SkipReason::MissingPairedFile,
                    format!(
                        "This CUE sheet references {} file(s) that could not be found next to it.",
                        missing.len()
                    ),
                );
            }
            for referenced in &sheet.referenced_paths {
                consumed.insert(referenced.clone());
            }
            accepted(
                cue_path.to_path_buf(),
                ContainerKind::DirectFile,
                ContentKind::DiscImage,
                identity_for(cue_path, cue_path.parent().unwrap_or(cue_path)),
                format!(
                    "Disc image ({} file(s) referenced by this CUE sheet).",
                    sheet.referenced_paths.len()
                ),
            )
        }
        Err(_) => skipped(
            cue_path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::DiscImage),
            SkipReason::MissingPairedFile,
            "This CUE sheet could not be read or names no data file.".to_string(),
        ),
    }
}

fn discover_file(
    path: &Path,
    source_root: &Path,
    whdload_dat: Option<&WhdloadDatContext<'_>>,
    structural_evidence: &mut Vec<DiscoveredStructuralEvidence>,
) -> GameDiscovery {
    let container = detect_container(path, path.is_dir());
    match &container {
        ContainerKind::Folder(FolderRole::WhdloadInstall) => {
            discover_whdload_folder(path, whdload_dat)
        }
        ContainerKind::Folder(FolderRole::ExtractedGame) => discover_extracted_folder(path),
        ContainerKind::Folder(FolderRole::Plain) => unreachable!("plain folders are recursed"),
        ContainerKind::Archive(format) => discover_archive(path, *format, source_root),
        ContainerKind::DirectFile => discover_direct_file(path, source_root, structural_evidence),
    }
}

/// A discovered WHDLoad install: every readable `.slave` is parsed and
/// whole-file hashed once (the existing bounded
/// [`inspect_whdload_slave_file`]), then reconciled against `whdload_dat`
/// (see [`reconcile_whdload_slaves`]). A structurally valid slave always
/// yields strong structural Amiga evidence - so the platform hint is
/// `Amiga` from the slave's own structure, never from the folder name; an
/// exact whole-`.slave` SHA-1 hit in the catalogue additionally names the
/// release. The folder is refused only when its first `.slave` cannot be
/// read as a valid WHDLoad slave, exactly as before.
fn discover_whdload_folder(
    path: &Path,
    whdload_dat: Option<&WhdloadDatContext<'_>>,
) -> GameDiscovery {
    let slaves = find_slave_files(path);
    let Some(first_slave) = slaves.first() else {
        // classify_folder only returns WhdloadInstall when a `.slave` was
        // seen; a concurrent change could still make this empty.
        return skipped(
            path.to_path_buf(),
            ContainerKind::Folder(FolderRole::WhdloadInstall),
            Some(ContentKind::WhdloadInstall),
            SkipReason::InvalidContent("no readable .slave file remained".to_string()),
            "This looked like a WHDLoad install but no .slave file could be read.".to_string(),
        );
    };
    let first_artifact = match inspect_whdload_slave_file(first_slave) {
        Ok(artifact) => artifact,
        Err(error) => {
            return skipped(
                path.to_path_buf(),
                ContainerKind::Folder(FolderRole::WhdloadInstall),
                Some(ContentKind::WhdloadInstall),
                SkipReason::InvalidContent(format!("{error:?}")),
                "This folder has a .slave file, but it could not be read as a valid WHDLoad \
                 slave."
                    .to_string(),
            );
        }
    };

    // Parse the remaining slaves best-effort: a malformed extra slave never
    // erases the identity a valid one already established.
    let mut artifacts = vec![first_artifact];
    for extra in slaves.iter().skip(1) {
        if let Ok(artifact) = inspect_whdload_slave_file(extra) {
            artifacts.push(artifact);
        }
    }

    let reconciliation = reconcile_whdload_slaves(&artifacts, whdload_dat);

    // The platform hint comes from the verified slave structure
    // (`structural_slave_observation` -> `Amiga`), not the folder name.
    let mut identity = identity_for(path, path.parent().unwrap_or(path));
    if let Some(summary) = identity.as_mut() {
        summary.platform = reconciliation
            .observations
            .iter()
            .find_map(|observation| observation.platform_candidate.clone());
    }

    let mut explanation = format!(
        "WHDLoad install ({} readable slave file(s); verified Amiga WHDLoad slave structure).",
        artifacts.len()
    );
    if reconciliation.ambiguous {
        explanation.push_str(
            " Its slaves resolve to conflicting catalogue releases - identity needs review.",
        );
    } else if let Some(release) = reconciliation.agreed_release() {
        explanation.push_str(&format!(" Exact catalogue match: {release}."));
    }

    accepted(
        path.to_path_buf(),
        ContainerKind::Folder(FolderRole::WhdloadInstall),
        ContentKind::WhdloadInstall,
        identity,
        explanation,
    )
}

fn discover_extracted_folder(path: &Path) -> GameDiscovery {
    accepted(
        path.to_path_buf(),
        ContainerKind::Folder(FolderRole::ExtractedGame),
        ContentKind::ExtractedGameFolder,
        None,
        "Game folder (contains recognisable game content directly).".to_string(),
    )
}

fn discover_archive(path: &Path, format: ArchiveFormat, source_root: &Path) -> GameDiscovery {
    let container = ContainerKind::Archive(format);
    let Some(entries) = list_archive_entry_names(path, format) else {
        // RAR/7z are recognised but not listed - see container.rs docs.
        return GameDiscovery {
            path: path.to_path_buf(),
            container,
            content: None,
            platform_hint: None,
            identity_candidate: None,
            validation_state: ValidationState::Skipped,
            explanation: format!(
                "{} - contents are not yet inspected for this format, so EmuWiz can \
                 see the archive but not what's inside it.",
                format.label()
            ),
            skip_reason: None,
        };
    };
    let recognized_member = entries.iter().find(|entry| {
        Path::new(&entry.0)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| content_kind_for_extension(&extension.to_ascii_lowercase()).is_some())
            .unwrap_or(false)
    });
    let Some(member) = recognized_member else {
        return skipped(
            path.to_path_buf(),
            container,
            None,
            SkipReason::UnsupportedExtension,
            format!(
                "{} - none of its {} entries look like recognised game content.",
                format.label(),
                entries.len()
            ),
        );
    };
    let member_extension = Path::new(&member.0)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    let content = content_kind_for_extension(&member_extension)
        .expect("recognized_member only matches when content_kind_for_extension is Some");

    let identity = identity_for(path, source_root);
    let explanation = format!(
        "{} containing {} ({}).",
        format.label(),
        content.label(),
        member.0
    );
    match &identity {
        Some(summary) if summary.platform.is_some() => GameDiscovery {
            path: path.to_path_buf(),
            container,
            content: Some(content),
            platform_hint: summary.platform.clone(),
            identity_candidate: identity.clone(),
            validation_state: ValidationState::Accepted,
            explanation,
            skip_reason: None,
        },
        _ => GameDiscovery {
            path: path.to_path_buf(),
            container,
            content: Some(content),
            platform_hint: None,
            identity_candidate: identity,
            validation_state: ValidationState::Skipped,
            explanation,
            skip_reason: Some(SkipReason::RecognizedContentNoIdentityMatch),
        },
    }
}

fn discover_direct_file(
    path: &Path,
    source_root: &Path,
    structural_evidence: &mut Vec<DiscoveredStructuralEvidence>,
) -> GameDiscovery {
    let Some(extension) = extension_lowercase(path) else {
        return skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            None,
            SkipReason::UnsupportedExtension,
            "This file has no extension EmuWiz can recognise.".to_string(),
        );
    };

    if extension == "rdb" {
        return discover_amiga_image(path, source_root);
    }
    if extension == "adf" {
        return discover_amiga_floppy(path, source_root, structural_evidence);
    }
    if matches!(extension.as_str(), "hdf" | "hdfx") {
        return discover_ambiguous_disk_image(path, source_root);
    }
    if matches!(extension.as_str(), "z80" | "sna" | "szx") {
        return discover_spectrum_snapshot(path, source_root);
    }
    if extension == "dsk" {
        return discover_dsk_image(path, source_root);
    }
    if extension == "d88" {
        return discover_d88_image(path, source_root);
    }
    if matches!(extension.as_str(), "hdi" | "nhd") {
        return discover_hard_disk_image(path, source_root, &extension);
    }
    if matches!(extension.as_str(), "trd" | "scl") {
        return discover_trdos_media(path, source_root);
    }
    if matches!(extension.as_str(), "ssd" | "dsd") {
        return discover_dfs_media(path, source_root);
    }
    if matches!(extension.as_str(), "xbe" | "xex" | "xiso") {
        return discover_xbox_content(path, source_root, &extension);
    }

    let Some(content) = content_kind_for_extension(&extension) else {
        if extension == "bin" {
            return skipped(
                path.to_path_buf(),
                ContainerKind::DirectFile,
                None,
                SkipReason::MissingPairedFile,
                "This .bin file has no matching .cue sheet next to it.".to_string(),
            );
        }
        return skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            None,
            SkipReason::UnsupportedExtension,
            format!("EmuWiz doesn't yet recognise the .{extension} extension."),
        );
    };

    let identity = identity_for(path, source_root);
    let explanation = format!("{}.", content.label());
    match &identity {
        Some(summary) if summary.platform.is_some() => accepted(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            content,
            identity,
            explanation,
        ),
        _ => GameDiscovery {
            path: path.to_path_buf(),
            container: ContainerKind::DirectFile,
            content: Some(content),
            platform_hint: None,
            identity_candidate: identity,
            validation_state: ValidationState::Skipped,
            explanation,
            skip_reason: Some(SkipReason::RecognizedContentNoIdentityMatch),
        },
    }
}

fn discover_xbox_content(path: &Path, source_root: &Path, extension: &str) -> GameDiscovery {
    let (platform, content) = match extension {
        "xbe" => ("Xbox", ContentKind::Executable),
        "xex" => ("Xbox360", ContentKind::Executable),
        "xiso" => ("Xbox", ContentKind::DiscImage),
        _ => unreachable!("called only for registered Xbox extensions"),
    };
    let identity = crate::game_identity::inspect_catalogued_game_identity(path, Some(platform));
    if identity.complete
        && identity.platform
            == crate::game_identity::IdentityPlatform::from_catalogue(Some(platform))
    {
        return accepted(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            content,
            identity_for(path, source_root),
            format!(
                "{} content validated by its existing bounded identity path.",
                content.label()
            ),
        );
    }
    skipped(
        path.to_path_buf(),
        ContainerKind::DirectFile,
        Some(content),
        SkipReason::InvalidContent(format!(
            ".{extension} did not pass its Xbox identity checks"
        )),
        format!(
            "This .{extension} file was recognised, but its Xbox structure or identity did not validate."
        ),
    )
}

/// `.rdb` is registered unconditionally as [`ContentKind::AmigaImage`] (see
/// `content_registry`) - RDB is Amiga-specific, unlike `.hdf`/`.hdfx` (see
/// [`discover_ambiguous_disk_image`]). Still runs
/// [`amiga_disk::inspect_amiga_image`] to validate the file actually
/// parses (covers both RDB-partitioned and, since that function's fix,
/// flat AmigaDOS images) and to report the partition/filesystem detail.
fn discover_amiga_image(path: &Path, source_root: &Path) -> GameDiscovery {
    match amiga_disk::inspect_amiga_image(path) {
        Ok(disk) => accepted(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            ContentKind::AmigaImage,
            identity_for(path, source_root),
            amiga_explanation(&disk),
        ),
        Err(error) => skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::AmigaImage),
            SkipReason::InvalidContent(format!("{error:?}")),
            format!("This looked like an Amiga disk image but could not be read: {error}"),
        ),
    }
}

/// `.hdf`/`.hdfx` are a real extension collision: Amiga hard-disk images
/// and Sharp X68000 hard-disk images both use it (confirmed against a
/// real X68000 collection during validation, where archive members named
/// `*.hdf` were previously mislabelled `AmigaImage`). Resolution order:
///
/// 1. [`amiga_disk::inspect_amiga_image`] succeeds -> it really is a
///    readable Amiga image (RDB-partitioned or flat AmigaDOS); structural
///    evidence outranks naming, so this is accepted as `AmigaImage`
///    regardless of what the identity system's platform hint says.
/// 2. Amiga parsing fails, but the existing identity system resolves a
///    platform anyway (e.g. a `Sharp X68000` folder alias) -> accepted as
///    the more general [`ContentKind::ComputerDisk`], visible with a
///    known platform even though the exact disk format wasn't verified.
/// 3. Neither -> visible but flagged [`SkipReason::AmbiguousPlatform`]:
///    genuinely unresolved which format/platform this is.
fn discover_ambiguous_disk_image(path: &Path, source_root: &Path) -> GameDiscovery {
    if let Ok(disk) = amiga_disk::inspect_amiga_image(path) {
        return accepted(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            ContentKind::AmigaImage,
            identity_for(path, source_root),
            amiga_explanation(&disk),
        );
    }

    let identity = identity_for(path, source_root);
    match &identity {
        Some(summary) if summary.platform.is_some() => accepted(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            ContentKind::ComputerDisk,
            identity,
            "Disk image (.hdf/.hdfx is shared between formats; not verified as an Amiga \
             image, but the platform is otherwise identified)."
                .to_string(),
        ),
        _ => skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::ComputerDisk),
            SkipReason::AmbiguousPlatform,
            "This .hdf/.hdfx file could not be confirmed as an Amiga image, and no other \
             platform evidence identified it either - the extension is shared across formats \
             (e.g. Amiga and Sharp X68000)."
                .to_string(),
        ),
    }
}

fn amiga_explanation(disk: &AmigaDisk) -> String {
    format!("Amiga image ({} partition(s)).", disk.rdb.partitions.len())
}

/// `.adf` is the dominant Amiga floppy format, and - unlike `.rdb`, which
/// is Amiga-specific - it is a real cross-platform extension collision:
/// Acorn ADFS / Archimedes floppy images use `.adf` too (see the `Amiga`
/// platform's `conflicts_with`). It has therefore always been classified
/// mainly by extension and weak platform evidence, never by its own
/// contents. This routes it through [`amiga_disk::inspect_amiga_floppy`],
/// which reuses the existing bounded RDB / flat-AmigaDOS reader and the
/// existing OFS/FFS traversal to validate the on-disc boot and root
/// blocks:
///
/// 1. Contents validate as AmigaDOS -> accepted as [`ContentKind::AmigaImage`]
///    with a structural `Amiga` platform hint that outranks any filename
///    signal, and an explanation naming the DOS variant, OFS/FFS family,
///    block size, and flags.
/// 2. Contents do not validate as Amiga, but the existing identity system
///    still resolves a *non-Amiga* platform (e.g. an `Acorn Archimedes`
///    folder) -> accepted as the more general [`ContentKind::ComputerDisk`],
///    visible with that platform, the Amiga claim withheld.
/// 3. Neither -> refused with the structural failure explained; no Amiga
///    identity is produced from the `.adf` extension alone.
fn discover_amiga_floppy(
    path: &Path,
    source_root: &Path,
    structural_evidence: &mut Vec<DiscoveredStructuralEvidence>,
) -> GameDiscovery {
    match amiga_disk::inspect_amiga_floppy(path) {
        Ok(inspection) => {
            // The one production caller of `structural_amiga_floppy_observation`:
            // the validated OFS/FFS inspection becomes a `Strong` `Amiga`
            // platform candidate in the lineage pipeline. It is the
            // authority for the platform hint here - ahead of any filename
            // or folder signal - and it never asserts a release identity
            // (see that function's contract; the volume label stays in its
            // descriptive `notes` only).
            let observation = structural_amiga_floppy_observation(&inspection);
            let platform_hint = observation.platform_candidate.clone();
            structural_evidence.push(DiscoveredStructuralEvidence {
                path: path.to_path_buf(),
                observation,
            });
            GameDiscovery {
                path: path.to_path_buf(),
                container: ContainerKind::DirectFile,
                content: Some(ContentKind::AmigaImage),
                platform_hint,
                identity_candidate: identity_for(path, source_root),
                validation_state: ValidationState::Accepted,
                explanation: amiga_floppy_explanation(&inspection),
                skip_reason: None,
            }
        }
        Err(error) => {
            let identity = identity_for(path, source_root);
            match &identity {
                Some(summary)
                    if summary
                        .platform
                        .as_deref()
                        .is_some_and(|platform| platform != "Amiga") =>
                {
                    accepted(
                        path.to_path_buf(),
                        ContainerKind::DirectFile,
                        ContentKind::ComputerDisk,
                        identity,
                        "Disk image (.adf is shared between Amiga and Acorn ADFS; not \
                         verified as an Amiga floppy, but the platform is otherwise \
                         identified)."
                            .to_string(),
                    )
                }
                _ => amiga_floppy_refusal(path, &error),
            }
        }
    }
}

fn amiga_floppy_explanation(inspection: &amiga_disk::AmigaFloppyInspection) -> String {
    let filesystem = &inspection.filesystem;
    let family = match filesystem.family {
        amiga_disk::AmigaDosFamily::Ofs => "OFS",
        amiga_disk::AmigaDosFamily::Ffs => "FFS",
    };
    let mut detail = format!(
        "Amiga floppy image (DOS\\{} / {family}, {}-byte blocks",
        filesystem.dos_type, filesystem.block_size
    );
    if filesystem.international {
        detail.push_str(", international");
    }
    if filesystem.directory_cache {
        detail.push_str(", dir-cache");
    }
    detail.push(')');
    if let Some(label) = filesystem
        .volume_label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        detail.push_str(&format!(" - volume {label:?}"));
    }
    detail.push('.');
    detail
}

fn amiga_floppy_refusal(path: &Path, error: &amiga_disk::AmigaFloppyError) -> GameDiscovery {
    use amiga_disk::AmigaFloppyError;
    let (reason, explanation) = match error {
        AmigaFloppyError::Container(inner) => (
            SkipReason::InvalidContent(format!("{inner:?}")),
            format!(
                "This file is named .adf but its contents are not a readable Amiga \
                 floppy image: {inner}."
            ),
        ),
        AmigaFloppyError::NoPartition => (
            SkipReason::InvalidContent("no Amiga partition present".to_string()),
            "This .adf parsed as a container but held no traversable AmigaDOS filesystem."
                .to_string(),
        ),
        AmigaFloppyError::Filesystem(inner) => (
            SkipReason::InvalidContent(format!("{inner:?}")),
            format!(
                "This .adf has an AmigaDOS boot signature but its OFS/FFS structures \
                 did not validate: {inner}."
            ),
        ),
    };
    skipped(
        path.to_path_buf(),
        ContainerKind::DirectFile,
        Some(ContentKind::AmigaImage),
        reason,
        explanation,
    )
}

/// `.z80` / `.sna` / `.szx` are ZX Spectrum machine snapshots. The category
/// is known from the extension; the Sinclair family and any machine subtype
/// come only from parsing the bytes via
/// [`crate::zx_spectrum_snapshot::inspect_spectrum_snapshot_file`]. A file
/// that merely ends in one of these extensions never reaches
/// [`ValidationState::Accepted`] here without that structural proof, and a
/// snapshot never yields a unique game identity.
fn discover_spectrum_snapshot(path: &Path, source_root: &Path) -> GameDiscovery {
    match crate::zx_spectrum_snapshot::inspect_spectrum_snapshot_file(path) {
        Ok(inspection) => {
            let machine = inspection
                .machine()
                .map(|machine| machine.label().to_string());
            let subtype = if inspection.machine_subtype_is_encoded() {
                machine.clone().unwrap_or_else(|| "unknown".to_string())
            } else {
                machine
                    .clone()
                    .map(|label| format!("{label} (implied by the snapshot form, not encoded)"))
                    .unwrap_or_else(|| "machine subtype not encoded".to_string())
            };
            GameDiscovery {
                path: path.to_path_buf(),
                container: ContainerKind::DirectFile,
                content: Some(ContentKind::MachineSnapshot),
                // The snapshot structure is authoritative for the Sinclair
                // platform family, ahead of any filename signal.
                platform_hint: Some(
                    crate::zx_spectrum_snapshot::SpectrumMachine::PLATFORM.to_string(),
                ),
                identity_candidate: identity_for(path, source_root),
                validation_state: ValidationState::Accepted,
                explanation: format!(
                    "{} - {subtype}. Structural platform/media evidence only; exact game \
                     identity still needs a DAT/catalogue match.",
                    inspection.facts.format.label()
                ),
                skip_reason: None,
            }
        }
        Err(error) => skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::MachineSnapshot),
            SkipReason::InvalidContent(format!("{error}")),
            format!(
                "This file is named like a ZX Spectrum snapshot but its contents did not \
                 validate as one: {error}."
            ),
        ),
    }
}

/// `.dsk` is a CPCEMU container shared by the Amstrad CPC, ZX Spectrum +3 and
/// Amstrad PCW. It is resolved from contents through the shared structural
/// disk layer ([`crate::disk_format::inspect_disk_format`]):
///
/// 1. a valid `+3DOS`/PCW disk specification on track 0 -> accepted as a
///    ZX Spectrum disk;
/// 2. a valid bare CPCEMU container with a platform otherwise identified
///    (folder alias) -> accepted under that platform as a computer disk;
/// 3. a valid bare container with nothing else -> stays ambiguous
///    ([`SkipReason::RecognizedContentNoIdentityMatch`]), never forced to a
///    platform;
/// 4. not a valid container -> refused.
fn discover_dsk_image(path: &Path, source_root: &Path) -> GameDiscovery {
    use crate::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};

    let evidence = inspect_disk_format(
        path,
        &crate::safe_read::TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    let explanation = evidence
        .evidence
        .first()
        .cloned()
        .unwrap_or_else(|| "CPCEMU .dsk container".to_string());

    match evidence.format {
        Some(DiskFormat::SpectrumPlus3Disk) => GameDiscovery {
            path: path.to_path_buf(),
            container: ContainerKind::DirectFile,
            content: Some(ContentKind::ComputerDisk),
            platform_hint: Some("ZX Spectrum".to_string()),
            identity_candidate: identity_for(path, source_root),
            validation_state: ValidationState::Accepted,
            explanation: format!("ZX Spectrum +3 disk. {explanation}."),
            skip_reason: None,
        },
        Some(DiskFormat::CpcEmuDsk) => {
            let identity = identity_for(path, source_root);
            match &identity {
                Some(summary) if summary.platform.is_some() => accepted(
                    path.to_path_buf(),
                    ContainerKind::DirectFile,
                    ContentKind::ComputerDisk,
                    identity,
                    format!(
                        "Computer disk image (CPCEMU .dsk container; shared by CPC / +3 / PCW, \
                         platform identified from other evidence). {explanation}."
                    ),
                ),
                _ => GameDiscovery {
                    path: path.to_path_buf(),
                    container: ContainerKind::DirectFile,
                    content: Some(ContentKind::ComputerDisk),
                    platform_hint: None,
                    identity_candidate: identity,
                    validation_state: ValidationState::Skipped,
                    explanation: format!(
                        "Valid CPCEMU .dsk container, but it is shared by the Amstrad CPC, \
                         ZX Spectrum +3 and Amstrad PCW and nothing else identifies which. \
                         {explanation}."
                    ),
                    skip_reason: Some(SkipReason::RecognizedContentNoIdentityMatch),
                },
            }
        }
        _ => skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::ComputerDisk),
            SkipReason::InvalidContent(
                evidence
                    .refusal
                    .as_ref()
                    .map(|refusal| refusal.detail())
                    .unwrap_or_else(|| "not a recognised CPCEMU .dsk container".to_string()),
            ),
            "This .dsk file is not a readable CPCEMU disk container.".to_string(),
        ),
    }
}

/// `.d88` is a structurally validated container shared by PC-88, PC-98,
/// FM Towns and X68000. The parser proves only the container; folder evidence
/// is still required before discovery can accept a platform assignment.
fn discover_d88_image(path: &Path, source_root: &Path) -> GameDiscovery {
    use crate::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};

    let evidence = inspect_disk_format(
        path,
        &crate::safe_read::TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    let detail = evidence
        .evidence
        .first()
        .cloned()
        .unwrap_or_else(|| "D88 disk container".to_string());
    match evidence.format {
        Some(DiskFormat::D88Container) => {
            let identity = identity_for(path, source_root);
            match &identity {
                Some(summary) if summary.platform.is_some() => accepted(
                    path.to_path_buf(),
                    ContainerKind::DirectFile,
                    ContentKind::ComputerDisk,
                    identity,
                    format!(
                        "Valid D88 disk container; platform identified from folder evidence. {detail}."
                    ),
                ),
                _ => GameDiscovery {
                    path: path.to_path_buf(),
                    container: ContainerKind::DirectFile,
                    content: Some(ContentKind::ComputerDisk),
                    platform_hint: None,
                    identity_candidate: identity,
                    validation_state: ValidationState::Skipped,
                    explanation: format!(
                        "Valid D88 disk container, but D88 is shared by PC-88, PC-98, FM Towns and X68000; no platform corroboration was found. {detail}."
                    ),
                    skip_reason: Some(SkipReason::RecognizedContentNoIdentityMatch),
                },
            }
        }
        _ => skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::ComputerDisk),
            SkipReason::InvalidContent(
                evidence
                    .refusal
                    .as_ref()
                    .map(|refusal| refusal.detail())
                    .unwrap_or_else(|| "not a recognised D88 disk container".to_string()),
            ),
            "This file is not a readable D88 disk container.".to_string(),
        ),
    }
}

/// HDI and NHD headers prove only a coherent hard-disk container. They are
/// shared image formats, so platform identity still comes from folder/DAT/hash
/// evidence and never from capacity or the extension.
fn discover_hard_disk_image(path: &Path, source_root: &Path, extension: &str) -> GameDiscovery {
    use crate::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};

    let evidence = inspect_disk_format(
        path,
        &crate::safe_read::TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    let expected = if extension == "hdi" {
        DiskFormat::HdiContainer
    } else {
        DiskFormat::NhdContainer
    };
    let detail = evidence
        .evidence
        .first()
        .cloned()
        .unwrap_or_else(|| format!(".{extension} hard-disk container"));
    match evidence.format {
        Some(format) if format == expected => {
            let identity = identity_for(path, source_root);
            match &identity {
                Some(summary) if summary.platform.is_some() => accepted(
                    path.to_path_buf(),
                    ContainerKind::DirectFile,
                    ContentKind::ComputerDisk,
                    identity,
                    format!(
                        "Valid .{extension} hard-disk container; platform identified from other evidence. {detail}."
                    ),
                ),
                _ => GameDiscovery {
                    path: path.to_path_buf(),
                    container: ContainerKind::DirectFile,
                    content: Some(ContentKind::ComputerDisk),
                    platform_hint: None,
                    identity_candidate: identity,
                    validation_state: ValidationState::Skipped,
                    explanation: format!(
                        "Valid .{extension} hard-disk container, but its header does not prove PC-98 or any other platform. {detail}."
                    ),
                    skip_reason: Some(SkipReason::RecognizedContentNoIdentityMatch),
                },
            }
        }
        _ => skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::ComputerDisk),
            SkipReason::InvalidContent(
                evidence
                    .refusal
                    .as_ref()
                    .map(|refusal| refusal.detail())
                    .unwrap_or_else(|| format!("not a recognised .{extension} container")),
            ),
            format!("This .{extension} file is not a readable hard-disk container."),
        ),
    }
}

/// `.trd` (raw TR-DOS disk image) and `.scl` (SINCLAIR archive of TR-DOS
/// files) are ZX Spectrum-family media. Neither is proven by its extension:
/// resolution runs through the shared structural disk layer, which validates
/// the TR-DOS system descriptor or SINCLAIR archive arithmetic before claiming
/// anything.
fn discover_trdos_media(path: &Path, source_root: &Path) -> GameDiscovery {
    use crate::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};

    let evidence = inspect_disk_format(
        path,
        &crate::safe_read::TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    let detail = evidence
        .evidence
        .first()
        .cloned()
        .unwrap_or_else(|| "TR-DOS media".to_string());

    match evidence.format {
        Some(DiskFormat::SpectrumTrDosDisk) | Some(DiskFormat::SpectrumSclArchive) => {
            GameDiscovery {
                path: path.to_path_buf(),
                container: ContainerKind::DirectFile,
                content: Some(ContentKind::ComputerDisk),
                platform_hint: Some("ZX Spectrum".to_string()),
                identity_candidate: identity_for(path, source_root),
                validation_state: ValidationState::Accepted,
                explanation: format!(
                    "ZX Spectrum TR-DOS media. {detail}. Structural platform/media evidence only; \
                     exact game identity still needs a DAT/catalogue match."
                ),
                skip_reason: None,
            }
        }
        _ => skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::ComputerDisk),
            SkipReason::InvalidContent(
                evidence
                    .refusal
                    .as_ref()
                    .map(|refusal| refusal.detail())
                    .unwrap_or_else(|| "not a recognised TR-DOS disk or SCL archive".to_string()),
            ),
            "This file is named like ZX Spectrum TR-DOS media but its contents did not validate \
             as a TR-DOS disk or an SCL archive."
                .to_string(),
        ),
    }
}

/// `.ssd` and `.dsd` are raw Acorn DFS catalogues. A valid catalogue proves
/// Acorn/BBC-family disk media, but not BBC Micro versus Acorn Electron; a
/// containing folder may provide that corroboration. A bare valid image stays
/// visible but ambiguous, like the shared CPCEMU disk path.
fn discover_dfs_media(path: &Path, source_root: &Path) -> GameDiscovery {
    use crate::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};

    let evidence = inspect_disk_format(
        path,
        &crate::safe_read::TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    let detail = evidence
        .evidence
        .first()
        .cloned()
        .unwrap_or_else(|| "Acorn DFS media".to_string());

    match evidence.format {
        Some(DiskFormat::AcornDfsDisk) => {
            let identity = identity_for(path, source_root);
            match &identity {
                Some(summary) if summary.platform.is_some() => accepted(
                    path.to_path_buf(),
                    ContainerKind::DirectFile,
                    ContentKind::ComputerDisk,
                    identity,
                    format!(
                        "Acorn DFS computer disk ({detail}; BBC-family and Electron media, \
                         platform resolved from folder context)."
                    ),
                ),
                _ => GameDiscovery {
                    path: path.to_path_buf(),
                    container: ContainerKind::DirectFile,
                    content: Some(ContentKind::ComputerDisk),
                    platform_hint: None,
                    identity_candidate: identity,
                    validation_state: ValidationState::Skipped,
                    explanation: format!(
                        "Valid Acorn DFS disk, but DFS is shared by BBC Micro/BBC Master and \
                         Acorn Electron and no folder evidence identifies which. {detail}."
                    ),
                    skip_reason: Some(SkipReason::AmbiguousPlatform),
                },
            }
        }
        _ => skipped(
            path.to_path_buf(),
            ContainerKind::DirectFile,
            Some(ContentKind::ComputerDisk),
            SkipReason::InvalidContent(
                evidence
                    .refusal
                    .as_ref()
                    .map(|refusal| refusal.detail())
                    .unwrap_or_else(|| "not a recognised Acorn DFS disk".to_string()),
            ),
            "This file is named like Acorn DFS media but its catalogue and geometry did not \
             validate."
                .to_string(),
        ),
    }
}

fn identity_for(path: &Path, source_root: &Path) -> Option<IdentitySummary> {
    let metadata = std::fs::metadata(path).ok();
    let identity = ArchiveIdentity::from_path(path, source_root, metadata.as_ref());
    Some(IdentitySummary::from(&identity))
}

fn accepted(
    path: PathBuf,
    container: ContainerKind,
    content: ContentKind,
    identity: Option<IdentitySummary>,
    explanation: String,
) -> GameDiscovery {
    let platform_hint = identity
        .as_ref()
        .and_then(|summary| summary.platform.clone());
    GameDiscovery {
        path,
        container,
        content: Some(content),
        platform_hint,
        identity_candidate: identity,
        validation_state: ValidationState::Accepted,
        explanation,
        skip_reason: None,
    }
}

fn skipped(
    path: PathBuf,
    container: ContainerKind,
    content: Option<ContentKind>,
    reason: SkipReason,
    explanation: String,
) -> GameDiscovery {
    GameDiscovery {
        path,
        container,
        content,
        platform_hint: None,
        identity_candidate: None,
        validation_state: ValidationState::Skipped,
        explanation,
        skip_reason: Some(reason),
    }
}
