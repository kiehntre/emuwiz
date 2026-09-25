//! Identity-driven, local-first bezel/decorations resolution.
//!
//! This module only resolves and previews decorations.  It does not download
//! assets, edit emulator configuration, or touch source media.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MAX_BEZEL_SOURCE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_BEZEL_DIMENSION: u32 = 8192;
pub const MAX_BEZEL_DECODED_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_BEZEL_DEPTH: usize = 8;
pub const DEFAULT_MAX_BEZEL_ASSETS: usize = 2048;

const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DecorationScope {
    Game,
    System,
    Default,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DecorationSource {
    LocalPack { path: String },
    UserOverride { path: String },
    Provider { name: String, reference: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecorationProvenance {
    pub provider: String,
    pub reference: String,
    pub retrieved_at_unix_seconds: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecorationTarget {
    pub emulator: String,
    pub core: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DecorationReadiness {
    Ready,
    Unsupported { reason: String },
    MissingConfiguration { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DecorationEvidence {
    VerifiedIdentity { identity: String },
    CanonicalDatIdentity { identity: String },
    PlatformIdentity { platform: String },
    ExplicitUserMapping { mapping: String },
    FilenameHint { value: String },
}

impl DecorationEvidence {
    fn strength(&self) -> u8 {
        match self {
            Self::VerifiedIdentity { .. } => 5,
            Self::CanonicalDatIdentity { .. } => 4,
            Self::ExplicitUserMapping { .. } => 3,
            Self::PlatformIdentity { .. } => 2,
            Self::FilenameHint { .. } => 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecorationAsset {
    pub id: String,
    pub scope: DecorationScope,
    pub source: DecorationSource,
    pub evidence: DecorationEvidence,
    pub targets: Vec<DecorationTarget>,
    pub readiness: DecorationReadiness,
    pub provenance: DecorationProvenance,
    pub viewport: Option<ViewportMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ViewportMetadata {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecorationResolution {
    pub selected: Option<DecorationAsset>,
    pub candidates: Vec<DecorationAsset>,
    pub reason: String,
    pub conflicts: Vec<String>,
    pub target: DecorationTarget,
}

/// User-owned local bezel roots. These are deliberately separate from ROM
/// source roots: discovery never treats game folders as decoration folders
/// unless the user explicitly adds them here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalBezelConfig {
    pub roots: Vec<PathBuf>,
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    #[serde(default = "default_max_assets")]
    pub max_assets: usize,
}

impl Default for LocalBezelConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            max_depth: DEFAULT_MAX_BEZEL_DEPTH,
            max_assets: DEFAULT_MAX_BEZEL_ASSETS,
        }
    }
}

fn default_max_depth() -> usize {
    DEFAULT_MAX_BEZEL_DEPTH
}

fn default_max_assets() -> usize {
    DEFAULT_MAX_BEZEL_ASSETS
}

impl LocalBezelConfig {
    pub fn bounded(mut self) -> Self {
        self.max_depth = self.max_depth.clamp(1, DEFAULT_MAX_BEZEL_DEPTH);
        self.max_assets = self.max_assets.clamp(1, DEFAULT_MAX_BEZEL_ASSETS);
        self.roots.sort();
        self.roots.dedup();
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BezelMappingMetadata {
    #[serde(default)]
    pub game_identity: Option<String>,
    #[serde(default)]
    pub dat_identity: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub emulator: Option<String>,
    #[serde(default)]
    pub core: Option<String>,
    #[serde(default)]
    pub scope: Option<DecorationScope>,
    #[serde(default)]
    pub viewport: Option<ViewportMetadata>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BezelMatchContext {
    pub verified_identity: Option<String>,
    pub dat_identity: Option<String>,
    pub platform: Option<String>,
    pub game_title: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalBezelImageInfo {
    pub width: u32,
    pub height: u32,
    pub format: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalBezelCatalogue {
    pub assets: Vec<DecorationAsset>,
    pub images: BTreeMap<String, LocalBezelImageInfo>,
    pub warnings: Vec<String>,
}

pub fn local_bezel_config_path() -> Result<PathBuf, String> {
    crate::app_dirs::config_path("bezel_catalogue.json").map_err(|error| error.to_string())
}

pub fn load_local_bezel_config() -> Result<LocalBezelConfig, String> {
    let path = local_bezel_config_path()?;
    let Ok(bytes) = fs::read(&path) else {
        return Ok(LocalBezelConfig {
            max_depth: DEFAULT_MAX_BEZEL_DEPTH,
            max_assets: DEFAULT_MAX_BEZEL_ASSETS,
            ..LocalBezelConfig::default()
        });
    };
    serde_json::from_slice::<LocalBezelConfig>(&bytes)
        .map(|config| config.bounded())
        .map_err(|error| format!("bezel catalogue configuration is invalid: {error}"))
}

pub fn save_local_bezel_config(config: &LocalBezelConfig) -> Result<(), String> {
    let path = local_bezel_config_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| "bezel catalogue configuration has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    serde_json::to_writer_pretty(temporary.as_file(), &config.clone().bounded())
        .map_err(|error| error.to_string())?;
    temporary.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

/// Discover and validate local image assets without modifying the source
/// tree. Sidecar metadata is optional and is read only from `<image>.json`.
pub fn discover_local_bezel_catalogue(config: &LocalBezelConfig) -> LocalBezelCatalogue {
    let config = config.clone().bounded();
    let mut catalogue = LocalBezelCatalogue::default();
    let mut stack = config
        .roots
        .iter()
        .filter(|root| root.is_dir())
        .map(|root| (root.clone(), 0usize))
        .collect::<Vec<_>>();
    while let Some((directory, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            catalogue.warnings.push(format!(
                "could not read bezel directory {}",
                directory.display()
            ));
            continue;
        };
        let mut entries = entries.flatten().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() && !file_type.is_symlink() && depth < config.max_depth {
                stack.push((path, depth + 1));
                continue;
            }
            if !file_type.is_file()
                || catalogue.assets.len() >= config.max_assets
                || !supported_image_path(&path)
            {
                continue;
            }
            match inspect_local_bezel(&path) {
                Ok((asset, info)) => {
                    catalogue.images.insert(asset.id.clone(), info);
                    catalogue.assets.push(asset);
                }
                Err(error) => catalogue
                    .warnings
                    .push(format!("{}: {error}", path.display())),
            }
        }
    }
    catalogue
}

pub fn resolve_local_bezel_catalogue(
    catalogue: &LocalBezelCatalogue,
    context: &BezelMatchContext,
    target: &DecorationTarget,
) -> DecorationResolution {
    let candidates = catalogue
        .assets
        .iter()
        .filter_map(|asset| match_asset(asset, context))
        .collect();
    resolve_decoration(candidates, target.clone())
}

fn supported_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn inspect_local_bezel(path: &Path) -> Result<(DecorationAsset, LocalBezelImageInfo), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("metadata unavailable: {error}"))?;
    if metadata.len() > MAX_BEZEL_SOURCE_BYTES {
        return Err("source image exceeds the safe byte limit".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_BEZEL_SOURCE_BYTES) as usize);
    fs::File::open(path)
        .map_err(|error| format!("image could not be opened: {error}"))?
        .take(MAX_BEZEL_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("image could not be read: {error}"))?;
    if bytes.len() as u64 > MAX_BEZEL_SOURCE_BYTES {
        return Err("source image exceeds the safe byte limit".into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("image format is not recognised: {error}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_BEZEL_DIMENSION);
    limits.max_image_height = Some(MAX_BEZEL_DIMENSION);
    limits.max_alloc = Some(MAX_BEZEL_DECODED_BYTES);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| format!("image refused as malformed or unsafe: {error}"))?;
    let info = LocalBezelImageInfo {
        width: decoded.width(),
        height: decoded.height(),
        format: decoded
            .color()
            .has_alpha()
            .then_some("rgba".to_string())
            .unwrap_or_else(|| "rgb".to_string()),
    };
    let mapping = read_mapping_metadata(path);
    let id = path.to_string_lossy().to_string();
    let target = DecorationTarget {
        emulator: mapping
            .as_ref()
            .and_then(|mapping| mapping.emulator.clone())
            .unwrap_or_default(),
        core: mapping.as_ref().and_then(|mapping| mapping.core.clone()),
    };
    let scope = mapping
        .as_ref()
        .and_then(|mapping| mapping.scope.clone())
        .unwrap_or(DecorationScope::Default);
    Ok((
        DecorationAsset {
            id: id.clone(),
            scope,
            source: DecorationSource::LocalPack { path: id.clone() },
            evidence: DecorationEvidence::FilenameHint {
                value: path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            targets: if target.emulator.is_empty() {
                Vec::new()
            } else {
                vec![target]
            },
            readiness: DecorationReadiness::Ready,
            provenance: DecorationProvenance {
                provider: "Local bezel catalogue".into(),
                reference: id.clone(),
                retrieved_at_unix_seconds: None,
            },
            viewport: mapping.and_then(|mapping| mapping.viewport),
        },
        info,
    ))
}

fn read_mapping_metadata(path: &Path) -> Option<BezelMappingMetadata> {
    let sidecar = PathBuf::from(format!("{}.json", path.display()));
    let bytes = fs::read(sidecar).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn match_asset(asset: &DecorationAsset, context: &BezelMatchContext) -> Option<DecorationAsset> {
    let path = match &asset.source {
        DecorationSource::LocalPack { path } | DecorationSource::UserOverride { path } => path,
        DecorationSource::Provider { .. } => return None,
    };
    let mapping = read_mapping_metadata(Path::new(path));
    let mut matched = asset.clone();
    let mapping = mapping.unwrap_or_default();
    if let (Some(expected), Some(actual)) = (&mapping.game_identity, &context.verified_identity) {
        if expected == actual {
            matched.evidence = DecorationEvidence::VerifiedIdentity {
                identity: actual.clone(),
            };
            matched.scope = DecorationScope::Game;
            return Some(matched);
        }
    }
    if let (Some(expected), Some(actual)) = (&mapping.dat_identity, &context.dat_identity) {
        if expected == actual {
            matched.evidence = DecorationEvidence::CanonicalDatIdentity {
                identity: actual.clone(),
            };
            matched.scope = DecorationScope::Game;
            return Some(matched);
        }
    }
    if let Some(platform) = mapping.platform.as_ref().or(mapping.system.as_ref()) {
        if context
            .platform
            .as_deref()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(platform))
        {
            matched.evidence = DecorationEvidence::PlatformIdentity {
                platform: platform.clone(),
            };
            matched.scope = DecorationScope::System;
            return Some(matched);
        }
        return None;
    }
    let Some(title) = context.game_title.as_deref() else {
        return None;
    };
    let stem = path_stem(path);
    if normalise_key(&stem) == normalise_key(title) {
        matched.evidence = DecorationEvidence::FilenameHint { value: stem };
        matched.scope = DecorationScope::Game;
        return Some(matched);
    }
    None
}

fn path_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

fn normalise_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn resolve_decoration(
    mut candidates: Vec<DecorationAsset>,
    target: DecorationTarget,
) -> DecorationResolution {
    candidates.retain(|asset| {
        (asset.targets.is_empty()
            || asset.targets.iter().any(|candidate| {
                candidate.emulator == target.emulator
                    && (candidate.core.is_none() || candidate.core == target.core)
            }))
            && matches!(asset.readiness, DecorationReadiness::Ready)
    });
    candidates.sort_by(|left, right| {
        right
            .source_rank()
            .cmp(&left.source_rank())
            .then_with(|| right.scope_rank().cmp(&left.scope_rank()))
            .then_with(|| right.evidence.strength().cmp(&left.evidence.strength()))
            .then_with(|| left.id.cmp(&right.id))
    });
    let conflicts = candidates
        .windows(2)
        .filter(|pair| {
            pair[0].evidence.strength() == pair[1].evidence.strength()
                && pair[0].scope == pair[1].scope
        })
        .map(|pair| format!("{} conflicts with {}", pair[0].id, pair[1].id))
        .collect::<Vec<_>>();
    let selected = candidates.first().cloned();
    let reason = selected
        .as_ref()
        .map(|asset| format!("selected {} using explicit evidence", asset.id))
        .unwrap_or_else(|| "no compatible local decoration was found".into());
    DecorationResolution {
        selected,
        candidates,
        reason,
        conflicts,
        target,
    }
}

impl DecorationAsset {
    fn source_rank(&self) -> u8 {
        match self.source {
            DecorationSource::UserOverride { .. } => 3,
            DecorationSource::LocalPack { .. } => 2,
            DecorationSource::Provider { .. } => 1,
        }
    }

    fn scope_rank(&self) -> u8 {
        match self.scope {
            DecorationScope::Game => 3,
            DecorationScope::System => 2,
            DecorationScope::Default => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn png(path: &Path, width: u32, height: u32) {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([20, 40, 60, 255]));
        image.save(path).unwrap();
    }

    fn target() -> DecorationTarget {
        DecorationTarget {
            emulator: "retroarch".into(),
            core: None,
        }
    }

    fn asset(id: &str, scope: DecorationScope, evidence: DecorationEvidence) -> DecorationAsset {
        DecorationAsset {
            id: id.into(),
            scope,
            source: DecorationSource::LocalPack { path: id.into() },
            evidence,
            targets: vec![DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            }],
            readiness: DecorationReadiness::Ready,
            provenance: DecorationProvenance {
                provider: "local-test".into(),
                reference: id.into(),
                retrieved_at_unix_seconds: None,
            },
            viewport: None,
        }
    }

    #[test]
    fn verified_identity_beats_filename_guess() {
        let result = resolve_decoration(
            vec![
                asset(
                    "guess",
                    DecorationScope::Game,
                    DecorationEvidence::FilenameHint {
                        value: "game".into(),
                    },
                ),
                asset(
                    "verified",
                    DecorationScope::Game,
                    DecorationEvidence::VerifiedIdentity {
                        identity: "game-id".into(),
                    },
                ),
            ],
            DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
        );
        assert_eq!(result.selected.unwrap().id, "verified");
    }

    #[test]
    fn game_beats_system_and_default_fallbacks() {
        let mut candidates = vec![
            asset(
                "default",
                DecorationScope::Default,
                DecorationEvidence::PlatformIdentity {
                    platform: "snes".into(),
                },
            ),
            asset(
                "system",
                DecorationScope::System,
                DecorationEvidence::PlatformIdentity {
                    platform: "snes".into(),
                },
            ),
            asset(
                "game",
                DecorationScope::Game,
                DecorationEvidence::ExplicitUserMapping {
                    mapping: "game".into(),
                },
            ),
        ];
        for candidate in &mut candidates {
            candidate.targets[0].emulator = "dolphin".into();
        }
        let result = resolve_decoration(
            candidates,
            DecorationTarget {
                emulator: "dolphin".into(),
                core: None,
            },
        );
        assert_eq!(result.selected.unwrap().id, "game");
    }

    #[test]
    fn incompatible_emulator_is_not_selected() {
        let result = resolve_decoration(
            vec![asset(
                "dolphin",
                DecorationScope::Game,
                DecorationEvidence::VerifiedIdentity {
                    identity: "id".into(),
                },
            )],
            DecorationTarget {
                emulator: "dolphin".into(),
                core: None,
            },
        );
        assert!(result.selected.is_none());
    }

    #[test]
    fn user_override_wins_over_provider_default() {
        let mut override_asset = asset(
            "override",
            DecorationScope::Game,
            DecorationEvidence::ExplicitUserMapping {
                mapping: "manual".into(),
            },
        );
        override_asset.source = DecorationSource::UserOverride {
            path: "override.png".into(),
        };
        let result = resolve_decoration(
            vec![
                asset(
                    "provider",
                    DecorationScope::Default,
                    DecorationEvidence::VerifiedIdentity {
                        identity: "id".into(),
                    },
                ),
                override_asset,
            ],
            DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
        );
        assert_eq!(result.selected.unwrap().id, "override");
    }

    #[test]
    fn unsupported_target_remains_visible_as_no_ready_selection() {
        let mut candidate = asset(
            "future",
            DecorationScope::Game,
            DecorationEvidence::VerifiedIdentity {
                identity: "id".into(),
            },
        );
        candidate.readiness = DecorationReadiness::Unsupported {
            reason: "adapter is not implemented".into(),
        };
        let result = resolve_decoration(
            vec![candidate],
            DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
        );
        assert!(result.selected.is_none());
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn local_discovery_is_nested_bounded_and_format_checked() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("packs").join("snes");
        fs::create_dir_all(&nested).unwrap();
        let valid = nested.join("hero.png");
        png(&valid, 32, 16);
        fs::write(nested.join("broken.jpg"), b"not an image").unwrap();
        fs::write(nested.join("ignored.txt"), b"not an image").unwrap();
        let too_deep = root.path().join("a/b/c/d/e/f/g/h/i");
        fs::create_dir_all(&too_deep).unwrap();
        png(&too_deep.join("too-deep.png"), 8, 8);

        let catalogue = discover_local_bezel_catalogue(&LocalBezelConfig {
            roots: vec![root.path().to_path_buf()],
            max_depth: 3,
            max_assets: 10,
        });
        assert_eq!(catalogue.assets.len(), 1);
        assert!(catalogue.images.contains_key(&valid.display().to_string()));
        assert!(
            catalogue
                .warnings
                .iter()
                .any(|warning| warning.contains("broken.jpg"))
        );
    }

    #[test]
    fn oversized_dimensions_are_refused_without_touching_source() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("oversized.png");
        png(&path, MAX_BEZEL_DIMENSION + 1, 1);
        let before = fs::read(&path).unwrap();
        let catalogue = discover_local_bezel_catalogue(&LocalBezelConfig {
            roots: vec![root.path().to_path_buf()],
            ..LocalBezelConfig::default()
        });
        assert!(catalogue.assets.is_empty());
        assert!(
            catalogue
                .warnings
                .iter()
                .any(|warning| warning.contains("oversized.png"))
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn explicit_mapping_and_identity_outrank_filename_inference() {
        let root = tempfile::tempdir().unwrap();
        let verified = root.path().join("not-the-title.png");
        let guessed = root.path().join("Game.png");
        png(&verified, 64, 64);
        png(&guessed, 64, 64);
        fs::write(
            format!("{}.json", verified.display()),
            r#"{"game_identity":"verified-game","scope":"Game"}"#,
        )
        .unwrap();
        let catalogue = discover_local_bezel_catalogue(&LocalBezelConfig {
            roots: vec![root.path().to_path_buf()],
            ..LocalBezelConfig::default()
        });
        let result = resolve_local_bezel_catalogue(
            &catalogue,
            &BezelMatchContext {
                verified_identity: Some("verified-game".into()),
                game_title: Some("Game".into()),
                ..BezelMatchContext::default()
            },
            &target(),
        );
        assert_eq!(
            result.selected.unwrap().source,
            DecorationSource::LocalPack {
                path: verified.display().to_string()
            }
        );
    }

    #[test]
    fn game_specific_and_system_candidates_keep_duplicates_inspectable() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("snes.png");
        let game_a = root.path().join("game-a.png");
        let game_b = root.path().join("game-b.png");
        for path in [&system, &game_a, &game_b] {
            png(path, 64, 64);
        }
        fs::write(
            format!("{}.json", system.display()),
            r#"{"platform":"SNES"}"#,
        )
        .unwrap();
        for path in [&game_a, &game_b] {
            fs::write(
                format!("{}.json", path.display()),
                r#"{"dat_identity":"dat-game","scope":"Game"}"#,
            )
            .unwrap();
        }
        let catalogue = discover_local_bezel_catalogue(&LocalBezelConfig {
            roots: vec![root.path().to_path_buf()],
            ..LocalBezelConfig::default()
        });
        let result = resolve_local_bezel_catalogue(
            &catalogue,
            &BezelMatchContext {
                dat_identity: Some("dat-game".into()),
                platform: Some("SNES".into()),
                ..BezelMatchContext::default()
            },
            &target(),
        );
        assert_eq!(
            result.selected.as_ref().unwrap().scope,
            DecorationScope::Game
        );
        assert_eq!(result.candidates.len(), 3);
        assert!(!result.conflicts.is_empty());
    }
}
