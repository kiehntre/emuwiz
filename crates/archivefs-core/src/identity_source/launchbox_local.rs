//! Read-only index of media already present in a local LaunchBox library.
//!
//! This provider does not contact LaunchBox and does not replace RomM's
//! LaunchBox-derived identity. XML is parsed and the bounded media tree is
//! indexed during refresh; selected-game lookup is map access only.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use quick_xml::de::from_reader;
use serde::Deserialize;

use super::media_resolver::{MediaDelivery, MediaProvider, ProviderMediaSnapshot};
use super::model::MediaReference;

const MAX_XML_BYTES: usize = 32 * 1024 * 1024;
const MAX_MEDIA_FILES: usize = 100_000;
const MAX_MEDIA_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LaunchBoxMediaRole {
    Cover,
    Screenshot,
    Logo,
    Fanart,
    Banner,
    Video,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchBoxLocalMedia {
    pub role: LaunchBoxMediaRole,
    pub category: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LaunchBoxMatchStrength {
    DatabaseId,
    ExactPath,
    PlatformAndPath,
    TitleCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchBoxLocalGame {
    pub launchbox_id: String,
    pub database_id: Option<String>,
    pub platform: String,
    pub title: String,
    pub application_path: Option<PathBuf>,
    pub media: Vec<LaunchBoxLocalMedia>,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchBoxLocalProviderIndex {
    pub root: PathBuf,
    pub generation: u64,
    pub games: Vec<LaunchBoxLocalGame>,
    pub by_database_id: BTreeMap<String, usize>,
    pub by_exact_path: BTreeMap<String, usize>,
    pub by_platform_path: BTreeMap<String, usize>,
    pub media_files_indexed: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchBoxLookup<'a> {
    pub game: &'a LaunchBoxLocalGame,
    pub strength: LaunchBoxMatchStrength,
}

impl LaunchBoxLocalProviderIndex {
    /// Match strong LaunchBox identity first, then exact path forms. Title is
    /// intentionally only an explicit weak candidate and never overrides a
    /// stronger game identity.
    pub fn lookup<'a>(
        &'a self,
        database_id: Option<&str>,
        application_path: Option<&Path>,
        platform: Option<&str>,
        title: Option<&str>,
    ) -> Option<LaunchBoxLookup<'a>> {
        if let Some(id) = database_id.and_then(non_empty) {
            if let Some(&position) = self.by_database_id.get(id) {
                return self.games.get(position).map(|game| LaunchBoxLookup {
                    game,
                    strength: LaunchBoxMatchStrength::DatabaseId,
                });
            }
        }
        if let Some(path) = application_path {
            let key = normalized_path(path);
            if let Some(&position) = self.by_exact_path.get(&key) {
                return self.games.get(position).map(|game| LaunchBoxLookup {
                    game,
                    strength: LaunchBoxMatchStrength::ExactPath,
                });
            }
            if let Some(platform) = platform {
                let key = platform_path_key(platform, &key);
                if let Some(&position) = self.by_platform_path.get(&key) {
                    return self.games.get(position).map(|game| LaunchBoxLookup {
                        game,
                        strength: LaunchBoxMatchStrength::PlatformAndPath,
                    });
                }
            }
        }
        let title = title.and_then(non_empty)?;
        let key = title_key(title);
        self.games
            .iter()
            .position(|game| title_key(&game.title) == key)
            .map(|position| LaunchBoxLookup {
                game: &self.games[position],
                strength: LaunchBoxMatchStrength::TitleCandidate,
            })
    }

    /// Project one already-indexed local result into the shared media
    /// resolver. This performs no I/O and preserves local-path provenance.
    pub fn media_snapshot(&self, lookup: &LaunchBoxLookup<'_>) -> ProviderMediaSnapshot {
        let mut snapshot = ProviderMediaSnapshot::new(MediaProvider::LaunchBoxLocal);
        snapshot.delivery = MediaDelivery::LocalReady;
        for media in &lookup.game.media {
            let reference = MediaReference {
                hosted_reference: Some(media.path.to_string_lossy().into_owned()),
                public_reference: Some(format!(
                    "LaunchBoxLocal:{}:{}",
                    lookup.game.launchbox_id, media.category
                )),
            };
            match media.role {
                LaunchBoxMediaRole::Cover => snapshot.cover = Some(reference),
                LaunchBoxMediaRole::Screenshot => snapshot.screenshots.push(reference),
                LaunchBoxMediaRole::Video => snapshot.video = Some(reference),
                LaunchBoxMediaRole::Logo
                | LaunchBoxMediaRole::Fanart
                | LaunchBoxMediaRole::Banner
                | LaunchBoxMediaRole::Manual => {}
            }
        }
        snapshot
    }
}

/// Parse every bounded platform XML file and index its known local media.
pub fn discover_launchbox_local(
    root: &Path,
    generation: u64,
) -> Result<LaunchBoxLocalProviderIndex, String> {
    let platforms = root.join("Data/Platforms");
    let mut files = fs::read_dir(&platforms)
        .map_err(|error| format!("LaunchBox platform data unavailable: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "xml"))
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| entry.file_name());

    let mut index = LaunchBoxLocalProviderIndex {
        root: root.to_path_buf(),
        generation,
        games: Vec::new(),
        by_database_id: BTreeMap::new(),
        by_exact_path: BTreeMap::new(),
        by_platform_path: BTreeMap::new(),
        media_files_indexed: 0,
        warnings: Vec::new(),
    };
    for file in files {
        let path = file.path();
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        if bytes.len() > MAX_XML_BYTES {
            index
                .warnings
                .push(format!("{} exceeds XML size bound", path.display()));
            continue;
        }
        let parsed: RawLaunchBox = match from_reader(bytes.as_slice()) {
            Ok(value) => value,
            Err(error) => {
                index
                    .warnings
                    .push(format!("{}: invalid XML: {error}", path.display()));
                continue;
            }
        };
        let platform = parsed_platform(&path, &parsed.games);
        let platform_media = if safe_component(&platform) {
            index_platform_media(root, &platform, &mut index.media_files_indexed)
        } else {
            index.warnings.push(format!(
                "{} has an unsafe platform media component",
                path.display()
            ));
            Vec::new()
        };
        for game in parsed.games {
            let position = index.games.len();
            let launchbox_id = game.id.unwrap_or_default();
            let database_id = game.database_id.filter(|value| !value.trim().is_empty());
            let title = game.title.unwrap_or_default();
            let application_path = game
                .application_path
                .filter(|value| !value.trim().is_empty())
                .map(|value| PathBuf::from(value.replace('\\', "/")));
            let media = media_for_title(&platform_media, &title);
            let record = LaunchBoxLocalGame {
                launchbox_id: launchbox_id.clone(),
                database_id: database_id.clone(),
                platform: platform.clone(),
                title,
                application_path: application_path.clone(),
                media,
                provenance: format!("LaunchBox local {}", path.display()),
            };
            if let Some(id) = database_id {
                index.by_database_id.entry(id).or_insert(position);
            }
            if let Some(path) = application_path {
                let key = normalized_path(&path);
                index.by_exact_path.entry(key.clone()).or_insert(position);
                index
                    .by_platform_path
                    .entry(platform_path_key(&platform, &key))
                    .or_insert(position);
            }
            index.games.push(record);
        }
    }
    Ok(index)
}

#[derive(Debug, Deserialize)]
struct RawLaunchBox {
    #[serde(rename = "Game", default)]
    games: Vec<RawGame>,
}

#[derive(Debug, Deserialize)]
struct RawGame {
    #[serde(rename = "ID")]
    id: Option<String>,
    #[serde(rename = "DatabaseID")]
    database_id: Option<String>,
    #[serde(rename = "Platform")]
    platform: Option<String>,
    #[serde(rename = "Title")]
    title: Option<String>,
    #[serde(rename = "ApplicationPath")]
    application_path: Option<String>,
}

#[derive(Debug, Clone)]
struct IndexedFile {
    role: LaunchBoxMediaRole,
    category: String,
    path: PathBuf,
    key: String,
}

fn parsed_platform(path: &Path, games: &[RawGame]) -> String {
    games
        .iter()
        .find_map(|game| {
            game.platform
                .clone()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            path.file_stem()
                .map(|value| value.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Unknown".into())
}

fn index_platform_media(root: &Path, platform: &str, count: &mut usize) -> Vec<IndexedFile> {
    let mut result = Vec::new();
    let categories = [
        ("Box - Front", LaunchBoxMediaRole::Cover),
        ("Screenshot - Gameplay", LaunchBoxMediaRole::Screenshot),
        ("Screenshot - Game Title", LaunchBoxMediaRole::Screenshot),
        ("Clear Logo", LaunchBoxMediaRole::Logo),
        ("Fanart - Background", LaunchBoxMediaRole::Fanart),
        ("Banner", LaunchBoxMediaRole::Banner),
    ];
    for (category, role) in categories {
        collect_files(
            &root.join("Images").join(platform).join(category),
            category,
            role,
            &mut result,
            count,
            0,
        );
    }
    collect_files(
        &root.join("Videos").join(platform).join("Trailer"),
        "Trailer",
        LaunchBoxMediaRole::Video,
        &mut result,
        count,
        0,
    );
    collect_files(
        &root.join("Manuals").join(platform),
        "Manual",
        LaunchBoxMediaRole::Manual,
        &mut result,
        count,
        0,
    );
    result
}

fn collect_files(
    directory: &Path,
    category: &str,
    role: LaunchBoxMediaRole,
    result: &mut Vec<IndexedFile>,
    count: &mut usize,
    depth: usize,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < MAX_MEDIA_DEPTH {
                collect_files(&entry.path(), category, role, result, count, depth + 1);
            }
            continue;
        }
        if !kind.is_file() || *count >= MAX_MEDIA_FILES {
            continue;
        }
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !matches!(
            extension.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "mp4" | "mkv" | "pdf"
        ) {
            continue;
        }
        *count += 1;
        result.push(IndexedFile {
            role,
            category: category.into(),
            key: title_key_from_filename(&path),
            path,
        });
    }
}

fn media_for_title(files: &[IndexedFile], title: &str) -> Vec<LaunchBoxLocalMedia> {
    let key = title_key(title);
    files
        .iter()
        .filter(|file| file.key == key)
        .map(|file| LaunchBoxLocalMedia {
            role: file.role,
            category: file.category.clone(),
            path: file.path.clone(),
        })
        .collect()
}

fn title_key_from_filename(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let stem = stem.strip_suffix("-01").unwrap_or(&stem);
    title_key(stem)
}

fn title_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim()
        .to_ascii_lowercase()
}

fn platform_path_key(platform: &str, path: &str) -> String {
    format!("{}\0{}", title_key(platform), path)
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, LaunchBoxLocalProviderIndex) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Data/Platforms")).unwrap();
        std::fs::create_dir_all(root.join("Images/Nintendo 64/Box - Front/North America")).unwrap();
        std::fs::create_dir_all(root.join("Images/Nintendo 64/Screenshot - Gameplay")).unwrap();
        std::fs::create_dir_all(root.join("Images/Nintendo 64/Clear Logo")).unwrap();
        std::fs::create_dir_all(root.join("Videos/Nintendo 64/Trailer")).unwrap();
        std::fs::create_dir_all(root.join("Manuals/Nintendo 64")).unwrap();
        std::fs::write(
            root.join("Images/Nintendo 64/Box - Front/North America/Example-01.jpg"),
            b"cover",
        )
        .unwrap();
        std::fs::write(
            root.join("Images/Nintendo 64/Screenshot - Gameplay/Example-01.png"),
            b"shot",
        )
        .unwrap();
        std::fs::write(
            root.join("Images/Nintendo 64/Clear Logo/Example-01.png"),
            b"logo",
        )
        .unwrap();
        std::fs::write(
            root.join("Videos/Nintendo 64/Trailer/Example-01.mp4"),
            b"video",
        )
        .unwrap();
        std::fs::write(root.join("Manuals/Nintendo 64/Example-01.pdf"), b"manual").unwrap();
        std::fs::write(root.join("Data/Platforms/Nintendo 64.xml"), br#"<LaunchBox><Game><ID>lb-1</ID><DatabaseID>42</DatabaseID><Platform>Nintendo 64</Platform><Title>Example</Title><ApplicationPath>Z:\mnt\roms\Example.z64</ApplicationPath></Game></LaunchBox>"#).unwrap();
        let index = discover_launchbox_local(root, 7).unwrap();
        (dir, index)
    }

    #[test]
    fn indexes_roles_and_matches_by_database_id() {
        let (_dir, index) = fixture();
        let lookup = index.lookup(Some("42"), None, None, None).unwrap();
        assert_eq!(lookup.strength, LaunchBoxMatchStrength::DatabaseId);
        assert_eq!(lookup.game.media.len(), 5);
        let snapshot = index.media_snapshot(&lookup);
        assert!(snapshot.cover.is_some());
        assert_eq!(snapshot.screenshots.len(), 1);
        assert!(snapshot.video.is_some());
        assert_eq!(snapshot.provider, MediaProvider::LaunchBoxLocal);
        assert_eq!(index.generation, 7);
    }

    #[test]
    fn exact_path_and_title_are_fallbacks_but_unknown_title_does_not_match() {
        let (_dir, index) = fixture();
        let path = Path::new("z:/mnt/roms/example.z64");
        assert_eq!(
            index
                .lookup(None, Some(path), Some("Nintendo 64"), None)
                .unwrap()
                .strength,
            LaunchBoxMatchStrength::ExactPath
        );
        assert_eq!(
            index
                .lookup(None, None, None, Some("Example"))
                .unwrap()
                .strength,
            LaunchBoxMatchStrength::TitleCandidate
        );
        assert!(index.lookup(None, None, None, Some("Other")).is_none());
    }

    #[test]
    fn clear_logo_is_not_a_cover_and_missing_media_is_safe() {
        let (_dir, index) = fixture();
        let lookup = index.lookup(Some("42"), None, None, None).unwrap();
        let snapshot = index.media_snapshot(&lookup);
        assert_eq!(
            snapshot
                .cover
                .unwrap()
                .hosted_reference
                .unwrap()
                .contains("Box - Front"),
            true
        );
        assert!(index.games.iter().all(|game| {
            game.media
                .iter()
                .all(|media| media.path.starts_with(&index.root))
        }));
    }

    #[test]
    fn unsafe_platform_component_cannot_escape_launchbox_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Data/Platforms")).unwrap();
        std::fs::create_dir_all(root.join("Images/escape/Box - Front")).unwrap();
        std::fs::write(
            root.join("Images/escape/Box - Front/Example-01.jpg"),
            b"cover",
        )
        .unwrap();
        std::fs::write(
            root.join("Data/Platforms/unsafe.xml"),
            br#"<LaunchBox><Game><ID>lb-1</ID><Platform>../escape</Platform><Title>Example</Title></Game></LaunchBox>"#,
        )
        .unwrap();
        let index = discover_launchbox_local(root, 1).unwrap();
        assert!(index.games[0].media.is_empty());
        assert!(
            index
                .warnings
                .iter()
                .any(|warning| warning.contains("unsafe platform"))
        );
    }

    #[test]
    fn media_depth_is_relative_to_category_not_absolute_filesystem_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("one/two/three/four/five/six/LaunchBox");
        std::fs::create_dir_all(root.join("Data/Platforms")).unwrap();
        std::fs::create_dir_all(root.join("Images/Test/Box - Front/North America")).unwrap();
        std::fs::write(
            root.join("Images/Test/Box - Front/North America/Example-01.jpg"),
            b"cover",
        )
        .unwrap();
        std::fs::write(
            root.join("Data/Platforms/Test.xml"),
            br#"<LaunchBox><Game><ID>lb-1</ID><DatabaseID>1</DatabaseID><Platform>Test</Platform><Title>Example</Title></Game></LaunchBox>"#,
        )
        .unwrap();

        let index = discover_launchbox_local(&root, 1).unwrap();
        assert_eq!(index.media_files_indexed, 1);
        assert_eq!(index.games[0].media[0].role, LaunchBoxMediaRole::Cover);
    }
}
