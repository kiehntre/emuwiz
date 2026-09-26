//! Local manuals and guide discovery for GUI v2.
//!
//! This is deliberately a read-only foundation. Discovery is bounded to the
//! selected game's directory, a few conventional child directories, and
//! explicitly configured document roots. No document is downloaded,
//! extracted, rewritten, or deleted by this module.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const MAX_ROOTS: usize = 16;
const MAX_FILES_PER_DIRECTORY: usize = 512;
const MAX_DOCUMENTS: usize = 256;
const MAX_CBZ_ENTRIES: usize = 10_000;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GameDocumentViewerCapability {
    ExternalViewer,
    ExternalViewerUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub(crate) enum GameDocumentAssociation {
    Explicit,
    ExactGameIdentity,
    SameGameDirectory,
    ExactTitle,
    PlatformAndTitle,
    WeakFilename,
    Unmatched,
}

impl GameDocumentAssociation {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Explicit => "Explicit user association",
            Self::ExactGameIdentity => "Exact stored game identity",
            Self::SameGameDirectory => "Same game directory",
            Self::ExactTitle => "Exact title match",
            Self::PlatformAndTitle => "Platform and title match",
            Self::WeakFilename => "Weak filename hint",
            Self::Unmatched => "Not associated",
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
    pub(crate) viewer: GameDocumentViewerCapability,
}

impl GameDocument {
    pub(crate) fn source_label(&self) -> String {
        match &self.source {
            GameDocumentSource::NearbyGameDirectory => "nearby game directory".to_string(),
            GameDocumentSource::ConfiguredRoot(root) => {
                format!("configured root: {}", root.display())
            }
            GameDocumentSource::ExplicitAssociation => "explicit user association".to_string(),
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
                association_reason: association.label().to_string(),
                page_count,
                file_size: metadata.len(),
                viewer,
            });
        }
    }
    let mut documents = candidates.into_values().collect::<Vec<_>>();
    documents.sort_by_key(|document| {
        (
            document.association,
            normalize_title(&document.title),
            document.path.clone(),
        )
    });
    documents.truncate(MAX_DOCUMENTS);
    documents
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
) -> (Option<usize>, GameDocumentViewerCapability) {
    match format {
        GameDocumentFormat::Pdf => (
            pdf_page_count(path),
            GameDocumentViewerCapability::ExternalViewer,
        ),
        GameDocumentFormat::Cbz => (
            inspect_cbz(path).ok().map(|pages| pages.len()),
            GameDocumentViewerCapability::ExternalViewer,
        ),
        GameDocumentFormat::Cbr => (
            None,
            GameDocumentViewerCapability::ExternalViewerUnavailable,
        ),
        GameDocumentFormat::Unknown => (
            None,
            GameDocumentViewerCapability::ExternalViewerUnavailable,
        ),
    }
}

fn pdf_page_count(path: &Path) -> Option<usize> {
    let mut file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    let count = bytes
        .windows(12)
        .filter(|window| window[..11] == *b"/Type /Page" && window[11] != b's')
        .count();
    (count > 0).then_some(count)
}

pub(crate) fn inspect_cbz(path: &Path) -> Result<Vec<String>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("malformed CBZ: {error}"))?;
    if archive.len() > MAX_CBZ_ENTRIES {
        return Err("CBZ has too many entries".into());
    }
    let mut pages = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("malformed CBZ entry: {error}"))?;
        let name = entry.name().replace('\\', "/");
        let path = Path::new(&name);
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(format!("unsafe CBZ entry: {name}"));
        }
        if !entry.is_dir()
            && matches!(
                path.extension()
                    .and_then(|v| v.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp")
            )
        {
            pages.push(name);
        }
    }
    pages.sort_by_key(|name| natural_sort_key(name));
    Ok(pages)
}

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
enum NaturalPart {
    Text(String),
    Number(u64),
}

fn natural_sort_key(value: &str) -> Vec<NaturalPart> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut numeric = false;
    for character in value.chars() {
        let next_numeric = character.is_ascii_digit();
        if !current.is_empty() && next_numeric != numeric {
            parts.push(if numeric {
                NaturalPart::Number(current.parse().unwrap_or(u64::MAX))
            } else {
                NaturalPart::Text(current)
            });
            current = String::new();
        }
        numeric = next_numeric;
        current.push(character.to_ascii_lowercase());
    }
    if !current.is_empty() {
        parts.push(if numeric {
            NaturalPart::Number(current.parse().unwrap_or(u64::MAX))
        } else {
            NaturalPart::Text(current)
        });
    }
    parts
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
            GameDocumentViewerCapability::ExternalViewerUnavailable
        );
    }
    #[test]
    fn pdf_discovery_and_page_count_are_local_and_read_only() {
        let dir = tempdir().unwrap();
        let game = dir.path().join("game.bin");
        fs::write(&game, b"game").unwrap();
        let manual = dir.path().join("Sonic Manual.pdf");
        let bytes = b"%PDF /Type /Pages /Count 2 /Type /Page\n/Type /Page\n";
        fs::write(&manual, bytes).unwrap();
        let before = fs::read(&manual).unwrap();
        let docs = discover_documents(DocumentDiscoveryRequest {
            game_id: 4,
            game_title: "Sonic",
            platform: "Saturn",
            game_path: &game,
            roots: &[],
            associations: &BTreeMap::new(),
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
}
