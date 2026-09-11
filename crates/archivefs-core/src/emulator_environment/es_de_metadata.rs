//! Read-only ES-DE `gamelist.xml` metadata/media ingestion.
//!
//! This is intentionally an indexable provider snapshot, not an identity
//! resolver: DAT/hash evidence remains authoritative and provider conflicts
//! are retained as provenance.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

const MAX_GAMELIST_BYTES: usize = 8 * 1024 * 1024;
const MAX_SYSTEM_DIRECTORIES: usize = 256;
const AMBIGUOUS_INDEX: usize = usize::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EsDeProviderStatus {
    Missing,
    Found,
    Unreadable,
    Invalid,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeMediaRefs {
    pub cover: Option<PathBuf>,
    pub thumbnail: Option<PathBuf>,
    pub marquee: Option<PathBuf>,
    pub screenshot: Option<PathBuf>,
    pub video: Option<PathBuf>,
    /// Raw XML values are retained so a downloaded-media projection never
    /// loses the provider's original provenance.
    pub raw_cover: Option<String>,
    pub raw_thumbnail: Option<String>,
    pub raw_marquee: Option<String>,
    pub raw_screenshot: Option<String>,
    pub raw_video: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeGameEntry {
    pub path: PathBuf,
    /// Canonical ROM-root binding when discovery was given an explicit
    /// EmuWiz ROM root. The raw path remains the provider's original
    /// gamelist-relative path.
    pub canonical_path: Option<PathBuf>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub rating: Option<String>,
    pub release_date: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub genre: Option<String>,
    pub players: Option<String>,
    pub media: EsDeMediaRefs,
    pub system: String,
    pub canonical_platform: Option<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeProviderSnapshot {
    pub status: EsDeProviderStatus,
    pub gamelist_path: PathBuf,
    pub entries: Vec<EsDeGameEntry>,
    pub warnings: Vec<String>,
}

/// One cached, indexed view of an ES-DE snapshot.  Building this index is the
/// only place where provider XML and referenced media paths are read; lookups
/// are map operations and never walk the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeProviderIndex {
    pub generation: u64,
    pub entries: Vec<EsDeGameEntry>,
    pub by_platform_path: BTreeMap<String, usize>,
    pub media: BTreeMap<PathBuf, EsDeMediaAvailability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeProviderCollection {
    pub root: PathBuf,
    pub generation: u64,
    pub indexes: Vec<EsDeProviderIndex>,
    pub warnings: Vec<String>,
}

/// Discover the documented `gamelists/<system>/gamelist.xml` shape with one
/// bounded directory listing.  This is refresh-time work; callers retain the
/// returned collection and never parse XML during selected-game lookup.
pub fn discover_provider_snapshot(root: &Path, generation: u64) -> EsDeProviderCollection {
    discover_provider_snapshot_with_rom_root(root, generation, None)
}

pub fn discover_provider_snapshot_with_rom_root(
    root: &Path,
    generation: u64,
    canonical_rom_root: Option<&Path>,
) -> EsDeProviderCollection {
    let mut result = EsDeProviderCollection {
        root: root.to_path_buf(),
        generation,
        indexes: Vec::new(),
        warnings: Vec::new(),
    };
    let gamelists = root.join("gamelists");
    let Ok(entries) = fs::read_dir(&gamelists) else {
        result.warnings.push(format!(
            "ES-DE gamelists directory unavailable: {}",
            gamelists.display()
        ));
        return result;
    };
    let mut systems = entries.filter_map(Result::ok).collect::<Vec<_>>();
    systems.sort_by_key(|entry| entry.file_name());
    for directory in systems.into_iter().take(MAX_SYSTEM_DIRECTORIES) {
        let Ok(file_type) = directory.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let system = directory.file_name().to_string_lossy().into_owned();
        let path = directory.path().join("gamelist.xml");
        let Ok(xml) = read_bounded_gamelist(&path) else {
            result
                .warnings
                .push(format!("could not read {}", path.display()));
            continue;
        };
        result.indexes.push(parse_and_index_gamelist_with_roots(
            &path,
            &xml,
            &system,
            generation,
            &root.join("downloaded_media"),
            canonical_rom_root,
        ));
    }
    result
}

impl EsDeProviderCollection {
    pub fn lookup_path(&self, platform: &str, source: &Path) -> Option<EsDeResolvedEntry> {
        self.indexes
            .iter()
            .find_map(|index| index.lookup_path(platform, source))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EsDeMediaAvailability {
    pub exists: bool,
    pub readable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeResolvedEntry {
    pub entry: EsDeGameEntry,
    pub media: EsDeMediaAvailabilitySet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EsDeMediaAvailabilitySet {
    pub cover: Option<EsDeMediaAvailability>,
    pub screenshot: Option<EsDeMediaAvailability>,
    pub video: Option<EsDeMediaAvailability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EsDeMediaCoverage {
    pub entries: usize,
    pub cover_references: usize,
    pub covers_present: usize,
    pub screenshot_references: usize,
    pub screenshots_present: usize,
    pub video_references: usize,
    pub videos_present: usize,
}

/// Stable key used for exact provider matching.  Platform is part of the key
/// so two systems containing identically named files cannot collide.
pub fn provider_path_key(platform: &str, path: &Path) -> String {
    format!("{platform}\0{}", normalize_provider_path(path).display())
}

/// Build the one lookup index and cache existence/readability of every
/// referenced media item.  This intentionally performs no recursive scan.
pub fn index_snapshot(snapshot: &EsDeProviderSnapshot, generation: u64) -> EsDeProviderIndex {
    let mut index = EsDeProviderIndex {
        generation,
        entries: snapshot.entries.clone(),
        by_platform_path: BTreeMap::new(),
        media: BTreeMap::new(),
    };
    for (position, entry) in index.entries.iter().enumerate() {
        if let Some(platform) = entry.canonical_platform.as_deref() {
            index
                .by_platform_path
                .entry(provider_path_key(
                    platform,
                    entry.canonical_path.as_deref().unwrap_or(&entry.path),
                ))
                .and_modify(|existing| {
                    if *existing != AMBIGUOUS_INDEX
                        && snapshot
                            .entries
                            .get(*existing)
                            .is_some_and(|prior| !same_entry(prior, entry))
                    {
                        *existing = AMBIGUOUS_INDEX;
                    }
                })
                .or_insert(position);
        }
        for path in [
            entry.media.cover.as_ref(),
            entry.media.screenshot.as_ref(),
            entry.media.video.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            index
                .media
                .entry(normalize_provider_path(path))
                .or_insert_with(|| {
                    let exists = fs::symlink_metadata(path)
                        .map(|m| m.is_file())
                        .unwrap_or(false);
                    let readable = exists && fs::File::open(path).is_ok();
                    EsDeMediaAvailability { exists, readable }
                });
        }
    }
    index
}

impl EsDeProviderIndex {
    pub fn coverage(&self) -> EsDeMediaCoverage {
        let mut coverage = EsDeMediaCoverage {
            entries: self.entries.len(),
            ..Default::default()
        };
        for entry in &self.entries {
            for (reference, refs, present) in [
                (
                    entry.media.cover.as_ref(),
                    &mut coverage.cover_references,
                    &mut coverage.covers_present,
                ),
                (
                    entry.media.screenshot.as_ref(),
                    &mut coverage.screenshot_references,
                    &mut coverage.screenshots_present,
                ),
                (
                    entry.media.video.as_ref(),
                    &mut coverage.video_references,
                    &mut coverage.videos_present,
                ),
            ] {
                if let Some(path) = reference {
                    *refs += 1;
                    if self
                        .media
                        .get(&normalize_provider_path(path))
                        .is_some_and(|state| state.exists && state.readable)
                    {
                        *present += 1;
                    }
                }
            }
        }
        coverage
    }

    pub fn lookup_path(&self, platform: &str, source: &Path) -> Option<EsDeResolvedEntry> {
        let position = *self
            .by_platform_path
            .get(&provider_path_key(platform, source))?;
        if position == AMBIGUOUS_INDEX {
            return None;
        }
        let entry = self.entries.get(position)?.clone();
        let media = EsDeMediaAvailabilitySet {
            cover: entry
                .media
                .cover
                .as_ref()
                .and_then(|p| self.media.get(&normalize_provider_path(p)).copied()),
            screenshot: entry
                .media
                .screenshot
                .as_ref()
                .and_then(|p| self.media.get(&normalize_provider_path(p)).copied()),
            video: entry
                .media
                .video
                .as_ref()
                .and_then(|p| self.media.get(&normalize_provider_path(p)).copied()),
        };
        Some(EsDeResolvedEntry { entry, media })
    }
}

fn normalize_provider_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve an ES-DE system name through the existing reviewed mapping.  No
/// reverse mapping table is maintained here.
pub fn canonical_platform_for_system(system: &str) -> Option<&'static str> {
    crate::launch::es_de_export::ES_DE_SYSTEM_MAP
        .iter()
        .find(|mapping| mapping.es_de_system.eq_ignore_ascii_case(system))
        .map(|mapping| mapping.platform_id)
        .or_else(|| {
            crate::launch::es_de_export::ES_DE_SYSTEM_MAP
                .iter()
                .find(|mapping| mapping.es_de_fullname.eq_ignore_ascii_case(system))
                .map(|mapping| mapping.platform_id)
        })
        // ES-DE has both `genesis` and `megadrive` folders for the same
        // Mega Drive hardware; the reviewed forward mapping intentionally
        // chooses `megadrive`, so this reverse synonym reuses that row.
        .or_else(|| system.eq_ignore_ascii_case("genesis").then(|| "MegaDrive"))
}

/// Parse one explicitly supplied gamelist using the reviewed mapping, then
/// build its indexed provider view.  Callers can combine several snapshots
/// without changing the parser or introducing directory recursion.
pub fn parse_and_index_gamelist(
    gamelist_path: &Path,
    xml: &[u8],
    system: &str,
    generation: u64,
) -> EsDeProviderIndex {
    let snapshot = parse_gamelist(gamelist_path, xml, system, |name| {
        canonical_platform_for_system(name).map(str::to_string)
    });
    index_snapshot(&snapshot, generation)
}

/// Parse and index a gamelist using the ES-DE downloaded-media root. This is
/// the production refresh path; media is resolved deterministically here and
/// existence/readability is cached by `index_snapshot`, never during lookup.
pub fn parse_and_index_gamelist_with_media_root(
    gamelist_path: &Path,
    xml: &[u8],
    system: &str,
    generation: u64,
    media_root: &Path,
) -> EsDeProviderIndex {
    parse_and_index_gamelist_with_roots(gamelist_path, xml, system, generation, media_root, None)
}

pub fn parse_and_index_gamelist_with_roots(
    gamelist_path: &Path,
    xml: &[u8],
    system: &str,
    generation: u64,
    media_root: &Path,
    canonical_rom_root: Option<&Path>,
) -> EsDeProviderIndex {
    let snapshot = parse_gamelist_with_media_root(
        gamelist_path,
        xml,
        system,
        |name| canonical_platform_for_system(name).map(str::to_string),
        Some(media_root),
        canonical_rom_root,
    );
    index_snapshot(&snapshot, generation)
}

#[derive(Debug, Deserialize)]
struct RawGameList {
    #[serde(rename = "game", default)]
    games: Vec<RawGame>,
}

#[derive(Debug, Deserialize)]
struct RawGame {
    path: String,
    name: Option<String>,
    desc: Option<String>,
    rating: Option<String>,
    releasedate: Option<String>,
    developer: Option<String>,
    publisher: Option<String>,
    genre: Option<String>,
    players: Option<String>,
    image: Option<String>,
    thumbnail: Option<String>,
    marquee: Option<String>,
    video: Option<String>,
    screenshot: Option<String>,
}

/// Parses one bounded ES-DE gamelist. `system_map` is supplied by the caller
/// from the existing reviewed platform mapping; no second mapping table is
/// created here.
pub fn parse_gamelist(
    gamelist_path: &Path,
    xml: &[u8],
    system: &str,
    system_map: impl Fn(&str) -> Option<String>,
) -> EsDeProviderSnapshot {
    parse_gamelist_with_media_root(gamelist_path, xml, system, system_map, None, None)
}

fn parse_gamelist_with_media_root(
    gamelist_path: &Path,
    xml: &[u8],
    system: &str,
    system_map: impl Fn(&str) -> Option<String>,
    media_root: Option<&Path>,
    canonical_rom_root: Option<&Path>,
) -> EsDeProviderSnapshot {
    let mut snapshot = EsDeProviderSnapshot {
        status: EsDeProviderStatus::Found,
        gamelist_path: gamelist_path.to_path_buf(),
        entries: Vec::new(),
        warnings: Vec::new(),
    };
    if xml.len() > MAX_GAMELIST_BYTES {
        snapshot.status = EsDeProviderStatus::Invalid;
        snapshot
            .warnings
            .push("gamelist exceeds the bounded size limit".into());
        return snapshot;
    }
    let parsed: RawGameList = match quick_xml::de::from_reader(xml) {
        Ok(value) => value,
        Err(error) => {
            snapshot.status = EsDeProviderStatus::Invalid;
            snapshot
                .warnings
                .push(format!("gamelist XML is invalid: {error}"));
            return snapshot;
        }
    };
    let base = gamelist_path.parent().unwrap_or_else(|| Path::new("."));
    for (index, game) in parsed.games.into_iter().enumerate() {
        let Some(path) = safe_reference(base, &game.path) else {
            snapshot
                .warnings
                .push(format!("entry {index} has an unsafe or invalid game path"));
            continue;
        };
        let media = EsDeMediaRefs {
            cover: resolve_media_ref(
                base,
                media_root,
                &system,
                game.image.as_deref(),
                MediaKind::Image,
            ),
            thumbnail: resolve_media_ref(
                base,
                media_root,
                &system,
                game.thumbnail.as_deref(),
                MediaKind::Thumbnail,
            ),
            marquee: resolve_media_ref(
                base,
                media_root,
                &system,
                game.marquee.as_deref(),
                MediaKind::Marquee,
            ),
            screenshot: resolve_media_ref(
                base,
                media_root,
                &system,
                game.screenshot.as_deref(),
                MediaKind::Screenshot,
            ),
            video: resolve_media_ref(
                base,
                media_root,
                &system,
                game.video.as_deref(),
                MediaKind::Video,
            ),
            raw_cover: game.image,
            raw_thumbnail: game.thumbnail,
            raw_marquee: game.marquee,
            raw_screenshot: game.screenshot,
            raw_video: game.video,
        };
        snapshot.entries.push(EsDeGameEntry {
            path,
            canonical_path: canonical_rom_root
                .and_then(|root| canonical_game_path(root, &system, &game.path)),
            name: game.name.filter(|value| !value.trim().is_empty()),
            description: game.desc,
            rating: game.rating,
            release_date: game.releasedate,
            developer: game.developer,
            publisher: game.publisher,
            genre: game.genre,
            players: game.players,
            media,
            system: system.to_string(),
            canonical_platform: system_map(system),
            provenance: format!("ES-DE gamelist {} entry {index}", gamelist_path.display()),
        });
    }
    snapshot.status = EsDeProviderStatus::Ready;
    snapshot
}

fn canonical_game_path(root: &Path, system: &str, raw: &str) -> Option<PathBuf> {
    let value = raw.trim();
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\0')
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }
    let system_path = Path::new(system);
    if system.is_empty()
        || system_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(system_path).join(path))
}

fn same_entry(left: &EsDeGameEntry, right: &EsDeGameEntry) -> bool {
    left.path == right.path
        && left.canonical_path == right.canonical_path
        && left.name == right.name
        && left.description == right.description
        && left.rating == right.rating
        && left.release_date == right.release_date
        && left.developer == right.developer
        && left.publisher == right.publisher
        && left.genre == right.genre
        && left.players == right.players
        && left.media == right.media
        && left.system == right.system
        && left.canonical_platform == right.canonical_platform
}

fn read_bounded_gamelist(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("gamelist symlink refused".into());
    }
    if !metadata.file_type().is_file() {
        return Err("gamelist is not a regular file".into());
    }
    if metadata.len() > MAX_GAMELIST_BYTES as u64 {
        return Err("gamelist exceeds the bounded size limit".into());
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_GAMELIST_BYTES as u64) as usize);
    file.take(MAX_GAMELIST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_GAMELIST_BYTES {
        return Err("gamelist grew beyond the bounded size limit".into());
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Image,
    Thumbnail,
    Marquee,
    Screenshot,
    Video,
}

fn resolve_media_ref(
    gamelist_base: &Path,
    media_root: Option<&Path>,
    system: &str,
    raw: Option<&str>,
    kind: MediaKind,
) -> Option<PathBuf> {
    let raw = raw?;
    let safe = safe_reference(gamelist_base, raw)?;
    let Some(media_root) = media_root else {
        return Some(safe);
    };
    resolve_downloaded_media_ref(media_root, system, raw, kind)
}

fn resolve_downloaded_media_ref(
    media_root: &Path,
    system: &str,
    raw: &str,
    kind: MediaKind,
) -> Option<PathBuf> {
    let raw_path = Path::new(raw.trim());
    let mut components = raw_path.components();
    if matches!(components.next(), Some(Component::CurDir)) {
        // ES-DE's stored references conventionally begin with `./`.
    } else {
        return None;
    }
    let directory = components.next()?.as_os_str().to_str()?;
    let file = components.next()?.as_os_str().to_str()?;
    if directory != "images" && directory != "videos" {
        return None;
    }
    if components.next().is_some() {
        return None;
    }
    let (category, suffix) = match kind {
        MediaKind::Image => ("miximages", "-image"),
        MediaKind::Thumbnail => ("covers", "-thumb"),
        MediaKind::Marquee => ("marquees", "-marquee"),
        MediaKind::Screenshot => ("screenshots", "-image"),
        MediaKind::Video => ("videos", "-video"),
    };
    if (kind == MediaKind::Video) != (directory == "videos") {
        return None;
    }
    let path = Path::new(file);
    let extension = path.extension()?.to_str()?;
    let stem = path.file_stem()?.to_str()?;
    let basename = stem.strip_suffix(suffix)?;
    if basename.is_empty() || basename.contains('/') || basename.contains('\\') {
        return None;
    }
    Some(
        media_root
            .join(system)
            .join(category)
            .join(format!("{basename}.{extension}")),
    )
}

fn safe_reference(base: &Path, raw: &str) -> Option<PathBuf> {
    let value = raw.trim();
    if value.is_empty() || value.contains('\0') {
        return None;
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }
    Some(base.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_media_fields_and_preserves_platform_provenance() {
        let xml = br#"<gameList><game><path>./../roms/game.zip</path><name>Bad</name></game><game><path>game.rom</path><name>Good</name><image>media/cover.png</image><screenshot>media/shot.png</screenshot><video>media/demo.mp4</video></game></gameList>"#;
        let snapshot = parse_gamelist(
            Path::new("/esde/gamelists/nes/gamelist.xml"),
            xml,
            "nes",
            |system| (system == "nes").then(|| "NES".into()),
        );
        assert_eq!(snapshot.status, EsDeProviderStatus::Ready);
        assert_eq!(snapshot.entries.len(), 1);
        let entry = &snapshot.entries[0];
        assert_eq!(entry.canonical_platform.as_deref(), Some("NES"));
        assert!(entry.media.cover.is_some());
        assert!(entry.media.screenshot.is_some());
        assert!(entry.media.video.is_some());
        assert!(entry.provenance.contains("entry 1"));
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("unsafe"))
        );
    }

    #[test]
    fn malformed_and_oversized_lists_fail_closed() {
        let malformed = parse_gamelist(Path::new("list.xml"), b"<gameList>", "nes", |_| None);
        assert_eq!(malformed.status, EsDeProviderStatus::Invalid);
        let oversized = parse_gamelist(
            Path::new("list.xml"),
            &vec![b'x'; MAX_GAMELIST_BYTES + 1],
            "nes",
            |_| None,
        );
        assert_eq!(oversized.status, EsDeProviderStatus::Invalid);
    }

    #[test]
    fn indexed_lookup_is_exact_and_uses_cached_media_state() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("gamelist.xml");
        let cover = dir.path().join("cover.png");
        std::fs::write(&cover, b"png").unwrap();
        let xml = br#"<gameList><game><path>./game.rom</path><name>Example</name><image>./cover.png</image></game></gameList>"#;
        let index = parse_and_index_gamelist(&list, xml, "nes", 7);
        let hit = index
            .lookup_path("NES", &dir.path().join("./game.rom"))
            .unwrap();
        assert_eq!(hit.entry.name.as_deref(), Some("Example"));
        assert_eq!(
            hit.media.cover,
            Some(EsDeMediaAvailability {
                exists: true,
                readable: true
            })
        );
        assert!(
            index
                .lookup_path("SNES", &dir.path().join("game.rom"))
                .is_none()
        );
    }

    #[test]
    fn reverse_mapping_reuses_reviewed_es_de_rows() {
        assert_eq!(canonical_platform_for_system("genesis"), Some("MegaDrive"));
        assert_eq!(
            canonical_platform_for_system("Nintendo GameCube"),
            Some("GameCube")
        );
        assert_eq!(canonical_platform_for_system("not-a-system"), None);
    }

    #[test]
    fn provider_discovery_is_bounded_and_refresh_generation_is_retained() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("gamelists/nes")).unwrap();
        std::fs::write(
            dir.path().join("gamelists/nes/gamelist.xml"),
            b"<gameList/>",
        )
        .unwrap();
        let snapshot = discover_provider_snapshot(dir.path(), 42);
        assert_eq!(snapshot.generation, 42);
        assert_eq!(snapshot.indexes.len(), 1);
        assert_eq!(snapshot.indexes[0].generation, 42);
    }

    #[test]
    fn production_media_root_resolves_exact_cover_screenshot_and_video_paths() {
        let dir = tempfile::tempdir().unwrap();
        let media_root = dir.path().join("downloaded_media");
        for (category, name) in [
            ("miximages", "Example.png"),
            ("screenshots", "Example.png"),
            ("videos", "Example.mp4"),
        ] {
            let path = media_root.join("nes").join(category);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(name), b"media").unwrap();
        }
        let list = dir.path().join("gamelists/nes/gamelist.xml");
        let xml = br#"<gameList><game><path>./game.rom</path><image>./images/Example-image.png</image><screenshot>./images/Example-image.png</screenshot><video>./videos/Example-video.mp4</video></game></gameList>"#;
        let index = parse_and_index_gamelist_with_media_root(&list, xml, "nes", 9, &media_root);
        let hit = index
            .lookup_path("NES", &dir.path().join("gamelists/nes/game.rom"))
            .unwrap();
        assert_eq!(
            hit.entry.media.raw_cover.as_deref(),
            Some("./images/Example-image.png")
        );
        assert_eq!(
            hit.entry.media.cover,
            Some(media_root.join("nes/miximages/Example.png"))
        );
        assert_eq!(
            hit.entry.media.screenshot,
            Some(media_root.join("nes/screenshots/Example.png"))
        );
        assert_eq!(
            hit.entry.media.video,
            Some(media_root.join("nes/videos/Example.mp4"))
        );
        assert_eq!(hit.media.cover.unwrap().exists, true);
        assert_eq!(hit.media.screenshot.unwrap().exists, true);
        assert_eq!(hit.media.video.unwrap().exists, true);
        assert_eq!(index.generation, 9);
    }

    #[test]
    fn downloaded_media_resolution_is_exact_case_sensitive_and_missing_stays_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let media_root = dir.path().join("downloaded_media");
        let cover_dir = media_root.join("nes/miximages");
        std::fs::create_dir_all(&cover_dir).unwrap();
        std::fs::write(cover_dir.join("Exact.png"), b"media").unwrap();
        let list = dir.path().join("gamelists/nes/gamelist.xml");
        let xml = br#"<gameList><game><path>./exact.rom</path><image>./images/Exact-image.png</image><video>./videos/Missing-video.mp4</video></game><game><path>./case.rom</path><image>./images/exact-image.png</image></game><game><path>./escape.rom</path><image>./images/../Exact-image.png</image></game></gameList>"#;
        let index = parse_and_index_gamelist_with_media_root(&list, xml, "nes", 12, &media_root);
        let exact = index
            .lookup_path("NES", &dir.path().join("gamelists/nes/exact.rom"))
            .unwrap();
        assert_eq!(exact.media.cover.unwrap().exists, true);
        assert!(exact.media.video.unwrap().exists == false);
        let case = index
            .lookup_path("NES", &dir.path().join("gamelists/nes/case.rom"))
            .unwrap();
        assert_eq!(case.media.cover.unwrap().exists, false);
        let escaped = index
            .lookup_path("NES", &dir.path().join("gamelists/nes/escape.rom"))
            .unwrap();
        assert!(escaped.entry.media.cover.is_none());
    }

    #[test]
    fn lookup_uses_cached_generation_without_filesystem_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let media_root = dir.path().join("downloaded_media");
        let media = media_root.join("nes/miximages");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("Example.png"), b"media").unwrap();
        let list = dir.path().join("gamelists/nes/gamelist.xml");
        let xml = br#"<gameList><game><path>./game.rom</path><image>./images/Example-image.png</image></game></gameList>"#;
        let index = parse_and_index_gamelist_with_media_root(&list, xml, "nes", 33, &media_root);
        std::fs::remove_file(media.join("Example.png")).unwrap();
        let hit = index
            .lookup_path("NES", &dir.path().join("gamelists/nes/game.rom"))
            .unwrap();
        assert_eq!(hit.media.cover.unwrap().exists, true);
        assert_eq!(index.generation, 33);
    }

    #[test]
    fn discovered_gamelist_reads_are_bounded_at_the_exact_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gamelist.xml");
        std::fs::write(&path, b"<gameList/>").unwrap();
        assert_eq!(read_bounded_gamelist(&path).unwrap(), b"<gameList/>");
        std::fs::write(&path, vec![b'x'; MAX_GAMELIST_BYTES]).unwrap();
        assert_eq!(
            read_bounded_gamelist(&path).unwrap().len(),
            MAX_GAMELIST_BYTES
        );
        std::fs::write(&path, vec![b'x'; MAX_GAMELIST_BYTES + 1]).unwrap();
        assert!(read_bounded_gamelist(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn discovered_symlinked_gamelist_is_refused_without_following_target() {
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("gamelists/nes");
        std::fs::create_dir_all(&system).unwrap();
        let target = dir.path().join("target.xml");
        std::fs::write(
            &target,
            b"<gameList><game><path>./game.rom</path></game></gameList>",
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, system.join("gamelist.xml")).unwrap();
        let snapshot = discover_provider_snapshot(dir.path(), 1);
        assert!(snapshot.indexes.is_empty());
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("could not read"))
        );
    }

    #[test]
    fn canonical_rom_root_binds_different_installation_roots_by_platform_and_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let canonical_root = dir.path().join("emuwiz-roms");
        let list = dir.path().join("esde/gamelists/nes/gamelist.xml");
        let xml = br#"<gameList><game><path>./Lemmings.zip</path><name>Lemmings</name></game></gameList>"#;
        let index = parse_and_index_gamelist_with_roots(
            &list,
            xml,
            "nes",
            2,
            &dir.path().join("esde/downloaded_media"),
            Some(&canonical_root),
        );
        let hit = index.lookup_path("NES", &canonical_root.join("nes/Lemmings.zip"));
        assert_eq!(hit.unwrap().entry.name.as_deref(), Some("Lemmings"));
        assert!(
            index
                .lookup_path("NES", &dir.path().join("other-root/Lemmings.zip"))
                .is_none()
        );
    }

    #[test]
    fn identical_duplicates_deduplicate_but_conflicting_duplicates_become_ambiguous() {
        let list = Path::new("/esde/gamelists/nes/gamelist.xml");
        let identical = br#"<gameList><game><path>./game.rom</path><name>Same</name></game><game><path>./game.rom</path><name>Same</name></game></gameList>"#;
        let index = parse_and_index_gamelist(list, identical, "nes", 1);
        assert!(
            index
                .lookup_path("NES", &list.parent().unwrap().join("game.rom"))
                .is_some()
        );
        let conflicting = br#"<gameList><game><path>./game.rom</path><name>One</name></game><game><path>./game.rom</path><name>Two</name></game></gameList>"#;
        let index = parse_and_index_gamelist(list, conflicting, "nes", 1);
        assert!(
            index
                .lookup_path("NES", &list.parent().unwrap().join("game.rom"))
                .is_none()
        );
    }

    #[test]
    fn malformed_system_does_not_discard_a_valid_system_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("gamelists/nes")).unwrap();
        std::fs::create_dir_all(dir.path().join("gamelists/snes")).unwrap();
        std::fs::write(
            dir.path().join("gamelists/nes/gamelist.xml"),
            b"<gameList><game><path>./good.rom</path></game></gameList>",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("gamelists/snes/gamelist.xml"),
            b"<gameList>",
        )
        .unwrap();
        let snapshot = discover_provider_snapshot(dir.path(), 4);
        assert_eq!(snapshot.indexes.len(), 2);
        assert_eq!(snapshot.indexes[0].entries.len(), 1);
        assert_eq!(snapshot.indexes[1].entries.len(), 0);
    }

    #[test]
    fn downloaded_media_projection_covers_all_supported_local_roles() {
        let dir = tempfile::tempdir().unwrap();
        let media_root = dir.path().join("downloaded_media");
        for (category, name) in [
            ("miximages", "Game.png"),
            ("covers", "Game.png"),
            ("marquees", "Game.png"),
            ("screenshots", "Game.png"),
            ("videos", "Game.mp4"),
        ] {
            let path = media_root.join("nes").join(category);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(name), b"media").unwrap();
        }
        let list = dir.path().join("gamelists/nes/gamelist.xml");
        let xml = br#"<gameList><game><path>./Game.zip</path><image>./images/Game-image.png</image><thumbnail>./images/Game-thumb.png</thumbnail><marquee>./images/Game-marquee.png</marquee><screenshot>./images/Game-image.png</screenshot><video>./videos/Game-video.mp4</video></game></gameList>"#;
        let index = parse_and_index_gamelist_with_media_root(&list, xml, "nes", 5, &media_root);
        let entry = &index.entries[0];
        assert_eq!(
            entry.media.cover,
            Some(media_root.join("nes/miximages/Game.png"))
        );
        assert_eq!(
            entry.media.thumbnail,
            Some(media_root.join("nes/covers/Game.png"))
        );
        assert_eq!(
            entry.media.marquee,
            Some(media_root.join("nes/marquees/Game.png"))
        );
        assert_eq!(
            entry.media.screenshot,
            Some(media_root.join("nes/screenshots/Game.png"))
        );
        assert_eq!(
            entry.media.video,
            Some(media_root.join("nes/videos/Game.mp4"))
        );
        for path in [
            entry.media.cover.as_ref(),
            entry.media.thumbnail.as_ref(),
            entry.media.marquee.as_ref(),
            entry.media.screenshot.as_ref(),
            entry.media.video.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(path.starts_with(&media_root));
        }
    }
}
