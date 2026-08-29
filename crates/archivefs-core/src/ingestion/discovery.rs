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
use crate::amiga_disk::{self, AmigaDisk};
use crate::identity_source::whdload::inspect_whdload_slave_file;
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

/// The full, read-only result of discovering one source folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceDiscoveryReport {
    pub items: Vec<GameDiscovery>,
    pub stats: DiscoveryStats,
    pub skip_reasons: SkipReasonCounts,
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
    if !root.is_dir() {
        return Err(DiscoveryError::NotADirectory);
    }

    let files = walk_bounded(root)?;

    // CUE sheets are resolved first so their referenced `.bin`s are known
    // and excluded from independent classification - see module docs.
    let mut consumed: BTreeSet<PathBuf> = BTreeSet::new();
    let mut items = Vec::new();

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
        items.push(discover_file(path, root));
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

fn discover_file(path: &Path, source_root: &Path) -> GameDiscovery {
    let container = detect_container(path, path.is_dir());
    match &container {
        ContainerKind::Folder(FolderRole::WhdloadInstall) => discover_whdload_folder(path),
        ContainerKind::Folder(FolderRole::ExtractedGame) => discover_extracted_folder(path),
        ContainerKind::Folder(FolderRole::Plain) => unreachable!("plain folders are recursed"),
        ContainerKind::Archive(format) => discover_archive(path, *format, source_root),
        ContainerKind::DirectFile => discover_direct_file(path, source_root),
    }
}

fn discover_whdload_folder(path: &Path) -> GameDiscovery {
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
    match inspect_whdload_slave_file(first_slave) {
        Ok(_artifact) => accepted(
            path.to_path_buf(),
            ContainerKind::Folder(FolderRole::WhdloadInstall),
            ContentKind::WhdloadInstall,
            None,
            format!("WHDLoad install ({} slave file(s) found).", slaves.len()),
        ),
        Err(error) => skipped(
            path.to_path_buf(),
            ContainerKind::Folder(FolderRole::WhdloadInstall),
            Some(ContentKind::WhdloadInstall),
            SkipReason::InvalidContent(format!("{error:?}")),
            "This folder has a .slave file, but it could not be read as a valid WHDLoad slave."
                .to_string(),
        ),
    }
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

fn discover_direct_file(path: &Path, source_root: &Path) -> GameDiscovery {
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
    if matches!(extension.as_str(), "hdf" | "hdfx") {
        return discover_ambiguous_disk_image(path, source_root);
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
