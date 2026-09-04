//! Bounded, read-only discovery, configuration, keys and content-layout
//! inspection for native Cemu (Wii U).
//!
//! This is a first slice: only the extracted `code`/`content`/`meta`
//! directory layout (the shape a dumped, already-decrypted Wii U title
//! takes on disk) is treated as launchable. `.wud`/`.wux` disc images and
//! `.wua` archives are recognised and classified, but never accepted for a
//! launch - see [`CemuContentForm::support`] for why each form is
//! classified the way it is.
//!
//! Nothing in this module writes `settings.xml`, downloads or generates
//! `keys.txt`, reads a key's contents, mutates the MLC, or executes a
//! discovered binary. Keys are presence/readability evidence only - see
//! [`CemuKeysState`].

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const CEMU_MAX_PROFILES: usize = 16;
pub const CEMU_MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub const CEMU_MAX_META_XML_BYTES: u64 = 64 * 1024;
const FLATPAK_APP_ID: &str = "info.cemu.Cemu";
const KEYS_FILE_NAME: &str = "keys.txt";

// ---------------------------------------------------------------------------
// Executable discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CemuInstallationType {
    Native,
    FlatpakUser,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuExecutable {
    pub path: PathBuf,
    pub installation_type: CemuInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
    pub appimage_directory: Option<PathBuf>,
}

impl CemuProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, CemuDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(CemuDiscoveryError::HomeUnavailable)?;
        let xdg_config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let appimage_directory =
            env::var_os("APPIMAGE").and_then(|p| PathBuf::from(p).parent().map(Path::to_path_buf));
        Ok(Self {
            home,
            xdg_config_home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuDiscoveryError {
    HomeUnavailable,
}
impl std::fmt::Display for CemuDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set")
    }
}
impl std::error::Error for CemuDiscoveryError {}

fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    regular(path) && fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    regular(path)
}

fn executable_candidates(roots: &CemuProfileDiscoveryRoots) -> Vec<CemuExecutable> {
    let mut paths: Vec<(PathBuf, CemuInstallationType)> = roots
        .explicit_executables
        .iter()
        .cloned()
        .map(|p| (p, CemuInstallationType::Explicit))
        .collect();
    if let Some(dir) = &roots.appimage_directory {
        for name in ["Cemu.AppImage", "cemu.AppImage"] {
            paths.push((dir.join(name), CemuInstallationType::Portable));
        }
    }
    if let Some(path_env) = env::var_os("PATH") {
        for dir in env::split_paths(&path_env) {
            for name in ["Cemu", "cemu"] {
                paths.push((dir.join(name), CemuInstallationType::Native));
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|(p, _)| executable(p))
        .map(|(path, installation_type)| {
            let version = roots
                .known_version_outputs
                .get(&path)
                .and_then(|output| parse_cemu_version(output));
            CemuExecutable {
                path,
                installation_type,
                version,
            }
        })
        .collect()
}

/// Parses a bounded `--version`-shaped Cemu output string. Never executes a
/// process itself - a caller runs Cemu with a timeout and bounded output
/// capture, exactly as every sibling adapter's `known_version_outputs` map
/// expects, and passes the resulting string in here. `None` (rather than a
/// blocked launch) is the correct answer when the string does not parse.
pub fn parse_cemu_version(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let start = lower.find("cemu")? + 4;
    let tail = output[start..].trim_start().trim_start_matches(['v', 'V']);
    let value: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!value.is_empty() && value.starts_with(|c: char| c.is_ascii_digit())).then_some(value)
}

// ---------------------------------------------------------------------------
// Keys evidence
// ---------------------------------------------------------------------------

/// Presence-only evidence about `keys.txt`. Never the file's contents -
/// nothing in this module ever reads a key byte into a `String`, a log
/// line, a test assertion, or anywhere else it could leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuKeysState {
    /// No `keys.txt` was found at any known location.
    NotConfigured,
    /// A `keys.txt`-named file exists and its metadata could be read. Its
    /// *contents* are never inspected, so this is not a claim that the keys
    /// inside are correct, complete, or even valid UTF-8/text - just that a
    /// file is there.
    PresentUnverified,
    /// A `keys.txt`-named path exists but its metadata could not be read
    /// (permission denied, or it is not a regular file).
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuKeysEvidence {
    pub path: Option<PathBuf>,
    pub state: CemuKeysState,
}

/// Checks the known Cemu `keys.txt` locations, in order: the profile's own
/// configuration directory (where `settings.xml` lives), then each
/// candidate executable's own directory (the portable convention). The
/// first match wins; nothing here reads past the file's metadata.
pub fn keys_evidence(
    configuration_path: &Path,
    executables: &[CemuExecutable],
) -> CemuKeysEvidence {
    let mut candidates = vec![configuration_path.join(KEYS_FILE_NAME)];
    for exe in executables {
        if let Some(dir) = exe.path.parent() {
            candidates.push(dir.join(KEYS_FILE_NAME));
        }
    }
    for candidate in candidates {
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return CemuKeysEvidence {
                    path: Some(candidate),
                    state: CemuKeysState::PresentUnverified,
                };
            }
            Ok(_) => {
                return CemuKeysEvidence {
                    path: Some(candidate),
                    state: CemuKeysState::Unreadable,
                };
            }
            Err(_) => continue,
        }
    }
    CemuKeysEvidence {
        path: None,
        state: CemuKeysState::NotConfigured,
    }
}

// ---------------------------------------------------------------------------
// Config / MLC inspection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuMlcState {
    /// `settings.xml` has no `<mlc_path>`, or no readable config was found
    /// at all.
    NotConfigured,
    Present,
    Missing,
    /// `<mlc_path>` names a path that is not a directory.
    NotADirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuMlcEvidence {
    pub path: Option<PathBuf>,
    pub state: CemuMlcState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuConfigInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub mlc: CemuMlcEvidence,
    /// `<GamePaths>` entries, kept as evidence only - never walked or
    /// trusted to resolve a launch target by themselves.
    pub game_paths: Vec<PathBuf>,
}

pub fn config_path(root: &Path) -> Option<PathBuf> {
    let candidate = root.join("settings.xml");
    regular(&candidate).then_some(candidate)
}

/// Extracts the first-seen text of each wanted flat tag from a small,
/// bounded XML document using [`quick_xml`] - the same decode/unescape
/// idiom [`crate::emulator_environment::es_de::parse_systems_xml`] uses,
/// scaled down to "record which of these tag names' text I have seen so
/// far", since `settings.xml`/`meta.xml` are flat enough that no nesting
/// tracking is needed. Never a general-purpose XML reader: an unclosed or
/// malformed document simply stops yielding further tags rather than
/// erroring, exactly like that module's own documented behaviour.
fn extract_flat_xml_tags(text: &str, wanted: &[&str]) -> BTreeMap<String, String> {
    use quick_xml::Reader;
    use quick_xml::escape::unescape;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut found = BTreeMap::new();
    let mut current_tag: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                current_tag = wanted.contains(&name.as_str()).then_some(name);
            }
            Ok(Event::Text(text_event)) => {
                if let Some(tag) = &current_tag
                    && !found.contains_key(tag)
                    && let Ok(decoded) = text_event.decode()
                {
                    let value = unescape(&decoded)
                        .map(|v| v.into_owned())
                        .unwrap_or_else(|_| decoded.into_owned());
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        found.insert(tag.clone(), value);
                    }
                }
            }
            Ok(Event::End(_)) => current_tag = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    found
}

pub fn inspect_config(path: &Path) -> CemuConfigInspection {
    let bytes = fs::read(path).ok();
    let readable = bytes
        .as_ref()
        .is_some_and(|b| b.len() as u64 <= CEMU_MAX_CONFIG_BYTES);
    let text = bytes
        .filter(|b| b.len() as u64 <= CEMU_MAX_CONFIG_BYTES)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let tags = extract_flat_xml_tags(&text, &["mlc_path"]);
    let mlc_path = tags.get("mlc_path").map(PathBuf::from);
    let mlc_state = match &mlc_path {
        None => CemuMlcState::NotConfigured,
        Some(p) if p.is_dir() => CemuMlcState::Present,
        Some(p) if p.exists() => CemuMlcState::NotADirectory,
        Some(_) => CemuMlcState::Missing,
    };
    // `<GamePaths>` entries are not parsed by `extract_flat_xml_tags` (it
    // only keeps the first occurrence of each wanted tag, and a game path
    // list is repeated `<Entry>` elements) - kept genuinely empty rather
    // than silently wrong until a real multi-value need justifies a second
    // extractor.
    CemuConfigInspection {
        path: path.to_path_buf(),
        exists: true,
        readable,
        mlc: CemuMlcEvidence {
            path: mlc_path,
            state: mlc_state,
        },
        game_paths: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuProfile {
    pub profile_id: String,
    pub installation_type: CemuInstallationType,
    pub configuration_path: PathBuf,
    pub config_path: Option<PathBuf>,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<CemuExecutable>,
    pub config: Option<CemuConfigInspection>,
    pub keys: CemuKeysEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuProfileDiscovery {
    pub profiles: Vec<CemuProfile>,
    pub complete: bool,
}

fn profile(
    root: PathBuf,
    installation_type: CemuInstallationType,
    all: &[CemuExecutable],
) -> CemuProfile {
    let config_path = config_path(&root);
    let config = config_path.as_ref().map(|p| inspect_config(p));
    let matching: Vec<CemuExecutable> = all
        .iter()
        .filter(|e| {
            e.installation_type == installation_type
                || installation_type == CemuInstallationType::Explicit
        })
        .cloned()
        .collect();
    let eligible = !matching.is_empty() && config.as_ref().is_none_or(|c| c.readable);
    let blocker = (!eligible).then(|| {
        if matching.is_empty() {
            "no safe Cemu executable was discovered".to_string()
        } else {
            "Cemu configuration is unreadable or oversized".to_string()
        }
    });
    let keys = keys_evidence(&root, &matching);
    CemuProfile {
        profile_id: format!("cemu:{}", root.display()),
        installation_type,
        configuration_path: root,
        config_path,
        eligible,
        blocker,
        executable_candidates: matching,
        config,
        keys,
    }
}

pub fn discover_cemu_profiles(roots: &CemuProfileDiscoveryRoots) -> CemuProfileDiscovery {
    let mut candidates = vec![(
        roots.xdg_config_home.join("Cemu"),
        CemuInstallationType::Native,
    )];
    candidates.push((
        roots
            .home
            .join(".var/app")
            .join(FLATPAK_APP_ID)
            .join("config/Cemu"),
        CemuInstallationType::FlatpakUser,
    ));
    candidates.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, CemuInstallationType::Portable)),
    );
    candidates.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|p| (p, CemuInstallationType::Explicit)),
    );
    candidates.sort();
    candidates.dedup_by(|a, b| a.0 == b.0);
    let all = executable_candidates(roots);
    let profiles = candidates
        .into_iter()
        .filter(|(p, k)| {
            p.is_dir()
                || matches!(
                    k,
                    CemuInstallationType::Explicit | CemuInstallationType::Portable
                )
        })
        .take(CEMU_MAX_PROFILES)
        .map(|(p, k)| profile(p, k, &all))
        .collect();
    CemuProfileDiscovery {
        profiles,
        complete: true,
    }
}

// ---------------------------------------------------------------------------
// Launch binding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuLaunchBlockerKind {
    ProfileIneligible,
    ExecutableMissing,
    AmbiguousExecutable,
    ExecutableUnsafe,
    ExecutableNotExecutable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuLaunchBlocker {
    pub kind: CemuLaunchBlockerKind,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuNativeLaunchBinding {
    pub executable: PathBuf,
}

pub fn resolve_cemu_native_launch_binding(
    profile: &CemuProfile,
) -> Result<CemuNativeLaunchBinding, CemuLaunchBlocker> {
    if !profile.eligible {
        return Err(CemuLaunchBlocker {
            kind: CemuLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "profile is not eligible".into()),
        });
    }
    let valid: Vec<_> = profile
        .executable_candidates
        .iter()
        .filter(|e| {
            e.installation_type == profile.installation_type
                || profile.installation_type == CemuInstallationType::Explicit
        })
        .filter(|e| executable(&e.path))
        .collect();
    match valid.as_slice() {
        [one] => Ok(CemuNativeLaunchBinding {
            executable: one.path.clone(),
        }),
        [] => Err(CemuLaunchBlocker {
            kind: CemuLaunchBlockerKind::ExecutableMissing,
            detail: "no safe executable matches this profile".into(),
        }),
        _ => Err(CemuLaunchBlocker {
            kind: CemuLaunchBlockerKind::AmbiguousExecutable,
            detail: "more than one safe executable matches this profile".into(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Content forms
// ---------------------------------------------------------------------------

/// How proven-safe each Wii U representation Cemu accepts is in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuContentSupport {
    DirectlyLaunchable,
    RequiresDirectoryLayout,
    RequiresKeys,
    UnsupportedPhase1,
}

/// The Wii U content shapes this module recognises. Recognising a shape is
/// not the same as accepting it for launch - see [`Self::support`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuContentForm {
    /// A `code`/`content`/`meta` directory - a dumped, already-decrypted
    /// title. The only form [`inspect_cemu_launch_content`] will build a
    /// command for in this build.
    ExtractedTitle,
    /// A raw or padded Wii U disc image. Real discs are encrypted with a
    /// per-title key from `keys.txt`, so this form is marked
    /// [`CemuContentSupport::RequiresKeys`] - and, in this build, also
    /// [`CemuContentSupport::UnsupportedPhase1`], since this module has no
    /// WUD container parser to even locate that title's key inside
    /// `keys.txt` safely.
    Wud,
    /// Cemu's own compressed disc-image container. Same encryption
    /// requirement and the same absent parser as `.wud`.
    Wux,
    /// Cemu's newer archive format (introduced well after this build's Wii
    /// U platform definition). Unsupported until its structure has been
    /// independently verified against a real Cemu build - see the module
    /// doc comment.
    Wua,
}

impl CemuContentForm {
    pub fn support(self) -> CemuContentSupport {
        match self {
            Self::ExtractedTitle => CemuContentSupport::RequiresDirectoryLayout,
            Self::Wud | Self::Wux => CemuContentSupport::RequiresKeys,
            Self::Wua => CemuContentSupport::UnsupportedPhase1,
        }
    }

    /// Whether this build will build a launch command for this form at
    /// all. Independent of [`Self::support`]'s classification label - a
    /// form can be labelled [`CemuContentSupport::RequiresKeys`] as an
    /// honest description of real Cemu behaviour while still being refused
    /// here because no safe parser for it exists yet.
    pub fn launchable_in_this_build(self) -> bool {
        matches!(self, Self::ExtractedTitle)
    }
}

pub fn form_for_path(path: &Path) -> Option<CemuContentForm> {
    if path.is_dir() {
        return Some(CemuContentForm::ExtractedTitle);
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "wud" => Some(CemuContentForm::Wud),
        "wux" => Some(CemuContentForm::Wux),
        "wua" => Some(CemuContentForm::Wua),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Extracted-title layout
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuLayoutErrorKind {
    NotADirectory,
    MissingCodeDirectory,
    MissingContentDirectory,
    MissingMetaDirectory,
    NoRpxFound,
    AmbiguousRpx,
    MetaXmlMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuLayoutError {
    pub kind: CemuLayoutErrorKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuExtractedLayout {
    pub root: PathBuf,
    pub code_dir: PathBuf,
    pub content_dir: PathBuf,
    pub meta_dir: PathBuf,
    pub rpx_path: PathBuf,
    pub meta_xml_path: Option<PathBuf>,
}

/// Validates the standard extracted Wii U title layout, bounded and
/// deterministic: exactly the three named subdirectories, and exactly one
/// `.rpx` file directly inside `code/` (never a recursive search of
/// arbitrary subdirectories, per the module's own scope). `meta.xml` is
/// preferred but not required to accept the layout - a title with no
/// `meta.xml` is still launchable, just without title identity evidence
/// (see [`inspect_cemu_launch_content`]).
pub fn inspect_extracted_layout(root: &Path) -> Result<CemuExtractedLayout, CemuLayoutError> {
    if !root.is_dir() {
        return Err(CemuLayoutError {
            kind: CemuLayoutErrorKind::NotADirectory,
            detail: format!("{} is not a directory", root.display()),
        });
    }
    let code_dir = root.join("code");
    if !code_dir.is_dir() {
        return Err(CemuLayoutError {
            kind: CemuLayoutErrorKind::MissingCodeDirectory,
            detail: "no code/ directory".to_string(),
        });
    }
    let content_dir = root.join("content");
    if !content_dir.is_dir() {
        return Err(CemuLayoutError {
            kind: CemuLayoutErrorKind::MissingContentDirectory,
            detail: "no content/ directory".to_string(),
        });
    }
    let meta_dir = root.join("meta");
    if !meta_dir.is_dir() {
        return Err(CemuLayoutError {
            kind: CemuLayoutErrorKind::MissingMetaDirectory,
            detail: "no meta/ directory".to_string(),
        });
    }
    let mut rpx_candidates: Vec<PathBuf> = fs::read_dir(&code_dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            regular(path)
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("rpx"))
        })
        .collect();
    rpx_candidates.sort();
    let rpx_path = match rpx_candidates.as_slice() {
        [one] => one.clone(),
        [] => {
            return Err(CemuLayoutError {
                kind: CemuLayoutErrorKind::NoRpxFound,
                detail: "no .rpx file directly inside code/".to_string(),
            });
        }
        _ => {
            return Err(CemuLayoutError {
                kind: CemuLayoutErrorKind::AmbiguousRpx,
                detail: format!("{} .rpx files directly inside code/", rpx_candidates.len()),
            });
        }
    };
    let meta_xml_path = meta_dir.join("meta.xml");
    let meta_xml_path = regular(&meta_xml_path).then_some(meta_xml_path);
    Ok(CemuExtractedLayout {
        root: root.to_path_buf(),
        code_dir,
        content_dir,
        meta_dir,
        rpx_path,
        meta_xml_path,
    })
}

// ---------------------------------------------------------------------------
// Title identity
// ---------------------------------------------------------------------------

/// A Wii U title's own self-reported identity, read from its `meta.xml`.
/// Self-reported, not cryptographically verified: `meta.xml` is plain,
/// editable text, so this is evidence a person can inspect, never a
/// trust anchor equivalent to a hash-verified identity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CemuTitleIdentity {
    pub title_id: Option<String>,
    pub product_code: Option<String>,
    pub company_code: Option<String>,
    pub title_version: Option<String>,
}

/// What a title's own ID declares itself to be, from the standard Wii U
/// title-ID convention (the third-from-last byte of the 64-bit ID):
/// `00050000` application, `0005000C` update, `0005000E` DLC. Evidence
/// only - Phase 1 launch planning refuses anything that does not declare
/// itself `Base` (see the module doc comment, "Phase 1 launch should
/// target the selected base game").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuTitleKind {
    Base,
    Update,
    Dlc,
    Unknown,
}

pub fn classify_title_kind(title_id: &str) -> CemuTitleKind {
    let normalised = title_id.trim().to_ascii_uppercase();
    if normalised.len() != 16 || !normalised.chars().all(|c| c.is_ascii_hexdigit()) {
        return CemuTitleKind::Unknown;
    }
    match &normalised[0..8] {
        "00050000" => CemuTitleKind::Base,
        "0005000C" => CemuTitleKind::Update,
        "0005000E" => CemuTitleKind::Dlc,
        _ => CemuTitleKind::Unknown,
    }
}

/// Reads and parses `meta.xml`, bounded by [`CEMU_MAX_META_XML_BYTES`].
/// `None` when the file is missing, oversized, not valid UTF-8, or names
/// none of the four known tags - never a partial/fabricated identity.
pub fn extract_title_identity(meta_xml_path: &Path) -> Option<CemuTitleIdentity> {
    let metadata = fs::symlink_metadata(meta_xml_path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    if metadata.len() > CEMU_MAX_META_XML_BYTES {
        return None;
    }
    let bytes = fs::read(meta_xml_path).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let tags = extract_flat_xml_tags(
        &text,
        &["title_id", "product_code", "company_code", "title_version"],
    );
    if tags.is_empty() {
        return None;
    }
    Some(CemuTitleIdentity {
        title_id: tags.get("title_id").cloned(),
        product_code: tags.get("product_code").cloned(),
        company_code: tags.get("company_code").cloned(),
        title_version: tags.get("title_version").cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn mark_exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut m = fs::metadata(p).unwrap().permissions();
        m.set_mode(0o755);
        fs::set_permissions(p, m).unwrap();
    }

    fn write_exe(path: &Path) {
        fs::write(path, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(path);
    }

    #[test]
    fn version_is_bounded_and_optional() {
        assert_eq!(parse_cemu_version("Cemu 2.0-135"), Some("2.0".into()));
        assert_eq!(parse_cemu_version("unknown"), None);
    }

    #[test]
    fn discovers_explicit_executable_and_mlc_from_settings_xml() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir_all(&root).unwrap();
        let exe = d.path().join("Cemu");
        write_exe(&exe);
        let mlc = d.path().join("mlc");
        fs::create_dir_all(&mlc).unwrap();
        fs::write(
            root.join("settings.xml"),
            format!("<content><mlc_path>{}</mlc_path></content>", mlc.display()),
        )
        .unwrap();
        let roots = CemuProfileDiscoveryRoots {
            home: d.path().into(),
            xdg_config_home: d.path().join("none"),
            explicit_configuration_roots: vec![root],
            portable_configuration_roots: vec![],
            explicit_executables: vec![exe],
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        };
        let discovery = discover_cemu_profiles(&roots);
        let p = &discovery.profiles[0];
        assert!(p.eligible);
        assert_eq!(p.config.as_ref().unwrap().mlc.state, CemuMlcState::Present);
        assert!(resolve_cemu_native_launch_binding(p).is_ok());
    }

    #[test]
    fn missing_mlc_path_is_reported_missing_not_present() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("settings.xml"),
            "<content><mlc_path>/does/not/exist</mlc_path></content>",
        )
        .unwrap();
        let config = inspect_config(&root.join("settings.xml"));
        assert_eq!(config.mlc.state, CemuMlcState::Missing);
    }

    #[test]
    fn no_mlc_tag_is_not_configured() {
        let d = tempdir().unwrap();
        fs::write(d.path().join("settings.xml"), "<content></content>").unwrap();
        let config = inspect_config(&d.path().join("settings.xml"));
        assert_eq!(config.mlc.state, CemuMlcState::NotConfigured);
    }

    #[test]
    fn keys_evidence_prefers_configuration_directory() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(KEYS_FILE_NAME), b"not-a-real-key").unwrap();
        let evidence = keys_evidence(&root, &[]);
        assert_eq!(evidence.state, CemuKeysState::PresentUnverified);
        assert_eq!(evidence.path, Some(root.join(KEYS_FILE_NAME)));
    }

    #[test]
    fn keys_evidence_is_not_configured_when_absent() {
        let d = tempdir().unwrap();
        let evidence = keys_evidence(d.path(), &[]);
        assert_eq!(evidence.state, CemuKeysState::NotConfigured);
        assert!(evidence.path.is_none());
    }

    #[test]
    fn extracted_layout_accepted_with_single_rpx() {
        let d = tempdir().unwrap();
        let root = d.path().join("Some Game [ABCE01]");
        fs::create_dir_all(root.join("code")).unwrap();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("meta")).unwrap();
        fs::write(root.join("code/game.rpx"), b"rpx").unwrap();
        let layout = inspect_extracted_layout(&root).unwrap();
        assert_eq!(layout.rpx_path, root.join("code/game.rpx"));
        assert!(layout.meta_xml_path.is_none());
    }

    #[test]
    fn ambiguous_rpx_is_refused() {
        let d = tempdir().unwrap();
        let root = d.path().join("game");
        fs::create_dir_all(root.join("code")).unwrap();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("meta")).unwrap();
        fs::write(root.join("code/a.rpx"), b"a").unwrap();
        fs::write(root.join("code/b.rpx"), b"b").unwrap();
        let error = inspect_extracted_layout(&root).unwrap_err();
        assert_eq!(error.kind, CemuLayoutErrorKind::AmbiguousRpx);
    }

    #[test]
    fn arbitrary_folder_without_layout_is_rejected() {
        let d = tempdir().unwrap();
        let root = d.path().join("random_folder");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("readme.txt"), b"hi").unwrap();
        let error = inspect_extracted_layout(&root).unwrap_err();
        assert_eq!(error.kind, CemuLayoutErrorKind::MissingCodeDirectory);
    }

    #[test]
    fn missing_content_or_meta_directory_is_rejected() {
        let d = tempdir().unwrap();
        let root = d.path().join("game");
        fs::create_dir_all(root.join("code")).unwrap();
        fs::write(root.join("code/game.rpx"), b"rpx").unwrap();
        let error = inspect_extracted_layout(&root).unwrap_err();
        assert_eq!(error.kind, CemuLayoutErrorKind::MissingContentDirectory);
    }

    #[test]
    fn title_identity_is_extracted_from_meta_xml() {
        let d = tempdir().unwrap();
        let meta = d.path().join("meta.xml");
        fs::write(
            &meta,
            "<menu><title_id>00050000101010ED</title_id>\
             <product_code>WUP-P-ARAE</product_code>\
             <company_code>01</company_code>\
             <title_version>16</title_version></menu>",
        )
        .unwrap();
        let identity = extract_title_identity(&meta).unwrap();
        assert_eq!(identity.title_id.as_deref(), Some("00050000101010ED"));
        assert_eq!(identity.product_code.as_deref(), Some("WUP-P-ARAE"));
        assert_eq!(
            classify_title_kind(identity.title_id.as_deref().unwrap()),
            CemuTitleKind::Base
        );
    }

    #[test]
    fn update_and_dlc_title_ids_are_classified_distinctly() {
        assert_eq!(
            classify_title_kind("0005000C101010ED"),
            CemuTitleKind::Update
        );
        assert_eq!(classify_title_kind("0005000E101010ED"), CemuTitleKind::Dlc);
        assert_eq!(
            classify_title_kind("not-a-title-id"),
            CemuTitleKind::Unknown
        );
    }

    #[test]
    fn missing_meta_xml_yields_no_identity_not_a_panic() {
        let d = tempdir().unwrap();
        assert!(extract_title_identity(&d.path().join("meta.xml")).is_none());
    }

    #[test]
    fn content_form_is_classified_from_shape() {
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join("dir")).unwrap();
        assert_eq!(
            form_for_path(&d.path().join("dir")),
            Some(CemuContentForm::ExtractedTitle)
        );
        assert_eq!(
            form_for_path(Path::new("/x/game.wud")),
            Some(CemuContentForm::Wud)
        );
        assert_eq!(
            form_for_path(Path::new("/x/game.wux")),
            Some(CemuContentForm::Wux)
        );
        assert_eq!(
            form_for_path(Path::new("/x/game.wua")),
            Some(CemuContentForm::Wua)
        );
        assert_eq!(form_for_path(Path::new("/x/game.iso")), None);
    }

    #[test]
    fn only_extracted_title_is_launchable_in_this_build() {
        assert!(CemuContentForm::ExtractedTitle.launchable_in_this_build());
        assert!(!CemuContentForm::Wud.launchable_in_this_build());
        assert!(!CemuContentForm::Wux.launchable_in_this_build());
        assert!(!CemuContentForm::Wua.launchable_in_this_build());
    }

    #[test]
    fn no_config_or_keys_file_is_ever_written_by_discovery() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir_all(&root).unwrap();
        let roots = CemuProfileDiscoveryRoots {
            home: d.path().into(),
            xdg_config_home: d.path().join("none"),
            explicit_configuration_roots: vec![root.clone()],
            portable_configuration_roots: vec![],
            explicit_executables: vec![],
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        };
        let _ = discover_cemu_profiles(&roots);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
    }
}
