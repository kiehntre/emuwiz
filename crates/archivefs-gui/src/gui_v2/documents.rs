//! Local manuals and guide discovery for GUI v2.
//!
//! This is deliberately a read-only foundation. Discovery is bounded to the
//! selected game's directory, a few conventional child directories, and
//! explicitly configured document roots. No document is downloaded,
//! extracted, rewritten, or deleted by this module.

use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use archivefs_core::manual_document::{ManualLimits, ManualViewerAction, inspect_manual};
use serde::{Deserialize, Serialize};

const MAX_ROOTS: usize = 16;
const MAX_FILES_PER_DIRECTORY: usize = 512;
const MAX_DOCUMENTS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GameDocumentKind {
    #[default]
    Manual,
    StrategyGuide,
    ReferenceCard,
    Map,
    Magazine,
    Walkthrough,
    Other,
}

impl GameDocumentKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::StrategyGuide => "Strategy guide",
            Self::ReferenceCard => "Reference card",
            Self::Map => "Map",
            Self::Magazine => "Magazine",
            Self::Walkthrough => "Walkthrough",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GameDocumentFormat {
    Pdf,
    Cbz,
    Cbr,
    Unknown,
}

impl GameDocumentFormat {
    fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("pdf") => Self::Pdf,
            Some("cbz") => Self::Cbz,
            Some("cbr") => Self::Cbr,
            _ => Self::Unknown,
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Cbz => "CBZ",
            Self::Cbr => "CBR",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GameDocumentSource {
    NearbyGameDirectory,
    ConfiguredRoot(PathBuf),
    ExplicitAssociation,
    /// Provenance for a document reached through a RomM-provided manual
    /// reference, safely mapped to a user-trusted local root (never a
    /// remote URL). Carries that local root for display/explanation only;
    /// it does not change opening/resume behavior relative to a document
    /// discovered locally.
    Romm(PathBuf),
}

/// Whether "Open" can actually do something useful right now, established
/// by a bounded, non-executing probe (never opens the file itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DocumentOpenCapability {
    /// A local OS opener for this format was found on `PATH` (or is a
    /// built-in OS shell association on Windows/macOS).
    Supported,
    /// The format is externally-openable in principle, but no handler was
    /// found on this machine.
    NoHandler,
    /// The format is recognised but EmuWiz does not offer opening it yet
    /// (e.g. CBR).
    UnsupportedFormat,
    /// The document's file no longer exists at its resolved path.
    MissingFile,
}

/// Strongest evidence tying a document to exactly one already-verified
/// game identity, reusing whatever the rest of the codebase already
/// computed (see `archivefs_core::launch::planning::ResolvedIdentity`).
/// This module never derives identity itself - it only carries evidence a
/// caller already resolved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ExactIdentityEvidence {
    pub(crate) platform_id: String,
    pub(crate) game_key: String,
}

impl From<&archivefs_core::launch::planning::ResolvedIdentity> for ExactIdentityEvidence {
    fn from(identity: &archivefs_core::launch::planning::ResolvedIdentity) -> Self {
        Self {
            platform_id: identity.platform_id.clone(),
            game_key: identity.game_key.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub(crate) enum GameDocumentAssociation {
    Explicit,
    ExactGameIdentity(ExactIdentityEvidence),
    SameGameDirectory,
    ExactTitle,
    PlatformAndTitle,
    WeakFilename,
    Unmatched,
}

impl GameDocumentAssociation {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Explicit => "Explicit user association",
            Self::ExactGameIdentity(_) => "Exact stored game identity",
            Self::SameGameDirectory => "Same game directory",
            Self::ExactTitle => "Exact title match",
            Self::PlatformAndTitle => "Platform and title match",
            Self::WeakFilename => "Weak filename hint",
            Self::Unmatched => "Not associated",
        }
    }
}

/// Novice-facing, typed reasons a document cannot be shown/opened right
/// now. Technical detail (paths, underlying error causes) stays available
/// separately via [`DocumentUnavailableReason::technical_detail`] rather
/// than being folded into the user-facing message.
#[allow(
    dead_code,
    reason = "NoApplicationToOpen/FormatNotYetSupported are reserved for GUI wiring beyond this backend-only change"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DocumentUnavailableReason {
    NoLongerAvailable,
    CouldNotMapRommDocument(String),
    NoApplicationToOpen,
    AmbiguousMatch,
    FormatNotYetSupported,
}

#[allow(
    dead_code,
    reason = "novice-facing message/detail split is reserved for GUI wiring beyond this backend-only change"
)]
impl DocumentUnavailableReason {
    pub(crate) fn user_message(&self) -> &'static str {
        match self {
            Self::NoLongerAvailable => "This manual is no longer available.",
            Self::CouldNotMapRommDocument(_) => {
                "EmuWiz could not safely map this RomM document to a local file."
            }
            Self::NoApplicationToOpen => "No application is available to open this document.",
            Self::AmbiguousMatch => "More than one manual matches this game equally well.",
            Self::FormatNotYetSupported => {
                "This document format is recognised but cannot be opened yet."
            }
        }
    }

    pub(crate) fn technical_detail(&self) -> Option<&str> {
        match self {
            Self::CouldNotMapRommDocument(detail) => Some(detail),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GameDocument {
    pub(crate) path: PathBuf,
    pub(crate) format: GameDocumentFormat,
    pub(crate) kind: GameDocumentKind,
    pub(crate) title: String,
    pub(crate) platform: Option<String>,
    pub(crate) game_id: Option<i64>,
    pub(crate) source: GameDocumentSource,
    pub(crate) association: GameDocumentAssociation,
    pub(crate) association_reason: String,
    pub(crate) page_count: Option<usize>,
    pub(crate) file_size: u64,
    pub(crate) viewer: DocumentOpenCapability,
}

impl GameDocument {
    pub(crate) fn source_label(&self) -> String {
        match &self.source {
            GameDocumentSource::NearbyGameDirectory => "nearby game directory".to_string(),
            GameDocumentSource::ConfiguredRoot(root) => {
                format!("configured root: {}", root.display())
            }
            GameDocumentSource::ExplicitAssociation => "explicit user association".to_string(),
            GameDocumentSource::Romm(root) => {
                format!("RomM · mapped to local root: {}", root.display())
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DocumentReadingState {
    pub(crate) last_page: Option<usize>,
    pub(crate) zoom_percent: Option<u16>,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerInput {
    Confirm,
    Back,
    PreviousPage,
    NextPage,
    PreviousJump,
    NextJump,
    Pan,
    ZoomIn,
    ZoomOut,
    Menu,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerCommand {
    Open,
    Return,
    PreviousPage,
    NextPage,
    PreviousJump,
    NextJump,
    Pan,
    ZoomIn,
    ZoomOut,
    ToggleControls,
}

impl ViewerCommand {
    /// The canonical viewer action this command drives, where one exists.
    /// `Open`, the page jumps, `Pan` and the controls toggle have no effect on
    /// `ManualViewerState` yet, so they return `None` rather than guessing.
    #[allow(
        dead_code,
        reason = "reserved for the embedded viewer surface, which is not routed yet"
    )]
    pub(crate) fn viewer_action(self) -> Option<ManualViewerAction> {
        match self {
            Self::PreviousPage => Some(ManualViewerAction::PreviousPage),
            Self::NextPage => Some(ManualViewerAction::NextPage),
            Self::ZoomIn => Some(ManualViewerAction::ZoomIn),
            Self::ZoomOut => Some(ManualViewerAction::ZoomOut),
            Self::Return => Some(ManualViewerAction::Close),
            Self::Open | Self::PreviousJump | Self::NextJump | Self::Pan | Self::ToggleControls => {
                None
            }
        }
    }
}

#[allow(dead_code)]
pub(crate) fn map_viewer_input(input: ViewerInput) -> ViewerCommand {
    match input {
        ViewerInput::Confirm => ViewerCommand::Open,
        ViewerInput::Back => ViewerCommand::Return,
        ViewerInput::PreviousPage => ViewerCommand::PreviousPage,
        ViewerInput::NextPage => ViewerCommand::NextPage,
        ViewerInput::PreviousJump => ViewerCommand::PreviousJump,
        ViewerInput::NextJump => ViewerCommand::NextJump,
        ViewerInput::Pan => ViewerCommand::Pan,
        ViewerInput::ZoomIn => ViewerCommand::ZoomIn,
        ViewerInput::ZoomOut => ViewerCommand::ZoomOut,
        ViewerInput::Menu => ViewerCommand::ToggleControls,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DocumentPreferences {
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) associations: BTreeMap<PathBuf, i64>,
    pub(crate) reading: BTreeMap<PathBuf, DocumentReadingState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DocumentDiscoveryRequest<'a> {
    pub(crate) game_id: i64,
    pub(crate) game_title: &'a str,
    pub(crate) platform: &'a str,
    pub(crate) game_path: &'a Path,
    pub(crate) roots: &'a [PathBuf],
    pub(crate) associations: &'a BTreeMap<PathBuf, i64>,
    /// Already-resolved, strongest-tier identity evidence for this exact
    /// game, exactly as the rest of the codebase computed it (never
    /// derived here). When present, a document found in the game's own
    /// directory is promoted from the heuristic `SameGameDirectory` tier
    /// to `ExactGameIdentity`, since it is known to live alongside a
    /// verified - not just filename-matched - game.
    pub(crate) verified_identity: Option<&'a archivefs_core::launch::planning::ResolvedIdentity>,
}

pub(crate) fn discover_documents(request: DocumentDiscoveryRequest<'_>) -> Vec<GameDocument> {
    let game_directory = if request.game_path.is_dir() {
        request.game_path.to_path_buf()
    } else {
        request
            .game_path
            .parent()
            .unwrap_or(request.game_path)
            .to_path_buf()
    };
    let mut candidates: BTreeMap<PathBuf, GameDocument> = BTreeMap::new();
    let mut directories = vec![(
        game_directory.clone(),
        GameDocumentSource::NearbyGameDirectory,
    )];
    for name in ["manuals", "docs", "guides"] {
        directories.push((
            game_directory.join(name),
            GameDocumentSource::NearbyGameDirectory,
        ));
    }
    for root in request.roots.iter().take(MAX_ROOTS) {
        directories.push((
            root.clone(),
            GameDocumentSource::ConfiguredRoot(root.clone()),
        ));
    }
    for (directory, source) in directories {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_document_path(path))
            .take(MAX_FILES_PER_DIRECTORY)
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let canonical = fs::canonicalize(&path).unwrap_or(path.clone());
            if !is_within_allowed_root(&canonical, &directory)
                && source != GameDocumentSource::NearbyGameDirectory
            {
                continue;
            }
            let normalized_file = normalize_title(
                path.file_stem()
                    .and_then(|v| v.to_str())
                    .unwrap_or_default(),
            );
            let normalized_game = normalize_title(request.game_title);
            let explicit = request
                .associations
                .iter()
                .find_map(|(associated_path, id)| {
                    (id == &request.game_id
                        && fs::canonicalize(associated_path).ok().as_ref() == Some(&canonical))
                    .then_some(())
                })
                .is_some();
            let same_directory =
                canonical.parent() == game_directory.canonicalize().ok().as_deref();
            let exact_title = !normalized_game.is_empty() && normalized_file == normalized_game;
            let platform_title = exact_title
                && path
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(&request.platform.to_ascii_lowercase());
            let association = if explicit {
                GameDocumentAssociation::Explicit
            } else if same_directory && request.verified_identity.is_some() {
                GameDocumentAssociation::ExactGameIdentity(ExactIdentityEvidence::from(
                    request
                        .verified_identity
                        .expect("checked Some above via is_some()"),
                ))
            } else if same_directory {
                GameDocumentAssociation::SameGameDirectory
            } else if platform_title {
                GameDocumentAssociation::PlatformAndTitle
            } else if exact_title {
                GameDocumentAssociation::ExactTitle
            } else if normalized_file.contains(&normalized_game)
                || normalized_game.contains(&normalized_file)
            {
                GameDocumentAssociation::WeakFilename
            } else {
                GameDocumentAssociation::Unmatched
            };
            if matches!(
                association,
                GameDocumentAssociation::WeakFilename | GameDocumentAssociation::Unmatched
            ) {
                continue;
            }
            let format = GameDocumentFormat::from_path(&canonical);
            let (page_count, viewer) = inspect_capability(&canonical, format);
            let title = path
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("Untitled document")
                .replace(['_', '-'], " ");
            let association_reason = association.label().to_string();
            candidates.entry(canonical.clone()).or_insert(GameDocument {
                path: canonical,
                format,
                kind: infer_kind(&title),
                title,
                platform: Some(request.platform.to_string()),
                game_id: Some(request.game_id),
                source: if explicit {
                    GameDocumentSource::ExplicitAssociation
                } else {
                    source.clone()
                },
                association,
                association_reason,
                page_count,
                file_size: metadata.len(),
                viewer,
            });
        }
    }
    let mut documents = candidates.into_values().collect::<Vec<_>>();
    sort_documents(&mut documents);
    documents.truncate(MAX_DOCUMENTS);
    documents
}

fn sort_documents(documents: &mut [GameDocument]) {
    documents.sort_by_key(|document| {
        (
            document.association.clone(),
            normalize_title(&document.title),
            document.path.clone(),
        )
    });
}

fn is_document_path(path: &Path) -> bool {
    matches!(
        GameDocumentFormat::from_path(path),
        GameDocumentFormat::Pdf | GameDocumentFormat::Cbz | GameDocumentFormat::Cbr
    )
}

fn is_within_allowed_root(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn normalize_title(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn infer_kind(title: &str) -> GameDocumentKind {
    let lower = title.to_ascii_lowercase();
    if lower.contains("strategy") || lower.contains("prima") || lower.contains("guide") {
        GameDocumentKind::StrategyGuide
    } else if lower.contains("map") {
        GameDocumentKind::Map
    } else if lower.contains("reference") || lower.contains("card") {
        GameDocumentKind::ReferenceCard
    } else if lower.contains("magazine") || lower.contains("monthly") {
        GameDocumentKind::Magazine
    } else if lower.contains("walkthrough") || lower.contains("hint") {
        GameDocumentKind::Walkthrough
    } else if lower.contains("manual") || lower.contains("instruction") {
        GameDocumentKind::Manual
    } else {
        GameDocumentKind::Other
    }
}

fn inspect_capability(
    path: &Path,
    format: GameDocumentFormat,
) -> (Option<usize>, DocumentOpenCapability) {
    // One canonical inspector decides what a file is and how many pages it has
    // (`archivefs_core::manual_document`); discovery only asks it. It is bounded
    // and read-only, and trusts content signatures over the extension.
    let page_count = match format {
        GameDocumentFormat::Pdf | GameDocumentFormat::Cbz => {
            inspect_manual(path, &ManualLimits::default())
                .ok()
                .and_then(|inspection| inspection.page_count)
        }
        GameDocumentFormat::Cbr | GameDocumentFormat::Unknown => None,
    };
    (
        page_count,
        document_open_capability_with(path, format, external_handler_available()),
    )
}

/// The OS-opener program EmuWiz already uses
/// (`archivefs_core::identity_source::romm::manual::DesktopManualOpener`),
/// duplicated here only as a name for the PATH probe below - the actual
/// spawn always goes through that shared opener, never through this
/// module.
fn opener_program_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Pure, bounded PATH probe: does `program` exist as a file in any
/// directory listed in `path_value`? Never spawns or opens anything.
fn handler_on_path(path_value: Option<&std::ffi::OsStr>, program: &str) -> bool {
    path_value
        .is_some_and(|paths| std::env::split_paths(paths).any(|dir| dir.join(program).is_file()))
}

/// Whether an OS handler for the current opener program is actually
/// available. Windows/macOS ship a built-in shell association
/// (`explorer`/`open`) so they are not PATH-probed; other platforms are
/// probed for `xdg-open` on `PATH`. This never spawns a process - it only
/// checks for the executable's presence.
fn external_handler_available() -> bool {
    if cfg!(target_os = "windows") || cfg!(target_os = "macos") {
        return true;
    }
    handler_on_path(std::env::var_os("PATH").as_deref(), opener_program_name())
}

/// General open-capability projection: applies to any externally-opened
/// format (PDF, CBZ, future formats), not just CBZ. CBR/Unknown are always
/// `UnsupportedFormat`, independent of file existence or handler
/// availability, matching the "recognised but blocked" policy.
fn document_open_capability_with(
    path: &Path,
    format: GameDocumentFormat,
    handler_available: bool,
) -> DocumentOpenCapability {
    match format {
        GameDocumentFormat::Cbr | GameDocumentFormat::Unknown => {
            DocumentOpenCapability::UnsupportedFormat
        }
        GameDocumentFormat::Pdf | GameDocumentFormat::Cbz => {
            if !path.is_file() {
                DocumentOpenCapability::MissingFile
            } else if handler_available {
                DocumentOpenCapability::Supported
            } else {
                DocumentOpenCapability::NoHandler
            }
        }
    }
}

/// Request to project a RomM-provided manual reference into the local
/// document model. Reuses the existing, already-tested RomM path-safety
/// policy (`resolve_local_romm_manual`) unchanged - this module never
/// invents its own trust-root or traversal logic.
#[allow(dead_code)]
pub(crate) struct RommDocumentRequest<'a> {
    pub(crate) game_id: i64,
    pub(crate) platform: &'a str,
    pub(crate) mapping:
        Option<&'a archivefs_core::identity_source::romm::media_mapping::ValidatedRommMediaMapping>,
    pub(crate) manual: &'a archivefs_core::identity_source::model::MediaReference,
    pub(crate) verified_identity: Option<&'a archivefs_core::launch::planning::ResolvedIdentity>,
}

/// Projects a RomM manual reference into the same [`GameDocument`] model
/// used for local documents, so there is one document/association/
/// capability model rather than two parallel manual features. Never
/// fetches anything remote and never opens the file - only maps and
/// inspects it, exactly like local discovery does.
#[allow(dead_code)]
pub(crate) fn project_romm_manual_document(
    request: RommDocumentRequest<'_>,
) -> Result<GameDocument, DocumentUnavailableReason> {
    use archivefs_core::identity_source::romm::manual::{
        RommManualRefusal, resolve_local_romm_manual,
    };

    let local_root = request
        .mapping
        .map(|mapping| mapping.local_root().to_path_buf());
    let path =
        resolve_local_romm_manual(request.mapping, request.manual).map_err(
            |refusal| match refusal {
                RommManualRefusal::Unavailable => DocumentUnavailableReason::NoLongerAvailable,
                other => DocumentUnavailableReason::CouldNotMapRommDocument(other.to_string()),
            },
        )?;
    if !path.is_file() {
        return Err(DocumentUnavailableReason::NoLongerAvailable);
    }
    let format = GameDocumentFormat::from_path(&path);
    let (page_count, viewer) = inspect_capability(&path, format);
    let association = match request.verified_identity {
        Some(identity) => {
            GameDocumentAssociation::ExactGameIdentity(ExactIdentityEvidence::from(identity))
        }
        None => GameDocumentAssociation::PlatformAndTitle,
    };
    let association_reason = association.label().to_string();
    let title = path
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("Untitled document")
        .replace(['_', '-'], " ");
    let file_size = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    Ok(GameDocument {
        kind: infer_kind(&title),
        title,
        platform: Some(request.platform.to_string()),
        game_id: Some(request.game_id),
        source: GameDocumentSource::Romm(local_root.unwrap_or_default()),
        association,
        association_reason,
        page_count,
        file_size,
        viewer,
        path,
        format,
    })
}

/// Merges a RomM-projected document into an already-discovered local list,
/// keyed by canonical path so the same underlying file never appears
/// twice merely because it was reached through two provenances. Keeps
/// the list sorted by association strength.
#[allow(dead_code)]
pub(crate) fn merge_romm_document(documents: &mut Vec<GameDocument>, romm_document: GameDocument) {
    if documents
        .iter()
        .any(|existing| existing.path == romm_document.path)
    {
        return;
    }
    documents.push(romm_document);
    sort_documents(documents);
}

/// Picks a single "the manual" document, refusing to silently choose
/// between two matches that tie at the strongest identity-bearing tier.
/// Weaker ties (e.g. two `SameGameDirectory` matches) are not ambiguity
/// errors here since callers of this function only care about the
/// strongest-evidence pick; the full, un-collapsed list remains available
/// via [`discover_documents`] for display.
#[allow(dead_code)]
pub(crate) fn resolve_strongest_document(
    documents: &[GameDocument],
) -> Result<Option<&GameDocument>, DocumentUnavailableReason> {
    let Some(strongest) = documents.iter().map(|document| &document.association).min() else {
        return Ok(None);
    };
    let mut at_strongest = documents
        .iter()
        .filter(|document| &document.association == strongest);
    let first = at_strongest.next();
    if matches!(strongest, GameDocumentAssociation::ExactGameIdentity(_))
        && at_strongest.next().is_some()
    {
        return Err(DocumentUnavailableReason::AmbiguousMatch);
    }
    Ok(first)
}

pub(crate) fn safe_resume_page(
    state: Option<&DocumentReadingState>,
    page_count: Option<usize>,
) -> Option<usize> {
    let page = state.and_then(|state| state.last_page)?;
    (page > 0 && page_count.is_none_or(|count| page <= count)).then_some(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn cbz(path: &Path, names: &[&str]) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for name in names {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"page").unwrap();
        }
        writer.finish().unwrap();
    }

    /// Page names as the canonical inspector reports them.
    fn inspect_cbz(path: &Path) -> Result<Vec<String>, String> {
        inspect_manual(path, &ManualLimits::default())
            .map(|inspection| inspection.pages.into_iter().map(|p| p.name).collect())
            .map_err(|error| error.to_string())
    }

    /// A structurally valid one-section PDF with `pages` declared pages.
    fn minimal_pdf(pages: usize) -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Kids [3 0 R] /Count {pages} >>"),
            "<< /Type /Page /Parent 2 0 R >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(
            format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").bytes(),
        );
        out
    }

    #[test]
    fn cbz_pages_use_natural_order() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("guide.cbz");
        cbz(&path, &["page10.png", "page2.png", "page1.png"]);
        assert_eq!(
            inspect_cbz(&path).unwrap(),
            ["page1.png", "page2.png", "page10.png"]
        );
    }

    #[test]
    fn discovery_page_counts_come_from_the_canonical_inspector() {
        let dir = tempdir().unwrap();
        let cbz_path = dir.path().join("guide.cbz");
        cbz(&cbz_path, &["1.png", "2.png", "3.png"]);
        let pdf_path = dir.path().join("manual.pdf");
        fs::write(&pdf_path, minimal_pdf(5)).unwrap();
        assert_eq!(
            inspect_capability(&cbz_path, GameDocumentFormat::Cbz).0,
            Some(3)
        );
        assert_eq!(
            inspect_capability(&pdf_path, GameDocumentFormat::Pdf).0,
            Some(5)
        );
        // A byte-scan lookalike is no longer mistaken for a countable PDF.
        let fake = dir.path().join("fake.pdf");
        fs::write(&fake, b"%PDF /Type /Pages /Count 2 /Type /Page\n").unwrap();
        assert_eq!(inspect_capability(&fake, GameDocumentFormat::Pdf).0, None);
    }

    #[test]
    fn viewer_commands_map_onto_the_canonical_actions() {
        use ViewerCommand as C;
        assert_eq!(
            C::NextPage.viewer_action(),
            Some(ManualViewerAction::NextPage)
        );
        assert_eq!(
            C::PreviousPage.viewer_action(),
            Some(ManualViewerAction::PreviousPage)
        );
        assert_eq!(C::ZoomIn.viewer_action(), Some(ManualViewerAction::ZoomIn));
        assert_eq!(
            C::ZoomOut.viewer_action(),
            Some(ManualViewerAction::ZoomOut)
        );
        assert_eq!(C::Return.viewer_action(), Some(ManualViewerAction::Close));
        for command in [
            C::Open,
            C::PreviousJump,
            C::NextJump,
            C::Pan,
            C::ToggleControls,
        ] {
            assert_eq!(command.viewer_action(), None, "{command:?}");
        }
        // Every generic input reaches a command, and none panics.
        for input in [
            ViewerInput::Confirm,
            ViewerInput::Back,
            ViewerInput::PreviousPage,
            ViewerInput::NextPage,
            ViewerInput::PreviousJump,
            ViewerInput::NextJump,
            ViewerInput::Pan,
            ViewerInput::ZoomIn,
            ViewerInput::ZoomOut,
            ViewerInput::Menu,
        ] {
            let _ = map_viewer_input(input).viewer_action();
        }
    }
    #[test]
    fn unsafe_cbz_entries_are_refused() {
        let dir = tempdir().unwrap();
        for name in ["../escape.png", "/absolute.png"] {
            let path = dir.path().join("bad.cbz");
            cbz(&path, &[name]);
            assert!(inspect_cbz(&path).is_err());
        }
    }
    #[test]
    fn malformed_cbz_and_unsupported_cbr_are_safe() {
        let dir = tempdir().unwrap();
        let bad = dir.path().join("bad.cbz");
        fs::write(&bad, b"not zip").unwrap();
        assert!(inspect_cbz(&bad).is_err());
        assert_eq!(
            inspect_capability(&dir.path().join("x.cbr"), GameDocumentFormat::Cbr).1,
            DocumentOpenCapability::UnsupportedFormat
        );
    }
    #[test]
    fn pdf_discovery_and_page_count_are_local_and_read_only() {
        let dir = tempdir().unwrap();
        let game = dir.path().join("game.bin");
        fs::write(&game, b"game").unwrap();
        let manual = dir.path().join("Sonic Manual.pdf");
        fs::write(&manual, minimal_pdf(2)).unwrap();
        let before = fs::read(&manual).unwrap();
        let docs = discover_documents(DocumentDiscoveryRequest {
            game_id: 4,
            game_title: "Sonic",
            platform: "Saturn",
            game_path: &game,
            roots: &[],
            associations: &BTreeMap::new(),
            verified_identity: None,
        });
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].format, GameDocumentFormat::Pdf);
        assert_eq!(docs[0].page_count, Some(2));
        assert_eq!(fs::read(&manual).unwrap(), before);
    }
    #[test]
    fn explicit_association_beats_weak_filename_matching() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("Sonic guide.pdf");
        fs::write(&file, b"%PDF").unwrap();
        let mut associations = BTreeMap::new();
        associations.insert(file.clone(), 7);
        let docs = discover_documents(DocumentDiscoveryRequest {
            game_id: 7,
            game_title: "Sonic",
            platform: "Saturn",
            game_path: &dir.path().join("game.bin"),
            roots: &[],
            associations: &associations,
            verified_identity: None,
        });
        assert_eq!(docs[0].association, GameDocumentAssociation::Explicit);
    }
    #[test]
    fn weak_ambiguous_title_is_not_auto_linked() {
        let dir = tempdir().unwrap();
        let docs_root = dir.path().join("docs");
        fs::create_dir(&docs_root).unwrap();
        let file = docs_root.join("Sonic bonus.pdf");
        fs::write(&file, b"%PDF").unwrap();
        let game_dir = dir.path().join("game");
        fs::create_dir(&game_dir).unwrap();
        let game_path = game_dir.join("game.bin");
        let docs = discover_documents(DocumentDiscoveryRequest {
            game_id: 7,
            game_title: "Sonic",
            platform: "Saturn",
            game_path: &game_path,
            roots: &[docs_root],
            associations: &BTreeMap::new(),
            verified_identity: None,
        });
        assert!(docs.is_empty());
    }
    #[test]
    fn changed_document_invalidates_resume_page() {
        let state = DocumentReadingState {
            last_page: Some(9),
            zoom_percent: None,
        };
        assert_eq!(safe_resume_page(Some(&state), Some(8)), None);
        assert_eq!(safe_resume_page(Some(&state), Some(9)), Some(9));
    }
    #[test]
    fn controller_mapping_is_vendor_neutral() {
        assert_eq!(map_viewer_input(ViewerInput::Confirm), ViewerCommand::Open);
        assert_eq!(
            map_viewer_input(ViewerInput::PreviousPage),
            ViewerCommand::PreviousPage
        );
        assert_eq!(
            map_viewer_input(ViewerInput::Menu),
            ViewerCommand::ToggleControls
        );
    }

    use archivefs_core::identity_source::model::MediaReference;
    use archivefs_core::identity_source::romm::media_mapping::{
        RommMediaMapping, validate_romm_media_mapping,
    };
    use archivefs_core::launch::planning::ResolvedIdentity;

    fn identity() -> ResolvedIdentity {
        ResolvedIdentity {
            platform_id: "saturn".to_string(),
            game_key: "MK-81088".to_string(),
        }
    }

    // 1. local exact GameId association
    #[test]
    fn local_exact_identity_association_is_produced() {
        let dir = tempdir().unwrap();
        let game_dir = dir.path().join("game");
        fs::create_dir(&game_dir).unwrap();
        let game = game_dir.join("game.bin");
        fs::write(&game, b"game").unwrap();
        let manual = game_dir.join("readme.pdf");
        fs::write(&manual, b"%PDF").unwrap();
        let evidence = identity();
        let docs = discover_documents(DocumentDiscoveryRequest {
            game_id: 4,
            game_title: "Sonic",
            platform: "Saturn",
            game_path: &game,
            roots: &[],
            associations: &BTreeMap::new(),
            verified_identity: Some(&evidence),
        });
        assert_eq!(docs.len(), 1);
        assert!(matches!(
            docs[0].association,
            GameDocumentAssociation::ExactGameIdentity(_)
        ));
    }

    // 2. exact identity beats title match
    #[test]
    fn exact_identity_outranks_exact_title_match() {
        let dir = tempdir().unwrap();
        let game_dir = dir.path().join("game");
        fs::create_dir(&game_dir).unwrap();
        let title_root = dir.path().join("titles");
        fs::create_dir(&title_root).unwrap();
        let game = game_dir.join("game.bin");
        fs::write(&game, b"game").unwrap();
        let in_directory = game_dir.join("readme.pdf");
        fs::write(&in_directory, b"%PDF").unwrap();
        let title_matched = title_root.join("Sonic.pdf");
        fs::write(&title_matched, b"%PDF").unwrap();
        let evidence = identity();
        let docs = discover_documents(DocumentDiscoveryRequest {
            game_id: 4,
            game_title: "Sonic",
            platform: "Saturn",
            game_path: &game,
            roots: &[title_root],
            associations: &BTreeMap::new(),
            verified_identity: Some(&evidence),
        });
        assert!(matches!(
            docs[0].association,
            GameDocumentAssociation::ExactGameIdentity(_)
        ));
        assert!(
            docs.iter()
                .any(|document| document.association == GameDocumentAssociation::ExactTitle)
        );
    }

    fn romm_mapping(
        root: &Path,
    ) -> archivefs_core::identity_source::romm::media_mapping::ValidatedRommMediaMapping {
        validate_romm_media_mapping(&RommMediaMapping {
            provider_prefix: "/assets/romm/resources".to_string(),
            local_root: root.to_path_buf(),
        })
        .unwrap()
    }

    fn romm_manual(reference: &str) -> MediaReference {
        MediaReference {
            hosted_reference: Some(reference.to_string()),
            public_reference: None,
        }
    }

    // 3. RomM local trusted-root document maps into common model
    #[test]
    fn romm_document_maps_into_common_model() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("manual.pdf"), b"%PDF").unwrap();
        let mapping = romm_mapping(root.path());
        let manual = romm_manual("/assets/romm/resources/manual.pdf");
        let document = project_romm_manual_document(RommDocumentRequest {
            game_id: 9,
            platform: "Saturn",
            mapping: Some(&mapping),
            manual: &manual,
            verified_identity: None,
        })
        .unwrap();
        assert_eq!(document.format, GameDocumentFormat::Pdf);
        assert!(matches!(document.source, GameDocumentSource::Romm(_)));
        assert_eq!(
            document.association,
            GameDocumentAssociation::PlatformAndTitle
        );
    }

    // 4. RomM path outside trusted root refused
    #[test]
    fn romm_document_outside_trusted_root_is_refused() {
        let root = tempdir().unwrap();
        let mapping = romm_mapping(root.path());
        let manual = romm_manual("/assets/other/manual.pdf");
        let result = project_romm_manual_document(RommDocumentRequest {
            game_id: 9,
            platform: "Saturn",
            mapping: Some(&mapping),
            manual: &manual,
            verified_identity: None,
        });
        assert!(matches!(
            result,
            Err(DocumentUnavailableReason::CouldNotMapRommDocument(_))
        ));
    }

    // 5. RomM traversal refused
    #[test]
    fn romm_document_traversal_is_refused() {
        let root = tempdir().unwrap();
        let mapping = romm_mapping(root.path());
        let manual = romm_manual("/assets/romm/resources/../escape.pdf");
        let result = project_romm_manual_document(RommDocumentRequest {
            game_id: 9,
            platform: "Saturn",
            mapping: Some(&mapping),
            manual: &manual,
            verified_identity: None,
        });
        assert!(matches!(
            result,
            Err(DocumentUnavailableReason::CouldNotMapRommDocument(_))
        ));
    }

    // 6. same local document via RomM/local provenance does not duplicate identity
    #[test]
    fn merge_romm_document_avoids_duplicate_identity() {
        let dir = tempdir().unwrap();
        let game_dir = dir.path().join("game");
        fs::create_dir(&game_dir).unwrap();
        let game = game_dir.join("game.bin");
        fs::write(&game, b"game").unwrap();
        let manual_path = game_dir.join("readme.pdf");
        fs::write(&manual_path, b"%PDF").unwrap();
        let mut docs = discover_documents(DocumentDiscoveryRequest {
            game_id: 4,
            game_title: "Sonic",
            platform: "Saturn",
            game_path: &game,
            roots: &[],
            associations: &BTreeMap::new(),
            verified_identity: None,
        });
        assert_eq!(docs.len(), 1);
        let mapping = romm_mapping(&game_dir);
        let manual = romm_manual("/assets/romm/resources/readme.pdf");
        let romm_document = project_romm_manual_document(RommDocumentRequest {
            game_id: 4,
            platform: "Saturn",
            mapping: Some(&mapping),
            manual: &manual,
            verified_identity: None,
        })
        .unwrap();
        merge_romm_document(&mut docs, romm_document);
        assert_eq!(docs.len(), 1, "same canonical path must not duplicate");
    }

    // 7. ambiguous exact matches refused
    #[test]
    fn resolve_strongest_document_refuses_ambiguous_exact_matches() {
        let dir = tempdir().unwrap();
        let evidence = identity();
        let one = GameDocument {
            path: dir.path().join("a.pdf"),
            format: GameDocumentFormat::Pdf,
            kind: GameDocumentKind::Manual,
            title: "A".to_string(),
            platform: Some("Saturn".to_string()),
            game_id: Some(4),
            source: GameDocumentSource::NearbyGameDirectory,
            association: GameDocumentAssociation::ExactGameIdentity(ExactIdentityEvidence::from(
                &evidence,
            )),
            association_reason: String::new(),
            page_count: None,
            file_size: 0,
            viewer: DocumentOpenCapability::Supported,
        };
        let mut two = one.clone();
        two.path = dir.path().join("b.pdf");
        let candidates = [one, two];
        let result = resolve_strongest_document(&candidates);
        assert!(matches!(
            result,
            Err(DocumentUnavailableReason::AmbiguousMatch)
        ));
    }

    // 8 & 9. PDF/CBZ external open capability
    #[test]
    fn pdf_and_cbz_report_handler_backed_capability() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("a.pdf");
        fs::write(&pdf, b"%PDF").unwrap();
        assert_eq!(
            document_open_capability_with(&pdf, GameDocumentFormat::Pdf, true),
            DocumentOpenCapability::Supported
        );
        assert_eq!(
            document_open_capability_with(&pdf, GameDocumentFormat::Pdf, false),
            DocumentOpenCapability::NoHandler
        );
        let cbz = dir.path().join("a.cbz");
        fs::write(&cbz, b"PK").unwrap();
        assert_eq!(
            document_open_capability_with(&cbz, GameDocumentFormat::Cbz, true),
            DocumentOpenCapability::Supported
        );
        assert_eq!(
            document_open_capability_with(&cbz, GameDocumentFormat::Cbz, false),
            DocumentOpenCapability::NoHandler
        );
    }

    // 10. no xdg-open/handler disables Open
    #[test]
    fn handler_probe_is_pure_and_never_executes() {
        let dir = tempdir().unwrap();
        let path_var = std::ffi::OsString::from(dir.path());
        assert!(!handler_on_path(Some(&path_var), "xdg-open"));
        fs::write(dir.path().join("xdg-open"), b"#!/bin/sh\n").unwrap();
        assert!(handler_on_path(Some(&path_var), "xdg-open"));
    }

    // 11. missing document
    #[test]
    fn missing_local_file_reports_missing_capability() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("gone.pdf");
        assert_eq!(
            document_open_capability_with(&missing, GameDocumentFormat::Pdf, true),
            DocumentOpenCapability::MissingFile
        );
    }
    #[test]
    fn romm_missing_local_file_reports_unavailable() {
        let root = tempdir().unwrap();
        let mapping = romm_mapping(root.path());
        let manual = romm_manual("/assets/romm/resources/missing.pdf");
        let result = project_romm_manual_document(RommDocumentRequest {
            game_id: 9,
            platform: "Saturn",
            mapping: Some(&mapping),
            manual: &manual,
            verified_identity: None,
        });
        assert!(matches!(
            result,
            Err(DocumentUnavailableReason::NoLongerAvailable)
        ));
    }

    // 12. CBR remains recognised-but-blocked
    #[test]
    fn cbr_is_always_unsupported_regardless_of_handler_or_existence() {
        let dir = tempdir().unwrap();
        assert_eq!(
            document_open_capability_with(&dir.path().join("x.cbr"), GameDocumentFormat::Cbr, true),
            DocumentOpenCapability::UnsupportedFormat
        );
    }

    // 13. resume lookup uses unified document representation
    #[test]
    fn resume_state_is_shared_by_canonical_path_across_provenance() {
        let dir = tempdir().unwrap();
        let game_dir = dir.path().join("game");
        fs::create_dir(&game_dir).unwrap();
        let game = game_dir.join("game.bin");
        fs::write(&game, b"game").unwrap();
        let manual_path = game_dir.join("readme.pdf");
        fs::write(&manual_path, b"%PDF").unwrap();
        let local_docs = discover_documents(DocumentDiscoveryRequest {
            game_id: 4,
            game_title: "Sonic",
            platform: "Saturn",
            game_path: &game,
            roots: &[],
            associations: &BTreeMap::new(),
            verified_identity: None,
        });
        let mapping = romm_mapping(&game_dir);
        let manual = romm_manual("/assets/romm/resources/readme.pdf");
        let romm_document = project_romm_manual_document(RommDocumentRequest {
            game_id: 4,
            platform: "Saturn",
            mapping: Some(&mapping),
            manual: &manual,
            verified_identity: None,
        })
        .unwrap();
        // Same underlying file, reached two ways: resume state keyed by
        // canonical path resolves to the same entry either way.
        assert_eq!(local_docs[0].path, romm_document.path);
        let mut reading = BTreeMap::new();
        reading.insert(
            local_docs[0].path.clone(),
            DocumentReadingState {
                last_page: Some(3),
                zoom_percent: None,
            },
        );
        assert_eq!(
            safe_resume_page(reading.get(&romm_document.path), romm_document.page_count),
            Some(3)
        );
    }

    // 14. no network activity (by construction: this module performs no
    // network I/O anywhere - it only reads local files via `std::fs`, and
    // the RomM path resolves exclusively through the already-audited
    // `resolve_local_romm_manual`, which is local-filesystem-only).
    #[test]
    fn module_source_contains_no_network_primitives() {
        let source = include_str!("documents.rs");
        // Only the production code above the test module needs checking -
        // this assertion string itself would otherwise self-match.
        let production = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in ["TcpStream", "reqwest", "UdpSocket", "http:", "https:"] {
            assert!(
                !production.contains(forbidden),
                "unexpected network-shaped token: {forbidden}"
            );
        }
    }
}
