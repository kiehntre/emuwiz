//! Read-only index of media already present in a local LaunchBox library.
//!
//! This provider does not contact LaunchBox and does not replace RomM's
//! LaunchBox-derived identity. XML is parsed and the bounded media tree is
//! indexed during refresh; selected-game lookup is map access only.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use quick_xml::de::from_reader;
use serde::Deserialize;

use super::media_resolver::{MediaDelivery, MediaProvider, ProviderMediaSnapshot};
use super::model::MediaReference;

const MAX_XML_BYTES: usize = 32 * 1024 * 1024;
const MAX_MEDIA_FILES: usize = 100_000;
const MAX_MEDIA_DEPTH: usize = 8;

/// Sentinel stored in an identity map when two *different* game records
/// claim the same strong identity (a `DatabaseID`, or the same normalized
/// path). A genuine duplicate - the same record listed twice, e.g. because
/// a platform XML got merged from two sources - still deduplicates to one
/// entry; only a real conflict (same identity, different game data) is
/// pushed to this sentinel so `lookup` refuses to arbitrarily pick a
/// winner by XML ordering.
const AMBIGUOUS_INDEX: usize = usize::MAX;

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
            if let Some(position) = resolve_unambiguous(&self.by_database_id, id) {
                return self.games.get(position).map(|game| LaunchBoxLookup {
                    game,
                    strength: LaunchBoxMatchStrength::DatabaseId,
                });
            }
        }
        if let Some(path) = application_path {
            let key = normalized_path(path);
            if let Some(position) = resolve_unambiguous(&self.by_exact_path, &key) {
                return self.games.get(position).map(|game| LaunchBoxLookup {
                    game,
                    strength: LaunchBoxMatchStrength::ExactPath,
                });
            }
            if let Some(platform) = platform {
                let key = platform_path_key(platform, &key);
                if let Some(position) = resolve_unambiguous(&self.by_platform_path, &key) {
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
        let bytes = match read_bounded_xml(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                index.warnings.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
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
                insert_or_mark_ambiguous(
                    &mut index.by_database_id,
                    &index.games,
                    id,
                    position,
                    &record,
                );
            }
            if let Some(path) = application_path {
                let key = normalized_path(&path);
                insert_or_mark_ambiguous(
                    &mut index.by_exact_path,
                    &index.games,
                    key.clone(),
                    position,
                    &record,
                );
                insert_or_mark_ambiguous(
                    &mut index.by_platform_path,
                    &index.games,
                    platform_path_key(&platform, &key),
                    position,
                    &record,
                );
            }
            index.games.push(record);
        }
    }
    Ok(index)
}

/// Reads a platform XML file under the same fail-closed bound ES-DE's own
/// gamelist reader uses (`read_bounded_gamelist` in `es_de_metadata.rs`):
/// the size check runs against filesystem metadata *before* any bytes are
/// read, a symlink is refused outright rather than followed, and the actual
/// read is capped so a file that grows past the bound mid-read still can't
/// be pulled fully into memory. This replaces the previous `fs::read` (which
/// read the whole file first and only checked its length afterwards, so an
/// arbitrarily large file - not just one over the bound - would be loaded
/// into memory before being rejected).
fn read_bounded_xml(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("platform XML symlink refused".into());
    }
    if !metadata.file_type().is_file() {
        return Err("platform XML is not a regular file".into());
    }
    if metadata.len() > MAX_XML_BYTES as u64 {
        return Err("platform XML exceeds the bounded size limit".into());
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_XML_BYTES as u64) as usize);
    file.take(MAX_XML_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_XML_BYTES {
        return Err("platform XML grew beyond the bounded size limit".into());
    }
    Ok(bytes)
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

/// Reconciles a LaunchBox `ApplicationPath` (often Wine's own view of the
/// filesystem) with EmuWiz's native path so the two compare as the same
/// identity.
///
/// LaunchBox typically runs under Wine, and every standard Wine prefix
/// (`wineboot`, and every tool built on it) maps drive `Z:` to the host
/// filesystem root `/` - it is the one drive letter with a documented,
/// universal host mapping. `Z:\mnt\roms\Example.z64` is therefore Wine's
/// own spelling of the native `/mnt/roms/Example.z64`, not a different or
/// unrelated path, and stripping exactly that one drive prefix is a
/// deterministic, explainable rule rather than a guess.
///
/// No other drive letter is touched. `C:` (the prefix's own virtual `C:\`
/// drive, e.g. `C:\LaunchBox\Games\...`) has no such mapping: it names a
/// location inside the Wine prefix, not any specific native directory this
/// code could name without guessing. Those paths are lowercased and
/// slash-unified like any other, and compared as their own dialect - they
/// will not spuriously match a native EmuWiz path.
fn normalized_path(path: &Path) -> String {
    let value = path
        .to_string_lossy()
        .replace('\\', "/")
        .trim()
        .to_ascii_lowercase();
    collapse_repeated_slashes(&strip_wine_z_drive(&value))
}

/// `z:/...` becomes `/...`; anything else (a bare `z:` with no following
/// slash, any other drive letter, an already-native path) is returned
/// unchanged. Only ever recognizes the one documented Wine convention,
/// never a partial or malformed drive prefix.
fn strip_wine_z_drive(value: &str) -> String {
    match value.strip_prefix("z:/") {
        Some(rest) => format!("/{rest}"),
        None => value.to_string(),
    }
}

/// Collapses runs of `/` into one. Needed because a Wine path can carry a
/// doubled separator (e.g. `Z:\\mnt\...` becomes `z://mnt/...` after the
/// backslash swap above and the drive-prefix strip); without this, two
/// forms of the same real path would fail a string-identity comparison.
fn collapse_repeated_slashes(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut last_was_slash = false;
    for ch in value.chars() {
        let is_slash = ch == '/';
        if is_slash && last_was_slash {
            continue;
        }
        last_was_slash = is_slash;
        result.push(ch);
    }
    result
}

fn platform_path_key(platform: &str, path: &str) -> String {
    format!("{}\0{}", title_key(platform), path)
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

/// Records `position` under `key` in `map`, applying the identity-conflict
/// policy: a first sighting of `key` simply wins; a second sighting whose
/// game record is byte-for-byte identical to the one already indexed
/// deduplicates silently (still pointing at the first position); a second
/// sighting whose record *differs* means two distinct games are claiming
/// the same strong identity, which is a real conflict, not a duplicate -
/// the slot is marked [`AMBIGUOUS_INDEX`] so `lookup` refuses to guess a
/// winner from XML ordering. An already-ambiguous slot stays ambiguous.
fn insert_or_mark_ambiguous(
    map: &mut BTreeMap<String, usize>,
    games: &[LaunchBoxLocalGame],
    key: String,
    position: usize,
    record: &LaunchBoxLocalGame,
) {
    map.entry(key)
        .and_modify(|existing| {
            if *existing != AMBIGUOUS_INDEX && games.get(*existing) != Some(record) {
                *existing = AMBIGUOUS_INDEX;
            }
        })
        .or_insert(position);
}

/// Looks up `key` in an identity map, returning `None` for both "not found"
/// and "found, but ambiguous" - a conflicting identity must never resolve to
/// either of the disputing records.
fn resolve_unambiguous(map: &BTreeMap<String, usize>, key: &str) -> Option<usize> {
    match map.get(key) {
        Some(&AMBIGUOUS_INDEX) => None,
        Some(&position) => Some(position),
        None => None,
    }
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

    #[test]
    fn wine_z_drive_backslash_matches_native_linux_path() {
        assert_eq!(
            normalized_path(Path::new(r"Z:\mnt\roms\Example.z64")),
            normalized_path(Path::new("/mnt/roms/Example.z64")),
        );
    }

    #[test]
    fn wine_z_drive_forward_slash_matches_native_linux_path() {
        assert_eq!(
            normalized_path(Path::new("z:/mnt/roms/example.z64")),
            normalized_path(Path::new("/mnt/roms/Example.z64")),
        );
    }

    #[test]
    fn wine_z_drive_mixed_slash_direction_still_matches() {
        assert_eq!(
            normalized_path(Path::new(r"Z:\mnt/roms\Example.z64")),
            normalized_path(Path::new("/mnt/roms/Example.z64")),
        );
    }

    #[test]
    fn wine_z_drive_is_case_insensitive_on_both_the_drive_letter_and_path() {
        assert_eq!(
            normalized_path(Path::new(r"z:\MNT\ROMS\EXAMPLE.Z64")),
            normalized_path(Path::new("/mnt/roms/example.z64")),
        );
    }

    #[test]
    fn ordinary_c_drive_is_not_rewritten_into_a_native_root_path() {
        let value = normalized_path(Path::new(r"C:\LaunchBox\Games\foo.zip"));
        assert_eq!(value, "c:/launchbox/games/foo.zip");
        assert_ne!(value, "/launchbox/games/foo.zip");
        assert!(!value.starts_with('/'));
    }

    #[test]
    fn an_arbitrary_other_drive_letter_is_left_alone_too() {
        let value = normalized_path(Path::new(r"D:\Roms\foo.zip"));
        assert_eq!(value, "d:/roms/foo.zip");
        assert!(!value.starts_with('/'));
    }

    #[test]
    fn malformed_drive_prefixes_are_not_guessed_at() {
        // No slash immediately after the colon: not the documented Wine
        // form, so it is left untouched rather than treated as a match.
        assert_eq!(normalized_path(Path::new("z:mnt/roms")), "z:mnt/roms");
        // A bogus multi-character "drive" is never recognized.
        assert_eq!(normalized_path(Path::new("zz:/mnt/roms")), "zz:/mnt/roms");
        // Doubled separator after the drive prefix still resolves to a
        // single, exact-comparable native path.
        assert_eq!(
            normalized_path(Path::new(r"Z:\\mnt\roms\Example.z64")),
            normalized_path(Path::new("/mnt/roms/Example.z64")),
        );
    }

    #[test]
    fn native_linux_paths_are_unaffected_by_wine_normalization() {
        assert_eq!(
            normalized_path(Path::new("/mnt/roms/Example.z64")),
            "/mnt/roms/example.z64"
        );
    }

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

    /// The real-world case this whole tier exists for: LaunchBox's own XML
    /// stores the Wine-side `Z:\...` path (see `fixture()`'s
    /// `<ApplicationPath>Z:\mnt\roms\Example.z64</ApplicationPath>`), but the
    /// caller here is EmuWiz's own identity record, which only ever knows
    /// its native Linux path with no drive prefix at all. Proves the
    /// provider's own `lookup()` tier - not just the `normalized_path`
    /// helper in isolation - reconciles the two and still projects real
    /// media, exactly as `exact_path_and_title_are_fallbacks_but_unknown_
    /// title_does_not_match` already proves for the (weaker) self-consistent
    /// z:-prefixed case.
    #[test]
    fn native_linux_query_path_matches_a_wine_style_launchbox_application_path() {
        let (_dir, index) = fixture();
        let native_path = Path::new("/mnt/roms/Example.z64");
        let lookup = index
            .lookup(None, Some(native_path), Some("Nintendo 64"), None)
            .expect("native path must match the Wine-style ApplicationPath");
        assert_eq!(lookup.strength, LaunchBoxMatchStrength::ExactPath);
        let snapshot = index.media_snapshot(&lookup);
        assert!(
            snapshot.cover.is_some(),
            "matched entry must still carry its real LaunchBox media"
        );
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

    /// Writes a single platform XML made of exactly the two `<Game>` bodies
    /// given, in that order, and returns the resulting index. Used by the
    /// duplicate-identity tests below, which only differ in the two game
    /// bodies and (for the order test) their order.
    fn discover_from_two_games(first: &str, second: &str) -> LaunchBoxLocalProviderIndex {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Data/Platforms")).unwrap();
        let xml = format!("<LaunchBox>{first}{second}</LaunchBox>");
        std::fs::write(root.join("Data/Platforms/Test.xml"), xml).unwrap();
        discover_launchbox_local(root, 1).unwrap()
    }

    // --- Section 4: duplicate / conflicting identity handling ---

    #[test]
    fn identical_duplicate_database_id_dedupes_and_still_resolves() {
        // The exact same game record listed twice (e.g. a platform XML
        // merged from two sources) is not a real conflict: it should
        // dedupe to one entry and still resolve normally.
        let game = "<Game><ID>lb-1</ID><DatabaseID>42</DatabaseID><Platform>Test</Platform><Title>Example</Title></Game>";
        let index = discover_from_two_games(game, game);
        assert_eq!(index.games.len(), 2, "both records are still stored");
        let lookup = index
            .lookup(Some("42"), None, None, None)
            .expect("an identical duplicate must still resolve");
        assert_eq!(lookup.strength, LaunchBoxMatchStrength::DatabaseId);
        assert_eq!(lookup.game.title, "Example");
    }

    #[test]
    fn conflicting_duplicate_database_id_becomes_ambiguous() {
        // Two different games erroneously sharing one DatabaseID must not
        // let XML ordering silently pick a winner.
        let first = "<Game><ID>lb-1</ID><DatabaseID>42</DatabaseID><Platform>Test</Platform><Title>First</Title></Game>";
        let second = "<Game><ID>lb-2</ID><DatabaseID>42</DatabaseID><Platform>Test</Platform><Title>Second</Title></Game>";
        let index = discover_from_two_games(first, second);
        assert!(
            index.lookup(Some("42"), None, None, None).is_none(),
            "a conflicting DatabaseID must not arbitrarily project either game's media"
        );
        // Each game is still individually reachable by its own weak title
        // candidate; only the shared strong identity is suppressed.
        assert_eq!(
            index
                .lookup(None, None, None, Some("First"))
                .unwrap()
                .game
                .title,
            "First"
        );
    }

    #[test]
    fn conflicting_duplicate_normalized_path_becomes_ambiguous() {
        // Two different games claiming the same normalized ApplicationPath
        // (here spelled with different Wine slash direction) must also
        // resolve to ambiguous, not to whichever happened to parse first.
        let first = r#"<Game><ID>lb-1</ID><Platform>Test</Platform><Title>First</Title><ApplicationPath>Z:\mnt\roms\Example.z64</ApplicationPath></Game>"#;
        let second = r#"<Game><ID>lb-2</ID><Platform>Test</Platform><Title>Second</Title><ApplicationPath>z:/mnt/roms/Example.z64</ApplicationPath></Game>"#;
        let index = discover_from_two_games(first, second);
        let path = Path::new("/mnt/roms/Example.z64");
        assert!(
            index.lookup(None, Some(path), Some("Test"), None).is_none(),
            "a conflicting normalized path must not arbitrarily project either game's media"
        );
    }

    #[test]
    fn ambiguous_conflict_outcome_is_independent_of_xml_entry_order() {
        let first = "<Game><ID>lb-1</ID><DatabaseID>7</DatabaseID><Platform>Test</Platform><Title>First</Title></Game>";
        let second = "<Game><ID>lb-2</ID><DatabaseID>7</DatabaseID><Platform>Test</Platform><Title>Second</Title></Game>";
        let forward = discover_from_two_games(first, second);
        let reversed = discover_from_two_games(second, first);
        assert!(forward.lookup(Some("7"), None, None, None).is_none());
        assert!(reversed.lookup(Some("7"), None, None, None).is_none());
    }

    // --- Section 5: DatabaseID stays the strongest tier ---

    #[test]
    fn database_id_match_wins_over_a_conflicting_weaker_path_match() {
        // Game A is looked up by its own DatabaseID while the caller also
        // supplies an ApplicationPath that only matches a *different* game
        // (Game B). If the weaker ExactPath tier were consulted first, or
        // preferred, this would incorrectly resolve to Game B. DatabaseID
        // must win regardless.
        //
        // The provider does not - and cannot - independently prove that a
        // RomM `metadata_provider_ids` value under the "launchbox" provider
        // key and a LaunchBox-local `DatabaseID` share one global namespace;
        // that RomM/LaunchBox ID-space equivalence is an external semantic
        // assumption fed in by the caller (see `romm/normalise.rs`'s
        // `METADATA_ID_FIELDS`), not something this module verifies. This
        // test only proves the *local* priority ordering once a
        // `database_id` is supplied, not the cross-system assumption.
        let game_a = "<Game><ID>lb-a</ID><DatabaseID>1</DatabaseID><Platform>Test</Platform><Title>GameA</Title><ApplicationPath>/mnt/roms/a.zip</ApplicationPath></Game>";
        let game_b = "<Game><ID>lb-b</ID><Platform>Test</Platform><Title>GameB</Title><ApplicationPath>/mnt/roms/b.zip</ApplicationPath></Game>";
        let index = discover_from_two_games(game_a, game_b);
        let lookup = index
            .lookup(
                Some("1"),
                Some(Path::new("/mnt/roms/b.zip")),
                Some("Test"),
                None,
            )
            .unwrap();
        assert_eq!(lookup.strength, LaunchBoxMatchStrength::DatabaseId);
        assert_eq!(lookup.game.title, "GameA");
    }

    // --- Section 6: XML safety ---

    #[test]
    fn read_bounded_xml_accepts_the_bound_and_rejects_one_byte_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Test.xml");
        std::fs::write(&path, vec![b'a'; MAX_XML_BYTES]).unwrap();
        assert_eq!(read_bounded_xml(&path).unwrap().len(), MAX_XML_BYTES);
        std::fs::write(&path, vec![b'a'; MAX_XML_BYTES + 1]).unwrap();
        assert!(read_bounded_xml(&path).is_err());
    }

    #[test]
    fn oversized_platform_xml_is_skipped_with_a_warning_not_a_fatal_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Data/Platforms")).unwrap();
        std::fs::write(
            root.join("Data/Platforms/Huge.xml"),
            vec![b'a'; MAX_XML_BYTES + 1],
        )
        .unwrap();
        let index = discover_launchbox_local(root, 1).unwrap();
        assert!(index.games.is_empty());
        assert!(
            index
                .warnings
                .iter()
                .any(|warning| warning.contains("exceeds the bounded size limit"))
        );
    }

    #[test]
    fn malformed_platform_xml_is_a_non_fatal_warning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Data/Platforms")).unwrap();
        std::fs::write(
            root.join("Data/Platforms/Broken.xml"),
            b"<LaunchBox><Game><ID>lb-1</ID>",
        )
        .unwrap();
        let index = discover_launchbox_local(root, 1).unwrap();
        assert!(index.games.is_empty());
        assert!(
            index
                .warnings
                .iter()
                .any(|warning| warning.contains("invalid XML"))
        );
    }

    // --- Section 7: symlinks are not followed ---

    #[cfg(unix)]
    #[test]
    fn symlinked_platform_xml_is_not_consumed_and_target_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Data/Platforms")).unwrap();
        let target = dir.path().join("target.xml");
        std::fs::write(
            &target,
            br#"<LaunchBox><Game><ID>lb-1</ID><DatabaseID>99</DatabaseID><Platform>Test</Platform><Title>Example</Title></Game></LaunchBox>"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, root.join("Data/Platforms/Test.xml")).unwrap();
        let index = discover_launchbox_local(root, 1).unwrap();
        assert!(
            index.games.is_empty(),
            "a symlinked platform XML must not be parsed, whether or not its target is safe"
        );
        assert!(
            index.lookup(Some("99"), None, None, None).is_none(),
            "the symlink target's game must not be reachable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_media_file_is_not_projected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Data/Platforms")).unwrap();
        std::fs::create_dir_all(root.join("Images/Test/Box - Front")).unwrap();
        let target = dir.path().join("real-cover.jpg");
        std::fs::write(&target, b"cover").unwrap();
        std::os::unix::fs::symlink(&target, root.join("Images/Test/Box - Front/Example-01.jpg"))
            .unwrap();
        std::fs::write(
            root.join("Data/Platforms/Test.xml"),
            br#"<LaunchBox><Game><ID>lb-1</ID><DatabaseID>1</DatabaseID><Platform>Test</Platform><Title>Example</Title></Game></LaunchBox>"#,
        )
        .unwrap();
        let index = discover_launchbox_local(root, 1).unwrap();
        assert_eq!(index.media_files_indexed, 0);
        assert!(index.games[0].media.is_empty());
    }
}
